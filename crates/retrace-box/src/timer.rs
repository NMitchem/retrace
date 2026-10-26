//! M53 §3f: the pure half of `ITIMER_REAL`. One timer per process, on the guest clock (`synthetic_tsc`
//! plus the commpage offset), in 24 MHz ticks. No guest memory and no clock of its own: `Box_` passes
//! `now` in, so every answer is a function of box state that record and replay hold identically and
//! nothing is recorded (halt H5).

/// Ticks per microsecond (`hw.tbfrequency` is 24 MHz, M48 plan F6).
pub const TICKS_PER_US: u64 = 24;

/// xnu's `itimerfix` bound on seconds (t0 M8, the bound M7 measured for `select`).
pub const MAX_SECS: i64 = 100_000_000;

/// The armed state of `ITIMER_REAL`, plus the two pieces of bookkeeping its firing needs. Carried in
/// `BoxState` so a seek restores a timer the guest armed behind the checkpoint.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RealTimer {
    /// Guest-clock tick at which the timer next expires, or None while disarmed.
    deadline: Option<u64>,
    /// Ticks between expiries after the first; 0 for a one-shot.
    interval: u64,
    /// A syscall landmark has just returned, so the next settle checks the timer (M53 §3f: the timer
    /// fires at landmark finishes and idle jumps, never mid-window, whatever the debugger steps).
    check: bool,
    /// A fire whose default action ends the process, waiting for the next `run()`/`step()` to hand
    /// the recorder the stop it ends the recording at.
    terminal: Option<u64>,
}

impl RealTimer {
    /// Arm the timer to expire `value_ticks` from `now`, repeating every `interval_ticks`; a zero
    /// value disarms it. Returns the previous `(remaining, interval)` in ticks, as `remaining` would
    /// have answered at `now`.
    pub fn arm(&mut self, now: u64, value_ticks: u64, interval_ticks: u64) -> (u64, u64) {
        let old = self.remaining(now);
        if value_ticks == 0 {
            self.deadline = None;
            self.interval = 0;
        } else {
            self.deadline = Some(now.saturating_add(value_ticks));
            self.interval = interval_ticks;
        }
        old
    }

    /// `(ticks left, interval)`; `(0, 0)` while disarmed. The interval is reported as set. A timer
    /// that is armed but not yet fired reports at least one tick, as xnu reports one microsecond for
    /// an expiry its callout has not run yet.
    pub fn remaining(&self, now: u64) -> (u64, u64) {
        match self.deadline {
            None => (0, 0),
            Some(d) => (d.saturating_sub(now).max(1), self.interval),
        }
    }

    /// Has the timer expired at `now`? Consumes the expiry: a one-shot disarms, an interval re-arms
    /// from its previous deadline (`realitexpire`), and if that is still not in the future it skips
    /// the missed periods, raising ONE signal.
    pub fn due(&mut self, now: u64) -> bool {
        let Some(d) = self.deadline else { return false };
        if now < d { return false; }
        self.deadline = (now - d).checked_div(self.interval).map(|missed| d + (missed + 1) * self.interval);
        true
    }

    /// The tick the idle jump may go to, or None while disarmed.
    pub fn deadline(&self) -> Option<u64> { self.deadline }

    /// A landmark's return was applied: the next settle checks the timer.
    pub fn note_landmark(&mut self) { self.check = true; }

    /// Consume the landmark mark.
    pub fn take_check(&mut self) -> bool { std::mem::take(&mut self.check) }

    /// Remember a fire that ends the process with `sig`.
    pub fn set_terminal(&mut self, sig: u64) { self.terminal = Some(sig); }

    /// Is a fire that ends the process waiting?
    pub fn has_terminal(&self) -> bool { self.terminal.is_some() }

    /// Consume the fire that ends the process.
    pub fn take_terminal(&mut self) -> Option<u64> { self.terminal.take() }
}

/// `itimerval` is `{ timeval it_interval; timeval it_value; }`, each `{ i64 tv_sec; i32 tv_usec; pad }`.
pub const ITIMERVAL_BYTES: usize = 32;

