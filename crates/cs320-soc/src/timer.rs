//! The system counter (control and read frames), the Armv8-M system timers and the CMSDK
//! slow-clock timer.

use crate::env::{coresight_id, narrow, slot, widen, Env};
use cs320_core::time::{FracPeriod, TICKS_PER_SEC};
use cs320_core::{Slot, Ticks};

const CNT_PIDR: [u32; 8] = [0x19, 0xB0, 0x0B, 0x00, 0x04, 0, 0, 0];
const CNT_CIDR: [u32; 4] = [0x0D, 0xF0, 0x05, 0xB1];

/// System counter: a 64-bit counter at the counter clock, with the control frame
/// (`syscounter_armv8-m_cntrl_reg_map.h`) and the read frames.
pub struct SysCounter {
    pub cntcr: u32,
    /// Counter value at `origin` (when disabled: the frozen value).
    base: u64,
    origin: Ticks,
    pub cntscr0: u32,
    pub cntscr1: u32,
    period: FracPeriod,
    hz: u64,
}

impl SysCounter {
    pub fn new(hz: u64) -> Self {
        SysCounter { cntcr: 0, base: 0, origin: 0, cntscr0: 0, cntscr1: 0, period: FracPeriod::from_hz(hz), hz }
    }

    pub fn reset(&mut self, now: Ticks, start_enabled: bool) {
        // the FVP starts the counter at reset (refcounter.non_arch_start_at_default); CNTCR
        // reads 0x101 (EN and FCREQ0)
        self.cntcr = if start_enabled { 0x101 } else { 0x100 };
        self.base = 0;
        self.origin = now;
    }

    pub fn enabled(&self) -> bool {
        self.cntcr & 1 != 0
    }

    pub fn value(&self, now: Ticks) -> u64 {
        if self.enabled() {
            self.base.wrapping_add(self.period.count(self.origin, now))
        } else {
            self.base
        }
    }

    fn set_value(&mut self, now: Ticks, v: u64) {
        self.base = v;
        self.origin = now;
    }

    /// Time at which the counter reaches `target` (`None` when stopped or already past).
    pub fn time_of(&self, now: Ticks, target: u64) -> Option<Ticks> {
        if !self.enabled() {
            return None;
        }
        let cur = self.value(now);
        if target <= cur {
            return Some(now);
        }
        let n = target - cur;
        // ticks of the counter period, from the origin of the current count
        let elapsed = self.period.count(self.origin, now);
        Some(self.period.after(self.origin, elapsed + n).max(now + 1))
    }

    pub fn hz(&self) -> u64 {
        self.hz
    }

    pub fn read_ctrl(&mut self, env: &mut Env, off: u32, size: u8) -> u32 {
        let v = match off & !3 {
            0x000 => self.cntcr,
            0x004 => 0x100,          // CNTSR: FCACK0
            0x020 => self.hz as u32, // CNTFID0
            0x008 => self.value(env.now) as u32,
            0x00C => (self.value(env.now) >> 32) as u32,
            0x010 | 0x0D0 => self.cntscr0,
            0x0D4 => self.cntscr1,
            0x01C => 0,
            o => coresight_id(o, &CNT_PIDR, &CNT_CIDR).unwrap_or(0),
        };
        narrow(v, off, size)
    }

    pub fn write_ctrl(&mut self, env: &mut Env, off: u32, size: u8, v: u32) -> bool {
        let now = env.now;
        match off & !3 {
            0x000 => {
                let was = self.enabled();
                let cur = self.value(now);
                self.cntcr = (widen(self.cntcr, off, size, v) & 0x1F) | 0x100;
                if was != self.enabled() {
                    self.set_value(now, cur);
                }
                if v & (1 << 4) != 0 {
                    // INTRCLR
                }
                true
            }
            0x008 => {
                let cur = self.value(now);
                self.set_value(now, (cur & 0xFFFF_FFFF_0000_0000) | widen(cur as u32, off, size, v) as u64);
                true
            }
            0x00C => {
                let cur = self.value(now);
                self.set_value(now, (cur & 0xFFFF_FFFF) | ((widen((cur >> 32) as u32, off, size, v) as u64) << 32));
                true
            }
            0x010 | 0x0D0 => {
                self.cntscr0 = widen(self.cntscr0, off, size, v);
                false
            }
            0x0D4 => {
                self.cntscr1 = widen(self.cntscr1, off, size, v);
                false
            }
            _ => false,
        }
    }

