# M45-kqinit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Emulate libdispatch's workqueue-kqueue initialisation, `kevent_qos` (374) with `KEVENT_FLAG_WORKQ`, as exactly one measured shape returning 0. Refuse every other shape by value, naming the field that differs. Then walk `automationmodetool`, and three GCD guests, to whatever each reaches next.

**Architecture:**
- A pure validator in `retrace-arch` decides whether a call is the measured init.
- `Box_::guest_kevent_qos` reads the one 72-byte entry through the guest's own page tables and asks the validator. It returns 0 or panics with the refusal.
- Record and replay each get an arm beside the workqueue pair's (symmetry rule 1), and the generic forward arm asserts 374 away.
- A repo-owned C fixture issues the call by hand, so the mechanism is guarded on any machine.
- Nothing changes the trace format.

**Tech Stack:** Rust 1.95.0 (`aarch64-apple-darwin`), Hypervisor.framework, cargo tests, clang for guest fixtures, POSIX `sh` for the sweep.

**Spec:** `docs/superpowers/specs/2026-09-28-retrace-m45-kqinit-design.md` (committed `cf54422`; corrected from this plan, see its §11). Its sections and rulings are cited as `M45 §3b`, `R1`, and so on.

## Global Constraints

- Toolchain `1.95.0`, target `aarch64-apple-darwin`. The gate is `cargo test` in chunks (every one `--no-fail-fast`, `--test-threads=1`) plus `cargo clippy --workspace --all-targets -- -D warnings`.
- **`clippy -D warnings` rejects dead code.** A private function, constant or field nothing uses fails it. Each task adds only what its own non-test code uses. `pub` items in `retrace-arch` are exported, so a `pub` constant used only by tests is allowed.
- `clippy.toml` bans `Instant::now`, `SystemTime::now` and `std::thread::Thread`.
- `--test-threads=1` on every `cargo test`, because only one VM is allowed per process.
- **`TRACE_MAGIC` does not move, and `crates/retrace-trace` has no diff** (M45 §3d, R6). No `Event` shape changes, and no snapshot bytes change meaning.
- **Symmetry rule 1:** the record arm and the replay mirror call the same `Box_` method with the same arguments, and both sit before the generic arm (CLAUDE.md).
- **The thread oracle's count stays at seven.** The replay mirror goes inside the generic `Syscall` arm's `if num ==` chain, which has already called `verify_thread`, so it adds no `verify_thread` of its own (M45 §3d).
- Every test that spawns the CLI goes through `util`'s helpers, which use `util::bin()`, the codesigned copy.
- No existing test changes an assertion, except `apple_walls_e2e::automationmodetool_records_and_replays`, which Task 3 un-ignores or re-parks.
- **Every refusal is a panic whose message starts `M45: unmeasured kevent_qos shape: `**. The tests match on that prefix and on the validator's exact field text.
- **Worktree shell rules:**
  - no `VAR=val cmd` prefix; put `export VAR=val` on its own line first;
  - no `git -C`;
  - put `echo "exit=$?"` in the **same** command as the cargo invocation it checks, **before** any pipe;
  - `--no-fail-fast` goes before `--`;
  - never `git stash`, which is shared across worktrees.
- **Controls (deliberate breakages):** run only on the **committed** tree, restore with `git checkout -- <file>`, and record each control's actual symptom in the task report. A control that stays green is a finding: report it, never paper over it.
- **Logs:** each command writes to `$L/t<N>-<what>.log`, where `L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit`. Shell state does not persist between tool calls, so every command that uses `$L` starts with its own `export L=…` line. Scratch traces go to `/private/tmp/claude-501/m45-*.bin`.
- **Grep gate logs with `grep -a`**, since they carry ANSI and UTF-8. Before any `awk`, sanitize with `LC_ALL=C tr -cd '\11\12\15\40-\176' < log | sed 's/\x1b\[[0-9;]*m//g'`.
- **Evidence commits exclude trace files**, using the directory-anchored pathspec `':(exclude)<dir>/*.bin'` (M44 P1: the bare form stages nothing).
- **Style:** match the surrounding code's comment density and idiom. Comments cite the spec as `M45 §3x` and t0 as `(t0 M2)`. Test names are sentences.
- **Never push.** The merge goes into local `main` only; the push waits for the operator.
- **Halt and ask** on any of the spec's five halt conditions (§7), or on this plan's **Halt 6**: t0 M3's native run of the fixture does not print `kqinit rc=0 carry=0`. That would mean the constant the box returns is not the kernel's. **Route, don't halt** when the walk finds a new wall: re-park with the measurement and name the successor.

## Review Focus

These are the five inputs or failure modes the spec implies but no task's tests would otherwise exercise, most likely first. Each is pinned by a test in the task that owns the code.

1. **A change list that straddles a 16 KiB page.** The two halves can sit on non-contiguous IPAs, so a flat `read_guest` of 72 bytes from the first half's IPA would read the wrong second half, and silently. The entry must be read page by page. Pinned in Task 2: `an_entry_straddling_a_page_is_read_whole` (fixture mode `straddle`).
2. **A change-list pointer that does not translate.** This must stop the recorder by name ("read 0 of 72 bytes"), never fault the host or read IPA 0. Pinned in Task 2 (mode `badptr` in `an_unmeasured_shape_stops_the_recorder_naming_what_differs`) and in Task 1 (`a_short_entry_is_refused_as_untranslated`).
3. **An `int` argument whose upper half is not zero**, for example `kq` loaded with `mov x0, #-1`, which gives `0xffffffffffffffff`. The kernel reads 32 bits, so this is the same call and must be accepted; the M38 `AT_FDCWD` class in reverse. Pinned in Task 1: `an_int_arguments_upper_half_is_ignored_as_the_kernel_ignores_it`.
4. **The call made where libdispatch makes it**, after the process's workqueue is up and a worker thread exists (M44 t0 M1: "issued right after the box's emulated 368/367 pair"). The fixture warms the workqueue with a `dispatch_async` first, in every mode, so every M45 test runs in that context. If Task 3 gates a GCD guest, its test also asserts the issuing thread's tag as t0 M2 measured it.
5. **Seeking across the emulated landmark.** The debugger's positions enter replay mid-trace, and a mirror that only works from landmark 1 would fail there. Pinned in Task 2: `seeks_either_side_of_the_landmark_replay_to_the_end`.

---

## File Structure

| File | Change | Task |
|---|---|---|
| `docs/superpowers/specs/2026-09-28-retrace-m45-kqinit-measurements.md` | create: t0's M1–M5 | 0 |
| `docs/sweep-evidence/2026-09-28-m45-t0/` | create: t0 evidence (stderr, native outputs, README) | 0 |
| `crates/retrace-arch/src/lib.rs` | `SYS_KEVENT_QOS`; the kevent constants, `KeventQos`, `KQINIT`, `kqinit_shape` (Task 1); the 374 documentation row and the `kqueue` row's comment (Task 2) | 1, 2 |
| `crates/retrace-arch/tests/kqinit.rs` | create: the validator's 8 tests | 1 |
| `crates/retrace-arch/tests/census.rs` | + 374 in `CENSUS`, and its doc line | 2 |
| `crates/retrace-guest/c/kqinit_dyn.c` | create: the mechanism fixture | 2 |
| `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs` | build it; `KQINIT_DYN`; `kqinit_guest_parses` | 2 |
| `crates/retrace-box/src/lib.rs` | `Box_::guest_kevent_qos` | 2 |
| `crates/retrace-core/src/lib.rs` | record arm, replay mirror, forward-arm assert | 2 |
| `crates/retrace/tests/kqinit_e2e.rs` | create: the 5-test gate (+1 if Task 3 gates a GCD guest) | 2, 3 |
| `crates/retrace/tests/apple_walls_e2e.rs` | `automationmodetool`: un-ignored or re-parked | 3 |
| `crates/retrace-guest/c/kqgcd_dyn.c`, `build.rs`, `src/lib.rs` | only if a GCD candidate completes | 3 |
| `docs/sweep-evidence/2026-09-28-m45/` | create: the walk's evidence and the sweep | 3 |
| `README.md`, `docs/status-log.md`, `CLAUDE.md` | docs | 4 |
| `.superpowers/sdd/2026-09-28-retrace-m45-kqinit/{predict,gate,tally}.sh` | the close | 5 |

Name each evidence directory for the day it is written. If t0 runs on another day, use that day's date and carry the name forward.

**Test-count prediction (made here, reconciled at the close):**

| Task | Tests added |
|---|---|
| 1 | `retrace-arch/tests/kqinit.rs` 8 (**new binary**) |
| 2 | `retrace-guest` unit `kqinit_guest_parses` 1; `retrace/tests/kqinit_e2e.rs` 5 (**new binary**) |
| 3 | if a GCD guest gates: `kqinit_e2e` +1 and `retrace-guest` unit `kqgcd_guest_parses` +1. If `automationmodetool` has outcome A, 1 test moves from ignored to passed. |

The baseline is M44's close, 832 passed / 0 failed / 9 ignored over 146 binaries. **Prediction:**

- outcome B, no GCD gate: **846 / 0 / 9 over 148**;
- with a GCD gate: 848 / 0 / 9;
- outcome A adds 1 passed and removes 1 ignored from either line.

Task 5 re-derives this from source.

---

### Task 0 (t0): Measurements first

**Files:**
- Create: `docs/superpowers/specs/2026-09-28-retrace-m45-kqinit-measurements.md`
- Create: `docs/sweep-evidence/2026-09-28-m45-t0/README.md`, plus the kept stderr and output files

**Interfaces:**
- Consumes: nothing.
- Produces: the measurements file, which later tasks read by section:
  - **M1:** automationmodetool's full 8 arguments and 72 entry bytes, and the issuing thread.
  - **M2:** per GCD candidate: whether it reaches 374, its shape against M1's, and its thread.
  - **M3:** the fixture's native output.
  - **M4:** automationmodetool's native rc and stdout.
  - **M5:** the `#[test]` count at the base.

