//! Virtual Streaming Interface of Arm Virtual Hardware (`arm_vsi.h`): a 1 MHz timer, a block
//! DMA and 64 user registers per instance. The user registers follow the AVH audio driver on
//! VSI0 (microphone in) and VSI1 (speaker out), and the AVH video driver on VSI4/VSI5
//! (camera in) and VSI6/VSI7 (display out).

use crate::board::{Frame, PixelFormat};
use crate::env::{narrow, slot, widen, Env};
use cs320_core::time::TICKS_PER_US;
use cs320_core::Ticks;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    AudioIn,
    AudioOut,
    VideoIn(usize),
    VideoOut(usize),
    None,
}

pub const TIMER_RUN: u32 = 1 << 0;
pub const TIMER_PERIODIC: u32 = 1 << 1;
pub const TIMER_TRIG_IRQ: u32 = 1 << 2;
pub const TIMER_TRIG_DMA: u32 = 1 << 3;
pub const DMA_ENABLE: u32 = 1 << 0;
pub const DMA_M2P: u32 = 1 << 1;

// video user registers
const R_MODE: usize = 0;
const R_CONTROL: usize = 1;
const R_STATUS: usize = 2;
const R_FILENAME_LEN: usize = 3;
const R_FILENAME_CHAR: usize = 4;
const R_FILENAME_VALID: usize = 5;
const R_FRAME_WIDTH: usize = 6;
const R_FRAME_HEIGHT: usize = 7;
const R_COLOR_FORMAT: usize = 8;
const R_FRAME_RATE: usize = 9;
const R_FRAME_INDEX: usize = 10;
const R_FRAME_COUNT: usize = 11;
const R_FRAME_COUNT_MAX: usize = 12;

pub struct Vsi {
    pub n: usize,
    pub role: Role,
    pub irq_enable: u32,
    pub irq_status: u32,
    pub timer_control: u32,
    pub timer_interval: u32,
    pub timer_count: u32,
    pub dma_control: u32,
    pub dma_address: u32,
    pub dma_block_size: u32,
    pub dma_block_num: u32,
    pub dma_block_index: u32,
    pub regs: [u32; 64],
    next: Ticks,
    filename: Vec<u8>,
    frame_buf: Vec<u8>,
    sample_buf: Vec<i16>,
    sample_time: Ticks,
}

impl Vsi {
    pub fn new(n: usize) -> Self {
        let role = match n {
            0 => Role::AudioIn,
            1 => Role::AudioOut,
            4 => Role::VideoIn(0),
            5 => Role::VideoIn(1),
            6 => Role::VideoOut(1),
            7 => Role::VideoOut(2),
            _ => Role::None,
        };
        Vsi {
            n,
            role,
            irq_enable: 0,
            irq_status: 0,
            timer_control: 0,
            timer_interval: 0,
            timer_count: 0,
            dma_control: 0,
            dma_address: 0,
            dma_block_size: 0,
            dma_block_num: 0,
            dma_block_index: 0,
            regs: [0; 64],
            next: 0,
            filename: Vec::new(),
            frame_buf: Vec::new(),
            sample_buf: Vec::new(),
            sample_time: 0,
        }
    }

    pub fn reset(&mut self) {
        let (n, role) = (self.n, self.role);
        *self = Vsi::new(n);
        self.role = role;
    }

    pub fn irq(&self) -> bool {
        self.irq_status & self.irq_enable != 0
    }

    fn timer_running(&self) -> bool {
        self.timer_control & TIMER_RUN != 0
    }

    fn restart_timer(&mut self, env: &mut Env) {
        let sl = slot::VSI[self.n];
        env.cancel(sl);
        if self.timer_running() {
            self.next = env.now + self.timer_interval.max(1) as u64 * TICKS_PER_US;
            env.schedule(sl, self.next);
        }
    }

