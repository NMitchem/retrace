// M38 gate. pipe's SECOND descriptor reaches the guest: the trace carries a guest-numbered write
// end in `ret1` (before M38 `x1` was never captured and the guest saw its own stale register),
// both ends are adjacent guest numbers, and bytes round-trip. Asserts on the trace and the bytes,
// never on an exit code alone (CLAUDE.md's first gate rule).
mod util;

const EXPECT_STDOUT: &[u8] = b"pair=1\nlow=1\nbytes=pipe\n";

#[test]
fn both_ends_reach_the_guest_and_bytes_round_trip() {
    let out = util::assert_rung_records_and_replays(retrace_guest::PIPE_DYN, &[], EXPECT_STDOUT);
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("pair=1"), "the write end must be the slot after the read end. Got:\n{s}");
    assert!(s.contains("bytes=pipe"), "bytes written into p[1] must come back out of p[0]. Got:\n{s}");
}

#[test]
fn the_trace_carries_a_guest_numbered_write_end_in_ret1() {
    let out = util::assert_rung_records_and_replays(retrace_guest::PIPE_DYN, &[], EXPECT_STDOUT);
    let events = retrace_trace::Reader::open(&out.trace).unwrap();
    let mut pairs = Vec::new();
    let mut others_with_ret1 = 0usize;
    for e in events.iter() {
        if let retrace_trace::Event::Syscall { num, ret, ret1, err, writes, .. } = e {
            if *num == 42 {
                assert!(writes.is_empty(), "pipe writes no guest memory");
                pairs.push((*ret, *ret1, *err));
            } else if *ret1 != 0 {
                others_with_ret1 += 1;
            }
        }
    }
    // The difference M38 makes: a write end that EXISTS in the trace, as a guest number.
    assert_eq!(pairs.len(), 1, "expected exactly one pipe landmark, saw {pairs:?}");
    let (r, w, err) = pairs[0];
    assert!(!err, "pipe must succeed: {pairs:?}");
    assert!(w == r + 1 && r >= 3 && w < 16,
        "pipe returned ({r}, {w}): both must be adjacent GUEST numbers (a host descriptor is >= 16, \
         a stale x1 is arbitrary)");
    // Narrow capture (spec R2): no other row carries a non-zero ret1.
    assert_eq!(others_with_ret1, 0, "ret1 must be 0 on every non-pipe landmark");
}

// The mirror's compare, verified able to fail (the dup2_e2e tamper pattern): a passing replay
// proves nothing about the compare on its own.
#[test]
fn a_tampered_pipe_write_end_is_caught_as_divergence() {
    let out = util::assert_rung_records_and_replays(retrace_guest::PIPE_DYN, &[], EXPECT_STDOUT);
    let mut events = retrace_trace::Reader::open(&out.trace).unwrap();
    let mut tampered = false;
    for e in events.iter_mut() {
        if let retrace_trace::Event::Syscall { num, ret1, err, .. } = e {
            if *num == 42 && !*err && !tampered { *ret1 = 99; tampered = true; }
        }
    }
    assert!(tampered, "no successful pipe landmark found to tamper");
    let mut w = retrace_trace::Writer::create(&out.trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);
    let rep = util::replay(&out.trace);
    assert_ne!(rep.code, 0, "replay must reject a write end the guest's own table cannot produce. stdout:\n{}",
        String::from_utf8_lossy(&rep.stdout));
    assert!(rep.stderr.contains("fd divergence") && rep.stderr.contains("pair"),
        "the divergence must name the pair mismatch, got stderr:\n{}", rep.stderr);
}
