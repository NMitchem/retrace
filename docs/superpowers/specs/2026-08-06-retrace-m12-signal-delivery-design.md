# retrace M12-signal-delivery — the guest's handlers actually run

M11 gave the guest its own signal dispositions and stopped short at the boundary it named: a signal
with a handler installed raises a loud assert rather than a plausible lie. M12 crosses that boundary.
It builds the signal frame, enters the guest's handler through the real `sa_tramp` contract, and
services `sigreturn(184)` to put the guest back — for signals the guest raises on itself **and** for
real hardware faults, which is where the capability actually pays.

The milestone is bounded by one measured claim: **the frame is a pure function of guest state.** Its
bytes come from the guest's registers, its own `SigTable`, its own stack pointer, and the ESR/FAR the
guest's own faulting instruction produced. The single value retrace invents — the `sigreturn` token —
is a constant. So delivery takes the standard symmetric posture: replay recomputes the frame and
byte-compares it before applying, exactly as M11's `sigaction` arm does.

## The problem, precisely

Two arms, one of which is a live wrong answer rather than a parked boundary.

1. **A self-raised caught signal asserts.** `crates/retrace-core/src/lib.rs:554` panics with
   "signal {sig} has a handler installed … Implement those (M12)". That is M11 working as designed:
   a fail-loud boundary, honestly documented.

2. **A *fault-derived* caught signal does not assert — it silently ignores the handler.**
   `Stop::Fault` (`crates/retrace-core/src/lib.rs:130`) appends `Event::Crash` and breaks. It never
   consults `sigtable`. So a guest that installs a `SIGSEGV` handler and then faults is recorded as a
   terminal crash with its handler skipped, and nothing says so.

   The M11 Status section states the opposite — "the first guest that actually faults will hit the
   `Handler` assert rather than a plausible lie" (`README.md`). That is not what the code does, and
   the discrepancy is live: M11 itself measured that `hello_rust` now installs real `SIGSEGV` and
   `SIGBUS` handlers at startup. Correcting that sentence is part of this milestone.

M12 makes (2) route through the same disposition decision as (1), and gives both a real
implementation instead of an assert.

## Verified facts (this host, HEAD `995f636`, 2026-08-06)

Measured with two throwaway probes compiled and run natively, not recalled. Both belong in `spikes/`.

### Struct geometry (`clang -arch arm64`, `offsetof`/`sizeof` against the live SDK)

```
sizeof(struct __darwin_arm_exception_state64) =  16    __far=0  __esr=8  __exception=12
sizeof(struct __darwin_arm_thread_state64)    = 272    __x=0 __fp=232 __lr=240 __sp=248 __pc=256 __cpsr=264
sizeof(struct __darwin_arm_neon_state64)      = 528
sizeof(struct __darwin_mcontext64)            = 816    __es=0  __ss=16  __ns=288
sizeof(ucontext_t)                            =  56    uc_onstack=0 uc_sigmask=4 uc_stack=8
                                                       uc_link=32 uc_mcsize=40 uc_mcontext=48
sizeof(siginfo_t)                             = 104    si_signo=0 si_errno=4 si_code=8 si_pid=12
                                                       si_uid=16 si_status=20 si_addr=24 si_value=32
sizeof(stack_t)                               =  24    ss_sp=0 ss_size=8 ss_flags=16
sizeof(sigset_t) = 4   sizeof(struct sigaction) = 16   sizeof(struct __sigaction) = 24
```

`uc_mcontext` is a **pointer**, so `sizeof(ucontext_t)` is 56 and the mcontext lives elsewhere in the
frame. A design that assumed one flat struct would be wrong by 816 bytes.

Constants, read out of the same headers: `SA_ONSTACK=0x1`, `SA_RESTART=0x2`, `SA_RESETHAND=0x4`,
`SA_NODEFER=0x10`, `SA_SIGINFO=0x40`; `SS_ONSTACK=0x1`, `SS_DISABLE=0x4`; `SIGILL=4`, `SIGTRAP=5`,
`SIGFPE=8`, `SIGBUS=10`, `SIGSEGV=11`; `SEGV_MAPERR=1`, `SEGV_ACCERR=2`, `BUS_ADRALN=1`,
`BUS_ADRERR=2`, `BUS_OBJERR=3`, `ILL_ILLOPC=1`, `TRAP_BRKPT=1`.

