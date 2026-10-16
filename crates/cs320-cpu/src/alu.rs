//! Integer helper functions of the ARMv7-M pseudocode (A2.2 "Pseudocode details of operations
//! on ARM core registers").

use crate::decode::{LongShiftOp, ASR, LSL, LSR, ROR, RRX};

// ---------------------------------------------------------------------------------------------
// Armv8.1-M scalar shifts (LSLL, ASRL, SQRSHRL, ...), after the architecture's pseudocode: a
// positive `shift` is to the left, a negative one to the right (the right-shifting mnemonics
// negate their amount), `round` adds half an LSB before a right shift, `sat` saturates and
// reports it (the Q flag).

/// Signed 64-bit shift.
fn sqrshl_d(src: i64, shift: i32, round: bool, sat: Option<&mut bool>) -> i64 {
    if shift <= -64 {
        return if round { 0 } else { src >> 63 };
    }
    if shift < 0 {
        if round {
            let s = src >> (-shift - 1);
            return (s >> 1) + (s & 1);
        }
        return src >> -shift;
    }
    if shift < 64 {
        let val = src.wrapping_shl(shift as u32);
        if sat.is_none() || (val >> shift) == src {
            return val;
        }
    } else if sat.is_none() || src == 0 {
        return 0;
    }
    if let Some(q) = sat {
        *q = true;
    }
    if src < 0 {
        i64::MIN
    } else {
        i64::MAX
    }
}

/// Unsigned 64-bit shift.
fn uqrshl_d(src: u64, shift: i32, round: bool, sat: Option<&mut bool>) -> u64 {
    if shift <= -(64 + round as i32) {
        return 0;
    }
    if shift < 0 {
        if round {
            let s = src >> (-shift - 1);
            return (s >> 1) + (s & 1);
        }
        return src >> -shift;
    }
    if shift < 64 {
        let val = src.wrapping_shl(shift as u32);
        if sat.is_none() || (val >> shift) == src {
            return val;
        }
    } else if sat.is_none() || src == 0 {
        return 0;
    }
    if let Some(q) = sat {
        *q = true;
    }
    u64::MAX
}

const MASK48: u64 = (1 << 48) - 1;

fn sext48(v: i64) -> i64 {
    (v << 16) >> 16
}

/// Signed shift saturating to 48 bits (SQRSHRL with `#48`).
fn sqrshl48_d(src: i64, shift: i32, round: bool, sat: Option<&mut bool>) -> i64 {
    if shift <= -48 {
        return if round { 0 } else { src >> 63 };
    }
    if shift < 0 {
        let val = if round {
            let s = src >> (-shift - 1);
            (s >> 1) + (s & 1)
        } else {
            src >> -shift
        };
        let ext = sext48(val);
        if sat.is_none() || val == ext {
            return ext;
        }
    } else if shift < 48 {
        let ext = sext48(src.wrapping_shl(shift as u32));
        if sat.is_none() || src == (ext >> shift) {
            return ext;
        }
    } else if sat.is_none() || src == 0 {
        return 0;
    }
    if let Some(q) = sat {
        *q = true;
    }
    if src >= 0 {
        (1 << 47) - 1
    } else {
        -(1 << 47)
    }
}

/// Unsigned shift saturating to 48 bits (UQRSHLL with `#48`).
fn uqrshl48_d(src: u64, shift: i32, round: bool, sat: Option<&mut bool>) -> u64 {
    if shift <= -(48 + round as i32) {
        return 0;
    }
    if shift < 0 {
        let val = if round {
            let s = src >> (-shift - 1);
            (s >> 1) + (s & 1)
        } else {
            src >> -shift
        };
        if sat.is_none() || val == val & MASK48 {
            return val & MASK48;
        }
    } else if shift < 48 {
        let ext = src.wrapping_shl(shift as u32) & MASK48;
        if sat.is_none() || src == (ext >> shift) {
            return ext;
        }
    } else if sat.is_none() || src == 0 {
        return 0;
    }
    if let Some(q) = sat {
        *q = true;
    }
    MASK48
}

