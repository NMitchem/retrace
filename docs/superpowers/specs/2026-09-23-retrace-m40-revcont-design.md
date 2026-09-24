# M40-revcont — `reverse-continue` in one pass, watch hits resolved by address, memory that is freed

**Date:** 2026-09-23. **Branch:** `m40-revcont` from `main` at the M39 merge (`786bf2b`).
**Companion:** `2026-09-23-retrace-m40-revcont-measurements.md` (t0, taken before this spec).
§2 cites it, and nothing in this document claims a measurement the companion does not carry.
**Approach:** option A of three, chosen by the operator on 2026-09-23 (R1).

## 1. Purpose

M39 reached rung 8: real CPython runs a script, crashes on a pointer it computed, and
`reverse-continue` walks back to the store. But that walk took **3.42 hours**, and two of them at
once exhausted 24 GB of RAM and 63 GB of swap. The headline demo is *reachable* but not *usable*,
and M39's demo transcript (Task 7c) was deferred here because producing it meant re-measuring the
defect (M39 R17). This milestone makes the demo usable. On the way it fixes a wrong answer the
debugger has been giving since M5: forward `continue` with a watchpoint can name an instruction
that never writes the watched memory.

It changes only the debugger's positioning (`crates/retrace/src/debug.rs`,
`crates/retrace-core`'s session and checkpoint layer) and `Box_`'s memory ownership. It adds no
dispatch arm, does not change the trace format, and does not touch record.

## 2. What was measured before this spec was written

The companion is the record. In summary, for the rung-8 recording (1,146 landmarks, P = (1144, 2470)):

- **There are 5 real writes to the watched cell** (M5). One native pass finds all 5, stepping over
  each hit in place, in **11.55 s CPU**.
- **`reverse-continue` pays ~20.8 s CPU per iteration and iterates over false hits** (M2). All 19
  capped iterations were the same store pc in window 1126, mostly 6 instructions apart.
  `resolve_hit_k` matches by **pc**, and that store ran **183 times** in window 1126 before
  reaching the watched cell (M6).
- **Forward `continue` resolves to the wrong instruction** (M7): it printed `resolved (1126, 29627)`,
  but the real write is at (1126, 1,765,682), and `stepi` over the "hit" leaves the cell unchanged.
- **Stepping with a watchpoint armed resolves exactly** (M6): a clean pre-retire `EC=0x34` stop at
  the real write, a clean retire after disarming, and clean stepping after re-arming. The code's
  "never armed while single-stepping" rule was an unmeasured analogy with breakpoints.
- **Every session re-decodes the whole 97.6 MB trace** (M3): 64 % in a bit-at-a-time `crc32`,
  36 % in bincode.
- **Every dropped `Box_` leaks its guest memory** (M4): ~55 MB and ~1,580 mappings per session,
  because `alloc_pages` `mmap`s and `Box_` has no `Drop`.

## 3. Design

### 3a. One exact resolver for every hit

Replace `resolve_hit_k(trace, cache, n, pc, from_k)` with a resolver that takes a **hit kind and an
ordinal**: "the position of the *m*-th hit of this kind in window `n`, counting from `from_k`":

- **Watch hits:** single-step from `(n, from_k)` **with the watchpoints armed**. A step that stops
  with the watchpoint exception class is a hit. The *m*-th is the answer (pre-retire, K = steps
  retired so far). Each earlier hit is stepped over in place (disarm, one step, re-arm).
- **Breakpoint hits:** single-step with breakpoints **disarmed** and compare the pc against the
  armed addresses before each step. The *m*-th match is the answer. For a breakpoint, pc equality
  *is* the definition of a hit, so this was never wrong. It only needs the ordinal so a breakpoint
  in a loop costs one resolution rather than one per pass through the loop.

This needs one new primitive on `ReplaySession`: a single step that **reports** a watchpoint stop
instead of treating it as a fault. Today `step_insns` hands any `Stop::Other` to `page_in_cache` and
`commit_reserved_page`, which read the FAR as an IPA. The new step must classify the exception
first. A watchpoint stop is never handed to those two functions. `step_insns`, `window_len_here` and
every other existing caller keep their contracts unchanged.

