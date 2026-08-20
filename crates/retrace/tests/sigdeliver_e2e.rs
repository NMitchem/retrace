// M12 mechanism gates. Freestanding guests with their own trampolines: they test retrace's entry
// contract without Apple's _sigtramp in the way (that one is Task 10's job).
//
// Each gate covers something the headline gate cannot see. A wild-pointer fault runs fine on the
// main stack, so SA_ONSTACK could be ignored entirely and a headline pass would prove nothing; and
// a guest that re-faults and dies immediately can never reveal clobbered vector state.
mod util;
use retrace_trace::Event;

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

/// Fast-follow: the `sigaltstack` replay mirror. `sigaction`'s oldact writeback (:1392 in
/// retrace-core) is byte-compared on replay; `sigaltstack`'s oldstack writeback is not — see
/// `.superpowers/sdd/fastfollow-sigaltstack-brief.md`. This is that gap's mutation test.
///
/// ALTSTACK's fast-follow query (`sigaltstack(NULL, &oss)`, added to altstack.s beside the
/// pre-existing install) is what puts a real oldstack `Region` in the trace at all — the install
/// call alone passes `oss=NULL` and record writes nothing.
///
/// The corruption is confined to the encoded stack_t's zero-padding TAIL (byte 20, past
/// `ss_flags` at 16..20) — deliberately NOT one of the three semantic fields (`ss_sp`/`ss_size`/
/// `ss_flags`) altstack.s's own query check reads back. A field corruption would be caught by the
/// GUEST's own check (it would call `exit` with a code other than the recorded 0, tripping the
/// pre-existing generic exit-code divergence at the `Exit` landmark) and would prove something
/// weaker: that SOME check exists somewhere, not that the SIGALTSTACK writeback itself is
/// unchecked. Confining it to padding — which `encode_oldstack` always emits as zero and which no
/// guest instruction ever reads — isolates exactly the gap this test exists to prove: with no
/// sigaltstack-specific check, NOTHING notices at the landmark where the corruption actually is.
///
/// Measured, not assumed (see the fast-follow report): before the mirror landed, this did NOT
/// make replay report success either — `apply_and_return` painted the corrupted byte into guest
/// memory at the sigaltstack landmark unnoticed, but that byte sat untouched until the run's
/// PRE-EXISTING, unrelated final full-memory `Snapshot` diff (CLAUDE.md's "at exit does a
/// full-memory comparison") tripped over it — reported as a bare
/// `memory divergence at ipa 0x.. replay=0xff recorded=0x00`, naming an address, not sigaltstack,
/// and at a LATER landmark than the sigaltstack call itself. That accidental, coarse, late catch
/// is what "no sigaltstack-specific check" looked like in practice on this fixture.
///
/// Now that the mirror (retrace-core's `SYS_SIGALTSTACK` arm at :1408) recomputes and
/// byte-compares the oldstack writeback, this corruption is caught immediately, AT the sigaltstack
/// landmark, by a `Divergence` naming it — before the guest even resumes.
#[test]
fn a_corrupted_sigaltstack_oldstack_region_is_a_divergence() {
    let (rec, trace) = util::record(retrace_guest::ALTSTACK);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    let mut events = retrace_trace::Reader::open(&trace).unwrap();

    // The QUERY call — sigaltstack(NULL, &oss) — has args[0] == 0 (no new stack) and args[1] != 0
    // (a real oss pointer). That is the landmark whose recorded `writes` carries the oldstack
    // Region this test corrupts; the earlier INSTALL call (args[0] != 0, args[1] == 0) has none.
    let i = events.iter().position(|e| matches!(e, Event::Syscall { num, args, .. }
            if *num == retrace_arch::SYS_SIGALTSTACK && args[0] == 0 && args[1] != 0))
        .expect("ALTSTACK must issue sigaltstack(NULL, &oss) — see the fast-follow query in \
                 altstack.s; without it record writes no oldstack Region and there is nothing \
                 here to corrupt");
    let oldstack_ipa = match &events[i] { Event::Syscall { args, .. } => args[1], _ => unreachable!() };

    let region = match &mut events[i] {
        Event::Syscall { writes, .. } => writes.iter_mut().find(|r| r.ipa == oldstack_ipa)
            .expect("the sigaltstack query's recorded writes must include a Region at args[1]"),
        _ => unreachable!(),
    };
    assert_eq!(region.bytes.len(), 24, "encode_oldstack always emits a 24-byte stack_t");
    assert_eq!(region.bytes[20], 0, "byte 20 is documented zero padding (past ss_flags at \
        16..20) — this test's whole point depends on corrupting a byte the guest never reads");
    region.bytes[20] = 0xff;

    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 3,
        "a corrupted oldstack Region must be reported as a DIVERGENCE (CLI exit 3), not silently \
         accepted; got exit {} stderr:\n{}", rep.code, rep.stderr);
    assert!(rep.stderr.contains("sigaltstack oldstack mismatch"),
        "the divergence must NAME the sigaltstack oldstack mismatch, not just fail generically; \
         stderr:\n{}", rep.stderr);
}
