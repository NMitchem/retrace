# M14-threads Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a stock full-`std` Rust guest that calls `std::thread::spawn` and `join` record and replay bit-for-bit, so retrace has a second guest thread of control.

**Architecture:** `Box_` gains a thread table of register contexts (the `BoxState` register subset plus `tpidrro_el0`, which that subset omits) and a `current` index. `bsdthread_create` is **emulated in the box, never forwarded** — it seeds a new context from the guest-supplied stack and libpthread's already-registered trampoline. A cooperative, block-driven scheduler switches only when a thread blocks or exits, picking the lowest-indexed `Runnable` thread; because that choice is a pure function of the guest's own syscall sequence, record and replay produce identical schedules with **nothing recorded** and **no trace format change**. The switch itself reuses the save/restore discipline M9's `flush_guest_tlb` and the PAC signing oracle already established.

**Tech Stack:** Rust 1.95.0 (`aarch64-apple-darwin`), Hypervisor.framework, `clang` for freestanding arm64 guest fixtures, `rustc` for full-`std` guest fixtures.

**Spec:** `docs/superpowers/specs/2026-08-12-retrace-m14-threads-design.md`. Read it before Task 1.

## Existing API this plan builds on — verified at `main` = `c685695`, not assumed

Every signature below was read out of the tree while writing this plan. Use these; do not invent parallel helpers.

| thing | truth | trap it avoids |
|---|---|---|
| `Regs` | `retrace_trace::Regs { x: [u64; 31], pc: u64, sp_el0: u64, cpsr: u64 }`, imported by `retrace-box` at `lib.rs:4` | the stack pointer field is **`sp_el0`**, not `sp` |
| reading all registers | `Box_::regs_snapshot(&self) -> Regs` (`lib.rs:2873`) | there is no `read_regs` |
| `FPCR` / `FPSR` | `hv_sys::reg::FPCR` / `reg::FPSR` — **`Reg`, not `SysReg`** | `get_sys(sysreg::FPCR)` does not compile; use `get_reg(reg::FPCR)` |
| `ELR_EL1`, `SPSR_EL1`, `TPIDRRO_EL0`, `TPIDR_EL0`, `SP_EL0` | `sysreg::*`, via `get_sys`/`set_sys` | — |
| SIMD | `Vcpu::get_simd(SimdReg) -> u128` / `set_simd` | `fp` is `[u128; 32]`, one `get_simd` per register |
| checkpoint | `Box_::checkpoint(&self) -> BoxState` (`lib.rs:2964`) | there is no `capture()` |
| building a `Box_` in a test | `Box_::load(&loaded)` (`lib.rs:919`); see `crates/retrace-box/tests/checkpoint.rs:11-12` | there is **no `for_test()`** — the snippets below use a local `tb()` helper wrapping it |
| `parse_macho` | `retrace_guest::parse_macho(b: &[u8]) -> Loaded` — takes **bytes**, returns `Loaded`, **not fallible** | the `*_GUEST` constants are PATHS; read them first, and do not `.expect()` the result |

**There is no `write_regs` yet.** Task 5 adds one as the inverse of `regs_snapshot`; it is the only new low-level helper this milestone needs.

### Where syscall dispatch actually lives — corrected during Task 3

This plan originally said the per-syscall dispatch is in `crates/retrace-box/src/lib.rs`. **It is not.** `retrace-box` owns `forward_and_diff`, the generic forward primitive, and the `Box_` methods that implement guest semantics — but the `match stop { Stop::Syscall { num, args } … }` dispatch is in **`crates/retrace-core/src/lib.rs`**, in two places that must stay mirrored:

| side | location | M13's `mach_vm_protect` example |
|---|---|---|
| record | `record_box`'s `match stop` | `lib.rs:352` → `b.guest_mprotect(args[1], args[2], args[4])` |
| replay | `ReplaySession::advance` | `lib.rs:1148-1149` → `self.b.guest_mprotect(args[1], args[2], args[4])` |

**Copy that pattern.** A new arm goes in *both*, calling the *same* `Box_` method with the *same* arguments — that identity is what makes symmetry rule 1 hold by construction rather than by inspection. Task 3's `panic!` scaffold is an exception only because it records no event and kills the recorder, so there is nothing for replay to mirror.

Note the arm must sit **before** the generic forward arm. Forwarding syscall 360 to the host is reproducibly fatal (Task 1).

## Global Constraints

- **macOS 26.x on Apple Silicon.** Every binary touching `hv_*` needs `com.apple.security.hypervisor` (ad-hoc signable via `tools/codesign-run.sh`).
- **`--test-threads=1` is mandatory** — one HVF VM per process. `just gate` sets it; a bare `cargo test` flakes with `HV_BUSY`.
- **`just gate` is THE exit gate:** `cargo test --workspace` + `clippy -D warnings`. **Baseline entering this milestone: 311 passed / 0 failed / 1 ignored (94 test binaries), clippy clean**, at `main` = `c685695`. The 1 ignored is `stackoverflow_rust_e2e` (M8 spec risk R3) and it stays ignored — do not "fix" it in this milestone.
- **`cargo test -p retrace --lib` IS INVALID.** `retrace` is a binary-only crate; `--lib` makes cargo fail the *entire* invocation with exit 101 and `error: no library targets found in package 'retrace'`, running nothing — including a `--bins` half passed in the same command. Use `cargo test -p retrace --bins`. This cost real time in M13 Task 12; the failure is indistinguishable from a test failure at the exit-code level.
- **GATE CADENCE.** A full gate is ~11 min wall-clock. Run it **in full** only where a live call site can actually move a dynamic gate: Tasks **7, 8, 9, 11, 12**. (Task 10 is excluded deliberately — it breaks `pick_next`, observes the failure, and reverts, so it ends on already-gated code and its own steps run targeted tests only.) Tasks **3, 4, 5, 6** add code nothing calls yet, so they run **targeted crate tests plus clippy**: `cargo test -p retrace-box --test threads --test checkpoint_seek -- --test-threads=1` and `cargo clippy --workspace --all-targets -- -D warnings`. Per-task count checksums are **batched, not abandoned** — the next full gate must equal the cumulative projection since the last one, and a mismatch is investigated *then*, not waved through.
- **Who runs the gate: the CONTROLLER, never an implementer.** Measured across four attempts in M13: both controller-run gates completed; both subagent-run gates were reaped mid-run once the agent went idle (killed at 44/90; SIGTERM at 19/90). Raising the subagent's Bash timeout keeps the *agent* alive but does not protect its orphaned child process. Implementers run fast crate-level tests and clippy only.
- **Do not run `sudo killall syspolicyd` during a gate.** It is a real remedy for accumulated Gatekeeper load but it **strands any process already mid-code-signature-validation** — that process then blocks forever on a dead daemon with zero accumulated CPU. Kill it *between* runs, never during one.
- **Symmetry rule 1:** a special case in record's `match stop` needs a mirror in replay's dispatch, both recomputing identical bytes. **Symmetry rule 2:** deterministic emulation belongs *below* the trace, inside `Box_::run()`, where it fires identically on both sides. M14's scheduler is a rule-2 citizen; keep it there.
- **Never reimplement Apple's PAC.** Sign/authenticate by running `pac*`/`aut*` on the guest vCPU.
- **Drop order is load-bearing.** `Box_`'s `vcpu` field must stay declared before `vm`. Adding a `threads` field must not reorder those two.

---

## File Structure

**Created:**
- `spikes/threadjoin.c` — Task 1's native measurement of the `pthread_join` blocking primitive.
- `crates/retrace-box/src/thread.rs` — the `Thread` TCB, `ThreadState`, `BlockReason`, the thread table and the scheduler pick function. **A new module, not more `lib.rs`:** `lib.rs` is already the largest, densest file in the workspace, and the scheduler is the one piece of M14 that is pure, VM-free, and exhaustively unit-testable. Keeping it separate is what makes Task 4 cheap to test.
- `crates/retrace-box/tests/threads.rs` — box-level gates for the table, the switch, and deadlock.
- `crates/retrace-guest/rs/threadrust.rs` — the headline guest.
- `crates/retrace/tests/thread_rust_e2e.rs` — the headline gate.
- `.superpowers/sdd/2026-08-12-retrace-m14-threads/` — task reports, measurements, review diffs.

**Modified:**
- `crates/retrace-box/src/lib.rs` — `mod thread;`, the `threads`/`current` fields, the switch, the `bsdthread_create`/`bsdthread_terminate` handlers, `BoxState`'s thread carriage, the deadlock assert.
- `crates/retrace-arch/src/lib.rs` — thread syscall numbers.
- `crates/retrace-guest/build.rs` — compile `threadrust.rs`.
- `crates/retrace-guest/src/lib.rs` — the `THREADRUST` path constant.
- `README.md`, `CLAUDE.md` — Task 12.

