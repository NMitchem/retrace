// M54 t3. The exec fixture: probe §4's stage 1 / stage 2 shape. Stage 1 sets state and replaces itself
// (`execve`) with stage 2 (itself, argv[1] "stage2"), which prints what it sees. Stage 2 prints only
// what is deterministic: no pid and no tid, but `pid_same=` and `tid_changed=` from values stage 1
// passes in argv. Every line is compared with a native run by value.
//
//   execself_dyn execve                 every row of spec 2b's execve column (descriptors, signals, mask,
//                                       pending, alternate stack, ITIMER_REAL, umask, cwd, environment,
//                                       argv "a b" and "", the thread id)
//   execself_dyn cloexec1               FD_CLOEXEC on fd 1: stage 2 reports 0/1/2 on stderr, its stdout is gone
//   execself_dyn worker                 exec from a worker thread while main blocks in pthread_join
//   execself_dyn pendmasked             a signal pending on a worker, masked in main: it survives exec
//   execself_dyn workersig              the same, unmasked in main: native dies of SIGUSR2 as stage 2 starts;
//                                       retrace refuses it by value (Ruling T3-a)
//   execself_dyn relpath <dir>          chdir to <dir>, then execve("./exec_target_dyn") (the target is there)
//   execself_dyn bytes <target>         argv "\xff\xfe", env X=\x80 into the target
//   execself_dyn pipe <target>          writes "hello" to a pipe whose read end is fd 5, then the target
//   execself_dyn chain <n>              execs itself with n-1 until 0, then prints "chain done"
//   execself_dyn errs <dir> <target>    execve's errors, one `<name> errno=<n>` line each
//   execself_dyn copy <path>            execs <path> (the no-disk guard)
//   execself_dyn e2big <target>         an argument past native's 1 MiB limit: E2BIG
//   execself_dyn bigargs <target>       an argument of 200 KB: past retrace's 128 KiB start-stack limit
#include <errno.h>
#include <stdint.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/event.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <unistd.h>

extern char **environ;
static char *self;
static const char *kind = "execve";
static void h(int s) { (void)s; }

// Under retrace a guest starts with a descriptor a native process does not have: dyld's /dev/dtracehelper,
// opened FD_CLOEXEC at the lowest free number, and each new image's dyld opens it again. Exec closes every
// FD_CLOEXEC descriptor, so none can be inherited and closing the ones found here changes nothing natively; it
// lets the descriptor numbers and the listing compare with native. Exec itself is the subject of this fixture.
static void drop_stray(void) {
    for (int fd = 3; fd < 16; fd++) { int fl = fcntl(fd, F_GETFD); if (fl >= 0 && (fl & FD_CLOEXEC)) close(fd); }
}


static uint64_t tid_now(void) { uint64_t t = 0; pthread_threadid_np(NULL, &t); return t; }

static void stage2(int argc, char **argv) {
    kind = argv[2];
    if (strcmp(kind, "cloexec1") == 0) {
        dprintf(2, "fd0=%s fd1=%s fd2=%s\n", fcntl(0, F_GETFD) >= 0 ? "open" : "closed",
                fcntl(1, F_GETFD) >= 0 ? "open" : "closed", fcntl(2, F_GETFD) >= 0 ? "open" : "closed");
        return;
    }
    printf("stage2 kind=%s argc=%d\n", kind, argc);
    for (int i = 3; i < argc; i++) {
        if (strncmp(argv[i], "tid:", 4) == 0) { printf("tid_changed=%s\n", strtoull(argv[i] + 4, 0, 10) != tid_now() ? "yes" : "no"); continue; }
        if (strncmp(argv[i], "pid:", 4) == 0) { printf("pid_same=%s\n", atoi(argv[i] + 4) == (int)getpid() ? "yes" : "no"); continue; }
        printf("argv[%d]=<%s>\n", i, argv[i]);
    }
    for (int fd = 3; fd < 16; fd++) {
        int fl = fcntl(fd, F_GETFD);
        if (fl < 0) continue;
        if (fd >= 8) { printf("fd %d open cloexec=%d\n", fd, fl & FD_CLOEXEC); continue; }
        struct stat st; fstat(fd, &st);
        printf("fd %d open (%s) cloexec=%d\n", fd, S_ISFIFO(st.st_mode) ? "fifo" : S_ISREG(st.st_mode) ? "file" : "other", fl & FD_CLOEXEC);
    }
    struct sigaction sa;
    sigaction(SIGUSR1, 0, &sa); printf("USR1 %s\n", sa.sa_handler == SIG_DFL ? "DFL" : sa.sa_handler == SIG_IGN ? "IGN" : "HANDLER");
    sigaction(SIGUSR2, 0, &sa); printf("USR2 %s\n", sa.sa_handler == SIG_DFL ? "DFL" : sa.sa_handler == SIG_IGN ? "IGN" : "HANDLER");
    sigset_t m; sigprocmask(SIG_SETMASK, 0, &m);
    printf("mask HUP=%d INT=%d USR2=%d\n", sigismember(&m, SIGHUP), sigismember(&m, SIGINT), sigismember(&m, SIGUSR2));
    sigset_t pend; sigpending(&pend);
    printf("pending HUP=%d USR2=%d\n", sigismember(&pend, SIGHUP), sigismember(&pend, SIGUSR2));
    stack_t ss; sigaltstack(0, &ss); printf("altstack flags=%d size=%zu\n", ss.ss_flags, (size_t)ss.ss_size);
    struct itimerval it; getitimer(ITIMER_REAL, &it);
    printf("itimer armed=%d\n", it.it_value.tv_sec > 0 || it.it_value.tv_usec > 0);
    mode_t um = umask(0); printf("umask=%03o\n", um);
    char cwd[1024]; printf("cwd=%s\n", getcwd(cwd, sizeof cwd));
    const char *x = getenv("X"); printf("env X=%s\n", x ? x : "(unset)");
}

