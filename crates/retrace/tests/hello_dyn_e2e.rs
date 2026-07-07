// The headline M2 gate: a normal dynamically-linked C program records and replays with zero
// divergence, dyld having mapped the shared cache itself.
mod util;
// BLOCKED (tracked): dyld boots all the way into EXECUTING shared-cache code, then hits the
// cache's fundamental host-binding wall — the arm64e shared cache is bound to retrace's OWN
// process (its __DATA is retrace's dirtied per-process COW state, and its pointers are PAC-signed
// with retrace's per-process keys, which are EL1-only and unreadable). Demand-paging it into a VM
// with independent PAC keys / a fresh dyld run cannot reproduce that state. Full analysis +
// how-far-dyld-got in .superpowers/sdd/task-9b-report.md. Ignored (not deleted) so it stays as the
// living record of the M2 gate and can be re-run with `--ignored` as the approach evolves.
#[ignore = "blocked on shared-cache host process binding (PAC keys + dirty __DATA); see task-9b-report.md"]
#[test]
fn hello_dyn_records_and_replays() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert_eq!(rec.stdout, b"hi\n");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, b"hi\n");
}
