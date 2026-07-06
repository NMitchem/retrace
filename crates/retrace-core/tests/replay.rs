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
    assert_eq!(r.exit_code, 0);
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
