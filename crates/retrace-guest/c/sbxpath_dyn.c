// M47 fixture (t0, after H7): libsystem_sandbox's sandbox_container_path_for_pid, which issues
// __mac_syscall("Sandbox", 4, {pid, 0, buf, len}) — the policy would write through the NESTED buf. An
// unsandboxed process has no container: natively rc -1, errno ENOTSUP (45), buf untouched
// (docs/sweep-evidence/2026-09-30-m47-t0/call4native.out). Under retrace the call is modelled, never
// forwarded (M47 §3d, the operator's 2026-10-01 ruling).
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
