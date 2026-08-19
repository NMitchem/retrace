//! M14: the guest's thread table and its cooperative scheduler. M16 additionally makes this module
//! the owner of per-thread signal state — the blocked mask, the pending set and the alternate stack
//! (see `Thread`'s `mask`/`pending`/`altstack` fields below), moved here from `SigTable` because
//! POSIX makes those three per-thread while dispositions stay process-wide and remain in `sig.rs`.
//!
//! **This module is deliberately VM-free.** Nothing here touches HVF, the vCPU, or guest memory —
//! it is bookkeeping plus one pick function, which is why it can be unit-tested exhaustively in
//! milliseconds while the rest of M14 needs a VM and `--test-threads=1`.
//!
//! **Why the scheduler is a pure function.** `pick_next` returns the lowest-indexed runnable
//! thread. Given the guest's own syscall sequence the choice is forced, so record and replay
//! schedule identically with nothing recorded and no trace-format change. That is symmetry rule 2:
//! deterministic behaviour belongs below the trace, where it fires identically on both sides.
// `retrace-box` imports Regs PRIVATELY at lib.rs:4 (`use retrace_trace::{Regs, Region};`), so
// `crate::Regs` does NOT resolve from a submodule. Import it from its own crate.
use retrace_trace::Regs;

/// One thread's register context.
///
/// This is `BoxState`'s register subset — which M4's checkpoint tests already prove is sufficient
/// to restore a vCPU mid-run — **plus `tpidrro_el0`, which `BoxState` does not carry.** Its absence
/// there is correct: the thread pointer is a constant (`TSD_IPA`) until threads exist. Threads are
/// exactly what makes it vary, so the one register the existing context set omits is the one M14
/// makes per-thread. Note `tpidr_el0` is NOT here: macOS 26 reads the CPU number from its low bits
/// and it must stay 0 for every thread (M2-cpuid).
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadCtx {
    pub regs: Regs,
    pub fp: [u128; 32],
    pub fpcr: u64,
    pub fpsr: u64,
    pub tpidrro_el0: u64,
    pub elr: u64,
    pub spsr: u64,
}

impl ThreadCtx {
    pub fn zeroed() -> Self {
        Self {
            // `Regs` derives Debug/Clone/PartialEq/Eq/Serialize/Deserialize but NOT Default —
            // construct it field-by-field rather than adding a derive to the trace crate.
            regs: Regs { x: [0u64; 31], pc: 0, sp_el0: 0, cpsr: 0 },
            fp: [0u128; 32],
            fpcr: 0,
            fpsr: 0,
            tpidrro_el0: 0,
            elr: 0,
            spsr: 0,
        }
    }
}

/// Why a thread cannot currently run.
///
/// The variants are deliberately concrete rather than an opaque token: `unblock_joiners_of` has to
/// decide who a thread exit wakes, and that is only answerable if the reason names its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockReason {
    /// Waiting for thread `target` to exit (`pthread_join`).
    Join { target: usize },
    /// Waiting on a futex-shaped address (the primitive Task 1 measured).
    Wait { addr: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    Runnable,
    Blocked(BlockReason),
    /// Exited with this return value. Kept in the table rather than removed: `join` may arrive
    /// AFTER the exit, and a removed thread cannot answer it. Indices must also stay stable.
    Exited(u64),
}

#[derive(Clone, Debug)]
pub struct Thread {
    pub ctx: ThreadCtx,
    pub state: ThreadState,
    /// `(base, len)` of the guest-allocated stack. The guest maps its own thread stacks, so M14
    /// never places one; this is recorded for teardown and for diagnostics only.
    pub stack: (u64, u64),
    /// M16: bit `(sig - 1)`, the same `sigset_t` encoding `SigTable.blocked` used before the split.
    /// Per-thread because POSIX makes it so; INHERITED by value at `spawn`.
    pub mask: u32,
    /// M16: signals raised for this thread while its mask blocked them. Materialised at the next
    /// unmask, which is a syscall landmark — the anchor that keeps delivery above the trace.
    pub pending: u32,
    /// M16: per-thread, and deliberately NOT inherited — a new thread starts with no alt stack.
    pub altstack: Option<(u64, u64, u64)>,
    /// M16: this thread has been redirected into a handler and has not been scheduled since.
    /// A second signal arriving now would stack a frame on a context that never ran the first —
    /// fail loud rather than guess at the kernel's queueing order.
    pub redirected: bool,
}

