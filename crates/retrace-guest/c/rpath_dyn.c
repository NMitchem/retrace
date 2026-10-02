// M47 fixture (spec §3f): a guest that links a dylib by @rpath (LC_RPATH @executable_path). dyld
// refuses the load unless AMFI's dyld policy allows @-path expansion, so reaching main at all is the
// AMFI answer arriving; the marker proves the dylib's code ran.
#include <stdio.h>

int rpath_marker(void);

int main(void) {
    printf("rpath marker=%d\n", rpath_marker());
    return 0;
}
