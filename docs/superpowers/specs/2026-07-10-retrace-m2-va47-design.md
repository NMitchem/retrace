# retrace M2-va47 — 47-bit guest VA (clear the objc arm64e ISA-mask wall)

**Design spec — 2026-07-10.** Sub-milestone of M2 (the loader), sibling of M2-cache and M2-mach —
completing the loader so a real dynamically-linked arm64 program runs end-to-end. (Named M2-va47,
not M3: M3 is the positioning milestone; this is loader-completion work.) Clears the boundary
documented in the memory
`retrace-guest-va-objc-isa-mask-wall` and `.superpowers/sdd/task-m2mach-7-report.md`: past the
re-signed shared cache and the mach-IPC servicing (M2-mach, merged `df0a0f6`), real dyld runs
~208 traps deep into libSystem init and aborts in Objective-C class realization
(`_map_images_nolock → addClassTableEntry`) dereferencing a **poisoned isa**.

## What this is

The box runs a **36-bit guest VA** (`TCR_EL1.T0SZ=28`). `hello_dyn` is a plain **arm64** (not
arm64e) process, so libobjc **strips** the shared cache's arm64e isa pointers with a compile-time
**47-bit `ISA_MASK`** (`0x00007FFFFFFFFFFF`) instead of authenticating them. Under T0SZ=28 the
hardware PAC signature occupies VA bits **[54:36]**, so its low bits **[46:36] survive** objc's
47-bit strip → a poisoned pointer → data abort. M2-va47 widens the guest VA to **47 bits**
(`T0SZ=17`, a 3-level stage-1 walk) so the PAC signature lands in bits **[54:47]**, entirely
**above** objc's mask, making the strip lossless. Exit: `hello_dyn_e2e` un-ignored and replaying
byte-for-byte.

This targets the project's actual headline goal — recording a real, plain-**arm64** program
(Homebrew `python3`) — which the arm64e-guest alternative would not generalize to (and which
third-party arm64e builds are gated against anyway).

## Verified facts (this host, macOS 26.x arm64e)

- **Current stage-1** (`crates/retrace-box/src/lib.rs`): 16 KiB granule (`GRANULE=0x4000`),
  `TCR_EL1_V=0x1_0080_B51C` (T0SZ=28 / TG0=16K / IPS=36-bit), MMU-on via `SCTLR_MMU_ON`. Two-level
  walk: `TTBR0 → L2` (one 16 KiB page at `PT_L2_IPA=0x8000`, 2048 entries, each `BLK=1<<25`=32 MiB)
  `→ L3` (promoted per exec range). `build_tables` builds the L2 and promotes exec ranges;
  `restore` re-points `TTBR0=PT_L2_IPA` without rebuilding (the tables ride in the snapshot).
- **The boundary is arithmetic-confirmed** (M2-mach Task 7 review): the poison isa
  `0xc92bd741ec2f1528 & 0x00007FFFFFFFFFFF = 0x5741ec2f1528`, exactly the observed faulting
  `far/ipa`. `0x6ae1` is objc's genuine isa signing discriminator.
- **The cache re-signing is proven correct in isolation** — in-guest `pacda`-sign → `autda`
  round-trips exactly (M2-cache). The M2-mach objc failure is purely PAC *bit placement* (VA
  size), not a signing error.
- **16 KiB granule level math**: page offset 14 bits; each table level 2048 entries (11 bits).
  L3 spans 2^25 (32 MiB), L2 spans 2^36 (64 GiB), L1 spans 2^47. T0SZ=28 (36-bit VA, bits [35:0])
  → start level L2 (2 levels). T0SZ=17 (47-bit VA, bits [46:0]) → start level **L1** (3 levels).
  Since every mapped IPA is < 2^36, only **L1[0]** is ever valid.

## Scope

**In:** insert one L1 table (16 KiB, entry[0] → the existing L2, rest invalid); repoint `TTBR0` at
it; set `T0SZ=17` (`TCR_EL1_V` → `0x1_0080_B511`); apply universally (all guests, one config);
carry the L1 in the snapshot so `restore` reproduces it; a **gating spike** proving the objc strip
is lossless under 47-bit; the **empirical walk** past objc to `main→write→exit`; un-ignore
`hello_dyn_e2e` + a double-replay determinism test.

