# retrace M18-workq Stage 2b — the worker runs, and the semaphore that joins it

Written 2026-08-23, against `main` at `60cea11` (Stage 2a merged, unpushed). This is the spec
section the M18 design document deliberately withheld:

> "Not specified here beyond its inputs, deliberately. It gets its own spec section and its own
> plan, written from Stage 2a's measurement."
> — `docs/superpowers/specs/2026-08-20-retrace-m18-workq-design.md:456-458`

That measurement now exists (`docs/superpowers/specs/2026-08-21-retrace-m18-stage2b-measurements.md`),
and this document is written from it. Where a claim comes from that document it is cited; where a
claim is an attribution or a hypothesis it says so, in the discipline Stage 2a's opcode names
established — **the raw value is the measurement, the name is a lead.**

## The problem, precisely

`crates/retrace-guest/c/dispatch_dyn.c` creates a semaphore, `dispatch_async`es a block onto the
global concurrent queue, waits on the semaphore, and prints `done`. The block prints `worker` and
signals. Two things stop it today, and Stage 2b owes both:

1. **No worker is ever built.** `workq_kernreturn` opcode `0x20` (`REQTHREADS`, attributed) reaches
   `Box_::guest_workq_kernreturn` and `panic!`s by design — "worker construction is Stage 2b"
   (`crates/retrace-box/src/lib.rs`). Stage 2a placed that wall deliberately, because the kernel
   allocates a workqueue thread's stack and pthread struct and enters `wqthread` with a register
   contract nobody had measured.
2. **Nothing can join it.** Main's `dispatch_semaphore_wait` lowers to a raw Mach trap that has no
   arm, falls to the generic negative-trap forward, and blocks retrace's own process forever.

## What Stage 2a measured, and what it did not

**Measured** (both runs, §1 and §3 of the measurements doc):

- `dispatch_semaphore_create` lowers to `mach_msg2` msgh_id 3418 (`semaphore_create`), already on
  `FORWARD_ALLOWLIST`; its reply mints port name `0x1403`.
- `dispatch_semaphore_wait` lowers to a raw Mach trap `num=-36` at `pc=0x1804adbb0`, carrying that
  same port in `args[0]`.
- `num=515` (`__ulock_wait`) appears **nowhere in either trace**. The M14/M17 `pthread + 0x34`
  address-equality correlation therefore has nothing to correlate on for this primitive.
- Both runs hang on that trap, produce zero bytes of guest stdout, and only one run's exit code was
  captured (142, external alarm).
- The `REQTHREADS` argument vector, verbatim: `[0x20, 0x0, 0x1, 0x40008ff, 0x0, 0x20, 0, 0]`
  (carried into `crates/retrace-box/tests/threads.rs`).
- Only two `workq_kernreturn` opcodes have ever been reached: `0x400` and `0x20`. That list is a
  floor, not a ceiling.

**Attributed, NOT verified on this machine** — each is a lead for Task 1, not a fact to build on:

- `-36` as `semaphore_wait_trap`, and `-33` as its signalling counterpart. `-33` **has never been
  observed at all**, in any trace, by anything. That `dispatch_semaphore_signal` lowers to `-33` —
  or to any trap — is inference from symmetry, and the design below must not assume it.
- The `WQOPS_*` opcode names, and `args[3]=0x40008ff` as a packed priority/QoS word.

**Unmeasured, and Stage 2b's to measure:**

- The `wqthread` entry register contract. `Box_::wq_thread_pc` and `Box_::pthread_size` were
  captured by Stage 1 (`crates/retrace-box/src/lib.rs:3334-3341`) and are consumed by nothing.
- Where the pthread struct sits relative to the kernel-allocated stack, and who initialises it.
- The park/return opcodes a running worker issues, which by construction cannot be enumerated until
  a worker runs.

## The mechanism, part 1: worker construction

### It needs no dispatch-loop change

Stage 2a already built the record arm (`crates/retrace-core/src/lib.rs:841`) and the replay mirror
(`:1842`), and both call `b.guest_workq_kernreturn(args)` — the same method with the same arguments.
Replacing the `REQTHREADS` panic with a spawn changes what that shared method *does*, identically on
both sides. **Symmetry rule 1 holds by construction, which is exactly the property CLAUDE.md says
the same-method-same-args discipline exists to buy.** The whole worker half lands inside
`retrace-box`; `retrace-core` is untouched by it.

### The kernel-allocates problem, and the escape hatch