`arm_hw_watchpoint`'s "NEVER while single-stepping" doc is **superseded for watchpoints**, citing
M6 (R2). It **stands for breakpoints**, whose pre-retire fire at the current pc would repeat
forever.

### 3b. `reverse-continue` is one forward pass plus one resolution

Today the loop opens two sessions per hit and resolves every hit it passes. The new command:

1. **Scan.** One session from `(1, 0)` with the breakpoints and watchpoints armed. It `advance()`s
   at native speed until it reaches landmark `pn`. At each hit it records a candidate
   `(n, kind, ordinal-in-window, thread, watched/pc)` and steps over the hit in place. A syscall
   write to a watched range (`Advance::WatchSyscall`) is a candidate at `(n, 0)`, as today.
2. **The partial window.** From `(pn, 0)` it single-steps exactly `pk` instructions, with watches
   armed and breakpoint pcs compared, so hits before P in P's own window are counted exactly and
   hits at or after P are not. If P is a terminal position (parked at the exit or crash), the scan
   simply runs to the end.
3. **Choose.** The **last** candidate that passes the thread filter is the answer. Scoped-out hits
   still occupy ordinals (R4). This preserves M15 Task 8's rule that the scan walks *through* a
   scoped-out hit.
4. **Resolve** only that candidate with §3a. Nothing needs resolving for a `WatchSyscall`, which is
   `(n, 0)`.

That is **at most three session opens per command**, however many hits there are: the scan, the
resolution, and the park at the answer, which today is a `reseek(n, k)`. The resolution's session
may be adopted as the parked session (with its watchpoints cleared, per the kept-session
invariant), which makes it two. With no earlier hit, it is the scan plus the park. The command's
output format does not change. When a breakpoint sits on a watched store, the breakpoint's step-over keeps the watches
armed, so both hits are recorded in order (breakpoint, then watch). Today's loop loses the watch hit
there, because its re-seek to `k + 1` steps over the store with nothing armed (R3).

### 3c. Forward `continue` keeps its scan and gains the exact resolver

The scan, the pre-step rule and the `kctx` rules (`kctx` for a watch, `kctx + 1` for a breakpoint)
stay as they are. Each hit is resolved as ordinal 1 of its kind from `kctx`. The transcript's shape
is unchanged. **Only K values change, and only where the old resolver was wrong.**

### 3d. A `Box_` frees its guest memory

`Backing` becomes the **owner** of its host allocation: dropping it `munmap`s. The two removal
sites, `unmap_overlapping` and `guest_munmap`, stop calling `munmap` themselves and let the removed
`Backing` drop after `vm.unmap`. `place_fixed`'s case-2 temporary, which never becomes a
`Backing`, keeps its explicit `munmap`.

`backings` is already declared after `vm`, so on a `Box_` drop the order is `hv_vcpu_destroy` →
`hv_vm_destroy` → `munmap`. **Host memory is never released while the VM can still map it.** The
field order becomes load-bearing a second time, and the comment above the struct must say so.

A process-global count of live backing bytes is kept (mapped by `alloc_pages` minus released) and
exposed read-only for the guard test (§4). It is a deterministic counter, not a measurement of RSS.

### 3e. The debugger decodes its trace once

`ReplaySession` holds its events behind a shared handle rather than an owned `Vec`. It gains
constructors that take an already-decoded trace, and `from_checkpoint` takes the decoded trace
instead of a path.

`CheckpointCache` is already documented as single-trace. It decodes the trace on first use and
holds it, so every `checkpointed_seek` and resolution opens its session without touching the file.
It asserts that later calls name the same path. **`checkpointed_seek`'s signature does not change,**
so the existing `checkpoint_seek.rs` tests compile as they are. M19's symbol table, which `Exec::new`
builds today with a second full `Reader::open` of the same file, is built from the cache's decoded
trace instead. `ReplaySession::open(path)`,
`replay()` and `seek()` keep decoding per call, as today. Only the debugger's hot path changes.

