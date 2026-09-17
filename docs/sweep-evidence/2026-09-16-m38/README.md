# Sweep evidence — M38 Task 5, 2026-09-16

The RCV-only message-queue `mach_msg2` (`options 0x4_0400_0102` — `RCV_MSG | RCV_TIMEOUT`, no
`SEND_MSG`) is refused deterministically by `Route::RefuseMqRecv`, and the code it returns,
`MACH_RCV_REFUSAL`, is **chosen here by measurement** against the six binaries M37 parked at exactly
this call (`/bin/launchctl`, `/usr/bin/automationmodetool`, `/usr/bin/desdp`, `/usr/bin/dyld_info`,
`/usr/bin/flex`, `/usr/bin/dddiagnose`). Each candidate code was built into the recorder, ad-hoc
signed, and run record-then-replay against all six; the winner is the code the most binaries accept.

## Method

`docs/sweep-evidence/2026-09-16-m38/measure.sh`, run detached from the worktree root. For each of the
three candidate codes it `sed`-edits `MACH_RCV_REFUSAL`, `cargo build -p retrace`, copies the binary
to `/tmp/m38-t5/retrace-<CODE>`, ad-hoc signs it with `retrace.entitlements`, then for each binary
records (`record-dyn <bin> -o …`, 180 s `perl alarm`, stdin `/dev/null`) and replays. The recorder's
pid is printed into `<bin>.<CODE>.rec.err`'s first line by the `apple-sweep.sh` idiom
(`sh -c 'echo "recpid=$$" >&2; exec …'`), so each cell's `recpid` is the recorder's own. The
committed constant is the winner; `measure.sh` restores the default with `git checkout` (not a
stash) before this file was written, and Steps 1–4 were committed first so that checkout is clean.

**Codes tried** (`osfmk/mach/message.h`, all in the `MACH_RCV` family `0x1000_40xx`):
`MACH_RCV_TIMED_OUT 0x1000_4003` (spec R3 default — the options word carries `RCV_TIMEOUT`, and a
queue with no sender is a queue whose receive times out), `MACH_RCV_INVALID_NAME 0x1000_4002`,
`MACH_RCV_PORT_DIED 0x1000_4006`.

**recorder-pid regime (spec R6): one.** §4b is retired since M37 — a `Scalar` position is never
probed, so a recorder pid landing in a guest backing no longer forwards as a host pointer, and the
N/I/S regimes no longer distinguish anything on these rows. All 18 cells ran in one regime: recpids
54954–55330 (`0xd6aa`–`0xd822`), which sit **inside** M36's old §4b window `[0x4000, 0x10000)` — the
trampoline page, the regime M37 labelled **I**, not N. (The plan pre-labelled this run "N"; the
measured pids say I, and the label here and in the gate reasons follows the measurement.) That is
irrelevant to the ruling, and it is the stronger fact: measured on the winner traces with the M37
README's `selfpid` reader, **0 self-pid `ESRCH`** in every kept trace — every self-pid
`csops`/`proc_info` call (12 or 13 per trace) succeeded — with every pid inside the window that
pre-M37 answered all but one of those calls `ESRCH`. So the pid regime is not a variable here and
the gate reasons cite "run I" and say why.

## The table (18 cells)

Every cell recorded **exactly one** RCV-only message-queue refusal line
(`receive-refusals=1`, the positive control that the new arm — and not another path — is what the
guest reached) and had `stdout-equal=y` (replay's stdout matched the recording's). `rc` = record
exit, `rp` = replay exit. "wall N" = the recorder panicked at the M33 fail-loud (`forwarded_shape`,
`crates/retrace-arch/src/lib.rs:944`) on an **unclassified** guest syscall N — the refusal was
**accepted** and the guest ran on to a call the box has never had an `arg_kinds` row for; `rp=3` is
replay reading past the panic-truncated trace (`expected recorded syscall, got None`), not a
divergence of its own. "brk" = the guest crashed downstream of the refusal (the code was not
tolerated).

| binary | `MACH_RCV_TIMED_OUT` (0x…4003) | `MACH_RCV_INVALID_NAME` (0x…4002) | `MACH_RCV_PORT_DIED` (0x…4006) |
|---|---|---|---|
| `launchctl` | **proceeds** — its own `exit(1)` usage, rc/rp 1/1 | **proceeds** — rc/rp 1/1 | **proceeds** — rc/rp 1/1 |
| `automationmodetool` | **wall** `kevent_qos` (374), rc/rp 101/3 | **wall** 374, rc/rp 101/3 | **wall** 374, rc/rp 101/3 |
| `desdp` | **wall** `openat_nocancel` (464), rc/rp 101/3 | **wall** 464, rc/rp 101/3 | **wall** 464, rc/rp 101/3 |
| `dyld_info` | **wall** `openat_nocancel` (464), rc/rp 101/3 | **wall** 464, rc/rp 101/3 | **wall** 464, rc/rp 101/3 |
| `flex` | **wall** `openat_nocancel` (464), rc/rp 101/3 | **wall** 464, rc/rp 101/3 | **wall** 464, rc/rp 101/3 |
| `dddiagnose` | **brk** `guest crashed pc=0x180302eb0 far=0x2000050050 esr=0x92000045`, rc/rp 139/139 | **wall** `statfs64` (345), rc/rp 101/3 | **brk** `guest crashed pc=0x193bbbca0 far=0xfffffffffffffff0 esr=0x92000004`, rc/rp 139/139 |

