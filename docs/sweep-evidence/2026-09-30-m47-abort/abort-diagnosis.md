# M47 git libmalloc abort: diagnosis

Agent: m47-abort, 2026-10-01. Diagnosis only. Nothing is committed and the worktree's source is
untouched. Every artifact named here is under `/private/tmp/claude-501/m47-abort/`, written `$A/` below.

## Status (kept current)
- [x] reproduction: aborting and clean traces of the same command, and a deterministic control
- [x] the recorded input that differs
- [x] where the bad pointer comes from, with file:line in retrace
- [x] fix written in a scratch copy. Controlled validation: 6/6 aborts became 0/6.
- [x] unpinned validation: `log -1` 0/30 (base 2/30); `commit` 0/20 (census 9/20)
- [x] guards written, and verified able to fail on a mutant that ignores the mask
- [x] clippy `-D warnings` clean on the fix copy
- [x] full `cargo test --workspace` on the fix copy: exit 0, 904 passed / 0 failed / 9 ignored (see "Gate")

## Root cause (one sentence)
retrace ignores `mach_vm_map`'s alignment **mask**. So libmalloc's xzone allocator gets its 4 MiB
"large" segment, requested with mask `0x3fffff`, at an address that is not on a 4 MiB boundary.
xzone registers a segment in its segment table one entry per 4 MiB step **from the base**. The
4 MiB granule that the segment's tail straddles therefore never gets an entry. `free` of any chunk
in that tail finds a zero entry and aborts with "pointer being freed was not allocated".