/// Signed 32-bit shift.
fn sqrshl_w(src: i32, shift: i32, round: bool, sat: Option<&mut bool>) -> i32 {
    if shift <= -32 {
        return if round { 0 } else { src >> 31 };
    }
    if shift < 0 {
        if round {
            let s = src >> (-shift - 1);
            return (s >> 1) + (s & 1);
        }
        return src >> -shift;
    }
    if shift < 32 {
        let val = src.wrapping_shl(shift as u32);
        if sat.is_none() || (val >> shift) == src {
            return val;
        }
    } else if sat.is_none() || src == 0 {
        return 0;
    }
    if let Some(q) = sat {
        *q = true;
    }
    if src < 0 {
        i32::MIN
    } else {
        i32::MAX
    }
}

/// Unsigned 32-bit shift.
fn uqrshl_w(src: u32, shift: i32, round: bool, sat: Option<&mut bool>) -> u32 {
    if shift <= -(32 + round as i32) {
        return 0;
    }
    if shift < 0 {
        if round {
            let s = src >> (-shift - 1);
            return (s >> 1) + (s & 1);
        }
        return src >> -shift;
    }
    if shift < 32 {
        let val = src.wrapping_shl(shift as u32);
        if sat.is_none() || (val >> shift) == src {
            return val;
        }
    } else if sat.is_none() || src == 0 {
        return 0;
    }
    if let Some(q) = sat {
        *q = true;
    }
    u32::MAX
}

/// A 64-bit scalar shift of `v` by `shift` (an immediate 1..=32, or the signed low byte of Rm);
/// `q` is set when a saturating form saturates.
pub fn scalar_shift64(op: LongShiftOp, v: u64, shift: i32, sat48: bool, q: &mut bool) -> u64 {
    use LongShiftOp::*;
    match op {
        Lsll => uqrshl_d(v, shift, false, None),
        Lsrl => uqrshl_d(v, -shift, false, None),
        Asrl => sqrshl_d(v as i64, -shift, false, None) as u64,
        Sqshll => sqrshl_d(v as i64, shift, false, Some(q)) as u64,
        Srshrl => sqrshl_d(v as i64, -shift, true, None) as u64,
        Uqshll => uqrshl_d(v, shift, false, Some(q)),
        Urshrl => uqrshl_d(v, -shift, true, None),
        Sqrshrl if sat48 => sqrshl48_d(v as i64, -shift, true, Some(q)) as u64,
        Sqrshrl => sqrshl_d(v as i64, -shift, true, Some(q)) as u64,
        Uqrshll if sat48 => uqrshl48_d(v, shift, true, Some(q)),
        Uqrshll => uqrshl_d(v, shift, true, Some(q)),
        Sqshl | Srshr | Uqshl | Urshr | Sqrshr | Uqrshl => v,
    }
}

/// A 32-bit scalar shift (SQSHL, SRSHR, UQSHL, URSHR, SQRSHR, UQRSHL), as `scalar_shift64`.
pub fn scalar_shift32(op: LongShiftOp, v: u32, shift: i32, q: &mut bool) -> u32 {
    use LongShiftOp::*;
    match op {
        Sqshl => sqrshl_w(v as i32, shift, false, Some(q)) as u32,
        Srshr => sqrshl_w(v as i32, -shift, true, None) as u32,
        Uqshl => uqrshl_w(v, shift, false, Some(q)),
        Urshr => uqrshl_w(v, -shift, true, None),
        Sqrshr => sqrshl_w(v as i32, -shift, true, Some(q)) as u32,
        Uqrshl => uqrshl_w(v, shift, true, Some(q)),
        _ => v,
    }
}

/// `AddWithCarry(x, y, carry_in)` returning (result, carry_out, overflow).
#[inline]
pub fn add_with_carry(x: u32, y: u32, carry: bool) -> (u32, bool, bool) {
    let wide = x as u64 + y as u64 + carry as u64;
    let r = wide as u32;
    let c = wide >> 32 != 0;
    let v = ((x ^ r) & (y ^ r)) >> 31 != 0;
    (r, c, v)
}

