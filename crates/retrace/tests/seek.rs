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
        (s.dbg_regs(), s.pc(), mem)
    };

    // Session 2: same seek, byte-compare registers and full memory.
    let mut s = ReplaySession::open(trace).unwrap();
    s.advance_to_landmark(100).unwrap();
    assert_eq!(s.dbg_regs(), regs1);
    assert_eq!(s.pc(), pc1);
    assert!(s.diff_memory(&snap_mem).is_none(), "memory diverged between two seeks");
}
