# M15-threaddebug Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make retrace's debugger able to name, inspect, and attribute the guest threads M14 taught it to record.

**Architecture:** The running thread is a *derived* property of position `(N, K)`, not a new coordinate — a switch happens only at a clean stop boundary between windows. So M15 mostly **exposes a fact `Box_` already computes** (`ReplaySession::current_thread()`), adds one recorded field so the divergence oracle can check it (`Event::Syscall.thread`, `TRACE_MAGIC` 0x0006 → 0x0007), and threads that identity through the watchpoint paths and the debug CLI.

**Tech Stack:** Rust 1.95.0, `aarch64-apple-darwin`, Hypervisor.framework, macOS 26.x on Apple Silicon.

**Spec:** `docs/superpowers/specs/2026-08-15-retrace-m15-threaddebug-design.md`

## Global Constraints

- **`--test-threads=1` is mandatory** on every cargo test invocation. HVF allows one VM per process.
- **`cargo clippy --workspace --all-targets -- -D warnings` must stay clean.** The denials are load-bearing, not style.
- **Do NOT run a bare `cargo test --workspace`.** It has been killed on this machine repeatedly. Chunk it:
  `cargo test --workspace --exclude retrace-box --exclude retrace`, then `cargo test -p retrace-box`, then per-target for `-p retrace` plus `cargo test -p retrace --bins`. **`cargo test -p retrace --lib` is INVALID** — `retrace` has no lib target and it fails the whole invocation.
- **Tallying gate logs:** ANSI colour codes break `grep '^test result'` and make awk die with "multibyte conversion failure". Strip first:
  `perl -pe 's/\e\[[0-9;]*[mGKH]//g'`.
- **Full `just gate` is the CONTROLLER's step, not the implementer's.** Implementers run only their own targeted tests. Run the gate ONCE, LAST, after the task's review and fix round have landed.
- **Symmetry rule 1:** a special case in record's `match stop` needs a mirror in replay's dispatch. Both arms live in `crates/retrace-core/src/lib.rs` — record in `record_box`, replay in `ReplaySession::advance` — and must call the *same* `Box_` method with the *same* arguments.
- **Symmetry rule 2:** deterministic emulation belongs *below* the trace, inside `Box_::run()`, shared by record and replay.
- **Never reimplement Apple's PAC.** Not touched by this milestone; do not go near it.
- **Every test must be mutation-checked against the defect it names.** M14 shipped **six** tests that could not fail for the property in their own name. Budget for this; it is this project's most reliable failure mode.
- **A finding parked on a later task's fix round has no owner.** M14 dropped one that way. If you find something, either fix it in your task or state loudly that it is unowned.

---

## File Structure

| File | Responsibility in M15 |
|---|---|
| `crates/retrace-core/src/lib.rs` | `current_thread()`, thread summaries, the oracle's thread compare, thread on `Advance::Watch*`, re-export of the thread types |
| `crates/retrace-box/src/lib.rs` | `dbg_regs_of(tid)`, the R1 invariant assertion |
| `crates/retrace-box/src/thread.rs` | `ThreadState` display/summary helper if needed |
| `crates/retrace-trace/src/lib.rs` | `Event::Syscall { thread }`, `TRACE_MAGIC` bump, magic tests |
| `crates/retrace/src/debug.rs` | `threads` / `regs <thread>` / labelled `where` / watch provenance output |
| `crates/retrace-guest/rs/watchthread.rs` + `build.rs` | the headline guest |
| `crates/retrace/tests/thread_watch_e2e.rs` | the headline gate |

---

