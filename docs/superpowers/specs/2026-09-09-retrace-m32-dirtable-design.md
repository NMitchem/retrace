# M32-dirtable — the canary decision belongs to the argument, not the syscall

**Date:** 2026-09-09
**Status:** design
**Charter:** `docs/superpowers/specs/2026-09-09-retrace-m32-m38-program-charter-design.md` (§3, M32)
**Discharges:** M30's owed-list, first entry — *"the largest single thing M30 gives up."*

## 1. The wall, located

Three symbols, by name and line, current as of `a33983d`:

| symbol | file:line | role |
|---|---|---|
| `retrace_arch::reads_guest_buffer(num: u64) -> bool` | `crates/retrace-arch/src/lib.rs:308` | the syscall-level predicate |
| `retrace_arch::dest_buffer(num: u64) -> Option<(usize, DestLen)>` | `crates/retrace-arch/src/lib.rs:176` | the (single) destination argument, for the clamp and window |
| `Box_::forward_and_diff` | `crates/retrace-box/src/lib.rs:3233` | `let fill_canary = !retrace_arch::reads_guest_buffer(num);` |

That last line is the defect in one statement: **a per-syscall boolean gating a per-argument
decision.**

M30 withholds the guard band from any syscall the kernel reads through, because a canary in such a
buffer reaches the kernel as data and corrupts the guest's output while record and replay stay
bit-identical (`bigwrite_e2e` is the regression test for exactly that). The withholding is correct.
It is *over-broad*: it covers every register of the call, including arguments the kernel only
**writes**.

`reads_guest_buffer` lists `sendfile` (337) and `mach_msg2_trap` (−47). Both have genuine
destination arguments that consequently receive no destination-side coverage.

## 2. Scope — and the measurement that set it

The charter scoped this milestone to `sendfile`. **A measurement taken before writing this spec
changed that**, and the finding is recorded here rather than quietly acted on:

> **No guest in this repo issues `sendfile`.** Syscall 337 appears in `retrace-arch`'s comments and
> in the `reads_guest_buffer` list (`lib.rs:323`), and nowhere else in the tree. Nothing in
> `crates/retrace-guest/` dispatches it.

Seeding the new allow-list with `sendfile` alone would ship a branch no guest ever takes — the
dead-channel trap M29 hit twice and M30 hit three more times. So the scope is:

**Build the per-argument mechanism, and seed it from `mach_msg2`, which every dynamic guest issues
constantly and which `crates/retrace-guest/asm/machmsg.s` exercises directly.** `sendfile` gets a
table entry alongside it, explicitly marked *structurally covered but unexercised* — a table entry
is cheap, and the honest label is what stops a later reader mistaking it for tested.

## 3. The safety-critical detail: polarity

The change is **default-deny with a destination allow-list**, never a source deny-list:

```rust
// CORRECT
fill_canary(arg i)  :=  !reads_guest_buffer(num) || is_known_dest_arg(num, i)

// WRONG — re-opens M30
fill_canary(arg i)  :=  !arg_is_source(num, i)
```

The reason is recorded at `crates/retrace-box/src/lib.rs:3222`:

> *"The decision is per-SYSCALL and covers all eight registers, because the reproduction's
> corrupting entry was a stale register that was not an argument at all."*

`forward_and_diff` snapshots a window for **every mapped-looking argument** — all eight registers,
not merely the syscall's declared ones. In M30's reproduction the corrupting entry was a stale
register that was not an argument at all. Under a source deny-list such a register is "not a
source", falls through to `fill`, and the corruption returns. Under a destination allow-list it is
not on the list, stays withheld, and M30's guarantee holds by construction.

**This inversion is the single thing most likely to be gotten wrong by an implementer working from a
one-line summary. It must appear in the plan's task text, not only here.**

## 4. Measurement — task 1, before any edit

### 4a. The `mach_msg2` send/receive boundary

The argument layout is **already decoded** and need not be reverse-engineered
(`crates/retrace-core/src/machmsg.rs:40-46`):

| field | source |
|---|---|
| `data` — the buffer, send **and** receive | `args[0]` |
| `send_size` | `hi(args[2])` |
| `rcv_size` | `lo(args[6])` |

A Mach reply overwrites the same buffer, so the send and receive regions **overlap by design**. The
kernel *reads* `[data, data + send_size)` and *writes* the reply into the same buffer.

