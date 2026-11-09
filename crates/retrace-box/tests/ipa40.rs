// M57 §3c: a page above 64 GiB, where macOS 27's malloc range group sits, survives both rebuild paths
// (`restore` and `from_checkpoint`) on a fresh 40-bit VM, through the L2 array's later pages.
// Ruling P1: at IPA_BITS = 36 the red is `guest_mmap` answering Err(22) (EINVAL from `fixed_fits`).
use retrace_box::Box_;
use retrace_guest::{parse_macho, HELLO};
use retrace_trace::Event;

const HIGH: u64 = 0x90_0000_0000;
const RW: u64 = 3;
const ANON_FIXED: u64 = 0x1012; // MAP_ANON|MAP_PRIVATE|MAP_FIXED
const PATTERN: [u8; 16] = [0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad, 0xae, 0xaf];

fn boxed_with_high_page() -> Box_ {
    let mut b = Box_::load(&parse_macho(&std::fs::read(HELLO).unwrap()));
    assert_eq!(b.guest_mmap(HIGH, 0x4000, RW, ANON_FIXED), Ok(HIGH), "a MAP_FIXED page above 64 GiB is honoured");
    let (host, len) = b.host_span_for_test(HIGH).expect("the page is backed");
    assert!(len >= PATTERN.len());
    unsafe { std::ptr::copy_nonoverlapping(PATTERN.as_ptr(), host, PATTERN.len()); }
    b
}

#[test]
fn a_page_above_64_gib_survives_snapshot_and_restore() {
    let b = boxed_with_high_page();
    let Event::Snapshot { regs, mem } = b.snapshot() else { panic!("snapshot") };
    drop(b);
    let r = Box_::restore(&mem, &regs);
    assert_eq!(r.read_guest(HIGH, 16), PATTERN);
    assert_eq!(r.va_to_ipa(HIGH), Some(HIGH));
}

#[test]
fn a_page_above_64_gib_survives_a_checkpoint_round_trip() {
    let b = boxed_with_high_page();
    let state = b.checkpoint();
    drop(b);
    let r = Box_::from_checkpoint(&state);
    assert_eq!(r.read_guest(HIGH, 16), PATTERN);
    assert_eq!(r.va_to_ipa(HIGH), Some(HIGH));
}
