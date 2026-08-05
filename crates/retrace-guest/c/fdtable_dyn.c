// M10. The fd-table semantics fixture.
//
// Before M10 the guest's descriptors WERE retrace's: forward_and_diff issues a raw svc in retrace's
// own process, so this program's first open() came back as 17 on the measured host — not because it
// had opened seventeen things, but because RETRACE holds 0-16 open. After M10 the number is a
// function of this program's own open/close sequence.
//
// It prints RELATIONSHIPS rather than absolute descriptor numbers, deliberately. The absolute value
// depends on how many fds libSystem happens to hold when main() runs, and that differs between a
// native process and a guest under retrace: measured, libsystem opens a socket before main under
// retrace (there is no real notifyd/bootstrap to talk to) and does not natively, so the first open
// is 4 here and 3 natively. That difference is environmental and says nothing about whether the fd
// table is correct — while these invariants say exactly that, and hold in both worlds:
//
//   low      the descriptor is the GUEST's own small number, not a host descriptor (>= 16)
//   dupnext  dup() takes the next lowest free slot
//   ebadf    reading a closed fd fails with EBADF, rather than reaching retrace's descriptor
//   reuse    a fresh open takes the just-closed slot back (POSIX lowest-not-currently-open)
#include <stdio.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>

int main(void) {
    int a = open("/dev/null", O_RDONLY);
    int d = dup(a);
    close(a);

    char buf[1];
    int r = read(a, buf, sizeof buf);       // must fail: a is closed
    int e_errno = errno;
    int dr = (int)read(d, buf, sizeof buf); // the alias survives; /dev/null is immediate EOF
    int e = open("/dev/null", O_RDONLY);    // must take a's slot back

    printf("low=%d\n", a >= 3 && a < 16);
    printf("dupnext=%d\n", d == a + 1);
    printf("ebadf=%d\n", r == -1 && e_errno == EBADF);
    printf("dupread=%d\n", dr == 0);
    printf("reuse=%d\n", e == a);

    close(d);
    close(e);
    fflush(stdout);
    return 0;
}
