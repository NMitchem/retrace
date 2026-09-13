// M35: a FAILING syscall's kernel writes are recorded and replayed.
//
// Until M35 `forward_and_diff` skipped write capture whenever the carry flag was set, on the
// stated assumption that a failed syscall writes nothing. Two fixtures say otherwise, and each
// was measured before its test was written (spec §4):
//
//   failsysctl (M28): sysctl(kern.ostype) into 2 bytes -> ENOMEM, and xnu's sysctl() entry writes
//     *oldlenp back (= 0) on that path. On the pre-M35 tree this guest recorded cleanly and its
//     replay DIVERGED: `ipa 0x100004010 replay=0x02 recorded=0x00`. The oracle saw it only because
//     `oldlen` survives to the final snapshot.
//   failproc (M35): sysctl(kern.proc.all) into exactly one kinfo_proc -> ENOMEM after 648 bytes
//     of data were copied out. The guest prints 8 of them, so a dropped capture is visible as a
//     stdout mismatch too (the bigread shape).
//
// Exit codes alone would not do (CLAUDE.md): a replay that applies nothing exits 0 on any guest
// whose divergence the terminal compare cannot see. So each test asserts on the landmark itself —
// `err: true` AND a captured region covering the bytes the kernel wrote — and only then on the
// replay's exit and output.
mod util;
use retrace_trace::Event;

/// The bytes a captured `writes` set holds for `[ipa, ipa + len)`, if some region covers it.
fn captured(writes: &[retrace_trace::Region], ipa: u64, len: usize) -> Option<Vec<u8>> {
    writes.iter().find_map(|r| {
        let end = r.ipa + r.bytes.len() as u64;
        (r.ipa <= ipa && ipa + len as u64 <= end)
            .then(|| r.bytes[(ipa - r.ipa) as usize..][..len].to_vec())
    })
}

/// The one `sysctl` landmark in `trace`: `(args, err, writes)`.
fn the_sysctl(trace: &std::path::Path) -> ([u64; 8], bool, Vec<retrace_trace::Region>) {
    let (events, torn) = retrace_trace::Reader::open_checked(trace).unwrap();
    assert!(!torn, "the recording must be complete");
    let mut hits = events.iter().filter_map(|e| match e {
        Event::Syscall { num, args, err, writes, .. } if *num == retrace_arch::SYS_SYSCTL =>
            Some((*args, *err, writes.clone())),
        _ => None,
    });
    let hit = hits.next().expect("the guest issues exactly one sysctl");
    assert!(hits.next().is_none(), "the guest issues exactly one sysctl");
    hit
}

#[test]
fn a_failing_sysctl_replays_bit_for_bit() {
    let (rec, trace) = util::record(retrace_guest::FAILSYSCTL);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert_eq!(rec.stdout, [0u8, 0], "the 2-byte buffer stays zero (xnu refuses before copying)");

    let (args, err, writes) = the_sysctl(&trace);
    assert!(err, "the undersized sysctl must FAIL on record");
    // The write M35 makes recordable: *oldlenp (args[3]) written back as 0 on the ENOMEM path.
    assert_eq!(captured(&writes, args[3], 8), Some(0u64.to_le_bytes().to_vec()),
        "the landmark must carry the kernel's write-back of *oldlenp; without it replay keeps \
         the guest's 2 and diverges at ipa {:#x} — the pre-M35 measurement. writes: {}",
        args[3], writes.len());

    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, rec.stdout, "replay stdout diverged from the recording");
}

#[test]
fn a_failing_proc_list_replays_bit_for_bit() {
    let (rec, trace) = util::record(retrace_guest::FAILPROC);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert_eq!(rec.stdout.len(), 8, "the guest writes the record's first 8 bytes");

    let (args, err, writes) = the_sysctl(&trace);
    assert!(err, "kern.proc.all into one record's worth of buffer must FAIL (ENOMEM) on record");
    // The data half: 648 bytes of kinfo_proc copied out BEFORE the failure, and *oldlenp -> 0.
    let data = captured(&writes, args[2], 648)
        .expect("the landmark must carry the 648-byte record the kernel copied out on its way to ENOMEM");
    assert_eq!(&data[..8], &rec.stdout[..],
        "the captured record's first 8 bytes are what the guest printed");
    assert_ne!(data, vec![0u8; 648],
        "a kinfo_proc is never all zero (p_pid, p_comm, ...); an all-zero capture means the window \
         was diffed before the kernel wrote it, which cannot happen — or the bytes are the guest's own");
    assert_eq!(captured(&writes, args[3], 8), Some(0u64.to_le_bytes().to_vec()),
        "*oldlenp is written back as 0 (sysctl_prochandle returns ENOMEM before oldidx advances)");

    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, rec.stdout, "replay printed different record bytes than the recording");
}
