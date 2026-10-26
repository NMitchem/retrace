// M53 t2 (spec §3c): readv, writev, preadv and pwritev, one mode per argv[1], each in a fresh
// mkdtemp directory under /tmp. iovec_e2e runs every mode natively and recorded and requires the
// same lines (t0 M3's probe, iovprobe.c, is where each answer was measured). Only values equal across
// runs are printed: counts, errnos, the bytes written or read, never the directory's name.
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/uio.h>
#include <unistd.h>

#define PAGE 16384
#define SHOW(rc) (long)(rc), (rc) < 0 ? errno : 0

static char dir[64];
static char file[96];

static int open_file(void) {
    snprintf(file, sizeof file, "%s/f", dir);
    return open(file, O_RDWR | O_CREAT | O_TRUNC, 0644);
}

static void rm_file(int fd) { close(fd); unlink(file); }

static void mode_file(void) {
    int fd = open_file();
    char a[3] = "abc", b[4] = "defg", c[5] = "hijkl";
    struct iovec w[3] = {{a, 3}, {b, 4}, {c, 5}};
    long rc = writev(fd, w, 3);
    printf("w=%ld\n", rc);
    lseek(fd, 0, SEEK_SET);
    char x[3], y[4], z[5];
    memset(x, 0, sizeof x); memset(y, 0, sizeof y); memset(z, 0, sizeof z);
    struct iovec r[3] = {{x, 3}, {y, 4}, {z, 5}};
    rc = readv(fd, r, 3);
    printf("r=%ld\n", rc);
    printf("%.3s|%.4s|%.5s\n", x, y, z);
    rc = readv(fd, r, 3);
    printf("at eof r=%ld\n", rc);
    rm_file(fd);
}

static void mode_pos(void) {
    int fd = open_file();
    write(fd, "aaaaaaaaaaaa", 12);
    char d1[2] = {'X', 'Y'}, d2[3] = {'Z', 'W', 'V'};
    struct iovec w[2] = {{d1, 2}, {d2, 3}};
    long rc = pwritev(fd, w, 2, 7);
    printf("w=%ld\n", rc);
    char q1[2], q2[3];
    memset(q1, 0, sizeof q1); memset(q2, 0, sizeof q2);
    struct iovec r[2] = {{q1, 2}, {q2, 3}};
    rc = preadv(fd, r, 2, 7);
    printf("r=%ld\n", rc);
    printf("got %.2s%.3s offset=%ld\n", q1, q2, (long)lseek(fd, 0, SEEK_CUR));
    errno = 0; rc = preadv(fd, r, 2, -1);
    printf("preadv(off -1) rc=%ld errno=%d\n", SHOW(rc));
    int p[2];
    pipe(p);
    errno = 0; rc = preadv(p[0], r, 2, 0);
    printf("preadv(pipe) rc=%ld errno=%d\n", SHOW(rc));
    close(p[0]); close(p[1]);
    rm_file(fd);
}

static void mode_console(void) {
    char m1[] = "a", m2[] = "b\n", e1[] = "e\n";
    struct iovec o[2] = {{m1, 1}, {m2, 2}};
    struct iovec e[1] = {{e1, 2}};
    fflush(stdout);
    long rc = writev(1, o, 2);
    long rc2 = writev(2, e, 1);
    // The counts go to a file so that stdout and stderr hold only the bytes under test.
    int fd = open_file();
    dprintf(fd, "%ld %ld\n", rc, rc2);
    lseek(fd, 0, SEEK_SET);
    char buf[16] = {0};
    read(fd, buf, sizeof buf - 1);
    rm_file(fd);
    if (strcmp(buf, "3 2\n")) { fprintf(stderr, "counts %s", buf); exit(1); }
}

static void mode_shortpipe(void) {
    int p[2];
    pipe(p);
    write(p[1], "12345", 5);
    char p1[3], p2[4], p3[8];
    memset(p1, 0, sizeof p1); memset(p2, 0, sizeof p2); memset(p3, 0, sizeof p3);
    struct iovec r[3] = {{p1, 3}, {p2, 4}, {p3, 8}};
    long rc = readv(p[0], r, 3);
    printf("r=%ld\n", rc);
    printf("%.3s|%.4s|%s\n", p1, p2, p3[0] == 0 ? "empty" : "filled");
    char more = 0;
    fcntl(p[0], F_SETFL, O_NONBLOCK);
    errno = 0;
    printf("drained=%s\n", read(p[0], &more, 1) < 0 && errno == EAGAIN ? "yes" : "no");
    // Written through writev, read back through readv: the pipe's count follows both.
    char w1[] = "ab", w2[] = "cde";
    struct iovec w[2] = {{w1, 2}, {w2, 3}};
    rc = writev(p[1], w, 2);
    printf("w=%ld\n", rc);
    char g1[2], g2[8];
    memset(g1, 0, sizeof g1); memset(g2, 0, sizeof g2);
    struct iovec g[2] = {{g1, 2}, {g2, 8}};
    rc = readv(p[0], g, 2);
    printf("r=%ld %.2s|%s\n", rc, g1, g2);
    close(p[0]); close(p[1]);
}

