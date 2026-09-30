// M46. The repeating timer-source fixture (spec §3f): a DISPATCH_SOURCE_TYPE_TIMER with a 50 ms
// interval prints "tick 1" to "tick 3", cancels itself on the third and signals main, which writes
// "done\n". It exercises the re-arm after every fire, manager reuse across fires, and the disarm
// the cancel issues for the armed fourth tick.
// Native stdout (M46 t0): "tick 1\ntick 2\ntick 3\ndone\n".
#include <dispatch/dispatch.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    dispatch_source_t t = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0,
                                                 dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0));
    __block int ticks = 0;
    dispatch_source_set_timer(t, dispatch_time(DISPATCH_TIME_NOW, 50 * NSEC_PER_MSEC), 50 * NSEC_PER_MSEC, 0);
    dispatch_source_set_event_handler(t, ^{
        char line[16];
        int n = snprintf(line, sizeof line, "tick %d\n", ++ticks);
        write(1, line, (size_t)n);
        if (ticks == 3) {
            dispatch_source_cancel(t);
            dispatch_semaphore_signal(sem);
        }
    });
    dispatch_resume(t);
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    write(1, "done\n", 5);
    return 0;
}
