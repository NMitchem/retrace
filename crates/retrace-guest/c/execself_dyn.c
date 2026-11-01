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
//   execself_dyn workersig              the same, unmasked in main: native dies of SIGUSR2 as stage 2 starts (159),
//                                       and so does retrace (C5, M54 t4)
//   execself_dyn spawn                  M54 t4: the execve column through posix_spawn(POSIX_SPAWN_SETEXEC): SETSIGDEF{USR2},
//                                       SETSIGMASK{INT}, an open, a dup2, a close and a chdir action (no pending signal)
//   execself_dyn spawn-cxd              CLOEXEC_DEFAULT, inherit 0/1/2 and 22 (a pipe end), dup2(20, 29): 20 and 21 close
//   execself_dyn spawn-hup              a pending, masked SIGHUP and SETSIGMASK(empty): natively the new image dies, exit 129
//   execself_dyn spawn-refuse <attr> [target]
//                                       setpgroup, setsid, resetids, noaslr, port, x86pref (and sched, suspended, flag 200)
//   execself_dyn spawn-fail <n> [target] the failing spawn n (the table at spawn_fail below), its errno and the state after
//   execself_dyn spawn-openx            T4-i: O_CLOEXEC open actions at the lowest free slot (closed) and not (kept); a doomed slot
//                                       replaced by a dup2 action and then closed (ok)
//   execself_dyn relpath <dir>          chdir to <dir>, then execve("./exec_target_dyn") (the target is there)
//   execself_dyn script <dir> <target>  M54 t5: writes <dir>/script `#!<target> one`, execs it with "two": the target prints one, the script path, two
//   execself_dyn script-rel <dir> <target>  the same with the interpreter written relative (`#!./<basename>`), from the target's directory
//   execself_dyn nested <dir> <target>  <dir>/inner is `#!<target>`, <dir>/script is `#!<dir>/inner`: prints `errno=<n>` for the exec
//   execself_dyn bytes <target>         argv "\xff\xfe", env X=\x80 into the target
//   execself_dyn pipe <target>          writes "hello" to a pipe whose read end is fd 20, then the target
//   execself_dyn chain <n>              execs itself with n-1 until 0, then prints "chain done"
//   execself_dyn errs <dir> <target>    execve's errors, one `<name> errno=<n>` line each
//   execself_dyn copy <path>            execs <path> (the no-disk guard)
//   execself_dyn e2big <target>         an argument past native's 1 MiB limit: E2BIG
//   execself_dyn bigargs <target>       an argument of 200 KB: past retrace's 128 KiB start-stack limit
#include <errno.h>
#include <spawn.h>
#include <mach/mach.h>
#include <sys/types.h>
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