    pub fn read_frame(&mut self, env: &mut Env, off: u32, size: u8) -> u32 {
        let v = match off & !3 {
            0x000 => self.value(env.now) as u32,
            0x004 => (self.value(env.now) >> 32) as u32,
            o => coresight_id(o, &CNT_PIDR, &CNT_CIDR).unwrap_or(0),
        };
        narrow(v, off, size)
    }
}

/// Armv8-M system timer frame (`systimer_armv8-m_reg_map.h`): compare value (CVAL), timer
/// value (TVAL), auto-increment, interrupt mask and status.
pub struct SysTimer {
    pub n: usize,
    pub cntfrq: u32,
    pub cval: u64,
    pub ctl: u32,
    pub aival: u64,
    pub aival_reload: u32,
    pub aival_ctl: u32,
    /// ISTATUS: the condition was met (level), for the interrupt line.
    fired: bool,
}

impl SysTimer {
    pub fn new(n: usize) -> Self {
        SysTimer { n, cntfrq: 0, cval: 0, ctl: 0, aival: 0, aival_reload: 0, aival_ctl: 0, fired: false }
    }

    pub fn reset(&mut self) {
        self.cntfrq = 0;
        self.cval = 0;
        self.ctl = 0;
        self.aival = 0;
        self.aival_reload = 0;
        self.aival_ctl = 0;
        self.fired = false;
    }

    fn enabled(&self) -> bool {
        self.ctl & 1 != 0
    }

    /// ISTATUS: enabled and the counter reached the compare value.
    pub fn istatus(&self, cnt: u64) -> bool {
        self.enabled() && cnt >= self.cval
    }

    /// Interrupt line: ISTATUS and not masked (plus the auto-increment interrupt).
    pub fn irq(&self, cnt: u64) -> bool {
        (self.istatus(cnt) && self.ctl & 2 == 0) || (self.aival_ctl & 1 != 0 && self.fired && self.ctl & 2 == 0)
    }

    /// Schedules the next event (the compare match) for the interrupt line evaluation.
    pub fn reschedule(&mut self, env: &mut Env, counter: &SysCounter) {
        let sl = slot::SYSTIMER[self.n];
        env.cancel(sl);
        if !self.enabled() {
            return;
        }
        let cnt = counter.value(env.now);
        let target = if self.aival_ctl & 1 != 0 { self.aival } else { self.cval };
        if cnt >= target {
            self.fired = true;
            return;
        }
        if let Some(t) = counter.time_of(env.now, target) {
            env.schedule(sl, t);
        }
    }

    /// The compare value was reached.
    pub fn event(&mut self, env: &mut Env, counter: &SysCounter) {
        self.fired = true;
        if self.aival_ctl & 1 != 0 {
            // auto-increment: the next compare value is one reload later
            self.aival = self.aival.wrapping_add(self.aival_reload as u64);
            self.cval = self.aival;
        }
        let _ = env;
        let _ = counter;
    }

    pub fn read(&mut self, env: &mut Env, off: u32, size: u8, counter: &SysCounter) -> u32 {
        let cnt = counter.value(env.now);
        let v = match off & !3 {
            0x000 => cnt as u32,
            0x004 => (cnt >> 32) as u32,
            0x010 => self.cntfrq,
            0x020 => self.cval as u32,
            0x024 => (self.cval >> 32) as u32,
            0x028 => self.cval.wrapping_sub(cnt) as u32,
            0x02C => self.ctl & 3 | ((self.istatus(cnt) as u32) << 2),
            0x040 => self.aival as u32,
            0x044 => (self.aival >> 32) as u32,
            0x048 => self.aival_reload,
            0x04C => self.aival_ctl & 1 | ((self.fired as u32 & self.aival_ctl) << 1),
            0x050 => 1,
            o => coresight_id(o, &CNT_PIDR, &CNT_CIDR).unwrap_or(0),
        };
        narrow(v, off, size)
    }

