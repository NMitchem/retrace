.section __TEXT,__text
.global _start
_start:
    adrp x1, _start@PAGE       // any valid code pointer
    mov  x9, #0                // modifier
    mov  x8, x1
    pacia x8, x9               // sign
    cmp  x8, x1
    cset x4, eq                // x4 = 1 if signing was a no-op (PAC disabled) -> failure
    autia x8, x9               // authenticate
    cmp  x8, x1
    cset x5, ne                // x5 = 1 if auth did not recover the original -> failure
    orr  x0, x4, x5            // x0 = 0 iff (PAC engaged AND auth round-trips)
    mov  x16, #1
    svc  #0x80
