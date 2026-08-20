# retrace M16-threadsignal — signals learn which thread they are for

M14 gave the guest threads. M15 gave the divergence oracle the ability to tell them apart at a
syscall landmark. **M16 gives the signal path the same thing**, because it does not have it: retrace
can today deliver a signal raised for one thread to a completely different one, silently, and no
gate can see it.

This is not only a coverage milestone. It closes a latent correctness defect in shipped code, splits
the signal state that POSIX makes per-thread away from the state it makes per-process, and discharges
the one standing fidelity caveat M15 shipped.

## The problem, precisely

`__pthread_kill(port, sig)` names a *target thread*. Retrace ignores the name and signals whoever is
running. With one thread that was true by construction; M14 made it false, and M15 documented the
consequence without fixing it.

Three separable defects hide behind that one sentence:

1. **No target resolution.** The port operand is not decoded, not validated, and not used.
2. **No non-current delivery.** `deliver_signal` reads registers off the live vCPU, so the frame is
   always built on the running thread's stack and the running thread is always the one redirected.
3. **No per-thread signal state.** The mask and the alternate stack live on a process-global
   `SigTable`. On macOS both are per-thread. A blocked signal is not merely mismodelled — it
   asserts, because M11 models no pending set.

The result is a guest for which `pthread_kill(child, SIGUSR1)` runs *main's* handler on *main's*
stack, and a `sigpending` that is contractually a lie.

## Measured, on this host, at `main` = `ed819c2`, 2026-08-19

### The target port is deliberately unvalidated, and the code says why

`crates/retrace-core/src/lib.rs:578-583`:

> `__pthread_kill`'s thread-port operand is NOT validated: 328 fires in no gate guest (measured: zero
> across hello_dyn/hello_rust/jq), so there is no observed port to compare against, and the guest has
> exactly one thread on one vCPU — any port it could name is that thread. Ungated rather than wrongly
> gated [...] Learn the port from `mach_thread_self` if a guest ever needs the check.

That reasoning was correct when written and is now false in its premise. M16 is the guest that needs
the check. The suggested fix — a `mach_thread_self` handler — turns out to be unnecessary; see
M16-port.

### The port→tid map already exists and is invertible

`crates/retrace-box/src/lib.rs:3180`, inside `guest_bsdthread_create`:

```rust
let port = GUEST_THREAD_PORT_BASE | tid as u32;
```

with `GUEST_THREAD_PORT_BASE = 0x0BAD_7000` (`:493`), written into the guest's own pthread struct at
`PTHREAD_KPORT_OFF = 0xf8` (`:468`). The TSD base sits at `PTHREAD_TSD_OFF = 0xe0` (`:474`) and the
kernel sets `TPIDRRO_EL0 = pthread + 0xe0`, so a thread's pthread struct is recoverable from its own
`tpidrro_el0` — which `ThreadCtx` carries per thread.

**Main is the exception, and it is the milestone's first measurement.** `ThreadTable::new` does not
go through `spawn`, so retrace never writes main's kport; libpthread's `__pthread_main_thread_init`
does, in userspace. Retrace has never read that field back. See risk R1.

### Delivery is hard-wired to the running thread

`crates/retrace-box/src/lib.rs:2812`:

```rust
pub fn deliver_signal(
    &mut self, sig: u64, si_code: u64, si_addr: u64, esr: u64, far: u64,
) -> (Vec<Region>, u64)
```

No target parameter. The body reads `x0..x28`, `FP`, `LR`, `SP_EL0`, `ELR_EL1`, `SPSR_EL1` straight
off the vCPU. `choose_frame_base` consults `self.sigtable.altstack()` (`:2735`), and the `sigreturn`
path restores the mask with `self.sigtable.set_mask(SIG_SETMASK, mask)` (`:2801`) — both
process-global.

### Mask and altstack are on `SigTable`; dispositions are too

`crates/retrace-box/src/sig.rs` exposes `is_blocked` (`:82`), `mask` (`:86`), `set_mask` (`:90`),
`altstack` (`:104`), `set_altstack` (`:108`) alongside `action`/`set_action`. POSIX makes
dispositions process-wide and the other two per-thread; retrace makes all five process-wide.

