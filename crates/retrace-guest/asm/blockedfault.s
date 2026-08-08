// M12 fail-loud fixture: block SIGSEGV, then fault.
//
// A blocked synchronous fault is not deliverable and not ignorable — a real kernel force-unblocks
// and kills. retrace must ABORT THE RECORDER rather than invent a policy, so this guest never exits
// cleanly by design. The exit(0) below is unreachable and a clean exit means the check was missed.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // sigaction(SIGSEGV, handler) so the disposition is Handler, not Dfl — the blocked-ness is the
    // thing under test, not the absence of a handler.
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

    // sigprocmask(SIG_BLOCK=1, &set, NULL) with SIGSEGV's bit (1 << (11-1)).
    adrp x1, set@PAGE
    add  x1, x1, set@PAGEOFF
    mov  w2, #0x400
    str  w2, [x1]
    mov  x0, #1                 // SIG_BLOCK
    mov  x2, #0
    mov  x16, #48               // SYS_sigprocmask
    svc  #0x80

    // Bit 46 set, as asm/crash.s and asm/segvcatch.s: only a STAGE-1 translation fault reaches
    // Stop::Fault, the arm that consults the disposition. A VA below 2^36 takes a stage-2 abort
    // instead and dies fatally before any blocked check runs — passing this fixture for the
    // wrong reason.
    movz x9, #0x4000, lsl #32   // 0x4000_0000_0000
    movk x9, #0xdead, lsl #16   // | 0xdead_0000
    str  xzr, [x9]              // faults while SIGSEGV is blocked -> recorder must abort

    mov  x0, #0                 // UNREACHABLE
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
    ret

.section __DATA,__data
.p2align 3
act:      .space 24
set:      .space 8
