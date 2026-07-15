# M2-cpuid Implementation Plan — guest CPU/cluster identity (TPIDR_EL0)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Set the guest `TPIDR_EL0` to 0 (cpu 0 / cluster 0) so macOS 26's `_os_cpu_cluster_number()` reads 0 instead of 48, clearing libmalloc xzone's out-of-bounds segment-group index — then walk `hello_dyn` past the xzone allocator to the next wall.

**Architecture:** macOS 26 derives the current CPU number from `TPIDR_EL0[11:0]` and the cluster number from `TPIDR_EL0[>=12]`. retrace wrongly sets `TPIDR_EL0 = TSD_IPA = 0x30000` (conflating it with the thread-self pointer, which is actually `TPIDRRO_EL0`), so cluster = `0x30000>>12` = 48; xzone's `sg_index = front_count*48 + front` overshoots its segment-group array by ~253 slots onto an unbacked page → silent fault. Fix: `TPIDR_EL0 = 0` at both constructor sites; leave `TPIDRRO_EL0 = TSD_IPA`. Below the trace, identical on record/replay. **A confirmation spike already validated this** (xzone fault gone, ~205→223 traps, gate 68/0/1 clippy clean, no regression). Spec: `docs/superpowers/specs/2026-07-14-retrace-m2-cpuid-design.md` — read it before starting.

**Tech Stack:** Rust workspace, Hypervisor.framework via `hv-sys`, arm64 asm guests. Investigation artifacts: `.superpowers/sdd/cputopo-research.md` (source + live-disassembly root cause), `.superpowers/sdd/cputopo-empirical.md` (empirical determinism check).

## Global Constraints

- Branch: `m2-cpuid` (create from `main` before Task 1).
- All test runs `--test-threads=1`; full gate `just gate` (baseline 68 passed / 0 failed / 1 ignored); clippy `-D warnings` clean at every commit; codesign + bounded-run rules as in prior milestones.
- Never fake a green; `hello_dyn_e2e` stays `#[ignore]`d (the spike showed the run does NOT reach main — it re-parks at the msgh_id 3409 task-port wall).
- Commit messages `M2-cpuid tN: <what>` + trailing `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Symmetry: `TPIDR_EL0` is set in both `load_dynamic` (record) and `restore` (replay); the value MUST match on both sides (it's a below-the-trace constant — an asymmetry would surface as a replay divergence).
- Exact values (verbatim): current `crates/retrace-box/src/lib.rs:903` (`load_dynamic`) and `:1297` (`restore`) both do `vcpu.set_sys(sysreg::TPIDR_EL0, TSD_IPA).unwrap();` where `TSD_IPA = 0x0003_0000`. Change the VALUE to `0`. Leave the adjacent `TPIDRRO_EL0 = TSD_IPA` lines (`:902`, `:1296`) UNCHANGED. macOS 26: `_os_cpu_number() = TPIDR_EL0 & 0xFFF`, `_os_cpu_cluster_number() = TPIDR_EL0 >> 12`. Stale header comment to correct: `lib.rs:55-57`. New wall (for Task 2 re-park): `mach_msg2 msgh_id 3409` to the guest task port (Mach task subsystem, base 3400; confirm the exact routine from task.defs/the trace).

---

### Task 1: Set TPIDR_EL0 = 0 (+ cpu-identity test)

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` — `TPIDR_EL0 = 0` at both sites + corrected comments (the two set-sys sites and the stale header note at `:55-57`).
- Create: the cpu-identity test (in-process box test preferred — see Step 1).

**Interfaces:** no signature change; the box presents guest `TPIDR_EL0 == 0` after `load_dynamic` and `restore`.

