#[test]
fn recording_hello_emits_hello_and_logs_events() {
    let bytes = std::fs::read(retrace_guest::HELLO).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    let dir = std::env::temp_dir();
    let trace = dir.join(format!("retrace-rec-{}.bin", std::process::id()));
    let s = retrace_core::record(&loaded, &trace).expect("record");
    assert_eq!(s.stdout, b"hello\n");
    assert_eq!(s.exit_code, 0);
    // Trace must contain: 1 Snapshot, 1 write Syscall, 1 Exit.
    let events = retrace_trace::Reader::open(&trace).unwrap();
    assert!(matches!(events.first(), Some(retrace_trace::Event::Snapshot{..})));
    assert!(events.iter().any(|e| matches!(e, retrace_trace::Event::Syscall{num,..} if *num==4)));
    assert!(matches!(events.last(), Some(retrace_trace::Event::Exit{code:0})));
}
