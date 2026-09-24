// M40: the repo-owned guard for watch-hit resolution BY ADDRESS. WATCHSWEEP's one sweeping store
// runs on 40 other addresses before it reaches the watched buf[40], so a resolver that matches the
// hit's pc (pre-M40 `resolve_hit_k`) lands on an earlier run of the store that never wrote the
// watched element. That is what rung 8's forward `continue` did, 1.7 M instructions early
// (t0 M7). Every coordinate here is DISCOVERED by an oracle the watch machinery cannot influence
// (step + read_mem, the watch_cli convention).
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

/// `&buf[40]`, from the recorded write(1, target, 8).
fn discover_target(trace: &Path) -> u64 {
    let mut s = retrace_core::ReplaySession::open(trace).unwrap();
    loop {
        if let Some((4, args)) = s.peek_syscall() {
            if args[0] == 1 { return args[1]; }
        }
        s.advance().unwrap();
    }
}

/// Ground truth, independent of the watch machinery: every K in window 1 whose instruction changed
/// `target`'s qword.
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

/// Every K in window 1 whose instruction is at `pc`: how often a store instruction ran.
fn ks_at_pc(trace: &Path, pc: u64) -> Vec<u64> {
    let mut s = retrace_core::seek(trace, 1, 0).unwrap();
    let mut out = Vec::new();
    let mut k = 0u64;
    loop {
        if s.pc() == pc { out.push(k); }
        if s.step_insns(1).is_err() { break; }
        k += 1;
    }
    out
}

fn pc_at(trace: &Path, k: u64) -> u64 { retrace_core::seek(trace, 1, k).unwrap().pc() }

/// The debugger's `x` rendering of an 8-byte little-endian value.
fn x_bytes(v: u64) -> String {
    v.to_le_bytes().iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

#[test]
fn continue_resolves_a_watch_hit_to_the_write_not_an_earlier_run_of_the_store() {
    let (rec, trace) = util::record(retrace_guest::WATCHSWEEP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let (tp, ts) = (Path::new(&trace), trace.to_str().unwrap());
    let t = discover_target(tp);
    let ks = discover_store_ks(tp, t);
    assert_eq!(ks.len(), 2, "watchsweep writes buf[40] exactly twice, got {ks:?}");
    let spc = pc_at(tp, ks[0]);
    // The precondition this file exists for: the sweeping store ran on other addresses FIRST.
    let runs = ks_at_pc(tp, spc);
    assert_eq!(runs.len(), 64, "the sweeping store runs once per element: {runs:?}");
    assert!(runs[0] < ks[0], "its first run precedes the watched write: {runs:?} vs {ks:?}");

    let (code, out, err) = debug_run(ts, &format!("watch 0x{t:x}; continue; stepi; x 0x{t:x} 8"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("hit watch 0x{t:x} (write at 0x{spc:x}) at (1, +?)")), "hit line:\n{out}");
    assert!(out.contains(&format!("resolved (1, {})", ks[0])),
        "continue must resolve to the write (K={}), not the store's first run (K={}):\n{out}", ks[0], runs[0]);
    // The effect, which no coordinate bookkeeping can fake: stepping the reported instruction lands
    // the sweep's value in buf[40].
    assert!(out.contains(&format!("0x{t:x}: {}", x_bytes(0x1111_1111_1111_1111 + 40))),
        "stepi over the reported instruction must write buf[40]:\n{out}");
}

#[test]
fn reverse_continue_walks_back_through_both_writers() {
    let (rec, trace) = util::record(retrace_guest::WATCHSWEEP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let (tp, ts) = (Path::new(&trace), trace.to_str().unwrap());
    let t = discover_target(tp);
    let ks = discover_store_ks(tp, t);
    assert_eq!(ks.len(), 2, "{ks:?}");
    let (pc1, pc2) = (pc_at(tp, ks[0]), pc_at(tp, ks[1]));
    assert_ne!(pc1, pc2, "the second writer is a different instruction");

    let (code, out, err) = debug_run(ts,
        &format!("continue; watch 0x{t:x}; reverse-continue; reverse-continue; reverse-continue"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("hit watch 0x{t:x} (write at 0x{pc2:x}) at (1, {})", ks[1])),
        "first reverse-continue from the exit: the second writer:\n{out}");
    assert!(out.contains(&format!("hit watch 0x{t:x} (write at 0x{pc1:x}) at (1, {})", ks[0])),
        "second: the sweep's write to buf[40], not an earlier run of its store:\n{out}");
    assert!(out.contains("no earlier hit"), "third: nothing before the first write:\n{out}");
}
