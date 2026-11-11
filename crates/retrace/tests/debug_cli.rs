// Golden-transcript tests for the debug executor's previously-untested paths.
// Each fn records ONE fresh trace and reuses it for all its CLI spawns.
//
// Absolute pc/coordinate values are DISCOVERED from each freshly-recorded trace, never hardcoded:
// record-dyn is not landmark-count-deterministic across separate runs (the write's landmark index
// `w` varies run to run), so every assert interpolates the discovered `w`. The guest pc values
// (`bpc`/`p_mid`/`p5`) and the window length happen to be stable across records — they are still
// discovered here so the harness stays honest and Task 2 can reuse the helpers verbatim.
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

/// Find the guest's `write(1, …)` and return `(W, boundary_pc)`: `W` is the landmark of the window
/// that begins when `write` returns, and `boundary_pc` is that window's first pc (the syscall return
/// address, in libsystem's `__write` stub). The session is dropped on return (one VM per process).
fn discover_write(trace: &Path) -> (usize, u64) {
    let mut s = retrace_core::ReplaySession::open(trace).unwrap();
    loop {
        if let Some((4, args)) = s.peek_syscall() {
            // position() = ELR = the write's return address (the boundary pc at K=0).
            if args[0] == 1 { s.advance().unwrap(); return (s.landmark(), s.position()); }
        }
        s.advance().unwrap();
    }
}

/// The live pc at coordinate `(w, 3)` — a mid-window pc three instructions into `write`'s return
/// window (`__write` has returned into guest code by then). The session is dropped on return.
fn discover_mid(trace: &Path, w: usize) -> u64 {
    let mut s = retrace_core::seek(trace, w, 0).unwrap();
    s.step_insns(3).unwrap();
    s.pc() // live reg PC at (w, 3)
}

#[test]
fn continue_hits_mid_window_and_reverse_continue_returns() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let (w, _bpc) = discover_write(tp);
    let p_mid = discover_mid(tp, w);
    let p5 = { let s = retrace_core::seek(tp, w, 5).unwrap(); s.pc() }; // pc at (w, 5)
    // (all discovery sessions dropped before spawning — one VM per process)

    // Mid-window HW-BVR hit: the continue catches p_mid via a hardware breakpoint (Advance::Break),
    // reports the unresolved coordinate, then K-resolves it to the exact (W, 3). reverse-continue
    // from (W, 5) walks back to that strictly-earlier hit.
    let (code, out, err) = debug_run(ts,
        &format!("break 0x{p_mid:x}; continue; where; stepi 2; where; reverse-continue; where"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("breakpoint at 0x{p_mid:x}")), "break echo:\n{out}");
    assert!(out.contains(&format!("hit 0x{p_mid:x} at ({w}, +?)")), "mid-window hit line:\n{out}");
    assert!(out.contains(&format!("resolved ({w}, 3)")), "K-resolution:\n{out}");
    assert!(out.contains(&format!("at ({w}, 3) pc=0x{p_mid:x}")), "where after hit:\n{out}");
    assert!(out.contains(&format!("at ({w}, 5) pc=0x{p5:x}")), "where after stepi 2:\n{out}");
    // reverse-continue from (W,5): the (W,3) hit is strictly earlier -> returns to it.
    assert!(out.contains(&format!("hit 0x{p_mid:x} at ({w}, 3)")), "reverse-continue hit:\n{out}");
    // HELLO_DYN is single-threaded throughout, so thread=0 is the only truthful answer (M15).
    //
    // M19 appends a symbol annotation to pc-bearing lines ("… thread=0  in _main+0x30"). Strip it
    // and keep `ends_with`: what the assertion protects is that the FINAL line of the transcript is
    // this `where` output — reverse-continue returned to (W,3) and nothing followed it — and only
    // "nothing follows" pins the second half. The annotation itself is asserted in `symbols_e2e`,
    // which owns it.
    let last = util::strip_annot(out.trim_end().lines().last().unwrap_or(""));
    assert!(last.ends_with(&format!("at ({w}, 3) pc=0x{p_mid:x} thread=0")), "final where:\n{out}");
}

