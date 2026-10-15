/*
 * Corstone-320 FVP probe: prints core ID registers, reset register values, counter rates and a
 * UART0 test over semihosting (SYS_WRITE0). Output goes to fixtures/fvp/probe.txt.
 */
#include <stdint.h>

extern uint32_t __reset_snapshot;
extern volatile uint32_t fault_count, last_fault_ipsr, last_fault_pc;
extern void (*const vectors[])(void);

/* ------------------------------------------------------------------ semihosting output */

static int semihost(int op, void *arg) {
  register int r0 __asm("r0") = op;
  register void *r1 __asm("r1") = arg;
  __asm volatile("bkpt 0xAB" : "+r"(r0) : "r"(r1) : "memory");
  return r0;
}

static void puts_(const char *s) { semihost(0x04, (void *)s); }

void semihost_exit(int code) {
  uint32_t block[2] = { 0x20026, (uint32_t)code };
  semihost(0x20, block);   /* SYS_EXIT_EXTENDED */
  semihost(0x18, (void *)0x20026);
  for (;;) {}
}

static void put_hex(uint32_t v) {
  char b[11] = "0x00000000";
  for (int i = 0; i < 8; i++) b[9 - i] = "0123456789abcdef"[(v >> (4 * i)) & 15];
  puts_(b);
}

static void put_dec(uint64_t v) {
  char b[24];
  int i = 23;
  b[i] = 0;
  if (v == 0) b[--i] = '0';
  while (v) { b[--i] = '0' + (v % 10); v /= 10; }
  puts_(&b[i]);
}

static void put_sdec(int64_t v) {
  if (v < 0) { puts_("-"); put_dec((uint64_t)(-v)); } else put_dec((uint64_t)v);
}

static void line(const char *name, uint32_t v) {
  puts_(name);
  puts_(": ");
  put_hex(v);
  puts_("\n");
}

/* ------------------------------------------------------------------ guarded reads */

#define REG(a) (*(volatile uint32_t *)(a))

static uint32_t rd(uint32_t addr) {
  __asm volatile("dsb\n isb" ::: "memory");
  uint32_t v = REG(addr);
  __asm volatile("dsb\n isb" ::: "memory");
  return v;
}

/* Prints name: value, or name: FAULT (ipsr) when the read faulted. */
static void show(const char *name, uint32_t addr) {
  uint32_t before = fault_count;
  uint32_t v = rd(addr);
  if (fault_count != before) {
    puts_(name);
    puts_(": FAULT ipsr=");
    put_hex(last_fault_ipsr);
    puts_(" addr=");
    put_hex(addr);
    puts_("\n");
  } else {
    line(name, v);
  }
}

/* Prints "0xADDR 0xVALUE" lines for a word range, like fvp_dump.py. */
static void dump_range(const char *name, uint32_t start, uint32_t end) {
  puts_("# ");
  puts_(name);
  puts_("\n");
  for (uint32_t a = start; a < end; a += 4) {
    uint32_t before = fault_count;
    uint32_t v = rd(a);
    put_hex(a);
    puts_(" ");
    if (fault_count != before) puts_("FAULT"); else put_hex(v);
    puts_("\n");
  }
}

/* ------------------------------------------------------------------ part 1: ID and reset state */

struct reg { const char *name; uint32_t addr; };

