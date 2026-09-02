# M25-cpython Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development (recommended)
> or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax for tracking. Each task is
> dispatchable on its own to an implementer who has read nothing else.

**Goal:** Rung 7. The real CPython interpreter binary running `-c 'print(1)'` records and replays
byte-identically, twice, exit 0 — or `cpython_e2e` stays parked at the first measured wall that is
not small, with that wall's evidence on the test.

**Architecture:** Two known fixes and then a wall-chain. Fix 1 is one bit in one constant inside
`Box_` (**symmetry rule 2** — below the trace, shared by record and replay, nothing recorded). Fix 2
is two entries in a pure lookup table in `retrace-arch` (**symmetry rule 1 is not engaged** — no
dispatch arm is added, so no mirror is owed and `verify_thread`'s site count does not move). Neither
touches `record_box`, `ReplaySession::advance`, or the `Event` enum.

**Spec:** `docs/superpowers/specs/2026-09-02-retrace-m25-cpython-design.md`
**Measurements:** `docs/superpowers/specs/2026-09-02-retrace-m25-cpython-measurements.md`
(cited as Finding 0–3)

**Branch:** `m25-cpython`, cut from `main` at `67384d8` (post-M23).

## Global Constraints

- **Never run cargo with `run_in_background`.** Every cargo invocation is foreground and bounded:
  ```sh
  perl -e 'alarm 600; exec @ARGV' cargo test -p retrace-box -- --test-threads=1
  ```
  macOS has no `timeout(1)`; `perl -e 'alarm N; exec @ARGV'` is this repo's substitute. Anything
  that would exceed the ten-minute tool ceiling is measured by the controller, not the implementer —
  say so and stop rather than backgrounding it.
- **`--test-threads=1` is mandatory.** HVF allows one VM per process; a bare `cargo test` flakes
  with `HV_BUSY`.
- **The gate is chunked** — the full `--workspace` run exceeds the ceiling and gets killed. Do not
  read a kill as a red. Run every chunk `--no-fail-fast` and capture cargo's exit code **before any
  pipe**; a pipe swallows it, and **this shell is zsh, where `${PIPESTATUS[0]}` is empty** — the
  array is `$pipestatus` and it is 1-indexed, so use `$pipestatus[1]` or redirect to a file and read
  `$?`.
  ```sh
  cargo test --workspace --exclude retrace-box --exclude retrace -- --test-threads=1
  cargo test -p retrace-box -- --test-threads=1
  cargo test -p retrace --test <name> -- --test-threads=1   # per-target, for each e2e gate
  cargo test -p retrace --bins -- --test-threads=1          # NEVER omit
  ```
  **Never omit the `--bins` chunk.** `--test <name>` selects integration targets only, so the unit
  tests inside the `retrace` binary (`crates/retrace/src/debug.rs`) run in **no** other chunk and
  vanish silently. `cargo test -p retrace --lib` is **invalid** for this crate (no lib target) and
  fails loudly; the trap is that the wrong flag is loud and the missing one is silent.
- **Grep gate logs with `grep -a`** — they carry ANSI and UTF-8 that trips plain grep.
- **Reconcile the total against `main`'s actual close at the time the gate runs**, by diffing
  `#[test]` counts file-by-file rather than trusting a sum. **Do not hard-code a baseline number.**
  M23 closed at 497/0/2 over 109 and M24-restoreaudit is closing concurrently with a different
  number; this branch predates it. Read the baseline off `main` when you get there.
- **`TRACE_MAGIC` does NOT move.** M25 records nothing new and adds no `Event` variant or field. If
  you believe you need a format change, stop — that is a spec deviation.
- **clippy must be clean at `-D warnings`** over `--workspace --all-targets`.
- **Every new dispatch arm, if the wall-chain produces one, obeys symmetry rule 1**: record arm in
  `record_box` and replay mirror in `ReplaySession::advance`, both in
  `crates/retrace-core/src/lib.rs`, calling the **same** `Box_` method with the **same** arguments,
  both placed **before** the generic forward arm — and a `verify_thread` call on any arm that
  consumes a landmark and `return`s, because every new mirror silently creates an oracle hole until
  its check is added.
- **Codesigning.** Any test that spawns `CARGO_BIN_EXE_retrace` bypasses cargo's signing runner and
  must go through `crates/retrace/tests/util/mod.rs::bin()`. Using the `util` helpers gets this for
  free; do not spawn the CLI by hand.
- **The two guest paths, verbatim.** Use these constants and no others:
  ```
  real:     /opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python
  launcher: /opt/homebrew/Frameworks/Python.framework/Versions/3.14/bin/python3.14
  ```
  These are the version-stable framework paths (both verified present at plan time, 33,568 and
  34,640 bytes). The measurements used the `Cellar/python@3.14/3.14.6/…` forms, which a `brew
  upgrade` moves; these survive it. Neither is a repo artifact, so **both gates skip with a loud
  `eprintln!` naming the missing path** — a silent skip reads as a green it did not earn.

---

### Task 1: Both gates, parked from the first commit

**No production code.** This task creates the milestone's honest scaffolding before either fix, so
the discipline holds from day one.

**Files:** create `crates/retrace/tests/cpython_e2e.rs`

**Do**

- [ ] **Step 1: merge `main`.** If M24-restoreaudit has landed on `main`, `git merge main` into
  `m25-cpython` **before running anything**. This branch was cut at `67384d8` and predates it.
  Resolve conflicts, then confirm all four SCTLR install sites still route through `sctlr_mmu_on`:
  ```sh
  grep -n "sctlr_mmu_on(" crates/retrace-box/src/lib.rs
  ```
  Expect seven lines: the definition, **four `set_sys` call sites** (1139, 1735, 2604, 4474 before
  the merge), and two mentions inside a doc comment and an assert message near 4576–4584. The four
  call sites are the invariant. If a fifth call site appeared or one stopped going through the
  derivation, **stop and report**:
  Fix 1's whole symmetry argument rests on that invariant.

- [ ] **Step 2: `cpython_e2e.rs`, parked.** One test,
  `the_real_cpython_interpreter_records_and_replays`, `#[ignore]`d with wall 1's exact evidence:
  ```
  #[ignore = "M25 wall 1 (unfixed as of this commit): the real interpreter dies on one instruction. \
              RECORD ERROR: non-syscall exit: MSR/MRS/sysreg trap (EC=0x18 ISS=0x12dc68 FSC=0x28) \
              far/ipa=0x0 (UNMAPPED) pc=0x4404 elr=0x1804fb070 — ISS 0x12dc68 decodes to \
              SYS #3, C7, C4, #1, Xt = DC ZVA, in _platform_memset zeroing 0x7f80 bytes for \
              CPython's allocator. An EL0 DC ZVA traps to EL1 with EC 0x18 when SCTLR_EL1.DZE == 0, \
              and run()'s only Ec::SysReg arm is try_emulate_timebase. UN-IGNORE when Task 2 sets \
              bit 14 of SCTLR_MMU_ON_BASE and the run reaches its own exit(0)."]
  ```
  The body: skip loudly if the real-interpreter path is absent, then
  `util::assert_rung_records_and_replays(REAL, &["-c", "print(1)"], b"1\n")`. That helper demands a
  clean `exit(0)` with exactly that stdout and replays **twice**, so a guest that died inside dyld
  cannot pass, and neither can one that reached "core initialized" and then failed on `encodings`.

- [ ] **Step 3: the launcher gate, RUNNING.** In the same file,
  `the_launcher_records_and_replays_its_own_posix_spawn_failure`, **not** ignored. It cannot use the
  rung helper, which requires exit 0. Use `util::record_dynamic_args(LAUNCHER, &["-c", "print(1)"])`
  and assert, in this order:
  1. `rec.code == 1` — the guest's own exit. Exit 4 is `RECORD ERROR`, 139 is a crash, so this
     discriminates against both.
  2. `rec.stdout` contains the bytes `posix_spawn` — **the load-bearing assertion.** Exit 1 alone
     is a code a weaker failure would also produce; only the guest's own error text proves it ran
     `pythonw.c`'s `err(1, …)` path. The text is on **stdout, not stderr**: retrace mirrors guest
     writes to fd 1 AND fd 2 into one buffer and prints it on its own stdout (the `is_console_write`
     arm of `record_box` in `crates/retrace-core/src/lib.rs`; the predicate at
     `crates/retrace-arch/src/lib.rs:22` covers both fds). Retrace's own `[retrace]` diagnostics go
     to its stderr, so `rec.stderr` never carries guest text. Say this in a comment. Assert
     `contains` on the bytes, never equality against the whole buffer.
  3. `rep.code == rec.code` and `rep.stdout == rec.stdout` from `util::replay(&trace)`. Because the
     mirrored buffer carries the guest's fd-2 text, this byte-equality IS the proof that replay
     reproduced the guest's output rather than merely exiting the same way.

- [ ] **Step 4: the file header says why the launcher gate exists.** Write, in as many words, that
  it pins a **known gap** — exec-in-place is unmodelled, the forwarded `posix_spawn` returns an error
  instead of replacing the image, and the run replaying byte-identically is retrace working
  correctly, not a bug. State that a successor milestone implementing exec-in-place must **rewrite**
  this test, not preserve it. (Spec risk R6: a test that pins a limitation will otherwise be
  defended by someone who reads it as a requirement.)

**Do not**

- Do not run any fix yet. This task must be committed with `cpython_e2e` still parked.
- Do not assert on `rec.stderr` for guest text — it carries only retrace's own diagnostics, and the
  assertion would fail for a reason unrelated to the guest. Do not assert on the exit code alone.
- Do not use `assert_rung_records_and_replays` for the launcher; it requires exit 0 and will fail.
- Do not hard-code the `Cellar/python@3.14/3.14.6/…` paths.

**Acceptance**

`perl -e 'alarm 600; exec @ARGV' cargo test -p retrace --test cpython_e2e -- --test-threads=1`
reports **1 passed / 1 ignored**. The launcher test passes on this machine; if Python is absent it
prints its loud skip and the implementer says so explicitly rather than reporting a green.

---

### Task 2: `SCTLR_EL1.DZE`

**TDD. Write the test before the fix.**

**Files:** modify `crates/retrace-box/src/lib.rs`

**Do**

- [ ] **Step 1: RED.** `SCTLR_MMU_ON_BASE` and `sctlr_mmu_on` are **private**, so the test must be a
  `#[cfg(test)] mod` inside `lib.rs` — the same placement as the existing `pac_posture_tests` module
  (around `lib.rs:4649`). Add `sctlr_enables_dc_zva_for_el0_and_nothing_else`, asserting on
  `sctlr_mmu_on(false)` and `sctlr_mmu_on(true)`:
  - `& 0x4000 != 0` — **DZE (bit 14) is SET.** Fails today.
  - `& 0x8000 == 0` — **UCT (bit 15) stays CLEAR.**
  - `& 0x0400_0000 == 0` — **UCI (bit 26) stays CLEAR.**

  The last two are not padding. They pin the spec's deliberate omission as a decision: nothing has
  measured a guest issuing `DC CVAU` / `IC IVAU` or reading `CTR_EL0`, the existing EC 0x18
  non-syscall exit is already the fail-loud path for that case, and setting them speculatively is
  the "right conclusion resting on an unmeasured supporting fact" this repo keeps catching. Say that
  in a comment above the assertions.

  Observe and record the failure. Expected: the DZE assertion fails; the two clear-bit assertions
  already pass.

- [ ] **Step 2: GREEN.** One edit at `crates/retrace-box/src/lib.rs:174`:
  ```rust
  const SCTLR_MMU_ON_BASE: u64 = 0x30d0_0800 | 1 | 4 | 0x1000 | 0x4000;
  ```
  Add a comment naming the bit (`DZE(14)`, matching the `SCTLR_PAC_EN` comment style two lines
  above), why it is set (EL0 `DC ZVA` traps to EL1 with EC 0x18 when it is clear; Apple's
  `_platform_memset` uses `DC ZVA` above a size threshold and CPython's allocator hits it at
  startup), and that `UCI`/`UCT` are deliberately left clear pending measurement.

- [ ] **Step 3: verify the record advances.** Bounded, foreground:
  ```sh
  perl -e 'alarm 400; exec @ARGV' cargo run -q -p retrace -- record-dyn \
    /opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python \
    -o /tmp/m25-pyreal.bin -- -c 'print(1)'
  ```
  Expect the EC 0x18 `RECORD ERROR` to be **gone** and the run to reach CPython's own output. Per
  Finding 2 the expected next stop is the `encodings` import failure with
  `OSError: [Errno 22] Invalid argument`, exit 1. That is wall 2, and it is Task 3's subject.
  **If something else appears, record it verbatim and report it — do not fix it in this task.**

- [ ] **Step 4: mutation check.** Revert the `| 0x4000` → the unit test must go RED and the record
  must die on the EC 0x18 line again. Restore. A fix never observed failing is not yet verified.

- [ ] **Step 5: the box chunk still passes.**
  `perl -e 'alarm 600; exec @ARGV' cargo test -p retrace-box -- --test-threads=1`. This is spec risk
  R3's check: DZE changes SCTLR for **every** guest, and the box crate holds the VM tests that would
  notice.

**Do not**

- Do not set `UCI` (bit 26) or `UCT` (bit 15). If a later wall in this milestone turns out to need
  one, that is a new measured finding and gets its own task and its own doc edit.
- Do not add an `Ec::SysReg` arm to emulate `DC ZVA`. The instruction should execute natively; the
  bit exists so it does not trap at all.
- Do not touch any of the four `vcpu.set_sys(sysreg::SCTLR_EL1, …)` call sites. The whole point is
  that they all route through the one derivation.
- Do not leave `cpython_e2e` un-ignored yet. It cannot pass until Task 3 at the earliest.

**Acceptance**

The unit test passes and was observed failing under the mutation; `cargo test -p retrace-box`
is green; the recorded run reaches CPython's own stdout/stderr instead of the EC 0x18 exit, and the
new stopping point is written down verbatim.

---

### Task 3: `fd_operands` gains `getdirentries64` and `fstatfs64`

**TDD. Write the test before the fix.**

**Files:** modify `crates/retrace-arch/src/lib.rs`

**Do**

- [ ] **Step 1: classify the three census leftovers, and write the answer down.** Finding 3 lists
  "possibly 228, 406, 427 (unidentified)". Resolve each against the SDK on this machine:
  ```sh
  H=$(xcrun --show-sdk-path)/usr/include/sys/syscall.h
  for n in 228 344 346 406 427; do grep -E "^#define[[:space:]]+SYS_[a-z_0-9]+[[:space:]]+${n}\$" "$H"; done
  ```
  The expected answers, all three of which mean **no change is needed** for them:
  - **228 = `fgetattrlist`** — already in the table as `SYS_FGETATTRLIST` → `&[0]`
    (`crates/retrace-arch/src/lib.rs:62` and `:102`).
  - **406 = `fcntl_nocancel`** — already in the table as `SYS_FCNTL_NOCANCEL` → `&[0]` (`:67`, `:100`).
  - **427 = `fsgetpath`** — `ssize_t fsgetpath(char *, size_t, fsid_t *, uint64_t)`
    (SDK `sys/fsgetpath.h:45`). **Takes no descriptor**, so its absence from the table is correct.

  If any answer differs from the above on this machine, **stop and report** — the spec's census
  correction is wrong and the scope changes. Otherwise carry these three into the commit message and
  into Task 5's status-log section, because "we checked and it was already right" is a result.

- [ ] **Step 2: RED.** Extend the existing tests near `crates/retrace-arch/src/lib.rs:510-560`:
  - Add `SYS_GETDIRENTRIES64` and `SYS_FSTATFS64` to the `for num in […]` list in
    `fd_operands_covers_the_measured_surface`, which asserts `&[0]`. Fails today: `&[]`.
  - Add `assert_eq!((SYS_GETDIRENTRIES64, SYS_FSTATFS64), (344, 346));` to `m25_syscall_numbers` — a
    **new** test beside the existing `m10_syscall_numbers`, following its pattern of pinning the
    numbers themselves so a mistyped constant fails here rather than as a wrong forward.
  - Add `assert_eq!(fd_operands(427), &[] as &[usize], …)` with a comment naming it `fsgetpath` and
    stating that it takes an `fsid_t *`, not a descriptor. This pins Step 1's classification so a
    later reader does not re-open the question.

- [ ] **Step 3: GREEN.** Two constants in the M10 block (`crates/retrace-arch/src/lib.rs:55-73`),
  beside their siblings and following its comment convention:
  ```rust
  pub const SYS_GETDIRENTRIES64: u64 = 344;
  pub const SYS_FSTATFS64: u64 = 346;
  ```
  and both added to the `=> &[0]` arm of `fd_operands` (`:97`). Document at the constants that
  `fstatfs64(int, struct statfs64 *)` is declared in the SDK's `sys/mount.h:444`, while
  `getdirentries64` is **not in the SDK at all** — libc calls it privately from `opendir`/`readdir`
  — so its `x0` operand position rests on Finding 3's captured trap arguments
  `[fd=0x4, buf, 0x2000, &basep]`. Label the second as measured-from-a-trap, not header-derived;
  this repo distinguishes attributed constants from measured ones at their use site.

- [ ] **Step 4: verify the guest advances.** Re-run Task 2 Step 3's record command. The
  `OSError: [Errno 22] Invalid argument` on the `python3.14` lib directory must be **gone**. Capture
  the new stopping point verbatim, whatever it is — that is Task 4's input.

- [ ] **Step 5: mutation check.** Remove `SYS_GETDIRENTRIES64` from the `fd_operands` arm only (leave
  the constant) → `fd_operands_covers_the_measured_surface` must go RED and the record must return to
  `[Errno 22]`. Restore.

**Do not**

- **Do not change `fd_operands`' default arm.** `_ => &[]` stays. Making an unclassified fd-taking
  syscall fail loud is the successor milestone described in the spec's Residual; its blast radius
  crosses every gate in the tree and must be measured before it lands. Adding it here would turn an
  unmeasured hazard into an unmeasured outage.
- Do not add 228, 406 or 427 to anything. Two of them are already there and the third takes no fd.
- Do not touch `allocates_fd`. Neither 344 nor 346 returns a new descriptor.
- Do not add a dispatch arm anywhere. `forward_and_diff` (`crates/retrace-box/src/lib.rs:2791`) takes
  `args` by value and keeps the guest's own view in `gargs`, so the trace still records the guest's
  descriptor. Nothing about record/replay dispatch changes.

**Acceptance**

`perl -e 'alarm 300; exec @ARGV' cargo test -p retrace-arch -- --test-threads=1` green, with the new
assertions observed failing before the fix; the recorded CPython run passes the `getdirentries64`
wall; the new stopping point is written down verbatim.

---

### Task 4: The wall-chain *(repeatable — run it again for each wall, or stop)*

This task is executed **once per wall**. Each pass is one commit. Read the stop criteria before
starting a pass, and apply them honestly: the milestone is allowed to close parked, and doing so is
the discipline working.

**Do**

- [ ] **Step 1: measure.** Bounded, foreground, and capture the whole log:
  ```sh
  perl -e 'alarm 500; exec @ARGV' env RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn \
    /opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python \
    -o /tmp/m25-py.bin -- -c 'print(1)' > /tmp/m25-py.log 2>&1
  ```
  `RETRACE_TRACE=1` is **record-only** — `ReplaySession` carries no trace instrumentation, so never
  expect a `[trap]` line from a replay. Grep the log with `grep -a`.

- [ ] **Step 2: name the wall.** From the last traps before the failure, state (a) the exact error
  or guest-visible symptom, (b) the syscall or instruction behind it, (c) which of the three
  permitted shapes it has:
  - **below the trace** (a `Box_::run()` arm or a constant, like Task 2) — symmetry rule 2, nothing
    recorded;
  - **table-shaped** (a pure lookup in `retrace-arch`, like Task 3) — no dispatch arm;
  - **a new dispatch arm** — record arm *and* replay mirror in `crates/retrace-core/src/lib.rs`,
    same `Box_` method, same arguments, both before the generic forward arm, plus `verify_thread` if
    the arm consumes a landmark and `return`s.

- [ ] **Step 3: STOP CRITERIA — check all four before writing any fix.** Proceed only if **every**
  one holds:
  1. The wall is **measured**, not inferred from reading code.
  2. Its fix is **small**: one constant, one table entry, or one arm pair. Not a trace-format change
     — `TRACE_MAGIC` does not move in this milestone.
  3. It is **mutation-verifiable**: you can state, before writing it, the revert that puts the
     failure back.
  4. It does not require modelling unmeasured kernel behaviour (an unmodelled `dup2`, a wake
     correction nothing has measured, an unenumerated opcode). A guest that reaches one of retrace's
     existing deliberate fail-loud paths is a **park**, not a fix — those paths are working as
     designed.

  **If any criterion fails: park and stop.** Update `cpython_e2e`'s `#[ignore]` reason to name
  *this* wall with its verbatim evidence — never a generic "CPython unsupported" — and go to Task 5.

- [ ] **Step 4: fix it, TDD.** RED first with a unit test at the right layer, then the fix, then the
  mutation check, then the layer's own chunk green. Same rhythm as Tasks 2 and 3.

- [ ] **Step 5: move the gate forward and commit.** Rewrite `cpython_e2e`'s `#[ignore]` reason to
  name the *new* stopping point. If the run now reaches a clean `exit(0)` with stdout `1\n`, **delete
  the `#[ignore]` attribute** and say so loudly — that is rung 7.

- [ ] **Step 6: loop or leave.** Return to Step 1 for the next wall, or go to Task 5.

**Do not**

- Do not guess at a wall you have not measured, and do not fix two walls in one commit — a milestone
  whose result is unreadable is worth less than a shorter one that is.
- Do not weaken an existing assertion or fail-loud path to get past a wall. Before editing any
  existing assertion, read what it was protecting (M19 lost four assertions to an appended suffix and
  the obvious repair would have destroyed a fifth).
- Do not let the chain run indefinitely. If three consecutive passes clear walls that are each one
  line, that is fine; if a pass takes more than one fix, the stop criteria have already been broken.

**Acceptance (per pass)**

One wall cleared with its RED observed and its mutation verified, or the gate parked with verbatim
evidence in the `#[ignore]` reason. The layer's own test chunk is green either way.

---

### Task 5: The gate and the two documents

**Do**

- [ ] **Step 1: merge `main` again if it moved,** then run the full chunked gate per Global
  Constraints — every chunk, `--no-fail-fast`, **including `--bins`**, exit codes captured before
  any pipe (`$pipestatus[1]` in zsh).

- [ ] **Step 2: reconcile file-by-file.** Read the baseline off `main` **as it stands when the gate
  runs** — do not carry a number from this document or from a memory of M23's 497/0/2, because
  M24-restoreaudit is closing concurrently and this branch was cut before it. Diff `#[test]` counts
  per file. The expected M25 delta, to be confirmed rather than assumed:
  - `crates/retrace-box/src/lib.rs` — **+1** (the SCTLR unit test), in the `-p retrace-box` chunk.
  - `crates/retrace-arch/src/lib.rs` — **+1** (`m25_syscall_numbers`), in the workspace chunk;
    the additions to `fd_operands_covers_the_measured_surface` change no count.
  - `crates/retrace/tests/cpython_e2e.rs` — **+2** (one running, one ignored **or** both running if
    the chain reached rung 7), and **+1 test binary**.
  - `--bins` — **unchanged.** M25 touches nothing in `crates/retrace/src/`, and `--bins` holding
    still is the load-bearing part of the reconciliation.

- [ ] **Step 3: clippy.** `cargo clippy --workspace --all-targets -- -D warnings`, clean.

- [ ] **Step 4: the two documents, which must not be merged.**
  - **README, edited in place.** Add a rung 7 row to the guest-ladder table (guest: the real CPython
    interpreter; note: `-c 'print(1)'`) — or say plainly under Known limits what it parks at, if it
    parked. Under "What works today", state the two facts M25 established: `SCTLR_EL1.DZE` is set so
    EL0 `DC ZVA` executes natively rather than trapping, and `fd_operands` covers
    `getdirentries64`/`fstatfs64`. Under "Known limits", add **exec-in-place is unmodelled** — a
    launcher that `posix_spawn`s with `POSIX_SPAWN_SETEXEC` records and replays faithfully but gets
    an error instead of a new image, so point retrace at the real binary — plus any wall the gate is
    parked at, plus the silent `fd_operands` default from the spec's Residual. Update the gate line's
    counts and the ignored-gate list.
  - **`docs/status-log.md`** — **append** a `## Status: M25-cpython` section. Never rewrite an
    earlier one. Say what was measured, what the two fixes were, what the chain reached, and what is
    left standing: the silent `fd_operands` default, exec-in-place, `UCI`/`UCT` unmeasured, and
    reverse execution/seeks over a CPython trace ungated.
  - **CLAUDE.md** — only if a statement in it became false. Expected: **none**.

- [ ] **Step 5: state the outcome without hedging.** If rung 7 is green, say so and say the gate is
  un-`#[ignore]`d. If it is parked, name the wall, and state that the milestone parked a new gate for
  a capability it does not yet have — which by this repo's discipline has regressed nothing.

**Do not**

- Do not restate the README in the status log or the log in the README. Two documents, two jobs.
- Do not report a total from `cat *.log` — read only chunks recorded complete in the exit-code file.
  M20's log records a tally of 337 instead of 118 because a glob swept in another chunk's
  half-written log.
- Do not report a kill as a red. Split the chunk and re-run it.

**Acceptance**

Every chunk `EXIT=0`, clippy clean, the total reconciled file-by-file against `main`'s actual close,
both documents updated, `git status` clean apart from intended files.

---

## Sequencing

Task 1 → 2 → 3 → 4 (repeat) → 5. Task 3 does not depend on Task 2's fix and could land first, but
Task 3's Step 4 verification cannot run until Task 2 is in, because the guest dies before it reaches
`getdirentries64`. Task 1 must be first and must be committed parked.

## Coordination

M24-restoreaudit is closing concurrently on `main` and touches `Box_::restore`, which is one of the
four SCTLR install sites Fix 1 depends on. Task 1 Step 1 merges `main` and re-checks the invariant;
Task 5 Step 1 merges again if it moved. Whichever milestone edits the README's "What works today" /
"Known limits" second reconciles.

Another agent may hold the machine for VM tests. `--test-threads=1` protects only within one process;
two sessions running VM tests at once will flake each other. Wait for the machine rather than racing
it — M22's close waited half an hour for exactly this and both milestones got a result they could
trust.

---

## Self-Review

1. **`TRACE_MAGIC` is still `RT\x00\x08`** and no `Event` variant or field changed.
2. **`verify_thread` still has exactly seven call sites** plus `mirror_delivery`'s inline eighth —
   unless the wall-chain added an early-returning mirror, in which case it has one more and that arm
   has its check.
3. **All four SCTLR install sites still route through `sctlr_mmu_on`.** `grep -n "sctlr_mmu_on("`
   returns the definition plus four call sites, one of which is in `restore`. If `restore` stopped
   going through it, Fix 1 became record-only — the M24 class — and the milestone is wrong.
4. **`UCI` (bit 26) and `UCT` (bit 15) are still clear**, and a test asserts it. If either got set,
   a measurement must exist saying why.
5. **`fd_operands`' default arm is still `_ => &[]`.** The fail-loud default is the successor
   milestone, and the spec's Residual describes it. If it changed here, the blast radius was never
   measured.
6. **The launcher gate still runs and still asserts the guest's own `posix_spawn` stderr**, not just
   exit 1. Its header still says it pins a limitation and must be rewritten, not defended, when
   exec-in-place lands.
7. **`cpython_e2e`'s state matches reality.** Either it is un-`#[ignore]`d and green, or its ignore
   reason names a wall that was actually measured, verbatim, with no generic wording.
8. **No existing assertion was loosened to make something pass.** If one changed, its subject
   changed, and the replacement pins the new rule.
9. **Both gates announce their skip.** A machine without Homebrew Python prints two loud lines and
   the close says so, rather than counting two greens it did not earn.
10. **The `--bins` chunk ran and its count is unchanged.** If it was omitted, the gate is short by
    every unit test in `crates/retrace/src/debug.rs` and nothing warned you.