#[test]
fn stepi_window_end_and_examine_errors_are_clean() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let (w, bpc) = discover_write(tp);
    // Window W's length (from K=0) and the 16 code bytes at bpc, both off one probe session.
    let (len, row) = {
        let mut s = retrace_core::seek(tp, w, 0).unwrap();
        let bytes = s.read_mem(bpc, 16).expect("bpc mapped at (w, 0)");
        let hex = bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
        let len = s.window_len_here().unwrap();
        (len, format!("0x{bpc:x}: {hex}"))
    };

    // Navigate to (W, 0) via a breakpoint on the window's first pc (a landmark-boundary hit), then
    // stepi one past the window end: the error names the REMAINING count (== len, since K == 0) and
    // the position is unchanged. Then x on an unmapped VA prints `unmapped` (no address prefix), and
    // x on the mapped pc prints the byte row.
    let (code, out, err) = debug_run(ts, &format!(
        "break 0x{bpc:x}; continue; stepi {}; where; x 0xdead00000000 16; x 0x{bpc:x} 16",
        len + 1));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("hit 0x{bpc:x} at ({w}, 0)")), "navigate to (W,0):\n{out}");
    // Pin the FULL printed error line exactly. `step_insns`'s raw Err carries a "; cannot step {k}"
    // tail (k = the requested count = len+1) that the executor strips before printing; assert the
    // exact head line AND that the tail is absent, so a regression that leaks the tail fails here.
    let want_err = format!("error: window {w} ends after {len} instruction(s)");
    assert!(out.lines().any(|l| l == want_err), "exact window-end error line ({want_err}):\n{out}");
    assert!(!out.contains("; cannot step"), "the '; cannot step {}' tail must be stripped:\n{out}", len + 1);
    assert!(out.contains(&format!("at ({w}, 0) pc=0x{bpc:x}")), "position unchanged after error:\n{out}");
    assert!(out.contains("unmapped"), "x on unmapped VA:\n{out}");
    assert!(!out.contains("0xdead00000000:"), "unmapped prints no address prefix:\n{out}");
    assert!(out.contains(&row), "x on mapped pc prints its byte row ({row}):\n{out}");

    // x len-cap: parse error -> whole script rejected before any output, exit 5.
    let (code2, out2, err2) = debug_run(ts, "x 0x1000 65537");
    assert_eq!(code2, 5, "len over cap is a parse error");
    assert!(out2.is_empty(), "no stdout on a parse error:\n{out2}");
    assert!(err2.contains("DEBUG ERROR: length 65537 exceeds max 65536"), "stderr: {err2}");
}

#[test]
fn delete_disarms_breakpoint() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let (_w, bpc) = discover_write(tp);
    let (code, out, err) = debug_run(ts,
        &format!("break 0x{bpc:x}; delete 0x{bpc:x}; continue"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("breakpoint at 0x{bpc:x}")), "break echo:\n{out}");
    assert!(out.contains(&format!("deleted 0x{bpc:x}")), "delete echo:\n{out}");
    assert!(out.contains("exited (code 0)"), "runs to exit without hitting:\n{out}");
    assert!(!out.contains("hit 0x"), "no hit after delete:\n{out}");
}

#[test]
fn break_beyond_six_fails_loud() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0);
    let script = (0..7).map(|i| format!("break 0x{:x}", 0x10000 + i * 4))
        .collect::<Vec<_>>().join("; ");
    let (code, _, err) = debug_run(trace.to_str().unwrap(), &script);
    assert_eq!(code, 5, "7th break must be a loud error, not silent .take(6)");
    assert!(err.contains("6"), "error names the hardware limit: {err}");
}

