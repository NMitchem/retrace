# M41-hitorder — every hit counted once, in one order, on the thread that runs it

**Date:** 2026-09-24. **Branch:** `m41-hitorder` from `main` at the M40 merge (`c68ba6d`).
**Companion:** `2026-09-24-retrace-m41-hitorder-measurements.md` (t0, taken before this spec). §2
cites it. Where this document states something from reading code rather than measurement, it says
so and names the Task 0 measurement that owes it.
**Approach:** A of two for the thread switch (R1), with scope "fixes plus a hit oracle" (R2), both
chosen by the operator on 2026-09-24.

## 1. Purpose

M42 is meant to put lldb in front of this debugger. Before that, its answers have to be right. M40
closed with two silent wrong answers on its owed list, R11 (forward `continue` skips a breakpoint
its own pre-step lands on) and T3 (breakpoint counting is blind at a thread switch). Measuring them
for this spec found that they are two instances of a wider defect: **the debugger has no defined
order for hits, and no single definition of where it stands.** Four more skips follow from that
(M3–M6), and T3 also reaches forward `continue`, not only `reverse-continue` (M8).

This milestone defines both things, the order and the position, and makes `continue` and
`reverse-continue` obey them. It then gives the debugger its own oracle: a brute-force enumeration of
every hit, which each command's answers are checked against. Every bug on this list was found by
review, not by a test. The oracle is how the next one gets found by a test.

It changes the debugger (`crates/retrace/src/debug.rs`), `ReplaySession`'s event epilogue and one
new stepping primitive (`crates/retrace-core`), and one idempotent `Box_` method. It adds no
dispatch arm, does not change the trace format, and does not touch record.

## 2. What was measured before this spec was written

The companion is the record. In summary, on `c68ba6d`:

- **R11, silent (M1):** `break 0x1000003a0; break 0x1000003a4; continue; continue` on `watchsweep`
  resolves `(1, 14)`. The right answer is `(1, 9)`. Exit 0.
- **R11, loud (M2):** two adjacent breakpoints across `fileio`'s `read` exit 5. Its message, "window 4
  ends after 0 instruction(s)", is not a window length: window 4 has 6 instructions. The `0` counts
  within `resolve_nth`'s one-instruction `step_insns(1)` call. Three controls rule out a poisoned
  seek.
- **A breakpoint on a watched store (M3, M4):** forward loses the watch half of the pair (the
  pre-step re-seeks with nothing armed), and backward loses the breakpoint half ("strictly before
  (1, 328)" excludes the breakpoint *at* (1, 328), which precedes the store).
- **A syscall write plus a breakpoint at (n, 0) (M5, M6):** forward loses the breakpoint (the
  pre-step steps off it unreported), and backward loses the syscall hit ("no earlier hit").
- **Arriving at (n, 0) by stepping (M7):** `reverse-continue` does not find the syscall write that
  happened just before it. This is M40's deliberate exclusion (its Ruling 2).
- **T3 (M8), on the existing `threadrust`:** at (263, 0) main has just blocked. `continue` reports a
  **phantom** hit on main's resume pc, which nothing executes there (the next retire is thread 1's).
  The **real** hit at (269, 0) is reachable in neither direction (exit 5), and a breakpoint on the
  child's first instruction at (263, 0) also exits 5. Forward `continue` is affected, which the
  README's Known limit does not say.

## 3. Design

### 3a. The thread switch happens when the event finishes (approach A)

