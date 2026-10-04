// M48 Task 6: JIT write-protect (spec §3f, §3h). MAP_JIT pages, toggled per thread through
// pthread_jit_write_protect_np, which writes S3_6_C15_C1_5 (§2c). stdout is line-buffered, so each
// line is its own write(2) and so a landmark: the seek test needs one inside a write-enabled window.
// Modes, from argv[1]:
//   basic       native's sprr.c sequence (docs/sweep-evidence/2026-10-02-m48-static/sprr.c): the
//               commpage words, the register around each toggle, 42 from JIT code, a child's register.
//   v8          a committed sub-range inside a larger PROT_NONE reservation, as V8 maps it (P5): a
//               1 MiB MAP_JIT reservation mapped PROT_NONE, the part from +0x40000 mprotected RWX, code
//               written write-enabled and run protected. The +0x40000 head is this fixture's: V8's own
//               offset is the first 256 KiB boundary past the base, so a head of 0x4000-0x34000 and a
//               PROT_NONE tail follow in the six measured walks (t0 M5). Then a whole munmap. Natively a
//               committed (RWX) MAP_JIT page refuses every further mprotect with EACCES, NONE included
//               (t6-jitprobe), so V8 never decommits one and neither does this mode.
//   twothreads  thread A stays write-enabled while thread B, protected, runs the page A writes. They
//               hand off through dispatch semaphores, so each hand-off blocks one thread and the
//               cooperative scheduler switches to the other: the view must follow the running thread.
//   fault       a store to a MAP_JIT page while protected (every thread starts so) faults, as native.
// The `JITWP page=` marker names the page; its address differs natively, so the e2e never compares it.
#include <dispatch/dispatch.h>
#include <libkern/OSCacheControl.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>

static unsigned long long rd(void) { uint64_t v; __asm__ volatile("mrs %0, S3_6_C15_C1_5" : "=r"(v)); return v; }
static unsigned long long cp64(uintptr_t off) { return *(volatile uint64_t *)(0xfffffc000ULL + off); }

static uint32_t *jit_page(void) {
    void *p = mmap(NULL, 0x4000, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_PRIVATE | MAP_ANON | MAP_JIT, -1, 0);
    if (p == MAP_FAILED) { puts("mmap failed"); return NULL; }
    printf("JITWP page=%p\n", p);
    return p;
}

// `mov x0, #n; ret` at p, written while write-enabled, then made visible to instruction fetch.
static void emit(uint32_t *p, unsigned n) {
    p[0] = 0xd2800000u | (n << 5);
    p[1] = 0xd65f03c0u;
    sys_icache_invalidate(p, 8);
}

static void *child(void *a) { (void)a; printf("child initial sprr=%#llx\n", rd()); return NULL; }

static int basic(void) {
    printf("commpage +0x10c=%u +0x110=%#llx +0x118=%#llx\n",
           (unsigned)*(volatile uint8_t *)0xfffffc10cULL, cp64(0x110), cp64(0x118));
    printf("main initial sprr=%#llx\n", rd());
    uint32_t *p = jit_page();
    if (!p) return 2;
    pthread_jit_write_protect_np(0); printf("main write-en sprr=%#llx\n", rd());
    p[0] = 0xd2800540u; p[1] = 0xd65f03c0u; // mov x0, #42; ret
    pthread_jit_write_protect_np(1); printf("main protect sprr=%#llx\n", rd());
    sys_icache_invalidate(p, 8);
    printf("jit call=%d\n", ((int (*)(void))p)());
    pthread_jit_write_protect_np(0);
    pthread_t th; pthread_create(&th, NULL, child, NULL); pthread_join(th, NULL);
    printf("main after-child sprr=%#llx\n", rd());
    return 0;
}

static int v8(void) {
    const size_t R = 0x100000, OFF = 0x40000;
    uint8_t *r = mmap(NULL, R, PROT_NONE, MAP_PRIVATE | MAP_ANON | MAP_NORESERVE | MAP_JIT, -1, 0);
    if (r == MAP_FAILED) { puts("mmap failed"); return 2; }
    if (mprotect(r + OFF, R - OFF, PROT_READ | PROT_WRITE | PROT_EXEC) != 0) { puts("mprotect failed"); return 2; }
    uint32_t *p = (uint32_t *)(r + OFF);
    pthread_jit_write_protect_np(0);
    p[0] = 0xd2800540u; p[1] = 0xd65f03c0u;
    pthread_jit_write_protect_np(1);
    sys_icache_invalidate(p, 8);
    printf("v8 call=%d\n", ((int (*)(void))p)());
    if (munmap(r, R) != 0) { puts("munmap failed"); return 2; }
    puts("v8 unmapped");
    return 0;
}

static uint32_t *page;
static dispatch_semaphore_t go_a, go_b;

static void *b_main(void *a) {
    (void)a;
    for (int round = 1; round <= 2; round++) {
        printf("B ran %d\n", ((int (*)(void))page)());
        dispatch_semaphore_signal(go_a);
        if (round < 2) dispatch_semaphore_wait(go_b, DISPATCH_TIME_FOREVER);
    }
    return NULL;
}

static int twothreads(void) {
    if (!(page = jit_page())) return 2;
    go_a = dispatch_semaphore_create(0);
    go_b = dispatch_semaphore_create(0);
    pthread_jit_write_protect_np(0); // A is write-enabled from here until after the join
    emit(page, 1);
    pthread_t th; pthread_create(&th, NULL, b_main, NULL);
    dispatch_semaphore_wait(go_a, DISPATCH_TIME_FOREVER); // B runs round 1
    emit(page, 2);
    printf("A sprr=%#llx wrote 2\n", rd());
    dispatch_semaphore_signal(go_b);
    dispatch_semaphore_wait(go_a, DISPATCH_TIME_FOREVER); // B runs round 2
    pthread_join(th, NULL);
    pthread_jit_write_protect_np(1);
    printf("A protect, ran %d\n", ((int (*)(void))page)());
    return 0;
}

static int fault(void) {
    uint32_t *p = jit_page();
    if (!p) return 2;
    *(volatile uint32_t *)p = 0xd2800540u; // protected: natively a fault; J2 would repair it
    puts("UNREACHED");
    return 0;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    const char *mode = argc > 1 ? argv[1] : "basic";
    if (!strcmp(mode, "basic")) return basic();
    if (!strcmp(mode, "v8")) return v8();
    if (!strcmp(mode, "twothreads")) return twothreads();
    if (!strcmp(mode, "fault")) return fault();
    printf("unknown mode %s\n", mode);
    return 2;
}