M14 seeded registers and refused to touch the pthread struct, because the *guest* had already
allocated and populated it — "writing them here would be retrace inventing guest state it does not
own" (the thread-start contract comment in `crates/retrace-box/src/lib.rs`). For a workqueue thread
the kernel owns both the stack and the struct, so that rule appears to forbid what Stage 2b must do.

**Hypothesis, to be verified or killed by Task 1:** libpthread's workqueue entry distinguishes a
*fresh* thread from a *reused* one by a flag bit, and on the fresh path performs its own struct
initialisation on the memory the kernel handed it. If that holds, retrace allocates two regions of
guest memory and sets the flag that says "fresh — initialise yourself," and libpthread fills in the
layout. The box then invents an **address**, not a **layout**, and M14's rule survives intact rather
than acquiring an exception.

**If Task 1 kills the hypothesis, that is a wall, not a licence to guess.** Stage 2b then re-parks
`dispatch_e2e` at "the wqthread struct contract requires the box to author guest state it does not
own" and closes, in the honest-gate discipline. Inventing a struct layout from libpthread source
that was never verified against this machine is precisely the failure mode Stage 2a's opcode
discipline exists to prevent.

### The flow, given the existing scheduler

The scheduler is cooperative and block-driven — it switches only when a thread blocks or exits
(CLAUDE.md, "Guest threads"). That yields the whole sequence with no new scheduling machinery:

1. `REQTHREADS` allocates a stack and a pthread region, builds a `ThreadCtx` entering at
   `wq_thread_pc`, and `threads.spawn`s it `Runnable`. It returns 0 and main keeps running — a
   switch here would reorder the guest's own output, the same argument `ThreadTable::spawn`'s doc
   comment already makes for `bsdthread_create`.
2. Main reaches `-36` and blocks on `BlockReason::Sem { port }`.
3. `pick_next()` finds the worker, `switch_to` enters it at `wq_thread_pc`.
4. The worker runs the block, writes `worker\n`, and signals.
5. Main wakes; the worker returns into `workq_kernreturn` with a park opcode.

Step 5 is the unmeasured one and the most likely place a new wall appears. `guest_workq_kernreturn`
already refuses unmeasured opcodes by value, so it will fail loud and name what to measure.

### Allocation and determinism

The box's mmap bump allocator from `MMAP_BASE` is a pure function of the guest's syscall sequence,
and the spawn happens inside the shared method both dispatch arms call, so the worker's stack and
struct land at identical IPAs on record and replay with nothing recorded. This is the same posture
`bsdthread_create` has: emulated below the trace, no trace-format change.

## The mechanism, part 2: the mach-semaphore park/wake seam

### The new block reason

`BlockReason` gains a third variant beside `Join { target }` and `Wait { addr }`:

```rust
/// Waiting on a mach semaphore, keyed by PORT NAME in retrace's own IPC space — not a guest
/// memory address. See this spec for why the M14/M17 correlation cannot be reused.
Sem { port: u64 },
```

and `ThreadTable` gains `unblock_sem_waiters_on(port) -> Vec<usize>` beside `unblock_waiters_on`.
The enum's own doc comment already says the variants are "deliberately concrete rather than an
opaque token" so the wake seam can name its target; this is that design being used as intended.

**The namespace difference is real and must be documented at the variant.** `Wait { addr }`'s value
is guest memory the box already tracks. `Sem { port }`'s value is a port name minted by a *forwarded*
`semaphore_create` in retrace's own IPC space, which reaches the guest through the recorded reply.
Replay applies that recorded reply, so both sides key on an identical value and the divergence
oracle's `(num, args)` check covers the trap that carries it.

### The traps

`Box_::guest_sem_wait` blocks the current thread; `Box_::guest_sem_signal` wakes waiters on the
port. Both are emulated and **never forwarded** — forwarding `-36` is not whole-process-*fatal* the
way forwarding `bsdthread_create` is, but it is whole-process-*hanging*, which is just as fatal to a
recording (measurements doc §2).

Each appends an ordinary `Event::Syscall` landmark, so `RETRACE_TRACE=1` still shows the sequence
and the oracle still checks `(num, args, thread)`. Nothing about the *switch* is recorded —
`Event::Sched` is gone and stays gone.

**The signal trap's number is not yet known.** `-33` is an attribution with zero observations behind
it. Task 1 measures what `dispatch_semaphore_signal` actually issues; until then the seam is
specified as "the wait trap is `-36`, measured; the signal trap is whatever Task 1 names." Any
negative trap the seam does not model keeps falling to the generic arm, which is why the guard below
is scoped by value rather than by "anything semaphore-shaped."

