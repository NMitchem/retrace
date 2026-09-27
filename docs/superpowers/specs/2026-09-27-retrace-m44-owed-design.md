# M44-owed: the missing rows, the swallowed skips, and M43's debugger debts

**Date:** 2026-09-27. **Branch:** `worktree-m44-owed`, to be cut from `main` at the M43 merge
(`64e471e`, pushed 2026-09-27). **Companion:** `2026-09-27-retrace-m44-owed-measurements.md`, written
by t0 (§3a) and cited by section once it exists. Until then every claim below says where it comes
from: **SDK** (`MacOSX.sdk/usr/include/sys/syscall.h`, read 2026-09-27), **code** (read, with a
path), **docs** (README / status log, with a line), or **inferred**. An inferred claim names the
t0 measurement that owes it.

**Approach:** chosen by the operator in brainstorming on 2026-09-27: a measurement task first, then
two independent tracks — breadth (A) and debugger debts (B) — in one milestone, one gate, one close.
**Corrected from the plan, 2026-09-27:** §3a M1 and M5, §3b A2, §3c B3 and B6, and R4. Writing
executable steps against the code found five things the prose had wrong or missing; §11 lists each.

## 1. Purpose

M43 closed the original design's v1 exit (2026-07-05 spec, its M4 and M5: "reverse-step through a
real crash in LLDB", the CPython demo). What is left has no endpoint in the docs; it is breadth and
hardening. M44 pays the owed list that is cheapest per line of change, plus the debugger debts M43
itself listed:

- **Breadth.** Five syscall numbers the Apple-binary corpus reaches have no `arg_kinds` row, so
  the recorder panics by name at `forwarded_shape` (`crates/retrace-arch/src/lib.rs:945`, the M33
  fail-loud) on seven binaries — `ls`, `ed`, `desdp`, `dyld_info`, `flex`, `dddiagnose`,
  `automationmodetool` (docs: README "Known limits", the ten-row table). One of the five is
  mis-numbered (§2a). A sixth `_nocancel` gap exists that no binary reaches yet (§2c).
- **Honest skips.** Eight test targets print their `SKIPPED` line with `eprintln!`, which libtest
  captures for a passing test, so no ordinary gate log can show a skip (docs: status log M43
  "What stays owed", Ruling F-4; CLAUDE.md "Honest-gate discipline").
- **One fd-table fidelity gap** owed since M38: `F_DUPFD_CLOEXEC` does not set close-on-exec on the
  host `dup`, so a forwarded `F_GETFD` reads 0 where native reads 1 (docs: README, the descriptor
  limit).
- **Debugger debts** from M43's "What stays owed" (status log, M43 section): a faster backing lookup
  for stepping, `disarm-rsi`, a step that ends at its own thread's exit, lldb rows for
  `step-out`/`finish`/`ni`/`next`, an arm64e backtrace-depth probe, and two unmeasured stepping
  behaviour changes.

**Success** is not "every gate green". It is: every targeted binary either records and replays
clean with its gate un-ignored, or is re-parked at a **new, measured** wall with its `#[ignore]`
reason rewritten; every skip line reaches a gate log; each debugger item ships with a test that
asserts the difference it makes, or is routed onward with its measurement.

## 2. What is known before t0

### 2a. 468 is `fchownat`, not `getattrlistat`

SDK: `SYS_getattrlistbulk 461`, `SYS_openat_nocancel 464`, `SYS_fchownat 468`,
`SYS_getattrlistat 476`, `SYS_statfs64 345`, `SYS_kevent_qos 374`.

