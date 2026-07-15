# retrace M2-bootstrap — service task_get_special_port(BOOTSTRAP)

**Design spec — 2026-07-15.** Sub-milestone of M2 (the loader), sibling of M2-mach (whose MIG
router and reply-encoders this directly extends) and the rest of the M2 chain. Clears the wall
M2-cpuid re-parked at: libxpc's initializer calls `task_get_special_port(TASK_BOOTSTRAP_PORT)`
(msgh_id 3409) and retrace's router has no handler, so it fails loud. The fix synthesizes the MIG
reply — a complex message carrying a fixed synthetic bootstrap-port right — and proceeds. Scope is
**fetch-and-cache**: a trivial write-only guest stashes the port and never sends to it, so this is a
small, bounded handler, not the front door to XPC/launchd.

## The wall's anatomy (two investigations, 2026-07-15; notes `.superpowers/sdd/bootstrap-research.md`, `bootwall-empirical.md`)

**Observed:** `RECORD ERROR: unsupported mach_msg2 at pc 0x1804abc34: msgh_id 3409 dest 0x203
(guest task port Some(515)) send_size 36`.

- `pc 0x1804abc34` = `libsystem_kernel.dylib`\`_mach_msg2_trap+8`; call chain
  `libxpc.dylib`\`__libxpc_initializer+1040 → _task_get_special_port → _mach_msg2_trap`. So
  **libxpc's image initializer** (run during `libSystem_initializer` at process launch) issues it,
  as its first action.
- **msgh_id 3409 = `task_get_special_port`** (Mach task subsystem base 3400, slot 9). Request: `dest
  = 0x203` (guest task port, name 515), `reply_port = 0xe03` (a fresh one-shot reply port),
  `send_size = 36` = header(24)+NDR(8)+`which_port:int`(4), **`which_port = 4 = TASK_BOOTSTRAP_PORT`**,
  `rcv_size = 48`.
- **The guest expects a complex reply carrying a port right.** Success reply = header(24) +
  descriptor_count(4) + one `mach_msg_port_descriptor_t`(12) + trailer(8) = 48 bytes, with the
  header's COMPLEX bit set. The guest's `__MIG_check__Reply__task_get_special_port` validates the
  descriptor's disposition and type — those bytes are load-bearing.

**Scope (the decisive finding):** **fetch-and-cache, not the front door to XPC.** libxpc's
initializer fetches the bootstrap port and stashes it in the `bootstrap_port` global; for a
write-only `hello_dyn` that uses no XPC services, it is never sent to before `main → write → exit`.
Every mach_msg2 before the wall is pure infrastructure (clock, semaphore, malloc, restartable
ranges) with zero look-ups; only ONE special-port-shaped call exists in the whole run. **Honest
hedge:** if libxpc/libtrace eagerly does one `bootstrap_look_up`, that send targets the synthetic
bootstrap *name* → `route()` returns `Unsupported` → one clearly-named next wall, decidable the
moment 3409 is serviced. Blast radius = one message, not an unbounded chain.

## Verified facts (this repo's MIG stack — read directly)

- **Router shape** (`crates/retrace-core/src/machmsg.rs`): `route(&Msg2, guest_task_port) -> Route`
  where `Route = { ServiceVmMap, StubMigReply(i32), Forward(&str), Unsupported(String) }`. It gates
  on `options == KOBJECT send+rcv`, then a msgh_id forward allowlist `{200,206,3418}`, then
  `dest == guest_task_port` id-cases `{4811→ServiceVmMap, 4822/8000/8001→StubMigReply}`, else
  `Unsupported`. `guest_task_port` is learned from `task_self_trap` (−28).
- **Reply builders are pure, golden-byte-tested functions.** `reply_header(out, msgh_size,
  reply_port, reply_id)` writes the 24-byte header with `REPLY_BITS = 0x1200` (bytes `00 12 00 00`,
  local = MOVE_SEND_ONCE) and reply_id = request_id + 100; `msgh_size` **excludes** the trailer.
  `encode_vm_map_reply` and `encode_mig_error` are the two existing shapes. `MACH_MSGH_BITS_COMPLEX
  = 0x8000_0000` is already defined.
- **Dispatch pattern to mirror** (`crates/retrace-core/src/lib.rs`, the `MACH_MSG2` arm, record
  ~:233-293 / replay ~:398-438): for `ServiceVmMap` — read the guest buffer at `m.data`, decode the
  request, compute the result, build `writes = vec![Region { ipa: m.data, bytes:
  encode_..reply(..) }]`, append `Event::Syscall { ret: MACH_MSG_SUCCESS, writes }`, call
  `apply_and_return(MACH_MSG_SUCCESS, false, &writes)`. Replay re-runs the identical builder and
  **byte-compares** its recomputed reply against the recording — that comparison IS the divergence
  oracle.