### Semaphore ownership: traps only

`semaphore_create` (msgh_id 3418) stays forwarded and allowlisted. The alternative — intercepting
3418, minting a synthetic port name, and synthesising the complex MIG reply with its port descriptor
(the M2-bootstrap shape) — buys architectural cleanliness, not correctness: the trace already makes
the port name identical on both sides. **Decided: traps only.** If a later stage measures the
forwarded create as genuinely nondeterministic, it upgrades to full interception without disturbing
the seam. The cost accepted is one host semaphore object per create that is never waited on or
signalled again.

## Determinism posture

Everything Stage 2b adds is **below the trace or symmetric by construction**:

- Worker construction: inside a shared `Box_` method both arms already call. Nothing recorded.
- The park/wake seam: new record arms with new replay mirrors, each calling the same `Box_` method
  with the same arguments (symmetry rule 1). The landmark is an ordinary `Event::Syscall`.
- No trace-format change, so **`TRACE_MAGIC` does not move.** Stage 2b recordings stay readable by
  the same reader that reads Stage 2a's.

## Fail-loud boundaries

- **`-36` and its signalling counterpart must never reach `forward_and_diff`.** The guard belongs at
  or before the **generic negative-trap arm** (`crates/retrace-core/src/lib.rs:531`). It does
  **not** belong at the BSD forward guard (`:1004`), which negative trap numbers never reach — the
  measurements doc §2 is explicit that the BSD guard is the right *shape* and the wrong *location*.
- **Unmeasured `workq_kernreturn` opcodes** already refuse by value. Unchanged.
- **Deadlock** — main blocked on a semaphore with no runnable worker — is already caught: `pick_next()`
  returns `None` and `Box_`'s deadlock assert fires. Stage 2b adds no new silent-hang path.
- **`assert_no_stranded_signals`** at clean exit is unchanged and still applies.

## Two things that will bite if they are not planned for

1. **The `verify_thread` census goes 7 → 9.** There are exactly 7 call sites today
   (`crates/retrace-core/src/lib.rs` lines 1271, 1381, 1412, 1439, 1472, 1555, 2143). CLAUDE.md:
   "every new mirror silently creates a new hole until its oracle call is added — nothing
   structural couples the two." The wait and signal mirrors each `return` before the generic
   dispatch, so each needs its own call, **placed after that arm's own field comparison** so a
   genuine argument divergence still reports as itself. Stage 2a's mirrors inherited the check and
   the census correctly stayed at 7; these do not inherit it.
2. **Stage 2a's companion gate breaks on purpose.**
   `dispatch_e2e.rs::the_workqueue_syscalls_are_emulated_not_forwarded` asserts
   `rec.stderr.contains("worker construction is Stage 2b")`. Removing that panic makes it red. It
   must be **rewritten to assert the new reality, not deleted** — its `_pthread_wqthread` tripwire
   ("no host workqueue thread may exist inside the recorder") is the one thing that file exists to
   forbid, and it outlives the wall it was written beside.

## Scope

**In:** worker construction at `REQTHREADS`; the mach-semaphore park/wake seam; the arms, mirrors,
oracle sites and guard that seam needs; the gate and the two documents.

**Out:** interception of `semaphore_create`; timed semaphore waits (`dispatch_dyn.c` passes
`DISPATCH_TIME_FOREVER`, so no timeout path is exercised); worker thread *reuse* or teardown beyond
what the measured sequence forces; more than one worker; QoS or priority semantics — `args[3]` is
decoded only if Task 1 shows the entry contract needs it; any preemptive scheduling change.

**A note on "one worker," because it is an attribution and not a measurement.** `args[2] = 0x1` in
the measured `REQTHREADS` vector *looks* like a thread count, and the scope above assumes one worker
is enough. That reading is unverified. Building exactly one worker is the right first move either
way — it is what the measured sequence needs to reach step 5 — but if libdispatch requests more once
the first parks, that is risk 5 materialising, not a defect in this decision.

## Exit criterion

Stage 2b closes in **exactly one of two honest states**, decided by measurement and not by
preference:

- **Green:** `dispatch_e2e::a_dispatch_async_guest_records_and_replays` is un-`#[ignore]`d and
  passes — the worker runs the block, main observes the signal, the guest exits 0, and two replays
  are byte-identical to the recording.
- **Re-parked:** the gate stays `#[ignore]`d with its reason **rewritten** to name the new wall,
  which must be a wall Stage 2b actually reached and measured — not the one it inherited. The README's
  "Known limits" and a new `docs/status-log.md` section say the same thing.

