//! GDB remote serial protocol server for the simulated Corstone-320.
//!
//! Compatible with the Arm CMSIS Debugger (cdt-gdb-adapter with an auxiliary GDB): the first
//! connection is the controller (it halts the core on connect, like pyOCD); further connections
//! on the same port are auxiliary — they see the target as stopped and read and write memory
//! while the core runs, but cannot control execution.

mod packet;
mod target;

use crossbeam_channel::{select, unbounded, Receiver};
use cs320_board::runtime::{BpKind, Cmd, Simulator};
use cs320_cpu::HaltReason;
use packet::{escape, frame, hex, unescape, unhex, Incoming, Reader};
use std::collections::BTreeMap;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
pub use target::{memory_map_xml, target_xml, NUM_REGS};

/// Monitor command handler: returns the text to print.
pub type MonitorFn = Arc<dyn Fn(&Simulator, &str) -> Option<String> + Send + Sync>;

pub struct Server {
    pub sim: Simulator,
    listener: TcpListener,
    controller: Arc<Mutex<Option<u64>>>,
    next_id: u64,
    pub monitor_ext: Option<MonitorFn>,
    /// Exit the process when the controller disconnects (launched by a debugger).
    pub exit_on_detach: bool,
}

impl Server {
    pub fn bind(sim: Simulator, addr: &str) -> std::io::Result<Server> {
        let listener = TcpListener::bind(addr)?;
        Ok(Server { sim, listener, controller: Arc::new(Mutex::new(None)), next_id: 1, monitor_ext: None, exit_on_detach: false })
    }

    pub fn local_port(&self) -> u16 {
        self.listener.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    /// Accepts connections forever (call from a dedicated thread).
    pub fn serve(mut self) {
        for stream in self.listener.incoming() {
            let Ok(stream) = stream else { continue };
            let _ = stream.set_nodelay(true);
            let id = self.next_id;
            self.next_id += 1;
            let is_controller = {
                let mut c = self.controller.lock().unwrap();
                if c.is_none() {
                    *c = Some(id);
                    true
                } else {
                    false
                }
            };
            let sim = self.sim.clone();
            let controller = self.controller.clone();
            let monitor = self.monitor_ext.clone();
            let exit_on_detach = self.exit_on_detach;
            std::thread::Builder::new()
                .name(format!("gdb-{id}"))
                .spawn(move || {
                    let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
                    log::info!("GDB connection {id} from {peer} ({})", if is_controller { "controller" } else { "auxiliary" });
                    let mut c = Connection::new(id, sim.clone(), stream, is_controller, monitor);
                    c.run();
                    log::info!("GDB connection {id} closed");
                    if is_controller {
                        sim.send(Cmd::Detach);
                        *controller.lock().unwrap() = None;
                        if exit_on_detach {
                            // the debug adapter normally ends the server itself; this only
                            // avoids an orphan process, so give it time to shut down cleanly
                            std::thread::sleep(std::time::Duration::from_secs(2));
                            std::process::exit(0);
                        }
                    }
                })
                .ok();
        }
    }
}

struct Connection {
    #[allow(dead_code)]
    id: u64,
    sim: Simulator,
    out: TcpStream,
    incoming: Receiver<Incoming>,
    stops: Option<Receiver<HaltReason>>,
    controller: bool,
    no_ack: Arc<AtomicBool>,
    running: bool,
    interrupted: bool,
    last_stop: HaltReason,
    sw_bps: Vec<u32>,
    hw_bps: Vec<u32>,
    flash: BTreeMap<u32, Vec<u8>>,
    monitor: Option<MonitorFn>,
    gdb_features: Vec<String>,
}

fn parse_hex(s: &[u8]) -> Option<u32> {
    u32::from_str_radix(std::str::from_utf8(s).ok()?, 16).ok()
}

fn split2(s: &[u8], sep: u8) -> Option<(&[u8], &[u8])> {
    let i = s.iter().position(|b| *b == sep)?;
    Some((&s[..i], &s[i + 1..]))
}

impl Connection {
    fn new(id: u64, sim: Simulator, stream: TcpStream, controller: bool, monitor: Option<MonitorFn>) -> Self {
        let no_ack = Arc::new(AtomicBool::new(false));
        let (tx, rx) = unbounded();
        let reader_stream = stream.try_clone().expect("clone stream");
        let na = no_ack.clone();
        std::thread::Builder::new()
            .name(format!("gdb-{id}-rx"))
            .spawn(move || {
                let mut r = Reader::new(reader_stream, na);
                loop {
                    let p = r.next();
                    let closed = p == Incoming::Closed;
                    if tx.send(p).is_err() || closed {
                        break;
                    }
                }
            })
            .ok();
        let stops = if controller { Some(sim.subscribe_stops()) } else { None };
        if controller {
            sim.send(Cmd::Attach { halt: true });
        }
        Connection {
            id,
            sim,
            out: stream,
            incoming: rx,
            stops,
            controller,
            no_ack,
            running: false,
            interrupted: false,
            last_stop: HaltReason::Request,
            sw_bps: Vec::new(),
            hw_bps: Vec::new(),
            flash: BTreeMap::new(),
            monitor,
            gdb_features: Vec::new(),
        }
    }

