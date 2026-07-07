# retrace M2-cache — Shared-Cache Re-signing — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Emulate the kernel syscall `shared_region_map_and_slide_2_np` (#536) inside the box — map the dyld shared cache from the file, apply its v5 fixups, and re-sign the arm64e auth pointers with the guest's own PAC keys — so real dyld runs and `hello_dyn` records + replays with zero divergence (removing the `#[ignore]` on `hello_dyn_e2e`).

**Architecture:** A lazy per-page cache pager. Intercept #294 (`shared_region_check_np` → return fixed base `0x180000000`) and #536 (establish the pager at fixed slide 0, don't forward). On a stage-2 fault in the cache window, stage pristine file bytes into anon guest memory; TEXT pages → RO+exec; DATA pages → walk the v5 fixup chain, apply regular rebases on the host, and re-sign auth slots via a guest signing oracle (in-guest `pacia`/`pacda` with the fixed keys). Everything is a deterministic function of (file, slide 0, fixed keys) → record and replay regenerate identical cache pages (not stored in the trace). Grounded in the GO spike (`.superpowers/sdd/m2cache-spike-findings.md`).

**Tech Stack:** Rust, Hypervisor.framework via `hv-sys`, the M2 box (MMU-on W^X, PAC, exec-mmap promotion), the arm64e dyld shared cache on disk.

## Global Constraints

- **Platform:** Apple Silicon, macOS 26.x (Tahoe). Branch `m2-loader` (continues M2). All prior tests stay green after every task; run VM tests `--test-threads=1` under a bounded timeout (no `timeout` binary — Perl process-group wrapper); a page-table/PAC mistake can hang `hv_vcpu_run`.
- **SPTM HARD RULE:** never `hv_vm_map` a file-backed page (hard-panics macOS 26). Cache pages are `pread` from the subcache file into ANON backings, then mapped.
- **Fixed slide = 0:** cache VA == `value_add` == `0x180000000`. #294, #536, the fixups, and the addrDiv blend all use this base consistently.
- **v5 only:** all slide-info on this host is v5 (16 KiB pages, `value_add=0x180000000`, A-family keys IA/DA). Assert version==5 and fail loudly otherwise (no silent mis-load).
- **Guest-key re-signing:** auth pointers are signed by executing `pacia`/`pacda` INSIDE the guest (fixed `PAC_KEYS`); never reimplement Apple PAC on the host. The host does only the (key-independent) target/modifier arithmetic.
- **Determinism:** the pager is a pure function of (file bytes, slide 0, fixed keys). Record and replay regenerate identical cache pages; do NOT store cache page contents in the trace. No new nondeterminism enters the oracle.
- **v5 decode (verified against real bytes — use verbatim):** slot `u64`: `runtimeOffset[33:0]`, `diversity[49:34]`, `addrDiv[50]`, `keyIsData[51]`, `next[62:52]`, `auth[63]`. `targetVA = 0x180000000 + runtimeOffset` (slide 0); `key = keyIsData ? DA : IA`; `modifier = addrDiv ? blend(slotSlidVA, diversity) : diversity`; `blend(a,d) = (a & 0x0000FFFF_FFFFFFFF) | (d << 48)`. Regular (auth=0): `finalPtr = (0x180000000 + runtimeOffset) | (high8[49:42]<<56)` — note regular layout reuses bits `[49:34]` region; decode per the spike's `dyld_cache_slide_pointer5` (regular: `runtimeOffset[33:0] high8[49:42] … next[62:52] auth[63]`; confirm the exact `high8` bit position against `cacheprobe.c`).
- **Cache path:** `/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_arm64e` (+ subcaches `.01`, `.02.dylddata`, …). `dyld_cache_header`: `mappingWithSlideOffset` at 0x138, `mapping_and_slide_info` is 56 bytes `{address,size,fileOffset,slideInfoFileOffset,slideInfoFileSize,flags,maxProt,initProt}`. `slide_info5`: `version@0, page_size@4, page_starts_count@8, value_add@0x10, page_starts[]@0x18` (u16 each; `0xFFFF`=empty).
- **License:** MIT OR Apache-2.0; clean-room (format learned from the on-disk bytes + public headers, not copied code).