### The `sa_tramp` entry contract

A probe installed its **own** trampoline via the raw `__sigaction` syscall (libc's `sigaction()`
overwrites `sa_tramp` with `_sigtramp`, so it cannot answer this question) and dumped the registers
the kernel entered it with, after a store to `0xdead0000`:

```
x0 = 0x10443856c   the catcher (handler VA)
x1 = 0x1e          infostyle == 30 (UC_FLAVOR), for an SA_SIGINFO handler
x2 = 0xb           the signal number
x3 = 0x16b9c62e0   siginfo_t *
x4 = 0x16b9c6348   ucontext_t *
x5 = 0x865fb04c76ccb870   the sigreturn token (process-randomized on the host)
sp = 0x16b9c62e0   == x3: THE FRAME BASE IS sp, with siginfo at offset 0
```

`x6 = 0xa` and `x7 = 0` were also observed. They are recorded here as observations, **not** modelled
as part of the contract — nothing in the frame explains them.

### The frame layout, derived from those pointers

| offset from new `sp` | contents | size |
|---|---|---|
| `+0` | `siginfo_t` | 104 |
| `+104` | `ucontext_t`, whose `uc_mcontext` points at `+160` | 56 |
| `+160` | `mcontext64` = exception(16) ‖ thread(272) ‖ neon(528) | 816 |
| | **total** | **976** |

Contents at entry: `uc_onstack=0`, `uc_sigmask=0`, `uc_mcsize=816`, `uc_mcontext=sp+160`;
`si_signo=11`, `si_code=2`, `si_addr=0xdead0000`; mcontext `__es.__far=0xdead0000`,
`__es.__esr=0x92000046` (the real hardware ESR: EC `0x24`, write, translation fault),
`__es.__exception=0`, `__ss.__pc` = the faulting instruction, `__ss.__sp` = the pre-signal `sp`,
`__ss.__cpsr=0`.

The pre-signal `sp` was `0x16b9c6730` and the frame base `0x16b9c62e0` — a gap of 1104 bytes for a
976-byte frame, i.e. **128 bytes of slack** between the frame top and the old `sp`. Reproduced, not
explained; a spike pins whether it is a red zone or alignment.

Note `si_code = SEGV_ACCERR(2)`, not `SEGV_MAPERR(1)`, for a wholly unmapped address. In the guest
*retrace* chooses this value, so the derivation must come from the ESR honestly rather than by
copying one host observation.

### The headline guest's behaviour, measured natively

A stock `rustc -O` binary storing through `0xdead0000`:

```
SIGSEGV query rc=0 handler=0x1003eb900 flags=0x41     (SA_ONSTACK|SA_SIGINFO)
SIGBUS  handler=0x1003eb900 flags=0x41
about to fault
EXIT=139                                              (128 + SIGSEGV)
```

Exit 139 alone cannot distinguish "the handler ran and returned" from "no handler". The disposition
query settles it: a handler **is** installed, and the process still exits 139 without hanging. A
handler that ran and did not reset the disposition before returning would loop forever; one that
aborted would exit 134. So the measured sequence is: fault → handler → not-a-stack-overflow →
`sigaction(SIGSEGV, SIG_DFL)` → return → `sigreturn` → the store re-executes → faults again → default
action terminates. That single run exercises delivery, the Apple trampoline, `siginfo`, `sigreturn`,
a mid-handler `sigaction` (already serviced since M11), a second fault, and M11's terminal path.

It also fails informatively: libstd's handler compares `si_addr` against the guard range it installed,
so **a wrong `si_addr` makes it print "has overflowed its stack" and exit 134 instead of 139.**

## Unmeasured — the plan's first task must measure these before any code is written

1. **What `_sigtramp` itself reads.** The probe pins what the kernel *passes*; Apple's trampoline may
   consult more of the frame. Disassemble it out of the shared cache.
