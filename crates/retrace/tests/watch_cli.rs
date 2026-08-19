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
    // WATCHLOOP is single-threaded throughout, so thread=0 is the only truthful answer (M15).
    assert!(out.trim_end().ends_with(&format!("at (1, {}) pc=0x{spc:x} thread=0", ks[1])), "final where:\n{out}");
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

#[test]
fn reverse_continue_finds_last_store() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let t = discover_target(tp);
    let ks = discover_store_ks(tp, t);
    let k_last = *ks.last().unwrap();
    let spc = { let s = retrace_core::seek(tp, 1, ks[0]).unwrap(); s.pc() };
    // Park just past the last store via stepi (watches are never armed during stepping), then ask
    // for the most recent writer: it must be the LAST store, not the first.
    let (code, out, err) = debug_run(ts,
        &format!("stepi {}; watch 0x{t:x}; reverse-continue; where", k_last + 1));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("hit watch 0x{t:x} (write at 0x{spc:x}) at (1, {k_last})")),
        "last-writer hit:\n{out}");
    // WATCHLOOP is single-threaded throughout, so thread=0 is the only truthful answer (M15).
    assert!(out.trim_end().ends_with(&format!("at (1, {k_last}) pc=0x{spc:x} thread=0")), "final where:\n{out}");
}

#[test]
fn reverse_continue_with_no_earlier_write_reports_none() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let t = discover_target(tp);
    // At (1, 0) nothing has written target yet.
    let (code, out, err) = debug_run(ts, &format!("watch 0x{t:x}; reverse-continue; where"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains("no earlier hit"), "no writer before (1,0):\n{out}");
    assert!(out.contains("at (1, 0)"), "position unchanged:\n{out}");
}

/// The read()'s buffer VA, the boundary landmark AFTER it, and that boundary's pc.
fn discover_read_cli(trace: &Path) -> (usize, u64, u64) {
    let mut s = retrace_core::ReplaySession::open(trace).unwrap();
    loop {
        if let Some((3, args)) = s.peek_syscall() {
            s.advance().unwrap();
            return (s.landmark(), args[1], s.position());
        }
        s.advance().unwrap();
    }
}

#[test]
fn syscall_writer_is_found_forward_and_backward() {
    let (rec, trace) = util::record(retrace_guest::FILEIO);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let (after_read, buf, bpc) = discover_read_cli(tp);
    let hit_line = format!("hit watch 0x{buf:x} (syscall write) at ({after_read}, 0)");
    let (code, out, err) = debug_run(ts,
        &format!("watch 0x{buf:x}; continue; where; stepi 2; reverse-continue; where"));
    assert_eq!(code, 0, "stderr: {err}");
    // Forward: continue stops at the read's boundary. Backward from (after_read, 2): the same
    // syscall hit at (after_read, 0) is strictly earlier — found again.
    assert_eq!(out.matches(&hit_line).count(), 2, "forward + reverse hits:\n{out}");
    assert!(out.contains(&format!("at ({after_read}, 0) pc=0x{bpc:x}")), "parked at boundary:\n{out}");
}

/// Parse `"<label> cell 0x…"` out of the guest's own stdout — same convention as
/// `thread_watch_e2e.rs`'s `parse_cell`, retaken independently here so this file does not depend
/// on that one.
fn parse_cell(stdout: &str, label: &str) -> u64 {
    let marker = format!("{label} cell ");
    let start = stdout.find(&marker)
        .unwrap_or_else(|| panic!("missing `{marker}` in stdout:\n{stdout}")) + marker.len();
    let rest = &stdout[start..];
    let hex = rest[..rest.find('\n').unwrap_or(rest.len())].trim();
    u64::from_str_radix(hex.trim_start_matches("0x"), 16)
        .unwrap_or_else(|_| panic!("bad address {hex:?} in stdout:\n{stdout}"))
}

