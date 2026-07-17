# retrace M4 — checkpointed reverse-execution seeks

**Design spec — 2026-07-16.** The first post-M3 milestone. M3 closed with `reverse_debug_e2e`
un-`#[ignore]`d and green ([M3 design](2026-07-16-retrace-m3-reverse-execution-design.md), merged
via M3-fastfollow, gate 97/0/0): position-coordinate `P=(N,K)` reverse debugging works end to end via
re-replay + hardware single-step, no checkpoints. M3 deliberately deferred checkpoints — "a pure
acceleration with meaningful machinery (committed-page copies, invalidation) ... until a guest's
replay time actually hurts" — and named the risk precisely: "huge trap windows make seeks slow
(dyld init runs millions of instructions between traps)." M4 builds that deferred acceleration,
proactively, ahead of upcoming guest-breadth work rather than in reaction to an observed slow case.

## The problem, precisely

Tracing the actual cost model (not the M3 spec's shorthand) through `ReplaySession` and the CLI's
`Exec` (`crates/retrace/src/debug.rs`) splits it into two very different costs:

1. **Landmark-to-landmark replay is cheap.** `ReplaySession::advance_to_landmark` runs `Box_::run()`
   at native VM speed between traps — the M3 spec's own claim ("242 events, sub-second") holds
   regardless of how large N gets. This is not the bottleneck.
2. **Intra-window single-stepping is expensive, and today it always restarts from K=0.**
   `step_insns`/`window_len_here` (`retrace-core/src/lib.rs:752,776`) retire one instruction per
   `hv_vcpu_run` call — a full HVF exit per instruction. Every movement in the CLI executor pays this
   from scratch: `Exec::reseek` (`debug.rs:147`), `resolve_hit_k` (`:113`), `probe_window_len`
   (`:157`), and `cmd_reverse_continue`'s per-hit scan loop (`:330`) each call `seek()`
   (`retrace-core/src/lib.rs:796`), which unconditionally opens from the trace's landmark-0 snapshot
   and single-steps from K=0 — even to reach a K it has visited before in the same session.
   `cmd_reverse_continue` is the worst case: it pays a fresh `resolve_hit_k` single-step walk for
   *every* breakpoint hit found on the way back to P.

So the acceleration target is not "replay more landmarks faster" — it's "never re-single-step a
window prefix this session has already paid for."

## Verified facts (this repo — read directly, HEAD `9267b7a`)

- **`ReplaySession`** (`retrace-core/src/lib.rs:378`) carries five fields: `b: Box_`, `events:
  Vec<Event>`, `idx: usize`, `stdout: Vec<u8>`, `guest_task_port: Option<u64>` (plus a diagnostic
  `truncated: bool`). `seek(trace_path, n, k)` (`:796`) = `ReplaySession::open` (restore from the
  leading `Event::Snapshot`, landmark 0) → `advance_to_landmark(n)` → `step_insns(k)`.
- **`Box_::restore`** (`retrace-box/src/lib.rs:1461`) is the only existing "rebuild guest from
  captured state" path, and it is correct **only at landmark 0**: it explicitly resets
  `reservations: Vec::new()`, `mmap_next: MMAP_BASE`, `bootstrap_port: None`, `cache: None`
  (`:1510`) rather than restoring them, and recomputes `l2_host`/`next_l3` by scanning the
  freshly-allocated `backings` for the page-table region (`:1495-1507`). These defaults are only
  correct because at landmark 0 the guest hasn't allocated anything yet — restoring them verbatim at
  a mid-run position would be wrong.
- **`Box_::snapshot()`** (`:1658`) and the trace-format `Regs` type (`crates/retrace-trace/src/lib.rs:7`,
  `{ x: [u64;31], pc, sp_el0, cpsr }`) capture **no FP/SIMD state** — no V0-V31, FPCR, or FPSR. This
  is invisible today because it's only ever exercised at landmark 0, where a fresh process has zeroed
  FP state by construction. dyld/libSystem use NEON in early code (memcpy, hashing); a mid-run
  checkpoint that zeroes FP state on restore would silently diverge from a cold `seek()` to the same
  position the moment an FP-derived value reaches architectural state a test can observe.
- **hv-sys does not yet wrap FP/SIMD registers**, but the raw bindgen bindings already exist: FPCR/FPSR
  are ordinary `hv_reg_t` values (`HV_REG_FPCR = 32`, `HV_REG_FPSR = 33`), readable/writable through
  the *same* `hv_vcpu_get_reg`/`hv_vcpu_set_reg` calls already wrapped for `PC`/`CPSR` (`hv-sys/src/lib.rs:44-51`).
  V0-V31 need a distinct pair, `hv_vcpu_get_simd_fp_reg`/`hv_vcpu_set_simd_fp_reg` over
  `hv_simd_fp_reg_t_HV_SIMD_FP_REG_Q0..Q31` (128-bit `hv_simd_fp_uchar16_t = u128`), both already
  present in the generated FFI — only the safe wrapper layer is missing.
- **`cache: Option<CacheMeta>`** (`retrace-box/src/lib.rs:251`) is not `Clone` — `CacheMeta`/`Subcache`
  own a `File` handle (`cache.rs:216-221,240-255`) — so it cannot be captured verbatim into an owned
  checkpoint struct. `install_cache_pager()` (`:586`) rebuilds it via `CacheMeta::load(DEFAULT_CACHE_PATH)`,
  a deterministic, cheap (subcache *headers* only, not the multi-GB payload) function of the fixed
  on-disk file and fixed keys.
- **All sysregs `Box_::restore` re-establishes are EL1** (`MAIR_EL1`, `TCR_EL1`, `TTBR0_EL1`,
  `CPACR_EL1`, `TPIDRRO_EL0`, PAC keys, `SCTLR_EL1`, `VBAR_EL1`) — the EL0 guest cannot mutate them, so
  reapplying them as fixed constants at any position is unconditionally correct. `TPIDR_EL0` is the one
  exception: it's forced to `0` (`:1480`, the M2-cpuid fix) but is EL0-writable and not part of `Regs`
  — cheap to capture/restore like any other sysreg rather than assume it's never written.
- **Guest memory is anonymous, page-table-backed** (`Vec<Region>` inside a snapshot) — a full deep
  copy of every backing. At `hello_dyn` scale a landmark-0 snapshot is a few MB (dyld's mapped
  segments dominate); a run reaching `main` has demand-paged hundreds to low-thousands of 16 KiB
  shared-cache pages on top, so a mid-run checkpoint is estimated in the **tens of MB**, not KB.
- **HVF is one-VM-per-process.** `Exec` already drops its live `ReplaySession` before opening a new
  one (`self.session = None;` ahead of every `seek()` call) — checkpoint restore must follow the same
  discipline; there is no way to hold two live VMs to compare or interpolate between.

## The mechanism

### M4-state — `BoxState` (retrace-box)

A new **in-memory-only** (not trace-format — zero `TRACE_MAGIC`/format change, same discipline as
M3) struct capturing `Box_`'s complete internal state at an arbitrary position, where `Box_::snapshot`
captures only enough for the landmark-0 case:

```rust
pub struct BoxState {
    regs: Regs,                  // x0..x30, pc, sp_el0, cpsr — existing trace-format type
    fp: [u128; 32],               // V0..V31 — NEW
    fpcr: u64, fpsr: u64,         // NEW
    tpidr_el0: u64,                // captured, not assumed zero
    mem: Vec<Region>,             // existing trace-format type — full backing contents
    reservations: Vec<Reservation>,
    mmap_next: u64,
    bootstrap_port: Option<u64>,
    cache_installed: bool,        // NOT the CacheMeta itself (not Clone; re-derived on restore)
    last_far: u64,
    synthetic_tsc: u64,
    cache_refault_ipa: u64,
    cache_refault_count: u64,
}
```

- `Box_::checkpoint(&self) -> BoxState` — reads every field above off the live vcpu/memory (new
  `hv-sys` FP/SIMD wrapper calls for `fp`/`fpcr`/`fpsr`; everything else already has an accessor or a
  direct field read).
- `Box_::from_checkpoint(&BoxState) -> Box_` — mirrors `Box_::restore`'s structure (fresh
  `hv_vm_create`/`hv_vcpu_create`, remap every backing, reapply the fixed EL1 sysregs, PAC keys,
  **and now `set_trap_debug_exceptions(true)`, which `Box_::restore` does at `:1484` and which this
  path must not omit** or `step()` silently stops trapping) but restores the *true* captured values
  for the fields `restore` defaults, restores `fp`/`fpcr`/`fpsr`/`tpidr_el0` via the new wrappers, and
  calls `install_cache_pager()` iff `cache_installed` (re-deriving `CacheMeta`, not deserializing it).
  `l2_host`/`next_l3` are recomputed post-restore from the freshly-allocated `backings`, exactly as
  `Box_::restore` already does — never stored as raw fields (they're host pointers, meaningless across
  VM instances).

