//! M44 A5 (spec §3b): skip lines reach a gate log. libtest captures `eprintln!` in a test that
//! passes, and a skip passes, so a skip announced that way never reaches an ordinary gate log
//! (measured at M43's close, Ruling F-4). `util::announce` writes past the capture.
//!
//! M44 final review FW-1: the detector scans every crate's `tests/` directory, recursively, and
//! flags `SKIP` anywhere in the call's first string literal. It had read only the top level of this
//! crate's `tests/`, and only literals that began `SKIP`, so two `[M32 t1 corpus] SKIPPED …` lines
//! in `retrace-core`'s `machmsgband_dyn.rs` escaped it.
use std::path::{Path, PathBuf};

mod util;

/// The macro call the detector looks for, spelled in two halves so that this file's own source
/// holds no call site for the detector to find.
const CALL: &str = concat!("eprintln", "!(");

/// The first token after a call's `(` — across whitespace and newlines, since several calls wrap —
/// if it is a string literal (plain, `r"…"` or `r#"…"#`): its body, escapes left as written.
fn first_literal(rest: &str) -> Option<&str> {
    let rest = rest.trim_start();
    if let Some(body) = rest.strip_prefix('"') {
        let mut esc = false;
        for (i, c) in body.char_indices() {
            match c {
                _ if esc => esc = false,
                '\\' => esc = true,
                '"' => return Some(&body[..i]),
                _ => {}
            }
        }
        return None;
    }
    let raw = rest.strip_prefix('r')?;
    let hashes = raw.len() - raw.trim_start_matches('#').len();
    let body = raw[hashes..].strip_prefix('"')?;
    body.find(&format!("\"{}", "#".repeat(hashes))).map(|end| &body[..end])
}

/// 1-based line numbers where `src` calls `eprintln!` with a first string literal that contains
/// `SKIP`, upper-case, anywhere in it.
fn eprintln_skip_sites(src: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = src[from..].find(CALL) {
        let at = from + i;
        if first_literal(&src[at + CALL.len()..]).is_some_and(|l| l.contains("SKIP")) {
            out.push(src[..at].matches('\n').count() + 1);
        }
        from = at + 1;
    }
    out
}

/// Every `.rs` file under `dir`, recursively (so `tests/util/` is included).
fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn no_test_target_announces_a_skip_through_eprintln() {
    // `CARGO_MANIFEST_DIR` is `…/crates/retrace`; its parent holds every crate.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    for e in std::fs::read_dir(crates).unwrap() {
        let tests = e.unwrap().path().join("tests");
        if tests.is_dir() {
            rs_files(&tests, &mut files);
        }
    }
    files.sort();
    // The three ways the first detector went blind, each pinned by a file it must now reach.
    for must in ["retrace-core/tests/machmsgband_dyn.rs", "retrace/tests/util/mod.rs", "retrace/tests/util/rsp.rs"] {
        assert!(files.iter().any(|p| p.ends_with(must)), "the scan missed {must} under {}", crates.display());
    }
    assert!(files.len() > 120,
        "scanned only {} .rs files under {}/*/tests/ (134 at M44's final review, across six crates)",
        files.len(), crates.display());
    let mut hits = Vec::new();
    for p in &files {
        let src = std::fs::read_to_string(p).unwrap();
        hits.extend(eprintln_skip_sites(&src).into_iter().map(|l| format!("{}:{l}", p.display())));
    }
    assert!(hits.is_empty(),
        "skip lines written with eprintln! — libtest captures them in a passing test; write them past \
         the capture (util::announce, or a local fn of the same body outside this crate):\n{}",
        hits.join("\n"));
}

#[test]
fn the_detector_finds_a_wrapped_eprintln_skip_and_nothing_else() {
    // Its own positive control: the wrapped shape `cpython_e2e` used, and a non-skip line.
    let src = "fn f() {\n    eprintln!(\n        \"SKIPPED x: {REAL} not found\");\n}\n";
    assert_eq!(eprintln_skip_sites(src), [2]);
    assert!(eprintln_skip_sites("eprintln!(\"not a skip\");").is_empty());
    // FW-1: `SKIP` mid-literal, the shape `machmsgband_dyn` used (with its `\` continuation). Then
    // a quote before the `SKIP`, escaped and raw, which a literal cut at its first `"` would miss.
    let mid = "if x {\n} else {\n    eprintln!(\"[M32 t1 corpus] SKIPPED jq: {JQ} not installed. The \\\n        corpus walk ...\");\n}\n";
    assert_eq!(eprintln_skip_sites(mid), [3]);
    assert_eq!(eprintln_skip_sites("eprintln!(\"[\\\"x\\\"] SKIPPED\");"), [1]);
    assert_eq!(eprintln_skip_sites("eprintln!(r#\"[\"t\"] SKIPPED\"#);"), [1]);
    // Negatives: `SKIP` outside the first literal (a variable, then a second literal), and lower case.
    assert!(eprintln_skip_sites("eprintln!(\"{} rows\", SKIP_COUNT);").is_empty());
    assert!(eprintln_skip_sites("eprintln!(\"a \\\"b\\\" c {}\", \"SKIPPED\");").is_empty());
    assert!(eprintln_skip_sites("eprintln!(\"skipped: lower case\");").is_empty());
}

#[test]
fn a_skip_line_control_reaches_the_gate_log() {
    // The close greps an ORDINARY gate log (no --nocapture) for this exact line: M44 §6 item 6.
    util::announce("SKIPLINES CONTROL: util::announce reaches a gate log past libtest's capture");
}
