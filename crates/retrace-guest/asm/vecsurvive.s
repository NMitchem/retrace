// M12: vector state must survive a handler. A handler is ordinary compiled code and will use NEON;
// if sigreturn does not restore v8, a handler that RETURNS silently corrupts the guest.
// The handler deliberately clobbers v8, so only a real restore makes this exit 0.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    adrp x1, act@PAGE
    add  x1, x1, act@PAGEOFF
    adrp x2, handler@PAGE
    add  x2, x2, handler@PAGEOFF
    str  x2, [x1, #0]
    adrp x2, tramp@PAGE
    add  x2, x2, tramp@PAGEOFF
    str  x2, [x1, #8]
    mov  w2, #0x40
    str  w2, [x1, #20]
    mov  x0, #11
    mov  x2, #0
    mov  x16, #46
    svc  #0x80

    // v8 = a known 64-bit pattern.
    adrp x9, pattern@PAGE
    add  x9, x9, pattern@PAGEOFF
    ldr  d8, [x9]

    mov  x16, #20
    svc  #0x80
    mov  x1, #11
    mov  x16, #37               // SYS_kill -> handler runs and clobbers v8
    svc  #0x80

    // Back from sigreturn: v8 must be the pattern again.
    adrp x9, pattern@PAGE
    add  x9, x9, pattern@PAGEOFF
    ldr  d9, [x9]
    fmov x10, d8
    fmov x11, d9
    cmp  x10, x11
    b.ne vec_lost
    mov  x0, #0
    mov  x16, #1
    svc  #0x80
vec_lost:
    mov  x0, #41                // v8 did not survive the handler
    mov  x16, #1
    svc  #0x80

tramp:
    stp  x4, x5, [sp, #-16]!
    str  x1, [sp, #-16]!
    blr  x0
    ldr  x1, [sp], #16
    ldp  x0, x2, [sp], #16
    mov  x16, #184
    svc  #0x80
    brk  #0

handler:
    movi d8, #0                 // clobber the very register the guest is checking
    ret

.section __DATA,__data
.p2align 3
act:      .space 24
pattern:  .quad 0x1122334455667788
