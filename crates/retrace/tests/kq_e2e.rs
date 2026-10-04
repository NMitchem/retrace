// M48 gate (spec §3c, §3d, §3h; K1). The guest's own kqueues, end to end, on the repo-owned
// fixture `crates/retrace-guest/c/kq_dyn.c`. Every recorded run is compared with the SAME binary
// run natively, and every assertion is on the guest's output, the trace, a replay session's state,
// or the recorder's own words. Never an exit code alone: before M48 the recorder stopped at the
// first kevent, and a model that answered every wait with "0 events" would still exit 0.
mod util;

use retrace_core::{Advance, ReplaySession};
use retrace_trace::Event;
use std::path::{Path, PathBuf};

/// One `kevent` landmark: (landmark index, thread, args, ret).
type Kev = (usize, u32, [u64; 8], u64);

/// The fixture run natively, with stdout a pipe, as `Command::output` gives it and as the recorder's
/// own stdout is under test.
fn native(argv: &[&str]) -> Vec<u8> {
    let o = std::process::Command::new(retrace_guest::KQ_DYN).args(argv).output().unwrap();
    assert!(o.status.success(), "native kq_dyn {argv:?}: {}", String::from_utf8_lossy(&o.stderr));
    o.stdout
}

/// Every event of `trace` with its landmark index (the replay session's `idx`).
fn events(trace: &Path) -> Vec<(usize, Event)> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().enumerate().collect()
}

fn kevents(trace: &Path) -> Vec<Kev> {
    events(trace).into_iter().filter_map(|(i, e)| match e {
        Event::Syscall { num, args, ret, thread, .. } if num == retrace_arch::SYS_KEVENT => Some((i, thread, args, ret)),
        _ => None,
    }).collect()
}

/// Record `argv`, and assert:
/// - exit 0 with native's exact output;
/// - R3: every kevent landmark carries no writes and `ret1` 0, and there is at least one;
/// - two byte-identical replays.
fn records_as_native(argv: &[&str]) -> (util::RunOut, PathBuf) {
    let (rec, trace) = util::record_dynamic_args(retrace_guest::KQ_DYN, argv);
    assert_eq!(rec.code, 0, "{argv:?}: record: {}", rec.stderr);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), String::from_utf8_lossy(&native(argv)), "{argv:?}: retrace vs native");
    let ks: Vec<(usize, u64, usize)> = events(&trace).into_iter().filter_map(|(i, e)| match e {
        Event::Syscall { num, ret1, writes, .. } if num == retrace_arch::SYS_KEVENT => Some((i, ret1, writes.len())),
        _ => None,
    }).collect();
    assert!(!ks.is_empty(), "{argv:?}: no kevent landmark, so this run proves nothing about the model");
    for (i, ret1, writes) in ks {
        assert!(ret1 == 0 && writes == 0, "{argv:?}: R3: kevent landmark {i} recorded ret1={ret1:#x} and {writes} writes");
    }
    for k in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "{argv:?}: replay {k}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "{argv:?}: replay {k} stdout");
    }
    (rec, trace)
}

/// The first landmark after `after` that `thread` issues.
fn next_on(trace: &Path, thread: u32, after: usize) -> usize {
    events(trace).into_iter().find_map(|(i, e)| match e {
        Event::Syscall { thread: t, .. } if i > after && t == thread => Some(i),
        _ => None,
    }).unwrap_or_else(|| panic!("no landmark of thread {thread} after {after}"))
}

fn synthetic_tsc(state: &str) -> u64 {
    let v = state.split("synthetic_tsc=0x").nth(1).and_then(|r| r.split_whitespace().next())
        .unwrap_or_else(|| panic!("no synthetic_tsc in {state}"));
    u64::from_str_radix(v, 16).unwrap()
}

/// A session at landmark `i`'s `svc`: the instruction that issues it, not yet retired
/// (`gcdtimer_e2e`'s helper). A tamper at `(i, 0)` would be overwritten inside window `i`.
fn session_at_svc(trace: &Path, i: usize) -> ReplaySession {
    const SVC_0X80: [u8; 4] = 0xd400_1001u32.to_le_bytes();
    let mut s = retrace_core::seek(trace, i, 0).unwrap();
    for _ in 0..1_000_000 {
        if s.read_mem_prefix(s.pc(), 4) == SVC_0X80 {
            return s;
        }
        s.step_insns(1).unwrap_or_else(|e| panic!("stepping window {i}: {e}"));
    }
    panic!("no svc within 1M instructions of landmark {i}");
}

