// THE M14 HEADLINE GATE. A stock full-std Rust binary spawns a thread, the child runs on retrace's
// single vCPU under a cooperative scheduler, and its return value crosses back through join.
//
// The exit code proves nothing on its own: a guest that never spawned also exits 0. `joined 42` is
// the assertion — it requires the child to have RUN and its value to have PROPAGATED.
mod util;
use retrace_trace::Event;

#[test]
fn a_rust_guest_spawns_a_thread_and_joins_it() {
    let (rec, trace) = util::record_dynamic(retrace_guest::THREADRUST);
    let out = String::from_utf8_lossy(&rec.stdout);

    assert!(out.contains("main before spawn"), "the guest must reach main; stdout:\n{out}");
    assert!(out.contains("child ran"),
        "THE CHILD THREAD MUST ACTUALLY RUN. Missing this line is what a scheduler that never \
         switches looks like; stdout:\n{out}");
    assert!(out.contains("joined 42"),
        "the child's return value must cross back through join; stdout:\n{out}");
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    let (events, torn) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(!torn, "a recorder killed mid-run leaves a torn trace — this must be complete");

    // The guest genuinely asked for a thread, rather than libstd optimizing the spawn away.
    assert!(
        events.iter().any(|e| matches!(e,
            Event::Syscall { num, .. } if *num == retrace_arch::SYS_BSDTHREAD_CREATE)),
        "the trace must contain the bsdthread_create the guest issued"
    );

    // Replay is byte-identical, twice. This is where a nondeterministic schedule would surface:
    // a different interleaving reorders the guest's own writes and the stdout comparison fails.
    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 0, "replay {i}; stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged — the schedule is not pure");
    }
}
