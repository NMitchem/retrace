// M13 t10 fail-loud negative. mprotect(PROT_NONE) over a page inside a PROT_NONE RESERVATION that
// was never committed. M13 models no-access only on BACKED pages: an unbacked protected page would
// fault at STAGE 2, where commit_reserved_page would silently materialize it rather than let it
// fault. So the box must ASSERT here, not quietly succeed.
//
// Reserves via _kernelrpc_mach_vm_map_trap (svc -15) with cur_protection = 0 — the same split
// trigger reservecommit.s uses — then mprotects a page inside it WITHOUT ever touching it, so
// commit_reserved_page never runs and the page stays genuinely unbacked.
//
// Exit 7 means the protect was allowed (the failure this guest exists to catch); exit 1 means the
// reservation itself failed, which is a different bug.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    adrp x21, addrbuf@PAGE
    add  x21, x21, addrbuf@PAGEOFF
    str  xzr, [x21]              // ANYWHERE hint = 0 (no preferred address)

    // _kernelrpc_mach_vm_map_trap(target, &address, size, mask, flags, cur_protection)
    mov  x0, #0                  // target (ignored by the box's intercept)
    mov  x1, x21                 // &address: box writes the chosen base back here
    movz x2, #0x10, lsl #16      // size = 0x100000 (1 MiB reservation)
    mov  x3, #0                  // mask
    mov  x4, #1                  // flags = VM_FLAGS_ANYWHERE
    mov  x5, #0                  // cur_protection = 0  => a RESERVATION, never backed
    mov  x16, #-15               // _kernelrpc_mach_vm_map_trap
    svc  #0x80
    cbnz x0, fail                // KERN_SUCCESS == 0

    ldr  x22, [x21]              // x22 = the reserved base, written back by the box

    // mprotect(base, 0x4000, PROT_NONE) on a page with NO backing. Must fail loud in the box.
    mov  x0, x22
    mov  x1, #0x4000
    mov  x2, #0                  // PROT_NONE
    mov  x16, #74                // SYS_mprotect
    svc  #0x80

    mov  x0, #7                  // never reached: the box must have asserted
    b    do_exit
fail:
    mov  x0, #1
do_exit:
    mov  x16, #1                 // SYS_exit
    svc  #0x80

.section __DATA,__data
.p2align 3
addrbuf: .quad 0
