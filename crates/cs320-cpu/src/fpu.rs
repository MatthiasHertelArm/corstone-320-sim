//! FPv5 floating-point arithmetic in half, single and double precision with the Arm rules for
//! rounding, flush-to-zero, default NaN, NaN propagation and cumulative exception flags
//! (DDI 0553B chapter A5 "Floating-point support").
//!
//! Half and single precision results are computed in `f64`, which is exact or carries an exact
//! residual sign, and are then rounded once to the target format by [`round_fmt`]. Double
//! precision results are the host's correctly rounded (to nearest) `f64` results with the sign
//! of the exact residual from a fused multiply-add, which makes the directed rounding modes and
//! the inexact flag exact as well.

use crate::decode::Prec;

pub const FPSCR_N: u32 = 1 << 31;
pub const FPSCR_Z: u32 = 1 << 30;
pub const FPSCR_C: u32 = 1 << 29;
pub const FPSCR_V: u32 = 1 << 28;
pub const FPSCR_QC: u32 = 1 << 27;
pub const FPSCR_AHP: u32 = 1 << 26;
pub const FPSCR_DN: u32 = 1 << 25;
pub const FPSCR_FZ: u32 = 1 << 24;
pub const FPSCR_FZ16: u32 = 1 << 19;
pub const FPSCR_IDC: u32 = 1 << 7;
pub const FPSCR_IXC: u32 = 1 << 4;
pub const FPSCR_UFC: u32 = 1 << 3;
pub const FPSCR_OFC: u32 = 1 << 2;
pub const FPSCR_DZC: u32 = 1 << 1;
pub const FPSCR_IOC: u32 = 1 << 0;
/// Writable bits of FPSCR on the Cortex-M85 (NZCV, QC, AHP, DN, FZ, RMode, FZ16, LTPSIZE,
/// the cumulative flags).
/// Writable FPSCR bits: flags, AHP/DN/FZ/RMode, FZ16, LTPSIZE (Armv8.1-M), cumulative exceptions.
pub const FPSCR_MASK: u32 = 0xFFCF_009F;
/// Reset value: LTPSIZE = 0b100 (no tail-predicated loop active).
pub const FPSCR_RESET: u32 = 0x0004_0000;

pub const DEFAULT_NAN32: u32 = 0x7FC0_0000;
pub const DEFAULT_NAN64: u64 = 0x7FF8_0000_0000_0000;
pub const DEFAULT_NAN16: u32 = 0x7E00;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rounding {
    Nearest,
    PlusInf,
    MinusInf,
    Zero,
    /// Ties away from zero (VRINTA, VCVTA).
    Away,
}