**Out (deferred):** an arm64e guest path; 4 KiB-granule VA layouts; any objc/runtime feature not
demanded by the trivial `write()`-only gate; the swarm extension to the dyld guest (stretch,
contingent on measured wall-clock, same as M2-mach's deferral). If a new *distinct* boundary
appears past objc (e.g. a hard system-daemon dependency), it is documented and deferred to a
further milestone — the gate stays `#[ignore]`d with an updated reason rather than faked.

## Exit criterion

`hello_dyn_e2e` un-ignored and green — record prints `hi\n`; replay reproduces stdout
byte-for-byte; per-syscall and final full-memory checks pass — plus a double-replay stability
test. The entire existing suite (M0/M1/M2/M2-cache/M2-mach) stays green **under the widened VA
config** (the regression gate), and `clippy -D warnings` is clean. If honestly blocked past objc,
the milestone lands as DONE_WITH_CONCERNS with the new boundary's anatomy documented and the gate
kept `#[ignore]`d.

## The mechanism

### 1. Insert an L1 table (`build_tables`)

Allocate a fresh 16 KiB page at a new fixed IPA (`PT_L1_IPA`), inside block 0 alongside the other
page-table pages (below `PT_L3_BASE`, clear of `PT_L2_IPA=0x8000` / stack / TSD / sign scratch).
Fill entry[0] = `PT_L2_IPA | DESC_TABLE`; leave entries [1..2048] zero (invalid → translation
fault if a VA ≥ 2^36 is ever touched, which never happens). Return `PT_L1_IPA` as the `TTBR0`
value instead of `PT_L2_IPA`. The L2/L3 build is otherwise unchanged. The L1 page is registered as
a backing so it rides in the snapshot.

### 2. Flip T0SZ (`TCR_EL1_V`)

`TCR_EL1.T0SZ` is bits [5:0]: `0x1C`(28) → `0x11`(17). `TCR_EL1_V: 0x1_0080_B51C → 0x1_0080_B511`.
IPS (output/IPA size) stays 36-bit — stage-2 is unchanged. Applied at every site that sets
`TCR_EL1` (`load`, `load_dynamic`, `restore`).

### 3. TTBR0 = the L1, everywhere

Every `TTBR0_EL1` write (`load`, `load_dynamic`, `restore`) points at `PT_L1_IPA`. `restore`'s
"re-point, do not rebuild" path (which sets `TTBR0=PT_L2_IPA` today) points at the L1 instead; the
L1 backing is already present from the snapshot, so no rebuild / double-map occurs.

### 4. Universal config + regression gate

One VA config for all guests (no loader/restore branching on guest type). An early task runs the
**full existing suite under the widened config** as a regression gate — the M0/M1 freestanding
guests use only low addresses (< a few MiB) and the commpage (`0xF_FFFF_C000` < 2^36), so a 47-bit
VA identity-maps them identically. PAC stays self-consistent: pacguest and the cache re-signing
sign *and* authenticate under the same T0SZ, so they round-trip regardless of VA size; the spike
proves objc's *external* strip now agrees. Only if an M0/M1 guest regresses do we fall back to a
dynamic-path-only config (restore branches on a snapshot-recorded VA flag) — a documented contingency, not the plan of record.

### 5. Determinism

The L1 table is a pure function of the fixed layout (one entry → L2), identical on record and
replay; it rides in the snapshot like every other table page, so `restore` reproduces it exactly.
No new nondeterminism enters the trace. The widened VA changes PAC bit placement identically on
both sides, so re-signed cache pages (regenerated by the deterministic pager) and their
authentication/strip are bit-identical across record and replay.

## The spike (gating Task 1)

Minimal/throwaway: set `T0SZ=17` + the L1 table in the dynamic load path, boot the dynamic guest,
and confirm the run advances **past objc class realization** — no poisoned-isa abort at
`addClassTableEntry`, trap count advances beyond ~208. **Go/no-go:** if the strip is still lossy
(objc still aborts on a poison isa), the VA theory is wrong — STOP and reconsider (arm64e guest,
or a deeper PAC-placement analysis) before building the walk. Record the observed pre/post trap
counts and the first failure past objc (the walk's starting point) in the spike report.

## Components

- `crates/retrace-box/src/lib.rs` — `PT_L1_IPA` const; `build_tables` L1 insertion + return
  `PT_L1_IPA`; `TCR_EL1_V` T0SZ=17; `TTBR0_EL1` = `PT_L1_IPA` at all three sites; `restore`
  re-point. Possibly a small L1-backing addition to the snapshot region set (if not automatic).
- `crates/retrace/tests/hello_dyn_e2e.rs` — remove `#[ignore]`, rewrite the stale comment, add the
  double-replay test (on success).
- Any per-wall fixes the walk surfaces (mirrored record/replay, committed separately).
- README + main-spec milestone note; memory update at close.

## Testing

1. **Spike assertion** (Task 1): the dynamic run clears objc (trap count past ~208, no poison-isa
   abort) — the go/no-go gate.
2. **Regression gate**: full existing suite green under the widened VA config (`just m1`,
   `--test-threads=1`) — proves M0/M1/M2/M2-cache/M2-mach unaffected. PAC round-trip (pacguest,
   sign_oracle) still green under T0SZ=17.
3. **The gate**: `hello_dyn_e2e` un-ignored + double-replay determinism test.
4. `clippy -D warnings` clean.

## Risk register

1. **The VA theory is wrong** (widening doesn't make objc's strip lossless). *Mitigation:* the
   gating spike catches this immediately, before any build investment; the arithmetic
   (`poison & 47-bit-mask == far`) strongly predicts success, but the spike is empirical proof.
2. **A 47-bit VA regresses an M0/M1 guest.** *Mitigation:* the regression gate runs the full suite
   under the new config early; documented fallback to a dynamic-only config if needed.
3. **Walls past objc** (more objc/libdispatch/xpc init before `main`). *Mitigation:* the empirical
   walk with fail-loud triage (M2-mach method); a new distinct boundary is documented and deferred,
   not faked.
4. **HVF rejects T0SZ=17 for the guest's TCR_EL1.** *Mitigation:* TCR_EL1 is guest-controlled and
   47-bit VA is a standard config; the spike confirms empirically. Assert-and-fail loudly on any
   translation setup error rather than mis-mapping.
5. **PAC placement subtlety** (TBI / bit-55 sign-select interactions differ from the [54:47]
   prediction). *Mitigation:* the spike observes the actual post-strip isa; if [54:47] is not the
   whole story, the spike names the real bit layout before we commit.

## Non-goals / explicitly deferred

arm64e guest support; 4 KiB-granule layouts; objc/runtime features beyond the trivial gate; swarm
extension to the dyld guest; anything past the first *distinct* non-objc boundary.

## Open questions for implementation planning

1. `PT_L1_IPA` exact value (a free fixed IPA in block 0 below `PT_L3_BASE`, clear of existing
   reserved pages).
2. Whether the snapshot region set picks up the L1 backing automatically or needs an explicit add
   (determined by reading the snapshot code in Task 1/2).
3. Whether the spike surfaces one wall past objc or several (sizes the walk; on-demand per M2-mach).
