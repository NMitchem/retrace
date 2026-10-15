//! M51 §3h: a pointer the program could not pass natively gets native's error under retrace, whether
//! retrace forwards the call or services it, and nothing is written through it. Every expected line is
//! native's (t0 M5); each test also compares the recording's output with a native run's.
mod util;
use retrace_trace::{Event, Reader};
use std::path::{Path, PathBuf};

const BOUND: u64 = 120;
const P: &str = retrace_guest::FID_BADPTR;

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("retrace-badptr-{}-{name}", std::process::id()))
}

/// Native, record and two replays of one mode: the same stdout, exit 0, no wall and no bug line.
/// Returns stdout and the recording's events.
fn agree(mode: &str, t: &Path) -> (String, Vec<Event>) {
    let native = util::run_bounded(P, &[mode], &[], BOUND);
    assert_eq!(native.code, Some(0), "{mode}: native. stderr:\n{}", native.stderr);
    let rec = util::run_bounded(util::bin(), &["record-dyn", P, "-o", t.to_str().unwrap(), "--", mode], &[], BOUND);
    assert!(!rec.stderr.contains("retrace: "), "{mode}: no line of retrace's own. stderr:\n{}", rec.stderr);
    assert_eq!((rec.code, rec.stdout.as_slice()), (Some(0), native.stdout.as_slice()), "{mode}: record against native");
    for i in 1..=2 {
        let rp = util::run_bounded(util::bin(), &["replay", t.to_str().unwrap()], &[], BOUND);
        assert_eq!((rp.code, rp.stdout.as_slice()), (Some(0), rec.stdout.as_slice()), "{mode}: replay {i}. stderr:\n{}", rp.stderr);
    }
    (String::from_utf8_lossy(&native.stdout).into_owned(), Reader::open(t).unwrap())
}

/// The recording's last landmark of syscall `num` that failed.
fn failed(events: &[Event], num: u64) -> (u64, usize) {
    events.iter().rev().find_map(|e| match e {
        Event::Syscall { num: n, ret, err: true, writes, .. } if *n == num => Some((*ret, writes.len())),
        _ => None,
    }).unwrap_or_else(|| panic!("no failed syscall {num} in the recording"))
}

/// Spec §4: a read into a page the program unmapped. Natively `EFAULT`; under retrace the call is
/// refused before the forward, recorded with no writes.
#[test]
fn a_read_into_unmapped_memory_returns_efault() {
    let t = scratch("unmapped.bin");
    let (out, events) = agree("readunmapped", &t);
    assert_eq!(out, "read(hole): rc=-1 errno=14\n");
    assert_eq!(failed(&events, 3), (retrace_arch::EFAULT, 0), "EFAULT, nothing written");
    let _ = std::fs::remove_file(&t);
}

/// Spec §4 "Bad pointers" (C1: the read-only page retrace models is the program's own code). Without
/// the check the forward writes into the code's backing and the call succeeds.
#[test]
fn a_read_into_the_programs_own_code_returns_efault_and_writes_nothing() {
    let t = scratch("code.bin");
    let (out, _) = agree("readcode", &t);
    assert_eq!(out, "read(code): rc=-1 errno=14\nunchanged=1\n");
    let _ = std::fs::remove_file(&t);
}

/// A forwarded call whose out-pointer is unmapped.
#[test]
fn a_stat_into_unmapped_memory_returns_efault() {
    let t = scratch("stat.bin");
    let (out, _) = agree("statbad", &t);
    assert_eq!(out, "stat(hole): rc=-1 errno=14\n");
    let _ = std::fs::remove_file(&t);
}

/// The console write, which retrace services rather than forwards: `EFAULT`, and nothing reaches stdout.
#[test]
fn a_console_write_from_unmapped_memory_returns_efault() {
    let t = scratch("console.bin");
    let (out, events) = agree("consolebad", &t);
    assert_eq!(out, "write(1, hole): rc=-1 errno=14\n", "the report line alone: the failed write printed nothing");
    assert_eq!(failed(&events, 4), (retrace_arch::EFAULT, 0));
    let _ = std::fs::remove_file(&t);
}

/// Spec §3h.2: two calls retrace emulates, a BSD one and a Mach trap, each with native's error
/// (t0 M5: `EFAULT` for the raw `sigaltstack`, `KERN_MEMORY_ERROR` for the allocate trap).
#[test]
fn an_emulated_call_with_a_bad_pointer_returns_natives_error() {
    for (mode, want) in [("altstackbad", "sigaltstack(hole): rc=-1 errno=14\n"),
                         ("machbad", "mach_vm_allocate_trap(hole): kr=0xa\n")] {
        let t = scratch(&format!("{mode}.bin"));
        let (out, _) = agree(mode, &t);
        assert_eq!(out, want, "{mode}");
        let _ = std::fs::remove_file(&t);
    }
}

/// C3: replay makes the same check at the same point and compares its answer with the recording's. A
/// recording that says the bad read succeeded diverges at that landmark.
#[test]
fn a_recording_that_forwarded_a_pointer_replay_refuses_is_a_divergence() {
    let t = scratch("tamper.bin");
    agree("readunmapped", &t);
    let mut events = Reader::open(&t).unwrap();
    let i = events.iter().rposition(|e| matches!(e, Event::Syscall { num: 3, err: true, .. })).expect("the refused read");
    if let Event::Syscall { ret, err, .. } = &mut events[i] { *ret = 16; *err = false; }
    let mut w = retrace_trace::Writer::create(&t).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);
    let rp = util::run_bounded(util::bin(), &["replay", t.to_str().unwrap()], &[], BOUND);
    assert_eq!(rp.code, Some(3), "a divergence. stderr:\n{}", rp.stderr);
    assert!(rp.stderr.contains("replay refuses a pointer"), "the check's own compare names it. stderr:\n{}", rp.stderr);
    let _ = std::fs::remove_file(&t);
}
