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

**Honestly blocked — by a new, distinct wall, not the one this milestone targeted.**
**⚠ SUPERSEDED / CORRECTED (M2-tbi, 2026-07-14):** the diagnosis in the paragraph below was **wrong**.
The wall past the B-family auth was **not** objc shared-cache preoptimization / cache-trust; it was a
one-line guest-MMU bug — a bit-63 / `FAST_IS_RW_POINTER` PAC collision because the guest `TCR_EL1`
left **TBI off**. It is fixed in M2-tbi. The paragraph is kept below as milestone history; see
**"Status: M2-tbi — arm64e data-pointer PAC (TCR TBI)"** for the verified root cause and fix.

> The end-to-end gate (`hello_dyn_e2e`) stays `#[ignore]`d. Past the B-family auth, objc self-aborts
> (exit 134) inside `realizeClassWithoutSwift → validateAlreadyRealizedClass`: "realized class ...
> has corrupt data pointer: malloc_size(...)=0". objc is **dynamically realizing** a class that
> already lives in the shared cache, and its `data()` pointer correctly strips to a **preoptimized,
> cache-resident** `class_rw_t` in libobjc `__AUTH_CONST` — legitimately not a `malloc`-heap
> allocation, so `malloc_size` returns 0 and objc fatals. A real process never takes this path: it
> uses objc's **shared-cache preoptimization** fast path, where cache classes are pre-realized *in
> the cache* and `realizeClassWithoutSwift` is never called on them. That fast path is disabled in
> the guest — the re-signed, demand-paged cache no longer presents as a trusted objc-optimized cache
> (the very re-signing M2-cache does to defeat FPAC invalidates the pointers objc's preoptimization
> vouches for) — so libobjc falls back to dynamic realization, which is fundamentally incompatible
> with preoptimized cache-resident metadata. Clearing this needs the guest to present a valid,
> trusted objc-optimized shared cache (`objc_opt` header, selector/class/protocol hash tables,
> cache-trust) — a distinct, larger subsystem entangled with the M2-cache re-signer design itself,
> not another `aut` to strip. That's the honest next milestone, not B-family PAC. Full anatomy in
> `.superpowers/sdd/task-m2bfam-2-report.md`.
>
> *(Correction: the `data()` value was **not** a cache-resident `class_rw_t` — it symbolicates to
> `_OBJC_CLASS_RO_$_NSObject`, a read-only `class_ro_t`. objc only reached `validateAlreadyRealizedClass`
> because it misread unrealized `NSObject` as already-realized: `has_rw_pointer()` tests bit 63 of the
> raw `class_data_bits::bits` word, and the guest value `0x964a8001ed950f80` had bit 63 set by the
> re-signed data-pointer PAC (TBI off). No objc-opt subsystem was ever needed.)*

**What runs today:** everything from M2/M2-cache/M2-mach/M2-va47, plus the strip-on-FPAC B-family
auth emulation — `just m1` reports **58 passed, 0 failed, 1 ignored**, clippy clean.

**Deferred:** combined auth-and-use B-family forms (`braab`/`blraab`, `ldraa`/`ldrab` — no
destination register to strip, the auth is implicit in a branch or load), an arm64e guest, and the
swarm extension to the dyld guest. See
`docs/superpowers/specs/2026-07-10-retrace-m2-bfam-design.md`.

## Status: M2-tbi — arm64e data-pointer PAC (TCR TBI) ✅

**M2-tbi is a correction, not a feature.** The wall M2-bfam's close-out documented as "objc
shared-cache preoptimization / cache-trust" (see the ⚠ note above) was a **misdiagnosis**. Past the
B-family strip, objc self-aborts (exit 134) in `realizeClassWithoutSwift → validateAlreadyRealizedClass`
("realized class `0x1ec2f1618` has corrupt data pointer: malloc_size(`0x1ed950f80`)=0"). M2-bfam read
this as objc dynamically realizing a *preoptimized, cache-resident `class_rw_t`* and concluded the
guest needed a trusted objc-optimized cache. **That was wrong.** The verified root cause is a
one-line guest-MMU bug.

**The evidence that disproves the old narrative:**

