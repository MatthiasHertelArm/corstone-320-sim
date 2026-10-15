//! Armv8.1-M Mainline instruction-set simulator of the Cortex-M85 core of the SSE-320.
//!
//! The core executes Thumb-2 with the DSP extension, the FPv5 floating-point extension in
//! half, single and double precision, the low-overhead-branch extension and the Security
//! Extension; implements the exception model, the System Control Space (NVIC, SCB, SysTick,
//! MPU, SAU, FPU control, PMU) and the debug components (DCB, DWT, FPB, ITM). Memory and
//! peripherals are reached through [`Bus`].

pub mod alu;
pub mod bus;
pub mod cpu;
pub mod decode;
mod exec;
pub mod fpu;
mod mve;
pub mod scs;

pub use bus::{Bus, BusError, Idau, IrqLines, ResetRequest};
pub use cpu::{Cpu, Exit, HaltReason, Watch};
