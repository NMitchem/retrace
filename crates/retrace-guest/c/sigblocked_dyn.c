// M55 t3 fixture (spec §3h; t0 p0/semsig.c and p0/pipethread.c): a signal to a thread that is blocked.
// Main blocks; a worker sends SIGUSR1 to the process (kill) or to main (pthread_kill), then releases
// main 50 ms later. Each handled mode prints what main's wait returned, whether the handler ran on
// main, and the handler's own frame (`x0`, the carry, whether the instruction before the saved pc
// is the svc), which is the kernel's return for the interrupted call.
//   sem          main in semaphore_wait, kill(getpid()): returns 14 (KERN_ABORTED), and is re-issued
//   semthread    the same, with pthread_kill(main)
//   semtimed     main in semaphore_timedwait (5 s), kill(getpid()): 14, nothing consumed
//   kevent       main in kevent on an EVFILT_USER, kill(getpid()): -1 EINTR, re-issued
//   keventthread the same, with pthread_kill(main)
//   cv           main in pthread_cond_wait, kill(getpid()): retrace refuses at the raise (M55)
//   term         main in pthread_join, a worker kill(getpid(), SIGTERM)s and would print after it:
//                natively the process dies at the raise (143) and the worker never prints
//   maskedsem    main masks SIGUSR1 and waits on a semaphore; the signal rides through the wake and
//                its handler runs at the unmask
//   maskedkevent the same through kevent
//   pipehandler  (M55 t4, p0/pipethread.c) main in pthread_join; a worker writes to a pipe whose reader is closed. SIGPIPE
//                is process-directed: its handler runs on MAIN, after the worker's write has returned EPIPE;
//                si_code is 0 and si_pid the process's own
//   pipemainmask the same with SIGPIPE masked on main (the worker unmasks it): the worker takes it
//   pipedfl      the same with SIGPIPE at its default: the process dies with 141 while main is blocked
#include <errno.h>
#include <mach/mach.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/event.h>
#include <unistd.h>

static semaphore_t sem;
static pthread_t mainth;
static volatile int on_main, handled;
static int directed;
static pthread_mutex_t mu = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cv = PTHREAD_COND_INITIALIZER;
static int ready;
static int kq;

static unsigned long long fx0, fcpsr, fpc;
static void h(int s, siginfo_t *si, void *uc_) {
    (void)s; (void)si;
    ucontext_t *uc = uc_;
    fx0 = uc->uc_mcontext->__ss.__x[0];
    fcpsr = uc->uc_mcontext->__ss.__cpsr;
    fpc = (unsigned long long)__darwin_arm_thread_state64_get_pc(uc->uc_mcontext->__ss);
    handled++;
    on_main = pthread_equal(pthread_self(), mainth);
}
static void frame(void) {
    unsigned insn = *(unsigned *)(fpc - 4);
    printf("  frame: x0=%#llx C=%llu svc=%d\n", fx0, (fcpsr >> 29) & 1, (insn & 0xffe0001f) == 0xd4000001);
}

static void raise_it(void) {
    if (directed) pthread_kill(mainth, SIGUSR1); else kill(getpid(), SIGUSR1);
}

static void *sem_worker(void *a) {
    (void)a;
    usleep(20000);
    raise_it();
    usleep(50000);
    semaphore_signal(sem);
    return NULL;
}
static void *cv_worker(void *a) {
    (void)a;
    usleep(20000);
    raise_it();
    usleep(50000);
    pthread_mutex_lock(&mu);
    ready = 1;
    pthread_cond_signal(&cv);
    pthread_mutex_unlock(&mu);
    return NULL;
}
static void trigger(void) {
    struct kevent ch;
    EV_SET(&ch, 1, EVFILT_USER, 0, NOTE_TRIGGER, 0, NULL);
    kevent(kq, &ch, 1, NULL, 0, NULL);
}
static void *kq_worker(void *a) {
    (void)a;
    usleep(20000);
    raise_it();
    usleep(50000);
    trigger();
    return NULL;
}
static void *killer(void *a) {
    (void)a;
    usleep(20000);
    kill(getpid(), SIGTERM);
    usleep(20000);
    printf("killer: still running after SIGTERM\n");
    fflush(stdout);
    return NULL;
}

static int pipefds[2], pipe_unmask, pipe_dfl;
static volatile int pran, pon_main, pcode, ppid;
static void ph(int s, siginfo_t *si, void *uc) {
    (void)s; (void)uc;
    pran++; pon_main = pthread_equal(pthread_self(), mainth); pcode = si->si_code; ppid = si->si_pid;
}
static void *pipe_writer(void *a) {
    (void)a;
    if (pipe_unmask) { sigset_t s; sigemptyset(&s); sigaddset(&s, SIGPIPE); pthread_sigmask(SIG_UNBLOCK, &s, NULL); }
    usleep(20000);
    errno = 0;
    ssize_t r = write(pipefds[1], "x", 1);
    /* pipedfl: the signal ends the process on main's thread, so a line printed here would race it natively. */
    if (!pipe_dfl) { printf("worker: write r=%zd errno=%d ran=%d\n", r, errno, pran); fflush(stdout); }
    usleep(20000);
    return NULL;
}

