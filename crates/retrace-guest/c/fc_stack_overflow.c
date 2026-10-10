// M49 first-contact corpus (spec §3b), row F: unbounded recursion until the stack runs out.
// Natively SIGSEGV (139) at the main thread's guard page. Each frame keeps a 4 KiB volatile buffer
// and uses its callee's result, so the recursion is neither a loop nor a tail call (and build.rs
// passes no -O). stdout is flushed before the recursion starts.
#include <stdio.h>

static int dive(volatile char *prev, int depth) {
    volatile char buf[4096];
    buf[0] = (char)depth;
    buf[4095] = prev ? prev[0] : 0;
    return dive(buf, depth + 1) + buf[100];
}

int main(void) {
    puts("recursing");
    fflush(stdout);
    return dive(0, 0);
}
