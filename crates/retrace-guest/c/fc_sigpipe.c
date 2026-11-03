// M55 t4 first-contact row: a write to a pipe whose reader is closed, with SIGPIPE at its default
// disposition. Natively the process dies with 141 on the write, so the last line is never printed.
#include <stdio.h>
#include <unistd.h>

int main(void) {
    int p[2];
    if (pipe(p) != 0) { puts("pipe failed"); return 1; }
    puts("writing to a pipe with no reader"); fflush(stdout);
    close(p[0]);
    ssize_t r = write(p[1], "x", 1);
    printf("not reached: %zd\n", r);
    return 0;
}
