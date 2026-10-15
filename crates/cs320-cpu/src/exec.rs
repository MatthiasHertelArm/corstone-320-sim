//! Instruction execution (DDI 0553B chapter C2 "Instruction details").
//!
//! Timing is that of a Fast Model: one instruction per cycle; the cycle counts returned here
//! are kept for the few multi-cycle cases the FVP models (exception entry and return).

use crate::alu::*;
use crate::bus::Bus;
use crate::cpu::*;
use crate::decode::*;
use crate::fpu;
use crate::fpu::Rounding;

const BRANCH_CYCLES: u32 = 1;

/// FNC_RETURN value placed in LR by BLXNS.
const FNC_RETURN: u32 = 0xFEFF_FFFF;

impl Cpu {
    #[inline]
    pub(crate) fn reg(&self, n: u8) -> u32 {
        if n == 15 {
            self.pc.wrapping_add(4)
        } else {
            self.r[n as usize]
        }
    }

    #[inline]
    pub(crate) fn set_reg(&mut self, n: u8, v: u32) {
        if n == 13 {
            let v = v & !3;
            if v < self.cur_sp_limit() {
                self.stkof = true;
                return;
            }
            self.r[13] = v;
        } else {
            self.r[n as usize] = v;
        }
    }

    /// Stack limit check of a descending stack access (`new_sp` is the lowest address used).
    #[inline]
    pub(crate) fn sp_limit_check(&mut self, new_sp: u32) -> Result<(), Trap> {
        if new_sp < self.cur_sp_limit() {
            return Err(Trap::Usage(CFSR_STKOF));
        }
        Ok(())
    }

    #[inline]
    fn sflag(&self, s: SetFlags) -> bool {
        match s {
            SetFlags::No => false,
            SetFlags::Yes => true,
            SetFlags::OutsideIt => !self.in_it_block(),
        }
    }

    /// `BXWritePC`/`LoadWritePC`: interworking branch, exception return in handler mode,
    /// function return from Non-secure code (FNC_RETURN).
    #[inline]
    fn bx_write_pc(&mut self, v: u32) {
        if self.handler_mode() && v >> 24 == 0xFF {
            self.exc_return = Some(v);
        } else if self.secure && v >> 24 == 0xFE {
            self.fnc_return = Some(v);
        } else {
            self.t = v & 1 != 0;
            self.next_pc = v & !1;
        }
    }

    fn check_watch(&mut self, addr: u32, size: u32, write: bool) {
        if !self.c_debugen {
            return;
        }
        if self.dwt.match_data(addr, size, write).is_some() {
            self.watch_hit = Some((addr, write as u8));
            return;
        }
        for w in &self.watchpoints {
            let kind_ok = if write { w.kind & 2 != 0 } else { w.kind & 1 != 0 };
            if kind_ok && addr < w.addr.wrapping_add(w.len) && addr.wrapping_add(size) > w.addr {
                self.watch_hit = Some((w.addr, write as u8));
                return;
            }
        }
    }

    fn check_data_mpu(&self, addr: u32, write: bool, unpriv: bool) -> Result<(), Trap> {
        let mpu = &self.mpu[self.st()];
        if !mpu.enabled() {
            return Ok(());
        }
        if mpu.ctrl & 2 == 0 && self.execution_priority() < 0 {
            return Ok(());
        }
        if (0xE000_0000..0xE010_0000).contains(&addr) {
            return Ok(());
        }
        let privileged = self.privileged() && !unpriv;
        match mpu.check(addr, privileged, write, false) {
            Some(true) => Ok(()),
            Some(false) => Err(Trap::Mem(CFSR_DACCVIOL, Some(addr))),
            None => {
                if mpu.ctrl & 4 != 0 && privileged {
                    Ok(())
                } else {
                    Err(Trap::Mem(CFSR_DACCVIOL, Some(addr)))
                }
            }
        }
    }

    /// Data read. `aligned` requires natural alignment (MemA); otherwise unaligned word and
    /// halfword accesses are allowed unless CCR.UNALIGN_TRP is set (MemU).
    pub(crate) fn mem_read<B: Bus>(&mut self, bus: &mut B, addr: u32, size: u8, aligned: bool, unpriv: bool) -> Result<u32, Trap> {
        if size > 1 && addr & (size as u32 - 1) != 0 {
            if aligned || self.ccr[self.st()] & 8 != 0 {
                return Err(Trap::Usage(CFSR_UNALIGNED));
            }
            let mut v = 0;
            for i in 0..size as u32 {
                v |= self.mem_read(bus, addr.wrapping_add(i), 1, true, unpriv)? << (8 * i);
            }
            return Ok(v);
        }
        self.check_data_mpu(addr, false, unpriv)?;
        if self.watch_active && self.c_debugen {
            self.check_watch(addr, size as u32, false);
        }
        if addr >> 20 == 0xE00 {
            if unpriv {
                return Err(Trap::Bus(CFSR_PRECISERR, Some(addr)));
            }
            return self.ppb_read(addr, size, false);
        }
        let ns = self.access_ns(bus, addr)?;
        bus.read(self.now, addr, size, ns, false).map_err(|_| Trap::Bus(CFSR_PRECISERR, Some(addr)))
    }

    pub(crate) fn mem_write<B: Bus>(&mut self, bus: &mut B, addr: u32, size: u8, v: u32, aligned: bool, unpriv: bool) -> Result<(), Trap> {
        if size > 1 && addr & (size as u32 - 1) != 0 {
            if aligned || self.ccr[self.st()] & 8 != 0 {
                return Err(Trap::Usage(CFSR_UNALIGNED));
            }
            for i in 0..size as u32 {
                self.mem_write(bus, addr.wrapping_add(i), 1, (v >> (8 * i)) & 0xFF, true, unpriv)?;
            }
            return Ok(());
        }
        self.check_data_mpu(addr, true, unpriv)?;
        if self.watch_active && self.c_debugen {
            self.check_watch(addr, size as u32, true);
        }
        if addr >> 20 == 0xE00 {
            if unpriv && (addr >= 0xE000_1000) {
                return Err(Trap::Bus(CFSR_PRECISERR, Some(addr)));
            }
            let r = self.ppb_write(addr, size, v, false);
            if matches!(addr & !0x3F, 0xE000_1000 | 0xE000_1040) {
                self.refresh_watch();
            }
            return r;
        }
        let ns = self.access_ns(bus, addr)?;
        self.invalidate_decode_page(addr);
        let res = bus.write(self.now, addr, size, v, ns, false);
        if let Some(t) = bus.take_stall() {
            self.now = self.now.max(cs320_core::time::ceil_cpu_cycle(t));
        }
        if res.is_err() {
            if self.actlr & 2 != 0 {
                return Err(Trap::Bus(CFSR_PRECISERR, Some(addr)));
            }
            // buffered write: imprecise BusFault
            self.bfsr |= (CFSR_IMPRECISERR >> 8) as u8;
            self.escalate_bus_fault();
        }
        Ok(())
    }

    fn escalate_bus_fault(&mut self) {
        let secure = !self.bfhfnmins;
        let slot = EXC_BUSFAULT;
        let ep = self.execution_priority();
        if self.shcsr_ena[S] & (1 << 17) != 0 && self.slot_effective_priority_pub(slot) < ep {
            self.set_pending(slot);
        } else {
            self.hfsr |= HFSR_FORCED;
            self.set_pending(EXC_HARDFAULT);
        }
        let _ = secure;
    }

    /// Debugger memory read (no side effects, no faults).
    pub fn debug_read<B: Bus>(&mut self, bus: &mut B, addr: u32, size: u8) -> Option<u32> {
        if addr >> 20 == 0xE00 {
            return self.ppb_read(addr, size, true).ok();
        }
        let ns = self.attribution(bus, addr).0 == false;
        bus.read(self.now, addr, size, ns, true).ok()
    }

    /// Debugger memory write.
    pub fn debug_write<B: Bus>(&mut self, bus: &mut B, addr: u32, size: u8, v: u32) -> bool {
        if addr >> 20 == 0xE00 {
            let ok = self.ppb_write(addr, size, v, true).is_ok();
            self.refresh_watch();
            return ok;
        }
        self.invalidate_decode_page(addr);
        let ns = self.attribution(bus, addr).0 == false;
        bus.write(self.now, addr, size, v, ns, true).is_ok()
    }

