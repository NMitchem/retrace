// M54 t3. The image the exec fixtures replace themselves with. It prints what it was given, so a test
// compares the recording with a native run line by line:
//   target argc=<n>
//   argv[i]=<hex>            for i >= 1 (an argument that starts "pid:" prints `pid` instead: the
//                            pid is the one thing that differs between runs)
//   X=<hex of getenv("X") or "unset">
//   pid_same=<yes|no>        only when argv[1] starts "pid:" (the caller's pid, passed through exec)
//   fd <n> <file|fifo|other> for each open descriptor 3..15
//   pipe=<hex>               a non-blocking read of fd 5, when fd 5 is a FIFO
// Hex keeps a byte string that is not UTF-8 (the `bytes` mode's "\xff\xfe") exact in the comparison.
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>


// Under retrace a guest starts with a descriptor a native process does not have: dyld's /dev/dtracehelper,
// opened FD_CLOEXEC at the lowest free number, and each new image's dyld opens it again. Exec closes every
// FD_CLOEXEC descriptor, so none can be inherited and closing the ones found here changes nothing natively; it
// lets the descriptor numbers and the listing compare with native. Exec itself is the subject of this fixture.
static void drop_stray(void) {
    for (int fd = 3; fd < 16; fd++) { int fl = fcntl(fd, F_GETFD); if (fl >= 0 && (fl & FD_CLOEXEC)) close(fd); }
}

static void hex(const char *s, size_t n) { for (size_t i = 0; i < n; i++) printf("%02x", (unsigned char)s[i]); }

int main(int argc, char **argv) {
    drop_stray();
    printf("target argc=%d\n", argc);
    for (int i = 1; i < argc; i++) {
        if (strncmp(argv[i], "pid:", 4) == 0) { printf("argv[%d]=pid\n", i); continue; }
        printf("argv[%d]=", i); hex(argv[i], strlen(argv[i])); printf("\n");
    }
    const char *x = getenv("X");
    printf("X="); if (x) hex(x, strlen(x)); else printf("unset"); printf("\n");
    if (argc > 1 && strncmp(argv[1], "pid:", 4) == 0)
        printf("pid_same=%s\n", atoi(argv[1] + 4) == (int)getpid() ? "yes" : "no");
    int fifo5 = 0;
    for (int fd = 3; fd < 16; fd++) {
        struct stat st;
        if (fstat(fd, &st) != 0) continue;
        const char *k = S_ISFIFO(st.st_mode) ? "fifo" : S_ISREG(st.st_mode) ? "file" : "other";
        printf("fd %d %s\n", fd, k);
        if (fd == 5 && S_ISFIFO(st.st_mode)) fifo5 = 1;
    }
    if (fifo5) {
        char buf[64];
        fcntl(5, F_SETFL, O_NONBLOCK);
        ssize_t n = read(5, buf, sizeof buf);
        printf("pipe="); if (n > 0) hex(buf, (size_t)n); printf("\n");
    }
    return 0;
}
