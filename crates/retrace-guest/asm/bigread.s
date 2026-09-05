// M26: a guest whose single read() returns MORE than PTR_WINDOW_CAP (64 KiB).
//
// The point is the TAIL. `forward_and_diff` snapshots a pre-image window per pointer arg and
// diffs that same window afterwards; if the window is smaller than what the kernel actually
// wrote, the bytes past it are written into guest memory on record but never captured as an
// `Event::Syscall` write — so replay restores stale bytes there and the guest reads garbage.
//
// This guest reads 96 KiB (0x18000, 1.5x the cap) and then emits ONE byte: the very LAST one,
// which lives 32 KiB beyond a 64 KiB window. The fixture is all 'A' except that final 'Z', so a
// truncated capture shows up as a single wrong byte on stdout rather than as a divergence — the
// oracle cannot see it, because (num, args) are identical on both sides. That silence is the
// whole reason this guest exists.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // open(path, O_RDONLY=0, 0)
    adrp x0, path@PAGE
    add  x0, x0, path@PAGEOFF
    mov  x1, #0
    mov  x2, #0
    mov  x16, #5                // SYS_open
    svc  #0x80
    mov  x19, x0                // fd

    // read(fd, buf, 0x18000)
    mov  x0, x19
    adrp x1, buf@PAGE
    add  x1, x1, buf@PAGEOFF
    mov  x2, #0x18000           // 96 KiB — deliberately > PTR_WINDOW_CAP
    mov  x16, #3                // SYS_read
    svc  #0x80
    mov  x20, x0                // nbytes actually read

    // write(1, buf + 0x17fff, 1) -- the LAST byte, far past a 64 KiB window
    mov  x0, #1
    adrp x1, buf@PAGE
    add  x1, x1, buf@PAGEOFF
    movz x2, #0x7fff            // 0x17fff is not a single-instruction immediate
    movk x2, #0x1, lsl #16
    add  x1, x1, x2
    mov  x2, #1
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
buf:      .space 0x18000
// `path:` is appended by the build script (generated) so it matches the fixture location.
