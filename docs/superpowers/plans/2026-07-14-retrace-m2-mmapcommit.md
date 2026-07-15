# M2-mmapcommit Implementation Plan — demand-commit for mach-VM reservations

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Back a `mach_vm_map` PROT_NONE reservation's pages on first-touch fault instead of dying, by adding reservation bookkeeping + a below-the-trace `commit_reserved_page` demand-committer (twin of the shared-cache pager), then walk `hello_dyn` past libmalloc's xzone allocator toward `main`.

**Architecture:** `guest_vm_reserve` (`cur_protection==0` reservations) deliberately leaves address space unbacked; libmalloc's xzone allocator (`_xzm_segment_group_alloc_chunk`, reached via objc `realizeClassWithoutSwift`) first-touches a page inside such a reservation that was never committed, and the stage-2 translation fault is fatal because the only fault-driven backer (`page_in_cache`) serves the shared-cache window only. Fix: record each reservation's extent, and on a `Stop::Other` fault inside a tracked reservation, `alloc_pages(GRANULE)` a zeroed page + `hv_vm_map` it + push a `Backing`, then re-run. It lives in the shared fault path (`run()`-adjacent, called from both record and replay `Stop::Other` arms), so record and replay commit identical zeroed pages in the same order and nothing enters the trace. Eager backing is impossible (the nano reservation is 24 GiB). Spec: `docs/superpowers/specs/2026-07-14-retrace-m2-mmapcommit-design.md` — read it before starting.

**Tech Stack:** Rust workspace (`retrace-arch`, `retrace-box`, `retrace-core`, `retrace-guest`, `retrace`), Hypervisor.framework via `hv-sys`, arm64 asm guests.

## Global Constraints

- Branch: `m2-mmapcommit` (create from `main` before Task 1).
- All test runs: `cargo test --workspace -- --test-threads=1` (one VM per process; parallel VMs flake with `HV_BUSY`). `just gate` (= `just m1`) is the full gate.
- `cargo clippy --workspace --all-targets -- -D warnings` clean at every commit.
- Any binary calling `hv_*` must be ad-hoc codesigned with `retrace.entitlements`; for a manually-run `target/.../retrace`: `codesign -s - -f --entitlements retrace.entitlements target/aarch64-apple-darwin/debug/retrace`. Manual VM runs bounded: `perl -e 'alarm 60; exec @ARGV' -- <cmd>`.
- No `Date::now()`/randomness in tests (determinism deny-list).
- Never fake a green: if the walk doesn't reach `main`, keep `hello_dyn_e2e` `#[ignore]`d with an updated reason; report DONE_WITH_CONCERNS.
- Commit messages: `M2-mmapcommit tN: <what>`.
- Two symmetry rules (from CLAUDE.md): (1) any record special-case needs a textually-parallel replay mirror that recomputes identical addresses/bytes; (2) deterministic emulation belongs below the trace in the shared fault path so it fires identically on both sides.
- Exact values / shapes (verbatim):
  - `MMAP_BASE = NANO_BAND_END = 0xA_0000_0000`; `GRANULE = 0x4000` (16 KiB); `alloc_pages(len)` returns zeroed anon memory rounded up to GRANULE (`crates/retrace-box/src/lib.rs:212-222`).
  - `guest_vm_reserve(&mut self, addr: u64, size: u64, anywhere: bool) -> u64` (`lib.rs:954-974`): ANYWHERE bumps `mmap_next` by GRANULE-rounded size and returns the old cursor; FIXED returns `addr`. Neither records the reservation today.
  - Fault plumbing: a stage-2 abort surfaces as `Stop::Other { esr }` with `fault_ipa()` = `last_far` (`lib.rs:645`, `:1140-1147`). Dispatch arms — record `crates/retrace-core/src/lib.rs:309-313`, replay `:529-533` — both read: `Stop::Other { esr } => { if b.page_in_cache(b.fault_ipa()) { continue; } … }`. The new committer is a second `if b.commit_reserved_page(b.fault_ipa()) { continue; }` guard inserted immediately after that line, IDENTICAL on both sides.
  - `page_in_cache` (`lib.rs:549-640`) is the servicing template: round to page base, refault-loop guard (8-strike via `cache_refault_ipa`/`cache_refault_count`), `alloc_pages(GRANULE)`, `hv_vm_map` one page, push `Backing`. `commit_reserved_page` reuses this shape minus file read/re-sign.
  - `mmap_next` is reset to `MMAP_BASE` in `restore()` (`lib.rs:1199`); the new reservation table resets to empty in the same place.
  - The MIG `vm_map` (msgh_id 4811) path already splits on `cur_protection == 0` → `guest_vm_reserve` on both sides (record `core lib.rs:237-238`, replay `:398-403`). The BSD-mmap anon path is `guest_mmap` and always backs eagerly (not a reservation).
  - Observed wall (run-layout-dependent addresses): `data abort (EC=0x24 ISS=0x7 FSC=0x7) far=0xa00...(UNMAPPED)`, `pc` in `libsystem_malloc`'s `_xzm_segment_group_alloc_chunk`, inside a `size=0x480000`, `cur_protection=0`, `VM_MEMORY_MALLOC`-tagged reservation.

