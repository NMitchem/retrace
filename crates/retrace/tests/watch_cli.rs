// Golden-transcript tests for M5 watchpoints. NEW file: the pre-M5 transcripts in debug_cli.rs
// are a regression oracle and must stay byte-identical. Every coordinate here is DISCOVERED:
// `target` from the recorded write(1, target, 8) args; the store coordinates by an independent
// memory-scan oracle (step + read_mem), so the watchpoint machinery is checked against ground
// truth it cannot influence.
mod util;
use std::path::Path;

fn debug_run(trace: &str, script: &str) -> (i32, String, String) {
    let out = std::process::Command::new(util::bin())
        .args(["debug", trace, "--script", script])
        .output().expect("spawn debug");
    (out.status.code().unwrap_or(-1),
     String::from_utf8(out.stdout).unwrap(),
     String::from_utf8(out.stderr).unwrap())
}

fn discover_target(trace: &Path) -> u64 {
    let mut s = retrace_core::ReplaySession::open(trace).unwrap();
    loop {
        if let Some((4, args)) = s.peek_syscall() {
            if args[0] == 1 { return args[1]; }
        }
        s.advance().unwrap();
    }
}

/// Ground-truth store coordinates in window 1: step one instruction at a time from (1,0) and
/// record every K whose instruction changed `target`'s qword. Independent of the watch machinery.
fn discover_store_ks(trace: &Path, target: u64) -> Vec<u64> {
    let mut s = retrace_core::seek(trace, 1, 0).unwrap();
    let mut ks = Vec::new();
    let mut prev = s.read_mem(target, 8).unwrap();
    let mut k = 0u64;
    while s.step_insns(1).is_ok() {
        let cur = s.read_mem(target, 8).unwrap();
        if cur != prev { ks.push(k); prev = cur; }
        k += 1;
    }
    ks
}

#[test]
fn watch_continue_hits_first_store_and_progress_rule_advances() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let t = discover_target(tp);
    let ks = discover_store_ks(tp, t);
    assert!(ks.len() >= 2, "watchloop must store at least twice, got {ks:?}");
    let spc = { let s = retrace_core::seek(tp, 1, ks[0]).unwrap(); s.pc() }; // the (single) store pc

    let (code, out, err) = debug_run(ts, &format!("watch 0x{t:x}; continue; where; continue; where"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("watch at 0x{t:x} len 8")), "watch echo:\n{out}");
    assert!(out.contains(&format!("hit watch 0x{t:x} (write at 0x{spc:x}) at (1, +?)")), "hit line:\n{out}");
    assert!(out.contains(&format!("resolved (1, {})", ks[0])), "first store K:\n{out}");
    assert!(out.contains(&format!("at (1, {}) pc=0x{spc:x}", ks[0])), "where after first hit:\n{out}");
    // Progress rule: the second continue pre-steps off the un-retired store and lands on the NEXT
    // execution of the same store pc — ks[1], not ks[0] again.
    assert!(out.contains(&format!("resolved (1, {})", ks[1])), "second hit advances:\n{out}");
    assert!(out.trim_end().ends_with(&format!("at (1, {}) pc=0x{spc:x}", ks[1])), "final where:\n{out}");
}

#[test]
fn watch_validation_is_fail_loud() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0);
    let ts = trace.to_str().unwrap();
    // Parse-time errors: exit 5, no stdout at all.
    let (c1, o1, e1) = debug_run(ts, "watch 0x1001 8");
    assert_eq!(c1, 5); assert!(o1.is_empty());
    assert!(e1.contains("watch address 0x1001 must be 8-byte aligned"), "stderr: {e1}");
    let (c2, _, e2) = debug_run(ts, "watch 0x1000 3");
    assert_eq!(c2, 5);
    assert!(e2.contains("watch len must be 1, 2, 4, or 8; got 3"), "stderr: {e2}");
    // Exec-time cap: the 5th watch errors naming the hardware limit.
    let script = (0..5).map(|i| format!("watch 0x{:x}", 0x10000u64 + i * 8))
        .collect::<Vec<_>>().join("; ");
    let (c3, _, e3) = debug_run(ts, &script);
    assert_eq!(c3, 5, "5th watch must be a loud error");
    assert!(e3.contains("cannot arm more than 4 watchpoints (hardware limit: DBGWVR0-3)"), "stderr: {e3}");
}

#[test]
fn unwatch_disarms() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let t = discover_target(tp);
    let (code, out, err) = debug_run(ts, &format!("watch 0x{t:x}; unwatch 0x{t:x}; continue"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("unwatched 0x{t:x}")), "unwatch echo:\n{out}");
    assert!(out.contains("exited (code 0)"), "runs to exit:\n{out}");
    assert!(!out.contains("hit watch"), "no hit after unwatch:\n{out}");
}
