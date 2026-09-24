# M41-hitorder Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `continue` and `reverse-continue` report every hit exactly once, in one defined
order, on the thread that executes it, and prove it against a brute-force hit oracle.

**Architecture:**
- **The switch.** `ReplaySession::finish_event` settles a pending thread switch, so a boundary
  `(n, 0)` shows the thread that runs next (§3a).
- **The cursor.** The debugger keeps a cursor `(n, k, phase)`, with phases ordered
  Sys < Bp < Watch. `continue` answers the first hit after it, `reverse-continue` the last hit
  before it, and forward resolution starts at `kctx` (§3b–§3d).
- **The oracle.** In test code, `enumerate_hits` single-steps a recording with everything armed
  and lists every hardware stop. Three chains of debugger commands are checked against that list
  (§3e).

**Tech Stack:** Rust 1.95.0 (pinned), Hypervisor.framework via `hv-sys`, arm64 asm and
Rust/C guest fixtures built by `retrace-guest/build.rs`.

**Spec:** `docs/superpowers/specs/2026-09-24-retrace-m41-hitorder-design.md`. Its measurements are
in `…-measurements.md` beside it, cited below as "t0 M1–M8". Read both before starting.

## Global Constraints

- **Toolchain:** `1.95.0`, target `aarch64-apple-darwin`.
- **`-- --test-threads=1` on every test command.** HVF allows one VM per process.
- **Clippy stays clean:** `cargo clippy --workspace --all-targets -- -D warnings`. `clippy.toml`
  bans `Instant::now`, `SystemTime::now` and `std::thread::Thread`, so nothing added here times
  anything.
- **No trace-format change.** `TRACE_MAGIC` stays `RT\x00\x0a`, and `Event` does not change.
- **No dispatch-arm change** in `record_box` or in the arms of `ReplaySession::advance`.
  `verify_thread` stays at **seven** call sites. Record is untouched.
- **`Box_` field order is load-bearing** (`vcpu`, then `vm`, then `backings`). Never reorder it.
- **No new `#[ignore]`.**
- **Existing debugger transcripts stay byte-identical:**
  - `debug_cli`, `watch_cli`, `watch`, `watch_dyn`
  - `watchsweep_e2e`, `thread_watch_e2e`
  - `crashy_cli`, `crashy_e2e`, `reverse_debug_e2e`
  - `checkpoint_seek`, `cpython_crash_e2e`, `sigcatch_dyn_e2e`
  - `debug.rs`'s existing unit tests

  The spec predicts no moved assertion (§4's audit: R5's case is pinned by no existing test). If one
  moves, **stop and report it. Do not edit the expectation.**
- **Halt rules (spec §7).** If §3a breaks an existing suite other than by the predicted
  boundary-thread change, stop and report (Task 2 Step 4). If an oracle check fails for a reason
  outside hit accounting (a resolver/hardware disagreement, a replay divergence, a fault), don't
  widen the milestone: record it for the README's Known limits and the owed list, and report it.
- **Discover every address and landmark; never hardcode one.** `threadrust`'s are shared-cache
  addresses.
- **Spawn the CLI through `util::bin()`**, which codesigns a copy.
- **Commit messages end with**
  `Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>`.
- **Grep logs with `grep -a`** (they carry ANSI and UTF-8).
- **Measure CPU seconds and counts, never wall-clock.** The operator runs concurrent sessions.
- **The ledger** is `.superpowers/sdd/2026-09-24-retrace-m41-hitorder/`. It is excluded through
  `.git/info/exclude` and never committed.
- **The session runs in the worktree `.claude/worktrees/m41-hitorder`**, where the harness refuses
  any command prefixed with `VAR=value`. Write `export VAR=value` on its own line instead, and keep
  shell commands simple.
- **Execution rulings are numbered from R12.** R9 and R10 below are this plan's own. R11 is left
  unused because this plan and M40 already use "R11" as the bug's name.

## Plan-time rulings

- **R9 — spec §5's Task 0 is folded into the steps that need each premise.**
  - `step_armed`'s premise is measured by its own test in Task 1 Step 2. If it fails, stop.
  - `threadrust`'s oracle cost is measured in Task 1 Step 7 and decides the oracle's starting
    landmark.
  - `blockedctx` is run in Task 2 Step 4, right after approach A lands, under the halt rule.

  A separate prototype task would have written each of these twice.
- **R10 — `continue` from a breakpoint on a faulting instruction crosses the fault the way it
  crosses a trap.** This replaces the spec's §3c final paragraph ("propagates as the same error it
  does today").
  - **Measured at plan time on `c68ba6d`, with `crashy`:** `break <crash pc>; continue; continue`
    exits 5 with `DEBUG ERROR: guest crashed at step 0/1`. A plain `continue; continue` reports
    `guest crashed: …` twice and exits 0.
  - **Why:** the new finish step already has to classify the stop, so `Armed::Fault` can take the
    `AtTrap` path at no cost, and both routes then give the same report. Review Focus 1 pins it.
  - **Not verified:** a *handled* fault, the M40 T3 minor. That stays owed, and nothing here claims
    to close it.

## Review Focus

Five inputs the spec implies but no spec'd test covers, most likely to bite first:

1. **A breakpoint on the instruction that crashes.** `continue` from it should report the crash
   (exit 0), just as `continue` does without the breakpoint, and repeat the report on a further
   `continue`. Today it exits 5. Task 3 owns it (R10).
2. **A watch scoped to a thread that never writes it.** Every hit is scoped out, so `continue`
   should run to `exited` with no hit reported. This now takes the loop path that replaced M15
   Task 8's recursion (R8). Task 3.
3. **`unwatch` while parked on a hit whose coordinate also holds a watched store.** The next
   `continue` should *not* report that store: what's still armed decides. Task 3.
4. **`continue` again after the guest has exited.** It should print `exited (code 0)` again and exit
   0, never an error. Task 3.
5. **A `reverse-continue` that finds nothing leaves the cursor where it was.** From a watch-phase
   park, "no earlier hit" followed by `continue` must step over the store, not report it a second
   time. This is where resetting the phase by accident would show. Task 3, where `reseek` starts
   resetting the phase.

## File structure

| File | Change | Responsibility |
|---|---|---|
| `crates/retrace-core/src/lib.rs` | modify | `Armed` + `ReplaySession::step_armed` (T1); `finish_event` settles, `current_thread`/`pc`/`position` docs (T2) |
| `crates/retrace-core/tests/replay.rs` | modify | `step_armed`'s premise test (T1) |
| `crates/retrace-box/src/lib.rs` | modify | `Box_::settle_schedule`, called by `run()`/`step()` entry (T2) |
| `crates/retrace/tests/util/mod.rs` | modify | `pub mod hits;` (T1) |
| `crates/retrace/tests/util/hits.rs` | create | the oracle, the transcript parser, the three chain checks (T1) |
| `crates/retrace/tests/hitorder_e2e.rs` | create | named regressions M1–M8, the §3a invariant, five oracle armings (T1); Review Focus tests (T3) |
| `crates/retrace/tests/blockedctx.rs` | modify | header comment (T2) |
| `crates/retrace/src/debug.rs` | modify | comments (T2); `Phase`, the cursor, `continue`, R7 message, 2 unit tests (T3); `reverse-continue` order (T4) |
| `README.md`, `CLAUDE.md`, `docs/status-log.md`, spec §10 | modify (T5) | the close |

---

### Task 1: `step_armed`, the hit oracle, and the REDs

**Files:**
- Modify: `crates/retrace-core/src/lib.rs`: `Armed` beside `pub enum Stepped` (~line 1269), and
  `step_armed` after `step_watched` (~line 2884).
- Modify: `crates/retrace-core/tests/replay.rs` (append).
- Modify: `crates/retrace/tests/util/mod.rs` (after the `use` lines).
- Create: `crates/retrace/tests/util/hits.rs`.
- Create: `crates/retrace/tests/hitorder_e2e.rs`.

**Interfaces:**
- Produces:
  - `retrace_core::Armed { Retired, Break, Watch, AtTrap, Fault }` (Debug, Clone, Copy, PartialEq,
    Eq).
  - `ReplaySession::step_armed(&mut self) -> Result<Armed, String>`.
  - `util::hits::{Phase, Hit, Key, enumerate_hits, answers, check_chains, debug}`.

  Task 3 uses `step_armed` and `Armed`. Tasks 3 and 4 add tests to `hitorder_e2e.rs` using its
  `ws_trace()`, `fio_trace()`, `discover_ws`, `discover_fio` and `hits::debug`.

- [ ] **Step 1: Add `Armed` and `step_armed`**

In `crates/retrace-core/src/lib.rs`, directly after the `Stepped` enum:

```rust
/// M41: what one fully armed single step did (`ReplaySession::step_armed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Armed {
    /// One instruction retired.
    Retired,
    /// The next instruction's address is an armed breakpoint. Nothing retired, and while it stays
    /// armed every further step stops here again.
    Break,
    /// The next instruction writes an armed watch range. Nothing retired (pre-retire, spike F4c).
    Watch,
    /// The next instruction is the window-ending trap. Nothing retired, and the trap is not
    /// consumed: `advance()` consumes it.
    AtTrap,
    /// The next instruction takes a real guest fault (a demand-paging one is handled and
    /// re-stepped instead). Nothing retired.
    Fault,
}
```

In `impl ReplaySession`, directly after `step_watched`:

```rust
    /// M41: single-step one instruction with whatever breakpoints AND watchpoints are armed, and
    /// report which stop, if any, it made. `step_watched`'s contract makes a breakpoint stop a
    /// caller bug; this is the step for callers that arm both — the M41 hit oracle (every hit is a
    /// hardware stop taken while single-stepping) and `continue`'s finish of the current
    /// coordinate. The exception class is checked FIRST, before any demand-paging fallback, because
    /// a breakpoint's or watchpoint's FAR is not an IPA (M40 §3a). Deterministic replay faults are
    /// handled and re-stepped exactly as in `step_insns`.
    pub fn step_armed(&mut self) -> Result<Armed, String> {
        loop {
            match self.b.step() {
                Stop::Step => return Ok(Armed::Retired),
                Stop::Other { esr } => {
                    match retrace_arch::ec_of(esr) {
                        retrace_arch::Ec::Breakpoint => return Ok(Armed::Break),
                        retrace_arch::Ec::Watchpoint => return Ok(Armed::Watch),
                        _ => {}
                    }
                    if self.b.page_in_cache(self.b.fault_ipa()) { continue; }
                    if self.b.commit_reserved_page(self.b.fault_ipa()) { continue; }
                    return Err(format!("fault during an armed step: {}", self.b.describe_stop(esr)));
                }
                Stop::Syscall { .. } => return Ok(Armed::AtTrap),
                Stop::Fault { .. } => return Ok(Armed::Fault),
            }
        }
    }
