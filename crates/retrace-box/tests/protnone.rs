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

use retrace_box::Stop;
use retrace_guest::{PROTNONE, PROTRESTORE};

// A real guest, a real mprotect, a real fault. The fault must arrive as Stop::Fault — the stage-1
// route through the EL1 trampoline that M12's disposition check consults — and NOT as Stop::Other,
// which is the stage-2 route commit_reserved_page owns. That classification is the whole of M13's
// "the hardware separates them" claim, so it is asserted rather than assumed.
#[test]
fn a_protected_page_faults_on_the_stage_one_route() {
    let loaded = parse_macho(&std::fs::read(PROTNONE).unwrap());
    let mut b = Box_::load(&loaded);
    let mut protected = 0u64;
    loop {
        match b.run() {
            // The guest's mmap and mprotect are ordinary syscalls; drive them through the box the
            // way the record loop does, so this test needs no recorder.
            Stop::Syscall { num: 197, args } => {
                let ipa = b.guest_mmap(args[0], args[1], args[2], args[3]).expect("anon mmap");
                b.set_x0_err_and_return(ipa, false);
            }
            Stop::Syscall { num: 74, args } => {
                protected = args[0];
                b.guest_mprotect(args[0], args[1], args[2]);
                b.set_x0_err_and_return(0, false);
            }
            Stop::Syscall { num: 1, args } => {
                panic!("the guest exited {} — the protected store was NOT denied, which is what a \
                        missing TLBI looks like", args[0]);
            }
            // Guarded arms don't make the match exhaustive over Stop::Syscall; this guest only
            // ever issues 197/74/1, so anything else is a bug in the guest or the dispatch.
            Stop::Syscall { num, .. } => panic!("unexpected syscall {num}"),
            Stop::Fault { esr, far, .. } => {
                assert_eq!(far & !0x3fff, protected,
                    "the fault must be at the protected page {protected:#x}, got {far:#x}");
                assert_eq!(esr & 0x3f, 0x0f,
                    "DFSC must be 0x0f (permission fault, level 3), got {:#x} — a translation \
                     fault here would mean the descriptor was invalidated rather than AP-denied",
                    esr & 0x3f);
                assert_eq!(retrace_arch::signal_of_esr(esr).0, retrace_arch::SIGBUS,
                    "the protected fault must map to the signal Darwin raises (spikes/protnone.c)");
                return;
            }
            Stop::Other { esr } => panic!(
                "a protected page must fault at STAGE 1 (Stop::Fault), not stage 2 (Stop::Other, \
                 esr={esr:#x}) — stage 2 is commit_reserved_page's route and would silently \
                 materialize the page"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}

// The restore direction, and the other half of the TLBI proof: after unprotect, the guest's store
// must succeed and the value must read back. A stale restrictive entry faults here instead.
#[test]
fn an_unprotected_page_is_usable_again() {
    let loaded = parse_macho(&std::fs::read(PROTRESTORE).unwrap());
    let mut b = Box_::load(&loaded);
    loop {
        match b.run() {
            Stop::Syscall { num: 197, args } => {
                let ipa = b.guest_mmap(args[0], args[1], args[2], args[3]).expect("anon mmap");
                b.set_x0_err_and_return(ipa, false);
            }
            Stop::Syscall { num: 74, args } => {
                b.guest_mprotect(args[0], args[1], args[2]);
                b.set_x0_err_and_return(0, false);
            }
            Stop::Syscall { num: 1, args } => {
                assert_eq!(args[0], 0,
                    "exit {} — 9 means the value did not survive the protect/unprotect round trip",
                    args[0]);
                return;
            }
            Stop::Syscall { num, .. } => panic!("unexpected syscall {num}"),
            Stop::Fault { esr, far, .. } => panic!(
                "the post-restore store must NOT fault (esr={esr:#x} far={far:#x}) — a stale \
                 restrictive TLB entry is what this looks like"),
            Stop::Other { esr } => panic!("unexpected stage-2 abort esr={esr:#x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}

// libstd's install_main_guard in miniature: a PROT_NONE MAP_FIXED mmap landing WHOLLY INSIDE an
// existing backing. That is map_mmap_region's "fully contained" case, which returns early through
// place_fixed — so a hook placed only at the normal exit would miss the one path that matters.
#[test]
fn a_fixed_prot_none_mmap_inside_a_backing_protects_it() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);

    // A backing to sit inside: 4 pages, mapped RW at a fresh address.
    let region = b.guest_mmap(0, 0x10000, 3, 0x1002).expect("anon mmap");
    assert!(!b.ipa_is_noaccess(region + 0x4000), "the backing starts fully accessible");

    // The guard: one page, FIXED, PROT_NONE, strictly inside it.
    let guard = region + 0x4000;
    let got = b.guest_mmap(guard, 0x4000, 0, 0x1012).expect("fixed PROT_NONE mmap");
    assert_eq!(got, guard, "a FIXED mmap is honored at the requested address");
    assert!(b.ipa_is_noaccess(guard), "the guard page must deny EL0 — this is the contained path");
    assert_eq!(b.noaccess(), &[(guard, 0x4000)], "and be tracked");

    // Its neighbours inside the same backing are untouched.
    assert!(!b.ipa_is_noaccess(region), "the page below the guard stays accessible");
    assert!(!b.ipa_is_noaccess(region + 0x8000), "the page above it stays accessible");
}

// Unmapping a protected range must drop it from the map, or the next thing mapped at that address
// inherits a protection its guest never asked for.
#[test]
fn munmap_drops_the_protection_with_the_pages() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let region = b.guest_mmap(0, 0x10000, 3, 0x1002).expect("anon mmap");
    let guard = region + 0x4000;
    b.guest_mmap(guard, 0x4000, 0, 0x1012).expect("fixed PROT_NONE mmap");
    assert_eq!(b.noaccess(), &[(guard, 0x4000)]);

    b.guest_munmap(guard, 0x4000);
    assert!(b.noaccess().is_empty(),
        "an unmapped range must leave the protection map, or the next mapping there inherits it");
}

use retrace_guest::PROTNONE_MACH;

// mach_vm_protect is a SEPARATE dispatch arm from mprotect and had no box call at all before M13.
// One implementation now serves both, so this proves the arm is wired rather than that the
// mechanism works twice.
//
// NOTE ON WHAT THIS DOES AND DOES NOT PROVE: this test drives the arm ITSELF, so it passes with or
// without Task 9's change to retrace-core — it pins the ARG LAYOUT (addr=args[1], size=args[2],
// prot=args[4]) and nothing more. The dispatch wiring is proved one crate up, by
// retrace-core/tests/protnone_mach.rs, which records the same guest through the real record loop.
#[test]
fn mach_vm_protect_denies_access_too() {
    // _kernelrpc_mach_vm_protect_trap. A const so it can sit IN the pattern: a `if num == ...`
    // guard is what tripped clippy::redundant_guards in Task 7 (plan defect #10).
    const MACH_VM_PROTECT: u64 = (-14i64) as u64;
    let loaded = parse_macho(&std::fs::read(PROTNONE_MACH).unwrap());
    let mut b = Box_::load(&loaded);
    let mut protected = 0u64;
    loop {
        match b.run() {
            Stop::Syscall { num: 197, args } => {
                let ipa = b.guest_mmap(args[0], args[1], args[2], args[3]).expect("anon mmap");
                b.set_x0_err_and_return(ipa, false);
            }
            // addr=args[1], size=args[2], prot=args[4]. set_maximum (args[3]) is ignored — M13
            // models current protection only.
            Stop::Syscall { num: MACH_VM_PROTECT, args } => {
                protected = args[1];
                b.guest_mprotect(args[1], args[2], args[4]);
                b.set_x0_err_and_return(0, false);
            }
            Stop::Syscall { num: 1, args } =>
                panic!("the guest exited {} — mach_vm_protect did not deny access", args[0]),
            // Guarded/const arms don't make the match exhaustive over Stop::Syscall (plan defect #9).
            Stop::Syscall { num, .. } => panic!("unexpected syscall {num}"),
            Stop::Fault { esr, far, .. } => {
                assert_eq!(far & !0x3fff, protected,
                    "the fault must be at the protected page {protected:#x}, got {far:#x}");
                assert_eq!(esr & 0x3f, 0x0f, "DFSC must be 0x0f (permission fault, level 3)");
                assert_eq!(retrace_arch::signal_of_esr(esr).0, retrace_arch::SIGBUS);
                return;
            }
            Stop::Other { esr } => panic!("expected a stage-1 fault, got stage-2 esr={esr:#x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}
