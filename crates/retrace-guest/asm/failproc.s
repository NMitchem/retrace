// M35: a guest whose sysctl(kern.proc.all) FAILS (ENOMEM) after the kernel has already copied
// one full kinfo_proc into its buffer — the DATA half of the `if !err` hole.
//
// xnu's kern.proc handlers (bsd/kern/kern_sysctl.c: sysdoproc_callback copies out each record
// while it fits; sysctl_prochandle returns ENOMEM when `needed > oldlen`) write what fits and
// THEN fail. Measured on the host before this guest was written: with *oldlenp = 648 =
// sizeof(kinfo_proc), ret=-1 errno=12, 648 bytes changed, nothing past them, *oldlenp -> 0.
//
// The guest then writes the first 8 bytes of the record to stdout, so a capture that misses
// them is visible as OUTPUT (the bigread shape) and not only at the terminal memory compare.
// The bytes themselves are host state (whichever process the kernel iterates first) — recorded
// and replayed, never regenerated, like task_info's audit token.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // mib = { CTL_KERN (1), KERN_PROC (14), KERN_PROC_ALL (0) }
    adrp x9, mib@PAGE
    add  x9, x9, mib@PAGEOFF
    mov  w10, #1
    str  w10, [x9]
    mov  w10, #14
    str  w10, [x9, #4]
    str  wzr, [x9, #8]

    // *oldlenp = 648: room for exactly one record, and the machine runs hundreds of processes
    adrp x11, oldlen@PAGE
    add  x11, x11, oldlen@PAGEOFF
    mov  x12, #648
    str  x12, [x11]

    // sysctl(mib, 3, buf, oldlenp, NULL, 0)
    mov  x0, x9
    mov  x1, #3
    adrp x2, buf@PAGE
    add  x2, x2, buf@PAGEOFF
    mov  x3, x11
    mov  x4, #0
    mov  x5, #0
    mov  x16, #202              // SYS___sysctl
    svc  #0x80

    // write(1, buf, 8) — the first 8 bytes the kernel copied out despite failing
    mov  x0, #1
    adrp x1, buf@PAGE
    add  x1, x1, buf@PAGEOFF
    mov  x2, #8
    mov  x16, #4                // SYS_write
    svc  #0x80

    // exit(0)
    mov  x0, #0
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
.p2align 4
mib:      .space 16
oldlen:   .space 8
buf:      .space 648
