// M13 t9. The same protection, through mach_vm_protect (svc -14) instead of mprotect (74). A
// separate dispatch arm that would otherwise be covered by nothing: before M13 it returned
// KERN_SUCCESS without calling into the box at all.
//
// _kernelrpc_mach_vm_protect_trap(target, addr, size, set_maximum, new_protection).
// As with protnone.s, the pre-protect store is what makes the TLBI load-bearing.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    mov  x0, #0
    mov  x1, #0x4000
    mov  x2, #3                 // PROT_READ|PROT_WRITE
    mov  x3, #0x1002            // MAP_PRIVATE|MAP_ANON
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197              // SYS_mmap
    svc  #0x80
    mov  x19, x0

    mov  x9, #0xAAAA
    str  x9, [x19]              // touch: populate the TLB

    // mach_vm_protect(task, addr=x19, size=0x4000, set_maximum=0, new_protection=0)
    mov  x0, #0                 // target task (ignored by retrace's arm)
    mov  x1, x19                // addr
    mov  x2, #0x4000            // size
    mov  x3, #0                 // set_maximum = FALSE
    mov  x4, #0                 // new_protection = PROT_NONE
    mov  x16, #-14              // _kernelrpc_mach_vm_protect_trap
    svc  #0x80

    mov  x9, #0xBBBB
    str  x9, [x19]              // <-- must take a stage-1 permission fault

    mov  x0, #7                 // "protection was not enforced" — never reached when M13 works
    mov  x16, #1
    svc  #0x80
