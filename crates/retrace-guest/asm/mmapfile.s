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
    mov  x19, x0                 // fd

    // mmap(0, 0x4000, PROT_READ(1), MAP_PRIVATE(2), fd, 0) -- file-backed: no MAP_ANON bit set
    mov  x0, #0
    mov  x1, #0x4000
    mov  x2, #1                  // PROT_READ
    mov  x3, #2                  // MAP_PRIVATE (no MAP_ANON => file-backed)
    mov  x4, x19                  // fd
    mov  x5, #0                  // offset
    mov  x16, #197               // SYS_mmap
    svc  #0x80
    mov  x20, x0                  // mapped addr

    // load the first byte of the mapped file
    ldrb w9, [x20]
    adrp x10, byte@PAGE
    add  x10, x10, byte@PAGEOFF
    strb w9, [x10]

    // write(1, &byte, 1)
    mov  x0, #1
    mov  x1, x10
    mov  x2, #1
    mov  x16, #4                  // SYS_write
    svc  #0x80

    // exit(0)
    mov  x0, #0
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
.p2align 4
byte: .space 8
// `path:` is appended by the build script (generated) so it matches the fixture location.
