# corstone-320-sim

Instruction-set simulator of the Arm Corstone-320 subsystem: a Cortex-M85 with Helium, an Ethos-U85
NPU, the SSE-320 peripherals and the MPS4 board. It is a GDB server for the Arm CMSIS Debugger, and it
shows a window with the board's display, camera and microphone. Written in Rust for Windows, macOS and
Linux.

It simulates the subsystem that the CMSIS-Pack `ARM::SSE_320_BSP` describes, so the BSP examples (Blinky,
Vio) run unmodified without the Corstone-320 Ecosystem FVP. The FVP is a Linux binary that needs Docker on
macOS; this simulator runs natively.

[PLAN.md](PLAN.md) has the scope, architecture and fidelity contract.

## What is simulated

| Part | Model |
|:--|:--|
| Cortex-M85 | Armv8.1-M Mainline with Helium (MVE), double-precision FPU, TrustZone, PACBTI, SysTick, NVIC, MPU, DWT, FPB, ITM. The ISA differential test (`firmware/isa-test`, 320 scalar groups and 107 Helium groups) runs against a recording of the FVP; the groups that still differ are listed in [fixtures/isa_test.known_mismatches.txt](fixtures/isa_test.known_mismatches.txt). |
| Ethos-U85 | Functional model of the APB register block and the command stream (cmd0/cmd1, weight and scale streams, CONV, DEPTHWISE, POOL, ELEMENTWISE, RESIZE, DMA, BRANCH) for the `ethos-u85-<macs>` configurations that Vela compiles for. The arithmetic follows the TRM pseudocode; timing is an estimate. |
| SSE-320 subsystem | Memory map and register maps of the SSE-320 TRM and the BSP headers. CMSDK UARTs, system timers, watchdogs, GPIO, I2C, SPI, I2S, DMA-350, HDLCD display (any resolution the firmware programs, up to 4096 × 4096). |
| MPS4 board | 10 user LEDs, 2 buttons, 8 switches, the display window, a camera and a microphone. The camera is a host webcam, image files, a folder of frames or a test pattern, delivered through the AVH video interface and a capture peripheral with a documented register map. The microphone is the host microphone, a WAV file or a tone, delivered through I2S and the AVH audio interface. |

Time is kept at the FVP's rates: one instruction per CPU clock at 25 MHz, and peripherals exact in their own
clocks, so `--stats` cycle counts are comparable with the FVP.

## Install

