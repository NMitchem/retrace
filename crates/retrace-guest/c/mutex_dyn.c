// M52 §3f fixture: contended pthread mutexes. Under retrace's cooperative scheduler a thread
// switches only when it blocks, so main holds a mutex across a pthread_join of a busy worker and
// every contender finds it held and goes to the kernel as psynch_mutexwait (301); main's unlock
// finds waiters and issues psynch_mutexdrop (302). Natively the contenders may or may not find the
// mutex held; the output is the same either way. Modes (argv[1]):
//   two        two contenders add ITERS to a counter each under the lock: counter=<2 * ITERS>
//   four       the same with four
//   relock     contenders wait in the kernel while main unlocks and re-locks RELOCKS times: the
//              woken waiter has not run, so a later drop counts it again and finds no queued waiter
//              (the prepost); `relock ok` when every increment landed
//   recursive  main holds a recursive mutex, a child's lock blocks until main unlocks it:
//              order=main-unlocked-then-child-locked
//   errorcheck a second thread contends an errorcheck mutex main holds, then main relocks its own:
//              relock=11 contended ok
//   cond       a condvar waiter signalled under its mutex, which main still holds across a join:
//              the waiter's re-lock is the contended 301 (`cond ok`)
//   fairshare  the same as `two` on a fairshare mutex (refused by value, C5)
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#define ITERS 50
#define RELOCKS 40

static pthread_mutex_t m;
static pthread_cond_t cv = PTHREAD_COND_INITIALIZER;
static int counter, ready, stage;
static const char *order = "child-locked-too-early";

static void *contender(void *arg) {
    (void)arg;
    for (int i = 0; i < ITERS; i++) {
        pthread_mutex_lock(&m);
        counter++;
        pthread_mutex_unlock(&m);
    }
    return NULL;
}

static void *once(void *arg) {
    (void)arg;
    pthread_mutex_lock(&m);
    counter++;
    pthread_mutex_unlock(&m);
    return NULL;
}

static void *recursive_child(void *arg) {
    (void)arg;
    pthread_mutex_lock(&m);
    if (stage == 1) order = "main-unlocked-then-child-locked";
    pthread_mutex_unlock(&m);
    return NULL;
}

static void *busy(void *arg) {
    (void)arg;
    volatile long s = 0;
    for (long i = 0; i < 100000; i++) s += i;
    return NULL;
}

static void *condwaiter(void *arg) {
    (void)arg;
    pthread_mutex_lock(&m);
    ready = 1;
    while (ready == 1) pthread_cond_wait(&cv, &m);
    counter += 100;
    pthread_mutex_unlock(&m);
    return NULL;
}

static void init(int type, int policy) {
    pthread_mutexattr_t a;
    pthread_mutexattr_init(&a);
    pthread_mutexattr_settype(&a, type);
    if (policy >= 0) pthread_mutexattr_setpolicy_np(&a, policy);
    pthread_mutex_init(&m, &a);
}

// Main takes the lock, starts `n` contenders running `fn`, then blocks in a join of a busy worker
// so each contender runs and finds the lock held.
static void contend(int n, void *(*fn)(void *), pthread_t *c) {
    pthread_t w;
    pthread_mutex_lock(&m);
    for (int i = 0; i < n; i++) pthread_create(&c[i], NULL, fn, NULL);
    pthread_create(&w, NULL, busy, NULL);
    pthread_join(w, NULL);
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "two";
    pthread_t c[4];
    setvbuf(stdout, NULL, _IONBF, 0);
    if (!strcmp(mode, "two") || !strcmp(mode, "four") || !strcmp(mode, "fairshare")) {
        int n = !strcmp(mode, "four") ? 4 : 2;
        init(PTHREAD_MUTEX_NORMAL, strcmp(mode, "fairshare") ? -1 : PTHREAD_MUTEX_POLICY_FAIRSHARE_NP);
        contend(n, contender, c);
        pthread_mutex_unlock(&m);
        for (int i = 0; i < n; i++) pthread_join(c[i], NULL);
        printf("counter=%d\n", counter);
    } else if (!strcmp(mode, "relock")) {
        init(PTHREAD_MUTEX_NORMAL, -1);
        contend(2, once, c);
        pthread_mutex_unlock(&m);
        for (int i = 0; i < RELOCKS; i++) {
            pthread_mutex_lock(&m);
            counter++;
            pthread_mutex_unlock(&m);
        }
        for (int i = 0; i < 2; i++) pthread_join(c[i], NULL);
        printf(counter == 2 + RELOCKS ? "relock ok\n" : "relock bad counter=%d\n", counter);
    } else if (!strcmp(mode, "recursive")) {
        init(PTHREAD_MUTEX_RECURSIVE, -1);
        contend(1, recursive_child, c);
        stage = 1;
        pthread_mutex_unlock(&m);
        pthread_join(c[0], NULL);
        printf("order=%s\n", order);
    } else if (!strcmp(mode, "errorcheck")) {
        init(PTHREAD_MUTEX_ERRORCHECK, -1);
        contend(1, once, c);
        int rc = pthread_mutex_lock(&m);
        pthread_mutex_unlock(&m);
        pthread_join(c[0], NULL);
        printf("relock=%d contended %s\n", rc, counter == 1 ? "ok" : "bad");
    } else if (!strcmp(mode, "cond")) {
        pthread_t w;
        init(PTHREAD_MUTEX_NORMAL, -1);
        pthread_create(&c[0], NULL, condwaiter, NULL);
        for (;;) {   // usleep is a landmark, so the waiter can run
            pthread_mutex_lock(&m);
            int r = ready;
            pthread_mutex_unlock(&m);
            if (r) break;
            usleep(1000);
        }
        pthread_mutex_lock(&m);
        ready = 2;
        pthread_cond_signal(&cv);   // under the lock
        pthread_create(&w, NULL, busy, NULL);
        pthread_join(w, NULL);      // blocks while main holds m
        pthread_mutex_unlock(&m);
        pthread_join(c[0], NULL);
        printf(counter == 100 ? "cond ok\n" : "cond bad counter=%d\n", counter);
    } else {
        printf("unknown mode %s\n", mode);
        return 2;
    }
    return 0;
}
