//! The private peripheral bus of the Cortex-M85: ITM, DWT, FPB, PMU, the System Control Space
//! (NVIC, SCB, SysTick, MPU, SAU, FPU control, debug control) with its Non-secure alias, the
//! processor-specific registers at 0xE001_E000, TPIU and the ROM table.

use crate::cpu::*;
use crate::fpu::FPSCR_MASK;
use cs320_core::time::TICKS_PER_CPU_CYCLE;
use cs320_core::Ticks;

/// SysTick timer, modelled analytically from the time of the next wrap.
#[derive(Clone, Debug)]
pub struct SysTick {
    pub ctrl: u32,
    pub load: u32,
    pub countflag: bool,
    /// Time at which the counter next reaches zero (u64::MAX when stopped).
    pub next_wrap: Ticks,
    /// Counter value while stopped.
    stopped_val: u32,
    /// False until the first wrap after an enable from a non-zero value.
    periodic: bool,
    pub calib: u32,
}

impl Default for SysTick {
    fn default() -> Self {
        SysTick { ctrl: 0, load: 0, countflag: false, next_wrap: Ticks::MAX, stopped_val: 0, periodic: true, calib: 0xC000_0000 }
    }
}

impl SysTick {
    fn enabled(&self) -> bool {
        self.ctrl & 1 != 0
    }

    pub fn value(&self, now: Ticks) -> u32 {
        if !self.enabled() || self.next_wrap == Ticks::MAX {
            return self.stopped_val;
        }
        if now >= self.next_wrap {
            return 0;
        }
        let r = (self.next_wrap - now) / TICKS_PER_CPU_CYCLE;
        if self.periodic {
            (r % (self.load as u64 + 1)) as u32
        } else {
            r as u32
        }
    }

    fn start_from(&mut self, now: Ticks, val: u32) {
        if val == 0 {
            self.periodic = true;
            self.next_wrap = if self.load == 0 { Ticks::MAX } else { now + (self.load as u64 + 1) * TICKS_PER_CPU_CYCLE };
            if self.load == 0 {
                self.stopped_val = 0;
            }
        } else {
            self.periodic = false;
            self.next_wrap = now + val as u64 * TICKS_PER_CPU_CYCLE;
        }
    }

    pub fn after_wrap(&mut self, t: Ticks) {
        self.periodic = true;
        if self.load == 0 {
            self.next_wrap = Ticks::MAX;
            self.stopped_val = 0;
        } else {
            self.next_wrap = t + (self.load as u64 + 1) * TICKS_PER_CPU_CYCLE;
        }
    }

    fn write_ctrl(&mut self, now: Ticks, v: u32) {
        let was = self.enabled();
        let cur = self.value(now);
        self.ctrl = v & 7;
        match (was, self.enabled()) {
            (false, true) => self.start_from(now, self.stopped_val),
            (true, false) => {
                self.stopped_val = cur;
                self.next_wrap = Ticks::MAX;
            }
            _ => {}
        }
    }

    fn write_val(&mut self, now: Ticks) {
        self.countflag = false;
        self.stopped_val = 0;
        if self.enabled() {
            self.start_from(now, 0);
        }
    }
}

/// PMSAv8 MPU with 8 regions.
#[derive(Clone, Debug, Default)]
pub struct Mpu {
    pub ctrl: u32,
    pub rnr: u32,
    pub rbar: [u32; 8],
    pub rlar: [u32; 8],
    pub mair: [u32; 2],
}

impl Mpu {
    pub fn enabled(&self) -> bool {
        self.ctrl & 1 != 0
    }

    /// Permission check: `None` when no region matches, otherwise whether the access is allowed.
    pub fn check(&self, addr: u32, privileged: bool, write: bool, exec: bool) -> Option<bool> {
        let mut hit: Option<usize> = None;
        for i in 0..8 {
            if self.rlar[i] & 1 == 0 {
                continue;
            }
            let base = self.rbar[i] & 0xFFFF_FFE0;
            let limit = self.rlar[i] | 0x1F;
            if addr >= base && addr <= limit {
                if hit.is_some() {
                    return Some(false);
                }
                hit = Some(i);
            }
        }
        let i = hit?;
        let rbar = self.rbar[i];
        let ap = (rbar >> 1) & 3;
        let xn = rbar & 1 != 0;
        let (pr, pw, ur, uw) = match ap {
            0 => (true, true, false, false),
            1 => (true, true, true, true),
            2 => (true, false, false, false),
            _ => (true, false, true, false),
        };
        let ok = if exec {
            !xn && if privileged { pr } else { ur }
        } else if write {
            if privileged {
                pw
            } else {
                uw
            }
        } else if privileged {
            pr
        } else {
            ur
        };
        Some(ok)
    }
}

/// Security attribution unit with 8 regions.
#[derive(Clone, Debug, Default)]
pub struct Sau {
    pub ctrl: u32,
    pub rnr: u32,
    pub rbar: [u32; 8],
    pub rlar: [u32; 8],
}

impl Sau {
    /// (secure, non-secure callable) of an address per the SAU alone.
    pub fn attribute(&self, addr: u32) -> (bool, bool) {
        if self.ctrl & 1 == 0 {
            return (self.ctrl & 2 == 0, false);
        }
        let mut result: Option<(bool, bool)> = None;
        for i in 0..8 {
            if self.rlar[i] & 1 == 0 {
                continue;
            }
            let base = self.rbar[i] & 0xFFFF_FFE0;
            let limit = self.rlar[i] | 0x1F;
            if addr >= base && addr <= limit {
                if result.is_some() {
                    return (true, false);
                }
                let nsc = self.rlar[i] & 2 != 0;
                result = Some((nsc, nsc));
            }
        }
        result.unwrap_or((true, false))
    }
}

/// Data watchpoint and trace unit with 4 comparators (Armv8-M MATCH encoding).
#[derive(Clone, Debug)]
pub struct Dwt {
    pub ctrl: u32,
    cyc_base: u32,
    cyc_origin: Ticks,
    cyc_running: bool,
    pub cpicnt: u32,
    pub exccnt: u32,
    pub sleepcnt: u32,
    pub lsucnt: u32,
    pub foldcnt: u32,
    pub comp: [u32; 4],
    pub function: [u32; 4],
}

impl Default for Dwt {
    fn default() -> Self {
        Dwt {
            ctrl: 0x4000_0000,
            cyc_base: 0,
            cyc_origin: 0,
            cyc_running: false,
            cpicnt: 0,
            exccnt: 0,
            sleepcnt: 0,
            lsucnt: 0,
            foldcnt: 0,
            comp: [0; 4],
            function: [0; 4],
        }
    }
}