**The hypothesis task 1 must confirm or refute**, stated as a hypothesis because nothing has
measured it: the canary band is placed at `ipa + len` (`lib.rs:3240`, `let base = *ipa + *len`) —
that is, **past** the window's length. If the window `len` for `args[0]` is always `>= send_size`,
the band lands where the kernel never reads, and filling it is safe. If `len` can be shorter than
`send_size`, it is not, and the entry must be withheld exactly as it is today.

Measure against a real `machmsg.s` recording and against a dynamic guest (which issues far more
varied messages), and record actual `(len, send_size, rcv_size)` triples. **Do not infer this from
the ABI.** `retrace-core/src/lib.rs:435` asserts `send_size <= 0x1000`, which bounds one operand but
says nothing about the band's placement relative to it.

### 4b. The dead-channel check, generalised

Before seeding **any** entry, confirm a guest in this repo actually dispatches that syscall. The
`sendfile` finding in §2 is what this check is for; it is cheap and it has already paid once.

## 5. What must change

### 5a. `retrace-arch` — a new per-argument predicate

Add beside `reads_guest_buffer`:

```rust
/// Arguments of a `reads_guest_buffer` syscall that the kernel WRITES rather than reads.
/// Allow-list, not deny-list — see the polarity argument in the M32 design.
pub fn is_known_dest_arg(num: u64, idx: usize) -> bool
```

Seeded from §4a's measurement. `reads_guest_buffer` is **kept unchanged** — it still gates
`overran_window`'s live detector (`crates/retrace-box/src/lib.rs:2946`), which is a different
question from whether a given band may be filled.

### 5b. `retrace-box` — carry the argument index, and apply it at all four sites

`windows` is pushed at `crates/retrace-box/src/lib.rs:3182` as
`windows.push((args[i], win, pre, pre_band, 0))` — the index `i` is **dropped**. It must be carried
so the fill can consult it.

`fill_canary` gates **four** sites, and all four must move to the same per-window predicate
together:

| site | line | what breaks if it disagrees |
|---|---|---|
| the fill | 3234 | — |
| `disturbed` check | 3447 | an unfilled band is asked whether its canary is intact |
| `overran` check | 3485 | wrong detector chosen for the band |
| the restore pass | 3541 | **a filled-but-unrestored canary is captured into the trace as a kernel write** |

The fill/restore pair is the dangerous one. **The invariant is: a band is filled if and only if it
is restored.** The plan must make this an explicit assertion, not a reviewer's diligence.

## 6. Positive controls

Required by charter §9. Two, because there are two distinct ways to get this wrong.

**Control 1 — the mechanism is wired up.** *(NEVER EXECUTED — see §9. No mechanism was built, so
there is no `is_known_dest_arg` to revert and no destination-side test to red. The status log
records it as an unexecuted control rather than a discharged one; it is left here as written so a
successor can see what was promised.)*
> Revert `is_known_dest_arg` to return `false` unconditionally (restoring per-syscall behaviour).
> The new destination-side test must go **RED**.

**Control 2 — the polarity of §3 is load-bearing.**
> Change the predicate to a source deny-list (`!arg_is_source(num, i)`). `bigwrite_e2e` must go
> **RED**.

If control 2 does **not** go red, the deny-list formulation is not caught by the existing regression
test, and that is a finding worth more than this milestone — report it and halt rather than
proceeding on a guard that cannot see its own inversion.

Record each control's exact failure message, as M31 recorded `rich: signal dispositions` and M28
recorded `let band = 0;`.

## 7. What this milestone deliberately does not do

- **`sendfile` is table-only.** Entered, and labelled unexercised. No socket guest is written; if a
  later milestone wants that coverage it must write one, and §4b is why the label matters.
- **`dest_buffer` still returns one destination per syscall.** A syscall with two genuine
  destinations remains unexpressible. `getdirentries64` and `recvfrom` already document a second
  destination dismissed on a number; this milestone does not change that shape.
- **No new syscalls are added to `reads_guest_buffer`.** That is M33.
- **The band is not widened.** M30's final owed entry stays unattempted, and M28's suppression count
  stays the warning for whoever takes it up.
- **If §4a refutes its hypothesis**, `mach_msg2` is withheld exactly as today, the mechanism still
  lands, and the milestone reports a *negative* measurement as its result. That is a success, not a
  failure — and it is the outcome that must not be quietly converted into shipping the entry anyway.