#[derive(Clone, Debug)]
pub struct ThreadTable {
    threads: Vec<Thread>,
    current: usize,
}

impl ThreadTable {
    /// A guest starts with exactly one thread, so a single-threaded guest has a one-entry table and
    /// takes precisely the pre-M14 path. That is the compatibility argument for every M0–M13 gate.
    pub fn new(main: ThreadCtx) -> Self {
        Self {
            threads: vec![Thread {
                ctx: main,
                state: ThreadState::Runnable,
                stack: (0, 0),
                mask: 0,
                pending: 0,
                altstack: None,
                redirected: false,
            }],
            current: 0,
        }
    }

    pub fn current(&self) -> usize { self.current }
    pub fn len(&self) -> usize { self.threads.len() }
    pub fn is_empty(&self) -> bool { self.threads.is_empty() }
    pub fn state_of(&self, tid: usize) -> ThreadState { self.threads[tid].state }
    pub fn ctx_of(&self, tid: usize) -> &ThreadCtx { &self.threads[tid].ctx }
    pub fn ctx_mut(&mut self, tid: usize) -> &mut ThreadCtx { &mut self.threads[tid].ctx }

    // ---- M16: per-thread mask, pending set, and alternate stack ----------------------------

    /// `1u32 << (sig - 1)`, guarded — the moved counterpart of `SigTable::idx`, which used to sit
    /// between every mask access and the shift. Range-checked against the SAME bound `SigTable`
    /// uses (`NSIG` == 32, i.e. signals 1..=31), deliberately, not 1..=32: `take_deliverable`'s
    /// result is looked up in `SigTable::action`, so a signal this guard admitted but `idx` rejected
    /// would panic far from the mistake instead of here, at the point it was made. Fix round 2
    /// (review finding 1): the review found `is_blocked_for`/`pend`/`take_deliverable` computed this
    /// shift unguarded — `sig == 0` underflows `sig - 1` (a shift-overflow panic in debug, a masked
    /// shift onto bit 31 in release), and `sig == 32` is representable in the `u32` mask but not in
    /// `SigTable`'s 1..=31.
    fn sig_bit(sig: u64) -> u32 {
        assert!(
            sig >= 1 && (sig as usize) < retrace_arch::NSIG,
            "signal {sig} out of range 1..{} — the caller passed a signal number no thread's mask \
             can represent; widen the mask or reject it at the syscall arm",
            retrace_arch::NSIG
        );
        1u32 << (sig - 1)
    }

    pub fn mask_of(&self, tid: usize) -> u32 { self.threads[tid].mask }

    /// `SigTable::set_mask`'s arithmetic, moved verbatim, including its fail-loud `how` panic.
    pub fn set_mask_of(&mut self, tid: usize, how: u64, set: u32) -> u32 {
        let old = self.threads[tid].mask;
        self.threads[tid].mask = match how {
            retrace_arch::SIG_BLOCK => old | set,
            retrace_arch::SIG_UNBLOCK => old & !set,
            retrace_arch::SIG_SETMASK => set,
            _ => panic!(
                "sigprocmask how={how} is not BLOCK(1)/UNBLOCK(2)/SETMASK(3) — an unmodelled \
                 value, not a guest error to swallow"
            ),
        };
        old
    }

    pub fn is_blocked_for(&self, tid: usize, sig: u64) -> bool {
        self.threads[tid].mask & Self::sig_bit(sig) != 0
    }

    pub fn pend(&mut self, tid: usize, sig: u64) {
        self.threads[tid].pending |= Self::sig_bit(sig);
    }

    pub fn pending_of(&self, tid: usize) -> u32 { self.threads[tid].pending }

    /// The lowest-numbered pending signal this thread's mask no longer blocks, CLEARED as it is
    /// taken.
    pub fn take_deliverable(&mut self, tid: usize) -> Option<u64> {
        let t = &mut self.threads[tid];
        let ready = t.pending & !t.mask;
        if ready == 0 {
            return None;
        }
        let sig = ready.trailing_zeros() as u64 + 1;
        t.pending &= !Self::sig_bit(sig);
        Some(sig)
    }

