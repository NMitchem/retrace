use std::process::Command;

// `.cargo/config.toml`'s `runner` ad-hoc codesigns the binary cargo invokes
// directly (the test harness) with the hypervisor entitlement, but CARGO_BIN_EXE_retrace
// is a separate binary that this test spawns itself — it never passes through
// that runner. Every executable that calls hv_* needs the entitlement (see
// tools/codesign-run.sh), so sign it here the same way before exec'ing it.
fn bin() -> &'static str {
    let p = env!("CARGO_BIN_EXE_retrace");
    let ent = concat!(env!("CARGO_MANIFEST_DIR"), "/../../retrace.entitlements");
    let out = Command::new("codesign")
        .args(["-s", "-", "-f", "--entitlements", ent, p])
        .output()
        .expect("codesign");
    assert!(out.status.success(), "codesign -f --entitlements failed for {p}: {}", String::from_utf8_lossy(&out.stderr));
    p
}

#[test]
fn record_then_replay_hello_in_separate_processes() {
    let trace = std::env::temp_dir().join(format!("retrace-e2e-{}.bin", std::process::id()));
    let rec = Command::new(bin())
        .args(["record", retrace_guest::HELLO, "-o", trace.to_str().unwrap()])
        .output().unwrap();
    assert!(rec.status.success(), "record failed: {}", String::from_utf8_lossy(&rec.stderr));
    assert_eq!(rec.stdout, b"hello\n");

    let rep = Command::new(bin())
        .args(["replay", trace.to_str().unwrap()])
        .output().unwrap();
    assert!(rep.status.success(), "replay diverged: {}", String::from_utf8_lossy(&rep.stderr));
    assert_eq!(rep.stdout, b"hello\n");
}
