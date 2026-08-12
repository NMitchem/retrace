# retrace M14-threads — rung 4, a guest with two threads of control

M10 named this milestone and three others went by first: *"Rung 4 — a guest that does substantial
work, or one that threads — is next. Rung 3 asked `jq` to read a file; it did not ask it to do
anything hard."* M11, M12 and M13 were each a detour into what a real guest tripped over — signals,
delivery, protection. The detours are done and none of them is wasted here: M13's `mach_vm_protect`
routing, billed in its own Status section as **dormant**, turns out to be a prerequisite for this
milestone, and M12's fault delivery is what a thread's guard page will eventually need.

M14 makes a stock `std::thread::spawn` + `join` Rust guest record and replay bit-for-bit.

## The problem, precisely

`Box_` has one vCPU and one implicit thread of control. The guest's registers *are* the vCPU's
registers; there is no notion of a thread that is not currently running. A guest that calls
`bsdthread_create` is asking for a second such context, and retrace has nowhere to put it.

The constraint that looks fatal is not. Hypervisor.framework allows one VM per process and retrace
creates one vCPU — but **a single vCPU is a gift to a deterministic replay engine, not an
obstacle.** Real threads on real cores are the classic source of replay nondeterminism. N guest
threads multiplexed onto one vCPU by a scheduler that is a pure function of recorded state are
deterministic by construction. M14 does not fight the single vCPU; it exploits it.

## Measured, on this host, at `main` = `c685695`, 2026-08-12

Everything in this section was run, not reasoned about.

### The registration half of threading already works

`RETRACE_TRACE=1 record-dyn` over `hello_rust` — a guest with **no** threads — shows:

| syscall | calls | status |
|---|---|---|
| `bsdthread_register` (366) | 1 | fires, survives, unremarked |
| `thread_selfid` (372) | 2 | fires, survives, unremarked |
| `bsdthread_create` (360) | 0 | never reached |
| `workq_open`/`workq_kernreturn` (367/368) | 0 | never reached |

libpthread hands the kernel its thread-start trampolines at startup on **every** dynamic guest
retrace has ever run, and retrace has been surviving that since M7 without anyone noticing. M14 is
therefore narrower than "implement threading": the registration is done, and the trampoline address
the kernel is supposed to enter a new thread at has already been handed over.

`semaphore_create` (msgh_id 3418) is likewise already on the forward allowlist.

### The walk dies at `bsdthread_create`, trap #258

A scratch `spawn`/`join` probe (`println!` in main, spawn a closure returning `42u32`, join, print
the value) built with the same `rustc --target aarch64-apple-darwin` recipe as `protrust`:

- **Natively:** prints `main before spawn` / `child ran` / `joined 42`, exit 0.
- **Under retrace:** prints `main before spawn`, then dies at trap #258 with **exit 133**
  (`Outcome::Signal`, `128 + SIGTRAP`), **printing no diagnostic of any kind**.

The last three traps, verbatim:

```
-15  mach_vm_map      args=[0x203, 0x27ff0f8, 0x20c000, 0x3fff, 0x1e000001, 0x3]
-14  mach_vm_protect  args=[0x203, 0x30000000, 0x4000, 0x0, 0x0, 0x3]
360  bsdthread_create args=[0x100024e00, 0x62180, 0x30207000, 0x30207000, 0x90008ff, 0x3]
```

Read in order: libpthread maps the new thread's stack (`0x20c000` ≈ 2.05 MiB), protects its guard
page `PROT_NONE` (`new_protection = 0` at `0x30000000`, one granule), then asks for the thread.

**That middle trap is M13's first real caller.** The M13 Status section says its `mach_vm_protect`
routing "is dormant… wired for the guest that eventually needs it, not for one that does today,"
having measured that `hello_rust` issues 47 such calls and never with `new_protection == 0`. The
guest that eventually needs it is this one, and it needs it one trap before the wall. M13 was a
prerequisite for M14 without either milestone knowing it.

### The `bsdthread_create` ABI, as this guest actually uses it

`bsdthread_create(func, func_arg, stack, pthread, flags)`:

| reg | value | meaning |
|---|---|---|
| x0 | `0x100024e00` | start routine, inside the guest binary's image |
| x1 | `0x62180` | argument |
| x2 | `0x30207000` | stack (guest-allocated, from the `mach_vm_map` above) |
| x3 | `0x30207000` | pthread struct |
| x4 | `0x90008ff` | flags |

The stack is guest-allocated: **M14 does not allocate thread stacks**, it accepts the one the guest
already mapped. That removes an entire class of IPA-placement work from this milestone.