```

- [ ] **Step 2: The premise test (the spec's owed Task 0 measurement, R9)**

Append to `crates/retrace-core/tests/replay.rs`:

```rust
// M41: `step_armed`, the hit oracle's primitive. Spec §3e owes this measurement: a breakpoint armed
// at the CURRENT pc stops before anything retires (and stops again while armed); a watched store
// stops pre-retire, its memory unwritten; one disarmed step then retires it; the window-ending trap
// is reported and left for `advance()` to consume.
#[test]
fn step_armed_reports_each_stop_class_without_retiring() {
    let trace = record_guest(retrace_guest::WATCHSWEEP, "m41-steparmed");
    // &buf[40] from the recorded write(1, &buf[40], 8); buf[0] is 320 bytes below it.
    let buf0 = {
        let mut s = retrace_core::ReplaySession::open(&trace).unwrap();
        loop {
            if let Some((4, a)) = s.peek_syscall() { if a[0] == 1 { break a[1] - 320; } }
            s.advance().unwrap();
        }
    };
    // K = 8 is watchsweep's sweeping `str`, first pass: it writes buf[0].
    let mut s = retrace_core::seek(&trace, 1, 8).unwrap();
    let store = s.pc();
    s.arm_breakpoints(&[store]);
    assert_eq!(s.step_armed().unwrap(), retrace_core::Armed::Break);
    assert_eq!(s.pc(), store, "a breakpoint stop retires nothing");
    assert_eq!(s.step_armed().unwrap(), retrace_core::Armed::Break, "and stops again while armed");
    s.clear_breakpoints();
    s.arm_watchpoints(&[(buf0, 8)]);
    assert_eq!(s.step_armed().unwrap(), retrace_core::Armed::Watch);
    assert_eq!(s.pc(), store, "a watch stop is pre-retire");
    assert_eq!(s.read_mem(buf0, 8).unwrap(), vec![0u8; 8], "the store has not written");
    s.clear_watchpoints();
    assert_eq!(s.step_armed().unwrap(), retrace_core::Armed::Retired);
    assert_eq!(s.pc(), store + 4);
    assert_ne!(s.read_mem(buf0, 8).unwrap(), vec![0u8; 8], "the disarmed step retired the store");
    // Run on to the window-ending trap (the write's svc): reported, not consumed.
    let mut steps = 0;
    while s.step_armed().unwrap() == retrace_core::Armed::Retired { steps += 1; }
    assert!(steps > 0 && s.landmark() == 1, "the trap is window 1's end, unconsumed");
    assert!(!matches!(s.advance().unwrap(), retrace_core::Advance::Exited(_)));
    assert_eq!(s.landmark(), 2, "advance() consumed it");
}
```

Run:

```bash
mkdir -p .superpowers/sdd/2026-09-24-retrace-m41-hitorder
cargo test -p retrace-core --test replay step_armed -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m41-hitorder/t1-steparmed.log 2>&1; echo "exit=$?"
```

Expected: PASS. **If it fails, stop and report.** The oracle's premise is false, and the spec
(§3e) has to be revisited before anything is built on it.

- [ ] **Step 3: Wire the `hits` module**

In `crates/retrace/tests/util/mod.rs`, after the `use std::sync::OnceLock;` line, add:

```rust
/// M41: the debugger's hit oracle and the checks built on it (see `hits.rs`).
pub mod hits;
```

- [ ] **Step 4: Write the oracle**

Create `crates/retrace/tests/util/hits.rs`:

```rust
//! M41: the debugger's ground truth — every hit a recording holds for one arming, found by brute
//! force — and the three checks that compare the debugger's own answers against it (spec §3e).
//!
//! Deliberately shares NONE of the debugger's machinery: no `resolve_nth`, no pre-step, no
//! scan/resolve split, no pc-based counting. Every hit is a HARDWARE stop taken while
//! single-stepping with everything armed (`ReplaySession::step_armed`), read AFTER the stop.
//! `Box_::step()` switches threads on entry, so the oracle sees the running thread whether or not
//! M41 §3a's settle is in place: it is independent of the fix it checks.
use retrace_core::{Advance, Armed, ReplaySession};
use std::path::Path;

/// Where a hit sits within one coordinate (n, k), in the hardware's order (spec §3b): a syscall's
/// write (k = 0 only), then a breakpoint, then a watched store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase { Sys, Bp, Watch }

/// One hit. For `Sys`, `pc` and `thread` are whatever the session shows at (n, 0) and are never
/// compared: a syscall's write has no instruction (see `Key`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit { pub n: usize, pub k: u64, pub phase: Phase, pub pc: u64, pub thread: u32 }

/// What the checks compare: the coordinate and phase always; pc and thread for an instruction hit.
pub type Key = (usize, u64, Phase, Option<(u64, u32)>);

impl Hit {
    pub fn key(&self) -> Key {
        (self.n, self.k, self.phase, (self.phase != Phase::Sys).then_some((self.pc, self.thread)))
    }
}

/// Every hit of `bps` and `ws` from `(from_n, 0)` to the end of the recording, in order. Panics on
/// a guest fault: no oracle fixture faults, and a fault's crossing is not part of this proof.
pub fn enumerate_hits(trace: &Path, bps: &[u64], ws: &[(u64, u64)], from_n: usize) -> Vec<Hit> {
    let mut s = retrace_core::seek(trace, from_n, 0).expect("oracle: seek");
    let mut hits = Vec::new();
    let (mut n, mut k) = (from_n, 0u64);
    s.arm_breakpoints(bps);
    s.arm_watchpoints(ws);
    loop {
        match s.step_armed().expect("oracle: armed step") {
            Armed::Retired => k += 1,
            Armed::Break => {
                hits.push(Hit { n, k, phase: Phase::Bp, pc: s.pc(), thread: s.current_thread() });
                // Step the breakpointed instruction with the watches still armed: it may be a
                // watched store too, and then it is a second hit at the same coordinate.
                s.clear_breakpoints();
                match s.step_armed().expect("oracle: step off a breakpoint") {
                    Armed::Retired => k += 1,
                    Armed::Watch => {
                        hits.push(Hit { n, k, phase: Phase::Watch, pc: s.pc(), thread: s.current_thread() });
                        step_over(&mut s, ws);
                        k += 1;
                    }
                    Armed::AtTrap => {
                        if cross(&mut s, &mut hits) { return hits; }
                        (n, k) = (s.landmark(), 0);
                    }
                    Armed::Fault => panic!("oracle: a guest fault at ({n}, {k})"),
                    Armed::Break => unreachable!("breakpoints are disarmed"),
                }
                s.arm_breakpoints(bps);
            }
            Armed::Watch => {
                hits.push(Hit { n, k, phase: Phase::Watch, pc: s.pc(), thread: s.current_thread() });
                s.clear_breakpoints();
                step_over(&mut s, ws);
                s.arm_breakpoints(bps);
                k += 1;
            }
            Armed::AtTrap => {
                s.clear_breakpoints();
                if cross(&mut s, &mut hits) { return hits; }
                (n, k) = (s.landmark(), 0);
                s.arm_breakpoints(bps);
            }
            Armed::Fault => panic!("oracle: a guest fault at ({n}, {k})"),
        }
    }
}

/// Retire a watched store with nothing watched, then re-arm. A second arm without a clear would
/// duplicate the session's syscall-watch ranges.
fn step_over(s: &mut ReplaySession, ws: &[(u64, u64)]) {
    s.clear_watchpoints();
    s.step_insns(1).expect("oracle: step over a watched store");
    s.arm_watchpoints(ws);
}

/// Consume the event the guest is parked at, watches armed and breakpoints off. True at the end.
fn cross(s: &mut ReplaySession, hits: &mut Vec<Hit>) -> bool {
    match s.advance().expect("oracle: advance") {
        Advance::Exited(_) => true,
        Advance::WatchSyscall { .. } => {
            hits.push(Hit { n: s.landmark(), k: 0, phase: Phase::Sys, pc: s.pc(), thread: s.current_thread() });
            false
        }
        Advance::Event => false,
        Advance::Break | Advance::Watch { .. } => panic!("oracle: an instruction retired during a crossing"),
    }
}

/// `retrace debug <trace> --script <script>` on the codesigned copy: (exit code, stdout, stderr).
pub fn debug(trace: &str, script: &str) -> (i32, String, String) {
    let out = std::process::Command::new(super::bin())
        .args(["debug", trace, "--script", script])
        .output().expect("spawn debug");
    (out.status.code().unwrap_or(-1),
     String::from_utf8(out.stdout).unwrap(),
     String::from_utf8(out.stderr).unwrap())
}

/// Parse a transcript in which every `continue` / `reverse-continue` is followed by `where` into
/// one answer per such command: `Some(key)` for a hit, `None` for an end ("exited", "guest
/// crashed", "guest terminated", "no earlier hit"). Stops early if the debugger died mid-script.
pub fn answers(transcript: &str) -> Vec<Option<Key>> {
    let lines: Vec<&str> = transcript.lines().map(super::strip_annot).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i] != "> continue" && lines[i] != "> reverse-continue" { i += 1; continue; }
        let mut j = i + 1;
        while j < lines.len() && !lines[j].starts_with("> ") { j += 1; }
        if lines.get(j).copied() != Some("> where") || j + 1 >= lines.len() { break; }
        out.push(parse_answer(&lines[i + 1..j], lines[j + 1], transcript));
        i = j + 2;
    }
    out
}

fn parse_answer(body: &[&str], wh: &str, all: &str) -> Option<Key> {
    let first = body.first().copied().unwrap_or("");
    if ["exited", "guest crashed", "guest terminated", "no earlier hit"].iter().any(|p| first.starts_with(p)) {
        return None;
    }
    let phase = if first.contains("(syscall write)") { Phase::Sys }
        else if first.starts_with("hit watch ") { Phase::Watch }
        else if first.starts_with("hit 0x") { Phase::Bp }
        else { panic!("unrecognised answer {first:?} in:\n{all}") };
    let (n, k) = coord_of(body).unwrap_or_else(|| panic!("no coordinate in {body:?}:\n{all}"));
    let (wn, wk, pc, thread) = parse_where(wh).unwrap_or_else(|| panic!("bad `where` {wh:?}:\n{all}"));
    assert_eq!((wn, wk), (n, k), "`where` disagrees with the reported coordinate:\n{all}");
    Some((n, k, phase, (phase != Phase::Sys).then_some((pc, thread))))
}

/// `resolved (N, K)` if present, else the `at (N, K)` that ends the hit line.
fn coord_of(body: &[&str]) -> Option<(usize, u64)> {
    if let Some(r) = body.iter().find_map(|l| l.strip_prefix("resolved ")) { return pair(r); }
    let first = body.first()?;
    pair(&first[first.rfind(" at ")? + 4..])
}

