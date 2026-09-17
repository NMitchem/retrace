# M38-owed — the small items on the owed list, and two forwards that become refusals

*Written 2026-09-16 from M37's "What stays owed" (`docs/status-log.md`, the section after "M38
does not exist"). This is a **fresh charter, not the M32–M38 run's M38**: that run ended at M37
under its own §3 rule, and its log section says so. The number is reused because it is the next
one; the log section this milestone appends opens with a forward pointer to the older one.*

## 1. Purpose

M37 closed the M32–M38 run with every remaining Apple-sweep wall classed C (a capability the tree
does not have) and a long owed list of items that are not walls: descriptor-return gaps, one
sentinel bug that has made `/bin/ls` print `ls: .: Bad file descriptor` since M10, and two
forwards that should never have been forwards. None needs a new subsystem. This milestone takes
the five that are (a) independently small, (b) each testable by a repo-owned fixture asserting on
the difference it makes, and (c) together worth one gate and one sweep re-baseline:

| # | Item | Owed since | Class |
|---|------|-----------|-------|
| 1 | `pipe`'s second descriptor never reaches the guest | M10 (`Ret::FdPair` documented at M33) | fd return |
| 2 | `fcntl(F_DUPFD)`/`F_DUPFD_CLOEXEC` unmodelled; `fcntl`/`ioctl` argument-less commands probed as pointers | M10 / M37 §4b residual | fd return + arg kind |
| 3 | `AT_FDCWD` rejected as EBADF in the 32-bit form real guests pass | M10 t3 (M33 Ruling 10) | sentinel |
| 4 | `execve`(59)/`posix_spawn`(244) forwarded to the host | M2 (M33 Ruling 7, "the operator's call") | refusal |
| 5 | The RCV-only message-queue `mach_msg2` aborts the record (six parked gates) | M36 | refusal |

The operator ruled item 4 on 2026-09-16: **refuse deterministically** (§3d). The operator also
pre-authorised the `TRACE_MAGIC` bump item 1 needs (§3a), which the previous charter listed as a
halt condition.

**Not taken** (§7): `fork`/process creation and modelling (rather than refusing) the RCV-shaped
message-queue call — both class C, both still "Park + HALT" for this milestone.

## 2. The items, located

Every line number is as of `a663051` (M37 merge).

**Item 1 — `pipe`.** `crates/retrace-box/src/lib.rs:966` `host_svc`: the `asm!` block has
`inout("x0") a[0] => ret` and `in("x1") a[1]`; it returns `(ret, carry)` and nothing else.
`apply_and_return` sets `x0` alone. `crates/retrace-arch/src/lib.rs:311` `Ret::FdPair` documents
the consequence: the guest receives retrace's host read-end, unbound, in `x0`, and its own stale
`x1`; both host ends leak in the recorder; every later use is EBADF. `/bin/csh` and `/bin/tcsh`
use both ends one landmark before their `fork` wall (M37 evidence README, audit 3).
`crates/retrace-trace/src/lib.rs:16` — `Event::Syscall { num, args, ret, err, writes, thread }`
has no field for a second return register.

**Item 2 — `fcntl`.** `crates/retrace-arch/src/lib.rs:495`
`SYS_FCNTL | SYS_FCNTL_NOCANCEL => row!(P, [Fd, Scalar, Ptr])` and `:524`
`SYS_IOCTL => row!(P, [Fd, Scalar, Ptr])`: the third argument's kind is fixed per syscall
number, but is command-dependent — an `int` for `F_SETFD`/`F_SETFL`/`F_DUPFD`, a pointer for
`F_GETPATH`/`F_PREALLOCATE`. Under `Ptr` the M37 `Scalar`-skip does not apply, so a small integer
is probed by `host_span` and would be rewritten if it equalled a mapped IPA (M37 measured it
inert on the corpus: `F_SETFD 1` is `1`). `F_DUPFD` returns a NEW descriptor that
`allocates_fd(92)` (false) never binds; no corpus guest issues it (M33 census).

**Item 3 — `AT_FDCWD`.** `crates/retrace-box/src/lib.rs:3179`, in `translate_fds`:
`if (v as i64) < 0 { continue; }`. libc passes `-2` as a 32-bit `int` in `w0`, so `x0 =
0xfffffffe`, which is positive as an `i64`; the lookup fails and the guest gets EBADF. Measured
on `/bin/ls` (twice) and `/bin/ed` (once) at M33. `crates/retrace-box/tests/fdxlat.rs:64` passes
`AT_FDCWD as u64` — the sign-extended form no guest produces — so the test is green while the real
form fails.

