//! M46 §3d, box level. The event manager's lifecycle on a static box, with the kernel's side
//! driven by hand the way `threads.rs` drives M18's workers. These tests pin the paths the
//! fixtures may not reach (Review Focus 1–2): a redelivery, a poke while bound, the reuse
//! re-entry, and the refusals. No timer is armed here, because a timer reads the guest clock, and
//! a static box has no commpage. `gcdtimer_e2e` covers the timers end to end.
//!
//! The expected flag words: `0x3C_4008` first use and `0x1E_4008` reuse, both observed natively by
//! t0 M2. `0x1E_0000` redelivery was NOT observed (no native `KEVENT_RETURN` found events
//! waiting), so it stays inferred from xnu (`pthread_workqueue.c:3695-3703`), and the redelivery
//! test pins an inferred value.
use retrace_arch::{KeventQos, EVENT_MANAGER_QOS, EVFILT_TIMER, EV_ADD, EV_ENABLE, EV_ONESHOT, KQINIT,
                   MANAGER_POKE, SIG_BLOCK, TIMER_IDENT_BASE, USER_WAKE_EVENT};
use retrace_box::kq::Manager;
use retrace_box::thread::{BlockReason, ThreadState};
use retrace_box::Box_;

const ENTRY: u64 = 0x2222;
const PTHSIZE: u64 = 0x2A10;

fn tb() -> Box_ {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::SPINLOOP).unwrap());
    Box_::load(&loaded)
}

/// A static box after `bsdthread_register` and M45's init: libdispatch's state when it first pokes
/// its manager. `e` is a scratch entry on the static stack page (the MMU is off, so VA == IPA).
fn inited() -> (Box_, u64) {
    let mut b = tb();
    b.guest_bsdthread_register([0x1111, ENTRY, PTHSIZE, 0, 0, 0, 0, 0]);
    let e = b.stack_top() - 0x200;
    b.poke_guest(e, &KQINIT.to_bytes());
    assert_eq!(b.guest_kevent_qos([0xffff_ffff, e, 1, 0, 0, 0, 0, 0x21]), Ok(0));
    (b, e)
}

fn poke(b: &mut Box_, e: u64) -> Result<u64, String> {
    b.poke_guest(e, &MANAGER_POKE.to_bytes());
    b.guest_kevent_qos([0xffff_ffff, e, 1, e + 0x100, 16, 0, 0, 0x23])
}

/// `inited()` plus the first poke: the manager spawned as thread 1. Returns its pthread.
fn spawned() -> (Box_, u64, u64) {
    let (mut b, e) = inited();
    assert_eq!(poke(&mut b, e), Ok(0));
    let pthread = b.threads().ctx_of(1).regs.x[0];
    (b, e, pthread)
}

fn kevent_return(list: u64, n: u64) -> [u64; 8] { [0x40, list, n, 0, 0, 0, 0, 0] }

#[test]
fn a_poke_spawns_the_manager_with_the_first_use_register_block_and_the_user_event() {
    let (b, _, pthread) = spawned();
    assert_eq!(b.threads().len(), 2);
    assert_eq!(b.dbg_kq().manager(), Manager::Bound(1));
    assert_eq!(b.threads().current(), 0, "the poke returns to main: nothing switches until main blocks");
    let ctx = b.threads().ctx_of(1);
    assert_eq!((ctx.regs.pc, ctx.elr), (ENTRY, ENTRY), "entered at the registered wqthread entry");
    assert_eq!(ctx.regs.x[1], 0x0BAD_7000 | (1 << 2) | 3, "the kport, GUEST_THREAD_PORT_BASE | (tid << 2) | 3");
    assert_eq!(ctx.regs.x[2], pthread - 0x8_0000, "the stack's low end, 512 KiB below the struct");
    assert_eq!(ctx.regs.x[3], pthread - 0x480, "the event list, at self - 16 x 72");
    assert_eq!(ctx.regs.x[4], 0x3C_4008, "first use: TSD_BASE_SET|EVENT_MANAGER|KEVENT|NEWSPI|PRIO_QOS|8");
    assert_eq!(ctx.regs.x[5], 1, "one event");
    assert!(ctx.regs.x[6..].iter().all(|&r| r == 0), "every other register is zero, as the kernel sets it");
    assert_eq!(ctx.regs.sp_el0, pthread - 0x480, "with no data payload, sp is the list");
    assert_eq!(ctx.tpidrro_el0, pthread + 0xe0, "TPIDRRO_EL0 = pthread + PTHREAD_TSD_OFF");
    assert_eq!(b.read_bytes_for_test(pthread - 0x480, 72), USER_WAKE_EVENT.to_bytes());
    assert!(!b.dbg_kq().has_pending(), "the trigger was delivered in the upcall");
}

