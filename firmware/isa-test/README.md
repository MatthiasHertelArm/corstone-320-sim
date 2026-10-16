# ISA differential test for the Cortex-M85 core

A bare-metal program that executes every instruction group of Armv8.1-M Mainline that the
simulator claims to support on fixed edge-case and pseudo-random operands and prints one digest
per group over semihosting (`name: 0x........`). Its output on Arm's `FVP_Corstone_SSE-320` is
recorded once in `fixtures/isa_test.fvp.txt`; `cargo test -p cs320-board --test reference` runs
the same ELF (`fixtures/elf/isa_test.elf`) in the simulator and compares line by line. Any
difference is a simulator bug (see microbit-v2-sim, decision 0006, for the method).

## Layout

| File | Content |
|:--|:--|
| `isa_test.c` | 320 scalar groups: integer ALU, shifts, multiply, DSP SIMD and saturation, CSEL family, FPv5 single/double/half precision in all FPSCR rounding modes (FZ, DN, AHP, FZ16 too), memory accesses, LDA/STL and exclusives, branches, low-overhead loops, CLRM/VSCCLRM, system registers, PACG/AUTG, TT, exceptions (SVC, PendSV with lazy FP stacking, SysTick, NVIC IRQ 16 and 224, UsageFault, STKOF, MemManage through the MPU, BusFault, VLSTM/VLLDM). Built without MVE. |
| `mve_test.c` | the `mve` section, after the marker line `mve: 0x00000001`: 107 Helium groups (loads/stores incl. gather/scatter and interleaving, integer and float arithmetic, reductions, shifts, predication with VCMP/VPT/VCTP, saturating multiplies, conversions, tail-predicated loops). Built with `-mcpu=cortex-m85`. |
| `startup.c`, `link.ld` | vector table (16 + 480 entries), reset, FPU enable; code in the Secure boot ROM at 0x11000000, data and stack in the DTCM at 0x30000000. |
| `build.sh` | builds `out/isa_test.elf` with GCC 13 and copies it to `fixtures/elf/`. `./build.sh --verbose "vadd,tt"` builds `out/isa_test_verbose.elf`, which also prints every value mixed into the digests of the listed groups. |
| `record_fvp.sh` | runs an ELF on the FVP in Docker (`cmsis-fvp-audit:11.32.23`) and re-records `fixtures/isa_test.fvp.txt`, or with `<elf> <out.txt>` saves the raw output of another image. |

## Workflow

```sh
export ARM_GCC_BIN=.../compilers.arm.arm.none.eabi.gcc/13.3.1/bin
firmware/isa-test/build.sh                 # -> fixtures/elf/isa_test.elf
firmware/isa-test/record_fvp.sh            # -> fixtures/isa_test.fvp.txt (needs Docker and the FVP image)
cargo test -p cs320-board --test reference
# while developing:
cargo build -p cs320sim --no-default-features
target/debug/cs320sim fixtures/elf/isa_test.elf --headless --speed max --semihosting --time 60
```

To find the instruction behind a mismatching digest, build a verbose image for that group, run it
on both the FVP (`record_fvp.sh out/isa_test_verbose.elf fvp.txt`) and the simulator, and diff the
`  0x........` lines that follow the group: the index of the first differing value points to the
iteration (operand pair and FPSCR mode) and the instruction.

`fixtures/isa_test.known_mismatches.txt` lists the groups that are allowed to differ for now;
the test prints them and reports listed groups that match again. `ISA_TEST_KNOWN_MISMATCHES=a,b`
adds names for one run.

## Notes

- The reference is recorded with the FVP's default parameters. There `CFGPACBTI=0`, so PACG,
  AUTG and the PAC key registers are not implemented on the FVP: the `pacg`/`pacg2` groups record
  the UsageFault that the FVP takes (the handler skips the instruction), not a PAC value.
- All digests are deterministic: no dependence on timing (SysTick is waited for), no uninitialised
  memory, pseudo-random operands from a fixed-seed xorshift per group.
- The FVP needs about 90 s for a run; the simulator about 30 s in a debug build.
