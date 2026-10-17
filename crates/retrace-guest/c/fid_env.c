// M51 (spec §3f): the program's environment, one entry per line as raw bytes in environ's order, then
// its count. envp_e2e runs it natively and under retrace with the same environment and compares.
#include <stdio.h>
#include <string.h>
#include <unistd.h>

extern char **environ;

int main(void) {
    int n = 0;
    for (char **e = environ; *e; e++, n++) {
        write(1, *e, strlen(*e));
        write(1, "\n", 1);
    }
    char b[32];
    int len = snprintf(b, sizeof b, "count=%d\n", n);
    write(1, b, len);
    return 0;
}
