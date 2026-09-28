.section __TEXT,__text
.global _start
.p2align 2
// M44 B5: an arm64e call chain for lldb's `bt` over gdb-remote. Each of f1..f3 signs its return
// address with paciasp and saves it in a frame record, so every saved LR on the stack — and x30
// itself in f3 — carries a PAC signature lldb's unwinder must strip. f3 then stores to the M6
// GARBAGE_VA (asm/crash.s), a stage-1 fault recorded as the terminal Event::Crash, leaving
// _start -> f1 -> f2 -> f3 on the stack. Static and freestanding like strip47: the main
// executable is arm64e, so the PAC posture is on (M7 Task 6).
_start:
    mov  x29, #0                 // the frame chain ends here
    bl   _f1
    mov  x0, #0
    mov  x16, #1                 // SYS_exit (unreached)
    svc  #0x80
.global _f1
_f1:
    paciasp
    stp  x29, x30, [sp, #-16]!
    mov  x29, sp
    bl   _f2
    ldp  x29, x30, [sp], #16
    autiasp
    ret
.global _f2
_f2:
    paciasp
    stp  x29, x30, [sp, #-16]!
    mov  x29, sp
    bl   _f3
    ldp  x29, x30, [sp], #16
    autiasp
    ret
.global _f3
_f3:
    paciasp
    stp  x29, x30, [sp, #-16]!
    mov  x29, sp
    movz x0, #0x4000, lsl #32    // 0x4000_0000_0000
    movk x0, #0xDEAD, lsl #16    // | 0xDEAD_0000
    mov  w1, #0x2A
    strb w1, [x0]                // stage-1 fault -> Stop::Fault (never retires)
    ldp  x29, x30, [sp], #16
    autiasp
    ret
