# M32–M38 program charter — an unattended soundness-then-breadth run

**Date:** 2026-09-09
**Status:** design, approved in conversation before writing
**Base:** local `main` with M31-checkpointparity merged (conditional — see §2)

This document is a **program charter**, not a milestone design. It governs a queue of milestones to
be executed largely by subagents without step-by-step human guidance. Every milestone in the queue
still gets its own spec and plan, written to the normal SDD standard (§9); this charter says *which*
milestones, *in what order*, *under what authority*, and — for the breadth phase — *by what
pre-committed decision rule*, so that an agent working unattended fills in a template rather than
inventing scope.

## 1. Why this shape

The selector for unattended work is **plan certainty**, not value. This repo's own history is the
argument: M15 ("the plan was wrong six ways and only measurement caught them"), M20 ("three
milestones of right conclusion, unmeasured supporting fact"), M25 ("its replay diagnosis was WRONG
three ways"), M11 ("measurement caught five plan defects"). A subagent that hits a wall its plan
mislocated cannot re-plan. It thrashes, or — worse — produces something that gates green while
being wrong.

So the queue front-loads work whose walls are **already located and named** by a prior milestone's
owed-list, and converts the one genuinely uncertain body of work (the Apple-binary sweep failures)
from *diagnosis* into *measurement* before any fix is attempted.

Two orderings follow, and both matter:

- **Soundness before breadth.** M26 recorded that `/bin/ps` was a truncation bug *misfiled* as an
  Apple-binary failure. The soundness phase (M32–M35) therefore plausibly retires some of the eight
  sweep failures as a side effect. Measuring the eight *before* that work would produce a table
  already stale by the time it was used.
- **Measurement before breadth fixes.** M36 produces evidence, and the parked gates that evidence
  owes — but no fixes. Its output is the input to M37–M38, so those milestones are specced by data
  rather than by an agent's guess.

## 2. Precondition: the base commit

M31-checkpointparity sits on an unmerged branch (`m31-checkpointparity`, 12 commits, 0 behind
`main`). The run's base is that branch merged into local `main`, and the merge is conditional on an
**independent** re-verification of its gate claim — not on the branch's own self-reported ledger:

1. the chunked workspace gate green, with cargo's exit code captured **before any pipe**, including
   the `--bins` chunk and any split library crate's `--doc` chunk;
2. `#[test]` counts reconciled **file-by-file** against M30's close, not by trusting a sum;
3. at least one of M31's documented positive controls re-run, to confirm the new guard is wired up
   rather than vacuous.

Rationale for insisting on the independent re-run: M30 shipped three instruments that could not
fire, and M28 shipped `let band = 0;` through a 523-test gate. A self-reported green from the branch
that wrote the test is precisely the evidence this repo has learned not to trust.

## 3. The queue

Milestones run **strictly sequentially**. They are not independent: M32–M35 all edit the same small
set of tables in `retrace-arch` (`dest_buffer`, `reads_guest_buffer`, `fd_operands`), so fanning
them out would produce conflicting edits to one file. One-VM-per-process (HVF) independently caps
concurrent end-to-end testing. **This run is a pipeline, not a swarm**, and a plan that assumes
otherwise is wrong.

### M32 — `dirtable`: a per-argument direction table

**Discharges:** M30's owed-list, first entry — *"the largest single thing M30 gives up."*

M30's guard band is withheld from any syscall whose buffer the *reader* consumes (`write`, `send*`,
`sendfile`, `mach_msg2`, …), because a canary written into such a buffer reaches the kernel as data
— the M30 bug itself. That withholding is currently per-**syscall**, so a syscall with both an
in-buffer and an out-parameter loses destination-side coverage on the out-parameter too.
`sendfile`'s in-out `off_t *` and `mach_msg2`'s receive buffer are the named casualties.

Make direction a property of the **argument**, not the syscall, so a reader syscall's genuine
destination arguments regain canary coverage.

**Plan certainty: high.** The wall is named, the two casualties are named, and the mechanism
(`reads_guest_buffer` as a syscall-level predicate) is already in the tree.

