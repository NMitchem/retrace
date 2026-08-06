# retrace M11-signals — the guest's signal dispositions are its own

Since M6 the README has carried the same top deferred item, unchanged through four milestones: a
signal the guest raises on itself is forwarded to the host and kills the recorder. M11 closes it the
way M10 closed the fd gap — by giving the box a real, guest-visible table and never forwarding the
syscalls that touch it.

M11 models **disposition**, not **delivery**. An uncaught fatal signal becomes a recorded terminal
event, replayable bit-for-bit, in the exact shape M6 gave a fault. A signal with a handler installed
raises a loud assert rather than a plausible lie. Running the handler — signal frames, the
`__sigtramp` ABI, `sigreturn` — is deliberately out of scope and named as the next milestone.

## The problem, precisely

`forward_and_diff` (`crates/retrace-box/src/lib.rs:2248`) issues the guest's syscall through a raw
`svc` **in retrace's own process**. No signal syscall is special-cased anywhere, so every one of them
falls through the generic arm at `crates/retrace-core/src/lib.rs:441` and executes against retrace.
Three consequences, in increasing order of severity:

1. **`__pthread_kill(self, SIGABRT)` kills the recorder.** The host process takes a real `SIGABRT`
   and dies with exit 134. The trace ends with no terminal event, so replay diverges at the last
   landmark with `expected recorded syscall, got None (truncated=false)`. A guest that aborts cannot
   be recorded at all.
2. **`sigaction` installs the guest's handler in retrace's signal table.** A guest that registers a
   `SIGSEGV` handler is registering a *guest virtual address* as the fault handler of the recorder
   process. Nothing currently prevents this; it has simply never been exercised.
3. **`kill(pid, sig)` would signal a real host process.** The operand is untranslated and unchecked,
   so a guest naming any pid reaches it. This is the only defect in this area that escapes the
   sandbox, and it is unguarded today.

M6 recorded this as a scope boundary rather than a bug — "the guest's `sigaction` handlers never run
— a fault is terminal, matching rr's default disposition for fatal signals"
(`docs/superpowers/specs/2026-07-19-retrace-m6-crash-design.md:198`). That framing covers (1)'s
*delivery* half honestly. It does not cover (2) or (3), which are guest state escaping into retrace's
process — the same defect class M9 and M10 fixed for descriptors.

## Verified facts (this host, HEAD `809c789`, 2026-08-05)

Resolved from `$(xcrun --show-sdk-path)/usr/include/sys/syscall.h` and `sys/signal.h`, not from
memory, per the M10 rule:

| num | name | disposition in M11 |
|-----|------|--------------------|
| 20  | `getpid`                 | unchanged (forwards) |
| 37  | `kill`                   | serviced; assert unless target is self |
| 46  | `sigaction`              | serviced against `SigTable` |
| 48  | `sigprocmask`            | serviced against `SigTable` |
| 52  | `sigpending`             | serviced — always empty (see below) |
| 53  | `sigaltstack`            | serviced; stored, not honoured |
| 111 | `sigsuspend`             | assert (blocking wait) |
| 184 | `sigreturn`              | assert (unreachable by construction) |
| 328 | `__pthread_kill`         | serviced; the raise path |
| 329 | `__pthread_sigmask`      | serviced against `SigTable` |
| 330 | `__sigwait`              | assert (blocking wait) |
| 333 | `__pthread_canceled`     | out of scope (not signal disposition) |
| 520 | `terminate_with_payload` | assert (unmodelled terminal path) |
| 521 | `abort_with_payload`     | assert (unmodelled terminal path) |

`NSIG` = 32 (`sys/signal.h:76`, "counting 0; could be 33 (mask is 1-32)"), `SIGABRT` = 6,
`SIG_DFL` = 0, `SIG_IGN` = 1.

**The `_nocancel` pairing rule yields nothing here.** M10's transferable lesson is to table the
`_nocancel` variant beside its plain form as a pair. Checked: the only `_nocancel` signal syscalls
are `sigsuspend_nocancel`(410) and `__sigwait_nocancel`(422). Both pair with calls M11 asserts on
anyway, so there is no silent-fallthrough twin for any *serviced* call. Stated explicitly so the next
reader does not have to re-derive it.

**`sigaction`'s in-param and out-param are different types.** From `sys/signal.h:277` and `:287`:

```c
struct __sigaction {                 /* the ACT argument — 24 bytes on arm64 */
        union __sigaction_u __sigaction_u;   /* 8 */
        void  (*sa_tramp)(...);              /* 8  <-- present only here */
        sigset_t sa_mask;                    /* 4 */
        int      sa_flags;                   /* 4 */
};
struct sigaction {                   /* the OLDACT writeback — 16 bytes */
        union __sigaction_u __sigaction_u;   /* 8 */
        sigset_t sa_mask;                    /* 4 */
        int      sa_flags;                   /* 4 */
};
```

Synthesizing the `oldact` write with the input layout would write 24 bytes where the guest expects
16, corrupting 8 bytes past the struct. This is the highest-probability silent-corruption bug in the
milestone and it gets a dedicated golden test.

**Code facts.** `Event` (`crates/retrace-trace/src/lib.rs:14`) has no signal variant; `TRACE_MAGIC`
is `0x0004`. No trace binary is checked into the repo — `rung3.json` is jq's input and
`mach_msg2_capture.txt` is a message capture — so bumping the magic invalidates no fixture.
`BoxState` (`crates/retrace-box/src/lib.rs:515`) already carries the fd table's guest-visible slots
for the stated reason that a mid-run capture cannot re-derive them. `crates/retrace-box/src/lib.rs`
is 2816 lines.

## Unmeasured — Task 1 of the plan must measure these before any code is written

The M10 lesson was that an unmeasured assumption in the spec (the "fd 3" that was really fd 4) is
what bites. These are stated as unknowns, not guesses:

1. **Which signal syscalls any gate guest actually issues, and how often.** `RETRACE_TRACE=1`
   histogram over `hello_dyn`, `hello_rust`, and `jq`, resolved against the SDK header. M6 asserted
   that `sigaction`/`sigaltstack` "keep recording as ordinary forwarded syscalls" but no count exists
   anywhere in the repo to support it.
2. **Whether the guest's `getpid` returns retrace's pid.** `getpid`(20) is not special-cased, so it
   should — which is what makes `kill`'s self-check testable as `args[0] == retrace's own pid`. This
   must be confirmed at runtime, not inferred, because the safety boundary in §3 rests on it.
3. **The `__pthread_kill` thread-port operand**, and whether it is stable across runs. M7 observed
   `args=[0x103, 0x6]`; whether `0x103` is derivable or must be learned the way `guest_task_port` is
   learned from `task_self_trap` is unknown.
4. **Which raise path libc actually takes for `abort()`** — `__pthread_kill`(328) or
   `abort_with_payload`(521). M7's trace shows 328, but that was one guest at one point in the
   startup sequence.
5. **Whether a Rust `panic!()` guest reaches its panic at all.** This determines whether the headline
   gate goes in green or parked, and it cannot be known until the mechanism exists.

## The mechanism

### M11-table — the box owns the guest's dispositions

A new module `crates/retrace-box/src/sig.rs`, alongside `cache.rs`. `lib.rs` is already 2816 lines
and `SigTable` is pure, self-contained, and testable without a VM, so it does not belong there.

```rust
pub enum Disposition { Dfl, Ign, Handler(u64) }   // u64 = guest VA

pub struct SigAction { pub disp: Disposition, pub mask: u32, pub flags: u32 }

