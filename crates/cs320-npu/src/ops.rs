//! The NPU_OP_* operations (TRM §3.10, §3.11) over dense buffers.

#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

use crate::fm::{Elem, Storage};
use crate::stream::{align_up, decode_fwd, decode_swd, sign_extend, ScaleStream, WeightOrder};
use crate::{op0, op1, Npu, NpuBus, ScaleReg};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Rescale {
    None,
    Normal,
    Global,
    ShiftOnly,
    /// average pooling over `n` valid inputs
    AvPool(u32),
}

const TRANSPOSE_HWC: u32 = 0;
const TRANSPOSE_WHC: u32 = 1;
const TRANSPOSE_HCW: u32 = 2;
const TRANSPOSE_WCH: u32 = 3;
const TRANSPOSE_CHW: u32 = 6;
const TRANSPOSE_CWH: u32 = 7;

/// Rounded arithmetic right shift (TRM §3.6.6 `rounded_shift_right`).
#[inline]
pub fn rounded_shift_right(acc: i128, shift: u32, mode: u32, dbl_rnd: u32) -> i64 {
    let shift = shift.min(63);
    if shift == 0 {
        return acc as i64;
    }
    let mut r: i128 = 1i128 << (shift - 1);
    let dbl_rnd = dbl_rnd.min(30);
    let d: i128 = if shift > 31 - dbl_rnd { 1i128 << (30 - dbl_rnd) } else { 0 };
    match mode {
        0 => r = if acc >= 0 { r + d } else { r - d },
        2 => r += d,
        4 => r = if acc >= 0 { 0 } else { (1i128 << shift) - 1 },
        5 => r = 0,
        3 if acc < 0 => r -= 1,
        _ => {}
    }
    ((acc + r) >> shift) as i64
}

#[inline]
fn truncate(v: i64, bits: u32, signed: bool) -> i64 {
    if bits >= 64 {
        v
    } else if signed {
        sign_extend(v, bits)
    } else {
        v & ((1i64 << bits) - 1)
    }
}

#[inline]
fn dot_i32(a: &[i16], b: &[i16]) -> i32 {
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let mut s = 0i32;
    for i in 0..n {
        s = s.wrapping_add(a[i] as i32 * b[i] as i32);
    }
    s
}

#[inline]
fn dot_i64(a: &[i16], b: &[i16]) -> i64 {
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let mut s = 0i64;
    for i in 0..n {
        s += a[i] as i64 * b[i] as i64;
    }
    s
}

#[inline]
fn clz32(a: i64) -> i64 {
    (a as u32).leading_zeros() as i64
}

struct AccumCtx {
    acc48: bool,
    acc_input: u32,
    acc_out: bool,
    scales: Option<ScaleStream>,
    per_channel: bool,
    ofm_scale: ScaleReg,
    /// IFM2 read as the accumulator input (acc_input == ifm2)
    acc_in: Vec<i64>,
    ow: usize,
    od: usize,
}

impl Npu {
    fn microblock_depth(&self) -> Result<usize, String> {
        let m = self.cfg.macs as usize;
        Ok(match self.st.microblock() {
            0 => m / 8,
            1 => m / 16,
            2 | 3 => m / 32,
            4 => m / 64,
            5 => m / 128,
            x => return Err(format!("reserved microblock {x}")),
        })
    }

    fn add_operation_to_chain(&mut self, use_ifm: bool, use_ifm2: bool, append: bool) {
        if self.chain_len != 0 && !append {
            self.warn("operation that cannot be chained inside a chain");
            self.chain_len = 0;
        }
        if self.chain_len == 0 {
            self.num_ifm = 0;
            self.fifo_valid = [false; 3];
        }
        self.chain_len += 1;
        if self.chain_len > 4 {
            self.warn("chain longer than 4 operations");
        }
        let ifm = self.st.precision(op0::IFM_PRECISION);
        let ifm2 = self.st.precision(op0::IFM2_PRECISION);
        let ofm = self.st.precision(op0::OFM_PRECISION);
        if use_ifm {
            if ifm.storage == Storage::Chained {
                let r = (self.st.p0(op0::IFM_REGION) & 7) as usize;
                if r >= 3 || !self.fifo_valid[r] {
                    self.warn("IFM reads a chaining buffer that no operation wrote");
                }
            } else if ifm.storage != Storage::None {
                self.num_ifm += 1;
            }
        }
        if use_ifm2 {
            if ifm2.storage == Storage::Chained {
                let r = (self.st.p0(op0::IFM2_REGION) & 7) as usize;
                if r >= 3 || !self.fifo_valid[r] {
                    self.warn("IFM2 reads a chaining buffer that no operation wrote");
                }
            } else if ifm2.storage != Storage::None {
                self.num_ifm += 1;
            }
        }
        if self.num_ifm > 3 {
            self.warn("more than three memory inputs in a chain");
        }
        if ofm.storage == Storage::Chained {
            let r = (self.st.p0(op0::OFM_REGION) & 7) as usize;
            if r < 3 {
                self.fifo_valid[r] = true;
                self.fifo[r].clear();
                self.fifo_rd[r] = 0;
            } else {
                self.warn("OFM chained to a region above 2");
            }
        } else {
            self.chain_len = 0;
        }
    }

    /// Reads the `[h][w][c]` input tensor `which` (0 = IFM, 2 = IFM2) with its zero point
    /// subtracted, from memory or from its chaining buffer.
    fn read_input<T: Elem>(&mut self, bus: &mut dyn NpuBus, which: u32, h: usize, w: usize, c: usize, out: &mut Vec<T>) {
        let fm = self.st.fm(which);
        let n = h * w * c;
        if fm.storage == Storage::Chained {
            out.clear();
            out.resize(n, T::default());
            let r = fm.region;
            if r < 3 {
                let avail = self.fifo[r].len().saturating_sub(self.fifo_rd[r]);
                let take = n.min(avail);
                for (o, v) in out.iter_mut().zip(&self.fifo[r][self.fifo_rd[r]..self.fifo_rd[r] + take]) {
                    *o = T::from_i64(*v - fm.zero_point);
                }
                self.fifo_rd[r] += take;
                if take < n {
                    self.warn("chaining buffer underflow");
                }
            }
            self.cycles += (n as u64) / 64;
            return;
        }
        if fm.storage == Storage::None {
            out.clear();
            out.resize(n, T::default());
            return;
        }
        let base = self.basep[fm.region];
        if !fm.read(bus, base, h, w, c, out) {
            self.warn("feature map read outside memory");
        }
        self.cycles += (n * fm.esize) as u64 / 32;
    }

