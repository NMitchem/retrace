// M55 Task 3 gate (spec §3h; t0 `p0/semsig`, `p0/pipethread`, T0-semtimed, T0-carry). A signal to a thread
// that is blocked: a terminating one ends the process at the raise whatever the target is doing, a
// handled one interrupts a semaphore wait or a `kevent` with native's measured return and runs its
// handler on the thread, and a handled one to a thread blocked where interruption is not modelled (a
// condition variable) is refused at the raise. Each mode of `crates/retrace-guest/c/sigblocked_dyn.c`
// is run natively, recorded and replayed twice. Every run is bounded: a wake that never comes leaves
// the guest blocked forever, which must fail its test rather than stall the gate.
mod util;

use retrace_trace::Event;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const BOUND: u64 = 120;

fn native(mode: &str) -> (Option<i32>, String) {
    let o = util::run_bounded(retrace_guest::SIGBLOCKED_DYN, &[mode], &[], BOUND);
    (o.code, String::from_utf8_lossy(&o.stdout).into_owned())
}

fn trace_path() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("retrace-sigblocked-{}-{}.bin", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
}

fn record(mode: &str) -> (util::Bounded, PathBuf) {
    let trace = trace_path();
    let rec = util::run_bounded(util::bin(),
        &["record-dyn", retrace_guest::SIGBLOCKED_DYN, "-o", trace.to_str().unwrap(), "--", mode], &[], BOUND);
    (rec, trace)
}

