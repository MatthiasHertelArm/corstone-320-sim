/* Corstone-320 FVP: Secure boot ROM at 0x11000000 (128 KB, image loaded with -a, INITSVTOR=0x11000000),
   Secure DTCM at 0x30000000 (32 KB) for .data/.bss and the stack. */
MEMORY
{
  ROM  (rx)  : ORIGIN = 0x11000000, LENGTH = 128K
  DTCM (rwx) : ORIGIN = 0x30000000, LENGTH = 32K
}
ENTRY(Reset_Handler)
SECTIONS
{
  .text : {
    KEEP(*(.vectors))
    *(.text*)
    *(.rodata*)
    . = ALIGN(4);
    __etext = .;
  } > ROM
  .data : AT(__etext) {
    __data_start = .;
    *(.data*)
    . = ALIGN(4);
    __data_end = .;
  } > DTCM
  .bss (NOLOAD) : {
    __bss_start = .;
    *(.bss*)
    *(COMMON)
    . = ALIGN(4);
    __bss_end = .;
  } > DTCM
  /* The last 256 bytes of DTCM hold the register snapshot taken by Reset_Handler before anything
     else runs; the stack grows down from just below it. */
  __reset_snapshot = ORIGIN(DTCM) + LENGTH(DTCM) - 256;
  __stack_top = __reset_snapshot;
}
