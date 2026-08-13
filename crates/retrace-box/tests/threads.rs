// M14: the thread table and the cooperative scheduler. These are PURE — no VM, no vCPU, no HVF —
// which is the entire reason `thread.rs` is a separate module. They run in milliseconds.
//
// Task 4's tests are pure and need no `Box_` at all. The VM-backed tests below them do, and share
// the `tb()` helper that arrived with Task 5's context-switch round-trip.
use retrace_box::thread::{BlockReason, ThreadCtx, ThreadState, ThreadTable};

fn ctx(pc: u64) -> ThreadCtx {
    let mut c = ThreadCtx::zeroed();
    c.elr = pc;
    c
}

#[test]
fn a_fresh_table_has_one_runnable_main_thread() {
    let t = ThreadTable::new(ctx(0x1000));
    assert_eq!(t.current(), 0);
    assert_eq!(t.live(), 1);
    // A single-threaded guest must take exactly today's path: thread 0, runnable, always picked.
    assert_eq!(t.pick_next(), Some(0));
}

#[test]
fn spawn_appends_a_runnable_thread_and_does_not_switch() {
    let mut t = ThreadTable::new(ctx(0x1000));
    let child = t.spawn(ctx(0x2000), (0x30200000, 0x8000));
    assert_eq!(child, 1);
    // The real kernel does not switch on create, and neither do we.
    assert_eq!(t.current(), 0, "bsdthread_create must NOT switch away from the caller");
    assert_eq!(t.live(), 2);
}

#[test]
fn blocking_the_only_runnable_thread_leaves_the_child_pickable() {
    let mut t = ThreadTable::new(ctx(0x1000));
    t.spawn(ctx(0x2000), (0x30200000, 0x8000));
    t.block(BlockReason::Join { target: 1 });
    assert_eq!(t.pick_next(), Some(1), "main blocked in join, so the child runs");
}

#[test]
fn pick_next_is_lowest_indexed_runnable_which_is_what_makes_replay_deterministic() {
    let mut t = ThreadTable::new(ctx(0x1000));
    t.spawn(ctx(0x2000), (0x30200000, 0x8000));
    t.spawn(ctx(0x3000), (0x30300000, 0x8000));
    t.block(BlockReason::Join { target: 2 });
    // Both 1 and 2 are runnable; the LOWEST index is forced, so record and replay agree.
    assert_eq!(t.pick_next(), Some(1));
}

#[test]
fn an_exited_thread_is_never_picked_and_unblocks_its_joiner() {
    let mut t = ThreadTable::new(ctx(0x1000));
    t.spawn(ctx(0x2000), (0x30200000, 0x8000));
    t.block(BlockReason::Join { target: 1 });
    // The child runs and exits.
    t.switch_to(1);
    t.exit_current(42);
    t.unblock_joiners_of(1);
    assert_eq!(t.pick_next(), Some(0), "main's join is satisfied, main runs again");
    assert!(matches!(t.state_of(1), ThreadState::Exited(42)));
}

#[test]
fn pick_next_skips_a_lower_indexed_exited_thread_for_a_still_runnable_higher_one() {
    // The realistic shape: main calls pthread_exit while a spawned child keeps running. Unlike
    // `an_exited_thread_is_never_picked_and_unblocks_its_joiner` above — where the exited thread
    // (index 1) was never going to beat the still-runnable index 0 regardless of whether
    // `pick_next` actually excludes `Exited` — this puts the exited thread at the LOWER index. A
    // `pick_next` that also matched `Exited` (e.g. `Runnable | Exited(_)`) would return `Some(0)`
    // here instead of `Some(1)`, so this is the case that actually pins the exclusion.
    let mut t = ThreadTable::new(ctx(0x1000));
    t.spawn(ctx(0x2000), (0x30200000, 0x8000));
    t.exit_current(0); // main (index 0) exits; the child (index 1) is still Runnable.
    assert_eq!(t.pick_next(), Some(1), "an exited thread must be skipped even when it is lowest-indexed");
}