    fn send(&mut self, data: &[u8]) {
        let _ = self.out.write_all(&frame(data));
    }

    fn send_str(&mut self, s: &str) {
        self.send(s.as_bytes());
    }

    fn run(&mut self) {
        loop {
            if self.running {
                let stops = self.stops.clone().expect("controller");
                select! {
                    recv(self.incoming) -> p => match p {
                        Ok(Incoming::Interrupt) => {
                            self.interrupted = true;
                            self.sim.send(Cmd::Halt);
                        }
                        Ok(Incoming::Packet(p)) => {
                            // only interrupts are expected while running; answer anyway
                            if !self.handle(&p) {
                                return;
                            }
                        }
                        Ok(Incoming::Closed) | Err(_) => return,
                    },
                    recv(stops) -> r => {
                        if let Ok(r) = r {
                            self.running = false;
                            self.last_stop = r;
                            let reply = self.stop_reply(r);
                            self.send_str(&reply);
                        }
                    }
                }
            } else {
                match self.incoming.recv() {
                    Ok(Incoming::Packet(p)) => {
                        if !self.handle(&p) {
                            return;
                        }
                    }
                    Ok(Incoming::Interrupt) => {
                        if self.controller {
                            let r = self.stop_reply(HaltReason::Request);
                            self.send_str(&r);
                        }
                    }
                    Ok(Incoming::Closed) | Err(_) => return,
                }
            }
        }
    }

    fn stop_reply(&mut self, r: HaltReason) -> String {
        let sig = if self.interrupted && r == HaltReason::Request { 2 } else { 5 };
        self.interrupted = false;
        let mut s = format!("T{sig:02x}");
        let regs = self.sim.with(|m| [m.cpu.r[13], m.cpu.r[14], m.cpu.pc]).unwrap_or([0; 3]);
        for (n, v) in [(13u32, regs[0]), (14, regs[1]), (15, regs[2])] {
            s += &format!("{n:02x}:{};", hex(&v.to_le_bytes()));
        }
        match r {
            HaltReason::Watchpoint(addr, kind) => {
                let k = if kind == 1 { "watch" } else { "awatch" };
                s += &format!("{k}:{addr:08x};");
            }
            HaltReason::Breakpoint => {
                let pc = regs[2];
                if self.hw_bps.contains(&pc) && self.feature("hwbreak+") {
                    s += "hwbreak:;";
                } else if self.sw_bps.contains(&pc) && self.feature("swbreak+") {
                    s += "swbreak:;";
                }
            }
            _ => {}
        }
        s += "thread:1;";
        s
    }

    fn feature(&self, f: &str) -> bool {
        self.gdb_features.iter().any(|x| x == f)
    }

    fn resume(&mut self) {
        if !self.controller {
            self.send_str("E01");
            return;
        }
        if let Some(st) = &self.stops {
            while st.try_recv().is_ok() {}
        }
        self.running = true;
        self.sim.send(Cmd::Continue);
    }

    fn step(&mut self) {
        if !self.controller {
            self.send_str("E01");
            return;
        }
        let r = self.sim.step().unwrap_or(HaltReason::Step);
        let r = if r == HaltReason::Step || r == HaltReason::Breakpoint { HaltReason::Step } else { r };
        self.last_stop = r;
        let reply = self.stop_reply(r);
        self.send_str(&reply);
    }

