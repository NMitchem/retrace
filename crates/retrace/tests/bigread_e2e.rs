// M26: a read() bigger than the record-side diff window must still replay byte-for-byte.
//
// `forward_and_diff` snapshots a pre-image window per pointer arg and diffs that same window
// afterwards. Until M26 the window was a flat `PTR_WINDOW_CAP` (64 KiB) while the forwarded read
// COUNT was clamped only by the destination's backing, so a read returning more than 64 KiB wrote
// its tail into guest memory on record and into no `Event` — and replay restored stale bytes there.
//
// The failure is LATENTLY silent, and the distinction is worth stating precisely because the
// obvious framing is wrong. The per-landmark oracle genuinely cannot see it: `(num, args)` are
// byte-identical on both sides, so the recording is self-consistent and merely incomplete. But the
// repo has a SECOND backstop — the terminal full-memory compare (`Box_::diff_memory`, called by all
// three terminal replay arms) — and stale bytes trip it at exit unless the guest either acts on
// them first or drops their backing (a read into an mmap that is then munmap'd would evade it
// entirely; no gate does that today).
//
// So the two known instances failed in two different places, and neither was ever a passing green:
// CPython (M25) branched on the bad data and diverged at a SYSCALL landmark ~560 calls in, while
// this guest ignores what it read and is therefore caught by the terminal compare. Measured by
// mutation, reverting only `diff_window`'s widening:
//   DIVERGENCE at landmark 6 pc=0x1000003f4: memory divergence at ipa 0x100014000:
//   replay=0x00 recorded=0x41
// 0x100014000 is buf+0x10000 — the FIRST byte past the old 64 KiB window, and 'A' is the fixture.
// That is the assertion on line 31 (`rp.code`), not the stdout assertion below it.
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
    // Pins the OBSERVABLE consequence, below the terminal compare that fires first: replay restored
    // only the first 64 KiB of the read, so without the fix the byte at +0x17fff is whatever the
    // guest's zeroed data page held rather than 'Z'. Kept even though the divergence assertion
    // above catches the mutation earlier, because a future change that weakened the terminal
    // compare would otherwise leave this class unguarded.
    assert_eq!(rp.stdout, b"Z",
        "replay did not reproduce the byte 96 KiB into the read buffer — the record-side diff \
         window truncated the capture");
    assert_eq!(rp.stdout, rec.stdout, "replay stdout diverged from the recording");
}
