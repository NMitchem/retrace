# M31-checkpointparity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `Box_::from_checkpoint` a standing *structural* parity guard — one test that diffs every observable field against the live box it was captured from — plus the written obligation that makes the next field somebody's problem before it ships.

**Architecture:** Mirror `crates/retrace-box/tests/restoreparity.rs`. Drive a guest to a mid-run point, stage non-default state through `Box_`'s own public methods, capture the live box's observable state, `checkpoint()`, `drop()` (HVF allows one VM per process), rebuild via `from_checkpoint`, and diff. The one field group that legitimately differs is asserted as deliberately reset, with its mechanism cited by file and line, rather than stripped out of the comparison.

**Tech Stack:** Rust 1.95.0, `aarch64-apple-darwin`, Hypervisor.framework. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-08-retrace-m31-checkpointparity-design.md`

## Global Constraints

- **`--test-threads=1` is mandatory.** HVF allows one VM per process. Every command below includes it.
- **One live `Box_` at a time.** `drop(b)` before constructing the next box, or `hv_vm_create` fails with `HV_BUSY`. Capture everything you need from the live box *before* the drop.
- **`clippy.toml` denials are load-bearing:** no `Instant::now`/`SystemTime::now`, no `std::thread::Thread`. The gate runs `-D warnings`.
- **Do not delete or duplicate the existing point tests.** `pacposture.rs`, `sigcheckpoint.rs`, `protnone.rs`, `tlbi.rs`, `threads.rs` and `checkpoint.rs` each cover one field on this path. The structural guard compares those fields as part of comparing everything; it does not replace them.
- **Baseline gate:** M30 closed at **549 passed / 0 failed / 2 ignored over 118 binaries**, clippy clean.
- Every commit message ends with:
  ```
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01GmzH15YsiDjpYoCvWWQfi5
  ```

## Facts established during planning (do not re-derive)

These were verified against the tree at `f760dbb`. Trust them; re-checking is fine, re-deriving is waste.

- **Already-public accessors** (no new code needed): `fds()`, `sigtable()`, `threads()`, `thread_start_pc()`, `wq_thread_pc()`, `pthread_size()`, `noaccess()`, `stack_top()`, `stack_size()`, `tpidr_el0()`, `tpidrro_el0()`, `fall_throughs()`, `read_guest()`, `dbg_backings()`, `dbg_next_l3()`, `dbg_internal_state()`, `dbg_regs()`, `dbg_fp_regs()`.
- **The only missing accessor** is for the four debugger fields: `bps_armed`, `wps_armed`, `watch_ranges`, `syscall_watch_hit` (`crates/retrace-box/src/lib.rs:505-508`). Task 1 adds it.
- **Public staging APIs:** `guest_vm_reserve`, `commit_reserved_page`, `guest_mmap`, `install_cache_pager`, `mint_bootstrap_port`, `protect_none`, `arm_hw_watchpoint`, `arm_hw_breakpoint`, `fds_mut`, `sigtable_mut`, `threads_mut`, `set_thread_start_pc`, `step`, `run`.
- **Derive facts:** `FdSlot` and `ThreadCtx` derive `PartialEq`. `FdTable`, `SigTable` and `ThreadTable` derive only `Clone`/`Debug` — so compare **projections** (`fds().slots()` returns `Vec<FdSlot>`; `format!("{:?}", ...)` for the tables). Do **not** add `PartialEq` to production types.
- **The debugger four are legitimately reset.** `crates/retrace/src/debug.rs:608`, `:641` and `:761` call `ReplaySession::arm_watchpoints` from the debugger's *own* stored watch list after a seek, so the box is not the authority for them. The guard asserts that reset explicitly, with that citation — it is not a bug.
- `Box_::from_checkpoint` is at `crates/retrace-box/src/lib.rs:5162`; it re-installs the cache on its last line via `if state.cache_installed { b.install_cache_pager(); }`, so `cache_installed` must compare **equal** as ordinary coverage.

---

### Task 1: The one missing accessor — `dbg_debug_state()`

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (add method near `dbg_next_l3` at :5316)
- Test: `crates/retrace-box/tests/checkpointparity.rs` (create)

**Interfaces:**
- Produces: `pub fn dbg_debug_state(&self) -> String` on `Box_` — used by Tasks 2 and 3.

- [ ] **Step 1: Write the failing test**

Create `crates/retrace-box/tests/checkpointparity.rs`:

```rust
// M31-checkpointparity. `from_checkpoint` is the replay-side construction path that restores the
// most state and runs mid-run, where nothing sits at a default. Each field it has ever dropped got
// a point test written after its own bug (pacposture.rs, sigcheckpoint.rs, protnone.rs, tlbi.rs,
// threads.rs); what none of them provide is a forcing function for the NEXT field. This file is
// that: one structural diff, plus an obligation.
use retrace_box::Box_;
use retrace_guest::{parse_macho, HELLO};

