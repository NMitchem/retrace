// A thread that spins while the thread it waits for cannot run (M52 §3j, Task 7's fixture). Main spins on a flag a
// created thread would set. Natively the thread runs in parallel and the program exits 0 at once. Under a
// cooperative single-vCPU scheduler the child never runs, because main's spin reaches no landmark: that is the
// documented threads limit, and retrace's stall line names it.
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
static volatile int flag;
static void *setter(void *a) { flag = 1; return 0; }
// `sleeper` (final-review I3): the child sleeps an hour before it sets the flag, so it is timed-blocked, not
// ready (main's own usleep(1000) is what lets it start and block). It only wakes in a settle that main's spin never reaches; the line must still name it.
static void *sleeping_setter(void *a) { sleep(3600); flag = 1; return 0; }
int main(int argc, char **argv) {
    pthread_t t;
    int sleeper = argc > 1 && !strcmp(argv[1], "sleeper");
    pthread_create(&t, 0, sleeper ? sleeping_setter : setter, 0);
    // The child only runs once main blocks; a short sleep lets it start and block in its own sleep.
    if (sleeper) usleep(1000);
    while (!flag) { /* spin: no syscall, no landmark */ }
    pthread_join(t, 0);
    printf("stall_dyn done\n");
    return 0;
}