impl Dwt {
    pub fn cyccnt(&self, now: Ticks) -> u32 {
        if self.cyc_running {
            self.cyc_base.wrapping_add(((now - self.cyc_origin) / TICKS_PER_CPU_CYCLE) as u32)
        } else {
            self.cyc_base
        }
    }

    fn set_cyccnt(&mut self, now: Ticks, v: u32) {
        self.cyc_base = v;
        self.cyc_origin = now;
    }

    /// Re-evaluates whether CYCCNT counts (CYCCNTENA and DEMCR.TRCENA).
    pub fn update_running(&mut self, now: Ticks, trcena: bool) {
        let run = self.ctrl & 1 != 0 && trcena;
        if run != self.cyc_running {
            let v = self.cyccnt(now);
            self.cyc_running = run;
            self.set_cyccnt(now, v);
        }
    }

    pub fn any_data_watch(&self) -> bool {
        self.function.iter().any(|f| matches!(f & 0xF, 4..=6))
    }

    /// Data address comparators that match an access; returns the matching address.
    pub fn match_data(&mut self, addr: u32, size: u32, write: bool) -> Option<u32> {
        let mut hit = None;
        for i in 0..4 {
            let f = self.function[i];
            let kind_ok = match f & 0xF {
                4 => true,
                5 => write,
                6 => !write,
                _ => false,
            };
            if !kind_ok {
                continue;
            }
            let csize = 1u64 << ((f >> 10) & 3);
            let lo = self.comp[i] as u64;
            let hi = lo + csize;
            if (addr as u64) < hi && (addr as u64 + size as u64) > lo {
                self.function[i] |= 1 << 24;
                hit = Some(addr);
            }
        }
        hit
    }
}

/// Flash patch and breakpoint unit (FPBv2: 8 instruction comparators).
#[derive(Clone, Debug, Default)]
pub struct Fpb {
    pub ctrl: u32,
    pub comp: [u32; 8],
}

impl Fpb {
    pub fn matches(&self, pc: u32) -> bool {
        if self.ctrl & 1 == 0 {
            return false;
        }
        self.comp.iter().any(|&c| c & 1 != 0 && (c & !1) == (pc & !1))
    }

    /// Free comparator index.
    pub fn free_comparator(&self) -> Option<usize> {
        self.comp.iter().position(|c| c & 1 == 0)
    }
}

/// Instrumentation trace macrocell; stimulus writes are collected for the host.
#[derive(Clone, Debug, Default)]
pub struct Itm {
    pub ter: u32,
    pub tpr: u32,
    pub tcr: u32,
    pub out: Vec<(u8, u32, u8)>,
}

/// Performance monitoring unit: a cycle counter and 8 event counters that count cycles
/// (event 0x0011) or retired instructions (event 0x0008).
#[derive(Clone, Debug, Default)]
pub struct Pmu {
    pub ctrl: u32,
    pub cntenset: u32,
    pub intenset: u32,
    pub ovsset: u32,
    pub evtyper: [u32; 8],
    pub evcntr_base: [u32; 8],
    pub ccntr_base: u64,
    /// Cycle and instruction counts at the last base update.
    origin_cycles: u64,
    origin_instret: u64,
    running: bool,
}

impl Pmu {
    fn enabled(&self) -> bool {
        self.ctrl & 1 != 0
    }

    pub fn ccntr(&self, cycles: u64) -> u64 {
        if self.running && self.cntenset & (1 << 31) != 0 {
            self.ccntr_base.wrapping_add(cycles - self.origin_cycles)
        } else {
            self.ccntr_base
        }
    }

    pub fn evcntr(&self, i: usize, cycles: u64, instret: u64) -> u32 {
        if !self.running || self.cntenset & (1 << i) == 0 {
            return self.evcntr_base[i] & 0xFFFF;
        }
        let d = match self.evtyper[i] & 0xFFFF {
            0x0011 => cycles - self.origin_cycles,
            0x0008 => instret - self.origin_instret,
            _ => 0,
        };
        (self.evcntr_base[i] as u64 + d) as u32 & 0xFFFF
    }

    /// Freezes the counters at the current values (before a change of configuration).
    pub fn sync(&mut self, cycles: u64, instret: u64) {
        self.ccntr_base = self.ccntr(cycles);
        for i in 0..8 {
            self.evcntr_base[i] = self.evcntr(i, cycles, instret);
        }
        self.origin_cycles = cycles;
        self.origin_instret = instret;
        self.running = self.enabled();
    }
}

const SCS_PIDR: [u32; 12] = [0x04, 0, 0, 0, 0x0C, 0xB0, 0x0B, 0x00, 0x0D, 0xE0, 0x05, 0xB1];
const DWT_PIDR: [u32; 12] = [0x04, 0, 0, 0, 0x02, 0xB0, 0x3B, 0x00, 0x0D, 0xE0, 0x05, 0xB1];
const FPB_PIDR: [u32; 12] = [0x04, 0, 0, 0, 0x03, 0xB0, 0x2B, 0x00, 0x0D, 0xE0, 0x05, 0xB1];
const ITM_PIDR: [u32; 12] = [0x04, 0, 0, 0, 0x01, 0xB0, 0x3B, 0x00, 0x0D, 0xE0, 0x05, 0xB1];
const PMU_PIDR: [u32; 12] = [0x04, 0, 0, 0, 0x04, 0xB0, 0x6B, 0x00, 0x0D, 0xE0, 0x05, 0xB1];
const TPIU_PIDR: [u32; 12] = [0x04, 0, 0, 0, 0xA1, 0xB9, 0x0B, 0x00, 0x0D, 0x90, 0x05, 0xB1];
const ROM_PIDR: [u32; 12] = [0x04, 0, 0, 0, 0xC4, 0xB4, 0x0B, 0x00, 0x0D, 0x10, 0x05, 0xB1];