2. **infostyle without `SA_SIGINFO`.** `30` (`UC_FLAVOR`) is the `SA_SIGINFO` value. The other is a
   guess until measured.
3. **The 128-byte gap** below the pre-signal `sp`.
4. **`crashy`'s live disposition at fault time.** M6's crash gate (`crashy_e2e`) covers a
   dynamically-linked C guest that faults. If anything in its libSystem startup installs a
   `SIGSEGV` handler, M12's new fault
   routing converts that `Event::Crash` into a delivery and breaks a green gate. This is M11's R3
   shape and gets M11's treatment: measure before writing the arm. The seeded swarm's injected faults
   need the same check.
5. **`si_code` for each ESR class retrace can actually produce**, so the mapping is derived rather
   than extrapolated from one observation.

## The mechanism

### M12-esr — the fault-to-signal mapping

`retrace-arch` gains `signal_of_esr(esr, far) -> (sig, si_code)`: EC `0x20`/`0x24`
(instruction/data abort from a lower EL) → `SIGSEGV` or `SIGBUS` by DFSC, EC `0x26` (SP alignment) →
`SIGBUS`, EC `0x00`/`0x0E` → `SIGILL`, EC `0x3C` (BRK) → `SIGTRAP`. Zero-dependency and pure, so the
whole table is unit-tested at full speed. Same crate and same shape as `default_action`.

### M12-frame — the builder is a pure function

`SigAction` gains `tramp: u64`, read from offset 8 of `struct __sigaction` — the field M11
deliberately discarded because nothing could use it yet. `encode_oldact` keeps returning a fixed
`[u8; 16]`: the 16-byte output struct has no `sa_tramp`, and a test pins that the captured value
cannot leak into the writeback.

`sig.rs` gains `build_frame(...) -> (Vec<u8>, EntryRegs)` — no `Box_`, no VM, no vCPU. Every offset in
the layout table above is asserted by a unit test that runs in microseconds. This is the structural
commitment of the milestone: the byte layout is the part most likely to be wrong, so it must not
require a hypervisor to test.

### M12-deliver — the box enters the handler

`Box_::deliver_signal(sig, si_code, si_addr) -> Vec<Region>` reads thread state off the vCPU, selects
the stack (the alternate one when `SA_ONSTACK` is set, one is installed, and the guest is not already
on it), calls the pure builder, writes the frame into guest memory, then sets `x0..x5`, `sp`, and
`pc = sa_tramp`. It blocks `sig | sa_mask` for the handler's duration unless `SA_NODEFER`, and honours
`SA_RESETHAND`.

Both record and replay call this one method. "Record and replay recompute identically" is therefore
true by construction rather than by discipline — there is one implementation, invoked twice.

### M12-sigreturn — and back

`Box_::sigreturn_restore(uctx_ipa, infostyle, token)` validates the token, reads the mcontext back out
of guest memory, and restores `x0..x28`, `fp`, `lr`, `sp`, `pc`, the vector state, the signal mask,
and the on-stack flag.

The host's token is process-randomized. Retrace synthesizes the entire frame, so it owns the token: a
fixed constant folded with the ucontext address — the posture of the fixed PAC keys. Nondeterminism
never gets an opening, and validation is a free fail-loud on a corrupted frame.

**PSTATE is sanitized.** `cpsr` is restored from a frame in guest-*writable* memory, so a guest that
rewrites that field could otherwise ask for arbitrary `SPSR_EL1`, mode bits included. The restore
masks to user-settable flags and never touches mode. This is the only place in M12 where
guest-controlled bytes reach a system register, and it gets its own fail-loud test.

### M12-neon — vector state

The mcontext's 528-byte NEON block is not decoration. A handler is ordinary compiled code and will
execute NEON; if `sigreturn` does not restore the vector registers, a handler that *returns* silently
corrupts the guest. The headline gate cannot catch that — its guest re-faults and dies immediately —
so it needs a gate of its own.

So `deliver_signal` fills the block from the vCPU via `get_simd(simd::q(0..32))` plus `FPCR`/`FPSR`,
and `sigreturn_restore` writes it back through `set_simd`.

