//! Command-line parsing: the FVP form (`-a`, `-C`, `-f`, `--simlimit` …), the native options
//! and the pyOCD-style `gdbserver` form of the CMSIS Debugger adapters.

use cs320_board::runtime::Speed;
use cs320_board::sources::{CameraSource, MicSource};
use cs320_soc::Params;
use std::path::PathBuf;

pub const USAGE: &str = "\
usage:
  cs320sim [run] [IMAGE...] [options]       run images (.elf/.axf/.hex/.bin)
  cs320sim -f CONFIG -a IMAGE [...]         FVP-compatible form (also with the CMSIS Arm-FVP debug adapter):
                                            -C param=value, -f FILE, -a [cpu0=]IMAGE, --simlimit SECONDS, --stat,
                                            --plugin X and -D are accepted; --list-params lists the parameters
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
  --camera SRC          camera source: pattern (default), none, an image file or a folder of images
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
    pub list_params: bool,
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub title: String,
    pub scenario: Option<PathBuf>,
    pub control_port: u16,
    pub exit_on_eot: bool,
    pub camera: CameraSource,
    pub mic: MicSource,
    pub params: Params,
    /// FVP parameters given but not honoured (reported once).
    pub ignored_params: Vec<String>,
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
            list_params: false,
            title: "Corstone-320 simulator".into(),
            scenario: None,
            control_port: crate::control::DEFAULT_PORT,
            exit_on_eot: false,
            camera: CameraSource::TestPattern,
            mic: MicSource::Silence,
            params: Params::default(),
            ignored_params: Vec::new(),
        }
    }
}

fn value(args: &[String], i: &mut usize, name: &str) -> Result<String, String> {
    *i += 1;
    args.get(*i).cloned().ok_or_else(|| format!("{name} needs a value"))
}

