//! PL031 real-time clock: a 1 Hz counter with a match interrupt.

use crate::env::{narrow, widen, Env};
use cs320_core::time::TICKS_PER_SEC;
use cs320_core::Ticks;

pub struct Pl031 {
    base: u32,
    origin: Ticks,
    pub mr: u32,
    pub lr: u32,
    pub cr: u32,
    pub imsc: u32,
    pub ris: u32,
    slot: cs320_core::Slot,
}

impl Pl031 {
    pub fn new(slot: cs320_core::Slot) -> Self {
        Pl031 { base: 0, origin: 0, mr: 0, lr: 0, cr: 0, imsc: 0, ris: 0, slot }
    }

    /// Reset with the counter at `epoch`.
    pub fn reset(&mut self, now: Ticks, epoch: u32) {
        self.base = epoch;
        self.origin = now;
        self.mr = 0;
        self.lr = epoch;
        self.cr = 0;
        self.imsc = 0;
        self.ris = 0;
    }

    pub fn value(&self, now: Ticks) -> u32 {
        self.base.wrapping_add(((now - self.origin) / TICKS_PER_SEC) as u32)
    }

    pub fn irq(&self) -> bool {
        self.ris & self.imsc & 1 != 0
    }

    fn reschedule(&mut self, env: &mut Env) {
        env.cancel(self.slot);
        let cur = self.value(env.now);
        let delta = self.mr.wrapping_sub(cur) as u64;
        if delta == 0 {
            self.ris |= 1;
            return;
        }
        // the time at which the counter equals MR
        let elapsed = (env.now - self.origin) % TICKS_PER_SEC;
        let t = env.now + delta * TICKS_PER_SEC - elapsed;
        env.schedule(self.slot, t);
    }

    pub fn event(&mut self, env: &mut Env) {
        if self.value(env.now) == self.mr {
            self.ris |= 1;
        }
        let _ = env;
    }

    pub fn read(&mut self, env: &mut Env, off: u32, size: u8) -> u32 {
        let v = match off & !3 {
            0x000 => self.value(env.now),
            0x004 => self.mr,
            0x008 => self.lr,
            0x00C => self.cr,
            0x010 => self.imsc,
            0x014 => self.ris,
            0x018 => self.ris & self.imsc,
            0xFE0 => 0x31,
            0xFE4 => 0x10,
            0xFE8 => 0x04,
            0xFEC => 0x00,
            0xFF0 => 0x0D,
            0xFF4 => 0xF0,
            0xFF8 => 0x05,
            0xFFC => 0xB1,
            _ => 0,
        };
        narrow(v, off, size)
    }

    pub fn write(&mut self, env: &mut Env, off: u32, size: u8, v: u32) {
        match off & !3 {
            0x004 => {
                self.mr = widen(self.mr, off, size, v);
                self.reschedule(env);
            }
            0x008 => {
                self.lr = widen(self.lr, off, size, v);
                self.base = self.lr;
                self.origin = env.now;
                self.reschedule(env);
            }
            0x00C => self.cr = widen(self.cr, off, size, v) & 1,
            0x010 => self.imsc = widen(self.imsc, off, size, v) & 1,
            0x01C => self.ris &= !(v & 1),
            _ => {}
        }
    }
}
