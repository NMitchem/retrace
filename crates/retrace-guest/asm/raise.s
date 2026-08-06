// M11 mechanism guest: raise SIGABRT on ourselves via kill(getpid(), SIGABRT).
// kill(37) rather than __pthread_kill(328) because a freestanding guest has no thread port
// without a mach trap, and this shape also exercises the self-pid safety check — getpid is not
// intercepted, so it returns retrace's own pid (measured, M11 Task 1 Step 0 answer 2).
// The raise is TERMINAL: the exit(1) below must never execute.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    mov  x16, #20               // SYS_getpid
    svc  #0x80                  // x0 = pid
    mov  x1, #6                 // SIGABRT
    mov  x16, #37               // SYS_kill
    svc  #0x80
    mov  x0, #1                 // UNREACHABLE — a nonzero exit makes a missed terminal loud
    mov  x16, #1                // SYS_exit
    svc  #0x80
