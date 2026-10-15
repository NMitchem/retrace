// M51 first-contact corpus (R8): __builtin_trap(), the brk a C program's own code reaches, so tier 1
// guards the trap class without swiftc. Natively SIGTRAP (133) after the line it flushed.
#include <stdio.h>

int main(void) {
    puts("about to trap");
    fflush(stdout);
    __builtin_trap();
}
