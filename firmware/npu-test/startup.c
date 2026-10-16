/* Startup for the Corstone-320 NPU test: vector table (16 system + 480 IRQ entries), .data/.bss
   init, FPU enable, semihosting exit. IRQ16_Handler (Ethos-U85) lives in npu_test.c. */
#include <stdint.h>

extern uint32_t __etext, __data_start, __data_end, __bss_start, __bss_end, __stack_top;
extern int main(void);
void semihost_exit(int code);

void Default_Handler(void) { for (;;) {} }
void NMI_Handler(void) __attribute__((weak, alias("Default_Handler")));
void HardFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void MemManage_Handler(void) __attribute__((weak, alias("Default_Handler")));
void BusFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void UsageFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void SecureFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void SVC_Handler(void) __attribute__((weak, alias("Default_Handler")));
void DebugMon_Handler(void) __attribute__((weak, alias("Default_Handler")));
void PendSV_Handler(void) __attribute__((weak, alias("Default_Handler")));
void SysTick_Handler(void) __attribute__((weak, alias("Default_Handler")));
void IRQ16_Handler(void) __attribute__((weak, alias("Default_Handler")));

void Reset_Handler(void) {
  uint32_t *s = &__etext, *d = &__data_start;
  while (d < &__data_end) *d++ = *s++;
  for (d = &__bss_start; d < &__bss_end;) *d++ = 0;
  *(volatile uint32_t *)0xE000ED88 |= (0xFu << 20);   /* CPACR: CP10, CP11 full access */
  __asm volatile("dsb\n isb");
  semihost_exit(main());
}

__attribute__((section(".vectors"), used))
void (*const vectors[16 + 480])(void) = {
  (void (*)(void))&__stack_top, Reset_Handler, NMI_Handler, HardFault_Handler,
  MemManage_Handler, BusFault_Handler, UsageFault_Handler, SecureFault_Handler, 0, 0, 0,
  SVC_Handler, DebugMon_Handler, 0, PendSV_Handler, SysTick_Handler,
  [16 + 16] = IRQ16_Handler,
};
