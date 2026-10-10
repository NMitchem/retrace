.section __TEXT,__text
.global _start
.p2align 2
// M2-mmapcommit Task 1 negative guest. Store to a WILD address (0xB_0000_0000 = 44 GiB): inside
// the 36-bit IPA space so stage-1 identity resolves it, but backed by NOTHING and inside no
// reservation. The store takes a stage-2 translation fault that commit_reserved_page must refuse
// to materialize (returns false), so a genuine wild pointer never gets silently backed. Since M49
// the record loop then converts it into the stage-1 fault native takes (F2): a recorded crash, not
// a recorder error (retrace-box/tests/stage2.rs, retrace-core/tests/signals.rs).
_start:
    movz x0, #0xB, lsl #32       // wild address 0xB_0000_0000: unbacked, in no reservation
    mov  w1, #0x99
    strb w1, [x0]                // stage-2 translation fault -> Stop::Other, then M49 F2's Stop::Fault
    // Unreached (the store above faults). Exit 0 only if it somehow did not.
    mov  x0, #0
    mov  x16, #1                 // SYS_exit
    svc  #0x80
