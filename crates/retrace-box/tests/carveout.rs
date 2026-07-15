// M2-carveout Task 1 placement/split unit tests, driving Box_ directly (pattern: reservecommit.rs).
// Model libmalloc's guarded-metadata carveout protocol: a mach_vm_deallocate must punch holes in a
// PROT_NONE reservation (remove / trim head / trim tail / split interior), and a hinted ANYWHERE map
// must treat reservations as occupied and search forward from the hint (kernel-faithful first-fit),
// so a hint at a reservation base lands in the punched hole — never inside the reservation. Run under
// --test-threads=1 (one HVF VM per process).
use retrace_box::{Box_, MMAP_BASE};
use retrace_guest::{parse_macho, HELLO};

fn boxed() -> Box_ {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    Box_::load(&loaded)
}

// (a) A strictly-interior mach_vm_deallocate SPLITS a reservation into two exact remnants — the
// carveout case. (guest_munmap is the shared deallocate path; both the trap and MIG routes funnel
// through it on record and replay.)
#[test]
fn interior_dealloc_splits_reservation_into_two_exact_remnants() {
    let mut b = boxed();
    let base = b.guest_vm_reserve(0, 0x40000, true);
    assert_eq!(base, MMAP_BASE, "the first ANYWHERE reservation bumps from MMAP_BASE");
    assert_eq!(b.reservations(), &[(base, 0x40000)]);
    b.guest_munmap(base + 0x10000, 0x10000); // punch [base+0x10000, base+0x20000)
    assert_eq!(b.reservations(), &[(base, 0x10000), (base + 0x20000, 0x20000)],
        "an interior punch splits the reservation into two exact remnants");
}

// (a) Head trim, tail trim, and full-cover removal — the other three subtraction cases, exact bounds.
#[test]
fn dealloc_trims_head_then_tail_then_removes_on_full_cover() {
    let mut b = boxed();
    let base = b.guest_vm_reserve(0, 0x40000, true);
    b.guest_munmap(base, 0x10000); // trim head
    assert_eq!(b.reservations(), &[(base + 0x10000, 0x30000)], "head trim");
    b.guest_munmap(base + 0x30000, 0x10000); // trim tail
    assert_eq!(b.reservations(), &[(base + 0x10000, 0x20000)], "tail trim");
    b.guest_munmap(base + 0x10000, 0x20000); // full cover of the remnant
    assert_eq!(b.reservations(), &[] as &[(u64, u64)], "a full-cover dealloc removes the entry");
}

// (b) A hinted ANYWHERE map (hint = reservation base, len <= hole) returns the HOLE base and backs
// the full requested length — kernel-faithful forced placement into the carveout.
#[test]
fn hinted_anywhere_map_lands_in_the_carveout_hole() {
    let mut b = boxed();
    let base = b.guest_vm_reserve(0, 0x40000, true);
    b.guest_munmap(base + 0x10000, 0x10000); // hole [base+0x10000, base+0x20000)
    let before = b.mapped_len();
    let got = b.guest_vm_map(base, 0x10000, true, false); // ANYWHERE, hint = reservation base
    assert_eq!(got, base + 0x10000,
        "first-fit forces the map into the carveout hole, not the reserved base");
    assert_eq!(b.mapped_len(), before + 0x10000, "the full requested length is backed");
}

// (c) touch in the remaining reserved band still demand-commits; (d) a touch in the punched hole
// outside the new backing is refused (the fatal path is preserved — deallocated space is not memory).
#[test]
fn hole_outside_backing_is_fatal_reserved_remnants_still_commit() {
    let mut b = boxed();
    let base = b.guest_vm_reserve(0, 0x40000, true);
    b.guest_munmap(base + 0x10000, 0x20000); // hole [base+0x10000, base+0x30000)
    assert_eq!(b.reservations(), &[(base, 0x10000), (base + 0x30000, 0x10000)]);
    // Map only the FRONT of the hole (ANYWHERE hint=base, len 0x10000) -> lands at the hole start.
    let got = b.guest_vm_map(base, 0x10000, true, false);
    assert_eq!(got, base + 0x10000);
    // (d) the REST of the hole [base+0x20000, base+0x30000) is neither reserved nor backed -> fatal.
    assert!(!b.commit_reserved_page(base + 0x20000),
        "a hole page outside the new backing must stay fatal (not demand-committable)");
    // (c) pages in the surviving reserved remnants still demand-commit.
    assert!(b.commit_reserved_page(base), "a head-remnant page still demand-commits");
    assert!(b.commit_reserved_page(base + 0x30000), "a tail-remnant page still demand-commits");
}

// (e) Step 1 nano rule. nano reserves AND commits its band FIXED (`_nano_common_map_vm_space`:
// flags = VM_MAKE_TAG(VM_MEMORY_MALLOC_NANO), VM_FLAGS_ANYWHERE bit clear). The FIXED path never
// consults range_is_free, so making reservations "occupied" for ANYWHERE cannot relocate a nano
// commit — it still lands at its requested base. (Regression guard for the M2-mach nano wall.)
#[test]
fn nano_fixed_commit_lands_at_requested_base_inside_a_reservation() {
    let mut b = boxed();
    let base = b.guest_vm_reserve(0, 0x40000, true);          // reserve the band
    let commit = b.guest_vm_map(base + 0x10000, 0x10000, false, false); // FIXED commit inside it
    assert_eq!(commit, base + 0x10000,
        "a FIXED commit lands at its requested base inside the reservation (nano path preserved)");
}

// (e) The behavioral half of the nano rule: a hinted ANYWHERE map into an INTACT reservation is now
// pushed to/after the reservation's end (kernel-faithful) instead of being honored at the reserved
// base. RED: today range_is_free is reservation-blind so the raw hint is honored at `base`.
#[test]
fn anywhere_hint_into_intact_reservation_is_pushed_past_it() {
    let mut b = boxed();
    let base = b.guest_vm_reserve(0, 0x40000, true);
    let got = b.guest_vm_map(base, 0x10000, true, false); // ANYWHERE, hint = intact reservation base
    assert!(got >= base + 0x40000,
        "an ANYWHERE hint into an intact reservation must be pushed to/after its end (got {got:#x}, base {base:#x})");
}
