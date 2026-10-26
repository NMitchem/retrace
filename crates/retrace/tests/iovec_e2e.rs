//! M53 t2 (spec §3c, §3m): `readv`, `writev`, `preadv` and `pwritev` translate their iovecs. Each
//! mode of `iovec_dyn` runs natively, is recorded, and is replayed twice: all three must print the
//! same lines, and the lines that matter are pinned by value (t0 M3's measured answers), so a call
//! that forwarded guest addresses (an `EFAULT` where native transfers) fails here and not by passing
//! quietly. Before M53 a forwarded `writev` lost its bytes to `EFAULT` and `readv` was refused.
mod util;

use retrace_trace::{Event, Reader};

const WALLS: &str = "https://github.com/NMitchem/retrace/blob/main/docs/walls.md";
const BOUND: u64 = 300;

struct Ran { trace: std::path::PathBuf }

/// Run `iovec_dyn <mode>` natively and under retrace, replay twice, and return the native output and
/// the recording's trace. Every run exits 0 and prints exactly what native printed; each `pinned` line
/// is in it, and the directory is removed.
fn as_natively(mode: &str, pinned: &[&str]) -> Ran {
    let native = util::run_bounded(retrace_guest::IOVEC_DYN, &[mode], &[], BOUND);
    assert_eq!(native.code, Some(0), "{mode}: native exits 0. stderr:\n{}", native.stderr);
    let (rec, trace) = util::record_dynamic_args(retrace_guest::IOVEC_DYN, &[mode]);
    assert_eq!(rec.code, 0, "{mode}: the recording exits 0. stderr:\n{}", rec.stderr);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), String::from_utf8_lossy(&native.stdout),
        "{mode}: the recording prints what native printed");
    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 0, "{mode}: replay {i} exits 0. stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "{mode}: replay {i} prints what the recording printed");
    }
    let out = String::from_utf8_lossy(&native.stdout).into_owned();
    for line in pinned { assert!(out.lines().any(|l| l == *line), "{mode}: no line {line:?} in:\n{out}"); }
    assert_eq!(out.lines().last(), Some("removed=yes"), "{mode}: the directory is removed:\n{out}");
    Ran { trace }
}

#[test]
fn a_writev_to_a_file_lands_every_entry_and_readv_reads_them_back() {
    let r = as_natively("file", &["w=12", "r=12", "abc|defg|hijkl", "at eof r=0"]);
    let _ = std::fs::remove_file(&r.trace);
}

#[test]
fn pwritev_and_preadv_use_their_offset() {
    let r = as_natively("pos", &[
        "w=5", "r=5",
        // The bytes landed at offset 7 and the descriptor's own offset did not move (12 after the write).
        "got XYZWV offset=12",
        // The host kernel's own answers, through the forward.
        "preadv(off -1) rc=-1 errno=22", "preadv(pipe) rc=-1 errno=29",
        "preadv(pipe, zero-length) rc=-1 errno=29",
    ]);
    let _ = std::fs::remove_file(&r.trace);
}