### Task 1: `current_thread()`, and the invariant it rests on

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` (add method near `landmark()`, `:1514`)
- Modify: `crates/retrace-box/src/lib.rs` (R1 assertion in `run()`/`step()` reschedule sites, `:2170-2172` and `:2243-2276`)
- Test: `crates/retrace-box/tests/threads.rs`

**Interfaces:**
- Produces: `ReplaySession::current_thread(&self) -> u32`; `Box_::threads()` already exists at `crates/retrace-box/src/lib.rs:3030` returning `&thread::ThreadTable`.

**Context you need:** `ThreadTable::current(&self) -> usize` (`crates/retrace-box/src/thread.rs:96`). `Box_` already exposes `threads()`. `ReplaySession` holds its box as the private field `b`.

- [ ] **Step 1: Write the failing test** in `crates/retrace-box/tests/threads.rs`

This is a box-level test because it needs a real thread switch. Follow the file's existing `pth(&b, n)` helper convention for a backed pthread address — read the top of the file first; do NOT invent a literal address, M14 hit `write_guest: ipa outside any mapped region` doing exactly that.

```rust
#[test]
fn current_thread_follows_the_scheduler_across_a_switch() {
    let mut b = tb();
    b.set_thread_start_pc(0x0001_804b_2000);
    let p = pth(&b, 1);
    b.guest_bsdthread_create([0x1000, 0, p, p, 0x90008ff, 0, 0, 0]);
    assert_eq!(b.threads().current(), 0, "creation must not switch — the real kernel does not either");

    // Block thread 0 so the scheduler has somewhere to go, then take the switch.
    b.threads_mut().block(retrace_box::thread::BlockReason::Wait { addr: 0xdead_0000 });
    b.schedule_after_block();
    assert_eq!(b.threads().current(), 1, "the scheduler must have switched to the child");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p retrace-box --test threads current_thread_follows -- --test-threads=1`
Expected: it should PASS on the box side already (M14 built this). **If it passes, that is the point** — it pins the behaviour `ReplaySession::current_thread()` will expose. Record that it passed and move on; do not fabricate a red.

- [ ] **Step 3: Add the accessor**

In `crates/retrace-core/src/lib.rs`, beside `pub fn landmark(&self) -> usize { self.idx }` (`:1514`):

```rust
    /// M15: which guest thread is running at this position. The thread is a DERIVED property of
    /// `(N, K)` — a switch happens only at a clean stop boundary between windows — so this is a
    /// query about the current position, not a coordinate the caller supplies.
    ///
    /// Reads what the box already computes. The schedule is a pure function of the guest's own
    /// syscall sequence (M14), recomputed identically on replay, so this needs nothing recorded.
    pub fn current_thread(&self) -> u32 { self.b.threads().current() as u32 }
```

- [ ] **Step 4: Assert the invariant M15 rests on (R1)**

The spec's R1: every claim about "the thread at position P" is false if a switch can happen mid-window. Today that is guaranteed by *where* the reschedule check sits, and documented only in a comment (`crates/retrace-box/src/lib.rs:2155-2169`). Make it enforced.

In `Box_::step()` (`:2243`), immediately after the existing reschedule block, add:

```rust
        // M15 R1: everything M15 says about "the thread at (N, K)" depends on the switch having
        // already happened before the first instruction of this window retires. Pin it: after this
        // point, no path may change `current` until the next run()/step() entry.
        debug_assert!(!self.threads.needs_reschedule(),
            "M15 R1: a reschedule is still pending after schedule_after_block — a mid-window switch \
             would make position->thread ambiguous");
```

Add the identical assertion after `run()`'s reschedule block (`:2170-2172`).

- [ ] **Step 5: Run the box suite**

Run: `cargo test -p retrace-box --test threads -- --test-threads=1`
Expected: PASS, test count = previous + 1.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace-box/src/lib.rs crates/retrace-box/tests/threads.rs
git commit -m "M15 t1: the session can be asked which thread is running"
```

---

### Task 2: The thread-table view, and the stale-current-slot trap

**Files:**
- Modify: `crates/retrace-core/src/lib.rs`
- Modify: `crates/retrace-box/src/lib.rs` (add `dbg_regs_of`)
- Test: `crates/retrace-box/tests/threads.rs`

**Interfaces:**
- Produces: `ReplaySession::thread_summaries(&self) -> Vec<ThreadSummary>`; `ReplaySession::dbg_regs_of(&self, tid: u32) -> Option<String>`; `pub struct ThreadSummary { pub tid: u32, pub state: ThreadState, pub is_current: bool }`; re-export `pub use retrace_box::thread::{BlockReason, ThreadState};`
- Consumes: Task 1's `current_thread()`.

**THE TRAP THIS TASK EXISTS TO AVOID — read before writing code.** `ThreadTable::ctx_of(current)` is **stale while that thread is running**. Only `switch_to_thread` writes a thread's context back into the table (`crates/retrace-box/src/lib.rs:3349-3359`), so between switches the live vCPU is the authority for the current thread and the table's slot holds whatever it had at the last switch. This is exactly why `checkpoint()` folds the live vCPU into `ctx_mut(current)` before cloning the table (`:3442-3485`). A `dbg_regs_of` that reads the table unconditionally will print **stale registers for the current thread, confidently**. That is the defect this task's test must be able to fail on.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn dbg_regs_of_reads_the_live_vcpu_for_the_current_thread_not_the_stale_table_slot() {
    let mut b = tb();
    // Put a distinctive value in a register of the CURRENT thread, WITHOUT switching. The table's
    // slot for thread 0 still holds whatever it had at construction, so a table read misses this.
    b.set_x(3, 0xfeed_face_dead_beef);

    let dump = b.dbg_regs_of(0).expect("thread 0 exists");
    assert!(dump.contains("feedfacedeadbeef"),
        "dbg_regs_of(current) must read the LIVE vCPU: the table's slot is stale between \
         switches, and printing it would be a confident lie. Got:\n{dump}");
}
```

Check the real helper name for setting a GPR on the box before writing this — M14 added `vcpu_set_x`/`set_x`-shaped helpers; grep `crates/retrace-box/src/lib.rs` for `fn set_x` and use whatever exists rather than assuming this name. If none is public, use the same mechanism the neighbouring tests in `threads.rs` use.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p retrace-box --test threads dbg_regs_of_reads_the_live -- --test-threads=1`
Expected: FAIL — `dbg_regs_of` does not exist yet (compile error is an acceptable red here).

