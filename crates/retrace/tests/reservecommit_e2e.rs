// M2-mmapcommit Task 1. A freestanding guest reserves a PROT_NONE region via
// _kernelrpc_mach_vm_map_trap (svc -15, cur_protection=0), then first-touches two DIFFERENT pages
// well inside it. Each first touch faults (the reservation is bookkeeping-only, unbacked) and must
// be demand-committed by commit_reserved_page with a fresh zeroed anon page — proving
// reserve -> fault -> zero-fill commit -> store -> load on record, then byte-identical on replay
// (including the final full-memory comparison over the committed pages).
mod util;
#[test]
fn reserve_first_touch_commits_and_replays() {
    let (rec, trace) = util::record(retrace_guest::RESERVECOMMIT);
    assert_eq!(rec.code, 0, "record failed (fatal data abort => committer not wired): {}", rec.stderr);
    assert_eq!(rec.stdout, b"\xAB\xCD", "guest must read back both sentinels stored into committed pages");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, rec.stdout, "replay stdout must match record stdout byte-for-byte");
}
