//! The simulator's camera capture peripheral (`CAM`), a simple alternative to the Mali-C55
//! ISP of the FVP: it writes one frame per period into a buffer in memory and raises an
//! interrupt. Register map (base 0x4840_0000, Secure alias 0x5840_0000, IRQ 132):
//!
//! | Offset | Register | Bits |
//! |:--|:--|:--|
//! | 0x000 | CTRL | 0 ENABLE, 1 CONTINUOUS (else one frame per enable), 2 SNAPSHOT (write 1: capture one frame now) |
//! | 0x004 | STATUS | 0 ACTIVE, 1 SOURCE (a frame source is connected) |
//! | 0x008 | WIDTH | pixels (1..4096) |
//! | 0x00C | HEIGHT | pixels (1..4096) |
//! | 0x010 | FORMAT | 0 GRAY8, 1 RGB565, 2 RGB888, 3 ARGB8888 (0xAARRGGBB words), 4 NV12, 5 YUV420 |
//! | 0x014 | FRAME_RATE | frames per second (1..240) |
//! | 0x018 | BUFFER | frame buffer address (4-byte aligned) |
//! | 0x01C | STRIDE | bytes per line, 0 = packed |
//! | 0x020 | IRQ_STATUS | 0 FRAME, 1 OVERRUN (a frame was written before the previous was acknowledged); write 1 to clear |
//! | 0x024 | IRQ_ENABLE | as IRQ_STATUS |
//! | 0x028 | FRAME_COUNT | frames captured since enable |
//! | 0x02C | MAX_WIDTH | 4096 (read only) |
//! | 0x030 | MAX_HEIGHT | 4096 (read only) |
//! | 0xFF0 | ID | 0x314D_4143 "CAM1" |

use crate::board::PixelFormat;
use crate::env::{narrow, slot, widen, Env};
use cs320_core::time::TICKS_PER_SEC;
use cs320_core::Ticks;

pub const MAX_DIM: u32 = 4096;

pub struct Camera {
    pub ctrl: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub frame_rate: u32,
    pub buffer: u32,
    pub stride: u32,
    pub irq_status: u32,
    pub irq_enable: u32,
    pub frame_count: u32,
    pub source: bool,
    next: Ticks,
    buf: Vec<u8>,
}

impl Camera {
    pub fn new() -> Self {
        Camera {
            ctrl: 0,
            width: 640,
            height: 480,
            format: 2,
            frame_rate: 30,
            buffer: 0,
            stride: 0,
            irq_status: 0,
            irq_enable: 0,
            frame_count: 0,
            source: false,
            next: 0,
            buf: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        let src = self.source;
        *self = Camera::new();
        self.source = src;
    }

    pub fn irq(&self) -> bool {
        self.irq_status & self.irq_enable != 0
    }

    pub fn pixel_format(&self) -> PixelFormat {
        match self.format {
            0 => PixelFormat::Gray8,
            1 => PixelFormat::Rgb565,
            2 => PixelFormat::Rgb888,
            3 => PixelFormat::Argb8888,
            4 => PixelFormat::Nv12,
            _ => PixelFormat::Yuv420,
        }
    }

    fn period(&self) -> Ticks {
        TICKS_PER_SEC / self.frame_rate.clamp(1, 240) as u64
    }

    fn restart(&mut self, env: &mut Env) {
        if self.ctrl & 1 != 0 {
            self.next = env.now + self.period();
            env.schedule(slot::CAM, self.next);
        } else {
            env.cancel(slot::CAM);
        }
    }

    fn capture(&mut self, env: &mut Env) {
        let (w, h) = (self.width.clamp(1, MAX_DIM), self.height.clamp(1, MAX_DIM));
        let fmt = self.pixel_format();
        let n = fmt.frame_bytes(w, h);
        self.buf.resize(n, 0);
        self.source = env.board.camera_frame(env.now, 0, w, h, fmt, &mut self.buf);
        if self.stride == 0 || matches!(fmt, PixelFormat::Nv12 | PixelFormat::Yuv420) {
            env.mem.write_bytes(self.buffer, &self.buf);
        } else {
            let line = (w * fmt.bits() / 8) as usize;
            for y in 0..h as usize {
                env.mem.write_bytes(self.buffer.wrapping_add(y as u32 * self.stride), &self.buf[y * line..(y + 1) * line]);
            }
        }
        if self.irq_status & 1 != 0 {
            self.irq_status |= 2;
        }
        self.irq_status |= 1;
        self.frame_count = self.frame_count.wrapping_add(1);
    }

    pub fn event(&mut self, env: &mut Env) {
        if self.ctrl & 1 == 0 {
            return;
        }
        self.capture(env);
        if self.ctrl & 2 != 0 {
            self.next += self.period();
            if self.next <= env.now {
                self.next = env.now + self.period();
            }
            env.schedule(slot::CAM, self.next);
        } else {
            self.ctrl &= !1;
        }
    }

    pub fn read(&self, off: u32, size: u8) -> u32 {
        let v = match off & !3 {
            0x000 => self.ctrl & 3,
            0x004 => (self.ctrl & 1) | ((self.source as u32) << 1),
            0x008 => self.width,
            0x00C => self.height,
            0x010 => self.format,
            0x014 => self.frame_rate,
            0x018 => self.buffer,
            0x01C => self.stride,
            0x020 => self.irq_status,
            0x024 => self.irq_enable,
            0x028 => self.frame_count,
            0x02C => MAX_DIM,
            0x030 => MAX_DIM,
            0xFF0 => 0x314D_4143,
            _ => 0,
        };
        narrow(v, off, size)
    }

    pub fn write(&mut self, env: &mut Env, off: u32, size: u8, v: u32) {
        match off & !3 {
            0x000 => {
                let was = self.ctrl & 1;
                self.ctrl = widen(self.ctrl, off, size, v) & 3;
                if v & 4 != 0 {
                    self.capture(env);
                }
                if was != self.ctrl & 1 {
                    if self.ctrl & 1 != 0 {
                        self.frame_count = 0;
                    }
                    self.restart(env);
                }
            }
            0x008 => self.width = widen(self.width, off, size, v).clamp(1, MAX_DIM),
            0x00C => self.height = widen(self.height, off, size, v).clamp(1, MAX_DIM),
            0x010 => self.format = widen(self.format, off, size, v) & 7,
            0x014 => {
                self.frame_rate = widen(self.frame_rate, off, size, v).clamp(1, 240);
                if self.ctrl & 1 != 0 {
                    self.restart(env);
                }
            }
            0x018 => self.buffer = widen(self.buffer, off, size, v) & !3,
            0x01C => self.stride = widen(self.stride, off, size, v),
            0x020 => self.irq_status &= !v,
            0x024 => self.irq_enable = widen(self.irq_enable, off, size, v) & 3,
            _ => {}
        }
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self::new()
    }
}
