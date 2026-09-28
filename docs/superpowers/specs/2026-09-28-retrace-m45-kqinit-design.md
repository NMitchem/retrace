# M45-kqinit: libdispatch's workqueue-kqueue initialisation, emulated

**Date:** 2026-09-28. **Branch:** `worktree-m45-kqinit`, to be cut from `main` at the M44 merge
(`60f0452`, pushed 2026-09-28). **Companion:** `2026-09-28-retrace-m45-kqinit-measurements.md`,
written by t0 (§3a) and cited by section once it exists. Until then every claim below says where it
comes from: **SDK** (`MacOSX.sdk/usr/include/sys/event.h`, read 2026-09-28), **xnu**
(`bsd/sys/event_private.h`, the copy M44 t0 fetched into its ledger), **code** (read, with a path),
**measured** (M44 t0 M1, with its evidence file), or **inferred**. An inferred claim names the t0
measurement that owes it.

**Approach:** chosen by the operator in brainstorming on 2026-09-28. **Target:** the `kevent_qos`
workqueue case, the item M44 routed to its own milestone (status log, M44 "What stays owed", first
bullet; Ruling R4). **Scope:** "init + walk". Emulate the one measured shape as a stateless,
validated success, refuse every other shape by value, and walk the guests to their next wall.
**Mechanism:** approach A of three. A stateful knote table (B) was rejected as state nothing reads.
Forwarding with translation (C) was rejected as unsound (§2c).

## 1. Purpose

`/usr/bin/automationmodetool` is parked at `kevent_qos` (374)
(`crates/retrace/tests/apple_walls_e2e.rs:75`, its `#[ignore]` reason). The call is not a missing
row. It is libdispatch's `_dispatch_kq_init`, which registers the event manager's `EVFILT_USER`
wake-up on the **process's workqueue kqueue**. Any guest whose libdispatch needs the manager reaches
it, for a timer, a source or an XPC channel. M45 makes that one call succeed, deterministically and
without touching the host, then measures what each guest reaches next.

**Success** is not "automationmodetool records". It has four parts:

1. The measured init shape records and replays bit-identically in a repo-owned guest, on any
   machine.
2. Every other `kevent_qos` shape stops the recorder with a message naming the field, the measured
   value and the actual value.
3. `automationmodetool` is either un-ignored or re-parked at a **new, measured** wall, with its
   `#[ignore]` reason rewritten.
4. Each GCD candidate t0 tries is either gated or recorded as owed, with its measured next call.

## 2. What is known before t0

### 2a. The call (measured, M44 t0 M1)

Evidence: `docs/sweep-evidence/2026-09-27-m44-t0/m1-automationmodetool.err` and the M44 ledger's
`task-0-report.md` §M1.

```
[trap] num=374 pc=0x1804afa48 args=[0xffffffff,0x27ff348,0x1,0x0,0x0,0x0,0x0,0x21]
```

The signature is `int kevent_qos(int kq, const struct kevent_qos_s *changelist, int nchanges,
struct kevent_qos_s *eventlist, int nevents, void *data_out, size_t *data_available, unsigned int
flags)` (xnu). The measured arguments:

| reg | parameter | value | meaning |
|---|---|---|---|
| x0 | `kq` (int) | `0xffffffff` | −1: no descriptor; zero `kqueue` (362) calls in the trace |
| x1 | `changelist` | `0x27ff348` | a guest stack address |
| x2 | `nchanges` (int) | 1 | |
| x3, x4 | `eventlist`, `nevents` | 0, 0 | no events requested |
| x5, x6 | `data_out`, `data_available` | 0, 0 | |
| x7 | `flags` (unsigned) | `0x21` | `KEVENT_FLAG_WORKQ` `0x20` (xnu `event_private.h:141`) \| `KEVENT_FLAG_IMMEDIATE` `0x1` (SDK `event.h:132`) |

