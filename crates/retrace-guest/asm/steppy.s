.section __TEXT,__text
.global _start
.p2align 2
_start:
    nop
    nop
    nop
    nop
    mrs  x1, cntvct_el0        // may trap-and-emulate (timebase) or retire natively — either is one step
    nop
    nop
    nop
    mov  x0, #0                // status
    mov  x16, #1               // SYS_exit
    svc  #0x80