static const struct reg id_regs[] = {
  { "CPUID", 0xE000ED00 }, { "ICTR", 0xE000E004 }, { "ACTLR", 0xE000E008 },
  { "ID_PFR0", 0xE000ED40 }, { "ID_PFR1", 0xE000ED44 }, { "ID_DFR0", 0xE000ED48 }, { "ID_AFR0", 0xE000ED4C },
  { "ID_MMFR0", 0xE000ED50 }, { "ID_MMFR1", 0xE000ED54 }, { "ID_MMFR2", 0xE000ED58 }, { "ID_MMFR3", 0xE000ED5C },
  { "ID_ISAR0", 0xE000ED60 }, { "ID_ISAR1", 0xE000ED64 }, { "ID_ISAR2", 0xE000ED68 }, { "ID_ISAR3", 0xE000ED6C },
  { "ID_ISAR4", 0xE000ED70 }, { "ID_ISAR5", 0xE000ED74 },
  { "CLIDR", 0xE000ED78 }, { "CTR", 0xE000ED7C }, { "CCSIDR", 0xE000ED80 }, { "CSSELR", 0xE000ED84 },
  { "CPACR", 0xE000ED88 }, { "NSACR", 0xE000ED8C },
  { "MPU_TYPE", 0xE000ED90 }, { "MPU_CTRL", 0xE000ED94 }, { "SAU_CTRL", 0xE000EDD0 }, { "SAU_TYPE", 0xE000EDD4 },
  { "DHCSR", 0xE000EDF0 }, { "DCRSR", 0xE000EDF4 }, { "DEMCR", 0xE000EDFC }, { "DAUTHCTRL", 0xE000EE04 },
  { "DSCSR", 0xE000EE08 },
  { "FPCCR", 0xE000EF34 }, { "FPCAR", 0xE000EF38 }, { "FPDSCR", 0xE000EF3C },
  { "MVFR0", 0xE000EF40 }, { "MVFR1", 0xE000EF44 }, { "MVFR2", 0xE000EF48 },
  { "AIRCR", 0xE000ED0C }, { "SCR", 0xE000ED10 }, { "CCR", 0xE000ED14 }, { "SHCSR", 0xE000ED24 },
  { "CFSR", 0xE000ED28 }, { "HFSR", 0xE000ED2C }, { "VTOR", 0xE000ED08 }, { "ICSR", 0xE000ED04 },
  { "SYST_CSR", 0xE000E010 }, { "SYST_RVR", 0xE000E014 }, { "SYST_CVR", 0xE000E018 }, { "SYST_CALIB", 0xE000E01C },
  { "DWT_CTRL", 0xE0001000 }, { "DWT_CYCCNT", 0xE0001004 }, { "DWT_DEVARCH", 0xE0001FBC },
  { "FP_CTRL", 0xE0002000 }, { "FP_REMAP", 0xE0002004 }, { "FP_DEVARCH", 0xE0002FBC },
  { "ITM_TCR", 0xE0000E80 }, { "ITM_DEVARCH", 0xE0000FBC },
  { "PMU_TYPE", 0xE0003E00 }, { "PMU_CTRL", 0xE0003E04 }, { "PMU_CNTENSET", 0xE0003C00 },
  { "PMU_CCNTR", 0xE00037E0 }, { "PMU_EVCNTR0", 0xE0003000 }, { "PMU_EVTYPER0", 0xE0003400 },
  { "PMU_AUTHSTATUS", 0xE0003FB8 }, { "PMU_DEVARCH", 0xE0003FBC }, { "PMU_DEVTYPE", 0xE0003FCC },
  { "PMU_PIDR4", 0xE0003FD0 }, { "PMU_PIDR5", 0xE0003FD4 }, { "PMU_PIDR6", 0xE0003FD8 }, { "PMU_PIDR7", 0xE0003FDC },
  { "PMU_PIDR0", 0xE0003FE0 }, { "PMU_PIDR1", 0xE0003FE4 }, { "PMU_PIDR2", 0xE0003FE8 }, { "PMU_PIDR3", 0xE0003FEC },
  { "PMU_CIDR0", 0xE0003FF0 }, { "PMU_CIDR1", 0xE0003FF4 }, { "PMU_CIDR2", 0xE0003FF8 }, { "PMU_CIDR3", 0xE0003FFC },
  { "ITCMCR", 0xE001E010 }, { "DTCMCR", 0xE001E014 }, { "PAHBCR", 0xE001E018 },
  { "CPDLPSTATE", 0xE001E300 }, { "DPDLPSTATE", 0xE001E304 },
  { "SCS_CIDR0", 0xE000EFF0 }, { "SCS_PIDR0", 0xE000EFE0 },
  { "CPU_IDENTITY", 0x4001F000 }, { "CPU_IDENTITY_S", 0x5001F000 }, { "CPU_PWRCTRL", 0x40012000 },
  { "CPU_PWRCTRL_S", 0x50012000 }, { "CPU_SECCTRL", 0x50011000 },
  { "SYSINFO_SOC_IDENTITY", 0x58020000 }, { "SYSINFO_SYS_CONFIG0", 0x58020004 },
  { "SYSCTRL_SECDBGSTAT", 0x58021000 }, { "SYSCTRL_RESET_SYNDROME", 0x58021100 },
  { "SYSCTRL_INITSVTOR0", 0x58021110 }, { "SYSCTRL_CPUWAIT", 0x58021118 }, { "SYSCTRL_GRETREG", 0x58021108 },
  { "CNTCR", 0x58100000 }, { "CNTSR", 0x58100004 }, { "CNTID", 0x5810001C }, { "CNTFID0", 0x58100020 },
};

