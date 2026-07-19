.section __TEXT,__text
.global _start
.p2align 2
// M6 stage-1 crash guest. VA 0x4000_DEAD_0000 has bit 46 set => L1 index 0x400; only L1[0] is
// ever valid (the identity map covers just the 36-bit IPA space), so the store takes a STAGE-1
// translation fault, delivered via the EL1 trampoline (run()'s INNER match) => Stop::Fault with
// FAR == this VA. Contrast asm/wildstore.s: a VA < 2^36 is stage-1-mapped but stage-2-unbacked
// => OUTER abort, which stays fatal (the M6 classification negative).
_start:
    movz x0, #0x4000, lsl #32    // 0x4000_0000_0000
    movk x0, #0xDEAD, lsl #16    // | 0xDEAD_0000
    mov  w1, #0x2A
    strb w1, [x0]                // stage-1 fault -> Stop::Fault (never retires)
    // Unreached.
    mov  x0, #0
    mov  x16, #1                 // SYS_exit
    svc  #0x80
