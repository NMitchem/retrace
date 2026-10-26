// M53 t1 (spec §3b): the filesystem and process calls the census's rows cover, one mode per
// argv[1], each in a fresh mkdtemp directory under /tmp. posix_e2e runs every mode natively and
// recorded and requires the same lines (t0 M2's probe, posixprobe.c, is where each was measured).
// Only values that are equal across runs are printed: never an inode, a time the program did not
// set, the directory's name, or the host's whole xattr list. fc_posix.c includes this file without
// its main and runs the modes in sequence in one directory.
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <mach/mach.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/attr.h>
#include <sys/clonefile.h>
#include <sys/file.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/time.h>
#include <sys/xattr.h>
#include <unistd.h>

// One "name rc= errno=" line; errno is 0 unless the call failed.
#define R(name, expr)                                            \
    do {                                                         \
        errno = 0;                                               \
        long _rc = (long)(expr);                                 \
        int _e = errno;                                          \
        printf("%s rc=%ld errno=%d\n", name, _rc, _rc < 0 ? _e : 0); \
    } while (0)

static char dir[64];

// A one-argument syscall issued directly (`svc #0x80`, number in x16), with errno set as libc does.
// libc's `syscall()` goes through the indirect syscall 0, which retrace refuses; fdatasync (187) and
// fsync_nocancel (408) have no wrapper libc routes there otherwise (t0 M2).
static long raw1(long num, long a0) {
    register long x0 __asm__("x0") = a0;
    register long x16 __asm__("x16") = num;
    long failed;
    __asm__ volatile("svc #0x80\n\tcset %1, cs" : "+r"(x0), "=r"(failed) : "r"(x16) : "memory", "cc");
    if (failed) { errno = (int)x0; return -1; }
    return x0;
}

static void path(char *out, size_t n, const char *name) { snprintf(out, n, "%s/%s", dir, name); }

static int open_dir(void) { return open(dir, O_RDONLY | O_DIRECTORY); }

static int make_file(const char *name, const char *data) {
    char p[256];
    path(p, sizeof p, name);
    int fd = open(p, O_RDWR | O_CREAT | O_TRUNC, 0644);
    if (fd >= 0 && data) write(fd, data, strlen(data));
    return fd;
}

static long size_of(int fd) {
    struct stat st;
    return fstat(fd, &st) == 0 ? (long)st.st_size : -1;
}

// Remove everything under `d`, directories first. Uses only calls the modes themselves exercise.
static void clear_dir(const char *d) {
    DIR *dp = opendir(d);
    if (!dp) return;
    struct dirent *e;
    while ((e = readdir(dp))) {
        if (!strcmp(e->d_name, ".") || !strcmp(e->d_name, "..")) continue;
        char p[512];
        snprintf(p, sizeof p, "%s/%s", d, e->d_name);
        struct stat st;
        if (lstat(p, &st) != 0) continue;
        if (S_ISDIR(st.st_mode)) {
            clear_dir(p);
            rmdir(p);
        } else {
            chflags(p, 0);
            unlink(p);
        }
    }
    closedir(dp);
}

static void mode_files(void) {
    char p[256];
    path(p, sizeof p, "f1");
    int fd = make_file("f1", "0123456789");
    R("ftruncate", ftruncate(fd, 6));
    printf("size=%ld\n", size_of(fd));
    R("truncate", truncate(p, 4));
    printf("size=%ld\n", size_of(fd));
    R("fsync", fsync(fd));
    R("fsync_nocancel", raw1(408, fd));
    R("fdatasync", raw1(187, fd));
    R("fpathconf(_PC_NAME_MAX)", fpathconf(fd, _PC_NAME_MAX));
    int fd2 = open(p, O_RDWR);
    R("flock(LOCK_EX|LOCK_NB)", flock(fd, LOCK_EX | LOCK_NB));
    R("flock(LOCK_EX|LOCK_NB, other fd, held)", flock(fd2, LOCK_EX | LOCK_NB));
    R("flock(LOCK_UN)", flock(fd, LOCK_UN));
    R("flock(LOCK_SH|LOCK_NB, other fd, free)", flock(fd2, LOCK_SH | LOCK_NB));
    R("flock(bad descriptor)", flock(-1, LOCK_SH));
    close(fd2);
    close(fd);
    clear_dir(dir);
}