**RE-SCOPED after measurement, 2026-09-09.** This entry originally scoped M32 to `sendfile` alone,
on the reasoning that `mach_msg2`'s overlapping send/receive buffer was too risky to touch. A
measurement taken while writing M32's spec inverted that: **no guest in this repo issues
`sendfile`** — syscall 337 appears only in `retrace-arch` comments and the `reads_guest_buffer`
list — while `mach_msg2` is issued constantly by every dynamic guest and directly by
`crates/retrace-guest/asm/machmsg.s`. Seeding from `sendfile` would have shipped a branch nothing
executes, which is the dead-channel trap this charter's own §1 warns about. M32 therefore builds the
mechanism and seeds it from a **measurement of `mach_msg2`'s send/receive boundary**, with
`sendfile` entered as table-only and labelled unexercised. See
`docs/superpowers/specs/2026-09-09-retrace-m32-dirtable-design.md` §2 and §4.

**The generalisable lesson, which binds M33–M38:** before seeding any table entry, confirm a guest
in this repo actually dispatches that syscall. The check is cheap and it has already paid once.

### M33 — `readerenum`: enumeration, and a loud failure

**Discharges:** M30's owed-list, second entry (`ioctl` and any unlisted reader syscall), and the
README's standing `fd_operands` complaint.

Two halves, both enumeration:

1. `reads_guest_buffer` is an allowlist, and "only enumeration prevents the class it guards" (M30).
   `ioctl` is named; sweep for the rest.
2. `fd_operands` **fails silently** on an unlisted syscall — the M10 class M25 hit again with
   `getdirentries64`. Silent, so it costs correctness without costing a test. Make it fail loud.

**Plan certainty: high.** Both halves are enumeration against a known interface.

**Expected side effect, not a regression:** half 2 will likely turn some currently-passing sweep
binaries into loud recorder panics. That is the fail-loud discipline working. M33 must therefore
re-run `tools/apple-sweep.sh` and re-baseline the tally, and **any binary that moves becomes a row
for M36's table, not a bug for M33 to fix.**

### M34 — `destgaps`: the three uncovered `dest_buffer` syscalls

**Discharges:** the README's "3 remain uncovered by `dest_buffer`" — `proc_info`,
`getattrlist`/`fgetattrlist`, `csops`.

Each needs its reply-length operand located and added, as M27 did for `ps`'s
`sysctl(KERN_PROC_ALL)` (`*(size_t*)x3`, 205,416 bytes).

**Plan certainty: high** for the mechanism, **medium** per operand — each location is a measurement.
The plan must make "measure the operand" an explicit task step with a recorded result, never an
assumption baked into an edit. This is the milestone most exposed to the "right conclusion,
unmeasured supporting fact" failure M20 named.

### M35 — `errholes`: the two holes M27 and M28 left

**Discharges:** M30's owed-list, sixth entry.

1. `Box_::diff_memory`'s `.min(avail)` clamp on the **replay** side.
2. The `if !err` gate that skips write capture — and band evaluation — on a **failing** syscall. The
   standing assumption is that a failed syscall writes nothing; M28 flagged it unmeasured.

**Plan certainty: medium.** Half 2 needs a guest that fails syscalls deliberately.

**CORRECTED 2026-09-09:** an earlier draft of this entry said that guest "does not exist and must be
written." It does exist — `crates/retrace-guest/asm/failsys.s` opens `/no/such/retrace/path` and
exits with the errno, and `failsysctl.s` is a second one. Whether either *reaches* the `if !err`
gate with a pointer argument worth banding is a measurement M35 still owes, so the certainty stays
medium; but the milestone starts from a fixture rather than from nothing, and it is no longer
obviously the queue's most likely halt point. It stays last in the soundness phase regardless, since
a halt there still banks M32–M34 as merged work.

### M36 — `sweepmeasure`: measurement, and the gates the README already owes

