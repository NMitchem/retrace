# M18-workq Stage 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the guest its own pthread registration so libdispatch gets a feature word instead of a `brk`, and measure what the guest does next.

**Architecture:** `bsdthread_register` (366) stops being forwarded to retrace's own already-registered host process and becomes emulated in the box, joining `bsdthread_create`/`bsdthread_terminate`/`ulock_*`. It captures `threadstart`, `wqthread` and `pthsize`, and returns a fixed synthesized feature word chosen to satisfy the four bit-tests measured in libpthread and libdispatch. Determinism posture is standard and symmetric — a constant is trivially identical on both sides, so record appends and replay recomputes and byte-compares. Nothing enters the trace that was not already there, and `TRACE_MAGIC` does not move.

**Tech Stack:** Rust 1.95.0, `aarch64-apple-darwin`, Hypervisor.framework, macOS 26 on Apple Silicon.

**Spec:** `docs/superpowers/specs/2026-08-20-retrace-m18-workq-design.md`

## Scope of THIS plan

The spec describes two stages. **This plan is Stage 1 plus the Stage-2 measurement.** Stage 2 (`workq_open`/`workq_kernreturn` emulation, the worker-park `BlockReason`, worker-thread construction) gets its own plan, written after Task 6 measures the interface — because no guest has ever reached those syscalls, and a plan containing invented code for an unmeasured kernel interface would be fiction rather than a plan.

Stage 1 is independently valuable and independently verifiable: it moves a measured wall, and it closes a latent whole-process hazard that has been live since M14.

## Global Constraints

Copied verbatim from the spec and `CLAUDE.md`; every task's requirements implicitly include these.

- **The synthesized feature word is `0x4000005E`.** It must satisfy all four measured gates: `w >= 1`; `(w & 0x4000001E) == 0x4000001E` (libpthread `__pthread_init` `+0x1048`); bit 4 set (libdispatch `0x180348F68`); bit 6 set with **bit 7 clear** (libdispatch `0x180348F90`/`0x180348F94`), which keeps `_dispatch_workloop_worker_thread` out of the picture entirely.
- **`--test-threads=1` is mandatory.** HVF allows one VM per process; a bare `cargo test` flakes with `HV_BUSY`.
- **Determinism posture: standard and symmetric.** Record and replay call the *same* `Box_` method with the *same* arguments; replay byte-compares. No new `Event` variant, no `TRACE_MAGIC` bump. Any landmark that cannot be recomputed is a format break and a re-plan, not a quiet addition.
- **Symmetry rule 1:** a special case in record's `match stop` needs a mirror in replay's dispatch, both recomputing identical values. Record arms live in `record_box`, replay mirrors in `ReplaySession::advance`, both in `crates/retrace-core/src/lib.rs` — **not** in `retrace-box`, which owns only the `Box_` methods those arms call.
- **The oracle census is 7 `verify_thread` call sites plus `mirror_delivery`'s eighth inline check** (`CLAUDE.md:224-230`). Every new replay mirror that `return`s before the generic dispatch silently creates a new hole until its oracle call is added. Stage 1 adds **no** new early-returning mirror at top level — its mirror lives *inside* the generic recorded-syscall block, which already calls `verify_thread` at `crates/retrace-core/src/lib.rs:1518` before reaching it. **Do not add an eighth site for it, and do not change the census number.**
- **Honest-gate discipline:** a headline gate is parked `#[ignore]`d at the current wall with the wall documented on the test itself, never faked green or deleted. Assert on the difference the work makes, never on an exit code a weaker failure would also produce.
- **Never reimplement Apple's PAC**, never execute from a writable guest page, never `hv_vm_map` a file-backed page.
- Guest binaries built by `crates/retrace-guest/build.rs`; a test that spawns the CLI must codesign it by hand (`crates/retrace/tests/util/mod.rs::bin()`).

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/retrace-guest/c/dispatch_dyn.c` | the rung-5 guest: one `dispatch_async` onto a global queue | create |
| `crates/retrace-guest/build.rs` | compile it with the `hello_dyn` recipe; export `DISPATCH_DYN` | modify |
| `crates/retrace-guest/src/lib.rs` | path constant for the new guest | modify |
| `crates/retrace/tests/dispatch_e2e.rs` | the headline gate, parked at the measured wall then moved forward | create |
| `crates/retrace-arch/src/lib.rs` | `SYS_WORKQ_OPEN` (367), `SYS_WORKQ_KERNRETURN` (368) + pinning test | modify |
| `crates/retrace-box/src/lib.rs` | `WORKQ_FEATURE_WORD`, `wq_thread_pc`/`pthread_size` fields, `guest_bsdthread_register` | modify |
| `crates/retrace-core/src/lib.rs` | record arm + replay mirror: stop forwarding 366 | modify |
| `README.md`, `docs/status-log.md`, `CLAUDE.md` | current state, history, invariants | modify |

---

### Task 1: The rung-5 guest, and the gate parked at the measured wall

Adds the guest and pins today's failure as an honest, `#[ignore]`d gate. Nothing about retrace's behaviour changes in this task — that is deliberate, so the gate's `#[ignore]` reason records the *pre-fix* wall and Task 6 can move it forward.