    fn ls_address(&self, rn: u8, addr: Addr) -> (u32, Option<u32>) {
        match addr {
            Addr::Imm { imm, add, index, wback } => {
                let base = self.reg(rn);
                let off = if add { base.wrapping_add(imm) } else { base.wrapping_sub(imm) };
                (if index { off } else { base }, if wback { Some(off) } else { None })
            }
            Addr::Reg { rm, shift } => (self.reg(rn).wrapping_add(self.reg(rm) << shift), None),
            Addr::Literal { imm, add } => {
                let base = self.pc.wrapping_add(4) & !3;
                (if add { base.wrapping_add(imm) } else { base.wrapping_sub(imm) }, None)
            }
        }
    }

    fn dp(&mut self, op: DpOp, s: bool, rd: u8, a: u32, b: u32, shift_carry: bool) -> u32 {
        use DpOp::*;
        let (r, c, v, logical) = match op {
            And | Tst => (a & b, shift_carry, false, true),
            Bic => (a & !b, shift_carry, false, true),
            Orr => (a | b, shift_carry, false, true),
            Orn => (a | !b, shift_carry, false, true),
            Eor | Teq => (a ^ b, shift_carry, false, true),
            Mov => (b, shift_carry, false, true),
            Mvn => (!b, shift_carry, false, true),
            Add | Cmn => {
                let (r, c, v) = add_with_carry(a, b, false);
                (r, c, v, false)
            }
            Adc => {
                let (r, c, v) = add_with_carry(a, b, self.c);
                (r, c, v, false)
            }
            Sub | Cmp => {
                let (r, c, v) = add_with_carry(a, !b, true);
                (r, c, v, false)
            }
            Sbc => {
                let (r, c, v) = add_with_carry(a, !b, self.c);
                (r, c, v, false)
            }
            Rsb => {
                let (r, c, v) = add_with_carry(b, !a, true);
                (r, c, v, false)
            }
        };
        if s {
            self.n = r >> 31 != 0;
            self.z = r == 0;
            self.c = c;
            if !logical {
                self.v = v;
            }
        }
        if matches!(op, Tst | Teq | Cmp | Cmn) {
            return 1;
        }
        if rd == 15 {
            self.next_pc = r & !1;
            return BRANCH_CYCLES;
        }
        self.set_reg(rd, r);
        1
    }

    fn set_nz(&mut self, r: u32) {
        self.n = r >> 31 != 0;
        self.z = r == 0;
    }

