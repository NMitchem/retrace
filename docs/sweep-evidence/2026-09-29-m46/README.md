# Sweep evidence — M46 Task 6, run 2026-09-30

Two measurements live here:
- **The walk.** Step 1 took `automationmodetool` past libdispatch's second `kevent_qos` (M45's
  wall), now modelled by M46, to its next wall. That evidence is under "The walk" below.
- **The sweep.** The full 54-entry corpus was swept once on this task's binary and diffed row by
  row against M45's run (`docs/sweep-evidence/2026-09-28-m45/`). Every moved row was then
  re-measured on a base binary built from `f907c33` (the M45 merge), alternating with the swept
  binary.

**Result: `TALLY pass=47 fail=7 skip=0`,** against M45's `pass=49 fail=5`. Seven rows differ from
M45's. Five are explained below by measurement, and two, the watchdog kills, are attributed to host
load by elimination:

- **`automationmodetool`** is still `FAIL` 101/n/a. It now stops at the next wall: `kevent_id`(375),
  a libdispatch workloop thread request, which has no `arg_kinds` row and is outside M46 (§7 H5).
  M45 stopped it at the second `kevent_qos`. **This is the only move M46 made** (outcome B).
- **`/bin/[` moved from `PASS` 2/2 to `FAIL` 137/n/a (timed out after 30s recording).** Not
  reproduced on either binary; attributed to host load by elimination. The kept trace had already
  recorded the guest's own `Exit code=2` when the watchdog killed the recorder. All four controls
  pass 2/2, two on each binary.
- **`/bin/kill` moved from `PASS` 2/2 to `FAIL` 2/137 (timed out after 30s replaying).** Not
  reproduced on either binary; attributed to host load by elimination. The sweep's own recording
  replays clean, rp 2 with no divergence, in 6–16 s on both binaries. All four controls pass.
- **`/bin/ps` moved from `FAIL` 0/3 to `PASS` 0/0.** Host state. M45's `FAIL` was a class-E row, a
  page the host reclaimed, and M45 measured it as intermittent. Here it passes on both binaries.
- **`/usr/bin/dddiagnose` moved from `PASS` 139/139 (identical fault) to `FAIL` 4/3, msgh_id 205.**
  Host state. Both outcomes occur on both binaries, as M45 measured.
- **`csh` and `tcsh`** moved their landmark only: 336 → 338 and 336 → 340. The landmark is the
  guest's own `gettimeofday` count. On all eight traced samples, on both binaries, the trap count
  minus the `gettimeofday` traps is 317, the constant M45 measured.

**The two new `FAIL`s are watchdog kills that no control reproduced.** Both `[` and `kill` were
killed by the 30 s watchdog during the sweep, while an unrelated `cargo-mutants` run loaded the
12-CPU host: a 1-minute load of 35.31 at the sweep's start, and 15-minute averages of 24.85 at the
start, 27.40 at 00:46 and 23.81 at the end (`sweep.log`, `sweep-midpoint.txt`). Neither kill
reproduces on either binary in any control, so M46 is not shown to cause them, and load is the
attribution by elimination, not a measurement. The two other count moves cancel: `ps` went `FAIL` →
`PASS` and `dddiagnose` went `PASS` → `FAIL`. If the two timeouts are set aside, the tally matches
M45's 49/5.

**R7 reaches no moved row.** R7 rewrites `gettimeofday`'s mach-time out-parameter only when the call
passes one (`x2 != 0`). Every kept trace was read, from the sweep, the controls and Step 1: 26
traces and 16–26 `gettimeofday` events each. Not one of those calls has `x2 != 0` (`r7reach.txt`).
So no landmark in this sweep moved because of R7.

## Host state

An unrelated `cargo-mutants` run shared the host throughout. It is not this milestone's, and it was
not touched. The host has 12 CPUs (`sysctl hw.ncpu`). The logged load averages, as `uptime`
printed them into committed files:

| when | load averages (1 / 5 / 15 min) |
|---|---|
| sweep start, 00:19 (`sweep.log`) | 35.31 / 22.56 / 24.85 |
| mid-sweep, 00:46, row 30 of 54 (`sweep-midpoint.txt`) | 14.33 / 26.32 / 27.40 |
| sweep end, 00:50:29 (`sweep.log`) | 13.38 / 19.16 / 23.81 |
| controls, 00:51–00:58 (`controls.txt`, per round) | 10.71 → 8.13 → **34.77** → 16.96 (1-min) |
| traced samples, 00:59–01:01 (`samples.txt`) | 7.5–8.1, then 6.34 (1-min) |

`sweep-midpoint.txt` is the output of `t6-wait-rows.sh`, a bounded wait this task ran on the sweep
log. It was written at 00:46 and copied here unchanged in fix round 1.

**Unlogged observations.** Three more `uptime` readings were taken by hand during the sweep. They
were not written to any file, so they are observations, not evidence: 1-minute loads of 41.47 at
00:20, 42.61 at 00:21 and 50.40 at 00:43.

The sweep took 31 minutes. Load is the attributed factor in every timeout below. Most control rounds
started at 1-minute loads of 8–17, lower than the sweep's start. The exception is round 2's base
start, 34.77, close to the sweep's 35.31, and that is the round in which the base binary's own `ps`
timed out.

## Method

**The sweep** was `tools/apple-sweep.sh`, run by this task alone, detached, with no concurrent
`cargo` (M45 T3-a). The wrapper is `t6-sweep.sh`:

- It prints `pidstart`, the binary's hash, commit and date, and `uptime` before and after (the
  M38/M39/M44/M45 shape, plus the load lines).
- It ran a no-op `cargo build -p retrace` before the sweep started. Then it copied the worktree's
  `target/aarch64-apple-darwin/debug/retrace` to the session scratchpad as `retrace-t6`, and signed
  it there ad hoc with `retrace.entitlements`, so no build could swap it mid-sweep.
- After signing, its sha256 is
  `9e3007defe855dfe05d57361f3163c87ad136a4cfff3eb0c9ef3b74737bc987e`.
