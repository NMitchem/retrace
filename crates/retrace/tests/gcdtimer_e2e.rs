// M46 gate (spec §3f, §4; plan R7). libdispatch's timers, end to end, on the synthetic clock.
// Every assertion is on the trace, on the guest's own output or on the recorder's own words, never
// on an exit code alone: a guest whose timer path read the host's clock still exits 0 once real
// time catches up with it.
mod util;

use retrace_trace::Event;
use std::path::{Path, PathBuf};

/// Record `argv` of `guest`, assert exit 0, and replay twice byte-identically.
fn records_and_replays(guest: &str, argv: &[&str]) -> (util::RunOut, PathBuf) {
    let (rec, trace) = util::record_dynamic_args(guest, argv);
    assert_eq!(rec.code, 0, "{argv:?}: record: {}", rec.stderr);
    for n in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "{argv:?}: replay {n}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "{argv:?}: replay {n} stdout");
    }
    (rec, trace)
}

/// Every event of `trace` with its landmark index (the replay session's `idx`).
fn events(trace: &Path) -> Vec<(usize, Event)> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().enumerate().collect()
}

/// R7: the guest's `mach_absolute_time` has one source. `mach_get_times` falls back to
/// `gettimeofday` (116) whenever the commpage stamp is a second or more from `mach_absolute_time`
/// (xnu `libsyscall/wrappers/mach_get_times.c`). retrace's commpage is frozen while its timebase is
/// synthetic, so it always falls back, and before R7 it handed the guest the HOST's mach time,
/// which is libdispatch's timer "now" (t0 M1).
#[test]
fn the_guest_clock_has_one_source() {
    let (rec, trace) = records_and_replays(retrace_guest::AFTER_DYN, &["clock"]);
    assert_eq!(rec.stdout, b"clock ok\n",
        "mach_get_times must agree with mach_absolute_time: got {:?}", String::from_utf8_lossy(&rec.stdout));
    let fallbacks = events(&trace).into_iter()
        .filter(|(_, e)| matches!(e, Event::Syscall { num, args, err: false, .. }
            if *num == retrace_arch::SYS_GETTIMEOFDAY && args[2] != 0))
        .count();
    assert!(fallbacks >= 1,
        "the fixture must reach gettimeofday's mach-time fallback, or this test proves nothing about R7");
}

/// Every `workq_kernreturn(THREAD_KEVENT_RETURN)` landmark, as (index, thread, ret).
fn kevent_returns(trace: &Path) -> Vec<(usize, u32, u64)> {
    events(trace).into_iter().filter_map(|(i, e)| match e {
        Event::Syscall { num, args, ret, thread, .. }
            if num == retrace_arch::SYS_WORKQ_KERNRETURN && args[0] == retrace_arch::WQOPS_THREAD_KEVENT_RETURN =>
            Some((i, thread, ret)),
        _ => None,
    }).collect()
}

/// The port name in every recorded `task_get_special_port` reply (msgh_id 3509), in trace order.
/// The reply is the one write its mach_msg2 landmark carries: a 24-byte header with `msgh_id` at
/// offset 20, the descriptor count at 24, then the port descriptor's name at 28
/// (`machmsg::encode_get_special_port_reply`).
fn special_port_replies(trace: &Path) -> Vec<u32> {
    let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    events(trace).into_iter().filter_map(|(_, e)| match e {
        Event::Syscall { writes, .. } if writes.len() == 1 && writes[0].bytes.len() >= 32
            && word(&writes[0].bytes, 20) == 3509 => Some(word(&writes[0].bytes, 28)),
        _ => None,
    }).collect()
}

/// Spec §3f test 1. A `dispatch_after` records to exit 0 with its markers, and two replays are
/// byte-identical. Before M46 the recorder stopped at libdispatch's second `kevent_qos`, so the
/// markers are the difference. The manager's KEVENT_RETURN landmarks, on a nonzero thread, are the
/// model's footprint in the trace.
#[test]
fn a_dispatch_after_fires_on_the_synthetic_clock_and_replays() {
    let (rec, trace) = records_and_replays(retrace_guest::AFTER_DYN, &[]);
    let out = String::from_utf8_lossy(&rec.stdout);
    assert!(out.starts_with("fired cell 0x") && out.ends_with("\nfired\ndone\n"), "stdout: {out:?}");
    // t0 Ruling T0-a: libxpc's bootstrap port is minted, so its name is nonzero. libdispatch's
    // debug control port is MACH_PORT_NULL, so its debug channel registers no EVFILT_MACHPORT knote.
    let ports = special_port_replies(&trace);
    assert!(ports.first().is_some_and(|&p| p != 0) && ports.iter().filter(|&&p| p == 0).count() == 1,
        "a minted bootstrap port first, and exactly one null debug control port (T0-a): {ports:#x?}");
    let rets = kevent_returns(&trace);
    let manager = rets.first().map(|r| r.1)
        .expect("the manager must hand its changes back through KEVENT_RETURN");
    assert_ne!(manager, 0, "the manager is a workqueue thread the box started, never main");
    assert!(rets.iter().all(|r| r.1 == manager), "one manager thread serves the whole run: {rets:?}");
    // One fire needs a handful of returns (t0 M3 counts the native ones). A manager whose "now"
    // lags its deadline re-arms after every fire and spins through returns until the clocks meet:
    // the failure R7 exists to prevent, and one an exit code cannot show.
    assert!(rets.len() <= 8,
        "{} KEVENT_RETURNs for one dispatch_after: the manager re-arms a timer it sees as not yet due", rets.len());
}

