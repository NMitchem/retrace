# retrace M2-xpcport — hand back a real kernel-valid bootstrap send right

**Design spec — 2026-07-15.** Sub-milestone of M2 (the loader), direct follow-up to
[M2-bootstrap](2026-07-15-retrace-m2-bootstrap-design.md) — it keeps that milestone's
`ServiceGetSpecialPort` route and complex-reply encoder but changes *which name* the reply carries.
Clears the wall M2-bootstrap re-parked at: libxpc's initializer, having received the synthetic
bootstrap-port name `0x0BAD_0B03`, eagerly calls `xpc_pipe_create_from_port(0x0BAD_0B03, 4)`, whose
one checked port operation fails because that name is not a real send right — so the pipe comes back
NULL and libxpc aborts (`brk #0x1`). The fix: instead of a fixed synthetic constant, **mint a
genuine kernel-valid send right in retrace's own IPC space and hand *its* name back** — recorded once
and replayed verbatim, exactly as `task_self_trap`'s port name already is. Scope is **one op past the
fetch**: make the retain succeed so pipe creation proceeds; do NOT stand up XPC/launchd servicing.

## The wall's anatomy (Task 1 root-cause, 2026-07-15; notes `.superpowers/sdd/` — the Opus walk artifacts `libxpc-disasm.txt`, `xpc-walk.log`; classified SMALL)

