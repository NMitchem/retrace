// M55 t1 fixture: a Unix stream socket pair as a guest object (spec 2b, 3b-3e). Each mode prints value
// lines and never an absolute descriptor: every guest under retrace holds one more descriptor than
// native (the stray syslog socket), so a line carries a difference or an "is the lowest free" check.
// A descriptor is named by its end, A (sv[0]) or B (sv[1]).
//
//   sockpair_dyn pair        return 0, B - A is 1, A is the lowest free, F_GETFL 0x2, F_GETFD 0
//   sockpair_dyn flow        3 bytes each way, FIONREAD on each end, the reads
//   sockpair_dyn big         nonblocking: a 16384-byte write returns 8192, reads of 1000 sum 8192
//   sockpair_dyn eof         the peer closed with 2 unread: reads 2 then 0, a write is EPIPE (SIGPIPE ignored)
//   sockpair_dyn level       the level-triggered kevent sequence of the spec's 2b
//   sockpair_dyn poll        poll's revents for each state and each requested bit
//   sockpair_dyn select      select's three sets for each state
//   sockpair_dyn dup2console dup2(B, 1) then write(1, "x", 1): the byte reaches A, not the console
//   sockpair_dyn execeof     FD_CLOEXEC on B, then exec itself as `afterexec <A>`: it sees POLLHUP on A
//   sockpair_dyn shapes      each other shape's return and errno (not SOCK_DGRAM)
//   sockpair_dyn dgram       SOCK_DGRAM (a wall under retrace)
//   sockpair_dyn refusals <shutdown|rights|sndbuf|msgx>   one call outside the model, after a good pair
//   sockpair_dyn worker      the pair is made, written and read on a pthread while main joins
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/event.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/time.h>
#include <sys/uio.h>
#include <unistd.h>

static int sv[2];
static char *self;

static void mk(void) {
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) != 0) { printf("socketpair failed errno=%d\n", errno); exit(1); }
}
static void nb(int fd) { fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK); }
static const char *who(int fd) { return fd == sv[0] ? "A" : fd == sv[1] ? "B" : "?"; }

static int mode_pair(void) {
    int probe = dup(0);
    close(probe);
    int rc = socketpair(AF_UNIX, SOCK_STREAM, 0, sv);
    printf("rc=%d d=%d lowest=%d\n", rc, sv[1] - sv[0], sv[0] == probe);
    printf("getfl=%#x getfd=%d\n", fcntl(sv[0], F_GETFL), fcntl(sv[0], F_GETFD));
    printf("getfl=%#x getfd=%d\n", fcntl(sv[1], F_GETFL), fcntl(sv[1], F_GETFD));
    return 0;
}

static int mode_flow(void) {
    mk();
    char buf[16];
    printf("write A %zd\n", write(sv[0], "abc", 3));
    printf("write B %zd\n", write(sv[1], "xyz", 3));
    int n = -1;
    ioctl(sv[0], FIONREAD, &n); printf("fionread A %d\n", n);
    ioctl(sv[1], FIONREAD, &n); printf("fionread B %d\n", n);
    ssize_t r = read(sv[1], buf, sizeof buf); printf("read B %zd %.*s\n", r, (int)r, buf);
    r = read(sv[0], buf, sizeof buf); printf("read A %zd %.*s\n", r, (int)r, buf);
    ioctl(sv[0], FIONREAD, &n); printf("fionread A %d\n", n);
    return 0;
}

static int mode_big(void) {
    mk();
    nb(sv[0]); nb(sv[1]);
    static char big[16384];
    memset(big, 'q', sizeof big);
    printf("write %zd\n", write(sv[0], big, sizeof big));
    ssize_t w = write(sv[0], big, 1);
    printf("write again %zd errno=%d\n", w, errno);
    long sum = 0; int reads = 0; ssize_t r;
    while ((r = read(sv[1], big, 1000)) > 0) { sum += r; reads++; }
    printf("sum=%ld reads=%d last=%zd errno=%d\n", sum, reads, r, errno);
    printf("write after drain %zd\n", write(sv[0], big, 100));
    return 0;
}