/// The four debugger fields are the only `Box_` state with no accessor at all, and the guard cannot
/// honestly assert a field is reset unless a test can observe it being reset.
#[test]
fn the_debug_state_accessor_reports_armed_watchpoints() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let clean = b.dbg_debug_state();
    assert!(clean.contains("bps_armed=false"), "precondition: nothing armed yet, got {clean}");
    assert!(clean.contains("wps_armed=false"), "precondition: nothing armed yet, got {clean}");

    b.arm_hw_watchpoint(0, b.stack_top() - 0x100, 8);
    let armed = b.dbg_debug_state();
    assert!(armed.contains("wps_armed=true"), "arming must be observable, got {armed}");
    assert!(armed.contains("watch_ranges=[("), "the ranges must be observable, got {armed}");
}
```

- [ ] **Step 2: Run it and confirm it fails**

```sh
cargo test -p retrace-box --test checkpointparity -- --test-threads=1
```

Expected: FAIL — `no method named 'dbg_debug_state' found`.

- [ ] **Step 3: Add the accessor**

In `crates/retrace-box/src/lib.rs`, immediately after `dbg_next_l3` (:5316):

```rust
    /// Test-only (M31): the four debugger fields, which are the only `Box_` state no accessor
    /// reaches. `tests/checkpointparity.rs` needs them to *observe* that `from_checkpoint` resets
    /// them — an assertion about a field nothing can read is an assertion about nothing. Kept out
    /// of `dbg_internal_state` deliberately: that string is compared by
    /// `restoreparity.rs` too, and adding fields to it changes an existing contract.
    #[doc(hidden)]
    pub fn dbg_debug_state(&self) -> String {
        format!("bps_armed={} wps_armed={} watch_ranges={:?} syscall_watch_hit={:?}",
            self.bps_armed, self.wps_armed, self.watch_ranges, self.syscall_watch_hit)
    }
```

- [ ] **Step 4: Run the test and confirm it passes**

```sh
cargo test -p retrace-box --test checkpointparity -- --test-threads=1
```

Expected: PASS, 1 test.

- [ ] **Step 5: Commit**

```sh
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/checkpointparity.rs
git commit -m "M31-checkpointparity t1: an accessor for the only state nothing can observe