**Observed:** at ~228 traps `RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1)
far/ipa=0x0 pc=... elr=0x180201190` — an `EC=0x3c` BRK at guest `pc 0x180201190` =
`libxpc.dylib`\``_xpc_create_bootstrap_pipe.cold.1`, crash string *"Bug in libxpc: Could not create
pipe to bootstrap server!"*, from `_libxpc_initializer+0x42c ← libSystem_initializer+0x100`.

The failing path (symbolicated at slide 0; libxpc `__TEXT` base `0x1801BE000`):

- `_libxpc_initializer` → `__xpc_create_bootstrap_pipe` @`0x1801BFB58` → `bl
  _xpc_pipe_create_from_port` → @`0x1801BFB60` `cbz x0` → `.cold.1` (the BRK) when x0 (the pipe) is 0.
- `_xpc_pipe_create_from_port` tail-calls an internal `__xpc_pipe_create(name = NULL, port =
  0x0BAD_0B03, flags = 4)` @`0x1801D92A8`. **Because `name == NULL` and the flags bit for
  port-derivation is clear, it skips `__xpc_pipe_derive_port` / `__xpc_mach_port_allocate`
  entirely.** Its *only* port-validity dependency is one **checked** call at @`0x1801D92FC`:
  `__xpc_mach_port_retain_send` → `@0x1801D9300 cbnz w0, <error>` → on nonzero kr it logs
  (`__os_assumes_log`), releases, sets the return object to 0 and returns NULL.
- `__xpc_mach_port_retain_send` (@`0x1801BFB04`) is a tail call to `mach_port_mod_refs(mach_task_self(),
  name, MACH_PORT_RIGHT_SEND = 0, +1)`, returning its `kern_return_t` directly (no NULL/DEAD
  special-casing).

**Empirically confirmed** (Task 1 temporarily instrumented the generic mach-trap arm, then reverted —
working tree clean): the three `__xpc_mach_port_retain_send` sites surface as three
`_kernelrpc_mach_port_mod_refs_trap` traps (mach trap **−19**), args `[task = 0x203, name =
0x0BAD_0B03, right = 0 = SEND, delta = +1]`, and **all three return `0xf` = KERN_INVALID_NAME**
(err=false, faithfully recorded). `0x0BAD_0B03` is not a name in retrace's IPC space — and retrace's
IPC space **is** the guest's (see Verified facts) — so the retain fails and the pipe is NULL.

**Classification — SMALL (decisive evidence).** `mach_port_mod_refs(SEND, +1)` is a purely *local*
IPC-namespace refcount operation — it never contacts launchd, sends nothing, and awaits no reply.
Pipe *creation* with `name == NULL` needs only that the port be a valid send right; it does **not**
send-and-wait during construction. So a genuinely valid send right makes all three retains return
KERN_SUCCESS and `__xpc_pipe_create` returns a non-NULL pipe. This is the same shape as the recent
one-value walls (TPIDR_EL0 = 0, TCR TBI), not a new subsystem.

## Verified facts (this repo's stack — read directly)

- **Ports are opaque `u32` names in a single shared namespace.** retrace *is* the process that hosts
  the guest; the guest's Mach traps (`mach_port_mod_refs`, `task_self`, …) are forwarded and executed
  against retrace's own task via `forward_and_diff`. So a port name minted in retrace's IPC space
  (`mach_task_self()`) is valid for the guest's forwarded `mach_port_mod_refs` on that same name.
  This is *why* the fix works and why the synthetic constant did not.
- **The `−19` retain traps are already handled — no change needed.** They fall through the generic
  negative-mach-trap arm (`crates/retrace-core/src/lib.rs:317`) → `forward_and_diff` → recorded ret;
  replay applies the recorded ret. Once the name is real, that same arm records KERN_SUCCESS on record
  and replays it. The fix does not touch this arm.
- **`task_self_trap` is the exact posture to copy.** `MACH_TASK_SELF` (−28) is forwarded, its real
  (nondeterministic, host-assigned) port name recorded as `ret`, and on replay applied from the
  recording (`lib.rs:321` record / `lib.rs:407` replay). A minted bootstrap-port name is
  nondeterministic in precisely the same way and is recorded/replayed the same way.
- **The reply encoder already takes the name as a parameter.**
  `machmsg::encode_get_special_port_reply(reply_port: u32, name: u32) -> Vec<u8>` builds the 48-byte
  complex reply (M2-bootstrap). Passing a real minted name instead of `SYNTHETIC_BOOTSTRAP_PORT`
  needs **no codec change** — the pure encoder is unchanged.
- **The existing `ServiceGetSpecialPort` dispatch arms** live at `crates/retrace-core/src/lib.rs`
  record ~:259-275, replay ~:438-454. Record decodes `which` (asserts == 4), builds the reply with
  the synthetic const, records `writes = [Region { ipa: m.data, bytes: reply }]`, and
  `apply_and_return(MACH_MSG_SUCCESS, false, &writes)`. Replay currently *recomputes* the reply and
  **byte-compares** it against the recording (the divergence oracle). This byte-compare is what must
  change (see The mechanism §3).

## The design posture flip (the crux)

M2-bootstrap's reply was a pure, *deterministic* function of `(reply_port, fixed constant)`, so replay
recomputed it and byte-compared — that comparison **was** the divergence oracle for the handler. A
**real minted port name is nondeterministic** (the kernel picks it; it varies per record run, exactly
like `task_self`'s name). Replay therefore **cannot** recompute it. So this handler moves from the
*synthesize-and-byte-compare* pattern to the *forward-and-record* pattern already used for every real
host port name in the trace:

- **Record** mints the port, builds the reply with the real name, and records the reply bytes.
- **Replay** applies the recorded reply **verbatim** — no recompute, no byte-compare — just like the
  `Forward` route and `task_self`.

Divergence protection for the handler is not lost; it moves downstream. Replay applies the recorded
reply, so the guest reads the **exact recorded name**; its subsequent `mach_port_mod_refs(name, …)`
traps therefore carry args identical to the recording, and the normal syscall `(num, args)` oracle
(`lib.rs` replay) catches any drift. Nothing nondeterministic escapes: the only nondeterministic value
is the name itself, and it is recorded once and replayed — the established `task_self` guarantee.

## The mechanism

### 1. Mint the port (`crates/retrace-box/src/lib.rs`)
Add a `Box_` method that mints once and caches:

```
pub fn mint_bootstrap_port(&mut self) -> u32   // mach_port_name_t
```

- On first call: `mach_port_construct(mach_task_self(), &options, 0, &name)` with
  `options.flags = MPO_INSERT_SEND_RIGHT` (use the SDK's constant — do not hard-code the bit) —
  atomically creates a receive right **and** inserts a send right, returning the name. The name then holds a send right, so the guest's
  forwarded `mach_port_mod_refs(SEND, +1)` succeeds. (Equivalent two-call fallback if `construct` is
  awkward to bind: `mach_port_allocate(RECEIVE)` + `mach_port_insert_right(…, MAKE_SEND)`. A bare
  receive right is **not** enough — `mod_refs(SEND)` on it returns KERN_INVALID_RIGHT.)
- Cache the name in a new `Box_` field `bootstrap_port: Option<u32>` and return the cached name on
  any later call (idempotent — a repeat `which=4` returns the same name, matching real bootstrap-port
  semantics). The field is a plain `Option<u32>` (no `Drop`), declared among the existing plain
  fields (after `backings`, like `reservations`/`mmap_next`), so the load-bearing
  **vcpu-before-vm drop order is unaffected**.
- **Lifetime:** retrace holds the receive right; the port is never deallocated, so the name stays
  valid for the (short-lived) record process. Storing only the `u32` name (not an owning RAII type)
  is deliberate — we *want* it to persist, and a `u32` has no destructor to fight the drop order.
- **Replay never calls this.** `bootstrap_port` stays `None` on the restore path; replay uses the
  recorded reply.

### 2. Record dispatch (`crates/retrace-core/src/lib.rs`, the `ServiceGetSpecialPort` arm)
```
machmsg::Route::ServiceGetSpecialPort => {
    let buf = b.read_guest(m.data, m.send_size as usize);
    let which = machmsg::decode_get_special_port(&buf)?;      // assert which == 4 (fail loud otherwise)
    let name = b.mint_bootstrap_port();                       // NEW: real, cached
    let writes = vec![Region { ipa: m.data,
        bytes: machmsg::encode_get_special_port_reply(m.reply_port, name) }];
    w.append(&Event::Syscall { num, args, ret: MACH_MSG_SUCCESS, err: false, writes: writes.clone() })?;
    b.apply_and_return(machmsg::MACH_MSG_SUCCESS, false, &writes);
}
```
Only the name source changes (`mint_bootstrap_port()` instead of the const); everything else is the
M2-bootstrap arm verbatim.

### 3. Replay dispatch (`crates/retrace-core/src/lib.rs`, the `ServiceGetSpecialPort` arm)
Drop the recompute + byte-compare; apply the recorded reply verbatim (forward-and-record posture).
Keep the `decode + assert which == 4` as a cheap, deterministic sanity guard (the send buffer at
`m.data` is present on replay — the guest re-issued the same message):
```
machmsg::Route::ServiceGetSpecialPort => {
    let buf = b.read_guest(m.data, m.send_size as usize);
    let which = machmsg::decode_get_special_port(&buf).map_err(…)?;
    assert_eq!(which, 4, "only TASK_BOOTSTRAP_PORT (4) is modeled; got which={which}");
    // The reply carries a real, nondeterministic minted name (task_self posture): apply the
    // recorded reply verbatim — do NOT recompute/byte-compare (the name can't be regenerated).
    b.apply_and_return(*ret, *err, writes);
}
```

### 4. Determinism
The minted name is a forwarded kernel result recorded exactly once (the `task_self` guarantee); the
reply is a pure function of `(m.reply_port, recorded name)`. Record and replay never disagree because
replay applies the recorded reply rather than regenerating the name. The three downstream `−19`
retains return KERN_SUCCESS on record and replay that recorded value; their args carry the recorded
name (the guest read it from the applied reply) so the syscall oracle sees matching args. As with
every prior trace, a *fresh* record run mints a different name and produces its own self-consistent
trace — normal per-trace variation (like forwarded getentropy/PID), not a determinism defect.

## Scope

**In:** `Box_::mint_bootstrap_port` (+ the `bootstrap_port` field); the record arm's one-line switch
to the minted name; the replay arm's switch from recompute/byte-compare to verbatim-apply; a box unit
test proving the minted name accepts `mach_port_mod_refs(SEND, +1)` (the fix's premise) and is
idempotent; retire the *runtime* use of `SYNTHETIC_BOOTSTRAP_PORT`; the walk of `hello_dyn` past the
XPC-pipe abort; advance or honestly re-park `hello_dyn_e2e`; README Status + memory at close.

**Out / the honest edge:** any *send* on the bootstrap port that expects a reply (a real
`bootstrap_look_up` / XPC round-trip) — if the walk reveals one, that is the genuine XPC /
dispatch-mach subsystem: **document + defer, do NOT pre-stub launchd/XPC**. Any `which != 4`
(host/other special ports) stays fail-loud. Modeling a real bootstrap namespace, receiving on the
port, or servicing dispatch-mach are all out. The deferred single-vCPU commpage-topology synthesis
(from M2-cpuid) remains deferred.

## Exit criterion

Box unit test green (minted name accepts a SEND mod_refs; idempotent); existing machmsg golden/route
tests unchanged and green; `just gate` green (73 baseline + the new box test, honest ignore count),
clippy `-D warnings` clean. The bounded traced `record-dyn hello_dyn` advances **past**
`xpc_pipe_create_from_port` (no `brk` in `_xpc_create_bootstrap_pipe`). Then, honestly, one of:

1. **The walk reaches `main → write → exit`** → un-ignore `hello_dyn_e2e`, assert byte-identical
   record+replay of `"hi\n"` and exit 0 (the **M2 headline gate finally green**), and double-replay.
2. **The walk re-parks at a new wall** (a further init MIG id / trap, e.g. within `__xpc_early_init`)
   → document it precisely in the `#[ignore]` reason + README + memory, and stop. No faked green.