**No checkpoint work is required, contrary to an earlier draft of this spec.** `BoxState` already
carries `fp: [u128; 32]`, `fpcr`, and `fpsr` (`crates/retrace-box/src/lib.rs:526`), captured at
`:2592` and restored at `:2650`. A recompute after an M4 seek therefore reads the same vector state
the recording had, and the byte-compare holds. The trace's `Event::Snapshot` `Regs` still carries no
vector state, but that is harmless: both runs start from a fresh vCPU at landmark 0, so their initial
FP state is identically zero.

### M12-route — three dispatch sites, three mirrors

1. **`Stop::Fault`** consults the `SigTable` first. `Handler` → deliver, append `SignalDelivery`,
   continue. Everything else → M6's `Event::Crash` terminal path, byte-for-byte unchanged.
2. **`kill`/`__pthread_kill` with `Handler`** replaces the M11 panic. It appends its ordinary
   `Event::Syscall` first — so the divergence oracle still checks `(num, args)` and the `kill` safety
   boundary still runs — and *then* delivers and appends `SignalDelivery`.
3. **`SYS_SIGRETURN`** replaces the M11 panic: an ordinary serviced syscall arm with no writes, whose
   register restore is recomputed on both sides.

Replay mirrors each: recompute through the same `deliver_signal`, byte-compare against the recorded
`writes`, then apply. The comparison *is* the divergence check, so an asymmetry surfaces loudly
rather than corrupting silently (symmetry rule 1).

**The M6 fault boundary survives, with an argument rather than an assertion.** Demand paging — the
cache window and `commit_reserved_page` — arrives as `Stop::Other`, a stage-2 abort taken to EL2
(`crates/retrace-core/src/lib.rs:631`). `Stop::Fault` is only ever a lower-EL **stage-1** abort
(`crates/retrace-box/src/lib.rs:1913`). M12 adds no new consumer of `Stop::Other`, so it cannot steal
a demand-paging case for exactly the reason M6 couldn't. That gets a regression test.

### M12-format — `Event::SignalDelivery`

```rust
SignalDelivery { sig: u64, si_code: u64, si_addr: u64, handler: u64, resume_pc: u64, writes: Vec<Region> }
```

`TRACE_MAGIC` `0x0005` → `0x0006`. No fixture is checked in, so nothing is invalidated.

One event shape for both causes, so there is one seek target, one debug line, and one mirror. This is
deliberately **not** handled below the trace inside `Box_::run()`, which symmetry rule 2 would
otherwise suggest. Rule 2's precedents — the timebase MRS, the Apple-IMPDEF undef-MRS, the B-family
FPAC strip — are *instruction* emulations: micro, high-frequency, semantically invisible. Entering a
signal handler is a *control transfer*: macro, rare, and the loudest thing that happens in a run. For
a reverse debugger, "rewind to where the signal was delivered" is a query users have, and hiding
delivery below the trace would cost the landmark to buy a determinism property the symmetric posture
already provides. M11 ruled the same way when it refused to fold `Event::Signal` into `Crash`.

## Fail-loud boundaries

Each of these is unmodelled, and asserts rather than guessing:

- **A blocked synchronous fault.** POSIX leaves it undefined and Darwin force-delivers; M11 models no
  pending set, and guessing here would be a plausible lie. Assert.
- **A fault taken inside a handler** (nested delivery). Assert.
- **`sigreturn` with a bad token**, or one asking for PSTATE mode bits. Assert.
- **`sigsuspend`(111), `__sigwait`(330), `terminate_with_payload`(520), `abort_with_payload`(521)**
  keep M11's asserts, unchanged.
- **`SA_RESTART`** is unreachable by construction: M12 delivers only synchronously, at a fault or a
  self-raise, and never interrupts a blocking syscall. Documented, not implemented.

## Determinism posture

