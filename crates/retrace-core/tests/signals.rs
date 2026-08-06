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

#[test]
#[should_panic(expected = "kill to a pid other than the guest's own")]
fn killing_another_process_fails_loud_instead_of_signalling_the_host() {
    let dir = std::env::temp_dir().join(format!("retrace-m11-ko-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let trace = dir.join("killother.bin");
    let bytes = std::fs::read(retrace_guest::KILLOTHER).expect("read killother guest");
    let loaded = retrace_guest::parse_macho(&bytes);
    let _ = retrace_core::record(&loaded, &trace);
}
