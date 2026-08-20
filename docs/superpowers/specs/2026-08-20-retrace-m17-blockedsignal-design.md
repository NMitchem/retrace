# retrace M17-blockedsignal — a signal reaches a thread that is blocked

M16 gave signals a thread identity: `pthread_kill(child, sig)` runs the handler on the child, and
every landmark carries a thread tag the divergence oracle checks. It stopped at one boundary, and
parked a gate there honestly: a target that is **blocked in `__ulock_wait`** rather than merely
not-current. M17 removes that boundary.

## The problem, precisely

`deliver_signal_to` builds the handler frame into the target thread's **saved context**. For a
`Runnable` thread that context is an ordinary suspension point and redirecting it is safe — M16
proved that, and `sigthread_e2e` gates it. For a thread blocked in `__ulock_wait`, M16 judged the
saved context to be the resume point its own blocking syscall owes a return through, and refused:

```
thread 1 is Blocked(Wait { addr: 809578548 }), not Runnable; deliver_signal_to would overwrite
the saved context its blocking syscall must resume through. Wake or skip it instead of
redirecting a thread that cannot run yet.
```

That guard is deliberate and correct as a *refusal*. It is not a claim that the case is hard — M16
chose to fail loud rather than reason it through, which is the right posture for a boundary it was
not funding. M17 funds it.

`sigblocked_e2e` is already committed, parked at exactly this panic, with a real three-thread guest
(`crates/retrace-guest/rs/sigblocked.rs`) behind it. Its assertions already encode the target
behaviour and were written correct-by-construction from Task 13's measurement. **This spec's exit
criterion is that gate coming green unmodified.**

## The load-bearing claim, which is a READING and must become a MEASUREMENT

Everything below rests on one fact:

> A `Wait`-blocked thread's saved context is a **complete post-syscall state** — `x0` already holds
> `__ulock_wait`'s return value and the pc is already past the `svc`.

The ordering in record's arm (`crates/retrace-core/src/lib.rs:865-870`) says so: `guest_ulock_wait`
marks the thread `Blocked`, and only *then* does `set_x0_err_and_return(rc, false)` complete the
syscall on the live vCPU; the switch that saves that context happens on the next `run()`.

If the claim holds, a signal frame built on that context **preserves** the resume point rather than
overwriting it — the frame saves what it displaces and `sigreturn` restores it — and the wall is
narrower than its own `#[ignore]` text describes. If the claim is FALSE, this design changes shape:
the materialisation site would first need the equivalent of `complete_syscall_before_delivery`
applied to a *saved* context rather than to the live vCPU, and that becomes a task of its own.

**Task 1 measures this before any other task begins.** Do not build on the reading.

## The mechanism

**Pend, then materialise at the wake.** Chosen over delivering immediately into a blocked thread's
context, and over interrupting the wait with `EINTR`. The alternatives and why they lost are in
"Approaches considered".

1. **Widen the pend condition.** The raise path today pends when the target's *mask* blocks the
   signal and delivers otherwise. It gains a second reason to pend: the target is not `Runnable`.
   Per-thread pending sets already exist (`Thread.pending`, M16), and `take_deliverable` already
   respects the mask, so a signal pended for BOTH reasons is released only when both clear —
   correct with no extra bookkeeping.

2. **Materialise when a wake makes the thread runnable.** `guest_ulock_wake` → `unblock_waiters_on`
   is the only site that wakes a `Wait`-blocked thread. `BlockReason::Join` is a variant nothing
   produces today — measured in M16 Task 13 and recorded in the guest's own comment — so there is
   exactly ONE materialisation site, not two. If `Join` ever gains a producer, this becomes two and
   the spec is wrong; a fail-loud assert should make that discoverable rather than silent.

3. **The delivery lands on the woken thread.** `deliver_signal_to(woken_tid, …)` frames that
   thread's completed post-wait context. It then runs the handler when scheduled and `sigreturn`s
   back into the wait's return. The `SignalDelivery` landmark is tagged with the **receiving**
   thread, as M16 established — not the waker whose syscall triggered it.

## The structural cost: landmark arithmetic

This is the part most likely to be got wrong, so it is specified rather than left to discovery.