---

### Task 1: Reservation bookkeeping + `commit_reserved_page` + mirrored dispatch

Add the reservation table, record extents in `guest_vm_reserve`, implement the demand-committer, wire the two mirrored dispatch guards, and prove it with a reserve→first-touch micro-guest (record + replay) plus a fail-loud negative test. The full suite is the regression gate.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` — `reservations` field + reset in `restore`; record in `guest_vm_reserve`; `commit_reserved_page`.
- Modify: `crates/retrace-core/src/lib.rs` — the mirrored `commit_reserved_page` dispatch guard in record and replay `Stop::Other` arms.
- Create: `crates/retrace-guest/asm/reservecommit.s` — the micro-guest.
- Modify: `crates/retrace-guest/build.rs` (build stanza), `crates/retrace-guest/src/lib.rs` (path const, e.g. `RESERVECOMMIT`).
- Create: `crates/retrace/tests/reservecommit_e2e.rs` — record+replay round-trip.
- Possibly: `crates/retrace-box/tests/` — the fail-loud negative test (store outside any reservation still aborts).

**Interfaces produced:** `Box_::commit_reserved_page(&mut self, ipa: u64) -> bool` (public, mirrors `page_in_cache` visibility); reservation recording inside `guest_vm_reserve` (no signature change); `retrace_guest::RESERVECOMMIT` path const. Task 2 relies on the committer being live in both dispatch arms.

- [ ] **Step 1: Decide + document the micro-guest route.** The observed wall arrives via MIG 4811, painful to synthesize from freestanding asm. Use the **`mach_vm_map` trap route** with `cur_protection = 0` if it is expressible and genuinely reaches `guest_vm_reserve` — FIRST verify how the trap-path `mach_vm_map` (BSD-side num or mach trap) is dispatched in `crates/retrace-core/src/lib.rs` and whether it splits on `cur_protection == 0` like the MIG path does. **If the trap path does NOT split (risk 4 in the spec): add the same `cur_protection == 0 → guest_vm_reserve` split there, mirrored in record and replay**, so a reservation via the trap genuinely reserves (never eagerly backs — a 24 GiB trap reservation would otherwise be fatal). If the trap route is not reasonably expressible from asm, fall back to driving the reservation another way the guest can emit; document the choice in the report. Do not silently eager-back a reservation.

- [ ] **Step 2: Write the failing micro-test (RED).** In `crates/retrace/tests/reservecommit_e2e.rs`: run the `RESERVECOMMIT` guest through record, assert record succeeds (no fatal data-abort) and the guest's exit code / stdout carries the value it stored-then-loaded from a reserved page; then replay the trace and assert byte-identical stdout + the replay's final full-memory comparison passes. The guest (asm) must: reserve a region (cur_protection=0), store a sentinel to a page inside it that is past any auto-committed prefix, load it back, and exit with / print it. Add a second store to a *different* page of the same reservation to prove per-page granularity. Run: `cargo test -p retrace reservecommit -- --test-threads=1` → expect FAIL (fatal data abort, committer not implemented / test guest absent).

- [ ] **Step 3: Reservation bookkeeping.** In `crates/retrace-box/src/lib.rs`: add `reservations: Vec<(u64, u64)>` (start, len) to `Box_` (declare it consistent with existing field/drop-order constraints — do NOT reorder `vcpu`/`vm`); initialize empty in all three constructors (`load`, `load_dynamic`, `restore` — search `mmap_next:` initializers, currently lib.rs ~483/855/1199) and reset to empty in `restore` alongside `mmap_next`. In `guest_vm_reserve`, push the reserved `(returned_addr, rounded_size)` for BOTH the ANYWHERE and FIXED branches (use the GRANULE-rounded size and the actually-returned base).

- [ ] **Step 4: `commit_reserved_page`.** Implement `pub fn commit_reserved_page(&mut self, ipa: u64) -> bool` next to `page_in_cache`. Round `ipa` down to GRANULE. Return `false` if the page base lies in no tracked reservation, or if it is already backed (reuse the existing backing lookup, e.g. `host_span`, so a re-fault on an already-committed page doesn't double-map). Reuse the refault-loop guard pattern from `page_in_cache` (a livelock must panic, not spin). On a genuine first-touch inside a reservation: `alloc_pages(GRANULE)` (zeroed) → `hv_vm_map(host, page_base, GRANULE, MemFlags::RWX)` → push `Backing { host, ipa: page_base, len: GRANULE }` → return `true`. No `set_region_exec` (data page; stage-1 `ATTR_DATA` default governs, W^X preserved); no TLBI (fresh IPA — same soundness as the cache pager).

- [ ] **Step 5: Mirrored dispatch.** In `crates/retrace-core/src/lib.rs`, insert `if b.commit_reserved_page(b.fault_ipa()) { continue; }` immediately after the `page_in_cache` guard in BOTH the record (`:309-313`) and replay (`:529-533`) `Stop::Other` arms. The two insertions must be textually identical (rule 1).

- [ ] **Step 6: Run to GREEN.** `cargo test -p retrace reservecommit -- --test-threads=1` passes (record + replay). Capture RED (Step 2) and GREEN output for the report (TDD evidence).

- [ ] **Step 7: Fail-loud negative test.** Add a test (in-process box test under `crates/retrace-box/tests/`, or an asm-guest e2e) proving a store to an address in NO reservation and NO backing still produces the fatal data-abort — the committer must not materialize wild pointers. Assert the error surfaces (record returns the data-abort `Err`, or the box `run()` returns `Stop::Other` for that IPA and `commit_reserved_page` returns false).

- [ ] **Step 8: Regression gate + commit.** `just gate` — expect the prior 58 passed plus the new tests, 0 failed, honest ignore count (still 1 if the walk hasn't reached `main` yet — that's Task 2), clippy clean. Commit: `M2-mmapcommit t1: reservation bookkeeping + commit_reserved_page demand-committer (below-the-trace, mirrored)`.

---

### Task 2: Walk `hello_dyn` past the xzone wall; advance or re-park the gate

With demand-commit live, walk the dynamic run and triage the next failure fail-loud. Either the run reaches `main → write → exit` (then un-ignore `hello_dyn_e2e` + add the double-replay determinism test — the M2 headline gate) or it hits a new distinct boundary (re-park the gate honestly with full anatomy).

**Files:**
- Modify: `crates/retrace/tests/hello_dyn_e2e.rs` — un-ignore + double-replay test (on success) OR rewrite the `#[ignore]` reason to the new wall.
- Modify: `README.md` — new M2-mmapcommit Status section (root cause, fix, walk outcome).
- Modify: memory pointer / docs per close-out.
- Any per-wall fixes the walk surfaces that are genuinely part of demand-commit (mirrored below the trace); a *distinct* new subsystem is documented and deferred, NOT walked into.

