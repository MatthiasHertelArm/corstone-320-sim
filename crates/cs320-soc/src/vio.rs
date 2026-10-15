//! Virtual I/O of Arm Virtual Hardware (`arm_vio.h`): signal output (LEDs), signal input
//! (buttons) and 64 values.

use crate::env::{narrow, widen, Env};

pub struct Vio {
    pub out_mask: u32,
    pub signal_out: u32,
    pub in_mask: u32,
    pub values: [i32; 64],
}

impl Vio {
    pub fn new() -> Self {
        Vio { out_mask: 0, signal_out: 0, in_mask: 0, values: [0; 64] }
    }

    pub fn reset(&mut self) {
        self.out_mask = 0;
        self.signal_out = 0;
        self.in_mask = 0;
        self.values = [0; 64];
    }

    pub fn read(&mut self, env: &mut Env, off: u32, size: u8) -> u32 {
        let v = match off & !3 {
            0x000 => self.out_mask,
            0x004 => self.signal_out,
            0x008 => self.in_mask,
            0x00C => env.board.vio_signal_in(env.now) & self.in_mask,
            0x010..=0x10C => {
                let i = ((off & !3) - 0x010) as usize / 4;
                match env.board.vio_value_in(env.now, i) {
                    Some(v) => v as u32,
                    None => self.values[i] as u32,
                }
            }
            _ => 0,
        };
        narrow(v, off, size)
    }

    pub fn write(&mut self, env: &mut Env, off: u32, size: u8, v: u32) {
        match off & !3 {
            0x000 => self.out_mask = widen(self.out_mask, off, size, v),
            0x004 => {
                let s = widen(self.signal_out, off, size, v);
                self.signal_out = (self.signal_out & !self.out_mask) | (s & self.out_mask);
                env.board.vio_signal_out(env.now, self.signal_out);
            }
            0x008 => self.in_mask = widen(self.in_mask, off, size, v),
            0x010..=0x10C => {
                let i = ((off & !3) - 0x010) as usize / 4;
                self.values[i] = widen(self.values[i] as u32, off, size, v) as i32;
                env.board.vio_value_out(env.now, i, self.values[i]);
            }
            _ => {}
        }
    }
}

impl Default for Vio {
    fn default() -> Self {
        Self::new()
    }
}
