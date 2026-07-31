// M8-stack. A MAP_FIXED mmap that PARTIALLY straddles a live backing -- overlapping it but neither
// contained in it nor covering it -- must fail loud.
//
// The two classified cases are handled: a request covering a backing drops it, and one contained in
// a backing reuses it in place (see fixedinner_e2e). A true straddle would need the backing split in
// two, which nothing in the guest corpus exercises. Dropping it wholesale instead would destroy live
// guest memory the real kernel keeps -- silently, since record and replay would destroy the same
// bytes and the determinism oracle would see no divergence. Fail-loud beats guessing at semantics
// nothing exercises; this test pins that choice so a future split implementation is a deliberate act.
use retrace_box::Box_;
use retrace_guest::{parse_macho, HELLO};

#[test]
#[should_panic(expected = "partially straddles a live backing")]
fn partial_straddle_map_fixed_fails_loud() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);

    // A 4-page anon region at the bump allocator's next slot.
    let base = b.guest_mmap(0, 0x10000, 3, 0x1002);

    // FIXED over its last page AND one page past its end: overlaps, but is neither contained in the
    // region nor covering it.
    let _ = b.guest_mmap(base + 0xC000, 0x8000, 3, 0x1012);
}
