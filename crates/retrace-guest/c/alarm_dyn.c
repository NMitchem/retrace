// M53 t5 (spec §3f): ITIMER_REAL, alarm, pause, sigsuspend, an interrupted sleep, and the thread a
// process-directed signal reaches. One mode per argv[1]; alarm_e2e runs every mode natively and
// recorded and requires the same lines (t0 M8's probe, alarmprobe.c, is where each was measured).
// Only values that are equal across runs are printed: whole seconds, a yes/no for a bound, and
// never a time. The modes (the ones with a thread index print "main" or "worker", never an id):
//   pause      a handler, setitimer 100 ms, pause()
//   interval   a 20 ms interval timer, three runs of the handler, then cancelled
//   get        getitimer after arming 1 s: the time left is in (0, 1 s]
//   alarm      alarm(5) returns 0, then alarm(0) returns 5
//   ignored    SIGALRM ignored: alarm(1), sleep(2) is not interrupted and the program exits normally
//   nohandler  alarm(1) and pause() with no handler: killed by SIGALRM
//   target     SIGALRM and kill(getpid()) with workers that mask the signal, and main that masks it
//   suspend    sigsuspend(empty) with SIGUSR1 blocked while a worker pthread_kills main: EINTR, the
//              handler ran, and the whole mask afterwards is the one before the call
//   pending    the same with the signal already pending when sigsuspend is called: returns at once
//   sleeper    alarm(1) and sleep(5): sleep returns 4 and the handler ran
//   masked     SIGALRM blocked, alarm(1), sleep(2) returns 0 and the handler runs at the unmask
//   einval     setitimer and getitimer with a bad `which`, and a microsecond field out of range
//   long       alarm(60) and pause(): under retrace this takes no host time (alarm_e2e bounds it)
//   virtual    setitimer(ITIMER_VIRTUAL): natively accepted, under retrace the wall
#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <unistd.h>

static volatile sig_atomic_t hits;
static volatile sig_atomic_t on_main;
static volatile sig_atomic_t on_worker;
static volatile sig_atomic_t stop;
static pthread_t main_thread;

static void handler(int sig) {
    (void)sig;
    hits++;
    if (pthread_equal(pthread_self(), main_thread)) on_main++; else on_worker++;
}

static void install(int sig) {
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = handler;
    sigemptyset(&sa.sa_mask);
    sigaction(sig, &sa, NULL);
}

static void block(int sig) {
    sigset_t s;
    sigemptyset(&s);
    sigaddset(&s, sig);
    pthread_sigmask(SIG_BLOCK, &s, NULL);
}

static void unblock(int sig) {
    sigset_t s;
    sigemptyset(&s);
    sigaddset(&s, sig);
    pthread_sigmask(SIG_UNBLOCK, &s, NULL);
}

static void timer(long sec, long usec, long isec, long iusec) {
    struct itimerval it = {{isec, (int)iusec}, {sec, (int)usec}};
    setitimer(ITIMER_REAL, &it, NULL);
}

static void clear(void) { hits = on_main = on_worker = stop = 0; }

// Sleep in short slices until `stop`, the way a worker waiting on main does.
static void *lingering(void *arg) {
    (void)arg;
    while (!stop) usleep(2000);
    return NULL;
}

static void *lingering_masked(void *arg) {
    block(SIGALRM);
    return lingering(arg);
}

static int sig_to_send;

static void *kill_unmasked(void *arg) {
    (void)arg;
    usleep(30000);
    kill(getpid(), sig_to_send);
    usleep(30000);
    stop = 1;
    return NULL;
}

static void *kill_masked(void *arg) {
    (void)arg;
    block(sig_to_send);
    usleep(30000);
    kill(getpid(), sig_to_send);
    usleep(30000);
    stop = 1;
    return NULL;
}

static void *pthread_kill_main(void *arg) {
    (void)arg;
    usleep(50000);
    pthread_kill(main_thread, SIGUSR1);
    return NULL;
}

static void report(const char *what) {
    printf("%s: ran on main=%d worker=%d\n", what, (int)on_main, (int)on_worker);
}

// Main waits for `stop` in short sleeps, so a signal reaches it while it is asleep.
static void main_waits(void) {
    for (int i = 0; i < 400 && !stop; i++) usleep(2000);
}

