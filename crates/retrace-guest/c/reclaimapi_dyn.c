// M59: the deferred-reclamation ring driven by hand through libsystem_kernel's own API, so a flush and a
// resize reclaim entries this program chose, and what they leave mapped can be checked.
//
// A task has one ring and libmalloc normally takes it at startup, so the e2e runs this with
// MallocSpaceEfficient=1 in the environment, which keeps xzone malloc from asking (measured on 27.2: the
// allocate below answers VM_RECLAIM_RESOURCE_SHORTAGE when libmalloc already holds the ring).
//
// Whether a range is still mapped is read through mach_vm_allocate(VM_FLAGS_FIXED) over it: 0 if the
// range is free (the probe is deallocated again), KERN_NO_SPACE (3) if something is still mapped there.
// The SDK doesn't ship <mach/vm_reclaim.h>, so the declarations are copied from xnu-12377's
// osfmk/mach/vm_reclaim.h.
#include <mach/mach.h>
#include <mach/mach_traps.h>
#include <mach/mach_vm.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct mach_vm_reclaim_ring_s *mach_vm_reclaim_ring_t;
typedef uint32_t mach_vm_reclaim_count_t;
typedef uint64_t mach_vm_reclaim_id_t;
typedef uint8_t mach_vm_reclaim_action_t;
typedef uint32_t mach_vm_reclaim_state_t;
typedef kern_return_t mach_vm_reclaim_error_t;
#define VM_RECLAIM_FREE 1
#define VM_RECLAIM_DEALLOCATE 2
#define VM_RECLAIM_ID_NULL UINT64_MAX

mach_vm_reclaim_error_t mach_vm_reclaim_ring_allocate(mach_vm_reclaim_ring_t *, mach_vm_reclaim_count_t, mach_vm_reclaim_count_t);
mach_vm_reclaim_error_t mach_vm_reclaim_ring_resize(mach_vm_reclaim_ring_t, mach_vm_reclaim_count_t);
mach_vm_reclaim_error_t mach_vm_reclaim_ring_capacity(mach_vm_reclaim_ring_t, mach_vm_reclaim_count_t *);
mach_vm_reclaim_count_t mach_vm_reclaim_round_capacity(mach_vm_reclaim_count_t);
mach_vm_reclaim_error_t mach_vm_reclaim_ring_flush(mach_vm_reclaim_ring_t, mach_vm_reclaim_count_t);
mach_vm_reclaim_error_t mach_vm_reclaim_try_enter(mach_vm_reclaim_ring_t, mach_vm_address_t, mach_vm_size_t,
                                                  mach_vm_reclaim_action_t, mach_vm_reclaim_id_t *, bool *);
mach_vm_reclaim_error_t mach_vm_reclaim_try_cancel(mach_vm_reclaim_ring_t, mach_vm_reclaim_id_t, mach_vm_address_t,
                                                   mach_vm_size_t, mach_vm_reclaim_action_t, mach_vm_reclaim_state_t *, bool *);

#define SZ (64 * 1024)

// Breakpoint targets for reclaim_e2e's checkpointed seek.
__attribute__((noinline)) void reclaimapi_ring_ready(void) { __asm__ volatile(""); }
__attribute__((noinline)) void reclaimapi_done(void) { __asm__ volatile(""); }

static int still_mapped(mach_vm_address_t a) {
    mach_vm_address_t probe = a;
    kern_return_t kr = mach_vm_allocate(mach_task_self(), &probe, SZ, VM_FLAGS_FIXED);
    if (kr == KERN_SUCCESS) { mach_vm_deallocate(mach_task_self(), probe, SZ); return 0; }
    return kr == KERN_NO_SPACE ? 1 : -kr;
}

static mach_vm_reclaim_id_t enter(mach_vm_reclaim_ring_t ring, mach_vm_address_t a, mach_vm_reclaim_action_t act) {
    mach_vm_reclaim_id_t id = VM_RECLAIM_ID_NULL;
    bool upd = false;
    kern_return_t kr = mach_vm_reclaim_try_enter(ring, a, SZ, act, &id, &upd);
    if (kr != KERN_SUCCESS) printf("try_enter kr %#x\n", kr);
    return id;
}