/// Values of the SCB identification registers of a Cortex-M85 (to be compared with the FVP).
pub const CPUID: u32 = 0x411F_D230;
const ID_REGS: [(u32, u32); 17] = [
    (0xED40, 0x0000_0030), // ID_PFR0
    (0xED44, 0x0000_0520), // ID_PFR1
    (0xED48, 0x0020_0000), // ID_DFR0
    (0xED4C, 0x0000_0000), // ID_AFR0
    (0xED50, 0x0010_1F40), // ID_MMFR0
    (0xED54, 0x0000_0000), // ID_MMFR1
    (0xED58, 0x0100_0000), // ID_MMFR2
    (0xED5C, 0x0000_0000), // ID_MMFR3
    (0xED60, 0x0809_2400), // ID_ISAR0
    (0xED64, 0x0211_2000), // ID_ISAR1
    (0xED68, 0x2023_2232), // ID_ISAR2
    (0xED6C, 0x0111_1131), // ID_ISAR3
    (0xED70, 0x0131_0132), // ID_ISAR4
    (0xED74, 0x0000_0000), // ID_ISAR5
    (0xED78, 0x0900_0003), // CLIDR
    (0xED7C, 0x8303_C003), // CTR
    (0xED90, 0x0000_0800), // MPU_TYPE
];
pub const MVFR0: u32 = 0x1011_0221;
pub const MVFR1: u32 = 0x1210_0211;
pub const MVFR2: u32 = 0x0000_0040;

fn id_reg(table: &[u32; 12], off: u32) -> u32 {
    if (0xFD0..=0xFFC).contains(&off) {
        table[((off - 0xFD0) / 4) as usize]
    } else {
        0
    }
}

fn bus_fault(addr: u32) -> Trap {
    Trap::Bus(CFSR_PRECISERR, Some(addr))
}

/// Which security view an SCS access uses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Secure,
    NonSecure,
}

impl Cpu {
    /// Read of the private peripheral bus. `debug` reads have no side effects.
    pub fn ppb_read(&mut self, addr: u32, size: u8, debug: bool) -> Result<u32, Trap> {
        let word_addr = addr & !3;
        if !debug && !self.privileged() && !(0xE000_0000..0xE000_1000).contains(&addr) {
            return Err(bus_fault(addr));
        }
        let w = self.ppb_read_word(word_addr, debug)?;
        let shift = (addr & 3) * 8;
        Ok(match size {
            1 => (w >> shift) & 0xFF,
            2 => (w >> shift) & 0xFFFF,
            _ => w,
        })
    }

    /// Write to the private peripheral bus.
    pub fn ppb_write(&mut self, addr: u32, size: u8, value: u32, debug: bool) -> Result<(), Trap> {
        let word_addr = addr & !3;
        let unpriv_ok = (0xE000_0000..0xE000_1000).contains(&addr) || (addr & !0x0002_0000 == 0xE000_EF00 && self.ccr[self.st()] & 2 != 0);
        if !debug && !self.privileged() && !unpriv_ok {
            return Err(bus_fault(addr));
        }
        let shift = (addr & 3) * 8;
        let (val, mask) = match size {
            1 => ((value & 0xFF) << shift, 0xFFu32 << shift),
            2 => ((value & 0xFFFF) << shift, 0xFFFFu32 << shift),
            _ => (value, 0xFFFF_FFFF),
        };
        self.ppb_write_word(word_addr, val, mask, debug)
    }

    /// Resolves the SCS view of an address: the SCS at 0xE000_E000 is seen as the current
    /// state; the alias at 0xE002_E000 is the Non-secure view for Secure code.
    fn scs_view(&self, a: u32, debug: bool) -> Result<Option<(u32, View)>, Trap> {
        if (0xE000_E000..0xE000_F000).contains(&a) {
            let v = if self.secure { View::Secure } else { View::NonSecure };
            return Ok(Some((a - 0xE000_E000, v)));
        }
        if (0xE002_E000..0xE002_F000).contains(&a) {
            if !self.secure && !debug {
                return Err(bus_fault(a));
            }
            return Ok(Some((a - 0xE002_E000, View::NonSecure)));
        }
        Ok(None)
    }

    fn nvic_word(&self, set: &ExcSet, word: u32) -> u32 {
        let i = word as usize;
        let mut v = 0u32;
        for b in 0..32 {
            let irq = i * 32 + b;
            if irq < NUM_IRQ && set_has(set, irq) {
                v |= 1 << b;
            }
        }
        v
    }

    fn nvic_pending_word(&self, word: u32) -> u32 {
        let i = word as usize;
        let mut v = 0u32;
        for b in 0..32 {
            let irq = i * 32 + b;
            if irq < NUM_IRQ && set_has(&self.pending, 16 + irq) {
                v |= 1 << b;
            }
        }
        v
    }

    fn nvic_active_word(&self, word: u32) -> u32 {
        let i = word as usize;
        let mut v = 0u32;
        for b in 0..32 {
            let irq = i * 32 + b;
            if irq < NUM_IRQ && set_has(&self.active, 16 + irq) {
                v |= 1 << b;
            }
        }
        v
    }

    /// IRQs visible in a view: Secure sees all, Non-secure only those that target it.
    fn nvic_view_mask(&self, word: u32, view: View) -> u32 {
        match view {
            View::Secure => !0,
            View::NonSecure => self.nvic_word(&self.itns, word),
        }
    }

