// M57 (spec §3c, §4, plan correction 2): a FIXED reservation above 64 GiB, as macOS 27's xzone malloc makes one, a
// FIXED|OVERWRITE commit inside it, and a pattern written and read back. The reservation is the first free candidate
// from 0x80_0000_0000 up in 32 GiB steps, above where malloc's own range group has been seen (0x71..0x7a), so a run
// is the same natively and under retrace wherever that group landed. Addresses go to stderr only.
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <stdio.h>

#define RESERVE (24ull << 30)
#define COMMIT (4ull << 20)

__attribute__((noinline)) void ipa40_done(volatile unsigned char *p) { __asm__ volatile("" :: "r"(p) : "memory"); }

int main(void) {
    mach_vm_address_t base = 0;
    kern_return_t kr = KERN_NO_SPACE;
    for (mach_vm_address_t at = 0x8000000000ull; at < 0xF000000000ull && kr != KERN_SUCCESS; at += 0x800000000ull) {
        base = at;
        kr = mach_vm_map(mach_task_self(), &base, RESERVE, 0, VM_FLAGS_FIXED, MEMORY_OBJECT_NULL, 0, FALSE,
                         VM_PROT_NONE, VM_PROT_ALL, VM_INHERIT_DEFAULT);
    }
    printf("ipa40 reserve=%d\n", kr);
    if (kr != KERN_SUCCESS) return 1;
    mach_vm_address_t seg = base + RESERVE / 2;
    kr = mach_vm_map(mach_task_self(), &seg, COMMIT, 0, VM_FLAGS_FIXED | VM_FLAGS_OVERWRITE, MEMORY_OBJECT_NULL, 0,
                     FALSE, VM_PROT_DEFAULT, VM_PROT_ALL, VM_INHERIT_DEFAULT);
    printf("ipa40 commit=%d\n", kr);
    if (kr != KERN_SUCCESS) return 1;
    fprintf(stderr, "ipa40 reserved at %#llx, committed at %#llx\n", base, seg);
    volatile unsigned char *p = (volatile unsigned char *)seg;
    for (int i = 0; i < 16; i++) { p[i] = (unsigned char)(0xa0 + i); p[COMMIT - 16 + i] = (unsigned char)(0x50 + i); }
    ipa40_done(p);
    printf("ipa40 first=");
    for (int i = 0; i < 16; i++) printf("%02x", p[i]);
    printf(" last=");
    for (int i = 0; i < 16; i++) printf("%02x", p[COMMIT - 16 + i]);
    printf("\n");
    return 0;
}