static void part1(void) {
  const uint32_t *snap = &__reset_snapshot;
  static const char *const snap_names[] = {
    "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10", "r11", "r12", "lr",
    "MSP", "PSP", "MSPLIM", "PSPLIM", "CONTROL", "PRIMASK", "BASEPRI", "FAULTMASK", "XPSR",
    "MSP_NS", "PSP_NS", "MSPLIM_NS", "PSPLIM_NS", "CONTROL_NS", "PRIMASK_NS", "BASEPRI_NS",
    "FAULTMASK_NS", "SP_NS", "FPSCR",
  };
  puts_("# part 1a: core state at reset (MRS in Reset_Handler before any register is written)\n");
  for (unsigned i = 0; i < sizeof(snap_names) / sizeof(snap_names[0]); i++) {
    puts_("reset.");
    line(snap_names[i], snap[i]);
  }
  line("vector_table.SP", (uint32_t)vectors[0]);
  line("vector_table.Reset", (uint32_t)vectors[1]);

  puts_("# part 1b: ID and control registers (CPU reads; CPACR already set to 0x00F00000)\n");
  for (unsigned i = 0; i < sizeof(id_regs) / sizeof(id_regs[0]); i++) show(id_regs[i].name, id_regs[i].addr);

  puts_("# part 1c: CCSIDR per CSSELR\n");
  for (uint32_t sel = 0; sel < 8; sel++) {
    REG(0xE000ED84) = sel;
    __asm volatile("dsb\n isb" ::: "memory");
    uint32_t rb = rd(0xE000ED84);
    puts_("CSSELR=");
    put_hex(sel);
    puts_(" readback=");
    put_hex(rb);
    puts_(" CCSIDR=");
    put_hex(rd(0xE000ED80));
    puts_("\n");
  }
  REG(0xE000ED84) = 0;

  puts_("# part 1d: faults so far\n");
  line("fault_count", fault_count);
}

/* ------------------------------------------------------------------ part 2: timing */

struct sample {
  uint32_t cntcv_lo, cntcv_hi;    /* system counter read frame 0x58101000 */
  uint32_t cyccnt;                /* DWT_CYCCNT */
  uint32_t pmu_ccntr;             /* PMU cycle counter */
  uint32_t pmu_ev0, pmu_ev1;      /* PMU event counters 0 (INST_RETIRED) and 1 (CPU_CYCLES) */
  uint32_t systick;               /* SYST_CVR */
  uint32_t fpga_1hz, fpga_100hz, fpga_cyc, fpga_pscntr;
  uint32_t slowclk;               /* 0x4802F004 CMSDK timer VALUE (counts down) */
  uint32_t rtc;                   /* 0x4820B000 RTCDR */
  uint32_t tim0_lo, tim0_hi;      /* systimer0 CNTPCT */
};

static void take(struct sample *s) {
  __asm volatile("dsb\n isb" ::: "memory");
  s->cyccnt = REG(0xE0001004);
  s->pmu_ccntr = REG(0xE00037E0);
  s->pmu_ev0 = REG(0xE0003000);
  s->pmu_ev1 = REG(0xE0003004);
  s->systick = REG(0xE000E018);
  s->cntcv_lo = REG(0x58101000);
  s->cntcv_hi = REG(0x58101004);
  s->tim0_lo = REG(0x48000000);
  s->tim0_hi = REG(0x48000004);
  s->fpga_1hz = REG(0x48202010);
  s->fpga_100hz = REG(0x48202014);
  s->fpga_cyc = REG(0x48202018);
  s->fpga_pscntr = REG(0x48202020);
  s->slowclk = REG(0x4802F004);
  s->rtc = REG(0x4820B000);
  __asm volatile("dsb\n isb" ::: "memory");
}

