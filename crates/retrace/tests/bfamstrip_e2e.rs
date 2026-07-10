// Proves the strip-on-FPAC arm in isolation: a guest DATA-B-signs a canonical pointer, corrupts a
// PAC bit so `autdb` FEAT_FPAC-faults, then executes `autdb`. The box must intercept the FPAC,
// strip x0 to canonical (emulating a successful authenticate), and skip the instruction; the guest
// then finds the recovered pointer == the original and exits 0. Without the arm, the autdb FPACs,
// the box errors out, and record exits nonzero. Also replays identically.
mod util;
#[test]
fn bfamstrip_fpac_auth_emulated() {
    let (rec, trace) = util::record(retrace_guest::BFAMSTRIP);
    assert_eq!(rec.code, 0, "record failed (autdb FPAC not emulated?): {}", rec.stderr);
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
}
