// M55 t5 gate (spec §3k; t0 §T0-tokio). Nine programs built with tokio `=1.54.1`, `features = ["full"]`, from
// the repo-owned crate `crates/retrace-guest/tokio` (its own workspace, built offline from a committed lock):
// each is run natively, recorded and replayed twice, and its line is pinned by value. Before M55 every one
// stopped at `socketpair (syscall 135)`, the first call of tokio's I/O driver. The crate is built once per test
// process with the toolchain that runs the tests; if it will not build offline every test announces a skip.
// `tk_test` is a libtest binary, so its output carries timing text: only its `signal_usr1` line and result
// prefix are compared with native, and the record and both replays must still agree byte for byte.
mod util;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

const BOUND: u64 = 180;
const MANIFEST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../retrace-guest/tokio/Cargo.toml");
const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../retrace-guest/tokio/data/tk_fs.txt");

/// Program name to executable path, or the first line of the build's error.
fn fixtures() -> &'static Result<HashMap<String, String>, String> {
    static BUILT: OnceLock<Result<HashMap<String, String>, String>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let target = concat!(env!("CARGO_TARGET_TMPDIR"), "/tokio-fixtures");
        let run = |sub: &str, extra: &[&str]| {
            Command::new(&cargo).args([sub, "--manifest-path", MANIFEST, "--release", "--offline", "-j", "6"])
                .args(extra).env("CARGO_TARGET_DIR", target).output().expect("spawn cargo")
        };
        // T5-a: only cargo's offline-fetch failure is a skip (no cached registry crate, or a network request
        // refused under --offline). Any other build failure, a compile error in a fixture included, is a
        // failure: `prog` panics on an error that does not start with `OFFLINE:`.
        let first_err = |o: &std::process::Output| {
            let err = String::from_utf8_lossy(&o.stderr);
            let line = err.lines().find(|l| l.contains("error")).unwrap_or("(no error line)");
            let offline = err.contains("offline mode (--offline)") || err.contains("--offline was specified");
            format!("{}{line}", if offline { "OFFLINE:" } else { "BUILD:" })
        };
        let b = run("build", &["--message-format=json"]);
        if !b.status.success() { return Err(first_err(&b)); }
        let t = run("test", &["--no-run", "--message-format=json"]);
        if !t.status.success() { return Err(first_err(&t)); }
        // The build's artifacts name the programs; the test run's name `tk_test`.
        let mut out = b.stdout.clone();
        out.extend_from_slice(&t.stdout);
        // Each compiler-artifact line carries `"target":{...,"name":"<n>",...}` and `"executable":"<path>"`.
        let mut map = HashMap::new();
        for line in String::from_utf8_lossy(&out).lines() {
            let (Some(e), Some(t)) = (line.split("\"executable\":\"").nth(1), line.split("\"target\":").nth(1)) else { continue };
            // `cargo test` also emits each bin as a libtest harness (`"test":true`); only the program counts.
            if t.starts_with("{\"kind\":[\"bin\"]") && line.split("\"profile\":").nth(1).is_some_and(|p| p.contains("\"test\":true")) { continue; }
            let Some(name) = t.split("\"name\":\"").nth(1).and_then(|s| s.split('"').next()) else { continue };
            map.insert(name.to_string(), e.split('"').next().unwrap().to_string());
        }
        Ok(map)
    })
}

/// The program's path, or announce the skip and return None.
fn prog(name: &str) -> Option<String> {
    match fixtures() {
        Ok(m) => Some(m.get(name).unwrap_or_else(|| panic!("no fixture {name}: {m:?}")).clone()),
        Err(e) if e.starts_with("OFFLINE:") => {
            util::announce(&format!("SKIPPED: tokio fixtures do not build offline: {e}"));
            None
        }
        Err(e) => panic!("the tokio fixtures failed to build: {e}"),
    }
}

fn trace_path() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("retrace-tokio-{}-{}.bin", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
}