- [ ] **Step 3: Implement `dbg_regs_of`**

In `crates/retrace-box/src/lib.rs`, beside the existing `dbg_regs`:

```rust
    /// M15: render a specific thread's registers. For the CURRENT thread this reads the live vCPU,
    /// because the table's slot is stale between switches (only `switch_to_thread` writes it back)
    /// — the same reason `checkpoint()` folds the vCPU in before cloning the table. For any other
    /// thread the table IS the authority: that thread is not on the vCPU.
    pub fn dbg_regs_of(&self, tid: usize) -> Option<String> {
        if tid >= self.threads.len() { return None; }
        if tid == self.threads.current() {
            return Some(self.dbg_regs());
        }
        Some(Self::format_ctx(self.threads.ctx_of(tid)))
    }
```

Implement `format_ctx(&ThreadCtx) -> String` to match `dbg_regs()`'s existing layout — read `dbg_regs()` first and mirror its format exactly, so the two dumps are visually comparable. Factor the shared formatting rather than duplicating it if `dbg_regs()` makes that easy; do not restructure it if it does not.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p retrace-box --test threads dbg_regs_of -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Mutation-check it**

Change the `tid == current` branch to always read the table, re-run: the test MUST fail. Revert.
**Note:** `mv`/`sed -i.bak` restore can give the file an older mtime than the build artifact, making cargo reuse the stale binary and report a false result. `touch` the file after reverting.
Record the observed failure output in your report.

- [ ] **Step 6: Add the session-level view**

In `crates/retrace-core/src/lib.rs`:

```rust
pub use retrace_box::thread::{BlockReason, ThreadState};

/// M15: one row of the debugger's thread listing.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadSummary { pub tid: u32, pub state: ThreadState, pub is_current: bool }
```

and on `ReplaySession`:

```rust
    /// M15: every thread the guest has created, in stable index order. Exited threads STAY in the
    /// table (a `join` may arrive after the exit), so they appear here too — that is information the
    /// debugger's user wants, not noise.
    pub fn thread_summaries(&self) -> Vec<ThreadSummary> {
        let t = self.b.threads();
        (0..t.len()).map(|i| ThreadSummary {
            tid: i as u32, state: t.state_of(i), is_current: i == t.current(),
        }).collect()
    }

    /// M15: a specific thread's registers, including a BLOCKED one — impossible before this
    /// milestone. `None` for an out-of-range id, which the CLI turns into a usage error.
    pub fn dbg_regs_of(&self, tid: u32) -> Option<String> { self.b.dbg_regs_of(tid as usize) }
```

- [ ] **Step 7: Run and commit**

Run: `cargo test -p retrace-box --test threads -- --test-threads=1` and
`cargo test -p retrace-core -- --test-threads=1`
Expected: both PASS.

```bash
git add crates/retrace-core/src/lib.rs crates/retrace-box/src/lib.rs crates/retrace-box/tests/threads.rs
git commit -m "M15 t2: a thread view, and registers for a thread that is not running"
```

---

### Task 3: `TRACE_MAGIC` bump and the recorded thread tag

**Files:**
- Modify: `crates/retrace-trace/src/lib.rs:46` (magic), `:16` (`Event::Syscall`), and its magic tests (`:271`, `:280`)
- Modify: `crates/retrace-core/src/lib.rs` — **every** `Event::Syscall { .. }` construction site in `record_box`
- Test: `crates/retrace-trace/src/lib.rs` unit tests

**Interfaces:**
- Produces: `Event::Syscall { num, args, ret, err, writes, thread }` where `thread: u32`.
- Consumes: Task 1's `current_thread()` — but on the RECORD side the equivalent is `b.threads().current() as u32`.

**Scale warning, measured:** there are **~34** `Event::Syscall { .. }` construction sites in `record_box` (`grep -n "Event::Syscall {" crates/retrace-core/src/lib.rs`). Adding a field breaks all of them at compile time, which is good — the danger is the site that compiles while writing a **stale or defaulted** value.

