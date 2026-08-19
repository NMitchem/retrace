// THE M15 HEADLINE GATE. The child writes the watched cell; main writes a different one.
//
// "the watch fired" is NOT the assertion — it fires correctly today with none of M15 present. The
// milestone's whole contribution is WHICH THREAD is named, so that is what this gate can fail on.
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

/// Parse `"<label> cell 0x…"` out of the guest's own stdout. A `static mut`'s address crosses no
/// syscall boundary (nothing the kernel sees ever carries it), so unlike M13's `protnone_rust_e2e`
/// (which learns its page from a recorded `mprotect` argument) there is no syscall to learn this
/// from — the guest prints it instead, which is still its own recorded behaviour, just on stdout
/// rather than in a syscall arg.
fn parse_cell(stdout: &str, label: &str) -> u64 {
    let marker = format!("{label} cell ");
    let start = stdout.find(&marker)
        .unwrap_or_else(|| panic!("missing `{marker}` in stdout:\n{stdout}")) + marker.len();
    let rest = &stdout[start..];
    let hex = rest[..rest.find('\n').unwrap_or(rest.len())].trim();
    u64::from_str_radix(hex.trim_start_matches("0x"), 16)
        .unwrap_or_else(|_| panic!("bad address {hex:?} in stdout:\n{stdout}"))
}

/// The landmark right after `bsdthread_create` returns to the thread THAT CALLED IT (thread 0): the
/// scheduler switches only when a thread blocks or exits (CLAUDE.md, "Guest threads"), so this
/// window is still owned by thread 0 even though `thread_summaries()` already lists thread 1 —
/// exactly the window where `regs 1` exercises `dbg_regs_of`'s non-current-thread path. Same
/// measurement as `debug_cli.rs`'s `discover_after_create`, independently retaken here on
/// WATCHTHREAD rather than shared, so this gate does not depend on that file. Returns
/// (N, boundary_pc).
fn discover_after_create(trace: &Path) -> (usize, u64) {
    let mut s = retrace_core::ReplaySession::open(trace).unwrap();
    loop {
        if let Some((num, _)) = s.peek_syscall() {
            if num == retrace_arch::SYS_BSDTHREAD_CREATE {
                s.advance().unwrap();
                return (s.landmark(), s.position());
            }
        }
        s.advance().unwrap();
    }
}

#[test]
fn reverse_continue_names_the_thread_that_wrote_the_watched_cell() {
    let (rec, trace) = util::record_dynamic(retrace_guest::WATCHTHREAD);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout).into_owned();
    assert!(out.contains("child wrote"),
        "THE CHILD THREAD MUST ACTUALLY RUN. stdout:\n{out}");

    let child = parse_cell(&out, "child");
    let main = parse_cell(&out, "main");
    assert_ne!(child, main,
        "the two cells must be DIFFERENT addresses — otherwise a watch on one can't be wrong about \
         which thread wrote it, and the gate would prove nothing; stdout:\n{out}");
    assert_eq!(child % 8, 0, "child cell {child:#x} must be 8-byte aligned to watch");

    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();

    // --- Part 1: `regs <child>` reaches a thread that is NOT current at the moment it's asked. --
    // Ground truth taken directly off the SAME accessors the CLI wraps, at the SAME coordinate the
    // script below parks at — independent of the CLI's own rendering (mirrors `debug_cli.rs`'s
    // `threads_lists_every_thread_and_regs_of_reaches_a_non_current_one`, re-measured on
    // WATCHTHREAD).
    let (n0, bpc) = discover_after_create(tp);
    let (summaries, expected_regs1) = {
        let g = retrace_core::seek(tp, n0, 0).unwrap();
        (g.thread_summaries(), g.dbg_regs_of(1).unwrap())
    };
    assert_eq!(summaries.len(), 2, "bsdthread_create must have added exactly one thread: {summaries:?}");
    assert!(summaries[0].tid == 0 && summaries[0].is_current,
        "thread 0 is still current here: the creator has not blocked yet: {summaries:?}");
    assert!(summaries[1].tid == 1 && !summaries[1].is_current,
        "thread 1 exists but is not yet current here: {summaries:?}");

    let (code1, out1, err1) = debug_run(ts, &format!("break 0x{bpc:x}; continue; threads; regs 1"));
    assert_eq!(code1, 0, "stderr: {err1}");
    assert!(out1.contains("* thread 0: Runnable"), "thread 0 marked current:\n{out1}");
    assert!(!out1.contains("* thread 1"), "thread 1 must not be marked current here:\n{out1}");
    assert!(out1.contains(&expected_regs1),
        "`regs 1` must dump thread 1's OWN registers while thread 0 is current:\n{out1}");

    // --- Part 2: THE HEADLINE. Run to completion, THEN watch the child's cell and reverse-continue
    // back to the store that hit it — checking WHICH thread reverse-continue names. -------------
    // The watch is armed only AFTER the run completes, so `reverse-continue`'s backward scan (not
    // a forward `continue`, which would trivially land on the same hit while running forward) is
    // what has to find and resolve it.
    let (code2, out2, err2) = debug_run(ts,
        &format!("continue; watch 0x{child:x}; reverse-continue; where"));
    assert_eq!(code2, 0, "stderr: {err2}");
    assert!(out2.contains(&format!("hit watch 0x{child:x}")),
        "reverse-continue must find the child's store on the watched cell:\n{out2}");

    // The claim this whole milestone exists to make. "The watch fired" is NOT it — a watch on
    // `child` fires identically with none of M15 present (Task 5's `hw_watch_hit_names_the_writing_
    // thread` pins that this field's plumbing already existed). What's new is that the NAME is
    // right: main writes `main`, never `child` — a scheduler-default or hardcoded thread=0 would
    // misattribute this store, and that is exactly the bug this assertion catches.
    //
    // Scope, in three layers, so a later reader doesn't credit this one assertion with all of them:
    // (1) Task 4's divergence oracle is the FIRST line of defence — it catches a broken
    // `current_thread()` at the next syscall landmark, before this assertion ever runs (confirmed in
    // the fix-round report: mutating `current_thread()` itself is caught there, not here).
    // (2) THIS gate proves the display path end-to-end: `where` reports the box's real
    // `current_thread()` at a *resolved* coordinate rather than a constant — the thing a user
    // actually reads, and a substantive check, but only of display, not a standalone scheduler-bug
    // catch. (3) `Advance::Watch { thread }` — the field the hardware hit itself carries — is
    // exercised by Task 8's `watch_thread_scoping_filters_the_others_write` (`watch_cli.rs`), not
    // here: this script never scopes a watch to a thread.
    let where_line = out2.lines().last().expect("a `where` line");
    // `ends_with`, not `contains`: `cmd_where`'s format (`"…thread={}"`) puts nothing after the
    // thread number, so "thread=1" can only ever be a suffix — `contains` would also pass a
    // hypothetical "thread=10".
    assert!(where_line.ends_with("thread=1"),
        "`where` after reverse-continue must name thread 1 — the CHILD actually wrote the watched \
         cell, main never touches it. got:\n{where_line}\nfull transcript:\n{out2}");
    assert!(!where_line.contains("thread=0"),
        "must not misattribute the child's store to thread 0 (main's own cell is a different \
         address and was never watched):\n{out2}");
}