#[test]
fn every_thread_blocked_is_a_deadlock_and_pick_next_says_so() {
    let mut t = ThreadTable::new(ctx(0x1000));
    t.spawn(ctx(0x2000), (0x30200000, 0x8000));
    t.block(BlockReason::Join { target: 1 });
    t.switch_to(1);
    t.block(BlockReason::Join { target: 0 });
    // Nobody can run. pick_next reports it rather than hanging or picking a blocked thread.
    assert_eq!(t.pick_next(), None, "a deadlock must be visible, not a hang");
}

/// M14 Task 8, carrying forward Task 4's review finding: `block(Join { target })` is a bare
/// primitive that does not check whether `target` already exited, so a target that exited BEFORE
/// the join call — which already ran `unblock_joiners_of` and will never run it again — must not
/// be blocked on, or the joiner waits forever on a wake that already happened.
///
/// Fix round 1, M-1: the guard used to live in a separate wrapper (`block_on_join`) that a caller
/// had to opt into — `block` itself stayed unguarded, which is exactly what the brief's own test
/// (`a_terminating_thread_exits_and_wakes_whoever_joined_it`) calls directly. The guard is now
/// folded into `block` itself, so this test calls `block(Join { .. })` directly — the same entry
/// point every other caller in this file uses — to prove the PRIMITIVE is safe, not an alias.
///
/// Mutation check (see the Task 8 report): reverting `block` to unconditionally set
/// `Blocked(reason)` turns this into a deadlock — `pick_next()` returns `None` instead of
/// `Some(0)` — so this test cannot pass for the wrong reason.
#[test]
fn blocking_on_an_already_exited_join_target_does_not_wait_forever() {
    let mut t = ThreadTable::new(ctx(0x1000));
    t.spawn(ctx(0x2000), (0x30200000, 0x8000));
    t.switch_to(1);
    t.exit_current(42); // the child exits BEFORE anyone joins it — unblock_joiners_of(1) fires now
    t.switch_to(0);
    t.block(BlockReason::Join { target: 1 }); // main tries to join the ALREADY-exited child
    assert_eq!(t.pick_next(), Some(0),
        "the child already exited — main must stay runnable, not wait for a wake that already happened");
    assert!(!matches!(t.state_of(0), ThreadState::Blocked(_)), "the guard must keep main out of Blocked");
}

/// A `Box_` for the VM-backed tests in this file.
///
/// There is no `Box_::for_test()`; the constructor is `Box_::load(&loaded)`, and every existing
/// retrace-box test builds one this way — see `tests/checkpoint.rs:11-12`, whose exact two-line
/// form this copies. `parse_macho` takes BYTES and returns `Loaded` directly: it is not fallible,
/// and the `SPINLOOP` constant is a PATH, so read it first. M14 needs no special guest for these
/// register-level tests, only a live vCPU. **`--test-threads=1` is mandatory: one HVF VM per
/// process.**
fn tb() -> retrace_box::Box_ {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::SPINLOOP).unwrap());
    retrace_box::Box_::load(&loaded)
}