/// Review Focus 2.
#[test]
fn a_second_poke_while_the_manager_is_bound_spawns_nothing() {
    let (mut b, e, _) = spawned();
    assert_eq!(poke(&mut b, e), Ok(0));
    assert_eq!(b.threads().len(), 2, "no second manager");
    assert_eq!(b.dbg_kq().manager(), Manager::Bound(1));
    assert!(b.dbg_kq().has_pending(), "the trigger waits for the manager's next scan");
}

#[test]
fn a_kevent_return_with_nothing_pending_parks_the_manager_on_its_svc() {
    let (mut b, _, pthread) = spawned();
    b.switch_to_thread(1);
    assert_eq!(b.try_workq_kernreturn(kevent_return(pthread - 0x480, 0)), Ok(0));
    assert_eq!(b.threads().state_of(1), ThreadState::Blocked(BlockReason::Parked));
    assert_eq!(b.dbg_kq().manager(), Manager::Unbound(1));
    b.set_x0_err_and_return(0, false);
    b.schedule_after_block();
    assert_eq!(b.threads().current(), 0, "the park hands the vCPU back to main");
    assert_eq!(b.threads().ctx_of(1).elr, ENTRY - 4, "parked on its svc, never resumable into a return");
}

#[test]
fn a_poke_to_a_parked_manager_reenters_it_with_the_reuse_flags() {
    let (mut b, e, pthread) = spawned();
    b.switch_to_thread(1);
    b.try_workq_kernreturn(kevent_return(pthread - 0x480, 0)).unwrap();
    b.set_x0_err_and_return(0, false);
    b.schedule_after_block();
    assert_eq!(poke(&mut b, e), Ok(0));
    assert_eq!(b.threads().state_of(1), ThreadState::Runnable, "unparked");
    assert_eq!(b.threads().len(), 2, "re-entered, not replaced");
    assert_eq!(b.dbg_kq().manager(), Manager::Bound(1));
    let ctx = b.threads().ctx_of(1);
    assert_eq!((ctx.regs.pc, ctx.regs.x[0], ctx.regs.x[3], ctx.regs.x[5]), (ENTRY, pthread, pthread - 0x480, 1));
    assert_eq!(ctx.regs.x[4], 0x1E_4008, "reuse: REUSE in place of TSD_BASE_SET");
    assert_eq!(b.read_bytes_for_test(pthread - 0x480, 72), USER_WAKE_EVENT.to_bytes());
}

/// Review Focus 1: events pending at the return are redelivered on the same thread, through the
/// ordinary return write. The box installs the block, and `set_x0_err_and_return(self)` completes
/// it, which resolves spec §3d's hazard.
#[test]
fn a_kevent_return_with_a_trigger_pending_redelivers_on_the_same_thread() {
    let (mut b, e, pthread) = spawned();
    assert_eq!(poke(&mut b, e), Ok(0)); // pending while bound
    b.switch_to_thread(1);
    let rc = b.try_workq_kernreturn(kevent_return(pthread - 0x480, 0)).unwrap();
    assert_eq!(rc, pthread, "the redelivery returns self, so the return write sets x0 = self");
    b.set_x0_err_and_return(rc, false);
    assert_eq!(b.threads().state_of(1), ThreadState::Runnable, "not parked");
    assert_eq!(b.dbg_kq().manager(), Manager::Bound(1));
    let ctx = b.thread_ctx(1).unwrap(); // the current thread: read live
    assert_eq!((ctx.regs.pc, ctx.regs.x[0], ctx.regs.x[5], ctx.regs.sp_el0), (ENTRY, pthread, 1, pthread - 0x480));
    // Inferred, not measured: t0 M2 saw no native redelivery (see the module doc).
    assert_eq!(ctx.regs.x[4], 0x1E_0000, "redelivery: REUSE|EVENT_MANAGER|KEVENT|NEWSPI, no PRIO_QOS");
    assert_eq!(b.read_bytes_for_test(pthread - 0x480, 72), USER_WAKE_EVENT.to_bytes());
}

