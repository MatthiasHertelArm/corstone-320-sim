//! Register-level model of the Corstone SSE-320 subsystem and the MPS4 board peripherals as
//! the `FVP_Corstone_SSE-320` presents them: the memories, the system control and timing
//! blocks, the CMSDK UARTs, GPIO, the FPGA I/O, I2C, SPI, I2S, the HDLCD display, the RTC, the
//! AVH virtual interfaces (VIO, VSI), the simulator's camera capture block, and the Ethos-U85
//! and DMA-350 register interfaces.
//!
//! [`System`] implements the core's [`cs320_cpu::Bus`].

pub mod board;
pub mod cam;
pub mod dma;
pub mod env;
pub mod fpgaio;
pub mod fvp;
pub mod gpio;
pub mod hdlcd;
pub mod i2c;
pub mod i2s;
pub mod mem;
pub mod npu;
pub mod rtc;
pub mod spi;
pub mod sysctrl;
pub mod timer;
pub mod uart;
pub mod vio;
pub mod vsi;
pub mod wdog;

use board::BoardIo;
use cam::Camera;
use cs320_core::{Scheduler, Slot, Ticks};
use cs320_cpu::{Bus, BusError, Idau, IrqLines};
use dma::Dma350;
use env::{slot, Clocks, Env, Warnings};
use fpgaio::{FpgaIo, Scc};
use gpio::CmsdkGpio;
use hdlcd::Hdlcd;
use i2c::SbconI2c;
use i2s::AudioI2s;
use mem::Memory;
use npu::EthosU;
use rtc::Pl031;
use spi::Pl022;
use std::collections::{HashMap, HashSet};
use sysctrl::{RegBlock, SysCtrl};
use timer::{CmsdkTimer, SysCounter, SysTimer};
use uart::CmsdkUart;
use vio::Vio;
use vsi::Vsi;
use wdog::{CmsdkWdog, SysWdog};

/// Interrupt numbers of the SSE-320 (`platform_irq.h`).
pub mod irq {
    pub const NONSEC_WATCHDOG_RESET_REQ: usize = 0;
    pub const NONSEC_WATCHDOG: usize = 1;
    pub const SLOWCLK_TIMER: usize = 2;
    pub const TIMER0: usize = 3;
    pub const TIMER1: usize = 4;
    pub const TIMER2: usize = 5;
    pub const MPC: usize = 9;
    pub const PPC: usize = 10;
    pub const NPU0: usize = 16;
    pub const DMA_SEC_COMBINED: usize = 24;
    pub const DMA_NONSEC_COMBINED: usize = 25;
    pub const TIMER3: usize = 27;
    pub const UART_RX: [usize; 6] = [33, 35, 37, 39, 41, 125];
    pub const UART_TX: [usize; 6] = [34, 36, 38, 40, 42, 126];
    pub const UART_COMBINED: [usize; 6] = [43, 44, 45, 46, 47, 127];
    pub const UART_OVF: usize = 48;
    pub const I2S: usize = 50;
    pub const DMA_CHANNEL: [usize; 2] = [57, 58];
    pub const GPIO: [usize; 4] = [69, 70, 71, 72];
    pub const RTC: usize = 128;
    pub const ISP: usize = 132;
    pub const HDLCD: usize = 133;
    pub const VSI: [usize; 8] = [224, 225, 226, 227, 228, 229, 230, 231];
}

/// Parameters of the subsystem: the single fixed Corstone-320 configuration (no `-C` parameters).
#[derive(Clone, Debug)]
pub struct Params {
    pub clocks: Clocks,
    /// `mps4_board.subsystem.iotss3_systemcontrol.INITSVTOR_RST` / `cpu0.INITSVTOR`.
    pub initsvtor: u32,
    /// `mps4_board.QSPI_SRAM_SIZE` in MB.
    pub qspi_size_mb: u32,
    /// Ethos-U85 MACs per cycle (`mps4_board.subsystem.ethosu.num_macs` on the FVP). The model
    /// must be compiled for the same configuration (Vela `ethos-u85-256`).
    pub npu_macs: u32,
    /// `mps4_board.subsystem.cpu0.CFGPACBTI`: PACBTI extension implemented (the FVP default is 0).
    pub pacbti: bool,
    /// `mps4_board.pl370_hdlcd.force_frame_rate` (0 = from the timing registers).
    pub hdlcd_frame_rate: u32,
    /// `mps4_board.rtc.RTCDR_use_current_time` / `RTCDR_reset_value`.
    pub rtc_current_time: bool,
    pub rtc_reset_value: u32,
    /// The system counter runs from reset (`refcounter.non_arch_start_at_default`).
    pub counter_at_reset: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            clocks: Clocks::default(),
            initsvtor: 0x1100_0000,
            qspi_size_mb: 128,
            npu_macs: 256,
            pacbti: false,
            hdlcd_frame_rate: 50,
            rtc_current_time: true,
            rtc_reset_value: 0,
            counter_at_reset: true,
        }
    }
}

