//! The target UARTs on TCP (VS Code Serial Monitor in TCP mode), on the console and in a file.

use cs320_board::runtime::{Cmd, Simulator};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;

pub fn serve_tcp(sim: Simulator, bind: &str, port: u16, n: usize) -> Result<(), String> {
    let addr = format!("{bind}:{port}");
    let listener = TcpListener::bind(&addr).map_err(|e| format!("UART{n} server on {addr}: {e}"))?;
    if n == 0 {
        eprintln!("UART0 on TCP port {port} (UART1-5 on the following ports)");
    }
    std::thread::Builder::new()
        .name(format!("uart{n}-tcp"))
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = stream.set_nodelay(true);
                let rx = sim.subscribe_uart(n);
                let mut w = match stream.try_clone() {
                    Ok(w) => w,
                    Err(_) => continue,
                };
                std::thread::spawn(move || {
                    for data in rx {
                        if w.write_all(&data).is_err() {
                            break;
                        }
                    }
                });
                let s = sim.clone();
                let mut r = stream;
                std::thread::spawn(move || {
                    let mut buf = [0u8; 256];
                    while let Ok(k) = r.read(&mut buf) {
                        if k == 0 {
                            break;
                        }
                        s.send(Cmd::UartIn(n, buf[..k].to_vec()));
                    }
                });
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn to_stdout(sim: Simulator, stdin: bool) {
    let rx = sim.subscribe_uart(0);
    std::thread::spawn(move || {
        let mut out = std::io::stdout();
        for data in rx {
            let _ = out.write_all(&data);
            let _ = out.flush();
        }
    });
    if stdin {
        std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            let mut input = std::io::stdin();
            while let Ok(n) = input.read(&mut buf) {
                if n == 0 {
                    break;
                }
                sim.send(Cmd::UartIn(0, buf[..n].to_vec()));
            }
        });
    }
}

/// UART0 output into a file (`uart0.out_file`).
pub fn to_file(sim: Simulator, path: &Path) -> Result<(), String> {
    let mut f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let rx = sim.subscribe_uart(0);
    std::thread::spawn(move || {
        for data in rx {
            let _ = f.write_all(&data);
            let _ = f.flush();
        }
    });
    Ok(())
}

/// Ends the process when UART0 sends EOT (0x04), as the FVP does with `uart0.shutdown_on_eot`.
pub fn exit_on_eot(sim: Simulator) {
    let rx = sim.subscribe_uart(0);
    std::thread::spawn(move || {
        for data in rx {
            if data.contains(&0x04) {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let _ = std::io::stdout().flush();
                let st = sim.status();
                eprintln!("\ncs320sim: EOT from the target at {:.6} s, exiting", cs320_core::time::to_secs(st.time));
                std::process::exit(0);
            }
        }
    });
}