### M4-cache — `SessionCheckpoint` + `CheckpointCache` (retrace-core)

```rust
pub struct SessionCheckpoint { box_state: BoxState, idx: usize, guest_task_port: Option<u64> }
```

`idx` and `guest_task_port` are the only `ReplaySession` fields that vary by position (confirmed by
reading every `&mut self` mutation across `advance()`). `stdout` is deliberately **not** captured —
nothing in `debug.rs` reads it (`cmd_continue`/exit paths print only `exit_code`); `truncated` is
re-derived by re-`open_checked`, a trace-level constant. Hardware breakpoint registers and SS arm/disarm
state are **not** part of a checkpoint — they're transient debugger config `Exec` re-applies after
obtaining *any* session, cold or checkpointed (`cmd_continue` already calls `arm_breakpoints` fresh
post-seek); capturing them would conflate debugger configuration with recorded-execution state.

```rust
pub struct CheckpointCache { /* BTreeMap<(usize, u64), SessionCheckpoint> + byte-budget/LRU ledger */ }
```

Keyed by `(N, K)` in trace-execution order — `(N, K)` tuple ordering matches true execution order
exactly (window N wholly precedes window N+1; within a window K only increases), so "best checkpoint
at or before a target" is `range(..=(n,k)).next_back()`. Owned by `Exec`, scoped to one trace for the
CLI session's lifetime — never persisted, never shared across traces or sessions. Given the ~tens-of-MB
per-entry cost, a realistic byte budget (a few hundred MB) holds on the order of 10-50 entries, not
hundreds — sized for an interactive session's working set of "positions near the current
investigation," not for full-trace coverage.

