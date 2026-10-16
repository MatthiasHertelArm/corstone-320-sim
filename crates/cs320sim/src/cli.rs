//! Command-line parsing: the FVP form (`-a`, `--simlimit` …), the native options and the
//! pyOCD-style `gdbserver` form of the CMSIS Debugger adapters. The simulator has one fixed
//! configuration (Corstone-320, Ethos-U85 with 256 MACs), so there are no `-C` parameters and
//! no configuration files.

use cs320_board::runtime::Speed;
use cs320_board::sources::{CameraSource, MicSource};
use cs320_soc::Params;
use std::path::PathBuf;

pub const USAGE: &str = "\
usage:
  cs320sim [run] [IMAGE...] [options]       run images (.elf/.axf/.hex/.bin)
  cs320sim -a IMAGE [...]                   FVP-compatible form (also with the CMSIS Arm-FVP debug adapter):
                                            -a [cpu0=]IMAGE, --simlimit SECONDS, --stat, --plugin X and -D
                                            are accepted; -C and -f are not (one fixed configuration)
  cs320sim gdbserver [--cbuild-run FILE] [--port N] [--telnet-port N] [--persist]
                                            GDB server with pyOCD-compatible options
  cs320sim load [IMAGE | --cbuild-run FILE] load an image into a running simulator (control port)
  cs320sim info [--cbuild-run FILE]         target information
  cs320sim register-cmsis-adapter [--extension DIR] [--apply --backup-dir DIR]
                                            add the corstone-320-sim adapter to CMSIS Solution

options:
  --cbuild-run FILE     images (and GDB port) from a CMSIS-Toolbox *.cbuild-run.yml
  --headless            no window; UART0 goes to stdout
  --gdb-port N          start the GDB server on port N (the core waits halted for the debugger)
  --reset-run           with a GDB port: run immediately instead of waiting halted
  --bind ADDR           address of the servers (default 127.0.0.1)
  --uart-tcp N          serve UART0 on TCP port N (VS Code Serial Monitor: TCP mode); UART k on N+k
  --uart-stdout         print UART0 on stdout (default with --headless)
  --uart-stdin          send stdin to UART0
  --send SECONDS:TEXT   send TEXT to UART0 after SECONDS of wall time
  --speed X|max         simulation speed factor (default 1 = real time)
  --time SECONDS        stop after this simulated time (headless)
  --exit-on-eot         exit when UART0 sends EOT (0x04), like the FVP's uart0.shutdown_on_eot
  --semihosting         serve semihosting calls (BKPT 0xAB) on the console
  --camera SRC          camera source: pattern (default), none, live (a webcam), an image file or a folder of images
  --camera-device N     the webcam for --camera live: ffmpeg's device index (default 0)
  --mic-device N        the microphone for --mic live: ffmpeg's audio index (default: the system input)
  --mic SRC             microphone source: silence (default), tone[:HZ[:AMP]], a WAV file, live
  --scenario FILE       timed inputs and expectations (YAML, simulated time); exit code 1 on failure
  --persist             keep running when the debugger disconnects
  --stats               print statistics at the end
  --control-port N      local control port for cs320sim load (default 4470, 0 = off)
  -q, --quiet / -v, --verbose
";

#[derive(Debug, Clone)]
pub struct Config {
    pub images: Vec<PathBuf>,
    pub cbuild_run: Option<PathBuf>,
    pub headless: bool,
    pub gdb_port: Option<u16>,
    pub bind: String,
    pub uart_tcp: Option<u16>,
    pub uart_stdout: bool,
    pub uart_stdin: bool,
    pub uart_file: Option<PathBuf>,
    pub uart_send: Vec<(f64, String)>,
    pub speed: Speed,
    pub max_time: Option<f64>,
    pub persist: bool,
    pub reset_run: bool,
    pub semihosting: bool,
    pub stats: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub help: bool,
    pub version: bool,
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub title: String,
    pub scenario: Option<PathBuf>,
    pub control_port: u16,
    pub exit_on_eot: bool,
    pub camera: CameraSource,
    /// The webcam for `--camera live`: ffmpeg's device index (0 by default).
    pub camera_device: u32,
    /// The microphone for `--mic live`: ffmpeg's audio device index (None: the system default).
    pub mic_device: Option<u32>,
    pub mic: MicSource,
    pub params: Params,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            images: Vec::new(),
            cbuild_run: None,
            headless: false,
            gdb_port: None,
            bind: "127.0.0.1".into(),
            uart_tcp: None,
            uart_stdout: false,
            uart_stdin: false,
            uart_file: None,
            uart_send: Vec::new(),
            speed: Speed::Realtime(1.0),
            max_time: None,
            persist: false,
            reset_run: false,
            semihosting: false,
            stats: false,
            quiet: false,
            verbose: false,
            help: false,
            version: false,
            title: "Corstone-320 simulator".into(),
            scenario: None,
            control_port: crate::control::DEFAULT_PORT,
            exit_on_eot: false,
            camera: CameraSource::TestPattern,
            camera_device: 0,
            mic_device: None,
            mic: MicSource::Silence,
            params: Params::default(),
        }
    }
}

