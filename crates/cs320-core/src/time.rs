//! Simulation time.
//!
//! One tick is 1/12.8 GHz (78.125 ps), the least common multiple of the 25 MHz system clock,
//! the 32 MHz and 100 MHz counter clocks and the 32.768 kHz slow clock of the SSE-320
//! (2^15 * 5^8 Hz), so that every clock has a whole number of ticks per period. A `u64` covers
//! 45 years.

/// Simulation time in ticks of 1/12.8 GHz.
pub type Ticks = u64;

pub const TICKS_PER_SEC: Ticks = 12_800_000_000;
pub const TICKS_PER_MS: Ticks = 12_800_000;
pub const TICKS_PER_US: Ticks = 12_800;
/// 25 MHz CPU and system clock.
pub const TICKS_PER_CPU_CYCLE: Ticks = 512;
/// 25 MHz peripheral clock.
pub const TICKS_PER_PERIPH_CLK: Ticks = 512;
/// 100 MHz reference counter clock.
pub const TICKS_PER_100MHZ: Ticks = 128;
/// 32 MHz clock.
pub const TICKS_PER_32MHZ: Ticks = 400;
/// 32.768 kHz slow clock.
pub const TICKS_PER_SLOWCLK: Ticks = 390_625;

pub const fn us(n: u64) -> Ticks {
    n * TICKS_PER_US
}

pub const fn ms(n: u64) -> Ticks {
    n * TICKS_PER_MS
}

/// Rounds a time up to the next CPU clock edge.
pub const fn ceil_cpu_cycle(t: Ticks) -> Ticks {
    t.div_ceil(TICKS_PER_CPU_CYCLE) * TICKS_PER_CPU_CYCLE
}

/// Converts ticks to seconds (for display only).
pub fn to_secs(t: Ticks) -> f64 {
    t as f64 / TICKS_PER_SEC as f64
}

/// Converts seconds to ticks, rounding to the nearest tick.
pub fn from_secs(s: f64) -> Ticks {
    (s * TICKS_PER_SEC as f64).round().max(0.0) as Ticks
}

/// Ticks per period of a clock of `hz` hertz, rounded to the nearest tick (for clocks that are
/// not whole divisors; use [`FracPeriod`] for exact sequences).
pub fn ticks_per_hz(hz: f64) -> Ticks {
    (TICKS_PER_SEC as f64 / hz.max(1e-9)).round().max(1.0) as Ticks
}

/// Accumulates a period that is not a whole number of ticks (baud and sample periods), so that
/// the error of a sequence of periods stays below one tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct FracPeriod {
    /// Period in units of 1/2^32 ticks.
    pub period_q32: u128,
}

impl FracPeriod {
    /// A period of `ticks_num / ticks_den` ticks.
    pub fn new(ticks_num: u128, ticks_den: u128) -> Self {
        Self { period_q32: (ticks_num << 32) / ticks_den.max(1) }
    }

    /// A period of `1 / hz` seconds.
    pub fn from_hz(hz: u64) -> Self {
        Self::new(TICKS_PER_SEC as u128, hz.max(1) as u128)
    }

    /// Time of the end of the `n`-th period after `start`.
    pub fn after(&self, start: Ticks, n: u64) -> Ticks {
        start + ((self.period_q32 * n as u128 + (1 << 31)) >> 32) as Ticks
    }

    /// Number of whole periods between `start` and `now`.
    pub fn count(&self, start: Ticks, now: Ticks) -> u64 {
        if now <= start || self.period_q32 == 0 {
            return 0;
        }
        ((((now - start) as u128) << 32) / self.period_q32) as u64
    }
}
