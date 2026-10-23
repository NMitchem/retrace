// M52 Task 3 gate (spec §3d, §4, C12; t0 M5). `semaphore_timedwait_trap` (-38) blocks with a deadline
// on the synthetic clock, and the semaphore family keeps the kernel's count from create to destroy,
// on the repo-owned fixture `crates/retrace-guest/c/semtimed_dyn.c`. Every recorded run is compared
// with the same binary run natively, and every assertion is on the guest's output, the trace, a
// replay session's state, or the recorder's own words. Never an exit code alone.
//
// Every run goes through `util::run_bounded`: a count the model drops blocks the recorder forever
// (`count` waits on a semaphore the guest already signalled), and a hang must fail the test, not
// stall the gate.
mod util;

use retrace_box::{sleep::ns_to_ticks, SYNTH_TSC_STRIDE};
use retrace_core::ReplaySession;
use retrace_trace::Event;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The bound on every run, in seconds. A healthy run takes a few seconds; the bound is for a hang.
const BOUND: u64 = 60;

fn cli(args: &[&str]) -> (Option<i32>, Vec<u8>, String) {
    let r = util::run_bounded(util::bin(), args, &[], BOUND);
    (r.code, r.stdout, r.stderr)
}

/// Record `semtimed_dyn <mode>` into a fresh trace, bounded: (code, stdout, stderr, trace).
fn record(mode: &str, bound: u64) -> (Option<i32>, String, String, PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let trace = std::env::temp_dir().join(format!("retrace-semtimed-{}-{n}.bin", std::process::id()));
    let r = util::run_bounded(util::bin(),
        &["record-dyn", retrace_guest::SEMTIMED_DYN, "-o", trace.to_str().unwrap(), "--", mode], &[], bound);
    (r.code, String::from_utf8(r.stdout).unwrap(), r.stderr, trace)
}

/// The fixture run natively, bounded: its stdout.
fn native(mode: &str) -> String {
    let r = util::run_bounded(retrace_guest::SEMTIMED_DYN, &[mode], &[], BOUND);
    assert_eq!(r.code, Some(0), "{mode}: native exit (None: the {BOUND} s bound fired)");
    String::from_utf8(r.stdout).unwrap()
}

/// Record `mode`, replay it twice, and require native's stdout from all three. Returns the trace and
/// the recording's stderr.
fn matches_native(mode: &str) -> (PathBuf, String) {
    let want = native(mode);
    let (code, out, err, trace) = record(mode, BOUND);
    assert_eq!(code, Some(0), "{mode}: record (None: the {BOUND} s bound fired): {err}");
    assert_eq!(out, want, "{mode}: retrace vs native");
    for k in 1..=2 {
        let (code, rp, err) = cli(&["replay", trace.to_str().unwrap()]);
        assert_eq!(code, Some(0), "{mode}: replay {k} (None: the bound fired): {err}");
        assert_eq!(String::from_utf8(rp).unwrap(), want, "{mode}: replay {k} stdout");
    }
    (trace, err)
}

/// Every `Syscall` landmark numbered `num`: (index, thread, args, ret).
fn landmarks(trace: &Path, num: u64) -> Vec<(usize, u32, [u64; 8], u64)> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().enumerate().filter_map(|(i, e)| match e {
        Event::Syscall { num: n, args, ret, thread, .. } if n == num => Some((i, thread, args, ret)),
        _ => None,
    }).collect()
}

#[test]
fn a_timed_wait_nobody_signals_returns_49_after_exactly_its_timeout() {
    let (_, err) = matches_native("timeout");
    // The deadline, not just the code (spec §4): the guest clock moved by the 50 ms request and by
    // nothing more than a few clock reads, though the recorder never slept.
    let ticks: u64 = err.lines().find_map(|l| l.strip_prefix("elapsed_ticks="))
        .unwrap_or_else(|| panic!("no elapsed_ticks line in the recording's stderr:\n{err}"))
        .trim().parse().unwrap();
    let want = ns_to_ticks(50_000_000);
    assert!(ticks >= want && ticks < want + 16 * SYNTH_TSC_STRIDE,
        "elapsed {ticks} ticks, not [{want}, {want} + 16 strides)");
}

#[test]
fn a_signalled_timed_wait_returns_0() { matches_native("signalled"); }