fn value(args: &[String], i: &mut usize, name: &str) -> Result<String, String> {
    *i += 1;
    args.get(*i).cloned().ok_or_else(|| format!("{name} needs a value"))
}

fn port(s: &str) -> Result<u16, String> {
    s.trim().trim_matches('"').parse().map_err(|_| format!("invalid port '{s}'"))
}

fn speed(s: &str) -> Result<Speed, String> {
    if s == "max" {
        Ok(Speed::Max)
    } else if s == "realtime" {
        Ok(Speed::Realtime(1.0))
    } else {
        s.parse::<f64>().map(Speed::Realtime).map_err(|_| format!("invalid speed '{s}'"))
    }
}

pub fn camera_source(s: &str) -> CameraSource {
    match s {
        "pattern" | "test" => CameraSource::TestPattern,
        "none" | "off" => CameraSource::None,
        "live" | "webcam" => CameraSource::Live,
        p => {
            let path = PathBuf::from(p);
            if path.is_dir() {
                CameraSource::Folder(path)
            } else {
                CameraSource::Image(path)
            }
        }
    }
}

pub fn mic_source(s: &str) -> MicSource {
    match s {
        "silence" | "none" | "off" => MicSource::Silence,
        "live" => MicSource::Live,
        t if t.starts_with("tone") => {
            let mut parts = t.split(':').skip(1);
            let hz = parts.next().and_then(|x| x.parse().ok()).unwrap_or(1000.0);
            let amp = parts.next().and_then(|x| x.parse().ok()).unwrap_or(0.5);
            MicSource::Tone(hz, amp)
        }
        p => MicSource::Wav(PathBuf::from(p)),
    }
}

/// Parses the arguments; FVP form when `-a`, `--plugin` or `--application` appear.
pub fn parse(raw: Vec<String>) -> Result<Config, String> {
    let args: Vec<String> = raw.iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
    let fvp = args.iter().any(|a| a == "-a" || a == "--plugin" || a.starts_with("--application"));
    if fvp {
        return parse_fvp(&args);
    }
    let mut c = Config::default();
    let mut i = 0;
    let mut gdbserver = false;
    if args.first().map(|s| s.as_str()) == Some("gdbserver") {
        gdbserver = true;
        i = 1;
    } else if args.first().map(|s| s.as_str()) == Some("run") {
        i = 1;
    }
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-h" | "--help" => c.help = true,
            "-V" | "--version" => c.version = true,
            "--cbuild-run" => c.cbuild_run = Some(value(&args, &mut i, a)?.into()),
            "--headless" => c.headless = true,
            "--gdb-port" | "--port" | "-p" => c.gdb_port = Some(port(&value(&args, &mut i, a)?)?),
            "--telnet-port" | "-T" | "--uart-tcp" => c.uart_tcp = Some(port(&value(&args, &mut i, a)?)?),
            "--bind" => c.bind = value(&args, &mut i, a)?,
            "--allow-remote" => c.bind = "0.0.0.0".into(),
            "--uart-stdout" => c.uart_stdout = true,
            "--uart-stdin" => c.uart_stdin = true,
            "--send" => {
                let v = value(&args, &mut i, a)?;
                let (t, text) = v.split_once(':').ok_or("--send SECONDS:TEXT")?;
                c.uart_send.push((t.parse().map_err(|_| "--send SECONDS:TEXT")?, text.replace("\\n", "\n").replace("\\r", "\r")));
            }
            "--speed" => c.speed = speed(&value(&args, &mut i, a)?)?,
            "--time" => c.max_time = Some(value(&args, &mut i, a)?.parse().map_err(|_| "--time SECONDS")?),
            "--exit-on-eot" => c.exit_on_eot = true,
            "--persist" => c.persist = true,
            "--reset-run" => c.reset_run = true,
            "--semihosting" | "-S" => c.semihosting = true,
            "--stats" | "--stat" => c.stats = true,
            "--camera" => c.camera = camera_source(&value(&args, &mut i, a)?),
            "--camera-device" => c.camera_device = value(&args, &mut i, a)?.parse().map_err(|_| "--camera-device N")?,
            "--mic-device" => c.mic_device = Some(value(&args, &mut i, a)?.parse().map_err(|_| "--mic-device N")?),
            "--mic" | "--microphone" => c.mic = mic_source(&value(&args, &mut i, a)?),
            "--scenario" => c.scenario = Some(value(&args, &mut i, a)?.into()),
            "--control-port" => c.control_port = port(&value(&args, &mut i, a)?)?,
            "-q" | "--quiet" => c.quiet = true,
            "-v" | "--verbose" => c.verbose = true,
            // pyOCD options that do not apply to the simulator
            "--probe" | "--uid" | "-u" | "--log" | "-L" | "--target" | "-t" | "--frequency" | "-O" | "--config" | "--pack" | "--elf"
            | "--connect" | "-M" | "--erase" | "--script" => {
                i += 1;
            }
            "--no-config" | "--vector-catch" | "--step-int" | "--no-wait" => {}
            _ if a.starts_with('-') => log::warn!("ignoring unknown option {a}"),
            _ => c.images.push(PathBuf::from(a)),
        }
        i += 1;
    }
    if gdbserver && c.gdb_port.is_none() {
        let p = c
            .cbuild_run
            .as_ref()
            .and_then(|f| cs320_loader::cbuild_run::RunConfig::load(f).ok())
            .and_then(|r| r.gdb_port())
            .unwrap_or(3333);
        c.gdb_port = Some(p);
    }
    Ok(c)
}

