//! M58 B6: `summary`, and `retrace debug <trace>` alone: how the run ended, every Rust panic with its backtrace,
//! and for a failing `assert_eq!` the addresses of its two operands, which `last-write` takes. The ledger fixture
//! is recorded as a reader records it; the line of the assert and of the bad store are found by their text.
//! The cargo fixtures skip loudly without cargo.
mod util;
use std::path::Path;
use std::sync::OnceLock;
use util::cargo::{cargo_path, cargo_test_under_retrace, copy_fixture, fresh_dir, readme_recipe_in, saved_after_running,
    LEDGER_BAD_STORE, LEDGER_MANIFEST};

fn json_lines(s: &str) -> Vec<serde_json::Value> {
    s.lines().map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}"))).collect()
}

fn debug(trace: &str, extra: &[&str]) -> (i32, String, String) {
    let mut args = vec!["debug", trace];
    args.extend_from_slice(extra);
    let r = util::run_bounded(util::bin(), &args, &[], 300);
    (r.code.unwrap_or(-1), String::from_utf8_lossy(&r.stdout).into_owned(), r.stderr)
}

/// `debug <trace> --json` with no script: the one summary object.
fn summary_json(trace: &str) -> serde_json::Value {
    let (code, out, err) = debug(trace, &["--json"]);
    assert_eq!(code, 0, "{err}");
    let objs = json_lines(&out);
    assert_eq!(objs.len(), 1, "{out}");
    assert_eq!(objs[0]["ok"], true, "{}", objs[0]);
    objs[0].clone()
}

fn ledger() -> &'static str {
    static T: OnceLock<String> = OnceLock::new();
    T.get_or_init(|| {
        let dir = copy_fixture(LEDGER_MANIFEST, "sum-ledger");
        let (code, out) = readme_recipe_in(&dir, &[]);
        assert_eq!(code, Some(101), "cargo's failing exit:\n{out}");
        let pairs = saved_after_running(&out);
        let (_, saved) = pairs.iter().find(|(r, _)| r.contains("unittests")).unwrap_or_else(|| panic!("{pairs:?}\n{out}"));
        saved.clone()
    })
}

fn line_of(file: &str, text: &str) -> u64 {
    let src = std::fs::read_to_string(Path::new(LEDGER_MANIFEST).parent().unwrap().join("src").join(file)).unwrap();
    1 + src.lines().position(|l| l.contains(text)).unwrap_or_else(|| panic!("{text} not in {file}")) as u64
}

/// The failing test's own assert: the first `assert_eq!(l.balance` after the test's name.
fn failing_assert_line() -> u64 {
    let src = std::fs::read_to_string(Path::new(LEDGER_MANIFEST).parent().unwrap().join("src/lib.rs")).unwrap();
    let lines: Vec<&str> = src.lines().collect();
    let t = lines.iter().position(|l| l.contains("fn settling_four_hundred_leaves_the_rest")).unwrap();
    1 + (t + lines[t..].iter().position(|l| l.contains("assert_eq!(l.balance")).unwrap()) as u64
}

fn skip_no_cargo(name: &str) -> bool {
    if cargo_path().is_none() {
        util::announce(&format!("SKIPPED {name}: cargo did not run. This gate did NOT run."));
        return true;
    }
    false
}

#[test]
fn ledger_summary_names_the_failing_assert() {
    if skip_no_cargo("ledger_summary_names_the_failing_assert") { return; }
    let s = summary_json(ledger());
    assert_eq!(s["ending"]["kind"], "exit", "{s}");
    assert_eq!(s["ending"]["code"], 101, "{s}");
    let panics = s["panics"].as_array().unwrap();
    assert_eq!(panics.len(), 1, "{s}");
    let want = failing_assert_line();
    let frames = panics[0]["frames"].as_array().unwrap();
    assert!(frames.iter().any(|f| f["function"].as_str().is_some_and(|n| n.contains("settling_four_hundred_leaves_the_rest"))
        && f["file"].as_str().is_some_and(|p| p.ends_with("lib.rs")) && f["line"].as_u64() == Some(want)),
        "the failing test at its assert line {want}: {}", panics[0]);
    assert!(panics[0]["assert"]["left"].as_str().unwrap().starts_with("0x"), "{}", panics[0]);
    assert!(panics[0]["assert"]["right"].as_str().unwrap().starts_with("0x"), "{}", panics[0]);
    // libtest captures the test's own stderr and prints a failure's message on stdout, so `left:` is in the stdout tail.
    assert!(s["stdout_tail"].as_str().unwrap().contains("left:"), "{s}");
    assert_eq!(s["truncated"], false, "{s}");
    assert!(s["threads"].as_array().is_some_and(|t| !t.is_empty()), "{s}");
    assert!(s["programs"].as_array().is_some_and(|p| p.len() == 1), "{s}");
}

