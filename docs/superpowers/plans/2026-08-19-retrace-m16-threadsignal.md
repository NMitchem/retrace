# M16-threadsignal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make retrace's signal path thread-aware — resolve `__pthread_kill`'s target port to a
thread, deliver into that thread even when it is not running, give each thread its own mask, pending
set and alternate stack, and tag the four remaining landmark variants with a thread.

**Architecture:** Per-thread signal state moves from the process-global `SigTable` onto `Thread`
(dispositions stay process-global, which is what POSIX says). `deliver_signal` stops reading the
live vCPU and instead operates on a target thread's saved `ThreadCtx`, with the current thread saved
into the table first so one code path serves both cases. Every delivery is anchored to a **syscall
landmark** — either the `pthread_kill` that raised it, or the `sigprocmask`/`pthread_sigmask` that
unblocked it — so nothing has to escape `Box_::run()` and symmetry rule 1 applies in its ordinary
form.

**Tech Stack:** Rust 1.95.0, `aarch64-apple-darwin`, macOS 26.x on Apple Silicon,
Hypervisor.framework via `hv-sys`. Guests are built by bare `rustc` from `crates/retrace-guest/rs/`.

**Spec:** `docs/superpowers/specs/2026-08-19-retrace-m16-threadsignal-design.md`

## Global Constraints

**Line numbers in this plan are approximate and drift as tasks land.** They were measured once and
every task that edits a file moves the ones below it — after Task 3 alone, every anchor here had
slipped between 1 and 5 lines. Each reference therefore also names the **symbol** or the exact
`match` guard, and the symbol is the authoritative half: search for it rather than jumping to the
number. A `~` prefix marks a number known to be approximate. Do not treat a line that does not
contain what the plan says it contains as evidence the plan is wrong about the *work* — grep for the
symbol first, and only then report a discrepancy.


- **`--test-threads=1` is mandatory** for anything that builds a VM. HVF allows one VM per process.
- **A bare `cargo test --workspace` gets killed on this machine.** Run the suite in chunks, every
  chunk with `--no-fail-fast`, and capture cargo's own exit code *before* piping to any filter —
  `cargo … | perl …; rc=$?` captures perl's status, which reported a failing M15 gate as exit 0.
- **`grep -a` on gate logs.** They land ISO-8859 even after ANSI stripping, so plain `grep` treats
  them as binary and prints nothing.
- **Clippy `--workspace --all-targets -D warnings` must pass.** `clippy.toml` bans `Instant::now`
  and `SystemTime::now` (determinism) and `std::thread::Thread` (retrace's core is single-threaded).
  The ban governs retrace's own crates — a *guest* under `crates/retrace-guest/rs/` is a separate
  binary compiled by bare `rustc` and may use `std::thread` freely, as `threadrust.rs` and
  `watchthread.rs` already do.
- **Symmetry rule 1:** a special case in record's `match stop` needs a mirror in replay's dispatch,
  both in `crates/retrace-core/src/lib.rs` (record in `record_box`, replay in
  `ReplaySession::advance`), calling the *same* `Box_` method with the *same* arguments, placed
  *before* the generic forward arm.
- **Symmetry rule 2:** deterministic instruction emulation belongs below the trace inside
  `Box_::run()`. **Signal delivery is not that** — it is a control transfer and stays a trace event,
  per M12's ruling.
- **`TRACE_MAGIC` becomes `RT\x00\x08`.** Changing `Event`'s shape is a format break.
- **No new record/replay asymmetry.** M2-xpcport's minted-port exception stays the only one.
- **Fail loud over silent defaults.** An unresolvable port, an unmodelled `how`, a second signal to
  an already-redirected thread: panic with a message naming the measurement or the missing feature.
- **Honest-gate discipline.** Never assert on an exit code a weaker failure would also produce.
  Assert on the difference the work makes. A skipped test must announce itself loudly.
- **Every claim about what a test catches is established by mutation, not by argument.** M15's plan
  predicted two catches that measurement disproved. If a step here claims a test catches something,
  the implementer verifies it by making the mutation and watching that test fail.
- **Overturn this plan with evidence.** If a step's claim disagrees with what the code actually
  does, measure it, report the discrepancy, and implement what is correct — do not implement
  something you have measured to be wrong because the plan said so.

---

### Task 1: Read a thread's kport back out of its pthread struct

Spec risk **R1**. Everything in M16-port rests on `[pthread + 0xf8]` holding a usable mach-port name
for *main*, which retrace has only ever **written** (for children), never **read**. Measure it before
building on it.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (new `pthread_of` / `kport_of` near `threads()`, ~`:3042`)
- Modify: `crates/retrace-core/src/lib.rs` (a `ReplaySession::dbg_kport_of` passthrough, beside the
  existing `dbg_regs_of` from M15 Task 2 — copy its current/non-current discipline)
- Test: `crates/retrace/tests/kport.rs` (new)

**Interfaces:**
- Produces: `Box_::pthread_of(&self, tid: usize) -> Option<u64>`,
  `Box_::kport_of(&self, tid: usize) -> Option<u32>`,
  `ReplaySession::dbg_kport_of(&self, tid: usize) -> Option<u32>`.
- Consumes: `PTHREAD_TSD_OFF` (`0xe0`, `retrace-box/src/lib.rs:474`), `PTHREAD_KPORT_OFF` (`0xf8`,
  `:468`), `GUEST_THREAD_PORT_BASE` (`0x0BAD_7000`, `:493`).

**Background the implementer needs:** a thread's TSD base is `pthread + 0xe0` and lives in
`TPIDRRO_EL0`, so `pthread = tpidrro_el0 - 0xe0`. For the **current** thread read `TPIDRRO_EL0` off
the vCPU; for a **non-current** thread read `threads().ctx_of(tid).tpidrro_el0` — the running
thread's table entry is stale between switches (`Box_::threads()`'s own doc says so). This is the
same split `dbg_regs_of` established in M15.

- [ ] **Step 1: Write the failing test**

```rust
// crates/retrace/tests/kport.rs
//
// M16 Task 1 / spec risk R1. The port->tid map M16 needs is read back OUT of the guest's own
// pthread struct rather than reconstructed from tid, because that is the only rule that covers
// main: retrace writes `GUEST_THREAD_PORT_BASE | tid` for every thread it spawns, but main's
// kport is written by libpthread's `__pthread_main_thread_init`, in userspace, and retrace has
// never read that field back. This test IS the measurement.
mod util;
use retrace_core::ReplaySession;
use std::path::Path;

#[test]
fn every_live_thread_has_a_distinct_readable_kport() {
    let (rec, trace) = util::record_dynamic(retrace_guest::THREADRUST);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    // Advance to a landmark where the child provably exists. THREADRUST's child prints, so once
    // "child ran" is in stdout the spawn is behind us; simplest reliable seek is to run to the end
    // of the trace minus nothing and inspect the table, so instead advance until the table grows.
    let mut s = ReplaySession::open(Path::new(&trace)).unwrap();
    // BOUNDED. `advance()`'s own doc says calling it after `Advance::Exited` is unspecified and
    // callers must not — so an unbounded "wait for the child" loop runs off the end of the trace on
    // any guest that never spawns. Stop on exit, and fail loud naming what we were waiting for.
    loop {
        if s.b_thread_count() >= 2 { break; }
        match s.advance().expect("no divergence on an untampered trace") {
            retrace_core::Advance::Exited(_) =>
                panic!("the recording ended with only {} thread(s): THREADRUST must spawn one, so \
                        either the guest changed or bsdthread_create was not emulated",
                       s.b_thread_count()),
            _ => continue,
        }
    }

    let main_port = s.dbg_kport_of(0).expect("main's pthread must be mapped and readable");
    let child_port = s.dbg_kport_of(1).expect("the child's pthread must be mapped and readable");

    assert_eq!(child_port, 0x0BAD_7001,
        "the child's kport is the one retrace itself wrote in guest_bsdthread_create: \
         GUEST_THREAD_PORT_BASE | tid");
    assert_ne!(main_port, 0,
        "R1: main's kport is libpthread's own write. A zero here means the field is not populated \
         at this point in the run, and M16-port must fall back to recognising 0x0BAD_7000|tid for \
         children and failing loud on anything else. Report the value either way.");
    assert_ne!(main_port, child_port,
        "two threads that share a kport would make port->tid resolution ambiguous");
    eprintln!("R1 MEASURED: main kport = {main_port:#x}, child kport = {child_port:#x}");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p retrace --test kport -- --test-threads=1 --nocapture`
Expected: FAIL to compile — no `dbg_kport_of`, no `b_thread_count`.

- [ ] **Step 3: Add the accessors to `Box_`**

Place beside `threads()` in `crates/retrace-box/src/lib.rs`:

```rust
/// The guest VA of `tid`'s `pthread` struct, derived from its thread pointer.
///
/// `TPIDRRO_EL0 = pthread + PTHREAD_TSD_OFF` is the kernel's convention, measured 4/4 for main and
/// child alike (see `guest_bsdthread_create`). The CURRENT thread's table entry is stale between
/// switches, so its thread pointer comes off the vCPU — the split `dbg_regs_of` established.
pub fn pthread_of(&self, tid: usize) -> Option<u64> {
    if tid >= self.threads.len() { return None; }
    let tp = if tid == self.threads.current() {
        self.vcpu.get_sys(sysreg::TPIDRRO_EL0).unwrap()
    } else {
        self.threads.ctx_of(tid).tpidrro_el0
    };
    tp.checked_sub(PTHREAD_TSD_OFF)
}

/// `tid`'s mach-port name, read back out of its own `pthread` struct at `+0xf8`.
///
/// READ, not reconstructed. For a thread retrace spawned this returns the
/// `GUEST_THREAD_PORT_BASE | tid` it wrote itself; for main it returns what libpthread's
/// `__pthread_main_thread_init` stored. Reading is what makes main need no special case.
pub fn kport_of(&self, tid: usize) -> Option<u32> {
    let va = self.pthread_of(tid)?;
    let ipa = self.va_to_ipa(va + PTHREAD_KPORT_OFF)?;
    let (hp, avail) = self.host_span(ipa)?;
    if avail < 4 { return None; }
    let mut b = [0u8; 4];
    unsafe { std::ptr::copy_nonoverlapping(hp, b.as_mut_ptr(), 4) };
    Some(u32::from_le_bytes(b))
}
```

- [ ] **Step 4: Add the `ReplaySession` passthroughs**

In `crates/retrace-core/src/lib.rs`, beside `dbg_regs_of`:

```rust
/// M16 Task 1: `Box_::kport_of`, for the R1 measurement gate. Test-only, like `dbg_regs_of`.
#[doc(hidden)]
pub fn dbg_kport_of(&self, tid: usize) -> Option<u32> { self.b.kport_of(tid) }

/// How many threads the guest has created so far. Test-only.
#[doc(hidden)]
pub fn b_thread_count(&self) -> usize { self.b.threads().len() }
```

- [ ] **Step 5: Run the test**

Run: `cargo test -p retrace --test kport -- --test-threads=1 --nocapture`
Expected: PASS, and the `R1 MEASURED:` line printed.

