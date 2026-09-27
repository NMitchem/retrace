# M44-owed Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pay M44's owed list. Three things are in it. First, the missing `arg_kinds` rows, and the `_nocancel` twin rule they break, both enforced structurally. Second, the skip lines libtest swallows. Third, M43's debugger debts: a faster backing lookup, `disarm-rsi`, a step that stops at its own thread's exit, lldb stepping rows, arm64e `bt` depth, and two stepping behaviour changes.

**Architecture:** A measurement task (t0) runs first and decides the shapes of the tasks that depend on it. Then comes a breadth track (A: `retrace-arch` rows and tests, one fd-table line, gates, test-harness plumbing), then a debugger track (B: `retrace-box`'s backing index, `crates/retrace/src/{debug,gdbserver}.rs`, lldb rows), then the sweep, the docs and one chunked gate. Nothing changes the trace format.

**Tech Stack:** Rust 1.95.0 (`aarch64-apple-darwin`), Hypervisor.framework, cargo tests, lldb-2100 (`/usr/bin/lldb`) over gdb-remote, clang for guest fixtures, POSIX `sh` for the sweep.

**Spec:** `docs/superpowers/specs/2026-09-27-retrace-m44-owed-design.md` (committed `be5dc2b`; corrected from this plan, see its §11). Its §2 facts and §8 rulings are cited as `M44 §2a`, `R1`, and so on.

## Global Constraints

- Toolchain `1.95.0`, target `aarch64-apple-darwin`. The gate is `cargo test` in chunks (every one `--no-fail-fast`, `--test-threads=1`) plus `cargo clippy --workspace --all-targets -- -D warnings`.
- **`clippy -D warnings` rejects dead code.** An unused function, a never-constructed variant, or a variant field never read all fail it. Each task adds only what its own non-test code uses. The `Halt` variants go in as follows: `ThreadExited` in Task 8, `StepInterrupted` in Task 11. No other task adds one.
- `clippy.toml` bans `Instant::now`, `SystemTime::now` and `std::thread::Thread`. Tests may `std::thread::sleep` in a polling loop, as `util::debug_bounded` does.
- `--test-threads=1` on every `cargo test`, because only one VM is allowed per process.
- **`TRACE_MAGIC` does not move, and `crates/retrace-trace` has no diff.** No `Event` shape changes, and no snapshot bytes change meaning (M44 §3b invariants).
- Every `retrace debug --script` transcript stays byte-identical. `step_thread` is gdb-remote-only.
- No existing test changes an assertion, except these three, which Tasks 11 and 12 rewrite (M44 §3c B6):
  - `gdbserver_e2e::a_blocked_step_runs_past_another_threads_breakpoint_to_the_stepped_thread`
  - `gdbserver_e2e::a_step_on_a_thread_that_is_not_running_is_refused_in_place`
  - `lldb_e2e::lldb_steps_a_blocked_thread_to_where_it_resumes_and_refuses_one_that_is_not_running`
- Every test that spawns the CLI uses `util::bin()`, the codesigned copy.
- The lldb tests run `lldb -x -b -s <file> </dev/null`, never with `-o`, and end with `script print("END")`, which they assert. They skip through `util::announce`, never `eprintln!` (from Task 5 on).
- **Every new `arg_kinds` row's comment is its C prototype**, and every `Ptr` names its bound with a citation (the table's own rule, `crates/retrace-arch/src/lib.rs:282-286`).
- **Symmetry rule 1:** any change to an arm in `record_box` gets its mirror in `ReplaySession::advance`, calling the same `Box_` method with the same arguments (CLAUDE.md).
- **Worktree shell rules:**
  - no `VAR=val cmd` prefix; put `export VAR=val` on its own line first;
  - no `git -C`;
  - put `echo "exit=$?"` in the **same** command as the cargo invocation it checks, **before** any pipe;
  - `--no-fail-fast` goes before `--`;
  - never `git stash`, which is shared across worktrees;
  - `/usr/bin/time` must run from a script in the scratchpad, not inline.
- **Controls (deliberate breakages):** run only on the **committed** tree, restore with `git checkout -- <file>`, and record each control's actual symptom in the task report. A control that stays green is a finding: report it, never paper over it.
- **Logs:** each command writes to `$L/t<N>-<what>.log`, where `L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed`. Shell state does not persist between tool calls, so every command that uses `$L` starts with its own `export L=…` line.
- **Grep gate logs with `grep -a`**, since they carry ANSI and UTF-8. Before any `awk`, sanitize with `LC_ALL=C tr -cd '\11\12\15\40-\176' < log | sed 's/\x1b\[[0-9;]*m//g'`, because `awk` dies on the multibyte bytes.
- **Style:** match the surrounding code's comment density and idiom. Comments cite the spec as `M44 §3x` and t0 as `(t0 M2)`. Test names are sentences.
- **Never push.** The merge goes into local `main` only; the push waits for the operator (spec §6 item 11).
- **Halt and ask** on any of the spec's five halt conditions (§7). **Route, don't halt** when an item proves bigger than a row or a small change: re-park it with its measurement and name the successor in the task report.

## Review Focus

These are the five inputs or failure modes the spec implies but no task's tests would otherwise exercise, most likely first. Each is pinned by a test in the task that owns the code.

1. **A zero-length read at exactly a backing's end.** The linear scan in `read_guest_checked(ipa, 0)` accepted `ipa == backing end` (`ipa + 0 <= end`) and returned `Some(vec![])`, and the index must answer the same. Pinned in Task 6: `a_zero_length_read_at_a_backings_end_is_held_as_the_scan_held_it`.
2. **A `_nocancel` twin whose plain form is intercepted by an emulation arm.** Examples are `close`, the console write, and `fcntl`'s `F_DUPFD`. The twin must take the same arm on record **and** replay. Otherwise it is forwarded with the plain *row* but without the plain *emulation*, which is exactly the shape of M9's console bug. Task 1 pins every mismatch t0 M4 finds with a test, or lists it in Known limits if no corpus guest reaches it.
3. **A `getattrlistbulk` buffer larger than the 64 KiB window**, as with `ls` on a big directory. If the row said `Ptr` with no cited cap, the kernel's excess write would land in guest memory and in no `Event`, which is M26's silent class. Task 2 pins whichever shape M2 decides through `diff_window_for_test` with a 150,000-byte size.
4. **`disarm-rsi` on an unarmed server, and `arm-rsi` twice.** Either must answer `OK`. An `E01` would make `rsi`'s own cleanup fail inside lldb. Pinned in Task 7.
5. **A step naming a thread that does not exist, or has exited.** After B6(b) it must still be refused in place, never run to the end of the recording. Pinned in Task 12: `a_step_on_a_thread_that_does_not_exist_is_refused_in_place`.

---

## File Structure

