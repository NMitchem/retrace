// M47 fixture (spec §3f): madvise, by mode.
//   zero  — map 1 MiB, fill it with 0xAB, MADV_ZERO the first 512 KiB, report both halves.
//   reuse — fill, MADV_FREE_REUSABLE, MADV_FREE_REUSE, report whether the bytes were kept, then
//           write a pattern and read it back.
//   bad   — an advice outside the measured set (MADV_CAN_REUSE, 9); prints what it returned.
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>

#define MIB (1u << 20)

static int all(const unsigned char *p, size_t n, unsigned char v) {
    for (size_t i = 0; i < n; i++) if (p[i] != v) return 0;
    return 1;
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "zero";
    unsigned char *p = mmap(NULL, MIB, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    if (p == MAP_FAILED) { perror("mmap"); return 1; }
    memset(p, 0xAB, MIB);
    if (strcmp(mode, "zero") == 0) {
        if (madvise(p, MIB / 2, MADV_ZERO) != 0) { printf("zero errno=%d\n", errno); return 1; }
        printf("zero low=%s high=%s\n", all(p, MIB / 2, 0) ? "zeros" : "dirty",
               all(p + MIB / 2, MIB / 2, 0xAB) ? "kept" : "changed");
        return 0;
    }
    if (strcmp(mode, "reuse") == 0) {
        if (madvise(p, MIB, MADV_FREE_REUSABLE) != 0) { printf("reusable errno=%d\n", errno); return 1; }
        if (madvise(p, MIB, MADV_FREE_REUSE) != 0) { printf("reuse errno=%d\n", errno); return 1; }
        printf("reuse %s\n", all(p, MIB, 0xAB) ? "kept" : "dropped");
        memset(p, 0xCD, MIB);
        printf("reuse %s\n", all(p, MIB, 0xCD) ? "ok" : "bad");
        return 0;
    }
    if (strcmp(mode, "bad") == 0) {
        int rc = madvise(p, MIB, MADV_CAN_REUSE);
        printf("bad rc=%d errno=%d\n", rc, rc ? errno : 0);
        return 0;
    }
    fprintf(stderr, "unknown mode %s\n", mode);
    return 2;
}