/// The subsystem: memories and peripherals, with a board attached.
pub struct System<B: BoardIo> {
    pub params: Params,
    pub mem: Memory,
    pub sched: Scheduler,
    pub board: B,
    pub warn: Warnings,
    pub sysctrl: SysCtrl,
    pub sysinfo: RegBlock,
    pub sacfg: RegBlock,
    pub nsacfg: RegBlock,
    pub mpc: [RegBlock; 5],
    pub blocks: HashMap<u32, RegBlock>,
    pub counter: SysCounter,
    pub timers: [SysTimer; 4],
    pub slowclk_timer: CmsdkTimer,
    pub wdog: [SysWdog; 2],
    pub slowclk_wdog: CmsdkWdog,
    pub uart: [CmsdkUart; 6],
    pub fpgaio: FpgaIo,
    pub scc: Scc,
    pub gpio: [CmsdkGpio; 4],
    pub i2c: [SbconI2c; 5],
    pub spi: [Pl022; 3],
    pub i2s: AudioI2s,
    pub hdlcd: Hdlcd,
    pub rtc: Pl031,
    pub vio: Vio,
    pub vsi: [Vsi; 8],
    pub cam: Camera,
    pub npu: EthosU,
    pub dma: Dma350,
    /// A reset requested by a watchdog or SWRESET (with the RESET_SYNDROME bit).
    pub reset_request: Option<u32>,
    /// Register storage of unmodelled peripherals.
    pub unimpl: HashMap<u32, u32>,
    reported: HashSet<u32>,
    irq_lines: IrqLines,
    irq_dirty: bool,
    deadline: Ticks,
    board_next: Ticks,
    /// Time of the last bus access (for interrupt line evaluation).
    deadline_now: Ticks,
    /// Log every peripheral access at debug level (`CS320_TRACE=periph`).
    pub trace: bool,
}

/// Identifies a peripheral at an address (Secure and Non-secure aliases resolved).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Periph {
    Dma,
    Npu,
    CpuPwrCtrl,
    CpuIdentity,
    CpuSecCtrl,
    Nsacfg,
    Sacfg,
    Mpc(usize),
    Block(u32),
    Gpio(usize),
    Hdlcd,
    SysTimer(usize),
    SysInfo,
    SysCtrl,
    SlowClkWdog,
    SlowClkTimer,
    Wdog(usize),
    CounterCtrl,
    CounterRead,
    I2c(usize),
    Spi(usize),
    Scc,
    I2s,
    FpgaIo,
    Uart(usize),
    Rtc,
    Isp,
    Cam,
    Vio,
    Vsi(usize),
    Unmodelled(&'static str),
}

