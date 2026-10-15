//! CMSDK GPIO (`gpio_cmsdk_reg_map.h`): 16-bit ports with output enables, alternate
//! function selection and level/edge interrupts; the masked-access windows at 0x400/0x800.

use crate::env::{narrow, widen, Env};

pub struct CmsdkGpio {
    pub port: usize,
    pub dataout: u16,
    pub outenable: u16,
    pub altfunc: u16,
    pub inten: u16,
    pub inttype: u16,
    pub intpol: u16,
    pub intstatus: u16,
    last_in: u16,
}

impl CmsdkGpio {
    pub fn new(port: usize) -> Self {
        CmsdkGpio { port, dataout: 0, outenable: 0, altfunc: 0, inten: 0, inttype: 0, intpol: 0, intstatus: 0, last_in: 0 }
    }

    pub fn reset(&mut self) {
        self.dataout = 0;
        self.outenable = 0;
        self.altfunc = 0;
        self.inten = 0;
        self.inttype = 0;
        self.intpol = 0;
        self.intstatus = 0;
        self.last_in = 0;
    }

    pub fn irq(&self) -> bool {
        self.intstatus & self.inten != 0
    }

    /// Current input value: driven outputs read back, the rest from the board.
    pub fn data(&mut self, env: &mut Env) -> u16 {
        let ext = env.board.gpio_in(env.now, self.port);
        let v = (self.dataout & self.outenable) | (ext & !self.outenable);
        self.detect(v);
        v
    }

    /// Evaluates the interrupt conditions on new input levels.
    pub fn detect(&mut self, v: u16) {
        for bit in 0..16u16 {
            let m = 1 << bit;
            let level = v & m != 0;
            let pol = self.intpol & m != 0;
            if self.inttype & m != 0 {
                // edge: rising when polarity set, falling otherwise
                let was = self.last_in & m != 0;
                if was != level && level == pol {
                    self.intstatus |= m;
                }
            } else if level == pol {
                self.intstatus |= m;
            }
        }
        self.last_in = v;
    }

    /// Re-evaluates the inputs (board change).
    pub fn poll(&mut self, env: &mut Env) {
        let _ = self.data(env);
    }

    fn update_out(&mut self, env: &mut Env) {
        env.board.gpio_out(env.now, self.port, self.dataout, self.outenable & !self.altfunc);
    }

    pub fn read(&mut self, env: &mut Env, off: u32, size: u8) -> u32 {
        let o = off & 0xFFF;
        let v = match o & !3 {
            0x000 => self.data(env) as u32,
            0x004 => self.dataout as u32,
            0x010 | 0x014 => self.outenable as u32,
            0x018 | 0x01C => self.altfunc as u32,
            0x020 | 0x024 => self.inten as u32,
            0x028 | 0x02C => self.inttype as u32,
            0x030 | 0x034 => self.intpol as u32,
            0x038 => self.intstatus as u32,
            0x400..=0x7FC => {
                // lower byte masked access: address bits 9:2 are the mask
                let mask = ((o - 0x400) >> 2) as u16;
                (self.data(env) & mask) as u32
            }
            0x800..=0xBFC => {
                let mask = (((o - 0x800) >> 2) as u16) << 8;
                (self.data(env) & mask) as u32
            }
            0xFE0 => 0x20,
            0xFE4 => 0xB8,
            0xFE8 => 0x1B,
            0xFF0 => 0x0D,
            0xFF4 => 0xF0,
            0xFF8 => 0x05,
            0xFFC => 0xB1,
            _ => 0,
        };
        narrow(v, off, size)
    }

    pub fn write(&mut self, env: &mut Env, off: u32, size: u8, v: u32) {
        let o = off & 0xFFF;
        let v16 = |old: u16| widen(old as u32, off, size, v) as u16;
        match o & !3 {
            0x000 | 0x004 => {
                self.dataout = v16(self.dataout);
                self.update_out(env);
            }
            0x010 => {
                self.outenable |= v as u16;
                self.update_out(env);
            }
            0x014 => {
                self.outenable &= !(v as u16);
                self.update_out(env);
            }
            0x018 => {
                self.altfunc |= v as u16;
                self.update_out(env);
            }
            0x01C => {
                self.altfunc &= !(v as u16);
                self.update_out(env);
            }
            0x020 => self.inten |= v as u16,
            0x024 => self.inten &= !(v as u16),
            0x028 => self.inttype |= v as u16,
            0x02C => self.inttype &= !(v as u16),
            0x030 => self.intpol |= v as u16,
            0x034 => self.intpol &= !(v as u16),
            0x038 => self.intstatus &= !(v as u16),
            0x400..=0x7FC => {
                let mask = ((o - 0x400) >> 2) as u16;
                self.dataout = (self.dataout & !mask) | (v as u16 & mask);
                self.update_out(env);
            }
            0x800..=0xBFC => {
                let mask = (((o - 0x800) >> 2) as u16) << 8;
                self.dataout = (self.dataout & !mask) | (v as u16 & mask);
                self.update_out(env);
            }
            _ => {}
        }
    }
}