A process-global decode counter in `retrace-trace`, counted in `Reader::open_checked` (which
`Reader::open` delegates to), makes "once" assertable (§4). Today a debug session decodes
the file at least twice before its first command (the opening seek and the symbol read), plus once
per later seek.

### 3f. Name the watched address when a wider store covers it

`watched_of` keeps its two existing rules: the range containing the FAR, then the range overlapping
the FAR's aligned doubleword. When neither matches, it now picks **the first armed range, in slot
order, that intersects `[align_down(FAR, 64), FAR + 64)`**. That window spans the widest single
store, a 64-byte `DC ZVA` block, and a 32-byte `stp q`. Only if nothing intersects does it fall back
to the FAR itself. Outputs that are right today are unchanged, because the new rule runs only where
both old rules missed.

> **Correction at execution (R10, ledger Ruling 4; see §10).** The window above,
> `[align_down(FAR, 64), FAR + 64)`, and the "32-byte `stp q`" reasoning were wrong, and the rule
> landed differently. The Arm ARM bounds a watchpoint's FAR to the naturally aligned block (at most
> the 64-byte `DC ZVA` block) that contains a watched address the store wrote, so a covering store's
> FAR always shares the watched range's 64-byte block. The third rule is therefore **the first armed
> range, in slot order, that intersects the FAR's naturally aligned 64-byte block
> `[align_down(FAR, 64), align_down(FAR, 64) + 64)`**. `FAR + 64` admitted FARs from the block below,
> which no covering store can report, and named a wrong range for them. The text above is left as
> written; this note supersedes it.

## 4. Guards: each asserts the difference it makes

- **A new repo-owned fixture, `watchsweep`** (C, `-O0`). One store instruction writes every element
  of a buffer in a loop, and the watched element is well past the start. So the store pc runs on
  other addresses first: M7's class, reduced. A second, **different** instruction then writes the
  watched element again, and the program announces the element's address with a `write(1, …)`, as
  `WATCHLOOP` does. The ground truth for store positions comes from `watch_cli`'s independent oracle
  (step and read), which the watch machinery cannot influence. Tests:
  - **`continue` lands on the real write:** `watch T; continue; stepi; x T 8` shows the new value,
    and the resolved K equals the oracle's first K. **RED on today's tree.**
  - **`reverse-continue` from the exit lands on the second writer**, at the oracle's last K.
  - **`reverse-continue`'s cost does not grow with the hits it passes:** a unit test in `debug.rs`
    drives `Exec` on the fixture and asserts that one `reverse-continue` makes **≤ 3** seeks. That
    count comes from a new seek counter on `CheckpointCache`, a deterministic proxy like
    `total_single_steps`. The test also asserts that the trace decode counter moved by exactly 1
    across the whole script. **RED on today's tree**, which pays two seeks per run of the store
    instruction between the resume point and each real hit.
- **The leak:** a test opens and drops a session several times and asserts the live-backing count
  returns to its starting value each time. RED on today's tree.
- **`watched_of`:** unit cases for FARs that a store which really covers the range could report:
  8 bytes below (a 16-byte `stp`, rung 8's `0xa01722ac0` case), 24 bytes below (a 32-byte
  `stp q`), and 24 bytes *above* within the same 64-byte block (`DC ZVA`). Also a FAR outside the
  window, which keeps the fallback, and the existing exact and doubleword cases, unchanged.
  **Corrected at execution (R10; see §3f's note and §10):** the "24 bytes below (a 32-byte
  `stp q`)" case was wrong. That FAR lies in the 64-byte block *below* the range's, and a 32-byte
  `stp q` covering the range reports a FAR inside the range's own block, never its base. It landed
  as a **fallback** case, beside a second one 56 bytes below. The block's last byte was added as a
  match case (`DC ZVA`).
- **Rung 8** keeps `cpython_crash_e2e`'s assertions unchanged. Its cost is measured and reported
  under the §6 acceptance, not asserted, because it skips without Homebrew Python and so guards
  nothing on another machine.
