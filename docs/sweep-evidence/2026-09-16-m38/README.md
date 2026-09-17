# Sweep evidence — M38 Tasks 5 and 6, 2026-09-16

Two measurements. **Task 5** (the first sections): the RCV-only message-queue `mach_msg2`
(`options 0x4_0400_0102` — `RCV_MSG | RCV_TIMEOUT`, no `SEND_MSG`) is refused deterministically by
`Route::RefuseMqRecv`, and the code it returns,
`MACH_RCV_REFUSAL`, is **chosen here by measurement** against the six binaries M37 parked at exactly
this call (`/bin/launchctl`, `/usr/bin/automationmodetool`, `/usr/bin/desdp`, `/usr/bin/dyld_info`,
`/usr/bin/flex`, `/usr/bin/dddiagnose`). Each candidate code was built into the recorder, ad-hoc
signed, and run record-then-replay against all six; the winner is the code the most binaries accept.
**Task 6** ("The sweep re-baseline", below): the full 54-entry corpus swept once on the close's
binary and diffed row by row against M37's run N, with every moved row explained by name — one of
them (`/bin/ed`) for a reason the plan had not predicted.

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
guest reached) and had `stdout-equal=y` (replay's stdout matched the recording's — a real check
only on the `launchctl` row, whose stdout is its 4,484-byte usage; on the five wall rows and the
two crash rows both `.out` files are empty, so equality there is trivially true and carries no
information). `rc` = record
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
guest dereferencing something derived from the return code) and under `INVALID_NAME` survives 50
landmarks further (receive #381 → wall 431), to its own next wall. The winner is asserted in code by
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

## The sweep re-baseline (Task 6)

One full run of `tools/apple-sweep.sh` over the committed 54-entry corpus, from the worktree,
detached, on the close's binary: commit `911214e` (the head after Tasks 1–5 and Task 5's fix
round; `crates/` and `tools/` unchanged by Task 6, which edits docs, comments and seven
`#[ignore]` reasons only), `target/aarch64-apple-darwin/debug/retrace` copied to a scratch path
and ad-hoc signed (sha256 after signing `80daf5c5add64f5658fd7ea400e5794416a2f88582c2a0770665e9314ba39502`)
so that later `cargo` runs could not swap the binary under the sweep. `RETRACE_SWEEP_KEEP=…/sweep`
kept every non-clean row's `rec.err`/`rp.err`/`rp.out`/`bin`; the traces were read (below) and
then removed — no `.bin` is committed. Log: `sweep.log` (its first two lines are the wrapper's
`pidstart=87610` and the binary's hash/commit/date; the script's own output follows verbatim).

