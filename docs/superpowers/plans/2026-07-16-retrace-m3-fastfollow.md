# M3 Fast-Follow Implementation Plan — executor golden tests + debugger hardening

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the M3 final review's fast-follow list: golden-transcript executor tests for the debug CLI's untested paths (I1), fail-loud >6 breakpoints, the continue-from-a-breakpoint edge (which also fixes back-to-back `continue`), the `pc()`/`cur_pc()` rename, and debug-CLI arg hardening.

**Architecture:** Two tasks. Task 1 adds a golden-transcript integration test file that freezes the CURRENT shipped semantics of the untested executor paths (mid-window HW-breakpoint continue with K-resolution, reverse-continue with a real earlier hit, stepi window-end error, `x` unmapped + len-cap, `delete`) — pure test addition. Task 2 changes behavior TDD-style on that harness: `break` #7 fails loud, `continue` pre-steps when standing on a breakpoint, `debug` arg errors become clean exit-2 usage lines, and the session's dual PC accessors are renamed to match `Box_`'s naming. Source of exact transcript strings: `.superpowers/sdd/task-5-report.md` §2 and the live behavior itself.

**Tech Stack:** Rust workspace; tests spawn the CLI via the `util::bin()` codesign pattern; in-process `ReplaySession` used only for per-recording discovery (drop before any spawn).

## Global Constraints

- **Branch:** create `m3-fastfollow` from `main` (at `de6d84b`): `git checkout -b m3-fastfollow`.
- **`--test-threads=1` everywhere** (one VM per process); in-process discovery sessions are DROPPED before any CLI spawn; never two sessions alive.
- **Gate baseline entering: 90/0/0** (`just gate`), clippy clean. Arithmetic: Task 1 → **93/0/0** (+3 test fns). Task 2 → **97/0/0** (+4 test fns).
- **Zero trace-format changes. Zero record/replay-path behavior changes** — every behavior change in this plan is debugger-executor- or CLI-arg-scoped. `replay()`, `advance()`'s dispatch arms, and `Box_::run()/step()` must not change (the Task 2 rename touches only `ReplaySession` accessor names + call sites).
- **Determinism:** every new printed byte derives from guest state, the script, or fixed strings.
- **Test-time budget:** each test fn records at most ONE fresh trace (`util::record_dynamic`) and reuses it for all its spawns. Do not add per-scenario recordings.
- **Exit codes:** 0 success, 2 usage, 3 divergence, 4 record error, 5 debug script error.
- **Commit messages:** `M3-ff tN: <what>` + `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` (match the executing model).
- **Never fake a green.** If a Task 1 golden test reveals the CURRENT behavior is broken (not merely surprising), that is a finding — report it; do not write the golden to match a bug without flagging it.

### Exact values / semantics (verbatim)

