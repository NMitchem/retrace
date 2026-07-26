# retrace M7 — rung 1 of the breadth ladder (a real Rust binary): Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A real Rust binary (`rustc`-built, full `std`) records and replays bit-for-bit through real
`/usr/lib/dyld` **while actually reaching `main`** — and the gate that judges it cannot be satisfied
by a guest that died in dyld.

**Architecture:** Four fully-specified tasks build the instrument and the honest RED (a rung
assertion that demands the guest ran; the `hello_rust` guest; `Stop::Fault` visibility in
`RETRACE_TRACE`; the born-`#[ignore]`d headline gate). Task 5 is a diagnosis that produces a written
route decision. **The fix is deliberately unplanned** — see the Replanning Gate after Task 5.

**Tech Stack:** Rust 1.95.0 pinned, `aarch64-apple-darwin`, Hypervisor.framework via `hv-sys`. No new
dependencies anywhere. Spec:
`docs/superpowers/specs/2026-07-26-retrace-m7-rust-design.md`. Branch `m7-rust`, base `5bbf1b0`.

## Global Constraints

- Tests MUST run with `--test-threads=1` (Hypervisor.framework permits one VM per process; a bare
  `cargo test` flakes with `HV_BUSY`). Full gate: `just gate`.
- A test that spawns the CLI must codesign it first — always go through
  `crates/retrace/tests/util/mod.rs::bin()` (existing helper; already handles it).
- `clippy.toml` denies `Instant::now` / `SystemTime::now` / `std::thread` — load-bearing determinism
  guards, not style. Do not introduce any.
- TDD: write the failing test, RUN it and confirm the failure mode, then implement. **Never weaken an
  existing assertion.** Never touch `crates/retrace-guest/asm/wildstore.s` semantics.
- Test output MUST be pristine — no stray warnings, and no panic spew from tests that deliberately
  provoke panics (see Task 1, Step 5).
- Commit style: `M7 t<N>: <what>` on branch `m7-rust`; end every commit message with the
  `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>` trailer used in this repo's
  history.
- Guest expectations, exact: `HELLO_DYN` prints `"hi\n"` and exits 0. `CRASHY` records a crash and
  exits **139**. `HELLO_RUST` must print `"hi from rust\n"` and exit 0.
- Crash process-exit convention (M6, unchanged): a crash outcome exits **139** (128+SIGSEGV) on
  `record`, `record-dyn` **and** `replay` — a recorded crash is a *successful* recording and a
  verified crash replay is a *successful* replay. **This convention is exactly why Task 1 exists.**
- **No M6 regression.** `crashy_e2e`, `crashy_cli`, `crash`, `vaipa`, `watch_dyn`, `hello_dyn_e2e`,
  `cache_pager`, `reservecommit` all stay green. Gate is 136/0/0 at base.
- **Wall-clock warning:** every `record_dynamic` call runs a full dyld bring-up, ~50s. Task 1's tests
  cost ~4 such runs (~3-4 min). Do not "optimize" this by dropping a replay iteration.

## File Structure

- `crates/retrace/tests/util/mod.rs` — **modify.** Gains the rung assertion helper. This is the
  existing home for shared test scaffolding (`bin`, `record`, `replay`, `record_dynamic`,
  `discover_crashy_addrs`), which resolves the spec's open question 4.
