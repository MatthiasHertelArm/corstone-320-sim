/*
 * ISA differential test for the Cortex-M85 (Armv8.1-M Mainline, FPv5 with double and half
 * precision, PACBTI, low-overhead loops, Security Extension).
 *
 * Executes instructions on deterministic pseudo-random and edge-case operands and prints a
 * digest per group over semihosting (SYS_WRITE0). Run the same image on corstone-320-sim and on
 * Arm's FVP_Corstone_SSE-320 and compare the outputs line by line; any difference is a
 * simulator bug. Derived from microbit-v2-sim/firmware/isa-test (Cortex-M4). The MVE section
 * (mve_test.c) comes last, after the marker line `mve: 0x00000001`.
 */
#include "isa_test.h"

/* ------------------------------------------------------------------ semihosting output */

static int semihost(int op, void *arg) {
  register int r0 __asm("r0") = op;
  register void *r1 __asm("r1") = arg;
  __asm volatile("bkpt 0xAB" : "+r"(r0) : "r"(r1) : "memory");
  return r0;
}

void puts_(const char *s) { semihost(0x04, (void *)s); }

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

void report(const char *name, uint32_t d) {
  puts_(name);
  puts_(": ");
  put_hex(d);
  puts_("\n");
}

/* ------------------------------------------------------------------ helpers */

uint32_t digest;
/* Built with -DVERBOSE_GROUPS='"name,name,..."': every value mixed into the digests of those
   groups is also printed as a line "  0x........" (ignored by the comparison), to find the
   instruction behind a mismatching digest by diffing the FVP and simulator outputs. */
static int verbose;
#ifdef VERBOSE_GROUPS
static int verbose_group(const char *name) {
  const char *l = VERBOSE_GROUPS;
  while (*l) {
    const char *n = name;
    while (*l && *l != ',' && *n && *l == *n) { l++; n++; }
    if ((*l == 0 || *l == ',') && *n == 0) return 1;
    while (*l && *l != ',') l++;
    if (*l == ',') l++;
  }
  return 0;
}
#else
static int verbose_group(const char *name) { (void)name; return 0; }
#endif
void mix(uint32_t v) {
  if (verbose) { puts_("  "); put_hex(v); puts_("\n"); }
  for (int i = 0; i < 4; i++) {
    digest ^= (v >> (8 * i)) & 0xFF;
    digest *= 16777619u;
  }
}

static uint32_t rng_state = 0x12345678;
uint32_t rnd(void) {
  uint32_t x = rng_state;
  x ^= x << 13; x ^= x >> 17; x ^= x << 5;
  rng_state = x;
  return x;
}

void begin(const char *name, unsigned namelen) {
  verbose = verbose_group(name);
  digest = 2166136261u;
  rng_state = 0x9E3779B9u ^ namelen;
}

static const uint32_t edges[] = {
  0, 1, 2, 0x7F, 0x80, 0xFF, 0x7FFF, 0x8000, 0xFFFF, 0x10000, 0x7FFFFFFF, 0x80000000, 0x80000001,
  0xFFFFFFFE, 0xFFFFFFFF, 0x55555555, 0xAAAAAAAA, 0x00FF00FF, 0xFF00FF00, 31, 32, 33,
};
#define NEDGE (sizeof(edges) / sizeof(edges[0]))
#define N 400

uint32_t operand(int i, int which) {
  if (i < (int)(NEDGE * NEDGE)) return which ? edges[i % NEDGE] : edges[i / NEDGE];
  return rnd();
}

/* binary operation with flags in and out (APSR NZCVQ and GE) */
#define OP2F(fn, insn)                                                                         \
  static uint32_t fn(uint32_t a, uint32_t b, uint32_t fin, uint32_t *fout) {                   \
    uint32_t r, f;                                                                             \
    __asm volatile("msr APSR_nzcvqg, %[fi]\n\t" insn "\n\tmrs %[f], APSR"                      \
                   : [r] "=&r"(r), [f] "=&r"(f) : [a] "r"(a), [b] "r"(b), [fi] "r"(fin) : "cc"); \
    *fout = f;                                                                                 \
    return r;                                                                                  \
  }

OP2F(op_adds, "adds %[r], %[a], %[b]")
OP2F(op_adcs, "adcs.w %[r], %[a], %[b]")
OP2F(op_subs, "subs %[r], %[a], %[b]")
OP2F(op_sbcs, "sbcs.w %[r], %[a], %[b]")
OP2F(op_rsbs, "rsbs.w %[r], %[a], %[b]")
OP2F(op_cmp, "cmp %[a], %[b]\n\tmov %[r], #0")
OP2F(op_cmn, "cmn %[a], %[b]\n\tmov %[r], #0")
OP2F(op_ands_lsl, "ands.w %[r], %[a], %[b], lsl #3")
OP2F(op_orrs_lsr, "orrs.w %[r], %[a], %[b], lsr #1")
OP2F(op_eors_asr, "eors.w %[r], %[a], %[b], asr #31")
OP2F(op_bics_ror, "bics.w %[r], %[a], %[b], ror #7")
OP2F(op_orns_rrx, "orns %[r], %[a], %[b], rrx")
OP2F(op_mvns_lsl, "mvns.w %[r], %[b], lsl #1")
OP2F(op_tst, "tst.w %[a], %[b], lsl #31\n\tmov %[r], #0")
OP2F(op_teq, "teq.w %[a], %[b], asr #2\n\tmov %[r], #0")
OP2F(op_adds_imm, "adds.w %[r], %[a], #0xFF000000")
OP2F(op_subs_imm, "subs.w %[r], %[a], #0x00AB00AB")
OP2F(op_ands_imm, "ands %[r], %[a], #0x80000000")
OP2F(op_lsls, "lsls.w %[r], %[a], %[b]")
OP2F(op_lsrs, "lsrs.w %[r], %[a], %[b]")
OP2F(op_asrs, "asrs.w %[r], %[a], %[b]")
OP2F(op_rors, "rors.w %[r], %[a], %[b]")
OP2F(op_lsls_imm, "lsls %[r], %[a], #17")
OP2F(op_lsrs_imm, "lsrs %[r], %[a], #32")
OP2F(op_asrs_imm, "asrs %[r], %[a], #32")
OP2F(op_rrxs, "rrxs %[r], %[a]")
static uint32_t op_muls(uint32_t a, uint32_t b, uint32_t fin, uint32_t *fout) {
  register uint32_t r __asm("r3");
  register uint32_t bb __asm("r2") = b;
  uint32_t f;
  __asm volatile("msr APSR_nzcvqg, %[fi]\n\tmovs r3, %[a]\n\tmuls r3, r2, r3\n\tmrs %[f], APSR"
                 : "=&l"(r), [f] "=&r"(f) : [a] "r"(a), "l"(bb), [fi] "r"(fin) : "cc");
  *fout = f;
  return r;
}
OP2F(op_qadd, "qadd %[r], %[a], %[b]")
OP2F(op_qsub, "qsub %[r], %[a], %[b]")
OP2F(op_qdadd, "qdadd %[r], %[a], %[b]")
OP2F(op_qdsub, "qdsub %[r], %[a], %[b]")
OP2F(op_ssat, "ssat %[r], #9, %[a], lsl #3")
OP2F(op_ssat2, "ssat %[r], #32, %[a], asr #5")
OP2F(op_usat, "usat %[r], #7, %[a], asr #2")
OP2F(op_usat0, "usat %[r], #0, %[a]")
OP2F(op_ssat16, "ssat16 %[r], #5, %[a]")
OP2F(op_usat16, "usat16 %[r], #9, %[a]")
OP2F(op_sadd16, "sadd16 %[r], %[a], %[b]")
OP2F(op_ssub16, "ssub16 %[r], %[a], %[b]")
OP2F(op_sasx, "sasx %[r], %[a], %[b]")
OP2F(op_ssax, "ssax %[r], %[a], %[b]")
OP2F(op_sadd8, "sadd8 %[r], %[a], %[b]")
OP2F(op_ssub8, "ssub8 %[r], %[a], %[b]")
OP2F(op_uadd16, "uadd16 %[r], %[a], %[b]")
OP2F(op_usub16, "usub16 %[r], %[a], %[b]")
OP2F(op_uasx, "uasx %[r], %[a], %[b]")
OP2F(op_usax, "usax %[r], %[a], %[b]")
OP2F(op_uadd8, "uadd8 %[r], %[a], %[b]")
OP2F(op_usub8, "usub8 %[r], %[a], %[b]")
OP2F(op_qadd16, "qadd16 %[r], %[a], %[b]")
OP2F(op_qsub16, "qsub16 %[r], %[a], %[b]")
OP2F(op_qasx, "qasx %[r], %[a], %[b]")
OP2F(op_qsax, "qsax %[r], %[a], %[b]")
OP2F(op_qadd8, "qadd8 %[r], %[a], %[b]")
OP2F(op_qsub8, "qsub8 %[r], %[a], %[b]")
OP2F(op_uqadd16, "uqadd16 %[r], %[a], %[b]")
OP2F(op_uqsub16, "uqsub16 %[r], %[a], %[b]")
OP2F(op_uqasx, "uqasx %[r], %[a], %[b]")
OP2F(op_uqsax, "uqsax %[r], %[a], %[b]")
OP2F(op_uqadd8, "uqadd8 %[r], %[a], %[b]")
OP2F(op_uqsub8, "uqsub8 %[r], %[a], %[b]")
OP2F(op_shadd16, "shadd16 %[r], %[a], %[b]")
OP2F(op_shsub16, "shsub16 %[r], %[a], %[b]")
OP2F(op_shasx, "shasx %[r], %[a], %[b]")
OP2F(op_shsax, "shsax %[r], %[a], %[b]")
OP2F(op_shadd8, "shadd8 %[r], %[a], %[b]")
OP2F(op_shsub8, "shsub8 %[r], %[a], %[b]")
OP2F(op_uhadd16, "uhadd16 %[r], %[a], %[b]")
OP2F(op_uhsub16, "uhsub16 %[r], %[a], %[b]")
OP2F(op_uhasx, "uhasx %[r], %[a], %[b]")
OP2F(op_uhsax, "uhsax %[r], %[a], %[b]")
OP2F(op_uhadd8, "uhadd8 %[r], %[a], %[b]")
OP2F(op_uhsub8, "uhsub8 %[r], %[a], %[b]")
OP2F(op_sel, "sel %[r], %[a], %[b]")
OP2F(op_smulbb, "smulbb %[r], %[a], %[b]")
OP2F(op_smultb, "smultb %[r], %[a], %[b]")
OP2F(op_smulbt, "smulbt %[r], %[a], %[b]")
OP2F(op_smultt, "smultt %[r], %[a], %[b]")
OP2F(op_smlabb, "smlabb %[r], %[a], %[b], %[a]")
OP2F(op_smlatt, "smlatt %[r], %[a], %[b], %[b]")
OP2F(op_smulwb, "smulwb %[r], %[a], %[b]")
OP2F(op_smulwt, "smulwt %[r], %[a], %[b]")
OP2F(op_smlawb, "smlawb %[r], %[a], %[b], %[a]")
OP2F(op_smlawt, "smlawt %[r], %[a], %[b], %[b]")
OP2F(op_smuad, "smuad %[r], %[a], %[b]")
OP2F(op_smuadx, "smuadx %[r], %[a], %[b]")
OP2F(op_smusd, "smusd %[r], %[a], %[b]")
OP2F(op_smusdx, "smusdx %[r], %[a], %[b]")
OP2F(op_smlad, "smlad %[r], %[a], %[b], %[a]")
OP2F(op_smladx, "smladx %[r], %[a], %[b], %[b]")
OP2F(op_smlsd, "smlsd %[r], %[a], %[b], %[a]")
OP2F(op_smlsdx, "smlsdx %[r], %[a], %[b], %[b]")
OP2F(op_smmul, "smmul %[r], %[a], %[b]")
OP2F(op_smmulr, "smmulr %[r], %[a], %[b]")
OP2F(op_smmla, "smmla %[r], %[a], %[b], %[a]")
OP2F(op_smmlar, "smmlar %[r], %[a], %[b], %[b]")
OP2F(op_smmls, "smmls %[r], %[a], %[b], %[a]")
OP2F(op_smmlsr, "smmlsr %[r], %[a], %[b], %[b]")
OP2F(op_usad8, "usad8 %[r], %[a], %[b]")
OP2F(op_usada8, "usada8 %[r], %[a], %[b], %[a]")
OP2F(op_mla, "mla %[r], %[a], %[b], %[a]")
OP2F(op_mls, "mls %[r], %[a], %[b], %[b]")
OP2F(op_sdiv, "sdiv %[r], %[a], %[b]")
OP2F(op_udiv, "udiv %[r], %[a], %[b]")
OP2F(op_clz, "clz %[r], %[a]")
OP2F(op_rbit, "rbit %[r], %[a]")
OP2F(op_rev, "rev %[r], %[a]")
OP2F(op_rev16, "rev16.w %[r], %[a]")
OP2F(op_revsh, "revsh.w %[r], %[a]")
OP2F(op_ubfx, "ubfx %[r], %[a], #5, #11")
OP2F(op_sbfx, "sbfx %[r], %[a], #13, #19")
OP2F(op_bfi, "mov %[r], %[b]\n\tbfi %[r], %[a], #7, #13")
OP2F(op_bfc, "mov %[r], %[a]\n\tbfc %[r], #3, #26")
OP2F(op_uxtab, "uxtab %[r], %[a], %[b], ror #8")
OP2F(op_sxtab, "sxtab %[r], %[a], %[b], ror #16")
OP2F(op_uxtah, "uxtah %[r], %[a], %[b], ror #24")
OP2F(op_sxtah, "sxtah %[r], %[a], %[b]")
OP2F(op_uxtab16, "uxtab16 %[r], %[a], %[b], ror #8")
OP2F(op_sxtab16, "sxtab16 %[r], %[a], %[b]")
OP2F(op_uxtb16, "uxtb16 %[r], %[b], ror #16")
OP2F(op_sxtb16, "sxtb16 %[r], %[b], ror #24")
OP2F(op_pkhbt, "pkhbt %[r], %[a], %[b], lsl #9")
OP2F(op_pkhtb, "pkhtb %[r], %[a], %[b], asr #12")
OP2F(op_movw_movt, "movw %[r], #0x1234\n\tmovt %[r], #0xABCD\n\tadd %[r], %[a]")
OP2F(op_it, "cmp %[a], %[b]\n\tite hi\n\tmovhi %[r], #1\n\tmovls %[r], #2\n\titt lt\n\taddlt %[r], %[r], #16\n\taddslt %[r], %[r], #0")
OP2F(op_it16, "mov %[r], %[a]\n\tcmp %[a], %[b]\n\titttt ne\n\taddne %[r], %[r], #1\n\tlslne %[r], %[r], #2\n\tsubne %[r], %[r], %[b]\n\teorne %[r], %[r], %[a]")
/* Armv8.1-M conditional select family (CSEL, CSINC, CSINV, CSNEG, CSET, CSETM, CINC, CINV, CNEG,
   with ZR operands) for one condition: the results are folded into one register. */
