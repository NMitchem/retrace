# M46-gcdtimers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make libdispatch timers work end to end on retrace's synthetic clock. A repo-owned `dispatch_after` guest and a repeating timer-source guest must record to exit 0 and replay bit-identically, with every step of the model a pure function of box state.

**Architecture:**
- **One clock (R7, Task 1).** The recorder rewrites `gettimeofday`'s (116) mach-time out-parameter to the guest's own clock before appending the event. libdispatch's "now" and `mach_absolute_time` then come from one source, `synthetic_tsc`. Replay applies the recorded value verbatim, so it does not change.
- **Pure validators (Task 2).** They live in `retrace-arch` and classify every `kevent_qos` call and every `KEVENT_RETURN` change entry, refusing the rest by value.
- **A pure knote table (Task 3).** `retrace-box/src/kq.rs` holds `WorkqKqueue`, carried in `Box_` through every rebuild path.
- **The manager model (Task 4).** The box spawns the event manager, re-enters it, redelivers to it and parks it. The firing rule in `schedule_after_block` fires overdue timers and makes the idle jump. All of it sits below the trace, so record, replay, `step()` and the rebuild paths share it.
- **No trace-format change.** `TRACE_MAGIC` does not move.

**Tech Stack:** Rust 1.95.0 (`aarch64-apple-darwin`), Hypervisor.framework, cargo tests, clang for guest fixtures, lldb for t0's native measurements, POSIX `sh` for the sweep.

**Spec:** `docs/superpowers/specs/2026-09-29-retrace-m46-gcdtimers-design.md` (committed `9ba90b5`; corrected from this plan, see its §11). Its sections and rulings are cited as `M46 §3b`, `R1` and so on. **R7 is new in this plan and needs the operator's approval** (spec §11 item 1).

## Global Constraints

- **Toolchain.** `1.95.0`, target `aarch64-apple-darwin`.
- **The gate.** `cargo test` in chunks, every chunk `--no-fail-fast` and `--test-threads=1`, plus `cargo clippy --workspace --all-targets -- -D warnings`.
- **`clippy -D warnings` rejects dead code.** A private function, constant or field that nothing uses fails it. Each task adds only what its own non-test code uses. A helper in a test file must be used in that file by the end of the task that adds it.
- **Banned calls.** `clippy.toml` bans `Instant::now`, `SystemTime::now` and `std::thread::Thread`.
- **One VM per process.** Every `cargo test` runs with `--test-threads=1`. Drop a `ReplaySession` before opening another in the same test.
- **The trace format does not move.** `TRACE_MAGIC` stays put, and `crates/retrace-trace` has no diff (R6). R7 changes a recorded value, not a format, and no replay code (spec §11 item 1).
- **Symmetry rule 1.** Every record arm and its replay mirror call the same `Box_` method with the same arguments.
- **The thread oracle's count stays at seven.** The new mirror code lives inside the existing `workq_kernreturn` and `kevent_qos` mirrors, which sit inside the generic `Syscall` arm's chain.
- **Spawn the CLI through `util`.** Every test that spawns the CLI uses `util`'s helpers, which call `util::bin()`, the codesigned copy.
- **Existing assertions stay, with three named exceptions:**
  - `kqinit_e2e.rs:69`'s refusal prefix changes from `M45:` to `M46:` (Task 4).
  - `apple_walls_e2e::automationmodetool_records_and_replays` is un-ignored or re-parked (Task 6).
  - `checkpointparity.rs`'s rich tier gains a staged field and a row (Task 3), an addition.
- **Refusal texts.** The tests match on these prefixes:
  - `M46: unmeasured kevent_qos shape: `
  - `M46: kevent_qos against the knote table: `
  - `M46: unmeasured KEVENT_RETURN change: changelist[<i>].`
  - `M46: KEVENT_RETURN against the knote table: `
  - `M46: workq_kernreturn THREAD_KEVENT_RETURN (0x40) from thread <t>, which is not the bound event manager`

  A replay-side refusal wraps the same message as `<call> refused on replay, though the recording accepted it — replay diverged before this landmark: <message>`.
- **Values pending t0.** A value marked `(t0 M2)` or `(t0 M3)` is sourced or inferred, not yet measured. Where t0 measures a different value, the task uses t0's and its report says so. A difference §3d cannot express is Halt H2.
- **Worktree shell rules:**
  - no `VAR=val cmd` prefix; put `export VAR=val` on its own line first;
  - no `git -C`;
  - put `echo "exit=$?"` in the **same** command as the cargo invocation it checks, **before** any pipe;
  - `--no-fail-fast` goes before `--`;
  - never `git stash`, which is shared across worktrees.
- **Controls (deliberate breakages).**
  - Run them only on the **committed** tree, and restore with `git checkout -- <file>`.
  - Record each control's actual symptom in the task report.
  - A control that stays green is a finding: report it, never paper over it.
- **Logs.** Each command writes to `$L/t<N>-<what>.log`, where `L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers`. Shell state does not persist between tool calls, so every command that uses `$L` starts with its own `export L=…` line. Scratch traces go to `/private/tmp/claude-501/m46-*.bin`.
- **Grep gate logs with `grep -a`**, since they carry ANSI and UTF-8. Before any `awk`, sanitize with `LC_ALL=C tr -cd '\11\12\15\40-\176' < log | sed 's/\x1b\[[0-9;]*m//g'`.
- **Evidence commits exclude trace files**, using the directory-anchored pathspec `':(exclude)<dir>/*.bin'` (M44 P1: the bare form stages nothing).
- **Style.**
  - Match the surrounding code's comment density and idiom.
  - Comments cite the spec as `M46 §3x`, rulings as `R<n>`, and t0 as `(t0 M2)`.
  - Test names are sentences.
- **Never push.** The merge goes into local `main` only; the push waits for the operator.
- **Halt and ask** on any of the spec's halts H1–H5 (§7), and on this plan's two:
  - **Halt 6:** t0 M3 sees an immediate (`0x23`) timer registration on a fixture's path.
  - **Halt 7:** t0 M1(c) finds the guest's `mach_absolute_time` at or above `2^62`, which libdispatch turns into `DISPATCH_TIME_FOREVER`. Also: a frozen commpage clock other than the timebase offset (approximate or continuous time) on the UPTIME timer path.

  H1 is pre-empted by R7 if the operator approves it. H1 still applies to any host-clock channel R7 does not close.

  **Route, don't halt,** when the walk finds a new wall: re-park with the measurement and name the successor (H5).

## Review Focus

These are the five inputs or failure modes the spec implies but no fixture is sure to reach, most likely first. Each is pinned by a test in the task that owns the code.

1. **A `KEVENT_RETURN` that finds events already pending.** This happens when a poke arrived while the manager was bound, or when the synthetic clock (384 µs of guest time per timebase read) passed a deadline before the return. The kernel re-enters the same thread with `0x1E0000`; a model that parks instead strands those events until the next activation.
   - Pinned in Task 4: `a_kevent_return_with_a_trigger_pending_redelivers_on_the_same_thread` (box level).
   - Pinned in Task 3: `a_timer_at_or_before_now_fires_and_a_later_one_does_not`, which is the call that return makes.
2. **A poke while the manager is bound**, spawned or re-entered but not yet run. There must be no second manager and no re-entry: the event waits for the manager's next scan. Pinned in Task 4: `a_second_poke_while_the_manager_is_bound_spawns_nothing`.
3. **The manager re-entered while it is the current thread.** This happens when it has just parked and the idle jump in the same settle fires its timer. `switch_to_thread` returns early for the current thread, so the fresh register block must reach the vCPU directly, or the manager resumes on its `svc` with `x0` = 0. `after_dyn`'s default mode takes exactly this path, because main blocks before the manager ever runs. Pinned by Task 4's test 1 and its control 2.
4. **Several knotes pending in one upcall.** The `USER` event comes first, then fired timers by ident, 16 at most. Pinned in Task 3: `fired_timers_are_delivered_in_ident_order_after_the_user_event`.
5. **A disarm or re-arm of a timer the model no longer holds as armed** (fired and queued, or delivered and dropped). It must be refused by name, never ignored. Pinned in Task 3: `a_change_to_a_timer_whose_fire_is_queued_is_refused` and `a_disarm_of_a_timer_that_is_not_armed_is_refused`.

---

## File Structure

| File | Change | Task |
|---|---|---|
| `docs/superpowers/specs/2026-09-29-retrace-m46-gcdtimers-measurements.md` | create: t0's M1–M4 | 0 |
| `docs/sweep-evidence/2026-09-29-m46-t0/` | create: t0 evidence (lldb logs, stderr, native outputs, README) | 0 |
| `crates/retrace-arch/src/lib.rs` | `SYS_GETTIMEOFDAY` (Task 1); the M46 section: constants, `MEMSTATUS_ADD`, `MANAGER_POKE`, `KeventShape`, `kevent_qos_shape`, `ChangeEntry`, `kevent_return_change`, `timer_fired_event`, `USER_WAKE_EVENT` (Task 2) | 1, 2 |
| `crates/retrace-arch/tests/gcdshapes.rs` | create: 14 validator tests | 2 |
| `crates/retrace-guest/c/after_dyn.c`, `c/timer_dyn.c` | create: the fixtures | 1, 5 |
| `crates/retrace-guest/build.rs`, `src/lib.rs` | build them; `AFTER_DYN`, `TIMER_DYN`; two parse tests | 1, 5 |
| `crates/retrace-box/src/kq.rs` | create: `WorkqKqueue`, `Manager`, `Timer`, `tsc_for_deadline`, 11 unit tests | 3 |
| `crates/retrace-box/src/thread.rs` | `stack_of`, `unpark`; `BlockReason::Parked`'s doc | 4 |
| `crates/retrace-box/src/lib.rs` | R7's `now_guest`/`timebase_offset`/`synthesize_mach_time_out` (Task 1); the `kq` field in six places, `dbg_kq`/`dbg_kq_mut` (Task 3); the manager model, the firing rule, `try_workq_kernreturn`, `guest_kevent_qos -> Result` (Task 4); `dbg_write_va` (Task 5) | 1, 3, 4, 5 |
| `crates/retrace-box/tests/checkpointparity.rs` | stage `kq`, a precondition, a row | 3 |
| `crates/retrace-box/tests/kqmanager.rs` | create: 7 box-level manager tests | 4 |
| `crates/retrace-core/src/lib.rs` | R7 in the record generic arm (Task 1); the two arms and mirrors (Task 4); `dbg_internal_state`, `dbg_armed_timers`, `dbg_write_mem` on `ReplaySession` (Task 5) | 1, 4, 5 |
| `crates/retrace/tests/gcdtimer_e2e.rs` | create: 8 tests (1 in Task 1, 1 in Task 4, 6 in Task 5) | 1, 4, 5 |
| `crates/retrace/tests/kqinit_e2e.rs` | line 69: `M45:` → `M46:` | 4 |
| `crates/retrace/tests/apple_walls_e2e.rs` | `automationmodetool`: un-ignored or re-parked | 6 |
| `docs/sweep-evidence/2026-09-29-m46/` | create: the walk and the sweep | 6 |
| `docs/status-log.md`, `docs/current-state.md`, `README.md`, `CLAUDE.md` | docs | 7 |
| `.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers/{predict,gate,tally}.sh` | the close | 8 |

Each evidence directory is named for the day it is written. If a task runs on another day, use that day's date and carry the name forward.

**Test-count prediction (made here, reconciled at the close):**

| Task | Tests added |
|---|---|
| 1 | `retrace-guest` unit `after_guest_parses` 1; `gcdtimer_e2e.rs` 1 (**new binary**) |
| 2 | `retrace-arch/tests/gcdshapes.rs` 14 (**new binary**) |
| 3 | `retrace-box` lib unit (`kq.rs`) 11 |
| 4 | `retrace-box/tests/kqmanager.rs` 7 (**new binary**); `gcdtimer_e2e.rs` +1 |
| 5 | `retrace-guest` unit `timer_guest_parses` 1; `gcdtimer_e2e.rs` +6 |
| 6 | if `automationmodetool` un-ignores: 1 test moves from ignored to passed |

The baseline is M45's close, 846 passed / 0 failed / 9 ignored over 148 binaries. t0 M4 re-derives it as 853 `#[test]` lines, plus the 2 `census.rs` tests that `legacy_equivalence.rs` compiles a second time.

**Prediction:** +42 `#[test]` lines (853 → 895), so passed + ignored = **897 over 151**. That is 888 / 0 / 9 if `automationmodetool` stays parked, or 889 / 0 / 8 if it un-ignores. Task 8 re-derives this from source.

---

### Task 0 (t0): Measurements first

**Files:**
- Create: `docs/superpowers/specs/2026-09-29-retrace-m46-gcdtimers-measurements.md`
- Create: `docs/sweep-evidence/2026-09-29-m46-t0/` (README plus the kept logs)

**Interfaces:**
- Consumes: nothing.
- Produces: the measurements file, which later tasks read by section:
  - **M1:** (a) the clock channel on retrace's side, (b) which clock libdispatch's timer arm reads natively, (c) the commpage timebase words and the guest's `mach_absolute_time` range. Together they give R7's premise, confirmed or measured away.
  - **M2:** the manager's native entry registers and event bytes. These confirm or replace `0x3C4008` / `0x1E4008` / `0x1E0000` and the two event layouts.
  - **M3:** per fixture mode: the native call sequence, the change entries, the poke's `qos`, the `wall` mode's timer `fflags`, and whether a third registration or an immediate timer registration appears.
  - **M4:** the base `#[test]` count.

**Everything experimental in this task is throwaway.** The only committed files are the measurements file and the evidence directory. Restore every source edit with `git checkout -- <file>` before committing.

