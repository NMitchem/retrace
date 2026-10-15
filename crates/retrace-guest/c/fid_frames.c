// M51 (spec §3d, T3-b): a fault whose handler's frame may not be pushable, on the main thread or a
// child, from M50's static probes (overflow_small.c, badalt.c, badalt2.c).
// usage: fid_frames <main|child> <overflow|null> <how>
//   overflow     a recursion of 200-byte frames into the stack's guard (4 KiB frames hang natively)
//   null         a store to 0x10
//   none         no handler
//   h            an SA_SIGINFO handler for SIGSEGV and SIGBUS, no alternate stack
//   ha-rw        the handler with SA_ONSTACK, on a 64 KiB alternate stack the faulting thread set
//   ha-none      the same, with the stack made PROT_NONE before sigaltstack
//   ha-unmapped  the same, with the stack munmapped after sigaltstack
//   s            a handler installed with signal() (M51 Task 6)
// The handler prints the signal, si_code (-1 for a signal() handler) and which thread ran it, then
// _exit(42)s. A child's stack is 512 KiB, as in the static probe. Every line is a raw write.
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

static const char *fault, *how;
static pthread_t main_thread;

static void say(int fd, const char *s) { write(fd, s, strlen(s)); }

static void report(int sig, int code) {
    char b[80];
    int n = snprintf(b, sizeof b, "HANDLER sig=%d si_code=%d on=%s\n", sig, code,
                     pthread_equal(pthread_self(), main_thread) ? "main" : "child");
    write(1, b, n);
    _exit(42);
}
static void handler(int sig, siginfo_t *si, void *uc) { (void)uc; report(sig, si->si_code); }
static void plain(int sig) { report(sig, -1); }

__attribute__((noinline)) static int recurse(volatile char *prev, int depth) {
    volatile char big[200];
    big[0] = (char)depth;
    big[199] = prev ? prev[0] : 0;
    return recurse(big, depth + 1) + big[100];
}

static void *body(void *a) {
    (void)a;
    if (!strncmp(how, "ha-", 3)) {
        size_t sz = 65536;
        char *p = mmap(NULL, sz, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
        if (!strcmp(how, "ha-none")) mprotect(p, sz, PROT_NONE);
        stack_t ss = { .ss_sp = p, .ss_size = sz, .ss_flags = 0 };
        if (sigaltstack(&ss, NULL)) { say(2, "sigaltstack failed\n"); _exit(3); }
        if (!strcmp(how, "ha-unmapped")) munmap(p, sz);
    }
    if (!strcmp(fault, "overflow")) recurse(NULL, 0);
    else *(volatile int *)0x10 = 1;
    return NULL;
}

int main(int argc, char **argv) {
    if (argc < 4) { say(2, "usage: fid_frames <main|child> <overflow|null> <how>\n"); return 2; }
    fault = argv[2];
    how = argv[3];
    main_thread = pthread_self();
    if (!strcmp(how, "s")) {
        signal(SIGSEGV, plain);
        signal(SIGBUS, plain);
    } else if (strcmp(how, "none")) {
        struct sigaction sa;
        memset(&sa, 0, sizeof sa);
        sa.sa_sigaction = handler;
        sa.sa_flags = SA_SIGINFO | (how[1] == 'a' ? SA_ONSTACK : 0);
        sigaction(SIGSEGV, &sa, 0);
        sigaction(SIGBUS, &sa, 0);
    }
    say(1, "start\n");
    if (!strcmp(argv[1], "child")) {
        pthread_t t;
        pthread_attr_t at;
        pthread_attr_init(&at);
        pthread_attr_setstacksize(&at, 512 * 1024);
        pthread_create(&t, &at, body, NULL);
        pthread_join(t, NULL);
    } else {
        body(NULL);
    }
    say(1, "survived\n");
    return 0;
}