    fn ppb_read_word(&mut self, a: u32, debug: bool) -> Result<u32, Trap> {
        let now = self.now;
        if let Some((off, view)) = self.scs_view(a, debug)? {
            return self.scs_read(off, view, debug);
        }
        Ok(match a {
            // ITM
            0xE000_0000..=0xE000_007C => 1,
            0xE000_0E00 => self.itm.ter,
            0xE000_0E40 => self.itm.tpr,
            0xE000_0E80 => self.itm.tcr,
            0xE000_0FB4 => 0,
            0xE000_0FD0..=0xE000_0FFC => id_reg(&ITM_PIDR, a - 0xE000_0000),
            // DWT
            0xE000_1000 => self.dwt.ctrl,
            0xE000_1004 => self.dwt.cyccnt(now),
            0xE000_1008 => self.dwt.cpicnt,
            0xE000_100C => self.dwt.exccnt,
            0xE000_1010 => self.dwt.sleepcnt,
            0xE000_1014 => self.dwt.lsucnt,
            0xE000_1018 => self.dwt.foldcnt,
            0xE000_101C => self.pc,
            0xE000_1020..=0xE000_105C => {
                let i = ((a - 0xE000_1020) / 16) as usize;
                match (a - 0xE000_1020) % 16 {
                    0 => self.dwt.comp[i],
                    8 => {
                        let f = self.dwt.function[i];
                        if !debug {
                            self.dwt.function[i] &= !(1 << 24);
                        }
                        f | (0b1011 << 27)
                    }
                    _ => 0,
                }
            }
            0xE000_1FBC => 0x47701A02,
            0xE000_1FD0..=0xE000_1FFC => id_reg(&DWT_PIDR, a - 0xE000_1000),
            // FPB
            0xE000_2000 => self.fpb.ctrl | (8 << 4) | (1 << 28),
            0xE000_2004 => 0,
            0xE000_2008..=0xE000_2024 => self.fpb.comp[((a - 0xE000_2008) / 4) as usize],
            0xE000_2FBC => 0x47701A03,
            0xE000_2FD0..=0xE000_2FFC => id_reg(&FPB_PIDR, a - 0xE000_2000),
            // PMU
            0xE000_3000..=0xE000_301C => {
                let i = ((a - 0xE000_3000) / 4) as usize;
                self.pmu.evcntr(i, now / TICKS_PER_CPU_CYCLE, self.instret)
            }
            0xE000_37E0 => self.pmu.ccntr(now / TICKS_PER_CPU_CYCLE) as u32,
            0xE000_37E4 => (self.pmu.ccntr(now / TICKS_PER_CPU_CYCLE) >> 32) as u32,
            0xE000_3400..=0xE000_341C => self.pmu.evtyper[((a - 0xE000_3400) / 4) as usize],
            0xE000_3C00 | 0xE000_3C20 => self.pmu.cntenset,
            0xE000_3C40 | 0xE000_3C60 => self.pmu.intenset,
            0xE000_3C80 | 0xE000_3CC0 => self.pmu.ovsset,
            0xE000_3E00 => 0x0000_0408 | (1 << 23),
            0xE000_3E04 => self.pmu.ctrl,
            0xE000_3FB8 => 0,
            0xE000_3FBC => 0x47702A06,
            0xE000_3FCC => 0x16,
            0xE000_3FD0..=0xE000_3FFC => id_reg(&PMU_PIDR, a - 0xE000_3000),
            // Cortex-M85 processor-specific registers
            0xE001_E000 => self.mscr,
            0xE001_E004 => self.pfcr,
            0xE001_E010 => 0x0000_0007 | (0x6 << 3), // ITCMCR: enabled, 32 KB
            0xE001_E014 => 0x0000_0007 | (0x6 << 3), // DTCMCR
            0xE001_E018 => 0,
            0xE001_E300 => self.cpdlpstate,
            0xE001_E304 => self.dpdlpstate,
            0xE001_E500..=0xE001_E6FF => 0,
            // TPIU
            0xE004_0000 => 0xF,
            0xE004_0004 => 1,
            0xE004_0FC8 => 0xCA0,
            0xE004_0FD0..=0xE004_0FFC => id_reg(&TPIU_PIDR, a - 0xE004_0000),
            // ROM table
            0xE00F_F000 => 0xFFF0_F003,
            0xE00F_F004 => 0xFFF0_2003,
            0xE00F_F008 => 0xFFF0_3003,
            0xE00F_F00C => 0xFFF0_1003,
            0xE00F_F010 => 0xFFF0_4003,
            0xE00F_F014 => 0xFFF4_1003,
            0xE00F_F018 => 0xFFF4_2002,
            0xE00F_FFCC => 1,
            0xE00F_FFD0..=0xE00F_FFFC => id_reg(&ROM_PIDR, a - 0xE00F_F000),
            _ => 0,
        })
    }

    fn scs_read(&mut self, off: u32, view: View, debug: bool) -> Result<u32, Trap> {
        let now = self.now;
        let secure_view = view == View::Secure;
        let i = !secure_view as usize;
        Ok(match off {
            0x004 => (NUM_IRQ as u32).div_ceil(32) - 1,
            0x008 => self.actlr,
            0x00C => self.cppwr,
            0x010 => {
                let v = self.systick[i].ctrl | ((self.systick[i].countflag as u32) << 16);
                if !debug {
                    self.systick[i].countflag = false;
                }
                v
            }
            0x014 => self.systick[i].load,
            0x018 => self.systick[i].value(now),
            0x01C => self.systick[i].calib,
            0x100..=0x13C => self.nvic_word(&self.irq_enabled, (off - 0x100) / 4) & self.nvic_view_mask((off - 0x100) / 4, view),
            0x180..=0x1BC => self.nvic_word(&self.irq_enabled, (off - 0x180) / 4) & self.nvic_view_mask((off - 0x180) / 4, view),
            0x200..=0x23C => self.nvic_pending_word((off - 0x200) / 4) & self.nvic_view_mask((off - 0x200) / 4, view),
            0x280..=0x2BC => self.nvic_pending_word((off - 0x280) / 4) & self.nvic_view_mask((off - 0x280) / 4, view),
            0x300..=0x33C => self.nvic_active_word((off - 0x300) / 4) & self.nvic_view_mask((off - 0x300) / 4, view),
            0x380..=0x3BC => {
                if secure_view {
                    self.nvic_word(&self.itns, (off - 0x380) / 4)
                } else {
                    0
                }
            }
            0x400..=0x5DC => {
                let base = (off - 0x400) as usize;
                let mut v = 0;
                for k in 0..4 {
                    let irq = base + k;
                    if irq < NUM_IRQ && (secure_view || set_has(&self.itns, irq)) {
                        v |= (self.prio_irq[irq] as u32) << (8 * k);
                    }
                }
                v
            }
            0xD00 => CPUID,
            0xD04 => self.icsr(view),
            0xD08 => self.vtor[i],
            0xD0C => {
                0xFA05_0000
                    | (self.prigroup[i] << 8)
                    | ((self.bfhfnmins as u32) << 13)
                    | ((self.pris as u32) << 14)
                    | ((self.sysresetreqs as u32) << 3)
            }
            0xD10 => self.scr[i],
            0xD14 => self.ccr[i],
            0xD18..=0xD20 => {
                let base = 4 + (off - 0xD18) as usize;
                let mut v = 0;
                for k in 0..4 {
                    let n = base + k;
                    let p = match n {
                        EXC_BUSFAULT | EXC_SECUREFAULT | EXC_DEBUGMON => {
                            if secure_view || (n == EXC_BUSFAULT && self.bfhfnmins) {
                                self.prio_sys[S][n]
                            } else {
                                0
                            }
                        }
                        _ => self.prio_sys[i][n],
                    };
                    v |= (p as u32) << (8 * k);
                }
                v
            }
            0xD24 => self.shcsr(view),
            0xD28 => {
                let mut v = self.mmfsr[i] as u32 | ((self.ufsr[i] as u32) << 16);
                if secure_view || self.bfhfnmins {
                    v |= (self.bfsr as u32) << 8;
                }
                v
            }
            0xD2C => {
                if secure_view || self.bfhfnmins {
                    self.hfsr
                } else {
                    0
                }
            }
            0xD30 => self.dfsr,
            0xD34 => self.mmfar[i],
            0xD38 => {
                if secure_view || self.bfhfnmins {
                    self.bfar
                } else {
                    0
                }
            }
            0xD3C => self.afsr,
            0xD80 => match self.csselr & 1 {
                0 => 0xF01F_E019,
                _ => 0x201F_E019,
            },
            0xD84 => self.csselr,
            0xD88 => self.cpacr[i],
            0xD8C => {
                if secure_view {
                    self.nsacr
                } else {
                    0
                }
            }
            0xD94 => self.mpu[i].ctrl,
            0xD98 => self.mpu[i].rnr,
            0xD9C | 0xDA4 | 0xDAC | 0xDB4 => {
                let r = ((self.mpu[i].rnr & 7) as usize + ((off - 0xD9C) / 8) as usize) & 7;
                self.mpu[i].rbar[r]
            }
            0xDA0 | 0xDA8 | 0xDB0 | 0xDB8 => {
                let r = ((self.mpu[i].rnr & 7) as usize + ((off - 0xDA0) / 8) as usize) & 7;
                self.mpu[i].rlar[r]
            }
            0xDC0 => self.mpu[i].mair[0],
            0xDC4 => self.mpu[i].mair[1],
            0xDD0 => {
                if secure_view {
                    self.sau.ctrl
                } else {
                    0
                }
            }
            0xDD4 => {
                if secure_view {
                    8
                } else {
                    0
                }
            }
            0xDD8 => {
                if secure_view {
                    self.sau.rnr
                } else {
                    0
                }
            }
            0xDDC => {
                if secure_view {
                    self.sau.rbar[(self.sau.rnr & 7) as usize]
                } else {
                    0
                }
            }
            0xDE0 => {
                if secure_view {
                    self.sau.rlar[(self.sau.rnr & 7) as usize]
                } else {
                    0
                }
            }
            0xDE4 => {
                if secure_view {
                    self.sfsr
                } else {
                    0
                }
            }
            0xDE8 => {
                if secure_view {
                    self.sfar
                } else {
                    0
                }
            }
            0xDF0 => self.dhcsr(),
            0xDF4 => 0,
            0xDF8 => self.dcrdr,
            0xDFC => self.demcr,
            0xE04 => self.dauthctrl,
            0xE08 => self.dscsr | ((self.secure as u32) << 16),
            0xF04 => 0,
            0xF34 => {
                let shared = self.fpccr[S] & 0xFC00_0000;
                if secure_view {
                    shared | (self.fpccr[S] & 0x03FF_FFFF)
                } else {
                    (shared & !(1 << 26)) | (self.fpccr[NS] & 0x03FF_FFFF & !(1 << 7))
                }
            }
            0xF38 => self.fpcar[i],
            0xF3C => self.fpdscr[i],
            0xF40 => MVFR0,
            0xF44 => MVFR1,
            0xF48 => MVFR2,
            0xFBC => 0x47702A04,
            0xFD0..=0xFFC => id_reg(&SCS_PIDR, off),
            _ => ID_REGS.iter().find(|(o, _)| *o - 0xE000 == off).map(|(_, v)| *v).unwrap_or(0),
        })
    }

