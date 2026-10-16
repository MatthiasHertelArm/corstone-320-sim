//! The memories of the SSE-320 FVP and their Secure/Non-secure aliases.
//!
//! | Memory | Non-secure | Secure | Size |
//! |:--|:--|:--|:--|
//! | ITCM | 0x0000_0000 | 0x1000_0000 | 32 KB |
//! | Boot ROM | — | 0x1100_0000 | 128 KB (writable by the loader and the debugger) |
//! | SRAM (FPGA) | 0x0100_0000 | 0x1200_0000 | 2 MB |
//! | DTCM | 0x2000_0000 | 0x3000_0000 | 32 KB |
//! | ISRAM0 | 0x2100_0000 | 0x3100_0000 | 2 MB |
//! | ISRAM1 | 0x2120_0000 | 0x3120_0000 | 2 MB |
//! | QSPI SRAM | 0x2800_0000 | 0x3800_0000 | 8 MB (parameter, up to 128 MB) |
//! | DDR4 | 0x6000_0000 … | alternating 256 MB blocks | 2 GB, allocated in 1 MB chunks |

pub const ITCM_SIZE: u32 = 0x8000;
pub const DTCM_SIZE: u32 = 0x8000;
pub const ROM_SIZE: u32 = 0x2_0000;
pub const SRAM_SIZE: u32 = 0x20_0000;
pub const ISRAM_SIZE: u32 = 0x20_0000;
pub const DDR_SIZE: u64 = 0x8000_0000;
const CHUNK: usize = 1 << 20;

/// Fill pattern of the FVP memories at start (`fill1`/`fill2` alternate per word).
pub const FILL1: u32 = 0xDFDF_DFCF;
pub const FILL2: u32 = 0xCFDF_DFDF;

pub struct Memory {
    pub itcm: Vec<u8>,
    pub dtcm: Vec<u8>,
    pub rom: Vec<u8>,
    pub sram: Vec<u8>,
    pub isram0: Vec<u8>,
    pub isram1: Vec<u8>,
    /// QSPI SRAM, allocated in chunks on first write like the DDR (128 MB on the FVP).
    qspi: Vec<Option<Box<[u8]>>>,
    pub qspi_size: u32,
    ddr: Vec<Option<Box<[u8]>>>,
    /// Incremented whenever the loader or the debugger writes code memory.
    pub code_gen: u64,
}

/// A resolved memory location.
pub enum Loc<'a> {
    Bytes(&'a mut [u8], usize),
    /// DDR chunk that is not allocated yet (reads return the fill pattern).
    Unallocated,
}

impl Memory {
    pub fn new(qspi_size: u32) -> Self {
        let mut m = Memory {
            itcm: vec![0; ITCM_SIZE as usize],
            dtcm: vec![0; DTCM_SIZE as usize],
            rom: vec![0; ROM_SIZE as usize],
            sram: vec![0; SRAM_SIZE as usize],
            isram0: vec![0; ISRAM_SIZE as usize],
            isram1: vec![0; ISRAM_SIZE as usize],
            qspi: (0..(qspi_size as usize).div_ceil(CHUNK)).map(|_| None).collect(),
            qspi_size,
            ddr: (0..(DDR_SIZE as usize / CHUNK)).map(|_| None).collect(),
            code_gen: 0,
        };
        m.fill_patterns();
        m
    }

    fn fill_patterns(&mut self) {
        for mem in [&mut self.rom, &mut self.sram] {
            for (i, w) in mem.chunks_mut(4).enumerate() {
                let v = if i % 2 == 0 { FILL1 } else { FILL2 };
                w.copy_from_slice(&v.to_le_bytes()[..w.len()]);
            }
        }
    }

    /// Power-on: the contents are kept (the loader writes the images before the first
    /// reset, as the FVP loads them at start); only the decode caches are invalidated.
    pub fn power_on(&mut self) {
        self.code_gen += 1;
    }

