/*
 * MVE (Helium) section of the ISA test, built with -mcpu=cortex-m85. Runs after the marker line
 * `mve: 0x00000001`; the simulator may stop at its first instruction while MVE is unimplemented.
 */
#include "isa_test.h"

#define LTP 0x00040000u

/* q0 = a, q1 = b, q2 = o (in/out); r4/r5 are scalar in/out operands (even/odd pair). */
#define MOP(fn, insn)                                                                           \
  static void fn(const uint32_t *a, const uint32_t *b, uint32_t *o, uint32_t *s, uint32_t fpscr_in, \
                 uint32_t *st_out, uint32_t *vpr_out) {                                         \
    uint32_t st, vp;                                                                            \
    register uint32_t r4 __asm("r4") = s[0];                                                    \
    register uint32_t r5 __asm("r5") = s[1];                                                    \
    __asm volatile("vmsr fpscr, %[fi]\n\tvmsr vpr, %[z]\n\t"                                   \
                   "vldrw.u32 q0, [%[a]]\n\tvldrw.u32 q1, [%[b]]\n\tvldrw.u32 q2, [%[o]]\n\t" insn \
                   "\n\tvstrw.u32 q2, [%[o]]\n\tvmrs %[st], fpscr\n\tvmrs %[vp], vpr"           \
                   : [st] "=&r"(st), [vp] "=&r"(vp), "+r"(r4), "+r"(r5)                         \
                   : [a] "r"(a), [b] "r"(b), [o] "r"(o), [fi] "r"(fpscr_in), [z] "r"(0)         \
                   : "q0", "q1", "q2", "memory", "cc");                                         \
    s[0] = r4;                                                                                  \
    s[1] = r5;                                                                                  \
    *st_out = st;                                                                               \
    *vpr_out = vp;                                                                              \
  }