The one change-list entry, as t0 M1 decoded it: `ident` 1, `filter` −10 (`EVFILT_USER`, SDK
`event.h:77`), `flags` `EV_ADD|EV_CLEAR` (`0x1|0x20`, SDK `event.h:136,143`), `qos` `0x02000000`
(`_PTHREAD_PRIORITY_EVENT_MANAGER_FLAG`), `udata` `~0x7` = `0xfffffffffffffff8`
(`DISPATCH_WLH_MANAGER`). That is byte-for-byte libdispatch's `_dispatch_kq_init`
(`event_kevent.c:689-699`).

**Not measured:** `fflags`, `xflags`, `data` and `ext[0..4]`. libdispatch builds the entry with a
designated initializer that names only the five fields above, so the rest are **inferred** zero.
t0 M1 (§3a) owes the full 72-byte dump.

### 2b. The layout (xnu `event_private.h:115-125`)

`struct kevent_qos_s` is 72 bytes, with no padding:

| offset | field | type |
|---|---|---|
| 0 | `ident` | u64 |
| 8 | `filter` | i16 |
| 10 | `flags` | u16 |
| 12 | `qos` | i32 |
| 16 | `udata` | u64 |
| 24 | `fflags` | u32 |
| 28 | `xflags` | u32 |
| 32 | `data` | i64 |
| 40 | `ext[4]` | u64 × 4 |

The table was read from the header during brainstorming. An offset stated in chat, `fflags` at 12,
was wrong, and this table corrects it.

### 2c. Why neither a row nor a refusal answers it (measured, M44 t0 M1; M44 R4)