Docs have called 468 `getattrlistat` since M34 (spec `2026-09-13-retrace-m34-destgaps-design.md:416`,
"No `getattrlistbulk` (461) or `getattrlistat` (468) row"), carried through the status log at
M34/M35/M36/M37/M38/M39/M40 and into README lines 824 and 1249. M38's ledger even wrote out the row
it expected (status log line 10011): "468 `[Fd, Path, Ptr, Dest(Reg(4)), Scalar, Scalar]`". That
is `getattrlistat`'s shape (`int getattrlistat(int fd, const char *path, struct attrlist *alist,
void *attrBuf, size_t attrBufSize, unsigned long options)`). Keyed at 468 it would classify
`fchownat(int fd, const char *path, uid_t owner, gid_t group, int flag)` — x3, the `gid`, would be
treated as a destination pointer whose length is the `flag` word: the clamp and the diff window
would both act on a number. **No corpus binary reaches 468 or 476** (docs: every wall in the table
names 461, 464, 345 or 374; none names 468). Ruling R1 (§8) drops 468 from the owed set.

The pattern is this repo's recurring one — right conclusion, wrong supporting fact — and it
survived seven milestones because nothing checks a number against its name. A1's test (§3b) reads
names and numbers from the same SDK header, which is the structural answer for the `_nocancel`
half of that class.

### 2b. What each remaining number is, and what is already suspected

- **464 `openat_nocancel`** — `openat`'s twin. The table's own rule (code: the `Source` doc comment,
  "Every `_nocancel` spelling shares its plain form's row") says it shares `SYS_OPENAT`'s row
  `[Fd, Path, Scalar, Scalar]`, `Ret::Fd`. Reached by `ed`, `desdp`, `dyld_info`, `flex` (docs).
  **Inferred, owed by t0 M3:** `desdp`, `dyld_info` and `flex` are hardlinks to one `xcrun`
  trampoline (docs: their `#[ignore]` reasons), and past the open, xcrun very likely `exec`s the
  real tool — which retrace refuses since M38 (docs: "Exec-in-place is unmodelled and refused"). So
  464 probably moves those three to the exec refusal, not to green. `ed` and `dddiagnose` are the
  real candidates for green.
- **345 `statfs64`** — `fstatfs64`'s path twin (SDK: 345/346). `fstatfs64`'s row is `[Fd, Ptr]` with
  the 2,168-byte struct measured at M29 Task 7 (code: `crates/retrace-arch/src/lib.rs:498`), so
  `statfs64` is `[Path, Ptr]`. Reached by `dddiagnose`.
- **461 `getattrlistbulk`** — `int getattrlistbulk(int dirfd, struct attrlist *alist, void
  *attrBuf, size_t attrBufSize, uint64_t options)`. `alist` is the fixed 24-byte struct its
  `getattrlist` siblings already cite. Whether `attrBuf` is `Ptr` (a cited kernel cap inside the
  window, the M34 Ruling 1 precedent for `getattrlist`) or `Dest(Reg(3))` (caller-sized, M38's
  proposal) is owed by t0 M2. Reached by `ls`.
