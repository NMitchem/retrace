# M40-revcont Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make rung 8's `reverse-continue` take seconds instead of hours, and make forward
`continue` name the instruction that actually writes a watched address.

**Architecture:** One exact resolver finds a hit by *ordinal*: watch hits come from the hardware's
own watchpoint stop while single-stepping with the watch armed, and breakpoint hits by pc.
`reverse-continue` becomes one forward pass that steps over each hit in place, then resolves only
the last one. `Backing` takes ownership of its host pages, so a dropped `Box_` frees them. The
debugger decodes its trace once and shares it between sessions.

**Tech Stack:** Rust 1.95.0 (pinned), Hypervisor.framework via `hv-sys`, arm64 assembly guest
fixtures built by `retrace-guest/build.rs`.

**Spec:** `docs/superpowers/specs/2026-09-23-retrace-m40-revcont-design.md`. Its measurements are in
the companion `…-measurements.md` (cited below as "t0 M1–M7"). Read both before starting.

## Global Constraints

- Toolchain `1.95.0`, target `aarch64-apple-darwin`. Edition 2021.
- **Every test command takes `-- --test-threads=1`**: HVF allows one VM per process.
- `cargo clippy --workspace --all-targets -- -D warnings` must stay clean. `clippy.toml` bans
  `Instant::now`, `SystemTime::now` and `std::thread::Thread`, so nothing added here times anything.
  The new instruments are **counts**.
- **No trace-format change.** `TRACE_MAGIC` stays `RT\x00\x0a`, and `Event` does not change.
- **No dispatch-arm change** in `record_box` or `ReplaySession::advance`. `verify_thread` stays at
  **seven** call sites.
- **`Box_` field order is load-bearing:** `vcpu`, then `vm`, then `backings`. Never reorder.
- **Every existing debugger transcript stays byte-identical:** `debug_cli`, `watch_cli`,
  `thread_watch_e2e`, `crashy_cli`, `crashy_e2e`, `reverse_debug_e2e`, `checkpoint_seek`,
  `cpython_crash_e2e`. If one moves, **stop and report it**. Do not edit the expectation.
- A test that spawns the CLI uses `util::bin()` (it codesigns a copy).
- Commit messages end with
  `Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>`.
- Measure in **CPU seconds and counts**, never wall-clock. The operator runs concurrent sessions,
  and load average 40 was measured in t0. **Cap every rung-8 debugger run in time** with
  `perl -e 'alarm shift; exec @ARGV' <secs> …` (Task 7 gives the exact commands). An uncapped
  pre-fix run exhausted the machine's swap.
- Grep logs with `grep -a` (they carry ANSI and UTF-8).
- The ledger lives at `.superpowers/sdd/2026-09-23-retrace-m40-revcont/`. It is git-ignored and
  never committed. `t0/` there holds the t0 evidence, including `t0/crash.bin`: the exact rung-8
  recording that t0's coordinates, such as (1126, 1,765,682), refer to.

## File structure

| File | Change | Responsibility |
|---|---|---|
| `crates/retrace-guest/asm/watchsweep.s` | create | the fixture: one store instruction sweeps a buffer, then a second writer |
| `crates/retrace-guest/build.rs` | modify | build `watchsweep` |
| `crates/retrace-guest/src/lib.rs` | modify | `WATCHSWEEP` path constant and parse test |
| `crates/retrace-trace/src/lib.rs` | modify | `decode_count()` instrument |
| `crates/retrace-core/src/lib.rs` | modify | `Stepped` / `step_watched`; `DecodedTrace`; `CheckpointCache` seek count and decoded trace |
| `crates/retrace-box/src/lib.rs` | modify | `Backing` owns its pages; `live_backing_bytes()`; `free_pages` |
| `crates/retrace-box/tests/backingfree.rs` | create | the leak guard |
| `crates/retrace/src/debug.rs` | modify | `resolve_nth` replaces `resolve_hit_k`; the one-pass `reverse-continue`; `watched_of`; 3 unit tests |
| `crates/retrace/tests/watchsweep_e2e.rs` | create | the e2e guards |
| `README.md`, `CLAUDE.md`, `docs/status-log.md`, the spec's §10 | modify (Task 8) | the close |

---

### Task 1: The `watchsweep` fixture, the two instruments, and the RED tests

**Files:**
- Create: `crates/retrace-guest/asm/watchsweep.s`
- Modify: `crates/retrace-guest/build.rs` (after the `watchloop` block, ~line 486)
- Modify: `crates/retrace-guest/src/lib.rs` (constant after `WATCHLOOP` ~line 180; test after `watchloop_guest_parses` ~line 322)
- Modify: `crates/retrace-trace/src/lib.rs` (`Reader::open_checked`, line ~103)
- Modify: `crates/retrace-core/src/lib.rs` (`CheckpointCache` ~line 2876, `checkpointed_seek` ~line 2959)
- Create: `crates/retrace/tests/watchsweep_e2e.rs`
- Modify: `crates/retrace/src/debug.rs` (`mod tests`, ~line 853)

**Interfaces:**
- Produces: `retrace_guest::WATCHSWEEP: &str`; `retrace_trace::decode_count() -> u64`;
  `CheckpointCache::seeks(&self) -> u64`. These are the instruments Tasks 3 and 5 are judged by.

- [ ] **Step 1: Remove the t0 instrumentation**

The worktree still carries t0's uncommitted `[prof]` counters and probe. They are preserved in
`t0/instrumentation.diff` and `t0/m40_probe.rs`.

```bash
git checkout -- crates/retrace-core/src/lib.rs crates/retrace/src/debug.rs
rm -f crates/retrace/tests/m40_probe.rs
git status --short   # expect: clean (the spec, measurements and this plan are committed)
```

- [ ] **Step 2: Write the fixture**

Create `crates/retrace-guest/asm/watchsweep.s`:

```asm
// M40: ONE store instruction sweeps a 64-element buffer, so it runs on 40 OTHER addresses before it
// reaches the watched element buf[40] (t0 M7's class, reduced: a watch hit must be resolved by
// ADDRESS, not by pc). A second, DIFFERENT store then rewrites buf[40]. write(1, &buf[40], 8)
// publishes the element's address in the trace args (the WATCHLOOP convention), then exit(0).
// Every sweep value is non-zero, so each store changes its element and a step+read oracle sees it.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    adrp x1, buf@PAGE
    add  x1, x1, buf@PAGEOFF
    mov  x3, #0                     // i
    movz x2, #0x1111
    movk x2, #0x1111, lsl #16
    movk x2, #0x1111, lsl #32
    movk x2, #0x1111, lsl #48       // x2 = 0x1111111111111111
sweep:
    add  x4, x2, x3                 // value = 0x1111111111111111 + i
    str  x4, [x1, x3, lsl #3]       // THE sweeping store: one pc, 64 addresses
    add  x3, x3, #1
    cmp  x3, #64
    b.lt sweep
    mov  x5, #0xbeef
    str  x5, [x1, #320]             // the second writer: buf[40] (40 * 8 = 320), a different pc
    mov  x0, #1
    add  x1, x1, #320               // &buf[40]
    mov  x2, #8
    mov  x16, #4                    // SYS_write(1, &buf[40], 8)
    svc  #0x80
    mov  x0, #0
    mov  x16, #1                    // SYS_exit(0)
    svc  #0x80
.section __DATA,__data
.p2align 3
buf: .space 512                     // 64 quads, zero-initialised
```

- [ ] **Step 3: Build it and name it**

In `crates/retrace-guest/build.rs`, directly after the `watchloop` block (the one ending
`assert!(status.success(), "watchloop guest build failed");`), add:

```rust
    // watchsweep (M40): one str pc sweeps buf[0..64] (so it runs on 40 other addresses before the
    // watched buf[40]), a second str rewrites buf[40], write(1, &buf[40], 8), exit(0). The guard
    // for watch-hit resolution by ADDRESS rather than by pc.
    let src = format!("{}/asm/watchsweep.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/watchsweep");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang watchsweep");
    assert!(status.success(), "watchsweep guest build failed");
```

In `crates/retrace-guest/src/lib.rs`, after `pub const WATCHLOOP: …`:

```rust
pub const WATCHSWEEP: &str = concat!(env!("OUT_DIR"), "/watchsweep");
```

and after the `watchloop_guest_parses` test:

```rust
    #[test]
    fn watchsweep_guest_parses() {
        let l = parse_macho(&std::fs::read(WATCHSWEEP).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
    }
```

Run: `cargo test -p retrace-guest watchsweep_guest_parses -- --test-threads=1`
Expected: PASS.

- [ ] **Step 4: Add the decode counter**

In `crates/retrace-trace/src/lib.rs`, above `impl Reader` (or beside the other module-level
items):

```rust
/// M40: how many times this process has decoded a trace file — every `Reader::open_checked` call,
/// which `Reader::open` delegates to. The debugger's decode-once contract (spec §3e) is asserted
/// against it; it is a count, never a timing.
static DECODES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// M40: see `DECODES`.
pub fn decode_count() -> u64 { DECODES.load(std::sync::atomic::Ordering::Relaxed) }
```

and make the first line of `open_checked`'s body:

```rust
        DECODES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
```

