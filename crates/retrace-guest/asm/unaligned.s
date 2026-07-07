.section __TEXT,__text
.global _start
_start:
    adrp x1, buf@PAGE
    add  x1, x1, buf@PAGEOFF
    add  x1, x1, #1            // odd address => unaligned
    movz x2, #0x7788
    movk x2, #0x5566, lsl #16
    movk x2, #0x3344, lsl #32
    movk x2, #0x1122, lsl #48
    str  x2, [x1]             // unaligned store: faults MMU-off, ok MMU-on Normal
    ldr  x3, [x1]
    cmp  x2, x3
    cset x0, ne               // x0 = 0 iff readback matches
    mov  x16, #1              // SYS_exit
    svc  #0x80
.section __DATA,__data
.p2align 4
buf: .space 32
