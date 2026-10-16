/* Shared between isa_test.c (scalar groups, built without MVE) and mve_test.c (MVE section). */
#ifndef ISA_TEST_H
#define ISA_TEST_H
#include <stdint.h>

void puts_(const char *s);
void report(const char *name, uint32_t digest);
void mix(uint32_t v);
uint32_t rnd(void);
void begin(const char *name, unsigned namelen);
uint32_t operand(int i, int which);      /* integer edge pairs, then pseudo-random */
uint32_t fp_operand(int i, int which);   /* single-precision edge pairs, then pseudo-random */
extern uint32_t digest;

#define BEGIN(name) begin(name, sizeof(name))

void run_mve(void);
#endif