## File Structure

```
crates/
├── retrace-arch/src/lib.rs        # + SYS_shared_region_check_np=294, SYS_..map_and_slide_2_np=536; v5 decode consts
├── retrace-box/
│   └── src/lib.rs (or new cache.rs module)  # v5 decode, cache metadata loader + routing table,
│                                             #  per-page fixup walker, guest signing oracle, pager,
│                                             #  #294/#536 interception, cache-window fault handling
├── retrace-core/src/lib.rs        # dispatch: route #294/#536 + cache-window faults to the pager (record+replay)
├── retrace-guest/…                # hello_dyn (exists); signing-stub bytes if injected
└── retrace/tests/                 # decode unit tests; oracle test; hello_dyn_e2e (un-ignore); swarm re-point
```

If `retrace-box/src/lib.rs` passes ~600 lines, split the cache machinery into `retrace-box/src/cache.rs` (keep `Box_`'s public surface stable).

---

### Task 1: v5 slide-info + auth-pointer decode (pure)

A pure decoder with no VM/file dependency, unit-tested against the spike's worked example.

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (syscall consts) or `crates/retrace-box/src/cache.rs` (create)
- Test: inline `#[cfg(test)]` in the cache module

**Interfaces:**
- Produces: `SlidePtr5 { auth: bool, runtime_offset: u64, diversity: u16, addr_div: bool, key_is_data: bool, high8: u8, next: u16 }` and `decode5(slot: u64) -> SlidePtr5`.
- Produces: `fn target_va(p: &SlidePtr5, value_add: u64, slide: u64) -> u64` and `fn modifier(p: &SlidePtr5, slot_slid_va: u64) -> u64` (with `blend`).
- Produces: `SYS_SHARED_REGION_CHECK_NP=294`, `SYS_SHARED_REGION_MAP_AND_SLIDE_2_NP=536` in retrace-arch.

- [ ] **Step 1: Write the failing test** — decode the spike's real slot and check the arithmetic:
```rust
#[test]
fn decodes_spike_auth_slot() {
    // From cacheprobe.c: .02.dylddata DATA page1 off 0x22d0
    let p = decode5(0x801dab846c2f15c8);
    assert!(p.auth && p.key_is_data /*DA*/ && p.addr_div);
    assert_eq!(p.runtime_offset, 0x6c2f15c8);
    assert_eq!(p.diversity, 0x6ae1);
    assert_eq!(p.next, 1);
    assert_eq!(target_va(&p, 0x180000000, 0), 0x1ec2f15c8);
    // modifier = blend(slot_slid_va, diversity)
    let slot = 0x1ec06c2d0u64; // slotUnslidVA @ slide 0
    assert_eq!(modifier(&p, slot), (slot & 0x0000_FFFF_FFFF_FFFF) | (0x6ae1u64 << 48));
    // a regular slot
    let r = decode5(0x001000010f3bec00);
    assert!(!r.auth);
    assert_eq!(target_va(&r, 0x180000000, 0), 0x28f3bec00);
}
```
- [ ] **Step 2: Run to verify it fails** — `cargo test -p retrace-box cache::` → FAIL (decode5 missing).
- [ ] **Step 3: Implement `decode5`/`target_va`/`modifier`** per the Global-Constraints bit layout; add the syscall consts. Verify the `high8`/regular bit positions against `spikes/cacheprobe.c`.
- [ ] **Step 4: Run to verify it passes**; `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] **Step 5: Commit** — `git commit -m "M2c t1: v5 slide-info + auth-pointer decode (target/modifier arithmetic); #294/#536 consts"`

---

### Task 2: cache metadata loader + IPA routing table (file parse)

**Files:** Modify the cache module; Test: inline, against the real cache files (read-only).

**Interfaces:**
- Produces: `CacheMeta` — parses the main + subcache `dyld_cache_header`s and their `mapping_and_slide_info[]`; exposes `region_of(ipa) -> Option<CacheRegion>` where `CacheRegion { subcache_path, file_offset_of_page(ipa), slide_info: Option<SlideInfo5>, is_exec: bool, page_index(ipa) }`, and `window() -> (base, size)`.
- Produces: `SlideInfo5 { page_size, value_add, page_starts: Vec<u16> }` parsed from a DATA mapping's slide-info blob.

- [ ] **Step 1: Failing test** — build `CacheMeta` from the real cache; assert `window() == (0x180000000, sharedRegionSize)`, that a known exec VA (`0x180ccb568`) routes to a TEXT subcache with `is_exec` and no slide-info, and that a DATA VA routes to a `.dylddata` subcache with a v5 `SlideInfo5` (`page_size==16384`, `value_add==0x180000000`).
- [ ] **Step 2: Run → FAIL** (CacheMeta missing).
- [ ] **Step 3: Implement** header + mapping + slide-info parsing across subcaches; build the routing (IPA→subcache/mapping/page). Assert slide-info `version==5`; fail loudly otherwise.
- [ ] **Step 4: Run → PASS**; clippy clean.
- [ ] **Step 5: Commit** — `git commit -m "M2c t2: cache metadata loader + IPA routing table (subcaches, v5 slide-info)"`

---

### Task 3: per-page fixup walker (host side)

**Files:** cache module; Test: inline (synthetic page, no VM).

**Interfaces:**
- Produces: `fn walk_page(page: &mut [u8; 16384], si: &SlideInfo5, page_index: usize, slide: u64) -> Vec<AuthSlot>` — applies **regular** rebases in place (host arithmetic) and returns the `AuthSlot { offset, target_va, key_is_data, modifier }` list for the guest oracle. Chain via `page_starts[page_index]` + `next*8`, stop at `next==0`; `0xFFFF` → no fixups (empty).

- [ ] **Step 1: Failing test** — a synthetic 16 KiB page with a hand-built chain: one regular slot (assert it's rebased to `value_add+runtime_offset` in place) and one auth slot (assert it's returned as an `AuthSlot` with the right target/modifier, and NOT written yet). A `page_starts[i]==0xFFFF` page returns empty and is unchanged.
- [ ] **Step 2: Run → FAIL** (walk_page missing).
- [ ] **Step 3: Implement** the in-page chain walk using `decode5`/`target_va`/`modifier`; regular slots written, auth slots collected.
- [ ] **Step 4: Run → PASS**; clippy clean.
- [ ] **Step 5: Commit** — `git commit -m "M2c t3: per-page v5 fixup walker (regular rebases in-place, auth slots collected)"`

---

### Task 4: guest signing oracle

Sign a batch of auth slots with the guest's fixed keys, in one guest vCPU run. This is the one VM-dependent unit.

**Files:** cache module (`Box_::sign_slots`); a signing stub (guest asm bytes or injected code); Test: `crates/retrace-box/tests/sign_oracle.rs`.

**Interfaces:**
- Produces: `Box_::sign_slots(&mut self, slots: &[AuthSlot]) -> Vec<u64>` — signs each `(target_va, modifier, key)` via in-guest `pacia`/`pacda` and returns the signed pointers, **without disturbing the caller's guest state** (save/restore vCPU around the run; or a dedicated signing context).

- [ ] **Step 1: Failing test** — in a fresh box (PAC keys set), sign the spike's worked-example slot; assert the signed value `!= target_va`, and that a guest `autda`/`autia` with the same modifier recovers `target_va` (round-trip — reuse `spikes/pacsign.c`'s proven method). Also assert `sign_slots` left the box's prior vCPU registers unchanged (save/restore correctness).
- [ ] **Step 2: Run → FAIL** (`sign_slots` missing).
- [ ] **Step 3: Implement** the signing stub + `sign_slots`. Concrete design (`spikes/pacsign.c` is the proven reference for keys/enable/pac* encoding):
  - **Signing scratch region (lazy-init on first `sign_slots`):** two anon guest pages at a fixed reserved IPA clear of everything else (e.g. in block 0's free area near the page tables, or a dedicated low IPA) — a **stub page mapped RO+exec (`ATTR_CODE`, via `set_region_exec`/`build_tables` path — W^X: the stub is code, never writable)** and a **table page mapped RW+non-exec** for the `(target, modifier, key)` inputs and the signed outputs. Never file-backed.
  - **Per-key signing:** each slot signs with `pacia` (IA, `key_is_data==false`) or `pacda` (DA, `key_is_data==true`). Either a single stub that branches on a per-entry key byte, or two stubs (run IA slots then DA slots) — your call; correctness is that each slot uses its own key.
  - **The stub** (hand-assembled bytes): loop over the table — load `target`,`modifier`,`key`; `pacia`/`pacda` into the target; store the signed value back; advance; `hvc #0` when done. Use no guest stack (GPRs + the table only). Batched (one `hv_vcpu_run` for the whole page's slots) is preferred; a per-slot run (host sets x0=target/x1=modifier, one `pac*`+`hvc` stub, loop on the host) is an acceptable simpler first cut — `sign_slots`'s signature hides which.
  - **Save/restore:** before running the stub, save the FULL architectural state the caller (dyld) needs — `x0..x30`, `PC`, `SP_EL0`, `CPSR` (and `ELR_EL1`/`SPSR_EL1` if a fault is mid-flight) — set PC=stub, run to the stub's `hvc`, read the signed values from the table, then restore every saved register. `sign_slots` drives its own `run()` loop (not the main dispatch), so the stub's `hvc` is unambiguous. Bounded-timeout the run.
  - **The Step-1 test must prove save/restore:** set a sentinel value in a GPR (and PC/SP/CPSR to known values) before `sign_slots`, and assert they are byte-identical afterward — a real regression guard, not just "it ran".
- [ ] **Step 4: Run → PASS**, then full workspace suite + clippy.
- [ ] **Step 5: Commit** — `git commit -m "M2c t4: guest signing oracle (in-VM pacia/pacda batch re-sign with fixed keys)"`

---

### Task 5: cache pager integration (#294/#536 + cache-window faults)

Wire the pieces: intercept #294/#536 and service cache-window stage-2 faults with stage→walk→sign→map.

**Files:** cache module (`Box_::page_in_cache`, pager state); `retrace-core` dispatch; Test: `crates/retrace-box/tests/cache_pager.rs`.

**Interfaces:**
- Produces: `Box_::install_cache_pager(&mut self)` (build `CacheMeta`, keep subcache fds); `Box_::page_in_cache(&mut self, ipa: u64) -> bool` (stage pristine file page, TEXT→ATTR_CODE, DATA→walk_page+sign_slots+write-back+map; returns whether it handled the IPA). `retrace-core` routes #294 (return `0x180000000`), #536 (install pager, success), and a cache-window `Stop::Other` data/instruction abort to `page_in_cache` (record AND replay identically).

- [ ] **Step 1: Failing test** — a guest that reads a known cache DATA pointer through the pager and `autda`s it: install the pager, fault-in the page containing the spike's worked-example slot, and assert the guest authenticates the re-signed pointer successfully (no FPAC). (A focused guest or a direct `page_in_cache` + in-guest auth check.)
- [ ] **Step 2: Run → FAIL** (pager missing).
- [ ] **Step 3: Implement** the pager + dispatch routing; regenerate identically on record and replay (no cache bytes in the trace).
- [ ] **Step 4: Run → PASS**; full suite + clippy clean.
- [ ] **Step 5: Commit** — `git commit -m "M2c t5: cache pager — #294/#536 interception + per-page stage/walk/sign/map (record+replay)"`

---

### Task 6: `hello_dyn` bring-up to green (iterate)

With the pager in place, run the gate and iterate the remaining dyld issues (the #536 descriptor details, `__DATA_CONST` prot, restart-into-cache-dyld, any residual special cases) until record + replay are zero-divergence. Investigation-shaped, but the cache wall is now removed.

**Files:** `retrace-core`/`retrace-box` (handlers as discovered); `crates/retrace/tests/hello_dyn_e2e.rs` (remove `#[ignore]`).

- [ ] **Step 1** — remove `#[ignore]`; run `cargo test -p retrace --test hello_dyn_e2e -- --nocapture --test-threads=1`; read the first failure past the cache map.
- [ ] **Step 2..N (iterate)** — classify each failure (FPAC at a slot → decode/modifier bug, fix the arithmetic; `__DATA_CONST` write fault → map it RO after signing; restart-into-cache-dyld → ensure pristine `__DATA`; a new mach trap → handle as in 9b). Commit each fix as `M2c t6: handle <thing>`. **Log every recorded substitution; never pass a nondeterministic input through silently.**
- [ ] **Step 3** — until record exits `hi\n`/0 and replay exits `hi\n`/0 with zero divergence.
- [ ] **Step 4** — `cargo test --workspace -- --test-threads=1` + clippy clean.
- [ ] **Step 5: Commit** — `git commit -m "M2c t6: hello_dyn records + replays through real dyld + re-signed cache (gate green)"`

If a NEW hard wall appears (an uncapturable state beyond the cache), stop and report DONE_WITH_CONCERNS with exactly how far dyld got and the blocking event — do not fake a green.

---

### Task 7: seeded swarm re-point + M2 gate + README (M2's original Task 10)

**Files:** `crates/retrace/tests/seeded_swarm.rs`; `README.md`.

- [ ] **Step 1** — re-point the swarm at `HELLO_DYN` (via the dynamic record path), same invariant (exit 0 or 3; exit 0 ⇒ stdout `hi\n`; exit 3 ⇒ stderr `DIVERGENCE`).
- [ ] **Step 2** — `cargo test -p retrace --test seeded_swarm -- --test-threads=1` PASS over all seeds; pin any failing seed in `REGRESSION_SEEDS.md`.
- [ ] **Step 3** — full gate: `cargo test --workspace -- --test-threads=1 && cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] **Step 4** — README M2 section: real dynamically-linked binaries record + replay; the shared cache is mapped and **re-signed with the guest's keys** (emulating kernel #536); note the finding (arm64e cache is host-process-bound) and what's deferred (ASLR slide, cross-build).
- [ ] **Step 5: Commit** — `git commit -m "M2c t7: seeded swarm over hello_dyn; M2 gate green; README"`

---

## Risk register
1. **PAC modifier/blend ABI exactness** — a wrong modifier FPAC-faults (loud, EC=0x1C), so iterate-to-green, not silent. T1 validates the arithmetic; T5/T6 validate end-to-end.
2. **#536 descriptor ABI** — derive from dyld's call site (9b logged it) + the XNU prototype; we only need to establish the mapping + return success at slide 0.
3. **Signing-oracle save/restore** — a T4 test asserts the box's prior vCPU state is byte-identical after `sign_slots`.
4. **`high8`/regular-slot bit position** — verify against `cacheprobe.c`'s working decode (T1).
5. **Trace size** — cache pages are regenerated, not stored; exclude the cache window from the final-memory diff if size demands (deterministic, no fidelity lost).
6. **Cross-boot / OS update** — pin the build; assert v5 and fail loudly on an unknown format.

## Non-goals / deferred
Randomized slide (ASLR), cross-build portability, JIT, cache-page trace snapshotting, positioning/signals/threads/debugger (M3+).

## Self-Review notes (author)
- **Spec coverage:** decode (T1), routing (T2), fixup walk (T3), signing oracle (T4), pager+dispatch (T5), bring-up (T6), swarm/gate/README (T7). Every design section maps to a task; T1–T4 are pure/unit-testable and de-risk the arithmetic before the VM integration.
- **Type consistency:** `SlidePtr5`/`decode5`/`target_va`/`modifier` (T1) feed `walk_page`→`AuthSlot` (T3) feed `sign_slots` (T4) feed `page_in_cache` (T5). `CacheMeta`/`SlideInfo5` (T2) feed the pager.
- **Biggest risk:** the exact PAC modifier ABI (risk #1) — front-loaded into T1's arithmetic test and T4's in-guest round-trip, so a mistake surfaces as a loud FPAC long before the full bring-up.
- **Determinism preserved:** the pager is a pure function of (file, slide 0, fixed keys); record/replay regenerate identical cache pages, so no new nondeterminism enters the oracle — M2's discipline holds.
