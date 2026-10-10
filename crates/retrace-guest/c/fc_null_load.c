// M49 first-contact corpus (spec §3b), row F: a load through a pointer to 0x10, the classic NULL
// struct-member read. Natively SIGSEGV (139). stdout is flushed before the fault, so the line is
// part of what native, the recording and both replays must agree on.
#include <stdio.h>

int main(void) {
    volatile long *p = (volatile long *)0x10;
    puts("loading through 0x10");
    fflush(stdout);
    long v = *p;
    printf("survived v=%ld\n", v);
    return 0;
}
