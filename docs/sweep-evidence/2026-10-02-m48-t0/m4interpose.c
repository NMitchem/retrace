// t0 M4 instrument, in place of the brief's lldb breakpoints. lldb cannot launch a process in this
// session: DevToolsSecurity is disabled and system.privilege.taskport needs authenticate-user, an
// interactive prompt (m4-lldb-launchtest.out). This dylib is loaded with DYLD_INSERT_LIBRARIES and
// interposes libsystem_kernel's psynch stubs, which libsystem_pthread imports. Each wrapper issues the
// same syscall itself (mov x16, #N; svc #0x80, as the stub does), so it sees what the stub sees at
// stub + 8: the kernel's raw x0 and the carry flag (the stub's b.lo). It logs the eight argument
// registers and that raw return, then returns as the stub does: carry clear, x0; carry set,
// errno = x0 and -1 (cerror). When x6 == 0 and x7 == 1 on a cvwait (the {0, 1 ns} shape) it also
// logs eight return addresses, symbolised with dladdr: the brief's `bt 8` on that condition.
#include <dlfcn.h>
#include <errno.h>
#include <execinfo.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

extern uint32_t __psynch_cvwait(void);
extern uint32_t __psynch_cvsignal(void);
extern uint32_t __psynch_cvbroad(void);
extern uint32_t __psynch_mutexwait(void);
extern uint32_t __psynch_mutexdrop(void);
extern uint32_t __psynch_cvclrprepost(void);

static uint64_t raw(uint64_t num, uint64_t a[8], int *carry) {
    register uint64_t x0 __asm__("x0") = a[0], x1 __asm__("x1") = a[1], x2 __asm__("x2") = a[2],
        x3 __asm__("x3") = a[3], x4 __asm__("x4") = a[4], x5 __asm__("x5") = a[5],
        x6 __asm__("x6") = a[6], x7 __asm__("x7") = a[7], x16 __asm__("x16") = num;
    uint32_t c;
    __asm__ volatile("svc #0x80\n\tcset %w[c], cs"
                     : "+r"(x0), "+r"(x1), "+r"(x2), "+r"(x3), "+r"(x4), "+r"(x5), "+r"(x6), "+r"(x7),
                       "+r"(x16), [c] "=r"(c)
                     :
                     : "memory", "cc");
    *carry = (int)c;
    return x0;
}

static void say(const char *s) { write(2, s, strlen(s)); }

static uint64_t tid(void) { uint64_t t = 0; pthread_threadid_np(NULL, &t); return t; }

static uint64_t call(const char *name, uint64_t num, uint64_t a[8]) {
    char buf[512];
    snprintf(buf, sizeof buf, "[m4] tid=%llu %s enter x0=%#llx x1=%#llx x2=%#llx x3=%#llx x4=%#llx x5=%#llx x6=%#llx x7=%#llx\n",
             (unsigned long long)tid(), name, (unsigned long long)a[0], (unsigned long long)a[1], (unsigned long long)a[2],
             (unsigned long long)a[3], (unsigned long long)a[4], (unsigned long long)a[5], (unsigned long long)a[6],
             (unsigned long long)a[7]);
    say(buf);
    if (num == 305 && a[6] == 0 && (uint32_t)a[7] == 1) {
        void *f[9];
        int n = backtrace(f, 9);
        for (int i = 1; i < n; i++) {
            Dl_info d;
            const char *img = "?", *sym = "?";
            uintptr_t off = 0;
            if (dladdr(f[i], &d)) {
                if (d.dli_fname) { img = strrchr(d.dli_fname, '/') ? strrchr(d.dli_fname, '/') + 1 : d.dli_fname; }
                if (d.dli_sname) { sym = d.dli_sname; off = (uintptr_t)f[i] - (uintptr_t)d.dli_saddr; }
            }
            snprintf(buf, sizeof buf, "[m4]   frame #%d %p %s`%s + %lu\n", i - 1, f[i], img, sym, (unsigned long)off);
            say(buf);
        }
    }
    int carry;
    uint64_t r = raw(num, a, &carry);
    snprintf(buf, sizeof buf, "[m4] tid=%llu %s return x0=%#llx carry=%d\n", (unsigned long long)tid(), name,
             (unsigned long long)r, carry);
    say(buf);
    if (carry) { errno = (int)r; return (uint64_t)-1; }
    return r;
}

#define WRAP(fn, num)                                                                              \
    static uint64_t my_##fn(uint64_t a0, uint64_t a1, uint64_t a2, uint64_t a3, uint64_t a4,       \
                            uint64_t a5, uint64_t a6, uint64_t a7) {                               \
        uint64_t a[8] = {a0, a1, a2, a3, a4, a5, a6, a7};                                          \
        return call(#fn, num, a);                                                                  \
    }                                                                                              \
    __attribute__((used)) static struct { const void *rep, *orig; } interpose_##fn                 \
        __attribute__((section("__DATA,__interpose"))) = {(const void *)my_##fn, (const void *)fn};

WRAP(__psynch_mutexwait, 301)
WRAP(__psynch_mutexdrop, 302)
WRAP(__psynch_cvbroad, 303)
WRAP(__psynch_cvsignal, 304)
WRAP(__psynch_cvwait, 305)
WRAP(__psynch_cvclrprepost, 312)
