# M2-xpcport Implementation Plan — hand back a real bootstrap send right

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `xpc_pipe_create_from_port` succeed by returning a real kernel-valid send right (not the synthetic constant) as the `task_get_special_port(BOOTSTRAP)` reply's port name, so libxpc's initializer builds its pipe and the guest advances past the `brk` at ~228 traps.

**Architecture:** Task 1 root-caused the XPC-pipe wall as SMALL: libxpc's `__xpc_pipe_create(name=NULL, port=0x0BAD0B03, flags=4)` does one checked `__xpc_mach_port_retain_send` = `mach_port_mod_refs(SEND,+1)`, which returns KERN_INVALID_NAME because `0x0BAD0B03` is not a real name in retrace's (== the guest's) IPC space → NULL → `brk`. The fix mints a genuine send right via `mach_port_construct(MPO_INSERT_SEND_RIGHT)` in retrace's own space and hands its name back. Because a minted name is **nondeterministic** (kernel-assigned), the handler moves from M2-bootstrap's *synthesize-and-byte-compare* posture to the *forward-and-record* posture used for `task_self`: record records the reply bytes; **replay applies them verbatim** (no recompute, no byte-compare). Spec: `docs/superpowers/specs/2026-07-15-retrace-m2-xpcport-design.md` — read it before starting.

**Tech Stack:** Rust workspace, Hypervisor.framework via `hv-sys`, arm64 guests, raw Mach calls via `extern "C"` (libSystem, already linked — no new dependency). Investigation artifacts: `.superpowers/sdd/` Task-1 walk (`libxpc-disasm.txt`, `xpc-walk.log`).

## Global Constraints

- **Branch:** `m2-xpcport` (already created from `main`; the spec is committed on it at `fbb77ab`).
- **Every test run uses `--test-threads=1`** (HVF: one VM per process). Full gate: `just gate` (`cargo test --workspace -- --test-threads=1` then `cargo clippy --workspace --all-targets -- -D warnings`). **Baseline: 73 passed / 0 failed / 1 ignored, clippy clean.**
- **Clippy `-D warnings` clean at every commit.**
- **Codesigning is automatic** for `cargo test`/`cargo run` (the `.cargo/config.toml` runner signs with `retrace.entitlements`). No hand-signing needed for the box test; the `#[ignore]`d e2e uses `util::bin()` which already hand-signs.
- **Never fake a green.** `hello_dyn_e2e` stays `#[ignore]`d **unless** the Task 3 walk genuinely reaches `main → write → exit` with byte-identical replay.
- **Deliberate record/replay ASYMMETRY (exception to CLAUDE.md symmetry rule 1).** The replay `ServiceGetSpecialPort` arm must **NOT** recompute the reply or byte-compare it — the minted name is nondeterministic and cannot be regenerated, so replay applies the recorded reply verbatim (the `task_self` posture). Do not "restore symmetry" by re-adding the byte-compare — that would guarantee a divergence. This asymmetry is the whole point of the milestone.
- **Do NOT forward msgh_id 3409** (would hand the guest the host's real launchd port). **Do NOT pre-stub XPC/`bootstrap_look_up`/dispatch-mach** — out of scope. Only `which == 4` is modeled; any other `which` fails loud.
- **Commit messages:** `M2-xpcport tN: <what>` + trailing `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` (match the executing model).

### Exact values (verbatim — copy, do not reinvent)

- **Mach C ABI** (declare `extern "C"` in `crates/retrace-box/src/lib.rs`; symbols live in libSystem, already linked):
  - `static mach_task_self_: u32;` — `mach_task_self()` is a C macro for this global; reading it needs `unsafe`.
  - `fn mach_port_construct(task: u32, options: *const MachPortOptions, context: u64, name: *mut u32) -> i32;`
  - `MachPortOptions` is `#[repr(C)]` `{ flags: u32, mpl_qlimit: u32, reserved: [u64; 2] }` (24 bytes; from `<mach/port.h>` `mach_port_options_t = { uint32_t flags; mach_port_limits_t mpl; uint64_t reserved[2]; }`, `mach_port_limits_t = { mach_port_msgcount_t mpl_qlimit }`).
  - `MPO_INSERT_SEND_RIGHT: u32 = 0x10` (`<mach/port.h>`).
  - For the box test's assertions: `fn mach_port_mod_refs(task: u32, name: u32, right: u32, delta: i32) -> i32;`, `MACH_PORT_RIGHT_SEND = 0`, `KERN_SUCCESS = 0`, `KERN_INVALID_NAME = 0xf`.
- **`Box_` field block** (`crates/retrace-box/src/lib.rs`): plain fields after `backings` are `reservations: Vec<(u64,u64)>` (:196), `mmap_next: u64` (:199), then pointer/u64 fields, `cache: Option<CacheMeta>` last (:222). The load-bearing rule: `vcpu` (:187) must drop before `vm` (:189); any `u32`/`Option<u32>` field (no `Drop`) added after `backings` is drop-order-safe.
- **Three `Box_ { … }` constructor literals** to extend with `bootstrap_port: None`: `load` (:506), `load_dynamic` (:917), `restore` (:1335). (`restore` fn at :1287.)
- **Dispatch arms** (`crates/retrace-core/src/lib.rs`): record `ServiceGetSpecialPort` at ~:259-275, replay at ~:438-454 (exact current text reproduced in Task 2). `read_guest(&self, ipa, len) -> Vec<u8>` (owned, so its borrow ends before the `&mut b` mint call). retrace-core depends on retrace-box, so `b.mint_bootstrap_port()` is callable.
- **`machmsg::encode_get_special_port_reply(reply_port: u32, name: u32) -> Vec<u8>`** already takes the name as a parameter — **no codec change**. `machmsg::SYNTHETIC_BOOTSTRAP_PORT` (`0x0BAD_0B03`) stays defined (its machmsg golden test still uses it as a fixed sample), but its runtime role is retired.
- **Box test model:** `crates/retrace-box/tests/cpuid.rs` (constructs via `Box_::load_dynamic(&exe, &dyld, "hello_dyn")` from `parse_macho(HELLO_DYN)` + `parse_macho(slice_arm64e(DYLD_PATH))`).
- **Gate arithmetic:** Task 1 adds one passing box test → **74 / 0 / 1**. Task 2 keeps 74 / 0 / 1. Task 3: if the walk reaches `main` and un-ignores the e2e → **75 / 0 / 0**; if it re-parks → **74 / 0 / 1**.

---

### Task 1: `Box_::mint_bootstrap_port` — mint + cache a real send right (+ box test)

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` — add the `bootstrap_port` field; extend the 3 constructors; add the `extern "C"` Mach ABI + `mint_bootstrap_port`.
- Create: `crates/retrace-box/tests/xpcport.rs` — the mint/idempotence/SEND-mod_refs test.

**Interfaces:**
- Consumes: nothing (leaf).
- Produces: `Box_::mint_bootstrap_port(&mut self) -> u32` — mints once via `mach_port_construct(MPO_INSERT_SEND_RIGHT)`, caches the name in `self.bootstrap_port`, returns the cached `u32` name on repeat. Task 2's record dispatch relies on this exact signature.

- [ ] **Step 1: Write the failing box test.** Create `crates/retrace-box/tests/xpcport.rs`:

```rust
// M2-xpcport: mint_bootstrap_port hands back a REAL kernel-valid send right (accepts a SEND
// mach_port_mod_refs +1) and is idempotent. This is the premise of the XPC-pipe fix — libxpc's
// __xpc_mach_port_retain_send = mach_port_mod_refs(SEND,+1) must SUCCEED on the handed-back name; it
// returned KERN_INVALID_NAME on the old synthetic constant 0x0BAD0B03 -> NULL pipe -> brk (Task 1).
use retrace_box::Box_;
use retrace_guest::{parse_macho, slice_arm64e, HELLO_DYN, DYLD_PATH};

extern "C" {
    static mach_task_self_: u32;
    fn mach_port_mod_refs(task: u32, name: u32, right: u32, delta: i32) -> i32;
}
const MACH_PORT_RIGHT_SEND: u32 = 0;
const KERN_SUCCESS: i32 = 0;
const KERN_INVALID_NAME: i32 = 0xf; // 15

#[test]
fn minted_bootstrap_port_accepts_a_send_mod_refs_and_is_idempotent() {
    let exe = parse_macho(&std::fs::read(HELLO_DYN).unwrap());
    let dyld = parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap()));
    let mut b = Box_::load_dynamic(&exe, &dyld, "hello_dyn");

    let name = b.mint_bootstrap_port();
    assert_ne!(name, 0, "minted name must be nonzero");
    assert_eq!(b.mint_bootstrap_port(), name, "mint is idempotent (cached)");

    // The fix's premise: a SEND mod_refs(+1) SUCCEEDS on the minted name (it holds a send right)...
    let kr = unsafe { mach_port_mod_refs(mach_task_self_, name, MACH_PORT_RIGHT_SEND, 1) };
    assert_eq!(kr, KERN_SUCCESS, "mod_refs(SEND,+1) on minted port must succeed; kr={kr:#x}");
    // ...whereas the old synthetic constant is not a real name — exactly why the pipe came back NULL.
    let kr_bad = unsafe { mach_port_mod_refs(mach_task_self_, 0x0BAD_0B03, MACH_PORT_RIGHT_SEND, 1) };
    assert_eq!(kr_bad, KERN_INVALID_NAME, "synthetic 0x0BAD0B03 must be INVALID_NAME; kr={kr_bad:#x}");

    // Balance the +1 we added (hygiene; not load-bearing — the process is single-shot).
    let _ = unsafe { mach_port_mod_refs(mach_task_self_, name, MACH_PORT_RIGHT_SEND, -1) };
}
```

- [ ] **Step 2: Run the test to verify it fails.**

Run: `cargo test -p retrace-box --test xpcport -- --test-threads=1`
Expected: FAIL — compile error `no method named mint_bootstrap_port found for struct Box_`.

- [ ] **Step 3: Add the `bootstrap_port` field to the `Box_` struct.** In `crates/retrace-box/src/lib.rs`, immediately after the `mmap_next: u64,` field (:199) insert:

```rust
    // M2-xpcport: the name of a real kernel-valid send right minted in retrace's OWN IPC space (==
    // the guest's, since Mach traps forward through), handed back as the task_get_special_port(
    // BOOTSTRAP) reply's port name so libxpc's mach_port_mod_refs(SEND,+1) succeeds. Nondeterministic
    // (kernel-assigned) → recorded and replayed like task_self, never regenerated on replay (restore
    // leaves it None). Minted once and cached (idempotent). Plain Option<u32> (no Drop), so the
    // load-bearing vcpu-before-vm drop order is unaffected; retrace holds the receive right for the
    // process lifetime (the name stays valid), so the port is deliberately never deallocated.
    bootstrap_port: Option<u32>,
