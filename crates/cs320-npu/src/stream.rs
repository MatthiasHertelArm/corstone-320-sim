//! Bit streams of the Ethos-U85: the weight stream (standard weight decoder with Golomb-Rice,
//! palette and zero-run coding; fast weight decoder with packed indices), the scale and bias
//! stream, and the order in which the MAC array consumes the weights of a kernel
//! (TRM §3.7 and §3.9; the ordering follows Vela's `EthosU85WeightOrdering`, which is the
//! encoder the streams come from).

/// Reads bit-packed symbols, least significant bit first (TRM §3.9.3). Reads past the end of
/// the data return zero bits.
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    #[inline]
    pub fn bit(&mut self) -> u32 {
        let b = self.data.get(self.pos >> 3).map_or(0, |b| (b >> (self.pos & 7)) & 1);
        self.pos += 1;
        b as u32
    }

    #[inline]
    pub fn uint(&mut self, n: u32) -> u64 {
        let mut v = 0u64;
        for i in 0..n {
            v |= (self.bit() as u64) << i;
        }
        v
    }

    pub fn sint(&mut self, n: u32) -> i64 {
        let v = self.uint(n);
        sign_extend(v as i64, n)
    }

    pub fn align(&mut self, bits: usize) {
        self.pos = self.pos.div_ceil(bits) * bits;
    }

    pub fn at_end(&self) -> bool {
        self.pos >= self.data.len() * 8
    }
}

#[inline]
pub fn sign_extend(v: i64, n: u32) -> i64 {
    if n >= 64 {
        return v;
    }
    let sh = 64 - n;
    (v << sh) >> sh
}

#[inline]
pub fn align_up(v: usize, a: usize) -> usize {
    v.div_ceil(a) * a
}

const ZDIV_DISABLE: u64 = 6;
const ZDIV_EOS: u64 = 7;
const WDIV_UNCOMPRESSED: u64 = 7;

