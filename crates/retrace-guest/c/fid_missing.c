// M51 (spec §3g): calls a function from a dylib build.rs deletes after linking, so dyld cannot load
// it and ends the launch through abort_with_payload (natively SIGABRT, 134, after dyld's message on
// stderr).
#include <stdio.h>

int fid_gone_value(void);

int main(void) {
    printf("%d\n", fid_gone_value());
    return 0;
}