- [ ] **Step 5: Add the seek counter**

In `crates/retrace-core/src/lib.rs`, add a field to `CheckpointCache`, after
`window_probe_steps: u64,`:

```rust
    seeks: u64, // M40: sessions opened through `checkpointed_seek` — a debugger command's cost proxy
```

initialise it in `CheckpointCache::new` (`seeks: 0`), and add beside `total_single_steps()`:

```rust
    /// M40: sessions opened through `checkpointed_seek` against this cache. The deterministic cost
    /// proxy for one debugger command: `reverse-continue` is bounded at 3 (spec §3b), however many
    /// hits it passes.
    pub fn seeks(&self) -> u64 { self.seeks }
```

Make the first statement of `checkpointed_seek`'s body:

```rust
    cache.seeks += 1;
```

- [ ] **Step 6: Write the e2e tests**

Create `crates/retrace/tests/watchsweep_e2e.rs`:

```rust
// M40: the repo-owned guard for watch-hit resolution BY ADDRESS. WATCHSWEEP's one sweeping store
// runs on 40 other addresses before it reaches the watched buf[40], so a resolver that matches the
// hit's pc (pre-M40 `resolve_hit_k`) lands on an earlier run of the store that never wrote the
// watched element. That is what rung 8's forward `continue` did, 1.7 M instructions early
// (t0 M7). Every coordinate here is DISCOVERED by an oracle the watch machinery cannot influence
// (step + read_mem, the watch_cli convention).
mod util;
use std::path::Path;

fn debug_run(trace: &str, script: &str) -> (i32, String, String) {
    let out = std::process::Command::new(util::bin())
        .args(["debug", trace, "--script", script])
        .output().expect("spawn debug");
    (out.status.code().unwrap_or(-1),
     String::from_utf8(out.stdout).unwrap(),
     String::from_utf8(out.stderr).unwrap())
}

/// `&buf[40]`, from the recorded write(1, target, 8).
fn discover_target(trace: &Path) -> u64 {
    let mut s = retrace_core::ReplaySession::open(trace).unwrap();
    loop {
        if let Some((4, args)) = s.peek_syscall() {
            if args[0] == 1 { return args[1]; }
        }
        s.advance().unwrap();
    }
}

/// Ground truth, independent of the watch machinery: every K in window 1 whose instruction changed
/// `target`'s qword.
fn discover_store_ks(trace: &Path, target: u64) -> Vec<u64> {
    let mut s = retrace_core::seek(trace, 1, 0).unwrap();
    let mut ks = Vec::new();
    let mut prev = s.read_mem(target, 8).unwrap();
    let mut k = 0u64;
    while s.step_insns(1).is_ok() {
        let cur = s.read_mem(target, 8).unwrap();
        if cur != prev { ks.push(k); prev = cur; }
        k += 1;
    }
    ks
}

/// Every K in window 1 whose instruction is at `pc`: how often a store instruction ran.
fn ks_at_pc(trace: &Path, pc: u64) -> Vec<u64> {
    let mut s = retrace_core::seek(trace, 1, 0).unwrap();
    let mut out = Vec::new();
    let mut k = 0u64;
    loop {
        if s.pc() == pc { out.push(k); }
        if s.step_insns(1).is_err() { break; }
        k += 1;
    }
    out
}

fn pc_at(trace: &Path, k: u64) -> u64 { retrace_core::seek(trace, 1, k).unwrap().pc() }

/// The debugger's `x` rendering of an 8-byte little-endian value.
fn x_bytes(v: u64) -> String {
    v.to_le_bytes().iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

#[test]
fn continue_resolves_a_watch_hit_to_the_write_not_an_earlier_run_of_the_store() {
    let (rec, trace) = util::record(retrace_guest::WATCHSWEEP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let (tp, ts) = (Path::new(&trace), trace.to_str().unwrap());
    let t = discover_target(tp);
    let ks = discover_store_ks(tp, t);
    assert_eq!(ks.len(), 2, "watchsweep writes buf[40] exactly twice, got {ks:?}");
    let spc = pc_at(tp, ks[0]);
    // The precondition this file exists for: the sweeping store ran on other addresses FIRST.
    let runs = ks_at_pc(tp, spc);
    assert_eq!(runs.len(), 64, "the sweeping store runs once per element: {runs:?}");
    assert!(runs[0] < ks[0], "its first run precedes the watched write: {runs:?} vs {ks:?}");

    let (code, out, err) = debug_run(ts, &format!("watch 0x{t:x}; continue; stepi; x 0x{t:x} 8"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("hit watch 0x{t:x} (write at 0x{spc:x}) at (1, +?)")), "hit line:\n{out}");
    assert!(out.contains(&format!("resolved (1, {})", ks[0])),
        "continue must resolve to the write (K={}), not the store's first run (K={}):\n{out}", ks[0], runs[0]);
    // The effect, which no coordinate bookkeeping can fake: stepping the reported instruction lands
    // the sweep's value in buf[40].
    assert!(out.contains(&format!("0x{t:x}: {}", x_bytes(0x1111_1111_1111_1111 + 40))),
        "stepi over the reported instruction must write buf[40]:\n{out}");
}

#[test]
fn reverse_continue_walks_back_through_both_writers() {
    let (rec, trace) = util::record(retrace_guest::WATCHSWEEP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let (tp, ts) = (Path::new(&trace), trace.to_str().unwrap());
    let t = discover_target(tp);
    let ks = discover_store_ks(tp, t);
    assert_eq!(ks.len(), 2, "{ks:?}");
    let (pc1, pc2) = (pc_at(tp, ks[0]), pc_at(tp, ks[1]));
    assert_ne!(pc1, pc2, "the second writer is a different instruction");

    let (code, out, err) = debug_run(ts,
        &format!("continue; watch 0x{t:x}; reverse-continue; reverse-continue; reverse-continue"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("hit watch 0x{t:x} (write at 0x{pc2:x}) at (1, {})", ks[1])),
        "first reverse-continue from the exit: the second writer:\n{out}");
    assert!(out.contains(&format!("hit watch 0x{t:x} (write at 0x{pc1:x}) at (1, {})", ks[0])),
        "second: the sweep's write to buf[40], not an earlier run of its store:\n{out}");
    assert!(out.contains("no earlier hit"), "third: nothing before the first write:\n{out}");
}
```

- [ ] **Step 7: Write the debugger's two cost tests**

In `crates/retrace/src/debug.rs`, inside `mod tests` after the last existing test:

```rust
    // -------------------------------------------------------------------------------------------
    // M40: VM-driving tests on the repo-owned WATCHSWEEP fixture. They live here, not in an e2e
    // file, because what they assert (the checkpoint cache's seek count and the trace decode count)
    // is internal to the debugger and invisible through the CLI.
    // -------------------------------------------------------------------------------------------

    fn record_watchsweep(tag: &str) -> std::path::PathBuf {
        let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::WATCHSWEEP).unwrap());
        let trace = std::env::temp_dir()
            .join(format!("retrace-m40-{tag}-{}.bin", std::process::id()));
        retrace_core::record(&loaded, &trace).expect("record watchsweep");
        trace
    }

    /// `&buf[40]`, from the recorded write(1, target, 8).
    fn watchsweep_target(trace: &Path) -> u64 {
        let mut s = retrace_core::ReplaySession::open(trace).unwrap();
        loop {
            if let Some((4, args)) = s.peek_syscall() {
                if args[0] == 1 { return args[1]; }
            }
            s.advance().unwrap();
        }
    }

    fn run_cmds(ex: &mut Exec, script: &str, sink: &mut Vec<u8>) {
        for cmd in parse_script(script).unwrap() { ex.exec(&cmd, sink).unwrap(); }
    }

    #[test] fn reverse_continue_makes_at_most_three_seeks_whatever_the_hits() {
        let trace = record_watchsweep("seeks");
        let t = watchsweep_target(&trace);
        let mut ex = Exec::new(&trace).unwrap();
        let mut sink = Vec::new();
        run_cmds(&mut ex, &format!("continue; watch 0x{t:x}"), &mut sink);
        let before = ex.cache.seeks();
        ex.exec(&Cmd::ReverseContinue, &mut sink).unwrap();
        let seeks = ex.cache.seeks() - before;
        let text = String::from_utf8_lossy(&sink).into_owned();
        assert!(text.contains(&format!("hit watch 0x{t:x} (write at ")), "{text}");
        // Spec §3b: the scan, the resolution and the park. Pre-M40 this paid two seeks per run of
        // the sweeping store between each resume point and each real hit.
        assert!(seeks <= 3, "one reverse-continue made {seeks} seeks:\n{text}");
    }

    #[test] fn a_debug_session_decodes_its_trace_once() {
        let trace = record_watchsweep("decodes");
        let t = watchsweep_target(&trace); // decodes on its own; the baseline is taken after it
        let d0 = retrace_trace::decode_count();
        let mut ex = Exec::new(&trace).unwrap();
        let mut sink = Vec::new();
        run_cmds(&mut ex, &format!("continue; watch 0x{t:x}; reverse-continue; stepi; reverse-stepi"), &mut sink);
        let decodes = retrace_trace::decode_count() - d0;
        // Spec §3e: every session the debugger opens, and M19's symbol table, share one decode.
        assert_eq!(decodes, 1, "a debug session decoded its trace {decodes} times:\n{}",
                   String::from_utf8_lossy(&sink));
    }
```