/// Spec §3f test 2. One kernel timer serves the bucket, so B's arm follows A's fire through a
/// KEVENT_RETURN that reprograms it.
#[test]
fn two_timers_on_one_bucket_fire_in_deadline_order() {
    let (rec, trace) = records_and_replays(retrace_guest::AFTER_DYN, &["two"]);
    assert_eq!(rec.stdout, b"A\nB\ndone\n", "got {:?}", String::from_utf8_lossy(&rec.stdout));
    let rets = kevent_returns(&trace);
    // Measured 3 (M46 t5, `task-5-report.md`; native 3, t0 M3). The bound 12 is headroom, not a
    // measurement.
    assert!(rets.len() >= 2 && rets.len() <= 12, "two fires: {} KEVENT_RETURNs", rets.len());
}

/// Spec §3f test 3: re-arm after every fire, and manager reuse across fires. The `KEVENT_RETURN`
/// disarm path (`ChangeEntry::TimerDelete`) is not guarded end to end by this fixture, because the
/// cancel's disarm does not reach the box before exit (M46 t5); the box-level refusals in `kq.rs`
/// are what cover it.
#[test]
fn a_repeating_timer_ticks_three_times_and_replays() {
    let (rec, trace) = records_and_replays(retrace_guest::TIMER_DYN, &[]);
    assert_eq!(rec.stdout, b"tick 1\ntick 2\ntick 3\ndone\n", "got {:?}", String::from_utf8_lossy(&rec.stdout));
    let rets = kevent_returns(&trace);
    let manager = rets.first().map(|r| r.1).expect("KEVENT_RETURNs");
    assert!(manager != 0 && rets.iter().all(|r| r.1 == manager), "one manager, reused across fires: {rets:?}");
    assert!(rets.len() >= 3 && rets.len() <= 24, "three fires: {} KEVENT_RETURNs", rets.len());
}

/// Spec §3f test 4, re-planned by t0 Ruling T0-b. A WALL timer is refused by value before it
/// arms.
///
/// libdispatch registers for calendar-change notifications the first time it arms a WALL timer,
/// with an `EVFILT_MACHPORT` `kevent_qos` from the manager (t0 M3, `wall` call 7), before the
/// KEVENT_RETURN that carries the timer's fflags `0x9c`. The recorder refuses that registration
/// by its filter. The `0x9c` refusal itself is pinned at box level (`kqmanager.rs`) and in
/// `gcdshapes.rs`.
///
/// An exit code alone would not do: a recorder that accepted the timer and jumped the clock to a
/// wall-clock deadline could exit either way.
#[test]
fn a_wall_clock_timer_is_refused_before_it_arms() {
    let (rec, trace) = util::record_dynamic_args(retrace_guest::AFTER_DYN, &["wall"]);
    assert_eq!(rec.code, 101, "the recorder must stop at the refusal. stderr:\n{}", rec.stderr);
    assert!(rec.stderr.contains("M46: unmeasured kevent_qos shape: changelist[0].filter is 0xfff8"),
        "the refusal must name the calendar-change registration's filter, -8 EVFILT_MACHPORT (t0 M3). \
         stderr:\n{}", rec.stderr);
    // Ruling T5-b: the `armed` count below is vacuous on an empty or truncated trace, so first
    // prove the landmarks recorded before the refusal survive it. Main's `0x23` registrations
    // (the memory-pressure source and the poke) both precede the manager's refused one.
    let evs = events(&trace);
    assert!(evs.iter().any(|(_, e)| matches!(e, Event::Syscall { num, args, .. }
        if *num == retrace_arch::SYS_KEVENT_QOS && args[7] & 0xffff_ffff == 0x23)),
        "the refused record must still leave its earlier landmarks readable: no 0x23 kevent_qos \
         among the trace's {} events", evs.len());
    // The refused call appends no landmark, so a KEVENT_RETURN carrying a change in the trace
    // would mean the manager armed something before the refusal. The WALL timer must never arm.
    let armed = evs.into_iter().filter(|(_, e)| matches!(e, Event::Syscall { num, args, .. }
        if *num == retrace_arch::SYS_WORKQ_KERNRETURN
        && args[0] == retrace_arch::WQOPS_THREAD_KEVENT_RETURN && args[2] & 0xffff_ffff != 0)).count();
    assert_eq!(armed, 0, "no KEVENT_RETURN carrying a change may precede the refusal");
    assert!(!rec.stdout.windows(6).any(|w| w == b"fired\n"), "the guest must not run past the refused registration");
}

