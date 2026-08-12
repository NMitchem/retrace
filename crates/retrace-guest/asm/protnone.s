// M13 t7. mprotect(PROT_NONE) must actually deny access — and the TLBI must actually invalidate.
//
// The pre-protect store is load-bearing: it puts a WRITABLE translation for the page in the TLB.
// If protect_none stamps ATTR_NONE without flushing, the second store hits that stale entry and
// SUCCEEDS, and this guest exits 0 instead of faulting. A guest that protected a never-touched page
// would pass with or without the flush — vacuously. That is why the touch comes first.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // p = mmap(NULL, 0x4000, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANON, -1, 0)
    mov  x0, #0
    mov  x1, #0x4000
    mov  x2, #3                 // PROT_READ|PROT_WRITE
    mov  x3, #0x1002            // MAP_PRIVATE|MAP_ANON
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197              // SYS_mmap
    svc  #0x80
    mov  x19, x0                // keep the address

    // Touch it: this is what populates the TLB with a writable entry.
    mov  x9, #0xAAAA
    str  x9, [x19]

    // mprotect(p, 0x4000, PROT_NONE)
    mov  x0, x19
    mov  x1, #0x4000
    mov  x2, #0                 // PROT_NONE
    mov  x16, #74               // SYS_mprotect
    svc  #0x80

    // The store under test. It MUST fault; reaching the exit below is the failure mode.
    mov  x9, #0xBBBB
    str  x9, [x19]              // <-- must take a stage-1 permission fault

    mov  x0, #7                 // "protection was not enforced" — never reached when M13 works
    mov  x16, #1                // SYS_exit
    svc  #0x80
