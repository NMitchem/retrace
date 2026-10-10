// M49 first-contact corpus (spec §3b): ordinary stdio file I/O. Creates a temporary file with
// mkstemp, writes lines with fprintf, reads them back with fgets, checks the size with stat, and
// unlinks it. Only contents and sizes are printed: the temporary name is random.
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    char path[] = "/tmp/fc_fileio.XXXXXX";
    int fd = mkstemp(path);
    if (fd < 0) { puts("mkstemp failed"); return 2; }
    FILE *f = fdopen(fd, "w");
    for (int i = 1; i <= 3; i++) fprintf(f, "line %d of 3\n", i);
    fclose(f);
    struct stat st;
    if (stat(path, &st) != 0) { puts("stat failed"); return 2; }
    printf("size=%lld\n", (long long)st.st_size);
    f = fopen(path, "r");
    char buf[64];
    while (fgets(buf, sizeof buf, f)) printf("read: %s", buf);
    fclose(f);
    printf("unlink=%d\n", unlink(path));
    return 0;
}
