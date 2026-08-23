# M18-workq Stage 2b Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a libdispatch worker thread inside the box and give it a mach-semaphore park/wake
seam, so `crates/retrace-guest/c/dispatch_dyn.c` runs its block, signals main, and records and
replays bit-for-bit — or parks the gate at a wall Stage 2b actually reached and measured.

**Architecture:** Two halves with opposite shapes. Worker construction lands entirely in
`retrace-box`: Stage 2a's record arm and replay mirror already call the same
`Box_::guest_workq_kernreturn` with the same args, so replacing its `REQTHREADS` panic with a spawn
is symmetric by construction and needs no `retrace-core` change at all. The semaphore seam is the
opposite — the wait trap is a raw negative Mach trap with no dedicated arm, so it needs new record
arms, new replay mirrors, two new `verify_thread` oracle sites, and a fail-loud guard on the generic
negative-trap forward.

**Tech Stack:** Rust 1.95.0, `aarch64-apple-darwin`, Hypervisor.framework, macOS 26.x on Apple
Silicon. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-design.md`

## Global Constraints

- **`--test-threads=1` is mandatory.** HVF allows one VM per process; a bare `cargo test` flakes with `HV_BUSY`.
- **The gate is chunked** — the full `--workspace` run exceeds the 10-minute tool ceiling and gets killed. A kill is not a red. Run every chunk `--no-fail-fast` and capture cargo's exit code **before any pipe**:
  ```sh
  cargo test --workspace --exclude retrace-box --exclude retrace -- --test-threads=1
  cargo test -p retrace-box -- --test-threads=1
  cargo test -p retrace --test <name> -- --test-threads=1   # per-target, for each e2e gate
  cargo test -p retrace --bins -- --test-threads=1          # NEVER omit: 8 unit tests run in no other chunk
  ```
  `cargo test -p retrace --lib` is **invalid** for this crate (no lib target) and fails loudly. The trap is that the wrong flag is loud and the missing one is silent.
- **Grep gate logs with `grep -a`** — they carry ANSI and UTF-8 that trips plain grep.
- **clippy must be clean at `-D warnings`.** `clippy.toml` bans `Instant::now`/`SystemTime::now` (determinism) and `std::thread::Thread` (retrace's core is single-threaded). Those denials are load-bearing, not style.
- **`TRACE_MAGIC` does NOT move.** Stage 2b adds no `Event` variant and no field. If you believe you need one, stop — that is a spec deviation, not an implementation detail.
- **Symmetry rule 1:** a special case in record's `match stop` needs a mirror in replay's dispatch, both calling the *same* `Box_` method with the *same* arguments. Record lives in `record_box`, replay in `ReplaySession::advance`, both in `crates/retrace-core/src/lib.rs`. New arms go **before** the generic forward arm.
- **Symmetry rule 2:** deterministic emulation is better done below the trace inside `Box_::run()`, which record and replay share.
- **Never forward** `workq_open` (367), `workq_kernreturn` (368), or the semaphore traps. Forwarding the semaphore wait is not whole-process-*fatal* but is whole-process-*hanging*, which is just as fatal to a recording.
- **Measured vs. attributed discipline.** The raw value is the measurement; the name is a lead. Any XNU/libpthread symbol name not verified against this machine must be labelled attributed at its use site, exactly as `guest_workq_kernreturn`'s opcode constants already are.
- **Honest-gate discipline.** A headline gate is parked `#[ignore]`d at the current wall with the wall documented honestly — never faked green, never deleted. Assert on the difference your work makes, never on an exit code a weaker failure would also produce.

---

### Task 1: Measure the `wqthread` entry contract and verify the trap numbers

**No production code.** This task writes one document. Stage 2a's Task 4 has the same shape and is
the model to follow; read it before starting.

**Files:**
- Create: `docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-wqthread-measurements.md`
- Read (do not modify): `docs/superpowers/specs/2026-08-21-retrace-m18-stage2b-measurements.md`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: the document above, whose §1 (register contract table), §2 (memory layout), §3 (verified
  trap numbers) and §4 (struct-init verdict) are consumed by name by Tasks 2, 3 and 4.

- [ ] **Step 1: Verify the two trap numbers against libsystem_kernel's own stubs**

This is the highest-value single command in the task: it converts `-36` and `-33` from *attributed*
to *verified*, discharging the largest attribution debt Stage 2a left. The trap stubs live in
libsystem_kernel and each loads its own trap number into `x16`.

```sh
otool -tV /usr/lib/system/libsystem_kernel.dylib \
  | grep -A6 -E '^_semaphore_(wait|signal|wait_signal|timedwait)_trap:'
```

Record, verbatim, the `mov x16, #...` (or `movn`) immediate for each stub found. `-36` and `-33` are
predictions from the prior document; **write down what the disassembly actually says**, and if it
disagrees, the disassembly wins and you say so in bold.

If `otool` cannot read the dylib (it is in the dyld shared cache), fall back to:

```sh
objdump --macho --disassemble --disassemble-symbols=_semaphore_wait_trap,_semaphore_signal_trap \
  /usr/lib/system/libsystem_kernel.dylib
```

If both fail, say so explicitly in the document rather than inferring — an unavailable measurement
is a finding, not a gap to paper over.

- [ ] **Step 2: Disassemble the workqueue thread entry point**

```sh
otool -tV /usr/lib/system/libsystem_pthread.dylib \
  | sed -n '/^_start_wqthread:/,/^_[a-z]/p' | head -60
otool -tV /usr/lib/system/libsystem_pthread.dylib \
  | sed -n '/^__pthread_wqthread:/,/^_[a-z]/p' | head -200
```

Read these the way M14 Task 9 read `__pthread_start` — **for which registers are actually READ on
the entry-to-dispatch path**, not for which registers the published ABI says exist. M14's plan
guessed the `_pthread_start` shape and was wrong; the real contract turned out to be "x0 and w5, and
func/arg come from the struct." Expect the same kind of surprise here and let the disassembly
decide.

For every register the code reads before its first call, record: the register, the instruction and
its address, and what the value is used for. That table is §1 of your document.

- [ ] **Step 3: Determine who initialises the pthread struct — the spec's load-bearing hypothesis**

The spec (`§ The kernel-allocates problem, and the escape hatch`) hypothesises that libpthread
initialises its own struct on the *fresh* thread path, selected by a flag bit, so retrace invents an
address and not a layout. **Verify or kill it.** Look in the `_pthread_wqthread` disassembly for:

- a `tbz`/`tbnz`/`and` test on the flags register identified in Step 2, and where each branch goes;
- a call to a struct-initialising routine on one branch (`__pthread_struct_init` or similar) —
  disassemble whatever it calls and confirm it writes the struct rather than reading it;
