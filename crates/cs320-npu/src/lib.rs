//! Functional model of the Arm Ethos-U85 NPU (arch 2.0, Arm Ethos-U85 TRM 102685 issue 05).
//!
//! The model executes command streams as written by Vela for the `ethos-u85-<macs>` accelerator
//! configurations: the APB register block of chapter 3 of the TRM, the cmd0 and cmd1 command set,
//! the standard and fast weight decoders, the scale and bias stream, and the CONV, DEPTHWISE,
//! POOL, ELEMENTWISE, RESIZE, DMA and BRANCH operations with their chaining buffers, lookup
//! tables and accumulator storage. The arithmetic follows the TRM pseudocode bit for bit; the
//! execution order does not: every operation reads its inputs into dense buffers, computes the
//! whole stripe at host speed and writes the result back in one go, which gives the same memory
//! contents as the hardware once an operation is complete.
//!
//! Timing is not modelled; [`Npu::start`] returns an estimate of the NPU cycles the stream would
//! take so that the platform can raise the completion interrupt after a plausible delay.

#![forbid(unsafe_code)]

pub mod fm;
mod ops;
pub mod stream;

use fm::{FmDesc, Storage};

/// Memory seen by the NPU through its AXI ports (40-bit addresses).
pub trait NpuBus {
    /// Reads `buf.len()` bytes; returns false when any byte is outside memory (those read as 0).
    fn read(&mut self, addr: u64, buf: &mut [u8]) -> bool;
    /// Writes `data`; returns false when any byte is outside memory.
    fn write(&mut self, addr: u64, data: &[u8]) -> bool;
}

impl NpuBus for Vec<u8> {
    fn read(&mut self, addr: u64, buf: &mut [u8]) -> bool {
        let a = addr as usize;
        if addr as usize + buf.len() <= self.len() {
            buf.copy_from_slice(&self[a..a + buf.len()]);
            true
        } else {
            buf.fill(0);
            false
        }
    }

    fn write(&mut self, addr: u64, data: &[u8]) -> bool {
        let a = addr as usize;
        if a + data.len() <= self.len() {
            self[a..a + data.len()].copy_from_slice(data);
            true
        } else {
            false
        }
    }
}

// Register offsets (ethosu85_interface.h NPU_REG_*).
pub const REG_ID: u32 = 0x000;
pub const REG_STATUS: u32 = 0x004;
pub const REG_CMD: u32 = 0x008;
pub const REG_RESET: u32 = 0x00C;
pub const REG_QBASE: u32 = 0x010;
pub const REG_QBASE_HI: u32 = 0x014;
pub const REG_QREAD: u32 = 0x018;
pub const REG_QCONFIG: u32 = 0x01C;
pub const REG_QSIZE: u32 = 0x020;
pub const REG_PROT: u32 = 0x024;
pub const REG_CONFIG: u32 = 0x028;
pub const REG_COND_STATUS: u32 = 0x030;
pub const REG_POWER_CTRL: u32 = 0x038;
pub const REG_REGIONCFG: u32 = 0x03C;
pub const REG_MEM_ATTR: u32 = 0x040;
pub const REG_AXI_SRAM: u32 = 0x050;
pub const REG_AXI_EXT: u32 = 0x054;
pub const REG_BASEP: u32 = 0x080;
pub const REG_PMCR: u32 = 0x1180;
pub const REG_PMCNTENSET: u32 = 0x1184;
pub const REG_PMCNTENCLR: u32 = 0x1188;
pub const REG_PMOVSSET: u32 = 0x118C;
pub const REG_PMOVSCLR: u32 = 0x1190;
pub const REG_PMINTSET: u32 = 0x1194;
pub const REG_PMINTCLR: u32 = 0x1198;
pub const REG_PMCCNTR: u32 = 0x11A0;
pub const REG_PMCCNTR_HI: u32 = 0x11A4;
pub const REG_PMCCNTR_CFG: u32 = 0x11A8;
pub const REG_PMCAXI_CHAN: u32 = 0x11AC;
pub const REG_PMEVCNTR: u32 = 0x1300;
pub const REG_PMEVTYPER: u32 = 0x1380;

// STATUS bits
pub const STATUS_STATE: u32 = 1 << 0;
pub const STATUS_IRQ_RAISED: u32 = 1 << 1;
pub const STATUS_BUS_STATUS: u32 = 1 << 2;
pub const STATUS_RESET_STATUS: u32 = 1 << 3;
pub const STATUS_CMD_PARSE_ERROR: u32 = 1 << 4;
pub const STATUS_CMD_END_REACHED: u32 = 1 << 5;
pub const STATUS_PMU_IRQ_RAISED: u32 = 1 << 6;
pub const STATUS_BRANCH_FAULT: u32 = 1 << 9;

/// ID register: architecture 2.0.0, product major 7, r0p0 status 1 (the Corstone-320 FVP).
pub const ID_VALUE: u32 = 0x2000_7001;

pub const LUT_SIZE: usize = 2048;

/// Size of the configuration: number of MACs per clock cycle (128 … 2048).
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub macs: u32,
}

