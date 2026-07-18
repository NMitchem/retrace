// Session-level watchpoint tests (M5). All addresses discovered from the freshly recorded trace:
// watchloop's write(1, target, 8) publishes `target` in the recorded syscall args.
mod util;
use std::path::Path;
use retrace_core::{Advance, ReplaySession};

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
        Advance::Watch => {
            let far = s.far();
            assert!(far >= target && far < target + 8, "far {far:#x} outside watched [{target:#x}; +8)");
            // Pre-retire (spike F4c): the first store (value 1) has NOT landed yet.
            assert_eq!(s.read_mem(target, 8).unwrap(), vec![0u8; 8], "store must not have retired");
        }
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
            Advance::Exited(report) => { assert_eq!(report.exit_code, 0); break; }
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
    assert!(matches!(s.advance().unwrap(), Advance::Watch),
        "watchpoint died when breakpoints were cleared (MDE sharing bug)");
}
