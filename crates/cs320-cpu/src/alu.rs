//! Integer helper functions of the ARMv7-M pseudocode (A2.2 "Pseudocode details of operations
//! on ARM core registers").

use crate::decode::{ASR, LSL, LSR, ROR, RRX};

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