#[test]
fn continue_from_a_breakpoint_steps_over_it() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let (w, _) = discover_write(tp);
    let p_mid = discover_mid(tp, w);
    // Back-to-back continue on a once-executed bp: the second continue finishes the coordinate
    // it stands on (M41 §3c: it executes the breakpointed instruction, then scans) and runs to
    // exit — NOT exit 5 (the old documented limitation).
    let (code, out, err) = debug_run(trace.to_str().unwrap(),
        &format!("break 0x{p_mid:x}; continue; where; continue; where"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("resolved ({w}, 3)")), "first hit:\n{out}");
    assert!(out.contains("exited (code 0)"), "second continue exits cleanly:\n{out}");
}

#[test]
fn continue_after_reverse_stepi_onto_boundary_bp() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let (w, bpc) = discover_write(tp);
    // Boundary bp: hit at (W,0); reverse-stepi off it; continue must reland deterministically
    // — no loop, no misresolve. Since M41 the reverse-stepi is an arrival at the window-ending
    // svc (cursor phase Bp, and the svc is not a breakpoint), so there is nothing to finish: the
    // scan consumes the trap and the `Event` arm's boundary check reports (W, 0).
    let (code, out, err) = debug_run(trace.to_str().unwrap(),
        &format!("break 0x{bpc:x}; continue; where; reverse-stepi; where; continue; where"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("hit 0x{bpc:x} at ({w}, 0)")), "boundary hit:\n{out}");
    // After reverse-stepi to (W-1, L), continue re-approaches: the boundary check fires
    // again at (W, 0) — both hits must be the exact boundary form, never a mid-window resolve.
    let hits = out.matches(&format!("hit 0x{bpc:x} at ({w}, 0)")).count();
    assert_eq!(hits, 2, "exactly two clean boundary-form hits, no loop:\n{out}");
    assert!(!out.contains("+?"), "no mid-window form in this transcript:\n{out}");
}

/// The landmark right after `bsdthread_create` returns to the thread THAT CALLED IT. The scheduler
/// switches only when a thread blocks or exits (CLAUDE.md, "Guest threads"), and the creator has not
/// blocked yet at this point, so this window is still owned by thread 0 even though
/// `thread_summaries()` already lists thread 1 — exactly the window where `regs 1` exercises
/// `dbg_regs_of`'s non-current-thread path. Returns (N, boundary_pc).
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

/// The `which`-th (0-indexed) `write(1, …)` in the trace: `(N, boundary_pc, thread)`, where `N` is
/// the landmark of the window right after the write returns and `thread` is that window's owning
/// thread — read straight off the box's own scheduler (`current_thread`), the same accessor `where`
/// and `threads` report from. THREADRUST prints three lines from two threads ("main before spawn"
/// [0], "child ran" [1], "joined 42" [0]); `which=1` is the only one NOT issued by thread 0.
fn discover_write_n(trace: &Path, which: usize) -> (usize, u64, u32) {
    let mut s = retrace_core::ReplaySession::open(trace).unwrap();
    let mut seen = 0;
    loop {
        if let Some((4, args)) = s.peek_syscall() {
            if args[0] == 1 {
                if seen == which {
                    s.advance().unwrap();
                    return (s.landmark(), s.position(), s.current_thread());
                }
                seen += 1;
            }
        }
        s.advance().unwrap();
    }
}

