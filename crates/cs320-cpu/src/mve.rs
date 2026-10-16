//! M-profile Vector Extension (Helium).
//!
//! The vector registers Q0–Q7 overlay S0–S31 (`Cpu::s`, Q*n* = S[4n..4n+4]). A beat-wise
//! instruction computes all 16 byte lanes of its result and merges them into the destination
//! under the byte predicate of [`Cpu::mve_pred`]: VPR.P0 inside a VPT block, the tail
//! predicate of a DLSTP/WLSTP loop, both ANDed. Predicated-off lanes are neither loaded nor
//! stored nor written.
//!
//! Phase 1 of the plan: the instructions CMSIS-DSP, CMSIS-NN and the compilers' vectorised
//! loops use. An encoding that is not implemented (VLD2/VLD4, the complex multiplies and
//! VQDMLADH, 64-bit lanes) halts the simulation with the encoding instead of executing wrongly.
//! The encodings were taken from GNU as 13.3 and cross-checked with armclang 6.24 (which
//! disagree on VQSHRN/VQSHRUN and VCVTB/T between halves: armclang is right).

use crate::bus::Bus;
use crate::cpu::*;
use crate::decode::{vfp_expand_imm, Prec};
use crate::fpu::{self, Rounding, FPSCR_C, FPSCR_QC};

type V = [u8; 16];

#[inline]
fn get(v: &V, i: usize, sz: usize) -> u64 {
    let mut x = 0u64;
    for k in 0..sz {
        x |= (v[i * sz + k] as u64) << (8 * k);
    }
    x
}

#[inline]
fn gets(v: &V, i: usize, sz: usize) -> i64 {
    sext(get(v, i, sz), 8 * sz as u32)
}

#[inline]
fn put(v: &mut V, i: usize, sz: usize, x: u64) {
    for k in 0..sz {
        v[i * sz + k] = (x >> (8 * k)) as u8;
    }
}

#[inline]
fn sext(x: u64, bits: u32) -> i64 {
    ((x << (64 - bits)) as i64) >> (64 - bits)
}

#[inline]
fn bmask(bits: u32) -> u64 {
    if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    }
}

/// Signed saturation to `bits`; returns the bit pattern and whether it saturated.
#[inline]
fn sat_s(x: i128, bits: u32) -> (u64, bool) {
    let max = (1i128 << (bits - 1)) - 1;
    let min = -(1i128 << (bits - 1));
    if x > max {
        (max as u64 & bmask(bits), true)
    } else if x < min {
        (min as u64 & bmask(bits), true)
    } else {
        (x as u64 & bmask(bits), false)
    }
}

#[inline]
fn sat_u(x: i128, bits: u32) -> (u64, bool) {
    let max = (1i128 << bits) - 1;
    if x > max {
        (max as u64, true)
    } else if x < 0 {
        (0, true)
    } else {
        (x as u64, false)
    }
}

/// Whether element `i` (size `sz`) is predicated on: the predicate bit of its lowest byte
/// decides (measured on the FVP with a byte compare followed by a halfword reduction).
#[inline]
fn active(pred: u16, i: usize, sz: usize) -> bool {
    (pred >> (i * sz)) & 1 != 0
}

/// Byte predicate of a VCTP: the first `n` elements of `size` bytes.
pub(crate) fn vctp_mask(size: u32, n: u32) -> u16 {
    let bytes = (n as u64 * size as u64).min(16);
    if bytes >= 16 {
        0xFFFF
    } else {
        ((1u32 << bytes) - 1) as u16
    }
}

/// Element-wise map of two vectors over the active elements (predicated-off lanes are
/// neither computed, so that they raise no floating-point flags, nor written).
#[inline]
fn map2(a: &V, b: &V, sz: usize, pred: u16, mut f: impl FnMut(u64, u64) -> u64) -> V {
    let mut d = [0u8; 16];
    for e in 0..16 / sz {
        if active(pred, e, sz) {
            put(&mut d, e, sz, f(get(a, e, sz), get(b, e, sz)));
        }
    }
    d
}

#[inline]
fn map1(a: &V, sz: usize, pred: u16, mut f: impl FnMut(u64) -> u64) -> V {
    let mut d = [0u8; 16];
    for e in 0..16 / sz {
        if active(pred, e, sz) {
            put(&mut d, e, sz, f(get(a, e, sz)));
        }
    }
    d
}

/// Bit pattern of the float precision used by an MVE float lane of `sz` bytes.
#[inline]
fn fprec(sz: usize) -> Prec {
    if sz == 2 {
        Prec::H
    } else {
        Prec::S
    }
}

/// Shift of a lane value by a signed amount (positive = left); `round` adds the rounding
/// constant before a right shift, `arith` keeps the sign on right shifts.
#[inline]
fn shift_lane(x: i128, sh: i32, bits: u32, round: bool, arith: bool) -> i128 {
    if sh >= 0 {
        // the lane values fit in 64 bits: a shift of 62 or more already saturates every
        // lane size (and masks to zero for the non-saturating forms)
        x << sh.min(62)
    } else {
        let n = (-sh) as u32;
        let x = if arith { x } else { x & bmask(bits) as i128 };
        if n >= 127 {
            if !round && arith && x < 0 {
                -1
            } else {
                0
            }
        } else {
            let r = if round { x + (1i128 << (n - 1)) } else { x };
            r >> n
        }
    }
}

/// Polynomial (carry-less) multiply.
#[inline]
fn pmul(a: u64, b: u64, bits: u32) -> u64 {
    let mut r = 0u64;
    for i in 0..bits {
        if (b >> i) & 1 != 0 {
            r ^= a << i;
        }
    }
    r
}

/// `AdvSIMDExpandImm`: the 64-bit replicated immediate of the modified-immediate encodings
/// and the kind of operation (`0` VMOV, `1` VMVN, `2` VORR, `3` VBIC); `None` if undefined.
fn expand_imm(cmode: u32, op: u32, imm8: u32) -> Option<(u64, u32)> {
    let i = imm8 as u64;
    let rep32 = |x: u64| x | (x << 32);
    let rep16 = |x: u64| rep32(x | (x << 16));
    let rep8 = |x: u64| rep16(x | (x << 8));
    let (v, logic) = match cmode >> 1 {
        0 => (rep32(i), cmode & 1 == 1),
        1 => (rep32(i << 8), cmode & 1 == 1),
        2 => (rep32(i << 16), cmode & 1 == 1),
        3 => (rep32(i << 24), cmode & 1 == 1),
        4 => (rep16(i), cmode & 1 == 1),
        5 => (rep16(i << 8), cmode & 1 == 1),
        6 => (rep32(if cmode & 1 == 0 { (i << 8) | 0xFF } else { (i << 16) | 0xFFFF }), false),
        _ => {
            if cmode & 1 == 0 {
                if op == 0 {
                    (rep8(i), false)
                } else {
                    let mut v = 0u64;
                    for k in 0..8 {
                        if (imm8 >> k) & 1 != 0 {
                            v |= 0xFF << (8 * k);
                        }
                    }
                    return Some((v, 0));
                }
            } else if op == 0 {
                (rep32(vfp_expand_imm(imm8) as u64), false)
            } else {
                return None;
            }
        }
    };
    let kind = match (logic, op) {
        (false, 0) => 0,
        (false, _) => 1,
        (true, 0) => 2,
        (true, _) => 3,
    };
    Some((v, kind))
}

impl Cpu {
    #[inline]
    fn rr(&self, n: u32) -> u32 {
        self.reg(n as u8)
    }

    #[inline]
    fn wr(&mut self, n: u32, v: u32) {
        self.set_reg(n as u8, v)
    }

    /// Reads Q`q` as 16 little-endian bytes.
    #[inline]
    pub(crate) fn q_get(&self, q: u32) -> V {
        let b = (q as usize & 7) * 4;
        let mut v = [0u8; 16];
        for k in 0..4 {
            v[4 * k..4 * k + 4].copy_from_slice(&self.s[b + k].to_le_bytes());
        }
        v
    }

    /// Writes the predicated bytes of `v` into Q`q`.
    #[inline]
    fn q_put(&mut self, q: u32, v: V, pred: u16) {
        let b = (q as usize & 7) * 4;
        if pred == 0xFFFF {
            for k in 0..4 {
                self.s[b + k] = u32::from_le_bytes([v[4 * k], v[4 * k + 1], v[4 * k + 2], v[4 * k + 3]]);
            }
            return;
        }
        for k in 0..4 {
            let mut bytes = self.s[b + k].to_le_bytes();
            for j in 0..4 {
                if (pred >> (4 * k + j)) & 1 != 0 {
                    bytes[j] = v[4 * k + j];
                }
            }
            self.s[b + k] = u32::from_le_bytes(bytes);
        }
    }

    /// The byte predicate of the current instruction: VPR.P0 inside a VPT block, the tail
    /// predicate of a tail-predicated loop, both when both apply, otherwise all lanes.
    #[inline]
    pub(crate) fn mve_pred(&self) -> u16 {
        let mut p = if self.vpr & 0x00FF_0000 != 0 { self.vpr as u16 } else { 0xFFFF };
        let lt = self.ltpsize();
        if lt != 16 {
            p &= vctp_mask(lt, self.r[14]);
        }
        p
    }

    /// The FPSCR view of MVE floating-point instructions as the FVP implements them: single
    /// precision always runs in flush-to-zero and default-NaN mode (FPSCR.FZ and FPSCR.DN are
    /// ignored, FZ16 is honoured for half precision) and the rounding mode is always nearest.
    #[inline]
    fn mve_fpscr(&self) -> u32 {
        (self.fpscr | fpu::FPSCR_FZ | fpu::FPSCR_DN) & !0x00C0_0000
    }

    /// Writes back the cumulative exception flags of an MVE operation.
    #[inline]
    fn set_mve_fpscr(&mut self, f: u32) {
        self.fpscr = (self.fpscr & !0x9F) | (f & 0x9F);
    }

    fn vload<B: Bus>(&mut self, bus: &mut B, addr: u32, sz: usize) -> Result<u64, Trap> {
        if sz == 8 {
            let lo = self.mem_read(bus, addr, 4, true, false)? as u64;
            let hi = self.mem_read(bus, addr.wrapping_add(4), 4, true, false)? as u64;
            Ok(lo | (hi << 32))
        } else {
            Ok(self.mem_read(bus, addr, sz as u8, true, false)? as u64)
        }
    }

    fn vstore<B: Bus>(&mut self, bus: &mut B, addr: u32, sz: usize, x: u64) -> Result<(), Trap> {
        if sz == 8 {
            self.mem_write(bus, addr, 4, x as u32, true, false)?;
            self.mem_write(bus, addr.wrapping_add(4), 4, (x >> 32) as u32, true, false)
        } else {
            self.mem_write(bus, addr, sz as u8, x as u32, true, false)
        }
    }

    /// Stores element `e` of `v` (`dsz` bytes in the register, `msz` in memory) under the
    /// byte predicate: whole element when all its lanes are on, bytewise otherwise.
    #[allow(clippy::too_many_arguments)]
    fn vstore_elem<B: Bus>(&mut self, bus: &mut B, addr: u32, v: &V, e: usize, dsz: usize, msz: usize, pred: u16) -> Result<(), Trap> {
        let lanes = ((1u32 << dsz) - 1) << (e * dsz);
        let p = pred as u32 & lanes;
        if p == 0 {
            return Ok(());
        }
        let x = get(v, e, dsz);
        if p == lanes || dsz != msz {
            return self.vstore(bus, addr, msz, x);
        }
        for k in 0..msz {
            if (p >> (e * dsz + k)) & 1 != 0 {
                self.mem_write(bus, addr.wrapping_add(k as u32), 1, (x >> (8 * k)) as u32 & 0xFF, true, false)?;
            }
        }
        Ok(())
    }