#define CSOP(fn, cond)                                                                          \
  static uint32_t fn(uint32_t a, uint32_t b, uint32_t fin, uint32_t *fout) {                    \
    uint32_t r, f, t;                                                                           \
    __asm volatile("msr APSR_nzcvqg, %[fi]\n\t"                                                 \
                   "csel %[r], %[a], %[b], " cond "\n\t"                                        \
                   "csinc %[t], %[a], %[b], " cond "\n\teor %[r], %[r], %[t], ror #3\n\t"       \
                   "csinv %[t], %[a], %[b], " cond "\n\teor %[r], %[r], %[t], ror #6\n\t"       \
                   "csneg %[t], %[a], %[b], " cond "\n\teor %[r], %[r], %[t], ror #9\n\t"       \
                   "cset %[t], " cond "\n\teor %[r], %[r], %[t], ror #12\n\t"                   \
                   "csetm %[t], " cond "\n\teor %[r], %[r], %[t], ror #15\n\t"                  \
                   "cinc %[t], %[a], " cond "\n\teor %[r], %[r], %[t], ror #18\n\t"             \
                   "cinv %[t], %[b], " cond "\n\teor %[r], %[r], %[t], ror #21\n\t"             \
                   "cneg %[t], %[a], " cond "\n\teor %[r], %[r], %[t], ror #24\n\t"             \
                   "csel %[t], zr, %[a], " cond "\n\teor %[r], %[r], %[t], ror #27\n\t"         \
                   "csinc %[t], %[b], zr, " cond "\n\teor %[r], %[r], %[t], ror #30\n\t"        \
                   "csneg %[t], zr, zr, " cond "\n\teor %[r], %[r], %[t], ror #1\n\t"           \
                   "mrs %[f], APSR"                                                             \
                   : [r] "=&r"(r), [f] "=&r"(f), [t] "=&r"(t)                                   \
                   : [a] "r"(a), [b] "r"(b), [fi] "r"(fin) : "cc");                             \
    *fout = f;                                                                                  \
    return r;                                                                                   \
  }
CSOP(op_cs_eq, "eq")
CSOP(op_cs_ne, "ne")
CSOP(op_cs_cs, "cs")
CSOP(op_cs_cc, "cc")
CSOP(op_cs_mi, "mi")
CSOP(op_cs_pl, "pl")
CSOP(op_cs_vs, "vs")
CSOP(op_cs_vc, "vc")
CSOP(op_cs_hi, "hi")
CSOP(op_cs_ls, "ls")
CSOP(op_cs_ge, "ge")
CSOP(op_cs_lt, "lt")
CSOP(op_cs_gt, "gt")
CSOP(op_cs_le, "le")
OP2F(op_cs_al, "csel %[r], %[a], %[b], al\n\tcsinc %[r], %[r], %[b], al\n\tcsinv %[r], %[r], %[a], al\n\tcsneg %[r], %[r], %[a], al")
/* PACG/AUTG as data operations: PAC of a with modifier b (keys set by run_pac) */
OP2F(op_pacg, "pacg %[r], %[a], %[b]\n\tautg %[r], %[a], %[b]\n\tpacg %[r], %[a], %[b]")
OP2F(op_pacg2, "pacg %[r], %[b], %[a]\n\tautg %[r], %[b], %[a]\n\tpacg %[r], %[b], %[a]")
/* PAC/AUT/BTI are NOPs while CONTROL.PAC_EN/BTI_EN are clear */
static uint32_t op_pac_nop(uint32_t a, uint32_t b, uint32_t fin, uint32_t *fout) {
  uint32_t r, f;
  __asm volatile("msr APSR_nzcvqg, %[fi]\n\tmov %[r], %[a]\n\tmov r12, %[b]\n\tpac r12, lr, sp\n\tbti\n\t"
                 "aut r12, lr, sp\n\teor %[r], %[r], r12\n\tmrs %[f], APSR"
                 : [r] "=&r"(r), [f] "=&r"(f) : [a] "r"(a), [b] "r"(b), [fi] "r"(fin) : "cc", "r12");
  *fout = f;
  return r;
}
/* TT with the SAU disabled: TT, TTT, TTA, TTAT of the same address */
OP2F(op_tt, "tt %[r], %[a]")
OP2F(op_ttt, "ttt %[r], %[a]")
OP2F(op_tta, "tta %[r], %[a]")
OP2F(op_ttat, "ttat %[r], %[a]")

typedef uint32_t (*op2f_t)(uint32_t, uint32_t, uint32_t, uint32_t *);

static void run_op2f(const char *name, op2f_t f, int divide) {
  BEGIN(name);
  for (int i = 0; i < N + (int)(NEDGE * NEDGE); i++) {
    uint32_t a = operand(i, 0), b = operand(i, 1);
    if (divide && b == 0) b = 3;
    uint32_t fin = (rnd() & 0xF80F0000u);
    uint32_t fout;
    uint32_t r = f(a, b, fin, &fout);
    mix(r);
    mix(fout);
  }
  report(name, digest);
}

/* long multiplies (64-bit results) */
#define OP4(fn, insn)                                                                           \
  static void fn(uint32_t a, uint32_t b, uint32_t *lo, uint32_t *hi) {                          \
    uint32_t l = *lo, h = *hi;                                                                  \
    __asm volatile(insn : [l] "+&r"(l), [h] "+&r"(h) : [a] "r"(a), [b] "r"(b));                 \
    *lo = l;                                                                                    \
    *hi = h;                                                                                    \
  }
OP4(op_umull, "umull %[l], %[h], %[a], %[b]")
OP4(op_smull, "smull %[l], %[h], %[a], %[b]")
OP4(op_umlal, "umlal %[l], %[h], %[a], %[b]")
OP4(op_smlal, "smlal %[l], %[h], %[a], %[b]")
OP4(op_umaal, "umaal %[l], %[h], %[a], %[b]")
OP4(op_smlalbb, "smlalbb %[l], %[h], %[a], %[b]")
OP4(op_smlaltb, "smlaltb %[l], %[h], %[a], %[b]")
OP4(op_smlald, "smlald %[l], %[h], %[a], %[b]")
OP4(op_smlaldx, "smlaldx %[l], %[h], %[a], %[b]")
OP4(op_smlsld, "smlsld %[l], %[h], %[a], %[b]")
OP4(op_smlsldx, "smlsldx %[l], %[h], %[a], %[b]")

typedef void (*op4_t)(uint32_t, uint32_t, uint32_t *, uint32_t *);

static void run_op4(const char *name, op4_t f) {
  BEGIN(name);
  for (int i = 0; i < N + (int)(NEDGE * NEDGE); i++) {
    uint32_t a = operand(i, 0), b = operand(i, 1), lo = rnd(), hi = rnd();
    f(a, b, &lo, &hi);
    mix(lo);
    mix(hi);
  }
  report(name, digest);
}

/* ------------------------------------------------------------------ floating point, single */

static const uint32_t fp_edges[] = {
  0x00000000, 0x80000000, 0x3F800000, 0xBF800000, 0x7F800000, 0xFF800000, 0x7FC00000, 0x7F800001,
  0xFFC00123, 0x00000001, 0x807FFFFF, 0x00800000, 0x7F7FFFFF, 0xFF7FFFFF, 0x4B000000, 0x4EFFFFFF,
  0x4F000000, 0xCF000000, 0x3EAAAAAB, 0x40490FDB, 0x34000000, 0x0C000000,
};
#define NFP (sizeof(fp_edges) / sizeof(fp_edges[0]))

uint32_t fp_operand(int i, int which) {
  if (i < (int)(NFP * NFP)) return which ? fp_edges[i % NFP] : fp_edges[i / NFP];
  uint32_t r = rnd();
  /* bias towards nearby exponents so that arithmetic is not dominated by overflow */
  if (r & 1) r = (r & 0x807FFFFF) | ((0x70 + (r >> 26)) << 23);
  return r;
}

#define FOP(fn, insn)                                                                           \
  static uint32_t fn(uint32_t a, uint32_t b, uint32_t c, uint32_t fpscr_in, uint32_t *fpscr_out) { \
    uint32_t r, st;                                                                             \
    __asm volatile("vmsr fpscr, %[fi]\n\t"                                                      \
                   "vmov s0, %[a]\n\tvmov s1, %[b]\n\tvmov s2, %[c]\n\t" insn                   \
                   "\n\tvmov %[r], s2\n\tvmrs %[st], fpscr"                                      \
                   : [r] "=&r"(r), [st] "=&r"(st) : [a] "r"(a), [b] "r"(b), [c] "r"(c), [fi] "r"(fpscr_in) \
                   : "s0", "s1", "s2", "cc");                                                   \
    *fpscr_out = st;                                                                            \
    return r;                                                                                   \
  }