## Reproduction
Binary: the base, `/private/tmp/claude-501/m47-base-retrace`, sha256
`e4ae912e44c071e33ca978f9f78613b21709cac4a996b8ed50deaac01582defb` (t0's base, `aa16f01`).
Repo: `$A/repo`, one commit, made by `$A/mkrepo.sh`. Command: `git log -1` with cwd = the repo and no
`-C`.

| run set | script / binary | result |
|---|---|---|
| `$A/l1`, `$A/l2` | `loop.sh`, base, RETRACE_TRACE=1 | 2 aborts in 13 (l1 run 1, l2 run 10) |
| `$A/val-log-base` | `val-log.sh`, base | **2 aborts in 30** |
| `$A/vm1` | `loopvm.sh`, diagnostic build (placement log) | 1 abort in 12 |
| `$A/ent1` | getentropy pinned only | 1 abort in 12; the reservation size still varies |
| `$A/ent2`, `$A/ent3` | getentropy AND gettimeofday pinned per seed | deterministic per seed; seeds 16, 28, 34, 39 abort |
| `$A/ctl-base` | seeds 16 ×3, 28, 34, 39; diagnostic build without the fix | **6/6 rc=134** |
| `$A/ctl-fix` | same seeds; diagnostic build WITH the fix (`rt-fixdiag`) | **0/6**; stdout == native |

Kept traces: `$A/abortA.bin` (= l2 run 10, aborts), `$A/cleanA.bin` (= l2 run 1, clean),
`$A/commit-abort.bin` (an aborting `commit` on the census binary).

## How it was found (measurements, in order)

### M1. Where control flow first differs (abort vs clean)
Both runs reach the same `free` of the same object: the 41448-byte (`0xa1e8`) read buffer of the
inner `tzload` (`tzload+248`, `bl free`). x0 is `0xa00848000` in the abort and `0xa008a0000` in the
clean run. `$A/pctrace.sh` single-stepped 400 instructions from there in each trace
(`abort-free.regs`, `clean-free.regs`), and `$A/pcs.py` diffed the two. Control flow first diverges
at step 60, in `_xzm_free + 168`:

```
_xzm_free+120  lsr  x10, x8, #36        ; ptr < 64 GiB
_xzm_free+128  ldr  x9, [x9, #0x200]    ; segment table base
_xzm_free+160  ubfx x10, x1, #22, #14   ; index = ptr >> 22
_xzm_free+164  ldr  w3, [x9, x10, lsl #2]
_xzm_free+168  tbz  w3, #31, +1580      ; entry not valid -> not-found path -> abort
```
The table entry for index `0x2802` is **0** in the abort, and `0x8028017b` in the clean run (valid,
with segment metadata at `0xa005ec000`).

### M2. Who writes the table entry
From the free, `watch <entry> 4; reverse-continue`:
- **Abort trace, entry `0x2802` (`0xa006a2008`): no earlier hit.** It was never written. Its
  neighbour `0x2801` was written at `_xzm_segment_table_allocated_at + 260` for the segment
  **`[0xa007fc000, 0xa00bfc000)`** (x1 and x22 at the store). The freed pointer `0xa00848000` lies
  inside that segment, but in granule `0x2802`.
- **Clean trace, entry `0x2802`:** written at the same pc, for the segment
  **`[0xa00854000, 0xa00c54000)`**. The freed pointer `0xa008a0000` lies in granule `0x2802`, which
  is the granule that got registered.

`_xzm_segment_table_allocated_at` loops: `+260 stlr w23,[x8]; +264 add x19,x19,#0x400000;
+268 cmp x22,x19; b.hi`. It writes one entry per 4 MiB step, starting at the BASE.
- With a 4 MiB-aligned base, that covers the segment exactly.
- With base `0xa007fc000`, it registers only `0x2801`. Everything from `0xa00800000` up, which is
  all but the first 16 KiB of the segment, cannot be found by `free`.

### M3. Where the segment address comes from
The segment comes from `mach_vm_map(task, &a, 0x400000, mask=0x3fffff, VM_FLAGS_ANYWHERE|tag 2, RW)`:
trap −15, line 740 of the abort trace's RETRACE_TRACE log. A placement log in a scratch build
(`[vmlog]`, `$A/diag-core.patch`) shows this one call comes back MISALIGNED in **every** run (12/12).
Natively the kernel returns an address with `addr & mask == 0`.

retrace never reads the mask:
- **Trap route:** `crates/retrace-core/src/lib.rs:42-45`, `vm_map_args`, returns
  `(addr_ptr, size, flags, prot)` and drops `args[3]` (the mask). The record arm is at `:412-431`
  and the replay mirror at `:2203-2225`.
- **MIG 4811 route:** `machmsg.rs:239` decodes `req.mask`, but neither the record arm (`:458-478`)
  nor the replay mirror (`:2023-2039`) uses it.
- **The box:** `crates/retrace-box/src/lib.rs:2159` `guest_vm_map` (bump at `:2177-2180`,
  `first_fit` at `:2168` and `:2347`) and `:2207` `guest_vm_reserve` (bump) take no alignment
  parameter at all.

### M4. Which recorded input decides abort vs clean
Placement is a pure function of earlier placements, so whatever varies between runs must move the
bump cursor. Diffing the runs' placement logs, the first divergence is the first 4811 reservation:
`mach_vm_map(hint 0, size 0x408000 + k*0x4000, prot 0)`, with k varying from run to run (0x408000,
0x454000, 0x4a8000, …).

Its caller, from a frame walk at the preceding `gettimeofday` (`$A/gtod-stack.txt`):
`__malloc_init` → `mvm_guarded_range_init + 52` → `arc4random_uniform` → corecrypto
`ccrng_uniform` → `ccrng_crypto_generate` → `ccrng_schedule_read` → … → `clock_gettime_nsec_np` →
`_mach_boottime_usec` → `gettimeofday`. The random guard size is drawn from corecrypto's RNG.

Pinning experiments used two switches in a diagnostic build. Both overwrite after the host call and
before the diff, so the pinned values are recorded as the kernel's writes:
- `RETRACE_FIXENTROPY=<seed>` overwrites `getentropy`'s bytes.
- `RETRACE_FIXTIME` overwrites `gettimeofday`'s `tv` with a per-call counter.

Results:
- **Entropy pinned alone:** the size still varies (`$A/ent1`).
- **Entropy AND time pinned:** the size, the segment address and the outcome are identical for a
  given seed, 3/3 repeats (`$A/ent2`, `$A/ctl-base`).

So the inputs are the two **getentropy** results (16 and 32 bytes) and the host **wall clock** from
the forwarded `gettimeofday`, which corecrypto's reseed schedule reads. Together they set the size
of the guarded-range reservation. That size sets where the bump cursor sits when the 4 MiB segment
is placed, and that sets how far the misaligned segment straddles a 4 MiB boundary.

**Prediction rule.** Both tzload buffers sit at segment offsets `0x40000` and `0x4c000`, which are
deterministic. The run aborts iff the segment base lies in `[0xa007b4000, 0xa00800000)`, i.e. iff
the inner buffer lands past the next 4 MiB boundary.
- `$A/rule.py` checks it against every run whose placement log survives (vm1, ent1, ent2, ent3;
  ent1's repeated seeds overwrote their own files). It is correct on 65 of 65 runs: 5 aborts and
  60 clean. The `ctl-base` seeds (6/6 aborts, segments `0xa007e8000`–`0xa007f4000`) also fall
  inside the range.
- The observed reservation sizes span 0x408000..0x4e4000 (56 page steps), and the 6 smallest
  (0x408000..0x41c000) abort: about 1 in 9 if the draw is uniform, which matches t0's "1 in 10".

### Why record and replay agree (why the oracle is blind)
Replay recomputes the same misaligned address from the same recorded args and byte-compares it with
the recorded one. They match, because both sides ignore the mask in the same way. The determinism
oracle checks record == replay. It does not check record == kernel.

### `git commit`
Measured on one aborting census-binary commit (`$A/commit-abort.sh` → `$A/commit-abort.bin`, the 4th
try):
- `continue; break _xzm_free+168; reverse-continue` lands on the failing check with
  `x1 = 0xa00800000`, index `x10 = 0x2802`, `w3 = 0`. This run's free was again tzload's
  (`x30 = tzload+252`).
- Watching entry `0x2801` and reverse-continuing reaches `_xzm_segment_table_allocated_at+260`,
  registering segment **`[0xa007d4000, 0xa00bd4000)`**. It is misaligned, only granule `0x2801` is
  registered, and the freed pointer is the first byte of granule `0x2802`.

t0's `deflateEnd` site belongs to the same class: libz's 64 KiB deflate buffers are xzone large
chunks in the same segment. Commit's higher abort rate fits this. It allocates more large chunks,
which reach further into the segment, so a smaller misalignment already puts one past the boundary.

## Hypotheses considered
| # | hypothesis | measurement | result |
|---|---|---|---|
| H1 | forwarded `MADV_FREE_REUSABLE` (the probe's theory) | t0: aborts with 7/8 no-op'd; `log -1` has no madvise before its abort | refuted (t0) |
| H2 | the guard-band canary or diff window corrupted malloc metadata | the bad entry was never written by anyone (`no earlier hit`); nothing overwrote it | refuted |
| H3 | demand-commit (`commit_reserved_page`) or TLB staleness lost a store | as H2: there is no store to lose; xzone never issues one for granule 0x2802 | refuted |
| H4 | the mach_vm_map mask is ignored, so the xzone segment is misaligned | M2–M4 above; the fix turns 6/6 controlled aborts into 0/6, and the unpinned rates to 0 | **confirmed** |

## Fix (scratch copy `$A/src-fix`, NOT applied to the worktree)
The full diff is `$A/fix.diff`: 7 files, +207 −21, made with `git diff` against a pristine baseline
commit inside `$A/src-fix`. The source part is about 40 changed lines in two files.

`crates/retrace-box/src/lib.rs`:
```diff
     pub fn guest_vm_map(&mut self, addr: u64, size: u64, anywhere: bool, exec: bool) -> u64 {
+        self.guest_vm_map_masked(addr, size, 0, anywhere, exec)
+    }
+
+    /// `guest_vm_map` with `mach_vm_map`'s alignment `mask`: an ANYWHERE placement returns an
+    /// address with `ipa & mask == 0`, as `vm_map_enter` does. [...why, the xzone table...]
+    pub fn guest_vm_map_masked(&mut self, addr: u64, size: u64, mask: u64, anywhere: bool, exec: bool) -> u64 {
         let (host, rlen) = alloc_pages(size as usize);
+        let m = mask | (GRANULE as u64 - 1);
         let ipa = if anywhere {
-            match if addr != 0 { self.first_fit(addr, rlen as u64) } else { None } {
+            match if addr != 0 { self.first_fit(addr, rlen as u64, m) } else { None } {
 ...
                 None => {
                     if exec { self.mmap_next = (self.mmap_next + (BLK - 1)) & !(BLK - 1); }
+                    self.mmap_next = (self.mmap_next + m) & !m;
                     let a = self.mmap_next; self.mmap_next += rlen as u64; a
 ...
     pub fn guest_vm_reserve(&mut self, addr: u64, size: u64, anywhere: bool) -> u64 {
+        self.guest_vm_reserve_masked(addr, size, 0, anywhere)
+    }
+    pub fn guest_vm_reserve_masked(&mut self, addr: u64, size: u64, mask: u64, anywhere: bool) -> u64 {
         let rounded = (size + GRANULE as u64 - 1) & !(GRANULE as u64 - 1);
         let base = if anywhere {
+            let m = mask | (GRANULE as u64 - 1);
+            self.mmap_next = (self.mmap_next + m) & !m;
             let end = self.mmap_next + rounded;
 ...
-    fn first_fit(&self, hint: u64, len: u64) -> Option<u64> {
+    fn first_fit(&self, hint: u64, len: u64, m: u64) -> Option<u64> {
         let g = GRANULE as u64;
-        let base = hint & !(g - 1);
-        let round_up = |x: u64| (x + g - 1) & !(g - 1);
+        let round_up = |x: u64| (x + m) & !m;
+        let base = round_up(hint & !(g - 1));
 ...
-        if SHARED_REGION_END > base { cands.push(SHARED_REGION_END); }
-        if SCRATCH_RESERVED_END > base { cands.push(SCRATCH_RESERVED_END); }
+        if round_up(SHARED_REGION_END) > base { cands.push(round_up(SHARED_REGION_END)); }
+        if round_up(SCRATCH_RESERVED_END) > base { cands.push(round_up(SCRATCH_RESERVED_END)); }
```

`crates/retrace-core/src/lib.rs`:
```diff
-fn vm_map_args(num: u64, args: &[u64; 8]) -> (u64, u64, u64, u64) {
-    if num == MACH_VM_MAP { (args[1], args[2], args[4], args[5]) }
-    else                  { (args[1], args[2], args[3], 0x3 /*RW*/) }
+fn vm_map_args(num: u64, args: &[u64; 8]) -> (u64, u64, u64, u64, u64) {
+    if num == MACH_VM_MAP { (args[1], args[2], args[3], args[4], args[5]) }
+    else                  { (args[1], args[2], 0, args[3], 0x3 /*RW*/) }
 }
 # record trap arm (:412) and replay trap mirror (:2203):
-                let (addr_ptr, size, flags, prot) = vm_map_args(num, &args);
+                let (addr_ptr, size, mask, flags, prot) = vm_map_args(num, &args);
-                    b.guest_vm_reserve(req, size, anywhere)
+                    b.guest_vm_reserve_masked(req, size, mask, anywhere)
-                    b.guest_vm_map(req, size, anywhere, exec)
+                    b.guest_vm_map_masked(req, size, mask, anywhere, exec)
 # record 4811 arm (:458) and replay 4811 mirror (:2023):
-                            b.guest_vm_reserve(req.address, req.size, anywhere)
+                            b.guest_vm_reserve_masked(req.address, req.size, req.mask, anywhere)
-                            b.guest_vm_map(req.address, req.size, anywhere, exec)
+                            b.guest_vm_map_masked(req.address, req.size, req.mask, anywhere, exec)
```

Notes for the controller:
- **Symmetry rule 1 holds by construction.** Record and replay call the same `_masked` method with
  the same `mask`, read from the same recorded args. No new state is added (`mmap_next` is already
  in `BoxState`), and the trace format does not change.
- **The old names delegate with `mask = 0`.** Every other caller and every existing test is
  untouched. With mask 0, `first_fit` is unchanged.
- **One edge case with mask 0.** The bump paths now round `mmap_next` up to the granule. That is a
  no-op whenever the cursor is already granule-aligned. The cursor can be unaligned only after a
  FIXED reservation whose size is not a page multiple and whose range covers the cursor, a case no
  measured caller produces. If you want strictly identical behaviour there, round with `mask` alone
  on the two bump paths.
- **A pre-fix trace replays as a loud divergence, not silently:**
  `rt-fix replay cleanA.bin` → exit 3,
  `DIVERGENCE at landmark 524 … mach_vm_map ipa mismatch: replay 0xa00c00000 != recorded
  0xa00854000`. The fix changes what a recorded placement means, so you should rule on whether that
  calls for a `TRACE_MAGIC` bump (M24 precedent).
- **The class is wider than git.** Any guest whose xzone "large" segment gets a masked ANYWHERE map
  is exposed: CPython, jq, the Apple sweep. A free in the unregistered tail aborts. If a later
  segment lands in that granule and registers it, xzone would instead resolve the pointer to the
  WRONG segment's metadata, which is silent heap corruption. That second outcome is inferred, not
  measured.
- **FIXED ignores the mask.** Every measured masked call is ANYWHERE.
- **`fix.diff` is against worktree HEAD `090bf5e`.** While I worked, the worktree gained uncommitted
  M47 edits in the same two source files (`retrace-box/src/lib.rs`, `retrace-core/src/lib.rs`). Those
  edits are not mine, and the diff may need a rebase onto them.

## Regression guards (all in `$A/src-fix`, all in `fix.diff`)
1. **`crates/retrace-guest/c/vmalign_dyn.c`** (+ `build.rs` entry, `VMALIGN_DYN`, a parse test).
   It makes five `mach_vm_map(…, 4 MiB, mask 0x3fffff, ANYWHERE)` calls:
   - the trap route;
   - the MIG 4811 route (`max_protection != VM_PROT_ALL` forces it);
   - both reservation forms;
   - a hinted (first_fit) one.

   Each call is preceded by a one-page `mach_vm_allocate`, so two consecutive maps cannot both be
   aligned under a placement that ignores the mask. The fixture prints `aligned=` per route at the
   end. Confirmed with the placement log: three −15 calls and two 4811 calls carry the mask.
   - native: 5 × `aligned=1`;
   - **base binary:** record prints 5 × `aligned=0`; replay reproduces it and **exits 0**. This is
     the oracle-blind case.
   - fixed binary: 5 × `aligned=1`, and both replays identical.
2. **`crates/retrace/tests/vmalign_e2e.rs`.** It calls `assert_rung_records_and_replays` with the
   native stdout. It also asserts that exactly three untagged trap-route maps carry the mask, so the
   other two must have gone to 4811.
3. **`crates/retrace-box/tests/vmalign.rs`.** Four unit tests: the bump map, the bump reservation,
   the hinted first_fit (each aligned and non-overlapping), and a mask-0 control that keeps
   contiguous placement.

**Able to fail.** A mutant (`$A/src-mut`, `let m = { let _ = mask; GRANULE as u64 - 1 }`) fails all
three aligned unit tests (`got 0xa00004000`, `0xa00004000`, `0xa00008000`) and the e2e test (stdout
5 × `aligned=0`). The mask-0 control passes on both (`$A/t-mut-*.log`). On the fix, every guard
passes (`$A/t-vmalign-*.log`).

## Validation
| what | binary | N | aborts | other |
|---|---|---|---|---|
| `log -1`, unpinned | base `m47-base-retrace` | 30 | **2** | — |
| `log -1`, unpinned | fix `rt-fix` (sha `43f00caf…`) | 30 | **0** | stdout == native 30/30; replay rc 0 and stdout == record 30/30 |
| `log -1`, inputs pinned, aborting seeds | diag, no fix | 6 | **6** | — |
| `log -1`, same seeds | diag + fix | 6 | **0** | segment at `0xa00800000` |
| `commit` (fresh repo, `maintenance.auto=false`, cwd = repo) | census `m47-census-retrace` | 20 | **9** | 9 with no commit |
| `commit`, same | census + fix `rt-cfix` (sha `75afceb5…`) | 20 | **0** | 20/20 committed, `git fsck` clean, replay rc == record rc 20/20 |

The base binary cannot record `commit` (M33 panic on `chdir`, syscall 12), so `commit` was compared
on the census build (t0's rows), with and without the fix (`$A/src-cfix` = fix + `census-build.patch`).
Statistics: with p = 0.1, 0/30 has probability 0.04; with p = 0.45, 0/20 has probability 6e-6. The
pinned control is the decisive one.

Clippy: `cargo clippy --workspace --all-targets -- -D warnings` on `$A/src-fix` exits 0
(`$A/clippy-fix.log`).

## Gate
`cargo test --workspace --no-fail-fast -- --test-threads=1` on `$A/src-fix` → `$A/gate-fix.log`.
It ran as one background command, so there was no chunking and no dropped `--bins`/doc-test
targets.
- **cargo exit 0:** 154 result lines, **904 passed, 0 failed, 9 ignored** (`$A/gatesum.py`).
- The new guards are among them: `vmalign_e2e`, `vmalign` (4), and `vmalign_guest_parses`.
- jq, Homebrew Python and lldb are present on this host, so their e2e tests ran rather than
  skipping.
- I did not reconcile this against t0's 905 `#[test]` count file by file.

## Evidence kept
`$A/` = `/private/tmp/claude-501/m47-abort/`:
- **Traces:** `abortA.bin`, `cleanA.bin`, `commit-abort.bin`.
- **Trap logs and the free-path trace:** `l1/`, `l2/` (RETRACE_TRACE logs of every run),
  `abort-free.regs`, `clean-free.regs`, `pctrace.sh`, `pcs.py`, `tzload.dis`, `gtod-stack.txt`,
  `fpwalk.py`, `lldbdis.sh`.
- **Run sets:** `vm1/`, `ent1/`, `ent2/`, `ent3/` (+ `ent3.txt`), `ctl-base/`, `ctl-fix/`, with
  `loopvm.sh`, `loopent.sh`, `loopent2.sh`, `rule.py`.
- **Validation:** `val-log-{base,fix}.txt`, `vc-{census,cfix}.txt`, with `val-log.sh` and
  `val-commit.sh`.
- **Patches and source copies:** the diagnostic patches `diag-box.patch` (entropy/time pinning) and
  `diag-core.patch` (placement log); the fix in `src-fix/` and `fix.diff`; the mutant in
  `src-mut/`; census + fix in `src-cfix/`; the fixture `vmalign_dyn.c`; `oldtrace-on-fix.err`.