**Item 4 — exec.** Rows `59 => row!(P, [Path, NestedSource, NestedSource])`
(`crates/retrace-arch/src/lib.rs:753`) and 244 are forwarded by the generic arm. They fail today
only because `NestedSource` is untranslated: the host kernel reads `argv` at a guest IPA and
returns EFAULT (to be measured, §3d). If nested-pointer translation ever lands, a forwarded exec
**replaces retrace's own process**. There is no named constant for either number; the arms use
literals.

**Item 5 — RCV-only `mach_msg2`.** `crates/retrace-core/src/machmsg.rs:106–114`, `route()`'s
message-queue branch: `SEND_MSG | RCV_MSG` with the MQ bit → `Route::RefuseMqSend` (deterministic
`MACH_SEND_INVALID_DEST`, the guest carries on); any other MQ shape →
`Route::Unsupported("message-queue send without the send+rcv RPC shape")`, which aborts the record
with rc 4. The six parked gates in `crates/retrace/tests/apple_walls_e2e.rs:35–55` all stop at
options `0x404000102` = `MACH64_SEND_MQ_CALL | MACH64_RCV_TIMEOUT (0x100) | MACH64_RCV_MSG (0x2)`:
a **receive with a timeout**, not a send at all — the `Unsupported` string misnames it. The
record arm for `RefuseMqSend` is `crates/retrace-core/src/lib.rs:521`, its replay mirror `:1895`.

## 3. Design

### 3a. `pipe` — capture `x1`, bind both ends (`TRACE_MAGIC` → `RT\x00\x0a`)

**Trace.** `Event::Syscall` gains `ret1: u64` after `ret`. This changes the record's bytes, so
`TRACE_MAGIC` becomes `RT\x00\x0a`; `magic_bumped_for_the_m24_trampoline_vector_padding`
(`lib.rs:298`) is renamed for this bump and pinned to the new value, and
`rejects_prior_format_version` gains a `RT\x00\x09` case beside its `\x02` one. No trace fixture is committed anywhere in the repo (`git ls-files` has no
`.bin`), so nothing in the tree is orphaned; the operator's own scratch recordings are.