pub fn rounding_mode(fpscr: u32) -> Rounding {
    match (fpscr >> 22) & 3 {
        0 => Rounding::Nearest,
        1 => Rounding::PlusInf,
        2 => Rounding::MinusInf,
        _ => Rounding::Zero,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Zero,
    Normal,
    Inf,
    QNaN,
    SNaN,
}

#[derive(Clone, Copy, Debug)]
struct Unpacked {
    kind: Kind,
    neg: bool,
    /// Exact value for zero and finite numbers (input denormals flushed if FZ).
    value: f64,
}

/// Bit layout of a format.
#[derive(Clone, Copy)]
struct Format {
    frac_bits: i32,
    exp_bits: i32,
}

const HALF: Format = Format { frac_bits: 10, exp_bits: 5 };
const SINGLE: Format = Format { frac_bits: 23, exp_bits: 8 };
const DOUBLE: Format = Format { frac_bits: 52, exp_bits: 11 };

fn format(p: Prec) -> Format {
    match p {
        Prec::H => HALF,
        Prec::S => SINGLE,
        Prec::D => DOUBLE,
    }
}

fn width(p: Prec) -> u32 {
    match p {
        Prec::H => 16,
        Prec::S => 32,
        Prec::D => 64,
    }
}

fn mask(p: Prec) -> u64 {
    match p {
        Prec::H => 0xFFFF,
        Prec::S => 0xFFFF_FFFF,
        Prec::D => u64::MAX,
    }
}

fn sign_bit(p: Prec) -> u64 {
    1u64 << (width(p) - 1)
}

fn quiet_bit(p: Prec) -> u64 {
    1u64 << (format(p).frac_bits - 1)
}

pub fn default_nan(p: Prec) -> u64 {
    match p {
        Prec::H => DEFAULT_NAN16 as u64,
        Prec::S => DEFAULT_NAN32 as u64,
        Prec::D => DEFAULT_NAN64,
    }
}

fn zero(neg: bool, p: Prec) -> u64 {
    if neg {
        sign_bit(p)
    } else {
        0
    }
}

fn inf(neg: bool, p: Prec) -> u64 {
    let f = format(p);
    zero(neg, p) | (((1u64 << f.exp_bits) - 1) << f.frac_bits)
}

fn fz_applies(p: Prec, fpscr: u32) -> bool {
    match p {
        Prec::H => fpscr & FPSCR_FZ16 != 0,
        _ => fpscr & FPSCR_FZ != 0,
    }
}

fn unpack(x: u64, p: Prec, fpscr: &mut u32) -> Unpacked {
    let f = format(p);
    let x = x & mask(p);
    let neg = x & sign_bit(p) != 0;
    let exp = ((x >> f.frac_bits) & ((1 << f.exp_bits) - 1)) as i32;
    let frac = x & ((1u64 << f.frac_bits) - 1);
    let max_exp = (1 << f.exp_bits) - 1;
    if exp == 0 {
        if frac == 0 || fz_applies(p, *fpscr) {
            // flushing a half-precision denormal under FZ16 does not signal Input Denormal
            if frac != 0 && p != Prec::H {
                *fpscr |= FPSCR_IDC;
            }
            return Unpacked { kind: Kind::Zero, neg, value: if neg { -0.0 } else { 0.0 } };
        }
        // `powi` of the minimum double exponent underflows: take the host's own value
        let v = match p {
            Prec::D => f64::from_bits(frac),
            Prec::S => f32::from_bits(frac as u32) as f64,
            Prec::H => frac as f64 * 2f64.powi(-24),
        };
        return Unpacked { kind: Kind::Normal, neg, value: if neg { -v } else { v } };
    }
    if exp == max_exp {
        if frac == 0 {
            return Unpacked { kind: Kind::Inf, neg, value: if neg { f64::NEG_INFINITY } else { f64::INFINITY } };
        }
        let kind = if frac & quiet_bit(p) != 0 { Kind::QNaN } else { Kind::SNaN };
        return Unpacked { kind, neg, value: f64::NAN };
    }
    let v = match p {
        Prec::D => f64::from_bits(x),
        Prec::S => f32::from_bits(x as u32) as f64,
        Prec::H => {
            let bias = 15;
            let v = (1024 + frac) as f64 * 2f64.powi(exp - bias - 10);
            if neg {
                -v
            } else {
                v
            }
        }
    };
    Unpacked { kind: Kind::Normal, neg, value: v }
}

fn process_nan(kind: Kind, x: u64, p: Prec, fpscr: &mut u32) -> u64 {
    if kind == Kind::SNaN {
        *fpscr |= FPSCR_IOC;
    }
    if *fpscr & FPSCR_DN != 0 {
        default_nan(p)
    } else {
        (x | quiet_bit(p)) & mask(p)
    }
}

fn process_nans(a: (Unpacked, u64), b: (Unpacked, u64), p: Prec, fpscr: &mut u32) -> Option<u64> {
    if a.0.kind == Kind::SNaN {
        Some(process_nan(Kind::SNaN, a.1, p, fpscr))
    } else if b.0.kind == Kind::SNaN {
        Some(process_nan(Kind::SNaN, b.1, p, fpscr))
    } else if a.0.kind == Kind::QNaN {
        Some(process_nan(Kind::QNaN, a.1, p, fpscr))
    } else if b.0.kind == Kind::QNaN {
        Some(process_nan(Kind::QNaN, b.1, p, fpscr))
    } else {
        None
    }
}

fn process_nans3(a: (Unpacked, u64), b: (Unpacked, u64), c: (Unpacked, u64), p: Prec, fpscr: &mut u32) -> Option<u64> {
    for x in [a, b, c] {
        if x.0.kind == Kind::SNaN {
            return Some(process_nan(Kind::SNaN, x.1, p, fpscr));
        }
    }
    for x in [a, b, c] {
        if x.0.kind == Kind::QNaN {
            return Some(process_nan(Kind::QNaN, x.1, p, fpscr));
        }
    }
    None
}

fn invalid(p: Prec, fpscr: &mut u32) -> u64 {
    *fpscr |= FPSCR_IOC;
    default_nan(p)
}

/// `FPRound` of the exact value `v + residual * tiny` (residual: sign of the exact error
/// relative to `v`, i.e. -1, 0 or +1) to a half or single format; `v` must be finite and
/// nonzero. With `ahp`, the half-precision alternative format has no infinities.
/// Mantissa and exponent of a finite nonzero double: |x| = m × 2^e exactly.
fn mant_exp(x: f64) -> (u128, i32) {
    let bits = x.abs().to_bits();
    let exp_field = (bits >> 52) as i32;
    let frac = bits & ((1 << 52) - 1);
    if exp_field == 0 {
        (frac as u128, -1074)
    } else {
        ((frac | (1 << 52)) as u128, exp_field - 1075)
    }
}

fn round_fmt(v: f64, residual: i32, fmt: Format, fpscr: &mut u32, ahp: bool, fz: bool) -> u32 {
    let (m, e) = mant_exp(v);
    round_mant(v < 0.0, m, e, residual, fmt, fpscr, ahp, fz) as u32
}

/// Rounds the exact value ±(m × 2^e), nudged by a residual of the given sign that is smaller
/// than one unit of `m` (in the signed-value sense), to the format with the FPSCR rounding mode,
/// setting the flags. `m` must be nonzero and below 2^122.
#[allow(clippy::too_many_arguments)]
fn round_mant(neg: bool, m: u128, e: i32, residual: i32, fmt: Format, fpscr: &mut u32, ahp: bool, fz: bool) -> u64 {
    let r = if neg { -residual } else { residual };
    let m2: u128 = (m << 2).wrapping_add(r as i128 as u128);
    let e2 = e - 2;
    let msb = 127 - m2.leading_zeros() as i32;
    let big_e = msb + e2;
    let bias = (1 << (fmt.exp_bits - 1)) - 1;
    let emin = 1 - bias;
    let sign_bit = (neg as u64) << (fmt.frac_bits + fmt.exp_bits);
    if fz && big_e < emin {
        *fpscr |= FPSCR_UFC;
        return sign_bit;
    }
    let lsb_exp = (big_e - fmt.frac_bits).max(emin - fmt.frac_bits);
    let sh = lsb_exp - e2;
    let (mut q, rem, half) = if sh <= 0 {
        (m2 << (-sh) as u32, 0u128, 1u128)
    } else if sh >= 127 {
        (0, m2, u128::MAX)
    } else {
        (m2 >> sh, m2 & ((1u128 << sh) - 1), 1u128 << (sh - 1))
    };
    let inexact = rem != 0;
    let inc = match rounding_mode(*fpscr) {
        Rounding::Nearest => rem > half || (rem == half && q & 1 == 1),
        Rounding::PlusInf => !neg && inexact,
        Rounding::MinusInf => neg && inexact,
        Rounding::Zero => false,
        Rounding::Away => rem >= half,
    };
    if inc {
        q += 1;
    }
    let mut lsb = lsb_exp;
    let normal_min = 1u128 << fmt.frac_bits;
    while q >= normal_min << 1 {
        q >>= 1;
        lsb += 1;
    }
    let tiny = big_e < emin;
    if tiny && inexact {
        *fpscr |= FPSCR_UFC;
    }
    let max_exp_field = (1 << fmt.exp_bits) - 1;
    let (biased, frac) = if q >= normal_min { (lsb + fmt.frac_bits + bias, (q - normal_min) as u64) } else { (0, q as u64) };
    let max_normal = (((max_exp_field - 1) as u64) << fmt.frac_bits) | ((1u64 << fmt.frac_bits) - 1);
    if ahp {
        if biased > max_exp_field {
            *fpscr |= FPSCR_IOC;
            return sign_bit | ((max_exp_field as u64) << fmt.frac_bits) | ((1u64 << fmt.frac_bits) - 1);
        }
    } else if biased >= max_exp_field {
        *fpscr |= FPSCR_OFC | FPSCR_IXC;
        let to_inf = match rounding_mode(*fpscr) {
            Rounding::Nearest | Rounding::Away => true,
            Rounding::PlusInf => !neg,
            Rounding::MinusInf => neg,
            Rounding::Zero => false,
        };
        return sign_bit | if to_inf { (max_exp_field as u64) << fmt.frac_bits } else { max_normal };
    }
    if inexact {
        *fpscr |= FPSCR_IXC;
    }
    sign_bit | ((biased as u64) << fmt.frac_bits) | frac
}

/// Rounds an exact double result ±(m × 2^e) with residual sign (see [`round_mant`]).
fn round_d(neg: bool, m: u128, e: i32, residual: i32, fpscr: &mut u32) -> u64 {
    if m == 0 && residual == 0 {
        return zero(neg, Prec::D);
    }
    if m == 0 {
        // only a residual: a value below one unit of the (zero) mantissa
        *fpscr |= FPSCR_UFC | FPSCR_IXC;
        return directed_tiny(residual < 0, fpscr);
    }
    round_mant(neg, m, e, residual, DOUBLE, fpscr, false, *fpscr & FPSCR_FZ != 0)
}

/// Exact sum of two nonzero values ±(mp × 2^ep) and ±(mc × 2^ec), rounded once to double.
fn fma_exact(sp: bool, mp: u128, ep: i32, sc: bool, mc: u128, ec: i32, fpscr: &mut u32) -> u64 {
    let len = |m: u128| 128 - m.leading_zeros() as i32;
    // A: the operand whose most significant bit is higher; B: the other one
    let (sa, ma, ea, sb, mb, eb) = if ep + len(mp) >= ec + len(mc) { (sp, mp, ep, sc, mc, ec) } else { (sc, mc, ec, sp, mp, ep) };
    // common exponent: A's MSB at bit 118
    let e = ea + len(ma) - 119;
    let a2 = ma << (ea - e) as u32;
    let shift = eb - e;
    let (b2, lost) = if shift >= 0 {
        if shift + len(mb) > 120 {
            // cannot happen: B's MSB is at or below A's
            (0u128, true)
        } else {
            (mb << shift as u32, false)
        }
    } else {
        let sh = (-shift) as u32;
        if sh >= 127 {
            (0, true)
        } else {
            (mb >> sh, mb & ((1u128 << sh) - 1) != 0)
        }
    };
    let va = if sa { -(a2 as i128) } else { a2 as i128 };
    let vb = if sb { -(b2 as i128) } else { b2 as i128 };
    let v = va + vb;
    // the bits lost from B make the true value larger in the direction of B's sign
    let residual = if lost {
        if sb {
            -1
        } else {
            1
        }
    } else {
        0
    };
    if v == 0 && !lost {
        return zero(rounding_mode(*fpscr) == Rounding::MinusInf, Prec::D);
    }
    if v == 0 {
        return round_d(residual < 0, 0, e, residual, fpscr);
    }
    let neg = v < 0;
    round_d(neg, v.unsigned_abs(), e, residual, fpscr)
}

/// Integer square root of a u128.
fn isqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let mut x = 1u128 << ((128 - n.leading_zeros()).div_ceil(2));
    loop {
        let y = (x + n / x) / 2;
        if y >= x {
            break;
        }
        x = y;
    }
    while x * x > n {
        x -= 1;
    }
    while (x + 1) * (x + 1) <= n {
        x += 1;
    }
    x
}

