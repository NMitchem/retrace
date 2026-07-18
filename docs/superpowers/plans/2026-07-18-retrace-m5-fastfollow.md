# retrace M5 Fast-Follow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the three fast-follow items from M5's final whole-branch review: two mechanical polish fixes and the M-1 semantic gap (pre-step boundary-cross consumes a syscall event with watchpoints unarmed, silently skipping a watched writer).

**Architecture:** No new machinery. T1 is comment/binding polish in retrace-box. T2 arms the watch set across `cmd_continue`'s one-event pre-step boundary-cross and handles `Advance::WatchSyscall` there, mirroring the scan loop's existing arm — proven by a new FILEIO transcript (breakpoint ON the read-svc instruction + watch on `buf`).

**Tech Stack:** Rust 1.95.0 (pinned), existing test harnesses.

## Global Constraints

- Branch: `m5-fastfollow` from `main` (3380c18). Commits: `M5-ff t<N>: <what>`.
- `--test-threads=1` on every cargo test; `cargo clippy --workspace -- -D warnings` clean before each commit; all runs FOREGROUND.
- Existing golden transcripts stay byte-identical: debug_cli.rs, reverse_debug_e2e.rs, checkpoint_seek.rs untouched; watch_cli.rs is append-only.
- Trace format and record path untouched.
- Gate arithmetic: baseline **120/0/0**; T1 leaves it 120; T2 takes it to **121/0/0**.

## Why M-1 is real (context for T2)

A hardware watch hit parks pre-retire at the store, so `last_watch_hit` always has k < window-len and the pre-step never crosses a boundary from it. But a **breakpoint placed on a window-ending syscall instruction** resolves to k = L (parked ON the trap). A `continue` from there takes the pre-step's `Err("ends after")` path: `reseek(n, k)` then ONE `advance()` on a session with **nothing armed** — consuming exactly the boundary syscall event. If that event's recorded writes overlap a watched range (e.g. `read()` filling a watched `buf`), the writer is consumed unreported and the scan continues past it: the debugger answers "who wrote this?" wrongly. No guest instruction retires during this crossing (the guest is parked on the trap), so arming watches for it can only surface `WatchSyscall` — never a hardware `Watch`, never `Break` (breakpoints stay unarmed by pre-step design).

---

### Task 1: Mechanical polish (comment + binding)

**Files:**
- Modify: `crates/retrace-box/src/lib.rs:261` (stale comment) and `:1691-1695` (binding fold)

**Interfaces:** none produced; pure polish, zero behavior change.

- [ ] **Step 1: Fix the stale field comment** — `crates/retrace-box/src/lib.rs:261` currently reads:

```rust
    // declared LAST — after vcpu/vm — so the load-bearing vcpu-before-vm drop order is unaffected.
```

Four M5 fields now follow `cache`, so "LAST" is false. Replace that one line with:

```rust
    // declared after vcpu/vm, so the load-bearing vcpu-before-vm drop order is unaffected.
```

- [ ] **Step 2: Fold the discarded binding** — in `apply_and_return` (`:1691-1695`), replace:

```rust
                if let Some(&(va, len)) = self.watch_ranges.iter()
                    .find(|&&(va, len)| w.ipa < va + len && va < end)
                {
                    let _ = len;
                    self.syscall_watch_hit = Some((va, w.ipa));
                }
```

with:

```rust
                if let Some(&(va, _)) = self.watch_ranges.iter()
                    .find(|&&(va, len)| w.ipa < va + len && va < end)
                {
                    self.syscall_watch_hit = Some((va, w.ipa));
                }
```

- [ ] **Step 3: Verify unchanged behavior**

Run: `cargo test -p retrace --test watch -- --test-threads=1` → 5 passed.
Run: `cargo clippy --workspace -- -D warnings` → clean.

- [ ] **Step 4: Commit**

```bash
git add crates/retrace-box/src/lib.rs
git commit -m "M5-ff t1: polish — un-stale cache-field comment, fold discarded len binding"
```

Gate arithmetic: unchanged, **120/0/0**.

---

### Task 2: M-1 — arm watches across the pre-step boundary-cross

