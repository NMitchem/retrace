// M52 Task 3 (spec §3d, t0 M5): mach semaphores with a deadline and the kernel's count, by mode
// (argv[1]). Every printed line is the same natively and under retrace; the one number that is not,
// `elapsed_ticks`, goes to stderr. The native lines are t0's (docs/sweep-evidence/2026-10-22-m52-t0b,
// native/sem.log and native/semextra.log).
//   timeout    semaphore_timedwait(s, {0, 50 ms}), signalled by nobody: kr=49, then whether the
//              guest's own mach_absolute_time moved by at least 50 ms across it; stderr gets the ticks
//   signalled  a child signals after main blocks with a 2 s timeout: kr=0
//   badnsec    {1, 1500000000}: kr=18, nothing blocks
//   count      two signals, then two 1 s waits (kr=0, kr=0) and a third 10 ms wait (kr=49)
//   initial    semaphore_create(.., 2): two untimed waits (kr=0, kr=0), then a 10 ms wait (kr=49)
//   destroy    a child waits untimed, main destroys: "waiter kr=37", then main's signal on the dead
//              name: "signal kr=15"
//   long       one 5 s timed wait nobody signals: kr=49
#include <mach/mach.h>
#include <mach/mach_time.h>
#include <mach/semaphore.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static semaphore_t s;

static void make(int value) {
    kern_return_t kr = semaphore_create(mach_task_self(), &s, SYNC_POLICY_FIFO, value);
    if (kr != KERN_SUCCESS) { printf("semaphore_create kr=%d\n", kr); _exit(3); }
}

static kern_return_t timed(int sec, int nsec) {
    mach_timespec_t ts = { (unsigned)sec, nsec };
    return semaphore_timedwait(s, ts);
}

static void *signaller(void *arg) {
    (void)arg;
    usleep(100000);
    semaphore_signal(s);
    return 0;
}

static void *waiter(void *arg) {
    (void)arg;
    printf("waiter kr=%d\n", semaphore_wait(s));
    return 0;
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "";
    pthread_t t;
    if (!strcmp(mode, "timeout")) {
        mach_timebase_info_data_t tb;
        mach_timebase_info(&tb);
        make(0);
        uint64_t t0 = mach_absolute_time();
        kern_return_t kr = timed(0, 50000000);
        uint64_t t1 = mach_absolute_time();
        printf("kr=%d\n", kr);
        printf("waited_at_least_timeout=%s\n", (t1 - t0) * tb.numer / tb.denom >= 50000000ull ? "yes" : "no");
        fprintf(stderr, "elapsed_ticks=%llu\n", (unsigned long long)(t1 - t0));
    } else if (!strcmp(mode, "signalled")) {
        make(0);
        pthread_create(&t, 0, signaller, 0);
        printf("kr=%d\n", timed(2, 0));
        pthread_join(t, 0);
    } else if (!strcmp(mode, "badnsec")) {
        make(0);
        printf("kr=%d\n", timed(1, 1500000000));
    } else if (!strcmp(mode, "count")) {
        make(0);
        semaphore_signal(s);
        semaphore_signal(s);
        printf("kr=%d\n", timed(1, 0));
        printf("kr=%d\n", timed(1, 0));
        printf("kr=%d\n", timed(0, 10000000));
    } else if (!strcmp(mode, "initial")) {
        make(2);
        printf("kr=%d\n", semaphore_wait(s));
        printf("kr=%d\n", semaphore_wait(s));
        printf("kr=%d\n", timed(0, 10000000));
    } else if (!strcmp(mode, "destroy")) {
        make(0);
        pthread_create(&t, 0, waiter, 0);
        usleep(100000);
        semaphore_destroy(mach_task_self(), s);
        pthread_join(t, 0);
        printf("signal kr=%d\n", semaphore_signal(s));
    } else if (!strcmp(mode, "long")) {
        make(0);
        printf("kr=%d\n", timed(5, 0));
    } else {
        fprintf(stderr, "usage: semtimed_dyn timeout|signalled|badnsec|count|initial|destroy|long\n");
        return 2;
    }
    return 0;
}
