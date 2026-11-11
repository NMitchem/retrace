//! M58 B4: `bt`, a frame-pointer walk, against lldb's `bt` over the same recording. The frame pcs are the claim:
//! lldb numbers its threads differently and prints more detail, so only the pcs are compared, from frame 0 up to
//! and including the first frame in the function the test names. A third test walks a thread that is not running.
//! lldb is not a repo artifact, so its tests skip loudly without it. Every run is bounded.
mod util;
use std::path::Path;
use std::process::{Command, Stdio};
use util::cargo::{cargo_path, copy_fixture, readme_recipe_in, saved_after_running, LEDGER_MANIFEST};

const BOUND: u64 = 180;
const LLDB: &str = "/usr/bin/lldb";

fn lldb_runs() -> bool {
    Command::new(LLDB).arg("--version").stdin(Stdio::null()).output().ok()
        .is_some_and(|o| o.status.success() && o.stdout.starts_with(b"lldb-"))
}

/// `(pc, text)` of each `#<i> 0x… …` frame line of retrace's `bt`.
fn retrace_frames(out: &str) -> Vec<(u64, String)> {
    out.lines().filter_map(|l| {
        let rest = l.strip_prefix('#')?;
        let (_, rest) = rest.split_once(' ')?;
        let pc = rest.split_whitespace().next()?;
        Some((u64::from_str_radix(pc.strip_prefix("0x")?, 16).ok()?, l.to_string()))
    }).collect()
}

/// `(pc, text)` of each `frame #<i>: 0x… …` line of lldb's `bt` (the output after the last `(lldb) bt`; the stops
/// before it print a frame 0 too). lldb lists an inlined function as a frame of its own at the same pc, so a pc
/// that repeats the one before it is one frame here.
fn lldb_frames(out: &str) -> Vec<(u64, String)> {
    let after = out.rsplit_once("(lldb) bt").map_or(out, |(_, a)| a);
    let mut v: Vec<(u64, String)> = after.lines().filter_map(|l| {
        let (_, rest) = l.split_once("frame #")?;
        let (_, rest) = rest.split_once(": ")?;
        let pc = rest.split_whitespace().next()?;
        Some((u64::from_str_radix(pc.strip_prefix("0x")?, 16).ok()?, l.to_string()))
    }).collect();
    v.dedup_by_key(|f| f.0);
    v
}

/// The pcs up to and including the first frame whose text satisfies `stop`; panics when none does.
fn through(frames: &[(u64, String)], stop: impl Fn(&str) -> bool, who: &str, raw: &str) -> Vec<u64> {
    let i = frames.iter().position(|(_, t)| stop(t)).unwrap_or_else(|| panic!("{who}: no frame in the target function:\n{raw}"));
    frames[..=i].iter().map(|f| f.0).collect()
}

fn debug_out(trace: &str, script: &str) -> String {
    let r = util::run_bounded(util::bin(), &["debug", trace, "--script", script], &[], BOUND);
    let out = String::from_utf8_lossy(&r.stdout).into_owned();
    println!("--- retrace debug --script {script:?}\n{out}");
    assert_eq!(r.code, Some(0), "debug exit; stderr:\n{}\nstdout:\n{out}", r.stderr);
    out
}

fn lldb_out(dir: Option<&Path>, trace: &str, cmds: &[(&str, &str)]) -> String {
    let mut args = vec!["lldb", trace, "--", "--batch"];
    for (f, c) in cmds { args.push(f); args.push(c); }
    let r = match dir { Some(d) => util::run_in(d, &args, &[], BOUND), None => util::run_bounded(util::bin(), &args, &[], BOUND) };
    let out = String::from_utf8_lossy(&r.stdout).into_owned();
    println!("--- retrace {}\n{out}", args.join(" "));
    assert_eq!(r.code, Some(0), "lldb exit; stderr:\n{}\nstdout:\n{out}", r.stderr);
    out
}

