# retrace M7 — rung 1 of the breadth ladder (a real Rust binary): Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A real Rust binary (`rustc`-built, full `std`) records and replays bit-for-bit through real
`/usr/lib/dyld` **while actually reaching `main`** — and the gate that judges it cannot be satisfied
by a guest that died in dyld.

**Architecture:** Four fully-specified tasks build the instrument and the honest RED (a rung
assertion that demands the guest ran; the `hello_rust` guest; `Stop::Fault` visibility in
`RETRACE_TRACE`; the born-`#[ignore]`d headline gate). Task 5 is a diagnosis that produces a written
route decision. Tasks 6-8 (the PAC-posture fix, the test repair it forces, and the closeout) were
**added by amendment after Task 5 landed** — see the Replanning Gate note after Task 5.

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

- [ ] **Step 7: No commit — this task produces no tracked files**

`.superpowers/` is git-ignored in this repo, so there is deliberately **nothing to commit** for this
task. Do not run `git commit` (with an empty index it exits non-zero and looks like a failure), and do
not `git add -f` the diagnosis to force it into history — the SDD workspace is scratch by design.

Instead, verify the tree is clean and report the diagnosis in full:

```bash
git status --porcelain   # expect: only the untracked .superpowers/ line
```

Your task report **is** the deliverable for this task. Copy the diagnosis's findings and the route
recommendation into it verbatim, so the conclusion survives even if the workspace is cleaned.

---

## ✅ REPLANNING GATE — reached, diagnosed, and passed

Tasks 1-5 are complete. The gate held: no fix was written until Task 5's diagnosis landed
(`.superpowers/sdd/2026-07-26-retrace-m7-rust/diagnosis.md`, carrying a **REVIEWED AND CLEARED TO
PLAN AGAINST** banner after two adversarial review rounds and two correction passes). **The human
partner approved proceeding**, and Tasks 6-8 below are the amendment the gate was waiting for. Task 5
answered the spec's open questions 1 and 2: hypotheses 1 and 2 are ruled out, hypothesis 3 is the
*site* but not the *cause*, and the route is **symmetry rule 2 — below the trace**.

`mach_vm_protect`'s no-op servicing is **exonerated for this fault** and is NOT touched, so spec risk
R3 does not fire. The constraints listed here were settled before the diagnosis and **still bind**:

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

---

## Amendment: Tasks 6-8 (the PAC posture fix)

**The cause, in one sentence** (from the diagnosis): retrace enables pointer authentication for
*every* guest (`SCTLR_EL1.EnIA|EnIB|EnDA|EnDB`, unconditionally, at four install sites), while macOS
enables PAC **per process, only for `arm64e` main executables** — so dyld's unconditional `paciza` in
its TLV-descriptor setup, architecturally a **NOP** in a real plain-`arm64` process, really signs
inside retrace, and `hello_rust`'s plain `blr x8` through that descriptor branches to a signed
pointer and takes an instruction abort.

**Three constraints that govern all three tasks. They are the most valuable things carried out of the
review chain; do not soften any of them.**

1. **FAIL LOUD, MANDATORY.** The main executable's base needs a **named constant** (there is none in
   the repo today — `grep -n EXE_BASE` finds nothing; the base is whatever `__TEXT.vmaddr` says,
   which is `0x100000000` for every current guest), and the derivation from the header at that base
   **must fail loud when `MH_MAGIC_64` is absent — it must NEVER fall back to a default posture.**
   The reason, which must appear in the code comment as well as here: **a silent PAC-off default is
   indistinguishable from correct for every guest this repo can build today**, since all of them are
   plain arm64. It would therefore hide the bug at precisely the moment an arm64e guest arrives —
   the same vacuous-green failure mode as a test that passes while testing nothing, in a second
   place.
2. **The failure modes are asymmetric, and that asymmetry dictates the shape.** A wrong *cache*
   posture fails **loud and early** (the guest faults on the first cache pointer it consumes). A
   **posture mismatch across the four SCTLR install sites** fails **LATE**: record PAC-off against
   replay PAC-on produces no fault at all, only a divergence at the final full-memory comparison —
   or, through `from_checkpoint()`, a silently mis-seeked debug session with no error whatsoever.
   That is the argument for **one named derivation helper, evaluated once at construction, used by
   all four sites**, plus the cross-check accessor in Task 6 Step 4 that makes a missed site fail in
   a unit test instead of in a replay six steps later.
3. **The falsifiable prediction, stated over BOTH directions.** The defect is a whole class (spec
   risk R4), and the class is bidirectional: arm64e cache code signs a pointer that plain-arm64
   client code consumes raw (garbled-branch instruction abort, EC `0x20` — the observed wall), **and**
   plain-arm64 code hands a raw pointer to arm64e cache code that `AUT*`s it (FPAC, EC `0x1C`).
   After the fix **neither** may appear. Task 6 Step 7 scores this explicitly rather than believing it.