impl Config {
    pub fn new(macs: u32) -> Self {
        Config { macs: macs.clamp(128, 2048).next_power_of_two() }
    }

    /// Number of standard weight decoders (TRM table 3-90).
    pub fn num_wd(&self) -> usize {
        match self.macs {
            128 | 256 => 1,
            512 => 2,
            _ => 4,
        }
    }

    /// Zero weights inserted after every depthwise sub-kernel (TRM table 3-90).
    pub fn dw_pad(&self, ifm16: bool) -> usize {
        match self.macs {
            128 | 256 => {
                if ifm16 {
                    2
                } else {
                    0
                }
            }
            512 => 2,
            _ => 6,
        }
    }

    /// External AXI ports: one for 128 and 256 MACs, two above (the U85 TRM's configurations).
    pub fn num_axi_ext(&self) -> u32 {
        if self.macs > 256 {
            2
        } else {
            1
        }
    }

    /// CONFIG register (TRM field layout, `config_r`): product 2 (U85), command stream version 1,
    /// two SRAM AXI ports, the external AXI ports (log2 in bit 10), the weight decoders (bits 12-13).
    pub fn word(&self) -> u32 {
        let macs_log2 = 31 - self.macs.leading_zeros();
        let wd_log2 = 31 - (self.num_wd() as u32).leading_zeros();
        let axi_ext_log2 = 31 - self.num_axi_ext().leading_zeros();
        (2 << 28) | (wd_log2 << 12) | (axi_ext_log2 << 10) | (1 << 8) | (1 << 4) | macs_log2
    }
}

/// Performance monitor registers.
#[derive(Default, Clone, Debug)]
pub struct Pmu {
    pub pmcr: u32,
    pub cnten: u32,
    pub ovs: u32,
    pub inten: u32,
    pub ccntr: u64,
    pub ccntr_cfg: u32,
    pub axi_chan: u32,
    pub evcntr: [u32; 8],
    pub evtyper: [u32; 8],
    pub mask_enabled: bool,
}

const PMU_EVENT_CYCLE: u32 = 17;
const PMU_EVENT_NPU_ACTIVE: u32 = 35;
const PMU_EVENT_MAC_ACTIVE: u32 = 48;

impl Pmu {
    fn reset(&mut self) {
        *self = Pmu { mask_enabled: true, ..Pmu::default() };
    }

    /// Accounts the cycles of a completed command stream.
    fn count(&mut self, cycles: u64, mac_cycles: u64) {
        if self.pmcr & 1 == 0 {
            return;
        }
        if self.pmcr & (1 << 3) != 0 && !self.mask_enabled {
            return;
        }
        if self.cnten & (1 << 31) != 0 {
            self.ccntr = self.ccntr.wrapping_add(cycles);
        }
        for i in 0..8 {
            if self.cnten & (1 << i) == 0 {
                continue;
            }
            let add = match self.evtyper[i] & 0x3FF {
                PMU_EVENT_CYCLE | PMU_EVENT_NPU_ACTIVE => cycles,
                PMU_EVENT_MAC_ACTIVE => mac_cycles,
                _ => 0,
            };
            let (v, ovf) = self.evcntr[i].overflowing_add(add as u32);
            self.evcntr[i] = v;
            if ovf || add >> 32 != 0 {
                self.ovs |= 1 << i;
            }
        }
    }

    fn irq(&self) -> bool {
        self.ovs & self.inten != 0
    }
}

/// Outcome of a command stream, applied by [`Npu::complete`].
#[derive(Clone, Copy, Debug)]
pub struct Completion {
    pub status: u32,
    pub qread: u32,
    pub cycles: u64,
    pub mac_cycles: u64,
}

/// The command-stream state registers (NPU_SET_*), stored as their raw payloads.
#[derive(Clone)]
pub struct State {
    /// cmd0 payloads (bits [31:16] of the command word) by opcode
    pub r0: [u16; 400],
    /// cmd1 payloads (addr_hi/other bits of the header in [23:16] above the 32-bit payload)
    pub r1: [u64; 152],
}

impl Default for State {
    fn default() -> Self {
        State { r0: [0; 400], r1: [0; 152] }
    }
}

