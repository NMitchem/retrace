// M6 planted-bug crash fixture (dynamic, links libSystem like hello_dyn). Fully deterministic:
//  1. fstat(1, &g.st): a recorded kernel write into a watchable global — the dynamic-guest
//     syscall-write watch target (Task 4). Contents vary per record run but are RECORDED, so
//     replay is bit-identical per trace.
//  2. Marker + address-reveal writes: tests discover &g.st and &g.ptr from the recorded
//     write(1, buf, len) args (args[1] IS the buffer's guest VA) — never hardcoded.
//  3. A volatile off-by-one store loop corrupts g.ptr (buf[4] aliases ptr by declaration order;
//     both are 8-aligned longs, so no padding sits between them).
//  4. *g.ptr faults: GARBAGE_VA has bit 46 set (L1 index 0x400, never mapped; < 2^47) — a
//     stage-1 EL0 data abort with FAR == GARBAGE_VA. Same constant as asm/crash.s.
#include <sys/stat.h>
#include <unistd.h>

#define GARBAGE_VA 0x4000DEAD0000UL

static struct {
    struct stat st;   /* fstat target */
    long buf[4];
    long *ptr;        /* directly follows buf: buf[4] IS ptr */
} g;

int main(void) {
    g.ptr = &g.buf[0];
    fstat(1, &g.st);
    write(1, "CRASHY:", 7);
    write(1, &g.st, 8);            /* args[1] == &g.st  */
    write(1, &g.ptr, 8);           /* args[1] == &g.ptr */
    volatile long *p = g.buf;
    for (int i = 0; i <= 4; i++)   /* planted off-by-one: i==4 corrupts g.ptr */
        p[i] = (long)GARBAGE_VA;
    *(volatile long *)g.ptr = 42;  /* stage-1 fault at GARBAGE_VA */
    return 0;                      /* unreached */
}
