/* Corstone-320 (FVP_Corstone_SSE-320 and corstone-320-sim): code in the Secure boot ROM at
   0x11000000 (128 KB, loaded with -a, INITSVTOR = 0x11000000), .data/.bss and the stack in the
   Secure DTCM at 0x30000000 (32 KB). */
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
  __stack_top = ORIGIN(DTCM) + LENGTH(DTCM);
}
