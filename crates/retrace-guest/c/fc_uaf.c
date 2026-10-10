// M49 first-contact corpus (spec §3b): a write through a freed pointer, then a read of what was
// written. Natively this "works" and prints the value, the bug a newcomer would then reverse-debug.
// The write lands 16 bytes into a 64-byte chunk, clear of the allocator's free-list words at its
// start, and the value is copied out before anything else can allocate.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(void) {
    puts("use after free");
    char *p = malloc(64);
    if (!p) return 2;
    strcpy(p + 16, "before");
    free(p);
    strcpy(p + 16, "after-free");
    char out[16];
    memcpy(out, p + 16, sizeof "after-free");
    printf("read back: %s\n", out);
    return 0;
}
