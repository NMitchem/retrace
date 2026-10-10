// M49 gate (spec §3f, §4; R6–R9; Review Focus 1 and 5). Sleep on the synthetic clock, end to end,
// on the repo-owned fixture `crates/retrace-guest/c/sleep_dyn.c`. Every recorded run is compared
// with the same binary run natively, and every assertion is on the guest's output, the trace, a
// replay session's state, or the recorder's own words. Never an exit code alone: libc's `nanosleep`
// returns 0 for a sleep that never blocked as readily as for one that did.
//
// Every run, native or under the CLI, goes through `util::run_bounded`: a sleep the model gets
// wrong can hang the recorder (a sleep that never blocks starves the thread a polling loop waits
// on), and a hang must fail the test, not stall the gate.
mod util;

use retrace_core::{Advance, ReplaySession};
use retrace_trace::Event;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The bound on every run, in seconds. A healthy run here takes a few seconds; the bound is for a
/// hang.
const BOUND: u64 = 120;

/// `mach_msg2_trap`'s number, as the trace keys it.
const MACH_MSG2: u64 = (-47i64) as u64;

/// `retrace <args>`, bounded: (code, stdout, stderr). `None` for the code means the bound fired.
fn cli(args: &[&str]) -> (Option<i32>, Vec<u8>, String) {
    let r = util::run_bounded(util::bin(), args, &[], BOUND);
    (r.code, r.stdout, r.stderr)
}

/// Record `sleep_dyn <argv>` into a fresh trace, bounded: (code, stdout, stderr, trace).
fn record(argv: &[&str]) -> (Option<i32>, Vec<u8>, String, PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let trace = std::env::temp_dir().join(format!("retrace-sleep-{}-{n}.bin", std::process::id()));
    let mut args = vec!["record-dyn", retrace_guest::SLEEP_DYN, "-o", trace.to_str().unwrap(), "--"];
    args.extend_from_slice(argv);
    let (code, out, err) = cli(&args);
    (code, out, err, trace)
}

/// The fixture run natively, bounded: (exit code, stdout).
fn native(argv: &[&str]) -> (i32, String) {
    let r = util::run_bounded(retrace_guest::SLEEP_DYN, argv, &[], BOUND);
    let code = r.code.unwrap_or_else(|| panic!("{argv:?}: the native run outlived the {BOUND} s bound"));
    (code, String::from_utf8_lossy(&r.stdout).into_owned())
}

/// `out` with every `delta_ns=<n>` replaced by `delta_ns=N`, and the values in order.
fn masked(out: &str) -> (String, Vec<u64>) {
    let mut vals = Vec::new();
    let lines: Vec<String> = out.lines().map(|l| match l.split_once("delta_ns=") {
        Some((head, v)) => { vals.push(v.parse().unwrap_or_else(|_| panic!("delta_ns in {l:?}"))); format!("{head}delta_ns=N") }
        None => l.to_string(),
    }).collect();
    (lines.join("\n") + "\n", vals)
}

/// Record `argv`, then replay it twice. Asserts:
/// - native exits 0, and every native `delta_ns` is at least `requested` (the kernel never wakes early);
/// - the recording exits 0 inside the bound, with native's output once each `delta_ns` is masked;
/// - every recorded `delta_ns` is EXACTLY `requested`: the idle jump lands on the deadline (R6);
/// - two replays, each inside the bound, print the recording's bytes.
///
/// Returns the recorded stdout and the trace.
fn records_and_replays_as_native(argv: &[&str], requested: &[u64]) -> (String, PathBuf) {
    let (code, want) = native(argv);
    assert_eq!(code, 0, "{argv:?}: native exit");
    let (want_masked, native_deltas) = masked(&want);
    assert_eq!(native_deltas.len(), requested.len(), "{argv:?}: native's delta lines: {want}");
    for (d, r) in native_deltas.iter().zip(requested) {
        assert!(d >= r, "{argv:?}: native slept {d} ns for a {r} ns request");
    }
    let (code, out, err, trace) = record(argv);
    assert_eq!(code, Some(0), "{argv:?}: record (None: the {BOUND} s bound fired): {err}");
    let out = String::from_utf8(out).unwrap();
    let (got_masked, deltas) = masked(&out);
    assert_eq!(got_masked, want_masked, "{argv:?}: retrace vs native, delta_ns masked");
    assert_eq!(deltas, requested, "{argv:?}: R6: the guest clock moves by exactly the request across each sleep");
    for k in 1..=2 {
        let (code, rp, err) = cli(&["replay", trace.to_str().unwrap()]);
        assert_eq!(code, Some(0), "{argv:?}: replay {k} (None: the bound fired): {err}");
        assert_eq!(String::from_utf8(rp).unwrap(), out, "{argv:?}: replay {k} stdout");
    }
    (out, trace)
}