int main(void) {
    uint64_t bytes = 1, deadline = 1;
    printf("accounting with no ring kr %d\n",
           mach_vm_reclaim_update_kernel_accounting_trap(mach_task_self(), &bytes, &deadline));
    mach_vm_reclaim_ring_t ring = NULL;
    mach_vm_reclaim_count_t cap = mach_vm_reclaim_round_capacity(1), now = 0;
    kern_return_t kr = mach_vm_reclaim_ring_allocate(&ring, cap, mach_vm_reclaim_round_capacity(8 * cap));
    printf("allocate kr %#x, capacity %u\n", kr, cap);
    if (kr != KERN_SUCCESS) return 1;
    mach_vm_reclaim_ring_t again = NULL;
    printf("second allocate kr %#x\n", mach_vm_reclaim_ring_allocate(&again, cap, cap));
    reclaimapi_ring_ready();

    mach_vm_address_t r[4];
    for (int i = 0; i < 4; i++) {
        r[i] = 0;
        kr = mach_vm_allocate(mach_task_self(), &r[i], SZ, VM_FLAGS_ANYWHERE);
        if (kr != KERN_SUCCESS) { printf("mach_vm_allocate kr %d\n", kr); return 1; }
        memset((void *)r[i], 0x5a, SZ);
    }
    // A and C deallocate, B frees, D deallocates but is cancelled before anything reclaims it.
    mach_vm_reclaim_id_t a = enter(ring, r[0], VM_RECLAIM_DEALLOCATE);
    mach_vm_reclaim_id_t b = enter(ring, r[1], VM_RECLAIM_FREE);
    mach_vm_reclaim_id_t c = enter(ring, r[2], VM_RECLAIM_DEALLOCATE);
    mach_vm_reclaim_id_t d = enter(ring, r[3], VM_RECLAIM_DEALLOCATE);
    printf("ids %llu %llu %llu %llu\n", a, b, c, d);
    mach_vm_reclaim_state_t st = 0;
    bool upd = false;
    kr = mach_vm_reclaim_try_cancel(ring, d, r[3], SZ, VM_RECLAIM_DEALLOCATE, &st, &upd);
    printf("cancel D kr %#x state %u\n", kr, st);

    kr = mach_vm_reclaim_ring_flush(ring, 2);
    printf("flush 2 kr %#x: mapped A %d B %d C %d D %d\n", kr,
           still_mapped(r[0]), still_mapped(r[1]), still_mapped(r[2]), still_mapped(r[3]));
    kr = mach_vm_reclaim_try_cancel(ring, a, r[0], SZ, VM_RECLAIM_DEALLOCATE, &st, &upd);
    printf("cancel A kr %#x state %u\n", kr, st);
    kr = mach_vm_reclaim_try_cancel(ring, b, r[1], SZ, VM_RECLAIM_FREE, &st, &upd);
    printf("cancel B kr %#x state %u\n", kr, st);

    kr = mach_vm_reclaim_ring_resize(ring, mach_vm_reclaim_round_capacity(2 * cap));
    mach_vm_reclaim_ring_capacity(ring, &now);
    printf("resize kr %#x capacity %u: mapped C %d D %d\n", kr, now, still_mapped(r[2]), still_mapped(r[3]));
    kr = mach_vm_reclaim_try_cancel(ring, c, r[2], SZ, VM_RECLAIM_DEALLOCATE, &st, &upd);
    printf("cancel C kr %#x state %u\n", kr, st);

    // Fill the ring with FREE entries over D (still mapped), then one more: a full ring hands back no id.
    mach_vm_reclaim_count_t n = 0;
    while (enter(ring, r[3], VM_RECLAIM_FREE) != VM_RECLAIM_ID_NULL) n++;
    printf("full after %u entries\n", n);
    kr = mach_vm_reclaim_ring_flush(ring, now);
    printf("flush all kr %#x, entering again gives id %s\n", kr,
           enter(ring, r[3], VM_RECLAIM_FREE) == VM_RECLAIM_ID_NULL ? "none" : "one");
    reclaimapi_done();
    return 0;
}
