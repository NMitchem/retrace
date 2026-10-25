// M52 Task 4 gate (spec §3e, R7, R11; t0 M6). `swtch_pri` (-59) and `thread_switch` (-61) yield to the
// next runnable thread with one clock stride, and a timed wait already due when it blocks yields first,
// on the repo-owned fixtures `crates/retrace-guest/c/yield_dyn.c` and `rs/yieldspin.rs`. Every recorded
// run is compared with the same binary run natively; every assertion is on the guest's output, the
// trace, or the recorder's own words, never an exit code alone.
//
// Every run goes through `util::run_bounded`: a poll the scheduler hands straight back to the poller
// spins forever, and a hang must fail the test, not stall the gate.
mod util;

use retrace_box::{sleep::ns_to_ticks, SYNTH_TSC_STRIDE};
use retrace_trace::Event;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The bound on every run, in seconds. A healthy run takes a few seconds; the bound is for a hang.
const BOUND: u64 = 60;

/// Record `guest mode` into a fresh trace, bounded: (code, stdout, stderr, trace).
fn record(guest: &str, mode: &[&str]) -> (Option<i32>, String, String, PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let trace = std::env::temp_dir().join(format!("retrace-yield-{}-{n}.bin", std::process::id()));
    let mut argv = vec!["record-dyn", guest, "-o", trace.to_str().unwrap()];
    if !mode.is_empty() { argv.push("--"); argv.extend_from_slice(mode); }
    let r = util::run_bounded(util::bin(), &argv, &[], BOUND);
    (r.code, String::from_utf8(r.stdout).unwrap(), r.stderr, trace)
}

/// The fixture run natively, bounded: its stdout.
fn native(guest: &str, mode: &[&str]) -> String {
    let r = util::run_bounded(guest, mode, &[], BOUND);
    assert_eq!(r.code, Some(0), "{mode:?}: native exit (None: the {BOUND} s bound fired)");
    String::from_utf8(r.stdout).unwrap()
}

/// Record, replay twice, and require native's stdout from all three. Returns the trace and the
/// recording's stderr.
fn matches_native(guest: &str, mode: &[&str]) -> (PathBuf, String) {
    let want = native(guest, mode);
    let (code, out, err, trace) = record(guest, mode);
    assert_eq!(code, Some(0), "{mode:?}: record (None: the {BOUND} s bound fired): {err}");
    assert_eq!(out, want, "{mode:?}: retrace vs native");
    for k in 1..=2 {
        let r = util::run_bounded(util::bin(), &["replay", trace.to_str().unwrap()], &[], BOUND);
        assert_eq!(r.code, Some(0), "{mode:?}: replay {k} (None: the bound fired): {}", r.stderr);
        assert_eq!(String::from_utf8(r.stdout).unwrap(), want, "{mode:?}: replay {k} stdout");
    }
    (trace, err)
}

/// Every `Syscall` landmark numbered `num`: (index, ret).
fn landmarks(trace: &Path, num: u64) -> Vec<(usize, u64)> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().enumerate().filter_map(|(i, e)| match e {
        Event::Syscall { num: n, ret, .. } if n == num => Some((i, ret)),
        _ => None,
    }).collect()
}

/// The `key=<n>` field of a stderr line the fixture printed.
fn field(err: &str, key: &str) -> u64 {
    err.split_whitespace().find_map(|w| w.strip_prefix(key).and_then(|v| v.strip_prefix('=')))
        .unwrap_or_else(|| panic!("no {key}= in the recording's stderr:\n{err}")).parse().unwrap()
}

#[test]
fn a_sched_yield_runs_a_runnable_peer_first() {
    let (_, err) = matches_native(retrace_guest::YIELD_DYN, &["peer"]);
    // One yield and no more: the child was runnable, ran at the first, and set the flag.
    assert_eq!(err.lines().find(|l| l.starts_with("yields=")), Some("yields=1"), "stderr:\n{err}");
}

