// M53 t4: poll, select and pselect over guest pipes (spec §3e; t0 §M7, probes/pollprobe.c). Each
// line is determined by the call alone, so native and retrace print the same, and pollsel_e2e
// compares the two. The pipe is 65,536 bytes when full and a write end is writable with 512 bytes of
// room (t0 M7).
//
// argv[1] selects the mode:
//   states    poll's pipe states: M7's spot rows, then its requested-bit matrix (9 bits x 4 states).
//   nval      a closed descriptor and fd = -1 in one array.
//   sleep     poll(NULL, 0, 100), timed with mach_absolute_time.       rc=0 waited_at_least=yes
//   cross     a writer thread writes after 50 ms; main polls with an infinite timeout.
//   timeout   a 30 ms timeout, nothing written.                           rc=0 revents=0x0
//   select    the same states through select, and its argument errors.
//   pselect   cross's shape through pselect with a NULL mask.             rc=1 r=1
//   pollfile  poll on a regular file: refused under retrace (not native-compared).
//   pselmask  pselect with a signal mask: refused under retrace (not native-compared).
//   pollclose a thread closes the read end main is polling: refused under retrace (not native-compared).
#include <errno.h>
#include <fcntl.h>
#include <mach/mach_time.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/select.h>
#include <sys/time.h>
#include <unistd.h>

#define EV (POLLIN | POLLOUT | POLLPRI | POLLERR | POLLHUP | POLLNVAL | POLLRDNORM | POLLWRNORM)

static void P(const char *name, struct pollfd *f, int n, int to) {
    for (int i = 0; i < n; i++) f[i].revents = 0x7fff; // poison: poll must overwrite it
    errno = 0;
    int rc = poll(f, n, to);
    int e = errno;
    printf("poll %s rc=%d errno=%d revents=", name, rc, rc < 0 ? e : 0);
    for (int i = 0; i < n; i++) printf("%s0x%x", i ? "," : "", (unsigned)(unsigned short)f[i].revents);
    printf("\n");
}

static void fill(int wfd) {
    char blk[4096];
    memset(blk, 'z', sizeof blk);
    fcntl(wfd, F_SETFL, O_NONBLOCK);
    long total = 0;
    for (;;) {
        long n = write(wfd, blk, sizeof blk);
        if (n < 0) break;
        total += n;
    }
    printf("filled %ld bytes, then errno=%d\n", total, errno);
}

static void drain(int rfd, int n) {
    char buf[4096];
    while (n > 0) {
        int k = n < (int)sizeof buf ? n : (int)sizeof buf;
        if (read(rfd, buf, k) != k) break;
        n -= k;
    }
}

// The matrix's four states (t0 M7): which fd to poll, with the pipe set up.
static int state_fd(int st, int p[2]) {
    pipe(p);
    switch (st) {
    case 0: write(p[1], "x", 1); return p[0];        // read end, 1 byte
    case 1: close(p[1]); return p[0];                 // read end, empty, writer closed
    case 2: return p[1];                              // write end, room
    default: close(p[0]); return p[1];                // write end, reader closed
    }
}

static int mode_states(void) {
    int p[2];
    struct pollfd f[3];
    char buf[8];
    const char *sn[] = {"read end 1 byte", "read end empty, writer closed", "write end room", "write end, reader closed"};
    pipe(p);
    f[0] = (struct pollfd){p[0], POLLIN, 0};
    P("read end, empty", f, 1, 0);
    write(p[1], "abc", 3);
    P("read end, 3 bytes, IN", f, 1, 0);
    f[0].events = POLLIN | POLLOUT;
    P("read end, 3 bytes, IN|OUT", f, 1, 0);
    f[0].events = 0;
    P("read end, 3 bytes, events 0", f, 1, 0);
    f[0].events = EV;
    P("read end, 3 bytes, every bit", f, 1, 0);
    f[0].events = POLLIN;
    close(p[1]);
    P("read end, pending, writer closed", f, 1, 0);
    read(p[0], buf, 8);
    P("read end, drained, writer closed", f, 1, 0);
    f[0].events = 0;
    P("read end, writer closed, events 0", f, 1, 0);
    close(p[0]);

    pipe(p);
    f[0] = (struct pollfd){p[1], POLLOUT, 0};
    P("write end, room", f, 1, 0);
    f[0].events = POLLIN;
    P("write end, IN", f, 1, 0);
    f[0].events = POLLOUT;
    fill(p[1]);
    P("write end, full", f, 1, 0);
    drain(p[0], 1);
    P("write end, 1 byte free", f, 1, 0);
    drain(p[0], 511);
    P("write end, 512 bytes free", f, 1, 0);
    drain(p[0], 3584);
    P("write end, 4096 bytes free", f, 1, 0);
    close(p[0]);
    P("write end, reader closed, OUT", f, 1, 0);
    f[0].events = POLLIN;
    P("write end, reader closed, IN", f, 1, 0);
    f[0].events = 0;
    P("write end, reader closed, events 0", f, 1, 0);
    close(p[1]);

    f[0] = (struct pollfd){p[1], POLLIN, 0};
    P("closed fd", f, 1, 0);
    f[0] = (struct pollfd){-1, POLLIN, 0};
    P("fd -1", f, 1, 0);
    f[0] = (struct pollfd){-5, POLLIN, 0x1234};
    P("fd -5", f, 1, 0);
    pipe(p);
    write(p[1], "a", 1);
    struct pollfd g[3] = {{-1, POLLIN, 0}, {p[0], POLLIN, 0}, {9999, POLLIN, 0}};
    P("mixed", g, 3, 0);
    close(p[0]);
    close(p[1]);

    short bits[] = {POLLIN, POLLPRI, POLLOUT, POLLRDNORM, POLLRDBAND, POLLWRNORM, POLLWRBAND, POLLERR, POLLHUP};
    const char *bn[] = {"IN", "PRI", "OUT", "RDNORM", "RDBAND", "WRNORM", "WRBAND", "ERR", "HUP"};
    for (int st = 0; st < 4; st++) {
        for (int b = 0; b < 9; b++) {
            int q[2];
            char name[96];
            struct pollfd m = {state_fd(st, q), bits[b], 0};
            snprintf(name, sizeof name, "matrix %s %s", sn[st], bn[b]);
            P(name, &m, 1, 0);
            close(q[0]);
            close(q[1]);
        }
    }
    return 0;
}

