// M49 first-contact corpus (spec §3b), row F: a call through a NULL function pointer. Natively
// SIGSEGV (139), an instruction abort at pc 0. Today retrace reports a stage-1 permission fault,
// which is SIGBUS (faults-retrace.txt): §4's `fc_null_call` guard is the trace's signal, not the
// exit code. stdout is flushed before the fault.
#include <stdio.h>

int main(void) {
    void (*volatile f)(void) = 0;
    puts("calling through NULL");
    fflush(stdout);
    f();
    puts("survived");
    return 0;
}
