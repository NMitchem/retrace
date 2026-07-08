# retrace M2-mach — mach-IPC kernel-RPC servicing (emulate `mach_msg2` MIG against the guest)

**Design spec — 2026-07-07.** Sub-milestone of M2 (sibling of M2-cache), targeting the wall
documented in `.superpowers/sdd/task-m2c6-report.md`: past the re-signed shared cache, real dyld
runs deep into libSystem init and aborts in libmalloc because its mandatory nano "pointer range"
reservation is a `mach_vm_map` issued as a **mach message RPC** (`mach_msg2_trap`, trap −47) that
retrace forwards to the **host task** — so the kernel maps memory into retrace's own address
space, returns an address outside the guest's nano range, and libmalloc aborts
("pointer range initial reservation failed").

## What this is

M2-mach intercepts `mach_msg2` in the box, decodes the MIG request from guest memory, and
services address-space RPCs against the **guest's** IPAs using the same machinery that already
services the trap-based `_kernelrpc_mach_vm_*_trap` fast paths. Read-only kernel queries keep
forwarding (decided allowlist, logged); optional kernel features are stubbed deterministically;
everything else fails loudly with a decoded name. Exit: `hello_dyn_e2e` un-ignored — a real
dynamically-linked `write()` program records and replays byte-for-byte.

## Verified facts (this host, macOS 26.x arm64e — live traced run, 2026-07-07)

A `RETRACE_TRACE=1 record-dyn` run of `hello_dyn` reproduces the wall at trap ~180 (EC=0x3c `brk`
in libmalloc, `elr=0x180322818`). Everything below is read off that run plus mig-generated stubs
from this SDK's `.defs` (authoritative for this OS build):

- **Exactly six `mach_msg2` calls happen before the abort, five distinct RPCs, all kernel-object
  calls.** Every one carries `options = 0x200000003` — per xnu's open-source `message.h` (these
  flags are SPI, not in the public SDK): `MACH64_SEND_MSG | MACH64_RCV_MSG |
  MACH64_SEND_KOBJECT_CALL`, non-vector. The flag-name mapping is asserted at decode time (any
  options shape outside the expected ones fails loudly), so a wrong constant cannot mis-route
  silently. **Zero daemon mach-IPC occurs before the wall.**
- The six, by msgh_id (destination in parens): `host_info` 200 (host port), `host_get_clock_service`
  206 (host port), `semaphore_create` 3418 (task port), `_kernelrpc_mach_vm_map` 4811 ×2 (task
  port), `mach_vm_deferred_reclamation_buffer_allocate` 4822 (task port).
- **4811 request layout** (mig, pack(4), 100 bytes = observed send size 0x64): header(24) +
  body(4) + one `mach_msg_port_descriptor_t` `object`(12, `MACH_PORT_NULL` here — the port
  argument is *why* this call can't use the −15 fast trap) + NDR(8) + `address`(8) `size`(8)
  `mask`(8) `flags`(4) `offset`(8) `copy`(4) `cur_protection`(4) `max_protection`(4)
  `inheritance`(4). Reply: header + NDR + RetCode(4) + `address`(8). Reply msgh_id = request+100.
- **4822** is libmalloc's vm_reclaim ring-buffer setup (`len`,`max_len`; 40 bytes = observed
  0x28). The kernel feature is optional; libmalloc handles unavailability — a deterministic
  failure stub suffices.
- **`mach_msg2_trap` has an 8-register ABI** — x0 data, x1 options, x2 bits|send_size≪32,
  x3 dest|reply≪32, x4 voucher|msgh_id≪32, x5 desc_count|rcv_name≪32, x6 rcv_size|priority≪32,
  x7 timeout. `Stop::Syscall` captures only x0–x6 today: **x7 is silently dropped** (latent
  forwarding bug; benign so far — x7 was 0 at the wall).
- **Forwarding −47 today actively mutates host state:** the host kernel mapped 1 MiB into
  retrace's own task (leaked when libmalloc's follow-up `mach_vm_deallocate` was serviced against
  the *guest*), and libmalloc's failure warning was `sendto`'d to the **host syslogd** over an
  AF_UNIX socket. Interception removes both.
- The failure sequence: 4811 → host returns host-space address `0x105478000` → libmalloc
  deallocates + logs to syslog → retries 4811 → aborts. The trap-based −15 attempts in between
  are already serviced correctly on guest IPAs.
- `forward_and_diff`'s captured `writes` for the six calls include **authentic kernel reply
  bytes** (the host kernel wrote its reply into the translated guest buffer) — a golden reference
  for reply header bits, sizes, and trailer, available before any new code lands.

## Scope

**In:** decode + route `mach_msg2` (−47); service the mach_vm MIG subsystem (4811 map; 4800
allocate / 4801 deallocate / 4802 protect if the walk surfaces them) against guest IPAs; stub
4822; keep forwarding a decided allowlist (200, 206, 3418); widen trap capture to x0–x7; the
empirical walk of the remaining libSystem initializers until `hello_dyn` reaches
`main() → write → exit`; loud-fail everything unrecognized.