---

### Task 1: Spike — what does `pthread_join` actually block on? (R1)

**This task blocks the scheduler design.** The spec deliberately leaves it unmeasured because the walk dies before reaching `join`. Do not skip it and do not guess: M13's equivalent spike overturned a shipped `signal_of_esr` row that had been wrong and unreached for six milestones.

**Files:**
- Create: `spikes/threadjoin.c`
- Create: `.superpowers/sdd/2026-08-12-retrace-m14-threads/task-1-report.md`

**Interfaces:**
- Consumes: nothing.
- Produces: a measured answer naming the blocking syscall (number and name) that `pthread_join` uses on this host, plus whether the *child* blocks on anything before running. Tasks 8 and 9 consume it.

- [ ] **Step 1: Write the spike**

```c
// spikes/threadjoin.c — M14 Task 1. What does pthread_join block on, and what does a thread's
// lifecycle look like in syscalls? Built and run natively; see spikes/README.md for the recipe.
#include <pthread.h>
#include <stdio.h>
#include <unistd.h>

static void *child(void *arg) {
    (void)arg;
    write(1, "child\n", 6);
    return (void *)42;
}

int main(void) {
    write(1, "before\n", 7);
    pthread_t t;
    if (pthread_create(&t, NULL, child, NULL) != 0) { write(2, "create failed\n", 14); return 1; }
    void *ret = NULL;
    pthread_join(t, &ret);
    printf("joined %ld\n", (long)ret);
    return 0;
}
```

- [ ] **Step 2: Build and run it natively**

```bash
cd /Users/noahmitchem/Documents/GitHub/retrace
clang -o spikes/threadjoin spikes/threadjoin.c -lpthread
./spikes/threadjoin; echo "exit=$?"
```

Expected: `before` / `child` / `joined 42`, exit 0. If this fails, stop — the spike itself is wrong.

- [ ] **Step 3: Capture the syscalls `pthread_join` makes**

`dtruss` needs root and a SIP carve-out, so prefer the tool that needs neither: run the same binary **under retrace** and read `RETRACE_TRACE=1`. It dies at `bsdthread_create` exactly as the Rust probe does, which is the point — capture everything *up to and including* that trap, then get the join-side answer from the disassembly of libsystem_pthread's `pthread_join`:

```bash
cd /Users/noahmitchem/Documents/GitHub/retrace
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn ./spikes/threadjoin -o /tmp/tj.bin 2>&1 | tail -40

# The join path, read from the shared cache's own code:
otool -tV /usr/lib/system/libsystem_pthread.dylib 2>/dev/null | sed -n '/_pthread_join:/,/^_/p' | head -80
```

Record which of `psynch_cvwait` (`SYS_PSYNCH_CVWAIT`), `__ulock_wait` (515) / `__ulock_wait2` (516), or a Mach `semaphore_wait` appears. **Report the syscall number, not just the name.**

- [ ] **Step 4: Answer the second question — does `bsdthread_create` reach the host?**

The spec flags this as an unverified hazard: if trap 360 is forwarded, the host may create a real thread inside retrace's own process starting at a guest address.

```bash
cd /Users/noahmitchem/Documents/GitHub/retrace
# Does the dispatch have an allowlist, or is forward the default?
grep -n "Stop::Syscall" -A 40 crates/retrace-core/src/lib.rs | head -60
```

Then settle it empirically — a forwarded `bsdthread_create` creates a host thread, so the recorder's own thread count changes:

```bash
cargo run -q -p retrace -- record-dyn ./spikes/threadjoin -o /tmp/tj2.bin 2>&1 &
RPID=$!; sleep 3; ps -M $RPID 2>/dev/null | wc -l; wait $RPID
```

- [ ] **Step 5: Write the report**

`task-1-report.md` must state, each as a measured fact with the command that produced it:
1. The blocking syscall `pthread_join` uses (number + name).
2. Whether the child blocks before running.
3. Whether `bsdthread_create` currently reaches the host, and if so what the host does with it.
4. **Any way in which this measurement contradicts the spec.** M13's Task 1–2 measurements falsified two inherited claims; say so loudly if it happens again.

- [ ] **Step 6: Commit**

```bash
cd /Users/noahmitchem/Documents/GitHub/retrace
git add spikes/threadjoin.c
git commit -m "M14 t1: spike — what pthread_join blocks on, measured not assumed"
```

(`spikes/*` binaries are gitignored; only the `.c` is committed.)

---

### Task 2: Measure the guest-side landscape after `bsdthread_create`

**Files:**
- Create: `.superpowers/sdd/2026-08-12-retrace-m14-threads/task-2-measurements.md`

**Interfaces:**
- Consumes: Task 1's report.
- Produces: the full trap sequence a threaded guest issues, the `bsdthread_register` trampoline address, and the thread-start register contract. Tasks 7 and 8 consume all three.

- [ ] **Step 1: Capture `bsdthread_register`'s arguments**

This is the call that tells the kernel where to start new threads, and it already fires on every dynamic guest. Its arguments *are* the thread-start contract:

```bash
cd /Users/noahmitchem/Documents/GitHub/retrace
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn \
  target/debug/build/retrace-guest-*/out/hello_rust -o /tmp/hr.bin 2>&1 \
  | grep -E 'num=366|num=372'
```

Record all six arguments of trap 366. On arm64 macOS the signature is
`bsdthread_register(threadstart, wqthread, pthsize, dummy, targetconc, dispatchqueue_offset)`.
**`threadstart` (x0) is the address the box must enter a new thread at.**

- [ ] **Step 2: Confirm the `bsdthread_create` ABI against a second guest**

The spec measured it once, from the Rust probe. Confirm with the C spike so the ABI is not an artifact of Rust's runtime:

```bash
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn ./spikes/threadjoin -o /tmp/tj.bin 2>&1 \
  | grep -E 'num=360|num=-15|num=-14' | tail -5
```

Expected shape, per the spec: `mach_vm_map` (stack) → `mach_vm_protect` with `new_protection = 0`
(guard page) → `bsdthread_create(func, arg, stack, pthread, flags)`. **If the two guests disagree
on argument positions, the C one wins for the ABI and both get documented.**

- [ ] **Step 3: Confirm M13's `mach_vm_protect` handles the guard page correctly**

The spec's headline claim is that M14's guest is M13's first real caller. Verify the protect
succeeds and lands where expected, rather than assuming the dormant path works:

```bash
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn ./spikes/threadjoin -o /tmp/tj.bin 2>&1 \
  | grep -B2 -A2 'num=-14'
```

Confirm: `new_protection == 0`, the address is one granule below the thread stack, and no error or
fail-loud fires. **If M13's path rejects it, that is a Task 3 fix and the plan grows a task.**

- [ ] **Step 4: Write the measurements report**

Must include: trap 366's six arguments; the `threadstart` address; the confirmed 360 ABI; the guard-page protect result; and the total trap count at which the guest dies.

- [ ] **Step 5: Commit**

```bash
git add .superpowers/sdd/2026-08-12-retrace-m14-threads/task-2-measurements.md 2>/dev/null || true
git commit --allow-empty -m "M14 t2: measure the guest-side threading landscape"
```

(`.superpowers/` self-ignores via a nested `.gitignore` containing `*`; the commit is a marker.)

---

### Task 3: Thread syscall numbers, and make the silent death loud

The spec calls today's behaviour a defect in its own right: retrace prints **nothing** when a guest hits `bsdthread_create`, then dies with exit 133. Fix that before building anything on top of it, so every later task has a diagnostic to read.

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs`
- Test: `crates/retrace-arch/src/lib.rs` (inline `#[cfg(test)]`, matching the existing `assert_eq!((SYS_PTHREAD_KILL, SYS_PTHREAD_SIGMASK, SYS_SIGWAIT), (328, 329, 330));` pattern)

**Interfaces:**
- Consumes: nothing.
- Produces: `SYS_BSDTHREAD_CREATE: u64 = 360`, `SYS_BSDTHREAD_TERMINATE: u64 = 361`, `SYS_BSDTHREAD_REGISTER: u64 = 366`, `SYS_THREAD_SELFID: u64 = 372`. Tasks 7, 8, 9 consume these.

- [ ] **Step 1: Write the failing test**

Append to the existing test module in `crates/retrace-arch/src/lib.rs`:

```rust
#[test]
fn thread_syscall_numbers_are_the_darwin_ones() {
    // Measured on macOS 26 (M14 Task 2): a NON-threading Rust guest already issues 366 and 372.
    assert_eq!(
        (SYS_BSDTHREAD_CREATE, SYS_BSDTHREAD_TERMINATE, SYS_BSDTHREAD_REGISTER, SYS_THREAD_SELFID),
        (360, 361, 366, 372)
    );
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cd /Users/noahmitchem/Documents/GitHub/retrace
cargo test -p retrace-arch thread_syscall_numbers -- --test-threads=1
```

