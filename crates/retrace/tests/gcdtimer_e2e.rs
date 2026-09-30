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
