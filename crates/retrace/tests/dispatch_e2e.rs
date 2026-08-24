//! M18 rung 5: a guest that dispatch_asyncs onto a global concurrent queue.
//!
//! **UN-PARKED at the M18 Stage 2b close** (Task 5), on its own green rather than on Task 4's
//! hand-run beside it: this body spawns the codesigned CLI through `util::record_dynamic`, and it
//! was run — `cargo test -p retrace --test dispatch_e2e -- --test-threads=1 --ignored`, ok, exit 0
//! — before the attribute came off. It had been parked since Stage 1 and re-parked twice, each time
//! at a measured wall; there is no wall left. The second test in this file has never been parked: it
//! is Stage 2a's own gate, narrowed to the one guarantee it exists to keep, plus (Stage 2b) the
//! trap census that proves this milestone's seam actually ran.

mod util;

use retrace_trace::Event;

#[test]
fn a_dispatch_async_guest_records_and_replays() {
    // A REAL body — the `stackoverflow_rust_e2e` pattern. Parking was one attribute and un-parking
    // was deleting it, rather than writing a test. A body that asserts nothing would be a test that
    // cannot fail, which is what the honest-gate discipline exists to prevent.
    //
    // Record the guest through real dyld, replay it, and replay it again — same harness shape as
    // `thread_rust_e2e.rs`. The assertions are UNCHANGED across the un-park: they were already
    // exactly the ones the cleared wall called for, so Task 5 ran this body and deleted the
    // attribute without touching a single one of them. Nothing here was loosened to earn the green.
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
/// This test is deliberately NARROWER than the headline gate above in what it says about the RUN —
/// and it stays that way now that gate is un-parked: it says nothing about replay, about
/// determinism, or about the guest finishing. It is the tripwire for one specific hazard, and its
/// two supporting checks below are there because they outlive every wall that passes them.
///
/// It is, however, WIDER in one direction, and Stage 2b widened it deliberately: it reads the
/// **trace** and takes a census of the mach-semaphore pair. See the comment on that census — every
/// other assertion in this file, this test's and the headline gate's alike, is satisfied by a run in
/// which Stage 2b's seam never executes.
///
/// **It never asserts on the exit code as the discriminator.** `crashy_e2e` asserts 139 for an
/// uncaught GUEST fault, so an exit code alone cannot tell "retrace SIGSEGV'd" apart from "the guest
/// faulted" — the honest-gate rule this repo learned from `segv_rust_e2e`.
#[test]
fn the_workqueue_syscalls_are_emulated_not_forwarded() {
    let (rec, trace) = util::record_dynamic(retrace_guest::DISPATCH_DYN);

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

    // ---- The mach-semaphore census: assert on the TRACE, because printed output is not enough ----
    //
    // Everything above — and every assertion the headline gate makes (`worker\n`, `done\n`, exit 0,
    // two byte-identical replays) — is satisfied by output this guest produces EVEN IF Stage 2b's
    // semaphore seam never executes. That is not a hypothetical. `dispatch_semaphore_signal`'s FAST
    // path is a pure atomic increment that issues NO TRAP AT ALL (measured, Stage 2b Task 1 §5 item
    // 7); the slow path is taken only because a waiter already exists. Main happens to reach `-36`
    // and block before the worker is ever scheduled, so the count is negative by the time the worker
    // signals and `ldaddl` falls through to the trap — but had the worker run first, BOTH halves
    // would have taken their fast paths, no landmark would exist, this milestone's arms would be
    // dead code, and the guest would still have printed `worker` and `done` and exited 0.
    //
    // So a green run of this guest is not by itself proof that the semaphore arms work. This census
    // is. It is `segv_rust_e2e`'s rule applied here: assert on the difference your work makes, in
    // the one form no weaker path can fake.
    //
    // The DIFFERING THREAD TAGS are the load-bearing part, not the counts. Both fast paths would
    // give zero landmarks; a single-threaded run of the same code would give the same tag twice; a
    // FORWARDED workq_kernreturn would give none at all, because its host worker dies at address 0
    // before writing anything. One thread waited and a DIFFERENT thread signalled is the shape only
    // an in-box worker built by `guest_workq_reqthreads`, scheduled by the box, parking and waking
    // through `BlockReason::Sem`, can produce.
    let (events, _) = retrace_trace::Reader::open_checked(&trace).unwrap();
    let tags = |want: u64| -> Vec<u32> {
        events.iter().filter_map(|e| match e {
            Event::Syscall { num, thread, .. } if *num == want => Some(*thread),
            _ => None,
        }).collect()
    };
    // The guest issues exactly one `dispatch_semaphore_wait` and exactly one
    // `dispatch_semaphore_signal` (see `c/dispatch_dyn.c`), so anything but one landmark each means
    // the trap census stopped matching the source — a fast path taken, or a trap taken twice.
    let waits = tags(retrace_arch::MACH_SEMAPHORE_WAIT);
    let signals = tags(retrace_arch::MACH_SEMAPHORE_SIGNAL);
    assert_eq!(waits.len(), 1,
        "expected exactly one semaphore_wait_trap(-36) landmark in the trace, got {}: {waits:?}. \
         Zero means the wait took its fast path and Stage 2b's seam never ran, which every other \
         assertion in this file would still have passed.", waits.len());
    assert_eq!(signals.len(), 1,
        "expected exactly one semaphore_signal_trap(-33) landmark in the trace, got {}: \
         {signals:?}. Zero means the signal took its fast path — a pure atomic increment that issues \
         no trap at all — leaving this milestone's arms unexercised behind a green run.",
        signals.len());
    assert_ne!(waits[0], signals[0],
        "the waiter and the signaller must be DIFFERENT threads — that is the difference M18 Stage \
         2b makes. Same tag ({}) means one thread did both, so no in-box workqueue worker ran and \
         the park/wake seam was never crossed.", waits[0]);
}