    /// Executes a decoded instruction whose condition passed. Returns the cycle count.
    pub(crate) fn execute<B: Bus>(&mut self, bus: &mut B, ins: Instr) -> Result<u32, Trap> {
        use Instr::*;
        match ins {
            Undefined => Err(Trap::Usage(CFSR_UNDEFINSTR)),
            NoCoproc => Err(Trap::Usage(CFSR_NOCP)),
            Mve(enc) => self.exec_mve(bus, enc),
            DpImm { op, s, rd, rn, imm, carry } => {
                let a = if matches!(op, DpOp::Mov | DpOp::Mvn) { 0 } else { self.reg(rn) };
                let c = match carry {
                    ImmCarry::Keep => self.c,
                    ImmCarry::Clear => false,
                    ImmCarry::Set => true,
                };
                let s = self.sflag(s);
                Ok(self.dp(op, s, rd, a, imm, c))
            }
            DpReg { op, s, rd, rn, rm, shift } => {
                let (b, c) = shift_c(self.reg(rm), shift.ty, shift.n as u32, self.c);
                let a = if matches!(op, DpOp::Mov | DpOp::Mvn) { 0 } else { self.reg(rn) };
                let s = self.sflag(s);
                Ok(self.dp(op, s, rd, a, b, c))
            }
            ShiftReg { s, ty, rd, rn, rm } => {
                let (r, c) = shift_c(self.reg(rn), ty, self.reg(rm) & 0xFF, self.c);
                if self.sflag(s) {
                    self.set_nz(r);
                    self.c = c;
                }
                self.set_reg(rd, r);
                Ok(1)
            }
            Adr { rd, imm, add } => {
                let base = self.pc.wrapping_add(4) & !3;
                self.set_reg(rd, if add { base.wrapping_add(imm) } else { base.wrapping_sub(imm) });
                Ok(1)
            }
            MovT { rd, imm } => {
                let v = ((imm as u32) << 16) | (self.reg(rd) & 0xFFFF);
                self.set_reg(rd, v);
                Ok(1)
            }
            Mul { s, rd, rn, rm } => {
                let r = self.reg(rn).wrapping_mul(self.reg(rm));
                if self.sflag(s) {
                    self.set_nz(r);
                }
                self.set_reg(rd, r);
                Ok(1)
            }
            Mla { sub, rd, rn, rm, ra } => {
                let p = self.reg(rn).wrapping_mul(self.reg(rm));
                let a = self.reg(ra);
                self.set_reg(rd, if sub { a.wrapping_sub(p) } else { a.wrapping_add(p) });
                Ok(2)
            }
            LongMul { op, rdlo, rdhi, rn, rm } => {
                let (a, b) = (self.reg(rn), self.reg(rm));
                let acc = ((self.reg(rdhi) as u64) << 32) | self.reg(rdlo) as u64;
                let r: u64 = match op {
                    self::LongMul::Smull => ((a as i32 as i64) * (b as i32 as i64)) as u64,
                    self::LongMul::Umull => (a as u64) * (b as u64),
                    self::LongMul::Smlal => (((a as i32 as i64) * (b as i32 as i64)) as u64).wrapping_add(acc),
                    self::LongMul::Umlal => ((a as u64) * (b as u64)).wrapping_add(acc),
                    self::LongMul::Umaal => (a as u64) * (b as u64) + self.reg(rdhi) as u64 + self.reg(rdlo) as u64,
                };
                self.set_reg(rdlo, r as u32);
                self.set_reg(rdhi, (r >> 32) as u32);
                Ok(1)
            }
            Div { signed, rd, rn, rm } => {
                let (a, b) = (self.reg(rn), self.reg(rm));
                if b == 0 {
                    if self.ccr[self.st()] & (1 << 4) != 0 {
                        return Err(Trap::Usage(CFSR_DIVBYZERO));
                    }
                    self.set_reg(rd, 0);
                    return Ok(2);
                }
                let r = if signed { (a as i32).wrapping_div(b as i32) as u32 } else { a / b };
                self.set_reg(rd, r);
                let bits = (32 - a.leading_zeros()).saturating_sub(32 - b.leading_zeros());
                Ok(2 + (bits / 4).min(10))
            }
            SmlaXy { rd, rn, rm, ra, nh, mh } => {
                let h = |v: u32, hi: bool| if hi { (v >> 16) as i16 as i32 } else { v as i16 as i32 };
                let p = h(self.reg(rn), nh) * h(self.reg(rm), mh);
                let r = if ra == 15 {
                    p
                } else {
                    let full = p as i64 + self.reg(ra) as i32 as i64;
                    if full != full as i32 as i64 {
                        self.q = true;
                    }
                    full as i32
                };
                self.set_reg(rd, r as u32);
                Ok(1)
            }
            SmlaWy { rd, rn, rm, ra, mh } => {
                let m = if mh { (self.reg(rm) >> 16) as i16 as i64 } else { self.reg(rm) as i16 as i64 };
                let p = ((self.reg(rn) as i32 as i64) * m) >> 16;
                let r = if ra == 15 {
                    p as i32
                } else {
                    let full = p + self.reg(ra) as i32 as i64;
                    if full != full as i32 as i64 {
                        self.q = true;
                    }
                    full as i32
                };
                self.set_reg(rd, r as u32);
                Ok(1)
            }
            Smlad { sub, x, rd, rn, rm, ra } => {
                let a = self.reg(rn);
                let b = if x { self.reg(rm).rotate_right(16) } else { self.reg(rm) };
                let p1 = (a as i16 as i64) * (b as i16 as i64);
                let p2 = ((a >> 16) as i16 as i64) * ((b >> 16) as i16 as i64);
                let mut sum = if sub { p1 - p2 } else { p1 + p2 };
                if ra != 15 {
                    sum += self.reg(ra) as i32 as i64;
                }
                if sum != sum as i32 as i64 {
                    self.q = true;
                }
                self.set_reg(rd, sum as i32 as u32);
                Ok(1)
            }
            Smlald { sub, x, rdlo, rdhi, rn, rm } => {
                let a = self.reg(rn);
                let b = if x { self.reg(rm).rotate_right(16) } else { self.reg(rm) };
                let p1 = (a as i16 as i64) * (b as i16 as i64);
                let p2 = ((a >> 16) as i16 as i64) * ((b >> 16) as i16 as i64);
                let acc = (((self.reg(rdhi) as u64) << 32) | self.reg(rdlo) as u64) as i64;
                let r = acc.wrapping_add(if sub { p1 - p2 } else { p1 + p2 }) as u64;
                self.set_reg(rdlo, r as u32);
                self.set_reg(rdhi, (r >> 32) as u32);
                Ok(1)
            }
            SmlalXy { rdlo, rdhi, rn, rm, nh, mh } => {
                let h = |v: u32, hi: bool| if hi { (v >> 16) as i16 as i64 } else { v as i16 as i64 };
                let p = h(self.reg(rn), nh) * h(self.reg(rm), mh);
                let acc = (((self.reg(rdhi) as u64) << 32) | self.reg(rdlo) as u64) as i64;
                let r = acc.wrapping_add(p) as u64;
                self.set_reg(rdlo, r as u32);
                self.set_reg(rdhi, (r >> 32) as u32);
                Ok(1)
            }
            Smmla { sub, round, rd, rn, rm, ra } => {
                let p = (self.reg(rn) as i32 as i64) * (self.reg(rm) as i32 as i64);
                let acc = if ra == 15 { 0 } else { (self.reg(ra) as i32 as i64) << 32 };
                let mut r = if sub { acc.wrapping_sub(p) } else { acc.wrapping_add(p) };
                if round {
                    r = r.wrapping_add(0x8000_0000);
                }
                self.set_reg(rd, (r >> 32) as u32);
                Ok(1)
            }
            Usada8 { rd, rn, rm, ra } => {
                let (a, b) = (self.reg(rn), self.reg(rm));
                let mut sum: u32 = (0..4).map(|i| (((a >> (8 * i)) & 0xFF) as i32 - ((b >> (8 * i)) & 0xFF) as i32).unsigned_abs()).sum();
                if ra != 15 {
                    sum = sum.wrapping_add(self.reg(ra));
                }
                self.set_reg(rd, sum);
                Ok(1)
            }
            Parallel { op, kind, rd, rn, rm } => {
                let r = self.parallel(op, kind, self.reg(rn), self.reg(rm));
                self.set_reg(rd, r);
                Ok(1)
            }
            Sat { signed, rd, rn, sat, shift } => {
                let operand = crate::alu::shift(self.reg(rn), shift.ty, shift.n as u32, self.c) as i32 as i64;
                let (r, sat_hit) = if signed {
                    let (r, s) = signed_sat_q(operand, sat as u32);
                    (r as u32, s)
                } else {
                    unsigned_sat_q(operand, sat as u32)
                };
                if sat_hit {
                    self.q = true;
                }
                self.set_reg(rd, r);
                Ok(1)
            }
            Sat16 { signed, rd, rn, sat } => {
                let v = self.reg(rn);
                let mut r = 0u32;
                for i in 0..2 {
                    let h = ((v >> (16 * i)) as i16) as i64;
                    let (x, s) = if signed {
                        let (x, s) = signed_sat_q(h, sat as u32);
                        (x as u32, s)
                    } else {
                        unsigned_sat_q(h, sat as u32)
                    };
                    if s {
                        self.q = true;
                    }
                    r |= (x & 0xFFFF) << (16 * i);
                }
                self.set_reg(rd, r);
                Ok(1)
            }
            Qop { op, rd, rn, rm } => {
                let m = self.reg(rm) as i32 as i64;
                let n = self.reg(rn) as i32 as i64;
                let mut sat = false;
                let mut s32 = |x: i64| {
                    let (r, s) = signed_sat_q(x, 32);
                    sat |= s;
                    r as i64
                };
                let r = match op {
                    QOp::Qadd => s32(m + n),
                    QOp::Qsub => s32(m - n),
                    QOp::Qdadd => {
                        let d = s32(2 * n);
                        s32(m + d)
                    }
                    QOp::Qdsub => {
                        let d = s32(2 * n);
                        s32(m - d)
                    }
                };
                if sat {
                    self.q = true;
                }
                self.set_reg(rd, r as i32 as u32);
                Ok(1)
            }
            Sel { rd, rn, rm } => {
                let (a, b) = (self.reg(rn), self.reg(rm));
                let mut r = 0;
                for i in 0..4 {
                    let src = if self.ge & (1 << i) != 0 { a } else { b };
                    r |= src & (0xFF << (8 * i));
                }
                self.set_reg(rd, r);
                Ok(1)
            }
            Clz { rd, rm } => {
                let v = self.reg(rm).leading_zeros();
                self.set_reg(rd, v);
                Ok(1)
            }
            Rbit { rd, rm } => {
                let v = self.reg(rm).reverse_bits();
                self.set_reg(rd, v);
                Ok(1)
            }
            Rev { rd, rm } => {
                let v = self.reg(rm).swap_bytes();
                self.set_reg(rd, v);
                Ok(1)
            }
            Rev16 { rd, rm } => {
                let v = self.reg(rm);
                self.set_reg(rd, ((v & 0x00FF_00FF) << 8) | ((v & 0xFF00_FF00) >> 8));
                Ok(1)
            }
            Revsh { rd, rm } => {
                let v = self.reg(rm);
                self.set_reg(rd, ((v as u16).swap_bytes() as i16) as i32 as u32);
                Ok(1)
            }
            Extend { signed, bits, rd, rn, rm, rot } => {
                let v = self.reg(rm).rotate_right(rot as u32);
                let r = match bits {
                    8 => {
                        let x = if signed { v as i8 as i32 as u32 } else { v & 0xFF };
                        if rn == 15 {
                            x
                        } else {
                            self.reg(rn).wrapping_add(x)
                        }
                    }
                    16 => {
                        let x = if signed { v as i16 as i32 as u32 } else { v & 0xFFFF };
                        if rn == 15 {
                            x
                        } else {
                            self.reg(rn).wrapping_add(x)
                        }
                    }
                    _ => {
                        let lane = |b: u32| if signed { (b as i8 as i16) as u16 as u32 } else { b & 0xFF };
                        let (lo, hi) = (lane(v), lane(v >> 16));
                        if rn == 15 {
                            lo | (hi << 16)
                        } else {
                            let a = self.reg(rn);
                            ((a as u16).wrapping_add(lo as u16) as u32) | (((a >> 16) as u16).wrapping_add(hi as u16) as u32) << 16
                        }
                    }
                };
                self.set_reg(rd, r);
                Ok(1)
            }
            Bfi { rd, rn, lsb, msb } => {
                if msb < lsb {
                    return Ok(1);
                }
                let width = (msb - lsb + 1) as u32;
                let mask = (if width == 32 { u32::MAX } else { (1u32 << width) - 1 }) << lsb;
                let src = if rn == 15 { 0 } else { self.reg(rn) };
                let v = (self.reg(rd) & !mask) | ((src << lsb) & mask);
                self.set_reg(rd, v);
                Ok(1)
            }
            Bfx { signed, rd, rn, lsb, widthm1 } => {
                let width = widthm1 as u32 + 1;
                if lsb as u32 + width > 32 {
                    return Ok(1);
                }
                let v = self.reg(rn) >> lsb;
                let r = if width == 32 {
                    v
                } else if signed {
                    let s = 32 - width;
                    (((v << s) as i32) >> s) as u32
                } else {
                    v & ((1 << width) - 1)
                };
                self.set_reg(rd, r);
                Ok(1)
            }
            Pkh { tb, rd, rn, rm, shift } => {
                let op2 = crate::alu::shift(self.reg(rm), shift.ty, shift.n as u32, self.c);
                let n = self.reg(rn);
                let r = if tb { (n & 0xFFFF_0000) | (op2 & 0xFFFF) } else { (n & 0xFFFF) | (op2 & 0xFFFF_0000) };
                self.set_reg(rd, r);
                Ok(1)
            }
            B { cond, imm } => {
                if cond != 14 && !condition_passed(cond, self.n, self.z, self.c, self.v) {
                    return Ok(1);
                }
                self.next_pc = self.pc.wrapping_add(4).wrapping_add(imm as u32);
                Ok(BRANCH_CYCLES)
            }
            Bl { imm } => {
                self.r[14] = self.next_pc | 1;
                self.next_pc = self.pc.wrapping_add(4).wrapping_add(imm as u32);
                Ok(BRANCH_CYCLES + 1)
            }
            Bx { rm } => {
                let t = self.reg(rm);
                self.bx_write_pc(t);
                Ok(BRANCH_CYCLES)
            }
            Blx { rm } => {
                let target = self.reg(rm);
                self.r[14] = self.pc.wrapping_add(2) | 1;
                self.t = target & 1 != 0;
                self.next_pc = target & !1;
                Ok(BRANCH_CYCLES)
            }
            Cbz { nonzero, rn, imm } => {
                if (self.reg(rn) != 0) == nonzero {
                    self.next_pc = self.pc.wrapping_add(4).wrapping_add(imm);
                    Ok(BRANCH_CYCLES)
                } else {
                    Ok(1)
                }
            }
            Tb { half, rn, rm } => {
                let base = self.reg(rn);
                let idx = self.reg(rm);
                let off = if half {
                    self.mem_read(bus, base.wrapping_add(idx << 1), 2, false, false)?
                } else {
                    self.mem_read(bus, base.wrapping_add(idx), 1, false, false)?
                };
                self.next_pc = self.pc.wrapping_add(4).wrapping_add(2 * off);
                Ok(2 + BRANCH_CYCLES)
            }
            Ldr { size, signed, unpriv, rt, rn, addr } => {
                let (a, wb) = self.ls_address(rn, addr);
                let mut v = self.mem_read(bus, a, size, false, unpriv)?;
                if signed {
                    v = match size {
                        1 => v as i8 as i32 as u32,
                        2 => v as i16 as i32 as u32,
                        _ => v,
                    };
                }
                if let Some(w) = wb {
                    self.set_reg(rn, w);
                }
                if rt == 15 {
                    if a & 3 != 0 {
                        return Err(Trap::Usage(CFSR_UNALIGNED));
                    }
                    self.bx_write_pc(v);
                    return Ok(2 + BRANCH_CYCLES);
                }
                self.set_reg(rt, v);
                Ok(2)
            }
            Str { size, unpriv, rt, rn, addr } => {
                let (a, wb) = self.ls_address(rn, addr);
                if rn == 13 {
                    if let Some(w) = wb {
                        self.sp_limit_check(w.min(a))?;
                    }
                }
                let v = self.reg(rt);
                self.mem_write(bus, a, size, v, false, unpriv)?;
                if let Some(w) = wb {
                    self.set_reg(rn, w);
                }
                Ok(2)
            }
            Ldrd { rt, rt2, rn, imm, add, index, wback } => {
                let base = if rn == 15 { self.pc.wrapping_add(4) & !3 } else { self.reg(rn) };
                let off = if add { base.wrapping_add(imm) } else { base.wrapping_sub(imm) };
                let a = if index { off } else { base };
                let v1 = self.mem_read(bus, a, 4, true, false)?;
                let v2 = self.mem_read(bus, a.wrapping_add(4), 4, true, false)?;
                if wback {
                    self.set_reg(rn, off);
                }
                self.set_reg(rt, v1);
                self.set_reg(rt2, v2);
                Ok(3)
            }
            Strd { rt, rt2, rn, imm, add, index, wback } => {
                let base = self.reg(rn);
                let off = if add { base.wrapping_add(imm) } else { base.wrapping_sub(imm) };
                let a = if index { off } else { base };
                if rn == 13 && wback {
                    self.sp_limit_check(off.min(a))?;
                }
                let (v1, v2) = (self.reg(rt), self.reg(rt2));
                self.mem_write(bus, a, 4, v1, true, false)?;
                self.mem_write(bus, a.wrapping_add(4), 4, v2, true, false)?;
                if wback {
                    self.set_reg(rn, off);
                }
                Ok(3)
            }
            Ldm { rn, regs, wback, db } => {
                let base = self.reg(rn);
                let count = regs.count_ones();
                let start = if db { base.wrapping_sub(4 * count) } else { base };
                let mut vals = [0u32; 16];
                let mut a = start;
                for (i, val) in vals.iter_mut().enumerate() {
                    if regs & (1 << i) != 0 {
                        *val = self.mem_read(bus, a, 4, true, false)?;
                        a = a.wrapping_add(4);
                    }
                }
                if wback {
                    let nb = if db { base.wrapping_sub(4 * count) } else { base.wrapping_add(4 * count) };
                    self.set_reg(rn, nb);
                }
                for (i, v) in vals.iter().enumerate().take(15) {
                    if regs & (1 << i) != 0 {
                        self.set_reg(i as u8, *v);
                    }
                }
                if regs & 0x8000 != 0 {
                    self.bx_write_pc(vals[15]);
                    return Ok(1 + count + BRANCH_CYCLES - 1);
                }
                Ok(1 + count)
            }
            Stm { rn, regs, wback, db } => {
                let base = self.reg(rn);
                let count = regs.count_ones();
                let start = if db { base.wrapping_sub(4 * count) } else { base };
                if rn == 13 && db && wback {
                    self.sp_limit_check(start)?;
                }
                let mut a = start;
                for i in 0..16u8 {
                    if regs & (1 << i) != 0 {
                        let v = self.reg(i);
                        self.mem_write(bus, a, 4, v, true, false)?;
                        a = a.wrapping_add(4);
                    }
                }
                if wback {
                    let nb = if db { base.wrapping_sub(4 * count) } else { base.wrapping_add(4 * count) };
                    self.set_reg(rn, nb);
                }
                Ok(1 + count)
            }
            Ldrex { size, rt, rn, imm } => {
                let a = self.reg(rn).wrapping_add(imm);
                let v = self.mem_read(bus, a, size, true, false)?;
                self.excl = true;
                self.set_reg(rt, v);
                Ok(2)
            }
            Strex { size, rd, rt, rn, imm } => {
                let a = self.reg(rn).wrapping_add(imm);
                if size > 1 && a & (size as u32 - 1) != 0 {
                    return Err(Trap::Usage(CFSR_UNALIGNED));
                }
                if self.excl {
                    let v = self.reg(rt);
                    self.mem_write(bus, a, size, v, true, false)?;
                    self.set_reg(rd, 0);
                } else {
                    self.set_reg(rd, 1);
                }
                self.excl = false;
                Ok(2)
            }
            Clrex => {
                self.excl = false;
                Ok(1)
            }
            MemHint => Ok(1),
            Mrs { rd, sysm } => {
                let v = self.mrs(sysm)?;
                self.set_reg(rd, v);
                Ok(1)
            }
            Msr { rn, sysm, mask } => {
                let v = self.reg(rn);
                self.msr(sysm, mask, v)?;
                Ok(1)
            }
            Cps { disable, i, f } => {
                if self.privileged() {
                    if disable {
                        if i {
                            self.set_primask(true);
                        }
                        if f && self.execution_priority() > -1 {
                            self.set_faultmask(true);
                        }
                    } else {
                        if i {
                            self.set_primask(false);
                        }
                        if f {
                            self.set_faultmask(false);
                        }
                    }
                }
                Ok(1)
            }
            Barrier(_) => Ok(1),
            Hint(h) => {
                match h {
                    self::Hint::Nop | self::Hint::Yield => {}
                    self::Hint::Wfi => {
                        self.sleeping = true;
                        self.sleep_wfe = false;
                    }
                    self::Hint::Wfe => {
                        if self.event_reg {
                            self.event_reg = false;
                        } else {
                            self.sleeping = true;
                            self.sleep_wfe = true;
                        }
                    }
                    self::Hint::Sev => self.event_reg = true,
                }
                Ok(1)
            }
            It { firstcond, mask } => {
                self.it = (firstcond << 4) | mask;
                Ok(1)
            }
            Bkpt { imm } => Err(Trap::Bkpt(imm)),
            Svc { .. } => Err(Trap::Svc),
            Vfp(v) => self.exec_vfp(bus, v),
            Csel { op, rd, rn, rm, cond } => {
                let n = if rn == 15 { 0 } else { self.reg(rn) };
                let m = if rm == 15 { 0 } else { self.reg(rm) };
                let r = if condition_passed(cond, self.n, self.z, self.c, self.v) {
                    n
                } else {
                    match op {
                        0 => m,
                        1 => m.wrapping_add(1),
                        2 => !m,
                        _ => m.wrapping_neg(),
                    }
                };
                self.set_reg(rd, r);
                Ok(1)
            }
            Tt { rd, rn, alt, unpriv } => {
                let addr = self.reg(rn);
                let v = self.test_target(bus, addr, alt, unpriv);
                self.set_reg(rd, v);
                Ok(1)
            }
            Sg => {
                if !self.secure {
                    // entry from Non-secure code through a Secure gateway
                    self.r[14] &= !1;
                    let ipsr = self.ipsr;
                    let spsel = self.spsel[S];
                    self.set_stack_state(ipsr, true, spsel);
                    self.exc_dirty = true;
                }
                Ok(1)
            }
            Bxns { rm } => {
                let target = self.reg(rm);
                if !self.secure {
                    return Err(Trap::Usage(CFSR_UNDEFINSTR));
                }
                if self.handler_mode() && target >> 24 == 0xFF {
                    self.exc_return = Some(target);
                } else if target >> 24 == 0xFE {
                    self.fnc_return = Some(target);
                } else {
                    self.t = true;
                    self.next_pc = target & !1;
                    if target & 1 == 0 {
                        self.switch_to_nonsecure();
                    }
                }
                Ok(1)
            }
            Blxns { rm } => {
                let target = self.reg(rm);
                if !self.secure {
                    return Err(Trap::Usage(CFSR_UNDEFINSTR));
                }
                if target & 1 == 0 {
                    // push the return address and partial xPSR on the Secure stack
                    let sp = self.r[13].wrapping_sub(8);
                    self.sp_limit_check(sp)?;
                    let ret = self.pc.wrapping_add(2) | 1;
                    let psr = self.xpsr() & 0x1FF | ((self.handler_mode() as u32) << 9);
                    self.mem_write(bus, sp, 4, ret, true, false)?;
                    self.mem_write(bus, sp + 4, 4, psr, true, false)?;
                    self.r[13] = sp;
                    self.r[14] = FNC_RETURN & !1 | (self.spsel[S] as u32 & 0) | 1;
                    self.t = true;
                    self.next_pc = target & !1;
                    self.switch_to_nonsecure();
                } else {
                    self.r[14] = self.pc.wrapping_add(2) | 1;
                    self.t = true;
                    self.next_pc = target & !1;
                }
                Ok(1)
            }
            Clrm { regs } => {
                for i in 0..13u8 {
                    if regs & (1 << i) != 0 {
                        self.r[i as usize] = 0;
                    }
                }
                if regs & (1 << 14) != 0 {
                    self.r[14] = 0;
                }
                if regs & (1 << 15) != 0 {
                    self.set_apsr_nzcvq(0);
                    self.ge = 0;
                }
                Ok(1)
            }
            Pacg { rd, rn, rm } => {
                let v = self.pac(self.reg(rn), self.reg(rm));
                self.set_reg(rd, v);
                Ok(1)
            }
            Autg { ra, rn, rm } => {
                if self.pac(self.reg(rn), self.reg(rm)) != self.reg(ra) {
                    return Err(Trap::Usage(CFSR_INVSTATE));
                }
                Ok(1)
            }
            Bxaut { ra, rn, rm } => {
                if self.pac(self.reg(rn), self.reg(rm)) != self.reg(ra) {
                    return Err(Trap::Usage(CFSR_INVSTATE));
                }
                let t = self.reg(rn);
                self.bx_write_pc(t);
                Ok(1)
            }
            LoopStart { rn, imm, while_, tp } => {
                let count = self.reg(rn);
                self.lo_branch_valid = false;
                if tp != 0 {
                    self.fp_check(bus)?;
                    self.set_ltpsize(tp);
                }
                self.r[14] = count;
                if while_ && count == 0 {
                    self.next_pc = self.pc.wrapping_add(4).wrapping_add(imm);
                    if tp != 0 {
                        self.set_ltpsize(16);
                    }
                }
                Ok(1)
            }
            LoopEnd { lr, tp, imm } => {
                if tp {
                    self.fp_check(bus)?;
                    let size = self.ltpsize();
                    let elems = 16 / size.max(1);
                    let count = self.r[14];
                    if count > elems {
                        self.r[14] = count - elems;
                        self.next_pc = self.pc.wrapping_add(4).wrapping_sub(imm);
                    } else {
                        self.r[14] = 0;
                        self.set_ltpsize(16);
                    }
                } else if lr {
                    let count = self.r[14];
                    if count > 1 {
                        self.r[14] = count - 1;
                        self.next_pc = self.pc.wrapping_add(4).wrapping_sub(imm);
                    } else {
                        self.r[14] = 0;
                    }
                } else {
                    // LE without LR: a cached loop end that always branches
                    self.next_pc = self.pc.wrapping_add(4).wrapping_sub(imm);
                }
                Ok(1)
            }
            Lctp => {
                self.fp_check(bus)?;
                self.set_ltpsize(16);
                Ok(1)
            }
            Vctp { size, rn } => {
                self.fp_check(bus)?;
                let n = self.reg(rn) as u64;
                let lanes = (16 / size as u64).min(n);
                let bytes = (lanes * size as u64) as u32;
                let p0 = if bytes >= 16 { 0xFFFF } else { (1u32 << bytes) - 1 };
                self.vpr = (self.vpr & 0xFFFF_0000) | p0;
                Ok(1)
            }
        }
    }

