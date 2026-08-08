// M12 signal-delivery spike: what registers does the kernel enter `sa_tramp` with?
//
// libc's sigaction() overwrites sa_tramp with its own _sigtramp, so the only way to see the
// kernel's entry contract is to install a trampoline of our own through the RAW __sigaction
// syscall and dump the registers on arrival. The frame base is then read back out of x3/x4.
#include <stdio.h>
#include <signal.h>
#include <unistd.h>
#include <stdint.h>
extern int __sigaction(int, void *, void *);
struct sa_probe { void *h; void *tr; uint32_t m; uint32_t f; };
uint64_t regs[10];
void report(void);
__attribute__((naked)) void my_tramp(void) {
    __asm__ volatile(
        "adrp x9, _regs@PAGE\n add x9, x9, _regs@PAGEOFF\n"
        "stp x0, x1, [x9, #0]\n stp x2, x3, [x9, #16]\n"
        "stp x4, x5, [x9, #32]\n stp x6, x7, [x9, #48]\n"
        "mov x10, sp\n str x10, [x9, #64]\n b _report\n");
}
void handler(int s, siginfo_t *i, void *u) { (void)s;(void)i;(void)u; }
void report(void) {
    char b[400];
    int n = snprintf(b, sizeof b,
      "TRAMP x0=%#llx x1=%#llx x2=%#llx x3=%#llx x4=%#llx x5=%#llx x6=%#llx x7=%#llx sp=%#llx\n",
      regs[0],regs[1],regs[2],regs[3],regs[4],regs[5],regs[6],regs[7],regs[8]);
    write(2,b,n);
    siginfo_t *si = (siginfo_t*)regs[3];
    ucontext_t *uc = (ucontext_t*)regs[4];
    n = snprintf(b, sizeof b, "siginfo: signo=%d code=%d addr=%p | uc: onstack=%d mask=%#x mcsize=%zu mctx=%p\n",
      si->si_signo, si->si_code, si->si_addr, uc->uc_onstack, uc->uc_sigmask, uc->uc_mcsize, (void*)uc->uc_mcontext);
    write(2,b,n);
    n = snprintf(b, sizeof b, "mctx: far=%#llx esr=%#x exc=%#x pc=%#llx sp=%#llx cpsr=%#x | deltas: si-sp=%lld uc-sp=%lld mc-uc=%lld\n",
      uc->uc_mcontext->__es.__far, uc->uc_mcontext->__es.__esr, uc->uc_mcontext->__es.__exception,
      uc->uc_mcontext->__ss.__pc, uc->uc_mcontext->__ss.__sp, uc->uc_mcontext->__ss.__cpsr,
      (long long)(regs[3]-regs[8]), (long long)(regs[4]-regs[8]), (long long)((uint64_t)uc->uc_mcontext-regs[4]));
    write(2,b,n);
    _exit(0);
}
int main(void) {
    struct sa_probe act = { (void*)handler, (void*)my_tramp, 0, SA_SIGINFO };
    fprintf(stderr, "handler=%p tramp=%p\n", (void*)handler, (void*)my_tramp); fflush(stderr);
    if (__sigaction(SIGSEGV, &act, 0) != 0) { perror("__sigaction"); return 1; }
    *(volatile uint64_t*)0xdead0000 = 1;
    return 0;
}
