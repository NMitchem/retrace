// M13-protnone. The no-access protection mechanism: the range table's arithmetic, the stage-1
// stamp, the fault it produces, and the restore. Run under --test-threads=1 (one HVF VM per
// process).
use retrace_box::subtract_range_for_test as subtract_range;

// The four cases carveout.rs already pins for `reservations`, now exercised through the shared
// helper so `noaccess` cannot grow a second, subtly-different copy of them.
#[test]
fn subtract_range_trims_splits_and_removes() {
    // Disjoint: untouched.
    let mut t = vec![(0x1000_0000, 0x1_0000)];
    subtract_range(&mut t, 0x2000_0000, 0x4000);
    assert_eq!(t, vec![(0x1000_0000, 0x1_0000)], "a disjoint cut leaves the entry whole");

    // Head trim: the cut covers the low end.
    let mut t = vec![(0x1000_0000, 0x1_0000)];
    subtract_range(&mut t, 0x1000_0000, 0x4000);
    assert_eq!(t, vec![(0x1000_4000, 0xc000)], "a head cut moves the start up");

    // Tail trim: the cut covers the high end.
    let mut t = vec![(0x1000_0000, 0x1_0000)];
    subtract_range(&mut t, 0x1000_c000, 0x4000);
    assert_eq!(t, vec![(0x1000_0000, 0xc000)], "a tail cut shortens the entry");

    // Interior punch: SPLITS into two entries.
    let mut t = vec![(0x1000_0000, 0x1_0000)];
    subtract_range(&mut t, 0x1000_4000, 0x4000);
    assert_eq!(t, vec![(0x1000_0000, 0x4000), (0x1000_8000, 0x8000)],
        "an interior cut splits the entry in two");

    // Full cover: the entry is removed.
    let mut t = vec![(0x1000_0000, 0x1_0000)];
    subtract_range(&mut t, 0x0fff_0000, 0x10_0000);
    assert!(t.is_empty(), "a covering cut removes the entry");

    // The kernel rounds the cut OUT to whole pages: start down, end up. A sub-page cut in the
    // middle of a page still removes that whole page.
    let mut t = vec![(0x1000_0000, 0x1_0000)];
    subtract_range(&mut t, 0x1000_4001, 1);
    assert_eq!(t, vec![(0x1000_0000, 0x4000), (0x1000_8000, 0x8000)],
        "a sub-page cut is rounded out to whole pages");
}

use retrace_box::Box_;
use retrace_guest::{parse_macho, HELLO};

// The stamp round-trips: a backed page goes no-access and comes back, and both the live page-table
// leaf and the tracked map agree at every step. This is the mechanism with no guest and no fault
// in the way.
#[test]
fn protect_none_stamps_the_leaf_and_tracks_the_range() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);

    // A page that is genuinely backed: reserve, then commit one page (the M2-mmapcommit path).
    let base = b.guest_vm_reserve(0, 0x10000, true);
    assert!(b.commit_reserved_page(base), "the page under test must be backed");

    assert!(!b.ipa_is_noaccess(base), "a freshly committed page is ordinary RW data");
    assert!(b.noaccess().is_empty(), "nothing is protected yet");

    b.protect_none(base, 0x4000);
    assert!(b.ipa_is_noaccess(base), "the leaf must deny EL0 after protect_none");
    assert_eq!(b.noaccess(), &[(base, 0x4000)], "the extent must be tracked");

    // Its neighbour inside the same reservation is untouched: the stamp is per-page.
    assert!(!b.ipa_is_noaccess(base + 0x4000), "protection must not leak to the next page");

    b.unprotect(base, 0x4000);
    assert!(!b.ipa_is_noaccess(base), "unprotect must restore EL0 access");
    assert!(b.noaccess().is_empty(), "the extent must be dropped from the map");
}

// A seeked or checkpointed session must agree with the run it came from about what is protected.
// The page-table STAMP rides along for free (the tables are backings, captured in `mem`); the MAP
// does not, and without it `unprotect` and the fail-loud asserts would disagree with the hardware.
#[test]
fn a_checkpoint_carries_both_the_stamp_and_the_map() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let base = b.guest_vm_reserve(0, 0x10000, true);
    assert!(b.commit_reserved_page(base));
    b.protect_none(base, 0x4000);

    let st = b.checkpoint();
    assert_eq!(st.noaccess, vec![(base, 0x4000)], "the map must be captured");
    drop(b); // one VM per process: the original must go before the restored one is built

    let b2 = Box_::from_checkpoint(&st);
    assert!(b2.ipa_is_noaccess(base),
        "the stage-1 stamp rides in `mem` with the page tables and must survive the restore");
    assert_eq!(b2.noaccess(), &[(base, 0x4000)],
        "the map must survive too, or unprotect and the hardware disagree");
}

// The M13-split invariant: no-access implies backed. Protecting a page with no backing would leave
// its fault at stage 2, where commit_reserved_page would silently materialize it — the exact
// silent-wrong-answer this milestone exists to remove. It must fail loud instead.
#[test]
#[should_panic(expected = "protect_none: no backing")]
fn protect_none_refuses_an_unbacked_page() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let base = b.guest_vm_reserve(0, 0x10000, true);  // reserved, deliberately NOT committed
    b.protect_none(base, 0x4000);
}