pub struct SigTable {
    disp: [SigAction; NSIG],            // index 1..=31; [0] unused, mirrors signal numbering
    blocked: u32,                       // bit (sig - 1); u32 because sigset_t is __uint32_t
    altstack: Option<(u64, u64, u64)>,  // (sp, size, flags) — stored, not honoured
}
```

The per-signal entry is a `SigAction`, not a bare `Disposition`, because the `oldact` writeback must
reproduce the mask and flags the guest previously installed — not just the handler. `blocked` is
`u32` for the same reason the structs are 24 and 16 bytes: `sigset_t` is `__uint32_t`
(`sys/_types.h:85`).

`Default` is all-`Dfl`, empty mask, no altstack, which is genuinely correct for a fresh process — so
there is no seeding step to get wrong. The module owns both struct layouts and the 24-in/16-out
conversion, in exactly one place.

`altstack` is recorded but never acted on: no handler runs this milestone, so there is nothing to run
on an alternate stack. Storing it makes `sigaltstack` a real syscall instead of a lie, and costs
nothing.

### M11-service — serviced above the trace, never forwarded

New arms in `record_box`'s `match stop`, placed **above** the generic forward arm at
`crates/retrace-core/src/lib.rs:441`. That ordering is the fix: it is what keeps `forward_and_diff`
from ever seeing a signal syscall.

Serviced state calls (46/48/52/53/329 — `sigpending` is a query rather than a disposition change, but
it is serviced identically) mutate or read the table, synthesize their own `(ret, err, writes)`
including the `oldact` writeback, append an ordinary `Event::Syscall`, and return via
`set_x0_err_and_return` (`crates/retrace-box/src/lib.rs:2152`). This follows the M9 console-close
precedent exactly.

Raise calls (37/328) resolve the target signal and consult the table:

```
Handler(va)                        -> assert!  (unmodelled; fail loud)
blocked                            -> assert!  (pending set unmodelled; fail loud)
Ign                                -> ret 0, Event::Syscall, guest continues
Dfl + default_action == Ignore     -> ret 0, Event::Syscall, guest continues
Dfl + default_action == Terminate  -> Event::Signal{sig, pc} + final snapshot; break
```

The terminal branch is structurally identical to M6's `Stop::Fault` arm at
`crates/retrace-core/src/lib.rs:124` — event, then final full-memory snapshot, then break — which is
why it inherits the existing seek and reverse-execution machinery without introducing a new concept.

**Servicing above the trace, not below, is a deliberate choice.** `SigTable` is a pure function of the
guest's own calls, which makes it formally eligible for symmetry rule 2 (handle it inside
`Box_::run()`, shared by both runs, and determinism is automatic). It is not done that way. Rule 2's
cases are *instruction* emulation — the timebase MRS, the Apple-IMPDEF undef-MRS, the B-family FPAC
strip. Every *syscall* in this codebase is handled above the trace with a replay mirror, because that
is what keeps it visible under `RETRACE_TRACE=1` and inside the divergence oracle's `(num, args)`
check. Those are precisely the diagnostics that made the M9 and M10 walls findable. The trace bytes
are worth it.

### M11-mirror — the replay half

Per symmetry rule 1, each record arm gets a mirror in replay's dispatch
(`crates/retrace-core/src/lib.rs:557`) that recomputes the same table transition and the same
synthesized bytes, then byte-compares against the recording. The comparison *is* the divergence
check, so an asymmetry surfaces as a divergence rather than as silent corruption.

`Event::Signal` gets a terminal arm mirroring the crash verify at
`crates/retrace-core/src/lib.rs:836`: recompute `(sig, pc)`, verify against the recording, then run
the existing full-memory comparison.

### M11-checkpoint — carry the table

`BoxState` gains the `SigTable`, with the justification already written for `pac_enabled`,
`stack_top`, and the fd slots: a mid-run capture cannot re-derive it. Without this, a `seek` into a
run that had installed a disposition restores a box that has forgotten it, and the first raise after
the seek takes the wrong branch — a divergence that would look like a signal bug and actually be a
checkpoint bug.

### M11-format — `Event::Signal`

```rust
Signal { sig: u64, pc: u64 }
```

`TRACE_MAGIC` `0x0004` → `0x0005`. `pc` is carried because it is what makes the event useful in a
debugger: it names the raise site.

Reusing `Event::Crash` with a synthetic ESR was considered and rejected. It would avoid the format
break, but it conflates two different terminal causes and makes the debug output lie — a `SIGABRT`
would print as a fault bearing an ESR the hardware never produced. Traces are not a distributed
artifact and no fixture is checked in, so the break is cheap; honesty about the terminal cause is the
entire point of recording it.

## Fail-loud boundaries

Every case is decided explicitly. Nothing falls through to a plausible success.

**Assert at raise, not at install.** A guest that installs a `SIGPIPE` handler and never receives
`SIGPIPE` is perfectly recordable, and libc plausibly installs handlers during startup that never
fire. `sigaction` with a real handler VA is therefore *faithfully modelled* — the table stores it —
and the assert fires only when a signal whose disposition is `Handler` is actually raised.

**`kill` to anything but self is a safety boundary, not a fidelity one.** It must never forward:
forwarding a guest's `kill(1, SIGKILL)` signals a real host process. Self is testable as
`args[0] == retrace's own pid` (pending measurement 2 above). Any other target asserts. Same shape
for `__pthread_kill`: assert unless the port is the guest's one known thread.

**Raising a blocked signal asserts.** POSIX says it goes pending until unblocked; modelling a pending
set is real work with no demonstrated consumer, and `abort()` explicitly unblocks `SIGABRT` before
raising, so the realistic path never reaches it.