    /// Clears all memories to the start-of-simulation state (fill patterns).
    pub fn clear(&mut self) {
        self.itcm.fill(0);
        self.dtcm.fill(0);
        self.isram0.fill(0);
        self.isram1.fill(0);
        for c in self.ddr.iter_mut().chain(self.qspi.iter_mut()) {
            *c = None;
        }
        self.fill_patterns();
        self.code_gen += 1;
    }

    /// The memory holding `addr` and the offset within it.
    pub fn resolve(&mut self, addr: u32) -> Option<Loc<'_>> {
        let top = addr >> 24;
        let (mem, base, size): (&mut Vec<u8>, u32, u32) = match top {
            0x00 | 0x10 => (&mut self.itcm, top << 24, ITCM_SIZE),
            0x11 => (&mut self.rom, 0x1100_0000, ROM_SIZE),
            0x01 => (&mut self.sram, 0x0100_0000, SRAM_SIZE),
            0x12 => (&mut self.sram, 0x1200_0000, SRAM_SIZE),
            0x20 | 0x30 => (&mut self.dtcm, top << 24, DTCM_SIZE),
            0x21 | 0x31 => {
                if addr & 0x00FF_FFFF < 0x20_0000 {
                    (&mut self.isram0, top << 24, ISRAM_SIZE)
                } else {
                    (&mut self.isram1, (top << 24) + 0x20_0000, ISRAM_SIZE)
                }
            }
            0x28..=0x2F | 0x38..=0x3F => {
                let off = addr & 0x07FF_FFFF;
                if off >= self.qspi_size {
                    return None;
                }
                let off = off as usize;
                return Some(match &mut self.qspi[off / CHUNK] {
                    Some(c) => Loc::Bytes(&mut c[..], off % CHUNK),
                    None => Loc::Unallocated,
                });
            }
            0x60..=0xDF => {
                let off = (addr - 0x6000_0000) as usize;
                let chunk = off / CHUNK;
                return Some(match &mut self.ddr[chunk] {
                    Some(c) => Loc::Bytes(&mut c[..], off % CHUNK),
                    None => Loc::Unallocated,
                });
            }
            _ => return None,
        };
        if addr.wrapping_sub(base) < size {
            Some(Loc::Bytes(&mut mem[..], (addr - base) as usize))
        } else {
            None
        }
    }

    /// The lazily allocated chunk holding `addr` (DDR or QSPI), filled with the FVP pattern.
    fn lazy_chunk(&mut self, addr: u32) -> Option<(&mut [u8], usize)> {
        let (chunks, off) = if (0x6000_0000..0xE000_0000).contains(&addr) {
            (&mut self.ddr, (addr - 0x6000_0000) as usize)
        } else {
            let off = addr & 0x07FF_FFFF;
            if off >= self.qspi_size {
                return None;
            }
            (&mut self.qspi, off as usize)
        };
        let chunk = off / CHUNK;
        if chunks[chunk].is_none() {
            let mut c = vec![0u8; CHUNK].into_boxed_slice();
            for (i, w) in c.chunks_mut(4).enumerate() {
                let v = if i % 2 == 0 { FILL1 } else { FILL2 };
                w.copy_from_slice(&v.to_le_bytes());
            }
            chunks[chunk] = Some(c);
        }
        Some((&mut chunks[chunk].as_mut().unwrap()[..], off % CHUNK))
    }

    fn is_lazy(addr: u32) -> bool {
        matches!(addr >> 24, 0x28..=0x2F | 0x38..=0x3F | 0x60..=0xDF)
    }

    /// Reads a little-endian value of 1, 2 or 4 bytes; `None` outside the memories.
    #[inline]
    pub fn read(&mut self, addr: u32, size: u8) -> Option<u32> {
        if size > 1 && (addr & 0x00FF_FFFF) + size as u32 > 0x100_0000 {
            return None;
        }
        match self.resolve(addr)? {
            Loc::Bytes(m, o) => {
                if o + size as usize > m.len() {
                    return None;
                }
                Some(match size {
                    1 => m[o] as u32,
                    2 => u16::from_le_bytes([m[o], m[o + 1]]) as u32,
                    _ => u32::from_le_bytes([m[o], m[o + 1], m[o + 2], m[o + 3]]),
                })
            }
            Loc::Unallocated => {
                let word = if (addr >> 2) & 1 == 0 { FILL1 } else { FILL2 };
                Some(crate::env::narrow(word, addr, size))
            }
        }
    }

    #[inline]
    pub fn write(&mut self, addr: u32, size: u8, v: u32) -> bool {
        if Self::is_lazy(addr) {
            let Some((c, off)) = self.lazy_chunk(addr) else { return false };
            if off + size as usize > CHUNK {
                return false;
            }
            Self::put(c, off, size, v);
            return true;
        }
        match self.resolve(addr) {
            Some(Loc::Bytes(m, o)) => {
                if o + size as usize > m.len() {
                    return false;
                }
                Self::put(m, o, size, v);
                true
            }
            _ => false,
        }
    }

    #[inline]
    fn put(m: &mut [u8], o: usize, size: u8, v: u32) {
        match size {
            1 => m[o] = v as u8,
            2 => m[o..o + 2].copy_from_slice(&(v as u16).to_le_bytes()),
            _ => m[o..o + 4].copy_from_slice(&v.to_le_bytes()),
        }
    }

    /// Whether `addr` is in a memory (any alias).
    pub fn is_memory(&self, addr: u32) -> bool {
        let top = addr >> 24;
        match top {
            0x00 | 0x10 => addr & 0x00FF_FFFF < ITCM_SIZE,
            0x11 => addr & 0x00FF_FFFF < ROM_SIZE,
            0x01 | 0x12 => addr & 0x00FF_FFFF < SRAM_SIZE,
            0x20 | 0x30 => addr & 0x00FF_FFFF < DTCM_SIZE,
            0x21 | 0x31 => addr & 0x00FF_FFFF < 2 * ISRAM_SIZE,
            0x28..=0x2F | 0x38..=0x3F => addr & 0x07FF_FFFF < self.qspi_size,
            0x60..=0xDF => true,
            _ => false,
        }
    }

    /// Copies `buf.len()` bytes from memory (DMA); bytes outside memory read as zero.
    pub fn read_bytes(&mut self, addr: u32, buf: &mut [u8]) -> bool {
        let mut ok = true;
        let mut i = 0;
        while i < buf.len() {
            let a = addr.wrapping_add(i as u32);
            let n = (buf.len() - i).min((0x1000 - (a & 0xFFF)) as usize);
            match self.resolve(a) {
                Some(Loc::Bytes(m, o)) if o + n <= m.len() => buf[i..i + n].copy_from_slice(&m[o..o + n]),
                Some(Loc::Unallocated) => {
                    for (k, b) in buf[i..i + n].iter_mut().enumerate() {
                        let aa = a + k as u32;
                        let word = if (aa >> 2) & 1 == 0 { FILL1 } else { FILL2 };
                        *b = (word >> ((aa & 3) * 8)) as u8;
                    }
                }
                _ => {
                    buf[i..i + n].fill(0);
                    ok = false;
                }
            }
            i += n;
        }
        ok
    }

    /// Copies bytes into memory (DMA).
    pub fn write_bytes(&mut self, addr: u32, data: &[u8]) -> bool {
        let mut ok = true;
        let mut i = 0;
        while i < data.len() {
            let a = addr.wrapping_add(i as u32);
            let n = (data.len() - i).min((0x1000 - (a & 0xFFF)) as usize);
            if Self::is_lazy(a) {
                match self.lazy_chunk(a) {
                    Some((c, off)) => c[off..off + n].copy_from_slice(&data[i..i + n]),
                    None => ok = false,
                }
            } else {
                match self.resolve(a) {
                    Some(Loc::Bytes(m, o)) if o + n <= m.len() => m[o..o + n].copy_from_slice(&data[i..i + n]),
                    _ => ok = false,
                }
            }
            i += n;
        }
        ok
    }

    /// Writes an image byte (loader, debugger `load`).
    pub fn load_byte(&mut self, addr: u32, b: u8) -> bool {
        let ok = self.write(addr, 1, b as u32);
        if ok {
            self.code_gen += 1;
        }
        ok
    }
}
