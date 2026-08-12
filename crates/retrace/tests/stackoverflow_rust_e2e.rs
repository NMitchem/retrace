// PARKED at M8 spec risk R3. This is the gate that comes green when retrace's backed stack and the
// stack libpthread reports agree.
//
// It is committed as real, compiling code with a real guest behind it rather than as a comment,
// because honest-gate discipline means the wall is documented where someone hits it — and because a
// gate that cannot be run cannot be un-parked by simply deleting an attribute.
mod util;

#[test]
#[ignore = "M8 spec risk R3: libstd computes its guard page at pthread_get_stackaddr_np() - \
            pthread_get_stacksize_np(), and macOS 26's libpthread reports a CONSTANT 0x7fc000 that \
            retrace cannot influence (M8 measured that a different getrlimit(RLIMIT_STACK) answer \
            leaves the computed address bit-identical). With DYN_STACK_SIZE = 256 KiB the guard \
            lands at 0x2004000, which is 7.73 MiB BELOW the real stack bottom 0x27C0000, so this \
            recursion runs off the stack into unbacked IPA and takes a STAGE-2 fault — a fatal \
            describe_stop, not a guest-visible signal — instead of striking the guard. Both fixes \
            are already measured and rejected in crates/retrace-box/src/lib.rs:35-53: backing a \
            full 8 MiB cost ~1.7x on hello_rust and far worse across the dyld suite, and getrlimit \
            cannot move the subtrahend. MEASURED, not assumed — forced with --ignored, this run \
            dies exactly where R3 says it will: 'RECORD ERROR: non-syscall exit: data abort \
            (EC=0x24 ISS=0x1c08047 FSC=0x7) far/ipa=0x27bff60 (UNMAPPED)', a stage-2 translation \
            fault 160 bytes below the stack bottom, nowhere near the 0x2004000 guard. UN-IGNORE \
            when R3 is fixed; M13 verified the enforcement mechanism itself is correct via \
            protnone_rust_e2e, which observed that very guard page being installed at 0x2004000 \
            and then protected a different page to prove enforcement."]
fn a_rust_stack_overflow_strikes_its_own_guard_page() {
    let (rec, trace) = util::record_dynamic(retrace_guest::OVERFLOW);
    let err = &rec.stderr; // RunOut::stderr is already a String
    assert!(err.contains("has overflowed its stack"),
        "libstd's handler must recognize the fault as a stack overflow by comparing si_addr against \
         its own guard range; stderr:\n{err}");
    assert_eq!(rec.code, 134, "134 == 128 + SIGABRT: libstd aborts after printing");
    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 134, "replay {i}");
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged");
    }
}