**Everything experimental in this task is throwaway.** The only committed files are the measurements file and the evidence directory. Restore every source edit with `git checkout -- <file>` before committing.

- [ ] **Step 1: M1: automationmodetool's whole call**

Throwaway: in `crates/retrace-core/src/lib.rs`, inside `if trace_log { if let Stop::Syscall { num, args } = &stop {`, directly after the `[trap]` `eprintln!`, add this. **Do not add a row for 374 or any arm.** Forwarding it is Halt 4's class.

```rust
                if *num == 374 {
                    let n = 72 * (args[2] as u32 as usize).min(8);
                    eprintln!("[kevent_qos] thread={thread} entry={:02x?}", b.read_va_prefix(args[1], n));
                }
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
mkdir -p $L
export RETRACE_TRACE=1
cargo build -p retrace > $L/t0-m1-build.log 2>&1; echo "exit=$?"
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn /usr/bin/automationmodetool -o /private/tmp/claude-501/m45-amt.bin > $L/t0-m1.out 2> $L/t0-m1.err; echo "exit=$?"
grep -a -E '^\[trap\] num=374 |^\[kevent_qos\]' $L/t0-m1.err
```

Expected: build `exit=0`; record `exit=101` (the M33 panic, unchanged); and exactly these two lines, with only `x1` and the thread free to differ:

```
[trap] num=374 (0x176) pc=0x1804afa48 args=[0xffffffff,<x1>,0x1,0x0,0x0,0x0,0x0,0x21]
[kevent_qos] thread=<t> entry=[01, 00, 00, 00, 00, 00, 00, 00, f6, ff, 21, 00, 00, 00, 00, 02, f8, ff, ff, ff, ff, ff, ff, ff, 00, … 48 zero bytes …]
```

The entry decodes as follows: `ident` 1 (bytes 0–7); `filter` −10 = `f6 ff` (8–9); `flags` `0x21` = `EV_ADD|EV_CLEAR` (10–11); `qos` `0x02000000` (12–15); `udata` `0xfffffffffffffff8` (16–23); then `fflags`, `xflags`, `data` and `ext[4]`, all zero (24–71). **Halt 1** if any byte differs, if any argument other than `x1` differs, or if `pc` differs. A different `pc` means a different libSystem build, so stop and report.

Record the thread number. Keep the throwaway in place for Step 2.

- [ ] **Step 2: M2: three GCD candidates**

Write the three candidates into `$L/t0-m2/`. Each prints `fired\ndone\n` natively.

`$L/t0-m2/timer.c`:

```c
// M45 t0 M2 candidate: a DISPATCH_SOURCE_TYPE_TIMER source, one shot, 1 ms.
#include <dispatch/dispatch.h>
#include <unistd.h>

int main(void) {
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    dispatch_source_t t = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0,
                                                 dispatch_get_global_queue(0, 0));
    dispatch_source_set_timer(t, dispatch_time(DISPATCH_TIME_NOW, 1000000), DISPATCH_TIME_FOREVER, 0);
    dispatch_source_set_event_handler(t, ^{
        write(1, "fired\n", 6);
        dispatch_semaphore_signal(sem);
    });
    dispatch_resume(t);
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    write(1, "done\n", 5);
    return 0;
}
```

`$L/t0-m2/after.c`:

```c
// M45 t0 M2 candidate: dispatch_after, 1 ms, onto the global queue.
#include <dispatch/dispatch.h>
#include <unistd.h>

int main(void) {
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 1000000), dispatch_get_global_queue(0, 0), ^{
        write(1, "fired\n", 6);
        dispatch_semaphore_signal(sem);
    });
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    write(1, "done\n", 5);
    return 0;
}
```

`$L/t0-m2/signal.c`:

```c
// M45 t0 M2 candidate: a DISPATCH_SOURCE_TYPE_SIGNAL source for SIGUSR1, then raise it.
#include <dispatch/dispatch.h>
#include <signal.h>
#include <unistd.h>

int main(void) {
    signal(SIGUSR1, SIG_IGN);
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    dispatch_source_t s = dispatch_source_create(DISPATCH_SOURCE_TYPE_SIGNAL, SIGUSR1, 0,
                                                 dispatch_get_global_queue(0, 0));
    dispatch_source_set_event_handler(s, ^{
        write(1, "fired\n", 6);
        dispatch_semaphore_signal(sem);
    });
    dispatch_resume(s);
    raise(SIGUSR1);
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    write(1, "done\n", 5);
    return 0;
}
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
export RETRACE_TRACE=1
for c in timer after signal; do clang -arch arm64 -o $L/t0-m2/$c $L/t0-m2/$c.c; echo "$c build=$?"; $L/t0-m2/$c > $L/t0-m2/$c.native.out; echo "$c native=$?"; done
for c in timer after signal; do tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn $L/t0-m2/$c -o /private/tmp/claude-501/m45-$c.bin > $L/t0-m2/$c.out 2> $L/t0-m2/$c.err; echo "$c record=$?"; grep -a -E '^\[trap\] num=374 |^\[kevent_qos\]' $L/t0-m2/$c.err; grep -a -E '^\[trap\]' $L/t0-m2/$c.err | tail -1; done
```

For each candidate, record:
- its native exit and output (expected 0, `fired\ndone\n`);
- whether it reaches 374;
- if it does, whether its arguments (other than `x1`) and all 72 entry bytes equal M1's, and its thread;
- if it does not, its record exit and last `[trap]` line.

**Halt 2 is per candidate.** A candidate whose 374 differs from M1's is recorded, and it is neither gated nor modelled in M45. It halts the milestone only if M1 itself failed. Restore the throwaway afterwards:

```bash
git checkout -- crates/retrace-core/src/lib.rs
git status --short
```

Expected: `git status --short` prints nothing (the ledger is ignored).

- [ ] **Step 3: M3: the fixture's native output**

Copy Task 2 Step 1's `kqinit_dyn.c` **verbatim** into `$L/t0-m3/kqinit_dyn.c`. The file is not in the tree yet, and Task 2 commits the identical text. Then:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
clang -arch arm64 -o $L/t0-m3/kqinit_dyn $L/t0-m3/kqinit_dyn.c; echo "build=$?"
for m in "" straddle; do $L/t0-m3/kqinit_dyn $m > $L/t0-m3/native-${m:-plain}.out 2>&1; echo "mode '${m}' rc=$?"; cat $L/t0-m3/native-${m:-plain}.out; done
```

Expected: `build=0`, and both modes `rc=0` printing exactly `kqinit rc=0 carry=0`. **Halt 6** otherwise. The refusal modes (`flags`, `badptr`) are not run natively: they are defined by retrace's refusal, and a native `badptr` is only a host `EFAULT`.

- [ ] **Step 4: M4: automationmodetool natively**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
/usr/bin/automationmodetool </dev/null > $L/t0-m4-native.out 2> $L/t0-m4-native.err; echo "rc=$?"
wc -c < $L/t0-m4-native.out; head -3 $L/t0-m4-native.out; head -3 $L/t0-m4-native.err
```

Record the rc, the stdout byte count and the first line. Task 3 needs them: `records_and_replays_clean` asserts rc 0, so a non-zero native rc means outcome A gets a `launchctl`-style body instead.

- [ ] **Step 5: M5: the base `#[test]` count**

```bash
grep -r -c -E '^\s*#\[test\]' crates --include='*.rs' | awk -F: '{s+=$2} END {print s}'
```

Expected: `839`. M44 closed at 832 + 9 = 841, which is 839 plus the two `census.rs` tests that `legacy_equivalence.rs` compiles a second time (M44 ledger, the reconciliation). A different number is reconciled file by file against M44's `predict.sh` output before Task 1 starts.

- [ ] **Step 6: Write the measurements file and the evidence; commit**

Create `docs/sweep-evidence/2026-09-28-m45-t0/` containing:
- `m1-automationmodetool.err` (copy of `$L/t0-m1.err`);
- `m2-<c>.err` and `m2-<c>.native.out` for each candidate, and each candidate's `.c`;
- `m3-native-plain.out` and `m3-native-straddle.out`;
- `m4-native.out` and `m4-native.err`;
- a `README.md` saying, for each file, which command produced it, on which commit, on which date.

Write `docs/superpowers/specs/2026-09-28-retrace-m45-kqinit-measurements.md` with one section per measurement (`## M1` … `## M5`). Each section gives the command, the result quoted from the evidence file, the decision the spec's rule makes from it, and any halt considered. Then:

```bash
git add docs/superpowers/specs/2026-09-28-retrace-m45-kqinit-measurements.md docs/sweep-evidence/2026-09-28-m45-t0 ':(exclude)docs/sweep-evidence/2026-09-28-m45-t0/*.bin'
git commit -m "M45 t0: measurements M1-M5 — the init's full entry, the GCD candidates, the native return, automationmodetool natively, the base count"
```

---

### Task 1: The validator (`retrace-arch`)

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs`: add `SYS_KEVENT_QOS` after `pub const SYS_WORKQ_KERNRETURN: u64 = 368;`, and a new section before the line `// ---- M12-signal-delivery`.
- Create: `crates/retrace-arch/tests/kqinit.rs`

**Interfaces:**
- Consumes: t0 M1, which confirms `KQINIT`'s bytes.
- Produces, all `pub` in `retrace_arch`:
  - `SYS_KEVENT_QOS: u64`
  - `EVFILT_USER: i16`, `EV_ADD: u16`, `EV_ENABLE: u16`, `EV_CLEAR: u16`
  - `KEVENT_FLAG_IMMEDIATE: u32`, `KEVENT_FLAG_WORKQ: u32`
  - `KEVENT_QOS_SIZE: usize`
  - `struct KeventQos`, with `from_bytes(&[u8; 72]) -> KeventQos`, `to_bytes(&self) -> [u8; 72]` and `fields(&self) -> [(&'static str, u64); 12]`
  - `KQINIT: KeventQos`
  - `kqinit_shape(args: [u64; 8], entry: &[u8]) -> Result<(), String>`

