//! M51 §3g: a handler installed with `signal()` runs as natively; signal 0 asks and is answered as
//! natively, for a pid and for a thread; and `abort_with_payload` ends the program with SIGABRT, as
//! natively, whether the program calls it or dyld does for a missing dylib.
mod util;
use retrace_trace::{Event, Reader};
use std::path::{Path, PathBuf};

const BOUND: u64 = 120;

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("retrace-sigfid-{}-{name}", std::process::id()))
}

/// Native, then record and two replays: all four must agree in exit code and stdout, and nothing
/// stops at a wall. Returns native's code and the recording's events.
fn agree(prog: &str, args: &[&str], t: &Path) -> (i32, Vec<Event>) {
    let what = format!("{} {args:?}", Path::new(prog).file_name().unwrap().to_string_lossy());
    let native = util::run_bounded(prog, args, &[], BOUND);
    let ncode = native.code.unwrap_or_else(|| panic!("{what}: native hung"));
    let mut argv = vec!["record", "-o", t.to_str().unwrap(), prog];
    if !args.is_empty() { argv.extend_from_slice(args); }
    let rec = util::run_bounded(util::bin(), &argv, &[], BOUND);
    assert!(!rec.stderr.contains("retrace: unsupported:"), "{what}: no wall. stderr:\n{}", rec.stderr);
    assert_eq!((rec.code, String::from_utf8_lossy(&rec.stdout)), (Some(ncode), String::from_utf8_lossy(&native.stdout)),
        "{what}: record against native. stderr:\n{}", rec.stderr);
    for i in 1..=2 {
        let rp = util::run_bounded(util::bin(), &["replay", t.to_str().unwrap()], &[], BOUND);
        assert_eq!((rp.code, rp.stdout.as_slice()), (rec.code, rec.stdout.as_slice()), "{what}: replay {i}. stderr:\n{}", rp.stderr);
    }
    (ncode, Reader::open(t).unwrap())
}

/// Spec §4 "Signals": `signal(SIGUSR1, h)` then two raises. The handler runs both times, with the
/// infostyle a handler without `SA_SIGINFO` receives. The `sig.rs` unit test pins that infostyle (1,
/// `UC_TRAD`); this test pins that the handler runs each time. Then the same handler for a
/// fault, which it ends with `_exit(42)`.
#[test]
fn a_handler_installed_with_signal_runs_each_time_its_signal_is_raised() {
    let t = scratch("raise.bin");
    let (code, events) = agree(retrace_guest::FID_SIGNALS, &["raise"], &t);
    assert_eq!(code, 0);
    let n = events.iter().filter(|e| matches!(e, Event::SignalDelivery { sig: 30, .. })).count();
    assert_eq!(n, 2, "two deliveries of SIGUSR1");
    let (code, events) = agree(retrace_guest::FID_SIGNALS, &["fault"], &t);
    assert_eq!(code, 42, "the handler ran and exited");
    assert!(events.iter().any(|e| matches!(e, Event::SignalDelivery { sig: 11, .. })), "the fault was delivered");
    let _ = std::fs::remove_file(&t);
}

/// C11: the `signal()` column of T3-b's matrix. A fault a `signal()` handler catches runs it where
/// the frame can be pushed, and is the kill native makes where it cannot (t0 M2).
#[test]
fn a_handler_installed_with_signal_takes_a_fault_as_natively() {
    for (thread, fault, want) in [("main", "null", 42), ("child", "null", 42), ("main", "overflow", 139), ("child", "overflow", 132)] {
        let t = scratch(&format!("s-{thread}-{fault}.bin"));
        let (code, _) = agree(retrace_guest::FID_FRAMES, &[thread, fault, "s"], &t);
        assert_eq!(code, want, "{thread} {fault} s");
        let _ = std::fs::remove_file(&t);
    }
}

