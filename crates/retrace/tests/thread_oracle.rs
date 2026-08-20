// M15 Task 4. The replay divergence oracle now compares the RECORDED thread tag against the LIVE
// scheduled thread on every syscall — this is the gap M14's Status section named as the oracle's
// sharpest limit: two threads running the SAME code issue byte-identical (num, args), so without a
// thread compare a wrong-thread replay of identical code continues in silence.
//
// `a_rust_guest_spawns_a_thread_and_joins_it` (thread_rust_e2e.rs) already proves replay stays
// byte-identical on an UNTAMPERED trace, but that test cannot distinguish a working thread oracle
// from no oracle at all — on the happy path record and replay schedule identically regardless, so
// the check never fires either way. This test exists to make the oracle itself observable: it must
// refuse a trace whose recorded schedule the live run does not match.
mod util;
use retrace_trace::Event;

#[test]
fn a_wrong_thread_on_replay_is_a_divergence() {
    // THREADRUST, not a single-threaded guest, and deliberately so: a single-threaded recording
    // tags every syscall with the same thread id, so there is no OTHER valid id to retag to — the
    // mutation below would have to invent a bogus constant, which would test something weaker (that
    // an out-of-range id is rejected) rather than the real property (that a wrong but genuinely
    // live thread is caught). THREADRUST's main spawns a child that itself prints and thus issues
    // its own syscalls, so the trace contains at least two distinct, real, scheduled thread ids.
    let (rec, trace) = util::record_dynamic(retrace_guest::THREADRUST);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout);
    assert!(out.contains("child ran"),
        "the child thread must actually run for this trace to contain more than one live thread \
         id; stdout:\n{out}");

    let mut events = retrace_trace::Reader::open(&trace).unwrap();

    // The set of thread ids that actually issued a syscall in this recording.
    let mut ids: Vec<u32> = events.iter().filter_map(|e| match e {
        Event::Syscall { thread, .. } => Some(*thread),
        _ => None,
    }).collect();
    ids.sort_unstable();
    ids.dedup();
    assert!(ids.len() >= 2,
        "need syscalls from at least two different threads to express this mutation meaningfully — \
         got thread ids {ids:?}. If THREADRUST's schedule ever changes such that the child issues \
         no syscall of its own, this trace would carry only one live thread id and this test would \
         need a different guest.");

    // Retag the FIRST syscall event (issued by main, before the child is even spawned) to some
    // OTHER thread id that genuinely appears elsewhere in THIS SAME trace — a real live thread the
    // guest actually scheduled, not an out-of-range constant.
    let first_idx = events.iter().position(|e| matches!(e, Event::Syscall { .. }))
        .expect("a recording of a guest that prints must contain a syscall");
    let orig = match &events[first_idx] { Event::Syscall { thread, .. } => *thread, _ => unreachable!() };
    let other = *ids.iter().find(|&&t| t != orig)
        .expect("at least one other id exists in `ids`, asserted above");
    if let Event::Syscall { thread, .. } = &mut events[first_idx] { *thread = other; }

    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);

    // CLI exit 3 is the Divergence convention (main.rs's `Err(d)` arm on `replay`), the same
    // convention every other divergence e2e in this crate checks against.
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 3,
        "a wrong-thread replay must be reported as a DIVERGENCE, not silently accepted; stderr:\n{}",
        rep.stderr);
    assert!(rep.stderr.contains("schedule diverged"),
        "the divergence detail should name what diverged (the schedule), not just that something \
         did; stderr:\n{}", rep.stderr);
}