**The mitigation is not optional (spec R4).** Capture the value **once per loop iteration**, immediately after `let stop = b.run();` (`crates/retrace-core/src/lib.rs:90`), and have every arm use that local. This is safe and future-proof: `ThreadTable::block` and `exit_current` change only a thread's *state*, never `current` (`crates/retrace-box/src/thread.rs:126-143`); only `switch_to` moves `current`, and that runs at `run()`/`step()` entry. Capturing early makes the tag immune to any handler that might switch later.

- [ ] **Step 1: Write the failing magic test**

In `crates/retrace-trace/src/lib.rs`, rename/replace `magic_bumped_for_the_signal_delivery_variant` (`:271`):

```rust
    #[test]
    fn magic_bumped_for_the_syscall_thread_tag() {
        // M15 adds `thread` to Event::Syscall — a shape change, so old traces MUST be rejected
        // whole rather than misparsed.
        assert_eq!(TRACE_MAGIC, *b"RT\x00\x07");
    }
```

and update `a_trace_written_with_the_old_magic_is_rejected_whole` (`:280`) so its "old magic" literal is `b"RT\x00\x06"` — the magic this milestone supersedes.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p retrace-trace magic -- --test-threads=1`
Expected: FAIL — left `RT\x00\x06`, right `RT\x00\x07`.

- [ ] **Step 3: Bump the magic and add the field**

`crates/retrace-trace/src/lib.rs:46`:

```rust
pub const TRACE_MAGIC: [u8;4] = *b"RT\x00\x07"; // "RT" + format version 0x0007 (M15: Syscall.thread)
```

`:16`:

```rust
    Syscall { num: u64, args: [u64;8], ret: u64, err: bool, writes: Vec<Region>, thread: u32 },
```

- [ ] **Step 4: Capture the thread once, in the record loop**

In `record_box` (`crates/retrace-core/src/lib.rs`), immediately after `let stop = b.run();`:

```rust
        // M15: the thread that produced this stop, captured ONCE and used by every append arm
        // below. Read here rather than at each append because this is the only point guaranteed to
        // be the trapping thread: `run()` reschedules at ENTRY, and no handler between here and the
        // append moves `current` (block/exit_current change state only). One source of the value,
        // ~34 uses — a stale or defaulted tag is the failure mode this ordering removes.
        let thread = b.threads().current() as u32;
```

Then add `thread` to **every** `Event::Syscall { .. }` construction in `record_box`. Work through the compiler's error list; do not hand-enumerate.

**Do NOT** add `thread` to the replay-side *match* patterns that use `..` (`:978`, `:999`, `:1009`, `:1521`) — those already ignore extra fields. Task 4 handles the replay comparison deliberately.

- [ ] **Step 5: Run the trace and core suites**

Run: `cargo test -p retrace-trace -- --test-threads=1` then `cargo test -p retrace-core -- --test-threads=1`
Expected: PASS. Fix any construction site the compiler still flags.

- [ ] **Step 6: Verify no site defaulted the tag**

Run: `grep -n "thread: 0" crates/retrace-core/src/lib.rs`
Expected: **no matches in `record_box`.** A literal `0` is the exact defect this task's design prevents; if one exists, it is a bug, not a shortcut. (A `0` inside a *test* fixture is fine.)

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-trace/src/lib.rs crates/retrace-core/src/lib.rs
git commit -m "M15 t3: the trace records which thread issued each syscall"
```

---

### Task 4: The oracle compares the thread

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` — `ReplaySession::advance`'s syscall verification (around `:1009`)
- Test: `crates/retrace/tests/` — a new `thread_oracle.rs`, or extend an existing replay test

**Interfaces:**
- Consumes: Task 3's `Event::Syscall.thread`, Task 1's `current_thread()`.

**Why this exists:** M14's Status section bills the oracle's missing thread identity as its sharpest limit — it compares `(num, args)` only, so **two threads running the same code can issue byte-identical syscalls and a wrong-thread replay continues in silence.**

- [ ] **Step 1: Write the failing test**

Build a trace, corrupt one syscall event's `thread` field, and require a `Divergence`. Read `crates/retrace/tests/determinism.rs` first for how that crate builds and replays a trace, and reuse its helpers.

```rust
#[test]
fn a_wrong_thread_on_replay_is_a_divergence() {
    // Record a threaded guest, then rewrite one Syscall event's thread tag to a different live
    // thread and require replay to reject it. Without M15's compare this passes silently, which is
    // precisely the hole M14's Status section names.
    // (Construct the tampered trace with retrace_trace::Writer over the read-back events.)
}
```

Fill this in against the real helpers — a plan snippet that names a helper this crate does not have is the failure mode M14's pre-flight scan caught four times. **Read before you write.**

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p retrace --test thread_oracle -- --test-threads=1`
Expected: FAIL — replay completes without a `Divergence`.