- any `brk` on the other branch, which is libpthread's way of saying "the kernel owed me this"
  (M14 hit exactly such a `brk #0xb001` for `PTHREAD_START_TSD_BASE_SET`).

Write §4 as one of two verdicts, in bold:
- **CONFIRMED** — name the flag bit, its value, and the routine that does the init.
- **KILLED** — name what libpthread requires the kernel to have written. This is a wall, and Task 5
  will park the gate at it. Do **not** reconstruct a struct layout from libpthread sources that were
  never verified against this machine; that is exactly the invention Stage 2a's discipline forbids.

- [ ] **Step 4: Determine the stack and struct memory layout**

From the same disassembly, determine where `_pthread_wqthread` expects the pthread struct to sit
relative to the stack pointer or the stack-address register it is handed, and whether it computes
one from the other. Record in §2: the size retrace must allocate, and the exact relationship between
the stack region and the struct address. If the code derives the struct from a register retrace
supplies, say which register.

Also record whether `TPIDRRO_EL0` is read on this path, and if so at what offset from the struct —
M14 measured `pthread + 0xe0` for a `bsdthread_create` child (4/4 by host probe) and the question is
whether a workqueue thread agrees.

- [ ] **Step 5: Disassemble the semaphore signal path in libdispatch**

The prior document is explicit that `-33` has **never been observed by anything** — that
`dispatch_semaphore_signal` traps at all is inference. Settle it:

```sh
otool -tV /usr/lib/system/libdispatch.dylib \
  | sed -n '/^_dispatch_semaphore_signal:/,/^_[a-z]/p' | head -80
```

Record whether the fast path is a pure atomic increment with no trap, and what the slow path calls
when a waiter exists. §3 must state, as a measured fact or an explicit unknown, which trap number
the signal path issues.

- [ ] **Step 6: Write the document**

Structure it as §1 register contract, §2 memory layout, §3 verified trap numbers, §4 struct-init
verdict, §5 what this hands Tasks 2–4. Follow the prior document's conventions exactly: every claim
labelled measured / attributed / unmeasured, every command that produced a number quoted verbatim,
and any claim you could not verify stated in bold as unverified rather than softened.

- [ ] **Step 7: Commit**

```bash
git add docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-wqthread-measurements.md
git commit -m "M18 Stage 2b t1: measure the wqthread entry contract and verify the trap numbers"
```

---

### Task 2: Build the worker at `REQTHREADS`

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` — `guest_workq_kernreturn`'s `WQOPS_QUEUE_REQTHREADS` arm; add `guest_workq_reqthreads`
- Modify: `crates/retrace-arch/src/lib.rs` — the two mach-semaphore trap-number constants
- Modify: `crates/retrace-core/src/lib.rs:531` — the fail-loud guard on the generic negative-trap forward
- Modify: `crates/retrace/tests/dispatch_e2e.rs` — retarget one assertion (see Step 7)
- Test: `crates/retrace-box/tests/threads.rs`

**Why this task reaches outside `retrace-box`.** Worker construction on its own needs no
`retrace-core` change — that is the plan's headline simplification and it still holds. But a worker
that exists is a worker that walks main into the mach semaphore wait, and until that trap is either
serviced (Task 4) or refused, it hangs the recorder. The guard is what makes this task's own
deliverable safe to leave in the tree, so it lands here rather than two tasks later.

**Interfaces:**
- Consumes: §1 (register contract), §2 (memory layout) and §4 (struct-init verdict) of
  `docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-wqthread-measurements.md`. **Read that
  document before writing code** — the register seeding below is driven by its §1 table and the
  values in it are not guessable.
- Produces:
  - `Box_::guest_workq_reqthreads(&mut self, args: [u64; 8]) -> u64`, returning 0. After it returns,
    the thread table holds exactly one additional `Runnable` thread whose `ctx.regs.pc` and
    `ctx.elr` are `self.wq_thread_pc.unwrap()`.
  - `retrace_arch::MACH_SEMAPHORE_WAIT` and `retrace_arch::MACH_SEMAPHORE_SIGNAL` — the verified
    trap numbers from Task 1 §3. **Task 4 consumes these; it does not define them.**
  - The fail-loud guard on the generic negative-trap forward arm
    (`crates/retrace-core/src/lib.rs:531`). Task 4's arms sit before it, which is what makes it
    stop firing without being removed.

**If Task 1's §4 verdict is KILLED, stop and report BLOCKED** with that verdict quoted. Do not
invent a struct layout. The controller will route to Task 5 to park the gate at that wall.

- [ ] **Step 1: Write the failing tests**

Add to `crates/retrace-box/tests/threads.rs`. Note the existing `tb()` helper at line 139 and the
existing Stage 2a workq tests around line 1008 — put these beside them.

```rust
/// M18 Stage 2b: REQTHREADS builds the worker libdispatch asked for. Before Stage 2b this panicked
/// by design ("worker construction is Stage 2b"); that wall is now gone and this is what replaced it.
#[test]
fn workq_reqthreads_spawns_one_runnable_worker_at_the_registered_entry() {
    let mut b = tb();
    // bsdthread_register captures the wqthread entry pc (arg 1) and the pthread size (arg 2).
    b.guest_bsdthread_register([0x1111, 0x2222, 0x3333, 0, 0, 0, 0, 0]);
    let before = b.threads().len();
    // The measured REQTHREADS args vector, verbatim (M18 Task 6 / Stage 2a t10).
    let rc = b.guest_workq_kernreturn([0x20, 0x0, 0x1, 0x40008ff, 0x0, 0x20, 0, 0]);
    assert_eq!(rc, 0, "libdispatch reads a failure as 'no workqueue at all'");
    assert_eq!(b.threads().len(), before + 1, "exactly one worker, not zero and not two");
    let w = before; // the new thread's index
    assert_eq!(b.threads().state_of(w), ThreadState::Runnable,
        "the worker must be runnable but NOT current — a switch here would reorder the guest's \
         own output, the same argument ThreadTable::spawn already makes for bsdthread_create");
    assert_eq!(b.threads().current(), 0, "REQTHREADS must not switch away from the caller");
    assert_eq!(b.threads().ctx_of(w).regs.pc, 0x2222, "entered at the REGISTERED wqthread, not an invented pc");
    assert_eq!(b.threads().ctx_of(w).elr, 0x2222,
        "both pc and elr, for the reason guest_bsdthread_create's comment gives: the two resume \
         paths read different registers");
}

/// The `guest_workq_open` posture: refuse to build a worker with no registered entry point rather
/// than invent one. Same failure mode M14 refused for bsdthread_create.
#[test]
#[should_panic(expected = "no registered wqthread")]
fn workq_reqthreads_refuses_without_a_registered_entry_point() {
    let mut b = tb();
    b.guest_workq_kernreturn([0x20, 0x0, 0x1, 0x40008ff, 0x0, 0x20, 0, 0]);
}

