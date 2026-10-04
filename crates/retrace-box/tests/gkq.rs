//! M48 §3c–§3d, box level: guest kqueues and the deadline queue, with the kernel's side driven by
//! hand as `kqmanager.rs` drives M46's manager. Review Focus 1 is pinned here against guest
//! memory, which the pure `gkq.rs` never sees. Review Focus 2 is pinned here with a reply that
//! differs from the 0 a blocking landmark writes: a kevent deadline wake always answers 0, so no
//! fixture can show a reply lost to a stale saved context.
//!
//! K10: a static box has no commpage and so no guest clock, so the static tests use NULL or zero
//! timeouts only. The two deadline tests load `hello_dyn` (the `stackgrow.rs` pattern); its stage 1
//! is identity-mapped, so a `guest_mmap` IPA is also the VA `guest_kevent` reads.
use retrace_arch::{Kevent, EVFILT_READ, EVFILT_USER, EV_ADD, EV_CLEAR, NOTE_FFCOPY, NOTE_TRIGGER,
                   PSTATE_C, SYS_READ, SYS_WRITE, TIMER_IDENT_BASE};
use retrace_box::thread::{BlockReason, ThreadCtx, ThreadState};
use retrace_box::Box_;
use retrace_guest::{parse_macho, slice_arm64e, DYLD_PATH, HELLO_DYN, SPINLOOP};

const KQ: u64 = 3;
/// `kqueue` and `pipe` (SDK `sys/syscall.h`), as the generic arm hands them to the hook.
const SYS_KQUEUE: u64 = 362;
const SYS_PIPE: u64 = 42;
const RW: u64 = 3;
const ANON: u64 = 0x1002; // MAP_ANON | MAP_PRIVATE

fn tb() -> Box_ {
    Box_::load(&parse_macho(&std::fs::read(SPINLOOP).unwrap()))
}

fn dynbox() -> Box_ {
    let exe = parse_macho(&std::fs::read(HELLO_DYN).unwrap());
    let dyld = parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap()));
    Box_::load_dynamic(&exe, &dyld, &["hello_dyn".to_string()])
}

/// A scratch page, and kqueue `KQ` made as a forwarded `kqueue()` makes one: a guest fd slot, then
/// the hook.
fn setup(b: &mut Box_) -> u64 {
    let base = b.guest_mmap(0, 0x4000, RW, ANON).unwrap();
    assert_eq!(b.fds_mut().alloc(), KQ);
    b.note_fd_effects(SYS_KQUEUE, [0; 8], KQ, 0, false).unwrap();
    base
}

fn kev(ident: u64, filter: i16, flags: u16, fflags: u32, data: i64, udata: u64) -> Kevent {
    Kevent { ident, filter, flags, fflags, data, udata }
}

/// `kevent(KQ, at, changes.len(), at + 0x100, nevents, timeout)`, the changes poked at `at`.
fn kevent(b: &mut Box_, at: u64, changes: &[Kevent], nevents: u64, timeout: u64) -> Result<(u64, bool), String> {
    for (i, c) in changes.iter().enumerate() { b.poke_guest(at + 32 * i as u64, &c.to_bytes()); }
    let list = if changes.is_empty() { 0 } else { at };
    b.guest_kevent([KQ, list, changes.len() as u64, at + 0x100, nevents, timeout, 0, 0])
}

fn spawn(b: &mut Box_) -> usize {
    let ctx = ThreadCtx { spsr: b.spsr(), ..ThreadCtx::zeroed() };
    b.threads_mut().spawn(ctx, (0, 0))
}

fn timespec(b: &mut Box_, at: u64, sec: u64, nsec: u64) -> u64 {
    b.poke_guest(at, &sec.to_le_bytes());
    b.poke_guest(at + 8, &nsec.to_le_bytes());
    at
}

fn synthetic_tsc(b: &Box_) -> u64 {
    let s = b.dbg_internal_state();
    let v = s.split("synthetic_tsc=0x").nth(1).and_then(|r| r.split_whitespace().next()).unwrap();
    u64::from_str_radix(v, 16).unwrap()
}

