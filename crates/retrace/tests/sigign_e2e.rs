// M11: the branch the terminal gate cannot reach. Without this, a bug that made EVERY raise
// terminal would pass the entire suite — and that is not hypothetical: disabling the replay-side
// sigaction mirror makes exactly this guest diverge ("expected recorded Signal, got Syscall{37}"),
// because replay's table would read Dfl for a signal the guest had set to SIG_IGN.
mod util;

#[test]
fn an_ignored_sigabrt_lets_the_guest_run_to_a_clean_exit() {
    let (rec, trace) = util::record(retrace_guest::SIGIGN);
    assert_eq!(rec.code, 0, "SIG_IGN must not terminate the guest; stderr: {}", rec.stderr);
    assert_eq!(rec.stdout, b"ok\n", "the guest ran PAST the raise and produced output");

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "stderr: {}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout);
}

// No `util::assert_trace_reproducible` here — this guest also calls getpid(20), which M11 leaves
// forwarding on purpose, so its trace embeds the recorder's pid and two SEPARATE recorder processes
// cannot produce byte-identical traces. The in-process twin in retrace-core/tests/signals.rs
// (two_recordings_of_the_sigign_guest_are_byte_identical) holds the pid constant and carries this
// coverage. See the longer note in sigraise_e2e.rs.