fn periph_of(addr: u32) -> Option<(Periph, u32)> {
    let secure = addr & 0x1000_0000 != 0;
    let a = addr & !0x1000_0000; // the Non-secure alias
    let off = a & 0xFFF;
    let p = match a & 0xFFFF_F000 {
        0x4000_2000 | 0x4000_3000 => return Some((Periph::Dma, a - 0x4000_2000)),
        0x4000_4000 | 0x4000_5000 => return Some((Periph::Npu, a - 0x4000_4000)),
        0x4001_1000 if secure => Periph::CpuSecCtrl,
        0x4001_2000 => Periph::CpuPwrCtrl,
        0x4001_F000 => Periph::CpuIdentity,
        0x4008_0000 => {
            if secure {
                Periph::Sacfg
            } else {
                Periph::Nsacfg
            }
        }
        0x4008_3000 if secure => Periph::Mpc(3),
        0x4008_4000 if secure => Periph::Mpc(4),
        0x4009_4000 if secure => Periph::Unmodelled("CC3XX"),
        0x4009_E000 if secure => Periph::Block(0x5009_E000),
        0x400A_0000 if secure => Periph::Block(0x500A_0000),
        0x4010_0000..=0x4010_3000 => Periph::Gpio(((a - 0x4010_0000) >> 12) as usize),
        0x4010_4000..=0x4010_7000 => Periph::Unmodelled("AHB USER"),
        0x4031_0000 => Periph::Hdlcd,
        0x4040_0000 => Periph::Unmodelled("Ethernet"),
        0x4050_0000 => Periph::Unmodelled("USB"),
        0x4070_0000..=0x4070_3000 => Periph::Unmodelled("USER APB"),
        0x4080_0000 | 0x4080_1000 => Periph::Unmodelled("QSPI"),
        0x4700_0000..=0x4700_2000 if secure => Periph::Mpc(((a - 0x4700_0000) >> 12) as usize),
        0x4800_0000..=0x4800_3000 => Periph::SysTimer(((a - 0x4800_0000) >> 12) as usize),
        0x4802_0000 => Periph::SysInfo,
        0x4802_1000 if secure => Periph::SysCtrl,
        0x4802_2000..=0x4802_A000 if secure => Periph::Block(addr & 0xFFFF_F000),
        0x4802_E000 if secure => Periph::SlowClkWdog,
        0x4802_F000 => Periph::SlowClkTimer,
        0x4804_0000 | 0x4804_1000 => {
            return Some((Periph::Wdog(secure as usize), a - 0x4804_0000));
        }
        0x4804_2000 if secure => Periph::Block(0x5804_2000),
        0x4810_0000 => {
            if secure {
                Periph::CounterCtrl
            } else {
                Periph::I2c(0)
            }
        }
        0x4810_1000 => Periph::CounterRead,
        0x4810_2000..=0x4810_4000 => Periph::Spi(((a - 0x4810_2000) >> 12) as usize),
        0x4810_5000 => Periph::I2c(1),
        0x4810_6000 => Periph::I2c(2),
        0x4810_7000 => Periph::Unmodelled("USER APB"),
        0x4810_8000 => Periph::I2c(3),
        0x4810_9000 => Periph::I2c(4),
        0x4820_0000 => Periph::Scc,
        0x4820_1000 => Periph::I2s,
        0x4820_2000 => Periph::FpgaIo,
        0x4820_3000..=0x4820_8000 => Periph::Uart(((a - 0x4820_3000) >> 12) as usize),
        // the SSE_320_BSP defines UART0_BASE_S as 0x5920_3000; the FVP answers there too
        0x4920_3000 if secure => Periph::Uart(0),
        0x4820_B000 => Periph::Rtc,
        0x4830_0000..=0x483F_F000 => return Some((Periph::Isp, a - 0x4830_0000)),
        0x4840_0000 => Periph::Cam,
        0x4FEE_0000 => Periph::Unmodelled("VSocket"),
        0x4FEF_0000 => Periph::Vio,
        0x4FF0_0000..=0x4FF7_F000 => return Some((Periph::Vsi(((a - 0x4FF0_0000) >> 16) as usize), a & 0xFFFF)),
        _ => return None,
    };
    Some((p, off))
}

/// Names of unmodelled register blocks at 0x5802_xxxx (the power policy units) and others.
fn block_name(base: u32) -> &'static str {
    match base {
        0x5802_2000 => "SYS PPU",
        0x5802_3000 => "CPU0 PPU",
        0x5802_8000 => "MGMT PPU",
        0x5802_9000 => "DBG PPU",
        0x5802_A000 => "NPU0 PPU",
        0x5804_2000 => "SAM",
        0x5009_E000 => "KMU",
        0x500A_0000 => "LCM",
        _ => "reserved",
    }
}

