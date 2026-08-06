// THE M11 HEADLINE GATE. A real dynamically-linked, full-std Rust binary panics; libstd's panic
// path reaches abort(), which raises SIGABRT on itself via __pthread_kill(328). Before M11 that
// signal was forwarded to the host and killed the RECORDER, so the program could not be recorded
// at all — the trace ended with no terminal event and replay could only diverge on it.
mod util;

#[test]
fn a_panicking_rust_guest_records_and_replays_its_own_death() {
    let (rec, trace) = util::record_dynamic(retrace_guest::PANICKY);
    assert_eq!(rec.code, 134, "134 == 128 + SIGABRT; stderr:\n{}", rec.stderr);
    assert!(rec.stderr.contains("guest terminated by signal 6"), "stderr:\n{}", rec.stderr);

    // `starts_with`, not equality: is_console_write folds fd 1 AND fd 2 into one recorded buffer
    // (M9), so this also carries libstd's panic message — which embeds an absolute build path and
    // would make an exact-match assertion brittle across machines. What must be pinned is that the
    // guest reached its OWN code rather than dying inside dyld, and the first line proves that.
    let out = String::from_utf8_lossy(&rec.stdout);
    assert!(out.starts_with("about to panic\n"),
            "the guest must reach its OWN code, not die inside dyld; stdout:\n{out}");
    // And that the panic actually ran, rather than the guest exiting some other way.
    assert!(out.contains("panicked at") && out.contains("M11"),
            "libstd's panic must have run; stdout:\n{out}");

    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 134, "replay {i}; stderr:\n{}", rep.stderr);
        assert!(rep.stderr.contains("guest terminated by signal 6"), "replay {i}:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged");
    }
}

/// The terminal shape, asserted on the real headline guest rather than only on the asm fixture:
/// a complete (untorn) trace ending in Signal-then-final-Snapshot. A recorder killed mid-run by a
/// host signal leaves a TORN trace, so this is the direct statement of the regression under repair.
#[test]
fn the_headline_trace_is_complete_and_ends_with_signal_then_snapshot() {
    let (rec, trace) = util::record_dynamic(retrace_guest::PANICKY);
    assert_eq!(rec.code, 134, "stderr:\n{}", rec.stderr);
    let (events, torn) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(!torn, "a recorder killed mid-run leaves a TORN trace — this must be complete");
    let n = events.len();
    assert!(matches!(events[n - 2], retrace_trace::Event::Signal { sig: 6, .. }),
            "expected Event::Signal at n-2, got {:?}", events[n - 2]);
    assert!(matches!(events[n - 1], retrace_trace::Event::Snapshot { .. }),
            "expected the final memory Snapshot at n-1, got {:?}", events[n - 1]);
}
