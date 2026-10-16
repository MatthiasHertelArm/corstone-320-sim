# Ethos-U85 differential test (NPU command streams)

A bare-metal program that runs six Vela-compiled int8 networks on the Ethos-U85 through Arm's
`ethos-u-core-driver` and prints, over semihosting, the NPU registers as the CPU reads them and one
digest per network output (`name: 0x........` lines, as in `firmware/isa-test`). Its output on Arm's
`FVP_Corstone_SSE-320` (NPU configuration of the simulator, 256 MACs) is recorded in
`fixtures/npu_test.fvp.txt`; the simulator runs the same ELF (`fixtures/elf/npu_test.elf`) and must
produce the same lines, except the `.cycles_*` timing lines.

## Output

| Line | Meaning |
|:--|:--|
| `npu.{id,config,status,prot,cmd}.reset` | ID (0x00), CONFIG (0x28), STATUS (0x04), PROT (0x24), CMD (0x08) read at 0x50004000 before any driver call |
| `npu.init` | return value of `ethosu_init(base=0x50004000, fast_memory=0x31000000, secure=1, privileged=1)` |
| `npu.*.init`, `npu.*.done` | the same registers after `ethosu_init` and after the last network |
| `npu.hw.version/arch/cfg` | `ethosu_get_hw_info`: product_major.version_major.version_minor.status, arch a.b.c, macs_per_cc / cmd stream version / custom DMA |
| `<net>.cmd`, `<net>.weights` | FNV-1a (32 bit) of the COP1 driver payload and of the weight blob as the CPU sees them in DDR: checks the ELF loader |
| `<net>.invoke` | return value of `ethosu_invoke_v3` (0 = OK, 0xffffffff = failed) |
| `<net>.job` | `drv->job.result` in the inference-end hook: 0 OK, 1 timeout, 2 NPU error, 0xff = never started |
| `<net>.status`, `<net>.cmdreg` | STATUS and CMD in the inference-end hook (after the IRQ handler cleared the IRQ) |
| `<net>.output` | FNV-1a of the whole output tensor (int8, in the arena at the Vela output offset) |
| `<net>.out_head` | the first 16 bytes of the output (or all of it when shorter) |
| `<net>.cycles_lo/hi` | NPU PMU cycle counter, counting from `NPU_ACTIVE` to `NPU_IDLE`: the FVP's timing model, not compared |

Markers: `npu-test begin`, `npu-test end`; after a timeout the remaining networks are skipped
(`npu-test timeout, remaining networks skipped`). The driver's error log lines (`E: file:line: ...`)
are printed too.

## Networks

All from Arm's ML-zoo (`https://github.com/ARM-software/ML-zoo`, Apache 2.0; the `.tflite` files are
Git-LFS objects, download them through `https://github.com/ARM-software/ML-zoo/raw/master/<path>`).
Every network compiles to a single NPU subgraph (no CPU operators). The input is int8, filled with
xorshift32 bytes (seed in each header); input and output live in the arena (region 1) at offset 0.

| Name | ML-zoo path (`models/`) | SHA-256 of the .tflite | Operators | Weights | Arena / fast scratch |
|:--|:--|:--|:--|--:|--:|
| `dnn_s` | `keyword_spotting/dnn_small/model_package_tf/model_archive/TFLite/tflite_int8/dnn_s_quantized.tflite` | cbc9be80…11e9 | FULLY_CONNECTED ×4, SOFTMAX | 89840 | 256 / 3040 |
| `cnn_s` | `keyword_spotting/cnn_small/model_package_tf/model_archive/TFLite/tflite_int8/cnn_s_quantized.tflite` | 32d791ec…8d4d | CONV_2D ×2, FULLY_CONNECTED ×3, SOFTMAX, RESHAPE | 79120 | 496 / 10880 |
| `ds_cnn_s` | `keyword_spotting/ds_cnn_small/model_package_tf/model_archive/TFLite/tflite_int8/ds_cnn_s_quantized.tflite` | 0f235d36…ca1a | CONV_2D ×5, DEPTHWISE_CONV_2D ×4, AVERAGE_POOL_2D, FULLY_CONNECTED, SOFTMAX | 33584 | 496 / 22336 |
| `kws_micronet_s` | `keyword_spotting/micronet_small/tflite_int8/kws_micronet_s.tflite` | 0b04ee05…7dae | CONV_2D ×7, DEPTHWISE_CONV_2D ×5, AVERAGE_POOL_2D | 85072 | 496 / 73248 |
| `har_cnn` | `experimental/har_cnn/har_int8.tflite` | 0ea9fb90…466f | CONV_2D, MAX_POOL_2D, FULLY_CONNECTED ×3, SOFTMAX | 471648 | 272 / 28416 |
| `vww2` | `visual_wake_words/micronet_vww2/tflite_int8/vww2_50_50_INT8.tflite` | 0c295ece…324c | CONV_2D ×37, DEPTHWISE_CONV_2D ×17, PAD ×5, ADD ×10, AVERAGE_POOL_2D | 163504 | 2512 / 35360 |

Full SHA-256 values: dnn_s cbc9be800da22587766862cb99aaed59649d0b0f1d0cc279a24b55c7171211e9,
cnn_s 32d791ecf6e111af33f512f694319e22826829a566d6bd3ffc431028a6d88d4d,
ds_cnn_s 0f235d3602308653fcdb50d73462080dec73f21473b4856f05f307d0eb24ca1a,
kws_micronet_s 0b04ee059edf9e392bb9b744a051f19818a662453b0fb4af825267f1a1937dae,
har_cnn 0ea9fb906272246cdad280e4fbda78c20fa55109b36028988a7f861a8e1c466f,
vww2 0c295ece396e33d625c0a3726f1d1f9a7b879612c05a973d0cbf5edf43bf324c.