An assertion about a field nothing can read is an assertion about nothing, so the
four debugger fields get an accessor before the guard that asserts on them.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GmzH15YsiDjpYoCvWWQfi5"
```

---

### Task 2: The structural guard, on a static mid-run fixture

**Files:**
- Modify: `crates/retrace-box/tests/checkpointparity.rs`

**Interfaces:**
- Consumes: `Box_::dbg_debug_state()` from Task 1.
- Produces: `fn assert_checkpoint_parity(b: Box_, label: &str)`, `fn assert_debug_state_is_deliberately_reset(r: &Box_, label: &str)` and `fn diff_backings(live: &[(u64, usize)], restored: &[(u64, usize)]) -> String`. Task 3 calls `assert_checkpoint_parity`.

- [ ] **Step 1: Write the failing test**

Append to `crates/retrace-box/tests/checkpointparity.rs`:

```rust
/// The entries that differ between two SORTED `(ipa, len)` backing lists, tagged with the side they
/// appear on and rendered in hex — empty when the two maps are identical. A two-pointer merge, so it
/// is multiplicity-exact: `[a, a, b]` against `[a, b, b]` has an empty set difference and is still
/// not the same map. (Same shape as `restoreparity.rs`'s helper, kept local because Rust integration
/// tests are separate binaries and sharing would mean a `util` module both must import.)
fn diff_backings(live: &[(u64, usize)], restored: &[(u64, usize)]) -> String {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    let show = |side, e: &(u64, usize)| format!("{side}-only (ipa {:#x}, len {:#x})", e.0, e.1);
    while i < live.len() || j < restored.len() {
        match (live.get(i), restored.get(j)) {
            (Some(l), Some(r)) if l == r => { i += 1; j += 1; }
            (Some(l), Some(r)) if l < r => { out.push(show("live", l)); i += 1; }
            (Some(_), Some(r)) => { out.push(show("restored", r)); j += 1; }
            (Some(l), None) => { out.push(show("live", l)); i += 1; }
            (None, Some(r)) => { out.push(show("restored", r)); j += 1; }
            (None, None) => unreachable!("loop condition guarantees one side still has entries"),
        }
    }
    out.join(", ")
}

/// The ONE field group that legitimately differs across `from_checkpoint` — asserted, not excused.
///
/// The four debugger fields (`bps_armed`, `wps_armed`, `watch_ranges`, `syscall_watch_hit`) are not
/// carried in `BoxState` and come back at their defaults. That is correct: the DEBUGGER owns the
/// watch list, not the box, and re-arms from its own stored copy after every seek — see
/// `crates/retrace/src/debug.rs:608`, `:641` and `:761`, each calling
/// `ReplaySession::arm_watchpoints(&ws)` where `ws` is the debugger's list. A box that restored them
/// would be a second authority for the same state.
///
/// This is deliberately a POSITIVE assertion rather than a `normalise()` that strips the field out
/// of the comparison. Stripping excuses a difference invisibly and passes just as well when the
/// field stops being reset; asserting the reset states what is supposed to happen and fails if it
/// stops happening. The rich tier arms a watchpoint before capture precisely so this assertion has
/// something to observe.
///
/// `cache_installed` is deliberately NOT treated this way: `from_checkpoint` re-installs the pager
/// on its last line (`crates/retrace-box/src/lib.rs`, `if state.cache_installed {
/// b.install_cache_pager(); }`), so it must compare EQUAL as ordinary coverage.
fn assert_debug_state_is_deliberately_reset(r: &Box_, label: &str) {
    assert_eq!(r.dbg_debug_state(),
        "bps_armed=false wps_armed=false watch_ranges=[] syscall_watch_hit=None",
        "{label}: the debugger four must come back at their defaults — the debugger re-arms from \
         its own list (crates/retrace/src/debug.rs:608, :641, :761). If this now differs, either \
         from_checkpoint started carrying them (delete this assertion and compare them instead) or \
         a new field joined the group (add it here).");
}

