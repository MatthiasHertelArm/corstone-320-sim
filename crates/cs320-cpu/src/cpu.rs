//! Core state, the exception model of Armv8.1-M Mainline with the Security Extension
//! (DDI 0553B chapter B3) and the run loop.

use crate::bus::{Bus, BusError, Idau, IrqLines, ResetRequest};
use crate::decode::{decode, is_32bit, Instr};
use crate::scs::{Dwt, Fpb, Itm, Mpu, Pmu, Sau, SysTick};
use cs320_core::time::{ceil_cpu_cycle, TICKS_PER_CPU_CYCLE};
use cs320_core::Ticks;

/// Number of external interrupts of the SSE-320.
pub const NUM_IRQ: usize = 256;
/// Number of exceptions (16 system exceptions + IRQs).
pub const NUM_EXC: usize = 16 + NUM_IRQ;
/// Exception slots: 0..NUM_EXC are the exception numbers (the Secure instance of a banked
/// system exception); `NS_SLOT_BASE + n` is the Non-secure instance of banked exception `n`.
pub const NS_SLOT_BASE: usize = NUM_EXC;
pub const NUM_SLOTS: usize = 512;

pub const EXC_RESET: usize = 1;
pub const EXC_NMI: usize = 2;
pub const EXC_HARDFAULT: usize = 3;
pub const EXC_MEMMANAGE: usize = 4;
pub const EXC_BUSFAULT: usize = 5;
pub const EXC_USAGEFAULT: usize = 6;
pub const EXC_SECUREFAULT: usize = 7;
pub const EXC_SVCALL: usize = 11;
pub const EXC_DEBUGMON: usize = 12;
pub const EXC_PENDSV: usize = 14;
pub const EXC_SYSTICK: usize = 15;

/// Banked system exceptions (one instance per security state).
pub const fn is_banked(n: usize) -> bool {
    matches!(n, EXC_MEMMANAGE | EXC_USAGEFAULT | EXC_SVCALL | EXC_PENDSV | EXC_SYSTICK)
}

// CFSR bits
pub const CFSR_IACCVIOL: u32 = 1 << 0;
pub const CFSR_DACCVIOL: u32 = 1 << 1;
pub const CFSR_MUNSTKERR: u32 = 1 << 3;
pub const CFSR_MSTKERR: u32 = 1 << 4;
pub const CFSR_MLSPERR: u32 = 1 << 5;
pub const CFSR_MMARVALID: u32 = 1 << 7;
pub const CFSR_IBUSERR: u32 = 1 << 8;
pub const CFSR_PRECISERR: u32 = 1 << 9;
pub const CFSR_IMPRECISERR: u32 = 1 << 10;
pub const CFSR_UNSTKERR: u32 = 1 << 11;
pub const CFSR_STKERR: u32 = 1 << 12;
pub const CFSR_LSPERR: u32 = 1 << 13;
pub const CFSR_BFARVALID: u32 = 1 << 15;
pub const CFSR_UNDEFINSTR: u32 = 1 << 16;
pub const CFSR_INVSTATE: u32 = 1 << 17;
pub const CFSR_INVPC: u32 = 1 << 18;
pub const CFSR_NOCP: u32 = 1 << 19;
pub const CFSR_STKOF: u32 = 1 << 20;
pub const CFSR_UNALIGNED: u32 = 1 << 24;
pub const CFSR_DIVBYZERO: u32 = 1 << 25;

// SFSR bits
pub const SFSR_INVEP: u32 = 1 << 0;
pub const SFSR_INVIS: u32 = 1 << 1;
pub const SFSR_INVER: u32 = 1 << 2;
pub const SFSR_AUVIOL: u32 = 1 << 3;
pub const SFSR_INVTRAN: u32 = 1 << 4;
pub const SFSR_LSPERR: u32 = 1 << 5;
pub const SFSR_SFARVALID: u32 = 1 << 6;
pub const SFSR_LSERR: u32 = 1 << 7;

pub const HFSR_VECTTBL: u32 = 1 << 1;
pub const HFSR_FORCED: u32 = 1 << 30;
pub const HFSR_DEBUGEVT: u32 = 1 << 31;

pub const DFSR_HALTED: u32 = 1 << 0;
pub const DFSR_BKPT: u32 = 1 << 1;
pub const DFSR_DWTTRAP: u32 = 1 << 2;
pub const DFSR_VCATCH: u32 = 1 << 3;
pub const DFSR_EXTERNAL: u32 = 1 << 4;
pub const DFSR_PMU: u32 = 1 << 5;

/// Index of the Secure and the Non-secure copy of a banked register.
pub const S: usize = 0;
pub const NS: usize = 1;

/// A synchronous fault or another reason to leave normal execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trap {
    /// UsageFault with the CFSR bit.
    Usage(u32),
    /// BusFault with CFSR bits and the fault address (precise errors).
    Bus(u32, Option<u32>),
    /// MemManage fault with CFSR bits and the fault address.
    Mem(u32, Option<u32>),
    /// SecureFault with SFSR bits and the fault address.
    Secure(u32, Option<u32>),
    Bkpt(u8),
    Svc,
    /// An instruction the simulator does not implement (MVE phase 2): halts with the encoding.
    Unimplemented(u32),
}

/// Why the core halted in debug state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HaltReason {
    /// Halt request from the debugger (C_HALT).
    Request,
    /// Single step completed.
    Step,
    /// BKPT instruction.
    Bkpt(u8),
    /// FPB comparator or debugger breakpoint.
    Breakpoint,
    /// DWT watchpoint: address and access kind (0 = read, 1 = write).
    Watchpoint(u32, u8),
    /// Vector catch (reset or fault).
    VectorCatch,
    Lockup,
    /// An instruction the simulator does not implement (the encoding).
    Unimplemented(u32),
}

/// Result of [`Cpu::run`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// The time limit or the bus deadline was reached.
    Deadline,
    /// The core sleeps (WFI/WFE) until an event.
    Sleeping,
    /// The core halted in debug state.
    Halted(HaltReason),
    /// The core requests a system reset.
    Reset(ResetRequest),
}

/// A data watchpoint set by the debugger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Watch {
    pub addr: u32,
    pub len: u32,
    /// bit 0 = read, bit 1 = write
    pub kind: u8,
}

/// A set of exception slots.
pub type ExcSet = [u64; 8];

#[inline]
pub fn set_has(s: &ExcSet, i: usize) -> bool {
    s[i / 64] & (1 << (i % 64)) != 0
}

#[inline]
pub fn set_add(s: &mut ExcSet, i: usize) {
    s[i / 64] |= 1 << (i % 64);
}

#[inline]
pub fn set_remove(s: &mut ExcSet, i: usize) {
    s[i / 64] &= !(1 << (i % 64));
}

#[inline]
pub fn set_empty(s: &ExcSet) -> bool {
    s.iter().all(|w| *w == 0)
}

pub fn set_iter(s: &ExcSet) -> impl Iterator<Item = usize> + '_ {
    s.iter().enumerate().flat_map(|(w, bits)| {
        let mut b = *bits;
        std::iter::from_fn(move || {
            if b == 0 {
                None
            } else {
                let i = b.trailing_zeros() as usize;
                b &= b - 1;
                Some(w * 64 + i)
            }
        })
    })
}

/// A decoded page of code (4 KB: 2048 halfwords).
pub struct DecodePage {
    pub ins: Box<[Instr; 2048]>,
    pub size: Box<[u8; 2048]>,
}

/// Core state of a Cortex-M85.
pub struct Cpu {
    /// R0..R14; R13 holds the active stack pointer.
    pub r: [u32; 16],
    /// Address of the next instruction to execute.
    pub pc: u32,
    /// The three stack pointers that are not active: index = security * 2 + psp.
    pub sp_bank: [u32; 4],
    pub msplim: [u32; 2],
    pub psplim: [u32; 2],
    pub n: bool,
    pub z: bool,
    pub c: bool,
    pub v: bool,
    pub q: bool,
    pub ge: u8,
    /// ITSTATE (EPSR.IT).
    pub it: u8,
    /// EPSR.T
    pub t: bool,
    /// IPSR exception number.
    pub ipsr: u32,
    /// Current security state.
    pub secure: bool,
    pub primask: [bool; 2],
    pub faultmask: [bool; 2],
    pub basepri: [u8; 2],
    pub npriv: [bool; 2],
    pub spsel: [bool; 2],
    pub fpca: bool,
    pub sfpa: bool,
    /// Floating-point registers S0..S31 (D0..D15, Q0..Q7).
    pub s: [u32; 32],
    pub fpscr: u32,
    /// MVE vector predication status and control.
    pub vpr: u32,
    /// Low-overhead loop: LO_BRANCH_INFO valid (the loop end branch is cached).
    pub lo_branch_valid: bool,

    /// Current time; advanced by the executed instructions.
    pub now: Ticks,
    /// Retired instructions.
    pub instret: u64,

    // exception state
    pub active: ExcSet,
    pub pending: ExcSet,
    /// NVIC enables of the external interrupts (bit n = IRQ n).
    pub irq_enabled: ExcSet,
    /// Interrupt line levels from the device.
    pub irq_lines: ExcSet,
    /// NVIC_ITNS: interrupts that target Non-secure state.
    pub itns: ExcSet,
    /// Priority of the external interrupts.
    pub prio_irq: [u8; NUM_IRQ],
    /// Priority of the system exceptions 4..15, per security state.
    pub prio_sys: [[u8; 16]; 2],
    pub(crate) exc_dirty: bool,
    exc_ready: Option<usize>,

