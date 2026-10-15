// M51 (spec §3h): a pointer the program could not pass natively, picked by argv[1]. Each mode prints one
// line naming the call, its return and errno (or kern_return) to stdout, then exits 0.
//   readunmapped  read(fd, hole, 16): a read from /dev/zero into a page the program unmapped
//   readcode      read(fd, code, 16): the same into the page holding main; then whether those bytes changed
//   statbad       stat("/", hole)
//   consolebad    write(1, hole, 8): the console write, which retrace services itself
//   altstackbad   sigaltstack(hole, NULL) by raw svc, which retrace services itself. libc's wrapper
//                 dereferences `ss` before the trap, so through libc the program faults in itself and the
//                 kernel never answers (t0 M5, Ruling T0-l); the raw call is the kernel's own answer.
//   sigactionnew / sigactionold / sigprocmaskset / sigprocmaskold / sigpendingbad / getrlimitbad /
//   mach2bad      the serviced calls' unreachable pointer by raw svc (t0's m6_probes.c shapes); each prints its line
//   straddle      a 24-byte sigaltstack ss starting 8 bytes before the end of a mapped page, the next unmapped
//   machbad       _kernelrpc_mach_vm_allocate_trap(mach_task_self(), hole, 16384, VM_FLAGS_ANYWHERE)
// `hole` is a page the program maps and then unmaps, so nothing maps it natively or under retrace.
// `machbad` calls the trap, not `mach_vm_allocate`, so the answer is the trap's own (KERN_MEMORY_ERROR,
// t0 M5) whatever the wrapper does with it.
#include <errno.h>
#include <fcntl.h>
#include <mach/mach.h>
#include <mach/mach_traps.h>
#include <mach/mach_vm.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/resource.h>
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

// A three-argument BSD call by hand, past libc's own dereference (t0's m6_probes.c shape).
static long raw3(long num, long a, long b, long c) {
    register long x0 __asm__("x0") = a;
    register long x1 __asm__("x1") = b;
    register long x2 __asm__("x2") = c;
    register long x16 __asm__("x16") = num;
    long carry;
    __asm__ volatile("svc #0x80\n cset %1, cs" : "+r"(x0), "=&r"(carry) : "r"(x1), "r"(x2), "r"(x16) : "memory", "cc");
    if (carry) { errno = (int)x0; return -1; }
    return x0;
}

