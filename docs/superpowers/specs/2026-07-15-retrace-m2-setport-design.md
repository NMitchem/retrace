# retrace M2-setport — service task_set_special_port(TASK_DEBUG_CONTROL_PORT)

**Design spec — 2026-07-15.** Sub-milestone of M2 (the loader), sibling of
[M2-bootstrap](2026-07-15-retrace-m2-bootstrap-design.md) / [M2-xpcport](2026-07-15-retrace-m2-xpcport-design.md)
(whose MIG router this extends). Clears the wall **M2-xpcport re-parked at**: once libxpc's initializer
completes, libsystem_trace's initializer (`_libtrace_init` → `_os_trace_create_debug_control_port`)
calls `task_set_special_port(TASK_DEBUG_CONTROL_PORT)` (msgh_id **3410**) on the guest task port, and
retrace's router has no handler → fail-loud. The fix: a dedicated `ServiceSetSpecialPort` route that
decodes `which_port`, asserts it is `10` (TASK_DEBUG_CONTROL_PORT), and replies a **`mig_reply_error`
KERN_SUCCESS** — synthetic, never forwarded (forwarding would set retrace's *own* debug-control port).
Scope is one more init-MIG id, same lineage as the serviced 3409 `task_get_special_port`.

## The wall's anatomy (M2-xpcport Task 3 walk, 2026-07-15; archived `.superpowers/sdd/xpcport-walk.log`)

**Observed** at ~242 traps: `RECORD ERROR: unsupported mach_msg2 at pc 0x1804abc34: msgh_id 3410 dest
0x203 (guest task port Some(515)) send_size 52`. The mach_msg2 log line and raw send buffer (verified
byte-for-byte in the M2-xpcport Task-3 review):

```
[mach_msg2] msgh_id=3410 dest=0x203 reply=0x1603 options=0x200000003 bits=0x80001513 send_size=52 rcv_size=44
  send+000: 13 15 00 80 34 00 00 00 03 02 00 00 03 16 00 00
  send+010: 00 00 00 00 52 0d 00 00 01 00 00 00 03 11 00 00
  send+020: 00 00 00 00 00 00 13 00 00 00 00 00 01 00 00 00
  send+030: 0a 00 00 00
```

- `pc 0x1804abc34` = `libsystem_kernel.dylib`\`_mach_msg2_trap+8`; caller
  `libsystem_trace.dylib`\`_os_trace_create_debug_control_port+0x60` ← `_libtrace_init+0xfc` ←
  `libSystem.B.dylib`\`libSystem_initializer+0x10c` (symbolicated live; a sibling init step right
  after `_libxpc_initializer`).
- **msgh_id 3410 = `task_set_special_port`** (Mach task subsystem base 3400, routine 10).
- **options `0x2_0000_0003`** = `MACH64_SEND_MSG | MACH64_RCV_MSG | MACH64_SEND_KOBJECT_CALL` — the
  KOBJECT send+rcv shape `route()` already gates on (identical to the serviced 4811/3409).
- **A COMPLEX request** (`bits 0x80001513`: COMPLEX | remote COPY_SEND | local MAKE_SEND_ONCE), 52
  bytes, laid out `header(24) + desc_count(4)=1 + port_descriptor(12) + NDR(8) + which_port:int(4)`.
  The descriptor names `0x1103` (disposition `0x13` = COPY_SEND, type `0x00` = PORT_DESCRIPTOR).
  **`which_port` sits at offset 48** (= 24 + 4 + 12 + 8) and equals `0x0a = 10 = TASK_DEBUG_CONTROL_PORT`.
- **`rcv_size = 44`** — the guest expects a 44-byte reply (see The reply below).

## The reply to synthesize (msgh_id 3410 → a mig_reply_error)

`task_set_special_port(task, which_port, special_port) -> kern_return_t` has **no out-parameters**, so
its MIG reply `__Reply__task_set_special_port_t` is byte-identical to a `mig_reply_error_t`:
`header(24) + NDR(8) + RetCode(4)` = 36 bytes, `+ 8-byte trailer = 44` (matching `rcv_size`), simple
(non-complex), reply id `3410 + 100 = 3510`, `RetCode = KERN_SUCCESS (0)`. **This is exactly what the
existing `machmsg::encode_mig_error(3410, reply_port, KERN_SUCCESS)` produces** — the same
byte-golden-tested encoder already used for the 4822/8000/8001 stubs. No codec change to
`encode_mig_error` is needed; the milestone only adds a decoder for `which_port` and a route/dispatch
that assert `which == 10` before calling it.

## Verified facts (this repo's MIG stack — read directly)

- **Router** (`crates/retrace-core/src/machmsg.rs`): `route(&Msg2, guest_task_port) -> Route` where
  `Route = { ServiceVmMap, ServiceGetSpecialPort, StubMigReply(i32), Forward(&str), Unsupported(String) }`.
  It gates on the KOBJECT options shape, a forward allowlist, then `dest == guest_task_port` id-cases
  `{4811→ServiceVmMap, 3409→ServiceGetSpecialPort, 4822→StubMigReply(NOT_SUPPORTED),
  8000/8001→StubMigReply(SUCCESS)}`, else `Unsupported`. 3410 currently hits the `_ => {}` → `Unsupported`.
- **`encode_mig_error(request_msgh_id, reply_port, retcode) -> Vec<u8>`** builds the 44-byte simple
  reply: `reply_header(out, 36, reply_port, request_msgh_id + 100)` + NDR + `retcode` + trailer. It is
  golden-tested (`encodes_a_byte_identical_mig_error_reply`, `mig_error_reply_has_the_documented_shape`,
  `restartable_register_stub_replies_success`). `encode_mig_error(3410, port, KERN_SUCCESS)` → id 3510,
  RetCode 0 — the exact bytes this milestone needs.
- **Dispatch pattern to mirror** (`crates/retrace-core/src/lib.rs`, the `MACH_MSG2` arm): the
  `ServiceGetSpecialPort` (3409) arms are the closest template — read the guest buffer at `m.data`,
  decode + assert `which`, build `writes = [Region { ipa: m.data, bytes: encode_…reply(…) }]`, append
  `Event::Syscall { ret: MACH_MSG_SUCCESS, writes }`, `apply_and_return`. **Unlike M2-xpcport**, replay
  here uses the STANDARD symmetric posture (see Determinism).
- **The inbound COPY_SEND descriptor is ignored, safely.** COPY_SEND leaves the sender's right intact
  (no consumption obligation on the receiver); retrace synthesizes the reply rather than actually
  receiving into a real port space; the guest's ref accounting for `0x1103` stays balanced. We never
  read the descriptor and never forward (forwarding 3410 would set retrace's *own*
  `TASK_DEBUG_CONTROL_PORT` — wrong target, possibly privileged).

## The mechanism

### 1. Router + decoder (`machmsg.rs`)
- Add `Route::ServiceSetSpecialPort` (no payload — `which` lives in the request body, decoded in
  dispatch like `ServiceGetSpecialPort`). In `route()`, under `dest == guest_task_port`:
  `3410 => ServiceSetSpecialPort`.
- Add `decode_set_special_port(buf: &[u8]) -> Result<u32 /*which*/, String>`: validate `buf.len() >= 52`
  and the msgh_id at offset 20 `== 3410`, return `which_port = u32_at(buf, 48)` (= header 24 +
  desc_count 4 + descriptor 12 + NDR 8). Parallels `decode_get_special_port` (which reads offset 32 for
  the *simple* 3409 request; the +16 here is the COMPLEX request's `desc_count(4) + descriptor(12)`).

### 2. Dispatch (`crates/retrace-core/src/lib.rs`), mirrored record + replay
New arm modeled on `ServiceGetSpecialPort` but with a deterministic reply (no minted port):
- **Record:** read the buffer, `decode_set_special_port` → `which`; **assert `which == 10`** (fail loud
  on any other special port — only DEBUG_CONTROL_PORT is modeled); `writes = [Region { ipa: m.data,
  bytes: encode_mig_error(m.msgh_id, m.reply_port, KERN_SUCCESS) }]`; append
  `Event::Syscall { ret: MACH_MSG_SUCCESS, writes }`; `apply_and_return`.
- **Replay:** re-decode + assert `which == 10`, **recompute** `encode_mig_error(m.msgh_id, m.reply_port,
  KERN_SUCCESS)`, and **byte-compare** it against the recording (the divergence oracle), then apply.

### 3. Determinism — STANDARD symmetric posture (NOT the M2-xpcport asymmetry)
The reply is a pure function of `(m.msgh_id, m.reply_port, KERN_SUCCESS)` — all deterministic (the
reply_port comes from the deterministic guest; the retcode and id are constants). So replay **recomputes
and byte-compares**, exactly like `ServiceVmMap` and the pre-M2-xpcport `ServiceGetSpecialPort`. This is
the ordinary symmetry rule 1, and it is CORRECT here — do **not** copy M2-xpcport's verbatim-apply /
no-byte-compare posture, which existed only because the minted port name was nondeterministic. Nothing
nondeterministic enters the trace; the byte-compare *is* the divergence check for this handler.

## Scope

**In:** `Route::ServiceSetSpecialPort` + the `3410` route arm; `decode_set_special_port`; the mirrored
record/replay dispatch arm (deterministic reply via the existing `encode_mig_error`); unit tests (route,
decode incl. reject non-10 `which` / short / wrong-id, and a dispatch-reply id/shape check); the walk of
`hello_dyn` past 3410; advance or re-park `hello_dyn_e2e`; README Status + memory at close.

**Out / the honest edge:** any `which != 10` (other task special ports — fail loud, add when a wall
demands); `host_set_special_port`; actually modeling/storing a debug-control port or delivering to it;
the descriptor's port `0x1103` (ignored — COPY_SEND, not forwarded). The deferred single-vCPU commpage
synthesis (from M2-cpuid) and the M2-xpcport replay-asymmetry test follow-up remain deferred.

## Exit criterion

Unit tests green (route, decode incl. rejects, reply shape); the walk advances past msgh_id 3410
(libsystem_trace's initializer proceeds); `just gate` green (74 baseline + new tests, honest ignore
count), clippy clean. Then, honestly, one of: **(A)** the walk reaches `main → write → exit` → un-ignore
`hello_dyn_e2e` + double-replay (the M2 headline gate); **(B)** it re-parks at a new boundary → document
it precisely (the new MIG id / trap / fault, symbolicated) and re-park; **(C)** it reveals a genuinely
larger subsystem → name it and defer. No faked green.

## Testing

1. **Router unit test:** `route(msg(3410, task_port, KOBJ), Some(task_port)) == ServiceSetSpecialPort`;
   3410 to a NON-task port → `Unsupported`; the existing route cases (4811/3409/4822/8000/8001) still hold.
2. **Decode unit test:** `decode_set_special_port` on a hand-built 52-byte COMPLEX request (msgh_id 3410
   at offset 20, desc_count 1, one port descriptor, NDR, `which=10` at offset 48) returns `Ok(10)`;
   rejects a short buffer (`< 52`), a wrong id, and (via the dispatch assert or the decoder — test
   whichever holds it) a `which != 10`. Build the fixture from the walk log's exact 52 bytes.
3. **Reply-shape test:** assert the dispatch's reply equals `encode_mig_error(3410, reply_port,
   KERN_SUCCESS)` — reply id `3510`, msgh_size `36`, 44 bytes total, RetCode `0`. (`encode_mig_error` is
   already golden-tested; this guards the id/retcode wiring.)
4. **Regression:** full `just gate` — the vm_map / get_special_port / stub / forward MIG paths and the
   existing machmsg golden tests stay green.
5. **The walk:** bounded traced `record-dyn hello_dyn`; confirm 3410 is serviced and standard fail-loud
   triage of the next wall.

## Risk register

1. **libsystem_trace does MORE after 3410 succeeds** (another init MIG, or reads the debug-control port
   back). Then the walk re-parks at that op. *Mitigation:* that is the honest boundary; the walk names
   it. Do NOT pre-stub beyond 3410.
2. **`which_port` offset wrong** (miscounting the COMPLEX request's descriptor block). *Mitigation:* the
   decode test builds the fixture from the walk log's exact 52 bytes and asserts `which == 10` at offset
   48; a wrong offset fails the test immediately.
3. **An implementer copies M2-xpcport's verbatim-apply replay posture** (dropping the byte-compare). That
   would *weaken* this handler's oracle for no reason — the reply here is deterministic. *Mitigation:*
   the Determinism section + Task text state the standard symmetric posture explicitly; the reply is
   recomputed and byte-compared.
4. **The inbound descriptor needs handling after all** (e.g. the guest later relies on the kernel having
   taken the port). *Mitigation:* COPY_SEND imposes no consumption; the walk empirically confirms the
   guest proceeds. If a later wall shows otherwise, model it then — not now.

## Components

- `crates/retrace-core/src/machmsg.rs` — `Route::ServiceSetSpecialPort`; the `3410` route arm;
  `decode_set_special_port`; unit tests (route, decode incl. rejects). No change to `encode_mig_error`.
- `crates/retrace-core/src/lib.rs` — the mirrored `ServiceSetSpecialPort` dispatch arm in record and
  replay `MACH_MSG2` handling (deterministic reply, standard byte-compare on replay).
- `crates/retrace/tests/hello_dyn_e2e.rs` — un-ignore + double-replay (if the walk reaches main) or
  re-park at the new boundary; append the M2-setport entry to the top-comment wall-chain history.
- README Status + memory (`retrace-objc-preoptimization-wall` chain) at close.

## Open questions for implementation planning

1. Whether `decode_set_special_port` also validates the complex bit / `desc_count == 1` (defensive) or
   only length + id + which — implementer's call; keep it parallel to `decode_get_special_port`.
2. Whether the walk reaches `main` (un-ignore + double-replay) or a new wall (re-park) — decided
   empirically by the walk.