- [ ] **Step 8: Run them and confirm RED for the right reasons**

```bash
L=.superpowers/sdd/2026-09-23-retrace-m40-revcont
cargo test -p retrace --test watchsweep_e2e -- --test-threads=1 > $L/t1-e2e.log 2>&1; echo "e2e exit=$?"
cargo test -p retrace --bins -- --test-threads=1 > $L/t1-bins.log 2>&1; echo "bins exit=$?"
grep -a -E '^test |panicked|seeks|decoded|resolved' $L/t1-e2e.log $L/t1-bins.log
```

Expected:
- `continue_resolves_a_watch_hit_to_the_write_not_an_earlier_run_of_the_store` **FAILS** on the
  `resolved (1, …)` assertion. The output shows `resolved (1, <runs[0]>)`: the store's first run,
  not `ks[0]`.
- `reverse_continue_walks_back_through_both_writers` **PASSES**. The pre-M40 loop's *final* answer
  was always right, only slow (t0 "What the measurements mean"). This test is the regression guard
  for Task 3.
- `reverse_continue_makes_at_most_three_seeks_whatever_the_hits` **FAILS** with a seek count well
  above 3. Record the number.
- `a_debug_session_decodes_its_trace_once` **FAILS** with a decode count well above 1. Record the
  number.
- The 11 pre-existing `debug.rs` tests pass.

If any of these goes the other way, stop and report it. A RED test that passes on the old code
tests nothing.

- [ ] **Step 9: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/retrace-guest crates/retrace-trace/src/lib.rs crates/retrace-core/src/lib.rs \
        crates/retrace/tests/watchsweep_e2e.rs crates/retrace/src/debug.rs
git commit -m "M40 t1: watchsweep fixture, seek/decode instruments, RED at the pc-resolution class

