// M52 t1 (spec §3b): mach_vm_copy (MIG 4807) by case, one per process so a refusal stops one
// recording. Output: `<case> kr=<n> sum=<fnv1a of the destination>`; no addresses. Adopted from the
// t0b probe vmcopy.c. Cases: disjoint, overlap_fwd, overlap_back, unmapped_src, none_dst, unaligned,
// zero, reserved.
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>

#define PG 16384ull

static unsigned long long fnv(const unsigned char *p, size_t n) {
    unsigned long long h = 1469598103934665603ull;
    for (size_t i = 0; i < n; i++) { h ^= p[i]; h *= 1099511628211ull; }
    return h;
}
static unsigned char *alloc(size_t n) {
    mach_vm_address_t a = 0;
    if (mach_vm_allocate(mach_task_self(), &a, n, VM_FLAGS_ANYWHERE)) exit(1);
    return (unsigned char *)a;
}
static void fill(unsigned char *p, size_t n, unsigned seed) {
    for (size_t i = 0; i < n; i++) p[i] = (unsigned char)((i * 31 + seed) ^ (i >> 8));
}
static void say(const char *c, kern_return_t kr, const unsigned char *d, size_t n) {
    printf("%s kr=%d sum=%016llx\n", c, kr, fnv(d, n));
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    const char *c = argc > 1 ? argv[1] : "disjoint";
    mach_port_t t = mach_task_self();
    kern_return_t kr;
    if (strcmp(c, "disjoint") == 0) {
        unsigned char *s = alloc(65536), *d = alloc(65536);
        fill(s, 65536, 1);
        kr = mach_vm_copy(t, (mach_vm_address_t)s, 65536, (mach_vm_address_t)d);
        say(c, kr, d, 65536);
    } else if (strcmp(c, "overlap_fwd") == 0) {         // dst = src + one page
        unsigned char *b = alloc(4 * PG); fill(b, 4 * PG, 2);
        kr = mach_vm_copy(t, (mach_vm_address_t)b, 2 * PG, (mach_vm_address_t)(b + PG));
        say(c, kr, b, 4 * PG);
    } else if (strcmp(c, "overlap_back") == 0) {        // dst = src - one page
        unsigned char *b = alloc(4 * PG); fill(b, 4 * PG, 3);
        kr = mach_vm_copy(t, (mach_vm_address_t)(b + PG), 2 * PG, (mach_vm_address_t)b);
        say(c, kr, b, 4 * PG);
    } else if (strcmp(c, "unmapped_src") == 0) {
        unsigned char *s = alloc(PG), *d = alloc(PG);
        mach_vm_deallocate(t, (mach_vm_address_t)s, PG);
        fill(d, PG, 4);
        kr = mach_vm_copy(t, (mach_vm_address_t)s, PG, (mach_vm_address_t)d);
        say(c, kr, d, PG);
    } else if (strcmp(c, "none_dst") == 0) {            // the checksum is taken after restoring RW
        unsigned char *s = alloc(PG), *d = alloc(PG);
        fill(s, PG, 7); fill(d, PG, 8);
        mprotect(d, PG, PROT_NONE);
        kr = mach_vm_copy(t, (mach_vm_address_t)s, PG, (mach_vm_address_t)d);
        mprotect(d, PG, PROT_READ | PROT_WRITE);
        say(c, kr, d, PG);
    } else if (strcmp(c, "unaligned") == 0) {           // 100 bytes, no page rounding
        unsigned char *s = alloc(PG), *d = alloc(PG);
        fill(s, PG, 9); fill(d, PG, 10);
        kr = mach_vm_copy(t, (mach_vm_address_t)s, 100, (mach_vm_address_t)d);
        say(c, kr, d, PG);
    } else if (strcmp(c, "zero") == 0) {
        unsigned char *s = alloc(PG), *d = alloc(PG);
        fill(s, PG, 11); fill(d, PG, 12);
        kr = mach_vm_copy(t, (mach_vm_address_t)s, 0, (mach_vm_address_t)d);
        say(c, kr, d, PG);
    } else if (strcmp(c, "reserved") == 0) {            // a PROT_NONE reservation committed by mprotect, never written
        mach_vm_address_t r = 0;
        mach_vm_map(t, &r, 2 * PG, 0, VM_FLAGS_ANYWHERE, MEMORY_OBJECT_NULL, 0, FALSE,
                    VM_PROT_NONE, VM_PROT_ALL, VM_INHERIT_DEFAULT);
        mprotect((void *)r, 2 * PG, PROT_READ | PROT_WRITE);
        unsigned char *d = alloc(2 * PG); fill(d, 2 * PG, 13);
        kr = mach_vm_copy(t, r, 2 * PG, (mach_vm_address_t)d);
        say(c, kr, d, 2 * PG);
    } else {
        return 2;
    }
    return 0;
}
