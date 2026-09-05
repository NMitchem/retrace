# M26-cpythonreplay Implementation Plan

> **Retroactive.** M26 was a debugging pass, not a planned milestone: the root cause was found before
> any plan existed, and writing one first would have been fiction. This records what was actually
> done, in the order it was done, so a reader can audit the sequence rather than trust a summary.
> The M24 precedent is the one being followed. Design:
> `docs/superpowers/specs/2026-09-05-retrace-m26-cpythonreplay-design.md`.

**Goal:** Close the replay divergence M25 parked rung 7 at — or park again with better evidence.
**Outcome:** Closed. `the_real_cpython_interpreter_records_and_replays` is un-`#[ignore]`d.

**Branch:** `m26-cpythonreplay`, cut from `main` at `9c67cfc` (M25 t5).

## Global Constraints

Unchanged from M25 and not restated: bounded foreground cargo (`perl -e 'alarm N; exec @ARGV'`),
`--test-threads=1`, chunked gate with exit codes captured before any pipe, `grep -a` on gate logs,
reconcile file-by-file against `main`'s actual close. One addition measured the hard way:

- **A whole-package `cargo test -p retrace-box` beats a per-target split**, because per-target drops
  the `Doc-tests` harness silently (M24's lesson, acted on rather than rediscovered).
- **First execution of a freshly codesigned binary can stall for minutes.** `bigread_e2e` took
  536.61s on its first run and 46.65s on its second, with the record process at 0:00.00 CPU
  throughout the stall. That is Gatekeeper validation, not the guest hanging. **Do not read it as a
  hang and do not kill it** — the second run is the honest timing.

---

### Task 0: Measure before believing anything *(done)*

- [x] Reproduce: record CPython, replay, confirm the divergence is deterministic. It is — same `pc`,
      same live args as M25, **different landmark index** (560 here, 568 there), which is itself the
      finding that the index was never evidence.
- [x] Read the error carefully. `num=75` is `madvise`, not `mmap` as M25's ignore reason claimed
      (`sys/syscall.h:115`). That correction is what made the two sides legible as normal-path vs
      error-path.
- [x] Instrument the divergence site to dump the buffer live was about to `write(2, …, 106)`:
      `ValueError: bad marshal data (unknown type code)`. A **data** divergence, not control-flow.
- [x] Dump the recorded events before the divergence. Landmark 556: `read` returns 88103, writes
      capture 65536.
- [x] Minimal hypothesis test — raise `PTR_WINDOW_CAP` to 256 KiB, re-record (the defect is
      record-side, so the old trace cannot be reused), replay: clean. Hypothesis confirmed.
- [x] Revert every temporary edit before writing the fix.

**Do not** skip the revert and "adapt" the probe into the fix. The probe proved a cause; it is not a
design.

---

### Task 1: The fix, TDD *(done — commit `c0faff1`)*

- [x] **A repo-owned guest first.** `bigread` reads 96 KiB and emits only the final byte. It exists
      because `cpython_e2e` **skips** when Homebrew Python is absent and therefore cannot be a
      regression guard — a gate that can silently not-run is not a guard.
- [x] **RED at the box layer.** `forward_and_diff_captures_a_read_larger_than_the_window` failed with
      "read returned 98304 bytes … captured 65536 bytes across 1 write(s)" — the same signature as
      CPython's 88103/65536. Watched fail before any production edit.
- [x] **RED end-to-end.** `bigread_e2e` records, deletes the fixture, replays. The deletion is what
      proves the tail came from the trace and not the disk.
- [x] **GREEN.** `writes_x2_bytes_to_x1` + `diff_window`; the clamp and the window now share one
      predicate so they cannot drift apart again.
- [x] **Mutation-verified.** Reverting only `diff_window`'s widening puts the failure back.

**Do not** fix this by raising the constant. 300 KiB breaks identically; the defect is that two
bounds which must agree were written twice.

---

### Task 2: Move the gate forward *(done — commits `a59974b`, `4bcfc6c`)*

- [x] Run the parked gate with `--ignored`. Green: 2 replays, exit 0, stdout `1\n`.
- [x] **Delete the `#[ignore]`** and say so loudly. Rung 7.
- [x] Replace the ignore reason with a header note recording what the wall actually was, including
      M25's two wrong claims — the record is corrected forward, not quietly overwritten.
- [x] **Correct `bigread_e2e`'s account of its own failure.** t1's comment claimed the stdout
      assertion failed pre-fix and "everything else stayed green". Measured by mutation: it is the
      *terminal memory compare* on line 31 that fires, at `buf+0x10000`. The failure is **latently**
      silent, not silent, and the milestone's own test was misdescribing it.

---

### Task 3: The gate and the two documents *(this task)*

- [x] Merge `main` if it moved. It had not — `9c67cfc` is still `main`'s tip.
- [ ] Full chunked gate, every chunk `EXIT=0`, including `--bins`.
- [ ] Reconcile file-by-file against `main`'s 512 / 0 / 3 over 113. Expected, to be confirmed rather
      than assumed:
  - `crates/retrace-box/src/lib.rs` — **+0** (`diff_window`/`writes_x2_bytes_to_x1` add no `#[test]`)
  - `crates/retrace-box/tests/memdiff.rs` — **+1**
  - `crates/retrace/tests/bigread_e2e.rs` — **+1**, and **+1 test binary**
  - `cpython_e2e` — **±0 running, −1 ignored** (un-parked, not added)
  - `--bins` — **unchanged**
- [ ] clippy clean over `--workspace --all-targets`.
- [ ] **README, edited in place.** Add rung 7 to the ladder — it now meets the ladder's entry
      condition, which is exactly why M25 kept it out. Update the gate line and the ignored-gate list
      (down to two). **Correct the `/bin/ps` attribution**: it is this bug, measured, not
      nondeterminism. Add the residual truncation class to Known limits.
- [ ] **`docs/status-log.md`, appended.** Never rewrite M25's section; it stands with its wrong
      diagnosis and this one points back at it.
- [ ] CLAUDE.md only if something in it became false.

---

## Successor: M27

Named, with its first task fixed by what M26 measured:

1. **Land the tripwire** — but measure its blast radius across every gate guest FIRST. That
   measurement is the whole reason M26 did not land it.
2. **`sysctl` (202)** — length behind `*(size_t*)x3`. This is `/bin/ps`, and closing it plausibly
   moves the Apple-binary count from 46 to 47.
3. **`pread_nocancel` (414)** — missing from `fd_operands`, the clamp, and the window. The unclamped
   forward is a host memory-safety hazard and should be fixed first among the three.
4. **A `dest_buffers` table** replacing `writes_x2_bytes_to_x1`, so the shape generalises to
   `Reg(i)` / `DerefU64(i)` / `Lo32(i)` lengths instead of hard-coding `x2`.
5. **`diff_memory`'s `.min(avail)`** — the backstop's own hole, unpaid since M1.

## Self-Review

1. `TRACE_MAGIC` is unchanged (`RT\x00\x09`). M26 records nothing new and adds no `Event` variant.
2. `verify_thread` still has seven call sites plus `mirror_delivery`'s inline eighth. No dispatch arm
   was added — the fix is entirely inside `Box_`, below the record/replay loop.
3. No existing assertion was loosened. The only deletions in `forward_and_diff` are the two lines the
   shared predicate replaced.
4. `cpython_e2e` demands exactly what it demanded while parked: two byte-identical replays and the
   exact stdout. The gate moved because its cause was removed, not because the bar moved.
5. `bigread_e2e` asserts `ret == 0x18000` so it cannot rot into a vacuous gate.
6. The residual class is published as a table, not implied to be closed.
