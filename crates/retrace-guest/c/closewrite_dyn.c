// M37 fix round 1 (C1). Closes a console fd and writes to it afterwards — the daemonize idiom.
// The kernel answers EBADF to a write on a closed descriptor; retrace must too, on BOTH sides.
// Before the fix, record kept slot 1 as the console after its faked close (so the write was
// mirrored and succeeded) while replay retired the slot (so it was not) — two stdouts, rc 0 on
// both, no divergence.
//
// Expected stdout (record == replay, bit for bit): before\nerr\n   (fd 1 and fd 2 fold into one
// mirrored buffer). Exit 0 only if both post-close writes fail with EBADF.
#include <unistd.h>
#include <errno.h>

int main(void) {
    write(1, "before\n", 7);
    close(1);
    int r1 = write(1, "after1\n", 7);
    int e1 = errno;
    write(2, "err\n", 4);
    close(2);
    int r2 = write(2, "after2\n", 7);
    int e2 = errno;
    return (r1 == -1 && e1 == EBADF && r2 == -1 && e2 == EBADF) ? 0 : 1;
}