| File | Change | Task |
|---|---|---|
| `crates/retrace-core/src/lib.rs:157` | `[trap]` prints `x0`–`x7` | 0 |
| `docs/superpowers/specs/2026-09-27-retrace-m44-owed-measurements.md` | create: t0's M1–M5 | 0 |
| `docs/sweep-evidence/<t0 date>-m44-t0/` | create: t0 evidence (stderr, traces' `[trap]` excerpts, README) | 0 |
| `crates/retrace-arch/tests/nocancel.rs` | create: the SDK-driven twin test | 1 |
| `crates/retrace-arch/src/lib.rs` | constants, rows, unit tests | 1, 2 |
| `crates/retrace-arch/tests/census.rs` | + 464 (Task 1), + 345, 461 (Task 2) | 1, 2 |
| `crates/retrace-arch/tests/legacy_equivalence.rs` | `EXPECTED_DIFFS` for each new differing row | 1, 2 |
| `crates/retrace-box/tests/truncguard.rs` | 461's window | 2 |
| `crates/retrace-box/src/lib.rs` (`guest_fcntl_dupfd`, ~4279) | `FD_CLOEXEC` on the host dup | 3 |
| `crates/retrace-guest/c/dupfd_dyn.c`, `crates/retrace/tests/dupfd_e2e.rs` | the `F_DUPFD_CLOEXEC` case | 3 |
| `crates/retrace/tests/apple_walls_e2e.rs` | `ls`/`ed` gates; seven gates re-parked or un-ignored | 4 |
| `crates/retrace/tests/util/mod.rs` | `pub fn announce` | 5 |
| 8 skip files + `crates/retrace/tests/lldb_e2e.rs` | skip lines via `util::announce` | 5 |
| `crates/retrace/tests/skiplines.rs` | create: detector + control | 5 |
| `crates/retrace-box/src/backings.rs` | create: `SpanIndex`, `Backings` | 6 |
| `crates/retrace-box/src/lib.rs`, `crates/retrace-box/Cargo.toml` | field type, constructors, four lookups; dev-dep `retrace-sim` | 6 |
| `crates/retrace/src/gdbserver.rs`, `crates/retrace/lldb/retrace.py` | `disarm-rsi` | 7 |
| `crates/retrace/src/debug.rs`, `crates/retrace/src/gdbserver.rs` | `Halt::ThreadExited` (Task 8); `Halt::StepInterrupted` (Task 11); not-running step (Task 12) | 8, 11, 12 |
| `crates/retrace/tests/gdbserver_e2e.rs` | rows for Tasks 7, 8, 11, 12 | 7, 8, 11, 12 |
| `crates/retrace/tests/lldb_e2e.rs` | rows for Tasks 8, 9, 10, 11, 12 | 8–12 |
| `crates/retrace-guest/asm/btchain.s`, `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs` | create: the arm64e call-chain fixture | 10 |
| `README.md`, `docs/status-log.md`, `CLAUDE.md`, `docs/sweep-evidence/<date>-m44/` | docs and the sweep | 13 |
| `.superpowers/sdd/2026-09-27-retrace-m44-owed/{gate,tally,predict}.sh` | the close | 14 |

**Test-count prediction (made here, reconciled at the close):**

| Task | Tests added |
|---|---|
| 1 | `nocancel.rs` 1 (**new binary**); `retrace-arch` unit `m44_syscall_numbers` 1 |
| 3 | `dupfd_e2e` 1 |
| 4 | `apple_walls_e2e` 2 |
| 5 | `skiplines.rs` 3 (**new binary**) |
| 6 | `retrace-box` unit 3 |
| 7 | `gdbserver_e2e` 1 |
| 8 | `gdbserver_e2e` 1, `lldb_e2e` 1 |
| 9 | `lldb_e2e` 2 |
| 10 | `lldb_e2e` 1 |
| 12 | `gdbserver_e2e` 1 (the does-not-exist row); the three rewrites are net 0 |

That is **+18 tests over 144 + 2 = 146 binaries**, before t0. Task 14 re-derives this from source.

---

### Task 0 (t0): Measurements first

**Files:**
- Modify: `crates/retrace-core/src/lib.rs:157-158` (committed: kept, per M44 §3b invariants)
- Create: `docs/superpowers/specs/2026-09-27-retrace-m44-owed-measurements.md`
- Create: `docs/sweep-evidence/<t0 date>-m44-t0/README.md` plus kept stderr files (name the directory for the day t0 runs, e.g. `2026-09-27-m44-t0`, as M37–M39 did)

**Interfaces:**
- Consumes: nothing.
- Produces: the measurements file's answers, which later tasks read by section:
  - **M1:** 374 routed, or its exact row.
  - **M2:** 461's third kind, `Ptr` or `Dest(Reg(3))`, with the citation.
  - **M3:** per binary: un-ignore, or the new wall in the recorder's own words (landmark, rc/rp, evidence file), plus `ed`'s and `ls`'s native rc and stdout length.
  - **M4:** the twin set and the `ORPHANS` list, plus arm mismatches.
  - **M5:** (i) CPU seconds, (ii) the fixture decision, (iii) the packet-count baselines, (iv) `next`'s pc.

**Everything experimental in this task is throwaway.** Commit the `[trap]` widening first. After that, the only uncommitted edits are experiments, restored with `git checkout -- <file>`.

- [ ] **Step 1: Widen `[trap]` to eight arguments and commit**

`kevent_qos` carries its `flags` in `x7`, and today's line prints `x0`–`x5` (M44 §2b). Replace lines 157–158:

```rust
                eprintln!("[trap] num={} (0x{:x}) pc={:#x} args=[{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x},{:#x}]",
                    *num as i64, num, b.position(), args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7]);
```

Check that nothing parses the six-argument form. The only other `[trap] num=` text in the tree is a static capture that no test reads, found while writing this plan:

```bash
grep -rn -a -F '[trap] num=' crates tools | grep -v 'retrace-core/src/lib.rs:15[78]'
```

Expected: only `crates/retrace-arch/tests/census.rs:2` (a doc comment), `crates/retrace-core/src/lib.rs:891` (a comment), and `crates/retrace-core/tests/fixtures/mach_msg2_capture.txt` (unreferenced: `grep -rn mach_msg2_capture crates` finds only the file itself).

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
mkdir -p $L
cargo build -p retrace > $L/t0-build.log 2>&1; echo "exit=$?"
git add crates/retrace-core/src/lib.rs
git commit -m "M44 t0: [trap] prints x0-x7 — kevent_qos's flags are x7 (M44 §2b)"
```

Expected: `exit=0`, one commit.

- [ ] **Step 2: M1: decode automationmodetool's `kevent_qos`**

Throwaway: inside the same `if trace_log` block, after the `[trap]` line, add a dump of the change list. **Do not add a row for 374.** Forwarding it is halt condition 4.

```rust
                if *num == 374 {
                    if let Some(ipa) = b.va_to_ipa(args[1]) {
                        eprintln!("[kevent_qos changelist] {:02x?}", b.read_guest_checked(ipa, 72 * args[2] as usize));
                    }
                }
```

Build, then run it:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
export RETRACE_TRACE=1
cargo build -p retrace > $L/t0-m1-build.log 2>&1; echo "exit=$?"
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn /usr/bin/automationmodetool -o /private/tmp/claude-501/m44-amt.bin > $L/t0-m1.out 2> $L/t0-m1.err; echo "exit=$?"
grep -a -E '^\[trap\] num=374 |^\[kevent_qos' $L/t0-m1.err
```

Expected: exit 101 (the M33 panic, unchanged), plus one `[trap] num=374` line and its change-list dump.

From xnu's `bsd/sys/event_private.h` (apple-oss-distributions/xnu on GitHub), cite:
- the `KEVENT_FLAG_*` values, including `KEVENT_FLAG_WORKQ` and `KEVENT_FLAG_WORKLOOP`;
- `struct kevent_qos_s` and its size, and the offset of `ident` (u64) and `filter` (i16);
- the prototype, confirming `flags` is `x7`.

If `sizeof(struct kevent_qos_s)` is not 72, re-run with the cited size.

**Decide (R4, plus one refinement this plan adds):** a row is allowed only if **all four** hold:
1. `x7` has neither the workqueue nor the workloop flag.
2. `x0` is a guest slot bound by an earlier `kqueue` (362). Look for a `[trap] num=362` earlier in the log, and its return in the trace.
3. No change-list entry's `filter` is a descriptor filter (`EVFILT_READ` −1, `EVFILT_WRITE` −2, `EVFILT_VNODE` −4, and others; cite the list from `sys/event.h`).
4. **New here:** `eventlist`'s extent, `nevents × sizeof(struct kevent_qos_s)`, is citable and inside 64 KiB, and `data_out`/`data_available` are NULL. `DestLen` cannot express count × size, so anything else is not a row.

Otherwise 374 is **routed** (the expected outcome). Restore with `git checkout -- crates/retrace-core/src/lib.rs`.

- [ ] **Step 3: M2: `getattrlistbulk`'s cap and `ls`'s buffer size**

Read `getattrlistbulk` in xnu's `bsd/vfs/vfs_attrlist.c`, and cite any check on `uap->bufferSize` (or the size actually copied out) before `copyout`, with file and function. With `RETRACE_TRACE=1` exported, run `record-dyn /bin/ls` from the worktree root and grep `[trap] num=461`; `x3` is the size. **Decide:** a cited cap ≤ 65,536 means `Ptr` (the `getattrlist` precedent, M34 Ruling 1). No citable cap, or no network to read xnu, means `Dest(Reg(3))`, the safe default: it widens the window and clamps the forward.

- [ ] **Step 4: M3: where each target lands with throwaway rows**

Throwaway rows in `crates/retrace-arch/src/lib.rs`:
- change `SYS_OPENAT =>` to `SYS_OPENAT | 464 =>`;
- add `345 => row!(P, [Path, Ptr]),`;
- add `461 => row!(P, [Fd, Ptr, Dest(Reg(3)), Scalar, Scalar]),`.

Build `retrace`. **Controller-run** (the sweep blocks for minutes; see the SDD lessons):

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/docs/sweep-evidence/2026-09-27-m44-t0
mkdir -p $E
printf '%s\n' /bin/ed /bin/ls /usr/bin/desdp /usr/bin/dyld_info /usr/bin/flex /usr/bin/dddiagnose /usr/bin/automationmodetool > $L/t0-m3-list.txt
export RETRACE_SWEEP_LIST=$L/t0-m3-list.txt
export RETRACE_SWEEP_KEEP=$E
export RETRACE_SWEEP_KEEP_ALL=1
tools/apple-sweep.sh > $L/t0-m3-sweep.log 2>&1; echo "exit=$?"
grep -a -E '^(PASS|FAIL|ROW|TALLY)' $L/t0-m3-sweep.log
```

Also take each target's native outcome, for any gate whose clean exit is not 0:

```bash
for b in /bin/ed /bin/ls; do "$b" </dev/null > $L/t0-m3-native-$(basename $b).out 2>&1; echo "$b rc=$?"; wc -c < $L/t0-m3-native-$(basename $b).out; done
```

Run `ls` from `crates/retrace`, the cwd a gate runs in, to get its native output.

**Decide per binary:**
- **un-ignore**: record and replay both exit with the native rc and stdout is byte-identical;
- **re-park**: take the wall's first `RECORD ERROR:` / `panicked at` line, landmark, rc/rp and evidence file from the `ROW` line.

The spec's §2b inference is that the xcrun trio lands on the exec refusal. Confirm or refute it with the recorder's `refusing execve`/`posix_spawn` line. Restore with `git checkout -- crates/retrace-arch/src/lib.rs`.

- [ ] **Step 5: M4: the `_nocancel` twin set and the intercepting arms**

```bash
SDK=$(xcrun --show-sdk-path)
grep -E '^#define[[:space:]]+SYS_[a-z_0-9]+_nocancel[[:space:]]' $SDK/usr/include/sys/syscall.h | awk '{print $2, $3}' > $L/t0-m4-nocancel.txt
wc -l < $L/t0-m4-nocancel.txt
```

Expected: 32. For each, find the plain name's number (`grep -E "^#define[[:space:]]+SYS_<plain>[[:space:]]"`). Use `cargo test -p retrace-arch --lib` via a throwaway `#[test]` that prints `arg_kinds(p).is_some(), arg_kinds(nc).is_some()` for each pair, or read the `match` arms. Record:
(a) every pair where exactly one side has a row, or both have different rows (expected to include `openat` 463/464 and `connect` 98/409);
(b) every `_nocancel` name with no plain twin: that is `ORPHANS`, sorted by name, each with its reason.

Then the arms: for each plain number in a pair, grep `crates/retrace-core/src/lib.rs` (`record_box` and `ReplaySession::advance`) and `crates/retrace-box/src/lib.rs` (`forward_and_diff`, `guest_fcntl_dupfd`, `is_console_write`, `is_console_close`) for a special case matching the plain constant. Record each place that matches the plain number without its twin. For each such mismatch, also record whether any corpus guest reaches the twin (`tests/census.rs`, and M3's traces).

- [ ] **Step 6: M5: debugger baselines** (controller-run for (i))

(i) **CPU.** Put a copy of `c652cf1` beside the worktree without a second git worktree, and build both:

```bash
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/m44-t0
mkdir -p $S/m42base
git archive c652cf1 | tar -x -C $S/m42base
```

Write `$S/cpu.sh`. `/usr/bin/time` must run from a script, and it runs one test three times per tree:

```bash
#!/bin/bash
# usage: cpu.sh <tree> <test-target> <test-name> <out>
cd "$1" || exit 2
cargo test -p retrace --test "$2" --no-run > /dev/null 2>&1 || { echo "build failed" > "$4"; exit 2; }
for i in 1 2 3; do
  /usr/bin/time -p cargo test -p retrace --test "$2" "$3" -- --test-threads=1 > /dev/null 2>> "$4"
done
```

Run it for `hitorder_e2e oracle_threadrust_breakpoints_at_both_switches` and for `cpython_crash_e2e` (the whole target), on `$S/m42base` and on the worktree, and record the three `user` lines of each. If the test's body differs between the two trees (`git diff c652cf1 64e471e -- crates/retrace/tests/hitorder_e2e.rs`), say so beside the number.

(ii) **arm64e fixture.** Confirm there's no arm64e guest with a call chain:

```bash
grep -n 'arm64e' crates/retrace-guest/build.rs
```

Every hit is `strip47`/`bfamstrip`, single-function asm. Record "B5 builds `btchain`".

(iii) **lldb baselines.** For each of three shapes, write an lldb command file that begins `log enable -f <file> gdb-remote packets`, then run it bounded:

```bash
perl -e 'alarm shift; exec @ARGV' 120 /usr/bin/lldb -x -b -s <cmds> </dev/null
```

Start `retrace gdbserver` on a `threadrust` recording (`util::rsp::threadrust_block`'s trace, or record one with `record-dyn`). The shapes:
- **A:** the blocked step past another thread's breakpoint: `lldb_e2e`'s session A commands.
- **B:** `thread select <other>; thread step-inst`: session B.
- **C:** a step across the child's `bsdthread_terminate` (361): `breakpoint set -a <its svc>`, then `process continue` until in its window, `thread step-inst`, `thread list`.

Record `grep -c 'vCont;s' <file>`, whether `END` printed, and the exit status.

(iv) **`next` without a line table.** On `crashy`, find the first `bl` in `_main`. Its address `B` is `nm crashy | grep ' _main$'` plus the offset of the first word with `w & 0xfc000000 == 0x94000000`, read with `otool -tv` (`EXE_BASE` equals `crashy`'s `__TEXT` vmaddr `0x100000000`, so `nm` addresses are guest pcs). Run the lldb script `breakpoint set -a B`, `process continue`, `next`, `register read pc`, `script print("END")`, and record the pc and the packet count.

- [ ] **Step 7: Write the measurements file, check the halt conditions, commit**

`docs/superpowers/specs/2026-09-27-retrace-m44-owed-measurements.md` gets a header naming the spec, the date, the branch commit, and the retrace binary's sha256. Then comes one section per M1–M5. Each section gives the method as run, the raw result (log file names), and the **decision**, with its citation. **Halt** (spec §7 halt 1) instead of committing if any of these holds:
- `automationmodetool` does not reach 374;
- `ls` does not reach 461;
- any of `ed`/`desdp`/`dyld_info`/`flex` does not reach 464;
- `dddiagnose` does not reach 345;
- the twin set does not contain 464.

Write `$E/README.md` the way `docs/sweep-evidence/2026-09-17-m39/README.md` is written: method, binary hash, and what each kept file is.

```bash
git add docs/superpowers/specs/2026-09-27-retrace-m44-owed-measurements.md docs/sweep-evidence/
git commit -m "M44 t0: measurements M1-M5 — kevent_qos routed or tabled, 461's shape, where each target lands, the twin set, debugger baselines"
```

---

### Task 1 (A1 + twins): every `_nocancel` spelling shares its plain form's row

**Files:**
- Create: `crates/retrace-arch/tests/nocancel.rs`
- Modify: `crates/retrace-arch/src/lib.rs` (constants near line 76; the `SYS_OPENAT` and `SYS_CONNECT` arms at lines 550 and 418; the unit tests `fd_operands_covers_the_measured_surface` ~1567 and a new `m44_syscall_numbers` beside `m10_syscall_numbers` ~1858)
- Modify: `crates/retrace-arch/tests/census.rs` (+ 464)
- Modify: `crates/retrace-arch/tests/legacy_equivalence.rs` (`EXPECTED_DIFFS`)
- Possibly modify: the arms t0 M4 found (`crates/retrace-core/src/lib.rs` and/or `crates/retrace-box/src/lib.rs`)

**Interfaces:**
- Consumes: t0 M4 (twin set, `ORPHANS`, arm mismatches).
- Produces: `pub const SYS_OPENAT_NOCANCEL: u64 = 464;` and `pub const SYS_CONNECT_NOCANCEL: u64 = 409;` in `retrace_arch`. Task 2 adds `SYS_STATFS64` and `SYS_GETATTRLISTBULK` beside them.

- [ ] **Step 1: Write the failing structural test**

Create `crates/retrace-arch/tests/nocancel.rs`:

```rust
//! M44 A1 (spec §3b): every `_nocancel` spelling shares its plain form's `arg_kinds` row — the rule
//! `ArgKind::Source`'s comment states and five milestones broke by hand (M9, M10, M27, `sendto`,
//! `openat`). Names and numbers both come from the SDK's own `sys/syscall.h`, read at test time
//! (`hv-sys`'s build already needs the SDK), so a row keyed at the wrong number — M44 §2a's class,
//! 468 carried as `getattrlistat` for seven milestones — cannot satisfy it.
use retrace_arch::arg_kinds;
use std::collections::BTreeMap;

/// `SYS_<name> <number>` for every syscall the SDK header defines.
fn sdk_syscalls() -> BTreeMap<String, u64> {
    let out = std::process::Command::new("xcrun").arg("--show-sdk-path").output().expect("run xcrun");
    assert!(out.status.success(), "xcrun --show-sdk-path failed");
    let path = format!("{}/usr/include/sys/syscall.h", String::from_utf8(out.stdout).unwrap().trim());
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut m = BTreeMap::new();
    for line in text.lines() {
        let mut w = line.split_whitespace();
        if w.next() != Some("#define") { continue; }
        let (Some(name), Some(num)) = (w.next(), w.next()) else { continue };
        let Some(name) = name.strip_prefix("SYS_") else { continue };
        if let Ok(n) = num.parse::<u64>() { m.insert(name.to_string(), n); }
    }
    assert!(m.len() > 400, "parsed only {} syscall numbers from {path}", m.len());
    m
}

/// `_nocancel` names with no plain twin in the SDK, sorted by name, each with t0 M4's reason.
/// Asserted EXACTLY, so the list cannot rot in either direction.
const ORPHANS: &[(&str, &str)] = &[];

#[test]
fn every_nocancel_spelling_shares_its_plain_forms_row() {
    let sdk = sdk_syscalls();
    let (mut orphans, mut mismatched, mut pairs) = (Vec::new(), Vec::new(), 0);
    for (name, &nc) in &sdk {
        let Some(plain) = name.strip_suffix("_nocancel") else { continue };
        let Some(&p) = sdk.get(plain) else { orphans.push(name.as_str()); continue };
        pairs += 1;
        let (a, b) = (arg_kinds(p), arg_kinds(nc));
        if (a.is_some() || b.is_some()) && a != b {
            mismatched.push(format!("{plain} ({p}): {a:?}\n  {name} ({nc}): {b:?}"));
        }
    }
    let expected: Vec<&str> = ORPHANS.iter().map(|(n, _)| *n).collect();
    assert_eq!(orphans, expected, "_nocancel names with no plain twin must be exactly ORPHANS");
    assert!(pairs >= 25, "only {pairs} twin pairs parsed — the parse is wrong, not the table");
    assert!(mismatched.is_empty(), "twins whose rows differ (the _nocancel trap):\n{}", mismatched.join("\n"));
}
```

Fill `ORPHANS` from t0 M4(b) as `("name_nocancel", "reason")` entries. If M4 found none, leave it empty.

- [ ] **Step 2: Run it and watch it fail on the expected pairs**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
cargo test -p retrace-arch --test nocancel --no-fail-fast -- --test-threads=1 > $L/t1-red.log 2>&1; echo "exit=$?"
grep -a -A40 'twins whose rows differ' $L/t1-red.log
```

Expected: exit 101. The failure lists `openat (463): Some(…)` against `openat_nocancel (464): None`, and `connect (98)` against `connect_nocancel (409)`, plus exactly the other pairs t0 M4(a) listed. **A pair the red run lists that M4 did not, or the reverse, is a finding.** Reconcile it in the measurements file before going on.

- [ ] **Step 3: Add the constants and join each twin to its plain row**

Beside `SYS_OPENAT` (line 76):

```rust
/// M44: `openat`'s `_nocancel` twin (SDK `SYS_openat_nocancel 464`). Reached by `/bin/ed` and the
/// `xcrun` trio (`desdp`, `dyld_info`, `flex`) since M38 moved their walls.
pub const SYS_OPENAT_NOCANCEL: u64 = 464;
/// M44: `connect`'s `_nocancel` twin (SDK `SYS_connect_nocancel 409`). No corpus guest reaches it;
/// the twin rule tables it (`tests/nocancel.rs`).
pub const SYS_CONNECT_NOCANCEL: u64 = 409;
```

Change the two arms:

```rust
        // connect(int s, const struct sockaddr *name, socklen_t namelen) / connect_nocancel:
        // namelen > SOCK_MAXADDRLEN (255) is rejected — the cited bound. M44: the twin joins.
        SYS_CONNECT | SYS_CONNECT_NOCANCEL => row!(P, [Fd, Ptr, Scalar]),
```

```rust
        // openat(int dirfd, const char *path, int flags, mode_t mode) / openat_nocancel → a NEW
        // descriptor. The dirfd is translated; AT_FDCWD passes through untouched. M44: 464 was the
        // `_nocancel` trap's fifth instance, and four corpus binaries stopped on it.
        SYS_OPENAT | SYS_OPENAT_NOCANCEL => row!(F, [Fd, Path, Scalar, Scalar]),
```

For every other pair from t0 M4(a), join the side without a row to the other side's arm with `|`, the same way, citing the SDK number in the comment. If both sides have rows that differ, **halt and ask**: which row is right is a judgment the plan cannot make.

- [ ] **Step 4: Run the structural test and watch it pass**

```bash
cargo test -p retrace-arch --test nocancel --no-fail-fast -- --test-threads=1 > $L/t1-green.log 2>&1; echo "exit=$?"
```

Expected: `exit=0`, `1 passed`.

- [ ] **Step 5: Census, legacy equivalence, unit tests**

In `tests/census.rs`, insert `464` after `463` in `CENSUS`. Add this at the end of the header comment:

```rust
//! M44 t0 adds numbers reached since M38 moved the Apple-sweep walls, each measured from the same
//! `[trap] num=` lines (`docs/superpowers/specs/2026-09-27-retrace-m44-owed-measurements.md` §M3):
//! 464 `openat_nocancel` (`/bin/ed`, `desdp`, `dyld_info`, `flex`).
```

In `tests/legacy_equivalence.rs`, append to `EXPECTED_DIFFS`, before its closing `];`:

```rust
    // M44 A1: `_nocancel` twins the SDK-driven twin test (`tests/nocancel.rs`) found. Each shares
    // its plain form's row, so its views differ from the pre-M33 tables exactly where the plain
    // form's row differs from "no row".
    (464, View::FdOperands, "openat_nocancel(dirfd, …): openat's twin — M44; exercised (/bin/ed, desdp, dyld_info, flex)"),
    (464, View::AllocatesFd, "openat_nocancel returns a new descriptor, as openat does — M44; exercised (/bin/ed, desdp, dyld_info, flex)"),
    (409, View::FdOperands, "connect_nocancel(s, …): connect's twin — M44; unexercised"),
```

Add one entry per differing view for every other twin Step 3 tabled. The word is `unexercised` unless the number is in `CENSUS`; the existing test `exercised_and_unexercised_match_the_census` enforces that.

In `crates/retrace-arch/src/lib.rs`'s test module:
- add `SYS_OPENAT_NOCANCEL, SYS_CONNECT_NOCANCEL` to the first array in `fd_operands_covers_the_measured_surface`;
- add, after `m25_syscall_numbers`:

```rust
    #[test]
    fn m44_syscall_numbers() {
        assert_eq!((SYS_OPENAT_NOCANCEL, SYS_CONNECT_NOCANCEL), (464, 409));
    }
```

- [ ] **Step 6: Fix each reachable arm mismatch (only if t0 M4 found one)**

For each place M4 found matching a plain number without its twin, where a corpus guest reaches the twin, change the condition to match both (`num == X || num == X_NOCANCEL`). Change it in **both** `record_box` and `ReplaySession::advance` if the arm is there (symmetry rule 1), or in the one `Box_` method if it is in the box. Pin each fix with a test in the file that already tests that arm (`crates/retrace-box/tests/fdtable.rs`, `consoleclose.rs`, or the arm's e2e). The test asserts the twin takes the arm, by the difference the arm makes. For example, a console-mirrored write through `write_nocancel` appears on stdout and in the trace with no forward.

For each mismatch **no** corpus guest reaches, add a line to the README's Known limits in Task 13. Do not change the arm unmeasured. If M4 found none, write "M4 found no arm mismatch" in the task report and skip this step.

- [ ] **Step 7: Run the whole crate, clippy, commit**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
cargo test -p retrace-arch --no-fail-fast -- --test-threads=1 > $L/t1-arch.log 2>&1; echo "exit=$?"
cargo clippy -p retrace-arch --all-targets -- -D warnings > $L/t1-clippy.log 2>&1; echo "exit=$?"
```

Expected: both `exit=0`. `legacy_equivalence`'s three tests, and `census.rs`'s two (which run twice), pass.

```bash
git add crates/retrace-arch
git commit -m "M44 A1: every _nocancel twin shares its plain row, checked against the SDK — 464 and 409 join"
```

- [ ] **Step 8: Control, on the committed tree**

Delete `| SYS_OPENAT_NOCANCEL` from the `openat` arm. Run `cargo test -p retrace-arch --test nocancel -- --test-threads=1`. Expected: it fails naming `openat (463)` and `openat_nocancel (464)`. Restore with `git checkout -- crates/retrace-arch/src/lib.rs`, and record the symptom in the report.

---

### Task 2 (A2): the rows for 345 and 461, and the 374 decision

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (constants; rows in the `// ---- paths` section after `SYS_FSTATAT64`; unit tests)
- Modify: `crates/retrace-arch/tests/census.rs` (+ 345, 461), `crates/retrace-arch/tests/legacy_equivalence.rs`
- Modify: `crates/retrace-box/tests/truncguard.rs` (`the_window_widens_for_the_m34_rows_and_not_for_getattrlist`)
- Modify (only if t0 M1 tabled 374): the same three files for 374

**Interfaces:**
- Consumes: t0 M1 (374's decision), t0 M2 (461's third kind), `SYS_OPENAT_NOCANCEL` from Task 1.
- Produces: `pub const SYS_STATFS64: u64 = 345;` and `pub const SYS_GETATTRLISTBULK: u64 = 461;`.

- [ ] **Step 1: Make the census say what t0 measured, and watch it fail**

In `tests/census.rs`, insert `345` between `344` and `346`, and `461` between `427` and `463`. Extend the M44 header note from Task 1:

```rust
//! 345 `statfs64` (`/usr/bin/dddiagnose`) and 461 `getattrlistbulk` (`/bin/ls`).
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
cargo test -p retrace-arch --test census --no-fail-fast -- --test-threads=1 > $L/t2-red.log 2>&1; echo "exit=$?"
grep -a 'census numbers with no arg_kinds row' $L/t2-red.log
```

Expected: exit 101, with `census numbers with no arg_kinds row: [345, 461]`.

- [ ] **Step 2: Add the constants and rows**

Beside the Task 1 constants:

```rust
/// M44: `statfs64(const char *path, struct statfs64 *buf)` — `fstatfs64`'s path twin (SDK 345/346).
/// Reached by `/usr/bin/dddiagnose` since M38.
pub const SYS_STATFS64: u64 = 345;
/// M44: `getattrlistbulk(int dirfd, struct attrlist *alist, void *attrBuf, size_t attrBufSize,
/// uint64_t options)` (SDK 461). Reached by `/bin/ls`, which bulk-enumerates a directory with it.
pub const SYS_GETATTRLISTBULK: u64 = 461;
```

In the `// ---- paths` section, after the `SYS_FSTATAT64` arm:

```rust
        // statfs64(const char *path, struct statfs64 *buf): the path twin of fstatfs64 (SDK
        // 345/346); buf is the same fixed 2,168-byte struct, measured at M29 Task 7 for fstatfs64 —
        // the cited bound. M44 (t0 M3: /usr/bin/dddiagnose reaches it).
        SYS_STATFS64 => row!(P, [Path, Ptr]),
```

Then **exactly one** of these, per t0 M2.

*If M2 cited a cap ≤ 65,536:*

```rust
        // getattrlistbulk(int dirfd, struct attrlist *alist, void *attrBuf, size_t attrBufSize,
        //                 uint64_t options): alist is the fixed 24-byte struct its getattrlist
        // siblings cite. attrBuf is kernel-bounded at <M2's cap> (<M2's file and function>), inside
        // the window, so Ptr by the rule above — the M34 Ruling 1 shape (t0 M2). ls passed <M2's x3>.
        SYS_GETATTRLISTBULK => row!(P, [Fd, Ptr, Ptr, Scalar, Scalar]),
```

*Otherwise:*

```rust
        // getattrlistbulk(int dirfd, struct attrlist *alist, void *attrBuf, size_t attrBufSize,
        //                 uint64_t options): alist is the fixed 24-byte struct its getattrlist
        // siblings cite. attrBuf is filled with as many entries as fit in attrBufSize — the
        // CALLER's size, with no kernel cap below the window that t0 M2 could cite — so Dest with
        // its length in x3: the clamp and the diff window both follow it (M26's class). ls passed
        // <M2's x3>. The dirfd is translated; the directory offset it advances is the host's.
        SYS_GETATTRLISTBULK => row!(P, [Fd, Ptr, Dest(Reg(3)), Scalar, Scalar]),
```

Replace the `<M2 …>` markers with the measured values; they are measurement data, not placeholders.

**If t0 M1 tabled 374** (all four conditions held), add its row here as M1 wrote it, with `SYS_KEVENT_QOS: u64 = 374`, and put 374 in `CENSUS`. **If M1 routed it** (expected), add nothing for 374.

- [ ] **Step 3: Unit tests, `EXPECTED_DIFFS`, `truncguard`**

In `m44_syscall_numbers`:

```rust
        assert_eq!((SYS_STATFS64, SYS_GETATTRLISTBULK), (345, 461));
```

Add `SYS_GETATTRLISTBULK` to `fd_operands_covers_the_measured_surface`'s first array.

*If `Dest`:* in `dest_buffer_knows_where_each_length_lives`, add

```rust
        // M44 t0 M2: getattrlistbulk fills attrBuf (x2) up to the caller's attrBufSize (x3).
        assert_eq!(dest_buffer(SYS_GETATTRLISTBULK), Some((2, DestLen::Reg(3))));
```

*If `Ptr`:* in `dest_buffer_omits_what_it_should`, add

```rust
        assert_eq!(dest_buffer(SYS_GETATTRLISTBULK), None,
            "getattrlistbulk is kernel-bounded at <M2's cap> and stays Ptr (t0 M2)");
```

`EXPECTED_DIFFS`:

```rust
    // M44 A2: getattrlistbulk's dirfd, and (if Dest) its buffer. statfs64 has neither an Fd nor a
    // Dest, so no view of it differs and it has no entry.
    (461, View::FdOperands, "getattrlistbulk(dirfd, …) — M44; exercised (/bin/ls)"),
    (461, View::DestBuffer, "getattrlistbulk's attrBuf is a Dest of x3 bytes (t0 M2: no cap below the window) — M44; exercised (/bin/ls)"),
```

Keep the second line only if the row is `Dest`.

In `crates/retrace-box/tests/truncguard.rs`, at the end of `the_window_widens_for_the_m34_rows_and_not_for_getattrlist`, where `args[3]` is already `150_000`:

```rust
    // M44 A2 (t0 M2): getattrlistbulk's attrBuf is x2 and its length x3.
    assert_eq!(b.diff_window_for_test(retrace_arch::SYS_GETATTRLISTBULK, 2, AVAIL, &args), 150_000,
        "getattrlistbulk has no kernel cap below the window: its Dest follows attrBufSize");
```

If the row is `Ptr`, the expected value is `FLAT` and the message names M2's cap.

- [ ] **Step 4: Run, clippy, commit**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
cargo test -p retrace-arch --no-fail-fast -- --test-threads=1 > $L/t2-arch.log 2>&1; echo "exit=$?"
cargo test -p retrace-box --test truncguard --no-fail-fast -- --test-threads=1 > $L/t2-truncguard.log 2>&1; echo "exit=$?"
cargo clippy -p retrace-arch -p retrace-box --all-targets -- -D warnings > $L/t2-clippy.log 2>&1; echo "exit=$?"
git add crates/retrace-arch crates/retrace-box/tests/truncguard.rs
git commit -m "M44 A2: statfs64 and getattrlistbulk rows (t0 M2's shape); 374 per t0 M1"
```

Expected: three `exit=0`.

- [ ] **Step 5: Control, on the committed tree**

Change the 461 row's `Dest(Reg(3))` to `Ptr` (or the reverse). `truncguard` and the `dest_buffer` unit test must both fail. Restore with `git checkout -- crates/retrace-arch/src/lib.rs`.

---

### Task 3 (A3): `F_DUPFD_CLOEXEC` sets close-on-exec on the host dup

**Files:**
- Modify: `crates/retrace-guest/c/dupfd_dyn.c`
- Modify: `crates/retrace/tests/dupfd_e2e.rs`
- Modify: `crates/retrace-box/src/lib.rs` (`guest_fcntl_dupfd`, ~4279)

**Interfaces:** Consumes nothing new; produces nothing later tasks use.

- [ ] **Step 1: Extend the fixture and the expectation (the failing test)**

In `dupfd_dyn.c`, after the `printf("setfd=%d\n", …)` line and before `fflush(stdout);`:

```c
    /* M44 A3: F_DUPFD_CLOEXEC on a FILE descriptor (an Open slot bound to the host dup), then
       F_GETFD. Native reads 1; before M44 a forwarded F_GETFD read the host dup's clear flag, 0. */
    int c = fcntl(f, F_DUPFD_CLOEXEC, 14);
    if (c < 0) return 7;
    printf("cloexec=%d\n", fcntl(c, F_GETFD));
    close(c);
```

Update the fixture's header comment. Expected stdout becomes `n=10\nsetfd=0\ncloexec=1\nalias\n`.

In `dupfd_e2e.rs`:

```rust
const EXPECT_STDOUT: &[u8] = b"n=10\nsetfd=0\ncloexec=1\nalias\n";
```

In `the_trace_carries_f_dupfd_returning_the_guest_slot_and_f_setfd_forwarded_verbatim`, change `assert_eq!(dupfds.len(), 2, …)` to `3`, and its comment to name the three: `(f, 10) -> 10`, `(f, 14) -> 14`, `(1, 12) -> 12`. Add:

```rust
#[test]
fn f_dupfd_cloexec_sets_close_on_exec_as_native_does() {
    // M44 A3: the one observable of F_DUPFD_CLOEXEC's bit is a forwarded F_GETFD, which reads the
    // HOST descriptor's flag. Before M44 that was the host `dup`'s clear flag, so the guest read 0
    // where native reads 1 — deterministic on both sides, a fidelity gap and not a divergence.
    let path = scratch_file("cloexec");
    let _ = std::fs::remove_file(&path);
    let out = util::assert_rung_records_and_replays(retrace_guest::DUPFD_DYN, &[path.to_str().unwrap()], EXPECT_STDOUT);
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("\ncloexec=1\n"), "F_GETFD after F_DUPFD_CLOEXEC must read FD_CLOEXEC (1). Got:\n{s}");
    let _ = std::fs::remove_file(&path);
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
cargo test -p retrace --test dupfd_e2e --no-fail-fast -- --test-threads=1 > $L/t3-red.log 2>&1; echo "exit=$?"
grep -a -E 'cloexec=|test result' $L/t3-red.log
```

Expected: exit 101. All three tests fail, and the output shows `cloexec=0`.

- [ ] **Step 3: Set the bit**

In `guest_fcntl_dupfd`, after the `if dup < 0 { … }` block:

```rust
        // M44 A3: F_DUPFD_CLOEXEC's bit, on the host descriptor that stands for the guest's. Its one
        // observable is a forwarded F_GETFD, which reads the host flag: without this the guest read
        // 0 where native reads 1 (owed since M38's final review). Record-side only, like the `dup`
        // itself — the recorded F_GETFD return carries the bit to replay.
        if args[1] == F_DUPFD_CLOEXEC {
            let r = unsafe { libc::fcntl(dup, libc::F_SETFD, libc::FD_CLOEXEC) };
            assert_eq!(r, 0, "F_SETFD(FD_CLOEXEC) on a fresh host dup failed: {}", std::io::Error::last_os_error());
        }
```

If `F_DUPFD_CLOEXEC` is not already in scope in `retrace-box/src/lib.rs`, write `retrace_arch::F_DUPFD_CLOEXEC`.

- [ ] **Step 4: Run it and watch it pass; clippy; commit**

```bash
cargo test -p retrace --test dupfd_e2e --no-fail-fast -- --test-threads=1 > $L/t3-green.log 2>&1; echo "exit=$?"
cargo clippy -p retrace-box -p retrace --all-targets -- -D warnings > $L/t3-clippy.log 2>&1; echo "exit=$?"
git add crates/retrace-guest/c/dupfd_dyn.c crates/retrace/tests/dupfd_e2e.rs crates/retrace-box/src/lib.rs
git commit -m "M44 A3: F_DUPFD_CLOEXEC sets close-on-exec on the host dup — F_GETFD reads 1 as native"
```

Expected: `3 passed`, clippy `exit=0`.

- [ ] **Step 5: Control, on the committed tree**

Comment out the `if args[1] == F_DUPFD_CLOEXEC { … }` block. `f_dupfd_cloexec_sets_close_on_exec_as_native_does` must fail with `cloexec=0`. Restore with `git checkout -- crates/retrace-box/src/lib.rs`.

---

### Task 4 (A4): the gates

**Files:**
- Modify: `crates/retrace/tests/apple_walls_e2e.rs`

**Interfaces:** Consumes t0 M1/M3 and the rows from Tasks 1–2. Each target's decision is **re-measured** on this build in Step 1, not taken from t0 alone: t0 used throwaway rows, and Task 2's 461 shape may differ from t0's.

- [ ] **Step 1: Re-measure the seven targets on the committed rows** (controller-run)

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/docs/sweep-evidence/2026-09-27-m44-t0/t4
mkdir -p $E
cargo build -p retrace > $L/t4-build.log 2>&1; echo "exit=$?"
export RETRACE_SWEEP_LIST=$L/t0-m3-list.txt
export RETRACE_SWEEP_KEEP=$E
export RETRACE_SWEEP_KEEP_ALL=1
tools/apple-sweep.sh > $L/t4-sweep.log 2>&1; echo "exit=$?"
grep -a -E '^(PASS|FAIL|TALLY)' $L/t4-sweep.log
```

If any row's label differs from t0 M3's, record why in the report before writing its gate.

- [ ] **Step 2: Add `ls` and `ed` gates**

After `dddiagnose_records_and_replays`, write the shape that fits each binary's native outcome (t0 M3):
- **Native exit 0**, stdout equal: `#[test] fn ls_records_and_replays() { records_and_replays_clean("/bin/ls"); }`, with a `///` comment naming M44 and the evidence file.
- **Native exit non-zero** (possible for `ed` at EOF): copy `launchctl_records_and_replays`'s body. Assert `rec.code == <native rc>`, and assert the stdout that proves the guest reached `main` (for `ed`, M3's measured bytes, or empty if it printed none; then assert on stderr's `?` only if M3 shows it). Replay is held to the recording. A doc comment explains why the exit code is not 0 and why no retrace failure can produce it: the CLI's own codes are 2/3/4/5 and 128+signo.
- **Parked:** the same `fn`, with `#[ignore = "…"]` in the file's reason format (below).

- [ ] **Step 3: Rewrite the five existing reasons, or un-ignore**

For each of `automationmodetool`, `desdp`, `dyld_info`, `flex` and `dddiagnose`:
- **Un-ignore:** delete the `#[ignore = …]` line, and add a `///` comment: `M44: un-parked — <syscall> has an arg_kinds row and the binary records to a clean exit and replays bit-for-bit (evidence docs/sweep-evidence/2026-09-27-m44-t0/t4/<name>.{rec,rp}.err).`
- **Re-park:** replace the reason with one in the file's existing format, every field read off the kept evidence:

```text
M44 wall, class <B|C per the charter enum> (<one-phrase class meaning>), parked, <routed to … | not routed>. <path>: <the row M44 added> records (<landmark it was at>); the row now stops <k> landmarks later at `<first RECORD ERROR / panicked at / refusing line, verbatim>` — <the syscall or symbol, and pc>, rc/rp <a>/<b>, landmark <n>. Evidence docs/sweep-evidence/2026-09-27-m44-t0/t4/<name>.{rec,rp}.err. UN-IGNORE when <the specific capability>.
```

For the xcrun trio, if M3 confirmed the exec refusal, the class is C (process creation) and it un-ignores "when exec-in-place is modelled". Keep each existing reason's NOTE about the random `/var/tmp/xcrun_db-XXXXXX` tempfile. For `automationmodetool` routed at 374, the wall is unchanged. Rewrite its reason to add t0 M1's measurement (flags, kq, filters) and "routed to its own milestone (M44 R4)".

- [ ] **Step 4: Run the gates and the positive control**

```bash
cargo test -p retrace --test apple_walls_e2e --no-fail-fast -- --test-threads=1 > $L/t4-gates.log 2>&1; echo "exit=$?"
cargo test -p retrace --test apple_walls_e2e --no-fail-fast -- --ignored --test-threads=1 > $L/t4-ignored.log 2>&1; echo "exit=$?"
grep -a -E '^test |panicked|record exited' $L/t4-ignored.log
```

Expected: the first run passes, and every un-ignored gate is `ok`. In the second, every parked gate **fails**, and its message quotes the wall named in its own reason. That is the file's positive control, per its header. Record any parked gate that passes: it has cleared its wall and must be un-ignored.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace/tests/apple_walls_e2e.rs docs/sweep-evidence/
git commit -m "M44 A4: ls and ed gated; the five M38 row-walls un-parked or re-parked at their measured walls"
```

---

### Task 5 (A5): skip lines reach a gate log

**Files:**
- Modify: `crates/retrace/tests/util/mod.rs`
- Modify: `crates/retrace/tests/{apple_walls_e2e,cpython_crash_e2e,cpython_e2e,jq_e2e,jq_file_e2e,symbolops_e2e,sysbin_e2e,fallthrough_e2e,lldb_e2e}.rs`
- Create: `crates/retrace/tests/skiplines.rs`

**Interfaces:** Produces `util::announce(line: &str)`. Tasks 8–12 use it in `lldb_e2e`.

- [ ] **Step 1: Add `util::announce`, then write the detector and the control (the failing test)**

In `crates/retrace/tests/util/mod.rs`, after `pub struct RunOut`:

```rust
/// One line on the test process's own stderr, past libtest's output capture. libtest captures
/// `eprintln!` in a test that passes, and a skip passes, so an `eprintln!` skip line reaches a gate
/// log only when its test fails (measured at M43's close). Moved here from `lldb_e2e` at M44 (A5).
pub fn announce(line: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{line}");
}
```

Create `crates/retrace/tests/skiplines.rs`:

```rust
//! M44 A5 (spec §3b): skip lines reach a gate log. libtest captures `eprintln!` in a test that
//! passes, and a skip passes, so a skip announced that way never reaches an ordinary gate log
//! (measured at M43's close, Ruling F-4). `util::announce` writes past the capture.
mod util;

/// 1-based line numbers where `src` has `eprintln!(` followed — across whitespace and newlines,
/// since several calls wrap — by a string literal beginning `SKIP`.
fn eprintln_skip_sites(src: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = src[from..].find("eprintln!(") {
        let at = from + i;
        let rest = src[at + "eprintln!(".len()..].trim_start();
        if rest.starts_with("\"SKIP") || rest.starts_with("r\"SKIP") || rest.starts_with("r#\"SKIP") {
            out.push(src[..at].matches('\n').count() + 1);
        }
        from = at + 1;
    }
    out
}

#[test]
fn no_test_target_announces_a_skip_through_eprintln() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests");
    let (mut hits, mut scanned) = (Vec::new(), 0);
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "rs") {
            scanned += 1;
            let src = std::fs::read_to_string(&p).unwrap();
            hits.extend(eprintln_skip_sites(&src).into_iter().map(|l| format!("{}:{l}", p.display())));
        }
    }
    assert!(scanned > 50, "scanned only {scanned} files in {dir}");
    assert!(hits.is_empty(),
        "skip lines written with eprintln! — libtest captures them in a passing test; use util::announce:\n{}",
        hits.join("\n"));
}

#[test]
fn the_detector_finds_a_wrapped_eprintln_skip_and_nothing_else() {
    // Its own positive control: the wrapped shape `cpython_e2e` used, and a non-skip line.
    let src = "fn f() {\n    eprintln!(\n        \"SKIPPED x: {REAL} not found\");\n}\n";
    assert_eq!(eprintln_skip_sites(src), [2]);
    assert!(eprintln_skip_sites("eprintln!(\"not a skip\");").is_empty());
}

#[test]
fn a_skip_line_control_reaches_the_gate_log() {
    // The close greps an ORDINARY gate log (no --nocapture) for this exact line: M44 §6 item 6.
    util::announce("SKIPLINES CONTROL: util::announce reaches a gate log past libtest's capture");
}
```

(This file's own string literals do not trip the detector. In source, the `\n` escapes are two characters, `\` and `n`, so `trim_start` stops at the backslash and the text after `eprintln!(` does not start with `"SKIP`.)

- [ ] **Step 2: Run it and watch it fail**

Expected: `no_test_target_announces_a_skip_through_eprintln` fails, listing about 13 sites across the 8 files (M44 §2d) plus any skip Task 4 added. The other two tests pass.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
cargo test -p retrace --test skiplines --no-fail-fast -- --test-threads=1 > $L/t5-red.log 2>&1; echo "exit=$?"
```

- [ ] **Step 3: Convert every site**

At every site the red run listed, replace `eprintln!(` with `util::announce(`:
- If the literal contains a `{`, wrap it: `util::announce(&format!("…"))`, keeping the literal byte-for-byte.
- If it doesn't, pass the literal directly. `format!` with no arguments trips `clippy::useless_format`.

In `lldb_e2e.rs`, delete the local `fn announce` and its doc comment, and change each `announce(` to `util::announce(`. If clippy then reports `use std::io::Write;` as unused, delete that import.

- [ ] **Step 4: Run, clippy, commit**

```bash
cargo test -p retrace --test skiplines --no-fail-fast -- --test-threads=1 > $L/t5-green.log 2>&1; echo "exit=$?"
grep -a 'SKIPLINES CONTROL' $L/t5-green.log
cargo clippy -p retrace --all-targets -- -D warnings > $L/t5-clippy.log 2>&1; echo "exit=$?"
```

Expected: `3 passed`, and the control line is present in the log **with no `--nocapture`**, which is the channel proven. Clippy `exit=0`. Also build every converted target (`cargo test -p retrace --no-run`) so a typo cannot hide in a file the chunk did not compile.

```bash
cargo test -p retrace --no-run > $L/t5-norun.log 2>&1; echo "exit=$?"
git add crates/retrace/tests
git commit -m "M44 A5: every skip line goes through util::announce, past libtest's capture; a detector and a control"
```

- [ ] **Step 5: Control, on the committed tree**

Change one converted site in `jq_e2e.rs` back to `eprintln!`. The detector must fail and name `jq_e2e.rs:<line>`. Restore with `git checkout -- crates/retrace/tests/jq_e2e.rs`.

---

### Task 6 (B1): the backing-lookup index

**Files:**
- Create: `crates/retrace-box/src/backings.rs`
- Modify: `crates/retrace-box/src/lib.rs`:
  - `mod backings;` near line 6;
  - the field at 494;
  - constructors at ~1378/1945/3184/5994;
  - closures at 1381/1948;
  - `build_tables` at 1194;
  - lookups `read_guest` 4640, `read_guest_checked` 4654, `host_span` ~3380, `backing_of` ~3401
- Modify: `crates/retrace-box/Cargo.toml` (`[dev-dependencies] retrace-sim`)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub(crate) struct Backings` (derefs to `[Backing]`; `push`, `extend`, `remove`, `holding(ipa, len) -> Option<&Backing>`, `containing(ipa) -> Option<&Backing>`) and `pub(crate) struct SpanIndex`. Nothing outside `retrace-box` sees either.

- [ ] **Step 1: Write `SpanIndex` and its failing tests**

Add to `crates/retrace-box/Cargo.toml`:

```toml
[dev-dependencies]
retrace-sim = { path = "../retrace-sim" }
```

Create `crates/retrace-box/src/backings.rs` with the tests and a stub whose lookups answer `None`:

```rust
//! M44 B1: the guest's backings, with a sorted index beside them.
//!
//! `read_guest`, `read_guest_checked`, `host_span` and `backing_of` scanned `backings` linearly, and
//! M43's step pre-decode made that scan hot (+38 % CPU on stepping-heavy tests; status log, M43
//! "What stays owed"). The index answers the same questions by binary search. **It sits beside the
//! Vec and never reorders it** (M44 R2): the Vec's order is what `snapshot` and `checkpoint`
//! iterate, and changing it could change what a snapshot's bytes mean.
//!
//! Correctness rests on one invariant, asserted on every insert: no two backings' IPA spans
//! overlap. Stage-2 maps each backing at its IPA and HVF refuses a mapping over a mapped range, so
//! the invariant already held; the assert makes a violation loud instead of a silent mis-index.
use crate::Backing;

/// `(start, len, pos)` per backing, sorted by `start`; `pos` is the backing's index in the Vec.
#[derive(Default)]
pub(crate) struct SpanIndex { e: Vec<(u64, usize, usize)> }

impl SpanIndex {
    pub(crate) fn insert(&mut self, _start: u64, _len: usize, _pos: usize) {}
    pub(crate) fn remove(&mut self, _pos: usize) {}
    pub(crate) fn holding(&self, _ipa: u64, _len: usize) -> Option<usize> { None }
    pub(crate) fn containing(&self, _ipa: u64) -> Option<usize> { None }
}

#[cfg(test)]
mod tests {
    use super::SpanIndex;
    use retrace_sim::Rng;

    /// The scans the index replaces, over a model Vec kept in insertion order.
    fn lin_holding(m: &[(u64, usize)], ipa: u64, len: usize) -> Option<usize> {
        m.iter().position(|&(s, l)| ipa >= s && ipa + len as u64 <= s + l as u64)
    }
    fn lin_containing(m: &[(u64, usize)], ipa: u64) -> Option<usize> {
        m.iter().position(|&(s, l)| ipa >= s && ipa < s + l as u64)
    }

    #[test]
    fn the_index_answers_every_probe_as_the_linear_scan_did() {
        const G: u64 = 0x4000; // one 16 KiB granule
        for seed in 0..64 {
            let mut r = Rng::seed(seed);
            let (mut idx, mut m) = (SpanIndex::default(), Vec::<(u64, usize)>::new());
            for _ in 0..200 {
                if !m.is_empty() && r.below(4) == 0 {
                    let pos = r.below(m.len() as u64) as usize;
                    idx.remove(pos);
                    m.remove(pos);
                } else {
                    let start = r.below(4096) * G;
                    let len = ((1 + r.below(8)) * G) as usize;
                    if m.iter().all(|&(s, l)| start + len as u64 <= s || s + l as u64 <= start) {
                        idx.insert(start, len, m.len());
                        m.push((start, len));
                    }
                }
                for _ in 0..32 {
                    // Probes biased onto the edges, where an off-by-one lives.
                    let ipa = match m.get(r.below(m.len().max(1) as u64) as usize) {
                        Some(&(s, l)) => [s, s + l as u64, s + l as u64 - 1, s.saturating_sub(1)][r.below(4) as usize],
                        None => r.below(4096 * G),
                    };
                    let len = [0usize, 1, 8, G as usize, 3 * G as usize][r.below(5) as usize];
                    let (got, want) = (idx.holding(ipa, len), lin_holding(&m, ipa, len));
                    // With len 0 two backings can both hold a span where one ends and the next
                    // begins; the scan took the first in Vec order, the index the one starting
                    // there. Either way `read_guest` returns an empty Vec, so only presence
                    // matters at len 0.
                    if len == 0 { assert_eq!(got.is_some(), want.is_some(), "seed {seed}: holding({ipa:#x}, 0)"); }
                    else { assert_eq!(got, want, "seed {seed}: holding({ipa:#x}, {len})"); }
                    assert_eq!(idx.containing(ipa), lin_containing(&m, ipa), "seed {seed}: containing({ipa:#x})");
                }
            }
        }
    }

    #[test]
    fn a_zero_length_read_at_a_backings_end_is_held_as_the_scan_held_it() {
        let mut idx = SpanIndex::default();
        idx.insert(0x4000, 0x4000, 0);
        assert_eq!(idx.holding(0x8000, 0), Some(0), "the scan accepted ipa == end for len 0");
        assert_eq!(idx.holding(0x8000, 1), None);
        assert_eq!(idx.containing(0x8000), None, "host_span's test is strict");
    }

    #[test]
    #[should_panic(expected = "overlaps")]
    fn an_overlapping_insert_fails_loud() {
        let mut idx = SpanIndex::default();
        idx.insert(0x8000, 0x8000, 0);
        idx.insert(0xc000, 0x4000, 1);
    }
}
```

Add `mod backings;` beside the other `mod` lines at the top of `lib.rs`. `use crate::Backing` compiles unused in the stub; that's fine for this step's red run, and it is used in Step 3.

- [ ] **Step 2: Run the tests and watch them fail**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
cargo test -p retrace-box --lib backings --no-fail-fast -- --test-threads=1 > $L/t6-red.log 2>&1; echo "exit=$?"
```

Expected: exit 101, with all three failing (the stub answers `None` and never panics).

- [ ] **Step 3: Implement `SpanIndex` and `Backings`**

Replace the stub `impl SpanIndex` with:

```rust
impl SpanIndex {
    /// Index the span at Vec position `pos`. Panics if it overlaps a span already indexed.
    pub(crate) fn insert(&mut self, start: u64, len: usize, pos: usize) {
        let i = self.e.partition_point(|&(s, _, _)| s < start);
        if let Some(&(ps, pl, _)) = i.checked_sub(1).map(|j| &self.e[j]) {
            assert!(ps + pl as u64 <= start, "backing {start:#x}+{len:#x} overlaps {ps:#x}+{pl:#x}");
        }
        if let Some(&(ns, nl, _)) = self.e.get(i) {
            assert!(start + len as u64 <= ns, "backing {start:#x}+{len:#x} overlaps {ns:#x}+{nl:#x}");
        }
        self.e.insert(i, (start, len, pos));
    }
    /// Forget Vec position `pos`; every later position shifts down by one, as `Vec::remove` does.
    pub(crate) fn remove(&mut self, pos: usize) {
        let i = self.e.iter().position(|&(_, _, p)| p == pos).expect("an indexed position");
        self.e.remove(i);
        for x in &mut self.e { if x.2 > pos { x.2 -= 1; } }
    }
    /// The backing holding all of `[ipa, ipa + len)` — `read_guest`'s test, `ipa >= start &&
    /// ipa + len <= end`. With `len == 0` that admits `ipa == end`, as the scan did. `checked_add`
    /// answers `None` where the scan's `ipa + len` would have overflowed.
    pub(crate) fn holding(&self, ipa: u64, len: usize) -> Option<usize> {
        let j = self.e.partition_point(|&(s, _, _)| s <= ipa).checked_sub(1)?;
        let (s, l, p) = self.e[j];
        ipa.checked_add(len as u64).is_some_and(|end| end <= s + l as u64).then_some(p)
    }
    /// The backing whose span contains the address `ipa` — `host_span`'s strict test.
    pub(crate) fn containing(&self, ipa: u64) -> Option<usize> {
        let j = self.e.partition_point(|&(s, _, _)| s <= ipa).checked_sub(1)?;
        let (s, l, p) = self.e[j];
        (ipa < s + l as u64).then_some(p)
    }
}

/// The Vec of backings and its index. Derefs to `[Backing]` for every read-only use; every
/// mutation is a method here, so the index cannot fall behind the Vec.
pub(crate) struct Backings { v: Vec<Backing>, idx: SpanIndex }

impl Backings {
    pub(crate) fn new() -> Self { Backings { v: Vec::new(), idx: SpanIndex::default() } }
    pub(crate) fn push(&mut self, b: Backing) {
        self.idx.insert(b.ipa, b.len, self.v.len());
        self.v.push(b);
    }
    pub(crate) fn extend(&mut self, it: impl IntoIterator<Item = Backing>) { for b in it { self.push(b); } }
    pub(crate) fn remove(&mut self, pos: usize) -> Backing {
        self.idx.remove(pos);
        self.v.remove(pos)
    }
    pub(crate) fn holding(&self, ipa: u64, len: usize) -> Option<&Backing> { self.idx.holding(ipa, len).map(|p| &self.v[p]) }
    pub(crate) fn containing(&self, ipa: u64) -> Option<&Backing> { self.idx.containing(ipa).map(|p| &self.v[p]) }
}

impl std::ops::Deref for Backings {
    type Target = [Backing];
    fn deref(&self) -> &[Backing] { &self.v }
}
```

Run the three tests: `cargo test -p retrace-box --lib backings -- --test-threads=1`. Expected: `3 passed`.

- [ ] **Step 4: Switch `Box_` to `Backings`, and let the compiler find every site**

In `lib.rs`:
- add `use backings::Backings;`;
- change the field to `backings: Backings,`;
- change each constructor's `let mut backings = Vec::new();` to `Backings::new()`;
- change each `map` closure's `backings: &mut Vec<Backing>` and `build_tables`' `backings: &mut Vec<Backing>` to `&mut Backings`.

`promote_and_set(…, backings: &[Backing], …)` stays as it is, since `&Backings` and `&mut Backings` coerce to `&[Backing]`. Then:

```bash
cargo build -p retrace-box > $L/t6-build.log 2>&1; echo "exit=$?"
grep -a -E '^error' -A6 $L/t6-build.log | head -60
```

Fix each error by using the `Backings` method of the same name. `push`, `extend` and `remove` keep their names, and reads go through `Deref`. **If an error needs `DerefMut`, or a Vec method that mutates** (`retain`, `iter_mut`, `sort`, `drain`, `clear`), stop and report: that site mutates backings in a way the index does not model.

- [ ] **Step 5: Route the four hot lookups through the index**

```rust
    pub fn read_guest(&self, ipa: u64, len: usize) -> Vec<u8> {
        if let Some(bk) = self.backings.holding(ipa, len) {
            let off = (ipa - bk.ipa) as usize;
            return unsafe { std::slice::from_raw_parts(bk.host.add(off), len) }.to_vec();
        }
        panic!("read_guest: ipa 0x{ipa:x} len {len} not mapped");
    }
```

`read_guest_checked`: the same body with `Some(…)` and `None`. `host_span`:

```rust
    fn host_span(&self, ipa: u64) -> Option<(*mut u8, usize)> {
        self.backings.containing(ipa).map(|bk| {
            let off = (ipa - bk.ipa) as usize;
            (unsafe { bk.host.add(off) }, bk.len - off)
        })
    }
```

`backing_of`: `self.backings.containing(ipa).map(|bk| (bk.ipa, bk.len))`. Keep each function's existing doc comment, and add one line to it: `M44 B1: by the index beside the Vec (backings.rs).`

- [ ] **Step 6: The box suite, the e2e stepping suites, clippy, commit** (controller-run for the e2e part)

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 > $L/t6-box.log 2>&1; echo "exit=$?"
for t in hitorder_e2e gdbserver_e2e llsc_e2e reverse_debug_e2e watchsweep_e2e; do cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t6-e2e-$t.log 2>&1; echo "$t exit=$?"; done
cargo clippy -p retrace-box --all-targets -- -D warnings > $L/t6-clippy.log 2>&1; echo "exit=$?"
grep -a -c 'overlaps' $L/t6-box.log $L/t6-e2e-*.log
```

Expected: every `exit=0`, and the overlap count is 0 in every log. **An overlap panic anywhere is halt 1**: the invariant this index rests on was false.

```bash
git add crates/retrace-box
git commit -m "M44 B1: backing lookups by a sorted index beside the Vec; spans asserted disjoint"
```

- [ ] **Step 7: The CPU bar** (controller-run, with t0 M5's `cpu.sh`)

Run `cpu.sh` on the worktree for `hitorder_e2e oracle_threadrust_breakpoints_at_both_switches`, three runs. The pass bar comes from t0 M5(i), with medians `T0` (`c652cf1`) and `T1` (`64e471e`): **the branch's median user time must be ≤ T1 − (T1 − T0)/2**. Record all nine numbers in the report. **A miss is routed, not halted:** the index stays (it is correct and faster), and the report names what else the pre-decode costs. Profile it first, e.g. `sample` or `xctrace` on the test binary, before naming anything.

- [ ] **Step 8: Control, on the committed tree**

In `holding`, change `<=` in `end <= s + l as u64` to `<`. `the_index_answers_every_probe_as_the_linear_scan_did` must fail. Restore with `git checkout -- crates/retrace-box/src/backings.rs`.

---

### Task 7 (B2): `disarm-rsi`

**Files:**
- Modify: `crates/retrace/src/gdbserver.rs` (`monitor`, ~412)
- Modify: `crates/retrace/lldb/retrace.py`
- Modify: `crates/retrace/tests/gdbserver_e2e.rs`

- [ ] **Step 1: Write the failing row**

In `gdbserver_e2e.rs`, after `a_reverse_step_moves_back_one_and_stops_at_the_start`:

```rust
#[test]
fn disarm_rsi_makes_the_next_bc_a_reverse_continue_again() {
    // M44 B2: what `rsi` sends when `ContinueInDirection` fails after `arm-rsi` succeeded. Armed, the
    // next `bc` is one step back; disarmed, it is a reverse continue — here, to the start. Both
    // monitor commands are idempotent: an `E` reply would make `rsi`'s own cleanup fail.
    let mut c = Rsp::spawn(watchsweep(), &[]);
    let entry = retrace_core::seek(watchsweep(), 1, 0).unwrap().pc();
    let cmd = |s: &str| format!("qRcmd,{}", hexs(s.as_bytes()));
    for _ in 0..3 { assert!(c.send("s").contains("reason:trace;")); }
    assert_eq!(c.send_collect(&cmd("disarm-rsi")).1, "OK", "disarming an unarmed server is not an error");
    assert_eq!(c.send_collect(&cmd("arm-rsi")).1, "OK");
    assert_eq!(c.send_collect(&cmd("arm-rsi")).1, "OK", "arming an armed server is not an error");
    assert_eq!(c.send_collect(&cmd("disarm-rsi")).1, "OK");
    let b = c.send("bc");
    assert_eq!(r::description(&b).as_deref(), Some("start of recording"), "disarmed: a reverse continue: {b}");
    assert_eq!(pc_of(&b), entry, "not entry + 8, where an armed bc would have stopped");
}
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
cargo test -p retrace --test gdbserver_e2e disarm_rsi --no-fail-fast -- --test-threads=1 > $L/t7-red.log 2>&1; echo "exit=$?"
```

Expected: exit 101, because `disarm-rsi` answers `E01`.

- [ ] **Step 2: Implement both halves**

In `monitor`, after the `"arm-rsi"` arm:

```rust
            // M44 B2: what `rsi` sends when `ContinueInDirection` fails after `arm-rsi` succeeded,
            // so the next `process continue -R` is a reverse continue again, not one step back.
            // Idempotent, like `arm-rsi`.
            "disarm-rsi" => { self.rsi_armed = false; (vec!["OK".into()], false) }
```

In `retrace.py`, replace the failure branch after `ContinueInDirection`:

```python
    err = process.ContinueInDirection(lldb.eRunReverse)
    if not err.Success():
        # M44 B2: the server is still armed. Disarm it, or the next `process continue -R` is one
        # instruction back instead of a reverse continue.
        ci.HandleCommand("process plugin packet monitor disarm-rsi", lldb.SBCommandReturnObject())
        result.SetError("retrace rsi: " + str(err))
        return
```

Update the module docstring's paragraph to mention `disarm-rsi`.

- [ ] **Step 3: Run it; run the rest of both targets; commit**

```bash
cargo test -p retrace --test gdbserver_e2e --no-fail-fast -- --test-threads=1 > $L/t7-gdbserver.log 2>&1; echo "exit=$?"
cargo test -p retrace --test lldb_e2e --no-fail-fast -- --test-threads=1 > $L/t7-lldb.log 2>&1; echo "exit=$?"
cargo clippy -p retrace --all-targets -- -D warnings > $L/t7-clippy.log 2>&1; echo "exit=$?"
git add crates/retrace/src/gdbserver.rs crates/retrace/lldb/retrace.py crates/retrace/tests/gdbserver_e2e.rs
git commit -m "M44 B2: monitor disarm-rsi; rsi disarms the server when its reverse resume fails"
```

Expected: every `exit=0`. The `retrace.py` failure branch has no automated test: a failing reverse resume is not reachable, since the server never sends the plain-signal stops that make one fail (M43 L4d). Say so in the report.

- [ ] **Step 4: Control, on the committed tree**

Make the `"disarm-rsi"` arm leave `rsi_armed` unchanged. The row must fail at the final `pc_of` (it lands at entry + 8). Restore with `git checkout -- crates/retrace/src/gdbserver.rs`.

---

### Task 8 (B3): a step stops at its own thread's exit

**Files:**
- Modify: `crates/retrace/src/debug.rs` (`Halt`, ~323; `step_thread`'s `Advance::Event` branch, ~983)
- Modify: `crates/retrace/src/gdbserver.rs` (`step`, ~265)
- Modify: `crates/retrace/tests/gdbserver_e2e.rs`, `crates/retrace/tests/lldb_e2e.rs`

**Interfaces:**
- Produces: `Halt::ThreadExited { thread: u32 }`.

- [ ] **Step 1: Write the failing wire row**

In `gdbserver_e2e.rs`, after `a_step_on_a_thread_that_is_not_running_is_refused_in_place`:

```rust
/// threadrust's child: the landmark of its own `bsdthread_terminate` and the child's thread.
fn threadrust_child_exit() -> (&'static Path, usize, u32) {
    let (tr, _, _) = r::threadrust_block();
    let ev = retrace_trace::Reader::open(tr).unwrap();
    let (x, child) = (1..ev.len()).find_map(|i| match ev[i] {
        retrace_trace::Event::Syscall { num, thread, .. } if num == retrace_arch::SYS_BSDTHREAD_TERMINATE => Some((i, thread)),
        _ => None,
    }).expect("the child's bsdthread_terminate");
    (tr, x, child)
}

#[test]
fn a_step_across_the_stepped_threads_own_exit_stops_there() {
    // M44 B3: the child's `bsdthread_terminate` is its last trap. Before M44 the step ran on until
    // the child was current again — never — and so to the end of the recording. The stop is named
    // on the thread now running: the exited one has no context left to name.
    let (tr, x, child) = threadrust_child_exit();
    let svc = r::trap_pc(tr, x);
    let mut c = Rsp::spawn(tr, &[]);
    assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
    let (_, at) = r::continue_to_window(&mut c, x);
    assert_eq!(r::key(&at, "thread"), Some(format!("{:x}", child + 1).as_str()), "{at}");
    assert_eq!(c.send(&format!("z0,{svc:x},4")), "OK");
    let s = c.send(&format!("vCont;s:{:x}", child + 1));
    assert!(s.contains("reason:exception;"), "not trace, and not the end of the recording: {s}");
    assert!(!s.contains("replaylog:end;"), "{s}");
    assert!(r::description(&s).unwrap().contains(&format!("thread {} exited during the step", child + 1)), "{s}");
    assert!(c.where_().starts_with(&format!("at ({}, 0)", x + 1)), "at the exit's boundary: {}", c.where_());
}
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
cargo test -p retrace --test gdbserver_e2e a_step_across_the_stepped --no-fail-fast -- --test-threads=1 > $L/t8-red.log 2>&1; echo "exit=$?"
```

Expected: exit 101, with the stop at the end of the recording (a terminal), not an exception naming the exit. If `SYS_BSDTHREAD_TERMINATE` is not the child's terminal landmark in the trace (t0 M5(iii) shape C recorded which it is), use the number t0 found.

- [ ] **Step 2: Implement**

`debug.rs`: add to `Halt`, after `Refused`:

```rust
    /// M44 B3: `step_thread`'s thread exited in the step's crossing, parked at (n, 0, Bp). Nothing
    /// runs it again, so the step ends at that boundary instead of at the end of the recording.
    ThreadExited { thread: u32 },
```

Import `ThreadState` in `debug.rs`'s `use retrace_core::{…}` line, the way `gdbserver.rs:9` does. In `step_thread`'s `Advance::Event` branch, before the `// `t` blocked.` comment:

```rust
                        // M44 B3: or `t` exited, crossing its own exit. Nothing will run it again, so
                        // the until-run below would reach the end of the recording; stop here.
                        let exited = self.sess().thread_summaries().iter()
                            .any(|s| s.tid == t && matches!(s.state, ThreadState::Exited(_)));
                        if exited {
                            (self.n, self.k, self.phase) = (n, 0, Phase::Bp);
                            return Ok(Halt::ThreadExited { thread: t });
                        }
```

`gdbserver.rs`, in `step`'s match, before `other => s.reply_forward(other),`:

```rust
                // M44 B3: the stepped thread is gone, so the stop cannot name it (`stop` looks every
                // named thread up in the table): named on the running thread, saying why.
                Halt::ThreadExited { thread } => Ok(s.stop(StopKind::Exception { signal: 5,
                    text: format!("thread {} exited during the step", thread + 1) }, None)),
```

If the compiler reports another exhaustive `match` on `Halt`, route `ThreadExited` there to the same stop. `reply_forward`'s catch-all must not see it.

- [ ] **Step 3: Run the wire row, then the lldb loop check**

```bash
cargo test -p retrace --test gdbserver_e2e --no-fail-fast -- --test-threads=1 > $L/t8-gdbserver.log 2>&1; echo "exit=$?"
```

Expected: every row passes, including the new one.

Add the lldb row in `lldb_e2e.rs`, after `lldb_steps_a_blocked_thread_…`:

```rust
#[test]
fn lldb_steps_a_thread_across_its_own_exit_and_stops_there() {
    // M44 B3 in lldb itself, and the loop check the spec requires before this form is kept: M43
    // measured lldb re-stepping forever when a step was answered on another thread. The session is
    // bounded (BOUND), so a loop fails here as a killed session with no END.
    if !lldb_runs() {
        util::announce("SKIPPED lldb_steps_a_thread_across_its_own_exit…: `/usr/bin/lldb --version` did not run. This gate did NOT run.");
        return;
    }
    let (tr, _, _) = util::rsp::threadrust_block();
    let ev = retrace_trace::Reader::open(tr).unwrap();
    let (x, child) = (1..ev.len()).find_map(|i| match ev[i] {
        retrace_trace::Event::Syscall { num, thread, .. } if num == retrace_arch::SYS_BSDTHREAD_TERMINATE => Some((i, thread)),
        _ => None,
    }).expect("the child's bsdthread_terminate");
    let svc = util::rsp::trap_pc(tr, x);
    let m = {
        let mut c = util::rsp::Rsp::spawn(tr, &[]);
        assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
        util::rsp::continue_to_window(&mut c, x).0
    };
    let mut cmds = to_svc(svc, m);
    cmds.extend([format!("thread select {}", child + 1), "thread step-inst".into(), "thread list".into()]);
    let (code, out, err) = session(tr, &cmds);
    let t = format!("exit {code:?}\n--- stdout\n{out}\n--- stderr\n{err}");
    assert!(out.lines().any(|l| l.trim() == "END"), "the batch ran to its end — no re-step loop: {t}");
    assert_eq!(code, Some(0), "{t}");
    assert!(out.contains(&format!("thread {} exited during the step", child + 1)), "{t}");
}
```

```bash
cargo test -p retrace --test lldb_e2e --no-fail-fast -- --test-threads=1 > $L/t8-lldb.log 2>&1; echo "exit=$?"
```

**If the new lldb row fails because the session hit `BOUND` without `END`, lldb loops on this form.** Commit nothing from this task. Restore with `git checkout -- crates/retrace`, and **route** B3: its report names the loop (packet count per t0 M5(iii)'s method) and the successor. Otherwise continue.

- [ ] **Step 4: Clippy, commit, control**

```bash
cargo clippy -p retrace --all-targets -- -D warnings > $L/t8-clippy.log 2>&1; echo "exit=$?"
git add crates/retrace
git commit -m "M44 B3: a step that crosses its own thread's exit stops at that boundary, named on the running thread"
```

Control: delete the `if exited { … }` block. The wire row must fail with the stop at the end of the recording. Restore with `git checkout -- crates/retrace/src/debug.rs`.

---

### Task 9 (B4): lldb rows for `ni`, `next`, `thread step-out` and `finish`

**Files:**
- Modify: `crates/retrace/tests/lldb_e2e.rs`

**Interfaces:** Consumes t0 M5(iv) (what `next` does at a `bl` with no line table).

- [ ] **Step 1: A helper for the first `bl` in `crashy`'s `main`, from symbols and the recording**

In `lldb_e2e.rs`:

```rust
/// The first `bl` in `crashy`'s `_main` (its `fstat` call), from the fixture's symbols and the
/// recording's own bytes: `EXE_BASE` is `crashy`'s `__TEXT` vmaddr, so `nm`'s address is the guest
/// pc. Never hard-coded.
fn crashy_first_bl(trace: &Path) -> u64 {
    let out = Command::new("nm").arg(retrace_guest::CRASHY).output().expect("nm");
    let main = String::from_utf8(out.stdout).unwrap().lines().find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.len() == 3 && f[2] == "_main").then(|| u64::from_str_radix(f[0], 16).unwrap())
    }).expect("_main in crashy");
    let s = retrace_core::seek(trace, 1, 0).unwrap();
    let code = s.read_mem(main, 256).expect("main's text is in the recording");
    let i = code.chunks(4).position(|w| u32::from_le_bytes(w.try_into().unwrap()) & 0xfc00_0000 == 0x9400_0000)
        .expect("a bl in main's first 64 instructions");
    main + 4 * i as u64
}
```

- [ ] **Step 2: Write the rows**

```rust
#[test]
fn lldb_steps_over_a_call_with_ni_and_next() {
    // M44 B4 (M43 F-3): lldb's step-over inserts one transient Z0 at the return address (t0 L5).
    // `ni` is instruction-level; `next` without a line table does what t0 M5(iv) measured.
    if !lldb_runs() {
        util::announce("SKIPPED lldb_steps_over_a_call…: `/usr/bin/lldb --version` did not run. This gate did NOT run.");
        return;
    }
    let tr = crashy_trace();
    let bl = crashy_first_bl(&tr);
    for (step, want) in [("thread step-inst-over", bl + 4), ("next", <t0 M5(iv)'s pc, as an expression of bl>)] {
        let cmds = vec![format!("breakpoint set -a {bl:#x}"), "process continue".into(), step.into(), "register read pc".into()];
        let (code, out, err) = session(&tr, &cmds);
        let t = format!("{step}: exit {code:?}\n--- stdout\n{out}\n--- stderr\n{err}");
        assert!(out.lines().any(|l| l.trim() == "END"), "{t}");
        assert_eq!(code, Some(0), "{t}");
        assert_eq!(values(&out, "pc = ").last().copied(), Some(want), "{t}");
    }
}

#[test]
fn lldb_steps_out_of_a_call_with_step_out_and_finish() {
    // M44 B4: one `thread step-inst` into the `bl`'s stub, then out again: lldb unwinds to the
    // caller and stops at the return address, the `bl`'s pc + 4.
    if !lldb_runs() {
        util::announce("SKIPPED lldb_steps_out_of_a_call…: `/usr/bin/lldb --version` did not run. This gate did NOT run.");
        return;
    }
    let tr = crashy_trace();
    let bl = crashy_first_bl(&tr);
    for out_cmd in ["thread step-out", "finish"] {
        let cmds = vec![format!("breakpoint set -a {bl:#x}"), "process continue".into(),
                        "thread step-inst".into(), out_cmd.into(), "register read pc".into()];
        let (code, out, err) = session(&tr, &cmds);
        let t = format!("{out_cmd}: exit {code:?}\n--- stdout\n{out}\n--- stderr\n{err}");
        assert!(out.lines().any(|l| l.trim() == "END"), "{t}");
        assert_eq!(code, Some(0), "{t}");
        assert_eq!(values(&out, "pc = ").last().copied(), Some(bl + 4), "{t}");
    }
}
```

Replace `<t0 M5(iv)'s pc, as an expression of bl>` with what M5(iv) measured, written relative to `bl`. **If M5(iv) found `next` does not stop at `bl + 4`**, the expected value is lldb's own behaviour, not a server bug; cite the measurement in the comment. **If `values(&out, "pc = ")` does not parse `register read pc`'s format**, look at `crashy_script`'s existing use in this file and match it.

- [ ] **Step 3: Run; fix or route**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
cargo test -p retrace --test lldb_e2e lldb_steps_ --no-fail-fast -- --test-threads=1 > $L/t9-lldb.log 2>&1; echo "exit=$?"
```

These rows test existing server behaviour, so they may pass on first run. That is the expected outcome, and a row passing first is **not** a red-green violation here: the task adds coverage, not behaviour. If one fails, read the server's stderr in the failure text. A small server bug (a mis-reported stop or a `Z0` mishandled) is fixed in `gdbserver.rs` with its own `gdbserver_e2e` row first. A larger one is routed, with the row committed `#[ignore]`d at the measured wall.

- [ ] **Step 4: Clippy, commit, control**

```bash
cargo clippy -p retrace --all-targets -- -D warnings > $L/t9-clippy.log 2>&1; echo "exit=$?"
git add crates/retrace/tests/lldb_e2e.rs
git commit -m "M44 B4: lldb rows for ni, next, thread step-out and finish over crashy's first call"
```

Control: change `bl + 4` in the step-out row to `bl + 8`. The row must fail showing lldb's actual pc. Restore with `git checkout -- crates/retrace/tests/lldb_e2e.rs`.

---

### Task 10 (B5): `bt` depth on an arm64e guest

**Files:**
- Create: `crates/retrace-guest/asm/btchain.s`
- Modify: `crates/retrace-guest/build.rs` (after `strip47`'s block, ~427), `crates/retrace-guest/src/lib.rs` (after `STRIP47`, ~174)
- Modify: `crates/retrace/tests/lldb_e2e.rs` (`session` gains server args; one row)
- Possibly modify: `crates/retrace/src/gdbserver.rs:15` (`QHOSTINFO`)

- [ ] **Step 1: The fixture**

`crates/retrace-guest/asm/btchain.s`:

```asm
.section __TEXT,__text
.global _start
.p2align 2
// M44 B5: an arm64e call chain for lldb's `bt` over gdb-remote. Each of f1..f3 signs its return
// address with paciasp and saves it in a frame record, so every saved LR on the stack — and x30
// itself in f3 — carries a PAC signature lldb's unwinder must strip. f3 then stores to the M6
// GARBAGE_VA (asm/crash.s), a stage-1 fault recorded as the terminal Event::Crash, leaving
// _start -> f1 -> f2 -> f3 on the stack. Static and freestanding like strip47: the main
// executable is arm64e, so the PAC posture is on (M7 Task 6).
_start:
    mov  x29, #0                 // the frame chain ends here
    bl   _f1
    mov  x0, #0
    mov  x16, #1                 // SYS_exit (unreached)
    svc  #0x80
.global _f1
_f1:
    paciasp
    stp  x29, x30, [sp, #-16]!
    mov  x29, sp
    bl   _f2
    ldp  x29, x30, [sp], #16
    autiasp
    ret
.global _f2
_f2:
    paciasp
    stp  x29, x30, [sp, #-16]!
    mov  x29, sp
    bl   _f3
    ldp  x29, x30, [sp], #16
    autiasp
    ret
.global _f3
_f3:
    paciasp
    stp  x29, x30, [sp, #-16]!
    mov  x29, sp
    movz x0, #0x4000, lsl #32    // 0x4000_0000_0000
    movk x0, #0xDEAD, lsl #16    // | 0xDEAD_0000
    mov  w1, #0x2A
    strb w1, [x0]                // stage-1 fault -> Stop::Fault (never retires)
    ldp  x29, x30, [sp], #16
    autiasp
    ret
```

In `build.rs`, after `strip47`'s block, copy it with `btchain`: the same `-arch arm64e -nostdlib -static -Wl,-e,_start` arguments and a comment naming M44 B5. In `src/lib.rs`:

```rust
/// M44 B5: arm64e `_start -> f1 -> f2 -> f3`, each frame's LR PAC-signed, crashing in f3.
pub const BTCHAIN: &str = concat!(env!("OUT_DIR"), "/btchain");
```

Check it:

```bash
cargo build -p retrace-guest > $L/t10-build.log 2>&1; echo "exit=$?"
otool -hv $(ls -d target/aarch64-apple-darwin/debug/build/retrace-guest-*/out/btchain | head -1) | grep -a 'ARM64'
```

Expected: `ARM64    E`, the arm64e subtype, as `strip47` reports.

- [ ] **Step 2: Let `session` pass server args, then write the row**

In `lldb_e2e.rs`, rename `session` to `session_with(trace, server_args: &[&str], cmds)`, passing `server_args` to `spawn_server`. Add `fn session(trace, cmds) { session_with(trace, &[], cmds) }` so existing callers are unchanged.

```rust
#[test]
fn lldb_backtraces_an_arm64e_guest_through_signed_return_addresses() {
    // M44 B5 (M43 F-3): only frame #0 was ever measured (crashy, arm64). Here every saved LR is
    // PAC-signed; an unwinder that does not strip stops at frame #0 or #1.
    if !lldb_runs() {
        util::announce("SKIPPED lldb_backtraces_an_arm64e_guest…: `/usr/bin/lldb --version` did not run. This gate did NOT run.");
        return;
    }
    let (rec, tr) = util::record(retrace_guest::BTCHAIN);
    assert_eq!(rec.code, 139, "record btchain: {}", rec.stderr);
    let cmds = vec!["process continue".into(), "bt".into()];
    let (code, out, err) = session_with(&tr, &["--exe", retrace_guest::BTCHAIN], &cmds);
    let t = format!("exit {code:?}\n--- stdout\n{out}\n--- stderr\n{err}");
    assert!(out.lines().any(|l| l.trim() == "END"), "{t}");
    let frames: Vec<&str> = out.lines().filter(|l| l.contains("frame #")).collect();
    assert!(frames.len() >= 3, "bt shows f3, f2 and f1 at least: {t}");
    for (i, f) in ["f3", "f2", "f1"].iter().enumerate() {
        assert!(frames[i].contains(&format!("`{f}")), "frame #{i} is {f} — a signed LR stripped: {t}");
    }
}
```

- [ ] **Step 3: Measure without, then with, `addressing_bits`**

```bash
cargo test -p retrace --test lldb_e2e lldb_backtraces --no-fail-fast -- --test-threads=1 > $L/t10-without.log 2>&1; echo "exit=$?"
```

If it passes, `QHOSTINFO` needs no change. Record that lldb strips without the hint, and go to Step 4. If it fails, append `addressing_bits:47;` to `QHOSTINFO` in `gdbserver.rs:15`, with a comment:

```rust
// M44 B5: `addressing_bits:47` — the guest VA width (T0SZ = 17). Without it lldb cannot strip a
// PAC-signed saved LR and an arm64e `bt` stops early (measured: <t10-without.log's frame count>).
```

Grep `gdbserver_e2e.rs` for an exact `qHostInfo` assertion, update it, and re-run:

```bash
grep -n 'qHostInfo\|cputype:16777228' crates/retrace/tests/gdbserver_e2e.rs
cargo test -p retrace --test lldb_e2e --no-fail-fast -- --test-threads=1 > $L/t10-with.log 2>&1; echo "exit=$?"
cargo test -p retrace --test gdbserver_e2e --no-fail-fast -- --test-threads=1 > $L/t10-gdbserver.log 2>&1; echo "exit=$?"
```

If it still fails, **route**: commit the row `#[ignore = "M44 wall: …the frame count and lldb's output…"]` and revert `QHOSTINFO`.

- [ ] **Step 4: Clippy, commit, control**

```bash
cargo clippy --workspace --all-targets -- -D warnings > $L/t10-clippy.log 2>&1; echo "exit=$?"
git add crates/retrace-guest crates/retrace
git commit -m "M44 B5: an arm64e call-chain fixture and lldb's bt through its signed return addresses"
```

Control (only if `QHOSTINFO` changed): remove `addressing_bits:47;`. The row must fail. Restore with `git checkout -- crates/retrace/src/gdbserver.rs`.

---

### Task 11 (B6a): another thread's hit ends a blocked step

**Files:**
- Modify: `crates/retrace/src/debug.rs` (`Halt`; `continue_until` ~748; `step_thread` ~983; a new helper `run_until_thread`)
- Modify: `crates/retrace/src/gdbserver.rs` (`step`)
- Modify: `crates/retrace/tests/gdbserver_e2e.rs` (rewrite `a_blocked_step_runs_past_another_threads_breakpoint_to_the_stepped_thread`)
- Modify: `crates/retrace/tests/lldb_e2e.rs` (rewrite session A of `lldb_steps_a_blocked_thread_…`)

**Interfaces:**
- Produces: `Halt::StepInterrupted { thread: u32, by: u32, what: String }` and `fn run_until_thread(&mut self, t: u32, out) -> Result<Halt, String>`. Task 12 calls the latter.

- [ ] **Step 1: Rewrite the wire row to the new behaviour (the failing test)**

Replace `a_blocked_step_runs_past_another_threads_breakpoint_to_the_stepped_thread` with:

```rust
#[test]
fn a_blocked_step_stops_at_another_threads_breakpoint_on_the_stepped_thread() {
    // M44 B6(a), replacing M43's R7 fallback row: during a blocked step, another thread's hit ends
    // the step. The stop is named on the STEPPED thread with reason exception (t0 L7's
    // measured-safe form) — M43 measured lldb looping forever (307,016 × `vCont;s:1` in 60 s) when
    // the same hit was reported as `reason:breakpoint` on the other thread.
    let (tr, n, t) = r::threadrust_block();
    let svc = r::trap_pc(tr, n);
    let b = retrace_core::seek(tr, n + 1, 0).unwrap().pc();
    let mut c = Rsp::spawn(tr, &[]);
    assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
    r::continue_to_window(&mut c, n);
    assert_eq!(c.send(&format!("z0,{svc:x},4")), "OK");
    assert_eq!(c.send(&format!("Z0,{b:x},4")), "OK");
    let s = c.send(&format!("vCont;s:{:x}", t + 1));
    assert!(s.contains("reason:exception;"), "{s}");
    assert_eq!(r::key(&s, "thread"), Some(format!("{:x}", t + 1).as_str()), "on the stepped thread: {s}");
    let d = r::description(&s).unwrap();
    assert!(d.contains(&format!("breakpoint at {b:#x}")), "names the other thread's hit: {d}");
    assert!(c.where_().starts_with(&format!("at ({}, 0)", n + 1)), "parked at the hit: {}", c.where_());
}
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
cargo test -p retrace --test gdbserver_e2e a_blocked_step_stops --no-fail-fast -- --test-threads=1 > $L/t11-red.log 2>&1; echo "exit=$?"
```

Expected: exit 101, with `reason:trace` at `svc + 4` (today's fallback).

- [ ] **Step 2: Implement**

`debug.rs`, `Halt`, after `ThreadExited`:

```rust
    /// M44 B6(a): a step of `thread` that blocked was ended by another thread's hit (`by`), parked
    /// at that hit. `what` names it for the stop's description.
    StepInterrupted { thread: u32, by: u32, what: String },
```

In `continue_until`, arm the user's hits for an until-run as well. Replace the `let (bps, ws) = match until { … };` with:

```rust
        // M44 B6(a): an until-run arms the user's hits too; `run_until_thread` turns a hit into
        // `StepInterrupted`. (M43's R7 fallback armed nothing, because the hit was reported on the
        // other thread and lldb looped on that; t0 L7's form names the stepped thread instead.)
        let (bps, ws): (Vec<u64>, Vec<(u64, u64)>) =
            (self.breakpoints.clone(), self.watches.iter().map(|&(a, l, _)| (a, l)).collect());
```

`until` is still read further down, by the finish's arrival check (`if let Some(t) = until`), so nothing else in the function changes. Then add after `step_thread`:

```rust
    /// Run until thread `t` is current again (M43 §3d's until-run), with the user's hits armed. A
    /// hit on the way ends the run as `StepInterrupted` on `t` (M44 B6(a)).
    fn run_until_thread<W: Write>(&mut self, t: u32, out: &mut W) -> Result<Halt, String> {
        Ok(match self.continue_until(Some(t), out)? {
            Halt::Break => {
                let (by, pc) = (self.sess().current_thread(), self.sess().pc());
                Halt::StepInterrupted { thread: t, by, what: format!("breakpoint at {pc:#x}") }
            }
            Halt::Watch { watched } => {
                let by = self.sess().current_thread();
                Halt::StepInterrupted { thread: t, by, what: format!("a store to watched {watched:#x}") }
            }
            Halt::WatchSys { watched, thread: by } => {
                Halt::StepInterrupted { thread: t, by, what: format!("a syscall write to watched {watched:#x}") }
            }
            other => other,
        })
    }
```

In `step_thread`, replace `self.continue_until(Some(t), out)` with `self.run_until_thread(t, out)`.

`gdbserver.rs`, `step`'s match, beside `ThreadExited`:

```rust
                // M44 B6(a): named on the stepped thread (t0 L7's measured-safe form), saying which
                // thread hit what; the cursor is parked at that hit.
                Halt::StepInterrupted { thread, by, what } => Ok(s.stop(StopKind::Exception { signal: 5,
                    text: format!("thread {} hit {what} during thread {}'s step", by + 1, thread + 1) }, Some(thread))),
```

`stop(…, Some(thread))` names a thread that is blocked but live, so `thread_ctx` has it.

- [ ] **Step 3: Run the wire suite, then rewrite lldb session A as the loop check**

```bash
cargo test -p retrace --test gdbserver_e2e --no-fail-fast -- --test-threads=1 > $L/t11-gdbserver.log 2>&1; echo "exit=$?"
```

Expected: every row passes. If another row fails, it pinned the fallback too. **Halt and ask** rather than rewrite a fourth row: the spec names three.

In `lldb_e2e.rs`, `lldb_steps_a_blocked_thread_to_where_it_resumes_and_refuses_one_that_is_not_running`: rename it `lldb_steps_a_blocked_thread_into_another_threads_breakpoint_and_one_that_is_not_running`. Replace its first comment with the M44 B6 story, and change session A's assertions:

```rust
    // Session A (M44 B6(a)): the step blocks, and the other thread's breakpoint at `b` ends it:
    // stopped on the stepped thread, reason exception, naming the hit — and lldb does not re-step.
    let mut a = to_svc(svc, m);
    a.extend([where_(), format!("breakpoint set -a {b:#x}"), "thread step-inst".into(), "thread list".into(),
              where_()]);
    let (code, out, err) = session(tr, &a);
    let ta = format!("exit {code:?}\n--- stdout\n{out}\n--- stderr\n{err}");
    assert!(out.lines().any(|l| l.trim() == "END"), "the batch ran to its end — no re-step loop: {ta}");
    assert_eq!(code, Some(0), "{ta}");
    let rows = thread_rows(&out);
    let stepped = rows.iter().find(|r| r.1 == me).unwrap_or_else(|| panic!("the stepped thread's row: {ta}"));
    assert!(stepped.3.contains(&format!("hit breakpoint at {b:#x}")), "{rows:?}: {ta}");
    let w = wheres(&out);
    assert_eq!(w.len(), 2, "{ta}");
    assert!(w[0].starts_with(&format!("{n}, ")), "lldb stopped in window n = {n}: {ta}");
    assert!(w[1].starts_with(&format!("{}, 0", n + 1)), "parked at the other thread's hit: {ta}");
```

Leave session B unchanged; Task 12 rewrites it.

```bash
cargo test -p retrace --test lldb_e2e --no-fail-fast -- --test-threads=1 > $L/t11-lldb.log 2>&1; echo "exit=$?"
```

**If session A hits `BOUND` without `END`, the form loops.** Commit nothing. Restore with `git checkout -- crates/retrace` and **route** B6(a), with the packet count taken per t0 M5(iii)'s method.

- [ ] **Step 4: Clippy, commit, control**

```bash
cargo clippy -p retrace --all-targets -- -D warnings > $L/t11-clippy.log 2>&1; echo "exit=$?"
git add crates/retrace
git commit -m "M44 B6(a): another thread's hit ends a blocked step, named on the stepped thread"
```

Control: in `continue_until`, restore the `Some(_) => (Vec::new(), Vec::new())` arm. The rewritten wire row must fail with `reason:trace`. Restore with `git checkout -- crates/retrace/src/debug.rs`.

---

### Task 12 (B6b): a step of a thread that is not running runs until it is

**Files:**
- Modify: `crates/retrace/src/debug.rs` (`step_thread`'s head, ~936)
- Modify: `crates/retrace/tests/gdbserver_e2e.rs` (rewrite `a_step_on_a_thread_that_is_not_running_is_refused_in_place`; add the does-not-exist row)
- Modify: `crates/retrace/tests/lldb_e2e.rs` (session B of the Task 11-renamed row)

**Interfaces:** Consumes `run_until_thread` from Task 11.

- [ ] **Step 1: The failing rows**

Replace `a_step_on_a_thread_that_is_not_running_is_refused_in_place` with two rows:

```rust
#[test]
fn a_step_on_a_thread_that_is_not_running_runs_until_it_is_scheduled() {
    // M44 B6(b), M43 T5-a's successor: a step of a live thread that is not running runs until that
    // thread is scheduled, then steps it one instruction — as a blocked step's tail already does.
    let (tr, n, t) = r::threadrust_block();
    let svc = r::trap_pc(tr, n);
    let other = if t == 0 { 1u32 } else { 0 }; // retrace's id of the thread that is not t
    // The oracle, without the server: the first landmark after n where `other` is current, then
    // one instruction of it.
    let want = (n + 1..).find_map(|m| {
        let mut s = retrace_core::seek(tr, m, 0).ok()?;
        (s.current_thread() == other).then(|| { s.step_insns(1).unwrap(); s.pc() })
    }).expect("the other thread runs again");
    let mut c = Rsp::spawn(tr, &[]);
    assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
    r::continue_to_window(&mut c, n);
    assert_eq!(c.send(&format!("z0,{svc:x},4")), "OK"); // nothing armed: B6(b) alone
    let s = c.send(&format!("vCont;s:{:x}", other + 1));
    assert!(s.contains("reason:trace;"), "{s}");
    assert_eq!(r::key(&s, "thread"), Some(format!("{:x}", other + 1).as_str()), "{s}");
    assert_eq!(pc_of(&s), want, "one instruction of the other thread, once it runs");
}

#[test]
fn a_step_on_a_thread_that_does_not_exist_is_refused_in_place() {
    // §3d rule 1 survives B6(b) for a thread that is not live: refused, nothing moves.
    let (tr, n, _) = r::threadrust_block();
    let svc = r::trap_pc(tr, n);
    let mut c = Rsp::spawn(tr, &[]);
    assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
    r::continue_to_window(&mut c, n);
    let before = c.where_();
    let s = c.send("vCont;s:63");
    assert!(s.contains("reason:exception;"), "{s}");
    assert!(r::description(&s).unwrap().contains("cannot step thread 99"), "{s}");
    assert_eq!(c.where_(), before);
}
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed
cargo test -p retrace --test gdbserver_e2e a_step_on_a_thread --no-fail-fast -- --test-threads=1 > $L/t12-red.log 2>&1; echo "exit=$?"
```

Expected: `…runs_until_it_is_scheduled` fails (refused today), and `…does_not_exist…` passes (today's refusal). Record both.

- [ ] **Step 2: Implement**

Replace `step_thread`'s head:

```rust
        let cur = self.sess().current_thread();
        if t != cur {
            // M44 B6(b): a live thread that is not running is run to until it is — the blocked
            // step's tail — then stepped one instruction. One that has exited, or never existed,
            // is still refused in place (§3d rule 1).
            let live = self.sess().thread_summaries().iter()
                .any(|s| s.tid == t && !matches!(s.state, ThreadState::Exited(_)));
            if !live {
                return Ok(Halt::Refused(format!("cannot step thread {}: it is not a live thread", t + 1)));
            }
            return match self.run_until_thread(t, out)? {
                Halt::Stepped => self.step_thread(t, out),
                other => Ok(other),
            };
        }
```

The recursion is one level deep: on re-entry `t == cur`.

- [ ] **Step 3: Wire suite, lldb session B as the loop check**

```bash
cargo test -p retrace --test gdbserver_e2e --no-fail-fast -- --test-threads=1 > $L/t12-gdbserver.log 2>&1; echo "exit=$?"
```

In the lldb row, replace session B:

```rust
    // Session B (M44 B6(b)): a step of the thread that is not running runs until it is, then steps
    // it: lldb shows that thread stopped by the step, one instruction past where it resumed.
    let want_b = (n + 1..).find_map(|mm| {
        let mut s = retrace_core::seek(tr, mm, 0).ok()?;
        (u64::from(s.current_thread()) + 1 == other).then(|| { s.step_insns(1).unwrap(); s.pc() })
    }).expect("the other thread runs again");
    let mut bb = to_svc(svc, m);
    bb.extend([format!("thread select {other}"), "thread step-inst".into(), "thread list".into()]);
    let (code, out, err) = session(tr, &bb);
    let tb = format!("exit {code:?}\n--- stdout\n{out}\n--- stderr\n{err}");
    assert!(out.lines().any(|l| l.trim() == "END"), "the batch ran to its end — no re-step loop: {tb}");
    assert_eq!(code, Some(0), "{tb}");
    let row = thread_rows(&out).into_iter().find(|r| r.0).unwrap_or_else(|| panic!("a selected thread: {tb}"));
    assert_eq!((row.1, row.2), (other, want_b), "{tb}");
```

Delete `other_pc` and its comment; they belonged to the refusal. Then run:

```bash
cargo test -p retrace --test lldb_e2e --no-fail-fast -- --test-threads=1 > $L/t12-lldb.log 2>&1; echo "exit=$?"
```

**If session B hits `BOUND` without `END`**: commit nothing, restore with `git checkout -- crates/retrace`, and **route** B6(b).

- [ ] **Step 4: Clippy, commit, control**

```bash
cargo clippy -p retrace --all-targets -- -D warnings > $L/t12-clippy.log 2>&1; echo "exit=$?"
git add crates/retrace
git commit -m "M44 B6(b): a step of a thread that is not running runs until it is scheduled, then steps it"
```

Control: make the `live` branch always refuse. `…runs_until_it_is_scheduled` must fail. Restore with `git checkout -- crates/retrace/src/debug.rs`.

---

### Task 13 (A6): the sweep and the docs

**Files:**
- Create: `docs/sweep-evidence/<sweep date>-m44/` (`README.md`, the sweep log, kept stderr)
- Modify: `README.md`, `docs/status-log.md`, `CLAUDE.md`

**Interfaces:** Consumes every task's report and the t0 measurements file. Produces the docs Task 14's reviewer reads.

- [ ] **Step 1: The full sweep on the close's binary** (controller-run)

Copy `target/aarch64-apple-darwin/debug/retrace` to the scratchpad and sign it there, as M39's evidence README describes, so a concurrent `cargo test` cannot swap it. Then:

```bash
export RETRACE_SWEEP_KEEP=<evidence dir>/sweep
tools/apple-sweep.sh <signed copy> > <evidence dir>/sweep.log 2>&1; echo "exit=$?"
grep -a '^TALLY' <evidence dir>/sweep.log
```

Diff every `ROW` label against M39's (`docs/sweep-evidence/2026-09-17-m39/`), and write the evidence README in M39's shape: method, binary hash and commit, the tally, and every moved row with its reason.

- [ ] **Step 2: README, edited in place**

- **"What works today":** each binary un-parked; the `_nocancel` rule now enforced by `tests/nocancel.rs`; `disarm-rsi`; step-at-exit; the lldb stepping rows; arm64e `bt`; B6(a)/(b). List only what landed; routed items go under Known limits.
- **"Known limits":**
  - the missing-row paragraph (`README.md` ~1248): 461/464/345 tabled, 374 routed with M1's measurement, and **468 corrected**: "468 is `fchownat`; `getattrlistat` is 476; neither is reached, and neither has a row (M44 R1)". Leave no "468 `getattrlistat`" text anywhere in the README (`grep -n 468 README.md`);
  - the Apple-sweep table and its prose (~752–830): the new tally and each row's new face;
  - the `F_DUPFD_CLOEXEC` sentence: remove it (paid);
  - "Debugging with lldb: what `retrace gdbserver` does not do": drop what B2–B6 retired, and name what was routed;
  - the Testing section's skip-check instruction: skips now reach the gate log, so `--nocapture` is no longer needed to see them;
  - the gate counts.

- [ ] **Step 3: CLAUDE.md**

Replace the paragraph in "Honest-gate discipline" from `**Where that line goes matters.**` through `A new skip should write past the capture, as `lldb_e2e` does.` with:

```markdown
  **Where that line goes matters.**
  libtest captures `eprintln!` in a *passing* test, and a skip passes, so an `eprintln!` skip line
  never reaches a gate log (measured at M43's close). Since M44 every skip goes through
  `util::announce` (`crates/retrace/tests/util/mod.rs`), which writes past the capture, and
  `skiplines.rs` fails the gate if any test file writes a `SKIP` line with `eprintln!`.
```

In the e2e list, add `skiplines` (M44: the skip-line detector and its control) and name the new `apple_walls_e2e` gates only if they run un-ignored. Update the count sentence for `--bins` only if it changed: `debug.rs`/`rsp.rs` unit counts. Re-count `#[test]` in both files.

- [ ] **Step 4: status-log, appended**

Append `## M44-owed: …` at the end of `docs/status-log.md`, never editing an earlier section. Mirror M43's subsections: what t0 measured; one subsection per task with its commit hashes; the gate; what measurement changed; rulings (R1–R6 plus every route taken); what stays owed. Include:
- a forward pointer: "The M34–M40 sections above name 468 `getattrlistat`. 468 is `fchownat` (SDK); `getattrlistat` is 476; see M44 §2a";
- every routed item, with its successor.

- [ ] **Step 5: Commit**

```bash
git add README.md CLAUDE.md docs/status-log.md docs/sweep-evidence/
git commit -m "M44 close docs: README in place, status log appended, CLAUDE.md's skip rule; the sweep"
```

---

### Task 14: the gate, the reconciliation, the merge (controller-run)

**Files:**
- Create: `.superpowers/sdd/2026-09-27-retrace-m44-owed/{predict.sh,gate.sh,tally.sh,gate-summary.txt}`

- [ ] **Step 1: Predict the count from source, before the gate**

`predict.sh` counts `^\s*#\[test\]` per file on `main` (`git show 64e471e:<path>`) and on the branch. It prints the per-file deltas and totals for `crates/*/tests/*.rs` and `crates/*/src/**/*.rs`, plus the new test-target count. Compare against this plan's table (+18, 146 binaries), and explain every difference by task before running the gate.

- [ ] **Step 2: The chunked gate**

Copy `.superpowers/sdd/2026-09-25-retrace-m43-lldb/gate.sh` and `tally.sh`, change `m43-lldb` to `m44-owed` and the ledger dir to `2026-09-27-retrace-m44-owed`, and run `gate.sh` in the background. It already runs the `ws`, `box` (whole package, so its `Doc-tests` included), and `bins` chunks, then every `crates/retrace/tests/*.rs` target by name (the list is built with `ls`, so `skiplines` is included), then clippy. Every chunk is `--no-fail-fast`, with its exit code in `gate-summary.txt`. **Read the logs, not the script's exit status.**

- [ ] **Step 3: Tally and reconcile**

Run `tally.sh`. The pass bar:
- `failed` 0, and every line in `gate-summary.txt` reads `exit=0`;
- `passed + ignored` equals Step 1's prediction;
- the number of `test result:` lines equals the prediction's binary count.

Any disagreement is reconciled file by file before anything is merged.

- [ ] **Step 4: The skip-line control**

```bash
grep -a -l 'SKIPLINES CONTROL' .superpowers/sdd/2026-09-27-retrace-m44-owed/gate-e2e-skiplines.log
```

Expected: the file is listed. This log came from an ordinary run, with no `--nocapture`. Record it; it is spec §6 item 6.

- [ ] **Step 5: Final review, then merge into local `main`**

Dispatch the whole-branch reviewer (the SDD skill's final review) and apply its fix wave. Re-run only the chunks a fix touched, and re-tally. Then, from the main checkout:

```bash
git merge --no-ff worktree-m44-owed -m "Merge M44-owed: the missing rows, the swallowed skips, and M43's debugger debts"
git rev-parse 'main^{tree}' worktree-m44-owed'^{tree}'
```

The two tree hashes must be equal, which is the tree-identity proof. Then run one headline gate in the main checkout for direct evidence (`cargo test -p retrace --test apple_walls_e2e -- --test-threads=1`). **Do not push.** Report the merge commit, and ask the operator about the push and the worktree cleanup.
