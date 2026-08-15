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

## Status: M2-cpuid — Guest CPU/Cluster Identity (TPIDR_EL0) ✅

**The xzone segment-group indexing wall M2-carveout re-parked at was a ONE-VALUE bug, not the
"per-CPU/cluster commpage-topology subsystem" that walk guessed.** Live lldb disassembly of the macOS
26 arm64e shared cache settled it: `_os_cpu_number() = TPIDR_EL0 & 0xFFF` and
`_os_cpu_cluster_number() = (uint32_t)TPIDR_EL0 >> 12` (verified inside
`_xzm_xzone_find_and_malloc_from_freelist_chunk`). retrace had set the guest **`TPIDR_EL0 = TSD_IPA =
0x30000`**, conflating it with the thread-self pointer — but `TPIDRRO_EL0` (untouched) is the real TSD
base; `TPIDR_EL0` carries the cpu/cluster id. So the guest's cpu number read as `0x30000 & 0xFFF = 0`
(accidentally correct) while its **cluster number read as `0x30000 >> 12 = 48`** — garbage, there is no
cluster 48. xzone's `sg_index = segment_group_front_count · clusterid + sg_front_index` overshot ~253
slots (`main + ~0x4e4c8`), past the `0x3e000` main-zone metadata block, and faulted **UNMAPPED** on the
segment-group lock. Deterministic-but-wrong: the "per-run variance" the M2-carveout walk saw
(`0x4e4c8` vs `0x4e740`, delta = one `sizeof(xzm_segment_group_s)`) was forwarded-entropy drift in the
pre-fault `gettimeofday` spin, not the index — the register-derived overshoot itself was fixed.

**The fix — one value.** Set the guest **`TPIDR_EL0 = 0`** (a single-vCPU guest is always cpu 0 /
cluster 0) at both constructor sites (`load_dynamic` and `restore`), leaving `TPIDRRO_EL0 = TSD_IPA`
alone. It is written **below the trace and is identical on record and replay** — a fixed constant, like
the PAC keys and the synthetic timebase — so nothing enters the trace and no `retrace-core`/trace-format
change is needed. With cpu and cluster both reading 0, every per-CPU/per-cluster index is in bounds.

**The xzone fault is gone.** The bounded traced `record-dyn hello_dyn` walk advances from ~205 to
**~218 traps**, past `_xzm_segment_group_alloc_chunk`, with **no earlier fault** — confirming nothing
the guest exercises dereferences `TPIDR_EL0` as a TSD base (that role is `TPIDRRO_EL0`'s). The
`cpuid` box unit proves the box presents guest `TPIDR_EL0 == 0` on both the dynamic and replay paths.

**Honestly blocked — at an unhandled Mach task-port MIG message (mach-IPC lineage, distinct from CPU
identity).** At ~218 traps the run hits `RECORD ERROR: unsupported mach_msg2 at pc 0x1804abc34:
msgh_id 3409 dest 0x203 (guest task port Some(515)) send_size 36` — a `mach_msg2` (trap -47) to the
**guest task port**. **msgh_id 3409 is the Mach `task` subsystem (MIG base 3400), routine index 9 =
`task_get_special_port`** (`task.defs` slot 9, macOS 26.4 SDK): the 36-byte request is `header(24) +
NDR(8) + which_port:int(4)`, and `which_port = 4 = TASK_BOOTSTRAP_PORT` — libSystem fetching its
bootstrap port. It is **Unsupported** because retrace's MIG router (`retrace-core::machmsg::route`) has
**no handler for this task-subsystem id**: it services `4811` (`_kernelrpc_mach_vm_map`), stubs `4822`
(`vm_reclaim`) / `8000`-`8001` (`task_restartable`), forwards the read-only allowlist `{200, 206,
3418}`, and **fails loud** on every other id to the task port. Servicing one more MIG id is
**M2-mach-lineage work — a distinct next milestone**, not walked into here beyond re-parking the gate.
The gate (`hello_dyn_e2e`) stays `#[ignore]`d and re-parked at this wall. (The exact trap count and
ports vary run-to-run because `getentropy`/PID are forwarded and recorded per-trace — normal
record/replay, enforced in lockstep by the divergence oracle, not a determinism defect.)

**Known debt (deferred hygiene, not fatal, not a determinism bug):** retrace still `memcpy`s the entire
**host** commpage into the guest, so `_COMM_PAGE_*` CPU/cluster **count** fields carry the host's
12-CPU / 2-cluster values. This is a latent host-topology leak, but harmless once the *index* is pinned
to 0 by this fix — the bytes are frozen once at setup, so a record/replay pair sees identical bytes, and
the oversized per-CPU arrays are never indexed past slot 0. A principled single-vCPU commpage synthesis
(counts = 1, pinned `MEMORY_SIZE`, `DEV_FIRM` policy) is the hygiene follow-up; deferred to keep this
milestone's fix isolated.

**What runs today:** everything through M2-carveout, plus a correct guest CPU/cluster identity —
`just gate` reports **69 passed, 0 failed, 1 ignored**, clippy clean. The headline `hello_dyn_e2e`
gate is still **red** (`#[ignore]`d): the guest does not yet reach `main → write → exit`. See
`docs/superpowers/specs/2026-07-14-retrace-m2-cpuid-design.md`.

**Deferred:** the `task_get_special_port` (msgh_id 3409) task-port MIG wall (M2-mach lineage — route
and service the task special-port surface); the single-vCPU commpage-topology synthesis (host-topology
leak hygiene); un-ignoring `hello_dyn_e2e` green; an arm64e guest.

## Status: M2-bootstrap — `task_get_special_port(BOOTSTRAP)` Servicing ✅ (walk re-parks at the XPC pipe)

**Root cause of the M2-cpuid wall.** libxpc's image initializer (`_libxpc_initializer`, run inside
`libSystem_initializer` at process launch, before `main`) calls
`task_get_special_port(TASK_BOOTSTRAP_PORT)` — a `mach_msg2` (trap -47) to the guest task port with
**msgh_id 3409** (Mach `task` subsystem base 3400, routine 9). retrace's MIG router had no handler for
that id and failed loud. The 36-byte request is `header(24) + NDR(8) + which_port:int(4)` with
`which_port = 4 = TASK_BOOTSTRAP_PORT`.

**The fix — a synthetic-port complex MIG reply.** `machmsg::route` now maps `dest == guest_task_port &&
msgh_id == 3409` to `ServiceGetSpecialPort`; the mirrored record/replay dispatch decodes `which`
(asserting `== 4`, fail-loud on any other special port) and synthesizes the 48-byte **complex** reply
(`MACH_MSGH_BITS_COMPLEX` set, one `mach_msg_port_descriptor_t`, disposition `MOVE_SEND`) carrying a
**fixed synthetic bootstrap-port name `SYNTHETIC_BOOTSTRAP_PORT = 0x0BAD_0B03`**. The reply is a pure
function of `(reply_port, name)` — both deterministic — built identically on record and replay, and the
divergence oracle byte-compares the recomputed reply, so nothing nondeterministic enters the trace. The
synthetic name is a fixed constant, chosen distinct from every port name the run uses, and is **never
forwarded** (forwarding 3409 would hand the guest the host's real launchd bootstrap port).

**The walk — libxpc accepts the reply, then the "dormant" hypothesis is falsified.** The bounded traced
`record-dyn hello_dyn` advances from ~218 to **~228 traps**. libxpc's initializer **accepts** the reply:
`__MIG_check__Reply__task_get_special_port` passes, it extracts `0x0BAD_0B03`, retains its send right
(three deterministically-forwarded `_kernelrpc_mach_port_mod_refs_trap` calls), and passes it to
`xpc_pipe_create_from_port`. But the design spec's **"fetch-and-cache, dormant" scope guess is
empirically wrong for this binary — libxpc's initializer is not lazy.** No `mach_msg2` ever targets
`0x0BAD_0B03` (grep-confirmed: no `bootstrap_look_up` send), and the synthetic name is collision-free
(it appears only as the `name` argument of the three bootstrap-caching `mod_refs`, never as a
differently-sourced forwarded name).

**Honestly blocked — at the XPC bootstrap-PIPE subsystem (distinct from the now-serviced MIG).** At
~228 traps the run aborts in `libxpc.dylib`_xpc_create_bootstrap_pipe.cold.1` with `brk #0x1`
(`EC=0x3c`) at guest `pc 0x180201190`, crash string **"Bug in libxpc: Could not create pipe to
bootstrap server!"**, called from `_libxpc_initializer+0x42c ← libSystem_initializer+0x100` (all
symbolicated live against the arm64e shared cache with the runtime slide backed out). The hot path:
after the send-right retain, `xpc_pipe_create_from_port(bootstrap_port = 0x0BAD_0B03, flags = 4)`
returns **NULL** — a real Mach dispatch channel to launchd cannot be stood up over the synthetic token —
so `cbz x0` takes the cold `__builtin_trap`. This is **not** a reply-format bug: the trap is downstream
of `__MIG_check__Reply`, and `0x0BAD_0B03` flows through cleanly, proving the complex reply decoded
correctly. Servicing this means standing up the **XPC pipe / dispatch-mach channel** subsystem against a
real bootstrap port — a distinct new milestone, explicitly **deferred** (do not pre-stub launchd/XPC).
The gate (`hello_dyn_e2e`) stays `#[ignore]`d and re-parked at this wall.

**What runs today:** everything through M2-cpuid, plus a serviced `task_get_special_port(BOOTSTRAP)` —
`just gate` reports **73 passed, 0 failed, 1 ignored**, clippy clean. The headline `hello_dyn_e2e` gate
is still **red** (`#[ignore]`d): the guest reaches libxpc's XPC-pipe construction but not yet
`main → write → exit`. See `docs/superpowers/specs/2026-07-15-retrace-m2-bootstrap-design.md`.

**Deferred:** the XPC bootstrap-pipe / dispatch-mach channel subsystem (`xpc_pipe_create_from_port` over
a real bootstrap port — a launchd/XPC front door); the single-vCPU commpage-topology synthesis
(host-topology leak hygiene); un-ignoring `hello_dyn_e2e` green; an arm64e guest.

## Status: M2-xpcport — Real Bootstrap Send Right ✅ (walk re-parks at libsystem_trace)

**Root cause of the M2-bootstrap wall.** M2-bootstrap handed libxpc a *synthetic* bootstrap-port name
(`SYNTHETIC_BOOTSTRAP_PORT = 0x0BAD_0B03`), and its initializer aborted (`brk #0x1`) because
`xpc_pipe_create_from_port(0x0BAD_0B03, 4)` returned NULL. M2-bootstrap guessed that clearing this meant
standing up a whole XPC / dispatch-mach channel to launchd. That guess was **wrong**. Task 1 root-caused
the abort: `xpc_pipe_create_from_port` with `name == NULL` does not send-and-wait during construction —
its *only* port-validity dependency is one **local** `mach_port_mod_refs(mach_task_self(), name, SEND,
+1)` retain. On the synthetic name that retain returns `KERN_INVALID_NAME`, so the pipe is NULL. The pipe
never needed a live channel to launchd — only a **genuinely valid send right**.