/// The structural guard.
///
/// **OBLIGATION when you add a field to `Box_`, or any `from_checkpoint`-time write:** it must be
/// either (a) compared here and EQUAL, or (b) asserted above as deliberately reset, with the
/// mechanism that re-establishes it on the replay side cited by file and line. There is no third
/// option that is safe. This path has dropped a field at least five times, each caught only after it shipped.
///
/// **What this deliberately does NOT do**, so it is not mistaken for more than it is:
/// it compares CONSTRUCTION at one landmark, not evolution afterwards
/// (`crates/retrace/tests/checkpoint_seek.rs` is that axis); and two boxes wrong in the SAME way are
/// invisible to any test that only diffs them against each other.
fn assert_checkpoint_parity(b: Box_, label: &str) {
    let live_internal = b.dbg_internal_state();
    let (top, size) = (b.stack_top(), b.stack_size());
    let (tp, tpro) = (b.tpidr_el0(), b.tpidrro_el0());
    let threads = format!("{:?}", b.threads());
    let nthreads = b.threads().len();
    let fds = b.fds().slots();
    let sigtable = format!("{:?}", b.sigtable());
    let start_pc = b.thread_start_pc();
    let wq_pc = b.wq_thread_pc();
    let psize = b.pthread_size();
    let noaccess = b.noaccess().to_vec();
    let fts = b.fall_throughs();
    let mut live_backings = b.dbg_backings();
    live_backings.sort_unstable();
    let next_l3 = b.dbg_next_l3();

    let state = b.checkpoint();
    drop(b); // one VM per process (HVF)
    let r = Box_::from_checkpoint(&state);

    assert_eq!(r.dbg_internal_state(), live_internal, "{label}: internal bookkeeping");
    assert_debug_state_is_deliberately_reset(&r, label);
    assert_eq!((r.stack_top(), r.stack_size()), (top, size), "{label}: stack geometry");
    assert_eq!((r.tpidr_el0(), r.tpidrro_el0()), (tp, tpro), "{label}: thread-pointer sysregs");
    assert_eq!(format!("{:?}", r.threads()), threads, "{label}: thread table");
    assert_eq!(r.threads().len(), nthreads, "{label}: thread count");
    assert_eq!(r.fds().slots(), fds, "{label}: guest-visible fd slots");
    assert_eq!(format!("{:?}", r.sigtable()), sigtable, "{label}: signal dispositions");
    assert_eq!(r.thread_start_pc(), start_pc, "{label}: bsdthread_register start pc");
    assert_eq!(r.wq_thread_pc(), wq_pc, "{label}: workqueue thread pc");
    assert_eq!(r.pthread_size(), psize, "{label}: pthread struct size");
    assert_eq!(r.noaccess(), noaccess.as_slice(), "{label}: PROT_NONE map");
    assert_eq!(r.fall_throughs(), fts, "{label}: fall-through counter");
    let mut restored_backings = r.dbg_backings();
    restored_backings.sort_unstable();
    assert_eq!(restored_backings.len(), live_backings.len(), "{label}: backing count");
    let diff = diff_backings(&live_backings, &restored_backings);
    assert!(diff.is_empty(), "{label}: live and restored disagree on the memory map: {diff}");
    assert_eq!(r.dbg_next_l3(), next_l3,
        "{label}: next free L3 table IPA — restored derived {:#x}, live had {next_l3:#x}",
        r.dbg_next_l3());
}

/// The static tier. Deliberately mid-run, not landmark 0: at landmark 0 a defaulted field and a
/// correctly-restored one are indistinguishable, which is the whole reason `checkpoint.rs` runs
/// mid-run too.
#[test]
fn a_checkpointed_static_box_matches_the_box_it_came_from() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let _ = b.run(); // reach the first syscall, so this is genuinely mid-run
    assert_checkpoint_parity(b, "static");
}
```

- [ ] **Step 2: Run it**

```sh
cargo test -p retrace-box --test checkpointparity -- --test-threads=1
```

Expected: it either PASSES (the static tier has no asymmetry — likely, since most fields are at defaults here) or FAILS naming one field. **If it fails, do not fix it in this task** — record the exact assertion message in the task report; Task 5 owns fixes.

- [ ] **Step 3: Commit**

```sh
git add crates/retrace-box/tests/checkpointparity.rs
git commit -m "M31-checkpointparity t2: the structural diff, and the obligation that outlives it

