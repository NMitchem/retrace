.section __TEXT,__text
.global _start
.p2align 2
_start:
    // open(path, O_RDONLY=0, 0)
    adrp x0, path@PAGE
    add  x0, x0, path@PAGEOFF
    mov  x1, #0                 // O_RDONLY
    mov  x2, #0
    mov  x16, #5                // SYS_open
    svc  #0x80
    mov  x19, x0                // save fd

    // fstat(fd, statbuf)  -- exercises a struct-write memory-diff
    mov  x0, x19
    adrp x1, statbuf@PAGE
    add  x1, x1, statbuf@PAGEOFF
    mov  x16, #189              // SYS_fstat
    svc  #0x80

    // read(fd, buf, 19)   -- the money case: kernel writes file bytes into buf
    mov  x0, x19
    adrp x1, buf@PAGE
    add  x1, x1, buf@PAGEOFF
    mov  x2, #19               // fixture length "retrace-m1-fixture\n"
    mov  x16, #3               // SYS_read
    svc  #0x80
    mov  x20, x0               // nbytes read

    // write(1, buf, nbytes)  -- emit what we read
    mov  x0, #1
    adrp x1, buf@PAGE
    add  x1, x1, buf@PAGEOFF
    mov  x2, x20
    mov  x16, #4              // SYS_write
    svc  #0x80

    // close(fd)
    mov  x0, x19
    mov  x16, #6             // SYS_close
    svc  #0x80

    // exit(0)
    mov  x0, #0
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
.p2align 4
buf:      .space 64
statbuf:  .space 256
// `path:` is appended by the build script (generated) so it matches the fixture location.
