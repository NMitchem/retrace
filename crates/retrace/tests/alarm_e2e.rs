// M53 t5 gate (spec §3f; t0 §M8). `setitimer`/`getitimer` (so `alarm`) run `ITIMER_REAL` on the
// synthetic clock, `sigsuspend` (so `pause`) waits for a signal, a signal ends a sleep with `EINTR`,
// and a process-directed signal reaches the main thread, all in the box and never forwarded: each
// mode of `crates/retrace-guest/c/alarm_dyn.c` is run natively, recorded and replayed twice, and the
// guest's own lines are the assertion. Before M53 `setitimer` and `sigsuspend` were walls at their
// first call. Every run is bounded: a wake that never comes leaves the guest blocked forever, which
// must fail its test rather than stall the gate.
mod util;

use retrace_core::ReplaySession;
use retrace_trace::Event;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const BOUND: u64 = 120;

fn native(mode: &str) -> (Option<i32>, String) {
    let o = util::run_bounded(retrace_guest::ALARM_DYN, &[mode], &[], BOUND);
    (o.code, String::from_utf8_lossy(&o.stdout).into_owned())
}

fn trace_path() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("retrace-alarm-{}-{}.bin", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
}

fn record(mode: &str, bound: u64) -> (util::Bounded, PathBuf) {
    let trace = trace_path();
    let rec = util::run_bounded(util::bin(),
        &["record-dyn", retrace_guest::ALARM_DYN, "-o", trace.to_str().unwrap(), "--", mode], &[], bound);
    (rec, trace)
}