**Route: symmetry rule 2 — below the trace.** Nothing new is observed or recorded, so rule 1's
mirrored arm plus byte-compare buys nothing and would only add a divergence surface. `SCTLR_EL1` is
part of the machine configuration both record and replay establish, and the posture is a pure
function of recorded bytes. **The trace format, `Event`, `Regs` and `TRACE_MAGIC` are NOT touched** —
no format break, and every trace on disk stays readable. Both prior PAC walls (M2-cache re-signing,
M2-bfam strip-on-FPAC) landed below the trace for the same reason.

**Full regression is REQUIRED for this fix**, in a way it was not for Tasks 2-3, because the posture
touches the replay and checkpoint paths. The named guards: `pac`, `sign_oracle`, `cache_pager`,
`bfamstrip`, `strip47`, `vaipa`, `reservecommit`, `mmap`, `munmap`, `remap`, the M6 surface
(`crashy_e2e`, `crashy_cli`, `crash`, `watch_dyn`), and **`hello_dyn_e2e` as the sharpest single
guard** — `hello_dyn` is arm64, so it moves to the new PAC-off posture and must stay bit-for-bit
green. Budget the wall clock: every `record_dynamic` is a full dyld bring-up, ~50s.

---

### Task 6: Derive the guest's PAC posture from the main executable

**Files:**
- Modify: `crates/retrace-guest/src/lib.rs` — `Loaded` (`:4`), `parse_macho` (`:9-56`), `mod tests`
- Modify: `crates/retrace-box/src/lib.rs` — `SCTLR_MMU_ON` (`:95-96`), the four install sites
  (`:586`, `:1000`, `:1582`, `:1958`), the `Box_` struct + its four literals, `BoxState` (`:278-298`),
  `checkpoint()` (`:1900`), `from_checkpoint()` (`:1941`), `sign_slots` (`:603`),
  `dbg_internal_state()` (`:2013`)
- Create: `crates/retrace-box/tests/pacposture.rs`
- **Check, expecting NO change:** `crates/retrace-core/src/lib.rs:448` (see Step 6)

**Interfaces:**
- Produces: `retrace_box::EXE_BASE: u64`; `retrace_box::pac_posture(cpusubtype: u32) -> bool`;
  `Box_::load_with_pac(&Loaded, bool) -> Box_` (Task 7 calls it); `Box_::dbg_pac_enabled(&self) -> bool`;
  `Loaded.cpusubtype: u32`.
- Consumes: `retrace_arch::CPU_SUBTYPE_ARM64E` (`= 2`, already defined and already used by
  `slice_arm64e`).

**Why the posture is an explicit parameter and not a bare `cpusubtype` lookup — the single most
important design detail of this amendment.** `crates/retrace-guest/build.rs` builds every asm/C guest
`-arch arm64`, `hello_rust` for `aarch64-apple-darwin`, and `build.rs:148` records that third-party
arm64e builds are gated. **Every guest this repo can currently build is plain arm64** (verified:
`hello`, `pacguest`, `steppy`, `hello_dyn`, `crashy`, `hello_rust` all report `ARM64 ALL`), so a bare
lookup would turn PAC off *everywhere, permanently*, and the re-signer, the signing oracle and the
strip-on-FPAC arm would all become permanently unexercised. An explicit parameter lets the PAC tests
keep constructing a genuinely PAC-on box while the *guest* path defaults to the faithful posture.

**Footgun to encode in the doc comment:** `load_with_pac`'s override is **test-only**. It must never
be used on a path whose trace is later replayed — replay re-derives the posture from the header, so
an overridden record run against a derived replay run is exactly constraint 2's late-failing
mismatch.

- [ ] **Step 1: Surface `cpusubtype` from the Mach-O parse (failing test first)**

`parse_macho` never reads header offset 8 today, so the record-side default has nothing to default
*from*. In `crates/retrace-guest/src/lib.rs`, inside `mod tests`:

```rust
    #[test]
    fn parse_macho_surfaces_cpusubtype() {
        // The guest's PAC posture is derived from this field (M7 t6): macOS enables PAC per process
        // only for arm64e main executables. Every guest this repo builds is plain arm64.
        let l = parse_macho(&std::fs::read(HELLO_RUST).unwrap());
        assert_eq!(l.cpusubtype & 0x00ff_ffff, 0,
                   "hello_rust must be CPU_SUBTYPE_ARM64_ALL, got {:#x}", l.cpusubtype);
        assert_ne!(l.cpusubtype & 0x00ff_ffff, retrace_arch::CPU_SUBTYPE_ARM64E,
                   "hello_rust is not arm64e — the ladder's premise is self-built arm64 binaries");
    }
```

Run: `cargo test -p retrace-guest parse_macho_surfaces_cpusubtype -- --test-threads=1`
Expected: FAIL to compile — `no field 'cpusubtype' on type 'Loaded'`.

- [ ] **Step 2: Implement it**

In `crates/retrace-guest/src/lib.rs`, add the field to `Loaded` and read it in `parse_macho`:

```rust
pub struct Loaded { pub segments: Vec<Segment>, pub entry: u64, pub dylinker: Option<String>, pub cpusubtype: u32 }
```

