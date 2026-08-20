# M17-blockedsignal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A signal sent to a thread blocked in `__ulock_wait` is pended and delivered when that thread wakes, so `sigblocked_e2e` comes green with its assertions unmodified.

**Architecture:** Widen the raise path's pend condition from "the target's mask blocks this signal" to "…or the target is not `Runnable`", then materialise the pending signal at the wake — the one landmark where a blocked thread becomes runnable. This reuses M16's per-thread pending set and its materialise-at-a-landmark pattern verbatim. Nothing new enters the trace; the pend/wake decision is a pure function of the guest's own syscall sequence, so record and replay schedule identically with nothing recorded.

**Tech Stack:** Rust 1.95.0, `aarch64-apple-darwin`, Hypervisor.framework, macOS 26.x on Apple Silicon.

**Spec:** `docs/superpowers/specs/2026-08-20-retrace-m17-blockedsignal-design.md`

## Global Constraints

Copied verbatim from the spec and CLAUDE.md. Every task's requirements implicitly include this section.

- **`--test-threads=1` is mandatory.** HVF allows one VM per process; without it tests flake `HV_BUSY`.
- **NEVER run `cargo test --workspace`.** It exceeds the 10-minute tool ceiling and gets killed on this machine. Chunk per-crate: `cargo test -p retrace-box`, `-p retrace-core`, `-p retrace-trace`, `-p retrace-arch`, `-p retrace-guest`, `-p retrace-sim`, `-p hv-sys`, `cargo test -p retrace --bins`, and `cargo test -p retrace --test <name>` per target. **`cargo test -p retrace --lib` is invalid** and fails the whole invocation.
- Grep gate logs with `grep -a` — they carry ANSI and UTF-8 that trips plain grep.
- **`TRACE_MAGIC` stays `RT\x00\x08`. Do not bump it.** `SignalDelivery` already exists and already carries a thread tag. A task that proposes a format change has misunderstood something.
- **The oracle census stays at SEVEN `verify_thread` call sites and EIGHT places the oracle compares a thread.** No task in this plan adds or removes one. A task that believes it must has discovered the spec's landmark-arithmetic correction is itself wrong, and must stop and say so rather than adjust a count quietly — this census has drifted three times already.
- **Symmetry rule 1:** a special case in record's `match stop` needs a mirror in `ReplaySession::advance`; both must call the *same* `Box_` method with the *same* arguments, and sit *before* the generic forward arm. Replay's byte-compare IS the divergence check.
- **Symmetry rule 2:** deterministic emulation belongs below the trace inside `Box_::run()` — but a `SignalDelivery` is a **landmark** and cannot live there.
- **Honest-gate discipline.** Never assert on an exit code a weaker failure would also produce. A skipped test must announce itself. Park at *measured* walls, quoting literal failure text.
- **Mutation over argument.** Every claim that a test catches something is established by making the mutation and watching it fail. A test never seen red has not been tested.
- `clippy.toml` bans `Instant::now`/`SystemTime::now` and `std::thread::Thread`. Load-bearing, not style.
- Tests that spawn the CLI must codesign — use `util::record`/`util::record_dynamic`/`util::replay`, never hand-rolled.
- Run `cargo clippy --workspace --all-targets -- -D warnings` before any task is called done.

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `crates/retrace/tests/blockedctx.rs` | **New.** The R1 measurement gate: pins that a `Wait`-blocked thread's saved context is a completed syscall. | 1 |
| `crates/retrace-box/src/thread.rs` | `unblock_waiters_on` reports *which* threads it woke; `unblock_joiners_of` likewise, as a tripwire; the non-destructive `take_deliverable_peek`. | 2, 4 |
| `crates/retrace-box/src/lib.rs` | `guest_ulock_wake` returns the woken tids; new `should_pend_for` predicate; the exit-time pending guard. | 2, 3, 6 |
| `crates/retrace-core/src/lib.rs` | Both raise arms consult `should_pend_for`; record's wake arm materialises; replay's wake hook consumes two landmarks. | 3, 4, 5 |
| `crates/retrace-box/tests/threads.rs` | Unit tests for the woken-tid reporting (pure `ThreadTable`, no VM). | 2 |
| `crates/retrace-box/tests/deliver.rs` | `Box_`-level unit tests: the widened pend predicate, the stranded-signal guard. Has the `boxed()` helper `threads.rs` lacks. | 3, 6 |
| `crates/retrace/tests/sigblocked_e2e.rs` | Un-parked. **Assertions unmodified.** | 7 |
| `crates/retrace/tests/thread_oracle.rs` | Mutation test for the materialised delivery's thread tag. | 8 |
| `README.md`, `docs/status-log.md`, `CLAUDE.md` | Docs. README edited in place; status-log appended, never rewritten. | 9 |

---

### Task 1: Measure the load-bearing claim (R1)

The entire design rests on a **reading** of `crates/retrace-core/src/lib.rs:865-870` — that `guest_ulock_wait` marks the thread `Blocked` and only *then* does `set_x0_err_and_return` complete the syscall, so the saved context is a completed post-syscall state. **Measure it before building anything on it.**

If this task fails, STOP and report. The design changes shape: materialisation would first need the equivalent of `complete_syscall_before_delivery` applied to a saved context rather than the live vCPU, which is a task of its own.

**Files:**
- Create: `crates/retrace/tests/blockedctx.rs`

**Interfaces:**
- Consumes: `retrace_core::{ReplaySession, Advance, ThreadState, BlockReason}` (all already public; `ThreadState`/`BlockReason` are re-exported at `crates/retrace-core/src/lib.rs:8`), `ReplaySession::thread_summaries() -> Vec<ThreadSummary>` where `ThreadSummary { tid: u32, state: ThreadState, is_current: bool }`, and `ReplaySession::dbg_regs_of(tid: usize) -> Option<String>`.
- Produces: nothing consumed by later tasks. This is a gate, not a component.

- [ ] **Step 1: Write the measurement gate**

Create `crates/retrace/tests/blockedctx.rs`:

```rust
// M17 Task 1 / spec risk R1. The M17 design rests on ONE fact, and this file is its measurement:
//
//   A `Wait`-blocked thread's saved context is a COMPLETE post-syscall state — x0 already holds
//   `__ulock_wait`'s return value and the pc is already past the `svc`.
//
// If that holds, a signal frame built on that context PRESERVES the resume point rather than
// overwriting it (the frame saves what it displaces; `sigreturn` restores it), and M16's parked
// wall is narrower than its own `#[ignore]` text describes. If it is false, materialisation must
// first complete the syscall on the SAVED context, which is a different design.
//
// The reading this checks is `crates/retrace-core/src/lib.rs:865-870`: `guest_ulock_wait` marks the
// thread Blocked, and only THEN does `set_x0_err_and_return` write x0 and advance the pc on the
// live vCPU; the switch that saves it happens on the next `run()`.
mod util;
use retrace_core::{Advance, BlockReason, ReplaySession, ThreadState};
use std::path::Path;

/// Pull `x{n}` out of `dbg_regs_of`'s dump. The format is `format_gprs`'s
/// `x{i:<2}={xi:#018x}  ` — note the left-aligned index, so `x0` is followed by a space.
fn parse_x(dump: &str, n: usize) -> u64 {
    let key = format!("x{n:<2}=");
    let at = dump.find(&key)
        .unwrap_or_else(|| panic!("no `{key}` in the register dump:\n{dump}"));
    let hex = &dump[at + key.len()..][..18]; // "0x" + 16 hex digits
    u64::from_str_radix(hex.trim_start_matches("0x"), 16)
        .unwrap_or_else(|e| panic!("could not parse `{hex}` from the dump: {e}\n{dump}"))
}

#[test]
fn a_wait_blocked_threads_saved_context_is_a_completed_syscall() {
    let (rec, trace) = util::record_dynamic(retrace_guest::THREADRUST);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    // Advance until some thread is parked in Blocked(Wait). THREADRUST's main joins its child, and
    // `__pthread_join` blocks in `__ulock_wait`, so this state is reached on every run.
    let mut s = ReplaySession::open(Path::new(&trace)).unwrap();
    let blocked = loop {
        if let Some(t) = s.thread_summaries().into_iter()
            .find(|t| matches!(t.state, ThreadState::Blocked(BlockReason::Wait { .. })))
        {
            break t.tid as usize;
        }
        match s.advance().expect("no divergence on an untampered trace") {
            Advance::Exited(_) => panic!(
                "THREADRUST never parked a thread in Blocked(Wait). `pthread_join` blocking in \
                 `__ulock_wait` is the premise of the whole M17 design, so either the guest \
                 changed or `guest_ulock_wait` stopped blocking — investigate before proceeding."),
            _ => continue,
        }
    };

    let dump = s.dbg_regs_of(blocked).expect("the blocked thread must have a saved context");
    let x0 = parse_x(&dump, 0);

    // THE MEASUREMENT. x0 == 0 is `__ulock_wait`'s return value, i.e. a COMPLETED syscall.
    // The two operation words are what x0 would hold if the context were the PRE-syscall state —
    // they are `guest_ulock_wait`'s own whitelist (`crates/retrace-box/src/lib.rs`), so they are
    // the sharpest possible discriminator rather than an arbitrary sentinel.
    assert_ne!(x0, 0x1000002,
        "R1 FALSE: x0 holds __ulock_wait's OPERATION WORD, so the saved context is the PRE-syscall \
         state. The M17 design's load-bearing claim does not hold — STOP and re-shape the design \
         per the spec's R1 note. Dump:\n{dump}");
    assert_ne!(x0, 0x1020002,
        "R1 FALSE: x0 holds __ulock_wait's other operation word (see the assertion above). \
         Dump:\n{dump}");
    assert_eq!(x0, 0,
        "R1: a Wait-blocked thread's saved context must be a COMPLETE post-syscall state, with x0 \
         holding __ulock_wait's return value of 0. Got {x0:#x} — neither the return value nor \
         either operation word, so the ordering is something this measurement did not anticipate. \
         Investigate before building on it. Dump:\n{dump}");

    eprintln!("R1 MEASURED: thread {blocked} is Blocked(Wait) with a completed context, x0={x0:#x}");
}
```

- [ ] **Step 2: Run it and record the literal result**

Run: `cargo test -p retrace --test blockedctx -- --test-threads=1 --nocapture`

Expected: PASS, printing the `R1 MEASURED:` line. **Paste the literal output into the task report.** If it FAILS, stop and report BLOCKED with the dump — the design changes shape and the remaining tasks are invalid as written.

- [ ] **Step 3: Prove the gate can fail**

Temporarily change the final `assert_eq!(x0, 0, …)` to `assert_eq!(x0, 1, …)`, re-run, confirm it fails, then revert. A measurement gate that has never been seen red has not been tested.

- [ ] **Step 4: Commit**

```bash
git add crates/retrace/tests/blockedctx.rs
git commit -m "M17 t1: measure that a Wait-blocked thread's saved context is a completed syscall"
```

---

### Task 2: `guest_ulock_wake` reports which threads it woke

Materialisation needs the tids, not a count. After the wake the woken threads are simply `Runnable`, indistinguishable from threads that were never blocked — so the wake must report them as it performs them.

**Files:**
- Modify: `crates/retrace-box/src/thread.rs:290` (`unblock_waiters_on`)
- Modify: `crates/retrace-box/src/lib.rs:3486` (`guest_ulock_wake`)
- Modify: `crates/retrace-core/src/lib.rs:877` (record's wake arm), `:1777` (replay's wake hook) — call-site updates only
- Test: `crates/retrace-box/tests/threads.rs:483`

**Interfaces:**
- Produces: `ThreadTable::unblock_waiters_on(&mut self, addr: u64) -> Vec<usize>` (was `-> usize`), and `Box_::guest_ulock_wake(&mut self, args: [u64; 8]) -> (u64, Vec<usize>)` (was `-> u64`). Tasks 4 and 5 destructure that pair.

- [ ] **Step 1: Update the existing test to the new shape (it must fail to compile first)**

In `crates/retrace-box/tests/threads.rs`, in `unblock_waiters_on_wakes_only_the_matching_address`, replace the two count assertions:

```rust
    assert_eq!(t.unblock_waiters_on(0xAAA0), vec![0],
        "exactly one waiter is on that address, and it is thread 0 — M17 needs the IDENTITY, not \
         just the count: materialising a pending signal at the wake requires knowing WHO woke");
