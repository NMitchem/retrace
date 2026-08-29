// M21-stackgrow: the main thread's BELIEVED stack (what libpthread reports, 0x7fc000) is reserved
// but not mapped, so a deep recursion grows into it page-by-page instead of running off the 256 KiB
// backing into unbacked IPA and killing the recorder (M8 spec risk R3).
//
// The guard page libstd installs at DYN_STACK_TOP - 0x7fc000 must stay OUTSIDE the reservation. If
// it were inside, a stack overflow would take the stage-2 route and be SILENTLY COMMITTED by
// commit_reserved_page instead of faulting — converting the overflow into a corrupted, silently
// continuing guest. That is the one failure mode this design must never reach, so it is the first
// thing asserted.
use retrace_box::Box_;
use retrace_guest::{parse_macho, slice_arm64e, HELLO_DYN, DYLD_PATH};

const GRANULE: u64 = 0x4000;

fn dynbox() -> Box_ {
    let exe = parse_macho(&std::fs::read(HELLO_DYN).unwrap());
    let dyld = parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap()));
    Box_::load_dynamic(&exe, &dyld, &["hello_dyn".to_string()])
}

#[test]
fn the_believed_stack_window_is_reserved_and_the_guard_page_is_not() {
    let mut b = dynbox();
    let (start, end) = b.believed_stack_window();

    // Geometry, pinned against the derivation rather than restated from it.
    assert_eq!(end, b.stack_top() - b.stack_size(), "the window must end at the backed stack bottom");
    assert_eq!(start, b.stack_top() - retrace_box::LIBPTHREAD_MAIN_STACK_SIZE + GRANULE,
               "the window must start ONE GRANULE above libstd's guard page");
    assert!(start < end, "window must be non-empty: {start:#x}..{end:#x}");

    // NON-VACUITY FIRST, before anything is committed: pages inside the window must be committable,
    // or the refusals below would just mean "nothing here is reachable at all".
    assert!(b.commit_reserved_page(end - GRANULE),
        "the page immediately below the backed stack must demand-commit — this is stack growth");
    assert!(b.commit_reserved_page(start),
        "the lowest page of the believed stack must demand-commit");

    // The guard page itself, and everything below it. Committing the guard page is the failure this
    // design exists to prevent: it would turn an overflow into silent corruption.
    let guard = start - GRANULE;
    assert!(!b.commit_reserved_page(guard),
        "libstd's guard page at {guard:#x} must NOT be demand-committable — it must stay unbacked \
         free space so libstd's own PROT_NONE mmap lands there and faults at STAGE 1");
    assert!(!b.commit_reserved_page(guard - GRANULE),
        "the page below the guard at {:#x} must stay fatal — a frame that vaults the guard is the \
         documented residual wall, not something to silently back", guard - GRANULE);
}