- Transcript contract: `.superpowers/sdd/task-5-report.md` §2 (command echo `> <cmd>`, `hit 0x… at (N, 0)` boundary form, `hit 0x… at (N, +?)` + `resolved (N, K)` mid-window form, `where` = `at (N, K) pc=0x…`, stepi window-end error text with M = instructions REMAINING from current K).
- Discovery pattern (from `crates/retrace/tests/reverse_debug_e2e.rs`): `ReplaySession::open` → loop `peek_syscall()` for the write event `(4, args)` with `args[0]==1` → `advance()` → capture `landmark()` (call it `W`) and the boundary pc. The post-write window `W` is code executed exactly once (write-return → exit path) — safe for unique mid-window breakpoint addresses.
- Mid-window discovery: `seek(trace, W, 0)` → `step_insns(3)` → capture the live-PC accessor (currently `cur_pc()`; after Task 2's rename, `pc()`) → `P_mid`, expected hit coordinate `(W, 3)`.
- HW limit: 6 breakpoint slots (DBGBVR0-5). Current code silently `.take(6)`s.
- Rename map (Task 2, aligning with `Box_::position()`/`Box_::pc()`): `ReplaySession::pc()` (ELR) → `position()`; `ReplaySession::cur_pc()` (live reg PC) → `pc()`. Transcript output strings do NOT change.
- **Continue pre-step rule (Task 2, fixes both the boundary-bp K=0 edge and back-to-back `continue`):** at the START of `continue`, if the executor's current live PC equals an armed breakpoint address, advance the position by exactly one instruction BEFORE arming BVRs: re-seek to `(N, K+1)`; if that seek fails with the window-end error, the next position is `(N+1, 0)` (seek `(N, K)` then one `advance()`; if THAT returns `Exited`, print `exited (code C)` and stop the command). Only then arm breakpoints and scan. Consequences to encode in tests: back-to-back `continue` on a once-executed breakpoint now runs to `exited (code 0)` instead of exit 5; continue after reverse-stepi onto a bp finds the NEXT genuine hit or exits cleanly.

---

### Task 1: Golden-transcript executor tests (freeze shipped semantics)

**Files:**
- Create: `crates/retrace/tests/debug_cli.rs` (uses `mod util;`)

**Interfaces:**
- Consumes: `util::{bin, record_dynamic}`; `retrace_core::{ReplaySession, seek}` for discovery (`peek_syscall`, `advance`, `landmark`, `step_insns`, `window_len_here`, `cur_pc`, `pc`).
- Produces: the harness + helpers Task 2 extends: `fn discover_write(trace: &Path) -> (usize /*W*/, u64 /*boundary pc*/)`, `fn discover_mid(trace: &Path, w: usize) -> u64 /*P_mid at (W,3)*/`, `fn debug_run(trace: &str, script: &str) -> (i32, String /*stdout*/, String /*stderr*/)` (spawns `util::bin()` with `["debug", trace, "--script", script]`).

- [ ] **Step 1: Write the three test fns (they are the failing tests — the file doesn't exist).** Structure (complete the exact expected strings from a first live run + report §2; the asserts must be full-strength — exact `hit`/`resolved`/`where` lines with discovered values interpolated, not substring shrugs):

```rust
// Golden-transcript tests for the debug executor's previously-untested paths.
// Each fn records ONE fresh trace and reuses it for all its CLI spawns.
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

fn discover_write(trace: &Path) -> (usize, u64) {
    let mut s = retrace_core::ReplaySession::open(trace).unwrap();
    loop {
        if let Some((4, args)) = s.peek_syscall() {
            if args[0] == 1 { s.advance().unwrap(); return (s.landmark(), s.pc()); }
        }
        s.advance().unwrap();
    }
}

fn discover_mid(trace: &Path, w: usize) -> u64 {
    let mut s = retrace_core::seek(trace, w, 0).unwrap();
    s.step_insns(3).unwrap();
    s.cur_pc() // Task 2 renames this to pc(); Task 2 updates this call site.
}

#[test]
fn continue_hits_mid_window_and_reverse_continue_returns() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let (w, _bpc) = discover_write(tp);
    let p_mid = discover_mid(tp, w);
    // (both discovery sessions dropped inside the helpers before spawning)

    // Mid-window HW-BVR hit resolves to the exact coordinate (W, 3).
    let (code, out, err) = debug_run(&trace,
        &format!("break 0x{p_mid:x}; continue; where; stepi 2; where; reverse-continue; where"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("hit 0x{p_mid:x} at ({w}, +?)")), "mid-window hit line:\n{out}");
    assert!(out.contains(&format!("resolved ({w}, 3)")), "K-resolution:\n{out}");
    assert!(out.contains(&format!("at ({w}, 3) pc=0x{p_mid:x}")), "where after hit:\n{out}");
    assert!(out.contains(&format!("at ({w}, 5) pc=")), "where after stepi 2:\n{out}");
    // reverse-continue from (W,5): the (W,3) hit is strictly earlier -> returns to it.
    assert!(out.contains(&format!("hit 0x{p_mid:x} at ({w}, 3)")), "reverse-continue hit:\n{out}");
    assert!(out.trim_end().ends_with(&format!("at ({w}, 3) pc=0x{p_mid:x}")), "final where:\n{out}");
}

#[test]
fn stepi_window_end_and_examine_errors_are_clean() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let (w, bpc) = discover_write(tp);
    let len = { let mut s = retrace_core::seek(tp, w, 0).unwrap(); s.window_len_here().unwrap() };

    // Navigate to (W, 0) via the boundary bp, then stepi past the window end:
    // printed error naming the REMAINING count (== len, since K == 0), position unchanged.
    // Also: x on an unmapped VA prints `unmapped`, x on the current pc prints bytes.
    let (code, out, _) = debug_run(&trace, &format!(
        "break 0x{bpc:x}; continue; stepi {}; where; x 0xdead00000000 16; x 0x{bpc:x} 16",
        len + 1));
    assert_eq!(code, 0);
    assert!(out.contains(&format!("hit 0x{bpc:x} at ({w}, 0)")), "navigate to (W,0):\n{out}");
    assert!(out.contains(&format!("{len} instruction(s)")), "error names remaining ({len}):\n{out}");
    assert!(out.contains(&format!("at ({w}, 0) pc=0x{bpc:x}")), "position unchanged after error:\n{out}");
    assert!(out.contains("unmapped"), "x on unmapped VA:\n{out}");
    assert!(out.contains(&format!("0x{bpc:x}:")), "x on mapped pc prints a hex row:\n{out}");

    // x len-cap: parse error -> whole script rejected, exit 5.
    let (code, _, err) = debug_run(&trace, "x 0x1000 65537");
    assert_eq!(code, 5, "len over cap is a parse error");
    assert!(err.contains("DEBUG ERROR"), "stderr: {err}");
}

#[test]
fn delete_disarms_breakpoint() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let (_w, bpc) = discover_write(tp);
    let (code, out, _) = debug_run(&trace,
        &format!("break 0x{bpc:x}; delete 0x{bpc:x}; continue"));
    assert_eq!(code, 0);
    assert!(out.contains(&format!("deleted 0x{bpc:x}")), "delete echo:\n{out}");
    assert!(out.contains("exited (code 0)"), "runs to exit without hitting:\n{out}");
    assert!(!out.contains("hit 0x"), "no hit after delete:\n{out}");
}
```

The `NOTE:` in the second test is an instruction, not a leftover: finalize that scenario against real behavior so the window-end error is exercised from `(W, 0)` with full-strength asserts, then delete the note. Where §2's exact line formats differ from these sketches (e.g. the `(N, +?)` placeholder's actual rendering), use the REAL strings — run each script once, read the transcript, pin it exactly.

- [ ] **Step 2: Run to verify the file fails/compiles as expected.** `cargo test -p retrace --test debug_cli -- --test-threads=1` — first run: compile errors or assert failures with the sketch strings. Iterate: pin each assert to the real transcript lines (full-strength, exact values). If any REAL behavior looks wrong (loop, misresolve, nondeterminism) — STOP on that scenario and report it as a finding instead of goldening a bug.

- [ ] **Step 3: Green + gate.** `cargo test -p retrace --test debug_cli -- --test-threads=1` → PASS (3). `just gate` → **93/0/0**, clippy clean.

- [ ] **Step 4: Commit.**

```bash
git add crates/retrace/tests/debug_cli.rs
git commit -m "M3-ff t1: golden-transcript executor tests — mid-window hit, reverse-continue, errors, delete

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: Behavior fixes — fail-loud break limit, continue pre-step, arg hardening, accessor rename

**Files:**
- Modify: `crates/retrace/src/debug.rs` (break limit; continue pre-step; call-site renames)
- Modify: `crates/retrace/src/main.rs` (debug arm usage errors → exit 2)
- Modify: `crates/retrace-core/src/lib.rs` (`pc()`→`position()`, `cur_pc()`→`pc()` on `ReplaySession` only)
- Modify: `crates/retrace/tests/{seek.rs, reverse_debug_e2e.rs, debug_cli.rs}` (rename call sites; new tests)
- Modify: `README.md` (M3 Deferred list: remove the back-to-back-continue and >6-silent lines; note `break` errors loudly beyond 6)

**Interfaces:**
- Consumes: Task 1's `debug_run`/`discover_write`/`discover_mid` helpers.
- Produces: renamed `ReplaySession::position()` (ELR) / `ReplaySession::pc()` (live reg PC); no other public-surface change.

- [ ] **Step 1: Write the failing tests** (append to `crates/retrace/tests/debug_cli.rs`; the arg test needs no VM):

```rust
#[test]
fn break_beyond_six_fails_loud() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0);
    let script = (0..7).map(|i| format!("break 0x{:x}", 0x10000 + i * 4))
        .collect::<Vec<_>>().join("; ");
    let (code, _, err) = debug_run(&trace, &script);
    assert_eq!(code, 5, "7th break must be a loud error, not silent .take(6)");
    assert!(err.contains("6"), "error names the hardware limit: {err}");
}