- [ ] **Step 3: Add the comparison**

In `ReplaySession::advance`, in the arm that binds a recorded `Event::Syscall` (`:1009`), bind the recorded thread and compare it against the recomputed one, in the same shape as the existing `(num, args)` check:

```rust
                        if self.b.threads().current() as u32 != *rthread {
                            return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                "thread {} on replay, {} recorded — the schedule diverged. Two \
                                 threads running the same code issue identical (num, args), which \
                                 is exactly the case this check exists to catch",
                                self.b.threads().current(), rthread) });
                        }
```

Place it **after** the existing `(num, args)` divergence check, so a genuine syscall divergence still reports as one rather than being masked by a thread mismatch it caused.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p retrace --test thread_oracle -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Prove it is not vacuous**

Delete the comparison, re-run: the test MUST fail. Restore, `touch` the file, re-run green. Record both transcripts in your report. A green here without this mutation is not evidence.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace/tests/thread_oracle.rs
git commit -m "M15 t4: replay notices when it runs the wrong thread"
```

---

### Task 5: Watch hits carry the thread that wrote

**Files:**
- Modify: `crates/retrace-core/src/lib.rs:825` (`Advance`), `:1469-1473` (hardware path), `:884-888` (software path)
- Test: `crates/retrace/tests/watch.rs`

**Interfaces:**
- Produces: `Advance::Watch { thread: u32 }` and `Advance::WatchSyscall { watched: u64, thread: u32 }`.
- Consumes: Task 1's `current_thread()`.

**Note:** `Advance::Watch` is currently a unit variant (`pub enum Advance { Event, Exited(ReplayReport), Break, Watch, WatchSyscall { watched: u64 } }`, `:825`). Making it a struct variant breaks every match site — follow the compiler.

- [ ] **Step 1: Write the failing test** in `crates/retrace/tests/watch.rs`, mirroring the existing `hw_watchpoint_fires_on_store_pre_retire_with_far`'s setup, and asserting the reported thread is the one that stored.

- [ ] **Step 2: Run to verify it fails.** Expected: compile error — `Watch` has no `thread` field.

- [ ] **Step 3: Add the field to both variants and populate from `self.b.threads().current() as u32`** at each construction site. For the software path, populate where `syscall_watch_hit` is consumed in `finish_event` (`:884-888`).

- [ ] **Step 4: Run the watch suites**

Run: `cargo test -p retrace --test watch -- --test-threads=1` then `cargo test -p retrace --test watch_dyn -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace/tests/watch.rs
git commit -m "M15 t5: a watch hit names the thread that wrote"
```

---

### Task 6: An armed watchpoint survives a context switch

**Files:**
- Test: `crates/retrace-box/tests/threads.rs`

**Why this is its own task:** debug registers (`DBGWVR/DBGWCR`, `DBGBVR/DBGBCR`, `MDSCR_EL1`) are **not** in `ThreadCtx` (`crates/retrace-box/src/thread.rs:24-32`) and `switch_to_thread` touches only `save_ctx`/`load_ctx` (`crates/retrace-box/src/lib.rs:3349-3359`). They are vCPU-global, so an armed watch keeps firing across a switch and catches any thread's store. **That is correct and desirable** — one vCPU, one address space — but every M5 test predates M14 and uses a single-threaded guest, so it is **correct by accident and entirely unexercised.** This task pins it. It is test-only; no product code should change.

- [ ] **Step 1: Write the test** — arm a hardware watchpoint, force a switch via `block` + `schedule_after_block`, and assert the watch registers are still armed afterward (read `DBGWCR0_EL1` and `MDSCR_EL1.MDE` back off the vCPU).

**Assert the hardware side, not only the bookkeeping.** M13's Task 8 defect was a test that checked only the software mirror (`watch_ranges`) and passed while the leaf disagreed. Checking `watch_ranges` alone here would pass even if `load_ctx` wiped `MDSCR_EL1`.

- [ ] **Step 2: Run it.** Expected: PASS (this pins existing behaviour).

- [ ] **Step 3: Mutation-check it.** Add `MDSCR_EL1` to `load_ctx`'s restore set so a switch clobbers it, re-run: the test MUST fail. Revert and `touch`. Record the transcript — without this, the test proves nothing, since it passes on unmodified code by construction.

- [ ] **Step 4: Commit**

```bash
git add crates/retrace-box/tests/threads.rs
git commit -m "M15 t6: the watchpoint that survives a switch now has a test saying so"
```

---

### Task 7: The debug CLI grows a thread vocabulary

**Files:**
- Modify: `crates/retrace/src/debug.rs` — `Cmd` (`:24-36`), `parse_one` (`:74-115`), `cmd_regs` (`:261`), `cmd_where` (`:266`)
- Test: `crates/retrace/tests/debug_cli.rs`

**Interfaces:**
- Consumes: Tasks 1 and 2 (`current_thread`, `thread_summaries`, `dbg_regs_of`).

**Context:** `Exec` holds `session: Option<ReplaySession>` with accessors `sess()`/`sess_mut()` (`:189-190`). `cmd_regs` currently does `let dump = self.sess().dbg_regs(); line(out, format_args!("{dump}"))`. `cmd_where` prints `at ({n}, {k}) pc={pc:#x}`. Output goes through the `line(out, format_args!(..))` helper — use it, do not `println!`.

- [ ] **Step 1: Write the failing tests** in `crates/retrace/tests/debug_cli.rs`, following that file's existing script-driven style (`--script '<cmds>'`). Cover: `threads` lists every thread and marks the current one; `where` names the thread; `regs 1` dumps a non-current thread; `regs 99` is a **usage error, not a panic** (matching `debug_arg_errors_are_usage_not_panics`).

- [ ] **Step 2: Run to verify they fail.** Expected: `unknown command: threads`.

- [ ] **Step 3: Add the commands**

`Cmd` gains:

```rust
    Threads,
    RegsOf(u32),
```

`parse_one` gains — note `regs` currently takes no operands, so it becomes at-most-one:

```rust
        "threads"         => expect_none(Cmd::Threads, &ops),
        "regs"            => {
            at_most_one(verb, &ops)?;
            match ops.first() {
                None => Ok(Cmd::Regs),
                Some(t) => Ok(Cmd::RegsOf(t.parse::<u32>()
                    .map_err(|_| format!("bad thread id: {t}"))?)),
            }
        }
```

`cmd_where` gains the thread:

```rust
        line(out, format_args!("at ({}, {}) pc={pc:#x} thread={}",
            self.n, self.k, self.sess().current_thread()))
```

Add `cmd_threads` rendering one line per `ThreadSummary` with a marker for the current thread, and `cmd_regs_of` returning a usage error for `None`.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p retrace --test debug_cli -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Check the determinism gate is still green**

Run: `cargo test -p retrace --test reverse_debug_e2e -- --test-threads=1`
Expected: PASS. Spec R3 — `reverse_debug_transcript_is_deterministic` is the guard that CLI output stays reproducible, and `where` just changed shape.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace/src/debug.rs crates/retrace/tests/debug_cli.rs
git commit -m "M15 t7: the debugger can name, list, and inspect threads"
```

---

### Task 8: Per-thread watch scoping

**Files:**
- Modify: `crates/retrace/src/debug.rs` — `Cmd::Watch`, `parse_one`, `Exec.watches`, `cmd_continue` (`:412-414`), `cmd_reverse_continue` (`:492-541`)
- Test: `crates/retrace/tests/watch_cli.rs`

**Design constraint from the spec:** scoping is a **debugger-side filter**. The hardware slot stays global — one vCPU underlies every thread, and there is no per-thread `DBGW`. The debugger discards hits whose thread does not match. Do NOT attempt to scope in hardware.

- [ ] **Step 1: Write the failing test** — a guest where two threads write the same watched address; `watch <addr> thread 1` reports only thread 1's write.

- [ ] **Step 2: Run to verify it fails.**

- [ ] **Step 3: Extend `Cmd::Watch(u64, u64)` to carry `Option<u32>`** and filter at the hit sites in `cmd_continue`/`cmd_reverse_continue`, which already match on `Advance::Watch`/`Advance::WatchSyscall` (`:497-502`).

- [ ] **Step 4: Run the watch CLI suite.** Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace/src/debug.rs crates/retrace/tests/watch_cli.rs
git commit -m "M15 t8: a watch can be scoped to one thread, in the debugger"
```

---

### Task 9: The headline — a guest whose threads write different memory

**Files:**
- Create: `crates/retrace-guest/rs/watchthread.rs`
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs`
- Create: `crates/retrace/tests/thread_watch_e2e.rs`

**Context:** copy the **real** `threadrust` recipe out of `crates/retrace-guest/build.rs` — do not paraphrase it. Add a `WATCHTHREAD` path constant beside `THREADRUST` in `crates/retrace-guest/src/lib.rs`. A test that spawns the CLI must codesign it first: copy `crates/retrace/tests/util/mod.rs::bin()`.

- [ ] **Step 1: Write the guest**

```rust
// M15's headline guest. Two threads that write DIFFERENT addresses, so a watch hit's thread
// attribution is a claim that can be wrong — which is what makes the gate meaningful.
//
// The child writes the watched cell; main never touches it. Exit 0 proves nothing on its own, and
// neither does "the watch fired" — the watch already fires correctly today without any of M15.
// The assertion is WHICH THREAD is named.
static mut CHILD_CELL: u64 = 0;
static mut MAIN_CELL: u64 = 0;

fn main() {
    println!("main before spawn");
    let h = std::thread::spawn(|| {
        unsafe { std::ptr::write_volatile(&raw mut CHILD_CELL, 0xC417_D000_0000_0001) };
        println!("child wrote");
    });
    unsafe { std::ptr::write_volatile(&raw mut MAIN_CELL, 0x9A1B_0000_0000_0002) };
    h.join().unwrap();
    println!("joined");
}
```

- [ ] **Step 2: Wire the build**, mirroring `threadrust`'s block in `build.rs` exactly.

- [ ] **Step 3: Write the gate**

It must assert **the attribution**, not the firing:

```rust
// THE M15 HEADLINE GATE. The child writes the watched cell; main writes a different one.
//
// "the watch fired" is NOT the assertion — it fires correctly today with none of M15 present. The
// milestone's whole contribution is WHICH THREAD is named, so that is what this gate can fail on.
```

Drive `retrace debug` with a script that watches the child's cell and runs `reverse-continue`, then assert the reported thread is the child's, and that `regs <child>` dumps registers for a thread that is not current.

**Learn the watched address from the guest's own recorded behaviour, not a hardcoded literal** — M13's `protnone_rust_e2e` learned its page from the recorded `mprotect` and took the *last* of four, because three were libSystem's. Do the analogous thing here rather than pinning an address that will move when the guest is recompiled.

- [ ] **Step 4: Run the gate.** Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-guest/rs/watchthread.rs crates/retrace-guest/build.rs \
        crates/retrace-guest/src/lib.rs crates/retrace/tests/thread_watch_e2e.rs
git commit -m "M15 t9: the headline — reverse-continue names the thread that wrote"
```

---

### Task 10: Prove the milestone is non-vacuous

**Files:** none changed (report-only; commit `--allow-empty`)

M14's Task 10 learned the lesson this task applies: **a mutation aimed at a pure function measures that function, not its callers.** Breaking `pick_next` failed 8 tests while *deleting the scheduler's call site in `run()` outright* passed the entire crate, 150/150. Break the **call site**, not just the callee.

- [ ] **Step 1: For each M15 mechanism, delete or corrupt it and record exactly which tests fail.**

| Mutation | Must fail |
|---|---|
| `current_thread()` returns a constant `0` | Task 1 and Task 7 tests |
| `dbg_regs_of` always reads the table | Task 2's stale-slot test |
| The oracle's thread compare deleted | Task 4's test |
| `Advance::Watch.thread` hardcoded to `0` | Task 5 and Task 9's gate |
| `MDSCR_EL1` added to `load_ctx` | Task 6's test |

- [ ] **Step 2: If any mutation fails ZERO tests, that mechanism is untested.** Say so loudly and stop — do not paper over it. This is the F-1 situation from M14, and it is the whole reason this task exists.

- [ ] **Step 3: Write the report** to `.superpowers/sdd/2026-08-15-retrace-m15-threaddebug/task-10-nonvacuity.md` and commit:

```bash
git commit --allow-empty -m "M15 t10: every mechanism fails a test when broken"
```

---

### Task 11: The honest close

- [ ] **Step 1: Run the full gate, chunked** (see Global Constraints). Record measured totals; **do not write a projection into the README.** Reconcile the number against the per-task counts rather than waving it through.

- [ ] **Step 2: Write the README Status section.** It must bill, honestly:
  - What now runs, with measured gate numbers.
  - **The `TRACE_MAGIC` bump and that every pre-M15 recording is now unreadable** (spec R2).
  - That `Event::Sched` was **considered and rejected**, and why — the silent landmark renumbering, and that nothing in the dispatch loops can see a switch. If open question 2 was resolved by removing the variant, say so.
  - That debug registers are vCPU-global and the cross-switch watch was correct-by-accident and untested until Task 6.
  - Everything still unmodelled: per-thread reverse execution as its own position space, preemption / spin-waiting guests, `workq`/GCD, thread priority, per-thread signal masks, plus everything M14 and M13 carry forward.
  - Whatever measurement contradicted this plan. A plan that survives contact unamended is more likely unexamined than perfect.
  - **THE FIDELITY CAVEAT — carried from Tasks 4 and 5; bill BOTH halves.** This is the requirement
    most likely to be lost, because it is a limit on work that PASSED. The milestone's guards are
    mutation-tested, but not all are exercised against a genuinely LIVE second thread id:
      * **Task 4 (the oracle's thread check).** All three landmark-consuming arms have per-arm
        mutation-tested guards. Only the GENERIC arm is exercised with a real second thread
        (THREADRUST). The two signal-path arms (caught-raise, `sigreturn`) are exercised by
        SIGFRAME, a SINGLE-threaded fixture: those tests prove the check FIRES and reports a
        `Divergence`; they do NOT prove it DISTINGUISHES two live schedules. No threaded-AND-
        signalling guest exists.
      * **Task 5 (the watch hit's thread).** Both construction sites — the hardware `Stop::Other`
        arm and the software `finish_event` — have per-site mutation-tested guards, but on
        WATCHLOOP and FILEIO, which are both single-threaded. Task 9's WATCHTHREAD gate is what
        discharges this half: if Task 9 landed asserting the CHILD's id specifically, say so and
        call it discharged; if it did not, this half stands and must be billed as an untested
        distinction.
    **Do NOT restate the superseded version of this caveat.** An earlier ruling said the two
    signal-path arms would ship "argued by inspection, untestable without a threaded-and-signalling
    guest." That ruling was wrong and was reversed — SIGFRAME reaches both arms. Billing the stale
    version would claim LESS coverage than the milestone actually has.

- [ ] **Step 3: Update CLAUDE.md** — the headline-gate list (eight → nine if Task 9 landed), and the "Guest threads" section, which currently states the oracle has no thread identity. **That claim becomes false in Task 4 and must be rewritten, not left standing.**

- [ ] **Step 4: Commit, then `superpowers:finishing-a-development-branch`.** The M14 precedent is a local `--no-ff` merge to `main` left unpushed; **ask, do not assume.**

---

## Self-Review

**Spec coverage.** M15-expose → Tasks 1, 2. M15-tag → Tasks 3, 4. M15-watch → Tasks 5, 8. M15-cli → Task 7. Untested-hazard test → Task 6. Exit criterion → Task 9. Fail-loud boundaries: unknown thread id → Tasks 2, 7 (usage error); recorded-vs-recomputed mismatch → Task 4; position→thread ambiguity → Task 1's R1 assertion. Risk register: R1 → Task 1; R2 → Tasks 3, 11; R3 → Task 7 Step 5; R4 → Task 3's capture-once design; R5 → Task 2 (the view reads `Box_`'s live table, never a parallel copy). Testing ladder → Tasks 1–2 (unit), 6 (box), 4 (oracle), 7–8 (CLI), 9 (headline), 10 (non-vacuity).

**Gaps accepted and named.** Spec open question 1 (how much of `ThreadCtx` the view exposes) is **resolved** in Task 2: a summary plus on-demand `dbg_regs_of`, so `threads` never clones 32 Q-registers per thread. Open question 3 (should both threads write) is **resolved** in Task 9: both write, to *different* addresses, so attribution is a claim that can be wrong. **Open question 2 — whether to delete the now-definitively-unused `Event::Sched` — has no task and is deliberately left to the implementer of Task 3**, who is already editing that enum under a magic bump; Task 11 must report whichever way it went rather than letting it pass silently.

**Placeholder scan.** Tasks 4, 5, 8 and 9 carry test *intent* plus explicit "read the real helpers first" instructions rather than fabricated helper calls. This is deliberate: M14's pre-flight scan found **four** compile-level errors in plan snippets that named APIs the tree does not have, each of which a subagent would have hit blind. A snippet inventing `util::` helpers would repeat exactly that. Every snippet that names a real API (`ThreadTable::current`, `Box_::threads`, `TRACE_MAGIC`, `Advance`, `Cmd`, `parse_one`, `expect_none`, `at_most_one`, `line`, `sess()`) was read out of the tree at `main` = `4b23cf8` while writing this plan.

**Type consistency.** `ThreadSummary { tid: u32, state: ThreadState, is_current: bool }` is defined in Task 2 and used unchanged in Task 7. `current_thread()` returns `u32` everywhere (Tasks 1, 4, 5, 7). `dbg_regs_of` takes `usize` on `Box_` and `u32` on `ReplaySession` — deliberate, matching each layer's existing convention (`ThreadTable` indexes with `usize`; the trace tag and CLI are `u32`), and the conversion happens in exactly one place, Task 2's session wrapper.

**Known plan risk.** Task 3 touches ~34 construction sites. The capture-once design makes a *stale* tag structurally impossible, but Step 6's `grep` for `thread: 0` is the backstop, and it is not optional.
