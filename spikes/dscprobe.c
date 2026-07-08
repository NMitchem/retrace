// Host probe: does DYLD_SHARED_REGION=private make dyld map the shared cache itself
// (via ordinary open/mmap/pread), instead of joining the kernel-managed shared region?
// Run twice:  ./dscprobe   vs   DYLD_SHARED_REGION=private ./dscprobe
// In normal mode &printf lies inside the kernel shared region returned by
// shared_region_check_np; in private mode it must lie outside it.
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/syscall.h>

int main(void) {
    uint64_t base = 0;
    long r = syscall(SYS_shared_region_check_np, &base);
    uint64_t p = (uint64_t)&printf;
    int inside = (r == 0) && p >= base && p < base + 0x200000000ULL; // 8 GiB span
    printf("shared_region_check_np -> %ld  kernel-region base=0x%llx\n", r, base);
    printf("&printf=0x%llx  => libSystem mapped %s\n", p,
           inside ? "INSIDE kernel shared region" : "OUTSIDE kernel shared region (PRIVATE mapping)");
    printf("DYLD_SHARED_REGION=%s\n", getenv("DYLD_SHARED_REGION") ?: "(unset)");
    return inside ? 10 : 20; // distinct exit codes for scripting
}
