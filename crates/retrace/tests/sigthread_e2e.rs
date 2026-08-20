// crates/retrace/tests/sigthread_e2e.rs
//
// M16 Task 7: the signal main raised for its CHILD is recorded as delivered to the child.
//
// Two tests, two claims, deliberately not overlapping: the first owns the child-directed raise (the
// FIRST delivery names the child) and this guest's whole ordered stdout; the second owns Task 9's
// masked/pending half (the delivery PAIR, and the second delivery's anchoring to the unmask
// landmark). Either one alone would be a weaker test if it also restated the other's claim.
//
// This asserts on the TRACE, not on the exit code: a guest whose handler never ran at all also
// exits 0, and a guest whose handler ran on the WRONG thread also exits 0. The recorded
// SignalDelivery.thread is the only observable that separates the three.
mod util;
use retrace_trace::Event;

#[test]
fn the_signal_is_delivered_to_the_named_child_thread() {
    let (rec, trace) = util::record_dynamic(retrace_guest::SIGTHREAD);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    let events = retrace_trace::Reader::open(&trace).unwrap();
    let delivered: Vec<u32> = events.iter().filter_map(|e| match e {
        Event::SignalDelivery { sig: 30, thread, .. } => Some(*thread),
        _ => None,
    }).collect();

    // Task 9 gave this guest a SECOND delivery — main's own signal, pended under a mask and
    // materialised at the unmask — so "exactly one" is no longer true and no longer this test's
    // claim. Task 7's claim is about the FIRST delivery only: the one `pthread_kill(child, …)`
    // produced went to the CHILD. The pair, and the anchoring of the second, belong to the pending
    // test below; restating them here would leave two tests asserting the same thing and neither
    // owning its own.
    //
    // How this still FAILS if the signal went to main instead of the child: the guest raises
    // SIGUSR1 exactly twice, in source order — at `pthread_kill(child)` and, much later, at
    // `pthread_kill(pthread_self())` — and the trace is ordered, so `delivered[0]` IS the
    // child-directed kill's delivery. Ignoring the target port makes it 0 (main takes its own
    // signal), which is exactly what this rejects. The ordered-stdout check below is the second,
    // independent tooth on the same claim: delivering to main happens SYNCHRONOUSLY inside
    // `pthread_kill`, so `handler` would print before `kill rc 0`.
    assert!(!delivered.is_empty(),
        "no SIGUSR1 delivery recorded at all — the child-directed raise produced nothing");
    assert_eq!(delivered[0], 1u32,
        "the FIRST SIGUSR1 delivery, the one pthread_kill(child) produced, must name thread 1 — \
         the child. Thread 0 here means the target port was ignored and main took its own signal, \
         which is the defect M16 closes.");

    // The user-visible half of the same claim, and the half Task 5 asserted inverted. `kill rc 0`
    // must now come BEFORE `handler`: main's pthread_kill returns without running anything, and
    // the child runs the handler only once main blocks in join. This is also, exactly, the order a
    // native run produces (MEASURED 20/20) — so M16 does not merely relabel a trace field, it
    // corrects observable behaviour.
    // Every line, in order — same shape as the Task 5 test this replaces, and for the same reason:
    // a filtered allowlist would silently drop unexpected output, and the length check is what
    // catches it. The pthread line is matched by prefix so an unrelated memory-layout change cannot
    // raise a false alarm here. Task 9 appended four lines to the guest (`self kill rc 0` ..
    // `unblocked`), and they are asserted here as well as in the pending test below: this test owns
    // "every line, in order", which it cannot keep while stopping at line 6.
    let out = String::from_utf8_lossy(&rec.stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 10, "unexpected extra or missing stdout line:\n{out}");
    assert_eq!(lines[0], "installed");
    assert!(lines[1].starts_with("child pthread 0x"), "line 2 was {:?}", lines[1]);
    assert_eq!(&lines[2..], &["kill rc 0", "handler", "child body", "joined",
                              "self kill rc 0", "pending 1", "handler", "unblocked"],
        "post-M16 order: the CHILD takes the signal. Full stdout:\n{out}");

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay must be clean; stderr:\n{}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout, "replay must be byte-identical");
}

/// M16 Task 9: a masked signal pends and is delivered at the unmask, not before and not never.
///
/// Two deliveries in this trace, and the pair is the assertion: the child's at the pthread_kill
/// landmark, main's at the pthread_sigmask landmark. One delivery means the pending half was
/// dropped; three means it was delivered twice.
#[test]
fn a_masked_signal_pends_and_is_delivered_when_the_mask_lifts() {
    let (rec, trace) = util::record_dynamic(retrace_guest::SIGTHREAD);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout);

    assert!(out.contains("pending 1"),
        "sigpending must report the signal main raised on itself while masked — an always-empty \
         answer is the lie M11 flagged and M16 fixes. stdout:\n{out}");

    let events = retrace_trace::Reader::open(&trace).unwrap();
    let delivered: Vec<u32> = events.iter().filter_map(|e| match e {
        Event::SignalDelivery { sig: 30, thread, .. } => Some(*thread),
        _ => None,
    }).collect();
    assert_eq!(delivered, vec![1u32, 0u32],
        "the child's delivery first, then main's pending one at the unmask landmark — and EXACTLY \
         those two: a third would mean `take_deliverable` did not clear the bit and the guest's \
         second mask call re-materialised the same signal");

    // The pending delivery is anchored to the mask call, not to some later point: the landmark
    // immediately before it is the pthread_sigmask that unblocked it. This is the assertion that
    // separates M16's design from the one it rejected — materialising at the SCHEDULER's switch
    // point would put a trace event below the trace, and the delivery would land next to some
    // arbitrary other landmark instead of this one.
    let di = events.iter().rposition(|e| matches!(e, Event::SignalDelivery { .. })).unwrap();
    match &events[di - 1] {
        Event::Syscall { num, .. } => assert!(
            *num == retrace_arch::SYS_SIGPROCMASK || *num == retrace_arch::SYS_PTHREAD_SIGMASK,
            "the pending delivery must sit immediately after the unmasking syscall, got num {num}"),
        other => panic!("expected the unmasking Syscall landmark before the delivery, got {other:?}"),
    }

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay must be clean; stderr:\n{}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout, "replay must be byte-identical");
}
