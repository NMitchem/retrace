# retrace M2-taskinfo — service task_info(TASK_AUDIT_TOKEN) by forwarding

**Design spec — 2026-07-15.** Sub-milestone of M2 (the loader), sibling of
[M2-setport](2026-07-15-retrace-m2-setport-design.md) (whose MIG router this extends). Clears the wall
**M2-setport re-parked at**: libsecinit's app-sandbox check (`_libsecinit_appsandbox_check` →
`xpc_copy_entitlements_for_self` → `_fetch_self_token`) calls `task_info(flavor=TASK_AUDIT_TOKEN)`
(msgh_id **3405**) on the guest task port, and retrace's router has no handler → fail-loud. The fix:
add `3405` to the `FORWARD_ALLOWLIST` so the existing `Forward` route services it — `task_info` is a
**read-only data query** (no ports), so forwarding it to the host returns retrace's own (== the
process's) audit token, captured by memory-diff and recorded (**forward-and-record**, the `task_self`
posture). One line plus a route test.

## The wall's anatomy (M2-setport Task 2 walk, 2026-07-15)

**Observed** at the re-park: `RECORD ERROR: unsupported mach_msg2 at pc 0x1804abc34: msgh_id 3405 dest
0x203 (guest task port Some(515)) send_size 40`. The mach_msg2 log line + raw send buffer:

```
[mach_msg2] msgh_id=3405 dest=0x203 reply=0x1603 options=0x200000003 bits=0x1513 send_size=40 rcv_size=424
  send+000: 13 15 00 00 28 00 00 00 03 02 00 00 03 16 00 00
  send+010: 00 00 00 00 4d 0d 00 00 00 00 00 00 01 00 00 00
  send+020: 0f 00 00 00 08 00 00 00
```

- `pc 0x1804abc34` = `libsystem_kernel.dylib`\`_mach_msg2_trap+8`; caller chain
  `libsystem_secinit`\`_libsecinit_appsandbox_check` → `libxpc`\`xpc_copy_entitlements_for_self` →
  `_xpc_get_self_audit_token` → `_fetch_self_token` → `task_info` (symbolicated in the M2-setport walk).
  Runs in `_libsecinit_initializer`, the sibling libSystem sub-initializer right after libtrace
  (`libSystem_initializer+0x118`).
- **msgh_id 3405 = `task_info`** (Mach task subsystem base 3400, routine 5).
- **A SIMPLE request** (`bits 0x1513`, no COMPLEX bit, no descriptor), 40 bytes: `header(24) + NDR(8) +
  flavor:int(4) + count:mach_msg_type_number_t(4)`. `flavor` at offset 32 = `0x0f = 15 =
  TASK_AUDIT_TOKEN`; `count` at offset 36 = `8 = TASK_AUDIT_TOKEN_COUNT`.
- **options `0x2_0000_0003`** = the KOBJECT send+rcv shape `route()` already gates on.
- **The reply** is `__Reply__task_info_t`: `header(24) + NDR(8) + RetCode(4) + count(4) +
  task_info_out[8]` (the 8-word `audit_token_t` = 32 bytes) ≈ 72 bytes (the guest's `rcv_size=424`
  buffer is generously oversized). The `audit_token_t` carries pid / asid / pidversion — **nondeterministic**.

## Why FORWARD, not synthesize (the architectural distinction)

The three task-port MIG ids serviced so far split cleanly by *what they return*:

- **Port RPCs → synthesize** (never forward): `task_get_special_port` (3409, M2-xpcport) returns a
  port and `task_set_special_port` (3410, M2-setport) consumes one; forwarding either would hand over
  or set the **host's real ports** (launchd, debug-control). So retrace mints/acknowledges them.
- **Read-only data queries → forward** (the `FORWARD_ALLOWLIST`): `host_info` (200),
  `host_get_clock_service` (206), `semaphore_create` (3418). `task_info` (3405) is the same kind — it
  returns a **data struct with no ports**, and retrace *is* the process the guest runs as, so
  forwarding `task_info` to retrace's task port (name 515, learned from `task_self`) returns the
  correct audit token for the process. It never mutates retrace's address space (unlike `vm_map`, the
  M2-mach wall), so forwarding is safe.

Because the audit token is nondeterministic, this is exactly the **forward-and-record** posture: on
record the `Forward` route calls `forward_and_diff` (issues the real trap, memory-diffs the guest
reply buffer to capture the kernel's writes) and records them; on replay it applies the recorded
writes verbatim (no re-forward, no byte-compare). The token enters the trace as a recorded forwarded
result — identical treatment to `task_self`, `getpid`, `getentropy`, already in the trace. Nothing
new about determinism; this is the established forwarded-result model.

## Verified facts (this repo's MIG stack — read directly)

- **`FORWARD_ALLOWLIST`** (`crates/retrace-core/src/machmsg.rs`): `&[(200, "host_info"), (206,
  "host_get_clock_service"), (3418, "semaphore_create")]`. In `route()` it is checked **before** the
  `dest == guest_task_port` gate: `if let Some((_, name)) = FORWARD_ALLOWLIST.iter().find(|(id,_)| *id
  == m.msgh_id) { return Route::Forward(name); }`. Keyed by msgh_id alone (unambiguous under the
  KOBJECT options shape). Adding `(3405, "task_info")` routes 3405 to `Route::Forward("task_info")`.
- **`Forward` dispatch** (`crates/retrace-core/src/lib.rs`): record arm (~:308) calls
  `b.forward_and_diff(num, args)` and records `Event::Syscall { ret, err, writes }`; replay arm (~:498)
  is `machmsg::Route::Forward(_) => b.apply_and_return(*ret, *err, writes)` — applies recorded writes,
  no re-forward. This is the forward-and-record path, already exercised by 200/206/3418. **No dispatch
  or decoder change is needed.**
- **`forward_and_diff` handles the reply buffer** — it re-issues the mach trap and diffs guest memory
  to capture whatever the kernel wrote into the guest's receive buffer (here the ~72-byte
  `__Reply__task_info_t`). This is how 206 `host_get_clock_service` (also a complex reply) already
  works.

