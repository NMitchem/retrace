//! M14: the guest's thread table and its cooperative scheduler.
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
            threads: vec![Thread { ctx: main, state: ThreadState::Runnable, stack: (0, 0) }],
            current: 0,
        }
    }

    pub fn current(&self) -> usize { self.current }
    pub fn len(&self) -> usize { self.threads.len() }
    pub fn is_empty(&self) -> bool { self.threads.is_empty() }
    pub fn state_of(&self, tid: usize) -> ThreadState { self.threads[tid].state }
    pub fn ctx_of(&self, tid: usize) -> &ThreadCtx { &self.threads[tid].ctx }
    pub fn ctx_mut(&mut self, tid: usize) -> &mut ThreadCtx { &mut self.threads[tid].ctx }

    /// Threads that have not exited.
    pub fn live(&self) -> usize {
        self.threads.iter().filter(|t| !matches!(t.state, ThreadState::Exited(_))).count()
    }

    /// Append a runnable thread. Does **not** switch: the real kernel returns to the caller after
    /// `bsdthread_create`, and a switch here would reorder the guest's own output.
    pub fn spawn(&mut self, ctx: ThreadCtx, stack: (u64, u64)) -> usize {
        self.threads.push(Thread { ctx, state: ThreadState::Runnable, stack });
        self.threads.len() - 1
    }

    pub fn block(&mut self, reason: BlockReason) {
        self.threads[self.current].state = ThreadState::Blocked(reason);
    }

    pub fn switch_to(&mut self, tid: usize) {
        assert!(tid < self.threads.len(), "switch to nonexistent thread {tid}");
        self.current = tid;
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

    /// The scheduler. Lowest-indexed runnable thread, or `None` if nobody can run.
    ///
    /// `None` is a deadlock, and the caller must fail loud rather than spin — see `Box_`'s
    /// deadlock assert. Returning an `Option` instead of panicking here keeps this module pure and
    /// lets the table be unit-tested for the deadlock case without catching a panic.
    pub fn pick_next(&self) -> Option<usize> {
        self.threads.iter().position(|t| matches!(t.state, ThreadState::Runnable))
    }
}
