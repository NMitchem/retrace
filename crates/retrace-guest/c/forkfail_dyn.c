// M47 fixture (spec §3f): fork() must fail with EAGAIN. Natively the fixture first lowers its own
// RLIMIT_NPROC to 1, so the kernel refuses the fork as it refuses one past the process limit (t0
// measured this under lldb); under retrace the fork never reaches the kernel (M47 §3e). A child, if
// one is ever created, exits at once without printing.
#include <errno.h>
#include <stdio.h>
#include <sys/resource.h>
#include <unistd.h>

int main(void) {
    struct rlimit rl = { 1, 1 };
    if (setrlimit(RLIMIT_NPROC, &rl) != 0) { perror("setrlimit"); return 1; }
    pid_t p = fork();
    if (p == 0) _exit(0);
    if (p < 0) { printf("fork failed errno=%d\n", errno); return 0; }
    printf("fork succeeded\n");
    return 1;
}
