// M21-stackgrow. This gate was PARKED at M8 spec risk R3 from M8 through M20, and M21 CLEARED that
// wall — but not the gate, because a second and different wall stands behind it. Both are measured.
//
// M8 R3, now cleared: libstd computes its stack-overflow guard page at pthread_get_stackaddr_np() -
// pthread_get_stacksize_np(); macOS 26's libpthread reports a CONSTANT 0x7fc000 that retrace cannot
// influence (M8 measured that a different getrlimit(RLIMIT_STACK) answer left the computed address
// bit-identical); and retrace backed only 256 KiB. So the guard landed at 0x2004000, 7.73 MiB BELOW
// the real stack bottom 0x27C0000, and this recursion ran off the stack into unbacked IPA and took a
// fatal STAGE-2 fault instead of striking the guard — 'far/ipa=0x27bff60 (UNMAPPED)', FSC=0x7, a
// TRANSLATION fault 160 bytes below the stack bottom.
//
// M21 reserves [0x2008000, 0x27C0000) — the stack the guest BELIEVES it has — so
// commit_reserved_page grows into it one zeroed page per stage-2 fault and the recursion walks all
// 7.72 MiB down to the guard. The guard page itself is deliberately left OUTSIDE the reservation, so
// it stays a backed PROT_NONE page that faults at STAGE 1 and reaches libstd's handler as a signal
// (M13's route). Measured on 2026-08-30, and the numbers are the whole argument:
//
//     [fault] pc=0x100000a70 esr=0x9200004f far=0x2007f30 ec=0x24
//     [fault] pc=0x1804fb710 esr=0x9200004f far=0x2007a90 ec=0x24
//
// far 0x2007f30 and 0x2007a90 are both INSIDE the guard page [0x2004000, 0x2008000) — 208 and 1392
// bytes below GUARD_TOP — and esr 0x9200004f decodes to DFSC 0x0f, a PERMISSION fault, not the
// 0x04..=0x07 translation fault of the before-picture. That distinction is the same one
// protnone_rust_e2e was built to assert: permission means the page is there and the guest may not
// touch it, which is what a guard page IS. The recursion now strikes its own guard.

mod util;

#[test]
#[ignore = "M8 risk R3 is CLEARED (see the measurement above: the recursion reaches its guard page \
            and faults at stage 1 with DFSC 0x0f). This gate is RE-PARKED one wall further on, at a \
            capability M21 does not have and did not set out to build. libstd HAS a handler \
            installed for the signal the guard fault maps to — signal 10, SIGBUS — so the \
            disposition check passes, but the faulting thread has that signal BLOCKED, and \
            retrace-core/src/lib.rs:203 asserts rather than guessing: 'raising blocked signal 10 \
            synchronously is not modelled: a fault cannot be deferred, POSIX leaves it undefined, \
            and Darwin force-delivers. M11 models no pending set, so implement one — and revisit \
            sigpending's always-empty answer — before a guest needs this.' A guest now needs it. \
            Clearing this means giving M11 a pending set for synchronously-raised blocked signals \
            and revisiting sigpending, which is a signal-model milestone and not a stack one. \
            UN-IGNORE when that lands. The progress M21 did make is not left ungated — see \
            a_rust_stack_overflow_now_reaches_its_guard_page_and_a_different_wall below, which runs."]
fn a_rust_stack_overflow_strikes_its_own_guard_page() {
    let (rec, trace) = util::record_dynamic(retrace_guest::OVERFLOW);
    let err = &rec.stderr; // RunOut::stderr is already a String
    assert!(err.contains("has overflowed its stack"),
        "libstd's handler must recognize the fault as a stack overflow by comparing si_addr against \
         its own guard range; stderr:\n{err}");
    assert_eq!(rec.code, 134, "134 == 128 + SIGABRT: libstd aborts after printing");
    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 134, "replay {i}");
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged");
    }
}

/// The difference M21 actually made, gated so it cannot regress in silence.
///
/// The headline gate above stays parked at a signal-model wall, which means nothing end-to-end would
/// otherwise notice if the reservation stopped working: `stackgrow.rs` and `restorereserve.rs` prove
/// the reservation EXISTS on both sides, and neither proves a real deep recursion USES it. This does.
///
/// It asserts on the two halves of the move, in the `protnone_rust_e2e` spirit of asserting on the
/// difference rather than on an outcome a weaker failure would also produce. First, the M8 R3
/// signature is GONE: no `(UNMAPPED)` stage-2 abort below the stack bottom. Second, the run now gets
/// all the way to signal delivery for the guard fault, which it could only reach by growing 7.72 MiB
/// through the reservation and faulting at stage 1 on the guard page.
///
/// Pinning the current wall's own text is deliberate, not brittle-by-accident: honest-gate discipline
/// makes the wall message the primary record, so when that wall moves this test MUST be revisited.
/// That is the point of it.
#[test]
fn a_rust_stack_overflow_now_reaches_its_guard_page_and_a_different_wall() {
    let (rec, _trace) = util::record_dynamic(retrace_guest::OVERFLOW);
    let err = &rec.stderr;
    assert!(!err.contains("(UNMAPPED)"),
        "M8 R3 regressed: the recursion ran off the backed stack into unbacked IPA again instead of \
         growing into the reservation; stderr:\n{err}");
    assert!(err.contains("blocked signal 10"),
        "the run must reach signal delivery for the guard-page fault — which it can only do by \
         growing through the reservation and faulting at STAGE 1 on the guard — and stop at the \
         signal-model wall named in the #[ignore] above; stderr:\n{err}");
}
