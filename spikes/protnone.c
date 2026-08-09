// M13 R1. Which signal does Darwin raise for a PROT_NONE access, and with what si_code?
//
// retrace's signal_of_esr maps an AArch64 permission fault (DFSC 0x0c..0x0f) to
// (SIGSEGV, SEGV_ACCERR) -- the Linux answer, and a row NO guest has ever exercised. Darwin's
// ux_exception translates EXC_BAD_ACCESS by code: KERN_INVALID_ADDRESS -> SIGSEGV, everything
// else (including KERN_PROTECTION_FAILURE) -> SIGBUS. libstd's install_main_guard comment says
// "This ensures SIGBUS will be raised on stack overflow." Three sources, two answers.
//
// The UNMAPPED control is not optional: if Darwin raised SIGBUS for that too, M6's crashy_e2e
// classification would be wrong as well, and this would be a much larger finding than M13.
#include <stdio.h>
#include <signal.h>
#include <string.h>
#include <setjmp.h>
#include <sys/mman.h>
#include <unistd.h>

static sigjmp_buf jb;
static volatile sig_atomic_t got_sig, got_code;
static void * volatile got_addr;

static void h(int sig, siginfo_t *si, void *uc) {
    (void)uc;
    got_sig = sig;
    got_code = si->si_code;
    got_addr = si->si_addr;
    siglongjmp(jb, 1);
}

static const char *signame(int s) {
    return s == SIGSEGV ? "SIGSEGV" : s == SIGBUS ? "SIGBUS" : "OTHER";
}

#define TRY(label, stmt) do {                                                      \
    got_sig = 0; got_code = -1; got_addr = (void *)0;                              \
    if (sigsetjmp(jb, 1) == 0) { stmt; printf("%-22s NO FAULT\n", label); }        \
    else printf("%-22s %-8s si_code=%d si_addr=%p\n",                              \
                label, signame(got_sig), (int)got_code, got_addr);                 \
} while (0)

int main(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_sigaction = h;
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGSEGV, &sa, NULL);
    sigaction(SIGBUS,  &sa, NULL);

    long ps = sysconf(_SC_PAGESIZE);
    printf("page size = %ld\n", ps);
    printf("SEGV_MAPERR=%d SEGV_ACCERR=%d BUS_ADRALN=%d BUS_ADRERR=%d BUS_OBJERR=%d\n",
           SEGV_MAPERR, SEGV_ACCERR, BUS_ADRALN, BUS_ADRERR, BUS_OBJERR);

    volatile char *p = mmap(NULL, ps, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
    if (p == MAP_FAILED) { perror("mmap PROT_NONE"); return 1; }
    printf("PROT_NONE page at %p\n", (void *)p);
    TRY("PROT_NONE load",  (void)*p);
    TRY("PROT_NONE store", *p = 1);

    // Control: an unmapped address MUST still be SIGSEGV, or crashy_e2e's premise breaks.
    volatile char *u = (volatile char *)0x4000dead0000UL;
    TRY("unmapped load",   (void)*u);
    TRY("unmapped store",  *u = 1);

    // The other protection-failure flavour: writing a read-only page.
    volatile char *ro = mmap(NULL, ps, PROT_READ, MAP_PRIVATE | MAP_ANON, -1, 0);
    if (ro == MAP_FAILED) { perror("mmap PROT_READ"); return 1; }
    TRY("PROT_READ store", *ro = 1);

    return 0;
}
