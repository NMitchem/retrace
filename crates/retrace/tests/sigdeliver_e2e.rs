// M12 mechanism gates. Freestanding guests with their own trampolines: they test retrace's entry
// contract without Apple's _sigtramp in the way (that one is Task 10's job).
//
// Each gate covers something the headline gate cannot see. A wild-pointer fault runs fine on the
// main stack, so SA_ONSTACK could be ignored entirely and a headline pass would prove nothing; and
// a guest that re-faults and dies immediately can never reveal clobbered vector state.
mod util;

#[test]
fn the_trampoline_is_entered_with_the_measured_registers() {
    let (rec, trace) = util::record(retrace_guest::SIGFRAME);
    // sigframe.s exits 0 only if x1..x5 and sp all matched; each mismatch exits with its own code
    // (21 infostyle, 22 signo, 23 sp, 24 uctx, 25 siginfo, 26 token).
    assert_eq!(rec.code, 0,
        "entry-register contract violated (see sigframe.s for the per-check exit codes); stderr:\n{}",
        rec.stderr);
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay stderr:\n{}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout);
}

#[test]
fn a_handler_can_repair_a_fault_and_sigreturn_past_it() {
    let (rec, trace) = util::record(retrace_guest::SEGVCATCH);
    assert_eq!(rec.code, 0,
        "the handler advances __ss.__pc by 4 and the guest continues; stderr:\n{}", rec.stderr);
    assert_eq!(rec.stdout, b"caught\nresumed\n",
        "both lines prove it: the handler ran AND the guest came back past the faulting store");
    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 0, "replay {i} stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} diverged");
    }
}

#[test]
fn a_handler_with_sa_onstack_runs_on_the_alternate_stack() {
    let (rec, trace) = util::record(retrace_guest::ALTSTACK);
    assert_eq!(rec.code, 0, "the handler asserts its own sp is inside the alt stack; stderr:\n{}",
        rec.stderr);
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay stderr:\n{}", rep.stderr);
}

#[test]
fn vector_state_survives_a_caught_signal() {
    // A handler is ordinary compiled code and will use NEON. Without sigreturn restoring Q0-Q31, a
    // handler that RETURNS silently corrupts the guest.
    let (rec, trace) = util::record(retrace_guest::VECSURVIVE);
    assert_eq!(rec.code, 0, "v8 must hold its pre-signal value after sigreturn; stderr:\n{}",
        rec.stderr);
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay stderr:\n{}", rep.stderr);
}

#[test]
fn a_blocked_synchronous_fault_fails_loud() {
    // The fail-loud pattern from killother_e2e: a nonzero exit whose stderr names the boundary.
    // A fault cannot be deferred, POSIX leaves it undefined, and M11 models no pending set — so
    // retrace asserts rather than guessing.
    let (rec, _trace) = util::record(retrace_guest::BLOCKEDFAULT);
    assert_ne!(rec.code, 0, "the guest must not reach exit(0); stderr:\n{}", rec.stderr);
    assert!(rec.stderr.contains("raising blocked signal"),
        "the abort must NAME the unmodelled boundary; stderr:\n{}", rec.stderr);
}

// The SECOND oracle, applied to the delivery path. The divergence oracle compares a replay against
// ONE recording, so it is structurally blind to a nondeterministic value entering the trace — the
// recording captures it once and replay reproduces it forever. This compares two RECORDINGS from
// two separate recorder processes, and a signal frame is exactly the kind of thing that could carry
// a host address or a per-process token into the trace without anyone noticing.
//
// segvcatch ONLY. The other three delivery fixtures self-raise, which needs a pid, and M11
// deliberately leaves getpid(20) forwarding — so the RECORDER's pid lands in their traces and two
// recordings differ by exactly that record. That is a known, documented property, not a defect
// this gate should paper over by relaxing the oracle. segvcatch faults instead, calls no getpid,
// and is therefore the one delivery guest this oracle can hold to.
#[test]
fn two_recordings_of_a_caught_fault_are_byte_identical() {
    util::assert_trace_reproducible(retrace_guest::SEGVCATCH);
}
