use std::path::PathBuf;
fn record_hello() -> PathBuf {
    let bytes = std::fs::read(retrace_guest::HELLO).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    let p = std::env::temp_dir().join(format!("retrace-replay-{}.bin", std::process::id()));
    retrace_core::record(&loaded, &p);
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