#[test]
fn a_sole_yielder_beside_a_sleeper_reaches_its_wake_one_stride_per_yield() {
    let (_, err) = matches_native(retrace_guest::YIELD_DYN, &["sleeper"]);
    let (yields, ticks) = (field(&err, "yields"), field(&err, "elapsed_ticks"));
    // R11: each yield moved the clock one stride and nothing jumped it, so the ticks main saw are the
    // yields' strides plus a few clock reads, and the yields alone cover the child's 10 ms sleep.
    assert!(yields * SYNTH_TSC_STRIDE <= ticks && ticks <= yields * SYNTH_TSC_STRIDE + 8 * SYNTH_TSC_STRIDE,
        "{yields} yields, {ticks} ticks: not [yields * stride, yields * stride + 8 strides]");
    assert!(yields * SYNTH_TSC_STRIDE >= ns_to_ticks(10_000_000),
        "{yields} yields did not cover 10 ms: the clock jumped (R11)");
}

#[test]
fn a_back_off_spinner_wakes_from_its_own_deadline_and_a_60_s_wait_does_not_time_out() {
    // "received" is main's recv_timeout(60 s) returning the child's message; a clock jumped on the
    // sole yielder would carry main past its deadline and print the timeout instead.
    let (_, _) = matches_native(retrace_guest::YIELDSPIN, &[]);
    assert_eq!(native(retrace_guest::YIELDSPIN, &[]), "received\nparked_by_timeout=yes\n");
}

#[test]
fn pthread_yield_np_and_every_measured_thread_switch_option_return_0() {
    let (trace, _) = matches_native(retrace_guest::YIELD_DYN, &["np"]);
    // The trap numbers, pinned: pthread_yield_np is swtch_pri, not thread_switch (t0 M6).
    let swtch = landmarks(&trace, retrace_arch::MACH_SWTCH_PRI);
    let switch = landmarks(&trace, retrace_arch::MACH_THREAD_SWITCH);
    assert_eq!((swtch.len(), switch.len()), (1, 6), "one swtch_pri, six thread_switch");
    assert!(swtch.iter().chain(&switch).all(|&(_, ret)| ret == 0), "every yield returns 0");
}

#[test]
fn an_unmeasured_thread_switch_option_is_refused_by_value() {
    let (code, _, err, trace) = record(retrace_guest::YIELD_DYN, &["badopt"]);
    let lines: Vec<&str> = err.lines().filter(|l| l.starts_with("retrace: unsupported:")).collect();
    assert_eq!(lines.len(), 1, "exactly one unsupported line; stderr:\n{err}");
    assert!(lines[0].contains("thread_switch") && lines[0].contains("option 4"), "{}", lines[0]);
    assert_eq!(code, Some(4), "a wall exits 4; stderr:\n{err}");
    let r = util::run_bounded(util::bin(), &["replay", trace.to_str().unwrap()], &[], BOUND);
    assert!(r.stderr.lines().any(|l| l.starts_with("retrace: incomplete recording: it ends at ")),
        "replay ends at the incomplete-recording line:\n{}", r.stderr);
    assert!(!r.stderr.contains("DIVERGENCE"), "the end of a walled recording is not a divergence");
}

#[test]
fn a_zero_timeout_semaphore_poll_beside_a_producer_finishes() { matches_native(retrace_guest::YIELD_DYN, &["sempoll"]); }

#[test]
fn a_zero_timeout_kevent_poll_beside_a_writer_finishes() { matches_native(retrace_guest::YIELD_DYN, &["kqpoll"]); }

// Final-review I2: a due `{0,0}` poll beside a thread that is timed-blocked (the child usleeps 10 ms
// first). Nothing else is runnable, so the poll re-picks itself; before the fix it added no stride and
// took no idle jump, so the clock never reached the sleeper's deadline and the loop spun forever.
#[test]
fn a_zero_timeout_semaphore_poll_beside_a_sleeping_producer_finishes() {
    matches_native(retrace_guest::YIELD_DYN, &["sempoll-sleeper"]);
}

#[test]
fn a_zero_timeout_kevent_poll_beside_a_sleeping_writer_finishes() {
    matches_native(retrace_guest::YIELD_DYN, &["kqpoll-sleeper"]);
}
