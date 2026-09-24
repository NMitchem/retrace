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
    // Progress rule: the cursor is ON the un-retired store (phase Watch, M41 §3b), so the second
    // continue finishes that coordinate — it executes the store with the watches disarmed — and
    // the scan lands on the NEXT execution of the same store pc — ks[1], not ks[0] again.
    assert!(out.contains(&format!("resolved (1, {})", ks[1])), "second hit advances:\n{out}");
    // WATCHLOOP is single-threaded throughout, so thread=0 is the only truthful answer (M15).
    // M19 appends a symbol annotation to the position line. Strip it and keep `ends_with` — the
    // assertion's content is that the final line IS the `where` output with nothing after the
    // thread id, which `contains` would stop pinning.
    let last = util::strip_annot(out.trim_end().lines().last().unwrap_or(""));
    assert!(last.ends_with(&format!("at (1, {}) pc=0x{spc:x} thread=0", ks[1])), "final where:\n{out}");
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
    // M19 annotation stripped; `ends_with` kept deliberately (see the note on the first such
    // assertion in this file).
    let last = util::strip_annot(out.trim_end().lines().last().unwrap_or(""));
    assert!(last.ends_with(&format!("at (1, {k_last}) pc=0x{spc:x} thread=0")), "final where:\n{out}");
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

/// M40 fix round 1, CRITICAL: `reverse-continue` run FROM the exact coordinate of a syscall-watch
/// hit (P = (after_read, 0), pk == 0) must not re-report that same hit as an earlier one. Before
/// this fix, phase 1's `WatchSyscall` arm recorded `last` unconditionally: the crossing `advance()`
/// that lands the scan cursor exactly ON P (`n == pn`, and a syscall hit's own coordinate is always
/// (n, 0) == (pn, pk)) looked indistinguishable from a hit strictly earlier than P, so the second
/// reverse-continue printed the identical hit line again and the session never moved. The pre-M40
/// loop's own `(n, k) < (pn, pk)` comparison never had this gap; this closes it in the M40 scan.
#[test]
fn reverse_continue_from_the_syscall_hit_itself_finds_nothing_earlier() {
    let (rec, trace) = util::record(retrace_guest::FILEIO);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let (after_read, buf, bpc) = discover_read_cli(tp);
    let hit_line = format!("hit watch 0x{buf:x} (syscall write) at ({after_read}, 0)");
    let (code, out, err) = debug_run(ts,
        &format!("watch 0x{buf:x}; continue; reverse-continue; where"));
    assert_eq!(code, 0, "stderr: {err}");
    // `continue` reports the hit once, landing exactly on it (P = (after_read, 0)); the
    // reverse-continue run from there must NOT find it again as "earlier".
    assert_eq!(out.matches(&hit_line).count(), 1, "forward hit only, not doubled:\n{out}");
    assert!(out.contains("no earlier hit"), "nothing precedes P itself:\n{out}");
    let last = util::strip_annot(out.trim_end().lines().last().unwrap_or(""));
    assert!(last.ends_with(&format!("at ({after_read}, 0) pc=0x{bpc:x} thread=0")),
        "parked unchanged at the boundary:\n{out}");
}

/// The K coordinate of `target_pc` within window `n`: seek to (n, 0) and single-step raw
/// instructions, with NO breakpoints/watchpoints armed, until `pc()` reaches it. Ground truth
/// independent of `reverse-continue`'s own breakpoint/ordinal-resolution machinery — the same
/// seek-and-step pattern `discover_store_ks` uses for watched stores, adapted to a single target
/// pc instead of a memory diff.
fn discover_k_of_pc(trace: &Path, n: usize, target_pc: u64) -> u64 {
    let mut s = retrace_core::seek(trace, n, 0).unwrap();
    let mut k = 0u64;
    while s.pc() != target_pc {
        s.step_insns(1).unwrap();
        k += 1;
    }
    k
}

/// The write()'s svc return address (ELR = svc_pc + 4), found the same way `discover_read_cli`
/// finds the read's: peek the recorded event for SYS_write(4) to fd 1, then cross it.
fn discover_write_ret_pc(trace: &Path) -> u64 {
    let mut s = retrace_core::ReplaySession::open(trace).unwrap();
    loop {
        if let Some((4, args)) = s.peek_syscall() {
            if args[0] == 1 { s.advance().unwrap(); return s.position(); }
        }
        s.advance().unwrap();
    }
}

