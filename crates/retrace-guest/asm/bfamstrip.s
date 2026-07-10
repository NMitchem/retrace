.section __TEXT,__text
.global _start
.p2align 2
// pacdb-sign a canonical pointer, flip a PAC-field bit so autdb FPAC-faults, then autdb. The box's
// try_emulate_fpac_auth strips x0 to canonical (emulated auth). Exit 0 if recovered == original.
_start:
    movz x19, #0x2000, lsl #16    // P = 0x0000_0000_2000_0000 (canonical low VA; never dereferenced)
    mov  x0, x19
    movz x1, #0x5678              // modifier (fixed)
    pacdb x0, x1                  // x0 = DATA-B-signed P (guest APDB key)
    mov  x2, #1
    lsl  x2, x2, #48              // a bit inside the PAC field (under the 47-bit VA)
    eor  x0, x0, x2               // corrupt the signature -> autdb will FEAT_FPAC-fault
    autdb x0, x1                  // FPAC -> box strips x0 to canonical, skips this instruction
    cmp  x0, x19                  // recovered == original?
    b.ne fail
    mov  x0, #0
    b    exit
fail:
    mov  x0, #1
exit:
    mov  x16, #1                  // SYS_exit
    svc  #0x80
