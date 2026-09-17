// M38 gate. AT_FDCWD in the form real guests pass. Asserts BOTH halves on the trace: that the
// guest passed the 32-bit form (0xfffffffe — so this test cannot go green on a fixture that
// happened to sign-extend) and that the call succeeded.
mod util;

#[test]
fn a_relative_fstatat_through_at_fdcwd_succeeds() {
    let out = util::assert_rung_records_and_replays(retrace_guest::ATFDCWD_DYN, &[], b"ok\n");
    let events = retrace_trace::Reader::open(&out.trace).unwrap();
    let seen: Vec<(u64, u64, bool)> = events.iter().filter_map(|e| match e {
        retrace_trace::Event::Syscall { num, args, ret, err, .. } if *num == retrace_arch::SYS_FSTATAT64 =>
            Some((args[0], *ret, *err)),
        _ => None,
    }).collect();
    assert!(!seen.is_empty(), "expected an fstatat64 landmark");
    assert!(seen.iter().any(|(dirfd, _, _)| *dirfd == 0xffff_fffe),
        "the guest must pass AT_FDCWD as the 32-bit form 0xfffffffe (the form the ABI delivers); saw {seen:?}");
    assert!(seen.iter().all(|(dirfd, _, err)| *dirfd != 0xffff_fffe || !*err),
        "fstatat64(AT_FDCWD, ...) must succeed — EBADF here is the M10-t3 sentinel bug: {seen:?}");
}