**Box.** `host_svc` makes `x1` `inout("x1") a[1] => ret1` and returns `(ret, ret1, carry)`.
`forward_and_diff` returns `(u64, u64, bool, Vec<Region>)` — `(ret, ret1, err, writes)`.
`ret1` is **`0` for every row except `Ret::FdPair`**; for that row, on `!err`, a new
`Box_::bind_returned_pair(host_r, host_w) -> (u64, u64)` allocates the read end first
(`FdTable::alloc`, lowest free, matching xnu's `retval[0]` then `retval[1]` order), binds each to
its host end, and returns the guest numbers, which go into `(ret, ret1)`.

`apply_and_return` and `set_x0_err_and_return` keep their signatures and call sites (the record
arm finishes with the latter — the host already wrote into guest memory — and replay with the
former; the plan measured this, correcting an earlier draft of this paragraph that named an
`apply_and_return_pair`). The `x1` write is one new method, `Box_::set_ret1(ret1)`, and **both**
generic dispatch arms call it **only when `returns_fd_pair(num)`** — one method, same argument,
both sides (symmetry rule 1 by construction). This is the narrow choice, made deliberately: xnu writes `x1` from `retval[1]`
after every syscall and retrace leaves the guest's `x1` stale, so a uniform capture would be more
faithful — but it changes every guest's post-syscall `x1` with no corpus measurement behind it,
and this run is unattended. Uniform capture is recorded as a later measurement (§7).

**Replay mirror** (`ReplaySession::advance`, the fd block at `crates/retrace-core/src/lib.rs:2370`):
beside the `SYS_DUP` branch, an `FdPair` branch does two `alloc()`s and compares the pair to the
recorded `(ret, ret1)`; a mismatch is the existing "fd divergence" error. Both sides then call
`apply_and_return` with the recorded `ret1`, so `x1` is set identically.

**`arg_kinds`.** `allocates_fd(42)` stays false (it means "binds ONE fd via `bind_returned_fd`");
a new view `returns_fd_pair(num)` is what `forward_and_diff` and the mirror consult.
`pipe_return_is_a_pair_and_is_not_bound` is rewritten as `pipe_return_is_a_pair_and_both_are_bound`.
`Ret::FdPair`'s doc comment loses its "unmodelled" paragraph and keeps the M37 evidence citation as
history.

**Recorder hygiene.** Both host ends are now in the table, so a guest `close` retires them
through the existing path and the leak `Ret::FdPair` names is gone.

**Fixture + gate.** `crates/retrace-guest/c/pipe_dyn.c`, built by `build.rs` like `dup2_dyn`:
`pipe(p)`; `write(p[1], "pipe\n", 5)`; `read(p[0], buf, 5)`; print `p[0]`, `p[1]` and the bytes
to stdout; `close` both; exit 0. `crates/retrace/tests/pipe_e2e.rs` asserts:
1. the recorded `pipe` event has `(ret, ret1) == (3, 4)` — the difference this task makes is a
   guest-numbered write end that exists at all;
2. the guest's stdout carries `3 4 pipe`;
3. replay is byte-identical (the usual helper).
Exit code is not asserted (honest-gate rule 1).

### 3b. `fcntl`/`ioctl` per-command kinds; `F_DUPFD` modelled

**`arg_kinds`.** A new `pub fn shape_of(num: u64, args: &[u64; 8]) -> Shape`. For `SYS_FCNTL`,
`SYS_FCNTL_NOCANCEL` and `SYS_IOCTL` it consults a per-command table keyed on `args[1]`; for
every other number it returns `forwarded_shape(num)`. The table:

| syscall | command | third arg | source |
|---------|---------|-----------|--------|
| fcntl | `F_DUPFD` 0, `F_GETFD` 1, `F_SETFD` 2, `F_GETFL` 3, `F_SETFL` 4, `F_NOCACHE` 48, `F_DUPFD_CLOEXEC` 67 | `Scalar` | `sys/fcntl.h`; the M33 census set plus the two dup commands |
| fcntl | `F_PREALLOCATE` 42, `F_GETPATH` 50, `F_ADDFILESIGS_RETURN` 97, `F_CHECK_LV` 98 | `Ptr` | the M33 census set, bounds as the row comment already cites |
| ioctl | `FIOCLEX`, `FIONCLEX` | `Scalar` | `sys/ioctl.h`, argument-less |

**An unlisted command keeps today's `Ptr`** — the default is unchanged behaviour, not a panic. A
fail-loud default would turn every command the census has not seen into a new sweep failure, which
is a halt condition for a gap this milestone did not create. The row comment names the default and
the census date it was tabled against.

**`F_DUPFD` / `F_DUPFD_CLOEXEC`.** The third argument is a **guest** minimum: the call returns the
lowest guest fd ≥ `min`. Modelled as a table operation with a host `dup` behind it, exactly as M37
did `dup2`: a `Box_::guest_fcntl_dupfd(args)` short-circuit beside `guest_dup2` at the top of
`forward_and_diff`, entered when `num` is an `fcntl` spelling and `args[1]` is one of the two
commands. It: resolves the source's host fd (EBADF if none); `libc::dup(h)` on the host (**never
`F_DUPFD` on the host** — its minimum would be a host number); `FdTable::dup_from(src, min)`, a
new table method that takes the lowest free slot ≥ `min` and gives it the **source slot's kind**
(M37's `dup` rule, so `F_DUPFD` on a console fd stays a console alias the M9 mirror catches);
binds; returns `(g, 0, false, [])`. `F_DUPFD_CLOEXEC`'s close-on-exec bit has no observable in
the box (exec is refused, §3d), so both commands share the path. Replay's fd block mirrors it with
`fds_mut().dup_from(src, min)` — the same table method record calls — and compares.

**`forward_and_diff`.** The `Scalar`-skip loop and `translate_fds` consult `shape_of(num, &args)`
instead of `forwarded_shape(num)`. `translate_fds`'s signature gains nothing: it already has
`args`.

**Fixture + gate.** `crates/retrace-guest/c/dupfd_dyn.c`: `open` a temp file the test passes as
`argv[1]`; `n = fcntl(fd, F_DUPFD, 10)`; `write(n, "dupfd\n", 6)`; `fcntl(n, F_SETFD, 1)`; print
`n`; close both. `crates/retrace/tests/dupfd_e2e.rs` asserts:
1. the recorded `F_DUPFD` event returned exactly `10` — a guest number honouring the guest
   minimum, which no host `dup` could produce, is the difference;
2. the file's bytes are `dupfd\n` after record (written through the dup) — asserted on the file,
   never the exit code, per `bigwrite_e2e`'s rule;
3. the `F_SETFD` event's `args[2] == 1` was forwarded verbatim (it is `Scalar` now; the assertion
   is on the trace, since the probe's rewrite was inert on this value anyway — the test documents
   the kind, it cannot prove the skip);
4. replay is byte-identical.

### 3c. `AT_FDCWD` in the form real guests pass

