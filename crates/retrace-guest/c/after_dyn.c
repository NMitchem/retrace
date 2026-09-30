// M46. The dispatch_after fixture (spec §3f; plan R7). argv[1] selects the mode:
//   (none)  dispatch_after 100 ms onto the default global queue. Main first prints
//           "fired cell <address>", so gcdtimer_e2e can watch the handler's store. The handler
//           stores 1 into `fired_cell`, writes "fired\n" and signals main, which writes "done\n".
//   two     B is registered at 200 ms, then A at 100 ms. They must print A then B. One kernel
//           timer serves the bucket, so B's arm follows A's fire through KEVENT_RETURN.
//   wall    dispatch_after on the WALL clock (dispatch_walltime). retrace must refuse its arm.
//   clock   mach_get_times against mach_absolute_time. It prints "clock ok" iff the absolute time
//           mach_get_times returns lies between two mach_absolute_time reads around it (R7).
// Native stdout (M46 t0): "fired cell 0x…\nfired\ndone\n", "A\nB\ndone\n", "fired\ndone\n",
// "clock ok\n".
#include <dispatch/dispatch.h>
#include <dlfcn.h>
#include <mach/mach_time.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

// The handler's store, and the cell gcdtimer_e2e watches. 8 bytes and 8-aligned, so one
// watchpoint covers it whole.
volatile uint64_t fired_cell;

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "";
    if (strcmp(mode, "clock") == 0) {
        // Private (xnu libsyscall/wrappers/mach_get_times.c) and absent from the SDK's headers, so
        // it is looked up rather than declared and linked. It returns a kern_return_t, an int.
        int (*get_times)(uint64_t *, uint64_t *, struct timespec *) =
            (int (*)(uint64_t *, uint64_t *, struct timespec *))dlsym(RTLD_DEFAULT, "mach_get_times");
        if (get_times == NULL) return 3;
        uint64_t before = mach_absolute_time(), abs = 0, cont = 0;
        struct timespec ts;
        if (get_times(&abs, &cont, &ts) != 0) return 4;
        uint64_t after = mach_absolute_time();
        printf("clock %s\n", before <= abs && abs <= after ? "ok" : "bad");
        return 0;
    }
    dispatch_queue_t q = dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0);
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    if (strcmp(mode, "") == 0) {
        printf("fired cell %p\n", (void *)&fired_cell);
        fflush(stdout);
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 100 * NSEC_PER_MSEC), q, ^{
            fired_cell = 1;
            write(1, "fired\n", 6);
            dispatch_semaphore_signal(sem);
        });
        dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    } else if (strcmp(mode, "two") == 0) {
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 200 * NSEC_PER_MSEC), q, ^{
            write(1, "B\n", 2);
            dispatch_semaphore_signal(sem);
        });
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 100 * NSEC_PER_MSEC), q, ^{
            write(1, "A\n", 2);
            dispatch_semaphore_signal(sem);
        });
        dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
        dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    } else if (strcmp(mode, "wall") == 0) {
        dispatch_after(dispatch_walltime(NULL, 100 * NSEC_PER_MSEC), q, ^{
            write(1, "fired\n", 6);
            dispatch_semaphore_signal(sem);
        });
        dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    } else {
        fprintf(stderr, "after_dyn: unknown mode %s\n", mode);
        return 2;
    }
    write(1, "done\n", 5);
    return 0;
}