static void loop_subs(uint32_t n) {
  __asm volatile(
      ".p2align 2\n"
      "1: subs %0, #1\n"
      "   bne 1b\n" : "+r"(n) : : "cc");
}

/* 1000 iterations of 1000 nops: 1,000,000 nops + 2,000 loop instructions */
static void loop_nops(uint32_t n) {
  __asm volatile(
      ".p2align 2\n"
      "1: .rept 1000\n"
      "   nop\n"
      "   .endr\n"
      "   subs %0, #1\n"
      "   bne 1b\n" : "+r"(n) : : "cc");
}

static void delta_line(const char *name, uint32_t a, uint32_t b) {
  puts_("  ");
  puts_(name);
  puts_(": before=");
  put_hex(a);
  puts_(" after=");
  put_hex(b);
  puts_(" delta=");
  put_sdec((int64_t)(int32_t)(b - a));
  puts_("\n");
}

static void report_deltas(const char *title, const struct sample *a, const struct sample *b) {
  puts_(title);
  puts_("\n");
  uint64_t ca = ((uint64_t)a->cntcv_hi << 32) | a->cntcv_lo;
  uint64_t cb = ((uint64_t)b->cntcv_hi << 32) | b->cntcv_lo;
  puts_("  CNTCV: before=");
  put_dec(ca);
  puts_(" after=");
  put_dec(cb);
  puts_(" delta=");
  put_dec(cb - ca);
  puts_("\n");
  uint64_t ta = ((uint64_t)a->tim0_hi << 32) | a->tim0_lo;
  uint64_t tb = ((uint64_t)b->tim0_hi << 32) | b->tim0_lo;
  puts_("  systimer0.CNTPCT: before=");
  put_dec(ta);
  puts_(" after=");
  put_dec(tb);
  puts_(" delta=");
  put_dec(tb - ta);
  puts_("\n");
  delta_line("DWT_CYCCNT", a->cyccnt, b->cyccnt);
  delta_line("PMU_CCNTR", a->pmu_ccntr, b->pmu_ccntr);
  delta_line("PMU_EVCNTR0(INST_RETIRED)", a->pmu_ev0, b->pmu_ev0);
  delta_line("PMU_EVCNTR1(CPU_CYCLES)", a->pmu_ev1, b->pmu_ev1);
  puts_("  SYST_CVR: before=");
  put_hex(a->systick);
  puts_(" after=");
  put_hex(b->systick);
  puts_(" down_by=");
  put_dec((a->systick - b->systick) & 0xFFFFFF);
  puts_("\n");
  delta_line("FPGAIO_CLK1HZ", a->fpga_1hz, b->fpga_1hz);
  delta_line("FPGAIO_CLK100HZ", a->fpga_100hz, b->fpga_100hz);
  delta_line("FPGAIO_COUNTER", a->fpga_cyc, b->fpga_cyc);
  delta_line("FPGAIO_PSCNTR", a->fpga_pscntr, b->fpga_pscntr);
  puts_("  SLOWCLK_TIMER_VALUE: before=");
  put_hex(a->slowclk);
  puts_(" after=");
  put_hex(b->slowclk);
  puts_(" down_by=");
  put_dec(a->slowclk - b->slowclk);
  puts_("\n");
  delta_line("RTCDR", a->rtc, b->rtc);
}