### `BoxState` already defines a thread context

`crates/retrace-box/src/lib.rs:569` carries `regs`, `fp`, `fpcr`, `fpsr`, `tpidr_el0`, `elr`, `spsr`
— and M4's checkpoint tests already prove that set is sufficient to restore a vCPU **mid-run**, at an
arbitrary position, including the EL1 exception-return pair. A thread control block is largely that
register subset. M14 does not have to discover which registers constitute a context; M4 did it and
tested it.

**With one exception, and it is the interesting one.** `BoxState` does **not** carry `TPIDRRO_EL0`,
and its absence is correct today: the thread pointer is a constant (`TSD_IPA`, set identically in
`load_dynamic` and in restore), so there is nothing per-position to snapshot. Threads are exactly
what makes it vary. So the single register the existing context set omits is the single register M14
makes per-thread — see risk R2, which is the same fact wearing a different hat.

## Unmeasured — the plan's first task must measure this before any code is written

**What `pthread_join` blocks on is unknown**, because the walk dies before reaching it. Candidates
are `psynch_cvwait`, `__ulock_wait`, and a Mach `semaphore_wait`. `semaphore_create` already being
forwarded is a hint, not evidence.

This is load-bearing: the blocking primitive *defines* the scheduler's switch point, so designing
around a guess would put the whole milestone on sand. M13's `spikes/protnone.c` measured the
protection signal and **overturned the shipped `signal_of_esr` table** — a Linux-shaped guess that
had never been reached in six milestones. Task 1 here is the same move.

Also unmeasured and worth one cheap check: **whether `bsdthread_create` currently reaches the host
kernel.** If it does, the host may be creating a real thread inside retrace's own process starting at
a guest address. The silent SIGTRAP is consistent with either that or a libpthread internal abort.
Either way the fix is the same, but the hazard should be known rather than assumed benign.

## The mechanism

### M14-tcb — the thread table

`Box_` gains `threads: Vec<Thread>` and `current: usize`. A `Thread` holds the `BoxState` register
subset above — **plus `tpidrro_el0`, which that subset omits** — plus:

```
enum ThreadState { Runnable, Blocked(BlockReason), Exited(u64) }
```

Thread 0 is the main thread, created in `load_dynamic`. **A single-threaded guest has a one-entry
table and takes exactly today's path** — that is the compatibility argument for every gate M0–M13,
and it should be stated as such rather than hoped for.

### M14-create — `bsdthread_create` is emulated, never forwarded

Routed into the box like M13's `mach_vm_protect`, and for a stronger reason: forwarding is not
merely wrong, it is dangerous. The handler allocates a TCB, seeds its registers from the ABI
measured above, points its entry at the trampoline the guest registered via `bsdthread_register`,
gives it its own thread-pointer register, marks it `Runnable`, and returns to the caller **without
switching** — the real kernel does not switch either.

### M14-switch — the context switch already has a precedent

M9's `flush_guest_tlb` and the PAC signing oracle both save the vCPU's registers, run something else
on it, and restore. A context switch is that same save/restore with the restore aimed at a
*different* TCB. No new hardware interaction, and the save/restore discipline is already
gate-covered.

### M14-sched — the scheduler is a pure function

On a blocking stop or a thread exit, pick the lowest-indexed `Runnable` thread. Given the guest's own
syscall sequence the choice is forced, so record and replay produce identical schedules with nothing
recorded. Per **symmetry rule 2** it lives inside `Box_::run()`, below the trace, which is *why*
determinism is automatic here rather than argued for.

## Determinism posture

**Standard, and the trace format does not change.** No new `Event`, no `TRACE_MAGIC` bump. The second
thread's syscalls interleave into the same global event stream, in the same order on both runs,
because the same pure scheduler produced them. Replay's existing divergence oracle checks the
interleaved stream unmodified — an incorrect schedule on replay surfaces as a `(num, args)`
divergence at the first misordered syscall, which is the failure mode you want.

This is the M0 principle applied once more: cache pages, the timebase and PAC keys are all
*regenerated identically* rather than recorded, and the schedule now joins them.

## Fail-loud boundaries

- **Deadlock** — no `Runnable` thread — panics with the thread table dumped. Never hang.
- **`bsdthread_create` reaching the host** becomes impossible, with an assert saying so.
- **Unmodelled thread operations** (`workq_*`, thread priority, per-thread signal targeting) assert
  rather than returning a plausible answer. The M10 lesson stands: `fcntl(F_DUPFD)` is unmodelled and
  *not* fail-loud, and that is recorded as a wart, not a precedent.
