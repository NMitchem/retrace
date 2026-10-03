// M48: the SPRR register and EL0 cache maintenance on a static guest (no commpage).
// by value, because a static guest has no commpage to admit a value from. exit(2) means the mrs
// read nonzero.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    mrs  x0, S3_6_C15_C1_5
    cbnz x0, 1f
    adr  x3, _start
    dsb  ish
    isb
    mov  x1, #1
    msr  S3_6_C15_C1_5, x1
    mov  x0, #0
    b    2f
1:  mov  x0, #2
2:  mov  x16, #1
    svc  #0x80