MOP(m_add_i8, "vadd.i8 q2, q0, q1")
MOP(m_add_i16, "vadd.i16 q2, q0, q1")
MOP(m_add_i32, "vadd.i32 q2, q0, q1")
MOP(m_sub_i8, "vsub.i8 q2, q0, q1")
MOP(m_sub_i16, "vsub.i16 q2, q0, q1")
MOP(m_sub_i32, "vsub.i32 q2, q0, q1")
MOP(m_mul_i8, "vmul.i8 q2, q0, q1")
MOP(m_mul_i16, "vmul.i16 q2, q0, q1")
MOP(m_mul_i32, "vmul.i32 q2, q0, q1")
MOP(m_mulh, "vmulh.s8 q2, q0, q1\n\tvrmulh.u16 q1, q2, q0\n\tveor q2, q2, q1")
MOP(m_add_scalar, "vadd.i32 q2, q0, r4\n\tvsub.i16 q1, q1, r5\n\tvmul.i8 q2, q2, r4\n\tveor q2, q2, q1")
MOP(m_add_f32, "vadd.f32 q2, q0, q1")
MOP(m_sub_f32, "vsub.f32 q2, q0, q1")
MOP(m_mul_f32, "vmul.f32 q2, q0, q1")
MOP(m_fma_f32, "vfma.f32 q2, q0, q1")
MOP(m_fms_f32, "vfms.f32 q2, q0, q1")
MOP(m_fmas_f32, "vfmas.f32 q2, q0, r4")
MOP(m_maxnm_f32, "vmaxnm.f32 q2, q0, q1")
MOP(m_minnm_f32, "vminnm.f32 q2, q0, q1")
MOP(m_abs_neg_f32, "vabs.f32 q2, q0\n\tvneg.f32 q1, q1\n\tvadd.f32 q2, q2, q1")
MOP(m_rint_f32, "vrintn.f32 q2, q0\n\tvrinta.f32 q1, q1\n\tvadd.f32 q2, q2, q1\n\tvrintz.f32 q1, q0\n\tvsub.f32 q2, q2, q1\n\tvrintm.f32 q1, q0\n\tvadd.f32 q2, q2, q1\n\tvrintp.f32 q1, q0\n\tvsub.f32 q2, q2, q1\n\tvrintx.f32 q1, q0\n\tvadd.f32 q2, q2, q1")
MOP(m_add_f16, "vadd.f16 q2, q0, q1")
MOP(m_mul_f16, "vmul.f16 q2, q0, q1")
MOP(m_fma_f16, "vfma.f16 q2, q0, q1")
MOP(m_and, "vand q2, q0, q1")
MOP(m_orr, "vorr q2, q0, q1")
MOP(m_eor, "veor q2, q0, q1")
MOP(m_bic_orn, "vbic q2, q0, q1\n\tvorn q1, q1, q0\n\tveor q2, q2, q1\n\tvmvn q2, q2")
MOP(m_mov_imm, "vmov.i32 q2, #0x1200\n\tveor q2, q2, q0\n\tvmov.i16 q1, #0xff\n\tveor q2, q2, q1\n\tvmov.i8 q1, #0x5a\n\tveor q2, q2, q1\n\tvmov.f32 q1, #1.5\n\tveor q2, q2, q1\n\tvmov.i32 q1, #0xff00ff00\n\tveor q2, q2, q1\n\tvmov.i64 q1, #0xff00ff00ff00ff00\n\tveor q2, q2, q1")
MOP(m_orr_bic_imm, "vorr.i32 q2, #0xff00\n\tvbic.i16 q2, #0xf0\n\tvorr.i16 q2, #0x1000\n\tvbic.i32 q2, #0x0f000000")
MOP(m_dup, "vdup.8 q2, r4\n\tveor q2, q2, q0\n\tvdup.16 q1, r5\n\tveor q2, q2, q1\n\tvdup.32 q1, r4\n\tveor q2, q2, q1")
MOP(m_mladava_s8, "vmladava.s8 r4, q0, q1")
MOP(m_mladava_s16, "vmladava.s16 r4, q0, q1")
MOP(m_mladava_s32, "vmladava.s32 r4, q0, q1")
MOP(m_mladava_u8, "vmladava.u8 r4, q0, q1\n\tvmladavax.s16 r4, q0, q1")
MOP(m_mlsdava, "vmlsdava.s32 r4, q0, q1\n\tmov r5, r4\n\tvmladav.s8 r4, q0, q1")
MOP(m_mlaldava_s32, "vmlaldava.s32 r4, r5, q0, q1")
MOP(m_mlaldava_s16, "vmlaldava.s16 r4, r5, q0, q1")
MOP(m_mlaldava_u32, "vmlaldava.u32 r4, r5, q0, q1")
MOP(m_mlsldava_s32, "vmlsldava.s32 r4, r5, q0, q1")
MOP(m_rmlaldavh_s32, "vrmlaldavha.s32 r4, r5, q0, q1")
MOP(m_rmlsldavh_s32, "vrmlsldavh.s32 r4, r5, q0, q1")
MOP(m_rmlalvh, "vrmlalvha.s32 r4, r5, q0, q1\n\tvrmlalvh.u32 r4, r5, q1, q0")
MOP(m_maxv_minv, "vmaxv.s8 r4, q0\n\tvminv.u16 r5, q1\n\tvdup.32 q2, r4\n\tvmaxav.s32 r4, q1\n\tvminav.s16 r5, q0")
MOP(m_addv, "vaddv.s32 r4, q0\n\tvaddva.u8 r4, q1\n\tvaddlv.s32 r4, r5, q1")
MOP(m_addlva, "vaddlva.u32 r4, r5, q0")
MOP(m_shl, "vshl.s32 q2, q0, q1\n\tvshl.u8 q1, q1, q0\n\tveor q2, q2, q1")
MOP(m_shl_imm, "vshl.u8 q2, q0, #3\n\tvshl.i32 q1, q1, #17\n\tveor q2, q2, q1\n\tvshl.i16 q1, q0, #0\n\tveor q2, q2, q1")
MOP(m_shr_imm, "vshr.s16 q2, q0, #4\n\tvshr.u32 q1, q1, #31\n\tveor q2, q2, q1\n\tvshr.s8 q1, q0, #8\n\tveor q2, q2, q1\n\tvrshr.s8 q1, q0, #2\n\tveor q2, q2, q1")
MOP(m_qshl, "vqshl.s32 q2, q0, #5\n\tvqshl.u16 q1, q1, #9\n\tveor q2, q2, q1\n\tvqshl.s8 q1, q0, q1\n\tveor q2, q2, q1\n\tvqrshl.s16 q1, q0, q1\n\tveor q2, q2, q1")
MOP(m_shl_scalar, "vmov q2, q0\n\tvshl.s16 q2, r4\n\tvqshl.u32 q1, r5\n\tveor q2, q2, q1\n\tvmov q1, q0\n\tvrshl.s8 q1, r4\n\tveor q2, q2, q1")
MOP(m_shll, "vshllb.s8 q2, q0, #3\n\tvshllt.u16 q1, q1, #8\n\tveor q2, q2, q1")
MOP(m_sli_sri, "vsli.32 q2, q0, #5\n\tvsri.16 q2, q1, #3\n\tvsli.8 q2, q1, #7")
MOP(m_shlc, "vshlc q2, r4, #7\n\tvshlc q2, r5, #32")
MOP(m_cmp_vpst_add, "vcmp.s32 gt, q0, q1\n\tvpst\n\tvaddt.i32 q2, q0, q1")
MOP(m_cmp_vpst_16, "vcmp.u16 hi, q0, q1\n\tvpst\n\tvsubt.i16 q2, q0, q1")
MOP(m_cmp_vpst_8, "vcmp.i8 ne, q0, q1\n\tvpstt\n\tvmult.i8 q2, q0, q1\n\tvaddt.i8 q2, q2, q1")
MOP(m_cmp_scalar, "vcmp.s8 le, q0, r4\n\tvpst\n\tvmovt q2, q1\n\tvcmp.i16 eq, q1, zr\n\tvpst\n\tvaddt.i16 q2, q2, q0")
MOP(m_cmp_f32, "vcmp.f32 ne, q0, q1\n\tvpst\n\tvaddt.f32 q2, q0, q1\n\tvcmp.f32 ge, q0, q1\n\tvpst\n\tvsubt.f32 q2, q2, q1")
MOP(m_vpt, "vpt.s16 ge, q0, q1\n\tvmult.i16 q2, q0, q1\n\tvpte.s32 lt, q0, q1\n\tvaddt.i32 q2, q2, q1\n\tvsube.i32 q2, q2, q0")
MOP(m_vpsel, "vcmp.s32 ge, q0, q1\n\tvpsel q2, q0, q1")
MOP(m_ctp8, "and r4, r4, #31\n\tvctp.8 r4\n\tvpst\n\tvsubt.i8 q2, q0, q1")
MOP(m_ctp16, "and r4, r4, #15\n\tvctp.16 r4\n\tvpst\n\tvaddt.i16 q2, q0, q1")
MOP(m_ctp32, "and r4, r4, #7\n\tvctp.32 r4\n\tvpst\n\tvmult.i32 q2, q0, q1")
MOP(m_ctp64, "and r4, r4, #3\n\tvctp.64 r4\n\tvpst\n\tvorrt q2, q0, q1")
MOP(m_qdmulh_s16, "vqdmulh.s16 q2, q0, q1")
MOP(m_qdmulh_s32, "vqdmulh.s32 q2, q0, q1")
MOP(m_qrdmulh, "vqrdmulh.s32 q2, q0, q1\n\tvqrdmulh.s16 q1, q0, q1\n\tveor q2, q2, q1\n\tvqdmulh.s8 q1, q1, q0\n\tveor q2, q2, q1")
MOP(m_qdmulh_scalar, "vqdmulh.s32 q2, q0, r4\n\tvqrdmulh.s16 q1, q1, r5\n\tveor q2, q2, q1")
MOP(m_qdmlah, "vqdmlah.s32 q2, q0, r4\n\tvqrdmlah.s16 q2, q1, r5")
MOP(m_qadd, "vqadd.s8 q2, q0, q1\n\tvqadd.u16 q1, q1, q0\n\tveor q2, q2, q1\n\tvqadd.s32 q1, q0, r4\n\tveor q2, q2, q1")
MOP(m_qsub, "vqsub.s32 q2, q0, q1\n\tvqsub.u16 q1, q1, q0\n\tveor q2, q2, q1\n\tvqsub.u8 q1, q0, r5\n\tveor q2, q2, q1")
MOP(m_cvt_f32_s32, "vcvt.f32.s32 q2, q0")
MOP(m_cvt_f32_u32, "vcvt.f32.u32 q2, q0")
MOP(m_cvt_s32_f32, "vcvt.s32.f32 q2, q0")
MOP(m_cvt_u32_f32, "vcvt.u32.f32 q2, q0")
MOP(m_cvt_fix, "vcvt.f32.s32 q2, q0, #4\n\tvcvt.u32.f32 q1, q1, #7\n\tveor q2, q2, q1")
MOP(m_cvt_rounding, "vcvtn.s32.f32 q2, q0\n\tvcvta.u32.f32 q1, q0\n\tveor q2, q2, q1\n\tvcvtp.s32.f32 q1, q0\n\tveor q2, q2, q1\n\tvcvtm.u32.f32 q1, q0\n\tveor q2, q2, q1")
MOP(m_cvt_f16, "vcvtb.f16.f32 q2, q0\n\tvcvtt.f16.f32 q2, q1")
MOP(m_cvt_f16_f32, "vcvtb.f32.f16 q2, q0\n\tvcvtt.f32.f16 q1, q0\n\tvadd.f32 q2, q2, q1")
MOP(m_cvt_f16_s16, "vcvt.f16.s16 q2, q0\n\tvcvt.s16.f16 q1, q1\n\tveor q2, q2, q1")
MOP(m_movl, "vmovlb.s8 q2, q0\n\tvmovlt.u16 q1, q1\n\tveor q2, q2, q1\n\tvmovlt.s16 q1, q0\n\tveor q2, q2, q1\n\tvmovlb.u8 q1, q0\n\tveor q2, q2, q1")
MOP(m_movn, "vmovnb.i32 q2, q0\n\tvmovnt.i16 q2, q1\n\tvqmovnb.s32 q1, q0\n\tveor q2, q2, q1\n\tvqmovunt.s16 q2, q0")
MOP(m_abs_neg, "vabs.s8 q2, q0\n\tvneg.s32 q1, q1\n\tveor q2, q2, q1\n\tvabs.s16 q1, q0\n\tveor q2, q2, q1\n\tvneg.s8 q1, q0\n\tveor q2, q2, q1\n\tvqabs.s32 q1, q0\n\tveor q2, q2, q1\n\tvqneg.s16 q1, q1\n\tveor q2, q2, q1")
MOP(m_clz_cls, "vclz.i16 q2, q0\n\tvcls.s8 q1, q1\n\tveor q2, q2, q1\n\tvclz.i32 q1, q0\n\tveor q2, q2, q1\n\tvcls.s16 q1, q0\n\tveor q2, q2, q1\n\tvclz.i8 q1, q1\n\tveor q2, q2, q1")
MOP(m_hadd, "vhadd.s8 q2, q0, q1\n\tvhadd.u16 q1, q1, q0\n\tveor q2, q2, q1\n\tvhadd.s32 q1, q0, r4\n\tveor q2, q2, q1\n\tvhsub.s16 q1, q0, q1\n\tveor q2, q2, q1")
MOP(m_rhadd, "vrhadd.u16 q2, q0, q1\n\tvrhadd.s8 q1, q1, q0\n\tveor q2, q2, q1\n\tvrhadd.s32 q1, q0, q1\n\tveor q2, q2, q1")
MOP(m_abd, "vabd.s32 q2, q0, q1\n\tvabd.u8 q1, q1, q0\n\tveor q2, q2, q1\n\tvabd.s16 q1, q0, q1\n\tveor q2, q2, q1")
MOP(m_max_min, "vmax.s8 q2, q0, q1\n\tvmin.u32 q1, q1, q0\n\tveor q2, q2, q1\n\tvmax.u16 q1, q0, q1\n\tveor q2, q2, q1\n\tvmin.s32 q1, q0, q1\n\tveor q2, q2, q1")
MOP(m_mla_scalar, "vmla.s32 q2, q0, r4\n\tvmlas.s16 q1, q0, r5\n\tveor q2, q2, q1\n\tvmla.u8 q2, q1, r4")
MOP(m_mull, "vmullb.s8 q2, q0, q1\n\tvmullt.u16 q1, q1, q0\n\tveor q2, q2, q1\n\tvmullb.s32 q1, q0, q2\n\tveor q2, q2, q1")
MOP(m_rev, "vrev16.8 q2, q0\n\tvrev32.16 q1, q1\n\tveor q2, q2, q1\n\tvrev64.32 q1, q0\n\tveor q2, q2, q1\n\tvrev32.8 q1, q0\n\tveor q2, q2, q1")
MOP(m_brsr, "vbrsr.32 q2, q0, r4\n\tvbrsr.8 q1, q1, r5\n\tveor q2, q2, q1")
MOP(m_idup, "and r5, r5, #0x7f\n\torr r5, r5, #1\n\tvidup.u32 q2, r4, #4\n\tvddup.u8 q1, r4, #1\n\tveor q2, q2, q1\n\tvdwdup.u16 q1, r4, r5, #2\n\tveor q2, q2, q1\n\tviwdup.u8 q1, r4, r5, #8\n\tveor q2, q2, q1")
MOP(m_mov_lane, "vmov r4, q0[3]\n\tvmov.s8 r5, q1[5]\n\tvmov.16 q2[3], r4\n\tvmov.32 q2[0], r5\n\tvmov.u16 r5, q0[7]\n\tvmov.8 q2[15], r5\n\tvmov r4, r5, q1[2], q1[0]\n\tvmov q2[3], q2[1], r4, r5")
MOP(m_cadd, "vcadd.i32 q2, q0, q1, #90\n\tvcadd.i16 q1, q1, q0, #270\n\tveor q2, q2, q1\n\tvhcadd.s8 q1, q0, q1, #90\n\tveor q2, q2, q1")
MOP(m_cmul_f32, "vcmul.f32 q2, q0, q1, #90\n\tvcmla.f32 q2, q0, q1, #180")
MOP(m_mvn_imm, "vmvn.i32 q2, #0xff\n\tveor q2, q2, q0\n\tvmvn.i16 q1, #0x4000\n\tveor q2, q2, q1")
MOP(m_predicated_scalar, "vcmp.s8 gt, q0, q1\n\tvpst\n\tvaddvat.s8 r4, q0\n\tvpst\n\tvmaxvt.s16 r5, q1")
MOP(m_p0_write, "uxth r4, r4\n\tvmsr p0, r4\n\tvpst\n\tvaddt.i8 q2, q0, q1\n\tvmrs r5, p0")

