# retrace M2-mmapcommit — demand-commit for mach-VM reservations

**Design spec — 2026-07-14.** Sub-milestone of M2 (the loader), sibling of M2-cache, M2-mach,
M2-va47, M2-bfam, and M2-tbi. Clears the wall M2-tbi's landing left behind: the guest faults
fatally on first touch of a page inside a `mach_vm_map` **PROT_NONE reservation** that retrace's
`guest_vm_reserve` deliberately never backed. The fix is a **below-the-trace demand-committer**:
on a translation fault inside a tracked reservation, back exactly the faulting page with a zeroed
anon page — the moral twin of the shared-cache demand-pager, minus the file read and re-sign.

## What this is (the wall's anatomy — investigated 2026-07-14, two independent passes)

Post M2-tbi, the dynamic run (`record-dyn hello_dyn`) reaches objc class realization, which
heap-allocates each `class_rw_t` via libmalloc — specifically Apple's newer **xzone allocator**.
The run dies:

```
RECORD ERROR: non-syscall exit: data abort (EC=0x24 ISS=0x7 FSC=0x7) far/ipa=0xa0010e744 (UNMAPPED) pc=0x1802f5590 elr=0x1800ea230
```

Empirical characterization (traced run, 203 traps; trace-file event dump; frame-pointer walk
symbolicated against the live cache):

- The faulting `pc = 0x1802f5590` is **`libsystem_malloc.dylib`:
  `_xzm_segment_group_alloc_chunk+0x1c4`** — xzone malloc committing a chunk from a segment group.
  (`elr` is stale from the last SVC; for a stage-2 fault trapped below EL1, `pc` is the true
  faulting instruction.) Call chain: `_libSystem_initializer → libdispatch_init → _objc_init →
  map_images → realizeClassWithoutSwift → xzm malloc → FAULT`.
- The owning allocation is **not** a BSD `mmap`: it is a `mach_msg2` MIG `vm_map` (msgh_id 4811)
  with `size = 0x480000` (4.5 MiB), `flags = VM_FLAGS_ANYWHERE | VM_MAKE_TAG(VM_MEMORY_MALLOC)`,
  and **`cur_protection = 0, max_protection = 0`** — a pure PROT_NONE address-space reservation.
  It bump-allocated at `0xa000c0000`, covering `[0xa000c0000, 0xa00540000)`.
- retrace routes `cur_protection == 0` to `Box_::guest_vm_reserve`
  (`crates/retrace-core/src/lib.rs:237-238` → `crates/retrace-box/src/lib.rs:947-974`), which is
  **"bookkeeping only — no host allocation, no stage-2 map"**: it returns an address and advances
  `mmap_next`. Nothing records the reservation's extent; no PTE exists at any stage.
- The guest later got exactly one real backing overlapping the reservation: a
  `mach_vm_map(ANYWHERE, size=0x3e000, prot=3)` that landed at the reservation base `0xa000c0000`
  (the ANYWHERE placement honored a hint because `range_is_free` checks only tracked `Backing`s —
  reservations are invisible to it). That commits `[0xa000c0000, 0xa000fe000)` only.
- The fault (`far` offset `0x4e744` into the reservation) is **~66 KiB past the committed
  sub-range**, and **no intervening call** (`mmap`/`mprotect`/`mach_vm_protect`/`mach_vm_map`)
  touches `[0xa000fe000, …)` before the abort. xzm reaches into its reserved segment expecting
  usable memory; retrace has no fault-driven path to provide it: `page_in_cache` services only the
  shared-region window (`crates/retrace-box/src/lib.rs:596`), so record's `Stop::Other` handling
  falls through to the fatal `describe_stop`.
- Exact addresses (`0xa0010e744`, `0xa000c0000`) are run-layout-dependent (argv length shifts
  them); the invariants are the fault class (EC=0x24 FSC=0x7, UNMAPPED), the PROT_NONE reservation
  route, and the xzm call site.

**Root cause, stated plainly:** retrace's address space has exactly two page states — *eagerly
backed at syscall-service time* or *never backed* — and `guest_vm_reserve` deliberately produces
the latter with no demand path to promote it. Real kernels give reserved-then-committed (or
zero-fill-on-demand) memory lazily on first touch; retrace must do the same for reservations.

## Verified facts (code, this repo)

- **Every other mmap path backs eagerly and fully.** `guest_mmap` (anon BSD, lib.rs:1014-1023),
  `guest_mmap_file` (pread-staged, :1047-1070), exec mmaps (`map_mmap_region` + `set_region_exec`,
  :1025-1046, fresh 32 MiB blocks), and `guest_vm_map` (mach anon with `cur_protection != 0`,
  :916-945) all do `alloc_pages(len)` + `hv_vm_map` for the whole rounded length. Only
  `guest_vm_reserve` (:947-974) returns unbacked addresses.
