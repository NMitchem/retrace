// M6 CLI crash surfaces. Static guests here; the dynamic crashy.c path lands in Task 3 and the
// #[ignore]d headline gate in Task 6.
mod util;

#[test]
fn record_and_replay_of_a_crash_exit_139_with_the_crash_line() {
    let (rec, trace) = util::record(retrace_guest::CRASH);
    assert_eq!(rec.code, 139, "stderr: {}", rec.stderr);
    assert!(rec.stderr.contains("guest crashed: pc="), "stderr: {}", rec.stderr);
    assert!(rec.stderr.contains("far=0x4000dead0000"), "stderr: {}", rec.stderr);
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 139, "stderr: {}", rep.stderr);
    assert!(rep.stderr.contains("far=0x4000dead0000"), "stderr: {}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout);
}