/// Decodes a standard weight stream (TRM §3.9.4 – §3.9.10) into weight values. Decoding stops at
/// the end-of-stream marker once `needed` values are available, or when the data runs out.
pub fn decode_swd(data: &[u8], needed: usize) -> Result<Vec<i16>, String> {
    let mut out: Vec<i16> = Vec::with_capacity(needed.min(data.len() * 2));
    let mut rd = BitReader::new(data);
    let mut palette = [0i32; 32];
    let mut palette_size = 0usize;
    let mut palette_bits = 0u32;
    let mut dirofs = 0i32;
    let mut first = true;
    let mut w_value: Vec<u32> = Vec::new();
    let mut z_value: Vec<u32> = Vec::new();
    loop {
        if out.len() >= needed || rd.at_end() {
            break;
        }
        let zdiv = rd.uint(3);
        if zdiv == ZDIV_EOS {
            // padding with '1' up to the next 128-bit boundary; a further stream may follow
            rd.align(128);
            first = true;
            continue;
        }
        if zdiv > 3 && zdiv != ZDIV_DISABLE {
            return Err(format!("reserved zdiv {zdiv} in the weight stream"));
        }
        let alternating = zdiv != ZDIV_DISABLE;
        let zdiv = zdiv as u32;
        let slicelen = rd.uint(15) as usize + 1;
        let wdiv = rd.uint(3);
        let wtrunc = rd.uint(1) != 0;
        let newpal = rd.uint(1) != 0;
        if first && !newpal {
            return Err("first weight slice without a palette".into());
        }
        first = false;
        if newpal {
            dirofs = rd.uint(5) as i32;
            let palsize = rd.uint(5) as usize;
            palette_size = if palsize == 0 { 0 } else { palsize + 1 };
            palette_bits = rd.uint(3) as u32 + 2;
            for p in palette.iter_mut().take(palette_size) {
                *p = rd.uint(palette_bits) as i32;
            }
        }
        let uncompressed = wdiv == WDIV_UNCOMPRESSED;
        let wremain_bits = if uncompressed {
            if palette_size > 0 {
                let mut b = 0;
                while (1usize << b) < palette_size {
                    b += 1;
                }
                b
            } else {
                palette_bits
            }
        } else {
            if wdiv > 5 {
                return Err(format!("reserved wdiv {wdiv} in the weight stream"));
            }
            wdiv as u32
        };
        let w_cnt = slicelen;
        let z_cnt = slicelen + newpal as usize;
        w_value.clear();
        w_value.resize(w_cnt, 0);
        z_value.clear();
        z_value.resize(z_cnt, 0);
        let zunary_len = if zdiv < 3 { 12 } else { 8 };
        let w_per_chunk = if uncompressed && wremain_bits > 5 { 8 } else { 12 };
        let (mut wq_i, mut wr_i, mut zq_i, mut zr_i) = (0usize, 0usize, 0usize, 0usize);
        let (mut wq, mut zq) = (0u32, 0u32);
        let mut w_q = [0u32; 12];
        let mut z_q = [0u32; 12];
        let mut w_prev_q = [0u32; 12];
        let mut z_prev_q = [0u32; 12];
        let (mut w_prev_n, mut z_prev_n) = (0usize, 0usize);
        let (mut prev_w_enable, mut prev_z_enable) = (false, false);
        loop {
            let balance = if alternating { wq_i as i64 - zq_i as i64 } else { 0 };
            let w_enable = (balance < 8 || !alternating) && wq_i < w_cnt;
            let z_enable = balance >= 0 && alternating && zq_i < z_cnt;
            let mut wunary0 = 0u64;
            if w_enable && !uncompressed {
                wunary0 = rd.uint(12);
            }
            let mut z_n = 0usize;
            if z_enable {
                let zunary = rd.uint(zunary_len);
                for i in 0..zunary_len {
                    if (zunary >> i) & 1 != 0 {
                        zq += 1;
                    } else {
                        z_q[z_n] = zq;
                        z_n += 1;
                        zq = 0;
                    }
                }
                zq_i += z_n;
            }
            let mut w_n = 0usize;
            if w_enable {
                let mut wunary1_len = 0;
                for i in 0..w_per_chunk {
                    if (wunary0 >> i) & 1 != 0 {
                        wunary1_len += 1;
                    }
                }
                let mut wunary1 = rd.uint(wunary1_len);
                for i in 0..w_per_chunk {
                    let mut code = 0;
                    if (wunary0 >> i) & 1 != 0 {
                        code = 1 + (wunary1 & 1) as u32;
                        wunary1 >>= 1;
                    }
                    wq += code;
                    if code < 2 || wtrunc {
                        w_q[w_n] = wq;
                        w_n += 1;
                        wq = 0;
                    }
                }
                wq_i += w_n;
            }
            // remainders of the quotients of the previous chunk
            if prev_w_enable {
                for q in w_prev_q.iter().take(w_prev_n) {
                    if wr_i >= w_cnt {
                        break;
                    }
                    let remain = rd.uint(wremain_bits) as u32;
                    w_value[wr_i] = (q << wremain_bits) + remain;
                    wr_i += 1;
                }
            }
            if prev_z_enable {
                for q in z_prev_q.iter().take(z_prev_n) {
                    if zr_i >= z_cnt {
                        break;
                    }
                    let remain = rd.uint(zdiv) as u32;
                    z_value[zr_i] = (q << zdiv) + remain;
                    zr_i += 1;
                }
            }
            prev_w_enable = w_enable;
            w_prev_n = w_n;
            w_prev_q = w_q;
            prev_z_enable = z_enable;
            z_prev_n = z_n;
            z_prev_q = z_q;
            if !(prev_w_enable || prev_z_enable) {
                break;
            }
            if rd.pos > data.len() * 8 + 64 {
                return Err("weight stream truncated".into());
            }
        }
        // expand the slice
        if newpal && alternating {
            out.resize(out.len() + z_value[0] as usize, 0);
        }
        for i in 0..w_cnt {
            let idx = w_value[i] as usize;
            let tmp = if idx < palette_size { palette[idx] } else { idx as i32 - palette_size as i32 + dirofs };
            let mag = tmp >> 1;
            out.push(if tmp & 1 != 0 { -mag } else { mag } as i16);
            if alternating {
                out.resize(out.len() + z_value[i + newpal as usize] as usize, 0);
            }
        }
    }
    Ok(out)
}

