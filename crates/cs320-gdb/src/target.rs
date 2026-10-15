//! Target description of the Cortex-M85 for GDB: registers, target.xml, memory map.

use cs320_cpu::Cpu;

/// Register numbers of the target description:
/// 0..15 r0..pc, 16 xpsr, 17 msp, 18 psp, 19 primask, 20 basepri, 21 faultmask, 22 control,
/// 23..38 d0..d15, 39 fpscr, 40 msp_ns, 41 psp_ns, 42 msp_s, 43 psp_s, 44 msplim_ns,
/// 45 psplim_ns, 46 msplim_s, 47 psplim_s, 48 vpr.
pub const NUM_REGS: u32 = 49;

#[derive(Clone, Copy, Debug)]
pub enum Sel {
    Core(u32),
    Special(u8),
    Double(u32),
}

pub fn reg_map(n: u32) -> Option<Sel> {
    Some(match n {
        0..=18 => Sel::Core(n),
        19..=22 => Sel::Special((n - 19) as u8),
        23..=38 => Sel::Double(n - 23),
        39 => Sel::Core(33),
        40 => Sel::Core(24),
        41 => Sel::Core(25),
        42 => Sel::Core(26),
        43 => Sel::Core(27),
        44 => Sel::Core(30),
        45 => Sel::Core(31),
        46 => Sel::Core(28),
        47 => Sel::Core(29),
        48 => Sel::Core(36),
        _ => return None,
    })
}

pub fn width(s: Sel) -> usize {
    match s {
        Sel::Double(_) => 8,
        _ => 4,
    }
}

pub fn read(cpu: &Cpu, s: Sel) -> Option<Vec<u8>> {
    Some(match s {
        Sel::Core(n) => cpu.read_core_reg(n)?.to_le_bytes().to_vec(),
        Sel::Special(i) => {
            let st = cpu.st();
            let v = match i {
                0 => cpu.primask[st] as u32,
                1 => cpu.basepri[st] as u32,
                2 => cpu.faultmask[st] as u32,
                _ => cpu.control(),
            };
            v.to_le_bytes().to_vec()
        }
        Sel::Double(d) => {
            let lo = cpu.s[(2 * d) as usize];
            let hi = cpu.s[(2 * d + 1) as usize];
            let mut v = lo.to_le_bytes().to_vec();
            v.extend(hi.to_le_bytes());
            v
        }
    })
}

pub fn write(cpu: &mut Cpu, s: Sel, data: &[u8]) -> bool {
    let w = |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
    if data.len() < width(s) {
        return false;
    }
    match s {
        Sel::Core(n) => cpu.write_core_reg(n, w(0)),
        Sel::Special(i) => {
            let v = w(0);
            match i {
                0 => cpu.set_primask(v & 1 != 0),
                1 => cpu.set_basepri(v as u8),
                2 => cpu.set_faultmask(v & 1 != 0),
                _ => cpu.set_control(v & 0xF),
            }
            true
        }
        Sel::Double(d) => {
            cpu.s[(2 * d) as usize] = w(0);
            cpu.s[(2 * d + 1) as usize] = w(4);
            true
        }
    }
}