- `crates/retrace/tests/rung.rs` — **NEW.** Tests the *instrument*: proves the rung assertion accepts
  a guest that ran and rejects one that crashed. Deliberately separate from `hello_rust_e2e.rs`, which
  tests the *guest* — one responsibility per file. (Refines the spec's Components list, which folded
  both into `hello_rust_e2e.rs`; the helper's own tests must exist before the guest does.)
- `crates/retrace-guest/rs/hello_rust.rs` — **NEW.** The rung-1 guest source.
- `crates/retrace-guest/build.rs` — **modify.** One `rustc` invocation, mirroring the existing
  one-`Command`-per-guest pattern.
- `crates/retrace-guest/src/lib.rs` — **modify.** `HELLO_RUST` path const + a parse test.
- `crates/retrace-core/src/lib.rs` — **modify.** `Stop::Fault` arm in the `RETRACE_TRACE` log filter.
- `crates/retrace/tests/hello_rust_e2e.rs` — **NEW.** The headline gate (born `#[ignore]`d).
- `README.md` — M7 Status section (at closeout, after the Replanning Gate).

---

### Task 1: The rung assertion — a gate that cannot be fooled by a recorded crash

**Files:**
- Modify: `crates/retrace/tests/util/mod.rs` (append after `discover_crashy_addrs`, ends at :73)
- Create: `crates/retrace/tests/rung.rs`

**Interfaces:**
- Consumes: `util::record_dynamic(guest: &str) -> (RunOut, PathBuf)`, `util::replay(&Path) -> RunOut`,
  `util::RunOut { code: i32, stdout: Vec<u8>, stderr: String }` — all existing.
- Produces: `util::assert_rung_records_and_replays(guest: &str, expect_stdout: &[u8]) -> RungOut`
  and `util::RungOut { trace: std::path::PathBuf, stdout: Vec<u8> }`. Task 4 calls this.

**Why this task is first:** M6 established that a recorded crash is a successful recording and a
verified crash replay is a successful replay (both exit 139). So a gate asserting only "records and
replays bit-for-bit" is *already satisfied today* by a Rust guest that dies in dyld without executing
one line of its own code. This helper is the discriminator, and it must exist and be proven before
anything relies on it.

- [ ] **Step 1: Write the failing tests**

Create `crates/retrace/tests/rung.rs`:

```rust
// Tests the breadth-ladder rung ASSERTION itself — the instrument, not any guest.
//
// M6 made a recorded crash a *successful* recording (exit 139) and a verified crash replay a
// *successful* replay. A gate that only checks "record and replay agree" is therefore satisfied by
// a guest that died inside dyld having run none of its own code: the trace is complete, the replay
// reproduces it byte-for-byte, and the divergence oracle is correctly silent. Agreement between two
// runs is not evidence that either run did anything. These two tests pin both polarities of the
// discriminator that closes that hole.
mod util;

#[test]
fn the_rung_assertion_accepts_a_guest_that_ran() {
    // hello_dyn reaches main and prints "hi\n" through real dyld — the positive control.
    let r = util::assert_rung_records_and_replays(retrace_guest::HELLO_DYN, b"hi\n");
    assert_eq!(r.stdout, b"hi\n");
    assert!(r.trace.exists(), "the rung helper must hand back the trace it recorded");
}

#[test]
fn the_rung_assertion_rejects_a_recorded_crash() {
    // crashy records a crash and replays it bit-for-bit, so it satisfies an agreement-only gate
    // completely. The rung assertion must still reject it. Panic hook suppressed so the deliberate
    // failure does not spew into otherwise-pristine test output.
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(|| {
        util::assert_rung_records_and_replays(retrace_guest::CRASHY, b"unreachable\n");
    });
    std::panic::set_hook(prev);
    assert!(outcome.is_err(),
        "the rung assertion MUST reject a guest that crashed in dyld — crashy records and replays \
         bit-for-bit, so an agreement-only gate would pass it");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p retrace --test rung -- --test-threads=1`
Expected: FAIL to compile — `no function or associated item named
'assert_rung_records_and_replays' found`. That is the correct RED for this step.

- [ ] **Step 3: Implement the helper**

Append to `crates/retrace/tests/util/mod.rs`:

```rust
/// What a breadth-ladder rung guest yielded once it PROVED IT RAN.
pub struct RungOut { pub trace: std::path::PathBuf, pub stdout: Vec<u8> }

/// The breadth-ladder rung assertion: record `guest` through real dyld, then replay it twice.
///
/// Demands a **clean exit 0 with exactly `expect_stdout`** — not merely that record and replay
/// agree. M6's convention makes a recorded crash a successful recording and a verified crash replay
/// a successful replay (both exit 139), so an agreement-only check is satisfied by a guest that died
/// inside dyld having executed none of its own code. `code == 0` is the discriminator: under M6 a
/// crash outcome always exits 139, so only a guest that reached its own `exit(0)` can pass, and the
/// stdout equality proves it got far enough to produce output.
///
/// Panics with a diagnostic on any failure — it is an assertion helper, and `tests/rung.rs` pins
/// both polarities.
pub fn assert_rung_records_and_replays(guest: &str, expect_stdout: &[u8]) -> RungOut {
    let (rec, trace) = record_dynamic(guest);
    assert_eq!(rec.code, 0,
        "rung guest must reach a clean exit(0); 139 means it CRASHED (M6 records that as a \
         successful recording, which is exactly what this assertion exists to reject). stderr:\n{}",
        rec.stderr);
    assert_eq!(rec.stdout, expect_stdout,
        "rung guest stdout mismatch — did it reach main? got {:?}, want {:?}",
        String::from_utf8_lossy(&rec.stdout), String::from_utf8_lossy(expect_stdout));
    for i in 0..2 {
        let rep = replay(&trace);
        assert_eq!(rep.code, 0, "replay {i} must exit 0. stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged from the recording");
    }
    RungOut { trace, stdout: rec.stdout }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p retrace --test rung -- --test-threads=1`
Expected: PASS, 2/2. Takes ~3-4 min (four dyld bring-ups).

- [ ] **Step 5: Verify the output is pristine**

Re-read the Step 4 output. The negative test provokes a deliberate panic; the suppressed hook must
keep it out of the log. Expected: no `panicked at` lines, no backtrace, no warnings. If any appear,
the hook suppression is wrong — fix it, do not accept the noise.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace/tests/util/mod.rs crates/retrace/tests/rung.rs
git commit -m "M7 t1: the rung assertion — a gate a recorded crash cannot satisfy

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: The `hello_rust` guest

**Files:**
- Create: `crates/retrace-guest/rs/hello_rust.rs`
- Modify: `crates/retrace-guest/build.rs` (append before the closing `}` of `main`, at :255)
- Modify: `crates/retrace-guest/src/lib.rs` (const beside `CRASHJMP` at :111; test in `mod tests`)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `retrace_guest::HELLO_RUST: &str` — an absolute path to the built binary. Task 4 uses it.

- [ ] **Step 1: Write the guest**

Create `crates/retrace-guest/rs/hello_rust.rs`:

```rust
// M7 rung 1 of the breadth ladder: the smallest REAL Rust program, built by the real toolchain.
//
// Full std and println! are the point — they pull in std::rt init, the stdout lock, and the stack
// guard, none of which a hand-written C fixture exercises. Deliberately no opt-level tuning and no
// panic=abort: the goal is what the toolchain emits by default. A Rust panic() would go
// panic -> abort() -> SIGABRT, which lands on M6's deferred signal-delivery boundary rather than the
// Stop::Fault crash path, so this guest must not panic.
fn main() {
    println!("hi from rust");
}
```

- [ ] **Step 2: Write the failing parse test**

In `crates/retrace-guest/src/lib.rs`, inside `mod tests`:

```rust
#[test]
fn hello_rust_guest_parses() {
    let l = parse_macho(&std::fs::read(HELLO_RUST).unwrap());
    assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64),
            "entry 0x{:x} not inside any segment", l.entry);
    // Rung 1's whole premise is a real dynamic binary through the real dynamic linker.
    assert_eq!(l.dylinker.as_deref(), Some("/usr/lib/dyld"),
               "hello_rust must be dynamically linked through real dyld");
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p retrace-guest hello_rust_guest_parses -- --test-threads=1`
Expected: FAIL to compile — `cannot find value 'HELLO_RUST' in this scope`.

- [ ] **Step 4: Add the build rule and the const**

Append to `crates/retrace-guest/build.rs`, before `main`'s closing brace:

```rust
    // hello_rust: M7 rung 1 — a real Rust binary from the real toolchain, full std. rustc on a
    // single file takes no cargo lock, so there is no build recursion; RUSTC is the toolchain cargo
    // is already using (pinned 1.95.0), so the guest can't drift to a different compiler than the
    // workspace. Plain --target aarch64-apple-darwin (NOT arm64e, per the ladder's premise that
    // self-built binaries are arm64); links libSystem via /usr/lib/dyld like hello_dyn.
    let src = format!("{}/rs/hello_rust.rs", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/hello_rust");
    println!("cargo:rerun-if-changed={src}");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let status = Command::new(rustc)
        .args(["--target", "aarch64-apple-darwin", "-o", &bin, &src])
        .status().expect("rustc hello_rust");
    assert!(status.success(), "hello_rust guest build failed");
```

Add to `crates/retrace-guest/src/lib.rs` beside the other consts:

```rust
pub const HELLO_RUST: &str = concat!(env!("OUT_DIR"), "/hello_rust");
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p retrace-guest hello_rust_guest_parses -- --test-threads=1`
Expected: PASS.

- [ ] **Step 6: Confirm the binary is what the spec measured**

Run: `otool -hv $(find target -name hello_rust -type f | head -1)`
Expected: `MH_MAGIC_64 ARM64 ALL ... EXECUTE ... NOUNDEFS DYLDLINK TWOLEVEL PIE
MH_HAS_TLV_DESCRIPTORS`. Record the byte size in the task report (the spec measured 466,296).
If `MH_HAS_TLV_DESCRIPTORS` is absent or the arch is arm64e, STOP and report — the spec's premises
were measured on a `rustc`-built binary and a mismatch means the build rule differs from the probe.

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-guest/rs/hello_rust.rs crates/retrace-guest/build.rs crates/retrace-guest/src/lib.rs
git commit -m "M7 t2: hello_rust — the rung-1 guest, built by the real Rust toolchain

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `Stop::Fault` in `RETRACE_TRACE`

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` (the `if trace_log` block, :72-94)
- Create: `crates/retrace/tests/faultlog.rs`

**Interfaces:**
- Consumes: `Stop::Fault { pc, esr, far }` (M6, `retrace-box`), `retrace_guest::CRASH` (M6).
- Produces: a `[fault] pc=… esr=… far=… ec=…` stderr line on a `RETRACE_TRACE=1` run. Task 5's
  diagnosis depends on it.

**Why:** the log filter matches only `Stop::Syscall`, so the one trap that matters for M7's wall is
the one it cannot show — while `CLAUDE.md` advertises the flag as logging "every dispatched trap".
This is a parked M6 follow-up minor, promoted onto M7's critical path.

- [ ] **Step 1: Write the failing test**

Create `crates/retrace/tests/faultlog.rs`:

```rust
// RETRACE_TRACE=1 must show the terminal fault, not just syscalls. CLAUDE.md advertises the flag as
// logging every dispatched trap, and M7's bring-up diagnosis needs the fault's pc/esr/far.
mod util;

#[test]
fn trace_log_shows_the_terminal_fault() {
    let trace = std::env::temp_dir().join(format!("retrace-faultlog-{}.bin", std::process::id()));
    let out = std::process::Command::new(util::bin())
        .args(["record", retrace_guest::CRASH, "-o", trace.to_str().unwrap()])
        .env("RETRACE_TRACE", "1")
        .output().unwrap();
    assert_eq!(out.status.code(), Some(139), "crash guest must record a crash");
    let err = String::from_utf8_lossy(&out.stderr);
    // crash.s stores to the never-mapped GARBAGE_VA => a lower-EL data abort (EC 0x24).
    assert!(err.contains("[fault] "), "no [fault] line in the trace log:\n{err}");
    assert!(err.contains("far=0x4000dead0000"), "[fault] line lacks the fault address:\n{err}");
    assert!(err.contains("ec=0x24"), "[fault] line lacks the data-abort class:\n{err}");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p retrace --test faultlog -- --test-threads=1`
Expected: FAIL on `no [fault] line in the trace log` — the run exits 139 and prints the crash line to
stderr, but no `[fault]` log line exists yet.

- [ ] **Step 3: Add the log arm**

In `crates/retrace-core/src/lib.rs`, inside `if trace_log`, immediately after the existing
`if let Stop::Syscall { .. }` block closes:

```rust
            if let Stop::Fault { pc, esr, far } = &stop {
                // The terminal trap. CLAUDE.md advertises this flag as logging every dispatched
                // trap, and a crash is the one an M7-style bring-up most needs to see.
                eprintln!("[fault] pc={:#x} esr={:#x} far={:#x} ec={:#x}",
                    *pc, *esr, *far, (*esr >> 26) & 0x3f);
            }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p retrace --test faultlog -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Confirm record and replay both log it**

Run: `RETRACE_TRACE=1 cargo run -q -p retrace -- replay /tmp/retrace-faultlog-*.bin 2>&1 | grep '\[fault\]'`
Expected: the same `[fault]` line. The block is in the shared dispatch, so both sides log it; confirm
rather than assume, and note the result in the task report.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace/tests/faultlog.rs
git commit -m "M7 t3: RETRACE_TRACE logs the terminal fault

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: The headline gate, born `#[ignore]`d — and the honest RED

**Files:**
- Create: `crates/retrace/tests/hello_rust_e2e.rs`

**Interfaces:**
- Consumes: `util::assert_rung_records_and_replays` (Task 1), `retrace_guest::HELLO_RUST` (Task 2).
- Produces: the M7 headline gate `hello_rust_records_and_replays_reaching_main`.

**This task deliberately ends RED.** Its deliverable is a parked gate plus a precisely characterised
failure — not a passing test. Per honest-gate discipline the gate stays `#[ignore]`d with the wall
named in its ignore reason. Do **not** weaken it, and do **not** attempt the fix here.

- [ ] **Step 1: Write the gate**

Create `crates/retrace/tests/hello_rust_e2e.rs`:

```rust
// THE M7 HEADLINE GATE. Rung 1 of the breadth ladder: a real Rust binary, built by the real
// toolchain with full std, records and replays bit-for-bit through real /usr/lib/dyld AND actually
// reaches main. The rung assertion (util::assert_rung_records_and_replays, proven both ways in
// tests/rung.rs) is what makes "reaches main" load-bearing rather than decorative: without it this
// gate would pass on a guest that crashed inside dyld, because M6 records such a crash as a
// successful recording that replays bit-for-bit.
mod util;

#[test]
#[ignore = "M7 rung 1 is parked at a PAC-garbled branch in dyld: a rustc-built hello_rust dies \
            after 240 traps without reaching main. EC 0x20 (instruction abort, lower EL), IFSC \
            level-0 translation fault, branch target 0x67c0001800fc388 = live PAC signature bits \
            over the valid shared-cache address 0x1800fc388 — the guest branched through a signed \
            pointer as if it were raw. Un-ignore only on a genuine double pass. See \
            docs/superpowers/specs/2026-07-26-retrace-m7-rust-design.md."]
fn hello_rust_records_and_replays_reaching_main() {
    util::assert_rung_records_and_replays(retrace_guest::HELLO_RUST, b"hi from rust\n");
}
```

- [ ] **Step 2: Run the ignored gate and characterise the failure exactly**

Run: `cargo test -p retrace --test hello_rust_e2e -- --ignored --test-threads=1`
Expected: FAIL on the rung assertion's `rec.code == 0` check, reporting 139.

Capture in the task report, verbatim: the assertion message, and the `guest crashed: pc=… esr=… far=…`
line from the captured stderr. Then confirm the spec's measured baseline still holds on this tree:

```bash
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn \
  "$(find target -name hello_rust -type f | head -1)" -o /tmp/m7-red.bin > /tmp/m7-red.log 2>&1
echo "exit=$?"
LC_ALL=C grep -ac '^\[trap\]' /tmp/m7-red.log     # spec measured 240
LC_ALL=C grep -a 'hi from rust' /tmp/m7-red.log   # spec measured 0 occurrences
LC_ALL=C grep -a '\[fault\]' /tmp/m7-red.log      # now visible, thanks to Task 3
```

**Note the `LC_ALL=C` and `grep -a`:** the log contains cargo/dyld bytes that are invalid UTF-8, and
a plain `grep` in a UTF-8 locale silently reports **zero** matches on a file that visibly contains
them. A silent zero is indistinguishable from a real zero — never accept an empty result here without
re-running with `grep -a`.

If the trap count, the crash class, or the branch target differ materially from the spec's measured
values, say so plainly in the report — the spec's hypotheses are ranked on those numbers.

- [ ] **Step 3: Confirm the gate is parked, not failing, in the normal gate**

Run: `just gate`
Expected: **0 failed**, **1 ignored** (this gate), passed count = 136 + Task 1's 2 + Task 2's 1 +
Task 3's 1 = **140**. Record the exact tally from your own run; do not copy this arithmetic.

- [ ] **Step 4: Commit**

```bash
git add crates/retrace/tests/hello_rust_e2e.rs
git commit -m "M7 t4: headline gate born ignored — parked at the PAC-garbled branch in dyld

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Diagnose the PAC-garbled branch

**Files:**
- Create: `.superpowers/sdd/2026-07-26-retrace-m7-rust/diagnosis.md` (a report, not code)
- No production code changes. If you find yourself editing a crate, you have left this task.

**Interfaces:**
- Consumes: the `[fault]` log line (Task 3), the crash trace from Task 4, and M6's crash park
  (`retrace debug` `continue`/`where`/`regs`/`reverse-stepi`/`x`).
- Produces: a written diagnosis naming (a) the branch instruction and its site, (b) where the signed
  pointer came from, (c) which spec hypothesis holds, (d) a route recommendation — symmetry rule 1
  (mirrored record/replay) or rule 2 (below the trace, inside `Box_::run()`).

**M6 is the instrument here.** The crash is recorded, so the debugger can park at it and step
*backwards* from the corpse to the branch site — which is the capability M6 exists to provide, now
used on retrace itself.

- [ ] **Step 1: Park at the fault and read the machine state**

```bash
cargo run -q -p retrace -- debug /tmp/m7-red.bin --script "continue; where; regs"
```

`continue` parks *at* the faulting instruction (M6's `Exec::park_at_terminal`), so `where` reports the
crash coordinate and pc, and `regs` dumps the register file at the fault. Record all of it.

- [ ] **Step 2: Step backwards to the branch site**

```bash
cargo run -q -p retrace -- debug /tmp/m7-red.bin --script "continue; reverse-stepi; where; regs"
```

The instruction abort never retired, so exactly one `reverse-stepi` from the park lands on the
**branching instruction**. Record its pc, and which register held the garbled target (match the
`regs` dump against `0x67c0001800fc388`).

- [ ] **Step 3: Decode the branch instruction**

```bash
cargo run -q -p retrace -- debug /tmp/m7-red.bin --script "continue; reverse-stepi; x <BRANCH_PC> 4"
```

Substitute the pc from Step 2. Decode the 4 bytes by hand (or `echo '<bytes>' | llvm-mc -disassemble
-arch=arm64`). **This single fact is the primary discriminator:**

- a plain `br`/`blr` ⇒ no authentication was attempted ⇒ the pointer was expected to be **raw**, so
  something signed a slot that should not have been, or a patch that should have overwritten it was
  lost (spec hypotheses 1 and 2)
- a `braa`/`blraa`/`braaz` ⇒ authentication *was* attempted, which would fault as FPAC rather than
  as a garbled branch target — if you see this, the spec's model is wrong and that is the finding

- [ ] **Step 4: Trace the pointer to its source**

`reverse-stepi` further to find the load that produced the branch target (expect an
`ldr xN, [xM, #off]`), then read that slot:

```bash
cargo run -q -p retrace -- debug /tmp/m7-red.bin --script "continue; reverse-stepi; reverse-stepi; where; regs; x <SLOT_ADDR> 8"
```

Determine whether the slot address falls in the shared cache (base `0x180000000`, so a cache offset)
and whether `crates/retrace-box/src/cache.rs`'s walker treats it as an auth slot — `walk_page` and
`sign_slots` are the relevant code. That answers hypothesis 2 directly.

- [ ] **Step 5: Test hypothesis 1 (the `mach_vm_protect` no-op)**

`mach_vm_protect` is serviced as a no-op success (`crates/retrace-core/src/lib.rs:239-242`). Establish
whether any of the 27 protect flips on `0x1ec444000` covers the slot from Step 4, and whether dyld
writes to that slot between the RW flip and the RO flip. The trace is the evidence: the recorded
`Syscall` events carry the protect args, and any dyld write to guest memory that retrace *did* honor
appears as a recorded write.

State plainly whether the patch was **lost** (hypothesis 1) or **applied but then mis-consumed**
(hypothesis 2). If the evidence cannot distinguish them, say so — an honest "undetermined, here is
what would settle it" is a valid deliverable and better than a guess.

- [ ] **Step 6: Write the diagnosis and the route recommendation**

Write `.superpowers/sdd/2026-07-26-retrace-m7-rust/diagnosis.md` covering: the branch site and
instruction; the pointer's provenance; which hypothesis the evidence supports and which it rules out;
the recommended route (rule 1 vs rule 2) **with the reason**; the blast radius (does the fix touch
`cache.rs`'s re-signer, or `mach_vm_protect`'s servicing, or something else — and which existing tests
guard it); and whether the defect looks like one pointer or a whole class (spec risk R4).

- [ ] **Step 7: Commit the diagnosis**

```bash
git add .superpowers/sdd/2026-07-26-retrace-m7-rust/diagnosis.md 2>/dev/null || true
git commit -m "M7 t5: diagnosis of the PAC-garbled branch in dyld

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

Note: `.superpowers/` is git-ignored in this repo, so this commit may be empty — that is fine, the
diagnosis lives in the SDD workspace and its content goes in the task report either way.

---

## ⛔ REPLANNING GATE — stop here

**Tasks 6 (the fix) and 7 (closeout) are deliberately NOT specified in this plan, and no worker
should attempt them from this document.**

This is not an oversight. The spec's three ranked hypotheses imply materially different fixes in
different crates — one of them changes `mach_vm_protect`'s servicing, load-bearing since M2, with its
own record/replay symmetry consequences. Writing steps for a fix before Task 5's diagnosis would mean
inventing a mechanism, and this plan would then be specifying code against a cause nobody has
established. That is precisely the failure mode the repo's M2 chain avoided by diagnosing first and
routing deliberately.

**After Task 5 lands, amend this plan** with Tasks 6+ derived from the diagnosis. This repo has
precedent: the M6 plan was amended mid-flight (commit `5ab0985`) when a task needed an interface the
original plan had not anticipated.

The amendment's tasks must satisfy, and the closeout is bound by, these constraints already settled:

- **Route** per the diagnosis: symmetry rule 1 (record's arm mirrored in replay, replay's byte-compare
  *is* the divergence check) or rule 2 (below the trace inside `Box_::run()`, so record and replay
  share it and determinism is automatic). Both prior PAC walls — M2-cache re-signing and M2-bfam
  strip-on-FPAC — landed below the trace.
- **Determinism:** any re-signed or stripped pointer stays a pure function of (file bytes, fixed
  slide, fixed keys), identical on both runs.
- **Never reimplement Apple's PAC** — sign and authenticate by running real `pac*`/`aut*` on the guest
  vCPU with the fixed keys.
- **A `mach_vm_protect` change is a route decision**, not an incidental edit: full regression across
  `cache_pager`, `reservecommit`, `mmap`, `munmap`, `remap`.
- **Closeout:** un-ignore the headline gate only on a genuine double pass; add the README M7 Status
  section (honest about what rung 1 proved, what the wall was, and the next boundary); `just gate`
  reports 0 failed / 0 ignored with the tally counted from a real run.
- **If the wall does not fall:** leave the gate `#[ignore]`d, rewrite its ignore reason and the README
  to name the *new* boundary precisely, and close the milestone honestly. Per the spec's exit
  criterion that is a legitimate M7 outcome — not a failure, and not a reason to loosen the gate.

## Post-plan notes for the executor

- Task order is strict: T1 → T2 → T3 → T4 → T5. T1 is first on purpose (it is the instrument the rest
  is judged by); T3 is before T4 because T4's characterisation step reads the `[fault]` line.
- **Threads are a hard stop.** If `hello_rust`'s `std` init spawns a thread, `Sched` is unused and
  threads are out of scope for M7 — STOP and report, do not improvise a scheduler. Per spec risk R2
  this re-parks the milestone rather than expanding it.
- **A Rust `panic!` is out of scope.** `panic` → `abort()` → `SIGABRT` is signal delivery, deferred
  since M6. The guest must not panic; if the wall turns out to involve a panic path, report it rather
  than following it.
- Every `record_dynamic` is a full dyld bring-up (~50s). Budget for it; do not trim replay iterations
  to save time.
- When summing a gate tally from a captured log, use `grep -a` with `LC_ALL=C` and strip ANSI escapes
  — a UTF-8-locale `grep` silently returns zero matches on cargo's colored output.
