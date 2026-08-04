// M9 capability gate. A MAP_FIXED PROT_EXEC mapping onto an ALREADY-TRANSLATED backing must work:
// promote the range, then invalidate the guest's stale TLB entry with the guest-side TLBI oracle.
//
// This is deliberately separate from the jq gate. jq's wall-chain past this point is of unknown
// depth, and the capability must stay proven whether or not rung 2 goes green.
mod util;

#[test]
fn fixed_exec_over_touched_backing_records_and_replays() {
    let (rec, trace) = util::record(retrace_guest::TLBIEXEC);
    assert_eq!(rec.code, 42,
        "guest must exit with the mapped code's return value (42). 101 means the RECORDER aborted \
         in place_fixed — the exec-over-live-backing refusal. stderr:\n{}", rec.stderr);

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 42, "replay must reproduce the exit code. stderr:\n{}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout, "replay stdout diverged from the recording");
}

#[test]
fn fixed_exec_over_touched_backing_is_trace_reproducible() {
    // Freestanding (-nostdlib -static): no clock, no entropy, no libmalloc — so the second oracle
    // applies. Two recordings must be byte-identical, proving the TLBI path introduces nothing
    // nondeterministic into the trace.
    //
    // Inlined rather than `util::assert_trace_reproducible` (which hardcodes exit 0 — right for its
    // existing HELLO/USRSTACK callers, both of which exit 0): this fixture deliberately exits 42, the
    // mapped code's return value, so a wrong answer cannot look like success (see tlbiexec.s's header
    // comment). Same two-recording byte-compare as that helper, gated on 42 instead.
    let (r1, t1) = util::record(retrace_guest::TLBIEXEC);
    assert_eq!(r1.code, 42, "first recording of tlbiexec failed: {}", r1.stderr);
    let (r2, t2) = util::record(retrace_guest::TLBIEXEC);
    assert_eq!(r2.code, 42, "second recording of tlbiexec failed: {}", r2.stderr);
    assert_eq!(r1.stdout, r2.stdout, "stdout differed between two recordings of tlbiexec");
    let (b1, b2) = (std::fs::read(&t1).expect("read trace 1"), std::fs::read(&t2).expect("read trace 2"));
    assert_eq!(b1, b2, "two recordings of tlbiexec produced different traces — a nondeterministic \
                         value is entering the trace");
}
