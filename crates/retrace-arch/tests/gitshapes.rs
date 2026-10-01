//! M47 (spec §3b–§3e): the pure halves of git's mechanisms — the path rows, `madvise_effect`,
//! `mac_syscall_model` with `sandbox_check_continuity`, and `fork_refusal_errno`. VM-free. Every
//! number is read from the SDK's headers at test time (M44 R5's method), so a value typed from
//! memory cannot satisfy these.
use retrace_arch::{arg_kinds, madvise_effect, ArgKind, MadviseEffect, Ret, MADV_FREE_REUSABLE, MADV_FREE_REUSE, MADV_ZERO, SYS_MADVISE};

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

/// M47 §3c: the advice values are the SDK's (`sys/mman.h`), and so is `madvise`'s number.
#[test]
fn the_madvise_values_are_the_sdks() {
    assert_eq!(define(&sdk_header("sys/syscall.h"), "SYS_madvise"), Some(SYS_MADVISE as i64));
    let m = sdk_header("sys/mman.h");
    for (name, v) in [("MADV_FREE_REUSABLE", MADV_FREE_REUSABLE), ("MADV_FREE_REUSE", MADV_FREE_REUSE),
                      ("MADV_ZERO", MADV_ZERO)] {
        assert_eq!(define(&m, name), Some(i64::from(v)), "{name}");
    }
}

/// M47 §3c: t0 M1(a)'s census, and only it, is modelled.
#[test]
fn the_measured_advice_values_are_modelled() {
    assert_eq!(madvise_effect(MADV_FREE_REUSABLE), Ok(MadviseEffect::NoOp));
    assert_eq!(madvise_effect(MADV_FREE_REUSE), Ok(MadviseEffect::NoOp));
    assert_eq!(madvise_effect(MADV_ZERO), Ok(MadviseEffect::Zero));
}

/// M47 §3c: every other value is refused by value, naming it — the kernel's whole `int` range is
/// swept at its edges and densely where `sys/mman.h` defines values.
#[test]
fn every_other_advice_value_is_refused_naming_it() {
    let accepted = [MADV_FREE_REUSABLE, MADV_FREE_REUSE, MADV_ZERO];
    for v in (0..=64u32).chain([0x7fff_ffff, 0x8000_0000, 0xffff_fff9, u32::MAX]) {
        if accepted.contains(&v) { continue; }
        let e = madvise_effect(v).unwrap_err();
        assert!(e.starts_with(&format!("M47: unmeasured madvise advice {v}.")), "{v}: {e}");
    }
}
