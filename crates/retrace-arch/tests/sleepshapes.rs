//! M49 (spec §3f S1): the `__semwait_signal` row and the never-forwarded pair. VM-free. Every number
//! is read from the SDK's `sys/syscall.h` at test time (M44 R5's method), so a value typed from
//! memory cannot satisfy these.
use retrace_arch::{arg_kinds, is_semwait, ArgKind, Ret, SYS___SEMWAIT_SIGNAL, SYS___SEMWAIT_SIGNAL_NOCANCEL};

fn sdk_header(rel: &str) -> String {
    let out = std::process::Command::new("xcrun").arg("--show-sdk-path").output().expect("run xcrun");
    assert!(out.status.success(), "xcrun --show-sdk-path failed");
    let path = format!("{}/usr/include/{rel}", String::from_utf8(out.stdout).unwrap().trim());
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The value of `#define <name> <value>` in `text` (`nodeshapes.rs`'s helper, decimal only here).
fn define(text: &str, name: &str) -> Option<u64> {
    text.lines().find_map(|l| {
        let mut w = l.split_whitespace();
        if w.next() != Some("#define") || w.next() != Some(name) { return None; }
        w.next()?.parse().ok()
    })
}

/// M49 §3f S1: 334 and its `_nocancel` twin are the SDK's numbers, and `is_semwait` is exactly
/// those two over the whole BSD range. The spec's "420" is `sem_wait_nocancel`; the twin is 423.
#[test]
fn the_semwait_pair_is_the_sdks_334_and_423_and_nothing_else() {
    let h = sdk_header("sys/syscall.h");
    assert_eq!(define(&h, "SYS___semwait_signal"), Some(SYS___SEMWAIT_SIGNAL));
    assert_eq!(define(&h, "SYS___semwait_signal_nocancel"), Some(SYS___SEMWAIT_SIGNAL_NOCANCEL));
    let ours: Vec<u64> = (0..1024).filter(|&n| is_semwait(n)).collect();
    assert_eq!(ours, vec![334, 423]);
}

/// M49 §3f S1: `__semwait_signal(cond_sem, mutex_sem, timeout, relative, tv_sec, tv_nsec)` is six
/// values: no descriptor, no pointer. Its `_nocancel` twin carries the same row.
#[test]
fn the_semwait_row_is_six_scalars_shared_by_its_nocancel_twin() {
    use ArgKind::*;
    for n in [SYS___SEMWAIT_SIGNAL, SYS___SEMWAIT_SIGNAL_NOCANCEL] {
        let s = arg_kinds(n).unwrap_or_else(|| panic!("syscall {n} has no arg_kinds row"));
        assert_eq!((s.args, s.ret), (&[Scalar, Scalar, Scalar, Scalar, Scalar, Scalar][..], Ret::Plain), "syscall {n}");
    }
}