typedef void (*mop_t)(const uint32_t *, const uint32_t *, uint32_t *, uint32_t *, uint32_t, uint32_t *, uint32_t *);

enum { M_INT, M_FLOAT };

static const uint32_t m_modes[] = { LTP, LTP | 0x00C00000, LTP | 0x01000000, LTP | 0x02880000 };

static void run_mop(const char *name, mop_t f, int kind) {
  BEGIN(name);
  unsigned nmodes = kind == M_FLOAT ? 4 : 1;
  for (unsigned m = 0; m < nmodes; m++) {
    for (int i = 0; i < 150; i++) {
      uint32_t a[4] __attribute__((aligned(16))), b[4] __attribute__((aligned(16))), o[4] __attribute__((aligned(16)));
      uint32_t s[2], st, vp;
      for (int j = 0; j < 4; j++) {
        a[j] = kind == M_FLOAT ? fp_operand(4 * i + j, 0) : operand(4 * i + j, 0);
        b[j] = kind == M_FLOAT ? fp_operand(4 * i + j, 1) : operand(4 * i + j, 1);
        o[j] = rnd();
      }
      s[0] = operand(i + 11, 1) ^ (rnd() & 0xFF);
      s[1] = rnd();
      f(a, b, o, s, m_modes[m] | (rnd() & 0x9F), &st, &vp);
      for (int j = 0; j < 4; j++) mix(o[j]);
      mix(s[0]); mix(s[1]); mix(st); mix(vp);
    }
  }
  report(name, digest);
}

