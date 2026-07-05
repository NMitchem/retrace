.section __TEXT,__text
.global _start
.p2align 2
_start:
    mov  x0, #1                 // fd = stdout
    adrp x1, msg@PAGE
    add  x1, x1, msg@PAGEOFF    // buf
    mov  x2, #6                 // len
    mov  x16, #4                // SYS_write
    svc  #0x80
    mov  x0, #0                 // status
    mov  x16, #1                // SYS_exit
    svc  #0x80
.section __DATA,__data
msg:
    .ascii "hello\n"
