//! MPS4 FPGA I/O block (`arm_mps3_io_reg_map.h`): LEDs, buttons, switches, the 1 Hz and
//! 100 Hz counters, the cycle counter with its prescaler, the MISC register; and the SCC
//! (serial configuration controller) register file of the FPGA.

use crate::env::{narrow, widen, Env};
use cs320_core::time::{FracPeriod, TICKS_PER_SEC};
use cs320_core::Ticks;

pub struct FpgaIo {
    pub leds: u32,
    pub prescale: u32,
    pub misc: u32,
    /// Origin of the counters (reset time).
    origin: Ticks,
    cycle_period: FracPeriod,
    /// Cycle counter value at `cycle_origin`.
    cycle_base: u32,
    cycle_origin: Ticks,
}

impl FpgaIo {
    pub fn new(cpu_hz: u64) -> Self {
        FpgaIo { leds: 0, prescale: 0, misc: 0, origin: 0, cycle_period: FracPeriod::from_hz(cpu_hz), cycle_base: 0, cycle_origin: 0 }
    }

    pub fn reset(&mut self, now: Ticks) {
        self.leds = 0;
        self.prescale = 0;
        self.misc = 0;
        self.origin = now;
        self.cycle_base = 0;
        self.cycle_origin = now;
    }

    fn counter(&self, now: Ticks) -> u32 {
        // the cycle counter runs at the system clock divided by (PRESCALE + 1)
        let div = self.prescale as u64 + 1;
        (self.cycle_base as u64 + self.cycle_period.count(self.cycle_origin, now) / div) as u32
    }

    pub fn read(&mut self, env: &mut Env, off: u32, size: u8) -> u32 {
        let now = env.now;
        let v = match off & !3 {
            0x000 => self.leds,
            0x008 => env.board.fpga_buttons(now) & 3,
            0x010 => ((now - self.origin) / TICKS_PER_SEC) as u32,
            0x014 => ((now - self.origin) / (TICKS_PER_SEC / 100)) as u32,
            0x018 => self.counter(now),
            0x01C => self.prescale,
            0x020 => {
                let div = self.prescale as u64 + 1;
                (self.cycle_period.count(self.cycle_origin, now) % div) as u32
            }
            0x028 => env.board.fpga_switches(now) & 0xFF,
            0x04C => self.misc,
            _ => 0,
        };
        narrow(v, off, size)
    }

    pub fn write(&mut self, env: &mut Env, off: u32, size: u8, v: u32) {
        let now = env.now;
        match off & !3 {
            0x000 => {
                self.leds = widen(self.leds, off, size, v) & 0x3FF;
                env.board.fpga_leds(now, self.leds);
            }
            0x010 | 0x014 => {
                // writing the 1 Hz / 100 Hz counters restarts them
                self.origin = now;
            }
            0x018 => {
                self.cycle_base = widen(self.counter(now), off, size, v);
                self.cycle_origin = now;
            }
            0x01C => {
                self.cycle_base = self.counter(now);
                self.cycle_origin = now;
                self.prescale = widen(self.prescale, off, size, v);
            }
            0x04C => {
                self.misc = widen(self.misc, off, size, v) & 0x7;
                env.board.fpga_misc(now, self.misc);
            }
            _ => {}
        }
    }
}

/// SCC: CFG_REG0..7 at 0x000.., CFG_DATA, CFG_CTRL, CFG_STAT, DLL, AID, ID. The FVP reports
/// a fixed identity; configuration writes are stored.
pub struct Scc {
    pub cfg: [u32; 8],
    pub cfgdata_rtn: u32,
    pub cfgdata_out: u32,
    pub cfgctrl: u32,
    pub cfgstat: u32,
    pub dll: u32,
}

impl Scc {
    pub fn new() -> Self {
        let mut s = Scc { cfg: [0; 8], cfgdata_rtn: 0, cfgdata_out: 0, cfgctrl: 0, cfgstat: 0, dll: 0 };
        s.reset();
        s
    }

    pub fn reset(&mut self) {
        // CFG_REG0..7 as the FVP reports them at reset
        self.cfg = std::array::from_fn(|i| crate::fvp::word(0x4820_0000 + 4 * i as u32).unwrap_or(0));
        self.cfgdata_rtn = 0;
        self.cfgdata_out = 0;
        self.cfgctrl = 0;
        self.cfgstat = 0;
        self.dll = 0xFF00_0001;
    }

    pub fn read(&self, off: u32, size: u8) -> u32 {
        let v = match off & !3 {
            0x000..=0x01C => self.cfg[((off & !3) / 4) as usize],
            0x0A0 => self.cfgdata_rtn,
            0x0A4 => self.cfgdata_out,
            0x0A8 => self.cfgctrl,
            0x0AC => self.cfgstat,
            0x100 => self.dll,
            0xFF8 => 0x0000_0020, // AID: FPGA build
            0xFFC => 0x4104_7220, // ID: MPS4, Corstone-320 image, revision
            o => crate::fvp::word(0x4820_0000 + o).unwrap_or(0),
        };
        narrow(v, off, size)
    }

    pub fn write(&mut self, off: u32, size: u8, v: u32) {
        match off & !3 {
            0x000..=0x01C => {
                let i = ((off & !3) / 4) as usize;
                self.cfg[i] = widen(self.cfg[i], off, size, v);
            }
            0x0A4 => self.cfgdata_out = widen(self.cfgdata_out, off, size, v),
            0x0A8 => {
                self.cfgctrl = widen(self.cfgctrl, off, size, v);
                if self.cfgctrl & (1 << 31) != 0 {
                    // a configuration transaction completes at once: the data returns
                    self.cfgdata_rtn = self.cfgdata_out;
                    self.cfgstat = 1;
                    self.cfgctrl &= !(1 << 31);
                }
            }
            0x0AC => self.cfgstat = 0,
            0x100 => self.dll = widen(self.dll, off, size, v),
            _ => {}
        }
    }
}

impl Default for Scc {
    fn default() -> Self {
        Self::new()
    }
}
