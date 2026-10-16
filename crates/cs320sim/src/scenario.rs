//! Scenario files: timed inputs and expectations in simulated time (deterministic).
//!
//! ```yaml
//! - at: 0.5            # seconds of simulated time
//!   press: 1           # FPGA user button 1 or 2
//! - at: 1.0
//!   release: 1
//!   switches: "00000101"
//!   uart: "hello\n"
//! - at: 1.5
//!   expect-uart: "= Blinky is running ="
//!   expect-leds: "0000000010"   # FPGA LEDs 9..0 (x = any)
//! - at: 2.0
//!   expect-vio-leds: "00000001"
//!   expect-display: { width: 320, height: 240 }   # optional crc32
//!   camera: pattern
//!   mic: "tone:1000:0.5"
//!   exit: true
//! ```

use crate::cli::{camera_source, mic_source};
use cs320_board::runtime::{Cmd, Simulator};
use cs320_board::Machine;
use cs320_core::time::{from_secs, to_secs};
use serde::Deserialize;
use std::sync::{Arc, Mutex};

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Step {
    pub at: f64,
    pub press: Option<u32>,
    pub release: Option<u32>,
    pub switches: Option<String>,
    pub vio_press: Option<u32>,
    pub vio_release: Option<u32>,
    pub vio_value: Option<[i32; 2]>,
    pub uart: Option<String>,
    pub uart1: Option<String>,
    pub camera: Option<String>,
    pub mic: Option<String>,
    pub reset: Option<bool>,
    pub expect_uart: Option<String>,
    pub expect_uart1: Option<String>,
    pub expect_leds: Option<String>,
    pub expect_vio_leds: Option<String>,
    pub expect_display: Option<ExpectDisplay>,
    pub expect_frames: Option<u64>,
    pub exit: Option<bool>,
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct ExpectDisplay {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub crc32: Option<String>,
}

#[derive(Default)]
pub struct Results {
    pub passed: usize,
    pub failed: usize,
}

fn check(res: &Arc<Mutex<Results>>, t: f64, ok: bool, what: String) {
    let mut r = res.lock().unwrap();
    if ok {
        r.passed += 1;
        eprintln!("[{t:8.3} s] PASS {what}");
    } else {
        r.failed += 1;
        eprintln!("[{t:8.3} s] FAIL {what}");
    }
}

/// Compares a bit pattern string (MSB first, `x` = don't care) with a value.
fn bits_match(pattern: &str, v: u32) -> bool {
    let p: Vec<char> = pattern.chars().filter(|c| !c.is_whitespace()).collect();
    let n = p.len();
    p.iter().enumerate().all(|(i, c)| {
        let bit = (v >> (n - 1 - i)) & 1;
        match c {
            '0' | '.' => bit == 0,
            '1' | '#' => bit == 1,
            _ => true,
        }
    })
}

fn bits_string(v: u32, n: usize) -> String {
    (0..n).rev().map(|i| if v & (1 << i) != 0 { '1' } else { '0' }).collect()
}

pub fn load(path: &std::path::Path) -> Result<Vec<Step>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_yaml_ng::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Schedules the steps on the simulator.
pub fn schedule(sim: &Simulator, steps: Vec<Step>) -> Arc<Mutex<Results>> {
    let res = Arc::new(Mutex::new(Results::default()));
    for step in steps {
        let r = res.clone();
        let at = from_secs(step.at);
        sim.send(Cmd::At(
            at,
            Box::new(move |m: &mut Machine| {
                let t = to_secs(m.now());
                let b = &mut m.sys.board;
                if let Some(p) = step.press {
                    b.inputs.buttons[(p.clamp(1, 2) - 1) as usize] = true;
                }
                if let Some(p) = step.release {
                    b.inputs.buttons[(p.clamp(1, 2) - 1) as usize] = false;
                }
                if let Some(s) = &step.switches {
                    let v = s.chars().filter(|c| *c == '0' || *c == '1').fold(0u8, |a, c| (a << 1) | (c == '1') as u8);
                    b.inputs.switches = v;
                }
                if let Some(p) = step.vio_press {
                    b.inputs.vio_buttons |= 1 << p.min(31);
                }
                if let Some(p) = step.vio_release {
                    b.inputs.vio_buttons &= !(1 << p.min(31));
                }
                if let Some([id, v]) = step.vio_value {
                    b.inputs.vio_values.retain(|(i, _)| *i as i32 != id);
                    b.inputs.vio_values.push((id as usize, v));
                }
                if let Some(c) = &step.camera {
                    b.camera.set_source(camera_source(c));
                }
                if let Some(s) = &step.mic {
                    b.mic.set_source(mic_source(s));
                }
                if let Some(u) = &step.uart {
                    m.uart_send(0, u.as_bytes());
                }
                if let Some(u) = &step.uart1 {
                    m.uart_send(1, u.as_bytes());
                }
                if step.reset == Some(true) {
                    m.reset_button();
                }
                let now = m.now();
                m.sys.inputs_changed(now);
                if let Some(e) = &step.expect_uart {
                    let text = m.uart_text(0);
                    check(&r, t, text.contains(e.as_str()), format!("UART0 contains {e:?}"));
                }
                if let Some(e) = &step.expect_uart1 {
                    let text = m.uart_text(1);
                    check(&r, t, text.contains(e.as_str()), format!("UART1 contains {e:?}"));
                }
                if let Some(e) = &step.expect_leds {
                    let v = m.sys.board.leds;
                    check(&r, t, bits_match(e, v), format!("FPGA LEDs {e} (got {})", bits_string(v, 10)));
                }
                if let Some(e) = &step.expect_vio_leds {
                    let v = m.sys.board.vio_leds;
                    check(&r, t, bits_match(e, v), format!("VIO LEDs {e} (got {})", bits_string(v, 8)));
                }
                if let Some(e) = &step.expect_display {
                    match m.display_crc(0) {
                        Some((w, h, crc)) => {
                            let ok = e.width.is_none_or(|x| x == w)
                                && e.height.is_none_or(|x| x == h)
                                && e.crc32.as_ref().is_none_or(|c| u32::from_str_radix(c.trim_start_matches("0x"), 16).ok() == Some(crc));
                            check(
                                &r,
                                t,
                                ok,
                                format!("display {:?}x{:?} crc {:?} (got {w}x{h} crc32 {crc:08x})", e.width, e.height, e.crc32),
                            );
                        }
                        None => check(&r, t, false, "display frame (none yet)".into()),
                    }
                }
                if let Some(n) = step.expect_frames {
                    let got = m.sys.board.frame_counts[0];
                    check(&r, t, got >= n, format!("at least {n} display frames (got {got})"));
                }
                if step.exit == Some(true) {
                    let failed = r.lock().unwrap().failed;
                    m.exit_code = Some(if failed > 0 { 1 } else { 0 });
                }
            }),
        ));
    }
    res
}
