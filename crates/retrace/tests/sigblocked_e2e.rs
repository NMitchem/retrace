// PARKED at the M16 blocked-target wall. This is the gate that comes green when a signal can be
// delivered to a thread that is blocked in `__ulock_wait` rather than merely not-current.
//
// It is committed as real, compiling code with a real guest behind it (`retrace-guest/rs/
// sigblocked.rs`) rather than as a comment, because honest-gate discipline means the wall is
// documented where someone hits it — and because a gate that cannot be run cannot be un-parked by
// simply deleting an attribute.
mod util;

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
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay must be clean; stderr:\n{}", rep.stderr);
}
