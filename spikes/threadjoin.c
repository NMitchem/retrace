// spikes/threadjoin.c — M14 Task 1. What does pthread_join block on, and what does a thread's
// lifecycle look like in syscalls? Built and run natively; see spikes/README.md for the recipe.
#include <pthread.h>
#include <stdio.h>
#include <unistd.h>

static void *child(void *arg) {
    (void)arg;
    write(1, "child\n", 6);
    return (void *)42;
}

int main(void) {
    write(1, "before\n", 7);
    pthread_t t;
    if (pthread_create(&t, NULL, child, NULL) != 0) { write(2, "create failed\n", 14); return 1; }
    void *ret = NULL;
    pthread_join(t, &ret);
    printf("joined %ld\n", (long)ret);
    return 0;
}