**The fix — mint a real kernel-valid send right.** retrace *is* the process that hosts the guest, and the
guest's Mach traps are forwarded and executed against retrace's own task, so a port name minted in
retrace's IPC space is valid for the guest's forwarded `mach_port_mod_refs` on that same name.
`Box_::mint_bootstrap_port` mints one (`mach_port_construct` with `MPO_INSERT_SEND_RIGHT` — a receive
right plus an inserted send right — in retrace's own space), caches the name, and the `ServiceGetSpecialPort`
arm hands *its* name back (observed `0x1003`) instead of the synthetic constant. A box unit test proves the
premise: `mach_port_mod_refs(SEND, +1)` on the minted name returns `KERN_SUCCESS` (the exact call that
returned `KERN_INVALID_NAME` on `0x0BAD_0B03`), and the mint is idempotent.

**The determinism-posture flip — synthesize-and-byte-compare → forward-and-record.** M2-bootstrap's reply
was a pure function of `(reply_port, fixed constant)`, so replay recomputed it and byte-compared — that
comparison *was* the divergence oracle for the handler. A **real minted name is nondeterministic** (the
kernel picks it; it varies per record run, exactly like `task_self`'s name), so replay **cannot** recompute
it. The handler therefore moves to the same posture used for every real host port name in the trace:
**record** mints the port and records the reply bytes; **replay** applies the recorded reply **verbatim**
(no recompute, no byte-compare). Divergence protection is not lost — it moves downstream: replay applies
the recorded reply, the guest reads the exact recorded name, and its subsequent `mach_port_mod_refs(name,
…)` traps carry args identical to the recording, which the normal syscall `(num, args)` oracle checks. The
only nondeterministic value is the name, recorded once and replayed — the established `task_self` guarantee.

**The walk — the pipe wall falls, re-parked at the os_trace initializer.** The bounded traced `record-dyn
hello_dyn` advances from ~228 to **~242 traps**. libxpc's three retains (`mach_port_mod_refs(SEND, +1)`,
trap -19, name `0x1003`) now return `KERN_SUCCESS`, `xpc_pipe_create_from_port` returns non-NULL, the
`brk #0x1` in `_xpc_create_bootstrap_pipe.cold.1` (pc `0x180201190`) is **gone**, and `_libxpc_initializer`
completes.

**Honestly blocked — at `task_set_special_port(TASK_DEBUG_CONTROL_PORT)` from libsystem_trace (a distinct,
small init MIG).** At ~242 traps the run fail-louds: `RECORD ERROR: unsupported mach_msg2 at pc
0x1804abc34: msgh_id 3410 dest 0x203 (guest task port Some(515)) send_size 52`. This is **not** a CPU fault
(no ESR/EC) — it is retrace's MIG router rejecting an unhandled id. **msgh_id 3410** = `task_set_special_port`
(Mach `task` subsystem base 3400, routine 10): a **complex** message (`msgh_bits 0x80001513`) carrying one
`COPY_SEND` port descriptor (name `0x1103`) with `which_port = 10 = TASK_DEBUG_CONTROL_PORT`, reply port
`0x1603` (`MAKE_SEND_ONCE`). Symbolicated live against the arm64e shared cache (the box loads at slide 0, so
trace pcs are unslid VAs; ASLR slide backed out via lldb), the caller is **not** libxpc — the `0x1802xxxxx`
range is shared with libsystem_trace — but `libsystem_trace.dylib`_os_trace_create_debug_control_port+0x60`
← `_libtrace_init+0xfc` ← `libSystem.B.dylib`libSystem_initializer+0x10c` ← dyld's
`findAndRunAllInitializers`. So this is the **os_log/os_trace image initializer installing its task
debug-control port** — a sibling `libSystem` sub-initializer that runs just after `_libxpc_initializer`
(`libSystem_initializer+0x100` called libxpc; `+0x10c` calls libtrace), which is exactly why widening past
the pipe brk surfaced it. It is a **small** next-init MIG step, the same task-subsystem lineage as the
serviced 3409 `task_get_special_port` and the stubbed `vm_reclaim` / `task_restartable`: service it by
accepting the complex request, handling the inbound debug-control-port descriptor, and synthesizing a
`__Reply__task_set_special_port_t` that returns `KERN_SUCCESS`, mirrored record/replay. It is **not** the
deferred XPC send / dispatch-mach subsystem — no `mach_msg2` targets the minted bootstrap port (`0x1003`),
and no `bootstrap_look_up` has appeared. Deferred to the next milestone; **do not pre-stub**.

**What runs today:** everything through M2-bootstrap, plus a **real minted bootstrap send right** that
carries the guest past libxpc's XPC-pipe construction — `just gate` reports **74 passed, 0 failed, 1
ignored**, clippy clean. The headline `hello_dyn_e2e` gate is still **red** (`#[ignore]`d): the guest now
clears `_libxpc_initializer` and reaches libsystem_trace's `_libtrace_init`, but not yet
`main → write → exit`. See `docs/superpowers/specs/2026-07-15-retrace-m2-xpcport-design.md`.

**Deferred:** `task_set_special_port(TASK_DEBUG_CONTROL_PORT)` servicing (the next small init MIG, in the
serviced-3409 lineage); the XPC send / dispatch-mach subsystem proper (a real `bootstrap_look_up` round-trip
— still unseen); the single-vCPU commpage-topology synthesis (host-topology leak hygiene); un-ignoring
`hello_dyn_e2e` green; an arm64e guest.

## Status: M2-setport — `task_set_special_port(DEBUG_CONTROL_PORT)` ✅ (walk re-parks at libsystem_secinit)

**Root cause of the M2-xpcport wall.** After the XPC-pipe wall fell, `_libxpc_initializer` completed and
`libSystem_initializer` ran its next sub-initializer, libsystem_trace's `_libtrace_init`. That initializer's
`_os_trace_create_debug_control_port` sends `task_set_special_port(TASK_DEBUG_CONTROL_PORT)` (msgh_id
**3410**) to the guest task port — a **complex** message (`msgh_bits 0x80001513`) carrying one `COPY_SEND`
port descriptor (name `0x1103`) with `which_port = 10`, reply port `0x1603` (`MAKE_SEND_ONCE`). retrace's
MIG router had no handler, so it fail-louded.

**The fix — a deterministic `mig_reply_error` KERN_SUCCESS.** The reply MIG stub expects on the
send-once reply port, `__Reply__task_set_special_port_t`, is byte-identical to a `mig_reply_error_t`: a
non-complex 36-byte message with reply id `3410 + 100 = 3510` and `RetCode = KERN_SUCCESS`. So the
`Route::ServiceSetSpecialPort` arm decodes the complex request, asserts `which_port == 10`
(`TASK_DEBUG_CONTROL_PORT`), and emits `machmsg::encode_mig_error(3410, reply_port, KERN_SUCCESS)`. The
request's inbound `COPY_SEND` port descriptor is decoded but **deliberately dropped** — it is *never*
forwarded, because forwarding a real `task_set_special_port` would install retrace's **own** debug-control
port. A single-vCPU deterministic replay has no debugger to attach, so acknowledging success and discarding
the port is both correct and side-effect-free.

**The STANDARD symmetric posture (not M2-xpcport's special case).** Unlike the bootstrap send right — whose
kernel-minted name is nondeterministic, forcing the forward-and-record / apply-verbatim posture — this reply
is a **pure function of `(msgh_id, reply_port, KERN_SUCCESS)`**. So the handler uses the ordinary symmetric
rule: **record** synthesizes the reply and appends it; **replay** *recomputes* the identical reply and
**byte-compares** it against the recording. That byte-compare *is* the divergence oracle for the handler —
an asymmetry would surface as a divergence, not silent corruption. (This is the posture of `ServiceVmMap` /
`StubMigReply`, deliberately *not* the verbatim-apply of `ServiceGetSpecialPort`.)

**The walk — the 3410 wall falls, re-parked at libsystem_secinit's sandbox check.** The bounded traced
`record-dyn hello_dyn` now services msgh_id 3410 (no `RECORD ERROR`), `_os_trace_create_debug_control_port`
accepts the reply, `_libtrace_init` completes, and the run advances one MIG call further (**~241–242 traps**,
the count within forwarded-entropy noise).

**Honestly blocked — at `task_info(TASK_AUDIT_TOKEN)` from libsystem_secinit (a distinct, small init MIG).**
At ~241 traps the run fail-louds: `RECORD ERROR: unsupported mach_msg2 at pc 0x1804abc34: msgh_id 3405 dest
0x203 (guest task port Some(515)) send_size 40`. Again **not** a CPU fault (no ESR/EC) — retrace's MIG router
rejecting an unhandled id. **msgh_id 3405** = `task_info` (Mach `task` subsystem base 3400, routine 5): a
**simple** message (`bits 0x1513`), 40 bytes = `header(24) + NDR(8) + flavor:int(4) + task_info_outCnt:int(4)`,
with `flavor = 15 = TASK_AUDIT_TOKEN` and `count = 8 = TASK_AUDIT_TOKEN_COUNT` (an `audit_token_t` is 8
words), reply port `0x1603` (`MAKE_SEND_ONCE`). Symbolicated against the arm64e shared cache (the box loads
at slide 0, so trace pcs are unslid VAs, resolved statically in lldb): the caller is **not** libsystem_trace
(that was the fallen 3410) but **libsystem_secinit's app-sandbox check** —
`libsystem_kernel.dylib`task_info+224` ← `libxpc.dylib`_fetch_self_token+60` ← (via `dispatch_once`)
`libxpc.dylib`_xpc_get_self_audit_token+144` ← `libxpc.dylib`xpc_copy_entitlements_for_self+20` ←
`libsystem_secinit.dylib`_libsecinit_appsandbox_check+72` ← `_libsecinit_initializer+160` ←
`libSystem.B.dylib`libSystem_initializer+0x118` ← dyld's `findAndRunAllInitializers`. So this is the
**sandbox-init image initializer fetching the process's own audit token** (process identity) — the sibling
`libSystem` sub-initializer that runs right after libtrace (`libSystem_initializer+0x10c` ran libtrace / 3410;
`+0x118` runs libsecinit), which is exactly why widening past the 3410 wall surfaced it. It is a **small**
next-init MIG step, the same task-subsystem lineage as the serviced 3409 `task_get_special_port` and 3410
`task_set_special_port`: service it by synthesizing a `__Reply__task_info_t` carrying an `audit_token_t` (8
words). Because the audit token holds host process identity (`pid`/`asid`/`pidversion` vary run-to-run), the
reply is **nondeterministic** — so this likely wants the **forward-and-record** posture (record forwards the
real `task_info` and records the reply; replay applies it verbatim), like `task_self`'s port name and
`getentropy`, **not** synthesize-and-byte-compare. Note the caller is libsecinit's **sandbox** check (via
`xpc_copy_entitlements_for_self`), so servicing `task_info` may surface a further libsecinit step (an
entitlement / sandbox query) once the token flows — to be discovered, **not** pre-stubbed. It is **not** the
deferred XPC send / dispatch-mach subsystem — dest is the guest task port (`0x203`), no `mach_msg2` targets
the minted bootstrap port (`0x1003`), and no `bootstrap_look_up` has appeared. Deferred to the next
milestone; **do not pre-stub**.

**What runs today:** everything through M2-xpcport, plus serviced `task_set_special_port(DEBUG_CONTROL_PORT)`
that carries the guest past libsystem_trace's debug-control-port install into libsystem_secinit's sandbox
initializer — `just gate` reports **77 passed, 0 failed, 1 ignored**, clippy clean. The headline
`hello_dyn_e2e` gate is still **red** (`#[ignore]`d): the guest now clears `_libtrace_init` and reaches
libsystem_secinit's `_libsecinit_appsandbox_check`, but not yet `main → write → exit`. See
`docs/superpowers/specs/2026-07-15-retrace-m2-setport-design.md`.

**Deferred:** `task_info(TASK_AUDIT_TOKEN)` servicing (the next small init MIG, in the serviced-3409/3410
lineage — likely forward-and-record for the nondeterministic audit token); whatever libsecinit's sandbox
check does after the token (an entitlement / sandbox query, still unseen); the XPC send / dispatch-mach
subsystem proper (a real `bootstrap_look_up` round-trip — still unseen); the single-vCPU commpage-topology
synthesis (host-topology leak hygiene); un-ignoring `hello_dyn_e2e` green; an arm64e guest.

## Status: M2-taskinfo — `task_info(TASK_AUDIT_TOKEN)` forwarded ✅ (the M2 headline gate is GREEN)

**Root cause of the M2-setport wall.** After the `task_set_special_port(DEBUG_CONTROL_PORT)` wall fell,
`libSystem_initializer` ran its next sub-initializer, libsystem_secinit's `_libsecinit_initializer`. Its
`_libsecinit_appsandbox_check` calls `xpc_copy_entitlements_for_self`, which — through libxpc's
`_xpc_get_self_audit_token` / `_fetch_self_token` (a `dispatch_once`) — sends `task_info(TASK_AUDIT_TOKEN)`
(msgh_id **3405**) to the guest task port to fetch the process's **own audit token** (its identity). This is a
**simple** message (`bits 0x1513`), 40 bytes = `header(24) + NDR(8) + flavor:int(4) + count:int(4)`, with
`flavor = 15 = TASK_AUDIT_TOKEN` and `count = 8 = TASK_AUDIT_TOKEN_COUNT` (an `audit_token_t` is 8 words),
reply port `0x1603` (`MAKE_SEND_ONCE`). retrace's MIG router had no handler, so it fail-louded.

**The fix — one `FORWARD_ALLOWLIST` entry (forward, don't synthesize).** Unlike 3409/3410, this reply is *not*
computed in the box: msgh_id **3405** is added to `machmsg`'s `FORWARD_ALLOWLIST`, joining the read-only
allowlist (`host_info` 200, `host_get_clock_service` 206, `semaphore_create` 3418). The existing `Forward`
route does the rest — **record** issues the *real* `task_info` trap against retrace's **own** task and captures
what the kernel wrote back with `forward_and_diff` (the audit-token reply bytes land in the trace as ordinary
recorded memory writes); **replay** never issues the trap — it applies the recorded writes verbatim. No
decoder, no dispatch arm, no synthesized reply: the whole functional change is the single allowlist entry.

**Why forward-and-record here, and why forwarding is safe (contrast 3409 / 3410).** The audit token embeds
**host process identity** (`pid` / `asid` / `pidversion`), which varies run-to-run, so the reply is
**nondeterministic** — it cannot be regenerated and byte-compared. Forwarding-and-recording is exactly the
posture already used for `task_self`'s kernel-picked port name and for `getentropy`: record the real bytes,
replay them verbatim (no recompute, no divergence byte-compare). Forwarding is **safe** here precisely because
`task_info(TASK_AUDIT_TOKEN)` returns **read-only out-of-line data with no port rights** — issuing it against
retrace's own task leaks nothing into the guest's IPC space. That is the opposite of 3409
`task_get_special_port` and 3410 `task_set_special_port`, which carry **port descriptors**: those had to be
minted/synthesized in the box to keep the guest's port namespace coherent (a forwarded real special port would
be retrace's, not the guest's). Read-only data → forward; port rights → synthesize.

**The walk — the LAST wall falls; `hello_dyn` runs to completion.** The bounded traced `record-dyn hello_dyn`
now forwards msgh_id 3405 (`[retrace] forwarding mach_msg2 task_info (msgh_id 3405) to host (decided
allowlist)`; no `RECORD ERROR`). libsystem_secinit's sandbox check proceeds from the forwarded token alone —
the further entitlement / sandbox query the M2-setport re-park warned *might* surface **did not appear** — so
`_libsecinit_initializer` completes, dyld's `findAndRunAllInitializers` returns, control reaches the program's
`main`, and hello_dyn runs to the end: `write(1, "hi\n", 3)` (trap 4) then `exit(0)` (trap 1). Record produces
exit 0 / stdout `"hi\n"`; **replay is byte-identical** (exit 0 / `"hi\n"`, empty stderr, zero divergence),
verified twice (double-replay). The headline `hello_dyn_e2e` gate is **un-`#[ignore]`d** and now runs green in
the default suite — a dynamically-linked C program records and replays bit-for-bit, dyld having mapped and
re-signed the shared cache itself.

**What runs today:** the full M2 headline path — `record-dyn hello_dyn` links against real `/usr/lib/dyld`,
demand-pages and re-signs the arm64e shared cache, runs every libSystem image initializer (libmalloc, libobjc,
libxpc, libtrace, libsecinit) through the serviced mach-IPC / MIG surface, reaches `main`, and records +
replays `write(1,"hi\n")` + `exit(0)` **byte-for-byte with zero divergence**. `just gate` reports **78 passed,
0 failed, 0 ignored**, clippy clean — the headline gate is GREEN, no longer parked. See
`docs/superpowers/specs/2026-07-15-retrace-m2-taskinfo-design.md`.

**Deferred:** the single-vCPU commpage-topology synthesis (the frozen host commpage still carries
12-CPU/2-cluster counts — harmless now the cpu/cluster index is pinned to 0, but a hygiene follow-up); the XPC
send / dispatch-mach subsystem proper (never exercised on this path — no `mach_msg2` targets the minted
bootstrap port `0x1003`, no `bootstrap_look_up` appears); larger / longer-running guests and an arm64e guest
(hello_dyn is a plain-arm64 program); broadening the record/replay surface beyond this single e2e program.

## Status: M3 — Reverse Execution ✅ (the M3 headline gate is GREEN)

**The idea — time is a coordinate; backward is forward.** A moment in a recorded run is named by
**P = (landmark N, step K)**: the machine state after the first `N` trace events have been consumed and `K`
further instructions have retired. `N` is exactly the event index replay already tracks (`idx`, the number
`Divergence.landmark` reports); `K` counts instructions inside landmark `N`'s window. Replay of a given trace is
bit-exact, so **P is total and deterministic — seeking the same P twice yields byte-identical machine state.**
That is M3's oracle, the direct extension of the divergence oracle. Nothing ever executes backward: every
reverse operation computes an *earlier* coordinate and re-seeks forward to it from the snapshot.

**The engine — re-replay + hardware single-step, no checkpoints.** `seek(N, K)` = restore snapshot → replay `N`
events at native speed (the divergence oracle verifies every trap on the way) → single-step `K` instructions.
`reverse-stepi` from (N, K) is `seek(N, K−1)`; at K = 0 it is `seek(N−1, len(window N−1))`, the window length
found by one forward counting pass. `reverse-continue` is one forward scan recording every breakpoint hit, then
a seek to the last hit strictly before P (a clean `no earlier hit` if none). Each seek is O(run length) — a full
`hello_dyn` replay is a few hundred landmarks and takes a few seconds, fine at this guest scale; checkpoints are
a pure acceleration deferred until a guest's replay time hurts. A host-side AArch64 interpreter was rejected
(it would reimplement Apple's PAC). Hardware makes the choice unambiguous anyway: **the HVF guest has no PMU
instruction counter (PMUVer = 0)**, so architectural single-step is the only exact tick source on this platform.

**Below the trace, settled by the M3-step spike (F1–F3).** Stepping lives entirely inside `Box_::step()` /
`run()`, invisible to the record/replay loop (symmetry rule 2), so M3 makes **zero trace-format changes** — no
`TRACE_MAGIC` bump, nothing about debugging enters a recording. The spike pinned down how debug exceptions route
on macOS 26 / Apple Silicon:
- **F1 — the step route is DIRECT-EL2.** A software single-step exception is delivered straight to the VMM
  (`ESR_EL2` EC = 0x32), guest still at EL0, PC advanced by exactly one instruction — *not* through the guest's
  `hvc` trampoline. `Box_::step` arms `PSTATE.SS` + `MDSCR_EL1.SS`, classifies one `hv_vcpu_run`, disarms both.
- **F2 — the EL1-parked corollary.** When the stepped instruction itself traps to EL1 (an SVC, or a
  below-the-trace timebase / undef-MRS / FPAC emulation), the step still surfaces as a direct-EL2 exit
  (EC = 0x32) but with the guest now parked at **EL1**, `ESR_EL1` / `ELR_EL1` holding the real trap.
  `run_one_for_step` dispatches off `ESR_EL1` exactly like `run()`: an emulation stands in for the step (counts
  as one), the window-ending SVC is returned unconsumed as `Stop::Syscall`. This corollary is directly visible
  in the gate — a `window_len_here` counting pass steps *through* the window-ending SVC and parks at the EL1
  trampoline (`0x4400`), so its `cur_pc()` is the trampoline, not the SVC; the coordinate `(N, len)` reached by
  `seek` instead parks at EL0 on the SVC. The e2e therefore anchors the round-trip on the `(N, K)` coordinate, not
  a probe's pc.
- **F3 — hardware breakpoints DELIVER.** A `DBGBVR0/DBGBCR0_EL1` instruction breakpoint fires directly to the
  VMM (`ESR_EL2` EC = 0x30, `PC == DBGBVR0`, before the instruction retires). This accelerates `continue` /
  `reverse-continue` mid-window hits (6 hardware slots); a hit that lands exactly on a landmark boundary is
  caught by a landmark-granular check, which also covers the 7th-and-beyond breakpoint at those boundaries.

**The command surface — `retrace debug <trace> --script '…'`.** A `;`-separated, self-echoing script; every
printed byte derives from guest state, the script, or a fixed string (no host pointers, no timing, no map
order), so a transcript is bit-reproducible. Commands: `break <a>` / `delete <a>` (up to 6 hardware slots,
sorted + deduped) · `continue` / `reverse-continue` · `stepi [n]` / `reverse-stepi [n]` · `regs` (the `dbg_regs`
dump) · `x <a> <len>` (hex bytes, or `unmapped`) · `where` (prints `(N, K)` + reg PC). A syntax error aborts the
whole script before any output (exit 5).

**The walk — the M3 headline gate is GREEN.** `reverse_debug_e2e` records a fresh `hello_dyn`, discovers the
`write(1,"hi\n")` landmark **in-process** (`peek_syscall` + `advance` — never a hardcoded address), drops that
session (one VM per process), then spawns `retrace debug --script 'break …; continue; where; regs; reverse-stepi;
where; reverse-stepi; where; stepi; where; reverse-continue; where'` **twice** on the same recording. On the
committed run: `continue` catches the breakpoint at the write-return boundary `(273, 0)` (pc `0x1804af834`);
`reverse-stepi` backs into the write's window `(272, 178)` (pc `0x1804af830`, the write SVC); a second
`reverse-stepi` steps to `(272, 177)`; `stepi` round-trips forward to `(272, 178)`; `reverse-continue` reports
`no earlier hit` (the sole hit `(273, 0)` is later, not before P). The **primary oracle** is that the two
transcripts are **byte-identical**; the coordinate lines are secondary anchors. Un-`#[ignore]`d on a genuine
double pass (two independent runs, each a fresh recording). `just gate` reports **97 passed, 0 failed, 0
ignored**, clippy clean (90 at the M3 close; the fast-follow added 7 debug-CLI golden tests).

**Deferred:** checkpoints (a pure seek-time acceleration — deferred until a guest's replay time hurts);
watchpoints (4 hardware slots exist, unused); symbolication (debugger addresses are raw guest VAs); an
interactive REPL (only scripted sessions today); step-over/`next` (de-scoped — use `stepi`); the
mid-window-vs-boundary K = 0 resolution edge (a boundary breakpoint interacting with the `K > K_cur` rule —
untested, the e2e uses a clean boundary hit); and the `Stop::Other`-while-stepping fault path (empirically
unreachable on `hello_dyn` — correct by construction, untriggered). `break` refuses a 7th breakpoint
(6 DBGBVR slots, loud error); `continue` from atop a breakpoint pre-steps one instruction (untested edge: a
pre-step that lands atop a *second* breakpoint on the adjacent instruction or the next window boundary may
resolve late or error — adjacent breakpoint pairs are deferred). See
`docs/superpowers/specs/2026-07-16-retrace-m3-reverse-execution-design.md`.

## Status: M4 — checkpointed reverse-execution seeks ✅ (the M4 headline gate is GREEN)

**The idea — cache mid-run machine state, keyed by the coordinate that names it.** M3 proved every seek is
`restore snapshot → replay N landmarks at native speed → single-step K instructions`, and left checkpoints as a
deferred pure acceleration. M4 builds them: a **`BoxState`** is a complete mid-run capture of `Box_` — full
guest memory (every backing region), all GPRs plus `PC`/`PSTATE`/`SP_EL0`, `ELR_EL1`/`SPSR_EL1`, `TPIDR_EL0`,
and the internal bookkeeping `restore()` gets wrong mid-run (reservations, the mmap cursor, the bootstrap port,
the cache-pager-installed flag, the last fault address, the synthetic timebase, cache-refault state), **plus**
`V0`–`V31`/`FPCR`/`FPSR`, which `Box_::restore()` never had to touch before now because landmark-0 restore is
always the clean pre-execution state. Fixed EL1 sysregs (`TTBR0_EL1`/`TCR_EL1`/`MAIR_EL1`/…) and the PAC keys
are re-established as constants on restore, exactly like `restore()` does — never captured state, by the
determinism design. A `SessionCheckpoint` pairs a `BoxState` with the coordinate `(N, K)` it
was captured at; a `CheckpointCache` holds a bounded set of them, **cost-gated** (only positions that cost at
least 64 single-steps to reach are worth caching), **byte-budgeted** (256 MiB), and **LRU-evicted** past that
budget. `checkpointed_seek(N, K)` tries, in order: an exact or same-window cache hit (resume from the nearest
checkpoint at or before `(N, K)` and single-step the remainder — no replay), an earlier-landmark checkpoint
(replay forward from there instead of from the snapshot), then falls back to M3's cold seek, which after finishing
inserts a new checkpoint at `(N, K)` if the cost gate says the position was expensive to reach. `retrace debug`'s
`Exec` calls `checkpointed_seek` at every seek site (`stepi`, `reverse-stepi`, `continue`, `reverse-continue`,
`where`) in place of M3's raw `seek`; the cache lives only inside one `debug` process's `ReplaySession` — never
persisted, never touching the trace format. A checkpoint's validity is scoped to one trace and one session by
construction, so persisting it across runs was never on the table.

**Why — single-stepping inside a window, not landmark replay, was the real bottleneck.** M3's own numbers showed
landmark replay runs at native speed; the cost lives entirely in the `K` single-steps taken *inside* a window to
reach a deep coordinate. A `reverse-stepi` that lands the debugger repeatedly near the same deep position inside
a long window (the common case when a user is single-stepping around one spot) was paying that full single-step
cost on every seek. Caching the machine state at that position turns a second nearby seek into a handful of
single-steps instead of thousands.

**The FP/SIMD gap it closed.** Because landmark-0 restore never needed vector state, `BoxState`/`Box_::checkpoint`/
`Box_::from_checkpoint` are the first code in this repo to save and restore `V0`–`V31`, `FPCR`, and `FPSR` for a
running guest — new `hv-sys` wrappers plus the capture/restore plumbing (task 1/2). `from_checkpoint` restores the
same sysreg block `restore()` does, **plus** `set_trap_debug_exceptions(true)` right after — a call easy to drop
when copying `restore()`'s shape, and one whose omission fails silently (checkpoint-resumed stepping simply stops
trapping) rather than loudly. Proven by `checkpointed_seek_matches_cold_across_a_neon_window`: it records
`hello_dyn` through real `/usr/lib/dyld` and exploits dyld's own early init, which uses NEON (memcpy, hashing)
well before any application code runs — `first_window_with_len` probes for a window at least 100 instructions
long, a checkpoint is taken mid-window, and the checkpoint-resumed continuation is byte-compared against a cold
seek to the same coordinate (registers, the FP/SIMD dump, and full memory), with an explicit nonzero-V-regs
assertion so the proof can't silently go vacuous. The `spinloop` guest program (`asm/spinloop.s`) is pure
integer code — two `subs`/`b.ne` counting loops, no vector instructions — and plays a different role entirely:
its two deliberately huge windows (~606 and ~4003 instructions) are what the cache-hit, byte-budget/LRU, and
speedup tests exercise (the 3990→5 numbers below).

**The numbers.** The first seek into `spinloop`'s ~4003-instruction window pays the full 3990 single-steps (the
window is expensive enough to trip the cost gate and get cached); a nearby second seek into the same window pays
5 single-steps from the cached checkpoint — roughly **800x**. Every existing debug-CLI transcript (7 `debug_cli`
golden tests plus `reverse_debug_e2e`) passes unmodified — checkpointing changes *when* state is computed, never
*what* is printed, so the transcripts stay byte-identical with checkpointing wired in.

**The walk — the M4 headline gate is GREEN.** At the M4 close, `just gate` reported **104 passed, 0 failed, 0 ignored**, clippy
clean (97 at the M3 close plus the fast-follow gate; M4 added seven new tests: `fp_and_simd_regs_roundtrip`,
`checkpoint_round_trip_is_lossless_mid_run`, `checkpointed_seek_same_and_earlier_window_hits_match_cold`,
`checkpoint_cache_respects_byte_budget_and_evicts_lru`, `checkpointed_seek_matches_cold_across_a_neon_window`,
`large_window_second_nearby_seek_is_far_cheaper_than_the_first`, and `spinloop_guest_parses`). The M4
fast-follow then added `gate_zero_same_key_reseek_does_not_double_count_bytes` and
`window_len_is_memoized_per_landmark`, taking the gate to **106 passed, 0 failed, 0 ignored**. See
`docs/superpowers/specs/2026-07-16-retrace-m4-checkpoints-design.md`.

**Deferred:** a user-facing config knob for the byte budget / cost-gate threshold (currently compile-time
constants). Persisting checkpoints across sessions — deliberately never: a checkpoint's validity is scoped to one
trace and one session by construction, so there is no cross-session use for one to serve. (Window-length
memoization, deferred at M4 close, landed in the M4 fast-follow: `CheckpointCache::window_len` measures each
window at most once per debug session, so a `reverse-stepi` crossing a landmark boundary into a large window pays
that window's length once, not on every crossing; `window_len_here` itself is unchanged.)

## Status: M5 — write watchpoints & reverse-continue-to-last-writer ✅

**The idea — watch a byte range, not just an address.** M3 gave `retrace debug` instruction breakpoints;
M5 adds `watch <addr> [len]` / `unwatch <addr>`, so `continue` and `reverse-continue` also stop on a
*write* to `[addr, addr+len)` (`len` ∈ {1, 2, 4, 8}, default 8, `addr` naturally aligned to `len` so the
range sits inside one BAS-selectable doubleword). A watched write can land two ways, both surfaced through
the same `continue`/`reverse-continue` scan: an EL0 guest store, caught by the CPU's own hardware
write-watchpoint comparators (`DBGWVR`/`DBGWCR`), and a kernel write delivered as a recorded syscall's
memory diff (e.g. `read()` filling a buffer), caught in software by intersecting each applied write's byte
range against the armed watch ranges. Detection is observation-only in both cases — nothing about *what
executes*, *what is written*, or *what enters the trace* changes; watching can only make a scan stop
sooner.

**The spike — `spikes/dbgw.c` (F4a-F4d), settled before any implementation.** Recorded in
`spikes/README.md`: arming `DBGWVR0_EL1`/`DBGWCR0_EL1` over an 8-byte guest qword (`BAS=0xFF`, store-only,
EL0-only) and running a `str` to it delivers **(F4a)** DIRECTLY to the VMM as an `hv_vcpu_run` exit —
`ESR_EL2` EC=0x34 (watchpoint from a lower EL), never through the guest's `VBAR` trampoline, the same
direct-EL2 shape as M3's single-step (EC=0x32) and HW-breakpoint (EC=0x30) exits. **(F4b)** `FAR` (the
exit's `virtual_address`) holds the *exact* accessed VA, not a page-truncated or offset one. **(F4c)** the
exit is **pre-retire**: at the hit, the watched qword still reads its old value and `PC` is parked *at* the
`str` itself, not past it — disarming and resuming re-executes the store exactly once. **(F4d)** BAS is
byte-selective, confirmed both ways: re-arming with `BAS=0xF0` (bytes 4..7) and running a `strb` to byte 0
does not fire, and the store still executes. All four sub-findings confirmed the M5 design's pre-retire
hypothesis exactly; no spec fallback was needed.

**The hardware path (`retrace-box`).** `hv-sys` exposes the four `DBGWVR0-3_EL1`/`DBGWCR0-3_EL1` comparator
pairs (`HW_WATCHPOINT_SLOTS`, 4 slots on this silicon vs. 6 breakpoint slots). `Box_::arm_hw_watchpoint(slot,
va, len)` sets `DBGWVRn = va & !7` and `DBGWCRn = DBGWCR_BASE | (bas << 5)`, where `DBGWCR_BASE = 0x15`
encodes E=1, PAC=EL0-only, LSC=store-only, and `bas` is the `len`-wide byte mask shifted to `va`'s position
within its doubleword; `clear_hw_watchpoints` disarms all four slots and forgets the watched ranges. Both
breakpoints and watchpoints share a single `MDSCR_EL1.MDE` enable bit, gated by the new `sync_mde` helper
(`MDE` stays set iff *either* `bps_armed` or `wps_armed` is true) — the fix for the sharing bug an
unconditional `MDE` clear in `clear_hw_breakpoints` would otherwise have (caught by a genuine TDD RED before
it ever landed): clearing breakpoints alone would silently disarm any watchpoints armed alongside them. The
regression test `mde_survives_clear_breakpoints_with_watches_armed`
(`crates/retrace/tests/watch.rs`) arms a watch, arms an unrelated breakpoint, clears *only* the breakpoint,
and asserts the watch still fires — proving the shared-register fix rather than just the individual arm/clear
calls. `Box_::run()` surfaces a watchpoint exit as the same generic `Stop::Other { esr }` HW breakpoints
already used; `ReplaySession::advance()` discriminates it by `ESR_EL2` EC ∈ {0x34, 0x35} (`retrace-arch`'s
`Ec::Watchpoint`), *before* the cache-fault/FPAC fallbacks, into `Advance::Watch`.

**The software path (`retrace-box` + `retrace-core`).** `Box_` gains `watch_ranges: Vec<(u64, u64)>` (armed
alongside the hardware slots) and `syscall_watch_hit: Option<(u64, u64)>`. Inside `apply_and_return`'s
per-write loop — replay-side application of a recorded syscall's memory diff, used by both `record_box` (to
keep running after forwarding a real syscall) and `ReplaySession::advance` — each write's IPA range is
intersected against the armed watch ranges *before* the copy runs (first overlap wins; the copy itself is
never skipped or altered, so detection cannot perturb what gets applied). `take_syscall_watch_hit()` lets
`ReplaySession::finish_event` report the event as `Advance::WatchSyscall { watched }` instead of plain
`Advance::Event` once the event is fully consumed — a reviewer traced all 11 of `advance()`'s event-return
sites to confirm every one routes through `finish_event`, so no syscall-driven write can silently bypass
detection. On record and on plain `retrace replay`, `watch_ranges` stays empty and the added check is a
single `is_empty` test — behaviorally invisible.

**The command surface — hit semantics and the `kctx` subtlety.** A hardware hit parks *at* the storing
instruction, before it executes (spike F4c): `hit watch 0x… (write at 0x…) at (N, +?)`, followed by
`resolved (N, K)` once `resolve_hit_k` pins the exact step; a syscall hit parks at the post-event boundary,
`hit watch 0x… (syscall write) at (N, 0)`. Resolving a hardware hit's K reuses M3's `resolve_hit_k`, but
`cmd_continue` searches from `kctx` for a watch hit and from `kctx + 1` for a breakpoint hit — a
breakpoint's pre-step already moved the cursor off a hit it was parked on, but a watched store can
legitimately fire at the exact coordinate the user just `stepi`'d to, and the store's PC can repeat across
loop iterations, so searching from `kctx + 1` would silently skip to the *next* iteration instead of
resolving the current one. A **progress rule** (hardware hits only, mirroring the existing
parked-on-breakpoint pre-step) tracks `last_watch_hit`: if `continue` starts parked exactly on the last
reported hardware hit, it pre-steps one unarmed instruction first, so the still-un-retired store cannot
re-fire forever; syscall hits never set it, since a pre-step off a post-event boundary could skip a
legitimate watched store as the new window's first instruction. `reverse-continue` needs no pre-step: its
scan keeps only hits strictly before P, which already excludes the parked-on store. `reverse-continue`'s
scan (`cmd_reverse_continue`) treats breakpoint, hardware-watch, and syscall-watch hits uniformly as an
`RHit` enum; a `WatchSys` hit resumes the next scan leg at `(n, 0)` (the writing event is already
consumed by the unarmed seek that found it, so it cannot re-fire, but a first-instruction store in window `n`
can still be caught).

**The tests and the numbers.** New: `watchloop_guest_parses` (`crates/retrace-guest/src/lib.rs`) for the new
`asm/watchloop.s` guest (eight same-PC stores to `target`, one byte-0 `strb` to `target2` as the BAS
negative case, then `write(1, target, 8)` to publish the watched address in the trace); five session-level
tests in `crates/retrace/tests/watch.rs` (`hw_watchpoint_fires_on_store_pre_retire_with_far`,
`watch_on_untouched_bytes_never_fires`, `mde_survives_clear_breakpoints_with_watches_armed`,
`syscall_write_to_watched_buf_is_reported_and_replay_completes`, `fstat_statbuf_write_is_detected` — the
last two the first debug-surface use of the pre-existing `FILEIO` guest); six golden-transcript tests in
`crates/retrace/tests/watch_cli.rs` (`watch_continue_hits_first_store_and_progress_rule_advances`,
`watch_validation_is_fail_loud`, `unwatch_disarms`, `reverse_continue_finds_last_store`,
`reverse_continue_with_no_earlier_write_reports_none`, `syscall_writer_is_found_forward_and_backward`); two
parser unit tests in `crates/retrace/src/debug.rs` (`parses_watch_and_unwatch`,
`rejects_bad_watch_len_and_alignment`) — 14 new tests in all. `just gate` climbed from the M4 close's
**106 passed, 0 failed, 0 ignored** through the six M5 tasks: 107 (the spike is not a Rust test; the
`watchloop` guest parser test), 110 (the three hardware/session watch tests), 112 (the two syscall-watch
tests), 117 (the three `watch`/`unwatch`/progress-rule golden-transcript tests plus the two parser unit
tests), to **120 passed, 0 failed, 0 ignored** at the M5 close (the two `reverse-continue` tests plus the
syscall-writer forward/backward test) — clippy clean throughout, every pre-existing golden transcript (7 `debug_cli`,
`reverse_debug_e2e`, `checkpoint_seek`) byte-identical. See
`docs/superpowers/specs/2026-07-18-retrace-m5-watchpoints-design.md`.

**Deferred:** read/access watchpoints (`rwatch`/`awatch` — the hardware's LSC field supports it, but it
doubles the CLI/test surface for a rarer use case); printing the old and new value on a hit (a presentation
nicety, not a capability); watch ranges wider than 8 bytes or crossing a doubleword (would need multi-slot
arming or a software fallback); symbol- or expression-based watch addresses (only raw guest VAs today, same
as breakpoints); watchpoint hits during plain `retrace replay` (the feature is `retrace debug`-only —
`replay` never arms a watch). Also unexercised beyond the M5 test surface: the software syscall-write check
compares an armed *VA* against a recorded write's *IPA*, which is exact only for identity-mapped static
guests (`WATCHLOOP`, `FILEIO` — the entirety of M5's test surface); an MMU-on dynamic guest (e.g. `hello_dyn`)
would need VA-to-IPA translation before the intersection is meaningful, deferred as future work.

The M5 fast-follow closed the final review's M-1: `cmd_continue`'s pre-step now crosses a window boundary
with watches armed, so a syscall write to a watched range in the crossed event is reported rather than
silently skipped (new golden-transcript test `pre_step_boundary_cross_reports_a_watched_syscall_write` in
`crates/retrace/tests/watch_cli.rs`), taking the gate from 120 to **121 passed, 0 failed, 0 ignored**.

## Status: M6 — crash recording & reverse-continue-to-the-bug ✅ (the M6 headline gate is GREEN)

**The idea — a crash is a recorded, replayed, seekable stop, not a retrace error.** Through M5, a guest
synchronous fault (wild pointer, NULL deref, jump to garbage) was indistinguishable from a retrace bug: it
surfaced as the generic `Stop::Other` diagnosis bucket; the dispatch tried the below-the-trace demand paths
(`page_in_cache`, then `commit_reserved_page`), and when both refused, record returned an `Err` carrying
`describe_stop`'s rendering — a class string (`"non-syscall exit: data abort …"` or `"instruction abort"`), the
FAR and whether it's mapped, and `ELR_EL1` — the same bring-up-failure shape a genuine retrace bug takes today.
Nothing about the crash entered the trace, and there was no position "at the crash" to seek to.

M6 gives guest faults a real, deterministic identity: **stage-1 EL0 data/instruction
aborts become `Stop::Fault { pc, esr, far }`** (`retrace-box/src/lib.rs`), recorded as a terminal
`Event::Crash { pc, esr, far }` (`retrace-trace/src/lib.rs`, `TRACE_MAGIC` bumped `0x03 → 0x04`) and
byte-verified on replay — exactly like `Exit`, just a different terminal shape. `RecordSummary` and
`ReplayReport` both gain a shared `Outcome { Exit { code }, Crash { pc, esr, far } }` (`retrace-core/src/lib.rs`)
in place of a bare exit code, and `record` / `record-dyn` / `replay` all print `guest crashed: pc=… far=… esr=…`
and exit **139** (128 + SIGSEGV) on a crash outcome — recording a crash is a *successful recording*, and a
verified crash replay is a *successful replay*, not a failure path.

**The two abort funnels stay exactly as distinct as they already were.** `retrace-box/src/lib.rs`'s `run()`
and `run_one_for_step` route a guest EL0 exception through the EL1 trampoline's `Ec::Hvc` arm (*inner*, decoded
from `ESR_EL1`); the below-the-trace demand paths — shared-cache page-in, reserved-page commit, and the
fail-loud wild-store negative — arrive as the *outer* `Ec::DataAbort` arm, decoded from the VMM's own
`ESR_EL2`. M6 adds exactly one new inner arm: `Ec::DataAbort | Ec::InstrAbort` (the latter new to
`retrace-arch`, decoded from EC `0x20|0x21`) with the lower-EL form of the EC (bit 0 clear) → `Stop::Fault`,
capturing `far = FAR_EL1` and `pc = ELR_EL1` (the vCPU's own PC at the HVC exit is the trampoline; the faulting
EL0 instruction's address is in `ELR_EL1`). A **same-EL** abort — the trampoline faulting on itself — still
falls through to the fail-loud `Stop::Other` path unchanged: that is a retrace bug, not a guest crash, and M6
does not touch it. The outer funnel is **completely untouched**: `asm/wildstore.s`'s store to an unbacked,
unreserved IPA still stays fatal (`wild_store_outside_any_reservation_stays_fatal`,
`crates/retrace-box/tests/reservecommit.rs`) — an unclaimed stage-2 abort is deliberately *not* reclassified as
a crash (see Deferred). The divergence oracle extends rather than weakens: replay's `Stop::Fault` arm requires
the next recorded event to be `Crash` with a byte-identical `(pc, esr, far)` triple, or the run diverges loudly
(`crates/retrace-core/tests/crash.rs`'s `perturbed_crash_triple_is_a_loud_divergence` rewrites a recorded
`Crash` event via `Writer` — a valid CRC, so the comparison itself is what catches it, not a checksum failure).
`checkpointed_seek` and `retrace debug` treat a crash as an ordinary terminal `(N, K)` position exactly like
`Exit` — both route through the single `Advance::Exited(ReplayReport)` variant, discriminated by
`ReplayReport.outcome`, rather than a second `Advance` variant (a deliberate simplification over the design
spec's sketch of a dedicated `Advance::Crashed`).

**Parking *at* the fault, not at the crash window's start.** `retrace debug`'s `continue` reaching a crash
parks at `(C, K_f)` — `C` the crash's landmark, `K_f` the count of instructions that *did* retire before the
never-retiring faulting instruction (`Exec::park_at_terminal`, `crates/retrace/src/debug.rs`) — so `pc()` is
the fault pc itself and the position orders **after every write in the recording**. That ordering is what
makes "run backward from the corpse to the bug" possible at all: the TDD RED for this (`crashy_cli.rs`) showed
parking at the window's *start* instead makes the corrupting store not-yet-earlier-than-P, so
`reverse-continue` reports the wrong (older) hit and the demo's byte-flip proof goes vacuous.

**The VA→IPA walker — sound by construction, not accidentally correct.** M5's software watch check compared
an armed **VA** against a recorded write's **IPA** directly — exact only for the identity-mapped static guests
that were M5's entire test surface, and silently wrong on any MMU-on guest. `Box_::va_to_ipa`
(`retrace-box/src/lib.rs`) is a read-only 3-level walk of the guest's *own* stage-1 tables (MMU off → identity;
unmapped at any level → `None`), and the watch intersection now translates the armed VA at check time before
comparing IPAs. **This changes no currently-passing or currently-failing case** — every guest mapping in this
repo today is identity (VA == IPA). `crates/retrace-box/tests/vaipa.rs` pins the L1 index shift with a
dedicated assertion (VA `1<<36` selects the empty L1[1], rather than falling through to L1[0]'s table,
so it must miss) — the index that makes `GARBAGE_VA` unmapped in the first place, and so the one an
unconstrained test suite could get wrong without any test noticing. A development-time-only mutation
check (not committed) separately confirmed the L2 index discriminates the same way: shifting it four
bits broke both tests. What's genuinely new is that the fix is no longer *incidentally* right:
`crates/retrace/tests/watch_dyn.rs` proves a syscall-write watch fires correctly on a real MMU-on dynamic guest
(`crashy`'s `fstat(1, &g.st)`, a kernel write into a watchable global) — the deferred M5 proof, now real.

**The demo — `crashy.c`, the whole point of the milestone.** `crates/retrace-guest/c/crashy.c`, built through
real `/usr/lib/dyld` exactly like `hello_dyn`: it calls `fstat`, writes a `"CRASHY:"` marker plus two
address-reveal writes (so tests discover `&g.st`/`&g.ptr` from the trace, never hardcoded), then runs a
volatile off-by-one loop — `for (i = 0; i <= 4; i++) p[i] = GARBAGE_VA` over a 4-long buffer that directly
precedes `g.ptr` in memory — so the *fifth* iteration overwrites `g.ptr` itself with an unmapped garbage
constant. The next store through `*g.ptr` takes a stage-1 EL0 data abort with `FAR == GARBAGE_VA`. The
headline script is entirely existing machinery pointed at this fixture:

```
continue                        # parks AT the fault: pc=<the faulting str>, far=0x4000dead0000
where                            # (C, K_f) — the crash position
watch 0x<&g.ptr>                 # arm a hardware write-watchpoint on the corrupted pointer
reverse-continue                 # walks BACKWARD from the crash to the planted off-by-one store
x 0x<&g.ptr> 8                   # still &g.buf[0] — pre-retire, the store hasn't happened yet
stepi                            # retire the one instruction that corrupts the pointer
x 0x<&g.ptr> 8                   # now reads GARBAGE_VA — the bug, caught in the act
```

The debugger finds the corrupting write starting **only** from the crash, with no forward knowledge of where
the bug is — that's the reverse-debugging story this whole milestone exists to prove.

**The headline gate — `crash_demo_end_to_end`, `crates/retrace/tests/crashy_e2e.rs`.** One test, the whole
story: `record-dyn` of `CRASHY` reports the crash outcome and exit 139; `replay` of that one trace verifies it
bit-for-bit **twice in a loop inside the test itself** (`crashy_e2e.rs:45-49`). Separately, the *test* was
proven **twice** as well, in the honest-gate sense: it stayed born `#[ignore]`d and was run as two independent
`cargo test -- --ignored` invocations before the `#[ignore]` line was removed — a different "twice" from the
in-test double replay, not a restatement of it. The scripted demo then runs against the fresh trace, and its
proof is deliberately **semantic, not a string match**: it asserts exactly two `x`-dump lines exist (closing a
vacuous-filter hole), that the *first* does **not** contain `GARBAGE_VA`'s little-endian bytes and the *second*
**does** — a value-flip that only the aliasing store can produce, with every address and byte discovered from
the trace and the fixture's own source constant, never a coordinate copied out of a hand run. The script also
runs `where` at the crash, but the headline gate asserts nothing about its output — the parked `(C, K_f)`
coordinate is exercised, not proved, here; that coverage lives in `crashy_cli.rs`'s
`continue_parks_at_the_crash_and_where_names_it`.

**The final tally.** `just gate` (full workspace `cargo test` + `cargo clippy --workspace --all-targets -- -D
warnings`, run fresh for this task): **136 passed, 0 failed, 0 ignored**, clippy clean. New this milestone: the
`Ec::InstrAbort` decode test (`retrace-arch`), `Event::Crash` roundtrip/torn-tail/version-reject tests
(`retrace-trace`), `crates/retrace-core/tests/crash.rs`'s three record/replay/divergence tests, `crashy.c` +
`crashy_e2e.rs`'s two fixture tests, `vaipa.rs`'s two walker tests, `watch_dyn.rs`'s dynamic-guest syscall-watch
proof, `crashy_cli.rs`'s two golden crash transcripts, and this section's headline gate. (A pre-existing,
M6-unrelated gate failure — `cache_pager::page_in_cache_data_resigns_auth_pointer_that_authenticates` FPAC-faulting
because the host's dyld shared cache moved past the worked example `cache_pager.rs` pinned — was diagnosed
during M6 and fixed by re-deriving the three constants independently from the current cache's own bytes; see
`spikes/cacheprobe.c` and its README. Not a crash-recording change, but why this milestone's tally is a clean
136/0/0 rather than 135/1/0.) See `docs/superpowers/specs/2026-07-19-retrace-m6-crash-design.md`.

**Deferred, carried forward as the next boundaries:**

- **Signal delivery.** The guest's `sigaction` handlers never run — a fault is terminal, matching rr's default
  disposition for fatal signals. `sigaction`/`sigaltstack` *calls* keep recording as ordinary forwarded
  syscalls; only their handlers being invoked on a real fault is out of scope.
- **Unclaimed stage-2 aborts stay fatal errors, deliberately.** `asm/wildstore.s`'s semantics are unchanged: a
  use-after-free store into a deallocated carveout hole still manifests as an outer stage-2 abort and kills the
  run loudly instead of recording a crash. Promoting it would let a genuine retrace IPA bug masquerade as a
  guest crash; revisiting needs a reservations-aware classifier that M6 does not build.
- **`rwatch`/`awatch`**, watch ranges wider than 8 bytes, and old→new value printing on a hit — all present in
  M5's own deferred list, unchanged by M6 (the VA→IPA fix makes the existing write-watch sound on MMU-on
  guests; it adds no new watch capability).
- arm64e guests, threads (`Sched` stays unused), and open-sourcing work — unchanged from M5.
- **The breadth ladder — C → Rust → brew jq — is the explicit next-milestone arc**, per the design spec's
  framing: M6 proves the crash story on one hand-planted arm64 C bug; the next milestones widen the guest
  surface (a self-built Rust binary, then a real Homebrew-packaged tool) rather than adding debugger
  capability, to find out what breaks when the guest is no longer a fixture written for this project.

## Status: M7 — the breadth ladder, rung 1 (a real `rustc`-built Rust binary)

**What rung 1 proved.** A `rustc`-built `hello_rust` — full `std`, produced by the real toolchain, not a
hand-written fixture — loads through real `/usr/lib/dyld` and runs `libSystem` init (a Rust binary pulls in
no `objc`), reaching further along paths `hello_dyn` never traversed at all — TLV setup and the Rust
runtime's own pre-`main` init — though **not** as far as `hello_dyn` gets: `hello_dyn` reaches `main` and
exits 0, while `hello_rust` still dies before `main`, inside libstd's pre-`main` init. M7 diagnosed and fixed
a real class of bug along the way (below), and the milestone closes with rung 1 **re-parked** at a new,
later, differently-shaped wall rather than green — a legitimate M7 outcome per the design spec's risk R1
("walls come in chains"), not a failure and not grounds to loosen the gate.

**The wall M7 found and fixed: PAC posture was global, not per-process.** retrace enabled PAC for every guest
unconditionally; real macOS enables it **per process**, only for `arm64e` main executables — a plain-`arm64`
process runs with PAC hardware-disabled. dyld's TLV-setup loop contains an unconditional `paciza x16`: on real
macOS running a plain-`arm64` process this is architecturally a NOP (PAC off), but inside retrace's
always-on posture it was a **real signature**, and the guest's later plain `blr` through that pointer branched
through live PAC signature bits as if they were a raw code address — `pc=…`, `esr=0x82000004` (EC `0x20`,
instruction abort lower EL), `far`/branch target `0x67c0001800fc388` (signature bits over the otherwise-valid
shared-cache address `0x1800fc388`). The defect was a *class*, not one pointer (spec risk R4), and
bidirectional: arm64e cache code signs a pointer that plain-arm64 client code then consumes raw, and
plain-arm64 code can hand a raw pointer to arm64e cache code that `AUT*`s it. The fix (`78d884a`, Task 6)
derives the guest's PAC posture from the **main executable's `cpusubtype`** in one helper, fed to all four
SCTLR install sites, with a mandatory fail-loud rule — the posture is never silently defaulted. Task 7 kept the
existing PAC tests falsifiable under the now-derived posture and, as a side effect, produced the repo's first
arm64e guests (`bfamstrip`, `strip47`).

**Why `hello_dyn` never hit this in four milestones of M2.** `hello_dyn` is also plain `arm64`, but it has
**zero `__thread_vars`** — no TLV setup, so no arm64e→arm64 pointer handoff ever occurred on its path. It
survived M2's entire wall-chain by luck of shape, not because its PAC posture was correct; M7 is the first
guest whose shape exercises the defect at all.

**The gate-credibility fix (Task 1).** The rung helper (`util::assert_rung_records_and_replays`) asserts exit
**0** and exact stdout, not mere record/replay agreement — a recorded crash exits 139, and M6 records a crash
as a *successful* recording that replays bit-for-bit, so an agreement-only gate would pass on a guest that died
in dyld without ever reaching `main`. That assertion is exactly what caught the wall below: rung 1 fails loud,
not green-by-accident.

**The wall rung 1 is parked at now — a different mechanism, not the same class.** With PAC no longer
corrupting the run, `hello_rust` gets substantially further (dyld completes, the Rust runtime's own pre-`main`
init begins) and then the guest's `libstd` panics installing the **main thread's stack-overflow guard page**:
`failed to allocate a guard page: Undefined error: 0 (os error 0)` at
`library/std/src/sys/pal/unix/stack_overflow.rs:526`, immediately preceded by an `mmap` trap (syscall 197,
`addr=0x16f4ec000 len=0x4000 prot=RW flags=PRIVATE|ANON|FIXED|…`) whose outcome the guest's `libstd` treats as
failure. There is **no HVF fault at all** here — no `pc`/`esr`/`far` triple, unlike the PAC wall — so this is
provably a **different mechanism** (spec risk R1's "normal ladder outcome"): a syscall-surface gap around
guard-page `mmap`/`mprotect` semantics, not a pointer-signing disagreement. The panic drives Rust's abort path,
which raises a real `SIGABRT` that reaches the host `record-dyn` process itself (exit 134). Because this lands
directly in the Rust `panic!` → `abort()` → `SIGABRT` signal-delivery path — explicitly out of scope since M6 —
M7 does not chase it; `hello_rust_records_and_replays_reaching_main` (`crates/retrace/tests/hello_rust_e2e.rs`)
stays `#[ignore]`d, its reason rewritten to this signature (the old PAC-garbled-branch text is now obsolete and
was deleted, per honest-gate discipline: a stale reason is worse than none).

**The final tally.** `just gate`: **146 passed / 0 failed / 1 ignored** — the 1 is `hello_rust_e2e`
(`hello_rust_records_and_replays_reaching_main`), deliberately parked at the guard-page wall described above,
not swept under the rug. Clippy is clean (`cargo clippy --workspace --all-targets -- -D warnings`, no
warnings), across 63 test binaries.

**Deferred / the next boundary:**

- **The guard-page `mmap` gap itself**, this milestone's parked wall — the immediate next thing a future rung-1
  attempt must characterize and either fix or further re-park.
- **Signal delivery** (unchanged from M6): `panic!`/`abort()` → `SIGABRT` is exactly the deferred fatal-signal
  path M6 already named; M7 confirms a *real* Rust binary reaches it almost immediately.
- **Threads** (`Sched` stays unused): not implicated by this wall — the trace shows no thread-spawn syscall
  before the panic, only main-thread guard-page setup — but remain out of scope per spec risk R2 if a later
  wall does spawn one.
- **arm64e main executables as full dynamic programs** — rung 1 itself has only ever run plain-`arm64`, and
  no real dynamically-linked arm64e program has recorded/replayed yet. But Task 7's `bfamstrip`/`strip47` are
  themselves arm64e main executables (freestanding `-nostdlib -static` asm fixtures, not dynamically-linked
  programs) that record and replay through the CLI, so `restore()`'s PAC-ON posture re-derivation *is*
  exercised end-to-end — the branch's strongest posture evidence to date, short of a full arm64e dynamic guest.
- **Rung 2 (`brew jq`, M8)** and beyond carry all of the above forward unchanged, plus whatever a real
  Homebrew-packaged tool's own init path turns out to need that `hello_rust` didn't.

See `docs/superpowers/specs/2026-07-26-retrace-m7-rust-design.md`.

## Status: M8-stack — guest stack identity (three real defects fixed, rung 1 advanced but still parked)

**What M8-stack set out to do.** M7 parked rung 1 (`hello_rust`) at libstd's stack-overflow guard page. M8
diagnosed that wall as two independent defects in retrace's **stack identity** — the guest was being told the
truth about neither *where* its stack is nor *how big* it is — and fixed both; honoring `MAP_FIXED` then
exposed a third, a recorder abort on an address the guest's space cannot hold, which is fixed here too. All
three fixes are real, tested, and land; the wall itself moved twice but did not fall, and the milestone's own
closing arithmetic turned out to rest on a premise that measurement refutes (below). Per spec risk R1 ("walls
come in chains"), re-parking is a legitimate outcome — but this one comes with a caveat sharp enough to name in
the same breath as the fixes.

**Defect 1: `sysctl({CTL_KERN, KERN_USRSTACK64})` was forwarded.** The guest asked where its stack was and
retrace handed it **retrace's own host-process stack address** — ASLR'd, different every run, and not a guest
address at all. `Box_` now carries its own `stack_top`/`stack_size`, set at load and **path-aware by
construction** (the static path maps one granule below `STACK_TOP_IPA`; the dynamic path maps `DYN_STACK_SIZE`
below `DYN_STACK_TOP`) — hardcoding either constant at the answer site would make the other path lie.
`usrstack64_reply` is a pure builder applied via `apply_and_return`, so replay recomputes the same bytes and
byte-compares them: the standard symmetric posture of symmetry rule 1, not M2-xpcport's deliberate asymmetry.

**Defect 2: anonymous `MAP_FIXED` was ignored outright.** `guest_mmap` took only a length and always
bump-allocated, so a `MAP_FIXED` request silently landed at `mmap_next`. It now honors `addr`/`flags` through
the same `map_mmap_region` the file-backed path already used, and — the part that matters — classifies a FIXED
request against the live backings into three cases rather than unmapping wholesale: **fully covers** (drop and
install), **fully contained in one backing** (copy into it in place, leaving the rest of that backing intact),
and **true partial straddle** (`assert!` fail-loud; no guest exercises it, and fail-loud beats guessing at
split semantics). The containment case is not a nicety — a guard page carved out of the stack lands *inside*
the 256 KiB dynamic-stack backing, and the naive wholesale drop would have unmapped the stack the guest is
running on; loaded image segments, the L1/L2 page tables and the PAC sign stub are each one backing apiece and
equally destroyable. It is exercised by `crates/retrace/tests/fixedinner_e2e.rs`, which asserts the surrounding
region keeps its contents. (It is *not*, as it turns out, what `hello_rust` ends up needing — see below.)

**These were semantically wrong, not merely nondeterministic — which is exactly why M2-cpuid's rule does not
excuse them.** M2-cpuid's position is that forwarded-syscall *variance* which is frozen identically into both
runs is harmless: it never threatens replay determinism, so retrace tolerates it. That rule does not apply
here, and M2-cpuid itself is the precedent for why. Its real defect was never nondeterminism — it was retrace
telling the guest something **false about the guest itself** (`TPIDR_EL0 = TSD_IPA`, from which macOS derived
cluster #48 and indexed out of bounds, deterministically, every single run). `KERN_USRSTACK64` has precisely
that shape: a stable, reproducible, *wrong* answer about the guest's own address space. It would have been a
bug even if retrace's host stack were pinned at a fixed address.

**Two new oracles, because the replay divergence oracle is structurally blind to this whole class.** The
divergence oracle compares replay against **one** recording, so a nondeterministic or simply wrong value that
enters the trace is captured once and reproduced faithfully forever. That is how this defect survived seven
milestones and 146 tests — and `usrstack_replays_bit_for_bit` **passed on the day it was written**, before any
fix, which is the cleanest possible demonstration of the blind spot.

- **Trace reproducibility** (`util::assert_trace_reproducible`): record the same guest twice and compare the
  two traces **byte for byte**, plus exit code and stdout. **Read its scope honestly: it covers *freestanding*
  guests only** (`hello`, `usrstack`). Dyld guests are *not* byte-reproducible run to run, and that was
  **measured, not assumed** — four recordings of `hello_dyn` produced 883 / 885 / 886 / 887 address fields,
  because `gettimeofday` and `getentropy` are forwarded and a libSystem polling loop runs a different number of
  iterations each time. That is accepted per-trace nondeterminism under M2-cpuid and does not threaten replay
  determinism, but it means this oracle must never be cited as "retrace is reproducible" — only as
  "freestanding retrace is reproducible". Making dyld guests reproducible is a milestone of its own.
- **Address-space shape** (`crates/retrace/tests/usrstack_e2e.rs` + the `asm/usrstack.s` fixture): a
  freestanding guest issues `sysctl(KERN_USRSTACK64)`, `getrlimit(RLIMIT_STACK)` and an anonymous `MAP_FIXED`
  mmap and publishes four `u64`s on stdout; the tests compare those against the geometry the box **actually
  built** (`STACK_TOP_IPA = 0x20000`, size one granule `0x4000`, FIXED target `0xB_0000_0000`). It is
  deliberately **address-shaped rather than byte-identical**: the claim under test is "the guest's view of its
  own address space matches the address space retrace constructed", which no whole-trace byte comparison can
  express — two recordings can agree byte-for-byte on an address that is wrong in both.

**Where rung 1 actually landed — advanced, re-parked, and the milestone's closing arithmetic refuted.** The
intended close was: libstd computes `stackptr = kern.usrstack64 - RLIMIT_STACK`, so with both fixes that
becomes `0x200000 - 0x40000 = 0x1C0000`, wholly inside the 256 KiB dynamic-stack backing, and the containment
case preserves the rest of the stack. **Measurement refutes the premise.** Disassembling the guest's own
statically-linked libstd shows `install_main_guard` (inlined into `std::rt::lang_start_internal`) computing
`align_up(pthread_get_stackaddr_np(self) - pthread_get_stacksize_np(self), pagesize)` — it asks **libpthread**,
not the kernel. Two probes settle which operand is which:

- Answering `kern.usrstack64` with `0x1f0000` instead of `0x200000` moved the mmap by exactly `-0x10000`.
  **Defect 1's fix is confirmed working end-to-end on the real dyld guest** — `pthread_get_stackaddr_np` really
  does return the guest's own stack top now.
- Answering `getrlimit(RLIMIT_STACK)` with `0x10000000` instead of `0x40000` left the mmap address
  **bit-identical**. macOS 26's `pthread_get_stacksize_np` calls `getrlimit` and then **ignores the reply**,
  reporting a constant `0x7fc000` (8 MiB minus one 16 KiB page) for the main thread. Synthesizing
  `RLIMIT_STACK` is therefore *correct* — and is asserted by the `usrstack` fixture — but **inert for this
  guest**: it is not the lever that moves the guard page.

So the guest computes `0x200000 - 0x7fc000`, which **underflows** to `0xffffffffffa04000`. And because Defect
2's fix now *honors* `MAP_FIXED`, that wild address is no longer quietly bump-allocated somewhere harmless — it
reaches the stage-2 mapper. That exposed a third defect, which this milestone also fixes.

**Defect 3: a wild `MAP_FIXED` address aborted the recorder.** `map_mmap_region` `expect`ed on `hv_vm_map`,
which rejects an IPA outside the 36-bit guest space with `HvError(4209590275)` = `HV_BAD_ARGUMENT` — so
**retrace itself panicked, exit 101**, with no HVF fault (no `pc`/`esr`/`far`) and no guest error text at all.
That is a strictly worse failure mode than M7's: the guest never got an answer to react to. A guest asking for
the impossible must get an **error back**; only retrace's *own* invariants may fail loud. Both FIXED paths now
validate the request first (`fixed_fits`: 16 KiB-aligned, no overflow, inside the 36-bit ceiling — a pure
function of the request and the fixed IPA geometry, so record and replay classify identically and the symmetry
is structural). The BSD `mmap` path answers the guest **`EINVAL`**, recorded and replayed as an ordinary failed
syscall — a rejected request is a strict no-op, leaving the backings and the `mmap_next` cursor untouched so
later placements are unaffected. The Mach path (`guest_vm_map`) has no errno channel plumbed to its four call
sites and no guest exercises it, so it fails loud with a diagnosis — the same posture as the partial-straddle
case beside it — rather than handing `hv_vm_map` an address it will reject. Covered by
`crates/retrace-box/tests/fixedwild.rs` (both paths, plus the no-op and no-over-rejection properties) and
`crates/retrace/tests/wildfixed_e2e.rs`, whose `asm/wildfixed.s` fixture mmaps `MAP_FIXED` at the exact
address `hello_rust` asks for and publishes the carry and errno it gets back.

**Where that leaves rung 1: back at a GUEST-side wall, with a truthful errno, and one boundary further on.**
With the recorder robust, `hello_rust` now fails the way the real kernel would make it fail: libstd panics
`failed to allocate a guard page: Invalid argument (os error 22)` (M7's signature was the same call site with
the nonsense `Undefined error: 0 (os error 0)`), then `fatal runtime error: initialization or cleanup bug,
aborting`. Two distinct things must land to clear it, and the second was previously hidden behind the first:

1. **The real lever for the guard-page address** — libpthread's own main-thread stack-size bookkeeping, which
   the probes above prove is *not* `getrlimit`.
2. **Guest-raised signal delivery** (deferred since M6, now the terminal failure). The guest's `abort()`
   forwards `__pthread_kill(sig=6)` — trap `num=328 args=[0x103,0x6]` — to the **host**, killing the
   `record-dyn` process itself (exit 134). The trace therefore ends with no terminal event, and replay
   diverges at the last landmark with `expected recorded syscall, got None (truncated=false)`. M6's crash
   recording covers HVF **faults**; a signal the guest raises on itself is a different path, and it is the same
   class of defect as Defect 3 — a guest-side event escaping into retrace's own process instead of being
   serviced against the guest.

**The final tally.** `just gate`: **171 passed / 0 failed / 1 ignored** — the 1 ignored is
`hello_rust_e2e::hello_rust_records_and_replays_reaching_main`, re-parked at the boundary above with its
`#[ignore]` reason rewritten to the new signature and the M7 guard-page text deleted, per honest-gate
discipline (a stale reason is worse than none). Clippy is clean
(`cargo clippy --workspace --all-targets -- -D warnings`).

**Deferred / the next boundary:**

- **Guest-raised signal delivery** (deferred since M6; now the terminal failure on rung 1 and the
  highest-priority item): `__pthread_kill`/`SIGABRT` is forwarded to the host and kills the recorder, so a
  guest that aborts cannot be recorded at all. Servicing it against the guest — the way M6 records a fault —
  is what turns rung 1's remaining failure into a *recordable, replayable* crash rather than a dead trace.
- **The real lever for the guard page** (new): `pthread_get_stacksize_np` is proven to ignore `RLIMIT_STACK`,
  so the guest's main-thread stack size comes from libpthread's own bookkeeping (`stackaddr - stackbottom`,
  seeded during `__pthread_init`, plausibly from the `main_stack=` entry of the `apple[]` array retrace builds,
  or from libpthread's built-in 8 MiB default). Characterizing *that* is what a future rung-1 attempt must do;
  a third synthesis mechanism aimed at `getrlimit` will not help.
- **Stack *size*** (spec risk R3): the dynamic guest's stack is 256 KiB, leaving ~240 KiB usable once a 16 KiB
  guard page is carved out of it. Real macOS gives the main thread 8 MiB. Nothing has needed the depth yet, but
  a deeper guest will, and the two facts interact — growing the stack is also one way the underflow above stops
  being an underflow.
- **`guest_munmap` has the identical wholesale-drop defect** that Defect 2's fix removed from the mmap path: it
  still drops the *entire* backing containing `ipa` and ignores `len` (`let _ = len;`). The three-case overlap
  classification should be shared with it.
- **`prot` is still ignored except for `PROT_EXEC`.** Stage-2 stays RWX by design (the VMM is the security
  boundary), so a `PROT_NONE` guard page is reused as RW — the guest gets no fault when it overflows into it.
  Correct guard-page *semantics* need this even once the address is right.
- **`guest_mmap_replay` naming** (carried from t5 review): it serves only the *file-backed* replay arm (the
  anon replay arm calls `guest_mmap` directly), so the more specific `guest_mmap_file_replay` was the better
  name; the generic one now reads as if it covers both.
- **Threads** (`Sched` stays unused): still not implicated — no thread-spawn syscall appears before the wall.
- **arm64e main executables as full dynamic programs**: unchanged from M7. The arm64e fixtures
  (`bfamstrip`, `strip47`) are freestanding, so no real dynamically-linked arm64e program has recorded and
  replayed yet.
- **Rung 2 (`brew jq`)**: unchanged, and now clearly gated behind rung 1 — a Homebrew-packaged tool has all of
  `hello_rust`'s init path plus its own.

See `docs/superpowers/specs/2026-07-31-retrace-m8-stack-design.md`.

## Status: M8-stack close — 🎉 rung 1 is GREEN, and the fix was to stop fighting a constant

**`hello_rust_e2e` is un-`#[ignore]`d and passes.** A real Rust binary — built by the real toolchain, full
`std`, dynamically linked — records and replays **bit-for-bit** through real `/usr/lib/dyld` and reaches
`main`, printing `hi from rust`. The gate is the strict one: `util::assert_rung_records_and_replays` demands
exit 0, exact stdout, a byte-identical replay, and a double replay, so it cannot pass on a guest that died
inside dyld. **`just gate`: 173 passed / 0 failed / 0 ignored**, clippy clean — nothing is ignored for the
first time since M2-taskinfo.

**The close was a two-constant change, and the reasoning is the interesting part.** The section above ended
with two things believed necessary to clear rung 1: characterize libpthread's main-thread stack-size
bookkeeping, and implement guest-raised signal delivery. **Neither was needed.** The measurement that mattered
was already in hand and pointed somewhere cheaper.

libstd's `install_main_guard` mmaps `MAP_FIXED` at `pthread_get_stackaddr_np() -
pthread_get_stacksize_np()`, and the probes proved the subtrahend is a **constant retrace cannot influence**
(macOS 26's libpthread reports `0x7fc000` and discards the `getrlimit` reply). Every attempt to make retrace
*answer* that question differently was therefore doomed. But retrace fully controls the **minuend**: with the
dynamic stack top at 2 MiB, `0x200000 - 0x7fc000` underflowed; with it at **40 MiB**, the guard page lands at
`0x2004000` — just above the L3 page-table window at 32 MiB, in free, mappable address space. The guest gets
its guard page and init completes.

**Only the top moved; the stack is still 256 KiB.** Backing a real 8 MiB stack also works and is arguably more
faithful — it was tried first, and it makes the guard page land *inside* the stack backing, which is exactly
the containment case Task 5 implemented. It was rejected on **measured cost**: per-syscall memory diffing
scales with total mapped guest memory, so `hello_rust` went from 8.4 s to 13.9 s and the dyld suite blew past
a 10-minute gate timeout. The guard page does not need to be *inside* the stack to be installed — it only
needs a mappable address — so the cheap placement gets the same behaviour for no cost.

`stack_geometry_tests::the_guard_page_libstd_computes_is_a_mappable_guest_address` pins the arithmetic as a
pure constant check that runs instantly on every gate: it fails the moment the layout is edited back into an
underflow or a collision with the L3 window. That is the regression this milestone most needed, because the
failure it guards against cost two separate walls.

**What this does and does not prove.** Rung 1 is a *breadth* result: it says retrace's syscall and memory
surface is now complete enough for a real language runtime's init path — libstd, libpthread, libmalloc, libxpc,
libsystem_trace, objc — end to end, deterministically, twice. It does **not** say retrace handles threads,
signals, or a program that does substantial work. `hello_rust` still only writes one line and exits.

**The next boundary, unchanged in substance:**

- **Guest-raised signal delivery** — still the top item, and still deferred rather than solved. It is no longer
  in rung 1's path only because nothing aborts any more: `__pthread_kill`/`SIGABRT` is forwarded to the host
  and would kill the recorder, so *any* guest that aborts still cannot be recorded. M6's crash machinery
  covers HVF faults; this is the sibling case.
- **`prot` is still ignored except for `PROT_EXEC`.** libstd `mprotect`s its guard page `PROT_NONE` (visible in
  the trace as trap 74 right after the guard mmap) and retrace accepts it while stage-2 stays RWX — so the
  guard page is real memory, and a stack overflow would silently scribble instead of faulting. The guard is
  *installed*, not *enforced*.
- **Stack size / spec risk R3** — the guest believes it has 8 MiB while 256 KiB is backed. A deep recursion
  faults on unmapped IPA rather than striking the guard. Unchanged by this fix, and now the more visible gap.
- **`guest_munmap`'s wholesale-drop defect**, the `guest_mmap_replay` rename, threads, and arm64e dynamic
  guests — all unchanged from the list above.
- **Rung 2 (`brew jq`)** is now genuinely next, and no longer gated behind rung 1.

See `docs/superpowers/specs/2026-07-31-retrace-m8-stack-design.md`.

## Status: M9-jq — 🎉 rung 2 is GREEN, and the wall was not where the milestone aimed

**`jq_e2e` passes: `brew jq -n '1+1'` records and replays bit-for-bit through real `/usr/lib/dyld`,
printing `2`.** It is the first guest that loads dylibs which are **not in the dyld shared cache** —
`libjq.1.dylib` and `libonig.5.dylib`, real files under `/opt/homebrew`, the latter reached through the
`/opt/homebrew/opt/oniguruma` symlink. Same strict gate as rung 1 (`assert_rung_records_and_replays`:
exit 0, exact stdout, byte-identical replay, replayed twice). **`just gate`: 185 passed / 0 failed /
0 ignored**, clippy clean.

`jq` comes from Homebrew, not from this repo, so `jq_e2e` announces a loud skip on a machine without it
rather than passing quietly — a silent skip reads as a green it did not earn.

**The milestone built a guest-side TLBI oracle. `jq` never needed it.** That is the honest headline, and
it is worth stating plainly rather than burying: the mechanism this milestone was designed around carried
`jq` without a single new fault, and the thing that actually blocked rung 2 was somewhere else entirely.
Both results are real; only one was predicted.

**The oracle (Tasks 1–3).** The long-standing rule was that `set_region_exec` is sound only on a block the
guest has never translated, *because the VMM cannot issue a guest TLBI* — so exec mmaps were placed in
fresh 32 MiB-exclusive blocks. The codebase had already predicted the fix in a comment: retrace can't issue
a TLBI, but **the guest can**, and retrace already knew how to make the guest run an instruction it did not
write — that is exactly what the PAC signing oracle does. Both halves existed. `flush_guest_tlb` runs
`tlbi vmalle1; dsb ish; isb; hvc #0` on the guest vCPU at EL1 from a dedicated scratch page, wrapped in the
sign stub's own save/restore discipline so a mid-run caller sees nothing, and the page uses `ATTR_TRAMP`
(EL1-exec) rather than `ATTR_CODE` — `tlbi` is an EL1 instruction and `ATTR_CODE` sets PXN.

**The spike's measured answers**, because the control is the interesting one:

- **F1 — does `tlbi vmalle1` execute untrapped at guest EL1?** Yes. It ran clean and reached its `hvc`.
- **F2 (control) — does a hand-flipped data→code leaf really stale-fault without a flush?** **Yes: the
  guard's premise held.** `ESR_EL1 EC=0x20` (instruction-abort, permission fault), with the payload's
  sentinel proving it never ran. This mattered: the spike's *first* run reported "EXECUTED ANYWAY", which
  would have said the invariant was over-conservative all along. That was a **measurement artifact** — every
  EL0 trap funnels through one unconditional-`hvc` vector, so the EL2 exception class cannot discriminate
  fault from success. Discriminating on `ESR_EL1` plus an `x0` sentinel reversed the answer. A spike that
  cannot distinguish its two outcomes is worse than no spike.
- **F3 — does the same page execute after the flush?** Yes, `x0=0x5a`: the payload genuinely ran.

**Task 3** then relaxed `place_fixed`: a `MAP_FIXED PROT_EXEC` request contained in a live backing is
promoted and *then* flushed, instead of asserting. That is dyld's real strategy for a non-cache dylib —
reserve the image's span, touch it, then `MAP_FIXED` each segment in with its own protections. A code-review
follow-up caught a genuine second-order defect before it could bite: `from_checkpoint` reset
`tlbi_stub_ready` to false while the restored backings already contained the stub's IPA, so a flush after a
checkpoint restore re-mapped an already-mapped IPA and panicked — latent since Task 2, first reachable at
Task 3, now pinned by `flush_guest_tlb_survives_checkpoint_restore`.

**Task 4** widened dyld's process-start stack from a hardcoded `argc=1` to a real `argv[0..argc]`, and gave
the CLI a `--` separator (`retrace record-dyn <exe> -o <trace> -- <guest args…>`). `jq` with no filter does
nothing, so rung 2 needed this regardless of the TLBI work. The old layout hid the argv and envp terminators
in two trailing zeros of a five-word vector; they are separate pushes now, which is what makes it correct for
any `argc`. `hello_dyn_e2e` and `hello_rust_e2e` passing with `&[]` is the proof the `argc=1` layout dyld
already accepts was left alone.

**The real wall: retrace was treating the guest's fd 0/1/2 as its own.** Two defects, one root cause, both
found by driving `jq`:

1. **Console writes were recognized only as `write` (4).** libc's **stdio** flush uses `write_nocancel`
   (397), so `printf` output fell through to the generic forward path and the **host** kernel performed the
   write — to retrace's own stdout. This is the nastiest shape a bug can take here: on a terminal the
   recording looked *perfect*, because the text appeared. The trace held no console bytes at all, and replay,
   which executes no syscall, printed nothing. Nothing in the gate had ever used stdio; `hello_dyn.c` calls
   `write(2)` directly.
2. **`close` of fd 0/1/2 was forwarded too**, so a guest closing its stdout closed **retrace's**. `jq` does
   this on the way out. Afterwards the CLI wrote the mirrored recording into a closed descriptor and the run
   reported success having emitted nothing — exit 0, empty stdout, no error anywhere.

Both now route through shared predicates in `retrace-arch`, `is_console_write` and `is_console_close`, rather
than each call site spelling out the condition. That is deliberate: record's arm and replay's mirror must
agree (symmetry rule 1), and when they don't the failure is **silent** — a forwarded console write still
prints, so nothing looks wrong until replay comes up empty. The close is faked, never forwarded, and needs no
replay arm: its `(ret=0, err=false, no writes)` flows through the generic `apply_and_return` with the
`(num, args)` divergence check intact. `stdio_dyn` and `closefd_dyn` pin each mechanism separately, asserting
the trace really records syscall 397 and a faked close of fd 1 — not merely that `jq` got further.

**What this does and does not prove.** Rung 2 is a breadth result about *loading*: dyld can bind and run a
program whose dylibs live outside the shared cache, and retrace's console surface is now faithful enough that
a stdio program's output belongs to the recording rather than to the recorder. `jq -n '1+1'` is still a
small program that computes one value and exits. Threads, signals, and real input are all still untouched.

**The next boundary:**

- **Guest-raised signal delivery** — unchanged, and still the top item. `__pthread_kill`/`SIGABRT` is
  forwarded to the host and would kill the recorder, so any guest that aborts still cannot be recorded.
- **No fd table.** Retrace still does not model an fd as *closed*: a guest that wrote to fd 1 after closing it
  would see the write succeed instead of `EBADF`. Faking the close fixed the leak, not the fidelity gap, and
  closing it properly means giving the box a real fd table. Nothing in the gate does this.
- **Block-exclusive exec placement is now retirable, but was not retired.** A non-FIXED `PROT_EXEC` mmap
  still rounds `mmap_next` up to a fresh 32 MiB block. With the oracle in hand that is no longer a
  *correctness* requirement — it is a flush avoided, not a hazard avoided — and the doc comment says so.
- **The anon `PROT_EXEC` / JIT gap is likewise unblocked but still open**: `guest_mmap` installs plain
  RW+non-exec pages for an anonymous exec mmap and warns. The oracle removes the reason it couldn't be fixed;
  no guest in the gate needs it yet.
- **`prot` is still ignored except for `PROT_EXEC`**, spec risk **R3** (the guest believes 8 MiB of stack
  while 256 KiB is backed), **`guest_munmap`'s wholesale-drop defect**, the `guest_mmap_replay` rename,
  threads, and arm64e dynamic guests — all unchanged.
- **Rung 3** — a guest that reads real input, or does substantial work — is next, and `jq` with a file
  argument is the natural first step now that `--` exists.

See `docs/superpowers/specs/2026-08-01-retrace-m9-jq-design.md`.

## Status: M10-fdtable — the guest's descriptors are its own, and rung 3 was already free

**The guest no longer borrows retrace's file descriptors.** Before M10, `forward_and_diff` issued the
guest's syscall via a raw `svc` in retrace's own process with no translation at all, so a guest fd
literally *was* a host fd: `jq '.name' t.json` observed `0x11`–`0x16` (17–22), and it started at 17
only because retrace itself holds 0–16 open. It now observes 3,4,5,6,7,8 — a function of the guest's
own `open`/`dup`/`close` sequence and nothing else. **`just gate`: 212 passed / 0 failed / 0 ignored**
(79 test binaries), clippy clean.

**That was a correctness defect, not merely a determinism one.** The guest's 17 raw `close()` calls
were forwarded straight into retrace's own descriptor table while `cache.rs` held a live fd on the
shared cache. A guest closing the wrong number would have closed a descriptor the *recorder* owns,
which is the M9 console bug generalised: the failure is silent, and the recording is wrong in a way
that looks fine.

**Rung 3 already passed before the milestone began, and the README says so rather than claiming it.**
`jq '.name' <fixture>` recorded and replayed bit-for-bit at HEAD `84983dc` with no fd table in
existence — the forward-and-record path already captured the file's bytes as recorded kernel writes,
and replay already executed no syscall. `jq_file_e2e` **pins** that capability; it did not earn it.
The test with teeth is the second one: it records from a scratch copy, rewrites that file to
`{"name":"TAMPERED"}`, and requires replay to still print `retrace` — the trace is self-contained, not
a script that re-reads the input.

**The mechanism: a split table.** Guest-visible slots (`Free|Open|Closed`, lowest-not-currently-open
from 3) are a pure function of the guest's own syscall sequence and are identical on record and
replay. A separate `guest_fd → host_fd` map is **record-only**, because replay executes no syscall and
opens no host fd. Host descriptor numbers therefore never enter the trace, and the milestone keeps the
**standard symmetric posture** (symmetry rule 1): replay recomputes what the allocator would have
produced and byte-compares — deliberately *not* M2-xpcport's verbatim-apply exception, which exists
only because a minted Mach port name cannot be regenerated. A guest fd can.

The oracle is proven non-vacuous rather than merely present: a passing replay would look identical if
the recompute were never reached, so `a_recorded_host_shaped_fd_is_caught_as_divergence` rewrites a
recorded `open()` return to 17 — exactly what a pre-M10 recording held — and requires replay to reject
it.

**What driving it actually found**, in the order it hurt:

- **`forward_and_diff` owns the whole fd contract, not half of it.** Translation-in lived in the box
  while binding-out lived in `retrace-core`'s dispatch, so any *other* driver of `forward_and_diff` had
  to remember the second half — and `memdiff`'s mini record loop did not, so its guest's `open()`
  returned an unbound host fd and its `read()` came back `EBADF`. Moving the binding into
  `forward_and_diff` made that test pass **untouched**, which is the evidence that the split was the
  bug rather than the test.
- **Console fds must map identically onto retrace's own.** M9 intercepts console *writes* and *closes*
  — but only those. stdio still `fstat()`s and `ioctl()`s fd 1 to choose a buffering mode, and leaving
  those unmapped answered `EBADF` to every one, crashing `watch_dyn`'s guest. A unit test asserting
  "console fds have no host mapping" **passed while being wrong**; only a real guest caught it.
- **`map_with_linking_np` (550) carries its fd inside a struct in guest memory**, so no operand index
  can name it. The array is const, so translation forwards a host-side *copy* with `mwlr_fd` rewritten;
  guest memory is never mutated and no host fd reaches the trace as data.
- **The plain-vs-`_nocancel` trap fired a third time, on a pre-existing latent defect.** The read-buffer
  clamp covered `read`(3) and `pread`(153) but not `read_nocancel`(396) — the variant `jq` actually uses
  — so those reads were forwarded unclamped and the host kernel could write past the destination
  backing. That bug predates M10; building the fd table is what surfaced it.
- **POSIX reuses closed slots, not merely free ones.** The RED run caught `alloc()` returning 5 after a
  `close(3)`. The `Free`/`Closed` distinction survives for checkpoint fidelity but does not gate reuse.
- **The guest's first `open` is 4 under retrace, not 3** — and that is environmental, not a table
  defect. libSystem opens a socket before `main` under retrace (there is no real notifyd/bootstrap to
  reach) and does not natively. So `fdtable_dyn` asserts **invariants** rather than absolute numbers —
  `low` (the descriptor is the guest's own small number, `>= 3` and `< 16`), `dupnext`, `ebadf`,
  `dupread`, `reuse` — all five of which hold *both* natively and under retrace. The spec's exit
  criterion said "fd 3"; the measurement corrected it, and pinning 3 would have tested libSystem's
  pre-main behaviour instead of the fd table. The companion test reads the recorded trace and rejects
  any recorded fd `>= 16`.

**Risk R1 fired during spec authoring, before a line of code existed.** A first pass over a `head -25`
syscall histogram tabled `read`(3) — which `jq` never calls — and missed `read_nocancel`(396),
`open_nocancel`(398), `socket`(97), `connect`(98) and `sendto`(133). Re-deriving from the **full**
untruncated histogram in Task 1 then found five more rows the spec had also missed: `fcntl_nocancel`(406),
`fstatat64`(470), `fgetattrlist`(228), `shm_open`(266), and `map_with_linking_np`(550). The transferable
rule for the next syscall table anyone writes here: **table the `_nocancel` variant beside its plain
form as a pair, never one number at a time** — macOS libc routinely takes only the `_nocancel` path, so
a plain-only predicate fails silently — and **never derive a syscall surface from a truncated
histogram**, because the tail is where the count-1 and count-2 syscalls live, which are exactly the ones
no existing test covers. Resolve numbers to names from
`$(xcrun --show-sdk-path)/usr/include/sys/syscall.h`, not from memory.

**The new boundary.** M10 closed the fd-fidelity gap M9 named and touched nothing else, so most of M9's
list carries forward verbatim:

- **Guest-raised signal delivery** — unchanged, and still the top item. `__pthread_kill`/`SIGABRT` is
  forwarded to the host and would kill the recorder, so any guest that aborts still cannot be recorded.
- **`dup2` is fail-loud, not modelled.** It names its own target slot rather than taking the lowest free
  one, and no gate guest calls it (measured: zero in the `jq` run), so `retrace-core` asserts on it. A
  silently mis-modelled `dup2` aliases the wrong file.
- **`fcntl(F_DUPFD)` is the weaker case: unmodelled and *not* fail-loud.** `fcntl` gets plain x0
  translation and no allocation-on-return, so an `F_DUPFD` would hand the guest an unbound descriptor
  rather than an assert. It is measured absent from `jq`'s 17 `fcntl` calls (`F_GETPATH`×10,
  `F_ADDFILESIGS_RETURN`×4, `F_CHECK_LV`×2, `F_SETFD`×1), but it is a missing row, not a guarded one —
  the honest next fix in this area.
- **Guest stdin is still retrace's.** fd 0 maps identically onto the host's; no gate guest reads it.
- **`RLIMIT_NOFILE` is unenforced** — the table just grows, and a guest calling `getrlimit` still gets
  the host's answer.
- **Block-exclusive exec placement is still retirable and still not retired**; the anon `PROT_EXEC`/JIT
  gap is likewise unblocked by M9's TLBI oracle but still open; **`prot` is still ignored except for
  `PROT_EXEC`** (spec risk R3 — the guest believes 8 MiB of stack while 256 KiB is backed);
  **`guest_munmap`'s wholesale-drop defect**, the `guest_mmap_replay` rename, threads, and arm64e
  dynamic guests — all unchanged.
- **Rung 4** — a guest that does substantial work, or one that threads — is next. Rung 3 asked `jq` to
  read a file; it did not ask it to do anything hard.

See `docs/superpowers/specs/2026-08-04-retrace-m10-fdtable-design.md`.

## Status: M11-signals — 🎉 a guest can abort, and the recorder lives to record it

**The README's top deferred item since M6 is closed.** A signal the guest raises on itself is now a
recorded, replayable terminal event instead of a host signal that kills the recorder. A real
full-`std` Rust binary that `panic!()`s records and replays bit-for-bit, exiting 134 (= 128 +
SIGABRT) on both sides. **`just gate`: 240 passed / 0 failed / 0 ignored** (85 test binaries),
clippy clean, nothing `#[ignore]`d. `panic_e2e` joins the headline set green and un-ignored — no
gate was parked this milestone.

**The bug, demonstrated rather than argued.** `forward_and_diff` issues the guest's syscall through
a raw `svc` in *retrace's own process*, and no signal syscall was special-cased anywhere. Revert
M11's dispatch arms and `cargo test -p retrace-core --test signals` does not merely fail — the test
harness itself dies: `process didn't exit successfully: (signal: 6, SIGABRT: process abort signal)`.
That is the whole milestone in one line of output.

**Two defects nobody had written down, both now fixed.** M6 recorded the *delivery* half honestly
("the guest's `sigaction` handlers never run"), but not these:

- **`sigaction` was reading and writing RETRACE's signal table.** Measured live: `hello_rust`'s
  startup query of `SIGSEGV` returned handler `0x104e6d7ec` with flags `0x41`
  (`SA_SIGINFO|SA_ONSTACK`) — *retrace's own libstd stack-overflow handler*. libstd installs only
  when the query returns `SIG_DFL`, so the guest silently skipped installing its own. With the
  guest's table in place the query returns `SIG_DFL` and libstd proceeds: `hello_rust`'s signal
  surface went from 3 calls to 6 (a `sigaltstack`, plus real handler installs for `SIGSEGV` and
  `SIGBUS`). The guest now has signal state of its own instead of borrowing the recorder's.
- **`kill(pid, sig)` reached any host pid, untranslated and unchecked.** The only defect in this
  area that escaped the sandbox. `killother_e2e` fires `kill(1, SIGKILL)` from a guest, requires the
  recorder to abort naming the boundary, and then asserts pid 1 is *still alive* — distinguishing
  `EPERM` (exists, not ours to signal) from `ESRCH` (gone), because both return `-1` and only the
  second is the catastrophe.

**Disposition, not delivery.** `SigTable` (`crates/retrace-box/src/sig.rs`) holds per-signal
disposition, the blocked mask, and the alt stack. It is a pure function of the guest's own calls, so
both runs compute it identically and **nothing about it enters the trace** — `FdTable::slots`'
posture, and the **standard symmetric** one (replay recomputes and byte-compares), deliberately not
M2-xpcport's verbatim-apply exception. A raise consults it: `Ign` continues, `Dfl`+fatal appends
`Event::Signal{sig,pc}` plus the final snapshot, and `Handler` **asserts** — running a handler needs
signal frames, the `__sigtramp` ABI and `sigreturn`, which is M12 and is the larger half.

`Event::Signal` is a new variant rather than `Event::Crash` with a synthetic ESR (`TRACE_MAGIC`
`0x0004`→`0x0005`; no fixture is checked in, so nothing was invalidated). A signal is not a fault,
and a `SIGABRT` printing as one bearing an ESR the hardware never produced is a lie the debug output
would carry forever.

**The measurement, which decided the milestone's shape** (`RETRACE_TRACE=1`, full untruncated
histograms over `hello_dyn`/`hello_rust`/`jq`, per M10's rule):

- Of `37/46/48/52/53/111/184/328/329/330/520/521`, **only `sigaction`(46) appeared at all** — 3×, in
  `hello_rust` alone. The other eleven were zero across all three guests. That zero-count is the
  evidence each `assert!` rests on.
- **`getpid`(20) returns retrace's own pid** — confirmed at runtime by recording in-process and
  comparing the recorded return against `std::process::id()`, not inferred. The `kill` self-check
  depends on it, so it was measured rather than assumed.
- **No guest installs a real handler.** The single non-query install is `SIGPIPE → SIG_IGN`. Spec
  risk R3 (an existing green test walled by the handler assert) did not fire.
- **The abort path is `__pthread_kill`(328), not `abort_with_payload`(521)** — settled by the
  headline guest: `args=[0x103, 0x6]`, the thread port matching M7's observation exactly, with
  `sigprocmask(SIG_SETMASK)` immediately before it (that is `abort()` unblocking `SIGABRT`, which is
  why the blocked-raise assert is unreachable on the realistic path). Risk R2 did not fire either.

**What driving it actually found:**

- **A default Rust `panic!()` never raises a signal at all.** With `panic=unwind` it unwinds to
  `lang_start`, prints, and exits **101**. The headline guest needs `-C panic=abort` to exit 134 and
  exercise anything this milestone added. Measured natively before the fixture was wired in — a
  plan that had assumed otherwise would have produced a gate that passed for the wrong reason or
  failed for a reason having nothing to do with signals.
- **The replay-side `sigaction` mirror is load-bearing, and that is proven rather than asserted.**
  Disable it and `sigign` diverges with `expected recorded Signal, got Some(Syscall { num: 37, … })`:
  replay's table still reads `Dfl` for `SIGABRT`, so it terminates a guest that had ignored it.
  Without `sigign_e2e`, a bug making *every* raise terminal would pass the entire suite.
- **The second oracle does not apply to these guests across processes, and was not weakened to
  pretend otherwise.** `assert_trace_reproducible` compares two recordings from two *separate*
  recorder processes; both signal guests call `getpid`, which M11 deliberately leaves forwarding, so
  the recorder's pid lands in the trace. Measured: the two traces differ in exactly one record — the
  CRC and body of the `num=20` event — and nowhere else. The coverage moved to in-process recordings
  (constant pid), which ask the question the oracle is actually for. Relaxing the helper to tolerate
  a varying pid would have blunted an oracle the whole project leans on, to buy nothing.
- **`sigaction`'s in-param and out-param are different C structs** — `struct __sigaction` is 24
  bytes (it carries `sa_tramp`), `struct sigaction` is 16. Synthesizing the `oldact` writeback at the
  input width would corrupt the guest 8 bytes past the struct and surface days later as something
  unrelated. `encode_oldact` returns a fixed `[u8; 16]`, so emitting the wrong width is impossible
  rather than merely tested.

**The new boundary.** M11 closed the signal-disposition gap and touched nothing else, so M10's list
carries forward almost verbatim:

- **Handler *delivery* is the top item now, in place of the one this milestone retired.** Signal
  frames, the `__sigtramp` ABI, `ucontext`/`mcontext` layout, and `sigreturn`(184) — M12, and the
  larger half of the problem. `hello_rust` now genuinely installs `SIGSEGV`/`SIGBUS` handlers, so the
  first guest that actually faults will hit the `Handler` assert rather than a plausible lie.

  > **Corrected by M12 (left in place rather than overwritten, because it was this milestone's
  > premise).** That last sentence was false when written. The `Handler` assert existed only on the
  > *self-raise* arm; `Stop::Fault` appended `Event::Crash` and broke without ever consulting the
  > `SigTable`. A guest that installed a `SIGSEGV` handler and then faulted was recorded as a
  > terminal crash with its handler silently skipped — the plausible lie, not the assert. See the
  > M12-signal-delivery section below.
- **`__pthread_kill`'s thread-port operand is wired but ungated.** 328 fires in no *freestanding*
  gate guest, so there was no observed port to validate against; its coverage rides entirely on
  `panic_e2e`. The guest has one thread on one vCPU, so any port it can name is that thread — ungated
  rather than wrongly gated. Learn it from `mach_thread_self` when a guest needs the check.
- **A pending signal set is unmodelled**, so raising a *blocked* signal asserts. That is what makes
  `sigpending` returning empty true by construction rather than a convenient lie — the two decisions
  stand or fall together, and whoever adds a pending mask must revisit both.
- **`sigsuspend`(111), `__sigwait`(330), `sigreturn`(184), `terminate_with_payload`(520) and
  `abort_with_payload`(521) are fail-loud asserts**, not models. 520/521 were live
  recorder-killing hazards before M11; asserting converts a silent host death into a loud stop.
- **`sigaltstack` is stored but not honoured** — no handler runs, so there is nothing to run on an
  alternate stack.
- Everything else from M10 is unchanged: **`dup2` fail-loud**, **`fcntl(F_DUPFD)` unmodelled and
  *not* fail-loud** (still the honest next fix in that area), **guest stdin is still retrace's**,
  **`RLIMIT_NOFILE` unenforced**, block-exclusive exec placement, **`prot` ignored except
  `PROT_EXEC`**, `guest_munmap`'s wholesale-drop defect, threads, and arm64e dynamic guests.

See `docs/superpowers/specs/2026-08-05-retrace-m11-signals-design.md`.

## Status: M12-signal-delivery — 🎉 the guest's handlers actually run

**M11's named boundary is closed.** A signal with a handler installed no longer asserts: retrace
builds the signal frame, enters the guest's handler through the real `sa_tramp` contract, and
services `sigreturn`(184) to put the guest back. Both causes route through the same disposition
decision — a signal the guest **raises on itself**, and a **hardware fault** its own instruction
produced. **`just gate`: 296 passed / 0 failed / 0 ignored** (90 test binaries), clippy
clean, nothing `#[ignore]`d. `segv_rust_e2e` joins the headline set green and un-ignored; no gate was
parked this milestone.

**The headline.** A stock full-`std` Rust binary (`rs/segvy.rs`) stores through a wild pointer.
libstd's **own** `SIGSEGV` handler runs, compares `si_addr` against the guard range it installed,
concludes this is not a stack overflow, resets the disposition to `SIG_DFL` and **returns**; the
store re-executes, faults again, and the default action terminates the guest at 139. Recorded and
replayed bit-for-bit, twice. One run exercises delivery, Apple's trampoline, `siginfo`, `sigreturn`,
a mid-handler `sigaction`, a second fault, and M11's terminal path.

**Exit 139 is necessary and nowhere near sufficient, and the gate says so.** An *uncaught* fault
exits 139 too — that is exactly what M6's `crashy_e2e` asserts — so a gate resting on the exit code
would have passed unchanged with M12's routing entirely broken. The gate asserts on the trace
instead: exactly one `SignalDelivery` with `sig == 11` whose `handler` is the VA libstd actually
installed, a `sigreturn` *after* it (the handler returned rather than aborting), a terminal
`Event::Crash` after that, and `resume_pc == ` the crash `pc` (the store was re-executed, not
skipped). The installed VA is **learned, not hardcoded**, and learning it is itself a test: the
handler is not a datum in the trace — `sigaction`'s event carries a *pointer* to the guest's
`struct __sigaction` — so the gate seeks a `ReplaySession` to that landmark and reads `sa_handler`
out of reconstructed guest memory.

**A correction to the M11 Status section, because this milestone's premise rested on it.** M11 wrote
that "the first guest that actually faults will hit the `Handler` assert rather than a plausible
lie." That was not what the code did. The assert existed only on the *self-raise* arm; `Stop::Fault`
appended `Event::Crash` and broke without ever consulting the `SigTable`. So a guest that installed a
`SIGSEGV` handler and then faulted was recorded as a terminal crash with its handler silently
skipped, and nothing said so. M11 itself measured that `hello_rust` installs real `SIGSEGV`/`SIGBUS`
handlers at startup, so the wrong answer was live rather than hypothetical.

**The measured facts that shaped it** (`spikes/sigabi.c`, `spikes/sigtramp.c`, `spikes/sigraisex0.c`
— compiled and run natively, not recalled):

- **The frame is 976 bytes at the new `sp`**: `siginfo_t`(104) ‖ `ucontext_t`(56) ‖ `mcontext64`(816),
  where `uc_mcontext` is a **pointer** to `+160`. `sp == x3 ==` the frame base, `siginfo` at offset 0.
  A design assuming one flat struct would have been wrong by 816 bytes.
- **The entry contract**: `x0`=the catcher, `x1`=infostyle, `x2`=the signal, `x3`=`siginfo_t*`,
  `x4`=`ucontext_t*`, `x5`=the `sigreturn` token. The host's token is process-randomized; retrace
  synthesizes the whole frame and so owns it, using a **constant** folded with the ucontext address —
  the fixed-PAC-keys posture. Nondeterminism never gets an opening, and validation is a free
  fail-loud on a corrupted frame.
- **`_sigtramp` was disassemblable out of the shared cache** (spec risk R2, resolved rather than
  worked around). It forwards `x3`/`x4` verbatim, reads **no** frame field, saves and restores the
  kernel's `x5` across the handler call, and **hardcodes infostyle `0x1e` into `sigreturn`'s second
  argument** regardless of what the kernel passed. retrace's `sigreturn` arm ignoring `args[1]` is
  therefore validated, not merely convenient.
- **infostyle without `SA_SIGINFO` is `0x1`** (R3) — measured, not guessed; the layout is otherwise
  identical. The `build_frame` assert cites it as measured, because shipping "unmeasured" in an
  assert message after measuring it would ship a lie.
- **R1 cleared before the arm was written**: `crashy` installs no `SIGSEGV`/`SIGBUS` handler before
  faulting (none in source; zero `num=46` in a live trace), so M6's `crashy_e2e` was never at risk of
  flipping from `Crash` to delivery. M11's R3 treatment, applied again.

**What driving it actually found — five defects that would have shipped broken behaviour.**
Twenty-three plan defects surfaced across eleven tasks; **not one was found by reading the plan** —
every single one came from executing it. The five that mattered:

1. **Delivery would never have entered the handler.** The plan said to set `ELR_EL1` to the
   trampoline, "the mirror of `set_x0_err_and_return`". Backwards: that function *reads* `ELR_EL1`
   and *writes* `reg::PC`/`reg::CPSR`. Nothing `ERET`s — the VMM parks at the trap and resumes via
   `reg::PC` — so writing `ELR_EL1` is inert, and the plan omitted `CPSR` entirely. Falsified by
   implementing the plan's version and running it rather than by arguing: the guest entered `0x4404`,
   **inside the exception vector table**, with the stale parked `CPSR` `0x3C5` (EL1h) instead of the
   guest's genuine EL0t. The plan contradicted its own test, and the test — written against observed
   behaviour — was the better evidence.
2. **`uc_onstack` would have lied on the alt-stack path**, telling a handler it was *not* on its
   alternate stack while it was. Invisible until a real guest read it back with
   `sigaltstack(NULL,&old)` from inside a handler.
3. **A caught self-raise reported itself to the guest as failed.** `deliver_signal` reads the frame's
   registers live, so it captured `x0` = the pid passed to `kill()` rather than the syscall's return
   value. Measured with a new probe: the kernel snapshots the context *after* completing the syscall
   return (`kill()` returned 0, frame `x0` = 0, and `PSTATE.C` did not survive while `Z` did). Every
   freestanding fixture overwrites `x0` before observing it, so no gate guest could have caught this
   — it would have first bitten under real libc as an invented error path, the quiet shape rather
   than the loud one.
4. **The replay-side `sigreturn` mirror as planned would have clobbered the registers it had just
   restored**, by running `apply_and_return` after the hook.
5. **A static guest could not execute a single NEON instruction — and that asymmetry predates M12 by
   eleven milestones.** `load_with_pac`, the static *record* path, never set `CPACR_EL1.FPEN`, while
   `restore()` — which every *replay* goes through — always has, as do `load_dynamic` and
   `from_checkpoint`. So a static guest using a vector register would **fail to record while
   replaying fine**: exactly the class symmetry rule 1 exists to prevent. Latent since M0 because no
   static fixture used a vector register until `vecsurvive` needed one. One line, matching the three
   existing sites.

The headline gate itself then failed twice before it passed, and both were the plan's test rather
than the mechanism:

- **It read the `sigaction` struct one landmark too early.** A coordinate `(N, 0)` is the state after
  `N` events have been *consumed*, so event `N` is still to come and the guest sits at the **start**
  of the window leading to it — before the stores that fill `struct __sigaction` have run. The
  alarming reading was that replay's memory reconstruction disagreed with record's at the same
  address, which would have been a real seek defect; record reads that identical `args[1]`. Falsified
  by printing both coordinates: at `(li, 0)` the struct reads `handler=0x4000, mask=0x27ff7a8` — a
  *stack address* sitting in the mask field, which is what uninitialized memory looks like — and at
  `(li + 1, 0)` it reads `handler=0x10001fa48, flags=0x441`, matching the delivery exactly. retrace
  was right at every step.
- **It expected the wrong terminal event.** The plan asserted `Event::Signal`; a fault-derived death
  whose disposition is no longer a handler goes down M6's `Event::Crash` path byte-for-byte
  unchanged, because it really is a fault and the hardware really did produce the ESR. Recording it
  as a `Signal` would be the exact mirror of the lie M11 refused when it declined to fold
  `Event::Signal` into `Crash`.

And one that is not a defect but a new shape: **the first mid-run two-event landmark pair.** A caught
raise writes `Syscall` *then* `SignalDelivery` at a single stop, so the coordinate between them names
a position the guest never occupies — the syscall is completed and the frame written as one
indivisible transition. `advance_to_landmark`'s `while self.idx < n` would have overshot and returned
`Ok`: a debugger seek silently at the wrong position. Now a named `Divergence`, with a test that also
pins that the landmark *after* the pair still seeks, so the guard refuses exactly one coordinate
rather than breaking seeks past deliveries.

**The gate set.** The mechanism is proven by freestanding asm guests that supply their **own**
trampoline, deliberately, so they test retrace's entry contract with libc out of the way: `sigframe`
(the `x0..x5` and `sp` contract, six named fields with six distinct exit codes), `segvcatch` (the
handler advances `__ss.__pc` and returns — the only gate proving `sigreturn` restores *mutated*
state), `altstack` (`SA_ONSTACK` honoured, the handler asserting its own `sp` lies inside the
alternate stack), `vecsurvive` (a known value in a vector register survives the round trip), and
`blockedfault` (fail-loud). `altstack` is mandatory precisely *because* the headline does not prove
alt-stack handling — a wild-pointer fault runs perfectly well on the main stack, so `SA_ONSTACK`
could be ignored entirely and the headline would still pass. That is honest-gate discipline applied
to a gate that passes. `sigcatch_dyn_e2e` is then the only gate that runs through **Apple's real
`_sigtramp`**, since libc's `sigaction()` overwrites `sa_tramp` with its own no matter what the
caller puts there. `crashy_e2e` is unchanged: an *uncaught* fault is still an `Event::Crash`.

**The determinism posture is standard symmetric** — record puts the frame bytes in `writes`, replay
recomputes them through the *same* `deliver_signal` and byte-compares before applying. Both sides
call one implementation, so "record and replay recompute identically" is true by construction rather
than by discipline. That mirror is load-bearing and it is proven so: drop replay's
`complete_syscall_before_delivery` and it fails with `first differing byte at frame+176: recomputed
0xa1 != recorded 0x00` — `__ss.__x[0]`, the low byte of the pid. The diagnostic names the field
because reporting only lengths and IPAs would be useless when both frames are 976 bytes at the same
address, which is the only mismatch that can occur.

**Delivery is a first-class trace event, not below-the-trace emulation.** Symmetry rule 2 would
otherwise suggest hiding it inside `Box_::run()`, but rule 2's precedents (the timebase MRS, the
Apple-IMPDEF undef-MRS, the B-family FPAC strip) are *instruction* emulations — micro, high-frequency,
semantically invisible. Entering a handler is a *control transfer*: macro, rare, and the loudest
thing that happens in a run. "Rewind to where the signal was delivered" is a query a reverse debugger
should answer, so `the_delivery_is_a_seekable_landmark` tests the payoff rather than claiming it.
`TRACE_MAGIC` `0x0005`→`0x0006`; no fixture is checked in, so nothing was invalidated.

**The new boundary: `PROT_NONE` enforcement, and it is now the top deferred item.**
`commit_reserved_page` silently demand-commits any page inside a tracked reservation, and `prot` is
ignored except `PROT_EXEC`. libstd's `install_main_guard` maps its stack-overflow guard page
`PROT_NONE MAP_FIXED` — so **in the guest that page does not guard**, and a Rust stack overflow grows
straight through it instead of faulting. That is why the headline guest uses a wild pointer rather
than the stack overflow that would otherwise have been the obvious choice. Making it fault needs real
page-table permissions plus a fault path that separates "reserved and committable" from "reserved
`PROT_NONE`, must fault" — a milestone's worth of work, and the obvious M13.

Also unmodelled and fail-loud rather than guessed, each named at the point it asserts: **a blocked
synchronous fault** (POSIX leaves it undefined and Darwin force-delivers; M11 models no pending set,
so guessing would be a plausible lie), **a fault taken inside a handler** (nested delivery), and
**`sigreturn` with a bad token or one asking for PSTATE mode bits**. `PSTATE` is sanitized on
restore: `cpsr` comes back from a frame in guest-*writable* memory, so the restore masks to
user-settable flags and never touches mode — the only place in M12 where guest-controlled bytes reach
a system register. **`SA_RESTART` is unreachable by construction** (M12 delivers only synchronously,
at a fault or a self-raise, and never interrupts a blocking syscall) and is documented rather than
implemented.

Everything else from M11 carries forward unchanged: a **pending signal set** is still unmodelled (so
`sigpending` returning empty stays true by construction), `sigsuspend`(111)/`__sigwait`(330)/
`terminate_with_payload`(520)/`abort_with_payload`(521) remain asserts, `__pthread_kill`'s thread-port
operand is wired but ungated, **`dup2` fail-loud**, **`fcntl(F_DUPFD)` unmodelled and *not*
fail-loud**, guest stdin is still retrace's, `RLIMIT_NOFILE` unenforced, `guest_munmap`'s
wholesale-drop defect, **threads**, **asynchronous signals from outside the guest** (nondeterministic
by nature — they need an explicit injection model), and **arm64e guests**, whose frame thread-state
is PAC-signed.

**One measured property that is not M12's, recorded so it is not rediscovered painfully.** The
**event count of a recording is not reproducible across runs**: `segvy` produced 258/262/263/268
events over five recordings of the same guest — same exit, same stdout, same structure, shifted
wholesale. A control run attributes it rather than assuming: **`hello_rust`, a headline gate green
since M8 and untouched by this milestone, varies too** (257/257/258). So this is pre-existing, and
presumably libmalloc's entropy-derived placement (cf. M2-carveout). It costs nothing today — no gate
asserts event counts, replay is always against one specific trace, and `segv_rust_e2e` asserts
structure only, verified stable over three consecutive runs — but it bounds what any
recording-against-recording oracle can ever check for a dynamic guest, which is a sharper limit than
M11's "these guests call `getpid`" already implied.

A follow-up noted rather than done: `vecsurvive` is now the only coverage for static-guest FP/SIMD,
and its *name* says it is about signals. If `CPACR` regresses, a signal test fails. The failure text
names `EC=0x07`, so it is diagnosable, but a dedicated one-instruction static fixture would separate
the two mechanisms the way `sigign`/`sigraise` separate theirs.

See `docs/superpowers/specs/2026-08-06-retrace-m12-signal-delivery-design.md`.

## Status: M13-protnone — 🎉 the guard page actually guards

**A `PROT_NONE` page now denies EL0 in hardware.** Before this milestone the guest's own protection
calls were a polite lie: `guest_mprotect` discarded `prot` and re-opened the range, `map_mmap_region`
ignored every bit but `PROT_EXEC`, and `mach_vm_protect` returned `KERN_SUCCESS` without touching a
page table. A new stage-1 attribute (`ATTR_NONE`, AP `0b00`), a tracked no-access map, and
`protect_none`/`unprotect` wired through **all three** protection call sites make the guest's request
real. **`just gate`: 311 passed / 0 failed / 1 ignored** (94 test binaries), clippy clean.

**M13 deliberately ends with `1 ignored` — the first non-zero ignored count since M2-taskinfo.** That
is `stackoverflow_rust_e2e`, parked at M8 spec risk R3, and it is honest-gate discipline rather than a
regression. It is billed below, not buried.

**The headline.** `rs/protrust.rs`, a stock full-`std` Rust binary, `mmap`s a page RW, touches it,
`mprotect`s it `PROT_NONE`, and stores through it. The store takes a **stage-1 permission fault** —
something no guest could produce in twelve prior milestones — which routes through M12's delivery into
libstd's *own* `SIGBUS` handler, and the guest dies of the re-executed store. Recorded and replayed
bit-for-bit, twice. The pre-protect touch is load-bearing rather than decorative: it puts a
**writable translation in the TLB**, so `protect_none`'s flush has to actually take. Delete the flush
and the guest prints `survived` and exits 0.

**Exit 139 is necessary and nowhere near sufficient, and the gate says so.** This is the same trap
`segv_rust_e2e` documented, one milestone on: an *unprotected* store to a wild address kills this
guest just as dead with M13's enforcement entirely absent, and M6's flat crash convention
(`Outcome::Crash` → `exit(139)`, whatever signal it maps to) means the code carries no information
about *which* fault occurred. So the gate asserts on the trace instead: **DFSC `0x0f`** (permission,
level 3) rather than `0x04..=0x07` (translation) — exactly the difference M13 creates — the FAR
masked to the protected page, `(SIGBUS, BUS_ADRALN)` from `signal_of_esr`, exactly one
`SignalDelivery` whose `si_addr` names that page, a `sigreturn` after it, and `resume_pc ==` the
terminal crash `pc`. The protected page is **learned from the guest's own recorded `mprotect`, not
hardcoded** — and learning it is itself a measurement: the run contains **four** `mprotect(…,
PROT_NONE)` calls and three are libSystem's own startup work, so the gate takes the **last**, which is
the guest's. Take the first and you assert against libpthread's guard at `0x38000`.

**The signal was measured, and the measurement contradicted the shipped table.** `spikes/protnone.c`,
compiled and run natively: a `PROT_NONE` access — **load and store alike** — raises
**`SIGBUS`/`BUS_ADRALN`**, not `SIGSEGV`/`SEGV_ACCERR`. `signal_of_esr`'s permission row said
`SIGSEGV`, the Linux-shaped guess, and it had **never been reached in six milestones** — every fault
any guest had ever recorded was a *translation* fault (`0x04..=0x07`). So the row was wrong and
nothing could have noticed, which is precisely what an unexercised branch is for. The spike carries
its own control: an access to a wholly unmapped address still raises `SIGSEGV`, so M6's `crashy_e2e`
classification is unaffected. (Informational, not consumed: a store to a `PROT_READ` page also
returns `BUS_ADRALN` — XNU does not distinguish "no permission" from "wrong permission" in `si_code`,
despite neither access being misaligned.)

**The hardware separates "committable" from "must fault" — no software gate does.** A protected page
is **backed**, so an EL0 access takes a stage-1 permission fault via the EL1 trampoline and arrives as
`Stop::Fault`, where M12's disposition check runs. A reserved-but-uncommitted page is **unbacked**, so
its access takes a stage-2 translation fault direct to EL2 and arrives as `Stop::Other`, where
`commit_reserved_page` demand-commits it. Two exception routes, two `Stop` variants, and the split is
free. It rests on one invariant, which `protect_none` **asserts** rather than assumes: every page it
protects must already be backed. Protecting an uncommitted reservation would fault at stage 2, where
`commit_reserved_page` would silently materialize the page instead of denying it — so that case fails
loud, and `protreserve.s` is the gate that proves it does.

**The guest-side TLBI finally has a caller that needs it.** M9 built `flush_guest_tlb` for exec
promotion and then found jq never used it; every other `set_region_attr` caller stamps an IPA the
guest has never translated, and each documents that as its soundness argument. `protect_none` is the
first that stamps a page the guest is **actively using** — libstd's guard lives inside the stack it is
running on. Its non-vacuity is measured, not argued: reverting the flush makes `protnone.s` report
"the protected store was NOT denied," verbatim. The pairing is asymmetric and the README says so
where the plan did not: `protrestore.s` (the `unprotect` direction) **wants** its store to succeed, so
it passes vacuously when nothing is protected at all. Only `protnone.s` proves the forward direction.

**The stack-overflow capability is PARKED, not delivered.** M12's Status section named a Rust stack
overflow as the obvious M13 headline. Measurement killed it. libstd computes its guard at
`pthread_get_stackaddr_np() - pthread_get_stacksize_np()`, and macOS 26's libpthread reports a
**constant `0x7fc000`**, so the guard lands at `0x2004000` — **7.73 MiB below** retrace's real 256 KiB
stack backing `[0x27C0000, 0x2800000)`. A deep recursion therefore runs off the stack into unbacked
IPA and takes a **stage-2** fault — a fatal `describe_stop`, not even a guest-visible signal — instead
of striking the guard. That is **M8 spec risk R3**, already documented at
`crates/retrace-box/src/lib.rs:35-53` with both fixes already measured and rejected there: backing a
full 8 MiB costs ~1.7x on `hello_rust` and worse across the dyld suite, and `getrlimit` cannot move
the subtrahend (M8 measured that answering `0x10000000` left the computed address bit-identical). So
`stackoverflow_rust_e2e` ships as **real, compiling code with a real guest behind it**, `#[ignore]`d at
that wall — because a gate that cannot be run cannot be un-parked by deleting an attribute. Forced
with `--ignored` it dies exactly where R3 says: a stage-2 translation fault 160 bytes below the stack
bottom, nowhere near the guard. **The enforcement mechanism is not what is missing** — the headline
gate observes that very guard page being installed at `0x2004000` and protects a different page to
prove enforcement works.

**A correction to M12's Status section, because this milestone's premise rested on it.** M12 wrote
that libstd's `install_main_guard` maps its guard page `PROT_NONE MAP_FIXED`. Measured, it does not:
the `mmap` **is** `MAP_FIXED` at the guard address but carries `PROT_READ|PROT_WRITE`, and the
`PROT_NONE` arrives from a *subsequent* `mprotect`. The consequence inverted two tasks' stated
significance — `guest_mprotect` is what makes libstd's guard fault, and `map_mmap_region`'s `prot == 0`
hook is **not** on libstd's path at all. Both remain necessary; the reason each is necessary changed.
The same measurement also identified which page is libstd's: `0x38000` and `0x43c000` appear in
`hello_dyn` too and are libpthread's own guards, while `0x2004000` appears only under libstd.

**What driving it actually found — fourteen plan defects, and the one that mattered was found by a
review that was nearly skipped.** Tasks 7–10 landed controller-implemented without independent review,
for defensible reasons (dispatched subagents kept stalling). The back-fill review of all four found
three clean and **one Important defect in Task 8**: `guest_munmap` dropped a range from the no-access
map with a bare `subtract_range` and never reset the stage-1 leaf. Stage-1 leaves live in the box's own
tables and **survive a stage-2 unmap** — `guest_munmap`'s own comment says so — so munmap-then-remap
left a valid new mapping holding a stale `ATTR_NONE`: a silent denial the guest can neither see nor
undo, in the exact milestone whose purpose is removing silent denials. `unmap_overlapping` had the same
gap and touched neither the map nor the leaf. Fixed with one `drop_protection` helper serving both
teardown paths. **Why the existing test missed it is the transferable lesson:**
`munmap_drops_the_protection_with_the_pages` asserted only `noaccess().is_empty()` — the *bookkeeping*
side — while every other test in that file also checks `ipa_is_noaccess`, the *hardware* side. **A test
that checks only the software mirror of a hardware fact will pass while the hardware disagrees.**

Three more worth keeping:

1. **Task 7 detonated a latent VMM bug that predates M13.** Its targeted 17/17 was green while the
   full gate was **red** at the first dynamic guest it reached. The three VMM scratch IPAs (sign stub
   `0x40000`, sign table `0x44000`, TLBI stub `0x48000`) are contiguous and **lazy**, so until first
   use they are absent from `backings`, `range_is_free` counted them free, and first-fit handed
   libsystem a 4 MiB thread-stack extent at `0x38000` straddling all three. M13 did not create that —
   it forced the first TLBI-stub creation to happen *after* guest allocation on the dynamic path, which
   nothing had ever done. Fixed with a forbidden `[0x40000, 0x4C000)` window, a pure function of
   constants and so identical on both runs. The sibling hazard it exposed is worse than the one that
   fired: `ensure_sign_stub` early-returns when its IPA is backed, so a guest extent covering `0x40000`
   would have made it **skip creating the stub and sign against guest memory** — a silent wrong answer
   rather than a loud `HV_ERROR`.
2. **Task 9's only planned test could not fail for the reason Task 9 existed.** It drove the dispatch
   itself from its own match arm, so it passed identically before and after the change — it pins the
   arg layout, which is worth pinning, but as the task's sole gate it would have let a completely
   unwired `mach_vm_protect` go green. A real record-and-replay gate was added, and **both** halves of
   symmetry rule 1 were measured by deletion: drop the record arm and the guest reports
   "protection not enforced"; drop the replay arm and replay reports `Divergence { landmark: 3 }`.
3. **Task 10's planned guest wrote `cur_protection` to the wrong register**, zeroing x5/x6/x7 alike so
   that its `args[7] == 0` assertion would have passed **vacuously** while documenting the wrong ABI.
   The trap passes `cur_protection` in x5, as `vm_map_args` has always read it.

**The determinism posture is standard symmetric, with no new mirror.** `mach_vm_protect` is routed
into the same `guest_mprotect` both sides call, so "record and replay recompute identically" is true by
construction. No `TRACE_MAGIC` change: M13 adds no event shape.

**The retained deviation, measured rather than assumed.** `commit_reserved_page` still silently
demand-commits any page inside a tracked reservation, and M13 keeps that deliberately. Its cost is
**zero, three runs each**: `hello_rust` 0/0/0, `hello_dyn` 0/0/0, `jq --version` 0/0/0. A zero from a
broken probe is indistinguishable from a real zero, so the instrument was proved against the static
`reservecommit` fixture whose whole purpose is that path — it reports two commits. The path is live
but no dynamic gate depends on it.

**`mach_vm_protect`'s routing is dormant, and that was checked before it was written.** `hello_rust`
issues 47 `mach_vm_protect` calls with `new_protection` in `{0x1, 0x3, 0x13}` and **never** `0`, so
routing it into the box alters no live behavior in any dynamic gate. It is wired for the guest that
eventually needs it, not for one that does today.

**Still unmodelled, and fail-loud rather than guessed:** every protection bit other than **no-access**
— `PROT_READ`-only, `PROT_WRITE`-only, and executable transitions are not modelled, and `unprotect`
restores `ATTR_DATA` unconditionally rather than the prior attribute (sound only because nothing but
data pages are ever protected today, and documented as a choice); and **protecting an uncommitted
reservation**, which asserts. Everything M12 carries forward is unchanged: a **pending signal set**,
**nested delivery**, a **blocked synchronous fault**, `dup2` (fail-loud), `fcntl(F_DUPFD)` (unmodelled
and *not* fail-loud), guest stdin still being retrace's, `RLIMIT_NOFILE`, **threads**, **asynchronous
signals**, and **arm64e guests**.

**Three fast-follows carried out, none blocking, all pre-existing or dormant:** `place_fixed` /
`unmap_overlapping` still never consult the forbidden scratch window before claiming an IPA (out of
reach today — no guest FIXED-maps below 4 GiB, since dyld's segments are ≥ 4 GiB); `protect_none` does
not dedupe overlapping protect calls, so protecting the same range twice tracks two entries (harmless
— the stamp is idempotent and `subtract_range` scans the whole table); and `guest_munmap` removes a
backing wholesale but drops protection only over `[ipa, len)`.

See `docs/superpowers/specs/2026-08-08-retrace-m13-protnone-design.md`.

## Status: M14-threads — 🎉 a guest with two threads of control

**A stock `std::thread::spawn` + `join` Rust guest now records and replays bit-for-bit.**
`rs/threadrust.rs` prints `main before spawn`, spawns a child that prints `child ran` and returns
`42u32`, joins it, and prints `joined 42` — recorded, then replayed byte-identically, twice.
`Box_` gained a thread table, an emulated `bsdthread_create`/`bsdthread_terminate`, a `__ulock_wait`/
`__ulock_wake` pair, and a cooperative block-driven scheduler. **The gate: 342 passed / 0 failed /
1 ignored** (96 test binaries), clippy clean over `--workspace --all-targets` with `-D warnings`.

That total was **measured in chunks, not by one `just gate` run**, and the distinction is recorded
rather than smoothed over: a single `cargo test --workspace` has been killed on this machine twice
this milestone (once at the 10-minute tool timeout, once mid-run), so the number comes from
`--workspace --exclude retrace-box --exclude retrace` (93), `-p retrace-box` (152), and the 43
`-p retrace` test targets plus `--bins` run one invocation at a time (97). It **reconciles** against
the milestone's own checkpoints rather than being taken on faith: Task 7's measured 326, plus 5 from
Task 8, 8 from Task 9, 1 from Task 10's F-1 follow-up, and 2 from Task 11 — and 95 binaries plus
`thread_rust_e2e` is 96. Every gate-count *projection* the plan carried was retired as it drifted;
this is the measured figure.

**`joined 42` is the whole assertion, and the gate says why.** Exit 0 proves nothing — a guest that
never spawned also exits 0, the trap `segv_rust_e2e` documented and `protnone_rust_e2e` sharpened.
That one line can be printed only if the child genuinely **ran** on retrace's single vCPU *and* its
return value **crossed back** through `join`. The gate also asserts the `bsdthread_create` event is in
the trace (libstd did not optimize the spawn away) and that two replays are byte-identical — which is
where a nondeterministic schedule would surface, since a different interleaving reorders the guest's
own writes.

**The single vCPU is a gift to a replay engine, not an obstacle.** Real threads on real cores are the
classic source of replay nondeterminism. N guest threads multiplexed onto one vCPU by a scheduler that
is a pure function of the guest's own syscall sequence are deterministic *by construction*. So the
schedule is **regenerated, never recorded** — it joins cache pages, the timebase and PAC keys as
things M0's principle says to recompute rather than store. **Nothing was added to the trace and
`TRACE_MAGIC` did not move.** The scheduler lives inside `Box_::run()` and `Box_::step()`, below the
trace, per symmetry rule 2, which is *why* determinism is automatic here rather than argued for.

**M13's `mach_vm_protect` routing, billed in its own Status section as dormant, was this milestone's
prerequisite.** M13 measured that `hello_rust` issues 47 such calls and never with
`new_protection == 0`, and wired the routing "for the guest that eventually needs it, not for one that
does today." That guest is this one, and it needs it **one trap before the wall**: libpthread maps the
new thread's stack, `mach_vm_protect`s its guard page `PROT_NONE`, *then* asks for the thread. Two
milestones were coupled without either knowing it.

**The registration half of threading had been working, unremarked, since M7.** Measured on
`hello_rust` — a guest with **no** threads — `bsdthread_register` (366) fires once and `thread_selfid`
(372) twice, both surviving silently on **every dynamic guest retrace has ever run**. libpthread hands
the kernel its thread-start trampoline at startup regardless, so the address the kernel is supposed to
enter a new thread at had already been handed over eight milestones ago. M14 was therefore narrower
than "implement threading" — and correspondingly, the box now *captures* that trampoline rather than
letting it pass.

**Forwarding `bsdthread_create` is not a hazard to weigh — it is a 100%-reproducible, whole-process
crash, and the spike said so before a line of box code was written.** The design spec called it a
*maybe* ("the host **may** be creating a real thread… starting at a guest address"). Measured over 40+
runs, both halves of that were wrong. The host does create a genuine OS thread inside retrace's own
process, entering **retrace's own** `_pthread_start` with `x0` pointing into guest backing memory — and
it never reaches guest code at all. It dies three instructions in, at libpthread's `brk #0xc473`, on a
PAC self-check of the pthread-struct pointer whose bytes were signed under the *guest's* key domain.
`SIGTRAP` with default disposition kills the whole process, which is why retrace's own main thread died
mid-serialize with nothing printed. That is a much stronger argument for emulate-never-forward than the
spec had, and it is now an assert.

**The measured ABI overturned the plan's, and the plan had pre-authorized being overturned.** The
classic `_pthread_start(self, kport, fun, funarg, stacksize, pflags)` shape — which the plan's own
Task 7 code encoded, seeding `x0`–`x4` — is **not** what macOS 26 does. `__pthread_start` reads only
**`x0`** (the pthread struct) and **`w5`** (flags) before dispatch; `x1`–`x4` are never touched, and
`func`/`arg` are loaded *from the struct* at `+0x90`/`+0x98` (`ldp x8, x0, [x19, #0x90]` immediately
before `blraaz x8`). The guest's own `_pthread_create` already stored them there before trapping, so
the box seeds two registers and populates nothing. The Task 7 test now asserts `x1 == 0` explicitly, so
a future implementer cannot quietly restore the guess. This is M13's Task-10 failure mode — a planned
guest writing the wrong register, whose assertion would have passed vacuously — **caught before it
shipped rather than after**.

**The headline's wall: emulating a syscall's *entry contract* is not the same as emulating the
syscall.** With Tasks 4–9 complete, the guest still failed — and not subtly. `pthread_join` returned
**success without the child ever running**, and libstd panicked `threads should not terminate
unexpectedly` because `Arc::get_mut` needs `strong_count == 1`. `RETRACE_TRACE=1` showed `360` firing
once and then nothing thread-shaped at all: no `515`, no `516`, no `361`. Task 7 had gotten every
register right; three of the kernel's **side effects** had no owner:

1. **The child's mach port at `pthread + 0xf8`.** `__pthread_join` does not unconditionally wait —
   `ldr w9, [x19, #0xf8]` makes the kport the wait value, and `cbz w8` **skips `__ulock_wait`
   entirely** when it is zero, deallocates, and returns success. The only two userspace writers of
   `+0xf8` in libsystem_pthread are off the `pthread_create` path, and a host probe read the field with
   the child provably not yet run, 5/5: `[+0xf8]` already equalled `pthread_mach_thread_np(t)` while
   `[+0x34]` was still 0. The kernel writes it; now so does the box.
2. **`TPIDRRO_EL0` is `pthread + 0xe0`, not `pthread`.** With the kport written the child ran for the
   first time and died two instructions in at `brk #0xb001` — *"BUG IN LIBPTHREAD:
   thread_set_tsd_base() wasn't called by the kernel"*. libpthread reads the register back the other
   way (`mrs x23, TPIDRRO_EL0` / `sub x21, x23, #0xe0`). Measured 4/4, main and child alike.
3. **`w5 |= PTHREAD_START_TSD_BASE_SET`** (bit 28) — the kernel's own assertion that it set the TSD
   base, and the `tbz` that produced that brk. ORed onto the guest's flags, never substituted.

(2) and (3) are one kernel behaviour and were fixed together deliberately: setting the flag while
leaving the base wrong would be the box asserting something it had not done. The transferable shape:
**when a spike measures behaviour on the host, list what the KERNEL contributed to that measurement,
because that list is exactly what the emulation must reproduce.** Task 1's spike correctly concluded
that `join` blocks on `__ulock_wait`, and that conclusion silently carried the precondition
`kport != 0`.

**The synthetic port needs no determinism exception, unlike M2-xpcport's.** It is
`GUEST_THREAD_PORT_BASE | tid` = `0x0BAD_7000 | tid`, a pure function of the guest's syscall sequence,
so record and replay compute the identical byte at the identical address and **nothing is recorded**.
A *real* kport would be host-allocated and therefore nondeterministic — exactly why M2-xpcport had to
take a deliberate record/replay asymmetry for its minted bootstrap port. This needs none, because
nothing outside the guest ever dereferences the name: libpthread uses it only as the `__ulock_wait`
comparison value at `pthread+0x34` and hands it back verbatim in `bsdthread_terminate`'s `port`
argument, both inside the box.

**The wait/wake correlation is address equality, and it was measured rather than fabricated.**
`__pthread_join` computes `add x21, x19, #0x34` before `___ulock_wait`; `__pthread_joiner_wake`
computes `add x1, x19, #0x34` before `___ulock_wake`. **Same word, `pthread + 0x34`** — so matching a
blocked `Wait{addr}` to a wake needs no address→thread-index correlation, which is what Task 8 had
believed was missing and unmeasurable. Also load-bearing and measured: `__pthread_terminate` calls
`joiner_wake` *before* `___bsdthread_terminate`, so the joiner is `Runnable` before the child is
`Exited` and no deferred-wake queue is needed.

**A wrong syscall number, sitting unexercised in the plan, found the way this project keeps finding
them.** The plan called 516 `__ulock_wait2`. Fresh disassembly says **516 is `__ulock_wake`** (`mov
x16, #0x204`) and `__ulock_wait2` is **544**; the SDK's `sys/syscall.h` confirms all three. Worse, 516
appeared nowhere in `retrace-arch` or `retrace-core` at all, so the guest's own wake call was falling
through to the generic arm and reaching `forward_and_diff` — **issuing a real `__ulock_wake` from
retrace's own process against a guest address**, the precise hazard class that makes 515 unforwardable,
applied to 515's other half.

**And then the guard against that very class of bug was itself dropped, which is worth recording
rather than quietly fixing.** The new `SYS_ULOCK_WAKE` was immediately noticed to be the one thread
syscall number *not* pinned by `thread_syscall_numbers_are_the_darwin_ones` — this project's whole
discipline for syscall numbers is that SDK cross-check — and it was routed to "the next fix round."
That round ran and addressed five other findings; **this one fell out of it and shipped unpinned
through Tasks 9, 10 and 11.** The milestone's close caught it: 516 is now in the tuple, cross-checked
against `MacOSX.sdk/usr/include/sys/syscall.h` lines 555/556, and **mutation-verified** rather than
assumed — set the constant to 544 (the `__ulock_wait2` number the plan had confused it with) and the
test fails `544` against `516`. The transferable part is not the one-line fix but the shape: a finding
parked on a *later* task's fix round has no owner, and nothing in the process notices when that round
closes without it.

**`ULF_NO_ERRNO` — "the library compares against `-4`" and "the kernel returns `-4`" are different
claims.** Both operation words `__pthread_join` can pass set bit 24 (`ULF_NO_ERRNO`), under which XNU
returns **`-errno` in `x0` with carry CLEAR**, not `+errno` with carry set. retrace was doing the
latter, sending the guest into libsyscall's `cerror` so that `join`'s own `cmn w0, #0x4` missed and it
re-waited forever. The review argued this from libpthread's comparisons — sound, but that is evidence
about what libpthread *expects*. The fix round measured **XNU directly** with a raw-`svc` probe
(libsyscall's stub branches on carry and would have destroyed both facts), **including a control with
the flag cleared**:

```
op=0x01000002  x0=0xfffffffffffffff2  w0=-14  C=0
op=0x01020002  x0=0xfffffffffffffff2  w0=-14  C=0
op=0x00000002  x0=0x000000000000000e  w0=+14  C=1   <- control: flag cleared, what retrace was doing
```

The control is what proves the flag is the distinguishing bit. `guest_ulock_wait`'s signature collapsed
from `Result<u64,u64>` to a bare `u64` as a result — with no `Err` variant, no dispatch arm can record
`err: true`, which makes the bug **unrepresentable** rather than merely fixed.

**Break the call site, not just the callee.** Task 10's job was proving the scheduler non-vacuous.
Corrupting `pick_next` to `Some(0)` failed 8 tests, so the scheduler's *logic* was well covered. Its
*wiring* was not: **deleting the call site in `run()` outright passed the entire crate, 150/150.** No
test in `threads.rs` called `run()` at all, and for a single-threaded guest `needs_reschedule()` is
false by construction, so the branch was already a no-op on every M0–M13 path — the compatibility
argument's exact cost. A `run()`-level test now closes it, with **both** its assertions mutation-proven
against **different** defects (delete the reschedule → `current()` reads 0; keep it but drop `load_ctx`
→ `current()` passes and only the returned `Stop`'s syscall number notices). Mutation-testing only the
first would have left the second assertion vacuous in the same sense.

**This milestone's signature defect was the test that cannot fail for the property it names — six
times.** `pick_next`'s exited-thread test (a mutant matching `Runnable | Exited(_)` passed all six
tests); the context-switch round-trip that left `pc`, `sp_el0`, `cpsr`, all 32 FP registers, `FPCR` and
`FPSR` unasserted; the checkpoint test that never called `from_checkpoint` at all, so a restore
dropping the whole thread table satisfied it (risk R4 exactly); Task 7's fixture whose `stack` and
`pthread` literals were *equal*, so three assertions could not detect a swap; Task 8's test that
hand-installed a `Join{target}` state the production path never produces; and Task 9's PSTATE test that
read `0 == 0` because HVF's reset `SPSR_EL1` happens to be 0 on this host. Each was caught by mutation
rather than by reading, and the last one is the sharpest lesson: **Task 10's milestone-level
non-vacuity probe could never have caught it**, because breaking `pick_next` does not touch
`regs.cpsr`. A per-fix mutation test catches what one milestone-wide probe cannot.

**The plan was wrong about where syscall dispatch lives, and that is worth recording.** It placed the
per-syscall `match` in `retrace-box`. It is in `retrace-core`, in **two mirrored places** — `record_box`
and `ReplaySession::advance` — which is symmetry rule 1's whole shape. Four further compile-level
errors were found in the plan before any dispatch (`Regs` lives in `retrace-trace` with `sp_el0`;
FPCR/FPSR are `Reg` not `SysReg`; `checkpoint()`/`load()` not `capture()`/`for_test()`; `Regs` derives
no `Default`), and every gate-count projection in the plan drifted and was retired in favour of
measurement.

**Two things `BoxState` had to start carrying, both of which break quietly rather than loudly.** The
**thread table** (risk R4: a checkpoint that drops the non-current threads still restores and still
runs), and **`thread_start_pc`** — the registered trampoline, which a mid-run capture cannot re-derive
because the registering syscall sits *behind* the checkpoint. `from_checkpoint` also now sources
`TPIDRRO_EL0` from the restored table rather than the `TSD_IPA` constant, which was wrong the instant a
checkpoint is taken while a non-main thread is running. M4's three seek gates are unchanged.

**Honest limits, and the sharpest one is in the oracle.** **The determinism oracle has no thread
identity.** It compares `(num, args)`, so two threads running the *same code* — the normal case for a
thread pool — can issue byte-identical syscalls and replay would continue on the wrong thread in
silence. Today's schedule is deterministic by construction, so this is a missing *belt* rather than a
live defect, but the honest wording is that divergence is caught **whenever the two threads' next
syscalls differ** — probabilistic, not structural. The format-compatible place to fix it already
exists: `Event::Sched { thread, until }` is in `retrace-trace` with **zero producers and zero
consumers**, so a schedule oracle costs a landmark-index change and a replay arm, not a `TRACE_MAGIC`
break. Also carried: `guest_bsdthread_create` returns **0** where the real syscall returns the child's
`pthread_t` (accepted as an open risk at Task 7 and never bitten, since libpthread's caller only tests
for `-1`); and `run()` and `step()` each carry the reschedule check independently, with no shared choke
point — fine at two entry points, worth revisiting at a third.

**Still unmodelled, and named rather than discovered later:** `workq`/GCD thread pools (`workq_open`/
`workq_kernreturn` have never fired); real **preemption** — scheduling is cooperative and switches only
at a block or an exit, so **a guest that spin-waits without ever trapping runs forever**; **per-thread
seek and stepping**; **thread-aware watchpoints** (M5's reverse-continue-to-last-writer stays
thread-agnostic); the **per-thread signal mask** (spec open question 2 — M11 modelled *dispositions* as
process-wide, which stays correct per POSIX, but the mask is per-thread and `spawn`+`join` never touches
one); thread priority and per-thread signal targeting, which assert rather than answering plausibly;
and any claim about more than a handful of threads. Everything M13 carries forward is unchanged: every
protection bit other than no-access, a **pending signal set**, **nested delivery**, a **blocked
synchronous fault**, `dup2` (fail-loud), `fcntl(F_DUPFD)` (unmodelled and *not* fail-loud), guest stdin
still being retrace's, `RLIMIT_NOFILE`, **asynchronous signals**, and **arm64e guests**.

**No new gate is parked, because Task 11's wall was cleared rather than hit.** The plan reserved a
parked gate for a capability M14 could not reach; the headline went green instead, and no guest today
demands the spin-wait case that would need preemption. The gate count therefore still carries exactly
**one** `#[ignore]` — `stackoverflow_rust_e2e`, at M8 spec risk R3, unchanged and not newly parked.

See `docs/superpowers/specs/2026-08-12-retrace-m14-threads-design.md`.