    pub fn altstack_of(&self, tid: usize) -> Option<(u64, u64, u64)> { self.threads[tid].altstack }

    /// M16: has `tid` been redirected into a handler it has not yet run?
    pub fn is_redirected(&self, tid: usize) -> bool { self.threads[tid].redirected }

    /// M16: mark (or clear) `tid`'s redirected flag. `deliver_signal_to` sets it on the target it
    /// just redirected; `switch_to` clears it for the thread being switched TO, because that thread
    /// is now running the handler it was given.
    pub fn set_redirected(&mut self, tid: usize, redirected: bool) {
        self.threads[tid].redirected = redirected;
    }

    pub fn set_altstack_of(
        &mut self,
        tid: usize,
        ss: Option<(u64, u64, u64)>,
    ) -> Option<(u64, u64, u64)> {
        std::mem::replace(&mut self.threads[tid].altstack, ss)
    }

    /// Threads that have not exited.
    pub fn live(&self) -> usize {
        self.threads.iter().filter(|t| !matches!(t.state, ThreadState::Exited(_))).count()
    }

    /// Append a runnable thread. Does **not** switch: the real kernel returns to the caller after
    /// `bsdthread_create`, and a switch here would reorder the guest's own output.
    ///
    /// POSIX inherits the creating thread's signal mask at `pthread_create`, BY VALUE. The
    /// alternate stack is NOT inherited — a new thread starts with none.
    pub fn spawn(&mut self, ctx: ThreadCtx, stack: (u64, u64)) -> usize {
        let mask = self.threads[self.current].mask;
        self.threads.push(Thread {
            ctx,
            state: ThreadState::Runnable,
            stack,
            mask,
            pending: 0,
            altstack: None,
            redirected: false,
        });
        self.threads.len() - 1
    }

    /// Block the current thread on `reason`.
    ///
    /// For `Join { target }`: guarded against a target that has ALREADY exited (fix round 1,
    /// M-1). `unblock_joiners_of(target)` fires exactly once, at `target`'s exit — if that has
    /// already happened, it will not fire again, so blocking anyway waits forever on a wake that
    /// already happened (M14 Task 4's review; carried forward and made mandatory at Task 8). This
    /// used to live in a separate wrapper (`block_on_join`) that callers had to opt into; folded
    /// into `block` itself so every caller of the ONE public entry point — including this
    /// module's own tests — gets the guard, not just callers who remembered the wrapper existed.
    /// `Wait { .. }` is unaffected: it has no analogous "already satisfied" state to check here
    /// (that check is `guest_ulock_wait`'s, against live guest memory, not this table's).
    pub fn block(&mut self, reason: BlockReason) {
        if let BlockReason::Join { target } = reason {
            if matches!(self.state_of(target), ThreadState::Exited(_)) {
                return;
            }
        }
        self.threads[self.current].state = ThreadState::Blocked(reason);
    }

    pub fn switch_to(&mut self, tid: usize) {
        assert!(tid < self.threads.len(), "switch to nonexistent thread {tid}");
        self.current = tid;
        // M16: `tid` is now running — if it was redirected into a handler, it is about to run that
        // handler, so the "un-run redirection" this flag guards against no longer holds.
        self.threads[tid].redirected = false;
    }

    pub fn exit_current(&mut self, code: u64) {
        self.threads[self.current].state = ThreadState::Exited(code);
    }

    /// Wake everyone joined on `tid`. Called on thread exit.
    pub fn unblock_joiners_of(&mut self, tid: usize) {
        for t in &mut self.threads {
            if let ThreadState::Blocked(BlockReason::Join { target }) = t.state {
                if target == tid {
                    t.state = ThreadState::Runnable;
                }
            }
        }
    }