static void mode_big(void) {
    size_t n = 200 * 1024;
    unsigned char *src = malloc(n), *dst = calloc(1, n);
    for (size_t i = 0; i < n; i++) src[i] = (unsigned char)(i * 7 + 3);
    int fd = open_file();
    struct iovec w[1] = {{src, n}};
    long wr = writev(fd, w, 1);
    lseek(fd, 0, SEEK_SET);
    struct iovec r[1] = {{dst, n}};
    long rr = readv(fd, r, 1);
    printf("w=%ld r=%ld same=%s\n", wr, rr, memcmp(src, dst, n) == 0 ? "yes" : "no");
    // Two entries, each half of it, into a fresh buffer.
    memset(dst, 0, n);
    lseek(fd, 0, SEEK_SET);
    struct iovec r2[2] = {{dst, n / 2}, {dst + n / 2, n / 2}};
    rr = readv(fd, r2, 2);
    printf("halves r=%ld same=%s\n", rr, memcmp(src, dst, n) == 0 ? "yes" : "no");
    rm_file(fd);
}

static void mode_limits(void) {
    int fd = open_file();
    static char buf[1025];
    static struct iovec v[1025];
    for (int i = 0; i < 1025; i++) { v[i].iov_base = buf + i; v[i].iov_len = 1; memset(buf + i, 'q', 1); }
    long rc;
    errno = 0; rc = readv(fd, v, 0);     printf("readv(iovcnt 0) rc=%ld errno=%d\n", SHOW(rc));
    errno = 0; rc = writev(fd, v, 0);    printf("writev(iovcnt 0) rc=%ld errno=%d\n", SHOW(rc));
    errno = 0; rc = writev(fd, v, 1024); printf("writev(iovcnt 1024) rc=%ld errno=%d\n", SHOW(rc));
    errno = 0; rc = writev(fd, v, 1025); printf("writev(iovcnt 1025) rc=%ld errno=%d\n", SHOW(rc));
    errno = 0; rc = writev(fd, v, -1);   printf("writev(iovcnt -1) rc=%ld errno=%d\n", SHOW(rc));
    lseek(fd, 0, SEEK_SET);
    errno = 0; rc = readv(fd, v, 1024);  printf("readv(iovcnt 1024) rc=%ld errno=%d\n", SHOW(rc));
    errno = 0; rc = readv(fd, v, 1025);  printf("readv(iovcnt 1025) rc=%ld errno=%d\n", SHOW(rc));
    errno = 0; rc = readv(fd, v, -1);    printf("readv(iovcnt -1) rc=%ld errno=%d\n", SHOW(rc));
    errno = 0; rc = pwritev(fd, v, 1025, 0); printf("pwritev(iovcnt 1025) rc=%ld errno=%d\n", SHOW(rc));
    rm_file(fd);
}