/// Advance `s` until one landmark moves the clock by more than 60000 ticks (2.5 ms; a timebase read
/// moves it 9216). Returns (that landmark, the clock after it).
fn the_idle_jump(mut s: ReplaySession) -> (usize, u64, ReplaySession) {
    loop {
        let (n, before) = (s.landmark(), synthetic_tsc(&s.dbg_internal_state()));
        match s.advance() {
            Ok(Advance::Exited(_)) => panic!("the run exited and no landmark jumped the clock"),
            Ok(_) => {}
            Err(d) => panic!("diverged at landmark {}: {}", d.landmark, d.detail),
        }
        let after = synthetic_tsc(&s.dbg_internal_state());
        if after - before > 60_000 {
            return (n, after, s);
        }
    }
}

/// A checkpoint at `(from, 0)`, continued to `to`, must equal a cold seek to `(to, 0)`, in every
/// register, main's saved context, box state (the guest kqueues included) and memory.
fn warm_matches_cold(trace: &Path, from: usize, to: usize) -> ReplaySession {
    let cp = retrace_core::seek(trace, from, 0).unwrap().checkpoint();
    let warm = {
        let mut s = ReplaySession::from_checkpoint(trace, &cp).unwrap();
        s.advance_to_landmark(to).unwrap_or_else(|d| panic!("warm: diverged at {}: {}", d.landmark, d.detail));
        (s.current_thread(), s.dbg_regs(), s.dbg_regs_of(0), s.dbg_fp_regs(), s.dbg_internal_state(), s.snapshot().1)
    };
    let cold = retrace_core::seek(trace, to, 0).unwrap();
    assert_eq!(warm.0, cold.current_thread(), "thread: checkpointed vs cold");
    assert_eq!(warm.1, cold.dbg_regs(), "registers: checkpointed vs cold");
    assert_eq!(warm.2, cold.dbg_regs_of(0), "main's saved context: checkpointed vs cold");
    assert_eq!(warm.3, cold.dbg_fp_regs(), "FP/SIMD: checkpointed vs cold");
    assert_eq!(warm.4, cold.dbg_internal_state(), "box state, the guest kqueues included: checkpointed vs cold");
    assert!(cold.diff_memory(&warm.5).is_none(), "memory: checkpointed vs cold");
    cold
}

/// Main's wait in `wake`, `timeout` and `pipe`: thread 0, no changes, one event.
fn main_wait(ks: &[Kev]) -> Kev {
    *ks.iter().find(|k| k.1 == 0 && k.2[2] == 0 && k.2[4] == 1).expect("main's kevent wait")
}

/// Review Focus 1. libuv's runtime detection passes one buffer as both lists. Native returns ONE
/// event over slot 0 and leaves the second change in slot 1; the kqueue is closed and its fd
/// reused, empty. The difference M48 makes: before it the recorder stopped at this call.
#[test]
fn the_runtime_detection_probe_returns_natives_one_event() {
    let (rec, trace) = records_as_native(&["probe"]);
    assert!(String::from_utf8_lossy(&rec.stdout).starts_with(
        "detect n=1 [ident=0x1e7e7711 filter=-10 flags=0x21 fflags=0 data=0 udata=0]\n\
         slot1 n=1 [ident=0x1e7e7711 filter=-10 flags=0 fflags=0x1000000 data=0 udata=0]\n"),
        "walls.md §1 row 1, native/kqdetect.out: {:?}", String::from_utf8_lossy(&rec.stdout));
    let ks = kevents(&trace);
    assert_eq!(ks.len(), 2, "detect, then the reused kqueue's poll: {ks:?}");
    let (_, _, args, ret) = ks[0];
    assert_eq!((args[1] == args[3], args[2], args[4], ret), (true, 2, 1, 1),
        "one buffer as both lists, two changes, one event slot, one event");
    assert_eq!((ks[1].2[0], ks[1].3), (args[0], 0), "the reused fd, and an empty kqueue behind it");
}