static void mode_links(void) {
    char p[256], q[256], buf[64];
    int fd = make_file("f1", "data");
    close(fd);
    int dfd = open_dir();
    path(p, sizeof p, "sl2");
    R("symlink", symlink("f1", p));
    R("symlinkat", symlinkat("f1", dfd, "sl1"));
    memset(buf, 0, sizeof buf);
    R("readlink", readlink(p, buf, sizeof buf));
    printf("target=%s\n", buf);
    memset(buf, 0, sizeof buf);
    R("readlinkat", readlinkat(dfd, "sl1", buf, sizeof buf));
    printf("target=%s\n", buf);
    path(p, sizeof p, "f1");
    path(q, sizeof q, "hl0");
    R("link", link(p, q));
    R("linkat", linkat(dfd, "f1", dfd, "hl1", 0));
    R("linkat(exists)", linkat(dfd, "f1", dfd, "hl1", 0));
    struct stat st;
    stat(p, &st);
    printf("nlink=%d\n", (int)st.st_nlink);
    R("renameat", renameat(dfd, "hl1", dfd, "hl2"));
    R("renameatx_np(RENAME_EXCL, exists)", renameatx_np(dfd, "hl2", dfd, "f1", RENAME_EXCL));
    R("renameatx_np(RENAME_EXCL, fresh)", renameatx_np(dfd, "hl2", dfd, "fresh", RENAME_EXCL));
    R("faccessat(fresh)", faccessat(dfd, "fresh", F_OK, 0));
    R("faccessat(hl2, renamed away)", faccessat(dfd, "hl2", F_OK, 0));
    close(dfd);
    clear_dir(dir);
}

static void mode_dirs(void) {
    char p[256];
    int dfd = open_dir();
    R("mkdirat", mkdirat(dfd, "d1", 0755));
    R("mkdirat(again)", mkdirat(dfd, "d1", 0755));
    // A guest descriptor on a directory the guest opened: every call below names it by number.
    int sub = openat(dfd, "d1", O_RDONLY | O_DIRECTORY);
    printf("subfd_opened=%d\n", sub >= 0);
    R("mkdirat(inner, on subfd)", mkdirat(sub, "inner", 0755));
    int xfd = openat(sub, "x", O_CREAT | O_WRONLY, 0644);
    printf("xfd_opened=%d\n", xfd >= 0);
    close(xfd);
    R("faccessat(x, on subfd)", faccessat(sub, "x", F_OK, 0));
    R("faccessat(missing, on subfd)", faccessat(sub, "nope", F_OK, 0));
    R("unlinkat(dir without AT_REMOVEDIR)", unlinkat(sub, "inner", 0));
    R("unlinkat(AT_REMOVEDIR)", unlinkat(sub, "inner", AT_REMOVEDIR));
    R("unlinkat(file)", unlinkat(sub, "x", 0));
    R("faccessat(x, removed)", faccessat(sub, "x", F_OK, 0));
    close(sub);
    path(p, sizeof p, "d1");
    R("rmdir", rmdir(p));
    R("rmdir(missing)", rmdir(p));
    close(dfd);
    clear_dir(dir);
}

static void show_mode(const char *name, const char *p) {
    struct stat st;
    if (stat(p, &st) == 0) printf("%s mode=%04o flags=%u\n", name, st.st_mode & 07777, (unsigned)st.st_flags);
}

static void mode_modes(void) {
    char p[256], q[256], ff[256];
    uid_t uid = getuid();
    gid_t gid = getgid();
    int fd = make_file("f1", "data");
    int dfd = open_dir();
    path(p, sizeof p, "f1");
    R("chmod", chmod(p, 0600));
    show_mode("after chmod", p);
    R("fchmod", fchmod(fd, 0640));
    show_mode("after fchmod", p);
    R("fchmodat(AT_SYMLINK_NOFOLLOW)", fchmodat(dfd, "f1", 0644, AT_SYMLINK_NOFOLLOW));
    show_mode("after fchmodat", p);
    path(q, sizeof q, "sl1");
    symlink("f1", q);
    R("chown(own uid/gid)", chown(p, uid, gid));
    R("fchown(own uid/gid)", fchown(fd, uid, gid));
    R("lchown(own uid/gid, symlink)", lchown(q, uid, gid));
    R("fchownat(own uid/gid)", fchownat(dfd, "f1", uid, gid, 0));
    R("chflags(UF_NODUMP)", chflags(p, UF_NODUMP));
    show_mode("after chflags", p);
    R("fchflags(0)", fchflags(fd, 0));
    show_mode("after fchflags", p);
    path(ff, sizeof ff, "ff1");
    R("mkfifo", mkfifo(ff, 0600));
    R("mkfifo(exists)", mkfifo(ff, 0600));
    show_mode("fifo", ff);
    close(fd);
    close(dfd);
    clear_dir(dir);
}

