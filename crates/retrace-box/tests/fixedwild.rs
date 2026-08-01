// M8-stack fast-follow. A MAP_FIXED request for an address the guest's IPA space cannot hold must
// be REJECTED with EINVAL -- the answer the real kernel gives -- instead of being carried down to
// `hv_vm_map`, which rejects it with HV_BAD_ARGUMENT through an `expect` and takes the RECORDER
// down (exit 101).
//
// This is not hypothetical. libstd's `install_main_guard` computes its guard page as
// `pthread_get_stackaddr_np() - pthread_get_stacksize_np()`; macOS 26's libpthread reports a
// constant 8 MiB-minus-a-page stack size for the main thread, so against the box's 256 KiB stack
// that subtraction UNDERFLOWS to 0xffffffffffa04000 and is mmapped MAP_FIXED. Task 5 taught the box
// to honor MAP_FIXED, which is correct -- but it also means a wild address now reaches hv_vm_map.
// A guest asking for the impossible must get an error back; only retrace's own invariants may
// fail loud.
//
// Determinism note: the check is a pure function of (addr, len, flags) and the fixed IPA geometry,
// so record and replay reject identically -- the symmetry is structural, no mirror needed.
use retrace_arch::EINVAL;
use retrace_box::Box_;
use retrace_guest::{parse_macho, HELLO};

fn boxed() -> Box_ {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    Box_::load(&loaded)
}

/// The exact address `hello_rust` asks for today (see the M8-stack Task 6 report).
const WILD: u64 = 0xffff_ffff_ffa0_4000;
/// One page past the top of the 36-bit guest IPA space.
const IPA_CEILING: u64 = 1 << 36;

const RW: u64 = 3; // PROT_READ|PROT_WRITE
const ANON_FIXED: u64 = 0x1012; // MAP_ANON|MAP_PRIVATE|MAP_FIXED

// ---- BSD path (mmap) ----

#[test]
fn wild_map_fixed_is_rejected_with_einval() {
    let mut b = boxed();
    assert_eq!(b.guest_mmap(WILD, 0x4000, RW, ANON_FIXED), Err(EINVAL),
        "a MAP_FIXED request far outside the guest IPA space must return EINVAL, not panic the \
         recorder inside hv_vm_map");
}

#[test]
fn map_fixed_crossing_the_ipa_ceiling_is_rejected() {
    let mut b = boxed();
    // Starts inside the guest IPA space but runs off the top of it.
    assert_eq!(b.guest_mmap(IPA_CEILING - 0x4000, 0x8000, RW, ANON_FIXED), Err(EINVAL),
        "a MAP_FIXED range that ENDS past the 36-bit IPA ceiling must be rejected -- only its base \
         being in range is not enough");
}

#[test]
fn misaligned_map_fixed_is_rejected() {
    let mut b = boxed();
    let base = b.guest_mmap(0, 0x10000, RW, 0x1002).unwrap();
    assert_eq!(b.guest_mmap(base + 0x1000, 0x4000, RW, ANON_FIXED), Err(EINVAL),
        "a MAP_FIXED address that is not page-aligned must be rejected (hv_vm_map would reject it \
         too, but as an opaque HV_BAD_ARGUMENT panic)");
}

// A rejected request must be a NO-OP: it maps nothing, frees whatever it provisionally allocated,
// and -- because the whole design rests on both runs choosing identical addresses -- must not
// disturb the placement cursor a later mmap draws from.
#[test]
fn a_rejected_map_fixed_changes_no_state() {
    let mut b = boxed();
    let first = b.guest_mmap(0, 0x4000, RW, 0x1002).unwrap();
    let mapped = b.mapped_len();

    assert_eq!(b.guest_mmap(WILD, 0x4000, RW, ANON_FIXED), Err(EINVAL));
    assert_eq!(b.mapped_len(), mapped, "a rejected mmap must not add or drop a backing");

    // The next ANYWHERE mmap must land where it would have if the rejected call had never happened
    // -- i.e. immediately after the first one, consuming no address space for the rejection.
    assert_eq!(b.guest_mmap(0, 0x4000, RW, 0x1002), Ok(first + 0x4000),
        "a rejected FIXED request moved the placement cursor -- record and replay would then \
         disagree the moment one of them took the rejection path");
}

// A valid FIXED request is unaffected: the guard must not over-reject.
#[test]
fn an_in_range_map_fixed_still_succeeds() {
    let mut b = boxed();
    let base = b.guest_mmap(0, 0x10000, RW, 0x1002).unwrap();
    assert_eq!(b.guest_mmap(base + 0x4000, 0x4000, RW, ANON_FIXED), Ok(base + 0x4000),
        "an in-range, page-aligned, contained FIXED request must still be honored");
}

// ---- Mach path (mach_vm_map / VM_FLAGS_OVERWRITE) ----

// The Mach FIXED path shares the same validation, but has no errno channel plumbed to the guest
// (its four call sites would each need a KERN_INVALID_ADDRESS reply, and no guest exercises it).
// It fails loud with a diagnosis instead of an opaque HvError -- the same posture as the partial
// straddle case next to it.
#[test]
#[should_panic(expected = "outside the guest's 36-bit IPA space")]
fn wild_fixed_vm_map_fails_loud() {
    let mut b = boxed();
    let _ = b.guest_vm_map(WILD, 0x4000, false, false);
}