- **Ports are opaque u32 names, no namespace** — the guest's names are the host's names (forwarded
  in). A synthetic name we inject is just an opaque token the guest stashes; retrace writes the
  descriptor bytes directly (no kernel translates it).
- **A structural golden reference exists in a walk log:** the forwarded `host_get_clock_service`
  (206) reply is a complex message with one port descriptor of the same shape — usable to
  cross-check the descriptor's disposition/type bytes.

## The reply to synthesize (msgh_id 3409, which = 4)

48 bytes total, `msgh_size` field = 40 (trailer excluded):

```
header (24):  bits = REPLY_BITS | MACH_MSGH_BITS_COMPLEX = 0x8000_1200   (bytes 00 12 00 80)
              msgh_size = 40
              remote = 0            (send-once right consumed)
              local  = m.reply_port (0xe03 in the observed run)
              voucher = 0
              id = 3409 + 100 = 3509
desc count (4): 1
port descriptor (12):  name = SYNTHETIC_BOOTSTRAP_PORT   (fixed u32 constant)
                       pad1 = 0
                       word = (type << 24) | (disposition << 16) | pad2
                            = (0x00 << 24) | (0x11 << 16) | 0
                            = 0x0011_0000              (bytes 00 00 11 00)
                       // disposition 0x11 = MACH_MSG_TYPE_MOVE_SEND, type 0x00 = PORT_DESCRIPTOR
trailer (8):  00 00 00 00 08 00 00 00   (mach_msg_trailer_t { type 0, size 8 })
```

The synthetic name is a **fixed constant** (determinism), chosen distinct from every port name
observed in the run (task 0x203, host 0x1c03/0x1f03, reply ports, etc.) and outside the range the
host kernel is likely to assign via forwarded port-allocation traps — e.g. a high, distinctive value
like `0x0BAD_0B03` (the implementer confirms non-collision against the walk's forwarded names; a
collision would let a later send to a *different* port mis-route). It is NEVER forwarded — forwarding
3409 would hand the guest the host's real launchd bootstrap port.

## The mechanism

### 1. Router (`machmsg.rs`)
Add `Route::ServiceGetSpecialPort` (no payload — `which` lives in the request body, decoded in
dispatch like `decode_vm_map`). In `route()`, under `dest == guest_task_port`: `3409 =>
ServiceGetSpecialPort`. Add `decode_get_special_port(buf) -> Result<u32 /*which*/, String>`
(validate id 3409, `send_size >= 36`, extract `which_port` at offset 32 = header 24 + NDR 8). Add
`encode_get_special_port_reply(reply_port: u32, name: u32) -> Vec<u8>` building the 48-byte complex
reply above; factor a complex-capable header (either a `reply_header_complex` or a `complex: bool`
param) so `REPLY_BITS | MACH_MSGH_BITS_COMPLEX` is set. Add `const SYNTHETIC_BOOTSTRAP_PORT: u32`.

### 2. Dispatch (`retrace-core/src/lib.rs`), mirrored record + replay
New arm mirroring `ServiceVmMap`: read the buffer, `decode_get_special_port` → `which`; **assert
`which == 4`** (fail loud on any other `which` — we only model BOOTSTRAP); `writes = vec![Region {
ipa: m.data, bytes: encode_get_special_port_reply(m.reply_port, SYNTHETIC_BOOTSTRAP_PORT) }]`;
append `Event::Syscall { ret: MACH_MSG_SUCCESS, writes }`; `apply_and_return`. Replay: identical
builder, byte-compared by the oracle. Textually parallel on both sides (symmetry rule 1).

### 3. Determinism
The reply is a pure function of `(m.reply_port, SYNTHETIC_BOOTSTRAP_PORT)` — both deterministic
(reply_port comes from the deterministic guest, the name is a fixed const). Same builder on record
and replay; the trace carries the reply bytes (as for every serviced MIG); replay recomputes and
byte-matches. Nothing nondeterministic enters.

## Scope

**In:** the `ServiceGetSpecialPort` route + `decode_get_special_port` + `encode_get_special_port_reply`
+ complex-header support + the synthetic-name const; the mirrored record/replay dispatch arm;
unit tests (route, decode incl. reject non-4 `which`, byte-golden encode); the walk of `hello_dyn`
past 3409; re-park or advance `hello_dyn_e2e`. README Status + memory at close.

