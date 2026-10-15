//! Arm semihosting (BKPT 0xAB) for test firmware: console output, input, clock, exit.

use crate::machine::Machine;
use cs320_core::time::to_secs;

const SYS_OPEN: u32 = 0x01;
const SYS_CLOSE: u32 = 0x02;
const SYS_WRITEC: u32 = 0x03;
const SYS_WRITE0: u32 = 0x04;
const SYS_WRITE: u32 = 0x05;
const SYS_READ: u32 = 0x06;
const SYS_READC: u32 = 0x07;
const SYS_ISERROR: u32 = 0x08;
const SYS_ISTTY: u32 = 0x09;
const SYS_SEEK: u32 = 0x0A;
const SYS_FLEN: u32 = 0x0C;
const SYS_CLOCK: u32 = 0x10;
const SYS_TIME: u32 = 0x11;
const SYS_ERRNO: u32 = 0x13;
const SYS_GET_CMDLINE: u32 = 0x15;
const SYS_HEAPINFO: u32 = 0x16;
const SYS_EXIT: u32 = 0x18;
const SYS_EXIT_EXTENDED: u32 = 0x20;
const SYS_ELAPSED: u32 = 0x30;
const SYS_TICKFREQ: u32 = 0x31;

const ADP_STOPPED_APPLICATION_EXIT: u32 = 0x20026;

impl Machine {
    fn rd(&mut self, a: u32) -> u32 {
        self.cpu.debug_read(&mut self.sys, a, 4).unwrap_or(0)
    }

    fn rd8(&mut self, a: u32) -> u8 {
        self.cpu.debug_read(&mut self.sys, a, 1).unwrap_or(0) as u8
    }

    fn wr8(&mut self, a: u32, v: u8) {
        self.cpu.debug_write(&mut self.sys, a, 1, v as u32);
    }

    fn wr(&mut self, a: u32, v: u32) {
        self.cpu.debug_write(&mut self.sys, a, 4, v);
    }

    /// Services a semihosting call; returns false when the program exited.
    pub(crate) fn semihost(&mut self) -> bool {
        let op = self.cpu.r[0];
        let arg = self.cpu.r[1];
        let ret: u32 = match op {
            SYS_OPEN => {
                let name_ptr = self.rd(arg);
                let mode = self.rd(arg + 4);
                let len = self.rd(arg + 8).min(256);
                let name: Vec<u8> = (0..len).map(|i| self.rd8(name_ptr + i)).collect();
                if name == b":tt" {
                    match mode {
                        0..=3 => 1,
                        4..=7 => 2,
                        _ => 3,
                    }
                } else {
                    u32::MAX
                }
            }
            SYS_CLOSE => 0,
            SYS_WRITEC => {
                let c = self.rd8(arg);
                self.semihost_out.push(c);
                0
            }
            SYS_WRITE0 => {
                let mut a = arg;
                loop {
                    let c = self.rd8(a);
                    if c == 0 || a.wrapping_sub(arg) > 0x10000 {
                        break;
                    }
                    self.semihost_out.push(c);
                    a += 1;
                }
                0
            }
            SYS_WRITE => {
                let fh = self.rd(arg);
                let ptr = self.rd(arg + 4);
                let len = self.rd(arg + 8);
                if fh == 2 || fh == 3 {
                    for i in 0..len.min(0x10000) {
                        let c = self.rd8(ptr + i);
                        self.semihost_out.push(c);
                    }
                    0
                } else {
                    len
                }
            }
            SYS_READ => {
                let ptr = self.rd(arg + 4);
                let len = self.rd(arg + 8);
                let n = (len as usize).min(self.semihost_in.len());
                for i in 0..n {
                    let b = self.semihost_in.remove(0);
                    self.wr8(ptr + i as u32, b);
                }
                len - n as u32
            }
            SYS_READC => {
                if self.semihost_in.is_empty() {
                    u32::MAX
                } else {
                    self.semihost_in.remove(0) as u32
                }
            }
            SYS_ISERROR => 0,
            SYS_ISTTY => 1,
            SYS_SEEK | SYS_ERRNO => 0,
            SYS_FLEN => 0,
            SYS_CLOCK => (to_secs(self.now()) * 100.0) as u32,
            SYS_TIME => to_secs(self.now()) as u32,
            SYS_GET_CMDLINE => {
                let buf = self.rd(arg);
                self.wr8(buf, 0);
                self.wr(arg + 4, 0);
                0
            }
            SYS_HEAPINFO => {
                let block = self.rd(arg);
                for i in 0..4 {
                    self.wr(block + 4 * i, 0);
                }
                0
            }
            SYS_EXIT => {
                self.exit_code = Some(if arg == ADP_STOPPED_APPLICATION_EXIT { 0 } else { 1 });
                return false;
            }
            SYS_EXIT_EXTENDED => {
                let reason = self.rd(arg);
                let code = self.rd(arg + 4);
                self.exit_code = Some(if reason == ADP_STOPPED_APPLICATION_EXIT { code as i32 } else { 1 });
                return false;
            }
            SYS_ELAPSED => {
                let t = self.cpu.now / cs320_core::time::TICKS_PER_CPU_CYCLE;
                self.wr(arg, t as u32);
                self.wr(arg + 4, (t >> 32) as u32);
                0
            }
            SYS_TICKFREQ => self.sys.params.clocks.cpu_hz as u32,
            _ => {
                log::warn!("semihosting operation {op:#x} is not supported");
                u32::MAX
            }
        };
        self.cpu.r[0] = ret;
        self.cpu.pc = self.cpu.pc.wrapping_add(2);
        self.cpu.resume();
        self.cpu.skip_bp_once();
        true
    }
}
