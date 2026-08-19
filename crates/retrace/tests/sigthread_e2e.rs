// crates/retrace/tests/sigthread_e2e.rs
//
// M16 Task 7: the signal main raised for its CHILD is recorded as delivered to the child.
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

    assert_eq!(delivered, vec![1u32],
        "exactly one SIGUSR1 delivery, to thread 1 — the child. Thread 0 here means the target \
         port was ignored and main took its own signal, which is the defect M16 closes.");

    // The user-visible half of the same claim, and the half Task 5 asserted inverted. `kill rc 0`
    // must now come BEFORE `handler`: main's pthread_kill returns without running anything, and
    // the child runs the handler only once main blocks in join. This is also, exactly, the order a
    // native run produces (MEASURED 20/20) — so M16 does not merely relabel a trace field, it
    // corrects observable behaviour.
    // Every line, in order — same shape as the Task 5 test this replaces, and for the same reason:
    // a filtered allowlist would silently drop unexpected output, and the length check is what
    // catches it. The pthread line is matched by prefix so an unrelated memory-layout change cannot
    // raise a false alarm here.
    let out = String::from_utf8_lossy(&rec.stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 6, "unexpected extra or missing stdout line:\n{out}");
    assert_eq!(lines[0], "installed");
    assert!(lines[1].starts_with("child pthread 0x"), "line 2 was {:?}", lines[1]);
    assert_eq!(&lines[2..], &["kill rc 0", "handler", "child body", "joined"],
        "post-M16 order: the CHILD takes the signal. Full stdout:\n{out}");

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay must be clean; stderr:\n{}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout, "replay must be byte-identical");
}