**Deliverable: a table, and a parked gate per measured wall. No fixes.** This milestone is forbidden
from changing *behaviour*; its production edits are documentation and `#[ignore]`d gates. A
behavioural change appearing in an M36 diff is a defect in the run, not a bonus.

**Why gates, when an earlier draft of this charter said measurement-only.** That draft was
under-scoped against the repo's own standard, and the README says so in its own voice. Of the four
`replay diverged` binaries it records that their cause *"stands unmeasured to this day, with **no
parked gate standing for it** — a gap in this repo's own discipline rather than a decision."* And
CLAUDE.md blesses paying that debt: *"A milestone that parks a **new** gate for a capability it does
not yet have has regressed nothing; that is the discipline working, not a backslide."* A table in a
status log is not a gate. M36 measuring these walls and then leaving them ungated would close the
excuse while leaving the gap.

**Four conditions on that authority** (referenced by §5's exception). A gate M36 parks must:

1. stand for a binary in the committed 54-entry corpus `tools/apple-sweep-binaries.txt`;
2. carry the **measured evidence** in its `#[ignore]` reason — never the sweep's category string,
   and never the inherited M23 `brk` belief (see the trap below);
3. never retire or `#[ignore]` a test that currently **passes**;
4. name what would un-park it, in the CLAUDE.md house style ("UN-IGNORE when that lands").

Anything outside those four is a §5 halt, unchanged.

Re-run `tools/apple-sweep.sh` on the post-M35 tree and produce one row per failing binary:

| column | meaning |
|---|---|
| `binary` | path as it appears in `tools/apple-sweep-binaries.txt` |
| `sweep_reason` | the string the sweep itself prints |
| `first_divergent_landmark` | landmark index, or `n/a` for a recorder panic / timeout |
| `trap` | syscall number / trap kind at that landmark |
| `root_cause_class` | one of the six in §6 (A, B, C, D, E1, E2) |
| `evidence` | path to a captured `RETRACE_TRACE=1` log or replay diff |

The known population as of M31 is eight: `csh` and `tcsh` (`recorder panicked` — the M10 fd table's
fail-loud unmodelled `dup2`); `automationmodetool`, `desdp`, `dyld_info`, `flex`, `/bin/launchctl`
(`replay diverged`); `/usr/bin/yes` (`timed out after 30s recording`). Plus `dddiagnose`, which is
intermittent and must get a row whether or not it fails on M36's run.

**One trap this milestone must not fall into.** The README already warns that *"what the sweep
reports is not why they fail"* — the four `replay diverged` binaries have been *believed* since M23
to reach a `brk`, and the sweep corroborates nothing about that. M36's `root_cause_class` must be
derived from evidence it captures, never from the sweep's category string or from the inherited
belief.

**Plan certainty: high by construction.** A measurement milestone cannot be wrong about a wall's
location, because locating the wall *is* the deliverable. This is the mechanism that makes the
breadth phase safe.

### M37–M38 — breadth fixes, routed by the table

Scope is **whatever M36's table routes to class B (§6)**, in the order the table gives.

- If class B is **empty**, the run ends at M36 and says so in the status log. M37/M38 are not
  invented to fill the slot.
- If class B is **small**, M37 takes it and M38 does not exist.
- If class B is **large**, M37 and M38 take two slices and the remainder is left named for a
  successor milestone.

Each still gets its own spec and plan (§9). Their specs are written *from the table*, and a row that
the table does not classify is a halt (§5), not an invitation.

## 4. Controller loop

Per milestone, in order:

1. Write the milestone spec → `docs/superpowers/specs/YYYY-MM-DD-retrace-m<NN>-<name>-design.md`
   and the plan → `docs/superpowers/plans/YYYY-MM-DD-retrace-m<NN>-<name>.md`, and commit **both to
   `main`** before branching. This matches the repo's existing convention rather than a new one:
   M31's three doc commits (`a063833`, `58415d0`, `23abc74`) sit on `main` and precede its
   implementation branch. An earlier draft of this charter had the spec written inside the milestone
   worktree; that was wrong and is corrected here.
2. Branch from local `main`: `git worktree add` a fresh worktree, branch `m<NN>-<name>`.
3. Execute tasks TDD, one subagent per task, reports under `.superpowers/sdd/`.
4. Code review the whole branch diff; fix rounds as needed.
5. **Full chunked gate** (§7) + `#[test]` reconciliation file-by-file against the previous close.
6. Edit `README.md` ("What works today" / "Known limits") **and** append a new section to
   `docs/status-log.md`. Both, never one — the README is edited in place, the log is append-only.
7. Merge to local `main`. **Do not push.**

## 5. Autonomy envelope

**Permitted unattended:** branch, commit, code review, fix rounds, run the gate, edit `README.md`
and `docs/status-log.md`, merge to local `main`.

**Also permitted, authorised by the operator 2026-09-09:** **writing each milestone's own spec and
plan** from its charter entry, and **running the full queue through M38** without an inter-milestone
check-in. Two obligations come with that authority and are not optional:

1. **Every autonomously-written spec still owes §9's contract in full**, and its measurement step
   runs *before* its first edit. M32's spec was re-scoped by a measurement taken while writing it
   (`sendfile` proved to be a dead channel); an autonomous spec gets no operator to catch that, so
   the measurement step is the only thing standing in its place.
2. **A re-scope is not a halt, but it is a loud ledger entry.** When a measurement contradicts the
   charter's premise for a milestone — the `sendfile` situation — record
   `Ruling: re-scoped <milestone> — <premise> was contradicted by <measurement> — <new scope>` and
   continue. The operator reads the ruling list at the end and reworks what they disagree with. Do
   not silently narrow scope, and do not stop.

The queue's own stopping point is the end of M38, or the first halt condition below.

**Never unattended:** `git push`. Full stop. The operator reviews the whole run's history before
anything becomes public.

**Halt and wait for the operator on:**

- a red gate that survives **one fix round** — where a fix round is one diagnose-edit-rerun cycle,
  not an open-ended loop;
- any need to park a **new** `#[ignore]` gate — that is a judgment about whether something is a wall
  or a bug, and it is the operator's (CLAUDE.md's honest-gate discipline). **One narrow exception,
  authorised by the operator 2026-09-09: M36 may park gates for the walls it measures**, under the
  four conditions in §3's M36 entry. The exception exists because for those binaries the README has
  *already ruled* that gates are owed, so M36 discharges an acknowledged debt rather than making a
  new judgment. It does not generalise to any other milestone;