FOP(f_add, "vadd.f32 s2, s0, s1")
FOP(f_sub, "vsub.f32 s2, s0, s1")
FOP(f_mul, "vmul.f32 s2, s0, s1")
FOP(f_nmul, "vnmul.f32 s2, s0, s1")
FOP(f_div, "vdiv.f32 s2, s0, s1")
FOP(f_sqrt, "vsqrt.f32 s2, s0")
FOP(f_abs, "vabs.f32 s2, s0")
FOP(f_neg, "vneg.f32 s2, s0")
FOP(f_mla, "vmla.f32 s2, s0, s1")
FOP(f_mls, "vmls.f32 s2, s0, s1")
FOP(f_nmla, "vnmla.f32 s2, s0, s1")
FOP(f_nmls, "vnmls.f32 s2, s0, s1")
FOP(f_fma, "vfma.f32 s2, s0, s1")
FOP(f_fms, "vfms.f32 s2, s0, s1")
FOP(f_fnma, "vfnma.f32 s2, s0, s1")
FOP(f_fnms, "vfnms.f32 s2, s0, s1")
FOP(f_cmp, "vcmp.f32 s0, s1\n\tvmrs APSR_nzcv, fpscr\n\tite gt\n\tvmovgt.f32 s2, s0\n\tvmovle.f32 s2, s1")
FOP(f_cmpe, "vcmpe.f32 s0, s1\n\tvmrs APSR_nzcv, fpscr\n\tit vs\n\tvmovvs.f32 s2, s0")
FOP(f_cmpz, "vcmp.f32 s0, #0\n\tvmrs APSR_nzcv, fpscr\n\tit mi\n\tvmovmi.f32 s2, s1")
FOP(f_cvt_s32, "vcvt.s32.f32 s2, s0")
FOP(f_cvt_u32, "vcvt.u32.f32 s2, s0")
FOP(f_cvtr_s32, "vcvtr.s32.f32 s2, s0")
FOP(f_cvtr_u32, "vcvtr.u32.f32 s2, s0")
FOP(f_cvt_f_s32, "vcvt.f32.s32 s2, s0")
FOP(f_cvt_f_u32, "vcvt.f32.u32 s2, s0")
FOP(f_cvt_fix_s16, "vmov.f32 s2, s0\n\tvcvt.s16.f32 s2, s2, #7")
FOP(f_cvt_fix_u32, "vmov.f32 s2, s0\n\tvcvt.u32.f32 s2, s2, #20")
FOP(f_cvt_fix_to_f, "vmov.f32 s2, s0\n\tvcvt.f32.s32 s2, s2, #10")
FOP(f_cvt_fix_u16_to_f, "vmov.f32 s2, s0\n\tvcvt.f32.u16 s2, s2, #3")
FOP(f_cvtb_h, "vmov.f32 s2, s1\n\tvcvtb.f16.f32 s2, s0")
FOP(f_cvtt_h, "vmov.f32 s2, s1\n\tvcvtt.f16.f32 s2, s0")
FOP(f_cvtb_s, "vcvtb.f32.f16 s2, s0")
FOP(f_cvtt_s, "vcvtt.f32.f16 s2, s0")
FOP(f_movimm, "vmov.f32 s2, #-1.5\n\tvadd.f32 s2, s2, s0")
/* FPv5 additions (Armv8-M) */
FOP(f_selgt, "vcmp.f32 s0, s1\n\tvmrs APSR_nzcv, fpscr\n\tvselgt.f32 s2, s0, s1")
FOP(f_selge, "vcmpe.f32 s0, s1\n\tvmrs APSR_nzcv, fpscr\n\tvselge.f32 s2, s0, s1")
FOP(f_seleq, "vcmp.f32 s0, s1\n\tvmrs APSR_nzcv, fpscr\n\tvseleq.f32 s2, s0, s1")
FOP(f_selvs, "vcmp.f32 s0, s1\n\tvmrs APSR_nzcv, fpscr\n\tvselvs.f32 s2, s0, s1")
FOP(f_maxnm, "vmaxnm.f32 s2, s0, s1")
FOP(f_minnm, "vminnm.f32 s2, s0, s1")
FOP(f_rinta, "vrinta.f32 s2, s0")
FOP(f_rintn, "vrintn.f32 s2, s0")
FOP(f_rintp, "vrintp.f32 s2, s0")
FOP(f_rintm, "vrintm.f32 s2, s0")
FOP(f_rintz, "vrintz.f32 s2, s0")
FOP(f_rintx, "vrintx.f32 s2, s0")
FOP(f_rintr, "vrintr.f32 s2, s0")
FOP(f_cvta_s32, "vcvta.s32.f32 s2, s0")
FOP(f_cvtn_s32, "vcvtn.s32.f32 s2, s0")
FOP(f_cvtp_s32, "vcvtp.s32.f32 s2, s0")
FOP(f_cvtm_s32, "vcvtm.s32.f32 s2, s0")
FOP(f_cvta_u32, "vcvta.u32.f32 s2, s0")
FOP(f_cvtn_u32, "vcvtn.u32.f32 s2, s0")
FOP(f_cvtp_u32, "vcvtp.u32.f32 s2, s0")
FOP(f_cvtm_u32, "vcvtm.u32.f32 s2, s0")

typedef uint32_t (*fop_t)(uint32_t, uint32_t, uint32_t, uint32_t, uint32_t *);

/* FPSCR.LTPSIZE (bits 18:16) is 0b100 outside tail-predicated loops; keep it so. */
#ifndef LTP
#define LTP 0x00040000u
#endif
static const uint32_t fpscr_modes[] = {
  LTP | 0x00000000,             /* RN */
  LTP | 0x00400000,             /* RP */
  LTP | 0x00800000,             /* RM */
  LTP | 0x00C00000,             /* RZ */
  LTP | 0x01000000,             /* FZ */
  LTP | 0x02000000,             /* DN */
  LTP | 0x07C00000,             /* AHP DN FZ RZ */
  LTP | 0x00080000,             /* FZ16 */
  LTP | 0x02480000,             /* DN FZ16 RP */
};
#define NMODES (sizeof(fpscr_modes) / sizeof(fpscr_modes[0]))

static void run_fop(const char *name, fop_t f) {
  BEGIN(name);
  for (unsigned m = 0; m < 7; m++) {
    for (int i = 0; i < 150 + (int)(NFP * NFP); i++) {
      uint32_t a = fp_operand(i, 0), b = fp_operand(i, 1), c = fp_operand(i + 7, 0);
      uint32_t st;
      uint32_t r = f(a, b, c, fpscr_modes[m] | (rnd() & 0x9F), &st);
      mix(r);
      mix(st);
    }
  }
  report(name, digest);
}

/* ------------------------------------------------------------------ floating point, double and half */

static const uint64_t dp_edges[] = {
  0x0000000000000000ull, 0x8000000000000000ull, 0x3FF0000000000000ull, 0xBFF0000000000000ull,
  0x7FF0000000000000ull, 0xFFF0000000000000ull, 0x7FF8000000000000ull, 0x7FF0000000000001ull,
  0xFFF8000000000123ull, 0x0000000000000001ull, 0x000FFFFFFFFFFFFFull, 0x0010000000000000ull,
  0x7FEFFFFFFFFFFFFFull, 0xFFEFFFFFFFFFFFFFull, 0x4330000000000000ull, 0x41DFFFFFFFC00000ull,
  0x41E0000000000000ull, 0xC1E0000000000000ull, 0x3FD5555555555555ull, 0x400921FB54442D18ull,
  0x3E70000000000000ull, 0x47EFFFFFE0000001ull,
};
#define NDP (sizeof(dp_edges) / sizeof(dp_edges[0]))

static const uint16_t hp_edges[] = {
  0x0000, 0x8000, 0x3C00, 0xBC00, 0x7C00, 0xFC00, 0x7E00, 0x7C01, 0xFE55, 0x0001, 0x03FF, 0x0400,
  0x7BFF, 0xFBFF, 0x3555, 0x4248, 0x5640, 0x6400, 0x7800, 0xE400, 0x3800, 0x3E00,
};
#define NHP (sizeof(hp_edges) / sizeof(hp_edges[0]))

static uint64_t dp_random(void) {
  uint32_t lo = rnd(), hi = rnd();
  if (hi & 1) hi = (hi & 0x800FFFFF) | ((0x3F0 + ((hi >> 21) & 0x3F)) << 20);
  return ((uint64_t)hi << 32) | lo;
}

static uint64_t dp_operand(int i, int which) {
  if (i < (int)(NDP * NDP)) return which ? dp_edges[i % NDP] : dp_edges[i / NDP];
  return dp_random();
}

static uint32_t hp_operand(int i, int which) {
  if (i < (int)(NHP * NHP)) return which ? hp_edges[i % NHP] : hp_edges[i / NHP];
  uint32_t r = rnd() & 0xFFFF;
  if (r & 1) r = (r & 0x83FF) | ((0x8 + ((r >> 12) & 0xF)) << 10);
  return r;
}

#define PACK(lo, hi) (((uint64_t)(uint32_t)(hi) << 32) | (uint32_t)(lo))

/* Operand kinds for run_dop: what goes into d0 (= s0:s1), d1 (= s2:s3) and d2 (= s4:s5). */
enum { K_DOUBLE, K_SINGLE, K_HALF, K_INT };

/* d0 = a, d1 = b, d2 = c; the instruction writes d2 (or s4); the result is d2. */
#define DOP(fn, insn)                                                                           \
  static uint64_t fn(uint64_t a, uint64_t b, uint64_t c, uint32_t fpscr_in, uint32_t *fpscr_out) { \
    uint32_t rl, rh, st;                                                                        \
    __asm volatile("vmsr fpscr, %[fi]\n\t"                                                      \
                   "vmov d0, %Q[a], %R[a]\n\tvmov d1, %Q[b], %R[b]\n\tvmov d2, %Q[c], %R[c]\n\t" insn \
                   "\n\tvmov %[rl], %[rh], d2\n\tvmrs %[st], fpscr"                             \
                   : [rl] "=&r"(rl), [rh] "=&r"(rh), [st] "=&r"(st)                             \
                   : [a] "r"(a), [b] "r"(b), [c] "r"(c), [fi] "r"(fpscr_in)                     \
                   : "d0", "d1", "d2", "cc");                                                   \
    *fpscr_out = st;                                                                            \
    return PACK(rl, rh);                                                                        \
  }
DOP(d_add, "vadd.f64 d2, d0, d1")
DOP(d_sub, "vsub.f64 d2, d0, d1")
DOP(d_mul, "vmul.f64 d2, d0, d1")
DOP(d_nmul, "vnmul.f64 d2, d0, d1")
DOP(d_div, "vdiv.f64 d2, d0, d1")
DOP(d_sqrt, "vsqrt.f64 d2, d0")
DOP(d_abs, "vabs.f64 d2, d0")
DOP(d_neg, "vneg.f64 d2, d0")
DOP(d_mla, "vmla.f64 d2, d0, d1")
DOP(d_mls, "vmls.f64 d2, d0, d1")
DOP(d_nmla, "vnmla.f64 d2, d0, d1")
DOP(d_nmls, "vnmls.f64 d2, d0, d1")
DOP(d_fma, "vfma.f64 d2, d0, d1")
DOP(d_fms, "vfms.f64 d2, d0, d1")
DOP(d_fnma, "vfnma.f64 d2, d0, d1")
DOP(d_fnms, "vfnms.f64 d2, d0, d1")
DOP(d_cmp, "vcmp.f64 d0, d1\n\tvmrs APSR_nzcv, fpscr\n\tite gt\n\tvmovgt.f64 d2, d0\n\tvmovle.f64 d2, d1")
DOP(d_cmpe, "vcmpe.f64 d0, d1\n\tvmrs APSR_nzcv, fpscr\n\tit vs\n\tvmovvs.f64 d2, d0")
DOP(d_cmpz, "vcmp.f64 d0, #0\n\tvmrs APSR_nzcv, fpscr\n\tit mi\n\tvmovmi.f64 d2, d1\n\tvcmpe.f64 d1, #0\n\tvmrs APSR_nzcv, fpscr\n\tit eq\n\tvnegeq.f64 d2, d2")
DOP(d_selgt, "vcmp.f64 d0, d1\n\tvmrs APSR_nzcv, fpscr\n\tvselgt.f64 d2, d0, d1")
DOP(d_selge, "vcmpe.f64 d0, d1\n\tvmrs APSR_nzcv, fpscr\n\tvselge.f64 d2, d0, d1")
DOP(d_seleq, "vcmp.f64 d0, d1\n\tvmrs APSR_nzcv, fpscr\n\tvseleq.f64 d2, d0, d1")
DOP(d_selvs, "vcmp.f64 d0, d1\n\tvmrs APSR_nzcv, fpscr\n\tvselvs.f64 d2, d0, d1")
DOP(d_maxnm, "vmaxnm.f64 d2, d0, d1")
DOP(d_minnm, "vminnm.f64 d2, d0, d1")
DOP(d_rinta, "vrinta.f64 d2, d0")
DOP(d_rintn, "vrintn.f64 d2, d0")
DOP(d_rintp, "vrintp.f64 d2, d0")
DOP(d_rintm, "vrintm.f64 d2, d0")
DOP(d_rintz, "vrintz.f64 d2, d0")
DOP(d_rintx, "vrintx.f64 d2, d0")
DOP(d_rintr, "vrintr.f64 d2, d0")
DOP(d_cvta_s32, "vcvta.s32.f64 s4, d0")
DOP(d_cvtn_s32, "vcvtn.s32.f64 s4, d0")
DOP(d_cvtp_s32, "vcvtp.s32.f64 s4, d0")
DOP(d_cvtm_s32, "vcvtm.s32.f64 s4, d0")
DOP(d_cvta_u32, "vcvta.u32.f64 s4, d0")
DOP(d_cvtn_u32, "vcvtn.u32.f64 s4, d0")
DOP(d_cvtp_u32, "vcvtp.u32.f64 s4, d0")
DOP(d_cvtm_u32, "vcvtm.u32.f64 s4, d0")
DOP(d_cvt_s32, "vcvt.s32.f64 s4, d0")
DOP(d_cvt_u32, "vcvt.u32.f64 s4, d0")
DOP(d_cvtr_s32, "vcvtr.s32.f64 s4, d0")
DOP(d_cvtr_u32, "vcvtr.u32.f64 s4, d0")
DOP(d_cvt_f_s32, "vcvt.f64.s32 d2, s0")           /* K_INT */
DOP(d_cvt_f_u32, "vcvt.f64.u32 d2, s1")           /* K_INT */
DOP(d_cvt_fix_s16, "vmov.f64 d2, d0\n\tvcvt.s16.f64 d2, d2, #7")
DOP(d_cvt_fix_u32, "vmov.f64 d2, d0\n\tvcvt.u32.f64 d2, d2, #20")
DOP(d_cvt_fix_to_f, "vmov.f64 d2, d0\n\tvcvt.f64.s32 d2, d2, #10")   /* K_INT */
DOP(d_cvt_fix_u16_to_f, "vmov.f64 d2, d0\n\tvcvt.f64.u16 d2, d2, #3") /* K_INT */
DOP(d_cvt_f32_f64, "vcvt.f32.f64 s4, d0")
DOP(d_cvt_f64_f32, "vcvt.f64.f32 d2, s0")        /* K_SINGLE */
DOP(d_cvtb_f16_f64, "vcvtb.f16.f64 s4, d0")
DOP(d_cvtt_f16_f64, "vcvtt.f16.f64 s4, d0")
DOP(d_cvtb_f64_f16, "vcvtb.f64.f16 d2, s0")      /* K_HALF */
DOP(d_cvtt_f64_f16, "vcvtt.f64.f16 d2, s0")      /* K_HALF */
DOP(d_movimm, "vmov.f64 d2, #2.5\n\tvadd.f64 d2, d2, d0\n\tvmov.f64 d1, #-0.125\n\tvmul.f64 d2, d2, d1")
DOP(d_movreg, "vmov.f64 d2, d0\n\tvmov s5, s3\n\tvmov %[rl], s4\n\tvmov s3, %[rl]\n\tvmov %[rl], %[rh], d1\n\tvmov s4, %[rh]")
/* half precision scalar (K_HALF: s0 = a, s1 = b; the result is s4 with s5 = c.hi) */
DOP(h_add, "vadd.f16 s4, s0, s1")
DOP(h_sub, "vsub.f16 s4, s0, s1")
DOP(h_mul, "vmul.f16 s4, s0, s1")
DOP(h_nmul, "vnmul.f16 s4, s0, s1")
DOP(h_div, "vdiv.f16 s4, s0, s1")
DOP(h_sqrt, "vsqrt.f16 s4, s0")
DOP(h_abs, "vabs.f16 s4, s0")
DOP(h_neg, "vneg.f16 s4, s0")
DOP(h_mla, "vmla.f16 s4, s0, s1")
DOP(h_mls, "vmls.f16 s4, s0, s1")
DOP(h_nmla, "vnmla.f16 s4, s0, s1")
DOP(h_nmls, "vnmls.f16 s4, s0, s1")
DOP(h_fma, "vfma.f16 s4, s0, s1")
DOP(h_fms, "vfms.f16 s4, s0, s1")
DOP(h_fnma, "vfnma.f16 s4, s0, s1")
DOP(h_fnms, "vfnms.f16 s4, s0, s1")
DOP(h_cmp, "vcmp.f16 s0, s1\n\tvmrs APSR_nzcv, fpscr\n\tite gt\n\tvmovgt.f32 s4, s0\n\tvmovle.f32 s4, s1")
DOP(h_cmpe, "vcmpe.f16 s0, s1\n\tvmrs APSR_nzcv, fpscr\n\tit vs\n\tvmovvs.f32 s4, s0")
DOP(h_cmpz, "vcmp.f16 s0, #0\n\tvmrs APSR_nzcv, fpscr\n\tit mi\n\tvmovmi.f32 s4, s1\n\tvcmpe.f16 s1, #0\n\tvmrs APSR_nzcv, fpscr\n\tit eq\n\tvmoveq.f32 s4, s0")
DOP(h_selgt, "vcmp.f16 s0, s1\n\tvmrs APSR_nzcv, fpscr\n\tvselgt.f16 s4, s0, s1")
DOP(h_selvs, "vcmpe.f16 s0, s1\n\tvmrs APSR_nzcv, fpscr\n\tvselvs.f16 s4, s0, s1")
DOP(h_maxnm, "vmaxnm.f16 s4, s0, s1")
DOP(h_minnm, "vminnm.f16 s4, s0, s1")
DOP(h_rinta, "vrinta.f16 s4, s0")
DOP(h_rintn, "vrintn.f16 s4, s0")
DOP(h_rintp, "vrintp.f16 s4, s0")
DOP(h_rintm, "vrintm.f16 s4, s0")
DOP(h_rintz, "vrintz.f16 s4, s0")
DOP(h_rintx, "vrintx.f16 s4, s0")
DOP(h_rintr, "vrintr.f16 s4, s0")
DOP(h_cvta_s32, "vcvta.s32.f16 s4, s0")
DOP(h_cvtn_s32, "vcvtn.s32.f16 s4, s0")
DOP(h_cvtp_u32, "vcvtp.u32.f16 s4, s0")
DOP(h_cvtm_u32, "vcvtm.u32.f16 s4, s0")
DOP(h_cvt_s32, "vcvt.s32.f16 s4, s0")
DOP(h_cvt_u32, "vcvt.u32.f16 s4, s0")
DOP(h_cvtr_s32, "vcvtr.s32.f16 s4, s0")
DOP(h_cvtr_u32, "vcvtr.u32.f16 s4, s0")
DOP(h_cvt_f_s32, "vcvt.f16.s32 s4, s0")          /* K_INT */
DOP(h_cvt_f_u32, "vcvt.f16.u32 s4, s1")          /* K_INT */
DOP(h_movimm, "vmov.f16 s4, #1.5\n\tvadd.f16 s4, s4, s0\n\tvmov.f16 s5, #-3.0\n\tvmul.f16 s4, s4, s5")
DOP(h_ins_movx, "vmov.f32 s4, s0\n\tvins.f16 s4, s1\n\tvmovx.f16 s5, s4\n\tvadd.f16 s5, s5, s1")
DOP(h_movreg, "vmov.f16 s4, %Q[a]\n\tvmov.f16 %[rl], s1\n\tvmov s5, %[rl]")

