// M30: a guest whose single write() hands the kernel MORE than PTR_WINDOW_CAP (64 KiB) of its own
// memory to READ. The mirror image of `bigread`, and the fixture for the Critical the M30 canary
// introduced.
//
// The canary is written into guest memory just past each argument's diff window and restored before
// the vCPU resumes, so no guest can see it. The KERNEL can: when it reads past a diff window it
// consumes those bytes as data. The recorded run's output is then silently wrong, while record and
// replay stay bit-identical — the bytes are restored, so nothing diverges. That is the failure a
// determinism oracle cannot see, which is why the test asserts on the OUTPUT FILE and not on an
// exit code (`crashy_e2e`'s rule: never assert on something a weaker failure also produces).
//
// **It writes to a FILE, and that is not incidental.** `retrace_arch::is_console_write` makes fd
// 0/1/2 a special case in `retrace-core`: those writes are mirrored out of guest memory and FAKED,
// never forwarded, so a guest writing to stdout never reaches `forward_and_diff` at all and cannot
// exercise this hazard. The first version of this fixture wrote to stdout and was measured
// vacuous — it stayed green with the fix reverted.
//
// **x4 is set deliberately, and that is the rest of the fixture.** The reviewer's C reproduction was
// corrupted through a STALE x4 the compiler happened to leave holding `buf + 128` — incidental, and
// a different clang would not reproduce it. Here it is written on purpose, because the hazard is
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
    // open(path, O_WRONLY|O_CREAT|O_TRUNC = 0x601, 0644)
    adrp x0, outpath@PAGE
    add  x0, x0, outpath@PAGEOFF
    movz x1, #0x601
    movz x2, #420               // 0644
    mov  x16, #5                // SYS_open
    svc  #0x80
    mov  x19, x0                // fd

    // write(fd, buf, 0x20000)
    mov  x0, x19
    adrp x1, buf@PAGE
    add  x1, x1, buf@PAGEOFF
    movz x2, #0x2, lsl #16      // 0x20000 = 128 KiB, twice PTR_WINDOW_CAP
    add  x4, x1, #128           // the stale-register condition, made deliberate — see above
    mov  x16, #4                // SYS_write
    svc  #0x80

    // close(fd)
    mov  x0, x19
    mov  x16, #6
    svc  #0x80

    // exit(0)
    mov  x0, #0
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
.p2align 4
buf:      .space 0x20000, 0x41
// `outpath:` is appended by the build script (generated) so it matches the OUT_DIR location.