#[test]
fn threads_lists_every_thread_and_regs_of_reaches_a_non_current_one() {
    let (rec, trace) = util::record_dynamic(retrace_guest::THREADRUST);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert!(String::from_utf8_lossy(&rec.stdout).contains("child ran"),
        "the child thread must actually run for thread 1 to exist");
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let (n, bpc) = discover_after_create(tp);

    // Ground truth taken directly off the SAME accessors the CLI wraps, at the SAME coordinate the
    // script below parks at — independent of the CLI's own rendering, so a `cmd_regs_of` that ignores
    // its argument (e.g. always dumping the current thread) fails this, not just a weaker "some
    // output appeared" check.
    let (summaries, expected_regs0, expected_regs1) = {
        let g = retrace_core::seek(tp, n, 0).unwrap();
        (g.thread_summaries(), g.dbg_regs_of(0).unwrap(), g.dbg_regs_of(1).unwrap())
    };
    assert_eq!(summaries.len(), 2, "bsdthread_create must have added exactly one thread: {summaries:?}");
    assert!(summaries[0].tid == 0 && summaries[0].is_current,
        "thread 0 is still current here: the creator has not blocked yet: {summaries:?}");
    assert!(summaries[1].tid == 1 && !summaries[1].is_current,
        "thread 1 exists but is not yet current here: {summaries:?}");
    assert_ne!(expected_regs0, expected_regs1, "the two threads must have distinct register state");

    let (code, out, err) = debug_run(ts, &format!("break 0x{bpc:x}; continue; threads; regs 1; regs"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains("* thread 0: Runnable"), "thread 0 marked current:\n{out}");
    assert!(out.contains("  thread 1: Runnable"), "thread 1 listed, not marked current:\n{out}");
    assert!(!out.contains("* thread 1"), "thread 1 must not be marked current here:\n{out}");
    assert!(out.contains(&expected_regs1), "`regs 1` must print thread 1's OWN registers:\n{out}");
    assert!(out.contains(&expected_regs0), "`regs` (no arg) must still dump the current thread:\n{out}");
}

#[test]
fn where_and_threads_name_the_thread_after_a_switch() {
    let (rec, trace) = util::record_dynamic(retrace_guest::THREADRUST);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let (n, bpc, tid) = discover_write_n(tp, 1); // "child ran": the child's own write
    assert_eq!(tid, 1, "the child's write must be issued by thread 1, not the scheduler default 0");

    // All three writes return through the SAME libSystem `__write` stub pc, so one `continue` lands
    // on the FIRST occurrence ("main before spawn"); a second `continue` steps over it (the
    // established back-to-back-continue behavior — see `continue_from_a_breakpoint_steps_over_it`)
    // and re-arms the same breakpoint, landing on the SECOND occurrence, which is `which=1`'s write.
    let (code, out, err) = debug_run(ts, &format!("break 0x{bpc:x}; continue; continue; where; threads"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("at ({n}, 0) pc=0x{bpc:x} thread=1")),
        "`where` must name the SWITCHED-TO thread, not always 0:\n{out}");
    assert!(out.contains("* thread 1: Runnable"), "thread 1 marked current after the switch:\n{out}");
    assert!(!out.contains("* thread 0:"), "thread 0 must no longer be marked current:\n{out}");
}

#[test]
fn regs_of_out_of_range_thread_is_a_usage_error_not_a_panic() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let (code, out, err) = debug_run(trace.to_str().unwrap(), "regs 99");
    assert_eq!(code, 5, "an out-of-range thread id is a controlled error, not a crash; stderr: {err}");
    assert!(err.contains("no such thread: 99"), "stderr names the bad id: {err}");
    assert!(!out.contains("x0="), "no register dump on a rejected thread id:\n{out}");
}

#[test]
fn debug_arg_errors_are_usage_not_panics() {
    for args in [vec!["debug"], vec!["debug", "/tmp/x.bin", "--script"]] {
        let out = std::process::Command::new(util::bin()).args(&args)
            .output().expect("spawn");
        assert_eq!(out.status.code(), Some(2), "usage exit for {args:?}: {}",
            String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stderr).contains("usage"),
            "usage text for {args:?}");
    }
}

#[test]
fn an_unknown_debug_argument_is_bad_input() {
    // A misspelt flag (`--jsno`) used to be ignored, giving a text summary where JSON was asked for. It is bad input:
    // one `retrace:` line naming it, exit 2, before the trace is read (this one does not exist).
    for args in [["debug", "/nonexistent-but-unread.bin", "--jsno"], ["debug", "--jsno", "/nonexistent-but-unread.bin"]] {
        let out = std::process::Command::new(util::bin()).args(args).output().expect("spawn");
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(2), "{args:?}: {err}");
        let lines: Vec<&str> = err.lines().collect();
        assert_eq!(lines.len(), 1, "{args:?}: one line:\n{err}");
        assert!(lines[0].starts_with("retrace: ") && lines[0].contains("--jsno") && !lines[0].contains("cannot read"), "{args:?}: {err}");
        assert!(out.stdout.is_empty(), "{args:?}: nothing on stdout");
    }
}