pub mod op0 {
    pub const STOP: u32 = 0;
    pub const IRQ: u32 = 1;
    pub const CONV: u32 = 2;
    pub const DEPTHWISE: u32 = 3;
    pub const POOL: u32 = 5;
    pub const ELEMENTWISE: u32 = 6;
    pub const RESIZE: u32 = 7;
    pub const DMA_START: u32 = 16;
    pub const DMA_WAIT: u32 = 17;
    pub const KERNEL_WAIT: u32 = 18;
    pub const PMU_MASK: u32 = 19;
    pub const IFM_PAD_TOP: u32 = 256;
    pub const IFM_PAD_LEFT: u32 = 257;
    pub const IFM_PAD_RIGHT: u32 = 258;
    pub const IFM_PAD_BOTTOM: u32 = 259;
    pub const IFM_DEPTH_M1: u32 = 260;
    pub const IFM_PRECISION: u32 = 261;
    pub const IFM_UPSCALE: u32 = 263;
    pub const IFM_BROADCAST: u32 = 264;
    pub const IFM_ZERO_POINT: u32 = 265;
    pub const IFM_WIDTH0_M1: u32 = 266;
    pub const IFM_HEIGHT0_M1: u32 = 267;
    pub const IFM_HEIGHT1_M1: u32 = 268;
    pub const IFM_REGION: u32 = 271;
    pub const OFM_WIDTH_M1: u32 = 273;
    pub const OFM_HEIGHT_M1: u32 = 274;
    pub const OFM_DEPTH_M1: u32 = 275;
    pub const OFM_PRECISION: u32 = 276;
    pub const OFM_BLK_WIDTH_M1: u32 = 277;
    pub const OFM_BLK_HEIGHT_M1: u32 = 278;
    pub const OFM_BLK_DEPTH_M1: u32 = 279;
    pub const OFM_ZERO_POINT: u32 = 280;
    pub const OFM_WIDTH0_M1: u32 = 282;
    pub const OFM_HEIGHT0_M1: u32 = 283;
    pub const OFM_HEIGHT1_M1: u32 = 284;
    pub const OFM_REGION: u32 = 287;
    pub const KERNEL_WIDTH_M1: u32 = 288;
    pub const KERNEL_HEIGHT_M1: u32 = 289;
    pub const KERNEL_STRIDE: u32 = 290;
    pub const ACC_FORMAT: u32 = 292;
    pub const ACTIVATION: u32 = 293;
    pub const ACTIVATION_MIN: u32 = 294;
    pub const ACTIVATION_MAX: u32 = 295;
    pub const WEIGHT_REGION: u32 = 296;
    pub const SCALE_REGION: u32 = 297;
    pub const RESIZE_X_SCALE_N_M1: u32 = 298;
    pub const RESIZE_Y_SCALE_N_M1: u32 = 299;
    pub const RESIZE_X_OFFSET: u32 = 300;
    pub const RESIZE_Y_OFFSET: u32 = 301;
    pub const WEIGHT_FORMAT: u32 = 302;
    pub const BLOCKDEP: u32 = 303;
    pub const DMA0_SRC_REGION: u32 = 304;
    pub const DMA0_DST_REGION: u32 = 305;
    pub const DMA0_SIZE0: u32 = 306;
    pub const DMA0_SIZE1: u32 = 307;
    pub const DMA0_IDX_REGION: u32 = 308;
    pub const IFM2_BROADCAST: u32 = 384;
    pub const IFM2_PRECISION: u32 = 389;
    pub const IFM2_ZERO_POINT: u32 = 393;
    pub const IFM2_WIDTH0_M1: u32 = 394;
    pub const IFM2_HEIGHT0_M1: u32 = 395;
    pub const IFM2_HEIGHT1_M1: u32 = 396;
    pub const IFM2_REGION: u32 = 399;
}

pub mod op1 {
    pub const IFM_BASE0: u32 = 0;
    pub const IFM_STRIDE_X: u32 = 4;
    pub const IFM_STRIDE_Y: u32 = 5;
    pub const IFM_STRIDE_C: u32 = 6;
    pub const OFM_BASE0: u32 = 16;
    pub const OFM_STRIDE_X: u32 = 20;
    pub const OFM_STRIDE_Y: u32 = 21;
    pub const OFM_STRIDE_C: u32 = 22;
    pub const WEIGHT_BASE: u32 = 32;
    pub const WEIGHT_LENGTH: u32 = 33;
    pub const SCALE_BASE: u32 = 34;
    pub const SCALE_LENGTH: u32 = 35;
    pub const OFM_SCALE: u32 = 36;
    pub const IFM_SCALE: u32 = 37;
    pub const IFM2_SCALE: u32 = 38;
    pub const OP_SCALAR: u32 = 39;
    pub const DMA0_SRC: u32 = 48;
    pub const DMA0_DST: u32 = 49;
    pub const DMA0_LEN: u32 = 50;
    pub const DMA0_SRC_STRIDE0: u32 = 51;
    pub const DMA0_SRC_STRIDE1: u32 = 52;
    pub const DMA0_DST_STRIDE0: u32 = 53;
    pub const DMA0_DST_STRIDE1: u32 = 54;
    pub const DMA0_IDX: u32 = 55;
    pub const DMA0_IDX_MAX: u32 = 56;
    pub const DMA0_IDX_SKIP1: u32 = 57;
    pub const IFM2_BASE0: u32 = 128;
    pub const IFM2_STRIDE_X: u32 = 132;
    pub const IFM2_STRIDE_Y: u32 = 133;
    pub const IFM2_STRIDE_C: u32 = 134;
    pub const WEIGHT1_BASE: u32 = 144;
    pub const WEIGHT1_LENGTH: u32 = 145;
    pub const RESIZE_X_STEP: u32 = 150;
    pub const RESIZE_Y_STEP: u32 = 151;
    pub const BRANCH: u32 = 256;
}

/// Precision, type, layout and storage of a feature map (NPU_SET_*_PRECISION).
#[derive(Clone, Copy, Debug)]
pub struct Precision {
    pub signed: bool,
    pub bits: u32,
    pub brick: bool,
    pub storage: Storage,
}

