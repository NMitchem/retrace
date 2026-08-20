// M17 Task 1 / spec risk R1. The M17 design rests on ONE fact, and this file is its measurement:
//
//   A `Wait`-blocked thread's saved context is a COMPLETE post-syscall state — x0 already holds
//   `__ulock_wait`'s return value and the pc is already past the `svc`.
//
// If that holds, a signal frame built on that context PRESERVES the resume point rather than
// overwriting it (the frame saves what it displaces; `sigreturn` restores it), and M16's parked
// wall is narrower than its own `#[ignore]` text describes. If it is false, materialisation must
// first complete the syscall on the SAVED context, which is a different design.
//
// The reading this checks is `crates/retrace-core/src/lib.rs:865-870`: `guest_ulock_wait` marks the
// thread Blocked, and only THEN does `set_x0_err_and_return` write x0 and advance the pc on the
// live vCPU; the switch that saves it happens on the next `run()`.
mod util;
use retrace_core::{Advance, BlockReason, ReplaySession, ThreadState};
use std::path::Path;

/// Pull `x{n}` out of `dbg_regs_of`'s dump. The format is `format_gprs`'s
/// `x{i:<2}={xi:#018x}  ` — note the left-aligned index, so `x0` is followed by a space.
fn parse_x(dump: &str, n: usize) -> u64 {
    let key = format!("x{n:<2}=");
    let at = dump.find(&key)
        .unwrap_or_else(|| panic!("no `{key}` in the register dump:\n{dump}"));
    let hex = &dump[at + key.len()..][..18]; // "0x" + 16 hex digits
    u64::from_str_radix(hex.trim_start_matches("0x"), 16)
        .unwrap_or_else(|e| panic!("could not parse `{hex}` from the dump: {e}\n{dump}"))
}

#[test]
fn a_wait_blocked_threads_saved_context_is_a_completed_syscall() {
    let (rec, trace) = util::record_dynamic(retrace_guest::THREADRUST);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    // Advance until some thread is parked in Blocked(Wait). THREADRUST's main joins its child, and
    // `__pthread_join` blocks in `__ulock_wait`, so this state is reached on every run.
    let mut s = ReplaySession::open(Path::new(&trace)).unwrap();
    let blocked = loop {
        if let Some(t) = s.thread_summaries().into_iter()
            .find(|t| matches!(t.state, ThreadState::Blocked(BlockReason::Wait { .. })))
        {
            break t.tid as usize;
        }
        match s.advance().expect("no divergence on an untampered trace") {
            Advance::Exited(_) => panic!(
                "THREADRUST never parked a thread in Blocked(Wait). `pthread_join` blocking in \
                 `__ulock_wait` is the premise of the whole M17 design, so either the guest \
                 changed or `guest_ulock_wait` stopped blocking — investigate before proceeding."),
            _ => continue,
        }
    };

    let dump = s.dbg_regs_of(blocked).expect("the blocked thread must have a saved context");
    let x0 = parse_x(&dump, 0);

    // THE MEASUREMENT. x0 == 0 is `__ulock_wait`'s return value, i.e. a COMPLETED syscall.
    // The two operation words are what x0 would hold if the context were the PRE-syscall state —
    // they are `guest_ulock_wait`'s own whitelist (`crates/retrace-box/src/lib.rs`), so they are
    // the sharpest possible discriminator rather than an arbitrary sentinel.
    assert_ne!(x0, 0x1000002,
        "R1 FALSE: x0 holds __ulock_wait's OPERATION WORD, so the saved context is the PRE-syscall \
         state. The M17 design's load-bearing claim does not hold — STOP and re-shape the design \
         per the spec's R1 note. Dump:\n{dump}");
    assert_ne!(x0, 0x1020002,
        "R1 FALSE: x0 holds __ulock_wait's other operation word (see the assertion above). \
         Dump:\n{dump}");
    assert_eq!(x0, 0,
        "R1: a Wait-blocked thread's saved context must be a COMPLETE post-syscall state, with x0 \
         holding __ulock_wait's return value of 0. Got {x0:#x} — neither the return value nor \
         either operation word, so the ordering is something this measurement did not anticipate. \
         Investigate before building on it. Dump:\n{dump}");

    eprintln!("R1 MEASURED: thread {blocked} is Blocked(Wait) with a completed context, x0={x0:#x}");
}
