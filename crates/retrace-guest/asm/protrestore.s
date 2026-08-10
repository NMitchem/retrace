// M13 t7. The restore direction: a page returned from PROT_NONE to RW must be usable again.
//
// The pre-protect store puts a writable entry in the TLB, and the protect replaces it. If unprotect
// stamps ATTR_DATA without flushing, the post-restore store hits the stale RESTRICTIVE entry and
// faults, and this guest dies instead of exiting 0. So both directions of the flush are covered by
// the pair, and neither can pass vacuously.
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

    mov  x0, x19                // mprotect(p, 0x4000, PROT_NONE)
    mov  x1, #0x4000
    mov  x2, #0
    mov  x16, #74
    svc  #0x80

    mov  x0, x19                // mprotect(p, 0x4000, PROT_READ|PROT_WRITE)
    mov  x1, #0x4000
    mov  x2, #3
    mov  x16, #74
    svc  #0x80

    mov  x9, #0xBBBB
    str  x9, [x19]              // must SUCCEED: a stale restrictive entry would fault here
    ldr  x10, [x19]
    cmp  x9, x10
    b.ne fail

    mov  x0, #0                 // exit 0: protected, restored, and usable
    mov  x16, #1
    svc  #0x80
fail:
    mov  x0, #9                 // the value did not survive the round trip
    mov  x16, #1
    svc  #0x80
