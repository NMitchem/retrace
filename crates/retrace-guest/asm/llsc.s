// M42 repo fixture: exclusive (LL/SC) pairs under stepping and debug stops (spec
// docs/superpowers/specs/2026-09-24-retrace-m42-llsc-design.md §4).
//
// Shapes (a)-(d) are M42 t0's scratch fixture byte for byte, so every coordinate in the t0
// measurements document holds for windows 1-4. (e)-(i) are the spec's additions.
//
// Each shape is followed by one syscall whose ARGUMENTS carry its result, so replay's divergence
// oracle, which compares (num, args[0..8]) at every landmark, names any difference there:
//   x3 = the cell after the shape, or a store-exclusive's status;
//   x4 = how many times a retry loop was ENTERED (1 per logical update when no store failed);
//   x5 = a second value.
// write(2) ignores x3..x5; they are there for the oracle. The exit window publishes in exit's
// status instead, because an Exit event records only its code.
//
// Landmarks (window n ends at landmark n):
//    1 write "a"  (a) discard-status, dyld getpid's words: fill an empty cell once
//    2 write "b"  (b) retry loop, three increments
//    3 write "c"  (c) CAS: if (*cas == 5) *cas = 9
//    4 write "d"  (d) ldxp/stxp pair
//    5 write "e"  (e) clrex between the halves: the store fails natively (x3 = 1)
//    6 write "f"  (f) a trapped timebase read between the halves: an exit, so it fails (x3 = 1)
//    7 getpid     (g) a syscall between the halves...
//    8 write "g"      ...so the store, first in window 8, fails (x3 = 1)
//    9 write "h"  (h) (a)'s shape on the filled cell: the cbnz is taken, an LDX with no STX
//   10 exit       (i) a one-pass retry loop in the EXIT window: exit(entries - 1) = exit(0)
// (a) adds ONE extra landmark (getpid) before its write iff its store was lost.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // ---- (a) discard-status, dyld getpid style: fill an empty cell once ----
    adrp x9, cella@PAGE
    add  x9, x9, cella@PAGEOFF
    movz w0, #0x4242                // the value to cache
a_ldx:
    ldxr w10, [x9]                  // K = 3 in window 1
    cbnz w10, a_after               // already filled: skip the store
a_stx:
    stxr wzr, w0, [x9]              // status DISCARDED (dyld getpid shape)
a_after:
    ldr  w11, [x9]
    cbnz w11, a_report
    mov  x16, #20                   // SYS_getpid: issued ONLY if the store was lost
    svc  #0x80
a_report:
    ldr  w3, [x9]                   // x3 = cell (0x4242 when the store landed)
    mov  x4, #0
    mov  x5, #0
    mov  x0, #1
    adrp x1, msga@PAGE
    add  x1, x1, msga@PAGEOFF
    mov  x2, #2
    mov  x16, #4                    // SYS_write(1, "a\n", 2), x3 = cell
a_svc:
    svc  #0x80

    // ---- (b) retry loop: counter += 1, three times ----
b_start:
    adrp x0, ctr@PAGE
    add  x0, x0, ctr@PAGEOFF
    mov  x19, #3                    // three logical increments
    mov  x20, #0                    // loop entries
b_retry:
    add  x20, x20, #1
b_ldx:
    ldaxr x1, [x0]
    add  x1, x1, #1
b_stx:
    stlxr w2, x1, [x0]
    cbnz w2, b_retry
b_next:
    subs x19, x19, #1
    b.ne b_retry
b_done:
    ldr  x3, [x0]                   // x3 = counter (3)
    mov  x4, x20                    // x4 = loop entries (3 when no stlxr failed)
    mov  x5, #0
    mov  x0, #1
    adrp x1, msgb@PAGE
    add  x1, x1, msgb@PAGEOFF
    mov  x2, #2
    mov  x16, #4                    // SYS_write(1, "b\n", 2), x3 = counter, x4 = entries
b_svc:
    svc  #0x80

    // ---- (c) CAS shape: if (*cas == 5) *cas = 9 ----
c_start:
    adrp x0, cas@PAGE
    add  x0, x0, cas@PAGEOFF
    mov  x3, #5                     // expected
    mov  x4, #9                     // new
    mov  x20, #0
c_retry:
    add  x20, x20, #1
c_ldx:
    ldaxr x1, [x0]
    cmp  x1, x3
    b.ne c_out
c_stx:
    stlxr w2, x4, [x0]
    cbnz w2, c_retry
c_out:
    ldr  x3, [x0]                   // x3 = cell (9)
    mov  x4, x20                    // x4 = loop entries (1)
    mov  x5, #0
    mov  x0, #1
    adrp x1, msgc@PAGE
    add  x1, x1, msgc@PAGEOFF
    mov  x2, #2
    mov  x16, #4                    // SYS_write(1, "c\n", 2), x3 = cell, x4 = entries
c_svc:
    svc  #0x80

    // ---- (d) pair: pair[0] += 10, pair[1] += 20 ----
d_start:
    adrp x0, pair@PAGE
    add  x0, x0, pair@PAGEOFF
    mov  x20, #0
d_retry:
    add  x20, x20, #1
d_ldx:
    ldxp x1, x2, [x0]
    add  x4, x1, #10
    add  x5, x2, #20
d_stx:
    stxp w3, x4, x5, [x0]
    cbnz w3, d_retry
