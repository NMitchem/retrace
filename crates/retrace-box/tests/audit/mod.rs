//! The source walk `refuse_audit.rs` and `guest_ptr_audit.rs` share (M51 §3h, Ruling P-R1). Not a test
//! target: each audit `mod`s it, and a helper only one of them uses is `allow(dead_code)` in the other.
#![allow(dead_code)]
use std::path::{Path, PathBuf};

/// The repository root, two levels above this crate.
pub fn repo() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap() }

/// Every `.rs` file under `dir`, recursively, in a stable order.
pub fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = rd.map(|e| e.unwrap().path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() { rust_files(&p, out) } else if p.extension().is_some_and(|e| e == "rs") { out.push(p) }
    }
}

/// The name a `fn` item line declares, or None for any other line.
pub fn fn_name(t: &str) -> Option<String> {
    let i = t.find("fn ")?;
    let quals = ["pub", "pub(crate)", "pub(super)", "const", "unsafe", "extern", "\"C\""];
    if !t[..i].split_whitespace().all(|w| quals.contains(&w)) { return None; }
    let name: String = t[i + 3..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    (!name.is_empty()).then_some(name)
}

/// One line of non-test source: its file (relative to the repository root), the function it sits in
/// (`<top level>` before the first), its 1-based number and its text, trimmed at the left.
pub struct Line {
    pub rel: String,
    pub func: String,
    pub no: usize,
    pub text: String,
}

/// Every line of `files` that a guest can reach: all but whole-line `//` comments, and nothing from a
/// `#[cfg(test)] mod` to the end of the file.
pub fn source_lines(root: &Path, files: &[PathBuf]) -> Vec<Line> {
    let mut out = Vec::new();
    for f in files {
        let rel = f.strip_prefix(root).unwrap().display().to_string();
        let text = std::fs::read_to_string(f).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let mut func = String::from("<top level>");
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim_start();
            if t == "#[cfg(test)]" && lines.get(i + 1).is_some_and(|n| n.trim_start().starts_with("mod ")) { break; }
            if t.starts_with("//") { continue; }
            if let Some(name) = fn_name(t) { func = name; }
            out.push(Line { rel: rel.clone(), func: func.clone(), no: i + 1, text: t.to_owned() });
        }
    }
    out
}
