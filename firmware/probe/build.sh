#!/bin/sh
# Builds the Corstone-320 FVP probe (Arm GNU Toolchain on PATH or ARM_GCC_BIN).
set -e
cd "$(dirname "$0")"
GCC="${ARM_GCC_BIN:+$ARM_GCC_BIN/}arm-none-eabi-gcc"
mkdir -p out
"$GCC" -mcpu=cortex-m85 -mthumb -mfloat-abi=hard -O1 -g -ffreestanding -nostdlib \
  -Wall -Wno-unused-variable -T link.ld startup.c probe.c -lgcc -o out/probe.elf
echo "out/probe.elf"
