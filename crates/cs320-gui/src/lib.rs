//! Window of the Corstone-320 simulator (placeholder until the GUI is written).

use cs320_board::runtime::Simulator;

pub struct GuiOptions {
    pub title: String,
}

pub fn run(_sim: Simulator, _opts: GuiOptions) -> Result<(), String> {
    Err("the GUI is not available in this build".into())
}
