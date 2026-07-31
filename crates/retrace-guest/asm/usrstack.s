// M8-stack. Exercises the three defects the milestone fixes, then publishes the results on
// stdout as four little-endian u64s so a test can assert on them:
//   [0]  kern.usrstack64   (sysctl {CTL_KERN=1, KERN_USRSTACK64=59})
//   [8]  rlim_cur          (getrlimit RLIMIT_STACK=3)
//   [16] rlim_max
//   [24] mmap return       (an anonymous MAP_FIXED request at 0xB_0000_0000)
//
// The MAP_FIXED target is deliberately an unrelated free IPA, NOT this guest's own stack bottom:
// on the static load path the whole guest stack is one 16 KiB page, so a MAP_FIXED there would
// unmap the stack this code is running on.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // sysctl(mib, 2, &out[0], &oldlen, NULL, 0)
    adrp x0, mib@PAGE
    add  x0, x0, mib@PAGEOFF
    mov  x1, #2
    adrp x2, out@PAGE
    add  x2, x2, out@PAGEOFF
    adrp x3, oldlen@PAGE
    add  x3, x3, oldlen@PAGEOFF
    mov  x4, #0
    mov  x5, #0
    mov  x16, #202                 // SYS___sysctl
    svc  #0x80

    // getrlimit(RLIMIT_STACK=3, &out[8])   -- writes rlim_cur then rlim_max
    mov  x0, #3
    adrp x1, out@PAGE
    add  x1, x1, out@PAGEOFF
    add  x1, x1, #8
    mov  x16, #194                 // SYS_getrlimit
    svc  #0x80

    // mmap(0xB_0000_0000, 0x4000, PROT_READ|PROT_WRITE, MAP_ANON|MAP_PRIVATE|MAP_FIXED, -1, 0)
    movz x0, #0xB, lsl #32
    mov  x1, #0x4000
    mov  x2, #3                    // PROT_READ|PROT_WRITE
    movz x3, #0x1012               // MAP_ANON(0x1000)|MAP_FIXED(0x10)|MAP_PRIVATE(0x02)
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197                 // SYS_mmap
    svc  #0x80
    adrp x9, out@PAGE
    add  x9, x9, out@PAGEOFF
    str  x0, [x9, #24]

    // write(1, out, 32)
    mov  x0, #1
    adrp x1, out@PAGE
    add  x1, x1, out@PAGEOFF
    mov  x2, #32
    mov  x16, #4                   // SYS_write
    svc  #0x80

    // exit(0)
    mov  x0, #0
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
.p2align 4
out:      .space 32                // usrstack, rlim_cur, rlim_max, mmap_ret
oldlen:   .quad 8                  // in/out: sizeof(u64)
mib:      .long 1                  // CTL_KERN
          .long 59                 // KERN_USRSTACK64