    pub(crate) fn exec_mve<B: Bus>(&mut self, bus: &mut B, enc: u32) -> Result<u32, Trap> {
        self.fp_check(bus)?;
        let h1 = enc & 0xFFFF;
        let h2 = enc >> 16;
        let coproc = (h2 >> 8) & 0xF;
        let done = if h1 & 0x0E00 == 0x0C00 {
            if coproc == 8 {
                self.mve_complex(h1, h2)
            } else {
                self.mve_ldst(bus, h1, h2)?
            }
        } else if h1 & 0x0F00 == 0x0F00 {
            self.mve_simd(h1, h2)
        } else if coproc == 0xB {
            self.mve_lane(h1, h2)
        } else if coproc == 0xE || coproc == 0xF {
            self.mve_dp(h1, h2)
        } else {
            false
        };
        if done {
            Ok(1)
        } else {
            Err(Trap::Unimplemented(enc))
        }
    }

    // ------------------------------------------------------------------ loads and stores

    fn mve_ldst<B: Bus>(&mut self, bus: &mut B, h1: u32, h2: u32) -> Result<bool, Trap> {
        let p = (h1 >> 8) & 1;
        let u = (h1 >> 7) & 1;
        let w = (h1 >> 5) & 1;
        let l = (h1 >> 4) & 1;
        let qd = (h2 >> 13) & 7;
        let pred = self.mve_pred();
        if p == 0 && u == 0 && w == 0 {
            // VMOV between two core registers and two 32-bit lanes (not beat-wise)
            if (h2 >> 5) & 0x7F != 0x78 || (h1 >> 6) & 1 != 0 || (h1 >> 12) & 1 != 0 {
                return Ok(false);
            }
            let rt2 = h1 & 0xF;
            let rt = h2 & 0xF;
            let idx = ((h2 >> 4) & 1) as usize;
            let b = qd as usize * 4;
            if l == 1 {
                self.s[b + idx] = self.rr(rt);
                self.s[b + idx + 2] = self.rr(rt2);
            } else {
                let (a, c) = (self.s[b + idx], self.s[b + idx + 2]);
                self.wr(rt, a);
                self.wr(rt2, c);
            }
            return Ok(true);
        }
        if (h1 >> 12) & 1 == 1 && (h2 >> 12) & 1 == 1 && p == 0 {
            // VLD2n/VLD4n/VST2n/VST4n: de-interleaving loads and interleaving stores, pattern n
            // covers bytes [16n, 16n + 16) of the structure stream; not predicated
            if u == 0 || (h1 >> 6) & 1 != 0 || (h2 >> 1) & 0xF != 0 {
                return Ok(false);
            }
            let rn = h1 & 0xF;
            let sz = 1usize << ((h2 >> 7) & 3);
            let four = h2 & 1 == 1;
            let regs = if four { 4 } else { 2 };
            let pat = ((h2 >> 5) & 3) as usize;
            if sz == 8 || rn == 15 || pat >= regs || qd as usize + regs > 8 {
                return Ok(false);
            }
            let base = self.reg(rn as u8);
            let n = 16 / sz;
            let per_reg = n / regs;
            let mut qs: Vec<V> = (0..regs).map(|k| self.q_get(qd + k as u32)).collect();
            for e in 0..n {
                let addr = base.wrapping_add((pat * 16 + e * sz) as u32);
                let (r, lane) = (e % regs, pat * per_reg + e / regs);
                if l == 1 {
                    let x = self.vload(bus, addr, sz)?;
                    put(&mut qs[r], lane, sz, x);
                } else {
                    self.vstore(bus, addr, sz, get(&qs[r], lane, sz))?;
                }
            }
            if l == 1 {
                for (k, q) in qs.into_iter().enumerate() {
                    self.q_put(qd + k as u32, q, 0xFFFF);
                }
            }
            if w == 1 {
                self.set_reg(rn as u8, base.wrapping_add(16 * regs as u32));
            }
            return Ok(true);
        }
        if p == 0 && w == 0 {
            // gather load / scatter store [Rn, Qm{, UXTW #n}]
            if (h2 >> 12) & 1 == 1 {
                return Ok(false);
            }
            let rn = h1 & 0xF;
            let qm = (h2 >> 1) & 7;
            let dsz = 1usize << ((h2 >> 7) & 3);
            let msz = 1usize << ((((h2 >> 6) & 1) << 1) | ((h2 >> 4) & 1));
            let unsigned = (h1 >> 12) & 1 == 1;
            if msz > dsz || rn == 15 {
                return Ok(false);
            }
            let shift = if h2 & 1 == 1 { msz.trailing_zeros() } else { 0 };
            let base = self.rr(rn);
            let offs = self.q_get(qm);
            let n = 16 / dsz;
            if l == 1 {
                let mut v = self.q_get(qd);
                for e in 0..n {
                    if !active(pred, e, dsz) {
                        continue;
                    }
                    let addr = base.wrapping_add((get(&offs, e, dsz) as u32) << shift);
                    let x = self.vload(bus, addr, msz)?;
                    let x = if unsigned { x } else { sext(x, 8 * msz as u32) as u64 };
                    put(&mut v, e, dsz, x);
                }
                self.q_put(qd, v, pred);
            } else {
                let v = self.q_get(qd);
                for e in 0..n {
                    let addr = base.wrapping_add((get(&offs, e, dsz) as u32) << shift);
                    self.vstore_elem(bus, addr, &v, e, dsz, msz, pred)?;
                }
            }
            return Ok(true);
        }
        let imm7 = h2 & 0x7F;
        if (h1 >> 12) & 1 == 1 && (h2 >> 12) & 1 == 1 {
            // [Qn, #imm]{!}: vector base addresses
            let qn = (h1 >> 1) & 7;
            let sz = match (h2 >> 7) & 3 {
                0 => 4usize,
                2 => 8,
                _ => return Ok(false),
            };
            let imm = imm7 * sz as u32;
            let off = if u == 1 { imm as i64 } else { -(imm as i64) };
            let bases = self.q_get(qn);
            let n = 16 / sz;
            if l == 1 {
                let mut v = self.q_get(qd);
                for e in 0..n {
                    if !active(pred, e, sz) {
                        continue;
                    }
                    let addr = (get(&bases, e, sz) as i64).wrapping_add(off) as u32;
                    let x = self.vload(bus, addr, sz)?;
                    put(&mut v, e, sz, x);
                }
                self.q_put(qd, v, pred);
            } else {
                let v = self.q_get(qd);
                for e in 0..n {
                    let addr = (get(&bases, e, sz) as i64).wrapping_add(off) as u32;
                    self.vstore_elem(bus, addr, &v, e, sz, sz, pred)?;
                }
            }
            if w == 1 {
                let mut nb = bases;
                for e in 0..n {
                    put(&mut nb, e, sz, (get(&bases, e, sz) as i64).wrapping_add(off) as u64 & bmask(8 * sz as u32));
                }
                self.q_put(qn, nb, pred);
            }
            return Ok(true);
        }
        let (msz, dsz, unsigned, rn) = if (h2 >> 12) & 1 == 1 {
            let sz = 1usize << ((h2 >> 7) & 3);
            if sz == 8 {
                return Ok(false);
            }
            (sz, sz, true, h1 & 0xF)
        } else {
            let msz = 1usize << ((h1 >> 3) & 1);
            let dsz = 1usize << ((h2 >> 7) & 3);
            if dsz <= msz || dsz == 8 {
                return Ok(false);
            }
            (msz, dsz, (h1 >> 12) & 1 == 1, h1 & 7)
        };
        if rn == 15 {
            return Ok(false);
        }
        let imm = imm7 * msz as u32;
        let off = if u == 1 { imm } else { imm.wrapping_neg() };
        let base = self.rr(rn);
        let addr = if p == 1 { base.wrapping_add(off) } else { base };
        let n = 16 / dsz;
        if l == 1 {
            let mut v = self.q_get(qd);
            for e in 0..n {
                if !active(pred, e, dsz) {
                    continue;
                }
                let x = self.vload(bus, addr.wrapping_add((e * msz) as u32), msz)?;
                let x = if unsigned { x } else { sext(x, 8 * msz as u32) as u64 };
                put(&mut v, e, dsz, x);
            }
            self.q_put(qd, v, pred);
        } else {
            let v = self.q_get(qd);
            for e in 0..n {
                self.vstore_elem(bus, addr.wrapping_add((e * msz) as u32), &v, e, dsz, msz, pred)?;
            }
        }
        if w == 1 {
            self.wr(rn, base.wrapping_add(off));
        }
        Ok(true)
    }

    // ------------------------------------------------------------- 0xEF/0xFF data processing

