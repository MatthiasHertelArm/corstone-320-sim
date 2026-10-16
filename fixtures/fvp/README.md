# Reference data from the Corstone-320 FVP

Recorded 2026-10-15 from `FVP_Corstone_SSE-320`, Fast Models 11.32+23 (Aug 9 2026), running in the
Docker image `cmsis-fvp-audit:11.32.23` (macOS host). The firmware is `firmware/probe/` (GCC 13.3.1,
`-mcpu=cortex-m85`), linked into the Secure boot ROM at 0x11000000 with data and stack in DTCM.

## Files

| File | Content |
|:--|:--|
| `params.txt` | `FVP_Corstone_SSE-320 --list-params` (default values and ranges of every parameter). |
| `ranges.txt` | The word ranges dumped into `regs-reset.txt` (`start end name`, hex, end exclusive). |
| `regs-reset.txt` | One `0xADDRESS 0xVALUE` line per word of every range, read through the model's GDBServer plugin while the core was halted at reset (`-D`), before the first instruction. 14331 words, no read refused. |
| `regs-reset-core.txt` | The `g` packet and `p0`..`p40` replies at the same halt. |
| `target.xml` | The register description the GDB server serves (`qXfer:features:read:target.xml`), 902 registers: `org.gnu.gdb.arm.m-profile`, `m-profile-mve` (vpr), `vfp` (d0-d15, fpscr), then the system registers. The simulator's GDB stub should serve the same. |
| `probe.txt` | Semihosting output of `firmware/probe`: core state at reset (MRS before anything is written), ID registers as the CPU reads them, CCSIDR per CSSELR, counter rates over calibrated loops, the UART0 test, a word dump of the PPB taken by the CPU (`# part 4`, same format as `regs-reset.txt`, taken after the probe's own setup) and reads at the memory edges. |
| `probe-stat.txt` | What the model prints after the probe exits when started with `--stat` (instruction count, simulated time). |

## Commands

Quiet model arguments used in every run (`<quiet>` below):

```
-C mps4_board.visualisation.disable-visualisation=1 -C vis_hdlcd.disable_visualisation=1
-C mps4_board.telnetterminalN.start_telnet=0 -C mps4_board.telnetterminalN.quiet=1   (N = 0, 1, 2, 5)
```

Docker wrapper (the image has the model on `PATH`, `~/.armlm` holds the cached licence; run from the
repo root and mount only the repo and `~/.armlm`: bind-mounting the whole home directory wedges
Docker Desktop's file sharing because it contains Docker's own disk image, and every container then
stays in `Created`):

```
docker run --rm -i --init -p 3333:3333 \
  --mount type=bind,src=$PWD,dst=$PWD --mount type=bind,src=$HOME/.armlm,dst=$HOME/.armlm --workdir $PWD \
  --env ARMLM_CACHED_LICENSES_LOCATION=$HOME/.armlm cmsis-fvp-audit:11.32.23 \
  stdbuf -oL -eL FVP_Corstone_SSE-320 <quiet> <args>
```

Register dump (`regs-reset.txt`, `regs-reset-core.txt`, `target.xml`):

```
FVP_Corstone_SSE-320 <quiet> -D --plugin /opt/avh-fvp/plugins/GDBServer.so \
  -C GDBServer.port=3333 -C GDBServer.allow_remote=1 \
  -C mps4_board.uart0.out_file=- -C mps4_board.subsystem.cpu0.semihosting-enable=1 \
  -a firmware/probe/out/probe.elf
python3 tools/fvp_dump.py --port 3333 --chunk 4 --ranges fixtures/fvp/ranges.txt --out fixtures/fvp/regs-reset.txt
python3 tools/fvp_dump.py --port 3333 --regs > fixtures/fvp/regs-reset-core.txt
```

Probe run (`probe.txt`, `probe-stat.txt`):

```
sh firmware/probe/build.sh      # ARM_GCC_BIN=<dir with arm-none-eabi-gcc>
FVP_Corstone_SSE-320 <quiet> --stat -C mps4_board.uart0.out_file=- \
  -C mps4_board.subsystem.cpu0.semihosting-enable=1 --simlimit 20 -a firmware/probe/out/probe.elf
```

## How the GDB server behaves (matters for `cs320-gdb`)

- Without `-D` the model opens the port and free-runs to completion; `-D` makes it wait halted at
  the reset vector. At that halt the `g` packet shows r0-r12 = 0, r13 = 0 (the MSP is loaded from the
  vector table only when the core starts: the probe's `reset.MSP` is 0x30007F00), r14 = 0xFFFFFFFF,
  r15 = 0x11000080 (the reset vector without the Thumb bit), xpsr = 0x01000000; `?` answers
  `T05;thread:p1.1;`, `qSupported` answers
  `PacketSize=8000;QStartNoAckMode+;xmlRegisters+;qXfer:features:read+;vContSupported+;qXfer:threads:read+`,
  `qXfer:memory-map:read` is not supported, `qRcmd help` lists only logging commands.
- The PPB (0xE000_0000-0xE00F_FFFF) is readable: CPUID, MVFRs, DWT/FPB/PMU, the ROM table all read
  through RSP, so no fallback to the probe was needed.
- **The server never replies `E..`.** Unmapped addresses (0x10008000, 0x40000000, 0xE0100000,
  0xF0000000, the CPU pwrctrl/secctrl blocks) read as 0x00000000. The CPU gets a BusFault at the
  same addresses (see `probe.txt`, `FAULT ipsr=0x00000005`).
- **`m` reads of more than one word return zeros for the PPB, SYSINFO/SYSCTRL and the other SSE-320
  peripheral blocks** (a 4-byte read of 0xE000ED00 gives 0x411FD230, an 8-byte read gives zeros);
  the SCC and the memories are fine with 256-byte reads. `fvp_dump.py --chunk 4` works around it.
- Touching the port with a probe connection (e.g. `nc -z`) counts as a client; closing it resumes the
  model, so wait for the `GDBServer: Listening ... port=3333` line instead of polling the port.

## Key values

Core (Cortex-M85 r1p0): CPUID 0x411FD230, ICTR 7 (256 IRQ lines in 8 ISER words, not 480),
ID_PFR0 0x20000030, ID_PFR1 0x00000230, ID_DFR0 0x10200000, ID_MMFR0 0x00111040, ID_MMFR2
0x01000000, ID_MMFR3 0x00000011, ID_ISAR0 0x01143110, ID_ISAR1 0x02212000, ID_ISAR2 0x20232232,
ID_ISAR3 0x01111131, ID_ISAR4 0x01310132, CLIDR 0x09200003, CTR 0x8303C003, CCSIDR 0xF01FE019
(CSSELR 0 = data: 32 KB, 4 ways, 32-byte lines) / 0xF03FE009 (CSSELR 1 = instruction: 32 KB, 2 ways);
only CSSELR bit 0 is implemented (readback masks the other bits). MVFR0 0x10110221, MVFR1 0x12100211,
MVFR2 0x00000040, MPU_TYPE 0x0800 (8 regions), SAU_TYPE 8, FPCCR 0xC0000004 at reset (S view;
the NS alias reads 0xC0000000), FPDSCR 0x00040000 (and FPSCR 0x00040000 at reset), AIRCR 0xFA050000,
CCR 0x00000201, DHCSR 0x03800000 at reset, DWT_CTRL 0x80000000 (NUMCOMP = 8, no trace features), FP_CTRL 0x10000080 (8 code comparators, FPB v2), PMU_TYPE 0x00A05F08 (8 event
counters), DEVARCH DWT 0x47711A02, FPB 0x47701A03, ITM 0x47701A01, PMU 0x47700A06, CoreSight
PIDR0-2 of the SCS/DWT/FPB/ITM 0x23/0xBD/0x0B, ITCMCR/DTCMCR 0x31 (enabled, 32 KB), PAHBCR 5.
The ROM table lists ITM/DWT/FPB/SCS/PMU/TPIU/ETM/CTI (0xE00FF000.. 0xFFF0F003, 0xFFF02003, 0xFFF03003,
0xFFF01003, 0xFFF41003, 0xFFF42003, 0xFFF04003) but the ETM and CTI blocks read all zeros (not modelled);
the TPIU has only its CoreSight IDs.

Reset state read by the probe: MSP 0x30007F00 (= vector[0]), PSP/MSPLIM/PSPLIM/CONTROL/PRIMASK/
BASEPRI/FAULTMASK 0, all NS banked registers 0, xPSR 0, LR 0xFFFFFFFF, r0-r12 0, VTOR 0x11000000,
SHCSR 0, CPACR 0 before the probe sets it.

Subsystem: SYSINFO SOC_IDENTITY 0x7E00043B, SYS_CONFIG0 0x00040014, SYS_CONFIG1 0x00000001;
SYSCTRL RESET_SYNDROME 1, INITSVTOR0 0x11000000, CPUWAIT 0, 0x58021120 = 0x0E; system counter CNTCR
0x101 (already enabled at reset, CNTSR 0x100), CNTFID0 100 000 000; systimer0/1/2 word 0x50 = 1,
systimer3 all zero; slow-clock timer VALUE 0xFFFFFFFF, CTRL 8; slow-clock watchdog 0xFFFFFFFF at
+0/+4; MPCs: CTRL 0x00800000, BLK_MAX/BLK_CFG as listed; KMU, LCM, SAM, PPUs, DMA-350 (PIDR
0xA0/0xB3/0x0B, channel 0 CH_BUILDCFG 0x3CDF0001/0x00080FFF) have their IDs; Ethos-U85, GPIO, VIO,
VSI, HDLCD, I2S, SPI, I2C, RTC, UARTs, FPGA IO, ISP, Ethernet, USB read all zeros at reset
(including their CoreSight/PrimeCell ID words: no PIDR for the CMSDK UARTs, RTC, HDLCD).
SCC (0x48200000): CFG_REG1 0x0089546B, CFG_REG2 0x01D982D6, 0x14 0x01040020, 0x18 0x00280020,
0x1C 0xFFFFFFFF, 0x24 4, 0x28 0x03A001D0, 0x64 5, 0x68 0x76F, 0x6C 8, 0x80 0x07D00000,
0x84 0x00500020, 0x8C 0xFFFF, 0x94 0xFFFF0000.

Memories: all RAM and DDR words read the fill pattern 0xDFDFDFCF / 0xCFDFDFDF, except ITCM and
DTCM (0x00000006 per word at the halt, 0x00000007/8 during the probe, so these are modelled
differently and not zero-initialised). The QSPI SRAM is 128 MB (0x3FFFFFF0 reads the pattern,
0x40000000 bus-faults), the BSP header says 8 MB. 0x80000000.. (DDR block 2) reads the pattern,
as do 0x90000000, 0xA0000000, 0xD0000000; the first word past ITCM, DTCM, ROM, code SRAM and
0x02000000 bus-fault on the CPU.

## Timing (probe.txt, part 2)

One instruction per 25 MHz core cycle; the loops are 2 000 000 instructions (subs/bne) and
1 002 000 instructions (1000 x (1000 nop + subs + bne)).

| Counter | Ticks per 1 000 000 instructions | Rate |
|:--|--:|:--|
| System counter CNTCV (0x58101000) and systimer0 CNTPCT (0x48000000) | 4 000 096 | 100 MHz, 4 per instruction |
| SysTick, CLKSOURCE=1 | 1 000 024 | 25 MHz, 1 per instruction |
| SysTick, CLKSOURCE=0 (reference clock) | 1 000 | 25 kHz (core / 1000) |
| DWT_CYCCNT | 125 003 | 1 per 8 instructions |
| Slow-clock CMSDK timer (0x4802F004) | 1 280 | 32 kHz exactly (not 32.768 kHz) |
| PMU CCNTR / EVCNTR (CPU_CYCLES, INST_RETIRED) | 0 | registers exist, counters never count; SW_INCR does increment EVCNTR0 |
| FPGA IO 1 Hz / 100 Hz / COUNTER / PSCNTR | 0 | never move, also with PRESCALE = 24 (the PRESCALE write reads back) |
| RTC PL031 RTCDR | 0 | reads 0 although `RTCDR_use_current_time=1`; RTCCR does not read back; no PIDs: not modelled |

Sampling overhead between two consecutive samples: 0 CNTCV ticks, 7 DWT_CYCCNT.
`--stat`: 97 332 669 instructions in 3.893200 s of simulated time = 25.000 MIPS, i.e. exactly one
instruction per 25 MHz cycle including the semihosting calls.

## Surprises and open points

- `ICTR` is 7 (8 x 32 = 256 interrupt lines), and the NVIC ITNS words 0xE000E380.. are present.
- CPU0 power control (0x40012000 / 0x50012000) and security control (0x50011000) BusFault on the
  CPU and read 0 over RSP: not in the model. CPU0 identity (0x4001F000) reads 0 but has PIDs
  (0x55/0xB8/0x0B).
- UART0 at 0x48203000: BAUDDIV and CTRL writes read back, DATA reads the last written byte,
  STATE never shows TX busy, INTSTATUS stays 0, no PIDs; **nothing ever appeared on stdout
  (`out_file=-`) or in a file (`out_file=<path>`, `unbuffered_output=1`)**, with 0.4 s of simulated
  time after the last byte. The telnet terminals were disabled (`start_telnet=0`); whether the FVP
  routes the UART only to the terminal is still open. The BSP's `UART0_BASE_S` is 0x59203000 (not
  0x58203000); both 0x59203000 and 0x58203000 read zeros and the 0x59203000 registers do not reflect
  the NS writes, so the S alias is not the same device in the model.
- The system counter read frame at 0x48101000 (NS) and 0x58101000 (S) both read 0 at the halt
  (counter at 0) and both count during the probe; the Secure frame was used for the measurements.
- NVIC ISPR0/ICPR0 (0xE000E200/0xE000E280) read 0x00004000 at the reset halt: IRQ 14 is pending
  before the first instruction. ITNS is all zero (every IRQ Secure), so the NS alias shows nothing
  pending.
- S/NS SCS aliases differ only where the architecture banks them: the pending bit above, VTOR
  (S 0x11000000, NS 0), SAU_TYPE (NS 0), FPCCR (NS 0xC0000000). Everything else reads the same
  through 0xE002Exxx.
- The DWT cycle counter advancing once per 8 instructions is the FVP's quantum, not a clock; the
  simulator should treat `DWT_CYCCNT` as cycles/8 if lockstep comparisons are wanted.
- Semihosting `SYS_WRITE0` output and the model's own `Info:` lines share stdout.
- Docker Desktop hung repeatedly during this session with containers stuck in `Created`; the cause
  was bind-mounting the whole home directory (it contains Docker's disk image). The final dump was
  taken with the narrow mounts shown above and is identical to the one taken before.
