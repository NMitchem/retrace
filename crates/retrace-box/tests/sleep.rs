//! M49 Task 5, box level: `Box_::guest_semwait_signal`, `Box_::guest_clock_get_time` and the sleep
//! model's field on a static box. A static box has no commpage, so any read of the guest clock
//! panics with M46's "the guest clock was read on a box with no commpage": that is what lets these
//! tests prove a refusal comes before the clock is read. The one test that needs a sleep admitted
//! loads `hello_dyn` instead (the `gkq.rs` pattern), because admitting one reads the clock. The
//! sleep's arithmetic is `sleep.rs`'s unit tests; the wake, the idle jump and the clock's values
//! are `sleep_e2e`'s, on a dynamic guest.
use std::panic::{catch_unwind, AssertUnwindSafe};

use retrace_arch::{SYS___SEMWAIT_SIGNAL, SYS___SEMWAIT_SIGNAL_NOCANCEL};
use retrace_box::sleep::{CALENDAR_CLOCK, SYSTEM_CLOCK};
use retrace_box::thread::{BlockReason, ThreadCtx, ThreadState};
use retrace_box::Box_;

fn tb() -> Box_ {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::SPINLOOP).unwrap());
    Box_::load(&loaded)
}

fn dynbox() -> Box_ {
    let exe = retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO_DYN).unwrap());
    let dyld = retrace_guest::parse_macho(retrace_guest::slice_arm64e(&std::fs::read(retrace_guest::DYLD_PATH).unwrap()));
    Box_::load_dynamic(&exe, &dyld, &["hello_dyn".to_string()])
}

/// libc `nanosleep`'s call for 1 µs on `cond` (sleep-report.md §1).
fn nanosleep_on(cond: u64) -> [u64; 8] { [cond, 0, 1, 1, 0, 1000, 0, 0] }

/// S4 (R8): every refused shape returns its text, leaves the thread runnable and the model as it
/// was, and reads no clock. Reading it here would panic, so a check moved after `now_guest()`
/// turns this test red.
#[test]
fn every_refusal_changes_nothing_and_reads_no_clock() {
    let mut b = tb();
    let model = b.dbg_sleep().clone();
    let with = |i: usize, v: u64| { let mut a = nanosleep_on(0x1203); a[i] = v; a };
    for (num, args, want) in [
        (SYS___SEMWAIT_SIGNAL_NOCANCEL, nanosleep_on(0x1203), "M49: semwait __semwait_signal_nocancel (423)"),
        (SYS___SEMWAIT_SIGNAL, with(0, 0), "M49: semwait cond_sem 0"),
        (SYS___SEMWAIT_SIGNAL, with(2, 0), "M49: semwait timeout 0"),
        (SYS___SEMWAIT_SIGNAL, with(3, 0), "M49: semwait relative 0"),
        (SYS___SEMWAIT_SIGNAL, with(1, 0x1403), "M49: semwait mutex_sem 0x1403"),
        (SYS___SEMWAIT_SIGNAL, with(5, 1_000_000_000), "M49: semwait tv_nsec 1000000000"),
    ] {
        let e = b.guest_semwait_signal(num, args).expect_err(want);
        assert!(e.starts_with(want), "{want}: {e}");
        assert_eq!(b.threads().state_of(0), ThreadState::Runnable, "{want}: the caller never blocked");
        assert_eq!(b.dbg_sleep(), &model, "{want}: the model is unchanged");
    }
}

/// S4: a port the guest passed to either semaphore trap is a semaphore a signal could wake early,
/// so a sleep on it is refused; a port neither trap touched is admitted.
#[test]
fn a_port_either_semaphore_trap_touched_is_refused_as_a_sleep() {
    let mut b = tb();
    let tid = b.threads_mut().spawn(ThreadCtx::zeroed(), (0x3020_0000, 0x8000));
    b.threads_mut().switch_to(tid);
    b.guest_sem_wait([0x1403, 0, 0, 0, 0, 0, 0, 0]);           // thread 1 waits on 0x1403
    b.threads_mut().switch_to(0);
    assert_eq!(b.guest_sem_signal([0x1403, 0, 0, 0, 0, 0, 0, 0]), (0, vec![tid]));
    assert_eq!(b.guest_sem_signal([0x1303, 0, 0, 0, 0, 0, 0, 0]), (0, vec![]), "a signal nobody waits for");
    for port in [0x1403, 0x1303] {
        let e = b.guest_semwait_signal(SYS___SEMWAIT_SIGNAL, nanosleep_on(port)).unwrap_err();
        assert!(e.starts_with(&format!("M49: semwait cond_sem {port:#x} was used with semaphore_wait_trap (-36)")), "{e}");
    }
    assert_eq!(b.dbg_sleep().admit(SYS___SEMWAIT_SIGNAL, nanosleep_on(0x1203)), Ok(24), "libc's clock_sem is admitted");
}

