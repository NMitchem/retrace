//! M46 §3c: the process's workqueue kqueue as libdispatch uses it. It holds the knotes libdispatch
//! registers and the event manager thread that drains them. Pure data with no `Box_` access:
//! `Box_` owns one (`Box_::kq`) and carries it through every rebuild path in `BoxState`, because a
//! mid-run capture cannot re-derive it.
//!
//! Every mutator whose kernel answer the model has not measured returns `Err` naming it, and the
//! box refuses by value (R5).

use std::collections::BTreeMap;

use retrace_arch::{timer_fired_event, KeventQos, USER_WAKE_EVENT};

/// The event manager thread, if there is one.
/// - **`Bound`**: running, or runnable with its events already written.
/// - **`Unbound`**: parked in `workq_kernreturn(THREAD_KEVENT_RETURN)` until a knote activates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Manager {
    #[default]
    None,
    Bound(usize),
    Unbound(usize),
}

/// One `EVFILT_TIMER` knote. `fired` means expired and queued but not yet delivered. It is
/// `ONESHOT`, so delivery drops it (xnu `kern_event.c:4429-4436`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timer {
    pub deadline: u64,
    pub leeway: u64,
    pub udata: u64,
    pub fired: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkqKqueue {
    /// The init's `EVFILT_USER` knote (ident 1): `None` until registered, then `Some(active)`.
    user: Option<bool>,
    /// The memory-pressure knote's udata. It is registered and never activates, because the model
    /// has no memory pressure. That is deterministic, and faithful to a host under none.
    memstatus: Option<u64>,
    /// Timer knotes by ident, so that every iteration, and so delivery order, is by ident.
    timers: BTreeMap<u64, Timer>,
    manager: Manager,
}

impl WorkqKqueue {
    /// M45's init registers the manager's `EVFILT_USER` knote.
    pub fn register_user(&mut self) -> Result<(), String> {
        if self.user.is_some() {
            return Err("a second workqueue-kqueue init: the EVFILT_USER knote (ident 1) is already \
                        registered".into());
        }
        self.user = Some(false);
        Ok(())
    }

    pub fn register_memstatus(&mut self, udata: u64) -> Result<(), String> {
        if let Some(first) = self.memstatus {
            return Err(format!("a second EVFILT_MEMORYSTATUS registration (udata {udata:#x}; the \
                                first carried {first:#x})"));
        }
        self.memstatus = Some(udata);
        Ok(())
    }

    /// A `NOTE_TRIGGER` touch activates the user knote (xnu `filt_usertouch`). Two triggers before a
    /// delivery are one event, because `EV_CLEAR` resets it only at delivery.
    pub fn trigger_user(&mut self) -> Result<(), String> {
        match &mut self.user {
            Some(active) => { *active = true; Ok(()) }
            None => Err("a NOTE_TRIGGER of the EVFILT_USER knote (ident 1) before the init \
                         registered it".into()),
        }
    }

    /// `EV_ADD` arms a timer, or reprograms an armed one (xnu `kern_event.c:1775-1819`).
    pub fn add_timer(&mut self, ident: u64, deadline: u64, leeway: u64, udata: u64) -> Result<(), String> {
        if self.timers.get(&ident).is_some_and(|t| t.fired) {
            return Err(format!("an arm of timer {ident:#x} while its fire is queued, undelivered: what \
                                the kernel does to the queued event is unmeasured"));
        }
        self.timers.insert(ident, Timer { deadline, leeway, udata, fired: false });
        Ok(())
    }

    pub fn delete_timer(&mut self, ident: u64) -> Result<(), String> {
        match self.timers.get(&ident) {
            Some(t) if !t.fired => {
                self.timers.remove(&ident);
                Ok(())
            }
            Some(_) => Err(format!("a disarm of timer {ident:#x} while its fire is queued, \
                                    undelivered: what the kernel does to the queued event is \
                                    unmeasured")),
            None => Err(format!("a disarm of timer {ident:#x}, which is not armed: the kernel answers \
                                 ENOENT as an EV_ERROR event, which is not modelled")),
        }
    }

