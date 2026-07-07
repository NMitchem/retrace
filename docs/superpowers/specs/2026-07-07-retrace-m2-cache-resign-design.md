# retrace M2-cache — shared-cache re-signing (emulate `shared_region_map_and_slide_2_np`)

**Design spec — 2026-07-07**

## What this is

The M2 dyld bring-up (Task 9b) got real `/usr/lib/dyld` booting inside the box and executing
shared-cache code, then hit a fundamental wall: modern arm64e dyld maps+slides+**PAC-signs** the
dyld shared cache via the kernel syscall `shared_region_map_and_slide_2_np` (#536), using the
**host process's per-process PAC keys**. A guest VM with its own fixed keys cannot authenticate
those signatures (FPAC fault, `ESR_EL1` EC=0x1C), and dyld does not re-sign the cache in userspace.
The master-spec bet — "let real dyld map the cache, write no cache parser" — cannot hold for the
arm64e shared cache.

This milestone **emulates syscall #536 inside the box**: intercept it, map the cache's pages from
the **file** (pristine, not the host's live mapping), apply the cache's slide/rebase fixups, and
**re-sign every arm64e auth pointer with the GUEST's fixed PAC keys** — by executing `pacia`/`pacda`
inside the guest itself (the guest's keys sign by definition), so we never reimplement Apple's PAC
crypto. This unblocks dyld and turns the `#[ignore]`d `hello_dyn_e2e` gate green.

The approach is validated end-to-end by the M2-cache spike (`.superpowers/sdd/m2cache-spike-findings.md`,
`spikes/cacheprobe.c`, `spikes/pacsign.c`): **GO**.

## Verified spike facts (this host, Tahoe/arm64e)

- **Format is uniform and simple:** all 14 slide-info regions are **v5**, 16 KiB pages,
  `value_add = 0x180000000`. TEXT subcaches (`.01/.05/.09`) are a single `r-x` mapping with **no
  slide info** (raw code, no fixups). DATA lives in `.NN.dylddata` subcaches whose mappings each
  carry a v5 slide-info blob. Each subcache file is a full `dyld_cache_header`; all map into one
  contiguous VA window at `0x180000000` (`sharedRegionSize ≈ 5.6 GiB`, `maxSlide = 0x10000000`).
- **v5 auth pointer decode** (verified byte-by-byte): `runtimeOffset[33:0] diversity[49:34]
  addrDiv[50] keyIsData[51] next[62:52] auth[63]`. `targetVA = value_add + runtimeOffset + slide`;
  `key = keyIsData ? DA : IA` (**A-family only** — no IB/DB in the cache);
  `modifier = addrDiv ? blend(slotSlidVA, diversity) : diversity`, where
  `blend(addr,d) = (addr & 0x0000FFFF_FFFFFFFF) | (d << 48)`. Regular (non-auth, bit63=0) slots:
  `finalPtr = (value_add + runtimeOffset + slide) | (high8 << 56)`.
- **Per-page self-contained chains:** `page_starts[i]` (u16, `0xFFFF` = empty) is the first fixup's
  byte offset in page `i`; chain via `next*8`, stop at `next==0`. **0 cross-page chains over ~27 M
  slots** — so we fix up + sign exactly one demand-faulted page at a time.
- **Guest signing oracle works:** the guest signs `(target, modifier)` for all key families with the
  fixed `PAC_KEYS`, round-trips (`aut*` recovers the raw), signatures are key- and modifier-distinct,
  and a wrong modifier FPAC-faults (EC=0x1C) — the exact wall, proving correctness is checkable.
- **Sourcing pristine file bytes fixes BOTH Task-9b facets:** re-signing fixes the PAC keys (facet b);
  a from-file cache also has **pristine `__DATA`** (`sMemoryManagerInitialized` false), fixing the
  dirtied-COW abort (facet a). The 9b wall came specifically from demand-paging the host's *live*
  (already host-signed, host-dirtied) mapping.

## Scope

**In scope:**
- Intercept syscall **#536** (`shared_region_map_and_slide_2_np`) and its check syscall #294
  (`shared_region_check_np`): do not forward to the real kernel; establish our own cache mapping at a
  fixed slide and report the base dyld expects.
- A **cache metadata loader**: parse every subcache header + mappings + slide-info at first use;
  build an **IPA → (subcache file, mapping, slide-info, page)** routing table over the cache VA
  window at a **fixed slide (0)**.
- A **lazy per-page cache pager** (stage-2 fault handler for the cache window): TEXT pages → stage
  pristine file bytes, map RO+exec (`ATTR_CODE`); DATA pages → stage pristine file bytes, walk the
  page's fixup chain, apply regular rebases (host arithmetic), and re-sign auth slots via the guest
  oracle, then map RW+non-exec (or RO for `__DATA_CONST`).
- A **guest signing oracle**: batch-sign a page's auth slots in one guest vCPU run using the fixed
  guest keys, saving/restoring dyld's vCPU state around it.
- Deterministic regeneration on replay (cache pages are a pure function of the file + fixed slide +
  fixed keys), so cache page contents need not be stored in the trace.
- Turn the `hello_dyn_e2e` gate green (record + replay `hi\n`/0, zero divergence) and re-point the
  seeded swarm at it (completing M2 Task 10).

**Out of scope (later milestones):**
- Non-zero / randomized cache slide (ASLR); cross-OS-build trace portability (pin the build).
- The full interpreter demo (M6); instruction-exact positioning, signals, threads (M3–M5).
- JIT (anon `PROT_EXEC` mmap) — warned, not supported.
- Snapshotting cache pages into the trace for cross-boot replay (regeneration suffices for the
  same-boot oracle; revisit if boot-independence is needed).

## Exit criterion

**`hello_dyn` (a normal `cc`-arm64 dynamically-linked program) records AND replays with zero
divergence** through real dyld + the re-signed shared cache — the `#[ignore]` removed from
`hello_dyn_e2e`, and the seeded swarm re-pointed at it (M2's original oracle-gated gate).

## The mechanism

### 1. Fixed slide + syscall interception
Choose **slide = 0** (cache VA == `value_add` == `0x180000000`; deterministic, within `maxSlide`).
- **#294 `shared_region_check_np`:** return the fixed cache base `0x180000000` (not forwarded).
- **#536 `shared_region_map_and_slide_2_np`:** do not forward. Parse the arguments enough to know
  dyld wants the cache mapped; establish our pager over the cache window at slide 0; return success.
  dyld proceeds to use the cache at `0x180000000+`; accesses fault into our pager. (The exact #536
  ABI — the mappings/slide descriptor it passes — is nailed during implementation from dyld's call
  site; we ignore dyld's requested slide and pin 0, keeping #294, the fixups, and the addrDiv blend
  all consistent on the same base.)

### 2. Cache metadata loader (once, at #536 / load)
For each subcache file (main + `.01`, `.02.dylddata`, …): read its `dyld_cache_header`, its
`mapping_and_slide_info[]` (address, size, fileOffset, slideInfoFileOffset/Size, prot), and, for
DATA mappings, its v5 slide-info (`page_size`, `value_add`, `page_starts[]`). Build a routing table:
a faulting cache IPA → the owning (subcache fd, mapping, slide-info, pageIndex, in-file offset).
Keep subcache fds open (record only). Low risk; buildable directly from the headers.

### 3. Lazy per-page pager (stage-2 fault in `[0x180000000, 0x180000000+sharedRegionSize)`)
On a guest data/instruction abort whose IPA is in the cache window:
1. Route the IPA to its (subcache, mapping, page).
2. **Stage pristine file bytes** into a fresh anon backing page (`pread` the subcache file at the
   page's in-file offset — **never** a file-backed `hv_vm_map`; SPTM hard-panics).
3. **TEXT / no-slide-info mapping:** map the page `ATTR_CODE` (RO+exec, via the Task-9a
   `set_region_exec` path) — cache code, no fixups.
4. **DATA / slide-info mapping:** walk `page_starts[pageIndex]`'s chain: for each slot, decode v5;
   **regular** → write `(value_add + runtimeOffset) | (high8<<56)` (slide 0) on the host; **auth** →
   record `(slotOffset, targetVA, key, modifier)` for the oracle. Then run the **guest signing
   oracle** (step 4 below) to sign all auth slots and write them back into the page. Map the page
   RW+non-exec (or RO for `__DATA_CONST`/auth-const mappings per their prot).
5. Resume dyld at the faulting instruction.

Cache pages are anon backings like any other; because the inputs (file bytes, slide 0, fixed keys,
fixup arithmetic) are identical on record and replay, the pages are **byte-identical** across both —
so they are regenerated on replay, not stored in the trace. (The final-memory oracle may exclude the
cache window for size, or include it since it matches; decide during implementation by trace size.)

### 4. Guest signing oracle
Given a page's auth slots `[(slotOffset, targetVA, key, modifier)]`:
1. **Save** dyld's vCPU state (GPRs, PC, SP, PSTATE).
2. Write the slot inputs into a small guest scratch region and point the vCPU at a **signing stub**
   (a few instructions: loop loading `target`+`modifier`, `pacia`/`pacda` per `key`, store the
   signed value, `hvc` when done). The stub executes with the guest's fixed keys → correct
   signatures.
3. Run the vCPU until the stub `hvc`s back; read the signed values; write them into the staged page
   at their `slotOffset`s.
4. **Restore** dyld's saved vCPU state and resume.
≤2048 slots/page, one vCPU run per faulted DATA page — trivially fast, amortized over a fault we
already take. (Alternative if save/restore proves fiddly: a dedicated pre-provisioned signing entry
that never disturbs dyld's live state; chosen during implementation.)

### 5. Determinism & the oracle
Everything the pager produces is a deterministic function of (subcache file bytes, slide 0, fixed
`PAC_KEYS`), so record and replay regenerate identical cache pages. The existing M1/M2 oracle
(per-syscall landmarks, exit-code, final-memory) applies unchanged; the swarm re-points at
`hello_dyn`. Nondeterministic *inputs* dyld consumes (getentropy, ports, times) keep the Task-9b
record-and-substitute handling.

## Components (building on M2)

- **`retrace-box`** — the pager: cache metadata loader, IPA routing table, per-page fixup+sign,
  guest signing oracle, #294/#536 interception hooks. Largest new surface.
- **`retrace-core`** — dispatch: route #294/#536 and cache-window stage-2 faults to the pager;
  record/replay symmetry (pager runs identically on both).
- **`retrace-arch`** — syscall consts (`SYS_shared_region_check_np=294`,
  `SYS_shared_region_map_and_slide_2_np=536`); v5 slide-info / auth-pointer decode constants.
- **`retrace-guest`** — `hello_dyn` (exists); the signing-stub bytes if injected as guest code.
- Tests: unit tests for the v5 decode (targetVA/modifier arithmetic against the spike's worked
  example), the routing table, and the fixup walker; an integration test that a re-signed cache
  auth pointer authenticates in-guest; and the `hello_dyn_e2e` end-to-end gate.

## Risk register

1. **PAC modifier/blend must match dyld's ptrauth ABI bit-exactly** (spike risk #1). A wrong
   `blend`/diversifier → FPAC fault. *Mitigation:* the decode is validated against real bytes; the
   true gate is end-to-end (re-sign → real dyld stops FPAC-faulting). The FPAC fault is loud
   (EC=0x1C), never silent — so a mistake is immediately visible, not a silent mis-auth.
2. **#536 ABI reverse-engineering.** The exact descriptor dyld passes must be parsed. *Mitigation:*
   Task 9b already reached and logged the #536 call; derive the ABI from dyld's call site + the XNU
   prototype; we only need enough to establish the mapping and return success at slide 0.
3. **vCPU state save/restore around the signing oracle.** Corrupting dyld's live state would diverge.
   *Mitigation:* save/restore the full architectural set; a focused unit test signs a known page and
   asserts dyld's state is byte-identical afterward; fall back to a pre-provisioned signing context.
4. **Trace size / oracle cost** if cache pages enter the final-memory snapshot. *Mitigation:*
   regenerate (don't store) cache pages; exclude the cache window from the final-memory diff if size
   demands (they're deterministic, so excluding them loses no fidelity).
5. **`__DATA` COW-init (facet a).** *Mitigation:* pristine-from-file `__DATA` gives
   `sMemoryManagerInitialized == false`; the normal "restart into cache dyld" then proceeds. Verified
   in principle by the spike; the end-to-end run confirms.
6. **Cross-boot / OS-update fragility of the cache format.** *Mitigation:* pin the macOS build in the
   trace; v5-only assumption asserted-and-fails-loudly on an unrecognized version (master risk #1).

## Non-goals / explicitly deferred
- Randomized slide (ASLR), cross-build portability, JIT, cache-page trace snapshotting, and
  everything already deferred by M2 (positioning, signals, threads, the debugger seam).

## Open questions for implementation planning
- **#536 descriptor ABI** — parse from dyld's call site; how much of it we must honor vs. can ignore
  (we pin slide 0 and self-map).
- **Signing-oracle mechanism** — save/restore dyld's state vs. a dedicated pre-provisioned signing
  entry that never touches dyld's live registers. Lean save/restore; measure.
- **`__DATA_CONST` vs `__DATA` prot** — map auth-const RO after signing (matches dyld's expectation)
  or RW; determine from the mapping's initProt and whether dyld mprotects it.
- **Cache window in the final-memory oracle** — include (bit-identical, costs size) or exclude
  (deterministic, cheaper). Decide by measured trace size.

## Self-review notes (author)
- **Spike coverage:** format (A), signing oracle (B), per-page determinism (C) all proven GO; decode
  formulas validated against real bytes with a worked example; both Task-9b facets (PAC + dirty
  `__DATA`) addressed by sourcing pristine file bytes + re-signing.
- **Biggest risk:** #1 (exact PAC modifier ABI) — but it fails loudly (FPAC), so it is a fast
  iterate-to-green, not a silent-divergence hazard. The plan should front-load a unit test that
  re-signs the spike's worked-example slot and authenticates it in-guest before wiring the full pager.
- **This keeps M2's determinism discipline:** the pager is a pure deterministic function of file +
  fixed slide + fixed keys, so record/replay regenerate identical cache state — no new nondeterminism
  enters the trace; the oracle is unchanged.