typedef uint64_t (*dop_t)(uint64_t, uint64_t, uint64_t, uint32_t, uint32_t *);

static void run_dop(const char *name, dop_t f, int kind) {
  BEGIN(name);
  for (unsigned m = 0; m < NMODES; m++) {
    for (int i = 0; i < 150 + 22 * 22; i++) {
      uint64_t a, b, c;
      switch (kind) {
        case K_DOUBLE:
          a = dp_operand(i, 0); b = dp_operand(i, 1); c = dp_operand(i + 7, 0);
          break;
        case K_SINGLE:
          a = PACK(fp_operand(i, 0), fp_operand(i, 1));
          b = PACK(fp_operand(i + 3, 1), fp_operand(i + 5, 0));
          c = PACK(fp_operand(i + 7, 0), rnd());
          break;
        case K_HALF:
          a = PACK(hp_operand(i, 0) | (rnd() << 16), hp_operand(i, 1) | (rnd() << 16));
          b = PACK(hp_operand(i + 3, 1) | (rnd() << 16), hp_operand(i + 5, 0) | (rnd() << 16));
          c = PACK(hp_operand(i + 7, 0) | (rnd() << 16), rnd());
          break;
        default:
          a = PACK(operand(i, 0), operand(i, 1));
          b = PACK(operand(i + 3, 1), operand(i + 5, 0));
          c = PACK(operand(i + 7, 0), rnd());
          break;
      }
      uint32_t st;
      uint64_t r = f(a, b, c, fpscr_modes[m] | (rnd() & 0x9F), &st);
      mix((uint32_t)r);
      mix((uint32_t)(r >> 32));
      mix(st);
    }
  }
  report(name, digest);
}

/* VLDR.16 / VSTR.16 through a byte buffer */
static uint8_t hbuf[64] __attribute__((aligned(4)));

static void run_half_memory(void) {
  BEGIN("h_vldr_vstr");
  for (int i = 0; i < 64; i++) hbuf[i] = (uint8_t)(i * 29 + 5);
  for (int i = 0; i < 80; i++) {
    uint32_t off = (rnd() % 56) & ~1u, v, w = rnd();
    __asm volatile("vldr.16 s0, [%[p], #2]\n\tvmov %[v], s0\n\tvmov s1, %[w]\n\tvstr.16 s1, [%[p], #-2]\n\t"
                   "vldr.16 s2, [%[p]]\n\tvadd.f16 s2, s2, s0\n\tvstr.16 s2, [%[p], #4]"
                   : [v] "=&r"(v) : [p] "r"(hbuf + 2 + off), [w] "r"(w) : "s0", "s1", "s2", "memory");
    mix(v);
  }
  for (int i = 0; i < 64; i += 4) mix(*(uint32_t *)(hbuf + i));
  report("h_vldr_vstr", digest);
}

/* ------------------------------------------------------------------ memory access */

static uint8_t buf[256] __attribute__((aligned(8)));

static void run_memory(void) {
  BEGIN("memory");
  for (int i = 0; i < 256; i++) buf[i] = (uint8_t)(i * 37 + 11);
  for (int i = 0; i < 200; i++) {
    uint32_t off = rnd() % 200, v1, v2, v3, v4, v5;
    uint8_t *p = buf + off;
    __asm volatile("ldr %[v1], [%[p]]\n\tldrh %[v2], [%[p], #1]\n\tldrsh %[v3], [%[p], #3]\n\t"
                   "ldrsb %[v4], [%[p], #2]\n\tldrb.w %[v5], [%[p], #5]"
                   : [v1] "=&r"(v1), [v2] "=&r"(v2), [v3] "=&r"(v3), [v4] "=&r"(v4), [v5] "=&r"(v5)
                   : [p] "r"(p) : "memory");
    mix(v1); mix(v2); mix(v3); mix(v4); mix(v5);
    uint32_t x = rnd();
    uint32_t *wp;
    __asm volatile("mov %[w], %[p]\n\tstr %[x], [%[w], #4]!\n\tstrh %[x], [%[w]], #6\n\tstrb %[x], [%[w], #-3]"
                   : [w] "=&r"(wp) : [p] "r"(buf + (off & ~7u) + 8), [x] "r"(x) : "memory");
    mix((uint32_t)((uint8_t *)wp - buf));
  }
  /* ldm/stm, ldrd/strd, push/pop */
  uint32_t a[8] __attribute__((aligned(8)));
  for (int i = 0; i < 8; i++) a[i] = rnd();
  uint32_t r0, r1, r2, r3, *pp = a;
  __asm volatile("ldmia %[p]!, {%[r0], %[r1], %[r2], %[r3]}" : [p] "+r"(pp), [r0] "=r"(r0), [r1] "=r"(r1), [r2] "=r"(r2), [r3] "=r"(r3));
  mix(r0); mix(r1); mix(r2); mix(r3); mix((uint32_t)(pp - a));
  __asm volatile("ldmdb %[p]!, {%[r0], %[r1]}" : [p] "+r"(pp), [r0] "=r"(r0), [r1] "=r"(r1));
  mix(r0); mix(r1); mix((uint32_t)(pp - a));
  __asm volatile("stmdb %[p]!, {%[r0], %[r1]}" : [p] "+r"(pp) : [r0] "r"(0x11111111), [r1] "r"(0x22222222) : "memory");
  __asm volatile("ldrd %[r0], %[r1], [%[p], #8]\n\tstrd %[r1], %[r0], [%[p]], #4" : [r0] "=&r"(r0), [r1] "=&r"(r1), [p] "+r"(pp) : : "memory");
  for (int i = 0; i < 8; i++) mix(a[i]);
  mix((uint32_t)(pp - a));
  /* exclusive monitor */
  uint32_t ex = 5, st1, st2, st3;
  __asm volatile("ldrex %[v], [%[p]]\n\tadd %[v], #1\n\tstrex %[s1], %[v], [%[p]]\n\tstrex %[s2], %[v], [%[p]]\n\t"
                 "ldrex %[v], [%[p]]\n\tclrex\n\tstrex %[s3], %[v], [%[p]]"
                 : [v] "=&r"(r0), [s1] "=&r"(st1), [s2] "=&r"(st2), [s3] "=&r"(st3) : [p] "r"(&ex) : "memory");
  mix(ex); mix(st1); mix(st2); mix(st3);
  report("memory", digest);
}

/* Armv8-M load-acquire/store-release, byte and halfword exclusives */
static void run_memory_v8(void) {
  BEGIN("memory_v8");
  for (int i = 0; i < 256; i++) buf[i] = (uint8_t)(i * 53 + 7);
  for (int i = 0; i < 100; i++) {
    uint32_t off = (rnd() % 200) & ~3u, v1, v2, v3, x = rnd();
    uint8_t *p = buf + off;
    __asm volatile("lda %[v1], [%[p]]\n\tldab %[v2], [%[q]]\n\tldah %[v3], [%[r]]\n\t"
                   "stl %[x], [%[r]]\n\tstlb %[x], [%[p]]\n\tstlh %[x], [%[q]]"
                   : [v1] "=&r"(v1), [v2] "=&r"(v2), [v3] "=&r"(v3)
                   : [p] "r"(p), [q] "r"(p + 5), [r] "r"(p + 2 + (off & 4)), [x] "r"(x) : "memory");
    mix(v1); mix(v2); mix(v3);
  }
  uint32_t v, s1, s2, s3, s4, s5;
  __asm volatile("ldaex %[v], [%[p]]\n\tadd %[v], #3\n\tstlex %[s1], %[v], [%[p]]\n\tstlex %[s2], %[v], [%[p]]\n\t"
                 "ldrexb %[v], [%[q]]\n\tadd %[v], #1\n\tstrexb %[s3], %[v], [%[q]]\n\t"
                 "ldrexh %[v], [%[p]]\n\tadd %[v], #1\n\tstrexh %[s4], %[v], [%[p]]\n\t"
                 "ldaexb %[v], [%[q]]\n\tldaexh %[v], [%[p]]\n\tadd %[v], #7\n\tstlexh %[s5], %[v], [%[p]]"
                 : [v] "=&r"(v), [s1] "=&r"(s1), [s2] "=&r"(s2), [s3] "=&r"(s3), [s4] "=&r"(s4), [s5] "=&r"(s5)
                 : [p] "r"(buf + 8), [q] "r"(buf + 13) : "memory");
  mix(v); mix(s1); mix(s2); mix(s3); mix(s4); mix(s5);
  for (int i = 0; i < 256; i += 4) mix(*(uint32_t *)(buf + i));
  report("memory_v8", digest);
}

