// M58 A2: `cargo test` of a fixture crate recorded through cargo's runner hook. The recipe and what it
// measured are in the M58 ledger's cargo-recipe.md.
#![allow(dead_code)]
use super::{bin, run_bounded, Bounded};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub const LEDGER_MANIFEST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../retrace-guest/agent/ledger/Cargo.toml");
pub const INVENTORY_MANIFEST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../retrace-guest/agent/inventory/Cargo.toml");

/// The statement in each fixture that stores the wrong value, exactly as written. Tests find its line by this
/// text (the fixtures carry no marker comment, since the README shows their source).
pub const LEDGER_BAD_STORE: &str = "*balance -= amount + fee;";
pub const INVENTORY_BAD_STORE: &str = "*slot += cases * CASE + loose;";

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

/// A copy of a fixture crate's sources (everything beside its manifest but `target/`) in a fresh scratch directory,
/// under the crate's own directory name (`…/ledger`), the way the README has a reader copy the installed example.
pub fn copy_fixture(manifest: &str, tag: &str) -> std::path::PathBuf {
    fn copy_tree(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap_or_else(|e| panic!("mkdir {}: {e}", dst.display()));
        for e in std::fs::read_dir(src).unwrap_or_else(|e| panic!("{}: {e}", src.display())) {
            let e = e.unwrap();
            if e.file_name() == "target" { continue; }
            let (s, d) = (e.path(), dst.join(e.file_name()));
            if e.file_type().unwrap().is_dir() { copy_tree(&s, &d) } else { std::fs::copy(&s, &d).unwrap_or_else(|e| panic!("{}: {e}", s.display())); }
        }
    }
    let src = Path::new(manifest).parent().unwrap();
    let dst = fresh_dir(tag).join(src.file_name().unwrap());
    copy_tree(src, &dst);
    dst
}

/// The README's recipe as a reader runs it, in `dir` (a crate's root): `cargo test --offline --no-fail-fast` with
/// the runner `retrace record` and `RETRACE_DIR="$PWD/target/retrace"`, `$PWD` being `dir` as the shell would expand
/// it, with cargo's own default target directory, so the paths are the ones the README shows; plus `env`. stdout
/// and stderr share one file, interleaved as a terminal shows them. Returns cargo's exit code (`None` when the 600 s
/// bound fired) and that output.
pub fn readme_recipe_in(dir: &Path, env: &[(&str, &str)]) -> (Option<i32>, String) {
    let cargo = cargo_path().expect("cargo");
    let mut vars: Vec<(String, String)> = vec![
        ("CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER".into(), format!("{} record", bin())),
        ("RETRACE_DIR".into(), dir.join("target/retrace").to_str().expect("a UTF-8 path").into()),
    ];
    for k in ["PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN", "TMPDIR"] {
        if let Ok(v) = std::env::var(k) { vars.push((k.into(), v)); }
    }
    vars.extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let log = dir.join("cargo-test.log");
    let f = std::fs::File::create(&log).expect("create the log");
    let mut c = std::process::Command::new(&cargo);
    c.args(["test", "--offline", "--no-fail-fast"]).current_dir(dir).env_clear()
        .stdin(std::process::Stdio::null()).stdout(f.try_clone().expect("clone the log")).stderr(f);
    for (k, v) in &vars { c.env(k, v); }
    let mut child = c.spawn().unwrap_or_else(|e| panic!("spawn {cargo}: {e}"));
    let mut status = None;
    for _ in 0..600 * 20 {
        if let Some(st) = child.try_wait().expect("try_wait") { status = Some(st); break; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if status.is_none() { let _ = child.kill(); let _ = child.wait(); }
    let code = status.and_then(|s| s.code());
    let out = String::from_utf8_lossy(&std::fs::read(&log).unwrap_or_default()).into_owned();
    let _ = std::fs::remove_file(&log);
    (code, out)
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

/// Remove a fixture copy (and the scratch directory above it) after a test passed; a failing test never reaches
/// it, so its recordings stay for debugging.
pub fn remove_fixture_copy(dir: &Path) {
    // Safe: `copy_fixture` puts the copy at `<private fresh_dir>/<crate name>`, so its parent is that scratch dir alone.
    // Guarded: only a directory `fresh_dir` made (its `retrace-cargo-` prefix) is ever removed.
    match dir.parent() {
        Some(p) if p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("retrace-cargo-")) => { let _ = std::fs::remove_dir_all(p); }
        _ => super::announce(&format!("NOTE: not removing {}: its parent is not a retrace-cargo- scratch dir", dir.display())),
    }
}
