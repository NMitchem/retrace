// M30 fix round 1: a write() bigger than the record-side diff window must reach the kernel with the
// guest's own bytes in it — not retrace's guard-band canary.
//
// The M30 canary is written into guest memory just past each argument's diff window and restored
// before the vCPU resumes. That makes it invisible to the guest, and it was assumed to be invisible
// full stop. It is not: the same memory is handed to the host kernel, and a syscall that READS more
// than its diff window through a pointer consumes those bytes as data.
//
// **Why this test asserts on the output bytes and nothing else.** Every weaker signal is clean when
// the bug is present — measured on the reproduction, not assumed:
//
//   record exit=0    replay exit=0    [M30 CANARY] lines: 0    no divergence at any landmark
//
// The recording is self-consistent because the bytes are restored before the guest resumes, so the
// determinism oracle has nothing to compare that differs. `crashy_e2e`'s rule applies with unusual
// force here — an exit-code assertion would pass over the exact defect this test exists for. The
// only witness is what the kernel actually wrote out.
//
// The corrupted region is `buf + 0x10080`, 64 bytes, from x4's band (x4 holds `buf + 128`; see
// `asm/bigwrite.s` for why that register is set deliberately rather than left stale). The failure
// message reports the count and the first offset so a regression names its own shape rather than
// saying only "not all A".
mod util;

const LEN: usize = 0x20000; // 128 KiB — twice PTR_WINDOW_CAP, matching the guest

#[test]
fn a_write_past_the_diff_window_is_not_corrupted_by_the_guard_band_canary() {
    let (rec, trace) = util::record(retrace_guest::BIGWRITE);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert_eq!(rec.stdout.len(), LEN, "the guest writes {LEN} bytes in one call");
    check_all_a(&rec.stdout, "RECORD");

    // Replay re-emits console writes from the recording rather than re-executing them, so this
    // half proves the corruption did not reach the TRACE either — a different question from the
    // record-side one above, and the one a future change that captured the band would break.
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    check_all_a(&rp.stdout, "REPLAY");
    assert_eq!(rp.stdout, rec.stdout, "replay stdout diverged from the recording");
}

fn check_all_a(bytes: &[u8], which: &str) {
    let bad: Vec<usize> = bytes.iter().enumerate()
        .filter(|(_, &b)| b != b'A').map(|(i, _)| i).collect();
    assert!(bad.is_empty(),
        "{which}: {} of {} bytes the guest wrote are not 'A', first at {:#x} (value {:#04x}). \
         The guest's buffer is filled from its own Mach-O image, so every byte the kernel read \
         should be 'A'; a run of exactly 64 corrupt bytes is retrace's own guard-band canary \
         reaching the kernel — see retrace_arch::reads_guest_buffer.",
        bad.len(), bytes.len(), bad[0], bytes[bad[0]]);
}