/// The first VM-backed test in this file; everything above it is pure.
///
/// The round-trip, for **every** field of `ThreadCtx` — which the first version of this test only
/// claimed. Review found it vacuous: it set x3, x29, ELR, SPSR and TPIDRRO and nothing else, and
/// `tb()`'s box already holds real values in the fields it never disturbed, so a `load_ctx` missing
/// `write_regs` outright, or a `save_ctx` missing the whole FP capture, would still have passed.
///
/// Shaped as load-then-save rather than set-clobber-load because `Box_` has no setter for PC,
/// SP_EL0, CPSR, FPCR or FPSR: `load_ctx` is the only writer for those, and it is the code under
/// test. Two DIFFERENT patterns run back to back, so a field `load_ctx` never writes cannot pass by
/// already holding the expected value — after pattern B the hardware would still read pattern A.
///
/// The values stay architecturally legal on purpose. A reserved or RAZ/WI bit that refused to
/// round-trip would be a fact about the CPU, not a defect in M14, so CPSR/SPSR keep mode EL0t and
/// vary only NZCV, FPCR varies only FZ, and FPSR varies only the exception flags.
#[test]
fn a_switch_round_trips_every_register_in_the_context() {
    let mut b = tb();
    for (round, seed) in [(0u64, 0x1111_0000_0000_0000u64), (1, 0x2222_0000_0000_0000)] {
        let mut want = ThreadCtx::zeroed();
        for (i, x) in want.regs.x.iter_mut().enumerate() { *x = seed | (i as u64 + 1); }
        want.regs.pc     = 0x1_0000_4000 + round * 0x40;
        want.regs.sp_el0 = 0x3020_8000 + round * 0x100;
        want.regs.cpsr   = if round == 0 { 0xA000_0000 } else { 0x6000_0000 }; // NZCV; mode EL0t
        for (i, f) in want.fp.iter_mut().enumerate() { *f = ((seed as u128) << 64) | (i as u128 + 1); }
        want.fpcr        = if round == 0 { 0 } else { 0x0100_0000 };           // FZ
        want.fpsr        = if round == 0 { 0x10 } else { 0x01 };               // IXC / IOC
        want.tpidrro_el0 = 0x0003_8000 + round * 0x4000;
        want.elr         = 0x1234_5000 + round * 0x1000;
        want.spsr        = if round == 0 { 0x3c4 } else { 0x8000_03c4 };       // EL0t + NZCV

        b.load_ctx(&want);

        // `save_ctx` reads the HARDWARE back, so this is a real round-trip and not a struct copy.
        assert_eq!(b.save_ctx(), want, "round {round}: the context did not survive load -> save");

        // Cross-check the sysregs through INDEPENDENT getters. `save_ctx`/`load_ctx` agreeing on the
        // WRONG register id would round-trip cleanly and still be wrong; these do not share it.
        assert_eq!(b.position(), want.elr, "round {round}: ELR_EL1");
        assert_eq!(b.spsr(), want.spsr, "round {round}: SPSR_EL1");
        assert_eq!(b.tpidrro_el0(), want.tpidrro_el0,
            "round {round}: tpidrro_el0 is THE per-thread register");
        assert_eq!(b.tpidr_el0(), 0, "tpidr_el0 must stay 0 — macOS reads the CPU number from it");
    }
}

#[test]
fn a_checkpoint_carries_every_thread_not_just_the_running_one() {
    let mut b = tb();
    let child = b.threads_mut().spawn(ctx(0x4242_0000), (0x3020_0000, 0x8000));
    b.threads_mut().block(BlockReason::Join { target: child });

    let st = b.checkpoint();

    // The failure this guards is QUIET: a checkpoint that drops non-current threads still restores
    // and still runs. Assert the table, not that the restore returned Ok.
    assert_eq!(st.threads.len(), 2, "the checkpoint must carry the child thread");
    assert_eq!(st.threads.ctx_of(child).elr, 0x4242_0000, "…and its register context");
    assert!(
        matches!(st.threads.state_of(0), ThreadState::Blocked(_)),
        "…and main's blocked state, or the restored run picks the wrong thread"
    );
}

/// The capture is only half of R4. A `from_checkpoint` that ignores `state.threads` passes the test
/// above untouched — the table would be carried into `BoxState` and then dropped on the floor at the
/// one moment it matters. Assert the RESTORED box, which is what a seeked session actually runs.
#[test]
fn a_restored_checkpoint_still_has_every_thread() {
    let st = {
        let mut b = tb();
        let child = b.threads_mut().spawn(ctx(0x4242_0000), (0x3020_0000, 0x8000));
        b.threads_mut().block(BlockReason::Join { target: child });
        b.checkpoint()
        // `b` drops here: one HVF VM per process, so it must be gone before `from_checkpoint`
        // builds a second one (`tests/checkpoint.rs:40` does the same with an explicit `drop`).
    };
    let r = retrace_box::Box_::from_checkpoint(&st);

    assert_eq!(r.threads().len(), 2, "the RESTORE must rebuild the child, not merely the capture");
    assert_eq!(r.threads().ctx_of(1).elr, 0x4242_0000, "…with its register context intact");
    assert_eq!(r.threads().current(), 0, "…and the same thread still running");
    assert!(
        matches!(r.threads().state_of(0), ThreadState::Blocked(BlockReason::Join { target: 1 })),
        "…and main still blocked on the child, or the restored run picks the wrong thread"
    );
}