    // SCB (index S / NS where banked)
    pub vtor: [u32; 2],
    pub prigroup: [u32; 2],
    pub bfhfnmins: bool,
    pub pris: bool,
    pub sysresetreqs: bool,
    pub scr: [u32; 2],
    pub ccr: [u32; 2],
    /// SHCSR enable bits: MEMFAULTENA and USGFAULTENA per state; BUSFAULTENA and
    /// SECUREFAULTENA in the Secure copy.
    pub shcsr_ena: [u32; 2],
    pub mmfsr: [u8; 2],
    pub ufsr: [u16; 2],
    pub bfsr: u8,
    pub hfsr: u32,
    pub dfsr: u32,
    pub mmfar: [u32; 2],
    pub bfar: u32,
    pub sfsr: u32,
    pub sfar: u32,
    pub afsr: u32,
    pub actlr: u32,
    pub cppwr: u32,
    pub cpacr: [u32; 2],
    pub nsacr: u32,
    pub csselr: u32,
    pub fpccr: [u32; 2],
    pub fpcar: [u32; 2],
    pub fpdscr: [u32; 2],
    pub cpdlpstate: u32,
    pub dpdlpstate: u32,
    pub mscr: u32,
    pub pfcr: u32,

    pub systick: [SysTick; 2],
    pub mpu: [Mpu; 2],
    pub sau: Sau,
    /// True while the SAU is disabled with ALLNS = 0 (every address is Secure): no
    /// attribution needed for Secure accesses.
    sau_all_secure: bool,
    pub dwt: Dwt,
    pub fpb: Fpb,
    pub itm: Itm,
    pub pmu: Pmu,

    // debug
    pub c_debugen: bool,
    pub c_halt: bool,
    pub c_step: bool,
    pub c_maskints: bool,
    pub c_pmov: bool,
    pub demcr: u32,
    pub dcrdr: u32,
    pub dauthctrl: u32,
    pub dscsr: u32,
    pub s_retire_st: bool,
    pub s_reset_st: bool,
    pub halted: Option<HaltReason>,
    pub lockup: bool,
    /// Breakpoints of the debugger in addition to the FPB (unlimited).
    pub sw_breakpoints: Vec<u32>,
    /// Watchpoints of the debugger in addition to the DWT.
    pub watchpoints: Vec<Watch>,
    pub(crate) watch_hit: Option<(u32, u8)>,
    pub(crate) watch_active: bool,
    /// Skip breakpoint checks for the first instruction after a resume.
    pub(crate) skip_bp: bool,
    /// Extra debugger-requested step: halt after one instruction.
    pub step_request: bool,
    pub sleeping: bool,
    pub(crate) sleep_wfe: bool,
    pub event_reg: bool,
    pub(crate) excl: bool,
    pub(crate) next_pc: u32,
    pub(crate) exc_return: Option<u32>,
    /// A function return through FNC_RETURN (BXNS) to execute after the instruction.
    pub(crate) fnc_return: Option<u32>,
    /// A stack pointer write below the stack limit happened in this instruction.
    pub(crate) stkof: bool,
    /// PAC keys (PAC_KEY_P/U, the Secure and Non-secure sets share storage per index).
    pub pac_key: [u32; 4],
    /// PACBTI extension implemented (`CFGPACBTI`): PACG/AUTG/BXAUT are UNDEFINED without it.
    pub pacbti: bool,
    pub(crate) reset_request: Option<ResetRequest>,
    /// Accumulated wait cycles from the bus for the current instruction.
    pub(crate) extra_cycles: u32,
    /// Number of exceptions taken (statistics).
    pub exceptions_taken: u64,
    /// Decoded instructions per 4 KB page (index = address >> 12).
    icache: Vec<Option<Box<DecodePage>>>,
    icache_gen: u64,
    /// Pages that hold decoded code (for invalidation on stores).
    icache_pages: Vec<u32>,
    /// The current fetch is from a Non-secure-callable address in Non-secure state.
    nsc_fetch: bool,
    /// Number of instructions still to log at trace level (`CS320_TRACE=insn`).
    pub trace_insn: u64,
    /// The instruction just executed was VPT/VPST (no VPT advance after it).
    pub(crate) vpt_set: bool,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    pub fn new() -> Self {
        let mut c = Cpu {
            r: [0; 16],
            pc: 0,
            sp_bank: [0; 4],
            msplim: [0; 2],
            psplim: [0; 2],
            n: false,
            z: false,
            c: false,
            v: false,
            q: false,
            ge: 0,
            it: 0,
            t: true,
            ipsr: 0,
            secure: true,
            primask: [false; 2],
            faultmask: [false; 2],
            basepri: [0; 2],
            npriv: [false; 2],
            spsel: [false; 2],
            fpca: false,
            sfpa: false,
            s: [0; 32],
            fpscr: crate::fpu::FPSCR_RESET,
            vpr: 0,
            lo_branch_valid: false,
            now: 0,
            instret: 0,
            active: [0; 8],
            pending: [0; 8],
            irq_enabled: [0; 8],
            irq_lines: [0; 8],
            itns: [0; 8],
            prio_irq: [0; NUM_IRQ],
            prio_sys: [[0; 16]; 2],
            exc_dirty: true,
            exc_ready: None,
            vtor: [0; 2],
            prigroup: [0; 2],
            bfhfnmins: false,
            pris: false,
            sysresetreqs: false,
            scr: [0; 2],
            ccr: [0x201; 2],
            shcsr_ena: [0; 2],
            mmfsr: [0; 2],
            ufsr: [0; 2],
            bfsr: 0,
            hfsr: 0,
            dfsr: 0,
            mmfar: [0; 2],
            bfar: 0,
            sfsr: 0,
            sfar: 0,
            afsr: 0,
            actlr: 0,
            cppwr: 0,
            cpacr: [0; 2],
            nsacr: 0,
            csselr: 0,
            fpccr: [0xC000_0004, 0xC000_0000],
            fpcar: [0; 2],
            fpdscr: [crate::fpu::FPSCR_RESET; 2],
            cpdlpstate: 0,
            dpdlpstate: 0,
            mscr: 0x3000,
            pfcr: 1,
            systick: [SysTick::default(), SysTick::default()],
            mpu: [Mpu::default(), Mpu::default()],
            sau: Sau::default(),
            sau_all_secure: true,
            dwt: Dwt::default(),
            fpb: Fpb::default(),
            itm: Itm::default(),
            pmu: Pmu::default(),
            c_debugen: false,
            c_halt: false,
            c_step: false,
            c_maskints: false,
            c_pmov: false,
            demcr: 0,
            dcrdr: 0,
            dauthctrl: 0,
            dscsr: 0,
            s_retire_st: false,
            s_reset_st: false,
            halted: None,
            lockup: false,
            sw_breakpoints: Vec::new(),
            watchpoints: Vec::new(),
            watch_hit: None,
            watch_active: false,
            skip_bp: false,
            step_request: false,
            sleeping: false,
            sleep_wfe: false,
            event_reg: false,
            excl: false,
            next_pc: 0,
            exc_return: None,
            fnc_return: None,
            stkof: false,
            pac_key: [0; 4],
            pacbti: false,
            reset_request: None,
            extra_cycles: 0,
            exceptions_taken: 0,
            icache: Vec::new(),
            icache_gen: u64::MAX,
            icache_pages: Vec::new(),
            nsc_fetch: false,
            trace_insn: if std::env::var("CS320_TRACE").is_ok_and(|v| v.contains("insn")) { 2_000_000 } else { 0 },
            vpt_set: false,
        };
        c.power_on_reset_debug();
        c
    }

    /// Resets the debug components (power-on reset only).
    fn power_on_reset_debug(&mut self) {
        self.c_debugen = false;
        self.c_halt = false;
        self.c_step = false;
        self.c_maskints = false;
        self.c_pmov = false;
        self.demcr = 0;
        self.dauthctrl = 0;
        self.dscsr = 0;
        self.dwt = Dwt::default();
        self.fpb = Fpb::default();
        self.itm = Itm::default();
        self.pmu = Pmu::default();
    }

    /// Cold reset: core, NVIC, SCB and debug components.
    pub fn power_on_reset<B: Bus>(&mut self, bus: &mut B) {
        self.power_on_reset_debug();
        self.reset(bus);
    }