- **Every existing debugger transcript must stay byte-identical** (`debug_cli`, `watch_cli`,
  `thread_watch_e2e`, `crashy_*`, `reverse_debug_e2e`, `checkpoint_seek`). If one moves, that is
  either a fixture that had been silently misresolved, to be shown with the oracle and recorded as a
  ruling, or a regression.

## 5. Task order and why

1. The `watchsweep` fixture and its RED tests: the continue position, the session count and the
   decode count.
2. The exact resolver (§3a) and forward `continue` on it (§3c). **This is the wrong-answer fix, so
   it comes first.**
3. The one-pass `reverse-continue` (§3b).
4. `Backing` ownership and the live-bytes guard (§3d).
5. Decode once (§3e).
6. `watched_of` (§3f).
7. Rung-8 measurement, and the close: the README's rung-8 demo transcript (M39 7c), the memory
   figures, the status log and the `CLAUDE.md` test list.

Tasks 4–6 are independent of one another, and each comes after 2–3 so the RED tests of Task 1 go
green from the algorithm first. What remains after that is resource cost.

## 6. Acceptance

- The Task 1 tests are RED on `786bf2b` and green after, **measured both ways**.
- **Rung 8, measured with `t0/prof.sh`** on the same recording shape:
  - `reverse-continue` costs **≤ 120 s CPU** in the dev build (expected ≈ 25–35 s: one pass
    ≈ 11.6 s per M5, plus one resolution);
  - peak RSS stays **≤ 1 GB**;
  - forward `continue` resolves to **(1126, 1,765,682)**;
  - `cpython_crash_e2e` passes, and its wall time is reported alongside the machine's load at the
    time.
- The demo script from M39 (`continue; watch <cell> 8; reverse-continue; where; x <cell> 8; stepi;
  x <cell> 8`) runs to completion standalone, and its transcript and memory figures go into the
  README.
- The gate is green, reconciled file-by-file against M39's 629 / 0 / 9 over 138. `TRACE_MAGIC` does
  not move, and `verify_thread` stays at **seven** sites. No dispatch arm changes.

## 7. What this milestone deliberately does not do

- **A faster `crc32`.** It is 64 % of a decode (M3), and every `replay` and test would gain. But
  after §3e the debugger decodes once, so it no longer bears on this milestone's goal. It is owed.
- **A release or opt-level profile for tests.** That changes the whole gate's build, and it is out
  of scope.
- **A backward, checkpoint-segmented search** (option C, rr-style). One forward pass already costs
  about one replay. That optimisation is for a recording where one pass itself is too slow, and none
  is measured.
- **The M39 owed items**: the stage-1 alias invisible to address readers, and symbols for
  runtime-loaded dylibs. Also untouched: `reverse-stepi`'s cost beyond what §3e gives it for free,
  lldb, async signals, exec-in-place, and the missing-row set.

## 8. Rulings (made while writing this spec)

- **R1 — option A**, by the operator on 2026-09-23, over B (resolver and leak only: cost stays
  proportional to real hits) and C (backward segmented search: more machinery than one pass needs).
- **R2 — watchpoints may be armed while single-stepping, for resolution,** on M6's measurement.
  Breakpoints still may not.
- **R3 — a breakpoint on a watched store yields both hits**, a deliberate improvement over the old
  loop. No existing transcript covers it.
- **R4 — ordinals count scoped-out hits**, because the hardware fires for them. The thread filter
  applies only when choosing, as M15 Task 8 made it.
- **R5 — acceptance is in CPU seconds and counts**, because the operator runs concurrent sessions
  (t0 conditions).
- **R6 — M39's Task 7c is carried here** (M39 R17). The transcript is taken after §6's numbers are
  met, on a standalone run.

## 9. Gate prediction