- [ ] **Step 1 (RED): write the failing cpu-identity test.** Preferred: an in-process box test (pattern: existing `crates/retrace-box/tests/` like `mmu.rs`/`pac.rs`) that constructs the box on the dynamic path and asserts `vcpu.get_sys(sysreg::TPIDR_EL0) == 0` (and, to lock intent, that `TPIDRRO_EL0 == TSD_IPA` — the TSD base must remain). If constructing the dynamic path in-process is impractical, fall back to a `cpuid.s` guest (`mrs x0, TPIDR_EL0; ` compute cpu=`x0 & 0xFFF`, cluster=`x0>>12`; exit encoding both) run through record + replay asserting cpu==0 && cluster==0. Run → expect FAIL (currently TPIDR_EL0 = TSD_IPA = 0x30000, so cluster reads 48). Capture the RED output.

- [ ] **Step 2 (GREEN): apply the fix.** At `lib.rs:903` and `:1297`, change `TSD_IPA` → `0` in the `TPIDR_EL0` set-sys calls only. Update the inline comment to explain that `TPIDR_EL0` carries the cpu/cluster id on macOS 26 (cpu = `[11:0]`, cluster = `[>=12]`), so a single-vCPU guest needs 0, while `TPIDRRO_EL0` remains the TSD base. Correct the stale header comment at `lib.rs:55-57` (it currently says both TPIDRRO_EL0 and TPIDR_EL0 are set to TSD_IPA — now only TPIDRRO_EL0 is).

- [ ] **Step 3 (GREEN verify):** the Step-1 test passes. Capture GREEN output (TDD evidence).

- [ ] **Step 4: regression gate.** `just gate` — expect 68 prior + the new test, 0 failed, 1 ignored, clippy clean. (The spike already confirmed 68/0/1 with this change; reproduce it.)

- [ ] **Step 5: commit.** `M2-cpuid t1: TPIDR_EL0 = 0 (cpu 0 / cluster 0) — clears xzone segment-group index overshoot (cluster 48)`.

---

### Task 2: Walk past the xzone allocator; re-park the gate

- [ ] **Step 1:** bounded traced `record-dyn hello_dyn`. Confirm the run passes `_xzm_segment_group_alloc_chunk` (the prior wall) and reaches ~223 traps at the new `mach_msg2 msgh_id 3409` task-port wall. Record trap count + the exact fault line. Identify the task-subsystem routine for msgh_id 3409 (from `task.defs` / mach headers / the trace decode) for an accurate re-park reason.
- [ ] **Step 2 (expected — re-park):** keep `hello_dyn_e2e` `#[ignore]`d; rewrite its reason + block comment to the msgh_id 3409 task-port MIG wall (routine name, dest = guest task port, send_size 36, mach-IPC/M2-mach lineage, distinct from CPU identity). (If the walk somehow reaches `main → write → exit` — not expected — un-ignore + double-replay instead; be rigorous about non-flakiness.)
- [ ] **Step 3:** add a `## Status: M2-cpuid …` README section (root cause: TPIDR_EL0 cluster-48; the one-value fix; the spike-confirmed advance; the new 3409 wall; the deferred commpage-synthesis hygiene note). Match the existing Status sections' voice. `just gate` green; clippy clean. Commit `M2-cpuid t2: walk past xzone — re-parked at msgh_id 3409 task-port MIG wall`.

---

## Integration & close-out

- Gate green with honest ignore count; memory (`retrace-objc-preoptimization-wall` chain) updated with the corrected root cause (TPIDR_EL0 cluster 48, NOT a per-run-nondeterministic CPU-topology subsystem) and the new 3409 wall.
- Merge `m2-cpuid` → `main` (`Merge M2-cpuid (guest CPU/cluster identity, TPIDR_EL0) into main`).
- Next milestone: the msgh_id 3409 task-port MIG wall (M2-mach lineage). Separately, the deferred coherent single-vCPU commpage synthesis (host-topology leak hygiene) remains available as a hardening pass.

## Notes for the implementer

- This is a one-value functional change already de-risked by a confirmation spike; the work is the test + honest docs. Do NOT expand scope into commpage synthesis or the 3409 wall — both are explicitly deferred in the spec.
- `TPIDRRO_EL0` must stay `TSD_IPA` — changing it would break pthread-self/errno. Only `TPIDR_EL0` changes.
- Keep the two sites' value identical (both 0); they are the record and replay constructors.