    /// Local (system) reset: core, NVIC and SCB. The debug components keep their state, and
    /// with DEMCR.VC_CORERESET the core halts before the first instruction.
    pub fn reset<B: Bus>(&mut self, bus: &mut B) {
        self.r = [0; 16];
        self.r[14] = 0xFFFF_FFFF;
        self.sp_bank = [0; 4];
        self.msplim = [0; 2];
        self.psplim = [0; 2];
        self.n = false;
        self.z = false;
        self.c = false;
        self.v = false;
        self.q = false;
        self.ge = 0;
        self.it = 0;
        self.ipsr = 0;
        self.secure = true;
        self.primask = [false; 2];
        self.faultmask = [false; 2];
        self.basepri = [0; 2];
        self.npriv = [false; 2];
        self.spsel = [false; 2];
        self.fpca = false;
        self.sfpa = false;
        self.s = [0; 32];
        self.fpscr = crate::fpu::FPSCR_RESET;
        self.vpr = 0;
        self.lo_branch_valid = false;
        self.active = [0; 8];
        self.pending = [0; 8];
        self.irq_enabled = [0; 8];
        self.itns = [0; 8];
        self.prio_irq = [0; NUM_IRQ];
        self.prio_sys = [[0; 16]; 2];
        self.vtor = [bus.reset_vector_base(), 0];
        self.prigroup = [0; 2];
        self.bfhfnmins = false;
        self.pris = false;
        self.sysresetreqs = false;
        self.scr = [0; 2];
        self.ccr = [0x201; 2];
        self.shcsr_ena = [0; 2];
        self.mmfsr = [0; 2];
        self.ufsr = [0; 2];
        self.bfsr = 0;
        self.hfsr = 0;
        self.mmfar = [0; 2];
        self.bfar = 0;
        self.sfsr = 0;
        self.sfar = 0;
        self.afsr = 0;
        self.actlr = 0;
        self.cppwr = 0;
        self.cpacr = [0; 2];
        self.nsacr = 0;
        self.csselr = 0;
        self.fpccr = [0xC000_0004, 0xC000_0000];
        self.fpcar = [0; 2];
        self.fpdscr = [crate::fpu::FPSCR_RESET; 2];
        self.cpdlpstate = 0;
        self.dpdlpstate = 0;
        self.mscr = 0x3000;
        self.pfcr = 1;
        self.systick = [SysTick::default(), SysTick::default()];
        self.mpu = [Mpu::default(), Mpu::default()];
        self.sau = Sau::default();
        self.sau_all_secure = true;
        self.lockup = false;
        self.sleeping = false;
        self.sleep_wfe = false;
        self.event_reg = false;
        self.excl = false;
        self.exc_return = None;
        self.fnc_return = None;
        self.stkof = false;
        self.reset_request = None;
        self.exc_dirty = true;
        self.s_reset_st = true;
        self.now = ceil_cpu_cycle(self.now);
        let base = self.vtor[S];
        let sp = bus.read(self.now, base, 4, false, true).unwrap_or(0);
        let rv = bus.read(self.now, base.wrapping_add(4), 4, false, true).unwrap_or(0);
        self.r[13] = sp & !3;
        self.pc = rv & !1;
        self.t = rv & 1 == 1;
        if self.c_debugen && self.demcr & 1 != 0 {
            self.halted = Some(HaltReason::VectorCatch);
            self.dfsr |= DFSR_VCATCH;
        }
    }

    // ---------------------------------------------------------------- registers

    /// Index of the banked register copies of the current state.
    #[inline]
    pub fn st(&self) -> usize {
        if self.secure {
            S
        } else {
            NS
        }
    }

    #[inline]
    pub fn handler_mode(&self) -> bool {
        self.ipsr != 0
    }

    #[inline]
    pub fn privileged(&self) -> bool {
        self.handler_mode() || !self.npriv[self.st()]
    }

    #[inline]
    fn using_psp(&self) -> bool {
        !self.handler_mode() && self.spsel[self.st()]
    }

    /// Index into `sp_bank` of a stack pointer.
    #[inline]
    fn sp_slot(secure: bool, psp: bool) -> usize {
        (!secure as usize) * 2 + psp as usize
    }

    #[inline]
    fn cur_sp_slot(&self) -> usize {
        Self::sp_slot(self.secure, self.using_psp())
    }

    /// Reads one of the four stack pointers.
    pub fn sp_of(&self, secure: bool, psp: bool) -> u32 {
        let slot = Self::sp_slot(secure, psp);
        if slot == self.cur_sp_slot() {
            self.r[13]
        } else {
            self.sp_bank[slot]
        }
    }

    pub fn set_sp_of(&mut self, secure: bool, psp: bool, v: u32) {
        let slot = Self::sp_slot(secure, psp);
        if slot == self.cur_sp_slot() {
            self.r[13] = v & !3;
        } else {
            self.sp_bank[slot] = v & !3;
        }
    }

    pub fn msp(&self) -> u32 {
        self.sp_of(self.secure, false)
    }

    pub fn psp(&self) -> u32 {
        self.sp_of(self.secure, true)
    }

    pub fn set_msp(&mut self, v: u32) {
        self.set_sp_of(self.secure, false, v);
    }

    pub fn set_psp(&mut self, v: u32) {
        self.set_sp_of(self.secure, true, v);
    }

    /// Stack limit of the active stack pointer.
    pub fn cur_sp_limit(&self) -> u32 {
        if self.using_psp() {
            self.psplim[self.st()]
        } else {
            self.msplim[self.st()]
        }
    }

    /// Changes the mode/stack/state selection and swaps the banked SP when it changes.
    pub(crate) fn set_stack_state(&mut self, ipsr: u32, secure: bool, spsel: bool) {
        let before = self.cur_sp_slot();
        self.ipsr = ipsr;
        self.secure = secure;
        self.spsel[self.st()] = spsel;
        let after = self.cur_sp_slot();
        if after != before {
            self.sp_bank[before] = self.r[13];
            self.r[13] = self.sp_bank[after];
        }
    }

    pub fn apsr(&self) -> u32 {
        ((self.n as u32) << 31)
            | ((self.z as u32) << 30)
            | ((self.c as u32) << 29)
            | ((self.v as u32) << 28)
            | ((self.q as u32) << 27)
            | ((self.ge as u32) << 16)
    }

    pub fn set_apsr_nzcvq(&mut self, v: u32) {
        self.n = v >> 31 & 1 != 0;
        self.z = v >> 30 & 1 != 0;
        self.c = v >> 29 & 1 != 0;
        self.v = v >> 28 & 1 != 0;
        self.q = v >> 27 & 1 != 0;
    }

    pub fn epsr(&self) -> u32 {
        ((self.t as u32) << 24) | (((self.it & 3) as u32) << 25) | (((self.it >> 2) as u32) << 10)
    }

    /// The combined xPSR.
    pub fn xpsr(&self) -> u32 {
        self.apsr() | self.epsr() | (self.ipsr & 0x1FF)
    }

    /// Writes the xPSR as a debugger does (all fields).
    pub fn set_xpsr(&mut self, v: u32) {
        self.set_apsr_nzcvq(v);
        self.ge = ((v >> 16) & 0xF) as u8;
        self.t = (v >> 24) & 1 != 0;
        self.it = ((((v >> 10) & 0x3F) << 2) | ((v >> 25) & 3)) as u8;
        let ipsr = v & 0x1FF;
        let (secure, spsel) = (self.secure, self.spsel[self.st()]);
        self.set_stack_state(ipsr, secure, spsel);
        self.exc_dirty = true;
    }

    pub fn control(&self) -> u32 {
        self.control_of(self.secure)
    }

    pub fn control_of(&self, secure: bool) -> u32 {
        let i = !secure as usize;
        (self.npriv[i] as u32)
            | ((self.spsel[i] as u32) << 1)
            | ((self.fpca as u32) << 2)
            | if secure { (self.sfpa as u32) << 3 } else { 0 }
    }

    pub fn set_control(&mut self, v: u32) {
        let secure = self.secure;
        self.set_control_of(secure, v);
    }

    pub fn set_control_of(&mut self, secure: bool, v: u32) {
        let i = !secure as usize;
        self.npriv[i] = v & 1 != 0;
        self.fpca = v & 4 != 0;
        if secure {
            self.sfpa = v & 8 != 0;
        }
        if secure == self.secure {
            let ipsr = self.ipsr;
            self.set_stack_state(ipsr, secure, v & 2 != 0);
        } else {
            self.spsel[i] = v & 2 != 0;
        }
    }

    pub fn set_primask(&mut self, v: bool) {
        self.primask[self.st()] = v;
        self.exc_dirty = true;
    }

    pub fn set_faultmask(&mut self, v: bool) {
        self.faultmask[self.st()] = v;
        self.exc_dirty = true;
    }

    pub fn set_basepri(&mut self, v: u8) {
        self.basepri[self.st()] = v & 0xE0;
        self.exc_dirty = true;
    }

    /// Reads a core register by its DCRSR number (Armv8-M): 0-12, 13 SP, 14 LR, 15 PC,
    /// 16 xPSR, 17 MSP, 18 PSP, 20 CONTROL/FAULTMASK/BASEPRI/PRIMASK, 24 MSP_NS, 25 PSP_NS,
    /// 26 MSP_S, 27 PSP_S, 28 MSPLIM_S, 29 PSPLIM_S, 30 MSPLIM_NS, 31 PSPLIM_NS, 33 FPSCR,
    /// 34 CONTROL_S..., 35 CONTROL_NS..., 36 VPR, 64..95 S0..S31.
    pub fn read_core_reg(&self, sel: u32) -> Option<u32> {
        let special = |secure: bool| {
            let i = !secure as usize;
            (self.control_of(secure) << 24) | ((self.faultmask[i] as u32) << 16) | ((self.basepri[i] as u32) << 8) | self.primask[i] as u32
        };
        Some(match sel {
            0..=12 | 14 => self.r[sel as usize],
            13 => self.r[13],
            15 => self.pc,
            16 => self.xpsr(),
            17 => self.msp(),
            18 => self.psp(),
            20 => special(self.secure),
            24 => self.sp_of(false, false),
            25 => self.sp_of(false, true),
            26 => self.sp_of(true, false),
            27 => self.sp_of(true, true),
            28 => self.msplim[S],
            29 => self.psplim[S],
            30 => self.msplim[NS],
            31 => self.psplim[NS],
            33 => self.fpscr,
            34 => special(true),
            35 => special(false),
            36 => self.vpr,
            64..=95 => self.s[(sel - 64) as usize],
            _ => return None,
        })
    }