static void show_mtime(const char *name, const char *p) {
    struct stat st;
    if (stat(p, &st) == 0) printf("%s mtime=%ld\n", name, (long)st.st_mtime);
}

static void mode_times(void) {
    char p[256];
    int fd = make_file("f1", "data");
    int dfd = open_dir();
    path(p, sizeof p, "f1");
    struct timeval tv[2] = {{1000000000, 0}, {1000000001, 0}};
    R("futimes", futimes(fd, tv));
    show_mtime("after futimes", p);
    struct timespec ts[2] = {{1000000000, 0}, {1000000002, 0}};
    R("futimens", futimens(fd, ts));
    show_mtime("after futimens", p);
    ts[1].tv_sec = 1000000003;
    R("utimensat", utimensat(dfd, "f1", ts, 0));
    show_mtime("after utimensat", p);
    struct attrlist al;
    memset(&al, 0, sizeof al);
    al.bitmapcount = ATTR_BIT_MAP_COUNT;
    al.commonattr = ATTR_CMN_MODTIME;
    struct timespec t = {1000000004, 0};
    R("setattrlist", setattrlist(p, &al, &t, sizeof t, 0));
    show_mtime("after setattrlist", p);
    R("setattrlist(bufferSize 0)", setattrlist(p, &al, &t, 0, 0));
    close(fd);
    close(dfd);
    clear_dir(dir);
}

static void show_clone(const char *name) {
    char p[256], buf[64];
    path(p, sizeof p, name);
    int fd = open(p, O_RDONLY);
    long n = fd >= 0 ? read(fd, buf, sizeof buf) : -1, sum = 0;
    for (long i = 0; i < n; i++) sum += (unsigned char)buf[i];
    printf("%s size=%ld sum=%ld\n", name, n, sum);
    if (fd >= 0) close(fd);
}

static void mode_clone(void) {
    char p[256], q[256];
    int fd = make_file("f1", "clone me, please");
    int dfd = open_dir();
    path(p, sizeof p, "f1");
    path(q, sizeof q, "cl1");
    R("clonefile", clonefile(p, q, 0));
    R("clonefile(exists)", clonefile(p, q, 0));
    show_clone("cl1");
    R("fclonefileat", fclonefileat(fd, dfd, "cl2", 0));
    show_clone("cl2");
    close(fd);
    close(dfd);
    clear_dir(dir);
}

// The names of the file's attributes that the program set; the host adds attributes of its own.
static int has_name(const char *list, long n, const char *name) {
    for (long i = 0; i < n; i += strlen(list + i) + 1)
        if (!strcmp(list + i, name)) return 1;
    return 0;
}

static void mode_xattr(void) {
    char p[256], out[1024], big[200];
    int fd = make_file("f1", "data");
    path(p, sizeof p, "f1");
    R("setxattr", setxattr(p, "user.m53", "abc", 3, 0, 0));
    R("setxattr(XATTR_CREATE, exists)", setxattr(p, "user.m53", "abc", 3, 0, XATTR_CREATE));
    R("getxattr(size query)", getxattr(p, "user.m53", NULL, 0, 0, 0));
    memset(out, 0, sizeof out);
    R("getxattr", getxattr(p, "user.m53", out, sizeof out, 0, 0));
    printf("value=%s\n", out);
    R("getxattr(short buffer)", getxattr(p, "user.m53", out, 2, 0, 0));
    R("getxattr(missing)", getxattr(p, "user.nope", out, sizeof out, 0, 0));
    long n = listxattr(p, out, sizeof out, 0);
    printf("listxattr has_name=%d\n", has_name(out, n, "user.m53"));
    R("fsetxattr", fsetxattr(fd, "user.m53b", "abc", 3, 0, 0));
    memset(out, 0, sizeof out);
    R("fgetxattr", fgetxattr(fd, "user.m53b", out, sizeof out, 0, 0));
    printf("value=%s\n", out);
    n = flistxattr(fd, out, sizeof out, 0);
    printf("flistxattr has_name=%d\n", has_name(out, n, "user.m53b"));
    memset(big, 'n', sizeof big - 1);
    big[sizeof big - 1] = 0;
    R("setxattr(name 199 bytes)", setxattr(p, big, "abc", 3, 0, 0));
    big[127] = 0;
    R("setxattr(name 127 bytes)", setxattr(p, big, "abc", 3, 0, 0));
    R("removexattr(127-byte name)", removexattr(p, big, 0));
    R("removexattr", removexattr(p, "user.m53", 0));
    R("removexattr(missing)", removexattr(p, "user.m53", 0));
    R("fremovexattr", fremovexattr(fd, "user.m53b", 0));
    close(fd);
    clear_dir(dir);
}

