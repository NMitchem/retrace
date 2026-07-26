# retrace M6 — crash recording & reverse-debug-to-the-corrupting-write: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stage-1 EL0 faults become recorded, replayed, seekable `Event::Crash` stops; the software
watch check translates VA→IPA through the guest's own tables; a planted-bug dynamic C guest proves
record-crash → `watch` → `reverse-continue` → corrupting store end-to-end.

**Architecture:** Additive widenings along existing seams (spec:
`docs/superpowers/specs/2026-07-19-retrace-m6-crash-design.md`, branch `m6-crash`, base `9573ce6`).
A new `Stop::Fault` from `run()`/`run_one_for_step`'s *inner* (EL1-trampoline) matches; mirrored
record/replay terminal paths per symmetry rule 1; one trace-format break (`0x03→0x04`) up front; a
read-only page-table walker feeding the M5 software watch intersection.

**Tech Stack:** Rust 1.95.0 pinned, aarch64-apple-darwin, Hypervisor.framework via hv-sys. No new
dependencies anywhere.

## Global Constraints

- Tests MUST run with `--test-threads=1` (one VM per process). Full gate: `just gate`.
- A test that spawns the CLI itself must codesign it first — always go through
  `crates/retrace/tests/util/mod.rs::bin()` (existing helper; already handles it).
- `clippy.toml` denies `Instant::now`/`SystemTime::now`/`std::thread` — do not introduce any.
- TDD: write the failing test, RUN it and confirm the failure mode, then implement. Never weaken
  an existing assertion; never touch `asm/wildstore.s` semantics (its fatal stage-2 outcome is a
  load-bearing M6 negative).
- Symmetry rule 1: record's crash arm and replay's crash verify are mirrors; replay byte-compares
  the recomputed `(pc, esr, far)` — that comparison IS the divergence check.
- Commit style: `M6 t<N>: <what>` on branch `m6-crash`. Commit at the end of every task (and at
  any green intermediate step you like); end commit messages with the Co-Authored-By trailer used
  in this repo's history.
- The trace-format break happens exactly once, in Task 1.
- `TRACE_MAGIC` new value: `*b"RT\x00\x04"`. Fault garbage VA everywhere: `0x4000DEAD0000`
  (bit 46 set → L1 index 0x400; only L1[0] is ever valid, so it is stage-1-unmapped by
  construction, and < 2^47).
- Crash process-exit convention: CLI exits `139` (128+SIGSEGV) for a crash outcome, on both
  `record`/`record-dyn` (a recorded crash is a SUCCESSFUL recording) and `replay` (a verified
  crash replay is a SUCCESSFUL replay).

## Plan-level refinements of the spec (deviations, with why)

1. **No `Advance::Crashed` variant; no `mmu_on` field.** The spec left the outcome shape open
   (Open Q2) and named `Advance::Crashed` provisionally. This plan surfaces a crash as
   `Advance::Exited(ReplayReport)` with a new `ReplayReport.outcome: Outcome` field, and REMOVES
   `exit_code` from both `RecordSummary` and `ReplayReport` — the compiler then forces every
   consumer (6 files, ~16 sites) to handle the crash case, which is the same enforcement the
   variant would have bought with 7 fewer near-identical match arms. MMU state is read from
   `SCTLR_EL1` bit 0 inside the walker instead of a constructor-set field: `restore` and
   `from_checkpoint` also construct boxes, and a live register read is authoritative at every
   construction site (all four currently set `SCTLR_MMU_ON`, `retrace-box/src/lib.rs:586,1000,1564,1898`).
2. **The static loader is MMU-on** (verified: `load` sets `SCTLR_MMU_ON`, `:586`), so a VA ≥ 2^36
   stage-1-faults even from a freestanding asm guest (only `L1[0]` is valid — `:409-417`). Spec
   Open Q1 is answered: the cheap fault harness is a tiny static asm guest through the normal
   in-process `record()`, no dyld needed. `wildstore.s` (VA < 2^36: stage-1-mapped,
   stage-2-unbacked → OUTER abort, fatal) and the new `crash.s` (VA ≥ 2^36 → INNER stage-1 fault
   → `Stop::Fault`) demonstrate the two funnels side by side.
3. **All guest mappings today are identity (VA == IPA)** — `build_tables` builds an identity map
   and every mmap/cache/commpage region is mapped at IPA == VA. So Task 4's walker fixes no
   *currently-failing* case; it makes the M5 software-watch intersection sound *by construction*
   (translated, `None`-on-unmapped) instead of accidentally-correct, and its dynamic-guest
   `WatchSyscall` test is genuinely new coverage (M5 only ever tested static guests).

## File Structure

- `crates/retrace-arch/src/lib.rs` — `Ec::InstrAbort` + decode (T1).
- `crates/retrace-trace/src/lib.rs` — `Event::Crash`, magic bump, tests (T1).
- `crates/retrace-guest/asm/crash.s`, `asm/crashjmp.s` — static stage-1 fault guests (T2);
  `c/crashy.c` — the planted-bug dynamic guest (T3); `build.rs` + `src/lib.rs` consts for each.
- `crates/retrace-box/src/lib.rs` — `Stop::Fault` + two inner-match arms (T2); `va_to_ipa` +
  translated intersection in `apply_and_return` (T4).
- `crates/retrace-core/src/lib.rs` — `Outcome`; record crash arm; replay crash verify;
  `step_insns`/`window_len_here` fault arms (T2).
- `crates/retrace-core/tests/crash.rs` — NEW: in-process record/replay/divergence tests, static
  guests (T2).
- `crates/retrace-box/tests/vaipa.rs` — NEW: walker unit tests (T4).
- `crates/retrace/src/main.rs` — outcome-aware exits (T2). `crates/retrace/src/debug.rs` —
  minimal outcome print (T2), full crash UX (T5).
- `crates/retrace/tests/crashy_e2e.rs` — NEW: CLI crash tests (T2: static; T3: dynamic;
  T6: headline gate). `crates/retrace/tests/watch_dyn.rs` — NEW: dynamic-guest syscall-watch
  (T4). `crates/retrace/tests/crashy_cli.rs` — NEW: golden crash-debug transcripts (T5).
- `README.md` — M6 Status section (T6).

---

### Task 1: Trace format break + instruction-abort decode

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (enum at `:2`, `ec_of` at `:27`, tests at bottom)
- Modify: `crates/retrace-trace/src/lib.rs` (`Event` `:14-19`, `TRACE_MAGIC` `:21`, tests `:83+`)

**Interfaces:**
- Produces: `retrace_arch::Ec::InstrAbort` (decoded from EC `0x20|0x21`);
  `retrace_trace::Event::Crash { pc: u64, esr: u64, far: u64 }`; `TRACE_MAGIC = *b"RT\x00\x04"`.
  Every later task consumes exactly these names/shapes.

