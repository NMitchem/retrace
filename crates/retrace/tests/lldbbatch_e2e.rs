//! M58 A4: the README's loop, run the way it is written. The `ledger` example crate is copied into a scratch
//! directory, as the README has a reader copy the installed one, and recorded with the README's recipe word for
//! word (`RETRACE_DIR=target/retrace` relative to the package, `cargo test --no-fail-fast`). Its failing test is
//! then queried with `retrace lldb <trace> -- --batch -o …`: the backtrace at the panic with file:line, then the last
//! write to the wrong value, by a write watchpoint and `process continue -R`. The value's address comes from the
//! backtrace itself (`assert_failed`'s `left=`), so one recording answers both; a second recording checks the
//! README's fallback, a `{:p}` print, against it. A last test runs the crash form on `crashy`, where lldb's batch mode
//! drops the `-o` commands after a crash stop and runs `-k` ones instead. The README's transcripts are this file's
//! output (run it with `--nocapture` to see them).
//!
//! lldb is not a repo artifact, so each lldb test skips loudly (`util::announce`) when `/usr/bin/lldb` does not run,
//! and the ledger tests skip loudly when cargo does not. Every run is bounded, so a hang fails instead of stalling.
mod util;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use util::cargo::{cargo_path, copy_fixture, readme_recipe_in, saved_after_running, LEDGER_MANIFEST};

/// One lldb session's bound, in seconds. The ledger sessions measured about 5 s.
const BOUND: u64 = 180;
const LLDB: &str = "/usr/bin/lldb";
/// The failing test, as lldb names its frame.
const TEST_FN: &str = "ledger::tests::settling_four_hundred_leaves_the_rest";

/// The README's two queries, as the `-o` commands after `retrace lldb <trace> -- --batch`. The watchpoint's
/// address is the run's own, read from its output.
const BT: [&str; 3] = ["breakpoint set -n rust_panic", "process continue", "bt 12"];
const LAST_WRITE_HEAD: [&str; 2] = ["breakpoint set -n rust_panic", "process continue"];
const LAST_WRITE_TAIL: [&str; 2] = ["process continue -R", "bt 4"];
fn watch(addr: &str) -> String { format!("watchpoint set expression -w write -s 8 -- {addr}") }

fn lldb_runs() -> bool {
    Command::new(LLDB).arg("--version").stdin(Stdio::null()).output().ok()
        .is_some_and(|o| o.status.success() && o.stdout.starts_with(b"lldb-"))
}

macro_rules! need {
    ($name:literal) => {
        if !lldb_runs() {
            util::announce(concat!("SKIPPED ", $name, ": /usr/bin/lldb did not run. This gate did NOT run."));
            return;
        }
        if cargo_path().is_none() {
            util::announce(concat!("SKIPPED ", $name, ": cargo is not runnable. This gate did NOT run."));
            return;
        }
    };
}

/// The 1-based number of the first line of `file` (under the ledger crate), at or after the line containing
/// `after`, that contains `what`. The fixture's lines are found, never hard-coded.
fn line_of(file: &str, after: &str, what: &str) -> usize {
    let p = Path::new(LEDGER_MANIFEST).parent().unwrap().join(file);
    let src = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let start = src.lines().position(|l| l.contains(after)).unwrap_or_else(|| panic!("{after:?} in {file}"));
    start + 1 + src.lines().skip(start).position(|l| l.contains(what)).unwrap_or_else(|| panic!("{what:?} in {file}"))
}

/// Whether `line` names `file:n` as lldb prints a frame's location (`at lib.rs:63:9`, or `at lib.rs:63`).
fn at(line: &str, file: &str, n: usize) -> bool {
    let key = format!(" at {file}:{n}");
    line.match_indices(&key).any(|(i, _)| !line[i + key.len()..].starts_with(|c: char| c.is_ascii_digit()))
}

fn hex(s: &str) -> u64 {
    u64::from_str_radix(s.trim().trim_start_matches("0x"), 16).unwrap_or_else(|e| panic!("{s:?}: {e}"))
}

/// The ledger, copied and recorded by the README's recipe with `env` added. Returns the copy, the trace's path as
/// the recipe printed it (relative to the copy) and cargo's output.
fn record_ledger(tag: &str, env: &[(&str, &str)]) -> (PathBuf, String, String) {
    let dir = copy_fixture(LEDGER_MANIFEST, tag);
    let (code, out) = readme_recipe_in(&dir, env);
    println!("--- cargo test under retrace, in {}\n{out}", dir.display());
    assert_eq!(code, Some(101), "cargo's failing exit:\n{out}");
    assert!(out.contains("test tests::settling_four_hundred_leaves_the_rest ... FAILED"), "{out}");
    // The failing binary's trace is the one saved under its `Running unittests` line, at the relative path given.
    let pairs = saved_after_running(&out);
    let (_, trace) = pairs.iter().find(|(r, _)| r.contains("unittests")).unwrap_or_else(|| panic!("{pairs:?}\n{out}"));
    assert!(trace.starts_with("target/retrace/ledger-") && trace.ends_with(".retrace"), "{trace}");
    assert!(dir.join(trace).is_file(), "{}", dir.join(trace).display());
    (dir, trace.clone(), out)
}

