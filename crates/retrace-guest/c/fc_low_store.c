// M49 first-contact corpus (spec §3b), row F: a store to 0x8000, a wild pointer into the low 4 GiB.
// Natively SIGSEGV (139): __PAGEZERO covers [0, 4 GiB). Under retrace before M49, 0x8000 is
// retrace's own L2 page table and the store rewrites its first entry (spec §2c). Whether the guest
// notices depends on what the TLB still holds: at plan time 17 of 20 recordings exited 0 silently
// and 3 stopped at an instruction abort on retrace's vector table; at t0 13 and 7 (t0 M1). stdout
// is flushed before the store.
#include <stdio.h>

int main(void) {
    volatile long *p = (volatile long *)0x8000;
    puts("storing to 0x8000");
    fflush(stdout);
    *p = 7;
    puts("survived");
    return 0;
}