#[test]
fn debug_of_a_missing_trace_is_bad_input() {
    // With no script `debug <trace>` runs `summary`; a trace that cannot be read is bad input, before any VM work.
    let out = std::process::Command::new(util::bin()).args(["debug", "/nonexistent-but-unread.bin"]).output().expect("spawn");
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot read"), "{}", String::from_utf8_lossy(&out.stderr));
}

fn json_lines(s: &str) -> Vec<serde_json::Value> {
    s.lines().map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}"))).collect()
}

#[test]
fn json_mode_one_object_per_command() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "record: {}", rec.stderr);
    let ts = trace.to_str().unwrap();
    let script = "where; break _main; continue; where; regs";
    let out = std::process::Command::new(util::bin())
        .args(["debug", ts, "--json", "--script", script]).output().expect("spawn");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let objs = json_lines(&String::from_utf8(out.stdout).unwrap());
    assert_eq!(objs.len(), 5);
    for o in &objs { assert_eq!(o["ok"], true, "{o}"); }
    let (code, text, err) = debug_run(ts, script);
    assert_eq!(code, 0, "{err}");
    let where_line = text.lines().rfind(|l| l.starts_with("at (")).unwrap();
    assert_eq!(objs[3]["output"], serde_json::json!([where_line]));
    let pos = objs[3]["position"].as_array().unwrap();
    assert!(pos.len() == 2 && pos.iter().all(|v| v.is_u64()));
    assert!(objs[3]["pc"].as_str().unwrap().starts_with("0x"));
}

#[test]
fn json_mode_error_is_a_json_line_and_exit_5() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "record: {}", rec.stderr);
    let out = std::process::Command::new(util::bin())
        .args(["debug", trace.to_str().unwrap(), "--script", "break nosuchsym", "--json"]).output().expect("spawn");
    assert_eq!(out.status.code(), Some(5));
    let objs = json_lines(&String::from_utf8(out.stdout).unwrap());
    assert_eq!(objs.len(), 1);
    assert_eq!(objs[0]["ok"], false);
    assert!(objs[0]["error"].is_string());
    assert!(String::from_utf8_lossy(&out.stderr).contains("DEBUG ERROR:"));
}

#[test]
fn json_mode_escapes_program_text() {
    // The shell's printf writes a quote, a backslash and a control byte to stderr; `summary --json` must carry them
    // as one valid line and give them back unchanged.
    let want = "q\"b\\c\u{1}d";
    let (rec, trace) = util::record_dynamic_args("/bin/sh", &["-c", "printf 'q\"b\\\\c\\001d\\n' >&2"]);
    assert_eq!(rec.code, 0, "record: {}", rec.stderr);
    let out = std::process::Command::new(util::bin())
        .args(["debug", trace.to_str().unwrap(), "--json"]).output().expect("spawn");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let objs = json_lines(&String::from_utf8(out.stdout).unwrap());
    assert_eq!(objs.len(), 1);
    let tail = objs[0]["stderr_tail"].as_str().expect("stderr_tail");
    assert!(tail.contains(want), "the program's characters survive the round trip: {tail:?}");
    // The text summary shows the same stderr under its header.
    let text = std::process::Command::new(util::bin()).args(["debug", trace.to_str().unwrap()]).output().expect("spawn");
    assert_eq!(text.status.code(), Some(0));
    let text = String::from_utf8_lossy(&text.stdout).into_owned();
    assert!(text.contains("stderr (last") && text.contains(want), "{text}");
}