## 8. Symmetry obligation

Per CLAUDE.md rule 2 this change sits **below the trace**: `forward_and_diff` is record-side, and the
canary is filled and restored *within one syscall's forwarding*, never surfacing to the
record/replay loop. No replay arm changes, and **no `TRACE_MAGIC` bump is required** — the trace's
shape is untouched.

That is the claim to check first in review. If any part of this change causes a byte to differ in
`Event::Syscall`'s recorded writes, the analysis above is wrong and the milestone has become a
format-affecting one — a charter §5 halt condition.

## 9. Outcome — the milestone measurement told us not to build

**Status: CLOSED as a measurement milestone. Tasks 2–6 were not executed.** Recorded here rather
than in a separate document because a spec whose conclusion contradicts its own plan must say so
where the plan's reader will look.

### What was measured

Task 1, after two fix rounds, walked **35 real `mach_msg2` landmarks** across `hello_dyn`, `jq` and
CPython — all three present, all three walked, none skipped. Each row was classified by calling the
real `machmsg::route()`, never a hand-copied allow-list.

| | |
|---|---|
| landmarks measured | 35 |
| **governed** by this milestone (`Route::Forward`) | **13** |
| max `avail` among governed calls | **24,672 bytes** |
| `window_cap` — the threshold for a band to exist at all | **65,536** |
| governed calls producing a nonzero band | **zero** |

`band > 0` requires `avail > window_cap`. No governed call comes within 40 KiB of it. So
`is_known_dest_arg(-47, 0)` — the entry §5a exists to add — **would have been inert on the day it
shipped**, not by argument but by measurement, across the three fixtures walked.

**That the 13 are shallow is one fact, not thirteen coincidences.** All five ids in
`FORWARD_ALLOWLIST` (200, 206, 3418, 3405, 412) are MIG-generated kernel-RPC stubs, and a MIG stub
builds `union { Request; Reply; } Mess;` as a **stack local** and passes `&Mess` as the message
buffer. `avail` is the distance from a buffer to the end of its backing, so for a governed call
`avail` *is* the stack depth measured from that stack's top — and the geometry holds for all three
stacks retrace produces: the main stack is 256 KiB backed with the buffer below its top
(`crates/retrace-box/src/lib.rs:94-95`), a pthread stack is the guest's own mmap and what
libpthread hands `bsdthread_create` is the stack **TOP**, with SP starting there and growing down
(`crates/retrace-box/src/lib.rs:4681-4683`), and a workqueue worker's stack
puts the struct at the top and grows down into the region
(`crates/retrace-box/src/lib.rs:4426-4441`). A shallow governed call is a shallow *frame*, and
every governed call measured here was made during process initialisation.

The same structure explains the outlier. The one landmark that DID carry a nonzero band — msgh_id
`0x400000cf` at ~4.1 MB `avail` — is a **libxpc message-queue send with a heap buffer**, the only
class in the corpus where `avail` is unrelated to stack depth at all, and exactly the class
`route()` excludes as `Route::RefuseMqSend`.

**What stays open, and this section will not pretend otherwise: nothing bounds the DEPTH at which a
governed id can fire.** All 13 measured calls are process-initialisation calls, which are shallow by
construction, so the population is **biased** — the sample size says less than it looks like it
does. A `semaphore_create` (3418) from a dispatch semaphore built deep inside a call chain, or a
`host_info` (200) behind a `sysconf`, are ordinary things for a program to do, and 64 KiB of frames
sits well inside a 256 KiB stack. The inertness finding is **unlikely to reverse and mechanistically
explained, but not proven**. `every_real_mach_msg2_in_the_corpus_is_checked_for_a_nonzero_band`
therefore asserts `governed_max_avail < PTR_WINDOW_CAP` rather than only printing it, so the day a
fixture **in that test's own corpus** contradicts this section, this section reds instead of quietly
rotting. That qualifier is the limit of the tripwire: a new e2e guest added elsewhere in the repo is
not walked by this test and would not trip it.

### Why that closes the whole milestone, not just the `mach_msg2` entry

§2 already recorded that `sendfile` has no guest. `reads_guest_buffer` has exactly **two** members
with a genuine kernel-written argument — `sendfile` (337) and `mach_msg2` (−47); every other member
(`write`, `pwrite`, `writev`, `pwritev`, the `send*` family, `msync`) has none. Both candidates are
now measured dead: one has no guest, the other has no band.