    /// Handles one packet; returns false to close the connection.
    fn handle(&mut self, p: &[u8]) -> bool {
        let Some(&c) = p.first() else {
            self.send_str("");
            return true;
        };
        let body = &p[1..];
        match c {
            b'?' => {
                let r = if self.controller { self.last_stop } else { HaltReason::Request };
                let s = self.stop_reply(r);
                self.send_str(&s);
            }
            b'q' => self.query(body),
            b'Q' => {
                if body == b"StartNoAckMode" {
                    self.send_str("OK");
                    self.no_ack.store(true, Ordering::Relaxed);
                } else {
                    self.send_str("");
                }
            }
            b'H' | b'T' => self.send_str("OK"),
            b'g' => {
                let v = self.read_all_regs();
                self.send_str(&v);
            }
            b'G' => {
                self.write_all_regs(body);
                self.send_str("OK");
            }
            b'p' => match parse_hex(body).and_then(|n| self.read_reg(n)) {
                Some(v) => self.send_str(&hex(&v)),
                None => self.send_str("E01"),
            },
            b'P' => {
                let ok = split2(body, b'=').and_then(|(n, v)| Some((parse_hex(n)?, unhex(v)?))).map(|(n, v)| self.write_reg(n, &v));
                self.send_str(if ok == Some(true) { "OK" } else { "E01" });
            }
            b'm' => {
                let Some((a, l)) = split2(body, b',') else { return self.err() };
                let (Some(a), Some(l)) = (parse_hex(a), parse_hex(l)) else { return self.err() };
                let data = self.sim.read_mem(a, l as usize);
                let valid: Vec<u8> = data.iter().take_while(|b| b.is_some()).map(|b| b.unwrap()).collect();
                if valid.is_empty() && l > 0 {
                    self.send_str("E14");
                } else {
                    self.send_str(&hex(&valid));
                }
            }
            b'M' => {
                let ok = (|| {
                    let (al, data) = split2(body, b':')?;
                    let (a, _) = split2(al, b',')?;
                    Some(self.sim.write_mem(parse_hex(a)?, unhex(data)?))
                })();
                self.send_str(if ok == Some(true) { "OK" } else { "E01" });
            }
            b'X' => {
                let ok = (|| {
                    let (al, data) = split2(body, b':')?;
                    let (a, _) = split2(al, b',')?;
                    Some(self.sim.write_mem(parse_hex(a)?, unescape(data)))
                })();
                self.send_str(if ok == Some(true) { "OK" } else { "E01" });
            }
            b'c' | b'C' => self.resume(),
            b's' | b'S' => self.step(),
            b'v' => return self.v_packet(body),
            b'Z' | b'z' => self.breakpoint(c == b'Z', body),
            b'D' => {
                self.send_str("OK");
                return false;
            }
            b'k' => return false,
            b'R' | b'r' => {
                if self.controller {
                    self.sim.send(Cmd::Reset { halt: true });
                    self.last_stop = HaltReason::VectorCatch;
                }
            }
            b'!' => self.send_str("OK"),
            _ => self.send_str(""),
        }
        true
    }

    fn err(&mut self) -> bool {
        self.send_str("E01");
        true
    }