```

and

```rust
    assert_eq!(t.unblock_waiters_on(0xC0DE), Vec::<usize>::new(),
        "a wake with no waiter is a no-op, not a fault");
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p retrace-box --test threads -- --test-threads=1 unblock_waiters_on`
Expected: FAIL to compile — `expected Vec<usize>, found usize`.

- [ ] **Step 3: Change `unblock_waiters_on` to report the woken tids**

In `crates/retrace-box/src/thread.rs`, replace the body of `unblock_waiters_on`. Keep the existing doc comment and add the M17 sentence:

```rust
    /// M17: returns the woken tids rather than a count. The count told a caller's test "woke the
    /// right one" apart from "woke everything"; the IDENTITY is what lets the dispatch arms
    /// materialise a pending signal on the thread that just became runnable. `Vec::len()` still
    /// answers the old question, so no existing claim is lost.
    pub fn unblock_waiters_on(&mut self, addr: u64) -> Vec<usize> {
        let mut woken = Vec::new();
        for (tid, t) in self.threads.iter_mut().enumerate() {
            if let ThreadState::Blocked(BlockReason::Wait { addr: a }) = t.state {
                if a == addr {
                    t.state = ThreadState::Runnable;
                    woken.push(tid);
                }
            }
        }
        woken
    }
```

- [ ] **Step 4: Change `guest_ulock_wake` to pass it through**

In `crates/retrace-box/src/lib.rs`, change the signature and the final two lines of `guest_ulock_wake`. Leave the operation-word assert and every existing doc paragraph untouched, and append this paragraph to its doc comment:

```rust
    /// M17: returns `(rc, woken_tids)`. `rc` is unchanged (always 0 — see above). The tids are what
    /// the dispatch arms need in order to materialise a signal pended on a thread that was blocked:
    /// after the wake those threads are merely `Runnable`, indistinguishable from threads that were
    /// never blocked, so the wake must report them as it performs them. Both dispatch loops
    /// destructure the same pair from the same call with the same args, so symmetry rule 1 holds.
    pub fn guest_ulock_wake(&mut self, args: [u64; 8]) -> (u64, Vec<usize>) {
```

and its tail:

```rust
        let woken = self.threads.unblock_waiters_on(args[1]);
        (0, woken)
    }
```

- [ ] **Step 4b: Close the spec's `BlockReason::Join` tripwire**

The spec requires exactly ONE materialisation site, which holds only because nothing produces
`BlockReason::Join` today (measured in M16 Task 13; the `sigblocked.rs` guest's own comment records
it). But `unblock_joiners_of` **is** called on every thread exit (`crates/retrace-box/src/lib.rs:3359`),
so it is a live second wake path that happens to wake nobody. If it ever wakes someone holding a
pending signal, M17 has a materialisation site it does not cover — and would silently swallow that
signal. Assert rather than assume.

In `crates/retrace-box/src/thread.rs`, change `unblock_joiners_of` to report who it woke:

```rust
    /// M17: returns the woken tids, for the same reason `unblock_waiters_on` does — except here the
    /// caller uses them only as a TRIPWIRE. Nothing produces `BlockReason::Join` today, so this
    /// wakes nobody; M17's "exactly one materialisation site" rests on that, and a silent change
    /// would strand a signal rather than fail.
    pub fn unblock_joiners_of(&mut self, tid: usize) -> Vec<usize> {
        let mut woken = Vec::new();
        for (i, t) in self.threads.iter_mut().enumerate() {
            if let ThreadState::Blocked(BlockReason::Join { target }) = t.state {
                if target == tid {
                    t.state = ThreadState::Runnable;
                    woken.push(i);
                }
            }
        }
        woken
    }
```

Then at its product call site, `crates/retrace-box/src/lib.rs:3359`, replace
`self.threads.unblock_joiners_of(me);` with:

```rust
        // M17 tripwire. `BlockReason::Join` has no producer today, so this wakes nobody and M17
        // materialises pended signals at exactly ONE site (`guest_ulock_wake`). If a producer ever
        // appears, this becomes a second wake path that M17 does not materialise at, and a signal
        // pended on such a thread would be swallowed silently. Fail instead.
        let joiners = self.threads.unblock_joiners_of(me);
        assert!(joiners.is_empty(),
            "unblock_joiners_of woke {joiners:?}, but BlockReason::Join is supposed to have no \
             producer. M17 materialises pended signals only at __ulock_wake, so this is now a \
             second wake path — teach it to materialise, or a signal pended on one of these \
             threads is lost silently.");