    fn mve_simd(&mut self, h1: u32, h2: u32) -> bool {
        let u = (h1 >> 12) & 1 == 1;
        let qd = (h2 >> 13) & 7;
        let qm = (h2 >> 1) & 7;
        let qn = (h1 >> 1) & 7;
        let op = (h2 >> 8) & 0xF;
        let b4 = (h2 >> 4) & 1 == 1;
        if (h2 >> 6) & 1 == 0 {
            return false;
        }
        let pred = self.mve_pred();
        if (h1 >> 7) & 1 == 0 {
            return self.mve_three_reg(h1, h2, u, qd, qn, qm, op, b4, pred);
        }
        if (h1 >> 3) & 7 == 0 && (h2 >> 7) & 1 == 0 && b4 {
            // one register and a modified immediate
            let imm8 = (((h1 >> 12) & 1) << 7) | ((h1 & 7) << 4) | (h2 & 0xF);
            let o = (h2 >> 5) & 1;
            let Some((imm, kind)) = expand_imm(op, o, imm8) else { return false };
            let d = self.q_get(qd);
            let mut v = [0u8; 16];
            for k in 0..16 {
                let i = (imm >> (8 * (k % 8))) as u8;
                v[k] = match kind {
                    0 => i,
                    1 => !i,
                    2 => d[k] | i,
                    _ => d[k] & !i,
                };
            }
            self.q_put(qd, v, pred);
            return true;
        }
        if u && (h1 >> 4) & 3 == 3 && !b4 && (h2 >> 11) & 1 == 0 {
            return self.mve_two_reg_misc(h1, h2, qd, qm, pred);
        }
        if !b4 || (h2 >> 7) & 1 == 1 {
            return false;
        }
        // shift by immediate
        let imm6 = h1 & 0x3F;
        let (sz, left, right) = if imm6 & 0x38 == 0x08 {
            (1usize, imm6 - 8, 16 - imm6)
        } else if imm6 & 0x30 == 0x10 {
            (2, imm6 - 16, 32 - imm6)
        } else if imm6 & 0x20 != 0 {
            (4, imm6 - 32, 64 - imm6)
        } else {
            return false;
        };
        let bits = 8 * sz as u32;
        let m = self.q_get(qm);
        let d = self.q_get(qd);
        let mut qc = false;
        let v = match op {
            0x0 => map1(&m, sz, pred, |x| {
                let x = if u { x as i128 } else { sext(x, bits) as i128 };
                shift_lane(x, -(right as i32), bits, false, !u) as u64 & bmask(bits)
            }),
            0x2 => map1(&m, sz, pred, |x| {
                let x = if u { x as i128 } else { sext(x, bits) as i128 };
                shift_lane(x, -(right as i32), bits, true, !u) as u64 & bmask(bits)
            }),
            0x4 if u => {
                // VSRI: shift right and insert
                let mut v = d;
                for e in 0..16 / sz {
                    let keep = !(bmask(bits) >> right) & bmask(bits);
                    put(&mut v, e, sz, (get(&d, e, sz) & keep) | (get(&m, e, sz) >> right));
                }
                v
            }
            0x5 if u => {
                // VSLI: shift left and insert
                let mut v = d;
                for e in 0..16 / sz {
                    let keep = bmask(left);
                    put(&mut v, e, sz, (get(&d, e, sz) & keep) | ((get(&m, e, sz) << left) & bmask(bits)));
                }
                v
            }
            0x5 => map1(&m, sz, pred, |x| (x << left) & bmask(bits)),
            0x6 if u => {
                // VQSHLU: signed value, unsigned saturated result
                let mut v = [0u8; 16];
                for e in 0..16 / sz {
                    let (r, s) = sat_u((gets(&m, e, sz) as i128) << left, bits);
                    qc |= s;
                    put(&mut v, e, sz, r);
                }
                v
            }
            0x7 => {
                let mut v = [0u8; 16];
                for e in 0..16 / sz {
                    let (r, s) =
                        if u { sat_u((get(&m, e, sz) as i128) << left, bits) } else { sat_s((gets(&m, e, sz) as i128) << left, bits) };
                    qc |= s;
                    put(&mut v, e, sz, r);
                }
                v
            }
            0xC | 0xE => {
                // VCVT from fixed point
                let fsz = if op == 0xC { 2 } else { 4 };
                if sz != fsz {
                    return false;
                }
                let fb = 64 - imm6;
                let mut fpscr = self.mve_fpscr();
                let v = map1(&m, sz, pred, |x| fpu::from_fixed(x as u32, fprec(sz), 8 * sz as u32, fb, u, false, &mut fpscr));
                self.set_mve_fpscr(fpscr);
                v
            }
            0xD | 0xF => {
                let fsz = if op == 0xD { 2 } else { 4 };
                if sz != fsz {
                    return false;
                }
                let fb = 64 - imm6;
                let mut fpscr = self.mve_fpscr();
                let v = map1(&m, sz, pred, |x| {
                    fpu::to_fixed(x, fprec(sz), 8 * sz as u32, fb, u, Some(Rounding::Zero), &mut fpscr) as u64 & bmask(bits)
                });
                self.set_mve_fpscr(fpscr);
                v
            }
            _ => return false,
        };
        if qc {
            self.fpscr |= FPSCR_QC;
        }
        self.q_put(qd, v, pred);
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn mve_three_reg(&mut self, h1: u32, _h2: u32, u: bool, qd: u32, qn: u32, qm: u32, op: u32, b4: bool, pred: u16) -> bool {
        let size = (h1 >> 4) & 3;
        let sz = 1usize << size;
        let bits = 8 * sz as u32;
        let fsub = (h1 >> 5) & 1 == 1;
        let fsz = if (h1 >> 4) & 1 == 1 { 2usize } else { 4 };
        let fp = fprec(fsz);
        let a = self.q_get(qn);
        let b = self.q_get(qm);
        let d = self.q_get(qd);
        let mut qc = false;
        let mut fpscr = self.mve_fpscr();
        let int_ok = size != 3;
        let v = match (op, b4) {
            (0x0, false) if int_ok => map2(&a, &b, sz, pred, |x, y| {
                let (x, y) = if u { (x as i128, y as i128) } else { (sext(x, bits) as i128, sext(y, bits) as i128) };
                ((x + y) >> 1) as u64 & bmask(bits)
            }),
            (0x0, true) if int_ok => {
                let mut v = [0u8; 16];
                for e in 0..16 / sz {
                    let (r, s) = if u {
                        sat_u(get(&a, e, sz) as i128 + get(&b, e, sz) as i128, bits)
                    } else {
                        sat_s(gets(&a, e, sz) as i128 + gets(&b, e, sz) as i128, bits)
                    };
                    qc |= s;
                    put(&mut v, e, sz, r);
                }
                v
            }
            (0x1, false) if int_ok => map2(&a, &b, sz, pred, |x, y| {
                let (x, y) = if u { (x as i128, y as i128) } else { (sext(x, bits) as i128, sext(y, bits) as i128) };
                ((x + y + 1) >> 1) as u64 & bmask(bits)
            }),
            (0x1, true) => match (u, size) {
                (false, 0) => map2(&a, &b, 1, pred, |x, y| x & y),
                (false, 1) => map2(&a, &b, 1, pred, |x, y| x & !y),
                (false, 2) => map2(&a, &b, 1, pred, |x, y| x | y),
                (false, 3) => map2(&a, &b, 1, pred, |x, y| (x | !y) & 0xFF),
                (true, 0) => map2(&a, &b, 1, pred, |x, y| x ^ y),
                _ => return false,
            },
            (0x2, false) if int_ok => map2(&a, &b, sz, pred, |x, y| {
                let (x, y) = if u { (x as i128, y as i128) } else { (sext(x, bits) as i128, sext(y, bits) as i128) };
                ((x - y) >> 1) as u64 & bmask(bits)
            }),
            (0x2, true) if int_ok => {
                let mut v = [0u8; 16];
                for e in 0..16 / sz {
                    let (r, s) = if u {
                        sat_u(get(&a, e, sz) as i128 - get(&b, e, sz) as i128, bits)
                    } else {
                        sat_s(gets(&a, e, sz) as i128 - gets(&b, e, sz) as i128, bits)
                    };
                    qc |= s;
                    put(&mut v, e, sz, r);
                }
                v
            }
            (0x4, _) | (0x5, _) if int_ok => {
                // VSHL/VQSHL/VRSHL/VQRSHL by the signed low byte of Qn (the operands are
                // reversed in this encoding: Qm is the value, Qn the shift)
                let round = op == 0x5;
                let mut v = [0u8; 16];
                for e in 0..16 / sz {
                    let sh = (get(&a, e, sz) & 0xFF) as u8 as i8 as i32;
                    let x = if u { get(&b, e, sz) as i128 } else { gets(&b, e, sz) as i128 };
                    let r = shift_lane(x, sh, bits, round, !u);
                    let r = if b4 {
                        let (r, s) = if u { sat_u(r, bits) } else { sat_s(r, bits) };
                        qc |= s;
                        r
                    } else {
                        r as u64 & bmask(bits)
                    };
                    put(&mut v, e, sz, r);
                }
                v
            }
            (0x6, _) if int_ok => map2(&a, &b, sz, pred, |x, y| {
                let pick = if u { (x > y) != b4 } else { (sext(x, bits) > sext(y, bits)) != b4 };
                if pick {
                    x
                } else {
                    y
                }
            }),
            (0x7, false) if int_ok => map2(&a, &b, sz, pred, |x, y| {
                let (x, y) = if u { (x as i128, y as i128) } else { (sext(x, bits) as i128, sext(y, bits) as i128) };
                (x - y).unsigned_abs() as u64 & bmask(bits)
            }),
            (0x8, false) if int_ok => map2(&a, &b, sz, pred, |x, y| if u { x.wrapping_sub(y) } else { x.wrapping_add(y) } & bmask(bits)),
            (0x9, true) if !u && int_ok => map2(&a, &b, sz, pred, |x, y| x.wrapping_mul(y) & bmask(bits)),
            (0xB, false) if int_ok => {
                let mut v = [0u8; 16];
                for e in 0..16 / sz {
                    let p = 2 * gets(&a, e, sz) as i128 * gets(&b, e, sz) as i128;
                    let p = if u { p + (1i128 << (bits - 1)) } else { p };
                    let (r, s) = sat_s(p >> bits, bits);
                    qc |= s;
                    put(&mut v, e, sz, r);
                }
                v
            }
            (0xC, true) => {
                // VFMA/VFMS: Qd = Qd + (±Qn) * Qm
                let mut r = [0u8; 16];
                for e in 0..16 / fsz {
                    if !active(pred, e, fsz) {
                        continue;
                    }
                    let x = get(&a, e, fsz);
                    let x = if fsub { fpu::neg(x, fp) } else { x };
                    put(&mut r, e, fsz, fpu::mul_add(get(&d, e, fsz), x, get(&b, e, fsz), fp, &mut fpscr));
                }
                r
            }
            (0xD, false) if !u => {
                map2(&a, &b, fsz, pred, |x, y| if fsub { fpu::sub(x, y, fp, &mut fpscr) } else { fpu::add(x, y, fp, &mut fpscr) })
            }
            (0xD, false) if u && fsub => map2(&a, &b, fsz, pred, |x, y| fpu::abs(fpu::sub(x, y, fp, &mut fpscr), fp)),
            (0xD, true) if u && !fsub => map2(&a, &b, fsz, pred, |x, y| fpu::mul(x, y, fp, &mut fpscr)),
            (0xF, true) if u => map2(&a, &b, fsz, pred, |x, y| fpu::max_min_nm(x, y, fp, !fsub, &mut fpscr)),
            _ => return false,
        };
        self.set_mve_fpscr(fpscr);
        if qc {
            self.fpscr |= FPSCR_QC;
        }
        self.q_put(qd, v, pred);
        true
    }

    fn mve_two_reg_misc(&mut self, h1: u32, h2: u32, qd: u32, qm: u32, pred: u16) -> bool {
        let size = (h1 >> 2) & 3;
        let sz = 1usize << size;
        let bits = 8 * sz as u32;
        let opc1 = h1 & 3;
        let opc2 = (h2 >> 7) & 0xF;
        let m = self.q_get(qm);
        let mut qc = false;
        let mut fpscr = self.mve_fpscr();
        let fp = fprec(sz);
        let float_ok = sz == 2 || sz == 4;
        let v = match (opc1, opc2) {
            (0, 0..=2) => {
                // VREV64/VREV32/VREV16: reverse the elements within containers
                let csz = 8usize >> opc2;
                if csz <= sz {
                    return false;
                }
                let mut v = [0u8; 16];
                for c in 0..16 / csz {
                    let per = csz / sz;
                    for e in 0..per {
                        put(&mut v, c * per + e, sz, get(&m, c * per + (per - 1 - e), sz));
                    }
                }
                v
            }
            (0, 0x8) if size != 3 => map1(&m, sz, pred, |x| {
                let s = sext(x, bits);
                let x = ((s ^ (s >> 1)) as u64) & bmask(bits);
                (x.leading_zeros() - (64 - bits) - 1).min(bits - 1) as u64
            }),
            (0, 0x9) if size != 3 => map1(&m, sz, pred, |x| (x.leading_zeros() - (64 - bits)) as u64),
            (0, 0xB) => map1(&m, 1, pred, |x| !x & 0xFF),
            (0, 0xE) if size != 3 => {
                let mut v = [0u8; 16];
                for e in 0..16 / sz {
                    let (r, s) = sat_s((gets(&m, e, sz) as i128).abs(), bits);
                    qc |= s;
                    put(&mut v, e, sz, r);
                }
                v
            }
            (0, 0xF) if size != 3 => {
                let mut v = [0u8; 16];
                for e in 0..16 / sz {
                    let (r, s) = sat_s(-(gets(&m, e, sz) as i128), bits);
                    qc |= s;
                    put(&mut v, e, sz, r);
                }
                v
            }
            (1, 0x6) if size != 3 => map1(&m, sz, pred, |x| sext(x, bits).unsigned_abs() & bmask(bits)),
            (1, 0x7) if size != 3 => map1(&m, sz, pred, |x| x.wrapping_neg() & bmask(bits)),
            (1, 0xE) if float_ok => map1(&m, sz, pred, |x| fpu::abs(x, fp)),
            (1, 0xF) if float_ok => map1(&m, sz, pred, |x| fpu::neg(x, fp)),
            (2, 0x8..=0xF) if float_ok => {
                let (mode, exact) = match opc2 {
                    0x8 => (Some(Rounding::Nearest), false),
                    0x9 => (None, true),
                    0xA => (Some(Rounding::Away), false),
                    0xB => (Some(Rounding::Zero), false),
                    0xD => (Some(Rounding::MinusInf), false),
                    0xF => (Some(Rounding::PlusInf), false),
                    _ => return false,
                };
                map1(&m, sz, pred, |x| fpu::rint(x, fp, mode, exact, &mut fpscr))
            }
            (3, 0x0..=0x7) if float_ok => {
                let mode = match opc2 >> 1 {
                    0 => Rounding::Away,
                    1 => Rounding::Nearest,
                    2 => Rounding::PlusInf,
                    _ => Rounding::MinusInf,
                };
                let unsigned = opc2 & 1 == 1;
                map1(&m, sz, pred, |x| fpu::to_fixed(x, fp, bits, 0, unsigned, Some(mode), &mut fpscr) as u64 & bmask(bits))
            }
            (3, 0xC) if float_ok => map1(&m, sz, pred, |x| fpu::from_fixed(x as u32, fp, bits, 0, false, true, &mut fpscr)),
            (3, 0xD) if float_ok => map1(&m, sz, pred, |x| fpu::from_fixed(x as u32, fp, bits, 0, true, true, &mut fpscr)),
            (3, 0xE) if float_ok => {
                map1(&m, sz, pred, |x| fpu::to_fixed(x, fp, bits, 0, false, Some(Rounding::Zero), &mut fpscr) as u64 & bmask(bits))
            }
            (3, 0xF) if float_ok => {
                map1(&m, sz, pred, |x| fpu::to_fixed(x, fp, bits, 0, true, Some(Rounding::Zero), &mut fpscr) as u64 & bmask(bits))
            }
            _ => return false,
        };
        self.set_mve_fpscr(fpscr);
        if qc {
            self.fpscr |= FPSCR_QC;
        }
        self.q_put(qd, v, pred);
        true
    }

    // ------------------------------------------------- VDUP and VMOV between a lane and a core register

    fn mve_lane(&mut self, h1: u32, h2: u32) -> bool {
        if (h2 >> 4) & 1 == 0 || (h1 >> 8) & 0xF != 0xE {
            return false;
        }
        let rt = (h2 >> 12) & 0xF;
        if rt == 13 || rt == 15 {
            return false;
        }
        if (h1 >> 7) & 1 == 1 && (h1 >> 4) & 1 == 0 {
            // VDUP
            if (h1 >> 5) & 1 == 0 || (h2 >> 6) & 3 != 0 {
                return false;
            }
            let qd = (h1 >> 1) & 7;
            let sz = if (h1 >> 6) & 1 == 1 {
                1
            } else if (h2 >> 5) & 1 == 1 {
                2
            } else {
                4
            };
            let x = self.rr(rt) as u64;
            let mut v = [0u8; 16];
            for e in 0..16 / sz {
                put(&mut v, e, sz, x & bmask(8 * sz as u32));
            }
            let pred = self.mve_pred();
            self.q_put(qd, v, pred);
            return true;
        }
        let unsigned = (h1 >> 7) & 1 == 1;
        let opc1 = (h1 >> 5) & 3;
        let l = (h1 >> 4) & 1;
        let vn = (h1 & 0xF) | (((h2 >> 7) & 1) << 4);
        let opc2 = (h2 >> 5) & 3;
        let (sz, idx) = if opc1 & 2 != 0 {
            (1usize, ((opc1 & 1) << 2) | opc2)
        } else if opc2 & 1 != 0 {
            (2, ((opc1 & 1) << 1) | (opc2 >> 1))
        } else if opc2 == 0 {
            (4, opc1 & 1)
        } else {
            return false;
        };
        if vn >= 16 || (l == 0 && unsigned) {
            return false;
        }
        let q = vn >> 1;
        let byte = ((vn & 1) as usize) * 8 + idx as usize * sz;
        let mut v = self.q_get(q);
        if l == 1 {
            let mut x = 0u64;
            for k in 0..sz {
                x |= (v[byte + k] as u64) << (8 * k);
            }
            let x = if unsigned || sz == 4 { x as u32 } else { sext(x, 8 * sz as u32) as u32 };
            self.wr(rt, x);
        } else {
            let x = self.rr(rt);
            for k in 0..sz {
                v[byte + k] = (x >> (8 * k)) as u8;
            }
            self.q_put(q, v, 0xFFFF);
        }
        true
    }

    // --------------------------------------------- 0xEE/0xFE: scalar operands, compares, reductions

    fn mve_dp(&mut self, h1: u32, h2: u32) -> bool {
        let u = (h1 >> 12) & 1 == 1;
        if (h1 >> 8) & 0xF != 0xE {
            return false;
        }
        let size = (h1 >> 4) & 3;
        let qn = (h1 >> 1) & 7;
        let b0 = h1 & 1 == 1;
        let qd = (h2 >> 13) & 7;
        let t = (h2 >> 12) & 1 == 1;
        let f = (h2 >> 8) & 1 == 1;
        let bit7 = (h2 >> 7) & 1 == 1;
        let op3 = (h2 >> 4) & 7;
        let qm = (h2 >> 1) & 7;
        let c0 = h2 & 1 == 1;
        let rm = h2 & 0xF;
        let pred = self.mve_pred();
        match (h1 >> 6) & 3 {
            0 | 1 => {
                if f {
                    if b0 && op3 & 1 == 0 && (u || size == 3) && (op3 & 2 == 0 || op3 & 4 != 0) {
                        return self.mve_cmp(h1, h2, u, size, qn, qd, t, bit7, qm, rm, pred);
                    }
                    match (bit7, op3) {
                        (false, 4) if b0 => {
                            // VADD/VSUB (vector, scalar)
                            if u || size == 3 || rm == 13 || rm == 15 {
                                return false;
                            }
                            let sz = 1usize << size;
                            let bits = 8 * sz as u32;
                            let s = self.rr(rm) as u64 & bmask(bits);
                            let a = self.q_get(qn);
                            let v = map1(&a, sz, pred, |x| if t { x.wrapping_sub(s) } else { x.wrapping_add(s) } & bmask(bits));
                            self.q_put(qd, v, pred);
                            true
                        }
                        (false, 4) => {
                            if rm == 13 || rm == 15 {
                                return false;
                            }
                            if size == 3 {
                                // VADD.F/VSUB.F (vector, scalar)
                                let fsz = if u { 2 } else { 4 };
                                let fp = fprec(fsz);
                                let s = self.rr(rm) as u64 & bmask(8 * fsz as u32);
                                let a = self.q_get(qn);
                                let mut fpscr = self.mve_fpscr();
                                let v =
                                    map1(
                                        &a,
                                        fsz,
                                        pred,
                                        |x| if t { fpu::sub(x, s, fp, &mut fpscr) } else { fpu::add(x, s, fp, &mut fpscr) },
                                    );
                                self.set_mve_fpscr(fpscr);
                                self.q_put(qd, v, pred);
                                true
                            } else {
                                // VHADD/VHSUB (vector, scalar)
                                let sz = 1usize << size;
                                let bits = 8 * sz as u32;
                                let s = self.rr(rm) as u64 & bmask(bits);
                                let s = if u { s as i128 } else { sext(s, bits) as i128 };
                                let a = self.q_get(qn);
                                let v = map1(&a, sz, pred, |x| {
                                    let x = if u { x as i128 } else { sext(x, bits) as i128 };
                                    (if t { x - s } else { x + s } >> 1) as u64 & bmask(bits)
                                });
                                self.q_put(qd, v, pred);
                                true
                            }
                        }
                        (_, 6) if b0 => {
                            // VIDUP/VDDUP/VIWDUP/VDWDUP
                            if u || size == 3 || op3 != 6 {
                                return false;
                            }
                            let sz = 1usize << size;
                            let bits = 8 * sz as u32;
                            let imm = 1u32 << (((bit7 as u32) << 1) | c0 as u32);
                            let rn = ((h1 >> 1) & 7) << 1;
                            let rmw = (((h2 >> 1) & 7) << 1) | 1;
                            let wrap = if rmw == 15 { None } else { Some(self.rr(rmw)) };
                            let mut off = self.rr(rn);
                            let mut v = [0u8; 16];
                            for e in 0..16 / sz {
                                put(&mut v, e, sz, off as u64 & bmask(bits));
                                match (t, wrap) {
                                    (false, None) => off = off.wrapping_add(imm),
                                    (true, None) => off = off.wrapping_sub(imm),
                                    (false, Some(w)) => {
                                        // wraps only when the wrap value is hit exactly (FVP)
                                        off = off.wrapping_add(imm);
                                        if off == w {
                                            off = 0;
                                        }
                                    }
                                    (true, Some(w)) => {
                                        if off == 0 {
                                            off = w.wrapping_sub(imm);
                                        } else {
                                            off = off.wrapping_sub(imm);
                                        }
                                    }
                                }
                            }
                            self.wr(rn, off);
                            self.q_put(qd, v, pred);
                            true
                        }
                        (false, 6) => {
                            if rm == 13 || rm == 15 {
                                return false;
                            }
                            if size == 3 {
                                // VQDMULLB/T (vector, scalar)
                                let ssz = if u { 4usize } else { 2 };
                                let s = sext(self.rr(rm) as u64, 8 * ssz as u32) as i128;
                                let a = self.q_get(qn);
                                let mut v = [0u8; 16];
                                let mut qc = false;
                                for e in 0..16 / (2 * ssz) {
                                    let x = gets(&a, 2 * e + t as usize, ssz) as i128;
                                    let (r, sat) = sat_s(2 * x * s, 16 * ssz as u32);
                                    qc |= sat;
                                    put(&mut v, e, 2 * ssz, r);
                                }
                                if qc {
                                    self.fpscr |= FPSCR_QC;
                                }
                                self.q_put(qd, v, pred);
                                true
                            } else {
                                // VQADD/VQSUB (vector, scalar)
                                let sz = 1usize << size;
                                let bits = 8 * sz as u32;
                                let s = self.rr(rm) as u64 & bmask(bits);
                                let s = if u { s as i128 } else { sext(s, bits) as i128 };
                                let a = self.q_get(qn);
                                let mut v = [0u8; 16];
                                let mut qc = false;
                                for e in 0..16 / sz {
                                    let x = if u { get(&a, e, sz) as i128 } else { gets(&a, e, sz) as i128 };
                                    let r = if t { x - s } else { x + s };
                                    let (r, sat) = if u { sat_u(r, bits) } else { sat_s(r, bits) };
                                    qc |= sat;
                                    put(&mut v, e, sz, r);
                                }
                                if qc {
                                    self.fpscr |= FPSCR_QC;
                                }
                                self.q_put(qd, v, pred);
                                true
                            }
                        }
                        (false, 0) if !b0 && !c0 && size == 3 => {
                            // VADC/VADCI/VSBC/VSBCI: 32-bit lanes with a carry chain through FPSCR.C
                            let a = self.q_get(qn);
                            let b = self.q_get(qm);
                            let mut carry = if t { u as u32 } else { (self.fpscr >> 29) & 1 };
                            let mut v = [0u8; 16];
                            for e in 0..4 {
                                let y = if u { !(get(&b, e, 4) as u32) } else { get(&b, e, 4) as u32 };
                                let sum = get(&a, e, 4) + y as u64 + carry as u64;
                                carry = (sum >> 32) as u32 & 1;
                                put(&mut v, e, 4, sum & 0xFFFF_FFFF);
                            }
                            self.fpscr = (self.fpscr & !FPSCR_C) | (carry << 29);
                            self.q_put(qd, v, pred);
                            true
                        }
                        (false, 0) if !b0 && !c0 => {
                            // VCADD (u) / VHCADD: complex add with rotation 90 (t = 0) or 270
                            let sz = 1usize << size;
                            let bits = 8 * sz as u32;
                            let a = self.q_get(qn);
                            let b = self.q_get(qm);
                            let mut v = [0u8; 16];
                            for p in 0..16 / sz / 2 {
                                let (re, im) = (2 * p, 2 * p + 1);
                                let (nr, ni, mr, mi) = (
                                    gets(&a, re, sz) as i128,
                                    gets(&a, im, sz) as i128,
                                    gets(&b, re, sz) as i128,
                                    gets(&b, im, sz) as i128,
                                );
                                let (r, i) = if t { (nr + mi, ni - mr) } else { (nr - mi, ni + mr) };
                                let (r, i) = if u { (r, i) } else { (r >> 1, i >> 1) };
                                put(&mut v, re, sz, r as u64 & bmask(bits));
                                put(&mut v, im, sz, i as u64 & bmask(bits));
                            }
                            self.q_put(qd, v, pred);
                            true
                        }
                        (false, 0) if !b0 && c0 && size == 3 => {
                            // VQDMULLB/T (vector)
                            let ssz = if u { 4usize } else { 2 };
                            let a = self.q_get(qn);
                            let b = self.q_get(qm);
                            let mut v = [0u8; 16];
                            let mut qc = false;
                            for e in 0..16 / (2 * ssz) {
                                let x = gets(&a, 2 * e + t as usize, ssz) as i128;
                                let y = gets(&b, 2 * e + t as usize, ssz) as i128;
                                let (r, sat) = sat_s(2 * x * y, 16 * ssz as u32);
                                qc |= sat;
                                put(&mut v, e, 2 * ssz, r);
                            }
                            if qc {
                                self.fpscr |= FPSCR_QC;
                            }
                            self.q_put(qd, v, pred);
                            true
                        }
                        _ => false,
                    }
                } else {
                    self.mve_dp_e(h1, h2, u, size, qn, b0, qd, t, bit7, op3, qm, rm, c0, pred)
                }
            }
            2 => self.mve_long(h1, h2, u, qn, b0, qd, t, bit7, qm, rm, c0, f, pred),
            _ => self.mve_reduce(h1, h2, u, size, qn, b0, qd, t, bit7, qm, c0, f, pred),
        }
    }

    /// VCMP, VPT, VPST, VPNOT, VPSEL.
    #[allow(clippy::too_many_arguments)]
    fn mve_cmp(
        &mut self,
        h1: u32,
        h2: u32,
        u: bool,
        size: u32,
        qn: u32,
        qd: u32,
        t: bool,
        bit7: bool,
        qm: u32,
        rm: u32,
        pred: u16,
    ) -> bool {
        let mask = (((h1 >> 6) & 1) << 3) | ((h2 >> 13) & 7);
        let reg = (h2 >> 6) & 1 == 1;
        let fc0 = if reg { (h2 >> 5) & 1 == 1 } else { h2 & 1 == 1 };
        let fc = ((t as u32) << 2) | ((bit7 as u32) << 1) | fc0 as u32;
        let float = size == 3;
        if reg && rm == 13 {
            if !(float && u) || fc != 0 {
                return false;
            }
            if mask == 0 {
                // VPNOT
                self.vpr ^= 0xFFFF;
            } else {
                // VPST
                self.vpr = (self.vpr & 0xFF00_FFFF) | (mask << 16) | (mask << 20);
                self.vpt_set = true;
            }
            return true;
        }
        if float && u && !reg && fc == 1 {
            // VPSEL: Qd = P0 ? Qn : Qm per byte lane (hw2[15:13] is Qd here)
            let a = self.q_get(qn);
            let b = self.q_get(qm);
            let p0 = self.vpr as u16;
            let mut v = b;
            for k in 0..16 {
                if (p0 >> k) & 1 != 0 {
                    v[k] = a[k];
                }
            }
            // the selection itself is not predicated by P0, but is by a tail predicate
            let lt = self.ltpsize();
            let tp = if lt != 16 { vctp_mask(lt, self.r[14]) } else { 0xFFFF };
            self.q_put(qd, v, tp);
            return true;
        }
        let sz = if float {
            if u {
                2usize
            } else {
                4
            }
        } else {
            1usize << size
        };
        let bits = 8 * sz as u32;
        let a = self.q_get(qn);
        let b = if reg {
            let s = if rm == 15 { 0 } else { self.rr(rm) as u64 & bmask(bits) };
            let mut v = [0u8; 16];
            for e in 0..16 / sz {
                put(&mut v, e, sz, s);
            }
            v
        } else {
            self.q_get(qm)
        };
        let mut p0: u16 = 0;
        let mut fpscr = self.mve_fpscr();
        for e in 0..16 / sz {
            if !active(pred, e, sz) {
                continue;
            }
            let (x, y) = (get(&a, e, sz), get(&b, e, sz));
            let r = if float {
                let nzcv = fpu::compare(x, y, fprec(sz), true, &mut fpscr) >> 28;
                let (n, z, v) = (nzcv & 8 != 0, nzcv & 4 != 0, nzcv & 1 != 0);
                match fc {
                    0 => z,
                    2 => !z,
                    4 => n == v,
                    6 => n != v,
                    5 => !z && n == v,
                    7 => z || n != v,
                    _ => return false,
                }
            } else {
                let (sx, sy) = (sext(x, bits), sext(y, bits));
                match fc {
                    0 => x == y,
                    2 => x != y,
                    1 => x >= y,
                    3 => x > y,
                    4 => sx >= sy,
                    6 => sx < sy,
                    5 => sx > sy,
                    _ => sx <= sy,
                }
            };
            if r {
                p0 |= (((1u32 << sz) - 1) << (e * sz)) as u16;
            }
        }
        self.set_mve_fpscr(fpscr);
        self.vpr = (self.vpr & 0xFFFF_0000) | p0 as u32;
        if mask != 0 {
            self.vpr = (self.vpr & 0xFF00_FFFF) | (mask << 16) | (mask << 20);
            self.vpt_set = true;
        }
        true
    }

    /// The 0xEE/0xFE group with hw2[11:8] = 1110: scalar multiplies, VMULL, VMULH, VMOVN,
    /// VQMOVN, VSHLL (T2), VCVTB/T between halves, VMAXA/VMINA.
    #[allow(clippy::too_many_arguments)]
    fn mve_dp_e(
        &mut self,
        h1: u32,
        _h2: u32,
        u: bool,
        size: u32,
        qn: u32,
        b0: bool,
        qd: u32,
        t: bool,
        bit7: bool,
        op3: u32,
        qm: u32,
        rm: u32,
        c0: bool,
        pred: u16,
    ) -> bool {
        let mut qc = false;
        let mut fpscr = self.mve_fpscr();
        let ok = match (bit7, op3) {
            (_, 6) if b0 && t && size == 3 => {
                // VSHL/VRSHL (bit 7 = 0) and VQSHL/VQRSHL (bit 7 = 1) by the signed low byte of Rm
                if rm == 13 || rm == 15 {
                    return false;
                }
                let sz = 1usize << ((h1 >> 2) & 3);
                if sz == 8 {
                    return false;
                }
                let bits = 8 * sz as u32;
                let round = (h1 >> 1) & 1 == 1;
                let sh = (self.rr(rm) & 0xFF) as u8 as i8 as i32;
                let d = self.q_get(qd);
                let mut v = [0u8; 16];
                for e in 0..16 / sz {
                    let x = if u { get(&d, e, sz) as i128 } else { gets(&d, e, sz) as i128 };
                    let r = shift_lane(x, sh, bits, round, !u);
                    let r = if bit7 {
                        let (r, sat) = if u { sat_u(r, bits) } else { sat_s(r, bits) };
                        qc |= sat;
                        r
                    } else {
                        r as u64 & bmask(bits)
                    };
                    put(&mut v, e, sz, r);
                }
                self.q_put(qd, v, pred);
                true
            }
            (false, 6) if b0 && !t => {
                if rm == 13 || rm == 15 {
                    return false;
                }
                if size == 3 {
                    // VMUL.F (vector, scalar)
                    let fsz = if u { 2 } else { 4 };
                    let fp = fprec(fsz);
                    let s = self.rr(rm) as u64 & bmask(8 * fsz as u32);
                    let a = self.q_get(qn);
                    let v = map1(&a, fsz, pred, |x| fpu::mul(x, s, fp, &mut fpscr));
                    self.q_put(qd, v, pred);
                } else {
                    // VQDMULH/VQRDMULH (vector, scalar)
                    let sz = 1usize << size;
                    let bits = 8 * sz as u32;
                    let s = sext(self.rr(rm) as u64, bits) as i128;
                    let a = self.q_get(qn);
                    let mut v = [0u8; 16];
                    for e in 0..16 / sz {
                        let p = 2 * gets(&a, e, sz) as i128 * s;
                        let p = if u { p + (1i128 << (bits - 1)) } else { p };
                        let (r, sat) = sat_s(p >> bits, bits);
                        qc |= sat;
                        put(&mut v, e, sz, r);
                    }
                    self.q_put(qd, v, pred);
                }
                true
            }
            (false, 6) if b0 => {
                if rm == 13 || rm == 15 || size == 3 {
                    return false;
                }
                let sz = 1usize << size;
                let bits = 8 * sz as u32;
                let a = self.q_get(qn);
                let s = self.rr(rm);
                let v = if u {
                    // VBRSR: bit-reverse the low Rm[7:0] bits of each element
                    let n = (s & 0xFF).min(bits);
                    map1(&a, sz, pred, |x| if n == 0 { 0 } else { (x & bmask(n)).reverse_bits() >> (64 - n) })
                } else {
                    // VMUL (vector, scalar)
                    map1(&a, sz, pred, |x| x.wrapping_mul(s as u64) & bmask(bits))
                };
                self.q_put(qd, v, pred);
                true
            }
            (false, 6) | (false, 4) if !b0 => {
                // VQDMLAH/VQDMLASH (op 6), VQRDMLAH/VQRDMLASH (op 4)
                if rm == 13 || rm == 15 || size == 3 || u {
                    return false;
                }
                let sz = 1usize << size;
                let bits = 8 * sz as u32;
                let s = sext(self.rr(rm) as u64, bits) as i128;
                let a = self.q_get(qn);
                let d = self.q_get(qd);
                let round = op3 == 4;
                let mut v = [0u8; 16];
                for e in 0..16 / sz {
                    let (x, acc) = (gets(&a, e, sz) as i128, gets(&d, e, sz) as i128);
                    let p = if t { 2 * x * acc + (s << bits) } else { 2 * x * s + (acc << bits) };
                    let p = if round { p + (1i128 << (bits - 1)) } else { p };
                    let (r, sat) = sat_s(p >> bits, bits);
                    qc |= sat;
                    put(&mut v, e, sz, r);
                }
                self.q_put(qd, v, pred);
                true
            }
            (false, 4) if b0 => {
                if rm == 13 || rm == 15 {
                    return false;
                }
                let a = self.q_get(qn);
                let d = self.q_get(qd);
                if size == 3 {
                    // VFMA (t = 0): Qd = Qd + Qn * Rm; VFMAS: Qd = Qd * Qn + Rm
                    let fsz = if u { 2 } else { 4 };
                    let fp = fprec(fsz);
                    let s = self.rr(rm) as u64 & bmask(8 * fsz as u32);
                    let mut v = [0u8; 16];
                    for e in 0..16 / fsz {
                        if !active(pred, e, fsz) {
                            continue;
                        }
                        let (x, acc) = (get(&a, e, fsz), get(&d, e, fsz));
                        let r = if t { fpu::mul_add(s, acc, x, fp, &mut fpscr) } else { fpu::mul_add(acc, x, s, fp, &mut fpscr) };
                        put(&mut v, e, fsz, r);
                    }
                    self.q_put(qd, v, pred);
                } else {
                    // VMLA: Qd = Qd + Qn * Rm; VMLAS: Qd = Qd * Qn + Rm
                    let sz = 1usize << size;
                    let bits = 8 * sz as u32;
                    let s = self.rr(rm) as u64;
                    let v = map2(
                        &a,
                        &d,
                        sz,
                        pred,
                        |x, acc| if t { acc.wrapping_mul(x).wrapping_add(s) } else { acc.wrapping_add(x.wrapping_mul(s)) } & bmask(bits),
                    );
                    self.q_put(qd, v, pred);
                }
                true
            }
            (false, 0) if !b0 && size == 3 => {
                // VCMUL.F: complex multiply with rotation (t:c0) × 90°
                let fsz = if u { 4usize } else { 2 };
                let rot = ((t as u32) << 1) | c0 as u32;
                let a = self.q_get(qn);
                let b = self.q_get(qm);
                let v = Self::complex_mul(&a, &b, None, fsz, rot, pred, &mut fpscr);
                self.q_put(qd, v, pred);
                true
            }
            (false, 0) if b0 && !c0 => {
                // VMULLB/VMULLT (integer and polynomial)
                let a = self.q_get(qn);
                let b = self.q_get(qm);
                let mut v = [0u8; 16];
                if size == 3 {
                    let ssz = if u { 2usize } else { 1 };
                    for e in 0..16 / (2 * ssz) {
                        let i = 2 * e + t as usize;
                        put(&mut v, e, 2 * ssz, pmul(get(&a, i, ssz), get(&b, i, ssz), 8 * ssz as u32));
                    }
                } else {
                    let ssz = 1usize << size;
                    for e in 0..16 / (2 * ssz) {
                        let i = 2 * e + t as usize;
                        let r = if u {
                            (get(&a, i, ssz) as i128) * (get(&b, i, ssz) as i128)
                        } else {
                            (gets(&a, i, ssz) as i128) * (gets(&b, i, ssz) as i128)
                        };
                        put(&mut v, e, 2 * ssz, r as u64 & bmask(16 * ssz as u32));
                    }
                }
                self.q_put(qd, v, pred);
                true
            }
            (false, 0) if b0 && c0 && size != 3 => {
                // VMULH (t = 0) / VRMULH
                let sz = 1usize << size;
                let bits = 8 * sz as u32;
                let a = self.q_get(qn);
                let b = self.q_get(qm);
                let v = map2(&a, &b, sz, pred, |x, y| {
                    let p = if u { x as i128 * y as i128 } else { sext(x, bits) as i128 * sext(y, bits) as i128 };
                    let p = if t { p + (1i128 << (bits - 1)) } else { p };
                    (p >> bits) as u64 & bmask(bits)
                });
                self.q_put(qd, v, pred);
                true
            }
            (false, 0) if b0 && c0 => {
                // size == 3: VQMOVN (h1[1] = 1), VSHLL T2 (h1[3:2] < 3), VCVTB/T between halves
                let m = self.q_get(qm);
                if (h1 >> 2) & 3 == 3 {
                    // VCVTB/VCVTT.F16.F32 (narrow, u = 0) / .F32.F16 (widen, u = 1)
                    let mut v = self.q_get(qd);
                    for e in 0..4 {
                        if !active(pred, e, 4) {
                            continue;
                        }
                        if u {
                            // MVE does not flush denormal half inputs under FZ16 (FVP)
                            put(&mut v, e, 4, fpu::convert_from_half(get(&m, 2 * e + t as usize, 2), Prec::S, false, &mut fpscr));
                        } else {
                            put(&mut v, 2 * e + t as usize, 2, fpu::convert(get(&m, e, 4), Prec::S, Prec::H, &mut fpscr));
                        }
                    }
                    self.q_put(qd, v, pred);
                } else if (h1 >> 1) & 1 == 1 {
                    // VQMOVNB/T: saturate the wide elements into the b/t halves
                    let rsz = 1usize << ((h1 >> 2) & 3);
                    if rsz > 2 {
                        return false;
                    }
                    let mut v = self.q_get(qd);
                    for e in 0..16 / (2 * rsz) {
                        let (r, sat) = if u {
                            sat_u(get(&m, e, 2 * rsz) as i128, 8 * rsz as u32)
                        } else {
                            sat_s(gets(&m, e, 2 * rsz) as i128, 8 * rsz as u32)
                        };
                        qc |= sat;
                        put(&mut v, 2 * e + t as usize, rsz, r);
                    }
                    self.q_put(qd, v, pred);
                } else {
                    // VSHLLB/T with shift = element size
                    let ssz = 1usize << ((h1 >> 2) & 3);
                    let mut v = [0u8; 16];
                    for e in 0..16 / (2 * ssz) {
                        let x = if u { get(&m, 2 * e + t as usize, ssz) } else { gets(&m, 2 * e + t as usize, ssz) as u64 };
                        put(&mut v, e, 2 * ssz, (x << (8 * ssz)) & bmask(16 * ssz as u32));
                    }
                    self.q_put(qd, v, pred);
                }
                true
            }
            (true, 0) if b0 && c0 && size == 3 => {
                let m = self.q_get(qm);
                let d = self.q_get(qd);
                if (h1 >> 1) & 1 == 1 {
                    if (h1 >> 2) & 3 == 3 {
                        // VMAXNMA/VMINNMA: Qd = maxnm(|Qd|, |Qm|)
                        let fsz = if u { 2 } else { 4 };
                        let fp = fprec(fsz);
                        let v = map2(&d, &m, fsz, pred, |x, y| fpu::max_min_nm(fpu::abs(x, fp), fpu::abs(y, fp), fp, !t, &mut fpscr));
                        self.q_put(qd, v, pred);
                    } else {
                        // VMAXA/VMINA: Qd (unsigned) against |Qm| (signed)
                        let sz = 1usize << ((h1 >> 2) & 3);
                        let bits = 8 * sz as u32;
                        let v = map2(&d, &m, sz, pred, |x, y| {
                            let ay = sext(y, bits).unsigned_abs() & bmask(bits);
                            if (x > ay) != t {
                                x
                            } else {
                                ay
                            }
                        });
                        self.q_put(qd, v, pred);
                    }
                } else {
                    // VMOVNB/T (u = 1) / VQMOVUNB/T (u = 0)
                    let rsz = 1usize << ((h1 >> 2) & 3);
                    if rsz > 2 {
                        return false;
                    }
                    let mut v = d;
                    for e in 0..16 / (2 * rsz) {
                        let r = if u {
                            get(&m, e, 2 * rsz) & bmask(8 * rsz as u32)
                        } else {
                            let (r, sat) = sat_u(gets(&m, e, 2 * rsz) as i128, 8 * rsz as u32);
                            qc |= sat;
                            r
                        };
                        put(&mut v, 2 * e + t as usize, rsz, r);
                    }
                    self.q_put(qd, v, pred);
                }
                true
            }
            _ => false,
        };
        self.set_mve_fpscr(fpscr);
        if qc {
            self.fpscr |= FPSCR_QC;
        }
        ok
    }

    /// hw1[7:6] = 10: VSHLL/VSHLC, the narrowing shifts, VABAV, VADDLV, VMLALDAV family.
    #[allow(clippy::too_many_arguments)]
    fn mve_long(
        &mut self,
        h1: u32,
        h2: u32,
        u: bool,
        qn: u32,
        b0: bool,
        qd: u32,
        t: bool,
        bit7: bool,
        qm: u32,
        rm: u32,
        c0: bool,
        f: bool,
        pred: u16,
    ) -> bool {
        let mut qc = false;
        if (h2 >> 6) & 1 == 1 {
            if !f {
                return false;
            }
            if (h1 >> 5) & 1 == 1 {
                if bit7 {
                    // VSHLC: shift the whole register left by imm, inserting the low bits of Rdm
                    if (h2 >> 4) & 3 != 0 || rm == 13 || rm == 15 {
                        return false;
                    }
                    let imm = if h1 & 0x1F == 0 { 32 } else { h1 & 0x1F };
                    let q = self.q_get(qd);
                    let x = u128::from_le_bytes(q);
                    let r = self.rr(rm) & bmask(imm) as u32;
                    let out = (x >> (128 - imm)) as u32;
                    let v = ((x << imm) | r as u128).to_le_bytes();
                    self.q_put(qd, v, pred);
                    self.wr(rm, out);
                    return true;
                }
                // VSHLLB/T by immediate
                let imm = h1 & 0x1F;
                let (ssz, sh) = if imm & 0x18 == 0x08 {
                    (1usize, imm - 8)
                } else if imm & 0x10 != 0 {
                    (2, imm - 16)
                } else {
                    return false;
                };
                let m = self.q_get(qm);
                let mut v = [0u8; 16];
                for e in 0..16 / (2 * ssz) {
                    let x = if u { get(&m, 2 * e + t as usize, ssz) } else { gets(&m, 2 * e + t as usize, ssz) as u64 };
                    put(&mut v, e, 2 * ssz, (x << sh) & bmask(16 * ssz as u32));
                }
                self.q_put(qd, v, pred);
                return true;
            }
            // narrowing shifts: VSHRN/VRSHRN (bit7, c0), VQSHRN (!bit7, !c0), VQRSHRN (!bit7, c0),
            // VQSHRUN (bit7, !c0, u = 0), VQRSHRUN (bit7, !c0, u = 1)
            let imm6 = h1 & 0x3F;
            let (rsz, sh) = if imm6 & 0x38 == 0x08 {
                (1usize, 16 - imm6)
            } else if imm6 & 0x30 == 0x10 {
                (2, 32 - imm6)
            } else {
                return false;
            };
            let bits = 8 * rsz as u32;
            let m = self.q_get(qm);
            let mut v = self.q_get(qd);
            for e in 0..16 / (2 * rsz) {
                let x = gets(&m, e, 2 * rsz) as i128;
                let r = match (bit7, c0) {
                    (true, true) => {
                        shift_lane(if u { get(&m, e, 2 * rsz) as i128 } else { x }, -(sh as i32), 2 * bits, u, true) as u64 & bmask(bits)
                    }
                    (false, _) => {
                        let s = shift_lane(if u { get(&m, e, 2 * rsz) as i128 } else { x }, -(sh as i32), 2 * bits, c0, !u);
                        let (r, sat) = if u { sat_u(s, bits) } else { sat_s(s, bits) };
                        qc |= sat;
                        r
                    }
                    (true, false) => {
                        let s = shift_lane(x, -(sh as i32), 2 * bits, u, true);
                        let (r, sat) = sat_u(s, bits);
                        qc |= sat;
                        r
                    }
                };
                put(&mut v, 2 * e + t as usize, rsz, r);
            }
            if qc {
                self.fpscr |= FPSCR_QC;
            }
            self.q_put(qd, v, pred);
            return true;
        }
        if (h2 >> 4) & 1 != 0 || bit7 {
            return false;
        }
        let acc = (h2 >> 5) & 1 == 1;
        let rda = (h2 >> 12) & 0xF;
        let n = self.q_get(qn);
        let m = self.q_get(qm);
        if f {
            if c0 {
                // VABAV: Rda += sum |Qn - Qm|
                let size = (h1 >> 4) & 3;
                if size == 3 || rda == 13 || rda == 15 {
                    return false;
                }
                let sz = 1usize << size;
                let bits = 8 * sz as u32;
                let mut sum = self.rr(rda) as u64;
                for e in 0..16 / sz {
                    if !active(pred, e, sz) {
                        continue;
                    }
                    let (x, y) = (get(&n, e, sz), get(&m, e, sz));
                    let d = if u {
                        (x as i128 - y as i128).unsigned_abs()
                    } else {
                        (sext(x, bits) as i128 - sext(y, bits) as i128).unsigned_abs()
                    };
                    sum = sum.wrapping_add(d as u64);
                }
                self.wr(rda, sum as u32);
                return true;
            }
            let rdalo = ((h2 >> 13) & 7) << 1;
            let rdahi = (((h1 >> 4) & 3) << 1) | 1;
            if b0 {
                // VADDLV(A): 64-bit sum of the 32-bit elements
                if (h1 >> 1) & 7 != 4 {
                    return false;
                }
                let mut sum: i128 = if acc { sext((self.rr(rdalo) as u64) | ((self.rr(rdahi) as u64) << 32), 64) as i128 } else { 0 };
                for e in 0..4 {
                    if !active(pred, e, 4) {
                        continue;
                    }
                    sum += if u { get(&m, e, 4) as i128 } else { gets(&m, e, 4) as i128 };
                }
                let r = sum as u64;
                self.wr(rdalo, r as u32);
                self.wr(rdahi, (r >> 32) as u32);
                return true;
            }
            // VRMLALDAVH(A)(X): each 32-bit product is rounded and shifted right by 8 on its own
            // before the 64-bit accumulation (measured on the FVP)
            let mut sum: i128 = if acc { sext((self.rr(rdalo) as u64) | ((self.rr(rdahi) as u64) << 32), 64) as i128 } else { 0 };
            for e in 0..4 {
                if !active(pred, e, 4) {
                    continue;
                }
                let i = if t { e ^ 1 } else { e };
                let p = if u { get(&n, e, 4) as i128 * get(&m, i, 4) as i128 } else { gets(&n, e, 4) as i128 * gets(&m, i, 4) as i128 };
                sum += (p + 0x80) >> 8;
            }
            let r = sum as u64;
            self.wr(rdalo, r as u32);
            self.wr(rdahi, (r >> 32) as u32);
            return true;
        }
        let rdalo = ((h2 >> 13) & 7) << 1;
        let rdahi = (((h1 >> 4) & 3) << 1) | 1;
        let acc_val = || sext((self.rr(rdalo) as u64) | ((self.rr(rdahi) as u64) << 32), 64) as i128;
        if c0 && u {
            // VRMLSLDAVH(A)(X)
            if b0 {
                return false;
            }
            let mut sum: i128 = if acc { acc_val() } else { 0 };
            for p in 0..2 {
                let (e0, e1) = (2 * p, 2 * p + 1);
                let (a0, a1) = (gets(&n, e0, 4) as i128, gets(&n, e1, 4) as i128);
                let (b0v, b1v) = (gets(&m, e0, 4) as i128, gets(&m, e1, 4) as i128);
                let (p0, p1) = if t { (a0 * b1v, a1 * b0v) } else { (a0 * b0v, a1 * b1v) };
                if active(pred, e0, 4) {
                    sum += (p0 + 0x80) >> 8;
                }
                if active(pred, e1, 4) {
                    sum += (-p1 + 0x80) >> 8;
                }
            }
            let r = sum as u64;
            self.wr(rdalo, r as u32);
            self.wr(rdahi, (r >> 32) as u32);
            return true;
        }
        // VMLALDAV(A)(X) (c0 = 0) / VMLSLDAV(A)(X) (c0 = 1, signed only)
        if c0 && u {
            return false;
        }
        let sz = if b0 { 4usize } else { 2 };
        let bits = 8 * sz as u32;
        let mut sum: i128 = if acc { acc_val() } else { 0 };
        for p in 0..16 / sz / 2 {
            let (e0, e1) = (2 * p, 2 * p + 1);
            let val = |v: &V, e: usize| if u { get(v, e, sz) as i128 } else { sext(get(v, e, sz), bits) as i128 };
            let (p0, p1) = if t {
                (val(&n, e0) * val(&m, e1), val(&n, e1) * val(&m, e0))
            } else {
                (val(&n, e0) * val(&m, e0), val(&n, e1) * val(&m, e1))
            };
            if active(pred, e0, sz) {
                sum += p0;
            }
            if active(pred, e1, sz) {
                if c0 {
                    sum -= p1;
                } else {
                    sum += p1;
                }
            }
        }
        let r = sum as u64;
        self.wr(rdalo, r as u32);
        self.wr(rdahi, (r >> 32) as u32);
        true
    }

    /// hw1[7:6] = 11: VMLADAV family, VADDV, VMAXV/VMINV/VMAXAV/VMINAV, VMAXNMV family.
    #[allow(clippy::too_many_arguments)]
    fn mve_reduce(
        &mut self,
        h1: u32,
        h2: u32,
        u: bool,
        size: u32,
        qn: u32,
        b0: bool,
        _qd: u32,
        t: bool,
        bit7: bool,
        qm: u32,
        c0: bool,
        f: bool,
        pred: u16,
    ) -> bool {
        let m = self.q_get(qm);
        if size == 3 {
            if (h2 >> 6) & 1 != 0 || (h2 >> 4) & 1 != 0 || bit7 {
                return false;
            }
            let acc = (h2 >> 5) & 1 == 1;
            let rda = ((h2 >> 13) & 7) << 1;
            if f && b0 {
                // VADDV(A)
                if (h1 >> 1) & 1 != 0 {
                    return false;
                }
                let sz = 1usize << ((h1 >> 2) & 3);
                if sz == 8 {
                    return false;
                }
                let bits = 8 * sz as u32;
                let mut sum: i64 = if acc { self.rr(rda) as i32 as i64 } else { 0 };
                for e in 0..16 / sz {
                    if !active(pred, e, sz) {
                        continue;
                    }
                    sum = sum.wrapping_add(if u { get(&m, e, sz) as i64 } else { sext(get(&m, e, sz), bits) });
                }
                self.wr(rda, sum as u32);
                return true;
            }
            // VMLADAV(A)(X) / VMLSDAV(A)(X)
            let sz = match (b0, f) {
                (false, true) => 1usize,
                (false, false) => 2,
                _ => 4,
            };
            if c0 && u {
                return false;
            }
            let bits = 8 * sz as u32;
            let n = self.q_get(qn);
            let mut sum: i128 = if acc { self.rr(rda) as i32 as i128 } else { 0 };
            let val = |v: &V, e: usize| if u { get(v, e, sz) as i128 } else { sext(get(v, e, sz), bits) as i128 };
            for p in 0..16 / sz / 2 {
                let (e0, e1) = (2 * p, 2 * p + 1);
                let (p0, p1) = if t {
                    (val(&n, e0) * val(&m, e1), val(&n, e1) * val(&m, e0))
                } else {
                    (val(&n, e0) * val(&m, e0), val(&n, e1) * val(&m, e1))
                };
                if active(pred, e0, sz) {
                    sum += p0;
                }
                if active(pred, e1, sz) {
                    if c0 {
                        sum -= p1;
                    } else {
                        sum += p1;
                    }
                }
            }
            self.wr(rda, sum as u32);
            return true;
        }
        if size != 2 || !f || (h2 >> 4) & 7 != 0 || c0 || b0 {
            return false;
        }
        let rda = (h2 >> 12) & 0xF;
        if rda == 13 || rda == 15 {
            return false;
        }
        let min = bit7;
        let abs = (h1 >> 1) & 1 == 0;
        let esz = (h1 >> 2) & 3;
        if esz == 3 {
            // VMAXNMV/VMINNMV (abs = false), VMAXNMAV/VMINNMAV
            let fsz = if u { 2usize } else { 4 };
            let fp = fprec(fsz);
            let mut fpscr = self.mve_fpscr();
            let mut r = self.rr(rda) as u64 & bmask(8 * fsz as u32);
            if abs {
                r = fpu::abs(r, fp);
            }
            for e in 0..16 / fsz {
                if !active(pred, e, fsz) {
                    continue;
                }
                let x = get(&m, e, fsz);
                let x = if abs { fpu::abs(x, fp) } else { x };
                r = fpu::max_min_nm(r, x, fp, !min, &mut fpscr);
            }
            self.set_mve_fpscr(fpscr);
            self.wr(rda, r as u32);
            return true;
        }
        let sz = 1usize << esz;
        let bits = 8 * sz as u32;
        let cur = self.rr(rda) as u64 & bmask(bits);
        if abs {
            // VMAXAV/VMINAV: unsigned Rda against |Qm| (signed elements)
            let mut r = cur;
            for e in 0..16 / sz {
                if !active(pred, e, sz) {
                    continue;
                }
                let x = sext(get(&m, e, sz), bits).unsigned_abs() & bmask(bits);
                if (x > r) != min {
                    r = x;
                }
            }
            self.wr(rda, r as u32);
        } else {
            let mut r = if u { cur as i64 } else { sext(cur, bits) };
            for e in 0..16 / sz {
                if !active(pred, e, sz) {
                    continue;
                }
                let x = if u { get(&m, e, sz) as i64 } else { sext(get(&m, e, sz), bits) };
                if (x > r) != min {
                    r = x;
                }
            }
            self.wr(rda, r as u32);
        }
        true
    }

    /// `Qd = (Qd +) Qn * Qm` on complex pairs (real, imaginary) with the rotation
    /// `rot` × 90° applied to Qn (`FPMulAdd` per lane, as VCMLA/VCMUL).
    fn complex_mul(a: &V, b: &V, acc: Option<&V>, fsz: usize, rot: u32, pred: u16, fpscr: &mut u32) -> V {
        let fp = fprec(fsz);
        let mut v = [0u8; 16];
        for p in 0..16 / fsz / 2 {
            let (re, im) = (2 * p, 2 * p + 1);
            if !active(pred, re, fsz) && !active(pred, im, fsz) {
                continue;
            }
            let (nr, ni, mr, mi) = (get(a, re, fsz), get(a, im, fsz), get(b, re, fsz), get(b, im, fsz));
            let (r1, r2, i1, i2) = match rot {
                0 => (nr, mr, nr, mi),
                1 => (fpu::neg(ni, fp), mi, ni, mr),
                2 => (fpu::neg(nr, fp), mr, fpu::neg(nr, fp), mi),
                _ => (ni, mi, fpu::neg(ni, fp), mr),
            };
            match acc {
                Some(d) => {
                    put(&mut v, re, fsz, fpu::mul_add(get(d, re, fsz), r1, r2, fp, fpscr));
                    put(&mut v, im, fsz, fpu::mul_add(get(d, im, fsz), i1, i2, fp, fpscr));
                }
                None => {
                    put(&mut v, re, fsz, fpu::mul(r1, r2, fp, fpscr));
                    put(&mut v, im, fsz, fpu::mul(i1, i2, fp, fpscr));
                }
            }
        }
        v
    }

    /// 0xFC/0xFD with coprocessor 8: VCADD.F (rotation 90/270) and VCMLA.F.
    fn mve_complex(&mut self, h1: u32, h2: u32) -> bool {
        // 1111 110 r 1 D 0 S Vn | Vd 1000 N 1 M 0 Vm  VCADD (r: 0 = 90°, 1 = 270°)
        // 1111 110 rot  D 1 S Vn | Vd 1000 N 1 M 0 Vm  VCMLA (rot × 90°)
        if (h2 >> 8) & 0xF != 0x8 || (h2 >> 6) & 1 != 1 || (h2 >> 4) & 1 != 0 || (h1 >> 9) & 1 != 0 {
            return false;
        }
        let fsz = if (h1 >> 4) & 1 == 1 { 4usize } else { 2 };
        let fp = fprec(fsz);
        let qd = (h2 >> 13) & 7;
        let qn = (h1 >> 1) & 7;
        let qm = (h2 >> 1) & 7;
        let pred = self.mve_pred();
        let a = self.q_get(qn);
        let b = self.q_get(qm);
        let mut fpscr = self.mve_fpscr();
        let v = if (h1 >> 5) & 1 == 1 {
            let rot = (h1 >> 7) & 3;
            let d = self.q_get(qd);
            Self::complex_mul(&a, &b, Some(&d), fsz, rot, pred, &mut fpscr)
        } else {
            if (h1 >> 7) & 1 != 1 {
                return false;
            }
            let rot270 = (h1 >> 8) & 1 == 1;
            let mut v = [0u8; 16];
            for p in 0..16 / fsz / 2 {
                let (re, im) = (2 * p, 2 * p + 1);
                if !active(pred, re, fsz) && !active(pred, im, fsz) {
                    continue;
                }
                let (nr, ni, mr, mi) = (get(&a, re, fsz), get(&a, im, fsz), get(&b, re, fsz), get(&b, im, fsz));
                let (r, i) = if rot270 {
                    (fpu::add(nr, mi, fp, &mut fpscr), fpu::sub(ni, mr, fp, &mut fpscr))
                } else {
                    (fpu::sub(nr, mi, fp, &mut fpscr), fpu::add(ni, mr, fp, &mut fpscr))
                };
                put(&mut v, re, fsz, r);
                put(&mut v, im, fsz, i);
            }
            v
        };
        self.set_mve_fpscr(fpscr);
        self.q_put(qd, v, pred);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{BusError, IrqLines};
    use cs320_core::Ticks;

    /// 64 KB of RAM at 0x1100_0000 (code) and 0x3000_0000 (data).
    struct Ram {
        code: Vec<u8>,
        data: Vec<u8>,
    }

    impl Ram {
        fn slot(&mut self, addr: u32) -> Option<(&mut Vec<u8>, usize)> {
            match addr >> 24 {
                0x11 => Some((&mut self.code, (addr & 0xFFFF) as usize)),
                0x30 => Some((&mut self.data, (addr & 0xFFFF) as usize)),
                _ => None,
            }
        }
    }

    impl Bus for Ram {
        fn read(&mut self, _now: Ticks, addr: u32, size: u8, _ns: bool, _debug: bool) -> Result<u32, BusError> {
            let (m, o) = self.slot(addr).ok_or(BusError)?;
            let mut v = 0;
            for i in 0..size as usize {
                v |= (m[o + i] as u32) << (8 * i);
            }
            Ok(v)
        }
        fn write(&mut self, _now: Ticks, addr: u32, size: u8, value: u32, _ns: bool, _debug: bool) -> Result<(), BusError> {
            let (m, o) = self.slot(addr).ok_or(BusError)?;
            for i in 0..size as usize {
                m[o + i] = (value >> (8 * i)) as u8;
            }
            Ok(())
        }
        fn fetch16(&mut self, addr: u32, _ns: bool) -> Result<u16, BusError> {
            let (m, o) = self.slot(addr).ok_or(BusError)?;
            Ok(u16::from_le_bytes([m[o], m[o + 1]]))
        }
        fn deadline(&self) -> Ticks {
            Ticks::MAX
        }
        fn take_irq_lines(&mut self) -> Option<IrqLines> {
            None
        }
        fn reset_vector_base(&self) -> u32 {
            0x1100_0000
        }
    }

    /// Runs `code` (halfwords) from the reset vector with q0 = a, q1 = b, q2 = o, r4/r5 = s;
    /// returns (q2, r4, r5, fpscr, vpr) after a final BKPT.
    fn run(code: &[u16], a: [u32; 4], b: [u32; 4], o: [u32; 4], s: [u32; 2], fpscr: u32) -> ([u32; 4], u32, u32, u32, u32) {
        let mut ram = Ram { code: vec![0; 0x10000], data: vec![0; 0x10000] };
        // vector table: SP, reset
        ram.code[0..4].copy_from_slice(&0x3000_8000u32.to_le_bytes());
        ram.code[4..8].copy_from_slice(&0x1100_0101u32.to_le_bytes());
        let mut pc = 0x100;
        for h in code {
            ram.code[pc..pc + 2].copy_from_slice(&h.to_le_bytes());
            pc += 2;
        }
        ram.code[pc..pc + 2].copy_from_slice(&0xBE00u16.to_le_bytes()); // bkpt
        let mut cpu = Cpu::new();
        cpu.power_on_reset(&mut ram);
        cpu.cpacr[0] = 0xF << 20;
        for (i, v) in a.iter().enumerate() {
            cpu.s[i] = *v;
        }
        for (i, v) in b.iter().enumerate() {
            cpu.s[4 + i] = *v;
        }
        for (i, v) in o.iter().enumerate() {
            cpu.s[8 + i] = *v;
        }
        cpu.r[4] = s[0];
        cpu.r[5] = s[1];
        cpu.fpscr = fpscr;
        cpu.vpr = 0;
        let end = 0x1100_0000 + pc as u32;
        for _ in 0..1000 {
            if cpu.pc == end || cpu.lockup {
                break;
            }
            cpu.step_instruction(&mut ram);
            if std::env::var("MVE_TEST_TRACE").is_ok() {
                eprintln!("after {:#x}: fpscr {:#x} vpr {:#x} q2 {:08x?}", cpu.pc, cpu.fpscr, cpu.vpr, &cpu.s[8..12]);
            }
        }
        assert_eq!(cpu.pc, end, "did not reach the end of the code (lockup {})", cpu.lockup);
        ([cpu.s[8], cpu.s[9], cpu.s[10], cpu.s[11]], cpu.r[4], cpu.r[5], cpu.fpscr, cpu.vpr)
    }

    #[test]
    fn vcmp_vpst_vaddt_flags() {
        // vcmp.f32 ne, q0, q1; vpst; vaddt.f32 q2, q0, q1; vcmp.f32 ge, q0, q1; vpst; vsubt.f32 q2, q2, q1
        let code = [0xEE31u16, 0x0F82, 0xFE71, 0x0F4D, 0xEF00, 0x4D42, 0xEE31, 0x1F02, 0xFE71, 0x0F4D, 0xEF24, 0x4D42];
        let a = [0xFF7F_FFFF; 4];
        let b = [0x807F_FFFF, 0x0080_0000, 0x7F7F_FFFF, 0xFF7F_FFFF];
        let o = [0x5BC8_4753, 0x3763_D682, 0xFBC5_BB3B, 0xD8FD_502A];
        let (q2, _, _, st, vp) = run(&code, a, b, o, [0; 2], 0x0004_0099);
        assert_eq!(q2, [0xFF7F_FFFF, 0xFF7F_FFFF, 0, 0x7F7F_FFFF]);
        assert_eq!(vp, 0xF000);
        assert_eq!(st, 0x0004_0099);
    }

    #[test]
    fn vcmul_vcmla() {
        // vcmul.f32 q2, q0, q1, #90; vcmla.f32 q2, q0, q1, #180
        let code = [0xFE30u16, 0x4E03, 0xFD30, 0x4842];
        let b = [0xFFC0_0123, 1, 0x807F_FFFF, 0x0080_0000];
        let (q2, _, _, _, _) = run(&code, [0; 4], b, [0x1111_1111; 4], [0; 2], 0x0004_0000);
        assert_eq!(q2, [0x7FC0_0000, 0x7FC0_0000, 0, 0x8000_0000]);
    }

    #[test]
    fn shifts_by_scalar() {
        // vmov q2, q0; vshl.s16 q2, r4; vqshl.u32 q1, r5; veor q2, q2, q1; vmov q1, q0; vrshl.s8 q1, r4; veor q2, q2, q1
        let code = [0xEF20u16, 0x4150, 0xEE35, 0x5E64, 0xFE39, 0x3EE5, 0xFF04, 0x4152, 0xEF20, 0x2150, 0xEE33, 0x3E64, 0xFF04, 0x4152];
        let a = [0xC005_F9B8, 0x73CA_2764, 0xA964_156C, 0xA07C_41C4];
        let b = [0x996A_0201, 0xD3F1_2217, 0x52F9_9188, 0x20DE_6926];
        let (q2, _, _, _, _) = run(&code, a, b, [0; 4], [0x80, 0xB034_A7A6], 0x0004_0000);
        assert_eq!(q2, [0xFFFF_FFFF, 0, 0xFFFF_0000, 0xFFFF_0000]);
    }

    #[test]
    fn increment_dup() {
        // vidup.u32 q2, r4, #4; vddup.u8 q1, r4, #1; veor q2, q2, q1; vdwdup.u16 q1, r4, r5, #2; veor; viwdup.u8 q1, r4, r5, #8; veor
        let code = [0xEE25u16, 0x4FEE, 0xEE05, 0x3F6E, 0xFF04, 0x4152, 0xEE15, 0x3F65, 0xFF04, 0x4152, 0xEE05, 0x2FE5, 0xFF04, 0x4152];
        let (q2, r4, _, _, _) = run(&code, [0; 4], [0; 4], [0; 4], [0x8000_002B, 0x51], 0x0004_0000);
        assert_eq!(q2, [0x8B3B_1920, 0xE75B_7504, 0xC37B_5178, 0x3FBB_AD7C]);
        assert_eq!(r4, 0x8000_009B);
    }

    #[test]
    fn vcvt_s16_f64_fixed() {
        // vmov.f64 d4, d0; vcvt.s16.f64 d4, d4, #7 with d0 = -1.0 (d4 = q2[1:0])
        let code = [0xEEB0u16, 0x4B40, 0xEEBE, 0x4B64];
        let (q2, _, _, st, _) = run(&code, [0, 0xBFF0_0000, 0, 0], [0; 4], [0; 4], [0; 2], 0x0004_0000);
        eprintln!("d2 = {:08x} {:08x} fpscr {st:#x}", q2[1], q2[0]);
        assert_eq!(q2[0], 0xFFFF_FF80);
    }

    #[test]
    fn vadd_f16_keeps_denormals() {
        // vadd.f16 q2, q0, q1
        let b = [0x7F80_0001u32, 0, 0, 0];
        let (q2, _, _, st, _) = run(&[0xEF10, 0x4D42], [0; 4], b, [0; 4], [0; 2], 0x0004_0000);
        assert_eq!(q2[0], 0x7E00_0001, "fpscr {st:#x}");
    }
}
