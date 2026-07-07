#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ec { Svc, Hvc, SysReg, SoftStep, Breakpoint, Watchpoint, DataAbort, Other(u8) }

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
        other => Ec::Other(other),
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
    }
}