    pub fn write(&mut self, env: &mut Env, off: u32, size: u8, v: u32, counter: &SysCounter) {
        let cnt = counter.value(env.now);
        match off & !3 {
            0x010 => self.cntfrq = widen(self.cntfrq, off, size, v),
            0x020 => {
                self.cval = (self.cval & 0xFFFF_FFFF_0000_0000) | widen(self.cval as u32, off, size, v) as u64;
                self.fired = false;
            }
            0x024 => {
                self.cval = (self.cval & 0xFFFF_FFFF) | ((widen((self.cval >> 32) as u32, off, size, v) as u64) << 32);
                self.fired = false;
            }
            0x028 => {
                let tval = widen(self.cval.wrapping_sub(cnt) as u32, off, size, v);
                self.cval = cnt.wrapping_add(tval as i32 as i64 as u64);
                self.fired = false;
            }
            0x02C => self.ctl = widen(self.ctl, off, size, v) & 3,
            0x048 => self.aival_reload = widen(self.aival_reload, off, size, v),
            0x04C => {
                let nv = widen(self.aival_ctl, off, size, v);
                if nv & 2 != 0 {
                    // IRQ_CLR
                    self.fired = false;
                }
                let was = self.aival_ctl & 1;
                self.aival_ctl = nv & 1;
                if was == 0 && nv & 1 != 0 {
                    self.aival = cnt.wrapping_add(self.aival_reload as u64);
                    self.cval = self.aival;
                }
            }
            _ => {}
        }
        self.reschedule(env, counter);
    }
}

/// CMSDK timer (the slow-clock timer): 32-bit down counter with reload and interrupt.
pub struct CmsdkTimer {
    pub ctrl: u32,
    pub reload: u32,
    pub intstatus: u32,
    base_value: u32,
    origin: Ticks,
    period: FracPeriod,
    slot: Slot,
    slowclk: bool,
}

impl CmsdkTimer {
    pub fn new(hz: u64, slot: Slot) -> Self {
        CmsdkTimer { ctrl: 0, reload: 0, intstatus: 0, base_value: 0, origin: 0, period: FracPeriod::from_hz(hz), slot, slowclk: false }
    }

    /// The SLOWCLK timer instance: the FVP resets it with CTRL = 8 and VALUE/RELOAD = 0xFFFF_FFFF.
    pub fn slowclk(mut self) -> Self {
        self.slowclk = true;
        self.reset(0);
        self
    }

    pub fn reset(&mut self, now: Ticks) {
        self.ctrl = if self.slowclk { 8 } else { 0 };
        self.reload = 0;
        self.intstatus = 0;
        self.base_value = if self.slowclk { 0xFFFF_FFFF } else { 0 };
        self.origin = now;
    }

    fn running(&self) -> bool {
        self.ctrl & 1 != 0 && self.ctrl & 2 == 0
    }

    pub fn value(&self, now: Ticks) -> u32 {
        if !self.running() {
            return self.base_value;
        }
        let elapsed = self.period.count(self.origin, now);
        if elapsed >= self.base_value as u64 {
            // wrapped: continues from the reload value
            let period = self.reload as u64 + 1;
            if period <= 1 {
                return 0;
            }
            let rem = (elapsed - self.base_value as u64) % period;
            (self.reload as u64 - rem) as u32
        } else {
            self.base_value - elapsed as u32
        }
    }

    pub fn irq(&self) -> bool {
        self.intstatus & 1 != 0 && self.ctrl & 8 != 0
    }

    fn reschedule(&mut self, env: &mut Env) {
        env.cancel(self.slot);
        if self.running() && self.base_value != 0 {
            let t = self.period.after(self.origin, self.base_value as u64);
            env.schedule(self.slot, t);
        }
    }

    pub fn event(&mut self, env: &mut Env) {
        // the counter reached zero: interrupt and reload
        self.intstatus |= 1;
        self.base_value = self.reload;
        self.origin = env.now;
        self.reschedule(env);
    }

    pub fn read(&mut self, env: &mut Env, off: u32, size: u8) -> u32 {
        let v = match off & !3 {
            0x000 => self.ctrl,
            0x004 => self.value(env.now),
            0x008 => self.reload,
            0x00C => self.intstatus,
            0xFE0 => 0x22,
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
        let now = env.now;
        match off & !3 {
            0x000 => {
                let cur = self.value(now);
                self.ctrl = widen(self.ctrl, off, size, v) & 0xF;
                self.base_value = cur;
                self.origin = now;
            }
            0x004 => {
                self.base_value = widen(self.value(now), off, size, v);
                self.origin = now;
            }
            0x008 => {
                self.reload = widen(self.reload, off, size, v);
                if !self.running() {
                    self.base_value = self.reload;
                }
            }
            0x00C => self.intstatus &= !(v & 1),
            _ => {}
        }
        self.reschedule(env);
    }
}

/// Ticks of one period of `hz` (helper for callers).
pub fn period_ticks(hz: u64) -> Ticks {
    TICKS_PER_SEC / hz.max(1)
}