// Under retrace a guest starts with a descriptor a native process does not have, and so does every new image:
// libsystem's `socket(AF_UNIX, SOCK_DGRAM)` takes the lowest free number (3), gets `fcntl(F_SETFD, FD_CLOEXEC)`,
// `connect`s to /var/run/syslog and `sendto`s once. A native run has no such socket: its first `open` returns 3,
// where under retrace it returns 4. That is a divergence retrace has (ledgered separately as F1), described here
// and not diagnosed. So nothing below checks a descriptor the stray could take: stage 1 places every descriptor it
// tests at 20 or above, and stage 2 closes nothing before it looks.
// Stage 1 only, before it creates anything: drop the stray so the descriptor numbers other modes print match native.
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
    for (int fd = 20; fd < 32; fd++) {   // only the numbers stage 1 placed; the stray cannot be here
        int fl = fcntl(fd, F_GETFD);
        if (fl < 0) continue;
        struct stat st; fstat(fd, &st);
        // The stray syslog socket (F1) takes the lowest free number in the new image, and a descriptor a test closed at
        // exec can leave that number inside this range: a native process has no socket, so it is not printed.
        if (S_ISSOCK(st.st_mode)) continue;
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

// State for every row of 2b's execve column. Every tested descriptor sits at 20 or above.
static void setup_state(int pend_hup) {
    // Fill 3..19 with dups so each descriptor below lands where it is tested, 20 and up, from the open or
    // pipe itself: `open(O_CLOEXEC)` is what exec is tested on, not a dup2 and an fcntl (Ruling T3-f).
    for (int fd = 3; fd < 20; fd++) { if (dup(0) != fd) { printf("filler landed elsewhere\n"); exit(3); } }
    int keep = open("/etc/hosts", O_RDONLY);                    // 20, survives
    int cx = open("/etc/hosts", O_RDONLY | O_CLOEXEC);          // 21, closed by exec
    int pp[2]; pipe(pp);                                        // 22, 23, survive
    int kq = kqueue();                                          // 24, closed by exec
    for (int fd = 3; fd < 20; fd++) close(fd);
    dup2(1, 25);                                                // a console alias: open, FD_CLOEXEC clear
    dup2(1, 26); fcntl(26, F_SETFD, FD_CLOEXEC);                // a console alias with FD_CLOEXEC: closed
    printf("stage1 fds keep=%d cloexec=%d pipe=%d,%d kq=%d\n", keep, cx, pp[0], pp[1], kq);
    signal(SIGUSR1, h); signal(SIGUSR2, SIG_IGN);
    if (pend_hup) {
        sigset_t m; sigemptyset(&m); sigaddset(&m, SIGHUP); sigprocmask(SIG_BLOCK, &m, 0);
        kill(getpid(), SIGHUP);                                 // masked, so pending across exec
    }
    static char alt[65536]; stack_t ss = { .ss_sp = alt, .ss_size = sizeof alt }; sigaltstack(&ss, 0);
    struct itimerval it = { .it_value = { .tv_sec = 100 } }; setitimer(ITIMER_REAL, &it, 0);
    umask(027); chdir("/tmp");
}

// Replace this image with stage 2 (itself) through posix_spawn with `attr` and `fa`; returns only on failure.
static void do_spawn(posix_spawnattr_t *attr, posix_spawn_file_actions_t *fa, const char *image) {
    snprintf(tidarg, sizeof tidarg, "tid:%llu", (unsigned long long)tid_now());
    snprintf(pidarg, sizeof pidarg, "pid:%d", (int)getpid());
    char *nargv[] = { (char *)image, "stage2", (char *)kind, "a b", "", tidarg, pidarg, NULL };
    char *nenv[] = { "X=carried", NULL };
    fflush(stdout);
    pid_t pid;
    int rc = posix_spawn(&pid, image, fa, attr, nargv, nenv);
    printf("spawn failed: errno=%d\n", rc);
    fflush(stdout);
}

static int state(int fd) { return fcntl(fd, F_GETFD) >= 0; }

// The spawn modes. Returns the exit status.
static int spawn_modes(const char *mode, int argc, char **argv) {
    posix_spawnattr_t attr; posix_spawn_file_actions_t fa;
    posix_spawnattr_init(&attr); posix_spawn_file_actions_init(&fa);
    sigset_t none, usr2, intr; sigemptyset(&none); sigemptyset(&usr2); sigaddset(&usr2, SIGUSR2); sigemptyset(&intr); sigaddset(&intr, SIGINT);
    kind = mode;
    if (strcmp(mode, "spawn") == 0) {
        printf("stage1 kind=%s\n", kind);
        setup_state(0);
        posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETEXEC | POSIX_SPAWN_SETSIGDEF | POSIX_SPAWN_SETSIGMASK);
        posix_spawnattr_setsigdefault(&attr, &usr2);
        posix_spawnattr_setsigmask(&attr, &intr);
        posix_spawn_file_actions_addopen(&fa, 27, "/etc/hosts", O_RDONLY, 0);
        posix_spawn_file_actions_adddup2(&fa, 20, 28);
        posix_spawn_file_actions_addclose(&fa, 22);
        posix_spawn_file_actions_addchdir(&fa, "/usr");
        do_spawn(&attr, &fa, self);
        return 1;
    }
    if (strcmp(mode, "spawn-cxd") == 0) {
        printf("stage1 kind=%s\n", kind);
        int a = open("/etc/hosts", O_RDONLY), b = open("/etc/hosts", O_RDONLY); int pp[2]; pipe(pp);
        dup2(a, 20); close(a); dup2(b, 21); close(b); dup2(pp[0], 22); close(pp[0]); dup2(pp[1], 23); close(pp[1]);
        posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETEXEC | POSIX_SPAWN_CLOEXEC_DEFAULT);
        for (int fd = 0; fd < 3; fd++) posix_spawn_file_actions_addinherit_np(&fa, fd);
        posix_spawn_file_actions_addinherit_np(&fa, 22);
        posix_spawn_file_actions_adddup2(&fa, 20, 29);
        do_spawn(&attr, &fa, self);
        return 1;
    }
    if (strcmp(mode, "spawn-openx") == 0) {
        printf("stage1 kind=%s\n", kind);
        // Fill 3..19, so 20 is the lowest free slot whatever the stray descriptor did (it was dropped at the start).
        for (int fd = 3; fd < 20; fd++) { if (dup(0) != fd) { printf("filler landed elsewhere\n"); exit(3); } }
        int a = open("/etc/hosts", O_RDONLY); dup2(a, 23); close(a);              // 23: plain
        a = open("/etc/hosts", O_RDONLY); dup2(a, 24); close(a); fcntl(24, F_SETFD, FD_CLOEXEC); // 24: doomed
        posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETEXEC);
        posix_spawn_file_actions_addopen(&fa, 20, "/etc/hosts", O_RDONLY | O_CLOEXEC, 0);  // lowest free: closed at exec
        posix_spawn_file_actions_addopen(&fa, 22, "/etc/hosts", O_RDONLY | O_CLOEXEC, 0);  // not (21 is): kept
        posix_spawn_file_actions_adddup2(&fa, 23, 24);                                     // replaces the doomed slot
        posix_spawn_file_actions_addclose(&fa, 24);                                        // ok, then closed
        do_spawn(&attr, &fa, self);
        return 1;
    }
    if (strcmp(mode, "spawn-hup") == 0) {
        printf("stage1 kind=%s\n", kind);
        sigset_t m; sigemptyset(&m); sigaddset(&m, SIGHUP); sigprocmask(SIG_BLOCK, &m, 0);
        kill(getpid(), SIGHUP);                                 // masked, so pending
        posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETEXEC | POSIX_SPAWN_SETSIGMASK);
        posix_spawnattr_setsigmask(&attr, &none);               // the new mask is empty: the pending HUP is taken at once
        do_spawn(&attr, &fa, self);
        return 1;
    }
    if (strcmp(mode, "spawn-refuse") == 0) {
        const char *what = argc > 2 ? argv[2] : "";
        const char *image = argc > 3 ? argv[3] : self;
        short fl = POSIX_SPAWN_SETEXEC;
        if (strcmp(what, "setpgroup") == 0) fl |= POSIX_SPAWN_SETPGROUP;
        else if (strcmp(what, "setsid") == 0) fl |= POSIX_SPAWN_SETSID;
        else if (strcmp(what, "resetids") == 0) fl |= POSIX_SPAWN_RESETIDS;
        else if (strcmp(what, "noaslr") == 0) fl |= 0x100;
        else if (strcmp(what, "sched") == 0) fl |= 0x10;
        else if (strcmp(what, "flag200") == 0) fl |= 0x200;
        posix_spawnattr_setflags(&attr, fl);
        if (strcmp(what, "port") == 0) posix_spawnattr_setspecialport_np(&attr, mach_task_self(), TASK_BOOTSTRAP_PORT);
        if (strcmp(what, "x86pref") == 0) { cpu_type_t t = CPU_TYPE_X86_64; size_t n; posix_spawnattr_setbinpref_np(&attr, 1, &t, &n); }
        if (strcmp(what, "arm64pref") == 0) { cpu_type_t t = CPU_TYPE_ARM64; size_t n; posix_spawnattr_setbinpref_np(&attr, 1, &t, &n); }
        printf("stage1 kind=%s\n", kind);
        char *nargv[] = { (char *)image, "stage2", "refuse", NULL }; char *nenv[] = { NULL };
        fflush(stdout);
        pid_t pid;
        int rc = posix_spawn(&pid, image, NULL, &attr, nargv, nenv);
        printf("spawn failed: errno=%d\n", rc); fflush(stdout);
        return 0;
    }
    if (strcmp(mode, "spawn-fail") == 0) {
        int n = argc > 2 ? atoi(argv[2]) : 0;
        const char *image = argc > 3 ? argv[3] : self;
        // 1 open of a missing path, 2 close of a closed fd, 3 dup2 from a closed fd, 4 chdir to a missing
        // directory, 5 fchdir of a bad fd, 6 a binpref for x86_64 against a plain arm64 image (EBADARCH),
        // 7 close of a closed fd with a missing image (the action's errno beats the image),
        // 10 close of a descriptor close-on-exec before the spawn (EBADF under SETEXEC), 11 a dup2 from one (EBADF),
        // 8 every action succeeds, then the image is missing (nothing persists), 9 the same, then not executable.
        char cwd0[1024]; getcwd(cwd0, sizeof cwd0);
        int pp[2]; int a = open("/etc/hosts", O_RDONLY | O_CLOEXEC); dup2(a, 21); close(a); fcntl(21, F_SETFD, FD_CLOEXEC);
        a = open("/etc/hosts", O_RDONLY); dup2(a, 20); close(a); pipe(pp); dup2(pp[0], 22); close(pp[0]); dup2(pp[1], 23); close(pp[1]);
        posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETEXEC);
        switch (n) {
        case 1: posix_spawn_file_actions_addopen(&fa, 27, "/nonexistent/x", O_RDONLY, 0); break;
        case 2: posix_spawn_file_actions_addclose(&fa, 77); break;
        case 3: posix_spawn_file_actions_adddup2(&fa, 77, 27); break;
        case 4: posix_spawn_file_actions_addchdir(&fa, "/nonexistent/dir"); break;
        case 5: posix_spawn_file_actions_addfchdir_np(&fa, 77); break;
        case 6: { cpu_type_t t = CPU_TYPE_X86_64; size_t k; posix_spawnattr_setbinpref_np(&attr, 1, &t, &k); } break;
        case 10: posix_spawn_file_actions_addclose(&fa, 21); break;
        case 11: posix_spawn_file_actions_adddup2(&fa, 21, 27); break;
        case 7: posix_spawn_file_actions_addclose(&fa, 77); image = "/nonexistent/tgt"; break;
        case 8: case 9:
            posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETEXEC | POSIX_SPAWN_CLOEXEC_DEFAULT);
            posix_spawn_file_actions_adddup2(&fa, 20, 27);
            posix_spawn_file_actions_addopen(&fa, 28, "/etc/hosts", O_RDONLY, 0);
            posix_spawn_file_actions_addclose(&fa, 22);
            posix_spawn_file_actions_addchdir(&fa, "/usr");
            for (int fd = 0; fd < 3; fd++) posix_spawn_file_actions_addinherit_np(&fa, fd);
            image = n == 8 ? "/nonexistent/tgt" : "/etc/hosts";
            break;
        }
        char *nargv[] = { (char *)image, "stage2", "fail", NULL }; char *nenv[] = { NULL };
        fflush(stdout);
        pid_t pid;
        int rc = posix_spawn(&pid, image, &fa, &attr, nargv, nenv);
        char cwd1[1024]; getcwd(cwd1, sizeof cwd1);
        printf("spawn failed: errno=%d\n", rc);
        printf("after: fd27=%s fd28=%s fd20=%s fd22=%s fd21cloexec=%d cwd_changed=%s\n", state(27) ? "open" : "closed",
               state(28) ? "open" : "closed", state(20) ? "open" : "closed", state(22) ? "open" : "closed",
               fcntl(21, F_GETFD) & FD_CLOEXEC, strcmp(cwd0, cwd1) ? "yes" : "no");
        fflush(stdout);
        return 0;
    }
    return 2;
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
    // M54 final review M2: a plausible but unmapped array, past the 16 KiB skip, with a missing file: still ENOENT.
    execve(missing, (char **)0x700000000000, e0); errline("missing_farargv", -1);
    execve(target, (char **)8, e0);          errline("badargv", -1);
    execve(target, badent, e0);              errline("badargventry", -1);
    execve(target, a0, (char **)8);          errline("badenvp", -1);
    execve(target, a0, badent);              errline("badenventry", -1);
    fflush(stdout);
}