3. **The walk reveals a real send on the bootstrap port** → that is the deferred XPC subsystem;
   re-park with that exact boundary named. No pre-stubbing.

## Testing

1. **Box mint test** (`crates/retrace-box/tests/`, `--test-threads=1`): `mint_bootstrap_port()`
   returns a nonzero name; `mach_port_mod_refs(mach_task_self(), name, MACH_PORT_RIGHT_SEND, +1)`
   returns KERN_SUCCESS (this is the fix's premise, and the direct RED baseline: the same assertion
   on `0x0BAD_0B03` returns KERN_INVALID_NAME). A second `mint_bootstrap_port()` returns the **same**
   name (idempotence). Clean up with a matching `mod_refs(SEND, −1)` if desired.
2. **machmsg unchanged:** the existing route test (3409 → `ServiceGetSpecialPort`; 3409 to a non-task
   port → `Unsupported`), `decode_get_special_port`, and the byte-golden `encode_get_special_port_reply`
   (which takes a fixed sample name) all stay green — the codec is untouched.
3. **Regression:** full `just gate` — the vm_map / stub / forward MIG paths and the M0/M1 suites are
   unperturbed.
4. **The walk:** bounded traced `record-dyn hello_dyn` with `RETRACE_TRACE=1`; confirm the three
   `−19` retains now return `0x0`, the pipe is non-NULL, and standard fail-loud triage of whatever
   comes next (main, or the honest next boundary).