static void part2(void) {
  struct sample s0, s1, s2, s3, s4, s5;
  puts_("# part 2: timing\n");

  /* system counter */
  line("CNTCR.before", rd(0x58100000));
  if (!(rd(0x58100000) & 1)) REG(0x58100000) = 1;
  line("CNTCR.after", rd(0x58100000));
  line("CNTFID0", rd(0x58100020));
  line("CNTSCR", rd(0x58100010));
  line("systimer0.CNTFRQ", rd(0x48000010));
  line("systimer0.CNTP_CTL", rd(0x4800002C));

  /* DWT */
  REG(0xE000EDFC) |= (1u << 24);          /* DEMCR.TRCENA */
  line("DEMCR", rd(0xE000EDFC));
  REG(0xE0001004) = 0;
  REG(0xE0001000) |= 1;                   /* CYCCNTENA */
  line("DWT_CTRL.after_enable", rd(0xE0001000));

  /* PMU: event counter 0 = INST_RETIRED (0x08), 1 = CPU_CYCLES (0x11) */
  REG(0xE0003400) = 0x08;
  REG(0xE0003404) = 0x11;
  REG(0xE0003C00) = (1u << 31) | 3;       /* CNTENSET: C, 0, 1 */
  REG(0xE0003E04) = 0x7;                  /* CTRL: E | P | C */
  line("PMU_CTRL.after_enable", rd(0xE0003E04));
  line("PMU_CNTENSET.after_enable", rd(0xE0003C00));
  line("PMU_EVTYPER0.readback", rd(0xE0003400));
  line("PMU_EVTYPER1.readback", rd(0xE0003404));

  /* SysTick: CLKSOURCE=1, LOAD=0xFFFFFF */
  REG(0xE000E014) = 0xFFFFFF;
  REG(0xE000E018) = 0;
  REG(0xE000E010) = 5;
  line("SYST_CSR.after_enable", rd(0xE000E010));

  /* slow-clock CMSDK timer: RELOAD=VALUE=0xFFFFFFFF, CTRL=1 */
  REG(0x4802F008) = 0xFFFFFFFFu;
  REG(0x4802F004) = 0xFFFFFFFFu;
  REG(0x4802F000) = 1;
  line("SLOWCLK_TIMER_CTRL.after_enable", rd(0x4802F000));
  line("RTCCR", rd(0x4820B00C));
  line("RTCDR", rd(0x4820B000));
  line("FPGAIO_PRESCALE", rd(0x4820201C));
  line("FPGAIO_PSCNTR", rd(0x48202020));
  line("faults_in_setup", fault_count);

  take(&s0);
  take(&s1);
  report_deltas("## empty (two consecutive samples, measures the sampling overhead)", &s0, &s1);

  take(&s2);
  loop_subs(1000000);
  take(&s3);
  report_deltas("## subs/bne loop, 1,000,000 iterations (2,000,000 instructions)", &s2, &s3);

  take(&s4);
  loop_nops(1000);
  take(&s5);
  report_deltas("## nop loop, 1,000 x (1000 nop + subs + bne) = 1,002,000 instructions", &s4, &s5);

  {
    uint64_t c2 = ((uint64_t)s2.cntcv_hi << 32) | s2.cntcv_lo;
    uint64_t c3 = ((uint64_t)s3.cntcv_hi << 32) | s3.cntcv_lo;
    uint64_t c4 = ((uint64_t)s4.cntcv_hi << 32) | s4.cntcv_lo;
    uint64_t c5 = ((uint64_t)s5.cntcv_hi << 32) | s5.cntcv_lo;
    puts_("## summary\n");
    puts_("CNTCV ticks per 1M instructions (subs loop): ");
    put_dec((c3 - c2) * 1000000 / 2000000);
    puts_("\n");
    puts_("CNTCV ticks per 1M instructions (nop loop): ");
    put_dec((c5 - c4) * 1000000 / 1002000);
    puts_("\n");
    puts_("DWT_CYCCNT per 1M instructions (subs loop): ");
    put_dec((uint64_t)(s3.cyccnt - s2.cyccnt) * 1000000 / 2000000);
    puts_("\n");
    puts_("DWT_CYCCNT per 1M instructions (nop loop): ");
    put_dec((uint64_t)(s5.cyccnt - s4.cyccnt) * 1000000 / 1002000);
    puts_("\n");
    puts_("SYST ticks per 1M instructions (subs loop): ");
    put_dec((uint64_t)((s2.systick - s3.systick) & 0xFFFFFF) * 1000000 / 2000000);
    puts_("\n");
    puts_("SLOWCLK ticks per 1M instructions (subs loop): ");
    put_dec((uint64_t)(s2.slowclk - s3.slowclk) * 1000000 / 2000000);
    puts_("\n");
    puts_("FPGAIO_COUNTER per 1M instructions (subs loop): ");
    put_dec((uint64_t)(s3.fpga_cyc - s2.fpga_cyc) * 1000000 / 2000000);
    puts_("\n");
  }

  /* a longer run to make the slow counters visible: 10 x subs loop = 20M instructions */
  take(&s0);
  for (int i = 0; i < 10; i++) loop_subs(1000000);
  take(&s1);
  report_deltas("## 10 x subs/bne loop (20,000,000 instructions)", &s0, &s1);

  /* SysTick with CLKSOURCE=0 (external reference clock) */
  REG(0xE000E010) = 0;
  REG(0xE000E018) = 0;
  REG(0xE000E010) = 1;
  line("SYST_CSR.clksource0", rd(0xE000E010));
  take(&s0);
  loop_subs(1000000);
  take(&s1);
  puts_("SYST_CVR with CLKSOURCE=0 over 2,000,000 instructions: down_by=");
  put_dec((s0.systick - s1.systick) & 0xFFFFFF);
  puts_("\n");
  REG(0xE000E010) = 5;

  /* FPGA IO prescaler: does COUNTER/PSCNTR run with PRESCALE != 0? */
  REG(0x4820201C) = 24;
  line("FPGAIO_PRESCALE.readback", rd(0x4820201C));
  take(&s0);
  loop_subs(1000000);
  take(&s1);
  report_deltas("## subs/bne loop with FPGAIO PRESCALE=24 (2,000,000 instructions)", &s0, &s1);
  REG(0x48202018) = 0;      /* COUNTER write */
  REG(0x48202020) = 0;      /* PSCNTR write */
  line("FPGAIO_COUNTER.after_write0", rd(0x48202018));
  line("FPGAIO_PSCNTR.after_write0", rd(0x48202020));
  REG(0x4820201C) = 0;

  /* RTC: enable and wait 60M instructions (2.4 s at 25 MHz) */
  line("RTC_PID0", rd(0x4820BFE0));
  line("RTC_PID1", rd(0x4820BFE4));
  line("RTC_PID2", rd(0x4820BFE8));
  REG(0x4820B00C) = 1;
  line("RTCCR.readback", rd(0x4820B00C));
  line("RTCDR.after_enable", rd(0x4820B000));
  take(&s0);
  for (int i = 0; i < 30; i++) loop_subs(1000000);
  take(&s1);
  report_deltas("## 30 x subs/bne loop with RTC enabled (60,000,000 instructions)", &s0, &s1);

  /* PMU: does SW_INCR move an event counter at all? */
  REG(0xE0003400) = 0x00;   /* SW_INCR */
  REG(0xE0003E04) = 0x1;
  REG(0xE0003CA0) = 0x1;
  REG(0xE0003CA0) = 0x1;
  REG(0xE0003CA0) = 0x1;
  line("PMU_EVCNTR0.after_3_swinc", rd(0xE0003000));
  line("PMU_CCNTR.now", rd(0xE00037E0));
  line("PMU_CTRL.now", rd(0xE0003E04));
  line("PMU_CNTENSET.now", rd(0xE0003C00));
  line("PMU_OVSSET.now", rd(0xE0003CC0));
  line("faults_in_part2", fault_count);
}