    fn icsr(&self, view: View) -> u32 {
        let secure_view = view == View::Secure;
        let i = !secure_view as usize;
        let vectpending = self.vect_pending();
        let mut isrpending = false;
        for w in 0..8 {
            let p = (self.pending[w] >> 16) | if w < 7 { self.pending[w + 1] << 48 } else { 0 };
            if p & self.irq_enabled[w] != 0 || self.irq_lines[w] & self.irq_enabled[w] != 0 {
                isrpending = true;
            }
        }
        let active_count = set_iter(&self.active).count();
        let vectactive = self.ipsr & 0x1FF;
        let rettobase = vectactive != 0 && active_count == 1;
        let p = |slot: usize| set_has(&self.pending, slot) as u32;
        (p(EXC_NMI) << 31)
            | (p(Self::sys_slot(EXC_PENDSV, secure_view)) << 28)
            | (p(Self::sys_slot(EXC_SYSTICK, secure_view)) << 26)
            | ((isrpending as u32) << 22)
            | ((vectpending & 0x1FF) << 12)
            | ((rettobase as u32) << 11)
            | vectactive
            | if i == S && !self.bfhfnmins { 0 } else { 0 }
    }

    fn shcsr(&self, view: View) -> u32 {
        let secure_view = view == View::Secure;
        let i = !secure_view as usize;
        let a = |slot: usize| set_has(&self.active, slot) as u32;
        let p = |slot: usize| set_has(&self.pending, slot) as u32;
        let s = |n: usize| Self::sys_slot(n, secure_view);
        let mut v = self.shcsr_ena[i] & ((1 << 16) | (1 << 18));
        if secure_view {
            v |= self.shcsr_ena[S] & ((1 << 17) | (1 << 19));
            v |= p(EXC_SECUREFAULT) << 20 | a(EXC_SECUREFAULT) << 4;
        }
        if secure_view || self.bfhfnmins {
            v |= p(EXC_BUSFAULT) << 14 | a(EXC_BUSFAULT) << 1 | a(EXC_NMI) << 5 | a(EXC_HARDFAULT) << 2;
        }
        v | p(s(EXC_SVCALL)) << 15
            | p(s(EXC_MEMMANAGE)) << 13
            | p(s(EXC_USAGEFAULT)) << 12
            | a(s(EXC_SYSTICK)) << 11
            | a(s(EXC_PENDSV)) << 10
            | a(EXC_DEBUGMON) << 8
            | a(s(EXC_SVCALL)) << 7
            | a(s(EXC_USAGEFAULT)) << 3
            | a(s(EXC_MEMMANAGE))
    }

    pub fn dhcsr(&self) -> u32 {
        ((self.s_reset_st as u32) << 25)
            | ((self.s_retire_st as u32) << 24)
            | (1 << 20) // S_SDE
            | ((self.lockup as u32) << 19)
            | ((self.sleeping as u32) << 18)
            | ((self.halted.is_some() as u32) << 17)
            | (1 << 16)
            | ((self.c_pmov as u32) << 6)
            | ((self.c_maskints as u32) << 3)
            | ((self.c_step as u32) << 2)
            | ((self.c_halt as u32) << 1)
            | self.c_debugen as u32
    }

