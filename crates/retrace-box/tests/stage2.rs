//! M49 (spec §3d): `Box_::resolve_stage2`, the one place a stage-2 stop is resolved. `wildstore.s`
//! stores to 0xB_0000_0000 (44 GiB), which stage 1 identity-maps and nothing backs: a stage-2
//! translation fault at EL0, straight to EL2. It also covers the stage-1 map it resolves against
//! (F3). Run under --test-threads=1 (one HVF VM per process).
use retrace_box::{Box_, Stage2, Stop, PT_L3_BASE, TRAMPOLINE_IPA, TSD_IPA};
use retrace_guest::{parse_macho, HELLO, WILDSTORE};

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

/// M49 final review M1: a store through a top-byte-tagged pointer to the same unbacked address is
/// not converted. `wildstore.s`'s `mov w1, #0x99` is patched to `movk x0, #0x5a00, lsl #48`, so
/// the `strb` goes through 0x5a00_000B_0000_0000; TBI0 lets stage 1 ignore the tag, and stage 2
/// misses at 0xB_0000_0000 as before. The pagers see the tagged FAR, so F2 must leave the stop the
/// loud recorder error it was before M49 rather than a SIGSEGV.
#[test]
fn a_stage2_abort_through_a_tagged_pointer_stays_loud() {
    const TAGGED: u64 = 0x5a00_0000_0000_0000 | WILD;
    let mut b = boxed(WILDSTORE);
    let entry = b.pc();
    b.dbg_write_va(entry + 4, &0xf2eb_4000u32.to_le_bytes()).unwrap(); // movk x0, #0x5a00, lsl #48
    let Stop::Other { esr } = b.run() else { panic!("the tagged wild store must be a stage-2 abort") };
    match b.resolve_stage2(esr) {
        Stage2::Unhandled(why) => assert!(why.contains(&format!("FAR {TAGGED:#x} carries a top-byte tag")),
            "the refusal names the tagged FAR: {why}"),
        other => panic!("a tagged FAR must stay loud, got {other:?}"),
    }
}

/// Task 3 (F3, R3): stage 1 leaves page 0, the L2 and L1 table pages and the L3 window unmapped,
/// and keeps everything else in block 0 (the TSD, the stack's neighbourhood) as it was.
#[test]
fn stage_one_leaves_page_zero_and_retraces_tables_unmapped() {
    let b = boxed(HELLO);
    for ipa in [0, 0x8000, 0xC000, PT_L3_BASE, 0x1ff_c000] {
        assert!(!b.ipa_is_el0_writable(ipa), "{ipa:#x} must have no stage-1 leaf");
    }
    assert_eq!(b.va_to_ipa(0x10), None, "a NULL-page VA does not translate");
    for ipa in [TSD_IPA, 0x10000, 0x200_0000] {
        assert!(b.ipa_is_el0_writable(ipa), "{ipa:#x} keeps its stage-1 data mapping");
    }
}

/// Task 3 (F3): an ANYWHERE placement with a hint never lands where stage 1 maps nothing. Before
/// F3, `first_fit` honoured a hint of 0x10 at IPA 0, and a hint inside the L3 window at that hint,
/// where a later `alloc_l3` would map a page table over the guest's data.
#[test]
fn a_hinted_placement_never_lands_on_page_zero_or_in_the_l3_window() {
    let mut b = boxed(HELLO);
    let low = b.guest_vm_map(0x10, 0x4000, true, false);
    assert!(low >= TRAMPOLINE_IPA, "a hint of 0x10 placed at {low:#x}, on page 0");
    assert!(!(0x8000..0x10000).contains(&low), "{low:#x} is a page-table page");
    let mid = b.guest_vm_map(PT_L3_BASE + 0x10_0000, 0x4000, true, false);
    // 0x200_0000 is PT_L3_CEIL (not exported), the L3 window's end.
    assert!(!(PT_L3_BASE..0x200_0000).contains(&mid), "a hint inside the L3 window placed at {mid:#x}");
}

/// Task 3 (F3): nothing may give a stage-1-unmapped page an attribute again.
#[test]
#[should_panic(expected = "M49 F3")]
fn set_region_exec_refuses_a_page_stage_one_leaves_unmapped() {
    let mut b = boxed(HELLO);
    b.set_region_exec(0, 0x4000);
}
