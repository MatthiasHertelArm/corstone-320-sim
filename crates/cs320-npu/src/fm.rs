//! Feature-map access: tiles, NHWC and NHCWB16 layouts, element precision (TRM §3.6.5, §3.8).
//! Whole tensors are read into and written from dense `[h][w][c]` buffers, one memory
//! transaction per contiguous run.

use crate::NpuBus;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Storage {
    Tile2x2 = 0,
    Tile3x1 = 1,
    Chained = 2,
    None = 3,
}

impl Storage {
    pub fn from_bits(v: u32) -> Self {
        match v & 3 {
            0 => Storage::Tile2x2,
            1 => Storage::Tile3x1,
            2 => Storage::Chained,
            _ => Storage::None,
        }
    }
}

/// Precision and layout of a feature map.
#[derive(Clone, Copy, Debug)]
pub struct FmDesc {
    pub base: [u64; 4],
    pub stride_x: u64,
    pub stride_y: u64,
    pub stride_c: u64,
    pub width0: usize,
    pub height: [usize; 2],
    pub storage: Storage,
    /// element size in bytes (1, 2, 4, 8)
    pub esize: usize,
    pub signed: bool,
    pub brick: bool,
    pub region: usize,
    pub zero_point: i64,
}

/// Elements of the dense buffers.
pub trait Elem: Copy + Default {
    fn from_i64(v: i64) -> Self;
    fn to_i64(self) -> i64;
}

impl Elem for i16 {
    #[inline]
    fn from_i64(v: i64) -> Self {
        v as i16
    }
    #[inline]
    fn to_i64(self) -> i64 {
        self as i64
    }
}

impl Elem for i32 {
    #[inline]
    fn from_i64(v: i64) -> Self {
        v as i32
    }
    #[inline]
    fn to_i64(self) -> i64 {
        self as i64
    }
}

impl Elem for i64 {
    #[inline]
    fn from_i64(v: i64) -> Self {
        v
    }
    #[inline]
    fn to_i64(self) -> i64 {
        self
    }
}

#[inline]
pub fn decode_elem(bytes: &[u8], esize: usize, signed: bool) -> i64 {
    match (esize, signed) {
        (1, false) => bytes[0] as i64,
        (1, true) => bytes[0] as i8 as i64,
        (2, false) => u16::from_le_bytes([bytes[0], bytes[1]]) as i64,
        (2, true) => i16::from_le_bytes([bytes[0], bytes[1]]) as i64,
        (4, false) => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i64,
        (4, true) => i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i64,
        _ => i64::from_le_bytes(bytes[..8].try_into().unwrap()),
    }
}

#[inline]
pub fn encode_elem(v: i64, esize: usize, out: &mut [u8]) {
    match esize {
        1 => out[0] = v as u8,
        2 => out[..2].copy_from_slice(&(v as u16).to_le_bytes()),
        4 => out[..4].copy_from_slice(&(v as u32).to_le_bytes()),
        _ => out[..8].copy_from_slice(&v.to_le_bytes()),
    }
}

impl FmDesc {
    /// Tile and coordinates within the tile of element (y, x).
    #[inline]
    fn tile(&self, y: usize, x: usize) -> (usize, usize, usize) {
        let (mut y, mut x, mut tile) = (y, x, 0);
        match self.storage {
            Storage::Tile2x2 => {
                if x >= self.width0 {
                    x -= self.width0;
                    tile += 1;
                }
                if y >= self.height[tile] {
                    y -= self.height[tile];
                    tile += 2;
                }
            }
            Storage::Tile3x1 => {
                if y >= self.height[1] {
                    y -= self.height[1];
                    tile = 2;
                } else if y >= self.height[0] {
                    y -= self.height[0];
                    tile = 1;
                }
            }
            _ => {}
        }
        (tile, y, x)
    }

    /// Byte address of element (y, x, c) within the region.
    pub fn addr(&self, y: usize, x: usize, c: usize) -> u64 {
        let (tile, ty, tx) = self.tile(y, x);
        let mut a = self.base[tile].wrapping_add(ty as u64 * self.stride_y);
        if self.brick {
            a = a.wrapping_add((c as u64 >> 4) * self.stride_c + ((16 * tx + (c & 15)) * self.esize) as u64);
        } else {
            a = a.wrapping_add(tx as u64 * self.stride_x + (c * self.esize) as u64);
        }
        a
    }

    /// Number of x positions from `x` that stay in the same tile (at most `n`).
    #[inline]
    fn run_x(&self, x: usize, n: usize) -> usize {
        match self.storage {
            Storage::Tile2x2 if x < self.width0 => n.min(self.width0 - x),
            _ => n,
        }
    }

