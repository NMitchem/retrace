// Record the mmap-a-file guest, delete the fixture, then replay: must reproduce the byte from
// the trace with zero divergence. Proves file-backed mmap is captured, not re-read on replay.
mod util;
#[test]
fn mmap_file_replays_after_delete() {
    let (rec, trace) = util::record(retrace_guest::MMAPFILE);
    assert_eq!(rec.code, 0);
    std::fs::remove_file(retrace_guest::MMAPFILE_FIXTURE).unwrap();
    let rp = util::replay(&trace);
    std::fs::write(retrace_guest::MMAPFILE_FIXTURE, b"MMAPFILE-OK\n").unwrap(); // restore artifact
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, b"M");
}
