# M2-taskinfo Implementation Plan — forward task_info(TASK_AUDIT_TOKEN)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Service the guest's `task_info(TASK_AUDIT_TOKEN)` MIG call (msgh_id 3405) by adding it to `FORWARD_ALLOWLIST` so the existing `Forward` route forwards-and-records it, then walk `hello_dyn` past it.

**Architecture:** At the M2-setport re-park, libsecinit's app-sandbox check calls `task_info(flavor=15=TASK_AUDIT_TOKEN)` on the guest task port (msgh_id 3405), and retrace's router has no handler → fails loud. `task_info` is a read-only DATA query that returns no ports, so — unlike the 3409/3410 port RPCs which must be synthesized — it belongs in `FORWARD_ALLOWLIST` alongside `host_info`/`semaphore_create`. Forwarding it to retrace's own task returns the process's real audit token, captured by `forward_and_diff` and recorded; replay applies the recorded writes (forward-and-record, the `task_self` posture — the token is nondeterministic). The fix is one line; the existing `Forward` dispatch (unchanged) does the rest. Spec: `docs/superpowers/specs/2026-07-15-retrace-m2-taskinfo-design.md` — read it before starting.

**Tech Stack:** Rust workspace, Hypervisor.framework via `hv-sys`, arm64 guests. The pure MIG router lives in `crates/retrace-core/src/machmsg.rs`; the record/replay `Forward` dispatch in `crates/retrace-core/src/lib.rs` (unchanged this milestone).

## Global Constraints

