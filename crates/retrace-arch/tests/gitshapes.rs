//! M47 (spec §3b–§3e): the pure halves of git's mechanisms — the path rows, `madvise_effect`,
//! `mac_syscall_model` with `sandbox_check_continuity`, and `fork_refusal_errno`. VM-free. Every
//! number is read from the SDK's headers at test time (M44 R5's method), so a value typed from
//! memory cannot satisfy these.
use retrace_arch::{arg_kinds, ArgKind, Ret};

fn sdk_header(rel: &str) -> String {
    let out = std::process::Command::new("xcrun").arg("--show-sdk-path").output().expect("run xcrun");
    assert!(out.status.success(), "xcrun --show-sdk-path failed");
    let path = format!("{}/usr/include/{rel}", String::from_utf8(out.stdout).unwrap().trim());
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The value of `#define <name> <value>` in `text`: parentheses stripped, decimal or `0x` hex, an
/// optional leading minus. `None` if the header does not define `name` numerically.
fn define(text: &str, name: &str) -> Option<i64> {
    text.lines().find_map(|l| {
        let mut w = l.split_whitespace();
        if w.next() != Some("#define") || w.next() != Some(name) { return None; }
        let v = w.next()?.trim_start_matches('(').trim_end_matches(')');
        let (neg, v) = match v.strip_prefix('-') { Some(r) => (true, r), None => (false, v) };
        let n = match v.strip_prefix("0x") { Some(h) => i64::from_str_radix(h, 16).ok()?, None => v.parse().ok()? };
        Some(if neg { -n } else { n })
    })
}

/// M47 §3b: the numbers the new rows are keyed by are the SDK's.
#[test]
fn the_path_rows_are_the_sdks_numbers() {
    let h = sdk_header("sys/syscall.h");
    for (name, n) in [("SYS_link", 9), ("SYS_chdir", 12), ("SYS_mkdir", 136), ("SYS_utimes", 138),
                      ("SYS___pthread_canceled", 333)] {
        assert_eq!(define(&h, name), Some(n), "{name}");
    }
}

/// M47 §3b: each row is its C prototype, argument by argument. All five are forwarded (R1 for
/// chdir; 333 on the 331 precedent), so none is a refusal and each returns a plain value.
#[test]
fn the_path_rows_are_written_from_their_prototypes() {
    use ArgKind::*;
    let want: [(u64, &[ArgKind]); 5] =
        [(12, &[Path]), (136, &[Path, Scalar]), (9, &[Path, Path]), (138, &[Path, Ptr]), (333, &[Scalar])];
    for (n, kinds) in want {
        let s = arg_kinds(n).unwrap_or_else(|| panic!("syscall {n} has no arg_kinds row"));
        assert_eq!((s.args, s.ret), (kinds, Ret::Plain), "syscall {n}");
    }
}
