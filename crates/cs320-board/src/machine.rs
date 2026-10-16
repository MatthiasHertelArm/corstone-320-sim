//! The simulated board: the core, the subsystem and the board, advanced together in time.

use crate::board::Cs320Board;
use cs320_core::Ticks;
use cs320_cpu::{Cpu, Exit, HaltReason, ResetRequest};
use cs320_loader::Image;
use cs320_soc::{Params, System};

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The requested time was reached.
    Time,
    /// The core halted in debug state.
    Halted(HaltReason),
    /// The program exited through semihosting.
    Exited,
}

/// RESET_SYNDROME bits.
pub const SYNDROME_POR: u32 = 1 << 0;
pub const SYNDROME_SWRESETREQ: u32 = 1 << 5;
pub const SYNDROME_HOSTRESETREQ: u32 = 1 << 7;
pub const SYNDROME_CPU0RSTREQ: u32 = 1 << 8;
pub const SYNDROME_CPU0LOCKUP: u32 = 1 << 12;

pub struct Machine {
    pub cpu: Cpu,
    pub sys: System<Cs320Board>,
    /// Halt instead of resetting when the core locks up (debugging aid).
    pub halt_on_lockup: bool,
    /// Number of resets since power-on.
    pub resets: u64,
    /// Serve semihosting calls (BKPT 0xAB) instead of halting.
    pub semihosting: bool,
    pub semihost_out: Vec<u8>,
    pub semihost_in: Vec<u8>,
    /// Exit code of a semihosting SYS_EXIT (or a scenario).
    pub exit_code: Option<i32>,
    /// Everything the target sent on each UART (last 1 MB), for scenario checks.
    pub uart_log: Vec<Vec<u8>>,
}

impl Default for Machine {
    fn default() -> Self {
        Self::new(Params::default())
    }
}

impl Machine {
    pub fn new(params: Params) -> Self {
        let mut cpu = Cpu::new();
        cpu.pacbti = params.pacbti;
        let sys = System::new(Cs320Board::new(), params);
        let mut m = Machine {
            cpu,
            sys,
            halt_on_lockup: false,
            resets: 0,
            semihosting: false,
            semihost_out: Vec::new(),
            semihost_in: Vec::new(),
            exit_code: None,
            uart_log: vec![Vec::new(); 6],
        };
        m.power_on();
        m
    }

    /// Writes an image into the memories.
    pub fn load_image(&mut self, img: &Image) -> Result<(), String> {
        for seg in &img.segments {
            for (i, b) in seg.data.iter().enumerate() {
                let a = seg.addr.wrapping_add(i as u32);
                if !self.sys.load_byte(a, *b) {
                    return Err(format!("image byte at {a:#010x} is outside the memories"));
                }
            }
        }
        // the reset vector base is the image's vector table, so the core boots the image
        if let Some(vtor) = img.vector_table() {
            self.sys.sysctrl.set_initsvtor_rst(vtor);
        }
        Ok(())
    }

    /// Power-on reset of everything.
    pub fn power_on(&mut self) {
        let now = self.cpu.now;
        self.sys.reset(now, false, SYNDROME_POR);
        self.cpu.power_on_reset(&mut self.sys);
    }

    /// Warm reset (SYSRESETREQ, watchdog, software reset, lockup); keeps the memories and
    /// the debug state.
    pub fn system_reset(&mut self, syndrome: u32) {
        let now = self.cpu.now;
        self.resets += 1;
        self.sys.reset(now, true, syndrome);
        self.cpu.reset(&mut self.sys);
    }

    /// Reset from the host (the FVP's reset button / `monitor reset`).
    pub fn reset_button(&mut self) {
        self.system_reset(SYNDROME_HOSTRESETREQ);
    }

    pub fn now(&self) -> Ticks {
        self.cpu.now
    }

    /// Runs until `until` or a halt.
    pub fn run_until(&mut self, until: Ticks) -> Stop {
        loop {
            self.sys.process_events(self.cpu.now);
            if self.exit_code.is_some() {
                return Stop::Exited;
            }
            if self.semihosting {
                self.cpu.c_debugen = true;
            }
            if let Some(syndrome) = self.sys.reset_request.take() {
                self.system_reset(syndrome);
            }
            if self.cpu.now >= until {
                return Stop::Time;
            }
            match self.cpu.run(&mut self.sys, until) {
                Exit::Deadline => {}
                Exit::Sleeping => {
                    let next = self.sys.next_event().min(self.cpu.next_internal_event()).min(until);
                    self.cpu.advance_to(next);
                }
                Exit::Halted(HaltReason::Bkpt(0xAB)) if self.semihosting => {
                    if !self.semihost() {
                        return Stop::Exited;
                    }
                }
                Exit::Halted(r) => return Stop::Halted(r),
                Exit::Reset(r) => match r {
                    ResetRequest::System => self.system_reset(SYNDROME_CPU0RSTREQ),
                    ResetRequest::Lockup => {
                        if self.halt_on_lockup && self.cpu.c_debugen {
                            self.cpu.halted = Some(HaltReason::Lockup);
                            return Stop::Halted(HaltReason::Lockup);
                        }
                        self.system_reset(SYNDROME_CPU0LOCKUP);
                    }
                },
            }
        }
    }

    /// Takes the bytes the target sent on each UART (UART0 also carries the semihosting
    /// console output).
    pub fn take_uart(&mut self) -> Vec<Vec<u8>> {
        let mut out: Vec<Vec<u8>> = self.sys.board.uart_out.iter_mut().map(std::mem::take).collect();
        out[0].append(&mut self.semihost_out);
        for (n, v) in out.iter().enumerate() {
            self.uart_log[n].extend_from_slice(v);
            if self.uart_log[n].len() > 1 << 20 {
                let cut = self.uart_log[n].len() - (1 << 19);
                self.uart_log[n].drain(..cut);
            }
        }
        out
    }

    /// UART output so far on UART `n`, including bytes not yet taken.
    pub fn uart_text(&self, n: usize) -> String {
        let mut v = self.uart_log[n].clone();
        v.extend_from_slice(&self.sys.board.uart_out[n]);
        if n == 0 {
            v.extend_from_slice(&self.semihost_out);
        }
        String::from_utf8_lossy(&v).to_string()
    }

    /// Sends bytes from the host to UART `n`.
    pub fn uart_send(&mut self, n: usize, data: &[u8]) {
        let now = self.cpu.now;
        self.sys.uart_send(now, n.min(5), data);
    }

    /// CRC-32 of the latest HDLCD frame (for scenario checks).
    pub fn display_crc(&self, display: usize) -> Option<(u32, u32, u32)> {
        let f = self.sys.board.displays.get(display)?.as_ref()?;
        let mut crc = 0xFFFF_FFFFu32;
        for b in &f.rgba {
            crc ^= *b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        Some((f.width, f.height, !crc))
    }
}