static void mode_ids(void) {
    gid_t g[32];
    int n = getgroups(0, NULL);
    int m = getgroups(32, g);
    printf("groups_count_matches=%s\n", n == m && n >= 0 ? "yes" : "no");
    if (n > 1) R("getgroups(1, below the count)", getgroups(1, g));
    printf("pgid_is_pgrp=%s\n", getpgid(0) == getpgrp() ? "yes" : "no");
    printf("sid_ok=%s\n", getsid(0) > 0 ? "yes" : "no");
    R("getpgid(999999)", getpgid(999999));
    errno = 0;
    int prio = getpriority(PRIO_PROCESS, 0);
    printf("prio=%d\n", prio);
    R("getpriority(bad which)", getpriority(99, 0));
}

// T1-ports: construct and destruct a port. The name is the kernel's and is not printed.
static void mode_ports(void) {
    mach_port_options_t opts = {.flags = MPO_INSERT_SEND_RIGHT};
    mach_port_name_t name = 0;
    kern_return_t kr = mach_port_construct(mach_task_self(), &opts, 0, &name);
    printf("mach_port_construct kr=%d name_nonzero=%d\n", kr, name != 0);
    kr = mach_port_destruct(mach_task_self(), name, 0, 0);
    printf("mach_port_destruct kr=%d\n", kr);
    kr = mach_port_destruct(mach_task_self(), name, 0, 0);
    printf("mach_port_destruct(again) kr=%d\n", kr);
}

static int wfd_a, wfd_b;

static void *flock_waiter(void *arg) {
    (void)arg;
    flock(wfd_b, LOCK_EX); // blocks natively until main unlocks
    printf("locked\n");
    fflush(stdout);
    return NULL;
}

// A flock another descriptor holds: natively it waits and is released after 50 ms; retrace walls (R6).
static void mode_flockwait(void) {
    wfd_a = make_file("f1", "data");
    char p[256];
    path(p, sizeof p, "f1");
    wfd_b = open(p, O_RDWR);
    flock(wfd_a, LOCK_EX);
    pthread_t t;
    pthread_create(&t, NULL, flock_waiter, NULL);
    usleep(50000);
    flock(wfd_a, LOCK_UN);
    pthread_join(t, NULL);
    close(wfd_a);
    close(wfd_b);
    clear_dir(dir);
}

// A four-argument syscall issued directly, errno as libc sets it: the stat64 _extended calls have no
// wrapper that reaches them with a NULL xsecurity (statx_np always passes a filesec).
static long raw4(long num, long a0, long a1, long a2, long a3) {
    register long x0 __asm__("x0") = a0;
    register long x1 __asm__("x1") = a1;
    register long x2 __asm__("x2") = a2;
    register long x3 __asm__("x3") = a3;
    register long x16 __asm__("x16") = num;
    long failed;
    __asm__ volatile("svc #0x80\n\tcset %4, cs" : "+r"(x0), "+r"(x1), "+r"(x2), "+r"(x3), "=r"(failed)
                     : "r"(x16) : "memory", "cc");
    if (failed) { errno = (int)x0; return -1; }
    return x0;
}

// M53 t6: setpriority and getpriority. 19 is the maximum nice value and always permitted, so the lines
// do not depend on the nice value the process inherited.
static void mode_prio(void) {
    R("setpriority(19)", setpriority(PRIO_PROCESS, 0, 19));
    errno = 0;
    printf("getpriority=%d\n", getpriority(PRIO_PROCESS, 0));
    R("setpriority(bad which)", setpriority(99, 0, 0));
    R("getpriority(bad which)", getpriority(99, 0));
}

