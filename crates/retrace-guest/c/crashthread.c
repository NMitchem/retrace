// M18 fast-follow: the missing intersection — a guest that is THREADED and FATAL.
//
// Every crashing guest in the tree (crashy, segvy, the asm micro-guests) is single-threaded, and
// every threaded guest (threadrust, watchthread, sigthread, sigblocked, dispatch_dyn) exits
// cleanly. So `Event::Crash`'s thread tag has never been recorded as anything but main's, and the
// oracle's `verify_thread` call on that arm has never fired with a live second thread to catch —
// the hole CLAUDE.md has named since M16. This guest is the smallest thing that closes it.
//
// C rather than Rust, deliberately. A full-std Rust guest installs libstd's own SIGSEGV handler,
// so the fault would route through SignalDelivery -> sigreturn -> re-fault before reaching Crash
// (that is exactly what segv_rust_e2e exists to assert). Here there is no handler, so the fault is
// a plain Stop::Fault whose disposition is not a handler and it lands on the Crash path directly —
// the distinction CLAUDE.md draws between a raised signal and a hardware fault.
//
// THE SCHEDULE IS THE POINT, and it is a consequence of the cooperative scheduler rather than of
// source order: the box switches only when a thread blocks or exits, so main runs uninterrupted
// through pthread_create and does not yield until it BLOCKS in pthread_join's __ulock_wait. Only
// then does the child run. The child therefore holds the vCPU when it faults, and the Crash
// landmark is tagged with the CHILD — a nonzero tag, which is the case no recording has produced.
//
// Both threads write before the fault, and both writes are load-bearing rather than decorative:
// the retag test needs at least two DISTINCT live thread ids in the trace to retag between, and a
// thread that issues no syscall contributes no id.
#include <pthread.h>
#include <unistd.h>

// Same constant as crashy.c and asm/crash.s: bit 46 set (L1 index 0x400, never mapped; < 2^47), so
// this is a stage-1 EL0 data abort with FAR == GARBAGE_VA rather than anything the demand-pager or
// the reservation-commit path could mistake for work of its own.
#define GARBAGE_VA 0x4000DEAD0000UL

static void *child(void *arg) {
    (void)arg;
    write(1, "child\n", 6);
    *(volatile long *)GARBAGE_VA = 42;  /* the fault, ON THE CHILD */
    return 0;                           /* unreached */
}

int main(void) {
    write(1, "main\n", 5);
    pthread_t t;
    pthread_create(&t, 0, child, 0);
    pthread_join(t, 0);                 /* main blocks here; the child then runs and dies */
    write(1, "joined\n", 7);            /* unreached */
    return 0;                           /* unreached */
}
