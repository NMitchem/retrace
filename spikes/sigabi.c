// M12 ABI measurement: signal-frame struct geometry on this exact macOS SDK.
#include <stdio.h>
#include <signal.h>
#include <sys/ucontext.h>
#include <stddef.h>

#define SZ(t)      printf("sizeof(%-28s) = %4zu\n", #t, sizeof(t))
#define OFF(t,f)   printf("offsetof(%-22s, %-14s) = %4zu\n", #t, #f, offsetof(t,f))

int main(void) {
    puts("--- sizes ---");
    SZ(struct __darwin_arm_exception_state64);
    SZ(struct __darwin_arm_thread_state64);
    SZ(struct __darwin_arm_neon_state64);
    SZ(struct __darwin_mcontext64);
    SZ(ucontext_t);
    SZ(siginfo_t);
    SZ(stack_t);
    SZ(sigset_t);
    SZ(struct sigaction);
    SZ(struct __sigaction);

    puts("\n--- mcontext64 ---");
    OFF(struct __darwin_mcontext64, __es);
    OFF(struct __darwin_mcontext64, __ss);
    OFF(struct __darwin_mcontext64, __ns);

    puts("\n--- arm_thread_state64 ---");
    OFF(struct __darwin_arm_thread_state64, __x);
    OFF(struct __darwin_arm_thread_state64, __fp);
    OFF(struct __darwin_arm_thread_state64, __lr);
    OFF(struct __darwin_arm_thread_state64, __sp);
    OFF(struct __darwin_arm_thread_state64, __pc);
    OFF(struct __darwin_arm_thread_state64, __cpsr);

    puts("\n--- arm_exception_state64 ---");
    OFF(struct __darwin_arm_exception_state64, __far);
    OFF(struct __darwin_arm_exception_state64, __esr);
    OFF(struct __darwin_arm_exception_state64, __exception);

    puts("\n--- ucontext_t ---");
    OFF(ucontext_t, uc_onstack);
    OFF(ucontext_t, uc_sigmask);
    OFF(ucontext_t, uc_stack);
    OFF(ucontext_t, uc_link);
    OFF(ucontext_t, uc_mcsize);
    OFF(ucontext_t, uc_mcontext);

    puts("\n--- siginfo_t ---");
    OFF(siginfo_t, si_signo);
    OFF(siginfo_t, si_errno);
    OFF(siginfo_t, si_code);
    OFF(siginfo_t, si_pid);
    OFF(siginfo_t, si_uid);
    OFF(siginfo_t, si_status);
    OFF(siginfo_t, si_addr);
    OFF(siginfo_t, si_value);
    OFF(siginfo_t, si_band);

    puts("\n--- stack_t ---");
    OFF(stack_t, ss_sp);
    OFF(stack_t, ss_size);
    OFF(stack_t, ss_flags);

    puts("\n--- struct __sigaction (the 24-byte INPUT struct) ---");
    OFF(struct __sigaction, sa_handler);
    OFF(struct __sigaction, sa_tramp);
    OFF(struct __sigaction, sa_mask);
    OFF(struct __sigaction, sa_flags);

    puts("\n--- si_code values we care about ---");
    printf("SEGV_MAPERR=%d SEGV_ACCERR=%d BUS_ADRALN=%d BUS_ADRERR=%d BUS_OBJERR=%d\n",
           SEGV_MAPERR, SEGV_ACCERR, BUS_ADRALN, BUS_ADRERR, BUS_OBJERR);
    printf("ILL_ILLOPC=%d ILL_ILLTRP=%d FPE_INTDIV=%d TRAP_BRKPT=%d SI_USER=%d\n",
           ILL_ILLOPC, ILL_ILLTRP, FPE_INTDIV, TRAP_BRKPT, SI_USER);
    printf("SA_ONSTACK=%#x SA_SIGINFO=%#x SA_RESTART=%#x SA_NODEFER=%#x SA_RESETHAND=%#x\n",
           SA_ONSTACK, SA_SIGINFO, SA_RESTART, SA_NODEFER, SA_RESETHAND);
    printf("SS_ONSTACK=%#x SS_DISABLE=%#x MINSIGSTKSZ=%d SIGSTKSZ=%d\n",
           SS_ONSTACK, SS_DISABLE, MINSIGSTKSZ, SIGSTKSZ);
    printf("SIGSEGV=%d SIGBUS=%d SIGILL=%d SIGFPE=%d SIGTRAP=%d\n",
           SIGSEGV, SIGBUS, SIGILL, SIGFPE, SIGTRAP);
    return 0;
}