int main(int argc, char **argv) {
    self = argv[0];
    if (argc > 1 && strcmp(argv[1], "stage2") == 0) { stage2(argc, argv); return 0; }
    // A native run inherits its parent's dispositions (a `nohup`ed gate ignores SIGHUP); a retrace guest starts
    // with every one SIG_DFL. The pending-HUP modes mean the default, so ask for it.
    signal(SIGHUP, SIG_DFL);
    drop_stray();
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
    if (strcmp(mode, "script") == 0 || strcmp(mode, "script-rel") == 0) {
        char path[1024], line[1100];
        snprintf(path, sizeof path, "%s/script", argv[2]);
        if (strcmp(mode, "script") == 0) snprintf(line, sizeof line, "#!%s one\n", argv[3]);
        else {
            char tmp[1024]; strncpy(tmp, argv[3], sizeof tmp - 1); tmp[sizeof tmp - 1] = 0;
            char *slash = strrchr(tmp, '/'); *slash = 0;
            if (chdir(tmp) != 0) { printf("chdir failed\n"); return 1; }
            snprintf(line, sizeof line, "#!./%s one\n", slash + 1);
        }
        int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0755);
        if (fd < 0 || write(fd, line, strlen(line)) < 0) { printf("write failed\n"); return 1; }
        close(fd); chmod(path, 0755);
        char *nargv[] = { "ignored", "two", NULL }; char *nenv[] = { NULL };
        execve(path, nargv, nenv);
        printf("exec failed: errno=%d\n", errno); return 1;
    }
    if (strcmp(mode, "nested") == 0) {
        char inner[1024], path[1024], line[1100];
        snprintf(inner, sizeof inner, "%s/inner", argv[2]);
        snprintf(path, sizeof path, "%s/script", argv[2]);
        snprintf(line, sizeof line, "#!%s\n", argv[3]);
        int fd = open(inner, O_WRONLY | O_CREAT | O_TRUNC, 0755); write(fd, line, strlen(line)); close(fd); chmod(inner, 0755);
        snprintf(line, sizeof line, "#!%s\n", inner);
        fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0755); write(fd, line, strlen(line)); close(fd); chmod(path, 0755);
        char *nargv[] = { "ignored", "two", NULL }; char *nenv[] = { NULL };
        execve(path, nargv, nenv);
        printf("errno=%d\n", errno); return 0;
    }
    if (strcmp(mode, "bytes") == 0) {
        char *nargv[] = { argv[2], "\xff\xfe", NULL }; char *nenv[] = { "X=\x80", NULL };
        execve(argv[2], nargv, nenv);
        printf("exec failed: errno=%d\n", errno); return 1;
    }
    if (strcmp(mode, "pipe") == 0) {
        int p[2]; pipe(p);
        dup2(p[0], 20); close(p[0]); dup2(p[1], 21); close(p[1]);
        write(21, "hello", 5);
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
    if (strncmp(mode, "spawn", 5) == 0) return spawn_modes(mode, argc, argv);
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
    setup_state(1);
    do_exec();
    return 1;
}
