// The M3-pos oracle: seeking the same landmark twice yields byte-identical machine state.
mod util;
use retrace_core::ReplaySession;

#[test]
fn landmark_seek_is_deterministic() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let trace = std::path::Path::new(&trace);

    // Session 1: seek landmark 100, capture state, then DROP (one VM per process).
    let (regs1, pc1, snap_mem) = {
        let mut s = ReplaySession::open(trace).unwrap();
        s.advance_to_landmark(100).unwrap();
        assert_eq!(s.landmark(), 100);
        let (_, mem) = s.snapshot();
        (s.dbg_regs(), s.position(), mem)
    };

    // Session 2: same seek, byte-compare registers and full memory.
    let mut s = ReplaySession::open(trace).unwrap();
    s.advance_to_landmark(100).unwrap();
    assert_eq!(s.dbg_regs(), regs1);
    assert_eq!(s.position(), pc1);
    assert!(s.diff_memory(&snap_mem).is_none(), "memory diverged between two seeks");

    // read_mem is all-or-nothing: the pc's code page reads back, an unmapped va is None, and a
    // span that runs off the end of a mapped backing is None (must NOT panic — the review fix).
    let pc = s.position();
    assert!(s.read_mem(pc, 16).is_some(), "pc's code page should be readable");
    assert!(s.read_mem(0xDEAD_0000_0000, 16).is_none(), "unmapped va should be None");
    assert!(s.read_mem(pc, 1 << 30).is_none(), "span crossing out of the backing should be None, not panic");
}

#[test]
fn window_len_is_deterministic() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0);
    let trace = std::path::Path::new(&trace);
    let (n, l1) = first_window_with_len(trace, 4);
    let l2 = { let mut s = retrace_core::seek(trace, n, 0).unwrap(); s.window_len_here().unwrap() };
    assert_eq!(l1, l2, "window {n} length differs between sessions");
}

#[test]
fn step_seek_is_deterministic_and_window_end_errors() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0);
    let trace = std::path::Path::new(&trace);
    let (n, len) = first_window_with_len(trace, 4);
    let k = len / 2;
    let (regs1, pc1, mem1) = {
        let mut s = retrace_core::seek(trace, n, k).unwrap();
        let (_, mem) = s.snapshot(); (s.dbg_regs(), s.position(), mem)
    };
    let s = retrace_core::seek(trace, n, k).unwrap();
    assert_eq!(s.dbg_regs(), regs1);
    assert_eq!(s.position(), pc1);
    assert!(s.diff_memory(&mem1).is_none());
    drop(s); // one VM per process: release this session before opening the past-the-end one
    // past-the-end is a clean, length-naming error
    let err = retrace_core::seek(trace, n, len + 1).unwrap_err();
    assert!(err.contains(&len.to_string()), "error should name the window length: {err}");
}

/// Probe a few landmarks for a window of at least `min` instructions (one session each,
/// SEQUENTIAL — never two alive). Deterministic per-trace.
fn first_window_with_len(trace: &std::path::Path, min: u64) -> (usize, u64) {
    for n in [10usize, 30, 60, 100, 150] {
        let mut s = retrace_core::seek(trace, n, 0).unwrap();
        let l = s.window_len_here().unwrap();
        drop(s);
        if l >= min { return (n, l); }
    }
    panic!("no window of >= {min} insns among the probes");
}
