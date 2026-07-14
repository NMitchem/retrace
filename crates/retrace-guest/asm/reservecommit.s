.section __TEXT,__text
.global _start
.p2align 2
// M2-mmapcommit Task 1 micro-guest. Reserve a PROT_NONE region via _kernelrpc_mach_vm_map_trap
// (svc -15, cur_protection = 0), then first-touch two DIFFERENT pages well inside it. The
// reservation is bookkeeping-only (unbacked), so each first touch faults; commit_reserved_page
// must demand-commit exactly the faulting page with a fresh zeroed anon page — proving
// reserve -> fault -> zero-fill commit -> store -> load on record, and byte-identical on replay.
// Exit 0 and print "\xAB\xCD" iff both stored sentinels read back. The second store lands in a
// different 16 KiB page from the first, proving per-page (not per-reservation) commit granularity.
_start:
    adrp x21, addrbuf@PAGE
    add  x21, x21, addrbuf@PAGEOFF
    str  xzr, [x21]              // ANYWHERE hint = 0 (no preferred address)

    // _kernelrpc_mach_vm_map_trap(target, &address, size, mask, flags, cur_protection)
    mov  x0, #0                  // target (ignored by the box's intercept)
    mov  x1, x21                 // &address: holds the hint; box writes the chosen base back here
    movz x2, #0x10, lsl #16      // size = 0x100000 (1 MiB reservation)
    mov  x3, #0                  // mask
    mov  x4, #1                  // flags = VM_FLAGS_ANYWHERE
    mov  x5, #0                  // cur_protection = 0  => PROT_NONE reservation (the split trigger)
    mov  x16, #-15               // _kernelrpc_mach_vm_map_trap
    svc  #0x80
    cbnz x0, fail                // KERN_SUCCESS == 0

    ldr  x22, [x21]              // x22 = reserved base (written back by the box)

    // First-touch page 16 (base + 0x40000): faults -> demand-commit -> store sentinel 0xAB.
    add  x23, x22, #0x40, lsl #12
    mov  w9, #0xAB
    strb w9, [x23]
    // First-touch a DIFFERENT page, page 2 (base + 0x8000): a separate per-page commit.
    add  x24, x22, #0x8, lsl #12
    mov  w9, #0xCD
    strb w9, [x24]

    // Read both sentinels back from the freshly-committed pages.
    ldrb w9,  [x23]             // expect 0xAB
    ldrb w10, [x24]            // expect 0xCD

    // Print them (deterministic stdout the replay must reproduce byte-for-byte).
    adrp x25, outbuf@PAGE
    add  x25, x25, outbuf@PAGEOFF
    strb w9,  [x25]
    strb w10, [x25, #1]
    mov  x0, #1                  // fd = stdout
    mov  x1, x25
    mov  x2, #2
    mov  x16, #4                 // SYS_write
    svc  #0x80

    // Verify and exit.
    cmp  w9, #0xAB
    b.ne fail
    cmp  w10, #0xCD
    b.ne fail
    mov  x0, #0
    b    do_exit
fail:
    mov  x0, #1
do_exit:
    mov  x16, #1                 // SYS_exit
    svc  #0x80

.section __DATA,__data
.p2align 3
addrbuf: .quad 0
outbuf:  .space 8
