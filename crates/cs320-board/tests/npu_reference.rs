//! The Ethos-U85 model against Arm's reference model: the NPU test firmware
//! (`firmware/npu-test`) runs six Vela-compiled networks through the ethos-u-core-driver and
//! prints the NPU registers and one digest per network output; the simulator must print exactly
//! what FVP_Corstone_SSE-320 printed (`fixtures/npu_test.fvp.txt`), except the `.cycles_*` lines,
//! which are the FVP's timing model.

use cs320_board::{Machine, Stop};
use cs320_core::time::from_secs;
use cs320_loader::Image;
use cs320_soc::Params;
use std::path::{Path, PathBuf};

fn fixture(p: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(p)
}

fn compared(line: &str) -> bool {
    (line.contains(": 0x") || line.starts_with("npu-test")) && !line.contains(".cycles_")
}

#[test]
fn npu_test_matches_fvp() {
    let img = Image::load(&fixture("elf/npu_test.elf")).unwrap();
    // The FVP reference was run with 1024 MACs (the NPU's CONFIG register and the Vela target);
    // the simulator's own default is 256.
    let params = Params { npu_macs: 1024, ..Params::default() };
    let mut m = Machine::new(params);
    m.load_image(&img).unwrap();
    m.power_on();
    m.semihosting = true;
    let stop = m.run_until(from_secs(120.0));
    let out = String::from_utf8_lossy(&m.take_uart()[0]).to_string();
    let sim: Vec<&str> = out.lines().filter(|l| compared(l)).collect();
    let reference = std::fs::read_to_string(fixture("npu_test.fvp.txt")).unwrap();
    let fvp: Vec<&str> = reference.lines().filter(|l| !l.starts_with('#') && compared(l)).collect();

    let mut failures = Vec::new();
    for (i, f) in fvp.iter().enumerate() {
        match sim.get(i) {
            Some(s) if s == f => {}
            Some(s) => failures.push(format!("  sim {s:40} fvp {f}")),
            None => failures.push(format!("  sim {:40} fvp {f}", "(not reached)")),
        }
    }
    for s in sim.iter().skip(fvp.len()) {
        failures.push(format!("  sim {s:40} fvp (nothing)"));
    }
    eprintln!("npu-test: {stop:?}, {} of {} lines compared, {} differ", sim.len().min(fvp.len()), fvp.len(), failures.len());
    if !failures.is_empty() {
        eprintln!("driver output:\n{out}");
    }
    assert!(failures.is_empty(), "lines that differ from the FVP:\n{}", failures.join("\n"));
    assert_eq!(stop, Stop::Exited);
    assert_eq!(m.exit_code, Some(0));
}
