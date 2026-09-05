# M21-stackgrow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a deep Rust recursion strikes libstd's guard page and reports `has overflowed its stack`,
instead of running off the 256 KiB backed stack into unbacked IPA and killing the recorder.

**Architecture:** Reserve the guest's believed-but-unbacked main-thread stack window
`[0x2008000, 0x27C0000)` at `load_dynamic`, and let the **existing** `commit_reserved_page` grow into
it one zeroed page per stage-2 fault. No new fault-path code, no new dispatch arm, no trace-format
change — this is M18's `place_worker_stack` pattern applied to the one stack that never received it.
The reservation stops one granule **above** the guard page so the guard keeps taking a stage-1
permission fault (M13's route) rather than being silently committed.

**Tech Stack:** Rust 1.95.0, `aarch64-apple-darwin`, Hypervisor.framework, macOS 26.x Apple Silicon.

**Spec:** `docs/superpowers/specs/2026-08-29-retrace-m21-stackgrow-design.md`

## Global Constraints

- **`--test-threads=1` is mandatory** on every `cargo test` invocation. HVF allows one VM per
  process; a bare `cargo test` flakes with `HV_BUSY`.
- **`just gate` does not complete as one command.** Chunk it, run each chunk `--no-fail-fast`, and
  capture cargo's exit code **before any pipe**. **Do not omit the `--bins` chunk.** Never use
  `cargo test -p retrace --lib` — there is no lib target and the whole invocation fails.
- **Grep gate logs with `grep -a`** — they carry ANSI and UTF-8 that trips plain grep.
- **Never reorder `Box_`'s struct fields.** `vcpu` must stay declared before `vm`.
- **Do not touch** `commit_reserved_page`, `protect_none`, or the record/replay fault dispatch. M21
  adds a reservation; it must not relax the strict gate.
- **No trace-format change.** No `Event` variant, no `TRACE_MAGIC` bump. The `verify_thread`
  call-site count stays **seven** — assert this in review rather than assuming it.
- Determinism: nothing nondeterministic may enter the trace. `clippy.toml` bans `Instant::now`,
  `SystemTime::now`, and `std::thread::Thread` in retrace's own code.
- Fixed geometry, verified against source: guard `0x2004000`, `GUARD_TOP` `0x2008000`,
  `DYN_STACK_BOTTOM` `0x27C0000`, window `0x7B8000` (7.72 MiB), `PT_L3_CEIL` `0x2000000`,
  `GRANULE` `0x4000`, libpthread's constant `0x7fc000`.

---

### Task 0: The four measurements the spec owes

**No production change.** This task exists because **T0-3 can invalidate the whole approach**, and
finding that out after Task 1 wastes Task 1. Take the measurements first, write them down, and stop
if T0-3 fires.

**Files:**
- Create: `docs/superpowers/specs/2026-08-29-retrace-m21-stackgrow-measurements.md`

**Interfaces:**
- Consumes: nothing.
- Produces: the measurements file, cited as **T0-1**…**T0-4** by later tasks and by the status-log
  section in Task 4. No code.

- [ ] **Step 1: T0-1 — does anything currently land in the believed-stack window?**

`range_is_free` treats reservations as occupied (M2-carveout), so reserving the window will make
ANYWHERE placement *avoid* a range it may use today. If something lands there now, every recording's
addresses shift.

Run a recording with trace logging and look for any mapping placed inside `[0x2008000, 0x27C0000)`:

```sh
RETRACE_TRACE=1 cargo run -p retrace -- record-dyn \
  "$(find target -name hello_dyn -type f | head -1)" -o /tmp/t0.bin 2>&1 \
  | tee /tmp/t0-1.log | grep -aiE 'mmap|vm_map|vm_alloc' | head -40
```

Record in the measurements file: every placement address observed, and explicitly whether any falls
in `[0x2008000, 0x27C0000)`. **Expected: none** — `MMAP_BASE` is 40 GiB and placement is hint-forward
first-fit.

- [ ] **Step 2: T0-2 — do any tests pin a reservation count or index?**

```sh
grep -rn "reservations" crates/ --include='*.rs' | grep -vE "^crates/retrace-box/src/lib.rs" 
grep -rn "reservations\(\)\|\.reservations\b" crates/ --include='*.rs' | grep -iE "len\(\)|\[0\]|\[1\]|count"
```

Record every hit and whether it pins a count/index (which one extra reservation would break) or only
membership (which it would not).

- [ ] **Step 3: T0-3 — the cost claim (LOAD-BEARING)**

The approach rests on "a reservation maps nothing, so guests that do not recurse pay nothing."
Measure `hello_rust` and one dyld-suite gate, three runs each, **before** any change:

```sh
for i in 1 2 3; do
  /usr/bin/time -p cargo test -p retrace --test hello_rust_e2e -- --test-threads=1 2>&1 | tail -3
done
for i in 1 2 3; do
  /usr/bin/time -p cargo test -p retrace --test hello_dyn_e2e -- --test-threads=1 2>&1 | tail -3
done
```

Write the numbers down now; Task 1 Step 8 repeats them after the change. **Prediction: no measurable
difference.** M8 measured eager 8 MiB backing at ~1.7x (**R3-d**) — if this change reproduces
anything like that, **stop and report**: the approach is invalidated and the milestone re-plans.

- [ ] **Step 4: T0-4 — where does the guest actually land today?**

```sh
cargo test -p retrace --test stackoverflow_rust_e2e -- --test-threads=1 --ignored --nocapture 2>&1 \
  | tee /tmp/t0-4.log | tail -30
```

Record the exact failure. **Expected (R3-c):** `data abort (EC=0x24 ISS=0x1c08047 FSC=0x7)
far/ipa=0x27bff60 (UNMAPPED)` — a stage-2 translation fault ~160 bytes below `0x27C0000`, nowhere
near the `0x2004000` guard. Note the FSC: `0x7` is a stage-2 translation fault, **not** `0x0f`
(permission). That distinction is the whole design.

- [ ] **Step 5: Write the measurements file**

Structure it like `docs/superpowers/specs/2026-08-27-retrace-m20-symbolops-measurements.md`: one
section per measurement, each headed **T0-N**, stating the command run, the raw output, and — in one
sentence — what it forecloses. Where a measurement contradicts the spec, say so plainly; the spec is
edited, not the measurement.

- [ ] **Step 6: Commit**

```bash
git add docs/superpowers/specs/2026-08-29-retrace-m21-stackgrow-measurements.md
git commit -m "M21-stackgrow t0: measure before reserving anything"
```

**Verification:** the file exists, all four measurements have raw output pasted in, and T0-3 shows no
pre-existing anomaly. If T0-1 found a placement inside the window, or T0-3's baseline cannot be
taken, **stop and report before Task 1.**

---

### Task 1: The reservation

**TDD. Write the failing test before the implementation.** This lands in the `-p retrace-box` chunk
and builds a real VM, so it needs `--test-threads=1`.

**Files:**
- Create: `crates/retrace-box/tests/stackgrow.rs`
- Modify: `crates/retrace-box/src/lib.rs` — constants near line 55; new method near
  `place_worker_stack` (line 3700); one call in `load_dynamic` after the `Box_` construction at
  line 1686.

**Interfaces:**
- Consumes: `Box_::load_dynamic(&exe, &dyld, &argv) -> Box_`, `Box_::commit_reserved_page(u64) -> bool`,
  `Box_::guest_vm_reserve(addr, size, anywhere) -> u64` (all already `pub`).
- Produces:
  - `pub const LIBPTHREAD_MAIN_STACK_SIZE: u64 = 0x7fc000;`
  - `pub fn believed_stack_window(&self) -> (u64, u64)` on `Box_` — returns `(GUARD_TOP, DYN_STACK_BOTTOM)`,
    i.e. `(start, end_exclusive)`. Tasks 2 and 3 use this; it exists because `DYN_STACK_TOP` /
    `DYN_STACK_SIZE` are deliberately private behind accessors (`stack_top()`, `stack_size()`) and
    an integration test cannot see them.

- [ ] **Step 1: Write the failing test**

Create `crates/retrace-box/tests/stackgrow.rs`. This mirrors the structure of
`workq_reqthreads_leaves_an_uncommittable_guard_page_below_each_worker_stack`
(`crates/retrace-box/tests/threads.rs:1241`) — **positive control first, refusals second** — for the
reason that test states: if `commit_reserved_page` refused every address here, the `assert!(!...)`
lines would pass while proving nothing.

```rust
// M21-stackgrow: the main thread's BELIEVED stack (what libpthread reports, 0x7fc000) is reserved
// but not mapped, so a deep recursion grows into it page-by-page instead of running off the 256 KiB
// backing into unbacked IPA and killing the recorder (M8 spec risk R3).
//
// The guard page libstd installs at DYN_STACK_TOP - 0x7fc000 must stay OUTSIDE the reservation. If
// it were inside, a stack overflow would take the stage-2 route and be SILENTLY COMMITTED by
// commit_reserved_page instead of faulting — converting the overflow into a corrupted, silently
// continuing guest. That is the one failure mode this design must never reach, so it is the first
// thing asserted.
use retrace_box::Box_;
use retrace_guest::{parse_macho, slice_arm64e, HELLO_DYN, DYLD_PATH};

const GRANULE: u64 = 0x4000;

fn dynbox() -> Box_ {
    let exe = parse_macho(&std::fs::read(HELLO_DYN).unwrap());
    let dyld = parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap()));
    Box_::load_dynamic(&exe, &dyld, &["hello_dyn".to_string()])
}

#[test]
fn the_believed_stack_window_is_reserved_and_the_guard_page_is_not() {
    let mut b = dynbox();
    let (start, end) = b.believed_stack_window();

    // Geometry, pinned against the derivation rather than restated from it.
    assert_eq!(end, b.stack_top() - b.stack_size(), "the window must end at the backed stack bottom");
    assert_eq!(start, b.stack_top() - retrace_box::LIBPTHREAD_MAIN_STACK_SIZE + GRANULE,
               "the window must start ONE GRANULE above libstd's guard page");
    assert!(start < end, "window must be non-empty: {start:#x}..{end:#x}");

    // NON-VACUITY FIRST, before anything is committed: pages inside the window must be committable,
    // or the refusals below would just mean "nothing here is reachable at all".
    assert!(b.commit_reserved_page(end - GRANULE),
        "the page immediately below the backed stack must demand-commit — this is stack growth");
    assert!(b.commit_reserved_page(start),
        "the lowest page of the believed stack must demand-commit");

    // The guard page itself, and everything below it. Committing the guard page is the failure this
    // design exists to prevent: it would turn an overflow into silent corruption.
    let guard = start - GRANULE;
    assert!(!b.commit_reserved_page(guard),
        "libstd's guard page at {guard:#x} must NOT be demand-committable — it must stay unbacked \
         free space so libstd's own PROT_NONE mmap lands there and faults at STAGE 1");
    assert!(!b.commit_reserved_page(guard - GRANULE),
        "the page below the guard at {:#x} must stay fatal — a frame that vaults the guard is the \
         documented residual wall, not something to silently back", guard - GRANULE);
}
```

- [ ] **Step 2: Run it to verify it fails**

```sh
cargo test -p retrace-box --test stackgrow -- --test-threads=1
```

Expected: **FAIL to compile** — `believed_stack_window` and `LIBPTHREAD_MAIN_STACK_SIZE` do not
exist. That is a legitimate red for step 2; the behavioural red comes once it compiles.

- [ ] **Step 3: Promote the libpthread constant**

In `crates/retrace-box/src/lib.rs`, near the `DYN_STACK_TOP` block (line ~55), add — and delete the
local `const LIBPTHREAD_MAIN_STACK_SIZE` from `stack_geometry_tests` (line ~4642) so there is one
source of truth:

```rust
/// macOS 26 libpthread's main-thread stack size: 8 MiB minus one 16 KiB page. It calls
/// `getrlimit(RLIMIT_STACK)` and then IGNORES the reply — M8 measured that answering 0x10000000
/// instead of 0x40000 left libstd's computed guard address bit-identical, so retrace cannot
/// influence this subtrahend and must lay its geometry out around it.
pub const LIBPTHREAD_MAIN_STACK_SIZE: u64 = 0x7fc000;
```

- [ ] **Step 4: Add the window derivation and its accessor**

Directly below the constant above:

```rust
/// The guest's BELIEVED stack bottom — where libstd's `install_main_guard` mmaps its guard page.
const GUARD_PAGE_IPA: u64 = DYN_STACK_TOP - LIBPTHREAD_MAIN_STACK_SIZE;   // 0x2004000
/// The believed-but-unbacked window M21 reserves: one granule ABOVE the guard page (so the guard
/// stays in free space and faults at stage 1), up to the real backed stack bottom.
const GUARD_TOP: u64 = GUARD_PAGE_IPA + GRANULE as u64;                    // 0x2008000
const DYN_STACK_BOTTOM: u64 = DYN_STACK_TOP - DYN_STACK_SIZE;              // 0x27C0000
```

and, next to `stack_top()` / `stack_size()`, the accessor:

```rust
/// `[start, end)` of the stack the guest BELIEVES it has but retrace does not back — reserved at
/// load, grown page-by-page by `commit_reserved_page`. Excludes libstd's guard page by one granule
/// at the bottom, and the really-backed stack at the top.
pub fn believed_stack_window(&self) -> (u64, u64) { (GUARD_TOP, DYN_STACK_BOTTOM) }
```

- [ ] **Step 5: Reserve the window at load**

Add the method beside `place_worker_stack` (line ~3700), whose pattern it copies:

```rust
/// Reserve the main thread's believed-but-unbacked stack (M8 spec risk R3).
///
/// **A reservation, not an `mmap`** — the same choice, for the same measured reason, as
/// `place_worker_stack`: `guest_vm_reserve` is bookkeeping only, and each page is demand-committed
/// with a fresh zeroed anon page on first touch by `commit_reserved_page`. Eagerly backing the full
/// 8 MiB was measured at ~1.7x on `hello_rust` and worse across the dyld suite, because the
/// per-syscall diff scales with total MAPPED memory — and a reservation maps nothing. A guest that
/// never recurses deeply commits zero pages and pays zero.
///
/// **The window deliberately stops one granule ABOVE the guard page.** libstd mmaps its guard
/// `MAP_FIXED PROT_NONE` at `GUARD_PAGE_IPA`; a backed PROT_NONE page faults at STAGE 1 (permission,
/// via the EL1 trampoline) and arrives as `Stop::Fault` for M12's disposition check, which is what
/// delivers SIGSEGV to libstd's handler. If the guard were inside the reservation it would instead
/// be unbacked, fault at STAGE 2, and be silently committed here — turning a stack overflow into a
/// corrupted guest that keeps running. That is M13's invariant (see `protect_none`), and this is the
/// one place M21 could have broken it.
///
/// Deterministic and trace-free: `load_dynamic` runs identically on record and replay, so the same
/// reservation exists on both sides and nothing about it enters the trace (symmetry rule 2).
fn reserve_believed_stack(&mut self) {
    assert!(GUARD_TOP > GUARD_PAGE_IPA,
        "the window must start ABOVE the guard page {GUARD_PAGE_IPA:#x}, or an overflow is \
         silently committed instead of faulting");
    assert!(GUARD_TOP < DYN_STACK_BOTTOM,
        "believed-stack window inverted: {GUARD_TOP:#x} >= {DYN_STACK_BOTTOM:#x}; DYN_STACK_TOP or \
         DYN_STACK_SIZE moved without moving this");
    assert!(GUARD_TOP >= PT_L3_CEIL,
        "the window would overlap the L3 translation tables below {PT_L3_CEIL:#x}");
    self.guest_vm_reserve(GUARD_TOP, DYN_STACK_BOTTOM - GUARD_TOP, false);
}
```

Then call it in `load_dynamic`, immediately after the `let mut b = Box_ { .. };` at line 1686 and
before the M14 `save_ctx` block:

```rust
        b.reserve_believed_stack();
```

- [ ] **Step 6: Run the test to verify it passes**

```sh
cargo test -p retrace-box --test stackgrow -- --test-threads=1
```

Expected: **PASS**, all five assertions.

- [ ] **Step 7: Verify the whole box crate is still green**

```sh
cargo test -p retrace-box --no-fail-fast -- --test-threads=1; echo "EXIT=$?"
```

Expected: green. Pay particular attention to `reservecommit`, `carveout`, `protnone`, and `threads` —
those four defend the gate M21 must not have relaxed. If `stack_geometry_tests` fails to compile,
it is because Step 3 removed its local constant; point it at the new `pub` one.

- [ ] **Step 8: Re-measure T0-3 and compare**

Repeat Task 0 Step 3's commands verbatim and append the after-numbers to the measurements file.
**Expected: no measurable difference.** If a regression appears, **stop and report** — the approach
is invalidated.

- [ ] **Step 9: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/stackgrow.rs \
        docs/superpowers/specs/2026-08-29-retrace-m21-stackgrow-measurements.md
git commit -m "M21-stackgrow t1: reserve the stack the guest believes it has"
```

**Verification:** `cargo test -p retrace-box --no-fail-fast -- --test-threads=1` green; clippy clean;
T0-3 shows no cost regression.

---

### Task 2: The geometry, and the property growth must not break

Two things Task 1 relies on but did not pin: the window arithmetic as a pure constant check, and the
fact that committed growth pages cannot confuse `stack_geometry_from_memory` (spec risk 7).

**Files:**
- Modify: `crates/retrace-box/src/lib.rs`, `mod stack_geometry_tests` (line ~4589 — **in-lib, not a
  `tests/` file**; do not create one).

**Interfaces:**
- Consumes: `GUARD_PAGE_IPA`, `GUARD_TOP`, `DYN_STACK_BOTTOM`, `LIBPTHREAD_MAIN_STACK_SIZE` from
  Task 1; `stack_geometry_from_memory(&[Region]) -> (u64, u64)`; the module's existing
  `fn region(ipa: u64, len: u64) -> Region` helper.
- Produces: nothing consumed later.

- [ ] **Step 1: Write the failing tests**

Append inside `mod stack_geometry_tests`, after
`the_guard_page_libstd_computes_is_a_mappable_guest_address`:

```rust
    // M21. The believed-stack window is derived, not typed — this pins the arithmetic both ways so
    // that moving DYN_STACK_TOP or DYN_STACK_SIZE without moving the window fails here, instantly
    // and on every gate, rather than in a guest that silently keeps running after an overflow.
    #[test]
    fn the_believed_stack_window_brackets_the_guard_page_and_the_backing() {
        assert_eq!(GUARD_PAGE_IPA, 0x2004000, "libstd's computed guard page");
        assert_eq!(GUARD_TOP, 0x2008000, "one granule above it");
        assert_eq!(DYN_STACK_BOTTOM, 0x27C0000, "the real backed stack bottom");
        assert_eq!(DYN_STACK_BOTTOM - GUARD_TOP, 0x7B8000, "7.72 MiB of believed-but-unbacked stack");

        assert_eq!(GUARD_TOP, GUARD_PAGE_IPA + GRANULE as u64,
            "the window must clear the guard page by EXACTLY one granule: more would leave a hole \
             a growing stack faults fatally in, less would swallow the guard");
        assert!(GUARD_TOP >= PT_L3_CEIL,
            "the window must not overlap the L3 translation tables below {PT_L3_CEIL:#x}");
        assert_eq!(GUARD_PAGE_IPA % GRANULE as u64, 0, "the guard page must be granule-aligned");
    }

    // M21 risk 7. `restore` re-derives the stack geometry from the snapshot's regions and FAILS LOUD
    // rather than guessing. Growth pages become new backings, so the question is whether they can
    // change its answer. They cannot — `covers` matches on `r.ipa == base`, an EXACT base rather
    // than containment, and every growth page starts below DYN_STACK_BOTTOM at its own granule.
    // This is a property M21 depends on but did not create, so it is pinned rather than argued.
    #[test]
    fn growth_pages_below_the_stack_do_not_shadow_the_geometry_probe() {
        let mut regions = vec![region(DYN_STACK_BOTTOM, DYN_STACK_SIZE)];
        // Three committed growth pages walking down, exactly as a deep recursion would leave them.
        for i in 1..=3u64 {
            regions.push(region(DYN_STACK_BOTTOM - i * GRANULE as u64, GRANULE as u64));
        }
        assert_eq!(stack_geometry_from_memory(&regions), (DYN_STACK_TOP, DYN_STACK_SIZE),
            "growth pages must not shadow the stack backing the probe matches on");
    }

    // Non-vacuity for the test above, split out so it can use the module's existing #[should_panic]
    // idiom rather than catch_unwind: a growth page ALONE must NOT satisfy the dynamic arm. Without
    // this, the assertion above could pass because the probe matches growth pages too.
    #[test]
    #[should_panic(expected = "refusing to guess a stack geometry")]
    fn a_growth_page_alone_does_not_satisfy_the_dynamic_arm() {
        let _ = stack_geometry_from_memory(&[region(DYN_STACK_BOTTOM - GRANULE as u64, GRANULE as u64)]);
    }
```

- [ ] **Step 2: Run to verify they fail**

```sh
cargo test -p retrace-box the_believed_stack_window_brackets -- --test-threads=1
cargo test -p retrace-box growth_pages_below_the_stack -- --test-threads=1
```

Expected before Task 1's constants exist: compile failure. Expected with Task 1 landed: **PASS** —
these pin behaviour Task 1 already implemented, so if they pass immediately that is correct, not a
skipped red. Confirm they can fail by temporarily changing `GUARD_TOP` to `GUARD_PAGE_IPA` and
re-running: the first test must fail on the "EXACTLY one granule" assertion. **Revert that edit.**

- [ ] **Step 3: Run the crate**

```sh
cargo test -p retrace-box --no-fail-fast -- --test-threads=1; echo "EXIT=$?"
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/retrace-box/src/lib.rs
git commit -m "M21-stackgrow t2: pin the window arithmetic and the probe it must not break"
```

**Verification:** both tests green, and both verified able to fail via the temporary edit in Step 2.

---

### Task 3: The headline gate

Un-park `stackoverflow_rust_e2e` and rewrite its `#[ignore]` reason into a description of the
mechanism plus the residual wall. Per honest-gate discipline the `#[ignore]` reason is the primary
record of a wall — so when the wall moves, that text is rewritten, not deleted wholesale.

**Files:**
- Modify: `crates/retrace/tests/stackoverflow_rust_e2e.rs`

**Interfaces:**
- Consumes: `util::record_dynamic`, `util::replay`, `retrace_guest::OVERFLOW` (all already used by
  the parked test — the test body itself does not change).
- Produces: nothing consumed later.

- [ ] **Step 1: Run the parked test forced, and confirm it now passes**

```sh
cargo test -p retrace --test stackoverflow_rust_e2e -- --test-threads=1 --ignored --nocapture
```

Expected: **PASS** — `has overflowed its stack` on stderr, exit 134, two byte-identical replays.
Contrast with T0-4's recorded failure, which is the before-picture.

If it still fails with a stage-2 fault, **stop and report** with the exact `far/ipa` and FSC: an FSC
of `0x7` below `0x2008000` means the growth stopped early; an FSC of `0x7` *at* `0x2004000` means the
guard page never got mmapped and the reservation may have swallowed it.

- [ ] **Step 2: Delete the `#[ignore]` and rewrite the header comment**

Replace the file's leading comment and the whole `#[ignore = "..."]` attribute (lines 1–26) with:

```rust
// M21-stackgrow. This gate was PARKED at M8 spec risk R3 from M8 through M20: libstd computes its
// stack-overflow guard page at pthread_get_stackaddr_np() - pthread_get_stacksize_np(), macOS 26's
// libpthread reports a CONSTANT 0x7fc000 that retrace cannot influence (M8 measured that a different
// getrlimit(RLIMIT_STACK) answer left the computed address bit-identical), and retrace backed only
// 256 KiB. So the guard landed at 0x2004000, 7.73 MiB BELOW the real stack bottom 0x27C0000, and
// this recursion ran off the stack into unbacked IPA and took a fatal STAGE-2 fault instead of
// striking the guard: 'far/ipa=0x27bff60 (UNMAPPED)', FSC=0x7.
//
// M21 reserves [0x2008000, 0x27C0000) — the stack the guest BELIEVES it has — so commit_reserved_page
// grows into it one zeroed page per stage-2 fault, and the recursion walks all the way down to the
// guard. The guard page itself is deliberately left OUTSIDE the reservation, so it stays a backed
// PROT_NONE page that faults at STAGE 1 and reaches libstd's handler as SIGSEGV (M13's route). The
// two rejected M8 fixes stay rejected: eager 8 MiB backing cost ~1.7x because the per-syscall diff
// scales with MAPPED memory, and a reservation maps nothing.
//
// RESIDUAL WALL, not closed by M21 and deliberately so: libstd's guard is a SINGLE 16 KiB page, so a
// frame larger than one granule can decrement SP straight past it into the free IPA below, which no
// reservation covers — commit_reserved_page correctly refuses and the fault stays fatal. Closing
// that would mean loosening commit_reserved_page's strict "never materialize untracked memory" gate,
// which reservecommit.rs and carveout.rs exist to defend; the trade is bad. This guest does not hit
// it: rs/overflow.rs's frame is [u64; 64] (512 bytes) plus overhead, far under the granule, so it
// descends page-by-page. See README "Known limits".
//
// The assertion is on the DIFFERENCE M21 makes, not on an exit code a weaker failure would share:
// 134 alone would also be produced by any other abort, so stderr must carry libstd's own recognition
// string — proof the guard was struck and the handler compared si_addr against its own guard range.
mod util;

#[test]
fn a_rust_stack_overflow_strikes_its_own_guard_page() {
```

Leave the test body exactly as it is — its four assertions were written for this moment and must not
be loosened.

- [ ] **Step 3: Run it un-parked**

```sh
cargo test -p retrace --test stackoverflow_rust_e2e -- --test-threads=1
```

Expected: **PASS**, and note it runs now without `--ignored`.

- [ ] **Step 4: Verify the neighbouring gates did not move**

```sh
for t in protnone_rust_e2e segv_rust_e2e hello_rust_e2e panic_e2e; do
  cargo test -p retrace --test $t -- --test-threads=1; echo "$t EXIT=$?"
done
```

Expected: all green. `protnone_rust_e2e` is the important one — it observes the very guard page at
`0x2004000` being installed, so it proves the reservation did not swallow it.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace/tests/stackoverflow_rust_e2e.rs
git commit -m "M21-stackgrow t3: the headline gate, un-parked"
```

**Verification:** the gate passes without `--ignored`, its assertions are unchanged from the parked
version, and the four neighbouring gates are green.

---

### Task 4: The gate and the two documents

**Files:**
- Modify: `README.md` — "What works today" and "Known limits"
- Modify: `docs/status-log.md` — append a new section (never rewrite an old one)

**Interfaces:**
- Consumes: the reconciled gate numbers produced in Step 1.
- Produces: nothing.

- [ ] **Step 1: Run the full gate, chunked**

Per `CLAUDE.md`. Capture each exit code **before any pipe**, and **do not omit `--bins`**:

```sh
cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1 \
  > /tmp/g1.log 2>&1; echo "CHUNK1=$?"
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 > /tmp/g2.log 2>&1; echo "CHUNK2=$?"
cargo test -p retrace --bins --no-fail-fast -- --test-threads=1 > /tmp/g3.log 2>&1; echo "CHUNK3=$?"
for t in $(ls crates/retrace/tests/*.rs | xargs -n1 basename | sed 's/\.rs$//' | grep -v '^util$'); do
  cargo test -p retrace --test "$t" --no-fail-fast -- --test-threads=1 \
    > "/tmp/g-$t.log" 2>&1; echo "$t=$?"
done
cargo clippy --workspace --all-targets -- -D warnings; echo "CLIPPY=$?"
```

Sum passed/failed/ignored across chunks with `grep -a` (the logs carry ANSI and UTF-8):

```sh
grep -ah "^test result:" /tmp/g1.log /tmp/g2.log /tmp/g3.log /tmp/g-*.log
```

- [ ] **Step 2: Reconcile the count file-by-file**

Do not trust the sum. M20 closed at **478 `#[test]` — 476 run, 2 parked**. M21 adds
1 (`stackgrow.rs`) + 3 (`stack_geometry_tests`) = **4 new**, and un-parks 1, so expect **482 total,
481 running, 1 parked** (`cache_symbol_e2e` alone). If your count differs, diff `#[test]` counts
file-by-file against M20's close rather than adjusting this number to match.

```sh
for d in hv-sys retrace-arch retrace-trace retrace-guest retrace-box retrace-core retrace-sim retrace; do
  printf "%-16s %s\n" "$d" "$(grep -rc '#\[test\]' crates/$d --include='*.rs' | awk -F: '{s+=$2} END {print s}')"
done
grep -rn '#\[ignore' crates/ --include='*.rs'
```

The `#[ignore]` grep must return **exactly one real parked test** — `cache_symbol_e2e`. If
`stackoverflow_rust_e2e` still appears, Task 3 Step 2 did not land.

- [ ] **Step 3: Edit the README**

The README is **edited in place** — it says what is true *now* and never carries a "superseded" note.

In "What works today", state that a deep recursion now strikes its own guard page and aborts with
libstd's own stack-overflow message, and that the main thread's believed stack grows on demand.

In "Known limits", **replace** the M8 R3 entry (the whole believed-8-MiB-vs-backed-256-KiB paragraph)
with the residual wall, stated at its true, narrower size:

> A guest frame larger than one 16 KiB granule can decrement SP past libstd's single guard page into
> free IPA below it, where no reservation covers the fault and it stays fatal. The believed stack
> itself now grows on demand, so this is no longer "any recursion past 256 KiB" — only a single frame
> exceeding a granule. Closing it would require loosening `commit_reserved_page`'s strict gate.

Update the gate line to the reconciled numbers, naming the measuring commit.

- [ ] **Step 4: Append to the status log**

`docs/status-log.md` is **append-only**: add a new `## Status: M21-stackgrow — ...` section and do
not rewrite M8's or any earlier entry. Where an earlier claim is now wrong, leave it standing with a
forward pointer. Cover: what was parked and for how long (M8→M20), the mechanism, that it is M18's
`place_worker_stack` pattern rather than a new idea, the four measurements with their numbers,
T0-3's before/after showing no cost regression, the residual wall, and the reconciled gate count.

- [ ] **Step 5: Commit**

```bash
git add README.md docs/status-log.md
git commit -m "M21-stackgrow t4: the gate, and the two documents"
```

**Verification:** gate green and reconciled file-by-file; exactly one `#[ignore]` remains; README
edited in place; status-log appended without touching earlier sections; `git status` clean apart from
intended files.

---

## Self-Review

**Spec coverage.** Every spec section maps to a task: the mechanism and the two fault routes → Task 1;
"why growth does not confuse `stack_geometry_from_memory`" → Task 2; the residual wall → Tasks 3–4;
all four measurements → Task 0 (T0-3 re-measured in Task 1 Step 8); exit criteria 1–5 → Tasks 3–4;
risk register rows 1–3, 6 → Task 0, row 7 → Task 2, rows 4–5 → documented in Tasks 3–4.

**Placeholder scan.** No TBD/TODO. Every code step carries real code; every command is runnable.

**Type consistency.** `believed_stack_window() -> (u64, u64)` returns `(start, end_exclusive)` and is
used that way in Tasks 1 and 2. `LIBPTHREAD_MAIN_STACK_SIZE` is `pub` in Task 1 Step 3 and referenced
via `retrace_box::` from the integration test, and unqualified inside `stack_geometry_tests` (which
does `use super::*`). `GUARD_PAGE_IPA` / `GUARD_TOP` / `DYN_STACK_BOTTOM` stay private and are used
only in-crate, which is why Task 1 adds the accessor for the integration test.

**Known ordering constraint.** Task 2's tests will pass immediately once Task 1 lands, since they pin
behaviour Task 1 implements. Task 2 Step 2 therefore requires proving they *can* fail via a temporary
edit — without that, they are unearned greens.
