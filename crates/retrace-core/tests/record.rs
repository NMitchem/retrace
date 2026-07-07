#[test]
fn recording_fileio_emits_fixture_and_logs_writes() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FILEIO).unwrap());
    let trace = std::env::temp_dir().join(format!("retrace-rec-fileio-{}.bin", std::process::id()));
    let s = retrace_core::record(&loaded, &trace).expect("record");
    assert_eq!(s.stdout, b"retrace-m1-fixture\n");
    let events = retrace_trace::Reader::open(&trace).unwrap();
    // A read() syscall must carry a non-empty `writes` (the file bytes).
    assert!(events.iter().any(|e| matches!(e,
        retrace_trace::Event::Syscall { num, writes, .. } if *num == 3 && !writes.is_empty())));
    // Trace ends Exit then a final Snapshot landmark.
    assert!(matches!(events[events.len()-2], retrace_trace::Event::Exit { .. }));
    assert!(matches!(events.last(), Some(retrace_trace::Event::Snapshot { .. })));
}
