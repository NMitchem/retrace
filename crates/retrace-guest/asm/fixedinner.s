// M8-stack. Regression cover for MAP_FIXED containment-reuse: a MAP_FIXED mmap landing WHOLLY
// INSIDE a larger existing backing must zero exactly the requested pages and leave the rest of that
// backing mapped WITH ITS CONTENTS. Before the fix the enclosing backing was dropped wholesale,
// which silently destroyed live guest memory -- deterministically, so no divergence ever fired and
// nothing flagged it.
//
// Two contained shapes, both real:
//   A) strictly-interior punch  -> bytes below AND above the punch must survive
//   B) punch at the region base -> bytes above the punch must survive. This is libstd's
//      `install_main_guard`, which mmaps MAP_FIXED at `usrstack64 - RLIMIT_STACK` -- wholly inside
//      the stack backing -- the case that would otherwise unmap the guest's own running stack.
//
// The marker bytes written before each punch are the sentinels: they live OUTSIDE the punched range
// but INSIDE the enclosing backing, so they survive only if the backing was reused rather than
// dropped.
//
// Publishes ten little-endian u64s on stdout:
//   [0] retA   [8] baseA   [16] A+0      [24] A+0x4000  [32] A+0x8000  [40] A+0xC000
//  [48] retB  [56] baseB   [64] B+0x4000 [72] B+0xC000
.section __TEXT,__text
.global _start
.p2align 2
_start:
    adrp x21, out@PAGE
    add  x21, x21, out@PAGEOFF

    // ---- Region A: mmap(0, 0x10000, RW, ANON|PRIVATE) -> 4 pages ----
    mov  x0, #0
    mov  x1, #0x10000
    mov  x2, #3                    // PROT_READ|PROT_WRITE
    movz x3, #0x1002               // MAP_ANON|MAP_PRIVATE
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197                 // SYS_mmap
    svc  #0x80
    mov  x19, x0                   // baseA

    // Fill each of the 4 pages with a distinct marker byte.
    mov  w9, #0x11
    strb w9, [x19]
    mov  w9, #0x22
    add  x10, x19, #0x4000
    strb w9, [x10]
    mov  w9, #0x33
    add  x10, x19, #0x8000
    strb w9, [x10]
    mov  w9, #0x44
    add  x10, x19, #0xC000
    strb w9, [x10]

    // Punch page 1 (interior): mmap(baseA+0x4000, 0x4000, RW, ANON|PRIVATE|FIXED).
    add  x0, x19, #0x4000
    mov  x1, #0x4000
    mov  x2, #3
    movz x3, #0x1012               // MAP_ANON|MAP_FIXED|MAP_PRIVATE
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197
    svc  #0x80
    str  x0,  [x21]                // [0] retA
    str  x19, [x21, #8]            // [8] baseA

    // Read back: head remnant, the punched page, both tail-remnant pages.
    ldrb w9, [x19]
    str  x9, [x21, #16]            // [16] expect 0x11 (head remnant kept its contents)
    add  x10, x19, #0x4000
    ldrb w9, [x10]
    str  x9, [x21, #24]            // [24] expect 0x00 (punched page is fresh + zeroed)
    add  x10, x19, #0x8000
    ldrb w9, [x10]
    str  x9, [x21, #32]            // [32] expect 0x33 (tail remnant kept its contents)
    add  x10, x19, #0xC000
    ldrb w9, [x10]
    str  x9, [x21, #40]            // [40] expect 0x44 (tail remnant kept its contents)

    // ---- Region B: same, but punch at the region base (the libstd guard-page shape) ----
    mov  x0, #0
    mov  x1, #0x10000
    mov  x2, #3
    movz x3, #0x1002
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197
    svc  #0x80
    mov  x20, x0                   // baseB

    mov  w9, #0x22
    add  x10, x20, #0x4000
    strb w9, [x10]
    mov  w9, #0x44
    add  x10, x20, #0xC000
    strb w9, [x10]

    // Punch page 0: mmap(baseB, 0x4000, RW, ANON|PRIVATE|FIXED) -- no head remnant, tail survives.
    mov  x0, x20
    mov  x1, #0x4000
    mov  x2, #3
    movz x3, #0x1012
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197
    svc  #0x80
    str  x0,  [x21, #48]           // [48] retB (must equal baseB)
    str  x20, [x21, #56]           // [56] baseB

    add  x10, x20, #0x4000
    ldrb w9, [x10]
    str  x9, [x21, #64]            // [64] expect 0x22 (survived above the guard page)
    add  x10, x20, #0xC000
    ldrb w9, [x10]
    str  x9, [x21, #72]            // [72] expect 0x44 (survived above the guard page)

    // write(1, out, 80)
    mov  x0, #1
    mov  x1, x21
    mov  x2, #80
    mov  x16, #4                   // SYS_write
    svc  #0x80

    // exit(0)
    mov  x0, #0
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
.p2align 4
out:      .space 80