```rust
    // mach_header_64: magic(0) cputype(4) cpusubtype(8). The low 24 bits are the subtype proper;
    // the top 8 are capability bits (arm64e carries a ptrauth ABI version there). The box derives
    // the guest's PAC posture from this — macOS enables PAC per process, only for arm64e mains.
    let cpusubtype = u32le(b, 8);
```

and return it: `Loaded { segments, entry: ..., dylinker, cpusubtype }`.

`grep -rn 'Loaded {' crates --include='*.rs'` and fix **every** construction site (there should be
exactly one, in `parse_macho`; if a test constructs one, fix it too).

Run: `cargo test -p retrace-guest parse_macho_surfaces_cpusubtype -- --test-threads=1` → PASS.

- [ ] **Step 3: Write the posture tests (failing)**

Create `crates/retrace-box/tests/pacposture.rs`:

```rust
// M7 t6: the guest's PAC posture is DERIVED, not assumed.
//
// macOS enables pointer authentication per process, only for arm64e main executables. retrace
// enabled it unconditionally for every guest, so dyld's unconditional `paciza` in TLV setup — a NOP
// in a real plain-arm64 process — really signed, and hello_rust's plain `blr x8` through the
// resulting descriptor branched to a signed pointer (M7's wall).
//
// The dangerous failure mode is NOT a wrong posture (that faults early and loudly). It is a posture
// MISMATCH between the four SCTLR install sites: record PAC-off against replay PAC-on never faults,
// it diverges only at the final full-memory compare, or mis-seeks silently through from_checkpoint.
// These tests exist to make that mismatch fail here instead of there.
//
// One VM per process: --test-threads=1, and every box is dropped before the next is created.
use retrace_arch::SYS_EXIT;
use retrace_box::{Box_, Stop};
use retrace_guest::{parse_macho, HELLO, PACGUEST};
use retrace_trace::Event;

// pacguest.s: signs with `pacia`, authenticates with `autia`, and leaves x0 = 0 iff PAC was ENGAGED
// and the round-trip recovered the pointer; x0 = 1 if signing was a no-op (PAC disabled).
fn pacguest_exit_arg(mut b: Box_) -> u64 {
    match b.run() {
        Stop::Syscall { num, args } if num == SYS_EXIT => args[0],
        Stop::Syscall { .. } => panic!("unexpected syscall"),
        Stop::Other { esr } => panic!("guest faulted esr={esr:#x}"),
        Stop::Fault { pc, esr, far } => panic!("guest crashed pc={pc:#x} esr={esr:#x} far={far:#x}"),
        Stop::Step => unreachable!("run() does not single-step"),
    }
}

#[test]
fn a_plain_arm64_guest_gets_pac_disabled_like_the_real_os() {
    let loaded = parse_macho(&std::fs::read(PACGUEST).unwrap());
    assert_eq!(loaded.cpusubtype & 0x00ff_ffff, 0, "pacguest must be plain arm64 for this test to mean anything");
    // x0 == 1 is pacguest reporting "signing was a NO-OP" — which is exactly what a plain-arm64
    // process gets from the real OS, and what retrace must now give it.
    assert_eq!(pacguest_exit_arg(Box_::load(&loaded)), 1,
               "a plain-arm64 guest must run with PAC DISABLED (pac* behaving as NOPs)");
}

#[test]
fn an_explicitly_pac_on_box_still_signs() {
    // The escape hatch the PAC tests need: every guest this repo can build is plain arm64, so
    // without this the re-signer, the signing oracle and the strip-on-FPAC arm are untestable.
    let loaded = parse_macho(&std::fs::read(PACGUEST).unwrap());
    assert_eq!(pacguest_exit_arg(Box_::load_with_pac(&loaded, true)), 0,
               "an explicitly PAC-on box must sign and authenticate (x0=0)");
}

#[test]
fn restore_rederives_the_same_posture_from_the_snapshot() {
    // restore() gets only (regions, regs) — no sysregs, no cpusubtype. It re-derives the posture
    // from the mach header the snapshot already contains at EXE_BASE. This is the site where a
    // mismatch would fail LATE (a divergence at the final memory compare, not a fault).
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let b = Box_::load(&loaded);
    let recorded = b.dbg_pac_enabled();
    let (mem, regs) = match b.snapshot() {
        Event::Snapshot { mem, regs } => (mem, regs),
        _ => unreachable!("snapshot() always returns Event::Snapshot"),
    };
    drop(b); // one VM per process — tear down before restore() creates the next one
    let r = Box_::restore(&mem, &regs);
    assert_eq!(r.dbg_pac_enabled(), recorded, "restore() must re-derive the RECORD run's posture");
    assert!(!recorded, "hello is plain arm64, so the posture must be PAC-off");
}

#[test]
fn from_checkpoint_carries_the_posture() {
    // The mid-run twin. Its snapshot is taken while the guest is running, so its header is NOT
    // pristine by construction — that is why the posture is stored in BoxState rather than
    // re-derived here.
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load_with_pac(&loaded, true);
    let _ = b.run(); // reach the first syscall so the checkpoint is genuinely mid-run
    let st = b.checkpoint();
    drop(b);
    let r = Box_::from_checkpoint(&st);
    assert!(r.dbg_pac_enabled(),
            "from_checkpoint must restore the captured posture, not re-derive from the header");
}
```