- [ ] **Step 1: Write the failing tests**

In `crates/retrace-arch/src/lib.rs` tests module add:

```rust
    #[test]
    fn decodes_instruction_and_data_aborts() {
        assert_eq!(ec_of(0x20u64 << 26), Ec::InstrAbort);
        assert_eq!(ec_of(0x21u64 << 26), Ec::InstrAbort);
        assert_eq!(ec_of(0x24u64 << 26), Ec::DataAbort);
    }
```

In `crates/retrace-trace/src/lib.rs` tests module add (mirroring `sample()`/`torn_tail` style):

```rust
    fn crash_sample() -> Vec<Event> {
        vec![
            Event::Snapshot { regs: Regs { x:[0;31], pc:0x100000000, sp_el0:0x2000_0000, cpsr:0 },
                              mem: vec![Region{ ipa:0x100000000, bytes: vec![1,2,3,4] }] },
            Event::Crash { pc: 0x100000010, esr: 0x92000005, far: 0x4000DEAD0000 },
            Event::Snapshot { regs: Regs { x:[0;31], pc:0x100000010, sp_el0:0x2000_0000, cpsr:0 },
                              mem: vec![Region{ ipa:0x100000000, bytes: vec![1,2,3,9] }] },
        ]
    }
    #[test]
    fn crash_roundtrip() {
        let f = tempfile();
        let mut w = Writer::create(&f).unwrap();
        for e in crash_sample() { w.append(&e).unwrap(); }
        drop(w);
        let (got, truncated) = Reader::open_checked(&f).unwrap();
        assert!(!truncated);
        assert_eq!(got, crash_sample());
    }
    #[test]
    fn rejects_v3_traces() {
        // The M6 magic bump: a well-formed 0x03-era trace must be rejected wholesale.
        let f = tempfile();
        let body = b"plausible record body bytes";
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RT\x00\x03");
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&crc32(body).to_le_bytes());
        bytes.extend_from_slice(body);
        std::fs::write(&f, &bytes).unwrap();
        let (got, truncated) = Reader::open_checked(&f).unwrap();
        assert!(truncated);
        assert!(got.is_empty());
    }
    #[test]
    fn torn_crash_tail_recovers_prefix() {
        let f = tempfile();
        let mut w = Writer::create(&f).unwrap();
        for e in crash_sample() { w.append(&e).unwrap(); }
        drop(w);
        let mut bytes = std::fs::read(&f).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        std::fs::write(&f, &bytes).unwrap();
        let (got, truncated) = Reader::open_checked(&f).unwrap();
        assert!(truncated);
        assert_eq!(got, crash_sample()[..2].to_vec()); // torn final Snapshot dropped, Crash kept
    }
```

NOTE: `tempfile()` is shared by all tests in this module and its name is per-process, not
per-test; tests run serially (`--test-threads=1`) and each test removes-then-recreates it, so this
is safe — do not "fix" it.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p retrace-arch -- --test-threads=1` → expected: compile error
(`no variant InstrAbort`).
Run: `cargo test -p retrace-trace -- --test-threads=1` → expected: compile error
(`no variant Crash`).

- [ ] **Step 3: Implement**

`retrace-arch`: change line 2 and add one `ec_of` line:

```rust
pub enum Ec { Svc, Hvc, SysReg, SoftStep, Breakpoint, Watchpoint, DataAbort, InstrAbort, Other(u8) }
```
and in `ec_of`, next to the DataAbort line:
```rust
        0x20 | 0x21 => Ec::InstrAbort,
```

`retrace-trace`: append the variant after `Exit` (last position — old indices stay stable) and
bump the magic:

```rust
    Exit { code: u64 },
    Crash { pc: u64, esr: u64, far: u64 },
}

pub const TRACE_MAGIC: [u8;4] = *b"RT\x00\x04"; // "RT" + format version 0x0004 (M6-crash: Event::Crash terminal)
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p retrace-arch -p retrace-trace -- --test-threads=1` → all PASS (including the
pre-existing `rejects_prior_format_version`, which still uses `RT\x00\x02` and still passes).

- [ ] **Step 5: Workspace still builds**

Run: `cargo build --workspace` → OK (nothing matches `Event` exhaustively without a catch-all;
verify, don't assume — fix any site the compiler names by adding the obvious arm, and note it in
the commit message).

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-arch crates/retrace-trace
git commit -m "M6 t1: trace format 0x04 — Event::Crash terminal + Ec::InstrAbort decode"
```

---

### Task 2: `Stop::Fault` + mirrored record/replay crash paths (static guests)

**Files:**
- Create: `crates/retrace-guest/asm/crash.s`, `crates/retrace-guest/asm/crashjmp.s`
- Modify: `crates/retrace-guest/build.rs` (copy an asm recipe, e.g. the machmsg block at
  `:138-145`), `crates/retrace-guest/src/lib.rs` (consts near `:86-108`)
- Modify: `crates/retrace-box/src/lib.rs` (`Stop` `:272`, `run()` inner match `:1355-1374`,
  `run_one_for_step` inner match `:1436-1450`)
- Modify: `crates/retrace-core/src/lib.rs` (`RecordSummary` `:39`, `record_box` `:52-367`,
  `ReplayReport` `:370`, `advance()` `:448-683`, `step_insns` `:771-789`, `window_len_here`
  `:795-808`)
- Modify: `crates/retrace/src/main.rs` (`record`/`record-dyn`/`replay` exit paths),
  `crates/retrace/src/debug.rs` (`report.exit_code` sites `:354`, `:414` — minimal outcome match),
  `crates/retrace/tests/watch.rs`, `crates/retrace-core/tests/mmap.rs`,
  `crates/retrace-core/tests/replay.rs` (mechanical `exit_code` → `outcome` updates; the compiler
  lists every site)
- Create: `crates/retrace-core/tests/crash.rs`
- Create: `crates/retrace/tests/crashy_e2e.rs` (static CLI tests only, this task)

