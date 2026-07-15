# M2-bootstrap Implementation Plan — service task_get_special_port(BOOTSTRAP)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Service the guest's `task_get_special_port(TASK_BOOTSTRAP_PORT)` MIG call (msgh_id 3409) by synthesizing a complex reply carrying a fixed synthetic bootstrap-port right, so libxpc's initializer proceeds — then walk `hello_dyn` past it.

**Architecture:** libxpc's image initializer calls `task_get_special_port(which=4)` during process launch; retrace's MIG router (`machmsg::route`) has no handler → fails loud. Add a `ServiceGetSpecialPort` route + a complex-message reply encoder (one `mach_msg_port_descriptor_t` with disposition MOVE_SEND, type PORT_DESCRIPTOR, name = a fixed synthetic const), mirroring the existing `ServiceVmMap` dispatch in both record and replay. Ports are opaque u32s with no namespace, so the guest just stashes the synthetic name; for a write-only guest it stays dormant. Deterministic (pure reply builder, fixed name); the replay oracle byte-compares the recomputed reply. Spec: `docs/superpowers/specs/2026-07-15-retrace-m2-bootstrap-design.md` — read it before starting.

**Tech Stack:** Rust workspace, Hypervisor.framework via `hv-sys`, arm64 guests. Investigation notes: `.superpowers/sdd/bootstrap-research.md` (MIG router + reply shape + scope), `.superpowers/sdd/bootwall-empirical.md` (exact request + init sequence).

## Global Constraints

