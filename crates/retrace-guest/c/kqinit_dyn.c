// M45. The kqinit fixture: libdispatch's `_dispatch_kq_init` call (`event_kevent.c:689-699`),
// issued by hand through `svc #0x80`, byte for byte what M44/M45 t0 M1 measured automationmodetool
// issue. x0 = 0xffffffff (int -1, no kqueue), one change-list entry, no event list, and
// x7 = KEVENT_FLAG_WORKQ | KEVENT_FLAG_IMMEDIATE. It uses inline `svc`, not syscall(3):
// libSystem's syscall() goes through the indirect SYS_syscall (0), so retrace would see syscall 0
// (M45 R4).
//
// Every mode first brings up the process's workqueue with one dispatch_async, because that is the
// context libdispatch makes this call in: M44 t0 M1 measured it right after the workqueue pair.
//
// argv[1] selects the mode:
//   (none)    the measured call from a stack entry
//   straddle  the measured call from an entry that straddles a 16 KiB page boundary
//   flags     the entry's flags are EV_ADD|EV_CLEAR|EV_ENABLE (0x25): retrace must refuse it
//   badptr    the change list is 1 << 47, past the 47-bit guest VA, so no page maps it: retrace
//             must refuse it
//
// Stdout in the two call modes: `kqinit rc=0 carry=0` (M45 t0 M3 measured it natively).
#include <dispatch/dispatch.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// xnu bsd/sys/event_private.h:115-125. Not in the SDK.
struct kevent_qos_s {
    uint64_t ident;
    int16_t  filter;
    uint16_t flags;
    int32_t  qos;
    uint64_t udata;
    uint32_t fflags;
    uint32_t xflags;
    int64_t  data;
    uint64_t ext[4];
};
_Static_assert(sizeof(struct kevent_qos_s) == 72, "xnu event_private.h: 72 bytes, no padding");

// Every register the call carries is an in/out operand, so the compiler assumes none survives the
// `svc`. The carry flag is the kernel's error bit, read straight after the trap.
static uint64_t kevent_qos_workq(uint64_t changelist, uint32_t *carry) {
    register uint64_t x0 __asm__("x0") = 0xffffffffu;  // (int)-1 in w0, upper half zero, as measured
    register uint64_t x1 __asm__("x1") = changelist;
    register uint64_t x2 __asm__("x2") = 1;            // nchanges
    register uint64_t x3 __asm__("x3") = 0;            // eventlist
    register uint64_t x4 __asm__("x4") = 0;            // nevents
    register uint64_t x5 __asm__("x5") = 0;            // data_out
    register uint64_t x6 __asm__("x6") = 0;            // data_available
    register uint64_t x7 __asm__("x7") = 0x21;         // KEVENT_FLAG_WORKQ | KEVENT_FLAG_IMMEDIATE
    register uint64_t x16 __asm__("x16") = 374;        // SYS_kevent_qos
    uint32_t c;
    __asm__ volatile("svc #0x80\n\tcset %w[c], cs"
        : "+r"(x0), "+r"(x1), "+r"(x2), "+r"(x3), "+r"(x4), "+r"(x5), "+r"(x6), "+r"(x7), "+r"(x16),
          [c] "=r"(c)
        :
        : "memory", "cc");
    *carry = c;
    return x0;
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "";

    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    dispatch_async(dispatch_get_global_queue(0, 0), ^{ dispatch_semaphore_signal(sem); });
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);

    struct kevent_qos_s kev = {
        .ident = 1,
        .filter = -10,             // EVFILT_USER
        .flags = 0x0001 | 0x0020,  // EV_ADD | EV_CLEAR
        .qos = 0x02000000,         // _PTHREAD_PRIORITY_EVENT_MANAGER_FLAG
        .udata = ~(uint64_t)0x7,   // DISPATCH_WLH_MANAGER
    };
    uint64_t changelist = (uint64_t)&kev;
    if (strcmp(mode, "flags") == 0) {
        kev.flags |= 0x0004;       // EV_ENABLE
    } else if (strcmp(mode, "badptr") == 0) {
        changelist = 1ull << 47;  // past the 47-bit guest VA: translates to nothing (M45 ruling P1)
    } else if (strcmp(mode, "straddle") == 0) {
        void *buf = NULL;
        if (posix_memalign(&buf, 16384, 32768) != 0) return 2;
        // 16384 - 40: 40 bytes in the first page, 32 in the second, and 8-byte aligned.
        char *at = (char *)buf + 16384 - 40;
        memcpy(at, &kev, sizeof kev);
        changelist = (uint64_t)at;
    } else if (mode[0] != '\0') {
        fprintf(stderr, "kqinit_dyn: unknown mode %s\n", mode);
        return 2;
    }
    uint32_t carry = 0;
    uint64_t rc = kevent_qos_workq(changelist, &carry);
    printf("kqinit rc=%llu carry=%u\n", (unsigned long long)rc, carry);
    return 0;
}