/// Review Focus 2. `switch_to_thread` returns early for the current thread, so a reply written only
/// to its saved context would never load. The reply here, 0x2a with the carry set, differs from
/// what the blocking landmark wrote, so a lost one shows.
#[test]
fn a_wake_of_the_current_thread_writes_the_vcpu() {
    let mut b = tb();
    let base = setup(&mut b);
    b.threads_mut().block(BlockReason::Kevent { kq: KQ, deadline: None });
    b.set_x0_err_and_return(0, false);
    let stale = b.threads().ctx_of(0).regs.x[0];
    let ev = kev(7, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0x1234).to_bytes();
    b.deliver_wake(0, 0x2a, true, &[(base, ev.to_vec())]).unwrap();
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable);
    assert_eq!(b.vcpu_get_x(0), 0x2a, "the reply is on the vCPU, where the current thread reads it");
    assert_ne!(b.regs_snapshot().cpsr & PSTATE_C, 0, "err sets the carry in the live CPSR");
    assert_eq!(b.threads().ctx_of(0).regs.x[0], stale, "the current thread's table entry is stale, and not where the reply goes");
    assert_eq!(b.read_bytes_for_test(base, 32), ev);
    b.schedule_after_block();
    assert_eq!((b.threads().current(), b.vcpu_get_x(0)), (0, 0x2a),
        "the pick returns the same thread and the switch returns early: the reply survives");
}

/// Review Focus 1. libuv's `uv__kqueue_runtime_detection` passes ONE buffer as the change list and
/// the event list (M47 `node.entry.txt`), so the call must read both changes before it writes an
/// event, as the kernel's copyin loop does. Native then holds one event over slot 0 and the
/// untouched trigger in slot 1 (`native/kqdetect.out`; walls.md §1 row 1).
#[test]
fn an_event_list_aliasing_the_change_list_is_read_before_it_is_written() {
    const ID: u64 = 0x1e7e_7711;
    let mut b = tb();
    let base = setup(&mut b);
    let add = kev(ID, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0);
    let trig = kev(ID, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0);
    b.poke_guest(base, &add.to_bytes());
    b.poke_guest(base + 32, &trig.to_bytes());
    let zero = timespec(&mut b, base + 0x200, 0, 0);
    assert_eq!(b.guest_kevent([KQ, base, 2, base, 1, zero, 0, 0]), Ok((1, false)), "native's one event");
    assert_eq!(b.read_bytes_for_test(base, 32), add.to_bytes(),
        "slot 0 holds the event: the add's flags, kn_sfflags 0, kn_sdata 0 and the trigger's udata 0 (F8)");
    assert_eq!(b.read_bytes_for_test(base + 32, 32), trig.to_bytes(), "one event is one slot: slot 1 keeps the trigger");
    assert!(!b.dbg_gkq().has_events(KQ), "the trigger was read and applied, and EV_CLEAR reset the knote at its delivery");
}

/// §3c: a `NOTE_TRIGGER` from another thread, in `uv_async_send`'s shape (one change, no event
/// list), wakes the waiter with its event (F8) at the trigger, not at a later switch. It is also
/// the other half of `deliver_wake`'s contract: a thread that is not on the vCPU gets its reply in
/// its saved context (a carry set there is cleared), and the running thread's registers are
/// untouched.
#[test]
fn a_trigger_from_another_thread_wakes_the_waiter_with_its_event() {
    let mut b = tb();
    let base = setup(&mut b);
    let t1 = spawn(&mut b);
    assert_eq!(kevent(&mut b, base, &[kev(7, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0x1234)], 0, 0), Ok((0, false)));
    assert_eq!(kevent(&mut b, base, &[], 1, 0), Ok((0, false)), "a block records 0");
    assert_eq!(b.threads().state_of(0), ThreadState::Blocked(BlockReason::Kevent { kq: KQ, deadline: None }));
    b.set_x0_err_and_return(0, false);
    b.schedule_after_block();
    assert_eq!(b.threads().current(), t1);
    b.threads_mut().ctx_mut(0).regs.cpsr |= PSTATE_C;
    b.vcpu_set_x(0, 0x77);
    let trig = kev(7, EVFILT_USER, 0, NOTE_TRIGGER | NOTE_FFCOPY | 5, 9, 0x5678);
    assert_eq!(kevent(&mut b, base + 0x1000, &[trig], 0, 0), Ok((0, false)));
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable);
    let ctx = b.threads().ctx_of(0);
    assert_eq!((ctx.regs.x[0], ctx.regs.cpsr & PSTATE_C), (1, 0), "one event, carry cleared, in main's saved context");
    assert_eq!(b.vcpu_get_x(0), 0x77, "the waker's registers are untouched: its own return is the arm's to set");
    assert_eq!(b.read_bytes_for_test(base + 0x100, 32), kev(7, EVFILT_USER, EV_ADD | EV_CLEAR, 5, 9, 0x5678).to_bytes(),
        "the creating flags, kn_sfflags after NOTE_FFCOPY, kn_sdata, and the trigger's udata (F8, K9)");
    assert_eq!(b.dbg_gkq().waiter(KQ), None);
    assert!(!b.dbg_gkq().has_events(KQ), "EV_CLEAR reset the knote at the delivery");
}