impl<B: BoardIo> System<B> {
    pub fn new(board: B, params: Params) -> Self {
        let c = params.clocks;
        let mut s = System {
            mem: Memory::new(params.qspi_size_mb.clamp(1, 128) << 20),
            sched: Scheduler::new(slot::COUNT),
            warn: Warnings::default(),
            sysctrl: SysCtrl::new(params.initsvtor),
            sysinfo: RegBlock::new("SYSINFO", sysctrl::SYSINFO_PIDR, sysctrl::SYSINFO_DEFAULTS).with_base(0x5802_0000),
            sacfg: RegBlock::new("SACFG", sysctrl::SACFG_PIDR, sysctrl::SACFG_DEFAULTS).with_base(0x5008_0000),
            nsacfg: RegBlock::new("NSACFG", sysctrl::NSACFG_PIDR, &[]).with_base(0x4008_0000),
            mpc: std::array::from_fn(|i| {
                let base = [0x5700_0000, 0x5700_1000, 0x5700_2000, 0x5008_3000, 0x5008_4000][i];
                RegBlock::new("MPC", sysctrl::MPC_PIDR, sysctrl::MPC_DEFAULTS).with_base(base)
            }),
            blocks: HashMap::new(),
            counter: SysCounter::new(c.counter_hz),
            timers: std::array::from_fn(SysTimer::new),
            slowclk_timer: CmsdkTimer::new(c.slow_hz, slot::SLOWCLK_TIMER).slowclk(),
            wdog: std::array::from_fn(SysWdog::new),
            slowclk_wdog: CmsdkWdog::new(c.slow_hz, slot::SLOWCLK_WDOG),
            uart: std::array::from_fn(CmsdkUart::new),
            fpgaio: FpgaIo::new(c.cpu_hz),
            scc: Scc::new(),
            gpio: std::array::from_fn(CmsdkGpio::new),
            i2c: std::array::from_fn(SbconI2c::new),
            spi: std::array::from_fn(|_| Pl022::new()),
            i2s: AudioI2s::new(),
            hdlcd: Hdlcd::new(),
            rtc: Pl031::new(slot::RTC),
            vio: Vio::new(),
            vsi: std::array::from_fn(Vsi::new),
            cam: Camera::new(),
            npu: EthosU::new(params.npu_macs),
            dma: Dma350::new(),
            reset_request: None,
            unimpl: HashMap::new(),
            reported: HashSet::new(),
            irq_lines: [0; 8],
            irq_dirty: true,
            deadline: Ticks::MAX,
            board_next: Ticks::MAX,
            deadline_now: 0,
            trace: std::env::var("CS320_TRACE").is_ok_and(|v| v.contains("periph")),
            board,
            params,
        };
        s.hdlcd.force_frame_rate = s.params.hdlcd_frame_rate;
        s.hdlcd.pixel_clock_hz = s.params.clocks.periph_hz;
        s.reset(0, false, 1);
        s
    }

    /// Resets all peripherals; `syndrome` is the RESET_SYNDROME bit of the cause.
    pub fn reset(&mut self, now: Ticks, warm: bool, syndrome: u32) {
        self.sched.clear();
        if !warm {
            self.mem.power_on();
        }
        self.sysctrl.reset(syndrome, !warm);
        self.sysinfo.reset();
        self.sacfg.reset();
        self.nsacfg.reset();
        for m in &mut self.mpc {
            m.reset();
        }
        for b in self.blocks.values_mut() {
            b.reset();
        }
        self.counter.reset(now, self.params.counter_at_reset);
        for t in &mut self.timers {
            t.reset();
        }
        self.slowclk_timer.reset(now);
        for w in &mut self.wdog {
            w.reset();
        }
        self.slowclk_wdog.reset(now);
        for u in &mut self.uart {
            u.reset();
        }
        self.fpgaio.reset(now);
        self.scc.reset();
        for g in &mut self.gpio {
            g.reset();
        }
        for i in &mut self.i2c {
            i.reset();
        }
        for s in &mut self.spi {
            s.reset();
        }
        self.i2s.reset();
        self.hdlcd.reset();
        let epoch = if self.params.rtc_current_time { self.board.rtc_epoch() } else { self.params.rtc_reset_value };
        self.rtc.reset(now, epoch);
        self.vio.reset();
        for v in &mut self.vsi {
            v.reset();
        }
        self.cam.reset();
        self.npu.reset();
        self.dma.reset();
        self.reset_request = None;
        self.unimpl.clear();
        self.irq_dirty = true;
        self.update_deadline();
    }

    fn update_deadline(&mut self) {
        self.deadline = self.sched.next_time().unwrap_or(Ticks::MAX).min(self.board_next);
    }

