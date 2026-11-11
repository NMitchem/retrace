//! M58 B5: `last-write` and `goto`. From a failing assert, `last-write <address>` names the store that wrote the
//! value: its function, file and line, thread and backtrace. A kernel write (a `read` into the buffer) is named by its
//! syscall. The ledger and inventory fixtures supply the stores; the line of each bad store is found by the text of
//! its statement, never by a marker. The cargo fixtures skip loudly without cargo.
mod util;
use std::path::Path;
use std::sync::OnceLock;
use util::cargo::{cargo_path, copy_fixture, readme_recipe_in, saved_after_running, INVENTORY_BAD_STORE,
    INVENTORY_MANIFEST, LEDGER_BAD_STORE, LEDGER_MANIFEST};

fn json_lines(s: &str) -> Vec<serde_json::Value> {
    s.lines().map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}"))).collect()
}

fn run(trace: &str, script: &str, json: bool) -> (i32, String, String) {
    let mut args = vec!["debug", trace];
    if json { args.push("--json"); }
    args.extend(["--script", script]);
    let r = util::run_bounded(util::bin(), &args, &[], 300);
    println!("--- retrace {args:?}\n{}\n{}", r.stdout.len(), String::from_utf8_lossy(&r.stdout));
    (r.code.unwrap_or(-1), String::from_utf8_lossy(&r.stdout).into_owned(), r.stderr)
}

/// A failing cargo fixture's trace and the address its test printed.
struct Fixture { trace: String, addr: String }

fn record_fixture(manifest: &str, show: &str, label: &str, tag: &str) -> Fixture {
    let dir = copy_fixture(manifest, tag);
    let (code, out) = readme_recipe_in(&dir, &[(show, "1")]);
    assert_eq!(code, Some(101), "cargo's failing exit:\n{out}");
    let addr = out.lines().find_map(|l| l.split(&format!("{label} at ")).nth(1)).unwrap_or_else(|| panic!("no address line:\n{out}"))
        .split_whitespace().next().unwrap().to_string();
    let pairs = saved_after_running(&out);
    let (_, saved) = pairs.iter().find(|(r, _)| r.contains("unittests")).unwrap_or_else(|| panic!("{pairs:?}\n{out}"));
    Fixture { trace: saved.clone(), addr }
}

fn ledger() -> &'static Fixture {
    static F: OnceLock<Fixture> = OnceLock::new();
    F.get_or_init(|| record_fixture(LEDGER_MANIFEST, "LEDGER_SHOW_ADDR", "balance", "lw-ledger"))
}

fn inventory() -> &'static Fixture {
    static F: OnceLock<Fixture> = OnceLock::new();
    F.get_or_init(|| record_fixture(INVENTORY_MANIFEST, "INVENTORY_SHOW_ADDR", "on_hand", "lw-inventory"))
}

/// The 1-based line of the statement `text` in `<fixture dir>/src/<file>`.
fn line_of(manifest: &str, file: &str, text: &str) -> u64 {
    let src = std::fs::read_to_string(Path::new(manifest).parent().unwrap().join("src").join(file)).unwrap();
    1 + src.lines().position(|l| l.contains(text)).unwrap_or_else(|| panic!("{text} not in {file}")) as u64
}

fn skip_no_cargo(name: &str) -> bool {
    if cargo_path().is_none() {
        util::announce(&format!("SKIPPED {name}: cargo did not run. This gate did NOT run."));
        return true;
    }
    false
}

fn last(objs: &[serde_json::Value]) -> &serde_json::Value { objs.last().unwrap() }

#[test]
fn finds_the_bad_store_in_ledger() {
    if skip_no_cargo("finds_the_bad_store_in_ledger") { return; }
    let f = ledger();
    let script = format!("break rust_panic; continue; last-write {}", f.addr);
    let (code, out, err) = run(&f.trace, &script, true);
    assert_eq!(code, 0, "{err}");
    let o = last(&json_lines(&out)).clone();
    assert_eq!(o["ok"], true, "{o}");
    assert_eq!(o["found"], true, "{o}");
    assert_eq!(o["kind"], "store", "{o}");
    assert!(o["file"].as_str().unwrap().ends_with("fees.rs"), "{o}");
    assert_eq!(o["line"].as_u64().unwrap(), line_of(LEDGER_MANIFEST, "fees.rs", LEDGER_BAD_STORE), "{o}");
    // The name alone, as a frame has it, with the offset into it as its own field (the text line keeps `+0x…`).
    assert_eq!(o["function"], "ledger::fees::charge", "{o}");
    assert_eq!(o["offset"], o["frames"][0]["offset"], "{o}");
    assert!(o["offset"].as_u64().is_some_and(|x| x > 0), "{o}");
    assert!(o["value_before"].as_str().unwrap().len() >= 2 && o["value_after"].is_string(), "{o}");
    assert_ne!(o["value_before"], o["value_after"], "{o}");
    assert!(o["frames"].as_array().unwrap().len() >= 2, "{o}");
    let (code, text, err) = run(&f.trace, &script, false);
    assert_eq!(code, 0, "{err}");
    let l = text.lines().find(|l| l.starts_with("last write to ")).unwrap_or_else(|| panic!("{text}"));
    assert!(l.contains(&format!("({} bytes): store at 0x", 8)) && l.contains("fees.rs:") && l.contains(" on thread "), "{l}");
    assert!(text.lines().any(|l| l.starts_with("#0 ")), "{text}");
}