M39 closed at **629 / 0 / 9 over 138**. This adds one e2e test file (`watchsweep_e2e`: +1 binary,
~2 tests), ~3 unit tests in `debug.rs` (session count, decode count, `watched_of`), ~1 leak test and
~1 resolver test in `retrace-core` or `retrace-box`, and no `#[ignore]`. Prediction: **≈ 636 / 0 / 9
over 139**, to be reconciled file-by-file. The number is a prediction, not a target.

## 10. Outcome

*(Filled in at the close, 2026-09-24, on branch `m40-revcont`. The status log's M40 section
carries the full account: every RED → green, the rulings R7–R12, the final review and its fix
wave, the owed list.)*

**Delivered.** All four fixes and the naming rule landed as designed, except where R10 corrected
§3f. `reverse-continue` on rung 8 went from 3.42 h of wall-clock to **0.53 s of CPU**. The gate is
**640 / 0 / 9 over 140** on the tree of the final-review fix commit (the close's first run, on
`5cea9a8` before the fix wave's one added test, read 639 / 0 / 9 over 140).

**§6, item by item.**

- *The Task 1 tests are RED on `786bf2b` and green after, measured both ways.* **Met.** Measured red
  on `8cb075d`, which is `786bf2b`'s debugger plus the fixture and the two counters (the tests could
  not exist on `786bf2b` itself). `continue` printed `resolved (1, 8)` where the real write is at
  K 208, and went green at Task 2. The walk-back test failed at its third assertion with a false
  writer at `(1, 203)` (R7), and went green at Task 2. The seek guard read **86** seeks (≤ 3
  required); it fell to 6 at Task 2 and went green at Task 3. The decode guard read **90** decodes
  (1 required); it fell to 10 at Task 2 and 7 at Task 3, and went green at Task 5.
- *`reverse-continue` ≤ 120 s CPU in the dev build (expected ≈ 25–35 s).* **Met: 0.53 s**, which is
  8.64 s (run B) − 8.11 s (run A) on t0's own recording (`t0/crash.bin`). The expectation was
  **wrong**, and the correction is below. The method was the plan's subtraction under
  `/usr/bin/time -l` and a 900 s `perl` alarm cap, not `t0/prof.sh`. It is the same CPU-seconds
  posture (R5), on the very recording rather than one of the same shape.
- *Peak RSS ≤ 1 GB.* **Met: 422,379,520 B ≈ 403 MB** (run B).
- *Forward `continue` resolves to (1126, 1,765,682).* **Met exactly.** After one `stepi` the cell
  reads `0x701238000`, t0 M5's value, and the hit line names the watched `0xa01722ac8`, not the FAR.
- *`cpython_crash_e2e` passes; wall time reported with the load.* **Met:** `1 passed`,
  `finished in 40.02s`. The load average was 1.36 before and 1.92 after. M39 measured 12,364 s
  standalone and 20,740 s contended.
- *The demo runs to completion standalone; its transcript and memory figures go into the README.*
  **Met, with one substitution.** Record exited 139, replay 139, debug 0. The transcript is in the
  README's rung-8 entry, verbatim. The demo's 10-second RSS sampler recorded nothing, because the
  session finished before its first tick, so the README's memory figure is run B's RSS.
- *The gate is green, reconciled file-by-file against 629 / 0 / 9 over 138; `TRACE_MAGIC` does not
  move; `verify_thread` stays at seven; no dispatch arm changes.* **Met.** The gate is
  640 / 0 / 9 over 140, every chunk exit 0 and clippy clean: +11 tests over five files and two new
  binaries, reconciled file by file in the status log. No changed line names `TRACE_MAGIC`, and the constant is
  byte-identical. `self.verify_thread(` still has **7** sites. Every `retrace-core` hunk lies outside
  `record_box` and `ReplaySession::advance`.

**Predictions, confirmed or corrected.**

- **Confirmed:** M6's armed-stepping resolver (§3a) became `step_watched` + `resolve_nth` and
  resolves rung 8 exactly. "At most three session opens" (§3b) is pinned by the seek guard. The
  optional adoption of the resolution's session as the parked one was not taken: the command is
  scan + resolve + `reseek`, or scan + `reseek` when nothing earlier is found. The memory fix (§3d)
  and decode-once (§3e) did what they said. **No existing transcript assertion moved** (§4).