#[test]
fn bt_matches_lldb_on_crashy() {
    if !lldb_runs() {
        util::announce("SKIPPED bt_matches_lldb_on_crashy: /usr/bin/lldb did not run. This gate did NOT run.");
        return;
    }
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert!(rec.code != 0, "crashy crashes: {}", rec.stderr);
    let ts = trace.to_str().unwrap();
    let mine = debug_out(ts, "continue; bt");
    // After a crash stop lldb's batch mode drops later `-o` commands; `-k` ones run.
    let theirs = lldb_out(None, ts, &[("-o", "process continue"), ("-k", "bt")]);
    let is_main = |t: &str| t.contains("main");
    let a = through(&retrace_frames(&mine), is_main, "retrace", &mine);
    let b = through(&lldb_frames(&theirs), is_main, "lldb", &theirs);
    assert_eq!(a, b, "frame pcs, retrace vs lldb");
}

#[test]
fn bt_matches_lldb_on_ledger_panic() {
    if !lldb_runs() || cargo_path().is_none() {
        util::announce("SKIPPED bt_matches_lldb_on_ledger_panic: lldb or cargo did not run. This gate did NOT run.");
        return;
    }
    let dir = copy_fixture(LEDGER_MANIFEST, "bt");
    let (code, out) = readme_recipe_in(&dir, &[]);
    assert_eq!(code, Some(101), "cargo's failing exit:\n{out}");
    let pairs = saved_after_running(&out);
    let (_, saved) = pairs.iter().find(|(r, _)| r.contains("unittests")).unwrap_or_else(|| panic!("{pairs:?}\n{out}"));
    let ts = saved.as_str();
    let test_fn = "ledger::tests::settling_four_hundred_leaves_the_rest";
    let src = std::fs::read_to_string(Path::new(LEDGER_MANIFEST).parent().unwrap().join("src/lib.rs")).unwrap();
    let start = src.lines().position(|l| l.contains("fn settling_four_hundred_leaves_the_rest")).unwrap();
    let assert_line = start + 1 + src.lines().skip(start).position(|l| l.contains("assert_eq!(l.balance")).unwrap();
    let mine = debug_out(ts, "break rust_panic; continue; bt");
    let theirs = lldb_out(Some(&dir), ts, &[("-o", "breakpoint set -n rust_panic"), ("-o", "process continue"), ("-o", "bt 64")]);
    let is_test = |t: &str| t.contains(test_fn);
    let a = through(&retrace_frames(&mine), is_test, "retrace", &mine);
    let b = through(&lldb_frames(&theirs), is_test, "lldb", &theirs);
    // Frame 0 differs by design: lldb's `breakpoint set -n` stops after the prologue (+12), retrace's `break` at the
    // symbol (+0). From frame 1 on it is the walk: at +0 frame 1 is `lr`, and it must land on lldb's.
    assert_eq!(a[1..], b[1..], "frame pcs from frame 1, retrace vs lldb");
    let l = mine.lines().find(|l| l.contains(test_fn)).unwrap();
    assert!(l.contains(&format!("lib.rs:{assert_line}")), "the test's frame at its assert_eq! line {assert_line}: {l}");
}

#[test]
fn bt_of_a_blocked_thread() {
    let (rec, trace) = util::record_dynamic(retrace_guest::WATCHTHREAD);
    assert_eq!(rec.code, 0, "{}", rec.stderr);
    let stdout = String::from_utf8_lossy(&rec.stdout).into_owned();
    let cell = stdout.split("child cell ").nth(1).and_then(|r| r.lines().next()).unwrap_or_else(|| panic!("{stdout}")).trim().to_string();
    let ts = trace.to_str().unwrap();
    // The child's store stops the run with main parked in `pthread_join`.
    let out = debug_out(ts, &format!("watch {cell}; continue; threads; bt; bt 0; bt 1"));
    let sections: Vec<&str> = out.split("\n#0 ").collect();
    assert!(sections.len() >= 4, "three bt outputs:\n{out}");
    let cur = retrace_frames(&format!("#0 {}", sections[1]));
    let main_t = retrace_frames(&format!("#0 {}", sections[2]));
    let main_pcs: Vec<u64> = main_t.iter().map(|f| f.0).collect();
    assert!(main_pcs.len() >= 3, "main's stack:\n{out}");
    assert_ne!(cur.iter().map(|f| f.0).collect::<Vec<_>>(), main_pcs, "a blocked thread's walk is its own:\n{out}");
    assert!(main_t.iter().any(|f| f.1.contains("main") || f.1.contains("join")), "a frame in main or the join path:\n{out}");
}
