// M16 Task 1 / spec risk R1. The port->tid map M16 needs is read back OUT of the guest's own
// pthread struct rather than reconstructed from tid, because that is the only rule that covers
// main: retrace writes `GUEST_THREAD_PORT_BASE | tid` for every thread it spawns, but main's
// kport is written by libpthread's `__pthread_main_thread_init`, in userspace, and retrace has
// never read that field back. This test IS the measurement.
mod util;
use retrace_core::ReplaySession;
use std::path::Path;

/// Advance until the child exists, or fail loud naming what we were waiting for. Shared by both
/// tests in this file: the bound is the point (see the panic), and two copies of it would drift.
fn seek_to_two_threads(s: &mut ReplaySession) {
    loop {
        if s.b_thread_count() >= 2 { return; }
        match s.advance().expect("no divergence on an untampered trace") {
            retrace_core::Advance::Exited(_) =>
                panic!("the recording ended with only {} thread(s): THREADRUST must spawn one, so \
                        either the guest changed or bsdthread_create was not emulated",
                       s.b_thread_count()),
            _ => continue,
        }
    }
}

#[test]
fn every_live_thread_has_a_distinct_readable_kport() {
    let (rec, trace) = util::record_dynamic(retrace_guest::THREADRUST);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    // Advance to a landmark where the child provably exists. THREADRUST's child prints, so once
    // "child ran" is in stdout the spawn is behind us; simplest reliable seek is to run to the end
    // of the trace minus nothing and inspect the table, so instead advance until the table grows.
    let mut s = ReplaySession::open(Path::new(&trace)).unwrap();
    seek_to_two_threads(&mut s);

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

/// M16 Task 4. Resolution is a search over LIVE threads' own kport fields, so it covers main
/// without a special case and cannot silently fall back to "whoever is running" — which is exactly
/// today's latent bug.
#[test]
fn a_port_resolves_to_the_thread_that_owns_it() {
    let (rec, trace) = util::record_dynamic(retrace_guest::THREADRUST);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);
    let mut s = ReplaySession::open(Path::new(&trace)).unwrap();
    seek_to_two_threads(&mut s);

    for tid in [0usize, 1] {
        let port = s.dbg_kport_of(tid).expect("readable");
        assert_eq!(s.dbg_thread_of_port(port), tid,
            "each thread's own port must resolve back to it");
    }
}

/// Fast-follow (the replay-side abort -> `Divergence` change). Port resolution failing is the
/// signature of a SCHEDULE divergence on the replay side, so replay must be able to *observe* the
/// failure rather than die of it. This pins the fallible form's two obligations: it returns `Err`
/// rather than unwinding, and the diagnostic still names every thread it searched — that list is
/// the whole debugging value of the message, and a `Result` refactor is exactly the kind of change
/// that quietly drops it.
///
/// `0xDEAD_BEEF` is safe as a never-issued port precisely because the test above pins what IS
/// issued: children get `GUEST_THREAD_PORT_BASE | tid` (`0x0BAD_7001` for tid 1) and main's comes
/// from libpthread. Neither can collide with this.
#[test]
fn an_unissued_port_is_reported_rather_than_aborted_on() {
    let (rec, trace) = util::record_dynamic(retrace_guest::THREADRUST);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);
    let mut s = ReplaySession::open(Path::new(&trace)).unwrap();
    seek_to_two_threads(&mut s);

    // Sanity: a REAL port still resolves, so a blanket-Err regression cannot pass this test.
    let child = s.dbg_kport_of(1).expect("the child's pthread must be mapped and readable");
    assert_eq!(s.dbg_try_thread_of_port(child), Ok(1),
        "a genuinely issued port must still resolve through the fallible form");

    let err = s.dbg_try_thread_of_port(0xDEAD_BEEF)
        .expect_err("a port no thread owns must not resolve");
    assert!(err.contains("belongs to no live guest thread"),
        "the diagnostic must say what went wrong; got:\n{err}");
    assert!(err.contains("tid=0") && err.contains("tid=1"),
        "the diagnostic must still name EVERY thread searched — that list is what makes it \
         actionable, and it is the first thing a Result refactor drops; got:\n{err}");
}