- **374 `kevent_qos`** — `int kevent_qos(int kq, const struct kevent_qos_s *changelist, int
  nchanges, struct kevent_qos_s *eventlist, int nevents, void *data_out, size_t *data_available,
  unsigned int flags)` (xnu's private `sys/event_private.h`; the SDK carries only the number). Two
  hazards, neither a row can express:
  1. **The workqueue.** With the workqueue flag, `kq` is not a descriptor: the call registers on
     the *process's* workqueue kqueue. Forwarded, that is retrace's own process — the class M18
     forbade for `workq_open`/`workq_kernreturn` (code: `crates/retrace-core/src/lib.rs:1214`), where a
     forward created a real worker thread inside the recorder.
  2. **Nested descriptors.** Each `kevent_qos_s` entry's `ident` is usually a file descriptor, and
     it sits *inside* the change list, where `translate_fds` never looks (it rewrites top-level
     registers only). A forwarded plain `kevent_qos` would have the host watch retrace's own
     descriptor of that number — M10's class, one level down.

  **The `[trap]` diagnostic cannot see the flags.** It prints `x0`–`x5` (code:
  `crates/retrace-core/src/lib.rs:157`); `flags` is `x7`. t0 M1 widens it.

### 2c. The `_nocancel` gap is wider than 464

SDK: 32 `SYS_*_nocancel` numbers. Code: `SYS_CONNECT` (98) has a row; its twin `connect_nocancel`
(409) has none. So a structural test will trip on at least 409 besides 464. No corpus binary is
known to reach 409 (inferred from the wall table; t0 M4 enumerates the whole set).

A table-only check is necessary but not sufficient. Several plain numbers are intercepted by an
arm in `record_box`/`ReplaySession::advance` before the generic forward (code: `close`, `dup`,
`dup2`, `fcntl`, `pipe`, `mmap`, …). If an arm matches the plain number and not its twin, the twin
is forwarded with the plain *row* but without the plain *emulation* — the M9 console bug's exact
shape. t0 M4 reads the arms for that too (§3a).

### 2d. The skip lines

Code: `SKIPPED`/`SKIPPING` lines written with `eprintln!` in `apple_walls_e2e.rs` (16, 54),
`cpython_crash_e2e.rs` (67), `cpython_e2e.rs` (49, 65), `jq_e2e.rs` (16), `jq_file_e2e.rs` (19, 33),
`symbolops_e2e.rs` (139, 179, 191), `sysbin_e2e.rs` (105), `fallthrough_e2e.rs` (39). `lldb_e2e.rs`
alone uses `announce` (`crates/retrace/tests/lldb_e2e.rs:29`, a `writeln!` to `std::io::stderr()`),
which libtest does not capture. Code: `util::run_env` spawns with `Command::output()`, whose child
stdin is closed (std docs), so a guest that reads stdin — `ed` — reads EOF in a gate exactly as
under the sweep's `</dev/null`.

### 2e. The debugger debts, as M43 left them

Docs (status log, M43 "What stays owed"): the pre-decode costs **+38 %** CPU on stepping-heavy tests
(`oracle_threadrust` 21.9 s → 30.5 s user) and +2 % on `cpython_crash_e2e`; `read_guest_checked` and
`va_leaf` scan linearly (code: `crates/retrace-box/src/lib.rs:4654`, `:4714`; `backings:
Vec<Backing>` at `:494`, `pt_entry` calls `read_guest_checked` once per level). `arm-rsi` exists
(code: `crates/retrace/src/gdbserver.rs:412`) and nothing disarms it; `retrace.py`'s `rsi` does not
clean up on a failed `ContinueInDirection`. The step path is `Exec::step_thread` behind
`gdbserver.rs:265`. An arm64e guest exists (M7 Task 7, the va47 property fixture, code:
`crates/retrace-guest/build.rs:414`); whether it has a call chain worth a `bt` is owed by t0 M5.

## 3. Design

### 3a. t0 — measurements first

On a **throwaway** build: experimental rows and prints are local and never committed. Results go in
the companion measurements file. A result that contradicts this spec's premise **halts t0** and goes
to the operator (§7, halt 1).

| # | Question | Method | Decides |
|---|---|---|---|
| **M1** | What is automationmodetool's `kevent_qos` call? | Widen `[trap]` to `x0`–`x7`; run `record-dyn /usr/bin/automationmodetool` under `RETRACE_TRACE=1`; decode `kq`, `flags` (flag values cited from xnu's `event_private.h`) and, from guest memory, every change-list entry's `ident` and `filter` (layout cited from the same header). | A row **only if all four hold**: no workqueue or workloop flag; `kq` is a guest slot bound by `kqueue` (362); no change-list `ident` is a descriptor (its filter says which); and `eventlist`'s extent, `nevents × sizeof(struct kevent_qos_s)`, is citable and inside the window, with `data_out`/`data_available` NULL (§11 item 5). Otherwise the gate is re-parked with the measurement and 374 is routed to its own milestone (R4). |
| **M2** | Does the kernel cap `getattrlistbulk`'s write, and what size does `ls` pass? | Read xnu `bsd/vfs/vfs_attrlist.c` for a cap before copyout and cite it; read `x3` at `ls`'s call. | A cited cap inside the 64 KiB window → `Ptr` (the `getattrlist` precedent). No cap → `Dest(Reg(3))`. |
| **M3** | Where does each target land once 464, 345 and 461 exist? | With the throwaway rows, record and replay `ed`, `ls`, `desdp`, `dyld_info`, `flex`, `dddiagnose` (no arguments, stdin closed, as the gate runs them); keep stderr and traces. | Per binary: un-ignore, or re-park at the measured wall with evidence. Confirms or refutes §2b's exec inference. |
| **M4** | Which `_nocancel` twins does the table lack, and which intercepting arms miss a twin? | Pair all 32 SDK `SYS_*_nocancel` names with their plain names; check both against `arg_kinds`. Read every arm in `record_box` and `ReplaySession::advance` that matches a plain number, and check it matches the twin. | A1's exact expected set. Every arm mismatch is a named item: fixed in A2 if a corpus guest can reach it, listed in Known limits if not. |
| **M5** | Debugger baselines and unknowns | (i) User CPU of `oracle_threadrust` and `cpython_crash_e2e` at `c652cf1` (M42 merge, before the pre-decode) and at `64e471e`, same machine, three runs each. (ii) Pre-answered while writing the plan: no arm64e fixture has a call chain (`strip47` and `bfamstrip` are single-function freestanding asm), so B5 builds one; t0 confirms. (iii) Today's lldb baseline on the three stepping shapes B3 and B6 change — a blocked step past another thread's breakpoint, a step naming a thread that is not running, a step across the stepped thread's own exit — as the server's `vCont` packet count per bounded session. (iv) What lldb-2100's `next` does at a `bl` in a function with no line table, against the server. | (i) B1's pass bar. (ii) B5's fixture. (iii) The baseline each new form in B3 and B6 is compared against, in-task (§3c). (iv) B4's `next` expectation. |

