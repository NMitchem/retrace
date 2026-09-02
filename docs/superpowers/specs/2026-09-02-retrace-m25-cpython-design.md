# M25-cpython design — the headline the vision named, and never once measured

Companion to [`2026-09-02-retrace-m25-cpython-measurements.md`](2026-09-02-retrace-m25-cpython-measurements.md).
Read that first. Every number below traces to a finding there, except the syscall identifications in
"The census, corrected", which were taken at spec time and are labelled as such.

## What this milestone is

The 2026-07-05 vision spec named one headline: reverse-debugging **real CPython**. Twenty-four
milestones later nothing in the tree had ever pointed `record-dyn` at `python3` — no spec, no plan,
no test, no status-log entry (measurements, "Why this document exists"). The belief that carried
that absence was that an interpreter was far away. Nobody had checked.

M22 is the precedent and the lesson: every Apple system binary failed identically for twenty
milestones, was read as a capability boundary, and turned out to be a loader assert on a fat header.
*A wall that every instance of a class hits identically deserves one probe before it is believed.*
The t0 probe here found the same shape. CPython's core runtime — the framework dylib mapped and
relocated, the allocator, the GIL, frozen importlib, the bytecode interpreter — **already records and
replays bit-for-bit** behind two defects, one of which is a single bit in a constant.

So the milestone is scoped to what measurement showed is actually in the way:

> **Rung 7: the real CPython interpreter binary running `-c 'print(1)'` records and replays
> byte-identically, twice, exit 0 — or the gate parks at the first measured wall that is not small,
> with that wall's evidence on the test.**

## The measured walls

| # | Wall | Evidence | Shape |
|---|---|---|---|
| 0 | `python3` on PATH is a launcher that `posix_spawn`s the real interpreter in place | Finding 0 | **Not a wall.** Records and replays byte-identically; a capability gap, scoped out |
| 1 | `DC ZVA` from EL0 traps, because `SCTLR_EL1.DZE` is clear | Finding 1 | One bit in one constant, below the trace |
| 2 | `getdirentries64` (344) and `fstatfs64` (346) forward an untranslated guest fd | Finding 3 | Two table entries, record-side only |
| 3+ | Unknown | — | **Unmeasured.** Nothing past wall 2 has been observed |

Wall 0 deserves its row because it is the command a user actually types, and because its outcome is
the surprising one: the launcher's failure is *retrace working correctly*. The forwarded
`posix_spawn` does not replace the image, `pythonw.c` takes its `err(1, …)` path, and that whole run
— 70,491,550 bytes of it — replays byte-identically with matching fall-through counts. Nothing about
determinism is broken. Exec-in-place is simply not modelled, and the vision scoped follow-fork out
of v1 without ever naming exec.

## Fix 1 — `SCTLR_EL1.DZE`

`_platform_memset` uses `DC ZVA` above a size threshold; CPython's allocator zeroes a 32,640-byte
block at startup and hits it. An EL0 `DC ZVA` traps to EL1 with EC 0x18 when `SCTLR_EL1.DZE == 0`,
and `Box_::run()`'s only `Ec::SysReg` arm is `try_emulate_timebase`
(`crates/retrace-box/src/lib.rs:1015`), so everything else in that class becomes a non-syscall exit.
That is the exact death in Finding 1, decoded from `ISS=0x12dc68` to `SYS #3, C7, C4, #1, Xt`.

The change is to set bit 14 (`0x4000`) in `SCTLR_MMU_ON_BASE`
(`crates/retrace-box/src/lib.rs:174` on this branch; the measurements cite `:185`, which is the same
constant at a different line on the branch they were taken from). It has the M2-tbi / M2-cpuid shape:
one constant, one bit.

### Why this is symmetric by construction, and not the M24 class

The comment above `sctlr_mmu_on` calls it **"the one derivation"**, and it is true today: all four
SCTLR install sites go through it — `crates/retrace-box/src/lib.rs` lines 1139, 1735, 2604 and
**4474**. The fourth is `restore`, replay's own entry point, reached through
`sctlr_mmu_on(state.pac_enabled)`. So a bit added to the base constant arrives on the replay side
without a second edit.