- **Today's silent death is itself a defect.** retrace printed nothing when the guest hit
  `bsdthread_create`. Whatever else M14 does, that trap must announce itself.

## Scope

**In:** the thread table, emulated `bsdthread_create`/`bsdthread_terminate`, cooperative
block-driven scheduling, per-thread thread-pointer state, deadlock detection, and a `spawn`+`join`
full-`std` Rust headline.

**Out, deliberately:** per-thread seek and stepping; thread-aware watchpoints (M5's
reverse-continue-to-last-writer stays thread-agnostic); real preemption; `workq`/GCD thread pools;
and any claim about more than a handful of threads.

**The known limit:** cooperative scheduling means a guest that spin-waits without ever trapping runs
forever. `spawn`+`join` never does this — main blocks in `join`, which is the switch point — but a
guest that busy-waits on an atomic would hang. If that cannot be made to work within M14, it earns a
**new parked gate** for the capability, documented at its wall. It does not get quietly omitted.

## Exit criterion

`thread_rust_e2e` over a `spawn`+`join` guest, recording and replaying bit-for-bit, twice.

The gate asserts **`joined 42`** in stdout. That one line is the strong assertion: it can appear only
if the child thread genuinely ran *and* its return value crossed back through `join`. Plus a
`bsdthread_create` event in the trace and byte-identical replay.

**Exit 0 proves nothing on its own** — a guest that never spawned also exits 0. This is the same
discipline `segv_rust_e2e` established and `protnone_rust_e2e` sharpened: the exit code is necessary
and nowhere near sufficient, and the gate says so in its own comments.

## Testing

Cheapest first, so the expensive gates run against measured ground:

1. **Spike** — measure the `pthread_join` blocking primitive natively, before any box code.
2. **Pure unit tests** — the scheduler pick function and the thread-state transitions need no VM.
   Fast, exhaustive, and they run in the leaf-crate chunk of the gate.
3. **Box-level gates** — `bsdthread_create` builds a correct TCB; a switch save/restores every
   register in the set (assert the *hardware* side, not only the bookkeeping — M13's Task 8 defect
   was a test that checked only the software mirror and passed while the leaf disagreed); the
   deadlock detector fires.
4. **Headline** — `thread_rust_e2e`.

## Risk register

- **R1 — the join primitive is something the scheduler cannot cheaply observe.** Mitigated by
  measuring it first (Task 1). If it turns out to be a spin with no trap, M14's cooperative model
  cannot serve `join` and the milestone re-scopes rather than fakes it.
- **R2 — per-thread thread-pointer state collides with M2-cpuid.** `TPIDR_EL0` is **not** a second
  TSD pointer: macOS 26 reads the CPU number from its low bits, and M2-cpuid was a whole
  sub-milestone learning that. Per-thread state belongs in `TPIDRRO_EL0`; `TPIDR_EL0` must stay 0.
  This is the single most likely place for M14 to reintroduce a solved bug.
- **R3 — a switch mid-`mach_msg2` or mid-demand-page corrupts in-flight box state.** Switch points
  must be restricted to clean stop boundaries.
- **R4 — checkpoints silently lose threads.** `BoxState` captures one register context; with a
  thread table it must capture all of them, *and* gain the `tpidrro_el0` it has never needed, or
  M4's seeks restore a truncated process. Even though per-thread *debugging* is out of scope,
  checkpoint *correctness* is not. Note the failure shape: a checkpoint that drops the non-current
  threads still restores and still runs, so this breaks quietly rather than loudly — the M13 Task 8
  signature. Any gate for it must assert the restored thread table, not merely that the seek
  succeeded.

## Components

- `crates/retrace-box/src/lib.rs` — thread table, TCB, switch, scheduler, `bsdthread_create`.
- `crates/retrace-arch/src/lib.rs` — the thread syscall numbers.
- `crates/retrace-guest/rs/threadrust.rs` + `build.rs` — the headline guest.
- `crates/retrace/tests/thread_rust_e2e.rs` — the gate.
- `spikes/threadjoin.c` — the Task 1 measurement.

## Open questions for implementation planning

1. Does `bsdthread_terminate` carry the thread's return value, or does libpthread stash it in the
   pthread struct before trapping? Determines whether `join`'s result needs box involvement at all.
2. Does the child thread need its own signal disposition state (M11) or is that process-wide? M11
   modelled dispositions as process-wide, which is correct per POSIX, but the *mask* is per-thread.
3. Should thread 0 be constructed in `load_dynamic` or lazily on first `bsdthread_create`? The former
   is more uniform; the latter touches less M0–M13 code.