    /// Time of the next device or board event.
    pub fn next_event(&mut self) -> Ticks {
        self.sched.next_time().unwrap_or(Ticks::MAX).min(self.board_next)
    }

    /// Sets the time at which the board changes an input (buttons for the GPIO interrupts).
    pub fn inputs_changed(&mut self, now: Ticks) {
        let mut env = Env {
            now,
            sched: &mut self.sched,
            mem: &mut self.mem,
            board: &mut self.board,
            warn: &mut self.warn,
            clocks: self.params.clocks,
        };
        for g in &mut self.gpio {
            g.poll(&mut env);
        }
        self.board_next = self.board.next_event(now);
        self.irq_dirty = true;
        self.update_deadline();
    }

    /// Bytes from the host to UART `n`.
    pub fn uart_send(&mut self, now: Ticks, n: usize, data: &[u8]) {
        let mut env = Env {
            now,
            sched: &mut self.sched,
            mem: &mut self.mem,
            board: &mut self.board,
            warn: &mut self.warn,
            clocks: self.params.clocks,
        };
        self.uart[n].host_send(&mut env, data);
        self.update_deadline();
    }

    /// Writes an image byte (loader).
    pub fn load_byte(&mut self, addr: u32, b: u8) -> bool {
        self.mem.load_byte(addr, b)
    }

    /// Processes all due events at `now`.
    pub fn process_events(&mut self, now: Ticks) {
        self.deadline_now = now;
        while let Some((sl, t)) = self.sched.pop_due(now) {
            self.event(sl, t);
            self.irq_dirty = true;
        }
        if self.board_next <= now {
            self.inputs_changed(now);
        }
        self.update_deadline();
    }

    fn event(&mut self, sl: Slot, t: Ticks) {
        let clocks = self.params.clocks;
        let mut env = Env { now: t, sched: &mut self.sched, mem: &mut self.mem, board: &mut self.board, warn: &mut self.warn, clocks };
        let i = sl.0 as usize;
        if i < 6 {
            self.uart[i].tx_done(&mut env);
        } else if i < 12 {
            self.uart[i - 6].rx_done(&mut env);
        } else if i < 16 {
            self.timers[i - 12].event(&mut env, &self.counter);
        } else if sl == slot::SLOWCLK_TIMER {
            self.slowclk_timer.event(&mut env);
        } else if i == 17 || i == 18 {
            self.wdog[i - 17].event(&mut env, &self.counter);
            if self.wdog[i - 17].reset_request {
                self.reset_request = Some(if i == 17 { 1 << 1 } else { 1 << 2 });
            }
        } else if sl == slot::SLOWCLK_WDOG {
            self.slowclk_wdog.event(&mut env);
            if self.slowclk_wdog.reset_request {
                self.reset_request = Some(1 << 3);
            }
        } else if sl == slot::HDLCD {
            self.hdlcd.event(&mut env);
        } else if sl == slot::I2S {
            self.i2s.event(&mut env);
        } else if sl == slot::RTC {
            self.rtc.event(&mut env);
        } else if (23..31).contains(&i) {
            self.vsi[i - 23].event(&mut env);
        } else if sl == slot::NPU {
            self.npu.event(&mut env);
        } else if sl == slot::CAM {
            self.cam.event(&mut env);
        }
    }

    fn irq_line(&self, irq: usize) -> bool {
        match irq {
            irq::NONSEC_WATCHDOG => self.wdog[0].irq(),
            irq::SLOWCLK_TIMER => self.slowclk_timer.irq(),
            irq::TIMER0..=irq::TIMER2 => self.timers[irq - irq::TIMER0].irq(self.counter_now()),
            irq::TIMER3 => self.timers[3].irq(self.counter_now()),
            irq::NPU0 => self.npu.irq(),
            irq::I2S => self.i2s.irq(),
            irq::RTC => self.rtc.irq(),
            irq::ISP => self.cam.irq(),
            irq::HDLCD => self.hdlcd.irq(),
            irq::UART_OVF => self.uart.iter().any(|u| u.irq_ovf()),
            57 | 58 => self.dma.irq_channel(irq - 57),
            irq::DMA_SEC_COMBINED | irq::DMA_NONSEC_COMBINED => (0..dma::NUM_CHANNELS).any(|i| self.dma.irq_channel(i)),
            69..=72 => self.gpio[irq - 69].irq(),
            224..=231 => self.vsi[irq - 224].irq(),
            _ => {
                for n in 0..6 {
                    if irq == irq::UART_RX[n] {
                        return self.uart[n].irq_rx();
                    }
                    if irq == irq::UART_TX[n] {
                        return self.uart[n].irq_tx();
                    }
                    if irq == irq::UART_COMBINED[n] {
                        return self.uart[n].irq_rx() || self.uart[n].irq_tx();
                    }
                }
                false
            }
        }
    }