// M53 t6: stat64_extended (341), lstat64_extended (342), fstat64_extended (343): once with a NULL
// xsecurity and size pointer, once with a buffer and its size, and once failing. Only stable fields.
static void show_ext(const char *name, long rc, int e, struct stat *st, size_t sz, int sized) {
    printf("%s rc=%ld errno=%d", name, rc, rc < 0 ? e : 0);
    if (rc == 0) printf(" fmt=%o size=%lld", st->st_mode & S_IFMT, (long long)st->st_size);
    if (sized) printf(" xsize=%ld", (long)sz);
    printf("\n");
}

static void mode_ext(void) {
    char p[256], missing[256], ln[256], label[96];
    path(p, sizeof p, "f1");
    path(missing, sizeof missing, "nothere");
    path(ln, sizeof ln, "l1");
    close(make_file("f1", "hello"));
    symlink("f1", ln);
    int fd = open(p, O_RDONLY);
    struct stat st;
    char buf[1024];
    size_t sz;
    long rc;
    long calls[2] = {341, 342};
    const char *names[2] = {"stat64_extended", "lstat64_extended"};
    for (int i = 0; i < 2; i++) {
        const char *target = i ? ln : p;
        memset(&st, 0, sizeof st);
        errno = 0;
        rc = raw4(calls[i], (long)target, (long)&st, 0, 0);
        snprintf(label, sizeof label, "%s(NULL xsecurity)", names[i]);
        show_ext(label, rc, errno, &st, 0, 0);
        memset(&st, 0, sizeof st);
        memset(buf, 0x5a, sizeof buf);
        sz = sizeof buf;
        errno = 0;
        rc = raw4(calls[i], (long)target, (long)&st, (long)buf, (long)&sz);
        snprintf(label, sizeof label, "%s(buffer)", names[i]);
        show_ext(label, rc, errno, &st, sz, 1);
        sz = sizeof buf;
        errno = 0;
        rc = raw4(calls[i], (long)missing, (long)&st, (long)buf, (long)&sz);
        snprintf(label, sizeof label, "%s(missing)", names[i]);
        show_ext(label, rc, errno, &st, sz, 0);
    }
    // A buffer shorter than the 44-byte filesec: the call still succeeds and the size written back is
    // the size needed; the buffer's first bytes are what the kernel left (measured, not printed).
    sz = 8;
    errno = 0;
    rc = raw4(341, (long)p, (long)&st, (long)buf, (long)&sz);
    show_ext("stat64_extended(short buffer)", rc, errno, &st, sz, 1);
    memset(&st, 0, sizeof st);
    errno = 0;
    rc = raw4(343, fd, (long)&st, 0, 0);
    show_ext("fstat64_extended(NULL xsecurity)", rc, errno, &st, 0, 0);
    memset(&st, 0, sizeof st);
    memset(buf, 0x5a, sizeof buf);
    sz = sizeof buf;
    errno = 0;
    rc = raw4(343, fd, (long)&st, (long)buf, (long)&sz);
    show_ext("fstat64_extended(buffer)", rc, errno, &st, sz, 1);
    sz = sizeof buf;
    errno = 0;
    rc = raw4(343, 99, (long)&st, (long)buf, (long)&sz);
    show_ext("fstat64_extended(bad fd)", rc, errno, &st, sz, 0);
    close(fd);
    clear_dir(dir);
}

static void run_mode(const char *mode) {
    if (!strcmp(mode, "files")) mode_files();
    else if (!strcmp(mode, "links")) mode_links();
    else if (!strcmp(mode, "dirs")) mode_dirs();
    else if (!strcmp(mode, "modes")) mode_modes();
    else if (!strcmp(mode, "times")) mode_times();
    else if (!strcmp(mode, "clone")) mode_clone();
    else if (!strcmp(mode, "xattr")) mode_xattr();
    else if (!strcmp(mode, "ids")) mode_ids();
    else if (!strcmp(mode, "ports")) mode_ports();
    else if (!strcmp(mode, "flockwait")) mode_flockwait();
    else if (!strcmp(mode, "prio")) mode_prio();
    else if (!strcmp(mode, "ext")) mode_ext();
    else { printf("posix: unknown mode\n"); exit(2); }
}

#ifndef POSIX_DYN_LIB
int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    strcpy(dir, "/tmp/posix_dyn.XXXXXX");
    if (!mkdtemp(dir)) { printf("posix: mkdtemp failed\n"); return 2; }
    run_mode(argc > 1 ? argv[1] : "");
    printf("removed=%s\n", unlinkat(AT_FDCWD, dir, AT_REMOVEDIR) == 0 ? "yes" : "no");
    return 0;
}
#endif
