//! M52 §3i: the stall watch. A pure state machine over host ticks; `Box_` owns the clock and the
//! timer. It decides one thing: whether the thread that has run since its switch-in, with others
//! ready, gets C8's line. Control-plane only: nothing here reaches the trace.

/// Host ticks per second (the 24 MHz mach timebase).
pub(crate) const TICKS_PER_SEC: u64 = 24_000_000;

pub struct StallWatch {
    threshold: u64,
    since: u64,
    fired: bool,
}

impl StallWatch {
    pub fn new(threshold: u64) -> Self { Self { threshold, since: 0, fired: false } }

    /// A thread was picked to run (C8): restart the clock and allow the line again.
    pub fn switched_in(&mut self, now: u64) { self.since = now; self.fired = false; }

    /// The host tick at which the running thread reaches the threshold.
    pub fn fire_at(&self) -> u64 { self.since + self.threshold }

    pub fn fired(&self) -> bool { self.fired }

    /// Whether the threshold has passed since the switch-in and the line is still owed.
    pub fn elapsed(&self, now: u64) -> bool { !self.fired && now.saturating_sub(self.since) >= self.threshold }

    /// C8's line, once per switch-in, when `runnable` (the other ready threads, ascending) or `timed`
    /// (the other threads blocked with a deadline, ascending: a spinner never reaches the settle that
    /// wakes them) is nonempty and the threshold has passed. Ready threads only: C8's original text;
    /// any timed thread: the ids are the union and the line says "ready to run or waiting on a timeout".
    pub fn check(&mut self, now: u64, cur: usize, runnable: &[usize], timed: &[usize]) -> Option<String> {
        if (runnable.is_empty() && timed.is_empty()) || !self.elapsed(now) { return None; }
        self.fired = true;
        let mut all: Vec<usize> = runnable.iter().chain(timed).copied().collect();
        all.sort_unstable();
        let ids: Vec<String> = all.iter().map(|t| t.to_string()).collect();
        let what = if timed.is_empty() { "ready to run" } else { "ready to run or waiting on a timeout" };
        Some(format!(
            "retrace: thread {cur} has run for {} s without blocking while thread(s) {} are {what}. \
             retrace runs another thread only when this one blocks, so if it is waiting for one of them, \
             the program will not finish (see docs/walls.md#threads)",
            self.threshold / TICKS_PER_SEC, ids.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const T: u64 = 20 * TICKS_PER_SEC;

    #[test]
    fn no_line_while_no_other_thread_is_runnable() {
        let mut w = StallWatch::new(T);
        w.switched_in(100);
        assert_eq!(w.check(100 + T, 0, &[], &[]), None);
        assert_eq!(w.check(100 + 10 * T, 0, &[], &[]), None);
    }

    #[test]
    fn no_line_before_the_threshold() {
        let mut w = StallWatch::new(T);
        w.switched_in(100);
        assert_eq!(w.check(100 + T - 1, 0, &[1], &[]), None);
    }

    #[test]
    fn one_line_after_the_threshold_naming_the_thread_and_the_runnable_ones() {
        let mut w = StallWatch::new(T);
        w.switched_in(100);
        assert_eq!(w.fire_at(), 100 + T);
        assert_eq!(w.check(100 + T, 0, &[1, 3], &[]).as_deref(), Some(
            "retrace: thread 0 has run for 20 s without blocking while thread(s) 1, 3 are ready to run. \
             retrace runs another thread only when this one blocks, so if it is waiting for one of them, \
             the program will not finish (see docs/walls.md#threads)"));
    }

    #[test]
    fn a_timed_blocked_thread_alone_gets_the_line_and_the_timeout_wording() {
        let mut w = StallWatch::new(T);
        w.switched_in(100);
        assert_eq!(w.check(100 + T, 0, &[], &[1]).as_deref(), Some(
            "retrace: thread 0 has run for 20 s without blocking while thread(s) 1 are ready to run or waiting on a timeout. \
             retrace runs another thread only when this one blocks, so if it is waiting for one of them, \
             the program will not finish (see docs/walls.md#threads)"));
    }

    #[test]
    fn ready_and_timed_threads_are_named_together_in_order() {
        let mut w = StallWatch::new(T);
        w.switched_in(0);
        let line = w.check(T, 0, &[3], &[1, 2]).unwrap();
        assert!(line.contains("thread(s) 1, 2, 3 are ready to run or waiting on a timeout."), "{line}");
    }

    #[test]
    fn a_switch_resets_the_watch() {
        let mut w = StallWatch::new(T);
        w.switched_in(100);
        w.switched_in(100 + T - 1);
        assert_eq!(w.check(100 + T, 0, &[1], &[]), None);
        assert!(w.check(100 + T - 1 + T, 0, &[1], &[]).is_some());
    }

    #[test]
    fn the_line_prints_once_per_switch_in() {
        let mut w = StallWatch::new(T);
        w.switched_in(0);
        assert!(w.check(T, 0, &[1], &[]).is_some());
        assert_eq!(w.check(T + 1, 0, &[1], &[]), None);
        w.switched_in(T + 2);
        assert!(w.check(2 * T + 2, 0, &[1], &[]).is_some());
    }
}
