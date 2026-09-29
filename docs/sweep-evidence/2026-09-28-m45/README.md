# Sweep evidence — M45 Task 3, run 2026-09-28

Two measurements live here:
- **The sweep.** The full 54-entry corpus was swept once on Task 3's binary and diffed row by row
  against M44's run (`docs/sweep-evidence/2026-09-27-m44/`).
- **The walk.** Task 3's Steps 1–3 took `automationmodetool` and the t0 M2 GCD candidates past the
  emulated `kevent_qos` init. That evidence is described under "The walk" below.

**Result: `TALLY pass=49 fail=5 skip=0`.** That is M44's tally, but it does not come from the same
rows. Five rows differ from M44's, and every one is explained below by measurement:

- **`automationmodetool`** is still `FAIL` 101/n/a. It now stops at the next wall, a second
  `kevent_qos` shape that M45 refuses by design (§7 Halt 3), where M44 stopped at M33's missing row
  for 374. This move is M45's, and it was expected (outcome B).
- **`/bin/ps` moved from `PASS` 0/0 to `FAIL` 0/3.** It is a **class-E row**: the recorder finished
  cleanly and the replay disagreed at the final full-memory compare. The cause is **host state**:
  - the final snapshot's page map reads as the host kernel having reclaimed one 16 KiB page that
    the guest had `MADV_FREE_REUSABLE`d, three landmarks before exit. That is a fingerprint reading
    (below), not an observed reclaim;
  - this is the owed hazard M37 named, measured for the first time;
  - it is not M45.
- **`dddiagnose` moved from `FAIL` 4/3 (msgh_id 205) to `PASS` 139/139**, an identical fault. The
  cause is **host state**: from run to run it takes either outcome, on the base binary as well.
  - The fault is M36's libsystem_malloc `mfm_alloc+0x230` face.
  - Its return **contradicts M37's retirement** of that face.
  - It is M38 R3's losing `MACH_RCV_TIMED_OUT` face, now seen under the winning code, so R3 is
    owed a re-measure.
- **`csh` and `tcsh`** moved their landmark only (338 → 336 and 333 → 336). Both new values sit
  inside M44's measured `gettimeofday` spread, and fresh samples here repeat it.

The two count moves cancel: `ps` went `PASS`→`FAIL` and `dddiagnose` went `FAIL`→`PASS`, in the
sweep's sense of `PASS`. The tally equals M44's, and outcome B's expected `pass=49 fail=5`, by
coincidence, not because nothing moved.

## Method

**The sweep** was `tools/apple-sweep.sh`, run by the controller and wrapped in `t3-sweep.sh`:

- The wrapper prints its `pidstart` and the binary's hash, commit and date first (the M38/M39/M44
  shape).
- The worktree's `target/aarch64-apple-darwin/debug/retrace` was copied to the session scratchpad as
  `retrace-t3` and ad-hoc signed there with `retrace.entitlements`, so no concurrent `cargo test`
  could swap it.
- After signing, its sha256 is
  `5c3c0be60e5dbd03309d63c49d16bb4297b21aedfa9b31fa7bbf20e6301862f0`.
- The commit swept is `09a105f`. It touches only `crates/retrace/tests/apple_walls_e2e.rs` and this
  directory, so the swept record and replay paths are Task 2's `28fddc4`.
- **The swept binary predates one text-only change.** Task 3's fix round 1 is the commit that adds
  this sentence. It changes only how `guest_kevent_qos`'s refusal formats `args`:
  - it is now one line, `args=[0xffffffff,0x27ff298,…]`, where it was a multi-line `{:#x?}` list;
  - its prefix, its field clause and its panic site `crates/retrace-box/src/lib.rs:5222:13` are
    unchanged;
  - the record and replay paths are unchanged.

  So `automationmodetool`'s `rec.err` in `sweep/` and in the walk's files shows the multi-line form.
