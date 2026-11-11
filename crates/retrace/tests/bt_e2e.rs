//! M58 B4: `bt`, a frame-pointer walk, against lldb's `bt` over the same recording. The frame pcs are the claim:
//! lldb numbers its threads differently and prints more detail, so only the pcs are compared, from frame 0 up to
//! and including the first frame in the function the test names. The blocked-thread test walks a thread that is not
//! running and compares it with the lldb thread whose frame 0 is the same pc.
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

/// `(pc, text)` of each `frame #<i>: 0x… …` line of `out`. lldb lists an inlined function as a frame of its own at
/// the pc of the function it is inlined into; such a frame (marked `[inlined]`, with the next frame at the same pc)
/// is dropped, since retrace's walk has one frame per pc. A recursive call, with the same pc twice and no
/// `[inlined]` mark, stays two frames.
fn parse_lldb(out: &str) -> Vec<(u64, String)> {
    let all: Vec<(u64, String)> = out.lines().filter_map(|l| {
        let (_, rest) = l.split_once("frame #")?;
        let (_, rest) = rest.split_once(": ")?;
        let pc = rest.split_whitespace().next()?;
        Some((u64::from_str_radix(pc.strip_prefix("0x")?, 16).ok()?, l.to_string()))
    }).collect();
    let mut v = Vec::new();
    for (i, f) in all.iter().enumerate() {
        if f.1.contains("[inlined]") && all.get(i + 1).is_some_and(|n| n.0 == f.0) { continue; }
        v.push(f.clone());
    }
    v
}

/// The frames of lldb's `bt`: the output after the last `(lldb) bt` (the stops before it print a frame 0 too).
fn lldb_frames(out: &str) -> Vec<(u64, String)> {
    parse_lldb(out.rsplit_once("(lldb) bt").map_or(out, |(_, a)| a))
}

/// Every thread's frames in lldb's `thread backtrace all`, from the output after the last such command.
fn lldb_all_threads(out: &str) -> Vec<Vec<(u64, String)>> {
    let after = out.rsplit_once("(lldb) thread backtrace all").map_or("", |(_, a)| a);
    let mut blocks: Vec<String> = Vec::new();
    for l in after.lines() {
        if l.trim_start_matches(['*', ' ']).starts_with("thread #") { blocks.push(String::new()); }
        if let Some(b) = blocks.last_mut() { b.push_str(l); b.push('\n'); }
    }
    blocks.iter().map(|b| parse_lldb(b)).collect()
}

/// Whether `text` names `sym` itself: not a path under it, such as `sym::{{closure}}` (retrace's form) or
/// `sym::_$u7b$$u7b$closure$u7d$$u7d$` (lldb's). lldb also appends a legacy hash, `sym::h0123…`, which is `sym`.
fn names(text: &str, sym: &str) -> bool {
    text.match_indices(sym).any(|(i, _)| { let r = &text[i + sym.len()..]; !r.starts_with("::{") && !r.starts_with("::_$") })
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
    let a = through(&retrace_frames(&mine), |t| t.contains(" in _main+"), "retrace", &mine);
    let b = through(&lldb_frames(&theirs), |t| t.contains("`main "), "lldb", &theirs);
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
    let mine = debug_out(ts, "break rust_panic; continue; where; bt");
    // lldb's `breakpoint set -n` stops after the prologue and retrace's `break` at the symbol, so lldb is told
    // retrace's exact pc and the frames are compared from frame 0.
    let pc = mine.lines().find_map(|l| l.split(" pc=").nth(1)).and_then(|r| r.split_whitespace().next())
        .unwrap_or_else(|| panic!("no `where` line:\n{mine}")).to_string();
    let theirs = lldb_out(Some(&dir), ts, &[("-o", &format!("breakpoint set -a {pc}")), ("-o", "process continue"), ("-o", "bt 64")]);
    let a = through(&retrace_frames(&mine), |t| names(t, test_fn), "retrace", &mine);
    let b = through(&lldb_frames(&theirs), |t| names(t, test_fn), "lldb", &theirs);
    assert_eq!(a, b, "frame pcs from frame 0, retrace vs lldb");
    let l = mine.lines().find(|l| l.starts_with('#') && l.contains(test_fn)).unwrap();
    assert!(l.contains(&format!("lib.rs:{assert_line}")), "the test's frame at its assert_eq! line {assert_line}: {l}");
}

