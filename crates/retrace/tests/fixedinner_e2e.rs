// M8-stack. A MAP_FIXED mmap that lands WHOLLY INSIDE a larger existing backing must zero only the
// requested pages; the rest of that backing must stay mapped WITH ITS CONTENTS.
//
// This is the regression cover for MAP_FIXED containment-reuse. The enclosing backing used to be
// dropped wholesale, destroying live guest memory the real kernel would have kept. That bug was
// invisible to the determinism oracle: record and replay destroyed the same bytes, so no divergence
// fired and retrace simply recorded a WRONG execution faithfully.
//
// It is not hypothetical. libstd's `install_main_guard` mmaps MAP_FIXED at
// `usrstack64 - RLIMIT_STACK`, wholly inside the dynamic stack backing -- region B below is that
// shape, and the wholesale drop would have unmapped the guest's own running stack. The same is true
// of every other single-backing region: loaded image segments, the L1/L2 page tables, the PAC sign
// stub/table.
mod util;

fn u64s(stdout: &[u8]) -> Vec<u64> {
    assert_eq!(stdout.len(), 80, "guest must publish exactly ten u64s, got {} bytes", stdout.len());
    stdout.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect()
}

#[test]
fn interior_map_fixed_reuses_the_backing_and_keeps_the_surrounding_bytes() {
    let (rec, _t) = util::record(retrace_guest::FIXEDINNER);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let f = u64s(&rec.stdout);
    let (ret_a, base_a) = (f[0], f[1]);

    assert_eq!(ret_a, base_a + 0x4000,
        "an interior MAP_FIXED must land at the requested address ({ret_a:#x})");
    assert_eq!(f[2], 0x11,
        "the sentinel BELOW the punch lost its contents (got {:#x}, want 0x11) -- the enclosing \
         backing was dropped wholesale instead of reused in place", f[2]);
    assert_eq!(f[3], 0x00, "the punched range must be zeroed like a fresh anon mapping, got {:#x}", f[3]);
    assert_eq!(f[4], 0x33,
        "the sentinel ABOVE the punch lost its contents (got {:#x}, want 0x33)", f[4]);
    assert_eq!(f[5], 0x44,
        "the second sentinel above the punch lost its contents (got {:#x}, want 0x44)", f[5]);
}

// The libstd `install_main_guard` shape: MAP_FIXED at the very base of an existing region, so there
// is no head remnant and everything above the guard page must survive.
#[test]
fn base_map_fixed_keeps_everything_above_the_guard_page() {
    let (rec, _t) = util::record(retrace_guest::FIXEDINNER);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let f = u64s(&rec.stdout);
    let (ret_b, base_b) = (f[6], f[7]);

    assert_eq!(ret_b, base_b, "a base-aligned MAP_FIXED must return the requested address");
    assert_eq!(f[8], 0x22,
        "the region above the guard page lost its contents (got {:#x}, want 0x22) -- this is the \
         shape that would unmap the guest's own running stack", f[8]);
    assert_eq!(f[9], 0x44,
        "the region above the guard page lost its contents (got {:#x}, want 0x44)", f[9]);
}

#[test]
fn fixedinner_replays_bit_for_bit() {
    let (rec, trace) = util::record(retrace_guest::FIXEDINNER);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, rec.stdout, "replay stdout must match record stdout byte-for-byte");
}