/// R4 has a hardware half. Carrying the table faithfully and then forcing the vCPU's thread pointer
/// back to the single-threaded constant hands a restored child MAIN's TSD — the same quiet break one
/// layer down. `tpidrro_el0` is precisely the register `BoxState`'s flat fields never carried,
/// because it was constant until threads existed.
#[test]
fn a_restored_checkpoint_puts_the_running_threads_pointer_back_on_the_vcpu() {
    let st = {
        let mut b = tb();
        let child = b.threads_mut().spawn(ctx(0x4242_0000), (0x3020_0000, 0x8000));
        b.switch_to_thread(child);
        b.set_tpidrro_el0(0x5150_0000); // the CHILD's own TSD, deliberately not TSD_IPA
        b.checkpoint()
    };
    let r = retrace_box::Box_::from_checkpoint(&st);

    assert_eq!(r.threads().current(), 1, "the child was the running thread at capture");
    assert_eq!(r.tpidrro_el0(), 0x5150_0000,
        "a restore that forces TSD_IPA hands the child main's thread pointer");
    assert_eq!(r.tpidr_el0(), 0, "tpidr_el0 must still be 0 — macOS reads the CPU number from it");
}

#[test]
fn bsdthread_create_builds_a_thread_at_the_registered_trampoline() {
    let mut b = tb();   // see `fn tb()` at the top of this file
    b.set_thread_start_pc(0x0001_804b_2000);

    // The ABI measured in Task 2: (func, arg, stack, pthread, flags). `stack` and `pthread`
    // deliberately DIFFER here (fix round 2) — Task 2 measured them equal in every capture taken so
    // far, but that is a real property of Apple's combined stack+struct allocation, not a contract
    // this box may rely on; using distinct values means a swap between x[0]/sp_el0 in the
    // implementation actually fails the assertions below instead of passing unchanged.
    let rc = b.guest_bsdthread_create([0x1_0002_4e00, 0x62180, 0x3020_6000, 0x3020_7000, 0x90008ff, 0, 0, 0]);

    assert_eq!(rc, 0, "create must succeed");
    assert_eq!(b.threads().len(), 2);
    assert_eq!(b.threads().current(), 0, "create does not switch — the caller keeps running");
    let c = b.threads().ctx_of(1);
    assert_eq!(c.elr, 0x0001_804b_2000, "the child enters at the REGISTERED trampoline, not at func");
    // Fix round 2: the resume convention (ELR-based vs PC-based) is Task 9's to settle — see
    // guest_bsdthread_create's comment — so both must carry the entry point today.
    assert_eq!(c.regs.pc, 0x0001_804b_2000, "the child's PC must also carry the trampoline");
    // MEASURED contract (Task 2, re-disassembled in review): __pthread_start reads x0 and w5 only.
    // func/arg arrive through the pthread struct at +0x90/+0x98, which the GUEST populated before
    // trapping — so they must NOT appear in registers here.
    assert_eq!(c.regs.x[0], 0x3020_7000, "x0 is the pthread-struct pointer");
    assert_eq!(c.regs.x[5], 0x90008ff, "w5 carries the flags __pthread_start tbnz/tbz-tests");
    assert_eq!(c.regs.x[1], 0, "x1 is NOT part of the contract — seeding it would be cargo cult");
    assert_eq!(c.tpidrro_el0, 0x3020_7000, "each thread gets its own thread pointer…");
    assert_eq!(c.regs.sp_el0, 0x3020_6000, "the child runs on the guest-allocated stack");
}

/// Task 7 fix round 1: `thread_start_pc` is learned from the guest's OWN `bsdthread_register` call,
/// which sits BEHIND any checkpoint taken after it — a restored session can never re-derive it by
/// replaying forward. Mirrors Task 6's `threads`-carrying precedent exactly: state a mid-run capture
/// cannot re-derive must be carried, or the checkpoint forgets it and breaks quietly. Here that
/// means a `bsdthread_create` on the restored session would hit the same fail-loud `.expect()` an
/// UNregistered guest hits, even though THIS guest registered before the checkpoint was taken.
#[test]
fn a_restored_checkpoint_still_knows_the_registered_trampoline() {
    let st = {
        let mut b = tb();
        b.set_thread_start_pc(0x0001_804b_2000);
        b.checkpoint()
    };
    let mut r = retrace_box::Box_::from_checkpoint(&st);

    assert_eq!(r.thread_start_pc(), Some(0x0001_804b_2000),
        "the checkpoint must carry the registered trampoline, or a restored session can never \
         re-derive it — bsdthread_register sits BEHIND the checkpoint");
    // The real failure mode, exercised end-to-end: without the carry, THIS call panics on a guest
    // that already registered — exactly like `bsdthread_create_without_a_registered_trampoline_
    // fails_loud` below, except here the guest did nothing wrong.
    let rc = r.guest_bsdthread_create([1, 2, 0x3020_7000, 0x3020_7000, 0, 0, 0, 0]);
    assert_eq!(rc, 0, "a restored session must be able to create a thread without re-registering");
}

