#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ec { Svc, Hvc, SysReg, SoftStep, Breakpoint, Watchpoint, DataAbort, InstrAbort, Other(u8) }

pub const SYS_WRITE: u64 = 4;
pub const SYS_EXIT: u64 = 1;
pub const SVC_IMM: u64 = 0x80;

pub const SYS_READ: u64 = 3;
pub const SYS_PREAD: u64 = 153;
pub const SYS_OPEN: u64 = 5;
pub const SYS_CLOSE: u64 = 6;
pub const SYS_MUNMAP: u64 = 73;
pub const SYS_MPROTECT: u64 = 74;
pub const SYS_FSTAT: u64 = 189;
pub const SYS_MMAP: u64 = 197;
pub const SYS_LSEEK: u64 = 199;
pub const SYS_SHARED_REGION_CHECK_NP: u64 = 294;
pub const SYS_SHARED_REGION_MAP_AND_SLIDE_2_NP: u64 = 536;

pub const SYS_SYSCTL: u64 = 202;
pub const SYS_GETRLIMIT: u64 = 194;
/// sysctl top-level: `CTL_KERN` (`sys/sysctl.h`).
pub const CTL_KERN: u32 = 1;
/// `sys/sysctl.h:276` — "LP64 user stack query". Forwarding this hands the guest the HOST
/// process's ASLR'd stack address; retrace must answer it from the guest's own geometry.
pub const KERN_USRSTACK64: u32 = 59;

/// `sys/resource.h:446`.
pub const RLIMIT_STACK: u64 = 3;
/// `sys/resource.h:458` — libc ORs this in for strict-POSIX `getrlimit`; the guest is observed
/// passing `0x1003`, so the resource must be masked before comparison.
pub const RLIMIT_POSIX_FLAG: u64 = 0x1000;

/// BSD errno: an argument the kernel rejects (a MAP_FIXED address outside the address space).
pub const EINVAL: u64 = 22;

pub const LC_LOAD_DYLINKER: u32 = 0xe;
pub const FAT_MAGIC: u32 = 0xcafe_babe;      // big-endian on disk; read with from_be
pub const FAT_MAGIC_64: u32 = 0xcafe_babf;
pub const CPU_TYPE_ARM64: u32 = 0x0100_000c;
pub const CPU_SUBTYPE_ARM64E: u32 = 2;
pub const PSTATE_C: u64 = 1 << 29;           // carry bit in NZCV (SPSR_EL1 / CPSR)

pub fn ec_of(esr_el2: u64) -> Ec {
    match ((esr_el2 >> 26) & 0x3f) as u8 {
        0x15 => Ec::Svc,
        0x16 => Ec::Hvc,
        0x18 => Ec::SysReg,
        0x32 | 0x33 => Ec::SoftStep,
        0x30 | 0x31 => Ec::Breakpoint,
        0x34 | 0x35 => Ec::Watchpoint,
        0x24 | 0x25 => Ec::DataAbort,
        0x20 | 0x21 => Ec::InstrAbort,
        other => Ec::Other(other),
    }
}

/// If `insn` is an AArch64 pointer-authentication AUT* instruction whose authenticated result
/// lands in a destination register — the `AUTIA/AUTIB/AUTDA/AUTDB` register-modifier variants and
/// their `AUTIZA/AUTIZB/AUTDZA/AUTDZB` zero-modifier forms — return that register number (Rd).
/// Returns None otherwise. Used to emulate a B-family auth that FEAT_FPAC-faulted by stripping Rd
/// to canonical (see `Box_::try_emulate_fpac_auth`). Combined auth-and-{branch,load} forms
/// (`braab`/`ldrab`/…) have no Rd to fix and are intentionally NOT matched (they fail loud).
pub fn decode_aut_rd(insn: u32) -> Option<u32> {
    // "Data-processing (1 source)" PAC encodings: [31:10] fixed per op, [9:5] Rn, [4:0] Rd.
    match insn & 0xFFFF_FC00 {
        0xDAC1_1000 | 0xDAC1_1400 | 0xDAC1_1800 | 0xDAC1_1C00   // AUTIA/AUTIB/AUTDA/AUTDB Xd,Xn
        | 0xDAC1_3000 | 0xDAC1_3400 | 0xDAC1_3800 | 0xDAC1_3C00 // AUTIZA/AUTIZB/AUTDZA/AUTDZB Xd
            => Some(insn & 0x1F),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decode_hvc_and_svc() {
        // From the spike: ESR_EL2 = 0x5a000000 => EC = 0x16 (HVC).
        assert_eq!(ec_of(0x5a000000), Ec::Hvc);
        // EC 0x15 (SVC from AArch64) in bits [31:26].
        assert_eq!(ec_of(0x15 << 26), Ec::Svc);
        assert_eq!(ec_of(0x18 << 26), Ec::SysReg);
        assert_eq!(ec_of(0x32 << 26), Ec::SoftStep);
    }

    #[test]
    fn syscall_numbers() {
        assert_eq!((SYS_READ, SYS_WRITE, SYS_OPEN, SYS_CLOSE, SYS_EXIT), (3,4,5,6,1));
        assert_eq!((SYS_FSTAT, SYS_LSEEK, SYS_MMAP, SYS_MUNMAP, SYS_MPROTECT), (189,199,197,73,74));
        assert_eq!((SYS_SHARED_REGION_CHECK_NP, SYS_SHARED_REGION_MAP_AND_SLIDE_2_NP), (294, 536));
        assert_eq!((SYS_SYSCTL, SYS_GETRLIMIT), (202, 194));
        assert_eq!((CTL_KERN, KERN_USRSTACK64), (1, 59));
        assert_eq!((RLIMIT_STACK, RLIMIT_POSIX_FLAG), (3, 0x1000));
    }

    #[test]
    fn decodes_aut_destination_register() {
        // AUTDB x16, x17 — the observed objc fault at addClassTableEntry+0x70.
        assert_eq!(decode_aut_rd(0xDAC1_1E30), Some(16));
        // Each register variant returns Rd (bits [4:0]).
        assert_eq!(decode_aut_rd(0xDAC1_1000 | (1 << 5)), Some(0));        // AUTIA x0, x1
        assert_eq!(decode_aut_rd(0xDAC1_1400 | (2 << 5) | 3), Some(3));    // AUTIB x3, x2
        assert_eq!(decode_aut_rd(0xDAC1_1800 | (10 << 5) | 9), Some(9));   // AUTDA x9, x10
        assert_eq!(decode_aut_rd(0xDAC1_3800 | 30), Some(30));            // AUTDZA x30 (Z form)
        // Not an AUT-with-Rd: NOP, and PACIA (a SIGN, base 0xDAC1_0000) must return None.
        assert_eq!(decode_aut_rd(0xD503_201F), None);                     // NOP
        assert_eq!(decode_aut_rd(0xDAC1_0000 | (1 << 5)), None);          // PACIA x0, x1 (sign)
    }

    #[test]
    fn decodes_instruction_and_data_aborts() {
        assert_eq!(ec_of(0x20u64 << 26), Ec::InstrAbort);
        assert_eq!(ec_of(0x21u64 << 26), Ec::InstrAbort);
        assert_eq!(ec_of(0x24u64 << 26), Ec::DataAbort);
    }
}
