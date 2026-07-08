.section __TEXT,__text
.global _start
.p2align 2
_start:
    // mmap A: mmap(NULL, 16384, PROT_READ|PROT_WRITE(3), MAP_ANON|MAP_PRIVATE(0x1002), -1, 0)
    mov  x0, #0
    mov  x1, #0x4000
    mov  x2, #3
    movz x3, #0x1002
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197             // SYS_mmap
    svc  #0x80
    mov  x19, x0               // A

    // store a byte pattern at [A] (ordinary store, no syscall)
    mov  w9, #0xAB
    strb w9, [x19]

    // munmap(A, 16384) — must actually release A so a later mapping can reuse address space
    mov  x0, x19
    mov  x1, #0x4000
    mov  x16, #73              // SYS_munmap
    svc  #0x80

    // mmap B: same shape as A
    mov  x0, #0
    mov  x1, #0x4000
    mov  x2, #3
    movz x3, #0x1002
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197              // SYS_mmap
    svc  #0x80
    mov  x20, x0                // B

    // store a different byte pattern at [B]
    mov  w9, #0xCD
    strb w9, [x20]

    // load back from B and compare against what we just stored
    ldrb w10, [x20]
    cmp  w10, #0xCD
    b.eq match
    mov  x0, #1                 // mismatch: exit 1
    b    do_exit
match:
    mov  x0, #0                 // match: exit 0
do_exit:
    mov  x16, #1                // SYS_exit
    svc  #0x80