    /// Wake every thread waiting on exactly `addr`. Returns how many were woken.
    ///
    /// **This is the wake seam, and it matches by ADDRESS EQUALITY — measured, not fabricated.**
    /// Task 8's review established that nothing woke a `Blocked(Wait { addr })` thread in any form,
    /// and correctly declined to invent an address→thread-index correlation. It does not need one:
    /// M14 Task 9 disassembled both halves of the pair (`.superpowers/…/task-9-measurements.md`) and
    /// they name the *same word*, `pthread + 0x34` —
    ///   * `__pthread_join` at `0x9028`:        `add x21, x19, #0x34` … `bl ___ulock_wait`
    ///   * `__pthread_joiner_wake` at `0x66f0`: `add x1,  x19, #0x34` … `bl ___ulock_wake`
    ///
    /// so equality on the address the guest itself supplies is the whole correlation.
    ///
    /// Returning a count rather than `()` is what lets the caller's test tell "woke the right one"
    /// apart from "woke everything"; nothing in the box branches on it. Zero is legal and not an
    /// error: the real kernel answers `ENOENT` when no one is waiting and `__pthread_joiner_wake`
    /// treats that as success (its `cmn w0, #0x2` / `b.eq` return path).
    pub fn unblock_waiters_on(&mut self, addr: u64) -> usize {
        let mut woken = 0;
        for t in &mut self.threads {
            if let ThreadState::Blocked(BlockReason::Wait { addr: a }) = t.state {
                if a == addr {
                    t.state = ThreadState::Runnable;
                    woken += 1;
                }
            }
        }
        woken
    }

    /// Does the vCPU need to be moved to a different thread before it can run again?
    ///
    /// The predicate `Box_::run()` consults on every entry, named so it can be unit-tested here
    /// rather than living as an inline condition inside the trap loop. **This is the compatibility
    /// argument for every M0–M13 gate:** a single-threaded guest has a one-entry table whose only
    /// thread is `Runnable`, so this is always false and `run()` takes precisely the pre-M14 path.
    pub fn needs_reschedule(&self) -> bool {
        !matches!(self.state_of(self.current), ThreadState::Runnable)
    }

