// M47 t0 M2(b) fallback (no debugger on this host): a DYLD_INSERT_LIBRARIES interposer of
// libsystem_kernel's __mac_syscall, so every call a NATIVE process makes through it from another
// image (libsystem_sandbox's rootless_check_trusted_internal and sandbox_container_path_for_pid,
// libsystem_trace's AMFI call) is logged with its policy, call, the first 48 bytes of its argument
// struct, the string at *(arg + 16) for Sandbox, the first 48 bytes at *(arg + 0) before and after,
// and the real return and errno. dyld's own __mac_syscall copy is internal to dyld and cannot be
// interposed; m2bnative.c reproduces those calls from their measured bytes instead.
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int __mac_syscall(const char *policy, int call, void *arg);

static void hex(const char *tag, const void *p, size_t n) {
    char line[512]; size_t o = 0;
    o += snprintf(line + o, sizeof line - o, "[m2b]   %s:", tag);
    for (size_t i = 0; i < n && o < sizeof line - 4; i++) o += snprintf(line + o, sizeof line - o, " %02x", ((const unsigned char *)p)[i]);
    snprintf(line + o, sizeof line - o, "\n");
    write(2, line, strlen(line));
}

static int my_mac_syscall(const char *policy, int call, void *arg) {
    unsigned char before[48] = {0}, after[48] = {0};
    const unsigned char *a = arg;
    void *p0 = arg ? *(void **)arg : NULL;
    int sandbox = policy && strcmp(policy, "Sandbox") == 0;
    if (sandbox && call == 2 && p0) memcpy(before, p0, sizeof before);
    int rc = __mac_syscall(policy, call, arg);
    int e = errno;
    if (sandbox && call == 2 && p0) memcpy(after, p0, sizeof after);
    char line[512];
    snprintf(line, sizeof line, "[m2b] __mac_syscall(\"%s\", %#x, %p) rc=%d errno=%d caller=%p\n",
             policy ? policy : "(null)", call, arg, rc, rc ? e : 0, __builtin_return_address(0));
    write(2, line, strlen(line));
    if (arg) hex("arg[0..48]", a, 48);
    if (sandbox && call == 2 && arg) {
        const char *op = *(const char **)(a + 16);
        snprintf(line, sizeof line, "[m2b]   op=\"%s\" *(arg+0) changed=%d\n", op ? op : "(null)", memcmp(before, after, sizeof before) != 0);
        write(2, line, strlen(line));
        if (p0) hex("*(arg+0)[0..48] after", after, 48);
    }
    errno = e;
    return rc;
}

__attribute__((used)) static struct { const void *replacement; const void *replacee; } interposers[]
    __attribute__((section("__DATA,__interpose"))) = { { (const void *)my_mac_syscall, (const void *)__mac_syscall } };