/* table branch through a dense switch (TBB/TBH) */
__attribute__((noinline)) static uint32_t sw(uint32_t x) {
  switch (x) {
    case 0: return 11; case 1: return 23; case 2: return 37; case 3: return 41; case 4: return 59;
    case 5: return 61; case 6: return 79; case 7: return 83; case 8: return 97; case 9: return 101;
    case 10: return 113; case 11: return 127; case 12: return 131; case 13: return 149; default: return 7;
  }
}

static void run_branches(void) {
  BEGIN("branches");
  for (uint32_t i = 0; i < 40; i++) mix(sw(i % 17));
  uint32_t r;
  __asm volatile("mov %[r], #0\n\tcbz %[r], 1f\n\tmov %[r], #5\n1:\tadd %[r], #1\n\tcbnz %[r], 2f\n\tmov %[r], #9\n2:" : [r] "=&r"(r));
  mix(r);
  uint32_t idx = 3, res;
  __asm volatile(
      "tbh [pc, %[i], lsl #1]\n"
      "3: .hword (4f-3b)/2, (5f-3b)/2, (6f-3b)/2, (7f-3b)/2\n"
      "4: mov %[r], #40\n b 8f\n"
      "5: mov %[r], #50\n b 8f\n"
      "6: mov %[r], #60\n b 8f\n"
      "7: mov %[r], #70\n"
      "8:"
      : [r] "=&r"(res) : [i] "r"(idx));
  mix(res);
  report("branches", digest);
}

/* ------------------------------------------------------------------ Armv8.1-M: loops, CLRM, system registers */

static void run_lob(void) {
  BEGIN("lob");
  static const uint32_t counts[] = { 1, 2, 3, 17, 100, 1000 };
  for (unsigned k = 0; k < sizeof(counts) / sizeof(counts[0]); k++) {
    uint32_t acc = rnd(), i = 0, n = counts[k];
    /* DLS/LE: the body modifies registers and flags */
    __asm volatile("dls lr, %[n]\n"
                   "1:\tadd %[acc], %[acc], %[i]\n\teor %[acc], %[acc], %[acc], ror #5\n\tadds %[i], #1\n\t"
                   "it cs\n\taddcs %[acc], #1\n\tle lr, 1b"
                   : [acc] "+r"(acc), [i] "+r"(i) : [n] "r"(n) : "lr", "cc");
    mix(acc); mix(i);
  }
  static const uint32_t wcounts[] = { 0, 1, 5, 64, 0, 257 };
  for (unsigned k = 0; k < sizeof(wcounts) / sizeof(wcounts[0]); k++) {
    uint32_t acc = rnd(), i = 0, n = wcounts[k];
    /* WLS/LE: a count of 0 skips the body */
    __asm volatile("wls lr, %[n], 2f\n"
                   "1:\tmla %[acc], %[acc], %[i], %[n]\n\tadd %[i], #3\n\tle lr, 1b\n"
                   "2:\tadd %[acc], %[acc], %[i]"
                   : [acc] "+r"(acc), [i] "+r"(i) : [n] "r"(n) : "lr", "cc");
    mix(acc); mix(i);
  }
  /* nested: outer DLS, inner WLS, LR saved around the inner loop */
  for (unsigned k = 0; k < 4; k++) {
    uint32_t acc = rnd(), i = 0, n = 3 + k, m = (k * 5) % 7, t;
    __asm volatile("dls lr, %[n]\n"
                   "1:\tmov %[t], lr\n\twls lr, %[m], 3f\n"
                   "2:\tadd %[acc], %[acc], %[i]\n\tadd %[i], #1\n\tle lr, 2b\n"
                   "3:\tmov lr, %[t]\n\teor %[acc], %[acc], %[acc], lsl #3\n\tle lr, 1b"
                   : [acc] "+r"(acc), [i] "+r"(i), [t] "=&r"(t) : [n] "r"(n), [m] "r"(m) : "lr", "cc");
    mix(acc); mix(i);
  }
  /* LE leaves LR at 0; a loop exited by a branch leaves the remaining count */
  {
    uint32_t lr_after, i = 0;
    __asm volatile("dls lr, %[n]\n1:\tadd %[i], #1\n\tcmp %[i], #4\n\tbeq 2f\n\tle lr, 1b\n2:\tmov %[l], lr"
                   : [i] "+r"(i), [l] "=&r"(lr_after) : [n] "r"(10) : "lr", "cc");
    mix(lr_after); mix(i);
  }
  report("lob", digest);
}

static void run_clrm(void) {
  BEGIN("clrm");
  for (int i = 0; i < 20; i++) {
    register uint32_t r4 __asm("r4") = rnd();
    register uint32_t r5 __asm("r5") = rnd();
    register uint32_t r6 __asm("r6") = rnd();
    register uint32_t r8 __asm("r8") = rnd();
    uint32_t fin = rnd() & 0xF80F0000u, f, t, l;
    __asm volatile("msr APSR_nzcvqg, %[fi]\n\tmov %[t], lr\n\tclrm {r4, r5, r8, lr, apsr}\n\tmov %[l], lr\n\tmov lr, %[t]\n\tmrs %[f], APSR"
                   : "+r"(r4), "+r"(r5), "+r"(r6), "+r"(r8), [f] "=&r"(f), [t] "=&r"(t), [l] "=&r"(l)
                   : [fi] "r"(fin) : "cc", "lr");
    mix(r4); mix(r5); mix(r6); mix(r8); mix(f); mix(l);
    /* CLRM without APSR keeps the flags */
    __asm volatile("msr APSR_nzcvqg, %[fi]\n\tclrm {r6}\n\tmrs %[f], APSR" : "+r"(r6), [f] "=&r"(f) : [fi] "r"(fin) : "cc");
    mix(r6); mix(f);
  }
  /* VSCCLRM: clears the listed FP registers and FPSCR */
  uint32_t v0, v1, v2, v3, v4, st;
  __asm volatile("vmsr fpscr, %[fi]\n\tvmov s0, %[a]\n\tvmov s1, %[b]\n\tvmov s2, %[a]\n\tvmov s3, %[b]\n\tvmov s4, %[a]\n\t"
                 "vscclrm {s1-s3, vpr}\n\t"
                 "vmov %[v0], s0\n\tvmov %[v1], s1\n\tvmov %[v2], s2\n\tvmov %[v3], s3\n\tvmov %[v4], s4\n\tvmrs %[st], fpscr"
                 : [v0] "=&r"(v0), [v1] "=&r"(v1), [v2] "=&r"(v2), [v3] "=&r"(v3), [v4] "=&r"(v4), [st] "=&r"(st)
                 : [a] "r"(0x3F800000), [b] "r"(0x40000000), [fi] "r"(LTP | 0x0040009F) : "s0", "s1", "s2", "s3", "s4");
  mix(v0); mix(v1); mix(v2); mix(v3); mix(v4); mix(st);
  uint64_t q5, q6;
  __asm volatile("vmov d5, %Q[a], %R[a]\n\tvmov d6, %Q[a], %R[a]\n\tvscclrm {d5, vpr}\n\tvmov %Q[r], %R[r], d5\n\tvmov %Q[s], %R[s], d6"
                 : [r] "=&r"(q5), [s] "=&r"(q6) : [a] "r"(0x4000000040400000ull) : "d5", "d6");
  mix((uint32_t)q5); mix((uint32_t)(q5 >> 32)); mix((uint32_t)q6); mix((uint32_t)(q6 >> 32));
  __asm volatile("vmsr fpscr, %[fi]" :: [fi] "r"(LTP));
  report("clrm", digest);
}

/* MSR/MRS encodings for the PAC key registers (GNU as 2.42 has no names for them) */
#define MSR_R0(sysm) ".inst.w " #sysm "\n\t"
static void set_pac_keys(void) {
  register uint32_t r0 __asm("r0");
  for (uint32_t k = 0; k < 8; k++) {
    r0 = 0x1234567u * (k + 1) ^ 0x89ABCDEFu;
    switch (k) {   /* msr PAC_KEY_P_0..3 (SYSm 0x20..0x23), PAC_KEY_U_0..3 (0x24..0x27) */
      case 0: __asm volatile(".inst.w 0xF3808820" :: "r"(r0)); break;
      case 1: __asm volatile(".inst.w 0xF3808821" :: "r"(r0)); break;
      case 2: __asm volatile(".inst.w 0xF3808822" :: "r"(r0)); break;
      case 3: __asm volatile(".inst.w 0xF3808823" :: "r"(r0)); break;
      case 4: __asm volatile(".inst.w 0xF3808824" :: "r"(r0)); break;
      case 5: __asm volatile(".inst.w 0xF3808825" :: "r"(r0)); break;
      case 6: __asm volatile(".inst.w 0xF3808826" :: "r"(r0)); break;
      case 7: __asm volatile(".inst.w 0xF3808827" :: "r"(r0)); break;
    }
  }
  __asm volatile("isb");
}

static void run_sysregs(void) {
  BEGIN("sysregs");
  uint32_t v, w;
#define MRS(name) ({ uint32_t x_; __asm volatile("mrs %0, " name : "=r"(x_)); x_; })
#define MSR(name, val) __asm volatile("msr " name ", %0" :: "r"(val) : "memory")
  mix(MRS("control")); mix(MRS("ipsr")); mix(MRS("epsr")); mix(MRS("msplim")); mix(MRS("psplim"));
  mix(MRS("primask")); mix(MRS("basepri")); mix(MRS("faultmask"));
  /* stack limits: bits [2:0] are RES0 */
  MSR("msplim", 0x30000123); mix(MRS("msplim"));
  MSR("psplim", 0x300004FF); mix(MRS("psplim"));
  MSR("msplim_ns", 0x20000077); mix(MRS("msplim_ns"));
  MSR("psplim_ns", 0x20000488); mix(MRS("psplim_ns"));
  MSR("msp_ns", 0x20001000); mix(MRS("msp_ns")); mix(MRS("sp_ns"));
  MSR("psp_ns", 0x20002004); mix(MRS("psp_ns")); mix(MRS("sp_ns"));
  MSR("control_ns", 2); mix(MRS("control_ns")); mix(MRS("sp_ns"));
  MSR("sp_ns", 0x20003008); mix(MRS("psp_ns")); mix(MRS("msp_ns"));
  MSR("control_ns", 0); mix(MRS("control_ns")); mix(MRS("sp_ns"));
  MSR("psp", 0x30006000); mix(MRS("psp"));
  MSR("msplim_ns", 0); MSR("psplim_ns", 0); MSR("msplim", 0); MSR("psplim", 0);
  /* BASEPRI: three priority bits */
  static const uint32_t pri[] = { 0x1F, 0x20, 0x21, 0x7F, 0x80, 0xFF, 0 };
  for (unsigned i = 0; i < 7; i++) { MSR("basepri", pri[i]); mix(MRS("basepri")); }
  MSR("basepri", 0x60); MSR("basepri_max", 0x80); mix(MRS("basepri"));
  MSR("basepri_max", 0x40); mix(MRS("basepri")); MSR("basepri_max", 0); mix(MRS("basepri"));
  MSR("basepri", 0);
  MSR("basepri_ns", 0xE3); mix(MRS("basepri_ns")); MSR("basepri_ns", 0);
  MSR("primask_ns", 1); mix(MRS("primask_ns")); MSR("primask_ns", 0);
  MSR("faultmask_ns", 1); mix(MRS("faultmask_ns")); MSR("faultmask_ns", 0);
  __asm volatile("cpsid i"); mix(MRS("primask")); __asm volatile("cpsie i"); mix(MRS("primask"));
  __asm volatile("cpsid f"); mix(MRS("faultmask")); __asm volatile("cpsie f"); mix(MRS("faultmask"));
  MSR("primask", 1); mix(MRS("primask")); MSR("primask", 0);
  MSR("faultmask", 1); mix(MRS("faultmask")); MSR("faultmask", 0);
  /* xPSR views */
  __asm volatile("msr APSR_nzcvqg, %0" :: "r"(0xF80F0000u) : "cc");
  mix(MRS("apsr")); mix(MRS("iapsr")); mix(MRS("eapsr")); mix(MRS("xpsr")); mix(MRS("iepsr"));
  __asm volatile("msr APSR_nzcvq, %0" :: "r"(0x30000000u) : "cc");
  mix(MRS("apsr")); mix(MRS("xpsr"));
  __asm volatile("msr APSR_g, %0" :: "r"(0x000A0000u) : "cc");
  mix(MRS("apsr"));
  /* CONTROL: FPCA and SFPA are writable from privileged code */
  v = MRS("control");
  MSR("control", v & ~0xCu); __asm volatile("isb"); mix(MRS("control"));
  MSR("control", v | 0x8u); __asm volatile("isb"); mix(MRS("control"));
  MSR("control", v); __asm volatile("isb"); mix(MRS("control"));
  /* PAC keys read back */
  set_pac_keys();
  {
    register uint32_t r0 __asm("r0");
    __asm volatile(".inst.w 0xF3EF8020" : "=r"(r0)); mix(r0);
    __asm volatile(".inst.w 0xF3EF8023" : "=r"(r0)); mix(r0);
    __asm volatile(".inst.w 0xF3EF8024" : "=r"(r0)); mix(r0);
    __asm volatile(".inst.w 0xF3EF8027" : "=r"(r0)); mix(r0);
  }
  /* FPSCR, FPDSCR */
  __asm volatile("vmsr fpscr, %0" :: "r"(0xFFFFFFFFu));
  __asm volatile("vmrs %0, fpscr" : "=r"(w)); mix(w);
  __asm volatile("vmsr fpscr, %0" :: "r"(LTP));
  __asm volatile("vmrs %0, fpscr" : "=r"(w)); mix(w);
  mix(*(volatile uint32_t *)0xE000EF3C);   /* FPDSCR */
  mix(*(volatile uint32_t *)0xE000EF34 & ~0u);   /* FPCCR */
  mix(*(volatile uint32_t *)0xE000ED88);   /* CPACR */
  mix(*(volatile uint32_t *)0xE000ED00);   /* CPUID */
  mix(*(volatile uint32_t *)0xE000EF40);   /* MVFR0 */
  mix(*(volatile uint32_t *)0xE000EF44);   /* MVFR1 */
  mix(*(volatile uint32_t *)0xE000EF48);   /* MVFR2 */
  mix(*(volatile uint32_t *)0xE000E004);   /* ICTR */
  report("sysregs", digest);
}