/// Determinism: the worker's stack and struct must land at the same IPAs on record and replay.
/// Two boxes driven through an identical call sequence must agree — that IS the property, and it is
/// what lets the whole worker half stay below the trace with nothing recorded.
#[test]
fn workq_reqthreads_allocates_deterministically() {
    let sp = |b: &retrace_box::Box_, w: usize| b.threads().ctx_of(w).regs.sp_el0;
    let mut a = tb();
    a.guest_bsdthread_register([0x1111, 0x2222, 0x3333, 0, 0, 0, 0, 0]);
    a.guest_workq_kernreturn([0x20, 0x0, 0x1, 0x40008ff, 0x0, 0x20, 0, 0]);
    let first = sp(&a, 1);
    drop(a); // HVF: one VM per process, so the second box cannot coexist with the first.
    let mut c = tb();
    c.guest_bsdthread_register([0x1111, 0x2222, 0x3333, 0, 0, 0, 0, 0]);
    c.guest_workq_kernreturn([0x20, 0x0, 0x1, 0x40008ff, 0x0, 0x20, 0, 0]);
    assert_eq!(sp(&c, 1), first, "identical call sequence must yield an identical worker stack IPA");
    assert_ne!(first, 0, "a zero stack pointer would mean nothing was allocated");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
cargo test -p retrace-box --test threads workq_reqthreads -- --test-threads=1
```

Expected: FAIL. The first and third panic with "worker construction is Stage 2b" (Stage 2a's wall);
the second fails because that panic's message does not contain "no registered wqthread".

- [ ] **Step 3: Replace the REQTHREADS wall with the spawn**

In `crates/retrace-box/src/lib.rs`, change `guest_workq_kernreturn`'s `WQOPS_QUEUE_REQTHREADS` arm
from the `panic!` to `WQOPS_QUEUE_REQTHREADS => self.guest_workq_reqthreads(args),` and **keep the
`other => panic!` arm exactly as it is** — the park/return opcodes a running worker issues are still
unmeasured, and that arm refusing them by value is what will name them.

Then add the method. The register-seeding block is where Task 1's §1 table lands; every line in it
must cite the instruction that justifies it, the way `guest_bsdthread_create`'s contract comment
does. Do not seed a register §1 does not show being read.

```rust
    /// `workq_kernreturn(WQOPS_QUEUE_REQTHREADS, ...)` — libdispatch asking the kernel for worker
    /// threads. **Emulated, never forwarded**, for the reason `guest_workq_open` documents.
    ///
    /// Stage 2a parked a deliberate panic here because the kernel allocates a workqueue thread's
    /// stack and pthread struct and enters `wqthread` with a register contract nobody had measured.
    /// Task 1 measured it; see
    /// `docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-wqthread-measurements.md`.
    ///
    /// **This differs from `bsdthread_create` in who owns the memory.** There the guest had already
    /// mapped the stack and populated the struct, so M14 seeded registers and touched nothing else.
    /// Here the kernel owns both, so the box must place them — but it places an ADDRESS, not a
    /// LAYOUT: libpthread initialises its own struct on the fresh-thread path (Task 1 §4), which is
    /// what keeps M14's no-invention rule intact rather than carving an exception out of it.
    ///
    /// Deterministic with nothing recorded: the bump allocator is a pure function of the guest's
    /// syscall sequence, and both dispatch arms reach this through the same
    /// `guest_workq_kernreturn(args)` call, so record and replay place the worker identically.
    fn guest_workq_reqthreads(&mut self, args: [u64; 8]) -> u64 {
        let entry = self.wq_thread_pc.expect(
            "M18 Stage 2b: REQTHREADS with no registered wqthread entry point — refusing to build \
             a worker that would enter an invented address. Every dynamic guest registers one at \
             startup (measured, M14 Task 2), so this means the guest took a path no measurement \
             covers.");
        let pthsize = self.pthread_size.expect(
            "M18 Stage 2b: REQTHREADS with no registered pthread size — captured by the same \
             bsdthread_register that captures the entry point, so this cannot happen without the \
             assert above firing first.") as u64;

        // Stack and struct, in ONE reservation so their relative placement is what Task 1 §2
        // measured rather than two independent bumps that could drift apart.
        let (stack_base, stack_top, pthread) = self.place_worker_stack(pthsize);

        let mut ctx = thread::ThreadCtx::zeroed();
        ctx.elr = entry;
        ctx.regs.pc = entry;
        // The creating thread's EL0 PSTATE, NOT zeroed()'s 0 — M14's lesson: 0 is EL0t with DAIF
        // clear, which looks like it works right up until the mask bits matter.
        ctx.spsr = self.spsr();
        ctx.regs.cpsr = ctx.spsr;
        ctx.regs.sp_el0 = stack_top;

        // ---- THE WQTHREAD ENTRY CONTRACT, MEASURED (Task 1 §1) ----------------------------
        // Seed exactly the registers §1 shows being READ before the first call, each with the
        // instruction and address that justifies it, in the style of guest_bsdthread_create's
        // contract comment. Registers §1 does not show being read stay zero: seeding one the code
        // never reads is invention that happens to be harmless today.
        // <implementer: transcribe §1 here>

        let _ = args; // the request's own arguments; §1 decides whether any of them is read.
        self.threads.spawn(ctx, (stack_base, stack_top - stack_base));
        0
    }

    /// Place a workqueue worker's stack and pthread struct. Returns `(stack_base, stack_top,
    /// pthread)`. The layout relationship is Task 1 §2's measurement, not a convention.
    fn place_worker_stack(&mut self, pthsize: u64) -> (u64, u64, u64) {
        // <implementer: size and relationship from §2. Use guest_vm_reserve(.., anywhere=true)
        // followed by commit_reserved_page for each page the worker will touch, or the same
        // placement path the guest's own thread stacks take — whichever §2's layout requires.
        // Whatever you choose must be a pure function of the call sequence.>
    }
```

**Note on `spawn`'s second argument.** `Thread::stack` is documented as `(base, len)` and is used
for teardown and diagnostics only. `guest_bsdthread_create` passes `(stack, 0)` with a placeholder
length because the syscall carries no size; here the box *does* know the size, so pass the real one.

- [ ] **Step 4: Add the trap-number constants**

In `crates/retrace-arch/src/lib.rs`, beside the existing syscall numbers. Use the values Task 1 §3
**verified**, not the predicted `-36`/`-33`, and say in the doc comment whether §3 confirmed or
corrected the prediction:

```rust
/// The mach semaphore wait trap. Negative x16, like the other Mach traps.
///
/// Measured: Stage 2a's two runs both ended on this trap at `pc=0x1804adbb0` carrying the port name
/// its preceding `semaphore_create` minted. Task 1 §3 verified the number against
/// libsystem_kernel's own stub. `Box_::guest_sem_wait` emulates it; it is never forwarded.
pub const MACH_SEMAPHORE_WAIT: u64 = /* Task 1 §3 */;
/// The mach semaphore signal trap — the wake half of the pair. Task 1 §3 measured it; before that
/// it had never been observed by anything, and this plan says so rather than inheriting a guess.
pub const MACH_SEMAPHORE_SIGNAL: u64 = /* Task 1 §3 */;
```

- [ ] **Step 5: Add the fail-loud guard at the generic negative-trap arm**

In `record_box`, inside the arm at `:531` (`Stop::Syscall { num, args } if (num as i64) < 0`), as its
first statement:

```rust
                // M18 Stage 2b: the semaphore pair must never reach here. Forwarding either is not
                // whole-process-fatal the way forwarding the workq pair is, but it is
                // whole-process-HANGING, which is just as fatal to a recording: both Stage 2a
                // measurement runs blocked here forever and produced zero bytes of guest stdout.
                //
                // This guard's SHAPE is the one the workq pair uses on the generic BSD forward arm,
                // but deliberately not its LOCATION: that arm is BSD-only and a negative trap
                // number never reaches it (measurements doc §2, corrected in 60cea11). Negative
                // traps are caught here, so the guard belongs here.
                assert!(num != retrace_arch::MACH_SEMAPHORE_WAIT
                     && num != retrace_arch::MACH_SEMAPHORE_SIGNAL,
                    "M18 Stage 2b: mach semaphore trap {num:#x} reached the generic forward arm — \
                     it must be serviced by its dedicated arm above (Box_::guest_sem_wait / \
                     guest_sem_signal). Forwarding it blocks retrace's own process forever on a \
                     semaphore only the guest's worker could signal. args={args:#x?}");
```

- [ ] **Step 6: Verify the guard fires — this is what makes worker construction safe**

```sh
cargo test -p retrace --test dispatch_e2e --no-fail-fast -- --test-threads=1 --nocapture --ignored 2>&1 | tail -40
```

Expected: FAIL, and **the failure must be the guard's assert message**. That is the whole point of
landing the guard in this task: the moment a worker exists, main reaches the semaphore wait, and
without the guard that trap reaches `forward_and_diff` and **hangs retrace's own process forever** —
burning the full tool timeout on every suite run from here until Task 4, with no reviewer able to
tell a hang from a broken task.

If the run hangs instead of asserting, the guard is misplaced: it is sitting *after* the forward
rather than before it. Fix the placement, do not raise the timeout.

- [ ] **Step 7: Retarget the companion test's moving assertion**

`crates/retrace/tests/dispatch_e2e.rs::the_workqueue_syscalls_are_emulated_not_forwarded` asserts
`rec.stderr.contains("worker construction is Stage 2b")` — the panic Step 3 just removed. It is now
red for a stale reason.

Change **only that one assertion** to name the guard's message instead, and update the doc comment
above the test to say what it now proves (the workqueue syscalls are emulated *and* a built worker
reaches the semaphore trap, which the guard refuses to forward).

**Keep the test's other two assertions verbatim.** `assert_ne!(rec.code, 139)` and the
`_pthread_wqthread` stderr tripwire are durable — they are the "no host workqueue thread inside the
recorder" guarantee this file exists to enforce, and they outlive every wall that moves past them.
Task 4 moves this assertion once more, and Task 5 sets its final state; that churn is deliberate, so
that every task boundary leaves the suite green rather than knowingly red.

- [ ] **Step 8: Run the tests to verify they pass**

```sh
cargo test -p retrace-box --test threads workq_reqthreads -- --test-threads=1
```

Expected: PASS, 3 tests.

- [ ] **Step 9: Run the full box suite plus the two chunks this task now reaches**

```sh
cargo test -p retrace-box --no-fail-fast -- --test-threads=1
cargo test -p retrace --test dispatch_e2e --no-fail-fast -- --test-threads=1
cargo clippy --workspace --all-targets -- -D warnings
```

The second chunk is here because Steps 4-7 reached into `retrace-arch`, `retrace-core` and the
e2e test; a `-p retrace-box`-only run would not have seen any of it. Expected: PASS (the headline
gate stays `#[ignore]`d and is not run by this command).

Expected: PASS. Note that `workq_kernreturn_reqthreads_is_the_named_stage_2a_wall` **will now fail**
— it is a `#[should_panic(expected = "worker construction is Stage 2b")]` on the wall you just
removed. Delete that test in this task and say so in the commit message: its successor is
`workq_reqthreads_spawns_one_runnable_worker_at_the_registered_entry`, which asserts what replaced
the wall. Keep `workq_kernreturn_refuses_an_unmeasured_opcode_by_value` untouched — the unmeasured-opcode
posture is unchanged and still load-bearing.

- [ ] **Step 10: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/threads.rs \
        crates/retrace-arch/src/lib.rs crates/retrace-core/src/lib.rs \
        crates/retrace/tests/dispatch_e2e.rs
git commit -m "M18 Stage 2b t2: REQTHREADS builds the worker, and the guard that makes it safe"
```

---

### Task 3: The mach-semaphore park/wake seam, and the worker's own park

**Files:**
- Modify: `crates/retrace-box/src/thread.rs` — `BlockReason` (two new variants), `unblock_sem_waiters_on`
- Modify: `crates/retrace-box/src/lib.rs` — `guest_sem_wait`, `guest_sem_signal`, and `guest_workq_kernreturn`'s new park arm
- Test: `crates/retrace-box/tests/threads.rs` (both the pure-table tests and the box-method tests)

**Where the tests go, because this crate splits them by kind.** `src/thread.rs`'s in-module
`#[cfg(test)] mod tests` holds only the M16 per-thread *state* tests (masks, pending sets, alt
stacks) and builds contexts with a bare `ThreadCtx::zeroed()`. The *scheduling* tests — `Join`,
`Wait`, `pick_next`, `unblock_*` — all live in `crates/retrace-box/tests/threads.rs` and use its
`ctx(pc)` helper (line 8). The new wake-seam tests are scheduling tests, so they go there.

