# retrace M2-carveout — reservation holes + kernel-faithful ANYWHERE placement

**Design spec — 2026-07-14.** Sub-milestone of M2 (the loader), sibling of M2-cache, M2-mach,
M2-va47, M2-bfam, M2-tbi, and M2-mmapcommit. Clears the wall M2-mmapcommit re-parked at: libmalloc's
xzone allocator dereferences a NULL pointer read from its own metadata, because retrace fails to
model libmalloc's **guarded-metadata carveout protocol**. The fix is precisely the two items
M2-mmapcommit's spec deferred "until a wall demands them" — this wall demands exactly those two:

1. **`mach_vm_deallocate` must punch holes in reservations** (today it's a no-op on them), and
2. **ANYWHERE placement must treat reservations as occupied and search forward from the hint**
   (today `range_is_free` consults only `backings`, so a hinted ANYWHERE map lands *inside* a
   reservation — impossible on a real kernel).

## The wall's anatomy (two independent investigations, 2026-07-14)

**Observed fault:** `_xzm_segment_group_alloc_segment+0x90` in libsystem_malloc, via
`realizeClassWithoutSwift → _xzm_xzone_malloc_freelist_outlined →
_xzm_xzone_find_and_malloc_from_freelist_chunk → _xzm_segment_group_alloc_chunk`. Faulting insn
`ldrb w9, [x8, #0x178]` with `x8 = 0`; x8 loaded by `ldp x27, x8, [x20, #0x10]` from
`x20 = 0xa0010e4c8`, a page `commit_reserved_page` demand-committed as zeros (the `ldp` itself
succeeds — demand-commit did its job; the *value* is the problem).

**Source identification** (libmalloc tag `libmalloc-812.100.31`, commit `c49dafa`; full citations
in `.superpowers/sdd/xzone-research.md`):

- `x20` is a `struct xzm_segment_group_s`. `+0x10` is `xzsg_range_group`, **`+0x18` is
  `xzsg_main_ref`** — the segment group's back-pointer to the main zone
  (`xzone_malloc.h:455-465`). The `ldrb` at `+0x178` reads the main zone's
  `xzmz_use_ranges`/defer bitfield byte (`xzone_malloc.h:1075-1080`).
- `xzsg_main_ref` is written **eagerly, once, at zone creation** (`xzone_malloc.c:9225`,
  `sg->xzsg_main_ref = main;`) — not lazily. The struct is all-zeros because the page holding it
  never received the zone-init stores: the guest's zone metadata was **supposed to live
  elsewhere**.
- **The guarded-metadata protocol** (`vm.c:143-181`, commit path `vm.c:228-249` +
  `xzone_malloc.c:9017-9024`): libmalloc reserves `MiB(4) + random_tail` (observed `0x4f4000`)
  PROT_NONE tagged `VM_MEMORY_MALLOC`, then **punches a 1 MiB carveout with `mach_vm_deallocate`
  at a random (entropy-derived) offset inside it**, then commits all metadata as one RW block via
  **`mach_vm_map(VM_FLAGS_ANYWHERE, address = reservation_base_as_hint, prot = 3, tag = MALLOC)`**.
  On a real kernel the reservation occupies the band, so the first-fit search starting at the hint
  is **forced into the carveout hole** — the metadata legitimately lands mid-reservation, and the
  PROT_NONE flanks are the "guard".

**Empirical confirmation** (traced run, 84 decoded VM ops; full table in
`.superpowers/sdd/xzwall-vmops.md`): exactly this sequence occurs — MIG-4811 reservation
`size=0x4f4000, cur_protection=0, tag=1(VM_MEMORY_MALLOC)` at `0xa000c0000`; a
`MACH_VM_DEALLOCATE` of a 1 MiB sub-range inside it (**no-op'd by retrace**); then the
`mach_vm_map(ANYWHERE, hint=0xa000c0000, size=0x3e000, prot=3)` metadata commit, which retrace
placed **at the raw hint** — `range_is_free` (`crates/retrace-box/src/lib.rs:1022-1027`) excludes
only `self.backings`, never `self.reservations`, so the reserved-but-unbacked base looked "free".
The flag decode itself is **correct** (flags word at request offset 72, ANYWHERE=0x1 read
properly; no OVERWRITE, no FIXED ops target the reservation) — this is a placement-semantics gap,
not a parse bug.

**Failure chain:** carveout dealloc no-op'd → hole never exists → hinted ANYWHERE commit lands at
the reservation base instead of the hole → the guest's zone-init stores land in
`[base, base+0x3e000)` while the addresses xzone later derives for segment-group metadata fall
elsewhere in the band → those pages are serviced by `commit_reserved_page` as fresh zeros →
`xzsg_main_ref == 0` → near-null deref, fatal.

Also observed: **12 identical `gettimeofday` traps immediately pre-fault.** libmalloc itself never
calls `gettimeofday` (it uses `mach_absolute_time`); this is a caller-side bounded backoff and is
expected to disappear once metadata placement is correct. Not load-bearing for the fix; noted so
the walk doesn't chase it.

## Verified facts (code, this repo)

- `reservations: Vec<(u64, u64)>` exists on `Box_` (M2-mmapcommit); `guest_vm_reserve` records
  both ANYWHERE and FIXED reservations; `commit_reserved_page` gates demand-commit to it;
  `restore()` resets it. Splitting a reservation therefore *automatically* makes hole pages
  non-demand-committable — a touch in a punched hole becomes fatal again, exactly matching real
  kernel semantics for deallocated address space.
- `guest_munmap` (`lib.rs:1126-1134`) drops overlapping `backings` + `hv_vm_unmap`s, but never
  touches `reservations`. The `mach_vm_deallocate` trap and MIG routes funnel into it (the
  implementer verifies the exact dispatch sites; both sides are already mirrored).
- `guest_vm_map`'s ANYWHERE branch (`lib.rs:961-984`): uses the hint iff
  `range_is_free(hint, len)`, else falls back to a fresh `mmap_next` bump. `range_is_free`
  (`lib.rs:1022-1027`) checks `backings` + the shared-region window only.
- The MIG vm_map decode (`machmsg.rs:93-105`) is verified correct — no decoder work needed.
- Placement is recomputed identically on record and replay (shared `Box_` code, bump state and
  reservation table reset on `restore`); the replay oracle byte-compares the reply carrying the
  returned address, so any asymmetry surfaces as a divergence, not corruption. Trace format
  unchanged.

## The mechanism

### 1. Hole-punching: `mach_vm_deallocate`/`munmap` trims reservations

Extend the shared deallocate path so `[addr, addr+len)` is subtracted from every overlapping
reservation entry: full cover removes the entry; head/tail overlap trims it; a strictly interior
punch **splits it into two entries** (the carveout case). Backings in range are dropped as today.
Result: the carveout hole is genuinely free-and-unreserved space — not demand-committable, not
"occupied" for placement.

### 2. Kernel-faithful ANYWHERE placement: hint-forward first-fit

Replace the hint branch's binary "hint free? else bump" with a first-fit search that mirrors what
the kernel does with `VM_FLAGS_ANYWHERE` + a non-zero address hint: starting at the (page-rounded)
hint, find the **lowest address ≥ hint** where `[a, a+len)` overlaps no backing, no reservation,
and no forbidden window (shared region / nano-band rules as embodied in `range_is_free` today);
walking candidate gaps in address order. With the hole modeled, `hint = reservation_base` lands
**exactly in the carveout** — reproducing the kernel's forced placement. A zero hint (or search
exhaustion below `mmap_next`) falls back to the existing bump path unchanged. `range_is_free`
additionally excludes `reservations` so FIXED-path callers and any other user of it see
reservations as occupied.

Determinism: the search is a pure function of (hint, len, backings, reservations, mmap_next) —
all identical on both sides. Below the trace? No — it changes *returned addresses*, but those are
recomputed by shared code on both sides and byte-checked by the oracle (same posture as every
existing address computation; symmetry rule 1 is satisfied structurally, not by a new mirror).

## Scope

**In:** reservation subtraction on deallocate (remove/trim/split); reservation-aware
`range_is_free`; hint-forward first-fit ANYWHERE placement in `guest_vm_map`; micro-tests (the
carveout protocol end-to-end + hole-touch-is-fatal + placement unit tests); the gating spike +
walk of `hello_dyn` past the xzone metadata wall toward `main → write → exit`; if `main` is
reached: un-ignore `hello_dyn_e2e` + the double-replay determinism test; else re-park honestly.
README Status + memory update at close.

**Out / the honest edge:** modeling `VM_FLAGS_OVERWRITE` beyond what exists (2 observed calls,
neither targets a reservation; current handling untouched); PROT_NONE guard-fault semantics for
*non-deallocated* reserved pages (unchanged from M2-mmapcommit — the nano-band note stands);
the xzone env/commpage escape hatch (`_COMM_PAGE_DEV_FIRM` + `MallocSecureAllocator=0` — viable
but a sidestep; documented as the fallback if the walk exposes a genuinely deeper xzone
dependency); the pre-fault `gettimeofday` backoff (expected to vanish; investigate only if it
survives the fix); reservation *merging* (adjacent re-reservation coalescing — not observed).

## Exit criterion

Micro-tests green on record and replay; the walk advances past `_xzm_segment_group_alloc_segment`
with `xzsg_main_ref` resolving non-null (i.e. metadata landed in the hole); `just gate` green
(61 + new tests, honest ignore count), clippy clean. If the walk reaches `main`: `hello_dyn_e2e`
un-ignored + double-replay test (the M2 headline gate). Otherwise re-parked with full anatomy.

## Testing

1. **Placement unit tests (in-process, `crates/retrace-box/tests/`):** drive `Box_` directly —
   reserve; punch an interior hole; assert the reservation table split (two entries, exact
   bounds); hinted ANYWHERE map with `hint = reservation base` returns the hole base and backs the
   full requested length; a touch in the *remaining* reserved band still demand-commits; a touch
   in the *hole* outside the new backing is refused (fatal path preserved).
2. **Carveout e2e micro-guest (TDD, asm):** reserve PROT_NONE (trap route, exists since
   M2-mmapcommit) → deallocate an interior 1 MiB hole → `mach_vm_map(ANYWHERE, hint=base,
   prot=RW)` → store/load a sentinel through the *returned* address → exit with it. Record +
   replay byte-identical. RED first (today the map lands at base and the test's placement
   assertion fails — or encode the placement expectation in the guest's exit code).
3. **Fail-loud negatives:** hole-touch fatal (above); wildstore regression stays green.
4. **Regression:** full `just gate` — placement changes must not perturb any existing test
   (existing tests use zero hints or FIXED, and the nano band's hinted commit at `0x600000000`
   must still resolve identically — verify, don't assume: that hint targets space *inside* the
   nano-band reservation, see risk 2).
5. **The walk (Task 2):** bounded traced `record-dyn hello_dyn`; the standard fail-loud triage.

## Risk register

1. **First-fit placement perturbs an existing consumer.** The known hinted-ANYWHERE user before
   xzone is libmalloc's **nano** commit at hint `0x600000000` *inside* the 24 GiB nano-band
   reservation (M2-mach modeled this: the commit must land at its hint). Making reservations
   "occupied" would push that commit out of the band and break nano. *Mitigation — this is the
   central design hazard:* real-kernel semantics for nano differ because nano's commit uses
   `VM_FLAGS_FIXED`-style placement or the kernel's reservation is nano's own entry being
   *overwritten*… The implementer must FIRST re-read how the nano commit is serviced today
   (M2-mach's `guest_vm_map` hint path + the `machmsg.rs` nano test) and preserve its behavior —
   plausibly by honoring a hinted ANYWHERE landing inside a reservation **iff the request's tag
   matches the reservation's** … NO: keep it principled — the kernel rule is that
   `mach_vm_map(ANYWHERE)` never lands inside ANY existing entry. If nano's commit really arrives
   as ANYWHERE-with-hint and works on real hardware, then on real hardware nano must have
   *deallocated or never-reserved* that sub-range — re-derive the truth from the trace (the
   M2-mach walk log / machmsg tests) before coding, and encode whichever semantic the evidence
   supports as a unit test. If the evidence is ambiguous, STOP and report NEEDS_CONTEXT rather
   than guessing (a wrong placement rule breaks the milestone that *modeled* nano).
2. **Reservation splitting breaks `commit_reserved_page` edge cases** (page straddling a split
   boundary). *Mitigation:* GRANULE-align all reservation arithmetic (they're page-granular in
   practice); unit-test the boundary pages explicitly.
3. **xzone needs more than correct placement** (further protocol steps past the metadata commit).
   *Mitigation:* the walk finds the next boundary honestly; the env/commpage escape hatch is the
   documented fallback; risk-register discipline as in M2-mmapcommit (whose risk #1 correctly
   predicted this wall).
4. **Determinism drift via placement.** *Mitigation:* placement inputs are all reset-on-restore
   state; the e2e micro-test replays; the oracle byte-checks returned addresses.

## Components

- `crates/retrace-box/src/lib.rs` — reservation subtraction in the deallocate path;
  reservation-aware `range_is_free`; hint-forward first-fit in `guest_vm_map`'s ANYWHERE branch.
- `crates/retrace-core/src/lib.rs` — none expected (dispatch already routes deallocate/map both
  sides); verify, don't assume.
- `crates/retrace-guest` — `carveout.s` micro-guest + build stanza + const.
- `crates/retrace/tests/` — carveout e2e; `hello_dyn_e2e` un-ignore or re-park.
- `crates/retrace-box/tests/` — placement/split/hole unit tests.
- README Status + memory at close.

## Open questions for implementation planning

1. Risk 1's nano-commit semantics — settle from evidence before coding the placement rule.
2. Whether `mach_vm_deallocate` arrives via `guest_munmap` on both trap and MIG routes (verify
   dispatch sites; mirror if any gap).
3. Whether the walk reaches `main` (un-ignore + double-replay in Task 2) or a new wall (re-park).