Expected: FAIL to **compile** — `cannot find value SYS_BSDTHREAD_CREATE in this scope`. A compile failure is a legitimate red here; do not "fix" it by writing the constants first.

- [ ] **Step 3: Add the constants**

Next to `SYS_PTHREAD_KILL`/`SYS_PTHREAD_SIGMASK` in `crates/retrace-arch/src/lib.rs`:

```rust
/// `bsdthread_create(func, func_arg, stack, pthread, flags)`. **Never forwarded** — the host would
/// create a real thread inside retrace's own process, starting at a GUEST address. M14 emulates it.
pub const SYS_BSDTHREAD_CREATE: u64 = 360;
/// `bsdthread_terminate(stackaddr, freesize, port, sem)` — a guest thread's exit.
pub const SYS_BSDTHREAD_TERMINATE: u64 = 361;
/// `bsdthread_register(threadstart, wqthread, pthsize, …)`. Already fires on EVERY dynamic guest
/// since M7, unremarked; `threadstart` is the address a new thread must be entered at.
pub const SYS_BSDTHREAD_REGISTER: u64 = 366;
/// `thread_selfid()` — already fires and already survives.
pub const SYS_THREAD_SELFID: u64 = 372;
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cargo test -p retrace-arch thread_syscall_numbers -- --test-threads=1
```
Expected: PASS.

- [ ] **Step 5: Make `bsdthread_create` announce itself**

In `Box_`'s syscall dispatch in `crates/retrace-box/src/lib.rs`, add an arm **before** the forward path. This is temporary scaffolding that Task 7 replaces with the real handler — its whole purpose is that Tasks 4–6 have a loud, greppable marker instead of a silent SIGTRAP:

```rust
retrace_arch::SYS_BSDTHREAD_CREATE => {
    // M14 Task 3 scaffolding, REPLACED by Task 7's real handler. Never forward this: the host
    // would create a real thread in retrace's own process at a guest address.
    panic!(
        "M14: guest called bsdthread_create(func={:#x}, arg={:#x}, stack={:#x}, pthread={:#x}, \
         flags={:#x}) — threads are not implemented yet. This is the M14 wall, reached honestly.",
        args[0], args[1], args[2], args[3], args[4]
    );
}
```

- [ ] **Step 6: Verify the death is now loud**

```bash
cargo run -q -p retrace -- record-dyn ./spikes/threadjoin -o /tmp/tj.bin 2>&1 | tail -3
```
Expected: the panic message above, naming all five arguments — **not** a silent exit 133.

- [ ] **Step 7: Targeted tests and clippy**

```bash
cargo test -p retrace-arch -- --test-threads=1
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 8: Commit**

```bash
git add crates/retrace-arch/src/lib.rs crates/retrace-box/src/lib.rs
git commit -m "M14 t3: the thread syscalls have numbers, and the wall stops being silent"
```

---

### Task 4: The thread table and the scheduler, as a pure module

The scheduler is the one piece of M14 that needs no VM. Isolating it in its own module is what makes it exhaustively testable in milliseconds instead of minutes.

**Files:**
- Create: `crates/retrace-box/src/thread.rs`
- Create: `crates/retrace-box/tests/threads.rs`
- Modify: `crates/retrace-box/src/lib.rs` (add `mod thread;` / `pub use`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct ThreadCtx { pub regs: Regs, pub fp: [u128; 32], pub fpcr: u64, pub fpsr: u64, pub tpidrro_el0: u64, pub elr: u64, pub spsr: u64 }`
  - `pub enum ThreadState { Runnable, Blocked(BlockReason), Exited(u64) }`
  - `pub enum BlockReason { Join { target: usize }, Wait { addr: u64 } }`
  - `pub struct Thread { pub ctx: ThreadCtx, pub state: ThreadState, pub stack: (u64, u64) }`
  - `pub struct ThreadTable { threads: Vec<Thread>, current: usize }` with
    `fn new(main: ThreadCtx) -> Self`, `fn current(&self) -> usize`,
    `fn spawn(&mut self, ctx: ThreadCtx, stack: (u64, u64)) -> usize`,
    `fn block(&mut self, reason: BlockReason)`, `fn exit_current(&mut self, code: u64)`,
    `fn unblock_joiners_of(&mut self, tid: usize)`,
    `fn pick_next(&self) -> Option<usize>`, `fn live(&self) -> usize`.
- Tasks 5–11 consume all of it.

- [ ] **Step 1: Write the failing tests**

Create `crates/retrace-box/tests/threads.rs`:

```rust
// M14: the thread table and the cooperative scheduler. These are PURE — no VM, no vCPU, no HVF —
// which is the entire reason `thread.rs` is a separate module. They run in milliseconds.
use retrace_box::thread::{BlockReason, ThreadCtx, ThreadState, ThreadTable};

fn ctx(pc: u64) -> ThreadCtx {
    let mut c = ThreadCtx::zeroed();
    c.elr = pc;
    c
}

// NOTE: do NOT add a `tb()` helper in this task. None of Task 4's six tests are VM-backed — they
// exercise `ThreadTable`/`ThreadCtx` only — so an unused helper would be dead code and fail clippy
// under `-D warnings`. **Task 5 adds `tb()` alongside the first test that needs it.**

#[test]
fn a_fresh_table_has_one_runnable_main_thread() {
    let t = ThreadTable::new(ctx(0x1000));
    assert_eq!(t.current(), 0);
    assert_eq!(t.live(), 1);
    // A single-threaded guest must take exactly today's path: thread 0, runnable, always picked.
    assert_eq!(t.pick_next(), Some(0));
}

#[test]
fn spawn_appends_a_runnable_thread_and_does_not_switch() {
    let mut t = ThreadTable::new(ctx(0x1000));
    let child = t.spawn(ctx(0x2000), (0x30200000, 0x8000));
    assert_eq!(child, 1);
    // The real kernel does not switch on create, and neither do we.
    assert_eq!(t.current(), 0, "bsdthread_create must NOT switch away from the caller");
    assert_eq!(t.live(), 2);
}

#[test]
fn blocking_the_only_runnable_thread_leaves_the_child_pickable() {
    let mut t = ThreadTable::new(ctx(0x1000));
    t.spawn(ctx(0x2000), (0x30200000, 0x8000));
    t.block(BlockReason::Join { target: 1 });
    assert_eq!(t.pick_next(), Some(1), "main blocked in join, so the child runs");
}

#[test]
fn pick_next_is_lowest_indexed_runnable_which_is_what_makes_replay_deterministic() {
    let mut t = ThreadTable::new(ctx(0x1000));
    t.spawn(ctx(0x2000), (0x30200000, 0x8000));
    t.spawn(ctx(0x3000), (0x30300000, 0x8000));
    t.block(BlockReason::Join { target: 2 });
    // Both 1 and 2 are runnable; the LOWEST index is forced, so record and replay agree.
    assert_eq!(t.pick_next(), Some(1));
}

#[test]
fn an_exited_thread_is_never_picked_and_unblocks_its_joiner() {
    let mut t = ThreadTable::new(ctx(0x1000));
    t.spawn(ctx(0x2000), (0x30200000, 0x8000));
    t.block(BlockReason::Join { target: 1 });
    // The child runs and exits.
    t.switch_to(1);
    t.exit_current(42);
    t.unblock_joiners_of(1);
    assert_eq!(t.pick_next(), Some(0), "main's join is satisfied, main runs again");
    assert!(matches!(t.state_of(1), ThreadState::Exited(42)));
}

#[test]
fn every_thread_blocked_is_a_deadlock_and_pick_next_says_so() {
    let mut t = ThreadTable::new(ctx(0x1000));
    t.spawn(ctx(0x2000), (0x30200000, 0x8000));
    t.block(BlockReason::Join { target: 1 });
    t.switch_to(1);
    t.block(BlockReason::Join { target: 0 });
    // Nobody can run. pick_next reports it rather than hanging or picking a blocked thread.
    assert_eq!(t.pick_next(), None, "a deadlock must be visible, not a hang");
}
```

- [ ] **Step 2: Run them and watch them fail**

```bash
cargo test -p retrace-box --test threads -- --test-threads=1
```
Expected: FAIL to compile — `unresolved import retrace_box::thread`.

- [ ] **Step 3: Write `crates/retrace-box/src/thread.rs`**

