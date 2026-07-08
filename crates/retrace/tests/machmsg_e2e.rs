// A freestanding guest issues a REAL wire-format _kernelrpc_mach_vm_map (4811) via mach_msg2
// (svc -47): the box must service it against guest IPAs (not forward it), the guest stores
// through the returned mapping, prints 2 bytes, exits 0 — and the trace replays identically.
mod util;
#[test]
fn machmsg_vm_map_records_and_replays() {
    let (rec, trace) = util::record(retrace_guest::MACHMSG);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert_eq!(rec.stdout, b"MK");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, b"MK");
}
