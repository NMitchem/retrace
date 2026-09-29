// M45 t0 M2 candidate: a DISPATCH_SOURCE_TYPE_TIMER source, one shot, 1 ms.
#include <dispatch/dispatch.h>
#include <unistd.h>

int main(void) {
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    dispatch_source_t t = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0,
                                                 dispatch_get_global_queue(0, 0));
    dispatch_source_set_timer(t, dispatch_time(DISPATCH_TIME_NOW, 1000000), DISPATCH_TIME_FOREVER, 0);
    dispatch_source_set_event_handler(t, ^{
        write(1, "fired\n", 6);
        dispatch_semaphore_signal(sem);
    });
    dispatch_resume(t);
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    write(1, "done\n", 5);
    return 0;
}