#[test]
fn nanoseconds_of_a_second_or_more_answer_18() { matches_native("badnsec"); }

#[test]
fn two_signals_then_two_waits_return_0_twice_as_the_kernel_counts() { matches_native("count"); }

#[test]
fn a_semaphore_created_with_value_2_admits_two_waits() { matches_native("initial"); }

#[test]
fn destroying_a_semaphore_a_thread_waits_on_wakes_it_with_natives_answer() {
    // `waiter kr=37` (KERN_TERMINATED), then main's `signal kr=15` (KERN_INVALID_NAME) on the dead name.
    let (_, _) = matches_native("destroy");
    assert_eq!(native("destroy"), "waiter kr=37\nsignal kr=15\n");
}

/// C12: a 5 s wait takes no host time. The recorder's whole run, startup included, fits in 4 s.
#[test]
fn a_long_timed_wait_takes_no_host_time() {
    let (code, out, err, _) = record("long", 4);
    assert_eq!(code, Some(0), "the recording (None: the 4 s bound fired): {err}");
    assert_eq!(out, "kr=49\n");
}

/// A checkpoint taken after the second signal must carry the kernel's count (`BoxState::sems`):
/// without it the warm side's first timed wait blocks where the cold side's returns 0.
#[test]
fn a_seek_past_a_counted_signal_matches_a_cold_seek() {
    let (trace, _) = matches_native("count");
    let signals = landmarks(&trace, retrace_arch::MACH_SEMAPHORE_SIGNAL);
    let waits = landmarks(&trace, retrace_arch::MACH_SEMAPHORE_TIMEDWAIT);
    assert_eq!((signals.len(), waits.len()), (2, 3), "two signals, three timed waits");
    let at = signals[1].0 + 1;
    let to = waits[2].0 + 1;
    let from = retrace_core::seek(&trace, at, 0).unwrap();
    let state = from.dbg_internal_state();
    assert!(state.contains("sems=Semaphores { counts: {") && !state.contains("counts: {}"),
        "the checkpoint holds the kernel's count of 2:\n{state}");
    let cp = from.checkpoint();
    drop(from); // one VM per process
    let warm = {
        let mut s = ReplaySession::from_checkpoint(&trace, &cp).unwrap();
        s.advance_to_landmark(to).unwrap_or_else(|d| panic!("warm: diverged at {}: {}", d.landmark, d.detail));
        (s.current_thread(), s.dbg_regs(), s.dbg_fp_regs(), s.dbg_internal_state(), s.snapshot().1)
    };
    let cold = retrace_core::seek(&trace, to, 0).unwrap();
    assert_eq!(warm.0, cold.current_thread(), "thread: checkpointed vs cold");
    assert_eq!(warm.1, cold.dbg_regs(), "registers: checkpointed vs cold");
    assert_eq!(warm.2, cold.dbg_fp_regs(), "FP/SIMD: checkpointed vs cold");
    assert_eq!(warm.3, cold.dbg_internal_state(), "the semaphore count and the clock: checkpointed vs cold");
    assert!(cold.diff_memory(&warm.4).is_none(), "memory: checkpointed vs cold");
}

/// A `semaphore_timedwait_trap` the recording accepted but replay refuses is reachable only after an
/// earlier silent divergence, so replay names it as a `Divergence`, never a panic. The trace cannot
/// carry that divergence (the arguments are compared before the mirror runs), so the test plants its
/// effect in the box: the semaphore noted as an admitted sleep's `cond_sem`.
#[test]
fn a_timed_wait_refused_on_replay_is_a_divergence_naming_it_not_a_panic() {
    let (trace, _) = matches_native("timeout");
    let (i, _, args, _) = landmarks(&trace, retrace_arch::MACH_SEMAPHORE_TIMEDWAIT).remove(0);
    let mut s = retrace_core::seek(&trace, i, 0).unwrap();
    s.dbg_sleep_mut().note_sleep(args[0] as u32);
    let d = match s.advance() { Err(d) => d, Ok(_) => panic!("the timed wait at landmark {i} replayed over a planted sleep") };
    assert_eq!(d.landmark, i);
    assert!(d.detail.starts_with("semaphore_timedwait_trap refused on replay, though the recording accepted it")
            && d.detail.contains(&format!("semwait semaphore_timedwait_trap (-38) on port {:#x}", args[0] as u32)), "{}", d.detail);
}
