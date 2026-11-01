// M54 t4, tier-1 fixture: the program replaces itself with `argv[1]` (the exec target's path) through
// posix_spawn(POSIX_SPAWN_SETEXEC) with a dup2 of stdout to 7 and a close of it again, CLOEXEC_DEFAULT, and
// descriptors 0, 1 and 2 inherited. The target is given "spawned".
#include <spawn.h>
#include <stdio.h>
#include <unistd.h>

extern char **environ;

int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: fc_spawn_exec <target>\n"); return 2; }
    posix_spawnattr_t attr; posix_spawn_file_actions_t fa;
    posix_spawnattr_init(&attr); posix_spawn_file_actions_init(&fa);
    posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETEXEC | POSIX_SPAWN_CLOEXEC_DEFAULT);
    posix_spawn_file_actions_adddup2(&fa, 1, 7);
    posix_spawn_file_actions_addclose(&fa, 7);
    for (int fd = 0; fd < 3; fd++) posix_spawn_file_actions_addinherit_np(&fa, fd);
    char *nargv[] = { argv[1], "spawned", NULL };
    pid_t pid;
    int rc = posix_spawn(&pid, argv[1], &fa, &attr, nargv, environ);
    fprintf(stderr, "posix_spawn: errno %d\n", rc);
    return 1;
}