#[test]
fn a_console_writev_prints_on_record_and_on_both_replays() {
    // Natively the bytes reach stdout and stderr. Before M53 they were lost: a forwarded writev
    // answered EFAULT and printed nothing.
    // Native runs with a pipe for stdout, as the recorder's is (run_bounded's own is a capture file, where
    // pwritev succeeds): /bin/sh honours pipefail, so the guest's exit code survives the `cat`.
    let native = util::run_bounded("/bin/sh",
        &["-c", r#"set -o pipefail; "$0" console | cat"#, retrace_guest::IOVEC_DYN], &[], BOUND);
    assert_eq!(native.code, Some(0), "native exits 0. stderr:\n{}", native.stderr);
    assert_eq!(String::from_utf8_lossy(&native.stdout), "ab\n");
    assert_eq!(native.stderr, "e\npwritev rc=-1 errno=29\n", "native: pwritev on a pipe is ESPIPE");
    let (rec, trace) = util::record_dynamic_args(retrace_guest::IOVEC_DYN, &["console"]);
    assert_eq!(rec.code, 0, "the recording exits 0. stderr:\n{}", rec.stderr);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), "ab\n", "record prints the stdout entries");
    // pwritev is not a console write: its line is the host's answer, native's (ESPIPE on a pipe).
    assert_eq!(rec.stderr, native.stderr, "record prints the stderr entry and the pwritev answer native gives, and no line of retrace's own");
    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 0, "replay {i} exits 0. stderr:\n{}", rep.stderr);
        assert_eq!(String::from_utf8_lossy(&rep.stdout), "ab\n", "replay {i} prints the stdout entries");
        assert_eq!(rep.stderr, rec.stderr, "replay {i} prints the stderr entry and the pwritev answer the recording holds");
    }
    // The landmark is a write-less one: the bytes are printed, never recorded as memory.
    let events = Reader::open(&trace).unwrap();
    let wv: Vec<_> = events.iter().filter_map(|e| match e {
        Event::Syscall { num: 121 | 412, args, ret, err, writes, .. } if args[0] <= 2 => Some((*ret, *err, writes.len())),
        _ => None,
    }).collect();
    assert_eq!(wv, vec![(3, false, 0), (2, false, 0)], "the two console writevs, with no writes");
    let _ = std::fs::remove_file(&trace);
}

#[test]
fn a_short_readv_from_a_pipe_fills_only_what_arrived() {
    let r = as_natively("shortpipe", &[
        // 5 bytes into entries of 3, 4 and 8: entry 2 is untouched.
        "r=5", "123|45|empty", "drained=yes",
        // The pipe's count follows writev and readv, so the following readv sees exactly what was written.
        "w=5", "r=5 ab|cde",
    ]);
    let _ = std::fs::remove_file(&r.trace);
}

#[test]
fn a_readv_entry_of_200_kib_is_filled_and_replays() {
    const N: u64 = 200 * 1024;
    let r = as_natively("big", &["w=204800 r=204800 same=yes", "halves r=204800 same=yes"]);
    let events = Reader::open(&r.trace).unwrap();
    let readers: Vec<&Vec<retrace_trace::Region>> = events.iter().filter_map(|e| match e {
        Event::Syscall { num: 120 | 411, writes, err: false, .. } => Some(writes),
        _ => None,
    }).collect();
    assert!(readers.len() >= 2, "the two readvs are in the trace: {}", readers.len());
    for writes in &readers {
        let lo = writes.iter().map(|w| w.ipa).min().expect("a reader that filled its buffer wrote");
        let hi = writes.iter().map(|w| w.ipa + w.bytes.len() as u64).max().unwrap();
        // None outside the entry's buffer: the kernel's stores are bounded by the host iovec.
        assert!(hi - lo <= N, "writes span {:#x}..{:#x}, more than the {N}-byte buffer", lo, hi);
        let total: u64 = writes.iter().map(|w| w.bytes.len() as u64).sum();
        // The whole entry, less an unchanged span at each end (the pattern byte is 0 once in 256 and
        // the buffer was zeroed): never capped at the 64 KiB diff window.
        assert!(total + 16 >= N, "the readv's writes total {total} bytes of {N}");
    }
    let _ = std::fs::remove_file(&r.trace);
}

#[test]
fn iovcnt_limits_answer_natives_errnos() {
    let r = as_natively("limits", &[
        "readv(iovcnt 0) rc=-1 errno=22", "writev(iovcnt 0) rc=-1 errno=22",
        "writev(iovcnt 1024) rc=1024 errno=0", "writev(iovcnt 1025) rc=-1 errno=22", "writev(iovcnt -1) rc=-1 errno=22",
        "readv(iovcnt 1024) rc=1024 errno=0", "readv(iovcnt 1025) rc=-1 errno=22", "readv(iovcnt -1) rc=-1 errno=22",
        "pwritev(iovcnt 1025) rc=-1 errno=22", "writev(iovcnt 0, unmapped array) rc=-1 errno=22",
    ]);
    let _ = std::fs::remove_file(&r.trace);
}

