//! The Armv8-M system watchdogs (`syswdog_armv8-m_reg_map.h`: control frame + refresh frame)
//! and the CMSDK slow-clock watchdog.

use crate::env::{coresight_id, narrow, slot, widen, Env};
use crate::timer::SysCounter;
use cs320_core::time::FracPeriod;
use cs320_core::{Slot, Ticks};

const WDOG_PIDR: [u32; 8] = [0x01, 0xB0, 0x0B, 0x00, 0x04, 0, 0, 0];
const WDOG_CIDR: [u32; 4] = [0x0D, 0xF0, 0x05, 0xB1];

/// Generic watchdog: a compare value against the system counter; the first timeout raises the
/// watchdog signal (interrupt), the second (without a refresh) the reset request.
pub struct SysWdog {
    pub n: usize,
    pub wcs: u32,
    pub wor: u32,
    pub wcv: u64,
    /// WS0 (interrupt) and WS1 (reset) signals.
    pub ws0: bool,
    pub ws1: bool,
    pub reset_request: bool,
}

impl SysWdog {
    pub fn new(n: usize) -> Self {
        SysWdog { n, wcs: 0, wor: 0, wcv: 0, ws0: false, ws1: false, reset_request: false }
    }

    pub fn reset(&mut self) {
        self.wcs = 0;
        self.wor = 0;
        self.wcv = 0;
        self.ws0 = false;
        self.ws1 = false;
        self.reset_request = false;
    }

    pub fn irq(&self) -> bool {
        self.ws0
    }

    fn refresh(&mut self, env: &mut Env, counter: &SysCounter) {
        self.wcv = counter.value(env.now).wrapping_add(self.wor as u64);
        self.ws0 = false;
        self.ws1 = false;
        self.reschedule(env, counter);
    }

    fn reschedule(&mut self, env: &mut Env, counter: &SysCounter) {
        let sl = slot::WDOG[self.n];
        env.cancel(sl);
        if self.wcs & 1 != 0 {
            if let Some(t) = counter.time_of(env.now, self.wcv) {
                env.schedule(sl, t);
            }
        }
    }

    pub fn event(&mut self, env: &mut Env, counter: &SysCounter) {
        if self.wcs & 1 == 0 {
            return;
        }
        if !self.ws0 {
            self.ws0 = true;
            self.wcv = counter.value(env.now).wrapping_add(self.wor as u64);
            self.reschedule(env, counter);
        } else {
            self.ws1 = true;
            self.reset_request = true;
        }
    }

    pub fn read(&mut self, _env: &mut Env, off: u32, size: u8) -> u32 {
        let v = match off & !3 {
            0x0000 => self.wcs & 1 | ((self.ws0 as u32) << 1) | ((self.ws1 as u32) << 2),
            0x0008 => self.wor,
            0x0010 => self.wcv as u32,
            0x0014 => (self.wcv >> 32) as u32,
            0x0FCC | 0x1FCC => 0x0000_0041,
            0x1000 => 0,
            o if o >= 0x1000 => coresight_id(o - 0x1000, &WDOG_PIDR, &WDOG_CIDR).unwrap_or(0),
            o => coresight_id(o, &WDOG_PIDR, &WDOG_CIDR).unwrap_or(0),
        };
        narrow(v, off, size)
    }

    pub fn write(&mut self, env: &mut Env, off: u32, size: u8, v: u32, counter: &SysCounter) {
        match off & !3 {
            0x0000 => {
                let was = self.wcs & 1;
                self.wcs = widen(self.wcs, off, size, v) & 1;
                if was == 0 && self.wcs & 1 != 0 {
                    self.refresh(env, counter);
                } else {
                    self.reschedule(env, counter);
                }
            }
            0x0008 => {
                self.wor = widen(self.wor, off, size, v);
                self.refresh(env, counter);
            }
            0x0010 => {
                self.wcv = (self.wcv & 0xFFFF_FFFF_0000_0000) | widen(self.wcv as u32, off, size, v) as u64;
                self.reschedule(env, counter);
            }
            0x0014 => {
                self.wcv = (self.wcv & 0xFFFF_FFFF) | ((widen((self.wcv >> 32) as u32, off, size, v) as u64) << 32);
                self.reschedule(env, counter);
            }
            0x1000 => self.refresh(env, counter),
            _ => {}
        }
    }
}

/// CMSDK watchdog (the slow-clock watchdog): a down counter; the first timeout raises the
/// interrupt, the second the reset.
pub struct CmsdkWdog {
    pub load: u32,
    pub control: u32,
    pub raw_int: bool,
    pub lock: u32,
    pub reset_request: bool,
    base_value: u32,
    origin: Ticks,
    period: FracPeriod,
    slot: Slot,
}

impl CmsdkWdog {
    pub fn new(hz: u64, slot: Slot) -> Self {
        CmsdkWdog {
            load: 0xFFFF_FFFF,
            control: 0,
            raw_int: false,
            lock: 0,
            reset_request: false,
            base_value: 0xFFFF_FFFF,
            origin: 0,
            period: FracPeriod::from_hz(hz),
            slot,
        }
    }

    pub fn reset(&mut self, now: Ticks) {
        self.load = 0xFFFF_FFFF;
        self.control = 0;
        self.raw_int = false;
        self.lock = 0;
        self.reset_request = false;
        self.base_value = 0xFFFF_FFFF;
        self.origin = now;
    }

    fn running(&self) -> bool {
        self.control & 1 != 0
    }

    pub fn irq(&self) -> bool {
        self.raw_int && self.control & 1 != 0
    }

    pub fn value(&self, now: Ticks) -> u32 {
        if !self.running() {
            return self.base_value;
        }
        let elapsed = self.period.count(self.origin, now);
        self.base_value.saturating_sub(elapsed.min(u32::MAX as u64) as u32)
    }

    fn reschedule(&mut self, env: &mut Env) {
        env.cancel(self.slot);
        if self.running() {
            let t = self.period.after(self.origin, self.base_value as u64);
            env.schedule(self.slot, t);
        }
    }

    pub fn event(&mut self, env: &mut Env) {
        if !self.running() {
            return;
        }
        if self.raw_int && self.control & 2 != 0 {
            self.reset_request = true;
        }
        self.raw_int = true;
        self.base_value = self.load;
        self.origin = env.now;
        self.reschedule(env);
    }

    pub fn read(&mut self, env: &mut Env, off: u32, size: u8) -> u32 {
        let v = match off & !3 {
            0x000 => self.load,
            0x004 => self.value(env.now),
            0x008 => self.control,
            0x010 => self.raw_int as u32,
            0x014 => (self.raw_int && self.control & 1 != 0) as u32,
            0xC00 => (self.lock != 0x1ACC_E551) as u32,
            0xFE0 => 0x24,
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
        if self.lock != 0x1ACC_E551 && (off & !3) != 0xC00 && self.lock != 0 {
            return;
        }
        match off & !3 {
            0x000 => {
                self.load = widen(self.load, off, size, v);
                self.base_value = self.load;
                self.origin = env.now;
                self.raw_int = false;
            }
            0x008 => {
                let cur = self.value(env.now);
                self.control = widen(self.control, off, size, v) & 3;
                self.base_value = cur;
                self.origin = env.now;
            }
            0x00C => {
                self.raw_int = false;
                self.base_value = self.load;
                self.origin = env.now;
            }
            0xC00 => self.lock = if v == 0x1ACC_E551 { 0x1ACC_E551 } else { 1 },
            _ => {}
        }
        self.reschedule(env);
    }
}