/// Every event of `trace` with its landmark index (the replay session's `idx`).
fn events(trace: &Path) -> Vec<(usize, Event)> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().enumerate().collect()
}

/// One landmark: (index, thread, args, ret, ret1, err, writes).
type Lm = (usize, u32, [u64; 8], u64, u64, bool, Vec<retrace_trace::Region>);

/// Every `__semwait_signal` landmark.
fn semwaits(trace: &Path) -> Vec<Lm> {
    events(trace).into_iter().filter_map(|(i, e)| match e {
        Event::Syscall { num, args, ret, ret1, err, writes, thread } if num == retrace_arch::SYS___SEMWAIT_SIGNAL =>
            Some((i, thread, args, ret, ret1, err, writes)),
        _ => None,
    }).collect()
}

/// Every `mach_msg2` landmark whose `msgh_id` is `id`.
fn msgs(trace: &Path, id: u64) -> Vec<Lm> {
    events(trace).into_iter().filter_map(|(i, e)| match e {
        Event::Syscall { num, args, ret, ret1, err, writes, thread } if num == MACH_MSG2 && args[4] >> 32 == id =>
            Some((i, thread, args, ret, ret1, err, writes)),
        _ => None,
    }).collect()
}

/// S3: the landmark is the call: ret 0, ret1 0, carry clear, no writes (the answer comes at the
/// wake), and the arguments are libc `nanosleep`'s shape for `(sec, nsec)`.
fn assert_sleep_landmark(l: &Lm, sec: u64, nsec: u64) {
    let (i, _, args, ret, ret1, err, writes) = l;
    assert_eq!((*ret, *ret1, *err, writes.len()), (0, 0, false, 0), "landmark {i}: the 334 records the call, not the wake");
    assert_eq!(&args[1..6], &[0, 1, 1, sec, nsec], "landmark {i}: mutex 0, timeout 1, relative 1, {sec} s {nsec} ns");
    assert_ne!(args[0] as u32, 0, "landmark {i}: libc's clock_sem");
}

fn synthetic_tsc(state: &str) -> u64 {
    let v = state.split("synthetic_tsc=0x").nth(1).and_then(|r| r.split_whitespace().next())
        .unwrap_or_else(|| panic!("no synthetic_tsc in {state}"));
    u64::from_str_radix(v, 16).unwrap()
}

/// A session at landmark `i`'s `svc`, not yet retired (`kq_e2e`'s helper).
fn session_at_svc(trace: &Path, i: usize) -> ReplaySession {
    const SVC_0X80: [u8; 4] = 0xd400_1001u32.to_le_bytes();
    let mut s = retrace_core::seek(trace, i, 0).unwrap();
    for _ in 0..1_000_000 {
        if s.read_mem_prefix(s.pc(), 4) == SVC_0X80 { return s; }
        s.step_insns(1).unwrap_or_else(|e| panic!("stepping window {i}: {e}"));
    }
    panic!("no svc within 1M instructions of landmark {i}");
}

