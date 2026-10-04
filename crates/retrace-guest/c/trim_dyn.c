// M48 Task 3: V8's aligned-reservation trim (walls.md §1 row 3). Map 0x7c000 bytes, unmap a
// 0x10000 head and a tail whose length is not page-aligned (the kernel rounds its end up), then use
// the middle. Modes: trim (the middle keeps its bytes and takes writes), and head and tail (touching
// a released part faults, as it does natively). The TRIM marker names the two addresses the
// fault modes touch.
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "trim";
    uint8_t *p = mmap(NULL, 0x7c000, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    if (p == MAP_FAILED) { puts("mmap failed"); return 2; }
    for (size_t i = 0; i < 0x7c000; i++) p[i] = (uint8_t)(i * 7);
    printf("TRIM head=%p tail=%p\n", (void *)p, (void *)(p + 0x7c000 - 8));
    fflush(stdout);
    if (munmap(p, 0x10000) != 0 || munmap(p + 0x50000, 0x2bb20) != 0) { puts("munmap failed"); return 2; }
    if (!strcmp(mode, "head")) { volatile uint64_t *q = (volatile uint64_t *)p; return (int)*q; }
    if (!strcmp(mode, "tail")) { volatile uint64_t *q = (volatile uint64_t *)(p + 0x7c000 - 8); return (int)*q; }
    uint64_t sum = 0;
    for (size_t i = 0x10000; i < 0x50000; i++) sum = sum * 31 + p[i];
    memset(p + 0x10000, 0xab, 0x40000);
    for (size_t i = 0x10000; i < 0x50000; i++) if (p[i] != 0xab) { puts("the middle lost a write"); return 1; }
    printf("trim ok sum=%#llx\n", (unsigned long long)sum);
    return 0;
}
