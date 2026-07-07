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

    // mmap(0, 0x4000, PROT_READ|PROT_EXEC(5), MAP_PRIVATE(2), fd, 0)
    // prot has PROT_EXEC set => the VMM must promote this file-backed region to RO+exec
    // (ATTR_CODE) stage-1 pages so the guest can execute from it (W^X). MAP_PRIVATE, no
    // MAP_ANON => file-backed path.
    mov  x0, #0
    mov  x1, #0x4000
    mov  x2, #5                  // PROT_READ | PROT_EXEC
    mov  x3, #2                  // MAP_PRIVATE (no MAP_ANON => file-backed)
    mov  x4, x19                  // fd
    mov  x5, #0                  // offset
    mov  x16, #197               // SYS_mmap
    svc  #0x80
    mov  x20, x0                  // mapped code addr

    // call into the mapped code: it is `movz x0,#42 ; ret`, so it returns 42 in x0
    blr  x20

    // exit(x0)   -- x0 = 42 from the callee
    mov  x16, #1                 // SYS_exit
    svc  #0x80

// `path:` is appended by the build script (generated) so it matches the fixture location.