```

- [ ] **Step 4: Extend the three constructor literals.** In each of the three `Box_ { vm, vcpu, backings, reservations: Vec::new(), mmap_next: MMAP_BASE, … }` returns (`load` :506, `load_dynamic` :917, `restore` :1335), add `bootstrap_port: None,` right after `mmap_next: MMAP_BASE,`. For example the `load_dynamic` return becomes:

```rust
        Box_ { vm, vcpu, backings, reservations: Vec::new(), mmap_next: MMAP_BASE, bootstrap_port: None, l2_host, next_l3, last_far: 0, synthetic_tsc: SYNTH_TSC_START, cache_refault_ipa: 0, cache_refault_count: 0, cache: Some(cache_meta) }
```

Do the identical insertion in the `load` (:506) and `restore` (:1335) returns (they end `… cache: None }`).

- [ ] **Step 5: Add the Mach ABI + `mint_bootstrap_port`.** Add near the top of `crates/retrace-box/src/lib.rs` (with the other `const`/type decls, above `impl Box_`):

```rust
// --- M2-xpcport: mint a real bootstrap send right in retrace's own IPC space ---
// mach_port_options_t (<mach/port.h>): { uint32_t flags; mach_port_limits_t mpl; uint64_t reserved[2] }
// where mach_port_limits_t = { mach_port_msgcount_t mpl_qlimit } (one u32). 24 bytes, repr(C).
#[repr(C)]
struct MachPortOptions { flags: u32, mpl_qlimit: u32, reserved: [u64; 2] }
const MPO_INSERT_SEND_RIGHT: u32 = 0x10; // <mach/port.h>
extern "C" {
    static mach_task_self_: u32; // mach_task_self() is a C macro for this global
    // kern_return_t mach_port_construct(ipc_space_t, mach_port_options_t*, mach_port_context_t, mach_port_name_t*)
    fn mach_port_construct(task: u32, options: *const MachPortOptions, context: u64, name: *mut u32) -> i32;
}
```

Then add this method inside `impl Box_ { … }` (e.g. next to the other public service methods like `guest_mmap`):

```rust
    /// Mint (once, then cache) a real kernel-valid send right in retrace's OWN IPC space — which is
    /// the guest's space (Mach traps forward through), so the guest's forwarded
    /// `mach_port_mod_refs(SEND, +1)` on this name succeeds. Handed back as the synthetic
    /// `task_get_special_port(BOOTSTRAP)` reply's port name (M2-xpcport). The name is nondeterministic
    /// (kernel-assigned), so record records it and replay applies it verbatim — the `task_self`
    /// posture — never regenerated on replay. retrace holds the receive right for the process lifetime
    /// (the name stays valid); the port is deliberately never deallocated.
    pub fn mint_bootstrap_port(&mut self) -> u32 {
        if let Some(name) = self.bootstrap_port { return name; }
        let opts = MachPortOptions { flags: MPO_INSERT_SEND_RIGHT, mpl_qlimit: 0, reserved: [0, 0] };
        let mut name: u32 = 0;
        // SAFETY: a plain Mach call in retrace's own task; MPO_INSERT_SEND_RIGHT yields a name holding
        // a receive right (we keep it) AND a send right (for the guest's forwarded mod_refs).
        let kr = unsafe { mach_port_construct(mach_task_self_, &opts, 0, &mut name) };
        assert_eq!(kr, 0, "mach_port_construct failed: kr={kr:#x}");
        self.bootstrap_port = Some(name);
        name
    }