    /// FPSCR.LTPSIZE: the element size of the active tail-predicated loop (16 = none).
    pub(crate) fn ltpsize(&self) -> u32 {
        1 << ((self.fpscr >> 16) & 7)
    }

    pub(crate) fn set_ltpsize(&mut self, size: u8) {
        let f = match size {
            1 => 0,
            2 => 1,
            4 => 2,
            8 => 3,
            _ => 4,
        };
        self.fpscr = (self.fpscr & !(7 << 16)) | (f << 16);
    }

    /// A 32-bit pointer authentication code (the key registers are rarely set; the function
    /// only has to be deterministic and depend on the address, the modifier and the key).
    fn pac(&self, addr: u32, modifier: u32) -> u32 {
        let key = self.pac_key[0] ^ self.pac_key[1].rotate_left(7) ^ self.pac_key[2].rotate_left(13) ^ self.pac_key[3].rotate_left(23);
        let mut h = addr ^ modifier.rotate_left(16) ^ key ^ 0x9E37_79B9;
        for _ in 0..3 {
            h ^= h >> 16;
            h = h.wrapping_mul(0x85EB_CA6B);
            h ^= h >> 13;
            h = h.wrapping_mul(0xC2B2_AE35);
        }
        h
    }

    /// Switches to Non-secure state (BXNS, BLXNS).
    fn switch_to_nonsecure(&mut self) {
        let ipsr = self.ipsr;
        let spsel = self.spsel[NS];
        self.set_stack_state(ipsr, false, spsel);
        self.exc_dirty = true;
    }

