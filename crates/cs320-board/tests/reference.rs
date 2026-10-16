//! The core against Arm's reference model: the ISA test firmware (`firmware/isa-test`) must
//! print exactly the digests that FVP_Corstone_SSE-320 printed (`fixtures/isa_test.fvp.txt`).
//!
//! The firmware prints one `name: 0x........` line per instruction group; the groups after the
//! marker line `mve: 0x00000001` need MVE. While the simulator stops at its first unimplemented
//! MVE instruction, only the lines up to the marker are compared; once a run reaches the
//! semihosting exit, everything is.
//!
//! Groups listed in `fixtures/isa_test.known_mismatches.txt` (and in the environment variable
//! `ISA_TEST_KNOWN_MISMATCHES`, comma-separated) are allowed to differ or to be missing: the
//! test prints them and passes, so it can run in CI while the core is being fixed. A listed
//! group that matches again is reported so that it can be removed from the list.

use cs320_board::{Machine, Stop};
use cs320_core::time::from_secs;
use cs320_cpu::HaltReason;
use cs320_loader::Image;
use cs320_soc::Params;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn fixture(p: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(p)
}

fn known_mismatches() -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    if let Ok(text) = std::fs::read_to_string(fixture("isa_test.known_mismatches.txt")) {
        for line in text.lines() {
            let name = line.split('#').next().unwrap_or("").trim();
            if !name.is_empty() {
                set.insert(name.to_string());
            }
        }
    }
    if let Ok(extra) = std::env::var("ISA_TEST_KNOWN_MISMATCHES") {
        set.extend(extra.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from));
    }
    set
}

fn group_name(line: &str) -> &str {
    line.split(':').next().unwrap_or(line)
}

#[test]
fn isa_test_matches_fvp() {
    let img = Image::load(&fixture("elf/isa_test.elf")).unwrap();
    let mut m = Machine::new(Params::default());
    m.load_image(&img).unwrap();
    m.power_on();
    m.semihosting = true;
    let stop = m.run_until(from_secs(60.0));
    let out = String::from_utf8_lossy(&m.take_uart()[0]).to_string();
    // A lockup resets the core and the firmware starts over: keep the first run only.
    let first_run = match out.match_indices("isa-test begin").nth(1) {
        Some((i, _)) => &out[..i],
        None => &out[..],
    };
    let sim: Vec<&str> = first_run.lines().filter(|l| l.contains(": 0x")).collect();

    let reference = std::fs::read_to_string(fixture("isa_test.fvp.txt")).unwrap();
    let fvp: Vec<&str> = reference.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    let marker = fvp.iter().position(|l| l.starts_with("mve: ")).expect("the fixture has the `mve` marker line");
    let known = known_mismatches();

    // How much of the reference this run is expected to reproduce.
    let (expected, note) = match stop {
        Stop::Exited => (fvp.len(), format!("the run reached the semihosting exit (code {:?})", m.exit_code)),
        Stop::Halted(HaltReason::Unimplemented(enc)) if sim.len() > marker => {
            (marker + 1, format!("the run stopped at an unimplemented instruction ({enc:#010x}): the MVE section is not compared"))
        }
        other => (fvp.len(), format!("the run ended with {other:?} after {} resets", m.resets)),
    };
    let compared = sim.len().min(expected);

    let mut failures = Vec::new();
    let mut tolerated = Vec::new();
    let mut fixed = Vec::new();
    for (s, f) in sim[..compared].iter().zip(&fvp[..compared]) {
        let name = group_name(f);
        if s == f {
            if known.contains(name) {
                fixed.push(name.to_string());
            }
        } else if known.contains(name) {
            tolerated.push(format!("  sim {s:34} fvp {f}"));
        } else {
            failures.push(format!("  sim {s:34} fvp {f}"));
        }
    }
    for f in &fvp[compared..expected] {
        let name = group_name(f);
        if known.contains(name) {
            tolerated.push(format!("  sim {:34} fvp {f}", "(not reached)"));
        } else {
            failures.push(format!("  sim {:34} fvp {f}", "(not reached)"));
        }
    }

    eprintln!(
        "isa-test: {note}; {compared} of {} groups compared: {} differ, {} known mismatches tolerated",
        fvp.len(),
        failures.len(),
        tolerated.len()
    );
    if !tolerated.is_empty() {
        eprintln!("known mismatches (fixtures/isa_test.known_mismatches.txt):\n{}", tolerated.join("\n"));
    }
    if !fixed.is_empty() {
        eprintln!("listed as known mismatches but now matching the FVP (remove them from the list): {}", fixed.join(", "));
    }
    assert!(failures.is_empty(), "digests that differ from the FVP:\n{}", failures.join("\n"));
    if stop == Stop::Exited {
        assert_eq!(m.exit_code, Some(0));
    } else {
        // Not reaching the end is tolerated only while every missing group is a known mismatch.
        assert!(
            matches!(stop, Stop::Halted(HaltReason::Unimplemented(_)))
                || sim.len() >= expected
                || tolerated.iter().any(|t| t.contains("(not reached)")),
            "the test firmware did not finish: {note}"
        );
    }
}
