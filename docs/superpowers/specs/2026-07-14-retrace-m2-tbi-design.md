# retrace M2-tbi — arm64e data-pointer PAC placement (guest TCR TBI)

**Design spec — 2026-07-14.** Sub-milestone of M2 (the loader), sibling of M2-cache, M2-mach,
M2-va47, and M2-bfam. It corrects a **misdiagnosed** wall: what the M2-bfam close-out documented as
"objc shared-cache preoptimization" (memory `retrace-objc-preoptimization-wall`, README M2-bfam
Status, `.superpowers/sdd/task-m2bfam-2-report.md`, and the `hello_dyn_e2e` `#[ignore]` reason) is
**not** an objc-opt / cache-trust subsystem gap. It is a one-line guest-MMU bug: the guest's
`TCR_EL1` leaves **TBI off**, so re-signed data-pointer PACs occupy bit 63 and collide with objc's
`FAST_IS_RW_POINTER` flag. The fix and its outcome are **already confirmed by an investigation spike**
(2026-07-14) — this spec records the root cause, the fix, and the new boundary the fix exposes.

## What this is

Past the re-signed shared cache (M2-cache), mach-IPC servicing (M2-mach), the 47-bit guest VA
(M2-va47), and the B-family strip-on-FPAC arm (M2-bfam), real dyld reaches Objective-C class
realization and objc self-aborts (exit 134) in
`_objc_init → map_images → map_images_nolock → realizeClassWithoutSwift → validateAlreadyRealizedClass`:

```
objc[…]: realized class 0x1ec2f1618 has corrupt data pointer: malloc_size(0x1ed950f80) = 0
```

The M2-bfam close-out read this as objc dynamically realizing a "preoptimized cache-resident
`class_rw_t`" that legitimately isn't a heap allocation, and concluded the guest needed a
trusted objc-optimized cache (`objc_opt` header + hash tables + cache-trust). **That is wrong.**
Source research (`.superpowers/sdd/objc-preopt-research.md`, verbatim from `objc4-951.7` /
`dyld-1378`) and empirical symbolication show:

- The fatal class `0x1ec2f1618` is **`NSObject`**, and its `data()` pointer `0x1ed950f80`
  symbolicates (guest coords, libobjc `__TEXT` @ `0x18008C000`) to **`_OBJC_CLASS_RO_$_NSObject` —
  a `class_ro_t`, not a `class_rw_t`.** A correctly-realized class never has `data()` point at its
  own read-only `class_ro_t`; this only happens if objc took the already-realized branch on a class
  that is actually **unrealized**.
- `validateAlreadyRealizedClass` (`objc-runtime-new.mm:2942`) is an **unconditional**
  `malloc_size(rw) >= sizeof(class_rw_t)` on macOS — **no** `inSharedCache` / cache-range / trust
  guard exists to satisfy. There is **no cache-resident `class_rw_t`** in this ABI: `RW_REALIZED` /
  `setData` is set at exactly 3 objc4 sites, all `calloc`-backed, 0 in dyld; the objc_opt v16 wire
  format has no pre-realized-rw table.
- The host runs `hello_dyn` **fine** with `OBJC_DISABLE_PREOPTIMIZATION=YES` — preopt fully disabled
  is not this fatal. So "guest preopt disabled → dynamic realization → this fatal" is disproven.

The real mechanism: objc's `has_rw_pointer()` / `isRealized()` (`objc-runtime-new.h:2444`) reads
**bit 63** (`FAST_IS_RW_POINTER = 0x8000000000000000`) of the **raw** `class_data_bits_t::bits`
word in guest memory. The observed guest value is `0x964a8001ed950f80` — **bit 63 set** (top byte
`0x96`). So objc reads unrealized `NSObject` as already-realized, skips realization, and validates
its `data()` (the `class_ro_t`, `malloc_size` = 0) → fatal. On real hardware that same `bits` word
has bit 63 clear.

Bit 63 is polluted because the guest's PAC field extends into the top byte. The shared cache's v5
slide-info re-signer (A-family only) re-signs the `class_data_bits` slot as an A-family auth pointer
and stores it in guest memory; objc reads bit 63 of that stored value for `has_rw_pointer` **before**
the DB-key `autdb` (which M2-bfam strips) ever runs. Under the current `TCR_EL1.TBI0 = 0` with a
47-bit VA, the PAC field is bits [63:56] ∪ [54:47] — **including bit 63** — so the re-signed
pointer's PAC lands on objc's realized-flag. This slipped past every prior wall because the box
signs and authenticates with its own keys (internally symmetric); the break surfaces only when objc
reads the **raw** bits and treats bit 63 as a semantic flag — a guest-vs-host ABI mismatch.

## The fix

Match Apple's arm64e user configuration: enable **TBI0 (bit 37)** and **TBID0 (bit 51)** in the
guest `TCR_EL1`:

```
TCR_EL1_V: 0x1_0080_B511  →  0x8_0021_0080_B511
```