/// §3c: the `uv_async_send` shape. Main blocks with no timeout; a second thread's trigger (one
/// change, no event list) wakes it. The wake writes main's reply into its SAVED context at the
/// trigger's own landmark, while the waker keeps running.
#[test]
fn a_trigger_from_another_thread_wakes_the_blocked_waiter_after_the_wakers_landmark() {
    let (_, trace) = records_as_native(&["wake"]);
    let ks = kevents(&trace);
    let wait = main_wait(&ks);
    let trig = *ks.iter().find(|k| k.1 != 0).expect("the trigger, from the second thread");
    assert_eq!(wait.3, 0, "a blocking landmark records 0: the answer comes at the wake");
    assert_eq!((trig.2[2], trig.2[4], trig.3), (1, 0, 0), "one change, no event list, 0 (F7)");
    assert!(wait.0 < trig.0 && next_on(&trace, 0, wait.0) > trig.0,
        "main waits, the waker triggers, and only then does main run again: wait {}, trigger {}", wait.0, trig.0);
    let s = retrace_core::seek(&trace, trig.0 + 1, 0).unwrap();
    assert_eq!(s.current_thread(), trig.1, "a wake does not switch: the waker still runs");
    let regs = s.dbg_regs_of(0).unwrap();
    assert!(regs.contains("x0 =0x0000000000000001"), "main's saved x0 is its one event, written at the wake:\n{regs}");
}

/// §3d, R7: a 5 ms wait with nothing to wake it ends by the idle jump, which lands EXACTLY on the
/// deadline: 120000 ticks after the call's own clock (F6), not past it.
#[test]
fn a_five_millisecond_timeout_is_reached_by_the_idle_jump() {
    let (rec, trace) = records_as_native(&["timeout"]);
    assert_eq!(rec.stdout, b"timeout n=0 waited=1\n");
    let wait = main_wait(&kevents(&trace));
    assert_ne!(wait.2[5], 0, "a timed wait");
    let s = session_at_svc(&trace, wait.0);
    let at_call = synthetic_tsc(&s.dbg_internal_state());
    let (n, after, s) = the_idle_jump(s);
    assert!(n > wait.0, "the jump comes after main's wait, once the other thread has exited");
    assert_eq!(after, at_call + 120_000, "F6: 5 ms is 120000 ticks, from the clock the call read");
    assert_eq!(s.current_thread(), 0, "the jump woke main, the only thread left");
}

/// Review Focus 2, end to end: libuv's `uv__stream_try_select` on the only thread. Its 1 ns wait
/// converts to 0 ticks, blocks, and is woken in its own landmark's settle with no jump, while it is
/// still the current thread. A kevent deadline answer is 0, as is the blocking landmark's, so this
/// test cannot tell a reply on the vCPU from one lost to the saved context; the box test
/// `a_wake_of_the_current_thread_writes_the_vcpu` pins that with a distinct reply. What it does pin:
/// the same-settle wake (no deadlock, no jump), K3's never-ready answer on fd 1, and native's output.
#[test]
fn a_timeout_on_the_only_thread_answers_on_the_vcpu() {
    let (rec, trace) = records_as_native(&["tryselect"]);
    assert_eq!(rec.stdout, b"tryselect n=0 waited=0\n");
    let ks = kevents(&trace);
    assert_eq!(ks.len(), 1, "{ks:?}");
    let (i, thread, args, ret) = ks[0];
    assert_eq!((thread, args[2], args[4], ret), (0, 1, 1, 0), "one change, one event slot, 0 events");
    let mut s = session_at_svc(&trace, i);
    let ch = s.read_mem_prefix(args[1], 32);
    assert_eq!((&ch[..8], &ch[8..10], &ch[10..12]),
        (&1u64.to_le_bytes()[..], &retrace_arch::EVFILT_READ.to_le_bytes()[..], &0x5u16.to_le_bytes()[..]),
        "EVFILT_READ, EV_ADD|EV_ENABLE on fd 1 (walls.md §1)");
    let at_call = synthetic_tsc(&s.dbg_internal_state());
    s.advance().unwrap_or_else(|d| panic!("diverged at landmark {}: {}", d.landmark, d.detail));
    assert_eq!((s.landmark(), s.current_thread()), (i + 1, 0), "woken in its own settle, on the only thread");
    assert_eq!(synthetic_tsc(&s.dbg_internal_state()), at_call, "no idle jump: the deadline was due at the call");
    assert!(s.dbg_regs().contains("x0 =0x0000000000000000"), "x0 on the vCPU:\n{}", s.dbg_regs());
}

