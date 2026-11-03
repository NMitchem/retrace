// M51 (spec §3b): one trap per mode, from M50's static probe
// (docs/sweep-evidence/2026-10-10-m50-static/faults-and-exits/traps.c). usage: fid_traps <mode> [h]
//   brk      `mov x9, #7` then `brk #0xf000` (the mov is what the debugger steps back onto)
//   udf      udf #0
//   udfword  .inst 0xffffffff
//   hlt      hlt #0
//   pcalign  a branch to a pc 2 bytes past an instruction
//   spalign  a load through an SP 4 bytes off its alignment
//   amx      .inst 0x00201220, an AMX instruction: native runs it, HVF does not (a wall, R6)
//   cntfrq   mrs x0, cntfrq_el0: native runs it (x0 = the counter frequency), HVF traps it (EC 0x18, a wall, §3b, t0 T0-a)
// With "h" an SA_SIGINFO handler for SIGTRAP, SIGILL and SIGBUS prints the signal, si_code, where
// si_addr and the frame's FAR point relative to the frame's pc, and the frame's ESR, then _exit(42),
// so a re-executed trap cannot loop. Without it the process dies of the signal. Every line is a raw
// write, so nothing waits in a stdio buffer when the process dies.
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ucontext.h>
#include <unistd.h>

static const char *rel(uint64_t v, uint64_t pc) { return v == pc ? "pc" : v == 0 ? "0" : "other"; }

static void handler(int sig, siginfo_t *si, void *uc_) {
    ucontext_t *uc = uc_;
    uint64_t pc = uc->uc_mcontext->__ss.__pc;
    char b[160];
    int n = snprintf(b, sizeof b, "HANDLER sig=%d si_code=%d si_pid=%s si_addr=%s far=%s esr=%#llx\n", sig, si->si_code,
                     si->si_pid == 0 ? "0" : "set",
                     rel((uint64_t)si->si_addr, pc), rel(uc->uc_mcontext->__es.__far, pc),
                     (unsigned long long)uc->uc_mcontext->__es.__esr);
    write(1, b, n);
    _exit(42);
}

// Global and never inlined, so each trap's pc has a symbol of its own (traps_e2e reads it).
__attribute__((noinline)) void fid_brk(void) { __asm__ volatile("mov x9, #7\n brk #0xf000" ::: "x9"); }
__attribute__((noinline)) void fid_udf(void) { __asm__ volatile("udf #0"); }
__attribute__((noinline)) void fid_udfword(void) { __asm__ volatile(".inst 0xffffffff"); }
__attribute__((noinline)) void fid_hlt(void) { __asm__ volatile("hlt #0"); }
__attribute__((noinline)) void fid_pcalign(void) {
    __asm__ volatile("adr x9, 1f\n add x9, x9, #2\n br x9\n1: nop\n nop" ::: "x9");
}
__attribute__((noinline)) void fid_spalign(void) {
    __asm__ volatile("mov x10, sp\n sub x11, x10, #4\n mov sp, x11\n ldr x12, [sp]\n mov sp, x10"
                     ::: "x10", "x11", "x12", "memory");
}
__attribute__((noinline)) void fid_amx(void) { __asm__ volatile(".inst 0x00201220"); }
__attribute__((noinline)) uint64_t fid_cntfrq(void) { uint64_t v; __asm__ volatile("mrs %0, cntfrq_el0" : "=r"(v)); return v; }

int main(int argc, char **argv) {
    const char *m = argc > 1 ? argv[1] : "";
    if (argc > 2 && !strcmp(argv[2], "h")) {
        struct sigaction sa;
        memset(&sa, 0, sizeof sa);
        sa.sa_sigaction = handler;
        sa.sa_flags = SA_SIGINFO;
        sigaction(SIGTRAP, &sa, 0);
        sigaction(SIGILL, &sa, 0);
        sigaction(SIGBUS, &sa, 0);
    }
    write(1, "start\n", 6);
    if (!strcmp(m, "brk")) fid_brk();
    else if (!strcmp(m, "udf")) fid_udf();
    else if (!strcmp(m, "udfword")) fid_udfword();
    else if (!strcmp(m, "hlt")) fid_hlt();
    else if (!strcmp(m, "pcalign")) fid_pcalign();
    else if (!strcmp(m, "spalign")) fid_spalign();
    else if (!strcmp(m, "amx")) fid_amx();
    else if (!strcmp(m, "cntfrq")) (void)fid_cntfrq();
    else { write(2, "fid_traps: unknown mode\n", 24); return 2; }
    write(1, "survived\n", 9);
    return 0;
}
