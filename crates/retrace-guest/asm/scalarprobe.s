// M37: a guest whose lseek OFFSET is 0x4000 = TRAMPOLINE_IPA, a mapped guest IPA on every load
// path. Before M37, forward_and_diff's per-register probe rewrote any register holding a mapped
// IPA to a host pointer — including this offset, which is a NUMBER — and the host lseek returned
// the trampoline's host address. The arg_kinds row marks lseek's offset Scalar; a Scalar is never
// probed now, so the call returns 0x4000. (M34 §4b found the same rewrite on the recorder's pid.)
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // fd = open("/etc/hosts", O_RDONLY)
    adrp x0, path@PAGE
    add  x0, x0, path@PAGEOFF
    mov  x1, #0
    mov  x2, #0
    mov  x16, #5                // SYS_open
    svc  #0x80
    mov  x19, x0
    // lseek(fd, 0x4000, SEEK_SET) — the offset is the probe's bait
    mov  x0, x19
    mov  x1, #0x4000
    mov  x2, #0
    mov  x16, #199              // SYS_lseek
    svc  #0x80
    // exit(0)
    mov  x0, #0
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
path: .asciz "/etc/hosts"
