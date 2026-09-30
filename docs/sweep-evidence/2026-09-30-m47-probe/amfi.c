// Throwaway: ask AMFI for this process's dyld policy exactly as dyld does
// (__mac_syscall("AMFI", 0x5a, {inFlags, &outFlags})).
#include <stdio.h>
#include <stdint.h>
int __mac_syscall(const char *policy, int call, void *arg);
int main(void) {
    for (uint64_t in = 0; in <= 6; in += 2) {  // dyld passes 0, 2, 4 or 6 (amfiFlags' two bools)
        uint64_t out = 0xAAAAAAAAAAAAAAAAull;
        struct { uint64_t in; uint64_t *out; } a = { in, &out };
        int r = __mac_syscall("AMFI", 0x5a, &a);
        printf("in=%llu ret=%d out=%#llx\n", in, r, out);
    }
    return 0;
}
