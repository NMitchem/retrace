// M53 t3: a duplicated guest kqueue (spec §3d; t0 §M4, probes/kqdupprobe.c). Each mode prints one
// line that native and retrace both determine, and kqdup_e2e compares the two.
//
// argv[1] selects the mode:
//   shared    an EVFILT_USER knote added on kq, triggered through its F_DUPFD_CLOEXEC copy d, and
//             collected on kq.                                                    delivered=1
//   outlives  the same, then close(kq), and d still holds the knote.     still=1
//   fdclose   a pipe's read end watched through d, written, then closed: neither
//             descriptor sees the knote any more.                                 knotes_after=0
//   closewait a thread waits in kevent on kq, main closes kq: refused (T3-closewait), not native-compared.
//   mio       mio's sequence (tokio-1.53.3 runtime/io/driver.rs, mio-1.2.4 kqueue selector), less
//             Registry::register: kqueue, FD_CLOEXEC, two F_DUPFD_CLOEXEC copies at >= 3, a worker
//             that sleeps 20 ms and wakes through the first copy with one EV_ADD|EV_ONESHOT
//             change carrying NOTE_TRIGGER, and main's 1 s wait on the ORIGINAL.  woken=1 by=user
#include <fcntl.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <sys/event.h>
#include <unistd.h>

static int g_waker;

static void ev(struct kevent *k, uintptr_t id, int16_t filter, uint16_t flags, uint32_t fflags) {
    EV_SET(k, id, filter, flags, fflags, 0, (void *)0x77);
}

static int collect(int q, struct kevent *out) {
    struct timespec zero = {0, 0};
    return kevent(q, NULL, 0, out, 4, &zero);
}

static int mode_shared(int outlives) {
    struct kevent k, out[4];
    struct timespec zero = {0, 0};
    int kq = kqueue();
    int d = fcntl(kq, F_DUPFD_CLOEXEC, 0);
    ev(&k, 1, EVFILT_USER, EV_ADD | EV_CLEAR, 0);
    kevent(kq, &k, 1, NULL, 0, &zero);
    if (!outlives) {
        ev(&k, 1, EVFILT_USER, 0, NOTE_TRIGGER);
        kevent(d, &k, 1, NULL, 0, &zero);
        printf("delivered=%d\n", collect(kq, out));
        return 0;
    }
    close(kq);
    ev(&k, 1, EVFILT_USER, 0, NOTE_TRIGGER);
    kevent(d, &k, 1, NULL, 0, &zero);
    int n = collect(d, out);
    printf("still=%d\n", n);
    return 0;
}

static int mode_fdclose(void) {
    struct kevent k, out[4];
    struct timespec zero = {0, 0};
    int p[2];
    pipe(p);
    int kq = kqueue();
    int d = fcntl(kq, F_DUPFD_CLOEXEC, 0);
    ev(&k, p[0], EVFILT_READ, EV_ADD | EV_ENABLE, 0);
    kevent(d, &k, 1, NULL, 0, &zero);
    write(p[1], "x", 1);
    int before = collect(kq, out);
    close(p[0]);
    int after = collect(kq, out) + collect(d, out);
    printf("knotes_before=%d knotes_after=%d\n", before, after);
    return 0;
}

static void *waker(void *arg) {
    (void)arg;
    usleep(20000);
    struct kevent k;
    ev(&k, 0, EVFILT_USER, EV_ADD | EV_ONESHOT, NOTE_TRIGGER);
    kevent(g_waker, &k, 1, NULL, 0, NULL);
    return NULL;
}

static int mode_mio(void) {
    int kq = kqueue();
    fcntl(kq, F_SETFD, FD_CLOEXEC);
    g_waker = fcntl(kq, F_DUPFD_CLOEXEC, 3);
    int registry = fcntl(kq, F_DUPFD_CLOEXEC, 3);
    pthread_t t;
    pthread_create(&t, NULL, waker, NULL);
    struct kevent out[8];
    struct timespec one = {1, 0};
    int n = kevent(kq, NULL, 0, out, 8, &one);
    printf("woken=%d by=%s\n", n, n == 1 && out[0].filter == EVFILT_USER ? "user" : "other");
    pthread_join(t, NULL);
    close(kq);
    close(g_waker);
    close(registry);
    return 0;
}

static void *closewait_waiter(void *arg) {
    int q = *(int *)arg;
    struct kevent out[1];
    struct timespec two = {2, 0};
    kevent(q, NULL, 0, out, 1, &two);
    return NULL;
}

// Not a native comparison: natively the waiter wakes with EBADF. retrace refuses the close (T3-closewait).
static int mode_closewait(void) {
    int kq = kqueue();
    int d = fcntl(kq, F_DUPFD_CLOEXEC, 0);
    (void)d;
    pthread_t t;
    pthread_create(&t, NULL, closewait_waiter, &kq);
    usleep(20000);
    printf("closewait: closing\n");
    close(kq);
    pthread_join(t, NULL);
    printf("closewait: done\n");
    return 0;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    const char *m = argc > 1 ? argv[1] : "";
    if (!strcmp(m, "shared")) return mode_shared(0);
    if (!strcmp(m, "outlives")) return mode_shared(1);
    if (!strcmp(m, "fdclose")) return mode_fdclose();
    if (!strcmp(m, "mio")) return mode_mio();
    if (!strcmp(m, "closewait")) return mode_closewait();
    printf("kqdup: unknown mode\n");
    return 2;
}