    /// Writes the `[oh][ow][od]` output (TRM `write_ofm`): truncation to the OFM type, the
    /// condition flag, chaining, reverse and transpose.
    fn write_ofm(&mut self, bus: &mut dyn NpuBus, out: &mut [i64], allowed_all: bool) {
        let (oh, ow, od) = self.st.ofm_dims();
        let p = self.st.precision(op0::OFM_PRECISION);
        for v in out.iter_mut() {
            *v = truncate(*v, p.bits, p.signed);
        }
        if oh == 1 && ow == 1 && od == 1 {
            self.cond_status = (out[0] & 1) as u32;
        }
        if self.trace && out.len() <= 64 {
            log::trace!("npu ofm {}x{}x{} {:?}: {:?}", oh, ow, od, p.storage, out);
        }
        match p.storage {
            Storage::Chained => {
                let r = (self.st.p0(op0::OFM_REGION) & 7) as usize;
                if r < 3 {
                    self.fifo[r].extend_from_slice(out);
                }
                self.cycles += out.len() as u64 / 64;
                return;
            }
            Storage::None => return,
            _ => {}
        }
        let fm = self.st.fm(1);
        let base = self.basep[fm.region];
        let reverse = self.st.ofm_reverse();
        let transpose = self.st.ofm_transpose();
        self.cycles += (out.len() * fm.esize) as u64 / 32;
        if reverse == 0 && transpose == TRANSPOSE_HWC {
            if !fm.write(bus, base, oh, ow, od, out) {
                self.warn("feature map write outside memory");
            }
            return;
        }
        if !allowed_all && !matches!(transpose, TRANSPOSE_HWC | TRANSPOSE_HCW | TRANSPOSE_CHW) {
            self.warn("transpose not permitted for this operation");
        }
        if reverse != 0 && (transpose != TRANSPOSE_HWC || !allowed_all) {
            self.warn("reverse combined with transpose");
        }
        let mut ok = true;
        for y0 in 0..oh {
            for x0 in 0..ow {
                for c0 in 0..od {
                    let v = out[(y0 * ow + x0) * od + c0];
                    let (mut y, mut x, mut c) = (y0, x0, c0);
                    match reverse {
                        1 => y = oh - 1 - y,
                        2 => x = ow - 1 - x,
                        3 => c = od - 1 - c,
                        _ => {}
                    }
                    match transpose {
                        TRANSPOSE_WHC => std::mem::swap(&mut y, &mut x),
                        TRANSPOSE_HCW => std::mem::swap(&mut x, &mut c),
                        TRANSPOSE_CWH => std::mem::swap(&mut y, &mut c),
                        TRANSPOSE_CHW => {
                            let t = y;
                            y = c;
                            c = x;
                            x = t;
                        }
                        TRANSPOSE_WCH => {
                            let t = y;
                            y = x;
                            x = c;
                            c = t;
                        }
                        _ => {}
                    }
                    ok &= fm.write_elem(bus, base, y, x, c, v);
                }
            }
        }
        if !ok {
            self.warn("feature map write outside memory");
        }
    }

    fn read_region(&mut self, bus: &mut dyn NpuBus, region: usize, offset: u64, len: usize) -> Vec<u8> {
        let mut v = vec![0u8; len];
        let a = self.region_addr(region, offset);
        if !bus.read(a, &mut v) {
            self.warn("stream read outside memory");
        }
        self.cycles += len as u64 / 32;
        v
    }

    /// Decodes the weight streams of a CONV (`[oc][ky][kx][ic]`) or DEPTHWISE (`[ky][kx][c]`)
    /// kernel into dense weights; `kh`/`kw` are the undilated kernel dimensions.
    fn load_weights(&mut self, bus: &mut dyn NpuBus, depthwise: bool, kh: usize, kw: usize, ifm16: bool) -> Result<Vec<i16>, String> {
        let (sx, sy, part_kernel, dil_x, dil_y, _) = self.st.kernel_stride();
        let (_, _, od) = self.st.ofm_dims();
        let ic_n = if depthwise { 1 } else { self.st.p0(op0::IFM_DEPTH_M1) as usize + 1 };
        let wf = self.st.p0(op0::WEIGHT_FORMAT);
        let fwd = wf & 1 != 0;
        let sparse = (wf >> 4) & 1 != 0;
        let ublk = self.microblock_depth()?;
        let streams = self.cfg.num_wd();
        let mb_i16only = ublk == 16 && self.st.microblock() <= 1;
        let ifm_block_depth = if part_kernel || depthwise {
            16
        } else if !sparse && mb_i16only {
            32
        } else if !sparse || ifm16 {
            64
        } else {
            128
        };
        let order = WeightOrder {
            streams,
            depthwise,
            part_kernel: part_kernel || depthwise,
            sparse,
            ifm16,
            ofm_depth: od,
            ifm_depth: ic_n,
            ofm_block_depth: self.st.ofm_block().2,
            ifm_block_depth,
            ofm_ublock_depth: ublk,
            kernel_w: kw,
            kernel_h: kh,
            stride_x: sx,
            stride_y: sy,
            dilation_x: dil_x,
            dilation_y: dil_y,
            dw_pad: self.cfg.dw_pad(ifm16),
        };
        let region = (self.st.p0(op0::WEIGHT_REGION) & 7) as usize;
        let mut decoded: Vec<Vec<i16>> = Vec::with_capacity(streams);
        if fwd {
            if depthwise {
                self.warn("fast weight decoder used for a depthwise kernel");
            }
            let len = self.st.p1(op1::WEIGHT_LENGTH) as usize & 0xFFFF_FFFF;
            let data = self.read_region(bus, region, self.st.addr(op1::WEIGHT_BASE), len);
            let all = decode_fwd(&data)?;
            if streams == 1 {
                decoded.push(all);
            } else {
                let blk = order.fwd_block().max(1);
                decoded.resize(streams, Vec::new());
                for (i, chunk) in all.chunks(blk).enumerate() {
                    decoded[i % streams].extend_from_slice(chunk);
                }
            }
        } else {
            for s in 0..streams {
                let (base_op, len_op) = if s == 0 {
                    (op1::WEIGHT_BASE, op1::WEIGHT_LENGTH)
                } else {
                    (op1::WEIGHT1_BASE + 2 * (s as u32 - 1), op1::WEIGHT1_LENGTH + 2 * (s as u32 - 1))
                };
                let len = self.st.p1(len_op) as usize & 0xFFFF_FFFF;
                let data = self.read_region(bus, region, self.st.addr(base_op), len);
                decoded.push(decode_swd(&data, usize::MAX)?);
            }
        }
        let mut w = vec![0i16; if depthwise { kh * kw * od } else { od * kh * kw * ic_n }];
        let mut idx = vec![0usize; streams];
        let mut short = false;
        order.visit(|s, oc, ky, kx, ic| {
            let v = match decoded[s].get(idx[s]) {
                Some(&v) => v,
                None => {
                    short = true;
                    0
                }
            };
            idx[s] += 1;
            if oc != usize::MAX {
                let i = if depthwise { (ky * kw + kx) * od + oc } else { ((oc * kh + ky) * kw + kx) * ic_n + ic };
                if let Some(slot) = w.get_mut(i) {
                    *slot = v;
                }
            }
        });
        if short {
            self.warn("weight stream shorter than the kernel needs");
        }
        Ok(w)
    }