/// M16 Task 11. The three terminal-ish landmarks gained a thread tag; retagging any of them to a
/// genuinely live thread id must diverge. `Exit` is the one every guest reaches, so it is the one
/// that can be tested against a real threaded recording without a bespoke fixture.
#[test]
fn a_wrong_thread_on_the_exit_landmark_is_a_divergence() {
    let (rec, trace) = util::record_dynamic(retrace_guest::THREADRUST);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    let mut events = retrace_trace::Reader::open(&trace).unwrap();
    let ids: Vec<u32> = { let mut v: Vec<u32> = events.iter().filter_map(|e| match e {
        Event::Syscall { thread, .. } => Some(*thread), _ => None }).collect();
        v.sort_unstable(); v.dedup(); v };
    assert!(ids.len() >= 2, "need a genuinely threaded recording; got {ids:?}");

    let i = events.iter().position(|e| matches!(e, Event::Exit { .. }))
        .expect("a clean run ends in Exit");
    let orig = match &events[i] { Event::Exit { thread, .. } => *thread, _ => unreachable!() };
    let other = *ids.iter().find(|&&t| t != orig).expect("a second live id exists");
    if let Event::Exit { thread, .. } = &mut events[i] { *thread = other; }

    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 3, "CLI exit 3 is the Divergence convention; stderr:\n{}", rep.stderr);
    assert!(rep.stderr.contains("schedule diverged"),
        "the divergence detail should name what diverged (the schedule), not just that something \
         did; stderr:\n{}", rep.stderr);
}

/// M16 Task 12a. The CAUGHT-RAISE mirror, reached with two live threads for the first time.
///
/// M15 could only prove this arm FIRES: SIGFRAME is single-threaded, so there was no other live id
/// to retag to and the test had to use a bogus constant. SIGTHREAD's pthread_kill lands here with a
/// real second thread in the table, so the retag below is to a thread the guest actually scheduled.
#[test]
fn a_wrong_thread_at_the_caught_raise_mirror_is_a_divergence() {
    retag_and_expect_divergence(retrace_arch::SYS_PTHREAD_KILL);
}

/// M16 Task 12b. The SIGRETURN mirror. Stronger than 12a: the thread current at this landmark is
/// the CHILD, so the recorded tag here is a nonzero id — a value this arm has never seen.
#[test]
fn a_wrong_thread_at_the_sigreturn_mirror_is_a_divergence() {
    let orig = retag_and_expect_divergence(retrace_arch::SYS_SIGRETURN);
    // The precondition that makes 12b STRONGER than 12a, asserted rather than asserted-in-prose: the
    // thread current at this landmark must be the CHILD. If the fixture ever schedules sigreturn on
    // main instead, this test silently degrades into a duplicate of 12a — still green, no longer
    // covering the nonzero-tag case its doc comment claims.
    assert_ne!(orig, 0,
        "sigreturn must be tagged with a nonzero (child) thread id for this test to cover what 12a \
         does not; got {orig} — the fixture's schedule changed");
}

/// Retag the first `Syscall` landmark for `num` to some OTHER live thread id from this same trace
/// and assert replay refuses it. Shared by 12a and 12b; the reasoning for why the retag target must
/// be a genuinely live id, not an out-of-range constant, is the same one
/// `a_wrong_thread_on_replay_is_a_divergence` above already documents.
/// Returns the tag it found BEFORE retagging, so a caller can assert on what the fixture actually
/// scheduled rather than trusting the comment above it.
fn retag_and_expect_divergence(num_wanted: u64) -> u32 {
    let (rec, trace) = util::record_dynamic(retrace_guest::SIGTHREAD);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    let mut events = retrace_trace::Reader::open(&trace).unwrap();
    let mut ids: Vec<u32> = events.iter().filter_map(|e| match e {
        Event::Syscall { thread, .. } => Some(*thread), _ => None }).collect();
    ids.sort_unstable(); ids.dedup();
    assert!(ids.len() >= 2,
        "SIGTHREAD must schedule at least two threads that issue syscalls, or this mutation is the \
         bogus-constant one M15 was stuck with; got {ids:?}");

    let i = events.iter().position(|e| matches!(e, Event::Syscall { num, .. } if *num == num_wanted))
        .unwrap_or_else(|| panic!("no Syscall landmark for num {num_wanted} — this fixture no \
                                   longer reaches the mirror this test exists to cover"));
    let orig = match &events[i] { Event::Syscall { thread, .. } => *thread, _ => unreachable!() };
    let other = *ids.iter().find(|&&t| t != orig)
        .expect("a genuinely live second id, not a constant");
    if let Event::Syscall { thread, .. } = &mut events[i] { *thread = other; }

    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 3, "CLI exit 3 is the Divergence convention; stderr:\n{}", rep.stderr);
    // Exit 3 alone is too weak: it is the convention for EVERY divergence, so a trace that diverged
    // for some unrelated reason would pass this test while proving nothing about the thread oracle.
    // Pin the message `verify_thread` actually emits.
    assert!(rep.stderr.contains("the schedule diverged"),
        "the divergence must be the THREAD oracle's, not merely some divergence; stderr:\n{}",
        rep.stderr);
    orig
}