- `RETRACE_SWEEP_KEEP=…/sweep` kept every non-clean row's `rec.err`/`rp.err`/`rp.out`/`bin`.
- Log: `sweep.log`.

| `pidstart` | recpid range | `TALLY` | `SWEEP_EXIT` |
|---|---|---|---|
| 50734 | 50761–56189 (`0xc649`–`0xdb7d`) | `pass=49 fail=5 skip=0` | 0 |

**The controls** ran after the sweep, on the same host, alternating two binaries:

- **Swept binary:** `retrace-t3`, as above.
- **Base binary:** `retrace-base`, sha256 `a5fd53a3231a8d6d77498fce7d78b5c190ed004f6d62e9f1b47db0209275babd`
  after signing.
  - It is M45's base `a78f28f`, built from a `git archive a78f28f` extracted into the scratchpad,
    with its own target dir (`t3-base-build.sh`).
  - `git diff --stat 60f0452 a78f28f -- crates` is empty, so its crates are M44's close.
  - A fresh build was used instead of an older M44 worktree binary, because an archive build's
    provenance holds by construction.
- **Readers:** two throwaway trace readers, `tracedump.rs` and `ipawho.rs`, read kept traces through
  `retrace_trace::Reader::open_checked`. Each is built as its own crate in the scratchpad, depending
  on `crates/retrace-trace` by path. The sources are kept here.
- **No `cargo test` ran** while a control was recording.

## The non-clean rows

| row | label | `rc`/`rp` | wall |
|---|---|---|---|
| `/bin/csh` | `FAIL` (record error) | 4 / 3 | `mach_msg2` msgh_id 3403 (`mach_ports_register` ← `fork`), class C (unchanged); landmark 336 |
| `/bin/tcsh` | `FAIL` (record error) | 4 / 3 | 3403, class C (unchanged); landmark 336 |
| `/bin/ps` | `FAIL` (replay diverged at landmark 16044) | 0 / 3 | **class E, host state**: `memory divergence at ipa 0x701414078: replay=0xf5 recorded=0x00` at the final compare (below) |
| `/usr/bin/automationmodetool` | `FAIL` (recorder panicked) | 101 / n/a | `M45: unmeasured kevent_qos shape: x3 (eventlist) is 0x27fedb8, measured 0x0` — the second `kevent_qos`, Halt 3 (the walk, below) |
| `/usr/bin/yes` | `FAIL` (timed out after 30s recording) | 137 / n/a | never terminates; the watchdog, by design (unchanged) |

There is one `identical fault` row: `/usr/bin/dddiagnose` `PASS` 139/139, a guest data abort at pc
`0x180302eb0` (below).

## Row-by-row diff against M44

The comparison is `t3-rowdiff.sh`, output `rowdiff.txt`.

- It keys on the binary's path and compares `result`, `rc`, `rp` and `landmark`.
- It then compares `rec_reason`, with every parenthesised number (the Rust thread id) normalised out.
- **49 rows are identical and 5 differ.**

| row | M44 | M45 | moved by |
|---|---|---|---|
| `/usr/bin/automationmodetool` | `FAIL` 101/n/a, panic: syscall 374 has no row | `FAIL` 101/n/a, panic: `M45: unmeasured kevent_qos shape` | **M45**: the init is emulated, and the run reaches the second shape |
| `/bin/ps` | `PASS` 0/0 | `FAIL` 0/3, landmark 16044 | **host state**: a reclaimed `MADV_FREE_REUSABLE` page (below) |
| `/usr/bin/dddiagnose` | `FAIL` 4/3, `RECORD ERROR` msgh_id 205, landmark 455 | `PASS` 139/139 (identical fault) | **host state**: the outcome varies run to run on both binaries (below) |
| `/bin/csh` | `FAIL` 4/3, landmark 338 | `FAIL` 4/3, landmark 336 | the guest's own `gettimeofday` count (below) |
| `/bin/tcsh` | `FAIL` 4/3, landmark 333 | `FAIL` 4/3, landmark 336 | same |

