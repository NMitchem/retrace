// t0: `perl -e 'alarm N; exec @ARGV'` without perl. perl is a SIP-protected platform binary, so
// dyld purges DYLD_* from its environment and DYLD_INSERT_LIBRARIES never reaches the child. The
// alarm survives execvp, exactly as perl's does (exit 142 = SIGALRM).
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: alarmexec <secs> <cmd> [args...]\n"); return 2; }
    alarm((unsigned)atoi(argv[1]));
    execvp(argv[2], argv + 2);
    perror("execvp");
    return 127;
}
