// M12: validate the ENTRY CONTRACT. The trampoline checks every register retrace promises to set
// and exits with a distinct code per failed check, so a gate failure names the field that broke.
//
// Raises via kill() rather than faulting: a self-raise resumes AFTER the svc, so no pc repair is
// needed and this fixture tests exactly one thing — the register contract.
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
    mov  w2, #0x40              // SA_SIGINFO
    str  w2, [x1, #20]
    mov  x0, #11                // SIGSEGV
    mov  x2, #0
    mov  x16, #46               // SYS_sigaction
    svc  #0x80

    mov  x16, #20               // SYS_getpid
    svc  #0x80
    mov  x1, #11                // SIGSEGV
    mov  x16, #37               // SYS_kill -- delivered to our handler
    svc  #0x80

    mov  x0, #0                 // every check passed and sigreturn came back here
    mov  x16, #1
    svc  #0x80

// x0=catcher x1=infostyle x2=sig x3=siginfo* x4=ucontext* x5=token
tramp:
    cmp  x1, #30                // UC_FLAVOR
    b.ne bad_infostyle
    cmp  x2, #11                // the signal we raised
    b.ne bad_signo
    mov  x9, sp
    cmp  x3, x9                 // measured: sp IS the siginfo pointer
    b.ne bad_sp
    add  x9, x3, #104           // ucontext sits one siginfo_t past the base
    cmp  x4, x9
    b.ne bad_uctx
    ldr  w9, [x3, #0]           // siginfo->si_signo
    cmp  w9, #11
    b.ne bad_siginfo
    cbz  x5, bad_token          // the token must be non-zero

    stp  x4, x5, [sp, #-16]!
    str  x1, [sp, #-16]!
    blr  x0
    ldr  x1, [sp], #16
    ldp  x0, x2, [sp], #16
    mov  x16, #184              // sigreturn
    svc  #0x80
    brk  #0

bad_infostyle: mov x0, #21
    b exit
bad_signo:     mov x0, #22
    b exit
bad_sp:        mov x0, #23
    b exit
bad_uctx:      mov x0, #24
    b exit
bad_siginfo:   mov x0, #25
    b exit
bad_token:     mov x0, #26
exit:
    mov  x16, #1                // SYS_exit
    svc  #0x80

handler:
    ret                         // the checks are the point; the handler itself does nothing

.section __DATA,__data
.p2align 3
act:      .space 24
