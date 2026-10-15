//! Ethos-U85 NPU register interface (`ethosu85_interface.h` of the ethos-u-core-driver): the
//! identification, configuration, reset, protection, queue, PMU and base-pointer registers
//! behave so that the driver initialises; a command stream is not executed — starting one
//! raises the IRQ with `cmd_parse_error` set, so the driver reports a clean failure.

use crate::env::{narrow, widen, Env};
use std::collections::HashMap;

pub const PRODUCT_U85: u32 = 2;

pub struct EthosU {
    pub num_macs: u32,
    pub status: u32,
    pub cmd: u32,
    pub prot: u32,
    pub qbase: u64,
    pub qread: u32,
    pub qconfig: u32,
    pub qsize: u32,
    pub power_ctrl: u32,
    pub regioncfg: u32,
    pub regs: HashMap<u32, u32>,
    pub pmu: HashMap<u32, u32>,
    warned: bool,
}

impl EthosU {
    pub fn new(num_macs: u32) -> Self {
        let mut n = EthosU {
            num_macs,
            status: 0,
            cmd: 0,
            prot: 0,
            qbase: 0,
            qread: 0,
            qconfig: 0,
            qsize: 0,
            power_ctrl: 0,
            regioncfg: 0,
            regs: HashMap::new(),
            pmu: HashMap::new(),
            warned: false,
        };
        n.reset();
        n
    }

    pub fn reset(&mut self) {
        self.status = 0;
        self.cmd = 0;
        self.prot = 0;
        self.qbase = 0;
        self.qread = 0;
        self.qconfig = 0;
        self.qsize = 0;
        self.power_ctrl = 0;
        self.regioncfg = 0;
        self.regs.clear();
        self.pmu.clear();
    }

    /// ID: architecture 2.0.0, version r0p10 (value of the driver's default).
    pub fn id(&self) -> u32 {
        0x2000_0A01
    }

    pub fn config(&self) -> u32 {
        let macs_log2 = 31 - self.num_macs.max(1).leading_zeros();
        (PRODUCT_U85 << 28) | (1 << 4) | macs_log2
    }

    pub fn irq(&self) -> bool {
        self.status & 2 != 0 || self.status & (1 << 6) != 0
    }

    pub fn read(&mut self, _env: &mut Env, off: u32, size: u8) -> u32 {
        let o = off & 0xFFF_C;
        let v = match o {
            0x0000 => self.id(),
            0x0004 => self.status,
            0x0008 => self.cmd & 0x1C,
            0x000C => 0,
            0x0010 => self.qbase as u32,
            0x0014 => (self.qbase >> 32) as u32,
            0x0018 => self.qread,
            0x001C => self.qconfig,
            0x0020 => self.qsize,
            0x0024 => self.prot,
            0x0028 => self.config(),
            0x0038 => self.power_ctrl,
            0x003C => self.regioncfg,
            0x0FE0 => 0x85,
            0x0FE4 => 0xB0,
            0x0FE8 => 0x0B,
            0x0FEC => 0x00,
            0x0FF0 => 0x0D,
            0x0FF4 => 0xF0,
            0x0FF8 => 0x05,
            0x0FFC => 0xB1,
            0x1180..=0x1FFC => self.pmu.get(&o).copied().unwrap_or(0),
            _ => self.regs.get(&o).copied().unwrap_or(0),
        };
        narrow(v, off, size)
    }

    pub fn write(&mut self, env: &mut Env, off: u32, size: u8, v: u32) {
        let o = off & 0xFFF_C;
        match o {
            0x0008 => {
                let cmd = widen(0, off, size, v);
                self.cmd = cmd & 0x1C;
                if cmd & 2 != 0 {
                    self.status &= !2;
                }
                if cmd & 1 != 0 {
                    if !self.warned {
                        self.warned = true;
                        env.warn.warn(env.now, "Ethos-U85: command stream execution is not modelled; the inference fails with cmd_parse_error");
                    }
                    // the NPU reads the command stream and fails at once
                    self.status = (self.status & !(1 | (1 << 5))) | 2 | (1 << 4);
                    self.qread = self.qsize;
                }
                if cmd & (1 << 4) != 0 {
                    self.status &= !1;
                }
            }
            0x000C => {
                let r = widen(0, off, size, v);
                self.prot = r & 3;
                self.status = 0;
                self.qread = 0;
            }
            0x0010 => self.qbase = (self.qbase & 0xFFFF_FFFF_0000_0000) | widen(self.qbase as u32, off, size, v) as u64,
            0x0014 => self.qbase = (self.qbase & 0xFFFF_FFFF) | ((widen((self.qbase >> 32) as u32, off, size, v) as u64) << 32),
            0x001C => self.qconfig = widen(self.qconfig, off, size, v),
            0x0020 => self.qsize = widen(self.qsize, off, size, v),
            0x0038 => self.power_ctrl = widen(self.power_ctrl, off, size, v),
            0x003C => self.regioncfg = widen(self.regioncfg, off, size, v),
            0x1180..=0x1FFC => {
                let old = self.pmu.get(&o).copied().unwrap_or(0);
                let nv = widen(old, off, size, v);
                match o {
                    // PMCR: the counter reset bits do not stick
                    0x1180 => {
                        self.pmu.insert(o, nv & !6);
                        if nv & 2 != 0 {
                            for i in 0..8 {
                                self.pmu.insert(0x1300 + 4 * i, 0);
                            }
                        }
                        if nv & 4 != 0 {
                            self.pmu.insert(0x11A0, 0);
                            self.pmu.insert(0x11A4, 0);
                        }
                    }
                    // PMCNTENSET / PMINTSET / PMOVSSET: set bits
                    0x1184 | 0x118C | 0x1194 => {
                        let base = o - 4 * ((o - 0x1184) / 8 % 1);
                        let _ = base;
                        self.pmu.insert(o, old | nv);
                    }
                    // PMCNTENCLR / PMINTCLR / PMOVSCLR: clear bits of the set register
                    0x1188 | 0x1190 | 0x1198 => {
                        let set = o - 4;
                        let cur = self.pmu.get(&set).copied().unwrap_or(0);
                        self.pmu.insert(set, cur & !nv);
                    }
                    _ => {
                        self.pmu.insert(o, nv);
                    }
                }
            }
            _ => {
                let old = self.regs.get(&o).copied().unwrap_or(0);
                self.regs.insert(o, widen(old, off, size, v));
            }
        }
    }
}