/// Rounds the host's round-to-nearest double `s` with the sign of the exact residual to the
/// FPSCR rounding mode, setting the flags. `s` must be finite or the overflowed result of a
/// finite exact value.
fn round_f64(s: f64, residual: i32, fpscr: &mut u32) -> u64 {
    let mode = rounding_mode(*fpscr);
    let neg = s.is_sign_negative();
    let max_finite = f64::MAX.to_bits();
    if s.is_infinite() {
        *fpscr |= FPSCR_OFC | FPSCR_IXC;
        let to_inf = match mode {
            Rounding::Nearest | Rounding::Away => true,
            Rounding::PlusInf => !neg,
            Rounding::MinusInf => neg,
            Rounding::Zero => false,
        };
        return if to_inf { s.to_bits() } else { max_finite | ((neg as u64) << 63) };
    }
    let mut r = s;
    if residual != 0 {
        let toward_zero = (residual > 0) == neg; // the exact value is closer to zero
        let adjust = match mode {
            Rounding::Nearest | Rounding::Away => false,
            Rounding::PlusInf => residual > 0,
            Rounding::MinusInf => residual < 0,
            Rounding::Zero => toward_zero,
        };
        if adjust {
            // one ulp in the direction of the residual
            let bits = r.to_bits();
            let up = residual > 0;
            r = if r == 0.0 {
                if up {
                    f64::from_bits(1)
                } else {
                    -f64::from_bits(1)
                }
            } else if up != neg {
                f64::from_bits(bits + 1)
            } else {
                f64::from_bits(bits - 1)
            };
            if r.is_infinite() {
                *fpscr |= FPSCR_OFC | FPSCR_IXC;
                return r.to_bits();
            }
        }
        *fpscr |= FPSCR_IXC;
    }
    let tiny = r != 0.0 && r.abs() < f64::MIN_POSITIVE;
    if tiny {
        if *fpscr & FPSCR_FZ != 0 {
            *fpscr |= FPSCR_UFC;
            return zero(neg, Prec::D);
        }
        if residual != 0 {
            *fpscr |= FPSCR_UFC;
        }
    }
    // an exact result that is the maximum magnitude cannot have overflowed
    r.to_bits()
}