```

- [ ] **Step 6: Run the box test to verify it passes.**

Run: `cargo test -p retrace-box --test xpcport -- --test-threads=1`
Expected: PASS (`minted_bootstrap_port_accepts_a_send_mod_refs_and_is_idempotent ... ok`).

- [ ] **Step 7: Clippy + commit.**

Run: `cargo clippy -p retrace-box --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/xpcport.rs
git commit -m "M2-xpcport t1: mint a real bootstrap send right (Box_::mint_bootstrap_port)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: Wire the dispatch — record mints, replay applies verbatim

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` — the record `ServiceGetSpecialPort` arm (~:259-275) and the replay arm (~:438-454).
- Modify: `crates/retrace-core/src/machmsg.rs` — retarget the `SYNTHETIC_BOOTSTRAP_PORT` doc comment (its runtime role is retired).

**Interfaces:**
- Consumes: `Box_::mint_bootstrap_port(&mut self) -> u32` (Task 1); the unchanged `machmsg::encode_get_special_port_reply(u32, u32)`.
- Produces: nothing new (behavioral change verified by the Task 3 walk). This task has **no standalone unit test** — the machmsg codec is unchanged, and the record/replay behavior is integration-level; its correctness is proven by (a) no regression in `just gate` and (b) the Task 3 walk. This is intentional, not an omission.

- [ ] **Step 1: Switch the RECORD arm to a minted name.** Replace the current record arm:

```rust
                    machmsg::Route::ServiceGetSpecialPort => {
                        // task_get_special_port(3409): libxpc's initializer fetches TASK_BOOTSTRAP_PORT.
                        // Answer with a fixed synthetic port right (complex reply) — never forwarded
                        // (that would hand the guest the host's real launchd port). Only which==4 modeled.
                        let buf = b.read_guest(m.data, m.send_size as usize);
                        let which = machmsg::decode_get_special_port(&buf)
                            .unwrap_or_else(|e| panic!("task_get_special_port (3409) decode: {e}"));
                        assert_eq!(which, 4,
                            "only TASK_BOOTSTRAP_PORT (4) is modeled; got which={which}");
                        let writes = vec![Region { ipa: m.data,
                            bytes: machmsg::encode_get_special_port_reply(m.reply_port,
                                                                          machmsg::SYNTHETIC_BOOTSTRAP_PORT) }];
                        w.append(&Event::Syscall { num, args, ret: machmsg::MACH_MSG_SUCCESS,
                            err: false, writes: writes.clone() }).map_err(|e| format!("append mach_msg2 get_special_port: {e}"))?; count += 1;
                        b.apply_and_return(machmsg::MACH_MSG_SUCCESS, false, &writes);
                    }