**Files:**
- Modify: `crates/retrace/src/debug.rs` (the `Err(e) if e.contains("ends after")` branch inside `cmd_continue`'s pre-step block, currently at `:338-350`)
- Modify: `crates/retrace/tests/watch_cli.rs` (append one test)
- Modify: `README.md` (append the fast-follow sentence to the M5 Status section)

**Interfaces:**
- Consumes: `discover_read_cli(trace) -> (usize, u64, u64)` already in watch_cli.rs (returns post-read landmark, buf VA, boundary pc = read's return address); the syscall-hit grammar `hit watch {watched:#x} (syscall write) at ({n}, 0)`.
- Produces: none new — the pre-step path now reports syscall writers like the scan loop does.

- [ ] **Step 1: Write the failing test** (append to `crates/retrace/tests/watch_cli.rs`)

```rust
#[test]
fn pre_step_boundary_cross_reports_a_watched_syscall_write() {
    // Final-review M-1: park ON the read-svc via a breakpoint (resolves to k = window len),
    // then `watch buf; continue`. The pre-step crosses the boundary by consuming the read
    // event itself — the kernel write to buf must be reported, not silently skipped.
    let (rec, trace) = util::record(retrace_guest::FILEIO);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let (after_read, buf, bpc) = discover_read_cli(tp);
    let svc_pc = bpc - 4; // ELR (return addr) = svc pc + 4 on arm64 syscalls
    let (code, out, err) = debug_run(ts,
        &format!("break 0x{svc_pc:x}; continue; watch 0x{buf:x}; continue; where"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("hit watch 0x{buf:x} (syscall write) at ({after_read}, 0)")),
        "the crossed boundary event's write must be reported:\n{out}");
    assert!(out.trim_end().ends_with(&format!("at ({after_read}, 0) pc=0x{bpc:x}")),
        "parked at the post-event boundary:\n{out}");
}
```

- [ ] **Step 2: Run to verify RED**

Run: `cargo test -p retrace --test watch_cli pre_step_boundary_cross -- --test-threads=1`
Expected: FAIL — no `hit watch` line; the transcript instead runs to `exited (code 0)` (the writer was consumed unarmed).

- [ ] **Step 3: Implement** — in `cmd_continue`'s pre-step, replace the whole `Err(e) if e.contains("ends after")` branch body with:

```rust
                Err(e) if e.contains("ends after") => {
                    self.reseek(n, k)?; // window end: re-establish (N, K), then advance to (N+1, 0)
                    // Arm the watches for this one-event crossing: the consumed boundary event may
                    // itself be a syscall write to a watched range — without arming, that writer
                    // would be silently skipped (M5 final-review M-1). Breakpoints stay unarmed
                    // (the pre-step must not re-report the parked position); no instruction
                    // retires during the crossing (the guest is parked ON the trap), so only
                    // Event, WatchSyscall, or Exited can come back — never Watch or Break.
                    let ws = self.watches.clone();
                    self.sess_mut().arm_watchpoints(&ws);
                    match self.sess_mut().advance().map_err(|d|
                        format!("continue diverged at landmark {} pc {:#x}: {}", d.landmark, d.pc, d.detail))?
                    {
                        Advance::Exited(report) => {
                            line(out, format_args!("exited (code {})", report.exit_code))?;
                            let e = self.sess().landmark();
                            return self.reseek(e, 0);
                        }
                        Advance::WatchSyscall { watched } => {
                            let n = self.sess().landmark();
                            line(out, format_args!("hit watch {watched:#x} (syscall write) at ({n}, 0)"))?;
                            // Only watches were armed here — clear them; the session is kept.
                            self.sess_mut().clear_watchpoints();
                            self.n = n;
                            self.k = 0;
                            return Ok(());
                        }
                        _ => {
                            // Plain Event: disarm before the main scan re-arms (a second
                            // arm_watchpoints without a clear would duplicate watch_ranges).
                            self.sess_mut().clear_watchpoints();
                            self.n = self.sess().landmark();
                            self.k = 0;
                        }
                    }
                }
```

(The `Exited` arm keeps its existing shape — the reseek drops the armed session. Do not touch the doc comment above the pre-step block except to append one sentence: `A boundary-cross advances with the WATCHES armed, so a syscall write to a watched range in the crossed event is reported, not skipped.`)

- [ ] **Step 4: Run to verify GREEN**

Run: `cargo test -p retrace --test watch_cli -- --test-threads=1` → 7 passed.
Regression: `cargo test -p retrace --test debug_cli --test reverse_debug_e2e --test checkpoint_seek --test watch -- --test-threads=1` → 19 passed.

- [ ] **Step 5: README fast-follow sentence** — append to the very end of the M5 Status section (mirroring the M4 fast-follow precedent):

One sentence stating: the M5 fast-follow closed the final review's M-1 (the `continue` pre-step now crosses a window boundary with watches armed, so a syscall write to a watched range in the crossed event is reported rather than skipped — new test `pre_step_boundary_cross_reports_a_watched_syscall_write`), taking the gate from 120 to **121 passed, 0 failed, 0 ignored**. Fact-check the claim against the code before writing it.

- [ ] **Step 6: Full gate + commit**

Run: `just gate` (foreground) → expected **121 passed / 0 failed / 0 ignored**, clippy clean.

```bash
git add crates/retrace/src/debug.rs crates/retrace/tests/watch_cli.rs README.md
git commit -m "M5-ff t2: pre-step boundary-cross arms watches — crossed-event syscall writes reported (M-1)"
```

---

## After Task 2

Final branch review (Opus, small diff), then superpowers:finishing-a-development-branch. Ledger per task as usual.

## Self-review notes (applied)

- Spec coverage: exactly the ledger's three fast-follow items — (b) T1 step 1, (c) T1 step 2, M-1 T2. Nothing else pulled in (the DROP-triaged items stay dropped).
- The M-1 reachability analysis (bp-on-svc is the only trigger; hardware watch hits can't park at k = L) is encoded in the plan's "Why M-1 is real" section and the test's comment.
- Type consistency: reuses existing helpers/grammar verbatim; no new interfaces.
