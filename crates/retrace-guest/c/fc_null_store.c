// M49 first-contact corpus (spec §3b), row F: a store through a NULL pointer. Natively SIGSEGV
// (139). stdout is flushed before the fault.
#include <stdio.h>

int main(void) {
    volatile long *p = (volatile long *)0;
    puts("storing through NULL");
    fflush(stdout);
    *p = 7;
    puts("survived");
    return 0;
}