One test that compares every observable field across from_checkpoint, so field
N+1 is caught by a test rather than by a reviewer. The four debugger fields are
asserted as deliberately reset, with their mechanism cited by file and line: the
debugger owns the watch list and re-arms from its own copy after a seek, so a box
that restored them would be a second authority for the same state. Asserted
rather than stripped, because a strip passes just as well once the reset stops.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GmzH15YsiDjpYoCvWWQfi5"
```

---

### Task 3: The rich fixture — reach, with non-default preconditions

**Files:**
- Modify: `crates/retrace-box/tests/checkpointparity.rs`

**Interfaces:**
- Consumes: `assert_checkpoint_parity` from Task 2.

- [ ] **Step 1: Write the failing test**

Append:

```rust
/// The rich tier, and the reason this milestone is not just Task 2.
///
/// A field left at its default is compared, and the comparison proves nothing: `Default ==
/// Default` passes for a reason unrelated to the assertion's name. `restoreparity.rs` names that
/// trap explicitly. So this stages a NON-DEFAULT value into every field the structural diff reaches
/// that the static tier leaves empty, and asserts each one is non-default BEFORE capturing — the
/// preconditions are what separate a guard that agrees from a guard that cannot see.
///
/// State is staged through `Box_`'s own public methods rather than by running a real threaded guest:
/// a guest that spawns threads needs dyld and libpthread, and `retrace-box` cannot depend on
/// `retrace-core`. `tests/threads.rs` established this pattern (it builds thread contexts by hand on
/// a static box for exactly the same reason).
#[test]
fn a_checkpointed_box_with_rich_state_matches_the_box_it_came_from() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let _ = b.run(); // mid-run

    // fds: one open, one closed — Closed must stay distinguishable from Free across the restore.
    let open_fd = b.fds_mut().alloc();
    let closed_fd = b.fds_mut().alloc();
    b.fds_mut().close(closed_fd);

    // sigtable: a non-default disposition.
    b.sigtable_mut().set_action(6, retrace_box::SigAction {
        disp: retrace_box::Disposition::Ign, tramp: 0, mask: 0xf, flags: 0x2 });

    // per-thread signal state (carried wholesale inside `threads`).
    b.threads_mut().set_mask_of(0, retrace_arch::SIG_SETMASK, 0b1010);

    // the three pthread/workqueue scalars.
    b.set_thread_start_pc(0x1234_5000);

    // noaccess: a real PROT_NONE extent over backed memory (protect_none asserts on unbacked).
    let base = b.guest_vm_reserve(0, 0x10000, true);
    assert!(b.commit_reserved_page(base), "precondition: the reservation must commit");
    b.protect_none(base, 0x4000);

    // an ordinary anon mmap, to move mmap_next off its initial value.
    let _mapped = b.guest_mmap(0, 0x4000, 3, 0x1002);

    // A watchpoint, ARMED — so `assert_debug_state_is_deliberately_reset` is observing a real reset
    // rather than a field that was already at its default. Without this the reset assertion would
    // pass on a box that never armed anything, which proves nothing about from_checkpoint.
    b.arm_hw_watchpoint(0, base, 8);

    // PRECONDITIONS. Without these the comparison below is Default == Default.
    assert_eq!(b.fds().slots()[open_fd as usize], retrace_box::FdSlot::Open,
        "precondition: an OPEN guest fd");
    assert_eq!(b.fds().slots()[closed_fd as usize], retrace_box::FdSlot::Closed,
        "precondition: a CLOSED guest fd, distinct from Free");
    assert_ne!(format!("{:?}", b.sigtable()), format!("{:?}", retrace_box::SigTable::default()),
        "precondition: a non-default disposition table");
    assert_eq!(b.threads().mask_of(0), 0b1010, "precondition: a non-default blocked mask");
    assert_eq!(b.thread_start_pc(), Some(0x1234_5000), "precondition: bsdthread_register seen");
    assert_eq!(b.noaccess(), &[(base, 0x4000)], "precondition: a non-empty PROT_NONE map");
    assert!(b.dbg_debug_state().contains("wps_armed=true"),
        "precondition: a watchpoint is ARMED, so the reset assertion has something to observe");

    assert_checkpoint_parity(b, "rich");
}
```

- [ ] **Step 2: Run it**

```sh
cargo test -p retrace-box --test checkpointparity -- --test-threads=1
```

Expected: PASS, or FAIL naming one field. **Do not fix a failure here** — record the exact message; Task 5 owns fixes. If a *precondition* fails, that is this task's bug: the staging call was wrong, fix it and re-run.

- [ ] **Step 3: Commit**

```sh
git add crates/retrace-box/tests/checkpointparity.rs
git commit -m "M31-checkpointparity t3: stage the state, then assert it is really there