/// `Shift_C(value, type, amount, carry_in)` for amounts 0..=255 (register shifts included).
#[inline]
pub fn shift_c(value: u32, ty: u8, amount: u32, carry_in: bool) -> (u32, bool) {
    if ty == RRX {
        return ((value >> 1) | ((carry_in as u32) << 31), value & 1 != 0);
    }
    if amount == 0 {
        return (value, carry_in);
    }
    match ty {
        LSL => {
            if amount < 32 {
                (value << amount, (value >> (32 - amount)) & 1 != 0)
            } else if amount == 32 {
                (0, value & 1 != 0)
            } else {
                (0, false)
            }
        }
        LSR => {
            if amount < 32 {
                (value >> amount, (value >> (amount - 1)) & 1 != 0)
            } else if amount == 32 {
                (0, value >> 31 != 0)
            } else {
                (0, false)
            }
        }
        ASR => {
            if amount < 32 {
                (((value as i32) >> amount) as u32, (value >> (amount - 1)) & 1 != 0)
            } else {
                let r = ((value as i32) >> 31) as u32;
                (r, value >> 31 != 0)
            }
        }
        _ => {
            debug_assert_eq!(ty, ROR);
            let r = value.rotate_right(amount % 32);
            (r, r >> 31 != 0)
        }
    }
}

#[inline]
pub fn shift(value: u32, ty: u8, amount: u32, carry_in: bool) -> u32 {
    shift_c(value, ty, amount, carry_in).0
}

/// `SignedSatQ(i, n)`: saturates to an n-bit signed value; returns (result, saturated).
#[inline]
pub fn signed_sat_q(i: i64, n: u32) -> (i32, bool) {
    let max = (1i64 << (n - 1)) - 1;
    let min = -(1i64 << (n - 1));
    if i > max {
        (max as i32, true)
    } else if i < min {
        (min as i32, true)
    } else {
        (i as i32, false)
    }
}

/// `UnsignedSatQ(i, n)`: saturates to an n-bit unsigned value; returns (result, saturated).
#[inline]
pub fn unsigned_sat_q(i: i64, n: u32) -> (u32, bool) {
    let max = if n >= 32 { u32::MAX as i64 } else { (1i64 << n) - 1 };
    if i > max {
        (max as u32, true)
    } else if i < 0 {
        (0, true)
    } else {
        (i as u32, false)
    }
}