/// Scale register (NPU_SET_OFM_SCALE, IFM_SCALE, IFM2_SCALE).
#[derive(Clone, Copy, Debug)]
pub struct ScaleReg {
    pub scale: i32,
    pub shift: u32,
    pub dbl_rnd: u32,
    pub round_mode: u32,
}

impl State {
    #[inline]
    pub fn p0(&self, op: u32) -> u32 {
        self.r0[op as usize] as u32
    }

    #[inline]
    pub fn p1(&self, op: u32) -> u64 {
        self.r1[op as usize]
    }

    /// 40-bit address of a cmd1 register (addr_hi in header bits [23:16], addr_lo the payload).
    #[inline]
    pub fn addr(&self, op: u32) -> u64 {
        let v = self.r1[op as usize];
        ((v >> 32) & 0xFF) << 32 | (v & 0xFFFF_FFFF)
    }

    pub fn precision(&self, op: u32) -> Precision {
        let p = self.p0(op);
        let bits = 8 << ((p >> 2) & 3);
        // OFM_PRECISION keeps the precision in bits [18:17] of the command, IFM in [19:18]
        let bits = if op == op0::OFM_PRECISION { 8 << ((p >> 1) & 3) } else { bits };
        Precision { signed: p & 1 != 0, bits, brick: (p >> 6) & 3 == 1, storage: Storage::from_bits(p >> 14) }
    }

    pub fn scale_reg(&self, op: u32) -> ScaleReg {
        let v = self.r1[op as usize];
        let hi = (v >> 32) as u32; // header bits [31:16] >> 16
        let round_mode = if op == op1::OFM_SCALE { (hi >> 13) & 7 } else { (hi >> 13) & 1 };
        ScaleReg { scale: (v & 0x7FFF_FFFF) as i32, shift: hi & 0x3F, dbl_rnd: (hi >> 6) & 0x1F, round_mode }
    }

    pub fn zero_point(&self, op: u32, signed: bool) -> i64 {
        let z = self.p0(op);
        if signed {
            z as u16 as i16 as i64
        } else {
            z as i64
        }
    }

    pub fn fm(&self, which: u32) -> FmDesc {
        let (prec, zp, w0, h0, h1, reg, base, sx, sy, sc) = match which {
            0 => (
                op0::IFM_PRECISION,
                op0::IFM_ZERO_POINT,
                op0::IFM_WIDTH0_M1,
                op0::IFM_HEIGHT0_M1,
                op0::IFM_HEIGHT1_M1,
                op0::IFM_REGION,
                op1::IFM_BASE0,
                op1::IFM_STRIDE_X,
                op1::IFM_STRIDE_Y,
                op1::IFM_STRIDE_C,
            ),
            1 => (
                op0::OFM_PRECISION,
                op0::OFM_ZERO_POINT,
                op0::OFM_WIDTH0_M1,
                op0::OFM_HEIGHT0_M1,
                op0::OFM_HEIGHT1_M1,
                op0::OFM_REGION,
                op1::OFM_BASE0,
                op1::OFM_STRIDE_X,
                op1::OFM_STRIDE_Y,
                op1::OFM_STRIDE_C,
            ),
            _ => (
                op0::IFM2_PRECISION,
                op0::IFM2_ZERO_POINT,
                op0::IFM2_WIDTH0_M1,
                op0::IFM2_HEIGHT0_M1,
                op0::IFM2_HEIGHT1_M1,
                op0::IFM2_REGION,
                op1::IFM2_BASE0,
                op1::IFM2_STRIDE_X,
                op1::IFM2_STRIDE_Y,
                op1::IFM2_STRIDE_C,
            ),
        };
        let p = self.precision(prec);
        FmDesc {
            base: [self.addr(base), self.addr(base + 1), self.addr(base + 2), self.addr(base + 3)],
            stride_x: self.addr(sx),
            stride_y: self.addr(sy),
            stride_c: self.addr(sc),
            width0: self.p0(w0) as usize + 1,
            height: [self.p0(h0) as usize + 1, self.p0(h1) as usize + 1],
            storage: p.storage,
            esize: (p.bits / 8) as usize,
            signed: p.signed,
            brick: p.brick,
            region: (self.p0(reg) & 7) as usize,
            zero_point: self.zero_point(zp, p.signed),
        }
    }

    pub fn ofm_dims(&self) -> (usize, usize, usize) {
        (self.p0(op0::OFM_HEIGHT_M1) as usize + 1, self.p0(op0::OFM_WIDTH_M1) as usize + 1, self.p0(op0::OFM_DEPTH_M1) as usize + 1)
    }

    pub fn ofm_block(&self) -> (usize, usize, usize) {
        (
            self.p0(op0::OFM_BLK_HEIGHT_M1) as usize + 1,
            self.p0(op0::OFM_BLK_WIDTH_M1) as usize + 1,
            self.p0(op0::OFM_BLK_DEPTH_M1) as usize + 1,
        )
    }

    pub fn acc48(&self) -> bool {
        self.p0(op0::ACC_FORMAT) & 3 == 1
    }

    pub fn acc_input(&self) -> u32 {
        (self.p0(op0::ACC_FORMAT) >> 4) & 3
    }