**If `main_port` is 0 or the read returns `None`:** that is R1's fallback branch firing. Record the
measured value in your report, change the test to assert the fallback contract instead (children
resolve, main does not), and flag it — Task 4's `thread_of_port` then recognises only
`GUEST_THREAD_PORT_BASE | tid` and fails loud on anything else. Nothing in the headline fixture
needs `pthread_kill(main, …)`, so the milestone proceeds either way.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs crates/retrace/tests/kport.rs
git commit -m "M16 t1: a thread's mach port, read back out of its own pthread struct"
```

---

### Task 2: The remaining landmarks carry a thread

Moved ahead of the delivery work deliberately: tasks 6–9 assert on `SignalDelivery.thread`, so the
field has to exist before them. Populating all four from `threads().current()` here is *correct* for
`Exit`, `Crash` and `Signal` and is a placeholder only for `SignalDelivery`, which Task 7 changes to
the resolved target.

**Files:** the enum and the magic live in `crates/retrace-trace/src/lib.rs` (the `Event` enum,
`TRACE_MAGIC` at `:45`, the `sample()` fixture, and the magic-pair unit tests). **Everything else is
found by the property, not by a list**, and the list is deliberately not given: on M15, a controller
handed a reviewer four affected assertion sites instead of the property that had changed, a fifth
site survived review, and the enumeration had become the ceiling of the search.

**The property: every site that constructs or exhaustively matches `Event::Exit`, `Event::Crash`,
`Event::Signal`, or `Event::SignalDelivery`, in every crate, including test crates.** A construction
missing the new field is an `E0063` and an exhaustive pattern missing it is an `E0027` — both are
compile errors, so the compiler enumerates for you. Sites that match with `..` compile unchanged and
correctly need no edit.

**Checksum, measured at `b7860a6`** — use it to confirm your sweep was complete, not to bound it.
15 files mention those four variants, across 5 crates:

| Crate | Files |
|---|---|
| `retrace-trace` | `src/lib.rs` (5 mentions) |
| `retrace-core` | `src/lib.rs` (11), `tests/{crash,protnone_mach,record,replay,signals}.rs` (20 total) |
| `retrace-box` | `src/lib.rs` (1) |
| `retrace` | `src/debug.rs` (1), `tests/{crashy_cli,panic_e2e,protnone_rust_e2e,segv_rust_e2e,sigraise_e2e}.rs` (19 total) |
| `retrace-arch` | `src/lib.rs` (1) |

Most are `matches!`/`..` patterns that will not break. **The number that matters is how many the
compiler makes you touch, and whether any file in that table compiled untouched for a reason you can
state** — if a file in this table needed no edit, know why before concluding it needed none.

**Interfaces:**
- Produces: `Event::Exit { code: u64, thread: u32 }`,
  `Event::Crash { pc: u64, esr: u64, far: u64, thread: u32 }`,
  `Event::Signal { sig: u64, pc: u64, thread: u32 }`,
  `Event::SignalDelivery { sig, si_code, si_addr, handler, resume_pc, writes, thread: u32 }`,
  `TRACE_MAGIC == *b"RT\x00\x08"`.

**The semantics, which are NOT uniform and are easy to get backwards:** `Signal` tags the
**raising** thread; `SignalDelivery` tags the **receiving** thread. `Signal` is terminal — nothing
runs afterwards, so "who received it" names a thread that never executes again, while its `pc` field
is documented as naming the *raise site*. Tagging the raiser keeps `pc` and `thread` describing the
same event. `SignalDelivery` is not terminal and its receiver is the thread that goes on to run the
handler, which is the entire point of this milestone.

- [ ] **Step 1: Write the failing tests** in `crates/retrace-trace/src/lib.rs`'s test module

M15 shipped this pair; extend it rather than replacing it. Rename the existing magic test's reason
and add the round-trip:

```rust
#[test]
fn magic_bumped_for_the_landmark_thread_tags() {
    // M16: Exit/Crash/Signal/SignalDelivery each gained `thread`, so a pre-M16 trace is not merely
    // older — it is missing data the reader requires. Two tests, because "forgot to bump" and
    // "bumped to the wrong value" are different mistakes.
    assert_eq!(TRACE_MAGIC, *b"RT\x00\x08");
}