/// Native, then record and two replays: the exit code and the whole of stdout agree in all four, and
/// nothing stops at a wall. Returns the output and the recording's path.
fn records_as_native(mode: &str) -> (String, PathBuf) {
    let (ncode, nout) = native(mode);
    let ncode = ncode.unwrap_or_else(|| panic!("{mode}: native hung"));
    let (rec, trace) = record(mode);
    assert!(!rec.stderr.contains("retrace: unsupported:"), "{mode}: no wall. stderr:\n{}", rec.stderr);
    assert!(rec.code.is_some(), "{mode}: the recording hung. stderr:\n{}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout).into_owned();
    assert_eq!((rec.code, out.as_str()), (Some(ncode), nout.as_str()), "{mode}: record against native. stderr:\n{}", rec.stderr);
    for k in 1..=2 {
        let rp = util::run_bounded(util::bin(), &["replay", trace.to_str().unwrap()], &[], BOUND);
        assert_eq!((rp.code, rp.stdout.as_slice()), (rec.code, rec.stdout.as_slice()), "{mode}: replay {k}: {}", rp.stderr);
    }
    (out, trace)
}

fn events(trace: &PathBuf) -> Vec<Event> { retrace_trace::Reader::open(trace).unwrap() }

// T0-carry: the frame's carry is whatever the thread entered the trap with, and the semaphore call is
// entered with it clear, so the kernel's frame shows it clear.
const SEM_FRAME: &str = "  frame: x0=0xe C=0 svc=1\n";
const KEVENT_FRAME: &str = "  frame: x0=0x4 C=1 svc=1\n";

#[test]
fn a_signal_interrupts_a_semaphore_wait_with_kern_aborted() {
    let (out, trace) = records_as_native("sem");
    assert_eq!(out, format!("sem: first=14 second=0 handled=1 on_main=1\n{SEM_FRAME}"));
    let ev = events(&trace);
    assert!(ev.iter().any(|e| matches!(e, Event::SignalDelivery { sig: 30, thread: 0, .. })),
        "the handler was delivered to main");
}

#[test]
fn a_thread_directed_signal_interrupts_a_semaphore_wait() {
    let (out, _) = records_as_native("semthread");
    assert_eq!(out, format!("semthread: first=14 second=0 handled=1 on_main=1\n{SEM_FRAME}"));
}

#[test]
fn a_timed_semaphore_wait_is_interrupted_as_native() {
    // T0-semtimed: 14, nothing consumed, and the re-issued wait still waits for the worker's signal.
    let (out, _) = records_as_native("semtimed");
    assert_eq!(out, format!("semtimed: first=14 second=0 handled=1 on_main=1\n{SEM_FRAME}"));
}

#[test]
fn a_signal_interrupts_kevent_with_eintr() {
    let (out, _) = records_as_native("kevent");
    assert_eq!(out, format!("kevent: n=-1 errno=4 then=1 handled=1 on_main=1\n{KEVENT_FRAME}"));
}

#[test]
fn a_thread_directed_signal_interrupts_kevent() {
    let (out, _) = records_as_native("keventthread");
    assert_eq!(out, format!("keventthread: n=-1 errno=4 then=1 handled=1 on_main=1\n{KEVENT_FRAME}"));
}

#[test]
fn a_handled_signal_to_a_condvar_waiter_is_refused_at_the_raise() {
    let (rec, _) = record("cv");
    assert!(rec.stderr.contains(
        "retrace: unsupported: M55: a handled signal (SIGUSR1) to thread 0, blocked in psynch_cvwait"),
        "the refusal names the signal and the call. stderr:\n{}", rec.stderr);
    assert_eq!(rec.code, Some(4), "a wall exits 4. stderr:\n{}", rec.stderr);
    assert!(!rec.stderr.contains("DEADLOCK"), "not the deadlock wall, whose text would be false. stderr:\n{}", rec.stderr);
}

#[test]
fn a_terminating_signal_to_a_joining_main_ends_the_process() {
    let (out, trace) = records_as_native("term");
    assert!(!out.contains("killer: still running"), "the worker never prints after the raise: {out:?}");
    let (rec, _) = (util::run_bounded(retrace_guest::SIGBLOCKED_DYN, &["term"], &[], BOUND), ());
    assert_eq!(rec.code, Some(143), "native dies of SIGTERM");
    let ev = events(&trace);
    let last_landmark = ev.iter().rev().find(|e| !matches!(e, Event::Snapshot { .. })).unwrap();
    assert!(matches!(last_landmark, Event::Signal { sig: 15, .. }), "the last landmark is the terminal: {last_landmark:?}");
}

#[test]
fn a_masked_signal_rides_through_a_semaphore_wake() {
    let (out, _) = records_as_native("maskedsem");
    assert_eq!(out, "maskedsem: wait=0 handled=0\nafter unmask: handled=1 on_main=1\n");
}

#[test]
fn a_masked_signal_rides_through_a_kevent_wake() {
    let (out, _) = records_as_native("maskedkevent");
    assert_eq!(out, "maskedkevent: n=1 handled=0\nafter unmask: handled=1 on_main=1\n");
}

// M55 Task 4 (spec §3f; t0 `p0/pipethread`, T0-nosigpipe): a write that returns EPIPE on a guest pipe or socket
// end raises SIGPIPE, as a process-directed signal with `si_code` 0 and `si_pid` the process's own. Before M55
// the write returned EPIPE and nothing was raised: the host's SIGPIPE went to the recorder, which ignores it.

#[test]
fn sigpipe_reaches_a_joining_main_as_native() {
    let (out, trace) = records_as_native("pipehandler");
    assert_eq!(out, "worker: write r=-1 errno=32 ran=0\nmain: joined ran=1 on_main=1 si_code=0 si_pid_is_self=1\n");
    let ev = events(&trace);
    assert!(ev.iter().any(|e| matches!(e, Event::SignalDelivery { sig: 13, thread: 0, .. })),
        "the handler was delivered to main, not the writer");
}

#[test]
fn sigpipe_reaches_the_writer_when_main_masks_it() {
    let (out, trace) = records_as_native("pipemainmask");
    assert_eq!(out, "worker: write r=-1 errno=32 ran=1\nmain: joined ran=1 on_main=0 si_code=0 si_pid_is_self=1\n");
    let ev = events(&trace);
    assert!(ev.iter().any(|e| matches!(e, Event::SignalDelivery { sig: 13, thread, .. } if *thread != 0)),
        "the handler was delivered to the writer");
}

#[test]
fn a_default_sigpipe_ends_the_process_with_141() {
    let (out, trace) = records_as_native("pipedfl");
    assert_eq!(out, "", "the worker never prints after its write");
    let native = util::run_bounded(retrace_guest::SIGBLOCKED_DYN, &["pipedfl"], &[], BOUND);
    assert_eq!(native.code, Some(141), "native dies of SIGPIPE");
    let ev = events(&trace);
    let last = ev.iter().rev().find(|e| !matches!(e, Event::Snapshot { .. })).unwrap();
    assert!(matches!(last, Event::Signal { sig: 13, thread, .. } if *thread != 0), "the terminal is SIGPIPE, tagged with the writer: {last:?}");
}
