# retrace M2-cpuid — guest CPU/cluster identity (TPIDR_EL0)

**Design spec — 2026-07-14.** Sub-milestone of M2 (the loader), sibling of M2-cache, M2-mach,
M2-va47, M2-bfam, M2-tbi, M2-mmapcommit, and M2-carveout. Clears the wall M2-carveout re-parked at
(libmalloc's xzone segment-group indexing). The wall was **misdiagnosed** during the M2-carveout
walk as a "per-CPU commpage-topology subsystem" with per-run nondeterminism; live disassembly of the
macOS 26 binaries plus a confirmation spike prove it is a **one-value bug**: retrace sets the guest's
`TPIDR_EL0` to the TSD pointer (`0x30000`), but macOS 26 reads the current CPU *cluster* number out
of `TPIDR_EL0`'s high bits, yielding cluster **48** and an out-of-bounds segment-group index. The fix
is `TPIDR_EL0 = 0` (cpu 0 / cluster 0).

## The wall's anatomy (two investigations + live disassembly + spike, 2026-07-14)

**Observed fault (M2-carveout's boundary):** `_xzm_segment_group_alloc_chunk+0x1c4` (libsystem_malloc),
an UNMAPPED access at `sg+4`. The main-zone metadata block's `xzmz_total_size = 0x3e000` (fully
backed), yet xzone indexes a segment group at `main + ~0x4e4c8` — past the block, onto an unbacked
page; the compiled-out debug bounds assert lets it fault silently on an `os_unfair_lock` CAS.

**Root cause (live lldb disassembly of the on-disk shared cache; full notes
`.superpowers/sdd/cputopo-research.md`):** On macOS 26 arm64e,
- `_os_cpu_number() = TPIDR_EL0 & 0xFFF` (verified in `pthread_cpu_number_np` @ `0x1804f09ec`),
- `_os_cpu_cluster_number() = (uint32_t)TPIDR_EL0 >> 12` (verified in
  `_xzm_xzone_find_and_malloc_from_freelist_chunk+1196/+1220`).

retrace sets **`TPIDR_EL0 = TSD_IPA = 0x30000`** (`crates/retrace-box/src/lib.rs:903` in
`load_dynamic`, `:1297` in `restore`), conflating it with the thread-self pointer. Consequences:
- cpu number = `0x30000 & 0xFFF = 0` — accidentally correct,
- cluster number = `0x30000 >> 12 = 48` — **garbage** (there is no cluster 48).

xzone sizes `segment_group_count = front_count * (per_cluster ? ncpuclusters : 1)` at zone init
(`xzone_malloc.c:8846/8945/8955`, `per_cluster` true here) and at allocation computes
`sg_index = front_count*clusterid + sg_front_index`, `sg = xzmz_segment_groups + sg_index*0x278`
(`:363/367`). With `clusterid = 48`, `sg_index` (masked `& 0xff`) lands ~253 slots out →
`main + ~0x4e4c8`, past the `0x3e000` block. **Deterministic-but-wrong.**

**The "per-run variance" was a misattribution.** The M2-carveout walk saw the fault offset differ
(`0x4e4c8` vs `0x4e740`); the delta `0x278` is exactly one `sizeof(xzm_segment_group_s)` — a
different allocation reaching the fault first as the forwarded-`getentropy`/PID nondeterminism drifts
the pre-fault `gettimeofday` spin count (12→17). The register-derived overshoot itself is fixed and
deterministic. (Record legitimately differs run-to-run because `getentropy`/`proc_info` are forwarded
and their outputs recorded per-trace; that is normal for record/replay and does **not** threaten
replay determinism, which the divergence oracle enforces per trace.)

**`TPIDRRO_EL0` is not the culprit and must not change.** It correctly holds the TSD base
(`TSD_IPA`); dyld/libSystem read the errno slot and pthread-self through it (`lib.rs:55-57`). Only
`TPIDR_EL0` carries the cpu/cluster id on macOS 26.

## The fix

Set the guest `TPIDR_EL0` to **0** (cpu 0, cluster 0) at both sites (`load_dynamic` `lib.rs:903`,
`restore` `:1297`), leaving `TPIDRRO_EL0 = TSD_IPA` untouched. A single-vCPU guest is always cpu 0 /
cluster 0, so `_os_cpu_number()` and `_os_cpu_cluster_number()` both read 0 and every per-CPU/
per-cluster index is in bounds. `TPIDR_EL0` is set below the trace and is identical on record and
replay (a fixed constant, like the PAC keys and synthetic timebase), so nothing enters the trace.

## Verified facts (confirmation spike, 2026-07-14)

Set `TPIDR_EL0 = 0` at both sites, rebuilt, ran the bounded traced `record-dyn hello_dyn`:
- **The xzone segment-group fault is GONE.** The run advances from ~205 traps to **223 traps**,
  past `_xzm_segment_group_alloc_chunk` — the segment-group indexing no longer overshoots.
- **No regression:** full `just gate` stays **68 passed / 0 failed / 1 ignored**, clippy clean, with
  the spike applied.
- **No earlier fault:** the run got *further*, not earlier — confirming `TPIDR_EL0` is never
  dereferenced as a TSD base by anything the guest exercises (that role is `TPIDRRO_EL0`'s).
- **New wall reached** (see below), i.e. the fix cleanly exposes the next boundary.

The spike was reverted; the tree is clean. These are the actual observed results, not projections.

## The new wall (M2-cpuid's honest boundary)

At ~223 traps the run hits: `RECORD ERROR: unsupported mach_msg2 at pc 0x1804abc34: msgh_id 3409
dest 0x203 (guest task port Some(515)) send_size 36` — an unhandled MIG message to the guest **task
port**. msgh_id 3409 is in the Mach **task** subsystem (base 3400), a small (36-byte) request —
almost certainly `task_get_special_port` / `task_set_special_port` or a neighbor (the implementer
confirms the exact routine from `task.defs` / the trace). This is a **mach-IPC servicing** task in
the M2-mach lineage (route/service one more MIG id), distinct from CPU identity — the next milestone,
not in scope here beyond re-parking the gate at it.

## Scope

**In:** `TPIDR_EL0 = 0` at both constructor sites + a comment correcting the TSD-conflation; a
micro-test proving the box presents cpu 0 / cluster 0; the gating spike is already done (results
above) — Task 1 lands the fix under test; the walk confirms the advance to the 3409 wall and re-parks
`hello_dyn_e2e` honestly there. README Status + memory update at close.

**Out / the honest edge:**
- **The msgh_id 3409 task-port MIG wall** — next milestone (M2-mach lineage).
- **Coherent single-vCPU commpage synthesis** — retrace `memcpy`s the entire *host* commpage into the
  guest (`lib.rs:856-861`), so `_COMM_PAGE_LOGICAL/PHYSICAL/ACTIVE_CPUS`, `_NCPUS`, `_CPU_CLUSTERS`,
  `_CPU_TO_CLUSTER[]`, `_MEMORY_SIZE`, `_DEV_FIRM`, etc. carry the host's 12-CPU/2-cluster values.
  This is a **latent host-topology leak**, but it is **not fatal and not a determinism bug**: the
  bytes are frozen into guest memory once at setup, so a record/replay pair sees identical bytes, and
  the oversized per-CPU arrays (sized for 12 CPUs / 2 clusters) are harmless once the *index* is
  pinned to 0 by this fix. Synthesizing a real single-vCPU commpage (counts = 1, pinned MEMORY_SIZE,
  DEV_FIRM policy) is the principled hygiene follow-up; deferred until a wall forces it or a dedicated
  hygiene pass, to keep this milestone tight and its fix isolated. Documented as known debt.
- **Forwarded-entropy/PID nondeterminism across record runs** — normal for record/replay (outputs are
  recorded per-trace and replayed); not a bug, not in scope.

## Exit criterion

The micro-test proves the box sets guest `TPIDR_EL0 = 0` (cpu 0 / cluster 0) on both the dynamic and
replay paths; `just gate` stays green (68 + the new test, honest ignore count), clippy clean. The
walk confirms the run passes `_xzm_segment_group_alloc_chunk` and re-parks `hello_dyn_e2e` at the
msgh_id 3409 task-port wall (or, if the walk somehow reaches `main`, un-ignore + double-replay — not
expected, since the spike already showed the 3409 wall). No faked green.

## The mechanism

`crates/retrace-box/src/lib.rs`, two sites, e.g.:

```rust
// TPIDRRO_EL0 = TSD base (dyld/libSystem read errno + pthread-self through it). TPIDR_EL0 is NOT a
// second TSD pointer: macOS 26 reads the current CPU number from TPIDR_EL0[11:0] and the cluster
// number from TPIDR_EL0[>=12] (_os_cpu_number / _os_cpu_cluster_number). A single-vCPU guest is
// always cpu 0 / cluster 0, so TPIDR_EL0 must be 0 — TSD_IPA (0x30000) would read as cluster 48 and
// blow xzone's per-cluster segment-group index out of bounds.
vcpu.set_sys(sysreg::TPIDRRO_EL0, TSD_IPA).unwrap();
vcpu.set_sys(sysreg::TPIDR_EL0,   0).unwrap();
```

Below the trace, identical on record and replay; no `retrace-core`/trace-format change.

## Components

- `crates/retrace-box/src/lib.rs` — `TPIDR_EL0 = 0` at `load_dynamic` (~:903) and `restore` (~:1297)
  + the corrected comment (also fix the stale header note at `lib.rs:55-57` that says both are set to
  `TSD_IPA`).
- `crates/retrace-guest` — a `cpuid.s` micro-guest (`mrs` `TPIDR_EL0`, exit with cpu/cluster) + build
  stanza + const, OR an in-process box test reading the vcpu reg after `load_dynamic`.
- `crates/retrace/tests/` (or `crates/retrace-box/tests/`) — the cpu-identity test.
- `crates/retrace/tests/hello_dyn_e2e.rs` — re-park the `#[ignore]` reason at the msgh_id 3409 wall.
- README Status + memory (`retrace-objc-preoptimization-wall` chain) at close.

## Testing

1. **CPU-identity test:** assert the box presents guest `TPIDR_EL0 == 0` after `load_dynamic` and
   after `restore` (an in-process box test reading `vcpu.get_sys(TPIDR_EL0)` is simplest and covers
   both paths; a `cpuid.s` guest that reads and exits with it is the e2e alternative). Confirms
   cpu 0 / cluster 0.
2. **Regression:** full `just gate` — 68 prior tests stay green (the spike already showed this), plus
   the new test; clippy clean. `TPIDR_EL0` in `restore` was documented "harmless for M1"; the change
   to 0 must remain harmless (verified by the gate).
3. **The walk:** bounded traced `record-dyn hello_dyn` reaches the 3409 task-port wall; re-park.

## Risk register

1. **Something reads `TPIDR_EL0` as a TSD base after the fix.** *Mitigation:* the spike advanced the
   run 18 traps further with no earlier fault and the gate stayed green, so nothing exercised does;
   `TPIDRRO_EL0` holds the real TSD base. If a later walk faults on a small offset off address 0,
   revisit — but the evidence is strong.
2. **cpu/cluster read from somewhere other than `TPIDR_EL0`** on some path. *Mitigation:* the fix is
   validated end-to-end by the spike clearing the exact xzone wall; other per-CPU consumers (nano
   magazines) index by cpu number (already 0 before and after) and are unaffected.
3. **The oversized host-count arrays bite later** (a consumer that iterates all `ncpus`/`nclusters`
   rather than indexing the current one). *Mitigation:* out of scope; the deferred commpage synthesis
   addresses it; the walk finds it if it exists.

## Open questions for implementation planning

1. Micro-test form: in-process `vcpu.get_sys(TPIDR_EL0)` assertion (simplest, covers both sites) vs a
   `cpuid.s` e2e. Lean: the in-process box test.
2. The exact task routine behind msgh_id 3409 (for an accurate re-park reason) — confirm from
   `task.defs` / the trace in Task 2; does not affect this milestone's fix.