static char tidarg[40], pidarg[40];

// Replace this image with stage 2 (itself) as stage 1 of `kind` has set up the process.
static void do_exec(void) {
    snprintf(tidarg, sizeof tidarg, "tid:%llu", (unsigned long long)tid_now());
    snprintf(pidarg, sizeof pidarg, "pid:%d", (int)getpid());
    char *nargv[] = { self, "stage2", (char *)kind, "a b", "", tidarg, pidarg, NULL };
    char *nenv[] = { "X=carried", NULL };
    fflush(stdout);
    execve(self, nargv, nenv);
    printf("exec failed: errno=%d\n", errno);
    exit(1);
}

static pthread_mutex_t mu = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t c_ready = PTHREAD_COND_INITIALIZER, c_go = PTHREAD_COND_INITIALIZER;
static int ready, go;

static void *exec_worker(void *a) { (void)a; do_exec(); return 0; }

static void *sig_worker(void *a) {
    (void)a;
    sigset_t s; sigemptyset(&s); sigaddset(&s, SIGUSR2); pthread_sigmask(SIG_BLOCK, &s, 0);
    pthread_mutex_lock(&mu); ready = 1; pthread_cond_signal(&c_ready);
    while (!go) pthread_cond_wait(&c_go, &mu);
    pthread_mutex_unlock(&mu);
    return 0;
}

static void errline(const char *name, int rc) { (void)rc; printf("%s errno=%d\n", name, errno); }

static void errs(const char *dir, const char *target) {
    char noexec[1024], garbage[1024], missing[1024];
    snprintf(noexec, sizeof noexec, "%s/noexec", dir);
    snprintf(garbage, sizeof garbage, "%s/garbage", dir);
    snprintf(missing, sizeof missing, "%s/missing", dir);
    int fd = open(noexec, O_WRONLY | O_CREAT | O_TRUNC, 0644); write(fd, "x\n", 2); fchmod(fd, 0644); close(fd);
    fd = open(garbage, O_WRONLY | O_CREAT | O_TRUNC, 0755); write(fd, "not a mach-o\n", 13); fchmod(fd, 0755); close(fd);
    char *a0[] = { "x", NULL }; char *e0[] = { NULL };
    char *badent[] = { (char *)8, NULL };
    execve(missing, a0, e0);                 errline("missing", -1);
    execve(noexec, a0, e0);                  errline("noexec", -1);
    execve(dir, a0, e0);                     errline("directory", -1);
    execve(garbage, a0, e0);                 errline("garbage", -1);
    execve((const char *)8, a0, e0);         errline("badpath", -1);
    // The kernel finds and checks the file before it copies the argument strings in, so a bad argument
    // vector is EFAULT only for an image it would run, and a missing or garbage file keeps its own errno.
    execve(missing, (char **)8, e0);         errline("missing_badargv", -1);
    execve(garbage, (char **)8, e0);         errline("garbage_badargv", -1);
    execve(target, (char **)8, e0);          errline("badargv", -1);
    execve(target, badent, e0);              errline("badargventry", -1);
    execve(target, a0, (char **)8);          errline("badenvp", -1);
    execve(target, a0, badent);              errline("badenventry", -1);
    fflush(stdout);
}

