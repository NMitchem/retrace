// M52 §3c fixture: every route a guest reads a thread id by that retrace answers, plus the two
// libpthread mutex kinds that compare owners by id. Modes (argv[1]):
//   routes  main and three children check pthread_threadid_np(NULL) against a direct svc of 372;
//           main reads each live child's id through its pthread_t; a fourth child follows a join.
//   raw     each thread's id in decimal (synthetic under retrace, so compared only with the replay).
//   mutex   a recursive mutex main holds is EBUSY to a child's trylock; an errorcheck relock is EDEADLK.
//   pidinfo one proc_pidinfo(PROC_PIDTHREADID64INFO) on the caller's own id (a wall under retrace).
// thread_info and syscall(372) are walls by design and are not called here.
#include <libproc.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <sys/proc_info.h>
#include <unistd.h>

struct row { unsigned long long np, sc; volatile int done; int rc; pthread_mutex_t *m; };

// A direct svc, the way libsystem_kernel's __thread_selfid issues it.
static unsigned long long selfid_svc(void) {
    register unsigned long long x0 __asm__("x0");
    register unsigned long long x16 __asm__("x16") = 372;
    __asm__ volatile("svc #0x80" : "=r"(x0) : "r"(x16) : "memory", "cc");
    return x0;
}
static void read_ids(struct row *r) {
    uint64_t id = 0;
    pthread_threadid_np(NULL, &id);
    r->np = id;
    r->sc = selfid_svc();
}
static void *child(void *arg) {
    struct row *r = arg;
    read_ids(r);
    r->done = 1;
    usleep(200000);   // stay alive until main has read this thread's id through its pthread_t
    return 0;
}
static void *trylocker(void *arg) {
    struct row *r = arg;
    r->rc = pthread_mutex_trylock(r->m);
    return 0;
}
static int yes(int b) { return b; }

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "routes";
    setvbuf(stdout, NULL, _IONBF, 0);
    if (!strcmp(mode, "pidinfo")) {
        uint64_t id = 0; pthread_threadid_np(NULL, &id);
        struct proc_threadinfo pti;
        printf("pidinfo=%d\n", proc_pidinfo(getpid(), PROC_PIDTHREADID64INFO, id, &pti, sizeof pti));
        return 0;
    }
    if (!strcmp(mode, "mutex")) {
        pthread_mutexattr_t a; pthread_mutex_t rec, ec; struct row r = {0}; pthread_t t;
        pthread_mutexattr_init(&a);
        pthread_mutexattr_settype(&a, PTHREAD_MUTEX_RECURSIVE);
        pthread_mutex_init(&rec, &a);
        pthread_mutexattr_settype(&a, PTHREAD_MUTEX_ERRORCHECK);
        pthread_mutex_init(&ec, &a);
        pthread_mutex_lock(&rec);
        r.m = &rec;
        pthread_create(&t, 0, trylocker, &r);
        pthread_join(t, 0);
        printf("trylock=%d\n", r.rc);
        pthread_mutex_lock(&ec);
        printf("relock=%d\n", pthread_mutex_lock(&ec));
        return 0;
    }
    struct row m = {0}, c[4] = {{0}};
    pthread_t t[4];
    read_ids(&m);
    for (int i = 0; i < 3; i++) pthread_create(&t[i], 0, child, &c[i]);
    for (int i = 0; i < 3; i++) while (!c[i].done) usleep(1000);
    int agree = m.np == m.sc, np_of_child = 1;
    for (int i = 0; i < 3; i++) {
        uint64_t id = 0;
        pthread_threadid_np(t[i], &id);
        agree &= c[i].np == c[i].sc;
        np_of_child &= id == c[i].np;
    }
    for (int i = 0; i < 3; i++) pthread_join(t[i], 0);
    pthread_create(&t[3], 0, child, &c[3]);
    pthread_join(t[3], 0);
    if (!strcmp(mode, "raw")) {
        printf("main %llu\n", m.np);
        for (int i = 0; i < 4; i++) printf("c%d %llu\n", i + 1, c[i].np);
        return 0;
    }
    int distinct = 1, nonzero = m.np != 0, main_not_child = 1, reused = 0;
    for (int i = 0; i < 4; i++) {
        nonzero &= c[i].np != 0;
        main_not_child &= c[i].np != m.np;
        for (int j = i + 1; j < 4; j++) distinct &= c[i].np != c[j].np;
        if (i < 3) reused |= c[3].np == c[i].np;
    }
    printf("routes agree=%s\n", yes(agree && np_of_child) ? "yes" : "no");
    printf("distinct=%s\n", yes(distinct && main_not_child) ? "yes" : "no");
    printf("nonzero=%s\n", nonzero ? "yes" : "no");
    printf("main_is_not_a_child=%s\n", main_not_child ? "yes" : "no");
    printf("reused=%s\n", reused ? "yes" : "no");
    return 0;
}