/* ------------------------------------------------------------------ part 3: UART0 */

static void part3(void) {
  puts_("# part 3: UART0 at 0x48203000\n");
  line("UART0_STATE.reset", rd(0x48203004));
  line("UART0_CTRL.reset", rd(0x48203008));
  line("UART0_INTSTATUS.reset", rd(0x4820300C));
  line("UART0_BAUDDIV.reset", rd(0x48203010));
  line("UART0_S_alias_0x59203004_STATE.reset", rd(0x59203004));
  line("UART0_S_alias_0x59203010_BAUDDIV.reset", rd(0x59203010));
  line("0x58203004.reset", rd(0x58203004));
  REG(0x48203010) = 217;                  /* 25 MHz / 115200 */
  REG(0x48203008) = 1;                    /* TX_EN */
  line("UART0_BAUDDIV.readback", rd(0x48203010));
  line("UART0_CTRL.readback", rd(0x48203008));
  REG(0x48203000) = 'h';
  line("UART0_STATE.after_byte", rd(0x48203004));
  line("UART0_CTRL.after_byte", rd(0x48203008));
  line("UART0_INTSTATUS.after_byte", rd(0x4820300C));
  line("UART0_BAUDDIV.after_byte", rd(0x48203010));
  uint32_t spins = 0;
  while (rd(0x48203004) & 1) spins++;
  line("UART0_STATE.spins_until_tx_empty", spins);
  const char *s = "ello from the probe\n";
  for (; *s; s++) {
    while (rd(0x48203004) & 1) {}
    REG(0x48203000) = (uint32_t)*s;
  }
  while (rd(0x48203004) & 1) {}
  line("UART0_STATE.after_text", rd(0x48203004));
  line("UART0_INTSTATUS.after_text", rd(0x4820300C));
  line("UART0_DATA.read", rd(0x48203000));
  line("UART0_S_alias_0x59203010_BAUDDIV.after", rd(0x59203010));
  line("UART0_S_alias_0x59203008_CTRL.after", rd(0x59203008));
  line("UART0_PID0", rd(0x48203FE0));
  line("UART0_PID1", rd(0x48203FE4));
  line("UART0_PID2", rd(0x48203FE8));
  line("UART0_CID0", rd(0x48203FF0));
  line("faults_in_part3", fault_count);
}

