// M47 t0, after H7: the native answer of the (Sandbox, 4) caller the census found,
// libsystem_sandbox`sandbox_container_path_for_pid (symbolicated statically, h7-call4.txt), for an
// unsandboxed ad-hoc process. Its __mac_syscall struct (retrace debug `x`, h7-call4.txt) is
// {u64 pid; u64 0; char *buf; u64 len = 0x400}: buf is a NESTED out-pointer.
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int sandbox_container_path_for_pid(pid_t pid, char *buffer, size_t bufsize);

int main(void) {
    char buf[1024];
    memset(buf, 0xAB, sizeof buf);
    errno = 0;
    int rc = sandbox_container_path_for_pid(getpid(), buf, sizeof buf);
    int e = errno;
    int touched = 0;
    for (unsigned i = 0; i < sizeof buf; i++) if ((unsigned char)buf[i] != 0xAB) { touched = 1; break; }
    printf("sandbox_container_path_for_pid rc=%d errno=%d buf_touched=%d\n", rc, e, touched);
    return 0;
}