- [ ] **Step 1: Write the failing test**

Create `crates/retrace-arch/tests/kqinit.rs`:

```rust
//! M45 (spec §3b, §4): the validator that decides whether a `kevent_qos` (374) is libdispatch's
//! measured workqueue-kqueue init. Pure and VM-free. `kqinit_shape` is the whole modelled surface:
//! `Box_::guest_kevent_qos` returns 0 on `Ok` and refuses on `Err`, so every shape this file does
//! not accept is a shape retrace stops on by name.
use retrace_arch::{kqinit_shape, KEVENT_QOS_SIZE, KQINIT};

/// The call as M44 t0 M1 measured it (`[trap] num=374 pc=0x1804afa48
/// args=[0xffffffff,0x27ff348,0x1,0x0,0x0,0x0,0x0,0x21]`), re-measured by M45 t0 M1.
const MEASURED: [u64; 8] = [0xffff_ffff, 0x27f_f348, 1, 0, 0, 0, 0, 0x21];

fn entry() -> [u8; KEVENT_QOS_SIZE] { KQINIT.to_bytes() }

#[test]
fn the_measured_init_is_accepted() {
    assert_eq!(kqinit_shape(MEASURED, &entry()), Ok(()));
}

/// R1: the entry is compared exactly, all 72 bytes. A field left unchecked would survive a flip.
#[test]
fn every_single_bit_flip_of_the_entry_is_refused() {
    for byte in 0..KEVENT_QOS_SIZE {
        for bit in 0..8 {
            let mut e = entry();
            e[byte] ^= 1 << bit;
            assert!(kqinit_shape(MEASURED, &e).is_err(), "byte {byte} bit {bit} flipped and still accepted");
        }
    }
}

/// Each field's refusal names it, and flipping bit 0 of the byte at xnu's offset is what reaches
/// it, so this pins the layout (`event_private.h:115-125`) as well as the message.
#[test]
fn each_entry_field_is_named_at_its_xnu_offset() {
    let at = [("ident", 0), ("filter", 8), ("flags", 10), ("qos", 12), ("udata", 16), ("fflags", 24),
        ("xflags", 28), ("data", 32), ("ext[0]", 40), ("ext[1]", 48), ("ext[2]", 56), ("ext[3]", 64)];
    for (name, off) in at {
        let mut e = entry();
        e[off] ^= 1;
        let err = kqinit_shape(MEASURED, &e).unwrap_err();
        assert!(err.starts_with(&format!("changelist[0].{name} is ")), "offset {off}: {err}");
    }
}

/// R2: `kq`, `nchanges`, `nevents` and `flags` are C `int`/`unsigned int`, and the kernel reads 32
/// bits of each. `mov x0, #-1` (`0xffffffffffffffff`) is the same call as the measured
/// `0xffffffff`.
#[test]
fn an_int_arguments_upper_half_is_ignored_as_the_kernel_ignores_it() {
    for i in [0, 2, 4, 7] {
        for bit in 32..64 {
            let mut a = MEASURED;
            a[i] ^= 1u64 << bit;
            assert_eq!(kqinit_shape(a, &entry()), Ok(()), "x{i} bit {bit}: an int's upper half is not the kernel's");
        }
    }
}

#[test]
fn every_bit_the_kernel_reads_of_each_checked_argument_is_refused_by_register() {
    for (i, width) in [(0, 32), (2, 32), (3, 64), (4, 32), (5, 64), (6, 64), (7, 32)] {
        for bit in 0..width {
            let mut a = MEASURED;
            a[i] ^= 1u64 << bit;
            let err = kqinit_shape(a, &entry()).unwrap_err();
            assert!(err.starts_with(&format!("x{i} (")), "x{i} bit {bit}: {err}");
        }
    }
}

/// `x1` is WHERE the entry is, not what it is. The box reads the entry through it; the validator
/// judges the bytes.
#[test]
fn the_change_list_address_is_not_compared() {
    for x1 in [0, 0x10, 0x27f_f348, 0x1_0000_4000, u64::MAX] {
        let mut a = MEASURED;
        a[1] = x1;
        assert_eq!(kqinit_shape(a, &entry()), Ok(()), "x1 = {x1:#x}");
    }
}

