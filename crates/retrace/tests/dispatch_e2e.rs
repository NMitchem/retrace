//! M18 rung 5: a guest that dispatch_asyncs onto a global concurrent queue.
//!
//! Parked at the Stage-2b wall. See the `#[ignore]` reason for what stops it today. The second test
//! in this file is NOT parked: it is Stage 2a's own gate, and it asserts what Stage 2a changed.

mod util;

#[test]
#[ignore = "M18 Stage 2b not implemented: the guest reaches a mach semaphore nothing can signal. \
            Stage 2a moved this wall — workq_open(367) and workq_kernreturn(368) are now EMULATED \
            in the box (Box_::guest_workq_open / guest_workq_kernreturn), each with a record arm, a \
            replay mirror and a fail-loud guard on the generic forward arm, so the host kernel no \
            longer acts on retrace's own process and the host-worker-thread hazard that produced the \
            recorder's own SIGSEGV is GONE. **Stage 2b Task 2 then moved this wall again.** The old \
            wall was retrace's own REQTHREADS panic ('worker construction is Stage 2b'), parked \
            because the register contract for entering `wqthread` was unmeasured; Task 1 measured \
            it (docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-wqthread-measurements.md) and \
            Task 2 built the worker, so that panic is gone and REQTHREADS now spawns a Runnable \
            thread at the registered wqthread entry. The wall today is one trap further on and is \
            still retrace's OWN deliberate refusal: main reaches the mach semaphore wait and the \
            fail-loud guard on record_box's generic negative-trap forward arm refuses trap -36 by \
            name, because forwarding it would block retrace's own process forever on a semaphore \
            only the guest's worker could signal. Measured behind it (Task 4/t10, two runs, \
            REQTHREADS temporarily stubbed to 0, see \
            docs/superpowers/specs/2026-08-21-retrace-m18-stage2b-measurements.md): the mach_msg2 \
            at pc=0x1804adc34 is NOT specific to this path — it is libsystem_kernel's shared \
            mach_msg2 trampoline, hit 12 times across 10 msgh_ids per run — and the one right after \
            REQTHREADS is semaphore_create (msgh_id 3418), already forward-allowlisted, whose reply \
            mints port name 0x1403. dispatch_semaphore_wait then lowers NOT to __ulock_wait (515 \
            appears nowhere in either trace) but to a raw Mach trap, num=-36 at pc=0x1804adbb0, \
            carrying that same port in args[0] (the name semaphore_wait_trap was attributed then \
            and is now VERIFIED — Task 1 §3a read `mov x16, #-0x24` off libsystem_kernel's own \
            stub; retrace_arch::MACH_SEMAPHORE_WAIT). Before Task 2's guard existed it reached \
            forward_and_diff and blocked FOREVER in retrace's own process, which nothing there will \
            ever signal: both runs hung there and both produced 0 bytes of guest stdout, and the \
            one run whose exit code was captured was killed by the external alarm (142) — the other \
            run's exit code is unmeasured, which that document says in bold rather than reading it \
            as a different outcome. That hang is what the guard now converts into a named assert. \
            So what Stage 2b still owes is the SECOND half: a park/wake seam for the mach \
            semaphore — which cannot reuse M14/M17's `pthread + 0x34` address-equality \
            correlation, since that is specific to __ulock_wait's guest-memory address and this \
            primitive correlates on a port name in retrace's own IPC space — plus the park opcode \
            a running worker issues (measured 0x4, and it must not return). Un-park when the \
            worker actually runs the block and main observes the signal."]
fn a_dispatch_async_guest_records_and_replays() {
    // A REAL body that genuinely fails at the wall — the `stackoverflow_rust_e2e` pattern. Parking
    // is then one attribute, and un-parking is deleting one line rather than writing a test. A
    // body that asserts nothing would be a test that cannot fail, which is what the honest-gate
    // discipline exists to prevent.
    //
    // Record the guest through real dyld, replay it, and replay it again — same harness shape as
    // `thread_rust_e2e.rs`. Today this fails at the wall named in the #[ignore] reason above, which
    // is the primary record of where that wall is: since Stage 2b t2 the record run gets a worker
    // built and stops one trap later, at retrace's own fail-loud refusal to forward the mach
    // semaphore wait, still having written no guest stdout at all — the first assertion below is
    // what catches it. It must stay an assertion about what
    // the guest PRINTED: before Stage 2a the same body failed the same way on a completely
    // different cause (a SIGSEGV taken by retrace itself on a host workqueue worker thread, exit
    // 139), and 139 is also what `crashy_e2e` asserts for an uncaught GUEST fault — which is
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

/// M18 Stage 2a's own gate — NOT ignored, and it asserts the one thing Stage 2a changed.
///
/// Before Stage 2a, `workq_open`/`workq_kernreturn` fell to the generic forward arm and the HOST
/// kernel acted on retrace's own process: it created a real workqueue worker thread inside the
/// recorder, entered it at `start_wqthread` -> `_pthread_wqthread`, and died at address 0. The
/// record run exited 139 from RETRACE's own SIGSEGV, having written no guest stdout at all.
///
/// After Stage 2a both syscalls are emulated in the box, and the run stops at a wall of retrace's
/// own making instead — deterministically, in its own process, on its own terms.
///
/// **Stage 2b Task 2 moved which wall that is, and the assertion moved with it.** It used to name
/// the `REQTHREADS` panic ("worker construction is Stage 2b"); that panic is gone, because
/// `REQTHREADS` now builds the worker. So the run walks one step further, into the mach semaphore
/// wait main blocks on — and stops at the fail-loud guard on `record_box`'s generic negative-trap
/// forward arm, which refuses trap `-36` by name rather than letting it reach `forward_and_diff`
/// and hang retrace's own process forever. The guard's message still proves what this test exists
/// to prove — the workqueue syscalls were emulated rather than forwarded, or the run would have
/// died at 139 long before reaching it — and additionally that the run advanced past `REQTHREADS`,
/// which no longer refuses. It is deliberately NOT evidence that the worker is correctly built:
/// nothing has entered it yet, and `retrace-box`'s `workq_reqthreads_*` tests are what assert the
/// thread table and the entry contract. Task 4 services this trap and moves the assertion once more.
///
/// **The assertion is on the message, not the exit code.** `crashy_e2e` asserts 139 for an uncaught
/// GUEST fault, so an exit code alone cannot tell "retrace SIGSEGV'd" apart from "the guest
/// faulted" — the honest-gate rule this repo learned from `segv_rust_e2e`.
#[test]
fn the_workqueue_syscalls_are_emulated_not_forwarded() {
    let (rec, _trace) = util::record_dynamic(retrace_guest::DISPATCH_DYN);

    assert!(rec.stderr.contains("mach semaphore trap") &&
            rec.stderr.contains("reached the generic forward arm"),
        "the record run must stop at retrace's OWN named semaphore-forward guard, which is only \
         reachable if workq_kernreturn was emulated rather than forwarded; stderr:\n{}", rec.stderr);
    // The pre-2a signature, named so a regression is legible rather than just red. 139 is SIGSEGV;
    // this is a supporting check, not the assertion above.
    assert_ne!(rec.code, 139,
        "exit 139 is the pre-Stage-2a signature: retrace itself took a SIGSEGV on a host workqueue \
         worker thread. stderr:\n{}", rec.stderr);
    // A tripwire, not a discriminator, and it is worth being exact about which: the pre-2a symbol
    // was observed in a CRASH REPORT under ~/Library/Logs/DiagnosticReports, not on stderr — a
    // process killed by SIGSEGV prints nothing — so this would have passed pre-2a too. It stays
    // because a future path that does print it to stderr is a host workqueue thread inside the
    // recorder, which is the one thing this file exists to forbid.
    assert!(!rec.stderr.contains("_pthread_wqthread"),
        "no host workqueue thread may exist inside the recorder; stderr:\n{}", rec.stderr);
}
