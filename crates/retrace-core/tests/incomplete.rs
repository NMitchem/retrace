//! M50 §3c, C2: replay's end-of-trace rule, in process. A recording with no terminal event, the
//! shape a wall or a killed recorder leaves, replays to its end and ends there as
//! `Outcome::Incomplete`. A difference anywhere before its end stays a divergence.
use retrace_core::{record, replay, Outcome};
use retrace_trace::{Event, Reader, Writer};
use std::path::{Path, PathBuf};

/// The static `HELLO` guest recorded whole, and its events.
fn recorded_hello(name: &str) -> (PathBuf, Vec<Event>, Vec<u8>) {
    let p = std::env::temp_dir().join(format!("retrace-incomplete-{}-{name}.bin", std::process::id()));
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO).unwrap());
    let s = record(&loaded, &p).unwrap();
    assert!(matches!(s.outcome, Outcome::Exit { .. }), "{:?}", s.outcome);
    let (events, torn) = Reader::open_checked(&p).unwrap();
    assert!(!torn);
    (p, events, s.stdout)
}

fn rewrite(p: &Path, events: &[Event]) {
    let mut w = Writer::create(p).unwrap();
    for e in events { w.append(e).unwrap(); }
}

/// Everything before the terminal event, as a recorder killed there leaves it.
fn before_end(events: &[Event]) -> usize {
    events.iter().position(|e| matches!(e, Event::Exit { .. })).expect("HELLO exits")
}

#[test]
fn a_recording_cut_before_its_end_replays_to_the_cut_as_incomplete() {
    let (p, events, stdout) = recorded_hello("cut");
    let end = before_end(&events);
    rewrite(&p, &events[..end]);
    let r = replay(&p).expect("the end of an incomplete recording is not a divergence");
    assert!(matches!(&r.outcome, Outcome::Incomplete(at) if at.starts_with(&format!("landmark {end} (pc 0x"))),
        "{:?}", r.outcome);
    assert_eq!(r.stdout, stdout, "replay prints what the recording printed before the cut");
    let _ = std::fs::remove_file(&p);
}

#[test]
fn a_difference_before_the_end_of_an_incomplete_recording_is_still_a_divergence() {
    let (p, mut events, _) = recorded_hello("tamper");
    events.truncate(before_end(&events));
    let i = events.iter().position(|e| matches!(e, Event::Syscall { .. })).expect("HELLO writes");
    if let Event::Syscall { args, .. } = &mut events[i] { args[2] ^= 1; }
    rewrite(&p, &events);
    let d = replay(&p).expect_err("a mismatch before the end is a divergence, not an incomplete recording");
    assert_eq!(d.landmark, i);
    assert!(d.detail.contains("mismatch"), "the tampered argument is named as a mismatch: {}", d.detail);
    let _ = std::fs::remove_file(&p);
}