A field at its default is compared and proves nothing. Every field the structural
diff reaches is staged non-default and asserted non-default before capture, so
the guard passes because the state agrees rather than because it cannot see.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GmzH15YsiDjpYoCvWWQfi5"
```

---

### Task 4: The positive control — prove the guard can fire

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (temporarily, then revert)
- Modify: `crates/retrace-box/tests/checkpointparity.rs` (record the result in a comment)

A guard nobody proved can fire is not yet an instrument. M28's `let band = 0;` mutation passed the entire 523-test gate unnoticed before its positive control existed.

- [ ] **Step 1: Mutate one genuinely restored field**

In `Box_::from_checkpoint` (`crates/retrace-box/src/lib.rs:5162`), change:

```rust
            sigtable: state.sigtable.clone(),
```

to:

```rust
            sigtable: SigTable::default(), // MUTATION — Task 4 positive control, revert me
```

- [ ] **Step 2: Run the guard and confirm it goes RED**

```sh
cargo test -p retrace-box --test checkpointparity -- --test-threads=1
```

Expected: `a_checkpointed_box_with_rich_state_matches_the_box_it_came_from` FAILS with `rich: signal dispositions`. Copy the exact failure line.

If it PASSES, the guard does not reach `sigtable` and Task 3 is wrong — stop and report that, do not proceed.

- [ ] **Step 3: Revert the mutation**

```sh
git checkout -- crates/retrace-box/src/lib.rs
git diff --stat   # must be empty for lib.rs
grep -rn 'MUTATION' crates/   # must print nothing
```

> An unreverted mutation left in a worktree is not hypothetical: M30's branch was found carrying exactly that, and gating it would have gated a mutant. Verify both commands above before committing.

- [ ] **Step 4: Record the measurement in the test file**

Add above `assert_checkpoint_parity`:

```rust
/// **Proven able to fire (M31 t4).** Replacing `sigtable: state.sigtable.clone()` with
/// `SigTable::default()` in `from_checkpoint` turns
/// `a_checkpointed_box_with_rich_state_matches_the_box_it_came_from` RED at `rich: signal
/// dispositions`. Recorded because a guard nobody has watched fail is a guard nobody knows is wired
/// up — M28's `let band = 0;` passed a 523-test gate before its own positive control existed.
```

- [ ] **Step 5: Commit**

```sh
git add crates/retrace-box/tests/checkpointparity.rs
git commit -m "M31-checkpointparity t4: watch the guard fail, then write down what it took

Positive control: resetting sigtable in from_checkpoint turns the rich-tier test
red at 'rich: signal dispositions'. Mutation reverted and verified absent.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GmzH15YsiDjpYoCvWWQfi5"
```

---

### Task 5: Fix what the guard caught, park what it cannot reach

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (only if Tasks 2/3 recorded a failure)
- Modify: `crates/retrace-box/tests/checkpointparity.rs`

**Decision procedure — apply it literally, do not improvise:**

For each divergence recorded in Tasks 2 and 3:

1. **Can a real guest reach this state?** (Does any code path outside a test set the field before a checkpoint is taken?) If **yes** → it is a bug. Carry the field in `BoxState` if it is not carried, restore it in `from_checkpoint`, and add a named regression test in the shape of `sigcheckpoint.rs`. Then re-run the guard.
2. If **no** → it is not reachable today. Do **not** fix blind. Add it to the owed list in Task 6's status-log section, naming the fixture that would be required.
3. If a divergence is **legitimate** (a mechanism re-establishes it on the replay side), assert it as a deliberate reset in the shape of `assert_debug_state_is_deliberately_reset`, **with that mechanism cited by file and line**. Never silently exclude the field from the comparison, and never add an excuse with no citation.

- [ ] **Step 1: List every divergence from the Task 2 and Task 3 reports**

If both tasks passed with no divergence, write that down explicitly and skip to Step 4. A clean first run is a real result — the structural guard and its obligation are the deliverable, not a bug count.

- [ ] **Step 2: Apply the decision procedure to each**

- [ ] **Step 3: Re-run the full file**

```sh
cargo test -p retrace-box --test checkpointparity -- --test-threads=1
```

Expected: all tests PASS.

- [ ] **Step 4: Reconcile the enumeration inconsistency**

Three places count this path's instances and none agree:
- `crates/retrace-box/tests/restoreparity.rs` — "bitten five times (M9 t3, M10, M11, M14, M18)"
- the `BoxState` field comments — number themselves 1..5 as M7 t6, M8, M10, M11, M23 t1
- `from_checkpoint`'s `TPIDRRO_EL0` comment — "the fourth field here … (M9 t3, M10, M11)"

Read the cited milestones' status-log sections, determine which list means what (whole-class vs. this-path), and correct the wrong ones **in place**, adding no fourth enumeration. If the evidence does not settle it, say so in the status log rather than picking one.

- [ ] **Step 5: Commit**

```sh
git add -A crates/
git commit -m "M31-checkpointparity t5: close what is reachable, name what is not

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GmzH15YsiDjpYoCvWWQfi5"
```

---

### Task 6: The gate and the two documents

**Files:**
- Modify: `README.md` ("Known limits" — the `from_checkpoint` paragraph near line 633)
- Modify: `docs/status-log.md` (append a new `## Status: M31-checkpointparity` section)

