use std::path::PathBuf;
fn record_hello() -> PathBuf {
    let bytes = std::fs::read(retrace_guest::HELLO).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    let p = std::env::temp_dir().join(format!("retrace-replay-{}.bin", std::process::id()));
    retrace_core::record(&loaded, &p).expect("record");
    p
}
#[test]
fn replay_reproduces_recording_with_zero_divergence() {
    let trace = record_hello();
    let r = retrace_core::replay(&trace).expect("replay must not diverge");
    assert_eq!(r.stdout, b"hello\n");
    assert_eq!(r.outcome, retrace_core::Outcome::Exit { code: 0 });
}
#[test]
fn tampered_syscall_arg_is_caught_as_divergence() {
    let trace = record_hello();
    // Flip the recorded write() fd so the replayed guest's args no longer match.
    let mut events = retrace_trace::Reader::open(&trace).unwrap();
    for e in events.iter_mut() {
        if let retrace_trace::Event::Syscall { args, .. } = e { args[0] = 99; }
    }
    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);
    let err = retrace_core::replay(&trace).unwrap_err();
    assert!(err.detail.contains("syscall"), "divergence should name the mismatch: {}", err.detail);
}

#[test]
fn empty_trace_is_a_named_divergence_not_a_panic() {
    // A trace truncated to zero bytes (the leading Snapshot is lost) must fail by name,
    // never panic — this is the hardening the seeded swarm depends on.
    let trace = std::env::temp_dir().join(format!("retrace-empty-{}.bin", std::process::id()));
    std::fs::write(&trace, b"").unwrap();
    let err = retrace_core::replay(&trace).unwrap_err();
    assert!(err.detail.contains("empty/torn"), "empty trace should name the failure: {}", err.detail);
}

#[test]
fn missing_trace_is_a_named_divergence_not_a_panic() {
    let trace = std::env::temp_dir().join(format!("retrace-missing-{}.bin", std::process::id()));
    let _ = std::fs::remove_file(&trace);
    let err = retrace_core::replay(&trace).unwrap_err();
    assert!(err.detail.contains("cannot open trace"), "missing trace should name the failure: {}", err.detail);
}

fn record_fileio() -> PathBuf {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FILEIO).unwrap());
    let p = std::env::temp_dir().join(format!("retrace-replay-fileio-{}.bin", std::process::id()));
    retrace_core::record(&loaded, &p).expect("record");
    p
}
#[test]
fn fileio_replays_identically_even_after_fixture_deleted() {
    let trace = record_fileio();
    // Delete the fixture the guest read: replay must still reproduce it from the trace.
    let _ = std::fs::remove_file(retrace_guest::FIXTURE);
    let result = retrace_core::replay(&trace);
    // Restore the fixture for other tests in the same binary BEFORE asserting, so a failed
    // assertion here can't leave the fixture deleted and poison later runs.
    std::fs::write(retrace_guest::FIXTURE, b"retrace-m1-fixture\n").unwrap();
    let r = result.expect("replay must not diverge");
    assert_eq!(r.stdout, b"retrace-m1-fixture\n");
    assert_eq!(r.outcome, retrace_core::Outcome::Exit { code: 0 });
}
#[test]
fn tampered_read_write_is_caught_by_final_memory() {
    let trace = record_fileio();
    // Corrupt a recorded read()'s writes so replay's buffer diverges from the recorded final memory.
    let mut events = retrace_trace::Reader::open(&trace).unwrap();
    for e in events.iter_mut() {
        if let retrace_trace::Event::Syscall { num, writes, .. } = e {
            if *num == 3 { if let Some(w) = writes.first_mut() { w.bytes[0] ^= 0xff; } }
        }
    }
    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);
    let err = retrace_core::replay(&trace).unwrap_err();
    assert!(err.detail.contains("memory divergence") || err.detail.contains("syscall"),
        "expected a named divergence, got: {}", err.detail);
}

// ---- M12 t8: the replay mirror for signal delivery.
//
// Symmetry rule 1 in its sharpest form: replay recomputes the frame through the SAME
// deliver_signal and byte-compares it against the recording before advancing, so an asymmetry
// between the two sides surfaces as a loud divergence rather than as silent corruption.

fn record_guest(path: &str, tag: &str) -> PathBuf {
    let loaded = retrace_guest::parse_macho(&std::fs::read(path).unwrap());
    let p = std::env::temp_dir().join(format!("retrace-m12-rep-{tag}-{}.bin", std::process::id()));
    retrace_core::record(&loaded, &p).expect("record");
    p
}

// The FAULT path: a guest that catches SIGSEGV, repairs its own pc, and runs on.
#[test]
fn a_caught_fault_replays_bit_for_bit() {
    let trace = record_guest(retrace_guest::SEGVCATCH, "segvcatch");
    let r = retrace_core::replay(&trace).expect("replay must not diverge");
    assert_eq!(r.outcome, retrace_core::Outcome::Exit { code: 0 },
        "the handler ran on replay too — a Crash here means the mirror skipped it");
    assert_eq!(r.stdout, b"caught\nresumed\n",
        "both the handler's write and the resumed guest's write must reappear");
    std::fs::remove_file(&trace).ok();
}

// The RAISE path, which the fault path cannot stand in for: delivery happens at a syscall
// boundary, so the mirror must complete the syscall exactly as record did before rebuilding the
// frame. Omit that and the recomputed frame differs from the recording in x0 and PSTATE.C.
#[test]
fn a_caught_self_raise_replays_bit_for_bit() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::SIGFRAME).unwrap());
    let p = std::env::temp_dir().join(format!("retrace-m12-rep-raise-{}.bin", std::process::id()));
    let rec = retrace_core::record(&loaded, &p).expect("record");
    let rep = retrace_core::replay(&p).expect("replay must not diverge");
    // Compared against the RECORDING rather than against a literal, so this keeps asking the
    // right question if the fixture's own checks change.
    assert_eq!(rep.outcome, rec.outcome, "replay must reach the same outcome it recorded");
    assert_eq!(rep.stdout, rec.stdout);
    std::fs::remove_file(&p).ok();
}