static int mode_target(void) {
    pthread_t w;
    // The timer's SIGALRM, neither thread masking it: main takes it.
    clear();
    pthread_create(&w, NULL, lingering, NULL);
    timer(0, 30000, 0, 0);
    usleep(200000);
    stop = 1;
    pthread_join(w, NULL);
    report("alarm, neither masks");
    // Main masks it, the worker does not: it stays pending on main even while the worker runs.
    clear();
    pthread_create(&w, NULL, lingering, NULL);
    block(SIGALRM);
    timer(0, 30000, 0, 0);
    usleep(100000);
    report("alarm, main masks");
    unblock(SIGALRM);
    report("  after main unblocks");
    stop = 1;
    pthread_join(w, NULL);
    // The worker masks it, main does not: main again.
    clear();
    pthread_create(&w, NULL, lingering_masked, NULL);
    timer(0, 30000, 0, 0);
    usleep(200000);
    stop = 1;
    pthread_join(w, NULL);
    report("alarm, worker masks");
    // kill(getpid()) from a worker: main again, whoever sends it.
    sig_to_send = SIGUSR1;
    clear();
    pthread_create(&w, NULL, kill_unmasked, NULL);
    usleep(300000);
    stop = 1;
    pthread_join(w, NULL);
    report("kill from an unmasked worker");
    clear();
    pthread_create(&w, NULL, kill_masked, NULL);
    usleep(300000);
    stop = 1;
    pthread_join(w, NULL);
    report("kill from a worker that masks it");
    // ... and with main masking it, it waits on main until main unmasks.
    clear();
    block(SIGUSR1);
    pthread_create(&w, NULL, kill_unmasked, NULL);
    main_waits();
    report("kill, main masks");
    unblock(SIGUSR1);
    report("  after main unblocks");
    pthread_join(w, NULL);
    return 0;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    const char *mode = argc > 1 ? argv[1] : "";
    main_thread = pthread_self();
    install(SIGALRM);
    install(SIGUSR1);
    printf("alarm %s\n", mode);
    if (!strcmp(mode, "pause")) {
        timer(0, 100000, 0, 0);
        errno = 0;
        int rc = pause();
        printf("rc=%d errno=%d handled=%d\n", rc, errno, (int)hits);
    } else if (!strcmp(mode, "interval")) {
        timer(0, 20000, 0, 20000);
        while (hits < 3) pause();
        timer(0, 0, 0, 0);
        printf("runs=%d\n", (int)hits);
        usleep(100000);
        printf("after the cancel runs=%d\n", (int)hits);
    } else if (!strcmp(mode, "get")) {
        timer(1, 0, 0, 300000);
        struct itimerval cur;
        int rc = getitimer(ITIMER_REAL, &cur);
        int ok = cur.it_value.tv_sec == 0 ? cur.it_value.tv_usec > 0 : (cur.it_value.tv_sec == 1 && cur.it_value.tv_usec == 0);
        printf("rc=%d remaining_ok=%s interval=%ld.%06d\n", rc, ok ? "yes" : "no",
               (long)cur.it_interval.tv_sec, (int)cur.it_interval.tv_usec);
        timer(0, 0, 0, 0);
        getitimer(ITIMER_REAL, &cur);
        printf("disarmed value=%ld.%06d\n", (long)cur.it_value.tv_sec, (int)cur.it_value.tv_usec);
    } else if (!strcmp(mode, "alarm")) {
        unsigned first = alarm(5);
        unsigned second = alarm(0);
        printf("first=%u second=%u\n", first, second);
        unsigned a = alarm(2);
        unsigned b = alarm(1);
        unsigned c = alarm(0);
        printf("again %u %u %u\n", a, b, c);
    } else if (!strcmp(mode, "ignored")) {
        signal(SIGALRM, SIG_IGN);
        alarm(1);
        unsigned left = sleep(2);
        printf("slept left=%u handled=%d\n", left, (int)hits);
    } else if (!strcmp(mode, "nohandler")) {
        signal(SIGALRM, SIG_DFL);
        alarm(1);
        pause();
        printf("not reached\n");
    } else if (!strcmp(mode, "target")) {
        return mode_target();
    } else if (!strcmp(mode, "suspend") || !strcmp(mode, "pending")) {
        sigset_t empty, old, now;
        sigemptyset(&empty);
        block(SIGUSR1);
        pthread_t w;
        if (!strcmp(mode, "suspend")) pthread_create(&w, NULL, pthread_kill_main, NULL);
        else kill(getpid(), SIGUSR1);   // pends on main, which masks it
        errno = 0;
        int rc = sigsuspend(&empty);
        int e = errno;
        pthread_sigmask(SIG_BLOCK, NULL, &now);
        (void)old;
        printf("rc=%d errno=%d handled=%d mask_restored=%s\n", rc, e, (int)hits,
               sigismember(&now, SIGUSR1) && !sigismember(&now, SIGALRM) ? "yes" : "no");
        if (!strcmp(mode, "suspend")) pthread_join(w, NULL);
    } else if (!strcmp(mode, "sleeper")) {
        alarm(1);
        unsigned left = sleep(5);
        printf("left=%u handled=%d\n", left, (int)hits);
    } else if (!strcmp(mode, "masked")) {
        block(SIGALRM);
        alarm(1);
        unsigned left = sleep(2);
        printf("left=%u handled=%d\n", left, (int)hits);
        unblock(SIGALRM);
        printf("after the unmask handled=%d\n", (int)hits);
    } else if (!strcmp(mode, "einval")) {
        struct itimerval it = {{0, 0}, {0, 1000000}}, cur;
        errno = 0;
        int rc = setitimer(ITIMER_REAL, &it, NULL);
        printf("usec 1000000 rc=%d errno=%d\n", rc, rc < 0 ? errno : 0);
        it.it_value.tv_usec = 0;
        it.it_value.tv_sec = -1;
        errno = 0;
        rc = setitimer(ITIMER_REAL, &it, NULL);
        printf("sec -1 rc=%d errno=%d\n", rc, rc < 0 ? errno : 0);
        errno = 0;
        rc = setitimer(7, &it, NULL);
        printf("which 7 rc=%d errno=%d\n", rc, rc < 0 ? errno : 0);
        errno = 0;
        rc = getitimer(7, &cur);
        printf("getitimer which 7 rc=%d errno=%d\n", rc, rc < 0 ? errno : 0);
    } else if (!strcmp(mode, "long")) {
        alarm(60);
        errno = 0;
        int rc = pause();
        printf("rc=%d errno=%d handled=%d\n", rc, errno, (int)hits);
    } else if (!strcmp(mode, "virtual")) {
        struct itimerval it = {{0, 0}, {5, 0}};
        errno = 0;
        int rc = setitimer(ITIMER_VIRTUAL, &it, NULL);
        printf("virtual rc=%d errno=%d\n", rc, rc < 0 ? errno : 0);
    } else {
        printf("unknown mode\n");
        return 2;
    }
    return 0;
}