Either way the stage delivers a moved wall and a truthful document. A stage that re-parks is not a
failed stage; that is the honest-gate discipline working.

## Testing

- **Thread-table unit tests** (`crates/retrace-box/src/thread.rs`): `Sem { port }` blocks, wakes by
  port equality, wakes the right thread and not others, and an empty wake is legal and not an error
  (the posture `unblock_waiters_on` already documents).
- **Box unit tests** (`crates/retrace-box/tests/threads.rs`, where the Stage 2a workq tests live):
  `REQTHREADS` spawns exactly one runnable thread entered at `wq_thread_pc`; it refuses when no
  `wqthread` was registered; the allocated stack and struct are at deterministic IPAs.
- **The e2e** (`crates/retrace/tests/dispatch_e2e.rs`): the existing body is already correct and
  asserts the right things — `worker\n`, `done\n`, exit 0, two byte-identical replays. It asserts on
  *what the guest printed*, never on the exit code alone, for the reason the file's own comment
  gives (`crashy_e2e` asserts 139 for an uncaught guest fault, so 139 cannot discriminate).
- **The gate** runs chunked, per CLAUDE.md — including the `--bins` chunk, which no `--test <name>`
  filter reaches.

## Risk register

1. **The struct-init hypothesis is wrong** — libpthread expects a kernel-populated struct. *Impact:*
   high; it is the difference between Stage 2b landing and re-parking. *Mitigation:* Task 1 measures
   it before any code is written, and a kill is a documented wall, not a prompt to guess.
2. **`dispatch_semaphore_signal` does not lower to a trap the seam models** — or does not trap at
   all on some path. *Mitigation:* Task 1 measures the actual signal path; the seam is keyed on what
   is measured, and anything else keeps hitting the guard and failing loud by value.
3. **The park/return opcode set is larger than one opcode.** *Mitigation:* already handled —
   `guest_workq_kernreturn` refuses unmeasured opcodes by value and names them. Expect at least one
   iteration here; it is a measurement round, not a defect.
4. **The worker needs guest state beyond stack and struct** — a mach thread port of its own, a TSD
   base, a kevent list. *Mitigation:* this is the M14 lesson restated ("emulating a syscall's entry
   contract is not the same as emulating the syscall"), and it is exactly what Task 1 exists to
   enumerate before Task 2 writes anything.
5. **One worker is not enough** — libdispatch requests more once the first parks. *Mitigation:* out
   of scope by decision above; if measured, it re-parks the gate with a named wall.

## Components

| Component | Crate | Change |
|---|---|---|
| `BlockReason::Sem { port }`, `unblock_sem_waiters_on` | `retrace-box` (`thread.rs`) | new variant + wake seam |
| `guest_workq_kernreturn` REQTHREADS arm | `retrace-box` (`lib.rs`) | panic → spawn |
| worker stack/struct allocation, `ThreadCtx` construction | `retrace-box` (`lib.rs`) | new |
| `guest_sem_wait` / `guest_sem_signal` | `retrace-box` (`lib.rs`) | new |
| record arms + replay mirrors for the seam | `retrace-core` (`lib.rs`) | new, paired |
| `verify_thread` sites 8 and 9 | `retrace-core` (`lib.rs`) | new, one per mirror |
| negative-trap forward guard | `retrace-core` (`lib.rs:531`) | new |
| `dispatch_e2e` gate + companion test | `retrace` (tests) | un-park or re-park; rewrite companion |
| README "Known limits", `docs/status-log.md` | docs | edit in place / append |

## Open questions for implementation planning

1. Does the `wqthread` entry contract read a **kevent list** argument, and does the fresh-thread path
   accept it empty? (Task 1.)
2. Does the worker need its own mach thread port written somewhere, as `bsdthread_create`'s child
   needed one at `pthread + 0xf8`? (Task 1 — and note that M14 discovered that requirement only
   because `pthread_join` silently succeeded without it, so the analogous failure here would be
   silent too.)
3. Is `TPIDRRO_EL0 = pthread + 0xe0` the same for a workqueue thread as for a `bsdthread_create`
   child? (Task 1; the offset is measured for the latter, 4/4 by host probe.)
4. Does `args[3] = 0x40008ff` need decoding at all, or does the entry contract ignore it? (Task 1.)
5. Where exactly does the guard for the seam's traps sit relative to the generic negative-trap arm —
   inside it as a leading assert, or as a preceding arm? (Implementation detail; either satisfies
   the fail-loud requirement, and the plan should pick one so both dispatch loops match.)
