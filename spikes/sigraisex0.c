// M12 probe: what does the kernel put in the SAVED x0 of a signal frame when the signal was
// RAISED BY THE GUEST ITSELF (kill/__pthread_kill), as opposed to caused by a fault?
//
// Why it matters: retrace's caught-raise arm appends the syscall event and delivers WITHOUT first
// setting the syscall's return value, so the frame captures x0 = the kill's first argument (the
// pid) and sigreturn restores that as the syscall's result. If the real kernel instead delivers
// with x0 already holding the syscall's return (0 on success), a guest whose libc checks
// `if (kill(...) != 0)` after the handler returns takes an error path retrace invented.
//
// Build (no HVF, no entitlement, no codesign needed):
//   clang -O0 -o sigraisex0 sigraisex0.c && ./sigraisex0
#include <stdio.h>
#include <signal.h>
#include <unistd.h>
#include <sys/ucontext.h>

static volatile unsigned long long saved_x0, saved_x1, saved_pc, saved_cpsr;

static void handler(int sig, siginfo_t *si, void *uc_) {
    (void)sig; (void)si;
    ucontext_t *uc = (ucontext_t *)uc_;
    saved_x0 = uc->uc_mcontext->__ss.__x[0];
    saved_x1 = uc->uc_mcontext->__ss.__x[1];
    saved_pc = uc->uc_mcontext->__ss.__pc;
    saved_cpsr = uc->uc_mcontext->__ss.__cpsr;
}

int main(void) {
    struct sigaction sa = {0};
    sa.sa_sigaction = handler;
    sa.sa_flags = SA_SIGINFO;
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGUSR1, &sa, NULL) != 0) { perror("sigaction"); return 1; }

    pid_t me = getpid();
    printf("getpid()          = %d (%#x)\n", me, me);

    // Force the carry flag SET immediately before the syscall. Darwin signals syscall errors via
    // PSTATE.C, and retrace captures the frame's PSTATE from the raw trap-time SPSR_EL1. If the
    // kernel saved the PRE-syscall PSTATE, C comes back set here; if it saves the POST-return
    // PSTATE of the successful kill, C comes back clear. `subs` with equal operands sets C.
    unsigned long long junk;
    __asm__ volatile("subs %0, xzr, xzr" : "=r"(junk) :: "cc");
    int r = kill(me, SIGUSR1);   // delivered synchronously, before kill() returns

    printf("kill() returned   = %d\n", r);
    printf("frame saved x0    = %#llx\n", saved_x0);
    printf("frame saved x1    = %#llx\n", saved_x1);
    printf("frame saved pc    = %#llx\n", saved_pc);
    printf("frame saved cpsr  = %#llx  (C = %llu)\n", saved_cpsr, (saved_cpsr >> 29) & 1);
    printf("\nx0 == pid  -> the kernel delivers with the SYSCALL ARGUMENT still in x0\n");
    printf("x0 == 0    -> the kernel sets the syscall's RETURN VALUE before delivering\n");
    printf("C  == 1    -> the frame carries PRE-syscall PSTATE (a stale error flag)\n");
    printf("C  == 0    -> the frame carries the POST-return PSTATE of the successful call\n");
    return 0;
}