#[test]
fn a_kevent_return_from_a_thread_that_is_not_the_bound_manager_is_refused() {
    let (mut b, _, pthread) = spawned();
    let err = b.try_workq_kernreturn(kevent_return(pthread - 0x480, 0)).unwrap_err();
    assert!(err.contains("from thread 0, which is not the bound event manager"), "{err}");
}

/// A manager parked on its `svc`, the vCPU back on main: the state
/// `a_kevent_return_with_nothing_pending_parks_the_manager_on_its_svc` pins.
fn parked() -> (Box_, u64) {
    let (mut b, e, pthread) = spawned();
    b.switch_to_thread(1);
    b.try_workq_kernreturn(kevent_return(pthread - 0x480, 0)).unwrap();
    b.set_x0_err_and_return(0, false);
    b.schedule_after_block();
    (b, e)
}

/// `sys/signal.h`.
const SIGUSR1: u64 = 30;

/// M46 final review I2. `should_pend_for` pends a signal on a parked manager, because it is
/// `Blocked`. The poke's re-entry would make it Runnable and replace its context, leaving the bit
/// set where `assert_no_stranded_signals` cannot see it, so the unpark refuses.
#[test]
#[should_panic(expected = "M46: unpark of parked workqueue thread 1 with signal set 0x20000000 pending")]
fn a_poke_to_a_parked_manager_with_a_signal_pending_on_it_is_refused() {
    let (mut b, e) = parked();
    assert!(b.should_pend_for(1, SIGUSR1), "the raise path's own predicate pends on a parked target");
    b.threads_mut().pend(1, SIGUSR1);
    let _ = poke(&mut b, e);
}

/// The same refusal for a signal the manager's mask blocks, which `peek_deliverable` does not see.
/// The mask on a workqueue thread is unmeasured, so the unpark checks the whole pending set.
#[test]
#[should_panic(expected = "M46: unpark of parked workqueue thread 1 with signal set 0x20000000 pending")]
fn a_poke_to_a_parked_manager_with_a_masked_signal_pending_on_it_is_refused_too() {
    let (mut b, e) = parked();
    b.threads_mut().set_mask_of(1, SIG_BLOCK, 1 << (SIGUSR1 - 1));
    b.threads_mut().pend(1, SIGUSR1);
    assert_eq!(b.threads().peek_deliverable(1), None, "masked, so not deliverable");
    let _ = poke(&mut b, e);
}

/// M46 §7: the refusal names the fflags, and it comes before any clock read, so it reaches a box
/// with no commpage.
#[test]
fn a_kevent_return_carrying_a_wall_timer_is_refused_naming_its_fflags() {
    let (mut b, _, pthread) = spawned();
    b.switch_to_thread(1);
    let wall = KeventQos { ident: TIMER_IDENT_BASE | 6, filter: EVFILT_TIMER, flags: EV_ADD | EV_ENABLE | EV_ONESHOT,
                           qos: EVENT_MANAGER_QOS, udata: 0x6c850, fflags: 0x9c, xflags: 0, data: 1, ext: [0; 4] };
    b.poke_guest(pthread - 0x480, &wall.to_bytes());
    let err = b.try_workq_kernreturn(kevent_return(pthread - 0x480, 1)).unwrap_err();
    assert!(err.contains("M46: unmeasured KEVENT_RETURN change: changelist[0].fflags is 0x9c"), "{err}");
}
