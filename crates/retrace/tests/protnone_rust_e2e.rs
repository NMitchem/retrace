// THE M13 HEADLINE GATE. A stock full-std Rust binary protects one of its own pages PROT_NONE and
// stores through it. The store takes a stage-1 PERMISSION fault — something no guest could produce
// before this milestone — which routes through M12's delivery to libstd's own handler and kills it.
//
// The exit code proves nothing on its own: crashy_e2e already asserts that an uncaught fault kills a
// guest, and an UNPROTECTED store to a wild address would kill this one just as dead with M13's
// enforcement entirely absent. The DFSC assertion is the gate — 0x0f (permission) rather than
// 0x04..0x07 (translation) is exactly the difference M13 creates.
mod util;
use retrace_trace::Event;

#[test]
fn a_rust_guest_faults_on_a_page_it_protected_itself() {
    let (rec, trace) = util::record_dynamic(retrace_guest::PROTRUST);
    let out = String::from_utf8_lossy(&rec.stdout);
    assert!(out.contains("mapped and touched"),
        "the guest must reach its own code, not die in dyld; stdout:\n{out}");
    assert!(out.contains("protected"), "mprotect must succeed; stdout:\n{out}");
    assert!(!out.contains("survived"),
        "THE STORE THROUGH A PROT_NONE PAGE MUST FAULT. Reaching this line is what a stale \
         permissive TLB entry looks like — protect_none stamped ATTR_NONE but the flush did not \
         take; stdout:\n{out}");

    // 139, not 138, and the difference is architecture rather than an accident. Run NATIVELY this
    // guest dies of SIGBUS and exits 138; under retrace a fault-derived death is `Outcome::Crash`,
    // whose CLI convention is a flat `exit(139)` for every crash whatever signal it maps to
    // (main.rs:21-23, M6). Only a signal the guest RAISES on itself takes the `128 + sig` path.
    // So 139 here says nothing about SIGBUS — the trace assertions below are what pin that.
    assert_eq!(rec.code, 139, "M6's crash convention is a flat 139; stderr:\n{}", rec.stderr);

    let (events, torn) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(!torn, "a recorder killed mid-run leaves a torn trace — this must be complete");

    // The page the guest protected, learned from its own recorded mprotect rather than hardcoded.
    //
    // The LAST such call, not the first: this run contains four `mprotect(…, PROT_NONE)` calls, and
    // three of them are libSystem's own startup work — including libstd's stack guard at 0x2004000,
    // the very address M13 Task 2 measured and the reason the stack-overflow gate is parked. The
    // guest's own protect is the last one, immediately before the fault. If that ever stops being
    // true the `far` assertion below fails loudly with both addresses rather than quietly checking
    // the wrong page.
    let protected = events.iter().rev().find_map(|e| match e {
        Event::Syscall { num, args, .. }
            if *num == retrace_arch::SYS_MPROTECT && args[2] == 0 => Some(args[0]),
        _ => None,
    }).expect("the guest issues mprotect(…, PROT_NONE)");

    // (1) A PERMISSION fault at that page — the assertion the exit code cannot make.
    let (esr, far) = events.iter().find_map(|e| match e {
        Event::Crash { esr, far, .. } => Some((*esr, *far)),
        _ => None,
    }).expect("the protected store must terminate the guest");
    assert_eq!(esr & 0x3f, 0x0f,
        "DFSC must be 0x0f (permission fault, level 3), got {:#x}. A translation fault (0x04..0x07) \
         would mean the page was UNMAPPED rather than AP-denied — a different mechanism that would \
         pass a weaker gate.", esr & 0x3f);
    assert_eq!(far & !0x3fff, protected,
        "the fault must be at the protected page {protected:#x}, got {far:#x}");
    assert_eq!(retrace_arch::signal_of_esr(esr), (retrace_arch::SIGBUS, retrace_arch::BUS_ADRALN),
        "a protection failure is SIGBUS/BUS_ADRALN on Darwin (measured, spikes/protnone.c)");

    // (2) libstd installs SIGSEGV/SIGBUS handlers at startup, so the fault is DELIVERED first —
    // this guest exercises M12's delivery through a permission fault for the first time.
    let deliveries: Vec<_> = events.iter().enumerate()
        .filter(|(_, e)| matches!(e, Event::SignalDelivery { .. })).collect();
    assert_eq!(deliveries.len(), 1, "exactly one handler entry");
    let (di, Event::SignalDelivery { sig, si_addr, resume_pc, .. }) = deliveries[0] else {
        unreachable!()
    };
    assert_eq!(*sig, retrace_arch::SIGBUS, "delivered as the signal Darwin actually raises");
    assert_eq!(si_addr & !0x3fff, protected, "si_addr must name the protected page");

    // (3) The handler RETURNED and the store re-executed — beyond the plan, and it is what separates
    // "libstd saw the fault" from "libstd handled it and the hardware faulted again". Same shape
    // segv_rust_e2e pins for SIGSEGV, now proven for a permission fault.
    let si = events.iter().enumerate().position(|(i, e)| i > di && matches!(e,
        Event::Syscall { num, .. } if *num == retrace_arch::SYS_SIGRETURN))
        .expect("libstd's handler resets to SIG_DFL and returns — there must be a sigreturn");
    let ti = events.iter().position(|e| matches!(e, Event::Crash { .. }))
        .expect("the re-fault must terminate the guest");
    assert!(ti > si, "the terminal crash follows the sigreturn");
    let crash_pc = match &events[ti] { Event::Crash { pc, .. } => *pc, _ => unreachable!() };
    assert_eq!(*resume_pc, crash_pc,
        "sigreturn resumed AT the faulting instruction, so the second fault is the same store");

    // (4) replay is byte-identical, twice.
    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 139, "replay {i}; stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged");
    }
}

#[test]
fn the_protection_fault_is_a_seekable_landmark() {
    // "Rewind to the moment the protected page was touched" is the reverse-debugging payoff of
    // enforcing PROT_NONE at all. M12 made delivery seekable; this proves M13's fault reaches it.
    let (_, trace) = util::record_dynamic(retrace_guest::PROTRUST);
    let (events, _) = retrace_trace::Reader::open_checked(&trace).unwrap();
    let di = events.iter().position(|e| matches!(e, Event::SignalDelivery { .. })).unwrap();
    let s = retrace_core::seek(&trace, di, 0).expect("seek to the protection-fault landmark");
    assert_eq!(s.landmark(), di);
}