Run: `cargo test -p retrace-box --test pacposture -- --test-threads=1`
Expected: FAIL to compile — `no function or associated item named 'load_with_pac'` /
`no method named 'dbg_pac_enabled'`. That is the correct RED.

- [ ] **Step 4: Implement the posture in `retrace-box`**

In `crates/retrace-box/src/lib.rs`, replace the `SCTLR_MMU_ON` const (`:95-96`) with a base + a
derivation, and add the named constant and helpers near it:

```rust
// base 0x30d00800 + M(1) + C(4) + I(0x1000). PAC is NOT in the base: it is per-guest (see below).
const SCTLR_MMU_ON_BASE: u64 = 0x30d0_0800 | 1 | 4 | 0x1000;
// EnIA(31) | EnIB(30) | EnDA(27) | EnDB(13)
const SCTLR_PAC_EN: u64 = 0x8000_0000 | 0x4000_0000 | 0x0800_0000 | 0x2000;

/// The main executable's load address. Every guest this repo builds links `__TEXT` at
/// `0x1_0000_0000`, and replay has NO independent way to learn it — a snapshot is a flat set of IPA
/// regions with no Mach-O in sight. Naming it makes the one assumption `pac_posture_from_memory`
/// rests on explicit and checkable instead of buried.
pub const EXE_BASE: u64 = 0x1_0000_0000;

/// **The one derivation.** All four SCTLR install sites go through this (directly, or through the
/// two wrappers below). macOS enables pointer authentication per process, only for `arm64e` main
/// executables — a plain-`arm64` process sees `PAC*`/`AUT*` as NOPs and `BRAA`/`BLRAA` as
/// `BR`/`BLR`. retrace must match, or arm64e cache code and plain-arm64 client code disagree about
/// whether a pointer carries a signature (M7's wall).
pub fn pac_posture(cpusubtype: u32) -> bool {
    (cpusubtype & 0x00ff_ffff) == retrace_arch::CPU_SUBTYPE_ARM64E
}

/// SCTLR_EL1 for a guest with the given posture. Never build this value ad hoc.
fn sctlr_mmu_on(pac_enabled: bool) -> u64 {
    SCTLR_MMU_ON_BASE | if pac_enabled { SCTLR_PAC_EN } else { 0 }
}

/// Re-derive the posture from a snapshot's own memory — `restore()`'s only route, since its inputs
/// are `(regions, regs)` and `Regs` is `{x[31], pc, sp_el0, cpsr}`. Pure: `parse_macho` maps
/// `__TEXT` from `fileoff == 0`, so the mach header is genuinely in guest memory and therefore in
/// the snapshot, and record and replay cannot disagree about bytes the trace must contain anyway.
///
/// FAILS LOUD and NEVER defaults. A silent PAC-off fallback is indistinguishable from correct for
/// every guest this repo can build today (all plain arm64), so it would hide a broken derivation at
/// exactly the moment an arm64e guest arrives.
fn pac_posture_from_memory(regions: &[Region]) -> bool {
    let hdr = regions.iter()
        .find(|r| r.ipa <= EXE_BASE && EXE_BASE + 12 <= r.ipa + r.bytes.len() as u64)
        .unwrap_or_else(|| panic!(
            "no snapshot region covers EXE_BASE {EXE_BASE:#x}; refusing to guess a PAC posture"));
    let o = (EXE_BASE - hdr.ipa) as usize;
    let magic = u32::from_le_bytes(hdr.bytes[o..o+4].try_into().unwrap());
    assert_eq!(magic, 0xfeed_facf,
        "no MH_MAGIC_64 at EXE_BASE {EXE_BASE:#x} (found {magic:#x}) — refusing to guess a PAC posture");
    pac_posture(u32::from_le_bytes(hdr.bytes[o+8..o+12].try_into().unwrap()))
}
```

Then, in order:

1. **`Box_` gains `pac_enabled: bool`.** **APPEND it — do not reorder the struct.** `vcpu` must stay
   declared before `vm` (HVF requires `hv_vcpu_destroy` before `hv_vm_destroy`); field order is
   load-bearing. Add `pac_enabled` to all four `Box_ { .. }` literals.
2. **`load`** (`:551`) becomes a thin default over an explicit constructor:

```rust
    /// Load a static guest with the posture the real OS would give it (derived from the main
    /// executable's `cpusubtype`).
    pub fn load(loaded: &Loaded) -> Box_ { Self::load_with_pac(loaded, pac_posture(loaded.cpusubtype)) }

    /// `load` with an EXPLICIT PAC posture. **Test-only override.** Every guest this repo can build
    /// is plain arm64, so the PAC tests (`pac`, `sign_oracle`, `cache_pager`) would otherwise have
    /// no PAC-on box to assert against. NEVER use the override on a path whose trace is later
    /// replayed: replay re-derives the posture from the header, and an overridden record run against
    /// a derived replay run is a posture mismatch — which fails LATE (a divergence at the final
    /// memory compare), not loudly.
    pub fn load_with_pac(loaded: &Loaded, pac_enabled: bool) -> Box_ { /* the existing body */ }
```

