//! The README's demo, run the way the README shows it. `examples/visits.c` overflows a 16-byte name field
//! into the `sqlite3_stmt *` beside it whenever it picks the one 28-character name, so about one run in five
//! crashes inside SQLite. Each claim the README makes next to the demo is checked here:
//!
//! - natively, some runs pass and some crash, so a plain rerun does not reliably reproduce it;
//! - AddressSanitizer reports only a SEGV, never an overflow (the write stays inside the struct);
//! - recording until a run crashes, then `last-write` on `app.insert`, names `visits.c:21`, the `memcpy`.
//!
//! Every run is bounded and every recording is deleted. It skips loudly without clang.
mod util;

use std::path::{Path, PathBuf};
use std::process::Command;

const VISITS_C: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/visits.c");
/// "urray Ho", the 8 bytes of "Grace Brewster Murray Hopper" that land on `app.insert`, little-endian.
const FAULT: &str = "0x6f48207961727275";
const BOUND: u64 = 120;
/// At a measured one crash in five, 60 runs all passing has a probability under 1e-6.
const TRIES: usize = 60;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("retrace-demo-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Build `visits.c` into `dir` with `extra` flags. False, after a skip line, when clang can't.
fn build(dir: &Path, exe: &str, extra: &[&str], test: &str) -> bool {
    let mut args = vec!["-g", "-o", exe, VISITS_C, "-lsqlite3"];
    args.extend_from_slice(extra);
    let built = Command::new("clang").args(&args).current_dir(dir).output();
    if !built.as_ref().is_ok_and(|o| o.status.success()) {
        util::announce(&format!("SKIPPED {test}: clang did not build examples/visits.c ({built:?}). This gate did NOT run."));
        return false;
    }
    true
}

/// The line in `visits.c` that holds `text`.
fn line_of(text: &str) -> usize {
    let src = std::fs::read_to_string(VISITS_C).unwrap();
    src.lines().position(|l| l.contains(text)).unwrap_or_else(|| panic!("`{text}` is not in visits.c")) + 1
}

/// `&app.insert`: the symbol `_app` from `nm`, plus 16 for `last_visitor[16]`. The program is loaded
/// unslid under retrace, so the file's address is the recording's.
fn app_insert(exe: &Path) -> String {
    let nm = Command::new("nm").arg(exe).output().expect("nm");
    let out = String::from_utf8_lossy(&nm.stdout).into_owned();
    let line = out.lines().find(|l| l.ends_with(" _app")).unwrap_or_else(|| panic!("no _app in nm output:\n{out}"));
    let base = u64::from_str_radix(line.split_whitespace().next().unwrap(), 16).unwrap();
    format!("{:#x}", base + 16)
}

#[test]
fn natively_it_crashes_on_some_runs_only() {
    let dir = scratch("native");
    if !build(&dir, "visits", &[], "natively_it_crashes_on_some_runs_only") {
        return;
    }
    let exe = dir.join("visits");
    let (mut pass, mut crash) = (0, 0);
    for _ in 0..TRIES {
        match util::run_bounded(exe.to_str().unwrap(), &[], &[], BOUND).code {
            Some(0) => pass += 1,
            Some(139) => crash += 1,
            other => panic!("visits exited {other:?}"),
        }
    }
    assert!(pass > 0 && crash > 0, "{pass} passing and {crash} crashing runs of {TRIES}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn address_sanitizer_sees_a_segv_and_no_overflow() {
    let dir = scratch("asan");
    if !build(&dir, "visits", &["-fsanitize=address"], "address_sanitizer_sees_a_segv_and_no_overflow") {
        return;
    }
    let exe = dir.join("visits");
    let mut crashed = None;
    for _ in 0..TRIES {
        let r = util::run_bounded(exe.to_str().unwrap(), &[], &[], BOUND);
        assert!(!r.stderr.contains("overflow"), "ASan reported an overflow:\n{}", r.stderr);
        if r.code != Some(0) {
            crashed = Some(r.stderr);
            break;
        }
    }
    let err = crashed.unwrap_or_else(|| panic!("no ASan run crashed in {TRIES}"));
    assert!(err.contains("AddressSanitizer: SEGV on unknown address"), "{err}");
    assert!(err.contains("libsqlite3.dylib"), "the SEGV is inside SQLite:\n{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_recording_of_the_crash_names_the_memcpy() {
    let dir = scratch("record");
    if !build(&dir, "visits", &[], "the_recording_of_the_crash_names_the_memcpy") {
        return;
    }
    // `while retrace record ./visits; do :; done`: each passing run's recording is overwritten.
    let mut rec = None;
    for _ in 0..TRIES {
        let r = util::run_in(&dir, &["record", "./visits"], &[], BOUND);
        match r.code {
            Some(0) => continue,
            Some(139) => {
                rec = Some(r);
                break;
            }
            other => panic!("record exited {other:?}:\n{}", r.stderr),
        }
    }
    let rec = rec.unwrap_or_else(|| panic!("no recorded run crashed in {TRIES}"));
    assert!(rec.stderr.contains(&format!("killed by SIGSEGV (fault at {FAULT}")), "{}", rec.stderr);

    let trace = dir.join("visits.retrace");
    let addr = app_insert(&dir.join("visits"));
    let script = format!("summary; last-write {addr}");
    let (code, out, err) = util::debug_bounded(trace.to_str().unwrap(), &script, BOUND);
    assert_eq!(code, Some(0), "debug: {err}\n{out}");
    let (summary, write) = out.split_once("> last-write").unwrap_or_else(|| panic!("no last-write:\n{out}"));
    // The crash is in SQLite, called from the bind on line 18 or the finalize on line 24.
    let (bind, fin) = (line_of("sqlite3_bind_text"), line_of("sqlite3_finalize"));
    assert!(summary.contains(&format!("fault at {FAULT}")), "{summary}");
    assert!(summary.contains(&format!("visits.c:{bind}")) || summary.contains(&format!("visits.c:{fin}")),
        "the crash is called from line {bind} or {fin}:\n{summary}");
    let memcpy = line_of("memcpy(app.last_visitor");
    assert!(write.contains(&format!("last write to {addr} (8 bytes): store")), "{write}");
    assert!(write.contains(&format!("visits.c:{memcpy}")), "last-write names the memcpy on line {memcpy}:\n{write}");

    let rp = util::run_bounded(util::bin(), &["replay", trace.to_str().unwrap()], &[], BOUND);
    assert_eq!(rp.code, Some(139), "replay: {}", rp.stderr);
    let _ = std::fs::remove_dir_all(&dir);
}