`mobilenet_v2_1.0_224_INT8` (ML-zoo image classification) also compiles to one NPU subgraph
(3.5 MB of weights, 1.5 MB fast scratch, 150 KB arena) but is not included: its C header would be
10 MB. The layout has room for it (DDR for the weights, 2 MB ISRAM for the fast scratch).

## Vela

Vela 5.2.0 (`pip install ethos-u-vela`), with the `Arm/vela.ini` shipped in the package:

```sh
vela --accelerator-config ethos-u85-256 --output-format raw --config Arm/vela.ini \
     --system-config Ethos_U85_SYS_DRAM_Mid --memory-mode Dedicated_Sram \
     --output-dir out/<name> <model>.tflite          # -> out/<name>/<name>_vela.npz
python3 gen_networks.py --out networks dnn_s=out/dnn_s_quantized/dnn_s_quantized_vela.npz ...
```

The raw output (`ethosu/vela/rawdata_writer.py`) holds `cmd_data` (the complete COP1 driver payload:
`COP1`, an OPTIMIZER_CONFIG action with the CONFIG and ID words Vela compiled for, a COMMAND_STREAM
action and the register command stream), `weight_data`, the scratch sizes and the input/output
offsets and regions. With `Dedicated_Sram` Vela uses region 0 for the weights, region 1 for the arena
(input, output, scratch) and region 2 for the fast scratch (the driver replaces base address 2 with
the `fast_memory` given to `ethosu_init`); `gen_networks.py` asserts this. `networks/net_<name>.h`
holds the three blobs as little-endian `uint32_t` arrays in section `.netdata` plus the input and a
`struct network` descriptor; `networks/networks.h` lists them in run order.

## Memory layout (`link.ld`)

| Region | Address | Content |
|:--|:--|:--|
| Secure boot ROM | 0x11000000 (128 KB) | vectors, code, rodata (`INITSVTOR` = 0x11000000) |
| DTCM | 0x30000000 (32 KB) | `.data`, `.bss`, stack (the NPU cannot reach the TCMs) |
| Secure DDR | 0x70000000 | `.netdata`: payloads, weights (NPU region 0) and inputs, 0.92 MB, loaded by the ELF loader |
| Secure BRAM alias | 0x12000000 (2 MB) | NPU region 1, the arena: zeroed and loaded with the input before each run |
| Secure internal SRAM | 0x31000000 (2 MB) | NPU region 2, the fast scratch (`fast_memory` of `ethosu_init`) |
| Ethos-U85 | 0x50004000 (Secure alias), IRQ 16 | the driver runs it secure and privileged |

Caches stay disabled (reset state), so the driver's weak cache hooks are no-ops. The driver is built
from the CMSIS pack `ARM.ethos-u-core-driver 1.26.2` (`ethosu_driver.c`, `ethosu_device_u85.c`,
`ethosu_pmu.c`, `-DETHOSU_ARCH=u85 -DETHOSU85 -DETHOSU_MACS=256`) with CMSIS-Core 6.3.0 for
`cmsis_compiler.h`; its `fprintf` logging is redirected to semihosting (`log_shim.h`); the mutex
hooks are the driver's defaults and the semaphore hooks poll (`ETHOSU_SEMAPHORE_WAIT_INFERENCE` =
10^8 polls). The PMU is armed in `ethosu_inference_begin` and read in `ethosu_inference_end`.

## Workflow

```sh
export ARM_GCC_BIN=.../compilers.arm.arm.none.eabi.gcc/13.3.1/bin
firmware/npu-test/build.sh                 # -> fixtures/elf/npu_test.elf
firmware/npu-test/record_fvp.sh            # -> fixtures/npu_test.fvp.txt (Docker, image cmsis-fvp-audit:11.32.23)
cargo build -p cs320sim
target/debug/cs320sim --headless --semihosting --time 60 fixtures/elf/npu_test.elf
```

`record_fvp.sh <elf> <out.txt>` keeps the raw FVP output of another image;
`FVP_EXTRA="-C mps4_board.subsystem.ethosu.diagnostics=4"` adds the NPU model's own messages. A run
takes about 6 s on the FVP (the six networks need 6 k to 40 k NPU cycles each); three recordings
were identical, including the cycle counts.

## What the FVP shows

- Before any driver call the CPU already reads ID = 0x20007001 (arch 2.0.0, product_major 7,
  R0P0 status 1), CONFIG = 0x2000251a (product 2 = U85, 2^10 MACs, command stream version 1,
  2 SRAM and 2 EXT AXI ports, 4 weight decoders, no custom DMA), STATUS = 0, PROT = 0 (user,
  secure) and CMD = 0x0000000c (clock and power Q-channels enabled). The GDB-server dump at the
  reset halt (`fixtures/fvp/regs-reset.txt`) read all NPU words as zero: that dump, not the NPU, is
  the odd one.
- After `ethosu_init` (soft reset requesting secure + privileged) PROT = 1 (active_CPL = 1).
- After every run STATUS = 0xffff0020 (`cmd_end_reached`, IRQ history mask all ones) and CMD reads
  0x00000002 although the driver's last write was 0x0c (Q-channel enables): the FVP's CMD read-back
  shows the `clear_irq` bit of the IRQ handler's write, not the later enables.
- The optimizer config check of the driver (`ethosu_dev_verify_optimizer_config`) compares CONFIG's
  product, macs_per_cc, num_axi_ext, num_axi_sram, cmd_stream_version and custom_dma and ID's
  arch_major (equal) and arch_minor (>=) with the words in the COP1 payload (0x2000251a and
  0x20007000 for these networks): a model must return these fields for an inference to start.
