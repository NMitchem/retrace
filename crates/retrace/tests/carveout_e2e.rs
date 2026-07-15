// M2-carveout Task 1 e2e. A freestanding guest reserves a PROT_NONE band, punches an interior hole
// with mach_vm_deallocate, then commits ANYWHERE with hint = reservation base. Kernel-faithful
// placement must force the commit into the carveout hole (base + 0x10000), not honor the raw hint —
// libmalloc's guarded-metadata protocol in miniature. The guest asserts the placement in-guest and
// exits 0 iff the map landed in the hole and the sentinel round-trips; record then replay must be
// byte-identical (the replay oracle byte-checks the returned address). Run under --test-threads=1.
mod util;
#[test]
fn carveout_hole_placement_records_and_replays() {
    let (rec, trace) = util::record(retrace_guest::CARVEOUT);
    assert_eq!(rec.code, 0,
        "record failed (map did not land in the carveout hole => hint-forward first-fit missing): {}", rec.stderr);
    assert_eq!(rec.stdout, b"\xAB", "guest must read back the sentinel stored through the hole address");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, rec.stdout, "replay stdout must match record stdout byte-for-byte");
}
