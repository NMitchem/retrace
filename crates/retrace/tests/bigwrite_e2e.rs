// M30 fix round 1: a write() bigger than the record-side diff window must reach the kernel with the
// guest's own bytes in it — not retrace's guard-band canary.
//
// The M30 canary is written into guest memory just past each argument's diff window and restored
// before the vCPU resumes. That makes it invisible to the guest, and it was assumed to be invisible
// full stop. It is not: the same memory is handed to the host kernel, and a syscall that READS more
// than its diff window through a pointer consumes those bytes as data.
//
// **Why this asserts on the bytes the kernel wrote and nothing else.** Every weaker signal is clean
// while the bug is present — measured on this exact fixture with the fix reverted, not assumed:
//
//   record exit=0    replay exit=0    [M30 CANARY] lines: 0    no divergence at any landmark
//
// The recording is self-consistent because the bytes are restored before the guest resumes, so the
// determinism oracle has nothing to compare that differs. `crashy_e2e`'s rule applies with unusual
// force: an exit-code assertion would pass straight over the defect this test exists for. The only
// witness is what the kernel actually wrote out.
//
// **The guest writes to a FILE for a reason that also cost this test its first version.**
// `retrace_arch::is_console_write` makes fd 0/1/2 mirrored-and-faked in retrace-core — read out of
// guest memory and never forwarded — so a guest writing to stdout never reaches `forward_and_diff`
// at all. The stdout version of this fixture was measured VACUOUS: green with the fix reverted.
//
// The corrupted region is `buf + 0x10080`, 64 bytes, from x4's band (x4 holds `buf + 128`; see
// `asm/bigwrite.s` for why that register is set deliberately rather than left stale). The failure
// message reports the count and the first offset so a regression names its own shape.
mod util;

const LEN: usize = 0x20000; // 128 KiB — twice PTR_WINDOW_CAP, matching the guest

#[test]
fn a_write_past_the_diff_window_is_not_corrupted_by_the_guard_band_canary() {
    let outfile = std::path::Path::new(retrace_guest::BIGWRITE_OUT);
    // Removed first so a stale file from an earlier run cannot be mistaken for this one's output —
    // the guest opens with O_TRUNC, but only if it ran at all.
    let _ = std::fs::remove_file(outfile);

    let (rec, trace) = util::record(retrace_guest::BIGWRITE);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);

    let written = std::fs::read(outfile).expect("the guest must have created its output file");
    assert_eq!(written.len(), LEN, "the guest writes {LEN} bytes in one call");
    let bad: Vec<usize> = written.iter().enumerate()
        .filter(|(_, &b)| b != b'A').map(|(i, _)| i).collect();
    assert!(bad.is_empty(),
        "{} of {} bytes the kernel wrote out are not 'A', first at {:#x} (value {:#04x}). The \
         guest's buffer is filled from its own Mach-O image, so every byte the kernel read should \
         be 'A'; a run of exactly 64 corrupt bytes is retrace's own guard-band canary reaching the \
         kernel — see retrace_arch::reads_guest_buffer.",
        bad.len(), written.len(), bad[0], written[bad[0]]);

    // Replay never re-executes the write, so this half asserts something different from the bytes
    // above: that the recording of a call whose band was deliberately NOT filled still replays
    // clean, including the terminal full-memory compare over the 128 KiB buffer.
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
}
