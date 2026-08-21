// M18 rung 5: the smallest guest that forces libdispatch's global-queue worker pool.
// dispatch_async onto the global concurrent queue makes libdispatch bring up its root queues,
// which is the path that asks the kernel for a workqueue. The semaphore keeps main alive until
// the block has run, so the worker's write is always in the trace.
#include <dispatch/dispatch.h>
#include <unistd.h>

int main(void) {
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    dispatch_queue_t q = dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0);
    dispatch_async(q, ^{
        write(1, "worker\n", 7);
        dispatch_semaphore_signal(sem);
    });
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    write(1, "done\n", 5);
    return 0;
}
