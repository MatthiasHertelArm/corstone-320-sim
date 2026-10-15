//! M-profile Vector Extension (Helium).
//!
//! Phase 1 of the plan: the instructions CMSIS-DSP, CMSIS-NN and the compilers' vectorised
//! loops use. An encoding that is not implemented halts the simulation with the encoding
//! instead of executing wrongly.

use crate::bus::Bus;
use crate::cpu::*;

impl Cpu {
    pub(crate) fn exec_mve<B: Bus>(&mut self, bus: &mut B, enc: u32) -> Result<u32, Trap> {
        self.fp_check(bus)?;
        Err(Trap::Unimplemented(enc))
    }
}