#[test]
fn json_mode_incomplete_recording_is_a_json_line_and_exit_5() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "record: {}", rec.stderr);
    // The same recording without its terminal event: incomplete.
    let mut ev = retrace_trace::Reader::open(&trace).unwrap();
    let end = ev.iter().position(|e| matches!(e, retrace_trace::Event::Crash { .. } | retrace_trace::Event::Exit { .. })).expect("a terminal event");
    ev.truncate(end);
    let cut = trace.with_extension("cut.bin");
    let mut w = retrace_trace::Writer::create(&cut).unwrap();
    for e in &ev { w.append(e).unwrap(); }
    drop(w);
    let out = std::process::Command::new(util::bin())
        .args(["debug", cut.to_str().unwrap(), "--json", "--script", "where"]).output().expect("spawn");
    assert_eq!(out.status.code(), Some(5), "{}", String::from_utf8_lossy(&out.stderr));
    let objs = json_lines(&String::from_utf8(out.stdout).unwrap());
    assert_eq!(objs.len(), 1);
    assert_eq!(objs[0]["ok"], false);
    assert!(objs[0]["command"].is_null());
    assert!(objs[0]["error"].as_str().unwrap().contains("incomplete recording"), "{}", objs[0]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("DEBUG ERROR:"));
}

/// M58 B2: a Rust name breaks by its readable form, without its hash, and prints readable.
#[test]
fn break_by_readable_rust_name() {
    if util::cargo::cargo_path().is_none() {
        util::announce("SKIPPED: cargo is not runnable (break_by_readable_rust_name)");
        return;
    }
    let dir = util::cargo::fresh_dir("readable");
    let b = util::cargo::cargo_test_under_retrace(util::cargo::LEDGER_MANIFEST, &dir,
        &["--no-fail-fast", "settling_four_hundred_leaves_the_rest"]);
    assert_ne!(b.code, Some(0), "the failing test must fail: {}", b.stderr);
    let trace = std::fs::read_dir(&dir).expect("recordings dir").filter_map(|e| e.ok()).map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "retrace")).expect("a recording");
    let t = trace.to_str().unwrap();

    let (code, out, err) = debug_run(t, "break rust_panic; continue; where");
    assert_eq!(code, 0, "{err}\n{out}");
    assert!(out.contains("(__rustc::rust_panic)"), "{out}");
    assert!(out.contains("breakpoint at 0x"), "{out}");
    assert!(out.contains("hit 0x"), "{out}");
    assert!(out.contains("in __rustc::rust_panic"), "{out}");

    let (code, out, err) = debug_run(t, "break ledger::tests::settling_four_hundred_leaves_the_rest");
    assert_eq!(code, 0, "{err}\n{out}");
    assert!(out.contains("(ledger::tests::settling_four_hundred_leaves_the_rest)"), "{out}");
    // The suffix form finds the same function.
    let (_, out2, _) = debug_run(t, "break tests::settling_four_hundred_leaves_the_rest");
    let bp = |s: &str| s.lines().find(|l| l.starts_with("breakpoint at")).map(str::to_string);
    assert!(bp(&out).is_some());
    assert_eq!(bp(&out), bp(&out2));
    // And no mangled text leaks into either line.
    assert!(!out.contains("__ZN") && !out.contains("17h"), "{out}");
}

/// M58 B3: a stop in the program's own code has a source file and line, from `atos` over the executable on disk.
#[test]
fn json_where_has_file_and_line_in_the_program() {
    if util::cargo::cargo_path().is_none() || !std::path::Path::new("/usr/bin/atos").exists() {
        util::announce("SKIPPED: cargo or atos is not runnable (json_where_has_file_and_line_in_the_program)");
        return;
    }
    let dir = util::cargo::fresh_dir("fileline");
    let b = util::cargo::cargo_test_under_retrace(util::cargo::LEDGER_MANIFEST, &dir,
        &["--no-fail-fast", "settling_four_hundred_leaves_the_rest"]);
    assert_ne!(b.code, Some(0), "the failing test must fail: {}", b.stderr);
    let trace = std::fs::read_dir(&dir).expect("recordings dir").filter_map(|e| e.ok()).map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "retrace")).expect("a recording");
    let out = std::process::Command::new(util::bin())
        .args(["debug", trace.to_str().unwrap(), "--json", "--script",
               "break ledger::tests::settling_four_hundred_leaves_the_rest; continue; where"]).output().expect("spawn");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let objs = json_lines(&String::from_utf8(out.stdout).unwrap());
    let last = objs.last().unwrap();
    assert!(last["file"].as_str().is_some_and(|f| f.starts_with('/') && f.ends_with("src/lib.rs")), "{last}");
    assert!(last["line"].as_u64().is_some_and(|l| l > 0), "{last}");
    assert!(last.get("source_note").is_none(), "{last}");

    // Ruling P12: a stop in Rust's std has no line, and the note never claims the binary lacks debug info.
    let out = std::process::Command::new(util::bin())
        .args(["debug", trace.to_str().unwrap(), "--json", "--script", "break rust_panic; continue; where"]).output().expect("spawn");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let objs = json_lines(&String::from_utf8(out.stdout).unwrap());
    let last = objs.last().unwrap();
    assert!(last["file"].is_null() && last["line"].is_null(), "{last}");
    assert_eq!(last["source_note"], "no line information at this address", "{last}");
}