static int mode_eof(void) {
    mk();
    signal(SIGPIPE, SIG_IGN);
    printf("write %zd\n", write(sv[0], "ab", 2));
    close(sv[0]);
    char buf[8];
    printf("read %zd\n", read(sv[1], buf, sizeof buf));
    printf("read %zd\n", read(sv[1], buf, sizeof buf));
    ssize_t w = write(sv[1], "x", 1);
    printf("write %zd errno=%d\n", w, errno);
    return 0;
}

static void show(const char *label, int n, struct kevent *ev) {
    printf("%s: n=%d", label, n);
    for (int i = 0; i < n; i++)
        printf(" [%s %s flags=%#x data=%lld]", who((int)ev[i].ident), ev[i].filter == EVFILT_READ ? "read" : "write",
               ev[i].flags, (long long)ev[i].data);
    printf("\n");
}

static int mode_level(void) {
    mk();
    nb(sv[0]);
    int kq = kqueue();
    struct kevent ch, ev[4];
    struct timespec zero = {0, 0};
    EV_SET(&ch, sv[0], EVFILT_READ, EV_ADD, 0, 0, 0);
    show("add read", kevent(kq, &ch, 1, ev, 4, &zero), ev);
    EV_SET(&ch, sv[0], EVFILT_WRITE, EV_ADD, 0, 0, 0);
    show("add write", kevent(kq, &ch, 1, ev, 4, &zero), ev);
    show("poll empty", kevent(kq, NULL, 0, ev, 4, &zero), ev);
    write(sv[1], "x", 1);
    show("poll after 1 byte", kevent(kq, NULL, 0, ev, 4, &zero), ev);
    show("poll again (level)", kevent(kq, NULL, 0, ev, 4, &zero), ev);
    char c;
    read(sv[0], &c, 1);
    show("poll after reading it", kevent(kq, NULL, 0, ev, 4, &zero), ev);
    EV_SET(&ch, sv[0], EVFILT_READ, EV_DELETE, 0, 0, 0);
    show("del read", kevent(kq, &ch, 1, ev, 4, &zero), ev);
    EV_SET(&ch, sv[0], EVFILT_WRITE, EV_DELETE, 0, 0, 0);
    show("del write", kevent(kq, &ch, 1, ev, 4, &zero), ev);
    // The peer closing fires both filters with EV_EOF.
    EV_SET(&ch, sv[0], EVFILT_READ, EV_ADD, 0, 0, 0);
    kevent(kq, &ch, 1, ev, 4, &zero);
    EV_SET(&ch, sv[0], EVFILT_WRITE, EV_ADD, 0, 0, 0);
    kevent(kq, &ch, 1, ev, 4, &zero);
    write(sv[1], "yz", 2);
    close(sv[1]);
    show("peer closed", kevent(kq, NULL, 0, ev, 4, &zero), ev);
    return 0;
}

enum { EMPTY, READABLE, OUTFULL, READABLE_OUTFULL, CLOSED_BYTES, CLOSED_DRAINED, NSTATES };
static const char *state_name[NSTATES] = { "empty", "readable", "outfull", "readable+outfull", "closed+bytes", "closed+drained" };

static void fill(int fd) {
    static char hundred[100];
    nb(fd);
    while (write(fd, hundred, sizeof hundred) > 0) {}
}

// A fresh pair in state `s`, as seen from A (sv[0]).
static void make_state(int s) {
    mk();
    switch (s) {
    case READABLE: write(sv[1], "x", 1); break;
    case OUTFULL: fill(sv[0]); break;
    case READABLE_OUTFULL: write(sv[1], "x", 1); fill(sv[0]); break;
    case CLOSED_BYTES: write(sv[1], "x", 1); close(sv[1]); break;
    case CLOSED_DRAINED: { char c; write(sv[1], "x", 1); read(sv[0], &c, 1); close(sv[1]); break; }
    default: break;
    }
}

static int mode_poll(void) {
    static const short bits[] = { POLLIN, POLLPRI, POLLOUT, POLLRDNORM, POLLWRNORM, POLLRDBAND, POLLWRBAND, 0,
        POLLIN | POLLOUT | POLLPRI | POLLRDNORM | POLLWRNORM | POLLRDBAND | POLLWRBAND };
    for (int s = 0; s < NSTATES; s++) {
        make_state(s);
        printf("%s:", state_name[s]);
        for (unsigned i = 0; i < sizeof bits / sizeof bits[0]; i++) {
            struct pollfd p = { sv[0], bits[i], 0 };
            int n = poll(&p, 1, 0);
            printf(" %#x=%d/%#x", bits[i], n, p.revents);
        }
        printf("\n");
        close(sv[0]);
        if (s < CLOSED_BYTES) close(sv[1]);
    }
    return 0;
}

