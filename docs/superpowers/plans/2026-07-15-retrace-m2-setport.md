# M2-setport Implementation Plan — service task_set_special_port(TASK_DEBUG_CONTROL_PORT)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Service the guest's `task_set_special_port(TASK_DEBUG_CONTROL_PORT)` MIG call (msgh_id 3410) with a `mig_reply_error` KERN_SUCCESS reply, so libsystem_trace's initializer proceeds, then walk `hello_dyn` past it.

**Architecture:** At ~242 traps libsystem_trace's `_libtrace_init` calls `task_set_special_port(which=10=TASK_DEBUG_CONTROL_PORT)` on the guest task port (msgh_id 3410), and retrace's MIG router has no handler → fails loud. `task_set_special_port` has no out-parameters, so its reply is a `mig_reply_error` (id 3510, 44 bytes) — exactly what the existing golden-tested `machmsg::encode_mig_error` produces. Add a dedicated `Route::ServiceSetSpecialPort` that decodes `which_port` (offset 48 in the COMPLEX request), asserts `== 10` (fail-loud, matching the 3409 sibling), and replies via `encode_mig_error`. The reply is DETERMINISTIC, so this uses the STANDARD symmetric posture — replay recomputes and byte-compares (unlike M2-xpcport's minted-port verbatim-apply). Spec: `docs/superpowers/specs/2026-07-15-retrace-m2-setport-design.md` — read it before starting.

**Tech Stack:** Rust workspace, Hypervisor.framework via `hv-sys`, arm64 guests. The pure MIG codec/router lives in `crates/retrace-core/src/machmsg.rs`; the record/replay dispatch in `crates/retrace-core/src/lib.rs`.

## Global Constraints

- **Branch:** `m2-setport` (already created from `main`; the spec is committed on it at `8c2b3d4`).
- **Every test run uses `--test-threads=1`** (HVF: one VM per process). Full gate: `just gate`. **Baseline: 74 passed / 0 failed / 1 ignored, clippy clean** (post-M2-xpcport-merge). Task 1 adds 3 machmsg unit tests → **77 / 0 / 1**.
- **Clippy `-D warnings` clean at every commit.** Codesigning is automatic for `cargo test`/`cargo run`.
- **Never fake a green.** `hello_dyn_e2e` stays `#[ignore]`d unless the Task 2 walk genuinely reaches `main → write → exit` with byte-identical replay.
- **STANDARD symmetric posture (do NOT copy M2-xpcport's asymmetry).** The reply here is deterministic (`encode_mig_error` is a pure function of `m.msgh_id`, `m.reply_port`, `KERN_SUCCESS`), so the replay arm MUST recompute the reply and byte-compare it against the recording — that byte-compare is the divergence oracle. Model the record/replay arms on `ServiceVmMap` / `StubMigReply` (which recompute+compare), NOT on the current `ServiceGetSpecialPort` replay arm (whose verbatim-apply exists only because its minted port name is nondeterministic).
- **Do NOT forward msgh_id 3410** (would set retrace's OWN `TASK_DEBUG_CONTROL_PORT` — wrong target, possibly privileged). **Do NOT** consume/handle the inbound COPY_SEND descriptor (`0x1103`) — ignored. Only `which == 10` is modeled; any other `which` fails loud.
- **Enum-exhaustiveness note:** adding `Route::ServiceSetSpecialPort` to the enum makes BOTH `match machmsg::route(...)` blocks in `lib.rs` (record ~:238, replay ~:419) non-exhaustive → the crate won't compile until both new dispatch arms exist. So the enum variant, the route arm, the decoder, AND both dispatch arms all land together in the GREEN step.
- **Commit messages:** `M2-setport tN: <what>` + trailing `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` (match the executing model).

### Exact values (verbatim — copy, do not reinvent)

- **The 3410 request** (52 bytes, from the M2-xpcport walk; `crates/retrace-core/src/machmsg.rs` offsets): `header(24) + desc_count(4)=1 + port_descriptor(12) + NDR(8) + which_port:int(4)`. `msgh_id` at offset 20 (`= 3410`); `which_port` at **offset 48** (= 24 + 4 + 12 + 8) `= 10`. Options `0x2_0000_0003` (the KOBJECT shape `route()` already gates on). `reply_port = m.reply_port`.
- **The reply:** `encode_mig_error(3410, reply_port, KERN_SUCCESS)` → id `3510`, msgh_size `36`, 44 bytes, RetCode `0`. **No change to `encode_mig_error`.** `machmsg::KERN_SUCCESS: i32 = 0` and `machmsg::MACH_MSG_SUCCESS: u64 = 0` both already exist.
- **`route()` insertion** (`machmsg.rs`, inside `if guest_task_port == Some(m.dest as u64) { match m.msgh_id { … } }`): add the `3410` arm after the `3409 => return Route::ServiceGetSpecialPort,` arm.
- **Dispatch insertion** (`lib.rs`): add the record arm after the `ServiceGetSpecialPort` record arm (before `StubMigReply`, ~:278) and the replay arm after the `ServiceGetSpecialPort` replay arm (before `StubMigReply`, ~:453).
- **Test helpers** already in the machmsg tests module: `msg(msgh_id, dest, options) -> Msg2`, `KOBJ` (= `0x2_0000_0003`), `NDR` (`[0,0,0,0,1,0,0,0]`), `u32_at`.
- **Gate arithmetic:** Task 1 → 77 / 0 / 1. Task 2: reach `main` and un-ignore → **78 / 0 / 0**; re-park → **77 / 0 / 1**.

---

### Task 1: Router variant + decoder + mirrored dispatch (+ unit tests)

**Files:**
- Modify: `crates/retrace-core/src/machmsg.rs` — `Route::ServiceSetSpecialPort`; the `3410` route arm; `decode_set_special_port`; the enum doc comment; unit tests.
- Modify: `crates/retrace-core/src/lib.rs` — the mirrored `ServiceSetSpecialPort` dispatch arms (record + replay).

**Interfaces:**
- Consumes: `machmsg::encode_mig_error(u32, u32, i32) -> Vec<u8>`, `machmsg::KERN_SUCCESS`, `machmsg::MACH_MSG_SUCCESS` (all existing).
- Produces: `Route::ServiceSetSpecialPort`; `machmsg::decode_set_special_port(&[u8]) -> Result<u32, String>`. Task 2's walk relies on both dispatch arms being live.

- [ ] **Step 1: Write the failing machmsg unit tests.** In the `#[cfg(test)] mod tests` of `crates/retrace-core/src/machmsg.rs`, after the existing `task_get_special_port` tests, add:

```rust
    // --- task_set_special_port (3410) — debug-control-port service (M2-setport) ---
    #[test]
    fn routes_set_special_port_to_service() {
        // 3410 to the guest task port is serviced (mig_reply_error KERN_SUCCESS).
        assert!(matches!(route(&msg(3410, 0x203, KOBJ), Some(0x203)),
                         Route::ServiceSetSpecialPort));
        // 3410 to a NON-task port is not ours to service (fail loud).
        assert!(matches!(route(&msg(3410, 0x999, KOBJ), Some(0x203)), Route::Unsupported(_)));
    }

    // Hand-built task_set_special_port request from the M2-xpcport walk's exact 52 bytes:
    // header(24) + desc_count(4)=1 + port_descriptor(12) + NDR(8) + which_port:int(4).
    // msgh_id at offset 20, which_port at offset 48.
    fn set_special_port_req(id: u32, which: u32) -> Vec<u8> {
        let mut b = vec![0u8; 52];
        b[20..24].copy_from_slice(&id.to_le_bytes());          // msgh_id
        b[24..28].copy_from_slice(&1u32.to_le_bytes());        // desc_count = 1
        b[28..32].copy_from_slice(&0x1103u32.to_le_bytes());   // port descriptor: name (offset 28)
        b[40..48].copy_from_slice(&NDR);                        // NDR record (offset 40)
        b[48..52].copy_from_slice(&which.to_le_bytes());        // which_port (offset 48)
        b
    }
    #[test]
    fn decodes_set_special_port_which() {
        // which_port = 10 = TASK_DEBUG_CONTROL_PORT (the only one modeled; decode returns it, dispatch asserts).
        assert_eq!(decode_set_special_port(&set_special_port_req(3410, 10)).unwrap(), 10);
    }
    #[test]
    fn decode_set_special_port_rejects_malformed() {
        assert!(decode_set_special_port(&set_special_port_req(3410, 10)[..51]).is_err()); // short (<52)
        assert!(decode_set_special_port(&set_special_port_req(3411, 10)).is_err());       // wrong id
    }
```

- [ ] **Step 2: Run the tests to verify they fail.**

Run: `cargo test -p retrace-core machmsg -- --test-threads=1`
Expected: FAIL — compile error (`no variant named ServiceSetSpecialPort`, `cannot find function decode_set_special_port`).

- [ ] **Step 3: Implement machmsg — variant, route arm, decoder, doc comment.** In `crates/retrace-core/src/machmsg.rs`:

(a) Add the variant to the `Route` enum and update its doc comment. Replace:

```rust
/// StubMigReply(retcode) answers an optional/no-op kernel routine (no out-params) with a
/// mig_reply_error carrying `retcode`; Forward is the decided read-only/create-once allowlist
/// (memory-diff'd like any mach trap); Unsupported carries a decoded description for the fail-loud error.
pub enum Route { ServiceVmMap, ServiceGetSpecialPort, StubMigReply(i32), Forward(&'static str), Unsupported(String) }
```

with:

```rust
/// ServiceSetSpecialPort answers task_set_special_port(DEBUG_CONTROL_PORT) with a mig_reply_error
/// KERN_SUCCESS (deterministic — standard symmetric posture); StubMigReply(retcode) answers an
/// optional/no-op kernel routine (no out-params) with a mig_reply_error carrying `retcode`; Forward
/// is the decided read-only/create-once allowlist (memory-diff'd like any mach trap); Unsupported
/// carries a decoded description for the fail-loud error.
pub enum Route { ServiceVmMap, ServiceGetSpecialPort, ServiceSetSpecialPort, StubMigReply(i32), Forward(&'static str), Unsupported(String) }
```

(b) Add the route arm. After the `3409 => return Route::ServiceGetSpecialPort,` line, insert:

```rust
            // task_set_special_port (task subsystem base 3400, slot 10): libsystem_trace's
            // initializer sets which=10=TASK_DEBUG_CONTROL_PORT at launch. Serviced with a
            // mig_reply_error KERN_SUCCESS (no out-params) — never forwarded (that would set
            // retrace's OWN debug-control port). `which` is decoded in dispatch and asserted == 10 (M2-setport).
            3410 => return Route::ServiceSetSpecialPort,
```

(c) Add the decoder. After `decode_get_special_port` (ends around the `Ok(u32_at(buf, 32))` line), insert:

```rust
/// task_set_special_port (3410) request body: a COMPLEX message —
/// header(24) + desc_count(4) + port_descriptor(12) + NDR(8) + `which_port: int`(4) = 52 bytes.
/// Returns `which_port` (offset 48 = header 24 + desc_count 4 + descriptor 12 + NDR 8); dispatch
/// asserts it == 10 (TASK_DEBUG_CONTROL_PORT) — the only one modeled. Validates the length and
/// msgh_id so a malformed/mis-routed request fails loud. The inbound COPY_SEND descriptor (offset 28)
/// is ignored — never consumed or forwarded.
pub fn decode_set_special_port(buf: &[u8]) -> Result<u32, String> {
    if buf.len() < 52 { return Err(format!("set_special_port request short: {} < 52", buf.len())); }
    let id = u32_at(buf, 20);
    if id != 3410 { return Err(format!("msgh_id {id} != 3410")); }
    Ok(u32_at(buf, 48)) // which_port = header(24) + desc_count(4) + descriptor(12) + NDR(8)
}
```

- [ ] **Step 4: Add the mirrored dispatch arms in `crates/retrace-core/src/lib.rs`** (required now — the enum match is otherwise non-exhaustive). 

(a) RECORD: after the `machmsg::Route::ServiceGetSpecialPort => { … }` record arm (the one that ends `b.apply_and_return(machmsg::MACH_MSG_SUCCESS, false, &writes);` just before `machmsg::Route::StubMigReply(retcode) =>`), insert:

```rust
                    machmsg::Route::ServiceSetSpecialPort => {
                        // task_set_special_port(3410): libsystem_trace's initializer sets its
                        // TASK_DEBUG_CONTROL_PORT. No out-params → reply a mig_reply_error KERN_SUCCESS
                        // (id 3510) — never forwarded (would set retrace's OWN debug-control port); the
                        // inbound COPY_SEND descriptor is ignored. Only which==10 modeled. The reply is
                        // DETERMINISTIC → STANDARD symmetric posture (replay recomputes + byte-compares).
                        let buf = b.read_guest(m.data, m.send_size as usize);
                        let which = machmsg::decode_set_special_port(&buf)
                            .unwrap_or_else(|e| panic!("task_set_special_port (3410) decode: {e}"));
                        assert_eq!(which, 10,
                            "only TASK_DEBUG_CONTROL_PORT (10) is modeled; got which={which}");
                        let writes = vec![Region { ipa: m.data,
                            bytes: machmsg::encode_mig_error(m.msgh_id, m.reply_port, machmsg::KERN_SUCCESS) }];
                        w.append(&Event::Syscall { num, args, ret: machmsg::MACH_MSG_SUCCESS,
                            err: false, writes: writes.clone() })
                            .map_err(|e| format!("append mach_msg2 set_special_port: {e}"))?; count += 1;
                        b.apply_and_return(machmsg::MACH_MSG_SUCCESS, false, &writes);
                    }
```

(b) REPLAY: after the `machmsg::Route::ServiceGetSpecialPort => { … }` replay arm (the one that ends `b.apply_and_return(*ret, *err, writes);` just before `machmsg::Route::StubMigReply(retcode) =>`), insert:

```rust
                                machmsg::Route::ServiceSetSpecialPort => {
                                    // Deterministic mig_reply_error reply (M2-setport) → STANDARD
                                    // symmetric posture: recompute and byte-compare against the
                                    // recording (the divergence oracle), then apply. (Contrast
                                    // ServiceGetSpecialPort, whose nondeterministic minted name forces
                                    // verbatim-apply — do NOT copy that here.)
                                    let buf = b.read_guest(m.data, m.send_size as usize);
                                    let which = machmsg::decode_set_special_port(&buf).map_err(|e| Divergence {
                                        landmark: idx, pc, detail: format!("replay set_special_port decode: {e}") })?;
                                    assert_eq!(which, 10,
                                        "only TASK_DEBUG_CONTROL_PORT (10) is modeled; got which={which}");
                                    let reply = machmsg::encode_mig_error(m.msgh_id, m.reply_port, machmsg::KERN_SUCCESS);
                                    if writes.len() != 1 || writes[0].bytes != reply {
                                        return Err(Divergence { landmark: idx, pc,
                                            detail: "task_set_special_port reply mismatch".into() });
                                    }
                                    b.apply_and_return(*ret, *err, writes);
                                }
```

- [ ] **Step 5: Run the machmsg tests to verify they pass.**

Run: `cargo test -p retrace-core machmsg -- --test-threads=1`
Expected: PASS (`routes_set_special_port_to_service`, `decodes_set_special_port_which`, `decode_set_special_port_rejects_malformed` all ok; the existing machmsg tests unchanged and green).

- [ ] **Step 6: Full gate — verify no regression.**

Run: `just gate`
Expected: **77 passed / 0 failed / 1 ignored**, clippy clean. (74 baseline + the 3 new machmsg tests; `hello_dyn_e2e` is the 1 ignored.)

- [ ] **Step 7: Commit.**

```bash
git add crates/retrace-core/src/machmsg.rs crates/retrace-core/src/lib.rs
git commit -m "M2-setport t1: service task_set_special_port(DEBUG_CONTROL_PORT) via mig_reply_error

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: Walk `hello_dyn` past 3410; advance or re-park the gate honestly

**Files:**
- Modify: `crates/retrace/tests/hello_dyn_e2e.rs` — un-ignore + double-replay (if the walk reaches `main`) OR re-park the `#[ignore]` reason at the new boundary + append the M2-setport entry to the top-comment history.
- Modify: `README.md` — add a `## Status: M2-setport …` section (mirror the `## Status: M2-xpcport …` section's shape).
- (The `~/.claude` MEMORY.md + memory-file update is the CONTROLLER's job — the implementer reports the outcome details; do NOT edit files under `~/.claude/`.)

**Interfaces:**
- Consumes: the live record/replay path with Task 1's `ServiceSetSpecialPort` arms wired in.
- Produces: the honest next-boundary record (or a green headline gate).

- [ ] **Step 1: Run the bounded traced walk.**

Run: `RETRACE_TRACE=1 cargo test -p retrace --test hello_dyn_e2e -- --ignored --test-threads=1 --nocapture 2>&1 | tee /tmp/setport-walk.log`
Confirm (quote from the log): the `mach_msg2 msgh_id=3410` is serviced (no `RECORD ERROR: unsupported mach_msg2 … msgh_id 3410`); the trap count advances beyond ~242.

- [ ] **Step 2: Triage the outcome.** Read the tail of the walk log. Exactly one holds:
  - **(A) Reached `main → write → exit`:** the `--ignored` test PASSES (records `write(1,"hi\n")` + exit, replay byte-identical). → do Step 3A.
  - **(B) Re-parked at a NEW wall** (a different init MIG id / trap / fault): capture the EXACT failure — `RECORD ERROR`/fault text, guest pc, ESR class, msgh_id + args or syscall num + args, and symbolicate the frame against the arm64e shared cache with the runtime slide backed out. → do Step 3B.
  - **(C) A genuinely larger subsystem** (e.g. a real XPC/dispatch-mach send expecting a reply): capture it, name it as deferred. → do Step 3B. **Do NOT implement it.**

- [ ] **Step 3A (only if outcome A): un-ignore the headline gate.** In `crates/retrace/tests/hello_dyn_e2e.rs`, delete the `#[ignore = "…"]` attribute on `hello_dyn_records_and_replays` (keep the test body + the historical top comment). Run `cargo test -p retrace --test hello_dyn_e2e -- --test-threads=1` → PASS; run it a SECOND time (double-replay) → PASS. Then do Step 4 with the "M2 headline gate GREEN" framing, gate arithmetic **78 / 0 / 0**.

- [ ] **Step 3B (only if outcome B or C): re-park honestly.** Rewrite the `#[ignore = "…"]` reason on `hello_dyn_records_and_replays`: (1) record that the 3410 wall FELL in M2-setport (task_set_special_port serviced → advanced past ~242), and (2) name the NEW boundary precisely (the captured pc/ESR/msgh_id+args/symbolicated frame), stating whether it is a further small init step (next milestone) or a genuinely larger deferred subsystem (do NOT pre-stub). Append the M2-setport entry to the top comment's wall-chain history (keep all prior history). Gate arithmetic stays **77 / 0 / 1**.

- [ ] **Step 4: Update the README.** Add a `## Status: M2-setport — task_set_special_port(DEBUG_CONTROL_PORT) ✅` section to `README.md`, mirroring the `## Status: M2-xpcport …` section (root cause, the fix = mig_reply_error KERN_SUCCESS for 3410, the note that this is the STANDARD symmetric posture, the walk outcome, a "What runs today" line with the gate count, and a "Deferred" line). Keep it honest and specific.

- [ ] **Step 5: Final gate + commit.**

Run: `just gate`
Expected: green at the honest count (**78/0/0** if 3A, else **77/0/1**), clippy clean.

```bash
git add crates/retrace/tests/hello_dyn_e2e.rs README.md
git commit -m "M2-setport t2: walk past the 3410 wall; <advanced gate to main | re-parked at NEXT>

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

Then report the outcome (A/B/C) and, if B/C, the exact next-wall one-liner (pc + symbol + what it needs) back to the controller for the memory update.

---

## Notes for the executor

- **The one thing not to get wrong:** the replay arm here MUST recompute the reply and byte-compare it (STANDARD symmetric posture). Do NOT copy the current `ServiceGetSpecialPort` replay arm's verbatim-apply — that is M2-xpcport's special case for a *nondeterministic minted port name*, which does not apply here (the `mig_reply_error` reply is fully deterministic). Model on `ServiceVmMap` / `StubMigReply`.
- **Enum exhaustiveness:** after adding `Route::ServiceSetSpecialPort`, the crate will not compile until BOTH dispatch arms (record + replay) exist — that is why Steps 3 and 4 land together before any test run in Step 5.
- **If Task 2 outcome is B/C**, stop after re-parking + README + commit. Do NOT begin the next subsystem. The milestone's honest deliverable is "the 3410 wall fell; here is the next named boundary."
