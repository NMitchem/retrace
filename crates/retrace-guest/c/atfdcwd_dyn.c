// M38. The AT_FDCWD fixture: a relative fstatat through the sentinel succeeds. Before M38 every
// real guest's AT_FDCWD (0xfffffffe in x0 — a 32-bit -2) was looked up as a descriptor and got
// EBADF; /bin/ls printed "ls: .: Bad file descriptor" on both runs and the sweep called it a PASS.
//
// Expected stdout (record == replay, bit for bit):   ok\n
#include <stdio.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <errno.h>

int main(void) {
    struct stat st;
    if (fstatat(AT_FDCWD, ".", &st, 0) == 0) printf("ok\n");
    else printf("errno=%d\n", errno);
    return 0;
}
