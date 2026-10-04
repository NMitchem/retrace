//! M48 Task 3, box level: `guest_munmap` releases exactly the pages it is asked to, splitting a
//! backing the range cuts (walls.md §1 row 3: V8's aligned-reservation trim). Static box: the MMU
//! is on with an identity map (`load_with_pac` sets `sctlr_mmu_on`), so VA == IPA, and these tests
//! see stage 2, which is where a release happens.
use retrace_box::Box_;

const ANON: u64 = 0x1002; // MAP_ANON | MAP_PRIVATE
const RW: u64 = 3;

fn tb() -> Box_ {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::SPINLOOP).unwrap());
    Box_::load(&loaded)
}

#[test]
fn a_head_trim_keeps_the_rest_mapped_with_its_bytes() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x7c000, RW, ANON).unwrap();
    b.poke_guest(a + 0x20000, b"middle");
    b.guest_munmap(a, 0x10000);
    assert!(!b.is_mapped(a) && !b.is_mapped(a + 0xc000), "the head is released");
    assert!(b.is_mapped(a + 0x10000) && b.is_mapped(a + 0x7bfff), "the rest stays mapped");
    assert_eq!(b.read_bytes_for_test(a + 0x20000, 6), b"middle");
}

#[test]
fn a_tail_trim_with_an_unaligned_length_rounds_its_end_up() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x7c000, RW, ANON).unwrap();
    b.guest_munmap(a + 0x50000, 0x2bb20); // ends at a + 0x7bb20, which rounds up to a + 0x7c000
    assert!(b.is_mapped(a + 0x4ffff));
    assert!(!b.is_mapped(a + 0x50000) && !b.is_mapped(a + 0x7bfff), "the whole rounded tail is released");
}

#[test]
fn an_interior_punch_splits_one_backing_into_two() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x40000, RW, ANON).unwrap();
    b.poke_guest(a, b"head");
    b.poke_guest(a + 0x3c000, b"tail");
    let before = b.mapped_len();
    b.guest_munmap(a + 0x10000, 0x8000);
    assert_eq!(b.mapped_len(), before - 0x8000, "exactly the punched pages leave the map");
    assert!(!b.is_mapped(a + 0x10000) && !b.is_mapped(a + 0x17fff));
    assert_eq!(b.read_bytes_for_test(a, 4), b"head");
    assert_eq!(b.read_bytes_for_test(a + 0x3c000, 4), b"tail");
}

#[test]
fn a_munmap_spanning_two_backings_drops_one_and_trims_the_other() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x10000, RW, ANON).unwrap();
    let c = b.guest_mmap(0, 0x10000, RW, ANON).unwrap();
    assert_eq!(c, a + 0x10000, "non-FIXED anon mmaps pack");
    b.guest_munmap(a, 0x18000);
    assert!(!b.is_mapped(a) && !b.is_mapped(c + 0x7fff));
    assert!(b.is_mapped(c + 0x8000), "the second backing keeps its tail");
}

#[test]
fn a_split_then_full_teardown_returns_every_byte() {
    let mut b = tb();
    let base = retrace_box::live_backing_bytes();
    let a = b.guest_mmap(0, 0x7c000, RW, ANON).unwrap();
    b.guest_munmap(a, 0x10000);
    b.guest_munmap(a + 0x50000, 0x2bb20);
    b.guest_munmap(a + 0x20000, 0x8000);
    b.guest_munmap(a + 0x10000, 0x40000);
    assert_eq!(retrace_box::live_backing_bytes(), base, "every host page is released exactly once");
}

#[test]
fn a_split_backing_survives_a_checkpoint() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x7c000, RW, ANON).unwrap();
    // A backing placed after `a`'s: its Vec position shifts up when `a` splits.
    let c = b.guest_mmap(0, 0x4000, RW, ANON).unwrap();
    b.poke_guest(a + 0x20000, b"kept");
    b.poke_guest(c, b"after");
    b.guest_munmap(a, 0x10000);
    b.guest_munmap(a + 0x50000, 0x2bb20);
    assert_eq!(b.read_bytes_for_test(c, 5), b"after", "the index followed the shift");
    let st = b.checkpoint();
    drop(b); // one VM per process
    let r = Box_::from_checkpoint(&st);
    assert!(!r.is_mapped(a) && !r.is_mapped(a + 0x50000));
    assert!(r.is_mapped(a + 0x10000) && r.is_mapped(a + 0x4ffff));
    assert_eq!(r.read_bytes_for_test(a + 0x20000, 4), b"kept");
    assert_eq!(r.read_bytes_for_test(c, 5), b"after");
}
