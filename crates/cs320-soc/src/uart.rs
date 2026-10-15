//! CMSDK APB UART (`uart_cmsdk_reg_map.h`): one-byte transmit and receive buffers, a baud
//! divider, TX/RX interrupts and the overflow flags.

use crate::env::{narrow, slot, widen, Env};
use cs320_core::time::FracPeriod;
use cs320_core::Ticks;
use std::collections::VecDeque;

pub const CTRL_TX_EN: u32 = 1 << 0;
pub const CTRL_RX_EN: u32 = 1 << 1;
pub const CTRL_TX_INTR_EN: u32 = 1 << 2;
pub const CTRL_RX_INTR_EN: u32 = 1 << 3;
pub const CTRL_TX_OVF_INTR_EN: u32 = 1 << 4;
pub const CTRL_RX_OVF_INTR_EN: u32 = 1 << 5;
pub const CTRL_HS_TEST: u32 = 1 << 6;

pub const STATE_TX_BF: u32 = 1 << 0;
pub const STATE_RX_BF: u32 = 1 << 1;
pub const STATE_TX_BO: u32 = 1 << 2;
pub const STATE_RX_BO: u32 = 1 << 3;

pub const INT_TX: u32 = 1 << 0;
pub const INT_RX: u32 = 1 << 1;
pub const INT_TX_OVF: u32 = 1 << 2;
pub const INT_RX_OVF: u32 = 1 << 3;

pub struct CmsdkUart {
    pub n: usize,
    pub ctrl: u32,
    pub state: u32,
    pub intstatus: u32,
    pub bauddiv: u32,
    tx_data: u8,
    rx_data: u8,
    /// Bytes from the host waiting for the receiver.
    pub host_rx: VecDeque<u8>,
    rx_busy: bool,
    /// Loop-back for the high-speed test mode.
    loopback: VecDeque<u8>,
}

impl CmsdkUart {
    pub fn new(n: usize) -> Self {
        CmsdkUart { n, ctrl: 0, state: 0, intstatus: 0, bauddiv: 0, tx_data: 0, rx_data: 0, host_rx: VecDeque::new(), rx_busy: false, loopback: VecDeque::new() }
    }

    pub fn reset(&mut self) {
        self.ctrl = 0;
        self.state = 0;
        self.intstatus = 0;
        self.bauddiv = 0;
        self.rx_busy = false;
        self.loopback.clear();
    }

    /// Time of one 10-bit frame.
    fn frame_time(&self, env: &Env) -> Ticks {
        let div = self.bauddiv.max(16) as u128;
        FracPeriod::new(cs320_core::time::TICKS_PER_SEC as u128 * div * 10, env.clocks.periph_hz as u128).after(0, 1)
    }

    pub fn irq_tx(&self) -> bool {
        self.intstatus & INT_TX != 0
    }

    pub fn irq_rx(&self) -> bool {
        self.intstatus & INT_RX != 0
    }

    pub fn irq_ovf(&self) -> bool {
        self.intstatus & (INT_TX_OVF | INT_RX_OVF) != 0
    }

    pub fn read(&mut self, _env: &mut Env, off: u32, size: u8, debug: bool) -> u32 {
        let v = match off & !3 {
            0x000 => {
                let d = self.rx_data as u32;
                if !debug {
                    self.state &= !STATE_RX_BF;
                }
                d
            }
            0x004 => self.state,
            0x008 => self.ctrl,
            0x00C => self.intstatus,
            0x010 => self.bauddiv,
            0xFD0 => 0x04,
            0xFE0 => 0x21,
            0xFE4 => 0xB8,
            0xFE8 => 0x1B,
            0xFF0 => 0x0D,
            0xFF4 => 0xF0,
            0xFF8 => 0x05,
            0xFFC => 0xB1,
            _ => 0,
        };
        narrow(v, off, size)
    }

    pub fn write(&mut self, env: &mut Env, off: u32, size: u8, v: u32) {
        match off & !3 {
            0x000 => {
                let b = (v & 0xFF) as u8;
                if self.ctrl & CTRL_TX_EN == 0 {
                    return;
                }
                if self.state & STATE_TX_BF != 0 {
                    self.state |= STATE_TX_BO;
                    if self.ctrl & CTRL_TX_OVF_INTR_EN != 0 {
                        self.intstatus |= INT_TX_OVF;
                    }
                    return;
                }
                self.tx_data = b;
                self.state |= STATE_TX_BF;
                let t = self.frame_time(env);
                env.schedule(slot::UART_TX[self.n], env.now + t);
            }
            0x004 => {
                // writing 1 clears the overrun flags
                self.state &= !(v & (STATE_TX_BO | STATE_RX_BO));
            }
            0x008 => {
                self.ctrl = widen(self.ctrl, off, size, v) & 0x7F;
                if self.ctrl & CTRL_RX_EN != 0 {
                    self.kick_rx(env);
                }
            }
            0x00C => self.intstatus &= !v,
            0x010 => self.bauddiv = widen(self.bauddiv, off, size, v) & 0xF_FFFF,
            _ => {}
        }
    }

    /// Bytes from the host.
    pub fn host_send(&mut self, env: &mut Env, data: &[u8]) {
        self.host_rx.extend(data);
        self.kick_rx(env);
    }

    fn kick_rx(&mut self, env: &mut Env) {
        if self.rx_busy || self.ctrl & CTRL_RX_EN == 0 || (self.host_rx.is_empty() && self.loopback.is_empty()) {
            return;
        }
        self.rx_busy = true;
        let t = self.frame_time(env);
        env.schedule(slot::UART_RX[self.n], env.now + t);
    }

    /// The transmit frame completed.
    pub fn tx_done(&mut self, env: &mut Env) {
        self.state &= !STATE_TX_BF;
        if self.ctrl & CTRL_TX_INTR_EN != 0 {
            self.intstatus |= INT_TX;
        }
        if self.ctrl & CTRL_HS_TEST != 0 {
            self.loopback.push_back(self.tx_data);
            self.kick_rx(env);
        } else {
            env.board.uart_tx(env.now, self.n, self.tx_data);
        }
    }

    /// A receive frame completed.
    pub fn rx_done(&mut self, env: &mut Env) {
        self.rx_busy = false;
        let b = match self.loopback.pop_front().or_else(|| self.host_rx.pop_front()) {
            Some(b) => b,
            None => return,
        };
        if self.ctrl & CTRL_RX_EN == 0 {
            return;
        }
        if self.state & STATE_RX_BF != 0 {
            self.state |= STATE_RX_BO;
            if self.ctrl & CTRL_RX_OVF_INTR_EN != 0 {
                self.intstatus |= INT_RX_OVF;
            }
        } else {
            self.rx_data = b;
            self.state |= STATE_RX_BF;
            if self.ctrl & CTRL_RX_INTR_EN != 0 {
                self.intstatus |= INT_RX;
            }
        }
        self.kick_rx(env);
    }
}