    pub fn acc_output_disabled(&self) -> bool {
        self.p0(op0::ACC_FORMAT) & (1 << 6) != 0
    }

    pub fn microblock(&self) -> u32 {
        (self.p0(op0::ACC_FORMAT) >> 8) & 7
    }

    /// Kernel stride register: (stride_x, stride_y, part_kernel_first, dilation_x, dilation_y, decomposition 4x4)
    pub fn kernel_stride(&self) -> (usize, usize, bool, usize, usize, bool) {
        let k = self.p0(op0::KERNEL_STRIDE);
        let sx = (k & 1) + 2 * ((k >> 6) & 1) + 1;
        let sy = ((k >> 1) & 1) + 2 * ((k >> 9) & 1) + 1;
        (sx as usize, sy as usize, (k >> 2) & 1 != 0, ((k >> 3) & 1) as usize + 1, ((k >> 4) & 1) as usize + 1, (k >> 5) & 1 != 0)
    }

    pub fn scale_mode_per_channel(&self) -> bool {
        (self.p0(op0::OFM_PRECISION) >> 8) & 1 == 0
    }

    pub fn ofm_reverse(&self) -> u32 {
        (self.p0(op0::OFM_PRECISION) >> 9) & 3
    }

    pub fn ofm_transpose(&self) -> u32 {
        (self.p0(op0::OFM_PRECISION) >> 11) & 7
    }
}

/// The NPU: registers, state and the command stream interpreter.
pub struct Npu {
    pub cfg: Config,
    pub status: u32,
    cmd_q: u32,
    pub prot: u32,
    pub qbase: u64,
    pub qread: u32,
    pub qconfig: u32,
    pub qsize: u32,
    pub cond_status: u32,
    pub power_ctrl: u32,
    pub regioncfg: u32,
    pub mem_attr: [u32; 4],
    pub axi_sram: u32,
    pub axi_ext: u32,
    pub basep: [u64; 8],
    other: std::collections::HashMap<u32, u32>,
    pub pmu: Pmu,
    pending: Option<Completion>,
    pub st: State,
    pub lut: Vec<u8>,
    fifo: [Vec<i64>; 3],
    fifo_rd: [usize; 3],
    fifo_valid: [bool; 3],
    chain_len: u32,
    num_ifm: u32,
    acc: Vec<i64>,
    acc_dims: (usize, usize, usize),
    dma_outstanding: u32,
    /// log each command (`log::trace!`)
    pub trace: bool,
    cycles: u64,
    mac_cycles: u64,
    warnings: Vec<String>,
}

impl Npu {
    pub fn new(cfg: Config) -> Self {
        let mut n = Npu {
            cfg,
            status: 0,
            cmd_q: 0xC,
            prot: 0,
            qbase: 0,
            qread: 0,
            qconfig: 0,
            qsize: 0,
            cond_status: 0,
            power_ctrl: 0,
            regioncfg: 0,
            mem_attr: [0; 4],
            axi_sram: 0,
            axi_ext: 0,
            basep: [0; 8],
            other: Default::default(),
            pmu: Pmu::default(),
            pending: None,
            st: State::default(),
            lut: vec![0; LUT_SIZE],
            fifo: [Vec::new(), Vec::new(), Vec::new()],
            fifo_rd: [0; 3],
            fifo_valid: [false; 3],
            chain_len: 0,
            num_ifm: 0,
            acc: Vec::new(),
            acc_dims: (0, 0, 0),
            dma_outstanding: 0,
            trace: false,
            cycles: 0,
            mac_cycles: 0,
            warnings: Vec::new(),
        };
        n.hard_reset();
        n
    }

    /// Power-on reset (the Q-channel enables come up set, PROT at secure privileged).
    pub fn hard_reset(&mut self) {
        self.soft_reset(0);
        self.cmd_q = 0xC;
        self.lut.fill(0);
    }

    /// RESET register write: `prot` holds the requested pending_CPL/pending_CSL bits.
    pub fn soft_reset(&mut self, prot: u32) {
        self.status = 0;
        self.prot = prot & 3;
        self.qbase = 0;
        self.qread = 0;
        self.qconfig = 0;
        self.qsize = 0;
        self.cond_status = 0;
        self.power_ctrl = 0;
        self.regioncfg = 0;
        self.mem_attr = [0; 4];
        self.axi_sram = 0;
        self.axi_ext = 0;
        self.basep = [0; 8];
        self.other.clear();
        self.pmu.reset();
        self.pending = None;
        self.st = State::default();
        self.chain_len = 0;
        self.num_ifm = 0;
        self.dma_outstanding = 0;
        for f in &mut self.fifo {
            f.clear();
        }
        self.fifo_rd = [0; 3];
        self.fifo_valid = [false; 3];
    }

    /// Interrupt request line.
    pub fn irq(&self) -> bool {
        self.status & STATUS_IRQ_RAISED != 0 || self.status & STATUS_PMU_IRQ_RAISED != 0
    }

    pub fn running(&self) -> bool {
        self.status & STATUS_STATE != 0
    }

