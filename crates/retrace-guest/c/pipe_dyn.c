// M38. The pipe fixture: both ends reach the guest as ITS OWN numbers, adjacent, read end first,
// and bytes written into the write end come back out of the read end. Prints invariants rather
// than absolute numbers (fdtable_dyn's lesson: libSystem holds one extra descriptor under
// retrace, so the first free slot is 4, not 3 — an absolute number tests libSystem, not the table).
//
// Expected stdout (record == replay, bit for bit):   pair=1\nlow=1\nbytes=pipe\n
#include <stdio.h>
#include <unistd.h>
#include <string.h>

int main(void) {
    int p[2] = { -1, -1 };
    if (pipe(p) != 0) { printf("pipe failed\n"); return 3; }
    printf("pair=%d\n", p[1] == p[0] + 1);              /* write end is the next slot */
    printf("low=%d\n", p[0] >= 3 && p[1] < 16);         /* guest numbers, not retrace's */
    char buf[8] = {0};
    if (write(p[1], "pipe", 4) != 4) { printf("write failed\n"); return 4; }
    if (read(p[0], buf, 4) != 4) { printf("read failed\n"); return 5; }
    printf("bytes=%s\n", buf);
    fflush(stdout);
    close(p[0]); close(p[1]);
    return 0;
}