`translate_fds`: `(v as i64) < 0` → `(v as i32) < 0`. A `u64 → i32` cast truncates to the low
32 bits, so both `0xfffffffe` and `0xffff_ffff_ffff_fffe` read as `-2`; a real descriptor never
has bit 31 set. The comment says which form the ABI actually delivers and cites M33 Ruling 10.

`fdxlat.rs:64`: the sentinel test passes `0xfffffffe` — the measured form — as its primary
case and keeps the sign-extended form as a second assertion, so the test can no longer be green
while the real form fails.

**Fixture + gate.** `crates/retrace-guest/c/atfdcwd_dyn.c`: `fstatat(AT_FDCWD, ".", &st, 0)`;
print `ok` or `errno`. `crates/retrace/tests/atfdcwd_e2e.rs` asserts the recorded `fstatat64`
(470) event has **both** `args[0] == 0xfffffffe` and `err == false`. The first half pins the form
the guest passed, so the fixture cannot pass by accident with a 64-bit sentinel; the second is the
fix.

Sweep effect, measured at the close and explained in the evidence README, not asserted by a gate:
`/bin/ls` stops printing `ls: .: Bad file descriptor`; `/bin/ed`'s `fstatat64` succeeds. Both rows
were PASS before (deterministic EBADF on both sides) and stay PASS; their recorded output changes.

### 3d. `execve`/`posix_spawn` — refused, never forwarded

**Measure first.** `RETRACE_TRACE=1` on the CPython launcher (`cpython_e2e`'s `LAUNCHER`) and on
`/bin/sh` (the corpus's `execve` user): the `(ret, err)` the forward returns today for 59 and 244.
The expectation is `EFAULT` (14) for both; whatever is measured becomes
`retrace_arch::EXEC_REFUSAL_ERRNO` with the measurement in its doc comment. **The value is chosen
for continuity, not fidelity**: it keeps the launcher test's assertions, `/bin/sh`'s sweep row and
every existing trace byte-identical. The doc says so in those words, and names `ENOSYS` as the
one-constant change if a later milestone prefers "exec is unmodelled" to be what the guest reads.

Named constants `SYS_EXECVE = 59` and `SYS_POSIX_SPAWN = 244` are added; the two rows keep their
kinds with a comment that the row is documentation now — nothing consults it for forwarding.

**Record arm** in `record_box`, placed **before the generic forward arm** (symmetry rule 1's
ordering, the only guard, as for `bsdthread_create`): on `num == SYS_EXECVE || num ==
SYS_POSIX_SPAWN`, `eprintln!` one refusal line naming the syscall, append
`Event::Syscall { num, args, ret: EXEC_REFUSAL_ERRNO, ret1: 0, err: true, writes: vec![], thread }`,
`apply_and_return`. **Replay mirror** inside the generic `Syscall` arm of `advance`, placed
directly after that arm's existing `(num, args)` compare and `verify_thread` (the position the
`SYS_SIGACTION` mirror occupies): recompute the same constant and empty write set, byte-compare
against the recording, `apply_and_return`, `finish_event`. **No new `verify_thread` site**: the
arm's own call already ran, so the count stays at seven (an earlier draft of this paragraph said
"eighth site"; the plan corrected it from the code). The same is true of §3e's mirror, which
sits inside the mach-trap arm beside `RefuseMqSend`'s.

**Gate.** `the_launcher_records_and_replays_its_own_posix_spawn_failure` keeps every assertion it
has (that *is* the continuity check) and gains one — and the obvious one is **wrong**: asserting
the recorded event has `ret == EXEC_REFUSAL_ERRNO` and `writes.is_empty()` is exactly what the
*forwarded* call produces today (an EFAULT with nothing written), so it cannot tell refusal from
forward — honest-gate rule 1. The difference the arm makes that a forward cannot fake is its
stderr line: the test asserts the recorder's stderr contains the refusal line
(`[retrace] refusing posix_spawn …`), which only the refusal arm prints. The trace assertion is
kept beside it as the continuity half (the guest saw the same errno), labelled as such. Nothing
guards `bsdthread_create`'s arm position today (M37 measured it), and this milestone adds no
structural guard for its own arm either: the stderr assertion is the guard, and an arm that
drifted below the generic forward would go red on it.

### 3e. RCV-only message-queue `mach_msg2` — refused deterministically

**Router.** In `route()`'s MQ branch, between the SEND|RCV case and the fallthrough:
`RCV_MSG set, SEND_MSG clear` → `Route::RefuseMqRecv`. A one-way send (`SEND_MSG` without
`RCV_MSG`) stays `Unsupported`, with its string corrected to say what it is. The
`Msg2` decode already carries `options`; nothing new is read.