/// Condition check (`ConditionPassed`) for a 4-bit condition and the APSR flags.
#[inline]
pub fn condition_passed(cond: u8, n: bool, z: bool, c: bool, v: bool) -> bool {
    let r = match cond >> 1 {
        0 => z,
        1 => c,
        2 => n,
        3 => v,
        4 => c && !z,
        5 => n == v,
        6 => n == v && !z,
        _ => true,
    };
    if cond & 1 == 1 && cond != 0xF {
        !r
    } else {
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adc() {
        assert_eq!(add_with_carry(0xFFFF_FFFF, 1, false), (0, true, false));
        assert_eq!(add_with_carry(0x7FFF_FFFF, 1, false), (0x8000_0000, false, true));
        // subtraction x - y = x + !y + 1
        assert_eq!(add_with_carry(5, !3, true), (2, true, false));
        assert_eq!(add_with_carry(3, !5, true), (0xFFFF_FFFE, false, false));
    }

    #[test]
    fn shifts() {
        assert_eq!(shift_c(0x8000_0001, LSL, 1, false), (2, true));
        assert_eq!(shift_c(0x8000_0001, LSR, 32, false), (0, true));
        assert_eq!(shift_c(0x8000_0000, ASR, 40, false), (0xFFFF_FFFF, true));
        assert_eq!(shift_c(0x0000_0003, ROR, 1, false), (0x8000_0001, true));
        assert_eq!(shift_c(0x8000_0000, ROR, 32, false), (0x8000_0000, true));
        assert_eq!(shift_c(0x0000_0003, RRX, 1, true), (0x8000_0001, true));
        assert_eq!(shift_c(5, LSL, 0, true), (5, true));
    }

    #[test]
    fn sat() {
        assert_eq!(signed_sat_q(200, 8), (127, true));
        assert_eq!(signed_sat_q(-200, 8), (-128, true));
        assert_eq!(unsigned_sat_q(-1, 8), (0, true));
        assert_eq!(unsigned_sat_q(255, 8), (255, false));
        assert_eq!(unsigned_sat_q(0, 0), (0, false));
    }

    #[test]
    fn scalar_shifts() {
        use LongShiftOp::*;
        let mut q = false;
        // LSLL #16 of 480 (the 16.16 fixed point of a C `(int64_t) w << 16`)
        assert_eq!(scalar_shift64(Lsll, 480, 16, false, &mut q), 480 << 16);
        assert_eq!(scalar_shift64(Lsll, 0xFFFF_FFFF, 32, false, &mut q), 0xFFFF_FFFF_0000_0000);
        assert_eq!(scalar_shift64(Lsrl, 0x8000_0000_0000_0000, 1, false, &mut q), 0x4000_0000_0000_0000);
        assert_eq!(scalar_shift64(Asrl, 0x8000_0000_0000_0000, 1, false, &mut q), 0xC000_0000_0000_0000);
        assert_eq!(scalar_shift64(Asrl, (-1024i64) as u64, 10, false, &mut q), (-1i64) as u64);
        // register forms: a negative amount shifts the other way
        assert_eq!(scalar_shift64(Lsll, 0x1000, -4, false, &mut q), 0x100);
        assert_eq!(scalar_shift64(Lsll, 0x8000_0000_0000_0000, -1, false, &mut q), 0x4000_0000_0000_0000);
        assert_eq!(scalar_shift64(Asrl, 0x1000, -4, false, &mut q), 0x10000);
        assert_eq!(scalar_shift64(Lsll, 1, 64, false, &mut q), 0);
        assert_eq!(scalar_shift64(Lsll, 1, 100, false, &mut q), 0);
        assert!(!q);
        // saturation and rounding
        assert_eq!(scalar_shift64(Sqshll, 0x4000_0000_0000_0000, 1, false, &mut q), i64::MAX as u64);
        assert!(q);
        q = false;
        assert_eq!(scalar_shift64(Sqshll, 0x2000_0000_0000_0000, 1, false, &mut q), 0x4000_0000_0000_0000);
        assert!(!q);
        assert_eq!(scalar_shift64(Uqshll, 0x8000_0000_0000_0000, 1, false, &mut q), u64::MAX);
        assert!(q);
        q = false;
        assert_eq!(scalar_shift64(Srshrl, 7, 1, false, &mut q), 4);
        assert_eq!(scalar_shift64(Srshrl, 5, 1, false, &mut q), 3);
        assert_eq!(scalar_shift64(Urshrl, 0x17, 4, false, &mut q), 1);
        assert_eq!(scalar_shift64(Urshrl, 0x18, 4, false, &mut q), 2);
        assert_eq!(scalar_shift64(Sqrshrl, (-7i64) as u64, 1, false, &mut q), (-3i64) as u64);
        assert_eq!(scalar_shift64(Sqrshrl, 1 << 40, -8, true, &mut q), (1i64 << 47) as u64 - 1);
        assert!(q);
        q = false;
        assert_eq!(scalar_shift64(Uqrshll, 1 << 40, 8, true, &mut q), (1 << 48) - 1);
        assert!(q);
        q = false;
        assert_eq!(scalar_shift64(Uqrshll, 1 << 40, 4, true, &mut q), 1 << 44);
        assert!(!q);
        // 32-bit forms
        assert_eq!(scalar_shift32(Sqshl, 0x4000_0000, 1, &mut q), i32::MAX as u32);
        assert!(q);
        q = false;
        assert_eq!(scalar_shift32(Uqshl, 0x4000_0000, 1, &mut q), 0x8000_0000);
        assert!(!q);
        assert_eq!(scalar_shift32(Srshr, (-6i32) as u32, 2, &mut q), (-1i32) as u32);
        assert_eq!(scalar_shift32(Urshr, 6, 2, &mut q), 2);
        assert_eq!(scalar_shift32(Sqrshr, 0x100, -4, &mut q), 0x1000);
        assert_eq!(scalar_shift32(Uqrshl, 0x100, -4, &mut q), 0x10);
        assert_eq!(scalar_shift32(Uqrshl, 0x8000_0000, 1, &mut q), u32::MAX);
        assert!(q);
    }

    #[test]
    fn conditions() {
        // GE: N == V
        assert!(condition_passed(0xA, true, false, false, true));
        assert!(!condition_passed(0xA, true, false, false, false));
        // AL
        assert!(condition_passed(0xE, false, false, false, false));
        // NE
        assert!(condition_passed(0x1, false, false, false, false));
    }
}