    pub fn write_core_reg(&mut self, sel: u32, v: u32) -> bool {
        let special = |cpu: &mut Self, secure: bool, v: u32| {
            let i = !secure as usize;
            cpu.primask[i] = v & 1 != 0;
            cpu.basepri[i] = ((v >> 8) as u8) & 0xE0;
            cpu.faultmask[i] = (v >> 16) & 1 != 0;
            cpu.set_control_of(secure, (v >> 24) & 0xF);
            cpu.exc_dirty = true;
        };
        match sel {
            0..=12 | 14 => self.r[sel as usize] = v,
            13 => self.r[13] = v & !3,
            15 => {
                self.pc = v & !1;
                self.skip_bp = false;
            }
            16 => self.set_xpsr(v),
            17 => self.set_msp(v),
            18 => self.set_psp(v),
            20 => {
                let s = self.secure;
                special(self, s, v)
            }
            24 => self.set_sp_of(false, false, v),
            25 => self.set_sp_of(false, true, v),
            26 => self.set_sp_of(true, false, v),
            27 => self.set_sp_of(true, true, v),
            28 => self.msplim[S] = v & !7,
            29 => self.psplim[S] = v & !7,
            30 => self.msplim[NS] = v & !7,
            31 => self.psplim[NS] = v & !7,
            33 => self.fpscr = v & crate::fpu::FPSCR_MASK,
            34 => special(self, true, v),
            35 => special(self, false, v),
            36 => self.vpr = v & 0xFFFF_000F,
            64..=95 => self.s[(sel - 64) as usize] = v,
            _ => return false,
        }
        true
    }

    // ---------------------------------------------------------------- security attribution

    /// Re-evaluates the SAU fast path after a SAU register write.
    pub(crate) fn sau_changed(&mut self) {
        self.sau_all_secure = self.sau.ctrl & 1 == 0 && self.sau.ctrl & 2 == 0;
    }

    /// Security attribute of an address: (secure, non-secure callable).
    pub fn attribution<B: Bus>(&self, bus: &B, addr: u32) -> (bool, bool) {
        match bus.idau(addr) {
            Idau::Exempt => return (self.secure, false),
            Idau::None => return (true, false),
            Idau::NonSecureCallable => {
                let (s, nsc) = self.sau.attribute(addr);
                // the IDAU says NSC: secure unless the SAU also says NS or NSC
                return if !s { (true, true) } else { (true, nsc) };
            }
            Idau::NonSecure => {}
        }
        self.sau.attribute(addr)
    }

    /// Security attribute of a data access from the current state: `Ok(ns)` with the
    /// transaction attribute, or a SecureFault for a Non-secure access to Secure memory.
    #[inline]
    pub(crate) fn access_ns<B: Bus>(&self, bus: &B, addr: u32) -> Result<bool, Trap> {
        if self.secure && self.sau_all_secure {
            return Ok(false);
        }
        let (s, _) = self.attribution(bus, addr);
        if self.secure {
            Ok(!s)
        } else if s {
            Err(Trap::Secure(SFSR_AUVIOL, Some(addr)))
        } else {
            Ok(true)
        }
    }

    // ---------------------------------------------------------------- exceptions

    /// Exception number of a slot.
    #[inline]
    pub fn slot_number(slot: usize) -> usize {
        if slot >= NS_SLOT_BASE {
            slot - NS_SLOT_BASE
        } else {
            slot
        }
    }

    /// The slot of system exception `n` targeting state `secure` (banked exceptions only).
    #[inline]
    pub fn sys_slot(n: usize, secure: bool) -> usize {
        if is_banked(n) && !secure {
            NS_SLOT_BASE + n
        } else {
            n
        }
    }

    /// Whether the exception of a slot targets Secure state.
    pub fn slot_secure(&self, slot: usize) -> bool {
        if slot >= NS_SLOT_BASE {
            return false;
        }
        match slot {
            EXC_NMI | EXC_HARDFAULT | EXC_BUSFAULT => !self.bfhfnmins,
            EXC_DEBUGMON => self.demcr & (1 << 20) != 0,
            n if n >= 16 => !set_has(&self.itns, n - 16),
            _ => true,
        }
    }

    /// Group priority of a priority value under PRIGROUP of a state.
    #[inline]
    fn group(&self, p: i32, secure: bool) -> i32 {
        if p < 0 {
            return p;
        }
        let mask = !((2u32 << self.prigroup[!secure as usize]) - 1) & 0xFF;
        (p as u32 & mask) as i32
    }

    /// PRIS mapping of a Non-secure priority.
    #[inline]
    fn pris_map(&self, p: i32, secure: bool) -> i32 {
        if !secure && self.pris && p >= 0 {
            (p >> 1) | 0x80
        } else {
            p
        }
    }

    /// Raw priority of a slot.
    #[inline]
    fn slot_priority(&self, slot: usize) -> i32 {
        let n = Self::slot_number(slot);
        let secure = self.slot_secure(slot);
        match n {
            EXC_RESET => -4,
            EXC_NMI => -2,
            EXC_HARDFAULT => {
                if secure && !self.bfhfnmins {
                    -3
                } else {
                    -1
                }
            }
            16.. => self.prio_irq[n - 16] as i32,
            _ => self.prio_sys[!secure as usize][n] as i32,
        }
    }

    /// Group priority of a slot with the PRIS mapping (what preemption compares).
    fn effective_priority(&self, slot: usize) -> i32 {
        let secure = self.slot_secure(slot);
        let p = self.slot_priority(slot);
        self.pris_map(self.group(p, secure), secure)
    }

    pub(crate) fn slot_effective_priority_pub(&self, slot: usize) -> i32 {
        self.effective_priority(slot)
    }

    /// `ExecutionPriority()`, optionally ignoring PRIMASK (for WFI wake-up).
    fn execution_priority_masked(&self, with_primask: bool) -> i32 {
        let mut highest = 256;
        for slot in set_iter(&self.active) {
            highest = highest.min(self.effective_priority(slot));
        }
        let mut boosted = 256;
        for (i, secure) in [(S, true), (NS, false)] {
            if self.primask[i] && with_primask {
                boosted = boosted.min(self.pris_map(0, secure));
            }
            if self.basepri[i] != 0 {
                let p = self.group(self.basepri[i] as i32, secure);
                boosted = boosted.min(self.pris_map(p, secure));
            }
            if self.faultmask[i] {
                let p = if secure {
                    if self.bfhfnmins {
                        -1
                    } else {
                        -3
                    }
                } else if self.bfhfnmins {
                    -1
                } else {
                    self.pris_map(0, false)
                };
                boosted = boosted.min(p);
            }
        }
        highest.min(boosted)
    }

    pub fn execution_priority(&self) -> i32 {
        self.execution_priority_masked(true)
    }

    /// The pending exceptions that may be taken, before priority comparison.
    fn pending_candidates(&self) -> ExcSet {
        let mut p = self.pending;
        let mut sys = ExcSet::default();
        for n in 0..16 {
            set_add(&mut sys, n);
            set_add(&mut sys, NS_SLOT_BASE + n);
        }
        for i in 0..8 {
            // slots 0..15 and the NS system slots are always candidates; IRQs need enable
            // (IRQ n sits in slot 16 + n)
            let irq = (self.irq_enabled[i] << 16) | if i > 0 { self.irq_enabled[i - 1] >> 48 } else { 0 };
            p[i] &= sys[i] | irq;
        }
        if self.c_maskints && self.c_debugen {
            // PendSV, SysTick and the interrupts are masked while stepping
            for slot in [EXC_PENDSV, EXC_SYSTICK, NS_SLOT_BASE + EXC_PENDSV, NS_SLOT_BASE + EXC_SYSTICK] {
                set_remove(&mut sys, slot);
            }
            for i in 0..8 {
                p[i] &= sys[i];
            }
        }
        p
    }

    /// Highest-priority pending exception regardless of the execution priority.
    fn highest_pending(&self) -> Option<usize> {
        let p = self.pending_candidates();
        let mut best: Option<(i32, usize)> = None;
        for slot in set_iter(&p) {
            let pr = self.effective_priority(slot);
            let n = Self::slot_number(slot);
            if best.is_none_or(|(bp, bn)| pr < bp || (pr == bp && n < Self::slot_number(bn))) {
                best = Some((pr, slot));
            }
        }
        best.map(|(_, slot)| slot)
    }

    fn compute_ready(&self, with_primask: bool) -> Option<usize> {
        let slot = self.highest_pending()?;
        if self.effective_priority(slot) < self.execution_priority_masked(with_primask) {
            Some(slot)
        } else {
            None
        }
    }

    pub(crate) fn mark_exc_dirty(&mut self) {
        self.exc_dirty = true;
    }

    /// Forces a re-evaluation of the pending exceptions (after debugger changes).
    pub fn mark_dirty(&mut self) {
        self.exc_dirty = true;
    }

