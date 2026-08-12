// M14: the thread table and the cooperative scheduler. These are PURE — no VM, no vCPU, no HVF —
// which is the entire reason `thread.rs` is a separate module. They run in milliseconds.
//
// No `tb()` helper here yet: Task 4's six tests only exercise `ThreadTable`/`ThreadCtx` and need no
// `Box_` at all. Task 5 introduces the first VM-backed test and its `tb()` helper alongside it.
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
