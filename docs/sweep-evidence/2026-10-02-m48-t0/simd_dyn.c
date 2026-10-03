// M48 Task 1: callee-saved SIMD state across a thread switch and across a signal handler's
// sigreturn. AAPCS64 makes d8-d15 callee-saved, so natively they survive any call. A mismatch is
// therefore retrace's: before the fix, every SIMD restore installed the host's v0 (walls.md §4).
// Modes: thread, signal. Prints "simd <mode> intact", or the first mismatch with exit 1.
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

// simd_across(fn, arg, in, out): load d8-d15 from in[0..8], call fn(arg), store d8-d15 to out[0..8].
// It saves and restores the caller's own d8-d15, as the ABI requires of a callee that uses them.
void simd_across(void (*fn)(void *), void *arg, const uint64_t *in, uint64_t *out);
__asm__(
    ".text\n.p2align 2\n.globl _simd_across\n_simd_across:\n"
    "  stp x29, x30, [sp, #-96]!\n"
    "  mov x29, sp\n"
    "  stp x19, x20, [sp, #16]\n"
    "  stp d8, d9, [sp, #32]\n  stp d10, d11, [sp, #48]\n"
    "  stp d12, d13, [sp, #64]\n  stp d14, d15, [sp, #80]\n"
    "  mov x19, x3\n  mov x20, x0\n"
    "  ldp d8, d9, [x2]\n  ldp d10, d11, [x2, #16]\n"
    "  ldp d12, d13, [x2, #32]\n  ldp d14, d15, [x2, #48]\n"
    "  mov x0, x1\n  blr x20\n"
    "  stp d8, d9, [x19]\n  stp d10, d11, [x19, #16]\n"
    "  stp d12, d13, [x19, #32]\n  stp d14, d15, [x19, #48]\n"
    "  ldp d8, d9, [sp, #32]\n  ldp d10, d11, [sp, #48]\n"
    "  ldp d12, d13, [sp, #64]\n  ldp d14, d15, [sp, #80]\n"
    "  ldp x19, x20, [sp, #16]\n"
    "  ldp x29, x30, [sp], #96\n  ret\n");

// The child dirties vector registers of its own, so a switch that leaked its state into main's would show.
static void *child(void *p) {
    __asm__ volatile("movi v8.16b, #0xa5\n movi v9.16b, #0xa5\n movi v16.16b, #0xa5\n movi v31.16b, #0xa5"
                     ::: "v8", "v9", "v16", "v31");
    return p;
}

static void join_a_child(void *unused) {
    (void)unused;
    pthread_t t;
    if (pthread_create(&t, NULL, child, NULL) != 0) { puts("pthread_create failed"); return; }
    pthread_join(t, NULL); // main blocks here, so the child runs, exits and wakes it
}

// Leaves d8-d15 zeroed on the way out. There is deliberately no clobber list, so the compiler
// restores nothing, and only sigreturn's restore of the interrupted context brings main's values back.
static void on_usr1(int sig) {
    (void)sig;
    __asm__ volatile("movi v8.2d, #0\n movi v9.2d, #0\n movi v10.2d, #0\n movi v11.2d, #0\n"
                     "movi v12.2d, #0\n movi v13.2d, #0\n movi v14.2d, #0\n movi v15.2d, #0\n");
}

static void raise_usr1(void *unused) { (void)unused; raise(SIGUSR1); }

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "thread";
    uint64_t in[8], out[8];
    for (int i = 0; i < 8; i++) in[i] = 0x0123456789abcdefULL * (uint64_t)(i + 1) ^ (0x5d00ULL << i);
    memset(out, 0, sizeof out);
    if (!strcmp(mode, "thread")) {
        simd_across(join_a_child, NULL, in, out);
    } else if (!strcmp(mode, "signal")) {
        signal(SIGUSR1, on_usr1);
        simd_across(raise_usr1, NULL, in, out);
    } else {
        printf("unknown mode %s\n", mode);
        return 2;
    }
    for (int i = 0; i < 8; i++) {
        if (out[i] != in[i]) {
            printf("simd %s MISMATCH d%d got %#llx want %#llx\n", mode, i + 8,
                   (unsigned long long)out[i], (unsigned long long)in[i]);
            return 1;
        }
    }
    printf("simd %s intact\n", mode);
    return 0;
}