That is worth naming precisely, because M24-restoreaudit exists for the opposite case: state
established on the record-only `load_dynamic` path that `restore` never rebuilds, which passes on
record and diverges on replay. `SCTLR_MMU_ON_BASE` is immune to that class *structurally*, not by
luck, and the four-site invariant is already enforced by `dbg_pac_enabled`
(`crates/retrace-box/src/lib.rs:4580`), which panics if any site set SCTLR without going through the
derivation.

This is symmetry rule 2 work in its cleanest form: `run()` is shared, the bit is in a constant both
sides compute from, and nothing is recorded.

### Why `TRACE_MAGIC` does not move, and why that reason is better than M23's

M23 changed the vector table's padding, which lives in the trampoline page and is therefore snapshot
**content** — so a pre-M23 recording restores its old padding and reproduces the bug M23 removed.
The README records that as a sharp edge rather than a clean bill of health.

SCTLR is not snapshot content. A snapshot is `(regions, regs)` and `Regs` is
`{x[31], pc, sp_el0, cpsr}` — SCTLR appears nowhere in it, which is exactly why
`pac_posture_from_memory` exists to re-derive the PAC posture from guest memory. The *current code's*
`sctlr_mmu_on` governs both sides of every replay, old traces included. A pre-M25 recording replayed
by post-M25 code gets `DZE=1` on a guest that never executed `DC ZVA` (it could not have — it would
have died), so there is nothing to diverge.

The converse is real and worth stating: a **post**-M25 recording replayed by **pre**-M25 code would
trap. That is a forward-compatibility hazard of a kind the trace format does not express, and it is
identical in kind to M23's. It is not a reason to bump the magic; it is a reason to say so here.

### Why `UCI` and `UCT` stay clear

Bits 26 (`UCI`, EL0 `DC CVAU` / `IC IVAU`) and 15 (`UCT`, EL0 `CTR_EL0` reads) are the two a JIT's
`sys_icache_invalidate` needs, and they are clear too. **Nothing has measured a guest issuing either
instruction.** Finding 1 observed `DC ZVA` and nothing else; whether Homebrew's 3.14 enables the
experimental JIT was explicitly not measured.

Setting them speculatively would be the failure this repo keeps writing down — a right conclusion
resting on an unmeasured supporting fact. Leaving them clear costs nothing, because the existing
EC 0x18 non-syscall exit is *already* the fail-loud path for that case: a guest that issues
`DC CVAU` dies with a decodable ISS naming the instruction, exactly as `DC ZVA` did. The unit test
pins both bits clear so the omission reads as a decision rather than an oversight.

## Fix 2 — two entries in `fd_operands`

`retrace_arch::fd_operands` (`crates/retrace-arch/src/lib.rs:97`) lists neither 344 nor 346, so
`Box_::translate_fds` left `x0 = 4` alone and the host kernel serviced `getdirentries64` on
**retrace's own fd 4**, which is not a directory. XNU returns `EINVAL` for a non-`VDIR` vnode, and
that `[Errno 22]` is what CPython printed while importing `encodings` (Finding 3).

This is the M10 class verbatim — "the guest's fds were retrace's host fds" — one table entry wide
per syscall. Both take their descriptor in `x0`: `fstatfs64(int, struct statfs64 *)` is declared in
the SDK's `sys/mount.h:444`, and `getdirentries64` is not in the SDK at all (libc calls it privately
from `opendir`/`readdir`), so its operand position rests on the trap arguments Finding 3 captured —
`[fd=0x4, buf, 0x2000, &basep]`, a directory fd the guest had just opened, in `x0`.

### Symmetry rule 1 is not engaged, and that is checkable

No dispatch arm is added, so no mirror is owed and `verify_thread`'s site count does not move.
`forward_and_diff` (`crates/retrace-box/src/lib.rs:2791`) takes `args: [u64;8]` **by value**, keeps
the guest's own view in `gargs`, and translates into a local copy before forwarding. What reaches
the trace is the guest's descriptor, unchanged. The fix therefore alters only which host descriptor
the record side forwards to, and hence which kernel writes get captured; replay applies those
recorded writes and never forwards anything.

### The census, corrected

Finding 3 lists 344 and 346 as missing, then adds "and possibly 228, 406, 427 (unidentified —
identify before classifying)". Identified at spec time against
`$(xcrun --show-sdk-path)/usr/include/sys/syscall.h` on this machine, all three resolve, and none of
them needs anything:

| # | Name | Disposition |
|---|---|---|
| 228 | `fgetattrlist` | **Already in the table** as `SYS_FGETATTRLIST` → `&[0]` (`lib.rs:62`, `:102`) |
| 406 | `fcntl_nocancel` | **Already in the table** as `SYS_FCNTL_NOCANCEL` → `&[0]` (`lib.rs:67`, `:100`) |
| 427 | `fsgetpath` | **Correctly absent.** `ssize_t fsgetpath(char *, size_t, fsid_t *, uint64_t)` (SDK `sys/fsgetpath.h:45`) takes no descriptor |

So the census's open question closes at two entries, not five. The plan still carries the
verification step, because a spec-time reading of a header is evidence and not a run.

## The wall-chain, and where it stops

After both fixes the run is re-measured with `RETRACE_TRACE=1` and the milestone proceeds as M2 did:
clear the next measured wall, re-measure, repeat. A wall is in scope only if its fix is **small,
below the trace or table-shaped, and mutation-verifiable**. Anything else — a new dispatch arm with a
recorded reply, a trace-format change, an unmodelled kernel behaviour — parks the gate with that
wall's evidence in the `#[ignore]` reason and closes the milestone.

Closing parked is an allowed outcome, not a failure. `dispatch_e2e` was parked by M18, moved twice as
each measured wall fell, and then cleared; `sysbin_e2e`'s second gate was parked by M22 and un-parked
by M23. Any dispatch arm that *is* added obeys symmetry rule 1 in full: record arm and replay mirror,
same `Box_` method, same arguments, both before the generic forward arm, and a `verify_thread` call
on any arm that consumes a landmark and returns — because every new mirror silently creates an oracle
hole until its check is added.

Finding "What was not measured" names what is plausibly next: `site`, `.pyc` reads
(`open`/`fstat`/`read`/`mmap`), and possibly `dup2`, which is M10's deliberate fail-loud and would be
a park rather than a fix.

## The gates

**`cpython_e2e`** — `crates/retrace/tests/cpython_e2e.rs`, created in the plan's first task and
parked `#[ignore]`d from that first commit, with wall 1's exact `RECORD ERROR` line as the reason.
It records and replays the real interpreter with `-c 'print(1)'` through
`util::assert_rung_records_and_replays`, which demands a clean `exit(0)` with exactly `b"1\n"` and
replays twice — so a guest that died inside dyld cannot pass it, and neither can one that reached
"core initialized" and then failed to import `encodings`.

**The launcher gate, running from the first commit.** `bin/python3.14` records and replays
byte-identically and exits 1 with the `posix_spawn` error text. This is the milestone's honest half:
it turns "exec-in-place is unmodelled" from a sentence in the README into a test that fails if the
limit's *shape* changes. It cannot use the rung helper, which requires exit 0; it asserts the guest's
own stderr text, because exit 1 alone is a code a weaker failure would also produce.

Both depend on a Homebrew binary that is not a repo artifact, so both skip with a loud `eprintln!`
when it is absent — the `jq_e2e` discipline, because a silent skip reads as a green it did not earn.

## Scope

**In.** `SCTLR_EL1.DZE`; `fd_operands` entries for 344 and 346; the classification of 228/406/427;
the two gates; the wall-chain for as long as its walls stay small; the README and status-log edits.

**Out.**

- **Exec-in-place** (`posix_spawn` with `POSIX_SPAWN_SETEXEC`, `execve`). It is a real capability and
  a successor milestone. Until it lands, the README says to point retrace at the real binary, and
  the launcher gate holds the current behaviour still.
- **`UCI` / `UCT`**, until a guest is measured issuing an instruction they gate.
- **Making the `fd_operands` default fail loud.** See Residual.
- **`node` and `git` as guests.** `node` is a JIT and needs W^X promotion and cache maintenance at
  scale; unprobed (Finding "What was not measured").

## Exit criterion

`cpython_e2e` is un-`#[ignore]`d and green — the real interpreter binary at
`/opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python`
records `-c 'print(1)'`, exits 0 with stdout exactly `1\n`, and replays byte-identically twice.

If it is still parked, the criterion is that it is parked at a **measured** wall whose evidence is on
the test, the README's Known limits and the status-log section, and that the launcher gate and both
fixes are green — a milestone that parks a new gate for a capability it does not yet have has
regressed nothing.

