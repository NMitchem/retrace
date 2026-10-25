// M52 fast-follow (windiff): a forwarded call's recorded write is the span of its diff window that
// changed, rounded out to 8-byte words, never the whole window. `tvloop_dyn` makes 1000
// `gettimeofday` calls with the timeval on the stack. Before the fix each one recorded the rest of
// the stack's backing, 18 KB here, for the 16 bytes the kernel wrote, which is how parking_lot's
// queue test filled the disk (M52 Ruling T8-pl-limit). The assertion is on the trace, because the
// program's output is the same either way.
mod util;

use retrace_trace::Event;

/// What all the 116 landmarks together may record. Measured after the fix: 1020 calls, 999 of them recording
/// only `tv_usec` (8 bytes), 18 the whole timeval, and 3 the timeval twice (a second probed register's window
/// reached it): 8 376 bytes. Before it, the first call's window alone was 18 288 bytes, and one call
/// recorded 36 592 across its windows.
const MOST_IN_ALL: usize = 64 * 1024;

#[test]
fn a_gettimeofday_records_the_bytes_it_changed_not_its_window() {
    let native = util::run_bounded(retrace_guest::TVLOOP_DYN, &[], &[], 60);
    assert_eq!(native.code, Some(0), "native exit: {}", native.stderr);
    assert_eq!(String::from_utf8_lossy(&native.stdout), "tvloop 1000\n");

    let (rec, trace) = util::record_dynamic(retrace_guest::TVLOOP_DYN);
    assert_eq!(rec.code, 0, "record: {}", rec.stderr);
    assert_eq!(rec.stdout, native.stdout, "record's stdout");
    for i in 0..2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "replay {i}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {i}'s stdout");
    }

    let (mut calls, mut total) = (0usize, 0usize);
    for e in retrace_trace::Reader::open(&trace).unwrap() {
        let Event::Syscall { num, args, err, writes, .. } = e else { continue };
        if num != retrace_arch::SYS_GETTIMEOFDAY || err { continue; }
        calls += 1;
        total += writes.iter().map(|w| w.bytes.len()).sum::<usize>();
        // The kernel writes the timeval and nothing else here (the timezone and mach-time pointers
        // are NULL), so a recorded byte far from it is the window, not the call. A span is rounded
        // to words counted from its own window's start, and a stale register's window may start at
        // any byte, so a region may reach up to 7 bytes past either end of the timeval.
        for w in &writes {
            assert!(args[0] <= w.ipa + 7 && w.ipa + w.bytes.len() as u64 <= args[0] + 16 + 7,
                "landmark {calls}: a recorded write at {:#x}+{} lies beyond the timeval at {:#x}",
                w.ipa, w.bytes.len(), args[0]);
        }
    }
    assert!(calls >= 1000, "every call reaches 116 under the frozen commpage; saw {calls}");
    assert!(total <= MOST_IN_ALL, "the 116 landmarks recorded {total} bytes in all, more than {MOST_IN_ALL}");
}
