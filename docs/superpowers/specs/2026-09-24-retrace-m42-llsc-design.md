# M42-llsc: stepping and debug stops that keep an exclusive pair's store

**Date:** 2026-09-24. **Branch:** `m42-llsc` from `main` at the M41 merge (`1d95a93`).
**Companion:** `2026-09-24-retrace-m42-llsc-measurements.md` (t0, M1–M7, plus M8, taken while
writing this spec). §2 cites it.
**Research:** the session scratchpad's `m42-t0-research.md` (encodings, precedents, census, the two
designs) and `m42/t0/code-facts.md` (line-exact code facts on `1d95a93`). They are not committed; this
spec restates everything it uses from them. A claim from reading code says so. Where a claim is
inferred and not measured, the spec names the task measurement that owes it.
**Approach:** design B of two (R1), chosen by the controller under the operator's autonomous-run
authorisation of 2026-09-24 (M41 → M42 → M43).

## 1. Purpose

M43 puts lldb in front of this debugger. lldb single-steps and plants breakpoints freely, and M41
found that the debugger cannot do either soundly inside an AArch64 exclusive pair. Every VM exit
between a load-exclusive (`ldxr`, `ldaxr`, `ldxp`, …) and its store-exclusive clears the core's
exclusive monitor, so the store fails:
- a single-step makes that exit;
- a hardware breakpoint or watchpoint stop makes it;
- a seek makes it, because it reaches `(n, k)` by stepping;
- a window-length probe makes it, and so does every terminal park.

Record never makes that exit, so record's store succeeded. The replay under the debugger then runs a
path the recording never ran.

t0 measured what that costs on a repo-shaped fixture. One class of failure is loud: a replay
divergence. The others are worse:
- `continue`, `reverse-continue` and `reverse-stepi` **hang** on a retry loop;
- a watch on a retry cell gives **phantom hits** that never end, and exits 0;
- `reverse-continue` answers **`no earlier hit`** when there is one;
- `stepi` reports coordinates the recording never had.

The debugger cannot be put in front of lldb until those are gone.

This milestone gives `Box_` a **shadow of the exclusive monitor** and emulates the store-exclusive
exactly when retrace itself caused the monitor to be lost. It lives below the trace (symmetry rule
2), so record and plain replay are untouched and provably never engage it. It changes no step
semantics: every instruction still retires once per `step()`, and every `(n, k)` stays addressable.

## 2. What was measured before this spec was written

The companion is the record. In summary, on `1d95a93`, with a scratch asm fixture holding four
LL/SC shapes. Each shape publishes its outcome in the arguments of the syscall that ends its window,
so the divergence oracle sees a lost store:
- (a) discard-status, dyld's `getpid` words exactly;
- (b) a retry loop, three increments;
- (c) a CAS shape;
- (d) an `ldxp`/`stxp` pair.

| # | Result on `1d95a93` |
|---|---|
| M1 | Native record and replay ×2 are clean. |
| M2 | `stepi` past (a)'s `ldxr`, then `continue`: **diverged** (an extra `getpid` at landmark 1). |
| M3 | Stepping (b): `stepi 1000000` **livelocks** (the counter never moves). `continue` to a breakpoint after the loop, `reverse-continue` to it, and `reverse-stepi` into window 2 all **hang** (killed at 60 s). A `reverse-continue` whose phase 2 steps window 2 answers **`no earlier hit`** (M3e): the right answer is `(2, 25)`. |
| M4 | `break` on (a)'s `stxr`: **diverged**, both directions. `break` on (b)'s `stlxr`: **phantom hits** forward, **hang** backward. |
| M5, M6 | `watch` on (b)'s, (c)'s or (d)'s cell: **phantom hits** forward (exit 0), **hang** backward. (a)'s cell: diverged both ways. |
| M7 | `stepi K; continue` at every K in each pair: every K from LDX+1 to STX+1 **diverges**; the LDX's own K passes. |
| M8 | **The step exit reports a stepped load-exclusive.** The retire of `ldxr`, `ldaxr` (twice) and `ldxp` reports `ESR = 0xcb000062`. Every other retire reports `0xcb000022`. So ISS.ISV (bit 24) is always 1, and ISS.EX (bit 6) is 1 exactly on a load-exclusive: the architecture's own syndrome for this problem. |

