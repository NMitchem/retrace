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

/// M16 Task 6 fix round 1 (review finding 2): `switch_to` clears `redirected` for the thread being
/// switched TO — that thread is now running the handler it was given, so the "un-run redirection"
/// `deliver_signal_to`'s fail-loud assert guards against no longer holds. Pure (no VM needed): the
/// flag lives entirely in `ThreadTable`. The reviewer's mutation (replacing the clear with a
/// no-op) left lib+deliver+threads all green, so this closes that gap.
#[test]
fn switch_to_clears_redirected_for_the_thread_it_switches_to() {
    let mut t = ThreadTable::new(ctx(0x1000));
    t.spawn(ctx(0x2000), (0x30200000, 0x8000));
    t.set_redirected(1, true);
    assert!(t.is_redirected(1));
    t.switch_to(1);
    assert!(!t.is_redirected(1), "thread 1 is now running — the un-run redirection is over");
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

/// A pthread-struct address inside THIS box's own mapped stack page.
///
/// **M14 t11 changed what a pthread pointer has to be.** `guest_bsdthread_create` now writes the
/// child's kport into the guest's pthread struct at `+0xf8` — the write `pthread_join` is unusable
/// without (see `bsdthread_create_writes_the_childs_thread_port_into_the_pthread_struct`) — so the
/// address these tests pass must be REAL, backed memory. The literal `0x3020_7000` they used before
/// is a genuine *dynamic*-guest address that the static `tb()` box does not back, and it now panics
/// in `write_guest`. That panic is correct behaviour, not collateral damage: an unmapped pthread
/// struct means the box's view of guest memory disagrees with the guest's.
///
/// `Box_::load` gives the static guest one GRANULE (0x4000) of stack, so `n` in `0..=1` stays inside
/// it with room for `+0xf8`.
fn pth(b: &retrace_box::Box_, n: u64) -> u64 { b.stack_top() - 0x4000 + n * 0x1000 }

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
    let (stack, pthread) = (pth(&b, 0), pth(&b, 1));
    let rc = b.guest_bsdthread_create([0x1_0002_4e00, 0x62180, stack, pthread, 0x90008ff, 0, 0, 0]);

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
    assert_eq!(c.regs.x[0], pthread, "x0 is the pthread-struct pointer");
    assert_eq!(c.regs.x[1], 0, "x1 is NOT part of the contract — seeding it would be cargo cult");
    assert_eq!(c.regs.sp_el0, stack, "the child runs on the guest-allocated stack");

    // M14 t11. Both of these used to assert the guest's raw values, and both were WRONG in the
    // same way: they described what the guest asked for rather than what the KERNEL delivers, so
    // the child brk'd on libpthread's own consistency check the first time one really ran.
    //
    // `__pthread_start`'s first two instructions are the check:
    //     0x6be0  tbnz w5, #0x1d, ...  ; PTHREAD_START_SUSPENDED  -> "kernel without ... support"
    //     0x6be4  tbz  w5, #0x1c, ...  ; TSD_BASE_SET *clear*     -> brk #0xb001, message
    //             "BUG IN LIBPTHREAD: thread_set_tsd_base() wasn't called by the kernel"
    // so bit 28 is the kernel's assertion that it set the TSD base, and it must be ORed in on top
    // of the guest's own flag bits rather than replacing them.
    assert_eq!(c.regs.x[5], 0x90008ff | 0x1000_0000,
        "w5 must carry the guest's flags PLUS the kernel's TSD_BASE_SET bit");

    // And the base it claims to have set: TPIDRRO_EL0 is the TSD, which sits at +0xe0 INSIDE the
    // pthread struct — not the struct pointer. `__pthread_join` reads it back the other way
    // (`mrs x23, TPIDRRO_EL0` / `sub x21, x23, #0xe0`), and a host probe measured +0xe0 exactly,
    // 4/4, for main and child alike. Setting the flag without this offset would be the box lying.
    assert_eq!(c.tpidrro_el0, pthread + 0xe0,
        "TPIDRRO_EL0 is the TSD at pthread+0xe0, which is what TSD_BASE_SET above promises");
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
    let p = pth(&r, 0);
    let rc = r.guest_bsdthread_create([1, 2, p, p, 0, 0, 0, 0]);
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
    let (p1, p2) = (pth(&b, 0), pth(&b, 1));
    b.guest_bsdthread_create([0x1_0002_4e00, 0, p1, p1, 0, 0, 0, 0]); // thread 1
    b.guest_bsdthread_create([0x1_0002_4e00, 0, p2, p2, 0, 0, 0, 0]); // thread 2

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

/// M17: the Join tripwire fires on the HAZARD it exists for, not on a proxy for it.
///
/// `unblock_joiners_of` is a live wake path that today wakes nobody, because no production code
/// constructs `BlockReason::Join` (measured — every construction site is in this file). M17's
/// "exactly one materialisation site" rests on that. If a producer ever appears, a joiner woken
/// while carrying a pending signal is a signal M17 materialises nowhere and therefore swallows in
/// silence — the one failure a determinism oracle cannot see, because record and replay agree.
///
/// The negative case is `a_terminating_thread_exits_and_wakes_whoever_joined_it` directly above:
/// the same wiring, the same wake, no pending signal — and it must stay green. That pair is why the
/// tripwire asserts on the pending set rather than on "woke anybody at all", which would have made
/// the legitimate wiring test impossible to write.
#[test]
#[should_panic(expected = "carrying pending signals")]
fn a_woken_joiner_carrying_a_pending_signal_trips_the_m17_tripwire() {
    let mut b = tb();
    b.set_thread_start_pc(0x0001_804b_2000);
    let p1 = pth(&b, 0);
    b.guest_bsdthread_create([0x1_0002_4e00, 0, p1, p1, 0, 0, 0, 0]); // thread 1

    // Main blocks joining the child AND is holding an undelivered SIGUSR1.
    b.threads_mut().block(retrace_box::thread::BlockReason::Join { target: 1 });
    b.threads_mut().pend(0, 30);
    b.switch_to_thread(1);

    b.guest_bsdthread_terminate([0x3020_7000, 0x8000, 0, 0, 0, 0, 0, 0]);
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
    let rc = b.guest_ulock_wait([OP, addr, 42, 0, 0, 0, 0, 0]);
    assert_eq!(rc, 0);
    assert_eq!(b.threads().state_of(0), retrace_box::thread::ThreadState::Runnable,
        "the value already changed — blocking now would deadlock a race the guest already won");

    // Case 2: the live value STILL matches — the wait is genuine, so the thread must block on it.
    b.poke_guest(addr, &42u32.to_le_bytes());
    let rc = b.guest_ulock_wait([OP, addr, 42, 0, 0, 0, 0, 0]);
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
/// report): reverting to `read_guest` (the panicking form) turns this into a panic instead of a
/// return value.
///
/// Fix round 1, I-2: and the value is **`-EFAULT`, not `+EFAULT`**. Both operation words this
/// syscall admits carry `ULF_NO_ERRNO` (bit 24), under which the kernel returns a negative errno in
/// x0 with `PSTATE.C` clear — measured on this host with a raw `svc #0x80` against syscall 515
/// (probe and output in the fix-round-1 report):
///
/// ```text
/// op=0x01000002  x0=0xfffffffffffffff2  w0=-14  C=0
/// op=0x01020002  x0=0xfffffffffffffff2  w0=-14  C=0
/// op=0x00000002  x0=0x000000000000000e  w0=+14  C=1     <- the convention this used to use
/// ```
///
/// The old `Err(14)` reached `set_x0_err_and_return(14, true)`, which set carry and sent the guest
/// into libsyscall's `cerror`; `__pthread_join` @ `0x911c` tests its result with `cmn w0, #0x4`
/// (`w0 == -4`), so it would have missed, re-read the word and re-waited — a livelock of recorded
/// 515 events. The `w0` assertion below is the one that actually matters to the guest: `cmn` and
/// `__pthread_mutex_ulock_unlock_slow`'s `tbz w0, #0x1f` both read the 32-bit view.
#[test]
fn ulock_wait_answers_negative_efault_on_an_unmapped_address_instead_of_panicking() {
    let mut b = tb();
    const OP: u64 = 0x1000002;
    let unmapped = 0xdead_beef_0000u64; // not inside any tracked guest backing
    let rc = b.guest_ulock_wait([OP, unmapped, 42, 0, 0, 0, 0, 0]);

    assert_eq!(rc, (-14i64) as u64,
        "an unmapped wait address must answer -EFAULT sign-extended, exactly as the kernel does");
    assert_eq!(rc as u32 as i32, -14,
        "…and w0 — the view every measured libpthread consumer actually tests — must be negative");
    assert_ne!(rc, 14, "+EFAULT is the errno convention this operation word does NOT use");
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

    assert_eq!(t.unblock_waiters_on(0xAAA0), vec![0],
        "exactly one waiter is on that address, and it is thread 0 — M17 needs the IDENTITY, not \
         just the count: materialising a pending signal at the wake requires knowing WHO woke");

    assert_eq!(t.state_of(0), ThreadState::Runnable, "the matching waiter must wake");
    assert!(matches!(t.state_of(2), ThreadState::Blocked(BlockReason::Wait { addr: 0xBBB0 })),
        "a waiter on a DIFFERENT address must not be woken — this is the whole selectivity claim");

    // A wake nobody is waiting on is legal and must not panic: the real kernel answers ENOENT and
    // `__pthread_joiner_wake` treats that as success (measured, task-9-measurements.md §2).
    assert_eq!(t.unblock_waiters_on(0xC0DE), Vec::<usize>::new(),
        "a wake with no waiter is a no-op, not a fault");
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
    b.guest_bsdthread_create([0x1_0002_4e00, 0, pth(&b, 0), pth(&b, 0), 0, 0, 0, 0]);
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
///
/// Fix round 1, I-1: **the creator's PSTATE is PINNED before the create, and that is what makes
/// this test able to fail.** `Box_::load` writes `reg::CPSR = 0` and never writes `SPSR_EL1` at
/// all, so without the pin `creator_spsr` is HVF's *reset* SPSR_EL1 — an unmeasured value. If that
/// value happens to be 0 (the likely case for a zero-initialised vCPU) then with the fix reverted
/// assertion (1) reads `0 == 0` and assertion (3) reads `0 == 0`: the test passes identically
/// whether `ctx.regs.cpsr = ctx.spsr` exists or not, and nothing in the suite covers it. Pinning
/// `0x8000_03c4` (EL0t + N — the same architecturally-legal value
/// `a_switch_round_trips_every_register_in_the_context` already proves round-trips on this
/// hardware) makes both assertions compare against a value that is distinct from every zero in
/// sight. Mutation-tested: deleting `ctx.regs.cpsr = ctx.spsr` makes this test fail, and the
/// transcript is in the Task 9 fix-round-1 report.
#[test]
fn a_created_thread_resumes_with_the_creating_threads_el0_pstate() {
    let mut b = tb();
    b.set_thread_start_pc(0x0001_804b_2000);
    b.set_spsr(0x8000_03c4);
    let creator_spsr = b.spsr();
    // The pin is the whole non-vacuity argument, so assert it took. A hardware that refused this
    // value would quietly return the test to comparing zeroes with itself.
    assert_eq!(creator_spsr, 0x8000_03c4, "the pinned creator PSTATE must survive the write");
    assert_ne!(creator_spsr, b.regs_snapshot().cpsr,
        "…and must differ from the live CPSR, or assertion (3) below cannot discriminate");
    b.guest_bsdthread_create([0x1_0002_4e00, 0, pth(&b, 0), pth(&b, 0), 0, 0, 0, 0]);

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
    let (p1, p2) = (pth(&b, 0), pth(&b, 1));
    b.guest_bsdthread_create([0x1_0002_4e00, 0, p1, p1, 0, 0, 0, 0]); // 1
    b.guest_bsdthread_create([0x1_0002_4e00, 0, p2, p2, 0, 0, 0, 0]); // 2

    // `pthread + 0x34` in the real flow (task-9-measurements.md); any two distinct mapped words do
    // here. Both hold the value their waiter compares against, so both waits are genuine.
    let joined = b.stack_top() - 0x40;
    let other = b.stack_top() - 0x80;
    const WAIT_OP: u64 = 0x1000002;
    const WAKE_OP: u64 = 0x1000002;
    b.poke_guest(joined, &42u32.to_le_bytes());
    b.poke_guest(other, &42u32.to_le_bytes());

    assert_eq!(b.guest_ulock_wait([WAIT_OP, joined, 42, 0, 0, 0, 0, 0]), 0); // main blocks
    b.switch_to_thread(2);
    assert_eq!(b.guest_ulock_wait([WAIT_OP, other, 42, 0, 0, 0, 0, 0]), 0);  // 2 blocks elsewhere
    b.switch_to_thread(1);

    assert_eq!(b.guest_ulock_wake([WAKE_OP, joined, 0, 0, 0, 0, 0, 0]), (0, vec![0]),
        "0 is the success value __pthread_joiner_wake accepts (measured, §2), and M17 adds the \
         IDENTITY alongside it: the wake reports that it woke the JOINER (0), not the thread \
         waiting on a different address (2). The state assertions below check the same thing from \
         the table's side; this checks what the wake itself reported, which is what the dispatch \
         arms actually consume.");

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

/// Fix round 1, I-3: `Box_::step()` is the SECOND way into the guest, and it must reschedule off a
/// blocked thread exactly like `Box_::run()` does. `ReplaySession`'s `step_insns` /
/// `window_len_here` drive the guest only through `step()`, so a session parked on a thread that
/// just blocked would otherwise measure an instruction window on the WRONG thread and let
/// `checkpointed_seek` memoize it — silently, since this is below the trace and no `Divergence` can
/// see it.
///
/// **The PC assertion is the point of this test, not decoration.** The obvious wrong fix — putting
/// the reschedule inside `run_one_for_step` instead of at the top of `step()` — still switches
/// threads, so `current() == 1` passes either way. But it runs `load_ctx` -> `write_regs` ->
/// `set_reg(reg::CPSR, …)` AFTER `step()` armed `PSTATE_SS`, wiping that bit while MDSCR_EL1.SS
/// stays set. That leaves the step state machine active-*pending*, so the exception fires before
/// the next instruction rather than after it: `Stop::Step` comes back having retired NOTHING, and
/// `pc` is still `entry + 4`. Asserting the retired PC is what separates the two.
///
/// The child runs REAL mapped guest code — the spinloop's own `loop1: subs x0, x0, #1` one
/// instruction past the entry — because a step needs something executable to retire. Its context is
/// built by hand rather than through `guest_bsdthread_create` so the PSTATE is pinned to the EL0t 0
/// that `Box_::load` gives the main thread, instead of HVF's unmeasured reset SPSR_EL1 (see I-1
/// above): a child resuming at EL1 would land in `run_one_for_step`'s trap arm and prove nothing.
#[test]
fn step_reschedules_off_a_blocked_thread_before_arming_the_step_bit() {
    let mut b = tb();
    let entry = b.pc();

    let mut child = ThreadCtx::zeroed();
    child.regs.pc = entry + 4;
    child.elr = entry + 4;
    child.regs.cpsr = 0;                    // EL0t, DAIF clear — what Box_::load runs main under
    child.regs.sp_el0 = b.stack_top() - 0x1000;
    b.threads_mut().spawn(child, (0, 0));
    b.threads_mut().block(BlockReason::Wait { addr: 0x10 });

    let stop = b.step();

    assert_eq!(b.threads().current(), 1,
        "step() must switch off the blocked thread, not single-step it");
    assert!(matches!(stop, retrace_box::Stop::Step), "one instruction, cleanly stepped: {stop:?}");
    assert_eq!(b.pc(), entry + 8,
        "exactly ONE instruction must have retired — entry+4 here means PSTATE_SS was wiped by a \
         reschedule placed after the arming");
}

/// A deadlock must be a loud panic, never a spin. The plan's Task 9 Step 1 test.
#[test]
fn a_deadlock_fails_loud_instead_of_hanging() {
    let mut b = tb();   // see `fn tb()` at the top of this file
    b.set_thread_start_pc(0x0001_804b_2000);
    b.guest_bsdthread_create([0x1_0002_4e00, 0, pth(&b, 0), pth(&b, 0), 0, 0, 0, 0]);
    b.threads_mut().block(retrace_box::thread::BlockReason::Join { target: 1 });
    b.switch_to_thread(1);
    b.threads_mut().block(retrace_box::thread::BlockReason::Join { target: 0 });

    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.schedule_after_block()));
    assert!(r.is_err(), "every thread blocked must panic, never spin");
}

/// **M14 Task 10, F-1: the switch's WIRING into `run()`, not just its logic.**
///
/// Task 10 mutation-tested the scheduler and found the logic well covered — breaking `pick_next`
/// fails 8 tests. It also found that *deleting the call site in `run()` outright fails nothing*:
/// all of `retrace-box`, 150/150, stayed green with `lib.rs:2140-2142` removed. Two structural
/// reasons: no test in this file called `run()` at all (every other one drives the box through
/// `schedule_after_block`, `switch_to_thread`, or `step`), and for a single-threaded guest
/// `needs_reschedule()` is false by construction, so the branch was already a no-op on every
/// M0–M13 path. `step()`'s call site was covered — by the test Task 9's fix round added for I-3 —
/// and `run()`'s, written in the same task, was not. This is that test, one entry point over.
///
/// **The discriminator is the syscall NUMBER, and it is why this test can actually fail.**
/// `SPINLOOP`'s two `svc`s are different calls (verified with `otool -tv`, `_start` at
/// `0x1_0000_0380`):
///   * `_start+0x20`: `mov x16,#4` … `svc` — SYS_write, which is where MAIN is headed from `+0`
///   * `_start+0x30`: `mov x0,#0` ; `mov x16,#1` ; `svc` — SYS_exit, where the CHILD is parked
///
/// So the returned `Stop` names which thread the vCPU actually ran, with no reliance on the thread
/// table's own bookkeeping to report it. **Both assertions are mutation-proven, and they catch
/// DIFFERENT defects — neither is redundant:**
///   * Delete the reschedule from `run()` (`lib.rs:2140-2142`) — blocked main resumes at `+0`,
///     `current()` reads 0, and the FIRST assertion fires. That is the F-1 mutation, which before
///     this test failed nothing anywhere in the crate.
///   * Keep the reschedule but drop `load_ctx` from `switch_to_thread` — the table reads 1 while
///     the vCPU still holds main's registers, so `current() == 1` PASSES and only the syscall
///     number notices: `num` reads 4 and the SECOND assertion fires.
///
/// The second case is why the `Stop` is asserted at all. `current()` is the table describing
/// itself; `num` is the hardware saying which code it really ran.
#[test]
fn run_switches_to_the_child_when_main_blocks_going_through_run() {
    let mut b = tb();
    let entry = b.pc();

    let mut child = ThreadCtx::zeroed();
    child.regs.pc = entry + 0x30;           // mov x0,#0 ; mov x16,#1 ; svc #0x80  -> SYS_exit
    child.elr = entry + 0x30;
    child.regs.cpsr = 0;                    // EL0t, DAIF clear — what Box_::load runs main under
    child.regs.sp_el0 = b.stack_top() - 0x1000;
    b.threads_mut().spawn(child, (0, 0));
    b.threads_mut().block(BlockReason::Wait { addr: 0x10 });

    let stop = b.run();

    assert_eq!(b.threads().current(), 1, "run() must switch off the blocked thread");
    match stop {
        retrace_box::Stop::Syscall { num, .. } => assert_eq!(
            num, 1,
            "the CHILD's SYS_exit must be what trapped — num=4 (SYS_write) means the vCPU ran \
             MAIN. Two distinct causes, and this assertion is mutation-proven against BOTH: the \
             reschedule in run() never fired, or switch_to_thread moved the table without \
             load_ctx moving the hardware"),
        other => panic!("expected the child's exit syscall, got {other:?}"),
    }
}

/// **M14 t11 root cause: the kernel writes the child's mach port into the guest's pthread struct.**
///
/// `__pthread_join` does NOT unconditionally wait. Disassembly of libsystem_pthread on this host:
/// ```text
/// 0x90b8  ldr  w9, [x19, #0x34]   ; the join futex word
/// 0x90bc  cmn  w9, #0x1           ; already -1 => thread exited
/// 0x90c0  b.eq 0x9208
/// 0x90c4  ldr  w9, [x19, #0xf8]   ; <-- else the KPORT becomes the wait value
/// 0x90c8  str  w9, [sp, #0x10]
/// 0x90cc  str  w9, [x19, #0x34]
/// ...
/// 0x9024  cbz  w8, 0x9198         ; kport == 0  =>  SKIP the wait entirely
/// 0x9198  bl   __pthread_deallocate
/// 0x91a4  mov  w21, #0x0          ;              =>  and return SUCCESS
/// ```
/// So a zero at `+0xf8` makes `join` free the thread and return 0 without ever blocking — the child
/// never runs, and the caller sees a thread that "joined" without having executed. That is exactly
/// how M14's headline guest failed: libstd's `Arc::get_mut(...).expect("threads should not
/// terminate unexpectedly")` panicked because the child never dropped its packet clone.
///
/// **The kernel is the writer, measured not inferred.** The only two userspace writers of `+0xf8`
/// in libsystem_pthread are `__pthread_main_thread_init` and `__pthread_wqthread_setup` — neither on
/// the `pthread_create` path. A host probe confirmed it directly, 5/5 runs: with the child provably
/// not yet run, `[+0xf8]` already equals `pthread_mach_thread_np(t)` and `[+0x34]` is still 0.
///
/// The port retrace writes is synthetic and derived from the thread id, so it is a pure function of
/// the guest's own syscall sequence — identical on record and replay, which is what lets this write
/// stay out of the trace entirely (both dispatch arms call `guest_bsdthread_create`).
#[test]
fn bsdthread_create_writes_the_childs_thread_port_into_the_pthread_struct() {
    let mut b = tb();
    b.set_thread_start_pc(0x0001_804b_2000);

    // A pthread struct inside the guest's own mapped stack backing. The literal 0x3020_7000 the
    // other tests use is a real dynamic-guest address that is NOT backed in this static box.
    let pthread = b.stack_top() - 0x4000;
    b.poke_guest(pthread + 0xf8, &0u32.to_le_bytes());   // as the host measures it pre-create

    b.guest_bsdthread_create([0x1_0002_4e00, 0, pthread + 0x1000, pthread, 0, 0, 0, 0]);

    let got = u32::from_le_bytes(b.read_guest(pthread + 0xf8, 4).try_into().unwrap());
    assert_ne!(got, 0,
        "pthread+0xf8 must be a non-zero thread port — join reads THIS word and skips \
         __ulock_wait entirely when it is 0, so a zero here is a child that never runs");
}

/// M15 Task 1: pins the invariant `ReplaySession::current_thread()` will expose — that
/// `ThreadTable::current()` tracks the scheduler's own switch, both before (creation must not
/// switch) and after (a block must) — using the same round-trip `run_switches_to_the_child_when_main_blocks`
/// above already exercises via `schedule_after_block()` directly rather than through `run()`.
#[test]
fn current_thread_follows_the_scheduler_across_a_switch() {
    let mut b = tb();
    b.set_thread_start_pc(0x0001_804b_2000);
    let p = pth(&b, 1);
    b.guest_bsdthread_create([0x1000, 0, p, p, 0x90008ff, 0, 0, 0]);
    assert_eq!(b.threads().current(), 0, "creation must not switch — the real kernel does not either");

    // Block thread 0 so the scheduler has somewhere to go, then take the switch.
    b.threads_mut().block(retrace_box::thread::BlockReason::Wait { addr: 0xdead_0000 });
    b.schedule_after_block();
    assert_eq!(b.threads().current(), 1, "the scheduler must have switched to the child");
}

/// M15 Task 2: the trap this task exists to avoid. `ThreadTable::ctx_of(current)` is stale while
/// that thread is running — only `switch_to_thread` writes a thread's context back into the table
/// (see its call in `checkpoint()`, which folds the live vCPU in before cloning the table for
/// exactly this reason). A `dbg_regs_of` that read the table unconditionally would print STALE
/// registers for the CURRENT thread, confidently — no panic, no error, just a quiet lie.
#[test]
fn dbg_regs_of_reads_the_live_vcpu_for_the_current_thread_not_the_stale_table_slot() {
    let mut b = tb();
    // Put a distinctive value in a register of the CURRENT thread, WITHOUT switching. The table's
    // slot for thread 0 still holds whatever it had at construction, so a table read misses this.
    b.vcpu_set_x(3, 0xfeed_face_dead_beef);

    let dump = b.dbg_regs_of(0).expect("thread 0 exists");
    assert!(dump.contains("feedfacedeadbeef"),
        "dbg_regs_of(current) must read the LIVE vCPU: the table's slot is stale between \
         switches, and printing it would be a confident lie. Got:\n{dump}");
}

/// The other half of the trap: a NON-current thread has no live vCPU state at all — the table IS
/// the authority for it, and this must work even while that thread is BLOCKED (impossible before
/// this milestone: there was no way to inspect a thread that wasn't running).
///
/// **Fix round 1:** the first version of this test called `block()` right after `spawn()` without
/// switching first. `ThreadTable::block` takes no tid — it unconditionally blocks
/// `self.threads[self.current]` (`thread.rs:126`) — and `spawn` never switches (M14's own contract:
/// "the real kernel does not switch on create, and neither do we"), so `current` was still 0 and
/// that call blocked thread 0, leaving thread 1 `Runnable` for the whole test. The test still
/// passed and still proved a real property (non-current reads the table), but not the BLOCKED case
/// its name claimed. Fixed by actually switching to the child so `block()` lands on it, then using
/// `schedule_after_block()` — the real scheduler path, not a table-only shortcut — to switch back to
/// main, which is what folds the child's live registers into its table slot on the way out
/// (`switch_to_thread` -> `save_ctx` -> `ctx_mut`, the same fold the current-thread test's doc
/// comment describes for `checkpoint()`).
#[test]
fn dbg_regs_of_reads_the_table_for_a_blocked_non_current_thread() {
    let mut b = tb();
    b.set_thread_start_pc(0x0001_804b_2000);
    let p = pth(&b, 1);
    b.guest_bsdthread_create([0x0001_0002_4e00, 0, p, p, 0, 0, 0, 0]);

    b.switch_to_thread(1);
    b.set_elr(0xcafe_babe_0000_0000); // a distinctive LIVE value; only a real switch-away folds it into the table
    b.threads_mut().block(BlockReason::Wait { addr: 0xdead_0000 }); // blocks the CURRENT thread, i.e. 1
    b.schedule_after_block(); // picks the lowest-indexed runnable thread — main (0) — switching away from 1

    assert_eq!(b.threads().current(), 0, "main must be the thread picked back up");
    assert!(matches!(b.threads().state_of(1), ThreadState::Blocked(_)),
        "thread 1 must actually be Blocked, or this does not test the blocked-non-current case");

    let dump = b.dbg_regs_of(1).expect("thread 1 exists");
    assert!(dump.contains("cafebabe00000000"),
        "dbg_regs_of(non-current) must read the TABLE, since that thread has no live vCPU state. \
         Got:\n{dump}");
}

/// An out-of-range thread id is a `None`, not a panic — the CLI turns this into a usage error.
#[test]
fn dbg_regs_of_is_none_for_an_out_of_range_thread_id() {
    let b = tb();
    assert_eq!(b.dbg_regs_of(1), None, "a single-threaded box has no thread 1");
}

/// M15 Task 6: `DBGWVR/DBGWCR`/`MDSCR_EL1` are vCPU-global and deliberately absent from
/// `ThreadCtx` (see its doc comment in `thread.rs`) — `switch_to_thread` moves only
/// `save_ctx`/`load_ctx`'s fields, neither of which mentions them. So a watchpoint armed before a
/// context switch must still be armed after one: one vCPU, one address space, so any thread's
/// store should trip it. That is correct and desirable, but every M5 watchpoint test predates M14
/// and runs a single-threaded guest, so this property was correct by accident and entirely
/// unexercised until now.
///
/// Asserts the HARDWARE leaf via `dbg_watch0_hw` (a test-only accessor added for this task — no
/// other route exists to read `DBGWVR0_EL1`/`DBGWCR0_EL1`/`MDSCR_EL1` back off the vCPU), not just
/// the software `watch_ranges` mirror `apply_and_return` consults on the syscall path. M13 Task
/// 8's defect was a test that checked only a software mirror and passed while the hardware leaf
/// disagreed; checking `watch_ranges` alone here would pass even if `load_ctx` wiped `MDSCR_EL1`.
/// See the Task 6 report for the mutation-check transcript proving this test would have failed
/// exactly that mutation.
#[test]
fn an_armed_watchpoint_survives_a_context_switch() {
    let mut b = tb();   // see `fn tb()` at the top of this file
    b.set_thread_start_pc(0x0001_804b_2000);
    let p = pth(&b, 1);
    b.guest_bsdthread_create([0x1_0002_4e00, 0, p, p, 0, 0, 0, 0]); // thread 1

    // Real mapped guest memory (the static stack backing), word-aligned — same address shape the
    // ulock_wait tests above already use.
    let addr = b.stack_top() - 0x40;
    b.arm_hw_watchpoint(0, addr, 4);

    // MDSCR_EL1.MDE — lib.rs:217, gates the whole HW breakpoint/watchpoint machine.
    const MDSCR_MDE_BIT: u64 = 1 << 15;
    let (wvr0, wcr0, mdscr0) = b.dbg_watch0_hw();
    // Nonvacuity: prove arming actually took, or the "survives a switch" assertions below would
    // pass trivially on a watchpoint that was never armed in the first place.
    assert_ne!(wcr0 & 0x1, 0, "arm_hw_watchpoint must set DBGWCR0_EL1's enable bit (E, bit0)");
    assert_ne!(mdscr0 & MDSCR_MDE_BIT, 0, "…and MDSCR_EL1.MDE");

    // Force a real switch — block main, then let the scheduler take it, the same idiom
    // `run_switches_to_the_child_when_main_blocks` above uses, not a table-only shortcut.
    b.threads_mut().block(retrace_box::thread::BlockReason::Join { target: 1 });
    b.schedule_after_block();
    assert_eq!(b.threads().current(), 1, "the switch must actually have happened");

    let (wvr1, wcr1, mdscr1) = b.dbg_watch0_hw();
    assert_eq!(wvr1, wvr0, "DBGWVR0_EL1 must still hold the armed address after the switch");
    assert_ne!(wcr1 & 0x1, 0,
        "DBGWCR0_EL1's enable bit (E, bit0 of DBGWCR_BASE) must still be set after the switch");
    assert_ne!(mdscr1 & MDSCR_MDE_BIT, 0,
        "MDSCR_EL1.MDE must still be set after the switch — the watch machine stays armed for \
         EVERY thread, which is what lets it catch any thread's store");
}

/// M18 Task 4: `bsdthread_register(threadstart, wqthread, pthsize, …)` must capture all three of
/// its first three args — `threadstart` for M14's `bsdthread_create` (unchanged), plus the two new
/// Stage-2 fields — and return the synthesized `WORKQ_FEATURE_WORD` rather than whatever the host
/// would say about retrace's own process.
#[test]
fn bsdthread_register_captures_all_three_and_returns_the_feature_word() {
    // args per the Darwin signature: (threadstart, wqthread, pthsize, …). The arch crate's own
    // doc comment on SYS_BSDTHREAD_REGISTER names them in this order.
    let mut b = tb();   // see `fn tb()` at the top of this file
    let rc = b.guest_bsdthread_register([0x1111, 0x2222, 0x3333, 0, 0, 0, 0, 0]);
    assert_eq!(rc, retrace_box::WORKQ_FEATURE_WORD as u64, "the guest must get the synthesized word");
    assert_eq!(b.thread_start_pc(), Some(0x1111), "threadstart still captured (M14's need)");
    assert_eq!(b.wq_thread_pc(), Some(0x2222), "wqthread captured — Stage 2 enters here");
    assert_eq!(b.pthread_size(), Some(0x3333), "pthsize captured — Stage 2 allocates this");
}

/// M18 Stage 2a: `workq_open` is EMULATED, never forwarded. Forwarding it brings up a real kernel
/// workqueue for RETRACE's own process, which is half of what makes the pair whole-process fatal
/// (Task 6's crash report: a host worker enters `start_wqthread` and jumps to address 0).
#[test]
fn workq_open_returns_success_once_the_guest_has_registered() {
    let mut b = tb();   // see `fn tb()` at the top of this file
    // The registration is the precondition: it is what captures `wqthread`, which Stage 2b enters.
    b.guest_bsdthread_register([0x1111, 0x2222, 0x3333, 0, 0, 0, 0, 0]);
    assert_eq!(b.guest_workq_open([0, 0, 0, 0, 0, 0, 0, 0]), 0,
        "workq_open must report success — libdispatch treats a failure as no workqueue at all");
}

/// The fail-loud half. A `workq_open` with no registered `wqthread` means the guest took a path no
/// measurement covers — the same posture `guest_bsdthread_create`'s `thread_start_pc.expect(...)`
/// takes, and for the same reason: refusing to invent a thread entry point.
#[test]
#[should_panic(expected = "workq_open before bsdthread_register")]
fn workq_open_before_registration_fails_loud() {
    let mut b = tb();
    b.guest_workq_open([0, 0, 0, 0, 0, 0, 0, 0]);
}

/// M18 Stage 2a: the `0x400` opcode — libdispatch configuring the workqueue for dispatch. It
/// carries a guest pointer in `args[1]` that Stage 2b will need; Stage 2a only has to not forward
/// it. Measured as the FIRST of the three workqueue traps, before `workq_open`.
#[test]
fn workq_kernreturn_setup_dispatch_succeeds() {
    let mut b = tb();
    b.guest_bsdthread_register([0x1111, 0x2222, 0x3333, 0, 0, 0, 0, 0]);
    // The measured args vector, verbatim from stage2-measurements.md §2.
    let rc = b.guest_workq_kernreturn([0x400, 0x27ff6a8, 0x18, 0x0, 0x0, 0x20, 0, 0]);
    assert_eq!(rc, 0, "setup must report success or libdispatch abandons the workqueue");
}

/// The deliberate, self-imposed Stage 2a wall. `REQTHREADS` is where a worker would be built, and
/// worker construction is Stage 2b — so this refuses BY NAME rather than returning a success the
/// guest would then wait forever on. Refusing here is strictly better than the behaviour it
/// replaces, which was handing the syscall to the host kernel and having the host spawn a real
/// thread inside the recorder.
#[test]
#[should_panic(expected = "worker construction is Stage 2b")]
fn workq_kernreturn_reqthreads_is_the_named_stage_2a_wall() {
    let mut b = tb();
    b.guest_bsdthread_register([0x1111, 0x2222, 0x3333, 0, 0, 0, 0, 0]);
    // The measured args vector, verbatim. args[3]=0x40008ff looks like a packed priority/QoS word
    // and is Stage 2b's to decode.
    b.guest_workq_kernreturn([0x20, 0x0, 0x1, 0x40008ff, 0x0, 0x20, 0, 0]);
}

/// The `guest_ulock_wake` posture: an operation word nobody measured is refused BY VALUE, so the
/// panic tells the next reader exactly what to go measure. Asserting that the message names the
/// value is the point of the test — a panic that just said "unsupported" would be useless.
#[test]
#[should_panic(expected = "0xbeef")]
fn workq_kernreturn_refuses_an_unmeasured_opcode_by_value() {
    let mut b = tb();
    b.guest_bsdthread_register([0x1111, 0x2222, 0x3333, 0, 0, 0, 0, 0]);
    b.guest_workq_kernreturn([0xbeef, 0, 0, 0, 0, 0, 0, 0]);
}
