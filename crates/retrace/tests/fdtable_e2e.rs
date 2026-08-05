// M10 gate. The guest's file descriptors are ITS OWN — the determinism property made observable.
//
// The fixture prints INVARIANTS rather than absolute descriptor numbers. An earlier version of this
// test hardcoded `first=3`, matching what the same C program prints natively, and it failed under
// retrace with `first=4`. That was not an fd-table defect: measured from the trace, libsystem opens
// a socket before main() under retrace (there is no real notifyd/bootstrap to reach) and never
// closes it, so the guest legitimately holds one more descriptor than a native process does. The
// absolute number therefore tests libSystem's pre-main behaviour, not the table. These invariants
// test the table, and are verified to hold natively too.
mod util;

/// Every invariant true. Identical natively and under retrace — verified in both.
const EXPECT: &[u8] = b"low=1\ndupnext=1\nebadf=1\ndupread=1\nreuse=1\n";

#[test]
fn guest_sees_its_own_fd_numbers_and_ebadf_after_close() {
    let out = util::assert_rung_records_and_replays(retrace_guest::FDTABLE_DYN, &[], EXPECT);
    // Restated individually so a failure names WHICH invariant broke rather than showing a diff.
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("low=1"),
        "the guest's descriptor must be its OWN small number. Pre-M10 the first open returned 17 — \
         retrace's descriptor number, because retrace holds 0-16 open. Got:\n{s}");
    assert!(s.contains("dupnext=1"),
        "dup must take the next lowest free guest slot. Got:\n{s}");
    assert!(s.contains("ebadf=1"),
        "reading a closed fd must give EBADF. Pre-M10 retrace did not model a closed fd at all, so \
         this read reached whatever retrace had open at that number and could SUCCEED. Got:\n{s}");
    assert!(s.contains("reuse=1"),
        "a fresh open must reuse the just-closed slot (POSIX lowest-not-currently-open). Got:\n{s}");
}

/// The trace must carry GUEST descriptors, not host ones. This is the property that makes a
/// recording a function of the guest rather than of the recorder's own open files.
#[test]
fn the_trace_records_guest_fds_not_host_fds() {
    let out = util::assert_rung_records_and_replays(retrace_guest::FDTABLE_DYN, &[], EXPECT);
    let events = retrace_trace::Reader::open(&out.trace).unwrap();
    let mut saw = 0usize;
    for e in events.iter() {
        if let retrace_trace::Event::Syscall { num, ret, err, .. } = e {
            if !*err && retrace_arch::allocates_fd(*num) {
                saw += 1;
                assert!(*ret >= 3 && *ret < 16,
                    "syscall {num} recorded fd {ret}: an fd >= 16 is a HOST descriptor leaking into \
                     the trace, which is what M10's table exists to prevent");
            }
        }
    }
    assert!(saw >= 2, "expected at least the open and the dup to be recorded, saw {saw}");
}