int main(int argc, char **argv) {
    drop_stray();
    self = argv[0];
    if (argc > 1 && strcmp(argv[1], "stage2") == 0) { stage2(argc, argv); return 0; }
    const char *mode = argc > 1 ? argv[1] : "execve";
    if (strcmp(mode, "chain") == 0) {
        int n = argc > 2 ? atoi(argv[2]) : 0;
        if (n <= 0) { printf("chain done\n"); return 0; }
        char nbuf[16]; snprintf(nbuf, sizeof nbuf, "%d", n - 1);
        char *nargv[] = { self, "chain", nbuf, NULL }; char *nenv[] = { NULL };
        execve(self, nargv, nenv);
        printf("exec failed: errno=%d\n", errno); return 1;
    }
    if (strcmp(mode, "errs") == 0) { errs(argv[2], argv[3]); return 0; }
    if (strcmp(mode, "copy") == 0) {
        char *nargv[] = { argv[2], "x", NULL }; char *nenv[] = { "X=copy", NULL };
        execve(argv[2], nargv, nenv);
        printf("exec failed: errno=%d\n", errno); return 1;
    }
    if (strcmp(mode, "relpath") == 0) {
        if (chdir(argv[2]) != 0) { printf("chdir failed\n"); return 1; }
        char *nargv[] = { "./exec_target_dyn", "rel", NULL }; char *nenv[] = { NULL };
        execve("./exec_target_dyn", nargv, nenv);
        printf("exec failed: errno=%d\n", errno); return 1;
    }
    if (strcmp(mode, "bytes") == 0) {
        char *nargv[] = { argv[2], "\xff\xfe", NULL }; char *nenv[] = { "X=\x80", NULL };
        execve(argv[2], nargv, nenv);
        printf("exec failed: errno=%d\n", errno); return 1;
    }
    if (strcmp(mode, "pipe") == 0) {
        int p[2]; pipe(p);
        dup2(p[0], 5); close(p[0]);
        write(p[1], "hello", 5);
        char *nargv[] = { argv[2], NULL }; char *nenv[] = { NULL };
        execve(argv[2], nargv, nenv);
        printf("exec failed: errno=%d\n", errno); return 1;
    }
    if (strcmp(mode, "e2big") == 0 || strcmp(mode, "bigargs") == 0) {
        size_t n = strcmp(mode, "e2big") == 0 ? 1100000 : 200000;
        char *big = malloc(n + 1); memset(big, 'a', n); big[n] = 0;
        char *nargv[] = { argv[2], big, NULL }; char *nenv[] = { NULL };
        execve(argv[2], nargv, nenv);
        printf("exec failed: errno=%d\n", errno); return 0;
    }
    kind = mode;
    printf("stage1 kind=%s\n", kind);
    if (strcmp(mode, "worker") == 0) {
        pthread_t t; pthread_create(&t, 0, exec_worker, 0); pthread_join(t, 0);
        printf("join returned\n"); return 1;
    }
    if (strcmp(mode, "pendmasked") == 0 || strcmp(mode, "workersig") == 0) {
        pthread_t t; pthread_create(&t, 0, sig_worker, 0);
        pthread_mutex_lock(&mu); while (!ready) pthread_cond_wait(&c_ready, &mu); pthread_mutex_unlock(&mu);
        pthread_kill(t, SIGUSR2);
        if (strcmp(mode, "pendmasked") == 0) {
            sigset_t s; sigemptyset(&s); sigaddset(&s, SIGUSR2); sigprocmask(SIG_BLOCK, &s, 0);
        }
        do_exec();
    }
    if (strcmp(mode, "cloexec1") == 0) { fcntl(1, F_SETFD, FD_CLOEXEC); do_exec(); }
    // execve: state for every row of 2b's execve column.
    int keep = open("/etc/hosts", O_RDONLY);                    // survives
    int cx = open("/etc/hosts", O_RDONLY | O_CLOEXEC);          // closed by exec
    int pp[2]; pipe(pp);                                        // survive
    int kq = kqueue();                                          // closed by exec
    dup2(1, 9);                                                 // a console alias: open, FD_CLOEXEC clear
    dup2(1, 11); fcntl(11, F_SETFD, FD_CLOEXEC);                // a console alias with FD_CLOEXEC: closed
    printf("stage1 fds keep=%d cloexec=%d pipe=%d,%d kq=%d\n", keep, cx, pp[0], pp[1], kq);
    signal(SIGUSR1, h); signal(SIGUSR2, SIG_IGN);
    sigset_t m; sigemptyset(&m); sigaddset(&m, SIGHUP); sigprocmask(SIG_BLOCK, &m, 0);
    kill(getpid(), SIGHUP);                                     // masked, so pending across exec
    static char alt[65536]; stack_t ss = { .ss_sp = alt, .ss_size = sizeof alt }; sigaltstack(&ss, 0);
    struct itimerval it = { .it_value = { .tv_sec = 100 } }; setitimer(ITIMER_REAL, &it, 0);
    umask(027); chdir("/tmp");
    do_exec();
    return 1;
}