- The fatal class `0x1ec2f1618` is **`NSObject`**, and its `data()` pointer `0x1ed950f80`
  symbolicates (guest coords, libobjc `__TEXT` @ `0x18008C000`) to **`_OBJC_CLASS_RO_$_NSObject` — a
  `class_ro_t`, not a `class_rw_t`.** A correctly-realized class never points `data()` at its own
  read-only `class_ro_t`; this only happens if objc took the already-realized branch on a class that
  is actually **unrealized**.
- `validateAlreadyRealizedClass` (objc4-951.7, `objc-runtime-new.mm:2942`) is an **unconditional**
  `malloc_size(rw) >= sizeof(class_rw_t)` check — there is **no** `inSharedCache` / cache-range /
  trust guard to satisfy, and no cache-resident `class_rw_t` in this ABI (`RW_REALIZED`/`setData` is
  set at 3 `calloc`-backed objc4 sites, 0 in dyld). The whole "present a trusted preoptimized cache"
  premise had nothing to satisfy.
- The host runs `hello_dyn` **fine** with `OBJC_DISABLE_PREOPTIMIZATION=YES` — preopt fully disabled
  is not this fatal. So "guest preopt disabled → this fatal" is disproven directly.

**The real mechanism (a bit-63 PAC collision).** objc's `has_rw_pointer()`/`isRealized()`
(`objc-runtime-new.h`) reads **bit 63** (`FAST_IS_RW_POINTER = 0x8000000000000000`) of the **raw**
`class_data_bits_t::bits` word in guest memory — a plain `bits & FAST_IS_RW_POINTER`. The observed
guest value `0x964a8001ed950f80` has **bit 63 set** (top byte `0x96`). So objc reads unrealized
`NSObject` as already-realized, skips realization, and validates its `data()` (the `class_ro_t`,
`malloc_size` = 0) → fatal. Bit 63 is polluted because the guest `TCR_EL1` leaves **TBI off**: under
the 47-bit VA the re-signed data-pointer PAC field spans bits [63:56] ∪ [54:47] — **including bit
63** — so, most likely, the M2-cache re-signer's A-family auth stored in `class_data_bits` lands its
PAC on objc's realized flag. On real hardware that same word has bit 63 clear. This slipped past every prior wall
because the box signs/authenticates with its own keys (internally symmetric); the break surfaces
only when objc reads the **raw** bits and treats bit 63 as a semantic flag — a guest-vs-host ABI
mismatch, not a PAC or objc-opt gap.

