//! Runs a command stream over a flat memory image, for debugging against a reference.
//!
//! ```text
//! run <mem.bin> <script> <out.bin>
//! ```
//! The script has one command per line: `macs N`, `basep I ADDR`, `qbase ADDR`, `qsize N`,
//! `trace`, `dump ADDR LEN` (appended to out.bin after the run). Numbers are decimal or 0x hex.

use cs320_npu::{Config, Npu};

fn num(s: &str) -> u64 {
    if let Some(h) = s.strip_prefix("0x") {
        u64::from_str_radix(h, 16).unwrap()
    } else {
        s.parse().unwrap()
    }
}

fn main() {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();
    let mut mem: Vec<u8> = std::fs::read(&args[1]).expect("memory image");
    let script = std::fs::read_to_string(&args[2]).expect("script");
    let mut macs = 1024;
    let mut basep = [0u64; 8];
    let mut qbase = 0u64;
    let mut qsize = 0u32;
    let mut dumps = Vec::new();
    let mut trace = false;
    for line in script.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["macs", n] => macs = num(n) as u32,
            ["basep", i, a] => basep[num(i) as usize] = num(a),
            ["qbase", a] => qbase = num(a),
            ["qsize", n] => qsize = num(n) as u32,
            ["trace"] => trace = true,
            ["dump", a, n] => dumps.push((num(a) as usize, num(n) as usize)),
            [] => {}
            _ => panic!("bad script line {line:?}"),
        }
    }
    let mut npu = Npu::new(Config::new(macs));
    npu.trace = trace;
    npu.write_reg(cs320_npu::REG_RESET, 1);
    for (i, b) in basep.iter().enumerate() {
        npu.write_reg(cs320_npu::REG_BASEP + 8 * i as u32, *b as u32);
        npu.write_reg(cs320_npu::REG_BASEP + 8 * i as u32 + 4, (*b >> 32) as u32);
    }
    npu.write_reg(cs320_npu::REG_QBASE, qbase as u32);
    npu.write_reg(cs320_npu::REG_QBASE_HI, (qbase >> 32) as u32);
    npu.write_reg(cs320_npu::REG_QSIZE, qsize);
    let t = std::time::Instant::now();
    assert!(npu.write_reg(cs320_npu::REG_CMD, 1));
    let c = npu.start(&mut mem);
    npu.complete();
    println!("status {:#010x} qread {} cycles {} host {:?}", c.status, c.qread, c.cycles, t.elapsed());
    for w in npu.take_warnings() {
        println!("warning: {w}");
    }
    let mut out = Vec::new();
    for (a, n) in dumps {
        out.extend_from_slice(&mem[a..a + n]);
    }
    std::fs::write(&args[3], out).unwrap();
}
