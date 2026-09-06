// M28: a guest whose sysctl FAILS with a deliberately undersized buffer.
//
// `forward_and_diff` skips write capture entirely when the syscall sets the carry flag ("A failed
// syscall wrote nothing to the guest's buffers"), and the M27 guard band lives INSIDE that same
// `if !err` — so the detector is off on this path too. The README has named this hole since M27 and
// named this exact suspect: sysctl with an undersized `oldp` returns ENOMEM and MAY copy out what
// fits. Nothing has measured it.
//
// This guest asks for kern.ostype ("Darwin") into a 2-byte buffer, then emits those 2 bytes. If the
// kernel wrote despite failing, the recording shows them and a replay — which captured no writes —
// shows zeros. Same shape as `bigread`: a silent truncation becomes visible OUTPUT rather than a
// divergence the oracle cannot see, because (num, args) are identical on both sides.
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

    // *oldlenp = 2, deliberately smaller than "Darwin\0"
    adrp x11, oldlen@PAGE
    add  x11, x11, oldlen@PAGEOFF
    mov  x12, #2
    str  x12, [x11]

    // sysctl(mib, 2, buf, oldlenp, NULL, 0)
    mov  x0, x9
    mov  x1, #2
    adrp x2, buf@PAGE
    add  x2, x2, buf@PAGEOFF
    mov  x3, x11
    mov  x4, #0
    mov  x5, #0
    mov  x16, #202              // SYS___sysctl
    svc  #0x80

    // write(1, buf, 2) — the bytes the kernel may or may not have written
    mov  x0, #1
    adrp x1, buf@PAGE
    add  x1, x1, buf@PAGEOFF
    mov  x2, #2
    mov  x16, #4                // SYS_write
    svc  #0x80

    // exit(0)
    mov  x0, #0
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
.p2align 4
mib:      .space 16
oldlen:   .space 8
buf:      .space 64
