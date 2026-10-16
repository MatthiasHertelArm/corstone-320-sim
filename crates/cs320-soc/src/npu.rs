//! Ethos-U85 NPU at 0x4000_4000 (Secure alias 0x5000_4000): the functional model of the
//! `cs320-npu` crate behind the APB register interface. A command stream runs to completion
//! when the driver writes CMD.transition_to_running_state; the completion (STATUS, QREAD, the PMU
//! and the IRQ) is delivered after the estimated NPU cycle count, during which STATUS reads as
//! running, as on the hardware. The NPU is clocked by the CPU clock.

use crate::env::{narrow, slot, widen, Env};
use crate::mem::Memory;
use cs320_core::time::ticks_per_hz;
use cs320_npu::{Config, Npu, NpuBus};

pub struct EthosU {
    pub core: Npu,
}

/// The memory map as the NPU's AXI ports see it (the same addresses as the CPU, 32-bit).
struct MemBus<'a>(&'a mut Memory);

impl NpuBus for MemBus<'_> {
    fn read(&mut self, addr: u64, buf: &mut [u8]) -> bool {
        if addr >> 32 != 0 || (addr as u32).checked_add(buf.len() as u32).is_none() {
            buf.fill(0);
            return false;
        }
        self.0.read_bytes(addr as u32, buf)
    }

    fn write(&mut self, addr: u64, data: &[u8]) -> bool {
        if addr >> 32 != 0 || (addr as u32).checked_add(data.len() as u32).is_none() {
            return false;
        }
        self.0.write_bytes(addr as u32, data)
    }
}

impl EthosU {
    pub fn new(num_macs: u32) -> Self {
        EthosU { core: Npu::new(Config::new(num_macs)) }
    }

    pub fn reset(&mut self) {
        self.core.hard_reset();
    }

    pub fn irq(&self) -> bool {
        self.core.irq()
    }

    pub fn read(&mut self, _env: &mut Env, off: u32, size: u8) -> u32 {
        narrow(self.core.read_reg(off), off, size)
    }

    pub fn write(&mut self, env: &mut Env, off: u32, size: u8, v: u32) {
        let word = widen(self.core.read_reg(off), off, size, v);
        if self.core.write_reg(off, word) {
            self.core.trace = log::log_enabled!(target: "cs320_npu", log::Level::Trace);
            let c = self.core.start(&mut MemBus(env.mem));
            for w in self.core.take_warnings() {
                env.warn.warn(env.now, format!("Ethos-U85: {w}"));
            }
            let at = env.now + c.cycles * ticks_per_hz(env.clocks.cpu_hz as f64);
            env.schedule(slot::NPU, at);
        }
    }

    /// The command stream started earlier completes: STATUS, QREAD, PMU and the IRQ.
    pub fn event(&mut self, _env: &mut Env) {
        self.core.complete();
    }
}