static void mask_usr1(int how) {
    sigset_t s;
    sigemptyset(&s);
    sigaddset(&s, SIGUSR1);
    pthread_sigmask(how, &s, NULL);
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "sem";
    directed = strstr(mode, "thread") != NULL || !strncmp(mode, "masked", 6);
    mainth = pthread_self();
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_sigaction = h;
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGUSR1, &sa, NULL);
    pthread_t w;
    if (!strncmp(mode, "sem", 3) && strcmp(mode, "semtimed") != 0) {
        semaphore_create(mach_task_self(), &sem, SYNC_POLICY_FIFO, 0);
        pthread_create(&w, NULL, sem_worker, NULL);
        kern_return_t kr = semaphore_wait(sem);
        kern_return_t kr2 = kr == KERN_ABORTED ? semaphore_wait(sem) : -1;
        pthread_join(w, NULL);
        printf("%s: first=%d second=%d handled=%d on_main=%d\n", mode, kr, kr2, handled, on_main);
        frame();
    } else if (!strcmp(mode, "semtimed")) {
        semaphore_create(mach_task_self(), &sem, SYNC_POLICY_FIFO, 0);
        pthread_create(&w, NULL, sem_worker, NULL);
        mach_timespec_t ts = {5, 0};
        kern_return_t kr = semaphore_timedwait(sem, ts);
        kern_return_t kr2 = kr == KERN_ABORTED ? semaphore_wait(sem) : -1;
        pthread_join(w, NULL);
        printf("semtimed: first=%d second=%d handled=%d on_main=%d\n", kr, kr2, handled, on_main);
        frame();
    } else if (!strcmp(mode, "cv")) {
        pthread_create(&w, NULL, cv_worker, NULL);
        pthread_mutex_lock(&mu);
        int loops = 0;
        while (!ready) { pthread_cond_wait(&cv, &mu); loops++; }
        pthread_mutex_unlock(&mu);
        pthread_join(w, NULL);
        printf("cv: loops=%d handled=%d on_main=%d\n", loops, handled, on_main);
    } else if (!strncmp(mode, "kevent", 6)) {
        kq = kqueue();
        struct kevent ch, ev;
        EV_SET(&ch, 1, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, NULL);
        kevent(kq, &ch, 1, NULL, 0, NULL);
        pthread_create(&w, NULL, kq_worker, NULL);
        errno = 0;
        int n = kevent(kq, NULL, 0, &ev, 1, NULL);
        int e = errno;
        int n2 = -9;
        if (n < 0) n2 = kevent(kq, NULL, 0, &ev, 1, NULL);
        pthread_join(w, NULL);
        printf("%s: n=%d errno=%d then=%d handled=%d on_main=%d\n", mode, n, e, n2, handled, on_main);
        frame();
    } else if (!strncmp(mode, "pipe", 4)) {
        pipe_dfl = !strcmp(mode, "pipedfl");
        if (!pipe_dfl) {
            struct sigaction ps;
            memset(&ps, 0, sizeof ps);
            ps.sa_sigaction = ph;
            ps.sa_flags = SA_SIGINFO;
            sigaction(SIGPIPE, &ps, NULL);
        }
        if (!strcmp(mode, "pipemainmask")) {
            sigset_t s; sigemptyset(&s); sigaddset(&s, SIGPIPE); pthread_sigmask(SIG_BLOCK, &s, NULL);
            pipe_unmask = 1;
        }
        pipe(pipefds);
        close(pipefds[0]);
        pthread_create(&w, NULL, pipe_writer, NULL);
        pthread_join(w, NULL);
        printf("main: joined ran=%d on_main=%d si_code=%#x si_pid_is_self=%d\n", pran, pon_main, pcode, ppid == getpid());
    } else if (!strcmp(mode, "term")) {
        pthread_create(&w, NULL, killer, NULL);
        pthread_join(w, NULL);
        printf("main: joined\n");
    } else if (!strcmp(mode, "maskedsem")) {
        semaphore_create(mach_task_self(), &sem, SYNC_POLICY_FIFO, 0);
        mask_usr1(SIG_BLOCK);
        pthread_create(&w, NULL, sem_worker, NULL);
        kern_return_t kr = semaphore_wait(sem);
        pthread_join(w, NULL);
        printf("maskedsem: wait=%d handled=%d\n", kr, handled);
        mask_usr1(SIG_UNBLOCK);
        printf("after unmask: handled=%d on_main=%d\n", handled, on_main);
    } else if (!strcmp(mode, "maskedkevent")) {
        kq = kqueue();
        struct kevent ch, ev;
        EV_SET(&ch, 1, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, NULL);
        kevent(kq, &ch, 1, NULL, 0, NULL);
        mask_usr1(SIG_BLOCK);
        pthread_create(&w, NULL, kq_worker, NULL);
        int n = kevent(kq, NULL, 0, &ev, 1, NULL);
        pthread_join(w, NULL);
        printf("maskedkevent: n=%d handled=%d\n", n, handled);
        mask_usr1(SIG_UNBLOCK);
        printf("after unmask: handled=%d on_main=%d\n", handled, on_main);
    } else {
        printf("unknown mode\n");
        return 2;
    }
    return 0;
}