/// R4, F9: a write's return moves the pipe's count and wakes the reader blocked on its read end,
/// with native's `data` (the count) and, after the write end closes, native's `EV_EOF`.
#[test]
fn a_pipe_write_wakes_the_reader_with_natives_byte_count_and_eof() {
    let (rec, trace) = records_as_native(&["pipe"]);
    let out = String::from_utf8_lossy(&rec.stdout);
    assert!(out.starts_with("readable n=1 [ident=0x1 filter=-1 flags=0x1 fflags=0 data=0x5 udata=0xabc]\nread=5\n")
            && out.ends_with("eof n=1 [ident=0x1 filter=-1 flags=0x8001 fflags=0 data=0 udata=0xabc]\n"), "{out:?}");
    let wait = main_wait(&kevents(&trace));
    let write = events(&trace).into_iter().find_map(|(i, e)| match e {
        Event::Syscall { num, ret: 5, thread, .. } if num == retrace_arch::SYS_WRITE && thread != 0 => Some(i),
        _ => None,
    }).expect("the writer thread's 5-byte write");
    assert!(wait.0 < write && next_on(&trace, 0, wait.0) > write, "main waits, the write lands, main runs");
    let regs = retrace_core::seek(&trace, write + 1, 0).unwrap().dbg_regs_of(0).unwrap();
    assert!(regs.contains("x0 =0x0000000000000001"), "the write's own landmark woke main with one event:\n{regs}");
}

/// F8: `EV_ONESHOT` drops the knote at its delivery.
#[test]
fn a_oneshot_knote_is_delivered_once_then_nothing() {
    let (rec, trace) = records_as_native(&["oneshot"]);
    assert_eq!(String::from_utf8_lossy(&rec.stdout),
        "oneshot n=1 [ident=0x7 filter=-10 flags=0x11 fflags=0 data=0 udata=0]\nagain n=0\n");
    assert_eq!(kevents(&trace).iter().map(|k| k.3).collect::<Vec<_>>(), vec![1, 0]);
}

/// Record a `bad` mode: the recorder must stop at the refusal, naming it, with the guest's kqueue()
/// landmark readable before it (so the empty-kevent check below is not vacuous).
fn refused(what: &str, why: &str) {
    let (rec, trace) = util::record_dynamic_args(retrace_guest::KQ_DYN, &["bad", what]);
    assert_eq!(rec.code, 101, "{what}: the recorder must stop at the refusal (a panic). stderr:\n{}", rec.stderr);
    assert!(rec.stderr.contains(why), "{what}: the refusal must say {why:?}. stderr:\n{}", rec.stderr);
    let evs = events(&trace);
    assert!(evs.iter().any(|(_, e)| matches!(e, Event::Syscall { num: 362, .. })),
        "{what}: the kqueue() landmark before the refusal must be readable ({} events)", evs.len());
    assert!(kevents(&trace).is_empty(), "{what}: a refused call appends no landmark");
    assert!(!String::from_utf8_lossy(&rec.stdout).contains("bad done"), "{what}: the guest must not run past it");
}

/// R5: a filter the model does not have is refused by value, naming the change and the field.
/// Natively the call returns 0 (`filter n=0`).
#[test]
fn an_unmodelled_filter_stops_the_recorder_naming_it() {
    refused("filter", "M48: kevent change 0: (ident 0x1, filter -7, flags 0x11, fflags 0x0, data 0x3e8): filter -7 is not modelled");
}

/// Restore parity (§3c "through every path"): a checkpoint taken while main is blocked in kevent
/// carries the kqueue and its waiter, so the trigger replayed from it wakes the same thread with the
/// same event as a cold seek does.
#[test]
fn a_seek_into_a_blocked_kevent_matches_a_cold_seek() {
    let (_, trace) = records_as_native(&["wake"]);
    let ks = kevents(&trace);
    let (wait, trig) = (main_wait(&ks), *ks.iter().find(|k| k.1 != 0).unwrap());
    let state = retrace_core::seek(&trace, wait.0 + 1, 0).unwrap().dbg_internal_state();
    assert!(state.contains("waiter: Some(Waiter { tid: 0,"), "the checkpoint is inside main's blocked kevent:\n{state}");
    let cold = warm_matches_cold(&trace, wait.0 + 1, trig.0 + 2);
    assert!(cold.dbg_regs_of(0).unwrap().contains("x0 =0x0000000000000001"));
}

