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
//   sockpair_dyn clearsock   which calls activate EV_CLEAR knotes on an end (M55 t2, p0/sockprobe2.c)
//   sockpair_dyn clearedges  EV_CLEAR read and write edges (p0/sockprobe.c)
//   sockpair_dyn clearpipe   EV_CLEAR on a pipe's two ends (p0/sockprobe.c)
//   sockpair_dyn receipt     mio's EV_RECEIPT register, then deletes, a bad descriptor and nevents 0
//   sockpair_dyn receiptcorner  T0-receipt: the room rule at nevents 0..3, bad one first and third, and EV_CLEAR|EV_ONESHOT
//   sockpair_dyn errnoreceipt   errors without EV_RECEIPT, and receipts mixed with a plain add
//   sockpair_dyn knoteclose  a knote on an end closed with a dup kept, then the peer's last close
//
// The t2 modes print an event as `[delta filter flags data u=udata]`, the ident relative to the pair's
// first end (or `#N` for a constant 900 or more, a descriptor that is not open).
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

// ---- M55 t2: EV_CLEAR, EV_RECEIPT and error events ----

static int base;
static struct timespec zero0 = {0, 0};

static void ev1(const struct kevent *e) {
    if (e->ident >= 900) printf(" [#%lu", (unsigned long)e->ident);
    else printf(" [%+ld", (long)e->ident - base);
    printf(" %s flags=%#x data=%lld u=%lu]", e->filter == EVFILT_READ ? "read" : "write", e->flags,
           (long long)e->data, (unsigned long)e->udata);
}
// A call's result: `n` events, or -1 and `err`. On -1 the first `stored` slots are printed too.
static void sh(const char *label, int n, int err, const struct kevent *ev, int stored) {
    printf("%s: n=%d", label, n);
    if (n < 0) printf(" errno=%d", err);
    for (int i = 0; i < (n < 0 ? stored : n); i++) ev1(&ev[i]);
    printf("\n");
}
static void poll0(int kq, const char *label) {
    struct kevent ev[4];
    int n = kevent(kq, NULL, 0, ev, 4, &zero0), e = errno;
    sh(label, n, e, ev, 0);
}
static void add2(int kq, int fd, unsigned short extra) {
    struct kevent c[2];
    EV_SET(&c[0], fd, EVFILT_READ, EV_ADD | extra, 0, 0, 0);
    EV_SET(&c[1], fd, EVFILT_WRITE, EV_ADD | extra, 0, 0, 0);
    kevent(kq, c, 2, NULL, 0, NULL);
}
static void fillfd(int fd) {
    static char k[1024];
    while (write(fd, k, sizeof k) > 0) {}
}

static int mode_clearsock(void) {
    mk(); nb(sv[0]); nb(sv[1]); base = sv[0];
    int kq = kqueue();
    add2(kq, sv[0], EV_CLEAR);
    char c;
    poll0(kq, "registered");
    write(sv[0], "a", 1); poll0(kq, "A writes 1 byte");
    write(sv[1], "b", 1); poll0(kq, "B writes 1 byte to A");
    read(sv[0], &c, 1); poll0(kq, "A reads it");
    read(sv[1], &c, 1); poll0(kq, "B reads A's byte");
    int d = dup(sv[1]); close(d); poll0(kq, "dup+close of B (not the last)");
    close(sv[1]); poll0(kq, "last close of B");
    return 0;
}

static int mode_clearedges(void) {
    mk(); nb(sv[0]); nb(sv[1]); base = sv[0];
    int kq = kqueue();
    add2(kq, sv[0], EV_CLEAR);
    poll0(kq, "registered");
    char c, b[16];
    static char big[8192];
    write(sv[1], "bb", 2); poll0(kq, "after 2 bytes");
    poll0(kq, "again, 2 unread");
    write(sv[1], "c", 1); poll0(kq, "after 1 more byte");
    read(sv[0], &c, 1); poll0(kq, "after reading 1");
    read(sv[0], b, sizeof b); poll0(kq, "after draining");
    fillfd(sv[0]); poll0(kq, "after fill");
    read(sv[1], big, 1); poll0(kq, "peer read 1");
    read(sv[1], big, 1023); poll0(kq, "peer read 1023 more");
    read(sv[1], big, 2048); poll0(kq, "peer read 2048 more");
    read(sv[1], big, sizeof big); poll0(kq, "peer read the rest");
    while (read(sv[1], big, sizeof big) > 0) {}
    poll0(kq, "peer drained");
    return 0;
}

