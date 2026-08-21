# retrace M18-workq — libdispatch reaches its worker pool

**Design spec — 2026-08-20**

Rung 5. A guest that calls `dispatch_async` onto a global concurrent queue records and replays
bit-for-bit, with the block running on a worker thread that retrace created and the trace's thread
tags proving it was not the main thread.

Every milestone since M14 has named "workq/GCD" in its out-of-scope list, in those two words, and
deferred it. This one takes it.

## The problem, precisely

The README's first Known Limit says there is no `workq_open` / `workq_kernreturn` handling, so
"programs that get concurrency through libdispatch — most real macOS applications — are not
supported." M14 measured both syscalls as firing **0 times, "never reached"**
(`docs/superpowers/specs/2026-08-12-retrace-m14-threads-design.md:37`), and that measurement was
taken from a *pthread* guest, which never had reason to reach them.

Nothing in the tree has ever pointed retrace at a libdispatch guest. So before this spec was written,
a throwaway probe did.

## What the probe measured

`dispatch_dyn.c` — `dispatch_semaphore_create`, `dispatch_async` onto
`dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0)`, `dispatch_semaphore_wait` on main —
built with the existing dynamic-guest recipe (`clang -arch arm64 -o <bin> <src>`, the `hello_dyn`
line at `crates/retrace-guest/build.rs:169-172`). Natively it prints `worker` / `done` and exits 0.

Under `RETRACE_TRACE=1 retrace record-dyn`, it loads through real `/usr/lib/dyld`, completes
libSystem initialisation, **reaches `main`**, and dies 405 traps in:

```
RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0xb001 FSC=0x1) pc=0x1804f5f20
```

`EC=0x3c` is a `BRK` — a deliberate trap, not a fault. Symbolised (retrace maps the shared cache at
**slide 0**, so guest addresses *are* unslid cache addresses, which is what made this cheap):

```
_dispatch_root_queue_poke_slow          libdispatch   0x180344744
 └ _dispatch_once_callout               libdispatch   0x180339630
    └ _dispatch_client_callout          libdispatch   0x1803504b0
       └ _dispatch_root_queues_init_once libdispatch  0x180348f64
          └ _pthread_workqueue_supported.cold.1  libsystem_pthread  0x1804f5f20
```

**`workq_open` (367) and `workq_kernreturn` (368) never fired — not once in 405 traps.** libdispatch
dies at the moment it asks *whether* workqueues are supported, before it ever uses one. The
workqueue syscalls are behind a gate that has never opened.

## The load-bearing claim, which is a READING and must become a MEASUREMENT

The chain below is read from disassembly of the host's own shared cache
(`dyld_info -arch arm64e -disassemble`). Every address is real and re-checkable. One link is still
an inference and Task 1 must close it.

**`_pthread_workqueue_supported`** (libsystem_pthread `+0x288C`) is four instructions:

```
ldr  w0, [x8, #0x1c]     ; __pthread_supported_features
cbz  w0, → .cold.1       ; ZERO → trap
ret
```

It traps **iff that global is zero**. The only writer, in `__pthread_init` (`+0x103C`):

```
bl   ___bsdthread_register
cmp  w0, #0x1
b.lt → 0x1060            ; return < 1 → SKIP the store entirely
mov  w8, #0x1e
movk w8, #0x4000, lsl #16    ; w8 = 0x4000001E
bics wzr, w8, w0             ; any required bit missing?
b.ne → 0x10d0                ; → a different crash
str  w0, [x8, #0x1c]         ; stored only on success
```

So `__pthread_supported_features` stays `0` exactly when `bsdthread_register` returns `< 1`.

retrace **forwards** `bsdthread_register` to the host kernel today
(`crates/retrace-core/src/lib.rs:815-820` record, `:1780-1784` replay — it captures `args[0]` as
`thread_start_pc`, then falls through to `forward_and_diff`). The host is retrace's *own* process,
which registered its own libpthread at startup, so the guest's call is a second registration.

**INFERENCE, NOT YET MEASURED:** that the host therefore returns `< 1`. The observed crash proves
the store was skipped, which proves the return was `< 1` **or** that the `bics` check diverted to
`0x10d0` — the two are distinguishable only by reading the actual returned value. Task 1 measures
it. This spec's mechanism is correct either way, but the *reason line* in the code comment must
record what was measured, not what was inferred.

