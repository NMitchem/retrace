use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

// `.cargo/config.toml`'s `runner` ad-hoc codesigns the binary cargo invokes
// directly (the test harness) with the hypervisor entitlement, but CARGO_BIN_EXE_retrace
// is a separate binary that this test spawns itself — it never passes through
// that runner. Every executable that calls hv_* needs the entitlement (see
// tools/codesign-run.sh), so sign it here the same way before exec'ing it.
pub fn bin() -> &'static str {
    let p = env!("CARGO_BIN_EXE_retrace");
    let ent = concat!(env!("CARGO_MANIFEST_DIR"), "/../../retrace.entitlements");
    let out = Command::new("codesign")
        .args(["-s", "-", "-f", "--entitlements", ent, p])
        .output()
        .expect("codesign");
    assert!(out.status.success(), "codesign -f --entitlements failed for {p}: {}", String::from_utf8_lossy(&out.stderr));
    p
}

pub struct RunOut { pub code: i32, pub stdout: Vec<u8>, pub stderr: String }

fn run(args: &[&str]) -> RunOut {
    let out = Command::new(bin()).args(args).output().unwrap();
    RunOut {
        code: out.status.code().unwrap_or(-1),
        stdout: out.stdout,
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

// Record `guest` into a fresh trace file in the system tempdir; a monotonic counter (plus
// this process's pid) keeps concurrent/repeated calls within one test binary from colliding.
pub fn record(guest: &str) -> (RunOut, std::path::PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let trace = std::env::temp_dir().join(format!("retrace-util-{}-{n}.bin", std::process::id()));
    let out = run(&["record", guest, "-o", trace.to_str().unwrap()]);
    (out, trace)
}

pub fn replay(trace: &std::path::Path) -> RunOut {
    run(&["replay", trace.to_str().unwrap()])
}

// record_dynamic is added in T9 once the CLI's --dynamic path exists.
