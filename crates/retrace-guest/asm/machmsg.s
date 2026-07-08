.section __TEXT,__text
.global _start
.p2align 2
// Exit codes name the failing stage: 1 = mach_msg2 ret != MACH_MSG_SUCCESS,
// 2 = reply RetCode != KERN_SUCCESS. Success path prints "MK" and exits 0.
_start:
    mov  x16, #-28              // task_self_trap
    svc  #0x80
    mov  x19, x0                // x19 = task port name
    mov  x16, #-26              // mach_reply_port
    svc  #0x80
    mov  x20, x0                // x20 = reply port name

    // Build the 100-byte __Request___kernelrpc_mach_vm_map_t in `msgbuf` (offsets per plan).
    adrp x21, msgbuf@PAGE
    add  x21, x21, msgbuf@PAGEOFF
    movz w9, #0x1513            // msgh_bits = COMPLEX | remote COPY_SEND | local MAKE_SEND_ONCE
    movk w9, #0x8000, lsl #16
    str  w9, [x21]              // +0  bits
    mov  w9, #100
    str  w9, [x21, #4]          // +4  msgh_size (informational; kernel uses the register copy)
    str  w19, [x21, #8]         // +8  remote = task port
    str  w20, [x21, #12]        // +12 local  = reply port
    str  wzr, [x21, #16]        // +16 voucher
    movz w9, #4811
    str  w9, [x21, #20]         // +20 msgh_id
    mov  w9, #1
    str  w9, [x21, #24]         // +24 descriptor count
    str  wzr, [x21, #28]        // +28 desc.name = MACH_PORT_NULL (anonymous memory)
    str  wzr, [x21, #32]        // +32 desc.pad1
    movz w9, #0x13, lsl #16     // +36 pad2:16=0 | disposition:8=19 (COPY_SEND) | type:8=0 (PORT)
    str  w9, [x21, #36]
    str  xzr, [x21, #40]        // +40 NDR (ignored by the box's decoder)
    movz x9, #0x7, lsl #32      // address hint 0x700000000: ANYWHERE hint, box may honor or
                                // relocate it; falls inside the 4-40 GiB nano band but this
                                // standalone guest reserves no nano range, so it's free here
    str  x9, [x21, #48]         // +48 address
    movz x9, #0x8000
    str  x9, [x21, #56]         // +56 size = 0x8000
    str  xzr, [x21, #64]        // +64 mask
    mov  w9, #1                 // VM_FLAGS_ANYWHERE
    str  w9, [x21, #72]         // +72 flags
    str  wzr, [x21, #76]        // +76 offset lo (u64 @76, pack(4): two u32 stores)
    str  wzr, [x21, #80]        // +80 offset hi
    str  wzr, [x21, #84]        // +84 copy = FALSE
    mov  w9, #3                 // VM_PROT_READ|WRITE
    str  w9, [x21, #88]         // +88 cur_protection
    mov  w9, #7
    str  w9, [x21, #92]         // +92 max_protection
    mov  w9, #1                 // VM_INHERIT_COPY
    str  w9, [x21, #96]         // +96 inheritance

    // mach_msg2_trap(buf, SEND|RCV|KOBJECT, bits|100<<32, task|reply<<32,
    //                0|4811<<32, 1|reply<<32, 52, 0)
    mov  x0, x21
    movz x1, #0x2, lsl #32
    orr  x1, x1, #0x3
    movz x2, #0x1513
    movk x2, #0x8000, lsl #16
    movk x2, #100, lsl #32
    mov  x3, x19
    orr  x3, x3, x20, lsl #32
    movz x4, #4811, lsl #32
    mov  x5, #1
    orr  x5, x5, x20, lsl #32
    mov  x6, #52
    mov  x7, #0
    mov  x16, #-47
    svc  #0x80
    cbnz x0, fail1              // MACH_MSG_SUCCESS == 0

    ldr  w9, [x21, #32]         // reply RetCode (header 24 + NDR 8)
    cbnz w9, fail2              // KERN_SUCCESS == 0
    ldr  w9, [x21, #36]         // reply address lo (u64 @36, pack(4): two 4-aligned loads —
    ldr  w10, [x21, #40]        //   an unaligned ldr faults on MMU-off Device memory)
    orr  x22, x9, x10, lsl #32  // x22 = mapped guest address

    movz w9, #0x4D              // 'M'
    strb w9, [x22]              // store through the serviced mapping…
    movz w9, #0x4B              // 'K'
    strb w9, [x22, #1]
    mov  x0, #1                 // …and print it back (proves the memory is real + replayable)
    mov  x1, x22
    mov  x2, #2
    mov  x16, #4                // SYS_write
    svc  #0x80

    mov  x0, #0
    b    exit
fail1:
    mov  x0, #1
    b    exit
fail2:
    mov  x0, #2
exit:
    mov  x16, #1                // SYS_exit
    svc  #0x80

.section __DATA,__data
.p2align 3
msgbuf: .space 128