**Out / the honest edge:** any `which != 4` (host/other task special ports — fail loud, add when a
wall demands); `host_get_special_port`; a `bootstrap_look_up`/XPC send TO the synthetic port (the
contained next wall if libxpc isn't lazy — document + defer, do NOT pre-stub a launchd/XPC
subsystem); modeling a real bootstrap namespace. The deferred single-vCPU commpage-synthesis hygiene
(from M2-cpuid) remains deferred.

## Exit criterion

Unit tests green (route, decode, byte-golden reply); the walk advances past msgh_id 3409 (libxpc's
initializer proceeds); `just gate` green (69 + new tests, honest ignore count), clippy clean. If the
walk reaches `main → write → exit`: un-ignore `hello_dyn_e2e` + double-replay (the M2 headline gate).
Otherwise re-park honestly at the next boundary (a different init MIG id, or the one contained
`bootstrap_look_up` send). No faked green.

## Testing

1. **Router unit test:** `route(msg(3409, task_port, KOBJ), Some(task_port)) == ServiceGetSpecialPort`;
   3409 to a NON-task port → `Unsupported`; unknown ids still `Unsupported`.
2. **Decode unit test:** `decode_get_special_port` on a hand-built 36-byte request returns
   `which == 4`; rejects a short buffer and a wrong id; a `which != 4` request is rejected (or the
   dispatch assert covers it — test whichever holds the invariant).
3. **Byte-golden encode test:** `encode_get_special_port_reply(reply_port, name)` equals a
   hand-verified 48-byte expected buffer (header COMPLEX bits `00 12 00 80`, size 40, id 3509, desc
   count 1, descriptor `name / 0 / 00 00 11 00`, trailer). Cross-check the descriptor
   disposition/type bytes against a captured `host_get_clock_service` (206) reply from a live run
   (capture it via `RETRACE_TRACE=1`; it's a real complex-port reply of the same shape).
4. **Regression:** full `just gate` — the new route arm must not perturb existing MIG handling
   (vm_map/stub/forward paths unchanged); the existing machmsg golden tests stay green.
5. **The walk:** bounded traced `record-dyn hello_dyn`; standard fail-loud triage of the next wall.

## Risk register

1. **libxpc eagerly sends to the bootstrap port** (`bootstrap_look_up`) rather than caching it. Then
   the guest sends to `SYNTHETIC_BOOTSTRAP_PORT` → `route()` sees a non-task dest → `Unsupported` →
   next wall. *Mitigation:* that is the honest boundary; the walk names it precisely; do NOT expand
   into XPC servicing here. Contained to one message.
2. **Synthetic name collides** with a name the host kernel assigns to a real guest port via a
   forwarded allocation trap → a later legitimate send mis-routes. *Mitigation:* pick the const
   outside the observed/assigned range (high, distinctive) and verify no collision in the walk log;
   the fail-loud router surfaces any surprise rather than silently mis-serving.
3. **The complex descriptor bytes are wrong** (disposition/type/pad), failing the guest's
   `__MIG_check__Reply`. *Mitigation:* the byte-golden test + cross-check against the captured 206
   clock reply (same descriptor shape) pins them; the walk confirms libxpc accepts the reply.
4. **Asymmetry between record and replay dispatch arms.** *Mitigation:* the arms are textually
   parallel and the replay oracle byte-compares the recomputed reply — an asymmetry surfaces as a
   divergence, caught by the walk/replay.

## Components

- `crates/retrace-core/src/machmsg.rs` — `Route::ServiceGetSpecialPort`; `route()` arm;
  `decode_get_special_port`; `encode_get_special_port_reply`; complex-header support;
  `SYNTHETIC_BOOTSTRAP_PORT`; unit + golden tests.
- `crates/retrace-core/src/lib.rs` — the mirrored `ServiceGetSpecialPort` dispatch arm in record and
  replay `MACH_MSG2` handling.
- `crates/retrace/tests/hello_dyn_e2e.rs` — un-ignore + double-replay (if the walk reaches main) or
  re-park at the new boundary.
- README Status + memory (`retrace-objc-preoptimization-wall` chain) at close.

## Open questions for implementation planning

1. Complex-header factoring: a `complex: bool` param on `reply_header` vs a separate
   `reply_header_complex` — implementer's call; keep the existing non-complex callers byte-identical.
2. The exact synthetic-name const value (confirm non-collision against the walk's forwarded names).
3. Whether the walk reaches `main` (un-ignore + double-replay) or a new wall (re-park) — decided by
   the Task 2 walk.