/// An FVP boolean parameter: 1 or true.
fn on(v: &str) -> bool {
    v == "1" || v.eq_ignore_ascii_case("true")
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

/// An FVP integer parameter (decimal or 0x hex).
fn int(s: &str) -> Result<u64, String> {
    let s = s.trim().trim_matches('"');
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).map_err(|_| format!("invalid number '{s}'"))
    } else {
        s.parse().map_err(|_| format!("invalid number '{s}'"))
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

/// Parses the arguments; FVP form when `-C`, `-a`, `-f` or `--plugin` appear.
pub fn parse(raw: Vec<String>) -> Result<Config, String> {
    let args: Vec<String> = raw.iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
    let fvp = args.iter().any(|a| a == "-C" || a == "-a" || a == "--plugin" || a == "-f" || a.starts_with("--application") || a == "--list-params" || a.starts_with("--parameter"));
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
            "--mic" | "--microphone" => c.mic = mic_source(&value(&args, &mut i, a)?),
            "--scenario" => c.scenario = Some(value(&args, &mut i, a)?.into()),
            "--control-port" => c.control_port = port(&value(&args, &mut i, a)?)?,
            "-q" | "--quiet" => c.quiet = true,
            "-v" | "--verbose" => c.verbose = true,
            // pyOCD options that do not apply to the simulator
            "--probe" | "--uid" | "-u" | "--log" | "-L" | "--target" | "-t" | "--frequency" | "-O" | "--config" | "--pack" | "--elf" | "--connect"
            | "-M" | "--erase" | "--script" => {
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

/// FVP command line: `-C key=value`, `-f config`, `-a [instance=]image`, `--plugin`, `-D`, ...
fn parse_fvp(args: &[String]) -> Result<Config, String> {
    let mut c = Config::default();
    let mut params: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-C" | "--parameter" => params.push(value(args, &mut i, a)?),
            _ if a.starts_with("-C") && a.len() > 2 && a.contains('=') => params.push(a[2..].to_string()),
            _ if a.starts_with("--parameter=") => params.push(a["--parameter=".len()..].to_string()),
            "-f" | "--config-file" => {
                let f = value(args, &mut i, a)?;
                let text = std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?;
                for line in text.lines() {
                    // `instance.parameter=value  # comment`, as the FVP writes and reads them
                    let l = line.split_once('#').map_or(line, |(l, _)| l).trim();
                    if !l.is_empty() {
                        params.push(l.to_string());
                    }
                }
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
            "--list-params" | "-l" => c.list_params = true,
            "-h" | "--help" => c.help = true,
            "--version" => c.version = true,
            "-q" | "--quiet" => c.quiet = true,
            "--camera" => c.camera = camera_source(&value(args, &mut i, a)?),
            "--mic" | "--microphone" => c.mic = mic_source(&value(args, &mut i, a)?),
            "--scenario" => c.scenario = Some(value(args, &mut i, a)?.into()),
            _ if a.starts_with('-') => {}
            _ => c.images.push(PathBuf::from(a)),
        }
        i += 1;
    }
    for p in params {
        apply_param(&mut c, &p)?;
    }
    Ok(c)
}

/// Applies one `instance.parameter=value`.
pub fn apply_param(c: &mut Config, p: &str) -> Result<(), String> {
    let Some((k, v)) = p.split_once('=') else { return Ok(()) };
    let (k, v) = (k.trim(), v.trim().trim_matches('"'));
    let key = k.rsplit('.').next().unwrap_or(k);
    let inst = k.rsplit_once('.').map(|(i, _)| i).unwrap_or("");
    match (inst, key) {
        ("GDBServer", "port") => c.gdb_port = Some(port(v)?),
        ("GDBServer", _) => {}
        (_, "uart_port") | (_, "uart-tcp") => c.uart_tcp = Some(port(v)?),
        (_, "disable-visualisation") | (_, "disable_visualisation") | (_, "headless") => c.headless = on(v),
        (_, "shutdown_on_eot") => c.exit_on_eot = on(v),
        (i, "out_file") if i.ends_with("uart0") => {
            if v == "-" {
                c.uart_stdout = true;
            } else if !v.is_empty() {
                c.uart_file = Some(PathBuf::from(v));
            }
        }
        (i, "unbuffered_output") if i.ends_with("uart0") => {}
        (i, "start_telnet") | (i, "quiet") if i.contains("telnetterminal") => {}
        (_, "semihosting-enable") => c.semihosting = on(v),
        (i, "INITSVTOR") | (i, "INITSVTOR_RST") if i.ends_with("cpu0") || i.ends_with("iotss3_systemcontrol") => c.params.initsvtor = int(v)? as u32 & 0xFFFF_FF80,
        (i, "num_macs") if i.ends_with("ethosu") => c.params.npu_macs = int(v)? as u32,
        (_, "QSPI_SRAM_SIZE") => c.params.qspi_size_mb = int(v)? as u32,
        (i, "force_frame_rate") if i.ends_with("hdlcd") => c.params.hdlcd_frame_rate = int(v)? as u32,
        (i, "RTCDR_use_current_time") if i.ends_with("rtc") => c.params.rtc_current_time = on(v),
        (i, "RTCDR_reset_value") if i.ends_with("rtc") => c.params.rtc_reset_value = int(v)? as u32,
        (i, "non_arch_start_at_default") if i.ends_with("refcounter") => c.params.counter_at_reset = on(v),
        (i, "base_frequency") if i.ends_with("refcounter") => c.params.clocks.counter_hz = int(v)?,
        ("cs320sim", "camera") => c.camera = camera_source(v),
        ("cs320sim", "microphone") | ("cs320sim", "mic") => c.mic = mic_source(v),
        ("cs320sim", "speed") => c.speed = speed(v)?,
        ("cs320sim", "persist") => c.persist = on(v),
        ("cs320sim", "scenario") => c.scenario = Some(PathBuf::from(v)),
        (i, "semihosting-cmd_line") | (i, "semihosting-cwd") | (i, "semihosting-heap_base") | (i, "semihosting-heap_limit") | (i, "semihosting-stack_base")
        | (i, "semihosting-stack_limit") | (i, "semihosting-Thumb_SVC")
            if i.ends_with("cpu0") => {}
        _ => c.ignored_params.push(k.to_string()),
    }
    Ok(())
}

/// The parameters `--list-params` prints (the FVP's names that the simulator honours).
pub fn param_list() -> &'static str {
    "\
# Parameters of the Corstone-320 simulator (FVP_Corstone_SSE-320 names; others are accepted and ignored)
# instance.parameter=value       #(type, mode) default = 'def value' : description
#------------------------------------------------------------------------------
GDBServer.port=3333                                   # (int   , init-time) default = '0xd05'  : GDB server port (with --plugin GDBServer.so)
mps4_board.visualisation.disable-visualisation=0      # (bool  , init-time) default = '0'      : No board window
vis_hdlcd.disable_visualisation=0                     # (bool  , init-time) default = '0'      : No display window (same as above)
mps4_board.uart0.out_file=                            # (string, init-time) default = ''       : File for UART0 output ('-' for stdout)
mps4_board.uart0.shutdown_on_eot=0                    # (bool  , init-time) default = '0'      : Exit when UART0 transmits EOT (0x04)
mps4_board.uart0.unbuffered_output=1                  # (bool  , init-time) default = '1'      : Accepted (output is always unbuffered)
mps4_board.subsystem.cpu0.semihosting-enable=0        # (bool  , init-time) default = '0'      : Serve semihosting (BKPT 0xAB) on stdout
mps4_board.subsystem.cpu0.INITSVTOR=0x11000000        # (int   , init-time) default = '0x11000000' : Secure vector table at reset
mps4_board.subsystem.iotss3_systemcontrol.INITSVTOR_RST=0x11000000 # (int, init-time) default = '0x11000000' : Reset value of INITSVTOR0
mps4_board.subsystem.ethosu.num_macs=1024             # (int   , init-time) default = '0x400'  : MACs per cycle reported in the NPU CONFIG register
mps4_board.subsystem.refcounter.base_frequency=25000000 # (int , init-time) default = '0x17d7840' : System counter frequency in Hz
mps4_board.subsystem.refcounter.non_arch_start_at_default=1 # (bool, init-time) default = '1'   : Counter runs from reset
mps4_board.QSPI_SRAM_SIZE=8                           # (int   , init-time) default = '0x8'    : QSPI SRAM size in MB
mps4_board.pl370_hdlcd.force_frame_rate=50            # (int   , init-time) default = '0x32'   : Display frames per simulated second (0: from the HDLCD timing)
mps4_board.rtc.RTCDR_use_current_time=1               # (bool  , init-time) default = '1'      : RTC starts at the host's Unix time
mps4_board.rtc.RTCDR_reset_value=0                    # (int   , init-time) default = '0x0'    : RTC reset value when not the current time
mps4_board.telnetterminalN.start_telnet=1             # (bool  , init-time) default = '1'      : Accepted (use --uart-tcp for a TCP port)
cs320sim.camera=pattern                               # (string, init-time) default = 'pattern' : Camera source: pattern, none, live, image file, folder
cs320sim.microphone=silence                           # (string, init-time) default = 'silence' : Microphone source: silence, tone[:HZ[:AMP]], WAV file, live
cs320sim.speed=1                                      # (string, init-time) default = '1'      : Simulation speed factor or 'max'
cs320sim.persist=0                                    # (bool  , init-time) default = '0'      : Keep running when the debugger disconnects
cs320sim.scenario=                                    # (string, init-time) default = ''       : Scenario file
"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn fvp_form() {
        let c = parse(v(&["-D", "--plugin", "/x/GDBServer.so\n", "-C", "GDBServer.port=3333\n", "-a", "out/app.axf"])).unwrap();
        assert_eq!(c.gdb_port, Some(3333));
        assert_eq!(c.images, vec![PathBuf::from("out/app.axf")]);
    }

    #[test]
    fn fvp_config_file() {
        let f = std::env::temp_dir().join(format!("cs320sim-cli-test-{}.txt", std::process::id()));
        std::fs::write(
            &f,
            "# comment\nmps4_board.visualisation.disable-visualisation=1   # no window\nmps4_board.uart0.shutdown_on_eot=1\nmps4_board.uart0.out_file=-\nmps4_board.subsystem.cpu0.INITSVTOR=0x12000000\nmps4_board.subsystem.ethosu.num_macs=256\n",
        )
        .unwrap();
        let c = parse(v(&["-f", f.to_str().unwrap(), "-C", "mps4_board.visualisation.disable-visualisation=0", "-a", "app.elf", "--simlimit", "60"])).unwrap();
        std::fs::remove_file(&f).ok();
        assert!(!c.headless, "a later -C overrides the config file");
        assert!(c.exit_on_eot);
        assert!(c.uart_stdout);
        assert_eq!(c.max_time, Some(60.0));
        assert_eq!(c.params.initsvtor, 0x1200_0000);
        assert_eq!(c.params.npu_macs, 256);
    }

    #[test]
    fn pyocd_form() {
        let c = parse(v(&["gdbserver", "--probe", "cmsisdap:", "--port", "3334", "--quiet", "--log", "*.x=info"])).unwrap();
        assert_eq!(c.gdb_port, Some(3334));
        assert!(c.quiet);
    }
}