<the RED numbers from Step 8: the resolved K vs ks[0], the seek count, the decode count>

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: The exact resolver and forward `continue` on it

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` (beside `step_insns`, ~line 2788)
- Modify: `crates/retrace-box/src/lib.rs` (`arm_hw_watchpoint` doc, ~line 2817)
- Modify: `crates/retrace/src/debug.rs` (`resolve_hit_k` ~line 220, `cmd_continue` ~lines 650 and 685–700, `cmd_reverse_continue` ~line 778)
- Test: `crates/retrace/tests/watchsweep_e2e.rs`

**Interfaces:**
- Consumes: `WATCHSWEEP` and the Task 1 tests.
- Produces, used by Task 3:
  - `retrace_core::Stepped { Retired, Watch, AtTrap }`
  - `ReplaySession::step_watched(&mut self) -> Result<Stepped, String>`
  - in `debug.rs`: `enum HitKind<'a> { Watch(&'a [(u64, u64)]), Break(&'a [u64]) }`
  - `fn resolve_nth(trace: &Path, cache: &mut CheckpointCache, n: usize, from_k: u64, kind: HitKind, ordinal: u64, expect_pc: u64) -> Result<u64, String>`

- [ ] **Step 1: Write the failing primitive test**

Append to `crates/retrace/tests/watchsweep_e2e.rs`:

```rust
#[test]
fn a_watch_aware_step_stops_pre_retire_exactly_at_the_watched_writes() {
    // Spec R2, on t0 M6's measurement: single-stepping WITH a watchpoint armed stops at exactly the
    // instructions that write the watched range, before they retire, and nowhere else. That is
    // true even though the sweeping store runs 64 times.
    let (rec, trace) = util::record(retrace_guest::WATCHSWEEP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let t = discover_target(tp);
    let ks = discover_store_ks(tp, t);
    let mut s = retrace_core::seek(tp, 1, 0).unwrap();
    s.arm_watchpoints(&[(t, 8)]);
    let (mut k, mut stops, mut at_stop) = (0u64, Vec::new(), Vec::new());
    loop {
        match s.step_watched().expect("watch-aware step") {
            retrace_core::Stepped::Retired => k += 1,
            retrace_core::Stepped::Watch => {
                stops.push(k);
                at_stop.push(s.read_mem(t, 8).unwrap()); // pre-retire: still the OLD value
                s.clear_watchpoints();
                s.step_insns(1).unwrap();
                s.arm_watchpoints(&[(t, 8)]);
                k += 1;
            }
            retrace_core::Stepped::AtTrap => break, // the write(1, …) that ends window 1
        }
    }
    assert_eq!(stops, ks, "stops at exactly the oracle's writes");
    assert_eq!(at_stop[0], 0u64.to_le_bytes().to_vec(), "the first stop is before buf[40] is written");
    assert_eq!(at_stop[1], (0x1111_1111_1111_1111u64 + 40).to_le_bytes().to_vec(),
               "the second stop is before the second writer lands");
}
```

Run: `cargo test -p retrace --test watchsweep_e2e a_watch_aware_step -- --test-threads=1`
Expected: FAIL to compile (`no method named step_watched`, `Stepped` not found).

- [ ] **Step 2: Implement `Stepped` and `step_watched`**

In `crates/retrace-core/src/lib.rs`, immediately before `pub struct ReplaySession`, add:

```rust
/// M40: what one watch-aware single step did (`ReplaySession::step_watched`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stepped {
    /// One instruction retired.
    Retired,
    /// The next instruction writes an armed watch range. It has NOT retired: the watchpoint is
    /// pre-retire (spike F4c), so the guest is parked on the store.
    Watch,
    /// The next instruction is the window-ending trap. Nothing retired, and the trap is not
    /// consumed.
    AtTrap,
}
```

and inside `impl ReplaySession`, directly after `step_insns`:

```rust
    /// M40: single-step one instruction with whatever watchpoints are armed, and REPORT a
    /// watchpoint stop rather than treating it as a fault. `step_insns` hands every `Stop::Other`
    /// to `page_in_cache`/`commit_reserved_page`, which read the FAR as an IPA; a watchpoint's FAR
    /// is the watched VA, so the class is checked FIRST here. Measured clean at t0 M6: a
    /// pre-retire `EC=0x34` stop at exactly the write, a clean retire once disarmed, and clean
    /// stepping after re-arming. Breakpoints must NOT be armed (one fires before retire at the
    /// current pc, forever). Deterministic replay faults are handled and re-stepped exactly as in
    /// `step_insns`.
    pub fn step_watched(&mut self) -> Result<Stepped, String> {
        loop {
            match self.b.step() {
                Stop::Step => return Ok(Stepped::Retired),
                Stop::Other { esr } => {
                    if matches!(retrace_arch::ec_of(esr), retrace_arch::Ec::Watchpoint) {
                        return Ok(Stepped::Watch);
                    }
                    if self.b.page_in_cache(self.b.fault_ipa()) { continue; }
                    if self.b.commit_reserved_page(self.b.fault_ipa()) { continue; }
                    return Err(format!("fault during a watch-aware step: {}", self.b.describe_stop(esr)));
                }
                Stop::Syscall { .. } => return Ok(Stepped::AtTrap),
                Stop::Fault { pc, far, .. } => return Err(format!(
                    "guest crashed during a watch-aware step: pc={pc:#x} far={far:#x}")),
            }
        }
    }
```

Run: `cargo test -p retrace --test watchsweep_e2e a_watch_aware_step -- --test-threads=1`
Expected: PASS.

- [ ] **Step 3: Replace `resolve_hit_k` with `resolve_nth`**

In `crates/retrace/src/debug.rs`, change the `use retrace_core::{…}` line to also import `Stepped`:

```rust
use retrace_core::{checkpointed_seek, Advance, CheckpointCache, Outcome, ReplayReport, ReplaySession, Stepped};
```

Delete `resolve_hit_k` and its doc comment entirely, and put in its place:

```rust
/// What `resolve_nth` counts (M40 §3a). A watch hit is identified BY ADDRESS: the hardware's own
/// watchpoint stop, while single-stepping with `Watch`'s ranges armed. A breakpoint hit is
/// identified by pc membership in `Break`'s addresses, stepped with breakpoints DISARMED (armed,
/// one fires before retire at the current pc, forever).
enum HitKind<'a> { Watch(&'a [(u64, u64)]), Break(&'a [u64]) }

/// Replay window `n` from `(n, from_k)` and return the K of the `ordinal`-th (1-based) hit of
/// `kind`, checking that the instruction there is at `expect_pc` (the pc the scan saw), so that a
/// scan/resolver disagreement fails loud instead of naming the wrong instruction. Replaces M3's
/// `resolve_hit_k`, which matched a watch hit BY PC: a store that ran on other addresses first
/// resolved to an earlier run that never wrote the watched range (t0 M6/M7: rung 8's `continue`
/// landed 1.7 M instructions early). Deterministic; runs on its own transient session, so the
/// caller must hold NO other live session (one VM per process).
fn resolve_nth(trace: &Path, cache: &mut CheckpointCache, n: usize, from_k: u64, kind: HitKind,
               ordinal: u64, expect_pc: u64) -> Result<u64, String> {
    debug_assert!(ordinal >= 1, "ordinals are 1-based");
    let mut s = checkpointed_seek(trace, cache, n, from_k)?;
    let (mut k, mut seen) = (from_k, 0u64);
    let found = |s: &ReplaySession, k: u64| -> Result<u64, String> {
        if s.pc() == expect_pc { Ok(k) } else { Err(format!(
            "resolve hit #{ordinal} in window {n}: the scan saw pc {expect_pc:#x}, the resolver reached {:#x} at K={k}",
            s.pc())) }
    };
    match kind {
        HitKind::Break(addrs) => loop {
            if addrs.contains(&s.pc()) {
                seen += 1;
                if seen == ordinal { return found(&s, k); }
            }
            s.step_insns(1).map_err(|e| format!("resolve breakpoint hit #{ordinal} in window {n}: {e}"))?;
            k += 1;
        },
        HitKind::Watch(ranges) => {
            s.arm_watchpoints(ranges);
            loop {
                match s.step_watched().map_err(|e| format!("resolve watch hit #{ordinal} in window {n}: {e}"))? {
                    Stepped::Retired => k += 1,
                    Stepped::Watch => {
                        seen += 1;
                        if seen == ordinal { return found(&s, k); }
                        s.clear_watchpoints(); // step over this earlier hit in place, then re-arm
                        s.step_insns(1).map_err(|e| format!("resolve watch hit #{ordinal} in window {n}: {e}"))?;
                        s.arm_watchpoints(ranges);
                        k += 1;
                    }
                    Stepped::AtTrap => return Err(format!(
                        "resolve watch hit #{ordinal} in window {n}: the window ended after {seen} hit(s) at K={k}")),
                }
            }
        }
    }
}
```

- [ ] **Step 4: Point the three call sites at it**

In `cmd_continue`'s `Advance::Break` arm, replace
`let k = resolve_hit_k(self.trace, &mut self.cache, n, p_hit, kctx + 1)?;` with:

```rust
                    let k = resolve_nth(self.trace, &mut self.cache, n, kctx + 1, HitKind::Break(&[p_hit]), 1, p_hit)?;
```

In `cmd_continue`'s `Advance::Watch` arm, replace the comment block that starts
`// Resolve from kctx, NOT kctx+1:` down to and including
`let k = resolve_hit_k(self.trace, &mut self.cache, n, p_hit, kctx)?;` with:

```rust
                    // Resolve from kctx, NOT kctx+1: unlike a breakpoint (whose parked-on case the
                    // pre-step already moved off), a watched store CAN legitimately fire at the
                    // exact parked coordinate (the user stepi'd up to it). The FIRST watch stop from
                    // kctx is this hit, found by the hardware BY ADDRESS (M40): matching the store's
                    // pc instead named an earlier run of a loop store that wrote elsewhere (t0 M7).
                    // This resolution runs whether or not the hit is scoped out: the vCPU is
                    // physically parked pre-retire at the store either way.
                    let kctx = if n == start_n { start_k } else { 0 };
                    self.session = None; // free the VM before the resolution seek
                    let k = resolve_nth(self.trace, &mut self.cache, n, kctx, HitKind::Watch(&ws), 1, p_hit)?;
```

In the same arm's long scoped-out comment, change
`a discarded hit pays a full \`resolve_hit_k\` seek` to `a discarded hit pays a full \`resolve_nth\` seek`.

In `cmd_reverse_continue`, which Task 3 replaces wholesale, keep it correct in the meantime.
Replace the combined arm

```rust
                RHit::Bp(pc) | RHit::Watch { pc, .. } => {
                    let from_k = if n == cur_n { cur_k } else { 0 };
                    let k = resolve_hit_k(self.trace, &mut self.cache, n, *pc, from_k)?;
                    (k, (n, k + 1)) // resume strictly past a resolved instruction hit
                }
```

with

```rust
                RHit::Bp(pc) => {
                    let from_k = if n == cur_n { cur_k } else { 0 };
                    let k = resolve_nth(self.trace, &mut self.cache, n, from_k, HitKind::Break(&[*pc]), 1, *pc)?;
                    (k, (n, k + 1)) // resume strictly past a resolved instruction hit
                }
                RHit::Watch { pc, .. } => {
                    let from_k = if n == cur_n { cur_k } else { 0 };
                    let k = resolve_nth(self.trace, &mut self.cache, n, from_k, HitKind::Watch(&ws), 1, *pc)?;
                    (k, (n, k + 1))
                }
```

- [ ] **Step 5: Supersede the "never while single-stepping" rule for watchpoints**

In `crates/retrace-box/src/lib.rs`, `arm_hw_watchpoint`'s doc currently ends
`Armed only around \`advance()\`/\`run()\` scans — NEVER while single-stepping (same discipline as breakpoints).`
Replace that sentence with:

```rust
    /// Armed around `advance()`/`run()` scans, AND — since M40, on its t0 M6 measurement — while
    /// single-stepping through `ReplaySession::step_watched`, which is how a watch hit is resolved
    /// by address: a watched store then stops the step pre-retire (`EC=0x34`) and nothing else
    /// changes. The M3 rule "never while single-stepping" still binds BREAKPOINTS, whose pre-retire
    /// fire at the current pc would repeat forever; it was carried over to watchpoints by analogy
    /// and never measured for them.
```

- [ ] **Step 6: Run the guards and the existing transcripts**

```bash
cargo test -p retrace --test watchsweep_e2e -- --test-threads=1
for t in watch_cli debug_cli thread_watch_e2e crashy_cli crashy_e2e reverse_debug_e2e checkpoint_seek; do
  cargo test -p retrace --test $t -- --test-threads=1 2>&1 | grep -a -E '^test result|FAILED|panicked'; done
cargo test -p retrace --bins -- --test-threads=1 2>&1 | grep -a -E '^test result|FAILED|panicked|seeks|decoded'
```

Expected:
- All three `watchsweep_e2e` tests PASS. The continue test is now green.
- Every listed existing target PASSES unchanged. **If any transcript moves, stop and report it.**
- `--bins`: the seek test **still FAILS**. The old loop now iterates once per *real* hit, but still
  pays two seeks each; Task 3 fixes that. The decode test **still FAILS** until Task 5. Everything
  else passes.

- [ ] **Step 7: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/retrace-core/src/lib.rs crates/retrace-box/src/lib.rs crates/retrace/src/debug.rs crates/retrace/tests/watchsweep_e2e.rs
git commit -m "M40 t2: resolve watch hits by address (step_watched + resolve_nth); continue names the real writer

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `reverse-continue` as one forward pass plus one resolution

**Files:**
- Modify: `crates/retrace/src/debug.rs` (`cmd_reverse_continue` and the doc comment directly above it)

**Interfaces:**
- Consumes: `Stepped`, `step_watched`, `HitKind`, `resolve_nth` (Task 2); `watched_of`,
  `watch_thread_matches`, `reseek`, `annot`, `line` (existing).
- Produces: nothing new. The command's output lines are byte-for-byte the formats it prints today.

- [ ] **Step 1: Confirm the RED test**

Run: `cargo test -p retrace --bins reverse_continue_makes_at_most_three_seeks -- --test-threads=1`
Expected: FAIL (seeks > 3).

- [ ] **Step 2: Replace the command**

Replace the doc comment directly above `fn cmd_reverse_continue` (it begins
`/// Run backward to the latest hit`) and the whole function body with:

```rust
    /// Run backward to the latest hit — breakpoint, hardware watch, or syscall watch — strictly
    /// before the current position P (M40, spec §3b). Replay only runs forward, so this makes ONE
    /// forward pass from landmark 1 and remembers the last qualifying hit:
    /// - phase 1 runs every window before P's at native speed, stepping over each hit in place;
    /// - phase 2 single-steps P's own window up to P, so exactly the hits before P count.
    ///
    /// Then it resolves only that one hit (`resolve_nth`, by ordinal) and parks there: at most three
    /// seeks, however many hits the pass went through. The pre-M40 loop re-seeked and resolved at
    /// every hit it passed, by pc, so a store that ran on other addresses first cost a full
    /// iteration per run (t0 M2/M6: 183 runs in one window of rung 8, ~20.8 s CPU each).
    ///
    /// A scoped-out watch hit still occupies an ordinal, because the hardware fired for it. The
    /// thread FILTER applies only where `last` is decided (M15 Task 8; spec R4). A breakpoint on a
    /// watched store yields both hits, breakpoint first (R3): its step-off keeps the watches armed.
    fn cmd_reverse_continue<W: Write>(&mut self, out: &mut W) -> Result<(), String> {
        enum RHit { Bp { pc: u64, ord: u64 }, Watch { watched: u64, pc: u64, ord: u64 }, WatchSys { watched: u64 } }
        let (pn, pk) = (self.n, self.k);
        let bps = self.breakpoints.clone();
        let ws: Vec<(u64, u64)> = self.watches.iter().map(|&(a, l, _)| (a, l)).collect();
        self.session = None; // the scan and the resolution use their own transient sessions
        let mut last: Option<(usize, RHit)> = None;
        {
            let mut s = checkpointed_seek(self.trace, &mut self.cache, 1, 0)?;
            s.arm_breakpoints(&bps);
            s.arm_watchpoints(&ws);
            let mut win = s.landmark();
            let (mut bp_ord, mut w_ord) = (0u64, 0u64); // hits so far in window `win`
            let mut ended = false;
            let mut bps_off_for_crossing = false;
            // Phase 1: every window before P's, at native speed.
            while s.landmark() < pn {
                let adv = s.advance().map_err(|d| format!("reverse-continue diverged: {}", d.detail))?;
                if bps_off_for_crossing { s.arm_breakpoints(&bps); bps_off_for_crossing = false; }
                let n = s.landmark();
                if n != win { win = n; bp_ord = 0; w_ord = 0; }
                match adv {
                    Advance::Event => {}
                    // Exited covers BOTH terminals (exit and crash): either way the scan is over.
                    Advance::Exited(_) => { ended = true; break; }
                    Advance::Break => {
                        let pc = s.pc();
                        bp_ord += 1;
                        last = Some((n, RHit::Bp { pc, ord: bp_ord })); // breakpoints are never scoped
                        s.clear_breakpoints(); // step off it with the watches still armed (R3)
                        match s.step_watched()? {
                            Stepped::Retired => s.arm_breakpoints(&bps),
                            Stepped::Watch => {
                                w_ord += 1;
                                let (watched, thread) = (watched_of(&ws, s.far()), s.current_thread());
                                if self.watch_thread_matches(watched, thread) {
                                    last = Some((n, RHit::Watch { watched, pc, ord: w_ord }));
                                }
                                s.clear_watchpoints();
                                s.step_insns(1)?;
                                s.arm_watchpoints(&ws);
                                s.arm_breakpoints(&bps);
                            }
                            // The breakpoint is ON the window-ending trap, so nothing retires
                            // before it. The next advance() crosses the trap with breakpoints still
                            // off (watches armed, so a syscall write is still seen) and re-arms
                            // them. That is `continue`'s boundary crossing.
                            Stepped::AtTrap => bps_off_for_crossing = true,
                        }
                    }
                    Advance::Watch { thread } => {
                        w_ord += 1;
                        let (watched, pc) = (watched_of(&ws, s.far()), s.pc());
                        if self.watch_thread_matches(watched, thread) {
                            last = Some((n, RHit::Watch { watched, pc, ord: w_ord }));
                        }
                        // Step over the watched store in place: retire it with nothing armed, re-arm.
                        s.clear_breakpoints();
                        s.clear_watchpoints();
                        s.step_insns(1)?;
                        s.arm_breakpoints(&bps);
                        s.arm_watchpoints(&ws);
                    }
                    Advance::WatchSyscall { watched, thread } => {
                        if self.watch_thread_matches(watched, thread) {
                            last = Some((n, RHit::WatchSys { watched }));
                        }
                    }
                }
            }
            // Phase 2: P's own window, instruction by instruction, so exactly the hits before P count.
            if !ended {
                if s.landmark() != pn {
                    return Err(format!("reverse-continue: the scan overshot landmark {pn} (at {})", s.landmark()));
                }
                if win != pn { bp_ord = 0; w_ord = 0; }
                s.clear_breakpoints(); // compared by pc below
                let mut k = 0u64;
                while k < pk {
                    let pc = s.pc();
                    if bps.contains(&pc) {
                        bp_ord += 1;
                        last = Some((pn, RHit::Bp { pc, ord: bp_ord }));
                    }
                    match s.step_watched()? {
                        Stepped::Retired => k += 1,
                        Stepped::Watch => {
                            w_ord += 1;
                            let (watched, thread) = (watched_of(&ws, s.far()), s.current_thread());
                            if self.watch_thread_matches(watched, thread) {
                                last = Some((pn, RHit::Watch { watched, pc, ord: w_ord }));
                            }
                            s.clear_watchpoints();
                            s.step_insns(1)?;
                            s.arm_watchpoints(&ws);
                            k += 1;
                        }
                        Stepped::AtTrap => return Err(format!(
                            "reverse-continue: window {pn} ended after {k} instruction(s), before P's {pk}")),
                    }
                }
            }
        } // the scan session drops here: one VM per process
        match last {
            Some((n, RHit::Bp { pc, ord })) => {
                let k = resolve_nth(self.trace, &mut self.cache, n, 0, HitKind::Break(&bps), ord, pc)?;
                let a = self.annot(pc);
                line(out, format_args!("hit {pc:#x} at ({n}, {k}){a}"))?;
                self.reseek(n, k)
            }
            Some((n, RHit::Watch { watched, pc, ord })) => {
                let k = resolve_nth(self.trace, &mut self.cache, n, 0, HitKind::Watch(&ws), ord, pc)?;
                let a = self.annot(pc);
                line(out, format_args!("hit watch {watched:#x} (write at {pc:#x}) at ({n}, {k}){a}"))?;
                self.last_watch_hit = Some((n, k));
                self.reseek(n, k)
            }
            Some((n, RHit::WatchSys { watched })) => {
                line(out, format_args!("hit watch {watched:#x} (syscall write) at ({n}, 0)"))?;
                self.reseek(n, 0)
            }
            None => { line(out, format_args!("no earlier hit"))?; self.reseek(pn, pk) }
        }
    }
```

Before replacing, check that the old function's output lines match the four `line(…)` formats
above character for character. They are copied from it. If `annot` or `line` has a different
signature, keep the call shape the old code used.

- [ ] **Step 3: Run the guards and every debugger transcript**

```bash
cargo test -p retrace --bins -- --test-threads=1 2>&1 | grep -a -E '^test result|FAILED|panicked|seeks|decoded'
cargo test -p retrace --test watchsweep_e2e -- --test-threads=1
for t in watch_cli debug_cli thread_watch_e2e crashy_cli crashy_e2e reverse_debug_e2e checkpoint_seek; do
  cargo test -p retrace --test $t -- --test-threads=1 2>&1 | grep -a -E '^test result|FAILED|panicked'; done
```

Expected:
- `reverse_continue_makes_at_most_three_seeks_whatever_the_hits` PASSES.
- `a_debug_session_decodes_its_trace_once` still FAILS (Task 5).
- All `watchsweep_e2e` tests PASS.
- Every listed transcript target PASSES unchanged. `thread_watch_e2e` is the scoped-watch guard
  (R4). **If it or any other moves, stop and report it.**

- [ ] **Step 4: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/retrace/src/debug.rs
git commit -m "M40 t3: reverse-continue is one forward pass plus one resolution (at most three seeks)

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: A `Box_` frees its guest memory

**Files:**
- Modify: `crates/retrace-box/src/lib.rs`: `Backing` (~line 441) and the struct comment below it;
  `alloc_pages` (~line 976); `place_fixed` case 2 (~line 2275); `unmap_overlapping` (~line 2336);
  `map_mmap_region`'s rejected-FIXED path (~line 2434); `guest_munmap` (~line 2526)
- Create: `crates/retrace-box/tests/backingfree.rs`

**Interfaces:**
- Produces: `retrace_box::live_backing_bytes() -> usize`, a count used by the guard test only.

- [ ] **Step 1: Add the counter and route the explicit frees through it**

In `crates/retrace-box/src/lib.rs`, directly above `fn alloc_pages`:

```rust
/// M40: bytes of guest backing currently mapped by `alloc_pages` and not yet released — a
/// deterministic count for the leak guard (`tests/backingfree.rs`), never a measurement of RSS.
static LIVE_BACKING_BYTES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// M40: see `LIVE_BACKING_BYTES`.
pub fn live_backing_bytes() -> usize { LIVE_BACKING_BYTES.load(std::sync::atomic::Ordering::Relaxed) }

/// M40: release an `alloc_pages` allocation. The caller guarantees `host` is a live `alloc_pages`
/// result of exactly `len` bytes, no longer mapped into the guest, and never touched again.
fn free_pages(host: *mut u8, len: usize) {
    // SAFETY: the caller's guarantee above.
    unsafe { libc::munmap(host as *mut _, len); }
    LIVE_BACKING_BYTES.fetch_sub(len, std::sync::atomic::Ordering::Relaxed);
}
```

In `alloc_pages`, after `assert!(p != libc::MAP_FAILED, "mmap backing failed");`:

```rust
    LIVE_BACKING_BYTES.fetch_add(len, std::sync::atomic::Ordering::Relaxed);
```

Replace the four `munmap`s of `alloc_pages` memory. The fifth, `libc::munmap(src, copy_len)` in
`guest_mmap_file`, unmaps a *host file* mapping, never an `alloc_pages` one. **Leave it alone.**
- `place_fixed` case 2: `libc::munmap(host as *mut _, rlen);` → `free_pages(host, rlen);`, still
  inside its `unsafe { … }` block after the copy.
- `unmap_overlapping`: `unsafe { libc::munmap(bk.host as *mut _, bk.len); }` → `free_pages(bk.host, bk.len);`
- `map_mmap_region`: `unsafe { libc::munmap(host as *mut _, rlen); }` → `free_pages(host, rlen);`
- `guest_munmap`: `unsafe { libc::munmap(bk.host as *mut _, bk.len); }` → `free_pages(bk.host, bk.len);`

- [ ] **Step 2: Write the leak guard**

Create `crates/retrace-box/tests/backingfree.rs`:

```rust
// M40: a dropped Box_ must release every byte of guest backing it allocated. Before M40 `Box_` had
// no Drop and `alloc_pages` mmaps were never unmapped, so every replay session the debugger opened
// and dropped leaked its whole guest memory: ~55 MB and ~1,580 mappings per session on rung 8
// (t0 M4), 2.3 GB after 41 sessions. The count is `live_backing_bytes()`, which is exact, not RSS.
use retrace_box::{live_backing_bytes, Box_};
use retrace_guest::{parse_macho, STEPPY};

#[test]
fn a_dropped_box_releases_every_backing_byte() {
    let loaded = parse_macho(&std::fs::read(STEPPY).unwrap());
    let base = live_backing_bytes();
    for round in 0..3 {
        let mut b = Box_::load(&loaded);
        let loaded_bytes = live_backing_bytes();
        assert!(loaded_bytes > base, "round {round}: a live box holds backing bytes");
        // A runtime backing too, and the explicit-removal path: guest_munmap gives back exactly it.
        let a = b.guest_mmap(0, 0x8000, 3, 0x1002).expect("anon mmap");
        assert_eq!(live_backing_bytes(), loaded_bytes + 0x8000, "round {round}: the mmap's backing is counted");
        b.guest_munmap(a, 0x8000);
        assert_eq!(live_backing_bytes(), loaded_bytes, "round {round}: guest_munmap releases it");
        let _ = b.guest_mmap(0, 0x4000, 3, 0x1002).expect("anon mmap");
        drop(b);
        assert_eq!(live_backing_bytes(), base, "round {round}: dropping the box must release every byte");
    }
}
```

- [ ] **Step 3: Run it and confirm RED at the drop**

Run: `cargo test -p retrace-box --test backingfree -- --test-threads=1`
Expected: FAIL at `dropping the box must release every byte`. The earlier assertions pass,
because the counter and the explicit `guest_munmap` path already agree.

- [ ] **Step 4: Make `Backing` the owner**

Replace `pub struct Backing { pub host: *mut u8, pub ipa: u64, pub len: usize }` and its one-line
comment with:

```rust
// A page-aligned host allocation mapped 1:1 into the guest at `ipa`. M40: the Backing OWNS its host
// pages. Dropping it releases them, so `backings` must never hold two Backings over one
// allocation, and every Backing must carry exactly the length `alloc_pages` returned (audited at
// M40: all 18 construction sites do).
pub struct Backing { pub host: *mut u8, pub ipa: u64, pub len: usize }

impl Drop for Backing {
    fn drop(&mut self) {
        // Released only once its stage-2 mapping is gone: by `vm.unmap` at the two removal sites
        // (`unmap_overlapping`, `guest_munmap`), or by `hv_vm_destroy` when the owning `Box_` drops,
        // because `backings` is declared after `vm`.
        free_pages(self.host, self.len);
    }
}
```

In `unmap_overlapping` and `guest_munmap`, replace the Step 1 `free_pages(bk.host, bk.len);` with:

```rust
                drop(bk); // M40: the Backing owns its pages; this releases them, after the stage-2 unmap above
```

(In `guest_munmap` the indentation is one level shallower.)

The two `free_pages(host, rlen)` calls in `place_fixed` case 2 and `map_mmap_region` **stay**.
Those allocations never became a `Backing`.

In the comment block directly above `pub struct Box_`, after the sentence ending
`\`vcpu\` MUST stay declared before \`vm\`.`, add:

```rust
// M40: `backings` MUST stay declared after `vm` for the same reason. Each Backing releases its host
// pages on drop, and host memory must not be released while the VM can still map it, so the order
// on drop is hv_vcpu_destroy -> hv_vm_destroy -> munmap.
```

- [ ] **Step 5: Run the guard and every `retrace-box` test, plus replay-heavy e2e targets**

```bash
cargo test -p retrace-box --test backingfree -- --test-threads=1
cargo test -p retrace-box -- --test-threads=1 2>&1 | grep -a -E '^test result|FAILED|panicked|SIGSEGV|double free'
for t in hello_dyn_e2e checkpoint_seek reverse_debug_e2e watchsweep_e2e cpython_e2e; do
  cargo test -p retrace --test $t -- --test-threads=1 2>&1 | grep -a -E '^test result|FAILED|panicked'; done
```

Expected:
- `backingfree` PASSES.
- Every `retrace-box` binary PASSES, with no crash, double free or segfault. A double release would
  also show as `live_backing_bytes()` wrapping, which the guard catches.
- The listed `retrace` targets PASS. `cpython_e2e` skips loud without Homebrew Python, and that is
  fine.

- [ ] **Step 6: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/backingfree.rs
git commit -m "M40 t4: Backing owns its host pages; a dropped Box_ frees its guest memory after hv_vm_destroy

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: The debugger decodes its trace once

**Files:**
- Modify: `crates/retrace-core/src/lib.rs`: add `DecodedTrace`; the `ReplaySession.events` field
  (~line 1274); `open` (~line 1316); `from_checkpoint` (~line 2835); `CheckpointCache` (~line 2876);
  `checkpointed_seek` (~line 2959)
- Modify: `crates/retrace/src/debug.rs`: `Exec::new`'s symbol read (~line 281)

**Interfaces:**
- Produces:
  - `retrace_core::DecodedTrace` (`Clone`), with `DecodedTrace::load(&Path) -> Result<DecodedTrace, String>` and `DecodedTrace::events(&self) -> &[Event]`
  - `ReplaySession::open_decoded(&DecodedTrace) -> Result<ReplaySession, String>`
  - `ReplaySession::from_checkpoint_decoded(&DecodedTrace, &SessionCheckpoint) -> Result<ReplaySession, String>`
  - `CheckpointCache::decoded(&mut self, &Path) -> Result<DecodedTrace, String>`

  `ReplaySession::open(&Path)`, `ReplaySession::from_checkpoint(&Path, …)` and
  `checkpointed_seek`'s signature are **unchanged**.

- [ ] **Step 1: Confirm the RED test**

Run: `cargo test -p retrace --bins a_debug_session_decodes_its_trace_once -- --test-threads=1`
Expected: FAIL (decodes > 1).

- [ ] **Step 2: Add `DecodedTrace` and the decoded constructors**

In `crates/retrace-core/src/lib.rs`, immediately before `pub struct ReplaySession`:

```rust
/// M40 §3e: a decoded recording — every whole, CRC-valid record, and whether `open_checked` dropped
/// a torn tail. Cheap to clone (the events are shared), so every session a debugger opens uses ONE
/// decode of the file. Before this, each session re-read and re-checked it (t0 M3: seconds of CPU
/// per session on a 97.6 MB trace, 64 % of it CRC).
#[derive(Clone)]
pub struct DecodedTrace { events: Rc<[Event]>, truncated: bool }

impl DecodedTrace {
    pub fn load(trace_path: &Path) -> Result<Self, String> {
        let (events, truncated) = retrace_trace::Reader::open_checked(trace_path)
            .map_err(|e| format!("cannot open trace: {e}"))?;
        Ok(DecodedTrace { events: events.into(), truncated })
    }
    pub fn events(&self) -> &[Event] { &self.events }
}
```

Change the `ReplaySession` field `events: Vec<Event>,` to `events: Rc<[Event]>,`. Every existing
use is `.get(…)` or `.len()`, which work unchanged.

Replace the body of `ReplaySession::open` so it reads:

```rust
    pub fn open(trace_path: &Path) -> Result<Self, String> {
        Self::open_decoded(&DecodedTrace::load(trace_path)?)
    }

    /// M40: `open` over an already-decoded trace (the debugger's hot path). Same contract and the
    /// same error strings; only the file read moved out.
    pub fn open_decoded(trace: &DecodedTrace) -> Result<Self, String> {
        // open_checked kept every whole, CRC-valid record and dropped a torn/corrupt tail; an
        // empty/torn trace or a lost leading Snapshot each become a named error (the caller turns
        // it into a landmark-0 Divergence, exit 3) rather than a panic.
        let events = Rc::clone(&trace.events);
        if events.is_empty() {
            return Err("empty/torn trace: no readable records".into());
        }
        // Rebuild the guest from the snapshot's exact regions (includes stack + trampoline);
        // restore maps only those regions and re-establishes fixed sysregs + captured registers.
        let b = match events.first() {
            Some(Event::Snapshot { regs, mem }) => Box_::restore(mem, regs),
            _ => return Err("trace missing leading Snapshot".into()),
        };
        // events[0] is the initial snapshot; the first landmark to consume is events[1].
        Ok(ReplaySession { b, events, idx: 1, stdout: Vec::new(), guest_task_port: None,
                           truncated: trace.truncated })
    }
```

If `Box_::restore`'s parameters are not `(&[Region], &Regs)`-compatible, pass `&mem[..]`/`regs`
as needed. The old code passed `&mem` (a `&Vec<Region>`) and `&regs`.

Replace `ReplaySession::from_checkpoint` with:

```rust
    /// Re-open a session's trace-level constants (`events`, `truncated`) and restore a `Box_` +
    /// position from a previously captured checkpoint, skipping the landmark-0 replay a cold `open`
    /// would pay. `stdout` starts empty — no checkpoint consumer reads it.
    pub fn from_checkpoint(trace_path: &Path, checkpoint: &SessionCheckpoint) -> Result<Self, String> {
        Self::from_checkpoint_decoded(&DecodedTrace::load(trace_path)?, checkpoint)
    }

    /// M40: `from_checkpoint` over an already-decoded trace.
    pub fn from_checkpoint_decoded(trace: &DecodedTrace, checkpoint: &SessionCheckpoint) -> Result<Self, String> {
        let b = Box_::from_checkpoint(&checkpoint.box_state);
        Ok(ReplaySession { b, events: Rc::clone(&trace.events), idx: checkpoint.idx, stdout: Vec::new(),
                            guest_task_port: checkpoint.guest_task_port, truncated: trace.truncated })
    }
```

- [ ] **Step 3: Give `CheckpointCache` the decoded trace**

Add a field to `CheckpointCache`, after `seeks: u64,`:

```rust
    trace: Option<(std::path::PathBuf, DecodedTrace)>, // M40: decoded once, on first use
```

initialise it in `new` (`trace: None`), and add the method:

```rust
    /// M40: the decoded trace this cache serves, decoding it on first use. The cache is
    /// single-trace by contract (see the struct doc), so a different path is a caller bug and
    /// fails loud.
    pub fn decoded(&mut self, trace_path: &Path) -> Result<DecodedTrace, String> {
        if let Some((p, t)) = &self.trace {
            assert_eq!(p.as_path(), trace_path, "CheckpointCache is single-trace: opened for {} but asked for {}",
                       p.display(), trace_path.display());
            return Ok(t.clone());
        }
        let t = DecodedTrace::load(trace_path)?;
        self.trace = Some((trace_path.to_path_buf(), t.clone()));
        Ok(t)
    }
```

In `checkpointed_seek`, after `cache.seeks += 1;`, add
`let trace = cache.decoded(trace_path)?;`. Then change the three session constructions:
`ReplaySession::from_checkpoint(trace_path, &checkpoint)?` → `ReplaySession::from_checkpoint_decoded(&trace, &checkpoint)?`
(twice), and `ReplaySession::open(trace_path)?` → `ReplaySession::open_decoded(&trace)?`.

- [ ] **Step 4: Build M19's symbol table from the same decode**

In `crates/retrace/src/debug.rs` `Exec::new`, replace the `let syms = retrace_trace::Reader::open(trace).ok()`
expression, keeping its comment, with:

```rust
        let syms = cache.decoded(trace).ok()
            .and_then(|t| t.events().iter().find_map(|e| match e {
                retrace_trace::Event::Snapshot { mem, .. } => Some(Symbols::from_snapshot(mem)),
                _ => None,
            }))
            .unwrap_or_default();
```

and append to that comment: `M40: from the cache's one decode, not a second read of the file.`

- [ ] **Step 5: Run the guard and everything the change touches**

```bash
cargo test -p retrace --bins -- --test-threads=1 2>&1 | grep -a -E '^test result|FAILED|panicked'
for t in checkpoint_seek debug_cli watch_cli reverse_debug_e2e symbols_e2e symbolops_e2e crashy_cli watchsweep_e2e; do
  cargo test -p retrace --test $t -- --test-threads=1 2>&1 | grep -a -E '^test result|FAILED|panicked|error: no test target'; done
cargo test -p retrace-core -- --test-threads=1 2>&1 | grep -a -E '^test result|FAILED|panicked'
```

Expected:
- All `--bins` tests PASS, including `a_debug_session_decodes_its_trace_once` (decodes == 1).
- Every listed target PASSES unchanged. `symbols_e2e` and `symbolops_e2e` guard the symbol table's
  new source.

- [ ] **Step 6: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/retrace-core/src/lib.rs crates/retrace/src/debug.rs
git commit -m "M40 t5: the debugger decodes its trace once (DecodedTrace shared by every session and the symbol table)

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: `watched_of` names the range a wider store covers

**Files:**
- Modify: `crates/retrace/src/debug.rs` (`watched_of` ~line 237; `mod tests`)

- [ ] **Step 1: Write the failing test**

In `mod tests`:

```rust
    #[test] fn watched_of_names_the_range_a_wider_store_covers() {
        let ws = [(0xa01722ac8u64, 8u64)];
        assert_eq!(watched_of(&ws, 0xa01722ac8), 0xa01722ac8, "exact");
        assert_eq!(watched_of(&ws, 0xa01722acc), 0xa01722ac8, "inside the range");
        // t0 M5/M7: rung 8's writers report FAR 0xa01722ac0, the base of a wider store that covers
        // the watched qword. Each case is a FAR that a store REALLY covering the range could report.
        assert_eq!(watched_of(&ws, 0xa01722ac0), 0xa01722ac8, "16-byte stp, 8 below");
        assert_eq!(watched_of(&ws, 0xa01722ab0), 0xa01722ac8, "32-byte stp q, 24 below");
        assert_eq!(watched_of(&ws, 0xa01722ae0), 0xa01722ac8, "DC ZVA, 24 above in the same 64-byte block");
        assert_eq!(watched_of(&ws, 0xa01722b40), 0xa01722b40, "out of reach: the honest fallback, unchanged");
    }
```

Run: `cargo test -p retrace --bins watched_of_names -- --test-threads=1`
Expected: FAIL on the `8 below` case (returns `0xa01722ac0`).

- [ ] **Step 2: Add the third rule**

Replace `watched_of` and its doc with:

```rust
/// The armed watch range containing `far` (exact byte); else the range overlapping `far`'s aligned
/// doubleword (FAR may report the comparator base — spike F4b); else (M40) the first range
/// intersecting `[align_down(far, 64), far + 64)`. That window spans the widest single store
/// (a 64-byte `DC ZVA` block, which may report any FAR inside it) and a 32-byte `stp q` reporting
/// its base below the range, which is what rung 8's writers do (t0 M5: FAR `…ac0` for a watched
/// `…ac8`). Else `far` itself (honest fallback, never a wrong range). Deterministic: `ws` is sorted,
/// first match wins. The third rule runs only where the first two missed, so no output they got
/// right changes.
fn watched_of(ws: &[(u64, u64)], far: u64) -> u64 {
    ws.iter().find(|&&(a, l)| far >= a && far < a + l)
        .or_else(|| ws.iter().find(|&&(a, l)| { let d = far & !7; d < a + l && a < d + 8 }))
        .or_else(|| ws.iter().find(|&&(a, l)| { let lo = far & !63; a < far + 64 && lo < a + l }))
        .map(|&(a, _)| a)
        .unwrap_or(far)
}
```

- [ ] **Step 3: Run it and the transcripts**

```bash
cargo test -p retrace --bins -- --test-threads=1 2>&1 | grep -a -E '^test result|FAILED|panicked'
for t in watch_cli thread_watch_e2e crashy_cli watchsweep_e2e; do
  cargo test -p retrace --test $t -- --test-threads=1 2>&1 | grep -a -E '^test result|FAILED|panicked'; done
```

Expected: all PASS, with transcripts unchanged.

- [ ] **Step 4: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/retrace/src/debug.rs
git commit -m "M40 t6: watched_of names the watched range when a wider store covers it

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Rung-8 acceptance, measured (no code)

**Files:** none committed. Evidence goes to `.superpowers/sdd/2026-09-23-retrace-m40-revcont/t7/`,
and the numbers go in the task report for Task 8.

**Halt rule:** if any threshold below is missed, **stop and report** with the numbers. Do not start
Task 8, and do not tune the thresholds.

- [ ] **Step 1: Build and set up**

```bash
cargo build -p retrace
L=.superpowers/sdd/2026-09-23-retrace-m40-revcont; mkdir -p $L/t7
BIN=target/aarch64-apple-darwin/debug/retrace
codesign -s - -f --entitlements retrace.entitlements $BIN
uptime > $L/t7/load.txt   # the concurrent load, for the record
```

- [ ] **Step 2: Forward `continue` on t0's own recording names the real writer**

```bash
/usr/bin/time -l $BIN debug $L/t0/crash.bin --script "watch 0xa01722ac8 8; continue; x 0xa01722ac8 8; stepi; x 0xa01722ac8 8; where" \
  > $L/t7/fwd.out 2> $L/t7/fwd.err; echo "exit=$?"; cat $L/t7/fwd.out
```

Expected:
- `resolved (1126, 1765682)` (t0 M6's K).
- After `stepi`, `0xa01722ac8: 00 80 23 01 07 00 00 00`, which is `0x701238000` little-endian.
- The hit line names `0xa01722ac8` (Task 6), not `0xa01722ac0`.

- [ ] **Step 3: `reverse-continue`'s own CPU cost, by subtraction**

Use two runs on the same recording. Run A stops before `reverse-continue`; run B includes it. Each
is capped at 900 s. `/usr/bin/time -l` measures the debugger process itself (`perl` `exec`s it):

```bash
/usr/bin/time -l perl -e 'alarm shift; exec @ARGV' 900 $BIN debug $L/t0/crash.bin \
  --script "continue; watch 0xa01722ac8 8" > $L/t7/runA.out 2> $L/t7/runA.err; echo "A exit=$?"
/usr/bin/time -l perl -e 'alarm shift; exec @ARGV' 900 $BIN debug $L/t0/crash.bin \
  --script "continue; watch 0xa01722ac8 8; reverse-continue" > $L/t7/runB.out 2> $L/t7/runB.err; echo "B exit=$?"
grep -a -E ' real | user | sys|maximum resident' $L/t7/runA.err $L/t7/runB.err; cat $L/t7/runB.out
```

CPU is user + sys. An exit of 142 (SIGALRM) means the cap hit. That is a miss: apply the halt rule.

Expected:
- Run B prints `hit watch 0xa01722ac8 (write at 0xa0182245c) at (1143, <K>)`: t0 M5's last writer,
  the store of `0x4000dead0000`.
- **CPU(B) − CPU(A) ≤ 120 s.** The expectation is ≈ 25–35 s.
- **Maximum RSS of run B ≤ 1 GB.**
- Neither run hits the cap.

- [ ] **Step 4: The rung-8 gate test's cost**

```bash
/usr/bin/time -l cargo test -p retrace --test cpython_crash_e2e -- --test-threads=1 > $L/t7/crashe2e.log 2>&1; echo "exit=$?"
grep -a -E 'test result|finished in|real|user|sys|maximum resident' $L/t7/crashe2e.log; uptime >> $L/t7/load.txt
```

Expected: `test result: ok. 1 passed`. Record `finished in` (M39: 12,364 s standalone, 20,740 s
contended) next to the load average.

- [ ] **Step 5: M39's demo, standalone, for the README (carries M39 Task 7c)**

M39's `demo.sh`, re-pointed at this worktree, sampling every 10 s and capped at 1800 s:

```bash
cat > $L/t7/demo.sh <<'EOF'
#!/bin/bash
# M40 Task 7 Step 5: M39's demo transcript (its Task 7c, carried here by M39 R17), standalone.
WT=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m40-revcont
D=$WT/.superpowers/sdd/2026-09-23-retrace-m40-revcont/t7
cd $WT || exit 99
REAL=/opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python
echo "### tree: $(git rev-parse HEAD) $(git status --short | wc -l | tr -d ' ') dirty"
cargo run -q -p retrace -- record-dyn "$REAL" -o $D/m40-demo.bin -- crates/retrace-guest/py/crash.py; echo "record exit=$?"
cargo run -q -p retrace -- replay $D/m40-demo.bin; echo "replay exit=$?"
CELL=$(cargo run -q -p retrace -- replay $D/m40-demo.bin 2>/dev/null | sed -n 's/.*cell=\(0x[0-9a-f]*\).*/\1/p')
echo "### CELL=$CELL"
( while true; do sleep 10; P=$(pgrep -f "retrace debug $D/m40-demo.bin" | head -1)
  echo "$(date +%T) rss_kb+cputime=$(ps -o rss=,cputime= -p "${P:-0}" 2>/dev/null)"; done ) > $D/demo-mem.log 2>&1 &
SAMPLER=$!
perl -e 'alarm shift; exec @ARGV' 1800 cargo run -q -p retrace -- debug $D/m40-demo.bin \
  --script "continue; watch $CELL 8; reverse-continue; where; x $CELL 8; stepi; x $CELL 8"; echo "debug exit=$?"
kill $SAMPLER 2>/dev/null
echo "DEMO DONE"
EOF
bash $L/t7/demo.sh > $L/t7/demo.log 2>&1; tail -30 $L/t7/demo.log
```

Expected: `record exit=139`, `replay exit=139`, a `CELL=` line, and `debug exit=0`, then
`DEMO DONE`. The transcript runs `continue` → `guest crashed`, then `watch`, then `reverse-continue`
→ `hit watch <CELL> (write at …) at (…)`, then `where`, `x` (still the old value: the store is
pre-retire), `stepi`, and `x` showing the bad pointer `00 00 ad de 00 40 00 00`. `demo-mem.log`
shows RSS staying flat. Keep the transcript verbatim for Task 8. The recording is fresh, so its
coordinates differ from t0's.

- [ ] **Step 6: Write the task report**

In `$L/task-7-report.md`, record every number above with its file, plus the load average at each
measurement. No commit.

---

### Task 8: The gate and the close

**Files:**
- Modify: `README.md`, `CLAUDE.md`, `docs/status-log.md`, and the spec's §10
  (`docs/superpowers/specs/2026-09-23-retrace-m40-revcont-design.md`)

- [ ] **Step 1: Run the gate, chunked, capturing every exit code before any pipe**

```bash
L=.superpowers/sdd/2026-09-23-retrace-m40-revcont; mkdir -p $L/gate
cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1 > $L/gate/ws.log 2>&1; echo $? > $L/gate/ws.exit
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 > $L/gate/box.log 2>&1; echo $? > $L/gate/box.exit
cargo test -p retrace --bins --no-fail-fast -- --test-threads=1 > $L/gate/bins.log 2>&1; echo $? > $L/gate/bins.exit
for t in $(ls crates/retrace/tests/*.rs | xargs -n1 basename | sed 's/\.rs$//'); do
  cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/gate/e2e-$t.log 2>&1; echo $? > $L/gate/e2e-$t.exit; done
cargo clippy --workspace --all-targets -- -D warnings > $L/gate/clippy.log 2>&1; echo $? > $L/gate/clippy.exit
cat $L/gate/*.exit | sort | uniq -c    # expect only 0s
```

A single tool call is capped at 10 minutes, so run the per-target loop in batches or in the
background, as M39 did. **Do not read a kill as a red.** Sum the `test result:` lines with
`grep -a`.

- [ ] **Step 2: Reconcile against M39 file by file**

M39 closed at **629 / 0 / 9 over 138**. The expected delta is **+8 tests over +2 binaries → 637 / 0 / 9
over 140**. That refines spec §9's rough ≈ 636 / 139:

| file | + tests |
|---|---|
| `retrace-guest/src/lib.rs` (`watchsweep_guest_parses`) | +1 |
| `retrace/tests/watchsweep_e2e.rs` (new binary) | +3 |
| `retrace/src/debug.rs` (two cost tests, `watched_of`) | +3 |
| `retrace-box/tests/backingfree.rs` (new binary) | +1 |

Diff `#[test]` counts per file against `786bf2b`
(`git diff --stat 786bf2b -- crates` plus `grep -c '#\[test\]'` on each changed file). Any
difference from the table must be explained by name. Confirm the invariants:
`git diff 786bf2b -- crates/retrace-trace/src/lib.rs | grep -c TRACE_MAGIC` = 0, and
`grep -c 'verify_thread(' crates/retrace-core/src/lib.rs` is unchanged from `786bf2b`.

- [ ] **Step 3: Edit the README, in place (it describes the present)**

- In "What works today", in the rung-8 entry (~line 424), add a **"Reverse-debugging CPython"**
  subsection. It holds Task 7 Step 5's transcript verbatim, and one sentence each on its measured
  CPU cost and its memory from Task 7.
- In "Known limits", **replace** the bullet beginning
  `**\`reverse-continue\` on a long recording is slow enough to plan around` (~line 1106) with the
  new reality. `reverse-continue` is one forward pass plus one resolution. It costs about one
  replay plus a window of single-steps; state Task 7 Step 3's measured CPU and RSS with the load
  noted. Name what remains: the pass is still a full forward replay from landmark 1, so the cost
  scales with the recording's length, not with the number of hits. Also state that a faster
  `crc32` is owed.
- Update the gate line to Step 1's measured totals, and add the testing-table rows for the two new
  binaries in the format of M39's rows (~line 495).

- [ ] **Step 4: `CLAUDE.md`**

- In the e2e list, after the `vmremap_e2e` entry, add: `\`watchsweep_e2e\` (M40: a guest whose one
  store instruction sweeps a buffer before reaching the watched element, so \`continue\` and
  \`reverse-continue\` must resolve a watch hit by address, not by pc — the class that put rung 8's
  \`continue\` 1.7 M instructions early)`.
- Change "the **11 unit tests** inside the \`retrace\` binary itself" to **14**, and "silently costs
  11 tests" to **14**. That is the figure the `--bins` trap warning depends on.

- [ ] **Step 5: Append the M40 section to `docs/status-log.md`**

Append; never rewrite earlier sections. Title it
`## Status: M40-revcont — reverse-continue in one pass, watch hits resolved by address`. It covers:
- what t0 measured (a pointer to the companion);
- the four fixes, each with its guard and its RED → green evidence from Tasks 1–6;
- Task 7's rung-8 numbers, with the load;
- the gate, with the reconciliation table;
- every ruling the execution made, numbered after the spec's R1–R6;
- **"What stays owed"**: the faster `crc32`, the forward-replay floor, M39's carried items, and any
  review minors.

The M39 section's R17 promised the transcript here: say that it landed.

- [ ] **Step 6: Fill the spec's §10 Outcome**

Record the measured figures against every §6 acceptance item, each prediction confirmed or
corrected, and anything found that was not sought.

- [ ] **Step 7: Commit**

```bash
git add README.md CLAUDE.md docs/status-log.md docs/superpowers/specs/2026-09-23-retrace-m40-revcont-design.md
git commit -m "M40 close: the gate (<measured totals>), rung-8 reverse-continue measured, the demo transcript

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```