Standard symmetric, following M11's `sigaction` arm (`crates/retrace-core/src/lib.rs:825`): record
puts the frame bytes in `writes`; replay recomputes them and byte-compares before applying. Every
input is guest state — registers, the `SigTable`, the stack pointer, the guest's own ESR and FAR — and
the one invented value is a constant. This is explicitly **not** M2-xpcport's verbatim-apply
exception; nothing nondeterministic is in play.

## Scope

**In:** `signal_of_esr`; `SigAction.tramp`; the pure `build_frame`; `Box_::deliver_signal` and
`sigreturn_restore`, including NEON and PSTATE sanitizing; `Event::SignalDelivery` and the magic bump;
the three dispatch sites and their replay mirrors; `SA_ONSTACK`/`sigaltstack` honouring with
`uc_onstack` and its restore; `SA_NODEFER`, `SA_RESETHAND`, and `sig | sa_mask` blocking;
fault-derived `SIGSEGV`/`SIGBUS`/`SIGILL`/`SIGTRAP`.

**Out, and named as such:** `PROT_NONE` enforcement and real guard pages — **the new top deferred
item** (see below); a pending-signal set; nested delivery; threads; asynchronous signals from outside
the guest, which are nondeterministic by nature and need an explicit injection model; `SA_RESTART`;
arm64e guests, whose frame thread-state is PAC-signed.

### Why `PROT_NONE` is out, and why it becomes the top deferred item

`Box_::commit_reserved_page` (`crates/retrace-box/src/lib.rs:1057`) silently demand-commits any page
inside a tracked reservation, and `prot` is ignored except `PROT_EXEC`. libstd's `install_main_guard`
maps its stack-overflow guard page `PROT_NONE MAP_FIXED` — so **in the guest that page does not
guard.** A Rust stack overflow grows straight through it instead of faulting.

That rules out "a Rust guest survives its own stack overflow" as an M12 gate, and it is the reason the
headline uses a wild pointer instead. Making `PROT_NONE` fault requires real page-table permissions
plus a fault path that separates "reserved and committable" from "reserved `PROT_NONE`, must fault" —
a milestone's worth of work, and the obvious M13.

## Exit criterion

`just gate` green with no existing assertion loosened; all five current headline gates still green and
un-ignored; and `segv_rust_e2e` — a stock full-`std` Rust guest faulting through a wild pointer —
recording and replaying bit-for-bit at exit 139, replayed twice, with a complete trace ending
`SignalDelivery` → `Signal` → `Snapshot`.

**Exit 139 is necessary and nowhere near sufficient, and the gate must say so.** An *uncaught* fault
already exits 139 — that is precisely what M6's `crashy_e2e::record_and_replay_of_a_crash_exit_139_
with_the_crash_line` asserts. So if M12's fault routing were entirely broken and the Rust guest's
handler were ignored exactly as it is today, the guest would still exit 139 and a gate resting on the
exit code would pass for the wrong reason. The gate therefore asserts on the trace:

- **exactly one** `Event::SignalDelivery` with `sig == 11`, whose `handler` equals the VA libstd
  installed (learned from the recorded `sigaction`, not hardcoded);
- a `SYS_SIGRETURN` syscall event *after* it, proving the handler returned rather than aborted;
- a terminal `Event::Signal { sig: 11 }` after that, proving the re-fault took the default action;
- and `resume_pc` equal to the faulting `pc`, proving the store was re-executed rather than skipped.

Without those four, the headline gate is an exit-code coincidence shared with a milestone-old test.

Per honest-gate discipline: if it does not clear, it parks `#[ignore]`d with the exact wall named in
both the test and the README, and a NEW gate is parked for the capability that wall blocks — never a
regression of the existing five.

## Testing

**Pure, no VM.** Frame offsets against the measured layout table; the `signal_of_esr` mapping; `tramp`
captured on input and provably absent from the 16-byte `oldact`; token derivation.

**Box-level, freestanding asm guests supplying their own trampoline** — these test the contract
without libc in the way:

- `sigframe.s` — the register contract: `x0..x5`, `sp == siginfo`, and the frame's contents.
- `segvcatch.s` — the handler advances `uc_mcontext->__ss.__pc` by 4 and returns; the guest continues
  past the faulting store and exits 0. **The only gate that proves `sigreturn` restores *mutated*
  state.**
