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
            if args[0] == 1 { s.advance().unwrap(); return (s.landmark(), s.pc()); }
        }
        s.advance().unwrap();
    }
}

/// The live pc at coordinate `(w, 3)` — a mid-window pc three instructions into `write`'s return
/// window (`__write` has returned into guest code by then). The session is dropped on return.
fn discover_mid(trace: &Path, w: usize) -> u64 {
    let mut s = retrace_core::seek(trace, w, 0).unwrap();
    s.step_insns(3).unwrap();
    s.cur_pc() // Task 2 renames this to pc(); Task 2 updates this call site.
}

#[test]
fn continue_hits_mid_window_and_reverse_continue_returns() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let (w, _bpc) = discover_write(tp);
    let p_mid = discover_mid(tp, w);
    let p5 = { let s = retrace_core::seek(tp, w, 5).unwrap(); s.cur_pc() }; // pc at (w, 5)
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
    assert!(out.contains(&format!("error: window {w} ends after {len} instruction(s)")),
        "error names remaining ({len}):\n{out}");
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