#[test]
fn continue_from_a_breakpoint_steps_over_it() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let (w, _) = discover_write(tp);
    let p_mid = discover_mid(tp, w);
    // Back-to-back continue on a once-executed bp: second continue pre-steps off the
    // bp and runs to exit — NOT exit 5 (the old documented limitation).
    let (code, out, err) = debug_run(&trace,
        &format!("break 0x{p_mid:x}; continue; where; continue; where"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("resolved ({w}, 3)")), "first hit:\n{out}");
    assert!(out.contains("exited (code 0)"), "second continue exits cleanly:\n{out}");
}

#[test]
fn continue_after_reverse_stepi_onto_boundary_bp() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let (w, bpc) = discover_write(tp);
    // Boundary bp: hit at (W,0); reverse-stepi off it; continue must pre-step/reland
    // deterministically (the kctx+1 K=0 edge) — no loop, no misresolve.
    let (code, out, err) = debug_run(&trace,
        &format!("break 0x{bpc:x}; continue; where; reverse-stepi; where; continue; where"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("hit 0x{bpc:x} at ({w}, 0)")), "boundary hit:\n{out}");
    // After reverse-stepi to (W-1, L), continue re-approaches: the boundary check fires
    // again at (W, 0) — pin the exact relanding behavior from the real transcript.
    let hits = out.matches(&format!("hit 0x{bpc:x}")).count();
    assert_eq!(hits, 2, "exactly two clean hits, no loop:\n{out}");
}

