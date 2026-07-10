.section __TEXT,__text
.global _start
.p2align 2
// P = 0x10000000 (canonical low VA under both 36- and 47-bit). Sign with pacda (APDA key + fixed
// modifier), then strip with objc's 47-bit ISA_MASK, write the 8-byte result, exit 0.
_start:
    movz x19, #0x1000, lsl #16       // P = 0x0000_0000_1000_0000
    mov  x0, x19
    movz x1, #0x1234                 // modifier (fixed; tweak only to force a RED under 36-bit)
    pacda x0, x1                     // x0 = signed P (PAC field per TCR_EL1.T0SZ)
    movz x2, #0xffff
    movk x2, #0xffff, lsl #16
    movk x2, #0x7fff, lsl #32        // x2 = 0x0000_7FFF_FFFF_FFFF (objc ISA_MASK)
    and  x0, x0, x2                  // objc-style strip
    adrp x3, buf@PAGE
    add  x3, x3, buf@PAGEOFF
    str  x0, [x3]
    mov  x0, #1                      // write(1, buf, 8)
    mov  x1, x3
    mov  x2, #8
    mov  x16, #4
    svc  #0x80
    mov  x0, #0                      // exit(0)
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
.p2align 3
buf: .space 8
