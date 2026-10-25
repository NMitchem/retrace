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

use retrace_core::{Advance, BlockReason, ReplaySession, ThreadState};
use retrace_trace::Event;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The bound on every run, in seconds. A healthy run here takes a few seconds; the bound is for a
/// hang.
const BOUND: u64 = 120;

/// `mach_msg2_trap`'s number, as the trace keys it.
const MACH_MSG2: u64 = (-47i64) as u64;

/// The 24 MHz guest clock: ticks per second (M48 plan F6).
const TICKS_PER_SEC: u64 = 24_000_000;

/// SDK `mach/clock_types.h` (`retrace_box::sleep::CALENDAR_CLOCK`, which this crate does not
/// depend on).
const CALENDAR_CLOCK: u32 = 1;

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

/// The guest clock at a session's position: the synthetic counter plus the commpage's timebase
/// offset (`_COMM_PAGE_TIMEBASE_OFFSET`, commpage + 0x88), as the guest's `mach_absolute_time`
/// computes it.
fn guest_clock(s: &ReplaySession) -> u64 {
    let off = s.read_mem_prefix(0xf_ffff_c088, 8);
    synthetic_tsc(&s.dbg_internal_state()).wrapping_add(u64::from_le_bytes(off.try_into().expect("the commpage is mapped")))
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
/// it 5 ticks since M52, 9216 before). Returns (that landmark, the clock after it, the session).
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
        assert_eq!(code, Some(4), "{what}: the recorder stops at the wall (None: the bound fired). stderr:\n{err}");
        let line = err.lines().find(|l| l.starts_with("retrace: unsupported: ")).unwrap_or_else(|| panic!("{what}: no wall line:\n{err}"));
        assert!(line.contains(why), "{what}: {why:?} in the wall line: {line}");
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
/// retrace the signal pends (the target is blocked) and the wake that would deliver it stops at a wall, by
/// name, on record and on replay of what was recorded alike: the saved context of a thread blocked
/// in `__semwait_signal` is unmeasured, and EINTR is M50's.
#[test]
fn a_signal_to_a_sleeping_thread_is_refused_at_the_wake_on_both_sides() {
    let (code, out) = native(&["sigsleeper"]);
    assert_eq!((code, out.as_str()), (0, "sigsleeper woke left=1 got=1\nsigsleeper done\n"), "native");
    let (code, out, err, trace) = record(&["sigsleeper"]);
    assert_eq!(code, Some(4), "the recorder stops at the wake's wall (None: the bound fired). stderr:\n{err}");
    let line = err.lines().find(|l| l.starts_with("retrace: unsupported: "))
        .unwrap_or_else(|| panic!("no wall line:\n{err}"));
    assert!(line.contains("M49: sleep of thread 1 reached its deadline with a signal pending (set 0x20000000)")
            && line.ends_with("#threads"), "{line}");
    assert!(out.is_empty(), "nothing ran past the refusal: {}", String::from_utf8_lossy(&out));
    let sleeps = semwaits(&trace);
    assert_eq!(sleeps.iter().map(|l| (l.1, l.2[4], l.2[5])).collect::<Vec<_>>(), [(0, 0, 100_000_000), (1, 1, 0)],
        "main's 100 ms, then the sleeper's 1 s");
    let (code, _, err) = cli(&["replay", trace.to_str().unwrap()]);
    assert_eq!(code, Some(4), "replay reaches the same wake (below the trace) at the recording's end: {err}");
    let line = err.lines().find(|l| l.starts_with("retrace: incomplete recording: "))
        .unwrap_or_else(|| panic!("no incomplete-recording line:\n{err}"));
    // C2: the `at` is the wall replay met there itself, which is what this test has always guarded.
    assert!(line.contains("M49: sleep of thread 1 reached its deadline with a signal pending"), "{line}");
}

/// §4 S3 (two sleepers): a 2 s sleeper created first and a 1 s sleeper created second wake in
/// deadline order, as native's ranks show, each by one idle jump to its own deadline, and each with
/// ETIMEDOUT and the carry in its own context. A queue in blocking order wakes the 2 s sleeper
/// first.
#[test]
fn two_sleepers_wake_in_deadline_order_each_on_its_own_deadline() {
    let (out, trace) = records_and_replays_as_native(&["twosleepers"], &[]);
    assert_eq!(out, "twosleepers 2s rank=1 1s rank=0\n");
    let sleeps = semwaits(&trace);
    assert_eq!(sleeps.len(), 2, "{sleeps:x?}");
    let two = sleeps.iter().find(|l| l.2[4] == 2).expect("the 2 s sleep");
    let one = sleeps.iter().find(|l| l.2[4] == 1).expect("the 1 s sleep");
    assert!(two.0 < one.0 && two.1 != one.1 && two.1 != 0 && one.1 != 0, "two sleeper threads, the 2 s one first");
    let (at_two, clock_two) = {
        let s = session_at_svc(&trace, two.0);
        (synthetic_tsc(&s.dbg_internal_state()), guest_clock(&s))
    }; // one VM per process: this session is dropped before the next is made
    let s = session_at_svc(&trace, one.0);
    let at_one = synthetic_tsc(&s.dbg_internal_state());
    let (n, after, s) = the_idle_jump(s);
    assert_eq!((n, after), (one.0, at_one + TICKS_PER_SEC), "the first jump is to the 1 s deadline");
    assert_eq!(s.current_thread(), one.1, "the 1 s sleeper runs first");
    assert_woken_with_etimedout(&s, one.1 as usize);
    let blocked = s.thread_summaries()[two.1 as usize].state;
    assert_eq!(blocked, ThreadState::Blocked(BlockReason::Sleep { deadline: clock_two + 2 * TICKS_PER_SEC }),
        "the 2 s sleeper still sleeps, until its own deadline");
    let (_, after, s) = the_idle_jump(s);
    assert_eq!(after, at_two + 2 * TICKS_PER_SEC, "the second jump is to the 2 s deadline");
    assert_eq!(s.current_thread(), two.1);
    assert_woken_with_etimedout(&s, two.1 as usize);
}

/// Review Focus 5 for the sleep: a `__semwait_signal` the recording accepted but replay refuses is
/// reachable only after an earlier silent divergence, so replay names it as a `Divergence`, never a
/// panic. The trace cannot carry that divergence (the arguments are compared before the mirror
/// runs), so the test plants its effect in the box: the sleep's `cond_sem` noted as a semaphore.
#[test]
fn a_sleep_refused_on_replay_is_a_divergence_naming_it_not_a_panic() {
    let (_, trace) = records_and_replays_as_native(&["usleep"], &[1_000_000]);
    let (i, _, args, ..) = semwaits(&trace).remove(0);
    let mut s = retrace_core::seek(&trace, i, 0).unwrap();
    s.dbg_sleep_mut().note_semaphore(args[0] as u32);
    let d = match s.advance() { Err(d) => d, Ok(_) => panic!("the sleep at landmark {i} replayed over a planted semaphore") };
    assert_eq!(d.landmark, i);
    assert!(d.detail.starts_with("__semwait_signal refused on replay, though the recording accepted it")
            && d.detail.contains(&format!("M49: semwait cond_sem {:#x} was used", args[0] as u32)), "{}", d.detail);
}

/// The same for `clock_get_time`: its port planted as a CALENDAR_CLOCK port before the 1000.
#[test]
fn a_clock_read_refused_on_replay_is_a_divergence_naming_it_not_a_panic() {
    let (_, trace) = records_and_replays_as_native(&["sleep1"], &[1_000_000_000]);
    let (i, _, args, ..) = msgs(&trace, 1000).remove(0);
    let mut s = retrace_core::seek(&trace, i, 0).unwrap();
    s.dbg_sleep_mut().learn_clock_port(args[3] & 0xffff_ffff, CALENDAR_CLOCK);
    let d = match s.advance() { Err(d) => d, Ok(_) => panic!("the clock read at landmark {i} replayed on a CALENDAR port") };
    assert_eq!(d.landmark, i);
    assert!(d.detail.starts_with("clock_get_time refused on replay, though the recording accepted it")
            && d.detail.contains("a CALENDAR_CLOCK (1) port"), "{}", d.detail);
}

/// S2 (R7): the 1000's reply is SYSTEM_CLOCK's reading of the guest clock at the call, `ticks ×
/// 125/3` ns, byte for byte; the mirror recomputes it, so a rewritten reply is a divergence naming
/// the call. The 334's mirror compares its return and its empty answer the same way.
#[test]
fn the_clock_reply_is_the_guest_clock_and_rewritten_answers_are_divergences() {
    let (_, trace) = records_and_replays_as_native(&["sleep1"], &[1_000_000_000]);
    let (r, _, args, _, _, _, writes) = msgs(&trace, 1000).remove(0);
    let ns = guest_clock(&session_at_svc(&trace, r)) as u128 * 125 / 3;
    let reply = retrace_core::machmsg::encode_clock_get_time_reply((args[3] >> 32) as u32,
        (ns / 1_000_000_000) as u32, (ns % 1_000_000_000) as i32);
    assert_eq!(writes.len(), 1);
    assert_eq!((writes[0].ipa, &writes[0].bytes), (args[0], &reply), "the reply is the guest clock, at the message buffer");
    let i = semwaits(&trace)[0].0;
    for (ext, at, why) in [
        ("rc.bin", i, "__semwait_signal rc mismatch: replay 0x0 != recorded 0x1"),
        ("ret1.bin", i, "__semwait_signal recorded ret1=0x1 err=false with 0 write(s)"),
        ("reply.bin", r, "clock_get_time reply mismatch"),
    ] {
        let mut ev = retrace_trace::Reader::open(&trace).unwrap();
        if let Event::Syscall { ret, ret1, writes, .. } = &mut ev[at] {
            match ext { "rc.bin" => *ret = 1, "ret1.bin" => *ret1 = 1, _ => writes[0].bytes[40] ^= 1 }
        }
        let bad = trace.with_extension(ext);
        let mut w = retrace_trace::Writer::create(&bad).unwrap();
        for e in &ev { w.append(e).unwrap(); }
        drop(w);
        let (code, _, err) = cli(&["replay", bad.to_str().unwrap()]);
        assert_eq!(code, Some(3), "{ext}: replay of the rewritten trace must diverge (exit 3): {err}");
        assert!(err.contains(&format!("DIVERGENCE at landmark {at} ")) && err.contains(why),
            "{ext}: the divergence must be the mirror's {why:?}, at landmark {at}: {err}");
    }
}

/// A seek across a sleep. A checkpoint taken while the 2 s sleeper is asleep and before the 1 s
/// sleeper's clock read, continued past both wakes, must equal a cold seek in every register, both
/// sleepers' saved contexts, box state and memory. The checkpoint must carry the learned clock port
/// (`BoxState::sleep`): without it, the 1 s sleeper's clock read is refused on the warm side.
#[test]
fn a_seek_across_a_sleep_matches_a_cold_seek() {
    let (_, trace) = records_and_replays_as_native(&["twosleepers"], &[]);
    let sleeps = semwaits(&trace);
    let two = sleeps.iter().find(|l| l.2[4] == 2).unwrap();
    let reads = msgs(&trace, 1000);
    assert!(reads.iter().any(|r| r.0 > two.0), "a clock read comes after the checkpoint");
    let from = retrace_core::seek(&trace, two.0 + 1, 0).unwrap();
    let state = from.dbg_internal_state();
    assert!(state.contains("sleep=SleepModel { clock_ports: {") && !state.contains("clock_ports: {}"),
        "the checkpoint holds the learned clock port:\n{state}");
    assert!(matches!(from.thread_summaries()[two.1 as usize].state, ThreadState::Blocked(BlockReason::Sleep { .. })),
        "the checkpoint is inside the 2 s sleep");
    let (n, _, _) = the_idle_jump(the_idle_jump(from).2);
    let to = n + 2;
    let cp = retrace_core::seek(&trace, two.0 + 1, 0).unwrap().checkpoint();
    let warm = {
        let mut s = ReplaySession::from_checkpoint(&trace, &cp).unwrap();
        s.advance_to_landmark(to).unwrap_or_else(|d| panic!("warm: diverged at {}: {}", d.landmark, d.detail));
        (s.current_thread(), s.dbg_regs(), s.dbg_regs_of(1), s.dbg_regs_of(2), s.dbg_fp_regs(), s.dbg_internal_state(), s.snapshot().1)
    };
    let cold = retrace_core::seek(&trace, to, 0).unwrap();
    assert_eq!(warm.0, cold.current_thread(), "thread: checkpointed vs cold");
    assert_eq!(warm.1, cold.dbg_regs(), "registers: checkpointed vs cold");
    assert_eq!(warm.2, cold.dbg_regs_of(1), "thread 1's context: checkpointed vs cold");
    assert_eq!(warm.3, cold.dbg_regs_of(2), "thread 2's context: checkpointed vs cold");
    assert_eq!(warm.4, cold.dbg_fp_regs(), "FP/SIMD: checkpointed vs cold");
    assert_eq!(warm.5, cold.dbg_internal_state(), "the sleep model and the clock: checkpointed vs cold");
    assert!(cold.diff_memory(&warm.6).is_none(), "memory: checkpointed vs cold");
}

/// Review Focus 1's short shapes. `usleep(1)` is 1000 ns, 24 ticks; `nanosleep` of 1 ns is 0
/// ticks, a deadline already reached at the call, which wakes in the settle of its own landmark
/// with no jump, on the vCPU (the only thread is the current one). A wake lost to the saved context
/// leaves x0 0, which `nanosleep` returns as -1.
#[test]
fn a_one_microsecond_and_a_zero_tick_sleep_wake_exactly_and_on_the_vcpu() {
    let (_, trace) = records_and_replays_as_native(&["usleep1"], &[1_000]);
    assert_sleep_landmark(&semwaits(&trace)[0], 0, 1_000);
    // Final-review I2: a zero-tick sleep on the only thread is a due poll, which moves the guest clock
    // one stride (5 ticks = 208 ns) where it moved it 0 before.
    let (_, trace) = records_and_replays_as_native(&["onens"], &[208]);
    let sleeps = semwaits(&trace);
    assert_sleep_landmark(&sleeps[0], 0, 1);
    let mut s = session_at_svc(&trace, sleeps[0].0);
    let at_call = synthetic_tsc(&s.dbg_internal_state());
    s.advance().unwrap_or_else(|d| panic!("diverged at landmark {}: {}", d.landmark, d.detail));
    assert_eq!((s.landmark(), s.current_thread()), (sleeps[0].0 + 1, 0), "woken in its own settle");
    assert_eq!(synthetic_tsc(&s.dbg_internal_state()), at_call + retrace_box::SYNTH_TSC_STRIDE,
        "one stride and no idle jump: the deadline was due at the call");
    assert_woken_with_etimedout(&s, 0);
}

/// Review Focus 1: a polling loop with a short sleep, waiting on another thread. Main sleeps 100 µs
/// at a time until a worker that never blocks sets its flag. The worker runs only while main
/// sleeps, so a sleep that does not block (or never wakes) hangs the recorder, and the bound turns
/// that hang into this test's failure. The output carries no count, so native's is matched exactly.
#[test]
fn a_polling_loop_with_a_short_sleep_waits_for_its_worker_and_terminates() {
    let (out, trace) = records_and_replays_as_native(&["pollloop"], &[]);
    assert_eq!(out, "pollloop done sum=5000050000\n");
    let sleeps = semwaits(&trace);
    assert!(!sleeps.is_empty(), "the loop slept at least once");
    for l in &sleeps {
        assert_eq!(l.1, 0, "only main sleeps");
        assert_sleep_landmark(l, 0, 100_000);
    }
}

/// Ruling T5-I1, where Review Focus 1's two halves meet: a polling loop on a 0-tick sleep. Each
/// `nanosleep` of 1 ns is due in the very settle that blocked it, so without the yield rule the
/// poller, the lowest-indexed runnable thread, is picked again at once and the worker it waits on
/// never runs: the recorder livelocks, and the bound turns that into this test's failure. With it,
/// the sleeper yields to the next runnable thread after it, and the loop ends as it does natively.
#[test]
fn a_polling_loop_on_a_zero_tick_sleep_yields_to_its_worker_and_terminates() {
    let (out, trace) = records_and_replays_as_native(&["pollzero"], &[]);
    assert_eq!(out, "pollzero done sum=5000050000\n");
    let sleeps = semwaits(&trace);
    assert!(!sleeps.is_empty(), "the loop slept at least once");
    for l in &sleeps {
        assert_eq!(l.1, 0, "only main sleeps");
        assert_sleep_landmark(l, 0, 1);
    }
}

/// Review Focus 5: a sleep while a sibling thread is runnable. Main sleeps 1 s; the worker runs
/// (it prints and exits without blocking) before the clock jumps, so `worker ran` precedes main's
/// line. The guest's delta across main's sleep is still exactly 1 s: the deadline is fixed at the
/// call, the worker's timebase reads move the clock by 5 ticks each (M52; 9216 before) and only
/// before the jump, and the jump lands on the deadline itself. Only a worker reading the clock
/// more than 24000000 / 5 = 4 800 000 times could carry it past the deadline; this one makes one write.
#[test]
fn a_sleep_beside_a_runnable_thread_lets_it_run_and_still_lasts_exactly_one_second() {
    let (out, trace) = records_and_replays_as_native(&["sleepwhilework"], &[1_000_000_000]);
    assert_eq!(out, "worker ran\nmain slept left=0 delta_ns=1000000000\n");
    let sleeps = semwaits(&trace);
    assert_eq!(sleeps.len(), 1, "{sleeps:x?}");
    assert_eq!(sleeps[0].1, 0, "main sleeps");
    let worker_write = events(&trace).into_iter().position(|(_, e)| matches!(e,
        Event::Syscall { num: 4, thread, .. } if thread != 0)).expect("the worker's write");
    assert!(worker_write > sleeps[0].0, "the worker writes while main sleeps");
}

/// M52 §3h, C9: a program that reads the clock 4 000 000 times, about one native second of the
/// densest measured suite (memchr's), must see less than one second pass in its own readings, as it
/// does natively. At the old 0x2400 ticks per read that is 1536 guest seconds and it prints `no`.
/// Run bounded, because 4 M reads are 4 M VM exits (about 4 s of host time per record or replay).
#[test]
fn a_program_that_reads_the_clock_as_often_as_a_long_test_suite_stays_under_one_guest_second() {
    let want = "under_one_guest_second=yes\n";
    let r = util::run_bounded(retrace_guest::CLOCKREADS, &[], &[], BOUND);
    assert_eq!(String::from_utf8_lossy(&r.stdout), want, "native");
    let trace = std::env::temp_dir().join(format!("retrace-clockreads-{}.bin", std::process::id()));
    let (code, out, err) = cli(&["record-dyn", retrace_guest::CLOCKREADS, "-o", trace.to_str().unwrap()]);
    assert_eq!(code, Some(0), "record (None: the {BOUND} s bound fired): {err}");
    assert_eq!(String::from_utf8(out).unwrap(), want, "recorded");
    let (code, rp, err) = cli(&["replay", trace.to_str().unwrap()]);
    assert_eq!(code, Some(0), "replay (None: the bound fired): {err}");
    assert_eq!(String::from_utf8(rp).unwrap(), want, "replayed");
    std::fs::remove_file(&trace).ok();
}