**This milestone's coverage deliverable is therefore empty across every fixture it could walk —
measured empty on hello_dyn, jq and CPython, not suspected empty.** The corpus is those three;
`/bin/ps` and the threaded/GCD fixtures were not walked, and the paragraph above says what that
costs the claim.

### The finding that replaces it

The per-argument defect §1 identifies is real, but it is not a defect in `reads_guest_buffer`. It is
a defect in the **schema**. Four functions answer one question — *what does this syscall do with each
of its arguments* — in four incompatible shapes, and two of them lost the argument index the other
two keep:

| function | shape | keyed by |
|---|---|---|
| `fd_operands` | `&'static [usize]` | argument indices |
| `dest_buffer` | `Option<(usize, DestLen)>` | argument index + length source |
| `reads_guest_buffer` | `bool` | whole syscall |
| `writes_via_nested_pointer` | `bool` | whole syscall |

§5a's fix would have added a **fifth** view, to recover per-argument information that `dest_buffer`
already stores eight lines away. That is the M26–M32 lineage's recurring shape: each milestone adds
a view and reconciles it against the others, and each new view is a fresh chance to ship inert.

**The successor is a unification** — one `arg_kinds(num) -> &'static [ArgKind]` table from which all
four current functions derive, proven by an equivalence sweep over every syscall number. M32's
per-argument direction then stops being a table and becomes a field.

### What Task 1 did land, and it stands

- `Box_::band_len(avail, win)` hoisted out of `forward_and_diff` into a `pub`, pure function beside
  `band_not_covered`, so exactly one copy of the expression exists and a test can call production
  rather than re-derive it.
- The structural proof that whenever a band exists it begins at least `window_cap` (65536) bytes
  into the buffer, while `retrace-core` asserts `send_size <= 0x1000` — so a band can never land in
  a kernel-read region, at any `avail`.
- That proof's external premise is now **asserted as a relation between the two constants**, at
  compile time, in the only crate where both are visible:
  `const _: () = assert!(machmsg::SEND_SIZE_MAX < retrace_box::PTR_WINDOW_CAP, …)` at module scope
  in `crates/retrace-core/tests/machmsgband_dyn.rs`. Module scope rather than a test body, because
  the corpus test skips `jq`/CPython when they are absent and its runtime tripwire sits at the end
  of that body — a `const _` is checked whenever the crate compiles, on any machine and in any gate
  chunk. **This took two rounds and the first one looked right.** Round one made
  `machmsg::SEND_SIZE_MAX` a shared `pub const` and had the test import it instead of redefining
  it, which genuinely improved the *per-landmark* checks (real captured sends now compared against
  the real bound) — but every one of those checks is `send_size <= SEND_SIZE_MAX`, which a
  **widening** only loosens, so drift stayed undetectable while four sites, one of them a failure
  message, stated that it reds. The mirror constant `retrace-box`'s proof had been carrying was
  deleted rather than renamed: nothing compared it to what it mirrored, so it could not detect the
  drift its name implied it caught. The compile-time assertion was verified able to fail — setting
  the bound to `0x20000` stops the crate compiling with the message above.
- `dbg_window_len_for` returning `Option<usize>`, so "unmapped" and "zero-length window" no longer
  collapse.
- The 35-landmark corpus measurement itself, which is the evidence this section rests on.

### A category error caught twice, worth naming once

The same mistake recurred at two scales and was caught by two different mechanisms: Task 1's
original fixture measured msgh_id 4811, which `Route::ServiceVmMap` services and never forwards
(caught by review); fix round 2's corpus walk initially risked counting a refused message-queue send
with a genuine 64-byte band at ~4.1 MB `avail` (caught by the implementer, by classifying via the
real `route()` before drawing a conclusion). Both would have produced a confident, wrong headline.
(This heading said "three times" and listed two. Fix round 1's re-derivation of the band formula is
a different class — calling production versus copying it — so it does not make the count up.)
**The generalisable rule: classify by calling the production router, never by a copy of its
allow-list.**

Note the corollary, since it bears on the successor: **bands are not globally inert.** A 4.1 MB
mmap-backed buffer got one. The inertness measured here is specific to `mach_msg2`'s governed calls,
whose buffers are stack-resident and shallow.
