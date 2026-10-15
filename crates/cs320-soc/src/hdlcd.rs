//! PL370 HDLCD display controller (`hdlcd_reg_map.h`): fetches the frame buffer once per
//! frame period, converts it with the pixel-format and colour-select registers, hands the
//! frame to the board and raises the DMA_END and VSYNC interrupts.

use crate::board::Frame;
use crate::env::{narrow, slot, widen, Env};
use cs320_core::time::{from_secs, TICKS_PER_SEC};
use cs320_core::Ticks;

pub const INT_DMA_END: u32 = 1 << 0;
pub const INT_BUS_ERROR: u32 = 1 << 1;
pub const INT_VSYNC: u32 = 1 << 2;
pub const INT_UNDERRUN: u32 = 1 << 3;

/// Largest frame the model scans out (4096 × 4096 pixels).
pub const MAX_DIM: u32 = 4096;

pub struct Hdlcd {
    pub int_raw: u32,
    pub int_mask: u32,
    pub user_out: u32,
    pub fb_base: u32,
    pub fb_line_length: u32,
    pub fb_line_count: u32,
    pub fb_line_pitch: u32,
    pub bus_options: u32,
    pub timing: [u32; 8],
    pub polarities: u32,
    pub command: u32,
    pub pixel_format: u32,
    pub select: [u32; 3],
    /// Frames per second forced by the `force_frame_rate` parameter (0 = from the timing).
    pub force_frame_rate: u32,
    /// Pixel clock for the timing-derived frame rate.
    pub pixel_clock_hz: u64,
    pub frames: u64,
    next_frame: Ticks,
    buf: Vec<u8>,
}