- [ ] **Step 1: Copy the sources into the ledger**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
mkdir -p $L/t0/src
cp -R /private/tmp/claude-501/m46-research/. $L/t0/src/ && ls $L/t0/src/*
```

The brainstorming copies live in `/private/tmp`, and a reboot clears it. If they are gone, fetch each file the spec cites by tag:

```bash
curl -sSfL https://raw.githubusercontent.com/apple-oss-distributions/<repo>/<tag>/<path> -o $L/t0/src/<repo>/<path with / as _>
```

Use `libdispatch-1542.100.32`, `xnu-12377.121.6`, `libpthread-539.100.4` and `libmalloc-812.100.31`. The files are:
- libdispatch: `src/event/event_kevent.c`, `src/event/event.c`, `src/source.c`, `src/queue.c`, `src/shims/time.h`, `src/event/event_internal.h`, `src/voucher.c`;
- xnu: `bsd/kern/kern_event.c`, `bsd/pthread/pthread_workqueue.c`, `bsd/pthread/workqueue_syscalls.h`, `bsd/sys/event_private.h`, `libsyscall/wrappers/mach_get_times.c`, `libsyscall/wrappers/__commpage_gettimeofday.c`;
- libpthread: `kern/kern_support.c`, `src/pthread.c`.

Also fetch xnu's `libsyscall/wrappers/__commpage_gettimeofday.c`, which the brainstorming did not copy. R7's premise rests on its "more than one second forces a syscall" test.

- [ ] **Step 2: Write the fixtures into the ledger and run them natively**

Copy Task 1 Step 1's `after_dyn.c` and Task 5 Step 1's `timer_dyn.c` **verbatim** into `$L/t0/`. The files are not in the tree yet, and the later tasks commit the identical text. Then:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
for f in after_dyn timer_dyn; do clang -arch arm64 -o $L/t0/$f $L/t0/$f.c; echo "$f build=$?"; done
for m in "" two wall clock; do $L/t0/after_dyn $m > $L/t0/native-after-${m:-default}.out 2>&1; echo "after '$m' rc=$?"; cat $L/t0/native-after-${m:-default}.out; done
$L/t0/timer_dyn > $L/t0/native-timer.out 2>&1; echo "timer rc=$?"; cat $L/t0/native-timer.out
```

Expected: every build `=0`, every `rc=0`, and these outputs:

| run | output |
|---|---|
| `after` default | `fired cell 0x…`, `fired`, `done` |
| `after two` | `A`, `B`, `done` |
| `after wall` | `fired`, `done` |
| `after clock` | `clock ok` |
| `timer` | `tick 1`, `tick 2`, `tick 3`, `done` |

A different native output is a fixture defect, not a halt. Fix the text in the ledger copy **and** in the task that commits it, and say so in the report.

- [ ] **Step 3: M1(a), the clock channel on retrace's side**

Record the `clock` mode on the base code, before R7 exists, and the default mode as far as M45 takes it:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
export RETRACE_TRACE=1
cargo build -p retrace > $L/t0-m1-build.log 2>&1; echo "exit=$?"
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn $L/t0/after_dyn -o /private/tmp/claude-501/m46-clock.bin -- clock > $L/t0/m1a-clock.out 2> $L/t0/m1a-clock.err; echo "clock record=$?"
cat $L/t0/m1a-clock.out
grep -a -E '^\[trap\] num=116 ' $L/t0/m1a-clock.err
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn $L/t0/after_dyn -o /private/tmp/claude-501/m46-after0.bin > $L/t0/m1a-after.out 2> $L/t0/m1a-after.err; echo "after record=$?"
grep -a -E '^\[trap\] num=116 ' $L/t0/m1a-after.err | awk '{print $NF}' | sort | uniq -c
grep -a 'panicked at' $L/t0/m1a-after.err | head -2
```

Expected, from the sources (spec §11 item 1):
- the `clock` record exits 0 and prints **`clock bad`**;
- its trace lines include at least one `num=116` whose `args` carry a nonzero third element, which is `mach_get_times`'s fallback;
- the default record exits 101 at M45's refusal, `M45: unmeasured kevent_qos shape: x3 (eventlist) …`.

**Decision:**
- `clock bad` with a nonzero third argument confirms R7's premise.
- `clock ok` with no such 116 measures it away. Task 1 then keeps its fixture and test but drops `synthesize_mach_time_out` and its record-arm block, and says so.

- [ ] **Step 4: M1(b), which clock the timer arm reads, natively**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
cat > $L/t0/m1b.lldb <<'EOF'
breakpoint set -n mach_get_times
breakpoint set -n mach_absolute_time
breakpoint set -n mach_approximate_time
breakpoint set -n mach_continuous_time
breakpoint set -n gettimeofday
breakpoint command add -o "bt 12" -o "continue" 1 2 3 4 5
run
EOF
lldb -b -s $L/t0/m1b.lldb -- $L/t0/after_dyn > $L/t0/m1b.log 2>&1; echo "exit=$?"
grep -a -E 'frame #0' $L/t0/m1b.log | sed 's/.*`//' | sort | uniq -c
grep -a -E 'frame #[0-9]+:.*(_dispatch_event_loop_timer_arm|_dispatch_timers_program|_dispatch_timers_run|_dispatch_timers_get_delay)' $L/t0/m1b.log | head
```

For each hit whose backtrace contains `_dispatch_event_loop_timer_arm`, `_dispatch_timers_program`, `_dispatch_timers_run` or `_dispatch_timers_get_delay`, record which clock function is at frame 0. Expected (libdispatch `src/shims/time.h:220-235`, `event_kevent.c:2526`): `mach_get_times`.

**Halt 7** if `mach_approximate_time` or `mach_continuous_time` is at frame 0 under those frames: those read frozen commpage words that R7 does not touch. If `breakpoint command add` rejects the id list, add the commands to each breakpoint separately.

- [ ] **Step 5: M1(c), the commpage timebase words**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
lldb -b -o "disassemble -n mach_absolute_time" -o "disassemble -n __commpage_gettimeofday_internal" -- $L/t0/after_dyn > $L/t0/m1c-disasm.log 2>&1; echo "exit=$?"
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace debug /private/tmp/claude-501/m46-clock.bin --script "x 0xffffc080 0x60" > $L/t0/m1c-commpage.out 2>&1; echo "exit=$?"
cat $L/t0/m1c-commpage.out
```

From the disassembly, record:
- which counter `mach_absolute_time` reads: `S3_4_C15_C10_6` or `CNTVCT_EL0`. `try_emulate_timebase` handles both. `CNTVCTSS_EL0` would be a new, un-emulated read, which is Halt 7's class;
- the commpage offset it adds. Expected `0x88`, `_COMM_PAGE_TIMEBASE_OFFSET`;
- the offset and the user-timebase byte it tests. Expected `0x90`: a value meaning "no user timebase" sends `mach_absolute_time` to the `-3` trap, which is a host clock, so it is H1.

From the commpage bytes, record the 8-byte timebase offset. Compute the guest's `mach_absolute_time` at start as `0x1_0000_0000 + offset` (wrapping). **Halt 7** if it is at or above `0x4000_0000_0000_0000`.

If the `x` address form is rejected, the commpage IPA is `0xF_FFFF_C000` (`crates/retrace-box/src/lib.rs`, `COMMPAGE_IPA`), so write the address as `0xfffffc080`. Task 1's `COMMPAGE_TIMEBASE_OFFSET_IPA` takes the measured offset.

- [ ] **Step 6: M2, the manager's entry, natively**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
cat > $L/t0/m2.lldb <<'EOF'
breakpoint set -n start_wqthread
breakpoint command add -o "register read x0 x1 x2 x3 x4 x5 sp" -o "memory read -s8 -fx -c27 $x0-0x480" -o "continue" 1
run
EOF
for m in "" two; do lldb -b -s $L/t0/m2.lldb -- $L/t0/after_dyn $m > $L/t0/m2-after-${m:-default}.log 2>&1; echo "after '$m' exit=$?"; done
lldb -b -s $L/t0/m2.lldb -- $L/t0/timer_dyn > $L/t0/m2-timer.log 2>&1; echo "timer exit=$?"
grep -a -E ' x4 = ' $L/t0/m2-*.log | sort | uniq -c
```

For every stop whose `x4` has bit `0x100000` (`EVENT_MANAGER`), record:
- the flags word;
- `x3` against `x0 - 0x480`;
- `x5`;
- `sp`;
- the events read at `x0 - 0x480`.

Classify each word as first use (`TSD_BASE_SET` set), reuse (`REUSE` set, `PRIO_QOS` set) or redelivery (`REUSE` set, `PRIO_QOS` clear). Expected: `0x3C4008`, `0x1E4008` and `0x1E0000`.

- The kernel may hand the first manager request to an idle thread it already has, so a native run may never show `0x3C4008`. Record that. The box always spawns a fresh manager, so that word then stays inferred, derived from M18's measured fresh word `0x244000` plus `KEVENT|EVENT_MANAGER` and QoS 8.
- The delivered events expected (spec §2c):
  - a `USER` event: `01 00…00 | f6 ff 21 00 00 00 00 02 | f8 ff ff ff ff ff ff ff | 0…`;
  - a timer event: filter `f9 ff`, flags `35 00`, qos `00 00 00 02`, data 1, `ext[1]` the leeway.

**Halt H2** if the registers or layout contradict spec §2c in a way §3d cannot express.

- [ ] **Step 7: M3, the native call sequence, per fixture mode**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
cat > $L/t0/m3.lldb <<'EOF'
breakpoint set -n kevent_qos
breakpoint command add -o "register read x0 x1 x2 x3 x4 x5 x6 x7" -o "memory read -s8 -fx -c9 $x1" -o "thread info" -o "continue" 1
breakpoint set -n __workq_kernreturn
breakpoint command add -o "register read x0 x1 x2 x3" -o "memory read -s8 -fx -c27 '$x1 ? $x1 : $sp'" -o "thread info" -o "continue" 2
run
EOF
for m in "" two wall; do lldb -b -s $L/t0/m3.lldb -- $L/t0/after_dyn $m > $L/t0/m3-after-${m:-default}.log 2>&1; echo "after '$m' exit=$?"; done
lldb -b -s $L/t0/m3.lldb -- $L/t0/timer_dyn > $L/t0/m3-timer.log 2>&1; echo "timer exit=$?"
```

If lldb rejects the ternary, give `__workq_kernreturn` a second breakpoint with `-c '$x0 == 0x40'` that does the memory read, and drop the read from the first.

For each log, write the ordered call list into the measurements file:
- every `kevent_qos` with `x7`, its entry's filter (bytes 8–9) and thread;
- every `__workq_kernreturn` with opcode `x0`, `x2` and thread;
- for each `0x40`, the change entries: filter, flags, fflags, ident, and whether `data` and `ext[1]` are nonzero.

Record in particular:
- **the poke's `qos` field** (bytes 12–15 of the `EVFILT_USER` entry). Task 2's `MANAGER_POKE` has `qos: 0`;
- **the `wall` mode's timer `fflags`.** Task 5's test 4 expects `0x9c`;
- **the pairing of timer ident and fflags.** Task 2's `UPTIME_TIMER_FFLAGS` puts `0x118` at tidx 0;
- whether a **third registration** appears: any `0x23` entry whose filter is neither −14 nor −10. That is Halt **H3**; `EVFILT_MACHPORT` is −8;
- whether an **immediate timer registration** appears: a `0x23` entry with filter −7. That is **Halt 6**;
- the number of `0x40` calls in the default mode, which Task 4's test 1 bound is checked against.

Then read `docs/sweep-evidence/2026-09-28-m45/automationmodetool.rec.err` for the calls after landmark 363: none are there, because the run stopped at 363. Note that `task_get_debug_control_port` is reached only past the model, and that Task 4 or Task 6 meets it if it matters (H3).

- [ ] **Step 8: M4, the base `#[test]` count**

```bash
grep -r -c -E '^\s*#\[test\]' crates --include='*.rs' | awk -F: '{s+=$2} END {print s}'
```

Expected: `853`. M45 closed at 846 + 9 = 855, which is 853 plus the two `census.rs` tests compiled twice. A different number is reconciled file by file against M45's `predict.sh` output (`.superpowers/sdd/2026-09-28-retrace-m45-kqinit/predict.txt` in the main checkout) before Task 1 starts.

- [ ] **Step 9: Write the measurements file and the evidence; commit**

Create `docs/sweep-evidence/2026-09-29-m46-t0/` containing:
- every `$L/t0/*.log`, `*.out` and `*.err` file;
- the two fixture sources;
- a `README.md` saying, for each file, which command produced it, on which commit, on which date.

Do not copy the sources from Step 1: they stay in the ledger, and the README names their tags.

Write the measurements file with one section per measurement, `## M1` (with (a), (b) and (c)) through `## M4`. Each section gives:
- the command;
- the result, quoted from the evidence file;
- the decision the spec's or plan's rule makes from it;
- any halt considered.

M1's section ends with one line: **R7's premise confirmed** or **measured away**. Then:

```bash
git add docs/superpowers/specs/2026-09-29-retrace-m46-gcdtimers-measurements.md docs/sweep-evidence/2026-09-29-m46-t0 ':(exclude)docs/sweep-evidence/2026-09-29-m46-t0/*.bin'
git commit -m "M46 t0: measurements M1-M4 — the clock channel, the manager's entry, the native sequence, the base count"
```

---

### Task 1: One clock (R7)

**Files:**
- Create: `crates/retrace-guest/c/after_dyn.c`
- Modify: `crates/retrace-guest/build.rs` (after the `kqinit_dyn` block), `crates/retrace-guest/src/lib.rs` (after `KQINIT_DYN`, plus a unit test after `kqinit_guest_parses`)
- Modify: `crates/retrace-arch/src/lib.rs`: add `SYS_GETTIMEOFDAY` directly after `pub const SYS_WRITE: u64 = 4;`
- Modify: `crates/retrace-box/src/lib.rs`: a constant after `pub const COMMPAGE_IPA`, and three methods directly before `/// Reserve the main thread's believed-but-unbacked stack (M8 spec risk R3).`
- Modify: `crates/retrace-core/src/lib.rs`: record's generic forward arm
- Create: `crates/retrace/tests/gcdtimer_e2e.rs`

**Interfaces:**
- Consumes: t0 M1 (the premise, and the commpage offset).
- Produces:
  - `retrace_arch::SYS_GETTIMEOFDAY: u64`
  - `retrace_guest::AFTER_DYN: &str`, with modes `""`, `two`, `wall` and `clock`
  - private `Box_::now_guest(&self) -> u64` and `Box_::timebase_offset(&self) -> u64`, which Task 4 uses
  - `pub fn Box_::synthesize_mach_time_out(&mut self, va: u64) -> Region`
  - in `gcdtimer_e2e.rs`: `records_and_replays(guest, argv) -> (util::RunOut, PathBuf)` and `events(trace) -> Vec<(usize, Event)>`

- [ ] **Step 1: The fixture and its build wiring**

Create `crates/retrace-guest/c/after_dyn.c`:

```c
// M46. The dispatch_after fixture (spec §3f; plan R7). argv[1] selects the mode:
//   (none)  dispatch_after 100 ms onto the default global queue. Main first prints
//           "fired cell <address>", so gcdtimer_e2e can watch the handler's store. The handler
//           stores 1 into `fired_cell`, writes "fired\n" and signals main, which writes "done\n".
//   two     B is registered at 200 ms, then A at 100 ms. They must print A then B. One kernel
//           timer serves the bucket, so B's arm follows A's fire through KEVENT_RETURN.
//   wall    dispatch_after on the WALL clock (dispatch_walltime). retrace must refuse its arm.
//   clock   mach_get_times against mach_absolute_time. It prints "clock ok" iff the absolute time
//           mach_get_times returns lies between two mach_absolute_time reads around it (R7).
// Native stdout (M46 t0): "fired cell 0x…\nfired\ndone\n", "A\nB\ndone\n", "fired\ndone\n",
// "clock ok\n".
#include <dispatch/dispatch.h>
#include <dlfcn.h>
#include <mach/mach_time.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

// The handler's store, and the cell gcdtimer_e2e watches. 8 bytes and 8-aligned, so one
// watchpoint covers it whole.
volatile uint64_t fired_cell;

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "";
    if (strcmp(mode, "clock") == 0) {
        // Private (xnu libsyscall/wrappers/mach_get_times.c) and absent from the SDK's headers, so
        // it is looked up rather than declared and linked. It returns a kern_return_t, an int.
        int (*get_times)(uint64_t *, uint64_t *, struct timespec *) =
            (int (*)(uint64_t *, uint64_t *, struct timespec *))dlsym(RTLD_DEFAULT, "mach_get_times");
        if (get_times == NULL) return 3;
        uint64_t before = mach_absolute_time(), abs = 0, cont = 0;
        struct timespec ts;
        if (get_times(&abs, &cont, &ts) != 0) return 4;
        uint64_t after = mach_absolute_time();
        printf("clock %s\n", before <= abs && abs <= after ? "ok" : "bad");
        return 0;
    }
    dispatch_queue_t q = dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0);
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    if (strcmp(mode, "") == 0) {
        printf("fired cell %p\n", (void *)&fired_cell);
        fflush(stdout);
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 100 * NSEC_PER_MSEC), q, ^{
            fired_cell = 1;
            write(1, "fired\n", 6);
            dispatch_semaphore_signal(sem);
        });
        dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    } else if (strcmp(mode, "two") == 0) {
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 200 * NSEC_PER_MSEC), q, ^{
            write(1, "B\n", 2);
            dispatch_semaphore_signal(sem);
        });
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 100 * NSEC_PER_MSEC), q, ^{
            write(1, "A\n", 2);
            dispatch_semaphore_signal(sem);
        });
        dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
        dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    } else if (strcmp(mode, "wall") == 0) {
        dispatch_after(dispatch_walltime(NULL, 100 * NSEC_PER_MSEC), q, ^{
            write(1, "fired\n", 6);
            dispatch_semaphore_signal(sem);
        });
        dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    } else {
        fprintf(stderr, "after_dyn: unknown mode %s\n", mode);
        return 2;
    }
    write(1, "done\n", 5);
    return 0;
}
```

In `crates/retrace-guest/build.rs`, directly after the `kqinit_dyn` block (after its `assert!(status.success(), "kqinit_dyn guest build failed");`):

```rust

    // after_dyn: the M46 fixture — dispatch_after on the synthetic clock (modes two, wall), and
    // mach_get_times against mach_absolute_time (mode clock, R7). Same recipe as hello_dyn.
    let src = format!("{}/c/after_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/after_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang after_dyn");
    assert!(status.success(), "after_dyn guest build failed");
```

In `crates/retrace-guest/src/lib.rs`, directly after the `KQINIT_DYN` constant:

```rust
/// M46: `dispatch_after` on the synthetic clock. `argv[1]` selects `two`, `wall` or `clock`; see the
/// source's header.
pub const AFTER_DYN: &str = concat!(env!("OUT_DIR"), "/after_dyn");
```

And in its `mod tests`, directly after `kqinit_guest_parses`:

```rust
    #[test]
    fn after_guest_parses() {
        // M46: proves the build.rs wiring and the path constant; behaviour is gcdtimer_e2e's.
        let l = parse_macho(&std::fs::read(AFTER_DYN).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
    }
```

- [ ] **Step 2: Write the failing gate**

Create `crates/retrace/tests/gcdtimer_e2e.rs`:

```rust
// M46 gate (spec §3f, §4; plan R7). libdispatch's timers, end to end, on the synthetic clock.
// Every assertion is on the trace, on the guest's own output or on the recorder's own words, never
// on an exit code alone: a guest whose timer path read the host's clock still exits 0 once real
// time catches up with it.
mod util;

use retrace_trace::Event;
use std::path::{Path, PathBuf};

/// Record `argv` of `guest`, assert exit 0, and replay twice byte-identically.
fn records_and_replays(guest: &str, argv: &[&str]) -> (util::RunOut, PathBuf) {
    let (rec, trace) = util::record_dynamic_args(guest, argv);
    assert_eq!(rec.code, 0, "{argv:?}: record: {}", rec.stderr);
    for n in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "{argv:?}: replay {n}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "{argv:?}: replay {n} stdout");
    }
    (rec, trace)
}

/// Every event of `trace` with its landmark index (the replay session's `idx`).
fn events(trace: &Path) -> Vec<(usize, Event)> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().enumerate().collect()
}

/// R7: the guest's `mach_absolute_time` has one source. `mach_get_times` falls back to
/// `gettimeofday` (116) whenever the commpage stamp is a second or more from `mach_absolute_time`
/// (xnu `libsyscall/wrappers/mach_get_times.c`). retrace's commpage is frozen while its timebase is
/// synthetic, so it always falls back, and before R7 it handed the guest the HOST's mach time,
/// which is libdispatch's timer "now" (t0 M1).
#[test]
fn the_guest_clock_has_one_source() {
    let (rec, trace) = records_and_replays(retrace_guest::AFTER_DYN, &["clock"]);
    assert_eq!(rec.stdout, b"clock ok\n",
        "mach_get_times must agree with mach_absolute_time: got {:?}", String::from_utf8_lossy(&rec.stdout));
    let fallbacks = events(&trace).into_iter()
        .filter(|(_, e)| matches!(e, Event::Syscall { num, args, err: false, .. }
            if *num == retrace_arch::SYS_GETTIMEOFDAY && args[2] != 0))
        .count();
    assert!(fallbacks >= 1,
        "the fixture must reach gettimeofday's mach-time fallback, or this test proves nothing about R7");
}
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
cargo test -p retrace --test gcdtimer_e2e --no-fail-fast -- --test-threads=1 > $L/t1-red.log 2>&1; echo "exit=$?"
grep -a -E 'error\[E0425\]|cannot find value|^test |^test result:' $L/t1-red.log | head
```

Expected: `exit=101`, with a compile error naming `SYS_GETTIMEOFDAY`. Once Step 3 adds the constant, before Steps 4–5, the same test fails on `got "clock bad\n"`: that is the RED t0 M1(a) predicts. Run it again after Step 3 to see it, and record both in the report.

- [ ] **Step 3: The constant**

In `crates/retrace-arch/src/lib.rs`, directly after `pub const SYS_WRITE: u64 = 4;`:

```rust
/// `gettimeofday(struct timeval *tp, struct timezone *tzp, uint64_t *mach_absolute_time)` — the
/// third, xnu-private out-pointer is what `mach_get_times` passes (xnu
/// `libsyscall/wrappers/mach_get_times.c`). The recorder rewrites what the kernel writes there to
/// the guest's own clock (M46 R7).
pub const SYS_GETTIMEOFDAY: u64 = 116;
```

Rerun Step 2's command. Expected: `exit=101` and `the_guest_clock_has_one_source … got "clock bad\n"`.

- [ ] **Step 4: The box method**

In `crates/retrace-box/src/lib.rs`, directly after `pub const COMMPAGE_IPA: u64 = 0x0000_000F_FFFF_C000;`:

```rust
/// M46: `_COMM_PAGE_TIMEBASE_OFFSET` (xnu `osfmk/arm/cpu_capabilities.h`): what the guest's
/// `mach_absolute_time` adds to the counter it reads (t0 M1(c)). The commpage is a copy frozen at
/// load and restored from the snapshot, so this word is identical on every rebuild path.
const COMMPAGE_TIMEBASE_OFFSET_IPA: u64 = COMMPAGE_IPA + 0x88;
```

Directly before `    /// Reserve the main thread's believed-but-unbacked stack (M8 spec risk R3).`:

```rust
    /// M46: the guest-visible `mach_absolute_time` for the current synthetic clock. It is the
    /// counter the timebase `MRS` last returned plus the commpage's timebase offset, which is what
    /// the guest's own `mach_absolute_time` computes (t0 M1(c)). Every input is box state that
    /// record and replay hold identically.
    fn now_guest(&self) -> u64 {
        self.synthetic_tsc.wrapping_add(self.timebase_offset())
    }

    /// M46: the commpage's `_COMM_PAGE_TIMEBASE_OFFSET`. Only a dynamic guest has a commpage, and
    /// only a dynamic guest reads the guest clock through `mach_get_times` or arms a timer.
    fn timebase_offset(&self) -> u64 {
        let b = self.read_guest_checked(COMMPAGE_TIMEBASE_OFFSET_IPA, 8).expect(
            "M46: the guest clock was read on a box with no commpage. Only a dynamic guest reaches \
             mach_get_times's fallback or arms a timer.");
        u64::from_le_bytes(b.try_into().unwrap())
    }

    /// M46 R7: rewrite `gettimeofday`'s (116) `mach_absolute_time` out-parameter, which the host
    /// kernel has just written with the HOST's mach time, to the guest's own clock. Returns the
    /// write for the recorder to append to the event, so that the trace carries the guest's value
    /// and replay's ordinary `apply_and_return` lands it last.
    ///
    /// **Why:** `mach_get_times` falls back to this syscall whenever the commpage's gettimeofday
    /// stamp is a second or more from `mach_absolute_time` (xnu `mach_get_times.c`,
    /// `__commpage_gettimeofday.c`). retrace freezes the commpage at load while the timebase is
    /// synthetic, so the fallback is always taken. Before R7 the guest then held two clocks:
    /// `mach_absolute_time` from `synthetic_tsc`, and `mach_get_times` from the host. libdispatch
    /// takes its timer "now" from the second (libdispatch `src/shims/time.h:220-235`) and its
    /// deadlines from the first, so no timer could fire on the synthetic clock (t0 M1).
    ///
    /// **Record-only, by design.** Replay applies the recorded write verbatim, so no replay code
    /// changes and a pre-M46 trace, which carries the host's value, replays exactly as before. The
    /// value is also the one replay would compute: `now_guest()` at this landmark reads only state
    /// both sides hold identically. The wall time in `tv` stays the host's, recorded as before.
    pub fn synthesize_mach_time_out(&mut self, va: u64) -> Region {
        let now = self.now_guest().to_le_bytes();
        // An aligned u64 cannot straddle a page, so one translation covers all 8 bytes.
        let ipa = self.va_to_ipa(va).filter(|_| va.is_multiple_of(8)).unwrap_or_else(|| panic!(
            "M46 R7: gettimeofday's mach-time out-parameter {va:#x} does not translate or is not \
             8-aligned, though the kernel has just written it"));
        self.write_guest(ipa, &now);
        Region { ipa, bytes: now.to_vec() }
    }

```

- [ ] **Step 5: The record arm**

In `crates/retrace-core/src/lib.rs`'s `record_box`, in the generic forward arm, replace:

```rust
                let (ret, ret1, err, writes) = b.forward_and_diff(num, args);
                // M38: pipe's write end. Gated on the row, not on `ret1 != 0`, so the guest's x1
                // is touched for exactly the rows replay touches it for.
                if retrace_arch::returns_fd_pair(num) { b.set_ret1(ret1); }
```

with:

```rust
                let (ret, ret1, err, mut writes) = b.forward_and_diff(num, args);
                // M38: pipe's write end. Gated on the row, not on `ret1 != 0`, so the guest's x1
                // is touched for exactly the rows replay touches it for.
                if retrace_arch::returns_fd_pair(num) { b.set_ret1(ret1); }
                // M46 R7: gettimeofday's mach-time out-parameter is the guest's own clock, not the
                // host's (see Box_::synthesize_mach_time_out). The rewrite is appended to the
                // event's writes, so replay applies it last with no code of its own. A failed call
                // wrote nothing, so there is nothing to rewrite.
                if num == retrace_arch::SYS_GETTIMEOFDAY && args[2] != 0 && !err {
                    writes.push(b.synthesize_mach_time_out(args[2]));
                }
```

- [ ] **Step 6: Run it green, plus the neighbours and clippy**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
cargo test -p retrace --test gcdtimer_e2e --no-fail-fast -- --test-threads=1 > $L/t1-green.log 2>&1; echo "exit=$?"
grep -a -E '^test |^test result:' $L/t1-green.log
cargo test -p retrace-guest --no-fail-fast -- --test-threads=1 > $L/t1-guest.log 2>&1; echo "exit=$?"
for t in hello_dyn_e2e dispatch_e2e kqinit_e2e jq_e2e cpython_e2e; do cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t1-$t.log 2>&1; echo "$t exit=$?"; done
cargo clippy --workspace --all-targets -- -D warnings > $L/t1-clippy.log 2>&1; echo "exit=$?"
```

Expected:
- `gcdtimer_e2e`: `1 passed`;
- the guest crate: green;
- every neighbour: `exit=0`. `jq_e2e` and `cpython_e2e` may print `SKIPPED` if their tools are absent, and the report must say which ran;
- clippy: `exit=0`.

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-guest/c/after_dyn.c crates/retrace-guest/build.rs crates/retrace-guest/src/lib.rs crates/retrace-arch/src/lib.rs crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs crates/retrace/tests/gcdtimer_e2e.rs
git commit -m "M46 t1: R7 — gettimeofday's mach-time out-parameter is the guest's own clock, recorded"
```

- [ ] **Step 8: The control (on the committed tree)**

Delete the R7 block (the `if num == retrace_arch::SYS_GETTIMEOFDAY …` statement and its comment) from `record_box`. Run `gcdtimer_e2e`.

Expected: `the_guest_clock_has_one_source` fails on `got "clock bad\n"`. Restore with `git checkout -- crates/retrace-core/src/lib.rs`, and check that `git status --short` prints nothing.

---

### Task 2: The validators (`retrace-arch`)

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs`: a new section directly before the line `// ---- M12-signal-delivery`, after M45's section
- Create: `crates/retrace-arch/tests/gcdshapes.rs`

**Interfaces:**
- Consumes: t0 M2 (event bytes) and t0 M3 (the poke's `qos`, and the timer ident-to-fflags pairing).
- Produces, all `pub` in `retrace_arch`:
  - constants: `EVFILT_TIMER: i16`, `EVFILT_MEMORYSTATUS: i16`, `EV_DELETE`, `EV_ONESHOT`, `EV_DISPATCH`, `EV_UDATA_SPECIFIC` (`u16`); `NOTE_TRIGGER`, `NOTE_NSECONDS`, `NOTE_ABSOLUTE`, `NOTE_LEEWAY`, `NOTE_CRITICAL`, `NOTE_BACKGROUND`, `NOTE_MACH_CONTINUOUS_TIME`, `NOTE_MACHTIME`, `KEVENT_FLAG_ERROR_EVENTS` (`u32`); `EVENT_MANAGER_QOS: i32`;
  - workqueue: `WQOPS_THREAD_KEVENT_RETURN: u64`; `WQ_FLAG_THREAD_{PRIO_QOS,REUSE,NEWSPI,KEVENT,EVENT_MANAGER,TSD_BASE_SET}: u32`; `WQ_KEVENT_LIST_LEN: usize`;
  - timers: `UPTIME_TIMER_FFLAGS: [u32; 3]`, `TIMER_IDENT_BASE: u64`;
  - measured entries: `MEMSTATUS_ADD: KeventQos`, `MANAGER_POKE: KeventQos`;
  - `enum KeventShape { Init, MemoryStatusAdd { udata: u64 }, ManagerPoke }`
  - `fn kevent_qos_shape(args: [u64; 8], entry: &[u8]) -> Result<KeventShape, String>`
  - `enum ChangeEntry { TimerAdd { ident: u64, deadline: u64, leeway: u64, udata: u64 }, TimerDelete { ident: u64 } }`
  - `fn kevent_return_change(entry: &[u8; KEVENT_QOS_SIZE]) -> Result<ChangeEntry, String>`
  - `fn timer_fired_event(ident: u64, leeway: u64, udata: u64) -> KeventQos`
  - `const USER_WAKE_EVENT: KeventQos`

- [ ] **Step 1: Write the failing test**

Create `crates/retrace-arch/tests/gcdshapes.rs`:

```rust
//! M46 (spec §3b, §4): the validators that decide which `kevent_qos` (374) calls and which
//! `workq_kernreturn(THREAD_KEVENT_RETURN)` change entries the box models. Pure and VM-free.
//! Every shape and entry this file does not accept is one the recorder stops on by name.
use retrace_arch::{
    kevent_qos_shape, kevent_return_change, timer_fired_event, ChangeEntry, KeventQos, KeventShape,
    EVENT_MANAGER_QOS, EVFILT_TIMER, EVFILT_USER, EV_ADD, EV_DELETE, EV_ENABLE, EV_ONESHOT,
    KEVENT_QOS_SIZE, KQINIT, MANAGER_POKE, MEMSTATUS_ADD, TIMER_IDENT_BASE, UPTIME_TIMER_FFLAGS,
    USER_WAKE_EVENT,
};

/// M45's measured init call (`kqinit.rs`).
const INIT_ARGS: [u64; 8] = [0xffff_ffff, 0x27f_f348, 1, 0, 0, 0, 0, 0x21];
/// M45's measurement of the second call (`automationmodetool.entry.txt`): one change, a 16-entry
/// event list, `x7` = `WORKQ|ERROR_EVENTS|IMMEDIATE`. `x1` and `x3` are stack addresses.
const REG_ARGS: [u64; 8] = [0xffff_ffff, 0x27f_ed00, 1, 0x27f_edb8, 0x10, 0, 0, 0x23];

fn memstatus(udata: u64) -> [u8; KEVENT_QOS_SIZE] { KeventQos { udata, ..MEMSTATUS_ADD }.to_bytes() }

fn timer_add(tidx: u64, deadline: i64, leeway: u64, udata: u64) -> KeventQos {
    KeventQos {
        ident: TIMER_IDENT_BASE | tidx, filter: EVFILT_TIMER, flags: EV_ADD | EV_ENABLE | EV_ONESHOT,
        qos: EVENT_MANAGER_QOS, udata, fflags: UPTIME_TIMER_FFLAGS[tidx as usize], xflags: 0,
        data: deadline, ext: [0, leeway, 0, 0],
    }
}

/// `kqinit_shape` is the `Init` arm of the new dispatcher, so M45's refusal texts survive it
/// (`kqinit_e2e`'s `flags` mode matches on this one).
#[test]
fn the_init_keeps_its_m45_meaning_through_the_dispatcher() {
    assert_eq!(kevent_qos_shape(INIT_ARGS, &KQINIT.to_bytes()), Ok(KeventShape::Init));
    let mut e = KQINIT.to_bytes();
    e[10] ^= 0x04; // EV_ENABLE
    assert_eq!(kevent_qos_shape(INIT_ARGS, &e).unwrap_err(), "changelist[0].flags is 0x25, measured 0x21");
}

/// R1: the memory-pressure registration's udata is a heap pointer that varies per run. It is read
/// and handed to the knote table, never compared.
#[test]
fn the_memory_pressure_registration_is_classified_with_its_udata_read() {
    for udata in [0x6c850, 0x1_0000_0000, u64::MAX] {
        assert_eq!(kevent_qos_shape(REG_ARGS, &memstatus(udata)), Ok(KeventShape::MemoryStatusAdd { udata }));
    }
}

#[test]
fn every_compared_bit_of_the_memory_pressure_entry_is_refused() {
    for byte in (0..KEVENT_QOS_SIZE).filter(|b| !(16..24).contains(b)) {
        for bit in 0..8 {
            let mut e = memstatus(0x6c850);
            e[byte] ^= 1 << bit;
            assert!(kevent_qos_shape(REG_ARGS, &e).is_err(), "byte {byte} bit {bit} flipped and still accepted");
        }
    }
}

/// The poke carries only constants (`_dispatch_event_loop_poke`), udata included, so every one of
/// its 576 bits is compared.
#[test]
fn the_manager_poke_is_classified_and_every_bit_of_it_is_compared() {
    assert_eq!(kevent_qos_shape(REG_ARGS, &MANAGER_POKE.to_bytes()), Ok(KeventShape::ManagerPoke));
    for byte in 0..KEVENT_QOS_SIZE {
        for bit in 0..8 {
            let mut e = MANAGER_POKE.to_bytes();
            e[byte] ^= 1 << bit;
            assert!(kevent_qos_shape(REG_ARGS, &e).is_err(), "byte {byte} bit {bit} flipped and still accepted");
        }
    }
}

/// R2: `kq`, `nchanges`, `nevents` and `flags` are `int`s the kernel reads 32 bits of;
/// `data_out` and `data_available` are pointers, compared whole. `x1` and `x3` are where the lists
/// are, not what they hold, so they are not compared (M45 R4).
#[test]
fn argument_widths_follow_the_kernels_types() {
    let e = MANAGER_POKE.to_bytes();
    for i in [0, 2, 4, 7] {
        for bit in 32..64 {
            let mut a = REG_ARGS;
            a[i] ^= 1u64 << bit;
            assert_eq!(kevent_qos_shape(a, &e), Ok(KeventShape::ManagerPoke), "x{i} bit {bit}");
        }
    }
    for (i, width) in [(0, 32), (2, 32), (4, 32), (5, 64), (6, 64)] {
        for bit in 0..width {
            let mut a = REG_ARGS;
            a[i] ^= 1u64 << bit;
            let err = kevent_qos_shape(a, &e).unwrap_err();
            assert!(err.starts_with(&format!("x{i} (")), "x{i} bit {bit}: {err}");
        }
    }
    for x in [0, 0x10, u64::MAX] {
        let mut a = REG_ARGS;
        (a[1], a[3]) = (x, x);
        assert_eq!(kevent_qos_shape(a, &e), Ok(KeventShape::ManagerPoke), "x1 = x3 = {x:#x}");
    }
}

#[test]
fn an_unmeasured_flags_word_is_refused_naming_the_two_measured_ones() {
    for bit in 0..32 {
        let mut a = REG_ARGS;
        a[7] ^= 1 << bit;
        if a[7] & 0xffff_ffff == 0x21 { continue; } // bit 1: the init's word, judged by kqinit_shape
        let err = kevent_qos_shape(a, &MANAGER_POKE.to_bytes()).unwrap_err();
        assert!(err.starts_with("x7 (flags, as unsigned int) is ") && err.contains("0x21") && err.contains("0x23"),
            "bit {bit}: {err}");
    }
}

/// An immediate timer registration (libdispatch's deferred-list overflow, `event_kevent.c:955-975`)
/// or any other filter is refused by name (M46 §7; plan Halt 6).
#[test]
fn a_registration_of_any_other_filter_is_refused_naming_the_filter() {
    let t = KeventQos { filter: EVFILT_TIMER, ..MANAGER_POKE }.to_bytes();
    let err = kevent_qos_shape(REG_ARGS, &t).unwrap_err();
    assert!(err.starts_with("changelist[0].filter is 0xfff9, measured 0xfff2 (EVFILT_MEMORYSTATUS) or 0xfff6 (EVFILT_USER)"),
        "{err}");
}

/// `read_va_prefix` stops at the first byte that does not translate, so an untranslated entry
/// arrives short and must be refused, never padded.
#[test]
fn a_short_registration_entry_is_refused_as_untranslated() {
    for len in [0, 40, 71] {
        let err = kevent_qos_shape(REG_ARGS, &MANAGER_POKE.to_bytes()[..len]).unwrap_err();
        assert!(err.contains(&format!("read {len} of 72 bytes")), "len {len}: {err}");
    }
}

/// R1: a timer arm's deadline (`data`), leeway (`ext[1]`) and udata vary per call and are read.
#[test]
fn a_timer_arm_is_classified_with_its_deadline_leeway_and_udata_read() {
    for tidx in 0..3 {
        for (deadline, leeway, udata) in [(0x1_2345_6789i64, 0u64, 0x6c850u64), (i64::MAX, u64::MAX, 1)] {
            let e = timer_add(tidx, deadline, leeway, udata);
            assert_eq!(kevent_return_change(&e.to_bytes()),
                Ok(ChangeEntry::TimerAdd { ident: TIMER_IDENT_BASE | tidx, deadline: deadline as u64, leeway, udata }));
        }
    }
}

#[test]
fn every_compared_bit_of_a_timer_arm_is_refused() {
    // Read, not compared (R1): udata 16..24, data 32..40, ext[1] 48..56.
    let read = |b: usize| (16..24).contains(&b) || (32..40).contains(&b) || (48..56).contains(&b);
    for byte in (0..KEVENT_QOS_SIZE).filter(|&b| !read(b)) {
        for bit in 0..8 {
            let mut e = timer_add(0, 0x1_2345_6789, 0x100, 0x6c850).to_bytes();
            e[byte] ^= 1 << bit;
            assert!(kevent_return_change(&e).is_err(), "byte {byte} bit {bit} flipped and still accepted");
        }
    }
}

#[test]
fn a_timer_disarm_is_classified() {
    let e = KeventQos { flags: EV_DELETE | EV_ONESHOT, data: 0, ext: [0; 4], ..timer_add(1, 0, 0, 0x6c850) };
    assert_eq!(kevent_return_change(&e.to_bytes()), Ok(ChangeEntry::TimerDelete { ident: TIMER_IDENT_BASE | 1 }));
}

/// M46 §7: MONOTONIC and WALL timers name the clock the model lacks. fflags are judged before the
/// ident, so the refusal names them (gcdtimer_e2e's `wall` test matches on this text).
#[test]
fn monotonic_and_wall_timers_are_refused_naming_their_fflags() {
    for (fflags, tidx) in [(0x198u32, 3u64), (0x9c, 6)] {
        let e = KeventQos { ident: TIMER_IDENT_BASE | tidx, fflags, ..timer_add(0, 1, 0, 1) };
        let err = kevent_return_change(&e.to_bytes()).unwrap_err();
        assert!(err.starts_with(&format!("fflags is {fflags:#x}, measured one of 0x118, 0x138, 0x158")), "{err}");
    }
}

/// The delivered events, as xnu lays them out (spec §2c), and as t0 M2 read them natively.
#[test]
fn the_delivered_events_are_the_measured_bytes() {
    let fired = timer_fired_event(TIMER_IDENT_BASE, 0x100, 0x6c850).to_bytes();
    assert_eq!(&fired[8..16], &[0xf9, 0xff, 0x35, 0x00, 0x00, 0x00, 0x00, 0x02], "filter -7, flags 0x35, qos");
    assert_eq!(&fired[32..40], &1i64.to_le_bytes(), "data: one expiration");
    assert_eq!(&fired[48..56], &0x100u64.to_le_bytes(), "ext[1]: the leeway");
    let user = USER_WAKE_EVENT.to_bytes();
    assert_eq!(&user[0..24], &[1, 0, 0, 0, 0, 0, 0, 0, 0xf6, 0xff, 0x21, 0x00, 0, 0, 0, 2,
                               0xf8, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    assert!(user[24..].iter().all(|&b| b == 0), "fflags, xflags, data and ext are zero");
    assert_eq!(USER_WAKE_EVENT.filter, EVFILT_USER);
}

fn sdk_header(rel: &str) -> String {
    let out = std::process::Command::new("xcrun").arg("--show-sdk-path").output().expect("run xcrun");
    assert!(out.status.success(), "xcrun --show-sdk-path failed");
    let path = format!("{}/usr/include/{rel}", String::from_utf8(out.stdout).unwrap().trim());
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The value of `#define <name> <value>` in `text`: parentheses stripped, decimal or `0x` hex, an
/// optional leading minus. `None` if the header does not define `name`.
fn define(text: &str, name: &str) -> Option<i64> {
    text.lines().find_map(|l| {
        let mut w = l.split_whitespace();
        if w.next() != Some("#define") || w.next() != Some(name) { return None; }
        let v = w.next()?.trim_start_matches('(').trim_end_matches(')');
        let (neg, v) = match v.strip_prefix('-') { Some(r) => (true, r), None => (false, v) };
        let n = match v.strip_prefix("0x") { Some(h) => i64::from_str_radix(h, 16).ok()?, None => v.parse().ok()? };
        Some(if neg { -n } else { n })
    })
}

/// The constants the SDK ships are the SDK's, read at test time (M44 R5's method).
/// `EVFILT_MEMORYSTATUS` is xnu-private (`event_private.h:81`), and this asserts the SDK still
/// lacks it, so a future SDK that ships it is noticed and cited instead.
#[test]
fn the_constants_are_the_sdks_where_the_sdk_has_them() {
    use retrace_arch as a;
    let ev = sdk_header("sys/event.h");
    for (name, v) in [
        ("EVFILT_TIMER", i64::from(a::EVFILT_TIMER)), ("EV_DELETE", i64::from(a::EV_DELETE)),
        ("EV_ONESHOT", i64::from(a::EV_ONESHOT)), ("EV_DISPATCH", i64::from(a::EV_DISPATCH)),
        ("EV_UDATA_SPECIFIC", i64::from(a::EV_UDATA_SPECIFIC)), ("NOTE_TRIGGER", i64::from(a::NOTE_TRIGGER)),
        ("NOTE_NSECONDS", i64::from(a::NOTE_NSECONDS)), ("NOTE_ABSOLUTE", i64::from(a::NOTE_ABSOLUTE)),
        ("NOTE_LEEWAY", i64::from(a::NOTE_LEEWAY)), ("NOTE_CRITICAL", i64::from(a::NOTE_CRITICAL)),
        ("NOTE_BACKGROUND", i64::from(a::NOTE_BACKGROUND)),
        ("NOTE_MACH_CONTINUOUS_TIME", i64::from(a::NOTE_MACH_CONTINUOUS_TIME)),
        ("NOTE_MACHTIME", i64::from(a::NOTE_MACHTIME)),
        ("KEVENT_FLAG_ERROR_EVENTS", i64::from(a::KEVENT_FLAG_ERROR_EVENTS)),
    ] {
        assert_eq!(define(&ev, name), Some(v), "{name}");
    }
    assert_eq!(define(&ev, "EVFILT_MEMORYSTATUS"), None,
        "the SDK now defines EVFILT_MEMORYSTATUS: cite it from the SDK instead of xnu");
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
cargo test -p retrace-arch --test gcdshapes --no-fail-fast -- --test-threads=1 > $L/t2-red.log 2>&1; echo "exit=$?"
grep -a -E 'unresolved import|error\[E0432\]' $L/t2-red.log | head -3
```

Expected: `exit=101`, and an `unresolved import` naming the new items.

- [ ] **Step 3: Implement**

In `crates/retrace-arch/src/lib.rs`, directly before the line `// ---- M12-signal-delivery`:

```rust
// ---- M46-gcdtimers: libdispatch's event manager and its timers ----------------------------------
// Values from the macOS 26 SDK's `sys/event.h`, which tests/gcdshapes.rs re-reads at test time,
// except where a comment cites xnu or libpthread: those are private and absent from the SDK.
/// `EVFILT_TIMER` (`sys/event.h:74`).
pub const EVFILT_TIMER: i16 = -7;
/// `EVFILT_MEMORYSTATUS` (xnu `bsd/sys/event_private.h:81`; the SDK does not ship it).
pub const EVFILT_MEMORYSTATUS: i16 = -14;
/// `EV_DELETE` (`sys/event.h:137`).
pub const EV_DELETE: u16 = 0x0002;
/// `EV_ONESHOT` (`sys/event.h:142`).
pub const EV_ONESHOT: u16 = 0x0010;
/// `EV_DISPATCH` (`sys/event.h:149`).
pub const EV_DISPATCH: u16 = 0x0080;
/// `EV_UDATA_SPECIFIC` (`sys/event.h:150`).
pub const EV_UDATA_SPECIFIC: u16 = 0x0100;
/// `NOTE_TRIGGER` (`sys/event.h:204`): fire an `EVFILT_USER` knote.
pub const NOTE_TRIGGER: u32 = 0x0100_0000;
/// The timer fflags (`sys/event.h:304-318`).
pub const NOTE_NSECONDS: u32 = 0x04;
pub const NOTE_ABSOLUTE: u32 = 0x08;
pub const NOTE_LEEWAY: u32 = 0x10;
pub const NOTE_CRITICAL: u32 = 0x20;
pub const NOTE_BACKGROUND: u32 = 0x40;
pub const NOTE_MACH_CONTINUOUS_TIME: u32 = 0x80;
pub const NOTE_MACHTIME: u32 = 0x100;
/// `KEVENT_FLAG_ERROR_EVENTS` (`sys/event.h:133`): copy out change errors only.
pub const KEVENT_FLAG_ERROR_EVENTS: u32 = 0x2;
/// `_PTHREAD_PRIORITY_EVENT_MANAGER_FLAG`: the `qos` of every knote in the manager's bucket (the
/// value `KQINIT` carries).
pub const EVENT_MANAGER_QOS: i32 = 0x0200_0000;

/// `WQOPS_THREAD_KEVENT_RETURN` (xnu `bsd/pthread/workqueue_syscalls.h`): a kevent worker handing
/// its change list back (libpthread `pthread.c:2581-2635`). Emulated by
/// `Box_::try_workq_kernreturn`, and only from the bound event manager.
pub const WQOPS_THREAD_KEVENT_RETURN: u64 = 0x40;
/// The flags word a workqueue thread's entry receives in `x4` (xnu `workqueue_syscalls.h:51-66`).
pub const WQ_FLAG_THREAD_PRIO_QOS: u32 = 0x0000_4000;
pub const WQ_FLAG_THREAD_REUSE: u32 = 0x0002_0000;
pub const WQ_FLAG_THREAD_NEWSPI: u32 = 0x0004_0000;
pub const WQ_FLAG_THREAD_KEVENT: u32 = 0x0008_0000;
pub const WQ_FLAG_THREAD_EVENT_MANAGER: u32 = 0x0010_0000;
pub const WQ_FLAG_THREAD_TSD_BASE_SET: u32 = 0x0020_0000;
/// `WQ_KEVENT_LIST_LEN` (libpthread `kern/kern_support.c`): the most events one kevent upcall
/// carries, laid out at `self − 16 × 72`.
pub const WQ_KEVENT_LIST_LEN: usize = 16;

/// The UPTIME timer fflags, indexed by libdispatch's timer index `tidx` (clock × 3 + QoS bucket,
/// with UPTIME = clock 0 and NORMAL/CRITICAL/BACKGROUND = 0/1/2; libdispatch
/// `event_internal.h:681-695`, the fflags table `event_kevent.c:49-75`; t0 M3). UPTIME is the only
/// clock M46 models.
pub const UPTIME_TIMER_FFLAGS: [u32; 3] = [
    NOTE_MACHTIME | NOTE_ABSOLUTE | NOTE_LEEWAY,
    NOTE_MACHTIME | NOTE_ABSOLUTE | NOTE_LEEWAY | NOTE_CRITICAL,
    NOTE_MACHTIME | NOTE_ABSOLUTE | NOTE_LEEWAY | NOTE_BACKGROUND,
];
/// `DISPATCH_KEVENT_TIMEOUT_IDENT_MASK` (libdispatch `event_kevent.c:2488`): a timer knote's
/// ident is this, ORed with its `tidx`.
pub const TIMER_IDENT_BASE: u64 = 0xffff_ffff_ffff_ff00;

/// libdispatch's memory-pressure registration (`_dispatch_memorypressure_init`, M46 §2a), measured
/// by M45's walk. `udata` is a heap pointer that varies per run: read, not compared (R1).
pub const MEMSTATUS_ADD: KeventQos = KeventQos {
    ident: 0,
    filter: EVFILT_MEMORYSTATUS,
    flags: EV_ADD | EV_ENABLE | EV_DISPATCH | EV_UDATA_SPECIFIC, // 0x0185
    qos: EVENT_MANAGER_QOS,
    udata: 0,
    fflags: 0xf000_0037, // PRESSURE_NORMAL|WARN|CRITICAL, PROC_LIMIT_WARN|CRITICAL, MSL_STATUS
    xflags: 0,
    data: 0,
    ext: [0; 4],
};

/// The event manager's poke (`_dispatch_event_loop_poke`, libdispatch `event_kevent.c:1979-1988`):
/// a `NOTE_TRIGGER` touch of the init's `EVFILT_USER` knote. Every field is a constant and every
/// field is compared (t0 M3; the struct literal sets no `qos`).
pub const MANAGER_POKE: KeventQos = KeventQos {
    ident: 1,
    filter: EVFILT_USER,
    flags: 0,
    qos: 0,
    udata: !0x7, // DISPATCH_WLH_MANAGER
    fflags: NOTE_TRIGGER,
    xflags: 0,
    data: 0,
    ext: [0; 4],
};

/// One `kevent_qos` call the box models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeventShape {
    /// M45's `_dispatch_kq_init` (`KQINIT`).
    Init,
    /// `MEMSTATUS_ADD`, carrying the udata it registered.
    MemoryStatusAdd { udata: u64 },
    /// `MANAGER_POKE`.
    ManagerPoke,
}

/// One entry of a `KEVENT_RETURN` change list the box models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeEntry {
    /// Arm (or reprogram) an UPTIME timer for the absolute deadline `deadline`, in the guest's
    /// `mach_absolute_time` ticks.
    TimerAdd { ident: u64, deadline: u64, leeway: u64, udata: u64 },
    /// Disarm an armed timer.
    TimerDelete { ident: u64 },
}

/// The first field of `got` that differs from `want`, skipping those named in `read`, as
/// `"<field> is <got>, measured <want>"`.
fn first_difference(got: &KeventQos, want: &KeventQos, read: &[&str]) -> Result<(), String> {
    for ((name, g), (_, w)) in got.fields().into_iter().zip(want.fields()) {
        if !read.contains(&name) && g != w {
            return Err(format!("{name} is {g:#x}, measured {w:#x}"));
        }
    }
    Ok(())
}

/// Classify a `kevent_qos` call as one of the three shapes the box models (M46 §3b), or name the
/// first register or field that differs. `x7 = 0x21` is M45's init, judged by `kqinit_shape` so
/// its texts survive. `x7 = 0x23` is a registration from a thread with no deferred-items list: the
/// memory-pressure source or the manager poke, told apart by the entry's filter.
///
/// `int` arguments are compared on the 32 bits the kernel reads, and pointers whole (R2). `x1` and
/// `x3` locate the lists and are not compared (M45 R4). A short `entry` did not fully translate.
pub fn kevent_qos_shape(args: [u64; 8], entry: &[u8]) -> Result<KeventShape, String> {
    const LOW: u64 = 0xffff_ffff;
    let init = u64::from(KEVENT_FLAG_WORKQ | KEVENT_FLAG_IMMEDIATE);
    let registration = u64::from(KEVENT_FLAG_WORKQ | KEVENT_FLAG_ERROR_EVENTS | KEVENT_FLAG_IMMEDIATE);
    let flags = args[7] & LOW;
    if flags == init {
        return kqinit_shape(args, entry).map(|()| KeventShape::Init);
    }
    if flags != registration {
        return Err(format!("x7 (flags, as unsigned int) is {flags:#x}, measured {init:#x} (the init) \
                            or {registration:#x} (a registration)"));
    }
    let checks: [(usize, &str, u64, u64); 5] = [
        (0, "kq, as int", 0xffff_ffff, args[0] & LOW),
        (2, "nchanges, as int", 1, args[2] & LOW),
        (4, "nevents, as int", WQ_KEVENT_LIST_LEN as u64, args[4] & LOW),
        (5, "data_out", 0, args[5]),
        (6, "data_available", 0, args[6]),
    ];
    for (i, name, want, got) in checks {
        if got != want {
            return Err(format!("x{i} ({name}) is {got:#x}, measured {want:#x}"));
        }
    }
    let Ok(bytes) = <&[u8; KEVENT_QOS_SIZE]>::try_from(entry) else {
        return Err(format!("the change list's entry read {} of {KEVENT_QOS_SIZE} bytes: it does not \
                            fully translate", entry.len()));
    };
    let e = KeventQos::from_bytes(bytes);
    match e.filter {
        EVFILT_MEMORYSTATUS => first_difference(&e, &MEMSTATUS_ADD, &["udata"])
            .map(|()| KeventShape::MemoryStatusAdd { udata: e.udata }),
        EVFILT_USER => first_difference(&e, &MANAGER_POKE, &[]).map(|()| KeventShape::ManagerPoke),
        other => Err(format!("filter is {:#x}, measured {:#x} (EVFILT_MEMORYSTATUS) or {:#x} \
                              (EVFILT_USER); an immediate timer or any other registration is not \
                              modelled (M46 §7)",
                             other as u16, EVFILT_MEMORYSTATUS as u16, EVFILT_USER as u16)),
    }
    .map_err(|why| format!("changelist[0].{why}"))
}

/// Classify one 72-byte `KEVENT_RETURN` change entry (M46 §3b): an UPTIME timer arm or disarm, or
/// `Err` naming the first field that differs. `filter`, `flags` and `fflags` are judged first, so a
/// MONOTONIC or WALL timer is refused by the fflags that name its clock, not by its ident. The
/// deadline (`data`), the leeway (`ext[1]`) and `udata` vary per call and are read (R1).
pub fn kevent_return_change(entry: &[u8; KEVENT_QOS_SIZE]) -> Result<ChangeEntry, String> {
    let e = KeventQos::from_bytes(entry);
    if e.filter != EVFILT_TIMER {
        return Err(format!("filter is {:#x}, measured {:#x} (EVFILT_TIMER): the manager's only \
                            measured change is a timer", e.filter as u16, EVFILT_TIMER as u16));
    }
    let arm = EV_ADD | EV_ENABLE | EV_ONESHOT;
    let disarm = EV_DELETE | EV_ONESHOT;
    if e.flags != arm && e.flags != disarm {
        return Err(format!("flags is {:#x}, measured {arm:#x} (an arm) or {disarm:#x} (a disarm)", e.flags));
    }
    let Some(tidx) = UPTIME_TIMER_FFLAGS.iter().position(|&f| f == e.fflags) else {
        let uptime = UPTIME_TIMER_FFLAGS.map(|f| format!("{f:#x}")).join(", ");
        return Err(format!("fflags is {:#x}, measured one of {uptime} (the UPTIME clock); MONOTONIC and \
                            WALL timers are not modelled (M46 §7)", e.fflags));
    };
    let want = KeventQos {
        ident: TIMER_IDENT_BASE | tidx as u64,
        filter: EVFILT_TIMER,
        flags: e.flags,
        qos: EVENT_MANAGER_QOS,
        udata: 0,
        fflags: e.fflags,
        xflags: 0,
        data: 0,
        ext: [0; 4],
    };
    first_difference(&e, &want, &["udata", "data", "ext[1]"])?;
    Ok(if e.flags == arm {
        ChangeEntry::TimerAdd { ident: e.ident, deadline: e.data as u64, leeway: e.ext[1], udata: e.udata }
    } else {
        ChangeEntry::TimerDelete { ident: e.ident }
    })
}

/// The event a fired timer knote delivers (xnu `kern_event.c:1822-1905`, `:4427`; spec §2c; t0 M2):
/// the registered flags plus `EV_CLEAR` (`0x35`), `data` 1 (one expiration), `ext[1]` the leeway.
pub fn timer_fired_event(ident: u64, leeway: u64, udata: u64) -> KeventQos {
    KeventQos {
        ident,
        filter: EVFILT_TIMER,
        flags: EV_ADD | EV_ENABLE | EV_ONESHOT | EV_CLEAR,
        qos: EVENT_MANAGER_QOS,
        udata,
        fflags: 0,
        xflags: 0,
        data: 1,
        ext: [0, leeway, 0, 0],
    }
}

/// The event the triggered `EVFILT_USER` knote delivers (xnu `kern_event.c:1972-1986`; t0 M2).
/// libdispatch ignores its content (`event_kevent.c:576-579`): its job is to bring the manager up.
pub const USER_WAKE_EVENT: KeventQos = KeventQos {
    ident: 1,
    filter: EVFILT_USER,
    flags: EV_ADD | EV_CLEAR,
    qos: EVENT_MANAGER_QOS,
    udata: !0x7,
    fflags: 0,
    xflags: 0,
    data: 0,
    ext: [0; 4],
};
```

Where t0 measured a different value (the poke's `qos`, the ident-to-fflags pairing, or the delivered event bytes), use t0's value in the constant **and** in the test's expectation, and say so in the report.

- [ ] **Step 4: Run it to verify it passes, plus the crate's other tests and clippy**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
cargo test -p retrace-arch --no-fail-fast -- --test-threads=1 > $L/t2-green.log 2>&1; echo "exit=$?"
grep -a '^test result:' $L/t2-green.log
cargo clippy -p retrace-arch --all-targets -- -D warnings > $L/t2-clippy.log 2>&1; echo "exit=$?"
```

Expected: `exit=0` for both. `gcdshapes.rs` shows `14 passed`, and every other `test result:` line shows `0 failed`, `kqinit.rs`'s 8 included.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-arch/src/lib.rs crates/retrace-arch/tests/gcdshapes.rs
git commit -m "M46 t2: the validators — kevent_qos's three modelled shapes and KEVENT_RETURN's timer changes, refused by value otherwise"
```

---

### Task 3: The knote table (`retrace-box`)

**Files:**
- Create: `crates/retrace-box/src/kq.rs`
- Modify: `crates/retrace-box/src/lib.rs`:
  - `pub mod kq;` after `pub mod thread;`;
  - the `kq` field in `Box_` (before `excl`) and in `BoxState` (before `excl`);
  - the three `Box_ {…}` literals in `load_with_pac`, `load_dynamic` and `restore`;
  - `checkpoint()` and `from_checkpoint()`;
  - `dbg_internal_state`;
  - `dbg_kq` and `dbg_kq_mut`.
- Modify: `crates/retrace-box/tests/checkpointparity.rs`

**Interfaces:**
- Consumes (Task 2): `retrace_arch::{KeventQos, timer_fired_event, USER_WAKE_EVENT}`.
- Produces, in `pub mod retrace_box::kq`:
  - `enum Manager { None, Bound(usize), Unbound(usize) }`, with `Default` = `None`
  - `struct Timer { deadline: u64, leeway: u64, udata: u64, fired: bool }`
  - `struct WorkqKqueue` (`Default`, `Clone`, `Debug`, `PartialEq`), with:
    - `register_user(&mut self) -> Result<(), String>`
    - `register_memstatus(&mut self, udata: u64) -> Result<(), String>`
    - `trigger_user(&mut self) -> Result<(), String>`
    - `add_timer(&mut self, ident: u64, deadline: u64, leeway: u64, udata: u64) -> Result<(), String>`
    - `delete_timer(&mut self, ident: u64) -> Result<(), String>`
    - `armed_count(&self) -> usize`
    - `earliest_deadline(&self) -> Option<u64>`
    - `fire_due(&mut self, now: u64) -> usize`
    - `has_pending(&self) -> bool`
    - `take_events(&mut self, max: usize) -> Vec<KeventQos>`
    - `manager(&self) -> Manager`
    - `set_manager(&mut self, m: Manager)`
  - `fn tsc_for_deadline(tsc: u64, offset: u64, deadline: u64) -> u64`
- Also produces `Box_::dbg_kq(&self) -> &kq::WorkqKqueue` and `Box_::dbg_kq_mut(&mut self) -> &mut kq::WorkqKqueue` (both `#[doc(hidden)]`).

- [ ] **Step 1: Write the module with its failing tests**

Create `crates/retrace-box/src/kq.rs`:

```rust
//! M46 §3c: the process's workqueue kqueue as libdispatch uses it. It holds the knotes libdispatch
//! registers and the event manager thread that drains them. Pure data with no `Box_` access:
//! `Box_` owns one (`Box_::kq`) and carries it through every rebuild path in `BoxState`, because a
//! mid-run capture cannot re-derive it.
//!
//! Every mutator whose kernel answer the model has not measured returns `Err` naming it, and the
//! box refuses by value (R5).

use std::collections::BTreeMap;

use retrace_arch::{timer_fired_event, KeventQos, USER_WAKE_EVENT};

/// The event manager thread, if there is one.
/// - **`Bound`**: running, or runnable with its events already written.
/// - **`Unbound`**: parked in `workq_kernreturn(THREAD_KEVENT_RETURN)` until a knote activates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Manager {
    #[default]
    None,
    Bound(usize),
    Unbound(usize),
}

/// One `EVFILT_TIMER` knote. `fired` means expired and queued but not yet delivered. It is
/// `ONESHOT`, so delivery drops it (xnu `kern_event.c:4429-4436`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timer {
    pub deadline: u64,
    pub leeway: u64,
    pub udata: u64,
    pub fired: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkqKqueue {
    /// The init's `EVFILT_USER` knote (ident 1): `None` until registered, then `Some(active)`.
    user: Option<bool>,
    /// The memory-pressure knote's udata. It is registered and never activates, because the model
    /// has no memory pressure. That is deterministic, and faithful to a host under none.
    memstatus: Option<u64>,
    /// Timer knotes by ident, so that every iteration, and so delivery order, is by ident.
    timers: BTreeMap<u64, Timer>,
    manager: Manager,
}

impl WorkqKqueue {
    /// M45's init registers the manager's `EVFILT_USER` knote.
    pub fn register_user(&mut self) -> Result<(), String> {
        if self.user.is_some() {
            return Err("a second workqueue-kqueue init: the EVFILT_USER knote (ident 1) is already \
                        registered".into());
        }
        self.user = Some(false);
        Ok(())
    }

    pub fn register_memstatus(&mut self, udata: u64) -> Result<(), String> {
        if let Some(first) = self.memstatus {
            return Err(format!("a second EVFILT_MEMORYSTATUS registration (udata {udata:#x}; the \
                                first carried {first:#x})"));
        }
        self.memstatus = Some(udata);
        Ok(())
    }

    /// A `NOTE_TRIGGER` touch activates the user knote (xnu `filt_usertouch`). Two triggers before a
    /// delivery are one event, because `EV_CLEAR` resets it only at delivery.
    pub fn trigger_user(&mut self) -> Result<(), String> {
        match &mut self.user {
            Some(active) => { *active = true; Ok(()) }
            None => Err("a NOTE_TRIGGER of the EVFILT_USER knote (ident 1) before the init \
                         registered it".into()),
        }
    }

    /// `EV_ADD` arms a timer, or reprograms an armed one (xnu `kern_event.c:1775-1819`).
    pub fn add_timer(&mut self, ident: u64, deadline: u64, leeway: u64, udata: u64) -> Result<(), String> {
        if self.timers.get(&ident).is_some_and(|t| t.fired) {
            return Err(format!("an arm of timer {ident:#x} while its fire is queued, undelivered: what \
                                the kernel does to the queued event is unmeasured"));
        }
        self.timers.insert(ident, Timer { deadline, leeway, udata, fired: false });
        Ok(())
    }

    pub fn delete_timer(&mut self, ident: u64) -> Result<(), String> {
        match self.timers.get(&ident) {
            Some(t) if !t.fired => {
                self.timers.remove(&ident);
                Ok(())
            }
            Some(_) => Err(format!("a disarm of timer {ident:#x} while its fire is queued, \
                                    undelivered: what the kernel does to the queued event is \
                                    unmeasured")),
            None => Err(format!("a disarm of timer {ident:#x}, which is not armed: the kernel answers \
                                 ENOENT as an EV_ERROR event, which is not modelled")),
        }
    }

    /// Timers armed and not yet fired.
    pub fn armed_count(&self) -> usize {
        self.timers.values().filter(|t| !t.fired).count()
    }

    /// The deadline the idle jump lands on (M46 §3e rule 2): the earliest over armed timers only.
    pub fn earliest_deadline(&self) -> Option<u64> {
        self.timers.values().filter(|t| !t.fired).map(|t| t.deadline).min()
    }

    /// Fire every armed timer whose deadline `now` has reached (§3e rule 1; a deadline already
    /// reached is active immediately, xnu `kern_event.c:1711-1751`). Returns how many fired.
    pub fn fire_due(&mut self, now: u64) -> usize {
        let mut n = 0;
        for t in self.timers.values_mut() {
            if !t.fired && t.deadline <= now {
                t.fired = true;
                n += 1;
            }
        }
        n
    }

    /// Is any knote active, so that the manager is wanted?
    pub fn has_pending(&self) -> bool {
        self.user == Some(true) || self.timers.values().any(|t| t.fired)
    }

    /// Deliver up to `max` active knotes, in table order: the user knote first, then fired timers by
    /// ident (M46 §3d). Delivery clears the user knote (`EV_CLEAR`) and drops each timer
    /// (`ONESHOT`). Anything past `max` stays active for the next scan, as the kernel leaves it.
    pub fn take_events(&mut self, max: usize) -> Vec<KeventQos> {
        let mut out = Vec::new();
        if self.user == Some(true) && out.len() < max {
            out.push(USER_WAKE_EVENT);
            self.user = Some(false);
        }
        let due: Vec<u64> = self.timers.iter().filter(|(_, t)| t.fired).map(|(&i, _)| i)
            .take(max.saturating_sub(out.len())).collect();
        for ident in due {
            let t = self.timers.remove(&ident).expect("collected from the map just above");
            out.push(timer_fired_event(ident, t.leeway, t.udata));
        }
        out
    }

    pub fn manager(&self) -> Manager { self.manager }
    pub fn set_manager(&mut self, m: Manager) { self.manager = m; }
}

/// M46 §3e rule 2: the synthetic counter at which the guest's `mach_absolute_time`
/// (`tsc + offset`, wrapping as the guest's add wraps) equals `deadline`. A deadline already
/// reached returns `tsc` unchanged: the clock never moves backwards.
pub fn tsc_for_deadline(tsc: u64, offset: u64, deadline: u64) -> u64 {
    let now = tsc.wrapping_add(offset);
    if deadline > now { tsc.wrapping_add(deadline - now) } else { tsc }
}

#[cfg(test)]
mod tests {
    use super::*;
    use retrace_arch::TIMER_IDENT_BASE;

    const T0: u64 = TIMER_IDENT_BASE;
    const T1: u64 = TIMER_IDENT_BASE | 1;

    fn inited() -> WorkqKqueue {
        let mut k = WorkqKqueue::default();
        k.register_user().unwrap();
        k
    }

    #[test]
    fn a_trigger_before_the_init_is_refused() {
        let err = WorkqKqueue::default().trigger_user().unwrap_err();
        assert!(err.contains("before the init"), "{err}");
    }

    #[test]
    fn a_second_init_or_memory_pressure_registration_is_refused() {
        let mut k = inited();
        assert!(k.register_user().unwrap_err().contains("second"));
        k.register_memstatus(0x6c850).unwrap();
        assert!(k.register_memstatus(0x6c850).unwrap_err().contains("second"));
    }

    #[test]
    fn a_trigger_raises_one_user_event_and_delivery_clears_it() {
        let mut k = inited();
        assert!(!k.has_pending());
        k.trigger_user().unwrap();
        k.trigger_user().unwrap();
        assert_eq!(k.take_events(16), vec![USER_WAKE_EVENT], "two triggers before a delivery are one event");
        assert!(!k.has_pending());
        assert!(k.take_events(16).is_empty());
    }

    /// Review Focus 4.
    #[test]
    fn fired_timers_are_delivered_in_ident_order_after_the_user_event() {
        let mut k = inited();
        k.add_timer(T1, 100, 7, 0xb).unwrap();
        k.add_timer(T0, 100, 5, 0xa).unwrap();
        k.trigger_user().unwrap();
        assert_eq!(k.fire_due(100), 2);
        assert_eq!(k.take_events(16),
            vec![USER_WAKE_EVENT, timer_fired_event(T0, 5, 0xa), timer_fired_event(T1, 7, 0xb)]);
    }

    #[test]
    fn a_delivered_timer_is_dropped() {
        let mut k = inited();
        k.add_timer(T0, 100, 0, 0xa).unwrap();
        k.fire_due(100);
        assert_eq!(k.take_events(16).len(), 1);
        assert_eq!((k.armed_count(), k.has_pending(), k.earliest_deadline()), (0, false, None));
    }

    #[test]
    fn an_arm_of_an_armed_timer_reprograms_it() {
        let mut k = inited();
        k.add_timer(T0, 200, 0, 0xb).unwrap();
        k.add_timer(T0, 100, 0, 0xa).unwrap();
        assert_eq!((k.armed_count(), k.earliest_deadline()), (1, Some(100)));
        assert_eq!(k.fire_due(99), 0);
        assert_eq!(k.fire_due(100), 1);
        assert_eq!(k.take_events(16), vec![timer_fired_event(T0, 0, 0xa)]);
    }

    /// Review Focus 5.
    #[test]
    fn a_change_to_a_timer_whose_fire_is_queued_is_refused() {
        let mut k = inited();
        k.add_timer(T0, 100, 0, 0xa).unwrap();
        k.fire_due(100);
        assert!(k.add_timer(T0, 300, 0, 0xa).unwrap_err().contains("queued"));
        assert!(k.delete_timer(T0).unwrap_err().contains("queued"));
    }

    /// Review Focus 5: a delivered timer is dropped, so a later disarm names a timer that is gone.
    #[test]
    fn a_disarm_of_a_timer_that_is_not_armed_is_refused() {
        let mut k = inited();
        assert!(k.delete_timer(T1).unwrap_err().contains("not armed"));
        k.add_timer(T0, 100, 0, 0xa).unwrap();
        k.fire_due(100);
        k.take_events(16);
        assert!(k.delete_timer(T0).unwrap_err().contains("not armed"));
    }

    /// Review Focus 1: the `KEVENT_RETURN` that arms a timer calls this before it scans, so a
    /// deadline the synthetic clock already passed is delivered by the same return.
    #[test]
    fn a_timer_at_or_before_now_fires_and_a_later_one_does_not() {
        let mut k = inited();
        k.add_timer(T0, 100, 0, 0xa).unwrap();
        k.add_timer(T1, 101, 0, 0xb).unwrap();
        assert_eq!(k.fire_due(100), 1, "a deadline equal to now has been reached");
        assert_eq!(k.armed_count(), 1);
        assert_eq!(k.fire_due(100), 0, "a fired timer does not fire twice");
        assert!(k.has_pending());
    }

    #[test]
    fn the_idle_jump_lands_on_the_deadline_and_never_moves_backwards() {
        assert_eq!(tsc_for_deadline(1000, 50, 2000), 1950, "tsc + offset lands on the deadline");
        assert_eq!(tsc_for_deadline(1000, 50, 1050), 1000, "a deadline equal to now needs no jump");
        assert_eq!(tsc_for_deadline(1000, 50, 10), 1000, "a passed deadline never moves the clock back");
        assert_eq!(tsc_for_deadline(1000, u64::MAX - 499, 600), 1100, "the offset wraps as the guest's add does");
    }

    #[test]
    fn the_earliest_deadline_is_over_armed_timers_only() {
        let mut k = inited();
        k.add_timer(T0, 100, 0, 0xa).unwrap();
        k.add_timer(T1, 300, 0, 0xb).unwrap();
        k.fire_due(100);
        assert_eq!(k.earliest_deadline(), Some(300), "a fired timer is not waited for");
    }
}
```

In `crates/retrace-box/src/lib.rs`, directly after `pub mod thread;`:

```rust
pub mod kq;
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
cargo test -p retrace-box --lib kq:: --no-fail-fast -- --test-threads=1 > $L/t3-kq.log 2>&1; echo "exit=$?"
grep -a -E '^test result:|warning: .*never used' $L/t3-kq.log
```

Expected: `exit=0` and `11 passed`. Until Step 2 uses them, dead-code warnings may name `WorkqKqueue`'s methods; Step 2 clears them.

This module is pure, so there is no RED run against missing code: the tests compile together with it. The failing state this task drives out is Step 3's parity precondition.

- [ ] **Step 2: The field, in all six places, and its accessors**

In `crates/retrace-box/src/lib.rs`:

**`Box_`:** directly before the doc comment `/// M42: the shadow of this vCPU's local exclusive monitor (spec §3a).` on the `excl` field:

```rust
    /// M46 §3c: the workqueue kqueue's knotes and its event manager. This is box state, not trace
    /// state: record and replay rebuild it from the guest's own syscalls, and every rebuild path
    /// carries it (`BoxState`).
    kq: kq::WorkqKqueue,
```

**`BoxState`:** directly before `    // M42: carried because a mid-pair capture cannot re-derive it.`:

```rust
    // M46: carried because a mid-run capture cannot re-derive it: the registrations, arms and fires
    // happened behind the checkpoint. Dropping it would make a seek past a fire replay a manager
    // whose knote table is empty (gcdtimer_e2e's seek test, and its control).
    pub kq: kq::WorkqKqueue,
```

**The three literals** in `load_with_pac`, `load_dynamic` and `restore`: replace every occurrence of `canary_disturbances: 0, excl: None` with `canary_disturbances: 0, kq: kq::WorkqKqueue::default(), excl: None`. There are exactly three, so use `replace_all`. `restore()`'s is landmark 0, where the table is empty, which is right.

**`checkpoint()`:** directly before `            excl: self.excl.clone(),`:

```rust
            kq: self.kq.clone(),
```

**`from_checkpoint()`:** directly before `            excl: state.excl.clone(),`:

```rust
            kq: state.kq.clone(),
```

**`dbg_internal_state`:** append ` kq={:?}` to the format string and `self.kq` to its arguments:

```rust
    pub fn dbg_internal_state(&self) -> String {
        format!("reservations={:?} mmap_next={:#x} bootstrap_port={:?} cache_installed={} last_far={:#x} synthetic_tsc={:#x} cache_refault_ipa={:#x} cache_refault_count={} pac_enabled={} kq={:?}",
            self.reservations, self.mmap_next, self.bootstrap_port, self.cache.is_some(),
            self.last_far, self.synthetic_tsc, self.cache_refault_ipa, self.cache_refault_count,
            self.pac_enabled, self.kq)
    }
```

**The accessors:** directly after `dbg_internal_state`:

```rust
    /// Test-only (M46): the workqueue kqueue, for `checkpointparity.rs` and the box-level manager
    /// tests.
    #[doc(hidden)]
    pub fn dbg_kq(&self) -> &kq::WorkqKqueue { &self.kq }

    /// Test-only (M46): stage knote-table state on a static box, the way `checkpointparity.rs`
    /// stages every other field through a public method.
    #[doc(hidden)]
    pub fn dbg_kq_mut(&mut self) -> &mut kq::WorkqKqueue { &mut self.kq }
```

- [ ] **Step 3: The parity row, test first**

In `crates/retrace-box/tests/checkpointparity.rs`:
- **The row.** In `assert_checkpoint_parity`, directly after `    let excl = b.dbg_excl();`, add `    let kq = b.dbg_kq().clone();`. Directly after the line `    assert_eq!(r.dbg_excl(), excl, "{label}: exclusive-monitor shadow (M42)");`, add:

```rust
    assert_eq!(r.dbg_kq(), &kq, "{label}: the workqueue kqueue (M46)");
```

- **The staging.** In `a_checkpointed_box_with_rich_state_matches_the_box_it_came_from`, directly before `    // PRECONDITIONS. Without these the comparison below is Default == Default.`:

```rust
    // The workqueue kqueue (M46): a registered user knote with a trigger pending, a memory-pressure
    // registration, an armed timer and a parked manager, so every field is off its default.
    {
        let kq = b.dbg_kq_mut();
        kq.register_user().unwrap();
        kq.trigger_user().unwrap();
        kq.register_memstatus(0x6c850).unwrap();
        kq.add_timer(retrace_arch::TIMER_IDENT_BASE, 0x1_2345_6789, 0x100, 0xAB00).unwrap();
        kq.set_manager(retrace_box::kq::Manager::Unbound(1));
    }
```

- **The precondition.** Directly after `    assert_eq!(b.tpidrro_el0(), 0xDEAD_0000, "precondition: tpidrro_el0 staged");`:

```rust
    assert_ne!(b.dbg_kq(), &retrace_box::kq::WorkqKqueue::default(),
        "precondition: a non-default knote table, or the M46 row compares Default == Default");
```

- **The doc.** Add one line to the test's doc comment: `/// The workqueue kqueue (M46) is staged through \`dbg_kq_mut\`, the one field no guest-free call reaches.`

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 > $L/t3-box.log 2>&1; echo "exit=$?"
grep -a '^test result:' $L/t3-box.log
cargo clippy --workspace --all-targets -- -D warnings > $L/t3-clippy.log 2>&1; echo "exit=$?"
```

Expected: both `exit=0`, and every `test result:` line with `0 failed`: `checkpointparity`, `restoreparity`, `checkpoint`, `threads` and the lib's `kq::` tests. Clippy is clean, because every `WorkqKqueue` method is now reached from a test or the box.

If clippy flags a method that only Task 4 will use, such as `fire_due` or `take_events`, as dead in non-test code, that is the rule working. `pub` items of a `pub mod` are not dead code, so this should not happen. If it does, report it and do not add `#[allow]`.

- [ ] **Step 4: Commit**

```bash
git add crates/retrace-box/src/kq.rs crates/retrace-box/src/lib.rs crates/retrace-box/tests/checkpointparity.rs
git commit -m "M46 t3: the workqueue kqueue — a pure knote table, carried through every rebuild path, with its parity row"
```

- [ ] **Step 5: The control (on the committed tree)**

In `from_checkpoint`, replace `kq: state.kq.clone(),` with `kq: kq::WorkqKqueue::default(),`. Run `cargo test -p retrace-box --test checkpointparity --no-fail-fast -- --test-threads=1`.

Expected: the rich tier fails on `the workqueue kqueue (M46)` or on `internal bookkeeping`. Restore with `git checkout -- crates/retrace-box/src/lib.rs`.

---

### Task 4: The manager model and the arms, test-first

**Files:**
- Modify: `crates/retrace-box/src/thread.rs`: `stack_of` and `unpark` after `state_of`; `BlockReason::Parked`'s doc
- Modify: `crates/retrace-box/src/lib.rs`:
  - four constants after `WQ_ENTRY_FLAGS_FRESH`;
  - `try_workq_kernreturn` (the old name becomes its wrapper);
  - `park_on_svc`, extracted from `guest_workq_park`;
  - `guest_kevent_qos -> Result`;
  - the manager methods after it;
  - `schedule_after_block`.
- Create: `crates/retrace-box/tests/kqmanager.rs`
- Modify: `crates/retrace-core/src/lib.rs`: the `workq_kernreturn` and `kevent_qos` record arms and replay mirrors
- Modify: `crates/retrace/tests/kqinit_e2e.rs:69`
- Modify: `crates/retrace/tests/gcdtimer_e2e.rs`: `kevent_returns` and test 1

**Interfaces:**
- Consumes:
  - Task 1: `now_guest`, `timebase_offset`, `AFTER_DYN`, `records_and_replays`, `events`.
  - Task 2: `kevent_qos_shape`, `KeventShape`, `kevent_return_change`, `ChangeEntry`, `WQOPS_THREAD_KEVENT_RETURN`, `WQ_FLAG_THREAD_*`, `WQ_KEVENT_LIST_LEN`, `KEVENT_QOS_SIZE`.
  - Task 3: `kq::{WorkqKqueue, Manager, tsc_for_deadline}`, `dbg_kq`.
- Produces:
  - `pub fn Box_::guest_kevent_qos(&mut self, args: [u64; 8]) -> Result<u64, String>` (was `&self -> u64`)
  - `pub fn Box_::try_workq_kernreturn(&mut self, args: [u64; 8]) -> Result<u64, String>`; `guest_workq_kernreturn` keeps its signature and panics on `Err`
  - `pub fn ThreadTable::stack_of(&self, tid: usize) -> (u64, u64)`
  - `pub fn ThreadTable::unpark(&mut self, tid: usize)`
  - private: `request_manager`, `spawn_manager`, `enter_manager`, `fill_manager_upcall`, `write_va_committing`, `guest_workq_kevent_return`, `park_on_svc`, `fire_due_timers`, `fmt_args`
  - `gcdtimer_e2e.rs`: `kevent_returns(trace) -> Vec<(usize, u32, u64)>`

- [ ] **Step 1: The failing gate: test 1, and the box-level manager tests**

Append to `crates/retrace/tests/gcdtimer_e2e.rs`:

```rust

/// Every `workq_kernreturn(THREAD_KEVENT_RETURN)` landmark, as (index, thread, ret).
fn kevent_returns(trace: &Path) -> Vec<(usize, u32, u64)> {
    events(trace).into_iter().filter_map(|(i, e)| match e {
        Event::Syscall { num, args, ret, thread, .. }
            if num == retrace_arch::SYS_WORKQ_KERNRETURN && args[0] == retrace_arch::WQOPS_THREAD_KEVENT_RETURN =>
            Some((i, thread, ret)),
        _ => None,
    }).collect()
}

/// Spec §3f test 1. A `dispatch_after` records to exit 0 with its markers, and two replays are
/// byte-identical. Before M46 the recorder stopped at libdispatch's second `kevent_qos`, so the
/// markers are the difference. The manager's KEVENT_RETURN landmarks, on a nonzero thread, are the
/// model's footprint in the trace.
#[test]
fn a_dispatch_after_fires_on_the_synthetic_clock_and_replays() {
    let (rec, trace) = records_and_replays(retrace_guest::AFTER_DYN, &[]);
    let out = String::from_utf8_lossy(&rec.stdout);
    assert!(out.starts_with("fired cell 0x") && out.ends_with("\nfired\ndone\n"), "stdout: {out:?}");
    let rets = kevent_returns(&trace);
    let manager = rets.first().map(|r| r.1)
        .expect("the manager must hand its changes back through KEVENT_RETURN");
    assert_ne!(manager, 0, "the manager is a workqueue thread the box started, never main");
    assert!(rets.iter().all(|r| r.1 == manager), "one manager thread serves the whole run: {rets:?}");
    // One fire needs a handful of returns (t0 M3 counts the native ones). A manager whose "now"
    // lags its deadline re-arms after every fire and spins through returns until the clocks meet:
    // the failure R7 exists to prevent, and one an exit code cannot show.
    assert!(rets.len() <= 8,
        "{} KEVENT_RETURNs for one dispatch_after: the manager re-arms a timer it sees as not yet due", rets.len());
}
```

Create `crates/retrace-box/tests/kqmanager.rs`:

```rust
//! M46 §3d, box level. The event manager's lifecycle on a static box, with the kernel's side
//! driven by hand the way `threads.rs` drives M18's workers. These tests pin the paths the
//! fixtures may not reach (Review Focus 1–2): a redelivery, a poke while bound, the reuse
//! re-entry, and the refusals. No timer is armed here, because a timer reads the guest clock, and
//! a static box has no commpage. `gcdtimer_e2e` covers the timers end to end.
//!
//! The expected flag words are t0 M2's: `0x3C_4008` first use, `0x1E_4008` reuse, `0x1E_0000`
//! redelivery.
use retrace_arch::{KeventQos, EVENT_MANAGER_QOS, EVFILT_TIMER, EV_ADD, EV_ENABLE, EV_ONESHOT, KQINIT,
                   MANAGER_POKE, TIMER_IDENT_BASE, USER_WAKE_EVENT};
use retrace_box::kq::Manager;
use retrace_box::thread::{BlockReason, ThreadState};
use retrace_box::Box_;

const ENTRY: u64 = 0x2222;
const PTHSIZE: u64 = 0x2A10;

fn tb() -> Box_ {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::SPINLOOP).unwrap());
    Box_::load(&loaded)
}

/// A static box after `bsdthread_register` and M45's init: libdispatch's state when it first pokes
/// its manager. `e` is a scratch entry on the static stack page (the MMU is off, so VA == IPA).
fn inited() -> (Box_, u64) {
    let mut b = tb();
    b.guest_bsdthread_register([0x1111, ENTRY, PTHSIZE, 0, 0, 0, 0, 0]);
    let e = b.stack_top() - 0x200;
    b.poke_guest(e, &KQINIT.to_bytes());
    assert_eq!(b.guest_kevent_qos([0xffff_ffff, e, 1, 0, 0, 0, 0, 0x21]), Ok(0));
    (b, e)
}

fn poke(b: &mut Box_, e: u64) -> Result<u64, String> {
    b.poke_guest(e, &MANAGER_POKE.to_bytes());
    b.guest_kevent_qos([0xffff_ffff, e, 1, e + 0x100, 16, 0, 0, 0x23])
}

/// `inited()` plus the first poke: the manager spawned as thread 1. Returns its pthread.
fn spawned() -> (Box_, u64, u64) {
    let (mut b, e) = inited();
    assert_eq!(poke(&mut b, e), Ok(0));
    let pthread = b.threads().ctx_of(1).regs.x[0];
    (b, e, pthread)
}

fn kevent_return(list: u64, n: u64) -> [u64; 8] { [0x40, list, n, 0, 0, 0, 0, 0] }

#[test]
fn a_poke_spawns_the_manager_with_the_first_use_register_block_and_the_user_event() {
    let (b, _, pthread) = spawned();
    assert_eq!(b.threads().len(), 2);
    assert_eq!(b.dbg_kq().manager(), Manager::Bound(1));
    assert_eq!(b.threads().current(), 0, "the poke returns to main: nothing switches until main blocks");
    let ctx = b.threads().ctx_of(1);
    assert_eq!((ctx.regs.pc, ctx.elr), (ENTRY, ENTRY), "entered at the registered wqthread entry");
    assert_eq!(ctx.regs.x[1], 0x0BAD_7000 | 1, "the kport, GUEST_THREAD_PORT_BASE | tid");
    assert_eq!(ctx.regs.x[2], pthread - 0x8_0000, "the stack's low end, 512 KiB below the struct");
    assert_eq!(ctx.regs.x[3], pthread - 0x480, "the event list, at self - 16 x 72");
    assert_eq!(ctx.regs.x[4], 0x3C_4008, "first use: TSD_BASE_SET|EVENT_MANAGER|KEVENT|NEWSPI|PRIO_QOS|8");
    assert_eq!(ctx.regs.x[5], 1, "one event");
    assert!(ctx.regs.x[6..].iter().all(|&r| r == 0), "every other register is zero, as the kernel sets it");
    assert_eq!(ctx.regs.sp_el0, pthread - 0x480, "with no data payload, sp is the list");
    assert_eq!(ctx.tpidrro_el0, pthread + 0xe0, "TPIDRRO_EL0 = pthread + PTHREAD_TSD_OFF");
    assert_eq!(b.read_bytes_for_test(pthread - 0x480, 72), USER_WAKE_EVENT.to_bytes());
    assert!(!b.dbg_kq().has_pending(), "the trigger was delivered in the upcall");
}

/// Review Focus 2.
#[test]
fn a_second_poke_while_the_manager_is_bound_spawns_nothing() {
    let (mut b, e, _) = spawned();
    assert_eq!(poke(&mut b, e), Ok(0));
    assert_eq!(b.threads().len(), 2, "no second manager");
    assert_eq!(b.dbg_kq().manager(), Manager::Bound(1));
    assert!(b.dbg_kq().has_pending(), "the trigger waits for the manager's next scan");
}

#[test]
fn a_kevent_return_with_nothing_pending_parks_the_manager_on_its_svc() {
    let (mut b, _, pthread) = spawned();
    b.switch_to_thread(1);
    assert_eq!(b.try_workq_kernreturn(kevent_return(pthread - 0x480, 0)), Ok(0));
    assert_eq!(b.threads().state_of(1), ThreadState::Blocked(BlockReason::Parked));
    assert_eq!(b.dbg_kq().manager(), Manager::Unbound(1));
    b.set_x0_err_and_return(0, false);
    b.schedule_after_block();
    assert_eq!(b.threads().current(), 0, "the park hands the vCPU back to main");
    assert_eq!(b.threads().ctx_of(1).elr, ENTRY - 4, "parked on its svc, never resumable into a return");
}

#[test]
fn a_poke_to_a_parked_manager_reenters_it_with_the_reuse_flags() {
    let (mut b, e, pthread) = spawned();
    b.switch_to_thread(1);
    b.try_workq_kernreturn(kevent_return(pthread - 0x480, 0)).unwrap();
    b.set_x0_err_and_return(0, false);
    b.schedule_after_block();
    assert_eq!(poke(&mut b, e), Ok(0));
    assert_eq!(b.threads().state_of(1), ThreadState::Runnable, "unparked");
    assert_eq!(b.threads().len(), 2, "re-entered, not replaced");
    assert_eq!(b.dbg_kq().manager(), Manager::Bound(1));
    let ctx = b.threads().ctx_of(1);
    assert_eq!((ctx.regs.pc, ctx.regs.x[0], ctx.regs.x[3], ctx.regs.x[5]), (ENTRY, pthread, pthread - 0x480, 1));
    assert_eq!(ctx.regs.x[4], 0x1E_4008, "reuse: REUSE in place of TSD_BASE_SET");
    assert_eq!(b.read_bytes_for_test(pthread - 0x480, 72), USER_WAKE_EVENT.to_bytes());
}

/// Review Focus 1: events pending at the return are redelivered on the same thread, through the
/// ordinary return write. The box installs the block, and `set_x0_err_and_return(self)` completes
/// it, which resolves spec §3d's hazard.
#[test]
fn a_kevent_return_with_a_trigger_pending_redelivers_on_the_same_thread() {
    let (mut b, e, pthread) = spawned();
    assert_eq!(poke(&mut b, e), Ok(0)); // pending while bound
    b.switch_to_thread(1);
    let rc = b.try_workq_kernreturn(kevent_return(pthread - 0x480, 0)).unwrap();
    assert_eq!(rc, pthread, "the redelivery returns self, so the return write sets x0 = self");
    b.set_x0_err_and_return(rc, false);
    assert_eq!(b.threads().state_of(1), ThreadState::Runnable, "not parked");
    assert_eq!(b.dbg_kq().manager(), Manager::Bound(1));
    let ctx = b.thread_ctx(1).unwrap(); // the current thread: read live
    assert_eq!((ctx.regs.pc, ctx.regs.x[0], ctx.regs.x[5], ctx.regs.sp_el0), (ENTRY, pthread, 1, pthread - 0x480));
    assert_eq!(ctx.regs.x[4], 0x1E_0000, "redelivery: REUSE|EVENT_MANAGER|KEVENT|NEWSPI, no PRIO_QOS");
    assert_eq!(b.read_bytes_for_test(pthread - 0x480, 72), USER_WAKE_EVENT.to_bytes());
}

#[test]
fn a_kevent_return_from_a_thread_that_is_not_the_bound_manager_is_refused() {
    let (mut b, _, pthread) = spawned();
    let err = b.try_workq_kernreturn(kevent_return(pthread - 0x480, 0)).unwrap_err();
    assert!(err.contains("from thread 0, which is not the bound event manager"), "{err}");
}

/// M46 §7: the refusal names the fflags, and it comes before any clock read, so it reaches a box
/// with no commpage.
#[test]
fn a_kevent_return_carrying_a_wall_timer_is_refused_naming_its_fflags() {
    let (mut b, _, pthread) = spawned();
    b.switch_to_thread(1);
    let wall = KeventQos { ident: TIMER_IDENT_BASE | 6, filter: EVFILT_TIMER, flags: EV_ADD | EV_ENABLE | EV_ONESHOT,
                           qos: EVENT_MANAGER_QOS, udata: 0x6c850, fflags: 0x9c, xflags: 0, data: 1, ext: [0; 4] };
    b.poke_guest(pthread - 0x480, &wall.to_bytes());
    let err = b.try_workq_kernreturn(kevent_return(pthread - 0x480, 1)).unwrap_err();
    assert!(err.contains("M46: unmeasured KEVENT_RETURN change: changelist[0].fflags is 0x9c"), "{err}");
}
```

Then run:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
cargo test -p retrace --test gcdtimer_e2e --no-fail-fast -- --test-threads=1 > $L/t4-red-e2e.log 2>&1; echo "exit=$?"
grep -a -E '^test |^test result:|M45: unmeasured kevent_qos shape' $L/t4-red-e2e.log | head
cargo test -p retrace-box --test kqmanager --no-fail-fast -- --test-threads=1 > $L/t4-red-box.log 2>&1; echo "exit=$?"
grep -a -E 'error\[E' $L/t4-red-box.log | head -3
```

Expected:
- `gcdtimer_e2e`: `exit=101`, `1 passed; 1 failed`. The new test's record exits 101 with `M45: unmeasured kevent_qos shape: x3 (eventlist)`, the second call.
- `kqmanager`: a compile error, because `guest_kevent_qos` returns `u64` and `try_workq_kernreturn` does not exist.

- [ ] **Step 2: The thread table**

In `crates/retrace-box/src/thread.rs`, directly after `pub fn state_of(&self, tid: usize) -> ThreadState { self.threads[tid].state }`:

```rust
    /// `(base, len)` of `tid`'s stack. M46's manager re-entry reads the base: it is the `x2` a
    /// workqueue upcall carries.
    pub fn stack_of(&self, tid: usize) -> (u64, u64) { self.threads[tid].stack }

    /// M46: make a parked workqueue thread runnable again. This is the reuse wake
    /// `BlockReason::Parked`'s doc reserved, used only for the event manager. Asserts the thread was
    /// parked, because waking anything else here would be a scheduling bug.
    pub fn unpark(&mut self, tid: usize) {
        assert_eq!(self.threads[tid].state, ThreadState::Blocked(BlockReason::Parked),
            "M46: unpark of thread {tid}, which is not parked");
        self.threads[tid].state = ThreadState::Runnable;
    }
```

In the doc comment of `BlockReason::Parked`, replace the paragraph that begins `/// **Keyless, and nothing wakes it.**` and ends `/// \`pick_next\` hand the vCPU back to main and lets the guest finish.` with:

```rust
    /// **Keyless.** A plain worker parked here is never woken: M18's scope excludes thread reuse, so
    /// it stays parked for the rest of the run, which is what makes `pick_next` hand the vCPU back to
    /// main. The one exception since M46 is the event manager, parked in
    /// `workq_kernreturn(THREAD_KEVENT_RETURN)`: a knote activation re-enters it through
    /// `ThreadTable::unpark` with a fresh register block (`Box_::request_manager`).
```

- [ ] **Step 3: The box model**

In `crates/retrace-box/src/lib.rs`, directly after the `WQ_ENTRY_FLAGS_FRESH` constant and its doc comment:

```rust

/// M46 §2c: the entry flags of the event manager's FIRST upcall:
/// `TSD_BASE_SET | EVENT_MANAGER | KEVENT | NEWSPI | PRIO_QOS | 8` = `0x3C_4008` (t0 M2). QoS 8 is
/// the manager's (xnu `pthread_workqueue.c:714`).
const WQ_ENTRY_FLAGS_MANAGER_FRESH: u64 = (retrace_arch::WQ_FLAG_THREAD_TSD_BASE_SET
    | retrace_arch::WQ_FLAG_THREAD_EVENT_MANAGER | retrace_arch::WQ_FLAG_THREAD_KEVENT
    | retrace_arch::WQ_FLAG_THREAD_NEWSPI | retrace_arch::WQ_FLAG_THREAD_PRIO_QOS) as u64 | 8;
/// M46 §2c: a parked manager re-entered: `REUSE` in place of `TSD_BASE_SET`, `0x1E_4008` (t0 M2).
const WQ_ENTRY_FLAGS_MANAGER_REUSE: u64 = (retrace_arch::WQ_FLAG_THREAD_REUSE
    | retrace_arch::WQ_FLAG_THREAD_EVENT_MANAGER | retrace_arch::WQ_FLAG_THREAD_KEVENT
    | retrace_arch::WQ_FLAG_THREAD_NEWSPI | retrace_arch::WQ_FLAG_THREAD_PRIO_QOS) as u64 | 8;
/// M46 §2c: a redelivery from inside `KEVENT_RETURN`, with no `PRIO_QOS` and no QoS byte:
/// `0x1E_0000` (xnu `pthread_workqueue.c:3695-3703`; t0 M2).
const WQ_ENTRY_FLAGS_MANAGER_REDELIVER: u64 = (retrace_arch::WQ_FLAG_THREAD_REUSE
    | retrace_arch::WQ_FLAG_THREAD_EVENT_MANAGER | retrace_arch::WQ_FLAG_THREAD_KEVENT
    | retrace_arch::WQ_FLAG_THREAD_NEWSPI) as u64;
/// M46 §2c: a kevent upcall's events sit at `self − 16 × 72` (libpthread `kern_support.c:887-913`).
const WQ_KEVENT_LIST_OFF: u64 = (retrace_arch::WQ_KEVENT_LIST_LEN * retrace_arch::KEVENT_QOS_SIZE) as u64;
```

**Replace `guest_workq_kernreturn`'s signature line and body.** Keep its doc comment, and append this paragraph to it: `/// M46: \`try_workq_kernreturn\` is the dispatch; this wrapper, which panics on its \`Err\`, keeps the M18 tests' signature.`

```rust
    pub fn guest_workq_kernreturn(&mut self, args: [u64; 8]) -> u64 {
        self.try_workq_kernreturn(args).unwrap_or_else(|m| panic!("{m}"))
    }

    /// The dispatch both arms call (M46 §3g). An `Err` is a refusal: the record arm panics with it,
    /// and the replay mirror reports it as a divergence. M18's per-opcode asserts inside
    /// REQTHREADS and THREAD_RETURN still panic on both sides.
    pub fn try_workq_kernreturn(&mut self, args: [u64; 8]) -> Result<u64, String> {
        // libdispatch configuring the workqueue for dispatch. Carries a guest pointer in `args[1]`
        // (measured `0x27ff6a8`) that Stage 2b needs and Stage 2a only has to not forward.
        const WQOPS_SETUP_DISPATCH: u64 = 0x400;
        // libdispatch asking for worker threads. Stage 2b's entry point; Stage 2a's wall.
        const WQOPS_QUEUE_REQTHREADS: u64 = 0x20;
        // A worker parking once its dispatch callback has returned. MEASURED by Stage 2b Task 1
        // (§3d) two ways: `__pthread_wqthread` at `0x2e84 mov w0, #0x4` … `bl ___workq_kernreturn`,
        // and a live host breakpoint on the last call before exit showing `x0=4, x1=x2=x3=0`.
        const WQOPS_THREAD_RETURN: u64 = 0x4;

        match args[0] {
            WQOPS_SETUP_DISPATCH => Ok(0),
            WQOPS_QUEUE_REQTHREADS => Ok(self.guest_workq_reqthreads(args)),
            WQOPS_THREAD_RETURN => Ok(self.guest_workq_park(args)),
            retrace_arch::WQOPS_THREAD_KEVENT_RETURN => self.guest_workq_kevent_return(args),
            other => Err(format!(
                "M18 Stage 2b: unmeasured workq_kernreturn opcode {other:#x} — only \
                 SETUP_DISPATCH ({WQOPS_SETUP_DISPATCH:#x}), REQTHREADS \
                 ({WQOPS_QUEUE_REQTHREADS:#x}), THREAD_RETURN ({WQOPS_THREAD_RETURN:#x}) and, since \
                 M46, THREAD_KEVENT_RETURN ({:#x}) have ever been observed (M18 Task 6; Stage 2b \
                 Task 1 §3d; M46 t0 M3). Measure what issues this one before modelling it; a \
                 guessed opcode silently corrupts the guest's workqueue state. args={args:#x?}",
                retrace_arch::WQOPS_THREAD_KEVENT_RETURN)),
        }
    }
```

**`park_on_svc`.** In `guest_workq_park`, replace the four lines from `        self.threads.block(thread::BlockReason::Parked);` through `        self.vcpu.set_sys(sysreg::ELR_EL1, elr.wrapping_sub(4)).unwrap();`, including the comment between them, with:

```rust
        self.park_on_svc();
```

Directly after `guest_workq_park`'s closing `}`:

```rust

    /// Block the current thread `Parked` and rewind its `ELR_EL1` onto the `svc`: the two moves
    /// `guest_workq_park`'s doc explains. Both dispatch arms call `set_x0_err_and_return` after the
    /// box returns, which sets `PC = ELR_EL1`, so the rewind is what keeps the parked context from
    /// being resumable into a return. A64 instructions are 4 bytes, and `ELR_EL1` on an SVC trap
    /// holds the address of the one AFTER the `svc`. `wrapping_sub`, because an ELR this low is a
    /// broken box, not a case for this helper to police. Shared by a worker's THREAD_RETURN and the
    /// manager's empty KEVENT_RETURN (M46).
    fn park_on_svc(&mut self) {
        self.threads.block(thread::BlockReason::Parked);
        let elr = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
        self.vcpu.set_sys(sysreg::ELR_EL1, elr.wrapping_sub(4)).unwrap();
    }
```

**`guest_kevent_qos`.** Replace the whole method, doc comment included, with:

```rust
    /// `kevent_qos(kq, changelist, nchanges, eventlist, nevents, data_out, data_available, flags)`
    /// with `KEVENT_FLAG_WORKQ`: libdispatch talking to the process's workqueue kqueue (M45, M46).
    ///
    /// **Emulated, never forwarded**, for the reason `guest_workq_open` documents, one level up. With
    /// `KEVENT_FLAG_WORKQ`, xnu resolves `kq` to the process's workqueue kqueue, which is RETRACE's
    /// own (M44 t0 M1). It cannot be refused with an errno either: libdispatch
    /// `DISPATCH_CLIENT_CRASH`es on any errno but `EINTR` (`event_kevent.c:700-709`).
    ///
    /// **Three shapes are modelled** (`retrace_arch::kevent_qos_shape`), each against the knote
    /// table (M46 §3d):
    /// - M45's init registers the manager's `EVFILT_USER` knote;
    /// - libdispatch's memory-pressure source registers, and never activates;
    /// - the manager poke triggers the user knote and requests the manager.
    ///
    /// Each returns 0 and writes nothing: under `ERROR_EVENTS` the kernel copies out only errors
    /// (§2a). Any other shape, or a table state the kernel's answer is unmeasured for, is refused by
    /// value, naming the field. `Err` carries the record arm's panic text; the replay mirror wraps it
    /// as a divergence (§3g).
    ///
    /// The entry is read through the guest's own stage-1 walk, page by page (`read_va_prefix`).
    /// Deterministic: the inputs are `args`, 72 bytes of guest memory and the knote table, which
    /// record and replay hold identically.
    pub fn guest_kevent_qos(&mut self, args: [u64; 8]) -> Result<u64, String> {
        let entry = self.read_va_prefix(args[1], retrace_arch::KEVENT_QOS_SIZE);
        let shape = retrace_arch::kevent_qos_shape(args, &entry).map_err(|why| format!(
            "M46: unmeasured kevent_qos shape: {why}. Modelled: libdispatch's workqueue-kqueue init, \
             its memory-pressure registration and its manager poke (M46 §2a-§2b). Measure what \
             issues this one before modelling it; a guessed kevent silently corrupts libdispatch's \
             event state. args=[{}]", Self::fmt_args(args)))?;
        let table = |why: String| format!(
            "M46: kevent_qos against the knote table: {why}. args=[{}]", Self::fmt_args(args));
        match shape {
            retrace_arch::KeventShape::Init => self.kq.register_user().map_err(table)?,
            retrace_arch::KeventShape::MemoryStatusAdd { udata } => self.kq.register_memstatus(udata).map_err(table)?,
            retrace_arch::KeventShape::ManagerPoke => {
                self.kq.trigger_user().map_err(table)?;
                self.request_manager();
            }
        }
        Ok(0)
    }

    /// `args` as `0x…,0x…` on one line (M45 T3-e: a refusal must fit a log line).
    fn fmt_args(args: [u64; 8]) -> String {
        args.map(|a| format!("{a:#x}")).join(",")
    }

    /// M46 §3d: a knote activated, so the manager is wanted. With no manager thread, spawn one;
    /// with a parked one, re-enter it; with a bound one, do nothing, because its next
    /// `KEVENT_RETURN` scan collects the events. A spawned or re-entered manager is Runnable and
    /// runs when the current thread blocks: the cooperative rule, unchanged.
    fn request_manager(&mut self) {
        if !self.kq.has_pending() { return; }
        match self.kq.manager() {
            kq::Manager::Bound(_) => {}
            kq::Manager::None => self.spawn_manager(),
            kq::Manager::Unbound(tid) => {
                self.threads.unpark(tid);
                let pthread = self.pthread_of(tid).expect("a parked manager has a pthread");
                let stack_base = self.threads.stack_of(tid).0;
                self.enter_manager(tid, pthread, stack_base, WQ_ENTRY_FLAGS_MANAGER_REUSE);
            }
        }
    }

    /// M46: the first manager request builds a fresh workqueue thread on M18's worker path
    /// (`place_worker_stack`). The same placement, the same cursor, and so deterministic with
    /// nothing recorded.
    fn spawn_manager(&mut self) {
        let pthsize = self.pthread_size.expect(
            "M46: a knote activated with no registered pthread size — bsdthread_register captures it, \
             and every dynamic guest registers at startup") as u64;
        let (stack_base, stack_top, pthread) = self.place_worker_stack(pthsize);
        let tid = self.threads.len();
        // The requesting thread's EL0 PSTATE, as `guest_workq_reqthreads` gives a worker (M14's
        // lesson: zeroed()'s 0 is not an EL0 PSTATE to resume into).
        let ctx = thread::ThreadCtx { spsr: self.spsr(), ..thread::ThreadCtx::zeroed() };
        let spawned = self.threads.spawn(ctx, (stack_base, stack_top - stack_base));
        assert_eq!(spawned, tid, "ThreadTable::spawn appends; the kport assumes it");
        self.enter_manager(tid, pthread, stack_base, WQ_ENTRY_FLAGS_MANAGER_FRESH);
    }

    /// M46 §2c, §3d: write the active knotes into `tid`'s event list and give it a fresh upcall
    /// register block, which is libpthread's `workq_set_register_state`. Every general register is
    /// zeroed except the six the entry reads.
    ///
    /// **The current thread's block goes onto the vCPU; any other thread's goes into its saved
    /// context.** `switch_to_thread` returns early for the current thread, so a block written only
    /// to the table would never load. That case is a manager re-entered in the same settle that
    /// parked it, or a redelivery from inside its own `KEVENT_RETURN` (Review Focus 3). A later
    /// switch away saves the live vCPU, the block included. A syscall exit has already cleared the
    /// exclusive shadow, so there is none to carry.
    fn enter_manager(&mut self, tid: usize, pthread: u64, stack_base: u64, flags: u64) {
        let entry = self.wq_thread_pc.expect(
            "M46: a manager upcall with no registered wqthread entry — refusing to enter an invented \
             address; bsdthread_register captures it at startup");
        let n = self.fill_manager_upcall(pthread);
        let list = pthread - WQ_KEVENT_LIST_OFF;
        let mut ctx = self.thread_ctx(tid).expect("the manager is in the thread table");
        ctx.regs.x = [0; 31];
        ctx.regs.pc = entry;
        ctx.elr = entry;
        ctx.regs.cpsr = ctx.spsr;
        ctx.regs.x[0] = pthread;
        ctx.regs.x[1] = (GUEST_THREAD_PORT_BASE | tid as u32) as u64;
        ctx.regs.x[2] = stack_base;
        ctx.regs.x[3] = list;
        ctx.regs.x[4] = flags;
        ctx.regs.x[5] = n as u64;
        // With no mach-message payload the stack top is the list itself (spec §2c).
        ctx.regs.sp_el0 = list;
        ctx.tpidrro_el0 = pthread + PTHREAD_TSD_OFF;
        if tid == self.threads.current() {
            self.load_ctx(&ctx);
        } else {
            *self.threads.ctx_mut(tid) = ctx;
        }
        self.kq.set_manager(kq::Manager::Bound(tid));
    }

    /// M46 §3d: deliver up to 16 active knotes into the manager's event list at `self − 0x480`.
    /// These are box writes computed from box state, recomputed on both sides and never recorded,
    /// like `guest_bsdthread_create`'s kport write. A fresh manager's list page is still a bare
    /// reservation, so the write commits it.
    fn fill_manager_upcall(&mut self, pthread: u64) -> usize {
        let events = self.kq.take_events(retrace_arch::WQ_KEVENT_LIST_LEN);
        assert!(!events.is_empty(), "M46: a manager upcall with no active knote — request_manager checks has_pending first");
        let list = pthread - WQ_KEVENT_LIST_OFF;
        for (i, e) in events.iter().enumerate() {
            let at = list + (i * retrace_arch::KEVENT_QOS_SIZE) as u64;
            self.write_va_committing(at, &e.to_bytes())
                .unwrap_or_else(|m| panic!("M46: writing the manager's event list: {m}"));
        }
        events.len()
    }

    /// Write `bytes` at guest VA `va`, page by page through the guest's own stage-1 walk,
    /// demand-committing a reserved page the way a guest store would (`commit_reserved_page`).
    fn write_va_committing(&mut self, va: u64, bytes: &[u8]) -> Result<(), String> {
        let mut done = 0;
        while done < bytes.len() {
            let a = va + done as u64;
            let ipa = self.va_to_ipa(a).ok_or_else(|| format!("{a:#x} does not translate"))?;
            let n = (((a | (GRANULE as u64 - 1)) + 1 - a) as usize).min(bytes.len() - done);
            if self.host_span(ipa).is_none() && !self.commit_reserved_page(ipa) {
                return Err(format!("{a:#x} (ipa {ipa:#x}) is neither backed nor reserved"));
            }
            self.write_guest(ipa, &bytes[done..done + n]);
            done += n;
        }
        Ok(())
    }

    /// `workq_kernreturn(THREAD_KEVENT_RETURN, changelist, nchanges, 0)`: the event manager handing
    /// back its change list (M46 §3d; libpthread `pthread.c:2581-2635`, xnu
    /// `pthread_workqueue.c:3641-3745`).
    ///
    /// In order, it:
    /// 1. registers every change (each classified first, all before any is applied);
    /// 2. fires every timer the guest clock has reached (§3e rule 1);
    /// 3. scans for active knotes.
    ///
    /// With some, it redelivers on the same thread and returns `self`, so that the arm's
    /// `set_x0_err_and_return(self, false)` completes the upcall. With none, the manager unbinds and
    /// parks on its `svc`. Every refusal comes before the clock is read.
    fn guest_workq_kevent_return(&mut self, args: [u64; 8]) -> Result<u64, String> {
        let cur = self.threads.current();
        if self.kq.manager() != kq::Manager::Bound(cur) {
            return Err(format!(
                "M46: workq_kernreturn THREAD_KEVENT_RETURN (0x40) from thread {cur}, which is not the \
                 bound event manager ({:?}): only a thread entered with WQ_FLAG_THREAD_EVENT_MANAGER \
                 returns kevents here (M46 §3d). args=[{}]", self.kq.manager(), Self::fmt_args(args)));
        }
        let n = (args[2] & 0xffff_ffff) as usize;
        if n > retrace_arch::WQ_KEVENT_LIST_LEN || args[3] & 0xffff_ffff != 0 {
            return Err(format!(
                "M46: workq_kernreturn THREAD_KEVENT_RETURN with {n} changes and x3 {:#x}; measured at \
                 most {} changes and x3 0 (libpthread pthread.c:2581-2635). args=[{}]",
                args[3], retrace_arch::WQ_KEVENT_LIST_LEN, Self::fmt_args(args)));
        }
        let size = retrace_arch::KEVENT_QOS_SIZE;
        let bytes = self.read_va_prefix(args[1], n * size);
        if bytes.len() < n * size {
            return Err(format!(
                "M46: unmeasured KEVENT_RETURN change: the change list read {} of {} bytes: it does not \
                 fully translate. args=[{}]", bytes.len(), n * size, Self::fmt_args(args)));
        }
        let mut changes = Vec::with_capacity(n);
        for (i, e) in bytes.chunks_exact(size).enumerate() {
            let entry: &[u8; retrace_arch::KEVENT_QOS_SIZE] = e.try_into().expect("chunks_exact");
            changes.push(retrace_arch::kevent_return_change(entry).map_err(|why| format!(
                "M46: unmeasured KEVENT_RETURN change: changelist[{i}].{why}. args=[{}]",
                Self::fmt_args(args)))?);
        }
        for c in changes {
            match c {
                retrace_arch::ChangeEntry::TimerAdd { ident, deadline, leeway, udata } =>
                    self.kq.add_timer(ident, deadline, leeway, udata),
                retrace_arch::ChangeEntry::TimerDelete { ident } => self.kq.delete_timer(ident),
            }.map_err(|why| format!("M46: KEVENT_RETURN against the knote table: {why}. args=[{}]",
                                    Self::fmt_args(args)))?;
        }
        if self.kq.armed_count() > 0 {
            let now = self.now_guest();
            self.kq.fire_due(now);
        }
        let pthread = self.pthread_of(cur).expect("the running manager has a pthread");
        if self.kq.has_pending() {
            let stack_base = self.threads.stack_of(cur).0;
            self.enter_manager(cur, pthread, stack_base, WQ_ENTRY_FLAGS_MANAGER_REDELIVER);
            Ok(pthread)
        } else {
            self.kq.set_manager(kq::Manager::Unbound(cur));
            self.park_on_svc();
            Ok(0)
        }
    }

    /// M46 §3e rule 1: fire every armed timer the guest clock has reached, and request the manager
    /// for them. Reads nothing while no timer is armed, so a guest without timers, and a static box
    /// without a commpage, never reach the clock.
    fn fire_due_timers(&mut self) {
        if self.kq.armed_count() == 0 { return; }
        let now = self.now_guest();
        if self.kq.fire_due(now) > 0 {
            self.request_manager();
        }
    }
```

**`schedule_after_block`.** Replace it, doc comment included, with:

```rust
    /// Pick and switch after the running thread blocked or exited.
    ///
    /// The pick is `ThreadTable::pick_next` — lowest-indexed runnable — which is a pure function of
    /// the guest's own syscall sequence. That is what lets record and replay schedule identically
    /// with NOTHING recorded and no trace-format change (symmetry rule 2).
    ///
    /// M46 §3e adds time, still a pure function of box state, because every path that reaches here
    /// (`run()`, `step()`, replay's `finish_event`) reaches it at the same point in that sequence:
    /// 1. **Overdue timers fire** before the pick.
    /// 2. **The idle jump.** If nothing is runnable and a timer is armed, `synthetic_tsc` jumps so
    ///    that the guest clock reads the earliest deadline (R4: the earliest kernel-faithful point),
    ///    rule 1 runs again, and the pick is retried, exactly once. The clock never moves
    ///    backwards (`kq::tsc_for_deadline`).
    /// 3. **Otherwise it is a deadlock**, as since M14, and the panic lists the knote table.
    ///
    /// A timer fires only when some thread blocks. A guest that spins without blocking never lets
    /// one fire: the cooperative scheduler's limit, extended to time (`docs/current-state.md`).
    pub fn schedule_after_block(&mut self) {
        self.fire_due_timers();
        let mut next = self.threads.pick_next();
        if next.is_none() {
            if let Some(deadline) = self.kq.earliest_deadline() {
                self.synthetic_tsc = kq::tsc_for_deadline(self.synthetic_tsc, self.timebase_offset(), deadline);
                self.fire_due_timers();
                next = self.threads.pick_next();
            }
        }
        match next {
            Some(tid) => self.switch_to_thread(tid),
            None => panic!(
                "M14: DEADLOCK — no runnable thread. {} live of {} total. States: {:?}. Knotes: {:?}",
                self.threads.live(),
                self.threads.len(),
                (0..self.threads.len()).map(|i| self.threads.state_of(i)).collect::<Vec<_>>(),
                self.kq
            ),
        }
    }
```

- [ ] **Step 4: The arms**

In `crates/retrace-core/src/lib.rs`'s `record_box`:
- In the `SYS_WORKQ_KERNRETURN` arm, replace `let rc = b.guest_workq_kernreturn(args);` with:

```rust
                // M46: the Result form both arms share; a refusal still stops the recorder here.
                let rc = b.try_workq_kernreturn(args).unwrap_or_else(|m| panic!("{m}"));
```

- In the `SYS_KEVENT_QOS` arm, replace `let rc = b.guest_kevent_qos(args);` with:

```rust
                let rc = b.guest_kevent_qos(args).unwrap_or_else(|m| panic!("{m}"));
```

- Update that arm's comment: replace `every shape but the\n            // measured init is refused by value` with `every shape but the three\n            // measured ones (M46) is refused by value`, keeping the file's own line breaks.

In `ReplaySession::advance`, replace the `if num == retrace_arch::SYS_WORKQ_KERNRETURN { … }` mirror with:

```rust
                            if num == retrace_arch::SYS_WORKQ_KERNRETURN {
                                // M46 §3g: a refusal here is reachable only after an earlier silent
                                // divergence (record accepted this call), so it is reported as one
                                // rather than panicking (M45 F-2).
                                let rc = match self.b.try_workq_kernreturn(args) {
                                    Ok(rc) => rc,
                                    Err(m) => return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "workq_kernreturn refused on replay, though the recording accepted it \
                                         — replay diverged before this landmark: {m}") }),
                                };
                                if rc != *ret {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("workq_kernreturn rc mismatch: replay {rc:#x} != recorded {ret:#x}") });
                                }
                                if *ret1 != 0 {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("workq_kernreturn recorded ret1={ret1:#x}; the emulation records 0") });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
```

In the `if num == retrace_arch::SYS_KEVENT_QOS { … }` mirror, replace `let rc = self.b.guest_kevent_qos(args);` with:

```rust
                                let rc = match self.b.guest_kevent_qos(args) {
                                    Ok(rc) => rc,
                                    Err(m) => return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "kevent_qos refused on replay, though the recording accepted it \
                                         — replay diverged before this landmark: {m}") }),
                                };
```

Then, directly after that mirror's `rc != *ret` block, add:

```rust
                                // M46 §3g: M45 owed this compare. Record fixes ret1 at 0.
                                if *ret1 != 0 {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("kevent_qos recorded ret1={ret1:#x}; the emulation records 0") });
                                }
```

In `crates/retrace/tests/kqinit_e2e.rs`, line 69, replace `M45: unmeasured kevent_qos shape: {why}` with `M46: unmeasured kevent_qos shape: {why}`. This is the plan's named exception: the prefix names the milestone that owns the refusal, and the field text after it is unchanged.

- [ ] **Step 5: Run the gate green, plus the neighbours and clippy**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 > $L/t4-box.log 2>&1; echo "exit=$?"
grep -a '^test result:' $L/t4-box.log
cargo test -p retrace --test gcdtimer_e2e --no-fail-fast -- --test-threads=1 > $L/t4-green.log 2>&1; echo "exit=$?"
grep -a -E '^test |^test result:' $L/t4-green.log
for t in kqinit_e2e dispatch_e2e thread_oracle checkpoint_seek hitorder_e2e llsc_e2e hello_dyn_e2e; do cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t4-$t.log 2>&1; echo "$t exit=$?"; done
cargo clippy --workspace --all-targets -- -D warnings > $L/t4-clippy.log 2>&1; echo "exit=$?"
export RETRACE_TRACE=1
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn target/aarch64-apple-darwin/debug/build/$(ls target/aarch64-apple-darwin/debug/build | grep retrace-guest | head -1)/out/after_dyn -o /private/tmp/claude-501/m46-after1.bin > $L/t4-after.out 2> $L/t4-after.err; echo "record exit=$?"
grep -a -c -E '^\[trap\] num=368 .*args=\[0x40,' $L/t4-after.err
```

The last three lines record the default mode under `RETRACE_TRACE` for the report. If the `ls | grep` picks a stale build directory, use `find target -name after_dyn -newer crates/retrace-guest/c/after_dyn.c`.

Expected:
- `retrace-box`: `exit=0`, with `kqmanager` `7 passed`, `threads` green (the `0xbeef` and `DEADLOCK` tests included), and `checkpointparity` green;
- `gcdtimer_e2e`: `exit=0`, `2 passed`;
- every neighbour: `exit=0`;
- clippy: `exit=0`;
- the recorded default mode: the count of `0x40` traps, which the report compares against t0 M3's native count and test 1's bound of 8.

**If test 1 does not go green,** diagnose from `$L/t4-after.err` before changing anything:
- A mach_msg2 refusal naming `task_get_debug_control_port`, or a third `kevent_qos` shape, is **H3**. Model a register-only knote if the fixtures never need it to fire; otherwise halt.
- Any other new subsystem is **H5**.
- A `DEADLOCK` whose knote table shows an armed timer means the idle jump did not happen. Check `schedule_after_block`.
- A `KEVENT_RETURN` count far above 8 means libdispatch sees its deadline as not yet due: check R7 (Task 1) and t0 M1(c)'s offset.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-box/src/thread.rs crates/retrace-box/src/lib.rs crates/retrace-box/tests/kqmanager.rs crates/retrace-core/src/lib.rs crates/retrace/tests/kqinit_e2e.rs crates/retrace/tests/gcdtimer_e2e.rs
git commit -m "M46 t4: the event manager — spawned, re-entered, redelivered and parked below the trace; timers fire on the synthetic clock"
```

- [ ] **Step 7: The three controls (on the committed tree)**

Run each, record the symptom, restore:

1. **The idle jump deleted.** In `schedule_after_block`, delete the `if next.is_none() { … }` block. Run `gcdtimer_e2e`. Expected: test 1 fails, and the record stderr carries `M14: DEADLOCK` with an armed timer in `Knotes:`. Restore with `git checkout -- crates/retrace-box/src/lib.rs`.
2. **The current-thread load removed** (Review Focus 3). In `enter_manager`, replace the `if tid == self.threads.current() { … } else { … }` with the table write alone, `*self.threads.ctx_mut(tid) = ctx;`. Run `gcdtimer_e2e` and `kqmanager`. Expected:
   - test 1 fails, because the re-entered manager resumes on its `svc` with `x0` = 0, which is refused as opcode `0x0`, or diverges;
   - `a_kevent_return_with_a_trigger_pending_redelivers_on_the_same_thread` fails.

   Restore with `git checkout -- crates/retrace-box/src/lib.rs`.
3. **The redelivery replaced by a park.** In `guest_workq_kevent_return`, change `if self.kq.has_pending() {` to `if false {`. Run `kqmanager`. Expected: the redelivery test fails, because the manager is `Unbound`. Restore with `git checkout -- crates/retrace-box/src/lib.rs`.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
cargo test -p retrace --test gcdtimer_e2e --no-fail-fast -- --test-threads=1 > $L/t4-control-<n>.log 2>&1; echo "exit=$?"
cargo test -p retrace-box --test kqmanager --no-fail-fast -- --test-threads=1 >> $L/t4-control-<n>.log 2>&1; echo "exit=$?"
grep -a -E '^test |^test result:' $L/t4-control-<n>.log
git status --short
```

Expected after each restore: `git status --short` prints nothing.

---

### Task 5: The rest of the gate

**Files:**
- Create: `crates/retrace-guest/c/timer_dyn.c`
- Modify: `crates/retrace-guest/build.rs` (after the `after_dyn` block), `crates/retrace-guest/src/lib.rs` (after `AFTER_DYN`, plus a test after `after_guest_parses`)
- Modify: `crates/retrace-box/src/lib.rs`: `dbg_write_va` after `dbg_kq_mut`
- Modify: `crates/retrace-core/src/lib.rs`: three `ReplaySession` passthroughs after `dbg_kport_of`
- Modify: `crates/retrace/tests/gcdtimer_e2e.rs`: tests 2–6

**Interfaces:**
- Consumes: everything above.
- Produces:
  - `retrace_guest::TIMER_DYN`
  - `pub fn Box_::dbg_write_va(&mut self, va: u64, bytes: &[u8]) -> Result<(), String>`
  - on `ReplaySession`: `dbg_internal_state() -> String`, `dbg_armed_timers() -> usize`, and `dbg_write_mem(va, bytes) -> Result<(), String>`

- [ ] **Step 1: The timer fixture and its wiring**

Create `crates/retrace-guest/c/timer_dyn.c`:

```c
// M46. The repeating timer-source fixture (spec §3f): a DISPATCH_SOURCE_TYPE_TIMER with a 50 ms
// interval prints "tick 1" to "tick 3", cancels itself on the third and signals main, which writes
// "done\n". It exercises the re-arm after every fire, manager reuse across fires, and the disarm
// the cancel issues for the armed fourth tick.
// Native stdout (M46 t0): "tick 1\ntick 2\ntick 3\ndone\n".
#include <dispatch/dispatch.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    dispatch_source_t t = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0,
                                                 dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0));
    __block int ticks = 0;
    dispatch_source_set_timer(t, dispatch_time(DISPATCH_TIME_NOW, 50 * NSEC_PER_MSEC), 50 * NSEC_PER_MSEC, 0);
    dispatch_source_set_event_handler(t, ^{
        char line[16];
        int n = snprintf(line, sizeof line, "tick %d\n", ++ticks);
        write(1, line, (size_t)n);
        if (ticks == 3) {
            dispatch_source_cancel(t);
            dispatch_semaphore_signal(sem);
        }
    });
    dispatch_resume(t);
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    write(1, "done\n", 5);
    return 0;
}
```

In `crates/retrace-guest/build.rs`, directly after the `after_dyn` block:

```rust

    // timer_dyn: the M46 repeating timer-source fixture. Same recipe as hello_dyn.
    let src = format!("{}/c/timer_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/timer_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang timer_dyn");
    assert!(status.success(), "timer_dyn guest build failed");
```

In `crates/retrace-guest/src/lib.rs`, directly after `AFTER_DYN`:

```rust
/// M46: a repeating `DISPATCH_SOURCE_TYPE_TIMER` that ticks three times and cancels itself.
pub const TIMER_DYN: &str = concat!(env!("OUT_DIR"), "/timer_dyn");
```

And after `after_guest_parses`:

```rust
    #[test]
    fn timer_guest_parses() {
        // M46: proves the build.rs wiring and the path constant; behaviour is gcdtimer_e2e's.
        let l = parse_macho(&std::fs::read(TIMER_DYN).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
    }
```

- [ ] **Step 2: The test accessors**

In `crates/retrace-box/src/lib.rs`, directly after `dbg_kq_mut`:

```rust
    /// Test-only (M46): write guest memory by VA, page by page, as the box's own event writes do.
    /// `gcdtimer_e2e` tampers an entry at its `svc` with this, to reach the replay-side validators.
    #[doc(hidden)]
    pub fn dbg_write_va(&mut self, va: u64, bytes: &[u8]) -> Result<(), String> {
        self.write_va_committing(va, bytes)
    }
```

In `crates/retrace-core/src/lib.rs`, directly after `pub fn dbg_kport_of(&self, tid: usize) -> Option<u32> { self.b.kport_of(tid) }`:

```rust
    /// M46: `Box_::dbg_internal_state`, which includes `synthetic_tsc` and the knote table. Test-only.
    #[doc(hidden)]
    pub fn dbg_internal_state(&self) -> String { self.b.dbg_internal_state() }
    /// M46: timers armed and not yet fired. Test-only: `gcdtimer_e2e` checks the idle jump delivered its timer.
    #[doc(hidden)]
    pub fn dbg_armed_timers(&self) -> usize { self.b.dbg_kq().armed_count() }
    /// M46: write guest memory by VA at the current position. Test-only: tampering here is how
    /// `gcdtimer_e2e` reaches the replay-side validators, which a rewritten trace field cannot
    /// reach, because the mirror compares recorded fields first.
    #[doc(hidden)]
    pub fn dbg_write_mem(&mut self, va: u64, bytes: &[u8]) -> Result<(), String> { self.b.dbg_write_va(va, bytes) }
```

- [ ] **Step 3: Tests 2–6**

Append to `crates/retrace/tests/gcdtimer_e2e.rs`:

```rust

/// Spec §3f test 2. One kernel timer serves the bucket, so B's arm follows A's fire through a
/// KEVENT_RETURN that reprograms it.
#[test]
fn two_timers_on_one_bucket_fire_in_deadline_order() {
    let (rec, trace) = records_and_replays(retrace_guest::AFTER_DYN, &["two"]);
    assert_eq!(rec.stdout, b"A\nB\ndone\n", "got {:?}", String::from_utf8_lossy(&rec.stdout));
    let rets = kevent_returns(&trace);
    assert!(rets.len() >= 2 && rets.len() <= 12, "two fires: {} KEVENT_RETURNs", rets.len());
}

/// Spec §3f test 3: re-arm after every fire, and manager reuse across fires.
#[test]
fn a_repeating_timer_ticks_three_times_and_replays() {
    let (rec, trace) = records_and_replays(retrace_guest::TIMER_DYN, &[]);
    assert_eq!(rec.stdout, b"tick 1\ntick 2\ntick 3\ndone\n", "got {:?}", String::from_utf8_lossy(&rec.stdout));
    let rets = kevent_returns(&trace);
    let manager = rets.first().map(|r| r.1).expect("KEVENT_RETURNs");
    assert!(manager != 0 && rets.iter().all(|r| r.1 == manager), "one manager, reused across fires: {rets:?}");
    assert!(rets.len() >= 3 && rets.len() <= 24, "three fires: {} KEVENT_RETURNs", rets.len());
}

/// Spec §3f test 4: a WALL timer is refused by value, naming its fflags. An exit code alone would
/// not do: a recorder that accepted the timer and jumped the clock to a wall-clock deadline could
/// exit either way.
#[test]
fn a_wall_clock_timer_is_refused_naming_its_fflags() {
    let (rec, _) = util::record_dynamic_args(retrace_guest::AFTER_DYN, &["wall"]);
    assert_eq!(rec.code, 101, "the recorder must stop at the refusal. stderr:\n{}", rec.stderr);
    assert!(rec.stderr.contains("M46: unmeasured KEVENT_RETURN change: changelist[")
            && rec.stderr.contains("fflags is 0x9c, measured one of 0x118, 0x138, 0x158"),
        "the refusal must name the WALL fflags (t0 M3). stderr:\n{}", rec.stderr);
    assert!(!rec.stdout.windows(6).any(|w| w == b"fired\n"), "the guest must not run past the refused arm");
}

fn synthetic_tsc(state: &str) -> u64 {
    let v = state.split("synthetic_tsc=0x").nth(1).and_then(|r| r.split_whitespace().next())
        .unwrap_or_else(|| panic!("no synthetic_tsc in {state}"));
    u64::from_str_radix(v, 16).unwrap()
}

/// The landmark whose settle makes `after_dyn`'s idle jump: the one across which `synthetic_tsc`
/// moves by more than one window's timebase reads could move it.
///
/// Found by the clock and not by the armed count. The manager's KEVENT_RETURN arms the timer and
/// that same landmark's settle fires it, because main is already blocked and the manager has just
/// parked (Review Focus 3). No landmark boundary ever sees the timer armed.
fn the_idle_jump(trace: &Path) -> usize {
    // 0x10_0000 ticks is ~44 ms at 24 MHz. A timebase read moves the counter 0x2400 (384 µs), so
    // this is ~113 reads inside one window. The jump itself is the 100 ms deadline minus the few
    // reads between `dispatch_time` and the park.
    const JUMP: u64 = 0x10_0000;
    let mut s = retrace_core::ReplaySession::open(trace).unwrap();
    loop {
        let (n, before) = (s.landmark(), synthetic_tsc(&s.dbg_internal_state()));
        match s.advance() {
            Ok(retrace_core::Advance::Exited(_)) => panic!("the run exited and no landmark jumped the clock"),
            Ok(_) => {}
            Err(d) => panic!("diverged at landmark {}: {}", d.landmark, d.detail),
        }
        if synthetic_tsc(&s.dbg_internal_state()) - before > JUMP {
            return n;
        }
    }
}

/// Spec §3f test 5, the seek half, which proves §3c's field is rebuilt on the non-linear path. A
/// checkpoint taken before the idle jump, continued across it, must equal a cold seek past it.
#[test]
fn a_seek_across_the_idle_jump_matches_a_cold_seek() {
    let (_, trace) = records_and_replays(retrace_guest::AFTER_DYN, &[]);
    let manager = kevent_returns(&trace)[0].1;
    let n = the_idle_jump(&trace);
    let cp = retrace_core::seek(&trace, n, 0).unwrap().checkpoint();
    let warm = {
        let mut s = retrace_core::ReplaySession::from_checkpoint(&trace, &cp).unwrap();
        s.advance_to_landmark(n + 1).unwrap_or_else(|d| panic!("warm: diverged at {}: {}", d.landmark, d.detail));
        (s.current_thread(), s.dbg_regs(), s.dbg_fp_regs(), s.dbg_internal_state(), s.snapshot().1)
    };
    let cold = retrace_core::seek(&trace, n + 1, 0).unwrap();
    // The jump fired the timer and re-entered the manager: the upcall is loaded, and the timer is
    // delivered, not still armed.
    assert_eq!(cold.current_thread(), manager, "the fire re-entered the manager, the only runnable thread");
    assert!(cold.dbg_regs().contains("x4 =0x00000000001e4008"),
        "the manager's upcall carries the reuse flags (t0 M2):\n{}", cold.dbg_regs());
    assert_eq!(cold.dbg_armed_timers(), 0, "the jump fired the timer it jumped to");
    assert_eq!(warm.0, cold.current_thread(), "thread: checkpointed vs cold");
    assert_eq!(warm.1, cold.dbg_regs(), "registers: checkpointed vs cold");
    assert_eq!(warm.2, cold.dbg_fp_regs(), "FP/SIMD: checkpointed vs cold");
    assert_eq!(warm.3, cold.dbg_internal_state(), "the knote table and the synthetic clock: checkpointed vs cold");
    assert!(cold.diff_memory(&warm.4).is_none(), "memory: checkpointed vs cold");
}

fn parse_cell(stdout: &str) -> u64 {
    let rest = &stdout[stdout.find("fired cell 0x").unwrap_or_else(|| panic!("no cell line in {stdout:?}")) + 13..];
    u64::from_str_radix(&rest[..rest.find('\n').unwrap_or(rest.len())], 16).unwrap()
}

/// Spec §3f test 5, the reverse half: with the whole run replayed, a watch on the handler's cell
/// and a `reverse-continue` must reach the handler's store, named on the worker that ran it.
#[test]
fn reverse_continue_reaches_the_handlers_store_and_names_its_worker() {
    let (rec, trace) = records_and_replays(retrace_guest::AFTER_DYN, &[]);
    let cell = parse_cell(&String::from_utf8_lossy(&rec.stdout));
    assert!(cell.is_multiple_of(8), "the cell must be 8-aligned to watch");
    let manager = kevent_returns(&trace)[0].1;
    // The handler's `write(1, "fired\n", 6)`; stdio's cell line goes out through write_nocancel.
    let writer = events(&trace).into_iter().find_map(|(_, e)| match e {
        Event::Syscall { num, args, thread, .. } if num == retrace_arch::SYS_WRITE && args[0] == 1 && args[2] == 6 => Some(thread),
        _ => None,
    }).expect("the handler's write of \"fired\\n\"");
    assert!(writer != 0 && writer != manager, "the handler runs on a worker: writer {writer}, manager {manager}");
    let (code, out, err) = util::debug_bounded(trace.to_str().unwrap(),
        &format!("continue; watch 0x{cell:x}; reverse-continue; where"), 300);
    assert_eq!(code, Some(0), "debug exited {code:?} (None: killed at the bound). stderr: {err}\nstdout: {out}");
    assert!(out.contains(&format!("hit watch 0x{cell:x}")), "reverse-continue must find the handler's store:\n{out}");
    let where_line = util::strip_annot(out.lines().last().expect("a `where` line"));
    assert!(where_line.ends_with(&format!("thread={writer}")),
        "`where` must name the worker that ran the handler, thread {writer}. got:\n{where_line}\n{out}");
}

/// A session at landmark `i`'s `svc`: the instruction that issues it, not yet retired. It steps one
/// instruction at a time, because the entry the call reads is written inside window `i`, so a
/// tamper at `(i, 0)` would be overwritten before the call reads it.
fn session_at_svc(trace: &Path, i: usize) -> retrace_core::ReplaySession {
    const SVC_0X80: [u8; 4] = 0xd400_1001u32.to_le_bytes();
    let mut s = retrace_core::seek(trace, i, 0).unwrap();
    for _ in 0..1_000_000 {
        if s.read_mem_prefix(s.pc(), 4) == SVC_0X80 {
            return s;
        }
        s.step_insns(1).unwrap_or_else(|e| panic!("stepping window {i}: {e}"));
    }
    panic!("no svc within 1M instructions of landmark {i}");
}

/// Spec §3f test 6 (M45 F-2). A shape the recording accepted but replay refuses is reachable only
/// after an earlier silent divergence, so replay must report it as a `Divergence` that names the
/// field, not panic. The tamper is in guest memory at the `svc`, because a rewritten trace field
/// is compared before the validator runs.
#[test]
fn a_shape_refused_on_replay_is_a_divergence_naming_it_not_a_panic() {
    let (_, trace) = records_and_replays(retrace_guest::AFTER_DYN, &[]);
    let evs = events(&trace);
    // Case 1: the manager poke, which is main's `0x23` registration whose entry is EVFILT_USER.
    let mut poked = false;
    for (i, list) in evs.iter().filter_map(|(i, e)| match e {
        Event::Syscall { num, args, .. } if *num == retrace_arch::SYS_KEVENT_QOS && args[7] & 0xffff_ffff == 0x23 => Some((*i, args[1])),
        _ => None,
    }) {
        let mut s = session_at_svc(&trace, i);
        if s.read_mem_prefix(list + 8, 2) != retrace_arch::EVFILT_USER.to_le_bytes() { continue; }
        s.dbg_write_mem(list + 24, &0u32.to_le_bytes()).unwrap(); // fflags: NOTE_TRIGGER -> 0
        let d = match s.advance() { Err(d) => d, Ok(_) => panic!("the tampered poke at landmark {i} replayed") };
        assert_eq!(d.landmark, i);
        assert!(d.detail.contains("replay diverged before this landmark")
                && d.detail.contains("changelist[0].fflags is 0x0, measured 0x1000000"), "{}", d.detail);
        poked = true;
        break;
    }
    assert!(poked, "no manager poke among the 0x23 registrations");
    // Case 2: the manager's first KEVENT_RETURN that carries a change, which is its timer arm,
    // turned WALL.
    let (j, list) = evs.iter().find_map(|(i, e)| match e {
        Event::Syscall { num, args, .. } if *num == retrace_arch::SYS_WORKQ_KERNRETURN
            && args[0] == retrace_arch::WQOPS_THREAD_KEVENT_RETURN && args[2] & 0xffff_ffff >= 1 => Some((*i, args[1])),
        _ => None,
    }).expect("a KEVENT_RETURN carrying a change");
    let mut s = session_at_svc(&trace, j);
    assert_eq!(s.read_mem_prefix(list + 8, 2), retrace_arch::EVFILT_TIMER.to_le_bytes(), "the first change is a timer");
    s.dbg_write_mem(list + 24, &0x9cu32.to_le_bytes()).unwrap();
    let d = match s.advance() { Err(d) => d, Ok(_) => panic!("the tampered KEVENT_RETURN at landmark {j} replayed") };
    assert_eq!(d.landmark, j);
    assert!(d.detail.starts_with("workq_kernreturn refused on replay")
            && d.detail.contains("changelist[0].fflags is 0x9c"), "{}", d.detail);
}
```

- [ ] **Step 4: Run it green, plus clippy**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
cargo test -p retrace --test gcdtimer_e2e --no-fail-fast -- --test-threads=1 > $L/t5-green.log 2>&1; echo "exit=$?"
grep -a -E '^test |^test result:' $L/t5-green.log
cargo test -p retrace-guest --no-fail-fast -- --test-threads=1 > $L/t5-guest.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets -- -D warnings > $L/t5-clippy.log 2>&1; echo "exit=$?"
```

Expected: `gcdtimer_e2e` `8 passed; 0 failed`; the guest crate green; clippy `exit=0`.

The count bounds in tests 2 and 3 are deliberately loose; the report records the measured counts. A test that fails only on its upper bound means the manager is re-arming. Report it; never widen the bound.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-guest/c/timer_dyn.c crates/retrace-guest/build.rs crates/retrace-guest/src/lib.rs crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs crates/retrace/tests/gcdtimer_e2e.rs
git commit -m "M46 t5: the gate — deadline order, a repeating timer, the WALL refusal, a seek across the fire, reverse-continue to the handler, replay-side divergence"
```

- [ ] **Step 6: The spec's three controls (§4, on the committed tree)**

Run each, record the symptom, restore:

1. **The fflags check widened.** In `kevent_return_change`, replace the `let Some(tidx) = … else { … };` statement with `let tidx = UPTIME_TIMER_FFLAGS.iter().position(|&f| f == e.fflags).unwrap_or(0);`. Run `gcdtimer_e2e`. Expected: `a_wall_clock_timer_…` fails. Either the recorder no longer names `fflags is 0x9c` (it refuses on the ident instead), or it runs on. Restore with `git checkout -- crates/retrace-arch/src/lib.rs`.
2. **`kq` dropped from `from_checkpoint`.** Replace `kq: state.kq.clone(),` with `kq: kq::WorkqKqueue::default(),`. Run `gcdtimer_e2e`. Expected: `a_seek_across_the_idle_jump_…` fails: the warm session diverges at landmark `n`, because the emptied table holds no bound manager, so the manager's KEVENT_RETURN is refused as coming from a thread that is not it. Restore with `git checkout -- crates/retrace-box/src/lib.rs`.
3. **M45's panic restored in the replay mirror.** In the `SYS_KEVENT_QOS` mirror, replace the `match self.b.guest_kevent_qos(args) { … };` with `self.b.guest_kevent_qos(args).unwrap_or_else(|m| panic!("{m}"));`. Run `gcdtimer_e2e`. Expected: `a_shape_refused_on_replay_…` fails with a panic in case 1. Restore with `git checkout -- crates/retrace-core/src/lib.rs`.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
cargo test -p retrace --test gcdtimer_e2e --no-fail-fast -- --test-threads=1 > $L/t5-control-<n>.log 2>&1; echo "exit=$?"
grep -a -E '^test |^test result:' $L/t5-control-<n>.log
git status --short
```

Expected after each restore: `git status --short` prints nothing.

---

### Task 6: The walk (controller-run for the sweep)

**Files:**
- Modify: `crates/retrace/tests/apple_walls_e2e.rs` (`automationmodetool_records_and_replays`)
- Create: `docs/sweep-evidence/2026-09-29-m46/` (`README.md`, the walk's stderr, the sweep log, `rowdiff.txt`)

**Interfaces:**
- Consumes: Tasks 1–5 landed.
- Produces:
  - outcome A (un-ignored) or B (re-parked at a new, measured wall) for `automationmodetool`;
  - the sweep tally and row diff that Task 7 writes into `docs/current-state.md` and the README.

- [ ] **Step 1: automationmodetool on the model**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/docs/sweep-evidence/2026-09-29-m46
mkdir -p $E
export RETRACE_TRACE=1
cargo build -p retrace > $L/t6-build.log 2>&1; echo "exit=$?"
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn /usr/bin/automationmodetool -o /private/tmp/claude-501/m46-amt.bin > $E/automationmodetool.rec.out 2> $E/automationmodetool.rec.err; echo "record exit=$?"
grep -a -c '^\[trap\] ' $E/automationmodetool.rec.err
tail -5 $E/automationmodetool.rec.err
```

M45 t0 M4 measured `automationmodetool`'s native no-argument rc, recorded in the M45 measurements file. Only if the record exited with that rc:

```bash
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/docs/sweep-evidence/2026-09-29-m46
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace replay /private/tmp/claude-501/m46-amt.bin > $E/automationmodetool.rp.out 2> $E/automationmodetool.rp.err; echo "replay exit=$?"
cmp $E/automationmodetool.rec.out $E/automationmodetool.rp.out; echo "cmp=$?"
```

**Decide:**
- **Outcome A:** the record exits with the native rc, its stdout equals the native stdout, and replay exits the same with `cmp=0`.
- **Outcome B:** anything else. The first failure is the new wall. Record the first `RECORD ERROR:` or `panicked at` line, its trap, pc and landmark (count the `[trap]` lines before it), and the rc/rp.
- **A third `kevent_qos` shape or an `EVFILT_MACHPORT`-shaped registration is H3.** A new subsystem is H5. Both re-park and route; neither is modelled here.

- [ ] **Step 2A (outcome A): un-ignore**

Delete the `#[ignore = "…"]` line above `fn automationmodetool_records_and_replays`, provided the test body's assertions match the native outcome. If the native rc is non-zero, replace the body the way M45's plan Task 3 Step 2A shows, with the native rc and first line in place. Append one sentence to the file's header comment saying M46 un-parked it, and why.

- [ ] **Step 2B (outcome B): re-park with the new wall**

Replace the `#[ignore = "…"]` reason with one in the file's house form, every field taken from Step 1's evidence:

```
M46 wall, class <B|C> (<one-line subsystem or row>), parked<, routed to <successor>|, not routed>. /usr/bin/automationmodetool: libdispatch's event manager and UPTIME timers are modelled since M46 (its memory-pressure registration records at landmark <N>, rc 0). The run now continues to <the wall in the recorder's own words: its trap/call, number, pc>, landmark <L>, rc/rp <x>/<y> (<'no replay ran' if the record did not exit cleanly>): `<first RECORD ERROR: / panicked at line>`. Evidence docs/sweep-evidence/2026-09-29-m46/automationmodetool.rec.err. UN-IGNORE when <what would clear it>.
```

Append one sentence to the file's header comment saying M46 moved `automationmodetool` past the second `kevent_qos` to the new wall.

- [ ] **Step 3: The sweep (controller-run)**

Copy `target/aarch64-apple-darwin/debug/retrace` into the session scratchpad and sign it there with `retrace.entitlements`, as M39's and M45's evidence READMEs describe, so a concurrent build cannot swap it. Then run the sweep **detached, with no concurrent `cargo`** (M45 T3-a):

```bash
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/docs/sweep-evidence/2026-09-29-m46
export RETRACE_SWEEP_KEEP=$E/sweep
tools/apple-sweep.sh <signed copy> > $E/sweep.log 2>&1; echo "exit=$?"
grep -a '^TALLY' $E/sweep.log
grep -a '^ROW' $E/sweep.log > $E/rows.txt
grep -a '^ROW' ../../../docs/sweep-evidence/2026-09-28-m45/sweep.log | diff - $E/rows.txt > $E/rowdiff.txt; echo "diff=$?"
```

The `../../../` path reads M45's committed evidence from inside the worktree. If it does not resolve, use `docs/sweep-evidence/2026-09-28-m45/sweep.log`, which the worktree carries at the same path.

**For every moved row**, measure it against a base binary before calling it M46's. Build the base from `f907c33`:

```bash
mkdir -p /private/tmp/claude-501/m46-base
git archive f907c33 | tar -x -C /private/tmp/claude-501/m46-base
```

Then run `cargo build -p retrace --target-dir /private/tmp/claude-501/m46-base-target` inside `/private/tmp/claude-501/m46-base`, and sign a copy. Record the row twice on each binary, alternating base and swept.
- A move the base binary shows too is **host state**, not M46.
- A move only the swept binary shows is M46's. R7 changes every guest that reaches `mach_get_times`'s fallback, so a row whose landmark count moved belongs to M46 once measured.

Write the evidence `README.md` in M45's shape: method, binary hash and commit, the tally, and every moved row with its reason. An unexplained moved row is **H5**.

- [ ] **Step 4: Commit**

```bash
git add crates/retrace/tests/apple_walls_e2e.rs docs/sweep-evidence/2026-09-29-m46 ':(exclude)docs/sweep-evidence/2026-09-29-m46/*.bin' ':(exclude)docs/sweep-evidence/2026-09-29-m46/sweep/*.bin'
git commit -m "M46 t6: the walk — automationmodetool <un-parked|re-parked at <wall>>, the sweep"
```

---

### Task 7: The docs

**Files:**
- Modify: `docs/status-log.md` (append), `docs/current-state.md` (edit in place), `CLAUDE.md`, and `README.md` only where it states something that changed

**Interfaces:** Consumes every task's report, the t0 measurements file and Task 6's evidence. Produces the docs Task 8's reviewer reads.

- [ ] **Step 1: `docs/current-state.md`, edited in place**

Find each passage with `grep -n` and edit it to describe the new reality:
- **"What works today":** after the paragraph on GCD and `kqinit_e2e` (`grep -n 'kqinit_e2e\|kevent_qos' docs/current-state.md`), add a paragraph covering:
  - libdispatch's event manager and UPTIME timers are modelled since M46, below the trace (`Box_::kq`, `schedule_after_block`'s firing rule);
  - a `dispatch_after` and a repeating timer source record and replay (`gcdtimer_e2e`);
  - the two refusal families;
  - R7: `gettimeofday`'s mach-time out-parameter is the guest's own clock, recorded.
- **Known limits.** Add three:
  - §3e's limit: a timer fires only when some thread blocks, so a guest that spins waiting for one hangs;
  - MONOTONIC and WALL timers, memory-pressure delivery and `EVFILT_MACHPORT`/`EVFILT_SIGNAL` knotes are refused by value;
  - wall time (`gettimeofday`'s `tv`) is still the host's, recorded, while mach time is synthetic.

  Replace the sentence saying any `kevent_qos` shape other than the init is refused with the three-shape account.
- **The ignored-gates paragraph** (`grep -n 'automationmodetool' docs/current-state.md`): outcome A says M46 un-parked it; outcome B names the new wall.
- **The Apple-sweep figure and table:** the new tally, measured date and commit, and every moved row.
- **The gate line:** not touched here. Task 8 writes the measured counts.

- [ ] **Step 2: `CLAUDE.md`**

In "Commands", the e2e list: replace `` `skiplines` (M44: the skip-line detector and its control). Run one with`` with:

```markdown
`skiplines` (M44: the skip-line detector and its control), `gcdtimer_e2e` (M46: libdispatch's
  event manager and UPTIME timers on the synthetic clock — a `dispatch_after`, a repeating timer
  source, two timers in deadline order, the WALL refusal by value, a seek across the idle jump,
  `reverse-continue` to the handler's store on its worker, a replay-side refusal reported as a
  divergence, and R7's one-clock check through `mach_get_times`). Run one with
```

In "Guest threads", replace:

```markdown
a workqueue worker parked at `workq_kernreturn`
opcode `0x4` (`BlockReason::Parked`) has **no waker at all**
```

with:

```markdown
a workqueue worker parked at `workq_kernreturn`
opcode `0x4` (`BlockReason::Parked`) has **no waker at all** — the event manager, parked at opcode
`0x40` since M46, is the one exception: a knote activation re-enters it (`ThreadTable::unpark`)
```

Match the file's own line breaks exactly, and check each replaced text with `grep -n` first. Then, directly after the paragraph ending `was measured false at M37.`, add:

```markdown
Since M46 the box also models libdispatch's **event manager**: `kevent_qos`'s memory-pressure
registration and manager poke join M45's init, the manager's `KEVENT_RETURN` (`0x40`) registers
UPTIME timers, and `schedule_after_block` fires them on `synthetic_tsc` — overdue timers first,
then, with nothing runnable, one jump of the clock to the earliest deadline. All of it is box state
(`Box_::kq`, carried in `BoxState`), rebuilt from the guest's own syscalls on both sides, so nothing
new is recorded. The recorder does rewrite one value: `gettimeofday`'s mach-time out-parameter
becomes the guest's own clock (M46 R7), because libdispatch reads its timer "now" through
`mach_get_times`, whose commpage path always falls back to 116 under a frozen commpage.
```

- [ ] **Step 3: README**

Check it against Tasks 1–6 (`grep -n 'GCD\|dispatch\|timer\|automationmodetool\|Apple' README.md`). Edit only what it states that changed: a limit in its Limits list, the Apple-sweep figure, or a headline capability. Leave the gate line for Task 8. If nothing it states changed, leave it alone, and say so in the report.

- [ ] **Step 4: `docs/status-log.md`, appended**

Append `## M46-gcdtimers: libdispatch timers, end to end, on the synthetic clock` at the end, never editing an earlier section. Mirror M45's subsections:
- what t0 measured, M1–M4, with the halts considered and R7's premise;
- one subsection per task with its commit hashes, and every control with its symptom;
- the walk: outcome A or B, the sweep tally, and the moved rows;
- the gate (from Task 8);
- what measurement changed;
- rulings: R1–R7 and every ruling made during execution;
- **What stays owed:**
  - MONOTONIC and WALL timers;
  - memory-pressure delivery;
  - machport and signal knotes;
  - workloops and `kevent_id`;
  - `kevent` on a guest `kqueue()` descriptor;
  - timed waits;
  - plain-worker reuse;
  - preemptive firing;
  - wall time still the host's;
  - `automationmodetool`'s new wall, if outcome B;
  - M45's and M44's untouched owed items, by reference.

Add, as a note inside the M46 section, a forward pointer for M45's owed items: "M45's two owed items (the replay-side validator panicking; the mirror not comparing `ret1`) are paid by M46; see Task 4."

- [ ] **Step 5: Commit**

```bash
git add docs/current-state.md CLAUDE.md README.md docs/status-log.md
git commit -m "M46 docs: current-state in place, status log appended, CLAUDE.md's gate list and guest-threads paragraph"
```

---

### Task 8: The gate, the reconciliation, the merge (controller-run)

**Files:**
- Create: `.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers/{predict.sh,gate.sh,tally.sh,gate-summary.txt}`

- [ ] **Step 1: Predict the count from source, before the gate**

Create `$L/predict.sh`:

```bash
#!/bin/bash
# M46 close (Task 8 Step 1): predict the gate's passed+ignored from source, before the gate.
# Counts `#[test]` lines per file at the M45 merge (f907c33) and at HEAD, for every .rs under
# crates/*/tests and crates/*/src; prints per-file deltas, totals, and the test-target counts.
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers || exit 1
BASE=f907c33
F=/private/tmp/claude-501/m46-predict-files.txt
count_base() { git show "$BASE:$1" 2>/dev/null | grep -c -E '^\s*#\[test\]'; }
count_head() { if [ -f "$1" ]; then grep -c -E '^\s*#\[test\]' "$1"; else echo 0; fi; }
{ git ls-tree -r --name-only "$BASE" -- crates; git ls-tree -r --name-only HEAD -- crates; } \
  | grep -E '^crates/[^/]+/(tests|src)/.*\.rs$' | sort -u > "$F"
tb=0; th=0
while read -r f; do
  b=$(count_base "$f"); h=$(count_head "$f")
  b=${b:-0}; h=${h:-0}
  tb=$((tb + b)); th=$((th + h))
  [ "$b" != "$h" ] && echo "DELTA $f: $b -> $h ($((h - b)))"
done < "$F"
rm -f "$F"
echo "TOTAL #[test]: $tb -> $th ($((th - tb)))"
for c in $(ls crates); do
  b=$(git ls-tree --name-only "$BASE" "crates/$c/tests/" 2>/dev/null | grep -c '\.rs$')
  h=$(ls crates/$c/tests/*.rs 2>/dev/null | wc -l | tr -d ' ')
  [ "$b" != "$h" ] && echo "TARGETS crates/$c/tests: $b -> $h"
done
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
bash $L/predict.sh > $L/predict.txt 2>&1; echo "exit=$?"
cat $L/predict.txt
```

Expected:
- `TOTAL #[test]: 853 -> 895` (+42), made up of:
  - `gcdshapes.rs` +14;
  - `retrace-box/src/kq.rs` +11;
  - `kqmanager.rs` +7;
  - `gcdtimer_e2e.rs` +8;
  - `retrace-guest/src/lib.rs` +2.
- Three `TARGETS` lines, each one file up: `retrace-arch`, `retrace-box` and `retrace`.

The prediction is then **passed + ignored = 897**, over **151** binaries. Explain every difference by task before running the gate.

- [ ] **Step 2: The chunked gate**

Create `$L/gate.sh`:

```bash
#!/bin/bash
# M46 close (Task 8 Step 2): the chunked gate, copied from M45's. Every chunk runs --no-fail-fast;
# each chunk's exit code is captured before any pipe and lands in gate-summary.txt. Read the logs,
# never this script's own exit status.
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers || exit 1
D=.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
S=$D/gate-summary.txt
: > "$S"
run() {
  name=$1; shift
  "$@" > "$D/gate-$name.log" 2>&1
  echo "$name exit=$?" >> "$S"
}
run ws cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1
run box cargo test -p retrace-box --no-fail-fast -- --test-threads=1
run bins cargo test -p retrace --bins --no-fail-fast -- --test-threads=1
ls crates/retrace/tests/*.rs | xargs -n1 basename | sed 's/\.rs$//' > "$D/gate-targets.txt"
while read -r n; do
  run "e2e-$n" cargo test -p retrace --test "$n" --no-fail-fast -- --test-threads=1
done < "$D/gate-targets.txt"
run clippy cargo clippy --workspace --all-targets -- -D warnings
echo DONE >> "$S"
```

Run it in the background (`bash $L/gate.sh`), and wait for `DONE` in `gate-summary.txt`. **Read the logs, not the script's exit status.**

- [ ] **Step 3: Tally and reconcile**

Create `$L/tally.sh`:

```bash
#!/bin/bash
# Per-chunk tally of the M46 close gate: passed/failed/ignored over binaries, ANSI stripped.
D=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
sum() {
  LC_ALL=C sed 's/\x1b\[[0-9;]*m//g' "$@" | grep -a '^test result:' \
    | awk '{p+=$4; f+=$6; i+=$8; n++} END {print p"/"f"/"i" over "n}'
}
for c in ws box bins; do echo "$c $(sum "$D/gate-$c.log")"; done
echo "e2e $(sum "$D"/gate-e2e-*.log)"
echo "e2e logs: $(ls "$D"/gate-e2e-*.log | wc -l)"
echo "all $(sum "$D"/gate-*.log)"
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
cat $L/gate-summary.txt
bash $L/tally.sh
grep -a -h 'SKIPPED' $L/gate-*.log | sort | uniq -c
```

The pass bar:
- `failed` is 0, and every line in `gate-summary.txt` reads `exit=0`;
- `passed + ignored` equals Step 1's prediction;
- the number of `test result:` lines equals 151, or Step 1's binary count;
- no `SKIPPED` line appears for a tool this machine has.

Any disagreement is reconciled file by file before anything is merged.

- [ ] **Step 4: Fill the gate line, then the final review**

Replace the numbers on the `**Gate:**` lines of `docs/current-state.md` and `README.md` (`grep -n '^\*\*Gate:\*\*'`) with the tally's, and the sentence after each that names the count of `crates/retrace/tests/` files. Append the gate to the status-log M46 section, then commit:

```bash
git add README.md docs/current-state.md docs/status-log.md
git commit -m "M46 close: the gate — <passed> passed / 0 failed / <ignored> ignored over <binaries>"
```

Dispatch the whole-branch reviewer (the SDD skill's final review) over `f907c33..HEAD`. Apply its fix wave, one commit per item. Re-run only the chunks a fix touched, and re-tally.

- [ ] **Step 5: The merge waits for the operator**

The worktree session cannot reach `main`'s checkout. Report the branch head, the tally and the reviewer's verdict, and ask the operator to merge from the main checkout:

```bash
git merge --no-ff worktree-m46-gcdtimers -m "Merge M46-gcdtimers: libdispatch timers, end to end, on the synthetic clock"
git rev-parse 'main^{tree}' 'worktree-m46-gcdtimers^{tree}'
```

The two tree hashes must be equal: that is the proof that `main` holds exactly the gated tree.

**Do not push.** Before any `git worktree remove`, copy the ledger `.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers/` into the main checkout's `.superpowers/sdd/`. `git worktree remove` deletes the ignored ledger silently (the M45 lesson).
