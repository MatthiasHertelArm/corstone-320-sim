//! The simulation thread and its handle.
//!
//! The thread owns the [`Machine`]. Everything else (GDB connections, the GUI, the UART
//! bridges) talks to it through [`Simulator`] commands, which are served between execution
//! quanta, so memory reads work while the core runs, as with a debug probe.

use crate::board::Inputs;
use crate::machine::{Machine, Stop};
use crate::sources::{CameraSource, MicSource, Rgba};
use crossbeam_channel::{bounded, unbounded, Receiver, RecvTimeoutError, Sender};
use cs320_core::time::{from_secs, to_secs, TICKS_PER_MS};
use cs320_core::Ticks;
use cs320_cpu::{HaltReason, Watch};
use cs320_loader::Image;
use cs320_soc::board::Frame;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Simulation speed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Speed {
    /// Simulated time follows wall-clock time scaled by the factor.
    Realtime(f64),
    /// As fast as possible.
    Max,
}

/// Input events from the GUI, scenarios or monitor commands.
#[derive(Clone, Debug)]
pub enum Input {
    /// FPGA I/O user button 0/1.
    Button(usize, bool),
    /// FPGA I/O user switch 0..7.
    Switch(usize, bool),
    Switches(u8),
    VioButton(usize, bool),
    VioValue(usize, i32),
    GpioIn(usize, u16),
    Camera(CameraSource),
    Microphone(MicSource),
    /// Microphone noise amplitude (0..1).
    MicNoise(f64),
    ResetButton,
}

/// Breakpoint and watchpoint kinds of the GDB remote protocol (Z0..Z4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BpKind {
    Software,
    Hardware,
    WriteWatch,
    ReadWatch,
    AccessWatch,
}

/// Number of hardware breakpoints (FPB comparators) and watchpoints (DWT comparators).
pub const HW_BREAKPOINTS: usize = 8;
pub const HW_WATCHPOINTS: usize = 4;

type Reply<T> = Sender<T>;

pub enum Cmd {
    /// Debugger attaches (sets C_DEBUGEN); with `halt` the core halts.
    Attach {
        halt: bool,
    },
    Detach,
    Halt,
    Continue,
    /// Single instruction step; replies when done.
    Step(Reply<HaltReason>),
    ReadMem(u32, usize, Reply<Vec<Option<u8>>>),
    WriteMem(u32, Vec<u8>, Reply<bool>),
    ReadReg(u32, Reply<Option<u32>>),
    WriteReg(u32, u32, Reply<bool>),
    Breakpoint {
        kind: BpKind,
        addr: u32,
        len: u32,
        insert: bool,
        reply: Reply<bool>,
    },
    /// Reset; `halt` stops at the reset vector.
    Reset {
        halt: bool,
    },
    /// Writes data blocks into memory (GDB `load`).
    Program(Vec<(u32, Vec<u8>)>, Reply<Result<(), String>>),
    /// Loads an image and resets.
    Load(Image, bool, Reply<Result<(), String>>),
    Input(Input),
    /// Bytes from the host to UART `n`.
    UartIn(usize, Vec<u8>),
    Speed(Speed),
    /// Pauses or resumes the simulation (independent of the debug halt).
    Pause(bool),
    /// Runs a closure on the machine (monitor commands, tests).
    With(Box<dyn FnOnce(&mut Machine) + Send>),
    /// Runs a closure when the simulated time reaches the given time (scenarios).
    At(Ticks, Box<dyn FnOnce(&mut Machine) + Send>),
    Shutdown,
}

/// State published for observers.
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub time: Ticks,
    pub halted: Option<HaltReason>,
    pub paused: bool,
    pub instret: u64,
    pub mips: f64,
    pub realtime_factor: f64,
    pub pc: u32,
    pub secure: bool,
    pub speed_max: bool,
    pub speed_factor: f64,
    pub inputs: Option<Inputs>,
    pub warnings: Vec<String>,
    pub resets: u64,
    /// A debugger is attached (C_DEBUGEN).
    pub debugger: bool,
    /// The simulation reached `Options::end_time` and paused there.
    pub end_reached: bool,
    pub leds: u32,
    pub vio_leds: u32,
    pub vio_values: Vec<i32>,
    pub gpio_out: [(u16, u16); 4],
    pub frame_counts: Vec<u64>,
    pub mic_level: f32,
    pub speaker_level: f32,
    pub camera_connected: bool,
    pub camera_error: Option<String>,
    pub mic_error: Option<String>,
    pub npu_warning: bool,
}

