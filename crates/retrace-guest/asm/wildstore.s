.section __TEXT,__text
.global _start
.p2align 2
// M2-mmapcommit Task 1 fail-loud negative guest. Store to a WILD address (0xB_0000_0000 = 44 GiB):
// inside the 36-bit IPA space so stage-1 identity resolves it, but backed by NOTHING and inside no
// reservation. The store must take a stage-2 translation fault that stays fatal — commit_reserved_
// page must refuse to materialize it (returns false), so a genuine wild pointer never gets silently
// backed. The box test drives this to Stop::Other and asserts the committer refuses the IPA.
_start:
    movz x0, #0xB, lsl #32       // wild address 0xB_0000_0000: unbacked, in no reservation
    mov  w1, #0x99
    strb w1, [x0]                // stage-2 translation fault -> Stop::Other (stays fatal)
    // Unreached (the store above faults). Exit 0 only if it somehow did not.
    mov  x0, #0
    mov  x16, #1                 // SYS_exit
    svc  #0x80
