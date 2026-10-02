// M47 t0 M2(b) fallback (no debugger on this host): issue natively the two Sandbox call-2 structs
// the census measured, byte for byte except the pointers and the pid, which are this process's own.
// Layouts from `retrace debug x` on a census-binary recording of hello_dyn (m2b-structs.txt):
//   dyld sandbox_check_common, "syscall-unix":
//     {+0 out* -> 40+ zero bytes on dyld's stack, +8 pid, +16 "syscall-unix", +24 0x41, +32 0x226, +40 1}
//   libsystem_sandbox rootless_check_trusted_internal, "file-write-data":
//     {+0 out* -> 32+ zero bytes, +8 1, +16 "file-write-data", +24 0xf0, +32 4, +40 0x20000005}
// Prints rc, errno, and whether the kernel wrote the +0 buffer.
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int __mac_syscall(const char *policy, int call, void *arg);

static void one(const char *tag, uint64_t f8, const char *op, uint64_t f24, uint64_t f32, uint64_t f40) {
    unsigned char out[64];
    memset(out, 0, sizeof out);
    uint64_t s[6] = { (uint64_t)(uintptr_t)out, f8, (uint64_t)(uintptr_t)op, f24, f32, f40 };
    errno = 0;
    int rc = __mac_syscall("Sandbox", 2, s);
    int e = errno;
    int touched = 0;
    for (unsigned i = 0; i < sizeof out; i++) if (out[i]) { touched = 1; break; }
    printf("%s: __mac_syscall(\"Sandbox\", 2, {out, %#llx, \"%s\", %#llx, %#llx, %#llx}) rc=%d errno=%d out_written=%d",
           tag, (unsigned long long)f8, op, (unsigned long long)f24, (unsigned long long)f32, (unsigned long long)f40,
           rc, rc ? e : 0, touched);
    if (touched) { printf(" out[0..40]="); for (int i = 0; i < 40; i++) printf("%02x", out[i]); }
    printf("\n");
}

int main(void) {
    one("dyld syscall-unix", (uint64_t)getpid(), "syscall-unix", 0x41, 0x226, 1);
    one("libsystem_sandbox file-write-data", 1, "file-write-data", 0xf0, 4, 0x20000005);
    return 0;
}