**Files:**
- Create: `crates/retrace-guest/c/dispatch_dyn.c`
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs`
- Create: `crates/retrace/tests/dispatch_e2e.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `retrace_guest::DISPATCH_DYN: &str` — absolute path to the built guest binary, same shape as the existing `HELLO_DYN`.

- [ ] **Step 1: Write the guest**

Create `crates/retrace-guest/c/dispatch_dyn.c`:

```c
// M18 rung 5: the smallest guest that forces libdispatch's global-queue worker pool.
// dispatch_async onto the global concurrent queue makes libdispatch bring up its root queues,
// which is the path that asks the kernel for a workqueue. The semaphore keeps main alive until
// the block has run, so the worker's write is always in the trace.
#include <dispatch/dispatch.h>
#include <unistd.h>

int main(void) {
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    dispatch_queue_t q = dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0);
    dispatch_async(q, ^{
        write(1, "worker\n", 7);
        dispatch_semaphore_signal(sem);
    });
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    write(1, "done\n", 5);
    return 0;
}
```

- [ ] **Step 2: Add the build rule**

In `crates/retrace-guest/build.rs`, after the `argv_echo` block, add — the same recipe as `hello_dyn` (`build.rs:169-172`), plain `-arch arm64`, real toolchain, links libSystem:

```rust
    // dispatch_dyn: M18's headline — a real dynamic guest that uses libdispatch. Same recipe as
    // hello_dyn (real toolchain, links libSystem, plain -arch arm64); blocks need no extra flags
    // because libSystem carries the blocks runtime.
    let src = format!("{}/c/dispatch_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/dispatch_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang dispatch_dyn");
    assert!(status.success(), "dispatch_dyn guest build failed");
```

- [ ] **Step 3: Export the path constant**

In `crates/retrace-guest/src/lib.rs`, beside the existing `HELLO_DYN` constant, add one in the identical style:

```rust
/// M18 rung 5: a dynamically-linked C guest that `dispatch_async`es onto a global queue.
pub const DISPATCH_DYN: &str = concat!(env!("OUT_DIR"), "/dispatch_dyn");
```

- [ ] **Step 4: Verify the guest builds and runs natively**

Run:
```sh
cargo build -p retrace-guest 2>&1 | tail -3
find target -name dispatch_dyn -type f -perm +111 | head -1 | xargs -I{} {}
```
Expected: prints `worker` then `done`, exit 0. If it does not, the guest is wrong and nothing downstream is meaningful — stop and report.

- [ ] **Step 5: Measure the current wall, and the host's `bsdthread_register` return**

This is the measurement the spec's Risk 1 requires. Add a **temporary** `eprintln!` in the record arm at `crates/retrace-core/src/lib.rs:815-820`, immediately after `forward_and_diff`:

```rust
                eprintln!("[M18 MEASURE] bsdthread_register args={args:x?} ret={ret:#x} err={err}");
```

Run:
```sh
RETRACE_TRACE=1 cargo run -p retrace -- record-dyn <path-to-dispatch_dyn> -o /tmp/m18.bin 2>&1 | tail -40
```