/// Task 8: `watch <addr> thread <n>` is a debugger-side filter, not a hardware one — the hardware
/// slot fires for every thread's store to the watched range, and the debugger discards hits whose
/// thread does not match (CLAUDE.md, "the hardware watchpoint slot stays global"). WATCHTHREAD's
/// `SHARED_CELL` is written by BOTH threads (main first, then the child, once M15's own scheduler
/// switches control at `h.join()`), so scoping is a claim that can be wrong in either direction —
/// a filter that ignores its thread argument, or one that just suppresses everything, would each
/// pass a single-direction check. This test asserts both directions in one run.
#[test]
fn watch_thread_scoping_filters_the_others_write() {
    let (rec, trace) = util::record_dynamic(retrace_guest::WATCHTHREAD);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout).into_owned();
    assert!(out.contains("child wrote"), "the child thread must actually run:\n{out}");
    let shared = parse_cell(&out, "shared");
    assert_eq!(shared % 8, 0, "shared cell {shared:#x} must be 8-byte aligned to watch");

    let ts = trace.to_str().unwrap();

    // Forward: main (thread 0) writes `shared` FIRST; scoping to thread 1 must SKIP that earlier,
    // real hardware hit and keep running until the child's later write — the mid-scan case, not
    // just "the first hit happens to already be the right one".
    let (code1, out1, err1) = debug_run(ts, &format!("watch 0x{shared:x} thread 1; continue; where"));
    assert_eq!(code1, 0, "stderr: {err1}");
    assert!(out1.contains(&format!("hit watch 0x{shared:x}")), "child's write must be reported:\n{out1}");
    let where1 = out1.lines().last().expect("a `where` line");
    assert!(where1.contains("thread=1"), "scoped to thread 1, the reported hit must be the child's:\n{out1}");
    assert!(!where1.contains("thread=0"), "must not report main's (thread 0's) earlier write:\n{out1}");

    // Backward, scoped to the OTHER thread: run to completion, then ask who wrote `shared` scoped
    // to thread 0. The unfiltered answer (and a filter that ignores its argument) is the child's
    // LATER write; the correct, scoped answer is main's EARLIER one — this direction is what
    // actually distinguishes real filtering from no filtering at all.
    let (code2, out2, err2) = debug_run(ts,
        &format!("continue; watch 0x{shared:x} thread 0; reverse-continue; where"));
    assert_eq!(code2, 0, "stderr: {err2}");
    assert!(out2.contains(&format!("hit watch 0x{shared:x}")), "main's write must be found:\n{out2}");
    let where2 = out2.lines().last().expect("a `where` line");
    assert!(where2.contains("thread=0"), "scoped to thread 0, the reported hit must be main's:\n{out2}");
    assert!(!where2.contains("thread=1"), "must not report the child's later write instead:\n{out2}");
}

/// Task 8 fix round 1: re-`watch`ing an ALREADY-armed address used to be a silent no-op — the
/// echo unconditionally printed the just-requested len/thread while the STORED entry (what
/// `arm_watchpoints` and `watch_thread_matches` actually consult) stayed unchanged, so a watch
/// could claim a new scope while the filter kept letting every thread through. Fixed by rejecting
/// the re-arm outright. Two directions: the reject itself must fire loudly (not silently accept a
/// lying echo), and `unwatch`-then-`watch` — the correct way to change a watch — must still make
/// the new scope REAL, proving the fix didn't just start rejecting every re-arm attempt.
#[test]
fn rewatch_without_unwatch_is_rejected_and_unwatch_then_rewatch_applies_the_new_scope() {
    let (rec, trace) = util::record_dynamic(retrace_guest::WATCHTHREAD);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout).into_owned();
    let shared = parse_cell(&out, "shared");
    let ts = trace.to_str().unwrap();

    // Direction 1: re-arming `shared` a second time, without `unwatch`, must be a loud usage
    // error — never a silent state change that leaves the echo lying about the armed scope.
    let (code1, _out1, err1) = debug_run(ts, &format!("watch 0x{shared:x}; watch 0x{shared:x} thread 1"));
    assert_eq!(code1, 5, "re-arming an already-watched address must be a usage error, not a no-op");
    assert!(err1.contains("already watched"), "stderr must name the problem: {err1}");

    // Direction 2: `unwatch` first, THEN re-`watch` with a scope, must make that scope REAL — not
    // just accepted and echoed. Reuses the same discrimination as
    // `watch_thread_scoping_filters_the_others_write`: main (thread 0) writes `shared` first, so
    // a working thread-1 scope must skip that real, earlier hit and land on the child's later one.
    let (code2, out2, err2) = debug_run(ts, &format!(
        "watch 0x{shared:x}; unwatch 0x{shared:x}; watch 0x{shared:x} thread 1; continue; where"));
    assert_eq!(code2, 0, "stderr: {err2}");
    assert!(out2.contains(&format!("watch at 0x{shared:x} len 8 thread 1")), "re-armed echo:\n{out2}");
    let where2 = out2.lines().last().expect("a `where` line");
    assert!(where2.contains("thread=1"), "the re-armed scope must actually filter to thread 1:\n{out2}");
    assert!(!where2.contains("thread=0"), "must not report main's write once re-scoped to thread 1:\n{out2}");
}

#[test]
fn pre_step_boundary_cross_reports_a_watched_syscall_write() {
    // Final-review M-1: park ON the read-svc via a breakpoint (resolves to k = window len),
    // then `watch buf; continue`. The pre-step crosses the boundary by consuming the read
    // event itself — the kernel write to buf must be reported, not silently skipped.
    let (rec, trace) = util::record(retrace_guest::FILEIO);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let (after_read, buf, bpc) = discover_read_cli(tp);
    let svc_pc = bpc - 4; // ELR (return addr) = svc pc + 4 on arm64 syscalls
    let (code, out, err) = debug_run(ts,
        &format!("break 0x{svc_pc:x}; continue; watch 0x{buf:x}; continue; where"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("hit watch 0x{buf:x} (syscall write) at ({after_read}, 0)")),
        "the crossed boundary event's write must be reported:\n{out}");
    // FILEIO is single-threaded throughout, so thread=0 is the only truthful answer (M15).
    assert!(out.trim_end().ends_with(&format!("at ({after_read}, 0) pc=0x{bpc:x} thread=0")),
        "parked at the post-event boundary:\n{out}");
}
