//! Common building blocks of the Corstone-320 simulator: the time base and the event scheduler.

pub mod sched;
pub mod time;

pub use sched::{Scheduler, Slot};
pub use time::Ticks;
