//! M51 §3d (T3-b): a fault whose handler's frame cannot be pushed is the kill native makes. Natively
//! (t0 M2) the kernel kills with SIGILL, except for a frame on the main thread's own stack, where the
//! process dies of the fault's own signal. Every cell runs natively and under retrace, and the two
//! must agree in exit code and output; the recording then replays twice to the same.
mod util;
use retrace_trace::{Event, Reader};
use std::path::PathBuf;

const BOUND: u64 = 120;
const P: &str = retrace_guest::FID_FRAMES;

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("retrace-frames-{}-{name}", std::process::id()))
}

/// One cell: native, then record and two replays, which must all agree with native in exit code and
/// stdout. Returns native's code and the recording's terminal event, for the cell's own checks.
fn cell(thread: &str, fault: &str, how: &str) -> (i32, Event) {
    let what = format!("{thread} {fault} {how}");
    let native = util::run_bounded(P, &[thread, fault, how], &[], BOUND);
    let ncode = native.code.unwrap_or_else(|| panic!("{what}: native hung (a cell t0 M2 ruled out of the matrix)"));
    let t = scratch(&format!("{thread}-{fault}-{how}.bin"));
    let tp = t.to_str().unwrap();
    let rec = util::run_bounded(util::bin(), &["record-dyn", P, "-o", tp, "--", thread, fault, how], &[], BOUND);
    assert_eq!((rec.code, String::from_utf8_lossy(&rec.stdout)), (Some(ncode), String::from_utf8_lossy(&native.stdout)),
        "{what}: record against native. stderr:\n{}", rec.stderr);
    for i in 1..=2 {
        let rp = util::run_bounded(util::bin(), &["replay", tp], &[], BOUND);
        assert_eq!((rp.code, rp.stdout.as_slice()), (rec.code, rec.stdout.as_slice()), "{what}: replay {i}. stderr:\n{}", rp.stderr);
    }
    let (events, torn) = Reader::open_checked(&t).unwrap();
    assert!(!torn, "{what}: a complete recording");
    let terminal = events[events.len() - 2].clone();
    let _ = std::fs::remove_file(&t);
    (ncode, terminal)
}

/// The `Crash`'s signal, or a panic naming what the terminal event was.
fn crash_sig(what: &str, e: &Event) -> u64 {
    match e { Event::Crash { sig, .. } => *sig, other => panic!("{what}: expected a Crash, got {other:?}") }
}

/// A main-thread stack overflow, with no handler and with one that has no alternate stack. The frame
/// would go below the overflowed stack, so the process dies of the fault's own signal, SIGSEGV.
#[test]
fn a_frame_on_the_overflowed_main_stack_kills_with_the_faults_own_signal() {
    for how in ["none", "h"] {
        let (code, e) = cell("main", "overflow", how);
        assert_eq!((code, crash_sig(how, &e)), (139, 11), "main overflow {how}");
    }
}

/// A child-thread stack overflow: with no handler it is the guard page's SIGBUS; with a handler and
/// no alternate stack the frame cannot be pushed on the child's own stack, and the kill is SIGILL.
#[test]
fn a_frame_on_an_overflowed_child_stack_kills_with_sigill() {
    let (code, e) = cell("child", "overflow", "none");
    assert_eq!((code, crash_sig("none", &e)), (138, 10), "child overflow, no handler");
    let (code, e) = cell("child", "overflow", "h");
    assert_eq!((code, crash_sig("h", &e)), (132, 4), "child overflow, a handler and no alternate stack");
}

/// A frame that can be pushed runs the handler, on either thread, on the thread's own stack or a
/// writable alternate one.
#[test]
fn a_pushable_frame_runs_the_handler() {
    for (thread, fault, how) in [("main", "null", "h"), ("child", "null", "h"), ("main", "null", "ha-rw"),
                                 ("child", "null", "ha-rw"), ("main", "overflow", "ha-rw"), ("child", "overflow", "ha-rw")] {
        let (code, e) = cell(thread, fault, how);
        assert_eq!(code, 42, "{thread} {fault} {how}: the handler ran");
        assert!(matches!(e, Event::Exit { code: 42, .. }), "{thread} {fault} {how}: {e:?}");
    }
}

/// An alternate stack made `PROT_NONE` before `sigaltstack`: the frame cannot be pushed, SIGILL.
#[test]
fn an_alternate_stack_made_prot_none_kills_with_sigill() {
    for (thread, fault) in [("main", "null"), ("child", "null"), ("main", "overflow"), ("child", "overflow")] {
        let (code, e) = cell(thread, fault, "ha-none");
        assert_eq!((code, crash_sig(thread, &e)), (132, 4), "{thread} {fault} ha-none");
    }
}

/// An alternate stack unmapped after `sigaltstack`: the frame cannot be pushed, SIGILL.
#[test]
fn an_alternate_stack_unmapped_after_sigaltstack_kills_with_sigill() {
    for (thread, fault) in [("main", "null"), ("child", "null"), ("main", "overflow"), ("child", "overflow")] {
        let (code, e) = cell(thread, fault, "ha-unmapped");
        assert_eq!((code, crash_sig(thread, &e)), (132, 4), "{thread} {fault} ha-unmapped");
    }
}

/// Pinned by value rather than against native: the child's NULL store with a `PROT_NONE` alternate
/// stack is recorded as a SIGILL crash whose ESR is still the store's data abort, and its closing line
/// says so on record and replay.
#[test]
fn an_unpushable_frame_is_recorded_as_sigill_with_the_faults_own_syndrome() {
    let t = scratch("pinned.bin");
    let tp = t.to_str().unwrap();
    let rec = util::run_bounded(util::bin(), &["record-dyn", P, "-o", tp, "--", "child", "null", "ha-none"], &[], BOUND);
    let (events, _) = Reader::open_checked(&t).unwrap();
    match &events[events.len() - 2] {
        Event::Crash { esr, far, sig, thread, .. } => {
            assert_eq!((*sig, (esr >> 26) & 0x3f, *far, *thread), (4, 0x24, 0x10, 1), "SIGILL, the store's data abort, on the child");
        }
        other => panic!("expected a Crash, got {other:?}"),
    }
    for (what, run) in [("record", rec), ("replay", util::run_bounded(util::bin(), &["replay", tp], &[], BOUND))] {
        assert_eq!(run.code, Some(132), "{what}: {}", run.stderr);
        assert!(run.stderr.lines().any(|l| l.starts_with("retrace: the program was killed by SIGILL (fault at 0x10, pc 0x")),
            "{what}: {}", run.stderr);
    }
    let _ = std::fs::remove_file(&t);
}
