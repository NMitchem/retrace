//! M52 §3d: the kernel's count for the mach semaphores the guest creates, signals and waits on
//! (`semaphore_create` 3418, `semaphore_signal_trap` -33, `semaphore_wait_trap` -36,
//! `semaphore_timedwait_trap` -38, `semaphore_destroy` 3419). No guest memory and no clock; the
//! thread table and the deadline live in `Box_` (`guest_sem_wait`, `guest_sem_timedwait`).
//!
//! A semaphore the model has not seen created counts from 0: libdispatch's own are created with
//! value 0, and a name the guest got before the box learned of it (a checkpoint taken mid-run
//! carries this state, so that does not arise) would only need the count a signal gave it.
use std::collections::{BTreeMap, BTreeSet};

/// Port name → the kernel's count, and the names a `semaphore_destroy` has killed. Box state
/// rebuilt on both sides from the guest's own calls and carried through every rebuild path
/// (`BoxState::sems`); nothing in it is recorded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Semaphores {
    counts: BTreeMap<u64, u32>,
    /// Names destroyed and not handed back by a later `create` (host names are reused). Natively a
    /// signal on one answers `KERN_INVALID_NAME`; a wait on one is unmeasured and refused.
    dead: BTreeSet<u64>,
}

impl Semaphores {
    /// `semaphore_create`'s reply named `port`: it starts at `value`, and is alive again if the host
    /// handed a destroyed name back.
    pub fn create(&mut self, port: u64, value: u32) {
        self.dead.remove(&port);
        self.counts.insert(port, value);
    }

    /// A signal that woke nobody: the kernel keeps it.
    pub fn signal(&mut self, port: u64) {
        let c = self.counts.entry(port).or_insert(0);
        *c = c.saturating_add(1);
    }

    /// A wait takes one from the count if there is one.
    pub fn take(&mut self, port: u64) -> bool {
        match self.counts.get_mut(&port) {
            Some(c) if *c > 0 => { *c -= 1; true }
            _ => false,
        }
    }

    /// `semaphore_destroy` succeeded on `port`: its count is gone and the name is dead.
    pub fn destroy(&mut self, port: u64) {
        self.counts.remove(&port);
        self.dead.insert(port);
    }

    /// Is `port` a name a destroy killed and no create has revived?
    pub fn is_dead(&self, port: u64) -> bool { self.dead.contains(&port) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signal_with_no_waiter_is_taken_by_the_next_wait() {
        let mut s = Semaphores::default();
        assert!(!s.take(0x1403), "an unknown port counts from 0");
        s.signal(0x1403);
        s.signal(0x1403);
        assert!(s.take(0x1403));
        assert!(s.take(0x1403));
        assert!(!s.take(0x1403), "two signals satisfy two waits, not three");
    }

    #[test]
    fn creates_value_is_the_start() {
        let mut s = Semaphores::default();
        s.create(0x1403, 2);
        assert!(s.take(0x1403) && s.take(0x1403) && !s.take(0x1403));
    }

    #[test]
    fn destroy_forgets_the_port_and_a_reused_name_starts_over() {
        let mut s = Semaphores::default();
        s.create(0x1403, 3);
        s.destroy(0x1403);
        assert!(s.is_dead(0x1403));
        assert!(!s.take(0x1403), "the count went with the port");
        s.create(0x1403, 0);
        assert!(!s.is_dead(0x1403), "the host reused the name");
        assert!(!s.take(0x1403), "a reused name starts at its new value, not the old count");
    }
}