```rust
//! M14: the guest's thread table and its cooperative scheduler.
//!
//! **This module is deliberately VM-free.** Nothing here touches HVF, the vCPU, or guest memory —
//! it is bookkeeping plus one pick function, which is why it can be unit-tested exhaustively in
//! milliseconds while the rest of M14 needs a VM and `--test-threads=1`.
//!
//! **Why the scheduler is a pure function.** `pick_next` returns the lowest-indexed runnable
//! thread. Given the guest's own syscall sequence the choice is forced, so record and replay
//! schedule identically with nothing recorded and no trace-format change. That is symmetry rule 2:
//! deterministic behaviour belongs below the trace, where it fires identically on both sides.
// `retrace-box` imports Regs PRIVATELY at lib.rs:4 (`use retrace_trace::{Regs, Region};`), so
// `crate::Regs` does NOT resolve from a submodule. Import it from its own crate.
use retrace_trace::Regs;

/// One thread's register context.
///
/// This is `BoxState`'s register subset — which M4's checkpoint tests already prove is sufficient
/// to restore a vCPU mid-run — **plus `tpidrro_el0`, which `BoxState` does not carry.** Its absence
/// there is correct: the thread pointer is a constant (`TSD_IPA`) until threads exist. Threads are
/// exactly what makes it vary, so the one register the existing context set omits is the one M14
/// makes per-thread. Note `tpidr_el0` is NOT here: macOS 26 reads the CPU number from its low bits
/// and it must stay 0 for every thread (M2-cpuid).
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadCtx {
    pub regs: Regs,
    pub fp: [u128; 32],
    pub fpcr: u64,
    pub fpsr: u64,
    pub tpidrro_el0: u64,
    pub elr: u64,
    pub spsr: u64,
}

impl ThreadCtx {
    pub fn zeroed() -> Self {
        Self {
            // `Regs` derives Debug/Clone/PartialEq/Eq/Serialize/Deserialize but NOT Default —
            // construct it field-by-field rather than adding a derive to the trace crate.
            regs: Regs { x: [0u64; 31], pc: 0, sp_el0: 0, cpsr: 0 },
            fp: [0u128; 32],
            fpcr: 0,
            fpsr: 0,
            tpidrro_el0: 0,
            elr: 0,
            spsr: 0,
        }
    }
}

/// Why a thread cannot currently run.
///
/// The variants are deliberately concrete rather than an opaque token: `unblock_joiners_of` has to
/// decide who a thread exit wakes, and that is only answerable if the reason names its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockReason {
    /// Waiting for thread `target` to exit (`pthread_join`).
    Join { target: usize },
    /// Waiting on a futex-shaped address (the primitive Task 1 measured).
    Wait { addr: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    Runnable,
    Blocked(BlockReason),
    /// Exited with this return value. Kept in the table rather than removed: `join` may arrive
    /// AFTER the exit, and a removed thread cannot answer it. Indices must also stay stable.
    Exited(u64),
}

#[derive(Clone, Debug)]
pub struct Thread {
    pub ctx: ThreadCtx,
    pub state: ThreadState,
    /// `(base, len)` of the guest-allocated stack. The guest maps its own thread stacks, so M14
    /// never places one; this is recorded for teardown and for diagnostics only.
    pub stack: (u64, u64),
}

#[derive(Clone, Debug)]
pub struct ThreadTable {
    threads: Vec<Thread>,
    current: usize,
}

impl ThreadTable {
    /// A guest starts with exactly one thread, so a single-threaded guest has a one-entry table and
    /// takes precisely the pre-M14 path. That is the compatibility argument for every M0–M13 gate.
    pub fn new(main: ThreadCtx) -> Self {
        Self {
            threads: vec![Thread { ctx: main, state: ThreadState::Runnable, stack: (0, 0) }],
            current: 0,
        }
    }

    pub fn current(&self) -> usize { self.current }
    pub fn len(&self) -> usize { self.threads.len() }
    pub fn is_empty(&self) -> bool { self.threads.is_empty() }
    pub fn state_of(&self, tid: usize) -> ThreadState { self.threads[tid].state }
    pub fn ctx_of(&self, tid: usize) -> &ThreadCtx { &self.threads[tid].ctx }
    pub fn ctx_mut(&mut self, tid: usize) -> &mut ThreadCtx { &mut self.threads[tid].ctx }

    /// Threads that have not exited.
    pub fn live(&self) -> usize {
        self.threads.iter().filter(|t| !matches!(t.state, ThreadState::Exited(_))).count()
    }

    /// Append a runnable thread. Does **not** switch: the real kernel returns to the caller after
    /// `bsdthread_create`, and a switch here would reorder the guest's own output.
    pub fn spawn(&mut self, ctx: ThreadCtx, stack: (u64, u64)) -> usize {
        self.threads.push(Thread { ctx, state: ThreadState::Runnable, stack });
        self.threads.len() - 1
    }

    pub fn block(&mut self, reason: BlockReason) {
        self.threads[self.current].state = ThreadState::Blocked(reason);
    }

    pub fn switch_to(&mut self, tid: usize) {
        assert!(tid < self.threads.len(), "switch to nonexistent thread {tid}");
        self.current = tid;
    }

    pub fn exit_current(&mut self, code: u64) {
        self.threads[self.current].state = ThreadState::Exited(code);
    }

    /// Wake everyone joined on `tid`. Called on thread exit.
    pub fn unblock_joiners_of(&mut self, tid: usize) {
        for t in &mut self.threads {
            if let ThreadState::Blocked(BlockReason::Join { target }) = t.state {
                if target == tid {
                    t.state = ThreadState::Runnable;
                }
            }
        }
    }

    /// The scheduler. Lowest-indexed runnable thread, or `None` if nobody can run.
    ///
    /// `None` is a deadlock, and the caller must fail loud rather than spin — see `Box_`'s
    /// deadlock assert. Returning an `Option` instead of panicking here keeps this module pure and
    /// lets the table be unit-tested for the deadlock case without catching a panic.
    pub fn pick_next(&self) -> Option<usize> {
        self.threads.iter().position(|t| matches!(t.state, ThreadState::Runnable))
    }
}
```

- [ ] **Step 4: Wire the module in**

In `crates/retrace-box/src/lib.rs`, next to the other module declarations:

```rust
pub mod thread;
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p retrace-box --test threads -- --test-threads=1
```
Expected: PASS, 6 tests. If `Regs` does not derive `Default`/`PartialEq`/`Clone`, add the derives it needs — do **not** hand-roll a second register struct.

- [ ] **Step 6: Targeted tests and clippy**

```bash
cargo test -p retrace-box --test threads --test checkpoint_seek -- --test-threads=1
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-box/src/thread.rs crates/retrace-box/tests/threads.rs crates/retrace-box/src/lib.rs
git commit -m "M14 t4: a thread table and a scheduler that is a pure function"
```

---

### Task 5: The context switch

**Files:**
- Modify: `crates/retrace-box/src/lib.rs`
- Test: `crates/retrace-box/tests/threads.rs`

**Interfaces:**
- Consumes: `ThreadCtx`, `ThreadTable` (Task 4).
- Produces: `Box_::save_ctx(&self) -> ThreadCtx`, `Box_::load_ctx(&mut self, ctx: &ThreadCtx)`, `Box_::switch_to_thread(&mut self, tid: usize)`. Tasks 7–11 consume `switch_to_thread`.

- [ ] **Step 1: Write the failing test**

Append to `crates/retrace-box/tests/threads.rs`:

This task adds the **first VM-backed test in the file**, so it must also add the `tb()` helper (Task 4 deliberately left it out — an unused helper is dead code under `-D warnings`). Add both:

```rust
/// A `Box_` for the VM-backed tests in this file.
///
/// There is no `Box_::for_test()`; the constructor is `Box_::load(&loaded)`, and every existing
/// retrace-box test builds one this way — see `tests/checkpoint.rs:11-12`, whose exact two-line
/// form this copies. `parse_macho` takes BYTES and returns `Loaded` directly: it is not fallible,
/// and the `SPINLOOP` constant is a PATH, so read it first. M14 needs no special guest for these
/// register-level tests, only a live vCPU. **`--test-threads=1` is mandatory: one HVF VM per
/// process.**
fn tb() -> retrace_box::Box_ {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::SPINLOOP).unwrap());
    retrace_box::Box_::load(&loaded)
}

// This one needs a VM.
#[test]
fn a_switch_round_trips_every_register_in_the_context() {
    let mut b = tb();
    // Distinctive values in every field the context claims to carry, so a dropped field shows up
    // as a mismatch rather than a coincidental zero-equals-zero pass.
    b.set_x(3, 0xdead_beef_0000_0003);
    b.set_x(29, 0xdead_beef_0000_001d);
    b.set_elr(0x1234_5000);
    b.set_spsr(0x3c4);
    b.set_tpidrro_el0(0x0003_8000);

    let saved = b.save_ctx();

    // Clobber the hardware, then restore.
    b.set_x(3, 0);
    b.set_x(29, 0);
    b.set_elr(0);
    b.set_spsr(0);
    b.set_tpidrro_el0(0);
    b.load_ctx(&saved);

    // Assert against the HARDWARE, not against `saved`. M13's Task 8 defect was a test that checked
    // only the software mirror and passed while the stage-1 leaf disagreed.
    assert_eq!(b.get_x(3), 0xdead_beef_0000_0003);
    assert_eq!(b.get_x(29), 0xdead_beef_0000_001d);
    assert_eq!(b.get_elr(), 0x1234_5000);
    assert_eq!(b.get_spsr(), 0x3c4);
    assert_eq!(b.get_tpidrro_el0(), 0x0003_8000, "tpidrro_el0 is THE per-thread register");
    assert_eq!(b.get_tpidr_el0(), 0, "tpidr_el0 must stay 0 — macOS reads the CPU number from it");
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p retrace-box --test threads a_switch_round_trips -- --test-threads=1
```
Expected: FAIL — `no method named save_ctx`.