static int mode_nval(void) {
    int p[2];
    pipe(p);
    int dead = p[1];
    close(p[0]);
    close(p[1]);
    struct pollfd f[2] = {{-1, POLLIN, 0}, {dead, POLLIN, 0}};
    P("closed and negative", f, 2, 0);
    return 0;
}

static int mode_sleep(void) {
    mach_timebase_info_data_t tb;
    mach_timebase_info(&tb);
    uint64_t a = mach_absolute_time();
    int rc = poll(NULL, 0, 100);
    uint64_t b = mach_absolute_time();
    uint64_t ns = (b - a) * tb.numer / tb.denom;
    printf("rc=%d waited_at_least=%s\n", rc, ns >= 100000000ull ? "yes" : "no");
    return 0;
}

static int g_w;
static void *writer(void *a) {
    usleep(50000);
    write(g_w, "x", 1);
    return NULL;
}

// Closes the descriptor g_w names after 50 ms (the write end of a watched pipe, or the watched end).
static void *closer(void *a) {
    usleep(50000);
    close(g_w);
    return NULL;
}

static int mode_cross(int sel) {
    int p[2];
    pipe(p);
    g_w = p[1];
    pthread_t t;
    pthread_create(&t, NULL, writer, NULL);
    if (sel) {
        fd_set r;
        FD_ZERO(&r);
        FD_SET(p[0], &r);
        int rc = pselect(p[0] + 1, &r, NULL, NULL, NULL, NULL);
        printf("rc=%d r=%d\n", rc, FD_ISSET(p[0], &r) != 0);
    } else {
        struct pollfd f = {p[0], POLLIN, 0};
        int rc = poll(&f, 1, -1);
        printf("rc=%d revents=0x%x\n", rc, (unsigned)(unsigned short)f.revents);
    }
    pthread_join(t, NULL);
    // Ruling T4-closepolled (b): closing the OTHER end of a watched pipe is a pipe change that wakes
    // the poller: the read end is ready (end of file), with POLLHUP beside POLLIN.
    int q[2];
    pipe(q);
    g_w = q[1];
    pthread_create(&t, NULL, closer, NULL);
    if (sel) {
        fd_set r;
        FD_ZERO(&r);
        FD_SET(q[0], &r);
        int rc = pselect(q[0] + 1, &r, NULL, NULL, NULL, NULL);
        printf("close: rc=%d r=%d\n", rc, FD_ISSET(q[0], &r) != 0);
        // Ruling T4-timeout: a timespec's nanoseconds past 1e9 are EINVAL, even with a ready descriptor.
        FD_ZERO(&r);
        FD_SET(q[0], &r);
        struct timespec bad = {0, 1000000001};
        errno = 0;
        rc = pselect(q[0] + 1, &r, NULL, NULL, &bad, NULL);
        printf("bad timespec: rc=%d errno=%d\n", rc, rc < 0 ? errno : 0);
    } else {
        struct pollfd f = {q[0], POLLIN, 0};
        int rc = poll(&f, 1, -1);
        printf("close: rc=%d revents=0x%x\n", rc, (unsigned)(unsigned short)f.revents);
    }
    pthread_join(t, NULL);
    return 0;
}

static int mode_timeout(void) {
    int p[2];
    pipe(p);
    struct pollfd f = {p[0], POLLIN, 0x1234};
    int rc = poll(&f, 1, 30);
    printf("rc=%d revents=0x%x\n", rc, (unsigned)(unsigned short)f.revents);
    return 0;
}