3. **The four install sites**, each replacing `SCTLR_MMU_ON`, each evaluating the posture **once**
   into a local before the `set_sys`:
   - `:586` (`load_with_pac`) — `let pac = pac_enabled;` (the parameter)
   - `:1000` (`load_dynamic`) — `let pac = pac_posture(exe.cpusubtype);` **the MAIN executable, not
     `dyld`.** dyld is arm64e; deriving from it would re-create the bug exactly.
   - `:1582` (`restore`) — `let pac = pac_posture_from_memory(regions);`
   - `:1958` (`from_checkpoint`) — `let pac = state.pac_enabled;`

   `load_dynamic` gets no explicit-override twin: no test needs a PAC-on dynamic box, and every
   override is a mismatch hazard. Add one only if a later task actually needs it.
4. **`BoxState` gains `pub pac_enabled: bool`** (`:278-298`), set by `checkpoint()` from the field.
   This is **load-bearing, not cheap convenience**: `restore()`'s snapshot is taken at landmark 0,
   *before the guest executes*, so its header is pristine by construction — but
   `from_checkpoint()`'s is **mid-run**, and the guest may have overwritten its own header by then.
   Storing the posture is what closes that hole.
5. **`dbg_internal_state()`** (`:2013`) gains ` pac_enabled={}` — the checkpoint round-trip parity
   diagnostic, so a field missed in the `BoxState` round-trip fails loudly instead of silently
   flipping the posture.
6. **The cross-check accessor** — this is the mechanism against constraint 2:

```rust
    /// Test-only: the guest's live PAC posture, read back from SCTLR_EL1 and cross-checked against
    /// the field the constructor derived. PANICS if they disagree — i.e. if some install site set
    /// SCTLR without going through `sctlr_mmu_on(pac_enabled)`. A posture mismatch between the four
    /// sites otherwise fails LATE (a replay divergence, or a silently mis-seeked session); this
    /// makes it fail in a unit test instead.
    #[doc(hidden)]
    pub fn dbg_pac_enabled(&self) -> bool {
        let live = self.vcpu.get_sys(sysreg::SCTLR_EL1).unwrap() & SCTLR_PAC_EN != 0;
        assert_eq!(live, self.pac_enabled,
            "SCTLR_EL1 PAC bits ({live}) disagree with Box_::pac_enabled ({}) — an install site \
             bypassed sctlr_mmu_on()", self.pac_enabled);
        live
    }
```

7. **`sign_slots` short-circuit** (`:603`): when the posture is PAC-off, return the targets unchanged
   instead of running the in-guest stub.

```rust
        // PAC-off guest: the real OS gives a non-arm64e process a REBASE-ONLY cache — its `braa`s
        // are plain `br`s over raw pointers. `pacia`/`pacda` on the stub would NOP to the same
        // result, but making this an INTENDED mode rather than an accident is the point (and it
        // saves a vCPU round-trip per cache data page).
        if !self.pac_enabled { return slots.iter().map(|s| s.target_va).collect(); }
```

Leave `authenticate` alone (its `aut*` genuinely NOP to the input under a PAC-off posture, which is
the correct inverse). **Do NOT delete `try_emulate_fpac_auth` or M2-bfam's strip-on-FPAC arm** — it
becomes correct-by-construction rather than dead for arm64 guests, and arm64e guests still need it.

- [ ] **Step 5: Run the posture tests**

Run: `cargo test -p retrace-box --test pacposture -- --test-threads=1`
Expected: PASS, 4/4.

- [ ] **Step 6: Check whether `retrace-core` needs anything (it should not)**

Run: `cargo build -p retrace-core`
Expected: **builds unchanged.** The diagnosis's blast radius lists `retrace-core/src/lib.rs:448`
(`Box_::restore(&mem, &regs)`) as "touched at minimum". That was written on the assumption the
posture would be *plumbed through the call site*; option (a) — re-deriving inside `restore()` from
the snapshot's own header — makes the plumbing unnecessary and `restore`'s signature unchanged.
**If it builds, that prediction is simply superseded: record the fact in the task report and do NOT
invent a change to satisfy it.** If it does NOT build, stop and report why before improvising.

- [ ] **Step 7: Settle the diagnosis's two inferences, and score the prediction**

The diagnosis observes that `paciza` is a bit-identical no-op in a native plain-arm64 process, but
*infers* (a) that macOS achieves it via the `SCTLR_EL1.EnI*` enables and (b) that a non-PAC process's
cache slots are therefore raw. Both are settled cheaply, right here, by the first record run:

```bash
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn \
  "$(find target -name hello_rust -type f | head -1)" -o /tmp/m7-fix.bin > /tmp/m7-fix.log 2>&1
echo "exit=$?"
LC_ALL=C grep -a 'hi from rust' /tmp/m7-fix.log
LC_ALL=C grep -a '\[fault\]'    /tmp/m7-fix.log
LC_ALL=C grep -ac '^\[trap\]'   /tmp/m7-fix.log
```

