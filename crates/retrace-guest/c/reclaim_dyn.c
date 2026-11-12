// M59: xzone malloc's deferred-reclamation ring, driven the way a program drives it.
//
// libmalloc asks the kernel for the ring at startup (mach_vm_deferred_reclamation_buffer_allocate,
// MIG 4822). When that fails, malloc_report() logs the failure, which on a terminal prints
// "xzm: failed to initialize deferred reclamation buffer" and otherwise opens a syslog socket.
// The first free descriptor is therefore the cheapest witness: 3 natively, 4 when the ring was refused.
//
// The churn loop frees enough large blocks to fill the ring's first 1016 entries, so libmalloc
// resizes it (MIG 4826), which drains every entry; whole cached segments go in as DEALLOCATE
// entries and are unmapped by that drain. Every block is filled and checked after it is allocated,
// never read before, so the output never depends on what a reused block held.
#include <mach/mach.h>
#include <mach/mach_traps.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(void) {
    int fd = dup(0);
    printf("first free fd %d\n", fd);
    close(fd);

    enum { KEEP = 48, ROUNDS = 6000 };
    static unsigned char *keep[KEEP];
    static size_t keep_size[KEEP];
    uint64_t sum = 0;
    for (int i = 0; i < ROUNDS; i++) {
        size_t sz = ((size_t)(1 + (i * 37) % 160)) << 15;          /* 32 KiB .. 5 MiB */
        unsigned char *p = malloc(sz);
        if (!p) { printf("malloc failed at %d\n", i); return 1; }
        size_t touch = sz < 65536 ? sz : 65536;
        memset(p, (unsigned char)i, touch);
        p[sz - 1] = (unsigned char)(i >> 3);
        int k = i % KEEP;
        if (keep[k]) {
            size_t t = keep_size[k] < 65536 ? keep_size[k] : 65536;
            sum += keep[k][0] + keep[k][t - 1] + keep[k][keep_size[k] - 1];
            free(keep[k]);
        }
        keep[k] = p;
        keep_size[k] = sz;
    }
    for (int k = 0; k < KEEP; k++) free(keep[k]);
    printf("churn %d rounds, checksum %llu\n", ROUNDS, (unsigned long long)sum);

    uint64_t bytes = 0, deadline = 0;
    kern_return_t kr = mach_vm_reclaim_update_kernel_accounting_trap(mach_task_self(), &bytes, &deadline);
    printf("accounting trap kr %d\n", kr);
    return 0;
}
