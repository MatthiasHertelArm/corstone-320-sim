#!/bin/sh
# Builds the SSE_320_BSP examples (Blinky, Vio) with the CMSIS-Toolbox and Arm GNU Toolchain.
# Needs cbuild on PATH (or CMSIS_TOOLBOX_BIN), cmake, ninja, and GCC_TOOLCHAIN_13_3_1 set to the
# bin directory of arm-none-eabi-gcc 13.3.1.
set -e
cd "$(dirname "$0")"
export CMSIS_PACK_ROOT="${CMSIS_PACK_ROOT:-$HOME/.cache/arm/packs}"
[ -n "$CMSIS_TOOLBOX_BIN" ] && export PATH="$CMSIS_TOOLBOX_BIN:$PATH"
for ex in Blinky Vio; do
  (cd "$ex" && cbuild "$ex.csolution.yml" --toolchain GCC --context "$ex.Debug+SSE-320-FVP" --packs --update-rte)
done
ls -l Blinky/build/SSE-320-FVP/GCC/Debug/Blinky/outdir/Blinky.elf Vio/build/SSE-320-FVP/GCC/Debug/Vio/outdir/Vio.elf