/// Advance `s` until one landmark moves the clock by more than 60000 ticks (a timebase read moves
/// it 9216). Returns (that landmark, the clock after it, the session).
fn the_idle_jump(mut s: ReplaySession) -> (usize, u64, ReplaySession) {
    loop {
        let (n, before) = (s.landmark(), synthetic_tsc(&s.dbg_internal_state()));
        match s.advance() {
            Ok(Advance::Exited(_)) => panic!("the run exited and no landmark jumped the clock"),
            Ok(_) => {}
            Err(d) => panic!("diverged at landmark {}: {}", d.landmark, d.detail),
        }
        let after = synthetic_tsc(&s.dbg_internal_state());
        if after - before > 60_000 { return (n, after, s); }
    }
}

/// S3's wake on thread `tid`: `ETIMEDOUT` (60) with the carry set, the kernel's unsignalled timeout.
fn assert_woken_with_etimedout(s: &ReplaySession, tid: usize) {
    let ctx = s.thread_ctx(tid).unwrap();
    assert_eq!(ctx.regs.x[0], retrace_arch::ETIMEDOUT, "thread {tid}'s x0 after the wake");
    assert_ne!(ctx.regs.cpsr & retrace_arch::PSTATE_C, 0, "thread {tid}'s carry after the wake");
}

/// §4 S1/S3 and R6, for `sleep(1)`. libc's `sleep` reads SYSTEM_CLOCK first (it passes a `rem`),
/// so the trace holds one 1000 and then one 334; the guest's own clock moves by exactly 1 s across
/// the call, though the recorder never sleeps; and both replays agree. Native's delta is 1 s plus
/// the scheduler's overshoot, so only its bound is compared.
#[test]
fn sleep_one_second_reads_the_clock_then_lands_exactly_on_its_deadline() {
    let (_, trace) = records_and_replays_as_native(&["sleep1"], &[1_000_000_000]);
    let (sleeps, reads) = (semwaits(&trace), msgs(&trace, 1000));
    assert_eq!((sleeps.len(), reads.len()), (1, 1), "one clock read, one sleep: {sleeps:x?} {reads:x?}");
    assert_sleep_landmark(&sleeps[0], 1, 0);
    assert!(reads[0].0 < sleeps[0].0, "the clock read comes before the sleep (nanosleep with rem)");
}

/// The same for the paths that skip the clock read (`usleep`, `nanosleep(…, NULL)`), at 1 ms and
/// 5 ms. Neither issues a 1000 (sleep-report.md §1).
#[test]
fn usleep_and_nanosleep_without_rem_sleep_exactly_and_read_no_clock() {
    for (mode, ns) in [("usleep", 1_000_000), ("nanosleep_null", 5_000_000)] {
        let (_, trace) = records_and_replays_as_native(&[mode], &[ns]);
        let sleeps = semwaits(&trace);
        assert_eq!(sleeps.len(), 1, "{mode}: {sleeps:x?}");
        assert_sleep_landmark(&sleeps[0], 0, ns);
        assert!(msgs(&trace, 1000).is_empty(), "{mode}: no clock_get_time without rem");
    }
}

/// S3's answer, seen by the guest: a hand-issued `__semwait_signal` on a fresh semaphore for 5 ms
/// returns -1 with errno 60, as native does, and the idle jump lands exactly 120000 ticks after the
/// call's own clock. `nanosleep` returns 0 for ETIMEDOUT and -1 for a 0 return alike, so only this
/// mode sees the value the wake writes.
#[test]
fn the_wake_answers_etimedout_with_the_carry_set_after_exactly_the_request() {
    let (out, trace) = records_and_replays_as_native(&["raw"], &[]);
    assert_eq!(out, "raw r=-1 errno=60\n");
    let sleeps = semwaits(&trace);
    assert_sleep_landmark(&sleeps[0], 0, 5_000_000);
    let s = session_at_svc(&trace, sleeps[0].0);
    let at_call = synthetic_tsc(&s.dbg_internal_state());
    let (n, after, s) = the_idle_jump(s);
    assert_eq!((n, after), (sleeps[0].0, at_call + 120_000), "the jump is the sleep's own settle, to its deadline");
    assert_woken_with_etimedout(&s, 0);
}