`TBI0 = 1` gives data pointers top-byte-ignore, so their PAC is placed in bits [54:47] and the top
byte (including bit 63) is preserved from the original canonical pointer = 0. `TBID0 = 1` exempts
**instruction** pointers from TBI, keeping their PAC full-strength in the top byte (Apple's TBID
posture). Result: a re-signed data pointer's bit 63 stays 0, `has_rw_pointer()` correctly reads
`NSObject` as unrealized, objc realizes it normally, and the `validateAlreadyRealizedClass` fatal
disappears. This is a **load-bearing MMU invariant** in the same class as W^X / `T0SZ` — the value
is a single constant (`TCR_EL1_V`) consumed by all three CPU-init sites (`load`, `load_dynamic`, and
the swarm path).

**Determinism:** `TCR_EL1` is set in the shared `load` / `load_dynamic` path (below the trace),
identical on record and replay — same posture as every other CPU-init sysreg. Nothing enters the
trace.

## Verified facts (spike, 2026-07-14)

- **The fix clears the wall.** With `TCR_EL1 = 0x8_0021_0080_B511`, a bounded `record-dyn hello_dyn`
  run **no longer** hits the objc `validateAlreadyRealizedClass` SIGABRT (exit 134). Instead it
  advances to a **new, distinct** wall (see below), exit 4 via retrace's clean `RECORD ERROR` path.
- **No regressions.** Full `just gate` stays **58 passed / 0 failed / 1 ignored**, clippy clean —
  the TCR change perturbs no existing test (mmu, pac, sign_oracle, cache_pager, dyld_load,
  bfamstrip, strip47, seeded_swarm all green).
- **Bit-63 arithmetic** confirmed against `objc4-951.7`: `FAST_IS_RW_POINTER = 0x8000000000000000`;
  guest `bits = 0x964a8001ed950f80` has bit 63 set; `has_rw_pointer` is a plain
  `bits & FAST_IS_RW_POINTER`.
- **TCR decode:** current `0x1_0080_B511` = T0SZ 17 (47-bit VA), IPS 0b001 (36-bit IPA), TBI0 = 0,
  TBID0 = 0. Fixed value sets bits 37 and 51 only; T0SZ / IPS / cacheability / share unchanged.

## The new wall (M2-tbi's honest boundary)

With classes realizing correctly, objc heap-allocates each `class_rw_t` via
`objc::zalloc<class_rw_t>() → calloc → libmalloc → mmap`, then touches the allocation and faults:

```
RECORD ERROR: non-syscall exit: data abort (EC=0x24 ISS=0x7 FSC=0x7)
              far/ipa=0xa0010e744 (UNMAPPED) pc=0x1802f5590 elr=0x1800ea230
```

`0xa0010e744` = `MMAP_BASE (0xA_0000_0000) + 0x10e744` — a **level-3 translation fault (FSC=0x7)**
on a page in the mmap allocation region that libmalloc obtained (an anonymous `mmap`) but retrace
reserved without backing/committing. This is a **demand-commit** gap: retrace needs to back
first-touched pages in `[MMAP_BASE, …)` with anon memory (and, on record, capture the zero-fill as
writes so replay reproduces it). That is a memory-management task, materially smaller than the
objc-opt subsystem the misdiagnosis feared, and is the next milestone — **not** in scope here beyond
re-parking the gate at it.

## Scope

**In:** the `TCR_EL1_V` constant change (`0x1_0080_B511 → 0x8_0021_0080_B511`) with a comment
explaining the TBI0/TBID0 rationale; a decode-comment/assertion or micro-check guarding the intent;
the full regression gate; and an **honest correction pass** over every artifact that carries the
disproven "objc preoptimization / cache-resident `class_rw_t`" narrative — the README M2-bfam Status
section, `docs/superpowers/specs/2026-07-10-retrace-m2-bfam-design.md` (status/closeout note),
`.superpowers/sdd/task-m2bfam-2-report.md` (append a correction), and the `hello_dyn_e2e`
`#[ignore]` reason (re-park at the mmap demand-commit wall). Update the memory
`retrace-objc-preoptimization-wall` (already done in the investigation session; keep consistent).

**Out / the honest edge:** the mmap demand-commit wall itself (its own milestone — likely
`M2-mmapcommit` or folded into an existing mmap handler); un-ignoring `hello_dyn_e2e` green (the
guest does not reach `main → write → exit` yet); any objc runtime work past the first mmap fault;
arm64e guest support; the swarm extension.

## Exit criterion

`TCR_EL1_V` carries TBI0+TBID0, the full `just gate` stays green (58/0/1) and clippy clean, and every
artifact naming the old "objc preoptimization" wall is corrected to the verified root cause and
re-parked at the mmap demand-commit boundary. `hello_dyn_e2e` remains `#[ignore]`d with the new,
accurate reason (the mmap fault at `MMAP_BASE + 0x10e744`). No fake green.

## The mechanism (code)

Single constant, `crates/retrace-box/src/lib.rs`:

```rust
// arm64e data-pointer PAC placement: TBI0(bit37)+TBID0(bit51) match Apple's user TCR so a signed
// DATA pointer's PAC lands in [54:47] with the top byte (incl. bit 63) preserved = 0. Without TBI,
// the 47-bit-VA PAC field spans [63:56]∪[54:47] and a re-signed class_data_bits pointer sets bit 63,
// which objc reads as FAST_IS_RW_POINTER (isRealized) → spurious already-realized → fatal.
const TCR_EL1_V: u64 = 0x8_0021_0080_B511;    // +TBI0+TBID0. T0SZ=17 (47-bit VA), TG0=16K, WBWA, inner-share, EPD1, IPS=36-bit
```

No `retrace-arch` change, no new run-loop arm, no trace-format change. The strip-on-FPAC arm from
M2-bfam is unaffected (it still strips the DB-key `autdb` at `data()`; the fix corrects the separate
bit-63 flag read that precedes it).

## Components

- `crates/retrace-box/src/lib.rs` — the `TCR_EL1_V` constant + rationale comment. (Consumed
  unchanged at all three CPU-init sites — no per-site edit.)
- `crates/retrace/tests/hello_dyn_e2e.rs` — rewrite the `#[ignore]` reason to the mmap demand-commit
  boundary.
- `README.md` — correct the M2-bfam Status "next wall" paragraph; add an M2-tbi Status section.
- `docs/superpowers/specs/2026-07-10-retrace-m2-bfam-design.md` — closeout/status correction note
  pointing here.
- `.superpowers/sdd/task-m2bfam-2-report.md` — appended correction (the wall it named was
  misdiagnosed; root cause + fix here).
- Memory `retrace-objc-preoptimization-wall` — kept consistent with this spec.

## Testing

1. **Regression / no-perturbation:** `just gate` green (58/0/1), clippy clean. The TCR change must
   not regress mmu, pac, sign_oracle, cache, or the M2-bfam/M2-va47 tests.
2. **Fix confirmation (manual, documented):** bounded `record-dyn hello_dyn` no longer prints the
   objc `corrupt data pointer` fatal; it reaches the mmap-region data abort at `MMAP_BASE + 0x10e744`.
   (Reproduce recipe below — a full green gate for `hello_dyn_e2e` is out of scope until the mmap
   wall falls.)
3. **Optional micro-guard:** a small in-VM assertion that a guest-signed low DATA pointer keeps bit
   63 = 0 under `TCR_EL1_V` (decide in planning — the fault is hard to synthesize meaningfully; the
   regression suite + documented walk may suffice, mirroring the M2-bfam `bfamstrip` decision).

### Reproduce (fix confirmation)

```sh
cargo build -p retrace && codesign -s - -f --entitlements retrace.entitlements \
  target/aarch64-apple-darwin/debug/retrace
HD=$(find target -name hello_dyn -path "*out*" | head -1)
RETRACE_TRACE=1 perl -e 'alarm 60; exec @ARGV' -- \
  ./target/aarch64-apple-darwin/debug/retrace record-dyn "$HD" -o /tmp/tbi.bin 2>walk.log
tail -4 walk.log   # ends at: data abort … far/ipa=0xa0010e744 (UNMAPPED) — NOT the objc fatal
```

## Risk register

1. **TBI perturbs another guest path.** *Mitigation:* the full regression gate is the proof; the
   spike already showed 58/0/1 green with the change. TBI0 only affects address bits [63:56] being
   ignored for data accesses (canonical low pointers have them = 0), and TBID0 preserves instruction
   PAC — no legitimate guest pointer relies on top-byte address bits under a 47-bit VA.
2. **The re-signer signs `class_data_bits` via a path other than v5 slide-info** (so the exact
   provenance of the bit-63-set value differs from the A-family-slide assumption). *Mitigation:* the
   fix is at the PAC-field level (TCR), independent of which guest `pac*` produced the value — the
   spike confirms it clears the flag regardless. Provenance is an implementation-note to confirm, not
   a gate on the fix.
3. **The mmap wall is deeper than "back a page."** *Mitigation:* out of scope here; M2-tbi only
   re-parks the gate there. The next milestone investigates it with the standard fail-loud walk.
4. **Silent doc drift** — a corrected artifact still cites the old narrative. *Mitigation:* Task 2
   enumerates every artifact (README, m2-bfam spec, task-m2bfam-2, `#[ignore]`, memory) and greps for
   "preoptimiz" / "class_rw_t" / "cache-trust" to confirm none still asserts the disproven story as
   fact.

## Non-goals / explicitly deferred

The mmap demand-commit wall and everything past it; un-ignoring `hello_dyn_e2e` green; arm64e guest
support; the swarm/dyld extension; any performance work. This milestone is the corrective TCR fix
plus an honest re-documentation, nothing more.

## Open questions for implementation planning

1. Whether to add the optional bit-63 micro-guard test or rely on the regression suite + documented
   walk (lean: rely on the suite; the fault is hard to synthesize authentically).
2. The exact provenance of the `class_data_bits` value (v5 slide-info A-family re-sign vs. another
   box signing path) — worth a one-line confirmation in the report, but does not change the fix.
3. Milestone name/placement for the mmap demand-commit wall (new `M2-mmapcommit` vs. extending an
   existing mmap handler) — decided when that milestone is specced, not here.