/// F7: with no event list the call applies its changes and returns 0, whatever its timeout. A zero
/// timeout polls.
#[test]
fn a_call_with_no_event_list_applies_its_changes_and_never_blocks() {
    let mut b = tb();
    let base = setup(&mut b);
    let add = kev(7, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0);
    let trig = kev(7, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0);
    assert_eq!(kevent(&mut b, base, &[add, trig], 0, 0), Ok((0, false)));
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable, "no scan, so no block, with a NULL timeout");
    assert!(b.dbg_gkq().has_events(KQ), "the trigger is applied, waiting for a scan");
    let zero = timespec(&mut b, base + 0x200, 0, 0);
    assert_eq!(kevent(&mut b, base, &[], 1, zero), Ok((1, false)));
    assert_eq!(kevent(&mut b, base, &[], 1, zero), Ok((0, false)), "a zero timeout returns 0 without blocking");
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable);
}

/// K5: the kernel copies the timeout in first, so an unmapped one is EFAULT with nothing applied;
/// here the one change would be refused if it were applied.
#[test]
fn an_unmapped_timeout_answers_efault_and_applies_no_change() {
    let mut b = tb();
    let base = setup(&mut b);
    let r = kevent(&mut b, base, &[kev(7, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0)], 1, 1 << 40);
    assert_eq!(r, Ok((14, true)), "EFAULT, carry set");
    assert!(b.dbg_gkq().knote(KQ, 7, EVFILT_USER).is_none());
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable);
}

/// Review Focus 2's path, and the header's "not special-cased" rule: node's 1 ns wait converts to
/// 0 ticks (F6), so it blocks with its deadline already reached and is woken by the deadline queue
/// in the same `schedule_after_block`, with no idle jump.
#[test]
fn a_one_nanosecond_wait_is_woken_in_the_same_settle_without_a_jump() {
    let mut b = dynbox();
    let base = setup(&mut b);
    let ts = timespec(&mut b, base + 0x200, 0, 1);
    assert_eq!(kevent(&mut b, base, &[], 1, ts), Ok((0, false)), "nothing active: it blocks and records 0");
    let ThreadState::Blocked(BlockReason::Kevent { kq: KQ, deadline: Some(d) }) = b.threads().state_of(0) else {
        panic!("blocked with a deadline: {:?}", b.threads().state_of(0));
    };
    b.set_x0_err_and_return(0, false);
    let before = synthetic_tsc(&b);
    b.schedule_after_block();
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable, "the deadline {d:#x} was already due");
    assert_eq!(b.threads().current(), 0);
    assert_eq!(synthetic_tsc(&b), before, "no idle jump: the deadline queue's own pass woke it");
    assert_eq!(b.dbg_gkq().waiter(KQ), None);
}

/// §3d, R7: with nothing runnable the one jump goes to the EARLIEST deadline of all, here a
/// thread's 1 ms ahead of another's 3 ms and of an M46 timer 7 ms after that. It wakes only that
/// thread. Before M48 there was no thread deadline to jump to, and the jump went to the timer.
#[test]
fn the_idle_jump_lands_on_the_earliest_thread_deadline_before_a_later_timer() {
    let mut b = dynbox();
    let base = setup(&mut b);
    assert_eq!(b.fds_mut().alloc(), KQ + 1);
    b.note_fd_effects(SYS_KQUEUE, [0; 8], KQ + 1, 0, false).unwrap();
    let t1 = spawn(&mut b);
    let ms1 = timespec(&mut b, base + 0x200, 0, 1_000_000);
    assert_eq!(kevent(&mut b, base, &[], 1, ms1), Ok((0, false)));
    b.set_x0_err_and_return(0, false);
    b.schedule_after_block();
    assert_eq!(b.threads().current(), t1, "1 ms is not reached: thread 1 runs");
    let ms3 = timespec(&mut b, base + 0x300, 0, 3_000_000);
    assert_eq!(b.guest_kevent([KQ + 1, 0, 0, base + 0x400, 1, ms3, 0, 0]), Ok((0, false)));
    let ThreadState::Blocked(BlockReason::Kevent { deadline: Some(d1), .. }) = b.threads().state_of(t1) else {
        panic!("thread 1 blocked with a deadline: {:?}", b.threads().state_of(t1));
    };
    b.dbg_kq_mut().add_timer(TIMER_IDENT_BASE, d1 + 168_000, 0, 0).unwrap();
    b.set_x0_err_and_return(0, false);
    let before = synthetic_tsc(&b);
    b.schedule_after_block();
    assert_eq!(synthetic_tsc(&b) - before, 24_000, "F6: exactly to thread 0's 1 ms deadline (24000 ticks), no further");
    assert_eq!((b.threads().current(), b.threads().state_of(0)), (0, ThreadState::Runnable));
    assert_eq!(b.threads().state_of(t1), ThreadState::Blocked(BlockReason::Kevent { kq: KQ + 1, deadline: Some(d1) }),
        "3 ms is not reached");
    assert_eq!(b.dbg_kq().armed_count(), 1, "the later timer neither fired nor was jumped to");
}

