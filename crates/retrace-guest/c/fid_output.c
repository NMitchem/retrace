// M51 (spec §3e, §3c): the output modes output_e2e drives, picked by argv[1]. Every console write is
// a raw write(2) to fd 1 or 2, so the bytes arrive in the order of the calls, natively and under
// retrace, with no stdio buffer between them.
//   split     out1, err1, out2, err2, alternating streams; exit 0
//   stall     one line, then a spin nothing ends but a kill
//   many      20000 lines, far more than a pipe holds; exit 7
//   warn      an anonymous PROT_EXEC map, which printed `[retrace warn]` before M51; exit 0
//   crash     one line, then a NULL store in fid_store_null (SIGSEGV)
//   crashlib  one line, then strlen(NULL), which faults inside the shared cache (SIGSEGV)
//   abort     one line, then abort() (SIGABRT)
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

static void say(int fd, const char *s) { write(fd, s, strlen(s)); }

// Global and never inlined, so the crash's pc has a symbol of its own for the closing line.
__attribute__((noinline)) void fid_store_null(void) { *(volatile int *)0 = 1; }

int main(int argc, char **argv) {
    const char *m = argc > 1 ? argv[1] : "";
    if (!strcmp(m, "split")) {
        say(1, "out1\n"); say(2, "err1\n"); say(1, "out2\n"); say(2, "err2\n");
        return 0;
    }
    if (!strcmp(m, "stall")) {
        say(1, "first\n");
        for (volatile unsigned long i = 0; ; i++) {}
    }
    if (!strcmp(m, "many")) {
        char b[32];
        for (int i = 0; i < 20000; i++) { int n = snprintf(b, sizeof b, "line %d\n", i); write(1, b, n); }
        return 7;
    }
    if (!strcmp(m, "warn")) {
        void *p = mmap(0, 0x4000, PROT_READ | PROT_EXEC, MAP_ANON | MAP_PRIVATE, -1, 0);
        say(1, p == MAP_FAILED ? "map failed\n" : "mapped\n");
        return 0;
    }
    if (!strcmp(m, "crash")) { say(1, "before\n"); fid_store_null(); }
    if (!strcmp(m, "crashlib")) {
        say(1, "before\n");
        const char *volatile s = 0;
        return (int)strlen(s);
    }
    if (!strcmp(m, "abort")) { say(1, "before\n"); abort(); }
    say(2, "fid_output: unknown mode\n");
    return 2;
}
