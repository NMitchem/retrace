//! M58 A4: the README's loop, run the way it is written. The `ledger` example crate is copied into a scratch
//! directory, as the README has a reader copy the installed one, and recorded with the README's recipe word for
//! word (`RETRACE_DIR="$PWD/target/retrace"`, `cargo test --no-fail-fast`). Its failing test is then queried with
//! `retrace lldb <trace> -- --batch -o …`: the backtrace at the panic with file:line, then the last write to the wrong
//! value, by a write watchpoint and `process continue -R`. The value's address comes from the backtrace itself
//! (`assert_failed`'s `left=`), so one recording answers both; a second recording checks the README's fallback, a
//! `{:p}` print, against it. The crash form runs on `crashy`, where lldb's batch mode drops the `-o` commands after a
//! crash stop and runs `-k` ones instead, and on `crashy.c` built with `-g` as C and as C++, for file and line in a
//! C and a C++ compiler's debug info. The README's transcripts are this file's output (run it with `--nocapture`).
//!
//! lldb is not a repo artifact, so each lldb test skips loudly (`util::announce`) when `/usr/bin/lldb` does not run,
//! the ledger tests when cargo does not, and the `-g` test when clang does not. Every run is bounded, so a hang fails
//! instead of stalling.
mod util;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use util::cargo::{cargo_path, copy_fixture, readme_recipe_in, saved_after_running, LEDGER_BAD_STORE, LEDGER_MANIFEST};

/// One lldb session's bound, in seconds. The ledger sessions measured about 5 s.
const BOUND: u64 = 180;
const LLDB: &str = "/usr/bin/lldb";
/// The failing test, as lldb names its frame.
const TEST_FN: &str = "ledger::tests::settling_four_hundred_leaves_the_rest";
const CRASHY_C: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../retrace-guest/c/crashy.c");

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

/// The 1-based number of the first line of `path`, at or after the line containing `after`, that contains `what`.
/// The fixtures' lines are found by their text, never hard-coded.
fn line_in(path: &Path, after: &str, what: &str) -> usize {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let start = src.lines().position(|l| l.contains(after)).unwrap_or_else(|| panic!("{after:?} in {}", path.display()));
    start + 1 + src.lines().skip(start).position(|l| l.contains(what)).unwrap_or_else(|| panic!("{what:?} in {}", path.display()))
}

/// `line_in` for a file of the ledger crate.
fn ledger_line(file: &str, after: &str, what: &str) -> usize {
    line_in(&Path::new(LEDGER_MANIFEST).parent().unwrap().join(file), after, what)
}

/// Whether `line` names `file:n` as lldb prints a frame's location (`at lib.rs:63:9`, or `at lib.rs:63`).
fn at(line: &str, file: &str, n: usize) -> bool {
    let key = format!(" at {file}:{n}");
    line.match_indices(&key).any(|(i, _)| !line[i + key.len()..].starts_with(|c: char| c.is_ascii_digit()))
}

fn hex(s: &str) -> u64 {
    u64::from_str_radix(s.trim().trim_start_matches("0x"), 16).unwrap_or_else(|e| panic!("{s:?}: {e}"))
}