/// Fix round 1, M-6: no production path in this codebase constructs `BlockReason::Join` today —
/// only `guest_ulock_wait` runs on a real trap, and it only ever produces `Wait { addr }` (see the
/// Task 8 report's ruling 1). This test hand-installs `Join { target: 1 }` to prove
/// `guest_bsdthread_terminate`'s OWN wiring (that it calls `exit_current` + `unblock_joiners_of`
/// together, correctly) — real code reacting to a state nothing produces yet. The reachable-path
/// half of this milestone's wake story is `ulock_wait_blocks_only_when_the_guests_condition_
/// still_holds` below, which drives the primitive that IS wired from a real trap.
///
/// Fix round 1, M-6 also strengthens this: a THIRD thread (2) is joined on an UNRELATED, still-
/// live target (0) before thread 1 exits. Without it, this test could not tell "wakes exactly
/// thread 1's joiners" apart from a bug that wakes every blocked thread — `pick_next()` returns
/// `Some(0)` either way, since 0 is the lowest-indexed thread regardless. Thread 2 staying
/// `Blocked` is the assertion that actually distinguishes the two.
#[test]
fn a_terminating_thread_exits_and_wakes_whoever_joined_it() {
    let mut b = tb();   // see `fn tb()` at the top of this file
    b.set_thread_start_pc(0x0001_804b_2000);
    b.guest_bsdthread_create([0x1_0002_4e00, 0, 0x3020_7000, 0x3020_7000, 0, 0, 0, 0]); // thread 1
    b.guest_bsdthread_create([0x1_0002_4e00, 0, 0x3020_9000, 0x3020_9000, 0, 0, 0, 0]); // thread 2

    // Main (0) joins the child (1) that's about to exit. Thread 2 joins an UNRELATED target (0,
    // which never exits in this test) — see the M-6 note above for why.
    b.threads_mut().block(retrace_box::thread::BlockReason::Join { target: 1 });
    b.switch_to_thread(2);
    b.threads_mut().block(retrace_box::thread::BlockReason::Join { target: 0 });
    b.switch_to_thread(1);

    b.guest_bsdthread_terminate([0x3020_7000, 0x8000, 0, 0, 0, 0, 0, 0]);

    assert!(matches!(b.threads().state_of(1), retrace_box::thread::ThreadState::Exited(_)));
    assert_eq!(b.threads().state_of(0), retrace_box::thread::ThreadState::Runnable,
        "main's join on the exited thread must be satisfied");
    assert!(matches!(b.threads().state_of(2), retrace_box::thread::ThreadState::Blocked(_)),
        "thread 2's join on an UNRELATED, still-live target must NOT be woken by thread 1's exit");
    assert_eq!(b.threads().pick_next(), Some(0), "main's join is satisfied");
}