/// Shared between the simulation thread and the observers.
#[derive(Default)]
pub struct Shared {
    pub status: Mutex<Status>,
    /// Latest frame of each display (HDLCD, VSI6, VSI7), replaced as frames arrive.
    pub displays: Mutex<Vec<Option<Frame>>>,
    /// Latest camera frame delivered to the target (for the preview).
    pub camera_preview: Mutex<Option<Rgba>>,
    /// Speaker sample blocks for playback: (time, rate, channels, samples).
    pub speaker: Mutex<Vec<(Ticks, u32, u32, Vec<i16>)>>,
    uart_subscribers: Mutex<Vec<(usize, Sender<Vec<u8>>)>>,
    stop_subscribers: Mutex<Vec<Sender<HaltReason>>>,
}

impl Shared {
    fn broadcast_uart(&self, n: usize, data: &[u8]) {
        let mut subs = self.uart_subscribers.lock().unwrap();
        subs.retain(|(k, s)| *k != n || s.send(data.to_vec()).is_ok());
    }

    fn broadcast_stop(&self, r: HaltReason) {
        let mut subs = self.stop_subscribers.lock().unwrap();
        subs.retain(|s| s.send(r).is_ok());
    }
}

/// Handle to the simulation thread.
#[derive(Clone)]
pub struct Simulator {
    tx: Sender<Cmd>,
    pub shared: Arc<Shared>,
}

/// Options of the simulation thread.
#[derive(Clone, Debug)]
pub struct Options {
    pub speed: Speed,
    /// Start halted (debugger attached, at the reset vector).
    pub start_halted: bool,
    pub halt_on_lockup: bool,
    /// Pause exactly at this simulated time (headless `--time`).
    pub end_time: Option<Ticks>,
}

impl Default for Options {
    fn default() -> Self {
        Options { speed: Speed::Realtime(1.0), start_halted: false, halt_on_lockup: false, end_time: None }
    }
}

impl Simulator {
    /// Starts the simulation thread with a machine.
    pub fn start(machine: Machine, opts: Options) -> Simulator {
        let (tx, rx) = unbounded();
        let shared = Arc::new(Shared::default());
        let sh = shared.clone();
        std::thread::Builder::new()
            .name("simulation".into())
            .spawn(move || SimThread::new(machine, rx, sh, opts).run())
            .expect("spawn simulation thread");
        Simulator { tx, shared }
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    fn call<T>(&self, f: impl FnOnce(Reply<T>) -> Cmd) -> Option<T> {
        let (rtx, rrx) = bounded(1);
        self.tx.send(f(rtx)).ok()?;
        rrx.recv().ok()
    }

    pub fn read_mem(&self, addr: u32, len: usize) -> Vec<Option<u8>> {
        self.call(|r| Cmd::ReadMem(addr, len, r)).unwrap_or_default()
    }

    pub fn write_mem(&self, addr: u32, data: Vec<u8>) -> bool {
        self.call(|r| Cmd::WriteMem(addr, data, r)).unwrap_or(false)
    }

    pub fn read_reg(&self, n: u32) -> Option<u32> {
        self.call(|r| Cmd::ReadReg(n, r)).flatten()
    }

    pub fn write_reg(&self, n: u32, v: u32) -> bool {
        self.call(|r| Cmd::WriteReg(n, v, r)).unwrap_or(false)
    }

    pub fn step(&self) -> Option<HaltReason> {
        self.call(Cmd::Step)
    }

    pub fn breakpoint(&self, kind: BpKind, addr: u32, len: u32, insert: bool) -> bool {
        self.call(|reply| Cmd::Breakpoint { kind, addr, len, insert, reply }).unwrap_or(false)
    }

    pub fn program(&self, blocks: Vec<(u32, Vec<u8>)>) -> Result<(), String> {
        self.call(|r| Cmd::Program(blocks, r)).unwrap_or_else(|| Err("simulator stopped".into()))
    }

    pub fn load(&self, img: Image, halt: bool) -> Result<(), String> {
        self.call(|r| Cmd::Load(img, halt, r)).unwrap_or_else(|| Err("simulator stopped".into()))
    }

    /// Runs `f` on the machine in the simulation thread and returns its result.
    pub fn with<T: Send + 'static>(&self, f: impl FnOnce(&mut Machine) -> T + Send + 'static) -> Option<T> {
        let (rtx, rrx) = bounded(1);
        self.tx
            .send(Cmd::With(Box::new(move |m| {
                let _ = rtx.send(f(m));
            })))
            .ok()?;
        rrx.recv().ok()
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap().clone()
    }

