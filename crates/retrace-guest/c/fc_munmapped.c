// M49 first-contact corpus (spec §3b), row F: a load from a page the program unmapped itself
// (a use-after-munmap). Natively SIGSEGV (139). The address is never printed: native mmap
// placement is randomised, so only the fault is deterministic. stdout is flushed before the fault.
#include <stdio.h>
#include <sys/mman.h>

int main(void) {
    volatile char *m = mmap(0, 0x4000, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    if (m == MAP_FAILED) { puts("mmap failed"); return 2; }
    m[8] = 42;
    printf("wrote %d, unmapping\n", m[8]);
    fflush(stdout);
    munmap((void *)m, 0x4000);
    char v = m[8];
    printf("survived v=%d\n", v);
    return 0;
}