    fn counter_now(&self) -> u64 {
        self.counter.value(self.deadline_now)
    }

    fn compute_irq_lines(&mut self, now: Ticks) -> IrqLines {
        self.deadline_now = now;
        let mut lines = [0u64; 8];
        const ALL: &[usize] = &[
            1, 2, 3, 4, 5, 16, 24, 25, 27, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 50, 57, 58, 69, 70, 71, 72, 125,
            126, 127, 128, 132, 133, 224, 225, 226, 227, 228, 229, 230, 231,
        ];
        for &i in ALL {
            if self.irq_line(i) {
                lines[i / 64] |= 1 << (i % 64);
            }
        }
        lines
    }

    /// Dispatches a peripheral register access.
    fn periph_read(&mut self, now: Ticks, addr: u32, size: u8, debug: bool) -> Result<u32, BusError> {
        let r = self.periph_read_inner(now, addr, size, debug);
        if self.trace && !debug {
            log::debug!("[{:.6}] rd {addr:#010x}/{size} -> {:?}", cs320_core::time::to_secs(now), r.as_ref().map(|v| format!("{v:#x}")));
        }
        r
    }

    fn periph_read_inner(&mut self, now: Ticks, addr: u32, size: u8, debug: bool) -> Result<u32, BusError> {
        let (p, off) = periph_of(addr).ok_or(BusError)?;
        if off >= 0xFB8 && !matches!(p, Periph::Cam | Periph::Vsi(_) | Periph::Rtc) {
            // identification registers as the FVP reports them
            if let Some(v) = fvp::word(addr) {
                return Ok(crate::env::narrow(v, addr, size));
            }
        }
        let clocks = self.params.clocks;
        let mut env = Env { now, sched: &mut self.sched, mem: &mut self.mem, board: &mut self.board, warn: &mut self.warn, clocks };
        let v = match p {
            Periph::Dma => self.dma.read(&mut env, off, size),
            Periph::Npu => self.npu.read(&mut env, off, size),
            Periph::CpuPwrCtrl => self.block(0x4001_2000, "CPU0 PWRCTRL").read(off, size),
            Periph::CpuIdentity => {
                if off & !3 == 0 {
                    0
                } else {
                    self.block(0x4001_F000, "CPU0 IDENTITY").read(off, size)
                }
            }
            Periph::CpuSecCtrl => self.block(0x5001_1000, "CPU0 SECCTRL").read(off, size),
            Periph::Nsacfg => self.nsacfg.read_fvp(off, size),
            Periph::Sacfg => self.sacfg.read_fvp(off, size),
            Periph::Mpc(i) => self.mpc[i.min(4)].read_fvp(off, size),
            Periph::Block(base) => self.block(base, block_name(base)).read_at(base, off, size),
            Periph::Gpio(i) => self.gpio[i].read(&mut env, off, size),
            Periph::Hdlcd => self.hdlcd.read(off, size),
            Periph::SysTimer(i) => self.timers[i].read(&mut env, off, size, &self.counter),
            Periph::SysInfo => self.sysinfo.read_fvp(off, size),
            Periph::SysCtrl => self.sysctrl.read(off, size),
            Periph::SlowClkWdog => self.slowclk_wdog.read(&mut env, off, size),
            Periph::SlowClkTimer => self.slowclk_timer.read(&mut env, off, size),
            Periph::Wdog(i) => self.wdog[i].read(&mut env, off, size),
            Periph::CounterCtrl => self.counter.read_ctrl(&mut env, off, size),
            Periph::CounterRead => self.counter.read_frame(&mut env, off, size),
            Periph::I2c(i) => self.i2c[i].read(off, size),
            Periph::Spi(i) => self.spi[i].read(off, size, debug),
            Periph::Scc => self.scc.read(off, size),
            Periph::I2s => self.i2s.read(&mut env, off, size, debug),
            Periph::FpgaIo => self.fpgaio.read(&mut env, off, size),
            Periph::Uart(i) => self.uart[i].read(&mut env, off, size, debug),
            Periph::Rtc => self.rtc.read(&mut env, off, size),
            Periph::Isp => {
                self.report(addr, "ISP (Mali-C55)");
                env::narrow(self.unimpl.get(&(addr & !3 & !0x1000_0000)).copied().unwrap_or(0), addr, size)
            }
            Periph::Cam => self.cam.read(off, size),
            Periph::Vio => self.vio.read(&mut env, off, size),
            Periph::Vsi(i) => self.vsi[i].read(&mut env, off, size),
            Periph::Unmodelled(name) => {
                self.report(addr, name);
                env::narrow(self.unimpl.get(&(addr & !3 & !0x1000_0000)).copied().unwrap_or(0), addr, size)
            }
        };
        Ok(v)
    }

