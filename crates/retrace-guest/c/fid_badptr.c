// M51 (spec §3h): a pointer the program could not pass natively, picked by argv[1]. Each mode prints one
// line naming the call, its return and errno (or kern_return) to stdout, then exits 0.
//   readunmapped  read(fd, hole, 16): a read from /dev/zero into a page the program unmapped
//   readcode      read(fd, code, 16): the same into the page holding main; then whether those bytes changed
//   statbad       stat("/", hole)
//   consolebad    write(1, hole, 8): the console write, which retrace services itself
//   altstackbad   sigaltstack(hole, NULL) by raw svc, which retrace services itself. libc's wrapper
//                 dereferences `ss` before the trap, so through libc the program faults in itself and the
//                 kernel never answers (t0 M5, Ruling T0-l); the raw call is the kernel's own answer.
//   machbad       _kernelrpc_mach_vm_allocate_trap(mach_task_self(), hole, 16384, VM_FLAGS_ANYWHERE)
// `hole` is a page the program maps and then unmaps, so nothing maps it natively or under retrace.
// `machbad` calls the trap, not `mach_vm_allocate`, so the answer is the trap's own (KERN_MEMORY_ERROR,
// t0 M5) whatever the wrapper does with it.
#include <errno.h>
#include <fcntl.h>
#include <mach/mach.h>
#include <mach/mach_traps.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

static void say(const char *s) { write(1, s, strlen(s)); }

static void report(const char *what, long rc, int err) {
    char b[96];
    int n = snprintf(b, sizeof b, "%s: rc=%ld errno=%d\n", what, rc, rc < 0 ? err : 0);
    write(1, b, n);
}

// sigaltstack (53) issued by hand: x16 = 53, `svc #0x80`, carry set means x0 is the errno.
static long raw_sigaltstack(void *ss, void *oss) {
    register long x0 __asm__("x0") = (long)ss;
    register long x1 __asm__("x1") = (long)oss;
    register long x16 __asm__("x16") = 53;
    long carry;
    __asm__ volatile("svc #0x80\n cset %1, cs" : "+r"(x0), "=&r"(carry) : "r"(x1), "r"(x16) : "memory", "cc");
    if (carry) { errno = (int)x0; return -1; }
    return x0;
}

int main(int argc, char **argv) {
    const char *m = argc > 1 ? argv[1] : "";
    void *hole = mmap(NULL, 16384, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    munmap(hole, 16384);
    int fd = open("/dev/zero", O_RDONLY);
    long rc;
    if (!strcmp(m, "readunmapped")) { rc = read(fd, hole, 16); report("read(hole)", rc, errno); }
    else if (!strcmp(m, "readcode")) {
        unsigned char *code = (unsigned char *)((uintptr_t)&main & ~(uintptr_t)0x3fff);
        unsigned char before[16], after[16];
        memcpy(before, code, 16);
        rc = read(fd, code, 16);
        int e = errno;
        memcpy(after, code, 16);
        report("read(code)", rc, e);
        say(memcmp(before, after, 16) ? "unchanged=0\n" : "unchanged=1\n");
    } else if (!strcmp(m, "statbad")) { rc = stat("/", (struct stat *)hole); report("stat(hole)", rc, errno); }
    else if (!strcmp(m, "consolebad")) { rc = write(1, hole, 8); report("write(1, hole)", rc, errno); }
    else if (!strcmp(m, "altstackbad")) { rc = raw_sigaltstack(hole, NULL); report("sigaltstack(hole)", rc, errno); }
    else if (!strcmp(m, "machbad")) {
        kern_return_t kr = _kernelrpc_mach_vm_allocate_trap(mach_task_self(), (mach_vm_offset_t *)hole, 16384,
                                                            VM_FLAGS_ANYWHERE);
        char b[64];
        int n = snprintf(b, sizeof b, "mach_vm_allocate_trap(hole): kr=0x%x\n", kr);
        write(1, b, n);
    } else { say("fid_badptr: unknown mode\n"); return 2; }
    return 0;
}