- [ ] **Step 3: Implement save/load/switch**

In `crates/retrace-box/src/lib.rs`:

```rust
impl Box_ {
    /// Capture the running thread's context off the vCPU.
    ///
    /// Same save/restore discipline `flush_guest_tlb` (M9) and the PAC signing oracle already use;
    /// a context switch is that operation with the restore aimed at a DIFFERENT thread.
    pub fn save_ctx(&self) -> thread::ThreadCtx {
        let mut fp = [0u128; 32];
        for (i, f) in fp.iter_mut().enumerate() { *f = self.vcpu.get_simd(simd::q(i as u32)).unwrap(); }
        thread::ThreadCtx {
            regs: self.regs_snapshot(),           // lib.rs:2873 — NOT read_regs
            fp,
            fpcr: self.vcpu.get_reg(reg::FPCR).unwrap(),   // Reg, not SysReg
            fpsr: self.vcpu.get_reg(reg::FPSR).unwrap(),   // Reg, not SysReg
            tpidrro_el0: self.vcpu.get_sys(sysreg::TPIDRRO_EL0).unwrap(),
            elr: self.vcpu.get_sys(sysreg::ELR_EL1).unwrap(),
            spsr: self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap(),
        }
    }

    /// The inverse of `regs_snapshot`. The only new low-level helper M14 needs.
    fn write_regs(&mut self, r: &Regs) {
        for (i, xi) in r.x.iter().enumerate() { self.vcpu.set_reg(reg::x(i as u32), *xi).unwrap(); }
        self.vcpu.set_reg(reg::PC, r.pc).unwrap();
        self.vcpu.set_sys(sysreg::SP_EL0, r.sp_el0).unwrap();
        self.vcpu.set_reg(reg::CPSR, r.cpsr).unwrap();
    }

    /// Install a thread's context onto the vCPU.
    ///
    /// `TPIDR_EL0` is deliberately NOT touched: macOS 26 reads the guest's CPU number from its low
    /// bits and the cluster from the rest, so it must stay 0 for every thread. Writing a
    /// per-thread value there is how M2-cpuid's OOB seg-group index bug comes back.
    pub fn load_ctx(&mut self, ctx: &thread::ThreadCtx) {
        self.write_regs(&ctx.regs);
        for (i, f) in ctx.fp.iter().enumerate() { self.vcpu.set_simd(simd::q(i as u32), *f).unwrap(); }
        self.vcpu.set_reg(reg::FPCR, ctx.fpcr).unwrap();   // Reg, not SysReg
        self.vcpu.set_reg(reg::FPSR, ctx.fpsr).unwrap();   // Reg, not SysReg
        self.vcpu.set_sys(sysreg::TPIDRRO_EL0, ctx.tpidrro_el0).unwrap();
        self.vcpu.set_sys(sysreg::ELR_EL1, ctx.elr).unwrap();
        self.vcpu.set_sys(sysreg::SPSR_EL1, ctx.spsr).unwrap();
    }

    /// Switch the vCPU from the running thread to `tid`.
    pub fn switch_to_thread(&mut self, tid: usize) {
        let cur = self.threads.current();
        if cur == tid {
            return;
        }
        let saved = self.save_ctx();
        *self.threads.ctx_mut(cur) = saved;
        self.threads.switch_to(tid);
        let next = self.threads.ctx_of(tid).clone();
        self.load_ctx(&next);
    }
}
```

Add the field to `Box_`, **after** `vcpu` and `vm` so the drop order stays intact:

```rust
    /// M14: the guest's threads. A single-threaded guest has one entry and takes the pre-M14 path.
    threads: thread::ThreadTable,
```

and initialise it in each constructor with `thread::ThreadTable::new(thread::ThreadCtx::zeroed())`, then overwrite thread 0's context from the vCPU at the end of `load_dynamic` so it reflects real startup state.

- [ ] **Step 4: Run the test to verify it passes**

```bash
cargo test -p retrace-box --test threads -- --test-threads=1
```
Expected: PASS, 7 tests.

- [ ] **Step 5: Prove the existing gates did not move**

Adding a field to `Box_` touches every constructor. Run the crate:

```bash
cargo test -p retrace-box -- --test-threads=1
```
Expected: **123 + 7 = 130 passed / 0 failed / 0 ignored.**

- [ ] **Step 6: Clippy, then commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/threads.rs
git commit -m "M14 t5: a context switch, built on the save/restore discipline M9 already proved"
```

---

### Task 6: `BoxState` carries the thread table (R4)

Risk R4 is that a checkpoint silently loses every non-current thread — it still restores, still runs, and breaks quietly. That is the M13 Task 8 signature, so the gate must assert the restored **table**, not merely that the seek succeeded.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (`BoxState`, `capture`, `from_checkpoint`)
- Test: `crates/retrace-box/tests/threads.rs`

**Interfaces:**
- Consumes: `ThreadTable` (Task 4), `save_ctx`/`load_ctx` (Task 5).
- Produces: `BoxState.threads: thread::ThreadTable`. Task 11's replay path depends on it.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_checkpoint_carries_every_thread_not_just_the_running_one() {
    let mut b = tb();   // see `fn tb()` at the top of this file
    let child = b.threads_mut().spawn(
        { let mut c = retrace_box::thread::ThreadCtx::zeroed(); c.elr = 0x4242_0000; c },
        (0x3020_0000, 0x8000),
    );
    b.threads_mut().block(retrace_box::thread::BlockReason::Join { target: child });

    let st = b.checkpoint();   // lib.rs:2964 — NOT capture()

    // The failure this guards is QUIET: a checkpoint that drops non-current threads still restores
    // and still runs. Assert the table, not that the restore returned Ok.
    assert_eq!(st.threads.len(), 2, "the checkpoint must carry the child thread");
    assert_eq!(st.threads.ctx_of(child).elr, 0x4242_0000, "…and its register context");
    assert!(
        matches!(st.threads.state_of(0), retrace_box::thread::ThreadState::Blocked(_)),
        "…and main's blocked state, or the restored run picks the wrong thread"
    );
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p retrace-box --test threads a_checkpoint_carries -- --test-threads=1
```
Expected: FAIL — `no field threads on type BoxState`.

- [ ] **Step 3: Add the field**

In `BoxState` (`crates/retrace-box/src/lib.rs:569`), beside `noaccess`:

```rust
    // M14: carried for the same reason as `reservations` and `noaccess` — a mid-run capture cannot
    // re-derive it. Note this SUPERSEDES the struct's flat register fields as the authority for
    // non-current threads: `regs`/`elr`/`spsr` still describe the RUNNING thread (so every M0–M13
    // consumer is unchanged), while `threads` carries all of them including that one.
    pub threads: thread::ThreadTable,
```

Populate it in `capture` (after `save_ctx` has folded the live context back into the table) and restore it in `from_checkpoint` before the vCPU registers are loaded.

- [ ] **Step 4: Run the test to verify it passes**

```bash
cargo test -p retrace-box --test threads -- --test-threads=1
```
Expected: PASS, 8 tests.

- [ ] **Step 5: Prove M4's seeks still work**

```bash
cargo test -p retrace-box -- --test-threads=1
cargo test -p retrace --test checkpoint_seek --test seek --test reverse_debug_e2e -- --test-threads=1
```
Expected: `retrace-box` **131 passed**; the three seek targets unchanged from baseline.

- [ ] **Step 6: Clippy, then commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/threads.rs
git commit -m "M14 t6: a checkpoint that forgets a thread breaks quietly, so it carries them all"
```

---

### Task 7: Emulated `bsdthread_create`

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` — replace Task 3's panic scaffold in `record_box`'s `match stop`, **and add the matching replay arm in `ReplaySession::advance`**. See "Where syscall dispatch actually lives" above; copy M13's `mach_vm_protect` pair at `:352` / `:1148`.
- Modify: `crates/retrace-box/src/lib.rs` — the `guest_bsdthread_create` method itself and the `thread_start_pc` field.
- Test: `crates/retrace-box/tests/threads.rs`

