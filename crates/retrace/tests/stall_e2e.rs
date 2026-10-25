// M52 §3i gate: the stall line, end to end, on the repo-owned fixture `stall_dyn.c`. Main spins on a
// flag a created thread would set. Natively the program exits at once; under the cooperative
// scheduler the child never runs, so the recording only ends when the bound's alarm does. What the
// recorder says on the way is the assertion: exactly one line, naming the spinner and the ready
// thread, and nothing else of retrace's own. Never an exit code: an alarm and a hang look the same.
mod util;

use std::path::PathBuf;

/// The recording's bound, in seconds. The line is due 20 s of host time after the spin starts, which
/// is about 0.4 s into the recording. The close2 gate measured host-side startup well past that: one
/// recording printed its line at 35.3 s, and one printed nothing inside a 40 s bound. So the bound
/// leaves 60 s for startup. It is a margin only: every assertion is unchanged.
const BOUND: u64 = 80;

fn record_stalled(name: &str) -> (Option<i32>, String, PathBuf) { record_stalled_with(name, &[]) }

fn record_stalled_with(name: &str, guest_args: &[&str]) -> (Option<i32>, String, PathBuf) {
    let trace = std::env::temp_dir().join(format!("retrace-stall-{}-{name}.bin", std::process::id()));
    let mut args = vec!["record-dyn", retrace_guest::STALL_DYN, "-o", trace.to_str().unwrap()];
    if !guest_args.is_empty() { args.push("--"); args.extend(guest_args); }
    let r = util::run_bounded(util::bin(), &args, &[], BOUND);
    (r.code, r.stderr, trace)
}

#[test]
fn a_thread_that_spins_while_another_waits_to_run_is_named_once() {
    // Natively it exits in about 10 ms. The close2 gate saw a first exec of a freshly built binary
    // outlast a 20 s bound, so this bound is a margin, not a measurement.
    let native = util::run_bounded(retrace_guest::STALL_DYN, &[], &[], 120);
    assert_eq!(native.code, Some(0), "native exit");
    assert_eq!(String::from_utf8_lossy(&native.stdout), "stall_dyn done\n");

    let (code, err, _) = record_stalled("once");
    assert_eq!(code, None, "the alarm ends the recording; stderr: {err}");
    let lines: Vec<&str> = err.lines().filter(|l| l.starts_with("retrace:")).collect();
    assert_eq!(lines.len(), 1, "exactly one line of retrace's own: {err}");
    assert!(lines[0].starts_with("retrace: thread 0 has run for 20 s"), "{}", lines[0]);
    assert!(lines[0].contains("thread(s) 1 are ready to run"), "{}", lines[0]);
    assert!(lines[0].contains("docs/walls.md#threads"), "{}", lines[0]);
}

#[test]
fn a_spinner_beside_a_sleeping_thread_names_it_as_waiting_on_a_timeout() {
    // Final-review I3: the child sleeps an hour, so it is timed-blocked and never Runnable; the spinner
    // never reaches the settle that would wake it. The line must still name it. No native run: the
    // native program takes the hour.
    let (code, err, _) = record_stalled_with("sleeper", &["sleeper"]);
    assert_eq!(code, None, "the alarm ends the recording; stderr: {err}");
    let lines: Vec<&str> = err.lines().filter(|l| l.starts_with("retrace:")).collect();
    assert_eq!(lines.len(), 1, "exactly one line of retrace's own: {err}");
    assert!(lines[0].starts_with("retrace: thread 0 has run for 20 s"), "{}", lines[0]);
    assert!(lines[0].contains("thread(s) 1 are ready to run or waiting on a timeout."), "{}", lines[0]);
    assert!(lines[0].contains("docs/walls.md#threads"), "{}", lines[0]);
}

#[test]
fn replaying_a_stalled_recording_never_prints_the_stall_line() {
    // T7-replay-spin: the recording ends mid-spin, so its replay spins as the recording did and
    // never reaches the incomplete-recording line (a known limit, documented by Task 9). The bound
    // outlasts the 20 s threshold, so a replay that watched would have printed its line by then.
    let (_, _, trace) = record_stalled("replay");
    let r = util::run_bounded(util::bin(), &["replay", trace.to_str().unwrap()], &[], 25);
    assert_eq!(r.code, None, "the replay spins and the bound ends it: {}", r.stderr);
    assert!(!r.stderr.contains("has run for"), "{}", r.stderr);
}