**Interfaces:**
- Consumes: `Event::Crash`, `Ec::InstrAbort` (T1).
- Produces:
  - `retrace_box::Stop::Fault { pc: u64, esr: u64, far: u64 }`
  - `retrace_core::Outcome` — `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Outcome { Exit { code: u64 }, Crash { pc: u64, esr: u64, far: u64 } }`
  - `RecordSummary { pub stdout: Vec<u8>, pub outcome: Outcome, pub events: usize }` (`exit_code` REMOVED)
  - `ReplayReport { pub stdout: Vec<u8>, pub outcome: Outcome }` (`exit_code` REMOVED)
  - `retrace_guest::CRASH`, `retrace_guest::CRASHJMP` path consts
  - `step_insns` errs with a message starting `guest crashed at step` when a step faults;
    `window_len_here` returns `Ok(n)` when the window ends in a fault (the crash window's length).

- [ ] **Step 1: Write the static fault guests**

`crates/retrace-guest/asm/crash.s`:

```asm
.section __TEXT,__text
.global _start
.p2align 2
// M6 stage-1 crash guest. VA 0x4000_DEAD_0000 has bit 46 set => L1 index 0x400; only L1[0] is
// ever valid (the identity map covers just the 36-bit IPA space), so the store takes a STAGE-1
// translation fault, delivered via the EL1 trampoline (run()'s INNER match) => Stop::Fault with
// FAR == this VA. Contrast asm/wildstore.s: a VA < 2^36 is stage-1-mapped but stage-2-unbacked
// => OUTER abort, which stays fatal (the M6 classification negative).
_start:
    movz x0, #0x4000, lsl #32    // 0x4000_0000_0000
    movk x0, #0xDEAD, lsl #16    // | 0xDEAD_0000
    mov  w1, #0x2A
    strb w1, [x0]                // stage-1 fault -> Stop::Fault (never retires)
    // Unreached.
    mov  x0, #0
    mov  x16, #1                 // SYS_exit
    svc  #0x80
```

`crates/retrace-guest/asm/crashjmp.s` (instruction-abort flavor):

```asm
.section __TEXT,__text
.global _start
.p2align 2
// M6 instruction-abort crash guest: branch to the same never-mapped VA as crash.s. The FETCH
// takes a stage-1 translation fault => EC 0x20 (lower-EL instruction abort) via the trampoline.
_start:
    movz x0, #0x4000, lsl #32
    movk x0, #0xDEAD, lsl #16
    br   x0                      // instruction abort at the target VA
    // Unreached.
    mov  x0, #0
    mov  x16, #1
    svc  #0x80
```

`build.rs`: add both, copying the machmsg asm recipe exactly (same flags:
`-arch arm64 -nostdlib -static -Wl,-e,_start`). `src/lib.rs`: add

```rust
pub const CRASH: &str = concat!(env!("OUT_DIR"), "/crash");
pub const CRASHJMP: &str = concat!(env!("OUT_DIR"), "/crashjmp");
```

- [ ] **Step 2: Write the failing core tests**

`crates/retrace-core/tests/crash.rs` (imports/setup mirroring `crates/retrace-core/tests/replay.rs`
— open that file first and copy its guest-loading + temp-path helpers verbatim):

```rust
// M6: a stage-1 guest fault records as Event::Crash and replays bit-for-bit. Static guests
// (CRASH/CRASHJMP) — the dynamic path is Task 3.
use retrace_core::{record, replay, Outcome};
use retrace_trace::{Event, Reader, Writer};

const GARBAGE_VA: u64 = 0x4000_DEAD_0000; // mirrors asm/crash.s (source-defined, not layout)

#[test]
fn data_abort_records_as_crash_and_replays() {
    let (loaded, trace) = load_and_path(retrace_guest::CRASH, "crash-da");
    let rec = record(&loaded, &trace).expect("record must SUCCEED on a guest crash");
    let Outcome::Crash { pc, esr, far } = rec.outcome else {
        panic!("expected crash outcome, got {:?}", rec.outcome);
    };
    assert_eq!(far, GARBAGE_VA);
    assert_eq!((esr >> 26) & 0x3f, 0x24, "lower-EL data abort EC");
    assert!(pc != 0);
    // The trace's terminal events are Crash then the final Snapshot.
    let events = Reader::open(&trace).unwrap();
    assert!(matches!(events[events.len() - 2], Event::Crash { .. }));
    assert!(matches!(events[events.len() - 1], Event::Snapshot { .. }));
    // Replay verifies the identical triple (the divergence oracle) — twice.
    for _ in 0..2 {
        let rep = replay(&trace).expect("replay of a crash trace succeeds");
        assert_eq!(rep.outcome, rec.outcome);
    }
}

#[test]
fn instruction_abort_records_as_crash() {
    let (loaded, trace) = load_and_path(retrace_guest::CRASHJMP, "crash-ia");
    let rec = record(&loaded, &trace).unwrap();
    let Outcome::Crash { pc, esr, far } = rec.outcome else { panic!("{:?}", rec.outcome) };
    assert_eq!((esr >> 26) & 0x3f, 0x20, "lower-EL instruction abort EC");
    assert_eq!(far, GARBAGE_VA);
    assert_eq!(pc, GARBAGE_VA, "instruction abort: the faulting pc IS the branch target");
    let rep = replay(&trace).unwrap();
    assert_eq!(rep.outcome, rec.outcome);
}

#[test]
fn perturbed_crash_triple_is_a_loud_divergence() {
    // Re-write the trace with a perturbed far via Writer (valid CRC — a raw byte flip would fail
    // the record CRC before the divergence compare ever ran) => replay must report Divergence.
    let (loaded, trace) = load_and_path(retrace_guest::CRASH, "crash-perturb");
    record(&loaded, &trace).unwrap();
    let events = Reader::open(&trace).unwrap();
    let tampered = trace.with_extension("tampered.bin");
    let mut w = Writer::create(&tampered).unwrap();
    for e in &events {
        match e {
            Event::Crash { pc, esr, far } =>
                w.append(&Event::Crash { pc: *pc, esr: *esr, far: far + 8 }).unwrap(),
            other => w.append(other).unwrap(),
        }
    }
    drop(w);
    let err = replay(&tampered).expect_err("perturbed crash triple must diverge");
    assert!(err.detail.contains("crash mismatch"), "got: {}", err.detail);
}
```

(`load_and_path` = whatever `replay.rs` names its equivalent helper — reuse it via a small local
copy if it isn't shared; it is `parse_macho(read(guest))` + a deterministic temp path.)

`crates/retrace/tests/crashy_e2e.rs` (CLI level, static):

```rust
// M6 CLI crash surfaces. Static guests here; the dynamic crashy.c path lands in Task 3 and the
// #[ignore]d headline gate in Task 6.
mod util;

#[test]
fn record_and_replay_of_a_crash_exit_139_with_the_crash_line() {
    let (rec, trace) = util::record(retrace_guest::CRASH);
    assert_eq!(rec.code, 139, "stderr: {}", rec.stderr);
    assert!(rec.stderr.contains("guest crashed: pc="), "stderr: {}", rec.stderr);
    assert!(rec.stderr.contains("far=0x4000dead0000"), "stderr: {}", rec.stderr);
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 139, "stderr: {}", rep.stderr);
    assert!(rep.stderr.contains("far=0x4000dead0000"), "stderr: {}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout);
}
```

- [ ] **Step 3: Run the new tests to verify they fail**

Run: `cargo test -p retrace-core --test crash -- --test-threads=1`
Expected: compile error (`Outcome` does not exist). This RED is a compile-RED; the behavioral RED
comes via the pre-implementation behavior check below.

Behavior check (proves today's failure mode, and pins the two-funnel claim): temporarily run
`cargo run -p retrace -- record <OUT_DIR>/crash -o /tmp/t.bin` — expected: `RECORD ERROR: …`
(exit 4), NOT a recording. (Find the built guest under
`target/debug/build/retrace-guest-*/out/crash`.) If this instead records or panics differently,
STOP and re-read spec risk R1 — do not proceed on a surprise.

- [ ] **Step 4: Implement `Stop::Fault` in the box**

`crates/retrace-box/src/lib.rs:272`:

```rust
pub enum Stop { Syscall { num: u64, args: [u64;8] }, Fault { pc: u64, esr: u64, far: u64 }, Other { esr: u64 }, Step }
```

In `run()`'s inner match (insert BEFORE the generic `_ =>` arm at `:1370`):

```rust
                        // M6: a lower-EL (EL0) data/instruction abort is a GUEST CRASH — a
                        // recordable stop, not a retrace bug. EC bit 0 distinguishes lower-EL
                        // (0x24/0x20, guest code faulted) from same-EL (0x25/0x21, the trampoline
                        // itself faulted — that stays in the fail-loud arm below). The faulting
                        // EL0 pc is ELR_EL1 (the vCPU's live PC is parked in the trampoline).
                        Ec::DataAbort | Ec::InstrAbort if (esr1 >> 26) & 1 == 0 => {
                            let far = self.vcpu.get_sys(sysreg::FAR_EL1).unwrap();
                            let pc = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
                            self.last_far = far;
                            return Stop::Fault { pc, esr: esr1, far };
                        }
```

Add the IDENTICAL arm in `run_one_for_step`'s inner match (before its `_ =>` at `:1446`), with the
comment `// M6: mirror of run()'s crash arm — a stepped instruction that faults does not retire.`

- [ ] **Step 5: Implement `Outcome` + the record/replay mirror in retrace-core**

At `:39`:

```rust
/// How a recorded (or replayed) run ended: a clean exit, or a guest synchronous fault (M6). The
/// triple is deterministic — identical guest state faults identically — so replay byte-compares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome { Exit { code: u64 }, Crash { pc: u64, esr: u64, far: u64 } }

pub struct RecordSummary { pub stdout: Vec<u8>, pub outcome: Outcome, pub events: usize }
```

In `record_box`: rename the `exit_code` local to `outcome`; the `SYS_EXIT` arm sets
`outcome = Outcome::Exit { code: args[0] };`. Add the crash arm right after it (`:97`), mirroring
its Exit+final-snapshot order:

```rust
            // M6: a stage-1 guest fault ends the recording as a CRASH — a successful recording
            // (mirror: replay's crash verify in advance()). Same terminal shape as exit:
            // Event::Crash, then the final full-memory snapshot.
            Stop::Fault { pc, esr, far } => {
                let final_snap = b.snapshot();
                w.append(&Event::Crash { pc, esr, far }).map_err(|e| format!("append crash: {e}"))?; count += 1;
                w.append(&final_snap).map_err(|e| format!("append final snapshot: {e}"))?; count += 1;
                outcome = Outcome::Crash { pc, esr, far };
                break;
            }
```

`ReplayReport` (`:370`): `pub struct ReplayReport { pub stdout: Vec<u8>, pub outcome: Outcome }`.
The existing Exit verify (`:466`) builds `outcome: Outcome::Exit { code: *code }`.

In `advance()`, add the mirror arm to the `match self.b.run()` (alongside `Stop::Other`):

```rust
                Stop::Fault { pc, esr, far } => {
                    // M6 mirror of record's crash arm. The triple compare IS the divergence check
                    // (symmetry rule 1); then the final-memory landmark, exactly like Exit.
                    match self.events.get(self.idx) {
                        Some(Event::Crash { pc: rpc, esr: resr, far: rfar }) => {
                            if pc != *rpc || esr != *resr || far != *rfar {
                                return Err(Divergence { landmark: self.idx, pc,
                                    detail: format!("crash mismatch: live (pc={pc:#x}, esr={esr:#x}, far={far:#x}) != recorded (pc={rpc:#x}, esr={resr:#x}, far={rfar:#x})") });
                            }
                            match self.events.get(self.idx + 1) {
                                Some(Event::Snapshot { mem: final_mem, .. }) => {
                                    if let Some(d) = self.b.diff_memory(final_mem) {
                                        return Err(Divergence { landmark: self.idx + 1, pc, detail: d });
                                    }
                                    return Ok(Advance::Exited(ReplayReport {
                                        stdout: std::mem::take(&mut self.stdout),
                                        outcome: Outcome::Crash { pc, esr, far } }));
                                }
                                other => return Err(Divergence { landmark: self.idx + 1, pc,
                                    detail: format!("expected final memory Snapshot after Crash, got {other:?}") }),
                            }
                        }
                        other => return Err(Divergence { landmark: self.idx, pc,
                            detail: format!("expected recorded Crash, got {other:?} (live fault: pc={pc:#x} far={far:#x})") }),
                    }
                }
```

`step_insns` (`:774` match) and `window_len_here` (`:798` match) gain:

```rust
                    // step_insns: stepping INTO the crash — the instruction never retires; the
                    // session stays parked immediately before it.
                    Stop::Fault { pc, esr: _, far } => return Err(format!(
                        "guest crashed at step {done}/{k}: pc={pc:#x} far={far:#x}")),
```
```rust
                // window_len_here: the crash ENDS the final window — its length is the count of
                // retired instructions before the fault (the fault itself never retires).
                Stop::Fault { .. } => return Ok(n),
```

- [ ] **Step 6: Fix every `exit_code` consumer the compiler names**

`cargo build --workspace` and follow the errors. Exhaustive expected list:

- `main.rs` `record`/`record-dyn` Ok-arms and `replay` Ok-arm — all three become:
  ```rust
                    use std::io::Write;
                    std::io::stdout().write_all(&s.stdout).unwrap();
                    match s.outcome {
                        retrace_core::Outcome::Exit { code } => exit(code as i32),
                        retrace_core::Outcome::Crash { pc, esr, far } => {
                            eprintln!("guest crashed: pc={pc:#x} far={far:#x} esr={esr:#x}");
                            exit(139);
                        }
                    }
  ```
  (in the `replay` arm the binding is `r`, not `s`).
- `debug.rs` `:354` and `:414` (`exited (code {})` lines): minimal this task —
  ```rust
                        Advance::Exited(report) => {
                            match report.outcome {
                                Outcome::Exit { code } => line(out, format_args!("exited (code {code})"))?,
                                Outcome::Crash { pc, esr, far } =>
                                    line(out, format_args!("guest crashed: pc={pc:#x} far={far:#x} esr={esr:#x}"))?,
                            }
                            let e = self.sess().landmark();
                            return self.reseek(e, 0);
                        }
  ```
  (add `Outcome` to the `use retrace_core::…` list; Task 5 refines the crash park position).
- `crates/retrace/tests/watch.rs`, `crates/retrace-core/tests/mmap.rs`,
  `crates/retrace-core/tests/replay.rs`: replace `.exit_code` asserts with
  `assert_eq!(rep.outcome, Outcome::Exit { code: 0 });` (or the code the test asserted).

- [ ] **Step 7: Run the new tests to verify they pass**

Run: `cargo test -p retrace-core --test crash -- --test-threads=1` → 3 PASS.
Run: `cargo test -p retrace --test crashy_e2e -- --test-threads=1` → 1 PASS.

- [ ] **Step 8: Full regression gate**

Run: `just gate` → 0 failed, clippy clean. Pay attention to `wildstore`-related tests
(`reservecommit_e2e`, box `reservecommit.rs`): they MUST still pass unchanged — wildstore's
stage-2 abort arrives via the OUTER match and never touches the new arms.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "M6 t2: Stop::Fault + mirrored record/replay crash paths (static guests, exit 139)"
```

---

### Task 3: `crashy.c` — the planted-bug dynamic guest, recorded through real dyld

**Files:**
- Create: `crates/retrace-guest/c/crashy.c`
- Modify: `crates/retrace-guest/build.rs` (copy the hello_dyn recipe `:147-156`),
  `crates/retrace-guest/src/lib.rs` (`pub const CRASHY`)
- Modify: `crates/retrace/tests/util/mod.rs` (shared `discover_crashy_addrs` helper),
  `crates/retrace/tests/crashy_e2e.rs` (add the dynamic test)

**Interfaces:**
- Consumes: T2's crash machinery; `util::record_dynamic` (existing).
- Produces: `retrace_guest::CRASHY`; the address-discovery convention later tasks reuse:
  the trace's marker write `write(1, "CRASHY:", 7)` is followed by `write(1, &g.st, 8)` then
  `write(1, &g.ptr, 8)` — so `args[1]` of those two events ARE the guest VAs of `g.st` and
  `g.ptr`. Helper `util::discover_crashy_addrs(trace: &std::path::Path) -> (u64 /*&g.st*/, u64
  /*&g.ptr*/)` is added to the SHARED `crates/retrace/tests/util/mod.rs` this task (one copy;
  T4/T5 call it via `mod util;` like every other suite).

- [ ] **Step 1: Write the fixture**

`crates/retrace-guest/c/crashy.c`:

```c
// M6 planted-bug crash fixture (dynamic, links libSystem like hello_dyn). Fully deterministic:
//  1. fstat(1, &g.st): a recorded kernel write into a watchable global — the dynamic-guest
//     syscall-write watch target (Task 4). Contents vary per record run but are RECORDED, so
//     replay is bit-identical per trace.
//  2. Marker + address-reveal writes: tests discover &g.st and &g.ptr from the recorded
//     write(1, buf, len) args (args[1] IS the buffer's guest VA) — never hardcoded.
//  3. A volatile off-by-one store loop corrupts g.ptr (buf[4] aliases ptr by declaration order;
//     both are 8-aligned longs, so no padding sits between them).
//  4. *g.ptr faults: GARBAGE_VA has bit 46 set (L1 index 0x400, never mapped; < 2^47) — a
//     stage-1 EL0 data abort with FAR == GARBAGE_VA. Same constant as asm/crash.s.
#include <sys/stat.h>
#include <unistd.h>

#define GARBAGE_VA 0x4000DEAD0000UL

static struct {
    struct stat st;   /* fstat target */
    long buf[4];
    long *ptr;        /* directly follows buf: buf[4] IS ptr */
} g;

int main(void) {
    g.ptr = &g.buf[0];
    fstat(1, &g.st);
    write(1, "CRASHY:", 7);
    write(1, &g.st, 8);            /* args[1] == &g.st  */
    write(1, &g.ptr, 8);           /* args[1] == &g.ptr */
    volatile long *p = g.buf;
    for (int i = 0; i <= 4; i++)   /* planted off-by-one: i==4 corrupts g.ptr */
        p[i] = (long)GARBAGE_VA;
    *(volatile long *)g.ptr = 42;  /* stage-1 fault at GARBAGE_VA */
    return 0;                      /* unreached */
}
```

`build.rs`: copy the hello_dyn block verbatim with `hello_dyn` → `crashy` (same
`clang -arch arm64 -o` recipe — no `-O`, so the volatile OOB store survives; the volatile is
belt-and-braces against a future flag change). `lib.rs`:
`pub const CRASHY: &str = concat!(env!("OUT_DIR"), "/crashy");`

- [ ] **Step 2: Write the failing dynamic e2e**

Add to the SHARED helper module `crates/retrace/tests/util/mod.rs`:

```rust
/// (&g.st, &g.ptr) of the crashy.c fixture, discovered from the recorded marker convention —
/// see c/crashy.c's header comment. Shared by crashy_e2e / watch_dyn / crashy_cli.
pub fn discover_crashy_addrs(trace: &std::path::Path) -> (u64, u64) {
    let events = retrace_trace::Reader::open(trace).unwrap();
    let mut it = events.iter().filter_map(|e| match e {
        retrace_trace::Event::Syscall { num: 4, args, .. } if args[0] == 1 => Some(*args),
        _ => None,
    });
    while let Some(a) = it.next() {
        if a[2] == 7 { // the "CRASHY:" marker write
            let st = it.next().expect("&g.st reveal write")[1];
            let ptr = it.next().expect("&g.ptr reveal write")[1];
            return (st, ptr);
        }
    }
    panic!("CRASHY: marker write not found in trace");
}
```

Append to `crates/retrace/tests/crashy_e2e.rs`:

```rust
const GARBAGE_VA: u64 = 0x4000_DEAD_0000; // mirrors c/crashy.c (source-defined)

#[test]
fn crashy_records_through_dyld_and_replays_bit_for_bit() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "stderr: {}", rec.stderr);
    assert!(rec.stderr.contains("guest crashed: pc="), "stderr: {}", rec.stderr);
    assert!(rec.stderr.contains("far=0x4000dead0000"), "stderr: {}", rec.stderr);
    let (st, ptr) = util::discover_crashy_addrs(&trace);
    assert_ne!(st, 0);
    assert_eq!(ptr, st + 144 + 32, "layout: ptr directly follows st(144) + buf(32)");
    for _ in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 139, "stderr: {}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout);
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p retrace --test crashy_e2e crashy_records -- --test-threads=1`
Expected: compile error (`CRASHY` not found) first; after Step 1's fixture exists, re-run —
expected GREEN or a real dynamic-path failure. If the record errors (exit 4) with an unexpected
stop, READ THE STDERR: an unserviced syscall on the crashy path (e.g. an fstat-adjacent trap
hello_dyn never made) is a genuine M2-style wall — handle it as its own mini-diagnosis, don't
paper over it. If `ptr != st + 176`, check `sizeof(struct stat)` on this SDK (the 144 assert may
need the real value — fix the TEST to the observed deterministic layout, with a comment).

- [ ] **Step 4: Run the full crashy_e2e + gate**

Run: `cargo test -p retrace --test crashy_e2e -- --test-threads=1` → all PASS.
Run: `just gate` → 0 failed.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "M6 t3: crashy.c planted-bug dynamic guest — crash records through real dyld, replays bit-for-bit"
```

---

### Task 4: VA→IPA walker + sound software-watch intersection

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (`va_to_ipa` + `pt_entry` new methods near
  `read_guest_checked` `:1742`; `apply_and_return` intersection `:1689-1696`)
- Create: `crates/retrace-box/tests/vaipa.rs`
- Create: `crates/retrace/tests/watch_dyn.rs`

**Interfaces:**
- Consumes: T3's `CRASHY` + discovery convention; existing `watch_ranges`/`syscall_watch_hit`
  (M5); table constants `PT_L1_IPA`, `DESC_BLOCK=0x1`, `DESC_TABLE=0x3`, `DESC_PAGE=0x3`,
  `BLK = 1<<25`, `GRANULE = 0x4000` (all existing, `retrace-box/src/lib.rs:35,78-86,203-209`).
- Produces: `pub fn va_to_ipa(&self, va: u64) -> Option<u64>` on `Box_`.

- [ ] **Step 1: Write the failing walker unit tests**

`crates/retrace-box/tests/vaipa.rs` (open `crates/retrace-box/tests/dyld_load.rs` first and copy
its guest/dyld loading imports and helpers):

```rust
// M6: the read-only stage-1 walker. Today every mapping is identity (VA == IPA) — these tests
// pin that AND the None-on-unmapped soundness the software watch check relies on.

#[test]
fn walker_is_identity_on_mapped_vas_and_none_on_unmapped() {
    // Static guest: MMU is on (load sets SCTLR_MMU_ON) over the identity map.
    let loaded = load_guest(retrace_guest::CRASH); // same helper shape as dyld_load.rs uses
    let b = retrace_box::Box_::load(&loaded);
    // The trampoline/stack region and the guest text are mapped identity.
    assert_eq!(b.va_to_ipa(0x1C000), Some(0x1C000), "stack region, identity");
    // Bit-46 VA: L1 index 0x400 is invalid -> None (the crash fixture's GARBAGE_VA).
    assert_eq!(b.va_to_ipa(0x4000_DEAD_0000), None);
    // Beyond the 47-bit space -> None.
    assert_eq!(b.va_to_ipa(1u64 << 47), None);
}

#[test]
fn walker_is_identity_across_the_dynamic_layout() {
    let (exe, dyld) = load_dynamic_pair(retrace_guest::CRASHY); // as dyld_load.rs does
    let b = retrace_box::Box_::load_dynamic(&exe, &dyld, "crashy");
    for va in [0x1C000u64, retrace_box::COMMPAGE_IPA, 0x0003_0000 /* TSD */] {
        assert_eq!(b.va_to_ipa(va), Some(va), "identity at {va:#x}");
    }
    assert_eq!(b.va_to_ipa(0x4000_DEAD_0000), None);
}
```

(If `dyld_load.rs`'s helpers have different names/shapes, mirror THOSE — the assertions above are
the contract, the loading scaffolding is whatever that file already does. If `0x1C000` turns out
not to be stage-1-mapped in the static layout, pick any address the file's existing tests prove
mapped — e.g. the loaded guest's own text base — and note it.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p retrace-box --test vaipa -- --test-threads=1`
Expected: compile error (`va_to_ipa` not found).

- [ ] **Step 3: Implement the walker**

In `retrace-box/src/lib.rs`, next to `read_guest_checked`:

```rust
    /// Read-only stage-1 walk of the guest's OWN page tables: VA -> IPA. MMU off => identity.
    /// None if unmapped at any level or beyond the 47-bit VA space — an unmapped VA cannot be
    /// the destination of an applied syscall write, so the watch check treats None as no-match.
    /// (Today every retrace mapping is identity; this makes the software watch check sound by
    /// construction rather than by that accident. M6.)
    pub fn va_to_ipa(&self, va: u64) -> Option<u64> {
        const PT_ADDR: u64 = 0x0000_FFFF_FFFF_C000; // descriptor output-address bits 47:14
        let sctlr = self.vcpu.get_sys(sysreg::SCTLR_EL1).unwrap();
        if sctlr & 1 == 0 { return Some(va); }
        if va >> 47 != 0 { return None; }
        let l1e = self.pt_entry(PT_L1_IPA, (va >> 36) & 0x7FF)?;
        if l1e & 0x3 != DESC_TABLE { return None; }
        let l2e = self.pt_entry(l1e & PT_ADDR, (va >> 25) & 0x7FF)?;
        match l2e & 0x3 {
            DESC_BLOCK => Some((l2e & PT_ADDR & !(BLK - 1)) | (va & (BLK - 1))),
            DESC_TABLE => {
                let l3e = self.pt_entry(l2e & PT_ADDR, (va >> 14) & 0x7FF)?;
                if l3e & 0x3 != DESC_PAGE { return None; }
                Some((l3e & PT_ADDR) | (va & (GRANULE as u64 - 1)))
            }
            _ => None,
        }
    }

    fn pt_entry(&self, table_ipa: u64, idx: u64) -> Option<u64> {
        let bytes = self.read_guest_checked(table_ipa + idx * 8, 8)?;
        Some(u64::from_le_bytes(bytes.try_into().unwrap()))
    }
```

(If the compiler complains that `DESC_BLOCK` patterns aren't usable in `match` arms because they
are `u64` consts — they are, const patterns are fine — but if the *values* overlap (`DESC_TABLE`
== `DESC_PAGE` == 0x3), the match arms as written are correct at L2: 0x1 = block, 0x3 = table.)

- [ ] **Step 4: Wire it into the M5 intersection**

Replace the `apply_and_return` find-closure (`:1691-1693`) so the armed VA is translated at check
time:

```rust
                if let Some(&(va, _)) = self.watch_ranges.iter().find(|&&(va, len)| {
                    // M6: translate the armed VA through the guest's own tables (identity when the
                    // MMU is off); an unmapped VA translates to None and cannot match.
                    self.va_to_ipa(va).is_some_and(|ipa| w.ipa < ipa + len && ipa < end)
                })
```

- [ ] **Step 5: Walker tests green**

Run: `cargo test -p retrace-box --test vaipa -- --test-threads=1` → PASS.

- [ ] **Step 6: The dynamic-guest syscall-watch proof (the M5 deferral's test)**

`crates/retrace/tests/watch_dyn.rs`:

```rust
// M6: syscall-write watch detection on a REAL MMU-on dynamic guest — the M5 deferral's proof.
// crashy's fstat(1, &g.st) is a recorded kernel write into a watchable global.
mod util;

#[test]
fn syscall_write_watch_fires_on_a_dynamic_guest() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "stderr: {}", rec.stderr);
    let (st, _ptr) = util::discover_crashy_addrs(&trace);
    let st_watch = st & !7; // watch must be 8-aligned; g.st is 8-aligned by layout
    assert_eq!(st_watch, st, "g.st is 8-aligned");
    let out = std::process::Command::new(util::bin())
        .args(["debug", trace.to_str().unwrap(), "--script",
               &format!("watch 0x{st:x}; continue")])
        .output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains(&format!("hit watch 0x{st:x} (syscall write)")),
            "expected the fstat kernel write to fire the software watch:\n{stdout}");
}
```

- [ ] **Step 7: Run it + the full gate**

Run: `cargo test -p retrace --test watch_dyn -- --test-threads=1` → PASS. (If the hit line does
not appear: FIRST check whether an earlier syscall write overlaps `g.st` — the assert message
prints the transcript; diagnose from the trace, not by loosening the assert.)
Run: `just gate` → 0 failed (the M5 static watch suites `watch.rs`/`watch_cli.rs` prove the
translated intersection regressed nothing — identity translation reproduces M5 behavior exactly).

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "M6 t4: va_to_ipa stage-1 walker — software watch intersection sound on MMU-on guests"
```

---

### Task 5: Debug-CLI crash UX — park at the fault, golden transcripts, the demo

**Files:**
- Modify: `crates/retrace/src/debug.rs` (`cmd_continue` Exited arm `:353-357` and the
  boundary-cross Exited arm `:353` region + `:413-417`; `cmd_stepi` is already crash-correct via
  T2's `step_insns` message — verify, don't change)
- Create: `crates/retrace/tests/crashy_cli.rs`

**Interfaces:**
- Consumes: `Outcome` (T2), `probe_window_len` (existing, `debug.rs:206`), T3's fixture +
  discovery, T4's watch soundness.
- Produces: the crash transcript contract (golden lines):
  - `guest crashed: pc=0x… far=0x… esr=0x…` (from `continue` reaching the crash)
  - after that `continue`, the session parks AT the fault: `where` prints
    `at (C, K_f) pc=0x<crash pc>` where `K_f` is the crash window's full length.

- [ ] **Step 1: Write the failing golden-transcript test**

`crates/retrace/tests/crashy_cli.rs` (copy `watch_cli.rs`'s `debug_run` helper and style;
addresses come from the shared `util::discover_crashy_addrs`):

```rust
// M6 golden crash-debug transcripts. Every coordinate DISCOVERED from the freshly-recorded
// trace (house convention); ground truth for the corrupting store comes from an independent
// memory-scan oracle, exactly like watch_cli.rs's discover_store_ks.
mod util;
use std::path::Path;

const GARBAGE_VA: u64 = 0x4000_DEAD_0000; // mirrors c/crashy.c

fn debug_run(trace: &str, script: &str) -> (i32, String, String) {
    let out = std::process::Command::new(util::bin())
        .args(["debug", trace, "--script", script])
        .output().expect("spawn debug");
    (out.status.code().unwrap_or(-1),
     String::from_utf8(out.stdout).unwrap(),
     String::from_utf8(out.stderr).unwrap())
}

fn recorded_crash_pc(trace: &Path) -> u64 {
    retrace_trace::Reader::open(trace).unwrap().iter().find_map(|e| match e {
        retrace_trace::Event::Crash { pc, .. } => Some(*pc),
        _ => None,
    }).expect("trace has a Crash event")
}

#[test]
fn continue_parks_at_the_crash_and_where_names_it() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "stderr: {}", rec.stderr);
    let ts = trace.to_str().unwrap();
    let crash_pc = recorded_crash_pc(Path::new(&trace));
    let (code, out, err) = debug_run(ts, "continue; where");
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("guest crashed: pc={crash_pc:#x} far=0x4000dead0000")),
            "crash line:\n{out}");
    // Parked AT the fault: where's pc is the crash pc (the faulting instruction, un-retired).
    assert!(out.trim_end().ends_with(&format!("pc={crash_pc:#x}")), "where:\n{out}");
}

