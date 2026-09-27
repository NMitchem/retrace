//! M44 A5 (spec §3b): skip lines reach a gate log. libtest captures `eprintln!` in a test that
//! passes, and a skip passes, so a skip announced that way never reaches an ordinary gate log
//! (measured at M43's close, Ruling F-4). `util::announce` writes past the capture.
mod util;

/// 1-based line numbers where `src` has `eprintln!(` followed — across whitespace and newlines,
/// since several calls wrap — by a string literal beginning `SKIP`.
fn eprintln_skip_sites(src: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = src[from..].find("eprintln!(") {
        let at = from + i;
        let rest = src[at + "eprintln!(".len()..].trim_start();
        if rest.starts_with("\"SKIP") || rest.starts_with("r\"SKIP") || rest.starts_with("r#\"SKIP") {
            out.push(src[..at].matches('\n').count() + 1);
        }
        from = at + 1;
    }
    out
}

#[test]
fn no_test_target_announces_a_skip_through_eprintln() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests");
    let (mut hits, mut scanned) = (Vec::new(), 0);
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "rs") {
            scanned += 1;
            let src = std::fs::read_to_string(&p).unwrap();
            hits.extend(eprintln_skip_sites(&src).into_iter().map(|l| format!("{}:{l}", p.display())));
        }
    }
    assert!(scanned > 50, "scanned only {scanned} files in {dir}");
    assert!(hits.is_empty(),
        "skip lines written with eprintln! — libtest captures them in a passing test; use util::announce:\n{}",
        hits.join("\n"));
}

#[test]
fn the_detector_finds_a_wrapped_eprintln_skip_and_nothing_else() {
    // Its own positive control: the wrapped shape `cpython_e2e` used, and a non-skip line.
    let src = "fn f() {\n    eprintln!(\n        \"SKIPPED x: {REAL} not found\");\n}\n";
    assert_eq!(eprintln_skip_sites(src), [2]);
    assert!(eprintln_skip_sites("eprintln!(\"not a skip\");").is_empty());
}

#[test]
fn a_skip_line_control_reaches_the_gate_log() {
    // The close greps an ORDINARY gate log (no --nocapture) for this exact line: M44 §6 item 6.
    util::announce("SKIPLINES CONTROL: util::announce reaches a gate log past libtest's capture");
}