- any need to bump `TRACE_MAGIC` — a format break invalidates every existing recording;
- a `root_cause_class` of **E2** (§6) — halt immediately, do not continue the queue. An **E1** row
  does not halt; an E row not yet disambiguated halts until M36 resolves it, and one that resists
  resolution is treated as E2;
- any task that would require inventing scope its spec does not cover.

A halt is **a stop with the branch left intact and a written explanation**, never a best guess and
never a silent narrowing of scope.

## 6. The `root_cause_class` enum

Defined here, in advance, so M37–M38's agents route rather than invent. Six classes. The
three-class draft could not express two of the eight known failures.

| class | meaning | routing |
|---|---|---|
| **A** `retired-by-soundness` | M32–M35 already fixed it | Verify it now passes; retire the row. No new work. |
| **B** `known-unmodelled` | A named, already-understood gap. `csh`/`tcsh`'s `dup2` is the type specimen. | → M37/M38 fix milestone. |
| **C** `new-subsystem` | Needs a capability that does not exist in the tree. | **Park + HALT.** Do not attempt. |
| **D** `not-a-defect` | Fails by design; no fix possible or wanted. `/usr/bin/yes` never terminates and is failed on purpose — excluding it would raise the tally without changing anything about retrace. | Document; retire the row; do not count as a defect. |
| **E1** `harness-nondeterministic` | The **sweep script** varies between identical runs — its own bug, not retrace's. | Fix or document; **continue the queue.** |
| **E2** `retrace-nondeterministic` | **Retrace's own record/replay** varies between identical runs. | **HALT the entire queue.** This becomes the next milestone regardless of what the queue said. |

