// The M52 fast-follow's fixture: gettimeofday, many times, with its timeval on the stack. Under retrace the frozen
// commpage sends each call to syscall 116, whose out-pointers have no fixed size. Before the fast-follow the recorder
// stored each call's whole diff window, the rest of the stack's backing (about 24 KB), for the 16 bytes the kernel
// wrote. parking_lot's queue test made 400 000 such calls and filled the disk.
#include <stdio.h>
#include <sys/time.h>
int main(void) {
    struct timeval tv;
    long ok = 0;
    for (int i = 0; i < 1000; i++) ok += gettimeofday(&tv, 0) == 0 && tv.tv_sec > 0;
    printf("tvloop %ld\n", ok);
    return 0;
}
