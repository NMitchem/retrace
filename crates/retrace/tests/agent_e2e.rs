//! M58 B7: the agent loop end to end, as the README shows it: record `cargo test` under retrace, ask
//! `retrace debug <trace>` what happened, then `goto` the panic and `last-write` the assert's left operand. Each
//! command runs as its own subprocess in the README's order and its stdout is saved under the SDD ledger's
//! `readme-loop/`, so the README's transcripts come from a real run. Skips loudly without cargo.
mod util;
use std::path::{Path, PathBuf};
use util::cargo::{cargo_path, copy_fixture, readme_recipe_in, remove_fixture_copy, saved_after_running,
    INVENTORY_BAD_STORE, INVENTORY_MANIFEST, LEDGER_BAD_STORE, LEDGER_MANIFEST};

fn json_lines(s: &str) -> Vec<serde_json::Value> {
    s.lines().map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}"))).collect()
}

fn out_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.superpowers/sdd/2026-11-10-retrace-m58-agent/readme-loop")
}

fn save(name: &str, body: &str) {
    let d = out_dir();
    let r = std::fs::create_dir_all(&d).and_then(|_| std::fs::write(d.join(name), body));
    if let Err(e) = r { util::announce(&format!("NOTE: could not save {name} under {}: {e}", d.display())); }
}

/// One `retrace debug` run; returns stdout, asserting a zero exit.
fn debug(trace: &str, extra: &[&str]) -> String {
    let mut args = vec!["debug", trace];
    args.extend_from_slice(extra);
    let r = util::run_bounded(util::bin(), &args, &[], 300);
    assert_eq!(r.code, Some(0), "debug {args:?}: {}", r.stderr);
    String::from_utf8_lossy(&r.stdout).into_owned()
}

fn line_of(manifest: &str, file: &str, text: &str) -> u64 {
    let src = std::fs::read_to_string(Path::new(manifest).parent().unwrap().join("src").join(file)).unwrap();
    1 + src.lines().position(|l| l.contains(text)).unwrap_or_else(|| panic!("{text} not in {file}")) as u64
}

/// Runs the loop; returns the last-write object and the failing panic.
fn the_loop(manifest: &str, tag: &str, name: &str, size: u32, file: &str, bad: &str) -> (serde_json::Value, serde_json::Value) {
    let dir = copy_fixture(manifest, tag);
    let (code, rec) = readme_recipe_in(&dir, &[]);
    assert_eq!(code, Some(101), "cargo's failing exit:\n{rec}");
    save(&format!("{name}-1-record.txt"), &rec);
    let pairs = saved_after_running(&rec);
    let mut found = None;
    for (_, t) in &pairs {
        let s = json_lines(&debug(t, &["--json"])).pop().unwrap();
        if s["panics"].as_array().is_some_and(|p| p.iter().any(|x| x.get("assert").is_some())) { found = Some(t.clone()); break; }
    }
    let trace = found.unwrap_or_else(|| panic!("no trace with an assert panic: {pairs:?}"));
    let rel = |t: &str| Path::new(t).strip_prefix(&dir).map(|p| p.display().to_string()).unwrap_or_else(|_| t.to_string());
    let mut cmds = vec![format!("cd {}", dir.file_name().unwrap().to_string_lossy()),
        "CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER=\"retrace record\" RETRACE_DIR=\"$PWD/target/retrace\" cargo test --offline --no-fail-fast".to_string()];
    let (text, sj) = (debug(&trace, &[]), debug(&trace, &["--json"]));
    save(&format!("{name}-2-summary.txt"), &text);
    save(&format!("{name}-3-summary-json.txt"), &sj);
    cmds.push(format!("retrace debug {}", rel(&trace)));
    cmds.push(format!("retrace debug {} --json", rel(&trace)));
    let s = json_lines(&sj).pop().unwrap();
    let p = s["panics"].as_array().unwrap().iter().find(|x| x.get("assert").is_some()).unwrap().clone();
    let (n, k) = (p["position"][0].as_u64().unwrap(), p["position"][1].as_u64().unwrap());
    let left = p["assert"]["left"].as_str().unwrap();
    let script = format!("goto {n} {k}; last-write {left} {size}");
    let lw_json = debug(&trace, &["--json", "--script", &script]);
    let lw_text = debug(&trace, &["--script", &script]);
    save(&format!("{name}-4-lastwrite-json.txt"), &lw_json);
    save(&format!("{name}-4-lastwrite.txt"), &lw_text);
    cmds.push(format!("retrace debug {} --json --script '{script}'", rel(&trace)));
    cmds.push(format!("retrace debug {} --script '{script}'", rel(&trace)));
    save(&format!("{name}-commands.txt"), &(cmds.join("\n") + "\n"));
    let objs = json_lines(&lw_json);
    let o = objs.iter().find(|o| o.get("found").is_some()).unwrap_or_else(|| panic!("no last-write line:\n{lw_json}")).clone();
    assert_eq!(o["found"], true, "{o}");
    assert_eq!(o["kind"], "store", "{o}");
    assert!(o["file"].as_str().unwrap().ends_with(file), "{o}");
    assert_eq!(o["line"].as_u64().unwrap(), line_of(manifest, file, bad), "{o}");
    remove_fixture_copy(&dir);
    (o, p)
}

fn skip_no_cargo(name: &str) -> bool {
    if cargo_path().is_none() {
        util::announce(&format!("SKIPPED {name}: cargo did not run. This gate did NOT run."));
        return true;
    }
    false
}

#[test]
fn the_readme_loop_on_ledger() {
    if skip_no_cargo("the_readme_loop_on_ledger") { return; }
    the_loop(LEDGER_MANIFEST, "agent-ledger", "ledger", 8, "fees.rs", LEDGER_BAD_STORE);
}

#[test]
fn the_same_loop_on_inventory() {
    if skip_no_cargo("the_same_loop_on_inventory") { return; }
    let (o, p) = the_loop(INVENTORY_MANIFEST, "agent-inventory", "inventory", 4, "restock.rs", INVENTORY_BAD_STORE);
    // The worker wrote the slot; the panic is on the test thread.
    let w = o["thread"].as_u64().unwrap();
    let t = p["thread"].as_u64().expect("the summary's panic names its thread");
    assert_ne!(w, 0, "{o}");
    assert_ne!(w, t, "the writer is not the panicking thread: {o} / {p}");
    assert!(o["frames"].as_array().unwrap().iter().any(|f| f["function"].as_str().is_some_and(|n| n.contains("Shelf::receive::{{closure}}"))),
        "the writer's frames include the spawned closure: {o}");
}
