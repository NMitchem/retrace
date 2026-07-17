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
    assert!(out.trim_end().ends_with(&format!("at ({w}, 3) pc=0x{p_mid:x}")), "final where:\n{out}");
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
    // Back-to-back continue on a once-executed bp: second continue pre-steps off the
    // bp and runs to exit — NOT exit 5 (the old documented limitation).
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
    // Boundary bp: hit at (W,0); reverse-stepi off it; continue must pre-step/reland
    // deterministically (the kctx+1 K=0 edge) — no loop, no misresolve.
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

#[test]
fn debug_arg_errors_are_usage_not_panics() {
    for args in [vec!["debug"], vec!["debug", "/nonexistent-but-unread.bin"],
                 vec!["debug", "/tmp/x.bin", "--script"]] {
        let out = std::process::Command::new(util::bin()).args(&args)
            .output().expect("spawn");
        assert_eq!(out.status.code(), Some(2), "usage exit for {args:?}: {}",
            String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stderr).contains("usage"),
            "usage text for {args:?}");
    }
}