#[test]
fn an_unreachable_entry_answers_natives_errno() {
    let r = as_natively("badentry", &[
        // A later bad entry: the prefix transferred and the offset moved. The first non-empty entry bad:
        // nothing did.
        "writev(good, bad) rc=-1 errno=14 size=10 offset=10", "writev(bad, good) rc=-1 errno=14 size=0",
        "readv(good10, bad10) rc=-1 errno=14 first=ABCDEFGHIJ offset=10",
        "readv(bad10, good10) rc=-1 errno=14 untouched=yes offset=0",
        // A zero-length entry is dropped before its base is examined.
        "readv(4 good, zero-length bad base, zero-length) rc=4 errno=0 got=WXYZ",
        "writev(zero-length bad base, 4 good) rc=4 errno=0", "writev(only a zero-length bad base) rc=0 errno=0",
        "writev(iov NULL, 2) rc=-1 errno=14", "writev(len -1) rc=-1 errno=22", "writev(fd -1) rc=-1 errno=9",
    ]);
    let _ = std::fs::remove_file(&r.trace);
}

#[test]
fn a_pipe_readv_with_a_later_unreachable_entry_ends_the_recording_at_one_clean_line() {
    // Natively a pipe readv fills entry 0 and drains nothing, which the pipe model cannot reproduce: a wall.
    let t = std::env::temp_dir().join(format!("retrace-iovec-{}-badpipe.bin", std::process::id()));
    let tp = t.to_str().unwrap();
    let rec = util::run_bounded(util::bin(),
        &["record-dyn", retrace_guest::IOVEC_DYN, "-o", tp, "--", "badpipe"], &[], BOUND);
    assert!(!rec.stderr.contains("panicked at"), "{}", rec.stderr);
    let lines: Vec<&str> = rec.stderr.lines().filter(|l| l.starts_with("retrace:")).collect();
    assert_eq!(lines.len(), 1, "exactly one retrace: line. stderr:\n{}", rec.stderr);
    assert!(lines[0].starts_with("retrace: unsupported: readv (syscall ") && lines[0].contains("on guest pipe fd ")
        && lines[0].ends_with(&format!(" - see {WALLS}#syscalls")), "{}", lines[0]);
    assert_eq!(rec.code, Some(4), "a wall exits 4. stderr:\n{}", rec.stderr);
    let rep = util::run_bounded(util::bin(), &["replay", tp], &[], BOUND);
    assert!(rep.stderr.lines().any(|l| l.starts_with("retrace: incomplete recording: it ends at ")), "{}", rep.stderr);
    assert!(!rep.stderr.contains("DIVERGENCE"), "the end of a walled recording is not a divergence");
    let _ = std::fs::remove_file(&t);
}

#[test]
fn an_older_trace_is_refused_by_name() {
    // M53 moved the format to RT\x00\x0f: a recording made before it (RT\x00\x0e) holds an EFAULT where
    // a writev was forwarded untranslated.
    let t = std::env::temp_dir().join(format!("retrace-iovec-{}-old.bin", std::process::id()));
    std::fs::write(&t, b"RT\x00\x0erest-of-a-pre-M53-trace").unwrap();
    let rep = util::run_bounded(util::bin(), &["replay", t.to_str().unwrap()], &[], BOUND);
    assert!(rep.stderr.contains("recorded by a different retrace trace format (not torn)"), "{}", rep.stderr);
    assert!(rep.stderr.contains(r"RT\x00\x0e") && rep.stderr.contains(r"RT\x00\x0f"),
        "the refusal names both magics: {}", rep.stderr);
    assert_ne!(rep.code, Some(0), "a refused trace does not replay");
    let _ = std::fs::remove_file(&t);
}