### M4-seek — `checkpointed_seek` (retrace-core)

```rust
pub fn checkpointed_seek(trace: &Path, cache: &mut CheckpointCache, n: usize, k: u64)
    -> Result<ReplaySession, String>
```

Same signature contract as today's `seek()` — the cache is purely an accelerator; a miss always falls
back to the existing cold path, so no new failure mode reaches callers.

1. **Lookup:** `cache.range(..=(n,k)).next_back()`.
2. **Resume**, by case (exhaustive — the range bound guarantees no hit is ever later than the target):
   - *Hit, same window* (`N' == n`, so `K' <= k`): `Box_::from_checkpoint` → `step_insns(k - K')` only.
     No landmark replay at all.
   - *Hit, earlier window* (`N' < n`): `Box_::from_checkpoint` → `advance_to_landmark(n)` (only the
     remaining landmarks) → `step_insns(k)`.
   - *Miss*: cold path — `ReplaySession::open` → `advance_to_landmark(n)` → `step_insns(k)`.
3. **Cost-gated insert:** the resume path above already knows the **single-step count** paid during
   this call (landmark count is deliberately excluded from the gate — landmark replay is native-speed
   and cheap per the verified facts above; gating on a blended sum would let a large-N/K=0 seek trip
   the same threshold as one that paid hundreds of real single-steps). If that count exceeds a fixed
   threshold, store `session.checkpoint()` at `(n, k)`, evicting LRU entries first if the byte budget
   would be exceeded.