**Interfaces:**
- Consumes: §3 (verified trap numbers) of Task 1's document.
- Produces:
  - `retrace_box::thread::BlockReason::Sem { port: u64 }`
  - `ThreadTable::unblock_sem_waiters_on(&mut self, port: u64) -> Vec<usize>`
  - `Box_::guest_sem_wait(&mut self, args: [u64; 8]) -> u64` — returns 0
  - `Box_::guest_sem_signal(&mut self, args: [u64; 8]) -> (u64, Vec<usize>)` — returns `(0, woken)`
  - a park arm in `guest_workq_kernreturn` for opcode `0x4`, leaving the worker non-runnable
    (see Step 9 — measured by Task 1, and required for the gate to go green)

  The `(rc, woken)` shape of `guest_sem_signal` deliberately mirrors `guest_ulock_wake`, because
  Task 4's record arm needs the woken tids for the same reason M17's wake arm does: to materialise a
  signal pended on a thread that could not run.

- [ ] **Step 1: Write the failing thread-table tests**

Add to `crates/retrace-box/tests/threads.rs`, beside the existing `Join`/`Wait` scheduling tests, using that file's `ctx(pc)` helper:

```rust
#[test]
fn a_thread_blocked_on_a_semaphore_is_woken_by_that_ports_signal() {
    let mut t = ThreadTable::new(ctx(0x1000));
    t.spawn(ctx(0x2000), (0x30200000, 0x8000));
    t.block(BlockReason::Sem { port: 0x1403 });
    assert_eq!(t.pick_next(), Some(1), "main parked on the semaphore, so the worker runs");
    assert_eq!(t.unblock_sem_waiters_on(0x1403), vec![0]);
    assert_eq!(t.state_of(0), ThreadState::Runnable);
}

#[test]
fn a_semaphore_signal_wakes_only_its_own_port() {
    let mut t = ThreadTable::new(ctx(0x1000));
    t.spawn(ctx(0x2000), (0x30200000, 0x8000));
    t.block(BlockReason::Sem { port: 0x1403 });
    // A DIFFERENT port must not wake it. Port names are dense small integers in retrace's own
    // IPC space, so a wake that ignored the key would look correct in a one-semaphore fixture.
    assert!(t.unblock_sem_waiters_on(0x1404).is_empty());
    assert_eq!(t.state_of(0), ThreadState::Blocked(BlockReason::Sem { port: 0x1403 }));
}

#[test]
fn a_semaphore_signal_with_no_waiter_is_legal_and_not_an_error() {
    let mut t = ThreadTable::new(ctx(0x1000));
    // The posture unblock_waiters_on already documents: the real kernel answers "nobody was
    // waiting" as an ordinary outcome, and dispatch's counting semaphore signals freely.
    assert!(t.unblock_sem_waiters_on(0x1403).is_empty());
}

#[test]
fn a_semaphore_block_does_not_disturb_the_ulock_wake_seam() {
    // Sem and Wait key on values from DIFFERENT address spaces that can collide numerically.
    // A port name of 0x1403 and a guest address of 0x1403 must never wake each other.
    let mut t = ThreadTable::new(ctx(0x1000));
    t.spawn(ctx(0x2000), (0x30200000, 0x8000));
    t.block(BlockReason::Sem { port: 0x1403 });
    assert!(t.unblock_waiters_on(0x1403).is_empty(), "an address wake must not wake a port waiter");
    assert_eq!(t.state_of(0), ThreadState::Blocked(BlockReason::Sem { port: 0x1403 }));
}
```

