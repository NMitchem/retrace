// M16 Task 1 / spec risk R1. The port->tid map M16 needs is read back OUT of the guest's own
// pthread struct rather than reconstructed from tid, because that is the only rule that covers
// main: retrace writes `GUEST_THREAD_PORT_BASE | tid` for every thread it spawns, but main's
// kport is written by libpthread's `__pthread_main_thread_init`, in userspace, and retrace has
// never read that field back. This test IS the measurement.
mod util;
use retrace_core::ReplaySession;
use std::path::Path;

#[test]
fn every_live_thread_has_a_distinct_readable_kport() {
    let (rec, trace) = util::record_dynamic(retrace_guest::THREADRUST);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    // Advance to a landmark where the child provably exists. THREADRUST's child prints, so once
    // "child ran" is in stdout the spawn is behind us; simplest reliable seek is to run to the end
    // of the trace minus nothing and inspect the table, so instead advance until the table grows.
    let mut s = ReplaySession::open(Path::new(&trace)).unwrap();
    // BOUNDED. `advance()`'s own doc says calling it after `Advance::Exited` is unspecified and
    // callers must not — so an unbounded "wait for the child" loop runs off the end of the trace on
    // any guest that never spawns. Stop on exit, and fail loud naming what we were waiting for.
    loop {
        if s.b_thread_count() >= 2 { break; }
        match s.advance().expect("no divergence on an untampered trace") {
            retrace_core::Advance::Exited(_) =>
                panic!("the recording ended with only {} thread(s): THREADRUST must spawn one, so \
                        either the guest changed or bsdthread_create was not emulated",
                       s.b_thread_count()),
            _ => continue,
        }
    }

    let main_port = s.dbg_kport_of(0).expect("main's pthread must be mapped and readable");
    let child_port = s.dbg_kport_of(1).expect("the child's pthread must be mapped and readable");

    assert_eq!(child_port, 0x0BAD_7001,
        "the child's kport is the one retrace itself wrote in guest_bsdthread_create: \
         GUEST_THREAD_PORT_BASE | tid");
    assert_ne!(main_port, 0,
        "R1: main's kport is libpthread's own write. A zero here means the field is not populated \
         at this point in the run, and M16-port must fall back to recognising 0x0BAD_7000|tid for \
         children and failing loud on anything else. Report the value either way.");
    assert_ne!(main_port, child_port,
        "two threads that share a kport would make port->tid resolution ambiguous");
    eprintln!("R1 MEASURED: main kport = {main_port:#x}, child kport = {child_port:#x}");
}