    fn periph_write(&mut self, now: Ticks, addr: u32, size: u8, v: u32, debug: bool) -> Result<(), BusError> {
        if self.trace && !debug {
            log::debug!("[{:.6}] wr {addr:#010x}/{size} <- {v:#x}", cs320_core::time::to_secs(now));
        }
        self.periph_write_inner(now, addr, size, v, debug)
    }

    fn periph_write_inner(&mut self, now: Ticks, addr: u32, size: u8, v: u32, _debug: bool) -> Result<(), BusError> {
        let (p, off) = periph_of(addr).ok_or(BusError)?;
        let clocks = self.params.clocks;
        let mut env = Env { now, sched: &mut self.sched, mem: &mut self.mem, board: &mut self.board, warn: &mut self.warn, clocks };
        match p {
            Periph::Dma => self.dma.write(&mut env, off, size, v),
            Periph::Npu => self.npu.write(&mut env, off, size, v),
            Periph::CpuPwrCtrl => self.block(0x4001_2000, "CPU0 PWRCTRL").write(off, size, v),
            Periph::CpuIdentity => {}
            Periph::CpuSecCtrl => self.block(0x5001_1000, "CPU0 SECCTRL").write(off, size, v),
            Periph::Nsacfg => self.nsacfg.write(off, size, v),
            Periph::Sacfg => {
                self.sacfg.write(off, size, v);
                // SECPPCINTCLR / SECMSCINTCLR / BRGINTCLR clear the status registers
                match off & !3 {
                    0x024 => self.sacfg.set(0x020, self.sacfg.get(0x020) & !v),
                    0x034 => self.sacfg.set(0x030, self.sacfg.get(0x030) & !v),
                    0x044 => self.sacfg.set(0x040, self.sacfg.get(0x040) & !v),
                    _ => {}
                }
            }
            Periph::Mpc(i) => {
                let m = &mut self.mpc[i.min(4)];
                m.write(off, size, v);
                if off & !3 == 0x01C {
                    // INT_CLEAR
                    m.set(0x018, 0);
                }
            }
            Periph::Block(base) => self.block(base, block_name(base)).write(off, size, v),
            Periph::Gpio(i) => self.gpio[i].write(&mut env, off, size, v),
            Periph::Hdlcd => self.hdlcd.write(&mut env, off, size, v),
            Periph::SysTimer(i) => self.timers[i].write(&mut env, off, size, v, &self.counter),
            Periph::SysInfo => {}
            Periph::SysCtrl => {
                self.sysctrl.write(off, size, v);
                if self.sysctrl.reset_request {
                    self.reset_request = Some(1 << 5);
                }
            }
            Periph::SlowClkWdog => self.slowclk_wdog.write(&mut env, off, size, v),
            Periph::SlowClkTimer => self.slowclk_timer.write(&mut env, off, size, v),
            Periph::Wdog(i) => self.wdog[i].write(&mut env, off, size, v, &self.counter),
            Periph::CounterCtrl => {
                if self.counter.write_ctrl(&mut env, off, size, v) {
                    for t in &mut self.timers {
                        t.reschedule(&mut env, &self.counter);
                    }
                }
            }
            Periph::CounterRead => {}
            Periph::I2c(i) => self.i2c[i].write(&mut env, off, size, v),
            Periph::Spi(i) => self.spi[i].write(off, size, v),
            Periph::Scc => self.scc.write(off, size, v),
            Periph::I2s => self.i2s.write(&mut env, off, size, v),
            Periph::FpgaIo => self.fpgaio.write(&mut env, off, size, v),
            Periph::Uart(i) => self.uart[i].write(&mut env, off, size, v),
            Periph::Rtc => self.rtc.write(&mut env, off, size, v),
            Periph::Isp | Periph::Unmodelled(_) => {
                let name = if let Periph::Unmodelled(n) = p { n } else { "ISP (Mali-C55)" };
                self.report(addr, name);
                let key = addr & !3 & !0x1000_0000;
                let old = self.unimpl.get(&key).copied().unwrap_or(0);
                self.unimpl.insert(key, env::widen(old, addr, size, v));
            }
            Periph::Cam => self.cam.write(&mut env, off, size, v),
            Periph::Vio => self.vio.write(&mut env, off, size, v),
            Periph::Vsi(i) => self.vsi[i].write(&mut env, off, size, v),
        }
        Ok(())
    }