    /// Timers armed and not yet fired.
    pub fn armed_count(&self) -> usize {
        self.timers.values().filter(|t| !t.fired).count()
    }

    /// The deadline the idle jump lands on (M46 §3e rule 2): the earliest over armed timers only.
    pub fn earliest_deadline(&self) -> Option<u64> {
        self.timers.values().filter(|t| !t.fired).map(|t| t.deadline).min()
    }

    /// Fire every armed timer whose deadline `now` has reached (§3e rule 1; a deadline already
    /// reached is active immediately, xnu `kern_event.c:1711-1751`). Returns how many fired.
    pub fn fire_due(&mut self, now: u64) -> usize {
        let mut n = 0;
        for t in self.timers.values_mut() {
            if !t.fired && t.deadline <= now {
                t.fired = true;
                n += 1;
            }
        }
        n
    }

    /// Is any knote active, so that the manager is wanted?
    pub fn has_pending(&self) -> bool {
        self.user == Some(true) || self.timers.values().any(|t| t.fired)
    }

    /// Deliver up to `max` active knotes, in table order: the user knote first, then fired timers by
    /// ident (M46 §3d). Delivery clears the user knote (`EV_CLEAR`) and drops each timer
    /// (`ONESHOT`). Anything past `max` stays active for the next scan, as the kernel leaves it.
    pub fn take_events(&mut self, max: usize) -> Vec<KeventQos> {
        let mut out = Vec::new();
        if self.user == Some(true) && out.len() < max {
            out.push(USER_WAKE_EVENT);
            self.user = Some(false);
        }
        let due: Vec<u64> = self.timers.iter().filter(|(_, t)| t.fired).map(|(&i, _)| i)
            .take(max.saturating_sub(out.len())).collect();
        for ident in due {
            let t = self.timers.remove(&ident).expect("collected from the map just above");
            out.push(timer_fired_event(ident, t.leeway, t.udata));
        }
        out
    }

    pub fn manager(&self) -> Manager { self.manager }
    pub fn set_manager(&mut self, m: Manager) { self.manager = m; }
}

/// M46 §3e rule 2: the synthetic counter at which the guest's `mach_absolute_time`
/// (`tsc + offset`, wrapping as the guest's add wraps) equals `deadline`. A deadline already
/// reached returns `tsc` unchanged: the clock never moves backwards.
pub fn tsc_for_deadline(tsc: u64, offset: u64, deadline: u64) -> u64 {
    let now = tsc.wrapping_add(offset);
    if deadline > now { tsc.wrapping_add(deadline - now) } else { tsc }
}

#[cfg(test)]
mod tests {
    use super::*;
    use retrace_arch::TIMER_IDENT_BASE;

    const T0: u64 = TIMER_IDENT_BASE;
    const T1: u64 = TIMER_IDENT_BASE | 1;

    fn inited() -> WorkqKqueue {
        let mut k = WorkqKqueue::default();
        k.register_user().unwrap();
        k
    }

    #[test]
    fn a_trigger_before_the_init_is_refused() {
        let err = WorkqKqueue::default().trigger_user().unwrap_err();
        assert!(err.contains("before the init"), "{err}");
    }

    #[test]
    fn a_second_init_or_memory_pressure_registration_is_refused() {
        let mut k = inited();
        assert!(k.register_user().unwrap_err().contains("second"));
        k.register_memstatus(0x6c850).unwrap();
        assert!(k.register_memstatus(0x6c850).unwrap_err().contains("second"));
    }

    #[test]
    fn a_trigger_raises_one_user_event_and_delivery_clears_it() {
        let mut k = inited();
        assert!(!k.has_pending());
        k.trigger_user().unwrap();
        k.trigger_user().unwrap();
        assert_eq!(k.take_events(16), vec![USER_WAKE_EVENT], "two triggers before a delivery are one event");
        assert!(!k.has_pending());
        assert!(k.take_events(16).is_empty());
    }