## The mechanism

Add one entry to `FORWARD_ALLOWLIST` in `crates/retrace-core/src/machmsg.rs`:

```rust
const FORWARD_ALLOWLIST: &[(u32, &str)] =
    &[(200, "host_info"), (206, "host_get_clock_service"), (3418, "semaphore_create"),
      (3405, "task_info")];
```

That is the whole functional change. `route()` returns `Route::Forward("task_info")` for 3405; the
existing record/replay `Forward` arms forward-and-record it. A route unit test locks the behavior; the
walk confirms libsecinit accepts the token and proceeds.

## Scope

**In:** the `FORWARD_ALLOWLIST` entry; a route unit test (`3405 → Forward("task_info")`); the walk of
`hello_dyn` past 3405; advance or re-park `hello_dyn_e2e`; README Status + memory at close.

**Out / the honest edge:** any *later* libsecinit step once the token flows (an entitlement query, a
sandbox-extension consume, or a further MIG id — that is the next wall, discovered by the walk, not
pre-stubbed); modeling a synthetic/deterministic audit token (unnecessary — forward-and-record is
correct and simpler). Deferred hygiene (single-vCPU commpage synthesis; the M2-xpcport
replay-asymmetry gate-test) remains deferred.

## Exit criterion

Route unit test green; `just gate` green (77 baseline + the new route test = 78/0/1, honest ignore
count), clippy clean. The walk advances past msgh_id 3405 (libsecinit's app-sandbox check proceeds).
Then, honestly, one of: **(A)** the walk reaches `main → write → exit` → un-ignore `hello_dyn_e2e` +
double-replay (the M2 headline gate); **(B)** it re-parks at a new boundary → document it precisely
(the new MIG id / trap / fault, symbolicated) and re-park; **(C)** it reveals a genuinely larger
subsystem → name it and defer. No faked green.

## Testing

1. **Router unit test:** `route(msg(3405, 0x203, KOBJ), Some(0x203)) == Route::Forward("task_info")`;
   the existing allowlist cases (200/206/3418) and the serviced-id cases (4811/3409/3410/4822/8000/8001)
   still hold. (No decode/encode test — there is no new codec; the reply is forwarded, not built.)
2. **Regression:** full `just gate` — the `Forward` dispatch is unchanged, so 200/206/3418 behavior is
   untouched; all existing machmsg golden/route tests stay green.
3. **The walk:** bounded traced `record-dyn hello_dyn`; confirm 3405 is forwarded (the trace logs the
   forwarded reply), the audit token flows into libsecinit, and standard fail-loud triage of the next
   wall.

## Risk register

1. **libsecinit does MORE after the token flows** (an entitlement/sandbox MIG, or reads a sandbox
   profile). Then the walk re-parks at that op. *Mitigation:* that is the honest boundary; the walk
   names it. Do NOT pre-stub libsecinit/sandbox.
2. **A task_info flavor that returns a port** would leak a host port name via the blanket forward. *In
   practice* no standard `task_info` flavor returns a port (they return data structs), and only
   flavor 15 is observed; forwarding read-only data is safe and recorded. *Mitigation:* if a future
   wall shows a port-bearing flavor, gate the allowlist entry then — YAGNI now.
3. **`forward_and_diff` mis-captures the reply** (wrong length / missed writes). *Mitigation:* the same
   path already forwards the complex `host_get_clock_service` (206) reply correctly; the walk confirms
   libsecinit's `__MIG_check__Reply__task_info` accepts the forwarded bytes.
4. **The forwarded token differs enough between runs to matter.** *Mitigation:* it is recorded once and
   replayed verbatim (forward-and-record) — replay never re-forwards, so the guest sees the identical
   token on both runs. Per-*trace* variation (a fresh record captures a fresh token) is normal, exactly
   like `getpid`/`task_self`.

## Components

- `crates/retrace-core/src/machmsg.rs` — the `FORWARD_ALLOWLIST` entry `(3405, "task_info")`; a route
  unit test. No dispatch, decoder, or encoder change.
- `crates/retrace/tests/hello_dyn_e2e.rs` — un-ignore + double-replay (if the walk reaches main) or
  re-park at the new boundary; append the M2-taskinfo entry to the top-comment wall-chain history.
- README Status + memory (`retrace-objc-preoptimization-wall` chain) at close.

## Open questions for implementation planning

1. Whether the walk reaches `main` (un-ignore + double-replay) or a new wall (re-park) — decided
   empirically by the walk. Given the caller is a sandbox check, a further libsecinit step is the most
   likely next boundary.