    /// `FunctionReturn`: pops the return address and partial xPSR pushed by BLXNS.
    pub(crate) fn function_return<B: Bus>(&mut self, bus: &mut B, v: u32) {
        let _ = v;
        let sp = self.r[13];
        let ret = match self.mem_read(bus, sp, 4, true, false) {
            Ok(x) => x,
            Err(t) => {
                let pc = self.pc;
                self.raise_fault(bus, t, pc);
                return;
            }
        };
        let psr = self.mem_read(bus, sp + 4, 4, true, false).unwrap_or(0);
        if (psr & 0x1FF) != (self.ipsr & 0x1FF) {
            let pc = self.pc;
            self.raise_fault(bus, Trap::Secure(SFSR_INVER, None), pc);
            return;
        }
        self.r[13] = sp + 8;
        self.pc = ret & !1;
        self.t = true;
        self.switch_to_nonsecure();
    }

    /// TT, TTT, TTA, TTAT.
    fn test_target<B: Bus>(&self, bus: &B, addr: u32, alt: bool, unpriv: bool) -> u32 {
        let st = if alt { NS } else { self.st() };
        let mpu = &self.mpu[st];
        let mut v = 0u32;
        let privileged = !unpriv && (self.handler_mode() || !self.npriv[st]);
        if mpu.enabled() {
            let mut hit: Option<usize> = None;
            for i in 0..8 {
                if mpu.rlar[i] & 1 != 0 && addr >= mpu.rbar[i] & 0xFFFF_FFE0 && addr <= mpu.rlar[i] | 0x1F {
                    hit = if hit.is_some() { None } else { Some(i) };
                    if hit.is_none() {
                        break;
                    }
                }
            }
            if let Some(i) = hit {
                v |= (i as u32) | (1 << 16);
            }
            match mpu.check(addr, privileged, false, false) {
                Some(true) => v |= 1 << 18,
                None if mpu.ctrl & 4 != 0 && privileged => v |= 1 << 18,
                _ => {}
            }
            match mpu.check(addr, privileged, true, false) {
                Some(true) => v |= 1 << 19,
                None if mpu.ctrl & 4 != 0 && privileged => v |= 1 << 19,
                _ => {}
            }
        } else {
            v |= (1 << 18) | (1 << 19);
        }
        if self.secure && !alt {
            let (s, _) = self.attribution(bus, addr);
            if s {
                v |= 1 << 22;
            }
            if self.sau.ctrl & 1 != 0 {
                for i in 0..8 {
                    if self.sau.rlar[i] & 1 != 0 && addr >= self.sau.rbar[i] & 0xFFFF_FFE0 && addr <= self.sau.rlar[i] | 0x1F {
                        v |= ((i as u32) << 8) | (1 << 17);
                        break;
                    }
                }
            }
            if !s {
                v |= (v & (3 << 18)) << 2;
            }
        }
        v
    }

