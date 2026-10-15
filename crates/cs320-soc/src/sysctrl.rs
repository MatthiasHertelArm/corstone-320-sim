//! The Corstone-320 system control and information blocks (`platform_regs.h`): SYSINFO,
//! SYSCTRL, the secure and non-secure access configuration blocks (PPC control), the memory
//! protection controllers, the CPU identity/power/security blocks and the power policy units.
//! They are register files with ID registers; the few functional registers (SWRESET,
//! RESET_SYNDROME, INITSVTOR0, GRETREG, CPUWAIT, the MPC/PPC interrupt registers) are modelled.

use crate::env::{coresight_id, narrow, widen};
use std::collections::HashMap;

/// A plain 4 KB register block with ID registers and reset values.
pub struct RegBlock {
    pub name: &'static str,
    pub regs: HashMap<u32, u32>,
    defaults: &'static [(u32, u32)],
    pidr: [u32; 8],
    cidr: [u32; 4],
}

const CIDR: [u32; 4] = [0x0D, 0xF0, 0x05, 0xB1];

impl RegBlock {
    pub fn new(name: &'static str, pidr: [u32; 8], defaults: &'static [(u32, u32)]) -> Self {
        let mut b = RegBlock { name, regs: HashMap::new(), defaults, pidr, cidr: CIDR };
        b.reset();
        b
    }

    pub fn reset(&mut self) {
        self.regs.clear();
        for (o, v) in self.defaults {
            self.regs.insert(*o, *v);
        }
    }

    pub fn get(&self, off: u32) -> u32 {
        self.regs.get(&(off & !3)).copied().unwrap_or(0)
    }

    pub fn set(&mut self, off: u32, v: u32) {
        self.regs.insert(off & !3, v);
    }

    pub fn read(&self, off: u32, size: u8) -> u32 {
        let o = off & !3;
        let v = coresight_id(o, &self.pidr, &self.cidr).unwrap_or_else(|| self.get(o));
        narrow(v, off, size)
    }

    pub fn write(&mut self, off: u32, size: u8, v: u32) {
        let o = off & !3;
        if o >= 0xFC8 {
            return;
        }
        let nv = widen(self.get(o), off, size, v);
        self.set(o, nv);
    }
}

pub const SYSINFO_PIDR: [u32; 8] = [0x58, 0xB8, 0x1B, 0x00, 0x04, 0, 0, 0];
pub const SYSCTRL_PIDR: [u32; 8] = [0x54, 0xB8, 0x1B, 0x00, 0x04, 0, 0, 0];
pub const SACFG_PIDR: [u32; 8] = [0x52, 0xB8, 0x1B, 0x00, 0x04, 0, 0, 0];
pub const NSACFG_PIDR: [u32; 8] = [0x53, 0xB8, 0x1B, 0x00, 0x04, 0, 0, 0];
pub const MPC_PIDR: [u32; 8] = [0x60, 0xB8, 0x1B, 0x00, 0x04, 0, 0, 0];
pub const GENERIC_PIDR: [u32; 8] = [0x00, 0xB0, 0x0B, 0x00, 0x04, 0, 0, 0];

/// SYSINFO reset values: SoC identity (Corstone-320), SYS_CONFIG0 (CPU0 = Cortex-M85 with
/// system TCM, one VM bank, crypto present), IIDR.
pub const SYSINFO_DEFAULTS: &[(u32, u32)] = &[(0x000, 0x0C0D_0A41), (0x004, 0x0000_3101), (0x008, 0x0000_0000), (0xFC8, 0x0B01_0A41)];

/// SYSCTRL reset values: SECDBGSTAT (debug enabled), CLK_CFG, RESET_SYNDROME (power-on reset),
/// INITSVTOR0 (set from the parameter at reset), CPUWAIT, PWRCTRL.
pub const SYSCTRL_DEFAULTS: &[(u32, u32)] =
    &[(0x000, 0x0000_0155), (0x010, 0x0000_0000), (0x014, 0x0000_0000), (0x100, 0x0000_0001), (0x104, 0x0000_0000), (0x110, 0x1100_0000), (0x120, 0x0000_0000), (0x1FC, 0x0000_0000), (0x200, 0x0000_0001)];

pub const SACFG_DEFAULTS: &[(u32, u32)] = &[(0x000, 0x0000_0000), (0x010, 0x0000_0000), (0x014, 0x0000_0000)];

/// MPC (SIE-300 style): CTRL, BLK_MAX, BLK_CFG, BLK_IDX, BLK_LUT, INT_STAT, INT_CLEAR, INT_EN, INT_INFO1/2, INT_SET.
pub const MPC_DEFAULTS: &[(u32, u32)] = &[(0x000, 0x0000_0100), (0x010, 0x0000_007F), (0x014, 0x0000_000E), (0x018, 0), (0x01C, 0), (0x020, 0), (0x028, 0)];

/// System control block: the register file plus the reset and boot behaviour.
pub struct SysCtrl {
    pub regs: RegBlock,
    /// INITSVTOR_RST parameter: the reset value of INITSVTOR0 and the core's reset vector base.
    pub initsvtor_rst: u32,
    /// A software reset was requested (SWRESET.SWRESETREQ).
    pub reset_request: bool,
}

impl SysCtrl {
    pub fn new(initsvtor_rst: u32) -> Self {
        let mut s = SysCtrl { regs: RegBlock::new("SYSCTRL", SYSCTRL_PIDR, SYSCTRL_DEFAULTS), initsvtor_rst, reset_request: false };
        s.regs.set(0x110, initsvtor_rst);
        s
    }

    /// Reset; `syndrome` is the RESET_SYNDROME bit of the cause (bit 0 power-on).
    pub fn reset(&mut self, syndrome: u32, power_on: bool) {
        let gretreg = self.regs.get(0x10C);
        let old_syndrome = self.regs.get(0x100);
        self.regs.reset();
        self.regs.set(0x110, self.initsvtor_rst);
        if power_on {
            self.regs.set(0x100, 1);
        } else {
            // GRETREG and the syndrome survive a warm reset
            self.regs.set(0x10C, gretreg);
            self.regs.set(0x100, old_syndrome | syndrome);
        }
        self.reset_request = false;
    }

    pub fn initsvtor(&self) -> u32 {
        self.regs.get(0x110) & 0xFFFF_FF80
    }

    pub fn read(&self, off: u32, size: u8) -> u32 {
        self.regs.read(off, size)
    }

    pub fn write(&mut self, off: u32, size: u8, v: u32) {
        match off & !3 {
            0x000 => {}
            0x004 => {
                // SECDBGSET: sets bits of SECDBGSTAT
                let s = self.regs.get(0x000) | v;
                self.regs.set(0x000, s);
            }
            0x008 => {
                let s = self.regs.get(0x000) & !v;
                self.regs.set(0x000, s);
            }
            0x100 => {
                // RESET_SYNDROME: write 1 to clear
                let s = self.regs.get(0x100) & !v;
                self.regs.set(0x100, s);
            }
            0x108 => {
                if v & (1 << 5) != 0 {
                    self.reset_request = true;
                }
            }
            0x110 => {
                if self.regs.get(0x110) & 1 == 0 {
                    // INITSVTOR0LOCK not set
                    self.regs.set(0x110, widen(self.regs.get(0x110), off, size, v) & 0xFFFF_FF81);
                }
            }
            _ => self.regs.write(off, size, v),
        }
    }
}
