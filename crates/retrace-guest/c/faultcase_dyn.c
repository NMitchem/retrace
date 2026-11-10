// M49 Task 3: one fault per argv-selected case, natively and under retrace. The cases are
// docs/sweep-evidence/2026-10-06-m49-static/faults.c's, plus four of the plan's.
//
//   faultcase_dyn <case> [h|a]
//   storeat <addr> as a third argument: a store of 7 to that address (M57: retrace's own table pages)
//   cases: null10 null0st unmapped garbage call0 low8000 low1g tsd
//          thread1g (low1g's load on a second thread, main blocked in pthread_join)
//          jump1g   (a call through a function pointer equal to 0x40000000)
//          jumpheap (a call into a zeroed read-write heap page: a permission fault natively too)
//          jumpgone (a call into the case's own PROT_EXEC file mapping after munmap)
//          deeprec  (unbounded recursion: a C stack overflow)
//   h: an SA_SIGINFO handler for SIGSEGV and SIGBUS that prints one line and _exit(42)s
//   a: the same handler, run on a 64 KiB alternate stack (SA_ONSTACK)
//
// The handler's line is identical natively and under retrace wherever retrace takes native's
// fault. It prints the signal, si_code and si_addr, the ESR's exception class and WnR bit, and
// which thread ran it. si_addr is printed relative to the case's own mapping for `unmapped` and
// `jumpgone` (ASLR moves native's) and withheld for `deeprec` (each stack's guard page is where that
// stack puts it). The line never prints the DFSC, whose level retrace does not reproduce (M49 R2),
// nor the pc, which ASLR moves natively.
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

static uintptr_t g_base;   // the case's own mapping: si_addr is printed relative to it
static int g_hide_addr;    // deeprec: si_addr is wherever this stack's guard page is

static void handler(int sig, siginfo_t *si, void *uc_) {
    ucontext_t *uc = uc_;
    unsigned esr = uc->uc_mcontext->__es.__esr;
    uintptr_t a = (uintptr_t)si->si_addr;
    char buf[160];
    const char *on = pthread_main_np() ? "main" : "child";
    int n;
    if (g_hide_addr)
        n = snprintf(buf, sizeof buf, "HANDLER sig=%d si_code=%d si_addr=guard on=%s\n", sig, si->si_code, on);
    else if (g_base)
        n = snprintf(buf, sizeof buf, "HANDLER sig=%d si_code=%d si_addr=base+%#lx ec=%#x wnr=%u on=%s\n",
            sig, si->si_code, (unsigned long)(a - g_base), esr >> 26, (esr >> 6) & 1, on);
    else
        n = snprintf(buf, sizeof buf, "HANDLER sig=%d si_code=%d si_addr=%#lx ec=%#x wnr=%u on=%s\n",
            sig, si->si_code, (unsigned long)a, esr >> 26, (esr >> 6) & 1, on);
    write(1, buf, n);
    _exit(42);
}

static void *child(void *arg) {
    (void)arg;
    return (void *)*(volatile long *)0x40000000UL;   // low1g's load, on this thread
}

static int rec(volatile char *prev, int n) {
    volatile char buf[4096];
    if (n < 0) return 0;   // never taken; without it clang warns that every path recurses
    buf[0] = (char)n;
    buf[4095] = prev ? prev[0] : 0;
    return rec(buf, n + 1) + buf[100];
}

// Map one 16 KiB page of this program's own file read+exec, so `jumpgone` has a code page to lose.
static char *map_own_text(const char *self) {
    int fd = open(self, O_RDONLY);
    if (fd < 0) return MAP_FAILED;
    char *m = mmap(0, 0x4000, PROT_READ | PROT_EXEC, MAP_PRIVATE, fd, 0);
    close(fd);
    return m;
}

int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: faultcase_dyn <case> [h|a]\n"); return 2; }
    const char *c = argv[1];
    if (argc > 2 && (argv[2][0] == 'h' || argv[2][0] == 'a')) {
        struct sigaction sa;
        memset(&sa, 0, sizeof sa);
        sa.sa_sigaction = handler;
        sa.sa_flags = SA_SIGINFO;
        if (argv[2][0] == 'a') {
            stack_t ss = { .ss_sp = malloc(1 << 16), .ss_size = 1 << 16, .ss_flags = 0 };
            if (!ss.ss_sp || sigaltstack(&ss, 0) != 0) { puts("sigaltstack failed"); return 2; }
            sa.sa_flags |= SA_ONSTACK;
        }
        sigaction(SIGSEGV, &sa, 0);
        sigaction(SIGBUS, &sa, 0);
    }
    printf("case %s\n", c);
    fflush(stdout);
    volatile long v = 0;
    if (!strcmp(c, "null10")) v = *(volatile long *)0x10;
    else if (!strcmp(c, "null0st")) *(volatile long *)0x0 = 7;
    else if (!strcmp(c, "unmapped")) {
        char *m = mmap(0, 0x4000, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
        if (m == MAP_FAILED) { puts("mmap failed"); return 2; }
        m[0] = 1;
        g_base = (uintptr_t)m;
        puts("mapped");
        fflush(stdout);
        munmap(m, 0x4000);
        v = *(volatile long *)(m + 8);
    }
    else if (!strcmp(c, "garbage")) v = *(volatile long *)0x4000DEAD0000UL;
    else if (!strcmp(c, "call0")) { void (*volatile f)(void) = 0; f(); }
    else if (!strcmp(c, "low8000")) *(volatile long *)0x8000 = 7;
    else if (!strcmp(c, "storeat")) *(volatile long *)strtoul(argv[3], 0, 0) = 7;   // M57: faultcase_dyn storeat h <addr>
    else if (!strcmp(c, "low1g")) v = *(volatile long *)0x40000000UL;
    else if (!strcmp(c, "tsd")) v = *(volatile long *)0x30000;
    else if (!strcmp(c, "thread1g")) {
        pthread_t t;
        if (pthread_create(&t, 0, child, 0) != 0) { puts("pthread_create failed"); return 2; }
        pthread_join(t, 0);
    }
    else if (!strcmp(c, "jump1g")) { void (*volatile f)(void) = (void (*)(void))0x40000000UL; f(); }
    else if (!strcmp(c, "jumpheap")) {
        char *m = mmap(0, 0x4000, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
        if (m == MAP_FAILED) { puts("mmap failed"); return 2; }
        memset(m, 0, 0x4000);
        void (*volatile f)(void) = (void (*)(void))m;
        f();
    }
    else if (!strcmp(c, "jumpgone")) {
        char *m = map_own_text(argv[0]);
        if (m == MAP_FAILED) { puts("mmap failed"); return 2; }
        g_base = (uintptr_t)m;
        puts("mapped");
        fflush(stdout);
        munmap(m, 0x4000);
        void (*volatile f)(void) = (void (*)(void))(m + 0x10);
        f();
    }
    else if (!strcmp(c, "deeprec")) { g_hide_addr = 1; v = rec(0, 0); }
    else { fprintf(stderr, "unknown case %s\n", c); return 2; }
    printf("survived v=%ld\n", (long)v);
    return 0;
}
