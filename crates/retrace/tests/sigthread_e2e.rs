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

    // EVERY line, in order — not a filtered allowlist. A filter silently drops output the guest
    // was not expected to produce, so a spurious warning or a second delivery would pass unnoticed;
    // the length check is what closes that. The pthread line is matched by PREFIX rather than by
    // value: its address is deterministic today (0x30207000, measured), but pinning a guest address
    // would make this test fail for any unrelated change to the box's memory layout, which is a
    // false alarm rather than a finding.
    let out = String::from_utf8_lossy(&rec.stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 6, "unexpected extra or missing stdout line:\n{out}");
    assert_eq!(lines[0], "installed");
    assert!(lines[1].starts_with("child pthread 0x"), "line 2 was {:?}", lines[1]);
    assert_eq!(&lines[2..], &["handler", "kill rc 0", "child body", "joined"],
        "pre-Task-7 order: main takes its own signal inside pthread_kill. Full stdout:\n{out}");

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay must be clean; stderr:\n{}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout, "replay must be byte-identical");
}
