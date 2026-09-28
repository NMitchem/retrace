// M38. The F_DUPFD fixture: the new descriptor honours the GUEST minimum (exactly 10 — nothing
// that low is open), writes through it reach the file, F_SETFD on it is a plain int argument,
// and F_DUPFD on stdout is a console alias (mirrored, not forwarded). argv[1] is a file path the
// test owns.
//
// M44 A3 adds F_DUPFD_CLOEXEC on the FILE descriptor `f` (an Open slot bound to the host dup, not
// the console alias below): the forwarded F_GETFD on the new descriptor must read back FD_CLOEXEC.
//
// Expected stdout (record == replay, bit for bit):   n=10\nsetfd=0\ncloexec=1\nalias\n
// Expected file after record:                        dupfd\n
#include <stdio.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>

int main(int argc, char **argv) {
    if (argc < 2) return 2;
    int f = open(argv[1], O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (f < 0) return 3;
    int n = fcntl(f, F_DUPFD, 10);
    printf("n=%d\n", n);
    if (n < 0) return 4;
    if (write(n, "dupfd\n", 6) != 6) return 5;
    printf("setfd=%d\n", fcntl(n, F_SETFD, FD_CLOEXEC));
    /* M44 A3: F_DUPFD_CLOEXEC on a FILE descriptor (an Open slot bound to the host dup), then
       F_GETFD. Native reads 1; before M44 a forwarded F_GETFD read the host dup's clear flag, 0. */
    int c = fcntl(f, F_DUPFD_CLOEXEC, 14);
    if (c < 0) return 7;
    printf("cloexec=%d\n", fcntl(c, F_GETFD));
    close(c);
    fflush(stdout);                       /* stdio is a pipe under retrace: flush BEFORE the raw write, or "alias" lands first */
    int a = fcntl(1, F_DUPFD_CLOEXEC, 12);
    if (a < 0) return 6;
    write(a, "alias\n", 6);
    close(a); close(n); close(f);
    return 0;
}