#[test]
fn reverse_continue_from_the_crash_finds_the_corrupting_store() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "stderr: {}", rec.stderr);
    let ts = trace.to_str().unwrap();
    let (_st, ptr) = util::discover_crashy_addrs(Path::new(&trace));
    // THE demo: run to the crash, watch the corrupted pointer, run BACKWARD to its last writer,
    // then prove it: g.ptr still holds the pre-store value (pre-retire), and one stepi later it
    // holds GARBAGE_VA.
    let script = format!(
        "continue; watch 0x{ptr:x}; reverse-continue; x 0x{ptr:x} 8; stepi; x 0x{ptr:x} 8");
    let (code, out, err) = debug_run(ts, &script);
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("hit watch 0x{ptr:x} (write at ")), "watch hit:\n{out}");
    let garbage_hex = GARBAGE_VA.to_le_bytes().iter()
        .map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
    let xs: Vec<&str> = out.lines().filter(|l| l.starts_with(&format!("0x{ptr:x}:"))).collect();
    assert_eq!(xs.len(), 2, "two x dumps:\n{out}");
    assert!(!xs[0].contains(&garbage_hex), "before the store g.ptr is NOT yet garbage:\n{out}");
    assert!(xs[1].contains(&garbage_hex), "after one stepi the corrupting store retired:\n{out}");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p retrace --test crashy_cli -- --test-threads=1`
Expected: `continue_parks_at_the_crash…` FAILS — T2's minimal arm parks at `(C, 0)`, so the final
`where` pc is the crash-window START, not the crash pc. (`reverse_continue…` may pass or fail
depending on parking; record the actual RED shape in the task report.)

- [ ] **Step 3: Implement park-at-the-fault**

In `cmd_continue`'s MAIN Exited arm (T2 version), replace with:

```rust
                Advance::Exited(report) => {
                    match report.outcome {
                        Outcome::Exit { code } => {
                            line(out, format_args!("exited (code {code})"))?;
                            let e = self.sess().landmark();
                            return self.reseek(e, 0);
                        }
                        Outcome::Crash { pc, esr, far } => {
                            line(out, format_args!("guest crashed: pc={pc:#x} far={far:#x} esr={esr:#x}"))?;
                            // Park AT the fault: (C, K_f) — the crash window's full length puts
                            // the fresh session immediately before the (never-retiring) faulting
                            // instruction, so pc() IS the crash pc and reverse-continue's P
                            // orders after every write in the recording.
                            let c = self.sess().landmark();
                            let kf = self.probe_window_len(c)?;
                            return self.reseek(c, kf);
                        }
                    }
                }