/// The README's recording, made once for the tests that share it.
fn plain() -> &'static (PathBuf, String, String) {
    static R: OnceLock<(PathBuf, String, String)> = OnceLock::new();
    R.get_or_init(|| record_ledger("plain", &[]))
}

/// `retrace lldb <trace> -- --batch <flag> <cmd> …` in `dir`, bounded. Returns lldb's exit code, stdout and stderr.
fn batch(dir: &Path, trace: &str, cmds: &[(&str, String)]) -> (Option<i32>, String, String) {
    let mut args: Vec<String> = vec!["lldb".into(), trace.into(), "--".into(), "--batch".into()];
    for (flag, c) in cmds { args.push(flag.to_string()); args.push(c.clone()); }
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let r = util::run_in(dir, &a, &[], BOUND);
    let out = String::from_utf8_lossy(&r.stdout).into_owned();
    println!("--- retrace {}\n{out}", args.join(" "));
    (r.code, out, r.stderr)
}

fn o(cmds: &[&str]) -> Vec<(&'static str, String)> { cmds.iter().map(|c| ("-o", c.to_string())).collect() }

/// The README's first query. Asserts the stop and the failing test's frame at its `assert_eq!` line, and returns
/// lldb's output.
fn bt_at_panic(dir: &Path, trace: &str) -> String {
    let assert_line = line_of("src/lib.rs", "fn settling_four_hundred_leaves_the_rest", "assert_eq!(l.balance");
    let (code, out, err) = batch(dir, trace, &o(&BT));
    assert_eq!(code, Some(0), "lldb's exit; stderr:\n{err}\nstdout:\n{out}");
    assert!(out.contains("stop reason = breakpoint 1.1"), "the panic's breakpoint stop:\n{out}");
    assert!(out.contains("rust_panic"), "{out}");
    // lldb's frame format prints the file's base name, `lib.rs`, not the full path.
    assert!(out.lines().any(|l| l.contains(TEST_FN) && at(l, "lib.rs", assert_line)),
        "a frame of the failing test at lib.rs:{assert_line}:\n{out}");
    out
}

/// The `left=` argument of the `assert_failed` frame: the address of the asserted value, `&l.balance`.
fn assert_left(bt: &str) -> u64 {
    let l = bt.lines().find(|l| l.contains("core::panicking::assert_failed::")).unwrap_or_else(|| panic!("no assert_failed frame:\n{bt}"));
    let v = l.split("left=").nth(1).unwrap_or_else(|| panic!("no left= in {l}"));
    hex(v.split([',', ')']).next().unwrap())
}

/// The README's second query, at `addr`: the reverse continue must stop at `charge`'s store on its `// M58-WRITE`
/// line, called from the failing test's `settle(400)` (not its `deposit` or `Ledger::new`), on the thread that
/// panicked.
fn last_write(dir: &Path, trace: &str, addr: u64) -> String {
    let write_line = line_of("src/fees.rs", "fn charge", "// M58-WRITE");
    let settle_line = line_of("src/lib.rs", "fn settling_four_hundred_leaves_the_rest", "l.settle(400)");
    let mut cmds = o(&LAST_WRITE_HEAD);
    cmds.push(("-o", watch(&format!("{addr:#x}"))));
    cmds.extend(o(&LAST_WRITE_TAIL));
    let (code, out, err) = batch(dir, trace, &cmds);
    assert_eq!(code, Some(0), "lldb's exit; stderr:\n{err}\nstdout:\n{out}");
    let (_, after) = out.split_once("stop reason = watchpoint 1").unwrap_or_else(|| panic!("no watchpoint stop:\n{out}"));
    let frame0 = after.lines().find(|l| l.contains("frame #0")).unwrap_or_else(|| panic!("{out}"));
    assert!(frame0.contains("ledger::fees::charge") && at(frame0, "fees.rs", write_line), "the store at fees.rs:{write_line}:\n{out}");
    assert!(after.lines().any(|l| l.contains(TEST_FN) && at(l, "lib.rs", settle_line)),
        "the bt reaches the test's settle call at lib.rs:{settle_line}:\n{out}");
    assert_eq!(stop_thread(&out, "watchpoint 1"), stop_thread(&out, "breakpoint 1.1"), "{out}");
    out
}

/// The thread lldb named on the stop whose reason starts with `reason` (`* thread #2, stop reason = …`).
fn stop_thread(out: &str, reason: &str) -> String {
    let l = out.lines().find(|l| l.contains(&format!("stop reason = {reason}"))).unwrap_or_else(|| panic!("no {reason:?} stop:\n{out}"));
    l.split(',').next().unwrap().trim().trim_start_matches("* ").to_string()
}

#[test]
fn bt_at_the_panic_names_the_failing_test_and_its_line() {
    need!("bt_at_the_panic_names_the_failing_test_and_its_line");
    let (dir, trace, _) = plain();
    bt_at_panic(dir, trace);
}

#[test]
fn the_asserted_value_s_last_write_is_found_from_the_same_recording() {
    need!("the_asserted_value_s_last_write_is_found_from_the_same_recording");
    let (dir, trace, _) = plain();
    let addr = assert_left(&bt_at_panic(dir, trace));
    last_write(dir, trace, addr);
}

#[test]
fn a_printed_address_is_the_same_value_and_finds_the_same_write() {
    need!("a_printed_address_is_the_same_value_and_finds_the_same_write");
    // The README's fallback for a value no assert names: the test prints its address ({:p}) and the suite is
    // recorded again. The example crate prints it when LEDGER_SHOW_ADDR=1.
    let (dir, trace, rec) = record_ledger("printed", &[("LEDGER_SHOW_ADDR", "1")]);
    let printed = hex(rec.lines().find_map(|l| l.strip_prefix("M58-ADDR balance ")).unwrap_or_else(|| panic!("the address line:\n{rec}")));
    assert_eq!(assert_left(&bt_at_panic(&dir, &trace)), printed, "assert_failed's left= is &l.balance");
    last_write(&dir, &trace, printed);
}

#[test]
fn after_a_crash_batch_mode_runs_k_commands_and_drops_o_ones() {
    if !lldb_runs() {
        util::announce("SKIPPED after_a_crash_batch_mode_runs_k_commands_and_drops_o_ones: /usr/bin/lldb did not run. This gate did NOT run.");
        return;
    }
    let dir = std::env::temp_dir().join(format!("retrace-lldbbatch-{}-crashy", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(retrace_guest::CRASHY, dir.join("crashy")).unwrap();
    let r = util::run_in(&dir, &["record", "./crashy"], &[], BOUND);
    assert_eq!(r.code, Some(139), "record crashy: {}", r.stderr);
    let ptr = format!("{:#x}", util::discover_crashy_addrs(&dir.join("crashy.retrace")).1);
    // The control: an `-o` command after the crash stop never runs, and lldb still exits 0.
    let (code, out, err) = batch(&dir, "crashy.retrace", &[("-o", "process continue".into()), ("-o", "bt".into())]);
    assert_eq!(code, Some(0), "{err}\n{out}");
    assert!(out.contains("stop reason = EXC_BAD_ACCESS"), "{out}");
    assert!(!out.contains("(lldb) bt"), "lldb ran an -o command after the crash stop; the README's -k advice is stale:\n{out}");
    // The README's form: `-k` commands run after the crash, back to the store that wrote the bad pointer.
    let cmds = [("-o", "process continue".to_string()), ("-k", "bt".into()), ("-k", watch(&ptr)),
        ("-k", "process continue -R".into()), ("-k", "bt".into())];
    let (code, out, err) = batch(&dir, "crashy.retrace", &cmds);
    assert_eq!(code, Some(0), "{err}\n{out}");
    assert!(out.contains("stop reason = EXC_BAD_ACCESS"), "{out}");
    let (_, after) = out.split_once("stop reason = watchpoint 1").unwrap_or_else(|| panic!("no watchpoint stop:\n{out}"));
    assert!(after.lines().any(|l| l.contains("frame #0") && l.contains("crashy`main")), "{out}");
    assert!(after.lines().any(|l| l.contains("str ")), "the store:\n{out}");
    assert_eq!(out.matches("(lldb) bt").count(), 2, "{out}");
}

/// The README shows these commands; this file runs them. A README edit that changes one must change this file.
#[test]
fn the_readme_shows_the_commands_this_file_runs() {
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md")).unwrap();
    let flags = |cmds: &[&str]| cmds.iter().map(|c| format!("-o '{c}'")).collect::<Vec<_>>().join(" ");
    for want in [
        "CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER=\"retrace record\" RETRACE_DIR=target/retrace cargo test --no-fail-fast".to_string(),
        format!("-- --batch {}", flags(&BT)),
        format!("-- --batch {} -o 'watchpoint set expression -w write -s 8 -- 0x", flags(&LAST_WRITE_HEAD)),
        flags(&LAST_WRITE_TAIL),
        "-- --batch -o 'process continue' -k 'bt' -k 'watchpoint set expression -w write -s 8 -- 0x".to_string(),
        "-k 'process continue -R' -k 'bt'".to_string(),
    ] {
        assert!(readme.contains(&want), "README.md does not show `{want}`");
    }
}