    fn query(&mut self, body: &[u8]) {
        let s = String::from_utf8_lossy(body).to_string();
        if let Some(rest) = s.strip_prefix("Supported") {
            self.gdb_features = rest.trim_start_matches(':').split(';').map(|x| x.to_string()).collect();
            self.send_str("PacketSize=4000;qXfer:features:read+;qXfer:memory-map:read+;qXfer:threads:read+;QStartNoAckMode+;vContSupported+;swbreak+;hwbreak+");
        } else if let Some(rest) = s.strip_prefix("Xfer:") {
            let parts: Vec<&str> = rest.splitn(4, ':').collect();
            if parts.len() == 4 && parts[1] == "read" {
                let doc = match (parts[0], parts[2]) {
                    ("features", "target.xml") => Some(target_xml()),
                    ("memory-map", _) => Some(memory_map_xml()),
                    ("threads", _) => Some(
                        r#"<?xml version="1.0"?><threads><thread id="1" core="0" name="SSE-320 Cortex-M85"></thread></threads>"#
                            .to_string(),
                    ),
                    _ => None,
                };
                let range = parts[3]
                    .split_once(',')
                    .and_then(|(o, l)| Some((usize::from_str_radix(o, 16).ok()?, usize::from_str_radix(l, 16).ok()?)));
                match (doc, range) {
                    (Some(d), Some((off, len))) => {
                        let bytes = d.as_bytes();
                        if off >= bytes.len() {
                            self.send_str("l");
                        } else {
                            let end = (off + len).min(bytes.len());
                            let mut reply = vec![if end == bytes.len() { b'l' } else { b'm' }];
                            reply.extend(escape(&bytes[off..end]));
                            self.send(&reply);
                        }
                    }
                    _ => self.send_str("E00"),
                }
            } else {
                self.send_str("");
            }
        } else if s == "Attached" {
            self.send_str("1");
        } else if s == "C" {
            self.send_str("QC1");
        } else if s == "fThreadInfo" {
            self.send_str("m1");
        } else if s == "sThreadInfo" {
            self.send_str("l");
        } else if s.starts_with("ThreadExtraInfo") {
            let h = hex(b"Cortex-M85");
            self.send_str(&h);
        } else if s == "Offsets" {
            self.send_str("Text=0;Data=0;Bss=0");
        } else if s.starts_with("Symbol") {
            self.send_str("OK");
        } else if let Some(h) = s.strip_prefix("Rcmd,") {
            let cmd = unhex(h.as_bytes()).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
            let out = self.monitor(cmd.trim());
            if !out.is_empty() {
                let mut text = out;
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                let pkt = format!("O{}", hex(text.as_bytes()));
                self.send_str(&pkt);
            }
            self.send_str("OK");
        } else if let Some(args) = s.strip_prefix("CRC:") {
            let r = args.split_once(',').and_then(|(a, l)| Some((u32::from_str_radix(a, 16).ok()?, usize::from_str_radix(l, 16).ok()?)));
            match r {
                Some((a, l)) => {
                    let data: Option<Vec<u8>> = self.sim.read_mem(a, l).into_iter().collect();
                    match data {
                        Some(d) => self.send_str(&format!("C{:08x}", crc32_gdb(&d))),
                        None => self.send_str("E01"),
                    }
                }
                None => self.send_str("E01"),
            }
        } else {
            self.send_str("");
        }
    }

    fn v_packet(&mut self, body: &[u8]) -> bool {
        let s = String::from_utf8_lossy(body);
        if s == "Cont?" {
            self.send_str("vCont;c;C;s;S;t");
        } else if let Some(actions) = s.strip_prefix("Cont;") {
            let first = actions.split(';').next().unwrap_or("");
            match first.as_bytes().first() {
                Some(b'c') | Some(b'C') => self.resume(),
                Some(b's') | Some(b'S') => self.step(),
                Some(b't') => {
                    if self.controller {
                        self.interrupted = true;
                        self.running = true;
                        self.sim.send(Cmd::Halt);
                    }
                }
                _ => self.send_str("E01"),
            }
        } else if let Some(args) = s.strip_prefix("FlashErase:") {
            let ok = args.split_once(',').and_then(|(a, l)| Some((u32::from_str_radix(a, 16).ok()?, u32::from_str_radix(l, 16).ok()?)));
            if ok.is_some() {
                // the memories need no erase
                self.send_str("OK");
            } else {
                self.send_str("E01");
            }
        } else if body.starts_with(b"FlashWrite:") {
            let rest = &body[b"FlashWrite:".len()..];
            match split2(rest, b':').and_then(|(a, d)| Some((parse_hex(a)?, unescape(d)))) {
                Some((a, d)) => {
                    self.flash.insert(a, d);
                    self.send_str("OK");
                }
                None => self.send_str("E01"),
            }
        } else if s == "FlashDone" {
            let blocks: Vec<(u32, Vec<u8>)> = std::mem::take(&mut self.flash).into_iter().collect();
            let r = self.sim.with(move |m| {
                for (a, d) in &blocks {
                    for (i, b) in d.iter().enumerate() {
                        m.sys.load_byte(a.wrapping_add(i as u32), *b);
                    }
                }
            });
            self.send_str(if r.is_some() { "OK" } else { "E01" });
        } else if s.starts_with("Kill") {
            self.send_str("OK");
            return false;
        } else {
            // unsupported packets (MustReplyEmpty included) get the empty reply
            self.send_str("");
        }
        true
    }

