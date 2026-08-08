// M12: SA_ONSTACK + sigaltstack. The handler checks its OWN sp is inside the alternate stack, which
// is the only way to prove the frame was placed there rather than on the normal stack.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // sigaltstack(&ss, NULL). struct sigaltstack: +0 ss_sp  +8 ss_size  +16 ss_flags
    adrp x0, ss@PAGE
    add  x0, x0, ss@PAGEOFF
    adrp x1, altbuf@PAGE
    add  x1, x1, altbuf@PAGEOFF
    str  x1, [x0, #0]
    mov  x1, #0x2000
    str  x1, [x0, #8]
    str  xzr, [x0, #16]
    mov  x1, #0                 // oss = NULL
    mov  x16, #53               // SYS_sigaltstack
    svc  #0x80

    adrp x1, act@PAGE
    add  x1, x1, act@PAGEOFF
    adrp x2, handler@PAGE
    add  x2, x2, handler@PAGEOFF
    str  x2, [x1, #0]
    adrp x2, tramp@PAGE
    add  x2, x2, tramp@PAGEOFF
    str  x2, [x1, #8]
    mov  w2, #0x41              // SA_SIGINFO | SA_ONSTACK
    str  w2, [x1, #20]
    mov  x0, #11
    mov  x2, #0
    mov  x16, #46
    svc  #0x80

    mov  x16, #20               // SYS_getpid
    svc  #0x80
    mov  x1, #11
    mov  x16, #37               // SYS_kill
    svc  #0x80

    mov  x0, #0
    mov  x16, #1
    svc  #0x80

tramp:
    // sp must be inside [altbuf, altbuf + 0x2000).
    adrp x9, altbuf@PAGE
    add  x9, x9, altbuf@PAGEOFF
    mov  x10, sp
    subs x10, x10, x9
    b.lo not_on_alt             // sp < altbuf
    mov  x11, #0x2000
    cmp  x10, x11
    b.hs not_on_alt             // sp >= altbuf + size
    stp  x4, x5, [sp, #-16]!
    str  x1, [sp, #-16]!
    blr  x0
    ldr  x1, [sp], #16
    ldp  x0, x2, [sp], #16
    mov  x16, #184
    svc  #0x80
    brk  #0

not_on_alt:
    mov  x0, #31                // the frame was NOT placed on the alternate stack
    mov  x16, #1
    svc  #0x80

handler:
    ret

.section __DATA,__data
.p2align 3
act:      .space 24
ss:       .space 24
.p2align 4
altbuf:   .space 0x2000
