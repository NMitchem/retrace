.section __TEXT,__text
.global _start
.p2align 2
_start:
    // mmap(NULL, 16384, PROT_READ|PROT_WRITE(3), MAP_ANON|MAP_PRIVATE(0x1002), -1, 0)
    mov  x0, #0
    mov  x1, #0x4000
    mov  x2, #3
    movz x3, #0x1002
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197             // SYS_mmap
    svc  #0x80
    mov  x19, x0              // mapped addr

    // store a byte pattern 0xAB at [addr] and 0xCD at [addr+1] (ordinary stores, no syscall)
    mov  w9, #0xAB
    strb w9, [x19]
    mov  w9, #0xCD
    strb w9, [x19, #1]

    // read them back and write both to stdout (proves the mapping is live)
    mov  x0, #1
    mov  x2, #2
    mov  x1, x19
    mov  x16, #4              // SYS_write (buf = the mmap'd region)
    svc  #0x80

    // munmap(addr, 16384)
    mov  x0, x19
    mov  x1, #0x4000
    mov  x16, #73            // SYS_munmap
    svc  #0x80

    // exit(0)
    mov  x0, #0
    mov  x16, #1
    svc  #0x80
