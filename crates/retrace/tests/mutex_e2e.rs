// M52 Task 5 gate (spec §3f; t0 M7; rulings T0-M7-prepost and T0-M7-cvwait, C5). Contended firstfit
// `pthread_mutex`es end to end, on the repo-owned fixtures `crates/retrace-guest/c/mutex_dyn.c` and
// `cpp/mutex_cpp.cpp`. Every recorded run is compared with the same binary run natively, and every
// assertion is on the guest's output, the trace, or the recorder's own words, never an exit code
// alone: a mutex that never blocks or hands on lets most of these guests exit 0 with a short counter.
//
// Every run goes through `util::run_bounded`: a wake the model drops strands a waiter forever, and a
// hang must fail the test, not stall the gate.
mod util;

use retrace_trace::Event;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The bound on every run, in seconds. A healthy run takes a few seconds; the bound is for a hang.
const BOUND: u64 = 120;

fn cli(args: &[&str]) -> (Option<i32>, Vec<u8>, String) {
    let r = util::run_bounded(util::bin(), args, &[], BOUND);
    (r.code, r.stdout, r.stderr)
}

/// Record `guest mode` into a fresh trace, bounded: (code, stdout, stderr, trace).
fn record(guest: &str, mode: &[&str]) -> (Option<i32>, String, String, PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let trace = std::env::temp_dir().join(format!("retrace-mutex-{}-{n}.bin", std::process::id()));
    let mut args = vec!["record", "-o", trace.to_str().unwrap(), guest];
    if !mode.is_empty() {
        args.extend_from_slice(mode);
    }
    let r = util::run_bounded(util::bin(), &args, &[], BOUND);
    (r.code, String::from_utf8(r.stdout).unwrap(), r.stderr, trace)
}

/// The fixture run natively, bounded: its stdout.
fn native(guest: &str, mode: &[&str]) -> String {
    let r = util::run_bounded(guest, mode, &[], BOUND);
    assert_eq!(r.code, Some(0), "{mode:?}: native exit (None: the {BOUND} s bound fired)");
    String::from_utf8(r.stdout).unwrap()
}

/// Record, replay twice, and require native's stdout from all three. Returns the trace and stdout.
fn matches_native(guest: &str, mode: &[&str]) -> (PathBuf, String) {
    let want = native(guest, mode);
    let (code, out, err, trace) = record(guest, mode);
    assert_eq!(code, Some(0), "{mode:?}: record (None: the {BOUND} s bound fired): {err}");
    assert_eq!(out, want, "{mode:?}: retrace vs native");
    for k in 1..=2 {
        let (code, rp, err) = cli(&["replay", trace.to_str().unwrap()]);
        assert_eq!(code, Some(0), "{mode:?}: replay {k} (None: the bound fired): {err}");
        assert_eq!(String::from_utf8(rp).unwrap(), want, "{mode:?}: replay {k} stdout");
    }
    (trace, out)
}

/// Every `Syscall` landmark numbered `num`: (index, thread, args, ret).
fn landmarks(trace: &Path, num: u64) -> Vec<(usize, u32, [u64; 8], u64)> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().enumerate().filter_map(|(i, e)| match e {
        Event::Syscall { num: n, args, ret, thread, .. } if n == num => Some((i, thread, args, ret)),
        _ => None,
    }).collect()
}

/// A modelled psynch landmark records nothing: no writes, no second return, no error (§3f).
fn assert_records_nothing(trace: &Path) {
    for e in retrace_trace::Reader::open(trace).unwrap() {
        if let Event::Syscall { num: 301 | 302, ret1, err, writes, .. } = e {
            assert!(ret1 == 0 && !err && writes.is_empty(), "a mutex landmark carries only its word");
        }
    }
}

const WAIT: u64 = retrace_arch::SYS_PSYNCH_MUTEXWAIT;
const DROP: u64 = retrace_arch::SYS_PSYNCH_MUTEXDROP;

#[test]
fn two_contenders_count_exactly() {
    let (trace, out) = matches_native(retrace_guest::MUTEX_DYN, &["two"]);
    assert_eq!(out, "counter=100\n");
    assert!(!landmarks(&trace, WAIT).is_empty(), "the contenders reached the kernel");
    assert_records_nothing(&trace);
}