    /// The timer interval elapsed.
    pub fn event(&mut self, env: &mut Env) {
        if !self.timer_running() {
            return;
        }
        self.timer_count = self.timer_count.wrapping_add(1);
        if self.timer_control & TIMER_TRIG_DMA != 0 && self.dma_control & DMA_ENABLE != 0 {
            self.dma_block(env);
        }
        if self.timer_control & TIMER_TRIG_IRQ != 0 {
            self.irq_status |= 1;
        }
        if self.timer_control & TIMER_PERIODIC != 0 {
            self.next += self.timer_interval.max(1) as u64 * TICKS_PER_US;
            if self.next <= env.now {
                self.next = env.now + self.timer_interval.max(1) as u64 * TICKS_PER_US;
            }
            env.schedule(slot::VSI[self.n], self.next);
        } else {
            self.timer_control &= !TIMER_RUN;
        }
    }

    fn video_format(&self) -> (u32, u32, PixelFormat) {
        let fmt = match self.regs[R_COLOR_FORMAT] {
            1 => PixelFormat::Gray8,
            2 => PixelFormat::Rgb888,
            3 => PixelFormat::Bgr565,
            4 => PixelFormat::Yuv420,
            5 => PixelFormat::Nv12,
            6 => PixelFormat::Nv21,
            _ => PixelFormat::Rgb888,
        };
        (self.regs[R_FRAME_WIDTH], self.regs[R_FRAME_HEIGHT], fmt)
    }

    /// One DMA block: peripheral to memory (input roles) or memory to peripheral (output).
    fn dma_block(&mut self, env: &mut Env) {
        let size = self.dma_block_size as usize;
        let addr = self.dma_address.wrapping_add(self.dma_block_index.wrapping_mul(self.dma_block_size));
        if size == 0 || self.dma_block_num == 0 {
            return;
        }
        if self.dma_control & DMA_M2P == 0 {
            // peripheral to memory
            self.frame_buf.resize(size, 0);
            match self.role {
                Role::AudioIn => {
                    let channels = self.regs[1].clamp(1, 32);
                    let bits = self.regs[2].clamp(8, 32);
                    let rate = self.regs[3].max(1);
                    let bytes = ((bits + 7) / 8) as usize;
                    let samples = size / bytes;
                    self.sample_buf.resize(samples, 0);
                    env.board.mic_samples(env.now, rate, channels, &mut self.sample_buf);
                    for (i, s) in self.sample_buf.iter().enumerate() {
                        let v = *s as i32;
                        match bytes {
                            1 => self.frame_buf[i] = ((v >> 8) + 128) as u8,
                            2 => self.frame_buf[2 * i..2 * i + 2].copy_from_slice(&(v as i16).to_le_bytes()),
                            3 => self.frame_buf[3 * i..3 * i + 3].copy_from_slice(&((v << 8) as u32).to_le_bytes()[..3]),
                            _ => self.frame_buf[4 * i..4 * i + 4].copy_from_slice(&((v << 16) as u32).to_le_bytes()),
                        }
                    }
                }
                Role::VideoIn(cam) => {
                    if self.regs[R_CONTROL] & 1 == 0 {
                        return;
                    }
                    let (w, h, fmt) = self.video_format();
                    let n = fmt.frame_bytes(w, h).min(size);
                    if !env.board.camera_frame(env.now, cam, w, h, fmt, &mut self.frame_buf[..n]) {
                        self.regs[R_STATUS] |= 1 << 5; // EOS: no source
                        self.irq_status |= 1 << 3;
                    }
                    self.regs[R_FRAME_INDEX] = self.dma_block_index;
                    self.regs[R_FRAME_COUNT] = self.regs[R_FRAME_COUNT].wrapping_add(1);
                }
                _ => self.frame_buf.fill(0),
            }
            env.mem.write_bytes(addr, &self.frame_buf);
        } else {
            self.frame_buf.resize(size, 0);
            env.mem.read_bytes(addr, &mut self.frame_buf);
            match self.role {
                Role::AudioOut => {
                    let channels = self.regs[1].clamp(1, 32);
                    let bits = self.regs[2].clamp(8, 32);
                    let rate = self.regs[3].max(1);
                    let bytes = ((bits + 7) / 8) as usize;
                    let samples = size / bytes;
                    self.sample_buf.clear();
                    for i in 0..samples {
                        let b = &self.frame_buf[i * bytes..(i + 1) * bytes];
                        let v: i16 = match bytes {
                            1 => ((b[0] as i16) - 128) << 8,
                            2 => i16::from_le_bytes([b[0], b[1]]),
                            3 => i16::from_le_bytes([b[1], b[2]]),
                            _ => i16::from_le_bytes([b[2], b[3]]),
                        };
                        self.sample_buf.push(v);
                    }
                    env.board.speaker_samples(self.sample_time, rate, channels, &self.sample_buf);
                    self.sample_time = env.now;
                }
                Role::VideoOut(display) => {
                    if self.regs[R_CONTROL] & 1 == 0 {
                        return;
                    }
                    let (w, h, fmt) = self.video_format();
                    if let Some(frame) = to_rgba(&self.frame_buf, w, h, fmt) {
                        env.board.display_frame(env.now, display, frame);
                    }
                    self.regs[R_FRAME_INDEX] = self.dma_block_index;
                    self.regs[R_FRAME_COUNT] = self.regs[R_FRAME_COUNT].wrapping_add(1);
                }
                _ => {}
            }
        }
        self.dma_block_index = (self.dma_block_index + 1) % self.dma_block_num.max(1);
    }

