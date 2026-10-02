// mach_vm_map's alignment mask on ANYWHERE placement, driving Box_ directly (pattern: carveout.rs).
// libmalloc's xzone allocator maps each 4 MiB segment with mask 0x3fffff and indexes its segment
// table by `addr >> 22`; a segment placed off a 4 MiB boundary leaves its tail granule unregistered
// and `free` there aborts (the M47 git abort). Each test first moves the bump cursor off the
// boundary with a one-page map, so a placement that ignores the mask fails it. Run under
// --test-threads=1 (one HVF VM per process).
use retrace_box::{Box_, MMAP_BASE};
use retrace_guest::{parse_macho, HELLO};

const SEG: u64 = 0x40_0000;
const MASK: u64 = SEG - 1;

fn boxed() -> Box_ {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    Box_::load(&loaded)
}

#[test]
fn a_masked_anywhere_map_from_the_bump_cursor_is_aligned() {
    let mut b = boxed();
    let page = b.guest_vm_map(0, 0x4000, true, false);
    assert_eq!(page, MMAP_BASE, "the first ANYWHERE map bumps from MMAP_BASE");
    let seg = b.guest_vm_map_masked(0, SEG, MASK, true, false);
    assert_eq!(seg & MASK, 0, "mask 0x3fffff must give a 4 MiB-aligned address, got {seg:#x}");
    assert!(seg >= page + 0x4000, "it must not overlap the page before it");
    let next = b.guest_vm_map(0, 0x4000, true, false);
    assert!(next >= seg + SEG, "the cursor moves past the aligned segment, got {next:#x}");
}

#[test]
fn a_masked_anywhere_reservation_is_aligned() {
    let mut b = boxed();
    let _ = b.guest_vm_map(0, 0x4000, true, false);
    let r = b.guest_vm_reserve_masked(0, SEG, MASK, true);
    assert_eq!(r & MASK, 0, "a reservation honours the mask too, got {r:#x}");
    assert_eq!(b.reservations().last(), Some(&(r, SEG)));
}

#[test]
fn a_masked_hinted_map_is_aligned_and_clears_the_occupied_hint() {
    let mut b = boxed();
    let page = b.guest_vm_map(0, 0x4000, true, false);
    let _ = b.guest_vm_map(0, 0x4000, true, false);
    // Hint at a page that is taken: first-fit must search forward to an ALIGNED free range.
    let seg = b.guest_vm_map_masked(page, SEG, MASK, true, false);
    assert_eq!(seg & MASK, 0, "the hinted first-fit path honours the mask, got {seg:#x}");
    assert!(seg >= page + 0x8000, "and lands clear of both pages, got {seg:#x}");
}

#[test]
fn mask_zero_keeps_the_old_contiguous_placement() {
    let mut b = boxed();
    let a = b.guest_vm_map(0, 0x4000, true, false);
    let c = b.guest_vm_map_masked(0, 0x4000, 0, true, false);
    assert_eq!(c, a + 0x4000, "an unmasked map is still the next page");
    let r = b.guest_vm_reserve_masked(0, 0x4000, 0, true);
    assert_eq!(r, c + 0x4000, "an unmasked reservation is still the next page");
}