/// Decodes a fast-weight-decoder stream (TRM §3.9.11): a 256-bit header with a 16-entry
/// lookup table, then 256-bit words of 32 raw weights, 64 4-bit indices or 128 2-bit indices.
pub fn decode_fwd(data: &[u8]) -> Result<Vec<i16>, String> {
    if data.len() < 32 {
        return Err("fast weight stream shorter than its header".into());
    }
    let mut rd = BitReader::new(data);
    let raw_mode = rd.uint(1) != 0;
    let small_lut = rd.uint(1) != 0;
    rd.uint(64);
    rd.uint(38);
    let zero_point = rd.sint(8);
    let mut lut = [0i16; 16];
    for v in lut.iter_mut() {
        let sign = rd.uint(1) != 0;
        let abs = rd.uint(8) as i16;
        *v = if sign { -abs } else { abs };
    }
    let mut out = Vec::with_capacity((data.len() - 32) * 4);
    while !rd.at_end() {
        if raw_mode {
            for _ in 0..32 {
                let v = rd.sint(8) - zero_point;
                out.push(v as i16);
            }
        } else if !small_lut {
            for _ in 0..64 {
                out.push(lut[rd.uint(4) as usize]);
            }
        } else {
            for _ in 0..128 {
                out.push(lut[rd.uint(2) as usize]);
            }
        }
    }
    Ok(out)
}

/// Scale, shift and bias of the output channels (TRM §3.7).
#[derive(Default, Clone, Debug)]
pub struct ScaleStream {
    pub bias: Vec<i64>,
    pub scale: Vec<i32>,
    pub shift: Vec<u32>,
}

impl ScaleStream {
    /// Parses the stream for `ofm_depth` channels in sets of `channels_in_set`; with `reverse_c`
    /// the sets are stored in reverse channel order.
    pub fn parse(data: &[u8], ofm_depth: usize, channels_in_set: usize, acc48: bool, reverse_c: bool) -> ScaleStream {
        let n = align_up(ofm_depth.max(1), channels_in_set);
        let mut s = ScaleStream { bias: vec![0; n], scale: vec![0; n], shift: vec![0; n] };
        let mut rd = BitReader::new(data);
        let mut parse_set = |s: &mut ScaleStream, oc: usize| {
            for i in 0..channels_in_set {
                let (bias, scale) = if acc48 {
                    let b = rd.uint(48);
                    (sign_extend(b as i64, 48), rd.uint(15) as i32)
                } else {
                    let b = rd.uint(32);
                    (b as u32 as i32 as i64, rd.uint(31) as i32)
                };
                rd.uint(1);
                let shift = rd.uint(6) as u32;
                rd.uint(10);
                if oc + i < n {
                    s.bias[oc + i] = bias;
                    s.scale[oc + i] = scale;
                    s.shift[oc + i] = shift;
                }
            }
        };
        if reverse_c {
            let mut oc = n;
            while oc > 0 {
                oc -= channels_in_set;
                parse_set(&mut s, oc);
            }
        } else {
            let mut oc = 0;
            while oc < ofm_depth {
                parse_set(&mut s, oc);
                oc += channels_in_set;
            }
        }
        s
    }
}