    pub fn read(&mut self, _env: &mut Env, off: u32, size: u8) -> u32 {
        let v = match off & !3 {
            0x000 => self.irq_enable,
            0x00C => self.irq_status,
            0x100 => self.timer_control,
            0x104 => self.timer_interval,
            0x108 => self.timer_count,
            0x200 => self.dma_control,
            0x204 => self.dma_address,
            0x208 => self.dma_block_size,
            0x20C => self.dma_block_num,
            0x210 => self.dma_block_index,
            0x300..=0x3FC => {
                let i = ((off & !3) - 0x300) as usize / 4;
                match (self.role, i) {
                    (Role::VideoIn(_) | Role::VideoOut(_), R_STATUS) => {
                        let active = self.regs[R_CONTROL] & 1 != 0;
                        (active as u32) | (self.regs[R_STATUS] & !1)
                    }
                    (Role::VideoIn(_) | Role::VideoOut(_), R_FILENAME_VALID) => 1,
                    (Role::VideoIn(_) | Role::VideoOut(_), R_FRAME_COUNT_MAX) => u32::MAX,
                    _ => self.regs[i],
                }
            }
            _ => 0,
        };
        narrow(v, off, size)
    }

    pub fn write(&mut self, env: &mut Env, off: u32, size: u8, v: u32) {
        match off & !3 {
            0x000 => self.irq_enable = widen(self.irq_enable, off, size, v),
            0x004 => self.irq_status |= v,
            0x008 => self.irq_status &= !v,
            0x100 => {
                self.timer_control = widen(self.timer_control, off, size, v) & 0xF;
                self.restart_timer(env);
            }
            0x104 => self.timer_interval = widen(self.timer_interval, off, size, v),
            0x108 => self.timer_count = widen(self.timer_count, off, size, v),
            0x200 => {
                let was = self.dma_control;
                self.dma_control = widen(self.dma_control, off, size, v) & 3;
                if was & DMA_ENABLE == 0 && self.dma_control & DMA_ENABLE != 0 {
                    self.dma_block_index = 0;
                    self.sample_time = env.now;
                }
            }
            0x204 => self.dma_address = widen(self.dma_address, off, size, v),
            0x208 => self.dma_block_size = widen(self.dma_block_size, off, size, v),
            0x20C => self.dma_block_num = widen(self.dma_block_num, off, size, v),
            0x300..=0x3FC => {
                let i = ((off & !3) - 0x300) as usize / 4;
                let nv = widen(self.regs[i], off, size, v);
                match (self.role, i) {
                    (Role::VideoIn(_) | Role::VideoOut(_), R_FILENAME_LEN) => {
                        self.filename.clear();
                        self.regs[i] = nv;
                    }
                    (Role::VideoIn(_) | Role::VideoOut(_), R_FILENAME_CHAR) => {
                        self.filename.push(nv as u8);
                        if self.filename.len() == self.regs[R_FILENAME_LEN] as usize {
                            let name = String::from_utf8_lossy(&self.filename).to_string();
                            env.warn.warn(env.now, format!("VSI{}: video file {name:?} requested; the simulator's camera/display sources are used instead", self.n));
                        }
                    }
                    (Role::VideoIn(_) | Role::VideoOut(_), R_CONTROL) => {
                        self.regs[i] = nv & 7;
                        if nv & 4 != 0 {
                            // buffer flush
                            self.regs[R_FRAME_COUNT] = 0;
                            self.dma_block_index = 0;
                        }
                        if nv & 1 == 0 {
                            self.regs[R_STATUS] &= !(1 << 5);
                        }
                    }
                    (_, R_MODE) => self.regs[i] = nv & 1,
                    _ => self.regs[i] = nv,
                }
            }
            _ => {}
        }
    }
}