- **Branch:** `m2-taskinfo` (already created from `main`; the spec is committed on it at `77f526a`).
- **Every test run uses `--test-threads=1`** (HVF: one VM per process). Full gate: `just gate`. **Baseline: 77 passed / 0 failed / 1 ignored, clippy clean** (post-M2-setport-merge).
- **Gate count is UNCHANGED at 77/0/1 after Task 1.** The new route coverage is an added *assertion* inside the existing `routes_the_decided_allowlist_to_forward` test (where 200/206/3418 are already asserted) — that is the DRY, house-consistent home for an allowlist forward, so no new test function is added and the passing-test count does not move. (The spec's "78" was an estimate assuming a new test fn; reusing the existing allowlist test is the better engineering choice. This is intentional, not a missing test.)
- **Clippy `-D warnings` clean at every commit.** Codesigning is automatic for `cargo test`/`cargo run`.
- **Never fake a green.** `hello_dyn_e2e` stays `#[ignore]`d unless the Task 2 walk genuinely reaches `main → write → exit` with byte-identical replay.
- **FORWARD, not synthesize.** `task_info` returns data, not ports, and never mutates retrace's address space, so it is safe to forward (the `Forward` route: record `forward_and_diff` + record writes; replay apply recorded writes). Do NOT write a synthesize-and-byte-compare handler and do NOT add a dispatch or decoder arm — the whole change is the allowlist entry.
- **Commit messages:** `M2-taskinfo tN: <what>` + trailing `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` (match the executing model).

### Exact values (verbatim — copy, do not reinvent)

- **The 3405 request** (40 bytes, SIMPLE / non-complex, from the M2-setport walk): `header(24) + NDR(8) + flavor:int(4) + count(4)`. `msgh_id` at offset 20 (`= 3405`); `flavor` at offset 32 (`= 15 = TASK_AUDIT_TOKEN`); `count` at offset 36 (`= 8`). options `0x2_0000_0003` (KOBJECT shape), dest `0x203` (guest task port). (The plan does NOT decode these — they are documented for context; the forward path reads none of them.)
- **`FORWARD_ALLOWLIST`** current value (`crates/retrace-core/src/machmsg.rs`): `&[(200, "host_info"), (206, "host_get_clock_service"), (3418, "semaphore_create")]`. Checked in `route()` BEFORE the `dest == guest_task_port` gate, keyed by msgh_id alone.
- **The existing route test** `routes_the_decided_allowlist_to_forward` (`machmsg.rs`) asserts the three current entries with `route(&msg(id, dest, KOBJ), Some(0x203))`. Helpers `msg(msgh_id, dest, options) -> Msg2` and `KOBJ` (= `0x2_0000_0003`) already exist in that test module.
- **`Forward` dispatch** (unchanged): record `crates/retrace-core/src/lib.rs:308` (`forward_and_diff` + record `Event::Syscall`); replay `lib.rs:498` (`Route::Forward(_) => b.apply_and_return(*ret, *err, writes)`).
- **Gate arithmetic:** Task 1 → 77 / 0 / 1 (unchanged). Task 2: reach `main` and un-ignore → **78 / 0 / 0**; re-park → **77 / 0 / 1**.

---

### Task 1: Add `task_info` (3405) to `FORWARD_ALLOWLIST` (+ route assertion)

**Files:**
- Modify: `crates/retrace-core/src/machmsg.rs` — the `FORWARD_ALLOWLIST` entry; the assertion in `routes_the_decided_allowlist_to_forward`.

**Interfaces:**
- Consumes: the existing `Route::Forward(&'static str)` variant and `FORWARD_ALLOWLIST` (both present).
- Produces: `route()` returns `Route::Forward("task_info")` for msgh_id 3405. Task 2's walk relies on this being live (so the `Forward` dispatch forwards-and-records the audit token).

- [ ] **Step 1: Write the failing assertion.** In `crates/retrace-core/src/machmsg.rs`, in the `routes_the_decided_allowlist_to_forward` test, add a fourth assertion after the `semaphore_create` line:

```rust
        assert!(matches!(route(&msg(3405, 0x203,  KOBJ), Some(0x203)), Route::Forward("task_info")));
```

- [ ] **Step 2: Run the test to verify it fails.**

Run: `cargo test -p retrace-core machmsg::tests::routes_the_decided_allowlist_to_forward -- --test-threads=1`
Expected: FAIL — the new assertion fails because 3405 is not yet in the allowlist, so `route()` falls through to `Route::Unsupported` (3405 isn't a serviced task-port id either), not `Route::Forward("task_info")`.

- [ ] **Step 3: Add the allowlist entry.** In `crates/retrace-core/src/machmsg.rs`, replace:

```rust
const FORWARD_ALLOWLIST: &[(u32, &str)] =
    &[(200, "host_info"), (206, "host_get_clock_service"), (3418, "semaphore_create")];
```

with:

```rust
const FORWARD_ALLOWLIST: &[(u32, &str)] =
    &[(200, "host_info"), (206, "host_get_clock_service"), (3418, "semaphore_create"),
      // task_info (task subsystem base 3400, slot 5): libsecinit's app-sandbox check reads the
      // process's audit token (flavor 15 = TASK_AUDIT_TOKEN). A read-only DATA query with no ports —
      // forwarded to retrace's own task (== the process) and recorded (forward-and-record; the token
      // is nondeterministic). NOT synthesized like the port RPCs 3409/3410 (M2-taskinfo).
      (3405, "task_info")];
```

- [ ] **Step 4: Run the test to verify it passes.**

Run: `cargo test -p retrace-core machmsg::tests::routes_the_decided_allowlist_to_forward -- --test-threads=1`
Expected: PASS (all four allowlist assertions, including `3405 → Forward("task_info")`).

- [ ] **Step 5: Full gate — verify no regression.**

Run: `just gate`
Expected: **77 passed / 0 failed / 1 ignored**, clippy clean. (The count is unchanged — the new coverage is an assertion inside an existing test, per Global Constraints; the `Forward` dispatch and all other MIG paths are untouched.)

- [ ] **Step 6: Commit.**

```bash
git add crates/retrace-core/src/machmsg.rs
git commit -m "M2-taskinfo t1: forward task_info(TASK_AUDIT_TOKEN) via FORWARD_ALLOWLIST

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: Walk `hello_dyn` past 3405; advance or re-park the gate honestly

**Files:**
- Modify: `crates/retrace/tests/hello_dyn_e2e.rs` — un-ignore + double-replay (if the walk reaches `main`) OR re-park the `#[ignore]` reason at the new boundary + append the M2-taskinfo entry to the top-comment history.
- Modify: `README.md` — add a `## Status: M2-taskinfo …` section (mirror the `## Status: M2-setport …` section's shape).
- (The `~/.claude` MEMORY.md + memory-file update is the CONTROLLER's job — the implementer reports the outcome; do NOT edit files under `~/.claude/`.)

**Interfaces:**
- Consumes: the live record/replay path with Task 1's allowlist entry (so 3405 forwards-and-records).
- Produces: the honest next-boundary record (or a green headline gate).

- [ ] **Step 1: Run the bounded traced walk.**

Run: `RETRACE_TRACE=1 cargo test -p retrace --test hello_dyn_e2e -- --ignored --test-threads=1 --nocapture 2>&1 | tee /tmp/taskinfo-walk.log`
Confirm (quote from the log): `mach_msg2 msgh_id=3405` is now forwarded (a `[retrace] forwarding mach_msg2 task_info …` line and the forwarded reply hexdump appear; no `RECORD ERROR: unsupported mach_msg2 … msgh_id 3405`); the trap count advances beyond the M2-setport re-park.

- [ ] **Step 2: Triage the outcome.** Read the tail of the walk log. Exactly one holds:
  - **(A) Reached `main → write → exit`:** the `--ignored` test PASSES (records `write(1,"hi\n")` + exit, replay byte-identical). → do Step 3A.
  - **(B) Re-parked at a NEW wall** (a further libsecinit step — an entitlement/sandbox MIG, a different msgh_id, or a trap/fault): capture the EXACT failure — `RECORD ERROR`/fault text, guest pc, ESR class, msgh_id + args or syscall num + args — and symbolicate the frame against the arm64e shared cache with the runtime slide backed out. → do Step 3B.
  - **(C) A genuinely larger subsystem** (e.g. a real sandbox-profile evaluation, or an XPC/dispatch-mach send expecting a reply): capture it, name it as deferred. → do Step 3B. **Do NOT implement it.**

- [ ] **Step 3A (only if outcome A): un-ignore the headline gate.** In `crates/retrace/tests/hello_dyn_e2e.rs`, delete the `#[ignore = "…"]` attribute on `hello_dyn_records_and_replays` (keep the test body + the historical top comment). Run `cargo test -p retrace --test hello_dyn_e2e -- --test-threads=1` → PASS; run it a SECOND time (double-replay) → PASS. Then README with the "M2 headline gate GREEN" framing, gate arithmetic **78 / 0 / 0**.

- [ ] **Step 3B (only if outcome B or C): re-park honestly.** Rewrite the `#[ignore = "…"]` reason on `hello_dyn_records_and_replays`: (1) record that the 3405 wall FELL in M2-taskinfo (task_info(TASK_AUDIT_TOKEN) forwarded → libsecinit's app-sandbox check proceeds), and (2) name the NEW boundary precisely (the captured pc/ESR/msgh_id+args/symbolicated frame), stating whether it is a further small init step (next milestone) or a genuinely larger deferred subsystem (do NOT pre-stub). Append the M2-taskinfo entry to the top comment's wall-chain history (keep all prior history). Gate arithmetic stays **77 / 0 / 1**.

- [ ] **Step 4: Update the README.** Add a `## Status: M2-taskinfo — task_info(TASK_AUDIT_TOKEN) forwarded ✅` section to `README.md`, mirroring the `## Status: M2-setport …` section (root cause, the fix = forward via FORWARD_ALLOWLIST, the note that this is forward-and-record because the token is nondeterministic — and why forwarding is safe here (read-only data, no ports) vs synthesized for 3409/3410, the walk outcome, a "What runs today" line with the gate count, and a "Deferred" line). Keep it honest and specific.

- [ ] **Step 5: Final gate + commit.**

Run: `just gate`
Expected: green at the honest count (**78/0/0** if 3A, else **77/0/1**), clippy clean.

```bash
git add crates/retrace/tests/hello_dyn_e2e.rs README.md
git commit -m "M2-taskinfo t2: walk past the 3405 wall; <advanced gate to main | re-parked at NEXT>

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

Then report the outcome (A/B/C) and, if B/C, the exact next-wall one-liner (pc + symbol + what it needs) back to the controller for the memory update.

---

## Notes for the executor

- **The whole functional change is one allowlist entry.** Resist adding a decoder, a dispatch arm, or a synthesized reply — `task_info` is forwarded, and the `Forward` route already forwards-and-records (record `forward_and_diff`, replay apply recorded writes). If you find yourself editing `lib.rs` in Task 1, stop: the change is in `machmsg.rs` only.
- **Gate count does not increase in Task 1** — the new assertion lives in the existing allowlist test. That is correct and intentional (Global Constraints); do not add a redundant new test function to bump the number.
- **If Task 2 outcome is B/C**, stop after re-parking + README + commit. Do NOT begin the next subsystem. The milestone's honest deliverable is "the 3405 wall fell; here is the next named boundary."