    /// Review Focus 4.
    #[test]
    fn fired_timers_are_delivered_in_ident_order_after_the_user_event() {
        let mut k = inited();
        k.add_timer(T1, 100, 7, 0xb).unwrap();
        k.add_timer(T0, 100, 5, 0xa).unwrap();
        k.trigger_user().unwrap();
        assert_eq!(k.fire_due(100), 2);
        assert_eq!(k.take_events(16),
            vec![USER_WAKE_EVENT, timer_fired_event(T0, 5, 0xa), timer_fired_event(T1, 7, 0xb)]);
    }

    #[test]
    fn a_delivered_timer_is_dropped() {
        let mut k = inited();
        k.add_timer(T0, 100, 0, 0xa).unwrap();
        k.fire_due(100);
        assert_eq!(k.take_events(16).len(), 1);
        assert_eq!((k.armed_count(), k.has_pending(), k.earliest_deadline()), (0, false, None));
    }

    #[test]
    fn an_arm_of_an_armed_timer_reprograms_it() {
        let mut k = inited();
        k.add_timer(T0, 200, 0, 0xb).unwrap();
        k.add_timer(T0, 100, 0, 0xa).unwrap();
        assert_eq!((k.armed_count(), k.earliest_deadline()), (1, Some(100)));
        assert_eq!(k.fire_due(99), 0);
        assert_eq!(k.fire_due(100), 1);
        assert_eq!(k.take_events(16), vec![timer_fired_event(T0, 0, 0xa)]);
    }

    /// Review Focus 5.
    #[test]
    fn a_change_to_a_timer_whose_fire_is_queued_is_refused() {
        let mut k = inited();
        k.add_timer(T0, 100, 0, 0xa).unwrap();
        k.fire_due(100);
        assert!(k.add_timer(T0, 300, 0, 0xa).unwrap_err().contains("queued"));
        assert!(k.delete_timer(T0).unwrap_err().contains("queued"));
    }

    /// Review Focus 5: a delivered timer is dropped, so a later disarm names a timer that is gone.
    #[test]
    fn a_disarm_of_a_timer_that_is_not_armed_is_refused() {
        let mut k = inited();
        assert!(k.delete_timer(T1).unwrap_err().contains("not armed"));
        k.add_timer(T0, 100, 0, 0xa).unwrap();
        k.fire_due(100);
        k.take_events(16);
        assert!(k.delete_timer(T0).unwrap_err().contains("not armed"));
    }

    /// Review Focus 1: the `KEVENT_RETURN` that arms a timer calls this before it scans, so a
    /// deadline the synthetic clock already passed is delivered by the same return.
    #[test]
    fn a_timer_at_or_before_now_fires_and_a_later_one_does_not() {
        let mut k = inited();
        k.add_timer(T0, 100, 0, 0xa).unwrap();
        k.add_timer(T1, 101, 0, 0xb).unwrap();
        assert_eq!(k.fire_due(100), 1, "a deadline equal to now has been reached");
        assert_eq!(k.armed_count(), 1);
        assert_eq!(k.fire_due(100), 0, "a fired timer does not fire twice");
        assert!(k.has_pending());
    }

    #[test]
    fn the_idle_jump_lands_on_the_deadline_and_never_moves_backwards() {
        assert_eq!(tsc_for_deadline(1000, 50, 2000), 1950, "tsc + offset lands on the deadline");
        assert_eq!(tsc_for_deadline(1000, 50, 1050), 1000, "a deadline equal to now needs no jump");
        assert_eq!(tsc_for_deadline(1000, 50, 10), 1000, "a passed deadline never moves the clock back");
        assert_eq!(tsc_for_deadline(1000, u64::MAX - 499, 600), 1100, "the offset wraps as the guest's add does");
    }

    #[test]
    fn the_earliest_deadline_is_over_armed_timers_only() {
        let mut k = inited();
        k.add_timer(T0, 100, 0, 0xa).unwrap();
        k.add_timer(T1, 300, 0, 0xb).unwrap();
        k.fire_due(100);
        assert_eq!(k.earliest_deadline(), Some(300), "a fired timer is not waited for");
    }
}
