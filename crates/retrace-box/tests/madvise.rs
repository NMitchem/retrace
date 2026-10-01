//! M47 §3c, box level: `Box_::guest_madvise` on a static box, with its mappings made by hand. These
//! pin the paths the fixtures may not reach (Review Focus 1–3): a zero-fill over committed and
//! reserved pages together, a range past the guest's mappings, an unaligned address, an `int`'s
//! upper half, and xnu's rounding of `len`. `gitprims_e2e`'s `madv_dyn` covers the arms end to end.
use retrace_arch::{MADV_FREE_REUSABLE, MADV_ZERO};
use retrace_box::Box_;

const G: u64 = 0x4000;

fn tb() -> Box_ {
    Box_::load(&retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO).unwrap()))
}

fn args(addr: u64, len: u64, advice: u64) -> [u64; 8] { [addr, len, advice, 0, 0, 0, 0, 0] }

/// Review Focus 1. A reserved page the guest never touched is not committed by the model: it
/// commits as zero on its first touch, on both sides, so writing it would only cost memory.
#[test]
fn zero_writes_every_backed_page_and_no_reserved_one() {
    let mut b = tb();
    let base = b.guest_vm_reserve(0, 4 * G, true);
    assert!(b.commit_reserved_page(base) && b.commit_reserved_page(base + 2 * G));
    b.poke_guest(base, &[0xAB; 16]);
    let w = b.guest_madvise(args(base, 4 * G, MADV_ZERO.into())).unwrap();
    let got: Vec<(u64, usize, bool)> = w.iter().map(|r| (r.ipa, r.bytes.len(), r.bytes.iter().all(|&x| x == 0))).collect();
    assert_eq!(got, vec![(base, G as usize, true), (base + 2 * G, G as usize, true)]);
    assert!(!b.is_mapped(base + G) && !b.is_mapped(base + 3 * G), "the model commits nothing");
    assert_eq!(b.read_guest(base, 16), vec![0xAB; 16], "guest_madvise computes the writes; the caller applies them");
}

#[test]
fn a_noop_advice_writes_nothing_over_a_backed_range() {
    let mut b = tb();
    let base = b.guest_vm_map(0, 2 * G, true, false);
    assert_eq!(b.guest_madvise(args(base, 2 * G, MADV_FREE_REUSABLE.into())), Ok(vec![]));
}

/// Review Focus 2. Refused whole: the error comes before any write is computed, and names the
/// first page outside.
#[test]
fn a_range_that_runs_past_the_guests_mappings_is_refused_naming_the_page() {
    let mut b = tb();
    let base = b.guest_vm_map(0, 2 * G, true, false);
    let e = b.guest_madvise(args(base, 3 * G, MADV_ZERO.into())).unwrap_err();
    assert!(e.starts_with(&format!("M47: madvise range page {:#x} is neither backed nor reserved", base + 2 * G)), "{e}");
}

/// t0 M1(d): native madvise accepts an address off a page (it answers 0), but no corpus call is
/// unaligned, so the model refuses rather than guessing what the kernel does with the head page.
#[test]
fn an_address_off_a_16k_page_is_refused() {
    let mut b = tb();
    let base = b.guest_vm_map(0, 2 * G, true, false);
    let e = b.guest_madvise(args(base + 0x1000, G, MADV_FREE_REUSABLE.into())).unwrap_err();
    assert!(e.starts_with(&format!("M47: madvise range starts at {:#x}, not on a 16 KiB page", base + 0x1000)), "{e}");
}

/// Review Focus 3. `behav` is a C `int`; the kernel reads 32 bits of the register.
#[test]
fn an_advice_ints_upper_half_is_ignored_as_the_kernel_ignores_it() {
    let mut b = tb();
    let base = b.guest_vm_map(0, G, true, false);
    for bit in 32..64 {
        assert_eq!(b.guest_madvise(args(base, G, u64::from(MADV_FREE_REUSABLE) | 1 << bit)), Ok(vec![]), "bit {bit}");
    }
}

/// t0 M1(d): xnu rounds `len` up to the page, so a zero-fill one byte into a page zeroes all of it.
#[test]
fn a_length_short_of_a_page_zeroes_the_whole_last_page_as_xnu_rounds_it() {
    let mut b = tb();
    let base = b.guest_vm_map(0, 2 * G, true, false);
    let w = b.guest_madvise(args(base, G + 1, MADV_ZERO.into())).unwrap();
    assert_eq!(w.iter().map(|r| r.ipa).collect::<Vec<_>>(), vec![base, base + G]);
}