#[test]
fn bt_of_a_blocked_thread() {
    if !lldb_runs() {
        util::announce("SKIPPED bt_of_a_blocked_thread: /usr/bin/lldb did not run. This gate did NOT run.");
        return;
    }
    let (rec, trace) = util::record_dynamic(retrace_guest::WATCHTHREAD);
    assert_eq!(rec.code, 0, "{}", rec.stderr);
    let stdout = String::from_utf8_lossy(&rec.stdout).into_owned();
    let cell = stdout.split("child cell ").nth(1).and_then(|r| r.lines().next()).unwrap_or_else(|| panic!("{stdout}")).trim().to_string();
    let ts = trace.to_str().unwrap();
    // The child's store stops the run with main parked in `pthread_join`: thread 0 is the blocked one.
    let mine = debug_out(ts, &format!("watch {cell}; continue; threads; bt 0"));
    let cur = retrace_frames(&debug_out(ts, &format!("watch {cell}; continue; bt")));
    let blocked = retrace_frames(&mine);
    assert!(blocked.len() >= 3, "main's stack:\n{mine}");
    assert_ne!(cur.iter().map(|f| f.0).collect::<Vec<_>>(), blocked.iter().map(|f| f.0).collect::<Vec<_>>(), "the blocked thread's walk is its own:\n{mine}");
    let theirs = lldb_out(None, ts, &[("-o", &format!("watchpoint set expression -w write -s 8 -- {cell}")), ("-o", "process continue"), ("-o", "thread backtrace all")]);
    // lldb numbers its threads its own way: the blocked one is the thread whose frame 0 is retrace's frame 0.
    let threads = lldb_all_threads(&theirs);
    let t = threads.iter().find(|t| t.first().map(|f| f.0) == Some(blocked[0].0))
        .unwrap_or_else(|| panic!("no lldb thread at {:#x}:\n{theirs}", blocked[0].0));
    let i = blocked.iter().position(|(_, t)| names(t, "watchthread::main")).expect("a frame in main");
    let a = &blocked[..=i];
    let b = through(t, |t| names(t, "watchthread::main"), "lldb", &theirs);
    // Every frame lldb reports, in order, is a frame of retrace's. Frame 0 is a syscall stub with no frame record, and
    // there retrace takes x30 as frame 1: the return into the caller that made the call (the cache function that
    // called `__ulock_wait`, no symbol in the recording). lldb has no symbols for it either and goes straight to the
    // record's caller, so retrace may carry extra frames, and only symbol-less ones.
    let mut at = 0;
    let mut extra = Vec::new();
    for (pc, text) in a {
        if at < b.len() && *pc == b[at] { at += 1 } else { extra.push((*pc, text.clone())) }
    }
    assert_eq!(at, b.len(), "lldb's frames, in order, are retrace's frames; retrace:\n{mine}\nlldb: {b:x?}");
    assert!(extra.len() <= 1 && extra.iter().all(|(_, t)| !t.contains(" in ")),
        "retrace's extra frames are at most one with no symbol: {extra:x?}\n{mine}");
    // That one extra frame is the stub's caller: frame 1, the blocked thread's own saved x30 (PAC bits stripped).
    if let Some((pc, _)) = extra.first() {
        assert_eq!(blocked.get(1).map(|f| f.0), Some(*pc), "the extra frame is frame 1:\n{mine}");
        let regs = debug_out(ts, &format!("watch {cell}; continue; regs 0"));
        let x30 = regs.split("x30=").nth(1).and_then(|r| r.split_whitespace().next())
            .and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok())
            .unwrap_or_else(|| panic!("no x30 in `regs 0`:\n{regs}"));
        assert_eq!(*pc, x30 & ((1 << 47) - 1), "frame 1 is thread 0's saved x30 ({x30:#x}):\n{mine}\n{regs}");
    }
}
