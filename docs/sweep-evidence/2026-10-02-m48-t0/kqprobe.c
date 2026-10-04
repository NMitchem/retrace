// $L/t0/kqprobe.c: libuv's two kevent shapes, byte for byte, and the pipe answers the model
// computes (walls.md §1 row 1 and "not walls"; plan F7–F9). Each probe uses a fresh kqueue.
#include <sys/event.h>
#include <sys/types.h>
#include <stdio.h>
#include <unistd.h>

static void show(const char *what, int n, const struct kevent *e) {
    printf("%s n=%d", what, n);
    for (int i = 0; i < n; i++)
        printf(" [ident=%#lx filter=%d flags=%#x fflags=%#x data=%ld udata=%p]",
               (unsigned long)e[i].ident, e[i].filter, e[i].flags, e[i].fflags, (long)e[i].data, e[i].udata);
    printf("\n");
}

static void one(const char *what, int fd, short filter, unsigned short flags, const struct timespec *t) {
    int kq = kqueue();
    struct kevent ev;
    EV_SET(&ev, fd, filter, flags, 0, 0, 0);
    int n = kevent(kq, &ev, 1, &ev, 1, t);
    show(what, n, &ev);
    close(kq);
}

int main(void) {
    struct timespec zero = {0, 0}, onens = {0, 1};
    // uv__kqueue_runtime_detection: add and trigger in one call, the event list aliasing the changes.
    int kq = kqueue();
    struct kevent ch[2];
    EV_SET(&ch[0], 0x1e7e7711, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0);
    EV_SET(&ch[1], 0x1e7e7711, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0);
    int n = kevent(kq, ch, 2, ch, 1, &zero);
    show("detect", n, ch);
    show("detect slot 1 afterwards", 1, &ch[1]);
    close(kq);
    // uv__stream_try_select: EVFILT_READ, EV_ADD|EV_ENABLE, 1 ns, on a pipe's write end and on fd 1.
    int p[2];
    pipe(p);
    one("read on a pipe write end, 1 ns", p[1], EVFILT_READ, EV_ADD | EV_ENABLE, &onens);
    one("read on fd 1, 1 ns", 1, EVFILT_READ, EV_ADD | EV_ENABLE, &onens);
    // The pipe model's readiness answers, its capacity and its EOF.
    one("write-ready on an empty pipe", p[1], EVFILT_WRITE, EV_ADD, &zero);
    write(p[1], "abc", 3);
    one("read-ready after 3 bytes", p[0], EVFILT_READ, EV_ADD, &zero);
    one("write-ready after 3 bytes", p[1], EVFILT_WRITE, EV_ADD, &zero);
    close(p[0]);
    one("read on the write end, reader closed", p[1], EVFILT_READ, EV_ADD | EV_ENABLE, &onens);
    one("write on the write end, reader closed", p[1], EVFILT_WRITE, EV_ADD, &zero);
    return 0;
}