    /// The next instruction executes without a breakpoint check.
    pub fn skip_bp_once(&mut self) {
        self.skip_bp = true;
    }

    /// Re-evaluates whether data accesses must be checked for watchpoints.
    pub fn refresh_watch(&mut self) {
        self.watch_active = !self.watchpoints.is_empty() || self.dwt.any_data_watch();
    }

    /// The exception that would be taken now.
    fn ready_exception(&mut self) -> Option<usize> {
        if self.exc_dirty {
            self.exc_dirty = false;
            self.exc_ready = self.compute_ready(true);
        }
        self.exc_ready
    }

    /// Highest pending exception number for ICSR.VECTPENDING (0 if none).
    pub fn vect_pending(&self) -> u32 {
        self.highest_pending().map(Self::slot_number).unwrap_or(0) as u32
    }

    pub fn set_pending(&mut self, slot: usize) {
        set_add(&mut self.pending, slot);
        self.exc_dirty = true;
    }

    pub fn clear_pending(&mut self, slot: usize) {
        set_remove(&mut self.pending, slot);
        // level-sensitive interrupts stay pending while their line is high and they are not active
        if (16..NUM_EXC).contains(&slot) && set_has(&self.irq_lines, slot - 16) && !set_has(&self.active, slot) {
            set_add(&mut self.pending, slot);
        }
        self.exc_dirty = true;
    }

    /// Pends a system exception of the current (or the given) security state.
    pub fn pend_sys(&mut self, n: usize, secure: bool) {
        self.set_pending(Self::sys_slot(n, secure));
    }

    /// New interrupt line levels from the device.
    pub fn set_irq_lines(&mut self, lines: IrqLines) {
        let mut changed = false;
        for (w, &line) in lines.iter().enumerate() {
            let rising = line & !self.irq_lines[w];
            self.irq_lines[w] = line;
            // a high line makes the interrupt pending unless it is active
            let active_w = (self.active[w] >> 16) | if w < 7 { self.active[w + 1] << 48 } else { 0 };
            let p = (line & !active_w) | rising;
            if p != 0 {
                changed = true;
                let mut bits = p;
                while bits != 0 {
                    let i = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    let irq = w * 64 + i;
                    if irq < NUM_IRQ {
                        set_add(&mut self.pending, 16 + irq);
                    }
                }
            }
        }
        if changed {
            self.exc_dirty = true;
        }
    }

    /// Software pend of an IRQ (ISPR/STIR).
    pub fn pend_irq(&mut self, irq: usize) {
        if irq < NUM_IRQ {
            self.set_pending(16 + irq);
        }
    }

    /// Enabled fault handler of the current state for a fault, with its SHCSR bit.
    fn fault_enabled(&self, exc: usize, secure: bool) -> bool {
        match exc {
            EXC_MEMMANAGE => self.shcsr_ena[!secure as usize] & (1 << 16) != 0,
            EXC_BUSFAULT => self.shcsr_ena[S] & (1 << 17) != 0,
            EXC_USAGEFAULT => self.shcsr_ena[!secure as usize] & (1 << 18) != 0,
            EXC_SECUREFAULT => self.shcsr_ena[S] & (1 << 19) != 0,
            _ => true,
        }
    }

    fn write_mem_stack<B: Bus>(&mut self, bus: &mut B, addr: u32, v: u32, ns: bool) -> Result<(), BusError> {
        if (0xE000_0000..0xE010_0000).contains(&addr) {
            return Err(BusError);
        }
        bus.write(self.now, addr, 4, v, ns, false)
    }

    fn read_mem_stack<B: Bus>(&mut self, bus: &mut B, addr: u32, ns: bool) -> Result<u32, BusError> {
        if (0xE000_0000..0xE010_0000).contains(&addr) {
            return Err(BusError);
        }
        bus.read(self.now, addr, 4, ns, false)
    }

    /// `PushStack` + `ExceptionTaken` for the exception in `slot`; `ret` is the return address.
    pub(crate) fn take_exception<B: Bus>(&mut self, bus: &mut B, slot: usize, ret: u32) {
        let _ = Self::slot_number(slot);
        let to_secure = self.slot_secure(slot);
        let from_secure = self.secure;
        let fp_frame = self.fpca;
        // the stack of the state that is interrupted
        let stack_ns = !from_secure;
        let callee = from_secure && !to_secure;
        let fp_callee = callee && fp_frame;
        let mut framesize: u32 = 0x20;
        if fp_frame {
            framesize += 0x48;
        }
        if callee {
            framesize += 0x28;
        }
        if fp_callee {
            framesize += 0x48;
        }
        let sp = self.r[13];
        let align_bit = (sp >> 2) & 1 == 1;
        let frameptr = sp.wrapping_sub(framesize) & !7;
        // stack limit check
        let limit = self.cur_sp_limit();
        if frameptr < limit {
            self.r[13] = limit;
            self.ufsr[self.st()] |= (CFSR_STKOF >> 16) as u16;
            let ufslot = Self::sys_slot(EXC_USAGEFAULT, from_secure);
            let ep = self.execution_priority();
            if self.ccr[self.st()] & (1 << 10) != 0 && ep < 0 {
                // STKOFHFNMIGN: the push is skipped, the handler runs without a frame
            } else if self.fault_enabled(EXC_USAGEFAULT, from_secure) && self.effective_priority(ufslot) < ep && slot != ufslot {
                self.set_pending(slot);
                return self.take_exception_frameless(bus, ufslot);
            } else if ep > -1 {
                self.hfsr |= HFSR_FORCED;
                self.set_pending(slot);
                return self.take_exception_frameless(bus, EXC_HARDFAULT);
            } else {
                return self.enter_lockup("stack overflow on exception entry");
            }
        }
        let xpsr = (self.xpsr() & !(1 << 9)) | ((align_bit as u32) << 9);
        let mut stk_err = false;
        let mut p = frameptr;
        if callee {
            let sig = 0xFEFA_125A | (!fp_frame as u32);
            let vals = [sig, 0, self.r[4], self.r[5], self.r[6], self.r[7], self.r[8], self.r[9], self.r[10], self.r[11]];
            for (i, v) in vals.iter().enumerate() {
                if self.write_mem_stack(bus, p + 4 * i as u32, *v, stack_ns).is_err() {
                    stk_err = true;
                }
            }
            p += 0x28;
        }
        let vals = [self.r[0], self.r[1], self.r[2], self.r[3], self.r[12], self.r[14], ret, xpsr];
        for (i, v) in vals.iter().enumerate() {
            if self.write_mem_stack(bus, p + 4 * i as u32, *v, stack_ns).is_err() {
                stk_err = true;
            }
        }
        p += 0x20;
        if fp_frame {
            let st = self.st();
            if self.fpccr[st] & (1 << 30) == 0 {
                for i in 0..16 {
                    if self.write_mem_stack(bus, p + 4 * i, self.s[i as usize], stack_ns).is_err() {
                        stk_err = true;
                    }
                }
                let (fpscr, vpr) = (self.fpscr, self.vpr);
                if self.write_mem_stack(bus, p + 0x40, fpscr, stack_ns).is_err() {
                    stk_err = true;
                }
                let _ = self.write_mem_stack(bus, p + 0x44, vpr, stack_ns);
                if fp_callee {
                    for i in 16..32 {
                        let _ = self.write_mem_stack(bus, p + 0x48 + 4 * (i - 16), self.s[i as usize], stack_ns);
                    }
                }
            } else {
                self.update_fpccr(p, fp_callee);
            }
        }
        self.r[13] = frameptr;
        let mut lr = 0xFFFF_FF80u32 | (1 << 7) | (1 << 5) | (1 << 4);
        if !self.handler_mode() {
            lr |= 1 << 3;
            if self.spsel[self.st()] {
                lr |= 1 << 2;
            }
        }
        if from_secure {
            lr |= 1 << 6;
        }
        if to_secure {
            lr |= 1;
        }
        if callee {
            lr &= !(1 << 5);
        }
        if fp_frame {
            lr &= !(1 << 4);
        }
        self.r[14] = lr;
        if stk_err {
            self.bfsr |= (CFSR_STKERR >> 8) as u8;
        }
        if callee {
            // registers that are not pushed as part of the basic frame are cleared when the
            // exception targets Non-secure state
            for r in 4..12 {
                self.r[r] = 0;
            }
            self.r[0] = 0;
            self.r[1] = 0;
            self.r[2] = 0;
            self.r[3] = 0;
            self.r[12] = 0;
            self.set_apsr_nzcvq(0);
            self.ge = 0;
            if fp_frame {
                self.s = [0; 32];
                self.fpscr = crate::fpu::FPSCR_RESET;
                self.vpr = 0;
            }
        }
        self.exception_taken(bus, slot);
        if stk_err {
            self.escalate_or_pend(EXC_BUSFAULT, !self.bfhfnmins);
        }
    }

    /// Takes a fault without pushing a frame (after a stack limit violation); the handler
    /// runs with the stack pointer at the limit.
    fn take_exception_frameless<B: Bus>(&mut self, bus: &mut B, slot: usize) {
        let mut lr = 0xFFFF_FF80u32 | (1 << 7) | (1 << 5) | (1 << 4);
        if !self.handler_mode() {
            lr |= 1 << 3;
            if self.spsel[self.st()] {
                lr |= 1 << 2;
            }
        }
        if self.secure {
            lr |= 1 << 6;
        }
        if self.slot_secure(slot) {
            lr |= 1;
        }
        self.r[14] = lr;
        self.exception_taken(bus, slot);
    }

