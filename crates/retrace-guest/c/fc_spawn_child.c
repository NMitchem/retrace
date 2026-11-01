// M54 t2: a posix_spawn that starts a real child. retrace records one process, so this is a wall.
// Natively: three lines, exit 0.
#include <spawn.h>
#include <stdio.h>
#include <sys/wait.h>
extern char **environ;
int main(void) {
    printf("parent: spawning\n");
    fflush(stdout);
    pid_t pid;
    char *argv[] = {"/bin/echo", "child", NULL};
    int rc = posix_spawn(&pid, "/bin/echo", NULL, NULL, argv, environ);
    if (rc != 0) { printf("parent: spawn failed %d\n", rc); return 1; }
    int st = 0;
    waitpid(pid, &st, 0);
    printf("parent: child exited %d\n", WEXITSTATUS(st));
    return 0;
}