    /// Warnings about unpredictable command streams collected during the last run.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// 32-bit register read at a word-aligned offset.
    pub fn read_reg(&self, off: u32) -> u32 {
        let off = off & 0x1FFC;
        match off {
            REG_ID => ID_VALUE,
            REG_STATUS => self.status,
            // reads back the last write (the Q-channel enables and clear_irq), as the FVP does
            REG_CMD => self.cmd_q,
            REG_RESET => 0,
            REG_QBASE => self.qbase as u32,
            REG_QBASE_HI => (self.qbase >> 32) as u32 & 0xFF,
            REG_QREAD => self.qread,
            REG_QCONFIG => self.qconfig,
            REG_QSIZE => self.qsize,
            REG_PROT => self.prot,
            REG_CONFIG => self.cfg.word(),
            REG_COND_STATUS => self.cond_status,
            REG_POWER_CTRL => self.power_ctrl,
            REG_REGIONCFG => self.regioncfg,
            0x040..=0x04C => self.mem_attr[((off - REG_MEM_ATTR) / 4) as usize],
            REG_AXI_SRAM => self.axi_sram,
            REG_AXI_EXT => self.axi_ext,
            0x080..=0x0BC => {
                let i = ((off - REG_BASEP) / 8) as usize;
                if off & 4 == 0 {
                    self.basep[i] as u32
                } else {
                    (self.basep[i] >> 32) as u32 & 0xFF
                }
            }
            // the NPU_SET_* registers are readable at 0x800 + 4 * (opcode - 256) (cmd0) and
            // 0x900 + 8 * opcode (cmd1)
            0x800..=0x8FC => self.st.r0[256 + ((off - 0x800) / 4) as usize] as u32,
            0x900..=0xBFC => {
                let i = ((off - 0x900) / 8) as usize;
                if i < 152 {
                    if off & 4 == 0 {
                        self.st.r1[i] as u32
                    } else {
                        (self.st.r1[i] >> 32) as u32
                    }
                } else {
                    0
                }
            }
            REG_PMCR => self.pmu.pmcr | (8 << 11),
            REG_PMCNTENSET | REG_PMCNTENCLR => self.pmu.cnten,
            REG_PMOVSSET | REG_PMOVSCLR => self.pmu.ovs,
            REG_PMINTSET | REG_PMINTCLR => self.pmu.inten,
            REG_PMCCNTR => self.pmu.ccntr as u32,
            REG_PMCCNTR_HI => (self.pmu.ccntr >> 32) as u32,
            REG_PMCCNTR_CFG => self.pmu.ccntr_cfg,
            REG_PMCAXI_CHAN => self.pmu.axi_chan,
            0x1300..=0x131C => self.pmu.evcntr[((off - REG_PMEVCNTR) / 4) as usize],
            0x1380..=0x139C => self.pmu.evtyper[((off - REG_PMEVTYPER) / 4) as usize],
            0xFD0..=0xFFC => coresight_id(off),
            _ => self.other.get(&off).copied().unwrap_or(0),
        }
    }

    /// 32-bit register write. Returns true when the write starts a command stream: the caller
    /// then calls [`Npu::start`] with the memory and later [`Npu::complete`].
    pub fn write_reg(&mut self, off: u32, v: u32) -> bool {
        let off = off & 0x1FFC;
        match off {
            REG_CMD => {
                self.cmd_q = v & 0x1E;
                if v & 2 != 0 {
                    self.status &= !STATUS_IRQ_RAISED;
                }
                if v >> 16 != 0 {
                    self.status &= !((v >> 16) << 16);
                }
                if v & 1 != 0 && !self.running() {
                    return true;
                }
                if v & (1 << 4) != 0 {
                    // stop request: the stream completes at the current command
                }
                false
            }
            REG_RESET => {
                self.soft_reset(v);
                false
            }
            REG_QBASE => {
                self.qbase = (self.qbase & !0xFFFF_FFFF) | v as u64;
                false
            }
            REG_QBASE_HI => {
                self.qbase = (self.qbase & 0xFFFF_FFFF) | ((v & 0xFF) as u64) << 32;
                false
            }
            REG_QCONFIG => {
                self.qconfig = v & 3;
                false
            }
            REG_QSIZE => {
                self.qsize = v;
                false
            }
            REG_POWER_CTRL => {
                self.power_ctrl = v & 0x3F;
                false
            }
            REG_REGIONCFG => {
                self.regioncfg = v & 0xFFFF;
                false
            }
            0x040..=0x04C => {
                self.mem_attr[((off - REG_MEM_ATTR) / 4) as usize] = v;
                false
            }
            REG_AXI_SRAM => {
                self.axi_sram = v;
                false
            }
            REG_AXI_EXT => {
                self.axi_ext = v;
                false
            }
            0x080..=0x0BC => {
                let i = ((off - REG_BASEP) / 8) as usize;
                if off & 4 == 0 {
                    self.basep[i] = (self.basep[i] & !0xFFFF_FFFF) | v as u64;
                } else {
                    self.basep[i] = (self.basep[i] & 0xFFFF_FFFF) | ((v & 0xFF) as u64) << 32;
                }
                false
            }
            REG_PMCR => {
                self.pmu.pmcr = v & 0x9;
                if v & 2 != 0 {
                    self.pmu.evcntr = [0; 8];
                }
                if v & 4 != 0 {
                    self.pmu.ccntr = 0;
                }
                false
            }
            REG_PMCNTENSET => {
                self.pmu.cnten |= v & 0x8000_00FF;
                false
            }
            REG_PMCNTENCLR => {
                self.pmu.cnten &= !v;
                false
            }
            REG_PMOVSSET => {
                self.pmu.ovs |= v & 0x8000_00FF;
                false
            }
            REG_PMOVSCLR => {
                self.pmu.ovs &= !v;
                false
            }
            REG_PMINTSET => {
                self.pmu.inten |= v & 0x8000_00FF;
                false
            }
            REG_PMINTCLR => {
                self.pmu.inten &= !v;
                false
            }
            REG_PMCCNTR => {
                self.pmu.ccntr = (self.pmu.ccntr & !0xFFFF_FFFF) | v as u64;
                false
            }
            REG_PMCCNTR_HI => {
                self.pmu.ccntr = (self.pmu.ccntr & 0xFFFF_FFFF) | ((v & 0xFFFF) as u64) << 32;
                false
            }
            REG_PMCCNTR_CFG => {
                self.pmu.ccntr_cfg = v;
                false
            }
            REG_PMCAXI_CHAN => {
                self.pmu.axi_chan = v;
                false
            }
            0x1300..=0x131C => {
                self.pmu.evcntr[((off - REG_PMEVCNTR) / 4) as usize] = v;
                false
            }
            0x1380..=0x139C => {
                self.pmu.evtyper[((off - REG_PMEVTYPER) / 4) as usize] = v & 0x3FF;
                false
            }
            REG_ID | REG_STATUS | REG_QREAD | REG_PROT | REG_CONFIG | REG_COND_STATUS => false,
            _ => {
                self.other.insert(off, v);
                false
            }
        }
    }

