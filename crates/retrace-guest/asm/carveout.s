.section __TEXT,__text
.global _start
.p2align 2
// M2-carveout Task 1 micro-guest: libmalloc's guarded-metadata carveout protocol in miniature.
// Reserve a PROT_NONE band via _kernelrpc_mach_vm_map_trap (svc -15, cur_protection=0), then punch
// an interior 0x10000 hole with mach_vm_deallocate (svc -12), then commit metadata via
// mach_vm_map(ANYWHERE, hint = reservation base, RW). On a real kernel the reservation occupies the
// band, so the hinted first-fit is FORCED into the carveout hole — the map must return the hole
// base (base + 0x10000), NOT the raw hint. The guest asserts that in-guest, stores/loads a sentinel
// through the returned address, prints it, and exits 0 iff placement + round-trip both hold.
// RED (pre-fix): the box honors the raw hint and returns `base`, so the placement check fails.
_start:
    adrp x21, addrbuf@PAGE
    add  x21, x21, addrbuf@PAGEOFF

    // --- reserve ANYWHERE PROT_NONE, size 0x40000 (256 KiB) ---
    str  xzr, [x21]              // ANYWHERE hint = 0 (box picks MMAP_BASE)
    mov  x0, #0                  // target (ignored by the box)
    mov  x1, x21                 // &address: box writes the chosen base back here
    movz x2, #0x4, lsl #16       // size = 0x40000
    mov  x3, #0                  // mask
    mov  x4, #1                  // flags = VM_FLAGS_ANYWHERE
    mov  x5, #0                  // cur_protection = 0  => PROT_NONE reservation
    mov  x16, #-15               // _kernelrpc_mach_vm_map_trap
    svc  #0x80
    cbnz x0, fail
    ldr  x22, [x21]             // x22 = reserved base

    // --- punch an interior hole: deallocate [base+0x10000, base+0x20000) (64 KiB) ---
    add  x23, x22, #0x10, lsl #12   // x23 = base + 0x10000  (the expected hole base)
    mov  x0, #0                  // target (ignored)
    mov  x1, x23                 // address
    movz x2, #0x1, lsl #16       // size = 0x10000
    mov  x16, #-12               // _kernelrpc_mach_vm_deallocate_trap
    svc  #0x80
    cbnz x0, fail

    // --- commit ANYWHERE with hint = reserved base, size 0x10000, RW ---
    str  x22, [x21]             // hint = reservation base (collides with the reservation)
    mov  x0, #0
    mov  x1, x21
    movz x2, #0x1, lsl #16       // size = 0x10000 (fits the hole exactly)
    mov  x3, #0                  // mask
    mov  x4, #1                  // flags = VM_FLAGS_ANYWHERE
    mov  x5, #3                  // cur_protection = RW  => a real backed commit
    mov  x16, #-15
    svc  #0x80
    cbnz x0, fail
    ldr  x24, [x21]             // x24 = returned address

    // --- placement assertion: the map must land in the carveout hole, not at the raw hint ---
    cmp  x24, x23
    b.ne fail                   // returned != base + 0x10000  =>  first-fit not kernel-faithful

    // --- store/load a sentinel through the returned (hole) address ---
    mov  w9, #0xAB
    strb w9, [x24]
    ldrb w10, [x24]
    cmp  w10, #0xAB
    b.ne fail

    // Print the sentinel (deterministic stdout the replay must reproduce byte-for-byte).
    adrp x25, outbuf@PAGE
    add  x25, x25, outbuf@PAGEOFF
    strb w10, [x25]
    mov  x0, #1                  // fd = stdout
    mov  x1, x25
    mov  x2, #1
    mov  x16, #4                 // SYS_write
    svc  #0x80

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
