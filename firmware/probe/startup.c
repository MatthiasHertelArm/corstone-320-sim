/* Startup for the Corstone-320 probe: vector table, a naked reset handler that snapshots the
   core registers before any of them is touched, .data/.bss init, FPU enable, fault handlers that
   skip the faulting instruction so a read of an unimplemented register does not end the run. */
#include <stdint.h>

extern uint32_t __etext, __data_start, __data_end, __bss_start, __bss_end, __stack_top, __reset_snapshot;
extern int main(void);
void semihost_exit(int code);

/* Set by the fault handler; the probe prints FAULT instead of a value when it is set. */
volatile uint32_t fault_count;
volatile uint32_t last_fault_ipsr;
volatile uint32_t last_fault_pc;

/* Secure-state snapshot layout (words), filled by Reset_Handler at __reset_snapshot:
   0..12 r0-r12, 13 lr, 14 msp, 15 psp, 16 msplim, 17 psplim, 18 control, 19 primask,
   20 basepri, 21 faultmask, 22 xpsr, 23 msp_ns, 24 psp_ns, 25 msplim_ns, 26 psplim_ns,
   27 control_ns, 28 primask_ns, 29 basepri_ns, 30 faultmask_ns, 31 sp_ns, 32 fpscr */
__attribute__((naked, noreturn)) void Reset_Handler(void) {
  __asm volatile(
      /* sp == __stack_top == __reset_snapshot: store r0-r12 and lr above the stack */
      "str r0, [sp, #0]\n str r1, [sp, #4]\n str r2, [sp, #8]\n str r3, [sp, #12]\n"
      "str r4, [sp, #16]\n str r5, [sp, #20]\n str r6, [sp, #24]\n str r7, [sp, #28]\n"
      "str r8, [sp, #32]\n str r9, [sp, #36]\n str r10, [sp, #40]\n str r11, [sp, #44]\n"
      "str r12, [sp, #48]\n str lr, [sp, #52]\n"
      "mrs r0, MSP\n str r0, [sp, #56]\n"
      "mrs r0, PSP\n str r0, [sp, #60]\n"
      "mrs r0, MSPLIM\n str r0, [sp, #64]\n"
      "mrs r0, PSPLIM\n str r0, [sp, #68]\n"
      "mrs r0, CONTROL\n str r0, [sp, #72]\n"
      "mrs r0, PRIMASK\n str r0, [sp, #76]\n"
      "mrs r0, BASEPRI\n str r0, [sp, #80]\n"
      "mrs r0, FAULTMASK\n str r0, [sp, #84]\n"
      "mrs r0, XPSR\n str r0, [sp, #88]\n"
      "mrs r0, MSP_NS\n str r0, [sp, #92]\n"
      "mrs r0, PSP_NS\n str r0, [sp, #96]\n"
      "mrs r0, MSPLIM_NS\n str r0, [sp, #100]\n"
      "mrs r0, PSPLIM_NS\n str r0, [sp, #104]\n"
      "mrs r0, CONTROL_NS\n str r0, [sp, #108]\n"
      "mrs r0, PRIMASK_NS\n str r0, [sp, #112]\n"
      "mrs r0, BASEPRI_NS\n str r0, [sp, #116]\n"
      "mrs r0, FAULTMASK_NS\n str r0, [sp, #120]\n"
      "mrs r0, SP_NS\n str r0, [sp, #124]\n"
      "b Reset_C\n");
}

__attribute__((noreturn)) void Reset_C(void) {
  uint32_t *s = &__etext, *d = &__data_start;
  while (d < &__data_end) *d++ = *s++;
  for (d = &__bss_start; d < &__bss_end;) *d++ = 0;
  *(volatile uint32_t *)0xE000ED88 |= (0xFu << 20);   /* CPACR: CP10, CP11 full access */
  __asm volatile("dsb\n isb");
  {
    uint32_t fpscr;
    __asm volatile("vmrs %0, fpscr" : "=r"(fpscr));
    (&__reset_snapshot)[32] = fpscr;
  }
  semihost_exit(main());
  for (;;) {}
}

/* Fault handler: record, skip the faulting instruction (16- or 32-bit Thumb) and return. */
void Fault_C(uint32_t *frame, uint32_t ipsr) {
  uint32_t pc = frame[6];
  uint16_t hw = *(volatile uint16_t *)pc;
  uint32_t len = ((hw >> 11) >= 0x1D) ? 4 : 2;
  fault_count++;
  last_fault_ipsr = ipsr;
  last_fault_pc = pc;
  frame[6] = pc + len;
  /* clear sticky fault status so the next fault is reported freshly */
  *(volatile uint32_t *)0xE000ED28 = 0xFFFFFFFFu;  /* CFSR */
  *(volatile uint32_t *)0xE000ED2C = 0xFFFFFFFFu;  /* HFSR */
  *(volatile uint32_t *)0xE000EDE4 = 0xFFFFFFFFu;  /* SFSR */
}

__attribute__((naked)) void Fault_Handler(void) {
  __asm volatile(
      "tst lr, #4\n"
      "ite eq\n"
      "mrseq r0, MSP\n"
      "mrsne r0, PSP\n"
      "mrs r1, IPSR\n"
      "push {lr}\n"
      "bl Fault_C\n"
      "pop {pc}\n");
}

void Default_Handler(void) { for (;;) {} }
void NMI_Handler(void) __attribute__((weak, alias("Default_Handler")));
void SVC_Handler(void) __attribute__((weak, alias("Default_Handler")));
void PendSV_Handler(void) __attribute__((weak, alias("Default_Handler")));
void SysTick_Handler(void) __attribute__((weak, alias("Default_Handler")));

__attribute__((section(".vectors"), used))
void (*const vectors[16 + 16])(void) = {
  (void (*)(void))&__stack_top, Reset_Handler, NMI_Handler, Fault_Handler,
  Fault_Handler, Fault_Handler, Fault_Handler, Fault_Handler, 0, 0, 0,
  SVC_Handler, 0, 0, PendSV_Handler, SysTick_Handler,
};
