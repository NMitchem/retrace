//! M49 §3f (S2–S4): the pure half of the sleep model. It holds `__semwait_signal`'s accepted shape
//! and its refusals by value, the clock ports learned from `host_get_clock_service` replies, the
//! ports the guest has used with the mach semaphore traps, and the 24 MHz arithmetic the sleep and
//! `clock_get_time` share. No guest memory and no clock: `Box_::guest_semwait_signal` and
//! `Box_::guest_clock_get_time` add `now_guest()` and the thread table.
use std::collections::{BTreeMap, BTreeSet};

/// `SYSTEM_CLOCK` (SDK `mach/clock_types.h`): uptime, `mach_absolute_time × 125/3` ns on this host
/// (`sleep-report.md`, native). The one clock `clock_get_time` answers (R7).
pub const SYSTEM_CLOCK: u32 = 0;
/// `CALENDAR_CLOCK` (SDK `mach/clock_types.h`): wall time, which retrace does not model (R7).
pub const CALENDAR_CLOCK: u32 = 1;

const NSEC_PER_SEC: u64 = 1_000_000_000;

/// Nanoseconds to guest-clock ticks at 24 MHz, truncating (M48 plan F6: `hw.tbfrequency` is
/// 24 000 000, so 1 ns is 0 ticks and 1 µs is 24).
pub fn ns_to_ticks(ns: u64) -> u64 { (ns as u128 * 3 / 125) as u64 }

/// A guest-clock reading as SYSTEM_CLOCK's `mach_timespec_t` (SDK `mach/clock_types.h`:
/// `unsigned int tv_sec; clock_res_t tv_nsec`): ticks × 125/3 ns, truncating, the conversion
/// native's SYSTEM_CLOCK makes of `mach_absolute_time` (`sleep-report.md`).
pub fn timespec_of_ticks(ticks: u64) -> (u32, i32) {
    let ns = ticks as u128 * 125 / 3;
    ((ns / NSEC_PER_SEC as u128) as u32, (ns % NSEC_PER_SEC as u128) as i32)
}

/// The call's name, for a refusal and for replay's divergence text.
pub fn call_name(num: u64) -> &'static str {
    if num == retrace_arch::SYS___SEMWAIT_SIGNAL_NOCANCEL { "__semwait_signal_nocancel" } else { "__semwait_signal" }
}

/// Box state the sleep model rebuilds from the guest's own calls on both sides and carries through
/// every rebuild path (`BoxState::sleep`). Nothing in it is recorded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SleepModel {
    /// Port name → clock id, for every clock port a successful `host_get_clock_service` (206)
    /// reply named (R7). The name is retrace's own (the 206 is forwarded), recorded in the 206's
    /// reply and applied verbatim on replay, so both sides learn the same name. A name learned
    /// again takes the clock id of its latest reply: a name is only reused after the guest
    /// deallocated it.
    clock_ports: BTreeMap<u64, u32>,
    /// Every port the guest has passed to `semaphore_wait_trap` (-36) or `semaphore_signal_trap`
    /// (-33). A `__semwait_signal` on one of these is a real semaphore wait that a signal could
    /// end early, which the model does not have (S4).
    sem_ports: BTreeSet<u64>,
}

impl SleepModel {
    /// M49 S2: `port` answers for clock `clock_id`.
    pub fn learn_clock_port(&mut self, port: u64, clock_id: u32) { self.clock_ports.insert(port, clock_id); }

    /// M49 S2: the clock a learned port answers for, or None for a port no 206 reply named.
    pub fn clock_of(&self, port: u64) -> Option<u32> { self.clock_ports.get(&port).copied() }

    /// M49 S4: the guest used `port` with a mach semaphore trap.
    pub fn note_semaphore(&mut self, port: u64) { self.sem_ports.insert(port); }

