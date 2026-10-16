#!/bin/sh
# Builds the Armv8.1-M ISA differential test (Arm GNU Toolchain 13 on PATH or ARM_GCC_BIN).
# isa_test.c and startup.c are built without MVE so that no Helium instruction appears before
# the `mve` marker; mve_test.c holds the MVE section. The ELF goes to out/ and to fixtures/elf/.
#
#   ./build.sh                      -> out/isa_test.elf, copied to ../../fixtures/elf/isa_test.elf
#   ./build.sh --verbose "vadd,tt"  -> out/isa_test_verbose.elf: prints every value mixed into
#                                      the digests of the listed groups (not copied)
set -e
cd "$(dirname "$0")"
GCC="${ARM_GCC_BIN:+$ARM_GCC_BIN/}arm-none-eabi-gcc"
COMMON="-mthumb -mfloat-abi=hard -O1 -g -ffreestanding -nostdlib -Wall -Wno-unused-variable"
SCALAR="-march=armv8.1-m.main+dsp+fp.dp+pacbti"
OUT=out/isa_test.elf
EXTRA=""
if [ "$1" = "--verbose" ]; then
  EXTRA="-DVERBOSE_GROUPS=\"$2\""
  OUT=out/isa_test_verbose.elf
fi
mkdir -p out
"$GCC" $SCALAR $COMMON -c startup.c -o out/startup.o
"$GCC" $SCALAR $COMMON $EXTRA $CFLAGS_EXTRA -c isa_test.c -o out/isa_test.o
"$GCC" -mcpu=cortex-m85 $COMMON -c mve_test.c -o out/mve_test.o
"$GCC" -mcpu=cortex-m85 $COMMON -T link.ld out/startup.o out/isa_test.o out/mve_test.o -lgcc -o "$OUT"
if [ "$OUT" = out/isa_test.elf ]; then
  cp out/isa_test.elf ../../fixtures/elf/isa_test.elf
  echo "out/isa_test.elf (copied to fixtures/elf/isa_test.elf)"
else
  echo "$OUT"
fi