#[test]
fn a_trace_written_with_the_previous_magic_is_rejected_whole() {
    let dir = std::env::temp_dir().join(format!("rt-m16-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("old.bin");
    std::fs::write(&p, b"RT\x00\x07rest-of-a-trace-that-will-never-be-read").unwrap();
    assert!(open_checked(&p).unwrap().is_empty(),
        "a magic mismatch must keep NOTHING, not misparse the tail");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_four_new_thread_tags_round_trip() {
    let evs = vec![
        Event::Exit { code: 7, thread: 3 },
        Event::Crash { pc: 0x1000, esr: 0x24, far: 0x2000, thread: 1 },
        Event::Signal { sig: 6, pc: 0x3000, thread: 2 },
        Event::SignalDelivery { sig: 30, si_code: 0x10001, si_addr: 0, handler: 0x4000,
                                resume_pc: 0x5000, writes: vec![], thread: 1 },
    ];
    let dir = std::env::temp_dir().join(format!("rt-m16-rt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("rt.bin");
    let mut w = Writer::create(&p).unwrap();
    for e in &evs { w.append(e).unwrap(); }
    drop(w);
    assert_eq!(Reader::open(&p).unwrap().iter().cloned().collect::<Vec<_>>(), evs);
    std::fs::remove_dir_all(&dir).ok();
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p retrace-trace -- --test-threads=1`
Expected: FAIL to compile — the four variants have no `thread` field; and the magic assert fails.

- [ ] **Step 3: Add the fields and bump the magic**

In `crates/retrace-trace/src/lib.rs`:

```rust
pub const TRACE_MAGIC: [u8;4] = *b"RT\x00\x08"; // "RT" + format version 0x0008 (M16: landmark threads)
```

```rust
    Exit { code: u64, thread: u32 },
    Crash { pc: u64, esr: u64, far: u64, thread: u32 },
    /// `thread` is the **raising** thread, matching `pc`'s raise-site semantics. Terminal, so the
    /// receiver never runs again and tagging it would describe a different event than `pc` does.
    Signal { sig: u64, pc: u64, thread: u32 },
    /// `thread` is the **receiving** thread — the one that goes on to run the handler, which may
    /// not be the thread that raised the signal. The raiser is already tagged on the `pthread_kill`
    /// `Syscall` landmark immediately preceding this one; on the fault path there is no raiser.
    SignalDelivery {
        sig: u64, si_code: u64, si_addr: u64, handler: u64, resume_pc: u64, writes: Vec<Region>,
        thread: u32,
    },
```

- [ ] **Step 4: Follow the compiler**

Every construction site is an `E0063` (missing field) — the safe failure. Populate each from the
recorder's live thread: `thread: b.threads().current() as u32` in `record_box`, and the matching
binding or `..` in `ReplaySession::advance` and `crates/retrace/src/debug.rs`.

Then run the backstop against the *dangerous* failure — a site that compiles while writing a
defaulted id:

Run: `grep -n "thread: 0" crates/retrace-core/src/lib.rs`
Expected: no hits inside `record_box`. A literal `0` there is a site that will silently mis-tag a
threaded guest. (`crates/retrace-trace`'s `sample()` fixture may legitimately use a constant.)

- [ ] **Step 5: Run the trace crate and the signal suites**

Run: `cargo test -p retrace-trace -- --test-threads=1`
Then: `cargo test -p retrace --test panic_e2e --test segv_rust_e2e --test crashy_cli -- --test-threads=1`
Expected: PASS. These three cover `Signal`, `SignalDelivery`, and `Crash` respectively.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-trace/src/lib.rs crates/retrace-core/src/lib.rs crates/retrace/src/debug.rs
git commit -m "M16 t2: Exit, Crash, Signal and SignalDelivery name a thread"
```

---

### Task 3: Mask, pending set and alternate stack become per-thread

**Files:** the split itself lands in `crates/retrace-box/src/thread.rs` (`Thread`, `ThreadTable`) and
`crates/retrace-box/src/sig.rs` (`SigTable` sheds `blocked` and `altstack`), with the new unit tests
in `thread.rs` (VM-free — no `--test-threads=1` needed).

**Every call site is found by the property, not by a list:** every use of
`SigTable::{is_blocked, mask, set_mask, altstack, set_altstack}` anywhere outside `sig.rs` itself.
Each becomes a compile error when the method disappears, so the compiler enumerates for you.

**Checksum, measured at `57acfd0`** — 30 sites across 4 files:

| File | Sites |
|---|---|
| `crates/retrace-box/tests/deliver.rs` | 11 |
| `crates/retrace-core/src/lib.rs` | 9 |
| `crates/retrace-box/src/lib.rs` | 6 |
| `crates/retrace-box/tests/sigcheckpoint.rs` | 4 |

**`crates/retrace-box/tests/sigcheckpoint.rs` is the one to handle deliberately, and it is worth
knowing what it guards before you touch it.** Its `from_checkpoint_carries_the_signal_table` (`:12`)
sets a disposition, a mask and an altstack, checkpoints, restores, and asserts all three survived:

```rust
    assert_eq!(r.sigtable().mask(), 0b1010, "the blocked mask must survive the restore");
    assert_eq!(r.sigtable().altstack(), Some((0x9000, 0x4000, 0)));
```

Those two lines are **the only proof anywhere that the checkpoint carries the mask and the alt
stack** — which is exactly the invariant this task's split puts at risk. The spec's claim that
"`BoxState` already carries both `threads` and `sigtable`, so checkpointing survives the split with
no new field" is a claim *this test verifies*.

So: **relocate these assertions, do not delete them.** After the split they read
`r.threads().mask_of(0)` and `r.threads().altstack_of(0)`, and the disposition assertion stays on
`sigtable()`. Deleting them because they stopped compiling would silently drop the proof, leaving a
green suite that no longer checks the thing the split most plausibly breaks. The test's name should
change too — it now covers a signal table *and* per-thread signal state.

**Interfaces:**
- Produces, on `ThreadTable`: `mask_of(tid) -> u32`, `set_mask_of(tid, how: u64, set: u32) -> u32`,
  `is_blocked_for(tid, sig: u64) -> bool`, `pend(tid, sig: u64)`,
  `take_deliverable(tid) -> Option<u64>`, `altstack_of(tid) -> Option<(u64,u64,u64)>`,
  `set_altstack_of(tid, ss) -> Option<(u64,u64,u64)>`.
- `SigTable` keeps `action`/`set_action` **only**.

**Why the split lands here:** POSIX makes dispositions process-wide and masks and alternate stacks
per-thread. `ThreadCtx` is documented as *"one thread's register context"* and a mask is not a
register, so the new fields go on `Thread`, beside `state` and `stack`. `BoxState` already carries
`threads` (cloned wholesale, `lib.rs:632`) and `sigtable` (`:669`), so checkpointing survives the
split with no new `BoxState` field.

**The compatibility argument is M14's, reused:** a single-threaded guest has a one-entry table, so
thread 0 holds the mask and every M0–M15 path behaves identically.

- [ ] **Step 1: Write the failing unit tests** in `crates/retrace-box/src/thread.rs`'s test module

```rust
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

#[test]
fn masks_are_independent_between_threads() {
    let mut t = ThreadTable::new(ThreadCtx::zeroed());
    let child = t.spawn(ThreadCtx::zeroed(), (0, 0));
    t.set_mask_of(0, retrace_arch::SIG_BLOCK, 1 << 29);
    assert!(t.is_blocked_for(0, 30));
    assert!(!t.is_blocked_for(child, 30),
        "this is the whole per-thread claim: main blocking a signal must not block it for the child");
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
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p retrace-box --lib thread`
Expected: FAIL to compile — none of these methods exist.

- [ ] **Step 3: Add the fields and methods**

```rust
#[derive(Clone, Debug)]
pub struct Thread {
    pub ctx: ThreadCtx,
    pub state: ThreadState,
    pub stack: (u64, u64),
    /// M16: bit `(sig - 1)`, the same `sigset_t` encoding `SigTable.blocked` used before the split.
    /// Per-thread because POSIX makes it so; INHERITED by value at `spawn`.
    pub mask: u32,
    /// M16: signals raised for this thread while its mask blocked them. Materialised at the next
    /// unmask, which is a syscall landmark — the anchor that keeps delivery above the trace.
    pub pending: u32,
    /// M16: per-thread, and deliberately NOT inherited — a new thread starts with no alt stack.
    pub altstack: Option<(u64, u64, u64)>,
}
```

Methods on `ThreadTable` (the mask arithmetic is `SigTable::set_mask`'s, moved verbatim including its
fail-loud `how` panic):

```rust
pub fn mask_of(&self, tid: usize) -> u32 { self.threads[tid].mask }

pub fn set_mask_of(&mut self, tid: usize, how: u64, set: u32) -> u32 {
    let old = self.threads[tid].mask;
    self.threads[tid].mask = match how {
        retrace_arch::SIG_BLOCK => old | set,
        retrace_arch::SIG_UNBLOCK => old & !set,
        retrace_arch::SIG_SETMASK => set,
        _ => panic!("sigprocmask how={how} is not BLOCK(1)/UNBLOCK(2)/SETMASK(3) — an unmodelled \
                     value, not a guest error to swallow"),
    };
    old
}

pub fn is_blocked_for(&self, tid: usize, sig: u64) -> bool {
    self.threads[tid].mask & (1u32 << (sig - 1)) != 0
}

pub fn pend(&mut self, tid: usize, sig: u64) { self.threads[tid].pending |= 1u32 << (sig - 1); }

pub fn pending_of(&self, tid: usize) -> u32 { self.threads[tid].pending }

/// The lowest-numbered pending signal this thread's mask no longer blocks, CLEARED as it is taken.
pub fn take_deliverable(&mut self, tid: usize) -> Option<u64> {
    let t = &mut self.threads[tid];
    let ready = t.pending & !t.mask;
    if ready == 0 { return None; }
    let sig = ready.trailing_zeros() as u64 + 1;
    t.pending &= !(1u32 << (sig - 1));
    Some(sig)
}

pub fn altstack_of(&self, tid: usize) -> Option<(u64, u64, u64)> { self.threads[tid].altstack }

pub fn set_altstack_of(&mut self, tid: usize, ss: Option<(u64, u64, u64)>)
    -> Option<(u64, u64, u64)> { std::mem::replace(&mut self.threads[tid].altstack, ss) }
```

`new` seeds `mask: 0, pending: 0, altstack: None`. `spawn` inherits the mask and nothing else:

```rust
pub fn spawn(&mut self, ctx: ThreadCtx, stack: (u64, u64)) -> usize {
    // POSIX inherits the creating thread's signal mask at pthread_create, BY VALUE. The alternate
    // stack is NOT inherited — a new thread starts with none.
    let mask = self.threads[self.current].mask;
    self.threads.push(Thread { ctx, state: ThreadState::Runnable, stack,
                               mask, pending: 0, altstack: None });
    self.threads.len() - 1
}
```

- [ ] **Step 4: Delete the moved state from `SigTable` and follow the compiler**

Remove `blocked`, `altstack`, `is_blocked`, `mask`, `set_mask`, `altstack`, `set_altstack` from
`crates/retrace-box/src/sig.rs`, and update its doc comment: it is now the disposition table alone.
Retarget every call site to the relevant thread:

- `Box_::on_altstack` (`lib.rs`, ~`:2734`) — the **current** thread's altstack and the live `SP_EL0`
- `deliver_signal` (~`:2815`) — the current thread for now; Task 6 parameterises it
- the `sigreturn` mask restore (`:2801`) — the **returning** (current) thread
- `retrace-core`'s `sigprocmask`/`pthread_sigmask` arm (~`:518`) and its replay mirror (~`:1126`) —
  the **calling** thread
- the `sigaltstack` arm — the **calling** thread
- the raise arm's `is_blocked` assert (`:584`) — leave it targeting the current thread; Task 7
  replaces the assert with the pending path

- [ ] **Step 5: Run the unit tests and the signal suites**

Run: `cargo test -p retrace-box --lib thread`
Then: `cargo test -p retrace-box --lib sig`
Then: `cargo test -p retrace --test panic_e2e --test thread_rust_e2e -- --test-threads=1`
Expected: PASS. This is a behaviour-preserving refactor — a regression here is a retarget you got
wrong, not a design problem.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-box/src/thread.rs crates/retrace-box/src/sig.rs \
        crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs
git commit -m "M16 t3: the mask, the pending set and the alt stack belong to a thread"
```

---

### Task 4: A mach port resolves to a thread

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (`thread_of_port`, beside Task 1's `kport_of`)
- Modify: `crates/retrace-core/src/lib.rs` (the `dbg_thread_of_port` passthrough, beside `dbg_kport_of`)
- Test: `crates/retrace/tests/kport.rs` (extend Task 1's file)

**Interfaces:**
- Produces: `Box_::thread_of_port(&self, port: u32) -> usize` — fail-loud, never `Option`.
- Consumes: Task 1's `Box_::kport_of`.

- [ ] **Step 1: Write the failing test**

```rust
/// M16 Task 4. Resolution is a search over LIVE threads' own kport fields, so it covers main
/// without a special case and cannot silently fall back to "whoever is running" — which is exactly
/// today's latent bug.
#[test]
fn a_port_resolves_to_the_thread_that_owns_it() {
    let (rec, trace) = util::record_dynamic(retrace_guest::THREADRUST);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);
    let mut s = ReplaySession::open(Path::new(&trace)).unwrap();
    seek_to_two_threads(&mut s);

    for tid in [0usize, 1] {
        let port = s.dbg_kport_of(tid).expect("readable");
        assert_eq!(s.dbg_thread_of_port(port), tid,
            "each thread's own port must resolve back to it");
    }
}
```

**Step 1a: first extract the seek Task 1 already got right.** Task 1's implementer replaced an
unbounded `while s.b_thread_count() < 2 { s.advance().unwrap(); }` with a bounded loop, because
`advance()`'s own doc says calling it after `Advance::Exited` is unspecified — so the unbounded form
runs off the end of the trace on any guest that never spawns. Do not write that loop back into the
same file, and do not copy the bounded one either (two copies drift, and verbatim duplication of a
logic block is a review finding). Lift Task 1's loop out of
`every_live_thread_has_a_distinct_readable_kport` into a file-local helper and have BOTH tests call
it:

```rust
/// Advance until the child exists, or fail loud naming what we were waiting for. Shared by both
/// tests in this file: the bound is the point (see the panic), and two copies of it would drift.
fn seek_to_two_threads(s: &mut ReplaySession) {
    loop {
        if s.b_thread_count() >= 2 { return; }
        match s.advance().expect("no divergence on an untampered trace") {
            retrace_core::Advance::Exited(_) =>
                panic!("the recording ended with only {} thread(s): THREADRUST must spawn one, so \
                        either the guest changed or bsdthread_create was not emulated",
                       s.b_thread_count()),
            _ => continue,
        }
    }
}
```

Task 1's test must still pass unchanged after the refactor.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p retrace --test kport -- --test-threads=1`
Expected: FAIL to compile — no `dbg_thread_of_port`.

- [ ] **Step 3: Implement**

```rust
/// Which thread owns `port`.
///
/// FAIL-LOUD, not `Option`: the alternative is defaulting to the current thread, which is the exact
/// latent defect M16 exists to close — `pthread_kill(child, sig)` running MAIN's handler on MAIN's
/// stack, silently. A port retrace cannot place is a modelling gap that must surface as a panic.
pub fn thread_of_port(&self, port: u32) -> usize {
    // `Option<u32>`, not a 0 sentinel: an unreadable pthread and a thread whose kport genuinely
    // reads 0 are different diagnoses, and this panic's whole job is to tell them apart.
    let mut seen: Vec<(usize, Option<u32>)> = Vec::new();
    for tid in 0..self.threads.len() {
        if matches!(self.threads.state_of(tid), thread::ThreadState::Exited(_)) { continue; }
        let p = self.kport_of(tid);
        if p == Some(port) { return tid; }
        seen.push((tid, p));
    }
    panic!("__pthread_kill names mach port {port:#x}, which belongs to no live guest thread \
            (searched {seen:x?} as (tid, kport)). Either the guest holds a port retrace never \
            issued, or its pthread struct moved — measure before widening this.");
}
```

Add the `ReplaySession` passthrough beside `dbg_kport_of`:

```rust
#[doc(hidden)]
pub fn dbg_thread_of_port(&self, port: u32) -> usize { self.b.thread_of_port(port) }
```

- [ ] **Step 4: Run**

Run: `cargo test -p retrace --test kport -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs crates/retrace/tests/kport.rs
git commit -m "M16 t4: a mach port names a thread, or the recorder stops"
```

---

### Task 5: A guest that is both threaded and signalling

The fixture arrives *before* the mechanism that needs it, deliberately — M15 shipped a plan where
Task 8 required a guest only Task 9 built, and the tasks had to be executed in reverse. This task's
guest is the one tasks 6–9 test against.

**This task ships the guest WITHOUT the masking steps.** Main masking itself belongs to Task 9,
which builds per-thread masks into the syscall arms; adding it here would make this task's own test
depend on a feature three tasks away.

**Files:**
- Create: `crates/retrace-guest/rs/sigthread.rs`
- Modify: `crates/retrace-guest/build.rs` (a recipe beside `watchthread`'s, ~`:474`)
- Modify: `crates/retrace-guest/src/lib.rs` (a `SIGTHREAD` const beside `WATCHTHREAD`, `:175`)
- Test: `crates/retrace/tests/sigthread_e2e.rs` (new — a viability check now, the headline gate at
  Task 10)

**Interfaces:**
- Produces: `retrace_guest::SIGTHREAD`.

- [ ] **Step 1: Write the guest**

```rust
// crates/retrace-guest/rs/sigthread.rs
//
// M16's headline guest: the first that is both THREADED and SIGNALLING. The oracle's caught-raise
// and sigreturn mirrors have never been reached by a guest with two live threads (M15's standing
// fidelity caveat), and `pthread_kill`'s target port has never been decoded at all.
//
// The ordering is the proof, not a convenience:
//   * the child is spawned BEFORE main masks anything, so it inherits an empty mask (Task 9)
//   * main signals the child while the child is Runnable-but-NOT-current — the cooperative
//     scheduler switches only on block or exit, so main still holds the vCPU
//   * the child therefore takes the signal in its NEVER-RUN entry context: the handler runs, then
//     sigreturn lands on `thread_start_pc` and the body starts.
//
// THE STDOUT LINE ORDER IS THIS GUEST'S REAL OBSERVABLE, and it is worth stating exactly, because
// all of it is MEASURED (record-dyn'd through the CLI before Task 5 was written):
//
//   native, 20/20 identical runs:  installed | child pthread | kill rc 0 | handler | child body | joined
//   retrace TODAY (pre-Task 7):    installed | child pthread | handler | kill rc 0 | child body | joined
//
// The inversion of `handler` and `kill rc 0` IS the defect M16 closes, made visible. Today retrace
// ignores __pthread_kill's target port and delivers to whoever is running — main — synchronously
// inside the pthread_kill syscall, so the handler prints BEFORE the syscall returns. Natively the
// CHILD takes it, so main's pthread_kill returns first. After Task 7 the child takes it here too:
// main prints "kill rc 0", blocks in join, and only then does the child run its handler and body —
// i.e. retrace's order becomes native's order, exactly.
//
// So Task 5's test asserts the WRONG order on purpose, documenting the bug; Task 7 flips it. Both
// assert against retrace's own recorded behaviour rather than against a native execution, because
// POSIX guarantees no such ordering — a native run is not a specification even when it reproduces
// 20/20 on one host. That retrace's post-M16 order happens to equal native's is a fidelity result
// worth reporting, not the thing being tested.
//
// Task 5 ships steps 1-4. Task 9 appends the mask/pending half.
//
// Same rustc recipe as watchthread: no -C panic=abort.

// PLAIN `extern "C"`, not `unsafe extern "C"`. build.rs invokes rustc with no `--edition`, so
// every Rust guest compiles as edition 2015, where `unsafe extern` is a syntax error. The only
// other Rust guest with an extern block, `protrust.rs:17`, uses this form. MEASURED: this file
// compiles and runs with the exact build.rs recipe; the `unsafe extern` form does not.
extern "C" {
    fn pthread_kill(thread: u64, sig: i32) -> i32;
    fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
    #[link_name = "write"]
    fn libc_write(fd: i32, buf: *const u8, n: usize) -> isize;
}

// SA_SIGINFO, installed via `sigaction` — NOT `signal(3)`. MEASURED: a `signal()`-installed
// handler is non-SA_SIGINFO, and `build_frame` (sig.rs:262) asserts fail-loud on exactly that:
// "a non-SA_SIGINFO handler is not modelled. Its infostyle is 0x1 (measured, vs 0x1e for
// SA_SIGINFO) and the frame layout is identical, so supporting it is small — but no gate guest
// exercises it." That wall is real and is NOT M16's to clear: infostyle is unrelated to thread
// attribution, and clearing it here would be scope creep into a different modelling gap. The
// assert stays honest and untouched; this guest simply installs the shape the box models.
//
// macOS `struct sigaction`: 8-byte handler union, 4-byte sigset_t mask, 4-byte flags. libc's
// wrapper fills in sa_tramp itself, so the guest never declares it.
#[repr(C)]
struct SigAction { handler: usize, mask: u32, flags: i32 }

const SA_SIGINFO: i32 = 0x0040;

const SIGUSR1: i32 = 30;

extern "C" fn on_usr1(_sig: i32, _info: *mut u8, _uap: *mut u8) {
    // A raw write(2) rather than println!: a handler must not take libstd's stdout lock, which the
    // interrupted thread may already hold. The child is the only thread that runs this in Task 5,
    // but the Task 9 half runs it on main too.
    let msg = b"handler\n";
    unsafe { libc_write(1, msg.as_ptr(), msg.len()) };
}

fn main() {
    // `as *const () as usize`, not `as usize`: rustc 1.95 warns `direct cast of function item
    // into an integer` (function_casts_as_integer, on by default) on the direct form, and a guest
    // that warns on every build is not pristine output.
    let act = SigAction { handler: on_usr1 as *const () as usize, mask: 0, flags: SA_SIGINFO };
    assert_eq!(unsafe { sigaction(SIGUSR1, &act, core::ptr::null_mut()) }, 0);
    println!("installed");

    let h = std::thread::spawn(|| {
        println!("child body");
    });

    // The child's pthread_t, which is what `pthread_kill` names. std exposes it; no libc crate.
    use std::os::unix::thread::JoinHandleExt;
    let child = h.as_pthread_t() as u64;
    println!("child pthread {child:#x}");

    let rc = unsafe { pthread_kill(child, SIGUSR1) };
    println!("kill rc {rc}");

    h.join().unwrap();
    println!("joined");
}
```

- [ ] **Step 2: Wire the build**

In `crates/retrace-guest/build.rs`, after the `watchthread` block:

```rust
    // sigthread: M16's headline — a full-std Rust binary whose MAIN signals its CHILD by name.
    // Same rustc recipe as watchthread.
    let src = format!("{}/rs/sigthread.rs", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/sigthread");
    println!("cargo:rerun-if-changed={src}");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let status = Command::new(rustc)
        .args(["--target", "aarch64-apple-darwin", "-o", &bin, &src])
        .status().expect("rustc sigthread");
    assert!(status.success(), "sigthread guest build failed");
```

In `crates/retrace-guest/src/lib.rs`, beside `WATCHTHREAD`:

```rust
pub const SIGTHREAD: &str = concat!(env!("OUT_DIR"), "/sigthread");
```

- [ ] **Step 3: Write the viability test**

```rust
// crates/retrace/tests/sigthread_e2e.rs
//
// M16 Task 5: the fixture exists and records. This is NOT yet the attribution gate — at this point
// retrace still ignores pthread_kill's target port and delivers to whoever is running, so the
// handler line here proves only that the guest is viable, not that the RIGHT thread took it.
// Task 7 replaces this test's body with the attribution assertion.
mod util;

#[test]
fn the_sigthread_guest_records_and_replays_with_main_wrongly_taking_the_signal() {
    let (rec, trace) = util::record_dynamic(retrace_guest::SIGTHREAD);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    // The ORDER, not a bag of contains() — a set-membership check passes identically before and
    // after Task 7 and would prove nothing. `handler` before `kill rc 0` means the handler ran
    // synchronously inside main's pthread_kill: main took the signal it aimed at the child. That
    // is today's defect, asserted rather than tolerated, so Task 7's flip is a visible change.
    // EVERY line, in order — not a filtered allowlist. A filter silently drops output the guest
    // was not expected to produce, so a spurious warning or a second delivery would pass unnoticed;
    // the length check is what closes that. The pthread line is matched by PREFIX rather than by
    // value: its address is deterministic today (0x30207000, measured), but pinning a guest address
    // would make this test fail for any unrelated change to the box's memory layout, which is a
    // false alarm rather than a finding.
    let out = String::from_utf8_lossy(&rec.stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 6, "unexpected extra or missing stdout line:\n{out}");
    assert_eq!(lines[0], "installed");
    assert!(lines[1].starts_with("child pthread 0x"), "line 2 was {:?}", lines[1]);
    assert_eq!(&lines[2..], &["handler", "kill rc 0", "child body", "joined"],
        "pre-Task-7 order: main takes its own signal inside pthread_kill. Full stdout:\n{out}");

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay must be clean; stderr:\n{}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout, "replay must be byte-identical");
}
```

- [ ] **Step 4: Run it**

Run: `cargo test -p retrace --test sigthread_e2e -- --test-threads=1 --nocapture`
Expected: PASS.

**This guest is already PROVEN viable — it is not a leap of faith.** Before this task was
finalised the controller compiled exactly this source with the build.rs recipe and ran it through
the CLI directly (`record-dyn` then `replay`): both exited 0, stdout was byte-identical between
record and replay, and the line order was the pre-Task-7 order this test asserts. So the frame
machinery already works for a libc-installed SA_SIGINFO handler on a guest that is ALSO threaded —
M16's single biggest unknown, answered before the task was dispatched. A failure here is therefore
a regression from something in Tasks 1-4, not an expected wall.

**If it fails anyway, the failure is information, and which failure matters.** Record the exact
message in your report:
- `a non-SA_SIGINFO handler is not modelled` (sig.rs:262) → the handler got installed with
  `signal(3)` instead of `sigaction`+SA_SIGINFO. This is the wall the guest is written to avoid;
  it is NOT M16's to clear. Fix the guest, not the box.
- a `brk`/PAC fault inside the handler → the frame the box builds is wrong for a libc-installed
  handler; measure before changing anything
- `__pthread_kill names mach port …, which belongs to no live guest thread` → Task 4 is working and
  the guest passes a port shape R1's fallback did not anticipate
- an assert about a blocked signal → unexpected here; the guest masks nothing in Task 5

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-guest/rs/sigthread.rs crates/retrace-guest/build.rs \
        crates/retrace-guest/src/lib.rs crates/retrace/tests/sigthread_e2e.rs
git commit -m "M16 t5: a guest whose main signals its child by name"
```

---

### Task 6: Delivery targets a thread, not the vCPU

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (`deliver_signal`, ~`:2815` → `deliver_signal_to`, plus a
  `dbg_fp_lr` accessor beside `vcpu_get_x` at `:2749`)
- Test: `crates/retrace-box/tests/deliver.rs`

**Interfaces:**
- Produces: `Box_::deliver_signal_to(&mut self, tid: usize, sig: u64, si_code: u64, si_addr: u64,
  esr: u64, far: u64) -> (Vec<Region>, u64)`. `deliver_signal(...)` stays as a thin wrapper
  delegating to `deliver_signal_to(self.threads.current(), …)`, so **no existing call site changes**.
- Consumes: Task 3's `ThreadTable::{mask_of, set_mask_of, altstack_of}`.

**The state `deliver_signal` reads today, and where it comes from in a `ThreadCtx`** — enumerated
rather than assumed, because spec R2 is exactly the risk of missing one:

| Live read | `ThreadCtx` source |
|---|---|
| `x0..x28` via `reg::x(i)` | `ctx.regs.x[0..29]` |
| `reg::FP`, `reg::LR` | `ctx.regs.x[29]`, `ctx.regs.x[30]` — pinned by a test below |
| `SP_EL0` | `ctx.regs.sp_el0` |
| `ELR_EL1` (as `ts.pc`) | `ctx.elr` — **not** `ctx.regs.pc`, which is the resume point |
| `SPSR_EL1` (as `ts.cpsr`) | `ctx.spsr` — **not** `ctx.regs.cpsr` |
| `q0..q31` | `ctx.fp` |
| `FPCR`, `FPSR` | `ctx.fpcr`, `ctx.fpsr` |

Every one is covered, so no `ThreadCtx` field needs adding. **`regs.pc` vs `elr` and `regs.cpsr` vs
`spsr` are the two easy mistakes** — at a syscall trap `regs.pc` is the trampoline the vCPU resumes
at while `elr` is the guest's own next instruction, and `sigreturn` must come back to the latter.

- [ ] **Step 1: Write the failing tests** in `crates/retrace-box/tests/deliver.rs`

```rust
/// M16 Task 6. `deliver_signal_to` sources FP/LR from `ThreadCtx.regs.x[29]`/`[30]`, because a
/// saved context has no separate FP/LR field. That is only correct if HVF aliases the registers.
#[test]
fn x29_and_x30_are_the_frame_pointer_and_link_register() {
    let mut b = boxed();
    b.vcpu_set_x(29, 0xF00D_0000_0000_0001);
    b.vcpu_set_x(30, 0xF00D_0000_0000_0002);
    let r = b.regs_snapshot();
    assert_eq!((r.x[29], r.x[30]), (0xF00D_0000_0000_0001, 0xF00D_0000_0000_0002));
    assert_eq!(b.dbg_fp_lr(), (0xF00D_0000_0000_0001, 0xF00D_0000_0000_0002),
        "HV_REG_FP/HV_REG_LR must alias X29/X30. If they do not, deliver_signal_to must carry them \
         as their own ThreadCtx fields instead of reading regs.x — measure, do not paper over it.");
}

/// M16 Task 6, the headline unit property: a signal delivered to a thread that is NOT running
/// lands on THAT thread's stack and redirects THAT thread, leaving the running one untouched.
///
/// This is the latent defect M16 closes, expressed at the smallest level that can express it.
#[test]
fn delivering_to_a_non_current_thread_leaves_the_running_one_alone() {
    let mut b = boxed();
    b.sigtable_mut().set_action(30, handler(retrace_arch::SA_SIGINFO, 0));

    // A second thread whose context is main's but on a DIFFERENT stack, so the frame's address
    // alone says which thread's stack it landed on.
    let mut ctx = b.save_ctx();
    let other_sp = ctx.regs.sp_el0 - 0x2000;
    ctx.regs.sp_el0 = other_sp;
    let elr_of_other = ctx.elr;
    let tid = b.threads_mut().spawn(ctx, (other_sp, 0));
    assert_eq!(tid, 1);

    let before = b.regs_snapshot();
    let (writes, resume_pc) = b.deliver_signal_to(tid, 30, retrace_arch::SI_USER, 0, 0, 0);

    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].ipa, other_sp - 128 - FRAME_LEN as u64,
        "the frame must land on thread 1's stack; landing on the running thread's stack IS the bug");
    assert_eq!(resume_pc, elr_of_other, "sigreturn returns to the TARGET's own next instruction");

    let after = b.regs_snapshot();
    assert_eq!((after.pc, after.sp_el0, after.x[0]), (before.pc, before.sp_el0, before.x[0]),
        "delivering to another thread must not redirect the running one");

    let t = b.threads().ctx_of(tid);
    assert_eq!(t.regs.pc, TRAMP, "thread 1 enters the trampoline when it is next scheduled");
    assert_eq!(t.regs.sp_el0, writes[0].ipa, "and resumes on the frame it was given");
    assert_eq!(t.regs.x[0], 0xabc0, "x0 = the catcher, in the TARGET's context");

    assert!(b.threads().is_blocked_for(tid, 30),
        "the signal is blocked for the handler's duration on the RECEIVING thread");
    assert!(!b.threads().is_blocked_for(0, 30),
        "and not on the thread that merely raised it");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p retrace-box --test deliver -- --test-threads=1`
Expected: FAIL to compile — no `deliver_signal_to`, no `dbg_fp_lr`.

- [ ] **Step 3: Add `dbg_fp_lr` and rewrite `deliver_signal`**

```rust
/// Test-only: `reg::FP`/`reg::LR` straight off the vCPU, so a test can pin that they alias X29/X30.
#[doc(hidden)]
pub fn dbg_fp_lr(&self) -> (u64, u64) {
    (self.vcpu.get_reg(reg::FP).unwrap(), self.vcpu.get_reg(reg::LR).unwrap())
}
```

**`on_altstack` becomes per-thread rather than being inlined.** It has exactly one product caller
(`deliver_signal`) plus two assertions in `deliver.rs`. Recomputing its predicate inline inside
`deliver_signal_to` would duplicate its body — the same duplicate-logic defect this task's design
note exists to prevent — and would leave `on_altstack()` product-dead, alive only for two tests,
which is how a helper rots into something wrong that nobody notices. Instead:

```rust
/// Is thread `tid` currently executing on ITS OWN alternate signal stack?
///
/// Same current/non-current discipline as `pthread_of` and `dbg_regs_of`: the running thread's
/// stack pointer is live in `SP_EL0` and its table entry is stale between switches, so only a
/// NON-current thread is read from the table. That distinction is load-bearing rather than
/// stylistic — inside `deliver_signal_to` the current thread's ctx has just been saved, so the
/// table would be right there, but `on_altstack()` called from anywhere else has had no such save
/// and reading the table unconditionally would answer from stale state.
pub fn on_altstack_of(&self, tid: usize) -> bool {
    let sp = if tid == self.threads.current() {
        self.vcpu.get_sys(sysreg::SP_EL0).unwrap()
    } else {
        self.threads.ctx_of(tid).regs.sp_el0
    };
    matches!(self.threads.altstack_of(tid), Some((base, size, _)) if sp >= base && sp < base + size)
}

/// The running thread's alternate-stack membership. Unchanged for every existing caller.
pub fn on_altstack(&self) -> bool { self.on_altstack_of(self.threads.current()) }
```

```rust
/// Deliver `sig` to thread `tid`, which need NOT be the running one.
///
/// The table is the authority for EVERY thread, including the running one whose entry is stale
/// between switches — so the current context is saved into it first and reloaded at the end. That
/// is what lets one code path serve a self-signal and a cross-thread signal identically, instead of
/// two paths that drift (M13 Task 8 shipped a test that checked only one of a mirrored pair).
pub fn deliver_signal_to(
    &mut self, tid: usize, sig: u64, si_code: u64, si_addr: u64, esr: u64, far: u64,
) -> (Vec<Region>, u64) {
    let act = self.sigtable.action(sig);
    let cur = self.threads.current();
    *self.threads.ctx_mut(cur) = self.save_ctx();

    let ctx = self.threads.ctx_of(tid).clone();
    let mut x = [0u64; 29];
    x.copy_from_slice(&ctx.regs.x[..29]);
    let ts = ThreadState {
        x,
        fp: ctx.regs.x[29],
        lr: ctx.regs.x[30],
        // ELR, not regs.pc: regs.pc is where the vCPU RESUMES (the trampoline, at a trap), while
        // ELR_EL1 is the guest's own next instruction — `position()`'s source, and what sigreturn
        // must come back to. Likewise spsr, not regs.cpsr.
        sp: ctx.regs.sp_el0,
        pc: ctx.elr,
        cpsr: ctx.spsr,
    };
    let ns = NeonState { v: ctx.fp, fpsr: ctx.fpsr as u32, fpcr: ctx.fpcr as u32 };

    // The TARGET's alt stack, and whether the TARGET is already on it — not the running thread's.
    let alt = self.threads.altstack_of(tid);
    let (frame_base, on_alt) = choose_frame_base(ts.sp, act, alt, self.on_altstack_of(tid));

    let inp = FrameInput {
        sig, si_code, si_addr, esr, far, ts, ns,
        mask: self.threads.mask_of(tid),   // the PRE-signal mask: what sigreturn restores
        act, frame_base, on_alt,
    };
    let (bytes, entry) = build_frame(&inp);
    self.write_guest(frame_base, &bytes);

    // Block for the handler's duration, unless SA_NODEFER — on the RECEIVING thread.
    let mut newmask = self.threads.mask_of(tid) | act.mask;
    if act.flags & retrace_arch::SA_NODEFER == 0 { newmask |= 1 << (sig - 1); }
    self.threads.set_mask_of(tid, retrace_arch::SIG_SETMASK, newmask);
    // SA_RESETHAND changes a DISPOSITION, which stays process-global.
    if act.flags & retrace_arch::SA_RESETHAND != 0 {
        self.sigtable.set_action(sig, SigAction { disp: Disposition::Dfl, ..act });
    }

    {
        let c = self.threads.ctx_mut(tid);
        for (i, xi) in entry.x.iter().enumerate() { c.regs.x[i] = *xi; }
        c.regs.sp_el0 = entry.sp;
        // The vCPU resumes at reg::PC, which load_ctx writes from regs.pc — so the trampoline
        // address goes THERE, exactly as the pre-M16 code wrote reg::PC and not ELR_EL1.
        c.regs.pc = entry.pc;
        c.regs.cpsr = ctx.spsr;
    }

    let back = self.threads.ctx_of(cur).clone();
    self.load_ctx(&back);

    (vec![Region { ipa: frame_base, bytes }], ts.pc)
}

/// M11/M12's entry point, unchanged for every existing caller: deliver to the running thread.
pub fn deliver_signal(
    &mut self, sig: u64, si_code: u64, si_addr: u64, esr: u64, far: u64,
) -> (Vec<Region>, u64) {
    self.deliver_signal_to(self.threads.current(), sig, si_code, si_addr, esr, far)
}
```

- [ ] **Step 4: Run the delivery suite and the signal e2es**

Run: `cargo test -p retrace-box --test deliver -- --test-threads=1`
Then: `cargo test -p retrace-core --test replay -- --test-threads=1`
Then: `cargo test -p retrace --test panic_e2e --test segv_rust_e2e --test protnone_rust_e2e -- --test-threads=1`
Expected: PASS. The self-signal path must be **byte-identical** to before — every one of these
existed before M16 and none of them has a second thread.

- [ ] **Step 5: Prove the new test is not vacuous**

Temporarily change `deliver_signal_to`'s `let ctx = self.threads.ctx_of(tid).clone();` to
`ctx_of(cur)`, i.e. reintroduce the bug.
Expected: `delivering_to_a_non_current_thread_leaves_the_running_one_alone` FAILS on the frame
address; the existing M12 tests stay GREEN. Revert. Record both outcomes in your report.

- [ ] **Step 6: Add the fail-loud double-signal boundary**

The spec's remaining fail-loud boundary, and it has to live here because `deliver_signal_to` is the
only place that knows a delivery happened. Stacking a second frame on a thread that has been
redirected but has not yet run would need the kernel's queueing semantics, which retrace does not
model — and nested delivery is already on M11's unmodelled list.

`Thread` gains one field — which breaks Task 3's `spawn` struct literal with an `E0063`. That is
the safe, compiler-forced failure; add the field there too and move on.

```rust
    /// M16: this thread has been redirected into a handler and has not been scheduled since.
    /// A second signal arriving now would stack a frame on a context that never ran the first —
    /// fail loud rather than guess at the kernel's queueing order.
    pub redirected: bool,
```

`deliver_signal_to` asserts and sets it, for a non-current target only (a self-signal runs its
handler immediately, so there is no un-run frame to stack on):

```rust
    if tid != cur {
        assert!(!self.threads.is_redirected(tid),
            "thread {tid} was already redirected into a handler and has not run since; a second              signal here would stack frames without the kernel's queueing semantics. Nested              delivery is unmodelled — implement queueing before a guest needs this.");
    }
```

and sets `self.threads.set_redirected(tid, tid != cur)` after the ctx rewrite.
`ThreadTable::switch_to` clears it for the thread being switched *to*, because that thread is now
running the handler it was given.

Test it:

```rust
#[test]
fn a_second_signal_to_an_unrun_redirected_thread_fails_loud() {
    let mut b = boxed();
    b.sigtable_mut().set_action(30, handler(retrace_arch::SA_SIGINFO, 0));
    let mut ctx = b.save_ctx();
    ctx.regs.sp_el0 -= 0x2000;
    let tid = b.threads_mut().spawn(ctx, (0, 0));
    b.deliver_signal_to(tid, 30, retrace_arch::SI_USER, 0, 0, 0);
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.deliver_signal_to(tid, 30, retrace_arch::SI_USER, 0, 0, 0);
    })).is_err();
    assert!(panicked, "stacking a frame on a context that never ran the first must not be silent");
}
```

Run: `cargo test -p retrace-box --test deliver -- --test-threads=1`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/src/thread.rs \
        crates/retrace-box/tests/deliver.rs
git commit -m "M16 t6: a signal is delivered to a thread, not to whoever is running"
```

---

### Task 7: The recorder resolves `__pthread_kill`'s target

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` — the raise arm in `record_box`, the `match` guard
  `if num == retrace_arch::SYS_KILL || num == retrace_arch::SYS_PTHREAD_KILL` (~`:572`)
- Test: `crates/retrace/tests/sigthread_e2e.rs`

**Interfaces:**
- Consumes: Task 4's `Box_::thread_of_port`, Task 6's `Box_::deliver_signal_to`, Task 3's
  `ThreadTable::{is_blocked_for, pend}`.
- Produces: a recorded `Event::SignalDelivery` whose `thread` is the **resolved target**.

**What changes, and what deliberately does not:**
- `kill(pid, sig)` is **process-directed**; retrace delivers it to the caller. That is what every
  single-threaded gate already assumes and what M11 measured — do not route it through
  `thread_of_port`, whose operand is a thread port, not a pid.
- `__pthread_kill(port, sig)` is **thread-directed** and now resolves.
- **There are TWO blocked-signal asserts, and only one of them moves. Do not grep for the assert
  and convert both.**
  - `:584-590`, in the RAISE arm, **becomes the pending path**. Its own text says M11 modelled no
    pending set; this task is that pending set.
  - `:152-155`, in the **fault** arm (`Stop::Fault`), **stays exactly as it is.** Its justification
    is different in kind — *"a fault cannot be deferred, POSIX leaves it undefined, and Darwin
    force-delivers"* — and it argues from the architecture rather than from a feature M11 skipped.
    A synchronous fault genuinely cannot go pending: there is no later point at which re-delivering
    it would mean anything. Converting it would turn a correct fail-loud boundary into quiet,
    wrong deferral.
- The caller's own syscall still completes first, in both branches, via
  `complete_syscall_before_delivery(0, false)`. **Read that function before using it here** — it
  exists because the frame's PSTATE comes from `SPSR_EL1` rather than the `reg::CPSR` that
  `set_x0_err_and_return` writes. Confirm it completes the *caller's* syscall and does not assume
  the caller is also the receiver; if it does assume that, split it and say so in your report.

- [ ] **Step 1: Write the failing test**

Replace `sigthread_e2e.rs`'s Task 5 body with the attribution assertion:

```rust
// M16 Task 7: the signal main raised for its CHILD is recorded as delivered to the child.
//
// This asserts on the TRACE, not on the exit code: a guest whose handler never ran at all also
// exits 0, and a guest whose handler ran on the WRONG thread also exits 0. The recorded
// SignalDelivery.thread is the only observable that separates the three.
mod util;
use retrace_trace::Event;

#[test]
fn the_signal_is_delivered_to_the_named_child_thread() {
    let (rec, trace) = util::record_dynamic(retrace_guest::SIGTHREAD);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    let events = retrace_trace::Reader::open(&trace).unwrap();
    let delivered: Vec<u32> = events.iter().filter_map(|e| match e {
        Event::SignalDelivery { sig: 30, thread, .. } => Some(*thread),
        _ => None,
    }).collect();

    assert_eq!(delivered, vec![1u32],
        "exactly one SIGUSR1 delivery, to thread 1 — the child. Thread 0 here means the target \
         port was ignored and main took its own signal, which is the defect M16 closes.");

    // The user-visible half of the same claim, and the half Task 5 asserted inverted. `kill rc 0`
    // must now come BEFORE `handler`: main's pthread_kill returns without running anything, and
    // the child runs the handler only once main blocks in join. This is also, exactly, the order a
    // native run produces (MEASURED 20/20) — so M16 does not merely relabel a trace field, it
    // corrects observable behaviour.
    // Every line, in order — same shape as the Task 5 test this replaces, and for the same reason:
    // a filtered allowlist would silently drop unexpected output, and the length check is what
    // catches it. The pthread line is matched by prefix so an unrelated memory-layout change cannot
    // raise a false alarm here.
    let out = String::from_utf8_lossy(&rec.stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 6, "unexpected extra or missing stdout line:\n{out}");
    assert_eq!(lines[0], "installed");
    assert!(lines[1].starts_with("child pthread 0x"), "line 2 was {:?}", lines[1]);
    assert_eq!(&lines[2..], &["kill rc 0", "handler", "child body", "joined"],
        "post-M16 order: the CHILD takes the signal. Full stdout:\n{out}");

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay must be clean; stderr:\n{}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout, "replay must be byte-identical");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p retrace --test sigthread_e2e -- --test-threads=1 --nocapture`
Expected: FAIL with `delivered == [0]` — main took the child's signal. **That failure is the bug
this milestone exists to close; quote it verbatim in your report.**

- [ ] **Step 3: Rewrite the raise arm**

**First, retire the comment that sits directly above the assert you are replacing.** It currently
reads:

```
    // __pthread_kill's thread-port operand is NOT validated: 328 fires in no gate guest
    // (measured: zero across hello_dyn/hello_rust/jq), so there is no observed port to
    // compare against, and the guest has exactly one thread on one vCPU -- any port it
    // could name is that thread. Ungated rather than wrongly gated; see the Status
    // section. Learn the port from mach_thread_self if a guest ever needs the check.
```

Every clause of it is falsified by this milestone: 328 now fires in a gate guest (SIGTHREAD, Task
5); the guest no longer has exactly one thread, so "any port it could name is that thread" is
false; the operand IS validated now, by Task 4's `thread_of_port`; and the suggested fix, a
`mach_thread_self` handler, was measured unnecessary (spec R1 — main's kport reads `0x103` straight
out of its own pthread struct, the child's `0xbad7001`). Leaving it would ship a comment
recommending a road M16 deliberately did not take, sitting on top of the code that took the other
one. Replace it with what is now true — the operand is validated by `thread_of_port`, which reads
each live thread's own kport out of its pthread struct, covers main without a special case, and is
fail-loud because defaulting to the current thread is the exact latent bug M16 exists to close —
and keep the old measurement (zero 328s across hello_dyn/hello_rust/jq) as the history of *why* it
was ungated before.

Then, in `record_box`, replace the body of the `SYS_KILL || SYS_PTHREAD_KILL` arm after the existing
`self_pid` safety check (which is unchanged):

```rust
                // M16: __pthread_kill names a TARGET THREAD; kill names the process. A
                // process-directed signal may go to any thread with it unblocked, and retrace picks
                // the caller — which is what every pre-M16 gate already assumes.
                let target = if num == retrace_arch::SYS_PTHREAD_KILL {
                    b.thread_of_port(args[0] as u32)
                } else {
                    b.threads().current()
                };
                let sig = args[1];
                let act = b.sigtable().action(sig);
                let thread = b.threads().current() as u32;

                if b.threads().is_blocked_for(target, sig) {
                    // M16 replaces M11's assert. The signal goes PENDING on the target and is
                    // materialised at the next unmask — a syscall landmark, which is what keeps
                    // delivery visible to both dispatch loops.
                    w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![], thread })
                        .map_err(|e| format!("append pended raise: {e}"))?; count += 1;
                    b.threads_mut().pend(target, sig);
                    b.set_x0_err_and_return(0, false);
                } else {
                    match act.disp {
                        retrace_box::Disposition::Handler(handler) => {
                            w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![], thread })
                                .map_err(|e| format!("append caught raise: {e}"))?; count += 1;
                            // The CALLER's syscall completes first, whether or not it is also the
                            // receiver. See spikes/sigraisex0.c and M12's note: the frame's PSTATE
                            // comes from SPSR_EL1, not from the reg::CPSR set_x0_err_and_return writes.
                            b.complete_syscall_before_delivery(0, false);
                            let (writes, resume_pc) =
                                b.deliver_signal_to(target, sig, retrace_arch::SI_USER, 0, 0, 0);
                            w.append(&Event::SignalDelivery {
                                sig, si_code: retrace_arch::SI_USER, si_addr: 0, handler, resume_pc,
                                writes, thread: target as u32 })
                                .map_err(|e| format!("append signal delivery: {e}"))?; count += 1;
                        }
                        retrace_box::Disposition::Ign => { /* unchanged */ }
                        retrace_box::Disposition::Dfl => { /* unchanged, except Signal carries
                                                              thread: b.threads().current() as u32 */ }
                    }
                }
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p retrace --test sigthread_e2e -- --test-threads=1 --nocapture`
Expected: the record half PASSES (`delivered == [1]`). **The replay half will still fail** — the
mirror is Task 8. If the replay failure is anything other than a divergence at the delivery landmark,
report it; that would mean the record side changed something the mirror was not the only consumer of.

- [ ] **Step 5: Run the pre-M16 signal gates for regression**

Run: `cargo test -p retrace --test panic_e2e --test crashy_e2e -- --test-threads=1`
Expected: PASS. These raise on a single-threaded guest, where `target == current` and the arm must
behave exactly as before.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace/tests/sigthread_e2e.rs
git commit -m "M16 t7: __pthread_kill delivers to the thread it names"
```

---

### Task 8: The replay mirror

**Files:**
- Modify: `crates/retrace-core/src/lib.rs:984-1100` (the raise mirror in `ReplaySession::advance`)
- Test: `crates/retrace/tests/sigthread_e2e.rs` (the replay half of Task 7's test turns green)

**Interfaces:**
- Consumes: everything Task 7 produced.

**`ReplaySession::mirror_delivery` (~`:890`) is a SHARED helper — parameterise it, do not duplicate
it.** It owns the `deliver_signal` call (`:895`) *and* the frame byte-compare that IS the divergence
check, and it already has **two** callers: `:1050`, the caught-raise mirror (`SI_USER`), and `:1510`,
the fault mirror. Task 9 adds a third. Writing a second raise-specific copy of it would reproduce
the defect M13 Task 8 shipped — a mirrored pair where only one half is checked — and is exactly what
Task 6's own design note prevents on the record side.

Give it a `tid: usize` parameter. Each **call site** recomputes its own target: the raise mirror via
`thread_of_port`, the fault mirror as the current thread, Task 9's as the calling thread. The one
helper then does both the frame byte-compare and the recorded-`thread` comparison. Note its match
arm is `Some(Event::SignalDelivery { sig: rsig, writes, .. })` — that `..` is why Task 2 never had
to touch it, and it is where you bind `thread` to compare.

**Symmetry rule 1 in its ordinary form:** the mirror calls the **same** `Box_` methods with the
**same** arguments — `thread_of_port`, then `deliver_signal_to` — recomputes the frame bytes, and
byte-compares them against the recording. That comparison *is* the divergence check; an asymmetry
surfaces as a divergence rather than as silent corruption. Do not consume the recorded `thread`;
recompute it and compare, which is the standard symmetric posture and the one M15 kept.

- [ ] **Step 1: Run the failing test**

Run: `cargo test -p retrace --test sigthread_e2e -- --test-threads=1 --nocapture`
Expected: the replay half FAILS. Record the exact divergence message.

- [ ] **Step 2: Mirror the record arm**

In `ReplaySession::advance`'s `SYS_KILL || SYS_PTHREAD_KILL` block, recompute the target and the
mask decision exactly as record did, then branch identically:

```rust
                    if num == retrace_arch::SYS_KILL || num == retrace_arch::SYS_PTHREAD_KILL {
                        let target = if num == retrace_arch::SYS_PTHREAD_KILL {
                            self.b.thread_of_port(args[0] as u32)
                        } else {
                            self.b.threads().current()
                        };
                        let sig = args[1];
                        let act = self.b.sigtable().action(sig);
                        let pended = self.b.threads().is_blocked_for(target, sig);
                        // A pended raise produces ONE landmark (the Syscall) and no delivery, so it
                        // falls through to the generic arm, which already verifies (num, args) and
                        // the thread tag. Only the mask side-effect happens here.
                        if pended {
                            self.b.threads_mut().pend(target, sig);
                        } else if let retrace_box::Disposition::Handler(_) = act.disp {
                            // ... existing caught-raise mirror, with deliver_signal_to(target, ...)
                            // and the recorded SignalDelivery's `thread` compared against
                            // `target as u32` AFTER the frame byte-compare.
                        }
                        // ... the terminal Dfl arm, unchanged except Signal's thread compare
                    }
```

Add the thread comparison to each mirrored landmark **after** that site's existing field check, so a
genuine frame or `(num, args)` divergence still reports as itself rather than being masked by the
thread mismatch it caused:

```rust
                                if *rthread != target as u32 {
                                    return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "signal delivery thread mismatch: live {target}, recorded \
                                         {rthread} — the signal was delivered to a different thread \
                                         than the recording did") });
                                }
```

- [ ] **Step 3: Run**

Run: `cargo test -p retrace --test sigthread_e2e -- --test-threads=1 --nocapture`
Expected: PASS, both halves.

- [ ] **Step 4: Prove the mirror is load-bearing**

Temporarily change the mirror's `deliver_signal_to(target, …)` to `deliver_signal_to(0, …)`.
Expected: `sigthread_e2e` FAILS with a frame byte-compare divergence — proving replay recomputes
rather than consuming. Revert and record the message.

- [ ] **Step 5: Run every signal and thread gate**

Run: `cargo test -p retrace --test panic_e2e --test segv_rust_e2e --test protnone_rust_e2e \
      --test thread_rust_e2e --test thread_watch_e2e --test crashy_e2e -- --test-threads=1`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-core/src/lib.rs
git commit -m "M16 t8: replay recomputes the target and byte-compares the frame"
```

---

### Task 9: A masked signal pends, and materialises when the mask lifts

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` — the `SYS_SIGPROCMASK`/`SYS_PTHREAD_SIGMASK` arm in
  `record_box` (~`:518`) and its replay mirror in `ReplaySession::advance` (~`:1126`); the
  `SYS_SIGPENDING` arm (~`:533`)
- Modify: `crates/retrace-guest/rs/sigthread.rs` (append the mask/pending half)
- Test: `crates/retrace/tests/sigthread_e2e.rs`

**Interfaces:**
- Consumes: Task 3's `ThreadTable::{set_mask_of, pending_of, take_deliverable}`, Task 6's
  `deliver_signal_to`.

**The anchor, restated because it is the design's load-bearing choice:** a pending signal is
materialised at the `sigprocmask`/`pthread_sigmask` landmark that unblocks it, **never** at the
scheduler's switch point. Materialising at a switch would mean producing a `SignalDelivery` — a trace
event — from inside `Box_::run()`, below the trace, which is exactly the argument M15 used to
**delete** `Event::Sched` rather than reserve it.

**The limit this accepts, which belongs in the close:** a signal left pending on a thread that never
touches its mask again is delivered **never**, where a real kernel would deliver it at the next
opportunity.

- [ ] **Step 1: Extend the guest**

Append to `sigthread.rs`'s `main`, after `h.join().unwrap()` and the `joined` line:

```rust
    // The mask/pending half (Task 9). Main blocks SIGUSR1 for ITSELF, raises it on itself so it
    // must pend, observes sigpending reporting it, then unblocks — which is the landmark the
    // delivery is anchored to.
    let mut set: u32 = 1 << (SIGUSR1 - 1);
    let mut old: u32 = 0;
    unsafe { pthread_sigmask(SIG_BLOCK, &set as *const u32, &mut old as *mut u32) };
    let rc2 = unsafe { pthread_kill(pthread_self(), SIGUSR1) };
    println!("self kill rc {rc2}");

    let mut pend: u32 = 0;
    unsafe { sigpending(&mut pend as *mut u32) };
    println!("pending {}", (pend >> (SIGUSR1 - 1)) & 1);

    unsafe { pthread_sigmask(SIG_UNBLOCK, &set as *const u32, &mut old as *mut u32) };
    println!("unblocked");
    let _ = &mut set;
```

extending the `extern "C"` block:

```rust
unsafe extern "C" {
    fn pthread_sigmask(how: i32, set: *const u32, old: *mut u32) -> i32;
    fn sigpending(set: *mut u32) -> i32;
    fn pthread_self() -> u64;
}
const SIG_BLOCK: i32 = 1;
const SIG_UNBLOCK: i32 = 2;
```

**`sigset_t` on macOS is a 32-bit `__darwin_sigset_t`**, which is why `u32` is the right shape here.
If the guest miscompiles or the values look wrong, verify the width against `<sys/_types/_sigset_t.h>`
on this host and report what you found rather than widening on a guess.

- [ ] **Step 2: Write the failing test**

Add to `sigthread_e2e.rs`:

```rust
/// M16 Task 9: a masked signal pends and is delivered at the unmask, not before and not never.
///
/// Two deliveries in this trace, and the pair is the assertion: the child's at the pthread_kill
/// landmark, main's at the pthread_sigmask landmark. One delivery means the pending half was
/// dropped; three means it was delivered twice.
#[test]
fn a_masked_signal_pends_and_is_delivered_when_the_mask_lifts() {
    let (rec, trace) = util::record_dynamic(retrace_guest::SIGTHREAD);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout);

    assert!(out.contains("pending 1"),
        "sigpending must report the signal main raised on itself while masked — an always-empty \
         answer is the lie M11 flagged and M16 fixes. stdout:\n{out}");

    let events = retrace_trace::Reader::open(&trace).unwrap();
    let delivered: Vec<u32> = events.iter().filter_map(|e| match e {
        Event::SignalDelivery { sig: 30, thread, .. } => Some(*thread),
        _ => None,
    }).collect();
    assert_eq!(delivered, vec![1u32, 0u32],
        "the child's delivery first, then main's pending one at the unmask landmark");

    // The pending delivery is anchored to the mask call, not to some later point: the landmark
    // immediately before it is the pthread_sigmask that unblocked it.
    let di = events.iter().rposition(|e| matches!(e, Event::SignalDelivery { .. })).unwrap();
    match &events[di - 1] {
        Event::Syscall { num, .. } => assert!(
            *num == retrace_arch::SYS_SIGPROCMASK || *num == retrace_arch::SYS_PTHREAD_SIGMASK,
            "the pending delivery must sit immediately after the unmasking syscall, got num {num}"),
        other => panic!("expected the unmasking Syscall landmark before the delivery, got {other:?}"),
    }

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay must be clean; stderr:\n{}", rep.stderr);
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p retrace --test sigthread_e2e -- --test-threads=1 --nocapture`
Expected: FAIL — `pending 0` in stdout and `delivered == [1]`.

- [ ] **Step 4: Implement the mask arm and the truthful `sigpending`**

In `record_box`'s mask arm: apply to the **calling** thread via `set_mask_of(current, how, set)`,
write back the old mask, append the `Syscall` landmark, and *then*:

```rust
                // M16: the unmask is the anchor. Materialise at most one pending signal here — the
                // lowest-numbered one the new mask no longer blocks. `take_deliverable` clears the
                // bit as it takes it, so a signal cannot be delivered twice.
                if let Some(psig) = b.threads_mut().take_deliverable(cur) {
                    if let retrace_box::Disposition::Handler(handler) = b.sigtable().action(psig).disp {
                        let (writes, resume_pc) =
                            b.deliver_signal_to(cur, psig, retrace_arch::SI_USER, 0, 0, 0);
                        w.append(&Event::SignalDelivery {
                            sig: psig, si_code: retrace_arch::SI_USER, si_addr: 0, handler,
                            resume_pc, writes, thread: cur as u32 })
                            .map_err(|e| format!("append pending delivery: {e}"))?; count += 1;
                    }
                }
```

`SYS_SIGPENDING` (~`:533`) stops returning a constant and writes `threads().pending_of(current)`.

Mirror all of it in `ReplaySession::advance`'s mask arm, recomputing and byte-comparing — through
`mirror_delivery`, which Task 8 gave a `tid` parameter. This is its **third** call site; pass the
calling thread. Do not open a fourth copy of the recompute-and-compare logic.

- [ ] **Step 5: Run**

Run: `cargo test -p retrace --test sigthread_e2e -- --test-threads=1 --nocapture`
Expected: PASS, all three tests in the file.

- [ ] **Step 6: Prove non-vacuity, twice**

1. Temporarily make `take_deliverable` always return `None`. Expected:
   `a_masked_signal_pends_and_is_delivered_when_the_mask_lifts` FAILS on `delivered == [1]`; the
   Task 7 test stays GREEN. Revert.
2. Temporarily make `spawn` seed `mask: 0` instead of inheriting. Expected: report which tests fail.
   The Task 3 unit test must; note honestly whether any e2e does, because the guest masks *after*
   spawning and so may not distinguish it.

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace-guest/rs/sigthread.rs \
        crates/retrace/tests/sigthread_e2e.rs
git commit -m "M16 t9: a masked signal waits for the mask to lift, and sigpending stops lying"
```

---

### Task 10: The mask is per-thread, and the guest's ordering proves it

Tasks 3 and 9 built per-thread masks and tested them at the unit level. Nothing yet proves the
property **end to end**, because through Task 9 the guest masks only *after* it has already signalled
the child. This task reorders the guest so main is masked at the moment it signals the child — and
the child, which inherited an empty mask, must still take the signal.

**Files:**
- Modify: `crates/retrace-guest/rs/sigthread.rs` (move the mask block above the `pthread_kill`)
- Test: `crates/retrace/tests/sigthread_e2e.rs`

**Why the ordering is the proof:** were the mask still process-global, main's block of SIGUSR1 would
suppress the *child's* delivery — the raise would take the pending path instead and the child's
handler would never run. So a single reordering turns "the mask is per-thread" from an assertion that
restates the implementation into an observable the guest itself produces.

- [ ] **Step 1: Reorder the guest**

`main` becomes, in order: install handler → **spawn the child** (so it inherits an empty mask) →
`pthread_sigmask(SIG_BLOCK, SIGUSR1)` on main → `pthread_kill(child, SIGUSR1)` → `join` →
`pthread_kill(self, SIGUSR1)` (pends) → `sigpending` → `pthread_sigmask(SIG_UNBLOCK, SIGUSR1)`.

The mask step must `println!("masked")` immediately after the `pthread_sigmask` returns — the test
below reads stdout for `"masked"` appearing before `"kill rc"`, which is how it proves main really
was masked at the moment it signalled rather than trusting the source order.

The spawn must stay **before** the mask: a child spawned after would inherit the block, and the test
below would then be measuring inheritance rather than independence.

- [ ] **Step 2: Write the failing test**

```rust
/// M16 Task 10: main is masked when it signals the child, and the child takes the signal anyway.
///
/// The single fact that separates a per-thread mask from a process-global one, expressed as
/// something the guest does rather than something a unit test asserts about a struct.
#[test]
fn main_masking_a_signal_does_not_block_it_for_the_child() {
    let (rec, trace) = util::record_dynamic(retrace_guest::SIGTHREAD);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout);

    // Ordering in stdout is the ground truth that main really was masked first.
    let masked_at = out.find("masked").expect("guest must announce its own mask");
    let killed_at = out.find("kill rc").expect("guest must announce the child kill");
    assert!(masked_at < killed_at,
        "main must already be masked when it signals the child, or this test measures nothing:\n{out}");

    let events = retrace_trace::Reader::open(&trace).unwrap();
    let first = events.iter().find_map(|e| match e {
        Event::SignalDelivery { sig: 30, thread, .. } => Some(*thread),
        _ => None,
    }).expect("the child's delivery must exist");
    assert_eq!(first, 1,
        "the child inherited an EMPTY mask and must take the signal even though main has it \
         blocked. A process-global mask would pend this instead and deliver nothing here.");

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay must be clean; stderr:\n{}", rep.stderr);
}
```

- [ ] **Step 3: Run**

Run: `cargo test -p retrace --test sigthread_e2e -- --test-threads=1 --nocapture`
Expected: PASS, all four tests in the file.

- [ ] **Step 4: Prove it non-vacuous**

Temporarily change `ThreadTable::is_blocked_for` to ignore its `tid` and always consult thread 0 —
a faithful re-creation of the process-global behaviour.
Expected: `main_masking_a_signal_does_not_block_it_for_the_child` FAILS. Record which *other* tests
fail alongside it; if the Task 7 test also fails, say so, because that tells the reader the two are
not independent checks. Revert.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-guest/rs/sigthread.rs crates/retrace/tests/sigthread_e2e.rs
git commit -m "M16 t10: main's mask is main's alone"
```

---

### Task 11: The oracle checks the three remaining landmark tags

`SignalDelivery`'s comparison landed in Task 8, beside the frame byte-compare it belongs with. This
task adds the other three.

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` (`ReplaySession::advance` — the `Exit`, `Crash` and
  terminal-`Signal` verify sites)
- Test: `crates/retrace/tests/thread_oracle.rs`

**Interfaces:**
- Consumes: `ReplaySession::verify_thread(rthread: u32, pc: u64) -> Result<(), Divergence>`, which
  M15 already provides.

**Carried in from Task 2's review (a deferred Minor — fix it while you are here).** The `Event::Exit`
doc in `crates/retrace-trace/src/lib.rs` justifies its `thread` tag's permanence with *"a threaded
guest still has exactly one thread call exit"*. That reason is wrong: nothing stops two guest threads
racing to call `exit`. The real invariant is in `record_box` at `crates/retrace-core/src/lib.rs:131-136`
— the `SYS_EXIT` arm **`break`s** after appending the first `Event::Exit`, so at most one can ever
exist in a trace and its tag is unambiguous because the RECORDING stops. The conclusion (permanent,
not a placeholder) is correct; replace only the parenthetical reason with the real one.

**Placement rule, inherited from M15 Task 4's fix round:** each call goes **after** that site's own
field comparison, never hoisted above it, so a genuine `(code)`/`(pc, esr, far)`/`(sig, pc)`
divergence is still reported as itself instead of being masked by the thread mismatch it caused.

- [ ] **Step 1: Write the failing test**

```rust
/// M16 Task 11. The three terminal-ish landmarks gained a thread tag; retagging any of them to a
/// genuinely live thread id must diverge. `Exit` is the one every guest reaches, so it is the one
/// that can be tested against a real threaded recording without a bespoke fixture.
#[test]
fn a_wrong_thread_on_the_exit_landmark_is_a_divergence() {
    let (rec, trace) = util::record_dynamic(retrace_guest::THREADRUST);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    let mut events = retrace_trace::Reader::open(&trace).unwrap().iter().cloned().collect::<Vec<_>>();
    let ids: Vec<u32> = { let mut v: Vec<u32> = events.iter().filter_map(|e| match e {
        Event::Syscall { thread, .. } => Some(*thread), _ => None }).collect();
        v.sort_unstable(); v.dedup(); v };
    assert!(ids.len() >= 2, "need a genuinely threaded recording; got {ids:?}");

    let i = events.iter().position(|e| matches!(e, Event::Exit { .. }))
        .expect("a clean run ends in Exit");
    let orig = match &events[i] { Event::Exit { thread, .. } => *thread, _ => unreachable!() };
    let other = *ids.iter().find(|&&t| t != orig).expect("a second live id exists");
    if let Event::Exit { thread, .. } = &mut events[i] { *thread = other; }

    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 3, "CLI exit 3 is the Divergence convention; stderr:\n{}", rep.stderr);
    assert!(rep.stderr.contains("thread"), "the divergence must name the thread mismatch:\n{}", rep.stderr);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p retrace --test thread_oracle -- --test-threads=1`
Expected: FAIL — replay exits 0, because nothing compares `Exit`'s thread.

- [ ] **Step 3: Add the three calls**

At the `Exit` verify site, after the existing exit-code comparison and **before** the final-memory
`Snapshot` check:

```rust
                                self.verify_thread(*thread, pc)?;
```

Do the same at the `Crash` site (after `(pc, esr, far)`) and the terminal `Signal` site (after
`(sig, pc)`).

- [ ] **Step 4: Run**

Run: `cargo test -p retrace --test thread_oracle -- --test-threads=1`
Then: `cargo test -p retrace --test crashy_e2e --test panic_e2e -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace/tests/thread_oracle.rs
git commit -m "M16 t11: the oracle checks Exit, Crash and Signal's thread too"
```

---

### Task 12: Discharge M15's standing caveat

M15 shipped the oracle's caught-raise and `sigreturn` mirrors proven to **fire**, not proven to
**distinguish**, because the only fixture that reached them (`SIGFRAME`) is single-threaded and had
no second live thread id to retag to. `SIGTHREAD` is that fixture.

**Files:**
- Test: `crates/retrace/tests/thread_oracle.rs`

**Interfaces:** consumes `retrace_guest::SIGTHREAD` and the mirrors' `verify_thread` calls, which
M15 already installed.

**What "distinguish" requires, and why the retag target matters:** the mutation must set the tag to
an id that **genuinely appears elsewhere in this same trace**. An out-of-range constant would test
something weaker — that a bogus id is rejected — rather than the real property, that a wrong but
genuinely live thread is caught. This is the reasoning `a_wrong_thread_on_replay_is_a_divergence`
already documents; reuse it, do not re-derive it.

- [ ] **Step 1: Write both failing tests**

```rust
/// M16 Task 12a. The CAUGHT-RAISE mirror, reached with two live threads for the first time.
///
/// M15 could only prove this arm FIRES: SIGFRAME is single-threaded, so there was no other live id
/// to retag to and the test had to use a bogus constant. SIGTHREAD's pthread_kill lands here with a
/// real second thread in the table, so the retag below is to a thread the guest actually scheduled.
#[test]
fn a_wrong_thread_at_the_caught_raise_mirror_is_a_divergence() {
    retag_and_expect_divergence(retrace_arch::SYS_PTHREAD_KILL);
}

/// M16 Task 12b. The SIGRETURN mirror. Stronger than 12a: the thread current at this landmark is
/// the CHILD, so the recorded tag here is a nonzero id — a value this arm has never seen.
#[test]
fn a_wrong_thread_at_the_sigreturn_mirror_is_a_divergence() {
    retag_and_expect_divergence(retrace_arch::SYS_SIGRETURN);
}

/// Retag the first `Syscall` landmark for `num` to some OTHER live thread id from this same trace
/// and assert replay refuses it.
fn retag_and_expect_divergence(num_wanted: u64) {
    let (rec, trace) = util::record_dynamic(retrace_guest::SIGTHREAD);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    let mut events = retrace_trace::Reader::open(&trace).unwrap().iter().cloned().collect::<Vec<_>>();
    let mut ids: Vec<u32> = events.iter().filter_map(|e| match e {
        Event::Syscall { thread, .. } => Some(*thread), _ => None }).collect();
    ids.sort_unstable(); ids.dedup();
    assert!(ids.len() >= 2,
        "SIGTHREAD must schedule at least two threads that issue syscalls, or this mutation is the \
         bogus-constant one M15 was stuck with; got {ids:?}");

    let i = events.iter().position(|e| matches!(e, Event::Syscall { num, .. } if *num == num_wanted))
        .unwrap_or_else(|| panic!("no Syscall landmark for num {num_wanted} — this fixture no \
                                   longer reaches the mirror this test exists to cover"));
    let orig = match &events[i] { Event::Syscall { thread, .. } => *thread, _ => unreachable!() };
    let other = *ids.iter().find(|&&t| t != orig)
        .expect("a genuinely live second id, not a constant");
    if let Event::Syscall { thread, .. } = &mut events[i] { *thread = other; }

    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 3, "CLI exit 3 is the Divergence convention; stderr:\n{}", rep.stderr);
}
```

- [ ] **Step 2: Run**

Run: `cargo test -p retrace --test thread_oracle -- --test-threads=1 --nocapture`
Expected: PASS. If either fails because the `position` lookup found nothing, the guest does not
reach that mirror — report which one and what the trace actually contains, rather than weakening the
test to whatever it does reach.

- [ ] **Step 3: Prove each mirror independently**

M15's Task 4 lesson: mutating one call site cannot demonstrate that a bug in another would be caught.

1. Delete the `verify_thread` call in the **caught-raise** mirror only. Expected: 12a FAILS, 12b
   stays GREEN. Restore.
2. Delete the `verify_thread` call in the **`sigreturn`** mirror only. Expected: 12b FAILS, 12a
   stays GREEN. Restore.

Record both transcripts. If deleting one fails both tests, the two are piggybacking on a single
check and the finding is more important than the tests — report it before proceeding.

- [ ] **Step 4: Commit**

```bash
git add crates/retrace/tests/thread_oracle.rs
git commit -m "M16 t12: the two signal-path oracle arms now distinguish two live schedules"
```

---

### Task 13: Park the blocked-target gate honestly

**Files:**
- Create: `crates/retrace-guest/rs/sigblocked.rs`
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs`
- Create: `crates/retrace/tests/sigblocked_e2e.rs`

**The discipline:** a milestone that parks a *new* gate for a capability it does not yet have has
regressed nothing. The guest is real code that compiles and can be un-`#[ignore]`d the day the wall
falls — `overflow.rs` and `stackoverflow_rust_e2e` are the precedent to copy.

- [ ] **Step 1: Write the guest**

**The obvious two-thread shape does not work, and the reason is worth understanding before you
write this.** M14's scheduler switches **only** when a thread blocks or exits. So if main spawns `a`
and immediately signals it, `a` is Runnable-and-never-run — not Blocked — and the gate would be
parked against a state its own guest never enters. Generalised: for any thread to be Blocked, some
other thread must have blocked first to let it run, so **main can never observe a blocked peer.**

A third thread breaks the deadlock, because a blocked joiner leaves its own joinee running:

```
main(0) spawns a(1); main joins a          -> main Blocked
  a runs, spawns b(2), joins b             -> a    Blocked(Join{2})
    b runs, and b is CURRENT while a is genuinely Blocked
    b signals a
```

```rust
// crates/retrace-guest/rs/sigblocked.rs
//
// The guest for the PARKED sigblocked_e2e gate: a signal whose target is BLOCKED in __ulock_wait,
// not merely not-current.
//
// Three threads, not two, and that is forced rather than incidental. The cooperative scheduler
// switches only on block or exit, so main can never be running while a peer is blocked — for the
// peer to have blocked, main must have blocked first. A blocked JOINER, though, leaves its joinee
// running: main joins a, a joins b, so b runs while a sits in __ulock_wait. b is the only thread
// that can express this signal.
//
// Built so the parked test is real code that compiles and can be un-ignored the day the wall falls.
use std::sync::atomic::{AtomicU64, Ordering};

unsafe extern "C" {
    fn pthread_kill(thread: u64, sig: i32) -> i32;
    fn signal(sig: i32, h: usize) -> usize;
    fn pthread_self() -> u64;
}
const SIGUSR1: i32 = 30;
static A_PT: AtomicU64 = AtomicU64::new(0);
extern "C" fn on_usr1(_sig: i32) {}

fn main() {
    unsafe { signal(SIGUSR1, on_usr1 as usize) };
    let a = std::thread::spawn(|| {
        // Publish a's own pthread_t BEFORE blocking, so b has something to name.
        A_PT.store(unsafe { pthread_self() }, Ordering::SeqCst);
        let b = std::thread::spawn(|| {
            // a is Blocked(Join) right now: b was scheduled precisely because a blocked.
            let at = A_PT.load(Ordering::SeqCst);
            unsafe { pthread_kill(at, SIGUSR1) };
            println!("b signalled a");
        });
        b.join().unwrap();
        println!("a resumed");
    });
    a.join().unwrap();
    println!("done");
}
```

Wire it into `build.rs` and `src/lib.rs` as `SIGBLOCKED`, copying the `sigthread` recipe verbatim.

- [ ] **Step 2: Write the parked gate**

```rust
// crates/retrace/tests/sigblocked_e2e.rs
mod util;

#[test]
#[ignore = "M16 wall: signalling a thread that is BLOCKED in __ulock_wait is unmodelled. The \
            target's saved ctx has elr already past the svc and the wake path (unblock_waiters_on) \
            still owes it a return value, so handler-then-sigreturn must interleave with the \
            unblock rather than simply redirect a never-run entry context — which is the only case \
            M16 delivers into. A real kernel interrupts the wait (EINTR or restart); retrace has no \
            mechanism for either. MEASURE the actual failure with --ignored before designing the \
            fix; do not assume it panics where you expect. UN-IGNORE when a blocked target is \
            modelled."]
fn a_signal_reaches_a_thread_blocked_in_ulock_wait() {
    let (rec, trace) = util::record_dynamic(retrace_guest::SIGBLOCKED);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay must be clean; stderr:\n{}", rep.stderr);
}
```

- [ ] **Step 3: Run it forced, and record what actually happens**

Run: `cargo test -p retrace --test sigblocked_e2e -- --test-threads=1 --ignored --nocapture`
Expected: FAIL. **Copy the exact failure into the `#[ignore]` reason**, replacing the "MEASURE"
sentence with what you measured — M13's `stackoverflow_rust_e2e` documents its wall with the literal
error text, and that is what makes a parked gate honest rather than a guess.

- [ ] **Step 4: Confirm it is skipped by default**

Run: `cargo test -p retrace --test sigblocked_e2e -- --test-threads=1`
Expected: `0 passed; 0 failed; 1 ignored`.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-guest/rs/sigblocked.rs crates/retrace-guest/build.rs \
        crates/retrace-guest/src/lib.rs crates/retrace/tests/sigblocked_e2e.rs
git commit -m "M16 t13: park the blocked-target gate at its measured wall"
```

---

### Task 14: The honest close

**Files:**
- Modify: `README.md` (a new `## Status: M16-threadsignal` section at the end)
- Modify: `CLAUDE.md` (the headline-gate list, and the "Guest threads" section)

**The README Status section is the single authoritative record.** Write it from what was measured,
not from what this plan predicted — and where the two disagree, the disagreement is the most useful
paragraph in the section.

- [ ] **Step 1: Run the full gate in chunks**

Every chunk gets `--no-fail-fast`, and cargo's exit code is captured **before** any pipe:

```bash
cd /Users/noahmitchem/Documents/GitHub/retrace
for chunk in "-p retrace-arch -p retrace-trace -p retrace-sim -p hv-sys" \
             "-p retrace-box -p retrace-guest" \
             "-p retrace-core -p retrace"; do
  cargo test $chunk --no-fail-fast -- --test-threads=1 > /tmp/m16-gate-$$.log 2>&1
  echo "CARGO_EXIT=$? for [$chunk]"
  grep -a "test result" /tmp/m16-gate-$$.log | tail -40
done
cargo clippy --workspace --all-targets -- -D warnings; echo "CLIPPY_EXIT=$?"
```

Record the totals. **M15 closed at 360 passed / 0 failed / 1 ignored over 98 binaries** — the delta
against that is the number that means something, and it must reconcile against the per-task counts
rather than being waved through.

- [ ] **Step 2: Verify every headline gate ran by name**

Run: `grep -a "hello_dyn\|hello_rust\|jq_\|panic_e2e\|thread_watch\|sigthread" /tmp/m16-gate-*.log`
Expected: each present and passing. If `jq` skipped, say so loudly — `/opt/homebrew/bin/jq` is not a
repo artifact and a silent skip reads as a green it did not earn.

- [ ] **Step 3: Write the README Status section**

It must cover, each in its own right:
- what runs today that did not before, in one sentence a reader can check
- the measured gate totals and the delta against M15's 360/0/1 over 98
- **`TRACE_MAGIC` is now `RT\x00\x08`; every pre-M16 recording is unreadable** — say it plainly, and
  say it is the second break in two milestones and why that was accepted
- the R1 answer: what main's kport actually read back as, and whether the fallback was needed
- **the limits, named rather than discovered later:** a signal left pending on a thread that never
  touches its mask again is never delivered; handler-before-body differs from a native run (R3);
  signalling a blocked target is parked; queueing and nested delivery are unmodelled;
  `sigwait`/`sigsuspend` still panic
- **what contradicted this plan.** M15's close recorded six plan defects that only measurement found.
  If this plan survived unamended, say so — but check first, because "unamended" is more often
  unexamined than perfect
- the one new `#[ignore]`, and confirmation that `stackoverflow_rust_e2e` is unchanged

- [ ] **Step 4: Update `CLAUDE.md`**

- the headline-gate list gains `sigthread_e2e` (a guest whose main signals its child by name)
- the "Guest threads" section gains a sentence: the signal path resolves `__pthread_kill`'s target
  port to a thread and delivers into that thread's saved context; masks, pending sets and alternate
  stacks are per-thread while dispositions stay process-global
- the M15 sentence about the oracle's two untested signal-path arms is now **false** — rewrite it,
  do not leave a stale caveat next to the work that discharged it

- [ ] **Step 5: Commit**

```bash
git add README.md CLAUDE.md
git commit -m "M16-threadsignal: the honest close"
```

**Carried in from the Task 3 review (finding 3), with the exact location:** `README.md:1675`, in the
**M11** Status section, reads:

> **Disposition, not delivery.** `SigTable` (`crates/retrace-box/src/sig.rs`) holds per-signal
> disposition, the blocked mask, and the alt stack.

Two of those three moved to `Thread` in `crates/retrace-box/src/thread.rs` in M16 Task 3. The
sentence is present-tense and names a file, so a reader following it lands on code that does not
match it. A Status section is a historical log and must not be rewritten to pretend M11 did
something it did not — correct it the way this project already corrects superseded claims: leave
M11's account of what M11 built, and say plainly that M16 later moved the mask and the alternate
stack to the thread table, pointing at the M16 Status section. Grep the README for other
`SigTable`-holds-the-mask claims before you edit; this is the one the reviewer found, not
necessarily the only one.
