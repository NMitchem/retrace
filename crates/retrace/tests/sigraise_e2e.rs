// M11 mechanism gate: a guest that raises SIGABRT on itself records and replays bit-for-bit,
// and the RECORDER SURVIVES. Before M11 the raise was forwarded and killed the recorder (exit 134),
// leaving a trace with no terminal event that replay could only diverge on.
mod util;

#[test]
fn a_self_raised_sigabrt_records_and_replays_with_the_recorder_intact() {
    let (rec, trace) = util::record(retrace_guest::RAISE);
    // 134 == 128 + SIGABRT(6), the convention M6 established (139 == 128 + SIGSEGV).
    assert_eq!(rec.code, 134, "the GUEST died by signal 6; stderr: {}", rec.stderr);
    assert!(rec.stderr.contains("guest terminated by signal 6"), "stderr: {}", rec.stderr);

    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 134, "replay {i}; stderr: {}", rep.stderr);
        assert!(rep.stderr.contains("guest terminated by signal 6"), "replay {i}: {}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged");
    }
}

/// THE regression this milestone exists to fix, asserted directly rather than inferred.
/// Before M11 the recorder itself took the SIGABRT; a complete trace was never written at all.
#[test]
fn the_trace_ends_with_signal_then_the_final_snapshot() {
    let (rec, trace) = util::record(retrace_guest::RAISE);
    assert_eq!(rec.code, 134, "stderr: {}", rec.stderr);
    let (events, torn) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(!torn, "a recorder killed mid-run leaves a TORN trace — this must be complete");
    let n = events.len();
    assert!(matches!(events[n - 2], retrace_trace::Event::Signal { sig: 6, .. }),
            "expected Event::Signal at n-2, got {:?}", events[n - 2]);
    assert!(matches!(events[n - 1], retrace_trace::Event::Snapshot { .. }),
            "expected the final memory Snapshot at n-1, got {:?}", events[n - 1]);
}

// NO `util::assert_trace_reproducible` gate here, and the reason is structural rather than a
// concession. That helper compares two recordings made by two SEPARATE recorder processes, and it
// cannot apply to this guest for two independent reasons:
//
//   1. It requires `code == 0`. This guest exits 134 BY DESIGN — the terminal signal is the point.
//   2. The guest calls getpid(20), which M11 deliberately does not intercept (the raise arm's
//      self-pid check depends on it forwarding), so the recorder's own pid is recorded as that
//      syscall's return. Two processes have two pids. Measured: the traces differ in exactly one
//      record — the CRC and body of the num=20 event — and in nothing else.
//
// The second oracle is not lost, it is applied where it is meaningful: retrace-core/tests/signals.rs
// records this guest twice IN ONE PROCESS, which holds the pid constant and asks the real question.
// Weakening the helper to tolerate a varying pid would blunt an oracle the whole project leans on.
