// M49 first-contact corpus (spec §3b): a pthread_mutex that is really contended. Main holds the
// mutex across a pthread_join of a worker, so under retrace's cooperative scheduler (which switches
// only when a thread blocks) the join's __ulock_wait runs the contender while the mutex is held,
// and its lock goes to the kernel as psynch_mutexwait (301), which M48 refuses by value: the psynch
// mutex wall M50 ranks (spec §7). Natively the contender may or may not find the mutex held; the
// output is the same either way. The join is the blocking call because it is modelled today
// (M14); a sleep would put the row behind the sleep wall until Task 5.
#include <pthread.h>
#include <stdio.h>

static pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER;
static int counter;

static void *contender(void *arg) {
    (void)arg;
    pthread_mutex_lock(&m);
    counter += 10;
    pthread_mutex_unlock(&m);
    return NULL;
}

static void *worker(void *arg) {
    volatile long sum = 0;
    for (long i = 0; i < 100000; i++) sum += i;
    *(long *)arg = sum;
    return NULL;
}

int main(void) {
    pthread_t c, w;
    long sum = 0;
    puts("contending");
    fflush(stdout);
    pthread_create(&c, NULL, contender, NULL);
    pthread_mutex_lock(&m);
    counter += 1;
    pthread_create(&w, NULL, worker, &sum);
    pthread_join(w, NULL);
    pthread_mutex_unlock(&m);
    pthread_join(c, NULL);
    printf("counter=%d sum=%ld\n", counter, sum);
    return 0;
}
