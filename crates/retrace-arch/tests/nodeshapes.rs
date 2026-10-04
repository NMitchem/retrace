//! M48 (spec §3b, §3e): the rows node's kqueues, condition variables and libuv's stdout
//! `setsockopt` need, and the psynch set that is never forwarded. VM-free. Every number is read
//! from the SDK's headers at test time (M44 R5's method), so a value typed from memory cannot
//! satisfy these.
use retrace_arch::{arg_kinds, decode_sprr_access, is_psynch, ArgKind, Ret, SprrAccess, SYS_KEVENT, SYS_PSYNCH_CVBROAD,
                   SYS_PSYNCH_CVSIGNAL, SYS_PSYNCH_CVWAIT, SYS_SETSOCKOPT};

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

/// M48 §3b/§3e: the numbers the new rows are keyed by are the SDK's.
#[test]
fn the_kevent_setsockopt_and_psynch_numbers_are_the_sdks() {
    let h = sdk_header("sys/syscall.h");
    for (name, n) in [("SYS_kevent", SYS_KEVENT), ("SYS_setsockopt", SYS_SETSOCKOPT),
                      ("SYS_psynch_cvbroad", SYS_PSYNCH_CVBROAD), ("SYS_psynch_cvsignal", SYS_PSYNCH_CVSIGNAL),
                      ("SYS_psynch_cvwait", SYS_PSYNCH_CVWAIT)] {
        assert_eq!(define(&h, name), Some(n as i64), "{name}");
    }
}

/// M48 §3e: `is_psynch` is EXACTLY the SDK's `SYS_psynch_*` set — 14 numbers at plan time, 297–309
/// and 312 — over the whole BSD range. The set is collected from the header, so an SDK that grows
/// a fifteenth fails here by name rather than being forwarded to the host's psynch state.
#[test]
fn is_psynch_is_exactly_the_sdks_psynch_set() {
    let h = sdk_header("sys/syscall.h");
    let mut sdk: Vec<u64> = h.lines().filter_map(|l| {
        let mut w = l.split_whitespace();
        if w.next() != Some("#define") || !w.next()?.starts_with("SYS_psynch_") { return None; }
        w.next()?.parse().ok()
    }).collect();
    sdk.sort_unstable();
    let ours: Vec<u64> = (0..1024).filter(|&n| is_psynch(n)).collect();
    assert_eq!(ours, sdk, "is_psynch must be exactly the SDK's SYS_psynch_* set");
}

/// M48 §3b: `kevent(kq, changelist, nchanges, eventlist, nevents, timeout)`. Operand 0 is a guest
/// kqueue DESCRIPTOR (unlike `kevent_qos`'s workqueue sentinel), the two lists are `Ptr`s the model
/// walks through the stage-1 tables, and the counts and `timeout` carry no kind of their own.
#[test]
fn the_kevent_row_is_its_prototype() {
    use ArgKind::*;
    let s = arg_kinds(SYS_KEVENT).expect("kevent (363) has no arg_kinds row");
    assert_eq!((s.args, s.ret), (&[Fd, Ptr, Scalar, Ptr, Scalar, Ptr][..], Ret::Plain));
}

/// M48 §3e, §3b: each psynch condition-variable row is its `kern_synch.c` prototype, argument by
/// argument, and `setsockopt`'s value is a `Source` read for `len` bytes (the `write` convention).
#[test]
fn the_psynch_and_setsockopt_rows_are_their_prototypes() {
    use ArgKind::*;
    let want: [(u64, &[ArgKind]); 4] = [
        (SYS_PSYNCH_CVBROAD, &[Ptr, Scalar, Scalar, Scalar, Ptr, Scalar, Scalar]),
        (SYS_PSYNCH_CVSIGNAL, &[Ptr, Scalar, Scalar, Scalar, Ptr, Scalar, Scalar, Scalar]),
        (SYS_PSYNCH_CVWAIT, &[Ptr, Scalar, Scalar, Ptr, Scalar, Scalar, Scalar, Scalar]),
        (SYS_SETSOCKOPT, &[Fd, Scalar, Scalar, Source, Scalar]),
    ];
    for (n, kinds) in want {
        let s = arg_kinds(n).unwrap_or_else(|| panic!("syscall {n} has no arg_kinds row"));
        assert_eq!((s.args, s.ret), (kinds, Ret::Plain), "syscall {n}");
    }
}

/// M48 §3f, §11b item 2: `S3_6_C15_C1_5` is decoded in both directions, with its register. The
/// words are clang's: `crates/retrace-guest/asm/sprrprobe.s` assembles the first and the third, and
/// `retrace-guest`'s `sprrprobe_guest_parses_and_carries_both_sprr_encodings` re-reads them from the
/// built binary, so a word typed wrongly here fails there.
#[test]
fn the_sprr_decode_reads_s3_6_c15_c1_5_in_both_directions() {
    use SprrAccess::*;
    assert_eq!(decode_sprr_access(0xd53e_f1a0), Some(Read { rt: 0 }));   // mrs x0, S3_6_C15_C1_5 (sprrprobe)
    assert_eq!(decode_sprr_access(0xd53e_f1a9), Some(Read { rt: 9 }));   // mrs x9, … (pthread's read-back)
    assert_eq!(decode_sprr_access(0xd51e_f1a1), Some(Write { rt: 1 }));  // msr S3_6_C15_C1_5, x1 (sprrprobe)
    assert_eq!(decode_sprr_access(0xd51e_f1a0), Some(Write { rt: 0 }));  // msr …, x0 (pthread_jit_write_protect_np)
    assert_eq!(decode_sprr_access(0xd51e_f1bf), Some(Write { rt: 31 })); // msr …, xzr: Rt 31 is XZR here
}

/// M48 §3f: the decode swallows no neighbour. Each of these stays undefined (or keeps its own
/// path) and surfaces as `Stop::Other`, never as an SPRR access.
#[test]
fn the_sprr_decode_refuses_every_neighbour() {
    for (w, what) in [
        (0xd53e_f180, "mrs x0, S3_6_C15_C1_4 (op2 4)"),
        (0xd53e_f1c0, "mrs x0, S3_6_C15_C1_6 (op2 6)"),
        (0xd53e_f2a0, "mrs x0, S3_6_C15_C2_5 (CRm 2)"),
        (0xd53d_f1a0, "mrs x0, S3_5_C15_C1_5 (op1 5)"),
        (0xd51c_f2e0, "msr S3_4_C15_C2_7, x0 (APRR: pthread's commpage +0x10c == 1 path)"),
        (0xd50b_7523, "ic ivau, x3 (a SYS, sys_icache_invalidate's)"),
        (0xd503_42df, "msr daifset, #2 (MSR immediate)"),
        (0xd503_3fdf, "isb"),
    ] {
        assert_eq!(decode_sprr_access(w), None, "{what} ({w:#010x})");
    }
}
