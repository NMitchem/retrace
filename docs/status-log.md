# retrace — milestone status log

The append-only engineering record, M0 through M17. Every section below was written at the close of
the milestone it names and is preserved **verbatim**: nothing was rewritten, condensed, or deleted
when this moved out of `README.md`.

**How to read it.** Each entry is true *as of its own milestone*, not as of today. Where a later
milestone falsified or narrowed an earlier claim, the earlier section carries an inline
`(Superseded …)` or `⚠ SUPERSEDED` annotation naming the milestone that corrected it — so on any
one topic, **the newest entry wins**. Those annotations are the format working as intended, not
damage to it: a claim is left standing as its own milestone's honest account, with a forward
pointer, rather than being quietly edited into agreement with what came later.

For what runs **today**, see [`README.md`](../README.md) — that is the current-state document and
the one to trust for capability, limits, and gate status. For per-milestone design specs and task
plans, see `docs/superpowers/specs/` and `docs/superpowers/plans/`.

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
disposition, the blocked mask, and the alt stack. *(Superseded in part, and left standing as M11's
own account of what M11 built: **M16-threadsignal moved the blocked mask and the alternate stack off
`SigTable` onto `Thread`** in `crates/retrace-box/src/thread.rs`, because POSIX makes both
per-thread. `SigTable` holds the dispositions, which are correctly process-global, and nothing else.
See the M16-threadsignal Status section.)* It is a pure function of the guest's own calls, so
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
  *(Superseded by M16-threadsignal, and left standing as M11's own account of what M11 built: the
  operand is now **decoded and gated** — `Box_::thread_of_port` resolves it by reading
  `[pthread + 0xf8]` back out of guest memory, with no special case for main, and a port matching no
  live thread is fail-loud. The `mach_thread_self` suggestion turned out to be **unnecessary**:
  main's own kport reads back through the identical path (measured, `0x103`). See the
  M16-threadsignal Status section.)*
- **A pending signal set is unmodelled**, so raising a *blocked* signal asserts. That is what makes
  `sigpending` returning empty true by construction rather than a convenient lie — the two decisions
  stand or fall together, and whoever adds a pending mask must revisit both.
  *(Superseded by M16-threadsignal, and left standing as M11's own account: the pending set is now
  **per-thread state on `Thread`**, a blocked raise *pends* instead of asserting, and `sigpending`
  reports it. The two decisions did fall together exactly as this sentence predicted — the same
  milestone that added the pending mask is the one that stopped `sigpending` lying. See the
  M16-threadsignal Status section.)*
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
is PAC-signed. *(Superseded in part by M16-threadsignal, and left standing as M12's own account:
both the pending signal set and `__pthread_kill`'s thread-port gate are implemented there, so
`sigpending` no longer returns empty by construction. The rest of this list still stands. See the
M16-threadsignal Status section.)*

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
signals**, and **arm64e guests**. *(Superseded in part by M16-threadsignal, and left standing as
M13's own account: the **pending signal set** is modelled there, per-thread. See the M16-threadsignal
Status section.)*

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
*(Superseded in part by M16-threadsignal, and left standing as M14's own account: three items in this
paragraph fell there — the **per-thread signal mask** this paragraph names as its spec's open
question 2 (the mask, the pending set and the alternate stack all moved off `SigTable` onto `Thread`,
and `spawn` inherits the mask), **per-thread signal targeting** (it resolves the port rather than
asserting), and the **pending signal set**. Preemption, `workq`/GCD, thread priority and per-thread
seek all still stand — see M16's own carry-forward list. See the M16-threadsignal Status section.)*

**No new gate is parked, because Task 11's wall was cleared rather than hit.** The plan reserved a
parked gate for a capability M14 could not reach; the headline went green instead, and no guest today
demands the spin-wait case that would need preemption. The gate count therefore still carries exactly
**one** `#[ignore]` — `stackoverflow_rust_e2e`, at M8 spec risk R3, unchanged and not newly parked.

See `docs/superpowers/specs/2026-08-12-retrace-m14-threads-design.md`.

## Status: M15-threaddebug — 🎉 the debugger can name the thread that wrote the byte

**`reverse-continue` now walks backward to a store and says which thread made it.** A new guest,
`rs/watchthread.rs`, spawns a child; the child writes `CHILD_CELL`, main writes `MAIN_CELL`, both
write `SHARED_CELL`, and the guest prints all three addresses. Arm a watch on the child's cell
*after* the run has already finished — so only a genuine backward scan can reach it — and
`reverse-continue` finds the store while `where` answers `thread=1`, the child, on an address main
never touches. The debugger also grew a thread vocabulary: `threads` lists every thread with its
state and marks the current one, `regs <tid>` dumps a **blocked** thread's registers straight out of
the thread table, `where` labels its coordinate with the owning thread, and
`watch <addr> [len] [thread <n>]` scopes a watch to one thread. Underneath, `Event::Syscall` gained a
`thread` field and the divergence oracle now compares it. **The gate: 360 passed / 0 failed /
1 ignored** across 98 test binaries at `259a4db`, clippy clean over `--workspace --all-targets` with
`-D warnings`.

That total was **measured in chunks, not by one `just gate` run** — a bare `cargo test --workspace`
still gets killed on this machine, as M14's close recorded — with every chunk run `--no-fail-fast`
and all three returning `CARGO_EXIT=0`. The delta is the number that means something: M14 closed at
**342 / 0 / 1 over 96 binaries**, so M15 adds **18 passing tests and 2 test binaries and does not
move the ignored count.** Those 18 reconcile exactly against the per-task counts rather than being
waved through — Task 1 ×1, Task 2 ×3, Task 4 ×3 (one `thread_oracle` gate plus two `replay.rs`
signal-path tests), Task 5 ×2, Task 6 ×1, Task 7 ×4 (three `debug_cli` e2e plus one parser unit
test), Task 8 ×3, Task 9 ×1 — and the two new binaries are exactly
`crates/retrace/tests/thread_oracle.rs` and `crates/retrace/tests/thread_watch_e2e.rs`. Every
headline gate ran and passed by name in the log: `hello_dyn`, `hello_rust`, **both** `jq` gates (not
skipped — `/opt/homebrew/bin/jq` was present), `panic_e2e`, and M15's own
`reverse_continue_names_the_thread_that_wrote_the_watched_cell`.

**`TRACE_MAGIC` moved, so every recording made before this milestone is now unreadable.** `RT\x00\x06`
→ `RT\x00\x07` (spec risk R2, billed here rather than discovered): `Event::Syscall` genuinely gained
a field the new reader requires, so an old trace is not merely older, it is missing data. The
rejection is loud and clean rather than a misparse — `open_checked` returns "keep nothing" on a magic
mismatch — and both halves are pinned by tests, one asserting the *new* magic
(`magic_bumped_for_the_syscall_thread_tag`, renamed from the M12 reason it used to carry) and one
asserting a trace written with the *previous* magic is rejected whole. Between them, "forgot to bump"
and "bumped to the wrong value" are both caught. **If you have a `.bin` from M14 or earlier that you
were mid-investigation on, re-record it.**

**`Event::Sched` was not merely left unused — it was deleted.** M14's Status section billed it as the
cheap, format-compatible place a future schedule oracle could live ("zero producers and zero
consumers, so a schedule oracle costs a landmark-index change and a replay arm, not a `TRACE_MAGIC`
break"). **That line is superseded by this one.** Emitting it was considered and rejected on two
measured grounds: it would **silently renumber every landmark** (`N` is a flat `Vec` index, so
interleaving a `Sched` per switch shifts every subsequent landmark — and *without* a magic break,
since the variant already parsed under the old magic, which makes it worse than a loud break given
checkpoints are cached by landmark and `advance_to_landmark` is a public seek target); and **nothing
in either dispatch loop can see a switch**, because `run()`'s reschedule check lives inside `Box_`,
below the trace, so producing `Sched` would need either a new channel out of `run()` or the
scheduling decision duplicated into both dispatch loops — the exact duplication symmetry rule 2
exists to prevent. Since Task 3 was already editing that enum under a magic bump, a reserved-but-dead
variant with an undocumented `until` field would only invite a future reader to assume it was live.
The oracle got its thread identity from a field on `Syscall` instead, which is complete: the schedule
can change only when a thread blocks or exits, and both are syscalls, so every other landmark's
thread is "the thread of the most recent syscall landmark."

**The determinism posture did not move.** The recorded `thread` is a *recording of the output* of a
function replay recomputes anyway — the standard symmetric posture, where replay recomputes and
byte-compares, never consumes. `verify_thread` compares and returns a `Divergence`; it never sets the
current thread. The schedule is still regenerated, not replayed.

**The oracle's thread check covers all three landmark-consuming arms, and the first attempt covered
one.** `ReplaySession::advance` consumes a recorded `Event::Syscall` at three places, not one: the
caught-raise mirror and the `sigreturn` mirror each sit as their own `if` block *above* the generic
match and each `return` before ever reaching it. The original commit put the comparison inside the
generic arm only, which was invisible to every gate — reaching either mirror needs a guest that is
both threaded and signalling, and none exists. The fix is one `verify_thread` helper called from all
three sites, each call placed *after* that site's own `(num, args)` check rather than hoisted above
all three, so a genuine argument divergence still reports as itself instead of being masked by the
thread mismatch it caused. Both new call sites were mutated *independently*, and each failed exactly
its own test with the other staying green — proof the two are not piggybacking on one working check.

**The attribution claim rests on three mechanisms, not one, and Task 10 measured which catches what.**
It is tempting to write "a watch hit names the thread that wrote" as though the headline gate proved
it end to end. It does not, and the split is the subtlest thing this milestone learned:

1. **The divergence oracle is the first line of defence.** A broken `current_thread()` — the
   scheduler-state bug that would make every attribution wrong — diverges at the very next syscall
   landmark, before any CLI assertion runs. Task 10 confirmed this by mutation: hardcoding
   `current_thread()` to `0` fails four gates, and all four fail through the oracle
   (`thread 0 on replay, 1 recorded`), not through their own assertions.
2. **The headline gate proves the display path.** `where` reports the box's real `current_thread()`
   at a *resolved* coordinate rather than a constant. Substantive — hardcoding `cmd_where`'s printed
   id to `0` fails it at exactly that line — but it is a check of what the user reads, not a
   standalone catch of a scheduler defect.
3. **`Advance::Watch { thread }` is consumed by the per-thread scoping filter, and only by it.** The
   field is threaded through five `Advance` sites *and* through `RHit::Watch`/`RHit::WatchSys` into
   `reverse-continue`'s backward scan, so the scan filters on state **captured at the hit** rather
   than re-derived afterwards. Task 10 measured that hardcoding that field to `0` is **not** caught
   by the headline gate — `cmd_where` re-derives the thread from the live scheduler and never reads
   the field, and the gate's script never scopes a watch — and is not caught by Task 5's tests
   either. Task 8's `watch_thread_scoping_filters_the_others_write` is the sole catcher.

That third measurement is what forced the split. The plan had predicted the headline gate would catch
it, and it was wrong.

**Debug registers are vCPU-global, the cross-switch watch was correct by accident, and now it has a
test saying so.** `ThreadCtx` carries `regs`, `fp`, `fpcr`, `fpsr`, `tpidrro_el0`, `elr`, `spsr` —
and `save_ctx`/`load_ctx` touch exactly those, so `DBGWVR`/`DBGWCR`/`MDSCR_EL1` sit entirely outside
the scheduler's save/restore discipline. That leak is the behaviour we want (one vCPU, one address
space, so an armed watch keeps firing across switches and catches *any* thread's store), but every
M5 test predates M14 and no test anywhere had ever armed a `DBGW` across a `switch_to_thread`. Task 6
closes it at the **hardware** leaf, not the `watch_ranges` software mirror — M13's own Task-8 defect
was a test that checked only the mirror — via a `#[doc(hidden)]` accessor reading the three registers
straight back off the vCPU. All three assertions are independently mutation-proven: clobber
`MDSCR_EL1` alone in `load_ctx` and only the MDE assertion fails; clobber `DBGWCR0_EL1` alone and
only the enable-bit assertion fails; clobber `DBGWVR0_EL1` alone and only the address assertion
fails. Mutating one register cannot demonstrate that a bug in another would be caught, and the
brief's Step 3 had specified only one of the three.

**The fidelity caveat: one half is discharged, the other still stands.** This is a limit on work that
*passed*, which is exactly the kind that gets lost.

- **Discharged — the watch hit's thread (Task 5).** Both construction sites (the hardware
  `Stop::Other` arm and the software `finish_event`) have per-site mutation-tested guards, but on
  `WATCHLOOP` and `FILEIO`, which are single-threaded and can only truthfully answer `thread == 0`;
  a hardcoded `thread: 0` would satisfy both. Task 9's gate is what discharges it: it asserts
  `ends_with("thread=1")` **and** `!contains("thread=0")` against ground truth that independently
  establishes tid 1 is the child (the guest prints two distinct cell addresses; `thread_summaries()`
  is read straight off `retrace_core::seek`), and it provably fails when attribution is wrong.
- **Still standing — the oracle's two signal-path arms (Task 4).** They are exercised only by
  `SIGFRAME`, which is **single-threaded**. Those tests prove the check *fires* and reports a
  `Divergence` at each site; they do **not** prove it *distinguishes two live schedules* there, since
  there is no second live thread id in the fixture to retag to. Only the generic arm gets that, via
  `THREADRUST`. Closing it needs a guest that is both threaded and signalling, which does not exist.
  *(Discharged by M16-threadsignal, and left standing here as M15's own account: `rs/sigthread.rs`
  is that guest, and independent mutation of each arm now fails its own gate while the other stays
  green. See the M16-threadsignal Status section.)*

**A thread scope naming a thread that never exists is silently inert — and arm-time validation is
the wrong fix.** `watch 0x… thread 99` parses, arms, and then suppresses every hit forever:
`watch_thread_matches` compares the scope against the hit's thread, never matches, and `continue`
runs to exit without ever reporting a hit — it still prints `exited (code N)`, so the silence is
specifically the absence of hits, not an absence of output. That is the same class Task 8's fix
round called intolerable — a scope announced but not applied — arriving through a different door.
**It cannot be fixed by validating the id when the watch is armed**, and the reason is load-bearing
rather than incidental: thread 1 legitimately does not exist yet when a user arms a watch *before*
`bsdthread_create` runs, which is the main way this feature gets used, so rejecting unknown ids at
parse time would break the ordinary case to catch the typo. The natural fast-follow is the other end — check at the end of the run
rather than constraining the arm. Note what the *bare* form does not buy: warning on zero matching
hits alone fires identically whether nothing wrote the address or nothing could ever have matched,
which is exactly what those two cases have in common. Distinguishing them needs zero matching hits
**and a nonzero count of scoped-out hits**, and that count is already available at both discard sites
(the forward recursion and the `WatchSyscall` fall-through) — it costs a counter on `Exec`, which is
where it has to live, since the forward path re-enters `cmd_continue` and a local would not survive
the call.

**Two coverage gaps are accepted and named rather than quietly dropped.**

1. **The `WatchSyscall` thread filter has no scoped coverage.** `watch_thread_matches` has five call
   sites; three of them are on the `WatchSyscall`/`RHit::WatchSys` path, and **none of the three is
   exercised with a scope.** Task 10's sweep proved it on the sharpest of the three: it bypassed the
   guard at the boundary-cross call site *only*, leaving the callee and the other four sites intact
   — and **zero tests failed.** Task 8's reviewer independently flagged the same class. The root
   cause is fixture shape, not logic: **no guest anywhere issues a *syscall* write to a watched cell
   from a scoped thread.** `WATCHTHREAD`'s threads write the watched cells with plain stores, so
   every thread-scoped script takes the hardware path, and the one test that does exercise the
   boundary-cross arm
   (`pre_step_boundary_cross_reports_a_watched_syscall_write`) never scopes its watch. Closing it
   needs a new guest whose thread writes a watched buffer through a syscall out-param. **Named
   fast-follow.**
2. Task 7's `cmd_threads`/`cmd_regs_of` take `&mut self` without needing it, and its parser unit test
   is parse-only, with all behavioural coverage resting on the e2e tests. Both cosmetic; neither
   fixed.

**What contradicted the plan, in detail — because a plan that survives contact unamended is more
likely unexamined than perfect.** This one did not survive unamended:

- **Task 8 required a guest no earlier task built.** `threadrust.rs` performs no writes at all, and
  the only guest the plan creates is `WATCHTHREAD` — in Task 9, Task 8's *successor*. The two tasks
  were **executed in reverse order** to fix it, and the plan was amended rather than the discrepancy
  papered over.
- **Task 10's mutation table was wrong twice about the same row, once *after* being amended.**
  Measurement disproved both claims. `current_thread()` → `0` is **not** caught by Task 1's test,
  which asserts on `Box_::threads().current()` against a bare `Box_` and never constructs a
  `ReplaySession` at all; and `Advance::Watch.thread` → `0` is **not** caught by Task 9's gate, for
  the structural reason in layer 3 above. Both corrections came from the sweep whose job was to
  measure the claims rather than accept them.
- **Task 9's brief demanded the watched address be learned from recorded behaviour, then specified a
  guest that stores to a `static mut`** — whose address is never an argument to anything the kernel
  sees, so M13's learn-it-from-a-recorded-`mprotect` trick had nothing to bite on. Resolved by having
  the guest **print** both cell addresses: still its own recorded behaviour, just stdout rather than a
  syscall argument, and printing *both* is what lets the gate assert they are distinct instead of
  assuming it.
- **Task 9's brief implied proving attribution and "regs of a non-current child" in one script.**
  That is impossible: at the coordinate `reverse-continue` parks on, the child *is* current by
  construction, because it is the thread that just executed the un-retired store, so `regs <child>`
  there is indistinguishable from plain `regs`. The gate is two parts against two coordinates.
- **Task 6's brief said "test-only, no product code" while also requiring the hardware leaf be
  asserted** — mutually exclusive, since no accessor for those registers existed. Resolved with one
  `#[doc(hidden)]` accessor, in the same family as `dbg_leak_ss`/`dbg_internal_state`/`dbg_pac_enabled`.
- **Task 7's brief listed only `debug_cli.rs` as its test file.** Adding `thread=` to `where`'s output
  broke `ends_with` assertions in `watch_cli.rs` and `crashy_cli.rs` too. None was weakened to
  `contains`; all were updated to carry the new suffix.
- **Task 3's brief undercounted the `Event::Syscall` construction sites** ("~34" against a measured
  35 plus one non-`..` match pattern) and did not mention the 36th site outside `retrace-core` in the
  trace crate's own `sample()` fixture. The compiler found all of them — a missed site is an `E0063`,
  which is the safe failure — and a `grep` for `thread: 0` in `record_box` is the backstop against
  the dangerous one, a site that compiles while writing a defaulted id.
- **Task 5 shipped a code comment asserting a borrow-checker error that does not occur.** The claim
  that matching `take_syscall_watch_hit()` inline makes the scrutinee temporary outlive the body was
  disproved by building it: the method returns an owned `Option`, so NLL ends the `&mut` borrow at the
  call. Both the pre-binding and its false rationale were removed.
- **Task 7's `regs 99` shipped as exit 5, not the exit 2 its brief named.** Exit 2 / "usage" is
  produced only by the CLI-argument branch that runs *before* `run_script`; every error *inside* a
  script (bad hex, the six-breakpoint limit, the examine cap) already goes to `DEBUG ERROR: …` and
  exit 5. An out-of-range thread id is a script-level error, so it belongs with its siblings.
- **Task 8 found a lying echo it had just made worse.** `cmd_watch` only inserted on a *new* address,
  so re-`watch`ing an armed address left the stored entry untouched while printing the
  just-requested len and scope — the `len` half predates M15, but Task 8 is what turns it into "the
  scoping feature reports a scope it did not apply." Re-arming without an intervening `unwatch` is
  now an explicit usage error, matching how every other watch-arming failure in that file behaves.

**Six times this milestone, a subagent corrected a controller claim by checking it instead of obeying
it** — five implementers questioning their own instructions, and the non-vacuity sweep, whose entire
job was to measure claims rather than accept them. One process failure is worth recording against
that: the `crashy_cli` regression escaped Task 7's review because the controller handed the reviewer
an *enumeration* of four affected sites instead of the *property* that had changed, and the list
became the ceiling of the search. That is a dispatch failure, not a review failure.

