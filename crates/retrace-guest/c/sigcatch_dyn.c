// M12: a dynamically-linked guest that catches SIGSEGV through Apple's REAL _sigtramp.
//
// Every other M12 guest hand-rolls its own trampoline, which is deliberate — those test retrace's
// entry contract with libc out of the way. This one is the opposite and the only one of its kind:
// libc's sigaction() overwrites sa_tramp with Apple's _sigtramp regardless of what the caller put
// there, so this is the guest that proves the frame retrace builds satisfies the trampoline that
// actually ships.
//
// GARBAGE_VA has bit 46 set, exactly as crashy.c's does (L1 index 0x400, never mapped; < 2^47), so
// the store takes a STAGE-1 translation fault and reaches Stop::Fault, the stop the delivery arm
// consults. (Before M49 a VA below 2^36 took a stage-2 abort instead and killed the recording;
// M49 F2 converts that into the same stop, but this fixture keeps the address it was written for.)
#include <stdio.h>
#include <signal.h>
#include <stdint.h>
#include <sys/ucontext.h>

#define GARBAGE_VA 0x4000DEAD0000UL

static void handler(int sig, siginfo_t *si, void *ucv) {
    ucontext_t *uc = (ucontext_t *)ucv;
    printf("caught sig=%d si_addr=%p\n", sig, si->si_addr);
    fflush(stdout);
    // Step past the faulting store so the guest can continue. This is what proves sigreturn
    // restores MUTATED state through the real trampoline, not merely through ours: without the
    // repair the guest would re-execute the same store and fault forever.
    uc->uc_mcontext->__ss.__pc += 4;
}

int main(void) {
    struct sigaction sa;
    // sigaction(), NOT signal(): signal() sets SA_RESTART and hides the flags, and this gate is
    // about the flags reaching retrace intact.
    sa.sa_sigaction = handler;
    sa.sa_flags = SA_SIGINFO;
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGSEGV, &sa, NULL) != 0) { printf("sigaction failed\n"); return 1; }
    printf("installed\n");
    fflush(stdout);

    *(volatile uint64_t *)GARBAGE_VA = 1;

    printf("resumed\n");
    fflush(stdout);
    return 0;
}