- **Eager backing of reservations is a hard non-starter:** libmalloc's nano band reservation is
  **24 GiB** (`machmsg.rs` test asserts `size == 0x6_0000_0000`, `cur_protection == 0`) — it
  cannot be host-allocated, and doesn't even fit the 36-bit IPA space. The reserve path exists
  precisely for this.
- **The fault-driven plumbing already exists and is symmetric.** A stage-2 abort surfaces as
  `Stop::Other` with `last_far` (lib.rs:1140-1147, `fault_ipa()` :645); record
  (`core lib.rs:309-313`) and replay (:529-533) both try `page_in_cache(fault_ipa())` and re-run
  on success. The cache pager's servicing shape (:549-640) — round to page base, guard against
  refault loops (8-strike counter), `alloc_pages(GRANULE)` zeroed, `hv_vm_map` one page, push
  `Backing` — is exactly what a reservation committer needs, minus file bytes and re-signing.
- **Zero-fill is deterministic:** `alloc_pages` returns zeroed anon memory (lib.rs:212-222)
  identically on record and replay; replay re-executes the guest's own stores, so both sides
  fault at the same IPAs in the same order and commit identical pages. **Nothing enters the
  trace** — same posture as the cache pager, the timebase MRS, and the FPAC strip.
- **`mach_vm_protect` is a no-op** (`core lib.rs:216-219`, `guest_mprotect` lib.rs:1092-1098) —
  fine under demand-commit: a protect-then-touch commit protocol still ends in a first-touch
  fault, which the committer services.
- **`mmap_next` (and all bump state) resets on `restore`** (lib.rs:1199), so replay's address
  sequence matches record; the reservation table must reset the same way.
- Stage-1 needs no work: the default identity map covers `[MMAP_BASE, …)` with RW non-exec
  `ATTR_DATA` blocks; a freshly committed page is data-only (W^X preserved), and the fresh-IPA
  argument means no TLBI is needed (same soundness argument as the cache pager and
  `set_region_exec`).

## The mechanism

### 1. Reservation bookkeeping (`Box_`)

`guest_vm_reserve` records what it reserves: a `reservations: Vec<(u64, u64)>` (start, len) on
`Box_`, pushed by both the ANYWHERE and FIXED branches, reset to empty in `restore()` (mirroring
`mmap_next`). `guest_munmap`/`mach_vm_deallocate` handling may trim an exactly-covered
reservation; partial splits are deferred until a walk demands them (fail-loud keeps us honest).

### 2. `commit_reserved_page(ipa) -> bool` (`Box_`, sibling of `page_in_cache`)

On a fault: round `ipa` to the 16 KiB page base; return `false` unless the page lies inside a
tracked reservation **and** is not already backed (`host_span` miss); then `alloc_pages(GRANULE)`
(zeroed) → `hv_vm_map` at the page base (`MemFlags::RWX`; stage-1 `ATTR_DATA` governs) → push
`Backing` → `true`. Reuse the cache pager's refault-loop guard pattern. Pages outside any
reservation still fail loud — a genuine wild pointer must stay a hard error, not get silently
materialized.

### 3. Dispatch (mirrored by construction)

Both record (`core lib.rs:309-313`) and replay (`core lib.rs:529-533`) handle `Stop::Other { esr }`
with an `if … { continue; }` guard, not a match-guard. The new committer is a second guard inserted
**immediately after** the existing cache line, textually identical on both sides (rule 1):

```rust
Stop::Other { esr } => {
    if b.page_in_cache(b.fault_ipa()) { continue; }
    if b.commit_reserved_page(b.fault_ipa()) { continue; }   // NEW — same line, record and replay
    // …existing fail-loud path (record logs regs+bt then Err; replay returns Divergence)…
}
```

### 4. Determinism

The committer is a pure function of (fault IPA, reservation table), both of which are identical
on record and replay (shared bump allocator, shared `guest_vm_reserve` calls, reset on restore);
committed contents are all-zeros then guest-authored stores. Nothing enters the trace; the final
full-memory comparison sees identical pages on both sides.

## Scope

**In:** reservation bookkeeping; `commit_reserved_page`; the two mirrored dispatch lines; a
micro-test proving reserve→first-touch→record/replay round-trip (see Testing); the gating spike +
empirical walk of `hello_dyn` past the xzm wall toward `main → write → exit`; if the walk reaches
`main`: un-ignore `hello_dyn_e2e` and add the double-replay determinism test (the M2 headline exit
criterion, deferred since M2-bfam); otherwise re-park the gate honestly at the next distinct
boundary. README Status + memory update at close.

