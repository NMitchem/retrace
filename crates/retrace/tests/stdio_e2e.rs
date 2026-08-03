// M9. A guest whose console output goes through stdio, i.e. through write_nocancel (397) rather
// than write (4) — the wall `brew jq` hit. Before the fix, 397 was not recognized as a console
// write: it fell through to the generic forward path, so the HOST kernel performed the write to
// retrace's own fd 1 (making a recording LOOK right on a terminal) while the trace captured nothing
// and replay — which never executes a syscall — printed nothing at all.
mod util;

#[test]
fn stdio_console_output_is_mirrored_and_replays() {
    let out = util::assert_rung_records_and_replays(retrace_guest::STDIO_DYN, &[], b"stdio\n");

    // Pin the MECHANISM, not merely the bytes. `assert_rung_records_and_replays` alone would go
    // green again if libc ever routed printf through write(4), quietly dropping coverage of the
    // _nocancel path this test exists for — so assert the recorded console write really is 397.
    let events = retrace_trace::Reader::open(&out.trace).expect("open trace");
    assert!(
        events.iter().any(|e| matches!(e,
            retrace_trace::Event::Syscall { num: 397, args, .. } if args[0] == 1)),
        "expected a recorded write_nocancel(fd=1, …) in the trace — if this fails because the \
         console write is now syscall 4, the _nocancel path is no longer covered here");
}

#[test]
fn a_guest_closing_its_stdout_does_not_close_retraces() {
    // The other half of the console wall, and jq's exact exit shape. The guest's fd 1 IS retrace's
    // own, so a forwarded close(1) really closes it — after which the CLI's own write of the
    // mirrored recording goes nowhere. It fails silently: exit status 0, empty stdout, no error.
    // That is why this asserts on OUTPUT rather than on the close's return value.
    let out = util::assert_rung_records_and_replays(retrace_guest::CLOSEFD_DYN, &[], b"closefd\n");
    assert_eq!(out.stdout, b"closefd\n");

    // Pin the mechanism: the close must be RECORDED (so replay verifies the guest still makes it)
    // and must have succeeded from the guest's point of view without ever reaching the host.
    let events = retrace_trace::Reader::open(&out.trace).expect("open trace");
    assert!(
        events.iter().any(|e| matches!(e,
            retrace_trace::Event::Syscall { num, args, ret: 0, err: false, .. }
                // close (6) or close_nocancel (399) — spelled out rather than reusing
                // retrace_arch::is_console_close so the test pins the numbers independently of the
                // predicate under test, and so it holds whichever variant libc picks.
                if (*num == 6 || *num == 399) && args[0] == 1)),
        "expected a recorded, faked close of fd 1 returning success");
}
