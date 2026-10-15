//! MPS3 audio I2S (`audio_i2s_mps3_reg_map.h`): transmit and receive FIFOs of stereo 16-bit
//! samples, a clock divider, water-level interrupts; the receiver is fed by the microphone
//! source of the board, the transmitter drives the speaker.

use crate::env::{narrow, slot, widen, Env};
use cs320_core::time::FracPeriod;
use cs320_core::Ticks;
use std::collections::VecDeque;

pub const FIFO_DEPTH: usize = 8;

pub struct AudioI2s {
    pub control: u32,
    pub error: u32,
    pub divide: u32,
    tx: VecDeque<u32>,
    rx: VecDeque<u32>,
    /// Sample period and the time of the next sample edge.
    period: FracPeriod,
    next: Ticks,
    running: bool,
    mic_buf: Vec<i16>,
    mic_pos: usize,
    spk_buf: Vec<i16>,
    spk_start: Ticks,
}

impl AudioI2s {
    pub fn new() -> Self {
        AudioI2s {
            control: 0,
            error: 0,
            divide: 0,
            tx: VecDeque::new(),
            rx: VecDeque::new(),
            period: FracPeriod::from_hz(48_000),
            next: 0,
            running: false,
            mic_buf: Vec::new(),
            mic_pos: 0,
            spk_buf: Vec::new(),
            spk_start: 0,
        }
    }

    pub fn reset(&mut self) {
        self.control = 0;
        self.error = 0;
        self.divide = 0;
        self.tx.clear();
        self.rx.clear();
        self.running = false;
        self.mic_buf.clear();
        self.mic_pos = 0;
        self.spk_buf.clear();
    }

    /// Sample rate from the divider: LRCLK = MCLK / (2 · LRDIV); 48 kHz at LRDIV 128.
    pub fn sample_rate(&self, env: &Env) -> u32 {
        let div = self.divide.max(1) as u64;
        (env.clocks.audio_mclk_hz / (2 * div)).max(1) as u32
    }

    fn tx_irq_cond(&self) -> bool {
        let water = ((self.control >> 8) & 0xF) as usize;
        self.tx.len() <= water
    }

    fn rx_irq_cond(&self) -> bool {
        let water = ((self.control >> 12) & 0xF) as usize;
        self.rx.len() > water
    }

    pub fn irq(&self) -> bool {
        (self.control & 2 != 0 && self.control & 1 != 0 && self.tx_irq_cond()) || (self.control & 8 != 0 && self.control & 4 != 0 && self.rx_irq_cond())
    }

    fn status(&self) -> u32 {
        (self.tx_irq_cond() as u32)
            | ((self.rx_irq_cond() as u32) << 1)
            | ((self.tx.is_empty() as u32) << 2)
            | (((self.tx.len() >= FIFO_DEPTH) as u32) << 3)
            | ((self.rx.is_empty() as u32) << 4)
            | (((self.rx.len() >= FIFO_DEPTH) as u32) << 5)
    }

    fn update_clock(&mut self, env: &mut Env) {
        let enabled = self.control & 5 != 0;
        if enabled && !self.running {
            self.running = true;
            self.period = FracPeriod::from_hz(self.sample_rate(env) as u64);
            self.next = env.now;
            self.mic_buf.clear();
            self.mic_pos = 0;
            self.spk_buf.clear();
            self.spk_start = env.now;
            env.schedule(slot::I2S, self.next);
        } else if !enabled && self.running {
            self.running = false;
            env.cancel(slot::I2S);
            self.flush_speaker(env);
        }
    }

    fn flush_speaker(&mut self, env: &mut Env) {
        if !self.spk_buf.is_empty() {
            let rate = self.sample_rate(env);
            env.board.speaker_samples(self.spk_start, rate, 2, &self.spk_buf);
            self.spk_buf.clear();
        }
    }

    /// One sample period elapsed.
    pub fn event(&mut self, env: &mut Env) {
        if !self.running {
            return;
        }
        let rate = self.sample_rate(env);
        if self.control & 4 != 0 {
            // receive a stereo sample from the microphone
            if self.mic_pos + 2 > self.mic_buf.len() {
                self.mic_buf.resize(2 * 256, 0);
                env.board.mic_samples(self.next, rate, 2, &mut self.mic_buf);
                self.mic_pos = 0;
            }
            let l = self.mic_buf[self.mic_pos] as u16 as u32;
            let r = self.mic_buf[self.mic_pos + 1] as u16 as u32;
            self.mic_pos += 2;
            if self.rx.len() >= FIFO_DEPTH {
                self.error |= 2;
            } else {
                self.rx.push_back((l << 16) | r);
            }
        }
        if self.control & 1 != 0 {
            match self.tx.pop_front() {
                Some(s) => {
                    if self.spk_buf.is_empty() {
                        self.spk_start = self.next;
                    }
                    self.spk_buf.push((s >> 16) as i16);
                    self.spk_buf.push(s as i16);
                }
                None => {
                    self.error |= 1;
                    if self.spk_buf.is_empty() {
                        self.spk_start = self.next;
                    }
                    self.spk_buf.push(0);
                    self.spk_buf.push(0);
                }
            }
            if self.spk_buf.len() >= 2 * 256 {
                self.flush_speaker(env);
            }
        }
        self.next = self.period.after(self.next, 1);
        env.schedule(slot::I2S, self.next);
    }

    pub fn read(&mut self, _env: &mut Env, off: u32, size: u8, debug: bool) -> u32 {
        let v = match off & !3 {
            0x000 => self.control,
            0x004 => self.status(),
            0x008 => self.error,
            0x00C => self.divide,
            0x014 => {
                if debug {
                    self.rx.front().copied().unwrap_or(0)
                } else {
                    self.rx.pop_front().unwrap_or(0)
                }
            }
            _ => 0,
        };
        narrow(v, off, size)
    }

    pub fn write(&mut self, env: &mut Env, off: u32, size: u8, v: u32) {
        match off & !3 {
            0x000 => {
                self.control = widen(self.control, off, size, v) & 0x0003_FF0F;
                if self.control & (1 << 16) != 0 {
                    self.tx.clear();
                    self.rx.clear();
                }
                self.update_clock(env);
            }
            0x008 => self.error &= !v,
            0x00C => {
                self.divide = widen(self.divide, off, size, v) & 0x3FF;
                if self.running {
                    self.period = FracPeriod::from_hz(self.sample_rate(env) as u64);
                }
            }
            0x010 => {
                if self.tx.len() >= FIFO_DEPTH {
                    self.error |= 1;
                } else {
                    self.tx.push_back(v);
                }
            }
            _ => {}
        }
    }
}

impl Default for AudioI2s {
    fn default() -> Self {
        Self::new()
    }
}