Every raw `seek()` call site in `debug.rs`'s `Exec` — `reseek`, `resolve_hit_k`, `probe_window_len`,
and `cmd_reverse_continue`'s scan loop — switches to `checkpointed_seek`. Tracing `cmd_reverse_continue`
end to end: each `resolve_hit_k` single-steps deep into a window to pin an exact K (over the gate
threshold, so it's checkpointed at `(n,k)`); the very next outer-loop iteration seeks to `(n, k+1)` —
a guaranteed cache hit needing one more step, instead of redoing the whole single-step walk from the
window's start. This is exactly the pattern the M3 risk register flagged, and the one this design
accelerates most.

**Non-goal:** the *first* visit to any position is exactly as expensive as today — there's nothing to
reuse yet. This accelerates *revisits*, which is what a debug session naturally produces (stepping
around a breakpoint, resolving successive nearby hits).

## Correctness invariant

Directly generalizing M3's existing divergence oracle ("seeking the same P twice yields byte-identical
state"): **a checkpoint-restored-and-continued session must produce byte-identical results to a cold
`seek()` to the same downstream (N,K).** No invalidation logic is needed — a checkpoint's validity
depends only on (trace file, position), both fixed for an `Exec`'s lifetime — only eviction, for space.
LRU bookkeeping must itself be deterministic (a generation counter, not hash-iteration order), or
checkpointing could silently reintroduce exactly the nondeterminism this project bans by policy
(`clippy.toml`'s wall-clock/thread denials).

## Scope

**In:** `BoxState` (full `Box_` capture incl. FP/SIMD, `cache_installed: bool`, captured `TPIDR_EL0`,
recomputed `l2_host`/`next_l3`); new `hv-sys` FP/SIMD register wrappers (`reg::FPCR`/`FPSR` via the
existing `hv_vcpu_get/set_reg`, a new `simd` module over `hv_vcpu_get/set_simd_fp_reg`);
`Box_::checkpoint`/`from_checkpoint`; `SessionCheckpoint`; `CheckpointCache` (byte-budget + LRU,
cost-gated on single-step count); `checkpointed_seek` wired into all four `debug.rs` call sites; the
test suite below; README Status + memory update at close.

**Out / named, not forgotten:** persisting checkpoints to disk or the trace format (session-scoped,
ephemeral only); cross-session or cross-trace checkpoint sharing; an eager coverage-guaranteed
checkpoint ladder (considered and rejected — a landmark-measured stride skips exactly the intra-window
positions that are expensive, and building a mid-window-aware ladder up front would re-pay the cost
this feature exists to avoid); any record-side change; watchpoints, symbolication, interactive REPL,
reverse-next/finish (already-deferred M3 items, untouched here); a user-facing config knob for the
cost threshold or byte budget (ship a sane internal constant; configurability is a possible follow-on,
not required now).

## Exit criterion

All tests below green; full existing gate (97 baseline + new tests) green throughout; clippy clean.
`reverse_debug_e2e`'s two-independent-sessions transcript-identity check stays green with checkpointing
wired in — checkpointing must be invisible to CLI output. If FP/SIMD register access under HVF does not
behave as documented (unexpected — this is a standard, documented API, not an empirically-uncertain
platform claim — but the codebase's honest-gate discipline applies regardless), park at the documented
boundary rather than fake green.

## Testing

1. **`CheckpointCache` unit tests** (no VM): insert past budget, verify oldest-unused evicted first,
   byte accounting stays bounded.
2. **Capture/restore round-trip:** `Box_::checkpoint()` immediately followed by `Box_::from_checkpoint()`
   at a mid-run position, byte-compared against the original `Box_` (regs incl. FP, full memory) with
   zero further execution — proves the capture pair itself is lossless before any seek logic sits on
   top.
3. **Checkpointed vs. cold seek:** from a warm cache, `checkpointed_seek(trace, cache, n2, k2)` vs. a
   cold `seek(trace, n2, k2)` — byte-identical `dbg_regs` (extended for FP) + `diff_memory`. Must
   deliberately cross a NEON-using window (dyld's early code) so the FP capture gap can't silently
   regress.
4. **Synthetic large-window guest:** a new small guest program with one deliberately huge window (a
   spin loop between two syscalls), so cache-hit behavior has something real to accelerate. Speedup
   proof is an **instrumented single-step counter, not wall-clock** (this project bans timing-based
   assertions by policy) — assert a second nearby seek retires meaningfully fewer steps than the first.
5. **Regression:** `reverse_debug_e2e`'s transcript-determinism check stays green; additionally, run
   the same script twice from a cold `Exec` (fresh cache both times) and assert identical transcripts —
   proves checkpointing introduces no cache-dependent nondeterminism. Full gate green throughout,
   `--test-threads=1` discipline unchanged.

## Risk register

1. **FP/SIMD wrapper is new hv-sys surface, unexercised until now.** *Mitigation:* the raw FFI bindings
   already exist (verified above); the safe wrapper is a small, mechanical addition mirroring the
   existing `PC`/`CPSR`/`x(n)` pattern. Test 2 (capture/restore round-trip) catches any mistake before
   test 3 (cold-vs-checkpointed) would surface it as a confusing divergence.
2. **Missing a `Box_` field in `BoxState`.** *Mitigation:* the field list above was cross-checked
   against the full `Box_` struct definition field-by-field (not by pattern-matching the restore
   function); test 2's byte-compare round-trip is the backstop — any missed field shows up immediately
   as a mismatch, not a silent later divergence.
3. **`cache_installed` re-derivation drifts from a true clone.** `CacheMeta::load` must be exactly as
   deterministic as `install_cache_pager()` already assumes it is (it's already relied on for
   record/replay symmetry) — no new risk beyond what M2-cache already proved.
4. **Per-checkpoint memory cost makes the byte-budget too coarse to be useful** (tens of MB means a
   modest budget holds few entries). *Mitigation:* cost-gating on single-step count (not "any visited
   position") means only genuinely expensive-to-reach positions get stored, so a small cache still
   targets the highest-value entries; the byte budget is a named tunable constant, not hardcoded
   invisibly, so it's a one-line change if a future guest's working set needs more headroom.
5. **The 6-hardware-breakpoint / debug-register interaction with a freshly restored vcpu.** *Mitigation:*
   confirmed breakpoints are never part of checkpoint state — `Exec` always re-arms them post-seek
   regardless of whether the session came from the cache or cold, so this is unchanged from M3's
   existing behavior.

## Components

- `crates/hv-sys/src/lib.rs` — `reg::FPCR`/`reg::FPSR` (via existing `hv_vcpu_get/set_reg`), a new
  `simd` module wrapping `hv_vcpu_get/set_simd_fp_reg` over `HV_SIMD_FP_REG_Q0..Q31`.
- `crates/retrace-box/src/lib.rs` — `BoxState`, `Box_::checkpoint()`, `Box_::from_checkpoint()`.
- `crates/retrace-core/src/lib.rs` — `SessionCheckpoint`, `CheckpointCache`, `checkpointed_seek()`.
- `crates/retrace/src/debug.rs` — `Exec` gains a `cache: CheckpointCache` field; `reseek`,
  `resolve_hit_k`, `probe_window_len`, `cmd_reverse_continue`'s scan loop switch to `checkpointed_seek`.
- `crates/retrace-guest` — a new small guest program (asm, following the existing `asm/*.s` pattern)
  with one deliberately huge single-window spin loop, for the synthetic large-window test.
- New tests: `CheckpointCache` unit tests, capture/restore round-trip, checkpointed-vs-cold-seek
  byte-compare (NEON-crossing), synthetic large-window speedup (step-counter based), cold-double-session
  transcript regression. README Status + memory at close.

## Open questions for implementation planning

1. Exact single-step cost-gate threshold and byte-budget constant — pick conservative starting values
   during implementation (both named, tunable constants, not user-facing config per Scope) and confirm
   against the synthetic large-window test's actual behavior rather than guessing blind.
2. Whether `BoxState`'s `Vec<Region>` should reuse the trace-format `Region` type as-is or a
   checkpoint-local equivalent — reuse is simpler unless the trace format's `Region` carries anything
   checkpoint-inappropriate (unlikely; decide at implementation).
3. Whether the LRU generation counter lives inside `CheckpointCache` itself or is threaded in from
   `Exec` — an implementation-level detail that doesn't affect this design's external behavior.