    /// Executes the command stream at QBASE..QBASE+QSIZE from its start, synchronously. STATUS shows
    /// the NPU running until [`Npu::complete`] applies the returned outcome.
    pub fn start(&mut self, bus: &mut dyn NpuBus) -> Completion {
        self.status |= STATUS_STATE;
        self.cycles = 0;
        self.mac_cycles = 0;
        let mut status =
            self.status & !(STATUS_STATE | STATUS_CMD_PARSE_ERROR | STATUS_CMD_END_REACHED | STATUS_BRANCH_FAULT | STATUS_BUS_STATUS);
        let qread = match self.run(bus, &mut status) {
            Ok(q) => q,
            Err((q, e)) => {
                log::warn!("Ethos-U85: {e}");
                q
            }
        };
        let c = Completion { status, qread, cycles: self.cycles + 16, mac_cycles: self.mac_cycles };
        self.pending = Some(c);
        c
    }

    /// Finishes the stream started by [`Npu::start`]: STATUS, QREAD, the PMU and the IRQ.
    pub fn complete(&mut self) {
        if let Some(c) = self.pending.take() {
            self.status = c.status;
            self.qread = c.qread;
            self.pmu.count(c.cycles, c.mac_cycles);
            if self.pmu.irq() {
                self.status |= STATUS_PMU_IRQ_RAISED;
            }
        }
    }

    fn run(&mut self, bus: &mut dyn NpuBus, status: &mut u32) -> Result<u32, (u32, String)> {
        let qsize = self.qsize as usize & !3;
        let mut words = vec![0u8; qsize];
        if !bus.read(self.qbase, &mut words) {
            *status |= STATUS_BUS_STATUS | STATUS_IRQ_RAISED;
            return Err((0, format!("command stream at {:#x} ({} bytes) is outside memory", self.qbase, qsize)));
        }
        let words: Vec<u32> = words.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let mut pc = 0usize;
        self.cycles += 32;
        loop {
            if pc >= words.len() {
                // ran out of commands: stops with cmd_end_reached
                *status |= STATUS_CMD_END_REACHED | STATUS_IRQ_RAISED;
                return Ok((pc * 4) as u32);
            }
            let h = words[pc];
            let op = h & 0x3FF;
            let ctrl = (h >> 14) & 3;
            let p0 = h >> 16;
            self.cycles += 1;
            let r = match ctrl {
                0 => {
                    pc += 1;
                    if self.trace {
                        log::trace!("npu cmd0 @{:#06x}: op {op} p0 {p0:#06x}", (pc - 1) * 4);
                    }
                    self.exec_cmd0(bus, op, p0, status)
                }
                1 => {
                    if pc + 1 >= words.len() {
                        *status |= STATUS_CMD_PARSE_ERROR | STATUS_IRQ_RAISED;
                        return Err(((pc * 4) as u32, "cmd1 command truncated by QSIZE".into()));
                    }
                    let p1 = words[pc + 1];
                    pc += 2;
                    if self.trace {
                        log::trace!("npu cmd1 @{:#06x}: op {op} p0 {p0:#06x} p1 {p1:#010x}", (pc - 2) * 4);
                    }
                    self.exec_cmd1(bus, op, p0, p1, status)
                }
                _ => {
                    *status |= STATUS_CMD_PARSE_ERROR | STATUS_IRQ_RAISED;
                    return Err(((pc * 4) as u32, format!("reserved command control {ctrl} at {:#x}", pc * 4)));
                }
            };
            match r {
                Ok(Flow::Next) => {}
                Ok(Flow::Stop) => {
                    *status |= STATUS_CMD_END_REACHED | STATUS_IRQ_RAISED;
                    return Ok((pc * 4) as u32);
                }
                Ok(Flow::Branch(t)) => {
                    if t as usize >= qsize {
                        *status |= STATUS_BRANCH_FAULT | STATUS_IRQ_RAISED;
                        return Err(((pc * 4) as u32, format!("branch target {t:#x} beyond QSIZE")));
                    }
                    pc = t as usize / 4;
                }
                Err(e) => {
                    *status |= STATUS_CMD_PARSE_ERROR | STATUS_IRQ_RAISED;
                    return Err(((pc * 4) as u32, e));
                }
            }
        }
    }