Record in the task report, verbatim: the `[M18 MEASURE]` line (the full `args` vector and the returned value), and the final `RECORD ERROR:` line. Then **remove the `eprintln!`** — it was a measurement, not a feature. Confirm with `git diff crates/retrace-core/src/lib.rs` showing no change.

The expected wall, which the probe already measured and which you are confirming still holds:
`non-syscall exit: exception (EC=0x3c ISS=0xb001 FSC=0x1) pc=0x1804f5f20` — a `BRK` in `_pthread_workqueue_supported.cold.1`.

- [ ] **Step 6: Write the gate, parked at that wall**

Create `crates/retrace/tests/dispatch_e2e.rs`. Copy the record/replay harness shape from `crates/retrace/tests/thread_rust_e2e.rs` (read it first — it is the closest sibling). The `#[ignore]` reason must state the wall **as measured in Step 5**, not as quoted from this plan:

```rust
//! M18 rung 5: a guest that dispatch_asyncs onto a global concurrent queue.
//!
//! Parked at the Stage-1 wall. See the `#[ignore]` reason for what stops it today.

mod util;

#[test]
#[ignore = "M18 Stage 1: libdispatch dies before any workqueue syscall fires. \
            _dispatch_root_queues_init_once calls _pthread_workqueue_supported, which traps at \
            .cold.1 because __pthread_supported_features is 0 — libpthread stores that word only \
            when bsdthread_register returns >= 1, and retrace forwards that call to its own \
            already-registered host process. Measured: BRK (EC=0x3c) at pc=0x1804f5f20, 245 traps \
            in, with workq_open/workq_kernreturn never fired. Un-park when the guest reaches its \
            worker."]
fn a_dispatch_async_guest_records_and_replays() {
    // A REAL body that genuinely fails at the wall — the `stackoverflow_rust_e2e` pattern. Parking
    // is then one attribute, and un-parking is deleting one line rather than writing a test. A
    // body that asserts nothing would be a test that cannot fail, which is what the honest-gate
    // discipline exists to prevent.
    //
    // Record the guest through real dyld, replay it, and replay it again. Copy the harness calls
    // verbatim from `thread_rust_e2e.rs` — same `util::bin()` codesigning, same record/replay/
    // double-replay shape — and assert the run reaches a clean exit with the worker's output
    // present. Today it fails at the BRK named in the #[ignore] reason above.
}
```

Write the body by reading `crates/retrace/tests/thread_rust_e2e.rs` and following it. Do **not** invent a new harness shape.

- [ ] **Step 7: Verify the gate is parked, not passing**

Run: `cargo test -p retrace --test dispatch_e2e -- --test-threads=1`
Expected: `0 passed; 0 failed; 1 ignored`.

- [ ] **Step 8: Commit**

```bash
git add crates/retrace-guest/c/dispatch_dyn.c crates/retrace-guest/build.rs \
        crates/retrace-guest/src/lib.rs crates/retrace/tests/dispatch_e2e.rs
