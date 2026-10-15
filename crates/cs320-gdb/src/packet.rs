//! GDB remote serial protocol framing: `$data#cs`, acknowledgements, escapes, Ctrl-C.

use std::io::{Read, Write};
use std::net::TcpStream;

#[derive(Debug, PartialEq, Eq)]
pub enum Incoming {
    Packet(Vec<u8>),
    Interrupt,
    Closed,
}

pub fn checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |a, b| a.wrapping_add(*b))
}

/// Escapes `#`, `$`, `}` and `*` for binary replies.
pub fn escape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for &b in data {
        if matches!(b, b'#' | b'$' | b'}' | b'*') {
            out.push(b'}');
            out.push(b ^ 0x20);
        } else {
            out.push(b);
        }
    }
    out
}

pub fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut it = data.iter();
    while let Some(&b) = it.next() {
        if b == b'}' {
            if let Some(&n) = it.next() {
                out.push(n ^ 0x20);
            }
        } else {
            out.push(b);
        }
    }
    out
}

/// Packet reader on a TCP stream; sends `+` for each received packet unless in no-ack mode.
pub struct Reader {
    stream: TcpStream,
    buf: Vec<u8>,
    pos: usize,
    pub no_ack: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Reader {
    pub fn new(stream: TcpStream, no_ack: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        Reader { stream, buf: Vec::new(), pos: 0, no_ack }
    }

    fn byte(&mut self) -> Option<u8> {
        if self.pos >= self.buf.len() {
            let mut tmp = [0u8; 4096];
            match self.stream.read(&mut tmp) {
                Ok(0) | Err(_) => return None,
                Ok(n) => {
                    self.buf.clear();
                    self.buf.extend_from_slice(&tmp[..n]);
                    self.pos = 0;
                }
            }
        }
        let b = self.buf[self.pos];
        self.pos += 1;
        Some(b)
    }

    pub fn next(&mut self) -> Incoming {
        loop {
            let Some(b) = self.byte() else { return Incoming::Closed };
            match b {
                0x03 => return Incoming::Interrupt,
                b'$' => {
                    let mut data = Vec::new();
                    loop {
                        let Some(c) = self.byte() else { return Incoming::Closed };
                        if c == b'#' {
                            break;
                        }
                        data.push(c);
                    }
                    let (Some(h), Some(l)) = (self.byte(), self.byte()) else { return Incoming::Closed };
                    let sum = u8::from_str_radix(std::str::from_utf8(&[h, l]).unwrap_or("00"), 16).unwrap_or(0);
                    let ok = sum == checksum(&data);
                    if !self.no_ack.load(std::sync::atomic::Ordering::Relaxed) {
                        let _ = self.stream.write_all(if ok { b"+" } else { b"-" });
                    }
                    if ok {
                        return Incoming::Packet(data);
                    }
                }
                _ => {} // '+', '-' and noise
            }
        }
    }
}

pub fn frame(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 4);
    out.push(b'$');
    out.extend_from_slice(data);
    out.push(b'#');
    out.extend_from_slice(format!("{:02x}", checksum(data)).as_bytes());
    out
}

pub fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &[u8]) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    s.chunks(2).map(|c| u8::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing() {
        assert_eq!(frame(b"OK"), b"$OK#9a");
        assert_eq!(unescape(&escape(b"a#b$c}d*")), b"a#b$c}d*");
        assert_eq!(unhex(b"0aff"), Some(vec![0x0a, 0xff]));
    }
}