    fn exec_cmd0(&mut self, bus: &mut dyn NpuBus, op: u32, p0: u32, status: &mut u32) -> Result<Flow, String> {
        match op {
            op0::STOP => {
                *status |= (p0 & 0xFFFF) << 16;
                self.chain_check_stop("NPU_OP_STOP");
                return Ok(Flow::Stop);
            }
            op0::IRQ => {
                *status |= (p0 & 0xFFFF) << 16 | STATUS_IRQ_RAISED;
                self.chain_check_stop("NPU_OP_IRQ");
            }
            op0::CONV => self.op_conv(bus, false, p0 & 1 != 0)?,
            op0::DEPTHWISE => self.op_conv(bus, true, false)?,
            op0::POOL => self.op_pool(bus, p0 & 7)?,
            op0::ELEMENTWISE => self.op_elementwise(bus, p0 & 0x3F)?,
            op0::RESIZE => self.op_resize(bus, p0 & 3)?,
            op0::DMA_START => self.op_dma(bus)?,
            op0::DMA_WAIT | op0::KERNEL_WAIT => {
                self.cycles += 4;
            }
            op0::PMU_MASK => {
                self.pmu.mask_enabled = p0 & 1 != 0;
            }
            256..=399 => {
                if op == op0::ACC_FORMAT && self.chain_len != 0 {
                    self.warn("NPU_SET_ACC_FORMAT inside a chain");
                }
                self.st.r0[op as usize] = p0 as u16;
            }
            _ => return Err(format!("unknown cmd0 opcode {op}")),
        }
        Ok(Flow::Next)
    }

    fn exec_cmd1(&mut self, _bus: &mut dyn NpuBus, op: u32, p0: u32, p1: u32, _status: &mut u32) -> Result<Flow, String> {
        match op {
            op1::BRANCH => {
                let cond = p0 & 1;
                if p1 & 3 != 0 {
                    return Err(format!("misaligned branch target {p1:#x}"));
                }
                if cond == 0 || self.cond_status & 1 != 0 {
                    return Ok(Flow::Branch(p1));
                }
            }
            0..=151 => {
                self.st.r1[op as usize] = ((p0 & 0xFFFF) as u64) << 32 | p1 as u64;
            }
            _ => return Err(format!("unknown cmd1 opcode {op}")),
        }
        Ok(Flow::Next)
    }

    fn chain_check_stop(&mut self, what: &str) {
        if self.chain_len != 0 {
            self.warn(&format!("{what} inside a chain"));
            self.chain_len = 0;
        }
    }

    pub(crate) fn warn(&mut self, msg: &str) {
        if self.warnings.len() < 64 && !self.warnings.iter().any(|w| w == msg) {
            log::warn!("Ethos-U85: {msg}");
            self.warnings.push(msg.to_string());
        }
    }

    /// Address of `offset` in `region`.
    #[inline]
    pub(crate) fn region_addr(&self, region: usize, offset: u64) -> u64 {
        self.basep[region & 7].wrapping_add(offset) & 0xFF_FFFF_FFFF
    }
}

enum Flow {
    Next,
    Stop,
    Branch(u32),
}

fn coresight_id(off: u32) -> u32 {
    match off {
        0xFE0 => 0x85,
        0xFE4 => 0xB0,
        0xFE8 => 0x0B,
        0xFEC => 0x00,
        0xFF0 => 0x0D,
        0xFF4 => 0xF0,
        0xFF8 => 0x05,
        0xFFC => 0xB1,
        _ => 0,
    }
}

#[cfg(test)]
mod config_tests {
    use super::Config;

    #[test]
    fn external_axi_ports_follow_the_mac_count() {
        // config_r: bit 10 is log2(num_axi_ext); the macs_per_cc field is log2(macs)
        assert_eq!((Config::new(256).word() >> 10) & 1, 0, "256 MACs: one external AXI port");
        assert_eq!((Config::new(128).word() >> 10) & 1, 0, "128 MACs: one external AXI port");
        assert_eq!((Config::new(1024).word() >> 10) & 1, 1, "1024 MACs: two external AXI ports");
        assert_eq!(Config::new(256).word() & 0xF, 8);
        assert_eq!(Config::new(1024).word() & 0xF, 10);
    }
}