/// `read_va_prefix` stops at the first byte that does not translate, so an entry the guest's page
/// tables do not fully map arrives short. It must be refused by name, never padded or guessed.
#[test]
fn a_short_entry_is_refused_as_untranslated() {
    for len in [0, 1, 40, 71] {
        let err = kqinit_shape(MEASURED, &entry()[..len]).unwrap_err();
        assert!(err.contains(&format!("read {len} of 72 bytes")), "len {len}: {err}");
    }
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

/// The constants are the SDK's, read from its headers at test time (M44 R5's method), so a value
/// typed from memory cannot satisfy this. `KEVENT_FLAG_WORKQ` is the one the SDK does not define;
/// it is xnu's `event_private.h:141`. This asserts that too, so an SDK that starts shipping it is
/// noticed and cited instead.
#[test]
fn the_constants_are_the_sdks() {
    let ev = sdk_header("sys/event.h");
    assert_eq!(define(&ev, "EVFILT_USER"), Some(i64::from(retrace_arch::EVFILT_USER)));
    assert_eq!(define(&ev, "EV_ADD"), Some(i64::from(retrace_arch::EV_ADD)));
    assert_eq!(define(&ev, "EV_ENABLE"), Some(i64::from(retrace_arch::EV_ENABLE)));
    assert_eq!(define(&ev, "EV_CLEAR"), Some(i64::from(retrace_arch::EV_CLEAR)));
    assert_eq!(define(&ev, "KEVENT_FLAG_IMMEDIATE"), Some(i64::from(retrace_arch::KEVENT_FLAG_IMMEDIATE)));
    assert_eq!(define(&ev, "KEVENT_FLAG_WORKQ"), None,
        "the SDK now defines KEVENT_FLAG_WORKQ: cite it from the SDK instead of xnu");
    let sc = sdk_header("sys/syscall.h");
    assert_eq!(define(&sc, "SYS_kevent_qos"), Some(retrace_arch::SYS_KEVENT_QOS as i64));
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
cargo test -p retrace-arch --test kqinit --no-fail-fast -- --test-threads=1 > $L/t1-red.log 2>&1; echo "exit=$?"
grep -a -E 'error\[E0432\]|unresolved import' $L/t1-red.log | head -3
```

Expected: `exit=101`, and an `unresolved import` naming `kqinit_shape` / `KEVENT_QOS_SIZE` / `KQINIT`.

- [ ] **Step 3: Implement**

In `crates/retrace-arch/src/lib.rs`, directly after `pub const SYS_WORKQ_KERNRETURN: u64 = 368;`:

```rust
/// `kevent_qos(int kq, const struct kevent_qos_s *changelist, int nchanges, struct kevent_qos_s
/// *eventlist, int nevents, void *data_out, size_t *data_available, unsigned int flags)`. This is
/// xnu's private `bsd/sys/event_private.h`; the SDK carries only the number. **Never forwarded**
/// (M45): with `KEVENT_FLAG_WORKQ` the kernel resolves `kq` to the PROCESS's workqueue kqueue,
/// allocating it if absent (`kern_event.c` `kevent_get_kqwq`), which is retrace's own, the class
/// `SYS_WORKQ_OPEN` names (M44 t0 M1). `Box_::guest_kevent_qos` emulates exactly one shape,
/// libdispatch's `_dispatch_kq_init` (`kqinit_shape`), and refuses every other by value.
pub const SYS_KEVENT_QOS: u64 = 374;
```

Directly before the line `// ---- M12-signal-delivery`:

```rust
// ---- M45-kqinit: libdispatch's workqueue-kqueue initialisation ------------------------------------
// Flag and filter values from the macOS 26 SDK's `sys/event.h`, which tests/kqinit.rs re-reads at
// test time. The exception is `KEVENT_FLAG_WORKQ`, which the SDK does not ship: it is xnu's
// `bsd/sys/event_private.h:141`, and the same test asserts the SDK still lacks it.
/// `EVFILT_USER` (`sys/event.h:77`), as the `i16` a `kevent_qos_s` carries.
pub const EVFILT_USER: i16 = -10;
/// `EV_ADD` (`sys/event.h:136`).
pub const EV_ADD: u16 = 0x0001;
/// `EV_ENABLE` (`sys/event.h:138`). Not in the measured shape; `kqinit_dyn`'s refusal mode adds it.
pub const EV_ENABLE: u16 = 0x0004;
/// `EV_CLEAR` (`sys/event.h:143`).
pub const EV_CLEAR: u16 = 0x0020;
/// `KEVENT_FLAG_IMMEDIATE` (`sys/event.h:132`): poll, never block.
pub const KEVENT_FLAG_IMMEDIATE: u32 = 0x1;
/// `KEVENT_FLAG_WORKQ` (xnu `bsd/sys/event_private.h:141`): "interact with the default workq kq".
pub const KEVENT_FLAG_WORKQ: u32 = 0x20;
/// `sizeof(struct kevent_qos_s)` (xnu `event_private.h:115-125`). The struct has no padding.
pub const KEVENT_QOS_SIZE: usize = 72;

/// `struct kevent_qos_s` (xnu `bsd/sys/event_private.h:115-125`), little-endian. Offsets: `ident`
/// 0, `filter` 8, `flags` 10, `qos` 12, `udata` 16, `fflags` 24, `xflags` 28, `data` 32, `ext` 40.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeventQos {
    pub ident: u64,
    pub filter: i16,
    pub flags: u16,
    pub qos: i32,
    pub udata: u64,
    pub fflags: u32,
    pub xflags: u32,
    pub data: i64,
    pub ext: [u64; 4],
}

impl KeventQos {
    pub fn from_bytes(b: &[u8; KEVENT_QOS_SIZE]) -> KeventQos {
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let u16_at = |o: usize| u16::from_le_bytes(b[o..o + 2].try_into().unwrap());
        KeventQos {
            ident: u64_at(0),
            filter: u16_at(8) as i16,
            flags: u16_at(10),
            qos: u32_at(12) as i32,
            udata: u64_at(16),
            fflags: u32_at(24),
            xflags: u32_at(28),
            data: u64_at(32) as i64,
            ext: [u64_at(40), u64_at(48), u64_at(56), u64_at(64)],
        }
    }

    pub fn to_bytes(&self) -> [u8; KEVENT_QOS_SIZE] {
        let mut b = [0u8; KEVENT_QOS_SIZE];
        b[0..8].copy_from_slice(&self.ident.to_le_bytes());
        b[8..10].copy_from_slice(&self.filter.to_le_bytes());
        b[10..12].copy_from_slice(&self.flags.to_le_bytes());
        b[12..16].copy_from_slice(&self.qos.to_le_bytes());
        b[16..24].copy_from_slice(&self.udata.to_le_bytes());
        b[24..28].copy_from_slice(&self.fflags.to_le_bytes());
        b[28..32].copy_from_slice(&self.xflags.to_le_bytes());
        b[32..40].copy_from_slice(&self.data.to_le_bytes());
        for (i, e) in self.ext.iter().enumerate() {
            b[40 + 8 * i..48 + 8 * i].copy_from_slice(&e.to_le_bytes());
        }
        b
    }

    /// Every field as `(name, bits)` in layout order, a signed field as its two's-complement bits
    /// at its own width, so a refusal prints `filter` as `0xfff6`, never a sign-extended `u64`.
    pub fn fields(&self) -> [(&'static str, u64); 12] {
        [
            ("ident", self.ident),
            ("filter", u64::from(self.filter as u16)),
            ("flags", u64::from(self.flags)),
            ("qos", u64::from(self.qos as u32)),
            ("udata", self.udata),
            ("fflags", u64::from(self.fflags)),
            ("xflags", u64::from(self.xflags)),
            ("data", self.data as u64),
            ("ext[0]", self.ext[0]),
            ("ext[1]", self.ext[1]),
            ("ext[2]", self.ext[2]),
            ("ext[3]", self.ext[3]),
        ]
    }
}

/// The one entry M45 models: libdispatch's `_dispatch_kq_init` (`event_kevent.c:689-699`).
/// `ident`, `filter`, `flags`, `qos` and `udata` were measured by M44 t0 M1, and the other seven
/// fields (all zero) by M45 t0 M1.
pub const KQINIT: KeventQos = KeventQos {
    ident: 1,
    filter: EVFILT_USER,
    flags: EV_ADD | EV_CLEAR,
    qos: 0x0200_0000, // _PTHREAD_PRIORITY_EVENT_MANAGER_FLAG
    udata: !0x7,      // DISPATCH_WLH_MANAGER
    fflags: 0,
    xflags: 0,
    data: 0,
    ext: [0; 4],
};

/// Is this `kevent_qos` call the measured init? `Ok` iff every argument the kernel reads, and all
/// 72 bytes of the one change-list entry, equal the measurement (M45 §2a). Otherwise `Err` names
/// the first difference: the register or field, what it is, and what was measured.
///
/// `int` and `unsigned int` parameters are compared on the 32 bits the kernel reads (R2; the M38
/// `AT_FDCWD` lesson), and pointers whole. `x1`, the change list's address, is where the entry is
/// rather than what it is, and is not compared: the caller reads the entry through it and passes
/// the bytes. A short `entry` means the change list did not fully translate.
pub fn kqinit_shape(args: [u64; 8], entry: &[u8]) -> Result<(), String> {
    const LOW: u64 = 0xffff_ffff;
    let checks: [(usize, &str, u64, u64); 7] = [
        (0, "kq, as int", 0xffff_ffff, args[0] & LOW),
        (2, "nchanges, as int", 1, args[2] & LOW),
        (3, "eventlist", 0, args[3]),
        (4, "nevents, as int", 0, args[4] & LOW),
        (5, "data_out", 0, args[5]),
        (6, "data_available", 0, args[6]),
        (7, "flags, as unsigned int", u64::from(KEVENT_FLAG_WORKQ | KEVENT_FLAG_IMMEDIATE), args[7] & LOW),
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
    for ((name, got), (_, want)) in KeventQos::from_bytes(bytes).fields().into_iter().zip(KQINIT.fields()) {
        if got != want {
            return Err(format!("changelist[0].{name} is {got:#x}, measured {want:#x}"));
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run it to verify it passes, plus the crate's other tests and clippy**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
cargo test -p retrace-arch --no-fail-fast -- --test-threads=1 > $L/t1-green.log 2>&1; echo "exit=$?"
grep -a '^test result:' $L/t1-green.log
cargo clippy -p retrace-arch --all-targets -- -D warnings > $L/t1-clippy.log 2>&1; echo "exit=$?"
```

Expected: `exit=0` for both. `kqinit.rs` shows `8 passed`, and every other `test result:` line shows `0 failed`.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-arch/src/lib.rs crates/retrace-arch/tests/kqinit.rs
git commit -m "M45 t1: kqinit_shape — the measured workqueue-kqueue init, and every other shape named"
```

---

### Task 2: The emulation, test-first

**Files:**
- Create: `crates/retrace-guest/c/kqinit_dyn.c`
- Modify: `crates/retrace-guest/build.rs` (after the `exec_dyn` block), `crates/retrace-guest/src/lib.rs` (after `EXEC_DYN`, plus a unit test after `exec_guest_parses`)
- Create: `crates/retrace/tests/kqinit_e2e.rs`
- Modify: `crates/retrace-arch/src/lib.rs`, in the `arg_kinds` "threads / workqueue" section after the `SYS_WORKQ_KERNRETURN` row, and the `kqueue` (362) row's comment
- Modify: `crates/retrace-arch/tests/census.rs` (`CENSUS` and the doc comment)
- Modify: `crates/retrace-box/src/lib.rs`: `guest_kevent_qos`, directly before `/// Reserve the main thread's believed-but-unbacked stack (M8 spec risk R3).`
- Modify: `crates/retrace-core/src/lib.rs`: record arm after the `SYS_WORKQ_KERNRETURN` arm; replay mirror after the `SYS_WORKQ_KERNRETURN` mirror; assert in the generic forward arm after the workq assert

**Interfaces:**
- Consumes (Task 1): `retrace_arch::{SYS_KEVENT_QOS, KEVENT_QOS_SIZE, kqinit_shape}`.
- Produces:
  - `pub fn Box_::guest_kevent_qos(&self, args: [u64; 8]) -> u64`
  - `retrace_guest::KQINIT_DYN: &str`
  - the refusal text `M45: unmeasured kevent_qos shape: <kqinit_shape's Err>`
  - the divergence text `kevent_qos rc mismatch: replay {rc:#x} != recorded {ret:#x}`

- [ ] **Step 1: The fixture and its build wiring**

Create `crates/retrace-guest/c/kqinit_dyn.c`:

```c
// M45. The kqinit fixture: libdispatch's `_dispatch_kq_init` call (`event_kevent.c:689-699`),
// issued by hand through `svc #0x80`, byte for byte what M44/M45 t0 M1 measured automationmodetool
// issue. x0 = 0xffffffff (int -1, no kqueue), one change-list entry, no event list, and
// x7 = KEVENT_FLAG_WORKQ | KEVENT_FLAG_IMMEDIATE. It uses inline `svc`, not syscall(3):
// libSystem's syscall() goes through the indirect SYS_syscall (0), so retrace would see syscall 0
// (M45 R4).
//
// Every mode first brings up the process's workqueue with one dispatch_async, because that is the
// context libdispatch makes this call in: M44 t0 M1 measured it right after the workqueue pair.
//
// argv[1] selects the mode:
//   (none)    the measured call from a stack entry
//   straddle  the measured call from an entry that straddles a 16 KiB page boundary
//   flags     the entry's flags are EV_ADD|EV_CLEAR|EV_ENABLE (0x25): retrace must refuse it
//   badptr    the change list is address 0x10, which no page maps: retrace must refuse it
//
// Stdout in the two call modes: `kqinit rc=0 carry=0` (M45 t0 M3 measured it natively).
#include <dispatch/dispatch.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// xnu bsd/sys/event_private.h:115-125. Not in the SDK.
struct kevent_qos_s {
    uint64_t ident;
    int16_t  filter;
    uint16_t flags;
    int32_t  qos;
    uint64_t udata;
    uint32_t fflags;
    uint32_t xflags;
    int64_t  data;
    uint64_t ext[4];
};
_Static_assert(sizeof(struct kevent_qos_s) == 72, "xnu event_private.h: 72 bytes, no padding");

// Every register the call carries is an in/out operand, so the compiler assumes none survives the
// `svc`. The carry flag is the kernel's error bit, read straight after the trap.
static uint64_t kevent_qos_workq(uint64_t changelist, uint32_t *carry) {
    register uint64_t x0 __asm__("x0") = 0xffffffffu;  // (int)-1 in w0, upper half zero, as measured
    register uint64_t x1 __asm__("x1") = changelist;
    register uint64_t x2 __asm__("x2") = 1;            // nchanges
    register uint64_t x3 __asm__("x3") = 0;            // eventlist
    register uint64_t x4 __asm__("x4") = 0;            // nevents
    register uint64_t x5 __asm__("x5") = 0;            // data_out
    register uint64_t x6 __asm__("x6") = 0;            // data_available
    register uint64_t x7 __asm__("x7") = 0x21;         // KEVENT_FLAG_WORKQ | KEVENT_FLAG_IMMEDIATE
    register uint64_t x16 __asm__("x16") = 374;        // SYS_kevent_qos
    uint32_t c;
    __asm__ volatile("svc #0x80\n\tcset %w[c], cs"
        : "+r"(x0), "+r"(x1), "+r"(x2), "+r"(x3), "+r"(x4), "+r"(x5), "+r"(x6), "+r"(x7), "+r"(x16),
          [c] "=r"(c)
        :
        : "memory", "cc");
    *carry = c;
    return x0;
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "";

    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    dispatch_async(dispatch_get_global_queue(0, 0), ^{ dispatch_semaphore_signal(sem); });
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);

    struct kevent_qos_s kev = {
        .ident = 1,
        .filter = -10,             // EVFILT_USER
        .flags = 0x0001 | 0x0020,  // EV_ADD | EV_CLEAR
        .qos = 0x02000000,         // _PTHREAD_PRIORITY_EVENT_MANAGER_FLAG
        .udata = ~(uint64_t)0x7,   // DISPATCH_WLH_MANAGER
    };
    uint64_t changelist = (uint64_t)&kev;
    if (strcmp(mode, "flags") == 0) {
        kev.flags |= 0x0004;       // EV_ENABLE
    } else if (strcmp(mode, "badptr") == 0) {
        changelist = 0x10;
    } else if (strcmp(mode, "straddle") == 0) {
        void *buf = NULL;
        if (posix_memalign(&buf, 16384, 32768) != 0) return 2;
        // 16384 - 40: 40 bytes in the first page, 32 in the second, and 8-byte aligned.
        char *at = (char *)buf + 16384 - 40;
        memcpy(at, &kev, sizeof kev);
        changelist = (uint64_t)at;
    } else if (mode[0] != '\0') {
        fprintf(stderr, "kqinit_dyn: unknown mode %s\n", mode);
        return 2;
    }
    uint32_t carry = 0;
    uint64_t rc = kevent_qos_workq(changelist, &carry);
    printf("kqinit rc=%llu carry=%u\n", (unsigned long long)rc, carry);
    return 0;
}
```

In `crates/retrace-guest/build.rs`, directly after the `exec_dyn` block (after its `assert!(status.success(), "exec_dyn guest build failed");`):

```rust

    // kqinit_dyn: the M45 fixture — libdispatch's workqueue-kqueue init call, issued by hand
    // through `svc`, after a dispatch_async brings the workqueue up. Same recipe as hello_dyn.
    let src = format!("{}/c/kqinit_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/kqinit_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang kqinit_dyn");
    assert!(status.success(), "kqinit_dyn guest build failed");
```

In `crates/retrace-guest/src/lib.rs`, directly after the `EXEC_DYN` constant:

```rust
/// M45: libdispatch's workqueue-kqueue init (`kevent_qos` 374), issued by hand after a
/// `dispatch_async`. `argv[1]` selects `straddle`, `flags` or `badptr`; see the source's header.
pub const KQINIT_DYN: &str = concat!(env!("OUT_DIR"), "/kqinit_dyn");
```

And in its `mod tests`, directly after `exec_guest_parses`:

```rust
    #[test]
    fn kqinit_guest_parses() {
        // M45: proves the build.rs wiring and the path constant; behaviour is kqinit_e2e's.
        let l = parse_macho(&std::fs::read(KQINIT_DYN).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
    }
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
cargo test -p retrace-guest kqinit_guest_parses --no-fail-fast -- --test-threads=1 > $L/t2-guest.log 2>&1; echo "exit=$?"
grep -a '^test result:' $L/t2-guest.log
```

Expected: `exit=0`, `1 passed`.

- [ ] **Step 2: Write the failing gate**

Create `crates/retrace/tests/kqinit_e2e.rs`:

```rust
// M45 gate (spec §3e, §4). libdispatch's workqueue-kqueue init, `kevent_qos` (374) with
// KEVENT_FLAG_WORKQ, is EMULATED as one measured shape returning 0, and every other shape is
// refused by value. The fixture issues the call by hand (`crates/retrace-guest/c/kqinit_dyn.c`),
// so the mechanism is guarded on any machine, with or without a libdispatch path that reaches it.
// Every assertion is on the trace or on the recorder's own words, never on an exit code alone: a
// fixture that never reached 374 exits 0 too.
mod util;

use retrace_trace::Event;
use std::path::{Path, PathBuf};

/// What the fixture prints after the call: t0 M3 measured it natively, in both call modes.
const MARKER: &[u8] = b"kqinit rc=0 carry=0\n";

/// Every `kevent_qos` event in `trace`, with its landmark index (the replay session's `idx`).
fn kevent_events(trace: &Path) -> Vec<(usize, Event)> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().enumerate()
        .filter(|(_, e)| matches!(e, Event::Syscall { num, .. } if *num == retrace_arch::SYS_KEVENT_QOS))
        .collect()
}

/// Record `mode`; assert the one emulated landmark and the marker; replay twice byte-identically.
fn records_one_emulated_init(mode: &[&str]) -> PathBuf {
    let (rec, trace) = util::record_dynamic_args(retrace_guest::KQINIT_DYN, mode);
    assert_eq!(rec.code, 0, "{mode:?}: record: {}", rec.stderr);
    let ev = kevent_events(&trace);
    assert_eq!(ev.len(), 1, "{mode:?}: exactly one kevent_qos landmark: {ev:?}");
    let Event::Syscall { args, ret, err, writes, .. } = &ev[0].1 else { unreachable!() };
    // The difference M45 makes: a forward would carry the host's writes or return; a missing arm
    // never gets here (the recorder panics at M33's row check, or at the forward arm's assert).
    assert_eq!((*ret, *err, writes.len()), (0, false, 0),
        "{mode:?}: the emulation returns 0, clears carry and writes nothing");
    assert_eq!(args[7], 0x21, "{mode:?}: KEVENT_FLAG_WORKQ | KEVENT_FLAG_IMMEDIATE");
    assert_eq!(rec.stdout, MARKER, "{mode:?}: got {:?}", String::from_utf8_lossy(&rec.stdout));
    for n in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "{mode:?}: replay {n}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "{mode:?}: replay {n} stdout");
    }
    trace
}

