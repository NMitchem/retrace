//! M50 §3c (B4, R6): the `refuse!` audit. A deep shared helper refuses a guest's call by panicking
//! through `retrace_arch::refuse!`, whose payload carries the wall marker. Each site is a deliberate,
//! reviewed decision, so the set of them is committed in `refuse-sites.txt` beside this file: adding,
//! moving or removing one without editing the list fails here, and M51 and M53 work the list down.
//! Nothing but `wall.rs` may write the marker either: a hand-built marked string would be a wall that
//! nobody classified at its site.
use std::collections::BTreeMap;
use std::path::Path;

mod audit;
use audit::{repo, rust_files, source_lines};

/// A message as the list spells it: the literal up to its first format argument, escape, line
/// continuation or closing quote, at most 40 characters, trimmed. The list was first written from t0's
/// `refuse-list.tsv` by the same reduction.
fn prefix(literal: &str) -> String {
    literal.chars().take_while(|&c| c != '{' && c != '\\' && c != '"').take(40).collect::<String>().trim().to_owned()
}

/// Every `refuse!` call in the non-test source of every crate, as `<file>\t<fn>\t<Category>\t<prefix>`.
/// A site writes `refuse!(<Category>, "` on one line; the audit refuses any other spelling, so a
/// site cannot hide from it by formatting.
fn sites_in_source() -> Vec<String> {
    let root = repo();
    let mut files = Vec::new();
    for krate in std::fs::read_dir(root.join("crates")).unwrap() {
        rust_files(&krate.unwrap().path().join("src"), &mut files);
    }
    let mut sites = Vec::new();
    // The test module ends the source a guest can reach (`source_lines` stops there).
    for l in source_lines(&root, &files) {
        if l.rel == "crates/retrace-arch/src/wall.rs" { continue; } // the macro's own definition
        let (rel, func) = (&l.rel, &l.func);
        let mut rest = l.text.as_str();
        while let Some(at) = rest.find("refuse!(") {
            rest = &rest[at + "refuse!(".len()..];
            let (category, msg) = rest.split_once(',')
                .unwrap_or_else(|| panic!("{rel}:{}: write `refuse!(<Category>, \"…\"` on one line", l.no));
            let literal = msg.trim_start().strip_prefix('"')
                .unwrap_or_else(|| panic!("{rel}:{}: a refuse! message starts with its literal on the refuse! line", l.no));
            sites.push(format!("{rel}\t{func}\t{}\t{}", category.trim(), prefix(literal)));
        }
    }
    sites.sort();
    sites
}

fn counts(v: &[String]) -> BTreeMap<&str, i32> {
    let mut m = BTreeMap::new();
    for s in v { *m.entry(s.as_str()).or_insert(0) += 1; }
    m
}

#[test]
fn every_refuse_site_is_on_the_committed_list_and_every_listed_site_exists() {
    let list = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/refuse-sites.txt")).unwrap();
    let mut listed: Vec<String> = list.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(str::to_owned).collect();
    listed.sort();
    let found = sites_in_source();
    let (l, f) = (counts(&listed), counts(&found));
    let only_listed: Vec<_> = l.iter().filter(|(k, n)| f.get(*k) < Some(n)).map(|(k, _)| *k).collect();
    let only_found: Vec<_> = f.iter().filter(|(k, n)| l.get(*k) < Some(n)).map(|(k, _)| *k).collect();
    assert!(only_listed.is_empty() && only_found.is_empty(),
        "the refuse! sites differ from crates/retrace-box/tests/refuse-sites.txt. A site is added, moved or \
         removed only with the list, in the same commit.\nlisted but not in the source:\n{}\n\nin the source \
         but not listed:\n{}\n\nthe source's sites, for review:\n{}",
        only_listed.join("\n"), only_found.join("\n"), found.join("\n"));
}

#[test]
fn the_wall_marker_is_written_only_by_wall_rs() {
    let root = repo();
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    let writers: Vec<String> = files.iter()
        .filter(|f| !f.ends_with("crates/retrace-arch/src/wall.rs"))
        .filter(|f| std::fs::read_to_string(f).unwrap().contains(retrace_arch::WALL_MARKER))
        .map(|f| f.strip_prefix(&root).unwrap().display().to_string())
        .collect();
    assert!(writers.is_empty(), "R6: only wall() and refuse! write the wall marker, but these files spell it: {writers:?}");
    assert!(files.len() > 100, "the walk found the source ({} files)", files.len());
}