/// S4 (R8): each unmodelled shape stops the recorder by value, naming its field, before anything
/// reaches the host or the trace. Natively every one of them returns (the `refuse <what> done`
/// line); under retrace the guest never gets there.
#[test]
fn every_unmodelled_sleep_shape_stops_the_recorder_naming_it() {
    for (what, native_line, why) in [
        ("relative0", "refuse relative0 r=-1 errno=60", "M49: semwait relative 0: an absolute deadline"),
        ("timeout0", "refuse timeout0 r=0 errno=0", "M49: semwait timeout 0: only timeout 1 is modelled"),
        ("mutex", "refuse mutex r=-1 errno=60", "M49: semwait mutex_sem 0x"),
        ("nsec", "refuse nsec r=-1 errno=22", "M49: semwait tv_nsec 1000000000:"),
        ("semtrap", "refuse semtrap r=0 errno=0", "was used with semaphore_wait_trap (-36) or semaphore_signal_trap (-33)"),
        ("nosem", "refuse nosem r=-1 errno=22", "M49: semwait cond_sem 0: native answers EINVAL"),
        ("nocancel", "refuse nocancel r=-1 errno=60", "M49: semwait __semwait_signal_nocancel (423) is not modelled"),
        ("calendar", "refuse calendar kr=0", "M49: clock_get_time on port 0x"),
    ] {
        let (code, out) = native(&["refuse", what]);
        assert_eq!((code, out), (0, format!("{native_line}\nrefuse {what} done\n")), "{what}: native returns");
        let (code, out, err, trace) = record(&["refuse", what]);
        assert_eq!(code, Some(101), "{what}: the recorder must stop at the refusal (None: the bound fired). stderr:\n{err}");
        assert!(err.contains(why), "{what}: {why:?} in stderr:\n{err}");
        assert!(out.is_empty(), "{what}: the guest must not run past the refusal: {}", String::from_utf8_lossy(&out));
        assert!(semwaits(&trace).is_empty(), "{what}: a refused call is never recorded");
        if what == "calendar" {
            assert!(err.contains("a CALENDAR_CLOCK (1) port"), "{err}");
            assert!(msgs(&trace, 1000).is_empty(), "a refused clock_get_time is never recorded");
            assert_eq!(msgs(&trace, 206).len(), 2, "both clock ports were fetched and recorded (libc's SYSTEM_CLOCK, then CALENDAR_CLOCK)");
        }
    }
}

/// S5 (R9): a signal to a sleeping thread is refused, not delivered. `sigsleeper`'s main signals a
/// thread asleep in `sleep(1)`; natively the handler runs and `sleep` returns the 1 s left. Under
/// retrace the signal pends (the target is blocked) and the wake that would deliver it aborts by
/// name, on record and on replay of what was recorded alike: the saved context of a thread blocked
/// in `__semwait_signal` is unmeasured, and EINTR is M50's.
#[test]
fn a_signal_to_a_sleeping_thread_is_refused_at_the_wake_on_both_sides() {
    let (code, out) = native(&["sigsleeper"]);
    assert_eq!((code, out.as_str()), (0, "sigsleeper woke left=1 got=1\nsigsleeper done\n"), "native");
    let (code, out, err, trace) = record(&["sigsleeper"]);
    assert_eq!(code, Some(101), "the recorder must stop at the wake (None: the bound fired). stderr:\n{err}");
    assert!(err.contains("M49: sleep of thread 1 reached its deadline with a signal pending (set 0x20000000)"), "{err}");
    assert!(out.is_empty(), "nothing ran past the refusal: {}", String::from_utf8_lossy(&out));
    let sleeps = semwaits(&trace);
    assert_eq!(sleeps.iter().map(|l| (l.1, l.2[4], l.2[5])).collect::<Vec<_>>(), [(0, 0, 100_000_000), (1, 1, 0)],
        "main's 100 ms, then the sleeper's 1 s");
    let (code, _, err) = cli(&["replay", trace.to_str().unwrap()]);
    assert_eq!(code, Some(101), "replay reaches the same wake (below the trace): {err}");
    assert!(err.contains("M49: sleep of thread 1 reached its deadline with a signal pending"), "{err}");
}
