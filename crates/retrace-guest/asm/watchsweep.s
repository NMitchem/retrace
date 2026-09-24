// M40: ONE store instruction sweeps a 64-element buffer, so it runs on 40 OTHER addresses before it
// reaches the watched element buf[40] (t0 M7's class, reduced: a watch hit must be resolved by
// ADDRESS, not by pc). A second, DIFFERENT store then rewrites buf[40]. write(1, &buf[40], 8)
// publishes the element's address in the trace args (the WATCHLOOP convention), then exit(0).
// Every sweep value is non-zero, so each store changes its element and a step+read oracle sees it.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    adrp x1, buf@PAGE
    add  x1, x1, buf@PAGEOFF
    mov  x3, #0                     // i
    movz x2, #0x1111
    movk x2, #0x1111, lsl #16
    movk x2, #0x1111, lsl #32
    movk x2, #0x1111, lsl #48       // x2 = 0x1111111111111111
sweep:
    add  x4, x2, x3                 // value = 0x1111111111111111 + i
    str  x4, [x1, x3, lsl #3]       // THE sweeping store: one pc, 64 addresses
    add  x3, x3, #1
    cmp  x3, #64
    b.lt sweep
    mov  x5, #0xbeef
    str  x5, [x1, #320]             // the second writer: buf[40] (40 * 8 = 320), a different pc
    mov  x0, #1
    add  x1, x1, #320               // &buf[40]
    mov  x2, #8
    mov  x16, #4                    // SYS_write(1, &buf[40], 8)
    svc  #0x80
    mov  x0, #0
    mov  x16, #1                    // SYS_exit(0)
    svc  #0x80
.section __DATA,__data
.p2align 3
buf: .space 512                     // 64 quads, zero-initialised
