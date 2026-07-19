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

const GARBAGE_VA: u64 = 0x4000_DEAD_0000; // mirrors c/crashy.c (source-defined)

#[test]
fn crashy_records_through_dyld_and_replays_bit_for_bit() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "stderr: {}", rec.stderr);
    assert!(rec.stderr.contains("guest crashed: pc="), "stderr: {}", rec.stderr);
    // far=0x4000dead0000, derived from GARBAGE_VA so the const is the single source of truth.
    assert!(rec.stderr.contains(&format!("far={GARBAGE_VA:#x}")), "stderr: {}", rec.stderr);
    let (st, ptr) = util::discover_crashy_addrs(&trace);
    assert_ne!(st, 0);
    assert_eq!(ptr, st + 144 + 32, "layout: ptr directly follows st(144) + buf(32)");
    for _ in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 139, "stderr: {}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout);
    }
}