- [ ] **Step 2: Run them to verify they fail**

```sh
cargo test -p retrace-box --test threads semaphore -- --test-threads=1
```

Expected: FAIL to compile — `BlockReason::Sem` and `unblock_sem_waiters_on` do not exist.

- [ ] **Step 3: Add the variant and the wake seam**

In `crates/retrace-box/src/thread.rs`, add to `BlockReason`:

```rust
    /// M18 Stage 2b: waiting on a mach semaphore, keyed by PORT NAME.
    ///
    /// **The key lives in a different address space from every other variant here, and that is the
    /// whole point.** `Wait { addr }` correlates on `pthread + 0x34`, a guest memory address the box
    /// already tracks, because that is the value `__ulock_wait`/`__ulock_wake` name. A mach
    /// semaphore names a PORT in retrace's OWN IPC space, minted by a forwarded `semaphore_create`
    /// and reaching the guest through the recorded reply — never written into guest memory as such.
    /// Stage 2a measured that `dispatch_semaphore_wait` does not lower to a ulock at all
    /// (`num=515` appears nowhere in either trace), so there is no address to correlate on and this
    /// cannot be folded into `Wait`.
    ///
    /// Numeric collision with a `Wait` address is possible and harmless: the variants are distinct,
    /// so `unblock_waiters_on` and `unblock_sem_waiters_on` cannot wake each other's sleepers.
    Sem { port: u64 },
```

and the wake seam beside `unblock_waiters_on`:

```rust
    /// Wake every thread waiting on the mach semaphore named by `port`. Returns which tids woke.
    ///
    /// Sibling of `unblock_waiters_on`, and identical in every respect except the namespace of the
    /// key — see `BlockReason::Sem`. Returning the tids rather than `()` is what lets the caller
    /// materialise a signal pended on a thread that could not run, exactly as M17 uses the ulock
    /// wake's return. An empty result is legal and not an error: a counting semaphore is signalled
    /// freely whether or not anyone is parked on it.
    pub fn unblock_sem_waiters_on(&mut self, port: u64) -> Vec<usize> {
        let mut woken = Vec::new();
        for (tid, t) in self.threads.iter_mut().enumerate() {
            if let ThreadState::Blocked(BlockReason::Sem { port: p }) = t.state {
                if p == port {
                    t.state = ThreadState::Runnable;
                    woken.push(tid);
                }
            }
        }
        woken
    }
```

**Check `block`'s already-satisfied guard.** `block` special-cases `Join { target }` against a target
that has already exited. `Sem` has no analogous state in this table — the semaphore's count lives in
guest memory that libdispatch owns, and the guest's own fast path already decided it must block
before issuing the trap. Add a one-line comment in `block` saying `Sem` is unaffected and why, so the
next reader does not have to re-derive it.

- [ ] **Step 4: Run them to verify they pass**

```sh
cargo test -p retrace-box --test threads semaphore -- --test-threads=1
```

Expected: PASS, 4 tests.

- [ ] **Step 5: Write the failing box-method tests**

Add to `crates/retrace-box/tests/threads.rs`:

```rust
/// M18 Stage 2b: the wait trap parks the caller on the port it names. The port comes from args[0],
/// measured: both Stage 2a runs ended on this trap carrying 0x1403, the name the immediately
/// preceding semaphore_create reply minted.
#[test]
fn sem_wait_blocks_the_caller_on_the_port_in_arg0() {
    let mut b = tb();
    b.guest_bsdthread_register([0x1111, 0x2222, 0x3333, 0, 0, 0, 0, 0]);
    b.guest_workq_kernreturn([0x20, 0x0, 0x1, 0x40008ff, 0x0, 0x20, 0, 0]);
    let rc = b.guest_sem_wait([0x1403, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(rc, 0);
    assert_eq!(b.threads().state_of(0),
        ThreadState::Blocked(BlockReason::Sem { port: 0x1403 }));
    assert_eq!(b.threads().pick_next(), Some(1), "the worker is now the only runnable thread");
}

/// The signal trap wakes it, and names who it woke — the shape Task 4's record arm needs.
#[test]
fn sem_signal_wakes_the_waiter_and_names_it() {
    let mut b = tb();
    b.guest_bsdthread_register([0x1111, 0x2222, 0x3333, 0, 0, 0, 0, 0]);
    b.guest_workq_kernreturn([0x20, 0x0, 0x1, 0x40008ff, 0x0, 0x20, 0, 0]);
    b.guest_sem_wait([0x1403, 0, 0, 0, 0, 0, 0, 0]);
    let (rc, woken) = b.guest_sem_signal([0x1403, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(rc, 0);
    assert_eq!(woken, vec![0], "identity matters: M17 materialises a pended signal on the woken thread");
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable);
}
```

- [ ] **Step 6: Run them to verify they fail**

```sh
cargo test -p retrace-box --test threads sem_ -- --test-threads=1
```

Expected: FAIL to compile — `guest_sem_wait` / `guest_sem_signal` do not exist.

- [ ] **Step 7: Add the two box methods**

In `crates/retrace-box/src/lib.rs`, beside `guest_ulock_wait`/`guest_ulock_wake`:

```rust
    /// The mach semaphore wait trap, emulated. `args[0]` is the semaphore's port name.
    ///
    /// **Never forwarded.** Forwarding it is not whole-process-*fatal* the way forwarding
    /// `bsdthread_create` is — no host thread is created and nothing jumps to a guest address — but
    /// it is whole-process-*hanging*: it blocks retrace's own process on a semaphore that only the
    /// guest's own worker could ever signal, and nothing in retrace's process will. Both Stage 2a
    /// measurement runs died exactly that way, producing zero bytes of guest stdout
    /// (`docs/superpowers/specs/2026-08-21-retrace-m18-stage2b-measurements.md` §1, §2).
    ///
    /// Returns 0 unconditionally: the guest's own fast path already decided it must block before
    /// issuing the trap, so there is no "already signalled" case for this arm to answer.
    pub fn guest_sem_wait(&mut self, args: [u64; 8]) -> u64 {
        self.threads.block(thread::BlockReason::Sem { port: args[0] });
        0
    }

    /// The mach semaphore signal trap, emulated. `args[0]` is the port name; returns the woken tids
    /// for the reason `guest_ulock_wake` does — M17's materialisation site needs the identity.
    ///
    /// **Never forwarded**, for `guest_sem_wait`'s reason exactly.
    pub fn guest_sem_signal(&mut self, args: [u64; 8]) -> (u64, Vec<usize>) {
        let woken = self.threads.unblock_sem_waiters_on(args[0]);
        (0, woken)
    }
```

**On the absent operation-word assert.** `guest_ulock_wait` and `guest_ulock_wake` both assert on
`args[0]` because syscall 515/516 multiplex many operations through one number and an unmeasured
width would silently deadlock. These traps do not: the trap number *is* the operation, and `args[0]`
is the port. Add a sentence to each doc comment saying so — an absent guard that looks like an
oversight next to two neighbours that have one will be re-litigated by every future reader.

- [ ] **Step 8: Run them to verify they pass**

```sh
cargo test -p retrace-box --test threads sem_ -- --test-threads=1
cargo test -p retrace-box -- --test-threads=1
```

Expected: PASS both.

- [ ] **Step 9: Handle the worker's park — opcode `0x4`, measured by Task 1**

Task 1 measured something nothing could measure before a worker ran, and it changes this task's
scope. When the worker finishes its block it calls `workq_kernreturn(0x4, 0, 0, 0)` to park, and
libpthread `brk`s with `"BUG IN LIBPTHREAD: __workq_kernreturn returned"` **immediately after the
call**. See §5 of `docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-wqthread-measurements.md`.

Without this, `guest_workq_kernreturn`'s `other =>` arm refuses `0x4` by value and kills the run
right after the worker's useful work — so the gate cannot go green.

Two constraints, and the mechanism between them is yours:

1. **The parked worker must end up non-runnable**, so `pick_next()` returns main and the guest can
   finish. A parked workqueue thread is *alive* — the real kernel can hand it more work — so
   modelling it as `Exited` is a poorer fit than a `Blocked` variant: `Exited` carries a return
   value a parked worker does not have, and `live()` would start lying. Stage 2b's scope excludes
   thread reuse, so nothing will wake it; that is fine and expected.
2. **It must not be left resumable into a return from `workq_kernreturn`.** This is the sharp edge.
   The existing dispatch arms call `b.set_x0_err_and_return(rc, false)` unconditionally, which
   advances the saved PC past the `svc` — leaving a context that, if anything ever resumed it, would
   return from the call libpthread `brk`s on. `bsdthread_terminate`'s arm shows the established
   shape for a syscall that does not return: it deliberately omits that call and documents why.

**This needs NO new dispatch arm and no `retrace-core` change.** `guest_workq_kernreturn` is already
mirrored on both sides by Stage 2a, so a new opcode arm inside it is symmetric by construction — the
same property that let Task 2 change REQTHREADS without touching the core. If you find yourself
editing `retrace-core`, stop and re-read this paragraph.

Write a test in `crates/retrace-box/tests/threads.rs` asserting that after a park the worker is not
pickable and `pick_next()` returns main. Then run:

```sh
cargo test -p retrace-box --test threads -- --test-threads=1
```

- [ ] **Step 10: Commit**

```bash
git add crates/retrace-box/src/thread.rs crates/retrace-box/src/lib.rs crates/retrace-box/tests/threads.rs
git commit -m "M18 Stage 2b t3: a park/wake seam keyed on a port name, and the worker's own park"
```

---

