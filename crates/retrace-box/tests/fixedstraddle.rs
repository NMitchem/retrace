// M8-stack. FIXED overlap classification, on BOTH FIXED paths: the BSD one (`guest_mmap` ->
// `map_mmap_region`) and the Mach one (`guest_vm_map`, which dyld/libmalloc drive with
// VM_FLAGS_OVERWRITE). Both delegate to the same `place_fixed` helper, so these tests pin the shared
// contract from each entry point.
//
// A request CONTAINED in a live backing must reuse that backing in place, leaving the surrounding
// bytes mapped and intact. A TRUE PARTIAL STRADDLE -- overlapping a backing but neither contained in
// it nor covering it -- must fail loud: splitting a backing is unimplemented, and dropping it
// wholesale would destroy live guest memory the real kernel keeps. That destruction is invisible to
// the determinism oracle, since record and replay would destroy the same bytes and no divergence
// would fire, so retrace would record a WRONG execution faithfully.
use retrace_box::Box_;
use retrace_guest::{parse_macho, HELLO};
use retrace_trace::Region;

fn boxed() -> Box_ {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    Box_::load(&loaded)
}

// ---- BSD path (mmap) ----

#[test]
#[should_panic(expected = "partially straddles a live backing")]
fn partial_straddle_map_fixed_fails_loud() {
    let mut b = boxed();
    // A 4-page anon region at the bump allocator's next slot.
    let base = b.guest_mmap(0, 0x10000, 3, 0x1002);
    // FIXED over its last page AND one page past its end: overlaps, but is neither contained in the
    // region nor covering it.
    let _ = b.guest_mmap(base + 0xC000, 0x8000, 3, 0x1012);
}

// ---- Mach path (mach_vm_map / VM_FLAGS_OVERWRITE) ----

#[test]
#[should_panic(expected = "partially straddles a live backing")]
fn partial_straddle_vm_map_overwrite_fails_loud() {
    let mut b = boxed();
    let base = b.guest_vm_map(0, 0x10000, true, false);          // ANYWHERE: a 4-page region
    let _ = b.guest_vm_map(base + 0xC000, 0x8000, false, false); // FIXED, straddling its end
}

#[test]
fn contained_vm_map_overwrite_reuses_the_backing_and_keeps_the_surrounding_bytes() {
    let mut b = boxed();
    let base = b.guest_vm_map(0, 0x10000, true, false);          // ANYWHERE: a 4-page region
    let mapped_before = b.mapped_len();

    // Sentinels OUTSIDE the range about to be overwritten but INSIDE the same backing: page 0 and
    // page 3. They survive only if the backing is reused rather than dropped.
    b.apply_and_return(0, false, &[
        Region { ipa: base,          bytes: vec![0x11] },
        Region { ipa: base + 0x4000, bytes: vec![0x22] },        // inside the overwritten page
        Region { ipa: base + 0xC000, bytes: vec![0x44] },
    ]);

    // OVERWRITE page 1, wholly inside the region.
    let ret = b.guest_vm_map(base + 0x4000, 0x4000, false, false);
    assert_eq!(ret, base + 0x4000, "a contained FIXED vm_map must return the requested address");

    // The enclosing backing is reused: none added, none dropped.
    assert_eq!(b.mapped_len(), mapped_before,
        "containment must reuse the existing backing, not add or drop one");

    // The surrounding pages are still mapped AND still hold their contents.
    assert_eq!(b.read_guest(base, 1), vec![0x11],
        "the sentinel below the overwrite was destroyed -- the enclosing backing was dropped \
         wholesale instead of reused in place");
    assert_eq!(b.read_guest(base + 0xC000, 1), vec![0x44],
        "the sentinel above the overwrite was destroyed");
    // ...and the overwritten range came back as fresh zero pages, like a real anonymous mapping.
    assert_eq!(b.read_guest(base + 0x4000, 1), vec![0x00],
        "the overwritten range must be zeroed like a fresh anonymous mapping");
}

// A FIXED vm_map that fully covers the backing keeps the original drop-and-replace behaviour.
#[test]
fn covering_vm_map_overwrite_still_replaces_the_backing() {
    let mut b = boxed();
    let base = b.guest_vm_map(0, 0x4000, true, false);
    b.apply_and_return(0, false, &[Region { ipa: base, bytes: vec![0x55] }]);

    let ret = b.guest_vm_map(base, 0x4000, false, false);        // exactly covers it
    assert_eq!(ret, base);
    assert_eq!(b.read_guest(base, 1), vec![0x00],
        "a fully-covering FIXED map must install a fresh zeroed region");
}
