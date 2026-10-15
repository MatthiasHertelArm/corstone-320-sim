//! The Corstone-320 board model (MPS4 with display, camera and microphone) and the
//! [`Machine`] that runs it.

pub mod board;
pub mod machine;
pub mod runtime;
mod semihost;
pub mod sources;

pub use board::{Cs320Board, Inputs};
pub use machine::{Machine, Stop};
pub use runtime::{Simulator, Speed};