    /// Reads the scale and bias stream for the output channels.
    fn load_scales(&mut self, bus: &mut dyn NpuBus) -> Result<Option<ScaleStream>, String> {
        let len = (self.st.p1(op1::SCALE_LENGTH) & 0xF_FFFF) as usize;
        if len == 0 {
            return Ok(None);
        }
        let region = (self.st.p0(op0::SCALE_REGION) & 7) as usize;
        let data = self.read_region(bus, region, self.st.addr(op1::SCALE_BASE), len);
        let (_, _, od) = self.st.ofm_dims();
        let set = align_up(self.microblock_depth()?, 16);
        Ok(Some(ScaleStream::parse(&data, od, set, self.st.acc48(), self.st.ofm_reverse() == 3)))
    }

    fn accum_ctx(&mut self, bus: &mut dyn NpuBus, with_scales: bool) -> Result<AccumCtx, String> {
        let (oh, ow, od) = self.st.ofm_dims();
        let acc_input = self.st.acc_input();
        let mut acc_in = Vec::new();
        if acc_input == 2 {
            let prec = self.st.precision(op0::IFM2_PRECISION);
            let want = if self.st.acc48() { 64 } else { 32 };
            if prec.bits != want || !prec.signed {
                self.warn("IFM2 precision does not match the accumulator format");
            }
            self.read_input::<i64>(bus, 2, oh, ow, od, &mut acc_in);
        }
        let per_channel = self.st.scale_mode_per_channel();
        let scales = if with_scales || per_channel { self.load_scales(bus)? } else { None };
        Ok(AccumCtx {
            acc48: self.st.acc48(),
            acc_input,
            acc_out: !self.st.acc_output_disabled(),
            scales,
            per_channel,
            ofm_scale: self.st.scale_reg(op1::OFM_SCALE),
            acc_in,
            ow,
            od,
        })
    }

    #[inline]
    fn read_accumulator(&mut self, cx: &AccumCtx, oy: usize, ox: usize, oc: usize, reset: i64) -> i64 {
        match cx.acc_input {
            1 => {
                let (bh, bw, bd) = self.acc_dims;
                if oy < bh && ox < bw && oc < bd {
                    self.acc[(oy * bw + ox) * bd + oc]
                } else {
                    0
                }
            }
            2 => cx.acc_in.get((oy * cx.ow + ox) * cx.od + oc).copied().unwrap_or(0),
            _ => reset,
        }
    }

    #[inline]
    fn accumulate(acc: i64, v: i64, acc48: bool) -> i64 {
        let s = acc.wrapping_add(v);
        if acc48 {
            sign_extend(s, 48)
        } else {
            s as i32 as i64
        }
    }

    /// TRM `rescale`: `oc` is the output channel for per-channel scaling.
    fn rescale(&self, cx: &AccumCtx, acc: i64, oc: usize, mode: Rescale) -> i64 {
        if mode == Rescale::None {
            return acc;
        }
        let (scale, shift): (i64, u32) = match mode {
            Rescale::AvPool(n) => {
                let d = n.max(1) as i64;
                let k = 64 - ((d - 1) as u64).leading_zeros(); // (1<<k)/2 < d <= (1<<k)
                let numerator = ((1i128 << 30) + 1) << k;
                ((numerator / d as i128) as i64, 30 + k)
            }
            Rescale::Normal if cx.per_channel => match &cx.scales {
                Some(s) if oc < s.scale.len() => (s.scale[oc] as i64, s.shift[oc]),
                _ => (cx.ofm_scale.scale as i64, cx.ofm_scale.shift),
            },
            _ => (cx.ofm_scale.scale as i64, cx.ofm_scale.shift),
        };
        let result: i128 = if mode != Rescale::ShiftOnly { acc as i128 * scale as i128 } else { acc as i128 };
        rounded_shift_right(result, shift, cx.ofm_scale.round_mode, cx.ofm_scale.dbl_rnd)
    }

    /// TRM `activation`: zero point, clipping and the lookup tables.
    fn activation(&self, acc64: i64) -> i64 {
        let p = self.st.precision(op0::OFM_PRECISION);
        if p.bits == 64 {
            return acc64;
        }
        let (zero_point, act_min, act_max) = if p.signed {
            (
                self.st.p0(op0::OFM_ZERO_POINT) as u16 as i16 as i64,
                self.st.p0(op0::ACTIVATION_MIN) as u16 as i16 as i64,
                self.st.p0(op0::ACTIVATION_MAX) as u16 as i16 as i64,
            )
        } else {
            (self.st.p0(op0::OFM_ZERO_POINT) as i64, self.st.p0(op0::ACTIVATION_MIN) as i64, self.st.p0(op0::ACTIVATION_MAX) as i64)
        };
        let act = self.st.p0(op0::ACTIVATION);
        let function = act & 0x1F;
        let table = ((act >> 5) & 7) as usize;
        let clip_none = (act >> 12) & 1 != 0;
        let mut acc = (acc64.wrapping_add(zero_point)) as i32 as i64;
        if function == 10 {
            acc = acc.clamp(-0x10000, 0xFFFF);
        } else if function == 11 {
            acc = acc.clamp(-0x1F000, 0x1EFFF);
        } else if !clip_none {
            acc = acc.clamp(act_min, act_max);
        }
        let lut = &self.lut;
        let lut16 = |i: usize| i16::from_le_bytes([lut[(2 * i) & (crate::LUT_SIZE - 1)], lut[(2 * i + 1) & (crate::LUT_SIZE - 1)]]) as i64;
        match function {
            0 => {}
            1 => acc = lut[((table << 8) + (acc & 0xFF) as usize) & (crate::LUT_SIZE - 1)] as i64,
            4 => acc = lut[((table << 8) + ((acc + 128) & 0xFF) as usize) & (crate::LUT_SIZE - 1)] as i8 as i64,
            5 => acc = lut16((table << 8) + ((acc + 128) & 0xFF) as usize),
            7 => {
                let i = ((table << 8) + ((acc + 128) & 0xFF) as usize) * 4;
                let i = i & (crate::LUT_SIZE - 1);
                acc = i32::from_le_bytes([lut[i], lut[i + 1], lut[i + 2], lut[i + 3]]) as i64;
            }
            8 => {
                let index = (256 + (acc >> 7)) as usize;
                let base = lut16(2 * index);
                let slope = lut16(2 * index + 1);
                acc = ((base << 7) + (acc & 0x7F) * slope + 64) >> 7;
                acc = acc as i16 as i64;
            }
            9 => {
                let index = (256 + (acc >> 7)) as usize;
                let base = lut16(2 * index);
                let slope = lut16(2 * index + 1);
                acc = (base << 7) + (acc & 0x7F) * slope;
            }
            10 => {
                let index = (256 + (acc >> 8)) as usize;
                let base = lut16(2 * index);
                let slope = lut16(2 * index + 1);
                let round = if acc >= 0 { 0x80 } else { 0x7F };
                acc = ((base << 8) + (acc & 0xFF) * slope + round) >> 8;
            }
            11 => {
                let index = (256 + (acc >> 9)) as usize;
                let base = lut16(2 * index);
                let slope = lut16(2 * index + 1);
                let round = if acc >= 0 { 0x100_0200 } else { 0x100_01FF };
                acc = ((base << 9) + (acc & 0x1FF) * slope + round) >> 10;
            }
            _ => {}
        }
        acc
    }