`yes` is identical: 137/n/a, the watchdog.

### `/bin/ps`: class E, a host-reclaimed page

**What diverged.**
- Replay consumed all 16043 syscall landmarks (`#16043` is `Exit` code 0).
- It stopped at `#16044`, the final full-memory `Snapshot`, where `diff_memory` found
  `ipa 0x701414078: replay=0xf5 recorded=0x00`.
- So the recording itself says the guest ran to a clean exit. Only the final memory image
  disagrees.

**Which landmark wrote the byte** (`ipawho.rs` on the sweep's own `ps.bin`, output `ps-pagemap.txt`):
- `#253` is `sysctl`(202), with name length 4 and `oldp = 0x701400000`. That is the process-table
  query M26 found in `ps`; the MIB itself was not decoded here.
- Its recorded write is `0x701400000` + `0x4ce30`, and it carries `0xf5` at `0x701414078`.
- **No later event writes that byte.**
- After `#253`, only two events name the range:
  - `#16040` `madvise(0x701400000, 0x50000, 7)`;
  - `#16041` `madvise(0x701450000, 0x24000, 7)`.
- `7` is `MADV_FREE_REUSABLE`, which the status log already calls routine libmalloc housekeeping.
  Here it comes as the buffer is freed, three landmarks before `exit`.
- `madvise` is forwarded. Its `arg_kinds` row is `75 => row!(P, [Ptr, Scalar, Scalar])`, which
  rebases the range onto retrace's own backing. So on record the host kernel marks retrace's own
  pages reusable. Replay executes no syscall, so on replay nothing does.

**The page map is the fingerprint.** For each 16 KiB page of `[0x701400000, 0x701430000)`, the 12
pages the `sysctl` output fills, `ps-pagemap.txt` counts non-zero bytes in the final snapshot
against `#253`'s write:
- 11 pages match `#253` count for count, which fits no later rewrite. The replay's first mismatch at
  `0x701414078` also shows the five pages before it byte-identical.
- one differs: **page `0x701414000`, 0 non-zero bytes in the final snapshot against 1876 in the
  recorded write.**

One whole, aligned page reads back as zeros, inside a range madvised reusable, and nothing
recorded wrote it. That is the host reclaiming a reusable anonymous page, which then reads as zero
fill. The replay's copy still holds the kernel's data. The compare reports the page's first byte
that differs (offset `0x78`), and the page's first `0x78` bytes are zero on both sides.

**Named before, measured now.** M37's status-log section carries this hazard as owed:

> `MADV_FREE_REUSABLE` on the guest backing … the host may lazily reclaim those guest pages; a
> later read of zeros there is nondeterminism no diff window captures. Not measured.

M38 carried it forward as "unchanged, unmeasured". This row is its first measured instance.

**Not M45:**
- The trace contains **zero** `kevent_qos` (374) events (`ps-pagemap.txt`).
- Every line M45 changes in `crates/` has no effect unless `num == 374`:
  - the record arm;
  - the replay mirror;
  - the forward arm's assert, which is evaluated on every forwarded call but fails only for 374;
  - the 374 `arg_kinds` row;
  - the validator.
- The sweep's own recording, replayed on the **base** binary, stops at the identical landmark, pc,
  ipa and bytes (`ps-replay-control.txt`):
  ```
  replay of sweep ps.bin on base: rp=3 DIVERGENCE at landmark 16044 pc=0x1804b5584: memory divergence at ipa 0x701414078: replay=0xf5 recorded=0x00
  replay of sweep ps.bin on t3: rp=3 DIVERGENCE at landmark 16044 pc=0x1804b5584: memory divergence at ipa 0x701414078: replay=0xf5 recorded=0x00
  ```

**It is intermittent** (`t3-ps-control.sh`, output `ps-control.txt`).
- 20 fresh record-then-replay runs, alternating the two binaries (10 each), were all clean: rc 0,
  rp 0, stdout identical.
- The host logged 590–614 processes and memory-pressure level 1 (normal) throughout.
- For the row to diverge, the host must reclaim a page within the few landmarks between the
  `madvise` and the final snapshot.

**What was not done.** No fresh base-binary recording reproduced the reclaim. Forcing one means
applying real memory pressure to a shared host (`memory_pressure -l warn`), which this task did not
do. The base binary reproduces the divergence as a replay of the same recording, and the mechanism
is the forwarded `madvise`, which M45 does not touch.

### `/usr/bin/dddiagnose`: two outcomes, on both binaries

The sweep's run (`sweep/dddiagnose.rec.err`) crashed:
`guest crashed: pc=0x180302eb0 far=0x4000050050 esr=0x92000045`. That is EC `0x24`, a write, with a
level-1 translation fault. Replay faulted identically, so the sweep counts it as `PASS` (identical
fault). M44's sweep reached `mach_msg2` msgh_id 205 (`host_get_io_main`) instead.

Traced controls (`t3-dddiagnose-control.sh`, output `dddiagnose-control.txt`), alternating binaries:

| round | binary | `rc`/`rp` | outcome |
|---|---|---|---|
| a1 | t3 | 139 / 139 | fault at pc `0x180302eb0`, far `0x2000050040` |
| a1 | base | 139 / 139 | fault at pc `0x180302eb0`, far `0x4000050050` (the sweep's far) |
| a2 | t3 | 4 / 3 | `RECORD ERROR` msgh_id 205 (451 traps) |
| a2 | base | 4 / 3 | `RECORD ERROR` msgh_id 205 (453 traps) |
| b1 | t3 | 4 / 3 | msgh_id 205 (458 traps) |
| b1 | base | 4 / 3 | msgh_id 205 (454 traps) |
| b2 | t3 | 4 / 3 | msgh_id 205 (454 traps) |
| b2 | base | 4 / 3 | msgh_id 205 (448 traps) |

**Both outcomes occur on both binaries**, one fault and three 205 stops on each.
`dddiagnose-traps.txt` gives each run's trap count, its `kevent_qos` (374) count (0 on all eight),
the trap number of the refused RCV-only `mach_msg2`, and the outcome line.

- Every run refuses that receive exactly once (`receive-refusals=1`).
- The two faulting runs refuse it at trap #384 (t3) and #383 (base). They then fault after trap 393
  and 392, about 9 landmarks later.

**Where the runs part** (`t3-dddiagnose-fork.sh`, output `dddiagnose-fork.txt`):
- **The first split.** By trap number, the faulting run a1-base and the 205 run a2-base first
  differ at `[trap]` line 209, one extra `gettimeofday`.
- **Before line 209, which values differ.** `dddiagnose-prefork.txt` diffs lines 1–208 in full,
  arguments included:
  - the recorder's own pid, in `proc_info(0xf, pid)` (#24) and in `csops`(169) (#200);
  - host port names, in three `mach_msg2` headers (#185, #192, #193) and in two `-18` port traps
    (#186, #194);
  - `x6` of `gettimeofday` #208, a register that call does not read.
- **Where the placement differs.** In the two runs' VM / `sysctl` / `proc_info` / `mach_msg2`
  lists (`dddiagnose-a1-base.vm`, `dddiagnose-a2-base.vm`), line 86 is the same
  `mach_vm_deallocate`. It frees `0xa0027c000` in one run and `0xa00408000` in the other.
- **Then the fault.** The faulting run faults about 180 landmarks after line 209, writing a
  run-varying address.

**This is not new, and it contradicts a retirement.** The status log has met this face three times:
- **M36** symbolicated pc `0x180302eb0` to libsystem_malloc `mfm_alloc+0x230`: a data abort,
  `esr=0x92000045`, DFSC `0x05` (`docs/status-log.md:8169`, `:8208`). It classed the face
  "downstream of §4b" (`:8342–8347`).
  - Its Finding 3 (`:8429–8452`) located a crash-vs-`brk` fork in a `gettimeofday` polling loop.
  - It measured `far` varying between runs while pc and esr did not.
  - Both fit what M45 sees: the split starts at a `gettimeofday` count, and `far` varies.
- **M37 retired it** (`:9638–9644`, `:9726–9729`): "neither face can be reached with a
  correctly-forwarded pid (three sweeps, 0 `identical fault`, 0 `brk`)". M45 reaches it with
  correctly forwarded pids, on M44's crates as well as M45's.
- **M38 R3** chose `MACH_RCV_INVALID_NAME` over `MACH_RCV_TIMED_OUT` on one run per code
  (`:10112`, `:10115–10119`). `TIMED_OUT`'s losing cell on `dddiagnose` was this same face: pc
  `0x180302eb0`, esr `0x92000045`, "~10 landmarks after the receive". M45 now sees it under
  `INVALID_NAME`, about 9 landmarks after the refused receive. In the one row that discriminated,
  R3's single-run comparison may have compared two samples of this coin flip, so R3 is owed a
  re-measure.

**Root cause not measured.** Nothing was traced past the placement split. The `apple_walls_e2e`
gate stays parked at msgh_id 205. Its reason now also records the fault outcome, and it un-ignores
only when 205 is serviced and the `mfm_alloc` face is explained or measured absent. This README
does not edit the status log; Task 4 carries the two forward pointers (M37's retirement, M38 R3).

### `csh`/`tcsh`: gettimeofday

M44 measured this spread (`docs/sweep-evidence/2026-09-27-m44/csh-samples.txt`):
- `csh`'s trap count before the 3403 wall ranged 333–346 on both of its binaries;
- on all eight runs, count minus `gettimeofday` (116) traps was exactly 316.

M45's 336 and 336 sit inside that range. This task repeated the sample for both shells on M45's
base and swept binaries (`t3-csh-samples.sh`, output `csh-samples.txt`):
- eight runs, two per shell per binary, alternating;
- trap counts ranged 336–344;
- on **all eight**, count minus `gettimeofday` was exactly **317**, on both binaries and for both
  shells.

**The landmark is the guest's `gettimeofday` count.** The constant is one higher than M44's 316.
The base binary, whose crates are M44's close, shows the same 317 today, so the shift is not in
M45's diff. Whether the host or an M44 change after its sweep added the trap was not traced.

## The walk (Steps 1–3)

**`automationmodetool`: outcome B, §7 Halt 3.**
- The emulated init records at landmark 362: rc 0, no writes, thread 0.
- The next call, landmark 363, is a second `kevent_qos` that M45 refuses. Record rc 101, and no
  replay ran.
- Its arguments are: `x3` event list `0x27fedb8`, `x4` 16, `x7` `0x23`
  (`KEVENT_FLAG_WORKQ|KEVENT_FLAG_ERROR_EVENTS|KEVENT_FLAG_IMMEDIATE`).
- Its change entry was read by `retrace debug` at the stub's `svc` (`0x1804afa44`):
  - filter `0xfff2` (-14);
  - flags `0x0185` (`EV_ADD|EV_ENABLE|EV_DISPATCH|EV_UDATA_SPECIFIC`);
  - qos `0x02000000`;
  - fflags `0xf0000037`;
  - every other field zero apart from `udata`.
- By those values this is libdispatch's own memory-pressure source registration. That is inferred,
  not symbolicated.
- It is re-parked there and not modelled. The gate's reason carries the measurement.

**GCD candidates.**
- `timer` stops at landmark 247 and `after` at landmark 246, both at the same second shape. Only
  `udata` and the stack addresses differ, and the return address is identical in all three
  programs. Neither is gated.
- `signal` was dropped: natively it hangs (t0 Ruling T0-a).

Walk files:
- `automationmodetool.{rec.err,rec.out,entry.txt}`;
- `gcd-{timer,after}.{rec.err,rec.out,entry.txt}`;
- `landmarks.txt`, which is `tracedump.rs` on the three traces;
- the scripts `t3-step1.sh`, `t3-step3.sh`, `t3-wall-entry.sh`, `t3-candidates-measure.sh` and
  `t3-evidence-copy.sh`.

## Ruling

Every row that moved is explained by measurement:
- **`automationmodetool`** moved by M45, as designed: the init is emulated, and the next shape is
  refused by value (Halt 3).
- **`ps`** moved by host state. A forwarded `MADV_FREE_REUSABLE` let the host reclaim one guest page
  before the final snapshot. The base binary replays the recording to the identical divergence. The
  trace has no 374, so it reaches none of M45's code.
- **`dddiagnose`** moved by host state. Both binaries produce both outcomes, run to run.
  - The fault is M36's `mfm_alloc` face, which M37 retired.
  - It is also M38 R3's losing face, now seen under the winning code.
  - Both are forward pointers owed to Task 4's status-log section.
- **`csh`/`tcsh`** moved by the guest's `gettimeofday` count.

**No row moved because of M45's diff other than `automationmodetool`.**

- `ps` is a class-E row, the kind M44's README could say it had none of.
- Its class is a pre-existing, previously named, now measured hazard. It is not a regression of this
  milestone.
- It is owed to its own successor: a forwarded `MADV_FREE_REUSABLE` makes the recorded final image
  depend on host memory pressure.

## Files

- `sweep.log`: the controller's full detached log, which holds the wrapper's two header lines, 54
  `ROW` lines, `TALLY pass=49 fail=5 skip=0` and `SWEEP_EXIT=0`.
- `sweep/<basename>.rec.err` for every non-clean row, and `.rp.err`/`.rp.out` where a replay ran.
  The rows are `automationmodetool`, `csh`, `dddiagnose`, `ps`, `tcsh` and `yes`.
- `rowdiff.txt`: the row-by-row diff against M44.
- `ps-pagemap.txt`: the `ps` trace's writers of `0x701414078`, the calls naming its 4 MiB block, the
  page map, and the 374 count.
- `ps-replay-control.txt`: the sweep's `ps` recording replayed on both binaries.
- `ps-control.txt`: the fresh alternating `ps` runs.
- `dddiagnose-control.txt`: the traced alternating `dddiagnose` runs.
- `dddiagnose-fork.txt`: where a crashing run and a 205 run part.
- `dddiagnose-traps.txt`: per traced run, the trap count, the 374 count, the refused receive's trap
  number, and the outcome.
- `dddiagnose-prefork.txt`: every differing `[trap]` line before that split, arguments included.
- `dddiagnose-a1-base.vm` and `dddiagnose-a2-base.vm`: the two runs' VM / `sysctl` / `proc_info` /
  `mach_msg2` trap lines.
- `csh-samples.txt`: the `gettimeofday` sample for `csh`/`tcsh` on both binaries.
- The walk's files, listed above.
- The scripts, with the session's scratchpad paths left as they ran:
  - `t3-sweep.sh`;
  - `t3-base-build.sh`;
  - `t3-ps-control.sh`;
  - `t3-ps-replay-control.sh`;
  - `t3-ps-ipawho.sh`;
  - `t3-dddiagnose-control.sh`;
  - `t3-dddiagnose-fork.sh`;
  - `t3-csh-samples.sh`;
  - `t3-rowdiff.sh`;
  - `t3-step4-evidence.sh`;
  - fix round 1's `t3f1-dddiagnose-evidence.sh`;
  - the walk's five.
- `tracedump.rs` and `ipawho.rs`: the throwaway readers' sources.
- **No `.bin` trace files are committed.**
  - The sweep kept six. `ps.bin` was read by the investigation above, and the other five were removed
    unread.
  - Every control's traces lived in the scratchpad and were removed there. The walk's three
    (`/private/tmp/claude-501/m45-{amt2,timer-2,after-2}.bin`) were removed after `landmarks.txt`
    and the `entry.txt` reads.