```

The three existing `threads.rs` tests that call `unblock_joiners_of` (lines ~59, ~108, and the
`guest_bsdthread_terminate` wiring test ~345) call it on `ThreadTable` directly and ignore the
return, so they keep compiling unchanged — but run them to confirm.

- [ ] **Step 5: Update both dispatch call sites (mechanical, no behaviour change yet)**

`crates/retrace-core/src/lib.rs:877`, record:

```rust
            Stop::Syscall { num, args } if num == retrace_arch::SYS_ULOCK_WAKE => {
                // M17 Task 4 makes use of `_woken`; this task only threads it through.
                let (rc, _woken) = b.guest_ulock_wake(args);
                w.append(&Event::Syscall { num, args, ret: rc, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append ulock_wake: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);
            }
```

`crates/retrace-core/src/lib.rs:1777`, replay — change only the destructuring line, leaving the whole comment block and the compare untouched:

```rust
                                let ((rc, _woken), rerr) = (self.b.guest_ulock_wake(args), false);
```

- [ ] **Step 6: Run the tests**

```sh
cargo test -p retrace-box --test threads -- --test-threads=1
cargo test -p retrace --test thread_rust_e2e -- --test-threads=1
cargo test -p retrace --test sigthread_e2e -- --test-threads=1
```
Expected: all PASS. `thread_rust_e2e` and `sigthread_e2e` are the join-path gates and would fail loudly if the wake changed who it wakes.

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-box/src/thread.rs crates/retrace-box/src/lib.rs \
        crates/retrace-core/src/lib.rs crates/retrace-box/tests/threads.rs
git commit -m "M17 t2: the wake reports which threads it woke, not just how many"
```

---

### Task 3: Widen the pend condition

Today the raise path pends when the target's *mask* blocks the signal and delivers otherwise. It gains a second reason to pend: the target is not `Runnable`.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (new `should_pend_for` near `deliver_signal_to`)
- Modify: `crates/retrace-core/src/lib.rs` (record's raise arm ~`:699`, replay's raise mirror ~`:1195`)
- Test: `crates/retrace-box/tests/deliver.rs` (NOT `threads.rs` — these need a `Box_`, and
  `deliver.rs` is the file with the `boxed()` helper; `threads.rs` tests `ThreadTable` purely, with
  no VM, using its own `ctx(pc)` helper)

**Interfaces:**
- Consumes: `ThreadTable::is_blocked_for(tid, sig) -> bool`, `ThreadTable::state_of(tid) -> ThreadState`, `Box_::check_deliverable(tid) -> Result<(), String>`.
- Produces: `Box_::should_pend_for(&self, tid: usize, sig: u64) -> bool`. Tasks 4 and 5 rely on both dispatch loops calling exactly this.

- [ ] **Step 1: Write the failing tests**

Append to `crates/retrace-box/tests/deliver.rs`:

```rust
// ---- M17: the widened pend condition ------------------------------------------------------------
//
// A signal pends for TWO reasons now, not one. `take_deliverable` already respects the mask, so a
// signal pended for both is released only when both clear — no extra bookkeeping.

/// The pre-M17 reason, unchanged: the target's own mask blocks this signal.
#[test]
fn should_pend_for_is_true_when_the_targets_mask_blocks_the_signal() {
    let mut b = boxed();
    // `set_mask_of(tid, how, set)` — three arguments, matching what sigprocmask's arm calls.
    b.threads_mut().set_mask_of(0, retrace_arch::SIG_BLOCK, 1 << (30 - 1));
    assert!(b.should_pend_for(0, 30), "a masked signal pends, as it did before M17");
}

/// The M17 reason: the target cannot run, so it cannot be redirected into a handler yet.
#[test]
fn should_pend_for_is_true_when_the_target_is_blocked() {
    let mut b = boxed();
    let mut ctx = b.save_ctx();
    ctx.regs.sp_el0 -= 0x2000;
    let tid = b.threads_mut().spawn(ctx, (0, 0));
    b.threads_mut().switch_to(tid);
    b.threads_mut().block(retrace_box::thread::BlockReason::Wait { addr: 0xdead_0000 });
    b.threads_mut().switch_to(0);

    assert!(b.should_pend_for(tid, 30),
        "a BLOCKED target pends even with the signal unmasked — this is the M17 change, and it is \
         what stops the raise path reaching `deliver_signal_to`'s Runnable guard");
}

/// The negative case, without which a `should_pend_for` that always returned true would pass both
/// tests above while pending every signal in the tree and delivering none.
#[test]
fn should_pend_for_is_false_for_a_runnable_target_with_the_signal_unmasked() {
    let mut b = boxed();
    let mut ctx = b.save_ctx();
    ctx.regs.sp_el0 -= 0x2000;
    let tid = b.threads_mut().spawn(ctx, (0, 0));
    assert!(!b.should_pend_for(tid, 30),
        "a Runnable target with the signal unmasked must be delivered to, not pended — this is \
         every pre-M17 delivery, including sigthread's");
    assert!(!b.should_pend_for(0, 30), "and the current thread is Runnable by definition");
}

/// The other negative, and the one the spec is explicit about: an `Exited` target must NOT pend.
/// Its signal has no wake to be materialised at, and `assert_no_stranded_signals` scans `Blocked`
/// threads only — so pending here would swallow it in silence. Keeping `should_pend_for` false
/// leaves the raise path reaching `check_deliverable`'s refusal, which is where the spec puts it:
/// "the existing `deliver_signal_to` `Exited` arm stays a panic".
#[test]
fn should_pend_for_is_false_for_an_exited_target_so_it_still_fails_loud() {
    let mut b = boxed();
    let mut ctx = b.save_ctx();
    ctx.regs.sp_el0 -= 0x2000;
    let tid = b.threads_mut().spawn(ctx, (0, 0));
    b.threads_mut().switch_to(tid);
    b.threads_mut().exit_current(0);
    b.threads_mut().switch_to(0);

    assert!(!b.should_pend_for(tid, 30),
        "an Exited target must not pend — `delivering_to_an_exited_thread_fails_loud` is the \
         posture the spec keeps, and a pend would route around it into a silent swallow");
}
```

**Note on `set_mask_of`:** if `ThreadTable` exposes a different setter name, use whatever `sigprocmask`'s record arm calls (grep `set_mask_of\|set_mask` in `crates/retrace-box/src/thread.rs`) — do not add a new one.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p retrace-box --test deliver -- --test-threads=1 should_pend_for`
Expected: FAIL to compile — no method named `should_pend_for`.

- [ ] **Step 3: Implement the predicate**

In `crates/retrace-box/src/lib.rs`, immediately above `check_deliverable`:

```rust
    /// M17: should a raise targeting `tid` PEND rather than deliver?
    ///
    /// Two reasons, and they are independent. The mask reason is M16's and unchanged. The state
    /// reason is M17's: a BLOCKED target cannot be redirected into a handler yet — its saved
    /// context is the resume point its blocking syscall owes a return through — so the signal waits
    /// on its pending set and is materialised at the wake instead.
    ///
    /// `Blocked(_)` specifically, NOT "anything but `Runnable`". An `Exited` target must keep
    /// reaching `check_deliverable`'s refusal: the spec keeps that arm a panic because a signal to
    /// a dead thread is a modelling bug rather than a schedule divergence, and there is no wake to
    /// materialise it at. Pending on a dead thread would swallow the signal in silence instead —
    /// `assert_no_stranded_signals` scans `Blocked` threads only — which is the one failure shape a
    /// determinism oracle cannot see.
    ///
    /// This is the predicate BOTH dispatch loops consult, written once so they cannot drift on the
    /// pend-vs-deliver decision while both stayed green. `take_deliverable` already filters by
    /// mask, so a signal pended for both reasons is released only when both have cleared.
    ///
    /// Note the relationship to `check_deliverable`: this decides whether to pend, that decides
    /// whether a delivery may proceed. They agree on `Runnable` by construction — which is why the
    /// raise path can no longer reach that guard's refusal for a merely-blocked target.
    pub fn should_pend_for(&self, tid: usize, sig: u64) -> bool {
        self.threads.is_blocked_for(tid, sig)
            || matches!(self.threads.state_of(tid), thread::ThreadState::Blocked(_))
    }
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p retrace-box --test deliver -- --test-threads=1 should_pend_for`
Expected: 4 PASS.

- [ ] **Step 5: Point both dispatch loops at it**

In `crates/retrace-core/src/lib.rs`, record's raise arm: replace the condition `if b.threads().is_blocked_for(target, sig) {` with

```rust
                // M17: the pend condition is now `should_pend_for` — mask OR not-Runnable — and it
                // is a `Box_` method precisely so replay's mirror consults the identical predicate
                // rather than a second copy of the same `||`.
                if b.should_pend_for(target, sig) {
```

and in replay's raise mirror, replace `if self.b.threads().is_blocked_for(target, sig) {` with

```rust
                        if self.b.should_pend_for(target, sig) {
```

**Find them with:** `grep -n "is_blocked_for(target" crates/retrace-core/src/lib.rs` — there are exactly two, one per loop. Do not change `is_blocked_for`'s other callers (the synchronous-fault guard at `:191` is about a *fault*, which cannot be deferred, and must keep asserting).

- [ ] **Step 6: Run the signal gates**

```sh
cargo test -p retrace-box --test deliver -- --test-threads=1
cargo test -p retrace-box --test threads -- --test-threads=1
cargo test -p retrace --test sigthread_e2e -- --test-threads=1
cargo test -p retrace --test sigraise_e2e -- --test-threads=1
cargo test -p retrace --test panic_e2e -- --test-threads=1
```
Expected: all PASS. These prove the widened condition did not start pending signals that used to be delivered.

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs crates/retrace-box/tests/deliver.rs
git commit -m "M17 t3: a raise pends when the target cannot run, not only when it is masked"
```

---

### Task 4: Record's wake arm materialises the pending signal

**Files:**
- Modify: `crates/retrace-core/src/lib.rs:877` (record's `SYS_ULOCK_WAKE` arm)

**Interfaces:**
- Consumes: `Box_::guest_ulock_wake -> (u64, Vec<usize>)` (Task 2), `take_pending_delivery(b, tid) -> Option<(u64, u64)>` (existing, `:93`), `Box_::deliver_signal_to`.
- Produces: the record-side shape Task 5 mirrors exactly.

**Ordering note:** Step 1 writes an arm that calls `take_deliverable_peek`, which Step 2 adds. The
tree will not compile between them — that is expected, not a mistake. Do both before running
anything. (Steps are ordered this way so the arm's shape, which is what the task is *about*, is read
before the small helper it needs.)

- [ ] **Step 1: Write the record arm**

Replace record's `SYS_ULOCK_WAKE` arm entirely:

```rust
            Stop::Syscall { num, args } if num == retrace_arch::SYS_ULOCK_WAKE => {
                let (rc, woken) = b.guest_ulock_wake(args);
                w.append(&Event::Syscall { num, args, ret: rc, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append ulock_wake: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);

                // M17: THE ANCHOR. A signal pended on a thread because it could not run is
                // materialised HERE, at the wake landmark that made it runnable — the same argument
                // the mask arm above makes for the unmask landmark, and for the same reason: the
                // scheduler's switch point lives inside `Box_::run()`, below the trace, where a
                // `SignalDelivery` could not be emitted at all.
                //
                // NO `complete_syscall_before_delivery` here, and that is the difference from the
                // other two materialisation sites. That call fixes SPSR_EL1 on the LIVE vCPU, which
                // is the CALLER — and here the caller is the WAKER, not the receiver. The receiver's
                // frame is built from its own saved context, which Task 1 measured to be a already
                // completed syscall. Calling it would corrupt the waker's PSTATE instead.
                let deliver_to: Vec<usize> = woken.iter().copied()
                    .filter(|&t| b.threads().take_deliverable_peek(t).is_some())
                    .collect();
                assert!(deliver_to.len() <= 1,
                    "one wake made {} threads deliverable at once ({deliver_to:?}); that needs N+1 \
                     landmarks at a single stop and a decision about their order, which M17 does \
                     not model. No fixture produces it — measure the guest before modelling it.",
                    deliver_to.len());
                if let Some(&wtid) = deliver_to.first() {
                    if let Some((psig, handler)) = take_pending_delivery(&mut b, wtid) {
                        let (dwrites, resume_pc) =
                            b.deliver_signal_to(wtid, psig, retrace_arch::SI_USER, 0, 0, 0);
                        // The tag is the RECEIVER — the woken thread — not `thread`, which is the
                        // waker whose syscall this landmark belongs to. They always differ here, so
                        // this is the sharpest case of the rule `Event::SignalDelivery.thread`
                        // states, and `mirror_delivery`'s inline check is what enforces it.
                        w.append(&Event::SignalDelivery { sig: psig, si_code: retrace_arch::SI_USER,
                                                          si_addr: 0, handler, resume_pc,
                                                          writes: dwrites, thread: wtid as u32 })
                            .map_err(|e| format!("append woken delivery: {e}"))?; count += 1;
                    }
                }
            }
```

- [ ] **Step 2: Add the non-destructive peek `take_deliverable_peek` needs**

`take_deliverable` CLEARS the bit it returns, so it cannot be used to *ask* the question. Add to `crates/retrace-box/src/thread.rs`, directly above `take_deliverable`:

```rust
    /// M17: the same question `take_deliverable` answers, WITHOUT clearing the bit.
    ///
    /// Needed because the wake site must count how many woken threads have a deliverable signal
    /// before materialising any of them — and `take_deliverable`'s clear is exactly what makes it
    /// safe to call once per landmark. Asking with it would consume the signal it was asking about.
    pub fn take_deliverable_peek(&self, tid: usize) -> Option<u64> {
        let t = &self.threads[tid];
        let ready = t.pending & !t.mask;
        if ready == 0 { return None; }
        Some(ready.trailing_zeros() as u64 + 1)
    }
```

- [ ] **Step 3: Add its unit test**

Append to `crates/retrace-box/src/thread.rs`'s test module:

```rust
    // M17: the peek must be non-destructive, which is the entire reason it exists — the wake site
    // asks the question before deciding to materialise, and `take_deliverable`'s clear would
    // consume the signal it was asking about.
    #[test]
    fn take_deliverable_peek_does_not_clear_the_bit() {
        let mut t = ThreadTable::new(ctx(0x1000));
        t.pend(0, 30);
        assert_eq!(t.take_deliverable_peek(0), Some(30));
        assert_eq!(t.take_deliverable_peek(0), Some(30), "peeking twice must answer twice");
        assert_eq!(t.take_deliverable(0), Some(30), "and the real take still finds it");
        assert_eq!(t.take_deliverable_peek(0), None, "which DID clear it");
    }
```

- [ ] **Step 4: Run**

```sh
cargo test -p retrace-box -- --test-threads=1
cargo test -p retrace --test sigthread_e2e -- --test-threads=1
```
Expected: PASS. `sigblocked_e2e` will still fail — replay has no mirror yet. That is Task 5.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace-box/src/thread.rs
git commit -m "M17 t4: record materialises a pended signal at the wake that made its thread runnable"
```

---

### Task 5: Replay's wake hook consumes two landmarks

**Files:**
- Modify: `crates/retrace-core/src/lib.rs:1777` (replay's `SYS_ULOCK_WAKE` hook)

**Interfaces:**
- Consumes: everything Task 4 produced, plus `ReplaySession::mirror_delivery(tid, sig, si_code, si_addr, esr, far, pc)` and `self.idx`.
- Produces: nothing later tasks consume.

**Read before writing:** the hoisted mask arm's two-landmark tail at `crates/retrace-core/src/lib.rs:1390-1412`. This task copies that shape exactly.

**Do NOT hoist this hook into its own arm, and do NOT add a `verify_thread` call.** The hook lives inside the generic `Some(Event::Syscall { .. })` arm beginning at `:1415`, whose `verify_thread` call at `:1429` has already run; and it `return`s explicitly rather than falling through. See the spec's landmark-arithmetic section.

- [ ] **Step 1: Rewrite the hook's tail**

Keep the entire existing comment block and the `rc`/`err` compare. Replace only from `self.b.set_x0_err_and_return(*ret, *err);` to the end of the `if`:

```rust
                                self.b.set_x0_err_and_return(*ret, *err);
                                // M17: record's wake arm materialises a signal pended on a thread
                                // it just woke, appending a SECOND landmark. This side must consume
                                // both — `finish_event` takes one, `mirror_delivery` takes the
                                // other — exactly as the mask arm at :1390 does. Getting this wrong
                                // does not corrupt anything quietly: the delivery landmark would be
                                // met by the next unrelated stop and reported as "expected recorded
                                // syscall, got SignalDelivery" at some landmark past the wake.
                                //
                                // The same `Box_` calls with the same arguments as record, in the
                                // same order, so which signal materialises on which thread is
                                // identical by construction rather than by two matches agreeing.
                                let deliver_to: Vec<usize> = woken.iter().copied()
                                    .filter(|&t| self.b.threads().take_deliverable_peek(t).is_some())
                                    .collect();
                                assert!(deliver_to.len() <= 1,
                                    "one wake made {} threads deliverable at once ({deliver_to:?}) \
                                     — record asserts the same bound; see its arm",
                                    deliver_to.len());
                                return match deliver_to.first() {
                                    Some(&wtid) => match take_pending_delivery(&mut self.b, wtid) {
                                        Some((psig, _handler)) => {
                                            // Consume the Syscall landmark by hand; mirror_delivery
                                            // takes the SignalDelivery. No
                                            // complete_syscall_before_delivery — the receiver is the
                                            // WOKEN thread, not the caller, so there is no live-vCPU
                                            // PSTATE to fix up. Record's arm omits it for the same
                                            // reason; the pair must match.
                                            self.idx += 1;
                                            self.mirror_delivery(wtid, psig, retrace_arch::SI_USER,
                                                                 0, 0, 0, pc)
                                        }
                                        None => self.finish_event(),
                                    },
                                    None => self.finish_event(),
                                };
```

and change the destructuring line above it (from Task 2) to bind `woken` rather than `_woken`:

```rust
                                let ((rc, woken), rerr) = (self.b.guest_ulock_wake(args), false);
```

- [ ] **Step 2: Run the full threaded and signal gate set**

```sh
cargo test -p retrace --test thread_rust_e2e -- --test-threads=1
cargo test -p retrace --test thread_watch_e2e -- --test-threads=1
cargo test -p retrace --test sigthread_e2e -- --test-threads=1
cargo test -p retrace --test thread_oracle -- --test-threads=1
cargo test -p retrace --test kport -- --test-threads=1
```
Expected: all PASS. A landmark-arithmetic error shows up here as a divergence naming a landmark past the wake — if you see "expected recorded syscall, got SignalDelivery", Step 1's two-landmark consumption is wrong.

- [ ] **Step 3: Commit**

```bash
git add crates/retrace-core/src/lib.rs
git commit -m "M17 t5: replay's wake hook consumes the materialised delivery landmark too"
```

---

### Task 6: The exit-time pending-signal guard

A signal pended on a thread nothing ever wakes is delivered never. That is a real divergence from POSIX, and a silently-swallowed signal is worse for a determinism oracle than a crash.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (new `Box_::assert_no_stranded_signals`)
- Modify: `crates/retrace-core/src/lib.rs` (record's `SYS_EXIT` arm, before the final snapshot)
- Test: `crates/retrace-box/tests/deliver.rs` (needs `boxed()`, as in Task 3)

**Interfaces:**
- Produces: `Box_::assert_no_stranded_signals(&self)`.

**Ruling on the spec's open question 2:** the guard fires on the **clean exit path only**, not on the crash path. A guest already dying should not have this guard fire on top of its crash and replace the real diagnosis with a secondary one.

- [ ] **Step 1: Write the failing test**

Append to `crates/retrace-box/tests/deliver.rs`:

```rust
/// M17: a signal pended on a thread that never wakes is delivered NEVER — the accepted cost of
/// pend-until-wake. Accepted, but not hidden: exiting 0 while silently swallowing a signal is the
/// worst failure shape for a determinism oracle, because record and replay would agree and both be
/// wrong.
#[test]
#[should_panic(expected = "still Blocked with pending signal")]
fn a_signal_stranded_on_a_never_woken_thread_fails_loud_at_exit() {
    let mut b = boxed();
    let mut ctx = b.save_ctx();
    ctx.regs.sp_el0 -= 0x2000;
    let tid = b.threads_mut().spawn(ctx, (0, 0));
    b.threads_mut().switch_to(tid);
    b.threads_mut().block(retrace_box::thread::BlockReason::Wait { addr: 0xdead_0000 });
    b.threads_mut().switch_to(0);
    b.threads_mut().pend(tid, 30);

    b.assert_no_stranded_signals();
}

/// The negative case: the ordinary exit, where nothing is stranded, must not fire the guard.
#[test]
fn a_clean_exit_with_no_pending_signals_passes_the_guard() {
    let mut b = boxed();
    let mut ctx = b.save_ctx();
    ctx.regs.sp_el0 -= 0x2000;
    b.threads_mut().spawn(ctx, (0, 0));
    b.assert_no_stranded_signals();
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p retrace-box --test deliver -- --test-threads=1 stranded`
Expected: FAIL to compile — no method `assert_no_stranded_signals`.

- [ ] **Step 3: Implement**

In `crates/retrace-box/src/lib.rs`, next to `check_deliverable`:

```rust
    /// M17: fail loud if the guest is exiting with a signal pended on a thread that can never take
    /// it. Pend-until-wake delivers a pended signal at the wake that makes its thread runnable — so
    /// a thread still `Blocked` at exit, holding a signal its mask does not block, never got it.
    ///
    /// This is the semantic gap between pend-until-wake and POSIX, which would interrupt the wait.
    /// Named rather than hidden: exiting 0 while swallowing a signal makes record and replay agree
    /// with each other and both be wrong, which is the one failure a determinism oracle cannot see.
    ///
    /// Clean-exit path only. A guest that is already crashing must be diagnosed by its crash, not
    /// by a secondary guard firing on top of it.
    pub fn assert_no_stranded_signals(&self) {
        for tid in 0..self.threads.len() {
            if !matches!(self.threads.state_of(tid), thread::ThreadState::Blocked(_)) { continue; }
            if let Some(sig) = self.threads.take_deliverable_peek(tid) {
                panic!("thread {tid} is exiting still Blocked with pending signal {sig}, which it \
                        can therefore never take: M17 materialises a pended signal at the WAKE, and \
                        this thread was never woken. Either the guest deadlocked, or the signal \
                        needs the wait to be interrupted (EINTR) rather than deferred — which M17 \
                        deliberately does not model. State: {:?}", self.threads.state_of(tid));
            }
        }
    }
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p retrace-box --test deliver -- --test-threads=1 stranded a_clean_exit`
Expected: 2 PASS.

- [ ] **Step 5: Call it from record's exit path**

In `crates/retrace-core/src/lib.rs`, in record's `SYS_EXIT` arm, immediately before the `Event::Exit` is appended:

```rust
                // M17: the clean-exit path only — see `assert_no_stranded_signals`. The crash path
                // deliberately does not call this: a guest already dying must be diagnosed by its
                // crash, not by a secondary guard firing on top of it.
                b.assert_no_stranded_signals();
```

**Find it with:** `grep -n "append exit\|Event::Exit {" crates/retrace-core/src/lib.rs` and use the arm inside `record_box`, not replay's verify.

- [ ] **Step 6: Run the exit-path gates**

```sh
cargo test -p retrace --test hello_rust_e2e -- --test-threads=1
cargo test -p retrace --test thread_rust_e2e -- --test-threads=1
cargo test -p retrace --test crashy_e2e -- --test-threads=1
cargo test -p retrace --test sigthread_e2e -- --test-threads=1
```
Expected: all PASS. `crashy_e2e` is the one that proves the guard did not get onto the crash path.

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs crates/retrace-box/tests/deliver.rs
git commit -m "M17 t6: a signal stranded on a never-woken thread fails loud instead of vanishing"
```

---

### Task 7: Un-park `sigblocked_e2e`

**Files:**
- Modify: `crates/retrace/tests/sigblocked_e2e.rs`

**The four assertions MUST NOT be modified.** They were written correct-by-construction from M16 Task 13's measurement, and the gate's own header explains that a fix which *skips* the blocked target exits 0 and changes no stdout — which is why it asserts on the trace. If an assertion fails, read it as a claim about the fix, not as a stale assertion.

- [ ] **Step 1: Run it parked, and capture the literal before-state**

Run: `cargo test -p retrace --test sigblocked_e2e -- --test-threads=1 --ignored --nocapture`
Expected: it now PASSES (Tasks 3–5 removed the wall). **Paste the literal output into the report.** If it fails, the failure text is the deliverable — report it rather than editing the test.

- [ ] **Step 2: Remove the `#[ignore]`**

Delete the entire `#[ignore = "M16 wall: …"]` attribute. Then rewrite the file's header comment, which currently says the assertions are unexercised:

Replace the paragraph beginning `// **These assertions are UNEXERCISED TODAY and cannot be verified by running.**` and its bullet list with:

```rust
// **These assertions were written before they could be run**, correct-by-construction from what
// M16 Task 13 measured and from the guest's own source, and M17 un-parked the gate without
// modifying one of them — which is the strongest thing that can be said for a parked gate's
// honesty. What each pins:
//   - Thread 1 is `a`. Tids are creation-ordered (main 0, `a` 1, `b` 2), and the resolution of
//     `b`'s `pthread_kill(a_pt, …)` to that tid is the thing this gate exists to check.
//   - `a` really is blocked in `__ulock_wait` when the signal arrives — the state only
//     `guest_ulock_wait` produces.
//   - Sig 30 is `SIGUSR1`, the only signal the guest raises, raised exactly once.
// M17 delivers the signal at the WAKE that makes `a` runnable again, not at the raise, so the
// delivery landmark sits after `b`'s wake rather than at `b`'s pthread_kill. Both orderings satisfy
// the assertions below, and the "blocked BEFORE the delivery" tooth is what keeps that honest.
```

- [ ] **Step 3: Run it un-parked**

Run: `cargo test -p retrace --test sigblocked_e2e -- --test-threads=1`
Expected: `1 passed; 0 failed; 0 ignored`.

- [ ] **Step 4: Prove the gate rejects the skip non-fix**

Temporarily make record's wake arm (Task 4) skip materialisation entirely — change `if let Some(&wtid) = deliver_to.first()` to `if let Some(&wtid) = None::<&usize>`. Re-run the gate.
Expected: FAIL on `delivered == vec![1u32]` with an EMPTY list — the exact non-fix the gate's header names. Revert.

This is the mutation that proves the gate is worth having. **Capture its literal output.**

- [ ] **Step 5: Commit**

```bash
git add crates/retrace/tests/sigblocked_e2e.rs
git commit -m "M17 t7: un-park the blocked-target gate, assertions unmodified"
```

---

### Task 8: The materialised delivery's thread tag is checked

The new delivery reaches `mirror_delivery` by a route no existing test uses — tagged with a thread that is neither the caller nor the current thread. `mirror_delivery`'s inline `rthread != tid` check (the eighth oracle place) is what makes a wrong woken thread a divergence rather than silent corruption. Prove it fires.

**Files:**
- Modify: `crates/retrace/tests/thread_oracle.rs`

- [ ] **Step 1: Write the mutation test**

Append to `crates/retrace/tests/thread_oracle.rs`:

```rust
/// M17. The signal materialised at a WAKE is tagged with the woken thread — neither the caller of
/// the syscall that produced the landmark (that is the waker) nor the thread that was current. No
/// other delivery in the tree has that shape, so `mirror_delivery`'s inline receiving-thread check
/// reaches this route for the first time here.
///
/// Only the TAG is corrupted, not one byte of `writes` — the same isolation
/// `a_wrong_thread_on_the_delivery_landmark_is_a_divergence` documents above: a wrong-thread
/// delivery lands the frame on a different stack, so `Region`'s derived `PartialEq` would trip the
/// frame compare first and this check would never speak.
#[test]
fn a_wrong_thread_on_a_wake_materialised_delivery_is_a_divergence() {
    let (rec, trace) = util::record_dynamic(retrace_guest::SIGBLOCKED);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    let mut events = retrace_trace::Reader::open(&trace).unwrap();
    let i = events.iter().position(|e| matches!(e, Event::SignalDelivery { sig: 30, .. }))
        .expect("SIGBLOCKED must record exactly one SIGUSR1 delivery — sigblocked_e2e asserts it");
    let orig = match &events[i] { Event::SignalDelivery { thread, .. } => *thread, _ => unreachable!() };
    assert_eq!(orig, 1,
        "the delivery must be tagged with `a` (tid 1), the BLOCKED target — if this is 0 or 2 the \
         signal went to the waker or to main and M17's whole claim is wrong");

    // Retag to the WAKER (tid 2, `b`), which is the specific wrong answer this route invites: it is
    // the thread whose syscall produced the landmark, and therefore the tag a careless
    // implementation would reach for.
    if let Event::SignalDelivery { thread, .. } = &mut events[i] { *thread = 2; }

    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 3, "CLI exit 3 is the Divergence convention; stderr:\n{}", rep.stderr);
    assert!(rep.stderr.contains("signal delivery thread mismatch"),
        "the divergence must be the DELIVERY thread check, not merely some divergence — exit 3 \
         alone would pass on any of them; stderr:\n{}", rep.stderr);
}
```

- [ ] **Step 2: Run it**

Run: `cargo test -p retrace --test thread_oracle -- --test-threads=1`
Expected: 6 PASS.

- [ ] **Step 3: Prove it bites**

Temporarily comment out `mirror_delivery`'s `if rthread != tid as u32 { … }` block. Re-run.
Expected: the new test FAILS (and `a_wrong_thread_on_the_delivery_landmark_is_a_divergence` fails too — both depend on that check). Restore, re-run, confirm green. **Capture both outputs.**

- [ ] **Step 4: Commit**

```bash
git add crates/retrace/tests/thread_oracle.rs
git commit -m "M17 t8: a wrong thread on a wake-materialised delivery is a divergence"
```

---

### Task 9: Documentation

**Files:**
- Modify: `README.md` (edited in place), `docs/status-log.md` (appended to, never rewritten), `CLAUDE.md`

- [ ] **Step 1: Run the full gate and get the real numbers**

Run each chunk from Global Constraints, then `cargo clippy --workspace --all-targets -- -D warnings`. Reconcile the total against the previous close (**395 passed / 0 failed / 2 ignored across 102 test binaries at `b73bdbb`**) by counting the tests this milestone added: Task 1 adds 1 (and 1 new binary), Task 3 adds 4, Task 4 adds 1, Task 6 adds 2, Task 8 adds 1, and Task 7 moves 1 test from ignored to passing. **Expected: 405 passed / 0 failed / 1 ignored across 103 test binaries.** If the measured number differs, the measured number wins — find and explain the difference rather than adjusting the arithmetic.

- [ ] **Step 2: Update the README in place**

Remove the `sigblocked_e2e` bullet from "Known limits" (`README.md:131`) — the wall is gone. Leave the `stackoverflow_rust_e2e` bullet, and change the bullet's lead-in from "**Two gates are parked**" to "**One gate is parked**".

Add to "Known limits", because pend-until-wake's cost is now a real current limit:

```markdown
- **A signal to a thread that never wakes is never delivered.** Signals to a blocked thread are
  pended and materialised at the wake that makes the thread runnable; retrace does not interrupt the
  wait with `EINTR` as a real kernel would. A guest that strands a signal this way fails loud at
  exit rather than exiting 0 and swallowing it.
```

Update the gate line with Step 1's measured numbers and the commit measured at.

- [ ] **Step 3: Append to `docs/status-log.md`**

Append a new `## Status: M17-blockedsignal` section at the end. **Never rewrite an existing section.** Annotate M16's parked-gate claim with a forward pointer rather than editing it, following the pattern the sigaltstack and replay-divergence fast-follows established in that file.

The section must state: what the milestone did; that the R1 claim was **measured** in Task 1 (with the literal `R1 MEASURED:` line); that the landmark-arithmetic correction in the spec was found during plan-writing and what the wrong reading was; the oracle census unchanged at 7/8; and the accepted semantic gap with its guard.

- [ ] **Step 4: Update `CLAUDE.md`**

In the "Signals are per-thread too (M16)" paragraph, add that a signal to a **blocked** thread pends and is materialised at the wake, so the materialisation sites are now two (the unmasking `sigprocmask` and the `__ulock_wake`) rather than one. **Do not change the oracle census sentence** — it is still seven sites and eight places.

- [ ] **Step 5: Commit**

```bash
git add README.md docs/status-log.md CLAUDE.md
git commit -m "M17: the docs catch up with the blocked-target wall falling"
```
