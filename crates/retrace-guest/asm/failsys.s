.section __TEXT,__text
.global _start
_start:
    adrp x0, badpath@PAGE
    add  x0, x0, badpath@PAGEOFF
    mov  x1, #0               // O_RDONLY
    mov  x16, #5              // SYS_open
    svc  #0x80               // carry set on error; x0 = errno (ENOENT=2)
    mov  x1, x0              // save errno
    mov  x0, x1
    mov  x16, #1             // SYS_exit(errno)
    svc  #0x80
.section __DATA,__data
.p2align 3
badpath: .asciz "/no/such/retrace/path"