**That decision is what makes `sigpending` honest.** Because a blocked raise cannot proceed, the
pending set is provably always empty, so `sigpending` returning empty is true by construction rather
than a convenient lie. These two decisions must be made together or the second becomes a lie.

**Assert, explicitly unmodelled:** `sigreturn`(184), unreachable unless the model itself is wrong, so
its assert is a self-check; `sigsuspend`(111) and `__sigwait`(330), blocking waits that would deadlock
a single vCPU with no second thread to wake it; and `terminate_with_payload`(520) /
`abort_with_payload`(521), separate terminal paths that bypass disposition entirely. That last pair
matters more than its scope suggests: they are live recorder-killing hazards *today*, forwarding
straight into retrace's process. Asserting converts a silent host death into a loud stop, which is the
minimum this milestone owes them even though modelling them is out of scope.

**Modelled, not asserted:** `__pthread_sigmask`(329) beside `sigprocmask`(48), because it is the entry
libc actually takes and treating it as exotic would wall the first real guest. A fatal signal whose
disposition is `Ign` returns success and continues, which is simply correct.

**Assert message discipline**, following the `dup2` precedent at `crates/retrace-core/src/lib.rs:446`:
each names what is not modelled, the evidence it is unexercised, and what to implement. An assert
that says only "unsupported" is a dead end for whoever hits it.

**What an assert leaves behind.** These are `assert!`s in `record_box`, so they panic the recorder
mid-run and the trace ends without a terminal event. That is the intended outcome — it is loud, and
`open_checked` already drops a torn tail rather than panicking, so the partial trace cannot be
mistaken for a complete one. The distinction that matters: an assert kills the *recorder* with a
message naming the cause, whereas today's forwarding kills the recorder with a *host signal* and no
explanation at all.

## Determinism posture

`SigTable` is a pure function of the guest's own syscall sequence. Both runs execute an identical
sequence, so both compute an identical table; nothing about it enters the trace. This is `FdTable`'s
`slots` posture exactly, and it is the **standard symmetric** posture — replay recomputes and
byte-compares — not M2-xpcport's deliberate asymmetry. Nothing here is nondeterministic, so no
verbatim-apply exception is needed or wanted.

The terminal `Event::Signal` is the only signal-derived data in the trace, and it is a function of
guest state alone.

## Correctness invariant

**No signal syscall is ever issued in retrace's process.** After M11, `forward_and_diff` is
unreachable for numbers 37, 46, 48, 52, 53, 111, 184, 328, 329, 330, 520, and 521 — each is either
serviced against the guest or asserted. The hazards in "The problem, precisely" (2) and (3) are gone
by construction rather than by guard.

## Scope

**In:** the `SigTable` and its module; servicing of 46/48/52/53/328/329/37 with replay mirrors;
`Event::Signal` + `Outcome::Signal` + the magic bump; `BoxState` carriage; asserts for
111/184/330/520/521 and for handler-installed, blocked, and non-self targets; the asm mechanism and
ignore-path fixtures; the safety-boundary test; the Rust panic headline gate.