**Still unmodelled, and named rather than discovered later:** **thread identity on any landmark that
is not a syscall** — only `Event::Syscall` carries the tag, so `Exit`, `Crash`, `Signal` and
`SignalDelivery` leave a multi-threaded guest's terminal or handler-entry landmark unattributed, the
same corner of the format the two untested signal-path oracle arms live in; **per-thread reverse
execution as its own position space** — `P` stays `(N, K)`, and "rewind thread B" is a search over
positions where B is current, not a coordinate change; **preemption** — scheduling is still
cooperative, so a guest that spin-waits without ever trapping still runs forever; **`workq`/GCD**
thread pools; **thread priority**; **per-thread signal masks**; and **scoping a watchpoint in
hardware** (the `DBGW` slot
stays global; filtering is the debugger's job). Everything M14 and M13 carry forward is unchanged:
`guest_bsdthread_create` still returns `0` where the real syscall returns the child's `pthread_t`;
`run()` and `step()` still carry the reschedule check independently; every protection bit other than
no-access, a pending signal set, nested delivery, a blocked synchronous fault, `dup2` (fail-loud),
`fcntl(F_DUPFD)` (unmodelled and *not* fail-loud), guest stdin still being retrace's, `RLIMIT_NOFILE`,
asynchronous signals, and arm64e guests.
*(Superseded in part by M16-threadsignal, and left standing as M15's own account: three items here
fell there — **thread identity on the non-syscall landmarks** (`Exit`, `Crash`, `Signal` and
`SignalDelivery` each carry a `thread` now, at the cost of a second `TRACE_MAGIC` break), **per-thread
signal masks**, and the **pending signal set**. Per-thread reverse execution, preemption, `workq`/GCD,
thread priority and hardware watchpoint scoping all still stand. See the M16-threadsignal Status
section.)*

**No new gate is parked.** The count still carries exactly **one** `#[ignore]` —
`stackoverflow_rust_e2e::a_rust_stack_overflow_strikes_its_own_guard_page`, at the M8 spec-risk-R3
wall, unchanged since M13. M15 parked nothing new and un-parked nothing.

See `docs/superpowers/specs/2026-08-15-retrace-m15-threaddebug-design.md`.

## Status: M16-threadsignal — 🎉 a signal knows which thread it is for

**`pthread_kill(child, SIGUSR1)` now runs the handler on the child.** A new guest,
`rs/sigthread.rs`, installs a `SIGUSR1` handler, spawns a child, masks `SIGUSR1` *for main only*,
signals the child by its `pthread_t`, joins it, then self-raises while still masked and unmasks —
and every step of that is recorded and replayed bit-for-bit. Before M16 the target port of
`__pthread_kill` was not decoded at all: the signal went to whoever held the vCPU, which was main,
synchronously inside the syscall. The one-line check a reader can run is the guest's own stdout
order: `kill rc 0` now precedes `handler`, where it used to follow it. Underneath, the blocked
mask, the pending set and the alternate stack moved off the process-global `SigTable` onto
`Thread`; `Box_::thread_of_port` resolves a mach port to a thread by reading `[pthread + 0xf8]`
back out of guest memory; `deliver_signal_to` builds the frame into a *named* thread's saved
context rather than off the live vCPU; a masked signal pends and materialises at the unmask
landmark; `sigpending` stops lying; and `Exit`, `Crash`, `Signal` and `SignalDelivery` each carry
the thread they belong to, with the oracle checking all four. **The gate: 387 passed / 0 failed /
2 ignored** across 101 test binaries at `dc04e48`, clippy clean over `--workspace --all-targets`
with `-D warnings` (`CLIPPY_EXIT=0`).

That total was again **measured in chunks, not by one `just gate` run**, every chunk
`--no-fail-fast` with cargo's exit code captured before any pipe, and **six of the seven chunks
returned `CARGO_EXIT=0`; the seventh (`gate-chunk-3.log`) was killed at the harness's 600 s ceiling
and contributes only the 60 results, over 27 binaries, that had already printed `test result: ok`
before the kill** — see hazard 2 below, where the kill and its handling are set out in full.
`rung.rs`, the one target cut off mid-test, recorded no result there and was re-run to completion in
the split, so nothing is counted twice and nothing is dropped. M15 closed at **360 / 0 / 1 over 98
binaries**, so M16 adds **27 passing tests, one new `#[ignore]`, and 3 test binaries** — and the
delta reconciles *exactly* rather than being waved through. It was checked by diffing `#[test]`
counts file-by-file between M15's close (`ed819c2`) and this HEAD: `kport.rs` +2 (new),
`sigthread_e2e.rs` +3 (new), `sigblocked_e2e.rs` +1 (new, and the one `#[ignore]`),
`thread_oracle.rs` +4, `thread.rs` 0→9, `sig.rs` 23→20, `deliver.rs` +7, `threads.rs` +1,
`retrace-box/src/lib.rs` +2, `retrace-trace/src/lib.rs` +2. That is **+28 raw new tests, one of them
ignored = +27 passed + 1 ignored**, and the +3 binaries are exactly the three new files.
**`sig.rs`'s −3 is a relocation, not a deletion** — the three mask/altstack unit tests moved to
`thread.rs` when the state they test moved there, which is why `thread.rs`'s +9 contains them.
`retrace-core`'s test count is byte-identical across the milestone. Every headline gate ran and
passed **by name** in the logs: `hello_dyn_e2e`, `hello_rust_e2e`, **both** `jq` gates —
`/opt/homebrew/bin/jq` was present and no skip `eprintln!` fired anywhere in any log, so that green
is earned — `panic_e2e`, `thread_watch_e2e`, and M16's own `sigthread_e2e` at 3/0.

**`TRACE_MAGIC` moved again, so every recording made before this milestone is unreadable.**
`RT\x00\x07` → `RT\x00\x08`: `Exit`, `Crash`, `Signal` and `SignalDelivery` each gained a `thread`
field the new reader requires, so an old trace is missing data rather than merely old. Rejection is
loud and whole — `open_checked` keeps nothing on a magic mismatch — and both halves are pinned by
tests, one asserting the *new* magic (`magic_bumped_for_the_landmark_thread_tags`) and one
asserting a trace written with the *previous* magic is rejected entire. **This is the second format
break in two milestones** (spec risk R4), and it was accepted on a specific ground rather than
waved through: every trace on disk was *already* dead from M15's break, so the break costs nothing
now and would cost real recordings later, once traces start being kept again. The spec named the
seam to cut if the milestone grew too large — M16-tag, not M16-pending, precisely because dropping
the tags drops the break with them — and it was not cut, so the break was paid deliberately. **If
you have a `.bin` from M15 or earlier, re-record it.**

**R1's measurement, first task of the milestone: main's kport reads back as `0x103`, and the
fallback was never needed.** The design assumed `[main_pthread + 0xf8]` holds a usable mach-port
name even though retrace never writes it — libpthread's `__pthread_main_thread_init` does, in
userspace — and that assumption had never been checked. Measured on `THREADRUST`, three separate
`record-dyn` + replay runs, identical every time: **`main kport = 0x103`, `child kport =
0xbad7001`** (the child's being the `GUEST_THREAD_PORT_BASE | tid` retrace itself wrote). Nonzero,
distinct, and readable through exactly the same `pthread_of`/`kport_of` path as a child's — so
`thread_of_port` needs **no special case for main**, and the fallback the spec held in reserve
(recognise `0x0BAD_7000 | tid` and fail loud on anything else) was never built. One caveat travels
with that number: `0x103` is *stable*, not *architecturally guaranteed*. It is the kernel's write,
not retrace's, so it is guest-observed data — like M2-xpcport's minted port — and nothing here
claims retrace could recompute it.

**What is proven and what is merely exercised are not the same list, and the pended-raise path is
where the difference bites.** Both halves of the pend path — record's `pend` and replay's mirror —
were verified consistent *in source* by Task 8's review, including the fall-through equivalence that
lets replay's blocked branch drop into generic dispatch. Source agreement is not a test. What Task
9's guest actually exercises end to end is the **self-directed** case: main masks `SIGUSR1`,
`pthread_kill`s *itself*, the signal pends, `sigpending` reports it, and the unmask materialises it
into a real `SignalDelivery` — and that is genuinely proven rather than argued, because mutating
replay's recomputed pending set to a constant `0` took `sigthread_e2e` — two tests, as that file
stood at Task 9 — from 2 passed to 2 failed, with a named divergence (`sigpending set mismatch …
recomputed [00,00,00,00] != recorded [00,00,00,20]`, bit 29 = `SIGUSR1`). **What remains
source-level agreement only** is the *cross-thread* pend — `pend(target, …)` where `target !=
current`, which no guest reaches, since `sigthread`'s masked raise is self-directed and its
cross-thread raise is unmasked — and `take_pending_delivery`'s `Ign` / `Dfl`-ignore discard
branches, which no guest reaches either. Those write and read a bit that both sides agree about
because they call the same function with the same arguments, not because anything runs them.

**The `Crash` thread check is installed and unexercised, and the reason is a missing fixture rather
than an oversight.** Task 11 added `verify_thread` at both the `Exit` and `Crash` replay sites. The
`Exit` site has a retag mutation test (`a_wrong_thread_on_the_exit_landmark_is_a_divergence`,
against `THREADRUST`); the `Crash` site has none, and **no honest test for it is constructible from
the guests in this tree**. Enumerated rather than assumed: `threadrust`, `watchthread` and
`sigthread` are the only guests that spawn threads, and none of them reaches the `Crash` arm —
`sigthread`'s signal is non-terminal. This project's own standard for a retag mutation is that it
must target a *genuinely live second thread id in the same trace* (it is exactly why M15's
bogus-constant mutations were judged too weak), and every crashing fixture is single-threaded, so
the only mutation available is precisely the weak kind. A threaded-crashing guest was ruled out of
scope. Two checks added, one mutation-proven, one not.

**Tasks 7, 9 and 10 are not three independent guards on per-thread masks — they are three
observables of one defect class.** Measured, not assumed: mutating `ThreadTable::is_blocked_for` to
ignore its `tid` — a faithful re-creation of the process-global mask M16 replaced — fails **all
three** `sigthread_e2e` tests *and* the `thread.rs` unit test
`masks_are_independent_between_threads`. Worth stating because the close would otherwise read as
three checks where there is one. What each *uniquely* contributes: Task 7 owns "delivery goes to the
thread the port names"; Task 9 owns "a masked signal pends and materialises at the unmask"; and
Task 10 owns the **stdout-ordering proof** (`masked` precedes `kill rc`) — the only one of the three
that makes the per-thread mask an observable of the guest's own behaviour rather than an assertion
about a struct. One deferred minor sits alongside: the `first` binding and its `assert_eq!` in
`sigthread_e2e::main_masking_a_signal_does_not_block_it_for_the_child` — Task 10's trace-side check —
are mechanically the same assertion as Task 7's `delivered[0] == 1u32`. It was specified verbatim by
the task brief and self-disclosed; noted for whoever tightens test independence. (Anchored to the
symbol rather than to the line numbers this paragraph used to cite, which had already drifted.)

**M15's standing fidelity caveat is discharged, and the evidence is specific rather than
rhetorical.** M15 shipped the oracle's caught-raise and `sigreturn` mirrors proven to *fire* but not
to *distinguish two live schedules*, because the only fixture reaching them (`SIGFRAME`) is
single-threaded. `sigthread` is the guest that is both threaded and signalling, and Task 12 proved
the distinction by independent mutation: disabling the caught-raise mirror's `verify_thread` fails
`a_wrong_thread_at_the_caught_raise_mirror_is_a_divergence` while the `sigreturn` test stays green,
and disabling the `sigreturn` mirror's does exactly the reverse. Both were restored and the call
census returned to seven after each. The review strengthened the transcript into a proof by
establishing that `self.verify_thread(*rthread, pc)?` is **the only statement in either arm capable
of producing a divergence once `(num, args)` has already matched**, so the observed exit-code flip
is mechanically attributable to the thread check and to nothing incidental; and the two arms are
structurally non-overlapping branches, so neither mutation can contaminate the other's test.
`CLAUDE.md`'s statement of the caveat has been **rewritten**, not left standing beside the work that
discharged it. M15's Status section keeps its own wording, because a Status section is a historical
log rather than a live claim; it gains a forward pointer instead — the same treatment M11's stale
`SigTable` sentence gets in this milestone.

**`verify_thread` has seven call sites, not three, four or six — and the drift is the lesson.** The
seven, with attribution: M15 Task 4's three (the generic dispatch, the caught-raise mirror, the
`SYS_SIGRETURN` mirror); M16 Task 8's terminal `Signal`; M16 Task 9's hoisted mask mirror; M16 Task
11's `Exit` and `Crash`. Verified by grep at this close rather than inherited from the census — and
the grep turns up a detail the census does not state: the **`SignalDelivery` landmark's thread is
checked by an eighth comparison that is not a `verify_thread` call at all**, but an inline `rthread
!= tid` test inside `mirror_delivery`, because that tag names the *receiving* thread rather than the
current one. So "seven `verify_thread` sites" and "eight places the oracle compares a thread" are
both true, and only the first is what a grep for `verify_thread` returns. The census in its own doc
drifted **three times inside this one milestone** and was corrected in Task 12. The pattern
underneath is what matters: every one of those sites exists because a mirror was found that
`return`s *before* reaching the generic dispatch, so **each new mirror silently creates a new hole
until someone remembers to add its oracle call**. Nothing structural couples "add a mirror" to "add
its `verify_thread`"; today the coupling is a habit and a grep.

**The `sigaltstack` oldstack writeback is the one remaining serviced-syscall writeback with no
divergence check — pre-existing, and deliberately not fixed here.** Replay's hook — the
`num == retrace_arch::SYS_SIGALTSTACK && args[0] != 0` block inside `ReplaySession::advance`'s
generic dispatch arm, in `crates/retrace-core/src/lib.rs`; anchored to the symbol rather than to a
line number, per this branch's own `3ce67aa` — reads the *new* stack out of guest memory and calls
`set_altstack_of` so the thread table stays in step, then applies the recorded bytes. It never
recomputes or byte-compares the *old* `stack_t` that record writes back at `args[1]` — unlike the
`sigaction` oldact compare six lines above it, the `sigprocmask` oldset compare in the hoisted mask
arm, and (as of M16) `sigpending`. Symmetry rule 1's check is simply absent there. This predates
M16, M16 did not create it, and M16 chose not to close it; it is named here so it is not later
discovered. **A scope note for whoever does fix it, measured and easy to get backwards:** record's
arm handles the query case `args[0] == 0` by reading `altstack_of` without changing state, so the
mirror's guard belongs on **`args[1] != 0`** — the writeback pointer — **not** on `args[0] != 0`,
which is how the current hook is guarded.

**[Closed after M16 by the `sigaltstack` fast-follow, `f000c0d`.]** Left standing above rather than
corrected, because this log is append-only. Replay's hook is now a real mirror of record's arm: both
sides go through one shared `retrace_box::decode_stack` / `encode_oldstack` pair, so the 24-byte
layout exists in exactly one place, and the compare is guarded on `args[1] != 0` — the writeback
pointer — exactly as the scope note above predicted.

One expectation recorded above needs correcting, and it is the kind that is easy to get backwards a
second time. The missing check did **not** mean a corrupted oldstack was silently accepted: the
pre-existing end-of-run full-memory `Snapshot` diff caught it anyway, seven landmarks late, as a bare
`memory divergence at ipa 0x…` naming no syscall at all. Measured on the same mutation, before and
after the fix:

```
before: DIVERGENCE at landmark 9: memory divergence at ipa 0x100004044: replay=0xff recorded=0x00
after:  DIVERGENCE at landmark 2: sigaltstack oldstack mismatch at 0x100004030: … != recorded [.. ff ..]
```

So the CLI's exit code was already 3 either way, and a gate asserting on the exit code alone would
have passed **before** the fix. `a_corrupted_sigaltstack_oldstack_region_is_a_divergence`
(`sigdeliver_e2e.rs`) therefore asserts on the divergence *message*, and the landmark shift 9 -> 2 —
not the green — is what proves the check fires at the sigaltstack landmark rather than by accident at
the end of the run. Whoever re-checks this later should look at the landmark number.

The fixture had to change too: `altstack.s` only ever called `sigaltstack(&ss, NULL)`, so `args[1]`
was 0, record wrote no oldstack `Region`, and there was nothing in any trace to corrupt. It now also
queries with `sigaltstack(NULL, &oss)` and checks the three returned fields (exit codes 32/33/34).

**A rough edge worth naming: some replay-side divergences abort instead of diverging.**
`deliver_signal_to`'s `Runnable` assertion and `thread_of_port`'s no-such-port panic are correctly
fail-loud, and calling them from the replay side is exactly what symmetry rule 1 demands — the two
loops must call the same `Box_` method with the same arguments. But on the replay side those are
the failure modes of a *schedule* divergence, and the `pthread_kill` landmark's own `verify_thread`
checks the **caller**, not the target, so it would not catch such a divergence first. M16 elsewhere
prefers a named `Divergence` at a landmark to a process abort. Recorded as a known rough edge, not
fixed.

**[Closed after M16 by the replay-divergence fast-follow, `fcc308b`.]** Left standing above because
this log is append-only. `Box_` now exposes `try_thread_of_port` and `check_deliverable`, returning
the existing diagnostics as `Err`; the panicking forms are thin wrappers over them, so record's
behaviour and both messages are unchanged — the two `should_panic` tests in
`retrace-box/tests/deliver.rs` are the regression guard proving that, and a mutation making
`check_deliverable` always succeed kills exactly those two plus the two new `Err` tests. Replay maps
`Err` to a named `Divergence` at the landmark.

**The honest limit, measured rather than assumed.** Neither converted arm is reachable by trace
mutation, so neither has an end-to-end gate: the conversion is proven at the seam (`Err` is returned
and carries its diagnostic) and by construction above that, not through a live guest. The reason is
structural and worth knowing before anyone tries again — **every mirror recomputes from live guest
state, and the trace supplies recorded values only to compare against**, so no recorded field's
corruption can make replay's thread table disagree with the port the live guest passes. Three
candidate levers were tried and all three fail:

- Main's kport *is* covered by the initial `Snapshot`'s Region (ipa `0x28000`, len 65536). But
  `__pthread_main_thread_init` rewrites that field with an ordinary guest store that replay
  re-executes, so the corruption does not survive — corrupted, replayed, `dbg_kport_of(0)` read back
  the original value unchanged.
- The child's kport is covered by **no** Region at all: `record_box`'s `SYS_BSDTHREAD_CREATE` arm
  appends `writes: vec![]` deliberately, since both sides recompute the identical byte.
- Corrupting `bsdthread_create`'s recorded `args[3]` (the pthread pointer) fails for the same
  reason as the first: replay's mirror calls `guest_bsdthread_create(args)` with **live** args, so
  the corruption surfaces as an ordinary argument divergence and proves nothing about this code.

Evidence in `.superpowers/sdd/kport-probe-findings.md`. Practically, these arms fire only when
retrace itself has a real schedule bug — precisely the case where a named landmark beats a process
abort. The delivery arm additionally cannot even be *recorded* today: `sigblocked_e2e`, the guest
that would produce it, is parked at record's own fail-loud guard. **Un-parking that gate is what
would make the delivery arm live**, and is the natural next step for whoever wants it covered.
*(Superseded in part by M17-blockedsignal, and left standing as M16's own account: the next step was
taken — `sigblocked_e2e` is un-parked and green, so `mirror_delivery` is genuinely called on the
wake-materialised delivery path. But the **`check_deliverable` Err branch inside it is still
unreached**, for a new reason: `guest_ulock_wake`'s `unblock_waiters_on` makes the woken thread
`Runnable` in the same call that produces the woken set, so every target reaching that point is
already Runnable on both sides. It now needs a genuine live-versus-recorded schedule mismatch, not an
un-parked gate. See the M17-blockedsignal Status section.)*

**The spec's open question 4 is answered, and the answer is not the one the spec predicted.** It
asked whether the debugger should surface the *receiving* thread at a `SignalDelivery` landmark, and
reasoned that "`where` already reports the box's live `current_thread()`, which at a delivery
landmark is the receiver, so the answer may be 'nothing to do' — but that needs checking rather than
assuming". **Checked, in the code, and the premise is false in exactly the case M16 created.** A
cross-thread delivery does not switch: `deliver_signal_to` saves the caller's context, builds the
frame into the *target's* saved ctx, and then `load_ctx`es the **caller** back onto the vCPU, leaving
`threads.current()` untouched. So at such a landmark `cmd_where` prints
`at (N, K) pc=… thread=<caller>` — main, in `sigthread`'s headline case — while the thread that will
run the handler is the child. `threads` marks the same caller with its `*`, and **no debug line
renders a `SignalDelivery` at all**: `Outcome` carries only `Exit`/`Crash`/`Signal`, so the spec's
conditional second half ("if a debug line prints `SignalDelivery` today it should carry the tag") has
no subject. Nothing printed is *wrong* — the guest really is executing on the caller, and
`current_thread()`'s doc already warns that a boundary names the thread that issued the landmark
rather than the one that will retire the next instruction. **Surfacing the receiver was considered
and declined for M16, and is named here as a follow-up rather than a non-issue:** the receiver is in
the trace (`Event::SignalDelivery.thread`) and the oracle checks it, but `ReplaySession` exposes no
accessor for the recorded landmark's tag, so a `where` that named it would be a new API surface plus
a rendering decision (two thread numbers on one line) — real design, not a one-line print, and not
what the Components table's "`where`/`threads` reporting unchanged but re-verified" reserved room
for. The re-verification is this paragraph; the spec's now-false clause has been struck at its
source.

**The named limits, stated here rather than discovered later:**

- **A signal pended on a thread that never touches its mask again is never delivered.** Delivery has
  exactly two anchors, both syscalls: the `pthread_kill` landmark and the
  `sigprocmask`/`pthread_sigmask` landmark that unblocks. `take_pending_delivery` operates on the
  **calling** thread, so a signal pended on thread B materialises only when *B itself* unmasks. A
  real kernel would deliver it at B's next opportunity. Anchoring to syscalls is deliberate — the
  reschedule check lives inside `run()`, below the trace, and producing a `SignalDelivery` from
  there needs either a new channel out of `run()` or the scheduling decision duplicated into both
  dispatch loops, which is the exact argument M15 used to *delete* `Event::Sched`.
- **Handler-before-body differs from a native run** (spec risk R3, accepted by design). A never-run
  target's saved context is the synthetic entry context `guest_bsdthread_create` built, so the frame
  lands on the child's stack, the child runs the handler first, `sigreturn`s, and *then* starts its
  body. A real kernel starts the thread and takes the signal at its first opportunity. This is why
  the gate asserts against **retrace's own recorded behaviour replayed identically**, not against
  native output the way `hello_dyn_e2e` compares against `"hi\n"`.
- **Signalling a thread that is Blocked is parked, with a gate to prove it** — see below.
  *(Superseded by M17-blockedsignal, and left standing as M16's own account: it is no longer parked.
  A signal to a `Blocked` target now **pends** and is materialised at the `__ulock_wake` that makes
  the thread runnable, and `sigblocked_e2e` is green with its assertions unmodified. What replaced
  the wall is a narrower, named gap: a signal to a thread nothing ever wakes is never delivered — no
  `EINTR` — and `assert_no_stranded_signals` fails loud at a clean exit rather than swallowing it.
  See the M17-blockedsignal Status section.)*
- **Signal queueing and nested delivery are unmodelled**, and a second signal raised for a thread
  already redirected and not yet scheduled is fail-loud rather than stacked.
- **`sigwait` (330) and `sigsuspend` (111) still panic**, unchanged from M11.
- **A pended signal whose default action is Terminate panics at the unmask** rather than killing the
  process. Both sides reach that panic at the same landmark, so it cannot desync; no guest reaches
  it (`abort()` unblocks `SIGABRT` before raising it).

**One new gate is parked, and `stackoverflow_rust_e2e` is untouched.** The ignored count goes 1 → 2.
*(Superseded by M17-blockedsignal, and left standing as M16's own account of what M16 measured: the
count goes back **2 → 1** there. `sigblocked_e2e` was un-parked — by deleting the `#[ignore]`
attribute, with the test body byte-for-byte unchanged — leaving `stackoverflow_rust_e2e` at the M8
R3 wall as the only live `#[ignore]`, still untouched. This paragraph is the discipline working
exactly as it describes: a gate parked at a measured wall, un-parked one milestone later without one
assertion being relaxed. See the M17-blockedsignal Status section.)*
`sigblocked_e2e::a_signal_reaches_a_thread_blocked_in_ulock_wait` is the new one: a three-thread
guest (three, not two, and forced rather than incidental — the cooperative scheduler switches only
on block or exit, so for a peer to be blocked main must have blocked first; a blocked *joiner*
leaves its joinee running, so `main → joins a → joins b`, and `b` is the only thread that can
express this signal at all). `stackoverflow_rust_e2e`'s `#[ignore]` reason is **byte-identical** to
before — M16 did not touch the M8 R3 wall — and those two are confirmed to be the **only** live
`#[ignore]` attributes anywhere in the test surface; the other four files a `grep` matches carry the
string inside prose comments narrating gate history, not as attributes.

**What contradicted this plan — because a plan that survives contact unamended is more likely
unexamined than perfect.** This one did not survive unamended:

- **Task 13's wall was cleaner than the plan predicted, and the reason is that M16 had already
  guarded it.** The brief predicted a messy failure — mismatched register state, EINTR/restart
  semantics — and explicitly warned "do not assume it panics where you expect." Forced with
  `--ignored`, what actually happens is a **clean fail-loud panic from M16's own Task 6 fix round**:
  `crates/retrace-box/src/lib.rs:2849`, "thread 1 is `Blocked(Wait { addr: 809578548 })`, not
  Runnable; `deliver_signal_to` would overwrite the saved context its blocking syscall must resume
  through." That guard was added when no product caller could reach it, on the argument that the
  failure mode would be silent corruption rather than a panic; Task 13's guest is the first caller
  to reach it, and it fires exactly as designed. The general *shape* of the prediction held (a
  blocked thread's saved ctx is a resume point that cannot simply be redirected); the *mechanism*
  was an already-installed assert, not a live discovery. The `#[ignore]` reason was written from the
  measurement rather than forced into the predicted shape.
- **The spec's `M16-target` rule is contradicted by the implementation, and the implementation is
  right.** The spec says `complete_syscall_before_delivery` is applied "**only when
  `target == current`**", reasoning that a non-current target is not returning from a syscall and
  applying the completion would corrupt its `x0`. Record's caught-raise arm calls it
  **unconditionally**, before `deliver_signal_to`. That is correct *because of* the refactor the
  same spec section describes: after it, the function operates on the live vCPU — the **caller** —
  whose context `deliver_signal_to` then saves into the table, while the target's frame is built
  from the target's own saved ctx and never touches the caller's. The conditional the spec
  imagined would break the caller instead: `pthread_kill` must still return 0 with `PSTATE.C` clear
  whether or not it signalled itself. The call site carries the reasoning; the spec sentence has
  been corrected at its source, the way its Fail-loud section was earlier on this branch.
- **The `verify_thread` census drifted three times in one milestone.** Documented above; it is
  listed here too because it is at least as much a *planning* failure as a code one — the count
  lived in a doc comment that each successive task had to notice was stale, and Task 12 is where it
  was finally corrected rather than re-copied.
- **Task 11's brief contradicted itself about how many call sites to add**, naming three where two
  were correct: the terminal `Signal` site's call had already landed in Task 8 (`449cf90`). The
  implementer resolved it by grepping rather than obeying, found the existing call with its
  "RAISING thread, not `target`" comment intact, and added only `Exit` and `Crash`.
- **Task 10's mutation measured three tests catching one defect class** where the plan implied
  independent guards. Documented above.
- **Task 9's non-vacuity prediction was wrong about which tests a mutation would fail.** The brief
  predicted that making `take_deliverable` always return `None` would fail Task 9's test and leave
  Task 7's green. It fails both — Task 7's on `lines.len(): left 9, right 10`, because the guest
  prints from inside its handler and a dropped delivery is a missing stdout line. The line-count
  assertion was **not** weakened to make the prediction come true.
- **Task 9's fix round deliberately deviated from the brief's mutation recipe, and was right to.**
  The brief proposed mutating record's raise arm alone, which answers a different question: replay
  recomputes the target independently, so a record-only mutation kills the test through a *replay
  divergence* and proves the oracle works rather than proving the test's delivery claim bites.
  Mutating **both** resolutions is the faithful simulation of the defect M16 closes, and under it
  the test fails on its own assertion, at `sigthread_e2e.rs:43`, with `left: 0, right: 1`.
- **Task 8's review found that replay's `pend` was a write-only side effect** at the time it was
  written — both sides maintained a pending bit nobody read. Task 9's materialisation is what gave
  it a consumer, and this close verified that by grep rather than by assumption:
  `ThreadTable::take_deliverable` has a **product** caller at `crates/retrace-core/src/lib.rs:93`,
  not merely test callers. That check mattered: Task 7 traded a fail-loud assert for a pending set,
  and an assert replaced by silence is the one direction this codebase's fail-loud constraint
  dislikes.
- **The spec's own open question 5 is answered, and the answer is yes.** It asked whether `Exit`'s
  thread tag was worth its format break "if it proves to carry no assertion anywhere." It does
  carry one — `a_wrong_thread_on_the_exit_landmark_is_a_divergence`, a retag mutation against a
  genuinely threaded trace. Its type doc was also corrected in passing: the old reason for the tag
  being unambiguous ("a threaded guest still has exactly one thread call exit") is false as stated,
  since nothing stops two threads racing to call `exit`; what actually makes it unambiguous is that
  `record_box`'s `SYS_EXIT` arm `break`s the record loop immediately after appending, so at most one
  `Event::Exit` can exist in a trace.
- **One commit subject overclaims, and is recorded rather than rewritten.** `af657a6` reads "M16
  t11: the oracle checks Exit, Crash and Signal's thread too", but that commit added only `Exit` and
  `Crash`; `Signal`'s landed in Task 8 (`449cf90`). True of the oracle's coverage, imprecise as a
  description of the commit. Judged not worth unwinding history over, and named here so this account
  does not repeat it.

**Two hazards on the project's own exit gate, both deliberately deferred, both named so the next
milestone does not rediscover them:**

1. **A codesign race between concurrent test binaries.** `crates/retrace/tests/util/mod.rs::bin()`
   runs `codesign -f` on the *one shared* `target/aarch64-apple-darwin/debug/retrace`; `-f` replaces
   the file, so a second test **process** can observe it missing mid-replacement and fail with
   `codesign -f --entitlements failed … No such file or directory`. **`--test-threads=1` does not
   prevent this** — it serialises threads inside one binary, while cargo runs test *binaries*
   concurrently as separate processes. Measured during M16 by running 13 `--test` targets in one
   invocation: `kport` failed, then passed 2/2 alone. It did **not** fire during this milestone's
   closing gate run (every chunk log was grepped for the failure string; zero hits), so the 387/0/2
   above is not a re-run of a spurious red. The fix is to sign a per-test-binary copy rather than
   the shared file. Deferred because `bin()` is shared test infrastructure and touching it at the
   close would be unreviewed. **M16 adds two more test binaries that reach `bin()` on every gate run** —
   `kport` and `sigthread_e2e`, both via `util::record_dynamic`, with `sigblocked_e2e` a third the
   day it is un-parked — **so it raises the collision odds it is deferring.** A gate that can go spuriously red teaches the reader to dismiss
   red, which is the real cost.
2. **The plan's own gate recipe exceeds the 10-minute tool-call ceiling.** New this milestone. The
   plan mandated a single `-p retrace-core -p retrace` chunk; it was killed mid-`rung.rs` at the
   harness's 600s Bash ceiling — *not* a test failure, and no orphan process was left holding the VM
   (`ps` was clean afterwards), so the one-VM-per-process invariant survived it. Every test that
   completed before the kill had passed. Splitting into `-p retrace-core` alone plus three
   `--test`-scoped sub-chunks finished it, and `rung.rs` was re-run to completion so no target was
   counted twice or dropped. This is distinct from the older "a bare `cargo test --workspace` gets
   killed on this machine" hazard, but confirms its shape: **this workspace's e2e suite no longer
   fits in a single bounded invocation**, and the next close will hit the same ceiling with the same
   recipe unless the recipe changes.

**Deferred minors still open at the close**, none of them fixed: `dbg_kport_of(tid: usize)` takes a
different tid width than its sibling `dbg_regs_of(tid: u32)`; the anti-drift claim in the
trace-format type doc is oversold (Task 2, no action taken); Task 10's duplicate trace-side check
(above); Tasks 12a/12b assert only `rep.code == 3` with no stderr message pin, unlike their siblings
— Task 12's mutation proof substitutes for the pin, and both were verbatim brief content rather than
implementer choices; and 12b's doc claims the `sigreturn` tag is a "nonzero id" without asserting
it, where 12c pins its equivalent precondition with `assert_eq!(orig, 1, …)`. **Three more were
named by the final whole-branch review and are carried here rather than fixed**, each a test or doc
tightening with no behaviour behind it: `deliver.rs`'s
`a_second_signal_to_an_unrun_redirected_thread_fails_loud` asserts only that
`catch_unwind(…).is_err()`, which the `Blocked`, `Exited` and `sig_bit` panics would also satisfy —
today only the intended assert can fire (the target is `Runnable` and unblocked), but its siblings
set the standard with `should_panic(expected = "is Blocked(")`, so it should downcast the payload
and assert it contains "already redirected"; `Box_::on_altstack()` has **no product caller left**
after M16 routed everything through `on_altstack_of`, so its doc line "Unchanged for every existing
caller" is now vacuously true and should either point at the `deliver.rs` test that keeps it or be
folded into that test; and the fault path carries a **textual** asymmetry — record's `Stop::Fault`
arm calls `b.deliver_signal(sig, …)` while replay's mirror calls `deliver_signal_to(cur, …)`. Those
are the same call (`deliver_signal` is a one-line delegation to `deliver_signal_to(current)`), so
symmetry rule 1 holds behaviourally; but this branch works hard to make symmetry *visible*, and
passing `thread as usize` on the record side would make the pair grep-identical.

**Everything M15 and earlier carry forward is unchanged.** Per-thread reverse execution as its own
position space, preemption (scheduling is still cooperative, so a guest that spin-waits without
trapping runs forever), `workq`/GCD thread pools, thread priority, hardware-scoped watchpoints, the
`WatchSyscall` thread filter's missing scoped coverage, M15's three named fast-follows,
`guest_bsdthread_create` still returning `0` where the real syscall returns the child's `pthread_t`,
`dup2` (fail-loud), `fcntl(F_DUPFD)` (unmodelled and *not* fail-loud), guest stdin still being
retrace's, `RLIMIT_NOFILE`, asynchronous signals from outside the process, per-thread *dispositions*
(correctly process-global — that is POSIX, not a gap), and arm64e guests.

**Process reality, recorded because it shaped the milestone.** Seventeen subagent deaths across M16,
six of them in the final session, and almost all infrastructural (`API Error: Connection lost
mid-response`, `ENOTFOUND`) rather than capability failures. Three implementers died mid-task
leaving uncommitted work; **every one was resumed from the tree rather than restarted, and nothing
was lost.** Two mitigations were adopted mid-milestone and both are worth keeping: reviewers write
their report to a file *before* returning it, which recovers a report lost in transit (though not
one the agent never got far enough to write); and expensive verification steps run *after* the
commit rather than before it, so a death costs the proof and not the task.

See `docs/superpowers/specs/2026-08-19-retrace-m16-threadsignal-design.md`.

## Status: M17-blockedsignal — 🎉 a signal reaches a thread that is blocked

**`pthread_kill(a, SIGUSR1)` now works when `a` is asleep in `__ulock_wait`.** M16 gave signals a
thread identity and then stopped at one boundary, parking `sigblocked_e2e` at a fail-loud guard: a
target that is `Blocked`, not merely not-current. `deliver_signal_to` builds the handler frame into
the target's **saved context**, and for a blocked thread that context is the resume point its own
blocking syscall owes a return value through — redirecting it would overwrite that resume point out
from under the wait. M17 funds the boundary M16 declined to fund. The gate is un-parked and green,
and the ignored count goes **2 → 1**.

**The mechanism is pend-until-wake.** A raise aimed at a thread that cannot run yet is *pended* on
that thread and *materialised* at the `__ulock_wake` that makes it runnable. That is a syscall
landmark, so both dispatch loops can see it — the identical argument that already kept delivery above
the trace for M16's unmasking `sigprocmask`/`pthread_sigmask`, reused rather than re-invented. There
is now a second materialisation site where M16 had one, and the two reasons a signal pends (masked;
blocked) are independent: `take_deliverable` already filters by mask, so a signal pended for both is
released only when both have cleared. **No trace-format change** — `SignalDelivery` already existed
and already carried a thread tag, so `TRACE_MAGIC` stays `RT\x00\x08` and an M16 recording is still
readable — where M15 and M16 each broke it.

The pieces: `should_pend_for` (`crates/retrace-box/src/lib.rs`) is the pend-vs-deliver predicate both
dispatch loops consult, written once so they cannot drift on that decision while both stayed green;
`guest_ulock_wake` now returns *which* threads it woke (`-> (u64, Vec<usize>)`) rather than only how
many, because the wake site cannot materialise onto a thread it cannot name; record's
`SYS_ULOCK_WAKE` arm materialises and appends a second landmark; replay's hook consumes both.

**The gate: 412 passed / 0 failed / 1 ignored** across 103 test binaries at `3501c9a`, clippy clean
over `--workspace --all-targets` with `-D warnings` (`CLIPPY_EXIT=0`). Measured in chunks again, and
this time **every chunk returned `CARGO_EXIT=0` — no kill, nothing partial** — 55 logs in total, each
one grepped for the codesign-race string and for `FAILED`/`panicked` with zero hits. The `jq` gates
genuinely ran rather than skipping (`/opt/homebrew/bin/jq` present), so none of the 412 is a silent
skip. The one `#[ignore]` is `stackoverflow_rust_e2e`, confirmed by grep to be the only live
`#[ignore]` attribute in the tree.

**The chunk recipe as the README documented it was short by 8 tests and one binary, and this close
found it by arithmetic rather than by luck.** Running the three documented chunks —
workspace-minus-two, `-p retrace-box`, and one `cargo test -p retrace --test <name>` per target —
totals **404 / 0 / 1 over 102 binaries**, not 412 / 0 / 1 over 103. The gap is
`crates/retrace/src/debug.rs`, which holds 8
`#[test]`s inside the `retrace` **bin** target: `--test <name>` selects integration-test targets only
and never builds the binary's own unittest harness. `just gate`'s unchunked `--workspace` run does
include it, and so does any **whole-package** chunk like `cargo test -p retrace`, because dropping
the `--test` filter builds every target in the package. The shortfall belongs specifically to the
per-target substitute the README recommends, and this close is the first to have leaned on it
end-to-end — see the reconciliation below, which measures that no earlier published number was
affected. `cargo test -p retrace --bins` supplies exactly
the missing 8, and the README's recipe now names it. Note the asymmetry that hid this: `--lib` is
invalid for this crate (there is no lib target) and fails the whole invocation loudly, which is
documented; `--bins` is valid, and omitting it fails **silently**, which was not.

The reconciliation against the previous close was done from **commits, never the working tree**:
`#[test]` counts at `e78019c` versus `3501c9a` give `thread.rs` +1, `deliver.rs` +11 (23 → 34),
`threads.rs` +1, `thread_oracle.rs` +1, and `blockedctx.rs` +2 as a new binary — **+16**, with a
tree-wide count of 397 → 413 agreeing independently. The baseline is main's tip at `b73bdbb`,
**395 passed / 0 failed / 2 ignored over 102 binaries** — one README line off this branch's
merge-base. So 395 + 16 = 411, plus `sigblocked_e2e` moving from ignored to passing = **412 passed,
1 ignored, 103 binaries**, which is what the run measured.

**That baseline is not the figure M16 published, and the difference is real work rather than a
counting error.** M16's own close (above, at `:2638`) published **387 / 0 / 2 over 101 binaries at
`dc04e48`**. Between `dc04e48` and `b73bdbb`, M16's fast-follow sweep added exactly 8 tests and one
binary: `sig.rs` +2, `deliver.rs` +3, `kport.rs` +1, `sigdeliver_e2e.rs` +1, and `harness.rs` new
with +1 — that new file being the 102nd binary. Both figures are internally complete, and the check
is the same one used above: 387 + 2 ignored = 389, the tree-wide `#[test]` count at `dc04e48`;
395 + 2 = 397, the count at `b73bdbb`.

**One coincidence is worth disarming here, because it is tempting and it is wrong.** 387 + 8 = 395
and 101 + 1 = 102 match the `--bins` shortfall above *exactly*, which invites reading M16's number as
having been 8 short for that reason — M17 catching a trap that had already fired once, unnoticed.
It reads well and it is false. Measured: M16's 387 + 2 equals the full tree-wide count at its own
commit, so it accounted for every `#[test]` in the tree, `debug.rs`'s 8 included; M15's 360 + 1 = 361
checks out identically at `259a4db`. The reason earlier closes were unaffected is that they chunked
with **whole-package** invocations — M16's plan mandated a single `-p retrace-core -p retrace` chunk
— and `cargo test -p retrace` *without* a `--test` filter does build the bin's unittest target. The
shortfall belongs specifically to the per-target `--test <name>` recipe the README wrote down, which
this close was the first to lean on end-to-end. The trap is newly created, not newly discovered, and
no published number before this one is owed a correction.

**The load-bearing claim was MEASURED before anything was built on it, and that is why the milestone
did not ship a bug.** The design rested on one reading of record's `SYS_ULOCK_WAIT` arm: that a
`Wait`-blocked thread's saved context is a *complete post-syscall state*, `x0` already holding
`__ulock_wait`'s return value. The spec named this R1, refused to build on the reading, and made
Task 1 a measurement task with its own gate, `crates/retrace/tests/blockedctx.rs`. It measured:

```
R1 MEASURED: thread 0 is Blocked(Wait) with a completed context, x0=0x0
```

`0x0` — the success return — and not either of the two pre-`svc` operation words (`0x1000002` /
`0x1020002`) that would have meant the ordering was the other way round. R1 held as read. The gate
was seen red (assertion flipped to `x0 == 1`) before being left green, so the measurement is a
measurement and not a tautology.

**Then the same saved context turned out to be wrong on a different axis — and that is this
milestone's sharpest lesson.** Task 4b measured the *other* half of the same context, the saved
`SPSR_EL1`, rather than inferring it from Task 1's result. It is `0x60000000`: mode `M[3:0] = 0`
(EL0t, as expected), Z set, and **bit 29, C, SET**. C set means "the syscall failed" — sitting
directly beside an `x0` of `0` that means "the syscall succeeded". The two halves of one context
disagreed.

The explanation is that nothing on the wake path had ever patched that SPSR. `set_x0_err_and_return`
writes `reg::CPSR`, the register the vCPU resumes from; the saved `ctx.spsr` is raw
exception-entry state, the guest's own incidental pre-`svc` NZCV. On every other path the gap is
invisible, because nothing reads SPSR before the next trap overwrites it. On a delivery path it is
the difference between a frame that says the wait succeeded and one that says it failed — and
`sigreturn` would have restored the lie. What the real kernel does was measured too, by the
`spikes/sigraisex0.c` probe M16 already had: a successful self-raise enters its handler with
`0x40000000`, C **clear**, because the kernel snapshots PSTATE *after* completing the return. Task 4c
closed it with `complete_saved_syscall_before_delivery`, the saved-context sibling of the live-vCPU
`complete_syscall_before_delivery` — same correction, applied to `ctx.spsr` instead of `SPSR_EL1`,
and deliberately with no `x0` write, because Task 1 had already measured that axis correct.

**The lesson, stated plainly: measuring one axis of a state and inferring the rest is the trap.** R1
was true. The natural next sentence — "so the saved context is fine, build on it" — was false. Two
tasks that both looked like the same question ("is the blocked thread's saved context usable?")
returned opposite answers on `x0` and on PSTATE, and only the second measurement found it. Worth
noting how close the spec came: it wrote that if R1 were FALSE, "the materialisation site would first
need the equivalent of `complete_syscall_before_delivery` applied to a *saved* context rather than to
the live vCPU, and that becomes a task of its own." R1 was TRUE and that task was needed **anyway**,
on an axis the risk register never separated out. The contingency was right about the shape of the
work and wrong about the trigger that would reveal it. Tasks 4b and 4c did not exist when the plan
was written; they exist because someone measured a thing the plan did not ask about.

**The landmark-arithmetic correction was found during plan-writing, not during implementation, and
the wrong reading is worth recording.** A materialising wake appends **TWO** landmarks — the ordinary
`Syscall`, then the `SignalDelivery` — where the ordinary path appends one, so replay must consume
two explicitly. The spec's *first* version said this required hoisting replay's `SYS_ULOCK_WAKE` hook
into its own dispatch arm and that the oracle count therefore went **7 → 8**. Both halves were wrong,
and the error was the same in both: it assumed the wake hook sat where the unmasking-`sigprocmask`
hook sat *before* M16 Task 9 hoisted it. It does not. Replay's wake hook lives **inside** the generic
`Some(Event::Syscall { .. })` arm, whose `verify_thread` call runs *before* control reaches the hook;
and it already `return`s explicitly rather than falling through, which was precisely the mask hook's
problem and the reason that one needed hoisting. So the hook stayed where it was, grew the
two-landmark tail the hoisted mask arm already uses, and no oracle site was added. Commit `8e4666f`
carries the correction. Had it been found during implementation instead, the symptom would have been
R2's signature: "expected recorded syscall, got `SignalDelivery`" reported far past the wake, which is
what M16 Task 9 actually measured (landmark 280 for an unmask at 271) before its hoist.

**The oracle census is UNCHANGED at seven and eight.** Seven `verify_thread` call sites, and eight
places the oracle compares a thread — the eighth still being `mirror_delivery`'s inline
`rthread != tid` test, which checks a delivery's **receiving** thread rather than the current one.
M17 added no site and removed none, and CLAUDE.md's census sentence is deliberately untouched. What
M17 did add is *traffic* on the eighth place by a route no existing test used: Task 8's
`a_wrong_thread_on_a_wake_materialised_delivery_is_a_divergence` (`thread_oracle.rs`) retags the
wake-materialised `SignalDelivery` from thread 1 (`a`, the blocked target) to thread 2 (`b`, the
waker — the specific wrong answer this route invites) and pins replay's `"signal delivery thread
mismatch"`. Proved by mutation: commenting out `mirror_delivery`'s check turns that test **and**
M16 Task 12c's delivery-landmark test red together, which is the correct coupling, since both depend
on the one comparison. Materialisation goes through `mirror_delivery` rather than a hand-rolled
compare precisely so that it lands on that check.

**The accepted semantic gap, and its guard.** Pend-until-wake diverges from POSIX in one direction
and the divergence is named rather than hidden: **a signal pended on a thread nothing ever wakes is
never delivered.** A real kernel would interrupt the wait with `EINTR`; retrace does not. That was a
deliberate choice over the alternative — `EINTR` changes a guest-visible syscall return value, so it
needs `__pthread_join`'s retry loop measured by disassembly and `__ulock_wait`'s `ULF_NO_ERRNO`
convention modelled, which the current arm hardcodes as `err: false`. That is a milestone of its own
and nothing in the tree needs it yet.

The guard is `Box_::assert_no_stranded_signals`, wired into record's `SYS_EXIT` arm immediately
before the `Event::Exit` append. It scans every thread and panics, naming the thread and the signal,
if a `Blocked` thread is exiting with a signal its mask does not block. **Clean-exit path only** — a
guest already crashing must be diagnosed by its crash, not by a secondary guard firing on top of it.
The reason it exists is that a swallowed signal makes record and replay agree with each other and
**both be wrong**, which is the one failure shape a determinism oracle structurally cannot see. Five
tests pin it, three of them mutation-killers: dropping the `Blocked` check, dropping the deliverable
check, and using `pending` instead of `pending & !mask` each turn exactly the predicted test red and
leave the other two green.

**The guard scans `Blocked(_)` only, and that is correct precisely because `should_pend_for` pends
for `Blocked(_)` only — the two are one decision, not two.** `should_pend_for` narrows to
`Blocked(_)` rather than the looser "anything but `Runnable`" (commit `67855f5`, a plan-time
correction). An unmasked signal to an `Exited` target must keep reaching `check_deliverable`'s
panic, which is *earlier and more precise* than any exit-time guard: a signal to a dead thread is a
modelling bug, not a schedule divergence, and there is no wake to materialise it at. Had
`should_pend_for` pended for `Exited` too, the signal would have gone onto a dead thread's pending
set and been swallowed in silence — the exit guard would never have seen it, because it does not
scan `Exited`. Whoever widens either one must widen the other in the same commit.

**The headline gate came green with its assertions untouched.** `sigblocked_e2e` was committed in
M16 as real compiling code behind a real three-thread guest, parked at the panic, with assertions
written **correct-by-construction from M16 Task 13's measurement before they could ever be run**.
Task 7 deleted the `#[ignore]` attribute and rewrote the file's now-false narration; diffing the
parked and un-parked versions with comments and the attribute stripped shows the test body
**byte-for-byte identical**. That is the strongest thing that can be said for a parked gate: it was
un-parked by someone who did not write it, without relaxing one assertion. The gate asserts on the
**trace**, not the exit code, because the guest's handler is empty — an exit-code gate would have
come green under the single most likely wrong fix, silently *skipping* the blocked target, which
exits 0 on both sides and changes no stdout. `delivered == vec![1u32]` rejects that, and the
"blocked BEFORE the delivery" tooth keeps the gate about the blocked case rather than the
merely-not-current case `sigthread_e2e` already covers.

One honest note on that gate's ordering: M17 delivers at the **wake**, not at the raise, so the
delivery landmark sits after `b`'s `__ulock_wake` rather than at `b`'s `pthread_kill`. Both orderings
satisfy the assertions as written, which is why the second tooth — that thread 1 entered
`__ulock_wait` *before* the delivery index — is load-bearing rather than decorative.

**M17 makes `sigblocked_e2e` the third gate reaching `util::bin()` on every gate run, exactly as M16
predicted — and then adds a fourth M16 did not. But the hazard the prediction was about had already
been fixed, and we nearly wrote the opposite into this log.** M16's close named it:
`crates/retrace/tests/util/mod.rs::bin()` ran `codesign -f` on the *one shared* `retrace` binary, so
a second test **process** could observe it missing mid-replacement — which `--test-threads=1` does
not prevent, because it serialises threads *inside* a binary while cargo runs binaries concurrently.
M16 deferred the fix and wrote that `kport` and `sigthread_e2e` were two callers, "with
`sigblocked_e2e` a third the day it is un-parked — so it raises the collision odds it is deferring."

Today is that day, and the caller count did grow — **by two, where M16 predicted one**. The two are
`sigblocked_e2e`, which was in the tree but `#[ignore]`d and so never invoked `bin()` until Task 7
un-parked it, and `blockedctx`, Task 1's new measurement gate, which records through
`util::record_dynamic` like the rest. Those are the only two: `sigblocked_e2e` was the only such
file M17 un-parked (`stackoverflow_rust_e2e` was also fully `#[ignore]`d at `b73bdbb` and stays
parked, so it contributes nothing to the delta), and `blockedctx` is the only new one. **The delta
is stated rather than the absolute deliberately.** A first draft of this paragraph published
absolute totals for both commits; re-measurement did not reproduce them, and the number that carries
the argument is the change, not the base — M16's own "two callers" was likewise an ordinal counting
what M16 added, not a census of the directory. A prediction about one gate under-counted the
milestone that fulfilled it, which is the ordinary way this kind of thing grows: nobody adds a
`bin()` caller on purpose, they add a gate.

**The odds it raises are of nothing, because `bin()` was fixed in M16's own fast-follow sweep.**
Commit `92bc793`, "fast-follow A19b: sign a per-process copy, not the shared binary", landed after
M16's close at `dc04e48` and at or before `b73bdbb`. `bin()` now signs
`format!("{p}-signed-{}", std::process::id())` — a per-process copy — which is exactly the fix M16
named as the real one, applied by M16 itself within days of deferring it. So M17 adds two callers to
a race that no longer exists.

**Why this is recorded at length rather than quietly corrected.** M17's controller wrote a ruling
instructing this milestone to *state that it had raised the collision odds*, and this section was
drafted saying `bin()` was "not fixed" and M16's reason for deferring "still holds". Both were false.
The error came from reading M16's Status section as a description of the code *now* — and it is
history, true as of `dc04e48` and preserved verbatim, exactly as this file's contract requires. The
log is the authority on what was believed; the code is the authority on what is. Reading the first as
the second is the specific way an append-only history misleads a reader who trusts it, and this
milestone caught it one commit before it became permanent, by opening `util/mod.rs` instead of citing
the log a third time. M16's deferral text stays standing; this paragraph is its forward pointer.

**Fail-loud boundaries, unchanged or newly stated:**

- **A signal to a thread nothing wakes is never delivered** — the accepted gap above, guarded by
  `assert_no_stranded_signals` on the clean-exit path.
- **`BlockReason::Join` gaining a producer** would add a second materialisation site this design does
  not cover. It has no producer today (measured in M16 Task 13), so there is exactly one.
- **A signal to an `Exited` thread still panics** in `check_deliverable`, deliberately and by the
  argument above.
- **`guest_ulock_wait` / `guest_ulock_wake`'s operation-word asserts are untouched.** M17 changes who
  gets woken to what, never which operation words are modelled.
- **Signal queueing and nested delivery remain unmodelled** (M16), `sigwait` (330) and `sigsuspend`
  (111) still panic (M11), and a pended signal whose default action is Terminate still panics at
  materialisation rather than killing the process.
- **At most ONE signal materialises per wake**, and a second deliverable one on the woken thread now
  asserts. Added in the final-review round, not during the tasks: the review observed that the
  sibling case — one wake making several *threads* deliverable — got an explicit `deliver_to.len()
  <= 1` bound with the reasoning "measure the guest before modelling it", while the multi-*signal*
  case got nothing. It is the same argument, and the gap was invisible for a specific reason worth
  recording: `take_pending_delivery` takes one bit, the woken thread is `Runnable` by then, and
  `assert_no_stranded_signals` scans `Blocked(_)` threads only — so the residue would have been
  swallowed with record and replay agreeing, the one failure a determinism oracle cannot see. The
  assert sits *outside* the `Some`/`None` match on the take, because an `Ign` disposition returns
  `None` **after** the bit was consumed, so that path swallows too. Byte-identical message on both
  sides, verified by extracting and comparing the two literals rather than by reading them.

**What is still unexercised, honestly.** Replay's `mirror_delivery` `check_deliverable` **Err** branch
is genuinely called now — the function is on the live wake path — but the Err arm itself stays
unreached, and for a *different* reason than before M17: `guest_ulock_wake`'s own `unblock_waiters_on`
transitions the woken thread to `Runnable` inside the **same call** that produces the woken set, so
every `wtid` reaching that point is already `Runnable` on both sides. The arm needs a genuine
live-versus-recorded schedule mismatch to fire. Task 7 rewrote that comment to say so rather than
leave M16's "un-parking the gate would make this arm live" standing: un-parking made
`mirror_delivery` live, which is what M16 was half right about, but it did not make the **Err**
branch inside it reachable, which is what M16 meant. The `Crash` landmark's `verify_thread` site also
remains the one unexercised oracle site, for
the same reason as at M16's close: no threaded guest in the tree crashes. Unrelated to this
milestone, and still open.

**Everything M16 and earlier carry forward is unchanged**, none of it fixed here: per-thread reverse
execution, preemption, `workq`/GCD, thread priority, hardware watchpoint scoping,
`guest_bsdthread_create` still returning `0` where the real syscall returns the child's `pthread_t`,
`dbg_kport_of(tid: usize)`'s type, `dup2` (fail-loud), `fcntl(F_DUPFD)` (unmodelled and *not*
fail-loud), guest stdin still being retrace's, `RLIMIT_NOFILE`, asynchronous signals from outside the
process, per-thread *dispositions* (correctly process-global — POSIX, not a gap), and arm64e guests.

See `docs/superpowers/specs/2026-08-20-retrace-m17-blockedsignal-design.md`.

## Status: M18-workq (Stage 1) — libdispatch brings its workqueue up, and the recorder cannot follow it yet

No 🎉 on this one, deliberately. M18 Stage 1 moved the GCD wall a long way and did not clear it. The
headline gate `dispatch_e2e` (rung 5, a guest that `dispatch_async`es a block onto a global concurrent
queue) is parked `#[ignore]`d — parked *twice* in one milestone, once when it was written and once
when Stage 1 knocked out the wall it was written against. A milestone that parks a new gate for a
capability it does not have has regressed nothing; this section is what makes that claim checkable.

**Gate: 414 passed / 0 failed / 2 ignored across 104 test binaries, measured at `faad6ba`**, clippy
clean over `--workspace --all-targets -D warnings`. Reconciled against M17's 412/0/1 over 103 by
diffing `#[test]` counts file-by-file rather than trusting the sum: `main` carries 413 attributes
(412 + 1 ignored), HEAD carries 416, and the three new ones are the feature-word test, the
`guest_bsdthread_register` test, and the parked `dispatch_e2e` — so 416 = 414 passed + 2 ignored,
exactly. The +1 binary is `dispatch_e2e` itself. `cargo metadata` reports 97 test+lib+bin targets and
the chunked run executed all 97, plus 7 doc-test runs = the 104 above.

### The Stage-1 wall, and why it was the fourth instance of one recurring bug

libdispatch never reached a workqueue syscall. `_dispatch_root_queues_init_once` calls
`_pthread_workqueue_supported`, which trapped at `.cold.1` (BRK, `EC=0x3c ISS=0xb001 FSC=0x1`,
`pc=0x1804f5f20`) because `__pthread_supported_features` was 0. libpthread stores that word only when
`bsdthread_register` returns >= 1 — and retrace **forwarded** that call to its own process, which the
host kernel had already registered at startup. Measured: `ret=0x16 err=true`, EINVAL. A genuine
host-call failure, not a wrong-but-successful answer.

That is the same bug retrace has now found four times: the guest's fds were retrace's (M10), the
guest's signal dispositions were retrace's (M11), the guest's pthread registration was retrace's
(here). The fix is the same shape every time — the guest's X is the guest's.

### `bsdthread_register` stopped being forwarded for TWO reasons, and the second was the urgent one

1. The answer was wrong, as above.
2. **`args[0]` and `args[1]` are thread ENTRY POINTS.** Forwarding the call handed *guest* addresses
   to the host kernel as **retrace's own** process's thread-start functions — the same
   whole-process-fatal class as forwarding `bsdthread_create`, which retrace has asserted against
   since M14. It had been harmless only because it *failed*. **Latent since M14; closed here.**

Reason 2 is the one worth carrying forward: a call that is wrong-but-failing looks identical to a
call that is fine, right up until it starts succeeding.

### The feature word: synthesized, and pinned to its gates rather than to itself

`WORKQ_FEATURE_WORD = 0x4000005E`, the smallest value satisfying every gate measured in the shipped
binaries. The test asserts the four gates, each with its address, rather than restating the literal —
a test that only re-asserted `0x4000005E` would pass even if the value were wrong for its purpose:

- `__pthread_init +0x1040`: `cmp w0,#1 / b.lt` — below 1 and the word is never stored (the Stage-1 bug).
- `__pthread_init +0x1048`: `bics wzr, w8, w0` against `0x4000001E` — every one of those bits must be present.
- `_dispatch_root_queues_init_once` `0x180348F68`: `tbz w0,#4` → `.cold.5`.
- `0x180348F90/F94`: bit 7 set registers three worker callbacks including the workloop worker; bit 7
  clear with bit 6 set registers two. **Bit 7 is deliberately CLEAR — it is the scope lever that keeps
  the workloop path out of M18, not an accident.**

Being a fixed constant is also what makes it deterministic for free: both runs compute the identical
value with nothing recorded. Symmetry rule 2's argument applied to a return value instead of an
instruction.

### Approach B was killed by measurement, and that is worth recording

The obvious cheap milestone would have been to push libdispatch onto a non-workqueue fallback and
reuse the `pthread_create` machinery M14–M17 already proved. **That fallback does not exist for the
global root queues on macOS 26.** Every exit from the workqueue path is a `.cold.N` crash stub; there
is no branch to a pool initialiser. `__dispatch_worker_thread` *is* in the binary, but it belongs to
`_dispatch_pthread_root_queue_create` — the public API for *user-created* root queues, not the global
ones. Making `_pthread_workqueue_supported` answer "unsupported" does not buy a fallback; it buys
`.cold.5`. The global concurrent queues have exactly one implementation and it is the kernel
workqueue. The cost of learning this was one probe; the cost of learning it later would have been a
half-built milestone.

### What Stage 1 actually bought, measured

`workq_open` (367) and `workq_kernreturn` (368) **fire for the first time in this project's history** —
answering the spec's open question 4, which M14 and M18's own probe had both measured as "never". In
order, verbatim:

```
[trap] num=368 pc=0x1804af9f0 args=[0x400,0x27ff6a8,0x18,0x0,0x0,0x20]
[trap] num=367 pc=0x1804afa1c args=[0x0,0x27ff6a8,0x18,0x0,0x0,0x20]
[trap] num=368 pc=0x1804af9f0 args=[0x20,0x0,0x1,0x40008ff,0x0,0x20]
```

Note a `workq_kernreturn` fires *before* `workq_open`, not after. Two distinct opcodes in `args[0]`
are reached — `0x400` and `0x20` — and those raw values, not their names, are the measurement:
`pthread/workqueue_private.h` is a private header that ships in neither `/usr/include` nor the Xcode
SDK, so the plausible XNU names (`WQOPS_SETUP_DISPATCH`, `WQOPS_QUEUE_REQTHREADS`) are recorded as
unverified leads. The list is a floor, not a ceiling: the park/return opcodes a *running* worker would
issue cannot be enumerated until a worker runs.

### The new wall: forwarding 367/368 kills the recorder, and it is not even deterministic

Neither dispatch loop has an arm for 367 or 368, so both reach the generic forward arm and the **host
kernel acts on retrace's own process**: it brings up a real workqueue for the recorder, is told to
configure it for dispatch with *guest* pointers, is asked for worker threads — and duly creates a real
worker thread **inside retrace**, entering it at `start_wqthread` → `_pthread_wqthread`, which jumps
through a dispatch function pointer that is NULL in this process and dies at address 0.

`EXC_BAD_ACCESS / SIGSEGV, KERN_INVALID_ADDRESS at 0x0`, faulting thread 2, from the crash report.
**The `exit(139)` this produces is NOT `Outcome::Crash`** — a distinction that matters because 139 is
exactly what `crashy_e2e` asserts for an uncaught *guest* fault. Three independent tells separate
them: no `guest crashed:` line on stderr, the guest's buffered stdout is 0 bytes, and the trace tail is
cut mid-`args=[…]`. This is the third demonstration of one rule — **a syscall whose arguments are
addresses or whose effect is a thread must never be forwarded to the recorder's own process.**

It is also a determinism violation, and the cheapest possible proof of one: three identical
consecutive runs dispatched **252, 253 and 254** traps. A real host thread races the vCPU thread and
kills the process at a different point each time. Nothing nondeterministic entered a trace — the
recording never completes — but a racing host thread inside the recorder is precisely the class of
thing retrace exists not to have. Stage 2's first job is a fail-loud assert on that forward path,
the same shape `bsdthread_create` has carried since M14.

### Honest-gate posture at this close

- `dispatch_e2e` — **parked, re-parked once.** Its `#[ignore]` reason was rewritten from the Stage-1
  BRK to the Stage-2 host-worker SIGSEGV, and the stale reason deleted. Verified to be in exactly one
  honest state: `1 ignored` normally, and it genuinely **FAILS** when run `--ignored`, caught by its
  worker-ran assertion against empty stdout — a parked body that could not fail would be the thing
  this discipline exists to prevent. Its body also documents why it must never assert on the exit
  code alone.
- `stackoverflow_rust_e2e` — unchanged, still parked at M8 risk R3.

### Boundaries and non-changes, stated so a later reader does not go looking

- **The oracle census in `CLAUDE.md` is UNCHANGED by this milestone.** Seven `verify_thread` call
  sites plus the eighth inline comparison in `mirror_delivery`. Task 5's replay mirror sits inside the
  generic recorded-`Event::Syscall` block, which has already called `verify_thread` before reaching
  it; adding a second call there would have been wrong. There is no eighth site to find.
- **`set_thread_start_pc` was NOT deleted.** The plan made that conditional on it becoming unused; it
  did not — 12 callers remain in `retrace-box/tests/threads.rs`, which construct thread-start state
  directly rather than through a trap.
- `guest_bsdthread_register` records `err: false` and `writes: vec![]` because it writes no guest
  memory and returns a constant; the replay mirror recomputes it and byte-compares. That comparison is
  vacuous today and becomes the oracle the moment the return stops being constant — the same shape as
  `bsdthread_create`'s mirror.
- `wq_thread_pc()` and `pthread_size()` are captured and tested but **consumed by nothing**. They are
  Stage 2's, and they are the reason Stage 2 does not have to re-measure the worker entry contract.
- **Task 5 changed libpthread's init path for every dynamic guest, not just the dispatch one** — the
  one risk this plan carried and could not retire. The five threading/dynamic gates were run as the
  detector with an explicit instruction to report BLOCKED rather than patch around a regression:
  `thread_rust_e2e`, `sigthread_e2e`, `thread_watch_e2e`, `hello_dyn_e2e`, `hello_rust_e2e` all pass
  unchanged.

**Everything M17 and earlier carry forward is unchanged**, none of it fixed here: per-thread reverse
execution, preemption, thread priority, hardware watchpoint scoping, `guest_bsdthread_create` still
returning `0` where the real syscall returns the child's `pthread_t`, `dup2` (fail-loud),
`fcntl(F_DUPFD)` (unmodelled and not fail-loud), guest stdin still being retrace's, `RLIMIT_NOFILE`,
asynchronous signals from outside the process, the unexercised `Crash` oracle site, and arm64e guests.

The Stage-2 measurement this milestone exists to produce is in
`.superpowers/sdd/2026-08-20-retrace-m18-workq/stage2-measurements.md`. See
`docs/superpowers/specs/2026-08-20-retrace-m18-workq-design.md`.

## Status: M18-workq (Stage 2a) — the workqueue pair is the guest's, and the wall behind it is measured

Still no 🎉. Stage 2a did not make a libdispatch guest run; it removed the thing that made the
recorder *die* while trying, and it measured — rather than guessed — what stands behind that. The
headline gate `dispatch_e2e` is still parked, re-parked for the second time in one milestone, and
this section is what makes "parked at a real wall" checkable rather than a claim.

**Gate:** this section deliberately carries **no measured pass/fail stamp**. The closing task did not
run the full chunked workspace gate — it was run separately, at the close, measured at `67e9a13` (420
passed / 0 failed / 2 ignored across 104 test binaries; clippy re-verified at `4d0f780`), and the
README's "Gate" line now carries that stamp, not this section. What was measured here is the
arithmetic CLAUDE.md says to trust
over a sum: the tree carries **422 `#[test]` attributes and exactly 2 live `#[ignore]`** (counted
with `grep -rn '^\s*#\[ignore'`, anchored — an unanchored grep matches a dozen prose mentions). That
is Stage 1's close of 416 plus this stage's six: two `workq_open` tests and three
`workq_kernreturn` tests in `retrace-box/tests/threads.rs`, and one new end-to-end gate in
`retrace/tests/dispatch_e2e.rs`. The two parks are unchanged in *number*: Stage 2a parks nothing new
and un-parks nothing.

### What landed: `workq_open` and `workq_kernreturn` stop being the host's

`Box_::guest_workq_open` and `Box_::guest_workq_kernreturn` join the `bsdthread_*`/`ulock_*` family:
emulated in the box, never forwarded, with a record arm and a replay mirror each that call the same
method with the same arguments (symmetry rule 1), plus a fail-loud assert on the generic forward arm
so no later edit can silently re-forward them. Nothing new enters the trace and `TRACE_MAGIC` does
not move — the return is recomputed on both sides and byte-compared, the M2-setport posture.

`workq_open` returns 0 and asserts the guest has registered a `wqthread` first. It deliberately does
**not** assert that it precedes the first `workq_kernreturn`: the measured order is
`kernreturn(0x400)` → `open` → `kernreturn(0x20)`, so the plausible-looking ordering assert would
fire on the real sequence. `workq_kernreturn` dispatches on `args[0]` and refuses **by value**
anything unmeasured, the `guest_ulock_wake` posture — so the panic names what to go measure instead
of inventing an answer. The two opcodes any run has ever reached are `0x400` (dispatch setup, returns
0) and `0x20` (request threads, the wall below). Their XNU names, `WQOPS_SETUP_DISPATCH` and
`WQOPS_QUEUE_REQTHREADS`, are attributed leads, not verified facts: `pthread/workqueue_private.h`
ships in neither `/usr/include` nor the Xcode SDK. The raw values are the measurement.

This is the fourth instance of one recurring bug, and the phrasing is by now a template: the guest's
fds were retrace's (M10), the guest's signal dispositions were retrace's (M11), the guest's pthread
registration was retrace's (M18 Stage 1), and the guest's workqueue was retrace's (here).

### The wall is now a refusal retrace chose, and that is the point

`REQTHREADS` panics with "worker construction is Stage 2b". That is not an unfinished edge — it is
the deliberate shape. The kernel allocates a workqueue thread's stack and pthread struct and enters
`wqthread` with a register contract **no run in this project has measured**, so a worker built here
would be invention, and invention on this path does not fail loudly: it produces a guest that runs
plausible-looking wrong code. A named refusal costs one parked gate; a guessed success costs the
determinism claim the whole project rests on.

### What forwarding them actually did, and why the gate asserts a message rather than a code

Forwarding was **whole-process fatal for the recorder**, measured in Stage 1 from a crash report:
the host kernel brought up a real workqueue for retrace's own process, was handed *guest* pointers to
configure it with, was asked for workers, and created a real worker thread **inside retrace**,
entering it at `start_wqthread` → `_pthread_wqthread`, which jumps through a dispatch function
pointer that is NULL in this process and dies at address 0 — `exit(139)` from retrace's own SIGSEGV.

That number is exactly why Stage 2a's own gate,
`dispatch_e2e::the_workqueue_syscalls_are_emulated_not_forwarded` (**not** ignored), asserts on the
**panic message** and not on the exit code: `crashy_e2e` asserts 139 for an uncaught *guest* fault,
so no exit code can tell "retrace SIGSEGV'd" apart from "the guest faulted." The string "worker
construction is Stage 2b" can only reach stderr if the guest's `workq_kernreturn` arrived at
retrace's own emulation; `assert_ne!(code, 139)` and "no `_pthread_wqthread` on stderr" are named
supporting checks, so a regression reads as itself rather than as a bare red. The test was verified
able to fail: with the expected string swapped for one that is absent, it FAILS (and the recorder's
real panic at `retrace-box/src/lib.rs:3394` is visible in the captured stderr); restored, it passes.

### Two corrections this stage made to claims already written down

1. **Task 6's §4 trap-count attribution is withdrawn.** Stage 1 measured three runs at 252 / 253 /
   254 dispatched traps and read the instability as evidence of the racing host worker thread. It was
   not evidence: dyld/libSystem guests are already irreproducible run-to-run from forwarded
   `gettimeofday`/`getentropy`, as `util/mod.rs` had recorded since an earlier milestone. Task 4 then
   closed the argument from the other side — with no host worker thread on the path at all, two runs
   still differed by 4 traps, and the difference reconciled **exactly**: +3 `gettimeofday`, +1
   `MACH_VM_MAP`, zero residual. What stands from Stage 1 is the crash report, which was conclusive
   on its own. The lesson is narrow and worth keeping: *a real finding with a wrong supporting
   argument is still a wrong argument*, and the wrong half propagates.
2. **The `verify_thread` census stays at SEVEN.** The M18 spec's earlier section said the oracle
   "must grow with the mirrors" — that M18 adds "at least two such mirrors … and possibly a third",
   that each "needs its own `verify_thread`", and that `CLAUDE.md`'s census would be updated in the
   same commit as the last one. That was wrong for these two mirrors, and it was *measured* wrong
   rather than argued: Stage 2a's mirrors sit inside the generic recorded-`Event::Syscall` arm, which
   calls `verify_thread` **before** the `if num == …` chain begins, so they inherit the check. The
   rule underneath is the one to carry: a mirror that `return`s from *before* the arm's own
   `verify_thread` creates a hole and owes a site; a mirror placed *after* it inherits one, and
   adding a second call there would make the census wrong in the other direction. `CLAUDE.md` is
   unedited by this stage.

### What Task 4 measured behind the wall

With `REQTHREADS` temporarily stubbed to return 0 (reverted before commit), two independent runs
under an external 120 s alarm, streams kept separate. The document is
`docs/superpowers/specs/2026-08-21-retrace-m18-stage2b-measurements.md` — relocated by the closing
task out of the gitignored `.superpowers/` tree, where it had been the one `git add -f`ed file in the
repo's history and a trap for the next `git clean`.

- **`dispatch_semaphore_wait` does not lower to a `__ulock_wait`.** `num=515` appears nowhere in
  either trace. It lowers to `semaphore_create` (a `mach_msg2`, msgh_id 3418, already
  forward-allowlisted) whose reply mints port name `0x1403`, followed by a **raw Mach trap**,
  `num=-36` at `pc=0x1804adbb0`, carrying that same port in `args[0]`. The name
  `semaphore_wait_trap` is attributed from public XNU sources and **not verified on this machine** —
  the raw number is the measurement, and the finding holds whatever `-36` turns out to be called.
- **The `mach_msg2` at `pc=0x1804adc34` is not specific to anything.** It is libsystem_kernel's
  shared `mach_msg2` trampoline, hit 12 times per run across 10 distinct msgh_ids. An earlier draft
  named three of them as if that were the list; corrected in place before commit.
- **The run ends in a hang, not a crash.** `num=-36` has no dedicated arm, so it reaches `forward_and_diff` and
  issues a real blocking wait **in retrace's own process** on a port nothing in that process will
  ever signal. Both runs hung there and both produced 0 bytes of guest stdout (preserved artifacts,
  `wc -c`); the exit code was captured for only one of them — 142, the external alarm — and the
  measurement document says in bold that the other's is **unmeasured, not a different outcome**,
  because that run's recorder ended when the agent process driving it died rather than when an alarm
  fired. The hang itself is not an inference from either exit code: it is read off both traces
  ending on the identical trap with nothing after, and off the code path. This is the same "never
  forward it" rule as the workq pair, milder in kind — no new host thread, no null jump — and just as
  fatal to a recording.
- **No third `workq_kernreturn` opcode appeared**, even with REQTHREADS permissive. Still a floor and
  not a ceiling: the park/return opcodes a *running* worker issues cannot be enumerated until one
  runs.
- **The correlating value is in a different address space.** M14/M17's whole thread-blocking model
  keys on a guest memory address (`pthread + 0x34`) because that is what `__ulock_wait` carries. A
  mach semaphore's is a **port name in retrace's own IPC space**, minted by a forwarded call and
  never written into guest memory as such. Stage 2b's park/wake seam cannot be a copy of M17's; that
  is a design decision to make deliberately rather than discover by force-fit.

### Honest-gate posture at this close

- `dispatch_e2e::a_dispatch_async_guest_records_and_replays` — **parked, re-parked a second time.**
  The `#[ignore]` reason was rewritten whole (the Stage-1 forwarding reason deleted, not appended to)
  to name the Stage-2b wall, cite the measurement document, and say what un-parking requires: a
  worker built and entered at the registered `wqthread`, plus a seam for the mach semaphore. The body
  is unchanged; only the stale comment describing the *pre-2a* failure mode was corrected, because it
  claimed a SIGSEGV and an exit 139 that the adjacent new gate now asserts must not happen.
- `dispatch_e2e::the_workqueue_syscalls_are_emulated_not_forwarded` — **new and un-parked**, verified
  in both directions (passes; fails when its expected string is broken).
- `stackoverflow_rust_e2e` — unchanged, still parked at M8 risk R3.

### Boundaries and non-changes, stated so a later reader does not go looking

- **`CLAUDE.md` is unedited.** Census at seven `verify_thread` sites plus the eighth inline
  comparison in `mirror_delivery`. There is no new site to find.
- **The two replay mirrors are unreachable by any Stage 2a test**, and this is stated rather than
  papered over: record never completes a trace containing a workq landmark, because the run stops at
  `REQTHREADS`. They are correct by construction under symmetry rule 1 — same method, same args —
  and become exercised the moment Stage 2b lets a recording get past the wall. Fabricating a trace to
  "test" them would have tested a mirror against itself.
- **`wq_thread_pc()` / `pthread_size()` are still consumed by nothing.** Captured in Stage 1, and
  they exist precisely so Stage 2b does not have to re-measure the worker entry contract.
- **`retrace-arch`'s doc comments for 367/368 were rewritten**, not just the code: both said the
  syscalls had "NEVER fired," which Stage 1 falsified and Stage 2a services.
- **Everything M17 and earlier carry forward is unchanged**, none of it fixed here: per-thread reverse
  execution, preemption, thread priority, hardware watchpoint scoping, `guest_bsdthread_create` still
  returning `0` where the real syscall returns the child's `pthread_t`, `dup2` (fail-loud),
  `fcntl(F_DUPFD)` (unmodelled and not fail-loud), guest stdin still being retrace's, `RLIMIT_NOFILE`,
  asynchronous signals from outside the process, the unexercised `Crash` oracle site, and arm64e
  guests.

See `docs/superpowers/specs/2026-08-20-retrace-m18-workq-design.md` (the "Stage 2, split by what is
measured" section) and `docs/superpowers/plans/2026-08-21-retrace-m18-workq-stage2a.md`.

## Status: M18-workq (Stage 2b) — 🎉 a guest that `dispatch_async`es runs, records and replays

The 🎉 Stage 2a withheld. `dispatch_e2e::a_dispatch_async_guest_records_and_replays` is
**un-parked**: a dynamically-linked C guest that `dispatch_async`es a block onto a global concurrent
queue and joins it with a `dispatch_semaphore` records through real `/usr/lib/dyld`, replays
bit-for-bit, and replays again byte-identically. That is rung 5, and it is the first guest in this
project whose second thread of control is created by **libdispatch** rather than by the guest's own
`pthread_create`.

**Gate:** **442 passed / 0 failed / 1 ignored across 104 test binaries**, measured at `4928487`, clippy clean
over `--workspace --all-targets` with `-D warnings`. Run chunked, as every milestone since M14 has
been — the unchunked `--workspace` run exceeds the tool ceiling and gets killed — with each chunk's
exit code captured *before* any pipe and every one of them `0`.

Reconciled file-by-file against Stage 2a's close of 422 `#[test]` / 2 ignored at `67e9a13`, rather
than by trusting a sum. **The entire delta is one file.** `crates/retrace-box/tests/threads.rs` goes
42 -> 63: Task 2 deleted `workq_kernreturn_reqthreads_is_the_named_stage_2a_wall` (-1, the Stage 2a
wall it removed) and added ten `workq_reqthreads_*`; Task 3 added the park pair, the three semaphore
unit tests and six cross-seam cases; fix round 1 added
`sem_signal_refuses_to_wake_more_than_one_waiter`. 422 - 1 + 22 = **443 `#[test]`**, of which one is
ignored. No new test file: still 99 files with tests, still 104 binaries. The ignored count moved
2 -> 1 on the un-park and nothing else touched it.

The un-park was earned on this test's own green, not on the hand-run Task 4 took beside it. Task 4
had driven the same guest end-to-end through the bare CLI — that is a different argv, environment and
codesigning path — and the `#[ignore]` reason it left said so in as many words, parking the gate on
the explicit ground that *a gate must be un-parked on its own green*. Task 5 ran the body
(`--ignored`, ok, exit 0) and only then deleted the attribute. No assertion was loosened to earn it;
the body is unchanged across the un-park.

### What landed: the worker is built inside the VM, and the semaphore is a seam

Three things, all of them below or symmetric across the trace, none of them adding a byte to the
recording — `TRACE_MAGIC` does not move:

1. **`REQTHREADS` (`workq_kernreturn` opcode `0x20`) builds a worker.** Stage 2a's deliberate
   `panic!` is gone. `Box_::guest_workq_reqthreads` places a stack and a pthread struct in **one**
   `guest_vm_reserve` (so their relative placement is what §2c measured, not two bumps that could
   drift apart), seeds the measured `wqthread` entry contract into a fresh `ThreadCtx`, and enters
   the thread at the guest's **own registered** `wqthread` — an address the guest supplied at
   `bsdthread_register`, never an invented one.
2. **The mach-semaphore pair is a park/wake seam.** `semaphore_wait_trap` (`-36`) parks the caller in
   `BlockReason::Sem { port }`; `semaphore_signal_trap` (`-33`) wakes it. The key is a **port name in
   retrace's own IPC space**, because that is what the trap carries — M14/M17's `pthread + 0x34`
   address correlation has nothing to work on here, exactly as Stage 2a's measurement warned.
3. **The worker parks rather than returning.** `workq_kernreturn` opcode `0x4` is
   `Box_::guest_workq_park`, which blocks in `BlockReason::Parked` and never returns to the guest —
   libpthread `brk`s if it does.

Both new traps get a record arm and a replay mirror calling the same `Box_` method with the same
arguments (symmetry rule 1), positioned **immediately before** the generic negative-trap arm whose
first statement is Task 2's family-wide guard. That order is load-bearing: placed after it, the arms
would be dead code that compiles, passes clippy, and silently hits the guard instead. The guard stays
where it is, because the other five stubs of the verified `-39..=-33` family are still unserviced and
must keep reaching it.

### What Task 1 measured, and what it retired

`docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-wqthread-measurements.md`. Three results
worth carrying:

- **§3 — the trap numbers are now VERIFIED, and Stage 2a's attributions were right.** Read straight
  off libsystem_kernel's own stubs on this machine: `_semaphore_signal_trap` is `mov x16, #-0x21`
  (**-33**) and `_semaphore_wait_trap` is `mov x16, #-0x24` (**-36**). The whole `-33..=-39` block is
  pinned, cross-checked by two neighbours — `_mach_msg_overwrite_trap` at -32 and `_mach_msg2_trap`
  at -47, the latter matching the `MACH_MSG2 = -47` this crate has used since M2. Stage 2a's largest
  attribution debt is discharged: the predecessor document labelled both numbers with an explicit
  "not checked against this machine" caveat, and both labels held.
- **§4 — the struct-init hypothesis is CONFIRMED, and it is why this milestone is possible at all.**
  libpthread distinguishes a *fresh* worker from a *reused* one by entry-flags bit 17, and on the
  fresh path calls `__pthread_wqthread_setup`, which **writes** the pthread struct rather than
  reading it — including the struct's own PAC signature, computed in-guest with the guest's own keys
  (`mov w16,#0x5b9; pacdb x17,x16`). So retrace hands over zeroed memory with the bit clear and
  libpthread authors the layout itself. **Retrace invents an address, not a layout** — the property
  M14's rule wanted, and the reason worker construction turned out to be measurable rather than a
  reimplementation of the kernel.
- **§3d — the park opcode came free.** `0x4` with `(0, 0, 0)`, and a **no-return** contract:
  libpthread stores "BUG IN LIBPTHREAD: __workq_kernreturn returned" and falls into `brk #0x1` if it
  ever comes back. The design spec's risk 3 is partially retired — one park opcode known by value,
  its contract known too. Still a floor, not a ceiling.

### The `verify_thread` census stays at SEVEN — the plan's `7 → 9` was wrong

This stage's plan and design both said the oracle census had to grow to nine, one new site per new
mirror, and that `CLAUDE.md` would be edited to match. **That was wrong, and it was caught by reading
the code rather than by trusting the plan** (commit `d781c30`, a correction with no code change).
Both new mirrors sit *inside* the generic recorded-`Event::Syscall` arm, which calls `verify_thread`
**before** its `if num == …` chain begins — so they inherit the check, exactly as Stage 2a's two
mirrors do. Adding a call would have made the census wrong in the other direction.

The rule underneath is the one to carry, and it is now stated for the second milestone running: **a
mirror that `return`s from before the arm's own `verify_thread` creates a hole and owes a site; a
mirror placed after it inherits one.** Position, not count, is what to check.

### Honest-gate posture at this close

- `dispatch_e2e::a_dispatch_async_guest_records_and_replays` — **UN-PARKED.** Parked at Stage 1 and
  re-parked twice, each time at a measured wall; there is no wall left. This is the discipline
  closing the loop it opened: M18 parked a gate for a capability retrace did not have, moved it as
  each wall fell, and un-parked it on a real green.
- `dispatch_e2e::the_workqueue_syscalls_are_emulated_not_forwarded` — **kept and widened.** Its
  Stage-2a assertion on the panic message "worker construction is Stage 2b" is gone, because Task 2
  removed that panic. Both of its durable checks are unchanged verbatim: `assert_ne!(code, 139)` and
  "no `_pthread_wqthread` on stderr", the tripwire the file exists to keep.
- `stackoverflow_rust_e2e` — unchanged, still parked at M8 risk R3. **It is now the only parked gate
  in the tree.**

### The census this close added, and why a green run was not proof

Everything the headline gate asserts — `worker`, `done`, exit 0, two byte-identical replays — is
satisfied by a run in which **this milestone's code never executes.** That is not hypothetical:
`dispatch_semaphore_signal`'s fast path is a bare `ldaddl` on the count word inside libdispatch's own
object and issues **no trap at all** (§5 item 7). Main happens to reach `-36` and block before the
worker is scheduled, so the count is negative by the time the worker signals and the atomic falls
through to the trap — but had the worker run first, both halves would have taken their fast paths,
no landmark would exist, the arms would be dead code, and the guest would still have printed both
lines and exited 0.

So the companion test now reads the **trace** and takes a census: exactly one `-36` landmark, exactly
one `-33`, and — the load-bearing part — **different thread tags**. One thread waited and a
*different* thread signalled is a shape only an in-box worker, built by `guest_workq_reqthreads`,
scheduled by the box, parking and waking through `BlockReason::Sem`, can produce. This is
`segv_rust_e2e`'s rule applied here: assert on the difference your work makes, in the one form no
weaker path can fake.

**And that census assertion was written inverted.** It shipped as `assert_eq!(waits[0], signals[0])`
under a message demanding the tags DIFFER — an assertion that passes in exactly the case it exists to
forbid and fails on the correct run. It was caught at the close, by reading the assertion against its
own message before running it, and corrected to `assert_ne!`. The lesson is narrow and specific: **an
assertion and the message explaining it are two statements of the same claim, and nothing in the
toolchain checks them against each other.** Clippy cannot see it; a green gate would not have caught
the reverse case, where an inverted assertion passes. Read them as a pair. This is the same class as
Task 4's heredoc lesson — an artifact that compiles, passes clippy, and is wrong in a way only a
human reading it can see.

### Boundaries and non-changes, stated so a later reader does not go looking

- **`-33` wakes exactly ONE waiter, asserted by value** (fix round 1, `eca70d7`), in
  `Box_::guest_sem_signal` — below the trace, so both dispatch loops inherit the bound through the
  same call and there is no second site to keep in step. The bound guards **two** unmeasured things,
  not one: `semaphore_signal_all_trap` (`-34`) is a separate trap that is still refused by the
  family guard, **and** *which* waiter `-33` should pick when several are parked has never been
  measured — `unblock_sem_waiters_on` would impose thread-table order, which is arbitrary the moment
  there are two. Servicing the plural case owes both answers.
- **A pending signal on a semaphore-parked thread ABORTS rather than being delivered**, deliberately,
  on both sides identically. M17 materialises at `__ulock_wake` using a *measured* correction to the
  woken thread's saved context (`blockedctx.rs`: saved `x0` 0, saved SPSR left C-set). Nothing has
  measured the equivalent for a thread parked in `semaphore_wait_trap`, and no fixture in this tree
  produces one, so copying M17's correction here would be a guess at unmeasured saved state. The
  assert names the measurement that is owed first. Silently dropping the wake was the alternative and
  is the one failure a determinism oracle **cannot** see: record and replay would agree with each
  other while the signal vanished.
- **`workq_kernreturn` knows exactly three opcodes** — `0x400`, `0x20`, `0x4` — and refuses any other
  **by value**, naming it. The opcodes a running worker can issue cannot be enumerated until one
  issues them, and now that one does, the floor may rise.
- **The QoS entry-flags word `0x244004` is an EXTRAPOLATION, not an observation** — Task 1's single
  load-bearing unverified claim (§1e, §5 item 4). Six queue configurations were tried and none
  reproduced the guest's own `0x040008ff` request live, because a host process's main thread carries
  a real QoS. Three neighbouring `(request → flags)` pairs were measured and the inversion is
  checked against all three. If a worker ever misbehaves in a way that smells like a QoS bucket,
  suspect this first.
- **One worker per request, `1..=WQ_MAX_WORKERS_PER_REQUEST`.** Every observed request asks for 1.
- **`thread_selfid` (372) hands every guest thread retrace's own host tid.** This is **pre-existing
  since M14**, not a Stage 2b regression — `retrace-arch` has forwarded 372 generically since before
  M14, so `thread_rust_e2e` and `sigthread_e2e` already share one id across threads. Recorded here
  because Stage 2b is the first stage where a reader might reasonably mistake it for new.
- **`dispatch_e2e` now costs two full dyld record runs of the same guest plus two replays**, because
  the companion test's first assertion is the headline gate's own. Accepted as the price of keeping
  the tripwire independent of the gate it guards.
- **`CLAUDE.md` is unedited by this stage** — see the note below, which is a decision for the
  repository's owner rather than something a milestone should take on its own.
- **Everything M17 and earlier carry forward is unchanged**, none of it fixed here: per-thread reverse
  execution, preemption, thread priority, hardware watchpoint scoping, `guest_bsdthread_create` still
  returning `0` where the real syscall returns the child's `pthread_t`, `dup2` (fail-loud),
  `fcntl(F_DUPFD)` (unmodelled and not fail-loud), guest stdin still being retrace's, `RLIMIT_NOFILE`,
  asynchronous signals from outside the process, the unexercised `Crash` oracle site, and arm64e
  guests.

### One thing this stage did NOT do, and is handing to the reader

`CLAUDE.md`'s "Guest threads" section is now **factually incomplete** in two sentences, and Stage 2b
deliberately left them standing rather than edit the repository's own instruction file on a
subagent's finding:

1. *"Blocking is `__ulock_wait` (515) and waking is `__ulock_wake` (516), correlated by address
   equality on `pthread + 0x34`"* — there is now a **second** blocking primitive with a **different**
   correlation key (the mach semaphore pair, keyed on a port name) and a **third** block state with
   no waker at all (`BlockReason::Parked`).
2. The *"two independent reasons with two matching materialisation sites"* paragraph now has an
   exception: Stage 2b adds a pend-capable `Blocked` state whose wake deliberately **asserts**
   instead of materialising, for the reason in the boundaries list above.

The section is demonstrably milestone-maintained — it already names M14, M15, M16 and M17 — so this
is factual correction rather than policy change. It is two sentences.

See `docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-design.md`,
`docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-wqthread-measurements.md`, and
`docs/superpowers/plans/2026-08-23-retrace-m18-stage2b.md`.

---

## Status: M18 fast-follow — the `Crash` oracle site stops being the one nobody tested

Closes the gap the three sections above carried forward by name. Those sections are left exactly as
written: each was true when written, and "the unexercised `Crash` oracle site" appearing in their
boundary lists is the record of how long it stayed open, not an error to be tidied away.

### The hole, and why it survived four milestones

`ReplaySession::advance` calls `verify_thread` at seven sites, one per arm that consumes a landmark
and `return`s before the generic dispatch, plus an eighth inline comparison in `mirror_delivery`.
Six of the seven had a test that retagged a real recording and proved the check fires. `Crash` had
none.

Not from neglect — from **absence**. Every crashing guest in the tree is single-threaded (`crashy`,
`segvy`, the `asm/` micro-guests), and every threaded guest exits cleanly (`threadrust`,
`watchthread`, `sigthread`, `sigblocked`, `dispatch_dyn`). The intersection was empty, so the
terminal `Event::Crash`'s thread tag had never once been recorded as anything but main's, and a
retag test had no second live thread to retag *to*. M16 created the site; M16, M17 and both M18
stages each noted it and moved on, because closing it needed a fixture rather than a fix.

### `crashthread`, and why its schedule is the whole design

`crates/retrace-guest/c/crashthread.c`: `main` writes, spawns a child, and blocks in `pthread_join`;
the child writes, then stores to `0x4000DEAD0000` with no handler installed.

The ordering is a consequence of the **cooperative scheduler**, not of source order. The box switches
only when a thread blocks or exits, so main runs uninterrupted through `pthread_create` and does not
yield until `pthread_join`'s `__ulock_wait`. Only then does the child run — so the child holds the
vCPU when it faults, and the `Crash` landmark is tagged with the **child**. A nonzero tag: the case
no recording had produced.

Three choices in that file are load-bearing and each is commented where it is made:

- **C, not Rust.** A full-`std` Rust guest installs libstd's own `SIGSEGV` handler, so the fault
  would route through `SignalDelivery` → `sigreturn` → re-fault and never reach the `Crash` path at
  all. That is precisely what `segv_rust_e2e` exists to assert. Here there is no handler, so the
  disposition is not a handler and the fault lands on `Crash` directly — the distinction `CLAUDE.md`
  draws between a raised signal and a hardware fault.
- **Both threads write.** The retag needs two *distinct live* thread ids in the trace, and a thread
  that issues no syscall contributes no id. Without the child's `write` the mutation would degrade
  into the bogus-constant form M15 was stuck with.
- **The same poison constant** as `crashy.c` and `asm/crash.s` (bit 46 set, `< 2^47`), so the fault
  is a stage-1 EL0 data abort with `FAR == GARBAGE_VA` and cannot be mistaken by the demand-pager or
  the reservation-commit path for work of its own.

### The failure it catches is the kind a green run cannot rule out

`a_wrong_thread_on_the_crash_landmark_is_a_divergence` was **verified able to fail**, and what the
verification showed is the reason the site mattered more than its five-year-old-looking size
suggests.

With `self.verify_thread(*rthread, pc)?` deleted from the `Crash` arm and nothing else changed,
replay does **not** report some other problem. It accepts the retagged trace, completes the crash,
and exits **139** — byte-for-byte the outcome a *correct* replay of this guest produces. The
uncaught case is therefore not merely undetected; it is **indistinguishable from success by every
signal outside the oracle**. Exit code, stdout, and the final memory compare all agree with a clean
run. Only the check itself can tell them apart, which is exactly why it could not be left
unexercised. Restored, the test passes.

This is the `Crash`-arm instance of the rule M15 stated for `Syscall`: two threads running the same
code produce byte-identical landmarks, so identity has to be checked rather than inferred. On the
terminal arm it is sharper still, because there is no *subsequent* divergence to catch what the
missing check let through — the run is over.

### Gate

**443 passed / 0 failed / 1 ignored across 104 test binaries**, measured at `114b19d`; clippy clean
at `-D warnings`. Run chunked, one `--test` target per invocation, with cargo's exit code captured
before any pipe — all 54 chunks `rc=0`.

Reconciles against Stage 2b's 442/0/1 at `4928487` by **exactly the one test added here**: `#[test]`
count in source is 444 and 444 ran, the binary count is unchanged at 104 because the test joined an
existing target, and the lone ignored gate is still `stackoverflow_rust_e2e` at M8 risk R3. No new
gate was parked and none was un-parked.

### Boundaries

`TRACE_MAGIC` did not move and no `Event` variant changed — this is a fixture and a test, not a
format or dispatch change. The `verify_thread` census stays at **seven**, plus `mirror_delivery`'s
inline eighth; nothing was added to either dispatch loop. What changed is that all eight are now
**exercised**, where before, seven were.

Still open, and unchanged by this work: asynchronous signals from outside the process, arm64e guests,
preemption-dependent races (the scheduler is cooperative by design), and symbol-level debugging —
addresses are still raw hex, which is what M19 takes up.

One note for a later reader on where the primary record lives: the `#[ignore]` reason on a parked
test, and the doc comment on a retag test, are the primary records for those tests. This section
summarises; it does not restate them, so the two cannot drift.


## Status: M19-symbols — 🎉 the debugger says `_child+0x30`, and never opens the binary to do it

`guest crashed: pc=0x10000050c far=0x4000dead0000 esr=0x92000045  in _child+0x30`.

The address on that line is the one M18's fast-follow already printed. The four words after it are
the milestone — and the interesting claim is not that the name appears, but **where it comes from**:
the recording, and nothing else. No binary path is supplied, no `--exe` flag exists, no file is
opened at debug time, and `TRACE_MAGIC` did not move.

### The symbols were already in every recording

M19 is the rare milestone whose enabling work was done years of milestones earlier and never noticed.
Two facts, measured before the design was written rather than assumed:

- **M4** — `parse_macho` maps every `LC_SEGMENT_64` except `__PAGEZERO`, so `__LINKEDIT`, which holds
  the `nlist_64` array and the string table, is mapped into guest memory like any other segment.
- **M5** — `Box_::snapshot` captures every backing in full.

Together those mean the symbol table is inside the opening `Event::Snapshot` of **every recording
already made in the current format**. M19 adds no field, no variant, and no bytes; it reads what M4
and M5 had been putting there all along. Recordings made before this milestone gained symbols
retroactively, which is the sharpest available evidence that nothing was added to the format.

That is also why the milestone is safe. The module is a pure function of bytes that are already in
the trace plus the fixed IPA layout constants. It never touches `record_box`,
`ReplaySession::advance`, or `Box_::run()`; **neither symmetry rule is engaged and the divergence
oracle cannot see M19 at all**, because nothing here is capable of making a recording diverge. A
milestone the oracle cannot see is normally a reason for suspicion — here it is a structural
consequence of staying above the trace, and it is what made a one-pass implementation defensible
where M18 needed three staged ones.

### Why not `--exe <path>`, the obvious alternative

The trace carries no path, UUID, or image identity (M6), so the two candidate designs were a
format break or a debug-time flag naming the binary. The flag is worse than it looks. A path can
name a *different build* than the one recorded — same filename, recompiled since — and the failure
mode is not an error but a confidently wrong name attached to a real address. Silent
mis-symbolication is worse than no symbolication, because hex at least tells the truth.

Reading the snapshot does not merely avoid that mismatch; it makes it **unrepresentable**. There is
no second artifact to disagree with, and no staleness window. The limits that remain (below) are
limits on what the recording *contains*, which is a much better class of limit to have.

### `LC_SYMTAB`, not the exports trie — and the lowercase `t` that decides it

M1 measured `crashthread`'s six symbols and found the one that matters is a **local**:

```
0000000100000460 T _main
00000001000004dc t _child        <-- lowercase t
```

`_child` is `static`, so it is in `LC_SYMTAB` but not in `LC_DYSYMTAB`'s external range and not in
the exports trie. A symbolicator built on exports — the more modern-looking choice — would name
`_main`, miss `_child`, and so fail to name **the exact function the M18 fast-follow exists to make
crash**. The reader parses `nlist_64` for that reason and no other. `0x10000050c − 0x1000004dc =
0x30` resolves with no slide arithmetic at all, because `EXE_BASE` equals the executable's own
`__TEXT` vmaddr (M2) — a property of the chosen IPA layout, not a coincidence of one binary.

### Two defects measurement caught that review would not have

**The design spec's own risk mitigation was wrong (P3).** R3 named the failure mode "confidently
wrong names" and prescribed deriving dyld's slide as `DYLD_BASE − dyld __TEXT vmaddr`. Measuring it
showed dyld's `__TEXT` vmaddr is `0x0`, so that expression yields the right number *here* — and only
here. It is the wrong rule, and for any image with a nonzero vmaddr it produces exactly the
confidently-wrong slide R3 was written to prevent. The mitigation contained the bug it was guarding
against, and it read as correct until a number was put next to it. The rule is the loader's own,
uniform across both images: `guest_va = file_vmaddr + slide`, with `slide` `0` for the main
executable and `DYLD_BASE` for dyld. The spec's R3 row was edited in place, before it had been
committed or acted on, and the measurements document records that it changed.

**`N_SECT` numerically equals the `N_TYPE` mask (P2).** Both are `0x0e`. The correct test is
`n_type & N_TYPE == N_SECT`; the slip `n_type & N_SECT != 0` compiles, reads plausibly, and silently
accepts `N_PBUD` (`0xc`) and `N_INDR` (`0xa`), neither of which carries an address in `n_value`. The
constant is spelled out rather than inlined so the equality is visible at the use site, and
`an_indirect_symbol_is_dropped` pins it.

### One deliberate deviation from the plan: malformation does not assert

The plan's global constraints said "absence is data, malformation is a bug", and required a
malformed table — offsets outside `__LINKEDIT`, an `n_strx` past `strsize` — to **assert**. The
implementation returns `None` and skips the entry instead, and the deviation is recorded at the
decision itself (`symbols.rs`, `for_image`'s doc comment) rather than left for a reader to discover
as a discrepancy.

The reasoning is a cost asymmetry that the constraint, written before the call site existed, could
not see. This code runs inside an interactive debug session over a *recording of a crash* — often
the only copy of a bug someone is chasing. A panic there costs the session; printing hex where a
name was possible costs almost nothing. Fail-loud is the right posture for the recorder, where a
wrong byte silently corrupts a trace; it is the wrong posture for a presentation layer that cannot
corrupt anything. A reader bug does not hide behind the leniency, because the unit tests assert on
specific *resolved names* — a reader that silently produced nothing would fail them rather than pass
quietly.

The rule behind it is that **symbolication may never fail a debug session**. It is
presentation; a name is a convenience and its absence must cost nothing. So `Exec::new` builds the
table inside a chain that ends in `unwrap_or_default()`: an unreadable trace, an absent `Snapshot`,
or a stripped image all yield an empty table rather than an error. No `Divergence` can originate in
this milestone, and none does.

The design spec left three questions for implementation, and all three closed. Two closed as the
spec leaned: only pc-bearing lines are symbolicated, and the table is built once per session rather
than per query. The third — R1, whether `__LINKEDIT` spans `Region`s — was settled by measurement
(P1: exactly one region for both images) and then **deliberately not relied on**: the spanning
gather was written anyway, because other backings in the same snapshot genuinely are per-page, so a
reader that assumed one-region-per-lookup would be correct today and wrong the first time anything
else was read.

Worth recording is *how* the once-per-session question closed, because the spec flagged a real
hazard: the seek machinery restores snapshots repeatedly, so a table cached off "the session's
snapshot" would need a cache key nobody had thought through. The implementation sidesteps the hazard
instead of solving it — it reads the **opening** `Snapshot` straight from the trace via
`Reader::open`, independent of wherever the session has since seeked. There is no key because there
is nothing to invalidate: the image as loaded is what every pc in the session refers to, and it never
changes. That is also why the debug CLI took a real dependency on `retrace-trace`, which it had
previously needed only in its tests.

`resolve` also clamps at `text_end` and returns `None` past it, rather than the nearest preceding
symbol. Without the clamp, any address above the last symbol — a cache pc, a stack address — would
resolve to `last_symbol + huge_offset`: a name, always, and wrong whenever it appeared. Bare hex is
the correct answer to "I don't know," and `an_address_with_no_symbol_degrades_to_bare_hex` pins it.

### The annotation is a suffix, deliberately

Every symbolicated line is `…existing text…  in _child+0x30` — appended at end of line, never
inserted after the address. `crashy_cli` greps `guest crashed: pc={pc:#x} far=…`; `debug_cli` greps
`hit 0x{pc:x} at (`. Inserting the name inside those lines would have broken established assertions
for no gain, and the two tests that did need touching were loosened by exactly the width of the new
suffix (an `ends_with` on the final `where` line became a `contains`), not weakened in what they
check. The raw address survives in every case, symbolicated or not — `format` is tested for it
explicitly, because something elsewhere in the tree may still be grepping for it.

`far` and `x <addr>` were left alone on purpose: `far` is a data address whose nearest text symbol is
noise, and an operand the user typed does not need to be told back to them.

One consequence a later reader should not have to rediscover: there are **two renderings**, and the
CLI does not use the library's. `Symbols::format` produces the self-contained
`0x10000050c (_child+0x30)`; the debug CLI uses its own `Exec::annot`, which produces the
` in _child+0x30` suffix appended to a line that already printed the address. The split exists
because the CLI's lines already contain the address in a shape other tests grep for, so a
self-contained rendering would have had to replace text those assertions depend on. `format` remains
the right thing for any consumer holding an address and no line to append to, and it is the one the
unit tests pin.

### The gate asserts the name, because the address is not the difference

Honest-gate discipline has a specific bite here. `pc=0x10000050c` printed before M19 too, so a gate
asserting on the address passes against a no-op implementation — it would be green on the day the
work started. The headline `the_debug_cli_names_the_faulting_function` therefore asserts on
**`_child`**, with a second assertion that the address is still present beside it.

**Verified able to fail, in this session rather than on report.** With
`SymbolTable::resolve` stubbed to return `None` — a faithful simulation of "M19 was never written" —
`symbols_e2e` goes to **0 passed / 4 failed**; restored, **4 passed / 0 failed**. The stub was
applied to the resolver rather than to the CLI on purpose: it makes every address unresolvable
without touching the printing path, so what goes red is the naming and nothing else.

Worth recording is *which* four went red, because one of them is a negative test and negative tests
are where vacuous greens live. `an_address_with_no_symbol_degrades_to_bare_hex` asserts that an
unresolvable address prints as hex — which a totally broken symbolicator would satisfy perfectly.
It fails under the stub anyway, because it carries a guard asserting that `hello_dyn`'s *own* image
still yields a usable table: "or the bare-hex assertion above is vacuous". So the test cannot pass by
everything being unresolvable, which is the only way a bare-hex assertion can lie.

### What the full gate caught that the working tree did not admit

The implementation looked finished before the gate ran. It was not: **four assertions in two files
were broken**, and only a complete chunked run surfaced them.

`watch_cli` (3 failures) and `thread_watch_e2e` (1) both pin the *end* of a `where` or watch-hit
line with `ends_with`. M19 appends its annotation to exactly those lines, so all four broke. Two
sibling files — `crashy_cli` and `debug_cli` — had already been updated for precisely this reason,
which is what made the omission easy to miss: the problem had been recognised and then only
partially applied. Nothing about the tree advertised the gap; the new gate was green, the new module
was green, and the failures lived in tests M19 never mentions.

The interesting part is the **fix that would have been wrong**. The two already-updated files had
been loosened from `ends_with` to `contains`, so copying that pattern was the obvious move — and for
`thread_watch_e2e` it would have silently destroyed the assertion. Its own comment says why it used
`ends_with`: `cmd_where` prints nothing after the thread id, so `thread=1` can only be a suffix, and
`contains("thread=1")` would **also pass a wrong-thread `thread=10`**. That test exists to catch a
misattributed store; loosening it would have left it green and blind, which is the same failure
shape as the M18 fast-follow's unexercised `Crash` site — a check that still runs and no longer
checks.

So the fix strips the annotation and **keeps** `ends_with`, via one shared helper
(`util::strip_annot`) rather than four local hacks. The same helper was then applied *back* to
`crashy_cli` and `debug_cli`, restoring the strength those two lost when they were loosened. Six
assertions now hold exactly the property they held before M19, and the one file that documented why
its form mattered is the reason all six do.

Two process notes a later reader may want. First, `contains` is the natural repair for a broken
`ends_with` and is *usually* harmless — the case where it is not is the case where a shorter value
is a prefix of a longer one, which is invisible unless the test says so. This one said so, in a
comment; had it not, the weakening would have shipped. Second, the gate that caught all four is the
per-target chunked run, not the new milestone's own tests: M19's gate was green throughout.

### The wall, and the gate parked at it

`cache_symbol_e2e` is parked `#[ignore]`d, and the reason on the test is the primary record.

Cache images carry no `LC_SYMTAB` in the region mapped into the guest, and the cache's local-symbol
area lives in a separate part of the on-disk cache file that `cache.rs` demand-pages for page
*contents* but never stages into guest memory. So the symmetry that makes the exe and dyld work —
`__LINKEDIT` mapped, therefore snapshotted — simply does not hold for the cache. Those symbols are
not in the recording.

This is the honest size of the limit, and it is large: most of a dynamically-linked guest's executing
pcs are *in* the cache, so M19 is the difference between naming your own functions and naming
everything. Clearing it owes a measurement, not an afternoon: either stage the local-symbol area at
record time — a determinism and recording-size question — or record a cache identity the debugger can
verify a local file against before trusting it. Reading the on-disk cache unverified would
reintroduce precisely the external-file dependency, and the stale-artifact mis-symbolication, that
choosing the snapshot eliminated.

Parking a gate for a capability the milestone does not have has regressed nothing; `dispatch_e2e` was
parked the same way by M18, moved twice as each measured wall fell, and then cleared.

**Both halves of that limit were observed directly, on a real `jq` recording**, rather than argued
from the code. Breaking at jq's own `_main` and stepping forward:

```
hit 0x100001130 at (294, +?)  in _main      <- jq's own 7 symbols: resolves
at (294, 203) pc=0x1804f8bf0 thread=0       <- shared cache: bare hex, no annotation
at (294, 603) pc=0x180389414 thread=0       <- shared cache: bare hex, no annotation
```

Exit 0 throughout, no panic. This is the stripped-binary case and the shared-cache case in one
transcript, and it is worth having because it shows the *shape* of the limit rather than its
statement: jq's own table is thin but real and names the entry point, while forty instructions later
the guest is in libSystem and every pc after that is a number. It is also the argument for the
`text_end` clamp — without it those cache pcs would each have resolved to jq's last symbol plus an
enormous offset, and the transcript would have looked informative while being false.

### Gate

**461 passed / 0 failed / 2 ignored across 105 test binaries**, measured at `97a4163`; clippy
clean at `-D warnings` over `--workspace --all-targets`. Run chunked — the workspace chunk,
`retrace-box`, `--bins`, and one `--test` target per invocation for each of the 52 `retrace`
integration gates — with cargo's exit code captured before any pipe. **All 56 chunks `rc=0`**
(55 test chunks plus clippy).

Reconciles against the M18 fast-follow's 443/0/1 at `114b19d` by exactly the code added here, checked
by diffing `#[test]` counts file-by-file rather than trusting a sum: **444 → 463**, with the only
per-file deltas being the two new files — `crates/retrace-core/src/symbols.rs` (+14 unit tests, no VM,
in the fast workspace chunk) and `crates/retrace/tests/symbols_e2e.rs` (+5, of which 4 are live). No
existing file's count moved, which is the check that M19 was additive rather than a rewrite.

The ignored count moves **1 → 2**, and the second is the deliberate `cache_symbol_e2e` above. The
binary count moves **104 → 105**, because `symbols_e2e` is a **new** integration-test target — where
the M18 fast-follow's one added test had joined an existing one, which is why that close moved the
test total without moving the binary total.

### Boundaries

`TRACE_MAGIC` is still `RT\x00\x08` and no `Event` variant changed. Nothing under `record_box`,
`ReplaySession::advance`, or `Box_::run()` was modified — the only edit to `retrace-core/src/lib.rs`
is the one-line `pub mod symbols;`. The `verify_thread` census is untouched at **seven** call sites
plus `mirror_delivery`'s inline eighth.

Still open, and stated as limits rather than left implicit: shared-cache addresses (above); stripped
binaries, which yield nothing because the *binary* kept nothing — `brew jq` ships 7 defined text
symbols against `threadrust`'s 969, and that is a fact about jq, not about retrace; and mangled Rust
names, printed as `_ZN…E` because a raw mangled name beats hex and needs no demangler.

Symbolication is also still **output-only**: `break _main` does not work, because every debugger
*operand* remains a raw address. The table needed to reverse that lookup now exists, so this is a
smaller gap than it was — but it is not closed, and no test claims it is.

Unchanged by this work: no DWARF and so no line numbers — an address becomes `_child+0x30` and never
`crashthread.c:35` — no unwinder and so no backtraces, asynchronous signals from outside the process,
arm64e guests, and preemption-dependent races.

---

## Status: M20-symbolops — the debugger stops demanding hex back

`break _child` works. That is the whole milestone, and its point is smaller and sharper than a
feature list suggests: M19 taught the debugger to *print* `in _child+0x30`, and every operand stayed
a raw address, so the tool spent a milestone telling you a name it would then refuse to accept. You
read the name off the transcript, went to `nm`, and typed the hex back. M20 closes that loop.

Like M19 it is presentation-layer. Nothing under `record_box`, `ReplaySession::advance`, or
`Box_::run()` was touched; `TRACE_MAGIC` is still `RT\x00\x08`; **neither symmetry rule is engaged
and the divergence oracle cannot see this milestone**, because nothing here can make a recording
diverge.

### The measurement that decided the design, and the one that was wrong

**S1 is the binding constraint.** `run_script` calls `parse_script` to completion *before* it calls
`Exec::new`, so at parse time the trace is not open and `Exec::syms` does not exist. M20 therefore
cannot resolve names inside `parse_addr`, which is where anyone would first try to put it. The
obvious repair — build `Exec` first, then parse with the table in hand — is foreclosed by a contract
`debug.rs` states in its own header: an over-long `x` span is *deliberately* a parse error raised
**before any VM work**. Reordering to buy a smaller diff would move every parse diagnostic behind VM
setup.

So `Cmd::Break`/`Cmd::Delete` carry an `Operand { Addr(u64), Sym(String) }`, parsing classifies but
never fails on an unknown name, and `Exec` resolves when it *runs* the command.

**S4 is the hard problem, and it is the one M19's code could not have shown.** M19's direction is
total: an address falls inside exactly one symbol's range. The reverse is not a function at all.
A real `threadrust` binds **19** names to more than one address — compiler-generated locals
(`_OUTLINED_FUNCTION_0`, `GCC_except_table0`) repeated per translation unit, every one of which
Mach-O keeps — and one dyld name, `___Block_byref_object_copy_`, carries **13 distinct addresses**.
"Pick the lowest" would silently choose one of thirteen, and the transcript would look entirely
normal. So an ambiguous name is an **error that lists every candidate**, and a name matching nothing
is an error that never falls back to reinterpreting the token as hex.

**And S4's first draft was wrong by a factor of ~235.** It reported dyld as 6331 defined text
symbols with **3255** duplicated names. `/usr/lib/dyld` is a Mach-O **universal binary**, and `nm`
without `-arch` concatenates the `x86_64` and `arm64e` slices, so almost every symbol appears twice
and reads as duplicated. The recorded guest loads the arm64e slice only, where the real figure is
**14**.

What caught it is worth recording, because it did not look like a measurement bug. The plan's own
Self-Review step said "check against a real dyld name from S4, not only a synthetic one". The first
name tried, `____chkstk_darwin`, resolved to a *single* address instead of erroring — which reads as
an implementation bug in `addrs_of`. It was not: that name is duplicated only *across* slices and
occurs exactly once in arm64e, so resolving it was correct and the number was wrong. A synthetic
test would never have surfaced it, and neither would a green gate.

The correction changed the rhetoric and not the design. 19 in `threadrust`, 14 in dyld, and a single
name carrying 13 addresses all say the identical thing: name → address is not a function, and a
lookup that silently picks is wrong on real input. **The conclusion was over-argued, not
unsupported** — which is a distinction worth naming, because the tempting response to discovering a
supporting number is inflated is to re-examine the conclusion, and here the conclusion never rested
on the inflated number.

The measurements document was corrected **in place**, with a note recording that it changed. That is
the opposite of this log's rule and deliberately so: a spec records what is true, while
`docs/status-log.md` is append-only precisely so an earlier claim that proved wrong is left standing.

### Rules decided rather than fallen into

- **Hex wins.** `0x`-prefixed ⇒ address; parses completely as hex ⇒ address; otherwise ⇒ name. Rule 2
  is what keeps every existing debug script working verbatim, and it costs one thing: a symbol
  literally named `deadbeef` is unreachable. Documented rather than papered over — Mach-O C symbols
  carry a leading underscore, so `_deadbeef` lands on rule 3 cleanly, and mangled Rust names are
  never all-hex. A sigil escape hatch is the additive follow-up if anything ever needs it; M20 does
  not build one speculatively.
- **The executable shadows dyld**, and that is a stated rule with a test, not an artifact of
  `images` happening to be built in `[EXE_BASE, DYLD_BASE]` order — a later reader reordering that
  array must break a test rather than silently change where a breakpoint lands. Matches are *not*
  merged across images: returning the union would report a name as ambiguous whenever dyld happened
  to define it too, refusing breakpoints the user is entitled to set. Measured mitigation for the
  common case — dyld does not define `_main`.
- **Exact match only.** Substring or suffix matching on mangled names is a convenience that
  reintroduces by construction the ambiguity S4 exists to refuse.

### The cost that was measured instead of discovered

Resolving at execution has one observable consequence, and it is a regression. `where; break zzz`
used to print **nothing** and exit 5, because a bad operand rejected the whole script before any
command ran. It now runs the `where`, prints it, and *then* fails — still exit 5. The exit code, the
compatibility question one would expect to be the hard one, does not change at all: `main.rs` has a
single `Err` arm, so parse errors and execution errors already shared exit 5.

This is in the design spec, in the README's Known limits, and pinned by a test. A behavioural
regression that is stated up front and asserted is a different object from one found later.

### `watch <name>` is out of scope on evidence, not effort

`nlist_64` has five fields in 16 bytes and **no size**. A symbol supplies an address and nothing
else. `watch` takes `<addr> [len]` and `x` takes `<addr> <len>`, so `watch _global` would have to
invent a width — and a watch of the wrong width silently misses writes to the bytes it failed to
cover, which is the same class of quiet wrongness that makes an ambiguous `break` an error. Refused
for the same reason, and a different milestone wearing the same syntax.

A related fact, confirmed by test rather than left as a source-read: `__DATA` symbols **are** in the
table and **are** reachable by name, because the filter keeps any defined symbol while `resolve`
clamps to `text_end`. Reaching them costs nothing — and is still not licence to ship `watch <name>`,
which the missing size blocks independently.

### Verifying the gate can fail, and one guard that earned its keep

Stubbing `Symbols::addrs_of` to return `Vec::new()` turns four of the five new e2e tests red. Two
details from that run matter more than the count:

- `a_stripped_guest_errors_cleanly_instead_of_guessing` went red **only because of its second half**.
  Its first half asserts that a missing name errors — which a resolver that resolves *nothing*
  satisfies perfectly. The `break _main` must-succeed check is the guard against that vacuity, and
  the stub run is what proved the guard load-bearing rather than decorative. A negative test needs a
  positive control or it is a green that measures nothing.
- `a_bad_name_fails_after_earlier_commands_have_run` stayed **green**, correctly. It pins the
  *ordering* change, not resolution; a debugger that resolves nothing still runs `where` before
  failing.

The headline itself avoids asserting that the code agrees with itself. `break _child; continue` is
checked on the **pc the guest stops at**, and the expected address comes from the *recording* rather
than from `addrs_of`: M1/M2 measured `crashthread`'s fault as `_child+0x30`, so the trace's terminal
`Event::Crash` says where `_child` begins without consulting the table under test. A no-op that
accepted the token and armed nothing would run to the fault; one that armed a wrong address would
stop at a pc whose distance from the crash is not `0x30`. Neither is excluded by asserting that the
command parsed.

The real-dyld ambiguity test discovers its duplicated name at runtime with `nm -arch arm64e` instead
of hardcoding one, both because which symbols dyld duplicates is a property of whatever OS shipped —
an update would turn a genuine pass into a spurious red — and because hardcoding without `-arch`
would have baked S4's own mistake into the gate, passing against names that are not ambiguous at all.

### What is still address-only

`watch`, `unwatch`, and `x`, on S5 above. Demangling remains separable — `break _ZN…E` works today by
exact match; raw mangled names beat hex and need no demangler, but they are not pretty. And the M19
shared-cache wall is untouched: `cache_symbol_e2e` stays parked, since a name M20 cannot print is a
name M20 cannot accept either.

### The M19 wall turned out to be documented with a mechanism that does not exist

Not planned work — it fell out of starting M21's measurements while M20's gate ran, and it is
recorded here because it changes what the *next* milestone should go looking for.

M19 parked `cache_symbol_e2e` and explained it this way: cache symbols are unreachable because "the
cache's local-symbol area lives in the on-disk cache file that `cache.rs` demand-pages but never
stages into guest memory." Measured on this machine, 2026-08-27:

- **`localSymbolsOffset` and `localSymbolsSize` are zero in all thirteen cache headers** — root,
  `.01` through `.12`. No `*.symbols*` artifact ships anywhere under the dyld directory. **There is
  no local-symbol area.** M19's suggested remedy — "stage the local-symbol area at record time" —
  named a thing that does not exist, and would have sent the next milestone hunting for it.
- Cached dylibs **do** carry `LC_SYMTAB`, `LC_DYSYMTAB` and `LC_DYLD_EXPORTS_TRIE`.
- Their `__LINKEDIT` lives in the `.dyldlinkedit` subcaches: **1.37 GiB of the cache's 5.40 GiB**,
  entirely inside the guest's 6.00 GiB shared-region window `[0x1_8000_0000, 0x3_0000_0000)` and
  **already routed** by `cache.rs`'s demand-pager, whose `assert_covers_window` requires
  `main -> .01 -> … -> .12.dyldlinkedit` to be contiguous.

So the bytes are neither missing nor unroutable. They are never **faulted** — nothing in the guest
reads a symbol table at runtime, so those pages are never staged into an anon page and never
captured by `snapshot()`. The exe and dyld resolve for the mirror reason: the guest's own loading
*does* touch their `__LINKEDIT`.

The wall is real and stays parked. But it is **narrower and more tractable** than its own text said,
and the measurement it owes is different: not "how do we get the bytes into the process" but "which
images does a real recording execute in", since staging `__LINKEDIT` for only those is bounded work
rather than 1.37 GiB. Both the README and the `#[ignore]` reason were corrected in place.

This is the third instance in two milestones of the same shape — M19's P3, M20's S4, and now this —
**a conclusion that was correct resting on a supporting fact nobody had measured.** All three were
caught by going to measure something adjacent, never by review and never by a green gate. That is an
argument for the measure-before-designing discipline that is stronger than any of the three
individually, because in every case the conclusion survived and only the reasoning was wrong — which
is precisely the error a passing test suite cannot see.

### Gate

**476 passed / 0 failed / 2 ignored across 106 test binaries**, clippy clean at `-D warnings`,
measured at `b8c2e33` over all 56 chunks, every one `EXIT=0`.

Reconciled against M19's 463/461/2 **file-by-file rather than by sum**, and each delta traces to
exactly one file: `symbols.rs` +7, `debug.rs` +3, the new `symbolops_e2e` target +5 = **+15**, giving
478 `#[test]` of which 476 run and 2 stay parked (`stackoverflow_rust_e2e` at M8 R3,
`cache_symbol_e2e` at the shared-cache wall above). Per chunk: A 111 → 118, B 219 → 219, `--bins`
8 → **11**.

That `--bins` number mattered twice. The plan predicted **no** CLAUDE.md edit was owed; that was
wrong. CLAUDE.md and the README both hardcode the count as the reason never to omit that chunk — the
one chunk whose omission is *silent* by design — so leaving it at 8 would have under-reported the
very thing the sentence exists to protect. Corrected in both.

One tallying error worth recording because it nearly entered the log: an early count of chunk A
read 337 instead of 118, because `cat *.log` swept in `retrace-box`'s half-written log. The
giveaway was that the excess was exactly 219, chunk B's total. The tally script now sums only chunks
recorded complete in `exitcodes.txt`. And "106 test binaries" is 99 test executables plus 7
`Doc-tests` harnesses that run zero tests each — the convention every milestone since M14 has used,
kept for comparability and now written down in the README rather than silently re-derived.

---

## Status: M22-fatheader — 🎉 retrace opens Apple's own binaries, and it was never a capability wall

**2026-08-29.** Branch `worktree-m22-fatheader`, cut from `main` at `ccfc8f9`.

### What happened

The milestone did not start as a milestone. It started as a question about whether the project was
useful yet, and the first probe — pointing `record-dyn` at a spread of real system binaries — failed
on every single one, identically:

```
thread 'main' panicked at crates/retrace-guest/src/lib.rs:10:5:
assertion `left == right` failed: not a 64-bit Mach-O (MH_MAGIC_64)
  left: 3199925962      # 0xBEBAFECA — FAT_MAGIC read little-endian
```

Twenty milestones of guest-ladder work had been built on self-compiled binaries plus Homebrew's thin
`jq`, and the natural reading of that history was that Apple's binaries were beyond retrace's
runtime. **They were not.** Every macOS system binary is a *universal* file whose first four bytes
are `0xcafebabe`, and `parse_macho` asserted `MH_MAGIC_64` against byte 0. retrace could always run
them. It could not **open** them.

`lipo -thin arm64e` and re-running settled it in one command: `/bin/echo` recorded, and replayed.
Nothing below the loader had to change — and the reason is structural, not lucky. An arm64e main
turns PAC on through M7's existing `pac_posture(cpusubtype)` path, and replay never reads the file at
all: `restore()` re-derives the posture from the snapshot's own mach header via
`pac_posture_from_memory`. The arm64e guests this unlocks therefore replay **by construction**.
`TRACE_MAGIC` did not move, and no existing recording was invalidated.

### What it unlocked, measured

Sampled `/bin` + every 8th of `/usr/bin`, pointing retrace straight at each file; PASS requires
record, replay, byte-identical stdout **and** equal exit codes. **34 of 54 pass — from a baseline of
exactly zero.** `cat`, `ls`, `cp`, `mv`, `rm`, `chmod`, `mkdir`, `ln`, `df`, `grep`, `wc`, `uname`,
`sh`, `dash`, `expr`, `bzip2` among them.

The distribution mattered more than the number. The 20 failures are **four** causes, not a tail:
13 × an uncategorised `EC=0x00` exit at `pc=0x4204` (modern ObjC/Swift `/usr/bin` tools, cause
**unmeasured**), 4 × an unrouted `mach_msg2` `msgh_id` 412, 2 × the M10 fd table's fail-loud
unmodelled `dup2` working exactly as designed, and 1 genuine divergence (`ps`) — the oracle catching
nondeterminism rather than reproducing something wrong in silence. `sysbin_e2e`'s second gate is
parked at the first group, naming the exception text and stating plainly that nothing has measured
why. Clearing it is plausibly the difference between 63% and ~87%.

### The lesson worth keeping

**A wall that every instance of a class hits identically deserves one probe before it is believed.**
The evidence for "retrace cannot run Apple binaries" was overwhelming and entirely circumstantial:
twenty milestones, a guest ladder built around the limitation, and a 100% failure rate across every
binary tried. None of it was evidence about the *cause*. One `lipo` invocation — thirty seconds —
would have distinguished a loader defect from a capability wall at any point in those twenty
milestones, and the reading that had accumulated was wrong in the direction that costs most: it made
a five-line omission look like a research problem.

The M19→M20 correction on the shared-cache symbol wall was the same shape, one milestone earlier:
the stated mechanism was false, and the wall turned out narrower than its own text. Two in a row is a
pattern, not a coincidence. **The failure mode is a right conclusion resting on an unmeasured
supporting fact** — a passing test suite cannot see it, because nothing is failing.

### Gate

**480 passed / 0 failed / 3 ignored across 107 test binaries**, clippy clean at `-D warnings` over
`--workspace --all-targets`, measured over all **58 chunks, every one `EXIT=0`**.

Reconciled against M20's 476 / 0 / 2 over 106 **file-by-file rather than by sum**, and every delta
traces to exactly one place: `retrace-guest` +3 (the fat-header tests) and the new `sysbin_e2e`
target +1 running / +1 ignored. Per chunk: A 118 → **121**, B 219 → **219**, `--bins` 11 → **11**.
Chunk B and `--bins` holding still is the load-bearing part of that reconciliation — it is what says
a change to the loader disturbed nothing below it. The third ignored gate is M22's own parked
`pc=0x4204` wall, joining `stackoverflow_rust_e2e` (M8 R3) and `cache_symbol_e2e` (M19).

Both green targets were additionally **verified able to fail**, by mutating the `slice_native` call
back out of `parse_macho`, observing the exact `MH_MAGIC_64` failure in each, and restoring.

**The gate was deferred, not skipped, and the deferral is worth recording.** A concurrent
M21-stackgrow session held the machine mid-`cargo test -p retrace-box`, and every VM test wants the
hardware to itself. Rather than run both and risk flaking the *other* milestone's result, the runner
polled for the M21 session's test processes to go quiet for a full minute and only then started —
45-minute backstop, so it could not hang forever. The cost was about half an hour of waiting; the
alternative was a number neither milestone could trust. An earlier draft of this section published
the expected delta as *"expected, not measured"*; the measurement then matched it exactly, which is
pleasant but is not what made publishing the hedge correct.

## Status: M23-xpcpipe — 🎉 Apple's binaries were behind one message, and `pc=0x4204` was our own trampoline

M22 left 20 of 54 Apple system binaries failing in four named causes and parked a gate at the
largest of them. M23 cleared the two biggest: **46 of 54 now record and replay** (stdout
byte-identical, exit codes equal, fall-through counts equal), up from 34.

Neither cause was what M22's text said it was.

**`pc=0x4204` was never a wall — it was retrace destroying its own evidence.** Each of the 16 EL1
vector slots is 0x80 bytes, of which only the first 4 held `hvc #0`. The remaining 0x7c were zero,
which decodes as `UDF #0`. When execution ran past a slot head it executed that `UDF` **at EL1**,
which overwrote `ELR_EL1`/`SPSR_EL1` with the trampoline's own address and re-vectored — destroying
the original exception's identity and reporting a pc inside retrace that had nothing to do with the
guest. Thirteen of M22's twenty failures were that one masking defect wearing thirteen faces. The
padding is now `hvc #1`: a fall-through is distinguishable at the VM exit, **counted**, and compared
across record and replay.

The honest part is what that did **not** explain. The masking is root-caused; the stale-PC resume
that lands on the padding in the first place is **not**. M23 removed the thing that hid it and
measured where it happens (~0.27% of vector entries), and nothing more. Calling that "the pc=0x4204
wall, cleared" would be the same overclaim the M19→M20 correction caught.

**The second cause was one unserviced message.** `host_get_special_port` (`msgh_id` 412) accounted
for 17 of the 20 once the loader defect below it was fixed. It is **forwarded and recorded**, not
synthesized, because the reply carries a host-minted port name that is nondeterministic by
construction — the `task_self` posture, a documented exception to symmetry rule 1 rather than drift
from it. The XPC message-queue send proper is refused deterministically, both sides recomputing an
identical refusal.

### The review this milestone should have had first

t1–t6 landed with **no code review of any kind**, and this milestone had no `.superpowers/sdd/`
directory at all — the only one since M6 without one. The review ran afterwards, three static
reviewers, one per seam. It found **no Critical defects**: symmetry rules 1 and 2 hold, the
divergence oracle gains no new hole (7 `verify_thread` sites plus `mirror_delivery`'s inline check =
8, unchanged, because M23 adds no early-returning mirror), and the un-parking is earned. Reviewing
after the fact still worked; it was luck that it did.

It also found three things worth the delay, landed as t6.5:

**A new abort path on four working syscalls.** `Route::Forward` read the request body for *every*
allowlisted id, but the body guard has something to check for exactly one of them (412), and
`read_guest` **panics** when a span does not fit inside a single backing. So t3 put a new way to kill
the recorder on four ids that had never touched guest memory before it (200 / 206 / 3418 / 3405).
Symmetry was not the problem — both arms read identically, exactly as rule 1 asks. Correctness under
rule 1 does not imply the thing being done identically is safe to do at all.

**A test that asserted the wrong half of its own claim.** `tests/trampoline.rs` discarded `run()`'s
`Stop`, so the suite asserted a fall-through was *counted* and never that the interrupted exception
was still *dispatched*. t1 claims both. Verified as a real hole rather than a theoretical one: with
the arm perturbed to count and then drop the exception,
`a_fall_through_onto_vector_padding_is_counted` still **passes** while the new
`a_fall_through_still_dispatches_the_exception_it_interrupted` **fails**.

**A guard that cannot do what it was asked to do, kept anyway for what it can.** The review proposed
asserting PC-in-padding and `SPSR_EL1 == EL0t` to catch a fall-through arriving after `ESR_EL1` was
already dispatched. Reading `set_x0_and_return` shows that cannot work: it clears neither `ESR_EL1`
nor `SPSR_EL1` nor `ELR_EL1`, so a duplicate presents byte-identical registers to a genuine first
fall-through and nothing measurable at that exit separates them. What landed is the narrower true
claim — `hvc #1` is written to exactly one place in guest memory, so a fall-through reported from
outside the vector table fails loud — with the duplicate-dispatch hole stated plainly in the code
and in the README rather than implied closed. **That hole is the one a determinism oracle
structurally cannot see:** the duplicate re-dispatches the same `(num, args)`, record and replay
agree, and the trace is self-consistently wrong. It is the M18 `semaphore_wait_trap` argument again,
and the third milestone in a row where the interesting finding is a *right conclusion resting on an
unmeasured supporting fact*.

### Left standing, deliberately

- **`TRACE_MAGIC` was not bumped, and should probably have been.** t1 changed the vector padding,
  which lives in the trampoline page and is therefore snapshot **content**. A pre-M23 recording still
  opens, and `restore` faithfully restores its old zero padding while the current code assumes
  trapping padding — so a fall-through on that replay reproduces the exact misattribution M23
  removed. The written rule covers changing `Event`'s *shape*; this changed what a snapshot's bytes
  *mean*, which the rule does not name.
- **`Box_::restore` does not rebuild the vector table.** `build_vector_table` is called only from
  `Box_::load` and `load_dynamic`, so the padding reaches replay purely because the trampoline page
  happens to be a snapshot backing — right by luck, not construction. The concurrent M21-stackgrow
  review found the same shape where the luck did **not** hold: a reservation built in `load_dynamic`
  that `restore` reset to empty, making that milestone record-only. Two milestones, one root pattern
  — **state established on a record-only path with replay left to reconstruct it** — which argues for
  auditing everything `load_dynamic` establishes that `restore` does not, rather than fixing two
  instances.
- **The new `brk` wall has no parked gate.** Four binaries (`automationmodetool`, `desdp`,
  `dyld_info`, `flex`) reach it and the cause is unmeasured. M22 parked a gate for its wall; M23 did
  not park one for the wall it found. By this repo's own discipline that is a gap, not a decision.
- The trampoline page is padded for only 0x800 of its 16 KiB; the rest is the same `UDF #0` M23
  removed from the slots. Nothing reaches it, and a test pins the boundary.

### A documentation defect this close inherited

The README's Known-limits bullet said "**Two gates are parked**" for the whole M22→M23 window while
its own Gate paragraph said three — M22 parked `sysbin_e2e`'s second gate and never updated the
bullet. M23's un-parking makes "two" true again by accident. It is recorded here rather than
silently corrected, because a current-state document that contradicted itself for a milestone is
precisely what the two-document split exists to prevent.

### Gate

**497 passed / 0 failed / 2 ignored across 109 test binaries**, clippy clean at `-D warnings` over
`--workspace --all-targets`, measured over all **59 test chunks, every one `EXIT=0`**.

Reconciled against M22's 480 / 0 / 3 over 107 **file-by-file rather than by sum**. Per chunk:
A 121 → **129**, B 219 → **225**, `--bins` **11 → 11**. Every delta traces to exactly one place —
all eight of A's to `machmsg.rs`, all six of B's to `trampoline.rs` (which already existed with one
test, so the box crate gains no new *suite*, which is why its suite count holds at 29 while the
concurrent M21 branch shows 30). The remaining +3 is one test each from the new `xpc_e2e` and
`fallthrough_e2e` targets plus `sysbin_e2e`'s second test moving from ignored to running. Total
**+17 running, −1 ignored, +2 binaries**.

`--bins` holding at 11 is the load-bearing part of that reconciliation: a change to the trampoline
and to the `mach_msg2` router disturbed nothing in the CLI below it. The ignored count going *down*
is the milestone's headline in one number — the first time since M2-taskinfo that a close removes a
parked gate without adding one, and the honest asterisk is that M23 found a new wall (the `brk`
group) and parked nothing for it.

Two of the three t6.5 fixes were verified able to fail, and the third was verified *unable* to do
what it was asked. `a_fall_through_still_dispatches_the_exception_it_interrupted` was falsified by
perturbing the arm to count-then-drop the exception, which leaves the pre-existing
`a_fall_through_onto_vector_padding_is_counted` **passing** — the cleanest demonstration in this
milestone that the old assertion was blind. `a_fall_through_from_outside_the_vector_table_fails_loud`
pokes an `hvc #1` one word past the table and pins the inclusive upper bound. F1's proposed
duplicate-dispatch guard was not landed, because reading `set_x0_and_return` shows it cannot work.

## Status: M21-stackgrow — 🎉 M8 risk R3 falls after thirteen milestones, and the gate stays parked

M8 measured that macOS 26's libpthread reports a **constant** `0x7fc000` main-thread stack size that
retrace cannot influence — answering `getrlimit(RLIMIT_STACK)` with `0x10000000` instead of `0x40000`
left libstd's computed guard address bit-identical. With retrace backing 256 KiB, libstd installed
its stack-overflow guard at `0x2004000`, **7.72 MiB below** where the real backing ended. A deep
recursion never reached it: it ran off the backing into unbacked IPA and killed the recorder with a
stage-2 translation fault. M8 rejected both obvious fixes with measurements — eager 8 MiB backing
cost ~1.7× on `hello_rust` and worse across the dyld suite; `getrlimit` cannot move the subtrahend —
and parked `stackoverflow_rust_e2e` there. It stayed parked from M8 through M20.

M21 stops trying to out-synthesize the constant and moves the other operand: **reserve the stack the
guest believes it has.** `[0x2008000, 0x27C0000)` is reserved but unbacked, and `commit_reserved_page`
— which already existed for `PROT_NONE` reservations since M2-mmapcommit — grows into it one zeroed
page per stage-2 fault. Nothing is eagerly backed, so M8's 1.7× is not paid.

The measurement that says it worked is the fault's *class*, not its presence:

    [fault] pc=0x100000a70 esr=0x9200004f far=0x2007f30 ec=0x24
    [fault] pc=0x1804fb710 esr=0x9200004f far=0x2007a90 ec=0x24

`far` 0x2007f30 and 0x2007a90 are inside the guard page `[0x2004000, 0x2008000)` — 208 and 1392 bytes
below `GUARD_TOP` — and `esr 0x9200004f` decodes to **DFSC 0x0f, a permission fault**. The
before-picture was `far/ipa=0x27bff60 (UNMAPPED)`, **FSC 0x7, a translation fault**, 7.72 MiB away at
the stack bottom. Permission versus translation is the whole argument: permission means the page is
*there* and the guest may not touch it, which is what a guard page is. That is the same distinction
`protnone_rust_e2e` was built to assert, reused as the oracle here.

The guard page is deliberately left **outside** the reservation, one granule below its start. Inside
it, a stack overflow would take the stage-2 route and be silently committed — converting an overflow
into a corrupted guest that keeps running, the one failure mode this design must never reach.

### The gate does not come green, and that is the discipline working

Behind M8's wall stands a different one. libstd **has** a handler installed for the signal the guard
fault maps to — signal 10, SIGBUS — so the disposition check passes; but the faulting thread has that
signal **blocked**, and `retrace-core/src/lib.rs:203` asserts rather than guessing:

> raising blocked signal 10 synchronously is not modelled: a fault cannot be deferred, POSIX leaves
> it undefined, and Darwin force-delivers. M11 models no pending set, so implement one — and revisit
> sigpending's always-empty answer — before a guest needs this.

M11 wrote that assert naming the measurement it owed. A guest now needs it. Clearing it means giving
M11 a pending set for synchronously-raised blocked signals, which is a signal-model milestone and not
a stack one, so the gate is **re-parked there** with that text rather than un-parked or faked green.

**The progress is gated anyway, and that gap was worth closing.** A parked headline gate means
nothing end-to-end would notice if the reservation stopped working — `stackgrow.rs` and
`restorereserve.rs` prove the reservation *exists* on both sides, and neither proves a real deep
recursion *uses* it. `a_rust_stack_overflow_now_reaches_its_guard_page_and_a_different_wall` runs, and
asserts on the difference rather than on an outcome a weaker failure would also produce. It was
verified able to fail by regressing M21 itself: with `reserve_believed_stack()` removed from
`load_dynamic` it fails with `far/ipa=0x27bff60 (UNMAPPED) pc=0x100000a70` — byte-identical to T0-4's
recorded before-picture.

### The defect every task before it was blind to

Task 2's code review found M21 was **record-only**, and t0–t2 could not have caught it: every one of
them tested the record side. `load_dynamic` is called only from `record_dynamic`; replay builds its
box through `Box_::restore`, which reset `reservations` to empty; and `commit_reserved_page` services
a growth fault only inside a reservation. So the first stack growth on replay was unserviced and came
back as a divergence. **The headline gate's two-replay requirement could never have passed**, for a
reason unrelated to the wall it named.

`restore`'s reset is *correct* for the guest's own reservations — replay rebuilds those by
re-executing its `mach_vm_reserve` landmarks through mirrored dispatch arms. M21's is the one entry
with no landmark to rebuild from, precisely because M21 keeps it below the trace. The asymmetry was
exactly one entry wide. The reset's own comment says reservations are cleared "so replay's
demand-commit address sequence matches record's" — and the operative clause is *matches record's*:
empty was right only while record's list was empty at snapshot time, and M21 made record start with
one entry without moving the other side.

Two documents asserted the opposite and are **corrected rather than reworded**: the doc comment on
`reserve_believed_stack` claimed "`load_dynamic` runs identically on record and replay, so the same
reservation exists on both sides" (replay never calls `load_dynamic` at all), and the design spec's
line 96 claimed the replay arms "service the growth without modification". Both now say what is true
and say that they were wrong.

**This is the second instance of one pattern, found the same week.** M23's review turned up the same
shape where the luck held: `build_vector_table` is called from `Box_::load` and `load_dynamic` but
not `restore`, so its trapping vector padding reaches replay only because the trampoline page happens
to be a snapshot backing. Two milestones, one root cause — **state established on a record-only path
with replay left to reconstruct it** — which argues for auditing everything `load_dynamic` establishes
that `restore` does not, rather than fixing instances as they surface.

### Two smaller corrections worth keeping

Task 2's review also found that the assert naming M21's central invariant was a **tautology**:
`GUARD_TOP > GUARD_PAGE_IPA` cannot fail given `GUARD_TOP = GUARD_PAGE_IPA + GRANULE` unless GRANULE
is 0. It was replaced with alignment checks, which the derivation does not already give — though the
tautology moved one link down rather than vanishing, and after the fix **no build-time assert covers
the central invariant at all**; it lives in a unit test.

And the task brief's own falsification recipe was wrong about its own test. Perturbing `GUARD_TOP`
fails on `assert_eq!(GUARD_TOP, 0x2008000)` — the absolute anchor — not on the "EXACTLY one granule"
assertion the brief and Ruling R2 both name. The test is non-vacuous; the brief's account of *why*
was not. Third milestone running where the notable finding is a right conclusion resting on an
unmeasured supporting fact.

### Gate

**504 passed / 0 failed / 2 ignored across 111 test binaries**, clippy clean at `-D warnings` over
`--workspace --all-targets`, measured over all **59 test chunks, every one `EXIT=0`**.

Reconciled against M23's 497 / 0 / 2 over 109 **file-by-file rather than by sum**. Per chunk:
A **129 → 129**, B 225 → **231**, `--bins` **11 → 11**. All six of B's are itemised — `stackgrow.rs`
1, three new `stack_geometry_tests`, `restorereserve.rs` 2 — and the remaining +1 is
`stackoverflow_rust_e2e`'s new running gate. Total **+7 running, ±0 ignored, +2 binaries**.

Chunk A holding exactly still is the load-bearing part here: M21 touches no crate in it, so any
movement would have meant something unintended. The ignored count holding at 2 is the honest number —
M21 cleared a wall and re-parked the same gate one wall further on, which is neither progress to
claim nor a regression to hide.

**The plan's own baseline was stale and would have produced a false reconciliation.** Task 4 says to
reconcile against "M20 closed at 478 `#[test]` — 476 run, 2 parked", written before M22 and M23
landed. Following it literally would have manufactured a ~21-test discrepancy out of two intervening
milestones. `main` was merged into the branch before the gate ran, and the reconciliation above is
against M23's actual close.

## Status: M24-restoreaudit — the eighth instance of a seven-time bug gets a mechanism instead of a fix

`Box_` has three construction paths, and only one of them runs on the record side:

| Path | Runs on | Builds from |
|---|---|---|
| `load` / `load_dynamic` | **record only** | the Mach-O + dyld |
| `restore` | **replay only** | a landmark-0 `Event::Snapshot` |
| `from_checkpoint` | **replay only** (M4 seeks) | a mid-run `BoxState` |

Anything a load path establishes that a replay path does not re-establish is a defect whose signature
is **a passing record followed by a diverging replay**. Every record-side test is blind to it by
construction. So, worse, is the determinism oracle — whenever *both* replay paths are wrong in the
same way, because the oracle compares replay against record's **trace**, never against record's
**box**.

The class is neither hypothetical nor new. By this repo's own written record it has shipped **seven**
times: M9 t3 (`from_checkpoint` reset a flag the restored state contradicted), M10 (fd slots not
carried, so a seeked session believed every fd Free and a post-seek `pread` returned `EBADF`), M11
(`sigtable` not carried, so a seek into a run that installed a disposition restored a box that had
forgotten it — an *ignored* signal would terminate the guest), M14 (`thread_start_pc`), M18
(`wq_thread_pc`), M21 (the believed-stack reservation made in `load_dynamic` only, which made M21
**record-only** until its own t2.5), and M23 t1 (the EL1 vector table, left open as finding **F5**).
The `BoxState` field comments are themselves a log of it — one of them reads *"the fifth field in
this struct to exist for that reason."*

Seven instances across fifteen milestones, each fixed individually, **none of them leaving behind a
mechanism that would catch the eighth.** That absence is what M24 exists to fix. Fixing the instances
turned out to be the smaller half.

### The four asymmetries the audit found

**G1 — `TPIDRRO_EL0` set unconditionally.** `restore` set it to `TSD_IPA` under a comment claiming to
"match load". True of `load_dynamic`; false of the static load, which never sets it and does not map
`TSD_IPA` at all — so a static guest's deref would have faulted on **replay only**. Corroboration
worth recording: `from_checkpoint` already did this correctly, taking the value per-thread from the
captured table with a comment explaining that a constant here is wrong. G1 brings `restore` into line
with a sibling path that had been right since M14 — independent evidence that the constant was a
genuine defect and not a harmless simplification.

**L1 — the vector table, M23's F5.** `restore` never called `build_vector_table`; the trapping padding
reached replay only because the trampoline page happens to be a snapshot backing. Correct by luck,
pinned by nothing. `restore` now asserts the snapshot carries the table this build makes.

**L2 — thread 0's saved context.** `load_dynamic` folds real startup state into it; `restore` left it
`ThreadCtx::zeroed()`. The fix is gated on the dynamic path, and **the gate is load-bearing**: seeding
it unconditionally traded the asymmetry for its mirror image, since the static load does not populate
thread 0 either. The parity test caught that over-correction immediately, which is the argument for
the test in one line.

**G2 — stranded signals on replay.** `ReplaySession::advance`'s terminal-exit arm now calls
`assert_no_stranded_signals()`, mirroring the guard record already had. Replay can strand a signal
record did not: a seek can land *past* the `__ulock_wake` a pended signal was waiting to materialise
at. A vanished signal is the one class the oracle structurally cannot see, because both sides agree —
so it has to be caught by a guard, and the guard has to exist on both sides.

### The part meant to outlive the instances

`crates/retrace-box/tests/restoreparity.rs` diffs a load box against a `restore` box built from that
same box's own snapshot, field by field, and states an obligation for future work: a new `Box_` field
or load-time write must be **either** covered there and equal, **or** named in `normalise()` with the
mirrored replay mechanism that re-establishes it, cited by file and line. There is no third option
that is safe. `normalise()` holds exactly one entry today — the shared-cache pager, which
`load_dynamic` installs eagerly and replay installs through the mirrored `#294`/`#536` dispatch arms.
That is a real mirror, not an excuse, and the entry names it.

t2 deepened the guard to 15 of `Box_`'s 27 state fields, plus two sysregs and the 0x800 vector table:
it added `backings` (count and the `(ipa, len)` set — load builds them from the Mach-O, `restore` from
`mem`, expected equal and nothing checked it), `next_l3` (derived from `backings` on both paths by
*different code*, which is exactly the shape that drifts), and the full thread-0 `ThreadCtx` plus the
thread count where only `ctx_of(0).regs.pc` had been compared.

**And it declined eleven, which is as much the point as the three.** `noaccess`, `bps_armed`,
`wps_armed`, `watch_ranges`, `syscall_watch_hit`, `tlbi_stub_ready`, `fds`, `sigtable`,
`thread_start_pc`, `wq_thread_pc` and `pthread_size` are all default on both sides at landmark 0.
Asserting `Default == Default` there is a test that passes for a reason unrelated to its name; adding
all eleven would have made the guard look twice as thorough while making it no more capable of
catching anything. The refusal is written into the test file with this reasoning, so the next reader
does not mistake it for an oversight — and the obligation text already requires them to be added the
moment a load path starts setting one before the first landmark.

### F4 closed at the layer it belongs to

M23 changed snapshot *content* — the trampoline's vector padding, `UDF #0` → `hvc #1` — without
bumping `TRACE_MAGIC`, and was honest about it as finding **F4**. L1's assert had already converted
that from a silent wrong replay into a loud refusal, which is strictly better and still the wrong
layer: a format break belongs at `open_checked`, not in an assert deep inside box construction. t3
moved `TRACE_MAGIC` `RT\x00\x08` → `RT\x00\x09`, so a pre-M23 recording is now refused before a
single byte of it is trusted.

**L1 stays anyway, and is not made redundant.** The magic guards the *file*; L1 guards the *box
construction*; they fail at different layers for different callers. A future change to
`build_vector_table()` that does not touch the format is caught only by L1.

The written rule is what actually changed. It said *changing `Event`'s shape is a format break*. This
was a change to what a snapshot's bytes **mean**, which a shape rule cannot see. Both are format
breaks now, in the README and in CLAUDE.md.

t3 also found that the new previous-magic rejection test wrote the **current** magic instead of the
previous one it names, so it was passing through the torn-tail path rather than the magic check.
Fixed to write `RT\x00\x08`, and the magic-specific rejection proven separately: the old magic alone
is rejected, the current magic alone is not.

### The gate

**509 passed / 0 failed / 2 ignored across 112 test binaries**, every chunk `EXIT=0`; clippy clean
over `--workspace --all-targets` with `-D warnings`. "112 test binaries" is 105 test executables plus
the 7 `Doc-tests` harnesses, the convention every milestone since M14 has counted by.

Reconciled against M21's 504 / 0 / 2 over 111 **file-by-file rather than by sum**, and the diff came
back exactly one file wide: `restoreparity.rs` **0 → 5**, every other file byte-identical in its
`#[test]` count. Source totals 506 → 511, which is 509 running plus the 2 parked. The ignored count
is unchanged and both parked gates are the same two — `stackoverflow_rust_e2e` at M21's signal-model
wall and `cache_symbol_e2e` at M19's shared-cache symbol wall. **M24 parks no new gate and un-parks
none**, which is correct for a milestone that buys a guarantee rather than a capability.

One gap was found *by* running the gate, and is recorded rather than smoothed over. Chunk B had to be
split per-target for CPU reasons, and `cargo test -p <crate> --test <name>` selects integration
targets **only** — so `retrace-box`'s `Doc-tests` harness ran in no chunk at all. It executes zero
tests, so nothing went red; it silently cost one of the 112. This is the exact sibling of the `--bins`
trap CLAUDE.md has documented since M17, and it was caught only because the reconciliation is
file-by-file rather than a sum. It was then run on its own (`--doc`, `EXIT=0`) so the 112 is measured
and not asserted. Both documents now name the second mouth of that trap.

### Residual, stated rather than left to be rediscovered

1. **`from_checkpoint` has no parity guard at all — this is the successor milestone.** It is the path
   with the *documented five-instance history* of this exact class (M9 t3, M10, M11, M14, M18), it
   restores far more state than `restore` does, and it runs mid-run where nothing is at a default.
   M24 closes the class on the path it has bitten **twice** and leaves it open on the path it has
   bitten **five times**. It is out of scope deliberately: it needs a different fixture — a box driven
   to a mid-run landmark, checkpointed, restored and diffed — and a judgement about what *should*
   legitimately differ at a mid-run landmark. That is a milestone's work, not a task's, and a shallow
   version of it inside M24 would be the same kind of near-miss the class is made of.
2. **Symmetric-but-wrong stays invisible.** A static box's thread-0 context is zeroed on *both* sides;
   a consumer reading it without refreshing gets zeros identically on record and replay. Wrong in the
   same way twice is the oracle's blind spot by construction, and no parity test between two boxes can
   see it either.
3. **Landmark 0 only.** The guard compares construction, not evolution. Two boxes that agree at
   landmark 0 and drift later are outside what this pins.

M23's section stands as written, F4 and F5 included; this section is their forward pointer. Both are
now closed — F5 by L1, F4 by the magic bump — and neither of M23's entries is edited to say so, which
is what the append-only rule is for.

### A process note

t1 landed **before** the spec and plan existed, which is a deviation from the SDD flow CLAUDE.md
describes. It is recorded in the spec under "Why this spec is retroactive" rather than back-dated,
because the audit found its first four asymmetries by following M21's and M23's scent rather than by
systematic enumeration — and an audit milestone that does not publish its negative space is
indistinguishable from four ad-hoc fixes wearing a milestone's name. The "Coverage" section of the
spec exists to be that negative space, and the README's Known-limits entry deliberately does **not**
say the class is closed.

---

## Status: M25-cpython — the headline target records on the first probe, and parks on the second replay

The 2026-07-05 vision spec names reverse-debugging a real CPython interpreter as the headline. Twenty-four
milestones later, nothing in the tree had ever pointed `record-dyn` at `python3` — no spec, no plan, no
test, no entry in this log. The belief carrying that absence was that an interpreter was far away, and
**nobody had checked**. M22's lesson applies verbatim: a wall that every instance of a class is assumed to
hit deserves one probe before it is believed. M25 is that probe, and it found the same shape M22 did —
the distance was mostly imagined.

**Finding 0: the thing on your `PATH` is not the interpreter.** `/opt/homebrew/bin/python3` resolves to a
`pythonw.c`-style shim that `posix_spawn`s (syscall 244, `POSIX_SPAWN_SETEXEC`) the real binary at
`Python.app/Contents/MacOS/Python` in its own place. retrace forwards 244 through the generic arm, the
call returns an error rather than replacing the image, and the shim takes its own `err(1, …)` path. That
run **records and replays byte-identically** — the oracle has nothing to disagree about, because retrace
reproduced the guest's own behaviour faithfully. It is retrace working, not a bug; but a probe that had
stopped there would have concluded "CPython does not run" while never having executed a line of CPython.
The two guest paths are pinned as constants in `cpython_e2e.rs`, in their version-stable framework form
rather than the `Cellar/python@3.14/3.14.6/…` form a `brew upgrade` moves.

**Wall 1 was one bit.** The real interpreter died at `non-syscall exit: MSR/MRS/sysreg trap (EC=0x18
ISS=0x12dc68)`. The ISS decodes to `dc zva, x3` — Apple's `_platform_memset` issues `DC ZVA` above a size
threshold, and CPython's allocator reaches that threshold during startup. It trapped because
`SCTLR_MMU_ON_BASE` left **DZE (bit 14)** clear and `run()`'s only `Ec::SysReg` arm handles the timebase.
The fix is `| 0x4000` on the one constant all four `set_sys(SCTLR_EL1, …)` sites derive from — **symmetry
rule 2**, below the trace: `DC ZVA` now executes natively inside `Box_::run()` on both sides, nothing is
recorded, and `TRACE_MAGIC` did not move. `sctlr_enables_dc_zva_for_el0_and_nothing_else` pins all three
bits as one decision, because **UCT (15) and UCI (26) stay deliberately clear**: nothing has measured a
guest issuing `DC CVAU` / `IC IVAU` or reading `CTR_EL0` from EL0, and the existing EC 0x18 exit already
fails loud if one does. Setting them speculatively would have been exactly the "right conclusion resting
on an unmeasured supporting fact" M19, M20 and M22 each caught in themselves.

**Wall 2 was two table entries, and it failed silently.** `os.listdir` on the stdlib directory called
`getdirentries64` (344), which is absent from `retrace_arch::fd_operands`, so guest fd 4 reached the host
kernel as *retrace's* fd 4 — not a directory — and XNU answered `EINVAL`. `fstatfs64` (346) had the same
gap. Both are now in the `&[0]` arm. The census step resolved three more numbers against this machine's
SDK and changed nothing: 228 (`fgetattrlist`) and 406 (`fcntl_nocancel`) were already present, and 427
(`fsgetpath`) takes an `fsid_t*` naming a volume rather than a descriptor, so its absence is **correct** —
pinned as an assertion so it is not re-opened later. `getdirentries64` is not in the SDK at all (libc
calls it privately from `opendir`/`readdir`), so its fd-in-`x0` position rests on captured trap arguments
and its constant is documented as **measured** rather than header-derived, beside siblings that are.

**What the chain reached.** With both fixes in, the real CPython interpreter running `-c 'print(1)'`
**records to a clean `exit(0)` having written exactly `1\n`** — `RETRACE_TRACE=1` shows `SYS_write(1,
"1\n")` as the last real trap before exit, reproduced twice. Every record-side wall on that path is gone.

**Wall 3 is on the replay side, and it is where the milestone parks.** Of the two replays the gate
demands, the first diverges:

```
DIVERGENCE at landmark 568 pc=0x1804b1834: syscall mismatch:
  live     (num=4,  args=[2, 30086578176, 106, 1, 0, 42963282272, 10, 200])
  recorded (num=75, args=[30086955008, 98304, 7, 0, 0, 42972720880, 42972417888, …])
```

`num=4` is `write` with `args[0]=2` (stderr); `num=75` is `mmap`. Live re-execution is issuing a
*different syscall* from the one the recording holds at that landmark, so the two runs' **sequences** had
already parted ways before the oracle's first complaint — this is not one call's arguments drifting. No
unit test at any single layer reproduces it, and closing it means tracing which earlier syscall's count or
ordering differs between a record and its own replay. That is modelling unmeasured guest/kernel behaviour,
which Task 4's stop criterion 4 rules out for a single pass, so `the_real_cpython_interpreter_records_and_replays`
was **re-`#[ignore]`d with that divergence verbatim in its reason** rather than loosened, deleted, or
asserted around. Rung 7 is deliberately **not** added to the README's ladder: the ladder's entry condition
is "records *and replays* byte-identically, twice", and this meets half of it. A milestone that parks a
new gate for a capability it does not have has regressed nothing.

**The gate: 512 passed / 0 failed / 3 ignored across 113 test binaries**, every chunk `EXIT=0`, clippy
clean over `--workspace --all-targets`. Reconciled against M24's 509 / 0 / 2 over 112 **file-by-file**:
`retrace-arch/src/lib.rs` 22 → 23, `retrace-box/src/lib.rs` 12 → 13, the new `retrace/tests/cpython_e2e.rs`
0 → 2 (one running, one ignored), every other file unchanged, and `--bins` **11 → 11**. The count closes
at both ends rather than only summing — 511 `#[test]` in the tree at M24 = 509 + 2, and 515 at M25 =
512 + 3 — so nothing is unaccounted for in either direction. M24's `Doc-tests` discovery was **acted on
rather than rediscovered**: chunk B ran `cargo test -p retrace-box` as a whole package and `Doc-tests
retrace_box` duly appears in its log. The `retrace` package still exceeded the ceiling, killed with 35 of
58 targets done; the remaining 23 were swept in two further chunks, so every target ran in exactly one
chunk and the union is the package.

**M24 landed first, so M25 reconciled the README** — the Coordination clause working as written. Merging
`main` was clean and all four `sctlr_mmu_on` install sites survived M24's rewrite of `restore` (M24 in
fact *added* a guard asserting that none of them builds SCTLR ad hoc, which now protects Fix 1 too).
Reconciling also surfaced a stale figure M23 had left: "What works today" still claimed **34 of 54** Apple
binaries while "Known limits" already said **46**. The README now says 46 in both places. That is the
hazard of a two-section current-state document, and it was caught by a milestone editing it second rather
than by anything structural.

**What is left standing, named rather than implied:**

- **`fd_operands`' default is still `_ => &[]`** — silent, not fail-loud. The next missing fd-taking
  syscall fails exactly the way 344 did, and unlike the fd table's `dup2` path it will not announce
  itself. Making it loud needs a blast-radius measurement nobody has taken.
- **Exec-in-place is unmodelled.** `POSIX_SPAWN_SETEXEC` returns an error instead of replacing the image,
  so shim-style launchers (`python3`, and `/usr/bin/git`'s relatives) run their failure path. The launcher
  gate holds that visible and must be **rewritten, not defended**, when exec-in-place lands.
- **`UCT` and `UCI` are unmeasured, not decided.** A guest with a JIT calling `sys_icache_invalidate`, or
  reading `CTR_EL0`, will hit the same EC 0x18 exit `DC ZVA` did. It will fail loud.
- **Reverse execution over a CPython trace is entirely ungated.** M25 measured record and one replay. No
  seek, checkpoint, watchpoint or `reverse-continue` has ever been pointed at a trace this size, and the
  M4 checkpoint cache's behaviour at CPython's landmark counts is unknown.

The successor is **M26-cpythonreplay**: find the earlier syscall whose count or ordering differs between
record and its own replay, and close it. Everything before landmark 568 is known-good on the record side,
which is a much narrower search than M25 started with.