/// How the weights of one kernel are laid out in the weight streams.
#[derive(Clone, Copy, Debug)]
pub struct WeightOrder {
    pub streams: usize,
    pub depthwise: bool,
    pub part_kernel: bool,
    pub sparse: bool,
    pub ifm16: bool,
    pub ofm_depth: usize,
    pub ifm_depth: usize,
    pub ofm_block_depth: usize,
    pub ifm_block_depth: usize,
    pub ofm_ublock_depth: usize,
    /// kernel size in undilated weights
    pub kernel_w: usize,
    pub kernel_h: usize,
    pub stride_x: usize,
    pub stride_y: usize,
    pub dilation_x: usize,
    pub dilation_y: usize,
    pub dw_pad: usize,
}

impl WeightOrder {
    pub fn sub_kernel_round(&self) -> usize {
        if self.depthwise {
            10
        } else if self.part_kernel {
            if self.ifm16 || self.sparse {
                10
            } else {
                5
            }
        } else {
            1
        }
    }

    pub fn ifm_ublock_depth(&self) -> usize {
        if self.depthwise {
            1
        } else if self.part_kernel && self.ifm16 && !self.sparse {
            8
        } else {
            self.ifm_block_depth
        }
    }

    /// Weights per stream in one fast-weight-decoder interleave block.
    pub fn fwd_block(&self) -> usize {
        self.ofm_ublock_depth * self.ifm_ublock_depth() * self.sub_kernel_round() / self.streams
    }