/// Converts a frame in one of the AVH colour formats to RGBA.
pub fn to_rgba(data: &[u8], w: u32, h: u32, fmt: PixelFormat) -> Option<Frame> {
    let n = fmt.frame_bytes(w, h);
    if data.len() < n || w == 0 || h == 0 {
        return None;
    }
    let (w_, h_) = (w as usize, h as usize);
    let mut rgba = vec![255u8; w_ * h_ * 4];
    match fmt {
        PixelFormat::Gray8 => {
            for i in 0..w_ * h_ {
                let g = data[i];
                rgba[4 * i..4 * i + 3].copy_from_slice(&[g, g, g]);
            }
        }
        PixelFormat::Rgb888 => {
            for i in 0..w_ * h_ {
                rgba[4 * i..4 * i + 3].copy_from_slice(&data[3 * i..3 * i + 3]);
            }
        }
        PixelFormat::Bgr565 | PixelFormat::Rgb565 => {
            for i in 0..w_ * h_ {
                let p = u16::from_le_bytes([data[2 * i], data[2 * i + 1]]);
                let (r, g, b) = (((p >> 11) & 0x1F) as u8, ((p >> 5) & 0x3F) as u8, (p & 0x1F) as u8);
                let (r, g, b) = ((r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2));
                if fmt == PixelFormat::Bgr565 {
                    rgba[4 * i..4 * i + 3].copy_from_slice(&[b, g, r]);
                } else {
                    rgba[4 * i..4 * i + 3].copy_from_slice(&[r, g, b]);
                }
            }
        }
        PixelFormat::Argb8888 => {
            for i in 0..w_ * h_ {
                let p = u32::from_le_bytes([data[4 * i], data[4 * i + 1], data[4 * i + 2], data[4 * i + 3]]);
                rgba[4 * i..4 * i + 3].copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
            }
        }
        PixelFormat::Nv12 | PixelFormat::Nv21 | PixelFormat::Yuv420 => {
            let y_plane = &data[..w_ * h_];
            for y in 0..h_ {
                for x in 0..w_ {
                    let yy = y_plane[y * w_ + x] as f32;
                    let (u, v) = match fmt {
                        PixelFormat::Yuv420 => {
                            let cw = w_.div_ceil(2);
                            let ch = h_.div_ceil(2);
                            let u = data[w_ * h_ + (y / 2) * cw + x / 2];
                            let v = data[w_ * h_ + cw * ch + (y / 2) * cw + x / 2];
                            (u, v)
                        }
                        _ => {
                            let cw = w_.div_ceil(2);
                            let i = w_ * h_ + (y / 2) * cw * 2 + (x / 2) * 2;
                            if fmt == PixelFormat::Nv12 {
                                (data[i], data[i + 1])
                            } else {
                                (data[i + 1], data[i])
                            }
                        }
                    };
                    let (u, v) = (u as f32 - 128.0, v as f32 - 128.0);
                    let r = (yy + 1.402 * v).clamp(0.0, 255.0) as u8;
                    let g = (yy - 0.344 * u - 0.714 * v).clamp(0.0, 255.0) as u8;
                    let b = (yy + 1.772 * u).clamp(0.0, 255.0) as u8;
                    let i = 4 * (y * w_ + x);
                    rgba[i..i + 3].copy_from_slice(&[r, g, b]);
                }
            }
        }
    }
    Some(Frame { width: w, height: h, rgba })
}