d_done:
    ldp  x3, x5, [x0]               // x3 = pair[0] (11), x5 = pair[1] (22)
    mov  x4, x20                    // x4 = loop entries (1)
    mov  x0, #1
    adrp x1, msgd@PAGE
    add  x1, x1, msgd@PAGEOFF
    mov  x2, #2
    mov  x16, #4                    // SYS_write(1, "d\n", 2), x3/x5 = pair, x4 = entries
d_svc:
    svc  #0x80

    // ---- (e) clrex between the halves: the store fails natively ----
e_start:
    adrp x0, celle@PAGE
    add  x0, x0, celle@PAGEOFF
    mov  w1, #7
e_ldx:
    ldxr w6, [x0]
e_clrex:
    clrex
e_stx:
    stxr w2, w1, [x0]
    mov  w3, w2                     // x3 = status (1: the store failed)
    ldr  w4, [x0]                   // x4 = cell (0: nothing was stored)
    mov  x5, #0
    mov  x0, #1
    adrp x1, msge@PAGE
    add  x1, x1, msge@PAGEOFF
    mov  x2, #2
    mov  x16, #4                    // SYS_write(1, "e\n", 2), x3 = status, x4 = cell
e_svc:
    svc  #0x80

    // ---- (f) a trapped timebase read between the halves: an exit, so the store fails ----
f_start:
    adrp x0, cellf@PAGE
    add  x0, x0, cellf@PAGEOFF
    mov  w1, #7
f_ldx:
    ldxr w6, [x0]
f_mrs:
    mrs  x7, cntvct_el0             // trapped and emulated below the trace (try_emulate_timebase)
f_stx:
    stxr w2, w1, [x0]
    mov  w3, w2                     // x3 = status (1: the store failed)
    ldr  w4, [x0]                   // x4 = cell (0)
    mov  x5, #0
    mov  x0, #1
    adrp x1, msgf@PAGE
    add  x1, x1, msgf@PAGEOFF
    mov  x2, #2
    mov  x16, #4                    // SYS_write(1, "f\n", 2), x3 = status, x4 = cell
f_svc:
    svc  #0x80

    // ---- (g) a syscall between the halves: the store, first in window 8, fails ----
g_start:
    adrp x19, cellg@PAGE            // callee-saved registers: the syscall returns in x0/x1
    add  x19, x19, cellg@PAGEOFF
    mov  w20, #7
g_ldx:
    ldxr w6, [x19]
    mov  x16, #20                   // SYS_getpid, between the halves
g_svc:
    svc  #0x80
g_stx:
    stxr w21, w20, [x19]
    mov  w3, w21                    // x3 = status (1: the store failed)
    ldr  w4, [x19]                  // x4 = cell (0)
    mov  x5, #0
    mov  x0, #1
    adrp x1, msgg@PAGE
    add  x1, x1, msgg@PAGEOFF
    mov  x2, #2
    mov  x16, #4                    // SYS_write(1, "g\n", 2), x3 = status, x4 = cell
g_wsvc:
    svc  #0x80

    // ---- (h) (a)'s shape on the filled cell: the cbnz is taken, a load with no store ----
h_start:
    adrp x9, cella@PAGE
    add  x9, x9, cella@PAGEOFF
    movz w0, #0x4343
h_ldx:
    ldxr w10, [x9]                  // cella already holds 0x4242
h_cbnz:
    cbnz w10, h_after               // taken: this load-exclusive has no store-exclusive
h_stx:
    stxr wzr, w0, [x9]              // never executed
h_after:
    ldr  w3, [x9]                   // x3 = cell (0x4242)
    mov  x4, #0
    mov  x5, #0
    mov  x0, #1
    adrp x1, msgh@PAGE
    add  x1, x1, msgh@PAGEOFF
    mov  x2, #2
    mov  x16, #4                    // SYS_write(1, "h\n", 2), x3 = cell
h_svc:
    svc  #0x80

    // ---- (i) the EXIT window holds a one-pass retry loop ----
i_start:
    adrp x0, ctri@PAGE
    add  x0, x0, ctri@PAGEOFF
    mov  x20, #0
i_retry:
    add  x20, x20, #1
i_ldx:
    ldaxr x1, [x0]
    add  x1, x1, #1
i_stx:
    stlxr w2, x1, [x0]
    cbnz w2, i_retry
    sub  x0, x20, #1                // exit status = entries - 1: 0 when no stlxr failed
    mov  x16, #1                    // SYS_exit
i_svc:
    svc  #0x80

.section __DATA,__data
// Each cell in its own 64-byte block, so a watch's FAR can only name its own cell.
.p2align 6
cella: .word 0
.p2align 6
ctr:   .quad 0
.p2align 6
cas:   .quad 5
.p2align 6
pair:  .quad 1, 2
.p2align 6
msga:  .ascii "a\n"
msgb:  .ascii "b\n"
msgc:  .ascii "c\n"
msgd:  .ascii "d\n"
// The spec's additions, appended so that t0's data addresses do not move.
.p2align 6
celle: .word 0
.p2align 6
cellf: .word 0
.p2align 6
cellg: .word 0
.p2align 6
ctri:  .quad 0
msge:  .ascii "e\n"
msgf:  .ascii "f\n"
msgg:  .ascii "g\n"
msgh:  .ascii "h\n"