/* ------------------------------------------------------------------ part 4: PPB dump from the CPU */

static void part4(void) {
  puts_("# part 4: PPB word dump from the CPU (0xADDR 0xVALUE, FAULT when the access faulted)\n");
  dump_range("SCS 0xE000E000-0xE000E0FF (ICTR, ACTLR, SysTick)", 0xE000E000, 0xE000E100);
  dump_range("NVIC ISER/ICER/ISPR/ICPR/IABR/ITNS 0xE000E100-0xE000E3FF", 0xE000E100, 0xE000E400);
  dump_range("NVIC IPR 0xE000E400-0xE000E5FF (first 128 words)", 0xE000E400, 0xE000E600);
  dump_range("SCB 0xE000ED00-0xE000EDFF", 0xE000ED00, 0xE000EE00);
  dump_range("DCB/DIB 0xE000EE00-0xE000EE2F", 0xE000EE00, 0xE000EE30);
  dump_range("STIR/FPU/CoreSight IDs 0xE000EF00-0xE000EFFF", 0xE000EF00, 0xE000F000);
  dump_range("DWT 0xE0001000-0xE000107F", 0xE0001000, 0xE0001080);
  dump_range("DWT IDs 0xE0001FA0-0xE0001FFF", 0xE0001FA0, 0xE0002000);
  dump_range("FPB 0xE0002000-0xE000203F", 0xE0002000, 0xE0002040);
  dump_range("FPB IDs 0xE0002FA0-0xE0002FFF", 0xE0002FA0, 0xE0003000);
  dump_range("ITM 0xE0000E00-0xE0000FFF", 0xE0000E00, 0xE0001000);
  dump_range("PMU counters 0xE0003000-0xE000303F", 0xE0003000, 0xE0003040);
  dump_range("PMU types 0xE0003400-0xE000343F", 0xE0003400, 0xE0003440);
  dump_range("PMU CCNTR 0xE00037E0-0xE00037FF", 0xE00037E0, 0xE0003800);
  dump_range("PMU control 0xE0003C00-0xE0003CFF", 0xE0003C00, 0xE0003D00);
  dump_range("PMU ID 0xE0003E00-0xE0003FFF", 0xE0003E00, 0xE0004000);
  dump_range("TCM/TGU/CPDLPSTATE 0xE001E000-0xE001E33F", 0xE001E000, 0xE001E340);
  dump_range("TCM block IDs 0xE001EFA0-0xE001EFFF", 0xE001EFA0, 0xE001F000);
  dump_range("NS SCS alias 0xE002ED00-0xE002EDFF", 0xE002ED00, 0xE002EE00);
  dump_range("NS SCS alias 0xE002E000-0xE002E01F", 0xE002E000, 0xE002E020);
  dump_range("TPIU 0xE0040000-0xE004003F", 0xE0040000, 0xE0040040);
  dump_range("TPIU IDs 0xE0040FA0-0xE0040FFF", 0xE0040FA0, 0xE0041000);
  dump_range("ETM 0xE0041000-0xE00410FF", 0xE0041000, 0xE0041100);
  dump_range("ETM IDs 0xE0041F00-0xE0041FFF", 0xE0041F00, 0xE0042000);
  dump_range("CTI 0xE0042000-0xE004203F", 0xE0042000, 0xE0042040);
  dump_range("CTI IDs 0xE0042FA0-0xE0042FFF", 0xE0042FA0, 0xE0043000);
  dump_range("ROM table 0xE00FF000-0xE00FF03F", 0xE00FF000, 0xE00FF040);
  dump_range("ROM table IDs 0xE00FFFA0-0xE00FFFFF", 0xE00FFFA0, 0xE0100000);
  line("faults_in_part4", fault_count);
}

