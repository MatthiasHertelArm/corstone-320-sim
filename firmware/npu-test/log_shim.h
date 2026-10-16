/* Force-included when compiling the Ethos-U core driver: routes its LOG_* fprintf calls to the
   test's semihosting printer (newlib's stdio streams are not set up in this bare-metal image). */
#include <stdio.h>
int npu_log_printf(void *stream, const char *fmt, ...);
#define fprintf(stream, ...) npu_log_printf((void *)(stream), __VA_ARGS__)
