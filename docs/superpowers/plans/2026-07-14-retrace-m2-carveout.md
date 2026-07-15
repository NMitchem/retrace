# M2-carveout Implementation Plan — reservation holes + kernel-faithful ANYWHERE placement

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Model libmalloc's guarded-metadata carveout protocol — `mach_vm_deallocate` must punch holes in tracked reservations, and hinted `VM_FLAGS_ANYWHERE` placement must treat reservations as occupied and first-fit forward from the hint (landing xzone's metadata commit in the carveout hole exactly as the kernel does) — then walk `hello_dyn` past the xzone segment-group NULL-deref toward `main`.

**Architecture:** xzone reserves ~5 MiB PROT_NONE, punches a 1 MiB hole with `mach_vm_deallocate`, then commits its zone metadata with `mach_vm_map(ANYWHERE, hint=reservation_base, prot=3)`, relying on the kernel's first-fit skipping the reservation into the hole. retrace no-ops the dealloc and honors the hint (its `range_is_free` checks only `backings`), so the metadata lands at the base, the guest's zone-init stores never reach the addresses xzone later derives, and a segment-group struct is read from a demand-committed zero page (`xzsg_main_ref == 0` → near-null deref). Fix in `Box_` (shared by record/replay, deterministic, trace format unchanged; returned addresses are byte-checked by the replay oracle): subtract deallocated ranges from `reservations` (remove/trim/split), make `range_is_free` reservation-aware, and give the ANYWHERE branch a hint-forward first-fit. Spec: `docs/superpowers/specs/2026-07-14-retrace-m2-carveout-design.md` — read it fully before starting, especially **risk 1**.

**Tech Stack:** Rust workspace, Hypervisor.framework via `hv-sys`, arm64 asm guests. Investigation artifacts (read for context): `.superpowers/sdd/xzone-research.md` (libmalloc source findings, incl. a scratchpad libmalloc checkout), `.superpowers/sdd/xzwall-vmops.md` (84-op decoded VM table of the failing run).

## Global Constraints

