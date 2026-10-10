//! M49 (spec §3d): `Box_::resolve_stage2`, the one place a stage-2 stop is resolved. `wildstore.s`
//! stores to 0xB_0000_0000 (44 GiB), which stage 1 identity-maps and nothing backs: a stage-2
//! translation fault at EL0, straight to EL2. Run under --test-threads=1 (one HVF VM per process).
use retrace_box::{Box_, Stage2, Stop};
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

/// Task 2 (F1): nothing owns the page, so the method hands the stop back with exactly the text the
/// six call sites used to build for themselves. Task 3 replaces this test: F2 converts this store.
#[test]
fn a_wild_store_outside_every_pager_is_left_to_the_caller() {
    let mut b = boxed(WILDSTORE);
    let Stop::Other { esr } = b.run() else { panic!("the wild store must be a stage-2 abort") };
    let got = b.resolve_stage2(esr);
    assert_eq!(got, Stage2::Unhandled(b.describe_stop(esr)));
}
