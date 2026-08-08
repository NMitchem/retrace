// The only gate that exercises APPLE's `_sigtramp` rather than a hand-written one: libc's
// `sigaction()` overwrites `sa_tramp` with its own, which is what a real program actually runs
// through. Every other M12 guest supplies its own trampoline and so tests retrace's entry contract
// with libc out of the way; this one tests that the frame retrace builds satisfies the trampoline
// that actually ships.
mod util;

#[test]
fn a_dynamic_c_guest_catches_repairs_and_continues_through_apples_sigtramp() {
    let (rec, trace) = util::record_dynamic(retrace_guest::SIGCATCH_DYN);
    assert_eq!(rec.code, 0, "stderr:\n{}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout);
    // The faulting VA has bit 46 set, exactly as crashy.c's GARBAGE_VA does: only a STAGE-1
    // translation fault reaches Stop::Fault, the stop the delivery arm consults. A VA below 2^36
    // takes a stage-2 abort instead and kills the recording before any handler could run.
    assert!(out.contains("si_addr=0x4000dead0000"),
        "the handler read si_addr out of the frame retrace built; stdout:\n{out}");
    assert!(out.contains("resumed"),
        "and sigreturn brought it back THROUGH Apple's trampoline; stdout:\n{out}");
    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 0, "replay {i}; stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} diverged");
    }
}
