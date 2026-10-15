# Plan: `corstone-320-sim` — a Corstone-320 simulator with a display, a camera and a microphone

> **Status (2026-10-15):** first implementation. See the "Status" section at the end for what is done.

## Context

Arm's Corstone-320 Ecosystem FVP (`FVP_Corstone_SSE-320`, Fast Models 11.32) models the SSE-320
subsystem: a Cortex-M85 (Armv8.1-M Mainline with Helium, double-precision FPU, TrustZone, PACBTI),
an Ethos-U85 NPU, a DMA-350, and the MPS4 board peripherals of the `ARM::SSE_320_BSP` pack (CMSDK
UARTs, system timers, watchdogs, GPIO, I2C, SPI, I2S, HDLCD display, Mali-C55 ISP with a virtual
camera, the VIO/VSI virtual interfaces of Arm Virtual Hardware). The FVP is a Linux binary (run in
Docker on macOS), its visualisation is a small fixed window, its camera needs the Mali-C55 firmware
stack and a frame file in the ISP model's own format, and its microphone exists only as a Python
script behind the VSI.

`corstone-320-sim` is a native simulator (Rust, Windows/macOS/Linux) of the same subsystem with the
same peripherals at the same addresses, built on the lessons of
[`microbit-v2-sim`](../microbit-v2-sim/PLAN.md), with three deliberate additions:

- a **display** of any resolution the firmware programs into the HDLCD (up to 4096 × 4096, 60 Hz),
  shown in a resizable window;
- a **camera**: a host webcam, image files, folders of frames or a test pattern, delivered through
  the AVH video interface (VSI4) *and* through a simple capture peripheral with a documented
  register map, so no ISP firmware is needed;
- a **microphone**: the host microphone, WAV files or a tone, delivered through the MPS3 I2S
  peripheral *and* the AVH audio interface (VSI0).

Everything that the BSP pack's examples (Blinky, Vio, ISP_minimal) and the CMSIS Debugger need works
unmodified: same ELF images, same FVP command line and parameter files (`-f fvp_config.txt`), same
GDB-server contract.

### What "the same subsystem" means (the fidelity contract)

| Level | Contract | Verified against |
|:--|:--|:--|
| Core | Architecturally exact Armv8.1-M Mainline state after every instruction: flags, FPSCR, Q registers and VPR, exception entry/return with the Security Extension, stack limits, faults. | `FVP_Corstone_SSE-320` in lockstep on an ISA test firmware (recorded once, see decision 0006 of microbit-v2-sim), the Cortex-M85 TRM. |
| Subsystem | Memory map and register maps of the SSE-320 TRM and the BSP headers (`platform_base_address.h`, `platform_regs.h`, `*_reg_map.h`, `SSE320.svd`). Reset values and ID registers as the FVP reports them. | Register dumps taken from the FVP through its GDB server (`fixtures/fvp/`). |
| Board | MPS4 FPGA I/O (10 LEDs, 2 buttons, 8 switches), UARTs, HDLCD timing, I2S clocking, VIO/VSI protocols of Arm Virtual Hardware. | The BSP drivers and the AVH `audio_drv.c`/`video_drv.c` (pack `ARM::SDS`). |
| Timing | The FVP is instruction-accurate, not cycle-accurate: one instruction per CPU clock (25 MHz), peripherals exact in their own clocks. The simulator does the same, so `--stat` cycle counts are comparable. | Counter rates measured on the FVP. |

Non-goals for 1.0:

- **Ethos-U85 inference**: the NPU registers exist (ID, reset, command queue, status, IRQ), so the
  driver initialises, but a command stream is not executed; the model raises the NPU's error IRQ
  and logs a warning once. Vela-compiled models therefore fail cleanly instead of silently.
- **Mali-C55 ISP**: the ISP register block reads as zero with a warning; the camera comes through
  VSI4 and the capture peripheral instead.