#[test]
fn the_measured_init_records_as_one_emulated_landmark_and_replays() {
    records_one_emulated_init(&[]);
}

/// Review Focus 1: the entry's two halves sit on different 16 KiB pages, which need not be
/// adjacent in IPA space, so only a page-by-page read gets the second half right.
#[test]
fn an_entry_straddling_a_page_is_read_whole() {
    records_one_emulated_init(&["straddle"]);
}

/// R5: a shape the box has not measured stops the recorder, naming what differs, rather than
/// returning an errno libdispatch would crash on with the cause hidden.
#[test]
fn an_unmeasured_shape_stops_the_recorder_naming_what_differs() {
    for (mode, why) in [
        ("flags", "changelist[0].flags is 0x25, measured 0x21"),
        ("badptr", "the change list's entry read 0 of 72 bytes: it does not fully translate"),
    ] {
        let (rec, trace) = util::record_dynamic_args(retrace_guest::KQINIT_DYN, &[mode]);
        assert_eq!(rec.code, 101, "{mode}: the recorder must stop at the refusal (a panic). stderr:\n{}", rec.stderr);
        assert!(rec.stderr.contains(&format!("M45: unmeasured kevent_qos shape: {why}")),
            "{mode}: the refusal must name {why:?}. stderr:\n{}", rec.stderr);
        assert!(kevent_events(&trace).is_empty(), "{mode}: a refused call appends no landmark");
        assert!(rec.stdout.is_empty(), "{mode}: the guest must not run past the refused call");
    }
}

/// The only test that can see the replay mirror. While the return is a constant, the mirror's
/// compare is vacuous on an honest trace (the M18 t5 mirror's comment says the same of its own).
/// So rewrite the recorded return, and replay must name the mismatch at that landmark; without the
/// mirror, generic replay would feed the 1 to the guest in silence.
#[test]
fn replay_recomputes_the_emulated_return() {
    let trace = records_one_emulated_init(&[]);
    let i = kevent_events(&trace)[0].0;
    let mut ev = retrace_trace::Reader::open(&trace).unwrap();
    if let Event::Syscall { ret, .. } = &mut ev[i] { *ret = 1; }
    let bad = trace.with_extension("rc1.bin");
    let mut w = retrace_trace::Writer::create(&bad).unwrap();
    for e in &ev { w.append(e).unwrap(); }
    drop(w);
    let rp = util::replay(&bad);
    assert_eq!(rp.code, 3, "replay of the rewritten trace must diverge (exit 3): {}", rp.stderr);
    assert!(rp.stderr.contains(&format!("DIVERGENCE at landmark {i} "))
        && rp.stderr.contains("kevent_qos rc mismatch: replay 0x0 != recorded 0x1"),
        "the divergence must be the mirror's, at the 374 landmark {i}: {}", rp.stderr);
}

/// Review Focus 5: a debugger position enters replay mid-trace. Seeking onto the emulated
/// landmark, and to either side of it, must replay to the recorded end.
#[test]
fn seeks_either_side_of_the_landmark_replay_to_the_end() {
    let trace = records_one_emulated_init(&[]);
    let i = kevent_events(&trace)[0].0;
    for n in [i - 1, i, i + 1] {
        let mut s = retrace_core::seek(trace.as_path(), n, 0).unwrap_or_else(|e| panic!("seek ({n}, 0): {e}"));
        loop {
            match s.advance() {
                Ok(retrace_core::Advance::Exited(r)) => {
                    assert_eq!(r.outcome, retrace_core::Outcome::Exit { code: 0 }, "({n}, 0)");
                    assert_eq!(r.stdout, MARKER, "({n}, 0): stdout");
                    break;
                }
                Ok(_) => {}
                Err(d) => panic!("({n}, 0): diverged at landmark {}: {}", d.landmark, d.detail),
            }
        }
    }
}
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
cargo test -p retrace --test kqinit_e2e --no-fail-fast -- --test-threads=1 > $L/t2-red.log 2>&1; echo "exit=$?"
grep -a -E '^test |^test result:' $L/t2-red.log
grep -a -c 'has no arg_kinds row' $L/t2-red.log
```

Expected: `exit=101`, `0 passed; 5 failed`, and the M33 panic text (`has no arg_kinds row`) present: every record stops at 374 today. If `retrace_core::seek`'s first argument does not accept `&Path`, match `llsc_e2e.rs`'s call (`retrace_core::seek(trace(), n, k)` with `trace() -> &'static Path`), and say so in the report.

