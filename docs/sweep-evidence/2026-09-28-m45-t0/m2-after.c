// M45 t0 M2 candidate: dispatch_after, 1 ms, onto the global queue.
#include <dispatch/dispatch.h>
#include <unistd.h>

int main(void) {
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 1000000), dispatch_get_global_queue(0, 0), ^{
        write(1, "fired\n", 6);
        dispatch_semaphore_signal(sem);
    });
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    write(1, "done\n", 5);
    return 0;
}