/* ------------------------------------------------------------------ loads, stores and tail loops */

static uint8_t mbuf[256] __attribute__((aligned(16)));
static uint8_t mout[256] __attribute__((aligned(16)));

static void fill(void) {
  for (int i = 0; i < 256; i++) { mbuf[i] = (uint8_t)(i * 73 + 19); mout[i] = 0; }
}

static void mix_out(void) {
  for (int i = 0; i < 256; i += 4) mix(*(uint32_t *)(mout + i));
}

static void run_mve_ldst(void) {
  BEGIN("mve_ldst");
  fill();
  for (int i = 0; i < 60; i++) {
    uint32_t off = rnd() % 120, st;
    const uint8_t *p = mbuf + off;
    uint8_t *q = mout + (rnd() % 100);
    __asm volatile("vmsr vpr, %[z]\n\t"
                   "vldrb.u8 q0, [%[p]]\n\tvstrb.8 q0, [%[q]]\n\t"
                   "vldrb.s16 q0, [%[p], #3]\n\tvstrh.16 q0, [%[q], #16]\n\t"
                   "vldrb.u32 q0, [%[p], #1]\n\tvstrw.32 q0, [%[q], #32]\n\t"
                   "vldrh.u16 q0, [%[p], #2]\n\tvstrb.16 q0, [%[q], #48]\n\t"
                   "vldrh.s32 q0, [%[p], #-2]\n\tvstrh.32 q0, [%[q], #56]\n\t"
                   "vldrw.u32 q0, [%[p], #4]\n\tvstrb.32 q0, [%[q], #64]\n\t"
                   "vldrb.s8 q0, [%[p], #7]\n\tvstrb.8 q0, [%[q], #68]\n\t"
                   "vldrh.u32 q0, [%[p], #6]\n\tvstrw.32 q0, [%[q], #84]\n\t"
                   "vldrb.u16 q0, [%[p], #5]\n\tvstrh.16 q0, [%[q], #100]\n\t"
                   "vmrs %[st], fpscr"
                   : [st] "=&r"(st) : [p] "r"(p + 8), [q] "r"(q), [z] "r"(0) : "q0", "memory");
    mix(st);
  }
  mix_out();
  /* writeback forms */
  const uint8_t *p = mbuf + 16;
  uint8_t *q = mout + 20;
  __asm volatile("vldrw.u32 q0, [%[p]], #16\n\tvstrw.32 q0, [%[q]], #16\n\t"
                 "vldrw.u32 q0, [%[p], #16]!\n\tvstrw.32 q0, [%[q], #32]!\n\t"
                 "vldrb.u8 q0, [%[p]], #-8\n\tvstrb.8 q0, [%[q]], #-4\n\t"
                 "vldrh.u16 q0, [%[p], #-16]!\n\tvstrh.16 q0, [%[q], #-16]!\n\t"
                 "vldrw.u32 q0, [%[p], #-32]\n\tvstrw.32 q0, [%[q], #124]"
                 : [p] "+r"(p), [q] "+r"(q) :: "q0", "memory");
  mix((uint32_t)(p - mbuf)); mix((uint32_t)(q - mout));
  mix_out();
  /* gather loads and scatter stores */
  uint32_t idx[4] __attribute__((aligned(16))) = { 3, 40, 7, 100 };
  uint32_t idx16[8] __attribute__((aligned(16)));
  for (int i = 0; i < 8; i++) ((uint16_t *)idx16)[i] = (uint16_t)(rnd() % 120);
  __asm volatile("vldrw.u32 q1, [%[i]]\n\t"
                 "vldrw.u32 q0, [%[p], q1, uxtw #2]\n\tvstrw.32 q0, [%[q], #160]\n\t"
                 "vldrb.u32 q0, [%[p], q1]\n\tvstrw.32 q0, [%[q], #176]\n\t"
                 "vldrh.u32 q0, [%[p], q1, uxtw #1]\n\tvstrw.32 q0, [%[q], #192]\n\t"
                 "vldrw.u32 q0, [%[p], q1]\n\tvstrw.32 q0, [%[q], q1, uxtw #2]\n\t"
                 "vstrb.32 q0, [%[q], q1]\n\tvstrh.32 q0, [%[q], q1, uxtw #1]\n\t"
                 "vldrw.u32 q1, [%[j]]\n\tvldrb.u16 q0, [%[p], q1]\n\tvstrh.16 q0, [%[q], #208]\n\t"
                 "vldrh.u16 q0, [%[p], q1]\n\tvstrh.16 q0, [%[q], q1]\n\t"
                 "vldrw.u32 q1, [%[i]]\n\tvadd.i32 q1, q1, %[base]\n\t"
                 "vldrw.u32 q0, [q1, #4]\n\tvstrw.32 q0, [%[q], #224]\n\tvstrw.32 q0, [q1, #64]\n\t"
                 "vldrd.u64 q0, [q1, #8]\n\tvstrd.64 q0, [q1, #128]\n\t"
                 "vldrw.u32 q0, [q1, #64]!\n\tvstrw.32 q0, [%[q], #240]\n\tvstrw.32 q1, [%[q], #0]"
                 :: [i] "r"(idx), [j] "r"(idx16), [p] "r"(mbuf), [q] "r"(mout), [base] "r"((uint32_t)mout) : "q0", "q1", "memory");
  mix_out();
  /* interleaving loads/stores */
  __asm volatile("vld20.8 {q0, q1}, [%[p]]\n\tvld21.8 {q0, q1}, [%[p]]\n\tvst20.16 {q0, q1}, [%[q]]\n\tvst21.16 {q0, q1}, [%[q]]\n\t"
                 "vld40.32 {q0, q1, q2, q3}, [%[p]]\n\tvld41.32 {q0, q1, q2, q3}, [%[p]]\n\tvld42.32 {q0, q1, q2, q3}, [%[p]]\n\tvld43.32 {q0, q1, q2, q3}, [%[p]]!\n\t"
                 "vst40.8 {q0, q1, q2, q3}, [%[r]]\n\tvst41.8 {q0, q1, q2, q3}, [%[r]]\n\tvst42.8 {q0, q1, q2, q3}, [%[r]]\n\tvst43.8 {q0, q1, q2, q3}, [%[r]]!"
                 : [p] "+r"(p), [q] "+r"(q) : [r] "r"(mout + 64) : "q0", "q1", "q2", "q3", "memory");
  mix((uint32_t)(p - mbuf));
  mix_out();
  report("mve_ldst", digest);
}

