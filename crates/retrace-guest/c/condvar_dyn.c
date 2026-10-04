// M48 Task 5: psynch condition variables (spec §3h). libpthread's condvars are psynch on this host
// (plan F2), so every blocking wait here is a psynch_cvwait (305) and every signal that finds a
// waiter a psynch_cvsignal (304) or psynch_cvbroad (303). Signals are issued so that no mutex ever
// has a kernel waiter (plan F3), except in `mutex` mode, which exists to reach one. No mode sleeps:
// usleep is __semwait_signal (334), which retrace does not model. Modes:
//   pingpong     ROUNDS rounds of strict alternation between main and one thread on one cv
//   broadcast    three waiters on one cv, released by one pthread_cond_broadcast
//   timedout     a 5 ms relative wait nobody signals, then the same wait issued raw
//   timedsignal  a 2 s relative wait that the second thread signals first
//   onens        node's shape: a {0, 1 ns} relative wait, then the same wait issued raw
//   mutex        a contended firstfit mutex, which reaches psynch_mutexwait (301); retrace refuses it
// Each mode prints only what every native run prints. timedout and onens also print the cv's three
// sequence words, whose S word shows whether the timeout's errno carried ECVCLEARED.
#include <errno.h>
#include <mach/mach_time.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define ROUNDS 8
#define CSEQ_OFF 24 // T0(M4): the offset of pthread_cond_t's c_seq[3] (Step 1's addendum pins it)

// libsystem_kernel's raw stub, exported (libpthread declares it in its private header). Its kernel
// prototype is kern_synch.c:_psynch_cvwait.
extern uint32_t __psynch_cvwait(pthread_cond_t *cv, uint64_t cvlsgen, uint32_t cvugen,
                                pthread_mutex_t *mutex, uint64_t mugen, uint32_t flags,
                                int64_t sec, uint32_t nsec);

static pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER, m2 = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t c = PTHREAD_COND_INITIALIZER, ready_cv = PTHREAD_COND_INITIALIZER,
                      raw = PTHREAD_COND_INITIALIZER;
static int turn, ready, go, flag, saw[3];

static void *ponger(void *arg) {
    (void)arg;
    for (int j = 0; j < ROUNDS; j++) {
        pthread_mutex_lock(&m);
        while (turn != 1) pthread_cond_wait(&c, &m);
        printf("pong %d\n", j);
        turn = 0;
        pthread_cond_signal(&c);
        pthread_mutex_unlock(&m);
    }
    return NULL;
}

static void *bwaiter(void *arg) {
    int k = (int)(intptr_t)arg;
    pthread_mutex_lock(&m);
    ready++;
    pthread_cond_signal(&ready_cv);
    while (!go) pthread_cond_wait(&c, &m);
    saw[k] = go;
    pthread_mutex_unlock(&m);
    return NULL;
}

static void *signaller(void *arg) {
    (void)arg;
    pthread_mutex_lock(&m);
    flag = 1;
    pthread_cond_signal(&c);
    pthread_mutex_unlock(&m);
    return NULL;
}

static void *locker(void *arg) {
    (void)arg;
    pthread_mutex_lock(&m);
    flag = 1;
    pthread_mutex_unlock(&m);
    return NULL;
}

static void dump(const char *mode, pthread_cond_t *cv) {
    const unsigned char *b = (const unsigned char *)cv + CSEQ_OFF;
    printf("%s c_seq:", mode);
    for (int i = 0; i < 12; i++) printf("%s%02x", i % 4 ? "" : " ", b[i]);
    printf("\n");
}

// A relative timed wait through libpthread, on `c`, which nobody signals. elapsed_ge_timeout is 1
// only if at least the interval passed on the guest's own clock (24 MHz, plan F6).
static void timed(const char *mode, long nsec) {
    struct timespec rel = { 0, nsec };
    uint64_t t0 = mach_absolute_time();
    pthread_mutex_lock(&m);
    int rc = pthread_cond_timedwait_relative_np(&c, &m, &rel);
    pthread_mutex_unlock(&m);
    uint64_t t1 = mach_absolute_time();
    printf("%s rc=%d elapsed_ge_timeout=%d\n", mode, rc, t1 - t0 >= (uint64_t)nsec * 3 / 125);
}

// The first wait on a fresh cv as libpthread would issue it (S carries the C bit, L one waiter,
// mutex 0, node's flags 0xa0), straight to the kernel, so its word and errno print unfiltered.
static void raw_wait(const char *mode, uint32_t nsec) {
    errno = 0;
    uint32_t rv = __psynch_cvwait(&raw, (1ull << 32) | 0x100, 0, NULL, 0, 0xa0, 0, nsec);
    printf("%s raw rv=%#x errno=%#x\n", mode, rv, errno);
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "";
    pthread_t t[3];
    if (!strcmp(mode, "pingpong")) {
        pthread_create(&t[0], NULL, ponger, NULL);
        for (int i = 0; i < ROUNDS; i++) {
            pthread_mutex_lock(&m);
            while (turn != 0) pthread_cond_wait(&c, &m);
            printf("ping %d\n", i);
            turn = 1;
            pthread_cond_signal(&c);
            pthread_mutex_unlock(&m);
        }
        pthread_join(t[0], NULL);
        printf("pingpong done\n");
    } else if (!strcmp(mode, "broadcast")) {
        for (int k = 0; k < 3; k++) pthread_create(&t[k], NULL, bwaiter, (void *)(intptr_t)k);
        pthread_mutex_lock(&m);
        while (ready < 3) pthread_cond_wait(&ready_cv, &m);
        go = 1;
        pthread_mutex_unlock(&m);
        pthread_cond_broadcast(&c); // outside the mutex, so no mutex reaches the kernel (plan F3)
        for (int k = 0; k < 3; k++) pthread_join(t[k], NULL);
        printf("broadcast woke 3: saw %d %d %d\n", saw[0], saw[1], saw[2]);
    } else if (!strcmp(mode, "timedout")) {
        timed("timedout", 5 * 1000 * 1000);
        dump("timedout", &c);
        raw_wait("timedout", 5 * 1000 * 1000);
    } else if (!strcmp(mode, "onens")) {
        timed("onens", 1);
        dump("onens", &c);
        raw_wait("onens", 1);
    } else if (!strcmp(mode, "timedsignal")) {
        struct timespec rel = { 2, 0 };
        pthread_create(&t[0], NULL, signaller, NULL);
        uint64_t t0 = mach_absolute_time();
        pthread_mutex_lock(&m);
        int rc = 0;
        while (!flag && rc == 0) rc = pthread_cond_timedwait_relative_np(&c, &m, &rel);
        pthread_mutex_unlock(&m);
        uint64_t t1 = mach_absolute_time();
        pthread_join(t[0], NULL);
        printf("timedsignal rc=%d flag=%d before_deadline=%d\n", rc, flag, t1 - t0 < 2ull * 24000000);
    } else if (!strcmp(mode, "mutex")) {
        // main holds m while it waits on another cv, so the locker contends for m in the kernel.
        struct timespec rel = { 0, 100 * 1000 * 1000 };
        pthread_mutex_lock(&m);
        pthread_create(&t[0], NULL, locker, NULL);
        pthread_mutex_lock(&m2);
        (void)pthread_cond_timedwait_relative_np(&c, &m2, &rel);
        pthread_mutex_unlock(&m2);
        pthread_mutex_unlock(&m);
        pthread_join(t[0], NULL);
        printf("mutex ok flag=%d\n", flag);
    } else {
        fprintf(stderr, "mode?\n");
        return 2;
    }
    return 0;
}