- The commit swept is `d369fcc` (M46 Task 5's fix round). The working tree also held this
  directory and the `apple_walls_e2e.rs` re-park, and neither is part of the binary.
- `RETRACE_SWEEP_KEEP=…/sweep` kept every non-clean row's `rec.err`/`rp.err`/`rp.out`/`bin`.
- The watchdog was the default, 30 s per phase, for comparability with M45.
- Log: `sweep.log`.

| `pidstart` | recpid range | `TALLY` | `SWEEP_EXIT` |
|---|---|---|---|
| 12446 | 12468–19096 (`0x30b4`–`0x4a98`) | `pass=47 fail=7 skip=0` | 0 |

**The base binary** is `retrace-base`, sha256
`1f42428f768c06c00f885e74c66cc3ccd248b9cacbc942ccd23783ade88b1599` after signing.
- It was built from `f907c33`, the M45 merge, extracted by `git archive f907c33 | tar -x -C
  /private/tmp/claude-501/m46-base`.
- It was built with its own target dir, `/private/tmp/claude-501/m46-base-target`
  (`t6-base-build.sh`).
- `f907c33` is an ancestor of `d369fcc`. `git diff --stat f907c33 d369fcc -- crates` shows 17 files
  changed, which are M46's Tasks 0–5.

**The controls** ran after the sweep, on the same host, alternating the two binaries:

- **`controls.txt`** (`t6-controls.sh`). The seven moved rows (`moved-rows.txt`) were re-swept by
  `tools/apple-sweep.sh` itself, through `RETRACE_SWEEP_LIST`. There were two rounds, in the order
  base, t6, base, t6. Each row is judged by the sweep's own labels and 30 s watchdog.
- **`samples.txt`** (`t6-samples.sh`), two rounds, alternating:
  - **A.** The sweep's own `kill.bin` replayed on both binaries, timed, bounded at 300 s.
  - **B.** M45's `csh`/`tcsh` sample: the traced trap count before the 3403 wall, and its
    `gettimeofday` share.
  - **C.** Traced `dddiagnose` records, bounded at 180 s. Each gives its trap count, its
    `gettimeofday` count, the trap number of the refused receive, and its outcome.
- **`timeouts.txt`** (`t6-timeouts.sh`): the last landmarks of each watchdog-killed run's kept
  trace.
- **`r7reach.txt`** (`t6-r7reach.sh`): per kept trace, the number of `gettimeofday` events, and the
  number with `x2 != 0`.
- **Reader.** `tracedump.rs` is a throwaway trace reader. It reads through
  `retrace_trace::Reader::open_checked`, and is built as its own crate in the scratchpad, depending
  on `crates/retrace-trace` by path. Its source is kept here.
- **No `cargo` ran** while the sweep or any control was recording. The reader was built before the
  sweep and rebuilt after the controls.

## The non-clean rows

| row | label | `rc`/`rp` | wall |
|---|---|---|---|
| `/bin/[` | `FAIL` (timed out after 30s recording) | 137 / n/a | **attributed to host load (not reproduced)**: the kept trace ends `#248 Exit code=2` with no final snapshot (below) |
| `/bin/csh` | `FAIL` (record error) | 4 / 3 | 3403 (`mach_ports_register` ← `fork`), class C (unchanged); landmark 338 |
| `/bin/kill` | `FAIL` (timed out after 30s replaying) | 2 / 137 | **attributed to host load (not reproduced)**: the recording is complete and replays clean on both binaries (below) |
| `/bin/tcsh` | `FAIL` (record error) | 4 / 3 | 3403, class C (unchanged); landmark 340 |
| `/usr/bin/automationmodetool` | `FAIL` (recorder panicked) | 101 / n/a | `M33: syscall 375 (375) has no arg_kinds row` — `kevent_id`, a workloop thread request, class C, M46 §7 H5 (the walk, below) |
| `/usr/bin/dddiagnose` | `FAIL` (record error) | 4 / 3 | `mach_msg2` msgh_id 205 (`host_get_io_main`), class C (unchanged); landmark 452 |
| `/usr/bin/yes` | `FAIL` (timed out after 30s recording) | 137 / n/a | never terminates; the watchdog, by design (unchanged) |

No row is an `identical fault` in this sweep.

## Row-by-row diff against M45

- `rowdiff.txt` is the brief's raw diff of the two runs' `ROW` lines. It marks all 54 lines, because
  the recorder's pid is a column and differs on every row.
- `rowdiff-norm.txt` is M45's normalised comparison (`t6-rowdiff.sh`, M45's `t3-rowdiff.sh`). It
  keys on the path and compares `result`, `rc`, `rp`, `landmark` and `rec_reason`, with every
  parenthesised number normalised out.
- **47 rows are identical and 7 differ.**

| row | M45 | M46 | moved by |
|---|---|---|---|
| `/usr/bin/automationmodetool` | `FAIL` 101/n/a, panic: `M45: unmeasured kevent_qos shape` | `FAIL` 101/n/a, panic: `M33: syscall 375 (375) has no arg_kinds row` | **M46**: the second `kevent_qos` is modelled, and the run reaches `kevent_id` |
| `/bin/[` | `PASS` 2/2 | `FAIL` 137/n/a, timed out recording | **attributed to host load (not reproduced)**, by elimination (below) |
| `/bin/kill` | `PASS` 2/2 | `FAIL` 2/137, timed out replaying | **attributed to host load (not reproduced)**, by elimination (below) |
| `/bin/ps` | `FAIL` 0/3, landmark 16044 (class E) | `PASS` 0/0 | **host state**: M45's intermittent reclaimed page (below) |
| `/usr/bin/dddiagnose` | `PASS` 139/139 (identical fault) | `FAIL` 4/3, 205, landmark 452 | **host state**: both outcomes on both binaries (below) |
| `/bin/csh` | `FAIL` 4/3, landmark 336 | `FAIL` 4/3, landmark 338 | the guest's own `gettimeofday` count (below) |
| `/bin/tcsh` | `FAIL` 4/3, landmark 336 | `FAIL` 4/3, landmark 340 | same |

`yes` is identical: 137/n/a, the watchdog.

### The controls, row by row

`controls.txt` gives each moved row on both binaries, two rounds each, in the order base, t6, base,
t6:

| row | base r1 | t6 r1 | base r2 | t6 r2 |
|---|---|---|---|---|
| `/bin/[` | PASS 2/2 | PASS 2/2 | PASS 2/2 | PASS 2/2 |
| `/bin/kill` | PASS 2/2 | PASS 2/2 | PASS 2/2 | PASS 2/2 |
| `/bin/ps` | PASS 0/0 | PASS 0/0 | **FAIL 137/n/a** (timed out recording) | PASS 0/0 |
| `/bin/csh` | 4/3, lm 337 | 4/3, lm 337 | 4/3, lm 336 | 4/3, lm 337 |
| `/bin/tcsh` | 4/3, lm 342 | 4/3, lm 339 | 4/3, lm 335 | 4/3, lm 338 |
| `automationmodetool` | 101, `M45: unmeasured kevent_qos shape` | 101, `M33: syscall 375` | 101, `M45: …` | 101, `M33: syscall 375` |
| `/usr/bin/dddiagnose` | 4/3, 205, lm 454 | **FAIL 137/n/a** (timed out recording) | 4/3, 205, lm 451 | PASS 139/139 (identical fault) |

**`automationmodetool` is the one row whose outcome follows the binary.** Both base runs stop at
M45's wall, and both t6 runs stop at `kevent_id`.

### `/bin/[` and `/bin/kill`: watchdog kills, not reproduced

Each row timed out once, in the sweep. That had never happened to either row: no earlier sweep
here (M38, M39, M44, M45) shows any timeout except `yes`.

**How far each killed run had got** (`timeouts.txt`):
- The sweep's `[.bin` holds 249 events, ending `#247 write_nocancel`, `#248 Exit code=2`, with **no
  final Snapshot**. So the guest had already run to its own `exit(2)`, the rc it gives on every
  other run, when the watchdog killed the recorder at its final full-memory snapshot. Nothing hung.
- The sweep's `kill.bin` is complete: `#243 Exit code=2`, then `#244 Snapshot`. Only its replay
  was killed.
- Two control runs were also killed by the watchdog, one on each binary. So a watchdog kill does
  not follow the binary:
  - **t6, round 1, `dddiagnose`.** Its trace ends `#390 Crash pc=0x180302eb0 esr=0x92000045`, the
    M45 `mfm_alloc` fault face, with no final Snapshot. The guest had reached its fault.
  - **base, round 2, `ps`.** Killed mid-run at landmark 7245 of its roughly 16043, in the round
    that started at a 1-minute load of 34.77. It was the **base** binary.

**The recordings are sound** (`samples.txt` A). The sweep's own `kill.bin` replayed on both
binaries, twice each, alternating:
```
A 1 base rp=2 secs=6 stdout=134 divergence-lines=0
A 1 t6 rp=2 secs=6 stdout=134 divergence-lines=0
A 2 base rp=2 secs=16 stdout=134 divergence-lines=0
A 2 t6 rp=2 secs=16 stdout=134 divergence-lines=0
```
At a load of about 8, the replay takes 6–16 s on either binary. In the sweep, which started at a
1-minute load of 35.31 (`sweep.log`), it took more than 30 s.

**Ruling.** Not reproduced on either binary; attributed to host load by elimination. Each row passes
4 of 4 controls, two on each binary, at logged 1-minute loads of 8–35, and the base binary was
never swept at the sweep's own load. So the cause is not measured. What supports load as the
attribution:
- the base binary has its own watchdog kill (`ps`, round 2), and so does t6 (`dddiagnose`,
  round 1);
- the sweep's `[` trace had reached the guest's own `Exit code=2`, `kill`'s recording is complete
  through `Exit` and its final Snapshot, and t6's `dddiagnose` kill came after its `Crash`: none of
  those kills cut off a guest that had not reached its terminal event. (The base `ps` kill, mid-run,
  is the exception, and it is on the base binary.)

### `/bin/ps`: back to `PASS`

M45's `FAIL` 0/3 was a final-snapshot divergence at `ipa 0x701414078`: a guest page that had been
`MADV_FREE_REUSABLE`d and that the host then reclaimed. M45 measured it as intermittent (20 of 20
clean control runs) and as reproducible on the base binary. Here it is:
- `PASS` in the sweep;
- `PASS` in both t6 controls;
- `PASS` in the first base control;
- in the second base control, killed by the watchdog under the load spike (above).

