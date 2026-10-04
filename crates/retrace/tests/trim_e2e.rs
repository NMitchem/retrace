//! M48 Task 3: V8's aligned-reservation trim (walls.md §1 row 3) through the real record loop.
//! `trim_dyn` maps 0x7c000 bytes, unmaps a 0x10000 head and a tail whose length is not page-aligned,
//! and uses the middle. Before M48 the first `munmap` dropped the whole backing, so the middle V8
//! keeps went with it. Every assertion is on the guest's own output, on the recorder's own words
//! about the faulting address or on the box's state, never on an exit code alone.
mod util;

use retrace_guest::TRIM_DYN;
use retrace_trace::{Event, Reader};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;

/// Native `trim_dyn <mode>`: its stdout and its exit code. A fatal signal is reported as the
/// shell reports it, 128 + the signal, which is what retrace's recorder exits with for a crash (M6).
fn native(mode: &str) -> (Vec<u8>, i32) {
    let out = std::process::Command::new(TRIM_DYN).arg(mode).output().unwrap();
    let code = out.status.code().or_else(|| out.status.signal().map(|s| 128 + s)).expect("an exit code or a signal");
    (out.stdout, code)
}

/// The guest's two trim `munmap`s, found in the trace by their shape: `(landmark, address)` of the
/// head's (length 0x10000) and of the tail's (0x50000 on, length 0x2bb20). The trace is the one
/// place both addresses survive a recorder that stops before it prints the guest's stdout.
/// Landmark indices are the replay session's: the leading snapshot is index 0.
fn trim_munmaps(trace: &Path) -> ((usize, u64), (usize, u64)) {
    let ms: Vec<(usize, u64, u64)> = Reader::open(trace).unwrap().into_iter().enumerate()
        .filter_map(|(i, e)| match e {
            Event::Syscall { num, args, .. } if num == retrace_arch::SYS_MUNMAP => Some((i, args[0], args[1])),
            _ => None,
        }).collect();
    let pair = ms.windows(2).find(|w| w[0].2 == 0x10000 && w[1].2 == 0x2bb20 && w[1].1 == w[0].1 + 0x50000)
        .unwrap_or_else(|| panic!("the trim's two munmaps are not in the trace: {ms:x?}"));
    ((pair[0].0, pair[0].1), (pair[1].0, pair[1].1))
}

#[test]
fn a_trimmed_reservation_keeps_its_middle_and_replays() {
    let (want, code) = native("trim");
    assert_eq!(code, 0, "native trim_dyn trim must pass");
    let (rec, trace) = util::record_dynamic_args(TRIM_DYN, &["trim"]);
    assert_eq!(rec.code, 0, "the trim must record to exit 0 (a middle lost to a whole-backing unmap faults): {}", rec.stderr);
    // The `TRIM` line carries the mapping's address, which differs from native's; the line after
    // it is address-independent and is the proof the middle kept its bytes and took the writes.
    let second = |s: &[u8]| String::from_utf8_lossy(s).lines().nth(1).map(str::to_owned);
    assert!(second(&want).is_some_and(|l| l.starts_with("trim ok sum=")), "native: {:?}", String::from_utf8_lossy(&want));
    assert_eq!(second(&rec.stdout), second(&want), "recorded: {:?}", String::from_utf8_lossy(&rec.stdout));
    for i in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "replay {i} must exit 0: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {i} stdout must be byte-identical");
    }
    let _ = std::fs::remove_file(&trace);
}