    /// Receives every byte the target sends on UART `n`.
    pub fn subscribe_uart(&self, n: usize) -> Receiver<Vec<u8>> {
        let (tx, rx) = unbounded();
        self.shared.uart_subscribers.lock().unwrap().push((n, tx));
        rx
    }

    /// Receives the reasons of debug halts.
    pub fn subscribe_stops(&self) -> Receiver<HaltReason> {
        let (tx, rx) = unbounded();
        self.shared.stop_subscribers.lock().unwrap().push(tx);
        rx
    }

    /// Takes the latest frame of a display.
    pub fn take_frame(&self, display: usize) -> Option<Frame> {
        self.shared.displays.lock().unwrap().get_mut(display).and_then(|f| f.take())
    }

    pub fn take_speaker(&self) -> Vec<(Ticks, u32, u32, Vec<i16>)> {
        std::mem::take(&mut *self.shared.speaker.lock().unwrap())
    }
}

struct SimThread {
    m: Machine,
    rx: Receiver<Cmd>,
    shared: Arc<Shared>,
    opts: Options,
    paused: bool,
    end_reached: bool,
    debug_halted: Option<HaltReason>,
    // pacing
    wall_origin: Instant,
    sim_origin: Ticks,
    last_publish: Instant,
    mips_instret: u64,
    mips_wall: Instant,
    mips_sim: Ticks,
    mips: f64,
    rt_factor: f64,
    timed: Vec<(Ticks, u64, TimedAction)>,
    timed_seq: u64,
}

/// Work queued for a simulated time.
type TimedAction = Box<dyn FnOnce(&mut Machine) + Send>;

impl SimThread {
    fn new(mut m: Machine, rx: Receiver<Cmd>, shared: Arc<Shared>, opts: Options) -> Self {
        m.halt_on_lockup = opts.halt_on_lockup;
        let mut debug_halted = None;
        if opts.start_halted {
            m.cpu.c_debugen = true;
            m.cpu.halted = Some(HaltReason::Request);
            m.cpu.c_halt = true;
            debug_halted = Some(HaltReason::Request);
        }
        shared.displays.lock().unwrap().resize(3, None);
        let now = Instant::now();
        SimThread {
            m,
            rx,
            shared,
            opts,
            paused: false,
            end_reached: false,
            debug_halted,
            wall_origin: now,
            sim_origin: 0,
            last_publish: now,
            mips_instret: 0,
            mips_wall: now,
            mips_sim: 0,
            mips: 0.0,
            rt_factor: 0.0,
            timed: Vec::new(),
            timed_seq: 0,
        }
    }

    fn run_timed(&mut self) {
        let now = self.m.now();
        while let Some(i) = self.timed.iter().position(|(t, _, _)| *t <= now) {
            let (_, _, f) = self.timed.remove(i);
            f(&mut self.m);
        }
    }

    fn next_timed(&self) -> Ticks {
        self.timed.iter().map(|(t, _, _)| *t).min().unwrap_or(Ticks::MAX)
    }

    fn resync_pacing(&mut self) {
        self.wall_origin = Instant::now();
        self.sim_origin = self.m.now();
    }

    fn running(&self) -> bool {
        !self.paused && self.debug_halted.is_none()
    }