/// Ruling T5-I2, S4's converse: once an admitted sleep has used a `cond_sem`, either semaphore trap
/// on it is refused by value, naming the trap and the port, before anything moves: the sleep has
/// no semaphore wake, so the trap's effect on it would be lost with record and replay agreeing.
/// `semaphore_destroy` (3419) is refused through the same check, which both Forward arms call. A
/// port no sleep used is untouched.
#[test]
fn a_semaphore_trap_on_a_cond_sem_an_admitted_sleep_used_is_refused_by_value() {
    let mut b = dynbox();
    assert_eq!(b.guest_semwait_signal(SYS___SEMWAIT_SIGNAL, nanosleep_on(0x1203)), Ok(0));
    assert!(matches!(b.threads().state_of(0), ThreadState::Blocked(BlockReason::Sleep { .. })), "main sleeps");
    let model = b.dbg_sleep().clone();
    let refusal = |call: &str| format!(
        "M49: semwait {call} on port 0x1203, the cond_sem of an admitted __semwait_signal: the model's \
         sleep has no semaphore wake, so what this does to the sleep is not modelled.");
    for call in ["semaphore_signal_trap (-33)", "semaphore_wait_trap (-36)"] {
        let args = [0x1203, 0, 0, 0, 0, 0, 0, 0];
        let r = catch_unwind(AssertUnwindSafe(|| {
            if call.starts_with("semaphore_wait") { b.guest_sem_wait(args); } else { b.guest_sem_signal(args); }
        }));
        let e = r.expect_err(call);
        assert_eq!(e.downcast_ref::<String>().map(String::as_str), Some(refusal(call).as_str()), "{call}");
        assert_eq!(b.dbg_sleep(), &model, "{call}: the model is unchanged");
        assert!(matches!(b.threads().state_of(0), ThreadState::Blocked(BlockReason::Sleep { .. })),
                "{call}: the sleeper is untouched");
    }
    assert_eq!(b.check_sleep_semaphore("semaphore_destroy (3419)", 0x1203),
               Err(refusal("semaphore_destroy (3419)")));
    assert_eq!(b.check_sleep_semaphore("semaphore_destroy (3419)", 0x1303), Ok(()), "a port no sleep used");
    assert_eq!(b.guest_sem_signal([0x1303, 0, 0, 0, 0, 0, 0, 0]), (0, vec![]), "a port no sleep used");
}

/// S2 (R7): `clock_get_time` answers only a SYSTEM_CLOCK port a 206 reply named. Each refusal
/// comes before the clock is read (a static box would panic reading it).
#[test]
fn clock_get_time_refuses_every_port_but_a_learned_system_clock_one() {
    let mut b = tb();
    let e = b.guest_clock_get_time(0x903).unwrap_err();
    assert!(e.starts_with("M49: clock_get_time to port 0x903, which no host_get_clock_service (206) reply named"), "{e}");
    b.learn_clock_port(0x903, CALENDAR_CLOCK);
    let e = b.guest_clock_get_time(0x903).unwrap_err();
    assert!(e.starts_with("M49: clock_get_time on port 0x903, a CALENDAR_CLOCK (1) port"), "{e}");
    b.learn_clock_port(0x903, 2);
    let e = b.guest_clock_get_time(0x903).unwrap_err();
    assert!(e.starts_with("M49: clock_get_time on port 0x903, clock id 2"), "{e}");
    b.learn_clock_port(0x903, SYSTEM_CLOCK);
    assert_eq!(b.dbg_sleep().clock_of(0x903), Some(SYSTEM_CLOCK), "a name learned again takes its latest clock");
}

/// The `sleep` field through `checkpoint`/`from_checkpoint`. Dropped, a seek past libc's init
/// would forget the clock port, and the next `sleep`'s `clock_get_time` would be refused on replay
/// (sleep_e2e's seek test is the end-to-end witness).
#[test]
fn the_learned_ports_and_semaphores_survive_a_checkpoint() {
    let mut b = tb();
    b.learn_clock_port(0x903, SYSTEM_CLOCK);
    b.learn_clock_port(0xa03, CALENDAR_CLOCK);
    b.guest_sem_signal([0x1303, 0, 0, 0, 0, 0, 0, 0]);
    b.dbg_sleep_mut().note_sleep(0x1203);   // as an admitted sleep would (Ruling T5-I2)
    let model = b.dbg_sleep().clone();
    let st = b.checkpoint();
    drop(b); // one VM per process
    let r = Box_::from_checkpoint(&st);
    assert_eq!(r.dbg_sleep(), &model, "the model is carried (BoxState::sleep)");
    assert_eq!(r.dbg_sleep().clock_of(0x903), Some(SYSTEM_CLOCK));
    assert!(r.dbg_sleep().admit(SYS___SEMWAIT_SIGNAL, nanosleep_on(0x1303)).is_err(), "the semaphore is remembered");
    assert!(r.check_sleep_semaphore("semaphore_wait_trap (-36)", 0x1203).is_err(), "the sleep's cond_sem is remembered");
    assert!(r.dbg_internal_state().contains("sleep=SleepModel"), "{}", r.dbg_internal_state());
}

/// S5 (R9): a signal pended on a sleeper is never swallowed. Its wake aborts (`sleep_e2e`'s
/// `sigsleeper`), and a sleeper nothing wakes fails loud at exit, as every blocked thread does.
#[test]
#[should_panic(expected = "still Blocked with pending signal")]
fn a_signal_pended_on_a_sleeper_that_never_wakes_fails_loud_at_exit() {
    let mut b = tb();
    let tid = b.threads_mut().spawn(ThreadCtx::zeroed(), (0x3020_0000, 0x8000));
    b.threads_mut().switch_to(tid);
    b.threads_mut().block(BlockReason::Sleep { deadline: 1 << 40 });
    b.threads_mut().switch_to(0);
    assert!(b.should_pend_for(tid, 30), "a sleeper is Blocked, so a raise pends");
    b.threads_mut().pend(tid, 30);
    b.assert_no_stranded_signals();
}