    /// Reads the `[h][w][c]` tensor into `out`, subtracting the zero point.
    pub fn read<T: Elem>(&self, bus: &mut dyn NpuBus, base: u64, h: usize, w: usize, c: usize, out: &mut Vec<T>) -> bool {
        out.clear();
        out.resize(h * w * c, T::default());
        let esize = self.esize;
        let mut buf: Vec<u8> = Vec::new();
        let mut ok = true;
        if !self.brick {
            let contiguous_x = self.stride_x == (c * esize) as u64;
            for y in 0..h {
                let mut x = 0;
                while x < w {
                    let n = self.run_x(x, w - x);
                    if contiguous_x {
                        let a = base.wrapping_add(self.addr(y, x, 0));
                        buf.resize(n * c * esize, 0);
                        ok &= bus.read(a, &mut buf);
                        let dst = &mut out[(y * w + x) * c..(y * w + x + n) * c];
                        decode_run(&buf, esize, self.signed, self.zero_point, dst);
                    } else {
                        buf.resize(c * esize, 0);
                        for xi in x..x + n {
                            let a = base.wrapping_add(self.addr(y, xi, 0));
                            ok &= bus.read(a, &mut buf);
                            let dst = &mut out[(y * w + xi) * c..(y * w + xi + 1) * c];
                            decode_run(&buf, esize, self.signed, self.zero_point, dst);
                        }
                    }
                    x += n;
                }
            }
        } else {
            let groups = c.div_ceil(16);
            let mut tmp: Vec<T> = Vec::new();
            for y in 0..h {
                let mut x = 0;
                while x < w {
                    let n = self.run_x(x, w - x);
                    for g in 0..groups {
                        let a = base.wrapping_add(self.addr(y, x, g * 16));
                        buf.resize(n * 16 * esize, 0);
                        ok &= bus.read(a, &mut buf);
                        tmp.clear();
                        tmp.resize(n * 16, T::default());
                        decode_run(&buf, esize, self.signed, self.zero_point, &mut tmp);
                        let cn = (c - g * 16).min(16);
                        for xi in 0..n {
                            let dst = &mut out[((y * w + x + xi) * c + g * 16)..((y * w + x + xi) * c + g * 16 + cn)];
                            dst.copy_from_slice(&tmp[xi * 16..xi * 16 + cn]);
                        }
                    }
                    x += n;
                }
            }
        }
        ok
    }

    /// Writes the `[h][w][c]` tensor of already truncated values.
    pub fn write(&self, bus: &mut dyn NpuBus, base: u64, h: usize, w: usize, c: usize, data: &[i64]) -> bool {
        let esize = self.esize;
        let mut buf: Vec<u8> = Vec::new();
        let mut ok = true;
        if !self.brick {
            let contiguous_x = self.stride_x == (c * esize) as u64;
            for y in 0..h {
                let mut x = 0;
                while x < w {
                    let n = self.run_x(x, w - x);
                    if contiguous_x {
                        let a = base.wrapping_add(self.addr(y, x, 0));
                        let src = &data[(y * w + x) * c..(y * w + x + n) * c];
                        buf.resize(src.len() * esize, 0);
                        encode_run(src, esize, &mut buf);
                        ok &= bus.write(a, &buf);
                    } else {
                        for xi in x..x + n {
                            let a = base.wrapping_add(self.addr(y, xi, 0));
                            let src = &data[(y * w + xi) * c..(y * w + xi + 1) * c];
                            buf.resize(src.len() * esize, 0);
                            encode_run(src, esize, &mut buf);
                            ok &= bus.write(a, &buf);
                        }
                    }
                    x += n;
                }
            }
        } else {
            let groups = c.div_ceil(16);
            let mut tmp: Vec<i64> = Vec::new();
            for y in 0..h {
                let mut x = 0;
                while x < w {
                    let n = self.run_x(x, w - x);
                    for g in 0..groups {
                        let cn = (c - g * 16).min(16);
                        tmp.clear();
                        tmp.resize(n * 16, 0);
                        for xi in 0..n {
                            let src = &data[((y * w + x + xi) * c + g * 16)..((y * w + x + xi) * c + g * 16 + cn)];
                            tmp[xi * 16..xi * 16 + cn].copy_from_slice(src);
                        }
                        buf.resize(n * 16 * esize, 0);
                        encode_run(&tmp, esize, &mut buf);
                        let a = base.wrapping_add(self.addr(y, x, g * 16));
                        ok &= bus.write(a, &buf);
                    }
                    x += n;
                }
            }
        }
        ok
    }

    /// Writes one element (the slow path used for transposed or reversed outputs).
    pub fn write_elem(&self, bus: &mut dyn NpuBus, base: u64, y: usize, x: usize, c: usize, v: i64) -> bool {
        let mut buf = [0u8; 8];
        encode_elem(v, self.esize, &mut buf);
        bus.write(base.wrapping_add(self.addr(y, x, c)), &buf[..self.esize])
    }
}

#[inline]
fn decode_run<T: Elem>(buf: &[u8], esize: usize, signed: bool, zp: i64, dst: &mut [T]) {
    match (esize, signed) {
        (1, false) => {
            for (d, b) in dst.iter_mut().zip(buf) {
                *d = T::from_i64(*b as i64 - zp);
            }
        }
        (1, true) => {
            for (d, b) in dst.iter_mut().zip(buf) {
                *d = T::from_i64(*b as i8 as i64 - zp);
            }
        }
        (2, true) => {
            for (d, b) in dst.iter_mut().zip(buf.chunks_exact(2)) {
                *d = T::from_i64(i16::from_le_bytes([b[0], b[1]]) as i64 - zp);
            }
        }
        (4, true) => {
            for (d, b) in dst.iter_mut().zip(buf.chunks_exact(4)) {
                *d = T::from_i64(i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as i64 - zp);
            }
        }
        _ => {
            for (d, b) in dst.iter_mut().zip(buf.chunks_exact(esize)) {
                *d = T::from_i64(decode_elem(b, esize, signed) - zp);
            }
        }
    }
}

#[inline]
fn encode_run(src: &[i64], esize: usize, buf: &mut [u8]) {
    match esize {
        1 => {
            for (b, v) in buf.iter_mut().zip(src) {
                *b = *v as u8;
            }
        }
        2 => {
            for (b, v) in buf.chunks_exact_mut(2).zip(src) {
                b.copy_from_slice(&(*v as u16).to_le_bytes());
            }
        }
        4 => {
            for (b, v) in buf.chunks_exact_mut(4).zip(src) {
                b.copy_from_slice(&(*v as u32).to_le_bytes());
            }
        }
        _ => {
            for (b, v) in buf.chunks_exact_mut(8).zip(src) {
                b.copy_from_slice(&v.to_le_bytes());
            }
        }
    }
}