#[test]
fn debug_arg_errors_are_usage_not_panics() {
    for args in [vec!["debug"], vec!["debug", "/nonexistent-but-unread.bin"],
                 vec!["debug", "/tmp/x.bin", "--script"]] {
        let out = std::process::Command::new(util::bin()).args(&args)
            .output().expect("spawn");
        assert_eq!(out.status.code(), Some(2), "usage exit for {args:?}: {}",
            String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stderr).contains("usage"),
            "usage text for {args:?}");
    }
}
```

- [ ] **Step 2: Verify RED.** `cargo test -p retrace --test debug_cli -- --test-threads=1` — the four new tests fail (7th break currently silent; second continue currently exit 5; arg cases currently exit 101 panics; boundary-continue behavior unpinned).

- [ ] **Step 3: Implement.**
  (a) **Break limit** (`debug.rs`, `cmd_break`): if adding would make `breakpoints.len() > 6`, return `Err("cannot arm more than 6 breakpoints (hardware limit: DBGBVR0-5)".into())`. Remove the `.take(6)` in `arm_breakpoints` and replace with `assert!(bps.len() <= 6, "break command enforces the limit")`; fix its comment.
  (b) **Continue pre-step** (`debug.rs`, top of the continue handler) — the Global Constraints rule, verbatim: if current live PC ∈ breakpoints → re-seek `(N, K+1)`; on window-end error → seek `(N, K)` + one `advance()` (→ `(N+1, 0)`); on `Exited` → print `exited (code C)`, command over. Then arm + scan as today. Delete the now-dead "no later occurrence" hard-error path only if it becomes unreachable — otherwise leave it as the fail-loud backstop.
  (c) **Arg hardening** (`main.rs` debug arm): missing `a[2]`, missing `--script`, or missing script value → `eprintln!("usage: retrace debug <trace> --script '<cmds>'"); exit(2);` (no panics).
  (d) **Rename** (`retrace-core`): `ReplaySession::pc` → `position`, `cur_pc` → `pc`; update every call site (`debug.rs`, `seek.rs`, `reverse_debug_e2e.rs`, `debug_cli.rs` — `discover_write` uses `position()`, `discover_mid` uses `pc()`). Doc-comment both: `position()` = ELR (last trap return address, the landmark anchor); `pc()` = live program counter. Transcript strings unchanged.
  (e) **README**: in the M3 Deferred list, drop the back-to-back-continue and >6-silent items; add one line: "`break` refuses a 7th breakpoint (6 DBGBVR slots, loud error); `continue` from atop a breakpoint pre-steps one instruction."

- [ ] **Step 4: GREEN + full gate.** `cargo test -p retrace --test debug_cli -- --test-threads=1` → PASS (7). `just gate` → **97/0/0**, clippy clean — including `reverse_debug_e2e` (its transcripts must be UNCHANGED by (a)-(d); if the e2e transcript changed, something leaked into printing — investigate, don't re-pin).

- [ ] **Step 5: Commit.**

```bash
git add crates/retrace/src/debug.rs crates/retrace/src/main.rs crates/retrace-core/src/lib.rs crates/retrace/tests README.md
git commit -m "M3-ff t2: fail-loud break limit, continue pre-step, usage errors, position()/pc() rename

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```