/// The other half of the M14 Task 8 report's ruling 1 answer: the STATE the real flow actually
/// produces is `Wait { addr }` (never `Join { target }` — see the report), so the primitive that
/// must be proven against something other than a hand-installed `block(Join { .. })` is
/// `guest_ulock_wait`'s own already-satisfied guard (Step 4). Both branches, against REAL guest
/// memory (not a struct copy) via `poke_guest`/`read_guest`. `0x1000002` is one of the two
/// operation words the M-2 fix measured from a fresh disassembly of `__pthread_join`'s retry loop
/// (see the report) — required now that `guest_ulock_wait` asserts on it.
#[test]
fn ulock_wait_blocks_only_when_the_guests_condition_still_holds() {
    let mut b = tb();   // see `fn tb()` at the top of this file
    // A real, mapped guest address: the static stack backing (a full granule below stack_top()),
    // so `read_guest_checked`'s "is this mapped" check has a real answer either way.
    let addr = b.stack_top() - 0x40;
    const OP: u64 = 0x1000002;

    // Case 1: the live value no longer matches what the guest expects (args[2]) — someone else
    // already changed it, so the wait is ALREADY SATISFIED and the thread must stay Runnable.
    b.poke_guest(addr, &99u32.to_le_bytes());
    let rc = b.guest_ulock_wait([OP, addr, 42, 0, 0, 0, 0, 0]).unwrap();
    assert_eq!(rc, 0);
    assert_eq!(b.threads().state_of(0), retrace_box::thread::ThreadState::Runnable,
        "the value already changed — blocking now would deadlock a race the guest already won");

    // Case 2: the live value STILL matches — the wait is genuine, so the thread must block on it.
    b.poke_guest(addr, &42u32.to_le_bytes());
    let rc = b.guest_ulock_wait([OP, addr, 42, 0, 0, 0, 0, 0]).unwrap();
    assert_eq!(rc, 0);
    assert!(
        matches!(b.threads().state_of(0),
            retrace_box::thread::ThreadState::Blocked(retrace_box::thread::BlockReason::Wait { addr: a }) if a == addr),
        "the value still matches — the thread must block on exactly this address"
    );
}

/// Fix round 1, M-2: an unmeasured operation word (bare op `2` in the low 16 bits, with none of
/// the flag bits `__pthread_join`'s retry loop is measured to always set alongside it — see the
/// report's fresh disassembly) must fail loud rather than silently do a 32-bit compare that could
/// deadlock a 64-bit waiter. Mutation check (see the report): deleting the `assert!` turns this
/// into a normal `Ok(0)` return instead of a panic.
#[test]
#[should_panic(expected = "unmeasured operation word")]
fn ulock_wait_rejects_an_unmeasured_operation_word() {
    let mut b = tb();
    let addr = b.stack_top() - 0x40;
    b.poke_guest(addr, &42u32.to_le_bytes());
    let _ = b.guest_ulock_wait([0x2, addr, 42, 0, 0, 0, 0, 0]); // bare op 2, no flag bits — unmeasured
}

/// Fix round 1, M-3: a bad guest address is legal guest behaviour (real `__ulock_wait` answers
/// `EFAULT`), not a retrace invariant violation — must not panic the box. Mutation check (see the
/// report): reverting to `read_guest` (the panicking form) turns this into a panic instead of
/// `Err(14)`.
#[test]
fn ulock_wait_answers_efault_on_an_unmapped_address_instead_of_panicking() {
    let mut b = tb();
    const OP: u64 = 0x1000002;
    let unmapped = 0xdead_beef_0000u64; // not inside any tracked guest backing
    assert_eq!(b.guest_ulock_wait([OP, unmapped, 42, 0, 0, 0, 0, 0]), Err(14),
        "an unmapped wait address must answer EFAULT, not panic the box");
}

#[test]
fn bsdthread_create_without_a_registered_trampoline_fails_loud() {
    let mut b = tb();   // see `fn tb()` at the top of this file
    // No bsdthread_register seen. Guessing a trampoline address would be a silent wrong answer.
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.guest_bsdthread_create([1, 2, 0x3020_7000, 4, 5, 0, 0, 0])
    }));
    assert!(r.is_err(), "must assert rather than invent an entry point");
}

// ---------------------------------------------------------------------------------------------
// M14 Task 9: the scheduler runs, and the wake seam that Task 8's review found had no owner.
// ---------------------------------------------------------------------------------------------