**The code, by measurement.** `pub const MACH_RCV_REFUSAL: u64`, default candidate
`MACH_RCV_TIMED_OUT (0x1000_4003)` — the options carry `RCV_TIMEOUT`, and a receive on a queue
with no sender in the box times out; that is the faithful answer, not a stub. The task tries the
plausible codes against the six binaries the M23-S6 way — `MACH_RCV_TIMED_OUT`,
`MACH_RCV_INVALID_NAME (0x1000_4002)`, `MACH_RCV_PORT_DIED (0x1000_4006)` — records for each
binary whether it proceeds past the call, retry-loops, or `brk`s, and keeps the code under which
the most binaries proceed. Ties go to `MACH_RCV_TIMED_OUT`. The table goes into the constant's doc
and the evidence README; the ruling is §8 R3.

**Record arm + replay mirror.** Clones of the `RefuseMqSend` pair at `lib.rs:521` / `:1895`:
writes nothing, returns the constant, replay recomputes and byte-compares. The receive buffer is
left untouched, which is what makes both the return and the empty write set constants.

**Gates.** Run the six `apple_walls_e2e` tests with `--ignored`. Each outcome is one of:
- **record + replay clean** → un-`#[ignore]`, and the README's sweep count moves;
- **record stops at a new wall** → the gate stays `#[ignore]`d with a **rewritten** reason naming
  the new landmark, `(num, args)`/`msgh_id`, the recorder's pid, and the evidence files under
  `docs/sweep-evidence/2026-09-16-m38/<bin>.{N}.{rec,rp}.err` (one pid regime is enough now that
  §4b is retired; the reason says so);
- **record clean, replay diverges** → class E → **halt** with the branch intact.
Re-parking an already-ignored gate is not a new `#[ignore]` and does not halt.

### 3f. Symmetry obligations (all five)

| Item | Record half | Replay mirror | Comparison |
|------|-------------|---------------|------------|
| pipe | `bind_returned_pair` | two `alloc()`s in the fd block | `(ret, ret1)` vs recorded |
| F_DUPFD | `FdTable::dup_from(src, min)` | same method, same args | `ret` vs recorded |
| AT_FDCWD | `translate_fds` (record only — replay forwards nothing) | none needed: the sentinel never reaches the trace | `(num, args)` oracle unchanged |
| exec | constant `(exec_refusal_errno(num), true, [])`, an arm before the generic forward | recompute + byte-compare, inside the generic replay arm after its `verify_thread` | standard posture |
| RCV mach_msg2 | constant `(MACH_RCV_REFUSAL, false, [])` | recompute + byte-compare, inside the mach-trap arm beside `RefuseMqSend`'s | standard posture |

Neither refusal is the `ServiceGetSpecialPort` verbatim-apply exception: both replies are
constants, so replay regenerates them. Neither adds a `verify_thread` site; the count stays
seven. `EXEC_REFUSAL_ERRNO` is spelled `retrace_arch::exec_refusal_errno(num) -> Option<u64>`
in the plan — `Some` doubles as the predicate, and it can carry two values if the two syscalls
measure differently (R4).

## 4. Task order and why

1. **pipe** — the format bump lands first, so every evidence trace this run keeps is `RT\x00\x0a`
   and the sweep is run once against one format.
2. **fcntl** — same fd-table family; its fixture may reuse pipe's pattern.
3. **AT_FDCWD** — smallest; ahead of the refusals so the close's sweep diff has one fd-table
   explanation and two refusal explanations, not an interleaving.
4. **exec refusal** — measure, then refuse.
5. **RCV refusal** — measure the code, refuse, re-run the six.
6. **close** — §9.

Tasks 1–5 each: worktree, TDD (red first — the pre-fix run is kept as a log, M37's review asked for
that), subagent review, merge to local main. Items 3, 4, 5 change what the sweep records; the
re-baseline is once, at 6.

## 5. Envelope

As the M32–M38 charter's, with two changes the operator made on 2026-09-16:

- **Push once at the close.** Every prior run held "never push"; this one pushes local `main`
  after the close commit. Nothing else is pushed — no branch, no mid-run state.
- **The `TRACE_MAGIC` bump is pre-authorised** (§3a). It is not a halt.

Unchanged halt conditions — stop with the branch intact and a written explanation, never a best
guess, never a silent narrowing:
- a red gate surviving one fix round;
- any **new** `#[ignore]` (a gate not ignored at `a663051`); re-parking one of the ten with a
  rewritten reason is not new;
- a class-E sweep row (record/replay disagree between runs of the same binary);
- any task needing scope this spec lacks.

A measurement that contradicts this spec's premise is a §8 ruling and a re-scope, written down —
not a halt, never silent.

