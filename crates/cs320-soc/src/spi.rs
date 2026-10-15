//! PL022 SSP (SPI) register stub: the transmit FIFO is always empty and not busy; data
//! written is echoed back in loop-back mode and otherwise dropped (no SPI devices are
//! modelled on the MPS4 board).

use crate::env::{narrow, widen};
use std::collections::VecDeque;

pub struct Pl022 {
    pub cr0: u32,
    pub cr1: u32,
    pub cpsr: u32,
    pub imsc: u32,
    pub ris: u32,
    pub dmacr: u32,
    rx: VecDeque<u16>,
}

impl Pl022 {
    pub fn new() -> Self {
        Pl022 { cr0: 0, cr1: 0, cpsr: 0, imsc: 0, ris: 0x8, dmacr: 0, rx: VecDeque::new() }
    }

    pub fn reset(&mut self) {
        *self = Pl022::new();
    }

    pub fn read(&mut self, off: u32, size: u8, debug: bool) -> u32 {
        let v = match off & !3 {
            0x000 => self.cr0,
            0x004 => self.cr1,
            0x008 => {
                if debug {
                    self.rx.front().copied().unwrap_or(0) as u32
                } else {
                    self.rx.pop_front().unwrap_or(0) as u32
                }
            }
            0x00C => 0x3 | if self.rx.is_empty() { 0 } else { 4 } | if self.rx.len() >= 8 { 8 } else { 0 },
            0x010 => self.cpsr,
            0x014 => self.imsc,
            0x018 => self.ris | if self.rx.is_empty() { 0 } else { 0x4 },
            0x01C => (self.ris | if self.rx.is_empty() { 0 } else { 0x4 }) & self.imsc,
            0x024 => self.dmacr,
            0xFE0 => 0x22,
            0xFE4 => 0x10,
            0xFE8 => 0x34,
            0xFEC => 0x00,
            0xFF0 => 0x0D,
            0xFF4 => 0xF0,
            0xFF8 => 0x05,
            0xFFC => 0xB1,
            _ => 0,
        };
        narrow(v, off, size)
    }

    pub fn write(&mut self, off: u32, size: u8, v: u32) {
        match off & !3 {
            0x000 => self.cr0 = widen(self.cr0, off, size, v) & 0xFFFF,
            0x004 => self.cr1 = widen(self.cr1, off, size, v) & 0xF,
            0x008 => {
                if self.cr1 & 1 != 0 {
                    // loop-back
                    if self.rx.len() < 8 {
                        self.rx.push_back(v as u16);
                    }
                } else if self.cr1 & 2 != 0 && self.rx.len() < 8 {
                    // a transfer with nothing connected reads all ones
                    self.rx.push_back(0xFFFF);
                }
            }
            0x010 => self.cpsr = widen(self.cpsr, off, size, v) & 0xFF,
            0x014 => self.imsc = widen(self.imsc, off, size, v) & 0xF,
            0x020 => self.ris &= !(v & 3),
            0x024 => self.dmacr = widen(self.dmacr, off, size, v) & 3,
            _ => {}
        }
    }
}

impl Default for Pl022 {
    fn default() -> Self {
        Self::new()
    }
}
