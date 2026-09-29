// M45 t0 M2 candidate: a DISPATCH_SOURCE_TYPE_SIGNAL source for SIGUSR1, then raise it.
#include <dispatch/dispatch.h>
#include <signal.h>
#include <unistd.h>

int main(void) {
    signal(SIGUSR1, SIG_IGN);
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    dispatch_source_t s = dispatch_source_create(DISPATCH_SOURCE_TYPE_SIGNAL, SIGUSR1, 0,
                                                 dispatch_get_global_queue(0, 0));
    dispatch_source_set_event_handler(s, ^{
        write(1, "fired\n", 6);
        dispatch_semaphore_signal(sem);
    });
    dispatch_resume(s);
    raise(SIGUSR1);
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    write(1, "done\n", 5);
    return 0;
}