No reclaim was seen this time. The move is host state, and the hazard M45 named is unchanged.

### `/usr/bin/dddiagnose`: the M45 coin flip, both faces on both binaries

M45 measured two outcomes on both binaries: the msgh_id 205 stop, and the `mfm_alloc+0x230` data
abort at pc `0x180302eb0`. M45's sweep drew the fault. This sweep drew 205. The outcomes across the
controls and samples:

| binary | 205 (4/3) | fault (139) |
|---|---|---|
| base (f907c33) | controls r1, r2; samples C2 | samples C1 (`far=0x2000050060`) |
| t6 (d369fcc) | the sweep; samples C1, C2 | controls r2 (`PASS` 139/139); controls r1 (the fault, then the watchdog, above) |

`samples.txt` C gives each traced run:
- the refused receive at trap 380–384;
- 20–24 `gettimeofday` traps;
- the fault about 9 landmarks after the receive, or 205 at trap 452–455.

That is the shape M45 recorded. The move is host state. The gate's `#[ignore]` reason already
names both faces and is unchanged.

### `csh`/`tcsh`: gettimeofday

`samples.txt` B repeats M45's sample, on both binaries, alternating:
```
B 1 csh base rc=4 traps=338 gtod=21 traps-gtod=317
B 1 csh t6 rc=4 traps=334 gtod=17 traps-gtod=317
B 1 tcsh base rc=4 traps=341 gtod=24 traps-gtod=317
B 1 tcsh t6 rc=4 traps=337 gtod=20 traps-gtod=317
B 2 csh base rc=4 traps=338 gtod=21 traps-gtod=317
B 2 csh t6 rc=4 traps=334 gtod=17 traps-gtod=317
B 2 tcsh base rc=4 traps=335 gtod=18 traps-gtod=317
B 2 tcsh t6 rc=4 traps=343 gtod=26 traps-gtod=317
```
- The trap counts range 334–343 on both binaries.
- The count minus the `gettimeofday` traps is exactly **317** on all eight, the constant M45
  measured.
- **The landmark is the guest's `gettimeofday` count.**
- None of these calls passes a mach-time out-parameter (`r7reach.txt`, 0 calls with `x2 != 0` in
  every kept `csh`/`tcsh` trace). So R7 does not act on them, and the move is not M46's.

## The walk (Step 1)

**`automationmodetool`: outcome B, re-parked at M46 §7 H5.** The native rc is 0 (M45 t0 M4), and
the record exited 101, so no replay ran (`t6-step1.sh`, `automationmodetool.rec.err`, 365 `[trap]`
lines).

**Where M46's model carries the run** (`landmarks.txt`, the trace's own events). `landmarks.txt`
was produced by an earlier build of the committed `tracedump.rs`, which lacked only the final
`with-mach-time-out-param` count; every line it does print is unchanged in the later build:
- `#362` is `kevent_qos`, M45's init: rc 0.
- `#363` is `kevent_qos` with `x3 = 0x27fedb8`, `x4 = 16`, `x7 = 0x23`: the memory-pressure
  registration M45 refused. It **records, rc 0, no writes, thread 0.**
- `#364` is `mach_msg2` msgh_id 3409 (decoded in `automationmodetool.rec.err`: `which` = 10,
  `TASK_DEBUG_CONTROL_PORT`). The trace shows `#364 Syscall num=-47 … ret=0x0 … err=false writes=1
  thread=0`: one write, the reply. That reply answers `MACH_PORT_NULL` by M46 Ruling T0-a (Task 0:
  `task_get_special_port(TASK_DEBUG_CONTROL_PORT = 10)` answers `KERN_SUCCESS` with
  `MACH_PORT_NULL`, recomputed and byte-compared on replay). `landmarks.txt` shows the write count,
  not the reply's bytes.

**The wall is landmark 365**, the call after those, which is never appended:
- `kevent_id`(375), trap pc `0x1804afa74`;
- `thread 'main' (10935275) panicked at crates/retrace-arch/src/lib.rs:1002:38: M33: syscall 375
  (375) has no arg_kinds row in crates/retrace-arch/src/lib.rs — it cannot be forwarded
  unclassified …`