/// M16 Task 12c. `SignalDelivery.thread` is the one tag whose check the frame byte-compare does NOT
/// subsume — and the only one with no test until now.
///
/// The retag leaves `writes` untouched on purpose. A wrong-thread DELIVERY lands the frame on a
/// different stack, so `Region`'s derived PartialEq (over `ipa` as well as `bytes`) trips the frame
/// compare first and this check never speaks. Corrupting only the TAG is the one input that isolates
/// it. Task 8's review measured the failure this guards: changing record's `thread: target as u32`
/// to `thread` — tagging the delivery with the caller instead of the resolved target, the exact
/// "simplification" the comments there warn against — yields a perfectly valid trace that every
/// other check accepts.
#[test]
fn a_wrong_thread_on_the_delivery_landmark_is_a_divergence() {
    let (rec, trace) = util::record_dynamic(retrace_guest::SIGTHREAD);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    let mut events = retrace_trace::Reader::open(&trace).unwrap();
    let i = events.iter().position(|e| matches!(e, Event::SignalDelivery { .. }))
        .expect("SIGTHREAD must record a delivery");
    let orig = match &events[i] { Event::SignalDelivery { thread, .. } => *thread, _ => unreachable!() };
    assert_eq!(orig, 1, "the delivery must be tagged with the CHILD, or this fixture no longer \
                         exercises a cross-thread delivery and the retag below proves nothing");
    // Only the tag. Not one byte of `writes`.
    if let Event::SignalDelivery { thread, .. } = &mut events[i] { *thread = 0; }

    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 3, "CLI exit 3 is the Divergence convention; stderr:\n{}", rep.stderr);
    assert!(rep.stderr.contains("signal delivery thread mismatch"),
        "the divergence must be the DELIVERY thread check, not the frame compare — if the frame \
         compare fired, the retag touched `writes` and the test is measuring the wrong thing:\n{}",
        rep.stderr);
}

/// M17. The signal materialised at a WAKE is tagged with the woken thread — neither the caller of
/// the syscall that produced the landmark (that is the waker) nor the thread that was current. No
/// other delivery in the tree has that shape, so `mirror_delivery`'s inline receiving-thread check
/// reaches this route for the first time here.
///
/// Only the TAG is corrupted, not one byte of `writes` — the same isolation
/// `a_wrong_thread_on_the_delivery_landmark_is_a_divergence` documents above: a wrong-thread
/// delivery lands the frame on a different stack, so `Region`'s derived `PartialEq` would trip the
/// frame compare first and this check would never speak.
#[test]
fn a_wrong_thread_on_a_wake_materialised_delivery_is_a_divergence() {
    let (rec, trace) = util::record_dynamic(retrace_guest::SIGBLOCKED);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    let mut events = retrace_trace::Reader::open(&trace).unwrap();
    let i = events.iter().position(|e| matches!(e, Event::SignalDelivery { sig: 30, .. }))
        .expect("SIGBLOCKED must record exactly one SIGUSR1 delivery — sigblocked_e2e asserts it");
    let orig = match &events[i] { Event::SignalDelivery { thread, .. } => *thread, _ => unreachable!() };
    assert_eq!(orig, 1,
        "the delivery must be tagged with `a` (tid 1), the BLOCKED target — if this is 0 or 2 the \
         signal went to the waker or to main and M17's whole claim is wrong");

    // Retag to the WAKER (tid 2, `b`), which is the specific wrong answer this route invites: it is
    // the thread whose syscall produced the landmark, and therefore the tag a careless
    // implementation would reach for.
    if let Event::SignalDelivery { thread, .. } = &mut events[i] { *thread = 2; }

    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 3, "CLI exit 3 is the Divergence convention; stderr:\n{}", rep.stderr);
    assert!(rep.stderr.contains("signal delivery thread mismatch"),
        "the divergence must be the DELIVERY thread check, not merely some divergence — exit 3 \
         alone would pass on any of them; stderr:\n{}", rep.stderr);
}