Replay handles `SYS_ULOCK_WAKE` today as a **hook** inside `advance`'s generic dispatch block
(`crates/retrace-core/src/lib.rs:1777`). That block ends in `finish_event()`, which consumes exactly
**ONE** landmark. A materialising wake appends **TWO** — the ordinary `Syscall`, then the
`SignalDelivery`.

Therefore replay's wake handling **must be hoisted into its own dispatch arm**, exactly as the
unmasking `sigprocmask` arm already was, for the reason documented at `:1325`. Leaving it a hook
would silently renumber every subsequent landmark.

This mirrors the rule CLAUDE.md states for the *record* side and the sigpending comment states for
the replay side: an arm that materialises nothing may stay a hook; an arm that materialises a
landmark may not.

## The oracle hole the hoist creates

M16's status entry is explicit, and it applies directly here:

> every one of those sites exists because a mirror was found that `return`s *before* reaching the
> generic dispatch, so **each new mirror silently creates a new hole until someone remembers to add
> its oracle call**. Nothing structural couples "add a mirror" to "add its `verify_thread`".

The hoisted wake arm `return`s before the generic dispatch. **It therefore needs its own
`verify_thread` call**, placed *after* that arm's own field comparison so a genuine argument
divergence still reports as itself.

Count goes from **seven** `verify_thread` call sites to **eight**, and from eight to **nine** places
the oracle compares a thread (the ninth remains `mirror_delivery`'s inline `rthread != tid` check,
which is deliberately not a `verify_thread` call because its tag names the receiving thread). Any
task that changes this count updates CLAUDE.md's paragraph, which has drifted three times before.

## Determinism posture

Standard and symmetric. Both dispatch loops call the same `Box_` methods with the same arguments and
materialise through the same shared helper (`take_deliverable`'s product caller at
`crates/retrace-core/src/lib.rs:93`), which exists precisely so the two sides cannot drift on *which*
signal is materialised. Replay's `mirror_delivery` byte-compares the recomputed frame against the
recorded one — that comparison IS the divergence check.

Nothing new enters the trace. The pend/wake decision is a pure function of the guest's own syscall
sequence, so record and replay produce identical schedules with nothing recorded — the same argument
that keeps `bsdthread_create` below the trace.

**No trace-format change.** `SignalDelivery` already exists and already carries a thread tag.
`TRACE_MAGIC` stays `RT\x00\x08`. A task that proposes bumping it has misunderstood something.

## Fail-loud boundaries

- **A signal pended on a thread nothing ever wakes is never delivered.** This is a real divergence
  from POSIX, which would interrupt the wait. At guest exit, if any thread is still `Blocked` with a
  nonzero pending set, **fail loud naming the thread and the signal** rather than exiting 0 and
  silently swallowing it. A silently-lost signal is exactly the class of defect this project treats
  as worse than a crash.
- **`BlockReason::Join` gaining a producer** would add a second materialisation site this design does
  not cover. Assert rather than assume.
- The existing `deliver_signal_to` `Exited` arm stays a panic. A signal to a dead thread is not a
  schedule divergence; it is a modelling bug, and nothing in this milestone makes it reachable.
- `guest_ulock_wait` / `guest_ulock_wake`'s operation-word asserts are untouched. M17 changes who
  gets woken up to what, never which operation words are modelled.

## Approaches considered

**Deliver immediately into the blocked thread's context** — build the frame at raise time, leave the
thread `Blocked`, let it run the handler when woken. Closer to POSIX ordering and the delivery
landmark sits where the raise happened. Rejected: it depends entirely on the load-bearing claim
above holding, and it leaves a thread simultaneously `Blocked` and `redirected` — a state
combination nothing in the box models, which `switch_to`'s redirected-flag discipline would have to
grow to understand.

**Interrupt the wait with `EINTR`** — wake the thread out of the wait, deliver, return `EINTR` so
libpthread's retry loop re-issues it. Most faithful to the real kernel. Rejected as
disproportionate: it changes a guest-visible syscall return value, so it needs `__pthread_join`'s
retry loop measured by disassembly, and `__ulock_wait`'s `ULF_NO_ERRNO` convention (`-errno` with
`PSTATE.C` clear) which the current arm hardcodes as `err: false`. That is a milestone of its own,
and nothing in the tree needs it yet.

**Pend until wake** — chosen. It reuses M16's pending machinery and its materialise-at-a-landmark
pattern verbatim, needs no new trace concept, no scheduler change, and no new thread state. Its cost
is the semantic gap named under fail-loud boundaries, which is guarded rather than hidden.