## 6. Acceptance

- `pipe_e2e`, `dupfd_e2e`, `atfdcwd_e2e` green, each asserting the difference named in §3.
- The launcher test green with its new assertion; `/bin/sh`'s sweep row unchanged.
- The six RCV gates each un-ignored or re-parked with a rewritten reason and evidence; none newly
  ignored; no class-E row.
- Sweep: PASS count ≥ 45 (M37's), every moved row explained by name in the evidence README, no
  row moved by anything this spec does not name.
- Gate: chunked, `--no-fail-fast`, exit codes captured before any pipe, `--bins` chunk run,
  `--doc` beside any per-target library split; total reconciled file-by-file against 596/0/10
  over 131.
- `Ret::FdPair`'s doc, the fcntl row comment, CLAUDE.md's `verify_thread` count and `TRACE_MAGIC`
  value, README "What works today"/"Known limits", and the status-log section all describe the new
  reality; the old "M38 does not exist" section carries a forward pointer.

## 7. What this milestone deliberately does not do

- **No `fork`/process creation.** Class C; `csh`/`tcsh` stay parked at 3403 with their reasons
  untouched except for the pipe landmark, which will now show two guest fds where it showed the
  raw host read-end and a stale `x1` — the reason's quoted landmark is refreshed, the wall is not.
- **No modelling of the RCV-shaped call.** Refusal only. If a binary needs a real receive (a
  reply on a port it actually holds), the refusal moves it one wall, and the new reason says so.
- **No uniform `x1` capture.** Narrow to `FdPair` (§3a); the uniform version is a measurement a
  later milestone can take with the field already in the trace.
- **No fail-loud default for unlisted `fcntl`/`ioctl` commands.** `Ptr` as today (§3b).
- **No nested-pointer translation.** Untouched; exec's refusal is precisely what makes it safe to
  add later.
- **No `pipe` binding model beyond two slots** — no pipe semantics (no buffered bytes on replay
  beyond what the recorded `read` writes carry), which is already what record/replay does for any
  host-backed fd.
- **No `bsdthread_create` ordering assert.** M37 measured its absence; adding it is a separate
  decision.
- **Nothing else on the owed list**: canary fill, `csops` ERANGE, band width, `getattrlistbulk`,
  `__disable_threadsignal`, console `writev` mirroring, the `MADV_FREE_REUSABLE` question — all
  carried forward unchanged.

## 8. Rulings (made while writing this spec)

- **R1 — the milestone is M38.** The log's "M38 does not exist" is a statement about the M32–M38
  charter's slot; this is a fresh charter reusing the next number. The old section gets a forward
  pointer; the new section's first paragraph says this.
- **R2 — `x1` narrow, not uniform.** §3a. Cost if wrong: a guest that reads `x1` after some other
  two-register syscall (`fork` is the only other one in the ABI, and it is class C) still sees a
  stale value — today's state.
- **R3 — the RCV refusal code is measured, default `MACH_RCV_TIMED_OUT`, ties to the default.**
  Written at task 5 with the table. Cost if wrong: a binary that would have proceeded under
  another code re-parks one wall early; the reason names the code tried.
- **R4 — the exec errno is the measured one, for continuity.** §3d. Cost if wrong: the guest reads
  `EFAULT` for a call that was refused, not faulted — a wording lie the doc comment owns; one
  constant to change.
- **R5 — unlisted `fcntl`/`ioctl` commands stay `Ptr`.** §3b. Cost if wrong: a scalar command
  outside the census that equals a mapped IPA is rewritten — the §4b class, measured inert on
  every command seen.
- **R6 — one pid regime for the re-run of the six.** §4b is retired (M37: 0 self-pid ESRCH on
  every regime), so N/I/S no longer distinguish anything on these rows. The reasons say "run N"
  and why.

## 9. Gate prediction

596/0/10 over 131 at `a663051`. Expected at close: +3 e2e gates (`pipe_e2e`, `dupfd_e2e`,
`atfdcwd_e2e`), +1 `retrace-trace` case (the `\x09` rejection), +1 `retrace-arch` (`shape_of`),
+1 `machmsg` unit test (the RCV-only shape routes to `RefuseMqRecv`, the one-way send still to
`Unsupported`), +1 `fdxlat` assertion (same test, not a new count), and the launcher test's
added assertions (same test). Binaries: +3 (one per new e2e target). Ignored:
**≤ 10** — the six RCV gates may each un-ignore; the four others are untouched. So roughly
**603+/0/≤10 over 134**, to be reconciled file-by-file, not trusted.

## 10. Outcome

*Appended at the close (Task 6, 2026-09-16/17), after the gate and the sweep.*

**Against §9.** Measured: **617 passed / 0 failed / 9 ignored over 135 binaries** on `911214e`
(the head after Tasks 1–5 and Task 5's fix round), every chunk's cargo exit 0, clippy clean, no
`SKIPPED` line (Homebrew `jq` and `python@3.14` present). Two predictions preceded the run and
each is corrected against the document it came from — the per-file reconciliation
(`task-6-numbers.md`, diffed against `a663051`'s 596/0/10 over 131) found both, and its own
first draft had conflated them (the Task 6 review caught that). **§9** said "roughly 603+/0/≤10
over 134": it counted **three** new e2e gates (`pipe_e2e`, `dupfd_e2e`, `atfdcwd_e2e`) where
there are four (`exec_e2e` is the plan's, added at Task 4 beside the launcher assertion §3d
asked for; this spec never named it), so its
binaries were +3 where the four new targets are **+4** (135, not 134); and it had no parse tests.
**The plan's Task 6 Step 7** said "612 + k / 0 / 10 − k over 135", k = 1 → 613/0/9 over 135: it
counted the binaries right and missed only the **four `retrace-guest` parse tests**
(`pipe_guest_parses`, `dupfd_guest_parses`, `atfdcwd_guest_parses`, `exec_guest_parses`, one per
new fixture, +4). The rest of the plan's per-file prediction held: `retrace-arch` +3, `machmsg.rs` +3, `fdtable.rs` +2,
`fdxlat.rs` +1, `pipe_e2e` +3, `dupfd_e2e` +2, `atfdcwd_e2e` +1, `exec_e2e` +1; `#[ignore]`
10 → 9; `--bins` 11 → 11. 596 + 20 + 1 (the un-parked gate now counts as passed) = 617.

**The exec errno (§3d, R4).** Both spellings measured **14 (`EFAULT`)** on the unmodified
recorder — `exec_dyn`'s pre-fix trace: `num=59 ret=14 err=true writes=0`, `num=244 ret=14
err=true writes=0`; guest stdout `execve=14` / `posix_spawn=14`; zero `refusing` lines — so
`exec_refusal_errno` returns `Some(14)` for both and no per-number split was needed. R4 stands as
written: continuity, not fidelity. `/bin/sh`'s sweep row is unchanged (`PASS` 1/1, the same
guest text) and its stderr gains the refusal line; the CPython launcher test kept every assertion
and gained the stderr one.

**The RCV refusal code (§3e, R3).** Measured over the six, 18 cells (three codes × six binaries;
the table is in `docs/sweep-evidence/2026-09-16-m38/README.md` and on `MACH_RCV_REFUSAL`'s doc):
`MACH_RCV_TIMED_OUT` accepted by 5 of 6, `MACH_RCV_INVALID_NAME` by **6 of 6**,
`MACH_RCV_PORT_DIED` by 5 of 6 — **not a tie**, so R3's default and tie-break did not apply and
the constant is `MACH_RCV_INVALID_NAME` (0x1000_4002). The decider was `dddiagnose`, which
data-aborts in the guest ~10 landmarks after the receive under both losing codes and under the
winner runs 50 landmarks further (381 → 431) to a wall of its own. The semantically faithful
default lost on one binary; the test `the_receive_refusal_is_a_receive_code` pins the measured
choice. One pid regime (R6): all 18 cells at recpids 54954–55330 — which the plan pre-labelled
"N" and which are in fact inside M36's old `[0x4000, 0x10000)` window, M37's regime **I** (a
Task 5 review finding, corrected in the README and the five reasons); irrelevant to the ruling
since §4b is retired, and 0 self-pid `ESRCH` was measured on every winner trace.

**Which of the six moved.** One un-parked: **`/bin/launchctl`** records to its own no-argument
usage `exit(1)` (4,484 bytes, byte-identical to the host's native output) and replays
bit-for-bit; its gate asserts on that outcome, never on `rc == 0`. Five re-parked, class B, one
syscall past the receive at a **missing `arg_kinds` row** (the M33 fail-loud, rc/rp 101/3):
`automationmodetool` → `kevent_qos` (374); `desdp`/`dyld_info`/`flex` (hard links of one xcrun
stub) → `openat_nocancel` (464); `dddiagnose` → `statfs64` (345). None class E; no new
`#[ignore]`; no halt. **Ruling (Task 5):** those rows are not added in M38 — scope this spec
lacks (§5) — and join 461/468 as the successor's *measured* scope: **the missing-row set — 461,
468, 464, 345, 374 — with the corpus binaries each blocks**, at the top of the owed list; 464 is
the `_nocancel` twin of `openat` (463), precisely the documented nocancel trap, and 345 the
fixed-struct twin of `fstatfs64` (M29).

**The sweep.** One run on the close's binary (`911214e`, recorder pids `0x1564a`–`0x15faa`):
**`TALLY pass=44 fail=10 skip=0`**, 46 rows unchanged against M37's run N, **8 moved**, every one
explained by name in the evidence README — the six above, and two this spec got wrong:

- **§3c was wrong about `ls`, and about `ed`.** It said both "were PASS before … and stay PASS;
  their recorded output changes". Task 3 measured `ls`: with the sentinel honoured its
  `fstatat64(AT_FDCWD, …)` succeeds and it runs on to `getattrlistbulk` (461), which has no row —
  its M37 `PASS` had been `ls` printing an error and exiting, identically on both sides.
  **Ruling (Task 3):** the 461/468 rows are not added in M38 (an unmeasured `Dest` row at the end
  of an unattended run is the "right conclusion, unmeasured fact" class); the row moves from a
  false PASS to a loud named wall, and §6's "PASS ≥ 45" is amended to "≥ 44 with `ls`'s move
  explained (≥ 45 if a gate un-parked)". The close's sweep then measured the same for **`ed`**,
  which the plan had not predicted: on the M37 binary its `fstatat64(AT_FDCWD, "/tmp/ed.XXXXXX")`
  — libc `mkstemp`'s directory check — was `EBADF`, `ed` wrote its error (lost to EFAULT) and
  `exit(2)` on both sides; on the M38 binary the stat succeeds and the next trap is
  `openat_nocancel(AT_FDCWD, …, O_RDWR|O_CREAT|O_EXCL, 0600)`, 464, no row. **Ruling (Task 6,
  the ledger's):** `ed` is handled exactly as `ls`; the floor is measured **44 = 45 − `ls` − `ed`
  + `launchctl`**; `ed` joins 464's blockers (four binaries behind one row); no gate is added
  because `ed` never had one. §6's acceptance is met as amended.
- `/bin/sh`'s label is unchanged and its stderr carries the refusal line (§6). `csh`/`tcsh` are
  unchanged at 3403 with their `pipe` landmark moved: the guest receives `(4, 5)`, moves both
  ends above `FSAFE` with `dup`/`close`, and both `fcntl(F_SETFD, 1)` succeed (M38 sweep `csh`
  #327/#330/#335, `tcsh` #335/#338/#343) where M37 had two `EBADF`s on a raw host descriptor and
  a stale register (§7's prediction, measured).

**What this spec's own text got wrong, recorded as history.** Two corrections were applied when
the plan was written from the code and are only recorded here: §3a's first draft named an
`apply_and_return_pair` that never existed — the `x1` write is `Box_::set_ret1`, called by both
dispatch arms under `returns_fd_pair`, and `apply_and_return`/`set_x0_err_and_return` kept their
signatures; and §3d's first draft said the exec mirror was an "eighth `verify_thread` site" —
there is none, both refusal mirrors sit inside existing generic arms after those arms' own
`verify_thread`, and the count stayed **seven** through every task. Two were decided by
measurement: §3c's "stay PASS" (above) and §3e/R3's default code (above). A third was measured
at Task 1: §3a's "`(ret, ret1) == (3, 4)`" came out as **(4, 5)** — libSystem holds one extra
descriptor under retrace — so the fixture asserts the invariants (`pair=1`, `low=1`) instead of
the numbers (`pipe_dyn.c:1–5`). One was a label: R6's
"run N" was regime I. §9's prediction is corrected above. §2's location line numbers are as of
`a663051` and shifted under each task, as expected.

**Rulings that held.** R1 (the milestone is M38 under a fresh charter; the old "M38 does not
exist" section carries a forward pointer), R2 (`x1` narrow — `pipe_e2e` asserts no other row
carries a non-zero `ret1`), R4 (continuity errno, one value), R5 (unlisted commands keep `Ptr`;
the row comment names the default and the census date). R3 held in its *procedure* — the code is
measured — and its default did not apply. R6 held with its label corrected.

**Owed by this milestone, beyond §7's list:** the missing-row set above (first); uniform `x1`
capture (a measurement, with the field now in the trace); a fail-loud default for unlisted
`fcntl`/`ioctl` commands (deliberately not taken, R5); modelling rather than refusing the RCV
call (class C); the `guest_fcntl_dupfd` range guard living record-side only (a review minor,
carried to the final review — which raised it to Important 1 and had it moved into
`FdTable::dup_from` in the fix wave, so it is paid, not owed); and the review minors each
task's report deferred.