### A blocked signal asserts rather than pends

`crates/retrace-core/src/lib.rs:584-590` refuses to raise a blocked signal, naming the missing
pending set. `sigpending` (52) is serviced at `:528` with an always-empty answer, and the assert's
own text flags that the answer stops being true the moment a pending set exists.

### `Thread` has room; `ThreadCtx` does not

`crates/retrace-box/src/thread.rs` defines `ThreadCtx` as *"one thread's register context"* — the
`BoxState` register subset plus `tpidrro_el0` — and `Thread` as `{ ctx, state, stack }`. A signal
mask is not a register, so it belongs on `Thread`.

### Checkpointing survives the split for free

`BoxState` carries **both** `threads` (`crates/retrace-box/src/lib.rs:632`, cloned wholesale) and
`sigtable` (`:669`). Moving mask/pending/altstack onto `Thread` and leaving dispositions on
`SigTable` keeps every piece checkpointed with no new `BoxState` field.

### Only `Event::Syscall` carries a thread

`crates/retrace-trace/src/lib.rs`: `TRACE_MAGIC = RT\x00\x07` (`:45`). `Syscall` has `thread: u32`;
`Exit`, `Crash`, `Signal`, and `SignalDelivery` do not.

### The oracle's two signal-path arms have never seen two live threads

`ReplaySession::verify_thread`'s own doc names the gap: the caught-raise mirror and the `sigreturn`
mirror are reachable only by a guest that is *both* threaded and signalling, and none exists. M15
shipped them proven-to-fire, not proven-to-distinguish.

### The guest crate can build this without a new dependency

`crates/retrace-guest/Cargo.toml` depends only on `retrace-arch`; Rust guests are compiled by bare
`rustc`. A guest reaches `pthread_kill` through a small `extern "C"` block and obtains the child's
`pthread_t` from `std::os::unix::thread::JoinHandleExt::as_pthread_t()`, which is in std.

## The mechanism

### M16-split — per-thread signal state moves to `Thread`

`Thread` gains `mask: u32`, `pending: u32`, `altstack: Option<(u64, u64, u64)>`. `SigTable` loses
`mask`/`set_mask`/`is_blocked`/`altstack`/`set_altstack` and keeps the dispositions. Every M11/M12
call site retargets to the *relevant thread* — the caller for a mask operation, the target for a
delivery.

`ThreadTable::spawn` copies the creating thread's mask into the new `Thread`, by value. POSIX
inherits the mask at creation; modelling it is what makes the per-thread claim testable (see
Testing).

The compatibility argument is M14's, unchanged: a single-threaded guest has a one-entry table, so
thread 0 holds the mask and every M0–M15 path behaves identically.

### M16-port — one rule resolves a port to a thread, with no special case for main

A thread's kport is the value at `[its pthread + PTHREAD_KPORT_OFF]`, where its pthread is
`tpidrro_el0 - PTHREAD_TSD_OFF`. `Box_::thread_of_port(port)` walks the live threads, reads each
thread's field, and returns the match.

**Reading the field back rather than reconstructing the name is what removes main's special case**,
and it is why no `mach_thread_self` handler is needed: for a child the read returns the
`0x0BAD_7000 | tid` retrace wrote, and for main it returns whatever libpthread wrote. The current
thread's `tpidrro_el0` comes from the vCPU, a non-current thread's from its saved ctx — the same
split `dbg_regs_of` established in M15.

A port matching no live thread is a fail-loud panic naming the ports searched.

### M16-target — one delivery mechanism, parameterised by thread

`deliver_signal` gains a target tid and stops reading the vCPU directly. The sequence is
`save_ctx(current)` — so the table is authoritative for every thread including the running one —
build the frame from the target's `ThreadCtx`, rewrite that ctx to enter the handler, then
`load_ctx(current)`. A self-signal and a cross-thread signal take the identical path.

`complete_syscall_before_delivery` is applied **only when `target == current`**. M12 measured that a
self-raise's frame snapshots the post-return context (`x0 = 0`, `PSTATE.C` clear) because delivery
happens at a syscall boundary; a non-current target is not returning from a syscall, and applying the
completion would corrupt its `x0`.

