// Milestone property, in isolation: under a 47-bit guest VA the hardware PAC signature lands in
// bits [54:47], ABOVE objc's 47-bit ISA_MASK, so an objc-style plain-AND strip of a pacda-signed
// pointer recovers the original. The guest signs a fixed canonical pointer P, strips it with the
// 47-bit mask, and writes the 8-byte result; we assert it equals P. RED under the old 36-bit VA
// (PAC bits [46:36] survive the strip); GREEN under 47-bit. Also replays byte-identically.
mod util;
#[test]
fn strip47_lossless_under_wide_va() {
    let p: u64 = 0x1000_0000;                       // must match strip47.s's P
    let (rec, trace) = util::record(retrace_guest::STRIP47);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert_eq!(rec.stdout, p.to_le_bytes(), "objc-style 47-bit strip was lossy (PAC bits below the mask)");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, p.to_le_bytes());
}