// mach_msg2_trap by hand with a header at `data`: a 24-byte send to the task port, as t0's mm2_kobj.
static long raw_mach_msg2(long data) {
    register long x0 __asm__("x0") = data;
    register long x1 __asm__("x1") = (long)(0x1ul | 0x2ul | 0x200000000ul);
    register long x2 __asm__("x2") = (24l << 32) | 0x1513;
    register long x3 __asm__("x3") = (long)mach_task_self();
    register long x4 __asm__("x4") = 3405l << 32;
    register long x5 __asm__("x5") = 0l;
    register long x6 __asm__("x6") = 256;
    register long x7 __asm__("x7") = 0;
    register long x16 __asm__("x16") = -47;
    __asm__ volatile("svc #0x80" : "+r"(x0) : "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x5), "r"(x6), "r"(x7), "r"(x16) : "memory", "cc");
    return x0;
}

// mach_msg2_trap by hand for a task_get_special_port (3409) request whose 36 bytes are at `data`; the
// header is in the registers as well as the buffer. `reply` is the reply port the answer is received on.
static long raw_mach_msg2_3409(long data, mach_port_t reply) {
    register long x0 __asm__("x0") = data;
    register long x1 __asm__("x1") = (long)(0x1ul | 0x2ul | 0x200000000ul);
    register long x2 __asm__("x2") = (36l << 32) | 0x1513;
    register long x3 __asm__("x3") = (long)mach_task_self() | ((long)reply << 32);
    register long x4 __asm__("x4") = 3409l << 32;
    register long x5 __asm__("x5") = (long)reply << 32;
    register long x6 __asm__("x6") = 256;
    register long x7 __asm__("x7") = 0;
    register long x16 __asm__("x16") = -47;
    __asm__ volatile("svc #0x80" : "+r"(x0) : "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x5), "r"(x6), "r"(x7), "r"(x16) : "memory", "cc");
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
    else if (!strcmp(m, "sigactionnew")) { rc = raw3(46, SIGUSR1, (long)hole, 0); report("sigaction(new hole)", rc, errno); }
    else if (!strcmp(m, "sigactionold")) { rc = raw3(46, SIGUSR1, 0, (long)hole); report("sigaction(old hole)", rc, errno); }
    else if (!strcmp(m, "sigprocmaskset")) { rc = raw3(48, 1, (long)hole, 0); report("sigprocmask(set hole)", rc, errno); }
    else if (!strcmp(m, "sigprocmaskold")) {
        // A bad oldset is swallowed: the call returns 0 and the new mask still applies.
        sigset_t s, q;
        sigemptyset(&s); sigaddset(&s, SIGUSR2);
        rc = raw3(48, 1, (long)&s, (long)hole);
        report("sigprocmask(oldset hole)", rc, errno);
        sigemptyset(&q); sigprocmask(0, NULL, &q);
        say(sigismember(&q, SIGUSR2) ? "blocked=1\n" : "blocked=0\n");
    }
    else if (!strcmp(m, "sigpendingbad")) { rc = raw3(52, (long)hole, 0, 0); report("sigpending(hole)", rc, errno); }
    else if (!strcmp(m, "getrlimitbad")) { rc = raw3(194, RLIMIT_STACK | 0x1000, (long)hole, 0); report("getrlimit(hole)", rc, errno); }
    else if (!strcmp(m, "mach2bad")) {
        char b[64];
        int n = snprintf(b, sizeof b, "mach_msg2(hole): ret=0x%lx\n", (unsigned long)raw_mach_msg2((long)hole));
        write(1, b, n);
    }
    else if (!strcmp(m, "statcode")) {
        // stat("/", <the page holding main>): the out-struct would land in the program's own code.
        unsigned char *code = (unsigned char *)((uintptr_t)&main & ~(uintptr_t)0x3fff);
        unsigned char before[16], after[16];
        memcpy(before, code, 16);
        rc = stat("/", (struct stat *)code);
        int e = errno;
        memcpy(after, code, 16);
        report("stat(code)", rc, e);
        say(memcmp(before, after, 16) ? "unchanged=0\n" : "unchanged=1\n");
    }
    else if (!strcmp(m, "mach3409edge")) {
        // A 36-byte task_get_special_port request ending exactly at a page edge, the next page unmapped:
        // its 40-byte reply does not fit.
        char *two = mmap(NULL, 32768, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
        munmap(two + 16384, 16384);
        uint32_t *msg = (uint32_t *)(two + 16384 - 36);
        mach_port_t reply = mig_get_reply_port();
        msg[0] = 0x1513; msg[1] = 36; msg[2] = mach_task_self(); msg[3] = reply; msg[4] = 0; msg[5] = 3409;
        msg[6] = 0; msg[7] = 0x00000001; msg[8] = 10; // NDR record, then which_port
        char b[64];
        int n = snprintf(b, sizeof b, "mach_msg2(3409 at edge): ret=0x%lx\n", (unsigned long)raw_mach_msg2_3409((long)msg, reply));
        write(1, b, n);
    }
    else if (!strcmp(m, "nonemmap") || !strcmp(m, "noneprotect") || !strcmp(m, "loadnone")) {
        // A page the program has not made accessible: PROT_NONE from mmap, or a vm_allocate made PROT_NONE.
        char *p;
        if (!strcmp(m, "noneprotect")) {
            mach_vm_address_t a = 0;
            mach_vm_allocate(mach_task_self(), &a, 16384, VM_FLAGS_ANYWHERE);
            mach_vm_protect(mach_task_self(), a, 16384, 0, VM_PROT_NONE);
            p = (char *)a;
        } else {
            p = mmap(NULL, 16384, PROT_NONE, MAP_ANON | MAP_PRIVATE, -1, 0);
        }
        if (!strcmp(m, "loadnone")) {
            volatile char c = *(volatile char *)p;
            (void)c;
            say("loaded\n");
        } else {
            rc = read(fd, p, 16);
            report(m, rc, errno);
        }
    }
    else if (!strcmp(m, "straddle")) {
        // A 24-byte stack_t that starts 8 bytes before the end of a mapped page whose next page is unmapped.
        char *two = mmap(NULL, 32768, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
        munmap(two + 16384, 16384);
        rc = raw_sigaltstack(two + 16384 - 8, NULL);
        report("sigaltstack(straddle)", rc, errno);
    }
    else if (!strcmp(m, "machbad")) {
        kern_return_t kr = _kernelrpc_mach_vm_allocate_trap(mach_task_self(), (mach_vm_offset_t *)hole, 16384,
                                                            VM_FLAGS_ANYWHERE);
        char b[64];
        int n = snprintf(b, sizeof b, "mach_vm_allocate_trap(hole): kr=0x%x\n", kr);
        write(1, b, n);
    } else { say("fid_badptr: unknown mode\n"); return 2; }
    return 0;
}