**What a never-run target's context is.** Cooperative scheduling switches only on block or exit, so a
Runnable-but-not-current thread is either never-run or freshly woken. In the headline case it is
never-run: its ctx is the synthetic entry context `guest_bsdthread_create` built, so the frame lands
on the child's stack, its pc becomes the handler, and `resume_pc` is `thread_start_pc`. When first
scheduled it runs the handler, `sigreturn`s, and *then* starts its body. A real kernel starts the
thread and takes the signal at its first opportunity; **handler-before-body is a deliberate modelling
choice, not an accident** — see risk R3.

### M16-pending — every delivery is anchored to a syscall landmark

Delivery has exactly two anchor points, both syscalls, so both are visible to both dispatch loops and
neither requires anything to escape `Box_::run()`:

- **the `pthread_kill` landmark**, when the target's mask does not block the signal
- **the `sigprocmask`/`pthread_sigmask` landmark** that unblocks a pending one

The alternative — materialising a pending signal at the scheduler's switch point — was considered and
rejected: the reschedule check lives inside `run()`, below the trace, and `SignalDelivery` is a trace
event. Producing one from there needs a new channel out of `run()` or the scheduling decision
duplicated into both dispatch loops, which is precisely the argument M15 used to **delete**
`Event::Sched` rather than reserve it. Anchoring to syscalls keeps symmetry rule 1 applicable in its
ordinary form.

Dispatch at the `pthread_kill` landmark, after resolving the target:

| Condition | Landmarks appended | Effect |
|---|---|---|
| target's mask blocks `sig` | `Syscall` | set target's `pending` bit; return 0 |
| `Handler(h)` | `Syscall`, `SignalDelivery { thread: target }` | frame built into target |
| `Ign` / `Dfl`-ignore | `Syscall` | return 0 |
| `Dfl`-terminate | `Syscall`, `Signal { sig, pc, thread: current }` | terminal |

The mask arm applies the new mask to the **calling thread only**, then delivers the lowest-numbered
signal in `pending & ~mask`, appending `SignalDelivery` after that call's own `Syscall` landmark.
`sigpending` (52) stops returning a constant and reports the calling thread's set.

**A signal raised for a thread already redirected but not yet scheduled is fail-loud.** Stacking
frames without the kernel's queueing semantics would be a guess, and nested delivery is already on
M11's unmodelled list.

### M16-tag — the remaining landmarks carry a thread

`TRACE_MAGIC` moves `RT\x00\x07` → `RT\x00\x08` and `thread: u32` joins the other four variants:

| Variant | Meaning |
|---|---|
| `Exit` | the thread that called `exit` |
| `Crash` | the faulting thread |
| `Signal` | the **raising** thread |
| `SignalDelivery` | the **receiving** thread |

**The two signal variants tag opposite ends, deliberately.** `Signal` is terminal — nothing runs, so
"who received it" names a thread that never executes another instruction, while its existing `pc`
field is documented as naming the *raise site*. Tagging the raiser keeps `pc` and `thread` describing
the same event. `SignalDelivery` is not terminal and the receiver is the thread that goes on to run
the handler, which is the whole point of the milestone; its raiser is already tagged on the
`pthread_kill` `Syscall` landmark immediately preceding it, and on the fault path there is no raiser.
Getting this backwards is the easy mistake, so both arms assert the distinction in their tests. `verify_thread`
gains a call site at each of the four, placed after that site's own field comparison so a genuine
divergence still reports as itself.

## Determinism posture

Unchanged from M15, and this milestone adds nothing nondeterministic. Every new quantity is a pure
function of the guest's own syscall sequence:

- the target tid is derived from guest memory the guest itself can read
- the mask, pending set, and altstack are per-thread bookkeeping the guest sets through syscalls
- the frame bytes are recomputed on replay and byte-compared before being applied — M12's posture

The recorded `thread` on the four new variants is, like M15's, a recording of the *output* of a
function replay recomputes anyway: the standard symmetric posture. `verify_thread` compares and
returns a `Divergence`; it never sets the current thread.

No new record/replay asymmetry is introduced. M2-xpcport's minted-port exception remains the only one.

