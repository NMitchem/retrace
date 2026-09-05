use retrace_box::*;
// Drive the file-I/O guest to its first read() and confirm forward_and_diff captures the
// file bytes as writes and returns the byte count.
#[test]
fn forward_and_diff_captures_read_bytes() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FILEIO).unwrap());
    let mut b = Box_::load(&loaded);
    // Advance to the read() syscall (open, then fstat, then read).
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_READ => {
                let (ret, _err, writes) = b.forward_and_diff(num, args);
                assert_eq!(ret, 19, "read should return the 19 fixture bytes");
                // the write must land at the read buffer (args[1]) and contain the fixture
                let w = writes.iter().find(|w| w.ipa == args[1]).expect("no write at read buf");
                assert!(w.bytes.starts_with(b"retrace-m1-fixture\n"));
                return;
            }
            Stop::Syscall { num, args } => { let (ret, _err, _) = b.forward_and_diff(num, args); b.set_x0_and_return(ret); }
            Stop::Other { esr } => panic!("unexpected exit esr=0x{esr:x}"),
            Stop::Fault { pc, esr, far } => panic!("guest crashed pc=0x{pc:x} esr=0x{esr:x} far=0x{far:x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}

// M26: the diff window must cover what the kernel ACTUALLY wrote, not a fixed 64 KiB.
//
// `forward_and_diff` snapshots a pre-image window per pointer arg, capped at `PTR_WINDOW_CAP`,
// and diffs that same window afterwards. The "Debt #1" clamp just below it bounds the forwarded
// read COUNT by the buffer's backing (`avail`), not by the window — so the host kernel may
// legitimately write far more than the window covers. Everything past it is written into guest
// memory on record and never recorded, and replay then restores stale bytes there.
//
// Nothing in the divergence oracle can see this: `(num, args)` are identical on both sides, so
// the recording is self-consistent and simply incomplete. M25 hit it as CPython reading an 88 KB
// `.pyc` and failing to unmarshal it ("bad marshal data (unknown type code)").
#[test]
fn forward_and_diff_captures_a_read_larger_than_the_window() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::BIGREAD).unwrap());
    let mut b = Box_::load(&loaded);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_READ => {
                let (ret, err, writes) = b.forward_and_diff(num, args);
                assert!(!err, "the fixture read should succeed, got err with ret={ret}");
                assert_eq!(ret, 0x18000, "the fixture is 96 KiB and should read whole");
                // Every byte the kernel reported writing must be covered by some recorded write.
                // Walk the buffer span and require coverage of the LAST byte specifically: it sits
                // 32 KiB past a 64 KiB window, so a truncated capture misses exactly it.
                let last = args[1] + ret - 1;
                let covered = writes.iter().any(|w| {
                    w.ipa <= last && last < w.ipa + w.bytes.len() as u64
                });
                let captured: usize = writes.iter().map(|w| w.bytes.len()).sum();
                assert!(covered,
                    "read returned {ret} bytes into {:#x}..={last:#x} but no recorded write covers \
                     the final byte; captured {captured} bytes across {} write(s) — the tail past \
                     PTR_WINDOW_CAP was dropped, so replay will restore stale bytes there",
                    args[1], writes.len());
                return;
            }
            Stop::Syscall { num, args } => { let (ret, _err, _) = b.forward_and_diff(num, args); b.set_x0_and_return(ret); }
            Stop::Other { esr } => panic!("unexpected exit esr=0x{esr:x}"),
            Stop::Fault { pc, esr, far } => panic!("guest crashed pc=0x{pc:x} esr=0x{esr:x} far=0x{far:x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}
