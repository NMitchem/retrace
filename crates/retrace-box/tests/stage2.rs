//! M49 (spec §3d): `Box_::resolve_stage2`, the one place a stage-2 stop is resolved. `wildstore.s`
//! stores to 0xB_0000_0000 (44 GiB), which stage 1 identity-maps and nothing backs: a stage-2
//! translation fault at EL0, straight to EL2. Run under --test-threads=1 (one HVF VM per process).
use retrace_box::{Box_, Stage2, Stop, TRAMPOLINE_IPA};
use retrace_guest::{parse_macho, WILDSTORE};

/// The address `asm/wildstore.s` stores 0x99 to.
const WILD: u64 = 0xB_0000_0000;

fn boxed(guest: &str) -> Box_ { Box_::load(&parse_macho(&std::fs::read(guest).unwrap())) }

/// Task 2 (F1): a page a pager owns resolves, and the caller re-runs the instruction.
#[test]
fn a_touch_inside_a_reservation_resolves_as_a_commit() {
    let mut b = boxed(WILDSTORE);
    b.guest_vm_reserve(WILD, 0x4000, false);
    let Stop::Other { esr } = b.run() else { panic!("the wild store must be a stage-2 abort") };
    assert_eq!(b.resolve_stage2(esr), Stage2::Resolved { committed: true });
    match b.run() {
        Stop::Syscall { num, args } => assert_eq!((num, args[0]), (retrace_arch::SYS_EXIT, 0),
            "the store retired and the guest reached its exit(0)"),
        other => panic!("expected exit(0) after the commit, got {other:?}"),
    }
    assert_eq!(b.read_bytes_for_test(WILD, 1), vec![0x99], "the re-run store landed in the committed page");
}

/// Task 3 (F2): an EL0 store to an address nothing backs becomes the stage-1 fault native takes.
/// `resolve_stage2` parks the guest at its EL1 vector with ELR, SPSR, FAR and ESR_EL1 written, and
/// the next `run()` returns that fault through the arm a stage-1 fault takes. Measured at plan
/// time: HVF reported ESR_EL2 0x93010046 (ISV, SRT 1, WnR, DFSC 0x06) at pc = entry + 8, the `strb`.
#[test]
fn a_wild_el0_store_converts_to_the_stage1_fault_native_takes() {
    let mut b = boxed(WILDSTORE);
    let store = b.pc() + 8; // movz, mov, strb
    let Stop::Other { esr } = b.run() else { panic!("the wild store must be a stage-2 abort") };
    assert_eq!(b.resolve_stage2(esr), Stage2::Converted { pc: store, esr: 0x9200_0047, far: WILD });
    assert_eq!(b.pc(), TRAMPOLINE_IPA + 0x400, "parked at the lower-EL synchronous vector");
    assert_eq!(b.position(), store, "ELR_EL1 is the faulting store, not the stale syscall return");
    assert_eq!(b.spsr(), 0, "SPSR_EL1 is the guest's EL0t PSTATE");
    match b.run() {
        Stop::Fault { pc, esr, far } => assert_eq!((pc, esr, far), (store, 0x9200_0047, WILD)),
        other => panic!("the vector must deliver the converted fault, got {other:?}"),
    }
}
