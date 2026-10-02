// M47 fixture (spec §3f): the path calls git's `add` and `commit` issue — mkdir, chdir, link and
// utimes, each with no arg_kinds row before M47, plus rename, which has had one since M44 — in the
// order git issues them. <dir> must exist and be empty. Prints markers, and the stat'd mtime and
// link count of the renamed file, which only a forwarded utimes and link can produce.
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 2) { fprintf(stderr, "usage: fsops_dyn <dir>\n"); return 2; }
    char d[1024];
    snprintf(d, sizeof d, "%s/d", argv[1]);
    if (mkdir(d, 0755) != 0) { perror("mkdir"); return 1; }
    if (chdir(d) != 0) { perror("chdir"); return 1; }
    int fd = open("f", O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (fd < 0) { perror("open"); return 1; }
    if (write(fd, "fsops\n", 6) != 6) { perror("write"); return 1; }
    close(fd);
    if (link("f", "g") != 0) { perror("link"); return 1; }
    if (rename("g", "h") != 0) { perror("rename"); return 1; }
    struct timeval tv[2] = { { 1000000000, 0 }, { 1234567890, 0 } };
    if (utimes("h", tv) != 0) { perror("utimes"); return 1; }
    struct stat st;
    if (stat("h", &st) != 0) { perror("stat"); return 1; }
    printf("mkdir chdir link rename utimes ok\n");
    printf("h mtime=%ld nlink=%d\n", (long)st.st_mtimespec.tv_sec, (int)st.st_nlink);
    return 0;
}
