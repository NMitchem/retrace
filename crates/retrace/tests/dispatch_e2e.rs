//! M18 rung 5: a guest that dispatch_asyncs onto a global concurrent queue.
//!
//! Parked at the Stage-2 wall. See the `#[ignore]` reason for what stops it today.

mod util;

#[test]
#[ignore = "M18 Stage 2 not implemented: workq_open(367) and workq_kernreturn(368) are still \
            FORWARDED, and forwarding them is whole-process fatal for the recorder. Stage 1's wall \
            is gone — t5 stopped forwarding bsdthread_register, so _pthread_workqueue_supported now \
            returns true and libdispatch brings its workqueue up rather than BRKing at .cold.1. \
            Measured (Task 6 Step 1, RETRACE_TRACE=1, see stage2-measurements.md): the guest now \
            reaches num=368 args[0]=0x400 (dispatch setup), num=367, num=368 args[0]=0x20 (request \
            threads) — the first time either syscall has ever fired — and then dies at the mach_msg2 \
            (num=-47) at pc=0x1804adc34. It is NOT the guest that dies: neither dispatch loop has an \
            arm for 367/368, so both reach the generic forward arm and the HOST kernel acts on \
            retrace's own process, creating a real workqueue worker thread inside the recorder. The \
            crash report shows the faulting thread is start_wqthread -> _pthread_wqthread jumping to \
            address 0x0 (EXC_BAD_ACCESS at 0). exit(139) here is that SIGSEGV, NOT Outcome::Crash — \
            no 'guest crashed' line is printed and the guest's stdout is 0 bytes. Because a real \
            host thread races the vCPU thread, the dispatched-trap count is not even stable: three \
            identical runs measured 252, 253 and 254. Un-park when 367/368 are emulated below the \
            trace and the guest reaches its worker."]
fn a_dispatch_async_guest_records_and_replays() {
    // A REAL body that genuinely fails at the wall — the `stackoverflow_rust_e2e` pattern. Parking
    // is then one attribute, and un-parking is deleting one line rather than writing a test. A
    // body that asserts nothing would be a test that cannot fail, which is what the honest-gate
    // discipline exists to prevent.
    //
    // Record the guest through real dyld, replay it, and replay it again — same harness shape as
    // `thread_rust_e2e.rs`. Today this fails at the wall named in the #[ignore] reason above:
    // record dies with a SIGSEGV taken by RETRACE ITSELF on a host workqueue worker thread, so it
    // exits 139 having written no guest stdout at all — the first assertion below is what catches
    // it. Note 139 is the same code `crashy_e2e` asserts for an uncaught GUEST fault, which is
    // exactly why this test must never assert on the exit code alone.
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
