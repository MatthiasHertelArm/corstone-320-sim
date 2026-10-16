#!/bin/sh
# Runs an ISA test ELF on Arm's FVP_Corstone_SSE-320 in Docker (image cmsis-fvp-audit:11.32.23)
# and writes its semihosting output to a file.
#
#   ./record_fvp.sh                       -> re-records ../../fixtures/isa_test.fvp.txt from
#                                            ../../fixtures/elf/isa_test.elf
#   ./record_fvp.sh <elf> <out.txt>       -> raw FVP stdout of another image (e.g. a --verbose build)
#
# Run from any directory; one FVP container at a time (container name isa-fvp).
set -e
ELF="${1:-}"
OUT="${2:-}"
case "$ELF" in "") ;; /*) ;; *) ELF="$PWD/$ELF" ;; esac
case "$OUT" in "") ;; /*) ;; *) OUT="$PWD/$OUT" ;; esac
cd "$(dirname "$0")/../.."
IMAGE="${FVP_IMAGE:-cmsis-fvp-audit:11.32.23}"
[ -n "$ELF" ] || ELF=fixtures/elf/isa_test.elf
docker rm -f isa-fvp >/dev/null 2>&1 || true
# Only the repository and the licence cache are mounted: binding the whole home directory
# (which contains Docker's own disk image) hangs Docker Desktop's file sharing on macOS.
MOUNTS="--mount type=bind,src=$PWD,dst=$PWD --mount type=bind,src=$HOME/.armlm,dst=$HOME/.armlm"
run() {
  docker run --rm -i --init $MOUNTS --workdir "$PWD" \
    --env ARMLM_CACHED_LICENSES_LOCATION="$HOME"/.armlm "$IMAGE" "$@"
}
VERSION=$(run FVP_Corstone_SSE-320 --version 2>/dev/null | head -1)
RAW=$(mktemp)
docker run --rm -i --init --name isa-fvp $MOUNTS --workdir "$PWD" \
  --env ARMLM_CACHED_LICENSES_LOCATION="$HOME"/.armlm "$IMAGE" \
  stdbuf -oL -eL FVP_Corstone_SSE-320 \
  -C mps4_board.visualisation.disable-visualisation=1 -C vis_hdlcd.disable_visualisation=1 \
  -C mps4_board.telnetterminal0.start_telnet=0 -C mps4_board.telnetterminal0.quiet=1 \
  -C mps4_board.telnetterminal1.start_telnet=0 -C mps4_board.telnetterminal1.quiet=1 \
  -C mps4_board.telnetterminal2.start_telnet=0 -C mps4_board.telnetterminal2.quiet=1 \
  -C mps4_board.telnetterminal5.start_telnet=0 -C mps4_board.telnetterminal5.quiet=1 \
  -C mps4_board.subsystem.cpu0.semihosting-enable=1 --simlimit 60 -a "$ELF" > "$RAW"
if [ -n "$OUT" ]; then
  mv "$RAW" "$OUT"
  echo "$OUT ($(grep -c ': 0x' "$OUT") digest lines)"
else
  if ! grep -q "isa-test end" "$RAW"; then
    echo "the FVP did not run the firmware to its end:" >&2
    tail -5 "$RAW" >&2
    exit 1
  fi
  {
    echo "# Output of firmware/isa-test on Arm FVP_Corstone_SSE-320, $VERSION (reference for the core)"
    echo "# Lines up to and including the marker 'mve' need no MVE; the groups after it are the MVE section."
    grep ': 0x' "$RAW"
  } > fixtures/isa_test.fvp.txt
  rm -f "$RAW"
  echo "fixtures/isa_test.fvp.txt ($(grep -c ': 0x' fixtures/isa_test.fvp.txt) digest lines)"
fi
