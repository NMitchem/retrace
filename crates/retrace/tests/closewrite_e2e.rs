// M37 fix round 1 (review C1). A console slot closed by the guest is CLOSED on both sides.
//
// The guest writes to fd 1, closes it, writes to it again; then the same on fd 2. The kernel's
// answer to the second write is EBADF, and the guest exits 0 only if BOTH post-close writes are
// EBADF — so the rung helper's exit-0 demand is what carries the two EBADF observations, and its
// record == replay stdout equality, and the recording's stderr, carry the model (the console mirror must not see either
// post-close write on either side).
//
// Before the fix: record's faked console close never touched the fd table, so slot 1 stayed
// `Console(1)` and the post-close write was mirrored (rc 1, stdout `before\nafter1\nerr\nafter2\n`),
// while replay's generic close mirror retired the slot and mirrored nothing — two stdouts, no
// divergence. Now both sides retire the slot through the same `FdTable::close`.
mod util;

#[test]
fn a_write_after_closing_a_console_fd_is_ebadf_on_both_sides() {
    // M51 §3e: fd 2's line is on stderr, apart from fd 1's, so each stream is pinned on its own.
    let (_, stderr) = util::assert_rung_records_and_replays_env(retrace_guest::CLOSEWRITE_DYN, &[], b"before\n", &[]);
    assert_eq!(stderr, "err\n", "fd 2's one write before its close, and nothing after it");
}