/// Rounds a finite nonzero exact value (`v` plus residual sign) to the format.
fn round_to(v: f64, residual: i32, p: Prec, fpscr: &mut u32) -> u64 {
    match p {
        Prec::D => round_f64(v, residual, fpscr),
        Prec::S => round_fmt(v, residual, SINGLE, fpscr, false, *fpscr & FPSCR_FZ != 0) as u64,
        Prec::H => round_fmt(v, residual, HALF, fpscr, false, *fpscr & FPSCR_FZ16 != 0) as u64,
    }
}

fn sign_of(x: f64) -> i32 {
    if x > 0.0 {
        1
    } else if x < 0.0 {
        -1
    } else {
        0
    }
}

/// Rounds an exact sum `s + err` (from TwoSum), handling an exact zero.
fn round_sum(s: f64, err: f64, p: Prec, fpscr: &mut u32) -> u64 {
    if s == 0.0 && err == 0.0 {
        return zero(rounding_mode(*fpscr) == Rounding::MinusInf, p);
    }
    if s == 0.0 {
        // cancellation to a value that is only in the error term (not for H/S inputs)
        return round_to(err, 0, p, fpscr);
    }
    round_to(s, sign_of(err), p, fpscr)
}

fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let s = a + b;
    if s.is_infinite() {
        return (s, 0.0);
    }
    let bb = s - a;
    let err = (a - (s - bb)) + (b - bb);
    (s, err)
}

pub fn neg(a: u64, p: Prec) -> u64 {
    (a ^ sign_bit(p)) & mask(p)
}

pub fn abs(a: u64, p: Prec) -> u64 {
    a & !sign_bit(p) & mask(p)
}

pub fn add(a: u64, b: u64, p: Prec, fpscr: &mut u32) -> u64 {
    let ua = unpack(a, p, fpscr);
    let ub = unpack(b, p, fpscr);
    if let Some(n) = process_nans((ua, a), (ub, b), p, fpscr) {
        return n;
    }
    add_unpacked(ua, ub, p, fpscr)
}

pub fn sub(a: u64, b: u64, p: Prec, fpscr: &mut u32) -> u64 {
    let ua = unpack(a, p, fpscr);
    let ub = unpack(b, p, fpscr);
    if let Some(n) = process_nans((ua, a), (ub, b), p, fpscr) {
        return n;
    }
    add_unpacked(ua, Unpacked { neg: !ub.neg, value: -ub.value, ..ub }, p, fpscr)
}

fn add_unpacked(ua: Unpacked, ub: Unpacked, p: Prec, fpscr: &mut u32) -> u64 {
    match (ua.kind, ub.kind) {
        (Kind::Inf, Kind::Inf) if ua.neg != ub.neg => invalid(p, fpscr),
        (Kind::Inf, _) => inf(ua.neg, p),
        (_, Kind::Inf) => inf(ub.neg, p),
        (Kind::Zero, Kind::Zero) if ua.neg == ub.neg => zero(ua.neg, p),
        _ => {
            let (s, err) = two_sum(ua.value, ub.value);
            round_sum(s, err, p, fpscr)
        }
    }
}

pub fn mul(a: u64, b: u64, p: Prec, fpscr: &mut u32) -> u64 {
    let ua = unpack(a, p, fpscr);
    let ub = unpack(b, p, fpscr);
    if let Some(n) = process_nans((ua, a), (ub, b), p, fpscr) {
        return n;
    }
    let sign = ua.neg != ub.neg;
    match (ua.kind, ub.kind) {
        (Kind::Inf, Kind::Zero) | (Kind::Zero, Kind::Inf) => invalid(p, fpscr),
        (Kind::Inf, _) | (_, Kind::Inf) => inf(sign, p),
        (Kind::Zero, _) | (_, Kind::Zero) => zero(sign, p),
        _ => {
            if p == Prec::D {
                let (ma, ea) = mant_exp(ua.value);
                let (mb, eb) = mant_exp(ub.value);
                return round_d(sign, ma * mb, ea + eb, 0, fpscr);
            }
            let prod = ua.value * ub.value;
            round_to(prod, 0, p, fpscr)
        }
    }
}

