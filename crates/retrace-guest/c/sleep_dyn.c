// M49 Task 5: sleep on the synthetic clock (spec §3f, §4; Review Focus 1 and 5). libc's sleep and
// nanosleep with a rem pointer issue clock_get_time (Mach message 1000) on the SYSTEM_CLOCK port libc
// fetched at init, then __semwait_signal (334) in its relative-timeout shape; usleep and
// nanosleep(…, NULL) skip the 1000 (sleep-report.md §1). Modes (argv[1]):
//   sleep1          sleep(1): the 1000, then the 334
//   usleep          usleep(1000): the 334 alone
//   nanosleep_null  nanosleep({0, 5 ms}, NULL)
//   usleep1         usleep(1): 1000 ns, 24 ticks at 24 MHz
//   onens           nanosleep({0, 1 ns}, NULL): 0 ticks, a deadline already reached at the call
//   raw             __semwait_signal on a fresh semaphore for 5 ms, issued by hand: the stub's -1/errno
//   twosleepers     a 2 s sleeper created first and a 1 s sleeper second; the order they wake in
//   pollloop        while (!done) usleep(100), waiting on a worker (Review Focus 1)
//   sigsleeper      main signals a thread that sleeps (S5: retrace refuses it at the wake)
//   refuse <what>   one hand-issued shape the model refuses (S4, R8): relative0, timeout0, mutex,
//                   nsec, semtrap, nosem, nocancel, calendar
// A timed mode prints delta_ns: the guest's own mach_absolute_time across the call, less one
// back-to-back pair of reads. Every timebase read moves retrace's synthetic clock by one stride,
// and the read after the wake is one of them, so under retrace delta_ns is exactly the request.
// Natively it is the request plus the scheduler's overshoot, which varies: sleep_e2e compares
// that one number by bound, never by value. Every other line is the same on every native run.
#include <errno.h>
#include <mach/clock.h>
#include <mach/mach.h>
#include <mach/mach_time.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

// libsystem_kernel's raw stubs, exported (SDK libsystem_kernel.tbd). The kernel prototype is
// __semwait_signal(int cond_sem, int mutex_sem, int timeout, int relative, int64_t tv_sec,
// int32_t tv_nsec), sys/syscall.h 334; its _nocancel twin is 423.
extern int __semwait_signal(int, int, int, int, int64_t, int32_t);
extern int __semwait_signal_nocancel(int, int, int, int, int64_t, int32_t);

static mach_timebase_info_data_t tb;

// One back-to-back pair of reads: the cost of a read, in ticks (see the header).
static uint64_t read_cost(void) {
    uint64_t a = mach_absolute_time();
    uint64_t b = mach_absolute_time();
    return b - a;
}

static unsigned long long ns(uint64_t ticks) { return (unsigned long long)(ticks * tb.numer / tb.denom); }

static void timed(const char *mode, int (*f)(void)) {
    uint64_t cost = read_cost();
    uint64_t t0 = mach_absolute_time();
    int rc = f();
    uint64_t t1 = mach_absolute_time();
    printf("%s rc=%d delta_ns=%llu\n", mode, rc, ns(t1 - t0 - cost));
}

static int do_sleep1(void) { return (int)sleep(1); }
static int do_usleep(void) { return usleep(1000); }
static int do_usleep1(void) { return usleep(1); }
static int do_nanosleep_null(void) { struct timespec t = { 0, 5000000 }; return nanosleep(&t, NULL); }
static int do_onens(void) { struct timespec t = { 0, 1 }; return nanosleep(&t, NULL); }

static semaphore_t fresh_semaphore(void) {
    semaphore_t s;
    kern_return_t kr = semaphore_create(mach_task_self(), &s, SYNC_POLICY_FIFO, 0);
    if (kr != KERN_SUCCESS) { printf("semaphore_create kr=%d\n", kr); _exit(3); }
    return s;
}

// twosleepers: each sleeper takes the next rank when it wakes; LSE atomics, no syscall.
static atomic_int woke;
static int rank[2];
static void *sleeper(void *arg) {
    int k = (int)(intptr_t)arg;
    sleep(k == 0 ? 2 : 1);
    rank[k] = atomic_fetch_add(&woke, 1);
    return NULL;
}

// pollloop: a worker that never blocks; main polls with a short usleep until it is done.
static atomic_int done;
static unsigned long long sum;
static void *summer(void *arg) {
    (void)arg;
    unsigned long long s = 0;
    for (unsigned i = 1; i <= 100000; i++) s += i;
    sum = s;
    atomic_store_explicit(&done, 1, memory_order_release);
    return NULL;
}