/// `(N, K)` → (N, K).
fn pair(s: &str) -> Option<(usize, u64)> {
    let (a, b) = s.trim().strip_prefix('(')?.strip_suffix(')')?.split_once(", ")?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// `at (N, K) pc=0x… thread=T` → (N, K, pc, T).
fn parse_where(l: &str) -> Option<(usize, u64, u64, u32)> {
    let rest = l.strip_prefix("at ")?;
    let close = rest.find(')')?;
    let (n, k) = pair(&rest[..=close])?;
    let (mut pc, mut thread) = (None, None);
    for tok in rest[close + 1..].split_whitespace() {
        if let Some(h) = tok.strip_prefix("pc=0x") { pc = u64::from_str_radix(h, 16).ok(); }
        if let Some(t) = tok.strip_prefix("thread=") { thread = t.parse().ok(); }
    }
    Some((n, k, pc?, thread?))
}

/// Spec §3e's three checks of one arming (a `;`-separated `break`/`watch` script) against `hits`,
/// the oracle's list for that arming.
pub fn check_chains(trace: &str, arming: &str, hits: &[Hit]) {
    let keys: Vec<Key> = hits.iter().map(Hit::key).collect();
    let len = keys.len();
    assert!(len >= 2, "an arming needs two hits for the zig-zag to have neighbours: {hits:?}");

    // 1. Forward: `continue` from the opening position visits every hit in order, then ends.
    let mut want: Vec<Option<Key>> = keys.iter().copied().map(Some).collect();
    want.push(None);
    chain(trace, "forward chain", &format!("{arming}; {}", "continue; where; ".repeat(len + 1)), &want);

    // 2. Backward: from the terminal, `reverse-continue` visits them in reverse, then ends. The
    //    leading `continue` runs to the terminal with nothing armed yet; its answer is the end.
    let mut want = vec![None];
    want.extend(keys.iter().rev().copied().map(Some));
    want.push(None);
    chain(trace, "backward chain",
          &format!("continue; where; {arming}; {}", "reverse-continue; where; ".repeat(len + 1)), &want);

    // 3. Zig-zag: a hit reached by `continue` gives its predecessor on `reverse-continue`, and one
    //    reached by `reverse-continue` gives its successor on `continue`.
    let mut script = format!("{arming}; continue; where; ");
    let mut want = vec![Some(keys[0])];
    for pair in keys.windows(2) {
        script += "continue; where; reverse-continue; where; continue; where; ";
        want.extend([Some(pair[1]), Some(pair[0]), Some(pair[1])]);
    }
    chain(trace, "zig-zag", &script, &want);
}

fn chain(trace: &str, what: &str, script: &str, want: &[Option<Key>]) {
    let (code, out, err) = debug(trace, script);
    let got = answers(&out);
    if let Some(i) = (0..want.len().max(got.len())).find(|&i| want.get(i) != got.get(i)) {
        panic!("{what}: answer #{i} is {:?}; the oracle says {:?}\n\
                ({} answers wanted, {} given; debugger exit {code}; stderr: {err})\n\
                every answer given: {got:?}",
               got.get(i), want.get(i), want.len(), got.len());
    }
    assert_eq!(code, 0, "{what}: the debugger exited {code}; stderr: {err}");
}
```

- [ ] **Step 5: Write the e2e file: discovery, named regressions, invariant, oracle armings**

Create `crates/retrace/tests/hitorder_e2e.rs`:

```rust
// M41: every hit counted once, in one order, on the thread that runs it (spec
// docs/superpowers/specs/2026-09-24-retrace-m41-hitorder-design.md). Three kinds of test:
//
// - a NAMED REGRESSION for each t0 measurement (M1–M8 in the companion), pinned to exact
//   coordinates;
// - spec §3a's INVARIANT: at a blocking boundary the position shows the thread that runs next;
// - the HIT ORACLE's three checks (util::hits::check_chains) on five armings.
//
// Every address and landmark is DISCOVERED. threadrust's are shared-cache addresses, valid only for
// this host's cache. Each fixture is recorded once per test process and shared: a debug session
// only reads its trace.
mod util;
use retrace_core::{Advance, ReplaySession};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use util::hits::{self, Phase};

fn recorded(cell: &'static OnceLock<PathBuf>, guest: &str, dynamic: bool) -> &'static Path {
    cell.get_or_init(|| {
        let (rec, trace) = if dynamic { util::record_dynamic(guest) } else { util::record(guest) };
        assert_eq!(rec.code, 0, "record {guest} failed: {}", rec.stderr);
        trace
    })
}
fn ws_trace() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    recorded(&C, retrace_guest::WATCHSWEEP, false)
}
fn fio_trace() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    recorded(&C, retrace_guest::FILEIO, false)
}
fn tr_trace() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    recorded(&C, retrace_guest::THREADRUST, true)
}
fn ts(p: &Path) -> &str { p.to_str().unwrap() }
fn last_line(out: &str) -> &str { util::strip_annot(out.trim_end().lines().last().unwrap_or("")) }

/// The K in window `n` at which `pc` first executes.
fn k_of_pc(tp: &Path, n: usize, pc: u64) -> u64 {
    let mut s = retrace_core::seek(tp, n, 0).unwrap();
    let mut k = 0;
    while s.pc() != pc {
        s.step_insns(1).unwrap_or_else(|e| panic!("pc {pc:#x} is not in window {n}: {e}"));
        k += 1;
    }
    k
}

/// watchsweep, discovered. `t` = &buf[40]; `store` = the sweeping `str`, first run at K =
/// `first_run`, writing buf[40] at K = `k_sweep`; `second` = the second writer, at K = `k_second`.
struct Ws { t: u64, store: u64, first_run: u64, k_sweep: u64, second: u64, k_second: u64 }

fn discover_ws(tp: &Path) -> Ws {
    let t = {
        let mut s = ReplaySession::open(tp).unwrap();
        loop {
            if let Some((4, a)) = s.peek_syscall() { if a[0] == 1 { break a[1]; } }
            s.advance().unwrap();
        }
    };
    // Ground truth by memory diff, independent of every watch mechanism.
    let mut s = retrace_core::seek(tp, 1, 0).unwrap();
    let (mut prev, mut k, mut ks, mut pcs) = (s.read_mem(t, 8).unwrap(), 0u64, vec![], vec![]);
    loop {
        let pc = s.pc();
        if s.step_insns(1).is_err() { break; }
        let cur = s.read_mem(t, 8).unwrap();
        if cur != prev { ks.push(k); pcs.push(pc); prev = cur; }
        k += 1;
    }
    assert_eq!(ks.len(), 2, "watchsweep writes buf[40] exactly twice: {ks:?}");
    Ws { t, store: pcs[0], first_run: k_of_pc(tp, 1, pcs[0]), k_sweep: ks[0], second: pcs[1], k_second: ks[1] }
}

/// fileio, discovered. `bpc` = the first instruction after `read`'s `svc` = (after_read, 0);
/// `read_svc` = that `svc`, at (read_win, k_read); `buf` = what `read` writes.
struct Fio { after_read: usize, read_win: usize, buf: u64, bpc: u64, read_svc: u64, k_read: u64 }

fn discover_fio(tp: &Path) -> Fio {
    let mut s = ReplaySession::open(tp).unwrap();
    let (after_read, buf, bpc) = loop {
        if let Some((3, a)) = s.peek_syscall() {
            s.advance().unwrap();
            break (s.landmark(), a[1], s.pc());
        }
        s.advance().unwrap();
    };
    let read_svc = bpc - 4; // arm64: a syscall returns to svc + 4
    Fio { after_read, read_win: after_read - 1, buf, bpc, read_svc, k_read: k_of_pc(tp, after_read - 1, read_svc) }
}

/// threadrust, discovered. `n_create` = after `bsdthread_create` (360); `n_block` = after main's
/// blocking `__ulock_wait` (515); `n_exit` = after the child's `bsdthread_terminate` (361).
/// `resume` = main's resume pc, `child` = the child's first pc — read from the thread table, never
/// from `position()`: after M41 §3a the boundary holds the INCOMING thread, so ELR is not main's.
struct Tr { n_create: usize, n_block: usize, n_exit: usize, resume: u64, child: u64 }

fn pc_of(s: &ReplaySession, tid: usize) -> u64 {
    let dump = s.dbg_regs_of(tid).expect("the thread exists");
    dump.split_whitespace().find_map(|t| t.strip_prefix("pc=0x"))
        .and_then(|h| u64::from_str_radix(h, 16).ok())
        .unwrap_or_else(|| panic!("no pc in {dump}"))
}

fn discover_tr(tp: &Path) -> Tr {
    let mut s = ReplaySession::open(tp).unwrap();
    let (mut n_create, mut n_block, mut n_exit, mut resume, mut child) = (0, 0, 0, 0, 0);
    loop {
        let num = s.peek_syscall().map(|(num, _)| num);
        if let Advance::Exited(_) = s.advance().unwrap() { break; }
        match num {
            Some(360) if n_create == 0 => n_create = s.landmark(),
            Some(515) if n_block == 0 => {
                n_block = s.landmark();
                resume = pc_of(&s, 0);
                child = pc_of(&s, 1);
            }
            Some(361) if n_exit == 0 => n_exit = s.landmark(),
            _ => {}
        }
    }
    assert!(0 < n_create && n_create < n_block && n_block < n_exit,
        "threadrust: create {n_create}, block {n_block}, exit {n_exit}");
    Tr { n_create, n_block, n_exit, resume, child }
}

/// The landmark the threadrust oracle starts from. Task 1 Step 7 measures the exhaustive cost; see
/// the ledger's `t1-tr-cost.log` for the figure that chose this.
fn tr_oracle_from(_t: &Tr) -> usize { 1 }

// ---- Named regressions (spec §4) ----------------------------------------------------------------

/// t0 M1 (M40's R11, silent): a `continue` parked on one breakpoint pre-steps onto a second. The
/// hit is AT the landing; resolving from `kctx + 1` named the next pass of the loop instead.
#[test]
fn m1_a_pre_step_that_lands_on_a_second_breakpoint_resolves_to_it() {
    let tp = ws_trace();
    let w = discover_ws(tp);
    let next = w.store + 4;
    let (code, out, err) = hits::debug(ts(tp),
        &format!("break 0x{:x}; break 0x{next:x}; continue; continue; where", w.store));
    assert_eq!(code, 0, "stderr: {err}");
    let k = w.first_run + 1;
    assert!(out.contains(&format!("resolved (1, {k})")),
        "the pre-step lands on 0x{next:x} at K={k}; t0 M1 named K={}:\n{out}", k + 5);
    assert!(last_line(&out).ends_with(&format!("at (1, {k}) pc=0x{next:x} thread=0")), "{out}");
}

/// t0 M2 (M40's R11, loud): the same shape across a boundary — a breakpoint on `read`'s `svc` and
/// one on the next instruction. It exited 5, with a message that was not a window length.
#[test]
fn m2_a_pre_step_across_a_boundary_onto_a_breakpoint_reports_it() {
    let tp = fio_trace();
    let f = discover_fio(tp);
    let (code, out, err) = hits::debug(ts(tp),
        &format!("break 0x{:x}; break 0x{:x}; continue; continue; where", f.read_svc, f.bpc));
    assert_eq!(code, 0, "t0 M2 exited 5 here; stderr: {err}\n{out}");
    assert!(out.contains(&format!("hit 0x{:x} at ({}, 0)", f.bpc, f.after_read)), "{out}");
    assert!(last_line(&out).ends_with(&format!("at ({}, 0) pc=0x{:x} thread=0", f.after_read, f.bpc)), "{out}");
}

/// t0 M3: a breakpoint on a watched store is TWO hits at one coordinate, breakpoint then watch.
/// Forward lost the watch: the pre-step re-seeked past the store with nothing armed.
#[test]
fn m3_continue_from_a_breakpoint_on_a_watched_store_reports_the_store() {
    let tp = ws_trace();
    let w = discover_ws(tp);
    let (code, out, err) = hits::debug(ts(tp), &format!(
        "watch 0x{:x}; break 0x{:x}; continue; continue; continue; where", w.t, w.second));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("resolved (1, {})", w.k_second)), "the breakpoint first:\n{out}");
    assert!(out.contains(&format!("hit watch 0x{:x} (write at 0x{:x}) at (1, {})", w.t, w.second, w.k_second)),
        "then the store under it (t0 M3 printed `exited`):\n{out}");
    assert!(!out.contains("exited"), "{out}");
}