/// FVP command line: `-a [instance=]image`, `--plugin`, `-D`, ...
fn parse_fvp(args: &[String]) -> Result<Config, String> {
    let mut c = Config::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-C" | "--parameter" | "-f" | "--config-file" | "--list-params" | "-l" => {
                return Err(format!("{a} is not supported: the simulator has one fixed configuration (no parameters, no config files)"));
            }
            _ if a.starts_with("-C") || a.starts_with("--parameter=") => {
                return Err(format!("{a} is not supported: the simulator has one fixed configuration (no parameters, no config files)"));
            }
            "-a" | "--application" => {
                let v = value(args, &mut i, a)?;
                let file = v.split_once('=').map(|(_, f)| f.to_string()).unwrap_or(v);
                c.images.push(PathBuf::from(file.trim_matches('"')));
            }
            _ if a.starts_with("--application=") => {
                let v = &a["--application=".len()..];
                let file = v.split_once('=').map(|(_, f)| f.to_string()).unwrap_or(v.to_string());
                c.images.push(PathBuf::from(file.trim_matches('"')));
            }
            "--plugin" | "--data" | "-I" | "--iris-port" | "--cpulimit" | "--cyclelimit" => {
                i += 1;
            }
            "--simlimit" => c.max_time = Some(value(args, &mut i, a)?.parse().map_err(|_| "--simlimit SECONDS")?),
            "--headless" => c.headless = true,
            "--stat" | "--stats" => c.stats = true,
            "-h" | "--help" => c.help = true,
            "--version" => c.version = true,
            "-q" | "--quiet" => c.quiet = true,
            "--camera" => c.camera = camera_source(&value(args, &mut i, a)?),
            "--camera-device" => c.camera_device = value(args, &mut i, a)?.parse().map_err(|_| "--camera-device N")?,
            "--mic-device" => c.mic_device = Some(value(args, &mut i, a)?.parse().map_err(|_| "--mic-device N")?),
            "--mic" | "--microphone" => c.mic = mic_source(&value(args, &mut i, a)?),
            "--scenario" => c.scenario = Some(value(args, &mut i, a)?.into()),
            _ if a.starts_with('-') => {}
            _ => c.images.push(PathBuf::from(a)),
        }
        i += 1;
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn fvp_form() {
        let c = parse(v(&["-D", "--plugin", "/x/GDBServer.so\n", "-a", "out/app.axf", "--simlimit", "60"])).unwrap();
        assert_eq!(c.images, vec![PathBuf::from("out/app.axf")]);
        assert_eq!(c.max_time, Some(60.0));
    }

    #[test]
    fn fvp_parameters_are_rejected() {
        assert!(parse(v(&["-a", "app.elf", "-C", "mps4_board.uart0.out_file=-"])).is_err());
        assert!(parse(v(&["-a", "app.elf", "-f", "fvp_config.txt"])).is_err());
    }

    #[test]
    fn pyocd_form() {
        let c = parse(v(&["gdbserver", "--probe", "cmsisdap:", "--port", "3334", "--quiet", "--log", "*.x=info"])).unwrap();
        assert_eq!(c.gdb_port, Some(3334));
        assert!(c.quiet);
    }
}