- **Corrected — §6's ≈ 25–35 s.** It assumed one pass costs ≈ 11.6 s "per M5". But M5's 11.55 s
  *included* the trace decode, and M3 had put every sample of a cold seek inside the decode. After
  §3e the debug session decodes once. Runs A and B both pay that decode, so the subtraction removes
  it, and what run B adds is the native forward pass, phase 2's single-steps and the resolution's.
  That attribution is inference from M3, M5 and the counters; the subtraction does not itself
  divide the 0.53 s.
- **Corrected — §3f and §4 (R10).** The window `[align_down(FAR, 64), FAR + 64)` and the
  "24 bytes below (32-byte `stp q`)" example were wrong. The rule that landed is the FAR's naturally
  aligned 64-byte block, per the Arm ARM's FAR bound. §3f and §4 carry marked notes.
- **Corrected — §4's fixture.** "C, `-O0`" landed as freestanding asm (`asm/watchsweep.s`), as the
  plan wrote it. The "~1 resolver test in `retrace-core` or `retrace-box`" landed in
  `watchsweep_e2e` as its third test.
- **Corrected — §9.** It predicted ≈ 636 / 0 / 9 over 139, and the gate measured 640 / 0 / 9 over
  140. It did not count `watchsweep_guest_parses` (+1), the two `watch_cli` guards Task 3's review
  added (+2), the R4 guard the final review added (+1), or the leak test's own binary
  (`backingfree`, +1 binary). `watchsweep_e2e` has four tests, not ~2.
- **Corrected — §1.** §1 said only forward `continue` gave a wrong answer. On the pre-fix tree
  `reverse-continue` did too (R7).

**Found without being sought.**

- **R8.** The plan's one-pass code counted a syscall-write hit *at* P, so `reverse-continue` right
  after a forward `continue` to a syscall hit repeated it forever. It was caught at review, before
  merge, and guarded by a test that was RED with the fix reverted.
- **Task 4's review** found that all four `Box_` constructors unwound `backings` before `vm`, which
  would release host pages under a live stage-2 mapping. The plan's own audit list had missed it.
- **Two inherited, silent gaps**, now written down rather than fixed. First, breakpoint hit counting
  at a thread switch: at (n, 0), `pc()` is the outgoing thread's, so a `reverse-continue` to a
  breakpoint can land one hit off. It is a README Known limit. Second, a breakpoint on a handled
  fault's instruction ends `step_watched` in `Err`. The pre-M40 loop had both.
- **t0 M4 answered an open M39 question.** Rung 8's tens of gigabytes were never the 256 MiB
  checkpoint cache. They were every dropped session's leaked guest memory, ~55 MB each.
- **The close's own invariant command was wrong.** The Task 8 brief's
  `git diff 786bf2b -- crates/retrace-trace/src/lib.rs | grep -c TRACE_MAGIC` prints 1, because an
  unchanged context line in the diff names the constant. The changed-lines form prints 0.
- **Forward `continue` skips a breakpoint hit that its own pre-step lands on (R11, owed).** It
  resolves from `kctx + 1`, so two adjacent breakpoints lose the second's hit: silently in a loop
  (`watchsweep` resolves `(1, 14)` for a correct `(1, 9)`), loudly in straight-line code (`FILEIO`
  exits 5). It is inherited from M3 (`e41aff8`), and §3c kept that path. The final review found it;
  it is a README Known limit, and the fix (resolve from `kctx`) is owed.
- **The final review's fix wave** added R4's missing guard, which was RED at `(1, 208)` for a
  correct `(1, 213)` with scoped-out hits dropped from the ordinal. It also made `step_watched`
  reject a breakpoint stop and made `reverse-continue` refuse to move forward. Two more changes
  were corrections: phase 2 now reads a watch hit's pc after the step, and two comments that
  claimed too much were narrowed. It also named the phantom direction of the thread-switch
  limit.
