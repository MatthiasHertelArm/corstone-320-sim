//! Context passed to peripheral models, the scheduler slots and the warning collector.

use crate::board::BoardIo;
use crate::mem::Memory;
use cs320_core::{Scheduler, Slot, Ticks};
use std::collections::HashSet;

/// Scheduler slots of the subsystem.
pub mod slot {
    use cs320_core::Slot;
    pub const UART_TX: [Slot; 6] = [Slot(0), Slot(1), Slot(2), Slot(3), Slot(4), Slot(5)];
    pub const UART_RX: [Slot; 6] = [Slot(6), Slot(7), Slot(8), Slot(9), Slot(10), Slot(11)];
    pub const SYSTIMER: [Slot; 4] = [Slot(12), Slot(13), Slot(14), Slot(15)];
    pub const SLOWCLK_TIMER: Slot = Slot(16);
    pub const WDOG: [Slot; 2] = [Slot(17), Slot(18)];
    pub const SLOWCLK_WDOG: Slot = Slot(19);
    pub const HDLCD: Slot = Slot(20);
    pub const I2S: Slot = Slot(21);
    pub const RTC: Slot = Slot(22);
    pub const VSI: [Slot; 8] = [Slot(23), Slot(24), Slot(25), Slot(26), Slot(27), Slot(28), Slot(29), Slot(30)];
    pub const CAM: Slot = Slot(31);
    pub const NPU: Slot = Slot(32);
    pub const FPGAIO: Slot = Slot(33);
    pub const DMA: Slot = Slot(34);
    pub const COUNT: usize = 35;
}

/// Warnings about behaviour the model does not cover or that is undefined on the device.
/// Each distinct message is logged once.
#[derive(Default, Debug)]
pub struct Warnings {
    seen: HashSet<String>,
    pub list: Vec<(Ticks, String)>,
}

impl Warnings {
    pub fn warn(&mut self, now: Ticks, msg: impl Into<String>) {
        let msg = msg.into();
        if self.seen.insert(msg.clone()) {
            log::warn!("{msg}");
            self.list.push((now, msg));
        }
    }
}

/// Clock configuration of the subsystem.
#[derive(Clone, Copy, Debug)]
pub struct Clocks {
    /// CPU and system clock (25 MHz on the FVP).
    pub cpu_hz: u64,
    /// Peripheral clock of the UARTs, SPI, I2C (25 MHz).
    pub periph_hz: u64,
    /// Clock of the system counter that drives the system timers.
    pub counter_hz: u64,
    /// Slow clock of the always-on timer and watchdog.
    pub slow_hz: u64,
    /// Master clock of the audio I2S.
    pub audio_mclk_hz: u64,
}

impl Default for Clocks {
    fn default() -> Self {
        // measured on FVP_Corstone_SSE-320: CNTFID0 = 100 MHz, the slow clock is 32 kHz (not 32.768)
        Clocks { cpu_hz: 25_000_000, periph_hz: 25_000_000, counter_hz: 100_000_000, slow_hz: 32_000, audio_mclk_hz: 12_288_000 }
    }
}

/// What a peripheral can reach while it handles an access or an event.
pub struct Env<'a> {
    pub now: Ticks,
    pub sched: &'a mut Scheduler,
    pub mem: &'a mut Memory,
    pub board: &'a mut dyn BoardIo,
    pub warn: &'a mut Warnings,
    pub clocks: Clocks,
}

impl Env<'_> {
    pub fn schedule(&mut self, slot: Slot, at: Ticks) {
        self.sched.schedule(slot, at);
    }

    pub fn cancel(&mut self, slot: Slot) {
        self.sched.cancel(slot);
    }
}

/// Reads a 32-bit register value as a 1, 2 or 4-byte access at a byte offset.
#[inline]
pub fn narrow(v: u32, addr: u32, size: u8) -> u32 {
    let sh = (addr & 3) * 8;
    match size {
        1 => (v >> sh) & 0xFF,
        2 => (v >> sh) & 0xFFFF,
        _ => v,
    }
}

/// Merges a 1, 2 or 4-byte write into a 32-bit register value.
#[inline]
pub fn widen(old: u32, addr: u32, size: u8, v: u32) -> u32 {
    let sh = (addr & 3) * 8;
    match size {
        1 => (old & !(0xFF << sh)) | ((v & 0xFF) << sh),
        2 => (old & !(0xFFFF << sh)) | ((v & 0xFFFF) << sh),
        _ => v,
    }
}

/// CoreSight-style component and peripheral ID registers at 0xFD0..0xFFC of a 4 KB block.
pub fn coresight_id(off: u32, pidr: &[u32; 8], cidr: &[u32; 4]) -> Option<u32> {
    Some(match off {
        0xFD0 => pidr[4],
        0xFD4 => pidr[5],
        0xFD8 => pidr[6],
        0xFDC => pidr[7],
        0xFE0 => pidr[0],
        0xFE4 => pidr[1],
        0xFE8 => pidr[2],
        0xFEC => pidr[3],
        0xFF0 => cidr[0],
        0xFF4 => cidr[1],
        0xFF8 => cidr[2],
        0xFFC => cidr[3],
        _ => return None,
    })
}
