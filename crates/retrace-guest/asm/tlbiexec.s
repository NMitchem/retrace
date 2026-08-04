// M9. The TLBI capability fixture: a MAP_FIXED PROT_EXEC mapping landing inside a backing the guest
// has ALREADY TRANSLATED must become executable. Before the TLBI oracle, place_fixed refused this
// outright ("exec promotion of an already-translated block would need a guest TLBI the VMM cannot
// issue") and the RECORDER ABORTED — exit 101, no guest error.
//
// This is dyld's non-cache-dylib strategy in miniature: reserve a span, touch it, then drop an
// executable segment into it at a fixed address.
//
// Exits with the mapped code's return value (42), so a wrong answer cannot look like success.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // mmap(0, 0x8000, PROT_READ|PROT_WRITE(3), MAP_ANON|MAP_PRIVATE(0x1002), -1, 0) -> 2 pages
    mov  x0, #0
    mov  x1, #0x8000
    mov  x2, #3
    movz x3, #0x1002
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197                 // SYS_mmap
    svc  #0x80
    mov  x19, x0                   // reservation base

    // TOUCH IT. This is the whole point: the store forces a stage-1 walk, so the block is
    // translated and its entry cached as DATA (RW, UXN) before the exec map arrives.
    mov  w9, #0x5A
    strb w9, [x19]
    ldrb w10, [x19]                // read it back too, so the entry is definitely live

    // open(path, O_RDONLY=0, 0) -- the file of code
    adrp x0, path@PAGE
    add  x0, x0, path@PAGEOFF
    mov  x1, #0
    mov  x2, #0
    mov  x16, #5                   // SYS_open
    svc  #0x80
    mov  x20, x0                   // fd

    // mmap(base, 0x4000, PROT_READ|PROT_EXEC(5), MAP_FIXED|MAP_PRIVATE(0x12), fd, 0)
    // FIXED, exec, and wholly CONTAINED in the live backing above -> the case place_fixed refused.
    mov  x0, x19
    mov  x1, #0x4000
    mov  x2, #5                    // PROT_READ | PROT_EXEC
    movz x3, #0x12                 // MAP_FIXED | MAP_PRIVATE (no MAP_ANON => file-backed)
    mov  x4, x20                   // fd
    mov  x5, #0
    mov  x16, #197
    svc  #0x80
    mov  x21, x0                   // must equal x19

    // Execute from it. The payload is `movz x0,#42 ; ret`.
    blr  x21

    // exit(x0)  -- 42 only if the flipped page really became executable
    mov  x16, #1                   // SYS_exit
    svc  #0x80

// `path:` is appended by the build script (generated) so it matches the fixture location.