static void run_mve_tail(void) {
  BEGIN("mve_tail");
  static const uint32_t counts[] = { 37, 16, 1, 0, 5, 33, 100 };
  for (unsigned k = 0; k < sizeof(counts) / sizeof(counts[0]); k++) {
    fill();
    const uint8_t *src = mbuf;
    uint8_t *dst = mout;
    uint32_t n = counts[k], st, lr_after;
    __asm volatile("vmov.i8 q1, #3\n\tvmsr vpr, %[z]\n\t"
                   "dlstp.8 lr, %[n]\n"
                   "1:\tvldrb.u8 q0, [%[s]], #16\n\tvadd.i8 q0, q0, q1\n\tvstrb.8 q0, [%[d]], #16\n\tletp lr, 1b\n\t"
                   "mov %[l], lr\n\tvmrs %[st], fpscr"
                   : [s] "+r"(src), [d] "+r"(dst), [st] "=&r"(st), [l] "=&r"(lr_after) : [n] "r"(n), [z] "r"(0) : "q0", "q1", "lr", "memory", "cc");
    mix((uint32_t)(src - mbuf)); mix((uint32_t)(dst - mout)); mix(st); mix(lr_after);
    mix_out();
    /* WLSTP.32 over words: a count of 0 skips the loop */
    fill();
    src = mbuf; dst = mout;
    __asm volatile("vmov.i32 q1, #0x10000\n\tvorr.i32 q1, #0x1\n\t"
                   "wlstp.32 lr, %[n], 2f\n"
                   "1:\tvldrw.u32 q0, [%[s]], #16\n\tvmul.i32 q0, q0, q1\n\tvstrw.32 q0, [%[d]], #16\n\tletp lr, 1b\n"
                   "2:\tmov %[l], lr\n\tvmrs %[st], fpscr"
                   : [s] "+r"(src), [d] "+r"(dst), [st] "=&r"(st), [l] "=&r"(lr_after) : [n] "r"(n) : "q0", "q1", "lr", "memory", "cc");
    mix((uint32_t)(src - mbuf)); mix((uint32_t)(dst - mout)); mix(st); mix(lr_after);
    mix_out();
    /* DLSTP.16 with VMLADAVA accumulation and a VCTP inside */
    src = mbuf;
    uint32_t acc;
    register uint32_t r4 __asm("r4") = 0;
    __asm volatile("dlstp.16 lr, %[n]\n"
                   "1:\tvldrh.u16 q0, [%[s]], #16\n\tvldrh.s16 q1, [%[s], #-8]\n\tvmladava.s16 r4, q0, q1\n\tletp lr, 1b\n\t"
                   "vmrs %[st], fpscr"
                   : [s] "+r"(src), "+r"(r4), [st] "=&r"(st) : [n] "r"(n) : "q0", "q1", "lr", "memory", "cc");
    acc = r4;
    mix(acc); mix((uint32_t)(src - mbuf)); mix(st);
    /* LCTP after an exited loop */
    __asm volatile("dlstp.32 lr, %[n]\n1:\tadd %[s], #4\n\tcmp %[s], %[e]\n\tbhs 2f\n\tletp lr, 1b\n2:\tvmrs %[st], fpscr\n\tlctp\n\tvmrs %[l], fpscr"
                   : [s] "+r"(src), [st] "=&r"(st), [l] "=&r"(lr_after) : [n] "r"(n + 1), [e] "r"(mbuf + 24) : "lr", "memory", "cc");
    mix(st); mix(lr_after); mix((uint32_t)(src - mbuf));
  }
  report("mve_tail", digest);
}

