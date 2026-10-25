// A thread that spins while the thread it waits for cannot run (M52 §3j, Task 7's fixture). Main spins on a flag a
// created thread would set. Natively the thread runs in parallel and the program exits 0 at once. Under a
// cooperative single-vCPU scheduler the child never runs, because main's spin reaches no landmark: that is the
// documented threads limit, and retrace's stall line names it.
#include <pthread.h>
#include <stdio.h>
static volatile int flag;
static void *setter(void *a) { flag = 1; return 0; }
int main(void) {
    pthread_t t;
    pthread_create(&t, 0, setter, 0);
    while (!flag) { /* spin: no syscall, no landmark */ }
    pthread_join(t, 0);
    printf("stall_dyn done\n");
    return 0;
}
