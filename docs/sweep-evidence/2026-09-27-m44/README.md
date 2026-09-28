# Sweep evidence — M44 Task 13, run 2026-09-27

One measurement: the full 54-entry corpus swept once on the close's binary and diffed row by row
against M39's run (`docs/sweep-evidence/2026-09-17-m39/`), per spec §3a A6 ("reconcile the tally row
by row against M39's `pass=44 fail=10 skip=0`") and §6 item 7 ("every moved row named").

**Result: `TALLY pass=49 fail=5 skip=0`, against M39's `pass=44 fail=10 skip=0`.** Six rows changed
their label:
- `ed` and `ls` move FAIL → PASS by M44's rows.
- `dddiagnose` moves from a recorder panic to a class-C record error by M44's `statfs64` row.
- The xcrun trio (`desdp`, `dyld_info`, `flex`) move FAIL → PASS in the sweep's sense. That move is
  **the host's xcrun cache, not M44**. A warm-cache control on the M44 base binary reproduces it
  (below).

`csh` and `tcsh` moved only their landmark, and that move is measured below as the guest's own
nondeterminism. No row moved for a reason this README does not measure.

## Method

`tools/apple-sweep.sh`, run detached from the worktree root, wrapped in `t13-sweep.sh` (this
directory). The wrapper prints its `pidstart` and the binary's hash, commit and date first (the
M38/M39 shape). The close's `target/aarch64-apple-darwin/debug/retrace` was copied to the session
scratchpad and ad-hoc signed there with `retrace.entitlements`, so no concurrent `cargo test` could
swap the binary under the sweep. Its sha256 after signing is
`4d3f1b9b565f363ba291317f4f8e371d6628ebed68718d7baad78c79c0020005`.
`RETRACE_SWEEP_KEEP=…/sweep` kept every non-clean row's `rec.err`/`rp.err`/`rp.out`/`bin`. The `.bin`
traces were removed unread and none is committed, the M37–M39 rule. Log: `sweep.log`.

| `pidstart` | recpid range | `TALLY` | `SWEEP_EXIT` |
|---|---|---|---|
| 35161 | 35186–36890 (`0x8972`–`0x901a`) | `pass=49 fail=5 skip=0` | 0 |

The recpid selects nothing, because §4b has been retired since M37.

Commit swept: `5ee07b8`, Task 12's first commit. Two commits followed it, both Task 12's fix round
1 (`50c81df`, `cbe59d7`). `git diff --stat 5ee07b8 cbe59d7 -- crates` shows they touch only
`crates/retrace/src/debug.rs`, `crates/retrace/src/gdbserver.rs` and three test files. That is the
debugger and its RSP server. The sweep runs only `record-dyn` and `replay`, and `replay` is
`retrace_core::replay` (`crates/retrace/src/main.rs:88-90`), which reaches neither file. The swept
binary's record and replay paths are therefore the close's. Task 13 changes only documentation.
Task 14's fix wave, if it touches `crates/`, is listed in the status log's M44 section against this
hash.

## The five non-clean rows

| row | label | `rc`/`rp` | wall |
|---|---|---|---|
| `/bin/csh` | `FAIL` (record error) | 4 / 3 | `mach_msg2` msgh_id 3403 (`mach_ports_register` ← `fork`), class C (unchanged) |
| `/bin/tcsh` | `FAIL` (record error) | 4 / 3 | 3403, class C (unchanged) |
| `/usr/bin/dddiagnose` | `FAIL` (record error) | 4 / 3 | `mach_msg2` msgh_id 205 (`host_get_io_main`), class C, **new face**; replay `DIVERGENCE at landmark 455` |
| `/usr/bin/automationmodetool` | `FAIL` (recorder panicked) | 101 / n/a | `kevent_qos` (374), no row (unchanged; routed by M44 R4) |
| `/usr/bin/yes` | `FAIL` (timed out after 30s recording) | 137 / n/a | never terminates; the watchdog, by design (unchanged) |

There is no `identical fault` row, no `replay diverged` row, and **no class-E row**: no recorder
finished cleanly and then had its replay disagree.

## Row-by-row diff against M39

The comparison keys on the binary's path. It compares `result`, `rc`, `rp` and `landmark`, then
`rec_reason` and `rp_line` with the Rust thread id, the landmark number and the mach port name
normalised out. **46 rows are identical and 8 differ.**

| row | M39 | M44 | moved by |
|---|---|---|---|
| `/bin/ed` | `FAIL` 101/n/a, panic: syscall 464 has no row | `PASS` 0/0 | **A1**: `openat_nocancel` (464) joins `openat`'s row |
| `/bin/ls` | `FAIL` 101/n/a, panic: syscall 461 has no row | `PASS` 0/0 | **A2**: `getattrlistbulk` (461) row |
| `/usr/bin/desdp` | `FAIL` 101/n/a, panic: syscall 464 | `PASS` 71/71 | **host state**: xcrun's warm cache skips 464, straight to the M38 `posix_spawn` refusal; see below |
| `/usr/bin/dyld_info` | `FAIL` 101/n/a, panic: syscall 464 | `PASS` 71/71 | same |
| `/usr/bin/flex` | `FAIL` 101/n/a, panic: syscall 464 | `PASS` 71/71 | same |
| `/usr/bin/dddiagnose` | `FAIL` 101/n/a, panic: syscall 345 has no row | `FAIL` 4/3, `RECORD ERROR` msgh_id 205 | **A2**: `statfs64` (345) row; the next wall is class C |
| `/bin/csh` | `FAIL` 4/3, landmark 346 | `FAIL` 4/3, landmark 338 | the guest's own spread, measured below |
| `/bin/tcsh` | `FAIL` 4/3, landmark 341 | `FAIL` 4/3, landmark 333 | same |

