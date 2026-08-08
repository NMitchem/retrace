// THE M12 HEADLINE GATE. A stock full-std Rust binary faults on a wild pointer. libstd's own
// SIGSEGV handler runs, decides it is not a stack overflow, resets to SIG_DFL and RETURNS; the
// store re-executes, faults again, and the default action terminates the guest.
//
// Exit 139 alone proves nothing: an UNCAUGHT fault exits 139 too — crashy_e2e asserts exactly that
// — so a gate resting on the exit code would pass unchanged if M12's routing were entirely broken
// and the handler ignored the way it was before this milestone. The trace assertions are the gate.
mod util;
use retrace_trace::Event;

#[test]
fn a_faulting_rust_guest_runs_its_own_handler_and_records_its_death() {
    let (rec, trace) = util::record_dynamic(retrace_guest::SEGVY);
    assert_eq!(rec.code, 139, "139 == 128 + SIGSEGV; stderr:\n{}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout);
    assert!(out.starts_with("about to fault\n"),
        "the guest must reach its OWN code, not die inside dyld; stdout:\n{out}");
    assert!(!out.contains("survived"),
        "the store must re-execute and kill it, not be skipped; stdout:\n{out}");
    assert!(!out.contains("has overflowed its stack"),
        "libstd compares si_addr against its guard range — this message means si_addr is WRONG \
         (and the guest would have exited 134, not 139); stdout:\n{out}");

    let (events, torn) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(!torn, "a recorder killed mid-run leaves a torn trace — this must be complete");

    // (1) exactly one delivery, for SIGSEGV, to the handler libstd actually installed.
    let deliveries: Vec<_> = events.iter().enumerate()
        .filter(|(_, e)| matches!(e, Event::SignalDelivery { .. })).collect();
    assert_eq!(deliveries.len(), 1, "exactly one handler entry");
    let (di, Event::SignalDelivery { sig, handler, resume_pc, .. }) = deliveries[0] else {
        unreachable!()
    };
    assert_eq!(*sig, 11);
    let installed = installed_sigsegv_handler(&trace, &events, di)
        .expect("libstd installs a SIGSEGV handler at startup — M11 measured flags 0x41");
    assert_eq!(*handler, installed,
        "the delivery must target the handler the guest installed ({installed:#x}), not some \
         other address");

    // (2) a sigreturn AFTER it: the handler RETURNED rather than aborting.
    let si = events.iter().enumerate().position(|(i, e)| i > di && matches!(e,
        Event::Syscall { num, .. } if *num == retrace_arch::SYS_SIGRETURN))
        .expect("libstd's handler resets to SIG_DFL and returns — there must be a sigreturn");

    // (3) a terminal Crash after that: the re-fault took the default action.
    //
    // Crash, NOT Signal, and the distinction is the architecture rather than a detail. A signal the
    // guest RAISES on itself with a fatal default action is `Event::Signal` (M11); a signal derived
    // from a HARDWARE FAULT whose disposition is not a handler goes down M6's `Event::Crash` path
    // byte-for-byte unchanged — it really is a fault, and the hardware really did produce the ESR,
    // so recording it as anything else would be the mirror of the lie M11 refused when it declined
    // to fold `Event::Signal` into `Crash`. `crashy_e2e` pins the same variant for a fault that was
    // never caught at all; what distinguishes THIS run from that one is assertions (1) and (2).
    let ti = events.iter().position(|e| matches!(e, Event::Crash { .. }))
        .expect("the re-fault must terminate the guest");
    assert!(ti > si, "the terminal crash follows the sigreturn");
    assert!(matches!(events.last(), Some(Event::Snapshot { .. })),
        "terminal events are followed by the final full-memory snapshot");

    // (4) the store re-executed rather than being skipped.
    let crash_pc = match &events[ti] { Event::Crash { pc, .. } => *pc, _ => unreachable!() };
    assert_eq!(*resume_pc, crash_pc,
        "sigreturn resumed AT the faulting instruction, so the second fault is the same store");

    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 139, "replay {i}; stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged");
    }
}

/// The handler VA libstd installed, learned from the guest's own `sigaction` call rather than
/// hardcoded (it moves with every build).
///
/// The trace does not carry it as a datum: `sigaction`'s event records `args[1]`, a POINTER to the
/// guest's `struct __sigaction`, and `writes` holds only what the kernel wrote back (the `oldact`).
/// So this seeks a replay session to that landmark and reads `sa_handler` out of guest memory —
/// which also means the assertion is checking retrace's own reconstruction of that memory.
///
/// Takes the last install BEFORE the delivery with a non-null handler: libstd's handler resets the
/// disposition to `SIG_DFL` from inside itself, so there is a later `sigaction(11, …)` whose
/// `sa_handler` is 0.
///
/// Seeks to `li + 1`, NOT `li`, and the off-by-one is the whole subtlety. A coordinate `(N, 0)` is
/// the state after `N` events have been *consumed*, so event `N` is the one still to come
/// (`peek_syscall` names it "the NEXT trace event to be consumed") and the guest is parked at the
/// START of the window leading to it — before it has executed the stores that fill the struct.
/// Reading at `(li, 0)` therefore returns whatever the stack held beforehand; measured here, that
/// was `handler=0x4000, mask=0x27ff7a8` — a stack address sitting in the mask field, which is what
/// uninitialized reads look like. At `(li + 1, 0)` the window has run and the syscall is applied,
/// and nothing has overwritten the struct yet. `li + 1` is resumable because a `sigaction` is a
/// plain serviced syscall: only a caught raise writes the two-event pair that makes an intermediate
/// coordinate unoccupiable.
fn installed_sigsegv_handler(trace: &std::path::Path, events: &[Event], before: usize) -> Option<u64> {
    let (li, act_ptr) = events.iter().enumerate().take(before).rev().find_map(|(i, e)| match e {
        Event::Syscall { num, args, .. }
            if *num == retrace_arch::SYS_SIGACTION && args[0] == 11 && args[1] != 0 => {
            Some((i, args[1]))
        }
        _ => None,
    })?;
    let s = retrace_core::seek(trace, li + 1, 0).expect("seek past the sigaction landmark");
    let bytes = s.read_mem(act_ptr, 8)?;
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

#[test]
fn the_delivery_is_a_seekable_landmark() {
    // The payoff that justified a first-class event over below-the-trace handling. If this cannot
    // be done, the architecture decision in the spec was wrong and should be revisited, not the test.
    let (_, trace) = util::record_dynamic(retrace_guest::SEGVY);
    let (events, _) = retrace_trace::Reader::open_checked(&trace).unwrap();
    let di = events.iter().position(|e| matches!(e, Event::SignalDelivery { .. })).unwrap();
    let s = retrace_core::seek(&trace, di, 0).expect("seek to the delivery landmark");
    assert_eq!(s.landmark(), di);
}