**Out (explicitly deferred):** port-namespace virtualization (host port *names* still enter guest
state via forwarded −26…−29 traps; replay-consistent because they replay from the trace); daemon
mach-IPC emulation (bootstrap/launchd, xpc, notifyd, os_log) beyond cleanly-failing stubs *if*
the walk demands them and the library tolerates failure (launchd-less processes are a supported
macOS reality); the older `mach_msg_trap` (−31) / `mach_msg_overwrite_trap` (−32) paths unless
observed; vector-form (`MACH64_MSG_VECTOR`) messages; semaphore semantics (create is forwarded;
signal/wait on it would be new work — none observed during init; threading is M4's problem).

## Exit criterion

`hello_dyn_e2e` un-ignored and green: record prints `hi\n`; replay reproduces stdout
byte-for-byte; per-syscall `(num, args)` checks and the final full-memory diff pass. Existing 43
tests + the M1 seeded swarm stay green; `clippy -D warnings` clean. Plus a double-replay test
(same trace replayed twice: identical output, memory diff clean both times). Extending the seeded
swarm to the dyld guest is a stretch goal contingent on measured wall-clock cost.

## The mechanism

### 1. Pure MIG codec (`crates/retrace-core/src/machmsg.rs`)

All protocol knowledge in pure functions — no VM access, no I/O, unit-testable against byte
fixtures:

- **Register unpack:** `[u64;8]` → `{data, options, bits, send_size, dest, reply_port, voucher,
  msgh_id, desc_count, rcv_name, rcv_size, priority, timeout}` per the packing above.
- **Request decode:** validate header against the unpacked registers (msgh_id, size, complex bit
  vs desc_count), then per-routine typed decoders (4811 first; layouts embedded as documented
  offsets with the mig-generated structs quoted in comments).
- **Reply encode:** `(request header, retcode, out-args)` → reply bytes: header (msgh_id+100,
  bits/trailer golden-copied from a captured real kernel reply), NDR, RetCode, out fields.
  MIG failure replies are `mig_reply_error_t` (header + NDR + RetCode).
- **Routing:** `(options, dest, msgh_id)` → `ServiceVm | Stub | Forward | Unsupported(decoded)`.
  Non-KOBJECT, vector-form, rcv-only, or malformed ⇒ `Unsupported` — never a silent forward.

### 2. Loop state: the guest's task port name

Routing needs "dest == the guest's task port". Both loops learn it the same way: observe the
result of `task_self_trap` (−28) as it flows through the loop (record: forwarded result; replay:
recorded result). Until −28 has been seen, no message can route to `ServiceVm` — any task-destined
KOBJECT call before that is `Unsupported` (fail-loud; in the observed run −28 fires long before
the first −47).

### 3. Record dispatch (new arm, num = −47)

Read the send buffer (`send_size` from x2 high) via `read_guest`; route:

- **ServiceVm (4811):** extract `(address hint, size, flags, cur_protection)`; call the existing
  `guest_vm_map` (honors free hints, bump-allocates otherwise, exec promotion for PROT_EXEC);
  encode a KERN_SUCCESS reply carrying the chosen guest address; write it into the receive buffer
  (same `data` pointer — non-vector combined send/rcv); record
  `Event::Syscall { num, args, ret: MACH_MSG_SUCCESS, writes: [reply @ data] }`; `apply_and_return`.
  MIG allocate/deallocate/protect route to `guest_vm_map`/`guest_munmap`/`guest_mprotect`
  identically if the walk surfaces them.
- **Stub (4822):** encode a `mig_reply_error_t` failure retcode (exact code chosen at
  implementation time by matching what a vm_reclaim-less kernel returns; libmalloc must take its
  no-reclaim path); record + apply as above.
- **Forward (200, 206, 3418):** unchanged `forward_and_diff` path, now with an eprintln naming
  the forwarded RPC (decided allowlist, not a default).
- **Unsupported:** return a record error naming msgh_id, dest, options, size + `dbg_backtrace` —
  the next work item names itself, M2c-6 style.

### 4. Replay dispatch (mirror arm)

`ServiceVm`/`Stub`: **re-service** — recreate the guest mapping (memory must exist for the guest
to touch), re-encode the reply, and **verify it byte-equals the recorded `writes`** (divergence
landmark, exactly like the trap-based `mach_vm_map` IPA verification). `Forward`: apply recorded
writes + ret (never executes). Same task-port learning, same routing — lockstep by construction.

### 5. Determinism & the oracle

A serviced reply is a deterministic function of (message bytes, allocator state), both identical
across record/replay; the recorded reply doubles as a per-RPC divergence check. Events stay plain
`Syscall` records, but widening args x0–x6 → x0–x7 changes the serialized `Event::Syscall`, so
the trace format version bumps (`RT\0\x02` → `RT\0\x03`; old traces are rejected loudly by the
existing magic check, consistent with the no-cross-version-portability posture). The forwarded allowlist (host_info, clock service,
semaphore_create) carries the same same-boot fidelity caveat as sysctl/commpage — recorded once,
replayed exactly. Net fidelity change vs today: the risk surface shrinks from "all of mach_msg2"
to three read-only/create-once calls, and record runs stop leaking host mappings and writing to
host syslog.

### 6. Diagnostics

`RETRACE_TRACE=1` additionally hexdumps −47 send buffers before dispatch and synthesized replies
after (bounded), so the empirical walk keeps naming each new failure precisely.

## Components (building on M2 / M2-cache)

- `retrace-core/src/machmsg.rs` — new: codec + routing (pure).
- `retrace-core/src/lib.rs` — record/replay −47 arms; task-port learning; forwarded-RPC logging.
- `retrace-box/src/lib.rs` — `Stop::Syscall` args widened `[u64;7]` → `[u64;8]` (capture x7;
  `host_svc` already takes 8); no other box changes — servicing reuses `read_guest`,
  `guest_vm_map`/`guest_munmap`/`guest_mprotect`, `apply_and_return`.
- `retrace-trace` — `Event::Syscall.args` widened to `[u64;8]`; `TRACE_MAGIC` version byte bumped
  to 0x0003.
- `retrace-guest` — new freestanding mach-msg guest (see Testing).
- README + main spec milestone note; memory update at close.

## Testing

1. **Codec unit tests (no VM):** register unpack round-trips; request decode against golden
   fixture bytes captured from the live run's six messages; reply encode verified byte-for-byte
   against an authentic kernel reply captured via today's `forward_and_diff` **before** the
   interception lands; routing tests covering every loud-fail branch.
2. **In-VM codec test independent of dyld:** a freestanding guest that hand-builds a MIG 4811
   request, issues `mach_msg2` via raw `svc`, asserts retcode/address in the reply, and
   stores/loads through the mapped memory. Fast, deterministic, immune to libSystem init churn.
3. **The gate:** `hello_dyn_e2e` un-ignored (see Exit criterion) + double-replay stability test.
4. **Regression:** full suite `--test-threads=1` + swarm + clippy, per `just m1`.

## Risk register

1. **The walk past malloc is unbounded in advance.** libdispatch/libxpc/libtrace initializers run
   next; new msgh_ids or daemon lookups may surface. *Mitigation:* every unknown fails loudly with
   its decoded name; triage rule is fixed (guest-address-space ⇒ service; read-only query ⇒
   forward+log; optional feature ⇒ stub; daemon ⇒ cleanly-failing stub only if tolerated). M2c-6's
   walk was 11 issues; budget for the same order. If a genuinely unserviceable dependency appears
   (e.g. an initializer that *requires* a live daemon), that boundary is documented and becomes
   the next milestone — the gate stays `#[ignore]`d with an updated reason rather than faked.
2. **Reply-format fidelity** (header bits, trailer, size checks in `__MIG_check__Reply`).
   *Mitigation:* golden-copy from authentic captured kernel replies, not from documentation;
   the in-VM guest test exercises libsystem-independent decode; dyld/libmalloc's own MIG checks
   are the loudest possible verifier.
3. **`guest_vm_map` semantics gap vs MIG `mach_vm_map`** (mask/alignment honored? nano wants
   `0x600000000`-range placement). *Mitigation:* the existing handler already honors free address
   hints (M2c-6 fix #11); if nano's request arrives hint-less with a mask, extend the allocator to
   honor the mask — verified by the reservation succeeding (libmalloc checks the range itself).
4. **Task-port name learning order** (a task-destined RPC before −28). *Mitigation:* fail-loud
   routing until learned; the observed trap order makes this theoretical for the gate.
5. **OS-update brittleness** (msgh_ids/layouts drift across macOS releases). *Mitigation:* same
   posture as the cache loader — IDs/layouts pinned from this SDK's mig output, assert-and-fail
   loudly on mismatch (msgh_id + size validation on every decode), never silently mis-parse.

## Non-goals / explicitly deferred

Port-namespace virtualization; daemon-IPC emulation beyond tolerated failure stubs; `mach_msg`
(−31)/`mach_msg_overwrite` (−32); vector-form messages; semaphore wait/signal semantics; any
general libSystem-runtime completeness beyond what the `write()`-only gate demands. Each is
deliberately deferred to keep this the smallest slice that un-ignores the gate.

## Open questions for implementation planning

1. Exact stub retcode for 4822 (read what a reclaim-less kernel returns, or what libmalloc's
   fallback branch accepts — determined during Task 1's golden-capture run).
2. Whether the walk surfaces MIG variants of allocate/deallocate/protect (pre-wire the decoders
   or add on demand — plan should default to on-demand, decoders are ~10 lines each).
3. Swarm extension cost for the dyld guest (measure one record/replay wall-clock first).