    fn block(&mut self, base: u32, name: &'static str) -> &mut RegBlock {
        self.blocks.entry(base).or_insert_with(|| RegBlock::new(name, sysctrl::GENERIC_PIDR, &[]).with_base(base))
    }

    fn report(&mut self, addr: u32, name: &str) {
        let key = addr & 0xFFFF_F000 & !0x1000_0000;
        if self.reported.insert(key) {
            self.warn.warn(
                self.deadline_now,
                format!("{name} at {:#010x} is not modelled: registers are stored, reads return the stored value", addr & 0xFFFF_F000),
            );
        }
    }
}

impl<B: BoardIo> Bus for System<B> {
    fn read(&mut self, now: Ticks, addr: u32, size: u8, _ns: bool, debug: bool) -> Result<u32, BusError> {
        if let Some(v) = self.mem.read(addr, size) {
            return Ok(v);
        }
        if addr >> 28 == 0x4 || addr >> 28 == 0x5 {
            self.deadline_now = now;
            let r = self.periph_read(now, addr, size, debug);
            if !debug {
                self.irq_dirty = true;
                self.update_deadline();
            }
            return r;
        }
        Err(BusError)
    }

    fn write(&mut self, now: Ticks, addr: u32, size: u8, value: u32, _ns: bool, debug: bool) -> Result<(), BusError> {
        if self.mem.write(addr, size, value) {
            if debug {
                self.mem.code_gen += 1;
            }
            return Ok(());
        }
        if addr >> 28 == 0x4 || addr >> 28 == 0x5 {
            self.deadline_now = now;
            let r = self.periph_write(now, addr, size, value, debug);
            self.irq_dirty = true;
            self.update_deadline();
            return r;
        }
        Err(BusError)
    }

    fn fetch16(&mut self, addr: u32, _ns: bool) -> Result<u16, BusError> {
        self.mem.read(addr, 2).map(|v| v as u16).ok_or(BusError)
    }

    fn deadline(&self) -> Ticks {
        self.deadline
    }

    fn take_irq_lines(&mut self) -> Option<IrqLines> {
        if !self.irq_dirty {
            return None;
        }
        self.irq_dirty = false;
        let now = self.deadline_now;
        let lines = self.compute_irq_lines(now);
        if lines != self.irq_lines {
            self.irq_lines = lines;
            Some(lines)
        } else {
            None
        }
    }

    fn idau(&self, addr: u32) -> Idau {
        if (0xE000_0000..0xE010_0000).contains(&addr) {
            Idau::Exempt
        } else if addr & 0x1000_0000 == 0 {
            // NSC regions of the code and RAM: NSCCFG of the SACFG
            let nsccfg = self.sacfg.get(0x014);
            if (nsccfg & 1 != 0 && addr < 0x2000_0000) || (nsccfg & 2 != 0 && (0x2000_0000..0x4000_0000).contains(&addr)) {
                Idau::NonSecureCallable
            } else {
                Idau::NonSecure
            }
        } else {
            Idau::None
        }
    }

    fn reset_vector_base(&self) -> u32 {
        self.sysctrl.initsvtor()
    }

    fn code_cacheable(&self, addr: u32) -> bool {
        self.mem.is_memory(addr)
    }

    fn code_generation(&self) -> u64 {
        self.mem.code_gen
    }
}
