//! The interface between the core and the rest of the system (memories, peripherals).
//!
//! The private peripheral bus (0xE000_0000..0xE010_0000: NVIC, SCB, SysTick, MPU, SAU, debug
//! components) is part of the core and never reaches the [`Bus`].

use cs320_core::Ticks;

/// A bus error response; the core turns it into a BusFault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusError;

/// Reason for a reset request from the core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetRequest {
    /// `AIRCR.SYSRESETREQ`: system reset of the device.
    System,
    /// The core locked up (the SSE-320 can reset the system on CPU0LOCKUP).
    Lockup,
}

/// Security attribute of an address as the Implementation Defined Attribution Unit sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Idau {
    /// Not covered by the IDAU: the SAU decides.
    None,
    /// Non-secure.
    NonSecure,
    /// Secure, Non-secure callable.
    NonSecureCallable,
    /// Exempt from attribution (the PPB, debug components).
    Exempt,
}

/// Interrupt line levels of the 480 external interrupts (bit n of word n/64 = IRQ n).
pub type IrqLines = [u64; 8];

pub trait Bus {
    /// Data read of 1, 2 or 4 bytes at `addr` (aligned to the size by the core, except for
    /// unaligned word/halfword accesses to normal memory). `ns` is the security attribute of
    /// the transaction. `debug` accesses come from the debugger and must not have side effects.
    fn read(&mut self, now: Ticks, addr: u32, size: u8, ns: bool, debug: bool) -> Result<u32, BusError>;

    /// Data write of 1, 2 or 4 bytes.
    fn write(&mut self, now: Ticks, addr: u32, size: u8, value: u32, ns: bool, debug: bool) -> Result<(), BusError>;

    /// Instruction fetch of a halfword.
    fn fetch16(&mut self, addr: u32, ns: bool) -> Result<u16, BusError>;

    /// The core must stop before starting an instruction at or after this time, so that the
    /// system can process its events.
    fn deadline(&self) -> Ticks;

    /// Interrupt line levels when they changed since the last call.
    fn take_irq_lines(&mut self) -> Option<IrqLines>;

    /// IDAU region number of an address for TT (`None` for exempt regions). The SSE-320 IDAU
    /// numbers the 256 MB blocks 0..13, the PPB is exempt, 0xE010_0000.. is region 15, the
    /// 0xF000_0000 system region is exempt and 0xF100_0000.. is region 17 (measured on the FVP).
    fn idau_region(&self, addr: u32) -> Option<u32> {
        match addr >> 24 {
            0x00..=0xDF => Some(addr >> 28),
            0xE0 if addr < 0xE010_0000 => None,
            0xE0..=0xEF => Some(15),
            0xF0 => None,
            _ => Some(17),
        }
    }

    /// Security attribute of an address from the system's IDAU.
    fn idau(&self, addr: u32) -> Idau {
        if (0xE000_0000..0xE010_0000).contains(&addr) {
            Idau::Exempt
        } else if addr & 0x1000_0000 == 0 {
            Idau::NonSecure
        } else {
            Idau::None
        }
    }

    /// Address of the Secure vector table at reset (`INITSVTOR`).
    fn reset_vector_base(&self) -> u32 {
        0
    }

    /// Extra wait cycles of an instruction fetch at `addr`.
    fn fetch_wait_cycles(&self, _addr: u32) -> u32 {
        0
    }

    /// Time until which the core stalls after the last access, if any.
    fn take_stall(&mut self) -> Option<Ticks> {
        None
    }

    /// Instructions in these address ranges may be cached in decoded form (code memory that
    /// only the loader and the debugger write). Pages of 4 KB.
    fn code_cacheable(&self, _addr: u32) -> bool {
        false
    }

    /// Changes whenever cacheable code memory is written (by the loader or the debugger).
    fn code_generation(&self) -> u64 {
        0
    }
}