- Branch: `m2-carveout` (create from `main` before Task 1).
- All test runs: `cargo test --workspace -- --test-threads=1`; full gate `just gate`; clippy `-D warnings` clean at every commit; codesign + bounded-run rules as in prior milestones (see `crates/retrace/tests/util/mod.rs::bin()`; `perl -e 'alarm 60; exec @ARGV' -- <cmd>`).
- Never fake a green; `hello_dyn_e2e` stays `#[ignore]`d unless the walk genuinely reaches `main → write → exit` with byte-identical replay.
- Commit messages: `M2-carveout tN: <what>` + trailing `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Symmetry rules: placement/subtraction logic lives in `Box_` methods called by both sides; any dispatch-layer change must be textually mirrored record↔replay.
- Exact values (verbatim, from the two investigations):
  - Observed protocol: MIG-4811 reservation `size=0x4f4000, cur_protection=0, tag=1` at `0xa000c0000` → `MACH_VM_DEALLOCATE` of a 1 MiB interior sub-range (currently no-op'd) → `mach_vm_map(ANYWHERE, hint=0xa000c0000, size=0x3e000, prot=3)` (currently lands at the hint; must land in the hole).
  - Fault: `ldrb w9,[x8,#0x178]`, x8=0 from `ldp x27,x8,[x20,#0x10]`; x20 = `xzm_segment_group_s*`; `+0x18` = `xzsg_main_ref`, written eagerly at `xzone_malloc.c:9225` (libmalloc-812.100.31).
  - Code sites: `guest_vm_map` ANYWHERE branch `crates/retrace-box/src/lib.rs:961-984`; `range_is_free` `:1022-1027` (backings-only); `guest_munmap` `:1126-1134` (never touches `reservations`); `reservations: Vec<(u64,u64)>` from M2-mmapcommit; `GRANULE = 0x4000`; `MMAP_BASE = 0xA_0000_0000`; nano band `[0x4_0000_0000, 0xA_0000_0000)`.
  - Gate baseline: **61 passed / 0 failed / 1 ignored**.

---

### Task 1: Hole-punching + reservation-aware first-fit placement (+ micro-tests)

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` — reservation subtraction in the deallocate path; reservation-aware `range_is_free`; hint-forward first-fit in `guest_vm_map`'s ANYWHERE branch.
- Verify (likely no change): `crates/retrace-core/src/lib.rs` — deallocate/map dispatch routes both sides.
- Create: `crates/retrace-guest/asm/carveout.s` + build stanza + `CARVEOUT` const.
- Create: `crates/retrace/tests/carveout_e2e.rs`; extend `crates/retrace-box/tests/` with placement/split unit tests.

- [ ] **Step 1 (EVIDENCE FIRST — risk 1, the central hazard): settle the nano-band commit semantics before writing any placement code.** The box header comment (`lib.rs:20-28`) says libmalloc nano commits sub-ranges "at EXACT hint addresses" inside its own 24 GiB FIXED reservation, and today that works only because `range_is_free` is reservation-blind. Making reservations opaque to ANYWHERE would relocate those commits and break the M2-mach nano modeling. Determine from evidence what the nano commit actually is: grep the failing run's decoded VM table (`.superpowers/sdd/xzwall-vmops.md`) and a fresh `RETRACE_TRACE=1` run for `mach_vm_map` calls with tag `0xb` (VM_MEMORY_MALLOC_NANO) or addresses in `[0x4_0000_0000, 0xA_0000_0000)`; read the `machmsg.rs` nano test; if needed read nanov2 source in the scratchpad libmalloc checkout (`nanov2_malloc.c`, its `mach_vm_map` calls — FIXED? FIXED|OVERWRITE? ANYWHERE-with-hint?). Encode the finding as the placement rule:
  - If nano commits are FIXED (or FIXED|OVERWRITE): simple — ANYWHERE excludes reservations; the FIXED path (`unmap_overlapping` + map) is untouched and keeps serving nano.
  - If nano commits are genuinely ANYWHERE-with-hint-inside-own-reservation: that contradicts real-kernel semantics — re-examine (does nano deallocate first? is the band reservation shaped differently?) and report **NEEDS_CONTEXT with the evidence** rather than guessing. A wrong rule here breaks a previously-conquered wall.
  Document the verdict + citations in your report before proceeding.

- [ ] **Step 2 (RED): placement/split unit tests** in `crates/retrace-box/tests/` driving `Box_` directly (pattern: `reservecommit.rs`): (a) reserve, punch interior hole → table splits into two exact remnants; head/tail trims; full-cover removes; (b) hinted ANYWHERE map (hint = reservation base, len ≤ hole) returns the hole base and fully backs the request; (c) touch in remaining reserved band still demand-commits; (d) touch in the hole outside the new backing is refused (`commit_reserved_page` false); (e) whatever nano rule Step 1 settled, as its own named test. Run → expect FAIL.

- [ ] **Step 3 (RED): carveout e2e micro-guest.** `carveout.s`: reserve PROT_NONE via the trap route (pattern: `reservecommit.s`) → `mach_vm_deallocate` an interior 1 MiB → `mach_vm_map(ANYWHERE, hint=base, prot=RW)` → store/load sentinel through the returned address → exit with a value encoding success + (returned_addr == expected hole base, computable in-guest from the trap return values). `carveout_e2e.rs` records + replays, asserts stdout/exit byte-identical. Run → expect FAIL (today the map lands at base).

- [ ] **Step 4 (GREEN): implement.** (a) Reservation subtraction: a `Box_` helper subtracting `[addr, addr+len)` from `reservations` (remove / trim head / trim tail / split interior; GRANULE-align), called from the shared deallocate path (`guest_munmap` — verify both trap and MIG deallocate routes reach it on both sides; mirror if any gap). (b) `range_is_free` also excludes `reservations` (per Step 1's rule). (c) ANYWHERE hint-forward first-fit: lowest `a ≥ page-rounded hint` with `[a, a+len)` clear of backings, reservations, and forbidden windows; walk gap edges in address order (candidates: hint itself, then ends of overlapping entries — keep it simple and deterministic); zero hint or no fit → existing bump path unchanged.

- [ ] **Step 5: verify GREEN** on Steps 2+3 tests; capture RED/GREEN output (TDD evidence).

- [ ] **Step 6: regression gate.** `just gate` — 61 prior tests must stay green (esp. `machmsg` nano, `reservecommit`, `wildstore`, `seeded_swarm`) plus the new tests; clippy clean. A nano regression here means Step 1's rule is wrong — STOP and reassess, do not patch around it.

- [ ] **Step 7: commit.** `M2-carveout t1: reservation hole-punching + hint-forward first-fit ANYWHERE placement (carveout protocol)`.

---

### Task 2: Walk past the xzone metadata wall; advance or re-park the gate

Same structure as M2-mmapcommit Task 2. Bounded traced walk; expect the metadata commit to land in the carveout and `_xzm_segment_group_alloc_segment` to proceed with a non-null `xzsg_main_ref`. Then:

- [ ] **Step 1:** traced walk; record trap count, new furthest point, symbolicated anatomy of any new fault. Confirm the 12× `gettimeofday` backoff disappeared (spec expects so; if it persists, note it — do not chase unless fatal).
- [ ] **Step 2a (reached `main → write → exit`):** un-ignore `hello_dyn_e2e`; verify record prints `hi\n`, replay byte-identical, per-syscall + final memory checks green; add the double-replay determinism test; run it repeatedly for stability. The M2 headline gate goes green — be rigorous, no flakiness.
- [ ] **Step 2b (new distinct wall):** keep `#[ignore]`, rewrite the reason + block comment with the verified anatomy; small mirrored below-the-trace fixes belonging to *this* milestone's scope may be applied and re-walked; a distinct subsystem is documented and deferred. The env/commpage xzone escape hatch (`_COMM_PAGE_DEV_FIRM` + `MallocAllowInternalSecurity=1` + `MallocSecureAllocator=0`, see xzone-research.md) is the documented fallback — propose, don't unilaterally take it.
- [ ] **Step 3:** README `## Status: M2-carveout …` section (root cause, fix, honest walk outcome); `just gate` green; clippy clean; commit `M2-carveout t2: walk past xzone metadata — <reached main | re-parked at NEWWALL>`.

---

## Integration & close-out

- Gate green with honest ignore count; memory updated (the wall-chain memory `retrace-objc-preoptimization-wall` gets the carveout outcome; if `main` was reached, say so loudly — it closes the M2 arc's headline).
- Merge `m2-carveout` → `main` (`Merge M2-carveout (reservation holes + kernel-faithful ANYWHERE placement) into main`).

## Notes for the implementer

- Step 1 is not optional ceremony — the nano hazard is real and the previous milestone's spec explicitly warned that changing ANYWHERE placement "perturbs the walk." Evidence before code.
- Keep the first-fit deterministic and simple (sorted gap-edge walk); cleverness here is a determinism risk.
- `Box_` field order is load-bearing (`vcpu` before `vm`). Reservation arithmetic stays GRANULE-aligned.
- Do not scope-creep into OVERWRITE modeling, guard-fault semantics, or the escape hatch; each is explicitly out of scope.
