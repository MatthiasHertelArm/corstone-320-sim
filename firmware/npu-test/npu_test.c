/*
 * Ethos-U85 differential test for the Corstone-320 simulator.
 *
 * Runs Vela-compiled networks (networks/net_NAME.h) on the NPU through Arm's ethos-u-core-driver and
 * prints, over semihosting (SYS_WRITE0), one `name: 0x........` line per value:
 *   npu.{id,config,status,prot,cmd}.{reset,init,done}  the NPU registers as the CPU reads them
 *                                 before any driver call, after ethosu_init and after all runs
 *   npu.init                      return value of ethosu_init
 *   <net>.cmd, <net>.weights      FNV-1a of the payload and the weights as loaded in memory
 *   <net>.invoke                  return value of ethosu_invoke_v3 (0 = OK)
 *   <net>.job                     job result seen in the inference-end hook (0 OK, 1 timeout, 2 error)
 *   <net>.status, <net>.cmdreg    NPU STATUS and CMD registers in the inference-end hook
 *   <net>.output                  FNV-1a of the whole output tensor
 *   <net>.out_head                the first (up to) 16 output bytes
 *   <net>.cycles_lo/hi            NPU PMU cycle counter, counting from NPU_ACTIVE to NPU_IDLE
 *                                 (timing of the platform: not compared with the simulator)
 * followed by `npu-test end`. The record on the FVP is fixtures/npu_test.fvp.txt.
 *
 * Memory: code in the Secure boot ROM, .data/.bss/stack in the DTCM, network blobs in Secure DDR
 * (.netdata, 0x70000000), arena (region 1) in the Secure BRAM alias 0x12000000, fast scratch
 * (region 2) in the Secure internal SRAM 0x31000000. NPU registers at the Secure alias 0x50004000,
 * IRQ 16. The NPU is initialised secure and privileged (the CPU boots Secure, privileged).
 */
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include "ethosu_driver.h"
#include "pmu_ethosu.h"
#include "npu_test.h"
#include "networks/networks.h"

#define NPU_BASE 0x50004000u
#define NPU_IRQ 16
#define REG(off) (*(volatile uint32_t *)(NPU_BASE + (off)))
#define REG_ID 0x00
#define REG_STATUS 0x04
#define REG_CMD 0x08
#define REG_PROT 0x24
#define REG_CONFIG 0x28

extern uint8_t __arena_start[], __arena_end[], __fast_start[], __fast_end[];

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

static void report(const char *name, const char *suffix, uint32_t v) {
  puts_(name);
  if (suffix) puts_(suffix);
  puts_(": ");
  put_hex(v);
  puts_("\n");
}

/* The driver's LOG_* macros expand to fprintf(stdout/stderr, ...); build.sh maps fprintf to this. */
int npu_log_printf(void *stream, const char *fmt, ...) {
  (void)stream;
  char buf[192];
  va_list ap;
  va_start(ap, fmt);
  int n = vsnprintf(buf, sizeof buf, fmt, ap);
  va_end(ap);
  puts_(buf);
  return n;
}

/* ------------------------------------------------------------------ digests */

static uint32_t fnv1a(const void *data, size_t n) {
  const uint8_t *p = data;
  uint32_t h = 0x811c9dc5u;
  while (n--) h = (h ^ *p++) * 0x01000193u;
  return h;
}

/* ------------------------------------------------------------------ driver hooks */

struct sem { volatile uint32_t count; };
static struct sem sems[4];
static unsigned nsems;

void *ethosu_semaphore_create(void) { return nsems < 4 ? &sems[nsems++] : NULL; }
void ethosu_semaphore_destroy(void *sem) { (void)sem; }
int ethosu_semaphore_give(void *sem) { ((struct sem *)sem)->count++; return 0; }
/* Polls; `timeout` counts loop iterations (ETHOSU_SEMAPHORE_WAIT_INFERENCE is set in build.sh). */
int ethosu_semaphore_take(void *sem, uint64_t timeout) {
  struct sem *s = sem;
  uint64_t n = 0;
  while (s->count == 0) {
    if (timeout != ETHOSU_SEMAPHORE_WAIT_FOREVER && ++n >= timeout) return -1;
    __asm volatile("nop");
  }
  s->count--;
  return 0;
}

static struct ethosu_driver drv;
static uint64_t run_cycles;
static uint32_t run_status, run_cmd, run_job;

void IRQ16_Handler(void) { ethosu_irq_handler(&drv); }

/* Called by the driver after the power request (so the soft reset is done) and before the command
   stream is started: arm the PMU cycle counter. */
void ethosu_inference_begin(struct ethosu_driver *d, void *user_arg) {
  (void)user_arg;
  ETHOSU_PMU_Enable(d);
  ETHOSU_PMU_PMCCNTR_CFG_Set_Start_Event(d, ETHOSU_PMU_NPU_ACTIVE);
  ETHOSU_PMU_PMCCNTR_CFG_Set_Stop_Event(d, ETHOSU_PMU_NPU_IDLE);
  ETHOSU_PMU_CYCCNT_Reset(d);
  ETHOSU_PMU_CNTR_Enable(d, ETHOSU_PMU_CCNT_Msk);
}