**The latent hazard this accidentally avoided.** `bsdthread_register`'s `args[0]`/`args[1]` are
thread entry points. Forwarding it hands *guest addresses* to the host kernel as **retrace's own**
process's thread-start functions. It has been harmless only because it fails. Had it succeeded,
retrace's next real thread would have entered guest code — the same whole-process-fatal class as
forwarding `bsdthread_create` (`crates/retrace-box/src/lib.rs:3293`). M18 stops forwarding it, which
closes that hazard as a side effect and should be said out loud where the arm changes.

## The design lever the disassembly handed us

The feature word is not merely something to repair — **it is retrace's to choose, and it selects how
much workqueue surface must be emulated.** Two consumers constrain it.

libpthread (above) requires `w0 >= 1` and `(w0 & 0x4000001E) == 0x4000001E`.

libdispatch, in `_dispatch_root_queues_init_once` (`0x180348F3C`):

```
0x180348F60  bl   __pthread_workqueue_supported
0x180348F68  tbz  w0, #0x4,  → 0x18034904C   ; bit 4 clear            → .cold.5
0x180348F90  tbnz w19, #0x7, → 0x180348FBC   ; bit 7 SET → register THREE callbacks
0x180348F94  tbz  w19, #0x6, → 0x18034905C   ; neither 7 nor 6        → .cold.4
0x180348FF4  bl   _pthread_workqueue_setup
0x180348FF8  cbnz w0,        → 0x180349054   ; setup failed           → .cold.2
             bl   _sysctlbyname
             cbnz w0,        → 0x180349064   ; sysctl failed          → .cold.3
```

With bit 7 **set**, libdispatch registers `_dispatch_worker_thread2`, `_dispatch_kevent_worker_thread`
*and* `_dispatch_workloop_worker_thread`. With bit 7 **clear** and bit 6 set, it registers only the
first two — the workloop worker never enters the picture.

