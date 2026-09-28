// M45 gate (spec §3e, §4). libdispatch's workqueue-kqueue init, `kevent_qos` (374) with
// KEVENT_FLAG_WORKQ, is EMULATED as one measured shape returning 0, and every other shape is
// refused by value. The fixture issues the call by hand (`crates/retrace-guest/c/kqinit_dyn.c`),
// so the mechanism is guarded on any machine, with or without a libdispatch path that reaches it.
// Every assertion is on the trace or on the recorder's own words, never on an exit code alone: a
// fixture that never reached 374 exits 0 too.
mod util;

use retrace_trace::Event;
use std::path::{Path, PathBuf};

/// What the fixture prints after the call: t0 M3 measured it natively, in both call modes.
const MARKER: &[u8] = b"kqinit rc=0 carry=0\n";

/// Every `kevent_qos` event in `trace`, with its landmark index (the replay session's `idx`).
fn kevent_events(trace: &Path) -> Vec<(usize, Event)> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().enumerate()
        .filter(|(_, e)| matches!(e, Event::Syscall { num, .. } if *num == retrace_arch::SYS_KEVENT_QOS))
        .collect()
}

/// Record `mode`; assert the one emulated landmark and the marker; replay twice byte-identically.
fn records_one_emulated_init(mode: &[&str]) -> PathBuf {
    let (rec, trace) = util::record_dynamic_args(retrace_guest::KQINIT_DYN, mode);
    assert_eq!(rec.code, 0, "{mode:?}: record: {}", rec.stderr);
    let ev = kevent_events(&trace);
    assert_eq!(ev.len(), 1, "{mode:?}: exactly one kevent_qos landmark: {ev:?}");
    let Event::Syscall { args, ret, err, writes, .. } = &ev[0].1 else { unreachable!() };
    // The difference M45 makes: a forward would carry the host's writes or return; a missing arm
    // never gets here (the recorder panics at M33's row check, or at the forward arm's assert).
    assert_eq!((*ret, *err, writes.len()), (0, false, 0),
        "{mode:?}: the emulation returns 0, clears carry and writes nothing");
    assert_eq!(args[7], 0x21, "{mode:?}: KEVENT_FLAG_WORKQ | KEVENT_FLAG_IMMEDIATE");
    assert_eq!(rec.stdout, MARKER, "{mode:?}: got {:?}", String::from_utf8_lossy(&rec.stdout));
    for n in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "{mode:?}: replay {n}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "{mode:?}: replay {n} stdout");
    }
    trace
}

#[test]
fn the_measured_init_records_as_one_emulated_landmark_and_replays() {
    records_one_emulated_init(&[]);
}

/// Review Focus 1: the entry's two halves sit on different 16 KiB pages, which need not be
/// adjacent in IPA space, so only a page-by-page read gets the second half right.
#[test]
fn an_entry_straddling_a_page_is_read_whole() {
    records_one_emulated_init(&["straddle"]);
}

/// R5: a shape the box has not measured stops the recorder, naming what differs, rather than
/// returning an errno libdispatch would crash on with the cause hidden.
#[test]
fn an_unmeasured_shape_stops_the_recorder_naming_what_differs() {
    for (mode, why) in [
        ("flags", "changelist[0].flags is 0x25, measured 0x21"),
        ("badptr", "the change list's entry read 0 of 72 bytes: it does not fully translate"),
    ] {
        let (rec, trace) = util::record_dynamic_args(retrace_guest::KQINIT_DYN, &[mode]);
        assert_eq!(rec.code, 101, "{mode}: the recorder must stop at the refusal (a panic). stderr:\n{}", rec.stderr);
        assert!(rec.stderr.contains(&format!("M45: unmeasured kevent_qos shape: {why}")),
            "{mode}: the refusal must name {why:?}. stderr:\n{}", rec.stderr);
        assert!(kevent_events(&trace).is_empty(), "{mode}: a refused call appends no landmark");
        assert!(rec.stdout.is_empty(), "{mode}: the guest must not run past the refused call");
    }
}

/// The only test that can see the replay mirror. While the return is a constant, the mirror's
/// compare is vacuous on an honest trace (the M18 t5 mirror's comment says the same of its own).
/// So rewrite the recorded return, and replay must name the mismatch at that landmark; without the
/// mirror, generic replay would feed the 1 to the guest in silence.
#[test]
fn replay_recomputes_the_emulated_return() {
    let trace = records_one_emulated_init(&[]);
    let i = kevent_events(&trace)[0].0;
    let mut ev = retrace_trace::Reader::open(&trace).unwrap();
    if let Event::Syscall { ret, .. } = &mut ev[i] { *ret = 1; }
    let bad = trace.with_extension("rc1.bin");
    let mut w = retrace_trace::Writer::create(&bad).unwrap();
    for e in &ev { w.append(e).unwrap(); }
    drop(w);
    let rp = util::replay(&bad);
    assert_eq!(rp.code, 3, "replay of the rewritten trace must diverge (exit 3): {}", rp.stderr);
    assert!(rp.stderr.contains(&format!("DIVERGENCE at landmark {i} "))
        && rp.stderr.contains("kevent_qos rc mismatch: replay 0x0 != recorded 0x1"),
        "the divergence must be the mirror's, at the 374 landmark {i}: {}", rp.stderr);
}

/// Review Focus 5: a debugger position enters replay mid-trace. Seeking onto the emulated
/// landmark, and to either side of it, must replay to the recorded end.
#[test]
fn seeks_either_side_of_the_landmark_replay_to_the_end() {
    let trace = records_one_emulated_init(&[]);
    let i = kevent_events(&trace)[0].0;
    for n in [i - 1, i, i + 1] {
        let mut s = retrace_core::seek(trace.as_path(), n, 0).unwrap_or_else(|e| panic!("seek ({n}, 0): {e}"));
        loop {
            match s.advance() {
                Ok(retrace_core::Advance::Exited(r)) => {
                    assert_eq!(r.outcome, retrace_core::Outcome::Exit { code: 0 }, "({n}, 0)");
                    assert_eq!(r.stdout, MARKER, "({n}, 0): stdout");
                    break;
                }
                Ok(_) => {}
                Err(d) => panic!("({n}, 0): diverged at landmark {}: {}", d.landmark, d.detail),
            }
        }
    }
}