/* Called before the job result is checked and the NPU possibly reset. */
void ethosu_inference_end(struct ethosu_driver *d, void *user_arg) {
  (void)user_arg;
  run_cycles = ETHOSU_PMU_Get_CCNTR(d);
  run_status = REG(REG_STATUS);
  run_cmd = REG(REG_CMD);
  run_job = d->job.result;
  ETHOSU_PMU_CNTR_Disable(d, ETHOSU_PMU_CCNT_Msk);
}

/* ------------------------------------------------------------------ test */

static void report_regs(const char *suffix) {
  report("npu.id", suffix, REG(REG_ID));
  report("npu.config", suffix, REG(REG_CONFIG));
  report("npu.status", suffix, REG(REG_STATUS));
  report("npu.prot", suffix, REG(REG_PROT));
  report("npu.cmd", suffix, REG(REG_CMD));
}

/* Returns 0 on success, 1 on an NPU error (continue with the next network), 2 on a timeout. */
static int run_network(const struct network *net) {
  uint8_t *arena = __arena_start, *fast = __fast_start;
  report(net->name, ".cmd", fnv1a(net->cmd, net->cmd_size));
  report(net->name, ".weights", fnv1a(net->weights, net->weights_size));
  if (net->scratch_size > (size_t)(__arena_end - __arena_start) ||
      net->fast_size > (size_t)(__fast_end - __fast_start) ||
      net->input_offset + net->input_size > net->scratch_size ||
      net->output_offset + net->output_size > net->scratch_size) {
    report(net->name, ".invoke", 0xbad0ba5e);
    return 1;
  }
  memset(arena, 0, net->scratch_size);
  memset(fast, 0, net->fast_size);
  memcpy(arena + net->input_offset, net->input, net->input_size);
  __asm volatile("dsb");

  uint64_t base[3] = { (uintptr_t)net->weights, (uintptr_t)arena, (uintptr_t)fast };
  size_t sizes[3] = { net->weights_size, net->scratch_size, net->fast_size };
  run_cycles = 0;
  run_status = 0;
  run_cmd = 0;
  run_job = 0xff;
  int rc = ethosu_invoke_v3(&drv, net->cmd, (int)net->cmd_size, base, sizes, 3, NULL);
  report(net->name, ".invoke", (uint32_t)rc);
  report(net->name, ".job", run_job);
  report(net->name, ".status", run_status);
  report(net->name, ".cmdreg", run_cmd);
  if (rc == 0) {
    const uint8_t *out = arena + net->output_offset;
    report(net->name, ".output", fnv1a(out, net->output_size));
    puts_(net->name);
    puts_(".out_head: 0x");
    for (size_t i = 0; i < net->output_size && i < 16; i++) {
      char b[3] = { "0123456789abcdef"[out[i] >> 4], "0123456789abcdef"[out[i] & 15], 0 };
      puts_(b);
    }
    puts_("\n");
  }
  report(net->name, ".cycles_lo", (uint32_t)run_cycles);
  report(net->name, ".cycles_hi", (uint32_t)(run_cycles >> 32));
  if (rc != 0) return run_job == ETHOSU_JOB_RESULT_TIMEOUT ? 2 : 1;
  return 0;
}

int main(void) {
  puts_("npu-test begin\n");
  report_regs(".reset");

  /* NVIC: enable IRQ 16 (NPU0_IRQn), default priority. PRIMASK is clear after reset. */
  *(volatile uint32_t *)0xE000E100 = 1u << NPU_IRQ;

  int rc = ethosu_init(&drv, (void *)NPU_BASE, __fast_start, (size_t)(__fast_end - __fast_start), 1, 1);
  report("npu.init", NULL, (uint32_t)rc);
  report_regs(".init");
  if (rc == 0) {
    struct ethosu_hw_info hw;
    ethosu_get_hw_info(&drv, &hw);
    report("npu.hw.version", NULL, (uint32_t)hw.version.product_major << 24 | hw.version.version_major << 16 |
                                     hw.version.version_minor << 8 | hw.version.version_status);
    report("npu.hw.arch", NULL, (uint32_t)hw.version.arch_major_rev << 16 | hw.version.arch_minor_rev << 8 |
                                  hw.version.arch_patch_rev);
    report("npu.hw.cfg", NULL, (uint32_t)hw.cfg.macs_per_cc << 8 | hw.cfg.cmd_stream_version << 4 | hw.cfg.custom_dma);
    for (size_t i = 0; i < NUM_NETWORKS; i++) {
      if (run_network(networks[i]) == 2) {
        puts_("npu-test timeout, remaining networks skipped\n");
        break;
      }
    }
    report_regs(".done");
  }
  puts_("npu-test end\n");
  return 0;
}