**The minimal legal word is therefore `0x4000005E`** (`0x4000001E` for libpthread, `| 0x40` for
libdispatch's bit 6, bit 7 deliberately clear). Choosing it is a scope decision made by measurement
rather than by guess, and it is the smallest surface that gets past both gates.

## Approach B is dead, and this is why it is worth recording

The obvious cheap milestone would have been to push libdispatch onto a non-workqueue fallback and
reuse the `pthread_create` machinery M14–M17 already proved. **That fallback does not exist for the
global root queues.** Every exit from the workqueue path above is a `.cold.N` crash stub; there is no
branch to a pool initialiser. `__dispatch_worker_thread` *is* present in the binary, but it belongs
to `_dispatch_pthread_root_queue_create` — the public API for *user-created* root queues — not the
global ones.

Making `_pthread_workqueue_supported` answer "unsupported" does not buy a fallback. It buys
`.cold.5`. On macOS 26 the global concurrent queues have exactly one implementation and it is the
kernel workqueue.

Recorded here because the cost of learning it was one probe, and the cost of learning it later would
have been a half-built milestone.

## The mechanism

Two stages. The second cannot be fully specified until the first lands, and this spec says so rather
than inventing it.

**Stage 1 — the guest's registration is the guest's.** `bsdthread_register` stops being forwarded and
becomes emulated in the box, joining `bsdthread_create`/`bsdthread_terminate`/`ulock_*`. It:

- captures `args[0]` as `thread_start_pc` (already done today),
- **additionally captures `args[1]` as the workqueue thread entry point** (`wqthread`) — the address
  the kernel enters when it hands a worker to userspace, which Stage 2 needs and which retrace
  currently discards, and `args[2]` as the pthread struct size,
- returns the synthesized constant feature word, never the host's.

This is the fourth instance of one recurring bug: the guest's fds were retrace's fds (M10), the
guest's signal dispositions were retrace's (M11), the guest's pthread registration is retrace's
(here). Naming it that way in the code comment is the point.

**Stage 2 — the workqueue itself.** `workq_open` (367) and `workq_kernreturn` (368) get syscall
constants in `retrace-arch` (neither exists there today) and emulated arms in both dispatch loops.
A worker that parks in `workq_kernreturn` needs a new `BlockReason` variant: unlike
`BlockReason::Wait { addr }`, a parked worker has **no guest address to correlate on** — the kernel,
not another thread, decides when it resumes. That makes the existing address-equality wake
(`crates/retrace-box/src/thread.rs:312-323`) inapplicable, and the wake source becomes the guest's
own enqueue, which is what keeps the whole thing a pure function of the guest's syscall sequence.

**Worker threads are structurally unlike `bsdthread_create` threads.** There, the *guest* allocates
the pthread struct and stack and passes them in; `guest_bsdthread_create` only fills in what the
kernel would have written. For a workqueue thread the **kernel** allocates the stack and the pthread
struct and enters `wqthread` with a specific register contract. Retrace must therefore do more, not
less, than it does for `bsdthread_create`: allocate guest stack, construct the pthread struct, set
`TPIDRRO_EL0`, and enter at the captured `wqthread`. The exact register contract is unmeasured —
Task 2 measures it the way M14 Task 2 measured `__pthread_start`'s, by disassembly plus a live run.

## Determinism posture

**Standard and symmetric** — the M2-setport posture, not the M2-xpcport asymmetry. Every value M18
introduces is either a fixed constant (the feature word) or a pure function of the guest's own
syscall sequence (which worker is parked, which is woken, which runs next). Record appends the
landmark; replay recomputes it with the same `Box_` method and the same arguments and byte-compares.
Nothing nondeterministic enters the trace, and no scheduling data is recorded — the M14 argument,
unchanged.

`Event` gains no variant and `TRACE_MAGIC` stays `RT\x00\x08`. If Stage 2 discovers a landmark that
genuinely cannot be recomputed, that is a format break and a re-plan, not a quiet addition.

**The oracle must grow with the mirrors.** `verify_thread` has exactly seven call sites plus
`mirror_delivery`'s eighth inline check (`CLAUDE.md:224-230`). Every new replay mirror that `return`s
before the generic dispatch **silently creates a new hole until its oracle call is added** — nothing
structural couples the two. M18 adds at least two such mirrors (`workq_open`, `workq_kernreturn`) and
possibly a third if `bsdthread_register`'s arm starts returning early. Each needs its own
`verify_thread`, placed *after* that arm's own field comparison. The census in `CLAUDE.md` is updated
in the same commit as the last mirror, not at the close.

## Fail-loud boundaries

- **Every unmeasured `workq_kernreturn` opcode asserts.** The interface is large and stateful; M18
  implements only the opcodes this guest issues and refuses the rest by name and value. This is the
  `guest_ulock_wake` posture (`crates/retrace-box/src/lib.rs:3588`), which asserts on any operation
  word it did not measure.
- **`workq_open` before `bsdthread_register`** asserts, mirroring `guest_bsdthread_create`'s
  `thread_start_pc.expect(...)`.
- **A feature word that is not the synthesized constant** — i.e. any future edit that lets the host's
  value through — asserts, because the constant is what both gates were measured against.
- **No worker to run queued work** is a deadlock, and `schedule_after_block`'s existing panic already
  reports it with the full thread-state dump.

## Scope

**In:** `bsdthread_register` emulation and the synthesized feature word; capture of `wqthread` and
pthread size; `workq_open`/`workq_kernreturn` constants and emulated arms scoped to the opcodes the
probe guest issues; a worker-park `BlockReason` and its wake; worker-thread construction; the new
oracle sites; the `dispatch_async` end-to-end gate.

**Out:** the kevent/kqueue worker path (bit 7 clear keeps the workloop worker out; the kevent
callback is *registered* but is expected never to be entered by this guest — if it is, that is a
measured wall and a parked gate, not a silent extension); QoS and thread priority; `dispatch_sync`
semantics beyond what this guest exercises; dispatch sources and timers; `dispatch_pthread_root_queue`;
preemption; per-thread reverse execution; everything M17 and earlier carry forward.

## Exit criterion

`dispatch_e2e` records and replays a `dispatch_async` guest bit-for-bit, twice, and asserts **on the
difference M18 makes** — per the honest-gate rule that an assertion a weaker failure would also pass
is not an assertion:

- the block's `write(1, "worker\n", 7)` appears in the trace on a thread whose tag is **not** the
  main thread's, which is the whole claim and which no pre-M18 build can produce;
- at least one workq syscall appears as a landmark at all — `workq_kernreturn` certainly, and
  `workq_open` only if macOS 26 still issues it (open question 4; the assertion names whichever the
  measurement finds, and must not require a call the OS no longer makes);
- the run reaches `Event::Exit` with code 0, and replay is byte-identical.

If Stage 2 hits a wall that this milestone cannot clear, the gate is parked `#[ignore]`d with the
wall documented on the test, the README's Known Limits, and the new status-log section — and the
milestone still ships Stage 1, which is independently verifiable and independently valuable.

## Testing

- Unit tests in `retrace-box` for the feature-word constant against **both** measured gates
  (`(w & 0x4000001E) == 0x4000001E`, `w >= 1`, bit 6 set, bit 7 clear) — the constant's whole job is
  to satisfy four bit-tests read from disassembly, so the test asserts those four, not the literal.
- `thread.rs` unit tests for the worker-park `BlockReason`: park, wake, and the scheduler's choice,
  in the style of the existing `ThreadTable` tests.
- A wrong-thread divergence test in the M15/M16/M17 lineage: retag a workq landmark to the wrong
  thread and assert replay reports a divergence — this is what proves the new oracle sites are real
  rather than decorative.
- `dispatch_e2e` as above. It is a repo artifact, so it never skips.

## Risk register

1. **The host's `bsdthread_register` return value is inferred, not measured.** *Mitigation:* Task 1
   measures it before any code changes; the comment records the measurement. Low risk to the design
   (the mechanism is the same either way), high risk to the *documentation* being another false
   claim of the kind M17 Task 9 had to correct three times.
2. **`workq_kernreturn`'s opcode surface is unknown and may be an M2-style chain.** Nothing has ever
   reached it. *Mitigation:* fail loud per opcode; park the gate honestly at the first unmeasured
   one; a milestone that parks a new gate for a capability it does not yet have has regressed
   nothing.
3. **The worker entry contract is unmeasured.** M14's lesson — "emulating a syscall's entry contract
   is not the same as emulating the syscall" — applies with more force here, because the kernel does
   more for a workqueue thread than for a `bsdthread_create` one. *Mitigation:* Task 2 measures it by
   disassembly of the registered `wqthread` plus a live run, exactly as M14 Task 2 did.
4. **The kevent worker may be entered.** libdispatch registers it even with bit 7 clear.
   *Mitigation:* assert on entry; if it fires, that is the measured wall and the gate parks there.
5. **A cooperative scheduler may not satisfy libdispatch's progress assumptions.** libdispatch may
   spin or assert if a worker does not appear to make progress. *Mitigation:* unknown until Stage 1
   lands; named here so it is not a surprise.

## Components

| Component | Crate | Change |
|---|---|---|
| `SYS_WORKQ_OPEN` (367), `SYS_WORKQ_KERNRETURN` (368) | `retrace-arch` | new constants — neither exists today |
| feature-word constant | `retrace-box` | new, with its four measured bit-tests |
| `guest_bsdthread_register` | `retrace-box` | new; captures `wqthread`, returns the constant |
| `guest_workq_open` / `guest_workq_kernreturn` | `retrace-box` | new |
| worker-park `BlockReason` + wake | `retrace-box::thread` | new variant; `pick_next` unchanged |
| worker thread construction | `retrace-box` | new; more than `guest_bsdthread_create` does |
| record arms | `retrace-core::record_box` | above the generic arm at `lib.rs:966` |
| replay mirrors + `verify_thread` | `retrace-core::ReplaySession::advance` | one oracle call per early-returning mirror |
| `dispatch_dyn.c` + build rule | `retrace-guest` | new guest, `hello_dyn` recipe |
| `dispatch_e2e` | `retrace` | the headline gate |
| oracle census | `CLAUDE.md` | updated with the last mirror, not at the close |

## Open questions for implementation planning

1. What does the host's `bsdthread_register` actually return for the guest's call? (Task 1; R1.)
2. What is the register contract at the registered `wqthread` entry point? (Task 2; R3.)
3. Which `workq_kernreturn` opcodes does this guest issue, and in what order? (Task 2; R2.)
4. Does `workq_open` fire at all on macOS 26, or has `pthread_workqueue_setup` replaced it?
5. Does the parked-worker wake need to distinguish "work available" from "thread requested", or does
   the guest's enqueue sequence make that distinction unnecessary — the way `pthread + 0x34` address
   equality made an address→thread map unnecessary in M14?
