// $L/t0/cvprobe.c: the psynch shapes node issues (walls.md §3), natively, so lldb can read the
// kernel's return words at each stub + 8. Modes: waitsignal, broad3, timeout, onens, layout.
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t c = PTHREAD_COND_INITIALIZER;
static int ready, go;

static void *waiter(void *arg) {
    (void)arg;
    pthread_mutex_lock(&m);
    ready++;
    while (!go) pthread_cond_wait(&c, &m);
    pthread_mutex_unlock(&m);
    return NULL;
}

static void wait_ready(int n) {
    for (;;) {
        pthread_mutex_lock(&m); int r = ready; pthread_mutex_unlock(&m);
        if (r >= n) break;
        usleep(1000);
    }
    usleep(20000); // let the last waiter reach the kernel
}

static void dump(const char *what) {
    const unsigned char *b = (const unsigned char *)&c;
    printf("%s:", what);
    for (size_t i = 0; i < sizeof c; i++) printf("%s%02x", i % 4 ? "" : " ", b[i]);
    printf("\n");
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "";
    pthread_t t[3];
    if (!strcmp(mode, "waitsignal")) {
        pthread_create(&t[0], NULL, waiter, NULL); wait_ready(1);
        pthread_mutex_lock(&m); go = 1; pthread_mutex_unlock(&m);
        pthread_cond_signal(&c);
        pthread_join(t[0], NULL);
    } else if (!strcmp(mode, "broad3")) {
        for (int i = 0; i < 3; i++) pthread_create(&t[i], NULL, waiter, NULL);
        wait_ready(3);
        pthread_mutex_lock(&m); go = 1; pthread_mutex_unlock(&m);
        pthread_cond_broadcast(&c);
        for (int i = 0; i < 3; i++) pthread_join(t[i], NULL);
    } else if (!strcmp(mode, "timeout") || !strcmp(mode, "onens")) {
        struct timespec rel = { 0, !strcmp(mode, "onens") ? 1 : 50 * 1000 * 1000 };
        pthread_mutex_lock(&m);
        int rc = pthread_cond_timedwait_relative_np(&c, &m, &rel);
        pthread_mutex_unlock(&m);
        printf("%s rc=%d\n", mode, rc);
        dump("after");
    } else if (!strcmp(mode, "layout")) {
        dump("fresh");
        pthread_create(&t[0], NULL, waiter, NULL); wait_ready(1);
        dump("one waiter");
        pthread_mutex_lock(&m); go = 1; pthread_mutex_unlock(&m);
        pthread_cond_signal(&c);
        pthread_join(t[0], NULL);
        dump("after the signal");
    } else { fprintf(stderr, "mode?\n"); return 2; }
    printf("%s done\n", mode);
    return 0;
}