    fn ppb_write_word(&mut self, a: u32, v: u32, mask: u32, debug: bool) -> Result<(), Trap> {
        let now = self.now;
        if let Some((off, view)) = self.scs_view(a, debug)? {
            return self.scs_write(off, view, v, mask, debug);
        }
        match a {
            0xE000_0000..=0xE000_007C => {
                let port = ((a - 0xE000_0000) / 4) as u8;
                if self.itm.tcr & 1 != 0 && self.itm.ter & (1 << port) != 0 && self.demcr & (1 << 24) != 0 {
                    let size = mask.count_ones() / 8;
                    let shift = mask.trailing_zeros();
                    self.itm.out.push((port, v >> shift, size as u8));
                }
            }
            0xE000_0E00 => self.itm.ter = merge(self.itm.ter, v, mask),
            0xE000_0E40 => self.itm.tpr = merge(self.itm.tpr, v, mask) & 0xF,
            0xE000_0E80 => self.itm.tcr = merge(self.itm.tcr, v, mask) & 0x00FF_FF1F,
            0xE000_1000 => {
                self.dwt.ctrl = (self.dwt.ctrl & 0xF000_0000) | (merge(self.dwt.ctrl, v, mask) & 0x007F_1FFF);
                let trcena = self.demcr & (1 << 24) != 0;
                self.dwt.update_running(now, trcena);
            }
            0xE000_1004 => {
                let v = merge(self.dwt.cyccnt(now), v, mask);
                self.dwt.set_cyccnt(now, v);
            }
            0xE000_1008 => self.dwt.cpicnt = v & 0xFF,
            0xE000_100C => self.dwt.exccnt = v & 0xFF,
            0xE000_1010 => self.dwt.sleepcnt = v & 0xFF,
            0xE000_1014 => self.dwt.lsucnt = v & 0xFF,
            0xE000_1018 => self.dwt.foldcnt = v & 0xFF,
            0xE000_1020..=0xE000_105C => {
                let i = ((a - 0xE000_1020) / 16) as usize;
                match (a - 0xE000_1020) % 16 {
                    0 => self.dwt.comp[i] = merge(self.dwt.comp[i], v, mask),
                    8 => self.dwt.function[i] = (self.dwt.function[i] & (1 << 24)) | (merge(self.dwt.function[i], v, mask) & 0x0000_0C3F),
                    _ => {}
                }
            }
            0xE000_2000 if v & 2 != 0 => {
                self.fpb.ctrl = v & 1;
            }
            0xE000_2008..=0xE000_2024 => {
                let i = ((a - 0xE000_2008) / 4) as usize;
                self.fpb.comp[i] = merge(self.fpb.comp[i], v, mask) & 0xFFFF_FFFF;
            }
            0xE000_3000..=0xE000_301C => {
                let i = ((a - 0xE000_3000) / 4) as usize;
                self.pmu.sync(now / TICKS_PER_CPU_CYCLE, self.instret);
                self.pmu.evcntr_base[i] = v & 0xFFFF;
            }
            0xE000_37E0 => {
                self.pmu.sync(now / TICKS_PER_CPU_CYCLE, self.instret);
                self.pmu.ccntr_base = (self.pmu.ccntr_base & 0xFFFF_FFFF_0000_0000) | v as u64;
            }
            0xE000_37E4 => {
                self.pmu.sync(now / TICKS_PER_CPU_CYCLE, self.instret);
                self.pmu.ccntr_base = (self.pmu.ccntr_base & 0xFFFF_FFFF) | ((v as u64) << 32);
            }
            0xE000_3400..=0xE000_341C => {
                self.pmu.sync(now / TICKS_PER_CPU_CYCLE, self.instret);
                self.pmu.evtyper[((a - 0xE000_3400) / 4) as usize] = v & 0xFFFF;
            }
            0xE000_3C00 => {
                self.pmu.sync(now / TICKS_PER_CPU_CYCLE, self.instret);
                self.pmu.cntenset |= v & 0x8000_00FF;
            }
            0xE000_3C20 => {
                self.pmu.sync(now / TICKS_PER_CPU_CYCLE, self.instret);
                self.pmu.cntenset &= !v;
            }
            0xE000_3C40 => self.pmu.intenset |= v & 0x8000_00FF,
            0xE000_3C60 => self.pmu.intenset &= !v,
            0xE000_3C80 => self.pmu.ovsset &= !v,
            0xE000_3CC0 => self.pmu.ovsset |= v & 0x8000_00FF,
            0xE000_3CA0 => {
                self.pmu.sync(now / TICKS_PER_CPU_CYCLE, self.instret);
                for i in 0..8 {
                    if v & (1 << i) != 0 && self.pmu.evtyper[i] & 0xFFFF == 0 {
                        self.pmu.evcntr_base[i] = self.pmu.evcntr_base[i].wrapping_add(1) & 0xFFFF;
                    }
                }
            }
            0xE000_3E04 => {
                self.pmu.sync(now / TICKS_PER_CPU_CYCLE, self.instret);
                self.pmu.ctrl = v & 0x1;
                if v & 2 != 0 {
                    self.pmu.evcntr_base = [0; 8];
                }
                if v & 4 != 0 {
                    self.pmu.ccntr_base = 0;
                }
                self.pmu.sync(now / TICKS_PER_CPU_CYCLE, self.instret);
            }
            0xE001_E000 => self.mscr = merge(self.mscr, v, mask),
            0xE001_E004 => self.pfcr = merge(self.pfcr, v, mask),
            0xE001_E300 => self.cpdlpstate = merge(self.cpdlpstate, v, mask) & 0x333,
            0xE001_E304 => self.dpdlpstate = merge(self.dpdlpstate, v, mask) & 0x3,
            _ => {}
        }
        Ok(())
    }