```

Apply the same `match report.outcome` refinement to the boundary-cross `Advance::Exited` arm
inside the pre-step block (both outcomes: same prints; Exit parks `(e, 0)` via the existing
lines, Crash parks `(c, kf)` the same way as above).

In `cmd_reverse_continue`'s scan, `Advance::Exited(_) => break None` already treats the crash as
scan-end (the report is unused there) — no change; add the one-line comment
`// Exited covers BOTH terminals (exit and crash): either way the scan is over.`

- [ ] **Step 4: Run the transcripts green**

Run: `cargo test -p retrace --test crashy_cli -- --test-threads=1` → 2 PASS.
Sanity: the `reverse_continue` test's `x`-before/`x`-after byte flip is the demo's proof — eyeball
one full transcript (`RETRACE_TRACE` not needed) and paste it into the task report.

- [ ] **Step 5: Full gate + commit**

Run: `just gate` → 0 failed (debug_cli.rs golden transcripts must be byte-identical — nothing in
the Exit path changed shape).

```bash
git add -A
git commit -m "M6 t5: debug CLI parks at the fault — crash transcripts + reverse-continue-to-corrupting-write demo"
```

---

### Task 6: Headline gate, README status, wrap

**Files:**
- Modify: `crates/retrace/tests/crashy_e2e.rs` (the `#[ignore]`d headline test, then un-ignore)
- Modify: `README.md` (new M6 Status section, appended after M5's per house convention)

**Interfaces:**
- Consumes: everything above.
- Produces: the un-ignored M6 headline gate `crash_demo_end_to_end`; the README M6 Status section.

- [ ] **Step 1: Write the headline gate (born `#[ignore]`d)**

Append to `crashy_e2e.rs` (it already has `GARBAGE_VA`; addresses via `util::discover_crashy_addrs`):

```rust
/// THE M6 HEADLINE GATE. One script, the whole story: record a real dynamically-linked C program
/// whose planted memory-corruption bug crashes it; replay verifies the crash bit-for-bit (twice);
/// the debugger seeks to the crash, watches the corrupted pointer, runs BACKWARD to the exact
/// out-of-bounds store that corrupted it, and proves it by watching the value flip on one stepi.
#[test]
#[ignore = "M6 headline gate: un-ignore only on a genuine double pass (honest-gate discipline)"]
fn crash_demo_end_to_end() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "record: {}", rec.stderr);
    assert!(rec.stderr.contains("far=0x4000dead0000"), "record: {}", rec.stderr);
    for _ in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 139, "replay: {}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout);
    }
    let (_st, ptr) = util::discover_crashy_addrs(&trace);
    let script = format!(
        "continue; where; watch 0x{ptr:x}; reverse-continue; x 0x{ptr:x} 8; stepi; x 0x{ptr:x} 8");
    let out = std::process::Command::new(util::bin())
        .args(["debug", trace.to_str().unwrap(), "--script", &script])
        .output().unwrap();
    assert_eq!(out.status.code(), Some(0), "debug: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("guest crashed: pc="), "{stdout}");
    assert!(stdout.contains(&format!("hit watch 0x{ptr:x} (write at ")), "{stdout}");
    let garbage_hex = GARBAGE_VA.to_le_bytes().iter()
        .map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
    let xs: Vec<&str> = stdout.lines().filter(|l| l.starts_with(&format!("0x{ptr:x}:"))).collect();
    assert_eq!(xs.len(), 2, "{stdout}");
    assert!(!xs[0].contains(&garbage_hex) && xs[1].contains(&garbage_hex),
            "the reverse-continue landed ON the corrupting store:\n{stdout}");
}
```

- [ ] **Step 2: Genuine double pass, then un-ignore**

Run twice:
`cargo test -p retrace --test crashy_e2e crash_demo_end_to_end -- --ignored --test-threads=1`
Expected: PASS both times. Only then delete the `#[ignore = …]` line (keep the doc comment).

- [ ] **Step 3: README M6 Status section**

Append after the M5 fast-follow paragraph, in the established voice. It MUST cover, honestly:
what M6 does (crashes recorded/replayed/seekable; the two-funnel classification — inner stage-1 =
crash, outer stage-2 = still fatal, wildstore unchanged; `Outcome` replacing exit codes; exit
139); the demo (crashy.c: `continue` → crash line → `watch` → `reverse-continue` → the OOB
store, value-flip proof); the walker (identity today, sound by construction, dynamic-guest
syscall-watch now proven); the final gate tally (count it from the `just gate` run — do NOT
guess); and the next boundaries, carried forward from the spec's Scope-out list: signal delivery
(sigaction handlers never run), unclaimed stage-2 aborts stay fatal (use-after-free into a
deallocated carveout hole), rwatch/awatch, >8-byte watches, old→new value printing, and the
breadth ladder (C → Rust → brew jq) as the next milestones.

- [ ] **Step 4: Final gate**

Run: `just gate` → **0 failed / 0 ignored**, clippy clean. Record the exact passed count for the
README/commit.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "M6 t6: headline gate GREEN — crash demo e2e un-ignored; README M6 status"
```

---

## Post-plan notes for the executor

- Task order is strict: T1 → T2 → T3 → T4 → T5 → T6 (each consumes the previous task's
  interfaces).
- Spec risk R1 (ESR ISS determinism) is settled empirically by T2's replay tests: if the triple
  ever mismatches across record/replay, the divergence fires loudly — investigate before
  proceeding, per the spec's fallback (mask a proven-nondeterministic ISS bit on BOTH sides,
  documented).
- If any step surfaces an unexpected wall (most likely T3's dyld-path record), STOP and report
  rather than improvising handlers — new syscall walls get the M2 treatment (diagnose with
  `RETRACE_TRACE=1`, then a deliberate route decision), possibly as an added task.