**Symmetry rule 1 applies here and it is not optional.** Record and replay must both call `b.guest_bsdthread_create(args)` with identical arguments, so both build an identical thread table. Omit the replay arm and replay runs a one-thread table against a two-thread recording — which surfaces as a divergence at the child's first syscall, not as a clean error. Task 9's plan text says to measure this by deletion; do that here too if it is cheap.

**Interfaces:**
- Consumes: Task 2's measured ABI and `threadstart` address; `ThreadTable::spawn`; `ThreadCtx`.
- Produces: `Box_::guest_bsdthread_create(&mut self, args: [u64; 8]) -> u64` and
  `Box_::thread_start_pc(&self) -> Option<u64>` (the address captured from `bsdthread_register`).

- [ ] **Step 1: Capture `threadstart` when the guest registers it**

Add a `bsdthread_register` arm that records x0 and then lets the call proceed as it does today (it already works — do not change its forwarding):

```rust
retrace_arch::SYS_BSDTHREAD_REGISTER => {
    // x0 is the address the kernel enters a NEW thread at. Already issued by every dynamic guest
    // since M7 (M14 Task 2), which is why M14 never has to synthesize a trampoline.
    self.thread_start_pc = Some(args[0]);
    // fall through to the existing behaviour
}
```

- [ ] **Step 2: Write the failing test**

```rust
#[test]
fn bsdthread_create_builds_a_thread_at_the_registered_trampoline() {
    let mut b = tb();   // see `fn tb()` at the top of this file
    b.set_thread_start_pc_for_test(0x1804b_2000);

    // The ABI measured in Task 2: (func, arg, stack, pthread, flags).
    let rc = b.guest_bsdthread_create([0x1_0002_4e00, 0x62180, 0x3020_7000, 0x3020_7000, 0x90008ff, 0, 0, 0]);

    assert_eq!(rc, 0, "create must succeed");
    assert_eq!(b.threads().len(), 2);
    assert_eq!(b.threads().current(), 0, "create does not switch — the caller keeps running");
    let c = b.threads().ctx_of(1);
    assert_eq!(c.elr, 0x1804b_2000, "the child enters at the REGISTERED trampoline, not at func");
    // MEASURED contract (Task 2, re-disassembled in review): __pthread_start reads x0 and w5 only.
    // func/arg arrive through the pthread struct at +0x90/+0x98, which the GUEST populated before
    // trapping — so they must NOT appear in registers here.
    assert_eq!(c.regs.x[0], 0x3020_7000, "x0 is the pthread-struct pointer");
    assert_eq!(c.regs.x[5], 0x90008ff, "w5 carries the flags __pthread_start tbnz/tbz-tests");
    assert_eq!(c.regs.x[1], 0, "x1 is NOT part of the contract — seeding it would be cargo cult");
    assert_eq!(c.tpidrro_el0, 0x3020_7000, "each thread gets its own thread pointer…");
    assert_ne!(c.regs.sp_el0, 0, "the child runs on the guest-allocated stack");
}

#[test]
fn bsdthread_create_without_a_registered_trampoline_fails_loud() {
    let mut b = tb();   // see `fn tb()` at the top of this file
    // No bsdthread_register seen. Guessing a trampoline address would be a silent wrong answer.
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.guest_bsdthread_create([1, 2, 0x3020_7000, 4, 5, 0, 0, 0])
    }));
    assert!(r.is_err(), "must assert rather than invent an entry point");
}
```

**This test was REWRITTEN after Task 2 measured the real contract, which is not what this plan originally guessed.** The plan had the classic five-register `_pthread_start(self, kport, fun, funarg, stacksize, pflags)` shape; the disassembly says `__pthread_start` reads only `x0` and `w5`, and takes `func`/`arg` from the pthread struct at `+0x90`/`+0x98`. Both Task 2 and its reviewer disassembled this independently and agree. **Use the measured contract above. Do not restore the five-register form.** M13's Task 10 shipped a planned guest that wrote to the wrong register and would have asserted vacuously — this is that same failure, caught before it shipped.

The `stack` (x2) and `pthread` (x3) arguments are numerically **identical** in every capture taken so far (both this plan's Rust probe and Task 2's C spike). That is a real property of Apple's combined stack+struct allocation, not a misread — but do not write code that relies on them being the same value.

- [ ] **Step 3: Run and watch it fail**

```bash
cargo test -p retrace-box --test threads bsdthread_create -- --test-threads=1
```
Expected: FAIL — `no method named guest_bsdthread_create`.

- [ ] **Step 4: Implement the handler**