/* ------------------------------------------------------------------ exceptions */

#define SCB_ICSR (*(volatile uint32_t *)0xE000ED04)
#define SCB_AIRCR (*(volatile uint32_t *)0xE000ED0C)
#define SCB_CCR (*(volatile uint32_t *)0xE000ED14)
#define SCB_SHPR1 (*(volatile uint32_t *)0xE000ED18)
#define SCB_SHPR2 (*(volatile uint32_t *)0xE000ED1C)
#define SCB_SHPR3 (*(volatile uint32_t *)0xE000ED20)
#define SCB_SHCSR (*(volatile uint32_t *)0xE000ED24)
#define SCB_CFSR (*(volatile uint32_t *)0xE000ED28)
#define SCB_HFSR (*(volatile uint32_t *)0xE000ED2C)
#define SCB_MMFAR (*(volatile uint32_t *)0xE000ED34)
#define SCB_BFAR (*(volatile uint32_t *)0xE000ED38)
#define SYST_CSR (*(volatile uint32_t *)0xE000E010)
#define SYST_RVR (*(volatile uint32_t *)0xE000E014)
#define SYST_CVR (*(volatile uint32_t *)0xE000E018)
#define NVIC_ISER0 (*(volatile uint32_t *)0xE000E100)
#define NVIC_ISER7 (*(volatile uint32_t *)0xE000E11C)
#define NVIC_ICER0 (*(volatile uint32_t *)0xE000E180)
#define NVIC_ICER7 (*(volatile uint32_t *)0xE000E19C)
#define NVIC_ISPR0 (*(volatile uint32_t *)0xE000E200)
#define NVIC_ISPR7 (*(volatile uint32_t *)0xE000E21C)
#define NVIC_IPR4 (*(volatile uint32_t *)0xE000E410)
#define NVIC_IPR56 (*(volatile uint32_t *)0xE000E4E0)
#define FPCCR (*(volatile uint32_t *)0xE000EF34)
#define MPU_TYPE (*(volatile uint32_t *)0xE000ED90)
#define MPU_CTRL (*(volatile uint32_t *)0xE000ED94)
#define MPU_RNR (*(volatile uint32_t *)0xE000ED98)
#define MPU_RBAR (*(volatile uint32_t *)0xE000ED9C)
#define MPU_RLAR (*(volatile uint32_t *)0xE000EDA0)
#define MPU_MAIR0 (*(volatile uint32_t *)0xE000EDC0)

static volatile uint32_t trace[32];
static volatile uint32_t ntrace;
static void t(uint32_t v) { if (ntrace < 32) trace[ntrace++] = v; }

void SVC_Handler(void) __attribute__((naked));
void SVC_Handler(void) {
  __asm volatile(
      "tst lr, #4\n\tite eq\n\tmrseq r0, msp\n\tmrsne r0, psp\n\t"
      "ldr r1, [r0, #24]\n\tldrb r1, [r1, #-2]\n\t"   /* SVC number from the instruction */
      "ldr r2, [r0]\n\tadd r2, r2, r1\n\tstr r2, [r0]\n\t" /* r0 += number */
      "bx lr");
}

static volatile float fp_in_handler;
void PendSV_Handler(void) {
  uint32_t lr;
  __asm volatile("mov %0, lr" : "=r"(lr) :: "memory");
  t(0x100 | (SCB_ICSR & 0x1FF));
  fp_in_handler = fp_in_handler * 3.0f + 1.0f;   /* uses the FPU: lazy stacking */
  t(lr);
  t(FPCCR & 0x3F);
}

static volatile uint32_t ticks;
void SysTick_Handler(void) {
  ticks++;
  if (ticks == 3) {
    SCB_ICSR = 1u << 28;   /* PendSV from SysTick: tail-chained after return */
    t(0x200 | ticks);
  }
}

void IRQ16_Handler(void) {
  t(0x300 | (SCB_ICSR & 0x1FF));
  NVIC_ISPR7 = 1;          /* IRQ224 has higher priority: preempts */
  t(0x301);
}

void IRQ224_Handler(void) { t(0x310 | ((SCB_ICSR >> 12) & 0x1FF)); }

/* Shared fault handler for HardFault, MemManage, BusFault, UsageFault, SecureFault: records the
   status registers, clears them and skips the faulting instruction. */
volatile uint32_t fault_cfsr, fault_hfsr, fault_sfsr, fault_mmfar, fault_bfar, fault_ipsr, fault_psp, fault_count;
void Fault_C(uint32_t *frame, uint32_t exc_return) {
  uint32_t pc = frame[6];
  uint16_t hw = *(volatile uint16_t *)pc;
  frame[6] = pc + (((hw >> 11) >= 0x1D) ? 4 : 2);
  fault_cfsr |= SCB_CFSR;
  fault_hfsr |= SCB_HFSR;
  fault_sfsr |= *(volatile uint32_t *)0xE000EDE4;
  fault_mmfar = SCB_MMFAR;
  fault_bfar = SCB_BFAR;
  fault_ipsr = (fault_ipsr << 8) | (MRS("ipsr") & 0xFF);
  __asm volatile("mrs %0, psp" : "=r"(fault_psp));
  fault_count++;
  SCB_CFSR = 0xFFFFFFFFu;
  SCB_HFSR = 0xFFFFFFFFu;
  *(volatile uint32_t *)0xE000EDE4 = 0xFFFFFFFFu;
  (void)exc_return;
}

__attribute__((naked)) void Fault_Handler(void) {
  __asm volatile("tst lr, #4\n\tite eq\n\tmrseq r0, msp\n\tmrsne r0, psp\n\tmov r1, lr\n\tpush {r4, lr}\n\tbl Fault_C\n\tpop {r4, pc}");
}
void HardFault_Handler(void) __attribute__((alias("Fault_Handler")));
void MemManage_Handler(void) __attribute__((alias("Fault_Handler")));
void BusFault_Handler(void) __attribute__((alias("Fault_Handler")));
void UsageFault_Handler(void) __attribute__((alias("Fault_Handler")));
void SecureFault_Handler(void) __attribute__((alias("Fault_Handler")));

static void mix_faults(void) {
  mix(fault_count); mix(fault_cfsr); mix(fault_hfsr); mix(fault_sfsr); mix(fault_ipsr);
  fault_count = fault_cfsr = fault_hfsr = fault_sfsr = fault_ipsr = 0;
}

static void run_exceptions(void) {
  BEGIN("exceptions");
  ntrace = 0;
  SCB_SHCSR |= (1u << 16) | (1u << 17) | (1u << 18) | (1u << 19);   /* MemManage, BusFault, UsageFault, SecureFault enabled */
  /* SVC: r0 += svc number */
  register uint32_t r0 __asm("r0") = 100;
  __asm volatile("svc #23" : "+r"(r0));
  mix(r0);
  /* PendSV with lazy FP stacking: thread FP registers survive */
  SCB_SHPR3 = (0xE0u << 16) | (0x40u << 24);   /* PendSV lowest, SysTick 0x40 */
  float keep = 1.25f;
  __asm volatile("vmov.f32 s16, #1.25" ::: "s16");
  fp_in_handler = 2.0f;
  mix(MRS("control"));
  SCB_ICSR = 1u << 28;
  __asm volatile("dsb\n isb");
  float after;
  __asm volatile("vmov %0, s16" : "=r"(after));
  mix(*(uint32_t *)&after == *(uint32_t *)&keep);
  mix(*(uint32_t *)&fp_in_handler);
  mix(FPCCR);
  /* SysTick: three interrupts, then PendSV pended from the handler */
  ticks = 0;
  SYST_RVR = 999;
  SYST_CVR = 0;
  SYST_CSR = 7;
  while (ticks < 5) {}
  SYST_CSR = 0;
  mix(ticks);
  /* SysTick without interrupt: poll COUNTFLAG */
  SYST_RVR = 499;
  SYST_CVR = 0;
  SYST_CSR = 5;
  while ((SYST_CSR & (1u << 16)) == 0) {}
  mix(1);
  mix((SYST_CSR >> 16) & 1);   /* COUNTFLAG clears on read */
  SYST_CSR = 0;
  mix(SYST_RVR);
  /* NVIC preemption: IRQ16 low priority, IRQ224 high */
  NVIC_IPR4 = 0x80u;
  NVIC_IPR56 = 0x20u;
  NVIC_ISER0 = 1u << 16;
  NVIC_ISER7 = 1;
  NVIC_ISPR0 = 1u << 16;
  __asm volatile("dsb\n isb");
  /* BASEPRI masks IRQ16 */
  __asm volatile("msr basepri, %0" ::"r"(0x80));
  NVIC_ISPR0 = 1u << 16;
  __asm volatile("dsb\n isb");
  t(0x400 | ((NVIC_ISPR0 >> 16) & 1));
  __asm volatile("msr basepri, %0" ::"r"(0));
  __asm volatile("dsb\n isb");
  /* PRIMASK */
  __asm volatile("cpsid i");
  NVIC_ISPR7 = 1;
  t(0x500 | (NVIC_ISPR7 & 1) | ((SCB_ICSR >> 12) & 0x1FF) << 4);
  __asm volatile("cpsie i");
  __asm volatile("dsb\n isb");
  NVIC_ICER0 = 1u << 16;
  NVIC_ICER7 = 1;
  /* usage faults, handled and skipped */
  SCB_CCR |= (1u << 4) | (1u << 3);   /* DIV_0_TRP, UNALIGN_TRP */
  volatile uint32_t z = 0, q = 7, v;
  v = q / z;
  mix_faults();
  v = *(volatile uint32_t *)((uint8_t *)buf + 1);
  mix_faults();
  __asm volatile("udf #0");
  mix_faults();
  SCB_CCR &= ~((1u << 4) | (1u << 3));
  (void)v;
  for (uint32_t i = 0; i < ntrace; i++) mix(trace[i]);
  mix(ntrace);
  report("exceptions", digest);
}

/* Stack limit: PSPLIM set 40 bytes below PSP, a 48-byte push faults with STKOF; the exception
   frame (32 bytes) still fits above the limit. */
static uint32_t psp_stack[64] __attribute__((aligned(8)));

static void run_stklim(void) {
  BEGIN("stklim");
  for (int i = 0; i < 64; i++) psp_stack[i] = 0xAA000000u | i;
  uint32_t top = (uint32_t)&psp_stack[64], sp_after, ctl;
  __asm volatile(
      "mrs %[c], control\n\tbic %[c], %[c], #4\n\tmsr control, %[c]\n\tisb\n\t"   /* no FP context */
      "msr psp, %[top]\n\tsub %[c], %[top], #40\n\tmsr psplim, %[c]\n\t"
      "mrs %[c], control\n\torr %[c], %[c], #2\n\tmsr control, %[c]\n\tisb\n\t"   /* thread on PSP */
      "stmdb sp!, {r0-r11}\n\t"                                                     /* STKOF */
      "mrs %[s], psp\n\t"
      "mrs %[c], control\n\tbic %[c], %[c], #2\n\tmsr control, %[c]\n\tisb\n\t"   /* back to MSP */
      "mov %[c], #0\n\tmsr psplim, %[c]"
      : [c] "=&r"(ctl), [s] "=&r"(sp_after) : [top] "r"(top) : "memory");
  mix(top - sp_after);
  mix(top - fault_psp);
  mix_faults();
  mix(fault_mmfar == top); mix(fault_bfar == top);
  for (int i = 0; i < 64; i++) mix(psp_stack[i] == (0xAA000000u | i));
  /* limit on MSP in handler mode is not exercised; a limit equal to the SP does not fault */
  __asm volatile("mrs %0, msp\n\tsub %0, #512\n\tbic %0, #7\n\tmsr msplim, %0\n\tpush {r4, r5}\n\tpop {r4, r5}\n\tmov %0, #0\n\tmsr msplim, %0" : "=&r"(ctl) :: "memory");
  mix_faults();
  report("stklim", digest);
}