**Its arguments** (`automationmodetool.entry.txt`, read by `retrace debug` at the stub's svc, pc
`0x1804afa70`, position (365, 83), thread 0):
- `x0`, the id: `0x6bc90`.
- `x1` = `x3` = `0x27ff458`: one change, and a one-entry event list at the same address
  (`x2` = `x4` = 1).
- `x5` = `x6` = 0.
- `x7` = `0x403` = `KEVENT_FLAG_WORKLOOP|KEVENT_FLAG_ERROR_EVENTS|KEVENT_FLAG_IMMEDIATE`.

**Its change entry:**
- ident and udata: `0x6bc90`;
- filter: `0xffef`, which is -17, `EVFILT_WORKLOOP`;
- flags: `0x0005` = `EV_ADD|EV_ENABLE`;
- qos: `0x8ff`;
- fflags: `0x111` = `NOTE_WL_THREAD_REQUEST|NOTE_WL_UPDATE_QOS|NOTE_WL_IGNORE_ESTALE`;
- data and `ext[0]`: 0;
- `ext[1]`: `0x6bcc8`, which is the id plus `0x38`;
- `ext[2]`: `0x3700000001`;
- `ext[3]`: `0x1ffea400000001`. That equals the 8 bytes at `0x6bcc8`, the queue's state word, so
  it is a thread request conditioned on that word.

**Who calls it** (`automationmodetool.frames.txt`, `t6-frames.sh`):
- **The queue.** The id is a dispatch queue. The pointer at id + `0x48` names it:
  `com.apple.NSXPCConnection.m-user.com.apple.dt.automationmode.reader`.
- **The frame chain.** It was walked from `x29` at the svc, and each saved lr had its PAC bits
  stripped. It was symbolicated by lldb against the host's shared cache. The guest's libraries sit
  at the same addresses (`automationmodetool.kqpoll.txt`, `t6-kqpoll.sh`):
  - `x30`, `0x180359a64`, is `_dispatch_kq_poll+220`, the return address of the `bl kevent_id` at
    `_dispatch_kq_poll+216`;
  - M45's `kevent_qos` return, `0x180359a2c` (`x30` in M45's `automationmodetool.entry.txt`), is
    `_dispatch_kq_poll+164`, the return address of the `bl kevent_qos` at `+160`.

  The chain, each frame named by its return address:
  - libdispatch `_dispatch_kq_poll+220`
  - libdispatch `_dispatch_event_loop_poke+336`
  - libxpc `_xpc_connection_init_failed+352`
  - libxpc `_xpc_connection_init+744`
  - libxpc `_xpc_connection_activate_if_needed+544`
  - libxpc `xpc_connection_resume+104`
  - AutomationMode `__29+[XAMObserver sharedInstance]_block_invoke+232`
  - AutomationMode `-[XAMObserver currentAutomationModeEnabledStateFromDaemon]+52`
  - AutomationMode `__38-[XAMObserver isAutomationModeEnabled]_block_invoke+36`
  - libdispatch `_dispatch_client_callout+16`
  - libdispatch `_dispatch_lane_barrier_sync_invoke_and_complete+56`
  - AutomationMode `-[XAMObserver isAutomationModeEnabled]+132`
  - AutomationMode `XAMIsAutomationModeEnabled+64`
- **What it is.** libxpc pokes the NSXPCConnection's workloop from its init-failed path. Why the
  connection's init failed was **not traced**. The daemon it names is not a service retrace hosts,
  and the refused message-queue send and receive earlier in the run are candidates, not a
  measurement.

**Classification.**
- This is not H3. It is not a `kevent_qos` shape, it is not on the timer path, and it is not an
  `EVFILT_MACHPORT` registration.
- It is **H5**, a new subsystem: workloops and `kevent_id`(375), which M46 §7 names as "Not in
  M46".
- It is re-parked there and routed to its own milestone. Nothing was modelled.
- The sweep's row and all four controls reproduce the same stop:
  - the two t6 runs stop at `M33: syscall 375`;
  - the two base runs stop at M45's `kevent_qos` refusal.

**The gate.**
- `apple_walls_e2e::automationmodetool_records_and_replays` keeps its body and its `#[ignore]`, with
  the reason rewritten in the house form.
