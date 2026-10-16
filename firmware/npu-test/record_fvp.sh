#!/bin/sh
# Runs the NPU test ELF on Arm's FVP_Corstone_SSE-320 in Docker (image cmsis-fvp-audit:11.32.23)
# with the default Ethos-U85 configuration (1024 MACs) and writes its semihosting output to a file.
#
#   ./record_fvp.sh                       -> re-records ../../fixtures/npu_test.fvp.txt from
#                                            ../../fixtures/elf/npu_test.elf
#   ./record_fvp.sh <elf> <out.txt>       -> raw FVP stdout of another image
#
# Run from any directory; one FVP container at a time (container name npu-fvp). The FVP's NPU
# model is slow: a run takes several minutes. FVP_EXTRA adds model parameters, e.g.
# FVP_EXTRA="-C mps4_board.subsystem.ethosu.diagnostics=4" for the NPU's own messages.
set -e
ELF="${1:-}"
OUT="${2:-}"
case "$ELF" in "") ;; /*) ;; *) ELF="$PWD/$ELF" ;; esac
case "$OUT" in "") ;; /*) ;; *) OUT="$PWD/$OUT" ;; esac
cd "$(dirname "$0")/../.."
IMAGE="${FVP_IMAGE:-cmsis-fvp-audit:11.32.23}"
[ -n "$ELF" ] || ELF=fixtures/elf/npu_test.elf
docker rm -f npu-fvp >/dev/null 2>&1 || true
# Only the repository and the licence cache are mounted: binding the whole home directory
# (which contains Docker's own disk image) hangs Docker Desktop's file sharing on macOS.
MOUNTS="--mount type=bind,src=$PWD,dst=$PWD --mount type=bind,src=$HOME/.armlm,dst=$HOME/.armlm"
run() {
  docker run --rm -i --init $MOUNTS --workdir "$PWD" \
    --env ARMLM_CACHED_LICENSES_LOCATION="$HOME"/.armlm "$IMAGE" "$@"
}
VERSION=$(run FVP_Corstone_SSE-320 --version 2>&1 | grep -m1 "Fast Models" | tr -d "[]")
RAW=$(mktemp)
docker run --rm -i --init --name npu-fvp $MOUNTS --workdir "$PWD" \
  --env ARMLM_CACHED_LICENSES_LOCATION="$HOME"/.armlm "$IMAGE" \
  stdbuf -oL -eL FVP_Corstone_SSE-320 \
  -C mps4_board.visualisation.disable-visualisation=1 -C vis_hdlcd.disable_visualisation=1 \
  -C mps4_board.telnetterminal0.start_telnet=0 -C mps4_board.telnetterminal0.quiet=1 \
  -C mps4_board.telnetterminal1.start_telnet=0 -C mps4_board.telnetterminal1.quiet=1 \
  -C mps4_board.telnetterminal2.start_telnet=0 -C mps4_board.telnetterminal2.quiet=1 \
  -C mps4_board.telnetterminal5.start_telnet=0 -C mps4_board.telnetterminal5.quiet=1 \
  -C mps4_board.subsystem.cpu0.semihosting-enable=1 \
  -C mps4_board.subsystem.ethosu.num_macs=1024 \
  $FVP_EXTRA --simlimit "${FVP_SIMLIMIT:-600}" -a "$ELF" > "$RAW" 2>&1
if [ -n "$OUT" ]; then
  mv "$RAW" "$OUT"
  echo "$OUT ($(grep -c ': 0x' "$OUT") digest lines)"
else
  if ! grep -q "npu-test end" "$RAW"; then
    echo "the FVP did not run the firmware to its end:" >&2
    tail -5 "$RAW" >&2
    exit 1
  fi
  {
    echo "# Output of firmware/npu-test on Arm FVP_Corstone_SSE-320, $VERSION, ethosu.num_macs=1024 (reference for the NPU)"
    echo "# <net>.cycles_lo/hi are the FVP's PMU cycle counts: timing, not to be compared with the simulator."
    grep ': 0x\|^npu-test' "$RAW"
  } > fixtures/npu_test.fvp.txt
  rm -f "$RAW"
  echo "fixtures/npu_test.fvp.txt ($(grep -c ': 0x' fixtures/npu_test.fvp.txt) digest lines)"
fi