#[test]
fn chains() {
    if skip_no_cargo("chains") { return; }
    let f = ledger();
    let script = format!("break rust_panic; continue; last-write {a}; last-write {a}", a = f.addr);
    let (code, out, err) = run(&f.trace, &script, true);
    assert_eq!(code, 0, "{err}");
    let objs = json_lines(&out);
    let (a, b) = (&objs[objs.len() - 2], last(&objs));
    assert_eq!((a["found"].clone(), b["found"].clone()), (true.into(), true.into()), "{a}\n{b}");
    let pos = |o: &serde_json::Value| (o["position"][0].as_u64().unwrap(), o["position"][1].as_u64().unwrap());
    assert!(pos(b) < pos(a), "the second write is earlier: {a}\n{b}");
    // The write before the bad store is the test's `deposit(500)`: it leaves 500 (little-endian) on the deposit's line.
    assert!(b["file"].as_str().unwrap().ends_with("lib.rs"), "the deposit: {b}");
    assert_eq!(b["line"].as_u64(), Some(line_of(LEDGER_MANIFEST, "lib.rs", "self.balance += amount;")), "the deposit's line: {b}");
    assert_eq!(b["value_after"], "f401000000000000", "the deposit leaves 500: {b}");
    assert_eq!(a["value_before"], "f401000000000000", "the bad store starts from 500: {a}");
}

#[test]
fn thread_named() {
    if skip_no_cargo("thread_named") { return; }
    let f = inventory();
    let (code, out, err) = run(&f.trace, &format!("break rust_panic; continue; last-write {} 4", f.addr), true);
    assert_eq!(code, 0, "{err}");
    let o = last(&json_lines(&out)).clone();
    assert_eq!(o["kind"], "store", "{o}");
    assert!(o["file"].as_str().unwrap().ends_with("restock.rs"), "{o}");
    assert_eq!(o["line"].as_u64().unwrap(), line_of(INVENTORY_MANIFEST, "restock.rs", INVENTORY_BAD_STORE), "{o}");
    assert_ne!(o["thread"].as_u64().unwrap(), 0, "the worker wrote it: {o}");
}

/// `buf` of the `bigread` guest, from its own symbol table: a 96 KiB read lands there.
fn bigread_buf() -> u64 {
    let r = std::process::Command::new("/usr/bin/nm").arg(retrace_guest::BIGREAD).output().expect("nm");
    let s = String::from_utf8_lossy(&r.stdout).into_owned();
    let a = s.lines().find_map(|l| { let mut w = l.split_whitespace(); let a = w.next()?; (w.last() == Some("buf")).then(|| a.to_string()) })
        .unwrap_or_else(|| panic!("no buf in nm:\n{s}"));
    u64::from_str_radix(a.trim_start_matches("0x"), 16).unwrap()
}

#[test]
fn kernel_write_is_a_syscall() {
    let (rec, trace) = util::record(retrace_guest::BIGREAD);
    assert_eq!(rec.code, 0, "{}", rec.stderr);
    let buf = format!("{:#x}", bigread_buf());
    let ts = trace.to_str().unwrap();
    let (code, out, err) = run(ts, &format!("continue; last-write {buf}"), true);
    assert_eq!(code, 0, "{err}");
    let o = last(&json_lines(&out)).clone();
    assert_eq!(o["found"], true, "{o}");
    assert_eq!(o["kind"], "syscall", "{o}");
    let name = o["syscall"]["name"].as_str().unwrap();
    assert!(name == "read" || name == "read_nocancel", "{o}");
    assert_eq!(o["syscall"]["number"], 3, "{o}");
    assert!(o["value_before"].is_null() && o["value_after"].as_str().unwrap().starts_with("41"), "{o}");
    assert_eq!(o["position"][1], 0, "{o}");
    let (_, text, _) = run(ts, &format!("continue; last-write {buf}"), false);
    assert!(text.lines().any(|l| l.starts_with(&format!("last write to {buf}: syscall read (3) on thread 0, ("))), "{text}");
}