- [ ] **Step 1: Bounded traced walk.** Build + codesign; `RETRACE_TRACE=1` bounded `record-dyn hello_dyn`. Confirm the run advances past `_xzm_segment_group_alloc_chunk` (the prior fatal). Record the new first-failure (trap count, ESR/fault, symbolicated pc via the shared cache) in the report.

- [ ] **Step 2a (if the walk reaches `main → write → exit`):** Un-ignore `hello_dyn_e2e`; ensure record prints `hi\n`, replay reproduces stdout byte-for-byte, per-syscall + final full-memory checks pass; add the double-replay stability test. This is the M2 headline exit criterion.

- [ ] **Step 2b (if blocked at a new distinct wall):** Keep `hello_dyn_e2e` `#[ignore]`d; rewrite its reason + block comment to the new boundary's verified anatomy (class, symbol, mechanism), citing a task report. Report DONE_WITH_CONCERNS.

- [ ] **Step 3: README + gate + commit.** Add the M2-mmapcommit Status section (honest about whether `main` was reached). `just gate` green (honest ignore count), clippy clean. Commit: `M2-mmapcommit t2: walk past xzone allocator — <reached main | re-parked at NEWWALL>`.

---

## Integration & close-out

- After Task 2, `just gate` green with an honest ignore count; if `main` was reached, `hello_dyn_e2e` is un-ignored + double-replayed (headline M2 gate GREEN) — otherwise re-parked with an accurate reason.
- Update memory (`retrace-objc-preoptimization-wall` chain / a new next-wall memory) with the outcome.
- Merge `m2-mmapcommit` → `main` (`Merge M2-mmapcommit (mach-VM reservation demand-commit) into main`).

## Notes for the implementer

- The committer MUST gate strictly to tracked reservations — materializing an arbitrary faulting page would silently mask real bring-up bugs (wild pointers). Everything outside a reservation stays fatal.
- Keep the two dispatch insertions byte-identical; an asymmetry surfaces as a replay divergence, which is the oracle working, but wastes a cycle.
- Do not add `range_is_free`/ANYWHERE-placement awareness of reservations, PROT_NONE guard-fault semantics, or partial-reservation munmap splitting unless Task 2's walk proves one is needed — they are explicitly out of scope (spec "Out / the honest edge").
- `Box_` field declaration order is load-bearing (`vcpu` before `vm`); add `reservations` without disturbing that.