## Scope

**In:** the widened pend condition; materialisation at the wake; the hoist of replay's `ULOCK_WAKE`
handling into its own arm; that arm's `verify_thread` call; the exit-time pending-signal guard;
un-parking `sigblocked_e2e`.

**Out:** `EINTR`/wait-interruption semantics. `BlockReason::Join`. Signals to `Exited` threads. The
`Crash` landmark's still-unexercised `verify_thread` site — a real gap, unrelated to this one, which
needs a threaded guest that crashes. GCD/libdispatch. Preemptive scheduling.

## Exit criterion

`sigblocked_e2e` passes **with its assertions unmodified**, un-`#[ignore]`d, and the README's
"Known limits" loses its `sigblocked_e2e` bullet. The whole gate must come green, including the
replay half — record-only is not this milestone.

A fix that makes the gate green by *skipping* the blocked target does not count: the guest's handler
is empty, so a skipped delivery exits 0 and changes no stdout. The gate already rejects this by
asserting on the trace (`delivered == vec![1u32]`), which is why it was written that way.

## Testing

- **`sigblocked_e2e`, un-parked.** The headline. Assertions unmodified.
- **A mutation test for the new oracle site.** Retag the wake landmark's thread tag to another live
  id and assert replay reports a divergence naming the schedule. Without it the hoisted arm's
  `verify_thread` is an untested hole — the exact failure mode M16's own census kept re-creating.
  Follow `thread_oracle.rs`'s existing shape, and pin the message, not just exit code 3.
- **Unit tests** for the widened pend condition (a `Blocked` target pends rather than delivering)
  and for materialise-at-wake (a wake on a thread with a deliverable pending signal produces one).
- **Mutation over argument.** Every claim that a test catches something is established by making the
  mutation and watching it fail. A test that has never been seen red has not been tested.

## Risk register

- **R1 — the load-bearing claim is false.** Mitigation: Task 1 measures it first and the design is
  re-shaped before anything is built on it. Impact: high, cost of detection: one task.
- **R2 — the hoist renumbers landmarks.** A hook that appends two landmarks corrupts every
  subsequent index. Mitigation: the hoist is specified above, not discovered; and the full e2e suite
  is the detector, since every existing threaded gate would fail loudly.
- **R3 — the new arm's oracle hole is forgotten.** Mitigation: named as its own deliverable with its
  own mutation test, and the seven→eight count is stated so a reviewer can check it by grep.
- **R4 — the guest deadlocks instead of delivering.** If the pend-until-wake ordering is wrong, `a`
  waits forever and the gate hangs rather than failing. Mitigation: the exit-time guard, plus
  `RETRACE_TRACE=1` on the record run as the first diagnostic.

## Components

- `crates/retrace-box/src/thread.rs` — no new state expected; `pend`/`take_deliverable` already
  suffice. If a task finds it needs a new `Thread` field, that is a signal the design is wrong.
- `crates/retrace-box/src/lib.rs` — the widened pend decision, the wake-site materialisation hook,
  the exit-time guard.
- `crates/retrace-core/src/lib.rs` — record's wake arm materialises; replay's wake handling is
  hoisted into its own arm with its own `verify_thread`.
- `crates/retrace/tests/sigblocked_e2e.rs` — un-parked, assertions untouched.
- `crates/retrace/tests/thread_oracle.rs` — the new site's mutation test.
- `README.md` (edited in place: "Known limits", the gate line) and `docs/status-log.md` (a new
  appended section — never a rewrite of an old one).

## Open questions for implementation planning

1. Does the materialisation belong inside `guest_ulock_wake` (below the trace, so both loops get it
   for free) or in the two dispatch arms (above the trace, explicit and symmetric)? The
   `SignalDelivery` is a **landmark**, so it cannot live below the trace — but the *decision* of
   which signal to materialise can. Task 1 should settle where the seam sits.
2. Should the exit-time guard fire on any `Blocked` thread with pending signals, or only when the
   guest exits 0? A guest already crashing should probably not have this guard fire on top.
3. If a wake makes SEVERAL threads runnable and more than one has a deliverable signal, the wake
   landmark would materialise several deliveries. `unblock_waiters_on` returns a count, so this is
   expressible — but no fixture produces it. Model it or assert on it, and say which.