/// The same across a deadline: the checkpoint holds main blocked with its 5 ms deadline, and the
/// idle jump replayed from it lands where a cold seek's does.
#[test]
fn a_seek_inside_a_timed_kevent_matches_a_cold_seek() {
    let (_, trace) = records_as_native(&["timeout"]);
    let wait = main_wait(&kevents(&trace));
    let from = retrace_core::seek(&trace, wait.0 + 1, 0).unwrap();
    assert!(from.dbg_internal_state().contains("waiter: Some(Waiter { tid: 0,"), "inside main's timed kevent");
    let (n, after, _) = the_idle_jump(from);
    let cold = warm_matches_cold(&trace, wait.0 + 1, n + 1);
    assert_eq!((cold.current_thread(), synthetic_tsc(&cold.dbg_internal_state())), (0, after));
}

/// Review Focus 5. A shape the recording accepted but replay refuses is reached only after an
/// earlier silent divergence, so it must be a `Divergence` naming the call and the field, never a
/// panic. The tamper is in guest memory at the `svc` (main's registration's filter, EVFILT_USER to
/// EVFILT_TIMER), because a rewritten trace field is compared before the model runs.
#[test]
fn a_kevent_refused_on_replay_is_a_divergence_naming_it_not_a_panic() {
    let (_, trace) = records_as_native(&["wake"]);
    let (i, _, args, _) = *kevents(&trace).iter().find(|k| k.1 == 0 && k.2[2] == 1 && k.2[4] == 0)
        .expect("main's registration: one change, no event list");
    let mut s = session_at_svc(&trace, i);
    assert_eq!(s.read_mem_prefix(args[1] + 8, 2), retrace_arch::EVFILT_USER.to_le_bytes(), "the change is EVFILT_USER");
    s.dbg_write_mem(args[1] + 8, &retrace_arch::EVFILT_TIMER.to_le_bytes()).unwrap();
    let d = match s.advance() { Err(d) => d, Ok(_) => panic!("the tampered kevent at landmark {i} replayed") };
    assert_eq!(d.landmark, i);
    assert!(d.detail.starts_with("kevent refused on replay, though the recording accepted it")
            && d.detail.contains("M48: kevent change 0: (ident 0x7, filter -7")
            && d.detail.contains("filter -7 is not modelled"), "{}", d.detail);
}

/// The only test that sees the mirror's compare on an honest model: rewrite the detect landmark's
/// return, then its `ret1`, and replay must name each at that landmark (`kqinit_e2e`'s pattern).
#[test]
fn a_rewritten_kevent_return_is_a_divergence_naming_the_rc() {
    let (_, trace) = records_as_native(&["probe"]);
    let i = kevents(&trace)[0].0;
    for (ext, bad_ret, bad_ret1, why) in [
        ("rc2.bin", 2, 0, "kevent rc mismatch: replay 0x1 (err=false) != recorded 0x2 (err=false)"),
        ("ret1.bin", 1, 1, "kevent recorded ret1=0x1 with 0 writes"),
    ] {
        let mut ev = retrace_trace::Reader::open(&trace).unwrap();
        if let Event::Syscall { ret, ret1, .. } = &mut ev[i] { (*ret, *ret1) = (bad_ret, bad_ret1); }
        let bad = trace.with_extension(ext);
        let mut w = retrace_trace::Writer::create(&bad).unwrap();
        for e in &ev { w.append(e).unwrap(); }
        drop(w);
        let rp = util::replay(&bad);
        assert_eq!(rp.code, 3, "{ext}: replay of the rewritten trace must diverge (exit 3): {}", rp.stderr);
        assert!(rp.stderr.contains(&format!("DIVERGENCE at landmark {i} ")) && rp.stderr.contains(why),
            "{ext}: the divergence must be the mirror's {why:?}, at landmark {i}: {}", rp.stderr);
    }
}

/// R5: kevent on a descriptor that is not a guest kqueue is refused by value. Natively it is EBADF
/// (`notkq n=-1`).
#[test]
fn a_kevent_on_a_descriptor_that_is_not_a_kqueue_is_refused_by_value() {
    refused("notkq", "M48: kevent on fd 1, which is not a guest kqueue: the kernel answers EBADF, which is not modelled");
}
