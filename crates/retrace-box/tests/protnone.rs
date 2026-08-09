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
