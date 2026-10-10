// M49 first-contact corpus (spec §3b), row S: nanosleep(&ts, NULL) for 30 ms, the shape the
// investigation measured (sleep-nano.c): straight to __semwait_signal (334) (spec §2e). Prints
// whether the guest's own mach_absolute_time advanced by at least the 30 ms it asked for.
#include <mach/mach_time.h>
#include <stdio.h>
#include <time.h>

int main(void) {
    mach_timebase_info_data_t tb;
    mach_timebase_info(&tb);
    puts("sleeping 30 ms");
    fflush(stdout);
    struct timespec req = {0, 30000000};
    uint64_t t0 = mach_absolute_time();
    int rc = nanosleep(&req, NULL);
    uint64_t ns = (mach_absolute_time() - t0) * tb.numer / tb.denom;
    printf("rc=%d ok=%d\n", rc, ns >= 30000000ull);
    return 0;
}
