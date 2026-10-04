// M48 §3h: the guest-kqueue fixture (K1). Every mode prints what it observed, and kq_e2e compares
// that with this binary's NATIVE output, so a line holds only what native and retrace both
// determine: counts, filters, flags, fflags, data, udata, and booleans. It never holds a raw pipe fd
// or a time. stdout is line-buffered, so a line reaches the trace as it is printed, and a refused
// mode's missing "bad done" is a fact, not a buffer that was never flushed.
//
// argv[1] selects the mode:
//   probe      libuv's uv__kqueue_runtime_detection, byte for byte (M47 node.entry.txt; walls.md §1
//              row 1). An EVFILT_USER add and its NOTE_TRIGGER go in one call whose event list is
//              the change list, on a throwaway kqueue that is then closed. The next kqueue() reuses
//              the fd and must start empty.
//   wake       main blocks in kevent with no timeout. A second thread triggers its EVFILT_USER
//              knote in uv_async_send's shape (one change, no event list).
//   timeout    main waits 5 ms with nothing to wake it, while a second thread makes three calls and
//              exits. Only the clock can end the wait.
//   tryselect  libuv's uv__stream_try_select (walls.md §1): EVFILT_READ, EV_ADD|EV_ENABLE on fd 1,
//              one event, a 1 ns timeout, on the only thread. With stdout a pipe, native answers 0.
//   pipe       main waits on a pipe's read end, and a second thread writes 5 bytes. Then the write
//              end's room, and EOF once the write end is closed.
//   oneshot    an EV_ONESHOT knote is delivered once, and a second poll finds nothing.
//   bad filter an EVFILT_TIMER change on a guest kqueue: retrace refuses it by value.
//   bad notkq  kevent on fd 1, which is not a kqueue: retrace refuses it by value.
#include <mach/mach_time.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/event.h>
#include <unistd.h>

// F6: the timebase is 24 MHz, so 5 ms is 120000 ticks.
#define FIVE_MS_TICKS 120000ULL

static int kq;
static int p[2];

static void show(const char *what, int n, const struct kevent *e) {
    printf("%s n=%d", what, n);
    for (int i = 0; i < n; i++)
        printf(" [ident=%#lx filter=%d flags=%#x fflags=%#x data=%#lx udata=%#lx]",
               (unsigned long)e[i].ident, e[i].filter, e[i].flags, e[i].fflags, (long)e[i].data,
               (unsigned long)(uintptr_t)e[i].udata);
    printf("\n");
}

static void *trigger(void *arg) {
    (void)arg;
    struct kevent t;
    EV_SET(&t, 7, EVFILT_USER, 0, NOTE_TRIGGER | NOTE_FFCOPY | 5, 9, (void *)0x5678);
    kevent(kq, &t, 1, NULL, 0, NULL);
    return NULL;
}

static void *busy(void *arg) {
    (void)arg;
    for (int i = 0; i < 3; i++) (void)getppid();
    return NULL;
}

static void *writer(void *arg) {
    (void)arg;
    write(p[1], "hello", 5);
    return NULL;
}

static int mode_probe(void) {
    kq = kqueue();
    struct kevent ch[2];
    EV_SET(&ch[0], 0x1e7e7711, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0);
    EV_SET(&ch[1], 0x1e7e7711, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0);
    int n = kevent(kq, ch, 2, ch, 1, &(struct timespec){0, 0});
    show("detect", n, ch);
    show("slot1", 1, &ch[1]);
    int first = kq;
    close(kq);
    kq = kqueue();
    struct kevent ev;
    n = kevent(kq, NULL, 0, &ev, 1, &(struct timespec){0, 0});
    printf("reused=%d n=%d\n", kq == first, n);
    return 0;
}

static int mode_wake(void) {
    kq = kqueue();
    struct kevent ev;
    EV_SET(&ev, 7, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, (void *)0x1234);
    kevent(kq, &ev, 1, NULL, 0, NULL);
    pthread_t t;
    pthread_create(&t, NULL, trigger, NULL);
    // Under retrace this always blocks: the new thread runs only once main does.
    int n = kevent(kq, NULL, 0, &ev, 1, NULL);
    show("wake", n, &ev);
    pthread_join(t, NULL);
    return 0;
}

