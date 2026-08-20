// PARKED at the M16 blocked-target wall. This is the gate that comes green when a signal can be
// delivered to a thread that is blocked in `__ulock_wait` rather than merely not-current.
//
// It is committed as real, compiling code with a real guest behind it (`retrace-guest/rs/
// sigblocked.rs`) rather than as a comment, because honest-gate discipline means the wall is
// documented where someone hits it — and because a gate that cannot be run cannot be un-parked by
// simply deleting an attribute.
//
// **This asserts on the TRACE, not on the exit code, and that is the whole point of the file.**
// The guest's `on_usr1` is empty, so stdout is byte-identical whether or not the signal is ever
// delivered, and an exit-code-only gate would therefore come green under the single most likely
// wrong fix: making `deliver_signal_to` silently SKIP a blocked target instead of delivering to it
// (exactly what the `#[ignore]` reason below warns is "not merely relaxing the assert"). A skipped
// delivery exits 0 on both sides. CLAUDE.md's rule — "never assert on an exit code a weaker failure
// would also produce" — is the same rule that made `segv_rust_e2e` assert on its trace, and it
// applies here with more force, because this gate's whole purpose is to be un-parked by someone
// who did not write it.
//
// **These assertions are UNEXERCISED TODAY and cannot be verified by running.** The record run
// panics at the wall (exit 101) before a trace with a delivery in it can exist, so everything below
// the first `assert_eq!` is unreachable until the wall falls. They are written correct-by-
// construction from what Task 13 MEASURED and from the guest's own source, not from a green run:
//   - Thread 1 is `a`. The wall's own panic text names it — "thread 1 is Blocked(Wait { addr: … })"
//     is `deliver_signal_to` reporting the tid that `thread_of_port` resolved `b`'s
//     `pthread_kill(a_pt, …)` to. Tids are creation-ordered (main 0, `a` 1, `b` 2), and that
//     resolution is the thing this gate exists to check, so it is asserted rather than assumed.
//   - `a` really is blocked in `__ulock_wait` when the signal arrives — same measurement: the state
//     the panic reports is `Blocked(Wait { addr })`, which only `guest_ulock_wait` produces.
//   - Sig 30 is `SIGUSR1`, the only signal the guest raises, and it raises it exactly once.
// If the wall falls and one of these fails, read it as a claim about the FIX rather than as a stale
// assertion: each names the property that separates delivering the signal from skipping it.
mod util;
use retrace_trace::Event;

#[test]
#[ignore = "M16 wall: signalling a thread that is BLOCKED in __ulock_wait is unmodelled. \
            MEASURED, not assumed — forced with --ignored, the RECORD run panics (exit code 101, \
            not a guest-visible failure) at crates/retrace-box/src/lib.rs:2849: 'thread 1 is \
            Blocked(Wait { addr: 809578548 }), not Runnable; deliver_signal_to would overwrite \
            the saved context its blocking syscall must resume through. Wake or skip it instead \
            of redirecting a thread that cannot run yet.' That is a deliberate M16 fail-loud guard \
            (deliver_signal_to's Blocked/Exited match arms, added because the failure mode is \
            silent corruption, not a panic, if it fires unguarded): a Blocked thread's saved ctx IS \
            the resume point its own blocking syscall (__ulock_wait) owes a return value through, \
            so redirecting it to a signal handler would overwrite that resume point out from under \
            the wait. Fixing this needs the wake path and the signal-delivery path to cooperate — \
            wake-then-deliver or deliver-then-resume-the-wait — not merely relaxing the assert. \
            UN-IGNORE when a blocked target is modelled."]
fn a_signal_reaches_a_thread_blocked_in_ulock_wait() {
    let (rec, trace) = util::record_dynamic(retrace_guest::SIGBLOCKED);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    let events = retrace_trace::Reader::open(&trace).unwrap();

    // Tooth 1 — the delivery happened, and it named `a`. `b` raises SIGUSR1 exactly once, so the
    // whole list is the assertion: an empty list is the skip-the-blocked-target non-fix, `[0]` or
    // `[2]` is the signal landing on whoever held the vCPU (the pre-M16 defect, re-created), and a
    // second entry would mean the pended bit was materialised twice.
    let delivered: Vec<u32> = events.iter().filter_map(|e| match e {
        Event::SignalDelivery { sig: 30, thread, .. } => Some(*thread),
        _ => None,
    }).collect();
    assert_eq!(delivered, vec![1u32],
        "exactly one SIGUSR1 delivery, to thread 1 — `a`, the thread `b` named by pthread_t. An \
         EMPTY list is the failure this gate exists to reject: a fix that skips a blocked target \
         rather than delivering to it exits 0 on both sides and changes no stdout, because this \
         guest's handler is empty.");

    let di = events.iter().position(|e| matches!(e, Event::SignalDelivery { sig: 30, .. }))
        .expect("the delivery asserted above must have an index");

    // Tooth 2 — `a` was genuinely BLOCKED when the signal arrived, and its wait still resumed.
    // Both halves matter and neither implies the other. The first is what makes this gate about
    // the blocked-target wall at all rather than about a merely-not-current target (which
    // `sigthread_e2e` already covers): thread 1 must have entered `__ulock_wait` before the
    // delivery landmark. The second is the resume: `a` must run again afterwards, which it can
    // only do by coming back through the wait its saved context was parked in — the exact resume
    // point the wall's panic says a naive redirect would overwrite.
    assert!(events[..di].iter().any(|e| matches!(e,
                Event::Syscall { num, thread: 1, .. } if *num == retrace_arch::SYS_ULOCK_WAIT)),
        "thread 1 must have blocked in __ulock_wait BEFORE the delivery, or this gate is measuring \
         the not-current case instead of the blocked case");
    assert!(events[di + 1..].iter().any(|e| matches!(e, Event::Syscall { thread: 1, .. })),
        "thread 1 must issue at least one more landmark after taking the signal — the wait has to \
         resume, not be stranded. It cannot print `a resumed` otherwise.");

    // The user-visible half of the same claim, in the only order the guest's source permits: `b`
    // prints after its pthread_kill returns, `a` prints only once its join over `b` returns, and
    // main prints only once its join over `a` returns. A stranded wait loses the last two lines.
    let out = String::from_utf8_lossy(&rec.stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines, ["b signalled a", "a resumed", "done"],
        "the guest's three lines, in program order. Full stdout:\n{out}");

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay must be clean; stderr:\n{}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout, "replay must be byte-identical");
}
