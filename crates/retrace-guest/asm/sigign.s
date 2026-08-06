// M11 non-terminal guest: ignore SIGABRT, raise it, then prove we kept running.
// sa_tramp is 0 — legal here ONLY because M11 never forwards sigaction to the kernel. If this
// fixture ever starts failing with an errno, it means the servicing arm was bypassed.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    mov  x0, #6                 // SIGABRT
    adrp x1, act@PAGE
    add  x1, x1, act@PAGEOFF    // act (struct __sigaction, 24 bytes)
    mov  x2, #0                 // oldact = NULL
    mov  x16, #46               // SYS_sigaction
    svc  #0x80

    mov  x16, #20               // SYS_getpid
    svc  #0x80
    mov  x1, #6                 // SIGABRT
    mov  x16, #37               // SYS_kill -- ignored, must return and continue
    svc  #0x80

    mov  x0, #1                 // fd = stdout
    adrp x1, msg@PAGE
    add  x1, x1, msg@PAGEOFF
    mov  x2, #3
    mov  x16, #4                // SYS_write
    svc  #0x80

    mov  x0, #0
    mov  x16, #1                // SYS_exit
    svc  #0x80

.section __DATA,__data
.p2align 3
act:
    .quad 1                     // __sigaction_u = SIG_IGN
    .quad 0                     // sa_tramp (unused: never forwarded)
    .long 0                     // sa_mask
    .long 0                     // sa_flags
msg:
    .ascii "ok\n"
