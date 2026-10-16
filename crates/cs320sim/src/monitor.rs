//! `monitor sim ...` commands of the GDB server.

use crate::cli::{camera_source, mic_source};
use cs320_board::runtime::{Cmd, Input, Simulator, Speed};
use cs320_core::time::to_secs;

pub fn handle(sim: &Simulator, cmd: &str) -> Option<String> {
    let w: Vec<&str> = cmd.split_whitespace().collect();
    if w.first() != Some(&"sim") {
        return None;
    }
    let f = |s: &str| s.parse::<f64>().ok();
    let pressed = |s: &str| matches!(s, "press" | "down" | "on" | "1");
    Some(match &w[1..] {
        ["time"] => format!("{:.6} s\n", to_secs(sim.status().time)),
        ["stats"] => {
            let s = sim.status();
            format!(
                "time {:.6} s, {} instructions, {:.1} MIPS, {:.2}x real time, {} resets, state {}\n",
                to_secs(s.time),
                s.instret,
                s.mips,
                s.realtime_factor,
                s.resets,
                if s.secure { "Secure" } else { "Non-secure" }
            )
        }
        ["speed", "max"] => {
            sim.send(Cmd::Speed(Speed::Max));
            "speed: max\n".into()
        }
        ["speed", "realtime"] => {
            sim.send(Cmd::Speed(Speed::Realtime(1.0)));
            "speed: real time\n".into()
        }
        ["speed", x] => match f(x) {
            Some(v) if v > 0.0 => {
                sim.send(Cmd::Speed(Speed::Realtime(v)));
                format!("speed: {v}x\n")
            }
            _ => "usage: sim speed <factor|max|realtime>\n".into(),
        },
        ["button", b, action] => {
            let idx = match *b {
                "1" | "0" | "pb1" => 0,
                "2" | "pb2" => 1,
                _ => return Some("usage: sim button <1|2> press|release\n".into()),
            };
            sim.send(Cmd::Input(Input::Button(idx, pressed(action))));
            format!("button {} {}\n", idx + 1, if pressed(action) { "pressed" } else { "released" })
        }
        ["switch", n, action] => match n.parse::<usize>() {
            Ok(i) if i < 8 => {
                sim.send(Cmd::Input(Input::Switch(i, pressed(action))));
                format!("switch {i} {}\n", if pressed(action) { "on" } else { "off" })
            }
            _ => "usage: sim switch <0-7> on|off\n".into(),
        },
        ["vio", "button", n, action] => match n.parse::<usize>() {
            Ok(i) if i < 32 => {
                sim.send(Cmd::Input(Input::VioButton(i, pressed(action))));
                format!("vioBUTTON{i} {}\n", if pressed(action) { "pressed" } else { "released" })
            }
            _ => "usage: sim vio button <n> press|release\n".into(),
        },
        ["vio", "value", id, v] => match (id.parse::<usize>(), v.parse::<i32>()) {
            (Ok(i), Ok(v)) if i < 64 => {
                sim.send(Cmd::Input(Input::VioValue(i, v)));
                format!("vio value {i} = {v}\n")
            }
            _ => "usage: sim vio value <id> <value>\n".into(),
        },
        ["leds"] => {
            let s = sim.status();
            let bits = |v: u32, n: usize| (0..n).rev().map(|i| if v & (1 << i) != 0 { '#' } else { '.' }).collect::<String>();
            format!("FPGA LEDs {}  VIO LEDs {}\n", bits(s.leds, 10), bits(s.vio_leds, 8))
        }
        ["uart", rest @ ..] => {
            let (n, words) = match rest.first().and_then(|x| x.parse::<usize>().ok()) {
                Some(n) if n < 6 && rest.len() > 1 => (n, &rest[1..]),
                _ => (0, rest),
            };
            let mut text = words.join(" ");
            text.push('\n');
            sim.send(Cmd::UartIn(n, text.into_bytes()));
            "sent\n".into()
        }
        ["camera", src] if camera_source(src).is_live() => {
            "camera: live needs the webcam started at start-up: run with --camera live (and --camera-device N)\n".to_string()
        }
        ["camera", src] => {
            sim.send(Cmd::Input(Input::Camera(camera_source(src))));
            format!("camera: {src}\n")
        }
        ["mic", rest @ ..] => {
            let spec = rest.join(":");
            sim.send(Cmd::Input(Input::Microphone(mic_source(&spec))));
            format!("microphone: {spec}\n")
        }
        ["display"] => sim
            .with(|m| match m.display_crc(0) {
                Some((w, h, crc)) => format!("{w}x{h} crc32 {crc:08x} ({} frames)\n", m.sys.board.frame_counts[0]),
                None => "no frame yet\n".into(),
            })
            .unwrap_or_default(),
        ["warnings"] => {
            let w = sim.status().warnings;
            if w.is_empty() {
                "no warnings\n".into()
            } else {
                w.join("\n") + "\n"
            }
        }
        _ => "unknown sim command (monitor help)\n".into(),
    })
}
