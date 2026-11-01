// M55 t1 first-contact row: the program a newcomer writes with a socket pair. A nonblocking pair, one
// message each way, one kqueue level wait, one end closed, and the other reads EOF. One line per step.
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/event.h>
#include <sys/socket.h>
#include <unistd.h>

int main(void) {
    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) != 0) { puts("socketpair failed"); return 1; }
    for (int i = 0; i < 2; i++) fcntl(sv[i], F_SETFL, fcntl(sv[i], F_GETFL) | O_NONBLOCK);
    char buf[8];
    ssize_t r = read(sv[0], buf, sizeof buf);
    printf("empty read: %zd errno=%d\n", r, r < 0 ? errno : 0);
    printf("ping: %zd\n", write(sv[0], "ping", 4));
    printf("pong: %zd\n", write(sv[1], "pong", 4));
    int kq = kqueue();
    struct kevent ch, ev;
    EV_SET(&ch, sv[0], EVFILT_READ, EV_ADD, 0, 0, 0);
    kevent(kq, &ch, 1, NULL, 0, NULL);
    int n = kevent(kq, NULL, 0, &ev, 1, NULL);
    printf("kevent: n=%d read data=%lld\n", n, n > 0 ? (long long)ev.data : -1LL);
    r = read(sv[0], buf, sizeof buf);
    printf("A got %zd %.*s\n", r, (int)r, buf);
    r = read(sv[1], buf, sizeof buf);
    printf("B got %zd %.*s\n", r, (int)r, buf);
    close(sv[1]);
    n = kevent(kq, NULL, 0, &ev, 1, NULL);
    printf("kevent after close: n=%d eof=%d\n", n, n > 0 && (ev.flags & EV_EOF) != 0);
    r = read(sv[0], buf, sizeof buf);
    printf("A at eof: %zd\n", r);
    return 0;
}