**The fix (one constant).** Match Apple's arm64e user configuration: enable **TBI0 (bit 37)** and
**TBID0 (bit 51)** in the guest `TCR_EL1` (`0x1_0080_B511 → 0x8_0021_0080_B511`). `TBI0` gives data
pointers top-byte-ignore, so their PAC lands in [54:47] and the top byte (incl. bit 63) is preserved
from the canonical pointer = 0; `TBID0` exempts **instruction** pointers from TBI, keeping their PAC
full-strength (Apple's TBID posture). A re-signed data pointer's bit 63 now stays 0,
`has_rw_pointer()` reads `NSObject` as unrealized, objc realizes it normally, and the
`validateAlreadyRealizedClass` fatal is **gone**. The same constant is read by every CPU-init
constructor — record's `load`/`load_dynamic` and replay's `restore` — so both sides configure TBI
identically; nothing enters the trace. This is a load-bearing MMU invariant in the same class as
W^X / `T0SZ`. The M2-bfam strip-on-FPAC arm is unaffected (it still
strips the DB-key `autdb` at `data()`; the fix corrects the separate bit-63 flag read that precedes it).

**Honestly blocked — at the new mmap demand-commit wall.** The end-to-end gate (`hello_dyn_e2e`)
stays `#[ignore]`d. With classes realizing correctly, objc heap-allocates each `class_rw_t`
(`objc::zalloc → calloc → libmalloc → mmap`) and first-touches the allocation, which faults with a
**level-3 translation fault** (data abort EC=0x24, FSC=0x7) on an **unmapped** page in the mmap
region (`MMAP_BASE = 0xA_0000_0000`) that libmalloc obtained via an anonymous `mmap` but retrace
reserved without backing. (One run faults at `far=0xa0010e744 = MMAP_BASE+0x10e744`; the exact offset
is **not** invariant — it shifts with argv layout, e.g. the `-o` path length. The invariant is the
first-touch fault on an unmapped page in `[MMAP_BASE, …)`.) Clearing it needs retrace to back
first-touched mmap pages with anon memory and, on record, capture the zero-fill as writes so replay
reproduces it — a **memory-management** task, materially smaller than the objc-opt subsystem the
misdiagnosis feared, and the next milestone.

**What runs today:** everything from M2/M2-cache/M2-mach/M2-va47/M2-bfam, now with objc class
realization working past the (disproven) preoptimization wall — `just gate` reports **58 passed, 0
failed, 1 ignored**, clippy clean. The TCR change perturbs no existing test.

**Deferred:** the mmap demand-commit wall itself (its own milestone); un-ignoring `hello_dyn_e2e`
green (the guest doesn't reach `main → write → exit` yet); an arm64e guest; the swarm extension. See
`docs/superpowers/specs/2026-07-14-retrace-m2-tbi-design.md`.

## Status: M2-mmapcommit — mach-VM Reservation Demand-Commit ✅

**The wall M2-tbi left behind falls with a below-the-trace demand-committer.** Past objc class
realization, libmalloc's **xzone** allocator manages memory in two states no prior retrace path
produced: it `mach_vm_map`s a large **PROT_NONE reservation** (`cur_protection == 0`) — pure address
space, no backing — then commits and first-touches pages inside it lazily. retrace's
`guest_vm_reserve` produced the reservation as *bookkeeping only* (a returned address, no stage-2
map), so the guest died at a **level-3 translation fault** (data abort EC=0x24 FSC=0x7) the first time
xzone touched an uncommitted reservation page (`_xzm_segment_group_alloc_chunk`, reached via
`realizeClassWithoutSwift`). Eager backing is a non-starter — libmalloc's nano-band reservation alone
is **24 GiB**, larger than the entire 36-bit IPA space.

**The fix (below the trace, mirrored).** `guest_vm_reserve` now records each reservation's
page-granular extent in a `reservations: Vec<(start, len)>` on `Box_` (reset to empty in `restore`
alongside `mmap_next`, so replay's address space matches record's). On a stage-2 fault,
`Box_::commit_reserved_page(ipa)` — the moral twin of the shared-cache demand-pager, minus the file
read and re-sign — backs *exactly* the faulting page with a fresh **zeroed** anon page iff it lies
inside a tracked reservation and isn't already backed; a fault outside every reservation stays
**fatal** (a wild pointer must never be silently materialized). It is dispatched by a second guard
inserted immediately after the cache pager's, **textually identical** in record and replay's
`Stop::Other` arms (symmetry rule 1). Zero-fill plus the guest's own re-executed stores are identical
on both sides, so **nothing about a committed page enters the trace** — the same posture as the cache
pager, the timebase MRS, and the FPAC strip. The trap-path `mach_vm_map` (num −15) got the same
`cur_protection == 0 → reserve` split as the MIG 4811 route, so a reservation can't arrive via the
trap and be eager-backed (fatal at 24 GiB).

**⚠ A deliberate, spec-sanctioned loss of wild-pointer detection.** Once the run reserves libmalloc's
24 GiB nano band `[0x4_0000_0000, 0xA_0000_0000)`, a stray pointer landing anywhere in that band now
demand-commits a zero page instead of staying a fatal fault. retrace is a **recorder, not a memory
protector** — it will satisfy a first-touch inside a reservation even where a real kernel would
`SIGSEGV` a PROT_NONE guard page. Stage-1 W^X still holds (committed pages are data-only, non-exec).
We accept this: enforcing PROT_NONE guard-fault semantics is explicitly out of scope.

**Honestly blocked — at the libmalloc xzone SEGMENT-allocator wall (a new, distinct boundary).** With
reservation pages demand-committed, the run advances **one frame deeper** — from the xzone *chunk*
allocator into `_xzm_segment_group_alloc_segment+0x90` — then faults NEAR-NULL (data abort EC=0x24
FSC=0x7, `far=0x178`). The faulting instruction is `ldrb w9, [x8, #0x178]` with **x8 = 0**; x8 was
just loaded by `ldp x27, x8, [x20, #0x10]` from `x20 = 0xa0010e4c8` — a **demand-committed** xzone
segment-group metadata page (the `ldp` itself **succeeds**, proving `commit_reserved_page` backed it),
whose `+0x18` slot is `0`. So xzone reads a **NULL segment pointer out of its own committed metadata**
and dereferences it. This is **distinct from demand-commit, which did its job**: the fault is an
xzone allocator-state inconsistency — a null segment link where a real kernel-backed run holds a valid
pointer — under retrace's approximated VM-op semantics and single-vCPU (no-preemption) model (a 12×
`gettimeofday` deadline-spin, with no second thread to make progress, immediately precedes it).
Investigating xzone's segment-group allocation protocol is a **distinct subsystem, deferred** — not
walked into (design spec, risk register #1). The gate (`hello_dyn_e2e`) stays `#[ignore]`d, re-parked
with the verified anatomy above. Trap count varies (~206–214) with the forwarded-`gettimeofday`
deadline-spin — not a determinism defect: record forwards real time, replay reproduces the recorded
values.

**What runs today:** everything from M2/M2-cache/M2-mach/M2-va47/M2-bfam/M2-tbi, plus mach-VM
reservation demand-commit — `just gate` reports **61 passed, 0 failed, 1 ignored**, clippy clean. The
new arm is inert for every existing test (none fault inside `[MMAP_BASE, …)`); the reservation
round-trip is proven by `reservecommit` (reserve → first-touch commit → store → load, byte-identical
replay; a two-page store proves per-page, not per-reservation, granularity) and the fail-loud
negative (`wild_store_outside_any_reservation_stays_fatal`).

**Deferred:** the xzone segment-allocator wall (its own milestone); un-ignoring `hello_dyn_e2e` green
(the guest still doesn't reach `main → write → exit`); partial-reservation munmap splitting and
reservation-aware `range_is_free`/ANYWHERE placement (no walk has forced them); an arm64e guest. See
`docs/superpowers/specs/2026-07-14-retrace-m2-mmapcommit-design.md`.

## Status: M2-carveout — Reservation Holes & Kernel-Faithful ANYWHERE Placement ✅

**The xzone "NULL segment pointer" wall M2-mmapcommit re-parked at was a placement gap, and it
falls with two pieces of the guarded-metadata protocol.** libmalloc protects its zone metadata with a
**guarded range**: it `mach_vm_map`s a ~5 MiB **PROT_NONE reservation**, `mach_vm_deallocate`s a
**1 MiB carveout hole** at an entropy-derived offset inside it, then commits the metadata with
`mach_vm_map(VM_FLAGS_ANYWHERE, address = reservation_base_as_hint, RW)`. On a real kernel the band
around the hole is occupied, so the ANYWHERE-with-hint search is **forced into the carveout hole** —
the metadata legitimately lands mid-reservation, flanked by PROT_NONE guard pages. retrace modeled
neither step: `mach_vm_deallocate` was a **no-op** on reservations (the hole never existed) and
ANYWHERE placement consulted only backings (so the hinted commit landed at the raw reservation base).
The metadata block then straddled pages retrace only demand-zeroed, and xzone read a **NULL back-
pointer** (`sg->xzsg_main_ref == 0`) out of its own "committed" metadata and dereferenced it — fatal.

**The fix (below the trace, mirrored structurally).** Two changes, both in the shared `Box_` VM code
so record and replay recompute identical addresses (the replay oracle byte-compares the returned
address — asymmetry surfaces as divergence, not corruption):

- **`subtract_reservations` on deallocate.** `guest_munmap` now punches `[addr, addr+len)` out of
  every overlapping reservation (GRANULE-aligned): full cover removes the entry, head/tail overlap
  trims it, a **strictly-interior punch splits it into two** — the carveout. The hole becomes
  genuinely free-and-unreserved: `commit_reserved_page` no longer materializes it (a touch there is
  fatal again, matching deallocated address space), and placement stops treating it as occupied.
- **Kernel-faithful hint-forward first-fit.** `range_is_free` additionally excludes reservations (a
  real `vm_map_entry` occupies its VA, so ANYWHERE can never land inside one), and `guest_vm_map`'s
  ANYWHERE branch searches forward from a non-zero hint via `first_fit` — a deterministic sorted
  gap-edge walk. A free hint returns verbatim (the common case, unchanged); a hint colliding with a
  reservation is pushed to the first free gap. With the hole modeled, the guarded commit's
  `hint = reservation_base` lands **exactly in the carveout hole**, reproducing the kernel's forced
  placement. Verified empirically: the commit's hint = reservation base first-fits to the hole base,
  identically to hardware. (`nano`'s band is reserved and committed **FIXED**, never ANYWHERE-with-hint
  — confirmed from libmalloc source + the trace — so the FIXED path is untouched and nano is
  preserved.)

**The NULL deref is gone.** With the metadata block landed at the carveout hole base, its segment
group's back-pointers resolve — the prior `ldrb [x8,#0x178]`, `x8 = 0` fault no longer occurs.

**Honestly blocked — at the libmalloc xzone SEGMENT-GROUP *indexing* wall (a new, distinct boundary).**
The run advances into `xzm_segment_group_alloc_chunk+0x1c4`, which faults **UNMAPPED** (data abort
EC=0x24 FSC=0x7) accessing `sg+4` (the segment-group lock) of `sg = &main->xzmz_segment_groups[
sg_index]`. Verified across **three traced runs** (fault addresses shift per run because the carveout
offset is entropy-derived): the main-zone block's own **`xzmz_total_size` field is `0x3e000`** and the
box committed **exactly that** (the `mach_vm_map` size *is* `0x3e000`, fully backed) — yet xzone
derives `sg` at `main + ~0x4e4c8`, **~`0x104c8` past the block the guest itself sized**. The offset
**varies run-to-run** (`0x4e4c8`/`0x4e740`) — the fingerprint of an *index*-derived address, not a
fixed struct offset. `sg_index = segment_group_front_count · clusterid + sg_front_index`, where
`clusterid`/front come from `_os_cpu_number()`/`_os_cpu_cluster_number()` **at alloc time** while
`segment_group_count` (which sizes `total_size`) is computed **at zone-init** from the commpage CPU
topology. retrace stages a **frozen copy of the host commpage (12 logical CPUs / 2 perflevel
clusters)**, so the guest lays out per-CPU/cluster segment-group metadata for a 12-CPU host but runs on
a **single vCPU** — the per-CPU segment-group index overshoots the block. This is an **xzone
per-CPU/cluster segment-group subsystem, distinct from carveout placement (now correct) and from
demand-commit (M2-mmapcommit's job) — deferred**, not walked into. A documented escape hatch exists
(`_COMM_PAGE_DEV_FIRM` + `MallocAllowInternalSecurity=1` + `MallocSecureAllocator=0` disables xzone
entirely); a principled single-vCPU commpage-topology model is the deeper fix. Determinism note:
within a record/replay pair the CPU/index reads are reproduced from the trace, so record and replay
stay in lockstep — only the wall's exact fault address varies **across** record runs. The gate
(`hello_dyn_e2e`) stays `#[ignore]`d, re-parked with the verified anatomy above.

**What runs today:** everything through M2-mmapcommit, plus reservation hole-punching and
kernel-faithful ANYWHERE placement — `just gate` reports **68 passed, 0 failed, 1 ignored**, clippy
clean. The carveout protocol is proven end-to-end by the `carveout` box units (interior-punch split,
head/tail trim, full-cover removal, hinted-ANYWHERE-into-the-hole, hole-touch-fatal, nano FIXED
regression guard) and the `carveout_e2e` guest (reserve → punch → hinted commit → sentinel round-trip,
byte-identical replay); existing tests (`reservecommit`, `machmsg`/nano, `wildstore`) stay green.

**Deferred:** the xzone per-CPU/cluster segment-group indexing wall (its own milestone or the envp
escape hatch); un-ignoring `hello_dyn_e2e` green (the guest still doesn't reach `main → write →
exit`); `VM_FLAGS_OVERWRITE` modeling and PROT_NONE guard-fault semantics (out of scope); reservation
merging (not observed); an arm64e guest. See
`docs/superpowers/specs/2026-07-14-retrace-m2-carveout-design.md`.
