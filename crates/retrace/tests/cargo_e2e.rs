// M58 A2 gate: `cargo test` of the repo-owned `ledger` crate recorded through cargo's runner hook
// (`CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="retrace record"`, `RETRACE_DIR`). The measured recipe is in the M58
// ledger's cargo-recipe.md: one `<crate>-<hash>.retrace` per unit-test and integration-test binary, doctests as a
// hash-free `rust_out.retrace`, a failing run exiting 101. The crate's one failing test is the point: the trace
// named under its `Running unittests` line must replay to the same failure.
mod util;

use std::sync::OnceLock;
use util::cargo::*;

struct Suite { code: Option<i32>, stdout: String, stderr: String, dir: std::path::PathBuf }

fn suite() -> Option<&'static Suite> {
    static S: OnceLock<Option<Suite>> = OnceLock::new();
    S.get_or_init(|| {
        cargo_path()?;
        let dir = fresh_dir("suite");
        let b = cargo_test_under_retrace(LEDGER_MANIFEST, &dir, &["--no-fail-fast"]);
        Some(Suite { code: b.code, stdout: String::from_utf8_lossy(&b.stdout).into_owned(), stderr: b.stderr, dir })
    }).as_ref()
}

fn have(what: &str) -> Option<&'static Suite> {
    let s = suite();
    if s.is_none() { util::announce(&format!("SKIPPED: cargo is not runnable ({what})")); }
    s
}

fn traces(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|r| r.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn is_hashed(name: &str, krate: &str) -> bool {
    name.strip_prefix(&format!("{krate}-")).and_then(|r| r.strip_suffix(".retrace"))
        .is_some_and(|h| h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[test]
fn ledger_suite_records_through_the_runner() {
    let Some(s) = have("ledger suite") else { return };
    assert_eq!(s.code, Some(101), "cargo's failure exit; stderr:\n{}", s.stderr);
    let t = traces(&s.dir);
    let hashed: Vec<&String> = t.iter().filter(|n| is_hashed(n, "ledger")).collect();
    assert_eq!(hashed.len(), 1, "one unit-test trace: {t:?}");
    assert!(t.iter().any(|n| is_hashed(n, "flow")), "one integration-test trace: {t:?}");
    assert!(t.iter().any(|n| n == "rust_out.retrace"), "the doctest's trace: {t:?}");
    assert_eq!(t.len(), 3, "{t:?}");
    // Replay of the failing unit-test trace: the same exit and the same stdout as the record.
    let p = s.dir.join(hashed[0]);
    let r = util::replay(&p);
    assert_eq!(r.code, 101, "replay: {}", r.stderr);
    let out = String::from_utf8_lossy(&r.stdout).into_owned();
    assert!(out.contains("settling_four_hundred_leaves_the_rest ... FAILED"), "{out}");
    assert!(s.stdout.contains(&out), "replay's stdout is what the record printed\nrecord:\n{}\nreplay:\n{out}", s.stdout);
}

#[test]
fn the_failing_trace_is_named_under_its_running_line() {
    let Some(s) = have("running line") else { return };
    let pairs = saved_after_running(&s.stderr);
    let (running, path) = pairs.iter().find(|(r, _)| r.contains("unittests")).unwrap_or_else(|| panic!("{pairs:?}\n{}", s.stderr));
    assert!(running.contains("ledger-"), "{running}");
    let r = util::replay(std::path::Path::new(path));
    assert_eq!(r.code, 101, "the named trace is the failing one: {}", r.stderr);
    assert!(String::from_utf8_lossy(&r.stdout).contains("FAILED"));
}

#[test]
fn a_passing_filter_exits_zero() {
    if cargo_path().is_none() {
        util::announce("SKIPPED: cargo is not runnable (filter)");
        return;
    }
    let dir = fresh_dir("filter");
    let b = cargo_test_under_retrace(LEDGER_MANIFEST, &dir, &["deposits_add_up"]);
    assert_eq!(b.code, Some(0), "stderr:\n{}", b.stderr);
    assert!(String::from_utf8_lossy(&b.stdout).contains("deposits_add_up ... ok"));
}