/// The ledger, copied and recorded by the README's recipe with `env` added. Returns the copy, the failing trace's
/// path relative to the copy (`target/retrace/<name>`, as the README's commands give it) and cargo's output.
fn record_ledger(tag: &str, env: &[(&str, &str)]) -> (PathBuf, String, String) {
    let dir = copy_fixture(LEDGER_MANIFEST, tag);
    let (code, out) = readme_recipe_in(&dir, env);
    println!("--- cargo test under retrace, in {}\n{out}", dir.display());
    assert_eq!(code, Some(101), "cargo's failing exit:\n{out}");
    assert!(out.contains("test tests::settling_four_hundred_leaves_the_rest ... FAILED"), "{out}");
    // The failing binary's trace is the one saved under its `Running unittests` line, at the absolute path
    // RETRACE_DIR gave.
    let pairs = saved_after_running(&out);
    let (_, saved) = pairs.iter().find(|(r, _)| r.contains("unittests")).unwrap_or_else(|| panic!("{pairs:?}\n{out}"));
    let name = Path::new(saved).file_name().unwrap().to_str().unwrap().to_string();
    assert!(name.starts_with("ledger-") && name.ends_with(".retrace"), "{saved}");
    assert_eq!(Path::new(saved), dir.join("target/retrace").join(&name), "the saved line names $PWD/target/retrace/<name>");
    let rel = format!("target/retrace/{name}");
    assert!(dir.join(&rel).is_file(), "{}", dir.join(&rel).display());
    (dir, rel, out)
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
    let assert_line = ledger_line("src/lib.rs", "fn settling_four_hundred_leaves_the_rest", "assert_eq!(l.balance");
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

/// The README's second query, at `addr`: the reverse continue must stop at `charge`'s store (`LEDGER_BAD_STORE`),
/// called from the failing test's `settle(400)` (not its `deposit` or `Ledger::new`), on the thread that panicked.
fn last_write(dir: &Path, trace: &str, addr: u64) -> String {
    let write_line = ledger_line("src/fees.rs", "fn charge", LEDGER_BAD_STORE);
    let settle_line = ledger_line("src/lib.rs", "fn settling_four_hundred_leaves_the_rest", "l.settle(400)");
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

/// A fresh scratch directory for one crashy run.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("retrace-lldbbatch-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Record `./<exe>` in `dir` (a crashy build) and return `&g.ptr`, the watch address, as `0x…`.
fn record_crashy(dir: &Path, exe: &str) -> String {
    let r = util::run_in(dir, &["record", &format!("./{exe}")], &[], BOUND);
    assert_eq!(r.code, Some(139), "record {exe}: {}", r.stderr);
    format!("{:#x}", util::discover_crashy_addrs(&dir.join(format!("{exe}.retrace"))).1)
}

/// The README's crash form: `-o 'process continue'`, then `-k` commands down to the store that wrote the pointer.
fn k_chain(ptr: &str) -> Vec<(&'static str, String)> {
    vec![("-o", "process continue".to_string()), ("-k", "bt".into()), ("-k", watch(ptr)),
        ("-k", "process continue -R".into()), ("-k", "bt".into())]
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
    let printed = hex(rec.lines().find_map(|l| l.strip_prefix("balance at ")).unwrap_or_else(|| panic!("the address line:\n{rec}")));
    assert_eq!(assert_left(&bt_at_panic(&dir, &trace)), printed, "in one recording, assert_failed's left= is the printed &l.balance");
    last_write(&dir, &trace, printed);
}

#[test]
fn after_a_crash_batch_mode_runs_k_commands_and_drops_o_ones() {
    if !lldb_runs() {
        util::announce("SKIPPED after_a_crash_batch_mode_runs_k_commands_and_drops_o_ones: /usr/bin/lldb did not run. This gate did NOT run.");
        return;
    }
    let dir = scratch("crashy");
    std::fs::copy(retrace_guest::CRASHY, dir.join("crashy")).unwrap();
    let ptr = record_crashy(&dir, "crashy");
    // The control: an `-o` command after the crash stop never runs, and lldb still exits 0.
    let (code, out, err) = batch(&dir, "crashy.retrace", &[("-o", "process continue".into()), ("-o", "bt".into())]);
    assert_eq!(code, Some(0), "{err}\n{out}");
    assert!(out.contains("stop reason = EXC_BAD_ACCESS"), "{out}");
    assert!(!out.contains("(lldb) bt"), "lldb ran an -o command after the crash stop; the README's -k advice is stale:\n{out}");
    // The README's form: `-k` commands run after the crash, back to the store that wrote the bad pointer.
    let (code, out, err) = batch(&dir, "crashy.retrace", &k_chain(&ptr));
    assert_eq!(code, Some(0), "{err}\n{out}");
    assert!(out.contains("stop reason = EXC_BAD_ACCESS"), "{out}");
    let (_, after) = out.split_once("stop reason = watchpoint 1").unwrap_or_else(|| panic!("no watchpoint stop:\n{out}"));
    assert!(after.lines().any(|l| l.contains("frame #0") && l.contains("crashy`main")), "{out}");
    assert!(after.lines().any(|l| l.contains("str ")), "the store:\n{out}");
    assert_eq!(out.matches("(lldb) bt").count(), 2, "{out}");
}

/// File and line for C and C++: `crashy.c` built with `-g` by `clang` and, as C++, by `clang++ -x c++` (one step, so
/// clang runs dsymutil and the `.dSYM` sits beside the program). The crash stops on the dereference's line and the
/// reverse continue on the store's, each found in the source by its text.
#[test]
fn c_and_cpp_built_with_debug_info_get_file_and_line() {
    if !lldb_runs() {
        util::announce("SKIPPED c_and_cpp_built_with_debug_info_get_file_and_line: /usr/bin/lldb did not run. This gate did NOT run.");
        return;
    }
    let src = Path::new(CRASHY_C);
    let fault_line = line_in(src, "int main", "*(volatile long *)g.ptr = 42;");
    let store_line = line_in(src, "int main", "p[i] = (long)GARBAGE_VA;");
    for (exe, compiler, lang) in [("crashy", "clang", &[][..]), ("crashypp", "clang++", &["-x", "c++"][..])] {
        let dir = scratch(&format!("g-{exe}"));
        let mut args: Vec<&str> = lang.to_vec();
        args.extend(["-g", "-arch", "arm64", "-o", exe, CRASHY_C]);
        let built = Command::new(compiler).args(&args).current_dir(&dir).status();
        if !built.as_ref().is_ok_and(|s| s.success()) {
            util::announce(&format!("SKIPPED c_and_cpp_built_with_debug_info_get_file_and_line ({exe}): {compiler} did not build crashy.c ({built:?}). This row did NOT run."));
            continue;
        }
        let ptr = record_crashy(&dir, exe);
        let (code, out, err) = batch(&dir, &format!("{exe}.retrace"), &k_chain(&ptr));
        assert_eq!(code, Some(0), "{exe}: {err}\n{out}");
        let (crash, after) = out.split_once("stop reason = watchpoint 1").unwrap_or_else(|| panic!("{exe}: no watchpoint stop:\n{out}"));
        assert!(crash.lines().any(|l| l.contains("frame #0") && l.contains(&format!("{exe}`main")) && at(l, "crashy.c", fault_line)),
            "{exe}: the crash at crashy.c:{fault_line}:\n{out}");
        assert!(after.lines().any(|l| l.contains("frame #0") && l.contains(&format!("{exe}`main")) && at(l, "crashy.c", store_line)),
            "{exe}: the store at crashy.c:{store_line}:\n{out}");
    }
}

/// The guide shows these commands; this file runs them. A guide edit that changes one must change this file.
#[test]
fn the_guide_shows_the_commands_this_file_runs() {
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/guide.md")).unwrap();
    let flags = |cmds: &[&str]| cmds.iter().map(|c| format!("-o '{c}'")).collect::<Vec<_>>().join(" ");
    for want in [
        "CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER=\"retrace record\" RETRACE_DIR=\"$PWD/target/retrace\" cargo test --no-fail-fast".to_string(),
        format!("-- --batch {}", flags(&BT)),
        format!("-- --batch {} -o 'watchpoint set expression -w write -s 8 -- 0x", flags(&LAST_WRITE_HEAD)),
        flags(&LAST_WRITE_TAIL),
        "-- --batch -o 'process continue' -k 'bt' -k 'watchpoint set expression -w write -s 8 -- 0x".to_string(),
        "-k 'process continue -R' -k 'bt'".to_string(),
    ] {
        assert!(readme.contains(&want), "docs/guide.md does not show `{want}`");
    }
}