pub fn target_xml() -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\"?>\n<!DOCTYPE target SYSTEM \"gdb-target.dtd\">\n<target version=\"1.0\">\n<architecture>arm</architecture>\n<feature name=\"org.gnu.gdb.arm.m-profile\">\n",
    );
    for i in 0..13 {
        s += &format!("<reg name=\"r{i}\" bitsize=\"32\" regnum=\"{i}\" type=\"uint32\" group=\"general\"/>\n");
    }
    s += "<reg name=\"sp\" bitsize=\"32\" regnum=\"13\" type=\"data_ptr\" group=\"general\"/>\n";
    s += "<reg name=\"lr\" bitsize=\"32\" regnum=\"14\" type=\"uint32\" group=\"general\"/>\n";
    s += "<reg name=\"pc\" bitsize=\"32\" regnum=\"15\" type=\"code_ptr\" group=\"general\"/>\n";
    s += "<reg name=\"xpsr\" bitsize=\"32\" regnum=\"16\" type=\"uint32\" group=\"general\"/>\n</feature>\n";
    s += "<feature name=\"org.gnu.gdb.arm.m-system\">\n";
    s += "<reg name=\"msp\" bitsize=\"32\" regnum=\"17\" type=\"data_ptr\" group=\"system\"/>\n";
    s += "<reg name=\"psp\" bitsize=\"32\" regnum=\"18\" type=\"data_ptr\" group=\"system\"/>\n";
    s += "<reg name=\"primask\" bitsize=\"32\" regnum=\"19\" type=\"uint32\" group=\"system\"/>\n";
    s += "<reg name=\"basepri\" bitsize=\"32\" regnum=\"20\" type=\"uint32\" group=\"system\"/>\n";
    s += "<reg name=\"faultmask\" bitsize=\"32\" regnum=\"21\" type=\"uint32\" group=\"system\"/>\n";
    s += "<reg name=\"control\" bitsize=\"32\" regnum=\"22\" type=\"uint32\" group=\"system\"/>\n</feature>\n";
    s += "<feature name=\"org.gnu.gdb.arm.vfp\">\n";
    for i in 0..16 {
        s += &format!("<reg name=\"d{i}\" bitsize=\"64\" regnum=\"{}\" type=\"ieee_double\" group=\"float\"/>\n", 23 + i);
    }
    s += "<reg name=\"fpscr\" bitsize=\"32\" regnum=\"39\" type=\"int\" group=\"float\"/>\n</feature>\n";
    s += "<feature name=\"org.gnu.gdb.arm.secext\">\n";
    s += "<reg name=\"msp_ns\" bitsize=\"32\" regnum=\"40\" type=\"data_ptr\" group=\"system\"/>\n";
    s += "<reg name=\"psp_ns\" bitsize=\"32\" regnum=\"41\" type=\"data_ptr\" group=\"system\"/>\n";
    s += "<reg name=\"msp_s\" bitsize=\"32\" regnum=\"42\" type=\"data_ptr\" group=\"system\"/>\n";
    s += "<reg name=\"psp_s\" bitsize=\"32\" regnum=\"43\" type=\"data_ptr\" group=\"system\"/>\n";
    s += "<reg name=\"msplim_ns\" bitsize=\"32\" regnum=\"44\" type=\"data_ptr\" group=\"system\"/>\n";
    s += "<reg name=\"psplim_ns\" bitsize=\"32\" regnum=\"45\" type=\"data_ptr\" group=\"system\"/>\n";
    s += "<reg name=\"msplim_s\" bitsize=\"32\" regnum=\"46\" type=\"data_ptr\" group=\"system\"/>\n";
    s += "<reg name=\"psplim_s\" bitsize=\"32\" regnum=\"47\" type=\"data_ptr\" group=\"system\"/>\n</feature>\n";
    s += "<feature name=\"org.gnu.gdb.arm.m-profile-mve\">\n";
    s += "<reg name=\"vpr\" bitsize=\"32\" regnum=\"48\" type=\"uint32\" group=\"system\"/>\n</feature>\n";
    s += "</target>\n";
    s
}

pub fn memory_map_xml() -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\"?>\n<!DOCTYPE memory-map PUBLIC \"+//IDN gnu.org//DTD GDB Memory Map V1.0//EN\" \"http://sourceware.org/gdb/gdb-memory-map.dtd\">\n<memory-map>\n",
    );
    let ram = |s: &mut String, start: u32, len: u32| {
        *s += &format!("<memory type=\"ram\" start=\"{start:#x}\" length=\"{len:#x}\"/>\n");
    };
    ram(&mut s, 0x0000_0000, 0x8000);
    ram(&mut s, 0x0100_0000, 0x20_0000);
    ram(&mut s, 0x1000_0000, 0x8000);
    ram(&mut s, 0x1100_0000, 0x2_0000);
    ram(&mut s, 0x1200_0000, 0x20_0000);
    ram(&mut s, 0x2000_0000, 0x8000);
    ram(&mut s, 0x2100_0000, 0x40_0000);
    ram(&mut s, 0x2800_0000, 0x800_0000);
    ram(&mut s, 0x3000_0000, 0x8000);
    ram(&mut s, 0x3100_0000, 0x40_0000);
    ram(&mut s, 0x3800_0000, 0x800_0000);
    ram(&mut s, 0x4000_0000, 0x1000_0000);
    ram(&mut s, 0x5000_0000, 0x1000_0000);
    ram(&mut s, 0x6000_0000, 0x8000_0000);
    ram(&mut s, 0xE000_0000, 0x10_0000);
    s += "</memory-map>\n";
    s
}

pub fn monitor_help() -> &'static str {
    "Monitor commands of the Corstone-320 simulator:\n\
     \x20 reset [halt|run]          reset the system (default: halt at the reset vector)\n\
     \x20 halt                      halt the core\n\
     \x20 sim time                  simulated time\n\
     \x20 sim stats                 instructions, MIPS, real-time factor\n\
     \x20 sim speed <x|max|realtime> simulation speed\n\
     \x20 sim button <1|2> press|release   FPGA user buttons\n\
     \x20 sim switch <0-7> on|off   FPGA user switches\n\
     \x20 sim vio button <0-7> press|release\n\
     \x20 sim vio value <id> <value>\n\
     \x20 sim leds                  FPGA and VIO LED state\n\
     \x20 sim uart [n] <text>       send text to UART n (default 0)\n\
     \x20 sim camera <pattern|none|file|folder>  camera source\n\
     \x20 sim mic <silence|tone HZ [AMP]|wav FILE>  microphone source\n\
     \x20 sim display               size and CRC of the last HDLCD frame\n\
     \x20 sim warnings              model warnings\n"
}
