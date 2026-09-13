// M37. The dup2 fixture: aliases of the console stay console writes, a console slot displaced by
// a file becomes a file write, dup2 onto an open slot displaces it, self-dup2 is a no-op, and a
// closed source is EBADF. argv[1] is a file path the test owns.
//
// Expected stdout (record == replay, bit for bit):   alias\nself=1\nebadf=1\n
// Expected file after record:                        file18\nfile17\nvia1\n
#include <stdio.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <string.h>

int main(int argc, char **argv) {
    if (argc < 2) return 2;
    int f = open(argv[1], O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (f < 0) return 3;
    dup2(1, 17); write(17, "alias\n", 6);        /* console alias: mirrored, not forwarded */
    dup2(f, 18); write(18, "file18\n", 7);        /* plain duplicate: the file */
    dup2(f, 17); write(17, "file17\n", 7);        /* displaces the alias: the file now */
    int s = dup2(f, f);
    printf("self=%d\n", s == f);
    int e = dup2(40, 19);
    printf("ebadf=%d\n", e == -1 && errno == EBADF);
    fflush(stdout);
    dup2(f, 1);                                   /* stdout IS the file from here */
    printf("via1\n");
    fflush(stdout);
    close(17); close(18); close(f);
    return 0;
}
