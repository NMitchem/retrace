//! M48 Task 5, box level: `Box_::guest_psynch` on a static box, with the threads a guest would run
//! switched in by hand (`settle_schedule`), the way `kqmanager.rs` drives the workqueue. The port's
//! arithmetic is `psynch.rs`'s unit tests; these pin what the box adds: who blocks, whose saved
//! context a wake writes, that a refusal changes nothing, and the checkpoint carry. No wait here is
//! timed, because a deadline reads the guest clock and a static box has no commpage; `condvar_e2e`
//! covers deadlines end to end.
use retrace_arch::{PSTATE_C, SYS_PSYNCH_CVBROAD, SYS_PSYNCH_CVSIGNAL, SYS_PSYNCH_CVWAIT};
use retrace_box::thread::{BlockReason, ThreadCtx, ThreadState};
use retrace_box::Box_;

const CV: u64 = 0x1_0000_8000;

fn tb() -> Box_ {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::SPINLOOP).unwrap());
    Box_::load(&loaded)
}

/// A static box with `n` threads beside main, spawned as `threads.rs` spawns them.
fn with_threads(n: u64) -> Box_ {
    let mut b = tb();
    for k in 0..n {
        let ctx = ThreadCtx { elr: 0x2000 + k * 0x100, ..ThreadCtx::zeroed() };
        b.threads_mut().spawn(ctx, (0x3020_0000 + k * 0x10_0000, 0x8000));
    }
    b
}

/// `cvwait`'s arguments for an untimed wait with mutex 0 and node's flags (`psynch.rs`'s builder).
fn wait(l: u32, s: u32) -> [u64; 8] { [CV, ((s as u64) << 32) | l as u64, 0, 0, 0, 0xa0, 0, 0] }

/// `cvsignal`'s, with no thread port.
fn signal(l: u32, s: u32, u: u32) -> [u64; 8] { [CV, ((s as u64) << 32) | l as u64, u as u64, 0, 0, 0, 0, 0xa0] }

/// The current thread waits and blocks, as the record arm drives it: the call, then
/// `set_x0_err_and_return(0, false)`, then the switch `run()` makes on its next entry.
fn block_in_cvwait(b: &mut Box_, l: u32, s: u32) {
    let tid = b.threads().current();
    assert_eq!(b.guest_psynch(SYS_PSYNCH_CVWAIT, wait(l, s)), Ok(0));
    b.set_x0_err_and_return(0, false);
    assert_eq!(b.threads().state_of(tid), ThreadState::Blocked(BlockReason::Cv { addr: CV, deadline: None }));
    b.settle_schedule();
}

/// A saved context that already reads 0 with carry clear cannot show whether a wake wrote it, so
/// give it a stale word and carry first.
fn stale(b: &mut Box_, tid: usize) {
    let c = b.threads_mut().ctx_mut(tid);
    c.regs.x[0] = 0xdead;
    c.regs.cpsr |= PSTATE_C;
}

#[test]
fn a_cvsignal_wakes_the_blocked_waiter_and_writes_its_saved_context() {
    let mut b = with_threads(1);
    block_in_cvwait(&mut b, 0x100, 1);
    assert_eq!(b.threads().current(), 1, "the waiter blocked, so the other thread runs");
    stale(&mut b, 0);
    assert_eq!(b.guest_psynch(SYS_PSYNCH_CVSIGNAL, signal(0x100, 0, 0)), Ok(0x101), "T0(M4): the signaller's word");
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable);
    let c = b.threads().ctx_of(0);
    assert_eq!((c.regs.x[0], c.regs.cpsr & PSTATE_C), (0, 0), "the woken waiter reads 0 with carry clear (deliver_wake)");
    assert!(b.dbg_psynch().is_empty(), "L == S clears and frees the cv");
    assert_eq!(b.threads().current(), 1, "a wake does not switch: the signaller keeps the vCPU");
}

