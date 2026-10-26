// M53 t3 gate (spec §3d; t0 §M4). A guest kqueue is shared by every descriptor on it, as tokio's mio
// duplicates its kqueue with F_DUPFD_CLOEXEC. Each mode of `crates/retrace-guest/c/kqdup_dyn.c` is
// run natively, recorded and replayed twice, and the guest's own line is the assertion: before M53 a
// dup of a guest kqueue was a wall, so the recording stopped at the dup and printed nothing.
mod util;

use retrace_core::ReplaySession;
use retrace_trace::Event;
use std::path::{Path, PathBuf};

fn native(mode: &str) -> String {
    let o = std::process::Command::new(retrace_guest::KQDUP_DYN).arg(mode).output().unwrap();
    assert!(o.status.success(), "native kqdup_dyn {mode}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Record `mode`, and assert native's exact output, then two byte-identical replays.
fn records_as_native(mode: &str) -> (String, PathBuf) {
    let (rec, trace) = util::record_dynamic_args(retrace_guest::KQDUP_DYN, &[mode]);
    assert_eq!(rec.code, 0, "{mode}: record: {}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout).into_owned();
    assert_eq!(out, native(mode), "{mode}: retrace vs native");
    for k in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "{mode}: replay {k}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "{mode}: replay {k} stdout");
    }
    (out, trace)
}

#[test]
fn a_knote_added_on_one_descriptor_is_delivered_through_its_duplicate() {
    let (out, _) = records_as_native("shared");
    assert_eq!(out, "delivered=1\n");
}

#[test]
fn a_kqueue_outlives_the_close_of_one_of_its_descriptors() {
    let (out, _) = records_as_native("outlives");
    assert_eq!(out, "still=1\n", "d still holds the knote");
}

#[test]
fn closing_a_watched_descriptor_removes_its_knote_from_a_shared_kqueue() {
    let (out, _) = records_as_native("fdclose");
    assert_eq!(out, "knotes_before=1 knotes_after=0\n");
}

#[test]
fn mios_waker_on_a_duplicated_kqueue_wakes_a_timed_wait() {
    let (out, _) = records_as_native("mio");
    assert_eq!(out, "woken=1 by=user\n");
}

/// Every `kevent` landmark: (index, thread, args, ret).
fn kevents(trace: &Path) -> Vec<(usize, u32, [u64; 8], u64)> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().enumerate().filter_map(|(i, e)| match e {
        Event::Syscall { num, args, ret, thread, .. } if num == retrace_arch::SYS_KEVENT => Some((i, thread, args, ret)),
        _ => None,
    }).collect()
}

/// A checkpoint inside main's blocked wait on the original descriptor, continued past the worker's
/// trigger (made through the copy), must equal a cold seek there, in every register, the saved
/// contexts, box state (the shared kqueue and its waiter included) and memory.
#[test]
fn a_seek_into_the_blocked_wait_on_the_duplicate_matches_a_cold_seek() {
    let (_, trace) = records_as_native("mio");
    let ks = kevents(&trace);
    let wait = *ks.iter().find(|k| k.1 == 0 && k.2[2] == 0 && k.2[4] == 8).expect("main's timed wait");
    let trig = *ks.iter().find(|k| k.1 != 0 && k.2[2] == 1).expect("the worker's wake through the copy");
    // One VM per process: each session ends before the next is built.
    let cp = {
        let from = retrace_core::seek(&trace, wait.0 + 1, 0).unwrap();
        let state = from.dbg_internal_state();
        assert!(state.contains("waiter: Some(Waiter { tid: 0,"), "the checkpoint is inside main's blocked kevent:\n{state}");
        from.checkpoint()
    };
    let to = trig.0 + 2;
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
    assert!(cold.dbg_regs_of(0).unwrap().contains("x0 =0x0000000000000001"), "main's wait returned the one event");
}