/// Pure. The wake seam's selectivity, at the table level.
///
/// Shaped like Task 8's M-6 fix: a SECOND waiter on a DIFFERENT address is what distinguishes
/// "wakes exactly this address's waiters" from "wakes every blocked thread". Without it,
/// `pick_next()` answers `Some(0)` either way and the test proves nothing.
#[test]
fn unblock_waiters_on_wakes_only_the_matching_address() {
    let mut t = ThreadTable::new(ctx(0x1000));
    t.spawn(ctx(0x2000), (0, 0));   // 1
    t.spawn(ctx(0x3000), (0, 0));   // 2

    t.switch_to(0);
    t.block(BlockReason::Wait { addr: 0xAAA0 });
    t.switch_to(2);
    t.block(BlockReason::Wait { addr: 0xBBB0 });
    t.switch_to(1);

    assert_eq!(t.unblock_waiters_on(0xAAA0), 1, "exactly one waiter is on that address");

    assert_eq!(t.state_of(0), ThreadState::Runnable, "the matching waiter must wake");
    assert!(matches!(t.state_of(2), ThreadState::Blocked(BlockReason::Wait { addr: 0xBBB0 })),
        "a waiter on a DIFFERENT address must not be woken — this is the whole selectivity claim");

    // A wake nobody is waiting on is legal and must not panic: the real kernel answers ENOENT and
    // `__pthread_joiner_wake` treats that as success (measured, task-9-measurements.md §2).
    assert_eq!(t.unblock_waiters_on(0xC0DE), 0, "a wake with no waiter is a no-op, not a fault");
}

/// Pure. The exact predicate `Box_::run()` consults on every entry.
///
/// This is the compatibility argument for every M0–M13 gate, expressed as a testable function
/// instead of an inline condition: a single-threaded guest has a one-entry table whose only thread
/// is `Runnable`, so `run()` never schedules and takes precisely the pre-M14 path.
#[test]
fn a_lone_runnable_thread_never_needs_rescheduling() {
    let mut t = ThreadTable::new(ctx(0x1000));
    assert!(!t.needs_reschedule(), "a single-threaded guest must never trigger a switch");

    t.block(BlockReason::Wait { addr: 0x10 });
    assert!(t.needs_reschedule(), "a blocked current thread must");

    t.switch_to(0);
    t.exit_current(0);
    assert!(t.needs_reschedule(), "…and so must an exited one");
}

/// The plan's Task 9 Step 1 test. `get_elr()` in the plan snippet is `position()` in the tree.
///
/// Asserts the live **PC** as well as ELR. `load_ctx` writes both, but only `reg::PC`/`reg::CPSR`
/// actually drive the next `hv_vcpu_run` — asserting ELR alone would pass on a `load_ctx` that
/// never moved the vCPU's execution point at all.
#[test]
fn run_switches_to_the_child_when_main_blocks() {
    let mut b = tb();   // see `fn tb()` at the top of this file
    b.set_thread_start_pc(0x0001_804b_2000);
    b.guest_bsdthread_create([0x1_0002_4e00, 0, 0x3020_7000, 0x3020_7000, 0, 0, 0, 0]);
    b.threads_mut().block(retrace_box::thread::BlockReason::Join { target: 1 });

    b.schedule_after_block();

    assert_eq!(b.threads().current(), 1, "the box must switch to the only runnable thread");
    assert_eq!(b.position(), 0x0001_804b_2000, "…and the vCPU must actually be running its context");
    assert_eq!(b.pc(), 0x0001_804b_2000, "…at the PC the next hv_vcpu_run resumes from");
}

/// Task 7 deferred this to Task 9 by name ("WHICH convention actually drives a scheduled thread's
/// first resume is Task 9's to settle"). It is `regs.pc`/`regs.cpsr`: `load_ctx` -> `write_regs`
/// writes those, and HVF resumes the vCPU from them.
///
/// So a fresh thread's `regs.cpsr` must be the creator's EL0 PSTATE, not `ThreadCtx::zeroed()`'s 0.
/// The failure this guards is quiet and awful: CPSR 0 is EL0t with DAIF clear, which *looks* like it
/// works right up until the mask bits matter.
#[test]
fn a_created_thread_resumes_with_the_creating_threads_el0_pstate() {
    let mut b = tb();
    b.set_thread_start_pc(0x0001_804b_2000);
    let creator_spsr = b.spsr();
    b.guest_bsdthread_create([0x1_0002_4e00, 0, 0x3020_7000, 0x3020_7000, 0, 0, 0, 0]);

    assert_eq!(b.threads().ctx_of(1).regs.cpsr, creator_spsr,
        "a child must resume at EL0 with the PSTATE its creator was running under, not 0");
    assert_eq!(b.threads().ctx_of(1).spsr, creator_spsr, "…and its saved SPSR must agree");

    // And the switch must actually install it, or the field above is bookkeeping nobody reads.
    b.threads_mut().block(retrace_box::thread::BlockReason::Join { target: 1 });
    b.schedule_after_block();
    assert_eq!(b.regs_snapshot().cpsr, creator_spsr, "the vCPU must be running with that PSTATE");
}