impl Hdlcd {
    pub fn new() -> Self {
        Hdlcd {
            int_raw: 0,
            int_mask: 0,
            user_out: 0,
            fb_base: 0,
            fb_line_length: 0,
            fb_line_count: 0,
            fb_line_pitch: 0,
            bus_options: 0,
            timing: [0; 8],
            polarities: 0,
            command: 0,
            pixel_format: 0,
            select: [0; 3],
            force_frame_rate: 50,
            pixel_clock_hz: 25_000_000,
            frames: 0,
            next_frame: 0,
            buf: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        let (rate, clk) = (self.force_frame_rate, self.pixel_clock_hz);
        *self = Hdlcd::new();
        self.force_frame_rate = rate;
        self.pixel_clock_hz = clk;
    }

    pub fn irq(&self) -> bool {
        self.int_raw & self.int_mask != 0
    }

    pub fn enabled(&self) -> bool {
        self.command & 1 != 0
    }

    fn bytes_per_pixel(&self) -> u32 {
        ((self.pixel_format >> 3) & 3) + 1
    }

    /// Width and height of the scanned-out image.
    pub fn size(&self) -> (u32, u32) {
        let bpp = self.bytes_per_pixel();
        let from_timing = self.timing[6] + 1;
        let from_length = self.fb_line_length / bpp;
        let w = if self.timing[6] != 0 { from_timing.min(from_length.max(1)) } else { from_length };
        let h = self.fb_line_count + 1;
        (w.clamp(1, MAX_DIM), h.clamp(1, MAX_DIM))
    }

    fn frame_period(&self) -> Ticks {
        if self.force_frame_rate != 0 {
            return TICKS_PER_SEC / self.force_frame_rate as u64;
        }
        let h_total: u64 = (self.timing[4] + 1 + self.timing[5] + 1 + self.timing[6] + 1 + self.timing[7] + 1) as u64;
        let v_total: u64 = (self.timing[0] + 1 + self.timing[1] + 1 + self.timing[2] + 1 + self.timing[3] + 1) as u64;
        let pixels = h_total * v_total;
        from_secs(pixels as f64 / self.pixel_clock_hz.max(1) as f64).max(TICKS_PER_SEC / 240)
    }

    fn restart(&mut self, env: &mut Env) {
        if self.enabled() {
            self.next_frame = env.now + self.frame_period();
            env.schedule(slot::HDLCD, self.next_frame);
        } else {
            env.cancel(slot::HDLCD);
        }
    }

    /// Converts the frame buffer pixel `p` (as a little-endian word of `bpp` bytes) to RGBA.
    fn convert(&self, p: u32) -> [u8; 4] {
        let chan = |sel: u32| -> u8 {
            let size = (sel >> 8) & 0xF;
            let off = sel & 0x1F;
            let default = ((sel >> 16) & 0xFF) as u8;
            if size == 0 {
                return default;
            }
            let v = (p >> off) & ((1u32 << size) - 1);
            // scale to 8 bits
            if size >= 8 {
                (v >> (size - 8)) as u8
            } else {
                ((v * 255) / ((1 << size) - 1)) as u8
            }
        };
        [chan(self.select[0]), chan(self.select[1]), chan(self.select[2]), 255]
    }

    /// Scans out one frame.
    pub fn event(&mut self, env: &mut Env) {
        if !self.enabled() {
            return;
        }
        let (w, h) = self.size();
        let bpp = self.bytes_per_pixel();
        let line_bytes = (w * bpp) as usize;
        let pitch = self.fb_line_pitch as i32;
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        self.buf.resize(line_bytes, 0);
        let big_endian = self.pixel_format & (1 << 31) != 0;
        let mut ok = true;
        for y in 0..h {
            let addr = (self.fb_base as i64 + y as i64 * pitch as i64) as u32;
            ok &= env.mem.read_bytes(addr, &mut self.buf);
            let row = &mut rgba[(y * w * 4) as usize..((y + 1) * w * 4) as usize];
            for x in 0..w as usize {
                let b = &self.buf[x * bpp as usize..(x + 1) * bpp as usize];
                let mut p = 0u32;
                for (i, byte) in b.iter().enumerate() {
                    if big_endian {
                        p = (p << 8) | *byte as u32;
                    } else {
                        p |= (*byte as u32) << (8 * i);
                    }
                }
                row[x * 4..x * 4 + 4].copy_from_slice(&self.convert(p));
            }
        }
        if !ok {
            self.int_raw |= INT_BUS_ERROR;
        }
        self.frames += 1;
        self.int_raw |= INT_DMA_END | INT_VSYNC;
        env.board.display_frame(env.now, 0, Frame { width: w, height: h, rgba });
        self.next_frame += self.frame_period();
        if self.next_frame <= env.now {
            self.next_frame = env.now + self.frame_period();
        }
        env.schedule(slot::HDLCD, self.next_frame);
    }

    pub fn read(&self, off: u32, size: u8) -> u32 {
        let v = match off & !3 {
            0x000 => 0x1CDC_1000,
            0x010 => self.int_raw,
            0x018 => self.int_mask,
            0x01C => self.int_raw & self.int_mask,
            0x020 => self.user_out,
            0x100 => self.fb_base,
            0x104 => self.fb_line_length,
            0x108 => self.fb_line_count,
            0x10C => self.fb_line_pitch,
            0x110 => self.bus_options,
            0x200..=0x21C => self.timing[((off & !3) - 0x200) as usize / 4],
            0x220 => self.polarities,
            0x230 => self.command,
            0x240 => self.pixel_format,
            0x244..=0x24C => self.select[((off & !3) - 0x244) as usize / 4],
            _ => 0,
        };
        narrow(v, off, size)
    }

    pub fn write(&mut self, env: &mut Env, off: u32, size: u8, v: u32) {
        match off & !3 {
            0x010 => self.int_raw = widen(self.int_raw, off, size, v) & 0xF,
            0x014 => self.int_raw &= !v,
            0x018 => self.int_mask = widen(self.int_mask, off, size, v) & 0xF,
            0x020 => self.user_out = widen(self.user_out, off, size, v),
            0x100 => self.fb_base = widen(self.fb_base, off, size, v) & !7,
            0x104 => self.fb_line_length = widen(self.fb_line_length, off, size, v) & !7,
            0x108 => self.fb_line_count = widen(self.fb_line_count, off, size, v) & 0xFFF,
            0x10C => self.fb_line_pitch = widen(self.fb_line_pitch, off, size, v) & !7,
            0x110 => self.bus_options = widen(self.bus_options, off, size, v),
            0x200..=0x21C => {
                let i = ((off & !3) - 0x200) as usize / 4;
                self.timing[i] = widen(self.timing[i], off, size, v) & 0xFFF;
            }
            0x220 => self.polarities = widen(self.polarities, off, size, v) & 0x1F,
            0x230 => {
                let was = self.enabled();
                self.command = widen(self.command, off, size, v) & 1;
                if was != self.enabled() {
                    self.restart(env);
                }
            }
            0x240 => self.pixel_format = widen(self.pixel_format, off, size, v) & 0x8000_0018,
            0x244..=0x24C => {
                let i = ((off & !3) - 0x244) as usize / 4;
                self.select[i] = widen(self.select[i], off, size, v) & 0x00FF_0F1F;
            }
            _ => {}
        }
    }
}

impl Default for Hdlcd {
    fn default() -> Self {
        Self::new()
    }
}