- `altstack.s` — `SA_ONSTACK` plus `sigaltstack`; the handler asserts its own `sp` lies inside the
  alternate stack. **Mandatory, because the headline gate does not prove alt-stack handling at all** —
  a wild-pointer fault runs perfectly well on the main stack, so `SA_ONSTACK` could be ignored
  entirely and the headline would still pass. Stating that is the honest-gate rule applied to a gate
  that *passes*.
- A vector-survival gate: hold a known value in a vector register across a caught fault and assert it
  survives `sigreturn`. Without it the NEON work is untested.
- Fail-loud gates for each boundary above.

**End-to-end.**

- `segv_rust_e2e` — the headline, as described in the exit criterion.
- A dynamically-linked C catch-and-recover guest — **the only gate that exercises Apple's real
  `_sigtramp`** rather than a hand-written one.
- `crashy_e2e` unchanged — the regression that an *uncaught* fault is still an `Event::Crash`.
- A reverse-debug seek to the `SignalDelivery` landmark. This is the payoff that justified a
  first-class event over below-the-trace handling, so it is tested rather than claimed.

Determinism coverage uses in-process recordings. M11 established that guests calling `getpid` cannot
use the cross-process `assert_trace_reproducible` oracle, because the recorder's pid lands in the
trace; the Rust guest will call it.

## Risk register

| # | Risk | Mitigation |
|---|---|---|
| R1 | `crashy_e2e` or the seeded swarm flips from `Crash` to delivery, breaking a green gate | Measure `crashy`'s live disposition at fault time in the plan's first task, before the arm is written (M11's R3 treatment) |
| R2 | `_sigtramp` reads more of the frame than the probe revealed | Spike: disassemble it from the shared cache |
| R3 | infostyle without `SA_SIGINFO` is wrong | Spike measures it; do not guess |
| R4 | `si_addr` wrong → libstd misreads the fault as stack overflow | The gate discriminates: 134 instead of 139, loudly |
| R5 | A returning handler clobbers guest vector state | `sigreturn_restore` writes Q0–Q31 back; the vector-survival gate proves it |
| R6 | The 128-byte gap matters and is reproduced wrongly | Spike; reproduce rather than explain |
| R7 | The headline gate passes on an exit-code coincidence — an uncaught fault also exits 139 | The four trace assertions in the exit criterion, not the exit code |

## Components

| Crate | Change |
|---|---|
| `retrace-arch` | `signal_of_esr`; `UC_FLAVOR`, `SA_*`, `SS_*`, `si_code` and struct-size constants |
| `retrace-box` (`sig.rs`) | `SigAction.tramp`; the pure `build_frame` and its layout tests |
| `retrace-box` (`lib.rs`) | `deliver_signal`, `sigreturn_restore` |
| `retrace-trace` | `Event::SignalDelivery`; `TRACE_MAGIC` → `0x0006` |
| `retrace-core` | The three dispatch sites and their three replay mirrors |
| `retrace-guest` | `sigframe.s`, `segvcatch.s`, `altstack.s`, a vector-survival guest, a dyn C recover guest, a Rust wild-pointer guest |
| `retrace` | `segv_rust_e2e` and the end-to-end gates above |
| `spikes` | The `sa_tramp` register probe and the `_sigtramp` disassembly |

## Open questions for implementation planning

1. Does `_sigtramp` require anything at `sp` below the frame (the 128-byte gap), or is it slack?
2. What infostyle does a non-`SA_SIGINFO` handler get, and does `_sigtramp` branch on it in a way
   that changes the frame we must build?
3. Should `si_code` follow the host's observed `SEGV_ACCERR` for unmapped addresses, or the DFSC-
   faithful `SEGV_MAPERR`? Nothing in the gate set depends on the answer, which is precisely why the
   choice must be made deliberately and documented.
4. Does any currently-green gate install a handler for a signal it then takes? (R1.)
5. `BoxState` already carries vector state, so a seek across a delivery should recompute the frame
   correctly. Is there a test that proves it, or does M12 owe one?
