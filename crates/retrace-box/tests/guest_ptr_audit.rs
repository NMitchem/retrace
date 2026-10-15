//! M51 §3h: the guest-pointer audit, beside M50's `refuse_audit`. Every call of an unchecked
//! guest-memory accessor (`read_guest`, `write_guest`, `read_u64`, `apply_and_return`) in retrace-box's
//! and retrace-core's non-test source is listed in `guest-ptr-sites.txt`, with why it cannot see a
//! pointer the program passed unchecked:
//! - `internal: …`: retrace's own memory, or a recorded region replay applies;
//! - `covered: <check> …`: a check in the same function answers native's error first.
//!
//! Since M51 a failed `read_guest` or `write_guest` is retrace's bug, so a new call goes on the list,
//! with its reason, in the same commit.
use std::collections::BTreeMap;
use std::path::Path;

mod audit;
use audit::{repo, rust_files, source_lines};

const ACCESSORS: [&str; 4] = ["read_guest", "write_guest", "read_u64", "apply_and_return"];

/// Every accessor call in the non-test source, as `<file>\t<fn>\t<accessor>`, and each function's
/// source text by `<file>\t<fn>`. A call is the accessor's name followed by `(`, not preceded by an
/// identifier character, on a line that does not define it; anything after `//` is ignored.
fn sites_in_source() -> (Vec<String>, BTreeMap<String, String>) {
    let root = repo();
    let mut files = Vec::new();
    for dir in ["crates/retrace-box/src", "crates/retrace-core/src"] { rust_files(&root.join(dir), &mut files); }
    let (mut sites, mut bodies) = (Vec::new(), BTreeMap::<String, String>::new());
    for l in source_lines(&root, &files) {
        let code = l.text.split("//").next().unwrap_or("");
        let body = bodies.entry(format!("{}\t{}", l.rel, l.func)).or_default();
        body.push_str(code);
        body.push('\n');
        for acc in ACCESSORS {
            if code.contains(&format!("fn {acc}(")) { continue; }
            let call = format!("{acc}(");
            let mut from = 0;
            while let Some(at) = code[from..].find(&call) {
                let at = from + at;
                from = at + call.len();
                if code[..at].chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_') { continue; }
                sites.push(format!("{}\t{}\t{acc}", l.rel, l.func));
            }
        }
    }
    sites.sort();
    (sites, bodies)
}

/// The committed list's rows, split into their four fields.
fn listed() -> Vec<Vec<String>> {
    let list = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guest-ptr-sites.txt")).unwrap();
    list.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').map(str::to_owned).collect()).collect()
}

fn counts(v: &[String]) -> BTreeMap<&str, i32> {
    let mut m = BTreeMap::new();
    for s in v { *m.entry(s.as_str()).or_insert(0) += 1; }
    m
}

#[test]
fn every_unchecked_guest_access_is_on_the_committed_list_and_every_listed_access_exists() {
    let mut keys: Vec<String> = listed().iter().map(|r| r[..3.min(r.len())].join("\t")).collect();
    keys.sort();
    let (found, _) = sites_in_source();
    let (l, f) = (counts(&keys), counts(&found));
    let only_listed: Vec<_> = l.iter().filter(|(k, n)| f.get(*k) < Some(n)).map(|(k, _)| *k).collect();
    let only_found: Vec<_> = f.iter().filter(|(k, n)| l.get(*k) < Some(n)).map(|(k, _)| *k).collect();
    assert!(only_listed.is_empty() && only_found.is_empty(),
        "the guest-memory accessor calls differ from crates/retrace-box/tests/guest-ptr-sites.txt. A call is \
         added, moved or removed only with the list, in the same commit, with its reason.\nlisted but not in \
         the source:\n{}\n\nin the source but not listed:\n{}\n\nthe source's calls, for review:\n{}",
        only_listed.join("\n"), only_found.join("\n"), found.join("\n"));
}

#[test]
fn every_listed_access_says_why_and_a_covered_one_names_a_check_its_function_makes() {
    let (_, bodies) = sites_in_source();
    for row in listed() {
        assert_eq!(row.len(), 4, "<file>\\t<fn>\\t<accessor>\\t<why>: {row:?}");
        let why = &row[3];
        if why.starts_with("internal: ") { continue; }
        let check = why.strip_prefix("covered: ").and_then(|w| w.split_whitespace().next())
            .unwrap_or_else(|| panic!("a reason starts `internal: ` or `covered: <check>`: {row:?}"));
        let body = bodies.get(&format!("{}\t{}", row[0], row[1])).map(String::as_str).unwrap_or("");
        assert!(body.contains(check), "{}::{} is listed as covered by `{check}`, which it never calls", row[0], row[1]);
    }
}