static int mode_clearpipe(void) {
    int p[2]; char b[16];
    static char big[8192];
    pipe(p); nb(p[0]); nb(p[1]); base = p[0];
    int kq = kqueue();
    struct kevent c, ev[2];
    EV_SET(&c, p[0], EVFILT_READ, EV_ADD | EV_CLEAR, 0, 0, 0);
    int n = kevent(kq, &c, 1, ev, 2, &zero0), e = errno; sh("add read", n, e, ev, 0);
    EV_SET(&c, p[1], EVFILT_WRITE, EV_ADD | EV_CLEAR, 0, 0, 0);
    n = kevent(kq, &c, 1, ev, 2, &zero0); e = errno; sh("add write", n, e, ev, 0);
    poll0(kq, "empty");
    poll0(kq, "again");
    write(p[1], "ab", 2); poll0(kq, "after 2 bytes");
    poll0(kq, "again");
    write(p[1], "c", 1); poll0(kq, "after 1 more");
    read(p[0], b, 1); poll0(kq, "after reading 1");
    fillfd(p[1]); poll0(kq, "after fill");
    read(p[0], big, 512); poll0(kq, "after reading 512");
    while (read(p[0], big, sizeof big) > 0) {}
    poll0(kq, "drained");
    close(p[1]); poll0(kq, "write end closed");
    return 0;
}

static int mode_receipt(void) {
    mk(); nb(sv[0]); nb(sv[1]); base = sv[0];
    int kq = kqueue();
    struct kevent ch[3], ev[3];
    EV_SET(&ch[0], sv[0], EVFILT_READ, EV_ADD | EV_CLEAR | EV_RECEIPT, 0, 0, (void *)7);
    EV_SET(&ch[1], sv[0], EVFILT_WRITE, EV_ADD | EV_CLEAR | EV_RECEIPT, 0, 0, (void *)8);
    int n = kevent(kq, ch, 2, ch, 2, NULL), e = errno;
    sh("register read+write, one array, NULL timeout", n, e, ch, 0);
    poll0(kq, "poll after the receipts");
    poll0(kq, "poll again");
    EV_SET(&ch[0], sv[0], EVFILT_READ, EV_DELETE | EV_RECEIPT, 0, 0, 0);
    EV_SET(&ch[1], 999, EVFILT_READ, EV_DELETE | EV_RECEIPT, 0, 0, 0);
    EV_SET(&ch[2], 998, EVFILT_READ, EV_ADD | EV_RECEIPT, 0, 0, 0);
    n = kevent(kq, ch, 3, ev, 3, &zero0); e = errno;
    sh("delete registered, delete 999, add 998", n, e, ev, 0);
    EV_SET(&ch[0], sv[0], EVFILT_READ, EV_ADD | EV_CLEAR | EV_RECEIPT, 0, 0, 0);
    n = kevent(kq, ch, 1, ev, 1, &zero0); e = errno;
    sh("re-add read", n, e, ev, 0);
    EV_SET(&ch[0], sv[0], EVFILT_READ, EV_ADD | EV_RECEIPT, 0, 0, 0);
    n = kevent(kq, ch, 1, ev, 0, &zero0); e = errno;
    sh("receipt with nevents 0", n, e, ev, 0);
    poll0(kq, "poll after it");
    return 0;
}

// T0-receipt: the room rule. Each case registers two sockets' read filters and one descriptor that is not
// open, then makes both sockets readable to see which changes applied.
static void rcase(int badfirst, int nevents, int receipt, int del) {
    int a[2], b[2];
    socketpair(AF_UNIX, SOCK_STREAM, 0, a);
    socketpair(AF_UNIX, SOCK_STREAM, 0, b);
    base = a[0];
    int kq = kqueue();
    struct kevent ch[3], out[3], ev[4];
    int fds[3] = {a[0], b[0], 999};
    unsigned short f = receipt ? EV_RECEIPT : 0;
    for (int i = 0; i < 3; i++) {
        int fd = badfirst ? fds[(i + 2) % 3] : fds[i];
        unsigned short fl = (fd == 999 && del) ? EV_DELETE : EV_ADD;
        EV_SET(&ch[i], fd, EVFILT_READ, fl | f, 0, 0, (void *)(unsigned long)(i + 1));
    }
    memset(out, 0, sizeof out);
    errno = 0;
    int n = kevent(kq, ch, 3, out, nevents, NULL), e = errno;
    int st = 0;
    while (st < 3 && out[st].ident) st++;
    char tag[96];
    snprintf(tag, sizeof tag, "%s%s bad %s nevents=%d", receipt ? "receipt" : "plain", del ? " delete" : "",
             badfirst ? "first" : "third", nevents);
    sh(tag, n, e, out, st);
    write(a[1], "x", 1); write(b[1], "y", 1);
    n = kevent(kq, NULL, 0, ev, 4, &zero0); e = errno;
    sh("  later trigger", n, e, ev, 0);
    close(kq);
    close(a[0]); close(a[1]); close(b[0]); close(b[1]);
}