/// t0 M4: the same pair, backward. From the store's watch hit, the breakpoint AT that coordinate
/// precedes it; "strictly before (1, K)" excluded it.
#[test]
fn m4_reverse_continue_from_a_watch_hit_finds_the_breakpoint_on_its_store() {
    let tp = ws_trace();
    let w = discover_ws(tp);
    let (code, out, err) = hits::debug(ts(tp), &format!(
        "continue; watch 0x{:x}; break 0x{:x}; reverse-continue; reverse-continue; where", w.t, w.second));
    assert_eq!(code, 0, "stderr: {err}");
    let watch = out.find(&format!("hit watch 0x{:x} (write at 0x{:x}) at (1, {})", w.t, w.second, w.k_second));
    let bp = out.find(&format!("hit 0x{:x} at (1, {})", w.second, w.k_second));
    assert!(watch.is_some() && bp.is_some() && watch < bp,
        "the watch, then the breakpoint under it (t0 M4 jumped to K={}):\n{out}", w.k_sweep);
}

/// t0 M5: a syscall write and a breakpoint at one (n, 0). The write comes first: the event ends the
/// window before. Forward lost the breakpoint: the pre-step stepped off it unreported.
#[test]
fn m5_continue_after_a_syscall_hit_reports_the_breakpoint_at_the_same_position() {
    let tp = fio_trace();
    let f = discover_fio(tp);
    let (code, out, err) = hits::debug(ts(tp), &format!(
        "watch 0x{:x}; break 0x{:x}; continue; continue; where", f.buf, f.bpc));
    assert_eq!(code, 0, "stderr: {err}");
    let sys = out.find(&format!("hit watch 0x{:x} (syscall write) at ({}, 0)", f.buf, f.after_read));
    let bp = out.find(&format!("hit 0x{:x} at ({}, 0)", f.bpc, f.after_read));
    assert!(sys.is_some() && bp.is_some() && sys < bp,
        "the write, then the breakpoint (t0 M5 printed `exited`):\n{out}");
}

/// t0 M6: the same pair, backward. From the breakpoint at (n, 0), the syscall write at (n, 0) is
/// behind it; `(n, 0) < (pn, pk)` excluded it and said "no earlier hit".
#[test]
fn m6_reverse_continue_from_a_boundary_breakpoint_finds_the_syscall_write_behind_it() {
    let tp = fio_trace();
    let f = discover_fio(tp);
    let (code, out, err) = hits::debug(ts(tp), &format!(
        "continue; watch 0x{:x}; break 0x{:x}; reverse-continue; reverse-continue; where", f.buf, f.bpc));
    assert_eq!(code, 0, "stderr: {err}");
    let bp = out.find(&format!("hit 0x{:x} at ({}, 0)", f.bpc, f.after_read));
    let sys = out.find(&format!("hit watch 0x{:x} (syscall write) at ({}, 0)", f.buf, f.after_read));
    assert!(bp.is_some() && sys.is_some() && bp < sys, "the breakpoint, then the write behind it:\n{out}");
    assert!(!out.contains("no earlier hit"), "t0 M6 said `no earlier hit`:\n{out}");
}

/// t0 M7 (R5): arriving at (n, 0) by stepping puts the cursor at the breakpoint phase, so the
/// syscall write just behind it is found. M40 excluded it for every P at k = 0; M41 excludes it
/// only for the hit itself.
#[test]
fn m7_reverse_continue_after_stepping_back_to_a_boundary_finds_the_syscall_write() {
    let tp = fio_trace();
    let f = discover_fio(tp);
    let next = f.bpc + 4;
    let (code, out, err) = hits::debug(ts(tp), &format!(
        "watch 0x{:x}; break 0x{next:x}; continue; continue; reverse-stepi; reverse-continue; where", f.buf));
    assert_eq!(code, 0, "stderr: {err}");
    let sys = format!("hit watch 0x{:x} (syscall write) at ({}, 0)", f.buf, f.after_read);
    assert_eq!(out.matches(&sys).count(), 2, "once going forward, once going back:\n{out}");
    assert!(!out.contains("no earlier hit"), "t0 M7 said `no earlier hit`:\n{out}");
}

/// t0 M8, forward: a breakpoint on main's resume pc after its blocking `__ulock_wait`. It runs once,
/// when main resumes after the child exits. At the block the boundary check read the OUTGOING
/// thread's pc and reported a phantom; at the resume the real hit was unresolvable (exit 5).
#[test]
fn m8_continue_reports_main_resuming_once_and_no_phantom_at_its_block() {
    let tp = tr_trace();
    let t = discover_tr(tp);
    let (code, out, err) = hits::debug(ts(tp), &format!("break 0x{:x}; continue; where; continue", t.resume));
    assert_eq!(code, 0, "stderr: {err}\n{out}");
    assert!(!out.contains(&format!("at ({}, ", t.n_block)), "no phantom at main's block (t0 M8):\n{out}");
    assert!(out.contains(&format!("hit 0x{:x} at ({}, 0)", t.resume, t.n_exit)), "{out}");
    assert!(out.contains(&format!("at ({}, 0) pc=0x{:x} thread=0", t.n_exit, t.resume)), "{out}");
    assert!(out.trim_end().ends_with("exited (code 0)"), "exactly one hit:\n{out}");
}

/// t0 M8, backward: the same single hit, found from the exit (it exited 5).
#[test]
fn m8_reverse_continue_finds_main_resuming_after_the_child() {
    let tp = tr_trace();
    let t = discover_tr(tp);
    let (code, out, err) = hits::debug(ts(tp),
        &format!("continue; break 0x{:x}; reverse-continue; where; reverse-continue", t.resume));
    assert_eq!(code, 0, "stderr: {err}\n{out}");
    assert!(out.contains(&format!("hit 0x{:x} at ({}, 0)", t.resume, t.n_exit)), "{out}");
    assert!(out.trim_end().ends_with("no earlier hit"), "exactly one hit:\n{out}");
}

/// t0 M8, the missed-hit shape: a breakpoint on the child's first instruction, which runs at main's
/// block (it exited 5).
#[test]
fn m8_continue_reports_the_childs_first_instruction_on_the_child() {
    let tp = tr_trace();
    let t = discover_tr(tp);
    let (code, out, err) = hits::debug(ts(tp), &format!("break 0x{:x}; continue; where", t.child));
    assert_eq!(code, 0, "stderr: {err}\n{out}");
    assert!(out.contains(&format!("hit 0x{:x} at ({}, 0)", t.child, t.n_block)), "{out}");
    assert!(last_line(&out).ends_with(&format!("at ({}, 0) pc=0x{:x} thread=1", t.n_block, t.child)), "{out}");
}

// ---- Spec §3a's invariant -------------------------------------------------------------------------

/// At the boundary after main blocks, the position already shows the thread that retires the next
/// instruction. M15 defined it as the thread that had just blocked.
#[test]
fn at_a_blocking_boundary_the_position_shows_the_thread_that_runs_next() {
    let tp = tr_trace();
    let t = discover_tr(tp);
    let s = retrace_core::seek(tp, t.n_block, 0).unwrap();
    let sum = s.thread_summaries();
    assert!(matches!(sum[0].state, retrace_core::ThreadState::Blocked(_)), "main has blocked: {sum:?}");
    assert_eq!(s.current_thread(), 1, "so the child runs next (M15 said 0 here): {sum:?}");
    assert_eq!(s.pc(), t.child, "from its first instruction");
    assert!(sum[1].is_current && !sum[0].is_current, "{sum:?}");
}

// ---- The hit oracle (spec §3e) ------------------------------------------------------------------

#[test]
fn oracle_watchsweep_two_adjacent_breakpoints() {
    let tp = ws_trace();
    let w = discover_ws(tp);
    let bps = [w.store, w.store + 4];
    let hits = hits::enumerate_hits(tp, &bps, &[], 1);
    // Self-check from the fixture's source: 64 passes through the loop, once through each.
    assert_eq!(hits.len(), 128, "{hits:?}");
    assert!(hits.iter().all(|h| (h.n, h.phase, h.thread) == (1, Phase::Bp, 0)), "{hits:?}");
    assert_eq!((hits[0].k, hits[1].k, hits[2].k), (w.first_run, w.first_run + 1, w.first_run + 5));
    hits::check_chains(ts(tp), &format!("break 0x{:x}; break 0x{:x}", bps[0], bps[1]), &hits);
}

#[test]
fn oracle_watchsweep_a_breakpoint_on_a_watched_store() {
    let tp = ws_trace();
    let w = discover_ws(tp);
    let hits = hits::enumerate_hits(tp, &[w.second], &[(w.t, 8)], 1);
    let shape: Vec<(u64, Phase)> = hits.iter().map(|h| (h.k, h.phase)).collect();
    assert_eq!(shape, vec![(w.k_sweep, Phase::Watch), (w.k_second, Phase::Bp), (w.k_second, Phase::Watch)],
        "{hits:?}");
    hits::check_chains(ts(tp), &format!("watch 0x{:x}; break 0x{:x}", w.t, w.second), &hits);
}

#[test]
fn oracle_fileio_breakpoints_either_side_of_a_boundary() {
    let tp = fio_trace();
    let f = discover_fio(tp);
    let hits = hits::enumerate_hits(tp, &[f.read_svc, f.bpc], &[], 1);
    let shape: Vec<(usize, u64, Phase)> = hits.iter().map(|h| (h.n, h.k, h.phase)).collect();
    assert_eq!(shape, vec![(f.read_win, f.k_read, Phase::Bp), (f.after_read, 0, Phase::Bp)], "{hits:?}");
    hits::check_chains(ts(tp), &format!("break 0x{:x}; break 0x{:x}", f.read_svc, f.bpc), &hits);
}

#[test]
fn oracle_fileio_a_syscall_write_and_a_breakpoint_at_one_boundary() {
    let tp = fio_trace();
    let f = discover_fio(tp);
    let hits = hits::enumerate_hits(tp, &[f.bpc], &[(f.buf, 8)], 1);
    let shape: Vec<(usize, u64, Phase)> = hits.iter().map(|h| (h.n, h.k, h.phase)).collect();
    assert_eq!(shape, vec![(f.after_read, 0, Phase::Sys), (f.after_read, 0, Phase::Bp)], "{hits:?}");
    hits::check_chains(ts(tp), &format!("watch 0x{:x}; break 0x{:x}", f.buf, f.bpc), &hits);
}