    fn scs_write(&mut self, off: u32, view: View, v: u32, mask: u32, debug: bool) -> Result<(), Trap> {
        let now = self.now;
        let full = mask == 0xFFFF_FFFF;
        let secure_view = view == View::Secure;
        let i = !secure_view as usize;
        match off {
            0x008 => self.actlr = merge(self.actlr, v, mask) & 0x3FF,
            0x00C => self.cppwr = merge(self.cppwr, v, mask),
            0x010 => {
                let nv = merge(self.systick[i].ctrl, v, mask);
                self.systick[i].write_ctrl(now, nv);
            }
            0x014 => self.systick[i].load = merge(self.systick[i].load, v, mask) & 0x00FF_FFFF,
            0x018 => self.systick[i].write_val(now),
            0x100..=0x13C | 0x180..=0x1BC | 0x200..=0x23C | 0x280..=0x2BC => {
                let kind = (off >> 7) & 7; // 2 ISER, 3 ICER, 4 ISPR, 5 ICPR
                let word = ((off & 0x7F) / 4) as usize;
                let bits = v & mask & self.nvic_view_mask(word as u32, view);
                for b in 0..32 {
                    if bits & (1 << b) == 0 {
                        continue;
                    }
                    let irq = word * 32 + b;
                    if irq >= NUM_IRQ {
                        continue;
                    }
                    match kind {
                        2 => set_add(&mut self.irq_enabled, irq),
                        3 => set_remove(&mut self.irq_enabled, irq),
                        4 => set_add(&mut self.pending, 16 + irq),
                        _ => self.clear_pending(16 + irq),
                    }
                }
                self.mark_exc_dirty();
            }
            0x380..=0x3BC if secure_view => {
                let word = ((off - 0x380) / 4) as usize;
                for b in 0..32 {
                    if mask & (1 << b) == 0 {
                        continue;
                    }
                    let irq = word * 32 + b;
                    if irq < NUM_IRQ {
                        if v & (1 << b) != 0 {
                            set_add(&mut self.itns, irq);
                        } else {
                            set_remove(&mut self.itns, irq);
                        }
                    }
                }
                self.mark_exc_dirty();
            }
            0x400..=0x5DC => {
                let base = (off - 0x400) as usize;
                for k in 0..4 {
                    let irq = base + k;
                    if mask & (0xFF << (8 * k)) != 0 && irq < NUM_IRQ && (secure_view || set_has(&self.itns, irq)) {
                        self.prio_irq[irq] = ((v >> (8 * k)) & 0xE0) as u8;
                    }
                }
                self.mark_exc_dirty();
            }
            0xD04 => {
                let v = v & mask;
                if v & (1 << 31) != 0 && (secure_view || self.bfhfnmins) {
                    self.set_pending(EXC_NMI);
                }
                if v & (1 << 28) != 0 {
                    self.set_pending(Self::sys_slot(EXC_PENDSV, secure_view));
                } else if v & (1 << 27) != 0 {
                    self.clear_pending(Self::sys_slot(EXC_PENDSV, secure_view));
                }
                if v & (1 << 26) != 0 {
                    self.set_pending(Self::sys_slot(EXC_SYSTICK, secure_view));
                } else if v & (1 << 25) != 0 {
                    self.clear_pending(Self::sys_slot(EXC_SYSTICK, secure_view));
                }
            }
            0xD08 => self.vtor[i] = merge(self.vtor[i], v, mask) & 0xFFFF_FF80,
            0xD0C if full && v >> 16 == 0x05FA => {
                self.prigroup[i] = (v >> 8) & 7;
                if secure_view {
                    self.bfhfnmins = v & (1 << 13) != 0;
                    self.pris = v & (1 << 14) != 0;
                    self.sysresetreqs = v & (1 << 3) != 0;
                }
                self.mark_exc_dirty();
                if v & 4 != 0 && (secure_view || !self.sysresetreqs) {
                    self.reset_request = Some(crate::bus::ResetRequest::System);
                }
                if v & 2 != 0 && self.halted.is_some() {
                    // VECTCLRACTIVE (debug only)
                    self.active = [0; 8];
                    self.ipsr = 0;
                }
            }
            0xD10 => self.scr[i] = merge(self.scr[i], v, mask) & 0x1E,
            0xD14 => self.ccr[i] = (merge(self.ccr[i], v, mask) & 0x000F_071B) | 0x200,
            0xD18..=0xD20 => {
                let base = 4 + (off - 0xD18) as usize;
                for k in 0..4 {
                    let n = base + k;
                    if mask & (0xFF << (8 * k)) == 0 {
                        continue;
                    }
                    let p = ((v >> (8 * k)) & 0xE0) as u8;
                    match n {
                        EXC_MEMMANAGE | EXC_USAGEFAULT | EXC_SVCALL | EXC_PENDSV | EXC_SYSTICK => self.prio_sys[i][n] = p,
                        EXC_BUSFAULT | EXC_SECUREFAULT | EXC_DEBUGMON => {
                            if secure_view || (n == EXC_BUSFAULT && self.bfhfnmins) {
                                self.prio_sys[S][n] = p;
                            }
                        }
                        _ => {}
                    }
                }
                self.mark_exc_dirty();
            }
            0xD24 => {
                let v = merge(self.shcsr(view), v, mask);
                self.shcsr_ena[i] = (self.shcsr_ena[i] & !((1 << 16) | (1 << 18))) | (v & ((1 << 16) | (1 << 18)));
                if secure_view {
                    self.shcsr_ena[S] = (self.shcsr_ena[S] & !((1 << 17) | (1 << 19))) | (v & ((1 << 17) | (1 << 19)));
                }
                let s = |n: usize| Self::sys_slot(n, secure_view);
                let set = |cpu: &mut Self, slot: usize, on: bool| {
                    if on {
                        set_add(&mut cpu.pending, slot);
                    } else {
                        set_remove(&mut cpu.pending, slot);
                    }
                };
                set(self, s(EXC_SVCALL), v & (1 << 15) != 0);
                set(self, s(EXC_MEMMANAGE), v & (1 << 13) != 0);
                set(self, s(EXC_USAGEFAULT), v & (1 << 12) != 0);
                if secure_view || self.bfhfnmins {
                    set(self, EXC_BUSFAULT, v & (1 << 14) != 0);
                }
                if secure_view {
                    set(self, EXC_SECUREFAULT, v & (1 << 20) != 0);
                }
                let act = |cpu: &mut Self, slot: usize, on: bool| {
                    if on {
                        set_add(&mut cpu.active, slot);
                    } else {
                        set_remove(&mut cpu.active, slot);
                    }
                };
                act(self, s(EXC_SYSTICK), v & (1 << 11) != 0);
                act(self, s(EXC_PENDSV), v & (1 << 10) != 0);
                act(self, EXC_DEBUGMON, v & (1 << 8) != 0);
                act(self, s(EXC_SVCALL), v & (1 << 7) != 0);
                act(self, s(EXC_USAGEFAULT), v & (1 << 3) != 0);
                act(self, s(EXC_MEMMANAGE), v & 1 != 0);
                if secure_view || self.bfhfnmins {
                    act(self, EXC_BUSFAULT, v & (1 << 1) != 0);
                }
                if secure_view {
                    act(self, EXC_SECUREFAULT, v & (1 << 4) != 0);
                }
                self.mark_exc_dirty();
            }
            0xD28 => {
                let c = v & mask;
                self.mmfsr[i] &= !(c as u8);
                self.ufsr[i] &= !((c >> 16) as u16);
                if secure_view || self.bfhfnmins {
                    self.bfsr &= !((c >> 8) as u8);
                }
            }
            0xD2C => {
                if secure_view || self.bfhfnmins {
                    self.hfsr &= !(v & mask);
                }
            }
            0xD30 => self.dfsr &= !(v & mask),
            0xD34 => self.mmfar[i] = merge(self.mmfar[i], v, mask),
            0xD38 => {
                if secure_view || self.bfhfnmins {
                    self.bfar = merge(self.bfar, v, mask);
                }
            }
            0xD3C => self.afsr &= !(v & mask),
            0xD84 => self.csselr = merge(self.csselr, v, mask) & 0xF,
            0xD88 => {
                let nv = merge(self.cpacr[i], v, mask) & 0x00F0_00FF;
                self.cpacr[i] = nv;
            }
            0xD8C if secure_view => self.nsacr = merge(self.nsacr, v, mask) & 0x0C00_00FF,
            0xD94 => self.mpu[i].ctrl = merge(self.mpu[i].ctrl, v, mask) & 7,
            0xD98 => self.mpu[i].rnr = merge(self.mpu[i].rnr, v, mask) & 7,
            0xD9C | 0xDA4 | 0xDAC | 0xDB4 => {
                let r = ((self.mpu[i].rnr & 7) as usize + ((off - 0xD9C) / 8) as usize) & 7;
                self.mpu[i].rbar[r] = merge(self.mpu[i].rbar[r], v, mask) & 0xFFFF_FFFF;
            }
            0xDA0 | 0xDA8 | 0xDB0 | 0xDB8 => {
                let r = ((self.mpu[i].rnr & 7) as usize + ((off - 0xDA0) / 8) as usize) & 7;
                self.mpu[i].rlar[r] = merge(self.mpu[i].rlar[r], v, mask) & 0xFFFF_FFEF;
            }
            0xDC0 => self.mpu[i].mair[0] = merge(self.mpu[i].mair[0], v, mask),
            0xDC4 => self.mpu[i].mair[1] = merge(self.mpu[i].mair[1], v, mask),
            0xDD0 if secure_view => {
                self.sau.ctrl = merge(self.sau.ctrl, v, mask) & 3;
                self.sau_changed();
            }
            0xDD8 if secure_view => self.sau.rnr = merge(self.sau.rnr, v, mask) & 7,
            0xDDC if secure_view => {
                let r = (self.sau.rnr & 7) as usize;
                self.sau.rbar[r] = merge(self.sau.rbar[r], v, mask) & 0xFFFF_FFE0;
            }
            0xDE0 if secure_view => {
                let r = (self.sau.rnr & 7) as usize;
                self.sau.rlar[r] = merge(self.sau.rlar[r], v, mask) & 0xFFFF_FFE3;
            }
            0xDE4 if secure_view => self.sfsr &= !(v & mask),
            0xDE8 if secure_view => self.sfar = merge(self.sfar, v, mask),
            0xDF0 => self.write_dhcsr(v, mask, debug),
            0xDF4 if debug && self.halted.is_some() => {
                let sel = v & 0x7F;
                if v & (1 << 16) != 0 {
                    let d = self.dcrdr;
                    self.write_core_reg(sel, d);
                } else {
                    self.dcrdr = self.read_core_reg(sel).unwrap_or(0);
                }
            }
            0xDF8 => self.dcrdr = merge(self.dcrdr, v, mask),
            0xDFC => {
                let nv = merge(self.demcr, v, mask);
                self.demcr = if secure_view { nv & 0x011F_0FF1 } else { (self.demcr & 0x0010_0F01) | (nv & 0x010F_00F0) };
                let trcena = self.demcr & (1 << 24) != 0;
                self.dwt.update_running(now, trcena);
            }
            0xE04 if debug => self.dauthctrl = merge(self.dauthctrl, v, mask) & 0xFF,
            0xE08 if debug => {
                if v & (1 << 1) != 0 {
                    self.dscsr = (self.dscsr & !1) | (v & 1);
                }
                if v & (1 << 17) != 0 && self.halted.is_some() {
                    // CDSKEY: change the current security state
                    let ipsr = self.ipsr;
                    let spsel = self.spsel[self.st()];
                    self.set_stack_state(ipsr, v & (1 << 16) != 0, spsel);
                }
            }
            0xF00 => {
                let irq = (v & mask & 0x1FF) as usize;
                if irq < NUM_IRQ && (secure_view || set_has(&self.itns, irq)) {
                    self.pend_irq(irq);
                }
            }
            0xF34 => {
                let cur = self.fpccr[i];
                let nv = merge(cur, v, mask);
                if secure_view {
                    self.fpccr[S] = (nv & 0xFC00_0000) | (nv & 0x0000_01FB);
                } else {
                    self.fpccr[NS] = (nv & 0x0000_017B) | (self.fpccr[NS] & 0xFC00_0000);
                    // ASPEN and LSPEN are shared (writable from Non-secure unless LSPENS)
                    if self.fpccr[S] & (1 << 29) == 0 {
                        self.fpccr[S] = (self.fpccr[S] & !(1 << 30)) | (nv & (1 << 30));
                    }
                    self.fpccr[S] = (self.fpccr[S] & !(1 << 31)) | (nv & (1 << 31));
                }
            }
            0xF38 => self.fpcar[i] = merge(self.fpcar[i], v, mask) & !7,
            0xF3C => self.fpdscr[i] = merge(self.fpdscr[i], v, mask) & 0x07C0_0000,
            _ => {}
        }
        Ok(())
    }

    fn write_dhcsr(&mut self, v: u32, mask: u32, debug: bool) {
        if mask != 0xFFFF_FFFF || v >> 16 != 0xA05F {
            return;
        }
        if debug {
            self.c_debugen = v & 1 != 0;
        }
        if !self.c_debugen {
            return;
        }
        self.c_maskints = v & 8 != 0;
        self.c_step = v & 4 != 0;
        self.c_pmov = v & 0x40 != 0;
        let halt = v & 2 != 0;
        if halt {
            self.c_halt = true;
        } else if self.halted.is_some() {
            self.resume();
        } else {
            self.c_halt = false;
        }
        self.mark_exc_dirty();
        if debug {
            // reading DHCSR clears the sticky S_RESET_ST and S_RETIRE_ST bits
            self.s_reset_st = false;
            self.s_retire_st = false;
        }
    }

    /// FPSCR write with the writable mask.
    pub fn set_fpscr(&mut self, v: u32) {
        self.fpscr = v & FPSCR_MASK;
    }
}

#[inline]
fn merge(old: u32, new: u32, mask: u32) -> u32 {
    (old & !mask) | (new & mask)
}