    /// TRM `write_accumulator`.
    #[inline]
    fn write_accumulator(&mut self, cx: &AccumCtx, out: &mut [i64], oy: usize, ox: usize, oc: usize, acc: i64, mode: Rescale) {
        if cx.acc_out {
            let bias = cx.scales.as_ref().and_then(|s| s.bias.get(oc).copied()).unwrap_or(0);
            let acc = Self::accumulate(acc, bias, cx.acc48);
            let v = self.rescale(cx, acc, oc, mode);
            let v = self.activation(v);
            out[(oy * cx.ow + ox) * cx.od + oc] = v;
        } else {
            let (bh, bw, bd) = self.st.ofm_block();
            if self.acc_dims != (bh, bw, bd) {
                self.acc_dims = (bh, bw, bd);
                self.acc.clear();
                self.acc.resize(bh * bw * bd, 0);
            }
            if oy < bh && ox < bw && oc < bd {
                self.acc[(oy * bw + ox) * bd + oc] = acc;
            }
        }
    }

    pub(crate) fn op_conv(&mut self, bus: &mut dyn NpuBus, depthwise: bool, weights_ifm2: bool) -> Result<(), String> {
        let (oh, ow, od) = self.st.ofm_dims();
        let dilated_h = self.st.p0(op0::KERNEL_HEIGHT_M1) as usize + 1;
        let dilated_w = self.st.p0(op0::KERNEL_WIDTH_M1) as usize + 1;
        let (sx, sy, _, dil_x, dil_y, _) = self.st.kernel_stride();
        let kh = dilated_h.div_ceil(dil_y);
        let kw = dilated_w.div_ceil(dil_x);
        let pad_t = self.st.p0(op0::IFM_PAD_TOP) as usize & 0x7F;
        let pad_l = self.st.p0(op0::IFM_PAD_LEFT) as usize & 0x7F;
        let pad_r = self.st.p0(op0::IFM_PAD_RIGHT) as usize & 0xFF;
        let pad_b = self.st.p0(op0::IFM_PAD_BOTTOM) as usize & 0xFF;
        let ifm_h = ((oh - 1) * sy + dilated_h) as isize - (pad_t + pad_b) as isize;
        let ifm_w = ((ow - 1) * sx + dilated_w) as isize - (pad_l + pad_r) as isize;
        let ic_n = if depthwise { od } else { self.st.p0(op0::IFM_DEPTH_M1) as usize + 1 };
        let use_ifm2 = weights_ifm2 || self.st.acc_input() == 2;
        self.add_operation_to_chain(true, use_ifm2, false);
        if ifm_h < 1 || ifm_w < 1 {
            return Err("convolution padding larger than the input".into());
        }
        let (ifm_h, ifm_w) = (ifm_h as usize, ifm_w as usize);
        let ifm_p = self.st.precision(op0::IFM_PRECISION);
        if !(ifm_p.bits == 8 || (ifm_p.bits == 16 && ifm_p.signed)) {
            self.warn("convolution with an unsupported IFM precision");
        }
        let ifm16 = ifm_p.bits == 16;
        // IFM, optionally upscaled 2x
        let upscale = self.st.p0(op0::IFM_UPSCALE) & 3;
        let mut ifm: Vec<i16> = Vec::new();
        if upscale == 0 {
            self.read_input::<i16>(bus, 0, ifm_h, ifm_w, ic_n, &mut ifm);
        } else {
            let (bh, bw) = (ifm_h.div_ceil(2), ifm_w.div_ceil(2));
            let mut base: Vec<i16> = Vec::new();
            self.read_input::<i16>(bus, 0, bh, bw, ic_n, &mut base);
            ifm.resize(ifm_h * ifm_w * ic_n, 0);
            for y in 0..ifm_h {
                for x in 0..ifm_w {
                    if upscale == 2 && (x & 1 == 1 || y & 1 == 1) {
                        continue;
                    }
                    let src = &base[((y >> 1) * bw + (x >> 1)) * ic_n..][..ic_n];
                    ifm[(y * ifm_w + x) * ic_n..][..ic_n].copy_from_slice(src);
                }
            }
        }
        // zero-padded IFM
        let ph = pad_t + ifm_h + pad_b;
        let pw = pad_l + ifm_w + pad_r;
        let mut pin = vec![0i16; ph * pw * ic_n];
        for y in 0..ifm_h {
            let src = &ifm[y * ifm_w * ic_n..(y + 1) * ifm_w * ic_n];
            let dst = &mut pin[((y + pad_t) * pw + pad_l) * ic_n..][..ifm_w * ic_n];
            dst.copy_from_slice(src);
        }
        drop(ifm);
        // weights
        let w: Vec<i16> = if weights_ifm2 {
            if dilated_h != 1 || dilated_w != 1 || oh != 1 {
                self.warn("weights from IFM2 with a kernel other than 1x1");
            }
            let mut m: Vec<i16> = Vec::new();
            self.read_input::<i16>(bus, 2, 1, ic_n, od, &mut m);
            let mut w = vec![0i16; od * ic_n];
            for ic in 0..ic_n {
                for oc in 0..od {
                    w[oc * ic_n + ic] = m[ic * od + oc];
                }
            }
            w
        } else {
            self.load_weights(bus, depthwise, kh, kw, ifm16)?
        };
        let cx = self.accum_ctx(bus, true)?;
        let acc48 = cx.acc48;
        let mut out = vec![0i64; if cx.acc_out { oh * ow * od } else { 0 }];
        let macs = (oh * ow * od * kh * kw * ic_n) as u64;
        let mac_cycles = macs.div_ceil(self.cfg.macs as u64);
        self.mac_cycles += mac_cycles;
        self.cycles += mac_cycles + 64;
        let kwc = kw * ic_n;
        if !depthwise {
            for oy in 0..oh {
                for ox in 0..ow {
                    let iy0 = oy * sy;
                    let ix0 = ox * sx;
                    for oc in 0..od {
                        let wk = &w[oc * kh * kwc..(oc + 1) * kh * kwc];
                        let mut acc = self.read_accumulator(&cx, oy, ox, oc, 0);
                        if acc48 {
                            let mut s = 0i64;
                            for ky in 0..kh {
                                let iy = iy0 + ky * dil_y;
                                let wrow = &wk[ky * kwc..(ky + 1) * kwc];
                                if dil_x == 1 {
                                    s += dot_i64(&pin[(iy * pw + ix0) * ic_n..][..kwc], wrow);
                                } else {
                                    for kx in 0..kw {
                                        let ix = ix0 + kx * dil_x;
                                        s += dot_i64(&pin[(iy * pw + ix) * ic_n..][..ic_n], &wrow[kx * ic_n..(kx + 1) * ic_n]);
                                    }
                                }
                            }
                            acc = sign_extend(acc.wrapping_add(s), 48);
                        } else {
                            let mut s = acc as i32;
                            for ky in 0..kh {
                                let iy = iy0 + ky * dil_y;
                                let wrow = &wk[ky * kwc..(ky + 1) * kwc];
                                if dil_x == 1 {
                                    s = s.wrapping_add(dot_i32(&pin[(iy * pw + ix0) * ic_n..][..kwc], wrow));
                                } else {
                                    for kx in 0..kw {
                                        let ix = ix0 + kx * dil_x;
                                        s = s.wrapping_add(dot_i32(
                                            &pin[(iy * pw + ix) * ic_n..][..ic_n],
                                            &wrow[kx * ic_n..(kx + 1) * ic_n],
                                        ));
                                    }
                                }
                            }
                            acc = s as i64;
                        }
                        self.write_accumulator(&cx, &mut out, oy, ox, oc, acc, Rescale::Normal);
                    }
                }
            }
        } else {
            let mut accs = vec![0i64; od];
            let mut accs32 = vec![0i32; od];
            for oy in 0..oh {
                for ox in 0..ow {
                    let iy0 = oy * sy;
                    let ix0 = ox * sx;
                    for oc in 0..od {
                        accs[oc] = self.read_accumulator(&cx, oy, ox, oc, 0);
                    }
                    if acc48 {
                        for ky in 0..kh {
                            let iy = iy0 + ky * dil_y;
                            for kx in 0..kw {
                                let ix = ix0 + kx * dil_x;
                                let row = &pin[(iy * pw + ix) * od..][..od];
                                let wk = &w[(ky * kw + kx) * od..][..od];
                                for c in 0..od {
                                    accs[c] += row[c] as i64 * wk[c] as i64;
                                }
                            }
                        }
                        for a in accs.iter_mut() {
                            *a = sign_extend(*a, 48);
                        }
                    } else {
                        for c in 0..od {
                            accs32[c] = accs[c] as i32;
                        }
                        for ky in 0..kh {
                            let iy = iy0 + ky * dil_y;
                            for kx in 0..kw {
                                let ix = ix0 + kx * dil_x;
                                let row = &pin[(iy * pw + ix) * od..][..od];
                                let wk = &w[(ky * kw + kx) * od..][..od];
                                for c in 0..od {
                                    accs32[c] = accs32[c].wrapping_add(row[c] as i32 * wk[c] as i32);
                                }
                            }
                        }
                        for c in 0..od {
                            accs[c] = accs32[c] as i64;
                        }
                    }
                    for oc in 0..od {
                        let acc = accs[oc];
                        self.write_accumulator(&cx, &mut out, oy, ox, oc, acc, Rescale::Normal);
                    }
                }
            }
        }
        if cx.acc_out {
            self.write_ofm(bus, &mut out, true);
        }
        Ok(())
    }

