// M8-stack fast-follow. A MAP_FIXED mmap for an address the guest's 36-bit IPA space cannot hold
// must come back to the GUEST as an error -- carry set, x0 = EINVAL -- exactly as the real kernel
// answers it. Before the fix that wild address reached hv_vm_map, which rejected it with
// HV_BAD_ARGUMENT through an `expect`, taking the RECORDER down (exit 101) with no HVF fault and no
// guest-visible error at all.
//
// The address here (0xffffffffffa04000) is the one libstd's `install_main_guard` actually computes
// inside `hello_rust`: `pthread_get_stackaddr_np() - pthread_get_stacksize_np()`, where macOS 26's
// libpthread reports a constant 8 MiB-minus-a-page size for the main thread, underflowing against
// the box's smaller stack.
//
// Publishes three little-endian u64s on stdout:
//   [0] carry after the wild mmap (1 = the syscall reported failure)
//   [8] x0 after the wild mmap    (the errno; EINVAL = 22)
//  [16] a byte stored + read back through a NORMAL anon mmap taken afterwards (0x5a), proving the
//       guest runs on past the rejection and the box is still usable
.section __TEXT,__text
.global _start
.p2align 2
_start:
    adrp x21, out@PAGE
    add  x21, x21, out@PAGEOFF

    // mmap(0xffffffffffa04000, 0x4000, RW, MAP_ANON|MAP_PRIVATE|MAP_FIXED) -- must fail, not crash.
    movz x0, #0x4000
    movk x0, #0xffa0, lsl #16
    movk x0, #0xffff, lsl #32
    movk x0, #0xffff, lsl #48
    mov  x1, #0x4000
    mov  x2, #3                    // PROT_READ|PROT_WRITE
    movz x3, #0x1012               // MAP_ANON|MAP_PRIVATE|MAP_FIXED
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197                 // SYS_mmap
    svc  #0x80
    cset w9, cs                    // carry set => the syscall failed
    str  x9, [x21]                 // [0] carry
    str  x0, [x21, #8]             // [8] errno

    // A normal anon mmap still works afterwards: map, store a marker, read it back.
    mov  x0, #0
    mov  x1, #0x4000
    mov  x2, #3
    movz x3, #0x1002               // MAP_ANON|MAP_PRIVATE
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197
    svc  #0x80
    mov  x19, x0
    mov  w9, #0x5a
    strb w9, [x19]
    ldrb w9, [x19]
    str  x9, [x21, #16]            // [16] expect 0x5a

    // write(1, out, 24)
    mov  x0, #1
    mov  x1, x21
    mov  x2, #24
    mov  x16, #4                   // SYS_write
    svc  #0x80

    // exit(0)
    mov  x0, #0
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
.p2align 4
out:      .space 24