Today `Box_::run()` and `step()` switch threads **on entry** (`retrace-box/src/lib.rs`, M14 Task 9).
So between a blocking event and the next entry, `pc()`, `current_thread()` and the registers
describe the thread that just blocked or exited. M15 wrote that down as a definition
(`ReplaySession::current_thread`'s doc: "at (N, 0) this names the thread that ISSUED landmark N"). T3
is that definition meeting code that counts breakpoints by `pc()`.

- **New `Box_::settle_schedule()`:** `if self.threads.needs_reschedule() { self.schedule_after_block() }`.
  It is idempotent and is what `run()` and `step()` already do on entry. Both keep their entry check
  as a safety net, which becomes a no-op on replay.
- **`ReplaySession::finish_event` calls it**, after computing `WatchSyscall`'s `thread`, so a
  syscall write is still attributed to the thread that issued it. Every path that consumes an event
  and returns `Event` or `WatchSyscall` goes through `finish_event`: grepped at spec time, where
  `Ok(Advance::Event)` and `Advance::WatchSyscall { … }` are constructed only inside `finish_event`
  (called from 29 sites), and re-grepped in Task 2. `Exited`, `Break` and `Watch` do not, and need not: an exit ends the run, and a
  mid-window stop has no pending switch (M15 R1's `debug_assert`).
- **The invariant it buys**, written into `pc()`'s and `current_thread()`'s docs in place of M15's
  paragraph: **at every position (n, k), `pc()` and `current_thread()` name the instruction the next
  step retires and the thread that retires it.**
- **Determinism is unaffected.** `verify_thread` runs inside each dispatch arm, before
  `finish_event`, so it still compares against the issuing thread, and it stays at **seven** sites.
  Checkpoints already carry the thread table (`BoxState.threads`), so a checkpoint at (n, 0) now
  captures the settled state, and a restore finds no pending switch. Record never calls
  `finish_event` and is untouched. The schedule is the same pure function of the syscall sequence: it
  is taken at the end of the event rather than on the next entry, and nothing on replay runs in
  between except the debugger's own reads.
- **Visible change:** at a **blocking** boundary (`__ulock_wait`, `bsdthread_terminate`,
  `semaphore_wait_trap`, a workq park), `where`, `threads` and `regs` show the incoming thread. No
  existing debugger test parks at a blocking boundary (`debug_cli`'s thread tests use `write` and
  `bsdthread_create`, which do not switch). `blockedctx.rs`'s header comment ("the switch that saves
  it happens on the next `run()`") is corrected. Its two assertions should hold, because the saved
  context is the same bytes saved one call earlier. Task 0 measures that rather than assuming it.
- **A deadlock panic** (`schedule_after_block` finding no runnable thread) now fires at the end of
  the event that caused it rather than on the next entry. Same panic, one call earlier.

### 3b. Hit order and the cursor

**Order.** A hit is at `(n, k, phase)`, and hits are totally ordered by that triple. At one
coordinate the phases are ordered as the hardware produces them:

1. **`Sys`**: a syscall's recorded write to a watched range. Only at k = 0: the event that ended
   window n−1 wrote it before the instruction at (n, 0) runs.
2. **`Bp`**: the instruction at (n, k) is about to execute, and its address is a breakpoint.
3. **`Watch`**: that instruction's store to a watched range, stopped before it retires.

**Cursor.** `Exec` holds `(n, k, phase)`. `last_watch_hit` is removed (it was `phase == Watch` in
disguise).

- `continue` answers **the first hit greater than the cursor**. `reverse-continue` answers **the
  last hit less than it**.
- Reporting a hit sets the cursor to that hit.
- Arriving anywhere else (the opening position, `stepi`, `reverse-stepi`, a terminal park) sets the
  cursor to `(n, k, Bp)` (R4). This is gdb's rule: a breakpoint at the pc you stand on is reported
  in neither direction, and a watched store you stepped up to still fires going forward. For every
  arrival except (n, 0) with a syscall hit behind it, that is today's behavior. For that one, it is
  M7's case, and `reverse-continue` now finds the write (R5).

All positions at one (n, k) share one machine state: before the instruction at (n, k). A
watchpoint stop is pre-retire, so a `Watch`-phase park is the same state as a `Bp`-phase one. Only
the cursor differs, so no session has to encode the phase.

### 3c. `continue` under the cursor, with R11

The pre-step becomes **finish this coordinate, then scan**:

1. If `phase == Sys` and `pc()` is a breakpoint: report `Bp` at (n, 0) and stop. **Fixes M5.**
2. Else, if `pc()` is a breakpoint or `phase == Watch`: step the instruction at (n, k) with
   `step_watched`, with the watches armed unless `phase == Watch`.
   - `Stepped::Watch`: report `Watch` at (n, k) and stop. **Fixes M3.** A scoped-out watch hit sets
     the cursor to (n, k, Watch) without a report and returns to step 2. That replaces today's
     recursion into `cmd_continue` with a loop (R8), because this is the code that recursion lived
     in.
   - `Retired`: now at (n, k + 1), with nothing there examined yet.
   - `AtTrap`: cross the boundary exactly as today, with the watches armed for the one event.
     `WatchSyscall` reports `Sys` at (n + 1, 0), `Exited` parks at the terminal, and `Event` leaves
     the cursor at (n + 1, 0) with only `Sys` consumed.
3. Otherwise (no breakpoint at `pc()` and `phase < Watch`), there's nothing to finish, and the
   hardware scan below would find a watch at (n, k) itself.
4. **Scan** as today, with breakpoints and watches armed. `Break` and `Watch` both resolve from
   **`kctx`** (R11: `debug.rs:707`'s `kctx + 1` becomes `kctx`). This is safe in every case:
   - Scanning from a coordinate whose `Bp` phase is still ahead (after a step or a crossing), the
     hit may be *at* `kctx`, and `kctx + 1` skipped it.
   - Scanning from a coordinate whose `Bp` phase is passed (step 3), `pc()` there is not a
     breakpoint, or step 2 would have run. So a breakpoint hit cannot resolve spuriously to `kctx`.
   - In a later window, `kctx = 0` and a hit at (n, 0) is found instead of skipped.

   A **scoped-out** `Watch` from the scan resolves as today, sets the cursor to (n, k, Watch)
   without a report, reseeks there, and goes back to step 2, which steps over it. That's a loop,
   not today's recursion (R8).

   **Fixes M1, M2.** The `Event` arm's boundary check reads `pc()`, which §3a makes the incoming
   thread's. **Fixes M8 forward.**

A fault during step 2 (a breakpoint on a faulting instruction) propagates as the same error it does
today. The handled-fault case stays owed (§7).

### 3d. `reverse-continue` under the cursor

The one-pass scan and the single resolution stay as M40 built them. What changes is what "before P"
means, with P now the triple `(pn, pk, pphase)`:

- Phase 1's `WatchSyscall` at window n counts when `(n, 0, Sys) < P`, replacing `(n, 0) < (pn, pk)`.
  At P = (pn, 0, Bp) (an arrival) it counts. **Fixes M6, M7.** At P = (pn, 0, Sys) (the hit itself)
  it does not, so M40's stuck-loop fix (Ruling 2, pinned by
  `reverse_continue_from_the_syscall_hit_itself_finds_nothing_earlier`) still holds.
- Phase 2 steps K < pk as today. **At K = pk, when `pphase == Watch`, the `Bp` at (pn, pk) counts**
  if `pc()` is a breakpoint. **Fixes M4.**
- The resolver's pc-before-step read at (n, 0) is the incoming thread's after §3a. **Fixes M8
  backward.**
- M40's defence `before_p` compares triples.
- The answer sets the cursor: `Bp` → (n, k, Bp), `Watch` → (n, k, Watch), `WatchSys` → (n, 0, Sys).

### 3e. The hit oracle

The oracle is test code in `crates/retrace/tests/util/`:
`enumerate_hits(trace, bps, watches, from_n) -> Vec<Hit>`, where
`Hit { n, k, phase, pc, thread }`. It deliberately shares none of the debugger's machinery: no
`resolve_nth`, no pre-step, no scan/resolve split, and no pc-based counting.

- **New primitive:** `ReplaySession::step_armed()`, one single step with breakpoints **and**
  watches armed. It reports `Retired`, `Break`, `Watch` or `AtTrap`, and handles
  deterministic-replay faults exactly as `step_watched` does. `step_watched` keeps its M40
  contract (a breakpoint stop is an error there). The premise, a breakpoint armed at the current pc
  stops pre-retire and one disarmed step then retires, is owed to Task 0.
- **Procedure:** seek to `(from_n, 0)` and arm everything, then for each step:
  - `Break` → record `Bp`, disarm breakpoints, and step once with the watches still armed. `Watch`
    there → record `Watch` at the same (n, k). Then re-arm.
  - `Watch` → record `Watch`, then step over it with everything disarmed.
  - `AtTrap` → cross with `advance()`, watches armed. `WatchSyscall` → record `Sys` at (n + 1, 0).
    `Exited` ends the list.

  Each record reads `pc()` and `current_thread()` **after** the stop. `step()` switches threads on
  entry whatever §3a does, so the oracle sees the running thread even if §3a is reverted: it is
  independent of the fix it checks.
- **Three checks per armed fixture:**
  1. **Forward chain:** from the opening position, `continue` until exit. The reported hits equal the
     list.
  2. **Backward chain:** from the terminal, `reverse-continue` until "no earlier hit". The hits
     equal the list reversed.
  3. **Zig-zag:** each hit reached by `reverse-continue` must give its successor on `continue`, and
     each hit reached by `continue` must give its predecessor on `reverse-continue`. This catches
     cursor state that depends on how you arrived.

  They run through `retrace debug --script`, and the transcripts are parsed from the `hit …` /
  `resolved (n, k)` lines, plus `where` for pc and thread.
- **Watches are unscoped in the oracle checks.** The thread filter keeps its existing tests
  (`thread_watch_e2e`, `watchsweep_e2e`'s scoped-ordinal test).

## 4. Guards: each asserts the difference it makes

**Named regressions**, one per measurement, each pinned to its exact coordinates and RED on
`c68ba6d`:

| Measurement | Assertion |
|---|---|
| M1 | `resolved (1, 9)` |
| M2 | Exit 0, with the second hit at (4, 0) |
| M3 | Third `continue` reports the watch at (1, 328) |
| M4 | Second `reverse-continue` reports the breakpoint at (1, 328) |
| M5 | Second `continue` reports the breakpoint at (4, 0) |
| M6 | Second `reverse-continue` reports the syscall hit at (4, 0) |
| M7 | `reverse-continue` after the `reverse-stepi` reports the syscall hit at (4, 0) |
| M8 | Forward: exactly one hit on main's resume pc, at the landmark after the child's 361 (M8: 269), k = 0, thread 0, and no hit at the landmark after main's 515 (M8: 263). Backward from exit: the same single hit. The child's-first-instruction breakpoint resolves at (263, 0) on thread 1 |

Addresses and landmarks are **discovered**, never hardcoded: `threadrust`'s are shared-cache
addresses, found the way `debug_cli`'s `discover_*` helpers find theirs.

**The new invariant (§3a):** at the landmark after main's blocking 515 in `threadrust`, `where`
names thread 1 and thread 1's first pc, and `threads` marks thread 1 current. It is RED today (M8's
`where` shows thread 0).

**The oracle's three checks** run on five armings:

| Fixture | Arming | Carries |
|---|---|---|
| `watchsweep` | `{0x3a0, 0x3a4}` | M1 |
| `watchsweep` | `0x3b4` + watch `buf[40]` | M3, M4 |
| `fileio` | `{read svc, next}` | M2 |
| `fileio` | `0x3c8` + watch `buf` | M5–M7 |
| `threadrust` | `{main resume, child first}` | M8 |

**Every arming is RED on `c68ba6d`. That is the oracle's positive control**, proof it can fail,
which M28 taught this project to demand of any tripwire. A self-check pins each list's length to a
count derived from the fixture source (e.g. 128 hits for `watchsweep` `{0x3a0, 0x3a4}`: 64 passes
× 2).

**`stepi` arrival:** `stepi` onto a breakpoint, then `continue`, does not report it, and neither does
`reverse-continue` (R4).

**Unchanged by construction, and checked:**
- `blockedctx.rs`'s two assertions.
- `verify_thread` at seven sites.
- `TRACE_MAGIC` at `RT\x00\x0a`.
- No dispatch arm in the diff: every `retrace-core` hunk lies outside `record_box` and the arms of
  `ReplaySession::advance`.

**The audit.** Every existing test that asserts on `continue`/`reverse-continue` output is listed in
the ledger:
- `debug_cli`, `watch_cli`, `watch`, `watch_dyn`
- `watchsweep_e2e`, `thread_watch_e2e`
- `crashy_cli`, `reverse_debug_e2e`
- `cpython_crash_e2e`, `sigcatch_dyn_e2e`
- `debug.rs`'s unit tests

Each moved assertion is classified: either it pinned a skip (show it with the oracle), or the cursor
changed the answer on purpose (so far only R5's case, which no existing test pins, read not
measured). Anything else is a regression.

## 5. Task order and why

0. **Measure the three owed premises**:
   - `step_armed`'s premise.
   - `threadrust`'s exhaustive-step cost against a budget of **≤ 120 s CPU** for all three checks,
     dev build.
   - `blockedctx` under approach A, prototyped and not committed.

   If the budget fails, the `threadrust` oracle starts at a declared landmark (the `bsdthread_create`
   one), with its breakpoints chosen on thread-only paths. If that fails too, a freestanding asm
   threaded fixture is built (R6).
1. **The REDs:** the named regressions, the invariant test, `step_armed`, the oracle, and its three
   checks on the five armings. Every test that asks the debugger a question is RED. `step_armed`'s
   own test and the oracle's self-checks are green, because they check the ground truth, not the
   debugger.
2. **§3a:** `settle_schedule` in `finish_event` and the rewritten docs. The M8 tests and the
   `threadrust` oracle arming go green, forward and backward.
3. **§3b + §3c:** the cursor and `continue`, with R11. M1, M2, M3 and M5 go green.
4. **§3d:** `reverse-continue` under the cursor. M4, M6 and M7 go green, and so do all five oracle
   armings.
5. **The audit, and R7's diagnostic.** `resolve_nth`'s breakpoint loop names the K it reached
   instead of a one-call step count.
6. **Close:**
   - The gate.
   - README: the two Known limits are removed, and cursor semantics are added to "What works today".
   - The status-log section.
   - CLAUDE.md's gate list gains the new e2e file.

§3a goes before §3c because R11's fix is only safe once `pc()` at (n, 0) is the running thread (§3c
step 4's second case reads it).

## 6. Acceptance

- Every Task 1 test is RED on `c68ba6d` and green after, **measured both ways**, with the RED
  transcripts in the ledger.
- All five oracle armings pass all three checks. The `threadrust` arming is within Task 0's budget,
  or in its declared fallback.
- The gate is green, reconciled file by file against M40's **640 / 0 / 9 over 140**. `TRACE_MAGIC`
  does not move, `verify_thread` stays at seven, no dispatch arm changes, and there is no new
  `#[ignore]`.

## 7. Halt rules, and what this milestone deliberately does not do

**Halt rules:**
- **If §3a breaks any existing gate other than by the boundary-thread change it predicts, stop and
  report.** It would mean something depends on the lazy switch, and that is a design question, not
  a patch.
- **If the oracle finds a mismatch outside hit accounting** (a resolver/hardware disagreement, a
  replay divergence, a fault), it becomes a README Known limit and an owed item. The milestone
  doesn't widen for it.

**Not done:**
- **lldb** is M42.
- **A breakpoint on the faulting instruction of a handled fault** still errors (M40 T3 minor),
  unless an oracle fixture happens to reach it, in which case the halt rule applies.
- **The `crc32` speedup and `reverse-stepi`'s cost**, both M40's owed items.
- **`resolve_nth`'s parameter count stays at seven.** Clippy's `too_many_arguments` fires at eight,
  so the phase is not passed to it: the cursor lives in `Exec`.
- **M40's other deferred review minors**, except R8's loop, which falls out of §3c.
- **Everything on M39's and M38's carried lists.**

## 8. Rulings (made while writing this spec)

- **R1 — approach A** (switch when the event finishes), by the operator, over B (lookahead
  accessors `next_pc()`/`next_thread()`). B would leave M15's definition in place and make each
  counting site remember the lookahead. That's the "nothing structural couples them" trap CLAUDE.md
  records for `verify_thread`, and lldb's register reads would inherit it.
- **R2 — scope is the fixes plus the hit oracle**, by the operator, over the fixes alone and over
  the fixes plus M40's debugger minors.
- **R3 — the same-coordinate skips are in scope**, by the operator on the design's second section.
  M6 was found after that approval, by t0. It is the same class, fixed by the same §3d rule, so it
  is included under R3.
- **R4 — an arrival's phase is `Bp`.** gdb's rule. It keeps every arrival's forward behavior as
  today (`continue_from_a_breakpoint_steps_over_it`,
  `continue_after_reverse_stepi_onto_boundary_bp`).
- **R5 — `reverse-continue` from an arrival at (n, 0) finds a syscall write at (n, 0).** This
  deliberately reverses M40's exclusion (M7) for arrivals. It is kept for the hit itself, where the
  cursor sits at `Sys`, so M40 Ruling 2's stuck loop cannot recur.
- **R6 — `threadrust` is the T3 fixture.** It is measured (M8) and carries both shapes. A new asm
  threaded fixture is built only if Task 0's budget and fallback both fail.
- **R7 — `resolve_nth`'s breakpoint-loop error names the K it reached.** The resolver is touched
  anyway, and M2 shows the current message misleads the reader of every loud failure in this class.
- **R8 — the scoped-out watch recursion becomes a loop**, because §3c rewrites the code it lives in.
  M40 recorded the recursion as able to overflow the stack on a hot scoped-out write loop.

## 9. Gate prediction

M40 closed at **640 / 0 / 9 over 140**. Expected additions:
- One e2e file (`hitorder_e2e`: +1 binary), with ~9 named regressions, 1 invariant test, 5 oracle
  armings and 5 self-checks.
- ~2 unit tests in `debug.rs` (cursor order, `stepi` arrival).
- ~1 `step_armed` test in `retrace-core`.
- No `#[ignore]`.

**Prediction: ≈ 663 / 0 / 9 over 141**, reconciled file by file. It's a prediction, not a target.

## 10. Outcome

Filled at the close (Task 5), on the tree of `d16fa97` plus the close's comment-only sweep of
`debug_cli.rs` and `watch_cli.rs`. `docs/status-log.md`'s M41 section is the full account; this
records each §6 item against its measured figure, each prediction confirmed or corrected, and what
was found without being sought. Every figure below traces to a ledger log or to the status log.
Amended by the final review's fix wave (R18, after `91d19a4`): one defect fixed, a sixth oracle
arming, and the `crates/retrace` chunks re-run, **666 / 0 / 9 over 141**.

**§6, item by item.**

- **Every Task 1 test RED on `c68ba6d` and green after, measured both ways.** RED was measured on
  `174cbbd`, whose debugger is `c68ba6d`'s: Task 1 added `Armed`/`step_armed` (used only by the
  oracle) and test code, and `debug.rs` has no Task 1 hunk. After R12 and R13 all **16** failed for
  their t0 reason (ledger `t1-red2.log`: the invariant test `left: 0, right: 1`, `m1` showing
  `resolved (1, 14)`, `m2` exit 5, the three `m8` tests exit 5, each oracle arming failing in its
  chain after its self-check passed; Task 1 was purely additive, 660 insertions and no deletion).
  Green: 5 at Task 2, 6 at Task 3, the last 5 at Task 4; `hitorder_e2e` 21 / 21 at Task 4 and
  21 / 21 in the gate (`finished in 46.30s`), then 22 / 22 in the fix wave's re-run
  (`finished in 47.11s`). The five Review Focus tests the plan added were not
  all red by design: `rf1` was (exit 5, R10's plan-time reading), `rf2`–`rf5` pin behaviour the
  rewrite had to keep and were green before and after.
- **All five oracle armings pass all three checks; `threadrust` within budget or in its fallback.**
  All five pass, forward, backward and zig-zag. `threadrust` is **both**: it starts at the declared
  fallback landmark (`bsdthread_create`'s, `n_create`), and it costs **16.20 s user + 0.27 s sys**
  green (Task 2), against the 120 s budget. The fallback was taken for a reason §5 did not foresee
  (R13, below), not for the budget. A **sixth** arming, added by the final review (R18,
  `oracle_watchsweep_hits_in_the_exit_window`), passes all three as well; it was red on `91d19a4`'s
  debugger (below).
- **The gate green, reconciled file by file against 640 / 0 / 9 over 140.** **665 passed /
  0 failed / 9 ignored over 141 test binaries**, 77 exit files all `0`, clippy clean — predicted
  from source before the run and matched chunk by chunk (173 / 292 / 183 + 9 / 17 over
  26 / 41 / 73 / 1). Three files moved: `retrace-core/tests/replay.rs` +1, `debug.rs` +3,
  `hitorder_e2e.rs` +21 (new binary); the status log carries the chunk table and the per-file
  reconciliation. `TRACE_MAGIC` unmoved
  (`crates/retrace-trace` has no diff), `self.verify_thread(` **7 → 7**, no dispatch arm changed (the
  two `retrace-core` hunks inside `ReplaySession::advance`'s arms are comment-only, Task 2's R15),
  and no `#[ignore]` added or removed (9 → 9). **After the fix wave: 666 / 0 / 9 over 141** —
  the fix touched `crates/retrace` only (no diff in the other seven crates since `91d19a4`), so
  `ws` (173 / 26) and `box` (292 / 41) were reused and the rest re-run: `e2e` 184 + 9 over 73,
  `bins` 17, clippy clean, 75 exit files all `0`; `hitorder_e2e` +22 instead of +21, so +26
  attributes against M40.

**Predictions, confirmed or corrected.**

- **§9's ≈ 663 / 0 / 9 over 141 — corrected to 665 / 0 / 9 over 141.** §9's figure is 640 + ~9
  named regressions + 1 invariant + 5 oracle armings + 5 self-checks + ~2 `debug.rs` unit tests + ~1
  `step_armed` test = 663. What landed: **10** named regressions, not ~9 (M8 is three tests:
  forward, backward, and the child's first instruction), so +1; the five self-checks are asserts
  **inside** each oracle test, not tests of their own, so −5; the plan's five Review Focus tests
  (`rf1`–`rf5`), which §9 did not foresee, +5; and R17's `a_zero_count_step_is_not_an_arrival`, +1.
  663 + 1 − 5 + 5 + 1 = **665**. The plan's own Task 5 figure, 664, is this minus R17's test. The
  binary count, 141, was right. After R18's sixth arming, **666**: §9 short by three, the plan by
  two.
- **§3a: `blockedctx`'s two assertions should hold** — confirmed, 2 / 2 under approach A (Task 2).
- **§3a: no existing debugger test parks at a blocking boundary** — confirmed by the audit: no
  pre-existing assertion moved.
- **§3a's visible change** (at a blocking boundary `where`, `threads` and `regs` show the incoming
  thread) — confirmed, and pinned by
  `at_a_blocking_boundary_the_position_shows_the_thread_that_runs_next`.
- **§3e: the oracle is independent of §3a** — confirmed as a measurement, not only an argument: at
  Task 1, before the settle existed, `oracle_threadrust`'s self-check already returned the child's
  first instruction on thread 1.
- **§3e / §5: `step_armed`'s premise** — confirmed by its own test at Task 1.
- **§4: every arming RED on `c68ba6d`** (the oracle's positive control) — confirmed, all five, each
  in its chain rather than in its self-check.
- **§4's audit: no moved assertion (R5's case is pinned by no existing test)** — confirmed. The
  audit's list also gained `crashy_e2e` and `checkpoint_seek` (the plan's) and `symbols_e2e` and
  `symbolops_e2e` (a grep at the close); every one passes unchanged.
- **§3c's last paragraph: a fault during the finish propagates as today's error** — superseded by
  the plan's R10 before any code: the fault is crossed like a trap, and `rf1` pins it.
- **§5: the `threadrust` oracle's cost against ≤ 120 s** — the budget held (8.56 s user red, 16.20 s
  user green, both from `n_create`), but the premise under it did not: from landmark 1 the
  recording could not be stepped at all (next item).

**Found, not sought.**

- **The exit terminal parked before its own window (the final review's Important #1, R18).**
  `park_at_terminal` reseeked an exit to `(E, 0)`, but `ReplaySession::advance`'s `Exit` arm does
  not bump `idx`, so `E` is the exit's own window and every hit in it was still ahead: on
  `watchsweep`, `break` at `(2, 1)` then `continue; continue; reverse-continue` gave `no earlier
  hit`, and every further `continue` reported `(2, 1)` again. Pre-existing (M3/M6); the crash and
  signal terminals already parked at `(C, K_f)`. **No oracle arming had a hit in the terminal
  window**, which is why the oracle could not see it: every oracle fixture should arm one. The exit
  now parks at `(E, K_f)`, on the exit `svc`.
- **A terminal is after every hit, not an arrival (found while fixing R18).** Parked at `(E, K_f)`
  with §3b's arrival phase `Bp`, a breakpoint ON the terminal instruction, which `continue`
  reports, was lost backward; the crash terminal already had that hole (`crashy`, a `break` on the
  faulting pc: `reverse-continue` from the crash said `no earlier hit`). Every terminal now parks
  at phase `Watch`, which corrects §3b's listing of "a terminal park" among the arrivals. The sixth
  arming breaks on the exit window's `(2, 1)` and on the exit `svc` `(2, 2)`, and was shown red
  twice: on `91d19a4`'s debugger (backward answer #1 `(1, 328, Bp)` for `(2, 2, Bp)`) and with
  only the exit park moved (`(2, 1, Bp)` for `(2, 2, Bp)`).

- **The LL/SC exclusive-monitor limit (R13, R14).** Any VM exit between an `ldxr` and its paired
  `stxr`, a single-step or a hardware breakpoint stop, fails the store-exclusive. On `threadrust`
  that is Libsyscall's `getpid` pid cache, and stepping through it made the replay issue a
  syscall the recording does not hold, a loud divergence six landmarks later. The debugger's own
  `seek` is exposed, not only the oracle. It lies outside hit accounting, so under §7's halt rule
  it became a README Known limit and the first owed item rather than widening M41. That a retry
  loop would livelock under single-step is inferred, not measured.
- **Two plan defects caught in review (R17).** I1: the `stepi`-arrival unit test could not fail,
  because `Exec::new` already parks at `Bp`; it now arrives from a Watch park and was shown red with
  each reset deleted. I2: `stepi 0` / `reverse-stepi 0` reset a Watch park without moving, so the
  store was reported twice; the cursor now resets only on a move, and
  `a_zero_count_step_is_not_an_arrival` was red before the fix.
- **Two more plan defects of one class (R12, R16):** test code that held two live VMs at once
  (`HV_BUSY`), in `discover_ws`/`discover_fio` and in the arrival test.
- **`position()`'s doc overclaimed** (Task 2 review, fixed by R15): a caught `SignalDelivery` breaks
  `pc() == position()` at its `(n, 0)` too.
- **F2**, at pre-flight: the scan's scoped-out `WatchSyscall` arm lacks the boundary-breakpoint
  check the `Event` arm has. Pre-existing, preserved, owed. The final review judged it plausibly
  closed by R11 (the hardware fires at a window-entry pc under `run()`, per `oracle_fileio`'s
  backward chain); the arm itself stays unverified.
- **Two latent edges** (Task 3 review observations), both owed: a crashing store that also writes a
  watched range exits 5 through the crossing's `Advance::Watch` Err, as it did before M41; and an
  early `?` exit in `cmd_continue` can leave a kept session armed, harmless while an Err aborts the
  script and live once the lldb seam keeps an `Exec` after an error.
- **The successor order changed.** §1 and §7 name lldb as M42. On 2026-09-24 the operator ordered
  M42 = the LL/SC single-step limit (R14) and M43 = the lldb seam, so the seam is built on sound
  stepping.