static void mode_badentry(void) {
    int fd = open_file();
    char *m = mmap(NULL, 2 * PAGE, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    memset(m, 'M', PAGE);
    char *bad = m + PAGE;
    munmap(bad, PAGE);
    long rc;
    struct iovec wb[2] = {{m, 10}, {bad, 10}};
    errno = 0; rc = writev(fd, wb, 2);
    printf("writev(good, bad) rc=%ld errno=%d size=%ld offset=%ld\n", SHOW(rc), (long)lseek(fd, 0, SEEK_END),
           (long)lseek(fd, 0, SEEK_CUR));
    ftruncate(fd, 0); lseek(fd, 0, SEEK_SET);
    struct iovec wb2[2] = {{bad, 10}, {m, 10}};
    errno = 0; rc = writev(fd, wb2, 2);
    printf("writev(bad, good) rc=%ld errno=%d size=%ld\n", SHOW(rc), (long)lseek(fd, 0, SEEK_END));
    ftruncate(fd, 0); lseek(fd, 0, SEEK_SET);
    write(fd, "ABCDEFGHIJKLMNOPQRST", 20);
    lseek(fd, 0, SEEK_SET);
    memset(m, 0, 16);
    struct iovec rb1[2] = {{m, 10}, {bad, 10}};
    errno = 0; rc = readv(fd, rb1, 2);
    printf("readv(good10, bad10) rc=%ld errno=%d first=%.10s offset=%ld\n", SHOW(rc), m, (long)lseek(fd, 0, SEEK_CUR));
    lseek(fd, 0, SEEK_SET);
    memset(m, 0, 16);
    struct iovec rb2[2] = {{bad, 10}, {m, 10}};
    errno = 0; rc = readv(fd, rb2, 2);
    printf("readv(bad10, good10) rc=%ld errno=%d untouched=%s offset=%ld\n", SHOW(rc), m[0] == 0 ? "yes" : "no",
           (long)lseek(fd, 0, SEEK_CUR));
    char z1[4] = {0};
    struct iovec zl[3] = {{z1, 4}, {bad, 0}, {z1, 0}};
    ftruncate(fd, 0); lseek(fd, 0, SEEK_SET);
    write(fd, "WXYZ", 4); lseek(fd, 0, SEEK_SET);
    errno = 0; rc = readv(fd, zl, 3);
    printf("readv(4 good, zero-length bad base, zero-length) rc=%ld errno=%d got=%.4s\n", SHOW(rc), z1);
    struct iovec zl2[2] = {{bad, 0}, {z1, 4}};
    errno = 0; rc = writev(fd, zl2, 2);
    printf("writev(zero-length bad base, 4 good) rc=%ld errno=%d\n", SHOW(rc));
    struct iovec zl3[1] = {{bad, 0}};
    errno = 0; rc = writev(fd, zl3, 1);
    printf("writev(only a zero-length bad base) rc=%ld errno=%d\n", SHOW(rc));
    errno = 0; rc = writev(fd, NULL, 2);
    printf("writev(iov NULL, 2) rc=%ld errno=%d\n", SHOW(rc));
    struct iovec nl[1] = {{z1, (size_t)-1}};
    errno = 0; rc = writev(fd, nl, 1);
    printf("writev(len -1) rc=%ld errno=%d\n", SHOW(rc));
    errno = 0; rc = writev(-1, zl3, 1);
    printf("writev(fd -1) rc=%ld errno=%d\n", SHOW(rc));
    rm_file(fd);
}

// A pipe readv whose second entry is unreachable: natively it fills entry 0 and drains nothing, which
// the pipe model cannot reproduce, so retrace ends the recording here with a wall (M53 §3c).
static void mode_badpipe(void) {
    char *m = mmap(NULL, 2 * PAGE, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    char *bad = m + PAGE;
    munmap(bad, PAGE);
    int p[2];
    pipe(p);
    write(p[1], "ABCDEFGHIJKLMNOPQRST", 20);
    struct iovec r[2] = {{m, 10}, {bad, 10}};
    long rc = readv(p[0], r, 2);
    printf("readv(pipe: good10, bad10) rc=%ld errno=%d\n", SHOW(rc));
}

static void run_mode(const char *mode) {
    if (!strcmp(mode, "file")) mode_file();
    else if (!strcmp(mode, "pos")) mode_pos();
    else if (!strcmp(mode, "console")) mode_console();
    else if (!strcmp(mode, "shortpipe")) mode_shortpipe();
    else if (!strcmp(mode, "big")) mode_big();
    else if (!strcmp(mode, "limits")) mode_limits();
    else if (!strcmp(mode, "badentry")) mode_badentry();
    else if (!strcmp(mode, "badpipe")) mode_badpipe();
    else { printf("unknown mode %s\n", mode); exit(2); }
}

int main(int argc, char **argv) {
    if (argc < 2) { printf("usage: iovec_dyn <mode>\n"); return 2; }
    setvbuf(stdout, NULL, _IOLBF, 0);
    strcpy(dir, "/tmp/iovec_dyn.XXXXXX");
    if (!mkdtemp(dir)) { printf("iovec: mkdtemp failed\n"); return 2; }
    run_mode(argv[1]);
    // `console` prints its bytes before anything else; the removal line is every other mode's last.
    int ok = rmdir(dir) == 0;
    if (strcmp(argv[1], "console")) printf("removed=%s\n", ok ? "yes" : "no");
    return 0;
}