| `pidstart` | recpid range | regime (M36's window `[0x4000,0x18000)`) | `TALLY` | `SWEEP_EXIT` |
|---|---|---|---|---|
| 87610 | 87626–90026 (`0x1564a`–`0x15faa`) | inside `[0x10000, 0x18000)`, the `os_alloc_once` slab — M37's regime **S**; one regime (spec R6, §4b retired) | `pass=44 fail=10 skip=0` | 0 |

`awk` over the 54 `ROW` lines: `ROW lines=54 recpid min=87626 (0x1564a) max=90026 (0x15faa)`. No
`identical fault` row; no `replay diverged` row; no row whose recorder finished cleanly and whose
replay then disagreed (the class-E halt condition) — every FAIL is a `RECORD ERROR`, a recorder
panic, or the `yes` watchdog.

**Row-by-row diff against M37's run N** (`.superpowers/sdd/2026-09-13-retrace-m37-classb/sweeps/sweep-N.log`
in the main checkout, the baseline `docs/sweep-evidence/2026-09-13-m37/README.md` tabulates; the
comparison keys on the binary's path and compares label, `rc`, `rp` and the `rec_reason` field):
**46 rows unchanged, 8 moved.** The plan predicted seven of the eight by name; the eighth is
`/bin/ed`, ruled at the close (below). The tally moved 45/9 → 44/10 = 45 − `ls` − `ed` + `launchctl`.

| row | M37 run N | M38 | moved by |
|---|---|---|---|
| `/bin/launchctl` | `FAIL` 4/3, `RECORD ERROR: … options 0x404000102: message-queue send without the send+rcv RPC shape` | **`PASS` 1/1** — its own no-argument usage on stdout (4,484 bytes), `exit(1)` on both sides | the RCV refusal (Task 5): the receive is refused `MACH_RCV_INVALID_NAME`, the guest carries on to its usage exit. **Un-parked.** |
| `/usr/bin/automationmodetool` | `FAIL` 4/3, the RCV line | `FAIL` 101/n/a, `recorder panicked: … syscall 374 (374) has no arg_kinds row` | the RCV refusal accepted, then the M33 fail-loud on `kevent_qos` (374), no row. Re-parked, class B (Task 5). |
| `/usr/bin/desdp` | `FAIL` 4/3, the RCV line | `FAIL` 101/n/a, `… syscall 464 (464) has no arg_kinds row` | same, on `openat_nocancel` (464). Re-parked, class B. |
| `/usr/bin/dyld_info` | `FAIL` 4/3, the RCV line | `FAIL` 101/n/a, `… syscall 464 (464) …` | same (one hard-linked xcrun stub with `desdp`/`flex`). Re-parked, class B. |
| `/usr/bin/flex` | `FAIL` 4/3, the RCV line | `FAIL` 101/n/a, `… syscall 464 (464) …` | same. Re-parked, class B. |
| `/usr/bin/dddiagnose` | `FAIL` 4/3, the RCV line | `FAIL` 101/n/a, `… syscall 345 (345) has no arg_kinds row` | same, on `statfs64` (345), 50 landmarks past the receive. Re-parked, class B. |
| `/bin/ls` | `PASS` 1/1 — while printing `ls: .: Bad file descriptor` | `FAIL` 101/n/a, `… syscall 461 (461) has no arg_kinds row` | `AT_FDCWD` (Task 3): the sentinel is honoured, `fstatat64(0xfffffffe, …)` succeeds (landmark #258 of 263 events, read off the sweep's kept `ls.bin` with the scratch reader before the trace was removed from the tree — the `ROW` line's `landmark` field is `n/a` because no replay ran; the trace is not committed, so the number is uncorroborated by anything in the repo beyond this README), and `ls` runs on to `getattrlistbulk` (461), which has no row — the M33 fail-loud. A **false PASS became a named wall** (the Task 3 ruling). |
| `/bin/ed` | `PASS` 2/2 — `ed`'s own error exit, its message lost | `FAIL` 101/n/a, `… syscall 464 (464) has no arg_kinds row` | `AT_FDCWD` (Task 3), the same mechanism on a row the plan had assumed would stay PASS — measured below and ruled at the close: a **false PASS became a named wall**. |

The three rows the plan said would keep their label did: `/bin/sh` is `PASS` 1/1 with the same
guest text (`Failed to exec /bin/bash as variant for /bin/sh (14: Bad address).`) and its stderr
now carries `[retrace] refusing execve (syscall 59): exec-in-place is unmodelled; returning errno
14 without forwarding` (`sweep/sh.rec.err`, a record of the same binary and invocation taken beside
the sweep, since a PASS row's stderr is not kept); `/bin/csh` and `/bin/tcsh` are `FAIL` 4/3 at the
same `msgh_id 3403` line (`fork`'s `mach_ports_register`), landmarks 336 and 344 (M37 run N: 331
and 329; the spread is the guest's own, as M37's audit 3 found, plus the new `dup` landmarks the
next subsection explains), with their `pipe` landmark moved. `/usr/bin/yes` is the watchdog as
before (`timed out after 30s recording`, rc 137). Every other row is byte-for-byte its M37 label
with `rc = rp`.

### `/bin/ed` — the row the plan did not predict, measured

Spec §3c said `ls` and `ed` "were PASS before (deterministic EBADF on both sides) and stay PASS;
their recorded output changes". Task 3 measured that false for `ls` (its `PASS` had been `ls`
printing an error and exiting, and with the sentinel fixed it reaches 461); the sweep measured it
false for `ed` in exactly the same way. Two traced records of `/bin/ed` (stdin `/dev/null`, as the
sweep runs it), one on the **M37 baseline binary** (`retrace-aa8d7b8`, the M37 sweeps' own signed
copy) and one on the M38 sweep binary, `sweep/ed.m37-baseline.traced.rec.err` and
`sweep/ed.traced.rec.err`:

- **M37 binary, rc 2.** Trap #261 of 264 (log line 398) is `num=470 … args=[0xfffffffe,0x100010080,0x27ff2f0,0x0,…]`
  — `fstatat64(AT_FDCWD, "/tmp/ed.XXXXXX", …)`: `0x100010080` is `ed`'s scratch-buffer template
  (`strings /bin/ed` has `/tmp/ed.XXXXXX`; the call is libc `mkstemp`'s `_gettemp` checking the
  directory). M33 measured this call `EBADF` ("on `/bin/ed`, once"); the trace shows what follows
  it directly: `num=412` `writev_nocancel(2, …)` (`ed`'s error message, lost to EFAULT — the
  nested-pointer entry in the README), `num=60` `umask`, `num=1` `exit(2)`. So the M37 `PASS 2/2`
  was `ed` failing to create its scratch buffer on both sides and exiting 2 — a determinism
  agreement about a failure, never a run of `ed`.
- **M38 binary, rc 101.** The same landmark (trap #256 of 257, log line 393, the same arguments)
  succeeds — read off the traced run's own trace, `#256 syscall 470 args=[0xfffffffe,
  0x100010080, …] ret=0 err=false writes=1` of 257 events, and off the sweep's kept `ed.bin` as
  `#258` of 259 (the two-landmark difference is the run-to-run spread M37's audit 3 measured on
  every row; neither trace is committed) — and the **next** trap is `num=464 …
  args=[0xfffffffe,0x100010080,0xa02,0x180,…]`:
  `openat_nocancel(AT_FDCWD, "/tmp/ed.XXXXXX", O_RDWR|O_CREAT|O_EXCL, 0600)`, `mkstemp`'s open,
  the `_nocancel` twin of `openat` (463). 464 has no `arg_kinds` row, so `forwarded_shape` panics
  by name (the M33 fail-loud); the recorder exits 101 with no terminal event and the harness
  labels the row before any replay runs.

**Ruling (close, Task 6 — the ledger's):** `/bin/ed` is handled exactly as `ls` was ruled at Task 3
— a silent lie replaced by a named wall, the honest-gate discipline working. The 464 row is not
added in M38 (Task 5's ruling: the missing rows are the successor's measured scope); `ed` joins
`desdp`/`dyld_info`/`flex` as the fourth corpus binary behind that one `_nocancel` row. Spec §6's
"PASS ≥ 45" — amended at Task 3 to "≥ 44 with `ls` explained (≥ 45 if a gate un-parked)" — is
measured **44 = 45 − `ls` − `ed` + `launchctl`**. `ed` never had a gate (its row was never parked),
so no `#[ignore]` is added; nothing halts. Cost if wrong: the same as the `ls` ruling — the
operator may want the two-row follow-up immediately, and 464 is one line, `openat`'s row shared by
its `_nocancel` twin.

### The `csh`/`tcsh` `pipe` landmark, refreshed

M37's audit 3 read, off the kept traces, that one landmark before their `fork` wall both C shells
`pipe` and received retrace's raw host read-end `0x17` in `x0` with their own stale `x1` as the
write end, then `fcntl(F_SETFD)`'d both to `EBADF`. With `pipe`'s pair bound (Task 1) the kept
sweep traces show, read with a scratch reader over `retrace-trace` (`#idx syscall N args=[x0..x3]
ret ret1 err writes`; index = position in the event vector, the initial `Snapshot` #0 — M36's
rule):

```
csh.bin  (recpid 87861, 336 events; the 3403 stop is landmark 336)
#327 syscall 42 args=[0x27ff300, 0x27fe298, 0x0, 0x5] ret=0x4 ret1=0x5 err=false writes=0     pipe → (4, 5)
#328 syscall 41 args=[0x4, 0xffffffff, 0x0, 0x5] ret=0x6 ret1=0x0 err=false writes=0          dup(4) → 6
#329 syscall 6  args=[0x4, 0xffffffff, 0x0, 0x5] ret=0x0 ret1=0x0 err=false writes=0          close(4)
#330 syscall 92 args=[0x6, 0x2, 0x1, 0x5] ret=0x0 ret1=0x0 err=false writes=0                 fcntl(6, F_SETFD, 1) → 0
#331 syscall 41 args=[0x5, 0xffffffff, 0x1, 0x5] ret=0x4 ret1=0x0 err=false writes=0          dup(5) → 4
#332 syscall 41 args=[0x4, 0xffffffff, 0x1, 0x5] ret=0x7 ret1=0x0 err=false writes=0          dup(4) → 7
#333 syscall 6  args=[0x4, 0xffffffff, 0x1, 0x5] ret=0x0 ret1=0x0 err=false writes=0          close(4)
#334 syscall 6  args=[0x5, 0xffffffff, 0x1, 0x5] ret=0x0 ret1=0x0 err=false writes=0          close(5)
#335 syscall 92 args=[0x7, 0x2, 0x1, 0x5] ret=0x0 ret1=0x0 err=false writes=0                 fcntl(7, F_SETFD, 1) → 0

tcsh.bin (recpid 89125, 344 events; the 3403 stop is landmark 344)
#335 syscall 42 args=[0x27ff2f0, 0x27fe288, 0x0, 0xa] ret=0x4 ret1=0x5 err=false writes=0     pipe → (4, 5)
#336 … #342 the same six dup/close calls with the same arguments and results (#338 fcntl(6, F_SETFD, 1) → 0)
#343 syscall 92 args=[0x7, 0x2, 0x1, 0xa] ret=0x0 ret1=0x0 err=false writes=0                 fcntl(7, F_SETFD, 1) → 0
```

The guest receives guest fds `(4, 5)` — `ret`/`ret1`, both bound — and does what the C shell does
with a pipe it means to keep across a script's redirections: moves each end above its `FSAFE`
with `dup`/`close`, then sets close-on-exec on the moved ends, and **both `fcntl(F_SETFD, 1)`
succeed** where M37 had two `EBADF`s. The `dup`s are new landmarks (M37's guest skipped them,
since a "descriptor" of `0x17` or `0x27fe298` was already above `FSAFE`), which is where the
row's few extra landmarks against M37 come from. Traced runs taken the same day on the same binary
(`sweep/csh.traced.rec.err`, recpid 91162; `sweep/tcsh.traced.rec.err`, recpid 91174) have the
identical shape at #326/#329/#334 (stop 335) and #328/#331/#336 (stop 337); the `[trap]` line
ordinal equals the landmark index (335 `[trap]` lines ↔ `DIVERGENCE at landmark 335`, the stop
itself being the one unrecorded trap). The `csh`/`tcsh` gate reasons quote the sweep-run
landmarks and name the traced runs. The wall is unchanged: `mach_ports_register` (3403) from
`xpc_atfork_prepare` ← `fork`, class C.

## Files

- `measure.sh` — the measurement script. `measure.tsv` — its 18 `ROW` lines. `measure.log` — its
  full detached log (build lines + rows + the closing `git checkout`).
- `<bin>.<CODE>.rec.err` / `.rp.err` for all 18 cells — kept (all three codes), since the ruling
  rests on the losing codes too (`dddiagnose`'s two crashes are the reason `INVALID_NAME` won). The
  `.rec.out`/`.rp.out` are zero-length for every binary except `launchctl` (whose stdout is its
  usage); `launchctl.MACH_RCV_INVALID_NAME.rec.out` and `launchctl.MACH_RCV_INVALID_NAME.rp.out`
  are kept as the winner's record and replay stdout (4,484 bytes each, byte-identical); the
  zero-length ones and the two losing codes' `launchctl` stdouts were removed.
- `gates.log` — the `apple_walls_e2e --ignored` run (the file's positive control: each parked gate
  prints its wall by name). `gate-<bin>.log` — the per-binary gate runs.
- `rerun.sh` — the shell re-record helper used to confirm the xcrun-path nondeterminism above.
- `sweep.log` — the Task 6 re-baseline's full log (54 `ROW` lines, `TALLY pass=44 fail=10 skip=0`,
  `SWEEP_EXIT=0`). `sweep/<basename>.rec.err` (and `.rp.err`/`.rp.out` where a replay ran) for
  every non-clean row: `automationmodetool`, `csh` (+ `rp.err`, empty `rp.out`), `dddiagnose`,
  `desdp`, `dyld_info`, `ed`, `flex`, `ls`, `tcsh` (+ `rp.err`, empty `rp.out`), `yes`; plus
  `sweep/sh.rec.err` (a record of the PASS row `/bin/sh` taken beside the sweep, kept for its
  refusal line), `sweep/csh.traced.rec.err` / `sweep/tcsh.traced.rec.err` (the `RETRACE_TRACE=1`
  records the refreshed gate reasons cite) and `sweep/ed.m37-baseline.traced.rec.err` /
  `sweep/ed.traced.rec.err` (the `/bin/ed` measurement behind the close's ruling).
- **No `.bin` trace files are committed** (large); Task 5's live under `/tmp/m38-t5/` and Task 6's
  in the session scratchpad, both for the session only. The `yes.bin` the sweep kept (~347 MB, a
  recorder SIGKILLed mid-`write`) was deleted unread, as ruled at M37.
