// M55 t4 fixture (spec §3g; t0 p0/blockread.c, T0-blockwrite): a blocking read or write on a guest pipe or
// socket end that would wait for another guest thread. retrace cannot run that thread while the call
// waits, so the call is a wall; a call that completes at once, or that is nonblocking, records as native.
//   pipe       main reads an empty pipe a worker writes 1 ms later
//   sock       the same over a socket pair
//   pipewrite  main writes 70000 bytes to an empty pipe (capacity 65536) that a worker reads
//   sockwrite  main writes 10000 bytes to an empty socket (capacity 8192) that a worker reads
//   nonblock   the read of `pipe` with O_NONBLOCK set (F_SETFL): EAGAIN; FIONBIO clears and sets it again
//   cleared    O_NONBLOCK set, then cleared by F_SETFL 0: the read blocks again, so it is a wall
//   dontwait   recv(MSG_DONTWAIT) on an empty socket: EAGAIN
//   fitswrite  main writes 65536 to an empty pipe and 8192 to an empty socket: both complete at once
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <unistd.h>

static int wfd, rfd;
static char big[70000];

static void *writer(void *a) {
    (void)a;
    usleep(1000);
    write(wfd, "w", 1);
    return NULL;
}
static void *reader(void *a) {
    (void)a;
    usleep(1000);
    char buf[4096];
    size_t total = 0;
    ssize_t r;
    while (total < 70000 && (r = read(rfd, buf, sizeof buf)) > 0) total += (size_t)r;
    return NULL;
}

static int mode_read(int sock) {
    int fds[2];
    if (sock) socketpair(AF_UNIX, SOCK_STREAM, 0, fds); else pipe(fds);
    wfd = fds[1];
    printf("start\n"); fflush(stdout);
    pthread_t t;
    pthread_create(&t, NULL, writer, NULL);
    char c = 0;
    ssize_t r = read(fds[0], &c, 1);
    pthread_join(t, NULL);
    printf("%s read r=%zd c=%c\n", sock ? "sock" : "pipe", r, c);
    return 0;
}

static int mode_write(int sock, size_t n) {
    int fds[2];
    if (sock) socketpair(AF_UNIX, SOCK_STREAM, 0, fds); else pipe(fds);
    rfd = fds[0];
    printf("start\n"); fflush(stdout);
    pthread_t t;
    pthread_create(&t, NULL, reader, NULL);
    memset(big, 'z', sizeof big);
    ssize_t r = write(fds[1], big, n);
    pthread_join(t, NULL);
    printf("%s write r=%zd\n", sock ? "sock" : "pipe", r);
    return 0;
}

static int mode_nonblock(void) {
    int fds[2];
    pipe(fds);
    int fl = fcntl(fds[0], F_GETFL);
    fcntl(fds[0], F_SETFL, fl | O_NONBLOCK);
    char c = 0;
    errno = 0;
    ssize_t r = read(fds[0], &c, 1);
    printf("nonblock read r=%zd errno=%d\n", r, r < 0 ? errno : 0);
    write(fds[1], "k", 1);
    r = read(fds[0], &c, 1);
    printf("after a write r=%zd c=%c\n", r, c);
    int on = 0;
    ioctl(fds[0], FIONBIO, &on);
    printf("cleared getfl nonblock=%d\n", (fcntl(fds[0], F_GETFL) & O_NONBLOCK) != 0);
    on = 1;
    ioctl(fds[0], FIONBIO, &on);
    errno = 0;
    r = read(fds[0], &c, 1);
    printf("fionbio read r=%zd errno=%d\n", r, r < 0 ? errno : 0);
    return 0;
}

/* cleared: O_NONBLOCK set then cleared again by F_SETFL 0, so the read is a blocking one: a wall. */
static int mode_cleared(void) {
    int fds[2];
    pipe(fds);
    wfd = fds[1];
    fcntl(fds[0], F_SETFL, O_NONBLOCK);
    fcntl(fds[0], F_SETFL, 0);
    printf("start\n"); fflush(stdout);
    pthread_t t;
    pthread_create(&t, NULL, writer, NULL);
    char c = 0;
    ssize_t r = read(fds[0], &c, 1);
    pthread_join(t, NULL);
    printf("cleared read r=%zd c=%c\n", r, c);
    return 0;
}

static int mode_dontwait(void) {
    int fds[2];
    socketpair(AF_UNIX, SOCK_STREAM, 0, fds);
    char c = 0;
    errno = 0;
    ssize_t r = recv(fds[0], &c, 1, MSG_DONTWAIT);
    printf("dontwait r=%zd errno=%d\n", r, r < 0 ? errno : 0);
    return 0;
}

static int mode_fitswrite(void) {
    int p[2], s[2];
    pipe(p);
    socketpair(AF_UNIX, SOCK_STREAM, 0, s);
    memset(big, 'z', sizeof big);
    printf("pipe write %zd\n", write(p[1], big, 65536));
    printf("sock write %zd\n", write(s[1], big, 8192));
    return 0;
}

int main(int argc, char **argv) {
    const char *m = argc > 1 ? argv[1] : "";
    if (!strcmp(m, "pipe")) return mode_read(0);
    if (!strcmp(m, "sock")) return mode_read(1);
    if (!strcmp(m, "pipewrite")) return mode_write(0, 70000);
    if (!strcmp(m, "sockwrite")) return mode_write(1, 10000);
    if (!strcmp(m, "nonblock")) return mode_nonblock();
    if (!strcmp(m, "cleared")) return mode_cleared();
    if (!strcmp(m, "dontwait")) return mode_dontwait();
    if (!strcmp(m, "fitswrite")) return mode_fitswrite();
    puts("unknown mode");
    return 2;
}