git commit -m "M18 t1: the rung-5 guest, and the gate parked at the measured wall"
```

---

### Task 2: Syscall constants for the workqueue pair

Mechanical and self-contained. `retrace-arch` is zero-dependency arch facts; these two numbers do not exist there today.

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `retrace_arch::SYS_WORKQ_OPEN: u64 = 367`, `retrace_arch::SYS_WORKQ_KERNRETURN: u64 = 368`.

- [ ] **Step 1: Extend the existing pinning test first**

`crates/retrace-arch/src/lib.rs` already has `thread_syscall_numbers_are_the_darwin_ones` (around line 616), which cross-checks the family against the SDK. Add the new pair to that same test rather than writing a second one — the test exists precisely to catch a wrong syscall number sitting unexercised, which is exactly the risk here:

```rust
        // M18: the workqueue pair. Both are SDK values (`MacOSX.sdk/usr/include/sys/syscall.h`).
        // M14 measured BOTH as firing zero times from a pthread guest; M18's probe measured them
        // as STILL firing zero times from a libdispatch guest, because libdispatch dies before it
        // reaches them. They are pinned here before they have ever fired, so the number is right
        // the first time one does.
        assert_eq!((SYS_WORKQ_OPEN, SYS_WORKQ_KERNRETURN), (367, 368));
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p retrace-arch thread_syscall_numbers -- --test-threads=1`
Expected: FAIL — `cannot find value SYS_WORKQ_OPEN in this scope`.

- [ ] **Step 3: Add the constants**

Beside `SYS_BSDTHREAD_REGISTER` (line 206), matching the surrounding doc-comment style:

```rust
/// `workq_open()` — brings up the process's kernel workqueue. Has NEVER fired: M14 measured zero
/// from a pthread guest, M18's probe measured zero from a libdispatch guest, because libdispatch
/// dies asking whether workqueues exist before it uses one. Pinned before first fire.
pub const SYS_WORKQ_OPEN: u64 = 367;
/// `workq_kernreturn(options, item, affinity, prio)` — the workqueue's whole control surface: a
/// worker parks, returns, and is dispatched through it. Never fired; see `SYS_WORKQ_OPEN`.
pub const SYS_WORKQ_KERNRETURN: u64 = 368;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p retrace-arch -- --test-threads=1`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-arch/src/lib.rs
git commit -m "M18 t2: pin workq_open/workq_kernreturn before either has ever fired"
```

---

### Task 3: The feature word, and the four gates it must satisfy

