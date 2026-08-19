// Session-level watchpoint tests (M5). All addresses discovered from the freshly recorded trace:
// watchloop's write(1, target, 8) publishes `target` in the recorded syscall args.
mod util;
use std::path::Path;
use retrace_core::{Advance, Outcome, ReplaySession};

/// `target`'s guest VA, from the recorded write(1, target, 8): the first fd-1 write's args[1].
fn discover_target(trace: &Path) -> u64 {
    let mut s = ReplaySession::open(trace).unwrap();
    loop {
        if let Some((4, args)) = s.peek_syscall() {
            if args[0] == 1 { return args[1]; }
        }
        s.advance().unwrap();
    }
}

#[test]
fn hw_watchpoint_fires_on_store_pre_retire_with_far() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let target = discover_target(tp);
    let mut s = ReplaySession::open(tp).unwrap();
    s.arm_watchpoints(&[(target, 8)]);
    match s.advance().unwrap() {
        Advance::Watch { thread: _ } => {
            let far = s.far();
            assert!(far >= target && far < target + 8, "far {far:#x} outside watched [{target:#x}; +8)");
            // Pre-retire (spike F4c): the first store (value 1) has NOT landed yet.
            assert_eq!(s.read_mem(target, 8).unwrap(), vec![0u8; 8], "store must not have retired");
        }
        _ => panic!("expected Advance::Watch"),
    }
}

/// M15 Task 5: a hardware watch hit reports WHICH thread stored.
///
/// WATCHLOOP is single-threaded, so the only truthful answer here is thread 0. What this pins is
/// the plumbing — the field exists, is populated from the live thread table at the hit site, and
/// reaches the caller. It does NOT prove the report can tell two live threads apart, because this
/// guest has no second thread to be wrong about; that is Task 9's headline gate, on THREADRUST.
/// Stated here rather than left implied: an assertion whose guest cannot exhibit the failure it
/// guards is worth exactly its plumbing check, and saying so is cheaper than re-deriving it later.
#[test]
fn hw_watch_hit_names_the_writing_thread() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let target = discover_target(tp);
    let mut s = ReplaySession::open(tp).unwrap();
    s.arm_watchpoints(&[(target, 8)]);
    match s.advance().unwrap() {
        Advance::Watch { thread } => assert_eq!(thread, 0,
            "the store came from the guest's only thread; the hit must name it"),
        _ => panic!("expected Advance::Watch"),
    }
}

#[test]
fn watch_on_untouched_bytes_never_fires() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let target = discover_target(tp);
    // target2 = target + 8 (consecutive .quad slots). The guest strb's ONLY byte 0 of target2;
    // watching bytes 4..8 (BAS=0xF0 in the same doubleword) must never fire.
    let mut s = ReplaySession::open(tp).unwrap();
    s.arm_watchpoints(&[(target + 8 + 4, 4)]);
    loop {
        match s.advance().unwrap() {
            Advance::Event => continue,
            Advance::Exited(report) => { assert_eq!(report.outcome, Outcome::Exit { code: 0 }); break; }
            _ => panic!("watchpoint fired on untouched bytes"),
        }
    }
}

#[test]
fn mde_survives_clear_breakpoints_with_watches_armed() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let target = discover_target(tp);
    let mut s = ReplaySession::open(tp).unwrap();
    s.arm_watchpoints(&[(target, 8)]);
    s.arm_breakpoints(&[0xdead_0000]); // never matched
    s.clear_breakpoints();             // must NOT disarm the watchpoint (shared MDSCR.MDE)
    assert!(matches!(s.advance().unwrap(), Advance::Watch { thread: _ }),
        "watchpoint died when breakpoints were cleared (MDE sharing bug)");
}

/// The read()'s buffer VA and the landmark index AFTER consuming the read event, from the trace.
fn discover_read(trace: &Path) -> (usize, u64) {
    let mut s = ReplaySession::open(trace).unwrap();
    loop {
        if let Some((3, args)) = s.peek_syscall() {
            s.advance().unwrap();
            return (s.landmark(), args[1]);
        }
        s.advance().unwrap();
    }
}

/// fstat()'s statbuf VA and the landmark index AFTER consuming the fstat event.
fn discover_fstat(trace: &Path) -> (usize, u64) {
    let mut s = ReplaySession::open(trace).unwrap();
    loop {
        if let Some((189, args)) = s.peek_syscall() {
            s.advance().unwrap();
            return (s.landmark(), args[1]);
        }
        s.advance().unwrap();
    }
}

#[test]
fn syscall_write_to_watched_buf_is_reported_and_replay_completes() {
    let (rec, trace) = util::record(retrace_guest::FILEIO);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let (after_read, buf) = discover_read(tp);
    let mut s = ReplaySession::open(tp).unwrap();
    s.arm_watchpoints(&[(buf, 8)]);
    // open consumes as Event (no writes hit); fstat writes statbuf only; read MUST report.
    let hit_at = loop {
        match s.advance().unwrap() {
            Advance::WatchSyscall { watched, thread: _ } => { assert_eq!(watched, buf); break s.landmark(); }
            Advance::Event => continue,
            _ => panic!("unexpected advance kind before the read"),
        }
    };
    assert_eq!(hit_at, after_read, "hit must be the read event's boundary");
    // Detection observed, never interfered: the rest of the replay completes byte-perfectly.
    loop {
        match s.advance().unwrap() {
            Advance::Event => continue,
            Advance::Exited(report) => {
                assert_eq!(report.outcome, Outcome::Exit { code: 0 });
                assert_eq!(report.stdout, b"retrace-m1-fixture\n".to_vec());
                break;
            }
            _ => panic!("no further watch hits expected"),
        }
    }
}

#[test]
fn fstat_statbuf_write_is_detected() {
    let (rec, trace) = util::record(retrace_guest::FILEIO);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let (after_fstat, statbuf) = discover_fstat(tp);
    let mut s = ReplaySession::open(tp).unwrap();
    s.arm_watchpoints(&[(statbuf, 8)]);
    loop {
        match s.advance().unwrap() {
            Advance::WatchSyscall { watched, thread: _ } => {
                assert_eq!(watched, statbuf);
                assert_eq!(s.landmark(), after_fstat);
                break;
            }
            Advance::Event => continue,
            _ => panic!("expected the fstat WatchSyscall first"),
        }
    }
}

/// M15 Task 5: the software (applied-writes) watch path reports the thread too.
///
/// A separate test from the hardware one because these are two independent construction sites in
/// `advance`: the hardware hit is built in the `Stop::Other` watchpoint arm, the syscall hit in
/// `finish_event`. Populating one and forgetting the other is the obvious way to half-do this, and
/// a single test over either path would not notice. Same single-thread vacuity caveat as above.
#[test]
fn syscall_watch_hit_names_the_writing_thread() {
    let (rec, trace) = util::record(retrace_guest::FILEIO);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let (_after_fstat, statbuf) = discover_fstat(tp);
    let mut s = ReplaySession::open(tp).unwrap();
    s.arm_watchpoints(&[(statbuf, 8)]);
    loop {
        match s.advance().unwrap() {
            Advance::WatchSyscall { watched, thread } => {
                assert_eq!(watched, statbuf);
                assert_eq!(thread, 0, "the kernel wrote on behalf of the guest's only thread");
                break;
            }
            Advance::Event => continue,
            _ => panic!("expected the fstat WatchSyscall first"),
        }
    }
}