static int mode_receiptcorner(void) {
    for (int badfirst = 0; badfirst < 2; badfirst++)
        for (int ne = 1; ne <= 3; ne++) rcase(badfirst, ne, 1, 0);
    for (int r = 0; r < 2; r++)
        for (int ne = 0; ne <= 1; ne++) { rcase(1, ne, r, 0); rcase(1, ne, r, 1); }
    // EV_ADD|EV_CLEAR|EV_ONESHOT on a socket's read filter.
    mk(); base = sv[0];
    int kq = kqueue();
    struct kevent ch, ev[4];
    EV_SET(&ch, sv[0], EVFILT_READ, EV_ADD | EV_CLEAR | EV_ONESHOT | EV_RECEIPT, 0, 0, 0);
    int n = kevent(kq, &ch, 1, ev, 1, NULL), e = errno; sh("oneshot register", n, e, ev, 0);
    write(sv[1], "x", 1); poll0(kq, "after the peer's write");
    write(sv[1], "y", 1); poll0(kq, "after a second write");
    EV_SET(&ch, sv[0], EVFILT_READ, EV_DELETE | EV_RECEIPT, 0, 0, 0);
    n = kevent(kq, &ch, 1, ev, 1, NULL); e = errno; sh("delete after delivery", n, e, ev, 0);
    EV_SET(&ch, sv[0], EVFILT_READ, EV_ADD | EV_CLEAR | EV_ONESHOT, 0, 0, 0);
    n = kevent(kq, &ch, 1, ev, 4, &zero0); e = errno; sh("register when readable", n, e, ev, 0);
    poll0(kq, "then poll");
    return 0;
}

static int mode_errnoreceipt(void) {
    mk(); nb(sv[0]); nb(sv[1]); base = sv[0];
    int kq = kqueue();
    struct kevent c3[3], e3[3];
    EV_SET(&c3[0], sv[0], EVFILT_READ, EV_ADD | EV_RECEIPT, 0, 0, (void *)11);
    EV_SET(&c3[1], sv[0], EVFILT_WRITE, EV_ADD | EV_RECEIPT, 0, 0, (void *)12);
    EV_SET(&c3[2], 997, EVFILT_READ, EV_ADD | EV_RECEIPT, 0, 0, (void *)13);
    int n = kevent(kq, c3, 3, e3, 1, &zero0), e = errno;
    sh("3 receipts, nevents 1", n, e, e3, 1);
    poll0(kq, "then");
    EV_SET(&c3[0], sv[0], EVFILT_READ, EV_DELETE, 0, 0, 0);
    EV_SET(&c3[1], 996, EVFILT_READ, EV_DELETE, 0, 0, 0);
    n = kevent(kq, c3, 2, e3, 3, &zero0); e = errno;
    sh("no receipt: delete registered, delete 996, nevents 3", n, e, e3, 0);
    n = kevent(kq, &c3[1], 1, NULL, 0, &zero0); e = errno;
    sh("no receipt: delete 996, nevents 0", n, e, e3, 0);
    write(sv[1], "z", 1);
    EV_SET(&c3[0], sv[0], EVFILT_READ, EV_ADD, 0, 0, 0);
    EV_SET(&c3[1], sv[0], EVFILT_WRITE, EV_DELETE | EV_RECEIPT, 0, 0, 0);
    n = kevent(kq, c3, 2, e3, 3, &zero0); e = errno;
    sh("mixed: plain add read (data waiting), receipt delete write", n, e, e3, 0);
    return 0;
}

static int mode_knoteclose(void) {
    mk(); nb(sv[0]); nb(sv[1]); base = sv[0];
    int d = dup(sv[0]), kq = kqueue();
    struct kevent ch, ev[4];
    EV_SET(&ch, sv[0], EVFILT_READ, EV_ADD, 0, 0, 0);
    kevent(kq, &ch, 1, NULL, 0, NULL);
    close(sv[0]);
    write(sv[1], "x", 1);
    poll0(kq, "closed A (a dup kept), B writes");
    close(sv[1]);
    EV_SET(&ch, d, EVFILT_READ, EV_ADD, 0, 0, 0);
    int n = kevent(kq, &ch, 1, ev, 4, &zero0), e = errno;
    sh("B closed, the dup's knote added", n, e, ev, 0);
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
    if (!strcmp(m, "clearsock")) return mode_clearsock();
    if (!strcmp(m, "clearedges")) return mode_clearedges();
    if (!strcmp(m, "clearpipe")) return mode_clearpipe();
    if (!strcmp(m, "receipt")) return mode_receipt();
    if (!strcmp(m, "receiptcorner")) return mode_receiptcorner();
    if (!strcmp(m, "errnoreceipt")) return mode_errnoreceipt();
    if (!strcmp(m, "knoteclose")) return mode_knoteclose();
    fprintf(stderr, "unknown mode %s\n", m);
    return 2;
}