### 3b. Track A — breadth

Order is chosen so each task's test is red before its change.

- **A1 — the `_nocancel` structural test** (`crates/retrace-arch`, a test target of its own). Parse
  the SDK header at test time (the SDK is already a build dependency: `hv-sys`'s `build.rs` runs
  bindgen against it). For every `X_nocancel` whose plain name `X` exists, assert
  `arg_kinds(nocancel) == arg_kinds(plain)` whenever either is `Some`. A nocancel name with no plain
  twin fails by name (a parse finding, not a silent skip). Red today on 464 and 409 at least; M4
  fixes the full expected set. **Guard:** it asserts the rule the table's own comment states, which
  five milestones have each broken by hand.
- **A2 — the rows.** `SYS_OPENAT | 464` (one row, `Ret::Fd`); `409` joins `SYS_CONNECT`; the rest of
  M4's set likewise; `345 => [Path, Ptr]`; `461` per M2. Each new row's comment carries its prototype
  and, for every `Ptr`, its cited bound — the table's existing rule. If 461 is `Dest`, the `truncguard`
  window test gains it. Any arm mismatch M4 found that a corpus guest can reach is fixed here, in
  **both** `record_box` and `ReplaySession::advance` (symmetry rule 1). Every new row whose views differ
  from the pre-M33 tables gets an `EXPECTED_DIFFS` entry in
  `crates/retrace-arch/tests/legacy_equivalence.rs`: that sweep covers 0..=1023 in both directions,
  and each entry's `exercised`/`unexercised` word must agree with the census. `tests/census.rs` gains
  345, 461 and 464, reached since M38 moved the walls (t0 M3 confirms each). **Guard:** A1 green;
  the census and `dest_buffer` unit tests extended for each new number.
- **A3 — `F_DUPFD_CLOEXEC`.** In `guest_fcntl_dupfd`, set `FD_CLOEXEC` on the host `dup` when the
  command is `F_DUPFD_CLOEXEC`. **Guard:** `dupfd_e2e` gains a case: `F_DUPFD_CLOEXEC` then `F_GETFD`
  returns **1**; it returns 0 before the change, which is the difference asserted.
- **A4 — the gates.** Add `ls_records_and_replays` and `ed_records_and_replays` to
  `apple_walls_e2e.rs`. For all seven targets, un-ignore or re-park per M1/M3, each reason rewritten
  in the file's existing form (wall in the recorder's own words, landmark, evidence path, what
  un-parks it). A gate whose binary exits non-zero natively asserts on what the binary does, as
  `launchctl_records_and_replays` does, never on `rc == 0`. **Guard:** the gates, and a `--ignored`
  run as the file's positive control.
- **A5 — skip lines.** Move `announce` from `lldb_e2e.rs` into `crates/retrace/tests/util/mod.rs`;
  switch every line in §2d to it. **Guard:** a new target, `skiplines.rs`, with (1) a source test
  that fails if any file under `crates/retrace/tests/` has `eprintln!(` followed — across whitespace
  and newlines, since several calls wrap — by a string literal beginning `SKIP`, and
  (2) a control test that `announce`s a fixed line (`SKIPLINES CONTROL: …`) which the close must find
  in an ordinary gate log (§6).