Also measured, on the hardware:
- **A watchpoint fires on a store-exclusive that will fail** (M5, pre-retire at the `stlxr`).
- **Window 1's length probes as 9, not 16**, once stepping has broken (a)'s pair.

Carried from M41's ledger (`t1-diverge-diag.md`, measured on `threadrust`):
- **The dynamic path holds three pairs**, one per image's first `getpid()`: `(g+1, 1)` `ldxr` and
  `(g+1, 3)` `stxr`, where `g` is each of the three `getpid` (20) landmarks.
- `seek(g+1, j)` for `j ≥ 2`, then forward replay, diverges six landmarks later.
- Stepping from landmark 1 cost 2.8 s CPU per 659k instructions.

## 3. Design

### 3a. The shadow monitor

`Box_` gains `excl: Option<Excl>`. `Excl` holds:
- the marked VA, top byte stripped (`va & 0x00FF_FFFF_FFFF_FFFF`, since `TCR_EL1` sets TBI0 and
  `va_to_ipa` does not strip);
- the element size in bytes (1, 2, 4, 8);
- whether it is a pair;
- the bytes the load returned (up to 16);
- how it was set: `Stepped` or `Inferred` (§3d), for the fail-loud messages.

The monitor belongs to the PE, not to a guest thread, so the shadow is **per-vCPU**. It is not in
`ThreadCtx`.

**Set.** `step()` sets the shadow when its exit is the EL0 software-step retire (`run_one_for_step`'s
`Ec::SoftStep`, `CPSR.EL == 0` arm) and the syndrome has ISV and EX set (M8). It sets nothing
otherwise, so the ordinary step path pays no instruction decode (R2).

On such a retire:
1. The retired instruction is at `pc − 4`. A load-exclusive never branches.
2. It is decoded with `retrace_arch::decode_excl`, which must return a load-exclusive. If it does not,
   the hardware and the decoder disagree, and the step fails loud.
3. The VA is the base register's value. A base that aliases a destination (`Rn ∈ {Rt, Rt2}`, with
   `Rn ≠ 31`) has been overwritten by the load, so it fails loud as unmodelled. `Rn = 31` is SP,
   read from `SP_EL0`, never `reg::x(31)`: in hv-sys, `reg::x(31)` *is* the PC.
4. The loaded bytes are read from guest memory at that VA, right after the retire. There is one vCPU,
   so memory then *is* what the load returned, and it holds even when `Rt` is XZR.

A later load-exclusive replaces the shadow, as it re-marks the hardware monitor.

**Clear.** The rule: **every exit that is not a debug exit clears the shadow.** Only three exits count
as debug exits:
- the EL0 software-step retire;
- a Breakpoint (EC `0x30`/`0x31`);
- a Watchpoint (EC `0x34`/`0x35`).

Everything else clears it: a syscall, an EL1 fault, an emulated instruction (timebase MRS, undef MRS,
FPAC), a stage-2 abort, and a vtimer or cancel exit. The rule is exact, not a heuristic:
- the non-debug exits are the exits record also takes, at the same instruction, in the same page
  state;
- each ends in an ERET, which clears the hardware monitor (Sail `AArch64_ExceptionReturn`, cited in
  the research);
- the debug exits exist only in a debugger session, and they are precisely the ones the shadow must
  survive.

