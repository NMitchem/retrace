# M4 Fast-Follow Implementation Plan — checkpoint-cache polish + window-length memoization

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the three review-triaged M4 fast-follow items: the gate-0 `used_bytes` accounting guard, `Rc` storage so cache hits stop deep-cloning full guest memory, and window-length memoization so `reverse-stepi` pays a large window's length once per session instead of once per boundary crossing.

**Architecture:** Two tasks, both inside the existing M4 surfaces. Task 1 is `CheckpointCache`-internal (retrace-core): a one-line same-key accounting guard plus an `Rc<SessionCheckpoint>` storage refactor (`ReplaySession::from_checkpoint` changes to take `&SessionCheckpoint`; `checkpointed_seek` is the only caller). Task 2 adds `CheckpointCache::window_len` — a per-landmark memo of window lengths (a fixed, deterministic property of the trace) with its own deterministic step counter for testing — and wires `Exec::probe_window_len` through it, then updates the README's Deferred list honestly. No new crates, no new files except none; zero public-API removals.

**Tech Stack:** Rust workspace; Hypervisor.framework via `hv-sys`; existing `checkpoint_seek.rs` test harness (`util::record`, `retrace_guest::SPINLOOP`).

## Global Constraints

- **Branch:** create `m4-fastfollow` from `main` (main is at merge commit `d565738`): `git checkout -b m4-fastfollow`.
- **Every test run uses `--test-threads=1`** (HVF: one VM per process). Full gate: `just gate`. **Baseline: 104 passed / 0 failed / 0 ignored, clippy clean** (verified on merged main, exit-0 confirmatory run).
- **One VM per process, even inside a single test:** never hold two `Box_`/`ReplaySession` values alive at once — drop the first, then open the second. Two live sessions = `HV_BUSY`. A NAMED session binding lives to end of scope regardless of last use.
- **Zero trace-format changes.** Do not touch `retrace-trace`. Everything here is in-memory, session-scoped.
- **Transcript stability is still the product.** `crates/retrace/tests/debug_cli.rs` (7 tests) and `crates/retrace/tests/reverse_debug_e2e.rs` (1 test) must pass byte-for-byte UNMODIFIED at both task gates — memoization must be invisible to CLI output.
- **Clippy `-D warnings` clean at every commit.** **Never fake a green** — if an assertion fails, the bug is real; do not loosen it.
- **`std::rc::Rc`, not `Arc`:** this project bans threads (`clippy.toml`), the cache is single-threaded by construction.
- **Explicitly OUT OF SCOPE (do not creep):** `checkpointed_seek` still calls `s.checkpoint()` (a full-memory capture) on every seek even when the cost gate will reject it — pre-existing, unchanged here. The config knob for budget/cost-gate. Any change to `ReplaySession::window_len_here` itself.
- **Gate arithmetic** (from baseline 104/0/0): T1 **105/0/0** (+1 test in `checkpoint_seek.rs`). T2 **106/0/0** (+1).
- **Commit messages:** `M4-ff tN: <what>` + trailing `Co-Authored-By: <executing model> <noreply@anthropic.com>` (match the executing model).

---

