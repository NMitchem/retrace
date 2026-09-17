// M38. The exec fixture: execve(2) and posix_spawn(2)+POSIX_SPAWN_SETEXEC both RETURN to the
// guest with the refusal errno instead of replacing the image. Before M38 both were forwarded and
// failed only because argv/envp are untranslated guest pointers; a forwarded exec that ever
// stopped EFAULTing would replace retrace's own process. The two errnos are printed so the
// pre-fix run MEASURES what the forward returned (the refusal reproduces it for continuity, spec
// R4) and the post-fix run proves nothing the guest sees changed.
//
// Expected stdout: execve=<E>\nposix_spawn=<E>\n   (E = retrace_arch::exec_refusal_errno)
#include <stdio.h>
#include <unistd.h>
#include <errno.h>
#include <spawn.h>

int main(void) {
    char *argv[] = { "/bin/echo", "should-not-run", NULL };
    char *envp[] = { NULL };
    execve("/bin/echo", argv, envp);
    printf("execve=%d\n", errno);
    posix_spawnattr_t attr;
    posix_spawnattr_init(&attr);
    posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETEXEC);
    pid_t pid = 0;
    int rc = posix_spawn(&pid, "/bin/echo", NULL, &attr, argv, envp);
    printf("posix_spawn=%d\n", rc);
    fflush(stdout);
    return 0;
}