*Amended while planning (the plan's R12):* the first bullet does not hold for an **asynchronous**
exit (vtimer or cancel), because record never takes one at the same instruction. Inside
`run_one_for_step` such an exit is retrace's own, like the step exit, so it **leaves** the shadow:
clearing there would make a stepped pair's outcome depend on host timing. In `run()` the shadow is
always clear when that arm runs (§3e), so it calls the classifier with "not debug", as every other
arm does.

**One classifying function is called from every exit arm of both `run()` and `run_one_for_step()`**,
including `run()`'s internal `continue` arms (2660, 2704, 2708, 2713). That is research risk #1's
guard.
- Both outer `_` arms (`run()` 2741, `run_one_for_step()` 2846) mix debug and non-debug exits, so
  the function classifies them by `ec_of(syndrome)`.
- In `run_one_for_step()`, `Stop::Step` is returned both by the EL0 retire (a debug exit) and by the
  three emulations (not debug exits), so the class is decided at the arm, never from the returned
  `Stop`.

Three more sites clear the shadow:
- **A `clrex` retire.** `step()` decodes the instruction at pc whenever the shadow is set (§3b), and
  clears it after a `clrex` retires.
- **The emulated store-exclusive** (§3b).
- **`switch_to_thread`,** after its `cur == tid` early return. Every switch already follows a syscall
  exit, so this site is belt and braces.

`from_checkpoint` restores the carried value (§3f). The constructors `load_with_pac`, `load_dynamic`
and `restore` start with `None`.

### 3b. The emulated store-exclusive, in `step()`

After `settle_schedule` (2778) and before the SS arming (2786), when the shadow is set, `step()`
decodes the instruction at pc:

- **`clrex`:** step it as normal, then clear the shadow.
- **A store-exclusive (`stxr`/`stlxr`/`stxp`/`stlxp`, any size):**
  1. **Validate.** Every check is a pure function of the shadow, the decoded instruction, the
     register values, the target bytes and the leaf descriptor (R4). A failure panics, with a message
     that names the check and the pc.
     - The VA (base register, tag stripped), element size and pair-ness equal the shadow's.
     - The VA is naturally aligned to the access (twice the element size for a pair).
     - The status register aliases none of the others: `s == t`, `pair && s == t2`, or
       `s == n && n != 31` is CONSTRAINED UNPREDICTABLE.
     - The target bytes still equal the shadow's loaded bytes. This is the tripwire for a plain store
       to the marked bytes between the halves, which is IMPLEMENTATION DEFINED natively.
     - The stage-1 leaf grants EL0 write, `AP[2:1] == 0b01`. A TEXT (`ATTR_CODE`) or
       `PROT_NONE` target would fault natively, and a host write ignores stage-1 permissions.
  2. **Raise the debug stops the hardware would raise** (§3c). If one applies, return it. The
     shadow stays set, the guest does not move, and nothing is written.
  3. **Emulate:**
     - write the bytes through the host mapping (`va_to_ipa`, then `write_guest`, which deliberately
       makes no syscall-watch check);
     - set `Ws = 0` unless `s == 31` (WZR discards the status: dyld's `stxr wzr`);
     - set `pc += 4`;
     - clear the shadow;
     - return `Stop::Step`.

     Store values come from `Rt`/`Rt2`, and 31 means XZR, which stores 0. CPSR is untouched: nothing
     trapped, so there is no ELR/SPSR to restore (code facts §1e).
- **Anything else:** step as normal. An instruction between the halves (`cbnz`, `add`, `cmp`,
  `b.ne`, a load) retires through the EL0 step arm, a debug exit, so the shadow survives it. A
  branch out of the sequence leaves the shadow set until the next non-debug exit, exactly as the
  hardware monitor stays set.

K is exact with no API change. The emulated store is one `Stop::Step`, and an emulated success is the
native success, so the stepped path now equals the path record ran. Window lengths, `reverse-stepi`
and the M41 cursor are unchanged in meaning.

### 3c. The debug stops the hardware would raise

Hardware order at one instruction is breakpoint first, then watchpoint. Both are taken pre-retire.
M5 measured that the watchpoint fires even on a store-exclusive that fails. So the emulator raises:

1. **A breakpoint stop, if one is armed at pc.**
   - `Box_` keeps no breakpoint list (`arm_hw_breakpoint`, 2859). So the check reads back each of
     the six slots' `DBGBCRn_EL1`/`DBGBVRn_EL1` and matches `DBGBCR_ARM` and the pc. The hardware
     registers are the ground truth, and reading them needs no new debugger field.
   - It returns `Stop::Other { esr }` with EC `0x30` and IL set.
   - `last_far = pc`. No consumer reads a breakpoint's FAR (code facts §4).
2. **Otherwise, a watchpoint stop, if an armed range in `watch_ranges` overlaps `[va, va + bytes)`.**
   - It returns `Stop::Other { esr }` with EC `0x34`, IL set and WnR set.
   - `last_far` is the lowest overlapped byte (R5), which `watched_of` resolves by exact byte.

Every consumer tests only `ec_of(esr)` (`advance`, `step_watched`, `step_armed`), so the ISS is not
otherwise modelled.

The callers then do what they already do after a hardware stop: clear breakpoints, or watches, and
call `step()` again. The next call raises the next stop, or emulates. A breakpoint on a watched
store-exclusive is therefore two hits at one coordinate, Bp then Watch, the order M41 §3b defines.

### 3d. A native stop inside a pair: inference

A breakpoint or watchpoint stop taken by `run()` natively, between an LDX that ran natively and its
STX, breaks the pair just as a step does. The ERET that resumes the guest clears the monitor. Nothing
recorded which instructions ran, so the shadow has to be **inferred** at that stop. This is the
milestone's one heuristic, and it is confined to native debug stops. Those happen only in a debugger
session: never in record, never in plain replay.

`run()` keeps a local `entry_pc`, the pc at each guest (re)entry (2659). At a Breakpoint or
Watchpoint exit at pc `P`, it scans backward from `P − 4`:

- at most 16 instructions (gdb's bound);
- never past the start of `P`'s 16 KiB page;
- stopping with no candidate at a store-exclusive, a `clrex`, an unconditional branch (`B`, `BL`,
  `BR`, `BLR`, `RET`) or an exception-generating instruction (`SVC`, `HVC`, `BRK`, …).

The first load-exclusive found, at `L`, is the candidate. The shadow is set from it only if all four
hold:

1. **`entry_pc ∉ (L, P]`.** The last entry, whose ERET cleared the monitor, came before the LDX.
2. **The base is not aliased by a destination** (`Rn ∉ {Rt, Rt2}` unless `Rn = 31`, which is SP),
   as in §3a.
3. **The LDX's destination register(s) still equal the bytes at the VA**, for each `Rt ≠ 31`. This
   check converts most wrong inferences (a jump into the middle of the pair, a rewritten base) into
   *no* inference, which is today's behaviour, not into a wrong emulation.
4. **The target is mapped.** An unmapped VA infers nothing.

(Amended while planning: this condition used to read "an LDX of a shape the decoder refuses fails
loud". That was vacuous, because `decode_excl` recognises the whole load-exclusive class by mask, so
there is no refused LDX shape.)

**Amended in execution (Task 5, the plan's Ruling T5-a).** Condition 3 as first written rejects
the dominant LL/SC shape by design. In an in-place retry loop (`ldaxr x1; add x1, x1, #1;
stlxr w2, x1`), the destination holds the *new* value at the store, never the loaded bytes. So
the fixture's (b) and (i) infer nothing, and neither do libsystem_kernel `__vfork`'s two sequences
(census #3 and #4), which rewrite `w10` between the halves. The spec's own named regressions
M4/M5 backward and E3 need that inference. Measured by a probe in `infer_excl`: condition 3 was
the only condition failing at (b)'s `stlxr`. Conditions 3 and 5 now read:

3. **Each destination that no instruction in `(L, P)` writes still equals its bytes at the VA**
   (`Rt ≠ 31`). A destination the sequence itself rewrites cannot be checked this way, so it is
   not checked.
5. **Every instruction in `(L, P)` has known register effects, and none writes the base.** Each
   one must be either:
   - a data-processing instruction (immediate class `100x`, or register class `x101`), which
     writes at most its `Rd` (bits 4:0; 31 is SP or XZR, and counts as a write of both); or
   - a conditional branch (`B.cond`, `CBZ`/`CBNZ`, `TBZ`/`TBNZ`), which writes no register.

   Any other instruction in `(L, P)` infers nothing: a load or store (which may write back its
   base), a system instruction, or SIMD. So does an `Rd` that equals `Rn`. For an SP base, that is
   any `Rd` of 31. Every census sequence passes: dyld `getpid`'s `cbnz`, and the `add`/`sub`/
   `subs`/`cmp`/`mov`/`csel` of the others. The check is a pure function over the scanned words,
   unit-tested beside `scan_back`.

Condition 5 turns the assumption "the base is not rewritten between the halves" (below) into a
check. What condition 3 no longer backstops is a sequence whose destinations are all rewritten:
there, "nothing branches into `(L, P]`" is the only guard against a jump into the middle of the
pair. That assumption was already a README residual. It remains one, and it now carries that
weight.

The inferred shadow's loaded bytes are the target bytes at the stop. The exit is classified first
(a debug exit, so nothing is cleared), then the inference runs.

**What the inference assumes, and has not measured:**
- the pair is reached by fall-through;
- nothing branches into `(L, P]` from outside;
- the base is not rewritten between the halves (checked by condition 5 since Task 5);
- no plain store of an *identical* value hits the marked bytes before the stop.

The research's census found every LL/SC sequence in the corpus satisfies all four. These are
recorded as README residuals, not as guarantees.

### 3e. `run()` entered with the shadow set

Native execution from inside a pair is impossible: entering the guest is an ERET, which clears the
monitor. So `run()`, after its `settle_schedule` (which clears the shadow on a switch), first steps
while the shadow is set:

- **`Stop::Step`:** continue if the shadow is still set; otherwise the sequence is finished, so fall
  into the native loop.
- **A Breakpoint or Watchpoint stop (synthesized or hardware), or a stage-2 abort:** return it,
  exactly as a native `run()` would return it.
- **A stop that came through the guest's EL1 vector** (`Stop::Syscall`, `Stop::Fault`, the inner
  `Stop::Other`): the classifier has already cleared the shadow. Fall into the native loop, which
  re-enters at the vector head and delivers that same stop through `run()`'s own arms. It never
  returns the step's version. This is the path M41's `AtTrap` → `advance()` already takes (step's
  syscall is left unconsumed at EL1).

`run()` never returns `Stop::Step` (its callers `unreachable!` it).

**The prologue is bounded at 16 steps** (the §3d scan bound, and gdb's). If the shadow is still set
after them, the prologue drops it and the native loop resumes.
- The only way to get there is a load-exclusive whose sequence a branch left: (h), or dyld's
  `getpid` when another thread filled the cache first.
- Without the bound, `run()` would single-step all the way to the next syscall. That is correct,
  but it costs a VM exit per instruction for as long as that takes.
- Dropping the shadow is what resuming natively does to the hardware monitor anyway. A
  store-exclusive to the marked bytes more than 16 instructions later, with no exit between, would
  then fail under the debugger where it succeeded natively. No census shape does that. It is a
  README residual, and it is loud if it ever matters, because the replay diverges.

(Amended while planning, R10 of the plan.)

### 3f. Checkpoints, record and plain replay

**`BoxState` carries `excl`**, after `fall_throughs`.
- `BoxState` is never persisted, so this is no format change and needs no `TRACE_MAGIC` bump
  (code facts §5).
- A checkpoint is always taken at an exit, where the hardware monitor is already open, so the shadow
  is the whole monitor state. With it carried, a checkpoint taken mid-pair is sound, where today
  every position after a stepped LDX is poison.
- `from_checkpoint` restores `excl` like any other guest state, and does not reset it with the four
  debugger fields.
- `checkpointparity.rs`'s obligation gets an equality row, through a `dbg_excl()` accessor.
- `restoreparity.rs`'s byte-compared `dbg_internal_state()` string is not changed. Its contract
  stays as it is.

**Record and plain replay never engage the shadow.** Neither steps nor arms a debug register, so no
debug exit happens and the shadow can never be set. Two asserts pin it:
- in `record_box` right after `b.run()` (141), naming the exit;
- in plain `replay()` after each `advance()`.

There is no dispatch arm and no trace change.

## 4. Guards: each asserts the difference it makes

**The repo fixture `llsc`** (`crates/retrace-guest/asm/llsc.s`, built like `watchsweep`).

- **Shapes (a)–(d) are t0's scratch fixture, unchanged and in the same order**, so every t0
  coordinate in the companion holds for windows 1–4.
- **Appended, each publishing its outcome in the arguments of the syscall that ends its window:**
  - (e) `ldxr; clrex; stxr`, which natively fails;
  - (f) `ldxr; mrs x6, cntvct_el0; stxr`, where the emulated timebase is a non-debug exit, so it
    natively fails;
  - (g) `ldxr; svc getpid; stxr`, which natively fails, with the store in the next window;
  - (h) (a)'s shape again on the filled cell: the `cbnz` is taken, so there is an LDX with no STX;
  - (i) in the **exit window**, a one-pass retry loop whose entry count rides in `exit`'s `x4`. That
    puts a pair on the terminal park's single-stepped window (M41's owed item).
- **Addresses are the fixture's symbols** (`a_ldx`, `b_stx`, …), resolved through the debugger's
  symbol operands, never hardcoded.

**Named regressions**, all in a new `crates/retrace/tests/llsc_e2e.rs`. Each is RED on `1d95a93`
(the RED transcript is ledgered). Every run that hung at t0 is **bounded**: a helper kills the child
after a fixed budget, so a regression fails the test instead of stalling the gate.

| t0 | Assertion |
|---|---|
| M2 | `stepi` past (a)'s `ldxr`, `continue`: exit 0, output `a…` byte-identical to replay |
| M3a | from `(2, 0)`, `stepi 25` lands on `b_done` at `(2, 25)`, and the counter reads 3 |
| M3b, M3c | `break b_done`: `continue` from `(1, 0)`, and `reverse-continue` from `(3, 0)`, each answer `hit … at (2, 25)`, within the bound |
| M3d | `reverse-stepi` from `(3, 0)` lands at `(2, 33)`, on window 2's `svc`: window 2's length is 33 |
| M3e | the phase-2 case answers `hit … at (2, 25)` |
| M4 | `break a_stx`: both directions exit 0. `break b_stx`: exactly three hits forward (K = 7, 14, 21), then the next window; backward from exit, the last of them |
| M5, M6 | `watch` on each of `ctr`, `cas`, `pair+8` and `cella`: exactly the recording's hit count (3, 1, 1, 1) forward, and the last one backward, all exit 0 |
| M7 | at every K of every window, `seek(n, k)` then `advance()` to exit, at the session level: exit 0 with the recorded output. And the CLI form, `stepi K; continue`, at each pair's LDX+1 and STX+1 |
| M3d′ | `reverse-stepi` from `(2, 0)` lands at `(1, 16)`, on window 1's `svc` (t0 parked at `(1, 9)` on a `getpid` the recording never ran) |

**Clear-rule positive controls** (e, f, g). These are green before and after the change, because
stepping them natively fails their stores just as record did. Each is shown **able to fail**: with
that one clear site deleted (a `clrex` retire, the emulations' `Stop::Step` arm, the `Svc` arm), its
seek-then-`continue` diverges. The deletion runs are ledgered (M28's lesson). Shape (h) is asserted
at the session level: the shadow is set after its `ldxr`, still set after the taken `cbnz`, and clear
after the window's `svc`.

**The emulator's checks** are unit-tested on the pure validator, one test per fail-loud branch:
- VA, size, pair mismatch;
- misalignment;
- each aliasing form, including `s == n == 31` allowed (SP ≠ WZR);
- value drift;
- a non-writable leaf.

The decoder is unit-tested in `retrace-arch` with t0's vectors, including CASP (`08207c82`)
**rejected** and `ldar`/`stlr` rejected.

**The inference** is tested on (a) and (b):
- a breakpoint on the STX, stop natively, clear, step: the store lands and the replay continues to
  exit 0 (M41's `diag_q4_break_between` shape);
- the same with a breakpoint between the halves;
- a stop on the LDX itself infers nothing.

**The hit oracle** (`util::hits`, M41) runs its three chains on llsc armings. Each arming arms a hit
in the exit window, M41's lesson.
- `{b_stx}` + `watch ctr` carries Bp then Watch at each of the three coordinates.
- `watch pair+8` carries a pair's overlap and FAR.
- `{i_stx, exit svc}` carries the terminal window.

`hits.rs`'s module doc drops "exact only over windows with no exclusive pair". Its ground truth is
now "a hardware stop, or the stop the emulator raises in its place".

**The dynamic path** (`threadrust`):
- **Q3 flipped.** For the first `getpid` landmark `g`, discovered from the trace, `seek(g+1, j)` for
  `j ∈ {2, 3, 4}` then forward to exit gives exit 0. The session asserts the shadow was engaged:
  set at `(g+1, 2)`, after the `ldxr` retires, and clear at `(g+1, 4)`, after the `stxr`.
- **The oracle starts at landmark 1**, reverting M41's R13, only if its checks fit M41's budget,
  **≤ 120 s CPU** for the arming. Otherwise R13 stands, with the measured cost ledgered.

**Unchanged, and checked:**
- `TRACE_MAGIC` stays `RT\x00\x0a`;
- no dispatch arm in `record_box` or `ReplaySession::advance` changes;
- `verify_thread` stays at seven;
- `blockedctx.rs`'s assertions;
- every existing debugger test's output. The audit lists any that moved. The expectation is none,
  since no existing fixture steps a pair.

## 5. Task order and why

1. **The decoder** (`retrace-arch`): `decode_excl` (`Ldx`, `Stx`, `Clrex`, with fields) and
   `is_fallthrough_barrier`, plus unit tests. Pure, no VM. Everything later consumes it.
2. **The fixture and the REDs:**
   - `llsc.s` with (a)–(i), the `build.rs` block and the `LLSC` const;
   - the bounded-run helper;
   - `llsc_e2e.rs` with every named regression and the controls.

   The record outcomes are measured first: (e), (f) and (g) must fail natively. If one does not, the
   clear rule is wrong for that exit class: a Ruling and a re-scope (§7). The REDs are ledgered, and
   the controls are green.
3. **The shadow, the stepped path (E1) and `run()` entered inside a pair (§3e):**
   - `Excl`;
   - the set, from the EX bit, with the decoder cross-check;
   - the one classifier at every exit arm;
   - the `clrex`, switch and emulation clears;
   - the pure validator with its unit tests, and the emulation;
   - `BoxState` carriage and the parity obligation;
   - the record and plain-replay asserts;
   - §3e's stepping prologue in `run()`.

   §3e is here, not with §3d, because the debugger resumes native execution from a stepped
   position (`stepi` inside a pair, then `continue`). Without the prologue, M2 stays red however
   exact the stepping is. (Amended while planning; the first draft placed §3e with §3d.)

   M2, M3a–e, M3d′, M7, (h) and the control deletions pass: all of them reach a pair only by
   stepping. §3c is excluded, so a breakpoint or watch that applies at an emulated STX panics
   loudly ("not yet raised"), and never skips its hit silently. M4–M6 stay RED.
4. **§3c, the raised stops.** The forward M4, M5 and M6 go green, and so do the oracle armings'
   forward chains and their self-checks (`enumerate_hits` steps every pair with everything armed).
5. **§3d, inference at native stops.** The backward M4–M6, the inference tests and every oracle
   chain go green.
6. **The dynamic path:** Q3 flipped, and the oracle-from-1 measurement (R7).
7. **Close:**
   - the gate, reconciled;
   - the audit;
   - `cpython_crash_e2e`'s and `hitorder_e2e`'s CPU time, before and after (step throughput);
   - README: the LL/SC Known limit is replaced by its residuals, and stepping across pairs goes into
     "What works today";
   - the status-log section;
   - CLAUDE.md: the gate list gains `llsc_e2e`, and symmetry rule 2's examples gain the
     store-exclusive emulation;
   - this spec's §10.

Why this order:
- §3c comes after §3b because it needs an emulator to raise stops *before*.
- §3d comes last among the mechanisms because it builds on a shadow that already works on the exact
  path. It is also the only heuristic, so it is reviewed on its own.
- Each task flips a named subset, so a reviewer can reject one without the others.

## 6. Acceptance

- Every named regression was RED on `1d95a93` and is green after, measured both ways, with the
  transcripts ledgered.
- Every clear-rule control was shown able to fail.
- Every oracle chain on every llsc arming passes.
- Q3 on `threadrust` passes. The oracle start is decided by R7's measurement.
- The gate is green, reconciled file by file against M41's **666 / 0 / 9 over 141**:
  - `TRACE_MAGIC` does not move;
  - no dispatch arm changes;
  - no new `#[ignore]`.
- `cpython_crash_e2e`'s CPU time (`/usr/bin/time -l`, user + sys) is within 10 % of its pre-M42
  figure, or the difference is a ledgered Ruling. It is CPU, not wall-clock, because the operator
  runs concurrent sessions.

## 7. Halt rules, and what this milestone deliberately does not do

**Halt rules** (the run charter's list applies; these are M42's own):
- **The fixture's record shows a store-exclusive succeeding after an exit this design says clears
  the monitor** ((e), (f) or (g) publishes status 0). The clear rule is then wrong for that class.
  That is a Ruling and a re-scope of the classifier, argued from the measurement. It becomes a halt
  only if no clear rule fits every measured class.
- **An existing test's output moves.** Stop and classify it. A pair was being crossed silently, or
  this work changed something it should not. This is the audit's job, and an unexplained move halts.
- **A nondeterministic record/replay difference** (the charter's E2 class) halts.

**Not done:**
- **WFE** (EC `0x01`), including `ldxr; wfe` spin-waits. It is unhandled on every path today, not
  only under stepping, and it is a separate item.
- **The asynchronous host-interrupt residual.** A host IRQ's ERET can land between the halves
  during record or replay, below anything retrace sees. The estimate is ~10⁻⁶ per sequence. For a
  discard-status pair it is a loud divergence, never a silent wrong recording. It stays a README
  residual.
- **Pairs straddling a page, and inference past 16 instructions.** No inference is made there, so
  today's behaviour stands: loud or a hang, never a wrong emulation. None is in the census.
- **A plain store of an identical value between the halves** (IMPLEMENTATION DEFINED natively), and
  the inference's other assumptions (§3d). They are residuals.
- **The M41 owed items other than the terminal park**: F2, the crashing-watched-store exit 5, the
  `?`-armed session (M43's), the blocking-boundary parity test, the Sys-park zero-step test, and
  `where`'s missing phase (M43's).
- **M40's `crc32` and `reverse-stepi` cost items.**

## 8. Rulings (made while writing this spec)

- **R1: design B** (a shadow monitor plus STX emulation) over design A (step the sequence as a unit,
  gdb-style).
  - A cannot keep K exact: `getpid`'s `cbnz` target is `stx + 4`, so one stop pc means either 2 or 3
    retired instructions.
  - A removes coordinates, and it changes `step()`'s contract.
  - A leaves the native-stop exposure (E2/E3) to B's machinery anyway.
  - B changes no coordinate semantics and touches neither record nor plain replay.
- **R2: the shadow is set from the step exit's ISS.EX bit (M8), not by decoding every stepped
  instruction.**
  - The decode happens only on an EX retire, cross-checked against the decoder, and while the shadow
    is set.
  - So the ordinary step path, which the hit oracle and the resolvers drive millions of times, pays
    nothing.
  - The byte, halfword and `ldaxp` forms are unmeasured. A form that fails to report EX would be
    stepped without a shadow. That is today's behaviour, loud or a hang, never a wrong emulation.
- **R3: inference at native stops, with the register-equality check (§3d).**
  - It is the one heuristic. It is kept, rather than re-stepping a whole window at every native stop
    in `reverse-continue`'s phase 1, because the latter would forfeit M40's native-speed scan on
    long windows.
  - The equality check makes most wrong inferences into no inference.
- **R4: fail loud by panic, from a pure validator.**
  - This is the box's convention, as in the `-33` arm.
  - The validator is a pure function, so every refusal branch has a unit test without a VM.
- **R5: the raised breakpoint stop's FAR is its pc.** No consumer reads it (code facts §4). The
  raised watch stop's FAR is **the lowest byte where the access overlaps an armed range**. That
  address lies both in the access and in the watch, so `watched_of` resolves it by its first,
  exact-byte rule.
  - Amended while planning (the plan's R11). The first draft used the access's start address, which
    resolves a watch on a pair's second element only through `watched_of`'s 64-byte-block fallback.
  - The hardware's report for a pair is unmeasured.
- **R6: the fixture keeps t0's four shapes verbatim** so t0's coordinates stay valid, and adds five.
  - (e), (f) and (g) are the clear rule's positive controls, one per class the rule names that a
    static fixture can reach: `clrex`, an emulated trap, a syscall.
  - The stage-2 class has no static-fixture reach (demand-commit needs a reservation). It shares the
    classifier's code and the census's dynamic-path shapes, and it is recorded as unexercised by a
    repo fixture.
- **R7: the `threadrust` oracle moves to landmark 1 only within ≤ 120 s CPU** (M41's budget), which
  the task measures. Otherwise R13 stands. Either way the `getpid` pairs are guarded by Q3.
- **R8: no format change.** `BoxState` is memory-only, the trace gains nothing, and `TRACE_MAGIC`
  stays.

## 9. Gate prediction

M41 closed at **666 / 0 / 9 over 141**. Expected additions:
- `retrace-arch`: ~8 decoder and barrier unit tests.
- `retrace-box`: ~8 validator unit tests, and 1 parity row, which changes no test count.
- `retrace-core` or `retrace-box`: ~3 session-level shadow tests: set, clear on (h), and carried by a
  checkpoint.
- `llsc_e2e`: +1 binary, ~22 tests (named regressions, controls, inference, three oracle armings with
  their self-checks).
- `threadrust`: ~1 Q3 test, in whichever existing file owns `threadrust`'s debugger tests.
- No `#[ignore]`.

**Prediction: ≈ 708 / 0 / 9 over 142**, reconciled file by file. It is a prediction, not a target.

## 10. Outcome

Filled at the close (Task 7).