### Task 1: `CheckpointCache` internals — same-key accounting guard + `Rc` storage

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` — file-top imports, `CheckpointCache` struct + `best_at_or_before`/`record_and_maybe_insert`, `ReplaySession::from_checkpoint` signature, `checkpointed_seek` body.
- Test: `crates/retrace/tests/checkpoint_seek.rs` (append one test).

**Interfaces:**
- Consumes: everything exists already (M4 Task 4's surfaces).
- Produces (Task 2 relies on): `CheckpointCache` unchanged public API (`new`/`total_single_steps`/`len`/`is_empty`/`used_bytes`); `ReplaySession::from_checkpoint(trace_path: &Path, checkpoint: &SessionCheckpoint) -> Result<Self, String>` (note: now takes `&SessionCheckpoint`).

- [ ] **Step 1: Write the failing test.** Append to `crates/retrace/tests/checkpoint_seek.rs`:

```rust
#[test]
fn gate_zero_same_key_reseek_does_not_double_count_bytes() {
    let (rec, trace) = util::record(retrace_guest::SPINLOOP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let trace = Path::new(&trace);
    // cost_gate_steps = 0 is the one configuration where an exact-position hit (steps_paid = 0)
    // still clears the gate and re-inserts at the SAME key — the overwrite path the byte
    // accounting must survive without double-counting.
    let mut cache = retrace_core::CheckpointCache::new(usize::MAX, 0);
    let _ = retrace_core::checkpointed_seek(trace, &mut cache, 1, 5).unwrap();
    assert_eq!(cache.len(), 1);
    let first_bytes = cache.used_bytes();
    assert!(first_bytes > 0, "a cached checkpoint must have nonzero measured size");
    let _ = retrace_core::checkpointed_seek(trace, &mut cache, 1, 5).unwrap();
    assert_eq!(cache.len(), 1, "same-key reseek must overwrite, not add an entry");
    assert_eq!(cache.used_bytes(), first_bytes,
        "used_bytes must not double-count on a same-key overwrite (gate 0)");
}
```

- [ ] **Step 2: Verify failure.** `cargo test -p retrace --test checkpoint_seek gate_zero -- --test-threads=1` — Expected: FAIL on the last assert (`used_bytes` comes back at exactly `2 * first_bytes`; both seeks land at the same deterministic position, so the two captures have identical sizes). The `len == 1` assert passes (BTreeMap insert overwrites). If it fails any other way, stop and report.

- [ ] **Step 3: Implement — guard + `Rc` refactor together.** In `crates/retrace-core/src/lib.rs`:

(a) Add to the file-top imports (alongside the existing `use` statements):

```rust
use std::rc::Rc;
```

(b) Change `CheckpointCache`'s `entries` field (line ~834):

```rust
    entries: std::collections::BTreeMap<(usize, u64), Rc<SessionCheckpoint>>,
```

(c) Replace `best_at_or_before` (lines ~860-866) — the hit path now shares the stored checkpoint instead of deep-cloning tens of MB of guest memory:

```rust
    /// The best cached position at or before `(n, k)` in execution order, if any — shared out via
    /// `Rc` (no memory copy) and marked most-recently-used.
    fn best_at_or_before(&mut self, n: usize, k: u64) -> Option<((usize, u64), Rc<SessionCheckpoint>)> {
        let key = *self.entries.range(..=(n, k)).next_back()?.0;
        self.touch(key);
        Some((key, Rc::clone(&self.entries[&key])))
    }
```

(d) Replace `record_and_maybe_insert` (lines ~868-882) — same logic, `Rc` parameter, plus the same-key guard:

```rust
    /// Record `steps_paid` toward the running total, and — only if it clears the cost gate — store
    /// `checkpoint` at `(n, k)`, evicting least-recently-used entries first while over budget.
    fn record_and_maybe_insert(&mut self, n: usize, k: u64, steps_paid: u64, checkpoint: Rc<SessionCheckpoint>) {
        self.total_single_steps += steps_paid;
        if steps_paid < self.cost_gate_steps { return; }
        let bytes = checkpoint.approx_bytes();
        while self.used_bytes + bytes > self.byte_budget && !self.recency.is_empty() {
            let oldest = self.recency.remove(0);
            if let Some(evicted) = self.entries.remove(&oldest) { self.used_bytes -= evicted.approx_bytes(); }
        }
        if bytes > self.byte_budget { return; } // a single entry over budget is never cached
        if let Some(old) = self.entries.insert((n, k), checkpoint) {
            self.used_bytes -= old.approx_bytes(); // same-key overwrite: retire the old entry's bytes
        }
        self.used_bytes += bytes;
        self.touch((n, k));
    }
```

(e) Change `ReplaySession::from_checkpoint` to borrow (line ~794; `idx`/`guest_task_port` are `Copy`, `box_state` was already accessed by reference — only the signature changes):

```rust
    pub fn from_checkpoint(trace_path: &Path, checkpoint: &SessionCheckpoint) -> Result<Self, String> {
```

(f) In `checkpointed_seek` (lines ~891-916): the two hit arms become `ReplaySession::from_checkpoint(trace_path, &checkpoint)?` (deref-coercion from `&Rc<SessionCheckpoint>`), and the final line becomes:

```rust
    cache.record_and_maybe_insert(n, k, steps_paid, Rc::new(s.checkpoint()));
```

(`SessionCheckpoint` keeps its `#[derive(Clone)]` — public type, harmless.) `checkpointed_seek` is the ONLY caller of `ReplaySession::from_checkpoint`; if the compiler surfaces another, fix it mechanically to pass a reference and note it in your report.

- [ ] **Step 4: Run the test file.** `cargo test -p retrace --test checkpoint_seek -- --test-threads=1` — Expected: PASS (5 tests: the 4 existing M4 tests prove the `Rc` refactor changed no behavior; the new gate-0 test proves the guard).

- [ ] **Step 5: Full gate.** `just gate` — Expected: **105 / 0 / 0**, clippy clean.

- [ ] **Step 6: Commit.**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace/tests/checkpoint_seek.rs
git commit -m "M4-ff t1: Rc checkpoint storage (no deep-clone on hit) + same-key used_bytes guard

Co-Authored-By: <executing model> <noreply@anthropic.com>"
```

---

### Task 2: window-length memoization — `CheckpointCache::window_len` + `Exec` wiring + README

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` — `CheckpointCache` struct/`new` (two fields) + two new methods.
- Modify: `crates/retrace/src/debug.rs` — `Exec::probe_window_len` body (lines ~164-170).
- Modify: `README.md` — the M4 Deferred paragraph (lines ~909-914).
- Test: `crates/retrace/tests/checkpoint_seek.rs` (append one test).

**Interfaces:**
- Consumes: `checkpointed_seek`, `ReplaySession::window_len_here` (existing, unchanged).
- Produces: `CheckpointCache::window_len(&mut self, trace_path: &Path, n: usize) -> Result<u64, String>` and `CheckpointCache::window_probe_steps(&self) -> u64`.

- [ ] **Step 1: Write the failing test.** Append to `crates/retrace/tests/checkpoint_seek.rs`:

```rust
#[test]
fn window_len_is_memoized_per_landmark() {
    let (rec, trace) = util::record(retrace_guest::SPINLOOP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let trace = Path::new(&trace);
    let mut cache = retrace_core::CheckpointCache::new(256 * 1024 * 1024, 64);
    // Landmark 2 = spinloop's ~4003-insn loop2 window: the expensive discovery this memo exists for.
    let len1 = cache.window_len(trace, 2).unwrap();
    assert!(len1 > 3000, "landmark 2 should be the ~4003-insn window, got {len1}");
    let probed_once = cache.window_probe_steps();
    assert_eq!(probed_once, len1, "first call must pay exactly one full-window probe");
    let len2 = cache.window_len(trace, 2).unwrap();
    assert_eq!(len2, len1);
    assert_eq!(cache.window_probe_steps(), probed_once,
        "second call must be a memo hit — zero additional probe steps");
}
```

- [ ] **Step 2: Verify failure.** `cargo test -p retrace --test checkpoint_seek window_len_is_memoized -- --test-threads=1` — Expected: FAIL to compile (`no method named window_len`).

- [ ] **Step 3: Implement.** In `crates/retrace-core/src/lib.rs`:

(a) Add two fields to `CheckpointCache` (after `total_single_steps: u64,`):

```rust
    window_lens: std::collections::BTreeMap<usize, u64>, // landmark N -> window length (fixed per trace)
    window_probe_steps: u64,
```

and initialize both in `new` (append to the struct literal):

```rust
                          window_lens: std::collections::BTreeMap::new(), window_probe_steps: 0 }
```

(b) Add two methods inside `impl CheckpointCache` (after `used_bytes`):

```rust
    /// Window length of landmark `n`, memoized: a window's length is a fixed, deterministic
    /// property of (trace, landmark), so it is measured at most once per cache lifetime. The
    /// measuring probe seeks via `checkpointed_seek` (benefiting from, and feeding, the position
    /// cache) and then single-steps the full window once. The caller must hold NO live
    /// `ReplaySession` (one VM per process). `window_probe_steps` counts the steps paid by these
    /// probes — the deterministic proxy the tests use; deliberately separate from
    /// `total_single_steps`, which counts only position-seek steps.
    pub fn window_len(&mut self, trace_path: &Path, n: usize) -> Result<u64, String> {
        if let Some(&len) = self.window_lens.get(&n) { return Ok(len); }
        let mut probe = checkpointed_seek(trace_path, self, n, 0)?;
        let len = probe.window_len_here()?;
        drop(probe); // free the VM before returning control to a caller that will open a session
        self.window_probe_steps += len;
        self.window_lens.insert(n, len);
        Ok(len)
    }

    /// Total single-steps ever paid by `window_len`'s discovery probes against this cache.
    pub fn window_probe_steps(&self) -> u64 { self.window_probe_steps }
```

(c) In `crates/retrace/src/debug.rs`, replace `probe_window_len` (lines ~164-170):

```rust
    /// Window length of landmark `n`, memoized in the checkpoint cache (measured on a transient
    /// probe session at most once per landmark per debug session). Drops the live session first —
    /// even a memo hit re-establishes it cheaply via the position cache; the caller re-seeks via
    /// `reseek`.
    fn probe_window_len(&mut self, n: usize) -> Result<u64, String> {
        self.session = None; // free the live VM before any probe (one VM per process)
        self.cache.window_len(self.trace, n)
    }
```

- [ ] **Step 4: Run the test file.** `cargo test -p retrace --test checkpoint_seek -- --test-threads=1` — Expected: PASS (6 tests).

- [ ] **Step 5: Full gate — the transcript-invisibility proof.** `just gate` — Expected: **106 / 0 / 0**, clippy clean. `debug_cli.rs` (7 tests) and `reverse_debug_e2e.rs` (1 test) must pass UNMODIFIED — if either fails, the bug is in the wiring, not the tests; do not touch them.

- [ ] **Step 6: Update the README Deferred paragraph.** In `README.md`, replace the paragraph at lines ~909-914:

```
**Deferred:** window-length memoization — `probe_window_len`/`window_len_here` still single-step a full window
from `K = 0` on every call, so a `reverse-stepi` that crosses a landmark boundary into a large window still pays
that window's full length on every crossing; `checkpointed_seek` accelerates *position* seeks only, not window-
length discovery. A user-facing config knob for the byte budget / cost-gate threshold (currently compile-time
constants). Persisting checkpoints across sessions — deliberately never: a checkpoint's validity is scoped to one
trace and one session by construction, so there is no cross-session use for one to serve.
```

with:

```
**Deferred:** a user-facing config knob for the byte budget / cost-gate threshold (currently compile-time
constants). Persisting checkpoints across sessions — deliberately never: a checkpoint's validity is scoped to one
trace and one session by construction, so there is no cross-session use for one to serve. (Window-length
memoization, deferred at M4 close, landed in the M4 fast-follow: `CheckpointCache::window_len` measures each
window at most once per debug session, so a `reverse-stepi` crossing a landmark boundary into a large window pays
that window's length once, not on every crossing; `window_len_here` itself is unchanged.)
```

- [ ] **Step 7: Commit.**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace/src/debug.rs crates/retrace/tests/checkpoint_seek.rs README.md
git commit -m "M4-ff t2: memoize window lengths in CheckpointCache — reverse-stepi pays a window once per session

Co-Authored-By: <executing model> <noreply@anthropic.com>"
```

---

## Notes for the executor

- **The one thing not to get wrong (Task 1):** the same-key guard must subtract `old.approx_bytes()` BEFORE `self.used_bytes += bytes;` runs — the insert-returns-`Some(old)` shape above does this naturally. Do not reorder it after the `+=`.
- **The one thing not to get wrong (Task 2):** `window_len`'s probe session must be dropped before the method returns (the explicit `drop(probe)` above) — the CLI caller immediately opens a new session via `reseek`, and two live sessions = `HV_BUSY`.
- **Sequential VMs, always** — the new tests follow the established `let _ = checkpointed_seek(...)` immediate-drop shape; preserve it if you restructure.
- **`Rc`, never `Arc`, never a raw clone:** if the borrow checker fights the `Rc` refactor, the answer is a smaller reborrow scope, not cloning the `SessionCheckpoint` — that would silently reintroduce the copy this task exists to remove.
