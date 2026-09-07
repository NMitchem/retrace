// A guest that exercises both sides of M29's DerefU64 refusal, in one program and in this
// order: the LEGAL call first (oldp == NULL, the "just tell me the size" form, which has no
// destination to bound), then an ILLEGAL one whose *oldlenp is larger than any backing.
//
// The order matters: the second call is expected to abort the recorder, so anything that must
// be observed has to happen before it.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // mib[0] = CTL_KERN (1), mib[1] = KERN_OSTYPE (1)
    adrp x9, mib@PAGE
    add  x9, x9, mib@PAGEOFF
    mov  w10, #1
    str  w10, [x9]
    str  w10, [x9, #4]

    // ---- call 1: sysctl(mib, 2, NULL, &oldlen, NULL, 0) — legal, no destination buffer.
    adrp x11, oldlen@PAGE
    add  x11, x11, oldlen@PAGEOFF
    mov  x12, #0
    str  x12, [x11]
    mov  x0, x9
    mov  x1, #2
    mov  x2, #0                 // oldp == NULL
    mov  x3, x11                // oldlenp
    mov  x4, #0
    mov  x5, #0
    mov  x16, #202              // SYS_sysctl
    svc  #0x80

    // ---- call 2: *oldlenp = 1 << 40, oldp = buf — far past any backing.
    mov  x12, #1
    lsl  x12, x12, #40
    str  x12, [x11]
    adrp x13, buf@PAGE
    add  x13, x13, buf@PAGEOFF
    mov  x0, x9
    mov  x1, #2
    mov  x2, x13                // oldp = buf
    mov  x3, x11                // oldlenp, now 1 TiB
    mov  x4, #0
    mov  x5, #0
    mov  x16, #202
    svc  #0x80

    // exit(0) — reached only if the refusal did not fire, which is itself the finding.
    mov  x0, #0
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
.p2align 4
mib:      .space 16
oldlen:   .space 8
buf:      .space 64