    fn run(mut self) {
        loop {
            // commands
            loop {
                let cmd = if self.running() {
                    match self.rx.try_recv() {
                        Ok(c) => c,
                        Err(_) => break,
                    }
                } else {
                    match self.rx.recv_timeout(Duration::from_millis(16)) {
                        Ok(c) => c,
                        Err(RecvTimeoutError::Timeout) => {
                            self.publish(true);
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                };
                if !self.handle(cmd) {
                    return;
                }
                if self.running() {
                    continue;
                }
            }
            // run a quantum
            let quantum = TICKS_PER_MS / 2;
            let mut target = (self.m.now() + quantum).min(self.next_timed().max(self.m.now() + 1));
            if let Some(end) = self.opts.end_time {
                target = target.min(end);
            }
            let stop = self.m.run_until(target);
            if self.opts.end_time.is_some_and(|end| self.m.now() >= end) {
                self.end_reached = true;
                self.paused = true;
            }
            self.run_timed();
            match stop {
                Stop::Time => {}
                Stop::Exited => {
                    self.drain_outputs();
                    self.paused = true;
                }
                Stop::Halted(r) => {
                    self.debug_halted = Some(r);
                    self.shared.broadcast_stop(r);
                }
            }
            self.drain_outputs();
            self.publish(false);
            self.pace();
        }
    }

    fn pace(&mut self) {
        if let Speed::Realtime(f) = self.opts.speed {
            let sim_elapsed = to_secs(self.m.now() - self.sim_origin) / f.max(1e-6);
            let wall_elapsed = self.wall_origin.elapsed().as_secs_f64();
            if sim_elapsed > wall_elapsed {
                let d = sim_elapsed - wall_elapsed;
                if d > 0.0005 {
                    std::thread::sleep(Duration::from_secs_f64(d.min(0.05)));
                }
            } else if wall_elapsed - sim_elapsed > 0.25 {
                // fell behind (slow host or a debugger stop): do not try to catch up
                self.resync_pacing();
            }
        }
    }

    fn drain_outputs(&mut self) {
        let out = self.m.take_uart();
        for (n, data) in out.iter().enumerate() {
            if !data.is_empty() {
                self.shared.broadcast_uart(n, data);
            }
        }
        let b = &mut self.m.sys.board;
        for d in 0..b.displays.len() {
            if let Some(f) = b.take_frame(d) {
                let mut sh = self.shared.displays.lock().unwrap();
                if sh.len() <= d {
                    sh.resize(d + 1, None);
                }
                sh[d] = Some(f);
            }
        }
        if let Some(p) = b.camera.preview.take() {
            *self.shared.camera_preview.lock().unwrap() = Some(p);
        }
        if !b.speaker.blocks.is_empty() {
            let mut s = self.shared.speaker.lock().unwrap();
            s.append(&mut b.speaker.blocks);
            if s.len() > 512 {
                let n = s.len() - 256;
                s.drain(..n);
            }
        }
    }

    fn publish(&mut self, force: bool) {
        if !force && self.last_publish.elapsed() < Duration::from_millis(15) {
            return;
        }
        self.last_publish = Instant::now();
        let wall = self.mips_wall.elapsed().as_secs_f64();
        if wall > 0.5 {
            self.mips = (self.m.cpu.instret - self.mips_instret) as f64 / wall / 1e6;
            self.rt_factor = to_secs(self.m.now() - self.mips_sim) / wall;
            self.mips_instret = self.m.cpu.instret;
            self.mips_wall = Instant::now();
            self.mips_sim = self.m.now();
        }
        let mut s = self.shared.status.lock().unwrap();
        s.time = self.m.now();
        s.halted = self.debug_halted;
        s.paused = self.paused;
        s.end_reached = self.end_reached;
        s.instret = self.m.cpu.instret;
        s.mips = self.mips;
        s.realtime_factor = self.rt_factor;
        s.pc = self.m.cpu.pc;
        s.secure = self.m.cpu.secure;
        s.resets = self.m.resets;
        s.debugger = self.m.cpu.c_debugen;
        match self.opts.speed {
            Speed::Max => {
                s.speed_max = true;
                s.speed_factor = 0.0;
            }
            Speed::Realtime(f) => {
                s.speed_max = false;
                s.speed_factor = f;
            }
        }
        let b = &self.m.sys.board;
        s.inputs = Some(b.inputs.clone());
        s.leds = b.leds;
        s.vio_leds = b.vio_leds;
        s.vio_values = b.vio_values.to_vec();
        s.gpio_out = b.gpio_out;
        s.frame_counts = b.frame_counts.clone();
        s.mic_level = b.mic.level;
        s.speaker_level = b.speaker.level;
        s.camera_connected = b.camera.connected();
        s.camera_error = b.camera.error.clone();
        s.mic_error = b.mic.error.clone();
        if s.warnings.len() != self.m.sys.warn.list.len() {
            s.warnings = self.m.sys.warn.list.iter().map(|(_, w)| w.clone()).collect();
        }
    }

    fn halt_now(&mut self, r: HaltReason) {
        self.m.cpu.halted = Some(r);
        self.m.cpu.c_halt = true;
        self.debug_halted = Some(r);
        self.shared.broadcast_stop(r);
    }

    fn handle(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Attach { halt } => {
                self.m.cpu.c_debugen = true;
                // a probe enables the cycle counter for the debugger's CPU time
                self.m.cpu.demcr |= 1 << 24;
                self.m.cpu.dwt.ctrl |= 1;
                let now = self.m.now();
                self.m.cpu.dwt.update_running(now, true);
                if halt && self.debug_halted.is_none() {
                    self.halt_now(HaltReason::Request);
                }
            }
            Cmd::Detach => {
                self.m.cpu.c_debugen = false;
                self.m.cpu.sw_breakpoints.clear();
                self.m.cpu.watchpoints.clear();
                self.m.cpu.fpb.ctrl = 0;
                if self.debug_halted.take().is_some() {
                    self.m.cpu.resume();
                    self.resync_pacing();
                }
            }
            Cmd::Halt => {
                if self.debug_halted.is_none() {
                    self.halt_now(HaltReason::Request);
                }
            }
            Cmd::Continue => {
                if self.debug_halted.take().is_some() {
                    self.m.cpu.resume();
                    self.resync_pacing();
                }
            }
            Cmd::Step(reply) => {
                let r = self.step();
                let _ = reply.send(r);
            }
            Cmd::ReadMem(addr, len, reply) => {
                let mut v = Vec::with_capacity(len);
                let mut a = addr;
                let end = addr.wrapping_add(len as u32);
                while a != end {
                    if a & 3 == 0 && end.wrapping_sub(a) >= 4 {
                        match self.m.cpu.debug_read(&mut self.m.sys, a, 4) {
                            Some(w) => v.extend(w.to_le_bytes().map(Some)),
                            None => v.extend([None; 4]),
                        }
                        a = a.wrapping_add(4);
                    } else {
                        v.push(self.m.cpu.debug_read(&mut self.m.sys, a, 1).map(|b| b as u8));
                        a = a.wrapping_add(1);
                    }
                }
                let _ = reply.send(v);
            }
            Cmd::WriteMem(addr, data, reply) => {
                let mut ok = true;
                let mut i = 0;
                while i < data.len() {
                    let a = addr.wrapping_add(i as u32);
                    if a & 3 == 0 && data.len() - i >= 4 {
                        let w = u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
                        ok &= self.m.cpu.debug_write(&mut self.m.sys, a, 4, w);
                        i += 4;
                    } else {
                        ok &= self.m.cpu.debug_write(&mut self.m.sys, a, 1, data[i] as u32);
                        i += 1;
                    }
                }
                let _ = reply.send(ok);
            }
            Cmd::ReadReg(n, reply) => {
                let _ = reply.send(self.m.cpu.read_core_reg(n));
            }
            Cmd::WriteReg(n, v, reply) => {
                let _ = reply.send(self.m.cpu.write_core_reg(n, v));
            }
            Cmd::Breakpoint { kind, addr, len, insert, reply } => {
                let _ = reply.send(self.breakpoint(kind, addr, len, insert));
            }
            Cmd::Reset { halt } => {
                self.m.reset_button();
                if halt {
                    self.m.cpu.c_debugen = true;
                    self.halt_now(HaltReason::VectorCatch);
                } else if self.debug_halted.take().is_some() {
                    self.m.cpu.resume();
                }
                self.resync_pacing();
            }
            Cmd::Program(blocks, reply) => {
                let mut res = Ok(());
                for (addr, data) in &blocks {
                    for (i, b) in data.iter().enumerate() {
                        if !self.m.sys.load_byte(addr.wrapping_add(i as u32), *b) {
                            res = Err(format!("address {:#010x} is not memory", addr.wrapping_add(i as u32)));
                        }
                    }
                }
                let _ = reply.send(res);
            }
            Cmd::Load(img, halt, reply) => {
                let r = self.m.load_image(&img);
                self.m.reset_button();
                if halt {
                    self.halt_now(HaltReason::VectorCatch);
                } else if self.debug_halted.take().is_some() {
                    self.m.cpu.resume();
                }
                self.resync_pacing();
                let _ = reply.send(r);
            }
            Cmd::Input(i) => self.input(i),
            Cmd::UartIn(n, data) => self.m.uart_send(n, &data),
            Cmd::Speed(s) => {
                self.opts.speed = s;
                self.resync_pacing();
            }
            Cmd::Pause(p) => {
                self.paused = p;
                if !p {
                    self.resync_pacing();
                }
                self.publish(true);
            }
            Cmd::With(f) => f(&mut self.m),
            Cmd::At(t, f) => {
                self.timed_seq += 1;
                self.timed.push((t, self.timed_seq, f));
                self.timed.sort_by_key(|(t, s, _)| (*t, *s));
                self.run_timed();
            }
            Cmd::Shutdown => return false,
        }
        self.publish(true);
        true
    }

    fn input(&mut self, i: Input) {
        let b = &mut self.m.sys.board;
        match i {
            Input::Button(n, p) => b.inputs.buttons[n.min(1)] = p,
            Input::Switch(n, on) => {
                let bit = 1u8 << (n.min(7));
                b.inputs.switches = if on { b.inputs.switches | bit } else { b.inputs.switches & !bit };
            }
            Input::Switches(m) => b.inputs.switches = m,
            Input::VioButton(n, p) => {
                let bit = 1u32 << (n.min(31));
                b.inputs.vio_buttons = if p { b.inputs.vio_buttons | bit } else { b.inputs.vio_buttons & !bit };
            }
            Input::VioValue(id, v) => {
                b.inputs.vio_values.retain(|(i, _)| *i != id);
                b.inputs.vio_values.push((id, v));
            }
            Input::GpioIn(port, v) => b.inputs.gpio_in[port.min(3)] = v,
            Input::Camera(src) => b.camera.set_source(src),
            Input::Microphone(src) => b.mic.set_source(src),
            Input::MicNoise(n) => b.mic.noise = n,
            Input::ResetButton => {
                self.m.reset_button();
                self.resync_pacing();
            }
        }
        let now = self.m.now();
        self.m.sys.inputs_changed(now);
    }

    /// One instruction step with interrupts masked (pyOCD's default); waits through sleep.
    fn step(&mut self) -> HaltReason {
        let cpu = &mut self.m.cpu;
        cpu.c_debugen = true;
        let mask = cpu.c_maskints;
        cpu.c_maskints = true;
        cpu.mark_dirty();
        cpu.resume();
        cpu.skip_bp_once();
        cpu.step_request = true;
        let limit = self.m.now() + from_secs(1.0);
        let r = match self.m.run_until(limit) {
            Stop::Halted(r) => r,
            Stop::Exited => HaltReason::Request,
            _ => {
                // still sleeping: report the halt at the current position
                self.m.cpu.step_request = false;
                self.m.cpu.halted = Some(HaltReason::Step);
                HaltReason::Step
            }
        };
        self.m.cpu.c_maskints = mask;
        self.m.cpu.mark_dirty();
        self.debug_halted = Some(r);
        self.drain_outputs();
        r
    }

    fn breakpoint(&mut self, kind: BpKind, addr: u32, len: u32, insert: bool) -> bool {
        let cpu = &mut self.m.cpu;
        match kind {
            BpKind::Software | BpKind::Hardware => {
                // hardware breakpoints use the FPB like a probe; software ones are unlimited
                if kind == BpKind::Hardware {
                    let comp = (addr & !1) | 1;
                    if insert {
                        if cpu.fpb.comp.contains(&comp) {
                            return true;
                        }
                        match cpu.fpb.free_comparator() {
                            Some(i) => {
                                cpu.fpb.comp[i] = comp;
                                cpu.fpb.ctrl |= 1;
                                true
                            }
                            None => false,
                        }
                    } else {
                        for c in cpu.fpb.comp.iter_mut() {
                            if *c == comp {
                                *c = 0;
                            }
                        }
                        true
                    }
                } else if insert {
                    if !cpu.sw_breakpoints.contains(&addr) {
                        cpu.sw_breakpoints.push(addr);
                    }
                    true
                } else {
                    cpu.sw_breakpoints.retain(|a| *a != addr);
                    true
                }
            }
            _ => {
                let k = match kind {
                    BpKind::WriteWatch => 2,
                    BpKind::ReadWatch => 1,
                    _ => 3,
                };
                let w = Watch { addr, len: len.max(1), kind: k };
                if insert {
                    if cpu.watchpoints.len() >= HW_WATCHPOINTS {
                        return false;
                    }
                    cpu.watchpoints.push(w);
                } else {
                    cpu.watchpoints.retain(|x| *x != w);
                }
                cpu.refresh_watch();
                true
            }
        }
    }
}
