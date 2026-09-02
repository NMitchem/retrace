# M24-restoreaudit design

**Status:** t1 landed (`81d32ef`), spec written after the fact — see "Why this spec is retroactive".

## The class, precisely

`Box_` has three construction paths, and only one of them runs on the record side:

| Path | Runs on | Builds from | Sets up |
|---|---|---|---|
| `load` / `load_dynamic` | **record only** | the Mach-O + dyld | everything, from scratch |
| `restore` | **replay only** | a landmark-0 `Event::Snapshot` | re-derives from memory |
| `from_checkpoint` | **replay only** (M4 seeks) | a mid-run `BoxState` | restores carried state |

Anything a load path establishes that a replay path does not re-establish is an asymmetry whose
signature is **a passing record followed by a diverging replay**. That signature is invisible to
every record-side test, and — this is the part that makes it a class rather than a bug — it is also
invisible to the determinism oracle whenever *both* replay paths get it wrong in the same way,
because the oracle only ever compares replay against record's *trace*, never against record's *box*.

The class is not hypothetical and it is not new. It has shipped, by our own written record, at least
seven times:

- **M9 t3** — `from_checkpoint` reset a flag the restored state contradicted (the shape the
  `BoxState` comments now name explicitly).
- **M10** — fd slots not carried; a seeked session believed every fd was Free, so a post-seek
  `pread` returned `EBADF`.
- **M11** — `sigtable` not carried; a seek into a run that installed a disposition restored a box
  that had forgotten it, so an *ignored* signal would terminate the guest.
- **M14** — `thread_start_pc` not carried; `bsdthread_create` on a restored session hit the
  fail-loud path an unregistered guest hits.
- **M18** — `wq_thread_pc`, same reason.
- **M21** — the believed-stack reservation was made in `load_dynamic` only. Replay's first
  stack-growth fault went unserviced and reported as a divergence. M21 was **record-only** until
  t2.5 fixed it.
- **M23 t1** — the EL1 vector table is built in both load paths and never in `restore`. It reached
  replay only because the trampoline happens to be a snapshot backing: correct by luck, pinned by
  nothing. Left open as review finding **F5**.

The `BoxState` field comments are themselves a written log of this class — one of them literally
reads *"the fifth field in this struct to exist for that reason."* Seven instances across fifteen
milestones, each fixed individually, none of them leaving behind a mechanism that would catch the
eighth. **That absence is what M24 exists to fix.** Fixing the instances is the smaller half.

## Why this spec is retroactive

t1 was implemented before this document existed, which is a deviation from the SDD flow CLAUDE.md
describes and is recorded here rather than hidden. The audit found its first four asymmetries by
following M21's and M23's scent, not by systematic enumeration. This spec therefore does two jobs:
it records what t1 did, and it states — in "Coverage: what is actually pinned" below — **what the
audit does and does not cover**, because an audit milestone that does not publish its negative space
is indistinguishable from four ad-hoc fixes wearing a milestone's name.

## What t1 found and fixed

Four asymmetries, three in `restore` and one on the replay dispatch side.

**G1 — `TPIDRRO_EL0` set unconditionally.** `restore` set it to `TSD_IPA` under a comment claiming to
"match load". True of `load_dynamic`; false of the static load, which never sets it (a fresh vCPU
leaves it 0) and does not map `TSD_IPA` at all — so a static guest's deref would have faulted on
**replay only**. Now gated on `stack_top == DYN_STACK_TOP`.

Corroboration worth recording: `from_checkpoint` already did this correctly, taking the value
per-thread from the captured table (`state.threads.ctx_of(cur_tid).tpidrro_el0`) with a comment
explaining that a constant here is wrong. G1 brings `restore` into line with a sibling path that had
been right since M14 — independent evidence that the constant was a genuine defect and not a
harmless simplification.

**L1 — the vector table (M23's F5).** `restore` now asserts the snapshot carries the table this build
makes. Side effect: it converts M23's **F4** (un-bumped `TRACE_MAGIC` despite t1 changing snapshot
content) from a silent wrong replay into a loud refusal. A pre-M23 recording carries `UDF #0`
padding, which the current code executes at EL1, destroying `ESR_EL1` and reproducing the
`pc=0x4204` misattribution M23 removed. **This mitigates F4; it does not close it** — see t3.

**L2 — thread 0's saved context.** `load_dynamic` folds real startup state into it; `restore` left it
`ThreadCtx::zeroed()`. Gated like G1, and the gate is load-bearing: seeding unconditionally traded
the asymmetry for its mirror image, since the static load does not populate thread 0 either. The
parity test caught that over-correction immediately, which is the argument for the test.