/// R4: a write's return, seen by the hook after the generic arm, makes a watched read end readable
/// and wakes its waiter with `data` the count (F9). The read's return drains it. A read past the
/// count means a write reached the pipe by a path the hook does not see, and a watched pipe whose
/// count is lost is refused by value (K1), not answered from a wrong count.
#[test]
fn a_pipe_write_wakes_a_reader_blocked_on_its_read_end() {
    let mut b = tb();
    let base = setup(&mut b);
    let t1 = spawn(&mut b);
    let (r, w) = (b.fds_mut().alloc(), b.fds_mut().alloc());
    b.note_fd_effects(SYS_PIPE, [0; 8], r, w, false).unwrap();
    assert_eq!(kevent(&mut b, base, &[kev(r, EVFILT_READ, EV_ADD, 0, 0, 0xabc)], 0, 0), Ok((0, false)));
    assert_eq!(kevent(&mut b, base, &[], 1, 0), Ok((0, false)), "an empty pipe: it blocks");
    b.set_x0_err_and_return(0, false);
    b.schedule_after_block();
    assert_eq!(b.threads().current(), t1);
    b.note_fd_effects(SYS_WRITE, [w, base + 0x800, 5, 0, 0, 0, 0, 0], 5, 0, false).unwrap();
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable);
    assert_eq!(b.threads().ctx_of(0).regs.x[0], 1);
    assert_eq!(b.read_bytes_for_test(base + 0x100, 32), kev(r, EVFILT_READ, EV_ADD, 0, 5, 0xabc).to_bytes());
    b.note_fd_effects(SYS_READ, [r, base + 0x800, 64, 0, 0, 0, 0, 0], 5, 0, false).unwrap();
    assert_eq!(b.dbg_gkq().pipe_count(r), Some(0));
    let e = b.note_fd_effects(SYS_READ, [r, base + 0x800, 64, 0, 0, 0, 0, 0], 1, 0, false).unwrap_err();
    assert!(e.starts_with(&format!("M48: pipe {r}: the byte count is unknown")), "{e}");
}

/// K8, box level: a trigger that would wake a thread with a signal pending is refused by value and
/// leaves it blocked.
#[test]
fn a_wake_with_a_signal_pending_is_refused_and_wakes_nobody() {
    let mut b = tb();
    let base = setup(&mut b);
    let t1 = spawn(&mut b);
    assert_eq!(kevent(&mut b, base, &[kev(7, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0)], 1, 0), Ok((0, false)));
    b.set_x0_err_and_return(0, false);
    b.threads_mut().pend(0, 30);
    b.schedule_after_block();
    assert_eq!(b.threads().current(), t1);
    let e = kevent(&mut b, base + 0x1000, &[kev(7, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0)], 0, 0).unwrap_err();
    assert!(e.starts_with("M48: a signal is pending on thread 0 (set 0x20000000"), "{e}");
    assert_eq!(b.threads().state_of(0), ThreadState::Blocked(BlockReason::Kevent { kq: KQ, deadline: None }), "woke nobody");
}

/// Restore parity at box level: the kqueue, its knote and its waiter ride in `BoxState`, and a
/// trigger on the restored box wakes the restored waiter.
#[test]
fn a_blocked_kevent_survives_a_checkpoint() {
    let mut b = tb();
    let base = setup(&mut b);
    let t1 = spawn(&mut b);
    assert_eq!(kevent(&mut b, base, &[kev(7, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0x1234)], 1, 0), Ok((0, false)));
    b.set_x0_err_and_return(0, false);
    b.schedule_after_block();
    let gkq = b.dbg_gkq().clone();
    let st = b.checkpoint();
    drop(b); // one VM per process
    let mut r = Box_::from_checkpoint(&st);
    assert_eq!(r.dbg_gkq(), &gkq, "the kqueue, its knote and its waiter");
    assert_eq!(r.threads().state_of(0), ThreadState::Blocked(BlockReason::Kevent { kq: KQ, deadline: None }));
    assert_eq!(r.threads().current(), t1);
    assert_eq!(kevent(&mut r, base + 0x1000, &[kev(7, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0x1234)], 0, 0), Ok((0, false)));
    assert_eq!(r.threads().state_of(0), ThreadState::Runnable, "the restored waiter is woken");
    assert_eq!(r.read_bytes_for_test(base + 0x100, 32), kev(7, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0x1234).to_bytes());
}