- **Forwarding is unsound.** With `KEVENT_FLAG_WORKQ`, xnu's `kevent_qos` goes to
  `kevent_get_kqwq` and then `p->p_fd.fd_wqkqueue`, allocating it if absent
  (`kern_event.c:8430-8431`, `:7404-7420`). That kqueue belongs to **retrace's own process**. This
  is the class M18 forbade for the workqueue pair
  (`crates/retrace-core/src/lib.rs:1222`, the forward arm's assert). It is also the fourth instance
  of the recurring bug the box's comments name: the guest's X was retrace's X
  (`crates/retrace-box/src/lib.rs`, `guest_workq_open`'s doc).
- **A refusal is unusable.** libdispatch `DISPATCH_CLIENT_CRASH`es on any errno but `EINTR`
  (`event_kevent.c:700-709`). An errno would only move the crash into the guest and hide its cause.
- **The call needs a modelled success**, and the smallest one is a constant. With
  `KEVENT_FLAG_IMMEDIATE` and no event list, a successful call places zero events and returns 0.
  This is **inferred** from xnu; t0 M3 owes the native return.

### 2d. What the box already does next to it (code)

The workqueue pair is emulated above the trace. The record arms are at
`crates/retrace-core/src/lib.rs:1033` and `:1044`. The replay mirrors are at `:2202` and `:2211`,
inside the `Syscall` arm's `if num ==` chain, which has already called `verify_thread`, so the
mirrors inherit the thread oracle. `Box_::guest_workq_kernreturn`
(`crates/retrace-box/src/lib.rs:4969`) refuses every unmeasured opcode **by value**, with a panic
naming what to measure. M45 copies that stance and that placement.

## 3. Design

### 3a. t0: measurements first

t0 runs on the branch before any product code and writes the companion file.

- **M1, the full entry.** Run `record-dyn /usr/bin/automationmodetool` under `RETRACE_TRACE=1`
  with a throwaway dump of all 72 bytes at `x1` and all eight arguments. The throwaway is removed
  before t0 commits. **Halt 1** applies if any field differs from §2a, or if an unmeasured field is
  non-zero.
- **M2, GCD candidates.** Build three throwaway C guests: a `DISPATCH_SOURCE_TYPE_TIMER` source,
  `dispatch_after`, and a `DISPATCH_SOURCE_TYPE_SIGNAL` source. Each writes a marker and exits.
  Record each under `RETRACE_TRACE=1` on the unmodified branch, where 374 still panics by name, so
  the trap line shows each candidate's 374 arguments without any emulation. M1's throwaway dump
  shows the entry. Note which candidates reach 374 and whether their shape equals M1's. A candidate
  that reaches a different shape is recorded, not modelled (**Halt 2**).
- **M3, the native return.** Build the §3e mechanism fixture and run it **natively** (not under
  retrace), printing the call's return and carry flag. Expected: 0, carry clear (§2c).
- **M4, the baseline.** The M44 close's counts (832/0/9 over 146), re-derived from source by the
  file-by-file method, so §9's prediction starts from a measured floor.

### 3b. The validator (`retrace-arch`)

`retrace-arch` stays zero-dependency. It gains:

- Named constants, each with a citation:
  - `SYS_KEVENT_QOS = 374`
  - `EVFILT_USER`, `EV_ADD`, `EV_CLEAR`, `NOTE_TRIGGER` (SDK)
  - `KEVENT_FLAG_IMMEDIATE` (SDK), `KEVENT_FLAG_WORKQ` (xnu)
  - `KEVENT_QOS_SIZE = 72`
  - the §2b offsets
- `pub const KQINIT_ENTRY: [u8; 72]`: the measured entry, built from named fields, with provenance
  per field.
- `pub fn kqinit_shape(args: [u64; 8], entry: &[u8]) -> Result<(), String>`. This is a pure
  function; `Ok` means "this is the measured init". It checks, in order, and the first mismatch
  becomes the `Err`, naming the argument or field, the measured value and the actual value:
  1. `args[0] as u32 == 0xffff_ffff`
  2. `args[2] as u32 == 1`
  3. `args[3] == 0 && args[4] as u32 == 0`
  4. `args[5] == 0 && args[6] == 0`
  5. `args[7] as u32 == KEVENT_FLAG_WORKQ | KEVENT_FLAG_IMMEDIATE`
  6. `entry.len() == 72`
  7. `entry == KQINIT_ENTRY`, reported by the first differing field, not the first differing byte

  `int` and `unsigned int` parameters are compared on their **low 32 bits**, because the kernel
  reads 32 bits (M38's `AT_FDCWD` lesson: the recorded register was `0xffffffff`, not
  `0xffffffffffffffff`). Pointers are compared on 64.

The entry is compared **exactly**, including `ident`, `udata` and `qos`, which brainstorming
proposed to accept as-is (Ruling R1).

### 3c. The box method (`retrace-box`)

`pub fn guest_kevent_qos(&mut self, args: [u64; 8]) -> u64`. It sits beside
`guest_workq_kernreturn` and carries a doc comment in that style: what it emulates, why it is never
forwarded, and what a refusal means.

1. Read `read_va_prefix(args[1], 72)` (`crates/retrace-box/src/lib.rs:5877`). It walks the guest's
   own stage-1 tables per 16 KiB page, so an entry straddling a page is read whole.
   - A short read, meaning the entry does not fully translate, is passed to `kqinit_shape`. That
     call refuses it at item 6.
   - It is never an errno, for §2c's reason.
2. `kqinit_shape(args, &bytes)`. On `Err(msg)` it panics with an "M45:" prefix, the message, and
   what the unmeasured shape needs (a measurement, per the refuse-by-value stance). On `Ok` it
   returns 0.

It keeps no state and writes no guest memory. Everything it reads (guest memory and `args`) is held
identically by record and replay.

### 3d. The arms (`retrace-core`)

- **Record.** A `Stop::Syscall { num, args } if num == SYS_KEVENT_QOS` arm goes directly after the
  `SYS_WORKQ_KERNRETURN` arm (`:1044`). It calls `b.guest_kevent_qos(args)` and appends
  `Event::Syscall { num, args, ret: rc, ret1: 0, err: false, writes: vec![], thread }`, followed by
  `set_x0_err_and_return(rc, false)`. It has the same comment shape as the workqueue arms: why
  `writes` is empty, and why the byte-compare is the oracle.
- **Replay.** An `if num == SYS_KEVENT_QOS` mirror goes directly after the `SYS_WORKQ_KERNRETURN`
  mirror (`:2211`). It makes the same call with the same arguments, returns a
  `Divergence { detail: "kevent_qos rc mismatch …" }` on a return-code mismatch, and ends with
  `set_x0_err_and_return(*ret, *err)` and `finish_event()`. **No `verify_thread` of its own:** this
  position inherits the arm-top call, as the comment at the workqueue mirrors states. Adding one
  would double-check and mislead the next reader about the count of seven.
- **Forward arm.** The assert at `:1222` gains `SYS_KEVENT_QOS` beside the workqueue pair, with its
  message widened to match. It is an `assert!`, kept in release, and it makes "never forwarded" a
  checked fact rather than an arm-ordering accident (the gap M37 measured for `bsdthread_create`).
- **Documentation row.** `374 => row!(P, [Scalar, Ptr, Scalar, Ptr, Scalar, Ptr, Ptr, Scalar])` goes
  in `arg_kinds`'s "threads / workqueue (emulated above the trace …) rows are documentation" section
  (`crates/retrace-arch/src/lib.rs:708-737`). Its comment names the emulation.
  - Whether a documentation row needs a census or `legacy_equivalence` entry is for the plan to
    settle by reading those tests. **Inferred:** it does, as 367/368 have.
  - The `kqueue` row's comment (`:806-808`, "No kevent spelling … is in the census") is corrected to
    say 374 is emulated and 363/369/375 remain row-less.

No trace-format change and no `TRACE_MAGIC` bump. No `Event` shape changes, no snapshot byte changes
meaning, and 374 has never been appended to any trace: before M45 it panicked at M33's row check
(`crates/retrace-arch/src/lib.rs:984`) before any append (Ruling R6).

### 3e. The mechanism fixture and its gate

- **The fixture:** `crates/retrace-guest/c/kqinit_dyn.c`, built by `build.rs` like its siblings.
  `main` builds `KQINIT_ENTRY`'s bytes on its stack and issues 374 through inline
  `svc #0x80` with `x16 = 374`, `w0 = -1` (so `x0` reads `0xffffffff`, as measured),
  `x2 = 1`, `x3`–`x6 = 0` and `w7 = 0x21`. It captures `x0` and the carry flag, writes a marker
  line carrying both, and exits 0.
  - Inline `svc`, not `syscall()`: libSystem's `syscall()` goes through the indirect `SYS_syscall`
    (0), so retrace would see syscall 0, not 374 (Ruling R4).
  - An argv switch selects the **refusal mode**, which issues the same call with the entry's
    `flags` set to `EV_ADD|EV_CLEAR|EV_ENABLE`. The refused value is one field away from the
    measured one, so the refusal must name `flags`.
- **The gate:** `crates/retrace/tests/kqinit_e2e.rs`, three tests, spawning the CLI with the
  `util::bin()` codesign pattern.
  - `kqinit_records_and_replays`:
    - the trace holds exactly one `Event::Syscall` with `num == 374`, and it has `ret == 0`,
      `err == false`, empty `writes` and `args[7] == 0x21`;
    - the marker line reports rc 0 and carry clear;
    - two replays are byte-identical to the recording in stdout and exit.

    **Assert on the trace, not the exit code:** a fixture that never reached 374 would exit 0 too.
  - `kqinit_refuses_unmeasured_shape`: the recorder fails, and its stderr carries `M45:`, the
    **entry's** field name `flags`, the measured `0x21` (`EV_ADD|EV_CLEAR`, which happens to equal
    the syscall's own `flags` value) and the actual `0x25`. The trace holds no 374 event. This is
    `exec_e2e`'s pattern (`crates/retrace/tests/exec_e2e.rs:22`), asserting the refusal line.
  - `kqinit_replay_recomputes_rc`: record, rewrite the 374 event's `ret` from 0 to 1 in a copy of the
    trace (`thread_oracle.rs`'s `Reader` → `Writer` retag pattern), and replay the copy. Replay must
    fail with a divergence whose detail carries `kevent_qos rc mismatch`, at the 374 landmark.
    - This is the only test that can see the replay mirror. For a constant return the mirror's
      byte-compare is otherwise vacuous, as the existing mirrors' own comment says ("it is vacuous
      while the return is a constant", `crates/retrace-core/src/lib.rs`, the M18 t5 mirror). Without
      the mirror, generic replay would feed the tampered 1 to the guest in silence.
- **Able to fail (controls, not committed).** Each is run once, and its red is recorded in the
  ledger:
  - **The record arm deleted:** the first test goes red, because the record reaches the forward
    arm's new assert naming 374.
  - **The replay mirror deleted:** the third test goes red, because replay accepts the tampered
    return.
  - **`kqinit_shape`'s item 7 deleted:** the second test goes red, because the recorder accepts
    `0x25`.

  Deleting the mirror does **not** turn the first test red. That is expected for a constant, not a
  gap in the first test.

### 3f. The walk

Task-level, after §3b–§3e land:

- **automationmodetool.**
  - Record and replay it under `RETRACE_TRACE=1`.
  - **Outcome A:** clean and bit-identical. Un-ignore the gate. The sweep moves to 50/54, confirmed
    by the sweep's own run, with no count carried forward unmeasured.
  - **Outcome B:** a new first failure. Re-park the gate with the rewritten reason: the call or
    trap, its pc, its landmark, its record and replay exit codes, its evidence file, and "UN-IGNORE
    when …". Route it in the status log. If that failure is another `kevent_qos` shape, or
    `kevent_id` (375), it is **not** modelled here (**Halt 3**).
- **GCD candidates.** Each t0 M2 candidate whose 374 equals M1's is recorded under the landed
  emulation.
  - A candidate that runs to a clean, bit-identical exit becomes a second gate in `kqinit_e2e.rs`,
    making three tests. It asserts the same one-event shape and the candidate's marker.
  - A candidate that stops is recorded in the status log's owed list with its measured next call.
  - If none completes, the mechanism gate (§3e) still carries the milestone, and the owed list says
    so plainly.

## 4. Guards: each asserts the difference it makes

| guard | fails if | would a weaker failure also pass it? |
|---|---|---|
| `retrace-arch/tests/kqinit.rs`: measured shape accepted | the validator rejects the real call | no |
| … every single-bit flip of the 72-byte entry refused (576 cases, exhaustive) | a field is unchecked | no: exhaustive over the entry |
| … each `int` argument's upper 32 bits ignored; each low bit refused | the validator reads the wrong width | no: both directions |
| … each refusal names its field | a refusal is unattributable | no |
| … short or empty entry refused at item 6 | an untranslatable change list slips through | no |
| `kqinit_e2e`: one 374 event, rc 0, no writes, `args[7] == 0x21`, 2 byte-identical replays | the arm is missing, forwards, or diverges | no: a forward would carry writes or a different rc, and a missing mirror diverges |
| `kqinit_e2e`: refusal names `flags` 0x21 vs 0x25 | the refusal is silent or mis-attributed | no |
| `kqinit_e2e`: a trace with `ret` rewritten to 1 diverges with `kevent_qos rc mismatch` | the replay mirror is missing or does not compare | no: without the mirror replay feeds the 1 in silence |
| forward-arm assert includes 374 | a future arm reordering forwards 374 | it is the guard |
| `automationmodetool_records_and_replays` | outcome A regresses | n/a when re-parked (outcome B) |

## 5. Task order and why

1. **t0**: measurements M1–M4, and the companion file. Nothing depends on a guess.
2. **The validator** and its exhaustive tests, pure and VM-free, with no dependency.
3. **The box method, the arms, the documentation row and the forward assert** land together as one
   task, because none is testable alone.
4. **The mechanism fixture and `kqinit_e2e`**, plus the able-to-fail controls.
5. **The walk**: automationmodetool and the GCD candidates, then gate moves and reasons.
6. **Docs.** README edited in place, status log appended, CLAUDE.md edited (§6).
7. **The gate**: chunked and reconciled, then the whole-branch review, the fix wave and the merge.

## 6. Acceptance

1. §4's guards are all present and green, and the two able-to-fail controls were measured red.
2. `automationmodetool` has outcome A or B (§3f), with evidence under
   `docs/sweep-evidence/2026-09-28-m45/`.
3. Each GCD candidate is gated or listed as owed with its measured next call.
4. The README is edited in place:
   - "What works today": libdispatch's workqueue-kqueue init is emulated.
   - "Known limits": every other `kevent_qos` shape is refused by value; 363/369/375 have no row;
     any GCD owed items.
   - The sweep line changes only if outcome A.
5. `docs/status-log.md` gains an appended M45 section: t0's measurements, the rulings, the walk,
   the gate and "What stays owed".
6. CLAUDE.md:
   - "Guest threads" gains `kevent_qos` beside the workqueue pair.
   - The forward arm's assert list sentence gains 374.
   - The e2e list gains `kqinit_e2e`.
7. The chunked gate is green, including the `--bins` chunk and every split library crate's `--doc`,
   and reconciled file by file against t0 M4 (§9).
8. The whole-branch review has run and its fix wave is applied, then a `--no-ff` merge into local
   `main` with the tree-identity check. Push and worktree cleanup are the operator's.

## 7. Halt rules, and what this milestone deliberately does not do

**Halt** means stop, write the measurement to the ledger, and report to the operator. Do not widen
the design to get past it.

1. t0 M1's full entry or arguments differ from §2a, or an unmeasured field is non-zero.
2. A GCD candidate's 374 differs from M1's shape. This halts widening, not the milestone: the
   candidate is recorded in the companion file and is neither gated nor modelled. The milestone's
   premise rests on M1 alone, which Halt 1 covers.
3. The walk's next wall is another `kevent_qos` shape or `kevent_id`. It is re-parked and routed.
   This is not a halt of the milestone, only of widening.
4. Any step would need a trace-format change or a `TRACE_MAGIC` bump.
5. A gate red that M45's diff does not explain.

**Not done here:**
- `EVFILT_USER` triggers (`NOTE_TRIGGER`)
- `EVFILT_TIMER` / `EVFILT_MACHPORT` knotes
- delivering events to workers (a `THREAD_RETURN` that carries a kevent list; see
  `guest_workq_park`'s refusal)
- any knote state
- `kevent` (363), `kevent64` (369) and `kevent_id` (375)
- `kevent_qos` on a guest `kqueue()` descriptor
- the other M44 owed items

## 8. Rulings (made while writing this spec)

- **R1: the entry is compared exactly**, all 72 bytes, including `ident`, `udata` and `qos`.
  Brainstorming accepted those three as-is on the grounds that they do not change what a no-op
  registration means. One shape has been measured, and every other shape is unmeasured by
  definition. The refuse-by-value stance (`guest_workq_kernreturn`: "the raw values are the
  measurement") admits only the measured bytes. A future libdispatch that changes them should stop
  the recorder and be measured, not pass silently.
- **R2: `int` parameters are compared on 32 bits and pointers on 64** (§3b).
- **R3: stateless** (approach A). B's table would be state nothing reads until a milestone that
  models triggers, and that milestone can add it with its own measurement.
- **R4: the fixture uses inline `svc`** (§3e).
- **R5: a refusal is a panic, not an errno** (§2c). An errno is libdispatch's crash with the cause
  hidden, whereas the panic names it.
- **R6: no `TRACE_MAGIC` bump** (§3d).

## 9. Gate prediction

This is written against M44's close, 832 passed / 0 failed / 9 ignored over 146 test binaries. t0 M4
re-derives that floor from source before the prediction is pinned.

- **New binaries:** 2 (`retrace-arch/tests/kqinit.rs` and `retrace/tests/kqinit_e2e.rs`), giving
  **148**.
- **New passing tests:** the validator file's count, which the plan pins by enumerating its tests;
  plus 3 in `kqinit_e2e`; plus 0 or 1 for a GCD candidate gate; plus 1 if automationmodetool has
  outcome A. The documentation row may add census rows but not tests (inferred, §3d).
- **Ignored:** 9, or 8 with outcome A.
- **Failed:** 0.

The plan turns this into one number per outcome before the gate runs. The gate is reconciled file by
file against it, not by the sum.

## 10. Conformance with the governing documents

The operator's standing condition: the spec must abide by the design documents. retrace's own are
the founding design (`2026-07-05-retrace-macos-record-replay-design.md`) and the rules in CLAUDE.md.
The founding design says it is built "in the charpente tradition", so the charpente `MANIFESTO.md`
and `DESIGN.md` (repository `charpente`, a sibling project) are checked too.

- **Founding design, component 2:** "we do not model syscall semantics … special-case only the
  handful that change the memory map or return fresh capabilities."
  - M45 models one call, which is a departure, and this section names it rather than hiding it.
  - The justification is the class retrace established at M14 and M18. Forwarding
    `bsdthread_create` or the workqueue pair acts on the **recorder's own process**, so those calls
    must be emulated, and `kevent_qos` with `KEVENT_FLAG_WORKQ` is measured to be in that class
    (§2c).
  - The model is the smallest possible, a constant 0, for exactly one measured shape.
- **Founding design, the determinism oracle:** "replay must reproduce the recording bit-for-bit".
  The mirror recomputes and byte-compares its return, and a tampered trace proves the comparison is
  live (§3e). The e2e gate replays twice.
  - The negative-space assertion "no syscall ever really executes during replay" holds, since the
    mirror executes nothing.
  - The forward arm's `assert!` for 374 is a negative-space assertion **kept on in release**, as
    the founding design requires.
- **Founding design, the nondeterminism surface:** nothing is added. The return is a constant, and
  the only input is guest memory both sides hold identically. No clock, entropy or host state is
  read.
- **CLAUDE.md, symmetry rule 1:** both arms, the same `Box_` method, the same arguments, both before
  the generic arm. **The thread oracle's count:** unchanged at seven, because the mirror inherits
  the arm-top call (§3d). **Honest-gate discipline:** automationmodetool is un-ignored or re-parked
  with a measured reason, the gate asserts the trace, not an exit code, and no skip is added.
- **charpente MANIFESTO §2, "agent output quality tracks oracle density":** five independent oracles
  guard the change, each shown able to fail by a recorded control (§3e):
  - an exhaustive single-bit-flip sweep of the validator
  - the trace assertion
  - the refusal-line assertion
  - the rewritten-return divergence, which alone makes the replay mirror observable
  - the forward arm's assert
  
  The one oracle that is vacuous here, the mirror's return-code byte-compare on an honest trace, is
  named as vacuous rather than counted.
- **MANIFESTO §3, "how fast and how precisely the framework says *no*":** a refusal names the field,
  the measured value, the actual value and the measurement owed, and it stops at the call, not
  later in the guest.
- **MANIFESTO §8, "no feature flags for determinism … optional guarantees are no guarantees":**
  there is no environment variable, fallback to forwarding, or "lenient" mode.
- **charpente DESIGN §1, the deterministic core:** no wall clock, no host threads, nothing ambient.
  `clippy.toml`'s two denials stand untouched.

The rest of charpente's DESIGN (storage, replication, auth, the typed web surface) governs a backend
framework and has no retrace analogue. It is not applicable, and it is not being skipped.