/// xnu's `itimerfix` on one `timeval`: seconds in `0..=MAX_SECS`, microseconds in `0..1_000_000`.
/// Returns the span in ticks, or None for EINVAL.
pub fn timeval_ticks(sec: i64, usec: i32) -> Option<u64> {
    if !(0..=MAX_SECS).contains(&sec) || !(0..1_000_000).contains(&usec) { return None; }
    Some(sec as u64 * 1_000_000 * TICKS_PER_US + usec as u64 * TICKS_PER_US)
}

/// Ticks as a `(tv_sec, tv_usec)` pair, microseconds rounded up so that libc's `alarm` (`old.tv_sec
/// + (old.tv_usec != 0)`) reports the whole seconds left rounded up (t0 M8).
pub fn ticks_timeval(ticks: u64) -> (i64, i32) {
    let us = ticks.div_ceil(TICKS_PER_US);
    ((us / 1_000_000) as i64, (us % 1_000_000) as i32)
}

/// The 32 bytes `itimerval` for `(interval, value)` in ticks.
pub fn itimerval_bytes(interval: u64, value: u64) -> [u8; ITIMERVAL_BYTES] {
    let mut out = [0u8; ITIMERVAL_BYTES];
    for (i, ticks) in [interval, value].into_iter().enumerate() {
        let (s, u) = ticks_timeval(ticks);
        out[i * 16..i * 16 + 8].copy_from_slice(&s.to_le_bytes());
        out[i * 16 + 8..i * 16 + 12].copy_from_slice(&u.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_returns_the_old_remaining_and_interval() {
        let mut t = RealTimer::default();
        assert_eq!(t.arm(100, 5000, 70), (0, 0), "a disarmed timer answers 0/0");
        assert_eq!(t.arm(1000, 200, 0), (4100, 70), "the old value counts down from its own arming");
        assert_eq!(t.deadline(), Some(1200));
    }

    #[test]
    fn remaining_counts_down_with_now_and_keeps_the_interval() {
        let mut t = RealTimer::default();
        t.arm(0, 1000, 300);
        assert_eq!(t.remaining(0), (1000, 300));
        assert_eq!(t.remaining(400), (600, 300));
        assert_eq!(t.remaining(5000), (1, 300), "armed but not yet fired reports a tick, never 0");
    }

    #[test]
    fn an_interval_rearms_from_its_deadline_and_skips_missed_periods() {
        let mut t = RealTimer::default();
        t.arm(0, 100, 50);
        assert!(!t.due(99));
        assert!(t.due(100) && t.deadline() == Some(150));
        assert!(t.due(400), "one signal for the missed periods");
        assert_eq!(t.deadline(), Some(450), "and the next deadline is in the future");
        let mut one = RealTimer::default();
        one.arm(0, 10, 0);
        assert!(one.due(10) && one.deadline().is_none() && !one.due(11), "a one-shot fires once");
    }

    #[test]
    fn a_zero_value_disarms() {
        let mut t = RealTimer::default();
        t.arm(0, 100, 50);
        assert_eq!(t.arm(10, 0, 50), (90, 50));
        assert!(t.deadline().is_none() && !t.due(1_000_000));
        assert_eq!(t.remaining(0), (0, 0));
    }

    #[test]
    fn timeval_bounds_are_itimerfixs_and_the_round_trip_rounds_up() {
        assert_eq!(timeval_ticks(0, 1), Some(24));
        assert_eq!(timeval_ticks(0, 1_000_000), None);
        assert_eq!(timeval_ticks(-1, 0), None);
        assert_eq!(timeval_ticks(100_000_001, 0), None);
        assert_eq!(ticks_timeval(24_000_000 * 5), (5, 0));
        assert_eq!(ticks_timeval(24_000_000 * 5 - 1), (5, 0), "a fraction of a microsecond rounds up");
        assert_eq!(ticks_timeval(24_000_000 * 5 - 25), (4, 999_999));
    }
}
