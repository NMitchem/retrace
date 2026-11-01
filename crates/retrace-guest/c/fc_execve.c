// M54 t3, tier-1 fixture: the program replaces itself with `argv[1]` (the exec target's path) through a
// plain execve, with an argument that holds a space, an empty one, and the environment plus X=fc.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

extern char **environ;

int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: fc_execve <target>\n"); return 2; }
    char *nargv[] = { argv[1], "a b", "", NULL };
    size_t n = 0; while (environ[n]) n++;
    char **envp = malloc((n + 2) * sizeof *envp);
    for (size_t i = 0; i < n; i++) envp[i] = environ[i];
    envp[n] = "X=fc"; envp[n + 1] = NULL;
    execve(argv[1], nargv, envp);
    perror("execve");
    return 1;
}