/// A nonzero exact value below the smallest denormal: zero, or one denormal ulp away from zero
/// in the directed modes.
fn directed_tiny(neg: bool, fpscr: &mut u32) -> u64 {
    if *fpscr & FPSCR_FZ != 0 {
        return zero(neg, Prec::D);
    }
    match rounding_mode(*fpscr) {
        Rounding::PlusInf if !neg => 1,
        Rounding::MinusInf if neg => (1u64 << 63) | 1,
        _ => zero(neg, Prec::D),
    }
}

pub fn div(a: u64, b: u64, p: Prec, fpscr: &mut u32) -> u64 {
    let ua = unpack(a, p, fpscr);
    let ub = unpack(b, p, fpscr);
    if let Some(n) = process_nans((ua, a), (ub, b), p, fpscr) {
        return n;
    }
    let sign = ua.neg != ub.neg;
    match (ua.kind, ub.kind) {
        (Kind::Inf, Kind::Inf) | (Kind::Zero, Kind::Zero) => invalid(p, fpscr),
        (Kind::Inf, _) => inf(sign, p),
        (_, Kind::Zero) => {
            *fpscr |= FPSCR_DZC;
            inf(sign, p)
        }
        (Kind::Zero, _) | (_, Kind::Inf) => zero(sign, p),
        _ => {
            if p == Prec::D {
                let (ma, ea) = mant_exp(ua.value);
                let (mb, eb) = mant_exp(ub.value);
                // at least 71 quotient bits whatever the (possibly denormal) mantissas
                let k = 124 - (128 - ma.leading_zeros() as i32);
                let num = ma << k;
                let q = num / mb;
                let sticky = i32::from(!num.is_multiple_of(mb));
                let residual = if sign { -sticky } else { sticky };
                return round_d(sign, q, ea - eb - k, residual, fpscr);
            }
            let q = ua.value / ub.value;
            let rem = (-q).mul_add(ub.value, ua.value);
            round_to(q, sign_of(rem) * sign_of(ub.value), p, fpscr)
        }
    }
}

pub fn sqrt(a: u64, p: Prec, fpscr: &mut u32) -> u64 {
    let ua = unpack(a, p, fpscr);
    match ua.kind {
        Kind::QNaN | Kind::SNaN => process_nan(ua.kind, a, p, fpscr),
        Kind::Zero => zero(ua.neg, p),
        Kind::Inf if !ua.neg => inf(false, p),
        _ if ua.neg => invalid(p, fpscr),
        _ => {
            if p == Prec::D {
                let (m, e) = mant_exp(ua.value);
                // at least 57 root bits whatever the mantissa, with an even exponent
                let mut k = 120 - (128 - m.leading_zeros() as i32);
                if (e - k) & 1 != 0 {
                    k -= 1;
                }
                let n = m << k;
                let r = isqrt(n);
                let sticky = if r * r != n { 1 } else { 0 };
                return round_d(false, r, (e - k) / 2, sticky, fpscr);
            }
            let r = ua.value.sqrt();
            let rem = (-r).mul_add(r, ua.value);
            round_to(r, sign_of(rem), p, fpscr)
        }
    }
}

/// `FPMulAdd(addend, op1, op2)`: addend + op1 * op2 with one rounding.
pub fn mul_add(addend: u64, op1: u64, op2: u64, p: Prec, fpscr: &mut u32) -> u64 {
    let ua = unpack(addend, p, fpscr);
    let u1 = unpack(op1, p, fpscr);
    let u2 = unpack(op2, p, fpscr);
    if let Some(n) = process_nans3((ua, addend), (u1, op1), (u2, op2), p, fpscr) {
        return n;
    }
    let inf_prod = u1.kind == Kind::Inf || u2.kind == Kind::Inf;
    let zero_prod = u1.kind == Kind::Zero || u2.kind == Kind::Zero;
    let sign_p = u1.neg != u2.neg;
    if inf_prod && zero_prod {
        return invalid(p, fpscr);
    }
    if ua.kind == Kind::Inf && inf_prod && ua.neg != sign_p {
        return invalid(p, fpscr);
    }
    if ua.kind == Kind::Inf {
        return inf(ua.neg, p);
    }
    if inf_prod {
        return inf(sign_p, p);
    }
    if ua.kind == Kind::Zero && zero_prod && ua.neg == sign_p {
        return zero(ua.neg, p);
    }
    if p == Prec::D {
        // exact: the product has up to 106 bits, the addend is aligned to it with a sticky bit
        let (mp, ep) = if zero_prod {
            (0u128, 0)
        } else {
            let (m1, e1) = mant_exp(u1.value);
            let (m2, e2) = mant_exp(u2.value);
            (m1 * m2, e1 + e2)
        };
        let (mc, ec) = if ua.kind == Kind::Zero { (0u128, 0) } else { mant_exp(ua.value) };
        if mp == 0 && mc == 0 {
            // zeros of different signs: +0, or -0 when rounding towards minus infinity
            return zero(rounding_mode(*fpscr) == Rounding::MinusInf, p);
        }
        if mp == 0 {
            return round_d(ua.neg, mc, ec, 0, fpscr);
        }
        if mc == 0 {
            return round_d(sign_p, mp, ep, 0, fpscr);
        }
        return fma_exact(sign_p, mp, ep, ua.neg, mc, ec, fpscr);
    }
    let prod = u1.value * u2.value; // exact for half and single operands
    let (s, err) = two_sum(ua.value, prod);
    round_sum(s, err, p, fpscr)
}