// The mirror is load-bearing, and this proves it rather than asserting it: corrupt one byte of the
// recorded frame and replay's recomputation must refuse to match it.
#[test]
fn a_tampered_signal_frame_is_caught_as_divergence() {
    let trace = record_guest(retrace_guest::SEGVCATCH, "tamper");
    let mut events = retrace_trace::Reader::open(&trace).unwrap();
    let d = events.iter_mut()
        .find(|e| matches!(e, retrace_trace::Event::SignalDelivery { .. }))
        .expect("a recorded delivery");
    if let retrace_trace::Event::SignalDelivery { writes, .. } = d { writes[0].bytes[0] ^= 0xff; }
    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);

    let err = retrace_core::replay(&trace).unwrap_err();
    assert!(err.detail.contains("signal frame mismatch"), "detail: {}", err.detail);
    std::fs::remove_file(&trace).ok();
}

// ---- M15 t4 fix round 1: the thread oracle's two call sites OUTSIDE the generic Event::Syscall
// match at ReplaySession::advance's bottom — the caught-raise mirror and the sigreturn mirror both
// consume a recorded Event::Syscall landmark and RETURN before ever reaching that generic match, so
// each needed its own call to `verify_thread` (see its doc comment). SIGFRAME reaches BOTH in one
// recording: the self-raise takes the caught-raise arm, and the handler's return takes the
// sigreturn arm.
//
// No guest that is both threaded AND signals itself exists yet (a follow-up, not this fix round —
// M15's headline guest takes no signals, and the signal guests are single-threaded), so there is no
// second LIVE thread id here to retag to the way `thread_oracle.rs`'s THREADRUST-based test does
// for the generic arm. These use the SAME idiom as `tampered_syscall_arg_is_caught_as_divergence`
// above instead — an arbitrary different value (`+= 1`), not one drawn from a second real schedule.
// That proves the comparison FIRES at these two call sites; it does not prove it distinguishes
// between two genuinely live schedules there, which is a narrower claim than the main M15 t4 test
// makes for the generic arm.
#[test]
fn a_tampered_thread_on_the_caught_raise_landmark_is_a_divergence() {
    let trace = record_guest(retrace_guest::SIGFRAME, "raise-thread-tamper");
    let mut events = retrace_trace::Reader::open(&trace).unwrap();
    let di = events.iter().position(|e| matches!(e, retrace_trace::Event::SignalDelivery { .. }))
        .expect("a delivery");
    // Record appends the ordinary (caught-raise) Syscall, THEN the delivery, at ONE stop (see the
    // record-side Disposition::Handler raise arm) — di - 1 is unambiguously that Syscall.
    match &mut events[di - 1] {
        retrace_trace::Event::Syscall { thread, .. } => *thread += 1,
        other => panic!("expected the caught-raise Syscall landmark just before the delivery, got {other:?}"),
    }
    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);

    let err = retrace_core::replay(&trace).unwrap_err();
    assert!(err.detail.contains("schedule diverged"),
        "expected the thread oracle at the caught-raise arm to fire: {}", err.detail);
    std::fs::remove_file(&trace).ok();
}

#[test]
fn a_tampered_thread_on_the_sigreturn_landmark_is_a_divergence() {
    let trace = record_guest(retrace_guest::SIGFRAME, "sigreturn-thread-tamper");
    let mut events = retrace_trace::Reader::open(&trace).unwrap();
    let si = events.iter().position(|e| matches!(e,
        retrace_trace::Event::Syscall { num, .. } if *num == retrace_arch::SYS_SIGRETURN))
        .expect("a sigreturn — the handler must have RETURNED, not aborted");
    match &mut events[si] {
        retrace_trace::Event::Syscall { thread, .. } => *thread += 1,
        other => panic!("expected the sigreturn Syscall landmark, got {other:?}"),
    }
    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);

    let err = retrace_core::replay(&trace).unwrap_err();
    assert!(err.detail.contains("schedule diverged"),
        "expected the thread oracle at the sigreturn arm to fire: {}", err.detail);
    std::fs::remove_file(&trace).ok();
}

// A caught raise is TWO landmarks written at ONE stop, so the coordinate between them names a
// position the guest never occupies. A seek there must SAY so rather than silently landing past
// it: the terminal Exit/Signal pairs report through their own path, and this is the first mid-run
// pair the trace format can contain.
#[test]
fn seeking_between_a_caught_raise_and_its_delivery_is_a_named_error() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::SIGFRAME).unwrap());
    let p = std::env::temp_dir().join(format!("retrace-m12-pair-{}.bin", std::process::id()));
    retrace_core::record(&loaded, &p).expect("record");
    let events = retrace_trace::Reader::open(&p).unwrap();
    let d = events.iter().position(|e| matches!(e, retrace_trace::Event::SignalDelivery { .. }))
        .expect("a delivery");

    let err = retrace_core::seek(&p, d, 0).unwrap_err();
    assert!(err.contains("not a resumable position"), "got: {err}");
    // And the landmark after the pair still seeks, so the guard refuses one coordinate rather
    // than breaking seeking past a delivery altogether.
    retrace_core::seek(&p, d + 1, 0).expect("the landmark after the pair must remain reachable");
    std::fs::remove_file(&p).ok();
}
