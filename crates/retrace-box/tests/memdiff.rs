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
                let (ret, _ret1, _err, writes) = b.forward_and_diff(num, args);
                assert_eq!(ret, 19, "read should return the 19 fixture bytes");
                // the write must land at the read buffer (args[1]) and contain the fixture
                let w = writes.iter().find(|w| w.ipa == args[1]).expect("no write at read buf");
                assert!(w.bytes.starts_with(b"retrace-m1-fixture\n"));
                return;
            }
            Stop::Syscall { num, args } => { let (ret, _ret1, _err, _) = b.forward_and_diff(num, args); b.set_x0_and_return(ret); }
            Stop::Other { esr } => panic!("unexpected exit esr=0x{esr:x}"),
            Stop::Fault { pc, esr, far } => panic!("guest crashed pc=0x{pc:x} esr=0x{esr:x} far=0x{far:x}"),
            Stop::Unsupported(u) => panic!("{u:?}"),
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
                let (ret, _ret1, err, writes) = b.forward_and_diff(num, args);
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
            Stop::Syscall { num, args } => { let (ret, _ret1, _err, _) = b.forward_and_diff(num, args); b.set_x0_and_return(ret); }
            Stop::Other { esr } => panic!("unexpected exit esr=0x{esr:x}"),
            Stop::Fault { pc, esr, far } => panic!("guest crashed pc=0x{pc:x} esr=0x{esr:x} far=0x{far:x}"),
            Stop::Unsupported(u) => panic!("{u:?}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}

// M52 fast-follow (windiff): what `forward_and_diff` records of a window is the span the call
// changed, rounded out to 8-byte words counted from the window's start, never the whole window.
// No VM: `changed_span` is the pure policy; windiff_e2e pins it on a real recording.
#[test]
fn an_unchanged_window_records_nothing() {
    assert_eq!(Box_::changed_span(&[7u8; 40], &[7u8; 40]), None);
    assert_eq!(Box_::changed_span(&[], &[]), None);
}

#[test]
fn one_changed_byte_records_its_word() {
    let pre = [0u8; 64];
    let mut post = pre;
    post[13] = 1;
    assert_eq!(Box_::changed_span(&pre, &post), Some(8..16));
}

#[test]
fn changes_far_apart_record_one_span_from_the_first_word_to_the_last() {
    let pre = [0u8; 4096];
    let mut post = pre;
    post[3] = 1;
    post[2001] = 1;
    assert_eq!(Box_::changed_span(&pre, &post), Some(0..2008));
}

#[test]
fn a_change_in_the_last_partial_word_is_clamped_to_the_window() {
    let pre = [0u8; 21];
    let mut post = pre;
    post[20] = 9;
    assert_eq!(Box_::changed_span(&pre, &post), Some(16..21));
    let (pre, mut post) = ([0u8; 5], [0u8; 5]);
    post[0] = 1;
    assert_eq!(Box_::changed_span(&pre, &post), Some(0..5));
}

#[test]
fn a_timeval_on_a_long_stack_window_records_sixteen_bytes() {
    // The parking_lot shape: a 16-byte timeval at the start of a window that runs ~24 KB to the end
    // of its backing. The old capture stored all of it.
    let pre = vec![0xa5u8; 24 * 1024];
    let mut post = pre.clone();
    post[..16].copy_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(Box_::changed_span(&pre, &post), Some(0..16));
}