/// M40 fix round 1, MINOR: a breakpoint set exactly ON a window-ending `svc` (the `Stepped::AtTrap`
/// path in phase 1's `Break` arm), and a SECOND breakpoint hit within the same window (ordinal ≥ 2
/// in `HitKind::Break`'s "search from k=0, count occurrences" contract) — neither was exercised by
/// any committed test before this fix round.
///
/// `read_svc_pc` and `write_svc_pc` are FILEIO's read and write syscalls' own `svc` instructions:
/// each is the LAST instruction of its window (the window-ending trap itself), so a breakpoint
/// there fires pre-retire on the trap — exactly the case `Stepped::AtTrap` exists for. `bpc` (from
/// `discover_read_cli`) is the FIRST instruction (K=0) of the window the read OPENS, which is the
/// SAME window `write_svc_pc` sits in — pairing them puts two breakpoints in one window, so the
/// scan must resolve breakpoint ordinal 2 to land on the later one.
///
/// Every address and coordinate here is discovered (`discover_read_cli`, `discover_write_ret_pc`,
/// `discover_k_of_pc`), never hardcoded.
#[test]
fn reverse_continue_crosses_a_breakpointed_svc_and_resolves_ordinal_two() {
    let (rec, trace) = util::record(retrace_guest::FILEIO);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let (after_read, _buf, bpc) = discover_read_cli(tp);
    let read_win = after_read - 1; // the window the read syscall itself closes
    let read_svc_pc = bpc - 4;     // ELR = svc + 4 on arm64 syscalls
    let write_svc_pc = discover_write_ret_pc(tp) - 4;
    let k_read = discover_k_of_pc(tp, read_win, read_svc_pc);
    let k_write = discover_k_of_pc(tp, after_read, write_svc_pc);

    // Script 1: one breakpoint on EACH window-ending svc. The first reverse-continue crosses the
    // read's own breakpointed trap (AtTrap) to reach the later, write one; the second finds the
    // earlier read hit; the third finds nothing before it.
    let (code1, out1, err1) = debug_run(ts, &format!(
        "continue; break 0x{read_svc_pc:x}; break 0x{write_svc_pc:x}; \
         reverse-continue; where; reverse-continue; where; reverse-continue"));
    assert_eq!(code1, 0, "stderr: {err1}");
    assert!(out1.contains("exited (code 0)"), "continue runs to exit before any breakpoint:\n{out1}");
    assert!(out1.contains(&format!("hit 0x{write_svc_pc:x} at ({after_read}, {k_write})")),
        "first reverse-continue finds the later (write) hit:\n{out1}");
    assert!(out1.contains(&format!("at ({after_read}, {k_write}) pc=0x{write_svc_pc:x}")),
        "parked pc IS the write breakpoint:\n{out1}");
    assert!(out1.contains(&format!("hit 0x{read_svc_pc:x} at ({read_win}, {k_read})")),
        "second reverse-continue finds the earlier (read) hit:\n{out1}");
    assert!(out1.contains(&format!("at ({read_win}, {k_read}) pc=0x{read_svc_pc:x}")),
        "parked pc IS the read breakpoint:\n{out1}");
    assert!(out1.trim_end().ends_with("no earlier hit"),
        "third reverse-continue: nothing precedes the read hit:\n{out1}");

    // Script 2: the SAME write-svc breakpoint, paired with `bpc` (K=0 of the window the read
    // OPENS) instead of the read's own svc — both breakpoints now sit inside window `after_read`,
    // so the first reverse-continue must resolve breakpoint ORDINAL 2 (past the K=0 hit) to land
    // on the write svc, at the identical coordinate script 1 found by a different route.
    let (code2, out2, err2) = debug_run(ts, &format!(
        "continue; break 0x{bpc:x}; break 0x{write_svc_pc:x}; \
         reverse-continue; where; reverse-continue; where; reverse-continue"));
    assert_eq!(code2, 0, "stderr: {err2}");
    assert!(out2.contains(&format!("hit 0x{write_svc_pc:x} at ({after_read}, {k_write})")),
        "ordinal 2 resolves to the write svc, same coordinate as script 1:\n{out2}");
    assert!(out2.contains(&format!("at ({after_read}, {k_write}) pc=0x{write_svc_pc:x}")),
        "parked pc IS the write breakpoint:\n{out2}");
    assert!(out2.contains(&format!("hit 0x{bpc:x} at ({after_read}, 0)")),
        "second reverse-continue finds the K=0 hit, ordinal 1:\n{out2}");
    assert!(out2.contains(&format!("at ({after_read}, 0) pc=0x{bpc:x}")),
        "parked pc IS the K=0 breakpoint:\n{out2}");
    assert!(out2.trim_end().ends_with("no earlier hit"),
        "third reverse-continue: nothing precedes the K=0 hit:\n{out2}");
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
    // then `watch buf; continue`. The finish (M41 §3c; this test's name keeps the pre-M41 word
    // "pre-step") crosses the boundary by consuming the read event itself, with the watches
    // armed for that one event — the kernel write to buf must be reported, not silently skipped.
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
    // M19 annotation stripped; `ends_with` kept deliberately (see the note on the first such
    // assertion in this file).
    let last = util::strip_annot(out.trim_end().lines().last().unwrap_or(""));
    assert!(last.ends_with(&format!("at ({after_read}, 0) pc=0x{bpc:x} thread=0")),
        "parked at the post-event boundary:\n{out}");
}