/// Native, then record and two replays: the exit code and the whole of stdout agree in all four, and
/// nothing stops at a wall. Returns the output and the recording's path.
fn records_as_native(mode: &str) -> (String, PathBuf) {
    let (ncode, nout) = native(mode);
    let ncode = ncode.unwrap_or_else(|| panic!("{mode}: native hung"));
    let (rec, trace) = record(mode, BOUND);
    assert!(!rec.stderr.contains("retrace: unsupported:"), "{mode}: no wall. stderr:\n{}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout).into_owned();
    assert_eq!((rec.code, out.as_str()), (Some(ncode), nout.as_str()), "{mode}: record against native. stderr:\n{}", rec.stderr);
    for k in 1..=2 {
        let rp = util::run_bounded(util::bin(), &["replay", trace.to_str().unwrap()], &[], BOUND);
        assert_eq!((rp.code, rp.stdout.as_slice()), (rec.code, rec.stdout.as_slice()), "{mode}: replay {k}: {}", rp.stderr);
    }
    (out, trace)
}

fn events(trace: &PathBuf) -> Vec<Event> { retrace_trace::Reader::open(trace).unwrap() }

#[test]
fn a_timer_ends_pause_through_its_handler() {
    let (out, trace) = records_as_native("pause");
    assert_eq!(out, "alarm pause\nrc=-1 errno=4 handled=1\n");
    let ev = events(&trace);
    assert!(ev.iter().any(|e| matches!(e, Event::Syscall { num: retrace_arch::SYS_SETITIMER, .. })), "the setitimer landmark");
    // The timer fires below the trace (symmetry rule 2), so its delivery has no landmark of its own;
    // the handler's return is one.
    assert!(!ev.iter().any(|e| matches!(e, Event::SignalDelivery { .. })), "nothing is recorded for the timer's delivery");
    assert!(ev.iter().any(|e| matches!(e, Event::Syscall { num: retrace_arch::SYS_SIGRETURN, .. })), "the handler returned");
}

#[test]
fn an_interval_timer_fires_until_cancelled() {
    let (out, _) = records_as_native("interval");
    assert_eq!(out, "alarm interval\nruns=3\nafter the cancel runs=3\n");
}

#[test]
fn getitimer_reports_the_time_left() {
    let (out, _) = records_as_native("get");
    assert_eq!(out, "alarm get\nrc=0 remaining_ok=yes interval=0.300000\ndisarmed value=0.000000\n");
}

#[test]
fn alarm_returns_the_seconds_left_on_the_previous_timer() {
    let (out, _) = records_as_native("alarm");
    assert_eq!(out, "alarm alarm\nfirst=0 second=5\nagain 0 2 1\n");
}

#[test]
fn an_alarm_with_no_handler_kills_the_program_as_natively() {
    let (ncode, _) = native("nohandler");
    assert_eq!(ncode, Some(128 + 14), "native dies of SIGALRM");
    let (out, trace) = records_as_native("nohandler");
    assert_eq!(out, "alarm nohandler\n", "nothing runs past the pause");
    let ev = events(&trace);
    let sigs: Vec<_> = ev.iter().filter_map(|e| match e { Event::Signal { sig, thread, .. } => Some((*sig, *thread)), _ => None }).collect();
    assert_eq!(sigs, [(14, 0)], "one terminal Event::Signal, SIGALRM, on the thread that was current");
    assert!(matches!(ev.last(), Some(Event::Snapshot { .. })), "the final memory snapshot follows");
}

#[test]
fn a_signal_to_a_program_that_ignores_it_changes_nothing() {
    let (out, _) = records_as_native("ignored");
    assert_eq!(out, "alarm ignored\nslept left=0 handled=0\n");
}

#[test]
fn the_timers_signal_reaches_the_thread_xnu_picks() {
    let (out, trace) = records_as_native("target");
    assert_eq!(out, "alarm target\n\
        alarm, neither masks: ran on main=1 worker=0\n\
        alarm, main masks: ran on main=0 worker=1\n\
        \x20 after main unblocks: ran on main=0 worker=1\n\
        alarm, worker masks: ran on main=1 worker=0\n\
        kill from an unmasked worker: ran on main=1 worker=0\n\
        kill from a worker that masks it: ran on main=1 worker=0\n\
        kill, main masks: ran on main=0 worker=0\n\
        \x20 after main unblocks: ran on main=1 worker=0\n");
    // The kill arm's pick is a decision both sides make; the timer's deliveries are not events.
    // Every kill in the fixture reaches main: its workers' masks are the same as main's, or main's mask is clear.
    let deliveries: Vec<u32> = events(&trace).iter().filter_map(|e| match e {
        Event::SignalDelivery { thread, .. } => Some(*thread), _ => None }).collect();
    assert!(!deliveries.is_empty() && deliveries.iter().all(|&t| t == 0), "{deliveries:?}");
}

#[test]
fn sigsuspend_returns_eintr_after_the_handler_and_restores_the_mask() {
    let (out, trace) = records_as_native("suspend");
    assert_eq!(out, "alarm suspend\nrc=-1 errno=4 handled=1 mask_restored=yes\n");
    let ev = events(&trace);
    let at = ev.iter().position(|e| matches!(e, Event::Syscall { num: retrace_arch::SYS_SIGSUSPEND | retrace_arch::SYS_SIGSUSPEND_NOCANCEL, .. }))
        .expect("the sigsuspend landmark");
    assert!(matches!(&ev[at], Event::Syscall { ret: 0, err: false, .. }), "it blocks: the landmark returns 0 and the answer comes at the wake");
    // The same call with the signal already pending returns at once, with the handler's delivery
    // as the landmark right after the call's.
    let (out, trace) = records_as_native("pending");
    assert_eq!(out, "alarm pending\nrc=-1 errno=4 handled=1 mask_restored=yes\n");
    let ev = events(&trace);
    let at = ev.iter().position(|e| matches!(e, Event::Syscall { num: retrace_arch::SYS_SIGSUSPEND | retrace_arch::SYS_SIGSUSPEND_NOCANCEL, .. }))
        .expect("the sigsuspend landmark");
    assert!(matches!(&ev[at], Event::Syscall { ret: 4, err: true, .. }), "EINTR, recorded");
    assert!(matches!(&ev[at + 1], Event::SignalDelivery { sig: 30, thread: 0, .. }), "and the handler's delivery follows it");
}

#[test]
fn an_alarm_ends_a_sleep_early() {
    let (out, _) = records_as_native("sleeper");
    assert_eq!(out, "alarm sleeper\nleft=4 handled=1\n");
    // t0 M8: a MASKED signal does not interrupt; the sleep returns 0 and the handler runs at the unmask.
    let (out, _) = records_as_native("masked");
    assert_eq!(out, "alarm masked\nleft=0 handled=0\nafter the unmask handled=1\n");
}

#[test]
fn a_bad_timer_argument_is_einval_as_natively() {
    let (out, _) = records_as_native("einval");
    assert_eq!(out, "alarm einval\nusec 1000000 rc=-1 errno=22\nsec -1 rc=-1 errno=22\nwhich 7 rc=-1 errno=22\ngetitimer which 7 rc=-1 errno=22\nkill 32 rc=-1 errno=22\nkill -1 rc=-1 errno=22\npthread_kill 32 rc=22\n");
}

#[test]
fn a_virtual_timer_is_refused_by_value() {
    let (rec, _) = record("virtual", BOUND);
    assert_eq!(rec.code, Some(4), "the recorder stops at the wall. stderr:\n{}", rec.stderr);
    let lines: Vec<&str> = rec.stderr.lines().filter(|l| l.starts_with("retrace: unsupported: ")).collect();
    assert_eq!(lines.len(), 1, "{}", rec.stderr);
    assert!(lines[0].contains("setitimer (syscall 83) with ITIMER_VIRTUAL") && lines[0].ends_with("#signals"), "{}", lines[0]);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), "alarm virtual\n", "nothing ran past the call");
}

