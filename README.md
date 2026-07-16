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
double pass (two independent runs, each a fresh recording). `just gate` reports **90 passed, 0 failed, 0
ignored**, clippy clean.

**Deferred:** checkpoints (a pure seek-time acceleration — deferred until a guest's replay time hurts);
watchpoints (4 hardware slots exist, unused); symbolication (debugger addresses are raw guest VAs); an
interactive REPL (only scripted sessions today); step-over and back-to-back `continue` on a single non-looping
breakpoint (step-over was de-scoped — a `continue` while parked *on* a breakpoint re-fires at 0 progress and
errors, exit 5); more than 6 breakpoints per session (the 7th+ is caught only at landmark boundaries); the
mid-window-vs-boundary K = 0 resolution edge (a boundary breakpoint interacting with the `K > K_cur` rule —
untested, the e2e uses a clean boundary hit); and the `Stop::Other`-while-stepping fault path (empirically
unreachable on `hello_dyn` — correct by construction, untriggered). See
`docs/superpowers/specs/2026-07-16-retrace-m3-reverse-execution-design.md`.
