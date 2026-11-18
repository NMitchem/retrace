// longjmp_dyn: plain setjmp/longjmp and sigsetjmp/siglongjmp. libplatform's longjmp ends with
// sigreturn(NULL, UC_SET_ALT_STACK|UC_RESET_ALT_STACK, 0). Output is words and counts, never addresses.
#include <setjmp.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static jmp_buf jb;
static sigjmp_buf sjb;
static char *alt_lo, *alt_hi;

static void handler(int sig) {
    (void)sig;
    char here;
    int on = &here >= alt_lo && &here < alt_hi;
    printf("handler ran %s the alternate stack\n", on ? "on" : "off");
    siglongjmp(sjb, 1);
}

static void report(const char *when) {
    stack_t o;
    sigaltstack(NULL, &o);
    printf("after %s: SS_ONSTACK %s\n", when, (o.ss_flags & SS_ONSTACK) ? "set" : "clear");
}

int main(int argc, char **argv) {
    const char *m = argc > 1 ? argv[1] : "longjmp";
    if (!strcmp(m, "longjmp")) {
        if (setjmp(jb) == 0) longjmp(jb, 1);
        puts("ok");
    } else if (!strcmp(m, "siglongjmp")) {
        if (sigsetjmp(sjb, 1) == 0) siglongjmp(sjb, 1);
        puts("ok");
    } else if (!strcmp(m, "loop")) {
        int n = 0;
        for (int i = 0; i < 1000; i++)
            if (setjmp(jb) == 0) longjmp(jb, 1); else n++;
        printf("count %d\n", n);
    } else if (!strcmp(m, "handler")) {
        size_t sz = 65536;
        alt_lo = malloc(sz);
        alt_hi = alt_lo + sz;
        stack_t s = { .ss_sp = alt_lo, .ss_size = sz, .ss_flags = 0 };
        sigaltstack(&s, NULL);
        struct sigaction sa;
        memset(&sa, 0, sizeof sa);
        sa.sa_handler = handler;
        sa.sa_flags = SA_ONSTACK;
        sigaction(SIGUSR1, &sa, NULL);
        for (int i = 0; i < 2; i++) {
            if (sigsetjmp(sjb, 1) == 0) raise(SIGUSR1);
            report(i == 0 ? "first jump" : "second jump");
        }
        puts("done");
    } else {
        fprintf(stderr, "unknown mode\n");
        return 2;
    }
    return 0;
}