static int mode_select(void) {
    for (int s = 0; s < NSTATES; s++) {
        make_state(s);
        int a = sv[0];
        int n[4];
        for (int which = 0; which < 4; which++) {
            fd_set r, w, x;
            FD_ZERO(&r); FD_ZERO(&w); FD_ZERO(&x);
            if (which == 0 || which == 3) FD_SET(a, &r);
            if (which == 1 || which == 3) FD_SET(a, &w);
            if (which == 2 || which == 3) FD_SET(a, &x);
            struct timeval zero = {0, 0};
            n[which] = select(a + 1, &r, &w, &x, &zero);
        }
        printf("%s: read=%d write=%d except=%d all=%d\n", state_name[s], n[0], n[1], n[2], n[3]);
        close(sv[0]);
        if (s < CLOSED_BYTES) close(sv[1]);
    }
    return 0;
}

static int mode_dup2console(void) {
    mk();
    nb(sv[0]);
    int out = dup(1);
    dup2(sv[1], 1);
    ssize_t w = write(1, "x", 1);
    char c = '?';
    ssize_t r = read(sv[0], &c, 1);
    dprintf(out, "wrote %zd read %zd byte=%c\n", w, r, c);
    // A second write through the dup, counted too.
    w = write(1, "yz", 2);
    int n = -1;
    ioctl(sv[0], FIONREAD, &n);
    dprintf(out, "wrote %zd fionread %d\n", w, n);
    return 0;
}

static int mode_execeof(void) {
    mk();
    fcntl(sv[1], F_SETFD, FD_CLOEXEC);
    char num[16];
    snprintf(num, sizeof num, "%d", sv[0]);
    printf("exec\n");
    char *args[] = { self, "afterexec", num, NULL };
    execv(self, args);
    printf("exec failed errno=%d\n", errno);
    return 1;
}

static int mode_afterexec(const char *fdstr) {
    struct pollfd p = { atoi(fdstr), POLLIN, 0 };
    int n = poll(&p, 1, 0);
    printf("after exec: poll n=%d hup=%d in=%d\n", n, !!(p.revents & POLLHUP), !!(p.revents & POLLIN));
    return 0;
}

static void shape(int domain, int type, int proto) {
    int v[2] = { -1, -1 };
    int rc = socketpair(domain, type, proto, v);
    int e = rc ? errno : 0;
    printf("domain=%d type=%d proto=%d: rc=%d errno=%d\n", domain, type, proto, rc, e);
    if (rc == 0) { close(v[0]); close(v[1]); }
}

static int mode_shapes(void) {
    shape(AF_UNIX, SOCK_SEQPACKET, 0);
    shape(AF_UNIX, SOCK_RAW, 0);
    shape(AF_UNIX, SOCK_STREAM, 6);
    shape(AF_INET, SOCK_STREAM, 0);
    shape(AF_INET6, SOCK_STREAM, 0);
    shape(99, SOCK_STREAM, 0);
    // An unwritable sv answers EFAULT and consumes no descriptor.
    void *page = mmap(NULL, 16384, PROT_NONE, MAP_ANON | MAP_PRIVATE, -1, 0);
    int probe = dup(0);
    close(probe);
    int rc = socketpair(AF_UNIX, SOCK_STREAM, 0, (int *)page);
    int e = rc ? errno : 0;
    int probe2 = dup(0);
    close(probe2);
    printf("unwritable sv: rc=%d errno=%d lowest_unchanged=%d\n", rc, e, probe == probe2);
    return 0;
}

static int mode_dgram(void) {
    int rc = socketpair(AF_UNIX, SOCK_DGRAM, 0, sv);
    printf("dgram rc=%d\n", rc);
    return 0;
}

