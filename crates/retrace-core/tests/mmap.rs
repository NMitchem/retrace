use std::path::PathBuf;
fn record(guest: &str) -> PathBuf {
    let loaded = retrace_guest::parse_macho(&std::fs::read(guest).unwrap());
    let p = std::env::temp_dir().join(format!("retrace-mmap-{}.bin", std::process::id()));
    retrace_core::record(&loaded, &p).expect("record");
    p
}
#[test]
fn mmap_guest_records_and_replays() {
    let trace = record(retrace_guest::MMAPGUEST);
    let r = retrace_core::replay(&trace).expect("replay");
    assert_eq!(r.stdout, vec![0xAB, 0xCD]);
    assert_eq!(r.outcome, retrace_core::Outcome::Exit { code: 0 });
}
