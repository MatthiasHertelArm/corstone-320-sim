/* Corstone-320 (FVP_Corstone_SSE-320 and corstone-320-sim), NPU test:
   code in the Secure boot ROM at 0x11000000 (128 KB, loaded with -a, INITSVTOR = 0x11000000),
   .data/.bss and the stack in the Secure DTCM at 0x30000000 (32 KB; the NPU cannot reach TCMs),
   network blobs (.netdata) in the Secure DDR at 0x70000000, the NPU arena (region 1) in the
   Secure BRAM alias at 0x12000000 and the NPU fast scratch (region 2) in the Secure internal SRAM
   at 0x31000000. */
MEMORY
{
  ROM   (rx)  : ORIGIN = 0x11000000, LENGTH = 128K
  DTCM  (rwx) : ORIGIN = 0x30000000, LENGTH = 32K
  BRAM  (rw)  : ORIGIN = 0x12000000, LENGTH = 2M
  ISRAM (rw)  : ORIGIN = 0x31000000, LENGTH = 2M
  DDR   (rw)  : ORIGIN = 0x70000000, LENGTH = 256M
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
    end = .;
  } > DTCM
  __stack_top = ORIGIN(DTCM) + LENGTH(DTCM);
  .netdata : {
    . = ALIGN(16);
    KEEP(*(.netdata*))
    . = ALIGN(16);
  } > DDR
  __arena_start = ORIGIN(BRAM);
  __arena_end = ORIGIN(BRAM) + LENGTH(BRAM);
  __fast_start = ORIGIN(ISRAM);
  __fast_end = ORIGIN(ISRAM) + LENGTH(ISRAM);
  /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) }
}