// sigsleeper: a thread that sleeps and is signalled.
static volatile sig_atomic_t got;
static void on_usr1(int s) { (void)s; got = 1; }
static void *sleeper1(void *arg) {
    (void)arg;
    unsigned left = sleep(1);
    printf("sigsleeper woke left=%u got=%d\n", left, (int)got);
    return NULL;
}

// refuse: each shape issued by hand, then the native result. Under retrace the recorder stops at
// the call, so `refuse <what> done` is never printed there.
static int refuse(const char *what) {
    semaphore_t s = fresh_semaphore();
    int r;
    errno = 0;
    if (!strcmp(what, "relative0")) {
        r = __semwait_signal(s, 0, 1, 0, 0, 0);          // an absolute deadline, already past
    } else if (!strcmp(what, "timeout0")) {
        semaphore_signal(s);                             // so the untimed wait returns natively
        r = __semwait_signal(s, 0, 0, 1, 0, 0);
    } else if (!strcmp(what, "mutex")) {
        semaphore_t m = fresh_semaphore();
        r = __semwait_signal(s, m, 1, 1, 0, 1000);       // signal m, then wait on s
    } else if (!strcmp(what, "nsec")) {
        r = __semwait_signal(s, 0, 1, 1, 0, 1000000000);
    } else if (!strcmp(what, "semtrap")) {
        semaphore_signal(s);                             // semaphore_signal_trap (-33) on s
        r = __semwait_signal(s, 0, 1, 1, 0, 1000);
    } else if (!strcmp(what, "nosem")) {
        r = __semwait_signal(0, 0, 1, 1, 0, 1000);
    } else if (!strcmp(what, "nocancel")) {
        r = __semwait_signal_nocancel(s, 0, 1, 1, 0, 1000);
    } else if (!strcmp(what, "calendar")) {
        clock_serv_t cal;
        mach_timespec_t ts;
        kern_return_t kr = host_get_clock_service(mach_host_self(), CALENDAR_CLOCK, &cal);
        if (kr == KERN_SUCCESS) kr = clock_get_time(cal, &ts);
        printf("refuse calendar kr=%d\n", kr);
        printf("refuse calendar done\n");
        return 0;
    } else {
        fprintf(stderr, "refuse what?\n");
        return 2;
    }
    printf("refuse %s r=%d errno=%d\n", what, r, errno);
    printf("refuse %s done\n", what);
    return 0;
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "";
    pthread_t t[2];
    mach_timebase_info(&tb);
    if (!strcmp(mode, "sleep1")) timed("sleep1", do_sleep1);
    else if (!strcmp(mode, "usleep")) timed("usleep", do_usleep);
    else if (!strcmp(mode, "nanosleep_null")) timed("nanosleep_null", do_nanosleep_null);
    else if (!strcmp(mode, "usleep1")) timed("usleep1", do_usleep1);
    else if (!strcmp(mode, "onens")) timed("onens", do_onens);
    else if (!strcmp(mode, "raw")) {
        semaphore_t s = fresh_semaphore();
        errno = 0;
        int r = __semwait_signal(s, 0, 1, 1, 0, 5000000);
        printf("raw r=%d errno=%d\n", r, errno);
    } else if (!strcmp(mode, "twosleepers")) {
        pthread_create(&t[0], NULL, sleeper, (void *)0);   // 2 s, created first
        pthread_create(&t[1], NULL, sleeper, (void *)1);   // 1 s, created second
        pthread_join(t[0], NULL);
        pthread_join(t[1], NULL);
        printf("twosleepers 2s rank=%d 1s rank=%d\n", rank[0], rank[1]);
    } else if (!strcmp(mode, "pollloop")) {
        pthread_create(&t[0], NULL, summer, NULL);
        while (!atomic_load_explicit(&done, memory_order_acquire)) usleep(100);
        pthread_join(t[0], NULL);
        printf("pollloop done sum=%llu\n", sum);
    } else if (!strcmp(mode, "sigsleeper")) {
        signal(SIGUSR1, on_usr1);
        pthread_create(&t[0], NULL, sleeper1, NULL);
        usleep(100000);                  // the sleeper is asleep before the signal, on every run
        pthread_kill(t[0], SIGUSR1);
        pthread_join(t[0], NULL);
        printf("sigsleeper done\n");
    } else if (!strcmp(mode, "refuse") && argc > 2) {
        return refuse(argv[2]);
    } else {
        fprintf(stderr, "mode?\n");
        return 2;
    }
    return 0;
}
