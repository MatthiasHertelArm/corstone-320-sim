//! Drives the GDB server like GDB and the cdt-gdb-adapter (controller + auxiliary connection).

use cs320_loader::Image;
use cs320_board::runtime::{Options, Simulator, Speed};
use cs320_board::Machine;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

struct Client {
    s: TcpStream,
    no_ack: bool,
}

fn checksum(d: &[u8]) -> u8 {
    d.iter().fold(0u8, |a, b| a.wrapping_add(*b))
}

impl Client {
    fn connect(port: u16) -> Client {
        let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        Client { s, no_ack: false }
    }

    fn send_raw(&mut self, d: &[u8]) {
        self.s.write_all(d).unwrap();
    }

    fn send(&mut self, p: &str) {
        let f = format!("${p}#{:02x}", checksum(p.as_bytes()));
        self.send_raw(f.as_bytes());
    }

    fn recv(&mut self) -> String {
        let mut b = [0u8; 1];
        loop {
            self.s.read_exact(&mut b).unwrap();
            if b[0] == b'$' {
                break;
            }
        }
        let mut data = Vec::new();
        loop {
            self.s.read_exact(&mut b).unwrap();
            if b[0] == b'#' {
                break;
            }
            data.push(b[0]);
        }
        let mut cs = [0u8; 2];
        self.s.read_exact(&mut cs).unwrap();
        assert_eq!(u8::from_str_radix(std::str::from_utf8(&cs).unwrap(), 16).unwrap(), checksum(&data));
        if !self.no_ack {
            self.send_raw(b"+");
        }
        String::from_utf8_lossy(&data).to_string()
    }

    fn cmd(&mut self, p: &str) -> String {
        self.send(p);
        self.recv()
    }
}

fn reg(g: &str, n: usize) -> u32 {
    let h = &g[8 * n..8 * n + 8];
    u32::from_le_bytes(std::array::from_fn(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap()))
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn controller_and_auxiliary() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/elf/BoardTest.elf");
    let img = Image::load(&path).unwrap();
    let main = img.symbol("main").unwrap() & !1;
    let reset = img.symbol("Reset_Handler").unwrap();
    let mut m = Machine::new();
    m.load_image(&img).unwrap();
    m.power_on();
    let sim = Simulator::start(m, Options { speed: Speed::Max, start_halted: true, ..Default::default() });
    let server = cs320_gdb::Server::bind(sim.clone(), "127.0.0.1:0").unwrap();
    let port = server.local_port();
    std::thread::spawn(move || server.serve());

    let mut c = Client::connect(port);
    let sup = c.cmd("qSupported:multiprocess+;swbreak+;hwbreak+;vContSupported+");
    assert!(sup.contains("qXfer:features:read+") && sup.contains("qXfer:memory-map:read+"), "{sup}");
    assert_eq!(c.cmd("QStartNoAckMode"), "OK");
    c.no_ack = true;
    let xml = c.cmd("qXfer:features:read:target.xml:0,ffb");
    assert!(xml.starts_with('l') && xml.contains("org.gnu.gdb.arm.m-profile") && xml.contains("org.gnu.gdb.arm.vfp"));
    let mm = c.cmd("qXfer:memory-map:read::0,ffb");
    assert!(mm.contains("type=\"flash\" start=\"0x0\""));
    assert!(c.cmd("?").starts_with("T05"));
    let g = c.cmd("g");
    assert_eq!(g.len(), (24 * 4 + 16 * 8) * 2, "r0-pc, xpsr, 6 system registers, fpscr; d0-d15");
    assert_eq!(reg(&g, 15), reset & !1, "halted at the reset vector");
    // vector table
    let vt = c.cmd("m0,8");
    assert_eq!(vt.len(), 16);
    // hardware breakpoint at main, continue
    assert_eq!(c.cmd(&format!("Z1,{main:x},2")), "OK");
    let stop = c.cmd("c");
    assert!(stop.starts_with("T05") && stop.contains("hwbreak"), "{stop}");
    let g = c.cmd("g");
    assert_eq!(reg(&g, 15), main & !1);
    assert_eq!(c.cmd(&format!("z1,{main:x},2")), "OK");
    // step
    let s = c.cmd("vCont;s:1");
    assert!(s.starts_with("T05"), "{s}");
    // auxiliary connection while the core runs
    let mut aux = Client::connect(port);
    aux.cmd("qSupported:swbreak+");
    assert!(aux.cmd("?").starts_with("T05"), "auxiliary sees a stopped target");
    c.send("c");
    std::thread::sleep(Duration::from_millis(100));
    let t1 = sim.status().time;
    let r1 = aux.cmd("me0001004,4"); // DWT_CYCCNT
    std::thread::sleep(Duration::from_millis(50));
    let r2 = aux.cmd("me0001004,4");
    assert_ne!(r1, r2, "CYCCNT advances while running (auxiliary reads)");
    let ctrl = aux.cmd("me0001000,4");
    let dwt_ctrl = u32::from_str_radix(&ctrl[..2], 16).unwrap();
    assert_eq!(dwt_ctrl & 1, 1, "CYCCNTENA set for the CMSIS Debugger CPU time");
    assert!(sim.status().time > t1, "target runs");
    assert!(sim.status().halted.is_none());
    assert_eq!(aux.cmd("c"), "E01", "auxiliary cannot resume");
    // interrupt
    c.send_raw(&[0x03]);
    let stop = c.recv();
    assert!(stop.starts_with("T02"), "{stop}");
    // monitor reset halt
    let out = c.cmd(&format!("qRcmd,{}", hex("reset halt")));
    assert!(out.starts_with('O'), "{out}");
    assert_eq!(c.recv(), "OK");
    std::thread::sleep(Duration::from_millis(50));
    let g = c.cmd("g");
    assert_eq!(reg(&g, 15), reset & !1);
    // simulator monitor command
    let out = c.cmd(&format!("qRcmd,{}", hex("sim leds")));
    assert!(out.starts_with('O'));
    assert_eq!(c.recv(), "OK");
    assert_eq!(c.cmd("D"), "OK");
}