/* MemManage through a PMSAv8 region: 64 bytes of DTCM read-only */
static uint32_t mpu_region[16] __attribute__((aligned(32)));

static void run_mpu(void) {
  BEGIN("mpu");
  for (int i = 0; i < 16; i++) mpu_region[i] = 0x5A000000u | i;
  uint32_t base = (uint32_t)mpu_region;
  mix(MPU_TYPE);
  MPU_MAIR0 = 0x44;                       /* attr 0: normal, non-cacheable */
  MPU_RNR = 0;
  MPU_RBAR = (base & ~31u) | (2u << 1) | 1u;   /* AP = RO privileged, XN */
  MPU_RLAR = ((base + 63) & ~31u) | 1u;        /* AttrIdx 0, enabled */
  MPU_RNR = 1;
  MPU_RBAR = 0x40000000u | (1u << 1) | 1u;     /* a second region: peripherals RW */
  MPU_RLAR = 0x4FFFFFE0u | 1u;
  MPU_CTRL = 5;                                /* ENABLE | PRIVDEFENA */
  __asm volatile("dsb\n isb");
  mix(MPU_RBAR); mix(MPU_RLAR); mix(MPU_CTRL);
  /* TT shows the region number */
  uint32_t fo;
  mix(op_tt(base + 8, 0, 0, &fo)); mix(op_ttt(base + 8, 0, 0, &fo));
  mix(op_tt(0x48203000, 0, 0, &fo)); mix(op_tt(0x30000000, 0, 0, &fo));
  mix(op_tt(base + 64, 0, 0, &fo)); mix(op_tt(base + 63, 0, 0, &fo));
  /* read allowed, write faults */
  mix(mpu_region[3]);
  uint32_t v = 0xEE;
  __asm volatile("str %[v], [%[p], #8]" :: [v] "r"(0x12345678), [p] "r"(base) : "memory");
  mix_faults();
  mix(fault_mmfar - base);
  mix(mpu_region[2]);
  __asm volatile("mov %[v], #0xEE\n\tstrh %[v], [%[p], #62]\n\tldr %[v], [%[p], #60]" : [v] "+r"(v) : [p] "r"(base) : "memory");
  mix(v);
  mix_faults();
  mix(fault_mmfar - base);
  /* the word after the region is writable */
  mix(op_tt(base + 64, 0, 0, &fo));
  MPU_CTRL = 0;
  __asm volatile("dsb\n isb");
  MPU_RNR = 0; MPU_RLAR = 0;
  MPU_RNR = 1; MPU_RLAR = 0;
  mix_faults();
  report("mpu", digest);
}

/* BusFault from an unmapped address (the destination register keeps its value) */
static void run_busfault(void) {
  BEGIN("busfault");
  uint32_t v = 0xEE;
  __asm volatile("ldr %[v], [%[p]]" : [v] "+r"(v) : [p] "r"(0x4FFF0000) : "memory");
  mix(v); mix_faults(); mix(fault_bfar);
  v = 0xEF;
  __asm volatile("ldrh %[v], [%[p], #2]" : [v] "+r"(v) : [p] "r"(0x4FFF1000) : "memory");
  mix(v); mix_faults(); mix(fault_bfar);
  v = 0xF0;
  __asm volatile("ldrb %[v], [%[p]]" : [v] "+r"(v) : [p] "r"(0x5FFF0003) : "memory");
  mix(v); mix_faults(); mix(fault_bfar);
  report("busfault", digest);
}

/* VLSTM/VLLDM: lazy floating-point state preservation for Secure code */
static uint32_t fp_save[40] __attribute__((aligned(8)));

static void run_fpsec(void) {
  BEGIN("fpsec");
  for (int i = 0; i < 40; i++) fp_save[i] = 0xCC000000u | i;
  uint32_t v0, v1, v2, st, c0, c1, c2, c3;
  __asm volatile(
      "vmsr fpscr, %[fi]\n\tvmov.f32 s0, #1.0\n\tvmov.f32 s1, #2.0\n\tvmov.f32 s15, #-4.0\n\tvmov.f32 s16, #8.0\n\t"
      "mrs %[c0], control\n\t"
      "vlstm %[p]\n\t"
      "ldr %[c1], [%[fpccr]]\n\t"            /* LSPACT */
      "vadd.f32 s2, s0, s1\n\t"              /* triggers the lazy save */
      "ldr %[c2], [%[fpccr]]\n\t"
      "vmov %[v0], s0\n\tvmov %[v1], s2\n\tvmov %[v2], s16\n\tvmrs %[st], fpscr\n\t"
      "vlldm %[p]\n\t"
      "ldr %[c3], [%[fpccr]]\n\t"
      : [v0] "=&r"(v0), [v1] "=&r"(v1), [v2] "=&r"(v2), [st] "=&r"(st), [c0] "=&r"(c0), [c1] "=&r"(c1), [c2] "=&r"(c2), [c3] "=&r"(c3)
      : [p] "r"(fp_save), [fpccr] "r"(0xE000EF34), [fi] "r"(LTP | 0x00400010)
      : "s0", "s1", "s2", "s15", "s16", "memory");
  mix(v0); mix(v1); mix(v2); mix(st); mix(c0); mix(c1); mix(c2); mix(c3);
  for (int i = 0; i < 40; i++) mix(fp_save[i]);
  __asm volatile("vmov %0, s0\n\tvmov %1, s2\n\tvmov %2, s15\n\tvmrs %3, fpscr" : "=r"(v0), "=r"(v1), "=r"(v2), "=r"(st));
  mix(v0); mix(v1); mix(v2); mix(st);
  mix(FPCCR);
  __asm volatile("vmsr fpscr, %0" :: "r"(LTP));
  report("fpsec", digest);
}

/* ------------------------------------------------------------------ main */

static void run_tt(void) {
  static const uint32_t addrs[] = {
    0x11000000, 0x1100FFFF, 0x01000000, 0x30000000, 0x30007FFC, 0x20000000, 0x38000000,
    0xE000ED00, 0xE002ED00, 0x48203000, 0x58203000, 0x4FFF0000, 0x00000000, 0xFFFFFFFC,
    0x10000000, 0x12000000, 0x21000000, 0x80000000, 0x1101FFFF, 0x11020000,
  };
  BEGIN("tt");
  for (unsigned i = 0; i < sizeof(addrs) / sizeof(addrs[0]); i++) {
    uint32_t fo;
    mix(op_tt(addrs[i], 0, 0, &fo)); mix(op_ttt(addrs[i], 0, 0, &fo));
    mix(op_tta(addrs[i], 0, 0, &fo)); mix(op_ttat(addrs[i], 0, 0, &fo));
  }
  report("tt", digest);
}