`LC_ALL=C` + `grep -a` are mandatory — the log carries non-UTF-8 bytes and a plain `grep` in a UTF-8
locale silently reports **zero** matches on a file that visibly contains them.

Then check the descriptor itself. **Re-derive its address, do not hardcode `0x100048c28`** — that was
measured on the Task-4 build and moves if the guest is rebuilt:

```bash
otool -l "$(find target -name hello_rust -type f | head -1)" | grep -A3 '__thread_vars'
# descriptor #3 = __thread_vars addr + 0x48; call it $SLOT
cargo run -q -p retrace -- debug /tmp/m7-fix.bin --script "continue; x $SLOT 8"
```

**Score three things in the task report:**

- **The inferences:** the descriptor's thunk word must now be the **raw** `0x1800fc388` (low 47 bits
  unchanged, PAC field zero). If it is, both inferences hold and the mechanism is confirmed rather
  than assumed. If it is still signed, the enables are not the control macOS uses and the diagnosis's
  link (i) is wrong — **stop and report that**, do not paper over it.
- **The prediction, both directions** (constraint 3): the old signature `pc = far =
  0x67c0001800fc388`, `esr=0x82000004` (EC `0x20`) must be **gone**, and **no `ec=0x1c` (FPAC)** may
  appear in its place. A `[fault]` line with high bits set over a valid low-47 address is direction
  one recurring; an EC `0x1C` is direction two. Quote any `[fault]` line verbatim.
- **The landmark rule:** the pre-crash trap count **drifts run to run (245-247 measured)**, so **no
  absolute trap index may be used as a landmark or as a pass/fail criterion** — use the `pc`/`esr`/
  `far` signature. Report the trap count as context only.

A *different* wall here is a legitimate outcome (spec risk R1) — record its signature and carry it
into Task 8's re-park. It is not a reason to weaken anything.

- [ ] **Step 8: Commit**

Task 7 repairs the tests this breaks; the workspace will not be fully green until then. Commit the
mechanism on its own anyway — it is one coherent change and the repair is a separate argument.

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/retrace-guest/src/lib.rs crates/retrace-box/src/lib.rs crates/retrace-box/tests/pacposture.rs
git commit -m "M7 t6: derive the guest's PAC posture from the main executable

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Keep the PAC tests falsifiable under the derived posture

**Files:**
- Modify: `crates/retrace-box/tests/pac.rs`, `crates/retrace-box/tests/sign_oracle.rs`,
  `crates/retrace-box/tests/cache_pager.rs`
- Modify: `crates/retrace-guest/build.rs` (the `bfamstrip` and `strip47` build rules) — **or** the
  fallback in Step 4
- Possibly modify: `crates/retrace/tests/bfamstrip_e2e.rs`, `crates/retrace/tests/strip47_e2e.rs`

**Interfaces:**
- Consumes: `Box_::load_with_pac` (Task 6).

**Why this task exists, and why it is not "adjusting tests to fit".** Task 6 breaks these tests **by
design**, and the two ways they break are not the same:

- **Unsatisfiable** — `pac.rs` asserts `x0 == 0`, i.e. *PAC engaged and `autia` recovered the
  pointer*. With every guest this repo can build being plain arm64, that assertion cannot be adjusted
  into truth. It can only be deleted, or given a genuinely PAC-on box. `sign_oracle.rs` and
  `cache_pager.rs` are the same shape: each has an `assert_ne!(signed, target, "PAC not engaged")`
  that fails loudly. **Note this correction to the diagnosis:** it describes `sign_oracle`'s
  round-trip as degenerating to a *silent* vacuous green — the round-trip alone would, but the
  existing `assert_ne!` guard is precisely what makes it fail loudly instead. Do not remove that
  guard; it is the thing doing the work.
- **Vacuously green** — `strip47_e2e` signs with `pacda`, strips with objc's 47-bit `ISA_MASK`, and
  asserts the result equals the original. With `pacda` NOP'd it still passes **while testing
  nothing**. The reviewers judged this the worse of the two failure modes, and it is the one that
  needs deliberate attention rather than a red test to follow.
- **`bfamstrip_e2e`** breaks a third way: its guest corrupts a PAC bit so `autdb` FEAT_FPAC-faults.
  With PAC off there is no fault to intercept, the corrupted value survives, and the guest exits 1.

**Everything in this task must end up asserting something that can actually fail.** And **M2-bfam's
strip-on-FPAC path and the `bfamstrip` guest must NOT be deleted** — arm64e guests still need them.

- [ ] **Step 1: Confirm the failure modes before changing anything**

Run each and record the *exact* failure, so the repair is aimed at an observed break, not a predicted
one:

```bash
cargo test -p retrace-box --test pac        -- --test-threads=1
cargo test -p retrace-box --test sign_oracle -- --test-threads=1
cargo test -p retrace-box --test cache_pager -- --test-threads=1
cargo test -p retrace --test bfamstrip_e2e   -- --test-threads=1
cargo test -p retrace --test strip47_e2e     -- --test-threads=1
```