/// `FPCompare`: returns the NZCV flags in bits 31..28.
pub fn compare(a: u64, b: u64, p: Prec, quiet_nan_exc: bool, fpscr: &mut u32) -> u32 {
    let ua = unpack(a, p, fpscr);
    let ub = unpack(b, p, fpscr);
    let nan = |k| k == Kind::QNaN || k == Kind::SNaN;
    if nan(ua.kind) || nan(ub.kind) {
        if ua.kind == Kind::SNaN || ub.kind == Kind::SNaN || quiet_nan_exc {
            *fpscr |= FPSCR_IOC;
        }
        return 0b0011 << 28;
    }
    if ua.value == ub.value {
        0b0110 << 28
    } else if ua.value < ub.value {
        0b1000 << 28
    } else {
        0b0010 << 28
    }
}

/// Rounds a finite value to an integer under `mode`; returns the integer value and whether
/// it was inexact.
fn round_int(v: f64, mode: Rounding) -> (f64, bool) {
    let fl = v.floor();
    let err = v - fl;
    if err == 0.0 {
        return (v, false);
    }
    let up = match mode {
        Rounding::Nearest => err > 0.5 || (err == 0.5 && (fl / 2.0).fract() != 0.0),
        Rounding::Away => err > 0.5 || (err == 0.5 && v > 0.0),
        Rounding::PlusInf => true,
        Rounding::MinusInf => false,
        Rounding::Zero => v < 0.0,
    };
    (if up { fl + 1.0 } else { fl }, true)
}

/// `FPToFixed`: float to a signed or unsigned integer with `frac` fraction bits, saturating to
/// `bits` (16 or 32). `mode` overrides FPSCR.RMode when given.
pub fn to_fixed(a: u64, p: Prec, bits: u32, frac: u32, unsigned: bool, mode: Option<Rounding>, fpscr: &mut u32) -> u32 {
    let ua = unpack(a, p, fpscr);
    if ua.kind == Kind::QNaN || ua.kind == Kind::SNaN {
        *fpscr |= FPSCR_IOC;
        return 0;
    }
    let (min, max): (i64, i64) = if unsigned { (0, (1i64 << bits) - 1) } else { (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1) };
    let to_bits = |v: i64| {
        if bits == 16 && unsigned {
            (v as u32) & 0xFFFF
        } else if bits == 16 {
            v as i16 as i32 as u32
        } else {
            v as u32
        }
    };
    let sat = |v: i64, fpscr: &mut u32| -> u32 {
        *fpscr |= FPSCR_IOC;
        to_bits(v)
    };
    if ua.kind == Kind::Inf {
        return sat(if ua.neg { min } else { max }, fpscr);
    }
    let v = ua.value * (1u64 << frac) as f64; // exact scaling
    let mode = mode.unwrap_or_else(|| rounding_mode(*fpscr));
    let (i, inexact) = round_int(v, mode);
    if i > max as f64 {
        return sat(max, fpscr);
    }
    if i < min as f64 {
        return sat(min, fpscr);
    }
    if inexact {
        *fpscr |= FPSCR_IXC;
    }
    to_bits(i as i64)
}

/// `FixedToFP`: integer with `frac` fraction bits to float, rounded to nearest (VCVT from
/// integer) or with FPSCR.RMode (fixed-point forms).
pub fn from_fixed(x: u32, p: Prec, bits: u32, frac: u32, unsigned: bool, round_nearest: bool, fpscr: &mut u32) -> u64 {
    let int: i64 = match (bits, unsigned) {
        (16, true) => (x & 0xFFFF) as i64,
        (16, false) => (x as u16 as i16) as i64,
        (_, true) => x as i64,
        (_, false) => x as i32 as i64,
    };
    if int == 0 {
        return 0;
    }
    let v = int as f64 / (1u64 << frac) as f64; // exact
    if round_nearest {
        let saved = *fpscr;
        *fpscr &= !(3 << 22);
        let r = round_to(v, 0, p, fpscr);
        *fpscr = (*fpscr & !(3 << 22)) | (saved & (3 << 22));
        r
    } else {
        round_to(v, 0, p, fpscr)
    }
}

/// `VRINT*`: rounds to an integral value in the same format.
pub fn rint(a: u64, p: Prec, mode: Option<Rounding>, exact_flag: bool, fpscr: &mut u32) -> u64 {
    let ua = unpack(a, p, fpscr);
    match ua.kind {
        Kind::QNaN | Kind::SNaN => process_nan(ua.kind, a, p, fpscr),
        Kind::Inf => inf(ua.neg, p),
        Kind::Zero => zero(ua.neg, p),
        Kind::Normal => {
            let mode = mode.unwrap_or_else(|| rounding_mode(*fpscr));
            let (i, inexact) = round_int(ua.value, mode);
            if inexact && exact_flag {
                *fpscr |= FPSCR_IXC;
            }
            if i == 0.0 {
                return zero(ua.neg, p);
            }
            // integral values of the format are representable: no rounding
            let saved = *fpscr;
            let r = round_to(i, 0, p, fpscr);
            *fpscr = saved | (*fpscr & (FPSCR_OFC | FPSCR_IXC)) | if inexact && exact_flag { FPSCR_IXC } else { 0 };
            r
        }
    }
}