    fn breakpoint(&mut self, insert: bool, body: &[u8]) {
        let s = String::from_utf8_lossy(body);
        let parts: Vec<&str> = s.split([',', ';']).collect();
        if parts.len() < 3 {
            return self.send_str("E01");
        }
        let (Ok(addr), Ok(len)) = (u32::from_str_radix(parts[1], 16), u32::from_str_radix(parts[2], 16)) else {
            return self.send_str("E01");
        };
        let addr = if parts[0] == "0" || parts[0] == "1" { addr & !1 } else { addr };
        let kind = match parts[0] {
            "0" => BpKind::Software,
            "1" => BpKind::Hardware,
            "2" => BpKind::WriteWatch,
            "3" => BpKind::ReadWatch,
            "4" => BpKind::AccessWatch,
            _ => return self.send_str(""),
        };
        let ok = self.sim.breakpoint(kind, addr, len, insert);
        let list = match kind {
            BpKind::Software => Some(&mut self.sw_bps),
            BpKind::Hardware => Some(&mut self.hw_bps),
            _ => None,
        };
        if let Some(l) = list {
            if insert && ok {
                l.push(addr);
            } else if !insert {
                l.retain(|a| *a != addr);
            }
        }
        self.send_str(if ok { "OK" } else { "E01" });
    }

    fn read_reg(&self, n: u32) -> Option<Vec<u8>> {
        let sel = target::reg_map(n)?;
        let v = self.sim.with(move |m| target::read(&m.cpu, sel))??;
        Some(v)
    }

    fn write_reg(&self, n: u32, data: &[u8]) -> bool {
        let Some(sel) = target::reg_map(n) else { return false };
        let data = data.to_vec();
        self.sim.with(move |m| target::write(&mut m.cpu, sel, &data)).unwrap_or(false)
    }

    fn read_all_regs(&self) -> String {
        let v = self
            .sim
            .with(|m| {
                let mut out = Vec::new();
                for n in 0..NUM_REGS {
                    if let Some(sel) = target::reg_map(n) {
                        out.extend(target::read(&m.cpu, sel).unwrap_or_default());
                    }
                }
                out
            })
            .unwrap_or_default();
        hex(&v)
    }

    fn write_all_regs(&self, body: &[u8]) {
        let Some(data) = unhex(body) else { return };
        self.sim.with(move |m| {
            let mut off = 0;
            for n in 0..NUM_REGS {
                if let Some(sel) = target::reg_map(n) {
                    let w = target::width(sel);
                    if off + w <= data.len() {
                        target::write(&mut m.cpu, sel, &data[off..off + w]);
                    }
                    off += w;
                }
            }
        });
    }

    fn monitor(&mut self, cmd: &str) -> String {
        let words: Vec<&str> = cmd.split_whitespace().collect();
        match words.as_slice() {
            ["reset", rest @ ..] => {
                if !self.controller {
                    return "reset is only available on the controlling connection".into();
                }
                // "reset" and "reset halt" halt at the reset vector, "reset run" runs
                let halt = rest.first() != Some(&"run");
                self.sim.send(Cmd::Reset { halt });
                if halt {
                    self.last_stop = HaltReason::VectorCatch;
                }
                if halt {
                    "Resetting target with halt\n".into()
                } else {
                    "Resetting target\n".into()
                }
            }
            ["halt"] => {
                if self.controller {
                    self.sim.send(Cmd::Halt);
                }
                String::new()
            }
            ["help"] | ["?"] => target::monitor_help().into(),
            _ => {
                if let Some(f) = &self.monitor {
                    if let Some(out) = f(&self.sim, cmd) {
                        return out;
                    }
                }
                format!("unknown monitor command '{cmd}' (monitor help lists the commands)\n")
            }
        }
    }
}

/// CRC used by GDB's qCRC (CRC-32 with polynomial 0x04C11DB7, initial value all ones, no
/// reflection, no final XOR).
fn crc32_gdb(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
        }
    }
    crc
}