fn synthetic_tsc(state: &str) -> u64 {
    let v = state.split("synthetic_tsc=0x").nth(1).and_then(|r| r.split_whitespace().next())
        .unwrap_or_else(|| panic!("no synthetic_tsc in {state}"));
    u64::from_str_radix(v, 16).unwrap()
}

/// The landmark whose settle makes `after_dyn`'s idle jump: the one across which `synthetic_tsc`
/// moves by more than one window's timebase reads could move it.
///
/// Found by the clock and not by the armed count. The manager's KEVENT_RETURN arms the timer and
/// that same landmark's settle fires it, because main is already blocked and the manager has just
/// parked (Review Focus 3). No landmark boundary ever sees the timer armed.
fn the_idle_jump(trace: &Path) -> usize {
    // 0x10_0000 ticks is ~44 ms at 24 MHz. A timebase read moves the counter 0x2400 (384 µs), so
    // this is ~113 reads inside one window. The jump itself is the 100 ms deadline minus the few
    // reads between `dispatch_time` and the park.
    const JUMP: u64 = 0x10_0000;
    let mut s = retrace_core::ReplaySession::open(trace).unwrap();
    loop {
        let (n, before) = (s.landmark(), synthetic_tsc(&s.dbg_internal_state()));
        match s.advance() {
            Ok(retrace_core::Advance::Exited(_)) => panic!("the run exited and no landmark jumped the clock"),
            Ok(_) => {}
            Err(d) => panic!("diverged at landmark {}: {}", d.landmark, d.detail),
        }
        if synthetic_tsc(&s.dbg_internal_state()) - before > JUMP {
            return n;
        }
    }
}

/// Spec §3f test 5, the seek half, which proves §3c's field is rebuilt on the non-linear path. A
/// checkpoint taken before the idle jump, continued across it, must equal a cold seek past it.
#[test]
fn a_seek_across_the_idle_jump_matches_a_cold_seek() {
    let (_, trace) = records_and_replays(retrace_guest::AFTER_DYN, &[]);
    let manager = kevent_returns(&trace)[0].1;
    let n = the_idle_jump(&trace);
    let cp = retrace_core::seek(&trace, n, 0).unwrap().checkpoint();
    let warm = {
        let mut s = retrace_core::ReplaySession::from_checkpoint(&trace, &cp).unwrap();
        s.advance_to_landmark(n + 1).unwrap_or_else(|d| panic!("warm: diverged at {}: {}", d.landmark, d.detail));
        (s.current_thread(), s.dbg_regs(), s.dbg_fp_regs(), s.dbg_internal_state(), s.snapshot().1)
    };
    let cold = retrace_core::seek(&trace, n + 1, 0).unwrap();
    // The jump fired the timer and re-entered the manager: the upcall is loaded, and the timer is
    // delivered, not still armed.
    assert_eq!(cold.current_thread(), manager, "the fire re-entered the manager, the only runnable thread");
    assert!(cold.dbg_regs().contains("x4 =0x00000000001e4008"),
        "the manager's upcall carries the reuse flags (t0 M2):\n{}", cold.dbg_regs());
    assert_eq!(cold.dbg_armed_timers(), 0, "the jump fired the timer it jumped to");
    assert_eq!(warm.0, cold.current_thread(), "thread: checkpointed vs cold");
    assert_eq!(warm.1, cold.dbg_regs(), "registers: checkpointed vs cold");
    assert_eq!(warm.2, cold.dbg_fp_regs(), "FP/SIMD: checkpointed vs cold");
    assert_eq!(warm.3, cold.dbg_internal_state(), "the knote table and the synthetic clock: checkpointed vs cold");
    assert!(cold.diff_memory(&warm.4).is_none(), "memory: checkpointed vs cold");
}

fn parse_cell(stdout: &str) -> u64 {
    let rest = &stdout[stdout.find("fired cell 0x").unwrap_or_else(|| panic!("no cell line in {stdout:?}")) + 13..];
    u64::from_str_radix(&rest[..rest.find('\n').unwrap_or(rest.len())], 16).unwrap()
}

