//! M18 rung 5: a guest that dispatch_asyncs onto a global concurrent queue.
//!
//! Still parked as of Stage 2b Task 4, but no longer parked at a WALL — see the `#[ignore]` reason,
//! which is the primary record of that and says plainly that nothing is left to park it at. Task 5
//! owns un-parking it on its own green. The second test in this file is NOT parked: it is Stage
//! 2a's own gate, narrowed to the one guarantee it exists to keep.

mod util;

#[test]
#[ignore = "M18 Stage 2b Task 5 owns this gate's final state, and this reason is stamped by Task \
            4 to say what is true now rather than what used to be. The wall this reason used to \
            name is GONE. Stage 2a emulated workq_open(367)/workq_kernreturn(368) in the box; Task \
            1 measured the wqthread entry contract \
            (docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-wqthread-measurements.md); Task \
            2 made REQTHREADS build a worker there, plus the fail-loud guard on record_box's \
            generic negative-trap arm that refused the whole -39..=-33 semaphore family by name; \
            Task 3 built the park/wake seam (BlockReason::Sem keyed on the PORT name, since \
            dispatch_semaphore_wait lowers to a raw Mach trap and not to __ulock_wait, so M14/M17's \
            `pthread + 0x34` address correlation has nothing to work on) and the worker's own park \
            at workq_kernreturn opcode 0x4; and Task 4 wired both halves into BOTH dispatch loops, \
            so -36 and -33 are serviced by arms sitting BEFORE that guard instead of reaching it. \
            Task 4 hand-ran this exact guest end to end and the whole thing worked: main blocks in \
            semaphore_wait_trap(-36) on port 0x1503, the box schedules the worker it built, the \
            worker writes 'worker', signals semaphore_signal_trap(-33) on that same port, parks in \
            workq_kernreturn(0x4), main resumes and writes 'done', and the guest exits 0 — replayed \
            twice, byte-identical stdout both times. So there is NOTHING LEFT TO PARK THIS AT, and \
            un-parking it is Task 5's first job. It stays parked here only because that hand-run \
            was a bare CLI record-dyn/replay, not this test: this body spawns the codesigned CLI \
            through util::record_dynamic and has not itself been run green under the workspace \
            gate. A gate must be un-parked on its own green, not on a measurement taken beside it."]
fn a_dispatch_async_guest_records_and_replays() {
    // A REAL body — the `stackoverflow_rust_e2e` pattern. Parking is one attribute and un-parking is
    // deleting one line, rather than writing a test. A body that asserts nothing would be a test
    // that cannot fail, which is what the honest-gate discipline exists to prevent.
    //
    // Record the guest through real dyld, replay it, and replay it again — same harness shape as
    // `thread_rust_e2e.rs`. UNCHANGED by Stage 2b Task 4, deliberately: the assertions below are
    // already exactly the ones the cleared wall calls for, and t4's hand-run of this same guest
    // through the bare CLI satisfied every one of them (see the #[ignore] reason). Task 5 runs this
    // body itself and decides the attribute.
    //
    // The assertions stay about what the guest PRINTED, and must: before Stage 2a the same body
    // failed on a completely different cause (a SIGSEGV taken by retrace ITSELF on a host workqueue
    // worker thread, exit 139), and 139 is also what `crashy_e2e` asserts for an uncaught GUEST
    // fault — which is exactly why this test must never assert on the exit code alone.
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

/// M18 Stage 2a's own gate — NOT ignored, and it keeps exactly ONE guarantee: **no host workqueue
/// thread may ever exist inside the recorder.**
///
/// Before Stage 2a, `workq_open`/`workq_kernreturn` fell to the generic forward arm and the HOST
/// kernel acted on retrace's own process: it created a real workqueue worker thread inside the
/// recorder, entered it at `start_wqthread` -> `_pthread_wqthread`, and died at address 0. The
/// record run exited 139 from RETRACE's own SIGSEGV, having written no guest stdout at all.
///
/// Its assertion has now moved three times, each time onto the sharpest thing then observable —
/// Stage 2a's `REQTHREADS` panic, Stage 2b t2's named `-36` forward guard, and now the worker
/// block's own stdout, because Stage 2b t4 serviced the semaphore pair and the guest stops at no
/// wall at all. Each move was forced by this file's own progress and none of them loosened it: the
/// guarantee is the same one, asserted against better evidence. "worker\n" is better evidence than
/// either predecessor, because a host workqueue thread cannot produce it — it dies at address 0
/// before writing anything — while both earlier signatures were merely things that happened to sit
/// past the fork in the road.
///
/// This test is deliberately NARROWER than the headline gate above and stays that way even once
/// that gate is un-parked: it says nothing about replay, about determinism, or about the guest
/// finishing. It is the tripwire for one specific hazard, and its two supporting checks below are
/// there because they outlive every wall that passes them.
///
/// **It never asserts on the exit code as the discriminator.** `crashy_e2e` asserts 139 for an
/// uncaught GUEST fault, so an exit code alone cannot tell "retrace SIGSEGV'd" apart from "the guest
/// faulted" — the honest-gate rule this repo learned from `segv_rust_e2e`.
#[test]
fn the_workqueue_syscalls_are_emulated_not_forwarded() {
    let (rec, _trace) = util::record_dynamic(retrace_guest::DISPATCH_DYN);

    // Stage 2b Task 4 moved this assertion for the third and last time, onto the thing that is
    // now actually observable: the worker block's OWN stdout. Servicing the semaphore pair let the
    // scheduler reach the worker for the first time, so the guest no longer stops at a wall at all
    // — the guard message this assertion used to name is unreachable for this guest, because the
    // dedicated -36/-33 arms sit before it.
    //
    // "worker\n" on the guest's stdout is the sharpest available evidence of the ONE thing this
    // test exists to prove, and sharper than the guard message ever was. That line can only be
    // written by a thread `Box_::guest_workq_reqthreads` built INSIDE the box and the box's own
    // scheduler ran: a FORWARDED workq_kernreturn instead brings up a host workqueue in retrace's
    // own process, whose worker enters `_pthread_wqthread`, jumps through a NULL dispatch function
    // pointer and dies at address 0 with no guest stdout at all (measured, M18 Task 6 crash
    // report). It is deliberately not an assertion on the whole run — the headline gate above owns
    // that, and owns whether it is parked.
    let out = String::from_utf8_lossy(&rec.stdout);
    assert!(out.contains("worker\n"),
        "the worker block's own stdout is what proves the workqueue syscalls were EMULATED: it can \
         only come from a thread the box built and the box scheduled. Its absence means either a \
         host workqueue thread (which dies at address 0, writing nothing) or a wall short of the \
         worker. stdout:\n{out}\nstderr:\n{}", rec.stderr);
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
