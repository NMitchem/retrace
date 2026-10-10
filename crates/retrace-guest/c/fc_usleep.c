// M49 first-contact corpus (spec §3b), row S: usleep(20000), the libc path that calls
// nanosleep(&ts, NULL) and so goes straight to __semwait_signal (334) (spec §2e). Prints whether
// the guest's own mach_absolute_time advanced by at least the 20 ms it asked for.
#include <mach/mach_time.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    mach_timebase_info_data_t tb;
    mach_timebase_info(&tb);
    puts("sleeping 20 ms");
    fflush(stdout);
    uint64_t t0 = mach_absolute_time();
    int rc = usleep(20000);
    uint64_t ns = (mach_absolute_time() - t0) * tb.numer / tb.denom;
    printf("rc=%d ok=%d\n", rc, ns >= 20000000ull);
    return 0;
}