- [ ] **Step 1: Derive the expected test count from source BEFORE running the gate**

```sh
git ls-tree -r --name-only HEAD | grep '\.rs$' | while IFS= read -r f; do
  n=$(git show "HEAD:$f" | grep -cE '^\s*#\[(test|tokio::test)\]'); [ "$n" -gt 0 ] && echo "$n $f"
done | awk '{s+=$1} END {print "total #[test]:", s}'
```

M30's baseline was 551 total (549 passed + 2 ignored). The delta must equal the number of `#[test]` functions this milestone added. Write the prediction down **before** the gate runs.

- [ ] **Step 2: Run the gate, chunked**

The full workspace run exceeds the 10-minute tool ceiling. Run these sequentially, in the background, capturing each exit code **before any pipe**:

```sh
cargo test --workspace --exclude retrace-box --exclude retrace -- --test-threads=1
cargo test -p retrace-box -- --test-threads=1          # WHOLE PACKAGE: keeps Doc-tests
cargo test -p retrace --bins -- --test-threads=1       # never omit: 11 unit tests live only here
# e2e: split the 59 targets in crates/retrace/tests/ into groups of 11, index-free:
#   ls crates/retrace/tests/*.rs | xargs -n1 basename | sed 's/\.rs$//' | sort > targets.txt
#   xargs -n11 < targets.txt > groups.txt
#   then one `cargo test -p retrace --test A --test B ...` per line
cargo clippy --workspace --all-targets -- -D warnings
```

Chunking pitfalls, all of which have bitten this repo:
- Do **not** chunk with shell array indexing — the tool's shell is zsh (1-indexed), a `#!/bin/bash` script is 0-indexed, and the two produce different group membership. Use `xargs -n11`. Assert the flattened groups equal the target list before running.
- Do **not** split `retrace-box` per-target; that drops its `Doc-tests` binary silently.
- Parse the log with `LC_ALL=C` and ANSI stripped — the gate log's UTF-8 kills plain `grep` *and* `awk`.

- [ ] **Step 3: Reconcile**

Compare the measured total against Step 1's prediction. They must match. If they do not, the chunking is wrong before the code is.

- [ ] **Step 4: Edit the README in place**

In "Known limits", the paragraph beginning "**Record-only box state is guarded on one replay path and not the other.**" (~:633) currently ends by naming `from_checkpoint` as "the successor milestone". Rewrite it to say what is now true: both replay paths carry a structural parity guard with the same obligation; state the two blind spots that remain (construction not evolution; two boxes wrong the same way); and state what the guard does not reach.

- [ ] **Step 5: Append to `docs/status-log.md`**

Append-only — never rewrite an existing section. Include: what was built, the corrected premise (that `checkpoint.rs` and five point tests already existed, and the gap was a forcing function rather than coverage), the positive-control measurement from Task 4, the enumeration reconciliation from Task 5, the gate figure, and a "What stays owed" list.

- [ ] **Step 6: Commit**

```sh
git add README.md docs/status-log.md
git commit -m "M31-checkpointparity t6: the gate, and both documents

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GmzH15YsiDjpYoCvWWQfi5"
```
