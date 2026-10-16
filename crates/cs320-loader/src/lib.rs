//! Program images: ELF (load by physical address, symbols), Intel HEX, raw binary, and the
//! `*.cbuild-run.yml` files of the CMSIS-Toolbox that name them.

use object::read::elf::{ElfFile32, FileHeader, ProgramHeader};
use object::{elf, Endianness, Object, ObjectSymbol};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub mod cbuild_run;

#[derive(Clone, Debug, Default)]
pub struct Segment {
    pub addr: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct Image {
    pub segments: Vec<Segment>,
    pub entry: Option<u32>,
    /// Function and object symbols (name -> address).
    pub symbols: BTreeMap<String, u32>,
    pub path: Option<PathBuf>,
}

impl Image {
    /// The vector table of the image: the 128-byte aligned address whose reset vector (word 1)
    /// is the entry point, found in the loaded bytes. None when no such table is loaded.
    pub fn vector_table(&self) -> Option<u32> {
        let entry = self.entry? & !1;
        for seg in &self.segments {
            let end = seg.addr as u64 + seg.data.len() as u64;
            let first = (seg.addr + 127) & !127;
            let mut a = first as u64;
            while a + 8 <= end {
                let o = (a - seg.addr as u64) as usize;
                let reset = u32::from_le_bytes([seg.data[o + 4], seg.data[o + 5], seg.data[o + 6], seg.data[o + 7]]);
                if reset & !1 == entry {
                    return Some(a as u32);
                }
                a += 128;
            }
        }
        None
    }
}

#[derive(Debug)]
pub struct LoadError(pub String);

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LoadError {}

fn err(s: impl Into<String>) -> LoadError {
    LoadError(s.into())
}

impl Image {
    /// Loads an image by file extension (.elf/.axf/.out, .hex, .bin at address 0).
    pub fn load(path: &Path) -> Result<Image, LoadError> {
        let data = std::fs::read(path).map_err(|e| err(format!("{}: {e}", path.display())))?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let mut img = match ext.as_str() {
            "hex" | "ihex" => Self::from_hex(std::str::from_utf8(&data).map_err(|_| err("HEX file is not text"))?)?,
            "bin" => Image { segments: vec![Segment { addr: 0, data }], ..Default::default() },
            _ => {
                if data.starts_with(b"\x7fELF") {
                    Self::from_elf(&data)?
                } else if data.starts_with(b":") {
                    Self::from_hex(std::str::from_utf8(&data).map_err(|_| err("HEX file is not text"))?)?
                } else {
                    return Err(err(format!("{}: unknown image format", path.display())));
                }
            }
        };
        img.path = Some(path.to_path_buf());
        Ok(img)
    }

    /// Loadable segments by physical address (LMA), like a flash programmer.
    pub fn from_elf(data: &[u8]) -> Result<Image, LoadError> {
        let file = ElfFile32::<Endianness>::parse(data).map_err(|e| err(format!("ELF: {e}")))?;
        let endian = file.endian();
        if file.elf_header().e_machine(endian) != elf::EM_ARM {
            return Err(err("ELF: not an Arm image"));
        }
        let mut segments = Vec::new();
        for ph in file.elf_program_headers() {
            if ph.p_type(endian) != elf::PT_LOAD || ph.p_filesz(endian) == 0 {
                continue;
            }
            let bytes = ph.data(endian, data).map_err(|_| err("ELF: bad segment"))?;
            segments.push(Segment { addr: ph.p_paddr(endian), data: bytes.to_vec() });
        }
        let mut symbols = BTreeMap::new();
        for s in file.symbols() {
            if let Ok(name) = s.name() {
                if !name.is_empty() && !name.starts_with('$') {
                    symbols.insert(name.to_string(), s.address() as u32);
                }
            }
        }
        Ok(Image { segments, entry: Some(file.entry() as u32), symbols, path: None })
    }

    pub fn from_hex(text: &str) -> Result<Image, LoadError> {
        let mut base: u32 = 0;
        let mut segments: Vec<Segment> = Vec::new();
        let mut entry = None;
        for rec in ihex::Reader::new(text) {
            match rec.map_err(|e| err(format!("HEX: {e}")))? {
                ihex::Record::Data { offset, value } => {
                    let addr = base.wrapping_add(offset as u32);
                    match segments.last_mut() {
                        Some(s) if s.addr + s.data.len() as u32 == addr => s.data.extend_from_slice(&value),
                        _ => segments.push(Segment { addr, data: value }),
                    }
                }
                ihex::Record::ExtendedLinearAddress(a) => base = (a as u32) << 16,
                ihex::Record::ExtendedSegmentAddress(a) => base = (a as u32) << 4,
                ihex::Record::StartLinearAddress(a) => entry = Some(a),
                ihex::Record::StartSegmentAddress { .. } => {}
                ihex::Record::EndOfFile => break,
            }
        }
        Ok(Image { segments, entry, ..Default::default() })
    }

    pub fn symbol(&self, name: &str) -> Option<u32> {
        self.symbols.get(name).copied()
    }

    /// Total number of bytes.
    pub fn size(&self) -> usize {
        self.segments.iter().map(|s| s.data.len()).sum()
    }
}