/// Spec §3f test 5, the reverse half: with the whole run replayed, a watch on the handler's cell
/// and a `reverse-continue` must reach the handler's store, named on the worker that ran it.
#[test]
fn reverse_continue_reaches_the_handlers_store_and_names_its_worker() {
    let (rec, trace) = records_and_replays(retrace_guest::AFTER_DYN, &[]);
    let cell = parse_cell(&String::from_utf8_lossy(&rec.stdout));
    assert!(cell.is_multiple_of(8), "the cell must be 8-aligned to watch");
    let manager = kevent_returns(&trace)[0].1;
    // The handler's `write(1, "fired\n", 6)`; stdio's cell line goes out through write_nocancel.
    let writer = events(&trace).into_iter().find_map(|(_, e)| match e {
        Event::Syscall { num, args, thread, .. } if num == retrace_arch::SYS_WRITE && args[0] == 1 && args[2] == 6 => Some(thread),
        _ => None,
    }).expect("the handler's write of \"fired\\n\"");
    assert!(writer != 0 && writer != manager, "the handler runs on a worker: writer {writer}, manager {manager}");
    let (code, out, err) = util::debug_bounded(trace.to_str().unwrap(),
        &format!("continue; watch 0x{cell:x}; reverse-continue; where"), 300);
    assert_eq!(code, Some(0), "debug exited {code:?} (None: killed at the bound). stderr: {err}\nstdout: {out}");
    assert!(out.contains(&format!("hit watch 0x{cell:x}")), "reverse-continue must find the handler's store:\n{out}");
    let where_line = util::strip_annot(out.lines().last().expect("a `where` line"));
    assert!(where_line.ends_with(&format!("thread={writer}")),
        "`where` must name the worker that ran the handler, thread {writer}. got:\n{where_line}\n{out}");
}

/// A session at landmark `i`'s `svc`: the instruction that issues it, not yet retired. It steps one
/// instruction at a time, because the entry the call reads is written inside window `i`, so a
/// tamper at `(i, 0)` would be overwritten before the call reads it.
fn session_at_svc(trace: &Path, i: usize) -> retrace_core::ReplaySession {
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

/// Spec §3f test 6 (M45 F-2). A shape the recording accepted but replay refuses is reachable only
/// after an earlier silent divergence, so replay must report it as a `Divergence` that names the
/// field, not panic. The tamper is in guest memory at the `svc`, because a rewritten trace field
/// is compared before the validator runs.
#[test]
fn a_shape_refused_on_replay_is_a_divergence_naming_it_not_a_panic() {
    let (_, trace) = records_and_replays(retrace_guest::AFTER_DYN, &[]);
    let evs = events(&trace);
    // Case 1: the manager poke, which is main's `0x23` registration whose entry is EVFILT_USER.
    let mut poked = false;
    for (i, list) in evs.iter().filter_map(|(i, e)| match e {
        Event::Syscall { num, args, .. } if *num == retrace_arch::SYS_KEVENT_QOS && args[7] & 0xffff_ffff == 0x23 => Some((*i, args[1])),
        _ => None,
    }) {
        let mut s = session_at_svc(&trace, i);
        if s.read_mem_prefix(list + 8, 2) != retrace_arch::EVFILT_USER.to_le_bytes() { continue; }
        s.dbg_write_mem(list + 24, &0u32.to_le_bytes()).unwrap(); // fflags: NOTE_TRIGGER -> 0
        let d = match s.advance() { Err(d) => d, Ok(_) => panic!("the tampered poke at landmark {i} replayed") };
        assert_eq!(d.landmark, i);
        assert!(d.detail.contains("replay diverged before this landmark")
                && d.detail.contains("changelist[0].fflags is 0x0, measured 0x1000000"), "{}", d.detail);
        poked = true;
        break;
    }
    assert!(poked, "no manager poke among the 0x23 registrations");
    // Case 2: the manager's first KEVENT_RETURN that carries a change, which is its timer arm,
    // turned WALL.
    let (j, list) = evs.iter().find_map(|(i, e)| match e {
        Event::Syscall { num, args, .. } if *num == retrace_arch::SYS_WORKQ_KERNRETURN
            && args[0] == retrace_arch::WQOPS_THREAD_KEVENT_RETURN && args[2] & 0xffff_ffff >= 1 => Some((*i, args[1])),
        _ => None,
    }).expect("a KEVENT_RETURN carrying a change");
    let mut s = session_at_svc(&trace, j);
    assert_eq!(s.read_mem_prefix(list + 8, 2), retrace_arch::EVFILT_TIMER.to_le_bytes(), "the first change is a timer");
    s.dbg_write_mem(list + 24, &0x9cu32.to_le_bytes()).unwrap();
    let d = match s.advance() { Err(d) => d, Ok(_) => panic!("the tampered KEVENT_RETURN at landmark {j} replayed") };
    assert_eq!(d.landmark, j);
    assert!(d.detail.starts_with("workq_kernreturn refused on replay")
            && d.detail.contains("changelist[0].fflags is 0x9c"), "{}", d.detail);
}
