use retrace_box::{Box_, Stop};
use retrace_arch::SYS_EXIT;
use retrace_guest::{parse_macho, UNALIGNED};

// The unaligned guest does a 64-bit store to an odd address, reads it back, and exits with
// x0 = (readback == written). MMU off, that store faults (Device memory); MMU on with Normal
// memory it succeeds. So a clean exit code 0 proves the MMU is on with Normal attributes.
#[test]
fn unaligned_store_runs_under_mmu_on() {
    let loaded = parse_macho(&std::fs::read(UNALIGNED).unwrap());
    let mut b = Box_::load(&loaded);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == SYS_EXIT => { assert_eq!(args[0], 0, "unaligned readback mismatch => MMU/Normal-memory wrong"); break; }
            Stop::Syscall { .. } => panic!("unexpected syscall before exit"),
            Stop::Other { esr } => panic!("guest faulted esr=0x{esr:x} (MMU likely misconfigured)"),
        }
    }
}