    /// Visits every weight position of the streams in consumption order:
    /// `f(stream, ofm_channel, ky, kx, ifm_channel)`; padding positions have `ofm_channel ==
    /// usize::MAX`. For depthwise kernels the ofm channel is the channel and `ifm_channel == 0`.
    pub fn visit(&self, mut f: impl FnMut(usize, usize, usize, usize, usize)) {
        const INTERLEAVE: usize = 4;
        let streams = self.streams;
        let (stride_x, stride_y) = if self.part_kernel || self.depthwise { (self.stride_x, self.stride_y) } else { (1, 1) };
        let decomp_x = 8 / self.dilation_x;
        let decomp_y = 8 / self.dilation_y;
        let sub_round = self.sub_kernel_round();
        let ifm_ublock_depth = self.ifm_ublock_depth();
        let ofm_ublock_depth = self.ofm_ublock_depth;
        let inner_step = if self.depthwise {
            1
        } else if self.sparse {
            16
        } else {
            8
        };
        let ifm_block_depth = self.ifm_block_depth.max(1);
        let ifm_depth_loop = if self.depthwise { 1 } else { self.ifm_depth };
        let mut ifm_loop_inc: isize = -(ifm_block_depth as isize);
        let mut ifm_block_z: isize = 0;
        let mut ofm_block_z = 0usize;
        while ofm_block_z < self.ofm_depth {
            ifm_loop_inc = -ifm_loop_inc;
            let clipped_ofm_block_depth = self.ofm_block_depth.min(self.ofm_depth - ofm_block_z);
            while ifm_block_z < ifm_depth_loop as isize && ifm_block_z >= 0 {
                let clipped_ifm_block_depth = ifm_block_depth.min(ifm_depth_loop - ifm_block_z as usize);
                let mut sub_kernel_x = 0;
                while sub_kernel_x < self.kernel_w {
                    let sub_width = (self.kernel_w - sub_kernel_x).min(decomp_x);
                    let mut sub_kernel_y = 0;
                    while sub_kernel_y < self.kernel_h {
                        let sub_height = (self.kernel_h - sub_kernel_y).min(decomp_y);
                        let ifm_block_depth_outer = if self.depthwise { 1 } else { clipped_ifm_block_depth };
                        let mut ifm_ublock_outer = 0;
                        while ifm_ublock_outer < ifm_block_depth_outer {
                            let mut ofm_ublock_outer = 0;
                            while ofm_ublock_outer < clipped_ofm_block_depth {
                                let mut sub_kernel_elements = align_up(sub_width * sub_height, sub_round) as isize;
                                for sy in 0..stride_y {
                                    let strided_kernel_h = (sub_height + stride_y - 1 - sy) / stride_y;
                                    for sx in 0..stride_x {
                                        let strided_kernel_w = (sub_width + stride_x - 1 - sx) / stride_x;
                                        for ky in 0..strided_kernel_h {
                                            for kx in 0..strided_kernel_w {
                                                let x = if ky % 2 == 0 { kx } else { strided_kernel_w - 1 - kx };
                                                sub_kernel_elements -= 1;
                                                let mut ifm_ublock_inner = 0;
                                                while ifm_ublock_inner < ifm_ublock_depth {
                                                    for stream in 0..streams {
                                                        let mut ofm_ublock_z = stream * INTERLEAVE;
                                                        while ofm_ublock_z < ofm_ublock_depth {
                                                            for ofm_ublock_inner in 0..INTERLEAVE {
                                                                for ifm_ublock_z in 0..inner_step {
                                                                    let wx = sub_kernel_x + sx + x * stride_x;
                                                                    let wy = sub_kernel_y + sy + ky * stride_y;
                                                                    let ifm_z = ifm_block_z as usize
                                                                        + ifm_ublock_outer
                                                                        + ifm_ublock_inner
                                                                        + ifm_ublock_z;
                                                                    let ofm_z =
                                                                        ofm_block_z + ofm_ublock_outer + ofm_ublock_inner + ofm_ublock_z;
                                                                    if ifm_z < self.ifm_depth && ofm_z < self.ofm_depth {
                                                                        f(stream, ofm_z, wy, wx, ifm_z);
                                                                    } else {
                                                                        f(stream, usize::MAX, 0, 0, 0);
                                                                    }
                                                                }
                                                            }
                                                            ofm_ublock_z += INTERLEAVE * streams;
                                                        }
                                                    }
                                                    ifm_ublock_inner += inner_step;
                                                }
                                                if self.depthwise && sub_kernel_elements % sub_round as isize == 0 {
                                                    let pad = self.dw_pad * ofm_ublock_depth / streams;
                                                    for stream in 0..streams {
                                                        for _ in 0..pad {
                                                            f(stream, usize::MAX, 0, 0, 0);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                if sub_kernel_elements > 0 {
                                    let mut pad = sub_kernel_elements as usize + if self.depthwise { self.dw_pad } else { 0 };
                                    pad = pad * ifm_ublock_depth * ofm_ublock_depth / streams;
                                    for stream in 0..streams {
                                        for _ in 0..pad {
                                            f(stream, usize::MAX, 0, 0, 0);
                                        }
                                    }
                                }
                                ofm_ublock_outer += ofm_ublock_depth;
                            }
                            ifm_ublock_outer += ifm_ublock_depth;
                        }
                        sub_kernel_y += decomp_y;
                    }
                    sub_kernel_x += decomp_x;
                }
                ifm_block_z += ifm_loop_inc;
            }
            // the next ofm block walks the ifm blocks in the opposite direction, starting at the last one
            ifm_block_z -= ifm_loop_inc;
            ofm_block_z += self.ofm_block_depth;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes bit-packed symbols (LSB first) for the tests.
    struct BitWriter {
        data: Vec<u8>,
        pos: usize,
    }

    impl BitWriter {
        fn new() -> Self {
            BitWriter { data: Vec::new(), pos: 0 }
        }
        fn put(&mut self, v: u64, n: u32) {
            for i in 0..n {
                if self.pos.is_multiple_of(8) {
                    self.data.push(0);
                }
                if (v >> i) & 1 != 0 {
                    *self.data.last_mut().unwrap() |= 1 << (self.pos % 8);
                }
                self.pos += 1;
            }
        }
    }

    #[test]
    fn bit_order_example() {
        // TRM §3.9.3: 0x12 (5 bits), 0xB (4 bits), 0x55 (7 bits) store 0x72, 0xAB
        let mut w = BitWriter::new();
        w.put(0x12, 5);
        w.put(0xB, 4);
        w.put(0x55, 7);
        assert_eq!(w.data, vec![0x72, 0xAB]);
        let mut r = BitReader::new(&w.data);
        assert_eq!(r.uint(5), 0x12);
        assert_eq!(r.uint(4), 0xB);
        assert_eq!(r.uint(7), 0x55);
    }

    /// An uncompressed, non-alternating slice in direct mode: every weight index is stored in
    /// `palette_bits` bits, 12 per chunk, quotients all zero.
    #[test]
    fn uncompressed_direct_slice() {
        let values: Vec<i16> = vec![3, -2, 0, 7, -7, 1, 0, 0, 5, -1, 2, 4, 6, -6];
        let mut w = BitWriter::new();
        w.put(ZDIV_DISABLE, 3);
        w.put(values.len() as u64 - 1, 15);
        w.put(WDIV_UNCOMPRESSED, 3);
        w.put(0, 1); // wtrunc
        w.put(1, 1); // newpal
        w.put(0, 5); // dirofs
        w.put(0, 5); // palsize 0: direct
        w.put(2, 3); // palbits = 4
                     // chunk 1: 12 symbols (no unary in uncompressed mode), chunk 2: 2 symbols,
                     // then remainders of chunk 1, then an empty chunk carrying the remainders of chunk 2
        let idx: Vec<u64> = values.iter().map(|&v| if v < 0 { ((-v as u64) << 1) | 1 } else { (v as u64) << 1 }).collect();
        for &i in &idx[..12] {
            w.put(i, 4);
        }
        for &i in &idx[12..] {
            w.put(i, 4);
        }
        w.put(ZDIV_EOS, 3);
        let out = decode_swd(&w.data, values.len()).unwrap();
        assert_eq!(&out[..values.len()], &values[..]);
    }

    /// Golomb-Rice coded palette slice with zero runs (alternating mode).
    #[test]
    fn grc_palette_alternating_slice() {
        // weights: 0, 5, 6, 0, 0, 0, 7, 0  (TRM §3.9.9 example): n = 3, zruns = {1, 0, 3, 1}
        let palette = [10u64, 12, 14]; // 5, 6, 7 as sign-magnitude (positive: value << 1)
        let mut w = BitWriter::new();
        w.put(0, 3); // zdiv = 0: alternating with divisor 1
        w.put(2, 15); // slicelen 3
        w.put(0, 3); // wdiv 0: quotient only
        w.put(0, 1);
        w.put(1, 1); // newpal
        w.put(0, 5);
        w.put(2, 5); // palsize field 2 → 3 entries
        w.put(2, 3); // palbits 4
        for &p in &palette {
            w.put(p, 4);
        }
        // chunk 1: wunary0 for indices 0,1,2 (quotients 0,1,2 with wdiv 0): unary 0 / 10 / 110
        // wunary0 bits: symbol i has bit set if quotient >= 1; wunary1 tells whether >= 2.
        // index 0 (q=0): bit0=0 -> symbol; index 1 (q=1): bit1=1, wunary1 bit=0 -> symbol
        // index 2 (q=2): bit2=1, wunary1 bit=1 (c=2, continue), bit3=0 (c=0) -> symbol wq=2
        w.put(0b0110, 12);
        // zunary (12 bits, zdiv 0): zruns 1,0,3,1 → 10 0 1110 10 then padding zeros = more zero runs
        // bits: 1,0, 0, 1,1,1,0, 1,0, 0,0,0  (only the first 4 symbols are used)
        let zbits = [1u64, 0, 0, 1, 1, 1, 0, 1, 0, 0, 0, 0];
        let mut z = 0u64;
        for (i, b) in zbits.iter().enumerate() {
            z |= b << i;
        }
        w.put(z, 12);
        w.put(0b10, 2); // wunary1: two set bits in wunary0 → 2 bits: index1 → 0, index2 → 1
                        // chunk 2: no new symbols (w: 3 of 3 done; z: 4 of 4 done); remainders: none (wdiv 0, zdiv 0)
        w.put(ZDIV_EOS, 3);
        let out = decode_swd(&w.data, 8).unwrap();
        assert_eq!(&out[..8], &[0, 5, 6, 0, 0, 0, 7, 0]);
    }

    #[test]
    fn fwd_small_lut() {
        let mut w = BitWriter::new();
        w.put(0, 1); // raw_mode
        w.put(1, 1); // small lut
        w.put(0, 64);
        w.put(0, 38);
        w.put(0, 8); // zero point
        let lut = [3i16, -4, 0, 9];
        for i in 0..16 {
            let v = lut[i % 4];
            w.put((v < 0) as u64, 1);
            w.put(v.unsigned_abs() as u64, 8);
        }
        assert_eq!(w.pos, 256);
        for i in 0..128 {
            w.put((i % 4) as u64, 2);
        }
        let out = decode_fwd(&w.data).unwrap();
        for i in 0..128 {
            assert_eq!(out[i], lut[i % 4]);
        }
    }

    #[test]
    fn scale_stream_i32() {
        let mut data = Vec::new();
        for i in 0..16u32 {
            let bias: i32 = -(i as i32) * 1000;
            let scale: u32 = 0x4000_0000 + i;
            let shift = 30 + (i % 3);
            data.extend_from_slice(&(bias as u32).to_le_bytes());
            data.extend_from_slice(&scale.to_le_bytes());
            data.push((shift & 0x3F) as u8);
            data.push(0);
        }
        // bit 31 of the scale word is reserved: scale is 31 bits
        let s = ScaleStream::parse(&data, 10, 16, false, false);
        for i in 0..10 {
            assert_eq!(s.bias[i], -(i as i64) * 1000);
            assert_eq!(s.scale[i], 0x4000_0000 + i as i32);
            assert_eq!(s.shift[i], 30 + (i as u32 % 3));
        }
    }

    #[test]
    fn weight_order_counts_every_weight_once() {
        let o = WeightOrder {
            streams: 4,
            depthwise: false,
            part_kernel: false,
            sparse: false,
            ifm16: false,
            ofm_depth: 20,
            ifm_depth: 70,
            ofm_block_depth: 16,
            ifm_block_depth: 64,
            ofm_ublock_depth: 16,
            kernel_w: 3,
            kernel_h: 3,
            stride_x: 1,
            stride_y: 1,
            dilation_x: 1,
            dilation_y: 1,
            dw_pad: 6,
        };
        let mut seen = vec![0u32; 20 * 9 * 70];
        let mut per_stream = [0usize; 4];
        o.visit(|s, oc, ky, kx, ic| {
            per_stream[s] += 1;
            if oc != usize::MAX {
                seen[((oc * 3 + ky) * 3 + kx) * 70 + ic] += 1;
            }
        });
        assert!(seen.iter().all(|&n| n == 1));
        assert!(per_stream.iter().all(|&n| n == per_stream[0]));
    }

    #[test]
    fn weight_order_depthwise_counts_every_weight_once() {
        let o = WeightOrder {
            streams: 4,
            depthwise: true,
            part_kernel: true,
            sparse: false,
            ifm16: false,
            ofm_depth: 24,
            ifm_depth: 1,
            ofm_block_depth: 16,
            ifm_block_depth: 16,
            ofm_ublock_depth: 16,
            kernel_w: 3,
            kernel_h: 3,
            stride_x: 2,
            stride_y: 2,
            dilation_x: 1,
            dilation_y: 1,
            dw_pad: 6,
        };
        let mut seen = vec![0u32; 24 * 9];
        o.visit(|_s, oc, ky, kx, _ic| {
            if oc != usize::MAX {
                seen[(oc * 3 + ky) * 3 + kx] += 1;
            }
        });
        assert!(seen.iter().all(|&n| n == 1), "{seen:?}");
    }
}