    /// `UpdateFPCCR` for lazy FP stacking.
    fn update_fpccr(&mut self, frameptr: u32, callee_fp: bool) {
        let st = self.st();
        let mut v = self.fpccr[st] & 0xFC00_0000;
        v |= 1; // LSPACT
        if !self.privileged() {
            v |= 1 << 1; // USER
        }
        if self.secure {
            v |= 1 << 2; // S
        }
        if !self.handler_mode() {
            v |= 1 << 3; // THREAD
        }
        let ep = self.execution_priority();
        if ep > -1 {
            v |= 1 << 4; // HFRDY
        }
        if self.fault_enabled(EXC_MEMMANAGE, self.secure) && ep > self.effective_priority(Self::sys_slot(EXC_MEMMANAGE, self.secure)) {
            v |= 1 << 5;
        }
        if self.fault_enabled(EXC_BUSFAULT, true) && ep > self.effective_priority(EXC_BUSFAULT) {
            v |= 1 << 6;
        }
        if self.fault_enabled(EXC_SECUREFAULT, true) && ep > self.effective_priority(EXC_SECUREFAULT) {
            v |= 1 << 7;
        }
        if self.demcr & (1 << 16) != 0 && ep > self.effective_priority(EXC_DEBUGMON) {
            v |= 1 << 8;
        }
        if self.fault_enabled(EXC_USAGEFAULT, self.secure) && ep > self.effective_priority(Self::sys_slot(EXC_USAGEFAULT, self.secure)) {
            v |= 1 << 10; // UFRDY
        }
        if callee_fp {
            v |= 1 << 26; // TS
        }
        self.fpccr[st] = v;
        self.fpcar[st] = frameptr & !7;
    }