    fn mrs(&mut self, sysm: u8) -> Result<u32, Trap> {
        let priv_ = self.privileged();
        Ok(match sysm {
            0..=7 => {
                let mut v = 0;
                if sysm & 1 != 0 {
                    v |= self.ipsr & 0x1FF;
                }
                if sysm & 4 == 0 {
                    v |= self.apsr();
                }
                v
            }
            8 if priv_ => self.msp(),
            9 if priv_ => self.psp(),
            10 if priv_ => self.msplim[self.st()],
            11 if priv_ => self.psplim[self.st()],
            16 => self.primask[self.st()] as u32,
            17 | 18 => self.basepri[self.st()] as u32,
            19 => self.faultmask[self.st()] as u32,
            20 => self.control(),
            0x20..=0x27 if self.secure && priv_ => self.pac_key[(sysm - 0x20) as usize & 3],
            0x88 if self.secure && priv_ => self.sp_of(false, false),
            0x89 if self.secure && priv_ => self.sp_of(false, true),
            0x8A if self.secure && priv_ => self.msplim[NS],
            0x8B if self.secure && priv_ => self.psplim[NS],
            0x90 if self.secure && priv_ => self.primask[NS] as u32,
            0x91 | 0x92 if self.secure && priv_ => self.basepri[NS] as u32,
            0x93 if self.secure && priv_ => self.faultmask[NS] as u32,
            0x94 if self.secure && priv_ => self.control_of(false),
            0x98 if self.secure && priv_ => self.sp_of(false, self.spsel[NS]),
            0xA0..=0xA7 if self.secure && priv_ => self.pac_key[(sysm - 0xA0) as usize & 3],
            _ => 0,
        })
    }

    fn msr(&mut self, sysm: u8, mask: u8, v: u32) -> Result<(), Trap> {
        if (0..=3).contains(&sysm) {
            if mask & 2 != 0 {
                self.set_apsr_nzcvq(v);
            }
            if mask & 1 != 0 {
                self.ge = ((v >> 16) & 0xF) as u8;
            }
            return Ok(());
        }
        if !self.privileged() {
            return Ok(());
        }
        let st = self.st();
        match sysm {
            8 => {
                self.set_msp(v);
                if (v & !3) < self.msplim[st] {
                    return Err(Trap::Usage(CFSR_STKOF));
                }
            }
            9 => {
                self.set_psp(v);
                if (v & !3) < self.psplim[st] {
                    return Err(Trap::Usage(CFSR_STKOF));
                }
            }
            10 => self.msplim[st] = v & !7,
            11 => self.psplim[st] = v & !7,
            16 => self.set_primask(v & 1 != 0),
            17 => self.set_basepri(v as u8),
            18 => {
                let nv = (v as u8) & 0xE0;
                let cur = self.basepri[st];
                if nv != 0 && (nv < cur || cur == 0) {
                    self.set_basepri(nv);
                }
            }
            19 if self.execution_priority() > -1 => self.set_faultmask(v & 1 != 0),
            20 => {
                let mut nv = v & 0xF;
                if self.handler_mode() {
                    nv = (nv & !2) | (self.spsel[st] as u32) << 1;
                }
                if !self.secure {
                    nv &= 7;
                }
                self.set_control(nv);
            }
            0x20..=0x27 if self.secure => self.pac_key[(sysm - 0x20) as usize & 3] = v,
            0x88 if self.secure => self.set_sp_of(false, false, v),
            0x89 if self.secure => self.set_sp_of(false, true, v),
            0x8A if self.secure => self.msplim[NS] = v & !7,
            0x8B if self.secure => self.psplim[NS] = v & !7,
            0x90 if self.secure => {
                self.primask[NS] = v & 1 != 0;
                self.exc_dirty = true;
            }
            0x91 if self.secure => {
                self.basepri[NS] = (v as u8) & 0xE0;
                self.exc_dirty = true;
            }
            0x92 if self.secure => {
                let nv = (v as u8) & 0xE0;
                let cur = self.basepri[NS];
                if nv != 0 && (nv < cur || cur == 0) {
                    self.basepri[NS] = nv;
                    self.exc_dirty = true;
                }
            }
            0x93 if self.secure => {
                self.faultmask[NS] = v & 1 != 0;
                self.exc_dirty = true;
            }
            0x94 if self.secure => self.set_control_of(false, v & 7),
            0x98 if self.secure => {
                let psp = self.spsel[NS];
                self.set_sp_of(false, psp, v);
            }
            0xA0..=0xA7 if self.secure => self.pac_key[(sysm - 0xA0) as usize & 3] = v,
            _ => {}
        }
        Ok(())
    }

    fn parallel(&mut self, op: ParOp, kind: ParKind, a: u32, b: u32) -> u32 {
        let signed = matches!(kind, ParKind::S | ParKind::Q | ParKind::Sh);
        let mut r = 0u32;
        let mut ge = 0u8;
        let lanes16 = matches!(op, ParOp::Add16 | ParOp::Sub16 | ParOp::Asx | ParOp::Sax);
        if lanes16 {
            let get = |v: u32, i: u32| -> i64 {
                let h = (v >> (16 * i)) & 0xFFFF;
                if signed {
                    h as u16 as i16 as i64
                } else {
                    h as i64
                }
            };
            let (a0, a1, b0, b1) = (get(a, 0), get(a, 1), get(b, 0), get(b, 1));
            let (r0, add0, r1, add1) = match op {
                ParOp::Add16 => (a0 + b0, true, a1 + b1, true),
                ParOp::Sub16 => (a0 - b0, false, a1 - b1, false),
                ParOp::Asx => (a0 - b1, false, a1 + b0, true),
                _ => (a0 + b1, true, a1 - b0, false),
            };
            for (i, (x, add)) in [(r0, add0), (r1, add1)].into_iter().enumerate() {
                let v: u32 = match kind {
                    ParKind::S => {
                        if x >= 0 {
                            ge |= 0b11 << (2 * i);
                        }
                        x as u32 & 0xFFFF
                    }
                    ParKind::U => {
                        if (add && x >= 0x1_0000) || (!add && x >= 0) {
                            ge |= 0b11 << (2 * i);
                        }
                        x as u32 & 0xFFFF
                    }
                    ParKind::Q => signed_sat_q(x, 16).0 as u32 & 0xFFFF,
                    ParKind::Uq => unsigned_sat_q(x, 16).0,
                    ParKind::Sh | ParKind::Uh => (x >> 1) as u32 & 0xFFFF,
                };
                r |= v << (16 * i);
            }
        } else {
            for i in 0..4 {
                let get = |v: u32| -> i64 {
                    let h = (v >> (8 * i)) & 0xFF;
                    if signed {
                        h as u8 as i8 as i64
                    } else {
                        h as i64
                    }
                };
                let add = op == ParOp::Add8;
                let x = if add { get(a) + get(b) } else { get(a) - get(b) };
                let v: u32 = match kind {
                    ParKind::S => {
                        if x >= 0 {
                            ge |= 1 << i;
                        }
                        x as u32 & 0xFF
                    }
                    ParKind::U => {
                        if (add && x >= 0x100) || (!add && x >= 0) {
                            ge |= 1 << i;
                        }
                        x as u32 & 0xFF
                    }
                    ParKind::Q => signed_sat_q(x, 8).0 as u32 & 0xFF,
                    ParKind::Uq => unsigned_sat_q(x, 8).0,
                    ParKind::Sh | ParKind::Uh => (x >> 1) as u32 & 0xFF,
                };
                r |= v << (8 * i);
            }
        }
        if matches!(kind, ParKind::S | ParKind::U) {
            self.ge = ge;
        }
        r
    }

    // ---------------------------------------------------------------- floating point

    /// Reads a floating-point register of the precision (`H`/`S`: S index, `D`: D index).
    #[inline]
    pub(crate) fn fp_get(&self, prec: Prec, i: u8) -> u64 {
        match prec {
            Prec::H => (self.s[i as usize & 31] & 0xFFFF) as u64,
            Prec::S => self.s[i as usize & 31] as u64,
            Prec::D => {
                let i = (i as usize & 15) * 2;
                self.s[i] as u64 | ((self.s[i + 1] as u64) << 32)
            }
        }
    }

