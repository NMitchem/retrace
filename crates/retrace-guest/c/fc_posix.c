// M53 t1 (spec §3b): a newcomer's POSIX file and process calls, in one run in one directory of its
// own: truncate, sync and lock; links and renames; a directory tree through a descriptor; modes,
// owners and flags; timestamps; clones; extended attributes; identity. It is posix_dyn.c's modes in
// sequence (that file's header says what each prints), then the removal of the directory.
#define POSIX_DYN_LIB
#include "posix_dyn.c"

int main(void) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    strcpy(dir, "/tmp/fc_posix.XXXXXX");
    if (!mkdtemp(dir)) { printf("posix: mkdtemp failed\n"); return 2; }
    const char *modes[] = {"files", "links", "dirs", "modes", "times", "clone", "xattr", "ids"};
    for (unsigned i = 0; i < sizeof modes / sizeof *modes; i++) {
        printf("-- %s\n", modes[i]);
        run_mode(modes[i]);
    }
    printf("removed=%s\n", unlinkat(AT_FDCWD, dir, AT_REMOVEDIR) == 0 ? "yes" : "no");
    return 0;
}