[Releases](https://github.com/MatthiasHertelArm/corstone-320-sim/releases) have one archive per host:
Linux, macOS and Windows, each on x64 and arm64. Each holds `cs320sim` (`cs320sim.exe`), this README, the
licence and the CMSIS Debugger adapter files in `cmsis/`; `SHA256SUMS` has the checksums.

Asset names: `cs320sim-<version>-<os>-<arch>.tar.gz` (`.zip` on Windows), with `<os>` `linux`, `macos` or
`windows` and `<arch>` `x64` or `arm64`.

macOS arm64 example, with the GitHub CLI:

```sh
gh release download v0.1.0 -R MatthiasHertelArm/corstone-320-sim -p 'cs320sim-v0.1.0-macos-arm64.tar.gz'
tar -xzf cs320sim-v0.1.0-macos-arm64.tar.gz
xattr -d com.apple.quarantine cs320sim-v0.1.0-macos-arm64/cs320sim   # unsigned binary
export PATH="$PWD/cs320sim-v0.1.0-macos-arm64:$PATH"
cs320sim --help
```

Verify a download against `SHA256SUMS` from the same release (`shasum -a 256 -c SHA256SUMS --ignore-missing`).
Without a release for your host, build from source (see [Build](#build)).

## Build

Rust 1.95 or later.

```sh
cargo build --release
# Linux needs the ALSA and X11/Wayland headers for audio and the window:
#   sudo apt install libasound2-dev libxkbcommon-dev libwayland-dev
# build without the window:
cargo build --release -p cs320sim --no-default-features
```

The binary is `target/release/cs320sim`. The CMSIS adapter calls it by that name, so put it on the `PATH`
(or set the adapter's command path) before you use it from VS Code.

## Run

```sh
cs320sim fixtures/elf/Blinky.elf                              # window with the board
cs320sim path/to/image.hex                                    # ELF, HEX or BIN images
cs320sim --cbuild-run path/to/<project>.cbuild-run.yml        # images from a CMSIS build
cs320sim Blinky.elf --headless --time 5                       # UART on stdout
cs320sim Blinky.elf --uart-tcp 5000                           # UART on a TCP port
cs320sim Blinky.elf --headless --scenario tests/scenarios/blinky.yaml   # exit code 1 on failure
```

`cs320sim --help` lists all options. The FVP command line is accepted as well (`cs320sim -a IMAGE`), so an
FVP debug adapter with the simulator as the model works too.

## Debug with the CMSIS Debugger

The simulator is a GDB server: `cs320sim gdbserver --cbuild-run <file> --port 3333` takes the options of
`pyocd gdbserver`. It serves the debugger's controlling connection, the auxiliary connection (Live Watch,
CPU Time), an M-profile target description, a memory map with flash for `load`, code breakpoints through
the FPB, watchpoints through the DWT, and monitor commands.

`cs320sim load`, `cs320sim erase` and `cs320sim info` talk to a running simulator on its control port.

## Use as a CMSIS target

This is the procedure for adding the simulator as a debug target to a CMSIS Solution project. It uses the
adapter in [cmsis/](cmsis/). The adapter's name is `corstone-320-sim`; the aliases `cs320sim` and
`corstone-320` also resolve to it.

1. **Build the simulator** (see [Build](#build)) and make `cs320sim` available on the `PATH`.

2. **Register the adapter with CMSIS Solution.** Dry run first; it prints where the files would go:

   ```sh
   cs320sim register-cmsis-adapter
   cs320sim register-cmsis-adapter --apply --backup-dir ~/cs320sim-backup
   ```

   `--apply` adds `corstone-320-sim.adapter.json` (the launch, attach and task definitions) and the
   `debug-adapters.yml` entry to the CMSIS Solution extension's catalog. The backup directory keeps the
   original files. Run it again after updating the extension; it reports "already registered" when nothing
   changed.

3. **Select the simulator as the debugger in the csolution.** Add a target type that uses the BSP's board
   and device, and name the debugger `corstone-320-sim`:

   ```yaml
   # Blinky.csolution.yml (the SSE-320 BSP pack ARM::SSE_320_BSP must be in packs:)
   target-types:
     - type: SSE-320-sim
       board: SSE-320
       device: SSE-320-FVP
       target-set:
         - set:
           debugger:
             name: corstone-320-sim
           images:
             - project-context: Blinky.Debug
   ```

   Keep the existing `SSE-320-FVP` target type for the FVP. The simulator target is a separate target type,
   so the two builds do not share an output folder.

4. **Build** with CMSIS Solution as usual (`cbuild` or the VS Code command). The debugger then reads the
   generated `*.cbuild-run.yml`, which lists the images and the GDB port for the simulator.

5. **Debug.** The adapter provides three launch configurations and three tasks:

   | Name | What it does |
   |:--|:--|
   | Corstone-320 simulator (launch) | Starts `cs320sim gdbserver` on the cbuild-run file, resets and halts at `main`, and connects the debugger on port 3333. |
   | Corstone-320 simulator (attach) | Connects to a simulator that is already running on port 3333. |
   | CMSIS Load / CMSIS Erase / CMSIS Run | Load the images into a running simulator, erase its flash, or run the simulator as a GDB server. |

   The debugger's **Simulation** settings set the adapter's options:

   | Option | Values | Default |
   |:--|:--|:--|
   | Speed | simulation speed factor (`1` is real time) or `max` | `1` |
   | Headless | `window` or `headless` (UART on the terminal, no window) | `window` |
   | Camera | `pattern`, `none`, `live`, an image file or a folder of images | `pattern` |

   The **Telnet** section serves the target UART on a TCP port (default 5000), which the VS Code serial
   monitor can open in TCP mode.

6. **Check the target before debugging.** `cs320sim info --cbuild-run <file>` prints the target information
   for the build.

If you do not want the adapter, use a hand-written `launch.json` with `"updateConfiguration": "manual"`
(the same `cs320sim gdbserver --cbuild-run <file> --port 3333` command as the launch configuration).

## Scenarios

A scenario drives inputs and checks outputs in simulated time, so the result is the same on every host:

```yaml
- at: 0.37
  expect-uart: "= Blinky is running ="
  expect-leds: "0000000001"
- at: 0.62
  expect-leds: "0000000010"
```

[tests/scenarios/](tests/scenarios/) has the BSP Blinky and Vio scenarios.

## Tests

```sh
cargo test --workspace --release
cargo test -p cs320-board --test reference   # ISA differential test against the recorded FVP output
```

The reference data is in [fixtures/](fixtures/), recorded from `FVP_Corstone_SSE-320` 11.32.
[tools/fvp_dump.py](tools/fvp_dump.py) dumps registers from a GDB server, and the firmware for the test
images is in [firmware/](firmware/).

## Licence

Apache License 2.0. See [LICENSE](LICENSE).