- `cargo test -p retrace --test apple_walls_e2e --no-fail-fast -- --test-threads=1`:
  `3 passed; 0 failed; 7 ignored`.
- The positive control is the same target with `automationmodetool -- --ignored`. It fails, and its
  panic names `M33: syscall 375 (375) has no arg_kinds row`.

## Ruling

Five moved rows are explained by measurement, and two are attributed by elimination:
- **`automationmodetool`** moved by M46. The memory-pressure registration now records, and the run
  reaches `kevent_id`, outside M46 (H5). In the controls, its outcome follows the binary.
- **`[`** and **`kill`**: watchdog kills, not reproduced on either binary, attributed to host load by
  elimination. The sweep started at a logged 1-minute load of 35.31, from an unrelated process.
  - The sweep's `[` trace had reached the guest's own exit, and `kill`'s recording is complete.
  - Each row passes 4 of 4 controls, two on each binary.
  - Both binaries have a watchdog kill of their own in the controls (base `ps`, t6 `dddiagnose`).
- **`ps`** moved by host state, M45's intermittent class-E reclaim not recurring.
- **`dddiagnose`** moved by host state, M45's coin flip: both faces occur on both binaries.
- **`csh`/`tcsh`** moved by the guest's `gettimeofday` count. `traps − gtod` is 317 on all eight
  samples on both binaries, and R7 does not act on any of those calls.

**No row is shown to have moved because of M46's diff other than `automationmodetool`.** No moved
row is left without an explanation, so none is H5 on that account, but the two timeouts rest on
elimination rather than a reproduced measurement. R7 changed nothing observable in this corpus: no kept
trace has a `gettimeofday` with a mach-time out-parameter.

## Files

- `sweep.log`: the full detached log. It holds the wrapper's header lines, `load-start`, 54 `ROW`
  lines, `TALLY pass=47 fail=7 skip=0`, `SWEEP_EXIT=0` and `load-end`.
- `sweep-midpoint.txt`: `t6-wait-rows.sh`'s output at 00:46, row 30 of 54, with its `uptime` line
  (added in fix round 1, unchanged).
- `rows.txt`: the sweep's `ROW` lines.
- `rowdiff.txt`: the brief's raw diff against M45's `ROW` lines.
- `rowdiff-norm.txt`: the normalised diff (47 identical, 7 differ).
- `sweep/<basename>.rec.err` for every non-clean row, with `.rp.err`/`.rp.out` where a replay ran.
  The rows are `[`, `automationmodetool`, `csh`, `dddiagnose`, `kill`, `tcsh` and `yes`.
- `moved-rows.txt`: the seven moved rows, the controls' list.
- `controls.txt`: the alternating re-sweep of the moved rows.
- `samples.txt`: the replayed `kill.bin`, the `csh`/`tcsh` sample, and the traced `dddiagnose`
  runs.
- `timeouts.txt`: the last landmarks of each watchdog-killed run.
- `r7reach.txt`: the `gettimeofday` counts per kept trace, and those R7 acts on.
- The walk: `automationmodetool.{rec.err,rec.out,entry.txt,frames.txt,kqpoll.txt}` and
  `landmarks.txt`.
- The scripts, with the session's scratchpad paths left as they ran:
  - `t6-step1.sh`, the brief's Step 1;
  - `t6-wall-entry.sh`;
  - `t6-frames.sh`;
  - `t6-base-build.sh`;
  - `t6-sweep.sh`;
  - `t6-rowdiff.sh`;
  - `t6-controls.sh`;
  - `t6-samples.sh`;
  - `t6-timeouts.sh`;
  - `t6-r7reach.sh`;
  - `t6-wait-rows.sh` and `t6-kqpoll.sh`, both added in fix round 1.
- `tracedump.rs`: the throwaway reader's source.
- **No `.bin` trace files are committed.**
  - The sweep kept seven. They were read by `timeouts.txt` and `r7reach.txt`, then removed.
  - The controls' traces lived in the scratchpad and were removed there.
  - The walk's trace, `/private/tmp/claude-501/m46-amt.bin`, was removed after `landmarks.txt`,
    `entry.txt`, `frames.txt` and `r7reach.txt` had read it.
