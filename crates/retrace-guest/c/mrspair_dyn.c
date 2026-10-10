// M49 fixture (spec §3e, Task 4; the evidence's `mrspair.c`, registers pinned): a discarded-status
// exclusive pair with a trapped timebase read between its halves. Natively the read does not trap
// and the store lands. Under retrace before M49 the read exits to the box, the ERET back clears
// the local monitor, and the store is lost every time (static `excl-report.md` §1).
//
// Modes (argv[1]):
//   (none)          the pair in this executable's own __TEXT (staged by `load_dynamic`), then the
//                   retry-loop control; prints "cell=<n> ctr=<n> tries=<n>".
//                   Native: cell=4242 ctr=1 tries=1. Under retrace since M49: cell=4242 ctr=0
//                   tries=5 (the retry loop reads its status, so X1 leaves it alone, and the
//                   trapped read fails its store on every pass: the documented limit).
//   dlopen <path>   the same pair in mrspair_lib.dylib, dlopen'd by absolute path, so it is staged
//                   by `guest_mmap_file`'s exec path; prints "libcell=<n>". Native and M49: 4242.
//
// Pinned registers: the pair is `ldxr w10, [x9]; mrs x11, cntvct_el0; cbnz w10, 1f;
// stxr wzr, w0, [x9]`, and the retry loop's store is `stxr w12, w10, [x9]`, so xrewrite_e2e can
// find each by its words in the recorded snapshot.
#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

static volatile uint32_t cell __attribute__((aligned(64)));
static volatile uint32_t ctr __attribute__((aligned(64)));

int main(int argc, char **argv) {
    if (argc == 3 && strcmp(argv[1], "dlopen") == 0) {
        void *h = dlopen(argv[2], RTLD_NOW);
        uint32_t (*fill)(uint32_t) = h ? (uint32_t (*)(uint32_t))dlsym(h, "mrspair_lib_fill") : NULL;
        if (!fill) { printf("dlopen/dlsym failed: %s\n", dlerror()); return 3; }
        printf("libcell=%u\n", fill(4242));
        return 0;
    }
    if (argc != 1) { printf("usage: mrspair_dyn [dlopen <path>]\n"); return 2; }
    uint32_t v = 4242;
    cell = 0;
    asm volatile(
        "mov x9, %[p]\n"
        "mov w0, %w[v]\n"
        "ldxr w10, [x9]\n"
        "mrs x11, cntvct_el0\n"
        "cbnz w10, 1f\n"
        "stxr wzr, w0, [x9]\n"
        "1:\n" : : [p] "r"(&cell), [v] "r"(v) : "x0", "x9", "x10", "x11", "memory");
    // The retry-loop control: the same trapped read inside an increment loop whose store's status
    // is read. Bounded at five passes so a store that never succeeds cannot hang the guest.
    uint32_t tries = 0;
    asm volatile(
        "mov x9, %[p]\n"
        "2: add %w[t], %w[t], #1\n"
        "ldxr w10, [x9]\n"
        "mrs x11, cntvct_el0\n"
        "add w10, w10, #1\n"
        "stxr w12, w10, [x9]\n"
        "cmp %w[t], #5\n"
        "b.hs 3f\n"
        "cbnz w12, 2b\n"
        "3:\n" : [t] "+r"(tries) : [p] "r"(&ctr) : "x9", "x10", "x11", "x12", "memory", "cc");
    printf("cell=%u ctr=%u tries=%u\n", cell, ctr, tries);
    return 0;
}