## Fail-loud boundaries

- a `pthread_kill` port matching no live thread — panic naming the ports searched
- **a target thread that is `Blocked(reason)`** — panic naming the reason. Redirecting it would
  overwrite the saved context its blocking syscall must resume through: a blocked thread's ctx *is*
  the resume point `__ulock_wait` owes a return value to. Added in Task 6's fix round, before any
  product caller could reach it, because the failure mode is silent corruption rather than a panic.
  **Task 13's parked `sigblocked_e2e` is precisely the guest that trips this arm** — measured, and
  the exact panic text is quoted in that gate's `#[ignore]` reason.
- **a target thread that is `Exited(code)`** — panic. There is no context left to resume through,
  so a frame built into it would never run. Same fix round, same argument.
- a second signal raised for a thread already redirected and not yet scheduled
- `sigwait` (330) and `sigsuspend` (111) — unchanged, still panic
- `kill` to a pid other than the guest's own — unchanged safety boundary
- a target thread whose pthread struct has no mapping at `+0xf8` — panic, matching
  `guest_bsdthread_create`'s existing treatment of the same field

## Scope

**In:** per-thread mask, pending set, and altstack; mask inheritance at spawn; port→tid resolution;
delivery into a non-current thread; pending materialisation at unmask; a truthful `sigpending`; thread
tags on the four remaining landmark variants; the oracle checking them; and a guest that is both
threaded and signalling.

**Out, and named so they are not later mistaken for oversights:** signalling a thread blocked in a
syscall (parked gate); signal queueing and nested delivery; `sigwait`/`sigsuspend`; asynchronous
signals from outside the process; per-thread *dispositions*, which are correctly process-global;
preemption; `workq`/GCD thread pools; thread priority; per-thread reverse execution as its own
position space.

M15's three named fast-follows stay out. They are unrelated to signals, and folding unrelated work
into an already-large milestone degrades the review surface for both.

## Exit criterion

A guest that spawns a thread, signals it by name from another thread, runs the handler on the *named*
thread, and separately takes a signal that was masked-then-pending-then-unmasked, **records and
replays bit-for-bit**, with the trace showing a `SignalDelivery` tagged with the receiving thread and
the oracle rejecting a retag of either previously-untested mirror.

## Testing

**The headline guest is `rs/sigthread.rs`**, and its ordering is the proof rather than a convenience:

1. install a SIGUSR1 handler; **spawn the child**, which inherits main's then-empty mask
2. **main masks SIGUSR1 for itself, after the spawn**
3. `pthread_kill(child, SIGUSR1)` — child is Runnable, not current, and unmasked
4. `join` — child is scheduled, runs the handler, `sigreturn`s, runs its body, exits
5. main `pthread_kill`s itself while still masked — the signal goes pending; `sigpending` reports it
6. main unblocks — the pending signal materialises at that landmark

Step 2's placement is load-bearing. **Were the mask still process-global, main's mask would suppress
the child's delivery** and the handler's output would vanish (or the existing blocked-signal assert
would fire). The per-thread claim is therefore proven by the guest's own observable ordering, not by
an assertion restating it.

**Gates:**

- `sigthread_e2e` — records and replays bit-for-bit; asserts on the **trace**: a `SignalDelivery`
  whose `thread` is 1, a `sigreturn` landmark tagged 1, and the second `SignalDelivery` anchored to
  the `pthread_sigmask` landmark tagged 0. Exit code 0 is what a guest that silently skipped every
  handler also produces, so per M6's rule it is not the assertion.
- two `thread_oracle` gates retagging the **caught-raise** and **`sigreturn`** mirrors to a live
  thread id and expecting a `Divergence`. Each must be mutated independently with the other staying
  green, per M15's Task 4 lesson — this is what discharges M15's standing caveat.
- VM-free unit tests in `thread.rs` for mask inheritance and pending-set arithmetic.
- a magic-bump pair, as M15 shipped: one test asserting the *new* magic, one asserting a trace
  written with the previous magic is rejected whole.
- **one new parked gate** — signalling a thread blocked in `__ulock_wait`, `#[ignore]`d with the wall
  stated.