    #[inline]
    pub(crate) fn fp_set(&mut self, prec: Prec, i: u8, v: u64) {
        match prec {
            Prec::H => self.s[i as usize & 31] = (v & 0xFFFF) as u32,
            Prec::S => self.s[i as usize & 31] = v as u32,
            Prec::D => {
                let i = (i as usize & 15) * 2;
                self.s[i] = v as u32;
                self.s[i + 1] = (v >> 32) as u32;
            }
        }
    }

    /// `ExecuteFPCheck()`: access check, lazy state preservation, FP context creation.
    pub(crate) fn fp_check<B: Bus>(&mut self, bus: &mut B) -> Result<(), Trap> {
        let st = self.st();
        let cp10 = (self.cpacr[st] >> 20) & 3;
        if cp10 == 0 || cp10 == 2 || (cp10 == 1 && !self.privileged()) {
            return Err(Trap::Usage(CFSR_NOCP));
        }
        if !self.secure && (self.nsacr >> 10) & 1 == 0 {
            return Err(Trap::Usage(CFSR_NOCP));
        }
        if self.cppwr & (1 << 20) != 0 {
            // CP10 is powered off (SU10)
            return Err(Trap::Usage(CFSR_NOCP));
        }
        if self.fpccr[S] & 1 != 0 {
            self.preserve_fp_state(bus, S);
        }
        if self.fpccr[NS] & 1 != 0 {
            self.preserve_fp_state(bus, NS);
        }
        if self.fpccr[S] & (1 << 31) != 0 && !self.fpca {
            self.fpscr = (self.fpscr & !0x07C0_0000) | (self.fpdscr[st] & 0x07C0_0000);
            self.fpca = true;
            if self.secure {
                self.sfpa = true;
            }
        } else if self.secure && !self.sfpa && self.fpccr[S] & (1 << 31) != 0 {
            self.sfpa = true;
        }
        Ok(())
    }

    /// `PreserveFPState()` of lazy stacking for the context of state `st`.
    fn preserve_fp_state<B: Bus>(&mut self, bus: &mut B, st: usize) {
        let base = self.fpcar[st];
        let ns = st == NS;
        let mut err = false;
        for i in 0..16u32 {
            if bus.write(self.now, base + 4 * i, 4, self.s[i as usize], ns, false).is_err() {
                err = true;
            }
        }
        if bus.write(self.now, base + 0x40, 4, self.fpscr, ns, false).is_err() {
            err = true;
        }
        let _ = bus.write(self.now, base + 0x44, 4, self.vpr, ns, false);
        if self.fpccr[st] & (1 << 26) != 0 {
            // TS: the additional context S16-S31, then all registers are cleared
            for i in 16..32u32 {
                let _ = bus.write(self.now, base + 0x48 + 4 * (i - 16), 4, self.s[i as usize], ns, false);
            }
            self.s = [0; 32];
            self.fpscr = 0;
            self.vpr = 0;
        }
        if err {
            self.bfsr |= (CFSR_LSPERR >> 8) as u8;
            if self.fpccr[st] & (1 << 6) != 0 {
                self.set_pending(EXC_BUSFAULT);
            } else {
                self.hfsr |= HFSR_FORCED;
                self.set_pending(EXC_HARDFAULT);
            }
        }
        self.fpccr[st] &= !1;
    }

