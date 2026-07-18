.section __TEXT,__text
.global _start
.p2align 2
_start:
    adrp x1, target@PAGE
    add  x1, x1, target@PAGEOFF
    mov  x2, #0                  // store value
    mov  x3, #8                  // 8 stores, values 1..=8, all from the SAME str pc
sloop:
    add  x2, x2, #1
    str  x2, [x1]                // THE watched store
    subs x3, x3, #1
    b.ne sloop
    adrp x4, target2@PAGE
    add  x4, x4, target2@PAGEOFF
    mov  w5, #0x5A
    strb w5, [x4]                // byte-0 store: the BAS negative (watch target2+4 must NOT fire)
    mov  x0, #1
    adrp x1, target@PAGE
    add  x1, x1, target@PAGEOFF
    mov  x2, #8
    mov  x16, #4                 // SYS_write(1, target, 8): publishes target's addr in the trace args
    svc  #0x80
    mov  x0, #0
    mov  x16, #1                 // SYS_exit
    svc  #0x80
.section __DATA,__data
.p2align 3
target:  .quad 0
target2: .quad 0