Every claim about what a test catches is to be established by mutation, not by argument. M15 recorded
two cases where the plan's prediction was wrong and only measurement found it.

## Risk register

**R1 — main's kport has never been read back.** The no-special-case design assumes
`[main_pthread + 0xf8]` holds the name `pthread_mach_thread_np` returns. Measured for children
because retrace writes it; for main it is libpthread's write, cited but never verified. **Measure
before building on it.** Fallback: recognise `0x0BAD_7000 | tid` for children and fail loud on
anything else, which costs `pthread_kill(main, …)` and nothing the fixture needs.

**R2 — the frame builder may read more vCPU state than `ThreadCtx` carries.** `ThreadCtx` has
`fp`/`fpcr`/`fpsr`, so NEON is probably covered — but "probably" is how M13 shipped a test that
checked only the software mirror. Enumerate `build_frame`/`choose_frame_base`'s reads against the ctx
fields *before* the refactor.

**R3 — handler-before-body differs from a native run.** A real kernel starts the thread and takes the
signal at its first opportunity. The gate must therefore **not** compare against native output the way
`hello_dyn_e2e` compares against `"hi\n"`; the guest's stdout is asserted against retrace's own
recorded behaviour, replayed identically, which is the property the project claims.

**R4 — a second `TRACE_MAGIC` break in two milestones.** Accepted: it is cheaper now, while every
trace on disk is already dead from M15, than after recordings start being kept again.

**R5 — the task count is above this project's norm.** The refactor tasks (M16-split, M16-target) are
behaviour-preserving and reviewable as such, which is what keeps size from becoming risk.

If the count grows further during planning, **M16-tag is the seam to cut, not M16-pending.** The two
oracle gates that discharge M15's standing caveat consume `Event::Syscall`, whose `thread` field
already exists — so the caveat is dischargeable with no format change at all. Deferring M16-tag drops
the `TRACE_MAGIC` break (R4) along with it and costs only the tags on the four terminal-ish variants.
Deferring M16-pending would instead remove the per-thread mask and pending set, which is the thing
this milestone was chosen to build.

## Components

| Crate | Change |
|---|---|
| `retrace-trace` | `thread: u32` on `Exit`/`Crash`/`Signal`/`SignalDelivery`; `TRACE_MAGIC` → `RT\x00\x08` |
| `retrace-box` | `Thread` gains mask/pending/altstack; `SigTable` sheds them; `thread_of_port`; `deliver_signal` retargeted; `spawn` inherits the mask |
| `retrace-core` | `pthread_kill` arm resolves and dispatches by target; mask arm materialises pending; four new `verify_thread` sites; mirrors for each |
| `retrace-guest` | `rs/sigthread.rs` + its `build.rs` recipe |
| `retrace` | `sigthread_e2e`, the two oracle gates, the parked blocked-target gate; `where`/`threads` reporting unchanged but re-verified |

## Open questions for implementation planning

1. **R1's measurement is task one.** Everything in M16-port depends on its answer, and the fallback
   changes the shape of `thread_of_port`.
2. **Does `sigreturn` need to restore the mask per-thread from the frame?** `:2801` currently calls
   `sigtable.set_mask`; the frame carries the pre-delivery mask, and after M16-split that restore
   targets the returning thread. Confirm the frame layout M11 measured still carries it where
   expected.
3. **Ordering of the two `pthread_kill`s in the fixture.** Step 5 self-signals main while masked. If
   `sigpending`'s new answer proves awkward to assert from a bare-`rustc` guest, printing the mask
   word is an acceptable substitute — but it must remain an assertion on recorded behaviour.
4. **Should the debugger surface the receiving thread at a `SignalDelivery` landmark?** `where`
   already reports the box's live `current_thread()`, which at a delivery landmark is the receiver, so
   the answer may be "nothing to do" — but that needs checking rather than assuming, and if a debug
   line prints `SignalDelivery` today it should carry the tag.
5. **Whether `Exit`'s thread tag is worth its break on its own.** A process exits once; the tag is
   cheap and consistent, but if it proves to carry no assertion anywhere, say so in the close rather
   than pretending it earned its place.