**Accepted (proceeds or a new wall — both mean the refusal was accepted) per code:**

| code | accepted / 6 | rejected |
|---|---|---|
| `MACH_RCV_TIMED_OUT` | 5 | `dddiagnose` brk |
| `MACH_RCV_INVALID_NAME` | **6** | — |
| `MACH_RCV_PORT_DIED` | 5 | `dddiagnose` brk |

## Ruling (spec R3)

**`MACH_RCV_REFUSAL = MACH_RCV_INVALID_NAME` (0x1000_4002).** It is the only candidate all six
binaries accept (6/6), against 5/6 for each of the other two. **Not a tie**, so the R3 default
(`MACH_RCV_TIMED_OUT`) and the tie-break do not apply — the semantically faithful "a queue no one
can send to times out" lost on exactly one binary, `/usr/bin/dddiagnose`, which under both
`TIMED_OUT` and `PORT_DIED` crashes in the guest ~10 landmarks after the receive (a data abort, the
guest dereferencing something derived from the return code) and under `INVALID_NAME` survives ~50
landmarks further, to its own next wall. The winner is asserted in code by
`machmsg::tests::the_receive_refusal_is_a_receive_code` (`MACH_RCV_REFUSAL == MACH_RCV_INVALID_NAME`,
`!= MACH_RCV_TIMED_OUT`), so a change to the constant must change the test — the choice is a
measurement, not a preference.

## What each binary now stops at (winner code)

The refusal is landmark `#R` in the winner trace; the row then runs to the M33 wall (or, for
`launchctl`, to its own clean exit). Divergence landmarks are the replay's `DIVERGENCE at landmark N`
line; the wall pc is that line's pc, which is the pc of the *unrecorded* trap the recorder panicked
on.

- **`/bin/launchctl`** — receive `#339`; runs to `exit(1)`, its own no-argument usage (4484 bytes,
  byte-identical to the host's native `/bin/launchctl` output measured 2026-09-16). **Un-parked.**
- **`/usr/bin/automationmodetool`** — receive `#343`; wall `kevent_qos` (374) at pc `0x1804afa48`,
  divergence landmark 363. Re-parked, class B.
- **`/usr/bin/desdp`** — receive `#363`; wall `openat_nocancel` (464) at pc `0x1804b3954`,
  divergence landmark 392. Re-parked, class B.
- **`/usr/bin/dyld_info`** — receive `#365`; wall `openat_nocancel` (464) at pc `0x1804b3954`,
  divergence landmark 393. Re-parked, class B.
- **`/usr/bin/flex`** — receive `#369`; wall `openat_nocancel` (464) at pc `0x1804b3954`,
  divergence landmark 398. Re-parked, class B.
- **`/usr/bin/dddiagnose`** — receive `#381`; wall `statfs64` (345) at pc `0x1804bd0cc`, divergence
  landmark 431. Re-parked, class B.

**A note on the three xcrun stubs.** `/usr/bin/desdp`, `/usr/bin/dyld_info` and `/usr/bin/flex` are
the Xcode `xcrun` trampoline (`dyld_info` and `flex` are hardlinks to a single inode, `desdp` to
another with 16 links); it opens a random-named `/var/tmp/xcrun_db-XXXXXX` (read out of guest memory
at the wall: the `openat_nocancel` path was `/var/tmp/xcrun_db-thliC5Xn` on one run). So the
*intervening* syscall path shifts run-to-run — a re-record of `desdp`/`flex` from the shell reordered
and lengthened the pre-wall syscalls (evidence `/tmp/m38-t5/<bin>.r1.*`, not committed) — but the
**wall syscall number is stable**: 464 for all three across both the measurement and the gate run,
374 for `automationmodetool`, 345 for `dddiagnose`. That variation is the guest's own tempfile
nondeterminism, not retrace's; the class-B verdict does not depend on the exact path.

**None of the five is class E.** Class E is a *clean* recording (a terminal event present) whose
replay then diverges. Here every wall row's recorder **panics** (rc 101) at the unclassified
syscall and never writes a terminal event, so the trace is truncated by construction and `rp=3`
is the expected read past its end — the M37 pattern, not a divergence of a complete recording. No
halt.

## Files

- `measure.sh` — the measurement script. `measure.tsv` — its 18 `ROW` lines. `measure.log` — its
  full detached log (build lines + rows + the closing `git checkout`).
- `<bin>.<CODE>.rec.err` / `.rp.err` for all 18 cells — kept (all three codes), since the ruling
  rests on the losing codes too (`dddiagnose`'s two crashes are the reason `INVALID_NAME` won). The
  `.rec.out`/`.rp.out` are zero-length for every binary except `launchctl` (whose stdout is its
  usage); `launchctl.MACH_RCV_INVALID_NAME.rec.out` is kept as the winner's stdout, the others were
  removed.
- `gates.log` — the `apple_walls_e2e --ignored` run (the file's positive control: each parked gate
  prints its wall by name). `gate-<bin>.log` — the per-binary gate runs.
- `rerun.sh` — the shell re-record helper used to confirm the xcrun-path nondeterminism above.
- **No `.bin` trace files are committed** (large); they live under `/tmp/m38-t5/` for this session.