// libc's syscall() goes through the indirect trap (syscall 0); sendmsg_x has no wrapper, so issue 481 directly.
// Darwin: carry set means error, with errno in x0; return -errno.
static long raw_syscall4(long n, long a, long b, long c, long d) {
    register long x16 __asm__("x16") = n;
    register long x0 __asm__("x0") = a;
    register long x1 __asm__("x1") = b;
    register long x2 __asm__("x2") = c;
    register long x3 __asm__("x3") = d;
    __asm__ volatile("svc #0x80\n\tb.cc 1f\n\tneg x0, x0\n1:"
                     : "+r"(x0) : "r"(x16), "r"(x1), "r"(x2), "r"(x3) : "memory", "cc");
    return x0;
}

static int mode_refusals(const char *which) {
    mk();
    printf("pair ok\n");
    if (!strcmp(which, "shutdown")) {
        int rc = shutdown(sv[0], SHUT_WR);
        printf("shutdown rc=%d errno=%d\n", rc, rc ? errno : 0);
    } else if (!strcmp(which, "rights")) {
        char data = 'x';
        struct iovec iov = { &data, 1 };
        char ctl[CMSG_SPACE(sizeof(int))];
        memset(ctl, 0, sizeof ctl);
        struct msghdr m;
        memset(&m, 0, sizeof m);
        m.msg_iov = &iov; m.msg_iovlen = 1;
        m.msg_control = ctl; m.msg_controllen = sizeof ctl;
        struct cmsghdr *c = CMSG_FIRSTHDR(&m);
        c->cmsg_level = SOL_SOCKET; c->cmsg_type = SCM_RIGHTS; c->cmsg_len = CMSG_LEN(sizeof(int));
        int fd = 0;
        memcpy(CMSG_DATA(c), &fd, sizeof fd);
        ssize_t r = sendmsg(sv[0], &m, 0);
        printf("sendmsg rights rc=%zd errno=%d\n", r, r < 0 ? errno : 0);
    } else if (!strcmp(which, "sndbuf")) {
        int v = 4096;
        int rc = setsockopt(sv[0], SOL_SOCKET, SO_SNDBUF, &v, sizeof v);
        printf("setsockopt SO_SNDBUF rc=%d errno=%d\n", rc, rc ? errno : 0);
    } else if (!strcmp(which, "msgx")) {
        char msgx[64];
        memset(msgx, 0, sizeof msgx);
        long rc = raw_syscall4(481, sv[0], (long)msgx, 0, 0);
        printf("sendmsg_x rc=%ld errno=%ld\n", rc, rc < 0 ? -rc : 0);
    } else {
        printf("unknown refusal %s\n", which);
        return 2;
    }
    return 0;
}

static void *worker_main(void *arg) {
    (void)arg;
    int probe = dup(0);
    close(probe);
    mk();
    printf("worker pair d=%d lowest=%d\n", sv[1] - sv[0], sv[0] == probe);
    write(sv[0], "hi", 2);
    char buf[4];
    ssize_t r = read(sv[1], buf, sizeof buf);
    printf("worker read %zd %.*s\n", r, (int)r, buf);
    return NULL;
}

static int mode_worker(void) {
    pthread_t t;
    pthread_create(&t, NULL, worker_main, NULL);
    pthread_join(t, NULL);
    printf("main joined\n");
    return 0;
}

int main(int argc, char **argv) {
    self = argv[0];
    setvbuf(stdout, NULL, _IONBF, 0);
    if (argc < 2) { fprintf(stderr, "usage: sockpair_dyn <mode>\n"); return 2; }
    const char *m = argv[1];
    if (!strcmp(m, "afterexec")) return argc > 2 ? mode_afterexec(argv[2]) : 2;
    if (!strcmp(m, "pair")) return mode_pair();
    if (!strcmp(m, "flow")) return mode_flow();
    if (!strcmp(m, "big")) return mode_big();
    if (!strcmp(m, "eof")) return mode_eof();
    if (!strcmp(m, "level")) return mode_level();
    if (!strcmp(m, "poll")) return mode_poll();
    if (!strcmp(m, "select")) return mode_select();
    if (!strcmp(m, "dup2console")) return mode_dup2console();
    if (!strcmp(m, "execeof")) return mode_execeof();
    if (!strcmp(m, "shapes")) return mode_shapes();
    if (!strcmp(m, "dgram")) return mode_dgram();
    if (!strcmp(m, "refusals")) return argc > 2 ? mode_refusals(argv[2]) : 2;
    if (!strcmp(m, "worker")) return mode_worker();
    fprintf(stderr, "unknown mode %s\n", m);
    return 2;
}
