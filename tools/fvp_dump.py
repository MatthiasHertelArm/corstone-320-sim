#!/usr/bin/env python3
"""Dump memory words from a GDB server (the FVP's GDBServer plugin) with a minimal RSP client.

Only the standard library is used (sockets), no gdb needed.

    fvp_dump.py --port 3333 --ranges ranges.txt --out regs-reset.txt
    fvp_dump.py --port 3333 --regs

ranges.txt: one `start end [name]` per line (hex, `end` exclusive; `#` starts a comment).
Output: `0xADDRESS 0xVALUE` per 32-bit little-endian word, grouped under `# name` headers;
`0xADDRESS ERR` when the server refuses a word (E.. reply or empty reply).
"""
import argparse
import socket
import sys
import time

MAX_CHUNK = 256  # bytes per `m` request


class Rsp:
    def __init__(self, host, port, timeout=5.0, verbose=False):
        self.sock = socket.create_connection((host, port), timeout=timeout)
        self.sock.settimeout(timeout)
        self.verbose = verbose
        self.buf = b""
        self.no_ack = False

    @staticmethod
    def checksum(data):
        return sum(data) & 0xFF

    def send(self, payload):
        if isinstance(payload, str):
            payload = payload.encode()
        pkt = b"$" + payload + b"#" + b"%02x" % self.checksum(payload)
        for attempt in range(5):
            if self.verbose:
                print("->", pkt.decode(), file=sys.stderr)
            self.sock.sendall(pkt)
            if self.no_ack:
                return
            c = self._recv_byte()
            while c == b"+":        # stray acks from an earlier exchange
                c = self._recv_byte()
            if c == b"+":
                return
            if c == b"-":
                continue
            if c == b"$":           # server skipped the ack and replied directly
                self.buf = c + self.buf
                return
            if c == b"":
                raise IOError("connection closed while waiting for ack")
        raise IOError("packet not acknowledged after 5 attempts")

    def _recv_byte(self):
        if not self.buf:
            try:
                self.buf = self.sock.recv(4096)
            except socket.timeout:
                return b""
            if not self.buf:
                raise IOError("connection closed")
        c, self.buf = self.buf[:1], self.buf[1:]
        return c

    def recv(self):
        # wait for '$'
        while True:
            c = self._recv_byte()
            if c == b"$":
                break
            if c == b"":
                return None  # timeout
        data = b""
        while True:
            c = self._recv_byte()
            if c == b"":
                return None
            if c == b"#":
                break
            data += c
        cs = self._recv_byte() + self._recv_byte()
        ok = int(cs, 16) == self.checksum(data)
        if not self.no_ack:
            self.sock.sendall(b"+" if ok else b"-")
        if self.verbose:
            print("<-", data.decode(errors="replace"), file=sys.stderr)
        if not ok:
            return self.recv()
        # unescape run-length encoding and 0x7d escapes
        out = bytearray()
        i = 0
        while i < len(data):
            b = data[i]
            if b == 0x7D:
                out.append(data[i + 1] ^ 0x20)
                i += 2
            elif b == 0x2A and out:  # '*' run-length
                n = data[i + 1] - 29
                out.extend(bytes([out[-1]]) * n)
                i += 2
            else:
                out.append(b)
                i += 1
        return bytes(out)

    def cmd(self, payload):
        self.send(payload)
        return self.recv()

    def read_mem(self, addr, length):
        """Returns bytes, or None if the server replied with an error / nothing."""
        r = self.cmd("m%x,%x" % (addr, length))
        if r is None or r == b"" or r.startswith(b"E"):
            return None
        try:
            data = bytes.fromhex(r.decode())
        except ValueError:
            return None
        if len(data) != length:
            return None
        return data


def parse_ranges(path):
    ranges = []
    with open(path) as f:
        for line in f:
            line = line.split("#", 1)[0].strip()
            if not line:
                continue
            parts = line.split(None, 2)
            start = int(parts[0], 16)
            end = int(parts[1], 16)
            name = parts[2] if len(parts) > 2 else "%#010x-%#010x" % (start, end)
            ranges.append((start, end, name))
    return ranges


def dump_range(rsp, start, end, out, stats):
    out.write("# %s\n" % stats["name"])
    addr = start
    while addr < end:
        n = min(MAX_CHUNK, end - addr)
        n -= n % 4
        data = rsp.read_mem(addr, n)
        if data is None:
            # retry word by word so a single inaccessible word does not hide its neighbours
            for a in range(addr, addr + n, 4):
                w = rsp.read_mem(a, 4)
                if w is None:
                    out.write("0x%08X ERR\n" % a)
                    stats["err"] += 1
                else:
                    out.write("0x%08X 0x%08X\n" % (a, int.from_bytes(w, "little")))
                    stats["ok"] += 1
        else:
            for i in range(0, n, 4):
                out.write("0x%08X 0x%08X\n" % (addr + i, int.from_bytes(data[i:i + 4], "little")))
                stats["ok"] += 1
        addr += n


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=3333)
    ap.add_argument("--ranges", help="file with `start end [name]` lines (hex)")
    ap.add_argument("--out", help="output file (default stdout)")
    ap.add_argument("--regs", action="store_true", help="print the g packet and p0..p40 raw")
    ap.add_argument("--header", action="append", default=[], help="extra header comment line(s)")
    ap.add_argument("--timeout", type=float, default=10.0)
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()

    rsp = Rsp(args.host, args.port, timeout=args.timeout, verbose=args.verbose)
    sup = rsp.cmd("qSupported:multiprocess+;swbreak+;hwbreak+;vContSupported+")
    print("qSupported:", sup.decode(errors="replace") if sup else sup, file=sys.stderr)
    halt = rsp.cmd("?")
    print("?:", halt.decode(errors="replace") if halt else halt, file=sys.stderr)

    if args.regs:
        g = rsp.cmd("g")
        print("g:", g.decode(errors="replace") if g else g)
        for i in range(0, 41):
            p = rsp.cmd("p%x" % i)
            print("p%d: %s" % (i, p.decode(errors="replace") if p else p))
        if not args.ranges:
            return

    if not args.ranges:
        ap.error("--ranges or --regs required")

    out = open(args.out, "w") if args.out else sys.stdout
    out.write("# Memory dump through the GDB Remote Serial Protocol, %s\n" % time.strftime("%Y-%m-%d %H:%M:%S"))
    out.write("# qSupported: %s\n" % (sup.decode(errors="replace") if sup else "(none)"))
    out.write("# ?: %s\n" % (halt.decode(errors="replace") if halt else "(none)"))
    for h in args.header:
        out.write("# %s\n" % h)
    total = {"ok": 0, "err": 0}
    for start, end, name in parse_ranges(args.ranges):
        stats = {"name": name, "ok": 0, "err": 0}
        dump_range(rsp, start, end, out, stats)
        total["ok"] += stats["ok"]
        total["err"] += stats["err"]
        print("%-60s %5d words, %d errors" % (name, stats["ok"], stats["err"]), file=sys.stderr)
    if args.out:
        out.close()
    print("total: %d words, %d errors" % (total["ok"], total["err"]), file=sys.stderr)


if __name__ == "__main__":
    main()