Expected: `pac` fails on `x0=1`; `sign_oracle` and `cache_pager` fail on their `PAC not engaged`
`assert_ne!`; `bfamstrip_e2e` fails with record exit 1; **`strip47_e2e` PASSES — vacuously.** If
`strip47_e2e` fails instead, say so; the vacuity analysis was wrong and the repair changes shape.

- [ ] **Step 2: Give the three in-process tests an explicitly PAC-on box**

In `pac.rs`, `sign_oracle.rs` and `cache_pager.rs`, change `Box_::load(&loaded)` to
`Box_::load_with_pac(&loaded, true)`, and update each file's header comment to say **why** — the
guest is plain arm64, so the box must be told, and that is the whole reason Task 6 made the posture
an explicit parameter. In `sign_oracle.rs` also fix the stale comment at `:33-35` ("`load` already
set the fixed PAC keys and SCTLR_EL1.EnIA/EnDA, so signing is live") — it is no longer true of
`load`.

**Change no assertion.** These tests are already correct; only the box they run in was wrong.

Run the three: expected PASS.

- [ ] **Step 3: Try making `bfamstrip` and `strip47` genuine arm64e guests**

These two are CLI end-to-end tests (`util::record` → `replay`), so `load_with_pac` cannot reach them
and an override would create a record/replay posture mismatch — constraint 2's late-failing hazard,
deliberately introduced. The honest repair is for the *guest* to be arm64e, which is what these
guests have always been *about*. They are freestanding `-nostdlib -static` asm and **never execute on
the host** (only inside the VM), so the arm64e-runtime gating does not apply — only the build must
work.

Probe it first, outside the build:

```bash
clang -arch arm64e -nostdlib -static -Wl,-e,_start \
  -o /tmp/bfamstrip_a64e crates/retrace-guest/asm/bfamstrip.s && otool -hv /tmp/bfamstrip_a64e | tail -1
```

Expected on success: `ARM64 E ...`. If it builds, change **only the `-arch` flag** in the `bfamstrip`
and `strip47` rules in `crates/retrace-guest/build.rs`, and extend each rule's comment: these are the
repo's first arm64e guests, and they exist to exercise the PAC-ON branch of `pac_posture` end to end
— which would otherwise be dead code in every test the gate runs.

Then re-run both e2e tests. Expected: PASS, and now for the right reason — `bfamstrip`'s `autdb`
genuinely FPAC-faults and is genuinely stripped, `strip47`'s `pacda` genuinely signs so the 47-bit
mask assertion is genuinely load-bearing. Confirm `strip47`'s recorded stdout is the 8-byte canonical
pointer, i.e. the assertion still passes *after* real signing — if it does not, the strip is lossy at
this posture and that is a finding, not something to mask.

- [ ] **Step 4: Fallback, only if Step 3's `clang -arch arm64e` probe FAILS**

Do not force it. Instead:

- Convert `bfamstrip`'s coverage to an in-process `retrace-box` test
  (`crates/retrace-box/tests/bfamstrip.rs`) that runs the guest under `Box_::load_with_pac(.., true)`
  and asserts the exit arg is 0 — the strip arm demonstrably firing. Delete the `bfamstrip_e2e`
  integration test only then, and say plainly in the report that the **replay half** of that coverage
  is lost until an arm64e guest exists.
- For `strip47_e2e`, do the same, or — if that is not practical — leave the e2e in place and add an
  in-process test that makes the 47-bit property falsifiable again. **Do not leave a vacuous green
  with no note:** whatever the outcome, the report must state exactly which assertions can still fail
  and which cannot.

Either way, `try_emulate_fpac_auth` and `crates/retrace-guest/asm/bfamstrip.s` stay.

- [ ] **Step 5: Full regression**

```bash
just gate
```

Expected: **0 failed.** Named guards to confirm individually in the report — `pac`, `sign_oracle`,
`cache_pager`, `bfamstrip`, `strip47`, `vaipa`, `reservecommit`, `mmap`, `munmap`, `remap`,
`crashy_e2e`, `crashy_cli`, `crash`, `watch_dyn`, and **`hello_dyn_e2e`, the sharpest single guard**:
`hello_dyn` is arm64, so it has moved to the new PAC-off posture and must still record and replay
bit-for-bit. If `hello_dyn_e2e` regresses, **stop** — the posture is wrong somewhere, and no amount
of test repair is the answer.

Report the exact tally from your own run; do not compute it. (It was 140/0/1 before Task 6.)

- [ ] **Step 6: Commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add -A crates/retrace-box/tests crates/retrace-guest/build.rs crates/retrace/tests
git commit -m "M7 t7: keep the PAC tests falsifiable under the derived posture

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: The honest-gate decision and closeout

**Files:**
- Modify: `crates/retrace/tests/hello_rust_e2e.rs` (un-ignore, **or** rewrite the ignore reason)
- Modify: `README.md` (M7 Status section)

**Interfaces:**
- Consumes: `util::assert_rung_records_and_replays` (Task 1), the Task 6/7 fix.

**BOTH OUTCOMES ARE SUCCESS.** Rung 1 green, or an honest re-park naming the *new* boundary
precisely. The spec makes the second a legitimate M7 outcome — "not a failure, and not a reason to
loosen the gate" — and risk R1 says walls come in chains. **What is NOT acceptable is a green
obtained by weakening the gate**: the rung assertion demands `code == 0` and exact stdout because a
recorded crash exits 139 and would otherwise satisfy an agreement-only gate. Do not touch it.

- [ ] **Step 1: Run the headline gate**

```bash
cargo test -p retrace --test hello_rust_e2e -- --ignored --test-threads=1
```

Record the result verbatim. On failure, capture the assertion message and the `guest crashed:
pc=… esr=… far=…` line, and the `[fault]` line from a `RETRACE_TRACE=1 record-dyn` run.

- [ ] **Step 2A: If it PASSED — demand a genuine DOUBLE pass, then un-ignore**

Run it **twice more**, whole. A single pass is not a double pass, and the rung helper's two replay
iterations are not a substitute for re-running the gate:

```bash
cargo test -p retrace --test hello_rust_e2e -- --ignored --test-threads=1
cargo test -p retrace --test hello_rust_e2e -- --ignored --test-threads=1
```

Then, and only then, delete the `#[ignore = "..."]` attribute from
`hello_rust_records_and_replays_reaching_main`. Leave the test body and its comment untouched.

Re-run without `--ignored` to prove it is now part of the ordinary gate:
`cargo test -p retrace --test hello_rust_e2e -- --test-threads=1`.

- [ ] **Step 2B: If it FAILED — re-park it honestly**

Keep the `#[ignore]`. **Rewrite its reason to name the NEW boundary**, with the same precision the
Task-4 reason had: the fault's `pc`/`esr`/`far`, the decoded EC, and what was ruled out. Delete the
now-obsolete PAC-garbled-branch text — a stale ignore reason is worse than none, because it sends the
next reader after a wall that no longer exists.

**Use the `pc`/`esr`/`far` signature as the landmark, never a trap index** — the pre-crash trap count
drifts run to run (245-247 measured), so an absolute index is not reproducible and must not appear as
a criterion. State whether the new wall is the *same class* (a PAC/pointer-handoff disagreement) or a
different mechanism; per spec risk R1 a different mechanism is a normal ladder outcome.

Do **not** weaken the assertion, delete the test, or reduce the expected stdout.

- [ ] **Step 3: Write the README M7 Status section**

Follow the existing Status sections' voice: what runs today, what it proves, and what the next wall
is. It must state, honestly:

- **What rung 1 proved.** A `rustc`-built binary with full `std` loads and runs through real
  `/usr/lib/dyld` — and, under 2A, records and replays bit-for-bit while reaching `main`.
- **What the wall actually was**, in one paragraph: retrace enabled PAC for every guest; macOS
  enables it per process, only for arm64e main executables; dyld's unconditional `paciza` in TLV
  setup is a NOP in a real plain-arm64 process but really signed inside retrace, and the guest's
  plain `blr` branched through the signature. The fix derives the posture from the main executable's
  `cpusubtype`, in one helper, used by all four SCTLR install sites — below the trace, no format
  change.
- **Why `hello_dyn` never hit it:** it is also plain arm64 but has **zero `__thread_vars`**, so it
  never contained an arm64e→arm64 pointer handoff. It survived M2's whole wall-chain by luck of
  shape, not because the posture was right.
- **The gate-credibility fix** (Task 1): a recorded crash is a successful recording under M6, so the
  rung assertion demands exit 0 and exact stdout. Agreement between two runs is not evidence that
  either run did anything.
- **The next boundary**, named precisely: under 2A, what rung 2 (`brew jq`, M8) will face plus the
  standing deferrals — threads, Rust `panic!` → `SIGABRT` signal delivery, arm64e main executables;
  under 2B, the new wall's signature and class.

- [ ] **Step 4: The full gate, counted from a real run**

```bash
just gate
```

Report the exact `passed / failed / ignored` tally **read from your own output**. Do not compute it
from arithmetic and do not carry a number forward from this plan. For reference only: 140/0/1 before
Task 6, and Tasks 6-7 add tests.

Expected under 2A: **0 failed, 0 ignored.** Under 2B: **0 failed, 1 ignored** — the headline gate,
parked with its rewritten reason. Both are a clean close.

When summing from a captured log, use `grep -a` with `LC_ALL=C` and strip ANSI escapes — a
UTF-8-locale `grep` silently returns zero matches on cargo's colored output.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace/tests/hello_rust_e2e.rs README.md
git commit -m "M7 t8: <un-ignore the rung-1 gate | re-park rung 1 at the new boundary> + README M7 Status

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

Pick the subject that matches what actually happened. Do not write "un-ignore" unless the gate
genuinely double-passed.

---

## Post-plan notes for the executor

- Task order is strict: T1 → T2 → T3 → T4 → T5 → T6 → T7 → T8. T1 is first on purpose (it is the
  instrument the rest is judged by); T3 is before T4 because T4's characterisation step reads the
  `[fault]` line. T6 lands the mechanism and leaves the workspace red by design; T7 repairs exactly
  the tests T6 breaks and is the first point at which `just gate` is green again; T8 decides the
  gate's honest state and closes the milestone either way.
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