- **DMA-350**: registers only, with a warning when a channel is enabled (planned for 1.1).
- Ethernet, USB, QSPI flash controller, CryptoCell, KMU/LCM/SAM/PPU logic, trace (ETM/TPIU
  output), the VSocket. Their ID registers read correctly; everything else reads as zero and is
  reported once.
- MVE (Helium) and PACBTI execution are phased (see the core section): the decoder knows every
  Armv8.1-M encoding, but a subset of MVE executes in 1.0.

## Sources of truth

| Topic | Source |
|:--|:--|
| Memory map, IRQ map, register maps | `ARM::SSE_320_BSP` 1.1.0: `Board/Platform/platform_base_address.h`, `platform_irq.h`, `platform_regs.h`, `Device/Include/*_reg_map.h`, `Device/Source/*_drv.c`, `SVD/SSE320.svd` |
| FVP parameters and behaviour | `FVP_Corstone_SSE-320 --list-params` (11.32.23, recorded in `fixtures/fvp/params.txt`), register dumps in `fixtures/fvp/` |
| Core | Armv8-M Architecture Reference Manual (DDI 0553B), Cortex-M85 TRM (101924), Armv8.1-M MVE pseudocode |
| AVH virtual interfaces | `ARM::SDS` pack: `arm_vsi.h`, `arm_vio.h`, `audio_drv.c`, `video_drv.c` |
| CMSIS-Debugger contract | as in microbit-v2-sim (CMSIS Solution 1.72, CMSIS Debugger 1.8, GDB 15.2); the Arm-FVP adapter template |
| Prior art | microbit-v2-sim (this project's template: time base, scheduler, RSP server, GUI, scenarios) |

## Requirements and acceptance

| ID | Requirement | Accepted when |
|:--|:--|:--|
| R1 | Runs the unmodified BSP examples | `Blinky`, `Vio` print their banners on UART0 and toggle the FPGA LEDs at 1 Hz / the VIO LEDs; `ISP_minimal` runs up to the ISP initialisation. The MLEK `kws` image boots to the NPU driver init. |
| R2 | FVP-compatible command line | `cs320sim -f fvp_config.txt -a app.axf --simlimit 60 --stat` works with the MLEK and BSP config files; `-C GDBServer.port=N --plugin x` starts the GDB server; `mps4_board.*` parameters are honoured or reported. |
| R3 | CMSIS Debugger compatible | Load & Debug, attach, breakpoints (8 hardware), watchpoints (4), step, reset, Live Watch, Peripheral Inspector (SSE320.svd), CPU Time, Serial Monitor on UART0. |
| R4 | Display, camera, microphone | HDLCD frames appear in the window at the programmed resolution; VSI4 and the capture peripheral deliver webcam/file frames; I2S and VSI0 deliver microphone/WAV samples at the configured rate. |
| R5 | Headless and deterministic | `--headless` with scenario files (buttons, switches, UART, display checksum, camera/microphone sources). Exit codes for CI. |
| R6 | Speed | ≥ 100 MIPS on Apple M-series for integer code; the display at 60 Hz costs less than 10 % of a core. |

## Architecture

```
┌─ cs320sim process ─────────────────────────────────────────────────────────────┐
│  GUI thread (eframe/egui)              sim thread (owns the Machine)            │
│  ┌──────────────────────────┐  inputs  ┌───────────────────────────────────┐    │
│  │ HDLCD panel, camera view │────────► │ Cortex-M85        cs320-cpu       │    │
│  │ mic meter, LEDs/buttons  │          │ SSE-320 + MPS4    cs320-soc       │    │
│  │ switches, VIO, 6 UARTs   │◄──────── │ board sources     cs320-board     │    │
│  └──────────────────────────┘ frames,  │ scheduler         12.8 GHz ticks  │    │
│  audio thread (cpal) ◄─────── samples, └────────┬───────────────┬──────────┘    │
│  camera thread (nokhwa/files) ──────► frame queue│ requests      │ bytes         │
│                          ┌──────────────────────┴──────┐ ┌──────┴─────────┐     │
│                          │ debug service: run control, │ │ UART bridges   │     │
│                          │ memory and register access  │ │ (6 × TCP/GUI)  │     │
│                          └──────┬───────────────┬──────┘ └──────┬─────────┘     │
│                           RSP :3333        RSP :3333       TCP :5000..5005      │
└────────────────────────────────┼───────────────┼────────────────┼───────────────┘
```

- **Time**: one tick is 1/12.8 GHz, the least common multiple of the 25 MHz system clock, the
  32 MHz and 100 MHz counter clocks and the 32.768 kHz slow clock (2^15 · 5^8). A CPU cycle is
  512 ticks; `u64` lasts 45 years.
- **Core timing**: one instruction per cycle plus the FVP's exception entry cost, as a Fast Model.
- **Memory**: ITCM, DTCM, SRAM, ISRAM0/1, boot ROM and QSPI SRAM are `Vec<u8>`; the 2 GB DDR4 is
  allocated in 1 MB chunks on first touch. Secure and Non-secure aliases (address bit 28) reach the
  same storage; the IDAU attributes come from the SSE-320 map; the MPCs and PPCs are modelled as
  registers with their interrupts but do not block accesses in 1.0 (a warning is logged when an
  access would be blocked on the FVP).
- **Determinism**: inputs are stamped in simulated time; host time enters only through pacing and
  the live camera/microphone, which are off in scenarios.

## Workspace layout

```
corstone-320-sim/
  crates/
    cs320-core/      time base, scheduler
    cs320-cpu/       Armv8.1-M Mainline core: decoder, interpreter, FPU (SP, DP, HP), MVE, security
                     extension, SCS (NVIC 480 IRQs, SAU, MPU, SysTick×2, PMU, DWT, FPB, ITM), debug
    cs320-soc/       SSE-320 memory map and peripherals, MPS4 FPGA peripherals, VIO/VSI, NPU stub
    cs320-board/     frame and sample sources (camera, microphone, speaker), the Machine, the sim thread
    cs320-loader/    ELF/AXF/HEX/BIN, cbuild-run.yml, FVP parameter files
    cs320-gdb/       RSP server (CMSIS Debugger contract), target.xml with the v8-M registers
    cs320-gui/       eframe window: display, camera, audio, board strip, UART terminals
    cs320sim/        the binary: FVP-compatible CLI, scenarios, control port, adapter registration
  fixtures/fvp/      register dumps and measurements taken from the FVP
  firmware/          test firmware (GCC 13, -mcpu=cortex-m85): ISA test, peripheral tests
  tests/scenarios/   scenario files for the BSP examples
  cmsis/             debug adapter template, debug-adapters.yml entry, launch.json example
  docs/decisions/    decisions that differ from this plan
```

## Core (`cs320-cpu`): Cortex-M85 r1p0

Inherited from microbit-v2-sim: the table-driven decoder with a decode cache, the interpreter,
the exact single-precision FPU, the exception machinery, DWT/FPB/ITM, the GDB-visible debug state.
New for Armv8.1-M Mainline:

- **Security Extension**: Secure and Non-secure states; banked MSP/PSP/MSPLIM/PSPLIM, PRIMASK,
  BASEPRI, FAULTMASK, CONTROL (SFPA), VTOR, SysTick, MPU, SCB registers (banked view at
  0xE002_E000); SAU with 8 regions; IDAU from the SSE-320 map (bit 28 = NS, except exempt PPB and
  the NSC region of the SRAM); `SG`, `BXNS`, `BLXNS`, `TT*`; exception entry with the additional
  state context and the integrity signature; `VLSTM`/`VLLDM`; SecureFault (SFSR/SFAR);
  AIRCR.PRIS/BFHFNMINS; NVIC ITNS. The core resets in Secure state at `INITSVTOR`.
- **Stack limits** (`MSPLIM`/`PSPLIM`, UsageFault STKOF), **CLRM**, **VSCCLRM**.
- **Low-overhead loops and branch future**: `WLS`/`DLS`/`LE`, `WLSTP`/`DLSTP`/`LETP`/`LCTP`, `BF*`
  (executed as the architecture allows when the branch cache misses).
- **Conditional select**: `CSEL`, `CSINC`, `CSINV`, `CSNEG` and aliases.
- **PACBTI**: `PAC`, `AUT`, `BTI` as NOPs when disabled (the reset state), `PACG`/`AUTG` computed.
- **FPU**: FPv5 with double precision (D0–D15) and half-precision scalar arithmetic; `VSEL`,
  `VMAXNM`/`VMINNM`, `VRINT*`, `VCVT{A,N,P,M}`, `VINS`/`VMOVX`, F32↔F64↔F16 conversions; lazy
  stacking with S16–S31 (TS) for Secure code.
- **MVE (Helium)**: Q0–Q7, VPR, beat-wise predication (`VPT`/`VPST`, `VCTP`, tail predication).
  Phase 1 (1.0): the integer and FP instructions that CMSIS-DSP and CMSIS-NN use for the Cortex-M85
  (loads/stores incl. widening/narrowing and scatter/gather, `VADD`/`VSUB`/`VMUL`/`VMLA`/`VMLADAV*`/
  `VMLALDAV*`/`VRMLALDAVH*`, `VQDMLAH`, shifts, `VMOV`, `VDUP`, `VCMP`/`VPSEL`, `VMAX`/`VMIN`,
  `VABS`/`VNEG`, `VCVT`, `VFMA`, `VADDV`/`VMAXV`). Phase 2: the rest (complex, bit-wise permutations,
  saturating rounding variants). An unimplemented MVE instruction halts the simulation with the
  encoding, never executes wrongly.
- **System Control Space**: CPUID `0x411FD230`, ID_PFR/ID_DFR/ID_AFR/ID_MMFR/ID_ISAR, CLIDR/CTR/
  CCSIDR (32 KB I and D caches, whose enable bits are accepted), MVFR0–2, ICTR (480 IRQs, 16 words),
  3 priority bits, SAU, PMSAv8 MPU (8 regions, MAIR), FPCCR banking, CPPWR, PMU (`PMU_CTRL`,
  cycle counter, 8 event counters counting cycles/instructions), DCB with DSCSR/DAUTHCTRL, DWT with
  4 comparators (v8-M MATCH encoding), FPBv2 with 8 comparators, ITM, TPIU, ROM table, CPU power
  control (`CPDLPSTATE` at 0xE001_E300), TCM/TGU registers at 0xE001_E000.

Debug: as in microbit-v2-sim (BKPT halts with C_DEBUGEN, FPB before, DWT after the access,
C_MASKINTS). DCRSR selectors follow Armv8-M (`MSP_NS`, `PSP_S`, `*LIM`, `CONTROL_S/NS`, VPR).

## Subsystem and board (`cs320-soc`)

| Block | Base (NS / S) | IRQ | Model |
|:--|:--|:--|:--|
| ITCM 32 KB, DTCM 32 KB, SRAM 2 MB, ISRAM0/1 2 MB, boot ROM 128 KB, QSPI SRAM 8 MB, DDR4 2 GB | see `platform_base_address.h` | — | RAM; the ROM is writable by the loader and the debugger only |
| System control, system info, SACFG/NSACFG, MPC ×5, PPC, CPU identity/power/security control, PPUs, KMU, LCM, SAM, CC3XX | 0x5802_1000 … | 9, 10, 11, 14, 20, 30, 31 | Registers with IDs and reset values; `SWRESET`, `RESET_SYNDROME`, `INITSVTOR0`, `GRETREG`, `CPUWAIT` functional; MPC/PPC interrupts from violation registers written by software |
| System counter (control + read frames), system timers ×4 | 0x5810_0000, 0x4810_1000, 0x4800_x000 | 3, 4, 5, 27 | 64-bit counter with scaling registers; CVAL/TVAL compare, auto-increment, IMASK/ISTATUS; exact in the counter clock |
| System watchdogs (S, NS), slow-clock CMSDK timer and watchdog | 0x4804_0000, 0x5804_0000, 0x4802_F000, 0x5802_E000 | 0, 1, 2 | Watchdog offset/compare with interrupt then reset request; CMSDK timer with reload and interrupt |
| CMSDK UART0–5 | 0x4820_3000 + n·0x1000 | 33–42, 125/126 (RX/TX), 43–47/127 (combined), 48 (overflow) | 1-byte TX/RX buffers, BAUDDIV frame timing from the 25 MHz clock, TX/RX interrupts, overflow bits; host side: stdout, TCP, GUI terminal, file (`out_file`) |
| FPGA I/O | 0x4820_2000 | — | LEDs[9:0], buttons[1:0], switches[7:0], 1 Hz and 100 Hz counters, cycle counter, prescaler, MISC (SPI nCS) |
| SCC | 0x4820_0000 | — | CFG_REG0–7, DLL, AID/ID as the FVP reports them |
| GPIO0–3 (CMSDK) | 0x4010_0000 + n·0x1000 | 69–72 | 16-bit ports with ALTFUNC, interrupt type/polarity/mask; GPIO0 bits mirror the LEDs/buttons in the GUI |
| I2C SBCon ×5 (touch, audio, shield0/1, DDR4 EEPROM) | 0x4810_0000, 0x4810_5000, 0x4810_6000, 0x4810_8000, 0x4810_9000 | — | Bit-banged SCL/SDA with a bus decoder; devices: audio codec register file (CS42L52-like at 0x4A), 24C EEPROM (0x50); others NACK |
| SPI PL022 ×3 | 0x4810_2000 … | — | Register stub: TX FIFO always empty, loopback when LBM |
| Audio I2S | 0x4820_1000 | 50 | TX/RX FIFOs (16 × 32 bit, L/R 16-bit), rate = 12.288 MHz / LRDIV, water-level interrupts; RX from the microphone source, TX to the speaker |
| HDLCD (PL370) | 0x4031_0000 / 0x5031_0000 | 133 | Frame buffer fetch per FB_*/pixel-format/colour-select registers at the end of each frame period (timing registers, or `force_frame_rate` 50 Hz as the FVP); DMA_END/VSYNC/BUS_ERROR interrupts; output frames to the GUI |
| RTC (PL031) | 0x4820_B000 | 128 | 1 Hz counter from the host clock at reset (`RTCDR_use_current_time`), match interrupt |
| VIO | 0x4FEF_0000 | — | SignalOut (LEDs 0–7), SignalIn (buttons 0–3), Value[64] (GUI sliders; scenario inputs) |
| VSI0–7 | 0x4FF0_0000 + n·0x1_0000 | 224–231 | Timer (1 MHz), DMA (block ring), IRQ regs; VSI0 audio in (microphone), VSI1 audio out (speaker), VSI4 video in (camera), VSI6 video out (second display), others: timer and DMA only |
| Capture peripheral (`CAM`) | 0x4840_0000 (the FVP's virtual-camera block, otherwise unused) | 132 | `CTRL`, `STATUS`, `WIDTH`, `HEIGHT`, `FORMAT` (RGB565/RGB888/GRAY8/NV12), `FRAME_RATE`, `BUFFER`, `STRIDE`, `IRQ_*`, `FRAME_COUNT`; writes one frame per period into the buffer and raises the IRQ |
| Ethos-U85 | 0x4000_4000 / 0x5000_4000 | 16 | ID/CONFIG registers of a U85-256, reset, PMU registers, command queue registers; `CMD.transition_to_running` raises the error IRQ with a warning (no inference) |
| DMA-350 | 0x4000_2000 / 0x5000_2000 | 24, 25, 26, 57, 58 | ID registers and channel registers stored; a channel enable logs a warning |
| ISP (Mali-C55) | 0x4830_0000 | 132 | Reads as zero, reported once |

Peripheral conventions:

- Secure and Non-secure aliases of a peripheral reach the same model; the PPC permission is not
  enforced in 1.0.
- Debugger accesses have no side effects (a UART DATA read by the debugger does not pop the FIFO).
- Unmodelled registers are stored and read back; accesses to unmodelled blocks are reported once.

## GDB server, CLI and GUI

- **GDB**: as microbit-v2-sim, with `org.gnu.gdb.arm.m-profile`, `m-system`, `vfp` (d0–d15),
  `secext` (msp_ns, psp_ns, msp_s, psp_s, msplim, psplim banked) and `m-profile-mve` (vpr) features; the
  memory map lists the RAMs (no flash: `load` writes memory directly, as on the FVP).
- **CLI**: the FVP form (`-a`, `-C`, `-f`, `--simlimit`, `--stat`, `--plugin`, `-D`, `--list-params`,
  `--version`) plus the native options of microbit-v2-sim. Parameters are the FVP's names:
  `mps4_board.uart0.out_file`, `mps4_board.uart0.shutdown_on_eot`, `mps4_board.uart0.unbuffered_output`,
  `mps4_board.visualisation.disable-visualisation`, `vis_hdlcd.disable_visualisation`,
  `mps4_board.subsystem.cpu0.semihosting-enable`, `mps4_board.subsystem.cpu0.INITSVTOR`,
  `mps4_board.subsystem.iotss3_systemcontrol.INITSVTOR_RST`, `mps4_board.subsystem.ethosu.num_macs`,
  `mps4_board.telnetterminalN.start_telnet/quiet/port`, `mps4_board.QSPI_SRAM_SIZE`,
  `mps4_board.pl370_hdlcd.force_frame_rate`, `mps4_board.rtc.*`; simulator-specific ones are
  `cs320sim.camera`, `cs320sim.microphone`, `cs320sim.speed`.
- **GUI**: the HDLCD output (zoom, fit, 1:1), the camera preview and source selector, the
  microphone meter and source selector, the MPS4 board strip (LEDs, buttons, switches), VIO LEDs/
  buttons/values, six UART terminals in tabs, the status line (time, MIPS, real-time factor).

## Tests

- `firmware/isa-test`: Armv8.1-M ISA groups (integer, DSP, SP/DP/HP FPU, LOB, CSEL, MVE subset,
  security transitions) with digests recorded on the FVP → `fixtures/isa_test.fvp.txt`.
- `fixtures/fvp/`: register dumps (`regs-reset.txt`) and the counter-rate measurement, taken with
  `tools/fvp_dump.py` through the FVP's GDB server.
- Scenarios for Blinky (`expect-uart`, `expect-leds`), Vio (VIO signals), a display test firmware
  (`expect-display` with a CRC of the frame), camera and microphone test firmware.
- `cargo test --workspace`: decoder, FPU, scheduler, RSP, peripheral unit tests.

## Milestones

| M | Content |
|:--|:--|
| M1 | Workspace, time base, core structure (480 IRQs, security state, banked registers, SAU/MPU/SCS), decoder for all Armv8.1-M encodings, integer/LOB/CSEL/TT/SG/DP-FPU execution, exception model. Blinky boots. |
| M2 | SSE-320 peripherals of the table; UART0 on the host; GDB server; FVP CLI; Blinky and Vio scenarios pass. |
| M3 | HDLCD, GUI with the display; I2S and VSI0 microphone; VSI4 and CAM camera; VIO. |
| M4 | ISA test against the FVP; register dumps; MVE phase 1; PMU. |
| M5 | Release builds, CMSIS adapter registration, documentation. |
| Later | MVE phase 2, DMA-350, PPC/MPC enforcement, Ethos-U85 execution (separate project), TF-M (Secure/Non-secure split images). |

## Status

See the section at the top. Decisions that differ from this plan are recorded in
[docs/decisions/](docs/decisions/).
