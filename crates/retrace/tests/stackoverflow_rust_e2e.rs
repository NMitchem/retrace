// M21-stackgrow. This gate was PARKED at M8 spec risk R3 from M8 through M20, and M21 CLEARED that
// wall — but not the gate, because a second and different wall stood behind it. Both were measured.
// M51 (Ruling T4-ssdisable) cleared the second: `sigaltstack` answered "a stack is installed" to a
// thread with none, so libstd never built its overflow stack, and the guard fault's handler frame
// had nowhere to go. An absent stack now answers SS_DISABLE as natively, libstd builds the stack,
// the handler runs on it, and the gate is un-ignored at native's 134.
//
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
fn a_rust_stack_overflow_strikes_its_own_guard_page() {
    let (rec, trace) = util::record_dynamic(retrace_guest::OVERFLOW);
    let err = &rec.stderr; // RunOut::stderr is already a String
    assert!(err.contains("has overflowed its stack"),
        "libstd's handler must recognize the fault as a stack overflow by comparing si_addr against \
         its own guard range; stderr:\n{err}");
    assert!(err.contains("fatal runtime error: stack overflow, aborting"), "stderr:\n{err}");
    assert_eq!(rec.code, 134, "134 == 128 + SIGABRT: libstd aborts after printing");
    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 134, "replay {i}");
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged");
        assert_eq!(rep.stderr, rec.stderr, "replay {i} stderr diverged");
    }
}

/// The difference M21 made, kept as its own gate so the reservation cannot regress in silence under
/// a change to the signal path: the M8 R3 signature is GONE (no `(UNMAPPED)` stage-2 abort below the
/// stack bottom), and the recursion walks the whole reservation to the guard page and reaches
/// libstd's handler, which prints its message.
#[test]
fn a_rust_stack_overflow_grows_through_the_reservation_to_its_guard_page() {
    let (rec, _trace) = util::record_dynamic(retrace_guest::OVERFLOW);
    let err = &rec.stderr;
    assert!(!err.contains("(UNMAPPED)"),
        "M8 R3 regressed: the recursion ran off the backed stack into unbacked IPA again instead of \
         growing into the reservation; stderr:\n{err}");
    assert!(err.contains("has overflowed its stack"),
        "the guard-page fault must reach libstd's handler; stderr:\n{err}");
}
