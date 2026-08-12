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