```rust
impl Box_ {
    /// `bsdthread_create(func, arg, stack, pthread, flags)`, emulated.
    ///
    /// **Never forwarded.** The host would create a real thread inside retrace's own process
    /// starting at a guest address — the same class of hazard as M13's `mach_vm_protect`, but worse,
    /// because it would execute.
    ///
    /// The stack is the guest's own: libpthread `mach_vm_map`s it and `mach_vm_protect`s its guard
    /// page PROT_NONE (M13's first real caller) two traps before this one. M14 places no IPAs.
    pub fn guest_bsdthread_create(&mut self, args: [u64; 8]) -> u64 {
        let (func, arg, stack, pthread, flags) = (args[0], args[1], args[2], args[3], args[4]);
        let start = self.thread_start_pc.expect(
            "M14: bsdthread_create before bsdthread_register — refusing to invent a thread entry \
             point. Every dynamic guest registers one at startup (measured, M14 Task 2), so this \
             means the guest took a path no measurement covers.",
        );

        let mut ctx = thread::ThreadCtx::zeroed();
        ctx.elr = start;
        ctx.spsr = self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap();
        ctx.regs.sp_el0 = stack;

        // THE THREAD-START CONTRACT, MEASURED (Task 2) AND INDEPENDENTLY RE-DISASSEMBLED IN REVIEW.
        // NOT the classic _pthread_start(self, kport, fun, funarg, stacksize, pflags) shape — this
        // plan originally guessed that and was WRONG. `__pthread_start` reads only:
        //   x0 — the pthread-struct pointer
        //   w5 — flags (tested via tbnz/tbz at its entry, 0x6be0/0x6be4)
        // and it loads func/arg FROM THE STRUCT, not from registers:
        //   `ldp x8, x0, [x19, #0x90]` at 0x6c50 → x8 = [pthread+0x90] (func),
        //                                          x0 = [pthread+0x98] (arg), then `blraaz x8`.
        // x1-x4 are never read on the entry-to-dispatch path. (One callee,
        // `__pthread_markcancel_if_canceled`, does `mov x0, x1` at 0x3788, but only on an
        // already-canceled branch this path does not take — internal bookkeeping, not dispatch.)
        //
        // So the box seeds x0 and w5 and nothing else. It does NOT populate the struct: the guest's
        // own `_pthread_create` stored func/arg at +0x90/+0x98 BEFORE issuing this trap, which is
        // why the fields are there to be read. Writing them here would be retrace inventing guest
        // state it does not own.
        ctx.regs.x[0] = pthread;
        ctx.regs.x[5] = flags;
        // Each thread gets its own thread pointer. TPIDR_EL0 stays 0 for ALL threads (M2-cpuid).
        ctx.tpidrro_el0 = pthread;

        // `func` and `arg` are deliberately unused: they reach the child through the struct, not
        // through us. Named in the destructuring above so the ABI stays documented at the call site.
        let _ = (func, arg);

        self.threads.spawn(ctx, (stack, 0));
        0
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p retrace-box --test threads -- --test-threads=1
```
Expected: PASS, 10 tests.

- [ ] **Step 6: FULL GATE — a live call site now exists**

**The controller runs this, not the implementer.**

```bash
just gate 2>&1 | tail -20
```
Expected: **321 passed / 0 failed / 1 ignored** (311 baseline + 10 new). Investigate any mismatch before continuing.

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/threads.rs
git commit -m "M14 t7: bsdthread_create builds a thread instead of dying"
```

---

### Task 8: Thread exit, and waking the joiner

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` — the `bsdthread_terminate` and blocking-primitive arms, **in both `record_box` and `ReplaySession::advance`**.
- Modify: `crates/retrace-box/src/lib.rs` — the `guest_bsdthread_terminate` method.
- Test: `crates/retrace-box/tests/threads.rs`

**Interfaces:**
- Consumes: Task 1's measured join primitive — **`__ulock_wait`, syscall 515** (measured, and independently cross-checked against the SDK's `sys/syscall.h`); `ThreadTable::exit_current`/`unblock_joiners_of`.
- Produces: `Box_::guest_bsdthread_terminate(&mut self, args: [u64; 8]) -> u64`.

**A design decision this task must make explicitly, not by accident.** The blocking arm can live either above the trace (in `retrace-core`'s dispatch, needing a replay mirror) or below it (inside `Box_::run()`, firing identically on both sides for free). The spec argues for below — symmetry rule 2 — because that is what makes the schedule a pure function with no recorded state. But `__ulock_wait` is a real syscall with a real return value, so whichever side handles it must decide what the guest sees when it wakes. **State the choice and its reasoning in the report**; do not leave it implied by where the code happened to land.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_terminating_thread_exits_and_wakes_whoever_joined_it() {
    let mut b = tb();   // see `fn tb()` at the top of this file
    b.set_thread_start_pc_for_test(0x1804b_2000);
    b.guest_bsdthread_create([0x1_0002_4e00, 0, 0x3020_7000, 0x3020_7000, 0, 0, 0, 0]);

    // Main joins the child, so main blocks and the child is the only runnable thread.
    b.threads_mut().block(retrace_box::thread::BlockReason::Join { target: 1 });
    b.switch_to_thread(1);

    b.guest_bsdthread_terminate([0x3020_7000, 0x8000, 0, 0, 0, 0, 0, 0]);

    assert!(matches!(b.threads().state_of(1), retrace_box::thread::ThreadState::Exited(_)));
    assert_eq!(b.threads().pick_next(), Some(0), "main's join is satisfied");
}
```

- [ ] **Step 2: Run and watch it fail**

```bash
cargo test -p retrace-box --test threads a_terminating_thread -- --test-threads=1
```
Expected: FAIL — `no method named guest_bsdthread_terminate`.

- [ ] **Step 3: Implement it**

```rust
impl Box_ {
    /// `bsdthread_terminate(stackaddr, freesize, port, sem)` — a guest thread's exit.
    ///
    /// **Does not return.** The calling thread is gone; the trap loop must switch rather than
    /// resume it, which is why this returns no value into x0.
    pub fn guest_bsdthread_terminate(&mut self, _args: [u64; 8]) -> u64 {
        let me = self.threads.current();
        // Open question 1 in the spec: whether the return value rides here or in the pthread
        // struct. Task 1/2 measured it; if it is in the struct, libpthread reads it back itself
        // and the box records 0 here rather than pretending to know.
        self.threads.exit_current(0);
        self.threads.unblock_joiners_of(me);
        0
    }
}
```

- [ ] **Step 4: Wire the blocking primitive Task 1 measured**

Task 1 SETTLED this: `__ulock_wait`, syscall **515**, pinned by disassembly (`mov x16, #0x203; svc #0x80` in `___ulock_wait`) and cross-checked against the SDK header. The other two candidates are ruled out — `psynch_cvwait` and `semaphore_wait` do not appear anywhere in `__pthread_join`. (`___semwait_signal_nocancel` DOES appear, but downstream of the `__ulock_wait` retry loop, gated on a semaphore slot the join path does not populate — do not mistake it for an alternate wait.) Add `SYS_ULOCK_WAIT: u64 = 515` to `retrace-arch` alongside Task 3's constants, then:

```rust
retrace_arch::SYS_ULOCK_WAIT => {
    // args[1] is the address being waited on. If the value there no longer matches what the guest
    // expects, the wait is ALREADY SATISFIED and the thread must stay runnable — blocking it would
    // deadlock a race the guest already won. That check is the difference between a scheduler and
    // a hang, so write it explicitly rather than assuming the guest only waits when it must.
    self.threads.block(thread::BlockReason::Wait { addr: args[1] });
    return Stop::Syscall { num, args };
}
```

**Two things to settle while writing this, and to state in the report:**
1. **What the woken thread sees in x0.** `__ulock_wait` returns a value the guest inspects. Decide it, justify it, and make record and replay agree — if they disagree the divergence oracle catches it, which is the good outcome, but only after a confusing failure.
2. **Whether the arm is above or below the trace** (see this task's Files note). Below is the spec's preference; if you put it above, it needs a replay mirror like every other `record_box` arm.

- [ ] **Step 5: Run the tests**

```bash
cargo test -p retrace-box --test threads -- --test-threads=1
```
Expected: PASS, 11 tests.

- [ ] **Step 6: FULL GATE (controller)**

```bash
just gate 2>&1 | tail -20
```
Expected: **322 passed / 0 failed / 1 ignored.**

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/threads.rs
git commit -m "M14 t8: a thread can exit, and its joiner wakes up"
```

---

### Task 9: The scheduler runs — switching inside `Box_::run()`

This is the task that makes the previous five do something. It belongs **below the trace** (symmetry rule 2), so record and replay both get it for free and neither dispatch loop changes.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (`run()`)
- Test: `crates/retrace-box/tests/threads.rs`

**Interfaces:**
- Consumes: everything from Tasks 4–8.
- Produces: no new public API — `run()` transparently multiplexes threads.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn run_switches_to_the_child_when_main_blocks() {
    let mut b = tb();   // see `fn tb()` at the top of this file
    b.set_thread_start_pc_for_test(0x1804b_2000);
    b.guest_bsdthread_create([0x1_0002_4e00, 0, 0x3020_7000, 0x3020_7000, 0, 0, 0, 0]);
    b.threads_mut().block(retrace_box::thread::BlockReason::Join { target: 1 });

    b.schedule_after_block();

    assert_eq!(b.threads().current(), 1, "the box must switch to the only runnable thread");
    assert_eq!(b.get_elr(), 0x1804b_2000, "…and the vCPU must actually be running its context");
}

#[test]
fn a_deadlock_fails_loud_instead_of_hanging() {
    let mut b = tb();   // see `fn tb()` at the top of this file
    b.set_thread_start_pc_for_test(0x1804b_2000);
    b.guest_bsdthread_create([0x1_0002_4e00, 0, 0x3020_7000, 0x3020_7000, 0, 0, 0, 0]);
    b.threads_mut().block(retrace_box::thread::BlockReason::Join { target: 1 });
    b.switch_to_thread(1);
    b.threads_mut().block(retrace_box::thread::BlockReason::Join { target: 0 });

    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.schedule_after_block()));
    assert!(r.is_err(), "every thread blocked must panic, never spin");
}
```

- [ ] **Step 2: Run and watch them fail**

```bash
cargo test -p retrace-box --test threads schedule -- --test-threads=1
```
Expected: FAIL — `no method named schedule_after_block`.

- [ ] **Step 3: Implement the scheduling point**

```rust
impl Box_ {
    /// Pick and switch after the running thread blocked or exited.
    ///
    /// The pick is `ThreadTable::pick_next` — lowest-indexed runnable — which is a pure function of
    /// the guest's own syscall sequence. That is what lets record and replay schedule identically
    /// with NOTHING recorded and no trace-format change.
    pub fn schedule_after_block(&mut self) {
        match self.threads.pick_next() {
            Some(tid) => self.switch_to_thread(tid),
            None => panic!(
                "M14: DEADLOCK — no runnable thread. {} live of {} total. States: {:?}",
                self.threads.live(),
                self.threads.len(),
                (0..self.threads.len()).map(|i| self.threads.state_of(i)).collect::<Vec<_>>()
            ),
        }
    }
}
```

Then call it from `run()` at the **stop boundary only** — after a stop is fully handled, never mid-`mach_msg2` and never mid-demand-page (risk R3):

```rust
// M14: switch only at a CLEAN stop boundary. Switching mid-mach_msg2 or mid-demand-page would
// leave in-flight box state describing a thread that is no longer running (risk R3).
if !matches!(self.threads.state_of(self.threads.current()), thread::ThreadState::Runnable) {
    self.schedule_after_block();
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p retrace-box --test threads -- --test-threads=1
```
Expected: PASS, 13 tests.

- [ ] **Step 5: FULL GATE (controller)**

`run()` is shared by every guest in the workspace, so this is the highest-risk edit in the milestone.

```bash
just gate 2>&1 | tail -20
```
Expected: **324 passed / 0 failed / 1 ignored.** A regression here means the single-threaded path stopped being the one-entry-table path — check that a lone `Runnable` thread 0 never triggers a switch.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/threads.rs
git commit -m "M14 t9: the scheduler runs, below the trace, where determinism is free"
```

---

### Task 10: Prove the switch is non-vacuous by deleting it

M9 built `flush_guest_tlb` and then discovered jq never used it. M13 measured its flush's non-vacuity by reverting it and watching the guest report `the protected store was NOT denied`. Do the same here **before** the headline, so the headline's green is known to mean something.

**Files:**
- Create: `.superpowers/sdd/2026-08-12-retrace-m14-threads/task-10-nonvacuity.md`

- [ ] **Step 1: Record the baseline**

```bash
cd /Users/noahmitchem/Documents/GitHub/retrace
cargo test -p retrace-box --test threads -- --test-threads=1 2>&1 | tail -3
```

- [ ] **Step 2: Break `pick_next` and confirm the tests notice**

Temporarily make it always return `Some(0)`:

```bash
# In crates/retrace-box/src/thread.rs, replace pick_next's body with `Some(0)`.
cargo test -p retrace-box --test threads -- --test-threads=1 2>&1 | tail -5
```
Expected: `run_switches_to_the_child_when_main_blocks` and
`a_deadlock_fails_loud_instead_of_hanging` **FAIL**. If they pass, the tests are vacuous and must be
strengthened before the headline is written.

- [ ] **Step 3: Revert and confirm green**

```bash
git checkout crates/retrace-box/src/thread.rs
cargo test -p retrace-box --test threads -- --test-threads=1 2>&1 | tail -3
```

- [ ] **Step 4: Write the report**

Record both outcomes verbatim. A test suite that cannot fail is the thing this task exists to catch.

- [ ] **Step 5: Commit**

```bash
git commit --allow-empty -m "M14 t10: the scheduler tests fail when the scheduler is broken"
```

---

### Task 11: The headline — `threadrust.rs` and `thread_rust_e2e`

**Files:**
- Create: `crates/retrace-guest/rs/threadrust.rs`
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs`
- Create: `crates/retrace/tests/thread_rust_e2e.rs`

- [ ] **Step 1: Write the guest**

```rust
// M14's headline guest. A stock full-std Rust binary with two threads of control.
//
// `joined 42` is the load-bearing line: it can be printed only if the child thread genuinely ran
// AND its return value crossed back through join. Exit 0 proves nothing — a guest that never
// spawned also exits 0, which is the trap segv_rust_e2e documented and protnone_rust_e2e sharpened.
fn main() {
    println!("main before spawn");
    let h = std::thread::spawn(|| {
        println!("child ran");
        42u32
    });
    let v = h.join().unwrap();
    println!("joined {v}");
}
```

- [ ] **Step 2: Wire the build**

In `crates/retrace-guest/build.rs`, following the `protrust` recipe exactly:

```rust
    // threadrust: M14's headline — a stock full-std Rust binary that spawns a thread and joins it.
    let src = format!("{}/rs/threadrust.rs", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/threadrust");
    println!("cargo:rerun-if-changed={src}");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let status = Command::new(rustc)
        .args(["--target", "aarch64-apple-darwin", "-o", &bin, &src])
        .status().expect("rustc threadrust");
    assert!(status.success(), "threadrust guest build failed");
```

In `crates/retrace-guest/src/lib.rs`, beside `PROTRUST`:

```rust
pub const THREADRUST: &str = concat!(env!("OUT_DIR"), "/threadrust");
```

- [ ] **Step 3: Write the gate**

```rust
// THE M14 HEADLINE GATE. A stock full-std Rust binary spawns a thread, the child runs on retrace's
// single vCPU under a cooperative scheduler, and its return value crosses back through join.
//
// The exit code proves nothing on its own: a guest that never spawned also exits 0. `joined 42` is
// the assertion — it requires the child to have RUN and its value to have PROPAGATED.
mod util;
use retrace_trace::Event;

#[test]
fn a_rust_guest_spawns_a_thread_and_joins_it() {
    let (rec, trace) = util::record_dynamic(retrace_guest::THREADRUST);
    let out = String::from_utf8_lossy(&rec.stdout);

    assert!(out.contains("main before spawn"), "the guest must reach main; stdout:\n{out}");
    assert!(out.contains("child ran"),
        "THE CHILD THREAD MUST ACTUALLY RUN. Missing this line is what a scheduler that never \
         switches looks like; stdout:\n{out}");
    assert!(out.contains("joined 42"),
        "the child's return value must cross back through join; stdout:\n{out}");
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    let (events, torn) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(!torn, "a recorder killed mid-run leaves a torn trace — this must be complete");

    // The guest genuinely asked for a thread, rather than libstd optimizing the spawn away.
    assert!(
        events.iter().any(|e| matches!(e,
            Event::Syscall { num, .. } if *num == retrace_arch::SYS_BSDTHREAD_CREATE)),
        "the trace must contain the bsdthread_create the guest issued"
    );

    // Replay is byte-identical, twice. This is where a nondeterministic schedule would surface:
    // a different interleaving reorders the guest's own writes and the stdout comparison fails.
    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 0, "replay {i}; stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged — the schedule is not pure");
    }
}
```

- [ ] **Step 4: Run it**

```bash
cargo test -p retrace --test thread_rust_e2e -- --test-threads=1
```

**This is the wall.** If it fails, use `RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn <threadrust> -o /tmp/t.bin` and read the last traps, exactly as Task 1 did. Diagnose before changing anything — `superpowers:systematic-debugging`.

- [ ] **Step 5: If a wall is found that M14 cannot clear, park a NEW gate**

Honest-gate discipline. Do **not** loosen this test, and do **not** regress any of the seven existing headline gates. Park a new `#[ignore = "…"]` test for the specific capability that is blocked, with the reason stating what was *measured*, and confirm the parked test dies where the reason says it does when forced with `--ignored`.

- [ ] **Step 6: FULL GATE (controller)**

```bash
just gate 2>&1 | tail -20
```
Expected: **325 passed / 0 failed / 1 ignored** (or 1 passed fewer and 2 ignored if Step 5 fired).

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-guest/rs/threadrust.rs crates/retrace-guest/build.rs \
        crates/retrace-guest/src/lib.rs crates/retrace/tests/thread_rust_e2e.rs
git commit -m "M14 t11: a Rust guest spawns a thread and joins it"
```

---

### Task 12: The honest close

- [ ] **Step 1: Run the full gate to a real number**

**Controller only.** Chunk it — a single `cargo test --workspace` has been killed on this machine:

```bash
pgrep -fl "cargo test" || echo "(clear to start)"
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude retrace-box --exclude retrace -- --test-threads=1
cargo test -p retrace-box -- --test-threads=1
# then per-target for -p retrace, plus:
cargo test -p retrace --bins -- --test-threads=1    # NOT --bins --lib; see Global Constraints
```

Record the measured totals. **Do not write a projection into the README.**

- [ ] **Step 2: Write the README Status section**

Append `## Status: M14-threads — …`. It must bill, honestly:
- What now runs, with the measured gate numbers.
- **That M13's `mach_vm_protect` routing, billed as dormant, was this milestone's prerequisite.**
- That `bsdthread_register` had been firing unremarked since M7.
- Every unmodelled thing: `workq`/GCD, preemption, per-thread seek/stepping, thread-aware watchpoints, spin-waiting guests, and everything M13 carried forward.
- Any gate parked in Task 11, and its wall.
- Whatever Tasks 1–2 measured that contradicted this plan. M13 found fourteen plan defects; a plan that survives contact unamended is more likely unexamined than perfect.

- [ ] **Step 3: Update CLAUDE.md**

The milestone list, the headline-gate paragraph (seven → eight if Task 11 landed), and the gate numbers.

- [ ] **Step 4: Commit and merge**

```bash
git add README.md CLAUDE.md
git commit -m "M14 t12: the honest close"
```
Then `superpowers:finishing-a-development-branch`. The M13 precedent is a local `--no-ff` merge to `main`; **ask, do not assume** — and note `main` was left unpushed after M13, so confirm whether this merge should push.

---

## Self-Review

**Spec coverage.** M14-tcb → Task 4. M14-create → Task 7. M14-switch → Task 5. M14-sched → Task 9. Determinism posture (no format change) → asserted by Task 11's replay comparison. Fail-loud boundaries → Tasks 3 (announce), 7 (no trampoline), 9 (deadlock). R1 → Task 1. R2 (`TPIDR_EL0` stays 0) → Tasks 5 and 7, asserted in Task 5's test. R3 (switch only at clean boundaries) → Task 9 Step 3. R4 → Task 6. Exit criterion → Task 11. Testing ladder → Tasks 1, 4, 5–9, 11. Open questions 1 and 3 → Tasks 8 and 5 respectively.

**Gap found and accepted:** spec open question 2 (per-thread signal *mask*) has no task. M11 modelled dispositions as process-wide, which stays correct, and the `spawn`+`join` headline never touches a per-thread mask. Task 12 must list it as unmodelled rather than let it pass silently.

**Placeholder scan.** No TBD/TODO. Task 8 Step 4 presents three concrete alternatives keyed to Task 1's measurement rather than deferring — the implementer writes one and deletes the others.

**Type consistency.** `ThreadCtx`/`ThreadState`/`BlockReason`/`Thread`/`ThreadTable` are defined in Task 4 and used unchanged in 5–11. `pick_next` returns `Option<usize>` throughout. `guest_bsdthread_create` returns `u64` in both its definition and its tests. `tpidrro_el0` (per-thread) and `tpidr_el0` (always 0) are kept distinct everywhere.

**Known plan risk.** Tasks 7 and 8 encode a *measured-once* ABI. Task 7 Step 2 says explicitly that Task 2's measurement overrides the plan's register layout. That instruction is load-bearing: M13's Task 10 shipped a planned guest writing to the wrong register whose assertion would have passed vacuously.
