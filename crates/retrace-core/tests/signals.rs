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
#[should_panic(expected = "kill to a pid other than the guest's own")]
fn killing_another_process_fails_loud_instead_of_signalling_the_host() {
    let dir = std::env::temp_dir().join(format!("retrace-m11-ko-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let trace = dir.join("killother.bin");
    let bytes = std::fs::read(retrace_guest::KILLOTHER).expect("read killother guest");
    let loaded = retrace_guest::parse_macho(&bytes);
    let _ = retrace_core::record(&loaded, &trace);
}
