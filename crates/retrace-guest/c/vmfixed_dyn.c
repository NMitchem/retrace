// M52 t1 (spec §3b): FIXED Mach VM requests without VM_FLAGS_OVERWRITE over occupied memory. Native
// answers KERN_NO_SPACE (3) and keeps the old bytes; adopted from the t0b probes fixednoover.c,
// fixedextra.c and fixedwin.c. Output has no addresses, so it compares with native byte for byte.
//   vmfixed_dyn cases      contained, covering, straddling, reservation, allocate, mig, adjacent, free
//   vmfixed_dyn overwrite  the contained case with VM_FLAGS_OVERWRITE: replaced, marker lost
//   vmfixed_dyn remap      mach_vm_remap FIXED onto its own allocation, without and with OVERWRITE
//   vmfixed_dyn windows    FIXED at the low, shared-region and commpage windows (and one free address)
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define PG 16384ull

static kern_return_t map_fixed(mach_vm_address_t at, mach_vm_size_t size, int flags, vm_prot_t max) {
    mach_vm_address_t a = at;
    return mach_vm_map(mach_task_self(), &a, size, 0, flags, MEMORY_OBJECT_NULL, 0, FALSE,
                       VM_PROT_DEFAULT, max, VM_INHERIT_DEFAULT);
}

// A free window of `pages` pages: allocate it ANYWHERE, then give it back.
static mach_vm_address_t window(int pages) {
    mach_vm_address_t w = 0;
    if (mach_vm_allocate(mach_task_self(), &w, pages * PG, VM_FLAGS_ANYWHERE)) { puts("window alloc failed"); exit(1); }
    mach_vm_deallocate(mach_task_self(), w, pages * PG);
    return w;
}

static volatile char *live(mach_vm_address_t at, int pages) {
    mach_vm_address_t a = at;
    if (mach_vm_allocate(mach_task_self(), &a, pages * PG, VM_FLAGS_FIXED)) { puts("live alloc failed"); exit(1); }
    return (volatile char *)a;
}

static void say(const char *c, kern_return_t kr, volatile char *marker) {
    if (marker) printf("%s kr=%d marker=%s\n", c, kr, *marker == 'M' ? "intact" : "lost");
    else printf("%s kr=%d\n", c, kr);
}

static void contained(const char *name, int flags) {
    mach_vm_address_t w = window(8);
    volatile char *a = live(w, 3);
    a[PG] = 'M';
    say(name, map_fixed((mach_vm_address_t)a + PG, PG, flags, VM_PROT_ALL), a + PG);
}

static void window_row(unsigned long long at) {
    mach_vm_address_t a = at;
    kern_return_t kr = mach_vm_allocate(mach_task_self(), &a, PG, VM_FLAGS_FIXED);
    printf("window %#llx kr=%d\n", at, kr);
    if (kr == 0) mach_vm_deallocate(mach_task_self(), a, PG);
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    const char *mode = argc > 1 ? argv[1] : "cases";
    mach_vm_address_t w;
    volatile char *a, *m;
    if (strcmp(mode, "overwrite") == 0) {
        contained("overwrite", VM_FLAGS_FIXED | VM_FLAGS_OVERWRITE);
    } else if (strcmp(mode, "remap") == 0) {
        // M52 final-review I1: mach_vm_remap (4813) FIXED over an occupied target. Without OVERWRITE
        // native answers KERN_NO_SPACE and keeps the old bytes; with it the target is replaced.
        for (int ow = 0; ow < 2; ow++) {
            w = window(8);
            a = live(w, 2);                     // the source, one page of 'S'
            a[0] = 'S';
            m = live(w + 4 * PG, 1);            // the target, which is occupied
            *m = 'M';
            mach_vm_address_t t = w + 4 * PG;
            vm_prot_t cur, max;
            kern_return_t kr = mach_vm_remap(mach_task_self(), &t, PG, 0,
                                             VM_FLAGS_FIXED | (ow ? VM_FLAGS_OVERWRITE : 0),
                                             mach_task_self(), (mach_vm_address_t)a, FALSE, &cur, &max,
                                             VM_INHERIT_DEFAULT);
            printf("remap%s kr=%d target=%s\n", ow ? "-overwrite" : "", kr, *m == 'S' ? "aliased" : *m == 'M' ? "intact" : "other");
        }
    } else if (strcmp(mode, "windows") == 0) {
        // 0x300000000 is free natively and in the box; the rest are __PAGEZERO, the shared region and
        // the commpages. None is a wall under M52 §3b: native's code is the box's code.
        unsigned long long v[] = {0x4000, 0x8000, 0x40000, 0x800000, 0x180000000ull, 0x2ffffc000ull,
                                  0xfffffc000ull, 0x300000000ull};
        for (unsigned i = 0; i < sizeof v / sizeof *v; i++) window_row(v[i]);
    } else {
        contained("contained", VM_FLAGS_FIXED);
        w = window(8);                          // covering: one live page inside a 5-page request
        m = live(w + 2 * PG, 1);
        *m = 'M';
        say("covering", map_fixed(w, 5 * PG, VM_FLAGS_FIXED, VM_PROT_ALL), m);
        w = window(8);                          // straddling: a live page's end plus a free page
        a = live(w + PG, 2);
        a[PG] = 'M';
        say("straddling", map_fixed(w + 2 * PG, 2 * PG, VM_FLAGS_FIXED, VM_PROT_ALL), a + PG);
        {                                       // over a PROT_NONE reservation
            mach_vm_address_t r = 0;
            mach_vm_map(mach_task_self(), &r, 4 * PG, 0, VM_FLAGS_ANYWHERE, MEMORY_OBJECT_NULL, 0, FALSE,
                        VM_PROT_NONE, VM_PROT_NONE, VM_INHERIT_DEFAULT);
            say("reservation", map_fixed(r + PG, PG, VM_FLAGS_FIXED, VM_PROT_ALL), 0);
        }
        w = window(8);                          // mach_vm_allocate's FIXED
        m = live(w + PG, 1);
        *m = 'M';
        mach_vm_address_t p = w + PG;
        say("allocate", mach_vm_allocate(mach_task_self(), &p, PG, VM_FLAGS_FIXED), m);
        // mig: max_protection != VM_PROT_ALL takes libsystem's MIG route (msgh_id 4811), not the trap.
        w = window(8);
        a = live(w, 3);
        a[PG] = 'M';
        say("mig", map_fixed((mach_vm_address_t)a + PG, PG, VM_FLAGS_FIXED, VM_PROT_READ | VM_PROT_WRITE), a + PG);
        // adjacent: one request ends where a live allocation starts, one starts where it ends.
        w = window(8);
        m = live(w + 2 * PG, 2);
        *m = 'M';
        kern_return_t k1 = map_fixed(w, 2 * PG, VM_FLAGS_FIXED, VM_PROT_ALL);
        kern_return_t k2 = map_fixed(w + 4 * PG, 2 * PG, VM_FLAGS_FIXED, VM_PROT_ALL);
        say("adjacent", k1 | k2, m);
        w = window(8);
        say("free", map_fixed(w + PG, 2 * PG, VM_FLAGS_FIXED, VM_PROT_ALL), 0);
    }
    return 0;
}
