// M2-mmapcommit Task 1 fail-loud negative + gating tests. commit_reserved_page must demand-commit
// pages ONLY inside a tracked reservation; a wild pointer (no reservation, no backing) must stay
// fatal — the committer must never materialize untracked memory. Run under --test-threads=1
// (one HVF VM per process).
use retrace_box::{Box_, Stop, MMAP_BASE};
use retrace_guest::{parse_macho, HELLO, WILDSTORE};

// The wild address wildstore.s stores to (0xB_0000_0000 = 44 GiB): inside the 36-bit IPA space so
// stage-1 identity resolves it, but backed by nothing and inside no reservation.
const WILD: u64 = 0xB_0000_0000;

// A real guest store to a wild, unreserved, unbacked address must surface as Stop::Other and be
// REFUSED by commit_reserved_page — proving the fatal data-abort path is preserved (no wild
// materialization). This mirrors what the record/replay dispatch does: page_in_cache=false,
// commit_reserved_page=false => describe_stop (fatal).
#[test]
fn wild_store_outside_any_reservation_stays_fatal() {
    let loaded = parse_macho(&std::fs::read(WILDSTORE).unwrap());
    let mut b = Box_::load(&loaded);
    match b.run() {
        Stop::Other { .. } => {
            let ipa = b.fault_ipa();
            assert_eq!(ipa & !0x3fff, WILD, "the fault must be the guest's wild store at {WILD:#x}, got {ipa:#x}");
            assert!(!b.commit_reserved_page(ipa),
                "committer must REFUSE a wild pointer at {ipa:#x} (no reservation, no backing)");
        }
        Stop::Syscall { num, args } =>
            panic!("expected an immediate wild-store data abort, got syscall num={num} args={args:?}"),
        // The wild store is stage-1-mapped (identity) but stage-2-unbacked => an OUTER abort
        // (Stop::Other), NEVER the INNER stage-1 crash arm — this guards that classification.
        Stop::Fault { pc, esr, far } =>
            panic!("wild store must stay a fatal stage-2 abort (Stop::Other), not Stop::Fault: pc={pc:#x} esr={esr:#x} far={far:#x}"),
        Stop::Step => unreachable!("run() does not single-step"),
    }
}

// Direct gating of commit_reserved_page: nothing outside a tracked reservation is committable, a
// first touch inside commits, a re-touch of a committed page refuses to double-map, a different
// page in the same reservation commits independently (per-page granularity), and a page past the
// reservation end stays uncommittable.
#[test]
fn commit_reserved_page_gates_strictly_to_tracked_reservations() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    // No reservation yet: even an MMAP_BASE-range page is not committable.
    assert!(!b.commit_reserved_page(MMAP_BASE + 0x8000),
        "with no reservation recorded, no page may be committed");
    // Reserve [MMAP_BASE, MMAP_BASE+0x100000) via the ANYWHERE reserve path.
    let base = b.guest_vm_reserve(0, 0x100000, true);
    assert_eq!(base, MMAP_BASE, "the first ANYWHERE reservation bumps from MMAP_BASE");
    // A page well inside commits once (true), then refuses to double-map (false).
    assert!(b.commit_reserved_page(base + 0x40000), "first touch inside the reservation must commit");
    assert!(!b.commit_reserved_page(base + 0x40000), "an already-committed page must not double-map");
    // A DIFFERENT page inside commits independently (per-page, not per-reservation, granularity).
    assert!(b.commit_reserved_page(base + 0x8000), "a different page in the same reservation commits separately");
    // A page at/after the reservation end is a wild pointer -> stays fatal.
    assert!(!b.commit_reserved_page(base + 0x100000), "a page outside the reservation must not be materialized");
}