    /// The scheduler. Lowest-indexed runnable thread, or `None` if nobody can run.
    ///
    /// `None` is a deadlock, and the caller must fail loud rather than spin — see `Box_`'s
    /// deadlock assert. Returning an `Option` instead of panicking here keeps this module pure and
    /// lets the table be unit-tested for the deadlock case without catching a panic.
    pub fn pick_next(&self) -> Option<usize> {
        self.threads.iter().position(|t| matches!(t.state, ThreadState::Runnable))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spawned_thread_inherits_the_creators_mask_by_value() {
        let mut t = ThreadTable::new(ThreadCtx::zeroed());
        t.set_mask_of(0, retrace_arch::SIG_BLOCK, 1 << 29); // SIGUSR1 (30) => bit 29
        let child = t.spawn(ThreadCtx::zeroed(), (0, 0));
        assert_eq!(t.mask_of(child), 1 << 29, "POSIX inherits the mask at creation");
        // BY VALUE: changing the creator afterwards must not reach the child.
        t.set_mask_of(0, retrace_arch::SIG_SETMASK, 0);
        assert_eq!(t.mask_of(child), 1 << 29, "inheritance is a copy, not a reference");
        assert_eq!(t.mask_of(0), 0);
    }

    // Relocated from sig.rs's SigTable::set_mask test (M16 fix round 1): the BLOCK/UNBLOCK/SETMASK
    // arithmetic table, including the OLD-mask return value at every step — nothing else in this
    // module asserts on set_mask_of's return, and "UNBLOCK clears" is the trigger the pending-signal
    // delivery path (a_pended_signal_is_taken_only_once_and_lowest_first, below) rests on.
    #[test]
    fn set_mask_of_honours_block_unblock_setmask_and_returns_the_old_mask() {
        let mut t = ThreadTable::new(ThreadCtx::zeroed());
        assert_eq!(t.set_mask_of(0, retrace_arch::SIG_BLOCK, 0b0110), 0, "returns the OLD mask");
        assert_eq!(t.mask_of(0), 0b0110);
        assert_eq!(t.set_mask_of(0, retrace_arch::SIG_BLOCK, 0b1000), 0b0110);
        assert_eq!(t.mask_of(0), 0b1110, "BLOCK is a union");
        assert_eq!(t.set_mask_of(0, retrace_arch::SIG_UNBLOCK, 0b0100), 0b1110);
        assert_eq!(t.mask_of(0), 0b1010, "UNBLOCK clears");
        assert_eq!(t.set_mask_of(0, retrace_arch::SIG_SETMASK, 0b0001), 0b1010);
        assert_eq!(t.mask_of(0), 0b0001, "SETMASK replaces");
    }

    #[test]
    fn masks_are_independent_between_threads() {
        let mut t = ThreadTable::new(ThreadCtx::zeroed());
        let child = t.spawn(ThreadCtx::zeroed(), (0, 0));
        t.set_mask_of(0, retrace_arch::SIG_BLOCK, 1 << 29);
        assert!(t.is_blocked_for(0, 30));
        assert!(!t.is_blocked_for(child, 30),
            "this is the whole per-thread claim: main blocking a signal must not block it for the child");
    }

    // Restored from sig.rs's SigTable::is_blocked_indexes_by_sig_minus_one (M16 fix round 2, review
    // finding 4): the same-thread NEIGHBOUR negatives. masks_are_independent_between_threads (above)
    // pins the shift amount and polarity via a different thread's mask, which is a real but distinct
    // property — this restores the direct, same-thread, both-neighbours check the earlier test
    // dropped.
    #[test]
    fn is_blocked_for_indexes_by_sig_minus_one() {
        let mut t = ThreadTable::new(ThreadCtx::zeroed());
        t.set_mask_of(0, retrace_arch::SIG_SETMASK, 1 << 5); // bit 5 == signal 6
        assert!(t.is_blocked_for(0, 6), "bit (sig-1) is the encoding");
        assert!(!t.is_blocked_for(0, 5));
        assert!(!t.is_blocked_for(0, 7));
    }

    // M16 fix round 2 (review finding 1): the guard `sig_bit` adds. sig=0 is the underflow case
    // (shift-overflow panic in debug, masked shift onto bit 31 in release, pre-fix); sig=32 is the
    // aliasing case (representable in the u32 mask, NOT in SigTable's 1..=31, pre-fix silently
    // accepted by pend and deferred to a far-away panic in SigTable::idx when take_deliverable later
    // handed 32 back).
    #[test]
    #[should_panic(expected = "out of range")]
    fn is_blocked_for_rejects_signal_zero() {
        let t = ThreadTable::new(ThreadCtx::zeroed());
        let _ = t.is_blocked_for(0, 0);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn pend_rejects_signal_32_which_the_mask_could_alias_but_sigtable_cannot_represent() {
        let mut t = ThreadTable::new(ThreadCtx::zeroed());
        t.pend(0, 32);
    }

    #[test]
    fn a_pended_signal_is_taken_only_once_and_lowest_first() {
        let mut t = ThreadTable::new(ThreadCtx::zeroed());
        t.set_mask_of(0, retrace_arch::SIG_BLOCK, (1 << 29) | (1 << 30)); // 30 and 31
        t.pend(0, 31);
        t.pend(0, 30);
        assert_eq!(t.take_deliverable(0), None, "both are still masked");
        t.set_mask_of(0, retrace_arch::SIG_UNBLOCK, 1 << 29);
        assert_eq!(t.take_deliverable(0), Some(30), "lowest deliverable first");
        assert_eq!(t.take_deliverable(0), None, "taking clears the bit; 31 is still masked");
        t.set_mask_of(0, retrace_arch::SIG_UNBLOCK, 1 << 30);
        assert_eq!(t.take_deliverable(0), Some(31));
        assert_eq!(t.take_deliverable(0), None);
    }

    #[test]
    fn alternate_stacks_are_per_thread() {
        let mut t = ThreadTable::new(ThreadCtx::zeroed());
        let child = t.spawn(ThreadCtx::zeroed(), (0, 0));
        t.set_altstack_of(0, Some((0x9000, 0x1000, 0)));
        assert_eq!(t.altstack_of(0), Some((0x9000, 0x1000, 0)));
        assert_eq!(t.altstack_of(child), None,
            "sigaltstack is per-thread, and is NOT inherited across pthread_create");
    }

    // Relocated from sig.rs's SigTable::set_altstack test (M16 fix round 1): the previous-value
    // return, which alternate_stacks_are_per_thread (above) never checks.
    #[test]
    fn set_altstack_of_returns_the_previous_value() {
        let mut t = ThreadTable::new(ThreadCtx::zeroed());
        assert_eq!(t.set_altstack_of(0, Some((0x9000, 0x4000, 0))), None);
        assert_eq!(t.altstack_of(0), Some((0x9000, 0x4000, 0)));
        assert_eq!(t.set_altstack_of(0, None), Some((0x9000, 0x4000, 0)));
    }
}
