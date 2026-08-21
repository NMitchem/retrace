//! M18 rung 5: a guest that dispatch_asyncs onto a global concurrent queue.
//!
//! Parked at the Stage-1 wall. See the `#[ignore]` reason for what stops it today.

mod util;

#[test]
#[ignore = "M18 Stage 1: libdispatch dies before any workqueue syscall fires. \
            _dispatch_root_queues_init_once calls _pthread_workqueue_supported, which traps at \
            .cold.1 because __pthread_supported_features is 0 — libpthread stores that word only \
            when bsdthread_register returns >= 1, and retrace forwards that call to its own \
            already-registered host process. Measured (Step 5, RETRACE_TRACE=1): \
            bsdthread_register args=[0x1804ecc14, 0x1804ecc08, 0x4000, 0x27fc0e0, 0x38, 0xa0, \
            0x4f00000000, 0x0] returns ret=0x16 err=true — EINVAL, confirming the forwarded call \
            genuinely fails rather than merely returning < 1. The guest then dies: BRK (EC=0x3c \
            ISS=0xb001 FSC=0x1) at pc=0x1804f5f20 (_pthread_workqueue_supported.cold.1), 241 \
            dispatched syscalls in (NOT the pre-spec probe's 405 — see task-1-report.md for that \
            discrepancy; the BRK site, EC/ISS/FSC and pc all match the probe exactly), with \
            workq_open/workq_kernreturn never fired. Un-park when the guest reaches its worker."]
fn a_dispatch_async_guest_records_and_replays() {
    // A REAL body that genuinely fails at the wall — the `stackoverflow_rust_e2e` pattern. Parking
    // is then one attribute, and un-parking is deleting one line rather than writing a test. A
    // body that asserts nothing would be a test that cannot fail, which is what the honest-gate
    // discipline exists to prevent.
    //
    // Record the guest through real dyld, replay it, and replay it again — same harness shape as
    // `thread_rust_e2e.rs`. Today this fails at the BRK named in the #[ignore] reason above: record
    // exits 4 with a RECORD ERROR rather than reaching either write.
    let (rec, trace) = util::record_dynamic(retrace_guest::DISPATCH_DYN);
    let out = String::from_utf8_lossy(&rec.stdout);

    assert!(out.contains("worker\n"),
        "THE WORKER BLOCK MUST ACTUALLY RUN on the kernel workqueue libdispatch brings up; \
         stdout:\n{out}\nstderr:\n{}", rec.stderr);
    assert!(out.contains("done\n"),
        "main must observe the semaphore signal and reach its own final write; stdout:\n{out}\n\
         stderr:\n{}", rec.stderr);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    // Replay is byte-identical, twice — the same double-replay shape as thread_rust_e2e, where a
    // nondeterministic schedule would surface as a stdout divergence.
    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 0, "replay {i}; stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged — the schedule is not pure");
    }
}
