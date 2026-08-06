// M11 safety-boundary guest: try to signal a process that is NOT the guest.
// The recorder must abort loudly. If this ever reaches its exit(0), retrace signalled pid 1.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    mov  x0, #1                 // pid 1 (launchd)
    mov  x1, #9                 // SIGKILL
    mov  x16, #37               // SYS_kill
    svc  #0x80
    mov  x0, #0
    mov  x16, #1                // SYS_exit
    svc  #0x80