- **A6 — sweep and docs.** Re-run `tools/apple-sweep.sh` once on the close's binary; evidence under
  `docs/sweep-evidence/<date the sweep runs>-m44/` (the M37–M39 naming), with every non-clean row's
  stderr kept. Reconcile the tally
  row by row against M39's `pass=44 fail=10 skip=0`. Correct the README's 468 text in place (R1).

**Invariants.** No `TRACE_MAGIC` bump: no `Event` shape changes, and no snapshot bytes change
meaning — rows change only what is forwarded and how. M1's widened `[trap]` line is **kept** unless a
test parses the six-argument form (it prints only under `RETRACE_TRACE`, on record only).

### 3c. Track B — debugger debts

Ordered safest first, so what is likeliest to be routed comes last.

- **B1 — backing lookup index** (`retrace-box`). A side index of `(ipa_start, ipa_end, idx)` sorted by
  `ipa_start`, binary-searched, rebuilt on every mutation of `backings` (the plan enumerates the
  mutation and lookup sites). **`backings` itself is not reordered** (R2): its iteration order may
  reach snapshot or checkpoint bytes, and changing that would change what a snapshot means.
  **Guard:** an equivalence test — over randomized IPAs and lengths from `retrace-sim`'s `Rng`,
  indexed lookup == linear lookup, including misses and spans that straddle two backings. **Pass
  bar:** from M5(i), close at least half the gap between `c652cf1` and `64e471e` on
  `oracle_threadrust` user CPU.
- **B2 — `disarm-rsi`.** A monitor command that clears `rsi_armed`, idempotent. `retrace.py`'s `rsi`
  sends it when `ContinueInDirection` fails. **Guard:** a `gdbserver_e2e` row — `arm-rsi`,
  `disarm-rsi`, then `bc` performs a full reverse-continue, asserted by the position reached, not a
  one-instruction step back.
- **B3 — a step stops at its thread's own exit.** In `Exec::step_thread`: when the stepped thread has
  exited after the crossing, stop at that boundary with a non-`trace` stop that names the exit,
  rather than running to the end of the recording. **Guard:** a `gdbserver_e2e` row on `threadrust`:
  stepping the child across its exit stops at the exit's landmark with a reason other than `trace`.
  The stop is reported on the thread now running: the exited thread has no context left to name
  (`stop()` looks every named thread up in the table). M43 measured lldb looping on a step answered
  on another thread, so the new form runs through a bounded lldb session before the commit is kept
  (M5(iii) is the baseline); a loop reverts it and routes B3.