    pub(crate) fn op_pool(&mut self, bus: &mut dyn NpuBus, mode: u32) -> Result<(), String> {
        const MAX: u32 = 0;
        const AVERAGE: u32 = 1;
        const REDUCE_SUM: u32 = 2;
        const SUM: u32 = 3;
        const NONE: u32 = 4;
        const MIN: u32 = 5;
        const ARGMAX_X: u32 = 6;
        const ARGMAX_Y: u32 = 7;
        let (oh, ow, od) = self.st.ofm_dims();
        let kh = self.st.p0(op0::KERNEL_HEIGHT_M1) as usize + 1;
        let kw = self.st.p0(op0::KERNEL_WIDTH_M1) as usize + 1;
        let (sx, sy, _, _, _, _) = self.st.kernel_stride();
        let pad_t = self.st.p0(op0::IFM_PAD_TOP) as isize & 0x7F;
        let pad_l = self.st.p0(op0::IFM_PAD_LEFT) as isize & 0x7F;
        let pad_r = self.st.p0(op0::IFM_PAD_RIGHT) as isize & 0xFF;
        let pad_b = self.st.p0(op0::IFM_PAD_BOTTOM) as isize & 0xFF;
        let ifm_h = ((oh - 1) * sy + kh) as isize - pad_t - pad_b;
        let ifm_w = ((ow - 1) * sx + kw) as isize - pad_l - pad_r;
        let use_ifm = mode != NONE;
        let use_ifm2 = self.st.acc_input() == 2;
        self.add_operation_to_chain(use_ifm, use_ifm2, false);
        if ifm_h < 1 || ifm_w < 1 {
            return Err("pooling padding larger than the input".into());
        }
        let (ifm_h, ifm_w) = (ifm_h as usize, ifm_w as usize);
        let ic_n = if mode == REDUCE_SUM { self.st.p0(op0::IFM_DEPTH_M1) as usize + 1 } else { od };
        let upscale = self.st.p0(op0::IFM_UPSCALE) & 3;
        let mut ifm: Vec<i32> = Vec::new();
        if use_ifm {
            if upscale == 0 {
                self.read_input::<i32>(bus, 0, ifm_h, ifm_w, ic_n, &mut ifm);
            } else {
                let (bh, bw) = (ifm_h.div_ceil(2), ifm_w.div_ceil(2));
                let mut base: Vec<i32> = Vec::new();
                self.read_input::<i32>(bus, 0, bh, bw, ic_n, &mut base);
                ifm.resize(ifm_h * ifm_w * ic_n, 0);
                for y in 0..ifm_h {
                    for x in 0..ifm_w {
                        if upscale == 2 && (x & 1 == 1 || y & 1 == 1) {
                            continue;
                        }
                        let src = &base[((y >> 1) * bw + (x >> 1)) * ic_n..][..ic_n];
                        ifm[(y * ifm_w + x) * ic_n..][..ic_n].copy_from_slice(src);
                    }
                }
            }
        }
        // pooling fetches the scale and bias stream only for per-channel scaling (the FVP adds no
        // bias to a reduce_sum with global scaling)
        let cx = self.accum_ctx(bus, false)?;
        let acc48 = cx.acc48;
        let mut out = vec![0i64; if cx.acc_out { oh * ow * od } else { 0 }];
        self.cycles += (oh * ow * ic_n * kh * kw) as u64 / (self.cfg.macs as u64 / 16).max(1) + 64;
        if mode == REDUCE_SUM {
            if od != 1 {
                self.warn("reduce_sum with an OFM depth other than 1");
            }
            for oy in 0..oh {
                for ox in 0..ow {
                    let mut acc = self.read_accumulator(&cx, oy, ox, 0, 0);
                    let row = &ifm[(oy * ifm_w + ox) * ic_n..][..ic_n];
                    for &v in row {
                        acc = Self::accumulate(acc, v as i64, acc48);
                    }
                    self.write_accumulator(&cx, &mut out, oy, ox, 0, acc, Rescale::Normal);
                }
            }
        } else {
            let identity: i64 = match mode {
                MAX | ARGMAX_X | ARGMAX_Y => -(1i64 << 31),
                MIN => (1i64 << 31) - 1,
                _ => 0,
            };
            let mut accs = vec![0i64; od];
            for oy in 0..oh {
                for ox in 0..ow {
                    for c in 0..od {
                        accs[c] = self.read_accumulator(&cx, oy, ox, c, identity);
                    }
                    let mut count = 0u32;
                    for ky in 0..kh {
                        let iy = -pad_t + (oy * sy + ky) as isize;
                        if iy < 0 || iy >= ifm_h as isize {
                            continue;
                        }
                        for kx in 0..kw {
                            let ix = -pad_l + (ox * sx + kx) as isize;
                            if ix < 0 || ix >= ifm_w as isize {
                                continue;
                            }
                            count += 1;
                            if !use_ifm {
                                continue;
                            }
                            let row = &ifm[(iy as usize * ifm_w + ix as usize) * od..][..od];
                            match mode {
                                MAX => {
                                    for c in 0..od {
                                        accs[c] = accs[c].max(row[c] as i64);
                                    }
                                }
                                MIN => {
                                    for c in 0..od {
                                        accs[c] = accs[c].min(row[c] as i64);
                                    }
                                }
                                AVERAGE | SUM => {
                                    for c in 0..od {
                                        accs[c] = Self::accumulate(accs[c], row[c] as i64, acc48);
                                    }
                                }
                                ARGMAX_X => {
                                    for c in 0..od {
                                        if row[c] as i64 > sign_extend(accs[c], 16) {
                                            accs[c] = row[c] as i64 + 65536 * kx as i64;
                                        }
                                    }
                                }
                                ARGMAX_Y => {
                                    for c in 0..od {
                                        if row[c] as i64 > sign_extend(accs[c], 16) {
                                            accs[c] = row[c] as i64 + 65536 * ky as i64;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    let rmode = if mode == AVERAGE { Rescale::AvPool(count) } else { Rescale::Normal };
                    for c in 0..od {
                        let acc = accs[c];
                        self.write_accumulator(&cx, &mut out, oy, ox, c, acc, rmode);
                    }
                }
            }
        }
        if cx.acc_out {
            self.write_ofm(bus, &mut out, true);
        }
        Ok(())
    }

    /// TRM `scale_a` / `scale_b`.
    #[inline]
    fn scale_input(&self, v: i64, s: &ScaleReg, apply: bool) -> i64 {
        if !apply {
            return v;
        }
        let r = v as i128 * s.scale as i128;
        rounded_shift_right(r, s.shift, s.round_mode, s.dbl_rnd)
    }

    pub(crate) fn op_elementwise(&mut self, bus: &mut dyn NpuBus, mode: u32) -> Result<(), String> {
        const MUL: u32 = 0;
        const ADD: u32 = 1;
        const SUB: u32 = 2;
        const MIN: u32 = 3;
        const MAX: u32 = 4;
        const LRELU: u32 = 5;
        const ABS: u32 = 6;
        const CLZ: u32 = 7;
        const SHR: u32 = 8;
        const SHL: u32 = 9;
        const LSR: u32 = 10;
        const DIV: u32 = 11;
        const CMP_EQ: u32 = 16;
        const CMP_NE: u32 = 17;
        const CMP_GE: u32 = 18;
        const CMP_GT: u32 = 19;
        const AND: u32 = 33;
        const OR: u32 = 34;
        const XOR: u32 = 35;
        const NOT: u32 = 36;
        const AND_NOT: u32 = 42;
        let (oh, ow, od) = self.st.ofm_dims();
        let unary = matches!(mode, LRELU | ABS | CLZ | NOT);
        let bm_a = self.st.p0(op0::IFM_BROADCAST) & 0xF;
        let bm_b = self.st.p0(op0::IFM2_BROADCAST) & 0xF;
        let use_ifm2 = !(unary || bm_b == 8);
        self.add_operation_to_chain(true, use_ifm2, true);
        let p = self.st.precision(op0::IFM_PRECISION);
        let p2 = self.st.precision(op0::IFM2_PRECISION);
        if use_ifm2 && p.bits != p2.bits {
            self.warn("elementwise inputs of different precision");
        }
        let scalar = self.st.p1(op1::OP_SCALAR) as u32 as i32 as i64;
        // inputs with their broadcast dimensions collapsed
        let dims = |bm: u32| (if bm & 1 != 0 { 1 } else { oh }, if bm & 2 != 0 { 1 } else { ow }, if bm & 4 != 0 { 1 } else { od });
        let mut a: Vec<i32> = Vec::new();
        let (ah, aw, ac) = dims(bm_a);
        if bm_a != 8 {
            if bm_a != 0 && p.storage == Storage::Chained {
                self.warn("broadcast from a chaining buffer");
            }
            self.read_input::<i32>(bus, 0, ah, aw, ac, &mut a);
        }
        let mut b: Vec<i32> = Vec::new();
        let (bh, bw, bc) = dims(bm_b);
        if use_ifm2 {
            if bm_b != 0 && p2.storage == Storage::Chained {
                self.warn("broadcast from a chaining buffer");
            }
            self.read_input::<i32>(bus, 2, bh, bw, bc, &mut b);
        }
        let cx = self.accum_ctx(bus, false)?;
        let apply_scale = p.bits == 8 || (p.bits == 16 && p.signed);
        let sa = self.st.scale_reg(op1::IFM_SCALE);
        let sb = self.st.scale_reg(op1::IFM2_SCALE);
        let ofm_scale = cx.ofm_scale;
        let mut out = vec![0i64; oh * ow * od];
        self.cycles += (oh * ow * od) as u64 / (self.cfg.macs as u64 / 16).max(1) + 64;
        let mut div_by_zero = false;
        for oy in 0..oh {
            for ox in 0..ow {
                for oc in 0..od {
                    let av: i64 = if bm_a == 8 {
                        scalar
                    } else {
                        let (iy, ix, ic) =
                            (if bm_a & 1 != 0 { 0 } else { oy }, if bm_a & 2 != 0 { 0 } else { ox }, if bm_a & 4 != 0 { 0 } else { oc });
                        a[(iy * aw + ix) * ac + ic] as i64
                    };
                    let bv: i64 = if !use_ifm2 {
                        if bm_b == 8 {
                            scalar
                        } else {
                            0
                        }
                    } else {
                        let (iy, ix, ic) =
                            (if bm_b & 1 != 0 { 0 } else { oy }, if bm_b & 2 != 0 { 0 } else { ox }, if bm_b & 4 != 0 { 0 } else { oc });
                        b[(iy * bw + ix) * bc + ic] as i64
                    };
                    let sa_v = |n: &Self, v: i64| n.scale_input(v, &sa, apply_scale);
                    let sb_v = |n: &Self, v: i64| n.scale_input(v, &sb, apply_scale);
                    let mut rmode = Rescale::Global;
                    let acc: i64 = match mode {
                        MUL => {
                            if !(p.bits == 8 || p.bits == 16) {
                                rmode = Rescale::ShiftOnly;
                            }
                            av * bv
                        }
                        ADD => sa_v(self, av) + sb_v(self, bv),
                        SUB => sa_v(self, av) - sb_v(self, bv),
                        MIN => sa_v(self, av).min(sb_v(self, bv)),
                        MAX => sa_v(self, av).max(sb_v(self, bv)),
                        LRELU => {
                            if av >= 0 {
                                sa_v(self, av)
                            } else {
                                sb_v(self, av)
                            }
                        }
                        ABS => sa_v(self, av).abs(),
                        CLZ => clz32(sa_v(self, av)),
                        SHR => {
                            rmode = Rescale::None;
                            let shift = (sb_v(self, bv) & 31) as u32;
                            rounded_shift_right(sa_v(self, av) as i128, shift, ofm_scale.round_mode, ofm_scale.dbl_rnd)
                        }
                        SHL => {
                            rmode = Rescale::None;
                            let shift = (sb_v(self, bv) & 31) as u32;
                            ((sa_v(self, av) as i32).wrapping_shl(shift)) as i64
                        }
                        LSR => {
                            rmode = Rescale::None;
                            let shift = (sb_v(self, bv) & 31) as u32;
                            let mut acc = sa_v(self, av);
                            match p.bits {
                                8 => acc &= 0xFF,
                                16 => acc &= 0xFFFF,
                                _ => {}
                            }
                            ((acc as u32) >> shift) as i64
                        }
                        DIV => {
                            if bv == 0 {
                                div_by_zero = true;
                                0
                            } else {
                                (av as i32).wrapping_div(bv as i32) as i64
                            }
                        }
                        CMP_EQ => -((sa_v(self, av) == sb_v(self, bv)) as i64),
                        CMP_NE => -((sa_v(self, av) != sb_v(self, bv)) as i64),
                        CMP_GE => -((sa_v(self, av) >= sb_v(self, bv)) as i64),
                        CMP_GT => -((sa_v(self, av) > sb_v(self, bv)) as i64),
                        NOT => !sa_v(self, av),
                        AND => sa_v(self, av) & sb_v(self, bv),
                        OR => sa_v(self, av) | sb_v(self, bv),
                        XOR => sa_v(self, av) ^ sb_v(self, bv),
                        AND_NOT => sa_v(self, av) & !sb_v(self, bv),
                        _ => return Err(format!("reserved elementwise mode {mode}")),
                    };
                    let v = self.rescale(&cx, acc, usize::MAX, rmode);
                    let v = self.activation(v);
                    out[(oy * ow + ox) * od + oc] = v;
                }
            }
        }
        if div_by_zero {
            self.warn("elementwise division by zero");
        }
        self.write_ofm(bus, &mut out, false);
        Ok(())
    }

    pub(crate) fn op_resize(&mut self, bus: &mut dyn NpuBus, mode: u32) -> Result<(), String> {
        let (oh, ow, od) = self.st.ofm_dims();
        let ifm_h = self.st.p0(op0::KERNEL_HEIGHT_M1) as usize + 1;
        let ifm_w = self.st.p0(op0::KERNEL_WIDTH_M1) as usize + 1;
        let scale_y_n = (self.st.p0(op0::RESIZE_Y_SCALE_N_M1) & 0x7FF) as i64 + 1;
        let scale_x_n = (self.st.p0(op0::RESIZE_X_SCALE_N_M1) & 0x7FF) as i64 + 1;
        let y_off = sign_extend((self.st.p0(op0::RESIZE_Y_OFFSET) & 0xFFF) as i64, 12);
        let x_off = sign_extend((self.st.p0(op0::RESIZE_X_OFFSET) & 0xFFF) as i64, 12);
        let ystep = self.st.p1(op1::RESIZE_Y_STEP);
        let xstep = self.st.p1(op1::RESIZE_X_STEP);
        let (step_y_int, step_y_mod) = ((ystep >> 32) as i64 & 0xF, (ystep & 0x7FF) as i64);
        let (step_x_int, step_x_mod) = ((xstep >> 32) as i64 & 0xF, (xstep & 0x7FF) as i64);
        if self.chain_len != 0 {
            self.warn("resize inside a chain");
            self.chain_len = 0;
        }
        self.add_operation_to_chain(true, false, false);
        let mut ifm: Vec<i32> = Vec::new();
        self.read_input::<i32>(bus, 0, ifm_h, ifm_w, od, &mut ifm);
        let cx = self.accum_ctx(bus, false)?;
        if cx.ofm_scale.shift < 16 {
            self.warn("resize with an OFM shift below 16");
        }
        let mut out = vec![0i64; oh * ow * od];
        self.cycles += (oh * ow * od) as u64 / (self.cfg.macs as u64 / 32).max(1) + 64;
        let at = |y: usize, x: usize, c: usize| ifm[(y * ifm_w + x) * od + c] as i64;
        let mut y_int: i64 = if y_off < 0 { -1 } else { 0 };
        let mut y_mod: i64 = if y_off < 0 { y_off + scale_y_n } else { y_off };
        for oy in 0..oh {
            let iy0 = y_int.max(0) as usize;
            let iy1 = ((y_int + 1).max(0) as usize).min(ifm_h - 1);
            let iy0 = iy0.min(ifm_h - 1);
            let mut x_int: i64 = if x_off < 0 { -1 } else { 0 };
            let mut x_mod: i64 = if x_off < 0 { x_off + scale_x_n } else { x_off };
            for ox in 0..ow {
                let ix0 = (x_int.max(0) as usize).min(ifm_w - 1);
                let ix1 = ((x_int + 1).max(0) as usize).min(ifm_w - 1);
                for oc in 0..od {
                    let v0 = at(iy0, ix0, oc);
                    let acc: i64 = match mode {
                        0 => {
                            let v1 = at(iy0, ix1, oc);
                            let v2 = at(iy1, ix0, oc);
                            let v3 = at(iy1, ix1, oc);
                            (scale_y_n - y_mod) * ((scale_x_n - x_mod) * v0 + x_mod * v1) + y_mod * ((scale_x_n - x_mod) * v2 + x_mod * v3)
                        }
                        2 => {
                            let ix = if 2 * x_mod < scale_x_n { ix0 } else { ix1 };
                            let iy = if 2 * y_mod < scale_y_n { iy0 } else { iy1 };
                            at(iy, ix, oc)
                        }
                        _ => v0,
                    };
                    let acc2 = acc << 16;
                    let v = self.rescale(&cx, acc2, usize::MAX, Rescale::ShiftOnly);
                    let v = self.activation(v);
                    out[(oy * ow + ox) * od + oc] = v;
                }
                x_mod += step_x_mod;
                x_int += step_x_int;
                if x_mod >= scale_x_n {
                    x_mod -= scale_x_n;
                    x_int += 1;
                }
            }
            y_mod += step_y_mod;
            y_int += step_y_int;
            if y_mod >= scale_y_n {
                y_mod -= scale_y_n;
                y_int += 1;
            }
        }
        self.write_ofm(bus, &mut out, false);
        Ok(())
    }

    pub(crate) fn op_dma(&mut self, bus: &mut dyn NpuBus) -> Result<(), String> {
        let src_reg = self.st.p0(op0::DMA0_SRC_REGION);
        let dst_reg = self.st.p0(op0::DMA0_DST_REGION);
        let src_region = (src_reg & 7) as usize;
        let src_internal = (src_reg >> 8) & 1 != 0;
        let stride_mode = (src_reg >> 9) & 3;
        let src_idx = (src_reg >> 11) & 1 != 0;
        let dst_region = (dst_reg & 7) as usize;
        let dst_internal = (dst_reg >> 8) & 1 != 0;
        let dst_idx = (dst_reg >> 11) & 1 != 0;
        let length = self.st.addr(op1::DMA0_LEN) as usize;
        let mut width = self.st.p0(op0::DMA0_SIZE0) as usize;
        let mut height = self.st.p0(op0::DMA0_SIZE1) as usize;
        let mut src_p = self.st.addr(op1::DMA0_SRC);
        let mut dst_p = self.st.addr(op1::DMA0_DST);
        let mut idx_p = self.st.addr(op1::DMA0_IDX);
        let idx_region = (self.st.p0(op0::DMA0_IDX_REGION) & 7) as usize;
        let idx_max = (self.st.p1(op1::DMA0_IDX_MAX) & 0x7FFF_FFFF) as i32;
        let src_stride_0 = self.st.addr(op1::DMA0_SRC_STRIDE0);
        let src_stride_1 = self.st.addr(op1::DMA0_SRC_STRIDE1);
        let dst_stride_0 = self.st.addr(op1::DMA0_DST_STRIDE0);
        let dst_stride_1 = self.st.addr(op1::DMA0_DST_STRIDE1);
        let idx_skip_1 = self.st.addr(op1::DMA0_IDX_SKIP1);
        if src_internal {
            self.warn("DMA source in internal memory");
        }
        if src_idx && dst_idx {
            self.warn("DMA gather and scatter at the same time");
        }
        if stride_mode == 0 {
            width = 1;
        }
        if stride_mode != 2 {
            height = 1;
        }
        let mut buf = vec![0u8; length];
        let mut ok = true;
        for _y in 0..height {
            for x in 0..width {
                let mut src_i = x as i64;
                let mut dst_i = x as i64;
                let mut valid = true;
                if src_idx || dst_idx {
                    let mut b = [0u8; 4];
                    ok &= bus.read(self.region_addr(idx_region, idx_p), &mut b);
                    idx_p += 4;
                    let idx = i32::from_le_bytes(b);
                    if idx < 0 || idx > idx_max {
                        valid = false;
                    }
                    if src_idx {
                        src_i = idx as i64;
                    } else {
                        dst_i = idx as i64;
                    }
                }
                if valid {
                    let src_addr = src_p.wrapping_add((src_i as u64).wrapping_mul(src_stride_0));
                    let dst_addr = dst_p.wrapping_add((dst_i as u64).wrapping_mul(dst_stride_0));
                    ok &= bus.read(self.region_addr(src_region, src_addr), &mut buf);
                    if dst_internal {
                        let d = dst_addr as usize;
                        if d + length <= crate::LUT_SIZE {
                            self.lut[d..d + length].copy_from_slice(&buf);
                        } else {
                            self.warn("DMA into internal memory beyond the lookup table");
                        }
                    } else {
                        ok &= bus.write(self.region_addr(dst_region, dst_addr), &buf);
                    }
                }
            }
            src_p = src_p.wrapping_add(src_stride_1);
            dst_p = dst_p.wrapping_add(dst_stride_1);
            if src_idx || dst_idx {
                idx_p = idx_p.wrapping_add(idx_skip_1);
            }
        }
        if !ok {
            self.warn("DMA outside memory");
        }
        self.cycles += ((length * width * height) as u64).div_ceil(32) + 32;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_modes() {
        // natural: round half up
        assert_eq!(rounded_shift_right(5, 1, 1, 0), 3);
        assert_eq!(rounded_shift_right(-5, 1, 1, 0), -2);
        // symmetric: round half away from zero
        assert_eq!(rounded_shift_right(-5, 1, 3, 0), -3);
        // truncate to zero / to lower
        assert_eq!(rounded_shift_right(-5, 1, 4, 0), -2);
        assert_eq!(rounded_shift_right(-5, 1, 5, 0), -3);
        // double rounding: a shift above 31 - dbl_rnd adds 1<<(30-dbl_rnd) to the rounding term
        assert_eq!(rounded_shift_right(1i128 << 31, 31, 0, 0), 1);
        assert_eq!(rounded_shift_right(1i128 << 30, 32, 0, 0), 1);
        assert_eq!(rounded_shift_right(1i128 << 30, 32, 1, 0), 0);
        assert_eq!(rounded_shift_right(-(1i128 << 30), 32, 0, 0), 0);
        assert_eq!(rounded_shift_right(-(3i128 << 30), 32, 0, 0), -1);
        assert_eq!(rounded_shift_right(100, 0, 0, 0), 100);
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate(0x1FF, 8, false), 0xFF);
        assert_eq!(truncate(0x1FF, 8, true), -1);
        assert_eq!(truncate(-200, 8, true), 56);
        assert_eq!(truncate(1 << 40, 64, true), 1 << 40);
    }

    #[test]
    fn clz() {
        assert_eq!(clz32(0), 32);
        assert_eq!(clz32(1), 31);
        assert_eq!(clz32(-1), 0);
    }
}
