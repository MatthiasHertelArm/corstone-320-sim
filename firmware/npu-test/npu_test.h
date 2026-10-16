/* Shared between npu_test.c and the generated network headers (networks/net_*.h). */
#ifndef NPU_TEST_H
#define NPU_TEST_H
#include <stdint.h>
#include <stddef.h>

/* One Vela-compiled network (raw output format, memory mode Dedicated_Sram):
   region 0 = weights, region 1 = arena (scratch, holds the input and the output),
   region 2 = fast scratch. */
struct network {
  const char *name;
  const uint32_t *cmd;       /* COP1 driver payload: driver actions + register command stream */
  size_t cmd_size;
  const uint32_t *weights;   /* encoded weights (region 0) */
  size_t weights_size;
  const uint32_t *input;     /* input tensor, copied to arena + input_offset before the run */
  size_t input_size;
  size_t input_offset;
  size_t output_size;        /* output tensor at arena + output_offset after the run */
  size_t output_offset;
  size_t scratch_size;       /* bytes of region 1 the network uses */
  size_t fast_size;          /* bytes of region 2 the network uses */
};
#endif