- **B4 — lldb rows for `thread step-out`, `finish`, `ni` and `next`** over a `bl` on `crashy`.
  **Guard:** each row's expected pc is computed from `crashy`'s symbols (the `bl`'s pc + 4), never
  hard-coded. A server bug a row exposes is fixed if small, or routed with its measurement.
- **B5 — arm64e `bt` depth.** Using M5(ii)'s fixture, run lldb's `bt` with and without
  `addressing_bits:47` in `qHostInfo`; ship the form that gives the full depth. **Guard:** an
  `lldb_e2e` row asserting ≥ 3 frames. Neither form works → re-parked with the measurement.
- **B6 — the two unmeasured behaviour changes.** (a) During a blocked step, another thread's hit
  ends the step with a `reason:exception` stop on the stepped thread naming the other thread's hit
  (M43's L7 measured-safe form). (b) A step of a non-running thread runs until that thread is
  scheduled, as a blocked step already does (M43 T5-a's successor). **Each proceeds only if** its new form, run
  through a bounded lldb session before the task's commit is kept, does not drive lldb into a re-step
  loop (M43 measured 80,103 × `vCont;s:2` in 60 s for the unsafe form; M5(iii) is the baseline),
  **and** every existing `gdbserver_e2e`/`lldb_e2e` row is unchanged except the three that pin the
  behaviour being changed — `gdbserver_e2e`'s
  `a_blocked_step_runs_past_another_threads_breakpoint_to_the_stepped_thread` and
  `a_step_on_a_thread_that_is_not_running_is_refused_in_place`, and `lldb_e2e`'s
  `lldb_steps_a_blocked_thread_to_where_it_resumes_and_refuses_one_that_is_not_running` — each
  rewritten to assert the new behaviour. A refusal row for a thread that does not exist stays.
  **Guard:** each new row is time-bounded (the `llsc_e2e` rule), so a loop fails rather than stalls.

The README's "Debugging with lldb: what `retrace gdbserver` does not do" is edited in place to drop
what B2–B6 retire.

## 4. Guards: each asserts the difference it makes

| Change | The assertion | Why a weaker failure cannot pass it |
|---|---|---|
| A1 | every SDK twin pair shares one row | the number and the name come from the same header, so a mis-numbered row (§2a's class) cannot satisfy it |
| A2 | un-ignored gates record to their measured outcome | before the row, the recorder panics with rc 101 — a code no guest produces |
| A3 | `F_GETFD` after `F_DUPFD_CLOEXEC` reads 1 | the pre-change value is 0 on both sides, deterministically |
| A4 | per-binary outcome, not `rc == 0` where the binary exits non-zero natively | the `launchctl` precedent |
| A5 | the control line appears in an ordinary gate log | an `eprintln!` line provably does not (M43's measurement) |
| B1 | indexed == linear on every probe, and the CPU bar | a wrong index would change a read, which the equivalence test sees before any e2e does |
| B2 | `bc` after `disarm-rsi` lands at a reverse-continue's position | an armed `bc` lands one instruction back |
| B3 | the stop is at the exit's landmark and is not `trace` | today the step runs to the end of the recording |
| B4 | pc == the `bl`'s pc + 4, derived from symbols | a no-op or a runaway step lands elsewhere |
| B5 | ≥ 3 frames | M43 measured frame #0 only |
| B6 | a bounded run reaches the named stop | a loop hits the bound and fails |

## 5. Task order and why

t0 → A1 → A2 → A3 → A4 → A5 → B1 → B2 → B3 → B4 → B5 → B6 → A6 → close.

t0 first because M1–M5 each decide a task's shape. A1 before A2 so the rows land against a red
test. A4 after A2 because the gates' outcomes depend on the rows. A5 is independent and cheap. B1
before the other B tasks because it speeds up every stepping test they add. B6 last among the B
tasks because it is the likeliest to be routed. A6 (the sweep) runs on the close's binary, after
every change that could move a row.

## 6. Acceptance

1. The measurements file exists, with M1–M5 answered and cited.
2. A1 green over every SDK `_nocancel` pair; M4's arm mismatches each fixed or listed by name.
3. Rows for 464, 345 and 409 (plus M4's set) landed; 461 landed per M2; 374 routed per M1's rule.
4. `dupfd_e2e`'s `F_DUPFD_CLOEXEC` case green.
5. Each of the seven targets un-ignored, or re-parked at a measured wall with a rewritten reason.
6. Zero `eprintln!` skip lines under `crates/retrace/tests/`; the `SKIPLINES CONTROL` line found by
   `grep -a` in an ordinary (no `--nocapture`) gate log.
7. The sweep re-run, with every moved row named against M39's tally.
8. B1's equivalence test green and its CPU bar met; B2–B4 rows green; B5 and B6 shipped or routed
   with their measurements.
9. The gate: **0 failed**, chunked per CLAUDE.md (every chunk `--no-fail-fast`, exit codes captured
   before any pipe, the `--bins` chunk included, `--doc` beside any library crate split per target),
   totals reconciled file by file against M43's **800 / 0 / 9 over 144**.
10. README edited in place (What works today; Known limits: the rows, 468, the sweep table and tally,
    the lldb limits, the counts); `docs/status-log.md` gains an M44 section with forward pointers to
    the M34/M38 text that misnamed 468 (those sections are not edited); CLAUDE.md's honest-gate
    paragraph ("Only `lldb_e2e` writes its skip line…") rewritten, and its e2e list updated for any
    new target.
11. Merged `--no-ff` into local `main`. **Not pushed** without the operator's say-so.

## 7. Halt rules, and what this milestone deliberately does not do

**Halt and ask** (do not work around):
1. A measurement that contradicts this spec's premise — e.g. 464 already tabled, `ls` never reaching
   461, `automationmodetool` not reaching 374.
2. Anything that would need a `TRACE_MAGIC` bump.
3. A replay divergence, or a newly red result, in any gate green at M43.
4. A forward that would act on retrace's own process — the `kevent_qos` workqueue class or any like
   it. Refused, never forwarded.
5. A red gate chunk that the task causing it cannot explain.

**Route, don't halt:** an item that proves bigger than a row or a small change is re-parked with its
measurement and routed to a later milestone. That is the M38 pattern, and it is not a failure.

**Not done here:** `fork` / process creation (`csh`, `tcsh`); modelling `kevent_qos` beyond a row;
the two long-parked gates (`stackoverflow_rust_e2e`, `symbols_e2e`); `gdbserver_e2e`'s runtime; the
lldb limits in M43 spec §7 that B2–B6 do not touch; M41's and M42's owed lists; rows for 468 or 476.

## 8. Rulings (made while writing this spec)

- **R1 — 468 is dropped, and neither 468 nor 476 gets a row.** 468 was mis-named (§2a) and no corpus
  binary reaches either number. An absent row fails loud at the forward (M33), which is the right
  behaviour for a call nothing has measured. The README is corrected in place; the status log's old
  sections stand, with M44's section pointing at the correction.
- **R2 — B1 indexes beside `backings`, never reorders it.** The order may reach snapshot bytes.
- **R3 — the debugger debts ride in M44 by the operator's choice** (brainstorming, 2026-09-27),
  against the controller's recommendation to split them into their own milestone. The tracks share
  no code, and "route, don't halt" (§7) keeps one track's surprise from holding the other hostage
  beyond the close.
- **R4 — 374's four-condition rule** (M1; the fourth added from the plan, §11 item 5). A row is the whole answer only for a plain guest-kqueue
  call with no descriptor idents. Anything else is a subsystem, routed.
- **R5 — A1 parses the SDK header at test time rather than committing a list.** A committed list
  cannot see a new SDK's twins, and the SDK is already required to build.
- **R6 — `ls` and `ed` get gates even if they park.** A wall with no gate is invisible to the gate
  log; the honest-gate rule wants it named.

## 9. Gate prediction

Before t0, estimated: **+12 to +18 tests**, **+2 or +3 binaries** (A1's target in `retrace-arch`,
`skiplines.rs`, and B1's equivalence test if it is a target of its own); `failed` 0; `ignored`
anywhere from **5 to 11** — lower if `ed`/`dddiagnose` go green and `ls` records, higher if `ls` and
`ed` park at new walls and the xcrun trio re-parks at the exec refusal. The plan replaces this with
per-file expected counts once t0 has run.

## 10. Outcome

*(Filled at the close.)*

## 11. Corrections from the plan (2026-09-27)

Writing the plan's steps against the code found five things this spec had wrong or missing. Each is
corrected in place above; this section says what changed and why, so a reader who met the first
version can tell it was overturned rather than misremembered.

1. **B6's condition was unsatisfiable as written.** "Every existing row is unchanged" cannot hold:
   three existing rows assert exactly the behaviour B6 changes (two in `gdbserver_e2e`, one in
   `lldb_e2e`, whose session A runs past another thread's breakpoint and whose session B is a
   refusal). They are named in §3c now and are rewritten, not preserved.
2. **M5(iii) could not measure B3's or B6's new form in t0**, because the form does not exist
   until the task writes it. t0 now measures today's baseline; each task measures its own form
   against it before its commit is kept. B3 gained the same check: its stop must be named on the
   running thread, which is the shape M43 measured lldb looping on.
3. **A2 missed `legacy_equivalence`.** Its sweep compares every number in 0..=1023 against the
   pre-M33 tables in both directions, so a new `Fd` or `Dest` row with no `EXPECTED_DIFFS` entry
   fails it — and the entry's `exercised` word is checked against the census, which A2 now extends.
4. **B4's `next` had no expectation.** lldb's `next` needs a line table and `crashy` has none, so
   what it does there is lldb's behaviour, not the server's. M5(iv) measures it.
5. **374's rule needed a fourth condition.** A row can describe `eventlist` only as a `Dest`, and
   `DestLen` has no form for `nevents × sizeof(struct kevent_qos_s)` — a count times a size. So even
   a plain guest-kqueue call is a row only if that extent is citable and inside the window, and the
   two `data_*` pointers are NULL. Anything else routes, which was already the expected outcome.
