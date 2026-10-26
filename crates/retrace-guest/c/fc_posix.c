// M53 t1 (spec §3b): a newcomer's POSIX file and process calls, in one run in one directory of its
// own: truncate, sync and lock; links and renames; a directory tree through a descriptor; modes,
// owners and flags; timestamps; clones; extended attributes; identity. It is posix_dyn.c's modes in
// sequence (that file's header says what each prints), then the removal of the directory.
#define POSIX_DYN_LIB
#include "posix_dyn.c"
#include <poll.h>
#include <sys/uio.h>

int main(void) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    strcpy(dir, "/tmp/fc_posix.XXXXXX");
    if (!mkdtemp(dir)) { printf("posix: mkdtemp failed\n"); return 2; }
    const char *modes[] = {"files", "links", "dirs", "modes", "times", "clone", "xattr", "ids"};
    for (unsigned i = 0; i < sizeof modes / sizeof *modes; i++) {
        printf("-- %s\n", modes[i]);
        run_mode(modes[i]);
    }
    printf("-- iovec\n");
    {
        // M53 t2 (C8): writev and readv, which a program that gathers its output or scatters its input
        // calls, are translated. One file in the directory, three entries each way.
        char path_[256];
        snprintf(path_, sizeof path_, "%s/iov", dir);
        int fd = open(path_, O_RDWR | O_CREAT | O_TRUNC, 0644);
        char a[3] = "abc", b[4] = "defg", c[5] = "hijkl";
        struct iovec w[3] = {{a, 3}, {b, 4}, {c, 5}};
        printf("writev rc=%ld\n", (long)writev(fd, w, 3));
        lseek(fd, 0, SEEK_SET);
        char x[3] = {0}, y[4] = {0}, z[5] = {0};
        struct iovec r[3] = {{x, 3}, {y, 4}, {z, 5}};
        printf("readv rc=%ld\n", (long)readv(fd, r, 3));
        printf("%.3s|%.4s|%.5s\n", x, y, z);
        close(fd);
        unlink(path_);
    }
    printf("-- poll\n");
    {
        // M53 t4 (C8): poll on a pipe, which an event loop's wakeup pipe is. Empty, then written.
        int pp[2];
        pipe(pp);
        struct pollfd pf = {pp[0], POLLIN, 0};
        printf("poll empty rc=%d\n", poll(&pf, 1, 0));
        write(pp[1], "x", 1);
        printf("poll ready rc=%d revents=0x%x\n", poll(&pf, 1, 0), (unsigned)pf.revents);
        close(pp[0]);
        close(pp[1]);
    }
    printf("removed=%s\n", unlinkat(AT_FDCWD, dir, AT_REMOVEDIR) == 0 ? "yes" : "no");
    return 0;
}
