//! DMA-350 (`dma350_regdef.h`): the register files of the controller and its two channels;
//! a channel command executes a one-dimensional copy or fill at once (the 2D, wrap and
//! template modes are reported and treated as 1D).

use crate::env::{coresight_id, narrow, widen, Env};
use std::collections::HashMap;

pub const NUM_CHANNELS: usize = 2;

const DMA_PIDR: [u32; 8] = [0x3A, 0xB3, 0x0B, 0x00, 0x04, 0, 0, 0];
const DMA_CIDR: [u32; 4] = [0x0D, 0xF0, 0x05, 0xB1];

#[derive(Default, Clone)]
pub struct Channel {
    pub regs: HashMap<u32, u32>,
    pub status: u32,
    pub intren: u32,
}

pub struct Dma350 {
    pub top: HashMap<u32, u32>,
    pub ch: [Channel; NUM_CHANNELS],
}

impl Dma350 {
    pub fn new() -> Self {
        let mut d = Dma350 { top: HashMap::new(), ch: Default::default() };
        d.reset();
        d
    }

    pub fn reset(&mut self) {
        self.top.clear();
        for c in &mut self.ch {
            *c = Channel::default();
            // CH_BUILDCFG0: 64-bit data, 32-bit address, 2 FIFO entries; CH_BUILDCFG1: extended features
            c.regs.insert(0xF8, 0x0020_0620);
            c.regs.insert(0xFC, 0x0000_0001);
            c.regs.insert(0xC8, 0x0B01_043B);
        }
        self.top.insert(0xFB0, 0x0000_0012 | (2 << 4));
        self.top.insert(0xFB4, 0x0000_0808);
        self.top.insert(0xFC8, 0x0B01_043B);
    }

    /// Interrupt lines: (secure combined, non-secure combined, [channel 0, channel 1]).
    pub fn irq_channel(&self, i: usize) -> bool {
        let c = &self.ch[i];
        (c.status & (1 << 16) != 0 && c.intren & 1 != 0) || (c.status & (1 << 17) != 0 && c.intren & 2 != 0)
    }

    pub fn read(&mut self, _env: &mut Env, off: u32, size: u8) -> u32 {
        let o = off & 0x1FFC;
        let v = if o >= 0x1000 {
            let i = ((o - 0x1000) / 0x100) as usize;
            let r = o & 0xFF;
            if i >= NUM_CHANNELS {
                0
            } else {
                match r {
                    0x00 => 0,
                    0x04 => self.ch[i].status,
                    0x08 => self.ch[i].intren,
                    _ => self.ch[i].regs.get(&r).copied().unwrap_or(0),
                }
            }
        } else {
            match o {
                0x000 => (0..NUM_CHANNELS).filter(|i| self.irq_channel(*i)).fold(0, |a, i| a | (1 << i)),
                0x100 => (0..NUM_CHANNELS).filter(|i| self.irq_channel(*i)).fold(0, |a, i| a | (1 << i)),
                _ => coresight_id(o, &DMA_PIDR, &DMA_CIDR).unwrap_or_else(|| self.top.get(&o).copied().unwrap_or(0)),
            }
        };
        narrow(v, off, size)
    }

    pub fn write(&mut self, env: &mut Env, off: u32, size: u8, v: u32) {
        let o = off & 0x1FFC;
        if o >= 0x1000 {
            let i = ((o - 0x1000) / 0x100) as usize;
            let r = o & 0xFF;
            if i >= NUM_CHANNELS {
                return;
            }
            match r {
                0x00 => {
                    if v & 1 != 0 {
                        self.run(env, i);
                    }
                    if v & 2 != 0 {
                        // CLEARCMD
                        self.ch[i].status = 0;
                    }
                }
                0x04 => self.ch[i].status &= !(v & 0x003F_0000),
                0x08 => self.ch[i].intren = widen(self.ch[i].intren, off, size, v),
                _ => {
                    let old = self.ch[i].regs.get(&r).copied().unwrap_or(0);
                    self.ch[i].regs.insert(r, widen(old, off, size, v));
                }
            }
        } else if o < 0xFB0 {
            let old = self.top.get(&o).copied().unwrap_or(0);
            self.top.insert(o, widen(old, off, size, v));
        }
    }

    fn run(&mut self, env: &mut Env, i: usize) {
        let g = |c: &Channel, r: u32| c.regs.get(&r).copied().unwrap_or(0);
        let c = &self.ch[i];
        let ctrl = g(c, 0x0C);
        let transize = 1usize << (ctrl & 7);
        let xtype = (ctrl >> 9) & 7;
        let ytype = (ctrl >> 12) & 7;
        let src = g(c, 0x10);
        let dst = g(c, 0x18);
        let xsize = g(c, 0x20);
        let (srcx, desx) = ((xsize & 0xFFFF) as usize, (xsize >> 16) as usize);
        let xinc = g(c, 0x30);
        let (srcinc, desinc) = ((xinc & 0xFFFF) as i16 as i64, (xinc >> 16) as i16 as i64);
        let fill = g(c, 0x38);
        if ytype != 0 {
            env.warn.warn(env.now, format!("DMA-350 channel {i}: 2D transfers are executed as 1D"));
        }
        let n = if xtype == 3 { desx } else { srcx.min(desx) };
        let mut buf = vec![0u8; transize];
        for k in 0..n {
            if xtype == 3 {
                buf.copy_from_slice(&fill.to_le_bytes()[..transize.min(4)].iter().cycle().take(transize).copied().collect::<Vec<u8>>());
            } else {
                let a = (src as i64 + k as i64 * srcinc * transize as i64) as u32;
                env.mem.read_bytes(a, &mut buf);
            }
            let d = (dst as i64 + k as i64 * desinc * transize as i64) as u32;
            env.mem.write_bytes(d, &buf);
        }
        let c = &mut self.ch[i];
        c.status |= 1 << 16; // STAT_DONE
        // the source and destination addresses advance
        c.regs.insert(0x10, (src as i64 + n as i64 * srcinc * transize as i64) as u32);
        c.regs.insert(0x18, (dst as i64 + n as i64 * desinc * transize as i64) as u32);
    }
}

impl Default for Dma350 {
    fn default() -> Self {
        Self::new()
    }
}