static int mode_timeout(void) {
    kq = kqueue();
    struct kevent ev;
    EV_SET(&ev, 7, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0);
    kevent(kq, &ev, 1, NULL, 0, NULL);
    pthread_t t;
    pthread_create(&t, NULL, busy, NULL);
    uint64_t t0 = mach_absolute_time();
    int n = kevent(kq, NULL, 0, &ev, 1, &(struct timespec){0, 5000000});
    uint64_t waited = mach_absolute_time() - t0;
    printf("timeout n=%d waited=%d\n", n, waited >= FIVE_MS_TICKS);
    pthread_join(t, NULL);
    return 0;
}

static int mode_tryselect(void) {
    kq = kqueue();
    struct kevent ch, ev;
    EV_SET(&ch, 1, EVFILT_READ, EV_ADD | EV_ENABLE, 0, 0, 0);
    uint64_t t0 = mach_absolute_time();
    int n = kevent(kq, &ch, 1, &ev, 1, &(struct timespec){0, 1});
    uint64_t waited = mach_absolute_time() - t0;
    printf("tryselect n=%d waited=%d\n", n, waited >= FIVE_MS_TICKS);
    close(kq);
    return 0;
}

static int mode_pipe(void) {
    pipe(p);
    kq = kqueue();
    struct kevent ev;
    EV_SET(&ev, p[0], EVFILT_READ, EV_ADD, 0, 0, (void *)0xabc);
    kevent(kq, &ev, 1, NULL, 0, NULL);
    pthread_t t;
    pthread_create(&t, NULL, writer, NULL);
    int n = kevent(kq, NULL, 0, &ev, 1, NULL);
    ev.ident = ev.ident == (uintptr_t)p[0]; // an fd number is the host's choice; which end it is is not
    show("readable", n, &ev);
    char buf[16];
    printf("read=%zd\n", read(p[0], buf, sizeof buf));
    pthread_join(t, NULL);
    EV_SET(&ev, p[1], EVFILT_WRITE, EV_ADD, 0, 0, 0);
    n = kevent(kq, &ev, 1, &ev, 1, &(struct timespec){0, 0});
    ev.ident = ev.ident == (uintptr_t)p[1];
    show("writable", n, &ev);
    close(p[1]);
    n = kevent(kq, NULL, 0, &ev, 1, &(struct timespec){0, 0});
    ev.ident = ev.ident == (uintptr_t)p[0];
    show("eof", n, &ev);
    return 0;
}

static int mode_oneshot(void) {
    kq = kqueue();
    struct kevent ch[2], ev;
    EV_SET(&ch[0], 7, EVFILT_USER, EV_ADD | EV_ONESHOT, 0, 0, 0);
    EV_SET(&ch[1], 7, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0);
    int n = kevent(kq, ch, 2, &ev, 1, &(struct timespec){0, 0});
    show("oneshot", n, &ev);
    n = kevent(kq, NULL, 0, &ev, 1, &(struct timespec){0, 0});
    printf("again n=%d\n", n);
    return 0;
}

static int mode_bad(const char *what) {
    kq = kqueue();
    struct kevent ev;
    if (!strcmp(what, "notkq")) {
        EV_SET(&ev, 7, EVFILT_USER, EV_ADD, 0, 0, 0);
        printf("notkq n=%d\n", kevent(1, &ev, 1, NULL, 0, NULL));
    } else {
        EV_SET(&ev, 1, EVFILT_TIMER, EV_ADD | EV_ONESHOT, 0, 1000, 0);
        printf("filter n=%d\n", kevent(kq, &ev, 1, NULL, 0, NULL));
    }
    printf("bad done\n");
    return 0;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    const char *mode = argc > 1 ? argv[1] : "probe";
    if (!strcmp(mode, "probe")) return mode_probe();
    if (!strcmp(mode, "wake")) return mode_wake();
    if (!strcmp(mode, "timeout")) return mode_timeout();
    if (!strcmp(mode, "tryselect")) return mode_tryselect();
    if (!strcmp(mode, "pipe")) return mode_pipe();
    if (!strcmp(mode, "oneshot")) return mode_oneshot();
    if (!strcmp(mode, "bad")) return mode_bad(argc > 2 ? argv[2] : "filter");
    fprintf(stderr, "kq_dyn: unknown mode %s\n", mode);
    return 2;
}
