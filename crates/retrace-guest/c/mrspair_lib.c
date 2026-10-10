// M49 fixture (spec §3e, Task 4): mrspair_dyn's getpid-shaped pair, in a dylib that mrspair_dyn
// dlopens by absolute path, so dyld stages its __TEXT with a FIXED exec mmap of the file and the
// pair reaches the box through `guest_mmap_file`'s exec path rather than through `load_dynamic`.
// The registers are pinned (x9 base, w10 loaded, x11 the read, w0 the value), so its words are
// exactly mrspair_dyn's and the getpid pair's, plus the trapped read:
//   ldxr w10, [x9]; mrs x11, cntvct_el0; cbnz w10, 1f; stxr wzr, w0, [x9]
#include <stdint.h>

static volatile uint32_t lib_cell __attribute__((aligned(64)));

uint32_t mrspair_lib_fill(uint32_t v) {
    lib_cell = 0;
    asm volatile(
        "mov x9, %[p]\n"
        "mov w0, %w[v]\n"
        "ldxr w10, [x9]\n"
        "mrs x11, cntvct_el0\n"
        "cbnz w10, 1f\n"
        "stxr wzr, w0, [x9]\n"
        "1:\n" : : [p] "r"(&lib_cell), [v] "r"(v) : "x0", "x9", "x10", "x11", "memory");
    return lib_cell;
}