/// `alarm(60)` and `pause()` end by the idle jump: no host time passes. The bound is 10 seconds, so a
/// timer that waited its 60 would be killed by it. Natively this takes the full minute, so it is not
/// compared with native here; `pause` is, with a 100 ms timer.
#[test]
fn a_long_alarm_takes_no_host_time() {
    let (rec, trace) = record("long", 10);
    assert_eq!(rec.code, Some(0), "stderr:\n{}", rec.stderr);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), "alarm long\nrc=-1 errno=4 handled=1\n");
    let rp = util::run_bounded(util::bin(), &["replay", trace.to_str().unwrap()], &[], 10);
    assert_eq!((rp.code, rp.stdout.as_slice()), (Some(0), rec.stdout.as_slice()), "replay: {}", rp.stderr);
}

/// A checkpoint taken just after `setitimer`, continued past the `pause` that the timer ends, must
/// equal a cold seek there: the timer in `BoxState` (box state), the registers, the saved contexts and memory.
#[test]
fn a_seek_across_the_timers_fire_matches_a_cold_seek() {
    let (_, trace) = records_as_native("pause");
    let evs: Vec<(usize, Event)> = events(&trace).into_iter().enumerate().collect();
    let set = evs.iter().find_map(|(i, e)| matches!(e, Event::Syscall { num: retrace_arch::SYS_SETITIMER, .. }).then_some(*i))
        .expect("the setitimer landmark");
    let pause = evs.iter().find_map(|(i, e)| match e {
        Event::Syscall { num: retrace_arch::SYS_SIGSUSPEND | retrace_arch::SYS_SIGSUSPEND_NOCANCEL, .. } => Some(*i),
        _ => None }).expect("the pause landmark");
    // One VM per process: each session ends before the next is built.
    let cp = {
        let from = retrace_core::seek(&trace, set + 1, 0).unwrap();
        let state = from.dbg_internal_state();
        assert!(!state.contains("itimer=RealTimer { deadline: None"), "the checkpoint carries the armed timer:\n{state}");
        from.checkpoint()
    };
    let to = pause + 1;
    let warm = {
        let mut s = ReplaySession::from_checkpoint(&trace, &cp).unwrap();
        s.advance_to_landmark(to).unwrap_or_else(|d| panic!("warm: diverged at {}: {}", d.landmark, d.detail));
        (s.current_thread(), s.dbg_regs(), s.dbg_internal_state(), s.snapshot().1)
    };
    let cold = retrace_core::seek(&trace, to, 0).unwrap();
    assert_eq!(warm.0, cold.current_thread(), "thread: checkpointed vs cold");
    assert_eq!(warm.1, cold.dbg_regs(), "registers: checkpointed vs cold");
    assert_eq!(warm.2, cold.dbg_internal_state(), "box state: checkpointed vs cold");
    assert!(cold.diff_memory(&warm.3).is_none(), "memory: checkpointed vs cold");
}