**Class E is one observation with two opposite correct responses, so it halts only until
disambiguated.** The symptom — a binary's result moving between identical runs, as `dddiagnose`
moved 45/9 ↔ 46/8 while M29 watched — does not by itself say which. Telling E1 from E2 is a
*measurement*, which is exactly what M36 is for; so an E row halts the queue **pending that
disambiguation**, not permanently and not never.

The asymmetry is the point. Halting for E1 wastes a night on a script bug, and the precedent for
script bugs is strong: M30's own section records the sweep's stderr channel silently dropping
`[M30 CANARY]` lines, and the script's first draft compared a variable against itself, reporting
four binaries as passing when they were not. Continuing past **E2**, on the other hand, means every
later milestone builds on a tool whose core guarantee is unproven — and CLAUDE.md puts that stake
plainly: *"Determinism is the whole game. Nothing nondeterministic may enter the trace."*

An E row that resists disambiguation — M36 cannot tell E1 from E2 — is treated as **E2**. The
expensive error is assuming the harness.

**A row that fits no class is itself a halt.** The enum is not to be extended by an agent.

## 7. Gate economics

The full chunked gate is expensive — `retrace` alone is 60 test binaries, many of them full
record-and-replay end-to-end runs. Eight full gates would dominate the run's wall clock.

- **During TDD tasks:** targeted subsets only (`cargo test -p <crate> --test <name> --
  --test-threads=1`).
- **At milestone close only:** the full chunked gate, per CLAUDE.md's chunking, `--no-fail-fast`,
  exit code captured **before any pipe**, **including the `--bins` chunk** and any split library
  crate's `--doc` chunk. Both omissions are silent — they cost binaries without turning anything
  red.

`--test-threads=1` is mandatory throughout (one VM per process).

## 8. Known risks

- **M35's risk was overstated in the first draft** — `failsys.s` and `failsysctl.s` already exist,
  so it starts from a fixture. What it still owes is a measurement that one of them reaches the
  `if !err` gate with a pointer argument worth banding. Placed last in the soundness phase anyway,
  so a halt there still banks M32–M34.
- **Two of this charter's own claims were wrong within hours of writing it** (M32's scope, M35's
  fixture), both caught by a cheap check against the tree rather than by review. That is the
  charter's §1 argument turned on the charter itself, and it is the reason every milestone spec owes
  a measurement step before its first edit.
- **M33 may move the sweep tally downward** by making a silent failure loud. Expected; those
  binaries become M36 rows.
- **M37–M38's scope is unknown until M36 runs.** Intended. The cost is that the run's tail cannot be
  estimated in advance; the benefit is that it is specced by evidence rather than by guess.
- **Symmetric-but-wrong remains invisible** (inherited from M24/M31): two artefacts wrong in the
  *same* way pass any test that only diffs them against each other. Nothing in this queue addresses
  that, and no milestone here may claim it does.
- **The operator is not in the loop for hours.** Every halt condition in §5 exists because the
  alternative is an agent making an operator's decision at 3am and documenting it convincingly.

## 9. What each milestone's own spec must contain

A charter entry is not a spec. Before implementation, each milestone's spec must state, at minimum:

1. **The wall, located** — file and line, with the symbol name alongside (M31's lesson: a bare line
   number goes stale inside the very commit that edits the file).
2. **What measurement establishes the wall is where the spec says it is** — and if none has been
   taken yet, that measurement is task 1, not an assumption.
3. **The symmetry obligation** — for any new trap-handling arm, which record arm and which replay
   arm change, and why they recompute identical bytes (CLAUDE.md rule 1); or why the change belongs
   below the trace instead (rule 2).
4. **The positive control** — the specific mutation that must turn the new test red, to be run and
   recorded. M28's `let band = 0;` and M31's `SigTable::default()` are the precedents. A guard
   nobody has watched fail is a guard nobody knows is wired up.
5. **What the milestone deliberately does not do**, so a later reader does not mistake its scope for
   coverage it never had.