**Out / the honest edge:** partial reservation splits on munmap (defer until a wall demands);
enforcing PROT_NONE fault semantics (retrace will satisfy first-touch inside a reservation even
where a real kernel would SIGSEGV a guard page — retrace is a recorder, not a memory protector;
stage-1 W^X still holds; documented, revisit only if a guest depends on guard faults);
`range_is_free`/ANYWHERE placement consulting reservations (today's placement collided a commit
with the reservation base and the run tolerated it; changing placement perturbs the walk — leave
as-is unless the walk proves it wrong); pre-committing reserved pages passed untouched to
forwarded syscalls (`host_span` miss in `forward_and_diff` — handle if the walk hits it);
performance (per-page commit traps are fine at hello_dyn scale).

## Exit criterion

The reservation micro-test records and replays green; the walk advances past
`_xzm_segment_group_alloc_chunk`; `just gate` green (58 + new tests, honest ignore count), clippy
clean. **If the walk reaches `main`:** `hello_dyn_e2e` un-ignored + double-replay test — the M2
headline gate goes green. If blocked earlier, the gate stays `#[ignore]`d re-parked at the new
boundary with full anatomy, per honest-gate discipline.

## Testing

1. **Micro-test (TDD, Task 1):** an asm guest that issues a PROT_NONE reservation, then stores to
   a page inside it (past any committed prefix), reads the value back, and exits with it — proving
   fault → zero-fill commit → store → load on record, and byte-identical replay. Route choice
   (mach trap vs MIG) decided in planning: whichever the plan pins, the test must traverse
   `guest_vm_reserve` and `commit_reserved_page` (assert via the refault counter staying 0 and the
   run not faulting fatally). A second store to a *different* page of the same reservation guards
   the per-page (not per-reservation) granularity.
2. **Fail-loud negative:** a store *outside* any reservation (and outside all backings) must still
   die with the data-abort `RECORD ERROR` — the committer must not materialize wild pointers.
   (In-process box test; can reuse the existing fixture patterns in `crates/retrace-box/tests/`.)
3. **Regression:** full `just gate` — the new arm must be inert for every existing test (none of
   them fault in `[MMAP_BASE, …)`).
4. **The walk (Task 2):** bounded traced `record-dyn hello_dyn`; triage each new failure fail-loud;
   the M2-mach/M2-va47/M2-bfam method.

## Risk register

1. **xzm's commit protocol needs more than zero-fill** (e.g. it re-maps sub-ranges
   FIXED|OVERWRITE expecting old contents preserved, or reads allocator metadata it expects a
   *kernel* commit call to have installed). *Mitigation:* the walk observes the first divergence
   from expectations immediately; FIXED `guest_vm_map` already handles overwrite mappings
   (`unmap_overlapping`).
2. **The committer masks a real bug** (wild store into reserved-but-unrelated space). *Mitigation:*
   gate strictly to tracked reservations; everything else stays fatal; refault-loop counter panics
   on livelock.
3. **Reservation table drifts between record and replay** (a route serviced on one side only).
   *Mitigation:* `guest_vm_reserve` is called from `route()`/dispatch shared paths that both sides
   execute (verified for MIG 4811 record `core lib.rs:237-238` / replay `:398-403`); the micro-test
   replays; the divergence oracle catches any drift as a reply/IPA mismatch.
4. **The trap-path `mach_vm_map` (num -15) doesn't split on `cur_protection == 0`** (only the MIG
   path was verified to split). If a reservation arrives via the trap, it would be eagerly backed
   by `guest_vm_map` — harmless for small sizes but wrong (and fatal at 24 GiB). *Mitigation:*
   Task 1 verifies the trap path and adds the same split (mirrored both sides) if missing.
5. **Walls past xzm** (more libmalloc/objc/libdispatch init before `main`). *Mitigation:* the
   empirical walk; a distinct new boundary is documented and deferred, not faked.

## Components

- `crates/retrace-box/src/lib.rs` — `reservations` field + reset in `restore`; recording in
  `guest_vm_reserve`; `commit_reserved_page`; possibly the trap-path reserve split (risk 4).
- `crates/retrace-core/src/lib.rs` — the mirrored dispatch line in record and replay `Stop::Other`
  arms (and the trap-path split's core side if needed).
- `crates/retrace-guest` — the reservation micro-guest (asm) + build stanza + path const.
- `crates/retrace/tests/` — the micro-test e2e; `hello_dyn_e2e` un-ignore or re-park.
- README Status section + memory `retrace-objc-preoptimization-wall` (next-wall pointer) at close.

## Open questions for implementation planning

1. Micro-guest route: `mach_vm_map` trap with `cur_protection=0` (trivially expressible in asm)
   vs. MIG 4811 (matches the observed wall exactly but is painful from freestanding asm). Lean:
   trap route **plus** fixing risk 4 so the trap route genuinely reserves; the MIG route is already
   proven by hello_dyn itself in the walk.
2. Whether `mach_vm_deallocate` trimming of reservations is needed for the walk or deferrable.
3. Whether the walk reaches `main` (un-ignore + double-replay lands in Task 2) or a new wall
   (re-park + document).
