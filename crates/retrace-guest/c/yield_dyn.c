// M52 Task 4 (spec §3e, t0 M6): yields, and polls that would spin, by mode (argv[1]). Stdout is the
// same natively and under retrace; the numbers that are not (yield counts, guest clock ticks) go to
// stderr, so a test reads them from the recording only.
//   peer      main creates a child that sets a flag; `while (!flag) sched_yield();` -> "done",
//             stderr `yields=<n>`
//   sleeper   the child usleep(10000)s, then sets the flag; main yields until it is set, reading
//             mach_absolute_time before and after -> "done", stderr `yields=<n> elapsed_ticks=<m>`
//   np        pthread_yield_np(), then thread_switch(MACH_PORT_NULL, o, t) for o in 0..2, t in 0..1,
//             a line `<call> rc=<n>` each (rc=0 natively for all seven), beside a child -> "done"
//   badopt    thread_switch(MACH_PORT_NULL, 4, 0): an option no caller passes, refused by value
//   sempoll   main loops semaphore_timedwait(s, {0,0}) until it returns 0; a child computes, then
//             signals -> "done"
//   kqpoll    main loops kevent(kq, ..., &{0,0}) on a pipe's read end; a child computes, then writes
//             the pipe -> "done"
#include <mach/mach.h>
#include <mach/mach_time.h>
#include <mach/semaphore.h>
#include <mach/thread_switch.h>
#include <pthread.h>
#include <sched.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/event.h>
#include <unistd.h>

static volatile int flag;
static semaphore_t sem;
static int pfd[2];

static void *setter(void *arg) { (void)arg; __atomic_store_n(&flag, 1, __ATOMIC_SEQ_CST); return 0; }

static void *sleeping_setter(void *arg) {
    (void)arg;
    usleep(10000);
    __atomic_store_n(&flag, 1, __ATOMIC_SEQ_CST);
    return 0;
}

static void compute(void) {
    volatile unsigned long x = 0;
    for (int i = 0; i < 2000; i++) x += i;
}

static void *sem_producer(void *arg) { (void)arg; compute(); semaphore_signal(sem); return 0; }

static void *pipe_producer(void *arg) { (void)arg; compute(); (void)!write(pfd[1], "x", 1); return 0; }

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "";
    pthread_t t;
    setvbuf(stdout, NULL, _IOLBF, 0);
    if (!strcmp(mode, "peer")) {
        pthread_create(&t, 0, setter, 0);
        unsigned yields = 0;
        while (!__atomic_load_n(&flag, __ATOMIC_SEQ_CST)) { sched_yield(); yields++; }
        pthread_join(t, 0);
        printf("done\n");
        fprintf(stderr, "yields=%u\n", yields);
    } else if (!strcmp(mode, "sleeper")) {
        pthread_create(&t, 0, sleeping_setter, 0);
        unsigned yields = 0;
        uint64_t t0 = mach_absolute_time();
        while (!__atomic_load_n(&flag, __ATOMIC_SEQ_CST)) { sched_yield(); yields++; }
        uint64_t t1 = mach_absolute_time();
        pthread_join(t, 0);
        printf("done\n");
        fprintf(stderr, "yields=%u elapsed_ticks=%llu\n", yields, (unsigned long long)(t1 - t0));
    } else if (!strcmp(mode, "np")) {
        pthread_create(&t, 0, setter, 0);
        pthread_yield_np();
        printf("pthread_yield_np rc=0\n");
        for (int o = 0; o <= 2; o++)
            for (int ms = 0; ms <= 1; ms++)
                printf("thread_switch(%d, %d) rc=%d\n", o, ms, (int)thread_switch(MACH_PORT_NULL, o, ms));
        pthread_join(t, 0);
        printf("done\n");
    } else if (!strcmp(mode, "badopt")) {
        printf("rc=%d\n", (int)thread_switch(MACH_PORT_NULL, 4, 0));
    } else if (!strcmp(mode, "sempoll")) {
        semaphore_create(mach_task_self(), &sem, SYNC_POLICY_FIFO, 0);
        pthread_create(&t, 0, sem_producer, 0);
        mach_timespec_t zero = {0, 0};
        while (semaphore_timedwait(sem, zero) != KERN_SUCCESS) {}
        pthread_join(t, 0);
        printf("done\n");
    } else if (!strcmp(mode, "kqpoll")) {
        (void)!pipe(pfd);
        int kq = kqueue();
        struct kevent ch, ev;
        EV_SET(&ch, pfd[0], EVFILT_READ, EV_ADD, 0, 0, 0);
        kevent(kq, &ch, 1, NULL, 0, NULL);
        pthread_create(&t, 0, pipe_producer, 0);
        struct timespec zero = {0, 0};
        while (kevent(kq, NULL, 0, &ev, 1, &zero) != 1) {}
        pthread_join(t, 0);
        printf("done\n");
    }
    return 0;
}