## Residual

- **The silent default in `fd_operands` is not fixed here, and it is the milestone's largest known
  hazard.** An fd-taking syscall absent from the table is forwarded with the guest's number
  unchanged and **nothing asserts**. `fd_operands`' own doc comment already states the rule — "absence
  must mean *provably takes no fd*, never *not gotten to yet*" — and wall 2 is the second time
  reality broke it (M10 recorded the same property for `F_DUPFD`). The failure it permits is the one
  the divergence oracle structurally cannot see: if a guest fd happens to be a valid retrace
  descriptor of the right type, the call succeeds against the wrong file, the writes are captured,
  and record and replay agree on something self-consistent and wrong. Wall 2 was *visible* only
  because retrace's fd 4 was not a directory.
  **Why it is out of scope here:** the fix is not a table entry but a complete classification of
  every fd-taking BSD syscall, plus a default that refuses rather than forwards. That default would
  fire on the first unclassified syscall any existing guest issues, so its blast radius crosses every
  gate in the tree.
  **The shape of the successor:** enumerate the fd-taking numbers from XNU's `syscalls.master`,
  table them with their operand indices, change the default from `&[]` to a fail-loud path, and then
  *measure the blast radius* — record every gate guest under a build that logs (rather than panics
  on) an unclassified fd-taking number, and turn the log into the remaining table entries before the
  panic lands. That measurement is the milestone, not the table.
- **Reverse execution, seeks and watchpoints over a CPython trace are not gated by M25.** Only
  `record` and `replay` were run; `debug` and `from_checkpoint` were not exercised on it (Finding
  "What was not measured"). An 80 MB trace of an interpreter is a plausible stress case for
  `checkpointed_seek` and nothing here claims it works.
- **Record and replay times were not captured.** Both completed inside a 270 s bound and that is all
  that is known. If the gate turns out to be slow, that is a fact to measure at the close, not a
  claim to make now.
- **What the forwarded `posix_spawn` did on the host was not captured** — only that the image was not
  replaced. The launcher gate pins the guest-visible outcome, not the host-side one.

## Risk register

| # | Risk | Disposition |
|---|---|---|
| R1 | The chain does not end at wall 2, and the milestone closes parked without rung 7. | **Accepted, and planned for.** Every wall past 2 is unmeasured by construction. The gate exists from the first commit precisely so that parking is a documented outcome rather than an unfinished one. |
| R2 | The silent `fd_operands` default lets some *other* guest record something self-consistently wrong, and M25 does not close it. | **Open, and stated above.** M25 narrows the hole by two entries and refuses to widen the fix beyond what it can measure. The successor is described, not deferred silently. |
| R3 | `DZE` changes SCTLR for **every** guest, including all currently-green gates. | **Mitigated by argument, verified by the gate.** A currently-green guest cannot have been executing `DC ZVA` — it would have died on the EC 0x18 exit, which is how wall 1 was found. The full chunked gate is the check. |
| R4 | `DC ZVA`'s block size comes from `DCZID_EL0`, which is host-derived and never recorded. | **Accepted.** It is a constant of the machine, and retrace has no cross-machine replay; record and replay run on the same CPU. Worth naming because it is a host value reaching guest behaviour without passing through the trace. Finding 2 shows `DCZID_EL0.DZP` permits the instruction on this machine. |
| R5 | The gate's guest path is version-specific and a `brew upgrade` silently turns the gate into a loud skip. | **Mitigated.** The gate uses the version-stable `/opt/homebrew/Frameworks/Python.framework/Versions/3.14/…` path rather than the `Cellar/python@3.14/3.14.6/…` one the measurements used, and the skip message names the path it wanted so a reader can tell "not installed" from "moved". |
| R6 | The launcher gate pins a **limitation**, so a successor that implements exec-in-place will see a passing test asserting the old behaviour and may defend it. | **Mitigated by wording.** The test's header says in as many words that it exists to hold a known gap still, and that the correct response to implementing exec is to rewrite it, not to preserve it. |
| R7 | M24-restoreaudit is closing concurrently on `main` and touches `restore`, one of the four SCTLR install sites. | **Mitigated by sequencing.** The plan merges `main` into the branch before any gate run, and re-checks that all four sites still route through `sctlr_mmu_on` after the merge. |