void run_mve(void) {
  run_mop("mve_add_i8", m_add_i8, M_INT);
  run_mop("mve_add_i16", m_add_i16, M_INT);
  run_mop("mve_add_i32", m_add_i32, M_INT);
  run_mop("mve_sub_i8", m_sub_i8, M_INT);
  run_mop("mve_sub_i16", m_sub_i16, M_INT);
  run_mop("mve_sub_i32", m_sub_i32, M_INT);
  run_mop("mve_mul_i8", m_mul_i8, M_INT);
  run_mop("mve_mul_i16", m_mul_i16, M_INT);
  run_mop("mve_mul_i32", m_mul_i32, M_INT);
  run_mop("mve_mulh", m_mulh, M_INT);
  run_mop("mve_add_scalar", m_add_scalar, M_INT);
  run_mop("mve_add_f32", m_add_f32, M_FLOAT);
  run_mop("mve_sub_f32", m_sub_f32, M_FLOAT);
  run_mop("mve_mul_f32", m_mul_f32, M_FLOAT);
  run_mop("mve_fma_f32", m_fma_f32, M_FLOAT);
  run_mop("mve_fms_f32", m_fms_f32, M_FLOAT);
  run_mop("mve_fmas_f32", m_fmas_f32, M_FLOAT);
  run_mop("mve_maxnm_f32", m_maxnm_f32, M_FLOAT);
  run_mop("mve_minnm_f32", m_minnm_f32, M_FLOAT);
  run_mop("mve_abs_neg_f32", m_abs_neg_f32, M_FLOAT);
  run_mop("mve_rint_f32", m_rint_f32, M_FLOAT);
  run_mop("mve_add_f16", m_add_f16, M_FLOAT);
  run_mop("mve_mul_f16", m_mul_f16, M_FLOAT);
  run_mop("mve_fma_f16", m_fma_f16, M_FLOAT);
  run_mop("mve_and", m_and, M_INT);
  run_mop("mve_orr", m_orr, M_INT);
  run_mop("mve_eor", m_eor, M_INT);
  run_mop("mve_bic_orn", m_bic_orn, M_INT);
  run_mop("mve_mov_imm", m_mov_imm, M_INT);
  run_mop("mve_orr_bic_imm", m_orr_bic_imm, M_INT);
  run_mop("mve_mvn_imm", m_mvn_imm, M_INT);
  run_mop("mve_dup", m_dup, M_INT);
  run_mop("mve_mladava_s8", m_mladava_s8, M_INT);
  run_mop("mve_mladava_s16", m_mladava_s16, M_INT);
  run_mop("mve_mladava_s32", m_mladava_s32, M_INT);
  run_mop("mve_mladava_u8", m_mladava_u8, M_INT);
  run_mop("mve_mlsdava", m_mlsdava, M_INT);
  run_mop("mve_mlaldava_s32", m_mlaldava_s32, M_INT);
  run_mop("mve_mlaldava_s16", m_mlaldava_s16, M_INT);
  run_mop("mve_mlaldava_u32", m_mlaldava_u32, M_INT);
  run_mop("mve_mlsldava_s32", m_mlsldava_s32, M_INT);
  run_mop("mve_rmlaldavh_s32", m_rmlaldavh_s32, M_INT);
  run_mop("mve_rmlsldavh_s32", m_rmlsldavh_s32, M_INT);
  run_mop("mve_rmlalvh", m_rmlalvh, M_INT);
  run_mop("mve_maxv_minv", m_maxv_minv, M_INT);
  run_mop("mve_addv", m_addv, M_INT);
  run_mop("mve_addlva", m_addlva, M_INT);
  run_mop("mve_shl", m_shl, M_INT);
  run_mop("mve_shl_imm", m_shl_imm, M_INT);
  run_mop("mve_shr_imm", m_shr_imm, M_INT);
  run_mop("mve_qshl", m_qshl, M_INT);
  run_mop("mve_shl_scalar", m_shl_scalar, M_INT);
  run_mop("mve_shll", m_shll, M_INT);
  run_mop("mve_sli_sri", m_sli_sri, M_INT);
  run_mop("mve_shlc", m_shlc, M_INT);
  run_mop("mve_cmp_vpst_add", m_cmp_vpst_add, M_INT);
  run_mop("mve_cmp_vpst_16", m_cmp_vpst_16, M_INT);
  run_mop("mve_cmp_vpst_8", m_cmp_vpst_8, M_INT);
  run_mop("mve_cmp_scalar", m_cmp_scalar, M_INT);
  run_mop("mve_cmp_f32", m_cmp_f32, M_FLOAT);
  run_mop("mve_vpt", m_vpt, M_INT);
  run_mop("mve_vpsel", m_vpsel, M_INT);
  run_mop("mve_ctp8", m_ctp8, M_INT);
  run_mop("mve_ctp16", m_ctp16, M_INT);
  run_mop("mve_ctp32", m_ctp32, M_INT);
  run_mop("mve_ctp64", m_ctp64, M_INT);
  run_mop("mve_predicated_scalar", m_predicated_scalar, M_INT);
  run_mop("mve_p0_write", m_p0_write, M_INT);
  run_mop("mve_qdmulh_s16", m_qdmulh_s16, M_INT);
  run_mop("mve_qdmulh_s32", m_qdmulh_s32, M_INT);
  run_mop("mve_qrdmulh", m_qrdmulh, M_INT);
  run_mop("mve_qdmulh_scalar", m_qdmulh_scalar, M_INT);
  run_mop("mve_qdmlah", m_qdmlah, M_INT);
  run_mop("mve_qadd", m_qadd, M_INT);
  run_mop("mve_qsub", m_qsub, M_INT);
  run_mop("mve_cvt_f32_s32", m_cvt_f32_s32, M_INT);
  run_mop("mve_cvt_f32_u32", m_cvt_f32_u32, M_INT);
  run_mop("mve_cvt_s32_f32", m_cvt_s32_f32, M_FLOAT);
  run_mop("mve_cvt_u32_f32", m_cvt_u32_f32, M_FLOAT);
  run_mop("mve_cvt_fix", m_cvt_fix, M_FLOAT);
  run_mop("mve_cvt_rounding", m_cvt_rounding, M_FLOAT);
  run_mop("mve_cvt_f16", m_cvt_f16, M_FLOAT);
  run_mop("mve_cvt_f16_f32", m_cvt_f16_f32, M_FLOAT);
  run_mop("mve_cvt_f16_s16", m_cvt_f16_s16, M_INT);
  run_mop("mve_movl", m_movl, M_INT);
  run_mop("mve_movn", m_movn, M_INT);
  run_mop("mve_abs_neg", m_abs_neg, M_INT);
  run_mop("mve_clz_cls", m_clz_cls, M_INT);
  run_mop("mve_hadd", m_hadd, M_INT);
  run_mop("mve_rhadd", m_rhadd, M_INT);
  run_mop("mve_abd", m_abd, M_INT);
  run_mop("mve_max_min", m_max_min, M_INT);
  run_mop("mve_mla_scalar", m_mla_scalar, M_INT);
  run_mop("mve_mull", m_mull, M_INT);
  run_mop("mve_rev", m_rev, M_INT);
  run_mop("mve_brsr", m_brsr, M_INT);
  run_mop("mve_idup", m_idup, M_INT);
  run_mop("mve_mov_lane", m_mov_lane, M_INT);
  run_mop("mve_cadd", m_cadd, M_INT);
  run_mop("mve_cmul_f32", m_cmul_f32, M_FLOAT);
  run_mve_ldst();
  run_mve_tail();
}
