// M26: a read() bigger than the record-side diff window must still replay byte-for-byte.
//
// `forward_and_diff` snapshots a pre-image window per pointer arg and diffs that same window
// afterwards. Until M26 the window was a flat `PTR_WINDOW_CAP` (64 KiB) while the forwarded read
// COUNT was clamped only by the destination's backing, so a read returning more than 64 KiB wrote
// its tail into guest memory on record and into no `Event` — and replay restored stale bytes there.
//
// The failure this guards is SILENT, which is the whole reason it is an e2e and not just a unit
// test. `(num, args)` are byte-identical on both sides, so the divergence oracle has nothing to
// complain about; the recording is self-consistent and merely incomplete. The only visible symptom
// is that the guest reads the wrong bytes back. M25 met it as CPython failing to unmarshal an 88 KB
// `.pyc` ("bad marshal data (unknown type code)") 560 landmarks into a run that recorded cleanly.
//
// The fixture is deleted between record and replay, exactly as `mmap_file_replays_after_delete`
// does: that is what proves the final byte came out of the TRACE rather than off the disk.
mod util;

#[test]
fn a_read_past_the_diff_window_replays_its_tail() {
    let (rec, trace) = util::record(retrace_guest::BIGREAD);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    // The guest emits only the LAST byte of a 96 KiB read — 32 KiB beyond a 64 KiB window.
    assert_eq!(rec.stdout, b"Z", "record read the wrong tail byte");

    let fixture = retrace_guest::BIGREAD_FIXTURE;
    let saved = std::fs::read(fixture).unwrap();
    std::fs::remove_file(fixture).unwrap();
    let rp = util::replay(&trace);
    std::fs::write(fixture, &saved).unwrap(); // restore the build artifact

    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    // Before M26 this was the assertion that failed while everything else stayed green: replay
    // restored only the first 64 KiB of the read, so the byte at +0x17fff was whatever the guest's
    // zeroed data page already held rather than 'Z'.
    assert_eq!(rp.stdout, b"Z",
        "replay did not reproduce the byte 96 KiB into the read buffer — the record-side diff \
         window truncated the capture");
    assert_eq!(rp.stdout, rec.stdout, "replay stdout diverged from the recording");
}