```

with:

```rust
                    machmsg::Route::ServiceGetSpecialPort => {
                        // task_get_special_port(3409): libxpc's initializer fetches TASK_BOOTSTRAP_PORT.
                        // Answer with a REAL kernel-valid send right minted in retrace's OWN IPC space
                        // (M2-xpcport) — never forwarded (that would hand over the host's real launchd
                        // port). The minted name is nondeterministic, so it is RECORDED here and replay
                        // applies it verbatim (the task_self posture). Only which==4 modeled.
                        let buf = b.read_guest(m.data, m.send_size as usize);
                        let which = machmsg::decode_get_special_port(&buf)
                            .unwrap_or_else(|e| panic!("task_get_special_port (3409) decode: {e}"));
                        assert_eq!(which, 4,
                            "only TASK_BOOTSTRAP_PORT (4) is modeled; got which={which}");
                        let name = b.mint_bootstrap_port();
                        let writes = vec![Region { ipa: m.data,
                            bytes: machmsg::encode_get_special_port_reply(m.reply_port, name) }];
                        w.append(&Event::Syscall { num, args, ret: machmsg::MACH_MSG_SUCCESS,
                            err: false, writes: writes.clone() }).map_err(|e| format!("append mach_msg2 get_special_port: {e}"))?; count += 1;
                        b.apply_and_return(machmsg::MACH_MSG_SUCCESS, false, &writes);
                    }