/// `FPMaxNum`/`FPMinNum` (VMAXNM/VMINNM).
pub fn max_min_nm(a: u64, b: u64, p: Prec, max: bool, fpscr: &mut u32) -> u64 {
    let ua = unpack(a, p, fpscr);
    let ub = unpack(b, p, fpscr);
    let nan = |k| k == Kind::QNaN || k == Kind::SNaN;
    // a quiet NaN operand loses against a number: it is replaced by the identity of the operation
    let (a2, ua2) = if ua.kind == Kind::QNaN && !nan(ub.kind) { (inf(max, p), unpack(inf(max, p), p, fpscr)) } else { (a, ua) };
    let (b2, ub2) = if ub.kind == Kind::QNaN && !nan(ua.kind) { (inf(max, p), unpack(inf(max, p), p, fpscr)) } else { (b, ub) };
    if let Some(n) = process_nans((ua2, a2), (ub2, b2), p, fpscr) {
        return n;
    }
    let pick_a = match (ua2.kind, ub2.kind) {
        (Kind::Zero, Kind::Zero) => {
            if max {
                !ua2.neg
            } else {
                ua2.neg
            }
        }
        _ => {
            if max {
                ua2.value >= ub2.value
            } else {
                ua2.value <= ub2.value
            }
        }
    };
    // a denormal operand flushed by FZ yields the zero it was flushed to
    if pick_a {
        if ua2.kind == Kind::Zero {
            zero(ua2.neg, p)
        } else {
            a2 & mask(p)
        }
    } else if ub2.kind == Kind::Zero {
        zero(ub2.neg, p)
    } else {
        b2 & mask(p)
    }
}

/// Conversion from half precision with or without the FZ16 flush of a denormal input.
pub fn convert_from_half(a: u64, to: Prec, flush: bool, fpscr: &mut u32) -> u64 {
    if flush {
        return convert(a, Prec::H, to, fpscr);
    }
    let saved = *fpscr & FPSCR_FZ16;
    *fpscr &= !FPSCR_FZ16;
    let r = convert(a, Prec::H, to, fpscr);
    *fpscr |= saved;
    r
}