/// R11, C4: `kill(pid, 0)` for the program's own pid, its process group, a process it may not signal
/// and one that does not exist. Each is forwarded, recorded and answered as natively.
#[test]
fn kill_with_signal_0_answers_as_natively_for_each_pid() {
    for mode in ["kill0self", "kill00", "kill0init", "kill0none"] {
        let t = scratch(&format!("{mode}.bin"));
        let (_, events) = agree(retrace_guest::FID_SIGNALS, &[mode], &t);
        assert!(events.iter().any(|e| matches!(e, Event::Syscall { num: 37, args, .. } if args[1] == 0)),
            "{mode}: the recorded kill(…, 0)");
        let _ = std::fs::remove_file(&t);
    }
}

/// C4: `pthread_kill(t, 0)` for the calling thread and for one already joined.
#[test]
fn pthread_kill_with_signal_0_answers_as_natively() {
    for mode in ["pkill0self", "pkill0exited"] {
        let t = scratch(&format!("{mode}.bin"));
        agree(retrace_guest::FID_SIGNALS, &[mode], &t);
        let _ = std::fs::remove_file(&t);
    }
}

/// R10: `abort_with_payload` from main, from a child thread, and from dyld for a missing dylib, is
/// SIGABRT: a terminal `Event::Signal` and the closing line, as natively.
#[test]
fn abort_with_payload_ends_the_program_with_sigabrt() {
    for (prog, args) in [(retrace_guest::FID_SIGNALS, &["payload"][..]), (retrace_guest::FID_SIGNALS, &["payloadthread"][..]),
                         (retrace_guest::FID_MISSING, &[][..])] {
        let t = scratch("payload.bin");
        let (code, events) = agree(prog, args, &t);
        assert_eq!(code, 134, "{args:?}");
        assert!(matches!(events[events.len() - 2], Event::Signal { sig: 6, .. }), "{args:?}: {:?}", events[events.len() - 2]);
        let rp = util::run_bounded(util::bin(), &["replay", t.to_str().unwrap()], &[], BOUND);
        assert_eq!(rp.stderr.lines().filter(|l| l.starts_with("retrace:")).collect::<Vec<_>>(),
                   ["retrace: the program was killed by SIGABRT"], "{args:?}");
        let _ = std::fs::remove_file(&t);
    }
    let native = util::run_bounded(retrace_guest::FID_MISSING, &[], &[], BOUND);
    assert!(native.stderr.contains("Library not loaded: @rpath/libfidgone.dylib"), "native dyld: {}", native.stderr);
    let t = scratch("missing.bin");
    let rec = util::run_bounded(util::bin(), &["record", "-o", t.to_str().unwrap(), retrace_guest::FID_MISSING], &[], BOUND);
    assert!(rec.stderr.contains("Library not loaded: @rpath/libfidgone.dylib"), "dyld's own line, on stderr: {}", rec.stderr);
    let _ = std::fs::remove_file(&t);
}

/// C4: replay recomputes `__pthread_kill(port, 0)`'s answer from its own thread table and compares it
/// with the recording's. A recording whose answer was changed diverges at that landmark.
#[test]
fn a_tampered_answer_to_pthread_kill_with_signal_0_is_a_divergence() {
    let t = scratch("tamper.bin");
    let rec = util::run_bounded(util::bin(), &["record", "-o", t.to_str().unwrap(), retrace_guest::FID_SIGNALS, "pkill0self"], &[], BOUND);
    assert_eq!(rec.code, Some(0), "{}", rec.stderr);
    let mut events = Reader::open(&t).unwrap();
    let i = events.iter().position(|e| matches!(e, Event::Syscall { num: 328, args, .. } if args[1] == 0))
        .expect("the recorded __pthread_kill(self, 0)");
    if let Event::Syscall { ret, err, .. } = &mut events[i] { *ret = retrace_arch::ESRCH; *err = true; }
    let mut w = retrace_trace::Writer::create(&t).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);
    let rp = util::run_bounded(util::bin(), &["replay", t.to_str().unwrap()], &[], BOUND);
    assert_eq!(rp.code, Some(3), "a divergence. stderr:\n{}", rp.stderr);
    assert!(rp.stderr.contains("__pthread_kill(port"), "the probe's own compare names it. stderr:\n{}", rp.stderr);
    let _ = std::fs::remove_file(&t);
}