```

- [ ] **Step 2: Switch the REPLAY arm to verbatim-apply (drop the recompute/byte-compare).** Replace the current replay arm:

```rust
                                machmsg::Route::ServiceGetSpecialPort => {
                                    // Mirror of record: re-decode (asserting which==4), rebuild the
                                    // synthetic-port reply, and byte-compare against the recording
                                    // (the divergence oracle), then apply.
                                    let buf = b.read_guest(m.data, m.send_size as usize);
                                    let which = machmsg::decode_get_special_port(&buf).map_err(|e| Divergence {
                                        landmark: idx, pc, detail: format!("replay get_special_port decode: {e}") })?;
                                    assert_eq!(which, 4,
                                        "only TASK_BOOTSTRAP_PORT (4) is modeled; got which={which}");
                                    let reply = machmsg::encode_get_special_port_reply(m.reply_port,
                                                                                       machmsg::SYNTHETIC_BOOTSTRAP_PORT);
                                    if writes.len() != 1 || writes[0].bytes != reply {
                                        return Err(Divergence { landmark: idx, pc,
                                            detail: "task_get_special_port reply mismatch".into() });
                                    }
                                    b.apply_and_return(*ret, *err, writes);
                                }
```

with:

```rust
                                machmsg::Route::ServiceGetSpecialPort => {
                                    // The reply carries a REAL, nondeterministic minted port name
                                    // (M2-xpcport, task_self posture): apply the recorded reply VERBATIM
                                    // — do NOT recompute/byte-compare (the name cannot be regenerated;
                                    // re-adding the byte-compare would guarantee a divergence). The
                                    // decode+assert(which==4) stays as a cheap deterministic guard.
                                    let buf = b.read_guest(m.data, m.send_size as usize);
                                    let which = machmsg::decode_get_special_port(&buf).map_err(|e| Divergence {
                                        landmark: idx, pc, detail: format!("replay get_special_port decode: {e}") })?;
                                    assert_eq!(which, 4,
                                        "only TASK_BOOTSTRAP_PORT (4) is modeled; got which={which}");
                                    b.apply_and_return(*ret, *err, writes);
                                }