#[test]
fn oracle_threadrust_breakpoints_at_both_switches() {
    let tp = tr_trace();
    let t = discover_tr(tp);
    let hits = hits::enumerate_hits(tp, &[t.resume, t.child], &[], tr_oracle_from(&t));
    let shape: Vec<(usize, u64, Phase, u64, u32)> = hits.iter().map(|h| (h.n, h.k, h.phase, h.pc, h.thread)).collect();
    assert_eq!(shape, vec![(t.n_block, 0, Phase::Bp, t.child, 1), (t.n_exit, 0, Phase::Bp, t.resume, 0)],
        "the child's first instruction on the child at main's block, then main resuming: {hits:?}");
    hits::check_chains(ts(tp), &format!("break 0x{:x}; break 0x{:x}", t.resume, t.child), &hits);
}
```

- [ ] **Step 6: Run and confirm RED for the right reasons**

```bash
L=.superpowers/sdd/2026-09-24-retrace-m41-hitorder
cargo test -p retrace --test hitorder_e2e --no-fail-fast -- --test-threads=1 > $L/t1-red.log 2>&1; echo "exit=$?"
grep -a -E '^test |panicked|answer #' $L/t1-red.log
```

Expected: **every test FAILS, each for its t0 reason.** Check that each failure message says what
t0 measured:

| Test | Expected failure |
|---|---|
| `m1_…` | `resolved (1, 14)` shown |
| `m2_…` | exit 5 |
| `m3_…` | `exited` |
| `m4_…` | watch at `k_sweep` |
| `m5_…` | `exited` |
| `m6_…` | `no earlier hit` |
| `m7_…` | `no earlier hit` |
| `m8_continue_reports_main…` | phantom at `n_block` |
| `m8_reverse…` | exit 5 |
| `m8_…child…` | exit 5 |
| `at_a_blocking_boundary…` | `current_thread` 0 |
| The five `oracle_…` | Each fails in `check_chains`, **after** its self-check asserts pass. The panic names a chain and an answer number. A self-check failure is the oracle disagreeing with the fixture's source: **stop and report it.** |

If any test passes, or fails for a different reason, **stop and report**. A RED that passes on the
old code tests nothing.

- [ ] **Step 7: Measure the threadrust oracle's cost (R9) and choose its start**

```bash
L=.superpowers/sdd/2026-09-24-retrace-m41-hitorder
cargo test -p retrace --test hitorder_e2e --no-run > /dev/null 2>&1
/usr/bin/time -l cargo test -p retrace --test hitorder_e2e oracle_threadrust -- --test-threads=1 > $L/t1-tr-cost.log 2>&1
grep -a -E 'user|sys|maximum resident' $L/t1-tr-cost.log
```

This RED run is dominated by `enumerate_hits`, since its `check_chains` fails at the forward chain's
first answer. The budget for the test when green is **≤ 120 s user+sys** (spec §5).
- **Enumeration alone above ~90 s CPU:** change `tr_oracle_from` to return `t.n_create`. That's
  sound, because neither breakpoint can execute before the child exists: `t.child` is the new
  thread's first instruction, and `t.resume` follows the trace's only 515. Record the ruling (R12)
  with the figure.
- **Otherwise:** keep `1`, and record the figure in the ledger.
- **If `enumerate_hits` panics with `oracle: a guest fault` before `n_create`:** a real guest
  fault inside dyld or `std` init. Take the same `n_create` fallback, which is sound for the same
  reason, and record R12.

- [ ] **Step 8: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/retrace-core/src/lib.rs crates/retrace-core/tests/replay.rs \
        crates/retrace/tests/util/mod.rs crates/retrace/tests/util/hits.rs crates/retrace/tests/hitorder_e2e.rs
git commit -m "M41 t1: step_armed, the hit oracle, and the REDs (M1-M8, the §3a invariant, five oracle armings)

<Step 6's failure per test, one line each; Step 7's CPU figure and the oracle start it chose>

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: The thread switch happens when the event finishes (spec §3a)

**Files:**
- Modify: `crates/retrace-box/src/lib.rs`:
  - `run()` entry (~line 2649);
  - `step()` entry (~line 2779);
  - add `settle_schedule` before `schedule_after_block` (~line 5503).
- Modify: `crates/retrace-core/src/lib.rs`:
  - `finish_event` (~line 1489);
  - the docs of `position` (~line 2764), `pc` (~line 2767) and `current_thread` (~line 2683).
- Modify: `crates/retrace/src/debug.rs`: two comments, `resolve_nth`'s doc (~line 229) and
  `reverse-continue`'s phase-2 read (~line 893).
- Modify: `crates/retrace/tests/blockedctx.rs` (header, lines 11–13).

**Interfaces:**
- Consumes: Task 1's tests.
- Produces: `pub fn settle_schedule(&mut self)` on `Box_`, and a `ReplaySession` whose `(n, 0)`
  after a blocking event shows the incoming thread. Task 3's R11 fix depends on it: spec §3c
  step 4 reads `pc()` at the scan's start.

- [ ] **Step 1: `Box_::settle_schedule`**

In `crates/retrace-box/src/lib.rs`, directly above `pub fn schedule_after_block`:

```rust
    /// M41 §3a: make a pending reschedule NOW — the switch `run()` and `step()` would otherwise
    /// make on their next entry. Idempotent (a no-op while the current thread is runnable), and it
    /// is the same switch at the same point in the guest's own syscall sequence, so the schedule
    /// stays a pure function of it (symmetry rule 2). `ReplaySession::finish_event` calls it, so a
    /// debugger position (n, 0) after a blocking event already shows the thread that runs next.
    pub fn settle_schedule(&mut self) {
        if self.threads.needs_reschedule() {
            self.schedule_after_block();
        }
    }
```

In `run()` **and** in `step()`, replace the three lines

```rust
        if self.threads.needs_reschedule() {
            self.schedule_after_block();
        }
```

with `self.settle_schedule();`, keeping every comment around them as it is. In `step()`, the
call stays exactly where the old `if` was, **above** the SS arming. That placement is load-bearing
(the comment there says why).

- [ ] **Step 2: `finish_event` settles, after the `WatchSyscall` tag**

In `crates/retrace-core/src/lib.rs`, replace `finish_event` (doc comment included) with:

```rust
    /// Finish consuming one trace event: bump idx and report it — as `WatchSyscall` if this event's
    /// applied writes overlapped an armed watch range (the event is consumed identically either
    /// way; only the report differs), else as plain `Event`.
    ///
    /// M41 §3a: then settle the schedule, so that at the position this event leads to, (n, 0),
    /// `pc()` and `current_thread()` name the instruction the next step retires and the thread that
    /// retires it — after a blocking event, the INCOMING thread. Ordered AFTER the `WatchSyscall`
    /// tag is taken, because a syscall's write belongs to the thread that issued it. Every
    /// event-consuming dispatch path returns through here (M41 spec §3a), and each arm's
    /// `verify_thread` has already run, so the divergence oracle still compares the issuing
    /// thread. Record never calls this; its `run()` makes the same switch on entry.
    fn finish_event(&mut self) -> Result<Advance, Divergence> {
        self.idx += 1;
        let adv = match self.b.take_syscall_watch_hit() {
            Some((watched, _ipa)) => Advance::WatchSyscall { watched, thread: self.current_thread() },
            None => Advance::Event,
        };
        self.b.settle_schedule();
        Ok(adv)
    }
```

Re-grep the spec §3a claim and put the output in the ledger:

```bash
grep -n "Ok(Advance::Event)\|Advance::WatchSyscall {" crates/retrace-core/src/lib.rs
```

Expected: both constructions appear only inside `finish_event` (the other hits are the `enum`
definition and doc text). If an event-consuming path constructs either one elsewhere, stop and
report it. It would skip the settle.

- [ ] **Step 3: Rewrite the docs M15 wrote**

`current_thread`: replace its doc paragraph that begins
`**At a landmark boundary \`(N, 0)\` this names the thread that ISSUED landmark \`N\`'s syscall`
(through `…surprising the first time you see it.`) with:

```rust
    /// **M41 §3a: at every position, including a landmark boundary `(N, 0)`, this names the thread
    /// that retires the next instruction.** `finish_event` settles a pending switch before a
    /// boundary is ever observed, so after a BLOCKING syscall (`__ulock_wait`, `bsdthread_terminate`,
    /// `semaphore_wait_trap`, a workq park) `(N, 0)` shows the INCOMING thread. Until M41 it showed
    /// the thread that had just blocked (M15's definition), which left every breakpoint counted by
    /// `pc()` at such a boundary blind: a phantom hit on the outgoing thread's resume pc and a missed
    /// one on the incoming thread's first instruction (M41 t0 M8). The divergence oracle is
    /// unaffected: each dispatch arm's `verify_thread` runs before `finish_event`, against the
    /// thread that issued the syscall.
```

`pc`: append one sentence to its doc: `M41 §3a: at a boundary after a blocking event this is the
INCOMING thread's pc — the instruction the next step retires.`

`position`: replace `Coincides with \`pc()\` only at a landmark boundary (K=0).` with
`Coincides with \`pc()\` at a landmark boundary (K=0) reached through a non-blocking event; after a
blocking one (M41 §3a) the vCPU holds the incoming thread, and this is that thread's own saved ELR.`

In `crates/retrace/src/debug.rs`:
- In `resolve_nth`'s doc, replace `— every hit of one breakpoint, every run of one loop store, e.g.
  README's Known-limits case of a breakpoint at a thread switch —` with `— every hit of one
  breakpoint, every run of one loop store —`.
- In `cmd_reverse_continue`'s phase 2, replace the comment

  ```rust
                    // Read BEFORE the step: the breakpoint check compares the pc about to execute.
                    // At (pn, 0) with a thread switch pending this is the OUTGOING thread's resume
                    // pc (step() switches on entry) — the documented blind spot in README's Known
                    // limits.
  ```

  with

  ```rust
                    // Read BEFORE the step: the breakpoint check compares the pc about to execute.
                    // At (pn, 0) after a blocking event that is the INCOMING thread's (M41 §3a).
  ```

In `crates/retrace/tests/blockedctx.rs`, replace `live vCPU; the switch that saves it happens on the
next \`run()\`.` with `live vCPU; the switch that saves it happens when that event finishes
(M41 §3a — before M41, on the next \`run()\`; nothing runs in between, so the saved bytes are the
same).`

- [ ] **Step 4: Run the §3a guards and the threaded suites (halt rule)**

```bash
L=.superpowers/sdd/2026-09-24-retrace-m41-hitorder
cargo test -p retrace --test hitorder_e2e --no-fail-fast -- --test-threads=1 > $L/t2-hitorder.log 2>&1; echo "hitorder exit=$?"
grep -a -E '^test ' $L/t2-hitorder.log
for t in blockedctx debug_cli thread_oracle thread_rust_e2e thread_watch_e2e sigthread_e2e; do
  cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t2-$t.log 2>&1; echo "$t exit=$?"; done
```

Then, as a second call (the 10-minute cap):

```bash
L=.superpowers/sdd/2026-09-24-retrace-m41-hitorder
for t in sigblocked_e2e dispatch_e2e checkpoint_seek seek kport; do
  cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t2-$t.log 2>&1; echo "$t exit=$?"; done
cargo test -p retrace-box --test threads -- --test-threads=1 > $L/t2-box-threads.log 2>&1; echo "box threads exit=$?"
cargo test -p retrace-core --no-fail-fast -- --test-threads=1 > $L/t2-core.log 2>&1; echo "core exit=$?"
```

Expected:
- **`hitorder_e2e`:** the three `m8_…` tests, `at_a_blocking_boundary…` and
  `oracle_threadrust_breakpoints_at_both_switches` **PASS**. `m1_…`–`m7_…` and the other four
  `oracle_…` still fail exactly as in Task 1.
- **Every other suite:** exit 0, including both `blockedctx` measurements.

**Halt rule (spec §7): if any other suite fails, stop and report it with its log.** It would mean
something depends on the lazy switch. That is a design question for the operator, not a patch.

Re-measure `oracle_threadrust` green with `/usr/bin/time -l`, as in Task 1 Step 7, and record the
figure. It must be ≤ 120 s user+sys. If it is over, take Task 1 Step 7's `n_create` fallback and
record R12.

- [ ] **Step 5: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs crates/retrace/src/debug.rs crates/retrace/tests/blockedctx.rs
git commit -m "M41 t2: the thread switch happens when the event finishes (spec §3a)

