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
    assert!(rep.stderr.contains("thread"), "the divergence must name the thread mismatch:\n{}", rep.stderr);
}
