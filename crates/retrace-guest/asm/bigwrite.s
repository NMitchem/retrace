// M30: a guest whose single write() hands the kernel MORE than PTR_WINDOW_CAP (64 KiB) of its own
// memory to READ. The mirror image of `bigread`, and the fixture for the Critical the M30 canary
// introduced.
//
// The canary is written into guest memory just past each argument's diff window and restored before
// the vCPU resumes, so no guest can see it. The KERNEL can: when it reads past a diff window it
// consumes those bytes as data. The recorded run's output is then silently wrong, while record and
// replay stay bit-identical — the bytes are restored, so nothing diverges. That is the failure a
// determinism oracle cannot see, which is why this guest asserts on its OUTPUT and not on an exit
// code (`crashy_e2e`'s rule: never assert on something a weaker failure also produces).
//
// **x4 is set deliberately, and that is the whole fixture.** The reviewer's C reproduction was
// corrupted through a STALE x4 left holding `buf + 128` by the compiler — incidental, and a
// different clang would not reproduce it. Here it is written on purpose, because the hazard is
// exactly that ANY mapped register value plants a canary 64 KiB downstream of itself: x4's band
// lands at buf+0x10080, deep inside the 128 KiB the kernel reads. Note the shrink cannot save the
// buffer either — x4's own 64 KiB window shrinks x1's band to zero, so x1 is unguarded AND x4 is
// the one that corrupts.
//
// The buffer is filled from the Mach-O image (`.space` with a fill byte) rather than by a store
// loop: fewer instructions, and every byte of the expected output is then a property of the file
// rather than of the guest's arithmetic.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // write(1, buf, 0x20000)
    mov  x0, #1
    adrp x1, buf@PAGE
    add  x1, x1, buf@PAGEOFF
    movz x2, #0x2, lsl #16      // 0x20000 = 128 KiB, twice PTR_WINDOW_CAP
    add  x4, x1, #128           // the stale-register condition, made deliberate — see above
    mov  x16, #4                // SYS_write
    svc  #0x80

    // exit(0)
    mov  x0, #0
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
.p2align 4
buf:      .space 0x20000, 0x41