int main(void) {
  puts_("isa-test begin\n");
  run_op2f("adds", op_adds, 0);
  run_op2f("adcs", op_adcs, 0);
  run_op2f("subs", op_subs, 0);
  run_op2f("sbcs", op_sbcs, 0);
  run_op2f("rsbs", op_rsbs, 0);
  run_op2f("cmp", op_cmp, 0);
  run_op2f("cmn", op_cmn, 0);
  run_op2f("ands_lsl", op_ands_lsl, 0);
  run_op2f("orrs_lsr", op_orrs_lsr, 0);
  run_op2f("eors_asr", op_eors_asr, 0);
  run_op2f("bics_ror", op_bics_ror, 0);
  run_op2f("orns_rrx", op_orns_rrx, 0);
  run_op2f("mvns_lsl", op_mvns_lsl, 0);
  run_op2f("tst", op_tst, 0);
  run_op2f("teq", op_teq, 0);
  run_op2f("adds_imm", op_adds_imm, 0);
  run_op2f("subs_imm", op_subs_imm, 0);
  run_op2f("ands_imm", op_ands_imm, 0);
  run_op2f("lsls", op_lsls, 0);
  run_op2f("lsrs", op_lsrs, 0);
  run_op2f("asrs", op_asrs, 0);
  run_op2f("rors", op_rors, 0);
  run_op2f("lsls_imm", op_lsls_imm, 0);
  run_op2f("lsrs_imm", op_lsrs_imm, 0);
  run_op2f("asrs_imm", op_asrs_imm, 0);
  run_op2f("rrxs", op_rrxs, 0);
  run_op2f("muls", op_muls, 0);
  run_op2f("mla", op_mla, 0);
  run_op2f("mls", op_mls, 0);
  run_op2f("sdiv", op_sdiv, 0);
  run_op2f("udiv", op_udiv, 0);
  run_op2f("qadd", op_qadd, 0);
  run_op2f("qsub", op_qsub, 0);
  run_op2f("qdadd", op_qdadd, 0);
  run_op2f("qdsub", op_qdsub, 0);
  run_op2f("ssat", op_ssat, 0);
  run_op2f("ssat32", op_ssat2, 0);
  run_op2f("usat", op_usat, 0);
  run_op2f("usat0", op_usat0, 0);
  run_op2f("ssat16", op_ssat16, 0);
  run_op2f("usat16", op_usat16, 0);
  run_op2f("sadd16", op_sadd16, 0);
  run_op2f("ssub16", op_ssub16, 0);
  run_op2f("sasx", op_sasx, 0);
  run_op2f("ssax", op_ssax, 0);
  run_op2f("sadd8", op_sadd8, 0);
  run_op2f("ssub8", op_ssub8, 0);
  run_op2f("uadd16", op_uadd16, 0);
  run_op2f("usub16", op_usub16, 0);
  run_op2f("uasx", op_uasx, 0);
  run_op2f("usax", op_usax, 0);
  run_op2f("uadd8", op_uadd8, 0);
  run_op2f("usub8", op_usub8, 0);
  run_op2f("qadd16", op_qadd16, 0);
  run_op2f("qsub16", op_qsub16, 0);
  run_op2f("qasx", op_qasx, 0);
  run_op2f("qsax", op_qsax, 0);
  run_op2f("qadd8", op_qadd8, 0);
  run_op2f("qsub8", op_qsub8, 0);
  run_op2f("uqadd16", op_uqadd16, 0);
  run_op2f("uqsub16", op_uqsub16, 0);
  run_op2f("uqasx", op_uqasx, 0);
  run_op2f("uqsax", op_uqsax, 0);
  run_op2f("uqadd8", op_uqadd8, 0);
  run_op2f("uqsub8", op_uqsub8, 0);
  run_op2f("shadd16", op_shadd16, 0);
  run_op2f("shsub16", op_shsub16, 0);
  run_op2f("shasx", op_shasx, 0);
  run_op2f("shsax", op_shsax, 0);
  run_op2f("shadd8", op_shadd8, 0);
  run_op2f("shsub8", op_shsub8, 0);
  run_op2f("uhadd16", op_uhadd16, 0);
  run_op2f("uhsub16", op_uhsub16, 0);
  run_op2f("uhasx", op_uhasx, 0);
  run_op2f("uhsax", op_uhsax, 0);
  run_op2f("uhadd8", op_uhadd8, 0);
  run_op2f("uhsub8", op_uhsub8, 0);
  run_op2f("sel", op_sel, 0);
  run_op2f("smulbb", op_smulbb, 0);
  run_op2f("smultb", op_smultb, 0);
  run_op2f("smulbt", op_smulbt, 0);
  run_op2f("smultt", op_smultt, 0);
  run_op2f("smlabb", op_smlabb, 0);
  run_op2f("smlatt", op_smlatt, 0);
  run_op2f("smulwb", op_smulwb, 0);
  run_op2f("smulwt", op_smulwt, 0);
  run_op2f("smlawb", op_smlawb, 0);
  run_op2f("smlawt", op_smlawt, 0);
  run_op2f("smuad", op_smuad, 0);
  run_op2f("smuadx", op_smuadx, 0);
  run_op2f("smusd", op_smusd, 0);
  run_op2f("smusdx", op_smusdx, 0);
  run_op2f("smlad", op_smlad, 0);
  run_op2f("smladx", op_smladx, 0);
  run_op2f("smlsd", op_smlsd, 0);
  run_op2f("smlsdx", op_smlsdx, 0);
  run_op2f("smmul", op_smmul, 0);
  run_op2f("smmulr", op_smmulr, 0);
  run_op2f("smmla", op_smmla, 0);
  run_op2f("smmlar", op_smmlar, 0);
  run_op2f("smmls", op_smmls, 0);
  run_op2f("smmlsr", op_smmlsr, 0);
  run_op2f("usad8", op_usad8, 0);
  run_op2f("usada8", op_usada8, 0);
  run_op2f("clz", op_clz, 0);
  run_op2f("rbit", op_rbit, 0);
  run_op2f("rev", op_rev, 0);
  run_op2f("rev16", op_rev16, 0);
  run_op2f("revsh", op_revsh, 0);
  run_op2f("ubfx", op_ubfx, 0);
  run_op2f("sbfx", op_sbfx, 0);
  run_op2f("bfi", op_bfi, 0);
  run_op2f("bfc", op_bfc, 0);
  run_op2f("uxtab", op_uxtab, 0);
  run_op2f("sxtab", op_sxtab, 0);
  run_op2f("uxtah", op_uxtah, 0);
  run_op2f("sxtah", op_sxtah, 0);
  run_op2f("uxtab16", op_uxtab16, 0);
  run_op2f("sxtab16", op_sxtab16, 0);
  run_op2f("uxtb16", op_uxtb16, 0);
  run_op2f("sxtb16", op_sxtb16, 0);
  run_op2f("pkhbt", op_pkhbt, 0);
  run_op2f("pkhtb", op_pkhtb, 0);
  run_op2f("movw_movt", op_movw_movt, 0);
  run_op2f("it", op_it, 0);
  run_op2f("it16", op_it16, 0);
  run_op2f("cs_eq", op_cs_eq, 0);
  run_op2f("cs_ne", op_cs_ne, 0);
  run_op2f("cs_cs", op_cs_cs, 0);
  run_op2f("cs_cc", op_cs_cc, 0);
  run_op2f("cs_mi", op_cs_mi, 0);
  run_op2f("cs_pl", op_cs_pl, 0);
  run_op2f("cs_vs", op_cs_vs, 0);
  run_op2f("cs_vc", op_cs_vc, 0);
  run_op2f("cs_hi", op_cs_hi, 0);
  run_op2f("cs_ls", op_cs_ls, 0);
  run_op2f("cs_ge", op_cs_ge, 0);
  run_op2f("cs_lt", op_cs_lt, 0);
  run_op2f("cs_gt", op_cs_gt, 0);
  run_op2f("cs_le", op_cs_le, 0);
  run_op2f("cs_al", op_cs_al, 0);
  run_op4("umull", op_umull);
  run_op4("smull", op_smull);
  run_op4("umlal", op_umlal);
  run_op4("smlal", op_smlal);
  run_op4("umaal", op_umaal);
  run_op4("smlalbb", op_smlalbb);
  run_op4("smlaltb", op_smlaltb);
  run_op4("smlald", op_smlald);
  run_op4("smlaldx", op_smlaldx);
  run_op4("smlsld", op_smlsld);
  run_op4("smlsldx", op_smlsldx);
  run_fop("vadd", f_add);
  run_fop("vsub", f_sub);
  run_fop("vmul", f_mul);
  run_fop("vnmul", f_nmul);
  run_fop("vdiv", f_div);
  run_fop("vsqrt", f_sqrt);
  run_fop("vabs", f_abs);
  run_fop("vneg", f_neg);
  run_fop("vmla", f_mla);
  run_fop("vmls", f_mls);
  run_fop("vnmla", f_nmla);
  run_fop("vnmls", f_nmls);
  run_fop("vfma", f_fma);
  run_fop("vfms", f_fms);
  run_fop("vfnma", f_fnma);
  run_fop("vfnms", f_fnms);
  run_fop("vcmp", f_cmp);
  run_fop("vcmpe", f_cmpe);
  run_fop("vcmpz", f_cmpz);
  run_fop("vcvt_s32", f_cvt_s32);
  run_fop("vcvt_u32", f_cvt_u32);
  run_fop("vcvtr_s32", f_cvtr_s32);
  run_fop("vcvtr_u32", f_cvtr_u32);
  run_fop("vcvt_f_s32", f_cvt_f_s32);
  run_fop("vcvt_f_u32", f_cvt_f_u32);
  run_fop("vcvt_fix_s16", f_cvt_fix_s16);
  run_fop("vcvt_fix_u32", f_cvt_fix_u32);
  run_fop("vcvt_fix_to_f", f_cvt_fix_to_f);
  run_fop("vcvt_fix_u16_to_f", f_cvt_fix_u16_to_f);
  run_fop("vcvtb_f16", f_cvtb_h);
  run_fop("vcvtt_f16", f_cvtt_h);
  run_fop("vcvtb_f32", f_cvtb_s);
  run_fop("vcvtt_f32", f_cvtt_s);
  run_fop("vmov_imm", f_movimm);
  run_fop("vselgt", f_selgt);
  run_fop("vselge", f_selge);
  run_fop("vseleq", f_seleq);
  run_fop("vselvs", f_selvs);
  run_fop("vmaxnm", f_maxnm);
  run_fop("vminnm", f_minnm);
  run_fop("vrinta", f_rinta);
  run_fop("vrintn", f_rintn);
  run_fop("vrintp", f_rintp);
  run_fop("vrintm", f_rintm);
  run_fop("vrintz", f_rintz);
  run_fop("vrintx", f_rintx);
  run_fop("vrintr", f_rintr);
  run_fop("vcvta_s32", f_cvta_s32);
  run_fop("vcvtn_s32", f_cvtn_s32);
  run_fop("vcvtp_s32", f_cvtp_s32);
  run_fop("vcvtm_s32", f_cvtm_s32);
  run_fop("vcvta_u32", f_cvta_u32);
  run_fop("vcvtn_u32", f_cvtn_u32);
  run_fop("vcvtp_u32", f_cvtp_u32);
  run_fop("vcvtm_u32", f_cvtm_u32);
  run_dop("d_vadd", d_add, K_DOUBLE);
  run_dop("d_vsub", d_sub, K_DOUBLE);
  run_dop("d_vmul", d_mul, K_DOUBLE);
  run_dop("d_vnmul", d_nmul, K_DOUBLE);
  run_dop("d_vdiv", d_div, K_DOUBLE);
  run_dop("d_vsqrt", d_sqrt, K_DOUBLE);
  run_dop("d_vabs", d_abs, K_DOUBLE);
  run_dop("d_vneg", d_neg, K_DOUBLE);
  run_dop("d_vmla", d_mla, K_DOUBLE);
  run_dop("d_vmls", d_mls, K_DOUBLE);
  run_dop("d_vnmla", d_nmla, K_DOUBLE);
  run_dop("d_vnmls", d_nmls, K_DOUBLE);
  run_dop("d_vfma", d_fma, K_DOUBLE);
  run_dop("d_vfms", d_fms, K_DOUBLE);
  run_dop("d_vfnma", d_fnma, K_DOUBLE);
  run_dop("d_vfnms", d_fnms, K_DOUBLE);
  run_dop("d_vcmp", d_cmp, K_DOUBLE);
  run_dop("d_vcmpe", d_cmpe, K_DOUBLE);
  run_dop("d_vcmpz", d_cmpz, K_DOUBLE);
  run_dop("d_vselgt", d_selgt, K_DOUBLE);
  run_dop("d_vselge", d_selge, K_DOUBLE);
  run_dop("d_vseleq", d_seleq, K_DOUBLE);
  run_dop("d_vselvs", d_selvs, K_DOUBLE);
  run_dop("d_vmaxnm", d_maxnm, K_DOUBLE);
  run_dop("d_vminnm", d_minnm, K_DOUBLE);
  run_dop("d_vrinta", d_rinta, K_DOUBLE);
  run_dop("d_vrintn", d_rintn, K_DOUBLE);
  run_dop("d_vrintp", d_rintp, K_DOUBLE);
  run_dop("d_vrintm", d_rintm, K_DOUBLE);
  run_dop("d_vrintz", d_rintz, K_DOUBLE);
  run_dop("d_vrintx", d_rintx, K_DOUBLE);
  run_dop("d_vrintr", d_rintr, K_DOUBLE);
  run_dop("d_vcvta_s32", d_cvta_s32, K_DOUBLE);
  run_dop("d_vcvtn_s32", d_cvtn_s32, K_DOUBLE);
  run_dop("d_vcvtp_s32", d_cvtp_s32, K_DOUBLE);
  run_dop("d_vcvtm_s32", d_cvtm_s32, K_DOUBLE);
  run_dop("d_vcvta_u32", d_cvta_u32, K_DOUBLE);
  run_dop("d_vcvtn_u32", d_cvtn_u32, K_DOUBLE);
  run_dop("d_vcvtp_u32", d_cvtp_u32, K_DOUBLE);
  run_dop("d_vcvtm_u32", d_cvtm_u32, K_DOUBLE);
  run_dop("d_vcvt_s32", d_cvt_s32, K_DOUBLE);
  run_dop("d_vcvt_u32", d_cvt_u32, K_DOUBLE);
  run_dop("d_vcvtr_s32", d_cvtr_s32, K_DOUBLE);
  run_dop("d_vcvtr_u32", d_cvtr_u32, K_DOUBLE);
  run_dop("d_vcvt_f_s32", d_cvt_f_s32, K_INT);
  run_dop("d_vcvt_f_u32", d_cvt_f_u32, K_INT);
  run_dop("d_vcvt_fix_s16", d_cvt_fix_s16, K_DOUBLE);
  run_dop("d_vcvt_fix_u32", d_cvt_fix_u32, K_DOUBLE);
  run_dop("d_vcvt_fix_to_f", d_cvt_fix_to_f, K_INT);
  run_dop("d_vcvt_fix_u16_to_f", d_cvt_fix_u16_to_f, K_INT);
  run_dop("d_vcvt_f32_f64", d_cvt_f32_f64, K_DOUBLE);
  run_dop("d_vcvt_f64_f32", d_cvt_f64_f32, K_SINGLE);
  run_dop("d_vcvtb_f16_f64", d_cvtb_f16_f64, K_DOUBLE);
  run_dop("d_vcvtt_f16_f64", d_cvtt_f16_f64, K_DOUBLE);
  run_dop("d_vcvtb_f64_f16", d_cvtb_f64_f16, K_HALF);
  run_dop("d_vcvtt_f64_f16", d_cvtt_f64_f16, K_HALF);
  run_dop("d_vmov_imm", d_movimm, K_DOUBLE);
  run_dop("d_vmov_reg", d_movreg, K_DOUBLE);
  run_dop("h_vadd", h_add, K_HALF);
  run_dop("h_vsub", h_sub, K_HALF);
  run_dop("h_vmul", h_mul, K_HALF);
  run_dop("h_vnmul", h_nmul, K_HALF);
  run_dop("h_vdiv", h_div, K_HALF);
  run_dop("h_vsqrt", h_sqrt, K_HALF);
  run_dop("h_vabs", h_abs, K_HALF);
  run_dop("h_vneg", h_neg, K_HALF);
  run_dop("h_vmla", h_mla, K_HALF);
  run_dop("h_vmls", h_mls, K_HALF);
  run_dop("h_vnmla", h_nmla, K_HALF);
  run_dop("h_vnmls", h_nmls, K_HALF);
  run_dop("h_vfma", h_fma, K_HALF);
  run_dop("h_vfms", h_fms, K_HALF);
  run_dop("h_vfnma", h_fnma, K_HALF);
  run_dop("h_vfnms", h_fnms, K_HALF);
  run_dop("h_vcmp", h_cmp, K_HALF);
  run_dop("h_vcmpe", h_cmpe, K_HALF);
  run_dop("h_vcmpz", h_cmpz, K_HALF);
  run_dop("h_vselgt", h_selgt, K_HALF);
  run_dop("h_vselvs", h_selvs, K_HALF);
  run_dop("h_vmaxnm", h_maxnm, K_HALF);
  run_dop("h_vminnm", h_minnm, K_HALF);
  run_dop("h_vrinta", h_rinta, K_HALF);
  run_dop("h_vrintn", h_rintn, K_HALF);
  run_dop("h_vrintp", h_rintp, K_HALF);
  run_dop("h_vrintm", h_rintm, K_HALF);
  run_dop("h_vrintz", h_rintz, K_HALF);
  run_dop("h_vrintx", h_rintx, K_HALF);
  run_dop("h_vrintr", h_rintr, K_HALF);
  run_dop("h_vcvta_s32", h_cvta_s32, K_HALF);
  run_dop("h_vcvtn_s32", h_cvtn_s32, K_HALF);
  run_dop("h_vcvtp_u32", h_cvtp_u32, K_HALF);
  run_dop("h_vcvtm_u32", h_cvtm_u32, K_HALF);
  run_dop("h_vcvt_s32", h_cvt_s32, K_HALF);
  run_dop("h_vcvt_u32", h_cvt_u32, K_HALF);
  run_dop("h_vcvtr_s32", h_cvtr_s32, K_HALF);
  run_dop("h_vcvtr_u32", h_cvtr_u32, K_HALF);
  run_dop("h_vcvt_f_s32", h_cvt_f_s32, K_INT);
  run_dop("h_vcvt_f_u32", h_cvt_f_u32, K_INT);
  run_dop("h_vmov_imm", h_movimm, K_HALF);
  run_dop("h_vins_vmovx", h_ins_movx, K_HALF);
  run_dop("h_vmov_reg", h_movreg, K_HALF);
  run_half_memory();
  run_memory();
  run_memory_v8();
  run_branches();
  run_lob();
  run_clrm();
  run_sysregs();
  set_pac_keys();
  run_op2f("pacg", op_pacg, 0);
  run_op2f("pacg2", op_pacg2, 0);
  run_op2f("pac_nop", op_pac_nop, 0);
  run_tt();
  run_exceptions();
  run_stklim();
  run_mpu();
  run_busfault();
  run_fpsec();
  report("mve", 1);
  run_mve();
  puts_("isa-test end\n");
  return 0;
}
