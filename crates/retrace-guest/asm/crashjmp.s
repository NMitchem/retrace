.section __TEXT,__text
.global _start
.p2align 2
// M6 instruction-abort crash guest: branch to the same never-mapped VA as crash.s. The FETCH
// takes a stage-1 translation fault => EC 0x20 (lower-EL instruction abort) via the trampoline.
_start:
    movz x0, #0x4000, lsl #32
    movk x0, #0xDEAD, lsl #16
    br   x0                      // instruction abort at the target VA
    // Unreached.
    mov  x0, #0
    mov  x16, #1
    svc  #0x80