The `automationmodetool` row's `rec_reason` also differs before normalisation, in two ways: the
thread id (a fresh number per process), and `crates/retrace-arch/src/lib.rs:946:38` → `:982:38`.
The second is the `forwarded_shape` fail-loud moving down 36 lines, because A1 and A2 added table
rows and constants above it. The panic site, the syscall (374), the label, `rc` and `rp` are all
identical.

### The trio's PASS is the sweep's sense, not the native outcome

`desdp`, `dyld_info` and `flex` all exit 71 (`EX_OSERR`) on both record and replay, with identical
stdout, so the sweep counts them `PASS`. Natively, `desdp` with no arguments exits 2 and prints
usage (t0 M3). They reach xcrun's `posix_spawn`, which M38 refuses:

```
[retrace] refusing posix_spawn (syscall 244): exec-in-place is unmodelled; returning errno 14 without forwarding
```

The full sweep keeps no `rec.err` for a `PASS` row, so the trio was re-run alone on the same signed
binary with `RETRACE_SWEEP_KEEP_ALL=1` (`t13-trio.sh`). `/var/tmp/xcrun_db` was present, which is a
warm cache, the full sweep's condition. The run gave `TALLY pass=3 fail=0 skip=0`, and all three
`trio/*.rec.err` carry the line above. Task 4 measured the same line from a cold cache
(`docs/sweep-evidence/2026-09-27-m44-t0/t4/`). Their `apple_walls_e2e` gates stay parked at that
refusal (Ruling T0-b), class C, because exec-in-place is unmodelled.

**Why they moved since M39, measured, and a correction.** This README first credited the trio's
move to A1's 464 row, and that was wrong. From a valid xcrun cache the trio skips 464 and `rename`
(128) and goes straight to `posix_spawn` (M44 t0, Ruling T0-e). `/var/tmp/xcrun_db` was present for
the whole sweep, with mtime 13:39, written by Task 4's cold-cache recordings' forwarded `rename`.

The control (`t13-trio-base.sh`, output in `trio-warm-control.txt`) ran after the sweep, from that
same cache, under `RETRACE_TRACE=1`. The M44 base binary `ebd0266` has no 464 or 128 row, yet it
records all three to 71/71, exactly as the swept binary does, with **zero** 464 and zero 128 traps
on either binary and one `posix_spawn` (244) each.

So M39's 464 panics were a cold cache, and this sweep's PASS is a warm one. The 464 row's evidence
is `ed`, whose 464 opens its own buffer file, together with Task 4's cold-cache runs. The trio
supplies none of it.

### The csh/tcsh landmark move is gettimeofday

`csh` went 346 → 338 and `tcsh` 341 → 333. Both stop at the same wall (`mach_msg2` 3403 at pc
`0x1804adc34`), with the same `rc`/`rp` of 4/3. M39's README recorded this spread without a
mechanism. It is measured here (`t13-csh-samples.sh`, output in `csh-samples.txt`): `csh` was
recorded four times with the M44 base binary (`ebd0266`) and four times with the swept binary,
alternating, on the same host within the same few minutes. The trap count before the wall ranged
333–346 on **both** binaries. On all eight runs, the count minus the `gettimeofday` (116) traps was
exactly 316. The only thing that moves is how many times the shell calls `gettimeofday`, so the
landmark is the guest's own timing-dependent count, not retrace's. The class-C verdict does not
depend on it.

`dddiagnose`'s landmark, 455, is one away from Task 4's 454 on `5ac50f7`. Its mechanism was not
measured. The gate asserts on the label and the wall, not on the landmark.

## Ruling

**Acceptance met.** Every row that moved is explained by measurement:
- `ed` and `ls` are un-parked by M44's rows: `ed` by 464 (A1) and `unlink` (A2), `ls` by 461 (A2); gated by A4.
- `dddiagnose` moved to its re-parked wall (A2, A4).
- The trio moved by host state, which the warm-cache control reproduces on the base binary. Their
  gates stay parked at the `posix_spawn` refusal (T0-b).

Nothing moved backwards: no `PASS` became a `FAIL`, and no `FAIL` changed class except `dddiagnose`,
which went from a missing row to class C. The only other moves are two landmarks, and those are now
attributed to a counted syscall.

## Files

- `sweep.log` — the full detached log: the wrapper's two header lines, 54 `ROW` lines,
  `TALLY pass=49 fail=5 skip=0`, `SWEEP_EXIT=0`.
- `sweep/<basename>.rec.err` (and `.rp.err`/`.rp.out` where a replay ran) for every non-clean row:
  `automationmodetool`, `csh`, `dddiagnose`, `tcsh`, `yes`.
- `trio/` — the trio re-run's log and its three `rec.err`.
- `csh-samples.txt` — the eight-run gettimeofday measurement.
- `trio-warm-control.txt` — the warm-cache control, base binary against swept binary.
- `t13-sweep.sh`, `t13-trio.sh`, `t13-csh-samples.sh`, `t13-trio-base.sh` — the scripts, with the session's scratchpad
  paths left as they ran.
- **No `.bin` trace files are committed.** The five the sweep kept were removed unread after the
  tally, and the trio re-run's three were removed the same way.
