# M18-workq Stage 2a Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop forwarding `workq_open` (367) and `workq_kernreturn` (368) to the host kernel — which today creates a real worker thread *inside the recorder* and kills it — by emulating them in the box, and measure the wall that appears behind them.

**Architecture:** Two new `Box_` methods join the `bsdthread_*`/`ulock_*` family. `guest_workq_open` returns 0 and asserts the guest has registered. `guest_workq_kernreturn` dispatches on `args[0]`: the measured `0x400` (dispatch setup) returns 0, the measured `0x20` (request threads) is a deliberate named wall because worker construction is Stage 2b, and any other opcode asserts by value. Both get record arms and replay mirrors calling the same method with the same arguments (symmetry rule 1), plus a guard on the generic forward arm so no later edit can silently re-forward them. Nothing enters the trace that was not already there and `TRACE_MAGIC` does not move.

**Tech Stack:** Rust 1.95.0, `aarch64-apple-darwin`, Hypervisor.framework, macOS 26 on Apple Silicon.

**Spec:** `docs/superpowers/specs/2026-08-20-retrace-m18-workq-design.md` — read the section **"Stage 2, split by what is measured"** at the end; it is what this plan implements. The sections above it are M18 as a whole and include a paragraph about `verify_thread` that the Stage 2a section explicitly corrects.

## Global Constraints