**Files:**
- Modify: `crates/retrace-box/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `retrace_box::WORKQ_FEATURE_WORD: u32 = 0x4000_005E`.

- [ ] **Step 1: Write the failing test**

In `crates/retrace-box/src/lib.rs`'s test module. The test asserts the **four measured bit-tests**, not the literal — the constant's entire job is to satisfy gates read out of disassembly, so a test that just re-states `0x4000005E` would pass even if the value were wrong for its purpose:

```rust
    #[test]
    fn the_feature_word_satisfies_every_gate_it_was_measured_against() {
        let w = WORKQ_FEATURE_WORD;
        // libpthread __pthread_init (+0x1040): `cmp w0,#1 / b.lt skip` — a value < 1 means the
        // feature word is never stored at all, which is the M18 Stage-1 bug.
        assert!(w >= 1, "must be >= 1 or libpthread skips the store");
        // libpthread __pthread_init (+0x1048): `mov w8,#0x1e / movk w8,#0x4000,lsl#16 /
        // bics wzr,w8,w0 / b.ne crash` — every bit of 0x4000001E must be present.
        assert_eq!(w & 0x4000_001E, 0x4000_001E, "libpthread requires all of 0x4000001E");
        // libdispatch _dispatch_root_queues_init_once (0x180348F68): `tbz w0,#4 -> .cold.5`.
        assert_ne!(w & (1 << 4), 0, "bit 4 clear crashes libdispatch at .cold.5");
        // libdispatch (0x180348F90/F94): bit 7 set registers THREE worker callbacks including the
        // workloop worker; bit 7 clear with bit 6 set registers two. Neither set is .cold.4.
        // Bit 7 is deliberately clear — it is the scope lever, not an accident.
        assert_ne!(w & (1 << 6), 0, "bit 6 must be set when bit 7 is clear, else .cold.4");
        assert_eq!(w & (1 << 7), 0, "bit 7 must stay CLEAR to keep the workloop worker out of scope");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p retrace-box the_feature_word -- --test-threads=1`
Expected: FAIL — `cannot find value WORKQ_FEATURE_WORD in this scope`.

- [ ] **Step 3: Add the constant**

Near the other guest-facing constants at the top of `crates/retrace-box/src/lib.rs`:

```rust
/// The `bsdthread_register` feature word retrace reports to the guest.
///
/// **Synthesized, never the host's.** The host is retrace's own already-registered process, so its
/// answer describes retrace, not the guest — the same category error as M10's fd table and M11's
/// signal dispositions. A fixed constant is also what makes this deterministic for free: both runs
/// compute the identical value with nothing recorded (symmetry rule 2's argument, applied to a
/// return value rather than an instruction).
///
/// The value is not arbitrary. It is the SMALLEST word satisfying every gate measured in the
/// shipped binaries (see the test beside this constant for the four, each with its address).
/// Bit 7 is deliberately CLEAR: with it set, libdispatch additionally registers
/// `_dispatch_workloop_worker_thread`, and the workloop path is out of M18's scope.
pub const WORKQ_FEATURE_WORD: u32 = 0x4000_005E;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p retrace-box the_feature_word -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/lib.rs
git commit -m "M18 t3: the synthesized feature word and the four gates it was measured against"
```

---

### Task 4: `guest_bsdthread_register`

**Files:**
- Modify: `crates/retrace-box/src/lib.rs`

**Interfaces:**
- Consumes: `WORKQ_FEATURE_WORD` (Task 3).
- Produces: `Box_::guest_bsdthread_register(&mut self, args: [u64; 8]) -> u64`, returning the feature word widened to `u64`. Also `Box_::wq_thread_pc(&self) -> Option<u64>` and `Box_::pthread_size(&self) -> Option<u32>` for Stage 2. Replaces all uses of `Box_::set_thread_start_pc` in `retrace-core`; that setter is removed.

- [ ] **Step 1: Write the failing test**

In `crates/retrace-box/tests/threads.rs`, beside that file's other VM-backed tests, using its existing `tb()` helper (`crates/retrace-box/tests/threads.rs:139-142`). **`Box_::for_test()` does not exist — do not invent it.** Every `Box_` in this repo is built by `Box_::load(&loaded)`, which is what `tb()` wraps; the test therefore needs a VM and runs under the mandatory `--test-threads=1`:

```rust
    #[test]
    fn bsdthread_register_captures_all_three_and_returns_the_feature_word() {
        // args per the Darwin signature: (threadstart, wqthread, pthsize, …). The arch crate's own
        // doc comment on SYS_BSDTHREAD_REGISTER names them in this order.
        let mut b = tb();   // see `fn tb()` at the top of this file
        let rc = b.guest_bsdthread_register([0x1111, 0x2222, 0x3333, 0, 0, 0, 0, 0]);
        assert_eq!(rc, WORKQ_FEATURE_WORD as u64, "the guest must get the synthesized word");
        assert_eq!(b.thread_start_pc(), Some(0x1111), "threadstart still captured (M14's need)");
        assert_eq!(b.wq_thread_pc(), Some(0x2222), "wqthread captured — Stage 2 enters here");
        assert_eq!(b.pthread_size(), Some(0x3333), "pthsize captured — Stage 2 allocates this");
    }
```

`Box_::thread_start_pc()` already exists (`crates/retrace-box/src/lib.rs:3278`), so that assertion needs nothing new. The two new getters come from Step 3.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p retrace-box bsdthread_register_captures -- --test-threads=1`
Expected: FAIL — no method `guest_bsdthread_register`.

- [ ] **Step 3: Add the fields and the method**

Add fields beside the existing `thread_start_pc` on `Box_` (**do not reorder existing struct fields** — `vcpu` must stay declared before `vm`):

```rust
    /// M18: the workqueue thread entry point from `bsdthread_register`'s `args[1]`. The address the
    /// kernel enters when it hands a worker thread to userspace. Captured in Stage 1, entered in
    /// Stage 2.
    wq_thread_pc: Option<u64>,
    /// M18: the guest's pthread struct size from `bsdthread_register`'s `args[2]`. The kernel — not
    /// the guest — allocates a workqueue thread's pthread struct, so Stage 2 needs this size.
    pthread_size: Option<u32>,
```

Then, replacing `set_thread_start_pc` (`crates/retrace-box/src/lib.rs:3283`):

```rust
    /// `bsdthread_register(threadstart, wqthread, pthsize, …)`, emulated.
    ///
    /// **Never forwarded, as of M18.** Two independent reasons, and the second is the one that
    /// makes this urgent rather than tidy:
    ///
    /// 1. The host is retrace's OWN process, which registered its own libpthread at startup. Its
    ///    answer describes retrace, not the guest, so the guest's `__pthread_supported_features`
    ///    never gets set and the first `dispatch_async` trips a `brk`. Fourth instance of one
    ///    recurring bug: the guest's fds were retrace's (M10), its dispositions were retrace's
    ///    (M11), its pthread registration was retrace's (here).
    /// 2. `args[0]`/`args[1]` are thread ENTRY POINTS. Forwarding this call hands GUEST addresses
    ///    to the host kernel as **retrace's own** process's thread-start functions — the same
    ///    whole-process-fatal class as forwarding `bsdthread_create`. It has been harmless only
    ///    because it fails. Latent since M14; closed here.
    pub fn guest_bsdthread_register(&mut self, args: [u64; 8]) -> u64 {
        self.thread_start_pc = Some(args[0]);
        self.wq_thread_pc = Some(args[1]);
        self.pthread_size = Some(args[2] as u32);
        WORKQ_FEATURE_WORD as u64
    }

    /// M18 Stage 2 reads these; Stage 1 only captures them.
    pub fn wq_thread_pc(&self) -> Option<u64> { self.wq_thread_pc }
    pub fn pthread_size(&self) -> Option<u32> { self.pthread_size }
```

Initialise both new fields to `None` wherever `Box_` is constructed and wherever `thread_start_pc` is initialised.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p retrace-box -- --test-threads=1`
Expected: all pass. If `set_thread_start_pc` had other callers, the crate will not compile until Task 5 updates them — in that case leave `set_thread_start_pc` in place for this task and delete it in Task 5, and say so in the task report.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/lib.rs
git commit -m "M18 t4: guest_bsdthread_register — the guest's registration is the guest's"
```

---

### Task 5: Stop forwarding 366 in both dispatch loops

The symmetry-rule-1 task. Both arms must call the same method with the same arguments.

**Files:**
- Modify: `crates/retrace-core/src/lib.rs:815-820` (record), `crates/retrace-core/src/lib.rs:1780-1784` (replay)

**Interfaces:**
- Consumes: `Box_::guest_bsdthread_register` (Task 4).
- Produces: nothing new; changes 366 from forwarded to emulated on both sides.

- [ ] **Step 1: Replace the record arm**

At `crates/retrace-core/src/lib.rs:815-820`, replace the whole arm. Note it no longer calls `forward_and_diff`, so it records `err: false` and `writes: vec![]` — exactly the shape of its `bsdthread_create` neighbour below it:

```rust
            // M18 t5: bsdthread_register is EMULATED, never forwarded (see
            // Box_::guest_bsdthread_register for both reasons — the guest's registration is the
            // guest's, AND forwarding hands guest addresses to the host kernel as retrace's own
            // thread-start functions).
            //
            // `writes` is empty and that is deliberate: the call writes no guest memory, and its
            // return is a compile-time constant that the replay mirror recomputes identically.
            // The byte-compare there IS the oracle (symmetry rule 1).
            Stop::Syscall { num, args } if num == retrace_arch::SYS_BSDTHREAD_REGISTER => {
                let rc = b.guest_bsdthread_register(args);
                w.append(&Event::Syscall { num, args, ret: rc, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append bsdthread_register: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);
            }
```

- [ ] **Step 2: Replace the replay mirror**

At `crates/retrace-core/src/lib.rs:1780-1784`, inside the generic recorded-`Event::Syscall` block. **This block already called `verify_thread` at line 1518** before reaching here — do not add another call, and do not change the census in `CLAUDE.md`:

```rust
                            // M18 t5: the record arm's mirror (symmetry rule 1). Same method, same
                            // args, so both sides capture the identical three addresses and compute
                            // the identical return. The byte-compare below is the divergence check;
                            // it is vacuous while the return is a constant and becomes the oracle
                            // the moment it is not — the same shape as bsdthread_create's mirror.
                            if num == retrace_arch::SYS_BSDTHREAD_REGISTER {
                                let rc = self.b.guest_bsdthread_register(args);
                                if rc != *ret {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("bsdthread_register rc mismatch: replay {rc:#x} != recorded {ret:#x}") });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
```

- [ ] **Step 3: Delete `set_thread_start_pc` if it is now unused**

Run: `grep -rn 'set_thread_start_pc' crates/`
If the only hits are its definition, delete it (`crates/retrace-box/src/lib.rs:3283`). A setter with no callers is dead weight that the next reader has to rule out.

- [ ] **Step 4: Run the threading gates — the ones most likely to break**

**This is the step that matters most in this task.** Returning a *successful* feature word changes libpthread's initialisation path for **every dynamic guest**, not just the dispatch one: `__pthread_init` continues past the store into branches it has never taken under retrace. Existing gates are the detector.

Run each, capturing exit codes before any pipe:
```sh
cargo test -p retrace --test thread_rust_e2e   -- --test-threads=1
cargo test -p retrace --test sigthread_e2e     -- --test-threads=1
cargo test -p retrace --test thread_watch_e2e  -- --test-threads=1
cargo test -p retrace --test hello_dyn_e2e     -- --test-threads=1
cargo test -p retrace --test hello_rust_e2e    -- --test-threads=1
```
Expected: all pass.

**If any of these regress, STOP and report it as a BLOCKED status — do not attempt a fix.** A regression here means the synthesized word sends libpthread somewhere retrace cannot follow, which is a spec-level finding (the constant may need different bits, or the change may need to be conditional), not an implementation bug. Include the failing gate, the divergence or wall, and the guest.

- [ ] **Step 5: Run the rest of the workspace**

```sh
cargo test --workspace --exclude retrace-box --exclude retrace -- --test-threads=1
cargo test -p retrace-box -- --test-threads=1
cargo test -p retrace --bins -- --test-threads=1
```
Expected: all pass. (The `--bins` chunk is the one whose omission silently costs 8 tests — do not skip it.)

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace-box/src/lib.rs
git commit -m "M18 t5: stop forwarding bsdthread_register; the guest gets its own feature word"
```

---

### Task 6: Measure what the guest does next, and re-park the gate honestly

The Stage-2 measurement. Its output is the input to the Stage-2 plan.

**Files:**
- Modify: `crates/retrace/tests/dispatch_e2e.rs`, `README.md`, `docs/status-log.md`
- Create: `.superpowers/sdd/2026-08-20-retrace-m18-workq/stage2-measurements.md`

**Interfaces:**
- Consumes: everything above.
- Produces: the measured `workq_kernreturn` opcode sequence and worker entry contract, for the Stage-2 plan.

- [ ] **Step 1: Re-run the guest and capture the new wall**

```sh
RETRACE_TRACE=1 cargo run -p retrace -- record-dyn <path-to-dispatch_dyn> -o /tmp/m18.bin > /tmp/m18-trace.log 2>&1
tail -40 /tmp/m18-trace.log
```

Record verbatim in `stage2-measurements.md`:
- the total trap count (`grep -ac '^\[trap\]' /tmp/m18-trace.log`) against Stage 1's **245** (the probe's own log; a same-config re-run measured 241 — argv differs, so a small delta is expected and is not a divergence) — the number that shows the wall moved;
- whether `num=367` and `num=368` appear at all (`grep -anE 'num=(367|368)' /tmp/m18-trace.log`), which answers the spec's open question 4;
- for every `num=368`, the full `args` vector — `args[0]` is the opcode, and the **set of distinct opcodes is what Stage 2 must implement**;
- the final line of the log, whether it is a `RECORD ERROR`, a panic, or a clean exit.

- [ ] **Step 2: If the guest now runs to completion, write the real gate**

Only if Step 1 ended cleanly. Assert **on the difference M18 makes** — per the honest-gate rule, never on an exit code a weaker failure would also produce. The load-bearing assertion is that the block ran on a thread that is *not* main, which no pre-M18 build can produce:

```rust
    // The whole claim: the worker's write is in the trace, tagged to a thread that is not main.
    let worker_writes: Vec<_> = events.iter()
        .filter(|e| matches!(e, Event::Syscall { num, args, .. }
                    if *num == retrace_arch::SYS_WRITE && args[0] == 1))
        .collect();
    assert!(worker_writes.iter().any(|e| matches!(e, Event::Syscall { thread, .. } if *thread != 0)),
            "the dispatched block must run on a worker thread, not on main");
```
Read `crates/retrace/tests/thread_watch_e2e.rs` for how it reaches the trace's events and how it names threads, and follow it — including its double-replay.

- [ ] **Step 3: If the guest hits a new wall, re-park the gate on that wall**

Rewrite the `#[ignore]` reason to describe the **new** wall as measured in Step 1, and delete the stale Stage-1 reason. It must name the syscall or PC reached, and say what is missing. The body stays as written in Task 1 — it is a real test that really fails, and only the reason changes.

A milestone that parks a *new* gate for a capability it does not yet have has regressed nothing; that is the discipline working.

- [ ] **Step 4: Verify the gate is in exactly one honest state**

Run: `cargo test -p retrace --test dispatch_e2e -- --test-threads=1`
Expected: either `1 passed` (Step 2 path, `#[ignore]` deleted) or `1 ignored` (Step 3 path). Never a silent skip, and never a pass that the assertions did not earn — if it passes, confirm by temporarily inverting the thread assertion that it can still fail.

- [ ] **Step 5: Update the two documents, each doing its own job**

**README** (edited in place — it says what is true *now*): update the "Known limits" GCD bullet to state what Stage 1 changed and what remains. If the rung table gains a row, add it. Restamp the gate line only if you have a full measured gate run; otherwise leave the existing stamp and say so in the report.

**`docs/status-log.md`** (append-only — never rewrite an earlier section): append a new M18 section covering the measured Stage-1 wall, the two reasons `bsdthread_register` stopped being forwarded (including the latent hazard that had been live since M14), the feature word and the four gates, **that approach B was killed by measurement and why** (the global root queues have no pthread-pool fallback on macOS 26), and whatever Step 1 measured.

Do not restate either in `CLAUDE.md`. The oracle census there is **unchanged** by this plan — say so explicitly in the report so a later reader does not go looking for an eighth site.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace/tests/dispatch_e2e.rs README.md docs/status-log.md
git commit -m "M18 t6: measure the Stage-2 surface; move the gate to the wall that is real now"
```

---

## Self-Review

**Spec coverage.** Stage 1 of the spec's mechanism → Tasks 3, 4, 5. `wqthread`/`pthsize` capture → Task 4. Feature-word lever and its four gates → Task 3. Syscall constants → Task 2. Guest and headline gate → Tasks 1, 6. Honest-gate parking → Tasks 1 and 6. Determinism posture and symmetry rule 1 → Task 5 (both arms, one method, byte-compare). Risk 1 (the inferred host return) → Task 1 Step 5, which measures it and then removes the instrumentation. Documentation split → Task 6 Step 5.

**Deliberately NOT covered, and tracked:** the spec's Stage 2 — `workq_open`/`workq_kernreturn` emulation, the worker-park `BlockReason`, worker-thread construction, the new `verify_thread` sites, and the wrong-thread divergence test. These need Task 6's measurement to exist first. They get their own plan and their own spec section. **Spec risks 2, 3, 4 and 5 all belong to that plan, not this one.**

**Placeholder scan.** No TBD/TODO. Every code step carries real code. Task 6's Step 2 assertion is conditional on a measured outcome rather than placeholder — the condition is stated, and the alternative branch (Step 3) is fully specified.

**Type consistency.** `guest_bsdthread_register(&mut self, args: [u64; 8]) -> u64` is defined in Task 4 and called with that exact signature in Task 5's two arms. `WORKQ_FEATURE_WORD: u32` is defined in Task 3 and widened at its two use sites (`WORKQ_FEATURE_WORD as u64`) in Tasks 3 and 4. `wq_thread_pc()`/`pthread_size()` are defined and tested in Task 4 and consumed only by the Stage-2 plan. `set_thread_start_pc` is removed in Task 5 Step 3, with Task 4 Step 4 naming the compile-order dependency explicitly.

**One risk this plan carries and cannot retire.** Task 5 changes libpthread's initialisation path for *every* dynamic guest, not only the dispatch one. Task 5 Step 4 is built as the detector, with an explicit instruction to report BLOCKED rather than attempt a fix, because a regression there is a spec-level finding.