fn record(exe: &str, args: &[&str]) -> (util::Bounded, PathBuf) {
    let trace = trace_path();
    let mut a = vec!["record", "-o", trace.to_str().unwrap(), exe];
    if !args.is_empty() { a.extend_from_slice(args); }
    (util::run_bounded(util::bin(), &a, &[], BOUND), trace)
}

/// Native, then record and two replays. Record's exit code and stdout agree with native (through `view`),
/// both replays agree with record byte for byte, and nothing stops at a wall.
fn records_as_native(exe: &str, args: &[&str], view: impl Fn(&str) -> String) -> String {
    let n = util::run_bounded(exe, args, &[], BOUND);
    let ncode = n.code.unwrap_or_else(|| panic!("{exe}: native hung"));
    let (rec, trace) = record(exe, args);
    assert!(!rec.stderr.contains("retrace: unsupported:"), "{exe}: no wall. stderr:\n{}", rec.stderr);
    let (nout, rout) = (String::from_utf8_lossy(&n.stdout).into_owned(), String::from_utf8_lossy(&rec.stdout).into_owned());
    assert_eq!((rec.code, view(&rout)), (Some(ncode), view(&nout)), "{exe}: record against native. stderr:\n{}", rec.stderr);
    for k in 1..=2 {
        let rp = util::run_bounded(util::bin(), &["replay", trace.to_str().unwrap()], &[], BOUND);
        assert_eq!((rp.code, rp.stdout.as_slice()), (rec.code, rec.stdout.as_slice()), "{exe}: replay {k}: {}", rp.stderr);
    }
    rout
}

fn whole(s: &str) -> String { s.to_string() }

fn pinned(name: &str, args: &[&str], line: &str) {
    let Some(exe) = prog(name) else { return };
    assert_eq!(records_as_native(&exe, args, whole), format!("{line}\n"));
}

#[test]
fn tk_full_records_and_replays() { pinned("tk_full", &[], "tokio full: 42"); }

#[test]
fn tk_ct_records_and_replays() { pinned("tk_ct", &[], "tokio ct: 42"); }

#[test]
fn tk_sig_records_and_replays() { pinned("tk_sig", &[], "tokio sig: got SIGUSR1"); }

#[test]
fn tk_kill_reaches_main_in_kevent() { pinned("tk_kill", &[], "tokio kill: got SIGUSR1"); }

#[test]
fn tk_chan_records_and_replays() { pinned("tk_chan", &[], "tokio chan: sum=60 blocking=61"); }

#[test]
fn tk_fs_records_and_replays() {
    pinned("tk_fs", &[DATA], "tokio fs: 45 bytes, first line \"retrace tokio fixture\"");
}

#[test]
fn tk_select_records_and_replays() { pinned("tk_select", &[], "tokio select: sleep"); }

#[test]
fn tk_test_signals_itself_through_a_parked_main() {
    let Some(exe) = prog("tk_test") else { return };
    let view = |s: &str| s.lines()
        .filter(|l| l.starts_with("test signal_usr1 ... ok") || l.starts_with("test result: ok. 1 passed"))
        .map(|l| l.split(';').next().unwrap().to_string()).collect::<Vec<_>>().join("\n");
    let out = records_as_native(&exe, &["--test-threads=1"], view);
    assert!(out.contains("tokio test: got SIGUSR1") || out.contains("signal_usr1 ... ok"), "{out}");
    assert_eq!(view(&out), "test signal_usr1 ... ok\ntest result: ok. 1 passed");
}

#[test]
fn tk_kill_mt_is_refused_at_the_raise_by_value() {
    let Some(exe) = prog("tk_kill_mt") else { return };
    let (rec, _) = record(&exe, &[]);
    assert!(rec.stderr.contains(
        "retrace: unsupported: a handled signal (SIGUSR1) to thread 0, blocked in psynch_cvwait"),
        "the refusal names the signal and the call. stderr:\n{}", rec.stderr);
    assert_eq!(rec.code, Some(4), "a wall exits 4. stderr:\n{}", rec.stderr);
    assert!(!rec.stderr.contains("DEADLOCK"), "not the deadlock wall. stderr:\n{}", rec.stderr);
}