/// THE WAKE SEAM, end to end through the two box entry points the guest actually calls — the thing
/// Task 8's review established does not exist in any form, and that Task 11 cannot pass without.
///
/// Both addresses are real mapped guest memory, so `guest_ulock_wait`'s condition check has a real
/// answer. Thread 2 waits on a DIFFERENT address for the M-6 reason above.
#[test]
fn ulock_wake_wakes_exactly_the_thread_waiting_on_that_address() {
    let mut b = tb();
    b.set_thread_start_pc(0x0001_804b_2000);
    b.guest_bsdthread_create([0x1_0002_4e00, 0, 0x3020_7000, 0x3020_7000, 0, 0, 0, 0]); // 1
    b.guest_bsdthread_create([0x1_0002_4e00, 0, 0x3020_9000, 0x3020_9000, 0, 0, 0, 0]); // 2

    // `pthread + 0x34` in the real flow (task-9-measurements.md); any two distinct mapped words do
    // here. Both hold the value their waiter compares against, so both waits are genuine.
    let joined = b.stack_top() - 0x40;
    let other = b.stack_top() - 0x80;
    const WAIT_OP: u64 = 0x1000002;
    const WAKE_OP: u64 = 0x1000002;
    b.poke_guest(joined, &42u32.to_le_bytes());
    b.poke_guest(other, &42u32.to_le_bytes());

    b.guest_ulock_wait([WAIT_OP, joined, 42, 0, 0, 0, 0, 0]).unwrap();   // main blocks
    b.switch_to_thread(2);
    b.guest_ulock_wait([WAIT_OP, other, 42, 0, 0, 0, 0, 0]).unwrap();    // 2 blocks elsewhere
    b.switch_to_thread(1);

    assert_eq!(b.guest_ulock_wake([WAKE_OP, joined, 0, 0, 0, 0, 0, 0]), 0,
        "0 is the success value __pthread_joiner_wake accepts (measured, §2)");

    assert_eq!(b.threads().state_of(0), retrace_box::thread::ThreadState::Runnable,
        "the joiner waiting on this exact address must wake");
    assert!(matches!(b.threads().state_of(2), retrace_box::thread::ThreadState::Blocked(_)),
        "a thread waiting on a different address must NOT be woken");
    assert_eq!(b.threads().pick_next(), Some(0), "…and the woken joiner is now schedulable");
}

/// `ULF_WAKE_THREAD` (0x200) names a specific thread PORT in x2 rather than waking by address.
/// Treating it as an address-wake would wake the wrong thread — silently. Measured wake shapes
/// only; everything else fails loud, the same posture `guest_ulock_wait` already takes.
#[test]
#[should_panic(expected = "unmeasured operation word")]
fn ulock_wake_rejects_an_unmeasured_operation_word() {
    let mut b = tb();
    let addr = b.stack_top() - 0x40;
    b.guest_ulock_wake([0x1000202, addr, 0, 0, 0, 0, 0, 0]); // ULF_WAKE_THREAD — unmeasured
}

/// A deadlock must be a loud panic, never a spin. The plan's Task 9 Step 1 test.
#[test]
fn a_deadlock_fails_loud_instead_of_hanging() {
    let mut b = tb();   // see `fn tb()` at the top of this file
    b.set_thread_start_pc(0x0001_804b_2000);
    b.guest_bsdthread_create([0x1_0002_4e00, 0, 0x3020_7000, 0x3020_7000, 0, 0, 0, 0]);
    b.threads_mut().block(retrace_box::thread::BlockReason::Join { target: 1 });
    b.switch_to_thread(1);
    b.threads_mut().block(retrace_box::thread::BlockReason::Join { target: 0 });

    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.schedule_after_block()));
    assert!(r.is_err(), "every thread blocked must panic, never spin");
}