    /// M49 S3–S4 (R8): admit a `__semwait_signal` (334) or refuse it by value. The accepted shape
    /// is libc `nanosleep`'s (`sleep-report.md` §1): `mutex_sem` 0, `timeout` 1, `relative` 1,
    /// `0 <= tv_sec <= u32::MAX` and `0 <= tv_nsec < 10^9`, on a `cond_sem` no semaphore trap has
    /// touched. Returns the sleep's length in ticks.
    ///
    /// The four `int` arguments and `tv_nsec` are read from the low 32 bits of their registers,
    /// as the kernel's argument struct reads them; `tv_sec` is the full `int64_t`. The number is
    /// checked first, then the arguments in order, then the semaphore set, so a call wrong in two
    /// fields names the earlier one. `tv_sec` above
    /// `u32::MAX` is refused although native accepts it: `mach_timespec_t`'s `tv_sec` is
    /// `unsigned int`, and the kernel's answer to a longer wait is unmeasured.
    pub fn admit(&self, num: u64, args: [u64; 8]) -> Result<u64, String> {
        let shown = format!("args [{:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x}]",
                            args[0], args[1], args[2], args[3], args[4], args[5]);
        if num != retrace_arch::SYS___SEMWAIT_SIGNAL {
            return Err(format!(
                "M49: semwait {} ({num}) is not modelled: only __semwait_signal (334)'s relative-timeout \
                 shape is, the one every measured sleep path issues (M49 §2e). {shown}", call_name(num)));
        }
        let (cond, mutex, timeout, relative) = (args[0] as u32, args[1] as u32, args[2] as u32, args[3] as u32);
        let (sec, nsec) = (args[4] as i64, args[5] as i32);
        if cond == 0 {
            return Err(format!("M49: semwait cond_sem 0: native answers EINVAL (sleep-sw.c `nosem`), \
                                which is not modelled. {shown}"));
        }
        if mutex != 0 {
            return Err(format!("M49: semwait mutex_sem {mutex:#x}: a signal-and-wait on two semaphores, \
                                which is not modelled. {shown}"));
        }
        if timeout != 1 {
            return Err(format!("M49: semwait timeout {timeout}: only timeout 1 is modelled; timeout 0 is an \
                                untimed wait that only a semaphore signal ends. {shown}"));
        }
        if relative != 1 {
            return Err(format!("M49: semwait relative {relative}: an absolute deadline on the calendar \
                                clock (wall time, sleep-sw.c `abs(cal)`), which is not modelled. {shown}"));
        }
        if !(0..=u32::MAX as i64).contains(&sec) {
            return Err(format!("M49: semwait tv_sec {sec}: outside 0..={} (mach_timespec_t's unsigned \
                                tv_sec); the kernel's answer is unmeasured. {shown}", u32::MAX));
        }
        if !(0..NSEC_PER_SEC as i32).contains(&nsec) {
            return Err(format!("M49: semwait tv_nsec {nsec}: outside 0..{NSEC_PER_SEC}; native answers \
                                EINVAL (sleep_dyn `refuse nsec`), which is not modelled. {shown}"));
        }
        if self.sem_ports.contains(&(cond as u64)) {
            return Err(format!("M49: semwait cond_sem {cond:#x} was used with semaphore_wait_trap (-36) or \
                                semaphore_signal_trap (-33): a semaphore a signal could wake early, which \
                                is not modelled. {shown}"));
        }
        Ok(ns_to_ticks(sec as u64 * NSEC_PER_SEC + nsec as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use retrace_arch::{SYS___SEMWAIT_SIGNAL, SYS___SEMWAIT_SIGNAL_NOCANCEL};

    /// libc `nanosleep`'s call (`sleep-report.md` §1): clock_sem, 0, 1, 1, sec, nsec.
    fn nanosleep(sec: u64, nsec: u64) -> [u64; 8] { [0x1203, 0, 1, 1, sec, nsec, 0, 0] }

    /// R6 at 24 MHz (M48 plan F6): what each measured sleep blocks for, in ticks.
    #[test]
    fn the_relative_shape_converts_at_24_mhz_truncating() {
        let m = SleepModel::default();
        for (sec, nsec, ticks) in [(1, 0, 24_000_000), (0, 1_000_000, 24_000), (0, 5_000_000, 120_000),
                                   (0, 100_000, 2_400), (0, 1_000, 24), (0, 41, 0), (0, 42, 1), (0, 1, 0),
                                   (0, 0, 0), (2, 999_999_999, 71_999_999)] {
            assert_eq!(m.admit(SYS___SEMWAIT_SIGNAL, nanosleep(sec, nsec)), Ok(ticks), "{sec} s {nsec} ns");
        }
    }

    /// R8: every shape the model has not measured is refused by value, naming the field. One row per
    /// refusal, so deleting any one of them turns this test red.
    #[test]
    fn every_unmodelled_shape_is_refused_by_value_naming_its_field() {
        let mut m = SleepModel::default();
        m.note_semaphore(0x1303);
        let with = |i: usize, v: u64| { let mut a = nanosleep(0, 1000); a[i] = v; a };
        let cases: [(u64, [u64; 8], &str); 10] = [
            (SYS___SEMWAIT_SIGNAL_NOCANCEL, nanosleep(0, 1000), "M49: semwait __semwait_signal_nocancel (423) is not modelled"),
            (SYS___SEMWAIT_SIGNAL, with(0, 0), "M49: semwait cond_sem 0: native answers EINVAL"),
            (SYS___SEMWAIT_SIGNAL, with(2, 0), "M49: semwait timeout 0: only timeout 1 is modelled"),
            (SYS___SEMWAIT_SIGNAL, with(2, 2), "M49: semwait timeout 2:"),
            (SYS___SEMWAIT_SIGNAL, with(3, 0), "M49: semwait relative 0: an absolute deadline"),
            (SYS___SEMWAIT_SIGNAL, with(1, 0x1403), "M49: semwait mutex_sem 0x1403:"),
            (SYS___SEMWAIT_SIGNAL, with(4, u64::MAX), "M49: semwait tv_sec -1:"),
            (SYS___SEMWAIT_SIGNAL, with(4, 1 << 32), "M49: semwait tv_sec 4294967296:"),
            (SYS___SEMWAIT_SIGNAL, with(5, 1_000_000_000), "M49: semwait tv_nsec 1000000000:"),
            (SYS___SEMWAIT_SIGNAL, with(0, 0x1303), "M49: semwait cond_sem 0x1303 was used with semaphore_wait_trap (-36)"),
        ];
        for (num, args, want) in cases {
            let e = m.admit(num, args).expect_err(want);
            assert!(e.starts_with(want), "{want}: {e}");
        }
        let e = m.admit(SYS___SEMWAIT_SIGNAL, with(5, u32::MAX as u64)).unwrap_err();
        assert!(e.starts_with("M49: semwait tv_nsec -1:"), "tv_nsec is the kernel's int32_t: {e}");
        assert_eq!(m.admit(SYS___SEMWAIT_SIGNAL, nanosleep(u32::MAX as u64, 0)), Ok(u32::MAX as u64 * 24_000_000),
                   "the largest tv_sec is accepted");
        let mut both = with(1, 0x1403);
        both[2] = 0;
        let e = m.admit(SYS___SEMWAIT_SIGNAL, both).unwrap_err();
        assert!(e.starts_with("M49: semwait mutex_sem 0x1403:"), "argument order, mutex_sem before timeout: {e}");
    }

    /// S2: SYSTEM_CLOCK's reply is the guest clock at 125/3 ns per tick, truncating.
    #[test]
    fn the_clock_reading_is_the_guest_clock_at_125_over_3_ns_per_tick() {
        assert_eq!(timespec_of_ticks(0), (0, 0));
        assert_eq!(timespec_of_ticks(3), (0, 125));
        assert_eq!(timespec_of_ticks(24_000_000), (1, 0));
        assert_eq!(timespec_of_ticks(24_000_001), (1, 41), "41.67 ns truncates to 41");
        assert_eq!(timespec_of_ticks(1 << 48), (11_728_124, 29_610_666),
                   "2^48 ticks: 11 728 124.029 610 666 s, computed in u128 so ticks × 125 cannot overflow");
    }

    /// R7: a learned port answers for its clock; a name learned again takes its latest clock.
    #[test]
    fn a_learned_port_answers_its_clock_and_a_relearned_name_takes_the_latest() {
        let mut m = SleepModel::default();
        assert_eq!(m.clock_of(0x903), None, "nothing learned yet");
        m.learn_clock_port(0x903, SYSTEM_CLOCK);
        m.learn_clock_port(0xa03, CALENDAR_CLOCK);
        assert_eq!((m.clock_of(0x903), m.clock_of(0xa03), m.clock_of(0xb03)), (Some(0), Some(1), None));
        m.learn_clock_port(0xa03, SYSTEM_CLOCK);
        assert_eq!(m.clock_of(0xa03), Some(0));
    }
}
