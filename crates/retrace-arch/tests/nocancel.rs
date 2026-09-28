//! M44 A1 (spec §3b): every `_nocancel` spelling shares its plain form's `arg_kinds` row — the rule
//! `ArgKind::Source`'s comment states and five milestones broke by hand (M9, M10, M27, `sendto`,
//! `openat`). Names and numbers both come from the SDK's own `sys/syscall.h`, read at test time
//! (`hv-sys`'s build already needs the SDK), so a row keyed at the wrong number — M44 §2a's class,
//! 468 carried as `getattrlistat` for seven milestones — cannot satisfy it.
use retrace_arch::arg_kinds;
use std::collections::BTreeMap;

/// `SYS_<name> <number>` for every syscall the SDK header defines.
fn sdk_syscalls() -> BTreeMap<String, u64> {
    let out = std::process::Command::new("xcrun").arg("--show-sdk-path").output().expect("run xcrun");
    assert!(out.status.success(), "xcrun --show-sdk-path failed");
    let path = format!("{}/usr/include/sys/syscall.h", String::from_utf8(out.stdout).unwrap().trim());
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut m = BTreeMap::new();
    for line in text.lines() {
        let mut w = line.split_whitespace();
        if w.next() != Some("#define") { continue; }
        let (Some(name), Some(num)) = (w.next(), w.next()) else { continue };
        let Some(name) = name.strip_prefix("SYS_") else { continue };
        if let Ok(n) = num.parse::<u64>() { m.insert(name.to_string(), n); }
    }
    assert!(m.len() > 400, "parsed only {} syscall numbers from {path}", m.len());
    m
}

/// `_nocancel` names with no plain twin in the SDK, sorted by name, each with t0 M4's reason.
/// Asserted EXACTLY, so the list cannot rot in either direction.
///
/// t0 M4 measured every one of the SDK's 32 `_nocancel` names against the same header's plain
/// names: all 32 have a plain twin, so this stays empty.
const ORPHANS: &[(&str, &str)] = &[];

#[test]
fn every_nocancel_spelling_shares_its_plain_forms_row() {
    let sdk = sdk_syscalls();
    let (mut orphans, mut mismatched, mut pairs) = (Vec::new(), Vec::new(), 0);
    for (name, &nc) in &sdk {
        let Some(plain) = name.strip_suffix("_nocancel") else { continue };
        let Some(&p) = sdk.get(plain) else { orphans.push(name.as_str()); continue };
        pairs += 1;
        let (a, b) = (arg_kinds(p), arg_kinds(nc));
        if (a.is_some() || b.is_some()) && a != b {
            mismatched.push(format!("{plain} ({p}): {a:?}\n  {name} ({nc}): {b:?}"));
        }
    }
    let expected: Vec<&str> = ORPHANS.iter().map(|(n, _)| *n).collect();
    assert_eq!(orphans, expected, "_nocancel names with no plain twin must be exactly ORPHANS");
    assert!(pairs >= 25, "only {pairs} twin pairs parsed — the parse is wrong, not the table");
    assert!(mismatched.is_empty(), "twins whose rows differ (the _nocancel trap):\n{}", mismatched.join("\n"));
}
