# retrace

A record/replay reverse debugger for Apple Silicon. See
`docs/superpowers/specs/2026-07-05-retrace-macos-record-replay-design.md`.

## Status: M0 — Box & Trace Spine ✅

Records a freestanding ARM64 guest inside a single-vCPU Hypervisor.framework VM and
replays it bit-for-bit from a snapshot, proving zero divergence over 200 fault-injection
seeds. Requires macOS 26.x on Apple Silicon.

M0's guest is a **freestanding synthetic binary** (`crates/retrace-guest/asm/hello.s`,
raw `write`/`exit` syscalls with the MMU off) — not the spec's `/bin/echo`-class
dynamically-linked program. Real dynamically-linked binaries need the dyld-shared-cache
loader, deferred to M2 (see below).

The divergence checker compares, per traced syscall, the `(num, args)` tuple and the
final exit code; M0's bit-for-bit guarantee rests on determinism-by-construction (the same
recorded inputs replayed through the same deterministic handler) plus CRC-checked trace
integrity, not on an exhaustive state comparison. (M1, below, adds a full-memory comparison
at exit as the divergence oracle's final check.)

```
just m0                                   # run the full gate
cargo run -p retrace -- record <macho> -o t.bin
cargo run -p retrace -- replay t.bin
```

Every binary is ad-hoc codesigned with `com.apple.security.hypervisor` automatically
(`.cargo/config.toml` runner). Non-root; SIP may stay enabled.

### Running tests

The in-process VM tests require `--test-threads=1`: Hypervisor.framework allows only one
VM per process on macOS, so tests that create a VM in-process must run one at a time.
`just m0` already sets this (`cargo test --workspace -- --test-threads=1`). A bare
`cargo test` may flake with `HV_BUSY` if the default multi-threaded test runner overlaps
two in-process VMs.

## Status: M1 — General Memory-Diff Syscall Recorder ✅

M1 replaces M0's hand-written `write`/`exit` handlers with a **general recorder**: on any
syscall trap it pointer-chases the argument registers, snapshots a window around each guest
pointer, forwards the real syscall to the host kernel (translating guest pointers to host
backing addresses), and diffs to find what the kernel wrote — logged as `writes: Vec<Region>`.
Replay applies the recorded writes and feeds the recorded return value; it never executes a
syscall itself. No per-syscall models — the same machinery handles `open`/`fstat`/`read`/
`close` without the recorder knowing anything about their semantics.

Proven on two guests (still freestanding, MMU-off, `crates/retrace-guest/asm/`):

- **`fileio`** — opens a fixture file, `fstat`s it, `read`s it, writes the bytes to stdout,
  closes it. Replays **byte-for-byte identically after the input fixture file is deleted** —
  the recorded `writes` (the kernel-filled read buffer) fully reconstruct the guest's memory
  without touching the filesystem again.
- **`mmapguest`** — `mmap`s an anonymous region, stores a byte pattern into it with ordinary
  loads/stores (no syscall), reads it back, and `munmap`s it. `mmap` is special-cased (it
  creates a new tracked backing at a deterministic fresh guest address); the plain stores
  replay by re-execution, not by diff.

The divergence oracle now includes a **final full-memory comparison** at guest exit (`Box_::
diff_memory`), on top of M0's per-syscall `(num, args)` check — so a divergence introduced
anywhere in guest memory, not just at a traced syscall boundary, is caught and named.

The trampoline, trace format (now with a 4-byte magic/version header and `Event::Syscall.
writes`), snapshot, divergence checker, and seeded swarm from M0 carry forward — the swarm
now records/replays both the file-I/O and mmap guests, 200 fault-injection seeds each,
proving the same zero-silent-divergence property as M0 over the new general recorder.

**Deferred to M2 or later:**
- **Error-ABI fidelity.** M1 assumes every recorded syscall succeeds; the macOS raw-syscall
  error convention (carry flag set, `x0` = errno) is not modeled. Guests/fixtures are
  constructed so nothing fails.
- **Honoring `munmap`/`mprotect`.** Both are recorded as no-ops in M1 (ret 0, no writes):
  with the MMU off, a trusted guest, and no address reuse, they write no guest memory, so
  skipping them is safe for now. A real loader with address-space reuse will need to honor
  them.
- **32-bit / narrow return-value fidelity** for syscalls that don't return a full 64-bit
  value.

## Status: M2 — The Loader (MMU-on, dyld, PAC) ✅ + M2-cache — Shared-Cache Re-signing ✅

M2 makes the box run **real, normally-compiled, dynamically-linked** code. It turns the guest
MMU on with guest-built **W^X** stage-1 page tables (executing a writable page hangs the vCPU on
Apple Silicon, so code is RO+exec and data is RW+non-exec), enables **PAC** with fixed keys,
loads a real arm64 Mach-O plus `/usr/lib/dyld` (a PIE dylinker, slid to a free base), and builds
the dyld4 process-start stack. The recorder gained full error-ABI fidelity (a raw-`svc` forwarder
that preserves the 64-bit return and the carry flag), a memory-safety clamp on forwarded counts,
honored `munmap`/`mprotect`, file-backed `mmap` staged through anonymous pages (a file-backed
`hv_vm_map` hard-panics macOS 26 — SPTM), and runtime exec-mmap promotion.

**M2-cache** solves the hard part the loader revealed: the arm64e **dyld shared cache** is bound
to the host process — its pointers are PAC-signed with the host's per-process keys and its
`__DATA` is host-dirtied — so a fresh-keyed guest cannot reuse the live cache. Rather than joining
the kernel shared region, the box **emulates the cache-mapping syscall itself**: a lazy per-page
pager maps each cache page from the file (pristine, fixed slide), walks its v5 slide-info fixup
chains, and **re-signs every arm64e auth pointer with the guest's own PAC keys** — using the guest
vCPU as an in-VM signing oracle (`pacia`/`pacda`), so Apple's PAC algorithm is never reimplemented.
This is **validated end-to-end**: real dyld maps the re-signed cache, restarts into the
cache-resident dyld, and **authenticates and executes thousands of re-signed cache pointers with
zero PAC faults**, running deep into `libSystem` initialization. The whole pager is a deterministic
function of (file, slide, fixed keys), so replay regenerates identical cache pages — nothing enters
the trace.

**What runs today:** the box, the loader, the memory-diff recorder + determinism oracle, and the
shared-cache re-signing — 43 tests plus the M1 seeded swarm, `clippy -D warnings` clean.

**Deferred to the next milestone (libSystem mach-IPC runtime).** The end-to-end gate
(`hello_dyn_e2e`, a `write()`-only dynamically-linked program recording and replaying byte-for-byte)
is present but `#[ignore]`d: past the cache, real dyld runs into `libSystem`/`libmalloc`
initialization, which reserves memory via a **mach message RPC** (`mach_msg2`) that must be serviced
against the guest's address space rather than the host task — the start of a distinct, larger
"libSystem runtime" subsystem (mach-IPC RPC emulation + the absent system daemons). The box, loader,
recorder, and cache re-signing are complete and validated; that runtime is the honest next boundary.

```
just m1                                   # run the full gate (same recipe as `just m0`)
```

## Status: M2-mach — mach-IPC Kernel-RPC Servicing ✅

**M2-mach** is a sibling sub-milestone of M2-cache, targeting the wall M2-cache's landing left
behind: past the re-signed shared cache, real dyld runs into `libSystem`/`libmalloc`
initialization, which reserves memory via a **mach message RPC** (`mach_msg2_trap`, trap −47)
rather than a fast trap — a request that was being forwarded to the **host** task instead of the
guest. M2-mach adds a pure MIG codec that decodes/encodes `mach_msg2` requests and replies against
guest memory, and dispatches on the decoded `msgh_id`/destination instead of blindly forwarding.

Two walls this uncovered both **fell**:

1. **libmalloc's nano "pointer range" reservation** — a `mach_vm_map` issued as `mach_msg2`
   requesting a **FIXED 24 GiB `PROT_NONE`** address-space reservation. `_kernelrpc_mach_vm_map`
   (msgh_id 4811) is now serviced on the guest's own IPAs; the FIXED case is handled by a new
   bookkeeping-only `guest_vm_reserve` (reserves the guest VA range with zero backing, matching
   what a real `PROT_NONE` reservation is), fixed up so it doesn't collide with the box's own
   `MMAP_BASE` bump allocator.
2. **The private `task_restartable` subsystem** (msgh_id 8000 `_register` / 8001
   `_synchronize`) — stubbed `KERN_SUCCESS` with no-op semantics, since a single-vCPU
   deterministic replay has no preemption to restart across.

A **decided allowlist** — `host_info` (200), `host_get_clock_service` (206), `semaphore_create`
(3418) — still forwards to the host (read-only queries / create-once calls with no guest-address
argument); everything else unrecognized fails loudly with the decoded name rather than silently
misrouting. Record/replay symmetry holds: replay recomputes and byte-compares every serviced
reply against the recording, the same divergence discipline as every other syscall path. An
in-VM guest test (`machmsg_e2e`, `crates/retrace/tests/machmsg_e2e.rs`) exercises a hand-built
wire-format 4811 request end-to-end — recorded and replayed, not just unit-decoded.

**What runs today:** everything from M2/M2-cache, plus the mach_msg2 codec and dispatch — 54
tests (0 failed, 1 known-ignored), including the M1 seeded swarm still showing zero silent
divergence, `clippy -D warnings` clean.

**What's deferred:** the `mach_msg2` surface serviced so far is narrow and demand-driven, not
exhaustive — deferred: port-namespace virtualization (real port rights, not the current
kernel-object fast path), daemon mach-IPC (no system daemons run in the guest yet),
non-`mach_msg2` legacy traps (`mach_msg`, −31/−32), vector-format messages, and full semaphore
semantics (`semaphore_create` currently forwards via the allowlist rather than being serviced).

**The end-to-end gate remains blocked — by a distinct, larger boundary, not by mach_msg2.**
`hello_dyn_e2e` (`crates/retrace/tests/hello_dyn_e2e.rs`) is still `#[ignore]`d. With both
mach_msg2 walls cleared, the recorded run now advances from ~177 traps to **~208**, deep into
`libSystem` initialization, and hits a **new** wall in Objective-C class realization
(`_map_images_nolock` → `addClassTableEntry`): `hello_dyn` is a plain **arm64** (not arm64e)
process, so libobjc **strips** the shared cache's arm64e isa pointers with a compile-time
47-bit `ISA_MASK` instead of authenticating them. retrace's guest runs a 36-bit VA
(`TCR_EL1.T0SZ=28`), so its PACDA signature lands in bits it doesn't expect the strip to touch,
producing a poisoned pointer and a data abort. The cache re-signing itself is proven correct
in isolation (in-guest `pacda`-sign → `autda` round-trips exactly); the mismatch is that real
macOS uses a 47-bit user VA (PAC bits cleanly above the mask) while retrace uses 36-bit.
Clearing it needs a 47-bit guest VA (`T0SZ=17`, a 3-level 16 KiB page-table walk instead of
today's 2-level) or an arm64e guest — core MMU/PAC work, distinct from mach-IPC servicing, and
the honest next milestone. See `docs/superpowers/specs/2026-07-07-retrace-m2-mach-design.md`.

## Status: M2-va47 — 47-bit Guest VA ✅

**M2-va47** clears the wall M2-mach's landing left behind: it widens the guest's stage-1
translation from a 36-bit to a **47-bit VA**. Concretely, it inserts one new **L1 table**
(`TTBR0 → L1 → L2 → L3`, a 3-level 16 KiB-granule walk instead of the old 2-level one) and sets
`TCR_EL1.T0SZ=17`. IPA/stage-2 stays 36-bit — this is purely a stage-1 (guest-VA) change, applied
universally across all guests in one config. This moves the hardware PAC signature into VA bits
[54:47], entirely above objc's compile-time 47-bit `ISA_MASK`, so libobjc's plain-arm64 isa strip
(`addClassTableEntry`) is now **lossless** instead of leaving live signature bits behind. Like
every other page table in the box, the new L1 rides in the snapshot, so determinism is preserved:
`restore` re-points `TTBR0` at it without rebuilding.

This is **proven** two ways. First, a dedicated guest+test, `strip47` (`crates/retrace/tests/
strip47_e2e.rs`): it `pacda`-signs a fixed pointer and objc-style-ANDs it with `ISA_MASK`, and the
test asserts the strip is lossless — genuinely **RED** under the old 36-bit VA (PAC bits `0xB0,
0x5E` survived the mask) and **GREEN** under the widened 47-bit VA. Second, the full suite stays
green under the new config: `just m1` reports **56 passed, 0 failed, 1 ignored**, clippy clean.

**Honestly blocked — by a new, distinct wall, not the one this milestone targeted.** The
end-to-end gate (`hello_dyn_e2e`) stays `#[ignore]`d. The VA widening does clear the isa-strip
wall — the old poisoned-isa data abort is gone, and the run advances past the isa load in
`addClassTableEntry` — but objc doesn't stop there: 8 instructions later, `addClassTableEntry+0x70`
executes `autdb x16, x17`, **authenticating** (not stripping) the class `data()`/`bits` pointer
with the **DATA-B key**, address-diversified and blended with discriminator `0xc93a`. This
hardware-faults FPAC (EC=0x1c), because retrace's M2-cache re-signer is **A-family only**: the
dyld v5 slide-info format cannot express B-family keys at all (`cache.rs::decode5` carries a
single IA/DA key bit), and the in-guest signing stub implements only `pacia`/`pacda`/`autia`/
`autda` — no `pacib`/`pacdb`/`autib`/`autdb`. So this DB-signed cache pointer keeps its host-key
signature and fails to authenticate under the guest's DB key. Clearing it needs **B-family
(DB/IB) PAC re-signing** — extending the re-signer and signing stub, likely objc-structure-aware —
a distinct, larger subsystem from widening the VA, and the honest next milestone.

**Deferred:** an arm64e guest path, 4 KiB-granule VA layouts, and the swarm extension to the dyld
guest. See `docs/superpowers/specs/2026-07-10-retrace-m2-va47-design.md`.

## Status: M2-bfam — objc B-family PAC ✅

**M2-bfam** clears the wall M2-va47's landing left behind: past the 47-bit-VA isa strip,
`addClassTableEntry+0x70` executes `autdb x16, x17` — a hardware **authenticate** (not a strip) of
the class `data()` pointer with the **DATA-B key**, which FPAC-faults (EC=0x1C) because M2-cache's
re-signer is **A-family only** (the dyld v5 slide-info format has room for only one IA/DA key bit,
so B-family-signed cache pointers were never re-signed at all). M2-bfam adds a new arm to the
shared run loop, `Box_::try_emulate_fpac_auth`: on an FPAC fault it decodes the faulting `aut*`
instruction at `ELR_EL1` (`retrace_arch::decode_aut_rd`, covering the register and zero-modifier
`AUTI*/AUTD*` forms), strips its destination register to the canonical 47-bit VA, and skips the
instruction — emulating a successful authenticate. Like the existing timebase/undef-MRS arms, this
lives *below* the record/replay layer (`run()` is shared), so it fires identically on both sides
and nothing enters the trace — determinism is automatic.

This is **proven** two ways. First, a dedicated micro-test, `bfamstrip` (`crates/retrace/tests/
bfamstrip_e2e.rs`): a guest DATA-B-signs a pointer, corrupts a PAC bit so `autdb` FEAT_FPAC-faults,
and the test asserts the box strips it back to the original — genuinely exercising the fault path
end-to-end, record and replay. `decode_aut_rd` also carries its own unit test covering every
register/zero-modifier encoding. Second, in the live dynamic run the arm fires **exactly 3 times**,
every one an `autdb x16, x17` inside libobjc (`addClassTableEntry`, `dataSegmentsContain`,
`realizeClassWithoutSwift`), each recovering a well-formed pointer landing cleanly inside libobjc's
`__AUTH_CONST` segment — mathematically-correct strips, not garbage — carrying `hello_dyn` from
~208 to ~216 traps **past** the original `addClassTableEntry+0x70` `autdb` wall.

**Honestly blocked — by a new, distinct wall, not the one this milestone targeted.** The
end-to-end gate (`hello_dyn_e2e`) stays `#[ignore]`d. Past the B-family auth, objc self-aborts
(exit 134) inside `realizeClassWithoutSwift → validateAlreadyRealizedClass`: "realized class ...
has corrupt data pointer: malloc_size(...)=0". objc is **dynamically realizing** a class that
already lives in the shared cache, and its `data()` pointer correctly strips to a **preoptimized,
cache-resident** `class_rw_t` in libobjc `__AUTH_CONST` — legitimately not a `malloc`-heap
allocation, so `malloc_size` returns 0 and objc fatals. A real process never takes this path: it
uses objc's **shared-cache preoptimization** fast path, where cache classes are pre-realized *in
the cache* and `realizeClassWithoutSwift` is never called on them. That fast path is disabled in
the guest — the re-signed, demand-paged cache no longer presents as a trusted objc-optimized cache
(the very re-signing M2-cache does to defeat FPAC invalidates the pointers objc's preoptimization
vouches for) — so libobjc falls back to dynamic realization, which is fundamentally incompatible
with preoptimized cache-resident metadata. Clearing this needs the guest to present a valid,
trusted objc-optimized shared cache (`objc_opt` header, selector/class/protocol hash tables,
cache-trust) — a distinct, larger subsystem entangled with the M2-cache re-signer design itself,
not another `aut` to strip. That's the honest next milestone, not B-family PAC. Full anatomy in
`.superpowers/sdd/task-m2bfam-2-report.md`.

**What runs today:** everything from M2/M2-cache/M2-mach/M2-va47, plus the strip-on-FPAC B-family
auth emulation — `just m1` reports **58 passed, 0 failed, 1 ignored**, clippy clean.

**Deferred:** combined auth-and-use B-family forms (`braab`/`blraab`, `ldraa`/`ldrab` — no
destination register to strip, the auth is implicit in a branch or load), an arm64e guest, and the
swarm extension to the dyld guest. See
`docs/superpowers/specs/2026-07-10-retrace-m2-bfam-design.md`.
