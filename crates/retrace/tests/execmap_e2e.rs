// Record + replay a guest that mmaps a small FILE of code PROT_READ|PROT_EXEC and blr's into it.
// Proves runtime exec-mmap promotion installs RO+exec pages for a PROT_EXEC mmap (the guest can
// execute mmap'd code under W^X) AND that replay reproduces executable mmaps with zero divergence.
// The mapped code is `movz x0,#42 ; ret`, so the guest exits with code 42.
mod util;
#[test]
fn exec_mmap_records_and_replays() {
    let (rec, trace) = util::record(retrace_guest::EXECMAP);
    assert_eq!(rec.code, 42, "record failed / wrong exit code: {}", rec.stderr);
    assert!(rec.stdout.is_empty(), "execmap guest writes no console output");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 42, "replay divergence / wrong exit code: {}", rp.stderr);
    assert!(rp.stdout.is_empty(), "execmap replay writes no console output");
}
