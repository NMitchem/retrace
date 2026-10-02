// mach_vm_map's alignment mask on an ANYWHERE placement (the m47-abort guard). libmalloc's xzone
// allocator maps each 4 MiB segment with mask 0x3fffff and then registers it in a segment table
// indexed by `addr >> 22`, one entry per 4 MiB granule from the base, so a segment that comes back
// off a 4 MiB boundary straddles a granule nobody registers, and a free of a chunk in that tail
// aborts with "pointer being freed was not allocated". Natively the kernel always honours the
// mask, so every line below says aligned=1.
//
// Each map is preceded by a one-page allocation so that a placement which ignores the mask lands
// off-boundary: two consecutive maps cannot both be aligned when a page separates them, so at
// least one line says aligned=0 under such a placement, whatever state the run starts in.
// Results are printed only at the end, so stdio's own allocations come after every map.
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <stdio.h>

#define SEG 0x400000ull
#define MASK (SEG - 1)

struct row { const char *name; kern_return_t kr; mach_vm_address_t a; };

static struct row one(const char *name, mach_vm_address_t hint, vm_prot_t cur, vm_prot_t max) {
    mach_vm_address_t pad = 0;
    mach_vm_allocate(mach_task_self(), &pad, 0x4000, VM_FLAGS_ANYWHERE);
    if (hint == 1) hint = pad; // a hint the kernel cannot honour verbatim: that page is taken
    mach_vm_address_t a = hint;
    // max_protection == VM_PROT_ALL with no object takes libsystem's trap route (-15); any other
    // max falls through to the MIG route (msgh_id 4811). cur == VM_PROT_NONE is a reservation.
    kern_return_t kr = mach_vm_map(mach_task_self(), &a, SEG, MASK, VM_FLAGS_ANYWHERE,
                                   MEMORY_OBJECT_NULL, 0, FALSE, cur, max, VM_INHERIT_DEFAULT);
    return (struct row){ name, kr, a };
}

int main(void) {
    struct row r[5];
    r[0] = one("trap", 0, VM_PROT_DEFAULT, VM_PROT_ALL);
    r[1] = one("mig", 0, VM_PROT_DEFAULT, VM_PROT_DEFAULT);
    r[2] = one("trap-reserve", 0, VM_PROT_NONE, VM_PROT_ALL);
    r[3] = one("mig-reserve", 0, VM_PROT_NONE, VM_PROT_DEFAULT);
    r[4] = one("trap-hinted", 1, VM_PROT_DEFAULT, VM_PROT_ALL);
    for (int i = 0; i < 5; i++)
        printf("%s kr=%d aligned=%d\n", r[i].name, r[i].kr, r[i].kr == 0 && (r[i].a & MASK) == 0);
    return 0;
}