#[test]
fn four_contenders_count_exactly() {
    let (trace, out) = matches_native(retrace_guest::MUTEX_DYN, &["four"]);
    assert_eq!(out, "counter=200\n");
    let (waits, drops) = (landmarks(&trace, WAIT), landmarks(&trace, DROP));
    assert!(waits.len() >= 4 && !drops.is_empty(), "four contenders block in 301 and main's unlock drops: {waits:x?} {drops:x?}");
    // A wait that blocks records 0: its word, `(seq << 8) | E | K`, is written into the waiter's
    // registers at the wake and reaches the trace only as the registers' effect (box test and
    // psynch.rs unit test pin the word).
    assert!(waits.iter().all(|w| w.3 == 0), "a blocked wait's landmark records 0: {waits:x?}");
    assert!(drops.iter().all(|d| d.3 == 0), "a drop answers 0: {drops:x?}");
    assert_records_nothing(&trace);
}

/// Main re-locks across a woken waiter that has not run yet; the output and the landmarks match native.
/// This does not pin the prepost: the test stays green with it removed (T5's control (a)), because
/// retrace's `relock` does not take that path. `psynch::tests::a_prepost_is_consumed_by_exactly_one_wait`
/// is the test that pins the prepost.
#[test]
fn a_relock_across_a_woken_waiter_matches_native() {
    let (trace, out) = matches_native(retrace_guest::MUTEX_DYN, &["relock"]);
    assert_eq!(out, "relock ok\n");
    assert!(!landmarks(&trace, DROP).is_empty());
    assert_records_nothing(&trace);
}

#[test]
fn a_recursive_mutex_locked_by_main_blocks_a_child_until_main_releases_it() {
    let (trace, out) = matches_native(retrace_guest::MUTEX_DYN, &["recursive"]);
    assert_eq!(out, "order=main-unlocked-then-child-locked\n");
    assert!(!landmarks(&trace, WAIT).is_empty(), "the child's lock blocked on the owner's synthetic id");
}

#[test]
fn an_errorcheck_mutex_contended_then_relocked_answers_edeadlk() {
    let (trace, out) = matches_native(retrace_guest::MUTEX_DYN, &["errorcheck"]);
    assert_eq!(out, "relock=11 contended ok\n");
    assert!(!landmarks(&trace, WAIT).is_empty());
}

/// T0-M7-cvwait: `pthread_cond_wait` passes mutex 0 to 305, so the order in the trace is 305
/// (`args[3] == 0`), 304 under the mutex, then the woken waiter's contended re-lock, 301 on the mutex.
#[test]
fn a_condvar_signalled_under_its_contended_mutex_hands_the_mutex_on() {
    let (trace, out) = matches_native(retrace_guest::MUTEX_DYN, &["cond"]);
    assert_eq!(out, "cond ok\n");
    let cw = landmarks(&trace, retrace_arch::SYS_PSYNCH_CVWAIT);
    let cs = landmarks(&trace, retrace_arch::SYS_PSYNCH_CVSIGNAL);
    let mw = landmarks(&trace, WAIT);
    let (w, s) = (cw.first().expect("a 305"), cs.first().expect("a 304"));
    assert_eq!(w.2[3], 0, "the cvwait carries mutex 0: {w:x?}");
    let relock = mw.iter().find(|m| m.0 > s.0).unwrap_or_else(|| panic!("no 301 after the 304: {mw:x?}"));
    assert!(w.0 < s.0 && relock.2[0] != w.2[0], "305, then 304, then 301 on the mutex, not the cv: {cw:x?} {cs:x?} {mw:x?}");
    assert_records_nothing(&trace);
}

#[test]
fn a_cpp_std_mutex_under_contention_counts_exactly() {
    let (trace, out) = matches_native(retrace_guest::MUTEX_CPP, &[]);
    assert_eq!(out, "counter=100\n");
    assert!(!landmarks(&trace, WAIT).is_empty());
}

/// C5: fairshare needs a sequence-matched wake, which M52 does not port. The recorder stops by value
/// before anything is recorded or forwarded; natively the same program completes.
#[test]
fn a_fairshare_mutex_is_refused_by_value() {
    assert_eq!(native(retrace_guest::MUTEX_DYN, &["fairshare"]), "counter=100\n");
    let (code, out, err, trace) = record(retrace_guest::MUTEX_DYN, &["fairshare"]);
    assert_eq!(code, Some(4), "the recorder must stop at the wall. stderr:\n{err}");
    let lines: Vec<&str> = err.lines().filter(|l| l.starts_with("retrace: unsupported: ")).collect();
    assert_eq!(lines.len(), 1, "exactly one wall line:\n{err}");
    assert!(lines[0].contains("psynch_mutexwait") && lines[0].contains("fairshare"), "wall line: {}", lines[0]);
    assert!(!out.contains("counter="), "the guest must not run past the refusal");
    assert!(landmarks(&trace, WAIT).is_empty(), "a refused call is never recorded");
}