// Native takes SIGSEGV on a released page (139). retrace does not reproduce that: the box removes a
// released page's STAGE-2 mapping and leaves its stage-1 identity block ("stage-1 identity block
// stays", `guest_munmap`), so the touch is a stage-2 translation fault straight to EL2, which the
// record loop reports as `describe_stop`'s "non-syscall exit" and a recorder exit of 4, never as a
// `Stop::Fault` and an `Event::Crash` (a stage-1 fault, which crashy takes at 0x4000dead0000). That
// is `munmap`'s behaviour since M2 and is not what Task 3 changes, so these assert on what the
// recorder says about the ADDRESS: the faulting IPA is exactly the released word, and the box
// itself calls it UNMAPPED. When a touch of a released page becomes a recorded crash, this is the
// test to move to the trace's terminal `Crash` and a replayed 139.
#[test]
fn the_trimmed_head_and_the_rounded_tail_are_gone() {
    for mode in ["head", "tail"] {
        let (_, ncode) = native(mode);
        assert_eq!(ncode, 139, "native {mode}: touching a released page is a SIGSEGV");
        let (rec, trace) = util::record_dynamic_args(TRIM_DYN, &[mode]);
        let ((_, head), (_, tail_start)) = trim_munmaps(&trace);
        // `head` touches the first word of the mapping; `tail` its last, which only an end rounded
        // UP to the page releases: the second `munmap` ends at +0x7bb20, mid-page. Were the end
        // rounded down instead, the tail word would stay mapped and this record would exit 0.
        let target = if mode == "head" { head } else { tail_start + 0x2c000 - 8 };
        assert_eq!(rec.code, 4, "{mode}: the touch of a released page stops the recorder: {}", rec.stderr);
        assert!(rec.stderr.contains("RECORD ERROR: non-syscall exit: data abort"), "{mode}: {}", rec.stderr);
        assert!(rec.stderr.contains(&format!("far/ipa={target:#x} (UNMAPPED)")),
            "{mode}: the fault is at the released word {target:#x}: {}", rec.stderr);
        let _ = std::fs::remove_file(&trace);
    }
}

/// The restore-parity guard for split backings. A checkpoint taken between the two `munmap`s holds
/// the head-trimmed backing; the second `munmap` then cuts the restored box's backing, and the
/// result must equal a cold seek that made both cuts on a box it built itself.
#[test]
fn a_seek_across_the_trim_matches_a_cold_seek() {
    let (rec, trace) = util::record_dynamic_args(TRIM_DYN, &["trim"]);
    assert_eq!(rec.code, 0, "record: {}", rec.stderr);
    let ((first, head), (second, tail_start)) = trim_munmaps(&trace);
    assert!(first < second, "the head is trimmed before the tail");

    let between = first + 1; // the head's munmap consumed, the tail's still to come
    let cp = retrace_core::seek(&trace, between, 0).unwrap().checkpoint();
    let warm = {
        let mut s = retrace_core::ReplaySession::from_checkpoint(&trace, &cp).unwrap();
        s.advance_to_landmark(second + 1).unwrap_or_else(|d| panic!("warm: diverged at {}: {}", d.landmark, d.detail));
        (s.current_thread(), s.dbg_regs(), s.dbg_fp_regs(), s.dbg_internal_state(), s.snapshot().1)
    };
    let cold = retrace_core::seek(&trace, second + 1, 0).unwrap();
    // Both cuts landed on the cold box: the head and the rounded tail are unmapped, the middle is
    // not. Without this the comparison below would pass on two boxes that both lost the middle.
    let tail = tail_start + 0x2c000 - 8; // the mapping's last 8 bytes: 0x7c000 on from `head`
    assert!(cold.read_mem(head, 8).is_none() && cold.read_mem(head + 0xc000, 8).is_none(), "the head is released");
    assert!(cold.read_mem(tail_start, 8).is_none() && cold.read_mem(tail, 8).is_none(), "the tail is released to its rounded end");
    assert!(cold.read_mem(head + 0x10000, 8).is_some() && cold.read_mem(tail_start - 8, 8).is_some(), "the middle stays mapped");
    assert_eq!(warm.0, cold.current_thread(), "thread: checkpointed vs cold");
    assert_eq!(warm.1, cold.dbg_regs(), "registers: checkpointed vs cold");
    assert_eq!(warm.2, cold.dbg_fp_regs(), "FP/SIMD: checkpointed vs cold");
    assert_eq!(warm.3, cold.dbg_internal_state(), "internal state: checkpointed vs cold");
    assert!(cold.diff_memory(&warm.4).is_none(), "memory: checkpointed vs cold");
    let _ = std::fs::remove_file(&trace);
}
