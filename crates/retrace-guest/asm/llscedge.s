// M43 §3i (M42 final review, Minors 2-4): the step-path shapes M42 left panicking. A separate
// fixture, so llsc.s and llscbound.s and their coordinates stay frozen.
//
// Window 1: a load-exclusive whose base is also its destination, `ldxr x9, [x9]`, with no
// store-exclusive after it. Stepping it panicked at the retire: the base was read AFTER the load
// overwrote it. getpid ends the window.
//
// Window 2: a load-exclusive then a store-exclusive on a word in __TEXT, which EL0 can read and
// cannot write. Natively the monitor is held, so the store takes a permission fault: the recording
// ENDS here, in a crash (exit 139). Stepped, the shadow is set and the emulation panicked on the
// non-writable target.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    adrp x9, celledge@PAGE
    add  x9, x9, celledge@PAGEOFF
edge_alias:
    ldxr x9, [x9]                   // (1, 2): base == destination; x9 becomes 0x5a5a
    mov  x16, #20                   // SYS_getpid: ends window 1 at (1, 4)
    svc  #0x80
    adrp x12, edge_ro@PAGE
    add  x12, x12, edge_ro@PAGEOFF
edge_ro_ldx:
    ldxr w10, [x12]                 // (2, 2): reads the read-only word
edge_ro_stx:
    stxr w13, w10, [x12]            // (2, 3): faults natively (permission); the recording's terminal
    mov  x0, #0
    mov  x16, #1                    // SYS_exit(0): never reached in the recording
    svc  #0x80
.p2align 2
edge_ro: .word 7                    // in __TEXT: EL0 read-only (ATTR_CODE)

.section __DATA,__data
.p2align 3
celledge: .quad 0x5a5a