- Branch: `m2-bootstrap` (create from `main` before Task 1).
- All test runs `--test-threads=1`; full gate `just gate` (baseline 69 passed / 0 failed / 1 ignored); clippy `-D warnings` clean at every commit; codesign + bounded-run rules as in prior milestones.
- Never fake a green; `hello_dyn_e2e` stays `#[ignore]`d unless the walk genuinely reaches `main → write → exit` with byte-identical replay.
- Commit messages `M2-bootstrap tN: <what>` + trailing `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Symmetry: the new dispatch arm must be textually parallel in record and replay `MACH_MSG2` handling; the reply builder is a pure fn used by both (an asymmetry surfaces as a replay divergence).
- Do NOT forward msgh_id 3409 (would hand the guest the host's real launchd port). Do NOT pre-stub XPC/`bootstrap_look_up` (out of scope). Only `which == 4` is modeled; any other `which` fails loud.
- Exact values (verbatim, from the two investigations + direct code read):
  - Request: msgh_id 3409 (`task_get_special_port`, task subsystem base 3400 slot 9), `dest = guest task port` (0x203 observed), `reply_port` observed 0xe03, `send_size = 36` = header(24)+NDR(8)+`which_port:int`(4), `which_port` at buffer offset 32, `= 4` (TASK_BOOTSTRAP_PORT), `rcv_size = 48`.
  - Reply (48 bytes; `msgh_size` field = 40, trailer excluded): header bits `REPLY_BITS(0x1200) | MACH_MSGH_BITS_COMPLEX(0x8000_0000) = 0x8000_1200` (bytes `00 12 00 80`); remote 0; local = `m.reply_port`; voucher 0; id `3409 + 100 = 3509`; desc_count 1; port descriptor = `name(u32) . pad1(0u32) . word 0x0011_0000` (bytes `00 00 11 00` = pad2 0, disposition 0x11 MOVE_SEND, type 0x00 PORT_DESCRIPTOR); trailer `00 00 00 00 08 00 00 00`.
  - `machmsg.rs` existing: `Route` enum, `route()`, `reply_header(out,msgh_size,reply_port,reply_id)` with `REPLY_BITS=0x1200`/`NDR`/`TRAILER` consts, `encode_vm_map_reply`, `encode_mig_error`, `MACH_MSGH_BITS_COMPLEX=0x8000_0000`. Dispatch: `retrace-core/src/lib.rs` `MACH_MSG2` arm, record ~:233-293 (the `ServiceVmMap` block is the template), replay ~:398-438.
  - `SYNTHETIC_BOOTSTRAP_PORT`: a fixed u32 distinct from all observed guest names (task 0x203, host 0x1c03/0x1f03, reply ports ~0xe03/0x1603) and outside the host's forwarded-allocation range — e.g. `0x0BAD_0B03` (confirm non-collision in the walk log; pick another distinctive high value if it collides).
  - Gate baseline: **69 passed / 0 failed / 1 ignored**.

---

### Task 1: Router + reply encoder + mirrored dispatch (+ tests)

**Files:**
- Modify: `crates/retrace-core/src/machmsg.rs` — `Route::ServiceGetSpecialPort`; `route()` arm; `decode_get_special_port`; `encode_get_special_port_reply`; complex-header support; `SYNTHETIC_BOOTSTRAP_PORT`; unit + golden tests.
- Modify: `crates/retrace-core/src/lib.rs` — the mirrored dispatch arm (record + replay).

**Interfaces produced:** `machmsg::decode_get_special_port(&[u8]) -> Result<u32,String>`; `machmsg::encode_get_special_port_reply(u32,u32) -> Vec<u8>`; `Route::ServiceGetSpecialPort`; `machmsg::SYNTHETIC_BOOTSTRAP_PORT`. Task 2 relies on the arm being live in both loops.

- [ ] **Step 1 (RED — router):** in `machmsg.rs` tests, assert `route(&msg(3409, 0x203, KOBJ), Some(0x203))` matches `Route::ServiceGetSpecialPort`, and that 3409 to a non-task port is `Unsupported`. Run `cargo test -p retrace-core machmsg -- --test-threads=1` → expect FAIL (variant/arm absent).

- [ ] **Step 2 (RED — decode + encode):** add tests: `decode_get_special_port` on a hand-built 36-byte request (header with id 3409 at offset 20, NDR, `which=4` at offset 32) returns `Ok(4)`, and rejects a short buffer / wrong id / (if you gate it here) `which != 4`. Byte-golden: `encode_get_special_port_reply(0xe03, SYNTHETIC_BOOTSTRAP_PORT)` equals a hand-verified 48-byte expected array (build it explicitly from the spec's layout). Run → FAIL (fns absent).

- [ ] **Step 3 (GREEN — implement machmsg):** add `Route::ServiceGetSpecialPort`; the `3409 => Route::ServiceGetSpecialPort` arm under `dest == guest_task_port`; `SYNTHETIC_BOOTSTRAP_PORT`; `decode_get_special_port` (validate `buf.len() >= 36`, id at offset 20 == 3409, return `u32_at(buf, 32)`); a complex-capable header (add `complex: bool` to `reply_header` OR a `reply_header_complex` — keep existing non-complex callers byte-identical, verified by the untouched vm_map/mig_error golden tests); `encode_get_special_port_reply` building the 48-byte reply. Run Steps 1–2 tests → GREEN. Capture RED/GREEN.

- [ ] **Step 4 (GREEN — dispatch, mirrored):** in `retrace-core/src/lib.rs`, add the `Route::ServiceGetSpecialPort` arm to BOTH the record (~:233-293) and replay (~:398-438) `MACH_MSG2` matches, textually parallel, modeled on the `ServiceVmMap` block: read `b.read_guest(m.data, m.send_size as usize)`; `let which = decode_get_special_port(&buf).unwrap_or_else(|e| panic!("task_get_special_port (3409) decode: {e}"))`; `assert_eq!(which, 4, "only TASK_BOOTSTRAP_PORT (4) is modeled; got which={which}")`; `let writes = vec![Region { ipa: m.data, bytes: encode_get_special_port_reply(m.reply_port, SYNTHETIC_BOOTSTRAP_PORT) }]`; append `Event::Syscall { num, args, ret: MACH_MSG_SUCCESS, err: false, writes: writes.clone() }`; `b.apply_and_return(MACH_MSG_SUCCESS, false, &writes)`. Replay mirrors the recorded-reply handling exactly as it does for `ServiceVmMap`.

- [ ] **Step 5: regression gate.** `just gate` — 69 prior tests green (esp. all `machmsg` goldens — the complex-header refactor must not perturb vm_map/mig_error bytes) plus the new tests; clippy clean. Commit: `M2-bootstrap t1: service task_get_special_port(BOOTSTRAP) — synthetic-port complex MIG reply (mirrored)`.

---

### Task 2: Walk past 3409; advance or re-park the gate

- [ ] **Step 1:** bounded traced `record-dyn hello_dyn`. Confirm the run advances past msgh_id 3409 (libxpc's initializer accepts the reply). Record trap count + the new furthest point + symbolicated pc. **Check for the risk-1/risk-2 signals:** does the guest immediately SEND to `SYNTHETIC_BOOTSTRAP_PORT` (a mach_msg2 with `dest == SYNTHETIC_BOOTSTRAP_PORT`)? does the synthetic name collide with any forwarded-assigned name? Report both.
- [ ] **Step 2a (reached `main → write → exit`):** un-ignore `hello_dyn_e2e`; verify record prints `hi\n`, replay reproduces stdout byte-for-byte + per-syscall + final memory checks pass; add the double-replay determinism test; run it repeatedly to confirm non-flaky. **The M2 headline gate goes green.**
- [ ] **Step 2b (new distinct wall):** keep `hello_dyn_e2e` `#[ignore]`d; rewrite its reason + block comment with the verified anatomy (symbol, mechanism, why distinct); a small mirrored below-the-trace fix belonging to *this* milestone may be applied and re-walked, but a distinct subsystem (e.g. a real `bootstrap_look_up`/XPC send) is documented and DEFERRED, not walked into.
- [ ] **Step 3:** add a `## Status: M2-bootstrap …` README section (root cause, the synthetic-port fix, the honest walk outcome). `just gate` green; clippy clean. Commit `M2-bootstrap t2: walk past task_get_special_port — <reached main | re-parked at NEWWALL>`.

---

## Integration & close-out

- Gate green with honest ignore count; memory updated (the wall-chain memory gets the outcome; if `main` was reached, say so loudly — it closes the M2 headline).
- Merge `m2-bootstrap` → `main` (`Merge M2-bootstrap (task_get_special_port BOOTSTRAP servicing) into main`).

## Notes for the implementer

- Keep the existing non-complex reply callers (`encode_vm_map_reply`, `encode_mig_error`) byte-identical — the machmsg golden tests are the guard; if they go red, your complex-header refactor changed the shared path.
- The synthetic name is load-bearing for determinism AND non-collision — a fixed const, distinct from every observed name. Verify against the walk log in Task 2.
- Do not forward 3409, do not pre-stub XPC, do not model `which != 4` — each is explicitly out of scope.