```

- [ ] **Step 3: Retarget the `SYNTHETIC_BOOTSTRAP_PORT` doc comment.** In `crates/retrace-core/src/machmsg.rs`, the const's doc comment currently describes it as "The synthetic name handed back for TASK_BOOTSTRAP_PORT." Replace that first sentence with a note that the runtime role is retired (M2-xpcport now mints a real port); the const remains only as the fixed sample name for `encodes_a_byte_identical_get_special_port_reply`. Replace:

```rust
/// The synthetic name handed back for TASK_BOOTSTRAP_PORT. A fixed constant (determinism), chosen
/// high and distinctive so it cannot collide with any port name observed in the run (task 0x203,
```

with:

```rust
/// A fixed sample port name. NOTE (M2-xpcport): its runtime role is RETIRED — the 3409 handler now
/// mints a REAL kernel-valid send right (Box_::mint_bootstrap_port) and hands that name back instead.
/// This const survives only as the fixed sample for `encodes_a_byte_identical_get_special_port_reply`
/// (a byte-golden test of the pure encoder). It is high and distinctive so it cannot collide with any
/// port name observed in the run (task 0x203,
```

(Leave the rest of the comment — the continuation `host 0x1c03/0x1f03, …` onward — and the `pub const SYNTHETIC_BOOTSTRAP_PORT: u32 = 0x0BAD_0B03;` line unchanged.)

- [ ] **Step 4: Full gate — verify no regression.**

Run: `just gate`
Expected: **74 passed / 0 failed / 1 ignored**, clippy clean. (The machmsg route/decode/encode golden tests are unchanged and green; the Task 1 box test is counted.)

- [ ] **Step 5: Commit.**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace-core/src/machmsg.rs
git commit -m "M2-xpcport t2: 3409 hands back the minted real port; replay applies verbatim

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: Walk `hello_dyn` past the XPC-pipe wall; advance or re-park the gate honestly

**Files:**
- Modify: `crates/retrace/tests/hello_dyn_e2e.rs` — un-ignore + double-replay (if the walk reaches `main`) OR re-park the `#[ignore]` reason at the new boundary.
- Modify: `README.md` — add a `## Status: M2-xpcport …` section (mirror the M2-bootstrap section's shape).
- Modify: memory — update `retrace-objc-preoptimization-wall` chain + `MEMORY.md` index at close.

**Interfaces:**
- Consumes: the live record/replay path with Task 1+2 wired in.
- Produces: the honest next-boundary record (or a green headline gate).

- [ ] **Step 1: Run the bounded traced walk.**

Run: `RETRACE_TRACE=1 cargo test -p retrace --test hello_dyn_e2e -- --ignored --test-threads=1 --nocapture 2>&1 | tee /tmp/xpcport-walk.log`
Expected observations (confirm each):
- The three `mach_port_mod_refs` (trap `-19`, name = the minted port) now return `ret=0x0` (KERN_SUCCESS), not `0xf`.
- No `brk` / `EC=0x3c` at guest `pc 0x180201190` (`_xpc_create_bootstrap_pipe.cold.1`) — the pipe is non-NULL and libxpc's initializer proceeds.
- The trap count advances beyond ~228.

- [ ] **Step 2: Triage the outcome and pick the branch.** Read the tail of `/tmp/xpcport-walk.log`. Exactly one holds:
  - **(A) Reached `main → write → exit`:** the run records `write(1, "hi\n")` then `exit`, and replay is byte-identical. → do Step 3A.
  - **(B) Re-parked at a NEW wall** (a different init MIG id / trap / fault, e.g. inside `__xpc_early_init`): capture the exact `RECORD ERROR` / fault (pc, ESR class, msgh_id or syscall num + args, symbolicated frame). → do Step 3B.
  - **(C) A real SEND on the bootstrap port** (a `mach_msg2` targeting the minted name expecting a reply, or dispatch-mach registration): this is the deferred XPC subsystem. Capture the exact send. → do Step 3B (re-park), naming this as the deferred XPC front door — do NOT implement it here.

- [ ] **Step 3A (only if outcome A): un-ignore the headline gate.** In `crates/retrace/tests/hello_dyn_e2e.rs`, delete the `#[ignore = "…"]` attribute on `hello_dyn_records_and_replays` (keep the test body). Then:

Run: `cargo test -p retrace --test hello_dyn_e2e -- --test-threads=1`
Expected: PASS (records + replays `"hi\n"`, exit 0, byte-identical). Then double-replay to be sure:
Run it a second time; expected PASS again. Proceed to Step 4 with the "M2 headline gate GREEN" framing and gate arithmetic **75 / 0 / 0**.

- [ ] **Step 3B (only if outcome B or C): re-park honestly.** Rewrite the `#[ignore = "…"]` reason on `hello_dyn_records_and_replays` to: (1) record that the XPC-pipe wall FELL in M2-xpcport (real minted send right → retains succeed → pipe non-NULL, advancing past ~228 traps), and (2) name the new boundary precisely (the captured pc/ESR/msgh_id+args/frame from Step 2), stating whether it is a further init step (small, next milestone) or the deferred XPC send/dispatch-mach subsystem (large — do not pre-stub). Keep the accumulated wall-chain history in the top comment; append the M2-xpcport entry. Gate arithmetic stays **74 / 0 / 1**.

- [ ] **Step 4: Update README Status + memory.** Add a `## Status: M2-xpcport — real bootstrap send right ✅` section to `README.md` (mirror the M2-bootstrap section: root cause, the fix, the determinism-posture flip, the walk outcome, "What runs today" with the gate count, and "Deferred"). Update the `retrace-objc-preoptimization-wall` memory file's CURRENT WALL entry and the `MEMORY.md` index line to reflect the outcome (gate count; whether the headline gate is green or re-parked at a named boundary; the minted-port fix landed).

- [ ] **Step 5: Final gate + commit.**

Run: `just gate`
Expected: green at the honest count (**75 / 0 / 0** if Step 3A, else **74 / 0 / 1**), clippy clean.

```bash
git add crates/retrace/tests/hello_dyn_e2e.rs README.md
git commit -m "M2-xpcport t3: walk past the XPC-pipe wall; <advanced gate to main | re-parked at NEXT>

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

(Commit the memory files separately if they live outside the repo working tree.)

---

## Notes for the executor

- **The one thing not to "fix":** the replay arm's lack of a byte-compare is deliberate (Global Constraints). If a reviewer or linter nudges you to make record and replay symmetric, re-read the determinism-posture section of the spec — the minted name is nondeterministic and must be replayed from the trace, exactly like `task_self`.
- **If `mach_port_construct` won't link or `MPO_INSERT_SEND_RIGHT` misbehaves** (box test's SEND `mod_refs` returns a nonzero kr), fall back to the two-call form inside `mint_bootstrap_port`: `mach_port_allocate(mach_task_self_, MACH_PORT_RIGHT_RECEIVE=1, &name)` then `mach_port_insert_right(mach_task_self_, name, name, MACH_MSG_TYPE_MAKE_SEND=20)`. A bare receive right is NOT enough. The box test is the acceptance gate either way.
- **If Task 3 outcome is B/C**, stop after re-parking — do not begin the next subsystem. The milestone's honest deliverable is "the XPC-pipe wall fell; here is the next named boundary."
