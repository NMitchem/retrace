// M6 CLI crash surfaces. Static guests here; the dynamic crashy.c path lands in Task 3, and the
// headline gate (`crash_demo_end_to_end`, below) landed in Task 6 — un-ignored, not #[ignore]d.
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

/// THE M6 HEADLINE GATE. One script, the whole story: record a real dynamically-linked C program
/// whose planted memory-corruption bug crashes it; replay verifies the crash bit-for-bit (twice);
/// the debugger seeks to the crash, watches the corrupted pointer, runs BACKWARD to the exact
/// out-of-bounds store that corrupted it, and proves it by watching the value flip on one stepi.
#[test]
fn crash_demo_end_to_end() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "record: {}", rec.stderr);
    assert!(rec.stderr.contains(&format!("far=0x{GARBAGE_VA:x}")), "record: {}", rec.stderr);
    for _ in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 139, "replay: {}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout);
    }
    let (_st, ptr) = util::discover_crashy_addrs(&trace);
    let script = format!(
        "continue; where; watch 0x{ptr:x}; reverse-continue; x 0x{ptr:x} 8; stepi; x 0x{ptr:x} 8");
    let out = std::process::Command::new(util::bin())
        .args(["debug", trace.to_str().unwrap(), "--script", &script])
        .output().unwrap();
    assert_eq!(out.status.code(), Some(0), "debug: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("guest crashed: pc="), "{stdout}");
    assert!(stdout.contains(&format!("hit watch 0x{ptr:x} (write at ")), "{stdout}");
    let garbage_hex = GARBAGE_VA.to_le_bytes().iter()
        .map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
    let xs: Vec<&str> = stdout.lines().filter(|l| l.starts_with(&format!("0x{ptr:x}:"))).collect();
    assert_eq!(xs.len(), 2, "{stdout}");
    assert!(!xs[0].contains(&garbage_hex) && xs[1].contains(&garbage_hex),
            "the reverse-continue landed ON the corrupting store:\n{stdout}");
}
