#!/bin/sh
# Builds the Ethos-U85 NPU test (Arm GNU Toolchain 13 on PATH or ARM_GCC_BIN) against Arm's
# ethos-u-core-driver (ETHOSU_DRIVER, default: the CMSIS pack ARM.ethos-u-core-driver 1.26.2 in
# ~/.cache/arm/packs) and CMSIS-Core (CMSIS_CORE, default: ARM.CMSIS 6.3.0 from the same cache).
# The ELF goes to out/ and to fixtures/elf/npu_test.elf.
#
#   ./build.sh                  -> out/npu_test.elf, copied to ../../fixtures/elf/npu_test.elf
#   ETHOSU_LOG_SEVERITY=3 ./build.sh   (driver log level: 0 err, 1 warn (default), 2 info, 3 debug)
set -e
cd "$(dirname "$0")"
GCC="${ARM_GCC_BIN:+$ARM_GCC_BIN/}arm-none-eabi-gcc"
PACKS="${CMSIS_PACK_ROOT:-$HOME/.cache/arm/packs}"
DRIVER="${ETHOSU_DRIVER:-$PACKS/ARM/ethos-u-core-driver/1.26.2/ethos_u_core_driver}"
CMSIS="${CMSIS_CORE:-$PACKS/ARM/CMSIS/6.3.0/CMSIS/Core/Include}"
COMMON="-mcpu=cortex-m85 -mthumb -mfloat-abi=hard -O2 -g -ffunction-sections -fdata-sections -Wall"
# ETHOSU_SEMAPHORE_WAIT_INFERENCE: polling iterations before an inference is declared timed out.
DRVDEFS="-DETHOSU_ARCH=u85 -DETHOSU_MACS=256 -DETHOSU85 -DETHOSU -DNDEBUG \
  -DETHOSU_LOG_SEVERITY=${ETHOSU_LOG_SEVERITY:-1} -DETHOSU_LOG_ENABLE=1 \
  -DETHOSU_SEMAPHORE_WAIT_INFERENCE=100000000ULL"
INC="-I. -I$DRIVER/include -I$DRIVER/src -I$CMSIS"
mkdir -p out
for f in ethosu_driver ethosu_device_u85 ethosu_pmu; do
  "$GCC" $COMMON $DRVDEFS $INC -include log_shim.h -c "$DRIVER/src/$f.c" -o "out/$f.o"
done
"$GCC" $COMMON -c startup.c -o out/startup.o
"$GCC" $COMMON $DRVDEFS $INC -c npu_test.c -o out/npu_test.o
"$GCC" $COMMON -T link.ld --specs=nano.specs --specs=nosys.specs -nostartfiles -Wl,--gc-sections \
  out/startup.o out/npu_test.o out/ethosu_driver.o out/ethosu_device_u85.o out/ethosu_pmu.o \
  -Wl,-Map=out/npu_test.map -o out/npu_test.elf
cp out/npu_test.elf ../../fixtures/elf/npu_test.elf
echo "out/npu_test.elf (copied to fixtures/elf/npu_test.elf)"