<which tests went green, the threaded suites' results, and the threadrust oracle's CPU figure>

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: The cursor, and `continue` under it (spec §3b, §3c; M40's R11; R7, R8, R10)

**Files:**
- Modify: `crates/retrace/src/debug.rs`:
  - the `use` line;
  - `resolve_nth` (~line 247);
  - `Exec` (~line 299), `Exec::new` and `reseek`;
  - `cmd_stepi`;
  - `cmd_continue`, replaced whole;
  - `cmd_reverse_continue`'s answer bookkeeping;
  - `mod tests`.
- Modify: `crates/retrace/tests/hitorder_e2e.rs` (append the Review Focus tests).

**Interfaces:**
- Consumes: `retrace_core::Armed`, `ReplaySession::step_armed` (Task 1); the §3a settle (Task 2).
- Produces: `enum Phase { Sys, Bp, Watch }` (private, `Ord`) and `Exec.phase`. Task 4 reads
  `self.phase` as `pphase`.

- [ ] **Step 1: Write the Review Focus tests first**

Append to `crates/retrace/tests/hitorder_e2e.rs`:

```rust
// ---- Review Focus (plan) ------------------------------------------------------------------------

/// Review Focus 1 / R10: `continue` from a breakpoint on the crashing instruction reports the
/// crash, as `continue` without it does. Measured at plan time on c68ba6d: exit 5,
/// "DEBUG ERROR: guest crashed at step 0/1".
#[test]
fn rf1_continue_from_a_breakpoint_on_the_crashing_instruction_reports_the_crash() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "crashy records its crash: {}", rec.stderr);
    let pc = retrace_trace::Reader::open(&trace).unwrap().iter().find_map(|e| match e {
        retrace_trace::Event::Crash { pc, .. } => Some(*pc),
        _ => None,
    }).expect("a recorded Crash");
    let (code, out, err) = hits::debug(ts(&trace), &format!("break 0x{pc:x}; continue; continue; continue"));
    assert_eq!(code, 0, "stderr: {err}\n{out}");
    assert!(out.contains(&format!("hit 0x{pc:x} at (")), "the breakpoint first:\n{out}");
    assert_eq!(out.matches(&format!("guest crashed: pc=0x{pc:x}")).count(), 2,
        "then the crash, and the crash again:\n{out}");
}

/// Review Focus 2 / R8: a watch scoped to a thread that never writes it. Every hit is scoped out,
/// so `continue` runs to the end without reporting one, and goes through the loop that replaced
/// M15 Task 8's recursion.
#[test]
fn rf2_a_watch_scoped_to_a_thread_that_never_writes_it_runs_to_the_end() {
    let tp = ws_trace();
    let w = discover_ws(tp);
    let (code, out, err) = hits::debug(ts(tp), &format!("watch 0x{:x} thread 1; continue; where", w.t));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(!out.contains("hit watch"), "{out}");
    assert!(out.contains("exited (code 0)"), "{out}");
}

/// Review Focus 3: `unwatch` while parked on a breakpoint whose instruction is a watched store. What
/// is armed now decides, so the store is not reported.
#[test]
fn rf3_unwatch_while_parked_on_a_watched_store_is_not_reported_after() {
    let tp = ws_trace();
    let w = discover_ws(tp);
    let (code, out, err) = hits::debug(ts(tp), &format!(
        "watch 0x{:x}; break 0x{:x}; continue; continue; unwatch 0x{:x}; continue", w.t, w.second, w.t));
    assert_eq!(code, 0, "stderr: {err}");
    assert_eq!(out.matches("hit watch").count(), 1, "the sweep's write only:\n{out}");
    assert!(out.trim_end().ends_with("exited (code 0)"), "{out}");
}

/// Review Focus 4: `continue` after the end repeats the end, and is never an error.
#[test]
fn rf4_continue_after_the_guest_exited_repeats_the_exit() {
    let tp = ws_trace();
    let (code, out, err) = hits::debug(ts(tp), "continue; continue");
    assert_eq!(code, 0, "stderr: {err}");
    assert_eq!(out.matches("exited (code 0)").count(), 2, "{out}");
}

/// Review Focus 5: "no earlier hit" leaves the cursor where it was. From a watch-phase park, the
/// next `continue` steps over the store rather than reporting it a second time.
#[test]
fn rf5_no_earlier_hit_keeps_the_cursor_on_the_store() {
    let tp = ws_trace();
    let w = discover_ws(tp);
    let buf0 = w.t - 320; // buf[0]: written once, by the sweep's first pass
    let (code, out, err) = hits::debug(ts(tp),
        &format!("watch 0x{buf0:x}; continue; reverse-continue; continue"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("resolved (1, {})", w.first_run)), "{out}");
    assert!(out.contains("no earlier hit"), "{out}");
    assert_eq!(out.matches("hit watch").count(), 1, "the store is not reported twice:\n{out}");
    assert!(out.trim_end().ends_with("exited (code 0)"), "{out}");
}
```

Run them. Expected: `rf1_…` **FAILS** (exit 5, `DEBUG ERROR: guest crashed at step 0/1`), and
`rf2_…`–`rf5_…` **PASS** on the Task 2 tree. They pin behaviour the cursor rewrite must keep. If
`rf1` passes, or any of the others fails, stop and report it.

```bash
L=.superpowers/sdd/2026-09-24-retrace-m41-hitorder
cargo test -p retrace --test hitorder_e2e rf -- --test-threads=1 > $L/t3-rf-before.log 2>&1; echo "exit=$?"
grep -a -E '^test |panicked' $L/t3-rf-before.log
```

- [ ] **Step 2: `Phase`, the cursor field, and the arrival rule**

In `crates/retrace/src/debug.rs`:

Change the `use` line to add `Armed`:

```rust
use retrace_core::{checkpointed_seek, Advance, Armed, CheckpointCache, Outcome, ReplayReport, ReplaySession, Stepped};
```

Directly above `struct Exec`, add:

```rust
/// M41 §3b: where a hit sits within one coordinate (n, k), in the order the hardware produces them.
/// Hits are totally ordered by `(n, k, Phase)`; the debugger's cursor is such a triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    /// A syscall's recorded write to a watched range. Only at k = 0: the event that ended window
    /// n − 1 wrote it before the instruction at (n, 0) runs.
    Sys,
    /// The instruction at (n, k) is about to execute and its address is a breakpoint. Also every
    /// ARRIVAL's phase (spec R4, gdb's rule): a breakpoint at the pc you stand on is reported in
    /// neither direction, and a watched store you stepped up to still fires going forward.
    Bp,
    /// That instruction's store to a watched range, stopped pre-retire.
    Watch,
}
```

In `struct Exec`, replace the `last_watch_hit` field and its doc comment with:

```rust
    /// M41: the cursor's phase at (n, k) — see `Phase`. `continue` answers the first hit after
    /// (n, k, phase), `reverse-continue` the last one before it. Replaces M5's `last_watch_hit`,
    /// which was `phase == Watch` in disguise. Every position at one (n, k) is the same machine
    /// state (before the instruction at (n, k); a watch stop is pre-retire), so no session encodes
    /// the phase.
    phase: Phase,
```

In `Exec::new`, replace `last_watch_hit: None,` with `phase: Phase::Bp,`.

In `reseek`, after `self.k = k;`, add:

```rust
        self.phase = Phase::Bp; // an arrival (R4); a caller that parks on a hit sets the hit's phase after
```

In `cmd_stepi`, replace the body's `match` with:

```rust
        match res {
            Ok(()) => { self.k += count; self.phase = Phase::Bp; Ok(()) } // an arrival (R4)
            Err(msg) => {
                let head = msg.split("; cannot step").next().unwrap_or(&msg);
                line(out, format_args!("error: {head}"))?;
                // The position did not change, so neither does the cursor.
                let (n0, k0, p0) = (self.n, self.k, self.phase);
                self.reseek(n0, k0)?;
                self.phase = p0;
                Ok(())
            }
        }
```

- [ ] **Step 3: R7 — `resolve_nth` names what it searched**

In `resolve_nth`'s `HitKind::Break` loop, replace

```rust
            s.step_insns(1).map_err(|e| format!("resolve breakpoint hit #{ordinal} in window {n}: {e}"))?;
```

with

```rust
            // M41 R7: name what was searched. The step is one instruction per call, so its own
            // "window N ends after 0 instruction(s)" read like a window length and was not one
            // (M41 t0 M2).
            s.step_insns(1).map_err(|e| if e.contains("ends after") {
                format!("resolve breakpoint hit #{ordinal} in window {n}: the window ended after {seen} hit(s) at K={k}")
            } else {
                format!("resolve breakpoint hit #{ordinal} in window {n}: {e}")
            })?;
```

- [ ] **Step 4: Replace `cmd_continue`**

Replace the whole of `cmd_continue` (its doc comment through its closing brace) with:

```rust
    /// Run forward to the first hit after the cursor (M41 §3c), or to the guest's end.
    ///
    /// **Finish** the current coordinate first:
    /// - a breakpoint that a syscall hit left ahead of the cursor at (n, 0) is reported;
    /// - if the cursor stands on a breakpoint or on a watched store, the instruction is executed,
    ///   with the watches armed unless the cursor is ON its store. So a store under a breakpoint
    ///   still reports (t0 M3), and one already reported is stepped over;
    /// - a trap or a fault there is crossed with `advance()`, with the watches armed for that one
    ///   event (R10).
    ///
    /// Then **scan** at native speed with hardware breakpoints (one DBGBVR slot each; ≤ 6) and
    /// watchpoints armed. A mid-window hit resolves to an exact (N, K) from `kctx`, and the landmark
    /// check catches a breakpoint exactly on a boundary. With nothing armed, runs to the end.
    fn cmd_continue<W: Write>(&mut self, out: &mut W) -> Result<(), String> {
        let bps = self.breakpoints.clone();
        let ws: Vec<(u64, u64)> = self.watches.iter().map(|&(a, l, _)| (a, l)).collect();
        let diverged = |d: retrace_core::Divergence|
            format!("continue diverged at landmark {} pc {:#x}: {}", d.landmark, d.pc, d.detail);
        'finish: loop {
            // ---- Finish (n, k): its hits still ahead of the cursor ----
            loop {
                let pc = self.sess().pc();
                let on_bp = bps.contains(&pc);
                if self.phase == Phase::Sys && on_bp {
                    // A breakpoint behind a syscall hit at (n, 0) is still ahead (t0 M5).
                    let a = self.annot(pc);
                    line(out, format_args!("hit {pc:#x} at ({}, 0){a}", self.n))?;
                    self.phase = Phase::Bp;
                    return Ok(());
                }
                if !on_bp && self.phase != Phase::Watch {
                    break; // nothing here is behind the cursor: the scan reports whatever is next
                }
                // Execute the instruction at (n, k). Its store is still ahead of the cursor unless
                // the cursor is ON it, so arm the watches exactly then.
                let arm = self.phase != Phase::Watch;
                if arm { self.sess_mut().arm_watchpoints(&ws); }
                let stepped = self.sess_mut().step_armed();
                if arm { self.sess_mut().clear_watchpoints(); } // the kept-session invariant
                match stepped? {
                    Armed::Retired => { self.k += 1; break; } // (n, k + 1): nothing there examined yet
                    Armed::Watch => {
                        let (n, k) = (self.n, self.k);
                        let watched = watched_of(&ws, self.sess().far());
                        let thread = self.sess().current_thread();
                        self.phase = Phase::Watch; // parked pre-retire on the store
                        if self.watch_thread_matches(watched, thread) {
                            let a = self.annot(pc);
                            line(out, format_args!("hit watch {watched:#x} (write at {pc:#x}) at ({n}, {k}){a}"))?;
                            return Ok(());
                        }
                        // Scoped out (M15 Task 8): passed, not reported. The next turn steps over it.
                    }
                    Armed::AtTrap | Armed::Fault => {
                        // The instruction ends the window (a trap) or faults (R10): nothing retired.
                        // `advance()` consumes the trap, or delivers or ends the fault, from a fresh
                        // session parked before it — the pre-M41 crossing's own move — with the
                        // watches armed for that one event, so a syscall write to a watched range is
                        // reported (M5 final-review M-1), and breakpoints off, so the parked
                        // position is not re-reported. No instruction retires during it.
                        let (n, k) = (self.n, self.k);
                        self.reseek(n, k)?;
                        self.sess_mut().arm_watchpoints(&ws);
                        let adv = self.sess_mut().advance().map_err(diverged)?;
                        match adv {
                            Advance::Exited(report) => return self.park_at_terminal(report, out),
                            Advance::WatchSyscall { watched, thread } if self.watch_thread_matches(watched, thread) => {
                                self.sess_mut().clear_watchpoints();
                                let n = self.sess().landmark();
                                line(out, format_args!("hit watch {watched:#x} (syscall write) at ({n}, 0)"))?;
                                (self.n, self.k, self.phase) = (n, 0, Phase::Sys);
                                return Ok(());
                            }
                            Advance::Event | Advance::WatchSyscall { .. } => {
                                // A plain event, or a syscall write scoped out by thread: at
                                // (n + 1, 0) only the Sys phase is behind the cursor.
                                self.sess_mut().clear_watchpoints();
                                let n = self.sess().landmark();
                                (self.n, self.k, self.phase) = (n, 0, Phase::Sys);
                            }
                            Advance::Break | Advance::Watch { .. } => return Err(
                                "continue: an instruction retired during a one-event crossing".into()),
                        }
                    }
                    Armed::Break => return Err(
                        "continue: a breakpoint stop with no breakpoint armed (the kept-session invariant)".into()),
                }
            }
            // ---- Scan from (n, k): the finish left nothing here behind the cursor ----
            let (start_n, start_k) = (self.n, self.k);
            self.sess_mut().arm_breakpoints(&bps);
            self.sess_mut().arm_watchpoints(&ws);
            loop {
                match self.sess_mut().advance().map_err(diverged)? {
                    Advance::Break => {
                        let n = self.sess().landmark();
                        let p_hit = self.sess().pc();
                        let a = self.annot(p_hit);
                        line(out, format_args!("hit {p_hit:#x} at ({n}, +?){a}"))?;
                        // M40's R11, fixed: resolve FROM kctx, not kctx + 1. In the start window
                        // kctx is the scan's start, where a breakpoint is still ahead (the finish
                        // stepped off any it had passed); in a later window kctx is 0, and a hit at
                        // (n, 0) is found rather than skipped.
                        let kctx = if n == start_n { start_k } else { 0 };
                        self.session = None; // free the VM before the resolution seek
                        let k = resolve_nth(self.trace, &mut self.cache, n, kctx, HitKind::Break(&[p_hit]), 1, p_hit)?;
                        line(out, format_args!("resolved ({n}, {k})"))?;
                        return self.reseek(n, k); // phase Bp
                    }
                    Advance::Event => {
                        let pc = self.sess().pc(); // the INCOMING thread's after a block (§3a)
                        if bps.contains(&pc) {
                            let n = self.sess().landmark();
                            let a = self.annot(pc);
                            line(out, format_args!("hit {pc:#x} at ({n}, 0){a}"))?;
                            self.sess_mut().clear_breakpoints(); // keep this session, hit-clean
                            self.sess_mut().clear_watchpoints();
                            (self.n, self.k, self.phase) = (n, 0, Phase::Bp);
                            return Ok(());
                        }
                        // no boundary match; keep scanning (hardware breakpoints stay armed)
                    }
                    Advance::Exited(report) => return self.park_at_terminal(report, out),
                    // Both watch arms name their `thread` field (not `..`): M15 Task 5 plumbs the
                    // writing thread through `Advance`, and naming it keeps every site that drops
                    // it greppable.
                    Advance::Watch { thread } => {
                        let n = self.sess().landmark();
                        let p_hit = self.sess().pc();
                        let watched = watched_of(&ws, self.sess().far());
                        let matched = self.watch_thread_matches(watched, thread);
                        if matched {
                            let a = self.annot(p_hit);
                            line(out, format_args!("hit watch {watched:#x} (write at {p_hit:#x}) at ({n}, +?){a}"))?;
                        }
                        // The FIRST watch stop from kctx is this hit, found by the hardware BY
                        // ADDRESS (M40). This resolution runs whether or not the hit is scoped out:
                        // the vCPU is physically parked pre-retire at the store either way.
                        let kctx = if n == start_n { start_k } else { 0 };
                        self.session = None; // free the VM before the resolution seek
                        let k = resolve_nth(self.trace, &mut self.cache, n, kctx, HitKind::Watch(&ws), 1, p_hit)?;
                        if matched {
                            line(out, format_args!("resolved ({n}, {k})"))?;
                        }
                        self.reseek(n, k)?;
                        self.phase = Phase::Watch;
                        if matched {
                            return Ok(());
                        }
                        // Scoped out (M15 Task 8): the finish steps over it. R8: a loop where M15
                        // recursed into this function, which could overflow the stack on a hot
                        // scoped-out write loop.
                        continue 'finish;
                    }
                    Advance::WatchSyscall { watched, thread } => {
                        if self.watch_thread_matches(watched, thread) {
                            let n = self.sess().landmark();
                            line(out, format_args!("hit watch {watched:#x} (syscall write) at ({n}, 0)"))?;
                            self.sess_mut().clear_breakpoints(); // keep this session, hit-clean
                            self.sess_mut().clear_watchpoints();
                            (self.n, self.k, self.phase) = (n, 0, Phase::Sys);
                            return Ok(());
                        }
                        // Scoped out: the writing event is already consumed, so keep scanning.
                    }
                }
            }
        }
    }
```

- [ ] **Step 5: `reverse-continue` keeps the cursor's bookkeeping (its order changes in Task 4)**

In `cmd_reverse_continue`:
- Replace `let (pn, pk) = (self.n, self.k);` with `let (pn, pk, pphase) = (self.n, self.k, self.phase);`.
- Replace the answer arms at the end so the cursor records what was reported:

```rust
        match last {
            Some((n, RHit::Bp { pc, ord })) => {
                let k = resolve_nth(self.trace, &mut self.cache, n, 0, HitKind::Break(&bps), ord, pc)?;
                before_p(n, k)?;
                let a = self.annot(pc);
                line(out, format_args!("hit {pc:#x} at ({n}, {k}){a}"))?;
                self.reseek(n, k) // phase Bp
            }
            Some((n, RHit::Watch { watched, pc, ord })) => {
                let k = resolve_nth(self.trace, &mut self.cache, n, 0, HitKind::Watch(&ws), ord, pc)?;
                before_p(n, k)?;
                let a = self.annot(pc);
                line(out, format_args!("hit watch {watched:#x} (write at {pc:#x}) at ({n}, {k}){a}"))?;
                self.reseek(n, k)?;
                self.phase = Phase::Watch;
                Ok(())
            }
            Some((n, RHit::WatchSys { watched })) => {
                line(out, format_args!("hit watch {watched:#x} (syscall write) at ({n}, 0)"))?;
                self.reseek(n, 0)?;
                self.phase = Phase::Sys;
                Ok(())
            }
            None => {
                line(out, format_args!("no earlier hit"))?;
                // The cursor stays where it was (Review Focus 5).
                self.reseek(pn, pk)?;
                self.phase = pphase;
                Ok(())
            }
        }
```

- Run `grep -n last_watch_hit crates/retrace/src/debug.rs`. Expected: no output.

- [ ] **Step 6: Two unit tests**

In `crates/retrace/src/debug.rs`'s `mod tests`, after `a_debug_session_decodes_its_trace_once`:

```rust
    #[test] fn hits_order_syscall_write_then_breakpoint_then_watch() {
        // M41 §3b: the hardware's own order at one coordinate, and the tuple order the cursor
        // compares with.
        assert!(Phase::Sys < Phase::Bp && Phase::Bp < Phase::Watch);
        assert!((4usize, 0u64, Phase::Watch) < (4, 1, Phase::Sys), "K dominates the phase");
        assert!((3usize, 9u64, Phase::Watch) < (4, 0, Phase::Sys), "N dominates K");
    }

    #[test] fn a_breakpoint_you_stepped_onto_is_reported_in_neither_direction() {
        // M41 R4 (gdb's rule): arriving by `stepi` puts the cursor AT the breakpoint phase, so
        // `continue` goes to the next pass and `reverse-continue` finds nothing earlier.
        let trace = record_watchsweep("arrival");
        let mut ex = Exec::new(&trace).unwrap();
        let mut sink = Vec::new();
        run_cmds(&mut ex, "stepi 8", &mut sink);
        let store = ex.sess().pc(); // the sweeping store's first pass
        run_cmds(&mut ex, &format!("break 0x{store:x}"), &mut sink);
        assert_eq!((ex.n, ex.k, ex.phase), (1, 8, Phase::Bp));
        run_cmds(&mut ex, "continue", &mut sink);
        let text = String::from_utf8_lossy(&sink).into_owned();
        // The loop body is five instructions, so the next pass is K = 13.
        assert!(text.contains("resolved (1, 13)"), "continue skips the breakpoint it stands on:\n{text}");

        let mut ex = Exec::new(&trace).unwrap();
        let mut sink = Vec::new();
        run_cmds(&mut ex, &format!("stepi 8; break 0x{store:x}; reverse-continue"), &mut sink);
        let text = String::from_utf8_lossy(&sink).into_owned();
        assert!(text.contains("no earlier hit"), "…and so does reverse-continue:\n{text}");
        assert_eq!((ex.n, ex.k, ex.phase), (1, 8, Phase::Bp), "the cursor is where it was");
    }
```

- [ ] **Step 7: Run and confirm what went green**

```bash
L=.superpowers/sdd/2026-09-24-retrace-m41-hitorder
cargo test -p retrace --test hitorder_e2e --no-fail-fast -- --test-threads=1 > $L/t3-hitorder.log 2>&1; echo "hitorder exit=$?"
grep -a -E '^test ' $L/t3-hitorder.log
cargo test -p retrace --bins -- --test-threads=1 > $L/t3-bins.log 2>&1; echo "bins exit=$?"
for t in debug_cli watch_cli watch watch_dyn watchsweep_e2e; do
  cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t3-$t.log 2>&1; echo "$t exit=$?"; done
```

Then, as a second call:

```bash
L=.superpowers/sdd/2026-09-24-retrace-m41-hitorder
for t in thread_watch_e2e crashy_cli crashy_e2e reverse_debug_e2e checkpoint_seek sigcatch_dyn_e2e symbolops_e2e symbols_e2e; do
  cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t3-$t.log 2>&1; echo "$t exit=$?"; done
```

Expected:
- **`hitorder_e2e` passes:**
  - `m1_…`, `m2_…`, `m3_…`, `m5_…`;
  - all `m8_…` and the invariant;
  - `rf1_…`–`rf5_…`;
  - `oracle_watchsweep_two_adjacent_breakpoints`, `oracle_fileio_breakpoints_either_side_of_a_boundary`
    and `oracle_threadrust_…`.
- **Still failing, each as in Task 1:** `m4_…`, `m6_…`, `m7_…`,
  `oracle_watchsweep_a_breakpoint_on_a_watched_store` and
  `oracle_fileio_a_syscall_write_and_a_breakpoint_at_one_boundary`. Both oracles fail in their
  backward chain or zig-zag, **not** their forward chain. Record which answer each names.
- **`--bins`:** 16 tests pass.
- **Every other suite:** exit 0, with transcripts unchanged.

If an existing assertion moves, stop and report it.

- [ ] **Step 8: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/retrace/src/debug.rs crates/retrace/tests/hitorder_e2e.rs
git commit -m "M41 t3: the (n, k, phase) cursor and continue under it; M40's R11 resolved from kctx

R7 names what the resolver searched; R8 turns the scoped-out recursion into a loop; R10 crosses a
fault the way it crosses a trap. <which tests went green; the two oracle armings still red, and at
which answer>

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: `reverse-continue` under the cursor (spec §3d)

**Files:**
- Modify: `crates/retrace/src/debug.rs`, `cmd_reverse_continue` only.

**Interfaces:**
- Consumes: `Phase`, and `pphase` bound at the top of `cmd_reverse_continue` (Task 3).

- [ ] **Step 1: The doc comment**

In `cmd_reverse_continue`'s doc, replace the first sentence (`Run backward to the latest hit —
breakpoint, hardware watch, or syscall watch — strictly before the current position P (M40, spec
§3b).`) with:

```rust
    /// Run backward to the last hit — breakpoint, hardware watch, or syscall watch — before the
    /// cursor P = (n, k, phase) in M41's hit order (spec §3b, §3d; M40 §3b for the one pass).
```

- [ ] **Step 2: A syscall write counts when its (n, 0, Sys) is before P**

Replace phase 1's `Advance::WatchSyscall` arm with:

```rust
                    Advance::WatchSyscall { watched, thread } => {
                        // M41 §3d: a syscall write sits at (n, 0, Sys). It is before P unless P is
                        // that very hit (the cursor at (pn, 0, Sys)) — M40's stuck-loop guard (its
                        // Ruling 2) is that one case of this comparison. An ARRIVAL at (n, 0) has
                        // phase Bp, so the write just behind it counts (t0 M6, M7; spec R5).
                        if (n, 0u64, Phase::Sys) < (pn, pk, pphase) && self.watch_thread_matches(watched, thread) {
                            last = Some((n, RHit::WatchSys { watched }));
                        }
                    }
```

- [ ] **Step 3: A breakpoint under a watch-phase cursor counts**

In phase 2, directly after the `while k < pk { … }` loop's closing brace (still inside
`if !ended {`), add:

```rust
                // M41 §3d: at K = pk itself, a breakpoint comes before a cursor that is ON the
                // store's watch (t0 M4). The pc is read before any step, as the loop above does.
                if pphase == Phase::Watch {
                    let pc = s.pc();
                    if bps.contains(&pc) {
                        bp_ord += 1;
                        last = Some((pn, RHit::Bp { pc, ord: bp_ord }));
                    }
                }
```

- [ ] **Step 4: The defence compares triples**

Replace the `before_p` closure with:

```rust
        let before_p = |n: usize, k: u64, ph: Phase| -> Result<(), String> {
            if (n, k, ph) < (pn, pk, pphase) { Ok(()) } else {
                Err(format!("reverse-continue: resolved ({n}, {k}, {ph:?}) is not before P ({pn}, {pk}, {pphase:?})"))
            }
        };
```

and update its uses:
- in the `RHit::Bp` arm: `before_p(n, k, Phase::Bp)?;`
- in the `RHit::Watch` arm: `before_p(n, k, Phase::Watch)?;`
- in the `RHit::WatchSys` arm, add `before_p(n, 0, Phase::Sys)?;` as its first statement.

- [ ] **Step 5: Run everything the debugger owns**

```bash
L=.superpowers/sdd/2026-09-24-retrace-m41-hitorder
cargo test -p retrace --test hitorder_e2e --no-fail-fast -- --test-threads=1 > $L/t4-hitorder.log 2>&1; echo "hitorder exit=$?"
grep -a -E '^test |test result' $L/t4-hitorder.log
cargo test -p retrace --bins -- --test-threads=1 > $L/t4-bins.log 2>&1; echo "bins exit=$?"
for t in debug_cli watch_cli watch watch_dyn watchsweep_e2e thread_watch_e2e; do
  cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t4-$t.log 2>&1; echo "$t exit=$?"; done
```

Then, as a second call:

```bash
L=.superpowers/sdd/2026-09-24-retrace-m41-hitorder
for t in crashy_cli crashy_e2e reverse_debug_e2e checkpoint_seek sigcatch_dyn_e2e cpython_crash_e2e; do
  cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t4-$t.log 2>&1; echo "$t exit=$?"; done
```

Expected:
- **`hitorder_e2e`:** all 21 pass, and so do all five oracle armings, all three chains each.
- **`--bins`:** 16 pass.
- **Every other suite:** exit 0, with transcripts unchanged. That includes
  `watch_cli`'s `reverse_continue_from_the_syscall_hit_itself_finds_nothing_earlier` (M40's
  Ruling 2, which §3d keeps) and `syscall_writer_is_found_forward_and_backward`.
- **`cpython_crash_e2e`:** passes or skips loud without Homebrew Python. Record which, and its
  runtime.

If any existing assertion moves, stop and report it.

- [ ] **Step 6: Clippy and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/retrace/src/debug.rs
git commit -m "M41 t4: reverse-continue under the cursor (spec §3d)

<the hitorder_e2e result line; the suites run and their results>

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: The gate, the audit, and the close

**Files:**
- Modify: `README.md`, `CLAUDE.md`, `docs/status-log.md`, and the spec's §10.

- [ ] **Step 1: The gate, chunked, capturing every exit code before any pipe**

```bash
L=.superpowers/sdd/2026-09-24-retrace-m41-hitorder
mkdir -p $L/gate
cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1 > $L/gate/ws.log 2>&1; echo $? > $L/gate/ws.exit
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 > $L/gate/box.log 2>&1; echo $? > $L/gate/box.exit
cargo test -p retrace --bins --no-fail-fast -- --test-threads=1 > $L/gate/bins.log 2>&1; echo $? > $L/gate/bins.exit
```

Then the per-target loop, in batches or in the background (each tool call is capped at 10
minutes). **Do not read a kill as a red.**

```bash
L=.superpowers/sdd/2026-09-24-retrace-m41-hitorder
for t in $(ls crates/retrace/tests/*.rs | xargs -n1 basename | sed 's/\.rs$//'); do
  cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/gate/e2e-$t.log 2>&1; echo $? > $L/gate/e2e-$t.exit; done
cargo clippy --workspace --all-targets -- -D warnings > $L/gate/clippy.log 2>&1; echo $? > $L/gate/clippy.exit
cat $L/gate/*.exit | sort | uniq -c
```

Expected: only 0s. Sum the `test result:` lines with `grep -a`.

- [ ] **Step 2: Reconcile against M40, file by file**

M40 closed at **640 / 0 / 9 over 140**. Expected delta: **+24 tests over +1 binary →
664 / 0 / 9 over 141**:

| File | + tests |
|---|---|
| `retrace-core/tests/replay.rs` (`step_armed_…`) | +1 |
| `retrace/src/debug.rs` (two unit tests) | +2 |
| `retrace/tests/hitorder_e2e.rs` (new binary: 10 named, 1 invariant, 5 oracle, 5 Review Focus) | +21 |

Diff the `#[test]` counts per file against `c68ba6d`: `git diff --stat c68ba6d -- crates`, plus
`grep -c '#\[test\]'` on each changed file. Explain any difference from the table by name.

Confirm the invariants:
- `git diff c68ba6d -- crates/retrace-trace` is empty.
- `grep -c 'verify_thread(' crates/retrace-core/src/lib.rs` equals its count at `c68ba6d`
  (`git show c68ba6d:crates/retrace-core/src/lib.rs | grep -c 'verify_thread('`).
- `git diff c68ba6d -- crates | grep -c '#\[ignore'` is 0.

- [ ] **Step 3: The audit (spec §4)**

Write `$L/audit.md`. It lists every test file that asserts on `continue` or `reverse-continue`
output:
- `debug_cli`, `watch_cli`, `watch`, `watch_dyn`
- `watchsweep_e2e`, `thread_watch_e2e`
- `crashy_cli`, `crashy_e2e`, `reverse_debug_e2e`
- `checkpoint_seek`, `cpython_crash_e2e`, `sigcatch_dyn_e2e`
- `debug.rs`'s unit tests

For each file, write "unchanged, passing at <gate log>". Then run
`git diff c68ba6d -- crates/retrace/tests crates/retrace/src/debug.rs` and confirm that no
pre-existing assertion was edited. The spec predicts none moved. If one did, classify it (it pinned
a skip, shown with the oracle; or the cursor changed the answer on purpose) and record it as a
ruling.

- [ ] **Step 4: README (edited in place: it describes the present)**

- **"Known limits":** delete the bullet beginning `**A \`reverse-continue\` to a breakpoint can land
  one hit off across a thread switch.**` and the bullet beginning `**A forward \`continue\` can skip
  a breakpoint hit that its own pre-step lands on.**`.
- **"What works today",** in the debugger's section: add a short **"Hit order"** paragraph. Hits are
  ordered by (n, k, phase), with a syscall's write before a breakpoint before a watched store.
  `continue` gives the first hit after where you stand and `reverse-continue` the last one before
  it. A breakpoint you stepped onto is reported in neither direction, gdb's rule. At a thread
  switch, the position shows the thread that runs next.
- **Also say:** the debugger is checked against a brute-force hit oracle (`hitorder_e2e`, five
  armings, three chains each).
- **The gate line** gets Step 1's measured totals, and the testing table gets a row for
  `hitorder_e2e` in the format of M40's rows.

- [ ] **Step 5: `CLAUDE.md`**

- In the e2e list, after the `watchsweep_e2e` entry, add: `\`hitorder_e2e\` (M41: named regressions
  for every hit the debugger used to skip or invent, the thread-at-a-boundary invariant, and the hit
  oracle — \`tests/util/hits.rs\` single-steps a recording with everything armed and checks
  \`continue\`/\`reverse-continue\` chains against every hardware stop)`.
- Change "the **14 unit tests** inside the \`retrace\` binary itself" to **16**, and "silently costs
  14 tests" to **16**.
- In "Guest threads", after the sentence ending `…no trace-format change — symmetry rule 2 doing its
  job.`, add: `Since M41 a replayed boundary shows the thread that runs next: \`finish_event\` settles
  the switch (\`Box_::settle_schedule\`) instead of leaving it to the next \`run()\`/\`step()\` entry.`

- [ ] **Step 6: Append the M41 section to `docs/status-log.md`**

Append only, never rewriting earlier sections. Title it `## Status: M41-hitorder — every hit
counted once, in one order, on the thread that runs it`. Cover:
- what t0 measured, with a pointer to the companion;
- §3a, the cursor, and the oracle, each with its RED → green evidence from Tasks 1–4;
- the gate, with the reconciliation table;
- the audit's outcome;
- every ruling: the spec's R1–R8, this plan's R9–R10, and execution's R12 onward;
- **"What stays owed":**
  - a handled fault under a breakpoint (unverified: R10 covers crashes only);
  - M40's `crc32`, forward-replay floor and `reverse-stepi` cost;
  - M39's and M38's carried lists;
  - M40's review minors not discharged here;
  - anything the oracle found outside hit accounting (spec §7's halt rule).

  Say plainly that the lldb seam is M42's.

- [ ] **Step 7: Fill the spec's §10 Outcome**

Record the measured figure against every §6 acceptance item, each prediction confirmed or
corrected, and anything found that was not sought.

- [ ] **Step 8: Commit**

```bash
git add README.md CLAUDE.md docs/status-log.md docs/superpowers/specs/2026-09-24-retrace-m41-hitorder-design.md
git commit -m "M41 close: the gate (<measured totals>), the audit, and the hit oracle's five armings green

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```