**G2 — stranded signals on replay.** `ReplaySession::advance`'s terminal-exit arm now calls
`assert_no_stranded_signals()`, mirroring the guard record already had. Replay can strand a signal
record did not: a seek or `from_checkpoint` can land *past* the `__ulock_wake` a pended signal was
waiting to materialise at. A vanished signal is the one class the divergence oracle structurally
cannot see, because both sides agree — so it has to be caught by a guard, and the guard has to exist
on both sides.

## The mechanism: `tests/restoreparity.rs`

The part meant to outlive the instances. It diffs a load box against a `restore` box built from that
same box's own snapshot, field by field, and states an obligation for future work: a new `Box_` field
or load-time write must be **either** covered there and equal, **or** named in `normalise()` with the
mirrored replay mechanism that re-establishes it, cited by file and line. There is no third option
that is safe.

`normalise()` currently holds exactly one entry — the shared-cache pager, which `load_dynamic`
installs eagerly and which replay installs through the mirrored `#294`/`#536` dispatch arms. That is
a real mirror, not an excuse, and the entry names it.

## Coverage: what is actually pinned

`Box_` carries 27 state fields (excluding the `vm`/`vcpu` handles). The parity guard compares **13**:
nine through `dbg_internal_state()` (`reservations`, `mmap_next`, `bootstrap_port`, `cache.is_some()`,
`last_far`, `synthetic_tsc`, `cache_refault_ipa`, `cache_refault_count`, `pac_enabled`) and four
through explicit assertions (`stack_top`, `stack_size`, `fall_throughs`, and `threads` — the last
only as `ctx_of(0).regs.pc`). It also compares two sysregs and the 0x800 vector-table bytes, neither
of which is a struct field.

**The 14 it does not compare**, and the honest reason each is currently safe:

- `noaccess`, `bps_armed`, `wps_armed`, `watch_ranges`, `syscall_watch_hit`, `tlbi_stub_ready`,
  `fds`, `sigtable`, `thread_start_pc`, `wq_thread_pc`, `pthread_size` — **default on both sides at
  landmark 0.** Symmetric today by coincidence of both paths choosing the same default, not by any
  mechanism. If a load path ever starts setting one of these before the first landmark, the guard
  will not notice.
- `backings` — restore builds them from `mem`, load from the Mach-O. Expected equal; **untested.**
- `next_l3`, `l2_host` — derived from `backings` on both paths. Expected equal; **untested.**
- `threads` beyond thread 0's `pc` — not the rest of `ThreadCtx`, not per-thread `tpidrro_el0`, not
  the count, not masks/pending/altstack.

**And the largest gap: `from_checkpoint` has no parity guard at all.** It is the path with the
*documented five-instance history* of this exact class (M9 t3, M10, M11, M14, M18 — the `BoxState`
comments enumerate them by name), it restores far more state than `restore` does, and it runs
mid-run where nothing is at a default. M24 as landed closes the class on the path it has bitten
*twice* and leaves it open on the path it has bitten *five times*. Stating that plainly is the point
of this section.

## Scope

**In:** the four fixes above; the parity guard for `load`↔`restore`; deepening that guard where it is
cheap (t2); closing F4 properly (t3); the two documents (t4).

**Out, and named rather than silently skipped:** a `from_checkpoint` parity guard. It needs a
different fixture — a box driven to a mid-run landmark, checkpointed, restored, and diffed — which is
a milestone's worth of work, not a task's, because "equal at a mid-run landmark" requires deciding
what *should* legitimately differ there. It is written up as the successor milestone in "Residual",
not left to be rediscovered.

## Exit criterion

The chunked gate green with the ignored count unchanged at 2; each new test demonstrated able to
fail; F4 closed as a format break rather than an assert; README "Known limits" and
`docs/status-log.md` both updated. No headline gate is un-parked by this milestone and none is newly
parked — M24 buys a guarantee, not a capability.

## Residual, stated up front

1. **`from_checkpoint` parity is untested** (above). The successor milestone.
2. **Symmetric-but-wrong is still invisible.** A static box's thread 0 context is zeroed on *both*
   sides; a consumer reading `ctx_of(current)` without refreshing gets zeros identically on record
   and replay. Wrong in the same way twice is the oracle's blind spot by construction, and no parity
   test between two replay paths can see it either.
3. **Landmark 0 only.** The guard compares construction, not evolution. Two boxes that agree at
   landmark 0 and drift apart later are outside what this pins.

## Risk register

| # | Risk | Mitigation |
|---|---|---|
| R1 | The parity guard passes vacuously | Every test mutation-verified: revert the fix, confirm red, with the assertion naming the divergent values. Done for G1/L2/L1 before the gate. |
| R2 | `normalise()` becomes a dumping ground for inconvenient differences | The obligation text requires each entry to cite the mirroring mechanism by file and line. One entry today. Review any growth. |
| R3 | The gate is read as proving more than it does | This spec's "Coverage" section is the counterweight; the README edit must not claim the class is closed, only that the `restore` path is guarded. |