- **`--test-threads=1` is mandatory** on every `cargo test` invocation. HVF allows one VM per process; a bare `cargo test` flakes with `HV_BUSY`.
- **`just gate` does not complete as one command** — it exceeds the 10-minute tool ceiling and gets killed. Chunk it, run every chunk `--no-fail-fast`, and capture cargo's exit code **before any pipe**. Do not omit the `cargo test -p retrace --bins` chunk: `--test <name>` selects integration targets only, so the unit tests inside the `retrace` binary run in no other chunk. `cargo test -p retrace --lib` is invalid for this crate and fails loudly.
- **Grep gate logs with `grep -a`** — they carry ANSI and UTF-8 that trips plain grep.
- **`clippy.toml` denials are load-bearing, not style:** no `Instant::now`/`SystemTime::now` (determinism), no `std::thread::Thread` (retrace's core is single-threaded by design). Clippy runs `-D warnings`.
- **Determinism posture for everything in this plan is standard and symmetric** (the M2-setport posture): record appends the landmark, replay recomputes it with the *same* `Box_` method and the *same* arguments and byte-compares. No new `Event` variant. `TRACE_MAGIC` stays `RT\x00\x08`.
- **The oracle census stays at seven `verify_thread` sites.** This was measured, not assumed — see Task 3 Step 4. Do not add `verify_thread` calls in this plan, and do not edit `CLAUDE.md`'s census.

---

## Scope of THIS plan

Stage 2a only. **In:** the two `Box_` methods, both dispatch-loop arms, both replay mirrors, the forward-path guard, unit tests, the Stage 2b measurement, the re-parked headline gate plus a new gate for what actually landed, and the two documents.

**Out — all Stage 2b:** worker-thread construction, the worker-park `BlockReason`, the wake seam, the wrong-thread divergence test, and a green `dispatch_e2e`. If you find yourself building a thread, you have left this plan.

---

### Task 1: `guest_workq_open` — the guest's workqueue is the guest's

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (add the method next to `guest_bsdthread_register`, around line 3332)
- Test: `crates/retrace-box/tests/threads.rs` (append; the file's VM-backed tests share `fn tb()` at line 139)

**Interfaces:**
- Consumes: `Box_::wq_thread_pc() -> Option<u64>` (exists, `lib.rs:3340`), `Box_::guest_bsdthread_register(&mut self, args: [u64; 8]) -> u64` (exists, `lib.rs:3332`).
- Produces: `pub fn guest_workq_open(&mut self, args: [u64; 8]) -> u64` on `Box_`. Task 3's two dispatch arms call it.

- [ ] **Step 1: Write the failing tests**

Append to `crates/retrace-box/tests/threads.rs`:

```rust
/// M18 Stage 2a: `workq_open` is EMULATED, never forwarded. Forwarding it brings up a real kernel
/// workqueue for RETRACE's own process, which is half of what makes the pair whole-process fatal
/// (Task 6's crash report: a host worker enters `start_wqthread` and jumps to address 0).
#[test]
fn workq_open_returns_success_once_the_guest_has_registered() {
    let mut b = tb();   // see `fn tb()` at the top of this file
    // The registration is the precondition: it is what captures `wqthread`, which Stage 2b enters.
    b.guest_bsdthread_register([0x1111, 0x2222, 0x3333, 0, 0, 0, 0, 0]);
    assert_eq!(b.guest_workq_open([0, 0, 0, 0, 0, 0, 0, 0]), 0,
        "workq_open must report success — libdispatch treats a failure as no workqueue at all");
}

/// The fail-loud half. A `workq_open` with no registered `wqthread` means the guest took a path no
/// measurement covers — the same posture `guest_bsdthread_create`'s `thread_start_pc.expect(...)`
/// takes, and for the same reason: refusing to invent a thread entry point.
#[test]
#[should_panic(expected = "workq_open before bsdthread_register")]
fn workq_open_before_registration_fails_loud() {
    let mut b = tb();
    b.guest_workq_open([0, 0, 0, 0, 0, 0, 0, 0]);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
cargo test -p retrace-box --test threads workq_open -- --test-threads=1
```
Expected: FAIL — `no method named guest_workq_open found for struct Box_`.

- [ ] **Step 3: Write the implementation**

In `crates/retrace-box/src/lib.rs`, immediately after `pub fn pthread_size(&self) -> Option<u32>` (around line 3341):

```rust
    /// `workq_open()` — the guest asks the kernel to bring up its workqueue.
    ///
    /// **Emulated, never forwarded.** Forwarding it brings up a real kernel workqueue for
    /// RETRACE's own process; combined with the `REQTHREADS` that follows, the host then creates a
    /// real worker thread inside the recorder and enters it at `start_wqthread`, which jumps
    /// through a dispatch function pointer that is NULL in this process and dies at address 0.
    /// That is measured, not theorised — M18 Task 6 caught it in a crash report
    /// (`.superpowers/sdd/2026-08-20-retrace-m18-workq/stage2-measurements.md` §3).
    ///
    /// This is the fourth instance of one recurring bug: the guest's fds were retrace's fds (M10),
    /// the guest's signal dispositions were retrace's (M11), the guest's pthread registration was
    /// retrace's (M18 Stage 1), and the guest's workqueue was retrace's (here).
    ///
    /// Returns 0. libdispatch reads a failure as "no workqueue at all", which would put it straight
    /// back on the path Stage 1 existed to clear.
    ///
    /// **There is deliberately no "open before kernreturn" assert.** The measured order is
    /// `kernreturn(0x400)` -> `open` -> `kernreturn(0x20)`: the first `workq_kernreturn` fires
    /// BEFORE `workq_open`. A plausible-looking ordering assert would fire on the real sequence.
    pub fn guest_workq_open(&mut self, _args: [u64; 8]) -> u64 {
        assert!(self.wq_thread_pc.is_some(),
            "M18 Stage 2a: workq_open before bsdthread_register — refusing to bring up a workqueue \
             with no registered wqthread entry point. Every dynamic guest registers one at startup \
             (measured, M14 Task 2), so this means the guest took a path no measurement covers.");
        0
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
cargo test -p retrace-box --test threads workq_open -- --test-threads=1
```
Expected: `2 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/threads.rs
git commit -m "M18 t7: guest_workq_open — the guest's workqueue is the guest's"
```

---

### Task 2: `guest_workq_kernreturn` — two measured opcodes, and a wall with a name

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (immediately after `guest_workq_open` from Task 1)
- Test: `crates/retrace-box/tests/threads.rs` (append)

**Interfaces:**
- Consumes: `fn tb()` (test helper, `threads.rs:139`), `Box_::guest_bsdthread_register` (exists).
- Produces: `pub fn guest_workq_kernreturn(&mut self, args: [u64; 8]) -> u64` on `Box_`. Task 3's two dispatch arms call it. Also two `pub const` opcode names used only inside `retrace-box` and its tests.

**The opcodes are the measurement.** From `stage2-measurements.md` §2, verbatim and in the order they fired:

```
[trap] num=368 (0x170) pc=0x1804af9f0 args=[0x400,0x27ff6a8,0x18,0x0,0x0,0x20]
[trap] num=367 (0x16f) pc=0x1804afa1c args=[0x0,0x27ff6a8,0x18,0x0,0x0,0x20]
[trap] num=368 (0x170) pc=0x1804af9f0 args=[0x20,0x0,0x1,0x40008ff,0x0,0x20]
```

The names `WQOPS_SETUP_DISPATCH` / `WQOPS_QUEUE_REQTHREADS` are attributed from XNU's public libpthread sources and are **NOT verified on this machine** — `pthread/workqueue_private.h` ships in neither `/usr/include` nor the Xcode SDK. Use the names in comments as a lead; the raw values are the measurement.

- [ ] **Step 1: Write the failing tests**

Append to `crates/retrace-box/tests/threads.rs`:

```rust
/// M18 Stage 2a: the `0x400` opcode — libdispatch configuring the workqueue for dispatch. It
/// carries a guest pointer in `args[1]` that Stage 2b will need; Stage 2a only has to not forward
/// it. Measured as the FIRST of the three workqueue traps, before `workq_open`.
#[test]
fn workq_kernreturn_setup_dispatch_succeeds() {
    let mut b = tb();
    b.guest_bsdthread_register([0x1111, 0x2222, 0x3333, 0, 0, 0, 0, 0]);
    // The measured args vector, verbatim from stage2-measurements.md §2.
    let rc = b.guest_workq_kernreturn([0x400, 0x27ff6a8, 0x18, 0x0, 0x0, 0x20, 0, 0]);
    assert_eq!(rc, 0, "setup must report success or libdispatch abandons the workqueue");
}

/// The deliberate, self-imposed Stage 2a wall. `REQTHREADS` is where a worker would be built, and
/// worker construction is Stage 2b — so this refuses BY NAME rather than returning a success the
/// guest would then wait forever on. Refusing here is strictly better than the behaviour it
/// replaces, which was handing the syscall to the host kernel and having the host spawn a real
/// thread inside the recorder.
#[test]
#[should_panic(expected = "worker construction is Stage 2b")]
fn workq_kernreturn_reqthreads_is_the_named_stage_2a_wall() {
    let mut b = tb();
    b.guest_bsdthread_register([0x1111, 0x2222, 0x3333, 0, 0, 0, 0, 0]);
    // The measured args vector, verbatim. args[3]=0x40008ff looks like a packed priority/QoS word
    // and is Stage 2b's to decode.
    b.guest_workq_kernreturn([0x20, 0x0, 0x1, 0x40008ff, 0x0, 0x20, 0, 0]);
}

/// The `guest_ulock_wake` posture: an operation word nobody measured is refused BY VALUE, so the
/// panic tells the next reader exactly what to go measure. Asserting that the message names the
/// value is the point of the test — a panic that just said "unsupported" would be useless.
#[test]
#[should_panic(expected = "0xbeef")]
fn workq_kernreturn_refuses_an_unmeasured_opcode_by_value() {
    let mut b = tb();
    b.guest_bsdthread_register([0x1111, 0x2222, 0x3333, 0, 0, 0, 0, 0]);
    b.guest_workq_kernreturn([0xbeef, 0, 0, 0, 0, 0, 0, 0]);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
cargo test -p retrace-box --test threads workq_kernreturn -- --test-threads=1
```
Expected: FAIL — `no method named guest_workq_kernreturn found for struct Box_`.

- [ ] **Step 3: Write the implementation**

In `crates/retrace-box/src/lib.rs`, immediately after `guest_workq_open`:

```rust
    /// `workq_kernreturn(op, arg2, arg3, arg4)` — the workqueue's whole control surface.
    ///
    /// **Emulated, never forwarded**, for the reason `guest_workq_open` documents. Dispatches on
    /// `args[0]`, the opcode, and takes the `guest_ulock_wake` fail-loud posture
    /// (`Box_::guest_ulock_wake`): every operation word this box has not measured is refused BY
    /// VALUE, so the panic names what to go measure.
    ///
    /// The two opcodes below are the ONLY ones any guest has ever reached — measured M18 Task 6,
    /// `.superpowers/sdd/2026-08-20-retrace-m18-workq/stage2-measurements.md` §2. That list is a
    /// floor, not a ceiling: the park/return opcodes a RUNNING worker issues cannot be enumerated
    /// until Stage 2b makes a worker run, which is precisely why this refuses rather than guesses.
    ///
    /// The XNU names in the constants are attributed from public libpthread sources and are NOT
    /// verified on this machine — `pthread/workqueue_private.h` ships in neither `/usr/include` nor
    /// the Xcode SDK. The raw values are the measurement; the names are a lead.
    pub fn guest_workq_kernreturn(&mut self, args: [u64; 8]) -> u64 {
        /// libdispatch configuring the workqueue for dispatch. Carries a guest pointer in `args[1]`
        /// (measured `0x27ff6a8`) that Stage 2b needs and Stage 2a only has to not forward.
        const WQOPS_SETUP_DISPATCH: u64 = 0x400;
        /// libdispatch asking for worker threads. Stage 2b's entry point; Stage 2a's wall.
        const WQOPS_QUEUE_REQTHREADS: u64 = 0x20;

        match args[0] {
            WQOPS_SETUP_DISPATCH => 0,
            WQOPS_QUEUE_REQTHREADS => panic!(
                "M18 Stage 2a: workq_kernreturn REQTHREADS ({:#x}) reached — worker construction \
                 is Stage 2b. This is a DELIBERATE wall, not a defect: the kernel allocates the \
                 stack and the pthread struct for a workqueue thread and enters `wqthread` with a \
                 register contract that is still unmeasured, so building one here would be \
                 invention. args={args:#x?}", args[0]),
            other => panic!(
                "M18 Stage 2a: unmeasured workq_kernreturn opcode {other:#x} — only \
                 SETUP_DISPATCH ({WQOPS_SETUP_DISPATCH:#x}) and REQTHREADS \
                 ({WQOPS_QUEUE_REQTHREADS:#x}) have ever been observed (M18 Task 6). Measure what \
                 issues this one before modelling it; a guessed opcode silently corrupts the \
                 guest's workqueue state. args={args:#x?}"),
        }
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
cargo test -p retrace-box --test threads workq_ -- --test-threads=1
```
Expected: `5 passed` (Task 1's two plus these three).

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/threads.rs
git commit -m "M18 t8: guest_workq_kernreturn — two measured opcodes, and a wall with a name"
```

---

### Task 3: Both dispatch loops, both mirrors, and the guard that keeps them there

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` — record arms after the `SYS_BSDTHREAD_REGISTER` arm (ends line 823); replay mirrors after the `SYS_BSDTHREAD_REGISTER` mirror (ends line 1789); the forward guard in the generic `Stop::Syscall` arm (near the `SYS_DUP2` assert, line 979)

**Interfaces:**
- Consumes: `Box_::guest_workq_open` (Task 1), `Box_::guest_workq_kernreturn` (Task 2), `retrace_arch::SYS_WORKQ_OPEN` = 367 and `SYS_WORKQ_KERNRETURN` = 368 (both already exist, `retrace-arch/src/lib.rs:210,213`).
- Produces: nothing later tasks call. Task 4 depends on these arms existing.

- [ ] **Step 1: Add the two record arms**

In `crates/retrace-core/src/lib.rs`, immediately after the `SYS_BSDTHREAD_REGISTER` arm (which ends at line 823 with `b.set_x0_err_and_return(rc, false);` and its closing brace), add:

```rust
            // M18 Stage 2a: workq_open is EMULATED, never forwarded (see Box_::guest_workq_open).
            // Forwarding brings up a real kernel workqueue for retrace's own process, which with
            // the REQTHREADS below makes the host create a worker thread INSIDE the recorder —
            // measured in a crash report, M18 Task 6.
            //
            // `writes` is empty and that is deliberate: the call writes no guest memory, and its
            // return is a constant the replay mirror recomputes identically. The byte-compare
            // there IS the oracle (symmetry rule 1).
            Stop::Syscall { num, args } if num == retrace_arch::SYS_WORKQ_OPEN => {
                let rc = b.guest_workq_open(args);
                w.append(&Event::Syscall { num, args, ret: rc, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append workq_open: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);
            }
            // M18 Stage 2a: workq_kernreturn is EMULATED, never forwarded — same reason. Note this
            // arm may PANIC by design: REQTHREADS is Stage 2a's deliberate named wall, so the
            // recorder stops here rather than handing the syscall to the host kernel.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_WORKQ_KERNRETURN => {
                let rc = b.guest_workq_kernreturn(args);
                w.append(&Event::Syscall { num, args, ret: rc, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append workq_kernreturn: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);
            }
```

- [ ] **Step 2: Add the two replay mirrors**

In the same file, immediately after the `SYS_BSDTHREAD_REGISTER` mirror (which ends at line 1789 with `return self.finish_event();` and its closing brace), add:

```rust
                            // M18 Stage 2a: the record arms' mirrors (symmetry rule 1). Same
                            // method, same args, so both sides compute the identical return; the
                            // byte-compare below is the divergence check. Placed HERE, with the
                            // other `if num ==` mirrors, deliberately: this arm already called
                            // `verify_thread` above (line ~1520, before the whole chain), so these
                            // inherit the thread oracle and must NOT add their own. See the spec's
                            // "Stage 2, split by what is measured" for the measurement behind that.
                            if num == retrace_arch::SYS_WORKQ_OPEN {
                                let rc = self.b.guest_workq_open(args);
                                if rc != *ret {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("workq_open rc mismatch: replay {rc:#x} != recorded {ret:#x}") });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
                            if num == retrace_arch::SYS_WORKQ_KERNRETURN {
                                let rc = self.b.guest_workq_kernreturn(args);
                                if rc != *ret {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("workq_kernreturn rc mismatch: replay {rc:#x} != recorded {ret:#x}") });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
```

**Note for the implementer, and do not "fix" it:** these mirrors are unreachable by any test in Stage 2a, because record dies at `REQTHREADS` before a trace containing a workq landmark is ever completed and replayed. They are correct by construction (symmetry rule 1: same method, same args) and become exercised in Stage 2b. Say so in your report rather than inventing a test that fabricates a trace.

- [ ] **Step 3: Add the forward-path guard**

In the generic `Stop::Syscall { num, args }` arm, immediately after the existing `SYS_DUP2` assert (line ~979, which ends with `implement target-slot allocation before a guest uses it");`), add:

```rust
                // M18 Stage 2a: the workqueue pair must never reach here. Forwarding them is not
                // merely wrong but whole-process fatal for the RECORDER: the host kernel brings up
                // a workqueue for retrace's own process and then creates a real worker thread in
                // it, entering `start_wqthread` -> `_pthread_wqthread`, which jumps through a
                // dispatch function pointer that is NULL in this process and dies at address 0.
                // Measured in a crash report, M18 Task 6 (stage2-measurements.md §3). The arms
                // above service both; this assert is what stops a later edit from removing one and
                // silently restoring the hazard — the same shape as the dup2 guard above.
                assert!(num != retrace_arch::SYS_WORKQ_OPEN && num != retrace_arch::SYS_WORKQ_KERNRETURN,
                    "workq syscall {num} reached the generic forward arm — it must be emulated \
                     above (M18 Stage 2a). Forwarding it creates a real host worker thread inside \
                     the recorder and takes a SIGSEGV at address 0.");
```

- [ ] **Step 4: Verify the oracle census did NOT move**

This is a real verification step, not a formality — the spec's correction rests on it.

```sh
grep -c "self.verify_thread(" crates/retrace-core/src/lib.rs
```
Expected: **7**, unchanged. If it is 9, you added `verify_thread` calls to the two new mirrors — remove them. They sit inside the generic `Event::Syscall` arm, which already verified the thread at line ~1520 before the `if num ==` chain begins, so an added call is a redundant check on a covered path and teaches the next reader a rule that is not the real one.

Also confirm `CLAUDE.md` was not edited:
```sh
git diff --name-only HEAD -- CLAUDE.md
```
Expected: empty output.

- [ ] **Step 5: Build and run the crates this touches**

```sh
cargo test -p retrace-core -- --test-threads=1
cargo test -p retrace-box --test threads -- --test-threads=1
cargo clippy -p retrace-core -p retrace-box --all-targets -- -D warnings
```
Expected: all pass, clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-core/src/lib.rs
git commit -m "M18 t9: 367/368 stop being forwarded; both arms, both mirrors, and the guard"
```

---

### Task 4: Measure what is behind the wall — the input to Stage 2b

**Files:**
- Create: `docs/superpowers/specs/2026-08-21-retrace-m18-stage2b-measurements.md`
  (this plan originally named `.superpowers/sdd/2026-08-20-retrace-m18-workq/stage2b-measurements.md`;
  Task 5 relocated it out of the gitignored `.superpowers/` tree, where it had to be `git add -f`ed)
- Temporarily modify (and **revert**, never commit): `crates/retrace-box/src/lib.rs`

**Interfaces:**
- Consumes: Tasks 1-3.
- Produces: the measurement document. Nothing in code.

This is a measurement task. Its output is a document; **no code from this task lands.**

- [ ] **Step 1: Locate the guest binary**

```sh
G=$(ls -t target/aarch64-apple-darwin/debug/build/retrace-guest-*/out/dispatch_dyn 2>/dev/null | head -1)
echo "$G"
```
If that is empty, build it first with `cargo build -p retrace-guest` and re-run. The path constant is `retrace_guest::DISPATCH_DYN`.

- [ ] **Step 2: Apply the throwaway permissive stub**

Change ONLY the `WQOPS_QUEUE_REQTHREADS` arm of `guest_workq_kernreturn` from the `panic!` to `0`:

```rust
            // TEMPORARY MEASUREMENT STUB — Task 4 only, reverted in Step 5. Never commit this.
            WQOPS_QUEUE_REQTHREADS => 0,
```

- [ ] **Step 3: Run the guest under a timeout, with the streams SEPARATED**

**The timeout is mandatory, not caution.** With no worker created, main has nothing to wake it. If `dispatch_semaphore_wait` lowers to a forwarded trap retrace has no arm for, the vCPU thread blocks in the host kernel forever. A hang reports nothing; the timeout converts it into an observation.

Do **not** merge stdout and stderr — Task 6 did, and it interleaved guest stdout into the trace and made the tail unreadable.

```sh
G=$(ls -t target/aarch64-apple-darwin/debug/build/retrace-guest-*/out/dispatch_dyn | head -1)
RETRACE_TRACE=1 timeout 120 cargo run -q -p retrace -- record-dyn "$G" -o /tmp/m18-2a.bin \
  > /tmp/m18-2a.out 2> /tmp/m18-2a.err
echo "EXIT=$?"      # 124 means the timeout fired — that is itself a finding
grep -ac '^\[trap\]' /tmp/m18-2a.err
tail -60 /tmp/m18-2a.err
```

- [ ] **Step 4: Record the findings verbatim**

Write `docs/superpowers/specs/2026-08-21-retrace-m18-stage2b-measurements.md` containing, each quoted exactly rather than summarised:

1. The exit code, and whether the timeout fired (124).
2. The dispatched trap count, stated **without** attributing run-to-run variance to anything — Task 6's §4 made exactly that error and the spec withdraws it. If you want to claim instability means something, you must first show it exceeds the variance dyld guests already have from 18 forwarded `gettimeofday` and 2 `getentropy` calls.
3. **The `mach_msg2` at `pc=0x1804adc34`** — its full `args` vector and, if `RETRACE_TRACE=1`'s decoder emitted one, the decoded `[mach_msg2]` line. This is the headline: Task 6 could only see it truncated.
4. Every trap after that one, in order, to the end.
5. **What `dispatch_semaphore_wait` lowers to** — a `__ulock_wait` (515), a mach semaphore trap, or a `mach_msg2` RPC. Grep for it: `grep -an 'num=515\|num=-3[0-9]\|semaphore' /tmp/m18-2a.err`. This decides whether Stage 2b's park/wake seam can reuse M14's address-equality correlation or needs something new, so it is the single most valuable thing this task produces.
6. Any `workq_kernreturn` opcode beyond `0x400`/`0x20` that appears — grep `grep -aoE 'num=368 .*args=\[0x[0-9a-f]+' /tmp/m18-2a.err | sort -u`.
7. The final line of the log, and whether it is a `RECORD ERROR`, a panic, a clean exit, or a truncation.

- [ ] **Step 5: Revert the stub and prove it is gone**

```sh
git checkout -- crates/retrace-box/src/lib.rs
git diff --stat            # must show NO change to crates/retrace-box/src/lib.rs
grep -n "TEMPORARY MEASUREMENT STUB" crates/retrace-box/src/lib.rs   # must print nothing
cargo test -p retrace-box --test threads workq_ -- --test-threads=1  # 5 passed, wall restored
```

- [ ] **Step 6: Commit the measurement only**

```bash
git add docs/superpowers/specs/2026-08-21-retrace-m18-stage2b-measurements.md
git commit -m "M18 t10: measure what is behind the Stage 2a wall"
```

---

### Task 5: The gate, in exactly two honest states, and the two documents

**Files:**
- Modify: `crates/retrace/tests/dispatch_e2e.rs`, `README.md`, `docs/status-log.md`

**Interfaces:**
- Consumes: Tasks 1-4, and `util::record_dynamic(guest: &str) -> (RunOut, PathBuf)` where `RunOut { code: i32, stdout: Vec<u8>, stderr: String }` (`crates/retrace/tests/util/mod.rs:45,71`).
- Produces: the closing state of Stage 2a.

- [ ] **Step 1: Add the gate for what actually landed**

The headline test stays parked. This new one runs green and asserts **the difference Stage 2a makes**. Append to `crates/retrace/tests/dispatch_e2e.rs`:

```rust
/// M18 Stage 2a's own gate — NOT ignored, and it asserts the one thing Stage 2a changed.
///
/// Before Stage 2a, `workq_open`/`workq_kernreturn` fell to the generic forward arm and the HOST
/// kernel acted on retrace's own process: it created a real workqueue worker thread inside the
/// recorder, entered it at `start_wqthread` -> `_pthread_wqthread`, and died at address 0. The
/// record run exited 139 from RETRACE's own SIGSEGV, having written no guest stdout at all.
///
/// After Stage 2a both syscalls are emulated in the box, and the run stops at retrace's own named
/// REQTHREADS wall instead — deterministically, in its own process, on its own terms.
///
/// **The assertion is on the message, not the exit code.** `crashy_e2e` asserts 139 for an uncaught
/// GUEST fault, so an exit code alone cannot tell "retrace SIGSEGV'd" apart from "the guest
/// faulted" — the honest-gate rule this repo learned from `segv_rust_e2e`. The panic text can only
/// appear if the guest's `workq_kernreturn` reached retrace's own emulation.
#[test]
fn the_workqueue_syscalls_are_emulated_not_forwarded() {
    let (rec, _trace) = util::record_dynamic(retrace_guest::DISPATCH_DYN);

    assert!(rec.stderr.contains("worker construction is Stage 2b"),
        "the record run must stop at retrace's OWN named REQTHREADS wall, which is only reachable \
         if workq_kernreturn was emulated rather than forwarded; stderr:\n{}", rec.stderr);
    // The pre-2a signature, named so a regression is legible rather than just red. 139 is SIGSEGV;
    // this is a supporting check, not the assertion above.
    assert_ne!(rec.code, 139,
        "exit 139 is the pre-Stage-2a signature: retrace itself took a SIGSEGV on a host workqueue \
         worker thread. stderr:\n{}", rec.stderr);
    // Nothing from the recorder's own crash path may appear — that path is what Stage 2a removed.
    assert!(!rec.stderr.contains("_pthread_wqthread"),
        "no host workqueue thread may exist inside the recorder; stderr:\n{}", rec.stderr);
}
```

- [ ] **Step 2: Re-park the headline gate on the wall that is real now**

Replace the entire `#[ignore = "..."]` attribute on `a_dispatch_async_guest_records_and_replays` with a reason describing the **Stage 2a** wall. Delete the stale Stage-1/forwarding reason completely — do not append to it. The body stays exactly as written; only the reason changes. The new reason must state:

- that 367/368 are now emulated in the box and no longer forwarded, so the host-thread hazard is gone;
- that the wall is now retrace's own **deliberate** `REQTHREADS` assert, because worker construction is Stage 2b;
- what Task 4 measured behind it (cite `2026-08-21-retrace-m18-stage2b-measurements.md` and name the `mach_msg2` finding and what `dispatch_semaphore_wait` lowers to);
- that un-parking needs a worker thread built and entered at the registered `wqthread`.

- [ ] **Step 3: Verify the gate is in exactly two honest states**

```sh
cargo test -p retrace --test dispatch_e2e -- --test-threads=1
```
Expected: `1 passed; 1 ignored` — never a silent skip, never a pass the assertions did not earn.

Then confirm the new test can actually fail: temporarily change `"worker construction is Stage 2b"` to `"this string is not in the output"`, re-run, confirm FAIL, and change it back. A test that cannot fail is what the honest-gate discipline exists to prevent.

- [ ] **Step 4: Run the gate, chunked**

Per the Global Constraints. Capture cargo's exit code **before any pipe**:

```sh
cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1
cargo test -p retrace-box --no-fail-fast -- --test-threads=1
cargo test -p retrace --bins --no-fail-fast -- --test-threads=1
# then each integration target in crates/retrace/tests/ individually:
cargo test -p retrace --test dispatch_e2e --no-fail-fast -- --test-threads=1
# ...and the rest; `ls crates/retrace/tests/*.rs` is the list
cargo clippy --workspace --all-targets -- -D warnings
```

Reconcile by diffing `#[test]` counts file-by-file rather than trusting a sum. **The baseline is HEAD, not M17's close** — M17 closed at 412/0/1, but M18 Stage 1 has landed since, so counted at this plan's start the tree has **416 `#[test]` and exactly 2 live `#[ignore]` attributes**:

```sh
grep -rn '#\[test\]' crates --include='*.rs' | wc -l     # 416 before this plan, 422 after
grep -rn '^\s*#\[ignore' crates --include='*.rs'          # exactly 2, both below
```

The two parks are `crates/retrace/tests/stackoverflow_rust_e2e.rs:10` (M8 spec risk R3, untouched by this plan) and `crates/retrace/tests/dispatch_e2e.rs:8` (M18's headline, whose reason Step 2 rewrites). Note that grepping for `#[ignore` **without** anchoring matches a dozen prose mentions in comments; anchor it or you will miscount.

Stage 2a adds 6 tests: 2 in Task 1 and 3 in Task 2 (`retrace-box/tests/threads.rs`), 1 in Task 5 (`retrace/tests/dispatch_e2e.rs`). Expect **2 ignored** at the close — unchanged, because Stage 2a parks nothing new and un-parks nothing.

- [ ] **Step 5: Update the two documents, each doing its own job**

**README** — edited in place, it says what is true *now*. Update the "Known limits" GCD/workqueue bullet: `workq_open`/`workq_kernreturn` are now emulated rather than forwarded, the recorder no longer creates a host worker thread, and what remains is worker construction. Restamp the gate line only if you have a full measured gate run from Step 4; otherwise leave the stamp and say so in your report.

**`docs/status-log.md`** — append-only, never rewrite an earlier section. Append an M18 Stage 2a section covering: the two emulated syscalls and the measured opcodes; that forwarding them was demonstrated whole-process fatal; the deliberate REQTHREADS wall and why a named refusal beats a guessed success; **the two corrections this stage made** (Task 6's §4 trap-count attribution withdrawn, and the `verify_thread` census measured to stay at seven rather than growing to nine); and what Task 4 measured.

Do not restate either in `CLAUDE.md`. **The oracle census in `CLAUDE.md` is unchanged by this plan** — say so explicitly in your report so a later reader does not go looking for new sites.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace/tests/dispatch_e2e.rs README.md docs/status-log.md
git commit -m "M18 t11: the workqueue pair is the guest's; re-park the gate on the wall that is real now"
```

---

## Self-Review

**Spec coverage.** Every element of the spec's Stage 2a section maps to a task: `guest_workq_open` and its registration assert plus the deliberate absence of an ordering assert → Task 1. `guest_workq_kernreturn`, the two measured opcodes, and the fail-loud posture → Task 2. Both record arms, both replay mirrors, symmetry rule 1, and the forward-path guard → Task 3. The census-stays-at-seven measurement → Task 3 Step 4, as an executable check rather than a claim. The Stage 2b measurement and its mandatory timeout → Task 4. The exit criterion, the two unavailable assertions named so they are not reached for, and the documentation split → Task 5.

**Deliberately NOT covered, and tracked:** everything in the spec's "Stage 2b — the worker runs". Spec risks 2-5 belong there. Risk 6 (the stub may hang) is discharged by Task 4 Step 3's timeout; risk 7 (the self-imposed wall could mask a nearer real wall) is discharged by Task 4 Step 3 running the guest *past* `0x20` before the wall is placed.

**Placeholder scan.** No TBD/TODO. Every code step carries real code. Task 4 is a measurement task whose deliverable is a document, and its seven required findings are enumerated rather than left to judgement. Task 5 Step 2 specifies the four things the new `#[ignore]` reason must state rather than pre-writing prose that would be stale before it was read.

**Type consistency.** `guest_workq_open(&mut self, args: [u64; 8]) -> u64` and `guest_workq_kernreturn(&mut self, args: [u64; 8]) -> u64` are defined in Tasks 1-2 and called with exactly those signatures in Task 3's four sites. `SYS_WORKQ_OPEN`/`SYS_WORKQ_KERNRETURN` already exist in `retrace-arch` (Stage 1 Task 2) and are used, not redefined. `util::record_dynamic` and `RunOut`'s three fields match `crates/retrace/tests/util/mod.rs:45,71`. `fn tb()` is the existing helper at `threads.rs:139`, not a new one.

**One risk this plan carries and cannot retire.** Task 3's two replay mirrors are unreachable by any Stage 2a test, because record never completes a trace containing a workq landmark. They are correct by construction under symmetry rule 1 and become exercised in Stage 2b. Task 3 Step 2 says this explicitly and instructs the implementer to report it rather than fabricate a trace to test them — an invented trace would test the mirror against itself.