/* ------------------------------------------------------------------ part 5: memory edges */

static void part5(void) {
  puts_("# part 5: memory edges read by the CPU\n");
  static const struct reg edges[] = {
    { "ITCM_S 0x10000000", 0x10000000 }, { "ITCM_NS 0x00000000", 0x00000000 },
    { "ITCM_S end 0x10008000", 0x10008000 }, { "ITCM_NS end 0x00008000", 0x00008000 },
    { "ROM_S 0x11000000", 0x11000000 }, { "ROM_NS? 0x01000000", 0x01000000 },
    { "ROM_S end 0x11020000", 0x11020000 },
    { "SRAM_S 0x12000000", 0x12000000 }, { "SRAM_NS 0x02000000", 0x02000000 },
    { "SRAM_S end 0x12200000", 0x12200000 },
    { "DTCM_S 0x30000000", 0x30000000 }, { "DTCM_NS 0x20000000", 0x20000000 },
    { "DTCM_S end 0x30008000", 0x30008000 }, { "DTCM_NS end 0x20008000", 0x20008000 },
    { "ISRAM0_S 0x31000000", 0x31000000 }, { "ISRAM0_NS 0x21000000", 0x21000000 },
    { "ISRAM1_S 0x31200000", 0x31200000 }, { "ISRAM1_S end 0x31400000", 0x31400000 },
    { "QSPI_S 0x38000000", 0x38000000 }, { "QSPI_NS 0x28000000", 0x28000000 },
    { "QSPI_S +8MB-16 0x387FFFF0", 0x387FFFF0 }, { "QSPI_S +8MB 0x38800000", 0x38800000 },
    { "QSPI_S +128MB-16 0x3FFFFFF0", 0x3FFFFFF0 }, { "QSPI_S +128MB 0x40000000", 0x40000000 },
    { "DDR_NS 0x60000000", 0x60000000 }, { "DDR_S 0x70000000", 0x70000000 },
    { "DDR_S +256MB-16 0x7FFFFFF0", 0x7FFFFFF0 }, { "DDR_S end 0x80000000", 0x80000000 },
    { "0x90000000", 0x90000000 }, { "0xA0000000", 0xA0000000 }, { "0xD0000000", 0xD0000000 },
  };
  for (unsigned i = 0; i < sizeof(edges) / sizeof(edges[0]); i++) show(edges[i].name, edges[i].addr);
  line("faults_in_part5", fault_count);
}

int main(void) {
  puts_("# Corstone-320 FVP probe\n");
  /* enable the configurable fault handlers so faults do not escalate to HardFault */
  REG(0xE000ED24) |= (1u << 16) | (1u << 17) | (1u << 18) | (1u << 19);  /* MEMFAULTENA|BUSFAULTENA|USGFAULTENA|SECUREFAULTENA */
  line("SHCSR.after_enable", rd(0xE000ED24));
  part1();
  part2();
  part3();
  part4();
  part5();
  line("total_faults", fault_count);
  for (int i = 0; i < 5; i++) loop_subs(1000000);   /* 0.4 s of simulated time for the UART to drain */
  puts_("# end\n");
  return 0;
}