/// M58 B3: a program rebuilt since the recording is not trusted: no file, no line, and the note says why.
#[test]
fn a_rebuilt_binary_gets_no_lines_and_says_why() {
    let d = std::env::temp_dir().join(format!("retrace-b3-rebuilt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::copy(retrace_guest::CRASHY, d.join("crashy")).unwrap();
    let r = util::run_in(&d, &["record", "./crashy"], &[], 120);
    assert_eq!(r.code, Some(139), "record crashy: {}", r.stderr);
    std::fs::copy(retrace_guest::HELLO_DYN, d.join("crashy")).unwrap();
    let out = std::process::Command::new(util::bin()).current_dir(&d)
        .args(["debug", "crashy.retrace", "--json", "--script", "break _main; continue; where"]).output().expect("spawn");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let objs = json_lines(&String::from_utf8(out.stdout).unwrap());
    let last = objs.last().unwrap();
    assert!(last["file"].is_null() && last["line"].is_null(), "{last}");
    assert!(last["source_note"].as_str().is_some_and(|n| n.contains("changed since the recording")), "{last}");
}

/// M58 final review: `summary` and `last-write` set the user's breakpoints and watches aside and put them back. After
/// either, from the start again, `continue` stops at the user's breakpoint and then at the user's watch.
#[test]
fn user_breakpoints_and_watches_survive_summary_and_last_write() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "record: {}", rec.stderr);
    let (st, ptr) = util::discover_crashy_addrs(&trace);
    let ts = trace.to_str().unwrap();
    // The two continues after `goto 1 0`, in order: the breakpoint at main's entry, then main's store to `g.ptr`.
    let after_goto = |out: &str| -> Vec<String> {
        let (_, tail) = out.split_once("> goto 1 0").unwrap_or_else(|| panic!("no goto:\n{out}"));
        tail.lines().filter(|l| l.starts_with("hit ")).map(str::to_string).collect()
    };
    let check = |what: &str, out: &str| {
        let hits = after_goto(out);
        assert!(hits.len() >= 2, "{what}: two hits after the goto:\n{out}");
        assert!(hits[0].starts_with("hit 0x") && hits[0].contains(" in _main"), "{what}: the user's breakpoint first:\n{out}");
        assert!(hits[1].starts_with(&format!("hit watch {ptr:#x} ")), "{what}: then the user's watch:\n{out}");
    };
    let (code, out, err) = debug_run(ts, &format!("break _main; watch {ptr:#x}; summary; goto 1 0; continue; continue"));
    assert_eq!(code, 0, "{err}\n{out}");
    assert!(out.contains("ended: killed by SIGSEGV"), "the summary ran:\n{out}");
    check("summary", &out);
    // `last-write` of `g.st`, which `fstat` wrote, from the crash: it finds the write, then the user's stops are back.
    let (code, out, err) = debug_run(ts, &format!(
        "break _main; watch {ptr:#x}; continue; continue; continue; last-write {st:#x}; goto 1 0; continue; continue"));
    assert_eq!(code, 0, "{err}\n{out}");
    assert!(out.contains(&format!("last write to {st:#x}: syscall ")), "the last-write found fstat's write:\n{out}");
    check("last-write", &out);
}
