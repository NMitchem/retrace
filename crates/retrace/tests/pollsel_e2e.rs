// M53 t4 gate (spec §3e; t0 §M7). `poll`, `select` and `pselect` are emulated in the box over guest
// pipes, never forwarded: each mode of `crates/retrace-guest/c/pollsel_dyn.c` is run natively,
// recorded and replayed twice, and the guest's own lines are the assertion. Before M53 these calls
// were unrowed numbers and walled at the first one. Every run is bounded: a wake that never comes
// leaves the guest blocked forever, which must fail its test rather than stall the gate.
mod util;

use retrace_core::ReplaySession;
use retrace_trace::Event;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const BOUND: u64 = 120;

fn native(mode: &str) -> String {
    let o = std::process::Command::new(retrace_guest::POLLSEL_DYN).arg(mode).output().unwrap();
    assert!(o.status.success(), "native pollsel_dyn {mode}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn trace_path() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("retrace-pollsel-{}-{}.bin", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
}

/// Record `mode` under the bound.
fn record(mode: &str) -> (util::Bounded, PathBuf) {
    let trace = trace_path();
    let rec = util::run_bounded(util::bin(),
        &["record", "-o", trace.to_str().unwrap(), retrace_guest::POLLSEL_DYN, mode], &[], BOUND);
    (rec, trace)
}

/// Record `mode`, assert native's exact output, then two byte-identical replays.
fn records_as_native(mode: &str) -> (String, PathBuf) {
    let (rec, trace) = record(mode);
    assert_eq!(rec.code, Some(0), "{mode}: record (None is the bound): {}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout).into_owned();
    assert_eq!(out, native(mode), "{mode}: retrace vs native");
    for k in 1..=2 {
        let rp = util::run_bounded(util::bin(), &["replay", trace.to_str().unwrap()], &[], BOUND);
        assert_eq!(rp.code, Some(0), "{mode}: replay {k}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "{mode}: replay {k} stdout");
    }
    (out, trace)
}

#[test]
fn each_pipe_state_polls_as_natively() {
    let (out, _) = records_as_native("states");
    // The spot rows t0 M7 measured, by value, so a model that agreed with native by accident of a
    // shared mistake in the fixture could not pass.
    for want in [
        "poll read end, empty rc=0 errno=0 revents=0x0\n",
        "poll read end, 3 bytes, IN rc=1 errno=0 revents=0x1\n",
        "poll read end, 3 bytes, IN|OUT rc=1 errno=0 revents=0x1\n",
        "poll read end, 3 bytes, events 0 rc=0 errno=0 revents=0x0\n",
        "poll read end, 3 bytes, every bit rc=1 errno=0 revents=0x43\n",
        "poll read end, pending, writer closed rc=1 errno=0 revents=0x11\n",
        "poll read end, drained, writer closed rc=1 errno=0 revents=0x11\n",
        "poll write end, room rc=1 errno=0 revents=0x4\n",
        "poll write end, full rc=0 errno=0 revents=0x0\n",
        "poll write end, 1 byte free rc=0 errno=0 revents=0x0\n",
        "poll write end, 512 bytes free rc=1 errno=0 revents=0x4\n",
        "poll write end, reader closed, OUT rc=1 errno=0 revents=0x10\n",
        "poll write end, reader closed, IN rc=1 errno=0 revents=0x11\n",
        "poll closed fd rc=1 errno=0 revents=0x20\n",
        "poll fd -5 rc=0 errno=0 revents=0x0\n",
        "poll mixed rc=2 errno=0 revents=0x0,0x1,0x20\n",
        "poll matrix write end room WRBAND rc=1 errno=0 revents=0x100\n",
        "poll matrix read end empty, writer closed RDBAND rc=1 errno=0 revents=0x90\n",
    ] {
        assert!(out.contains(want), "missing {want:?} in:\n{out}");
    }
    assert_eq!(out.lines().filter(|l| l.starts_with("poll matrix ")).count(), 36);
}

#[test]
fn a_closed_descriptor_answers_pollnval_and_a_negative_one_is_skipped() {
    let (out, _) = records_as_native("nval");
    assert_eq!(out, "poll closed and negative rc=1 errno=0 revents=0x0,0x20\n");
}

#[test]
fn a_poll_with_no_descriptors_sleeps_until_its_timeout() {
    let (out, _) = records_as_native("sleep");
    assert_eq!(out, "rc=0 waited_at_least=yes\n");
}

#[test]
fn a_poll_with_no_timeout_wakes_when_another_thread_writes() {
    let (out, _) = records_as_native("cross");
    assert_eq!(out, "rc=1 revents=0x1\nclose: rc=1 revents=0x11\n", "the writer's close wakes the poll with POLLHUP");
}

#[test]
fn a_poll_that_times_out_returns_0() {
    let (out, _) = records_as_native("timeout");
    assert_eq!(out, "rc=0 revents=0x0\n", "the revents the guest poisoned are cleared at the timeout");
}

#[test]
fn select_answers_the_same_states_as_natively() {
    let (out, _) = records_as_native("select");
    for want in [
        "select closed fd in the read set rc=-1 errno=9 r=",
        "select nfds -1 rc=-1 errno=22",
        "select nfds 1025 rc=-1 errno=22",
        "select timeout -1 s rc=-1 errno=22",
        "select seconds 100000001, ready rc=-1 errno=22 r=1",
        "select usec 1000000, ready rc=-1 errno=22 r=1",
        "timeval after: 0.080000\n",
    ] {
        assert!(out.contains(want), "missing {want:?} in:\n{out}");
    }
}

#[test]
fn pselect_with_no_mask_waits_like_select() {
    let (out, _) = records_as_native("pselect");
    assert_eq!(out, "rc=1 r=1\nclose: rc=1 r=1\nbad timespec: rc=-1 errno=22\n");
}

/// The wall line of `mode`: the recorder stops at it with exit 4, the guest prints nothing, and no
/// landmark is appended for the refused call.
fn refused(mode: &str) -> String {
    let (rec, _) = record(mode);
    assert_eq!(rec.code, Some(4), "{mode}: the recorder must stop at the wall. stderr:\n{}", rec.stderr);
    assert!(rec.stdout.is_empty(), "{mode}: the guest must not run past the refused call");
    let lines: Vec<&str> = rec.stderr.lines().filter(|l| l.starts_with("retrace: unsupported: ")).collect();
    assert_eq!(lines.len(), 1, "{mode}: exactly one wall line. stderr:\n{}", rec.stderr);
    lines[0].to_string()
}

#[test]
fn a_poll_on_a_file_is_refused_by_value() {
    let line = refused("pollfile");
    // The descriptor's number is the guest's (4 under retrace, 3 natively), so it is not pinned.
    assert!(line.starts_with("retrace: unsupported: poll (syscall 230) on fd "), "{line}");
    assert!(line.contains(", not a guest pipe end - see "), "{line}");
    assert!(line.ends_with("#threads"), "{line}");
}

#[test]
fn a_pselect_with_a_signal_mask_is_refused_by_value() {
    let line = refused("pselmask");
    assert!(line.contains("pselect") && line.contains("a signal mask"), "{line}");
}

#[test]
fn a_close_of_a_polled_descriptor_is_refused_by_value() {
    let (rec, _) = record("pollclose");
    assert_eq!(rec.code, Some(4), "{}", rec.stderr);
    let line = rec.stderr.lines().find(|l| l.starts_with("retrace: unsupported: ")).expect("a wall line");
    assert!(line.contains("close of descriptor ") && line.contains("while thread 0 waits in poll or select on it"), "{line}");
    assert!(line.ends_with("#threads"), "{line}");
}

/// A checkpoint inside main's blocked poll, continued past the writer's `write`, must equal a cold
/// seek there, in every register, the saved contexts, box state (the poll waiter included) and memory.
#[test]
fn a_seek_into_a_blocked_poll_matches_a_cold_seek() {
    let (_, trace) = records_as_native("cross");
    let evs: Vec<(usize, Event)> = retrace_trace::Reader::open(&trace).unwrap().into_iter().enumerate().collect();
    let wait = evs.iter().find_map(|(i, e)| match e {
        Event::Syscall { num: retrace_arch::SYS_POLL | retrace_arch::SYS_POLL_NOCANCEL, args, thread: 0, .. }
            if args[2] as u32 == u32::MAX => Some(*i),
        _ => None,
    }).expect("main's infinite poll");
    let write = evs.iter().find_map(|(i, e)| match e {
        Event::Syscall { num, thread, .. } if *i > wait && *thread != 0
            && (*num == retrace_arch::SYS_WRITE || *num == retrace_arch::SYS_WRITE_NOCANCEL) => Some(*i),
        _ => None,
    }).expect("the writer thread's write");
    // One VM per process: each session ends before the next is built.
    let cp = {
        let from = retrace_core::seek(&trace, wait + 1, 0).unwrap();
        let state = from.dbg_internal_state();
        assert!(state.contains("poll_waiters: {0: Poll {"), "the checkpoint is inside main's blocked poll:\n{state}");
        from.checkpoint()
    };
    let to = write + 2;
    let warm = {
        let mut s = ReplaySession::from_checkpoint(&trace, &cp).unwrap();
        s.advance_to_landmark(to).unwrap_or_else(|d| panic!("warm: diverged at {}: {}", d.landmark, d.detail));
        (s.current_thread(), s.dbg_regs(), s.dbg_regs_of(0), s.dbg_internal_state(), s.snapshot().1)
    };
    let cold = retrace_core::seek(&trace, to, 0).unwrap();
    assert_eq!(warm.0, cold.current_thread(), "thread: checkpointed vs cold");
    assert_eq!(warm.1, cold.dbg_regs(), "registers: checkpointed vs cold");
    assert_eq!(warm.2, cold.dbg_regs_of(0), "main's saved context: checkpointed vs cold");
    assert_eq!(warm.3, cold.dbg_internal_state(), "box state: checkpointed vs cold");
    assert!(cold.diff_memory(&warm.4).is_none(), "memory: checkpointed vs cold");
    assert!(cold.dbg_regs_of(0).unwrap().contains("x0 =0x0000000000000001"), "main's poll returned the one ready descriptor");
}