#[test]
fn a_cvbroad_wakes_every_waiter_and_each_reads_its_word() {
    let mut b = with_threads(3);
    block_in_cvwait(&mut b, 0x100, 1);
    block_in_cvwait(&mut b, 0x200, 0);
    block_in_cvwait(&mut b, 0x300, 0);
    assert_eq!(b.threads().current(), 3);
    for t in 0..3 { stale(&mut b, t); }
    // cvlsgen: S 0 over L 0x300; cvudgen: the old U 0 over the three being released.
    assert_eq!(b.guest_psynch(SYS_PSYNCH_CVBROAD, [CV, 0x300, 0x300, 0xa0, 0, 0, 0, 0]), Ok(0x301),
        "T0(M4): the broadcaster's word");
    for t in 0..3 {
        assert_eq!(b.threads().state_of(t), ThreadState::Runnable, "thread {t}");
        let c = b.threads().ctx_of(t);
        assert_eq!((c.regs.x[0], c.regs.cpsr & PSTATE_C), (0, 0), "thread {t}'s saved context");
    }
    assert!(b.dbg_psynch().is_empty());
}

/// T5-d and T5-e at the box: the numbers the port does not model, a port refusal reached through
/// the box, and a woken thread's pending signal are each refused by value with nothing changed.
#[test]
fn every_unmodelled_psynch_call_is_refused_by_value_with_nothing_changed() {
    let mut b = with_threads(1);
    block_in_cvwait(&mut b, 0x100, 1);
    let model = b.dbg_psynch().clone();
    for (num, name) in [(301, "psynch_mutexwait"), (302, "psynch_mutexdrop"), (312, "psynch_cvclrprepost"),
                        (306, "psynch_rw_rdlock"), (297, "psynch_rw_longrdlock")] {
        let e = b.guest_psynch(num, [CV, 0x100, 0, 0, 0, 0, 0, 0]).unwrap_err();
        assert!(e.starts_with(&format!("M48: psynch {name} ({num}) is not modelled")), "{e}");
    }
    let mut mutexed = wait(0x200, 0);
    mutexed[3] = 0x6000_1000;
    let e = b.guest_psynch(SYS_PSYNCH_CVWAIT, mutexed).unwrap_err();
    assert!(e.starts_with("M48: psynch psynch_cvwait") && e.contains("with mutex 0x60001000"), "{e}");
    b.threads_mut().pend(0, 30);
    let e = b.guest_psynch(SYS_PSYNCH_CVSIGNAL, signal(0x100, 0, 0)).unwrap_err();
    assert!(e.starts_with("M48: a signal is pending on thread 0"), "{e}");
    assert_eq!(b.dbg_psynch(), &model, "no refusal touched the model");
    assert_eq!(b.threads().state_of(0), ThreadState::Blocked(BlockReason::Cv { addr: CV, deadline: None }));
    assert_eq!(b.threads().state_of(1), ThreadState::Runnable, "the caller never blocked");
}

/// The `psynch` field through `checkpoint`/`from_checkpoint` (Task 4's pattern).
#[test]
fn a_blocked_cvwait_survives_a_checkpoint_and_is_woken_after_restore() {
    let mut b = with_threads(1);
    block_in_cvwait(&mut b, 0x100, 1);
    let model = b.dbg_psynch().clone();
    assert!(!model.is_empty(), "the waiter is queued");
    let st = b.checkpoint();
    drop(b); // one VM per process
    let mut r = Box_::from_checkpoint(&st);
    assert_eq!(r.dbg_psynch(), &model, "the queue is carried (BoxState::psynch)");
    assert_eq!(r.guest_psynch(SYS_PSYNCH_CVSIGNAL, signal(0x100, 0, 0)), Ok(0x101),
        "a restored box without the queue would prepost (the P bit, 0x2) and wake nobody");
    assert_eq!(r.threads().state_of(0), ThreadState::Runnable);
}
