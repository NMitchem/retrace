// M11: the record-side signal contract, exercised through the freestanding asm guests.
// Task 7 adds the CLI-level e2e gates; these are the in-process record assertions.
use retrace_core::Outcome;

#[test]
fn a_self_raised_sigabrt_is_a_recorded_terminal_signal_not_a_dead_recorder() {
    let dir = std::env::temp_dir().join(format!("retrace-m11-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let trace = dir.join("raise.bin");
    let bytes = std::fs::read(retrace_guest::RAISE).expect("read raise guest");
    let loaded = retrace_guest::parse_macho(&bytes);
    let s = retrace_core::record(&loaded, &trace).expect("record must SUCCEED — the whole point");
    match s.outcome {
        Outcome::Signal { sig } => assert_eq!(sig, 6, "SIGABRT"),
        other => panic!("expected Outcome::Signal, got {other:?}"),
    }
    // The terminal pair: Signal, then the final full-memory snapshot.
    let (events, torn) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(!torn, "a complete recording must not be torn");
    assert!(matches!(events[events.len() - 2], retrace_trace::Event::Signal { sig: 6, .. }),
            "second-to-last event must be Signal, got {:?}", events[events.len() - 2]);
    assert!(matches!(events[events.len() - 1], retrace_trace::Event::Snapshot { .. }),
            "last event must be the final memory snapshot");
    std::fs::remove_file(&trace).ok();
}

#[test]
fn an_ignored_signal_does_not_terminate_the_guest() {
    let dir = std::env::temp_dir().join(format!("retrace-m11-ign-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let trace = dir.join("sigign.bin");
    let bytes = std::fs::read(retrace_guest::SIGIGN).expect("read sigign guest");
    let loaded = retrace_guest::parse_macho(&bytes);
    let s = retrace_core::record(&loaded, &trace).expect("record");
    match s.outcome {
        Outcome::Exit { code } => assert_eq!(code, 0, "the guest ran PAST the ignored raise"),
        other => panic!("SIG_IGN must not terminate; got {other:?}"),
    }
    assert_eq!(s.stdout, b"ok\n", "the guest kept running and produced output");
    std::fs::remove_file(&trace).ok();
}

#[test]
fn a_recorded_signal_replays_identically_twice() {
    let dir = std::env::temp_dir().join(format!("retrace-m11-rep-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let trace = dir.join("raise-replay.bin");
    let bytes = std::fs::read(retrace_guest::RAISE).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    let rec = retrace_core::record(&loaded, &trace).expect("record");
    for i in 0..2 {
        let rep = retrace_core::replay(&trace)
            .unwrap_or_else(|d| panic!("replay {i} diverged at landmark {}: {}", d.landmark, d.detail));
        match rep.outcome {
            Outcome::Signal { sig } => assert_eq!(sig, 6),
            other => panic!("replay {i}: expected Outcome::Signal, got {other:?}"),
        }
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged");
    }
    std::fs::remove_file(&trace).ok();
}

#[test]
fn the_sigign_guest_replays_bit_for_bit() {
    let dir = std::env::temp_dir().join(format!("retrace-m11-ignrep-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let trace = dir.join("sigign-replay.bin");
    let bytes = std::fs::read(retrace_guest::SIGIGN).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    let rec = retrace_core::record(&loaded, &trace).expect("record");
    let rep = retrace_core::replay(&trace)
        .unwrap_or_else(|d| panic!("diverged at landmark {}: {}", d.landmark, d.detail));
    assert_eq!(rep.stdout, rec.stdout);
    assert_eq!(rep.stdout, b"ok\n");
    std::fs::remove_file(&trace).ok();
}

// The second oracle: two RECORDINGS byte-compared. Freestanding guests — no clock, no entropy, no
// libmalloc, no mach ports — so the usual preconditions hold (see util::assert_trace_reproducible).
//
// These live here, IN-PROCESS, rather than as CLI-level e2e tests, and that is load-bearing rather
// than incidental. Both signal guests call getpid(20), which M11 deliberately does NOT intercept
// (the raise arm's self-pid check depends on it forwarding), so the recorder's own pid is recorded
// as that syscall's return value. Two recordings made by two SEPARATE processes therefore cannot be
// byte-identical by construction — measured: the traces differ in exactly one record, the CRC and
// body of the num=20 event. Recording twice in ONE process holds the pid constant and asks the
// question the oracle is actually for: did anything ELSE nondeterministic enter the trace?
#[test]
fn two_recordings_of_the_sigign_guest_are_byte_identical() {
    let dir = std::env::temp_dir().join(format!("retrace-m11-idet-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bytes = std::fs::read(retrace_guest::SIGIGN).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    let (t1, t2) = (dir.join("i1.bin"), dir.join("i2.bin"));
    retrace_core::record(&loaded, &t1).expect("record 1");
    retrace_core::record(&loaded, &t2).expect("record 2");
    assert_eq!(std::fs::read(&t1).unwrap(), std::fs::read(&t2).unwrap(),
               "a nondeterministic value entered the trace (sigaction servicing is the new code \
                path here — it must not introduce one)");
    std::fs::remove_file(&t1).ok();
    std::fs::remove_file(&t2).ok();
}

#[test]
fn two_recordings_of_the_raise_guest_are_byte_identical() {
    let dir = std::env::temp_dir().join(format!("retrace-m11-det-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bytes = std::fs::read(retrace_guest::RAISE).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    let (t1, t2) = (dir.join("d1.bin"), dir.join("d2.bin"));
    retrace_core::record(&loaded, &t1).expect("record 1");
    retrace_core::record(&loaded, &t2).expect("record 2");
    assert_eq!(std::fs::read(&t1).unwrap(), std::fs::read(&t2).unwrap(),
               "a nondeterministic value entered the trace");
    std::fs::remove_file(&t1).ok();
    std::fs::remove_file(&t2).ok();
}

/// M50 §3h: was `#[should_panic]` on the same text. A kill to another pid is a wall in `signals`,
/// which ends the recording with its line, and still never signals a host process.
#[test]
fn killing_another_process_is_a_wall_instead_of_signalling_the_host() {
    let dir = std::env::temp_dir().join(format!("retrace-m11-ko-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let trace = dir.join("killother.bin");
    let bytes = std::fs::read(retrace_guest::KILLOTHER).expect("read killother guest");
    let loaded = retrace_guest::parse_macho(&bytes);
    let s = retrace_core::record(&loaded, &trace).expect("a wall ends the recording; it is not an I/O error");
    let Outcome::Refused(u) = &s.outcome else { panic!("expected a wall, got {:?}", s.outcome) };
    assert_eq!(u.category, retrace_arch::WallCategory::Signals);
    assert!(u.what.starts_with("kill to a pid other than the guest's own"), "{}", u.what);
}

// ---- M12: delivery. The dispositions above decide WHETHER a handler runs; these decide that it
// actually does, and that it can return.

/// Record a freestanding asm guest IN-PROCESS (the pattern every test in this file uses — there is
/// no shared helper; `crates/retrace/tests/util` is for the CLI-level gates).
fn record_asm(guest: &str) -> (retrace_core::Outcome, std::path::PathBuf) {
    let bytes = std::fs::read(guest).expect("read guest");
    let loaded = retrace_guest::parse_macho(&bytes);
    let name = guest.rsplit('/').next().unwrap();
    let p = std::env::temp_dir().join(format!("retrace-m12-{}-{name}.bin", std::process::id()));
    let s = retrace_core::record(&loaded, &p).expect("record must SUCCEED");
    (s.outcome, p)
}

// A fault with a handler installed must DELIVER, not crash. This is the live wrong answer M12
// exists to fix: Stop::Fault never consulted sigtable, so the handler was silently skipped.
#[test]
fn a_fault_with_a_handler_installed_delivers_instead_of_crashing() {
    let (outcome, trace) = record_asm(retrace_guest::SEGVCATCH);
    let (events, torn) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(!torn, "the recording must be complete");
    let deliveries: Vec<_> = events.iter()
        .filter(|e| matches!(e, retrace_trace::Event::SignalDelivery { .. })).collect();
    assert_eq!(deliveries.len(), 1, "exactly one delivery; events:\n{events:#?}");
    let retrace_trace::Event::SignalDelivery { sig, si_code, handler, .. } = deliveries[0] else {
        unreachable!()
    };
    assert_eq!(*sig, 11, "a store to an unmapped address is SIGSEGV");
    assert_eq!(*si_code, retrace_arch::SEGV_ACCERR, "a translation fault: native's SEGV_ACCERR (M49 R1)");
    assert_ne!(*handler, 0);
    assert!(!events.iter().any(|e| matches!(e, retrace_trace::Event::Crash { .. })),
        "the handler ran, so this is NOT a crash");
    assert_eq!(outcome, retrace_core::Outcome::Exit { code: 0 },
        "segvcatch repairs the fault and exits 0 — a Crash outcome here means the handler was skipped");
    std::fs::remove_file(&trace).ok();
}

#[test]
fn an_uncaught_fault_is_still_a_crash() {
    // The M6 regression. No handler installed => the Event::Crash path is untouched.
    //
    // CRASH, not WILDSTORE. Both store to a bad address, but only CRASH's has bit 46 set, so only
    // CRASH takes the STAGE-1 fault that reaches Stop::Fault directly. WILDSTORE is a stage-2 abort,
    // which M49 F2 converts into a Stop::Fault on the next run(); its own test is below.
    let (outcome, trace) = record_asm(retrace_guest::CRASH);
    assert!(matches!(outcome, retrace_core::Outcome::Crash { .. }), "got {outcome:?}");
    let (events, _) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(events.iter().any(|e| matches!(e, retrace_trace::Event::Crash { .. })),
        "an uncaught fault must still record as Crash, not a delivery");
    assert!(!events.iter().any(|e| matches!(e, retrace_trace::Event::SignalDelivery { .. })));
    std::fs::remove_file(&trace).ok();
}

// M49 F2: WILDSTORE's store to 0xB_0000_0000 is stage-1-mapped and stage-2-unbacked. Before M49
// it ended the recording with a recorder error; it is now the stage-1 fault native takes, recorded
// as a crash and replayed as one.
#[test]
fn a_wild_store_below_the_ipa_ceiling_records_and_replays_as_a_crash() {
    let (outcome, trace) = record_asm(retrace_guest::WILDSTORE);
    let Outcome::Crash { esr, far, .. } = outcome else { panic!("expected a crash, got {outcome:?}") };
    assert_eq!((esr, far), (0x9200_0047, 0xB_0000_0000), "a level-3 translation fault, a store, at the wild address");
    let (events, torn) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(!torn, "the recording must be complete");
    assert!(matches!(events[events.len() - 2], retrace_trace::Event::Crash { thread: 0, .. }));
    assert!(matches!(events[events.len() - 1], retrace_trace::Event::Snapshot { .. }));
    for _ in 0..2 {
        let rep = retrace_core::replay(&trace).expect("the crash replays");
        assert_eq!(rep.outcome, outcome);
    }
    std::fs::remove_file(&trace).ok();
}

#[test]
fn sigreturn_is_recorded_as_an_ordinary_syscall_between_delivery_and_resumption() {
    let (_, trace) = record_asm(retrace_guest::SEGVCATCH);
    let (events, _) = retrace_trace::Reader::open_checked(&trace).unwrap();
    let di = events.iter().position(|e| matches!(e, retrace_trace::Event::SignalDelivery { .. }))
        .expect("a delivery");
    let si = events.iter().position(|e| matches!(e,
        retrace_trace::Event::Syscall { num, .. } if *num == retrace_arch::SYS_SIGRETURN))
        .expect("a sigreturn — the handler must have RETURNED, not aborted");
    assert!(si > di, "sigreturn comes after the delivery it returns from");
    std::fs::remove_file(&trace).ok();
}

// The end-to-end counterpart of deliver.rs's frame assertions. A guest that raises a signal on
// ITSELF must resume from its own SUCCESSFUL kill(), not from a failure retrace invented: the
// kernel delivers such a signal with the syscall's return already applied (measured — see
// spikes/sigraisex0.c). Decoded from the RECORDED frame rather than from the fixture's own checks,
// so it keeps holding if those checks change.
#[test]
fn a_caught_self_raise_records_the_syscalls_success_in_the_frame() {
    let (_, trace) = record_asm(retrace_guest::SIGFRAME);
    let (events, _) = retrace_trace::Reader::open_checked(&trace).unwrap();
    let d = events.iter().find(|e| matches!(e, retrace_trace::Event::SignalDelivery { .. }))
        .expect("a delivery");
    let retrace_trace::Event::SignalDelivery { writes, .. } = d else { unreachable!() };
    let o = retrace_box::FRAME_MCONTEXT_OFF + 16; // __ss within mcontext64; __x[0] at its offset 0
    let f = &writes[0].bytes;
    let x0 = u64::from_le_bytes(f[o..o + 8].try_into().unwrap());
    let cpsr = u32::from_le_bytes(f[o + 264..o + 268].try_into().unwrap()) as u64;
    assert_eq!(x0, 0, "kill() returned 0: the frame carries the RETURN, not the pid argument");
    assert_eq!(cpsr & retrace_arch::PSTATE_C, 0,
        "a successful raise clears PSTATE.C; a stale carry reads as a failed kill() to the guest");
    std::fs::remove_file(&trace).ok();
}

/// M50 §3h: was `#[should_panic]`. The expected text says "synchronously", which appears ONLY in the
/// fault arm's message: the raise arm has carried its own "raising blocked signal" refusal since M11,
/// so matching the shorter prefix would pass even if the fault arm never gained a blocked check.
#[test]
fn a_blocked_synchronous_fault_is_a_wall_rather_than_a_guess() {
    // POSIX leaves this undefined and Darwin force-delivers. Guessing would be a plausible lie.
    let (outcome, _) = record_asm(retrace_guest::BLOCKEDFAULT);
    assert!(matches!(&outcome, Outcome::Refused(u) if u.category == retrace_arch::WallCategory::Signals
        && u.what.starts_with("raising blocked signal 11 synchronously")), "{outcome:?}");
}