### Task 4: Both dispatch loops, both mirrors, and the two oracle sites

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` — record arms (near `:899`), replay mirrors (near `:1903`)
- Modify: `crates/retrace/tests/dispatch_e2e.rs` — one assertion (see Step 4)

**The constants and the guard are NOT yours** — Task 2 landed both, because a built worker reaches
the semaphore trap and a trap with neither an arm nor a guard hangs the recorder. Use
`retrace_arch::MACH_SEMAPHORE_WAIT` / `MACH_SEMAPHORE_SIGNAL` as they already exist; do not
redefine them, and do not remove the guard — your arms sit *before* it, which is what makes it
stop firing.

**Interfaces:**
- Consumes: `Box_::guest_sem_wait` / `Box_::guest_sem_signal` from Task 3; §3 (verified trap
  numbers) of Task 1's document.
- Produces: nothing later tasks call directly. Task 5 runs the resulting binary.

**This is the task the spec names as the one that bites.** Two requirements are structural and
nothing in the compiler enforces either:

1. **The `verify_thread` census goes 7 → 9.** There are exactly 7 sites today
   (`crates/retrace-core/src/lib.rs` lines 1271, 1381, 1412, 1439, 1472, 1555, 2143). CLAUDE.md:
   "every new mirror silently creates a new hole until its oracle call is added — nothing structural
   couples the two." Each new mirror `return`s before the generic dispatch, so each needs its own
   call, **placed after that arm's own field comparison** so a genuine argument divergence still
   reports as itself.
2. **New arms go BEFORE the generic negative-trap arm** at `:531`. An arm placed after it is dead
   code that compiles, passes clippy, and silently forwards.

- [ ] **Step 1: Add the record arms**

In `record_box`, immediately **before** the generic negative-trap arm at `:531`. Model them on the
`SYS_ULOCK_WAIT` / `SYS_ULOCK_WAKE` arms at `:899` and `:911`:

```rust
            // M18 Stage 2b: the mach semaphore wait (see Box_::guest_sem_wait). Never forwarded —
            // it would block retrace's OWN process on a semaphore nothing there will ever signal.
            // Writes nothing to guest memory (it only moves thread-table state), so the event
            // carries no writes. `err` is hardcoded false: guest_sem_wait has no failure path.
            Stop::Syscall { num, args } if num == retrace_arch::MACH_SEMAPHORE_WAIT => {
                let rc = b.guest_sem_wait(args);
                w.append(&Event::Syscall { num, args, ret: rc, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append sem_wait: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);
            }
            // M18 Stage 2b: the wake half (see Box_::guest_sem_signal). Never forwarded, same
            // reason. The woken tids drive M17's materialisation, exactly as the ulock wake's do.
            Stop::Syscall { num, args } if num == retrace_arch::MACH_SEMAPHORE_SIGNAL => {
                let (rc, woken) = b.guest_sem_signal(args);
                w.append(&Event::Syscall { num, args, ret: rc, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append sem_signal: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);
                let _ = &woken; // see Step 5
            }
```

- [ ] **Step 2: Decide the signal arm's materialisation, and write down which you chose**

M17's `SYS_ULOCK_WAKE` arm at `:911` does more than wake: it materialises a signal pended on a thread
that could not run, with a `deliver_to.len() <= 1` bound and a
`complete_saved_syscall_before_delivery` on the woken thread's saved context. Read that arm in full
before writing this one.

The semaphore wake is the same *shape* of landmark. **Whether it needs the same materialisation
depends on whether any fixture can pend a signal on a semaphore-blocked thread**, and none in this
tree does today. Choose one and justify it in a comment at the arm:

- **Mirror M17's materialisation.** Symmetric with the ulock wake; costs code no fixture exercises.
- **Assert instead**: `assert!(woken.iter().all(|&t| b.threads().peek_deliverable(t).is_none()), ...)`
  — fails loud if a fixture ever pends a signal on a semaphore waiter, rather than silently
  stranding it.

**The one thing you may not do is drop `woken` silently.** M17's `assert_no_stranded_signals` skips
anything not `Blocked(_)`, so a signal pended on a thread this arm wakes would vanish while record
and replay agreed with each other — the one failure class a determinism oracle cannot see.

- [ ] **Step 3: Add the replay mirrors, each with its oracle site**

In `ReplaySession::advance`, beside the `SYS_ULOCK_WAIT` / `SYS_ULOCK_WAKE` mirrors at `:1903` and
`:1931`. Both must call the same `Box_` method with the same args — that identity is what makes
symmetry rule 1 hold by construction.

```rust
                            // M18 Stage 2b: the record arms' mirrors (symmetry rule 1). Replay must
                            // call the SAME Box_ method with IDENTICAL args so both sides move the
                            // thread table identically; the recorded (num, args) byte-compare above
                            // IS the divergence check.
                            if num == retrace_arch::MACH_SEMAPHORE_WAIT {
                                let (rc, rerr) = (self.b.guest_sem_wait(args), false);
                                // M18 Stage 2b: the thread oracle (see `verify_thread`'s doc).
                                // Site 8 of 9. Placed AFTER this arm's own field comparison so a
                                // genuine argument divergence still reports as itself — and needed
                                // at all because this arm `return`s before the generic dispatch,
                                // which is where the check would otherwise happen.
                                self.verify_thread(*rthread, pc)?;
                                /* ...the recorded-vs-recomputed comparison and return, in the
                                   exact shape the ULOCK_WAIT mirror uses... */
                            }
                            if num == retrace_arch::MACH_SEMAPHORE_SIGNAL {
                                let ((rc, woken), rerr) = (self.b.guest_sem_signal(args), false);
                                // Site 9 of 9. Same placement rule as site 8.
                                self.verify_thread(*rthread, pc)?;
                                /* ...same shape as the ULOCK_WAKE mirror, including whatever
                                   Step 5 decided the record arm does with `woken` — the two sides
                                   must agree on that too, or replay diverges from record on a
                                   path the byte-compare cannot see... */
                            }
```

Copy the surrounding mirrors' exact comparison-and-return structure rather than inventing one; they
already handle the recorded-`ret`/recomputed-`rc` compare and the `err` handling.

- [ ] **Step 4: Update the companion test — the guard is now unreachable for this guest**

Task 2 pointed `the_workqueue_syscalls_are_emulated_not_forwarded` at the guard's message. Your arms
sit before the guard, so the semaphore traps are now serviced and that message no longer appears:
the test is red again, for the third and second-to-last time.

Run the guest and read what actually happens now — the worker runs, main wakes, and the run reaches
either a clean exit or the next unmeasured wall (most likely a `workq_kernreturn` park opcode, which
the `other =>` arm refuses **by value** and therefore names for you). Retarget that one assertion at
what you observe, and **keep the two durable tripwires verbatim** as Task 2 did.

Do not attempt to make the headline `#[ignore]`d gate pass here. Task 5 owns the gate's final state
and both of its honest outcomes.

- [ ] **Step 5: Verify the census is 9, by counting**

```sh
grep -c 'self\.verify_thread(' crates/retrace-core/src/lib.rs
```

Expected: `9`. If it prints 7 or 8, a mirror is missing its oracle call and the hole is silent —
CLAUDE.md's count is the thing to check when adding an arm, and this is that check.

- [ ] **Step 6: Run the workspace chunks**

```sh
cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1
cargo test -p retrace-box --no-fail-fast -- --test-threads=1
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS all three. The `retrace` e2e targets are Task 5's.

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace/tests/dispatch_e2e.rs
git commit -m "M18 Stage 2b t4: the semaphore seam in both dispatch loops, with sites 8 and 9"
```

---

### Task 5: The gate, in exactly two honest states, and the two documents

**Files:**
- Modify: `crates/retrace/tests/dispatch_e2e.rs`
- Modify: `README.md` — "What works today", "Known limits", gate stamp
- Modify: `docs/status-log.md` — append a new section, never rewrite an old one

**Interfaces:**
- Consumes: everything Tasks 1–4 produced.
- Produces: the milestone's closing state.

- [ ] **Step 1: Run the headline gate and find out which state you are in**

```sh
cargo test -p retrace --test dispatch_e2e -- --test-threads=1 --nocapture --ignored
```

Capture the full output to a file first (`2>&1 | tee`), then read it — and remember the exit code is
lost through a pipe, so capture it separately if you need it. Two outcomes:

- **It passes.** The worker ran, main observed the signal, and both replays are byte-identical.
- **It fails.** Read the failure carefully and identify the wall. Likely candidates, all of which
  fail loud by construction: an unmeasured `workq_kernreturn` park opcode (named by value by the
  `other =>` arm Task 2 kept), the guard Task 2 landed, or a deadlock assert.

- [ ] **Step 2a: If it passed — un-park the gate**

Delete the entire `#[ignore = "..."]` attribute from `a_dispatch_async_guest_records_and_replays`.
That is the whole change: the body is already correct and already asserts the right things
(`worker\n`, `done\n`, exit 0, two byte-identical replays), and it asserts on what the guest
*printed* rather than on an exit code, for the reason its own comment gives.

- [ ] **Step 2b: If it failed — re-park it at the wall you actually reached**

**Rewrite** the `#[ignore]` reason. Do not leave Stage 2a's text in place — CLAUDE.md designates the
`#[ignore]` reason the *primary record* of a parked wall, and a stale one is worse than none. The new
text must name: what Stage 2b changed (the worker is built, the seam exists), the exact wall reached
with the value that names it, and what a future stage owes. Claim for the run only what the run
showed — Stage 2a's t11 needed a fix round for exactly this class of over-claim.

- [ ] **Step 3: Rewrite Stage 2a's companion test — do not delete it**

`the_workqueue_syscalls_are_emulated_not_forwarded` asserts
`rec.stderr.contains("worker construction is Stage 2b")`, which Task 2 removed. It is now red.

Rewrite it to assert the new reality. **Keep both of its other assertions verbatim** — the
`assert_ne!(rec.code, 139)` pre-Stage-2a signature and the `_pthread_wqthread` tripwire. That
tripwire ("no host workqueue thread may exist inside the recorder") is the one thing the file exists
to forbid, and it outlives the wall it was written beside. Rename the test if its name no longer
describes what it checks.

- [ ] **Step 4: Run the full gate, chunked, and count**

```sh
cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1
cargo test -p retrace-box --no-fail-fast -- --test-threads=1
for t in $(ls crates/retrace/tests/*.rs | xargs -n1 basename | sed 's/\.rs$//' | grep -v '^util$'); do
  cargo test -p retrace --test "$t" --no-fail-fast -- --test-threads=1
done
cargo test -p retrace --bins --no-fail-fast -- --test-threads=1
cargo clippy --workspace --all-targets -- -D warnings
```

Then reconcile the total against Stage 2a's close (**420 passed / 0 failed / 2 ignored across 104
test binaries**, measured at `67e9a13`) by diffing `#[test]` counts file-by-file — do not trust a
sum. Account for every delta: Task 2 deleted one test and added three; Task 3 added four plus two;
Task 5 changed the ignored count by one if the gate un-parked.

- [ ] **Step 5: Edit the README in place**

"What works today" gains rung 5 if the gate went green. "Known limits" — whose *first* entry is
currently the GCD/libdispatch limitation — is rewritten to describe the new reality, not the old one.
Restamp the gate line with the numbers Step 4 measured and the commit they were measured at.

The README is the current-state document and is **edited in place**; it never carries a "superseded"
note.

- [ ] **Step 6: Append to `docs/status-log.md`**

A **new** section. Never rewrite an old one — the log is append-only, and an earlier claim that later
proved wrong is left standing with a forward pointer rather than quietly corrected.

Include, following the Stage 2a section's shape: what landed, the measurement Task 1 produced
(especially whether §3 confirmed or corrected the `-36`/`-33` attribution, and the §4 struct-init
verdict), the `verify_thread` census moving 7 → 9 and why these mirrors needed sites when Stage 2a's
did not, the gate numbers, and a "boundaries and non-changes" list carrying forward what is still
unfixed.

- [ ] **Step 7: Commit**

```bash
git add crates/retrace/tests/dispatch_e2e.rs README.md docs/status-log.md
git commit -m "M18 Stage 2b t5: the gate in its honest state, and the two documents"
```

---

## Self-Review

**Spec coverage.** Every spec section maps to a task: worker construction → Task 2; the seam → Task 3;
arms, mirrors, oracle sites and guard → Task 4; determinism posture → enforced by Task 2's
determinism test and Task 4's same-method-same-args mirrors; fail-loud boundaries → Task 2 Step 5 and
Task 2's retained `other =>` arm; "two things that will bite" → Task 4's preamble (census) and Task 5
Step 3 (companion test); exit criterion → Task 5 Steps 2a/2b; testing → Tasks 2, 3, 5; the five open
questions → Task 1 Steps 2–5. Risk 1 (struct-init) has an explicit BLOCKED route at Task 2's head;
risks 2–5 land in Task 1 §3 and Task 5 Step 1.

**Known incompleteness, stated rather than hidden.** Three code blocks carry an explicit
`<implementer: ...>` marker — the register-seeding block and `place_worker_stack` in Task 2, and the
two trap-number constants in Task 4. These are **not** placeholders in the sense the writing-plans
skill forbids; they are values that do not exist yet and cannot be written down without measuring
them, which is precisely what Task 1 exists to do and what the spec forbids guessing. Each names the
document section that supplies it. Writing plausible values here would be the exact failure mode
M14's plan hit when it guessed the `_pthread_start` register shape and was wrong.

**Type consistency.** `guest_sem_wait -> u64` and `guest_sem_signal -> (u64, Vec<usize>)` are used
with those shapes in Task 3's tests and Task 4's arms and mirrors. `unblock_sem_waiters_on(u64) ->
Vec<usize>` matches `unblock_waiters_on`. `BlockReason::Sem { port: u64 }` is spelled identically in
the variant, the tests, and both box methods. `guest_workq_reqthreads` is private and reached only
through `guest_workq_kernreturn`, which both dispatch arms already call — so no arm changes for it,
which is the point.
