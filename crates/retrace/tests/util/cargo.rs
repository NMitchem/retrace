// M58 A2: `cargo test` of a fixture crate recorded through cargo's runner hook. The recipe and what it
// measured are in the M58 ledger's cargo-recipe.md.
#![allow(dead_code)]
use super::{bin, run_bounded, Bounded};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub const LEDGER_MANIFEST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../retrace-guest/agent/ledger/Cargo.toml");
pub const INVENTORY_MANIFEST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../retrace-guest/agent/inventory/Cargo.toml");

/// Is cargo runnable here? (The tests skip loudly when it is not.)
pub fn cargo_path() -> Option<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    std::process::Command::new(&cargo).arg("--version").output().ok().filter(|o| o.status.success()).map(|_| cargo)
}

/// A fresh directory path under the test temp dir, for a `RETRACE_DIR` (record creates it).
pub fn fresh_dir(tag: &str) -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!("retrace-cargo-{tag}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// `cargo test --offline --manifest-path <manifest> <extra>` with retrace as the runner, recordings placed in
/// `retrace_dir`, and a private target dir. `--no-fail-fast` is the caller's to pass in `extra`.
pub fn cargo_test_under_retrace(manifest: &str, retrace_dir: &Path, extra: &[&str]) -> Bounded {
    let cargo = cargo_path().expect("cargo");
    let runner = format!("{} record", bin());
    let target = concat!(env!("CARGO_TARGET_TMPDIR"), "/agent-fixtures");
    let rd = retrace_dir.to_str().unwrap().to_string();
    let mut env: Vec<(String, String)> = vec![
        ("CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER".into(), runner),
        ("RETRACE_DIR".into(), rd),
        ("CARGO_TARGET_DIR".into(), target.into()),
    ];
    for k in ["PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN", "TMPDIR"] {
        if let Ok(v) = std::env::var(k) { env.push((k.into(), v)); }
    }
    let envr: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mut args = vec!["test", "--offline", "--manifest-path", manifest];
    args.extend_from_slice(extra);
    run_bounded(&cargo, &args, &envr, 600)
}

/// Each `saved the recording to <p>` line's path, paired with the nearest preceding `Running ...` line, so a
/// caller can tell which binary a trace belongs to.
pub fn saved_after_running(stderr: &str) -> Vec<(String, String)> {
    let mut running = String::new();
    let mut out = vec![];
    for l in stderr.lines() {
        if l.trim_start().starts_with("Running ") { running = l.trim().to_string(); }
        if let Some(p) = l.split("saved the recording to ").nth(1) { out.push((running.clone(), p.trim().to_string())); }
    }
    out
}