**Out, and named as such:** handler invocation of any kind — signal frames, the `__sigtramp` ABI,
`ucontext`/`mcontext` layout, `sigreturn` (this is M12's job, and it is the larger half); a pending
signal set; honouring `sigaltstack`; signals between threads (there is one thread); asynchronous
signals from outside the guest, which are nondeterministic by nature and do not belong in a
deterministic recorder without an explicit injection model; `abort_with_payload` modelling;
`RLIMIT`-style resource interactions.

## Exit criterion

`just gate` is green with no existing assertion loosened, and `asm/raise.s` — a guest that raises
`SIGABRT` on itself — records and replays bit-for-bit with `Outcome::Signal{sig: 6}`, while the
recorder process exits 0.

The Rust `panic!()` headline gate goes in green if it clears, or parked `#[ignore]`d with its exact
observed signature written into both the ignore reason and the README Status section, per honest-gate
discipline. A parked gate is an acceptable outcome; a deleted one is not.

## Testing

**Pure unit tests, no VM** (`sig.rs`, `retrace-arch`): default state; install-then-read-back;
`sigprocmask` `BLOCK`/`UNBLOCK`/`SETMASK`; `default_action` classification. Plus the one that earns
its keep — a **golden byte test on the 24-in/16-out asymmetry**, asserting the synthesized `oldact`
write is 16 bytes with mask and flags at the right offsets. That bug would otherwise surface as guest
corruption 8 bytes past the struct and be diagnosed days later.

**Trace format:** `Event::Signal` round-trips through `Writer`/`Reader` with CRC intact; a trace
bearing the old `0x0004` magic is rejected loudly by `open_checked` rather than misparsed.

**`sigraise_e2e`** — `asm/raise.s` does `getpid`(20) then `kill`(37, pid, `SIGABRT`): two plain
syscalls, no libc, and it exercises the self-pid check as a side effect. Asserts `Outcome::Signal{sig:
6}`; the trace's terminal pair is `Event::Signal` then `Snapshot`; replay reproduces it; a double
replay is byte-identical; and — the assertion that looks redundant and is not — **the recorder exits
0, not 134**. That is the actual regression under repair, and naming it as its own assertion keeps it
from being quietly lost in a later refactor.

**`sigign_e2e`** — `asm/sigign.s` sets `SIGABRT` to `SIG_IGN`, raises it, then writes `ok` and exits
0. Proves the non-terminal branch and that the guest keeps running. Without it, a bug making every
raise terminal would pass the entire suite.

**`killother_e2e`** — `asm/killother.s` calls `kill(1, SIGKILL)`, and the test asserts the run aborts
with the assert message *and* that host pid 1 is untouched. This one is written rather than assumed,
because the failure it guards against is signalling a real host process: the only bug in this
milestone that escapes the sandbox.

**`panic_e2e`** — a Rust guest that `panic!()`s, green or honestly parked per the exit criterion.

## Risk register

- **R1 — the thread-port operand is not derivable.** If `__pthread_kill`'s port cannot be recognized
  as the guest's own thread, the self-check has nothing to compare against. Mitigation: learn it from
  `mach_thread_self` the way `guest_task_port` is learned from `task_self_trap`. Measurement 3 settles
  it before code is written.
- **R2 — libc's abort path is not 328.** If `abort()` routes through `abort_with_payload`(521), the
  headline gate hits an assert rather than the terminal path, and 521 has to move from "assert" to
  "modelled" — a scope increase decided on evidence. Measurement 4 settles it.
- **R3 — a gate guest already installs a handler for a signal it raises.** Would turn an assert into
  an immediate wall for an existing green test. Measurement 1 settles it, and it is the reason
  measurement 1 runs first.
- **R4 — the format bump ripples.** `TRACE_MAGIC` changes invalidate every trace, but no fixture is
  checked in and all traces are generated at test time, so the blast radius is bounded to any
  developer's stale local `t.bin`. Accepted.
- **R5 — walls come in chains** (M8's R1, restated). Clearing the abort path may reveal the next
  libSystem wall rather than a green rung. The parked-gate discipline in the exit criterion is the
  planned response, not a failure.

## Components

| Crate | Change |
|-------|--------|
| `retrace-arch` | `SYS_*` constants above; `SIGABRT`/`NSIG`/`SIG_DFL`/`SIG_IGN`; `DefaultAction` + `default_action(sig)` |
| `retrace-box` | new `sig.rs` (`SigTable`, `Disposition`, struct codecs); `Box_` field; `BoxState` carriage |
| `retrace-trace` | `Event::Signal { sig, pc }`; `TRACE_MAGIC` → `0x0005` |
| `retrace-core` | `Outcome::Signal { sig }`; record arms; replay mirrors; the asserts |
| `retrace-guest` | `asm/raise.s`, `asm/sigign.s`, `asm/killother.s`, `rs/panicky.rs` + `build.rs` wiring |
| `retrace` | `sigraise_e2e`, `sigign_e2e`, `killother_e2e`, `panic_e2e` |

## Open questions for implementation planning

1. Does the plan's Task 1 measurement change any row of the syscall table above? If a gate guest is
   measured issuing a call this spec asserts on, that row moves to "serviced" and the spec is amended
   before implementation — not worked around during it.
2. Should `Outcome::Signal` carry `pc` as well as `sig`? The event does. The CLI's crash reporting is
   the consumer that decides.
3. Does the debug CLI need a `signal` stop kind, or does the existing crash-stop presentation cover
   it? M6 made a crash a seekable stop; a signal inherits that machinery, but the *presentation*
   should not call a `SIGABRT` a fault — that is the same lie the `Event::Crash` reuse was rejected
   for.
