// M49 first-contact corpus (spec §3b), row S: sleep(1), the libc path that calls
// nanosleep(&ts, &rem), which issues clock_get_time (mach_msg2 1000) and then __semwait_signal
// (334) (spec §2e). It prints whether the guest's own mach_absolute_time advanced by at least the
// second it asked for: "1" natively, and under retrace once the idle jump reaches the deadline.
#include <mach/mach_time.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    mach_timebase_info_data_t tb;
    mach_timebase_info(&tb);
    puts("sleeping 1 s");
    fflush(stdout);
    uint64_t t0 = mach_absolute_time();
    unsigned left = sleep(1);
    uint64_t ns = (mach_absolute_time() - t0) * tb.numer / tb.denom;
    printf("left=%u ok=%d\n", left, ns >= 1000000000ull);
    return 0;
}
