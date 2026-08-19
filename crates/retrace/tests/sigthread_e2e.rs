// crates/retrace/tests/sigthread_e2e.rs
//
// M16 Task 5: the fixture exists and records. This is NOT yet the attribution gate — at this point
// retrace still ignores pthread_kill's target port and delivers to whoever is running, so the
// handler line here proves only that the guest is viable, not that the RIGHT thread took it.
// Task 7 replaces this test's body with the attribution assertion.
mod util;

#[test]
fn the_sigthread_guest_records_and_replays_with_main_wrongly_taking_the_signal() {
    let (rec, trace) = util::record_dynamic(retrace_guest::SIGTHREAD);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    // The ORDER, not a bag of contains() — a set-membership check passes identically before and
    // after Task 7 and would prove nothing. `handler` before `kill rc 0` means the handler ran
    // synchronously inside main's pthread_kill: main took the signal it aimed at the child. That
    // is today's defect, asserted rather than tolerated, so Task 7's flip is a visible change.
    let out = String::from_utf8_lossy(&rec.stdout);
    let order: Vec<&str> = out.lines()
        .filter(|l| ["installed", "handler", "kill rc 0", "child body", "joined"].contains(l))
        .collect();
    assert_eq!(order, vec!["installed", "handler", "kill rc 0", "child body", "joined"],
        "pre-Task-7 order: main takes its own signal inside pthread_kill. Full stdout:\n{out}");

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay must be clean; stderr:\n{}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout, "replay must be byte-identical");
}