static void S(const char *name, int nfds, int rfd, int wfd, int efd, struct timeval *tv) {
    fd_set r, w, x;
    FD_ZERO(&r); FD_ZERO(&w); FD_ZERO(&x);
    if (rfd >= 0) FD_SET(rfd, &r);
    if (wfd >= 0) FD_SET(wfd, &w);
    if (efd >= 0) FD_SET(efd, &x);
    errno = 0;
    int rc = select(nfds, rfd >= 0 ? &r : NULL, wfd >= 0 ? &w : NULL, efd >= 0 ? &x : NULL, tv);
    int e = errno;
    // The bits by FD_ISSET, never the words: a descriptor's number differs between native and the guest.
    printf("select %s rc=%d errno=%d r=%d w=%d x=%d\n", name, rc, rc < 0 ? e : 0,
           rfd >= 0 && FD_ISSET(rfd, &r), wfd >= 0 && FD_ISSET(wfd, &w), efd >= 0 && FD_ISSET(efd, &x));
}

static int mode_select(void) {
    int p[2];
    char buf[8];
    struct timeval zero = {0, 0};
    pipe(p);
    S("read end, empty", p[1] + 1, p[0], -1, -1, &zero);
    write(p[1], "abc", 3);
    S("read end, 3 bytes", p[1] + 1, p[0], -1, -1, &zero);
    S("write end, room", p[1] + 1, -1, p[1], -1, &zero);
    S("read end data, except set", p[1] + 1, p[0], -1, p[0], &zero);
    close(p[1]);
    S("read end, pending, writer closed", p[0] + 1, p[0], -1, -1, &zero);
    read(p[0], buf, 8);
    S("read end, drained, writer closed", p[0] + 1, p[0], -1, -1, &zero);
    close(p[0]);

    pipe(p);
    fill(p[1]);
    S("write end, full", p[1] + 1, -1, p[1], -1, &zero);
    close(p[0]);
    S("write end, reader closed", p[1] + 1, -1, p[1], -1, &zero);
    int dead = p[1];
    close(p[1]);
    S("closed fd in the read set", dead + 1, dead, -1, -1, &zero);
    S("nfds 0", 0, -1, -1, -1, &zero);
    struct timeval t80 = {0, 80000};
    S("nfds 0, 80 ms", 0, -1, -1, -1, &t80);
    printf("timeval after: %ld.%06d\n", (long)t80.tv_sec, (int)t80.tv_usec);
    S("nfds -1", -1, -1, -1, -1, &zero);
    S("nfds 1025", 1025, -1, -1, -1, &zero);
    struct timeval neg = {-1, 0};
    S("timeout -1 s", 0, -1, -1, -1, &neg);
    // Ruling T4-timeout: a timeval's seconds past 100,000,000 are EINVAL even with a ready descriptor.
    pipe(p);
    write(p[1], "x", 1);
    struct timeval big = {100000001, 0};
    S("seconds 100000001, ready", p[0] + 1, p[0], -1, -1, &big);
    struct timeval usec = {0, 1000000};
    S("usec 1000000, ready", p[0] + 1, p[0], -1, -1, &usec);
    return 0;
}

static int mode_pollfile(void) {
    int fd = open("/etc/hosts", O_RDONLY);
    struct pollfd f = {fd, POLLIN, 0};
    int rc = poll(&f, 1, 0);
    printf("rc=%d revents=0x%x\n", rc, (unsigned)(unsigned short)f.revents);
    return 0;
}

static int mode_pselmask(void) {
    int p[2];
    pipe(p);
    fd_set r;
    FD_ZERO(&r);
    FD_SET(p[0], &r);
    sigset_t m;
    sigemptyset(&m);
    struct timespec zero = {0, 0};
    int rc = pselect(p[0] + 1, &r, NULL, NULL, &zero, &m);
    printf("rc=%d\n", rc);
    return 0;
}

static int mode_pollclose(void) {
    int p[2];
    pipe(p);
    g_w = p[0];
    pthread_t t;
    pthread_create(&t, NULL, closer, NULL);
    struct pollfd f = {p[0], POLLIN, 0};
    int rc = poll(&f, 1, -1);
    printf("rc=%d\n", rc);
    pthread_join(t, NULL);
    return 0;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    if (argc < 2) { fprintf(stderr, "usage: pollsel_dyn <mode>\n"); return 2; }
    const char *m = argv[1];
    if (!strcmp(m, "states")) return mode_states();
    if (!strcmp(m, "nval")) return mode_nval();
    if (!strcmp(m, "sleep")) return mode_sleep();
    if (!strcmp(m, "cross")) return mode_cross(0);
    if (!strcmp(m, "timeout")) return mode_timeout();
    if (!strcmp(m, "select")) return mode_select();
    if (!strcmp(m, "pselect")) return mode_cross(1);
    if (!strcmp(m, "pollfile")) return mode_pollfile();
    if (!strcmp(m, "pselmask")) return mode_pselmask();
    if (!strcmp(m, "pollclose")) { printf("pollsel: pollclose\n"); return mode_pollclose(); }
    fprintf(stderr, "pollsel_dyn: unknown mode %s\n", m);
    return 2;
}