    fn exception_taken<B: Bus>(&mut self, bus: &mut B, slot: usize) {
        let n = Self::slot_number(slot);
        let to_secure = self.slot_secure(slot);
        let vaddr = self.vtor[!to_secure as usize].wrapping_add(4 * n as u32);
        let vector = match bus.read(self.now, vaddr, 4, !to_secure, false) {
            Ok(v) => v,
            Err(_) => {
                // vector table read error: HardFault
                self.hfsr |= HFSR_VECTTBL;
                if n == EXC_HARDFAULT || self.execution_priority() <= -1 {
                    self.enter_lockup("vector table read error");
                    return;
                }
                return self.exception_taken(bus, EXC_HARDFAULT);
            }
        };
        if to_secure && vector & 1 == 0 && n != EXC_RESET {
            // a Secure vector must have bit 0 set (INVEP otherwise)
            self.sfsr |= SFSR_INVEP;
            if self.fault_enabled(EXC_SECUREFAULT, true) && n != EXC_SECUREFAULT && n != EXC_HARDFAULT {
                return self.exception_taken(bus, EXC_SECUREFAULT);
            }
            self.hfsr |= HFSR_FORCED;
            if n == EXC_HARDFAULT {
                return self.enter_lockup("Secure vector without bit 0 (INVEP)");
            }
            return self.exception_taken(bus, EXC_HARDFAULT);
        }
        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "exception {n} taken at {:#010x} -> {:#010x} (slot {slot}) ufsr {:#06x} mmfsr {:#04x} bfsr {:#04x} hfsr {:#010x}",
                self.pc,
                vector & !1,
                self.ufsr[self.st()],
                self.mmfsr[self.st()],
                self.bfsr,
                self.hfsr
            );
        }
        self.pc = vector & !1;
        self.t = vector & 1 == 1;
        self.it = 0;
        self.set_stack_state(n as u32, to_secure, false);
        self.fpca = false;
        if to_secure {
            self.sfpa = false;
        }
        set_add(&mut self.active, slot);
        set_remove(&mut self.pending, slot);
        self.excl = false;
        self.event_reg = true;
        self.exc_dirty = true;
        self.exceptions_taken += 1;
        self.extra_cycles += 12;
        self.lo_branch_valid = false;
        if self.c_debugen {
            let vc = match n {
                EXC_HARDFAULT => self.demcr & (1 << 10) != 0,
                EXC_BUSFAULT => self.demcr & (1 << 8) != 0,
                EXC_MEMMANAGE => self.demcr & (1 << 4) != 0,
                EXC_SECUREFAULT => self.demcr & (1 << 11) != 0,
                EXC_USAGEFAULT => {
                    let u = self.ufsr[!to_secure as usize] as u32;
                    (u & 0x7 != 0 && self.demcr & (1 << 7) != 0)
                        || (u & 0x310 != 0 && self.demcr & (1 << 6) != 0)
                        || (u & 0x8 != 0 && self.demcr & (1 << 5) != 0)
                }
                _ => false,
            };
            if vc {
                self.halted = Some(HaltReason::VectorCatch);
                self.dfsr |= DFSR_VCATCH;
            }
        }
    }

    /// Makes `exc` pending when it is enabled and may preempt, otherwise escalates to HardFault.
    fn escalate_or_pend(&mut self, exc: usize, secure: bool) {
        let ep = self.execution_priority();
        let slot = Self::sys_slot(exc, secure);
        if self.fault_enabled(exc, secure) && self.effective_priority(slot) < ep {
            self.set_pending(slot);
        } else if ep > -1 {
            self.hfsr |= HFSR_FORCED;
            self.set_pending(EXC_HARDFAULT);
        } else {
            self.enter_lockup("fault at priority -1 or lower");
        }
    }

    fn enter_lockup(&mut self, why: &str) {
        log::warn!("core lockup at pc {:#010x}: {why}", self.pc);
        self.lockup = true;
        self.pc = 0xEFFF_FFFE;
        self.reset_request = Some(ResetRequest::Lockup);
    }

    /// Handles a synchronous fault of the instruction at `ret`.
    pub(crate) fn raise_fault<B: Bus>(&mut self, bus: &mut B, trap: Trap, ret: u32) {
        let st = self.st();
        let secure = self.secure;
        let (exc, exc_secure) = match trap {
            Trap::Usage(bit) => {
                self.ufsr[st] |= (bit >> 16) as u16;
                (EXC_USAGEFAULT, secure)
            }
            Trap::Bus(bits, addr) => {
                self.bfsr |= (bits >> 8) as u8;
                if let Some(a) = addr {
                    self.bfar = a;
                    self.bfsr |= (CFSR_BFARVALID >> 8) as u8;
                }
                (EXC_BUSFAULT, !self.bfhfnmins)
            }
            Trap::Mem(bits, addr) => {
                self.mmfsr[st] |= bits as u8;
                if let Some(a) = addr {
                    self.mmfar[st] = a;
                    self.mmfsr[st] |= CFSR_MMARVALID as u8;
                }
                (EXC_MEMMANAGE, secure)
            }
            Trap::Secure(bits, addr) => {
                self.sfsr |= bits;
                if let Some(a) = addr {
                    self.sfar = a;
                    self.sfsr |= SFSR_SFARVALID;
                }
                (EXC_SECUREFAULT, true)
            }
            Trap::Bkpt(_) => {
                self.hfsr |= HFSR_DEBUGEVT;
                (EXC_HARDFAULT, !self.bfhfnmins)
            }
            Trap::Svc => (EXC_SVCALL, secure),
            Trap::Unimplemented(enc) => {
                self.halted = Some(HaltReason::Unimplemented(enc));
                return;
            }
        };
        let ep = self.execution_priority();
        let slot = Self::sys_slot(exc, exc_secure);
        let target = if exc != EXC_HARDFAULT && self.fault_enabled(exc, exc_secure) && self.effective_priority(slot) < ep {
            slot
        } else {
            if exc != EXC_HARDFAULT {
                self.hfsr |= HFSR_FORCED;
            }
            EXC_HARDFAULT
        };
        if target == EXC_HARDFAULT && ep <= -1 {
            self.enter_lockup("fault escalation at priority -1 or lower");
            return;
        }
        self.take_exception(bus, target, ret);
    }

    /// `ExceptionReturn(EXC_RETURN)`.
    pub(crate) fn exception_return<B: Bus>(&mut self, bus: &mut B, exc_return: u32) {
        let returning = (self.ipsr & 0x1FF) as usize;
        let cur_secure = self.secure;
        let slot = Self::sys_slot(returning, cur_secure);
        let fault_invpc = |cpu: &mut Self, bus: &mut B| {
            cpu.raise_fault(bus, Trap::Usage(CFSR_INVPC), exc_return);
        };
        let es = exc_return & 1 != 0;
        let dcrs = exc_return & (1 << 5) != 0;
        let ftype = exc_return & (1 << 4) != 0;
        let to_thread = exc_return & (1 << 3) != 0;
        let use_psp = exc_return & (1 << 2) != 0;
        let frame_secure = exc_return & (1 << 6) != 0;
        if exc_return & 0x00FF_FF80 != 0x00FF_FF80 || exc_return & 2 != 0 {
            return fault_invpc(self, bus);
        }
        if es != cur_secure || (!cur_secure && frame_secure) {
            self.raise_fault(bus, Trap::Secure(SFSR_INVER, None), exc_return);
            return;
        }
        if !set_has(&self.active, slot) {
            return fault_invpc(self, bus);
        }
        let nested = set_iter(&self.active).count();
        if to_thread {
            if nested != 1 && self.ccr[self.st()] & 1 == 0 {
                return fault_invpc(self, bus);
            }
        } else if nested == 1 {
            return fault_invpc(self, bus);
        }
        // DeActivate
        set_remove(&mut self.active, slot);
        if returning != EXC_NMI && returning != EXC_HARDFAULT {
            self.faultmask[self.st()] = false;
        }
        if returning >= 16 && set_has(&self.irq_lines, returning - 16) {
            set_add(&mut self.pending, slot);
        }
        // the frame lives on the stack of the returning-to state
        let frameptr = self.sp_of(frame_secure, use_psp);
        let frame_ns = !frame_secure;
        let mut p = frameptr;
        let mut unstk_err = false;
        let mut callee_vals = [0u32; 8];
        if !dcrs {
            if !frame_secure {
                self.raise_fault(bus, Trap::Secure(SFSR_INVER, None), exc_return);
                return;
            }
            let sig = self.read_mem_stack(bus, p, frame_ns).unwrap_or(0);
            if sig != (0xFEFA_125A | ftype as u32) {
                self.raise_fault(bus, Trap::Secure(SFSR_INVIS, None), exc_return);
                return;
            }
            for (i, v) in callee_vals.iter_mut().enumerate() {
                match self.read_mem_stack(bus, p + 8 + 4 * i as u32, frame_ns) {
                    Ok(x) => *v = x,
                    Err(_) => unstk_err = true,
                }
            }
            p += 0x28;
        }
        let mut vals = [0u32; 8];
        for (i, v) in vals.iter_mut().enumerate() {
            match self.read_mem_stack(bus, p.wrapping_add(4 * i as u32), frame_ns) {
                Ok(x) => *v = x,
                Err(_) => unstk_err = true,
            }
        }
        p += 0x20;
        let fst = !frame_secure as usize;
        if !ftype {
            if self.fpccr[fst] & 1 != 0 {
                self.fpccr[fst] &= !1;
            } else {
                for i in 0..16 {
                    if let Ok(x) = self.read_mem_stack(bus, p + 4 * i, frame_ns) {
                        self.s[i as usize] = x;
                    }
                }
                if let Ok(x) = self.read_mem_stack(bus, p + 0x40, frame_ns) {
                    self.fpscr = x & crate::fpu::FPSCR_MASK;
                }
                if let Ok(x) = self.read_mem_stack(bus, p + 0x44, frame_ns) {
                    self.vpr = x & 0xFFFF_000F;
                }
                if !dcrs {
                    for i in 16..32 {
                        if let Ok(x) = self.read_mem_stack(bus, p + 0x48 + 4 * (i - 16), frame_ns) {
                            self.s[i as usize] = x;
                        }
                    }
                }
            }
            p += 0x48;
            if !dcrs {
                p += 0x48;
            }
        }
        let psr = vals[7];
        let spmask = if (psr >> 9) & 1 == 1 { 4 } else { 0 };
        let new_sp = p | spmask;
        let new_ipsr = psr & 0x1FF;
        // CLRONRET: Secure FP registers are cleared when returning to Non-secure code
        if cur_secure && !frame_secure && self.fpccr[S] & (1 << 28) != 0 && !ftype {
            for i in 0..16 {
                self.s[i] = 0;
            }
            self.fpscr = crate::fpu::FPSCR_RESET;
            self.vpr = 0;
        }
        self.set_stack_state(if to_thread { 0 } else { new_ipsr }, frame_secure, if to_thread { use_psp } else { self.spsel[fst] });
        self.set_sp_of(frame_secure, use_psp, new_sp);
        if !dcrs {
            self.r[4..12].copy_from_slice(&callee_vals);
        }
        self.r[0] = vals[0];
        self.r[1] = vals[1];
        self.r[2] = vals[2];
        self.r[3] = vals[3];
        self.r[12] = vals[4];
        self.r[14] = vals[5];
        self.pc = vals[6] & !1;
        self.fpca = !ftype;
        if frame_secure {
            self.sfpa = !ftype;
        }
        self.set_apsr_nzcvq(psr);
        self.ge = ((psr >> 16) & 0xF) as u8;
        self.t = (psr >> 24) & 1 != 0;
        self.it = ((((psr >> 10) & 0x3F) << 2) | ((psr >> 25) & 3)) as u8;
        self.excl = false;
        self.event_reg = true;
        self.exc_dirty = true;
        self.extra_cycles += 10;
        self.lo_branch_valid = false;
        if unstk_err {
            self.bfsr |= (CFSR_UNSTKERR >> 8) as u8;
            self.escalate_or_pend(EXC_BUSFAULT, !self.bfhfnmins);
        }
        if (to_thread && new_ipsr != 0) || (!to_thread && new_ipsr == 0) {
            self.raise_fault(bus, Trap::Usage(CFSR_INVPC), self.pc);
            return;
        }
        if to_thread && set_empty(&self.active) && self.scr[self.st()] & 2 != 0 {
            // SLEEPONEXIT
            self.sleeping = true;
            self.sleep_wfe = false;
        }
    }

    // ---------------------------------------------------------------- run loop

    /// Wake-up condition of WFI/WFE: an exception that would preempt if PRIMASK were clear.
    fn wake_condition(&self) -> bool {
        if self.sleep_wfe && self.event_reg {
            return true;
        }
        if self.compute_ready(false).is_some() {
            return true;
        }
        if self.scr[self.st()] & (1 << 4) != 0 && self.sleep_wfe && !set_empty(&self.pending) {
            return true;
        }
        false
    }

    /// Resumes execution after a debug halt.
    pub fn resume(&mut self) {
        self.halted = None;
        self.c_halt = false;
        self.skip_bp = true;
    }

    /// Requests a halt (C_HALT); takes effect at the next instruction boundary.
    pub fn request_halt(&mut self) {
        self.c_halt = true;
    }

    fn halt(&mut self, reason: HaltReason) -> Exit {
        self.halted = Some(reason);
        self.c_halt = true;
        self.sleeping = false;
        self.dfsr |= match reason {
            HaltReason::Bkpt(_) | HaltReason::Breakpoint => DFSR_BKPT,
            HaltReason::Watchpoint(..) => DFSR_DWTTRAP,
            HaltReason::VectorCatch => DFSR_VCATCH,
            _ => DFSR_HALTED,
        };
        Exit::Halted(reason)
    }

    fn next_systick_wrap(&self) -> Ticks {
        self.systick[S].next_wrap.min(self.systick[NS].next_wrap)
    }

    /// Runs until `limit`, the bus deadline, a debug halt, sleep or a reset request.
    pub fn run<B: Bus>(&mut self, bus: &mut B, limit: Ticks) -> Exit {
        loop {
            if let Some(lines) = bus.take_irq_lines() {
                self.set_irq_lines(lines);
            }
            if let Some(r) = self.halted {
                return Exit::Halted(r);
            }
            if let Some(r) = self.reset_request.take() {
                return Exit::Reset(r);
            }
            if self.c_halt && self.c_debugen {
                return self.halt(HaltReason::Request);
            }
            if self.now >= self.next_systick_wrap() {
                self.systick_wraps();
            }
            if let Some(slot) = self.ready_exception() {
                self.sleeping = false;
                let ret = self.pc;
                self.take_exception(bus, slot, ret);
                self.now += (self.extra_cycles as u64) * TICKS_PER_CPU_CYCLE;
                self.extra_cycles = 0;
                continue;
            }
            if self.sleeping {
                if self.wake_condition() {
                    self.sleeping = false;
                    if self.sleep_wfe {
                        self.event_reg = false;
                    }
                    continue;
                }
                return Exit::Sleeping;
            }
            let deadline = limit.min(bus.deadline()).min(self.next_systick_wrap());
            if self.now >= deadline {
                if self.now >= self.next_systick_wrap() {
                    continue;
                }
                return Exit::Deadline;
            }
            if !std::mem::take(&mut self.skip_bp) && self.c_debugen && self.breakpoint_at(self.pc) {
                return self.halt(HaltReason::Breakpoint);
            }
            let step = self.step_request || (self.c_step && self.c_debugen);
            self.step_instruction(bus);
            if let Some(r) = self.halted {
                return Exit::Halted(r);
            }
            if let Some((a, k)) = self.watch_hit.take() {
                if self.c_debugen {
                    return self.halt(HaltReason::Watchpoint(a, k));
                }
            }
            if step {
                self.step_request = false;
                return self.halt(HaltReason::Step);
            }
        }
    }

    fn breakpoint_at(&self, pc: u32) -> bool {
        self.sw_breakpoints.contains(&pc) || self.fpb.matches(pc)
    }

    /// Fetches, decodes and executes one instruction.
    pub(crate) fn step_instruction<B: Bus>(&mut self, bus: &mut B) {
        let pc = self.pc;
        if self.trace_insn > 0 {
            self.trace_insn -= 1;
            log::trace!(
                "[{}] pc {:#010x} sp {:#010x} r0 {:#010x} r1 {:#010x} lr {:#010x} {}",
                self.instret,
                pc,
                self.r[13],
                self.r[0],
                self.r[1],
                self.r[14],
                if self.secure { "S" } else { "NS" }
            );
        }
        if !self.t {
            self.raise_fault(bus, Trap::Usage(CFSR_INVSTATE), pc);
            self.finish_cycles(1);
            return;
        }
        let fetch_ns = match self.check_exec(bus, pc) {
            Ok(ns) => ns,
            Err(t) => {
                self.raise_fault(bus, t, pc);
                self.finish_cycles(1);
                return;
            }
        };
        let (ins, size) = match self.fetch_decode(bus, pc, fetch_ns) {
            Ok(v) => v,
            Err(()) => {
                self.raise_fault(bus, Trap::Bus(CFSR_IBUSERR, None), pc);
                self.finish_cycles(1);
                return;
            }
        };
        if self.nsc_fetch && !matches!(ins, Instr::Sg) {
            self.raise_fault(bus, Trap::Secure(SFSR_INVEP, None), pc);
            self.finish_cycles(1);
            return;
        }
        self.extra_cycles += bus.fetch_wait_cycles(pc);
        self.next_pc = pc.wrapping_add(size);
        let is_it = matches!(ins, Instr::It { .. });
        let skip = self.it & 0xF != 0 && !is_it && !self.it_condition_passed();
        let cycles = if skip {
            1
        } else {
            let r = self.execute(bus, ins);
            let r = if std::mem::take(&mut self.stkof) && r.is_ok() { Err(Trap::Usage(CFSR_STKOF)) } else { r };
            match r {
                Ok(c) => c,
                Err(Trap::Svc) => {
                    self.it_advance();
                    self.pc = self.next_pc;
                    self.instret += 1;
                    let ret = self.pc;
                    self.raise_fault(bus, Trap::Svc, ret);
                    self.finish_cycles(1);
                    return;
                }
                Err(Trap::Bkpt(imm)) => {
                    if self.c_debugen {
                        self.halt(HaltReason::Bkpt(imm));
                    } else if self.demcr & (1 << 16) != 0 {
                        // DebugMonitor
                        self.dfsr |= DFSR_BKPT;
                        self.pend_sys(EXC_DEBUGMON, self.slot_secure(EXC_DEBUGMON));
                        self.pc = self.next_pc;
                    } else {
                        self.raise_fault(bus, Trap::Bkpt(imm), pc);
                    }
                    self.finish_cycles(1);
                    return;
                }
                Err(t) => {
                    self.raise_fault(bus, t, pc);
                    self.finish_cycles(1);
                    return;
                }
            }
        };
        if !is_it {
            self.it_advance();
        }
        if std::mem::take(&mut self.vpt_set) {
            // VPT/VPST set the mask for the following instruction
        } else {
            self.vpt_advance();
        }
        self.pc = self.next_pc;
        self.instret += 1;
        self.finish_cycles(cycles);
        if let Some(er) = self.exc_return.take() {
            self.exception_return(bus, er);
            self.finish_cycles(0);
        }
        if let Some(fr) = self.fnc_return.take() {
            self.function_return(bus, fr);
        }
    }

    /// Fetches and decodes the instruction at `pc`, from the decode cache when possible.
    #[inline]
    fn fetch_decode<B: Bus>(&mut self, bus: &mut B, pc: u32, ns: bool) -> Result<(Instr, u32), ()> {
        let gen = bus.code_generation();
        if gen != self.icache_gen {
            self.icache_gen = gen;
            self.invalidate_decode_cache();
        }
        if bus.code_cacheable(pc) {
            let page = (pc >> 12) as usize;
            if self.icache.len() <= page {
                self.icache.resize_with(1 << 20, || None);
            }
            if self.icache[page].is_none() {
                self.icache[page] = Some(Box::new(DecodePage { ins: Box::new([Instr::Undefined; 2048]), size: Box::new([0; 2048]) }));
                self.icache_pages.push(page as u32);
            }
            let i = ((pc & 0xFFF) >> 1) as usize;
            let p = self.icache[page].as_mut().unwrap();
            let sz = p.size[i];
            if sz != 0 {
                return Ok((p.ins[i], sz as u32));
            }
            let r = Self::decode_at(bus, pc, ns)?;
            let p = self.icache[page].as_mut().unwrap();
            p.ins[i] = r.0;
            p.size[i] = r.1 as u8;
            return Ok(r);
        }
        Self::decode_at(bus, pc, ns)
    }

    /// Drops the decoded instructions of the page holding `addr` (a store into code memory).
    #[inline]
    pub(crate) fn invalidate_decode_page(&mut self, addr: u32) {
        let page = (addr >> 12) as usize;
        if page < self.icache.len() && self.icache[page].is_some() {
            self.icache[page] = None;
            self.icache_pages.retain(|p| *p as usize != page);
        }
    }

    pub fn invalidate_decode_cache(&mut self) {
        for p in std::mem::take(&mut self.icache_pages) {
            self.icache[p as usize] = None;
        }
    }

    fn decode_at<B: Bus>(bus: &mut B, pc: u32, ns: bool) -> Result<(Instr, u32), ()> {
        let hw1 = bus.fetch16(pc, ns).map_err(|_| ())?;
        if is_32bit(hw1) {
            let hw2 = bus.fetch16(pc.wrapping_add(2), ns).map_err(|_| ())?;
            Ok((decode(hw1, hw2), 4))
        } else {
            Ok((decode(hw1, 0), 2))
        }
    }

    #[inline]
    fn finish_cycles(&mut self, cycles: u32) {
        self.now += ((cycles + self.extra_cycles) as u64) * TICKS_PER_CPU_CYCLE;
        self.extra_cycles = 0;
    }

    #[inline]
    fn it_condition_passed(&self) -> bool {
        crate::alu::condition_passed(self.it >> 4, self.n, self.z, self.c, self.v)
    }

    #[inline]
    pub(crate) fn in_it_block(&self) -> bool {
        self.it & 0xF != 0
    }

    #[inline]
    fn it_advance(&mut self) {
        if self.it & 0x7 == 0 {
            self.it = 0;
        } else {
            self.it = (self.it & 0xE0) | ((self.it << 1) & 0x1F);
        }
    }

    /// `VPTAdvance()`: shifts the MVE predication masks after every instruction of a VPT
    /// block. The mask bits are relative: a 1 in the top bit means the next instruction has the
    /// opposite condition, so P0 is inverted for the beats the mask covers.
    #[inline]
    fn vpt_advance(&mut self) {
        let mask01 = (self.vpr >> 16) & 0xF;
        let mask23 = (self.vpr >> 20) & 0xF;
        if mask01 == 0 && mask23 == 0 {
            return;
        }
        let mut p0 = self.vpr & 0xFFFF;
        let adv = |m: u32, p0: &mut u32, lanes: u32| -> u32 {
            if m == 0 {
                return 0;
            }
            if m & 7 == 0 {
                return 0;
            }
            if m & 8 != 0 {
                *p0 ^= lanes;
            }
            (m << 1) & 0xF
        };
        let m01 = adv(mask01, &mut p0, 0x00FF);
        let m23 = adv(mask23, &mut p0, 0xFF00);
        self.vpr = (self.vpr & 0xFF00_0000) | (m23 << 20) | (m01 << 16) | p0;
    }

    /// Execute-never and security check of an instruction fetch; returns the fetch attribute.
    fn check_exec<B: Bus>(&mut self, bus: &B, pc: u32) -> Result<bool, Trap> {
        self.nsc_fetch = false;
        let ns = if self.secure && self.sau_all_secure {
            false
        } else {
            let (s, nsc) = self.attribution(bus, pc);
            if !self.secure && s {
                if !nsc {
                    return Err(Trap::Secure(SFSR_INVEP, None));
                }
                self.nsc_fetch = true;
            }
            if self.secure && !s {
                return Err(Trap::Secure(SFSR_INVTRAN, None));
            }
            !s
        };
        let xn_default = matches!(pc >> 28, 0x4 | 0x5 | 0xA..=0xF);
        let mpu = &self.mpu[self.st()];
        if mpu.enabled() && !(self.handler_mode() && mpu.ctrl & 2 == 0 && self.execution_priority() < 0) {
            match mpu.check(pc, self.privileged(), false, true) {
                Some(true) => return Ok(ns),
                Some(false) => return Err(Trap::Mem(CFSR_IACCVIOL, None)),
                None => {
                    if !(mpu.ctrl & 4 != 0 && self.privileged()) {
                        return Err(Trap::Mem(CFSR_IACCVIOL, None));
                    }
                }
            }
        }
        if xn_default {
            return Err(Trap::Mem(CFSR_IACCVIOL, None));
        }
        Ok(ns)
    }

    // ---------------------------------------------------------------- SysTick

    pub(crate) fn systick_wraps(&mut self) {
        for i in [S, NS] {
            if self.now >= self.systick[i].next_wrap {
                let t = self.systick[i].next_wrap;
                self.systick[i].countflag = true;
                if self.systick[i].ctrl & 2 != 0 {
                    self.set_pending(Self::sys_slot(EXC_SYSTICK, i == S));
                }
                self.systick[i].after_wrap(t);
            }
        }
    }

    /// Advances an idle core to `t` (rounded up to a CPU clock edge).
    pub fn advance_to(&mut self, t: Ticks) {
        if t > self.now {
            self.now = ceil_cpu_cycle(t);
        }
    }

    /// Next time the core has its own event (SysTick wrap).
    pub fn next_internal_event(&self) -> Ticks {
        self.next_systick_wrap()
    }
}
