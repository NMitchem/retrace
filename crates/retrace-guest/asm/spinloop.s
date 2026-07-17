.section __TEXT,__text
.global _start
.p2align 2
_start:
    mov  x0, #300                // window 1 (landmark 1, up to `write`): a modest spin, ~606 insns
loop1:
    subs x0, x0, #1
    b.ne loop1
    mov  x0, #1                  // fd = stdout
    adrp x1, msg@PAGE
    add  x1, x1, msg@PAGEOFF
    mov  x2, #6                  // len
    mov  x16, #4                 // SYS_write
    svc  #0x80
    mov  x0, #2000                // window 2 (landmark 2, up to `exit`): the huge spin, ~4003 insns
loop2:
    subs x0, x0, #1
    b.ne loop2
    mov  x0, #0                  // status
    mov  x16, #1                 // SYS_exit
    svc  #0x80
.section __DATA,__data
msg:
    .ascii "spin!\n"