#[test]
fn none_before_leaves_the_cursor() {
    // One instruction in, so the search runs back from here and not from the end (see the next test).
    let (rec, trace) = util::record(retrace_guest::BIGREAD);
    assert_eq!(rec.code, 0, "{}", rec.stderr);
    let buf = format!("{:#x}", bigread_buf());
    let (code, out, err) = run(trace.to_str().unwrap(), &format!("stepi 1; where; last-write {buf}; where"), true);
    assert_eq!(code, 0, "{err}");
    let objs = json_lines(&out);
    let o = &objs[2];
    assert_eq!(o["found"], false, "{o}");
    assert_eq!(o["reason"], "start", "{o}");
    assert_eq!(objs[1]["position"], o["position"]);
    assert_eq!(objs[1]["position"], objs[3]["position"]);
    let (_, text, _) = run(trace.to_str().unwrap(), &format!("stepi 1; last-write {buf}"), false);
    assert!(text.contains(&format!("no earlier write to {buf}: it holds what it held when the program started")), "{text}");
}

#[test]
fn at_the_start_it_searches_back_from_the_end() {
    // Nothing comes before the start of a recording, so `last-write` there runs back from its end and finds the
    // `read` that fills the buffer, as it would after `summary` or a `goto` to the end.
    let (rec, trace) = util::record(retrace_guest::BIGREAD);
    assert_eq!(rec.code, 0, "{}", rec.stderr);
    let buf = format!("{:#x}", bigread_buf());
    let (code, text, err) = run(trace.to_str().unwrap(), &format!("last-write {buf}"), false);
    assert_eq!(code, 0, "{err}");
    assert!(text.contains("at the start of the recording: searching back from its end"), "{text}");
    assert!(text.lines().any(|l| l.starts_with(&format!("last write to {buf}: syscall read (3) on thread 0, ("))), "{text}");
    let (code, out, err) = run(trace.to_str().unwrap(), &format!("last-write {buf}"), true);
    assert_eq!(code, 0, "{err}");
    let o = last(&json_lines(&out)).clone();
    assert_eq!(o["found"], true, "{o}");
    assert_eq!(o["kind"], "syscall", "{o}");
}

#[test]
fn slots_full() {
    let (rec, trace) = util::record(retrace_guest::BIGREAD);
    assert_eq!(rec.code, 0, "{}", rec.stderr);
    let (code, _, err) = run(trace.to_str().unwrap(), "watch 0x1000; watch 0x1008; watch 0x1010; watch 0x1018; last-write 0x1020", false);
    assert_eq!(code, 5);
    assert!(err.contains("last-write needs a free watchpoint slot; 4 are armed (hardware limit: DBGWVR0-3)"), "{err}");
}

#[test]
fn goto_lands_where_an_earlier_command_ended() {
    let (rec, trace) = util::record(retrace_guest::BIGREAD);
    assert_eq!(rec.code, 0, "{}", rec.stderr);
    let ts = trace.to_str().unwrap();
    let (code, out, err) = run(ts, "goto 3 5; where; goto (2, 0); where; goto 4:1; where", true);
    assert_eq!(code, 0, "{err}");
    let objs = json_lines(&out);
    assert_eq!(objs[1]["position"], serde_json::json!([3, 5]), "{}", objs[1]);
    assert_eq!(objs[3]["position"], serde_json::json!([2, 0]));
    assert_eq!(objs[5]["position"], serde_json::json!([4, 1]));
    let (code, _, err) = run(ts, "goto 999999 0", false);
    assert_eq!(code, 5);
    assert!(err.contains("last position"), "{err}");
}

/// A user breakpoint on the instruction after the store must not turn `value_after` into null: the verifying step
/// runs with the user's breakpoints off.
#[test]
fn user_breakpoint_after_the_store_keeps_value_after() {
    if skip_no_cargo("user_breakpoint_after_the_store_keeps_value_after") { return; }
    let f = ledger();
    let base = format!("break rust_panic; continue; last-write {}", f.addr);
    let first = last(&json_lines(&run(&f.trace, &base, true).1)).clone();
    let pc = u64::from_str_radix(first["pc"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
    let want = first["value_after"].clone();
    assert!(want.is_string(), "{first}");
    let script = format!("break rust_panic; continue; break {:#x}; last-write {}; where", pc + 4, f.addr);
    let (code, out, err) = run(&f.trace, &script, true);
    assert_eq!(code, 0, "{err}");
    let objs = json_lines(&out);
    let o = &objs[objs.len() - 2];
    assert_eq!(o["value_after"], want, "{o}");
    assert_eq!(o["position"], first["position"], "{o}");
}
