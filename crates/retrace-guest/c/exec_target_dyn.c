// M54 t3. The image the exec fixtures replace themselves with. It prints what it was given, so a test
// compares the recording with a native run line by line:
//   target argc=<n>
//   argv[i]=<hex>            for i >= 1 (an argument that starts "pid:" prints `pid` instead: the
//                            pid is the one thing that differs between runs)
//   apple0=<hex of apple[0]>  (t5: `executable_path=<the interpreter or path as exec'd>`)
//   X=<hex of getenv("X") or "unset">
//   pid_same=<yes|no>        only when argv[1] starts "pid:" (the caller's pid, passed through exec)
//   fd <n> <file|fifo|other> for each open descriptor 16..39
//   pipe=<hex>               a non-blocking read of fd 20, when fd 20 is a FIFO
// Hex keeps a byte string that is not UTF-8 (the `bytes` mode's "\xff\xfe") exact in the comparison.
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>


// Under retrace a guest starts with a descriptor a native process does not have, and so does every new image:
// libsystem's `socket(AF_UNIX, SOCK_DGRAM)` takes the lowest free number (3), gets `fcntl(F_SETFD, FD_CLOEXEC)`,
// `connect`s to /var/run/syslog and `sendto`s once. A native run has no such socket: its first `open` returns 3,
// where under retrace it returns 4. That is a divergence retrace has (ledgered separately as F1), described here
// and not diagnosed. So nothing below checks a descriptor the stray could take: the stage-1 fixture places every descriptor it
// tests at 20 or above, and this target closes nothing before it looks.

static void hex(const char *s, size_t n) { for (size_t i = 0; i < n; i++) printf("%02x", (unsigned char)s[i]); }

int main(int argc, char **argv, char **envp, char **apple) {
    printf("target argc=%d\n", argc);
    for (int i = 1; i < argc; i++) {
        if (strncmp(argv[i], "pid:", 4) == 0) { printf("argv[%d]=pid\n", i); continue; }
        printf("argv[%d]=", i); hex(argv[i], strlen(argv[i])); printf("\n");
    }
    if (apple && apple[0]) { printf("apple0="); hex(apple[0], strlen(apple[0])); printf("\n"); }
    const char *x = getenv("X");
    printf("X="); if (x) hex(x, strlen(x)); else printf("unset"); printf("\n");
    if (argc > 1 && strncmp(argv[1], "pid:", 4) == 0)
        printf("pid_same=%s\n", atoi(argv[1] + 4) == (int)getpid() ? "yes" : "no");
    int fifo20 = 0;
    for (int fd = 16; fd < 40; fd++) {   // above the stray's number
        struct stat st;
        if (fstat(fd, &st) != 0) continue;
        const char *k = S_ISFIFO(st.st_mode) ? "fifo" : S_ISREG(st.st_mode) ? "file" : "other";
        printf("fd %d %s\n", fd, k);
        if (fd == 20 && S_ISFIFO(st.st_mode)) fifo20 = 1;
    }
    if (fifo20) {
        char buf[64];
        fcntl(20, F_SETFL, O_NONBLOCK);
        ssize_t n = read(20, buf, sizeof buf);
        printf("pipe="); if (n > 0) hex(buf, (size_t)n); printf("\n");
    }
    return 0;
}