- [ ] **Step 3: The documentation row and the census**

In `crates/retrace-arch/src/lib.rs`'s `arg_kinds`, directly after the `SYS_WORKQ_KERNRETURN => row!(P, [Scalar, Ptr, Scalar, Scalar]),` line:

```rust
        // kevent_qos(int kq, const struct kevent_qos_s *changelist, int nchanges,
        //   struct kevent_qos_s *eventlist, int nevents, void *data_out, size_t *data_available,
        //   unsigned int flags): xnu-private (bsd/sys/event_private.h). Emulated (M45) as exactly
        // one shape, libdispatch's workqueue-kqueue init (`kqinit_shape`): kq is -1 (a value
        // under KEVENT_FLAG_WORKQ, not a descriptor), the change list is one 72-byte entry, and
        // eventlist/data_out/data_available are NULL — the bounds these kinds cite. No row can
        // describe a FORWARDED kevent_qos (M44 R4: count × size, nested idents); the generic
        // forward arm asserts 374 away, so this row is never consulted by a forward.
        SYS_KEVENT_QOS => row!(P, [Scalar, Ptr, Scalar, Ptr, Scalar, Ptr, Ptr, Scalar]),
```

Replace the `kqueue` row's comment. The old text is:

```rust
        // kqueue(void) → a NEW descriptor (bsd/kern/kern_event.c `kqueue`). Bound like open's
        // (EXPECTED_DIFFS; exercised by /bin/wait4path). No kevent spelling (363/369/374/375) is
        // in the census, so nothing yet consumes the bound slot.
```

The new text:

```rust
        // kqueue(void) → a NEW descriptor (bsd/kern/kern_event.c `kqueue`). Bound like open's
        // (EXPECTED_DIFFS; exercised by /bin/wait4path). Nothing yet consumes the bound slot: 374
        // is emulated only for the workqueue kqueue (M45), never on a descriptor, and 363/369/375
        // have no row.
```

In `crates/retrace-arch/tests/census.rs`, add `374` to `CENSUS` between `372` and `381`. Append this paragraph to the file's `//!` doc comment, after the M44 paragraph:

```rust
//!
//! M45 adds 374 `kevent_qos`, measured from the same `[trap] num=` lines by M44 t0 M1 and M45 t0 M1
//! (`/usr/bin/automationmodetool`): libdispatch's workqueue-kqueue init, emulated since M45 and
//! documented by its row.
```

The row has no `Fd`, `Dest`, `NestedDest`, `Source` or `Ret::Fd` kind, so no `legacy_equivalence` view differs, and **no `EXPECTED_DIFFS` entry is added**. Step 5's run confirms it.

- [ ] **Step 4: The box method and the three arms**

In `crates/retrace-box/src/lib.rs`, directly before `    /// Reserve the main thread's believed-but-unbacked stack (M8 spec risk R3).`:

```rust
    /// `kevent_qos(kq, changelist, nchanges, eventlist, nevents, data_out, data_available, flags)`
    /// with `KEVENT_FLAG_WORKQ`: libdispatch's `_dispatch_kq_init` registering the event manager's
    /// `EVFILT_USER` wake-up on the process's workqueue kqueue (M45).
    ///
    /// **Emulated, never forwarded**, for the reason `guest_workq_open` documents, one level up.
    /// With `KEVENT_FLAG_WORKQ`, xnu resolves `kq` to `p->p_fd.fd_wqkqueue`, allocating it if absent
    /// (`kern_event.c` `kevent_get_kqwq`), and that is RETRACE's own process's (M44 t0 M1). It
    /// cannot be refused with an errno either: libdispatch `DISPATCH_CLIENT_CRASH`es on any errno
    /// but `EINTR` (`event_kevent.c:700-709`), so an errno would only move the crash into the guest
    /// and hide why.
    ///
    /// **Exactly one shape is modelled**, the measured one (`retrace_arch::kqinit_shape`), and it is
    /// modelled as the smallest success there is. `KEVENT_FLAG_IMMEDIATE` with no event list places
    /// zero events, so the return is 0 (t0 M3 measured it natively). Nothing is kept, and `&self`
    /// says so: nothing M45 runs reads a knote table. A trigger, a timer, a delete or any other
    /// shape is refused BY VALUE, naming the field, which is `guest_workq_kernreturn`'s stance: a
    /// guessed kevent silently corrupts libdispatch's event state.
    ///
    /// The entry is read through the guest's own stage-1 walk, page by page (`read_va_prefix`), so
    /// an entry straddling two pages is read whole, and one that does not fully translate arrives
    /// short and is refused by the validator.
    ///
    /// Deterministic and above the trace: the only inputs are `args` and 72 bytes of guest memory,
    /// which record and replay hold identically, and both dispatch arms reach this through the same
    /// call. Symmetry rule 1 holds by construction.
    pub fn guest_kevent_qos(&self, args: [u64; 8]) -> u64 {
        let entry = self.read_va_prefix(args[1], retrace_arch::KEVENT_QOS_SIZE);
        if let Err(why) = retrace_arch::kqinit_shape(args, &entry) {
            panic!("M45: unmeasured kevent_qos shape: {why}. Only libdispatch's `_dispatch_kq_init` \
                    (KEVENT_FLAG_WORKQ|IMMEDIATE, one EVFILT_USER EV_ADD|EV_CLEAR entry, no event \
                    list) is modelled (M45 §2a). Measure what issues this one before modelling it; \
                    a guessed kevent silently corrupts libdispatch's event state. args={args:#x?}");
        }
        0
    }

```

In `crates/retrace-core/src/lib.rs`'s `record_box`, directly after the `SYS_WORKQ_KERNRETURN` arm's closing `}`:

```rust
            // M45: kevent_qos is EMULATED, never forwarded (see Box_::guest_kevent_qos). With
            // KEVENT_FLAG_WORKQ the host kernel would act on RETRACE's own workqueue kqueue, the
            // workq pair's class (M44 t0 M1). This arm may PANIC by design: every shape but the
            // measured init is refused by value, naming the field, before anything is appended.
            //
            // `writes` is empty and that is deliberate: the call writes no guest memory, and its
            // return is a constant the replay mirror recomputes identically.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_KEVENT_QOS => {
                let rc = b.guest_kevent_qos(args);
                w.append(&Event::Syscall { num, args, ret: rc, ret1: 0, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append kevent_qos: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);
            }
```

In the generic forward arm (`Stop::Syscall { num, args } => {`, below `// Every other syscall goes through the general memory-diff engine`), directly after the workq assert's closing `);`:

```rust
                // M45: kevent_qos joins them. With KEVENT_FLAG_WORKQ the host kernel resolves the
                // call to RETRACE's own workqueue kqueue (M44 t0 M1); the arm above emulates the one
                // measured shape and refuses the rest. This assert is what makes "never forwarded"
                // a checked fact rather than an arm-ordering accident (the gap M37 measured for
                // bsdthread_create).
                assert!(num != retrace_arch::SYS_KEVENT_QOS,
                    "kevent_qos (374) reached the generic forward arm — it must be emulated above \
                     (M45). Forwarded, KEVENT_FLAG_WORKQ acts on retrace's own workqueue kqueue.");
```

In `ReplaySession::advance`, directly after the `if num == retrace_arch::SYS_WORKQ_KERNRETURN { … }` mirror's closing `}`:

```rust
                            // M45: the record arm's mirror (symmetry rule 1), placed with the workq
                            // mirrors so it inherits the arm-top `verify_thread` and adds none of its
                            // own. While the return is a constant this compare is vacuous on an
                            // honest trace; kqinit_e2e's rewritten-return test is what makes it
                            // observable.
                            if num == retrace_arch::SYS_KEVENT_QOS {
                                let rc = self.b.guest_kevent_qos(args);
                                if rc != *ret {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("kevent_qos rc mismatch: replay {rc:#x} != recorded {ret:#x}") });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
```

- [ ] **Step 5: Run the gate green, plus the neighbours and clippy**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
cargo test -p retrace --test kqinit_e2e --no-fail-fast -- --test-threads=1 > $L/t2-green.log 2>&1; echo "exit=$?"
grep -a -E '^test |^test result:' $L/t2-green.log
cargo test -p retrace-arch --no-fail-fast -- --test-threads=1 > $L/t2-arch.log 2>&1; echo "exit=$?"
grep -a '^test result:' $L/t2-arch.log
for t in dispatch_e2e thread_oracle exec_e2e hello_dyn_e2e; do cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t2-$t.log 2>&1; echo "$t exit=$?"; done
cargo clippy --workspace --all-targets -- -D warnings > $L/t2-clippy.log 2>&1; echo "exit=$?"
```

Expected:
- `kqinit_e2e`: `exit=0`, `5 passed; 0 failed`;
- `retrace-arch`: `exit=0`, and `census`, `legacy_equivalence`, `nocancel` and `kqinit` all with `0 failed`;
- the four neighbours: `exit=0`;
- clippy: `exit=0`.

A `legacy_equivalence` red naming 374 means the row carries a kind the plan says it does not. Fix the row, not the test.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-guest/c/kqinit_dyn.c crates/retrace-guest/build.rs crates/retrace-guest/src/lib.rs crates/retrace/tests/kqinit_e2e.rs crates/retrace-arch/src/lib.rs crates/retrace-arch/tests/census.rs crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs
git commit -m "M45 t2: kevent_qos's workqueue-kqueue init emulated — one measured shape, refused by value otherwise, never forwarded"
```

- [ ] **Step 7: The three controls (on the committed tree)**

Run each control, record the symptom in the task report, then restore:

1. **The record arm deleted.** Delete the M45 record arm (the `Stop::Syscall { num, args } if num == retrace_arch::SYS_KEVENT_QOS => { … }` block, and its comment) from `record_box`. Run `kqinit_e2e`. Expected: `the_measured_init_…` fails, with record `code 101` and stderr carrying `kevent_qos (374) reached the generic forward arm`. Restore with `git checkout -- crates/retrace-core/src/lib.rs`.
2. **The replay mirror deleted.** Delete the M45 `if num == retrace_arch::SYS_KEVENT_QOS { … }` mirror from `ReplaySession::advance`. Run `kqinit_e2e`. Expected: `replay_recomputes_the_emulated_return` fails, since replay no longer reports `kevent_qos rc mismatch` at landmark `i`. It may still diverge later, at the final memory compare, and the report records which. `the_measured_init_…` stays green, which is expected for a constant (M45 §3e). Restore with `git checkout -- crates/retrace-core/src/lib.rs`.
3. **The entry compare deleted.** In `kqinit_shape`, delete the `for ((name, got), (_, want)) in …` loop. Run `kqinit_e2e` and `retrace-arch --test kqinit`. Expected: `an_unmeasured_shape_…` fails on `flags` (the recorder accepts `0x25`), and `every_single_bit_flip_…` and `each_entry_field_…` fail. Restore with `git checkout -- crates/retrace-arch/src/lib.rs`.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
cargo test -p retrace --test kqinit_e2e --no-fail-fast -- --test-threads=1 > $L/t2-control-<n>.log 2>&1; echo "exit=$?"
grep -a -E '^test |^test result:' $L/t2-control-<n>.log
git status --short
```

Expected after each restore: `git status --short` prints nothing.

---

### Task 3: The walk (controller-run for the sweep)

**Files:**
- Modify: `crates/retrace/tests/apple_walls_e2e.rs` (`automationmodetool_records_and_replays`)
- Create (only if a GCD candidate completes): `crates/retrace-guest/c/kqgcd_dyn.c`, plus its `build.rs` block, `KQGCD_DYN` and `kqgcd_guest_parses`, and one test in `kqinit_e2e.rs`
- Create: `docs/sweep-evidence/2026-09-28-m45/` (`README.md`, the sweep log, the walk's stderr)

**Interfaces:**
- Consumes: t0 M2 (the candidates and their threads), t0 M4 (automationmodetool's native rc and stdout), and Task 2's landed emulation.
- Produces: outcome A or B for `automationmodetool`; a GCD gate or an owed entry per candidate; the sweep tally Task 4 writes into the README.

- [ ] **Step 1: automationmodetool on the landed emulation**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/docs/sweep-evidence/2026-09-28-m45
mkdir -p $E
export RETRACE_TRACE=1
cargo build -p retrace > $L/t3-build.log 2>&1; echo "exit=$?"
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn /usr/bin/automationmodetool -o /private/tmp/claude-501/m45-amt2.bin > $E/automationmodetool.rec.out 2> $E/automationmodetool.rec.err; echo "record exit=$?"
grep -a -c '^\[trap\] num=374 ' $E/automationmodetool.rec.err
tail -5 $E/automationmodetool.rec.err
```

Then, only if the record exited with t0 M4's native rc:

```bash
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/docs/sweep-evidence/2026-09-28-m45
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace replay /private/tmp/claude-501/m45-amt2.bin > $E/automationmodetool.rp.out 2> $E/automationmodetool.rp.err; echo "replay exit=$?"
cmp $E/automationmodetool.rec.out $E/automationmodetool.rp.out; echo "cmp=$?"
```

**Decide:**
- **Outcome A:** the record exits with the native rc, its stdout equals t0 M4's native stdout, and replay exits the same with `cmp=0`.
- **Outcome B:** anything else. The first failure is the new wall: the first `RECORD ERROR:` or `panicked at` line, its trap, pc and landmark (count the `[trap]` lines before it), and the rc/rp.

If the new wall is another `kevent_qos` shape (an `M45: unmeasured kevent_qos shape:` panic) or `kevent_id` (375), that is **Halt 3's** case: re-park and route, and do not model it.

- [ ] **Step 2A (outcome A): un-ignore**

If t0 M4's native rc is 0: delete the `#[ignore = "…"]` line above `fn automationmodetool_records_and_replays`.

If t0 M4's native rc is non-zero, `records_and_replays_clean` (which asserts rc 0) is the wrong statement. Replace the test with this, putting t0 M4's measured rc in place of `NATIVE_RC` and its stdout's first line (up to the first `\n`) in place of `NATIVE_HEAD`:

```rust
#[test]
fn automationmodetool_records_and_replays() {
    // M45: un-parked. Its native no-argument outcome, measured by t0 M4, is rc NATIVE_RC with
    // stdout starting NATIVE_HEAD, so this asserts that outcome rather than rc == 0, as launchctl's
    // gate does.
    let path = "/usr/bin/automationmodetool";
    if !std::path::Path::new(path).exists() {
        util::announce(&format!("SKIPPED: {path} is not present on this machine"));
        return;
    }
    let (rec, trace) = util::record_dynamic(path);
    let tail = rec.stderr.lines().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    assert_eq!(rec.code, NATIVE_RC, "{path}: record exited {} — its native no-argument exit is NATIVE_RC (t0 M4):\n{tail}", rec.code);
    assert!(rec.stdout.starts_with(b"NATIVE_HEAD"), "{path}: stdout is not its native output — the guest did not reach main:\n{tail}");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, rec.code, "{path}: replay exited {}: {}", rp.code, rp.stderr.lines().last().unwrap_or(""));
    assert_eq!(rp.stdout, rec.stdout, "{path}: replay stdout differs from the recording");
}
```

Update the file's header comment: append one sentence saying M45 un-parked `automationmodetool`, and why.

- [ ] **Step 2B (outcome B): re-park with the new wall**

Replace the `#[ignore = "…"]` reason with one in the file's house form, every field from Step 1's evidence:

```
M45 wall, class <B|C> (<one-line subsystem or row>), parked<, routed to <successor>|, not routed>. /usr/bin/automationmodetool: `kevent_qos`(374)'s workqueue-kqueue init is emulated since M45 and records at landmark <N> (rc 0, no writes). The run now continues to <the wall in the recorder's own words: its trap/call, number, pc>, landmark <L>, rc/rp <x>/<y> (<'no replay ran' if the record did not exit cleanly>): `<first RECORD ERROR: / panicked at line>`. Evidence docs/sweep-evidence/2026-09-28-m45/automationmodetool.rec.err. UN-IGNORE when <what would clear it>.
```

Update the file's header comment: append one sentence saying M45 moved `automationmodetool` past 374 to the new wall.

- [ ] **Step 3: The GCD candidates on the landed emulation**

For each t0 M2 candidate whose 374 equalled M1's:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/docs/sweep-evidence/2026-09-28-m45
export RETRACE_TRACE=1
for c in <the matching candidates>; do tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn $L/t0-m2/$c -o /private/tmp/claude-501/m45-$c-2.bin > $E/gcd-$c.rec.out 2> $E/gcd-$c.rec.err; echo "$c record=$?"; tail -3 $E/gcd-$c.rec.err; done
```

A candidate that records to exit 0 with `fired\ndone\n` is then replayed twice (`… replay /private/tmp/claude-501/m45-<c>-2.bin`), and each replay's stdout is compared with `cmp`.

**If one or more candidates are clean and bit-identical,** gate the first clean one, in the order timer, after, signal:

1. Copy its source verbatim to `crates/retrace-guest/c/kqgcd_dyn.c`, replacing its first comment line with `// M45 (Task 3): <candidate>, the libdispatch path t0 M2 measured reaching _dispatch_kq_init.`
2. Add a `build.rs` block after `kqinit_dyn`'s, the same recipe with `kqgcd_dyn` for `kqinit_dyn`.
3. Add `pub const KQGCD_DYN: &str = concat!(env!("OUT_DIR"), "/kqgcd_dyn");` after `KQINIT_DYN`, with a one-line doc comment naming the candidate.
4. Add a `kqgcd_guest_parses` test after `kqinit_guest_parses`, the same body with `KQGCD_DYN`.
5. Add this test to `kqinit_e2e.rs`, putting t0 M2's measured thread number for this candidate in place of `T0_M2_THREAD`:

```rust
/// M45 Task 3: a libdispatch path that reaches `_dispatch_kq_init` on its own records the same one
/// emulated landmark, from the thread t0 M2 measured, and replays.
#[test]
fn a_libdispatch_path_reaches_the_same_emulated_init() {
    let (rec, trace) = util::record_dynamic(retrace_guest::KQGCD_DYN);
    assert_eq!(rec.code, 0, "record: {}", rec.stderr);
    let ev = kevent_events(&trace);
    assert_eq!(ev.len(), 1, "exactly one kevent_qos landmark: {ev:?}");
    let Event::Syscall { args, ret, err, writes, thread, .. } = &ev[0].1 else { unreachable!() };
    assert_eq!((*ret, *err, writes.len(), args[7]), (0, false, 0, 0x21));
    assert_eq!(*thread, T0_M2_THREAD, "the issuing thread t0 M2 measured");
    assert_eq!(rec.stdout, b"fired\ndone\n");
    for n in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "replay {n}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {n} stdout");
    }
}
```

Run it: `cargo test -p retrace --test kqinit_e2e --no-fail-fast -- --test-threads=1`, plus `cargo test -p retrace-guest --no-fail-fast -- --test-threads=1`. Expected: `6 passed` and the guest crate green.

**Every candidate that is not gated** goes into the Task 4 status-log "What stays owed" list, with its measured stop: the call, pc and landmark, quoted from `$E/gcd-<c>.rec.err`.

- [ ] **Step 4: The full sweep on this task's binary (controller-run)**

Copy `target/aarch64-apple-darwin/debug/retrace` into the session scratchpad and sign it there, as M39's evidence README describes, so a concurrent `cargo test` cannot swap it. Then:

```bash
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/docs/sweep-evidence/2026-09-28-m45
export RETRACE_SWEEP_KEEP=$E/sweep
tools/apple-sweep.sh <signed copy> > $E/sweep.log 2>&1; echo "exit=$?"
grep -a '^TALLY' $E/sweep.log
```