/// The README loop: the left operand's address and the panic's position, fed to `goto` and `last-write`.
#[test]
fn the_left_operand_leads_to_the_bad_store() {
    if skip_no_cargo("the_left_operand_leads_to_the_bad_store") { return; }
    let t = ledger();
    let s = summary_json(t);
    let p = &s["panics"][0];
    let (n, k) = (p["position"][0].as_u64().unwrap(), p["position"][1].as_u64().unwrap());
    let left = p["assert"]["left"].as_str().unwrap();
    let (code, out, err) = debug(t, &["--json", "--script", &format!("goto {n} {k}; last-write {left} 8")]);
    assert_eq!(code, 0, "{err}");
    let o = json_lines(&out).pop().unwrap();
    assert_eq!(o["found"], true, "{o}");
    assert_eq!(o["kind"], "store", "{o}");
    assert!(o["file"].as_str().unwrap().ends_with("fees.rs"), "{o}");
    assert_eq!(o["line"].as_u64().unwrap(), line_of("fees.rs", LEDGER_BAD_STORE), "{o}");
}

#[test]
fn text_summary_reads() {
    if skip_no_cargo("text_summary_reads") { return; }
    let (code, out, err) = debug(ledger(), &[]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("program: "), "{out}");
    assert!(out.contains("ended: exited with 101"), "{out}");
    assert!(out.contains("panic on thread "), "{out}");
    assert!(out.contains(&format!("lib.rs:{}", failing_assert_line())), "{out}");
    assert!(out.contains("assert_eq! left operand at 0x") && out.contains(", right at 0x"), "{out}");
    assert!(out.contains("threads:"), "{out}");
    assert!(!out.contains("stderr (last"), "an empty stderr prints no block: {out}");
}

#[test]
fn crashy_summary_is_a_crash() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "record: {}", rec.stderr);
    util::discover_crashy_addrs(&trace); // the recording is the crashy one
    let s = summary_json(trace.to_str().unwrap());
    assert_eq!(s["ending"]["kind"], "crash", "{s}");
    assert_eq!(s["ending"]["signal"], "SIGSEGV", "{s}");
    assert_eq!(s["ending"]["fault_address"], "0x4000dead0000", "{s}"); // c/crashy.c's GARBAGE_VA, the corrupted pointer
    assert_eq!(s["panics"].as_array().unwrap().len(), 0, "{s}");
    let (_, text, _) = debug(trace.to_str().unwrap(), &[]);
    assert!(text.contains("ended: killed by SIGSEGV, fault at 0x4000dead0000"), "{text}");
    // Where it stopped, in text too: the crashing thread and position, then its backtrace from the faulting pc, which
    // is in crashy's `main` (`_main`, a C name printed as recorded).
    let pos = &s["ending"]["position"];
    let at = format!("crash on thread {} at ({}, {})", s["ending"]["thread"], pos[0], pos[1]);
    assert!(text.lines().any(|l| l == at), "the ending's line `{at}`:\n{text}");
    let pc = s["ending"]["pc"].as_str().unwrap();
    assert!(text.lines().any(|l| l.trim_start().starts_with(&format!("#0 {pc} in _main+"))), "a #0 frame at {pc} in _main:\n{text}");
    assert_eq!(s["ending"]["function"], "_main", "the JSON function is the name alone: {s}");
    assert!(s["ending"]["offset"].as_u64().is_some_and(|o| o > 0), "and its offset: {s}");
}

#[test]
fn a_passing_run_has_no_panics() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record: {}", rec.stderr);
    let s = summary_json(trace.to_str().unwrap());
    assert_eq!(s["ending"]["kind"], "exit", "{s}");
    assert_eq!(s["ending"]["code"], 0, "{s}");
    assert_eq!(s["panics"].as_array().unwrap().len(), 0, "{s}");
}

/// A panic caught by `catch_unwind` is listed, and the run still exits 0. The flow binary's own trace.
#[test]
fn a_caught_panic_is_listed_but_the_run_exits_zero() {
    if skip_no_cargo("a_caught_panic_is_listed_but_the_run_exits_zero") { return; }
    let dir = fresh_dir("sum-caught");
    let b = cargo_test_under_retrace(LEDGER_MANIFEST, &dir, &["--test", "flow", "a_refused_settlement_is_caught"]);
    assert_eq!(b.code, Some(0), "stderr:\n{}", b.stderr);
    let trace = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path())
        .find(|p| p.file_name().unwrap().to_string_lossy().starts_with("flow-")).unwrap_or_else(|| panic!("no flow trace in {dir:?}"));
    let s = summary_json(trace.to_str().unwrap());
    assert_eq!(s["ending"]["code"], 0, "{s}");
    assert_eq!(s["panics"].as_array().unwrap().len(), 1, "{s}");
    assert!(s["panics"][0].get("assert").is_none(), "a plain panic has no operands: {s}");
}