/// Converts RGBA pixels to one of the formats (camera sources produce RGBA).
pub fn from_rgba(rgba: &[u8], w: u32, h: u32, fmt: PixelFormat, out: &mut [u8]) {
    let (w_, h_) = (w as usize, h as usize);
    let n = fmt.frame_bytes(w, h);
    if out.len() < n || rgba.len() < w_ * h_ * 4 {
        return;
    }
    let px = |i: usize| (rgba[4 * i], rgba[4 * i + 1], rgba[4 * i + 2]);
    match fmt {
        PixelFormat::Gray8 => {
            for i in 0..w_ * h_ {
                let (r, g, b) = px(i);
                out[i] = ((r as u32 * 77 + g as u32 * 150 + b as u32 * 29) >> 8) as u8;
            }
        }
        PixelFormat::Rgb888 => {
            for i in 0..w_ * h_ {
                let (r, g, b) = px(i);
                out[3 * i..3 * i + 3].copy_from_slice(&[r, g, b]);
            }
        }
        PixelFormat::Rgb565 | PixelFormat::Bgr565 => {
            for i in 0..w_ * h_ {
                let (r, g, b) = px(i);
                let (r, g, b) = if fmt == PixelFormat::Bgr565 { (b, g, r) } else { (r, g, b) };
                let p = ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3);
                out[2 * i..2 * i + 2].copy_from_slice(&p.to_le_bytes());
            }
        }
        PixelFormat::Argb8888 => {
            for i in 0..w_ * h_ {
                let (r, g, b) = px(i);
                out[4 * i..4 * i + 4].copy_from_slice(&[b, g, r, 255]);
            }
        }
        PixelFormat::Nv12 | PixelFormat::Nv21 | PixelFormat::Yuv420 => {
            let cw = w_.div_ceil(2);
            let ch = h_.div_ceil(2);
            for y in 0..h_ {
                for x in 0..w_ {
                    let (r, g, b) = px(y * w_ + x);
                    let (r, g, b) = (r as f32, g as f32, b as f32);
                    out[y * w_ + x] = (0.299 * r + 0.587 * g + 0.114 * b).clamp(0.0, 255.0) as u8;
                }
            }
            for cy in 0..ch {
                for cx in 0..cw {
                    let (mut su, mut sv, mut cnt) = (0f32, 0f32, 0f32);
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let (x, y) = (cx * 2 + dx, cy * 2 + dy);
                            if x < w_ && y < h_ {
                                let (r, g, b) = px(y * w_ + x);
                                let (r, g, b) = (r as f32, g as f32, b as f32);
                                su += -0.169 * r - 0.331 * g + 0.5 * b + 128.0;
                                sv += 0.5 * r - 0.419 * g - 0.081 * b + 128.0;
                                cnt += 1.0;
                            }
                        }
                    }
                    let u = (su / cnt).clamp(0.0, 255.0) as u8;
                    let v = (sv / cnt).clamp(0.0, 255.0) as u8;
                    match fmt {
                        PixelFormat::Yuv420 => {
                            out[w_ * h_ + cy * cw + cx] = u;
                            out[w_ * h_ + cw * ch + cy * cw + cx] = v;
                        }
                        PixelFormat::Nv12 => {
                            out[w_ * h_ + cy * cw * 2 + cx * 2] = u;
                            out[w_ * h_ + cy * cw * 2 + cx * 2 + 1] = v;
                        }
                        _ => {
                            out[w_ * h_ + cy * cw * 2 + cx * 2] = v;
                            out[w_ * h_ + cy * cw * 2 + cx * 2 + 1] = u;
                        }
                    }
                }
            }
        }
    }
}