Expected:
- outcome A: `TALLY pass=50 fail=4 skip=0`;
- outcome B: `pass=49 fail=5 skip=0`, with `automationmodetool`'s row now at the new wall.

Diff every `ROW` label against M44's (`docs/sweep-evidence/2026-09-27-m44/`). Any row other than `automationmodetool`'s that moved is explained by name in the evidence README, or it is **Halt 5**. Write the evidence `README.md` in M44's shape: method, binary hash and commit, the tally, and every moved row with its reason.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace/tests/apple_walls_e2e.rs docs/sweep-evidence/2026-09-28-m45 ':(exclude)docs/sweep-evidence/2026-09-28-m45/*.bin' ':(exclude)docs/sweep-evidence/2026-09-28-m45/sweep/*.bin'
git add crates/retrace-guest crates/retrace/tests/kqinit_e2e.rs
git commit -m "M45 t3: the walk — automationmodetool <un-parked|re-parked at <wall>>, the GCD candidates, the sweep"
```

(`git add crates/retrace-guest crates/retrace/tests/kqinit_e2e.rs` stages nothing when no GCD gate was added. That is expected.)

---

### Task 4: The docs

**Files:**
- Modify: `README.md`, `docs/status-log.md`, `CLAUDE.md`

**Interfaces:** Consumes every task's report, the t0 measurements file and Task 3's evidence. Produces the docs Task 5's reviewer reads.

- [ ] **Step 1: README, edited in place**

Find each passage with `grep -n`, and edit it to describe the new reality:

- **"What works today":** after the paragraph that describes GCD / `dispatch_e2e` (`grep -n 'dispatch_e2e\|dispatch_async' README.md`), add a paragraph. It says libdispatch's workqueue-kqueue init, `kevent_qos` (374) with `KEVENT_FLAG_WORKQ`, is emulated since M45 as exactly one measured shape returning 0 (`Box_::guest_kevent_qos`). It says every other shape stops the recorder by name, and that `kqinit_e2e` guards it with a hand-issued fixture. It also names outcome A or B for `automationmodetool` and any GCD gate.
- **The Apple-sweep intro** (`grep -n 'record and replay\*\*, stdout byte-identical' README.md`) and **the sweep table** (`grep -n 'automationmodetool' README.md`): the new tally, measured date and commit. For `automationmodetool`: outcome A removes its table row and says why; outcome B rewrites its face, class and route.
- **"The faces" paragraph** (`grep -n 'A missing row\*\* is' README.md`): after M45 no sweep binary reaches the missing-row panic through 374. Rewrite the sentence that says it is "reached now only by `automationmodetool`" and the three that follow it (M44's t0 account) to say what M45 did. If Task 3's sweep shows no binary at a missing-row panic, say so and name the evidence file.
- **Known limits, the missing-row paragraph** (`grep -n '374 `kevent_qos` is \*\*routed\*\*' README.md`): replace that sentence with one saying 374 is emulated since M45 for exactly the measured workqueue init. It must also say any other `kevent_qos` shape is refused by value, naming the field, and that `kevent` (363), `kevent64` (369) and `kevent_id` (375) have no row.
- **Known limits, the ignored-gates paragraph** (`grep -n 'automationmodetool` stays at 374' README.md`): outcome A says it was un-parked by M45; outcome B names the new wall.
- **The gate line** (`grep -n '^\*\*Gate:\*\*' README.md`): not touched here. Task 5 Step 4 writes the measured counts.

- [ ] **Step 2: CLAUDE.md**

In "Commands", the e2e list: replace `` `skiplines` (M44: the skip-line detector and its control). Run one with`` with:

```markdown
`skiplines` (M44: the skip-line detector and its control), `kqinit_e2e` (M45: libdispatch's
  workqueue-kqueue init, `kevent_qos` with `KEVENT_FLAG_WORKQ`, issued by hand from `kqinit_dyn.c`
  and emulated as one measured shape; asserts the landmark, the refusals by value, a rewritten
  return replay must name, and seeks across it). Run one with
```

In "Guest threads", replace:

```markdown
The generic arm's asserts
are `is_signal_syscall`, the workq pair and `writes_via_nested_pointer` only;
```

with:

```markdown
The generic arm's asserts
are `is_signal_syscall`, the workq pair, `kevent_qos` (M45) and `writes_via_nested_pointer` only;
```

Then, directly after the sentence that ends `was measured false at M37.`, add:

```markdown
Since M45 libdispatch's workqueue-kqueue init, `kevent_qos` (374) with `KEVENT_FLAG_WORKQ`, is
emulated beside the workq pair for the same reason (forwarded, it acts on retrace's own workqueue
kqueue): exactly one measured shape, returning 0 (`Box_::guest_kevent_qos`), and every other shape
refused by value, naming the field.
```

The line breaks in the quoted CLAUDE.md text are the file's own. Match them exactly when editing.

- [ ] **Step 3: status-log, appended**

Append `## M45-kqinit: libdispatch's workqueue-kqueue init, emulated` at the end of `docs/status-log.md`, never editing an earlier section. Mirror M44's subsections:

- what t0 measured, M1–M5, with the halts considered;
- one subsection per task with its commit hashes, including Task 2's three controls and their symptoms;
- the walk: outcome A or B and the sweep tally;
- the gate (from Task 5);
- what measurement changed;
- rulings: R1–R6 and every ruling made during execution;
- **What stays owed.** Every non-gated GCD candidate with its stop; `automationmodetool`'s new wall, if outcome B; the rest of the `kevent` family (363/369/375, and any `kevent_qos` shape other than the init); triggers, timers and machport knotes; the M44 owed items this milestone did not touch, carried forward by reference to M44's list.

Also add a forward pointer to the M44 section's claim that "374 `kevent_qos`, routed … must be emulated", as a note appended in the M45 section (never an edit of M44's): "M44's routed 374 is emulated by M45; see below."

- [ ] **Step 4: Commit**

```bash
git add README.md CLAUDE.md docs/status-log.md
git commit -m "M45 docs: README in place, status log appended, CLAUDE.md's workqueue paragraph and e2e list"
```

---

### Task 5: The gate, the reconciliation, the merge (controller-run)

**Files:**
- Create: `.superpowers/sdd/2026-09-28-retrace-m45-kqinit/{predict.sh,gate.sh,tally.sh,gate-summary.txt}`

- [ ] **Step 1: Predict the count from source, before the gate**

Create `$L/predict.sh`:

```bash
#!/bin/bash
# M45 close (Task 5 Step 1): predict the gate's passed+ignored from source, before the gate.
# Counts `#[test]` lines per file at the M44 merge (60f0452) and at HEAD, for every .rs under
# crates/*/tests and crates/*/src; prints per-file deltas, totals, and the test-target counts.
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit || exit 1
BASE=60f0452
F=/private/tmp/claude-501/m45-predict-files.txt
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
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
bash $L/predict.sh > $L/predict.txt 2>&1; echo "exit=$?"
cat $L/predict.txt
```

Expected:
- `TOTAL #[test]: 839 -> 853` (+14): `kqinit.rs` +8, `kqinit_e2e.rs` +5, `retrace-guest/src/lib.rs` +1;
- two `TARGETS` lines, each one file up: `crates/retrace-arch/tests` (`kqinit.rs`) and `crates/retrace/tests` (`kqinit_e2e.rs`);
- a GCD gate adds +2 (`kqinit_e2e.rs` +1, `retrace-guest/src/lib.rs` +1).

The prediction is then **passed + ignored = TOTAL + 2** (the `census.rs` pair compiled twice), over **148** binaries. Explain every difference by task before running the gate.

- [ ] **Step 2: The chunked gate**

Create `$L/gate.sh`:

```bash
#!/bin/bash
# M45 close (Task 5 Step 2): the chunked gate, copied from M44's. Every chunk runs --no-fail-fast;
# each chunk's exit code is captured before any pipe and lands in gate-summary.txt. Read the logs,
# never this script's own exit status.
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit || exit 1
D=.superpowers/sdd/2026-09-28-retrace-m45-kqinit
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
# Per-chunk tally of the M45 close gate: passed/failed/ignored over binaries, ANSI stripped.
D=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
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
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
cat $L/gate-summary.txt
bash $L/tally.sh
```

The pass bar:
- `failed` is 0, and every line in `gate-summary.txt` reads `exit=0`;
- `passed + ignored` equals Step 1's prediction;
- the number of `test result:` lines equals 148, or Step 1's binary count.

Any disagreement is reconciled file by file before anything is merged.

- [ ] **Step 4: Fill the README's gate line, then the final review**

Replace the numbers on the README's `**Gate:**` line (`grep -n '^\*\*Gate:\*\*' README.md`) with the tally's, and the sentence after it that names the count of `crates/retrace/tests/` files. Append the gate to the status-log M45 section, then commit:

```bash
git add README.md docs/status-log.md
git commit -m "M45 close: the gate — <passed> passed / 0 failed / <ignored> ignored over <binaries>"
```

Dispatch the whole-branch reviewer (the SDD skill's final review) over `60f0452..HEAD` and apply its fix wave, one commit per item. Re-run only the chunks a fix touched, and re-tally.

- [ ] **Step 5: Merge into local `main`**

From the main checkout (`/Users/noahmitchem/Documents/GitHub/retrace`, not the worktree):

```bash
git merge --no-ff worktree-m45-kqinit -m "Merge M45-kqinit: libdispatch's workqueue-kqueue init, emulated as one measured shape"
git rev-parse 'main^{tree}' 'worktree-m45-kqinit^{tree}'
```

The two tree hashes must be equal, which is the tree-identity proof. Then run one headline gate in the main checkout for direct evidence:

```bash
cargo test -p retrace --test kqinit_e2e -- --test-threads=1
```

**Do not push.** Report the merge commit, and ask the operator about the push and the worktree cleanup. Say that the ledger lives only in the worktree until it is copied to the main checkout's `.superpowers/sdd/`, as M41–M43's were.
