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
// The reading this checks is `crates/retrace-core/src/lib.rs:874-879`: `guest_ulock_wait` marks the
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

/// Pull `spsr` out of `dbg_regs_of`'s dump. Unlike `parse_x` (GPRs are zero-padded to a fixed
/// `#018x` width), `dbg_regs_of` prints `spsr={:#x}` with NO padding — its printed width varies
/// with the value — so this anchors on the `0x` prefix and reads the run of hex digits that
/// follows, rather than assuming a fixed slice width the way `parse_x` safely can.
fn parse_spsr(dump: &str) -> u64 {
    let key = "spsr=0x";
    let at = dump.find(key)
        .unwrap_or_else(|| panic!("no `{key}` in the register dump:\n{dump}"));
    let rest = &dump[at + key.len()..];
    let end = rest.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(rest.len());
    let hex = &rest[..end];
    u64::from_str_radix(hex, 16)
        .unwrap_or_else(|e| panic!("could not parse `{hex}` as spsr from the dump: {e}\n{dump}"))
}

// M17 Task 4b. Task 1 above measured R1 on ONE axis (x0). A review found a second axis nobody had
// looked at: `deliver_signal_to` writes the frame's PSTATE from the receiver's SAVED SPSR
// (`cpsr: ctx.spsr`, `crates/retrace-box/src/lib.rs:2973`), and `ctx.spsr` is a raw
// `SPSR_EL1` read (`save_ctx`, lib.rs:3161) that NOTHING on the wake path patches:
// `set_x0_err_and_return`/`apply_and_return` write only `reg::CPSR`, never `SPSR_EL1`, and the one
// function that DOES touch `SPSR_EL1` — `complete_syscall_before_delivery` — is deliberately not
// called on the wake arm, because the live vCPU there is the WAKER, not the receiver (see the
// comment at `crates/retrace-core/src/lib.rs` around the M17 wake landmark). So whatever is in a
// Wait-blocked thread's saved SPSR is whatever the architecture's automatic exception-entry save
// captured at the `svc` for `__ulock_wait` — untouched since. This measures that value directly.
#[test]
fn b_wait_blocked_threads_saved_spsr_is_an_unpatched_el0_pstate() {
    let (rec, trace) = util::record_dynamic(retrace_guest::THREADRUST);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    // Same discovery loop as the x0 measurement above: advance until some thread is parked in
    // Blocked(Wait).
    let mut s = ReplaySession::open(Path::new(&trace)).unwrap();
    let blocked = loop {
        if let Some(t) = s.thread_summaries().into_iter()
            .find(|t| matches!(t.state, ThreadState::Blocked(BlockReason::Wait { .. })))
        {
            break t.tid as usize;
        }
        match s.advance().expect("no divergence on an untampered trace") {
            Advance::Exited(_) => panic!(
                "THREADRUST never parked a thread in Blocked(Wait). Same precondition as the x0 \
                 measurement above; investigate before proceeding."),
            _ => continue,
        }
    };

    let dump = s.dbg_regs_of(blocked).expect("the blocked thread must have a saved context");
    let spsr = parse_spsr(&dump);
    let c_set = spsr & retrace_arch::PSTATE_C != 0;
    let mode = spsr & 0xf; // M[3:0]: exception level + SP select. EL0t (the only mode an EL0
                            // guest thread can trap FROM) encodes as 0b0000.

    // THE MEASUREMENT. Report the literal value loudly — this is a measurement first, an
    // assertion second, and the value is NOT decided in advance.
    eprintln!(
        "R1/PSTATE MEASURED: thread {blocked} is Blocked(Wait) with spsr={spsr:#x} \
         (bit 29 / C is {}, mode M[3:0]={mode:#x})",
        if c_set { "SET" } else { "CLEAR" }
    );

    // The saved SPSR must still be a plausible EL0 PSTATE — mode EL0t — because that is what
    // `deliver_signal_to` needs in order to resume the handler at EL0 at all (the comment at
    // lib.rs:2970-2973 depends on exactly this). If this ever fails, `ctx.spsr` is not what
    // `save_ctx` claims it is, which would be a correctness bug well upstream of M17.
    assert_eq!(mode, 0,
        "spsr={spsr:#x} has a non-EL0t mode (M[3:0]={mode:#x}) — the saved context is not a \
         plausible EL0 PSTATE at all, which is a deeper problem than R1. Dump:\n{dump}");

    // THE AXIS THIS TASK ADDS, AND THE FINDING. Bit 29 (C) is the bit a syscall-RETURN path would
    // normally set or clear — `set_x0_err_and_return`/`complete_syscall_before_delivery` exist
    // specifically to force it to reflect success/failure (see their doc comments; the
    // sigraisex0 probe measured the real kernel doing the same: a SUCCESSFUL syscall return comes
    // back with C **clear**). MEASURED: C is **SET** here, on a thread whose x0 == 0 — a
    // successful `__ulock_wait` return (proven by the test above). If this SPSR had gone through
    // that success/failure convention, a successful return would show C clear, not set. It
    // doesn't — which is exactly what "unpatched" predicts: nothing on the wake path touches
    // SPSR_EL1 for the receiver, so this C bit is not a manufactured completion flag at all, only
    // whatever the guest's own code left in NZCV at the `svc` for `__ulock_wait`, captured
    // as-is by the architecture's automatic exception-entry save. A caught signal delivered here
    // therefore resumes the handler carrying the guest's OWN incidental pre-syscall flags, not a
    // "syscall succeeded" flag retrace manufactured. That is consistent with R1 (x0 is still the
    // completed return value; only PSTATE is unpatched) but it is a DIFFERENT claim from "a
    // completed post-syscall state" — which is why it needed its own measurement rather than
    // riding on Task 1's. This assertion pins the measured bit rather than a guess: if a future
    // change starts routing the receiver's SPSR through `complete_syscall_before_delivery` (or an
    // equivalent), a successful __ulock_wait would flip C to clear here, and this test would be
    // the thing that catches it.
    assert!(c_set,
        "spsr={spsr:#x} has bit 29 (C) CLEAR; this test measured it SET. Expected failure mode: a \
         toolchain or OS bump moved the guest's own incidental pre-svc flags — that means the \
         premise this fix rests on changed, not that the fix broke. Dump:\n{dump}");
}
