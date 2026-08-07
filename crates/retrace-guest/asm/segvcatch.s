// M12: install a SIGSEGV handler with our own trampoline, fault, repair, and continue.
// The handler advances the saved pc past the faulting store, so sigreturn resuming MUTATED state
// is what makes this guest exit 0 instead of looping on the same fault forever.
//
// Freestanding on purpose: the trampoline is ours, so this tests retrace's entry contract without
// libc's _sigtramp in the way. W^X-safe — the trampoline is text, never stack.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // sigaction(SIGSEGV=11, &act, NULL). struct __sigaction is 24 bytes:
    //   +0 sa_handler  +8 sa_tramp  +16 sa_mask  +20 sa_flags
    // Addresses are stored at runtime via adrp/add so this works whether or not the load slides.
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
    mov  x2, #0                 // oldact = NULL
    mov  x16, #46               // SYS_sigaction
    svc  #0x80

    // Fault: store through an unmapped address. The handler advances past THIS instruction.
    movz x9, #0xdead, lsl #16
    str  xzr, [x9]              // <-- the faulting store

    mov  x0, #1
    adrp x1, resumed@PAGE
    add  x1, x1, resumed@PAGEOFF
    mov  x2, #8
    mov  x16, #4                // SYS_write
    svc  #0x80
    mov  x0, #0
    mov  x16, #1                // SYS_exit
    svc  #0x80

// Entered by retrace with x0=catcher x1=infostyle x2=sig x3=siginfo* x4=ucontext* x5=token.
tramp:
    stp  x4, x5, [sp, #-16]!    // keep ucontext* and token across the handler call
    str  x1, [sp, #-16]!
    blr  x0                     // call the handler (x0..x2 are already its args)
    ldr  x1, [sp], #16
    ldp  x0, x2, [sp], #16      // x0 = ucontext*, x2 = token
    mov  x16, #184              // sigreturn(uctx, infostyle, token)
    svc  #0x80
    brk  #0                     // sigreturn must not return

// void handler(int sig, siginfo_t *si, ucontext_t *uc) — advance uc->uc_mcontext->__ss.__pc by 4.
handler:
    mov  x0, #1
    adrp x1, caught@PAGE
    add  x1, x1, caught@PAGEOFF
    mov  x2, #7
    mov  x16, #4                // SYS_write
    svc  #0x80
    // x2 was clobbered by the write; reload the ucontext from the frame the trampoline saved.
    // Layout at handler entry: [sp]=infostyle [sp+8]=pad [sp+16]=ucontext* [sp+24]=token.
    ldr  x9, [sp, #16]          // ucontext*
    ldr  x10, [x9, #48]         // uc_mcontext (a POINTER — measured at ucontext+48)
    ldr  x11, [x10, #272]       // __ss.__pc: thread_state at mcontext+16, __pc at +256
    add  x11, x11, #4
    str  x11, [x10, #272]
    ret

.section __DATA,__data
.p2align 3
act:      .space 24
caught:   .ascii "caught\n"
resumed:  .ascii "resumed\n"