/// Conversion between precisions (`FPConvert`): VCVTB/VCVTT, VCVT.F64.F32, VCVT.F32.F64.
pub fn convert(a: u64, from: Prec, to: Prec, fpscr: &mut u32) -> u64 {
    let ua = unpack(a, from, fpscr);
    let ahp = to == Prec::H && *fpscr & FPSCR_AHP != 0;
    let ahp_from = from == Prec::H && *fpscr & FPSCR_AHP != 0;
    let f_from = format(from);
    // the alternative half-precision format has no infinities or NaNs
    let ua = if ahp_from && ua.kind != Kind::Zero && ua.kind != Kind::Normal {
        let frac = a & ((1u64 << f_from.frac_bits) - 1);
        let v = (1024 + frac) as f64 * 2f64.powi(31 - 15 - 10);
        Unpacked { kind: Kind::Normal, neg: ua.neg, value: if ua.neg { -v } else { v } }
    } else {
        ua
    };
    match ua.kind {
        Kind::QNaN | Kind::SNaN => {
            if ahp {
                *fpscr |= FPSCR_IOC;
                return zero(ua.neg, to);
            }
            if ua.kind == Kind::SNaN {
                *fpscr |= FPSCR_IOC;
            }
            if *fpscr & FPSCR_DN != 0 {
                return default_nan(to);
            }
            // keep the fraction bits that fit
            let frac = a & ((1u64 << f_from.frac_bits) - 1);
            let f_to = format(to);
            let frac_to = if f_to.frac_bits >= f_from.frac_bits {
                frac << (f_to.frac_bits - f_from.frac_bits)
            } else {
                frac >> (f_from.frac_bits - f_to.frac_bits)
            };
            (inf(ua.neg, to) | frac_to | quiet_bit(to)) & mask(to)
        }
        Kind::Inf => {
            if ahp {
                *fpscr |= FPSCR_IOC;
                zero(ua.neg, to) | 0x7FFF
            } else {
                inf(ua.neg, to)
            }
        }
        Kind::Zero => zero(ua.neg, to),
        Kind::Normal => match to {
            Prec::D => round_f64(ua.value, 0, fpscr),
            Prec::S => round_fmt(ua.value, 0, SINGLE, fpscr, false, *fpscr & FPSCR_FZ != 0) as u64,
            Prec::H => round_fmt(ua.value, 0, HALF, fpscr, ahp, false) as u64,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(x: f32) -> u64 {
        x.to_bits() as u64
    }

    fn d(x: f64) -> u64 {
        x.to_bits()
    }

    #[test]
    fn basic_single() {
        let mut s = 0;
        assert_eq!(add(f(1.0), f(2.0), Prec::S, &mut s), f(3.0));
        assert_eq!(mul(f(1.5), f(2.0), Prec::S, &mut s), f(3.0));
        assert_eq!(div(f(1.0), f(3.0), Prec::S, &mut s), f(1.0 / 3.0));
        assert_eq!(sqrt(f(2.0), Prec::S, &mut s), f(2f32.sqrt()));
        assert_eq!(sub(f(1.0), f(1.0), Prec::S, &mut s), 0);
        assert_eq!(s & FPSCR_IXC, FPSCR_IXC);
    }

    #[test]
    fn basic_double() {
        let mut s = 0;
        assert_eq!(add(d(1.0), d(2.0), Prec::D, &mut s), d(3.0));
        assert_eq!(mul(d(1.5), d(2.0), Prec::D, &mut s), d(3.0));
        assert_eq!(div(d(1.0), d(3.0), Prec::D, &mut s), d(1.0 / 3.0));
        assert_eq!(sqrt(d(2.0), Prec::D, &mut s), d(2f64.sqrt()));
        assert_eq!(mul_add(d(1.0), d(2.0), d(3.0), Prec::D, &mut s), d(7.0));
        assert_eq!(s & FPSCR_IXC, FPSCR_IXC);
        // directed rounding of an inexact quotient
        let mut up = 1 << 22;
        let q = div(d(1.0), d(3.0), Prec::D, &mut up);
        assert_eq!(q, d(1.0 / 3.0) + 1);
        let mut down = 2 << 22;
        assert_eq!(div(d(1.0), d(3.0), Prec::D, &mut down), d(1.0 / 3.0));
        let mut z = 3 << 22;
        assert_eq!(div(d(-1.0), d(3.0), Prec::D, &mut z), d(-1.0 / 3.0));
        // overflow
        let mut o = 0;
        assert_eq!(mul(d(1e300), d(1e300), Prec::D, &mut o), d(f64::INFINITY));
        assert!(o & FPSCR_OFC != 0);
        let mut oz = 3 << 22;
        assert_eq!(mul(d(1e300), d(1e300), Prec::D, &mut oz), d(f64::MAX));
    }

    #[test]
    fn half() {
        let mut s = 0;
        assert_eq!(convert(f(1.0), Prec::S, Prec::H, &mut s), 0x3C00);
        assert_eq!(convert(0x3C00, Prec::H, Prec::S, &mut s), f(1.0));
        assert_eq!(convert(0x3C00, Prec::H, Prec::D, &mut s), d(1.0));
        assert_eq!(convert(d(0.5), Prec::D, Prec::H, &mut s), 0x3800);
        assert_eq!(add(0x3C00, 0x3C00, Prec::H, &mut s), 0x4000);
        assert_eq!(convert(f(65520.0), Prec::S, Prec::H, &mut s), 0x7C00);
        assert!(s & FPSCR_OFC != 0);
    }

    #[test]
    fn conversions() {
        let mut s = 0;
        assert_eq!(to_fixed(f(-1.5), Prec::S, 32, 0, false, Some(Rounding::Zero), &mut s), -1i32 as u32);
        assert_eq!(to_fixed(f(2.5), Prec::S, 32, 0, false, None, &mut s), 2);
        assert_eq!(to_fixed(d(2.5), Prec::D, 32, 0, false, Some(Rounding::Away), &mut s), 3);
        assert_eq!(to_fixed(d(3e9), Prec::D, 32, 0, false, None, &mut s), 0x7FFF_FFFF);
        assert_eq!(from_fixed(7, Prec::D, 32, 1, false, true, &mut s), d(3.5));
        assert_eq!(convert(d(0.1), Prec::D, Prec::S, &mut s), f(0.1f64 as f32));
        assert_eq!(convert(f(0.1), Prec::S, Prec::D, &mut s), d(0.1f32 as f64));
        assert_eq!(rint(d(2.5), Prec::D, Some(Rounding::Nearest), false, &mut s), d(2.0));
        assert_eq!(rint(d(2.5), Prec::D, Some(Rounding::Away), false, &mut s), d(3.0));
        assert_eq!(rint(f(-0.4), Prec::S, Some(Rounding::Nearest), false, &mut s), f(-0.0));
        assert_eq!(max_min_nm(f(1.0), f(2.0), Prec::S, true, &mut s), f(2.0));
        assert_eq!(max_min_nm(DEFAULT_NAN32 as u64, f(2.0), Prec::S, false, &mut s), f(2.0));
    }

    #[test]
    fn nan_propagation() {
        let mut s = 0;
        let snan = 0x7F80_0001u64;
        let r = add(snan, f(1.0), Prec::S, &mut s);
        assert_eq!(r, 0x7FC0_0001);
        assert!(s & FPSCR_IOC != 0);
        let mut dn = FPSCR_DN;
        assert_eq!(add(snan, f(1.0), Prec::S, &mut dn), DEFAULT_NAN32 as u64);
        let mut t = 0;
        assert_eq!(add(d(f64::INFINITY), d(f64::NEG_INFINITY), Prec::D, &mut t), DEFAULT_NAN64);
    }
}

#[cfg(test)]
mod denorm_tests {
    use super::*;

    #[test]
    fn double_denormals_survive_without_fz() {
        let mut f = FPSCR_RESET;
        assert_eq!(add(0, 1, Prec::D, &mut f), 1, "0 + min denormal");
        assert_eq!(f & 0x9F, 0, "no flags");
        let mut f = FPSCR_RESET;
        assert_eq!(compare(0, 1, Prec::D, false, &mut f) >> 28, 0b1000, "0 < denormal");
        let mut f = FPSCR_RESET;
        assert_eq!(unpack(0xAD55, Prec::H, &mut f).value, -(0x155 as f64 + 1024.0) * 2f64.powi(11 - 15 - 10));
    }
}

#[cfg(test)]
mod h_mve_tests {
    use super::*;
    #[test]
    fn half_denormal_add_under_fz_dn() {
        let mut f = 0x0304_0000; // DN | FZ | LTPSIZE
        let r = add(0x807F, 0, Prec::H, &mut f);
        assert_eq!(r, 0x807F, "fpscr {f:#x}");
        let mut f = 0x0304_0000;
        let r = add(0x0123, 0, Prec::H, &mut f);
        assert_eq!(r, 0x0123, "fpscr {f:#x}");
        let mut f = 0x0304_0000;
        let r = add(0, 1, Prec::H, &mut f);
        assert_eq!(r, 1, "0 + min denormal, fpscr {f:#x}");
        let mut f = 0x0304_0000;
        let r = add(0x3C00, 0x807F, Prec::H, &mut f);
        assert_eq!(r, 0x3C00, "1 - denormal rounds to 1, fpscr {f:#x}");
    }
}