5. **Double-replay** (only if the walk reaches main): record once, replay twice, byte-identical.

## Risk register

1. **libxpc does a *further* kernel op on the port that a plain valid port doesn't satisfy** — e.g.
   `dispatch_mach` registers it in a port set, or it eagerly `bootstrap_look_up`s and awaits a reply.
   Then the walk re-parks at that op (fail-loud names it). *Mitigation:* that is the honest boundary
   and the deferred XPC subsystem; the real port maximizes how far we get (it satisfies *any local*
   op), and the walk decides empirically. Do NOT expand into XPC here. Task 1 found no send targets
   the port before the abort, so a send (if any) appears only *after* pipe creation succeeds.
2. **`mach_port_construct` binding / flags wrong** (e.g. `MPO_INSERT_SEND_RIGHT` omitted → a
   receive-only name → `mod_refs(SEND)` returns KERN_INVALID_RIGHT). *Mitigation:* the box mint test
   asserts a SEND `mod_refs` succeeds on the minted name — it fails loudly if the send right is
   missing. Two-call `allocate + insert_right(MAKE_SEND)` is the documented fallback.
3. **Replay tries to recompute the reply** (leftover M2-bootstrap byte-compare) → a guaranteed
   divergence, since the name can't be regenerated. *Mitigation:* §3 explicitly removes the
   recompute/byte-compare and applies recorded writes verbatim; the walk's replay leg confirms it.
4. **Port leak across many record runs.** Each record leaks one un-deallocated receive right until
   process exit. *Mitigation:* the record process is short-lived and single-shot; negligible. (If it
   ever matters, deallocate in `Box_::drop` — but that reintroduces a `Drop` field, so keep the plain
   `u32` and accept the process-lifetime leak.)
5. **Idempotence matters if `which=4` is asked twice.** *Mitigation:* caching in `bootstrap_port`
   returns the same name; real bootstrap semantics expect a stable port. (Single `hello_dyn` asks
   once; the cache is cheap insurance.)

## Components

- `crates/retrace-box/src/lib.rs` — `Box_::mint_bootstrap_port` (via `mach_port_construct` in
  retrace's own space); the `bootstrap_port: Option<u32>` field (plain, drop-order-safe); init to
  `None` in `load` / `load_dynamic` / `restore`.
- `crates/retrace-core/src/lib.rs` — the record `ServiceGetSpecialPort` arm switches to the minted
  name; the replay arm switches to verbatim-apply (drop the recompute/byte-compare).
- `crates/retrace-core/src/machmsg.rs` — **no functional change**; `SYNTHETIC_BOOTSTRAP_PORT` loses
  its runtime role (keep only if a golden test wants a fixed sample name, else remove — plan's call).
- `crates/retrace-box/tests/` — the mint/idempotence/SEND-mod_refs unit test.
- `crates/retrace/tests/hello_dyn_e2e.rs` — un-ignore + double-replay (if the walk reaches main) or
  re-park honestly at the new boundary.
- README Status + memory (`retrace-objc-preoptimization-wall` chain) at close.

## Open questions for implementation planning

1. `mach_port_construct` vs `mach_port_allocate` + `mach_port_insert_right` — implementer's call; the
   box mint test (SEND `mod_refs` succeeds) is the acceptance gate either way. Prefer `construct`
   (one atomic call).
2. How to bind the Mach call from Rust (raw `extern "C"` decls for `mach_task_self` /
   `mach_port_construct`, or a `mach2`-style crate) — keep it in `retrace-box` where host `unsafe`
   already lives; no new heavy dependency for one call.
3. Whether to delete `SYNTHETIC_BOOTSTRAP_PORT` outright or retain it as a named fixed sample for the
   encoder golden test.
4. Whether the walk reaches `main` (un-ignore + double-replay) or a new boundary (re-park) — decided
   empirically by the walk, exactly as in every prior milestone.
