# retrace M15-threaddebug — the debugger learns to see the threads it already records

M14 taught retrace to *record* a threaded guest. It did not teach the debugger to *read* one. Every
feature that makes retrace a reverse debugger rather than a recorder — M3's reverse execution, M4's
checkpointed seek, M5's watchpoints — predates threads and is blind to them. The result is a tool that
can replay a two-threaded program bit-for-bit and cannot tell you which thread wrote the byte you are
asking about.

M15 closes that gap. It is a debugger milestone, not a scheduler milestone: the schedule M14 built is
correct and stays untouched.

## The problem, precisely

**The thread is a derived property of position, and nothing derives it.**

A position is `P = (N, K)`. `N` is a plain 0-based index into the flat `events: Vec<Event>` the trace
is read into (`ReplaySession.idx` *is* `N` — `crates/retrace-core/src/lib.rs:797-798`; `open()` sets
`idx: 1` because `events[0]` is the leading snapshot, `:844-845`; every consumed event bumps it by
exactly one in `finish_event`, `:884-890`). `K` counts guest instructions retired by `Box_::step()`,
tallied by the caller loop in `step_insns` (`:1583-1605`) and `window_len_here` (`:1611-1627`).

Neither coordinate names a thread, and **neither needs to** — the running thread is fully determined
by `(N, K)`, because a switch happens only at a clean stop boundary between windows, never mid-window
(`Box_::run()`'s own comment, `crates/retrace-box/src/lib.rs:2155-2169`). That invariant is why
`window_lens` can be keyed on `N` alone (`crates/retrace-core/src/lib.rs:1678`, `:1703-1711`) and
`CheckpointCache` on `(N, K)` (`:1723-1727`) and still be correct on a threaded trace.

So the thread identity exists, is deterministic, and is recomputed identically on every replay. It is
simply **never asked for**. `ReplaySession` has no accessor exposing `Box_::threads()` at all, and
`crates/retrace/src/debug.rs` contains **zero** occurrences of the word "thread": no thread listing,
no per-thread registers, no provenance on a watch hit. `regs`, `where` and `x` silently report
whichever thread the scheduler happened to have current.

This is the shape of the milestone: **expose a fact the box already computes**, rather than compute a
new one.

## Measured, on this host, at `main` = `26c881b`, 2026-08-15

Everything in this section was read out of the tree, not assumed.

### The debugger cannot see the thread table

- `ReplaySession` exposes `advance`, `advance_to_landmark`, `step_insns`, `window_len_here`,
  `peek_syscall`, `position`, `pc`, `landmark`, `arm_breakpoints`/`clear_breakpoints`,
  `arm_watchpoints`/`clear_watchpoints`, `far`, `dbg_regs`, `dbg_fp_regs`, `read_mem`, `snapshot`,
  `diff_memory`, `checkpoint`/`from_checkpoint`. **None takes or returns a thread id.**
- `Box_::threads()` exists (`crates/retrace-box/src/lib.rs:3030`) and is never called from
  `retrace-core`.
- `grep -c thread crates/retrace/src/debug.rs` → **0**.

### `BoxState` already carries every thread, including blocked ones

`checkpoint()` clones the entire `ThreadTable` and folds the live vCPU into the current thread's slot
first (`crates/retrace-box/src/lib.rs:3442-3485`), because the table's copy of the running thread is
stale between switches. `from_checkpoint()` restores `TPIDRRO_EL0` from the restored table's current
thread rather than a constant (`:3515-3516`).

**Consequence M15 exploits:** a blocked thread's full register context is already present at every
checkpoint. Dumping it requires running nothing.

### Debug registers are vCPU-global and survive a context switch

`ThreadCtx` (`crates/retrace-box/src/thread.rs:24-32`) carries exactly `regs`, `fp`, `fpcr`, `fpsr`,
`tpidrro_el0`, `elr`, `spsr`. `save_ctx`/`load_ctx` (`crates/retrace-box/src/lib.rs:2991-3026`) touch
those and nothing else, and `switch_to_thread` (`:3349-3359`) calls only that pair. So
`DBGWVR/DBGWCR`, `DBGBVR/DBGBCR` and `MDSCR_EL1` are outside the scheduler's save/restore discipline
entirely.

**This is a leak, not a loss, and the leak is what we want.** All cooperative threads share one
physical vCPU and one address space, so an armed watchpoint keeps firing across switches and
correctly catches *any* thread's store to the watched address. The mechanism is right; what is missing
is (i) provenance — which thread stored — and (ii) optional scoping to one thread.

**It is also entirely untested.** Every M5 test predates M14 and uses a single-threaded guest; no test
anywhere exercises an armed `DBGW` across a `switch_to_thread`.

### Watchpoints have two detection paths, and both are thread-blind

- **Hardware:** `arm_hw_watchpoint` (`crates/retrace-box/src/lib.rs:2364`) programs one of four
  `DBGWVR/DBGWCR` slots and sets `wps_armed`; `sync_mde` (`:2392`) sets `MDSCR_EL1.MDE`. An EL0 store
  traps pre-retire with EC `0x34`/`0x35` (`crates/retrace-arch/src/lib.rs:2,156`), surfaced as
  `Stop::Other{esr}` and turned into `Advance::Watch` (`crates/retrace-core/src/lib.rs:1469-1473`).
  Evidence carried: ESR and FAR. **No thread.**
- **Software:** inside `apply_and_return` (`crates/retrace-box/src/lib.rs:2686-2704`), because a
  kernel-performed write (a syscall out-param such as `fstat`'s statbuf) executes no guest store and
  can never trap `DBGW`. Sets `syscall_watch_hit`, consumed by `finish_event`
  (`crates/retrace-core/src/lib.rs:884-888`) as `Advance::WatchSyscall`. Evidence carried:
  `Event::Syscall`, which has **no thread field**.

### `reverse-continue` re-scans from the beginning

`cmd_reverse_continue` (`crates/retrace/src/debug.rs:474-541`) cannot run backwards, so it scans
*forward from the start of the recording* on a fresh `ReplaySession`, re-arming breakpoints and
watches each iteration, recording every hit strictly before the current position and keeping the last
one. It matches on both `Advance::Watch` and `Advance::WatchSyscall` (`:497-502`).

**Consequence:** thread provenance must be reconstructed *during* that scan by asking the live session
which thread is current. There is nothing in the trace to read it off — which is exactly what §
"M15-tag" changes for the oracle, and exactly what it does *not* need to change for the debugger.

### `Event::Sched` is reserved and unused, and using it would renumber every landmark

`Event::Sched { thread: u32, until: u64 }` (`crates/retrace-trace/src/lib.rs:17`) has zero producers
and zero consumers tree-wide; the meaning of `until` is undocumented.

Emitting it was considered and **rejected**, for two measured reasons:

1. **It would silently renumber landmarks.** `N` is a flat `Vec` index, so interleaving `Sched` events
   shifts every subsequent landmark by one per switch. This is *not* a `TRACE_MAGIC` break — the
   variant already exists under the current magic, so old traces still parse and simply mean something
   different. Landmark numbers are treated as stable identity (checkpoints are cached by them,
   `advance_to_landmark` is a public seek target), so a silent renumbering is worse than a loud break.
2. **Nothing in the dispatch loops can see a switch.** `run()`'s reschedule check sits inside `Box_`
   (`crates/retrace-box/src/lib.rs:2170-2172`), below the trace. Emitting `Sched` would require either
   a new channel for `run()` to surface switches to its caller, or duplicating the scheduling decision
   into both dispatch loops — the exact duplication symmetry rule 2 exists to prevent.

## The mechanism

### M15-expose — the session can be asked which thread is running

`ReplaySession` gains `current_thread() -> u32` and a read-only view of the thread table (id, state,
and enough of each `ThreadCtx` to render registers). Both read what `Box_` already holds. **No trace
change, no behaviour change, no new determinism argument** — the schedule is already recomputed
identically on both runs, which is M14's own posture doing the work.

This is the foundation; every other part depends on it.

### M15-tag — the oracle gets a thread to compare

`Event::Syscall` gains `thread: u32`, and `TRACE_MAGIC` is bumped.

**Why `Syscall` alone is complete.** The schedule can change only when a thread blocks or exits, and
both are syscalls (`__ulock_wait` 515, `bsdthread_terminate` 361). Tagging syscalls therefore records
the whole schedule; every other landmark's thread is "the thread of the most recent syscall landmark."
Tagging more variants would add redundancy, not information.

Record writes `current_thread()`; replay recomputes it and compares, exactly like every other field
the oracle checks. This closes the gap M14's Status section bills as its sharpest limit: the oracle
has no thread identity, so **two threads running the same code can issue byte-identical `(num, args)`
and a wrong-thread replay continues in silence.**

### M15-watch — a hit says who did it

`Advance::Watch` and `Advance::WatchSyscall` carry the thread that produced the write, read from
`current_thread()` at the hit. `reverse-continue` reports it. Optional per-thread scoping is a
**debugger-side filter** — the hardware keeps watching globally because one vCPU underlies every
thread, and the debugger discards hits whose thread does not match.

### M15-cli — a thread vocabulary

- `threads` — list every thread with its state, marking the current one.
- `where` / `regs` — labelled with the owning thread.
- `regs <thread>` — dump a **blocked** thread's registers, read straight out of the thread table.
  Impossible today, and free given `BoxState` already carries every context.

## Determinism posture

**Unchanged, and deliberately so.** M15 adds no scheduling decision and no new source of state. The
thread identity it exposes is the same pure function of the guest's syscall sequence that M14
established; M15 merely reads it. The one trace change (`M15-tag`) is a *recording of* that function's
output so the oracle can check it — the standard symmetric posture, where replay recomputes and
compares.

`TRACE_MAGIC` is bumped once, loudly: `RT\x00\x06` → `RT\x00\x07`. Existing recordings become
unreadable, which is correct — their syscall events genuinely lack a field the new reader requires —
and the rejection is already clean rather than a misparse: `open_checked` returns
`Ok((Vec::new(), true))` on a magic mismatch, "reject loudly, keep nothing"
(`crates/retrace-trace/src/lib.rs:81-82`). The repo's ritual for this is a test named for the reason,
currently `magic_bumped_for_the_signal_delivery_variant` (`:271`) plus
`a_trace_written_with_the_old_magic_is_rejected_whole` (`:280`); M15 updates the former to name its
own reason and keeps the latter green against the new prior magic.

## Fail-loud boundaries

- **A thread id that does not exist in the table** — a tag naming an unknown thread is a corrupt
  trace, and asserts rather than rendering "thread 7" for a two-thread run.
- **`regs <thread>` for an out-of-range thread** — usage error, not a panic, matching
  `debug_arg_errors_are_usage_not_panics`.
- **A recorded thread tag that disagrees with the recomputed one** — a `Divergence`, not a warning.
  This is the whole point of M15-tag.
- **Position→thread ambiguity** — if a switch is ever observed mid-window, the invariant this design
  rests on is broken and it must assert rather than report a thread that is merely probable.

## Scope

**In:** exposing thread identity on `ReplaySession`; the `Syscall` thread tag and its oracle check;
thread provenance on both watch paths; optional per-thread watch scoping; the `threads` / `regs
<thread>` / labelled `where` CLI; and a test that an armed watchpoint survives a context switch.

**Out, deliberately:**

- **Per-thread reverse execution as a distinct position space.** P stays `(N, K)`. "Rewind thread B"
  is not a coordinate change; it is a search over positions where B is current, and it is out of scope
  until someone needs it.
- **Preemption.** Cooperative scheduling is unchanged; a spin-waiting guest still hangs.
- **`workq`/GCD**, thread priority, and per-thread signal masks — all still unmodelled.
- **Scoping a watchpoint in hardware.** Filtering happens in the debugger; the `DBGW` slot stays
  global.

## Exit criterion

A threaded guest in which **thread A writes a watched address and thread B does not**.
`reverse-continue` finds the write **and names thread A**.

The naming is the assertion. A gate that only checked "the watch fired" would pass with provenance
entirely wrong, since the watch already fires correctly today — the milestone's whole contribution is
the attribution, so that is what the gate must be able to fail on. Paired with a second assertion that
`regs <thread>` dumps a **blocked** thread's registers, which no version of retrace can do today.

## Testing

Cheapest first, so the expensive gates run against measured ground.

1. **Unit** — `current_thread()` tracks `ThreadTable::current()` across a switch; the thread-table
   view renders a blocked thread. No VM beyond the existing `tb()` box.
2. **Box-level** — an armed `DBGW` survives `switch_to_thread` and still fires (the hazard that is
   correct-by-accident and untested today). Assert the *hardware* side, not only the bookkeeping —
   M13's Task 8 defect was a test that checked only the software mirror.
3. **Oracle** — a recorded thread tag that is deliberately corrupted produces a `Divergence`. Prove it
   by mutation, not by reading.
4. **CLI** — `threads`, `regs <thread>`, labelled `where`, and their usage errors.
5. **Headline** — the exit criterion above.

Every test must be mutation-checked against the defect it names. M14 shipped **six** tests that could
not fail for the property in their own name; that is this project's most reliable failure mode and the
plan must budget for catching it rather than hoping.

## Risk register

- **R1 — the position→thread mapping rests on an invariant that is documented but unenforced.**
  Switches happen only at clean stop boundaries (`crates/retrace-box/src/lib.rs:2155-2169`). Every
  claim M15 makes about "the thread at position P" is false if that ever stops holding. Mitigation:
  assert it rather than assume it, and make the assertion part of M15-expose rather than a comment.
- **R2 — the `TRACE_MAGIC` bump invalidates every recording on disk**, including any a developer is
  mid-investigation on. Unavoidable and correct, but it must be called out in the Status section
  rather than discovered.
- **R3 — `reverse-continue` re-scans from the recording's start on a fresh session each iteration.**
  Thread tracking must not perturb that determinism; `reverse_debug_transcript_is_deterministic` is
  the existing guard and must stay green.
- **R4 — adding a field to `Event::Syscall` touches the hottest struct in the format.** Every
  construction site and every match must move together. A missed site is a *compile* error and so is
  self-announcing; the dangerous case is the site that compiles while writing a **stale or defaulted**
  thread id. Mitigation: on the record side the tag must be sourced from a single call to
  `current_thread()` at the point the event is appended — never passed down through a helper that
  might hold an older value, and never defaulted to 0. `record_box` has multiple arms that append a
  `Syscall` event (the generic forward arm plus every special-cased handler), so "one place" means one
  *source of the value*, not one call site, and the plan must enumerate the arms.
- **R5 — the debugger's thread view could disagree with the box's.** The view must be a read of
  `Box_`'s live table, never a copy the debugger maintains in parallel, or the two drift and the
  debugger lies with confidence.

## Components

- `crates/retrace-core/src/lib.rs` — `ReplaySession::current_thread`, the thread-table view, the
  oracle's thread comparison, thread on both `Advance::Watch*` variants.
- `crates/retrace-trace/src/lib.rs` — `Event::Syscall { thread }`, `TRACE_MAGIC` bump.
- `crates/retrace-box/src/lib.rs` — whatever accessor the view needs; the invariant assertion (R1).
- `crates/retrace/src/debug.rs` — `threads`, `regs <thread>`, labelled `where`, provenance in
  `reverse-continue` output.
- `crates/retrace-guest/rs/` + `crates/retrace/tests/` — the headline guest and gate.

## Open questions for implementation planning

1. **What does the thread-table view expose?** The full `ThreadCtx` per thread, or a summary plus an
   on-demand register fetch? The former is simpler; the latter avoids cloning 32 Q-registers per
   thread on every `threads` command.
2. **Does `Event::Sched` stay in the format?** It is now definitively unused and its `until` field is
   undocumented. Leaving a reserved variant that this milestone explicitly declined to use invites the
   next reader to assume it is live. Removing it is a format change we are already making.
3. **Should the headline guest's two threads be distinguishable by construction?** A child that writes
   and a main that does not is the clearest possible provenance assertion, but it means the gate proves
   attribution only in the easy direction. Worth deciding whether a second write from the other thread
   earns its complexity.