    fn exec_vfp<B: Bus>(&mut self, bus: &mut B, v: Vfp) -> Result<u32, Trap> {
        self.fp_check(bus)?;
        let mut st = self.fpscr;
        match v {
            Vfp::Arith { op, prec, d, n, m } => {
                let (a, b, c) = (self.fp_get(prec, d), self.fp_get(prec, n), self.fp_get(prec, m));
                let r = match op {
                    VArith::Mla => {
                        let p = fpu::mul(b, c, prec, &mut st);
                        fpu::add(a, p, prec, &mut st)
                    }
                    VArith::Mls => {
                        let p = fpu::mul(b, c, prec, &mut st);
                        fpu::add(a, fpu::neg(p, prec), prec, &mut st)
                    }
                    VArith::Nmla => {
                        let p = fpu::mul(b, c, prec, &mut st);
                        fpu::add(fpu::neg(a, prec), fpu::neg(p, prec), prec, &mut st)
                    }
                    VArith::Nmls => {
                        let p = fpu::mul(b, c, prec, &mut st);
                        fpu::add(fpu::neg(a, prec), p, prec, &mut st)
                    }
                    VArith::Mul => fpu::mul(b, c, prec, &mut st),
                    VArith::Nmul => fpu::neg(fpu::mul(b, c, prec, &mut st), prec),
                    VArith::Add => fpu::add(b, c, prec, &mut st),
                    VArith::Sub => fpu::sub(b, c, prec, &mut st),
                    VArith::Div => fpu::div(b, c, prec, &mut st),
                    VArith::Fma => fpu::mul_add(a, b, c, prec, &mut st),
                    VArith::Fms => fpu::mul_add(a, fpu::neg(b, prec), c, prec, &mut st),
                    VArith::Fnma => fpu::mul_add(fpu::neg(a, prec), fpu::neg(b, prec), c, prec, &mut st),
                    VArith::Fnms => fpu::mul_add(fpu::neg(a, prec), b, c, prec, &mut st),
                };
                self.fp_set(prec, d, r);
            }
            Vfp::Unary { op, prec, d, m } => {
                let a = self.fp_get(prec, m);
                let r = match op {
                    VUnary::Mov => a,
                    VUnary::Abs => fpu::abs(a, prec),
                    VUnary::Neg => fpu::neg(a, prec),
                    VUnary::Sqrt => fpu::sqrt(a, prec, &mut st),
                };
                self.fp_set(prec, d, r);
            }
            Vfp::MovImm { prec, d, imm } => self.fp_set(prec, d, imm),
            Vfp::Cmp { e, zero, prec, d, m } => {
                let b = if zero { 0 } else { self.fp_get(prec, m) };
                let flags = fpu::compare(self.fp_get(prec, d), b, prec, e, &mut st);
                st = (st & 0x0FFF_FFFF) | flags;
            }
            Vfp::CvtToInt { signed, round_zero, prec, sd, m } => {
                let a = self.fp_get(prec, m);
                let r = fpu::to_fixed(a, prec, 32, 0, !signed, if round_zero { Some(Rounding::Zero) } else { None }, &mut st);
                self.s[sd as usize & 31] = r;
            }
            Vfp::CvtFromInt { signed, prec, d, sm } => {
                let r = fpu::from_fixed(self.s[sm as usize & 31], prec, 32, 0, !signed, true, &mut st);
                self.fp_set(prec, d, r);
            }
            Vfp::CvtFixed { to_fixed, signed, bits, frac, prec, d } => {
                let x = self.fp_get(prec, d);
                let r = if to_fixed {
                    fpu::to_fixed(x, prec, bits as u32, frac as u32, !signed, Some(Rounding::Zero), &mut st) as u64
                } else {
                    let int = match prec {
                        Prec::D => x as u32,
                        _ => x as u32,
                    };
                    fpu::from_fixed(int, prec, bits as u32, frac as u32, !signed, false, &mut st)
                };
                self.fp_set(prec, d, r);
            }
            Vfp::CvtHalf { to_half, top, prec, d, m } => {
                if to_half {
                    let h = fpu::convert(self.fp_get(prec, m), prec, Prec::H, &mut st) as u32;
                    let cur = self.s[d as usize & 31];
                    self.s[d as usize & 31] = if top { (cur & 0xFFFF) | (h << 16) } else { (cur & 0xFFFF_0000) | h };
                } else {
                    let x = self.s[m as usize & 31];
                    let h = if top { x >> 16 } else { x & 0xFFFF };
                    let r = fpu::convert(h as u64, Prec::H, prec, &mut st);
                    self.fp_set(prec, d, r);
                }
            }
            Vfp::CvtDouble { to_double, d, m } => {
                if to_double {
                    let r = fpu::convert(self.fp_get(Prec::S, m), Prec::S, Prec::D, &mut st);
                    self.fp_set(Prec::D, d, r);
                } else {
                    let r = fpu::convert(self.fp_get(Prec::D, m), Prec::D, Prec::S, &mut st);
                    self.fp_set(Prec::S, d, r);
                }
            }
            Vfp::Vsel { cond, prec, d, n, m } => {
                let c = match cond {
                    0 => self.z,
                    1 => self.v,
                    2 => self.n == self.v,
                    _ => !self.z && self.n == self.v,
                };
                let r = if c { self.fp_get(prec, n) } else { self.fp_get(prec, m) };
                self.fp_set(prec, d, r);
            }
            Vfp::MaxMinNm { max, prec, d, n, m } => {
                let r = fpu::max_min_nm(self.fp_get(prec, n), self.fp_get(prec, m), prec, max, &mut st);
                self.fp_set(prec, d, r);
            }
            Vfp::Rint { mode, prec, d, m } => {
                let (rm, exact) = match mode {
                    RintMode::A => (Some(Rounding::Away), false),
                    RintMode::N => (Some(Rounding::Nearest), false),
                    RintMode::P => (Some(Rounding::PlusInf), false),
                    RintMode::M => (Some(Rounding::MinusInf), false),
                    RintMode::Z => (Some(Rounding::Zero), false),
                    RintMode::X => (None, true),
                    RintMode::R => (None, false),
                };
                let r = fpu::rint(self.fp_get(prec, m), prec, rm, exact, &mut st);
                self.fp_set(prec, d, r);
            }
            Vfp::CvtRound { mode, signed, prec, sd, m } => {
                let rm = match mode {
                    RintMode::A => Rounding::Away,
                    RintMode::N => Rounding::Nearest,
                    RintMode::P => Rounding::PlusInf,
                    _ => Rounding::MinusInf,
                };
                let r = fpu::to_fixed(self.fp_get(prec, m), prec, 32, 0, !signed, Some(rm), &mut st);
                self.s[sd as usize & 31] = r;
            }
            Vfp::Vins { d, m } => {
                let x = self.s[m as usize & 31] & 0xFFFF;
                let cur = self.s[d as usize & 31];
                self.s[d as usize & 31] = (cur & 0xFFFF) | (x << 16);
            }
            Vfp::Vmovx { d, m } => {
                let x = self.s[m as usize & 31] >> 16;
                self.s[d as usize & 31] = x;
            }
            Vfp::LdrStr { load, size, sd, rn, imm, add } => {
                let base = if rn == 15 { self.pc.wrapping_add(4) & !3 } else { self.reg(rn) };
                let a = if add { base.wrapping_add(imm) } else { base.wrapping_sub(imm) };
                match size {
                    2 => {
                        if load {
                            let v = self.mem_read(bus, a, 2, true, false)?;
                            self.s[sd as usize & 31] = v;
                        } else {
                            let v = self.s[sd as usize & 31] & 0xFFFF;
                            self.mem_write(bus, a, 2, v, true, false)?;
                        }
                    }
                    _ => {
                        let words = size as usize / 4;
                        for i in 0..words {
                            let idx = (sd as usize + i) & 31;
                            let addr = a.wrapping_add(4 * i as u32);
                            if load {
                                self.s[idx] = self.mem_read(bus, addr, 4, true, false)?;
                            } else {
                                let v = self.s[idx];
                                self.mem_write(bus, addr, 4, v, true, false)?;
                            }
                        }
                    }
                }
            }
            Vfp::LdmStm { load, sd, count, rn, inc, wback } => {
                let base = self.reg(rn);
                let bytes = 4 * count as u32;
                let start = if inc { base } else { base.wrapping_sub(bytes) };
                if rn == 13 && !inc && wback {
                    self.sp_limit_check(start)?;
                }
                for i in 0..count as usize {
                    let idx = (sd as usize + i) & 31;
                    let addr = start.wrapping_add(4 * i as u32);
                    if load {
                        self.s[idx] = self.mem_read(bus, addr, 4, true, false)?;
                    } else {
                        let v = self.s[idx];
                        self.mem_write(bus, addr, 4, v, true, false)?;
                    }
                }
                if wback {
                    self.set_reg(rn, if inc { base.wrapping_add(bytes) } else { base.wrapping_sub(bytes) });
                }
            }
            Vfp::MovCore { to_core, half, rt, sn } => {
                if to_core {
                    let v = self.s[sn as usize & 31];
                    self.set_reg(rt, if half { v & 0xFFFF } else { v });
                } else {
                    let v = self.reg(rt);
                    self.s[sn as usize & 31] = if half { v & 0xFFFF } else { v };
                }
            }
            Vfp::MovCore2 { to_core, rt, rt2, sm } => {
                let (i, j) = (sm as usize & 31, (sm as usize + 1) & 31);
                if to_core {
                    let (a, b) = (self.s[i], self.s[j]);
                    self.set_reg(rt, a);
                    self.set_reg(rt2, b);
                } else {
                    self.s[i] = self.reg(rt);
                    self.s[j] = self.reg(rt2);
                }
            }
            Vfp::MovScalar { to_core, rt, sm } => {
                if to_core {
                    let v = self.s[sm as usize & 31];
                    self.set_reg(rt, v);
                } else {
                    self.s[sm as usize & 31] = self.reg(rt);
                }
            }
            Vfp::Vmrs { rt, reg } => {
                let v = match reg {
                    0 => 0x4102_0000,
                    1 => st,
                    2 => st & 0xF800_0000,
                    5 => crate::scs::MVFR2,
                    6 => crate::scs::MVFR1,
                    7 => crate::scs::MVFR0,
                    8 => 0,
                    12 => self.vpr,
                    13 => self.vpr & 0xFFFF,
                    14 | 15 => {
                        if !self.secure {
                            return Err(Trap::Usage(CFSR_UNDEFINSTR));
                        }
                        let ctx = (st & 0xFFFF_0000) | (self.sfpa as u32);
                        if reg == 14 && !self.sfpa {
                            // no FP context in use: the default values, and SFPA stays clear
                            self.fpdscr[NS] | (self.fpdscr[NS] & 0x07C0_0000)
                        } else {
                            ctx
                        }
                    }
                    _ => return Err(Trap::Usage(CFSR_UNDEFINSTR)),
                };
                if rt == 15 {
                    if reg == 1 || reg == 2 {
                        self.n = v >> 31 & 1 != 0;
                        self.z = v >> 30 & 1 != 0;
                        self.c = v >> 29 & 1 != 0;
                        self.v = v >> 28 & 1 != 0;
                    }
                } else {
                    self.set_reg(rt, v);
                }
            }
            Vfp::Vmsr { rt, reg } => {
                let v = self.reg(rt);
                match reg {
                    1 => st = v & fpu::FPSCR_MASK,
                    2 => st = (st & 0x07FF_FFFF) | (v & 0xF800_0000),
                    12 => self.vpr = v & 0xFFFF_000F | (v & 0xFF00_0000) | (v & 0x00FF_0000),
                    13 => self.vpr = (self.vpr & 0xFFFF_0000) | (v & 0xFFFF),
                    14 | 15 => {
                        if !self.secure {
                            return Err(Trap::Usage(CFSR_UNDEFINSTR));
                        }
                        st = (st & 0x0000_FFFF) | (v & 0xFFFF_0000 & fpu::FPSCR_MASK);
                        self.sfpa = v & 1 != 0;
                    }
                    _ => return Err(Trap::Usage(CFSR_UNDEFINSTR)),
                }
            }
            Vfp::Vlstm { rn } => {
                if self.secure {
                    let base = self.reg(rn) & !7;
                    if self.sfpa {
                        for i in 0..32u32 {
                            let v = self.s[i as usize];
                            self.mem_write(bus, base + 4 * i, 4, v, true, false)?;
                        }
                        self.mem_write(bus, base + 0x80, 4, st, true, false)?;
                        let vpr = self.vpr;
                        self.mem_write(bus, base + 0x84, 4, vpr, true, false)?;
                    }
                    self.fpccr[S] &= !1;
                }
            }
            Vfp::Vlldm { rn } => {
                if self.secure {
                    let base = self.reg(rn) & !7;
                    if self.sfpa {
                        for i in 0..32u32 {
                            self.s[i as usize] = self.mem_read(bus, base + 4 * i, 4, true, false)?;
                        }
                        st = self.mem_read(bus, base + 0x80, 4, true, false)? & fpu::FPSCR_MASK;
                        self.vpr = self.mem_read(bus, base + 0x84, 4, true, false)? & 0xFFFF_000F;
                    }
                }
            }
            Vfp::Vscclrm { sd, count } => {
                if self.secure {
                    for i in 0..count as usize {
                        self.s[(sd as usize + i) & 31] = 0;
                    }
                    self.vpr = 0;
                }
            }
        }
        self.fpscr = st;
        Ok(1)
    }
}
