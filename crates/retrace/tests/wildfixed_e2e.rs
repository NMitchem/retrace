// M8-stack fast-follow. End-to-end cover for a MAP_FIXED request the guest's address space cannot
// hold: it must come back to the GUEST as a failed syscall (carry set, x0 = EINVAL) and be recorded
// and replayed like any other syscall result.
//
// The regression this pins is a RECORDER abort, not a guest bug. M8-stack Task 5 taught the box to
// honor MAP_FIXED -- correct, and required -- which meant a wild address stopped being quietly
// bump-allocated somewhere harmless and instead reached `hv_vm_map`. HVF rejects it with
// HV_BAD_ARGUMENT, retrace `expect`ed on that, and the whole recording process died with exit 101:
// no HVF fault, no guest error text, nothing to debug from. A guest asking for the impossible must
// get an error back; only retrace's OWN invariants may fail loud.
mod util;

fn u64s(stdout: &[u8]) -> Vec<u64> {
    assert_eq!(stdout.len(), 24, "guest must publish exactly three u64s, got {} bytes", stdout.len());
    stdout.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect()
}

const EINVAL: u64 = 22;

#[test]
fn a_wild_map_fixed_is_an_errno_to_the_guest_not_a_recorder_abort() {
    let (rec, _t) = util::record(retrace_guest::WILDFIXED);
    assert_eq!(rec.code, 0,
        "the recorder did not survive a wild MAP_FIXED (exit {}): {}", rec.code, rec.stderr);
    let f = u64s(&rec.stdout);

    assert_eq!(f[0], 1, "the guest must see the mmap FAIL (carry set); got carry={}", f[0]);
    assert_eq!(f[1], EINVAL,
        "a MAP_FIXED address outside the guest's address space must return EINVAL, got {}", f[1]);
    assert_eq!(f[2], 0x5a,
        "the guest must run on past the rejected mmap and still get working anonymous memory");
}

// The failed syscall is an ordinary recorded event: replay must reproduce the same carry and errno
// without executing anything, and the run must stay bit-identical.
#[test]
fn wildfixed_replays_bit_for_bit() {
    let (rec, trace) = util::record(retrace_guest::WILDFIXED);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, rec.stdout, "replay stdout must match record stdout byte-for-byte");
}
