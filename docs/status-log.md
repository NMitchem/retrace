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

---

## Status: M26-cpythonreplay — 🎉 rung 7, and the wall was ours, not CPython's

M25 parked rung 7 at a replay divergence and named the successor. The successor took one afternoon,
because the wall was not in CPython, not in the replay path, and not new: it was a 64 KiB constant
written in M1 and a clamp written in M2 that were never asked to agree.

**M25's own account of the wall was wrong in three ways, and each one mattered.** The `#[ignore]`
reason said the two runs' syscall *sequences* had parted ways, that `num=75` was `mmap`, and that the
divergence was at landmark 568.

1. `num=75` is **`madvise`** (`sys/syscall.h:115`; `mmap` is 197). The recorded call was
   `madvise(…, MADV_FREE_REUSABLE)` — routine libmalloc housekeeping. Read as `mmap` the two sides
   look like unrelated code paths; read correctly they are *normal path* versus *error path*, which
   is what pointed at the answer.
2. The sequences had **not** parted ways. Landmarks 0..559 matched exactly — they had to, or the
   oracle would have complained sooner.
3. The landmark index was never a stable fact. M25 pinned 568; this machine diverges at 560, same
   `pc`, same live args. Pinning it as evidence pinned noise beside signal.

**The measurement that closed it took one probe.** The live side was about to `write(2, …, 106)`. Read
those 106 bytes out of guest memory:

```
Error in sitecustomize; set PYTHONVERBOSE for traceback:
ValueError: bad marshal data (unknown type code)
```

A **data** divergence. The guest was unmarshalling a `.pyc` and hit a byte that is not a valid type
code — reacting correctly to bytes replay had failed to restore. Dumping the recorded events before
the divergence found it at landmark 556: `read(fd=4, buf=0x701528020, count=88104)` returned
**88103** bytes and recorded **65536**. 22567 bytes read and captured nowhere.

**Root cause.** `forward_and_diff` snapshots a pre-image window of each pointer argument, forwards,
then diffs that same window. The window was a flat `PTR_WINDOW_CAP` (64 KiB). The "Debt #1" clamp
immediately below bounds the forwarded read *count* by the destination's backing, **not** by the
window. Two bounds that must agree, written twice — so the kernel may legitimately write past what
the diff ever inspects.

**This was booked 25 milestones ago and half-paid.** M1's plan
(`2026-07-05-retrace-m1.md:874`) flagged the window policy as needing revisiting "once real programs
(large mappings, failing syscalls) are recorded". M2 then deliberately declined to touch it
(`2026-07-06-retrace-m2.md:462`) — and was *right* in the direction it considered: `x2` is only a
count for the read family, so clamping the snapshot by it would under-snapshot `fstat`'s buffer and
regress M1. Nobody asked the inverse — whether the window should widen **up** to `x2` where `x2`
genuinely is a count. M1's design spec even called this its "main engine risk", asserting it was
"caught loudly, never silently". That mitigation was half-right, and the half that failed is
explained below.

**Nothing previously green was corrupted, and this is structural rather than lucky.** No gate in the
repo's history ever ran a guest capable of a >64 KiB read through `forward_and_diff`. Every bulk file
read bypasses it: file-backed `mmap` goes through `guest_mmap_file`, which records its full extent;
the shared-cache pager reads fixed 16 KiB pages; retrace's own Mach-O loading is host-side
`std::fs::read`. The largest count that ever reached `forward_and_diff` was dyld's `pread` of 0x4000.
`jq_file_e2e`'s fixture is 28 bytes. The defect was unreachable by construction until CPython.

**The failure is *latently* silent, and M26's own test misdescribed this before an audit caught it.**
The per-landmark oracle genuinely cannot see it — `(num, args)` match, so the recording is
self-consistent and merely incomplete. But `Box_::diff_memory` compares every recorded region at
exit, and all three terminal replay arms fail on mismatch. So stale bytes surface **unless** the
guest acts on them first or drops their backing. That is why the two known instances failed in
different places:

| guest | what it did with the bad bytes | where it failed |
|---|---|---|
| CPython (M25) | branched on them | syscall landmark ~560 |
| `bigread` (M26) | ignored them | terminal memory compare, at `buf+0x10000` |

The genuinely silent escape hatch is a read into a mapping that is then `munmap`'d, since
`guest_munmap` removes the backing. No gate does that today, which is recorded here rather than left
unstated. The correction to `bigread_e2e`'s comment is its own commit (`4bcfc6c`) because the
milestone's own test claiming the wrong failure mode is exactly the kind of thing that ossifies.

**The fix is not a bigger constant** — 300 KiB would break identically. The clamp and the window now
consult one predicate, `writes_x2_bytes_to_x1`, so they cannot drift apart again; `diff_window`
widens only that argument for those syscalls and never exceeds `avail`. Everything else keeps the
heuristic, because widening unconditionally costs a pre-image copy on every pointer operand of every
syscall and M8 measured that per-syscall diff time is not free.

**🎉 Rung 7.** `the_real_cpython_interpreter_records_and_replays` is un-`#[ignore]`d. The real CPython
interpreter running `-c 'print(1)'` records and replays byte-identically, **twice**, exit 0, stdout
exactly `1\n`. The assertion never moved — it demands what it demanded while parked. The 2026-07-05
vision spec's headline target runs.

**`/bin/ps` was this bug, filed as something else.** The README said since M22 that `ps` is "the
oracle catching nondeterminism". It cannot be: replay never *executes* a syscall, so a process list
cannot vary between runs — and the M22 measurement document itself said "also not diagnosed" while
the README stated it with confidence. Measured with a prototype tripwire: it fires once, on
`num=202` (`sysctl`), and replay then diverges at `ipa 0x700810091`, **145 bytes past that window's
end**, replay holding zeros where the recording holds data. The README is corrected. M26's fix does
**not** cover it, because `sysctl`'s length lives at `*(size_t*)x3` rather than in a register.

**The gate: 515 passed / 0 failed / 2 ignored across 114 test binaries**, every chunk `EXIT=0`,
clippy clean. Reconciled against M25's 512 / 0 / 3 over 113 file-by-file: `memdiff.rs` 1 → 2,
the new `bigread_e2e.rs` 0 → 1, `--bins` 11 → 11, everything else unchanged. **+3 running from only
+2 new tests** — the third is the CPython gate leaving the ignored column. 515 `#[test]` at M25 =
512 + 3; 517 at M26 = 515 + 2. The ignored gates are back to two.

`bigread` is a new repo-owned guest rather than a reliance on `cpython_e2e`, and deliberately so:
that gate *skips* when Homebrew Python is absent, and a gate that can silently not-run cannot guard
anything. It deletes its fixture between record and replay, which is what proves the tail byte came
out of the trace rather than off the disk.

**What is left standing, named rather than implied.** The fix covers three syscalls of an
**open-ended** forwarded set — there is no BSD-syscall allowlist; everything not explicitly
intercepted is forwarded. Still truncating, each checked against the SDK: `sysctl` (202, unbounded,
and this is `ps`); `pread_nocancel` (414, absent from `fd_operands`, the clamp **and** the window,
where the missing clamp is a host memory-safety hazard rather than a fidelity gap);
`getdirentries64` (344) and `recvfrom` (29/403), which have exactly the fixed shape but are not in
the predicate; `getfsstat64` (347), where 30 mounts crosses the cap and this machine has 24;
`proc_info` (336); `getattrlist`/`fgetattrlist` (220/228); `csops` (169/170). The `readv`/`recvmsg`
family is a worse class — nested destination pointers that nothing translates. Two further holes
found and not paid: `diff_memory` silently truncates a recorded region longer than its replay backing
(`.min(avail)`, flagged in M1's own branch review and deferred to M2, where only the clamp half was
paid), and the `if !err` gate skips write capture entirely on a failed syscall, which the comment
treats as universal and which `sysctl`'s `ENOMEM` behaviour may contradict — **unmeasured, and named
rather than fixed on inference.**

**The tripwire exists, works, and was deliberately not landed.** Between the pre-image and post-image
copies the only thing that runs is `host_svc` — the guest vCPU is halted and recorder threads are
banned — so any pre≠post byte is provably a kernel write from that syscall. If the window was capped
and its final 64 bytes all changed, the write reached the window's last byte and likely ran past it.
Prototyped, it fired exactly once on `/bin/ps`, on the real culprit, with no false positives. Turning
it into the `panic!` it should be requires knowing whether it fires on any existing gate guest, and
that measurement was not run. Landing a fail-loud assert without it would be precisely the "right
conclusion resting on an unmeasured supporting fact" this repo keeps catching in itself. It is
**M27**'s first task, ahead of `sysctl`, `pread_nocancel`, a general `dest_buffers` table, and
`diff_memory`'s own hole.

---

## Status: M27-truncguard — the band proved silence isn't proof, and `ps` was M26's bug the whole time

M26 prototyped the guard band by hand and named the blast-radius measurement it was withholding a
`panic!` on. **M27 Task 1 lands it as an `eprintln!` first**, deliberately not a `panic!` — landing a
fail-loud assert without measuring what it fires on across the whole gate is the unmeasured-
supporting-fact trap this milestone exists to avoid — and Task 2 runs that measurement. It did not
confirm what the design predicted; it found something more important than a missing table entry.

**Task 2's measurement: zero firings, and `/bin/ps` did not converge the way the plan expected.**
With the band live as a warning, the full chunked gate ran 518 / 0 / 2 over 115 binaries, every
chunk `EXIT=0`, and the band fired **nowhere** in the gate. That much the plan expected. `/bin/ps`
did not: the design predicted exactly one firing, on `sysctl` (202). There were none — and `ps`
still diverged on replay, now as a **syscall mismatch** (live `fstat64` vs. recorded
`open_nocancel`) at landmark 6528, roughly 6,100 traps *after* the `sysctl` call in question, not as
M26's memory-compare divergence at the window's edge. The corruption's visible symptom had moved
downstream between M26's measurement and this one — the same root cause, a different place the
stale bytes were finally read and acted on. That the fix in Task 3 closed the divergence at *this*
new site too, not just the one M26 saw, is itself evidence R2 (whether `ps` has one cause or
several) resolves to **one**.

**The false negative.** Probing `forward_and_diff` directly killed three competing explanations in
order, each with its own measurement: the `if !err` gate did not skip the capture (`err=false` on
all 83 `sysctl` calls in the run); the band **was** taken (`win=65536, band=64`); and the kernel
**did** overrun (`*oldlenp` after the call is 205,416 — 139,880 bytes past the window). The band's
64 bytes at `[65536, 65600)` were zeros before the call **and** zeros after. `struct kinfo_proc`
carries long zero runs, and offset 65536 happens to land inside one — the kernel wrote zeros over
zeros, and a byte-compare cannot tell that from no write at all.

That is exactly the false negative the design document named for a 1-byte band — "a tail of zeros
written over zeros" — and then argued a 64-byte band made negligible. **That argument is measured
false, on this milestone's own headline case.** The band is proof when it fires; it is not proof of
absence when it stays silent, and both documents say so rather than the stronger claim the design
hoped for. This did not change what Tasks 3 and 4 had to do: `dest_buffer`'s `DerefU64` shape reads
`*oldlenp` *before* the forward, where `ps` has already stored its own allocation size, so the
window covers the whole write regardless of whether the band would ever have caught it.

**Task 3: `pread_nocancel`, and a table instead of a predicate.** `retrace_arch::writes_x2_bytes_to_x1`
— M26's yes/no predicate, good for exactly one shape (a register holding a byte count) — could not
express `sysctl`'s shape (a length behind a guest pointer), so it is gone, not extended a second
time. `retrace_arch::dest_buffer(num) -> Option<(usize, DestLen)>` replaces it: `DestLen::Reg(n)`
for the read family, `DestLen::DerefU64(n)` for `sysctl`'s `*oldlenp`. One table drives both the
clamp and the window, which is the property that matters — a disagreement between them is exactly
the M26 defect. `pread_nocancel` (414) turned out to be missing from **three** places at once:
`fd_operands`, the forwarded-count clamp, and the window. The missing clamp was the serious third of
those — an unclamped forward lets the host kernel write past the guest buffer's actual backing,
which is a host memory-safety hazard, not merely a fidelity gap. `sysctl` (202) is seeded with
`DestLen::DerefU64(3)`. The table's doc comment says plainly what is and is not in it: seeded only
with what is measured or SDK-verified, and everything else is deliberately absent so it announces
itself through the band rather than being guessed at.

**Task 4: `/bin/ps` records and replays.** `ps_records_and_replays` (`crates/retrace/tests/sysbin_e2e.rs`)
asserts a genuine record-and-replay agreement, not merely a quiet tripwire, precisely because Task
2 found the divergence's visible site is not fixed across measurements. Guest stdout is
byte-identical between record and replay. The Apple-binary sweep moves **46 → 47**.

**Task 5: the class fails loud.** The band's `eprintln!` becomes an `assert!` that panics on the
first overrun it sees, naming the syscall, the window size, and the IPA, and pointing at
`retrace_arch::dest_buffer` as the fix. And the `readv`/`recvmsg` nested-pointer family — `readv`
(120), `readv_nocancel` (411), `recvmsg` (27), `recvmsg_nocancel` (401), `preadv` (540), `recvmsg_x`
(480) — is refused **by value** in `retrace-core`'s record dispatch (`writes_via_nested_pointer`),
the same discipline `guest_workq_kernreturn` already uses for an opcode nothing has measured: their
destination sits behind a pointer *inside* a guest struct (`iovec.iov_base`, `msghdr.msg_iov`), which
`forward_and_diff` never translates, so before this fix forwarding one would have handed the host
kernel a **guest** address to write through, as a **host** one — a wild-write hazard, not a
truncation. Reading that as merely `EFAULT`-safe (guest IPAs being unlikely to collide with mapped
host addresses) is an inference nobody had measured and the downside of it being wrong is severe, so
the assert refuses rather than guesses. No guest anywhere in the gate calls any of the six — measured
absent from M25's 69-number CPython syscall census — so nothing that passed today is affected.

**The gate: 523 passed / 0 failed / 2 ignored across 115 test binaries**, every chunk `EXIT=0`,
clippy clean over `--workspace --all-targets`. Reconciled against M26's 515 / 0 / 2 over 114
**file-by-file**: `retrace-arch/src/lib.rs` **+4** (`pread_nocancel_is_treated_exactly_like_pread`,
`dest_buffer_knows_where_each_length_lives`, `dest_buffer_omits_what_it_should` from Task 3, plus
`the_nested_pointer_family_is_named_in_full` from Task 5); the new
`retrace-box/tests/truncguard.rs` **+3 and +1 binary** (the guard band's own unit tests, written
against `Box_::overran_window` directly rather than through a live syscall); `retrace/tests/sysbin_e2e.rs`
**+1** (`ps_records_and_replays`); `--bins` unchanged at **11**. **+8 running from +8 new tests** —
unlike M26, nothing moved out of the ignored column this time, so the count closes as pure addition:
515 + 4 + 3 + 1 = 523, and the tree holds 517 `#[test]` at M26 = 515 + 2, 525 at M27 = 523 + 2.

**What is left standing, named rather than implied:**

- **`Box_::diff_memory`'s own `.min(avail)` clamp**, on the *replay* side, silently truncates a
  recorded region longer than its replay-side backing. Flagged in M1's own branch review, deferred
  at M2 alongside the clamp M26 eventually paid, and still unpaid.
- **The `if !err` gate**, which skips write capture entirely on a failed syscall. M27 measured that
  this is *not* what `/bin/ps` hit — `err=false` on all 83 of its `sysctl` calls in the run — which
  narrows the question without closing it. The comment at the call site still treats the skip as
  universally safe; nothing has measured whether a failing `sysctl` (`ENOMEM`, say) writes a partial
  reply anyway.
- **The remainder of the audit table** — `getdirentries64` (344), `recvfrom` (29/403),
  `getfsstat64` (347), `proc_info` (336), `getattrlist`/`fgetattrlist` (220/228), `csops` (169/170) —
  moved from *listed* to *guarded* by the band, which is a weaker statement than *closed* and is
  written as the weaker one in both documents: the band is proof when it fires and not proof of
  absence when silent, so these syscalls are unmeasured rather than verified safe.
- **A clamp for the `DerefU64` shape** is owed and unmeasured: the window widens to cover `sysctl`'s
  `*oldlenp`, but the forwarded count itself is not clamped by it. `sysctl` was unclamped before M27
  too, so this regresses nothing — it is a debt carried forward, not a new one.
- **Strengthening the band itself** — sampling across the whole remaining backing under a fixed byte
  budget, rather than one contiguous 64-byte run immediately past the window, which is exactly the
  shape the zeros-over-zeros false negative exploited — was deliberately not attempted in M27. It is
  the obvious successor to this milestone's own finding.

The successor is open: no gate anywhere in the tree is currently parked on a truncation-class wall,
and the guard band stands as a live panic rather than a prototype. The next candidates are the
band-strengthening question above, the `if !err` gate, and `diff_memory`'s replay-side clamp —
whichever of the three a future measurement makes urgent first.

---

## Status: M28-bandproof — a tripwire proven able to fire, made attributable, and its silence explained rather than assumed

M27 landed the guard band as a fail-loud `assert!` after measuring it fire zero times across the
whole gate — and closed with the same softened claim it opened with: a changed band byte was proof
of *some* kernel write past the diff window, but not proof it belonged to the argument whose overrun
it was meant to catch, because another argument's own window landing in that range would trip it
too. M27's final review put paying that debt at the top of the successor list, ahead of widening
anything. **M28 is that milestone, and it does not widen the band — it makes the band trustworthy
first**, per the design's own title.

**Task 1: nobody had proven the band could fire at all.** Every existing guest that reaches the
band's `assert!` reaches it *not firing* — that is what "zero firings across 523 tests" meant — and
`let band = 0;`, a mutation that disables the band outright, passed that same 523-test gate
identically to the shipped code. A test that has never watched the code it guards fail is not a
control. `Box_::set_window_cap_for_test` shrinks the diff-window cap to a test-chosen value, and a
positive control drives `fileio`'s `fstat` — which writes a MEASURED `sizeof(struct stat) = 144`
bytes and is deliberately absent from `retrace_arch::dest_buffer`, so no widening rescues it —
through a 64-byte cap. With `let band = 0;` applied, the test was verified to **FAIL**, panicking
`NOT-THE-GUARD-BAND: fstat wrote past a 64-byte window and nothing fired` rather than silently
passing; reverted, it is green. The guard band can now be shown to fire.

**Task 2: a band means this argument, or it means nothing.** `Box_::band_not_covered(ipa, len, band,
others)` shrinks each argument's band to exclude any byte some *other* window of the same call
already inspects — the spans are collected before the post-forward loop consumes `windows`, since
every span (including the argument's own, which ends exactly where its band begins and so can never
suppress itself) must be visible when each band is computed. Proven on span *intersection*, not
merely start position, which is the case a naive implementation misses: a neighbour beginning
*before* the band but reaching into it truncates exactly as much as one starting inside it. With this
in place, a changed byte in what remains of the band cannot be a write that another window of this
call already captured — so it is proof of a kernel write past everything this call's diff inspected.
Which *argument* overran is still not established: a different argument's overrun, running past its
own window, reaches this band too. The
shrink itself announces through an `eprintln!("[M28 BANDSHRINK] …")` rather than a second assert,
because whether it fires rarely or commonly was still unmeasured — that measurement is Task 3, not
an assumption Task 2 was entitled to make about its own code.

**Task 3: the shrink is not rare, and that is a finding about M27, not a regression in M28.** No
`[M28 BANDSHRINK]` line was visible in the full gate's captured output at Task 2's commit — but that
is not a count: the line is recorder stderr, and every e2e gate test drives the recorder as a child
process via `crates/retrace/tests/util/mod.rs`, which pipes its stderr into a `String` a passing test
never prints, so no such line could have reached that log regardless of how often it fired. The one direct
measurement is `/bin/ps`, run by hand (reproduce with `RETRACE_TRACE=1 cargo run -p retrace --
record-dyn /bin/ps`, now that the line is gated behind that flag — see Known limits): it shrank a
band **31 times** — six syscalls, 344 (`getdirentries64`) ×15, 399 ×11, 33 (`access`) ×2, and one
each of 5 (`open`), 347, 339 (`fstat64`) — every one a complete `64 -> 0` on a capped 65536-byte
window. The mechanism is address arithmetic, not a bug: two arguments of one call routinely point
into the same backing (`ps`'s two pointers sit 304 bytes apart on the stack), and once both take a
64 KiB window, each argument's band lands entirely inside the other's window. **Suppression is not a
blind spot.** A suppressed byte is one some other window of the same call already inspects — the
kernel write there is captured, just recorded against a different argument's ipa — so nothing is lost
by not flagging it a second time. And it cannot become one structurally: the window with the maximal
end address in a backing can never itself be suppressed, since suppression needs some other window
ending even further out, which is impossible for whichever window already ends furthest — so the band
immediately past everything the call inspected stays guarded no matter how many inner bands get
truncated to zero. What the 31 actually measures is how often M27's band was claiming proof it did not
have: the band was weaker than M27's own text admitted, and only becomes correct — for the first
time — with Task 2's shrink in place. The ruling was explicit: do not narrow the shrink to make this
number smaller; a band that cannot be attributed proves nothing, which is the whole thesis of Task 2.

**Task 4: the `if !err` skip, measured on one purpose-built case.** `forward_and_diff` skips write
capture — and the band along with it — entirely when a syscall's carry flag is set, on the stated
but unmeasured assumption that a failed syscall writes nothing. A guest built to fail deliberately
(`sysctl(KERN_OSTYPE)` into a 2-byte buffer, where `"Darwin\0"` needs seven) measured it directly:
`err=true ret=12 (ENOMEM) writes_captured=0 buf_changed=false` — the kernel wrote nothing, before or
after. The reviewer went beyond the brief and reproduced this independently, out-of-band:
disassembling the committed guest to confirm the mib, namelen and undersized `oldlenp` are what it
actually issues, and replaying the identical syscall twice against the live host kernel. This is one
measured case, not a general proof about failing syscalls — the gate stays open, now with a datum in
it instead of none.

**Task 5: Branch B, by the measurement, not by default.** The spec offered two branches for the
`if !err` finding: hoist the diff out of the skip (Branch A) if the measurement found a real loss, or
land the measurement as a standing test with no code change (Branch B) if it did not. Task 4 found no
loss on the one case built to provoke it, so Branch A is not taken and is not half-implemented "just
in case" — `failwrite.rs` lands as a standing measurement
(`a_failing_sysctl_is_measured_for_writes`), asserting only what is known (the call fails) and the
datum Task 4 measured (the buffer is unchanged), not a general claim neither task earned.

**Task 6 restores the strong claim in the assert's own message.** M27 had softened it because it was
false; Task 2 made it true, so the message now says a changed band byte IS proof of a kernel write
past everything this call's diff inspected, and still points at `retrace_arch::dest_buffer` as the
fix. The positive control's `#[should_panic(expected = "changed a byte in the")]` still matches the
rewritten wording; re-run of `truncguard.rs` after the change: 8 passed, 0 failed — the file
as it stood at Task 6, before the fix wave took it to 11.

**The gate: 532 passed / 0 failed / 2 ignored across 116 test binaries**, every chunk `EXIT=0`,
clippy clean over `--workspace --all-targets`. Reconciled against M27's 523 / 0 / 2 over 115
file-by-file: the existing `retrace-box/tests/truncguard.rs` **+8** (the positive control, Task 2's
four `band_not_covered` tests — covering span intersection and self-exclusion, not merely start
position — and three more from the fix wave: multiple-overlap, `band == 0`, and `len == 0`),
the new `retrace-box/tests/failwrite.rs` **+1 and +1 binary**. `--bins` unchanged at **11**, every
other file unchanged. 523 + 8 + 1 = 532; the tree holds 525 `#[test]` at M27 = 523 + 2, and 534 at
M28 = 532 + 2. The two ignored gates (`stackoverflow_rust_e2e`, `cache_symbol_e2e`) are unchanged
from M27. **M28 parked nothing new.**

**What is left standing, named rather than implied:**

- **The band's coverage limit is untouched.** M27 measured a contiguous 64-byte band missing a real
  139,880-byte overrun on `/bin/ps`, because the bytes past the window were zeros before and after
  (`struct kinfo_proc` carries long zero runs) — a byte-compare cannot tell a kernel write of zeros
  from no write at all. M28 did not fix that and did not attempt to: the band is proof when it fires;
  its silence is still not proof of absence.
- **`Box_::diff_memory`'s own `.min(avail)` clamp**, on the replay side, is still unpaid — flagged in
  M1's own branch review, deferred at M2.
- **The `DerefU64` clamp for `sysctl`'s in-out `*oldlenp`** is still owed: the window widens to cover
  it, but the forwarded count itself is not clamped by it.
- **The remainder of the audit table** — `getdirentries64` (344), `recvfrom` (29/403), `getfsstat64`
  (347), `proc_info` (336), `getattrlist`/`fgetattrlist` (220/228), `csops` (169/170) — stays guarded
  by the band rather than cleared by `dest_buffer`, unchanged from M27.
- **Strengthening the band itself** — sampling across the whole remaining backing under a fixed byte
  budget, rather than one contiguous 64-byte run — is now *unblocked*: the "not covered by another
  window of this call" precondition Task 2 needed exists in code as `band_not_covered`. But Task 3's
  count is a warning to whoever takes it up next, not an invitation: a naive wider sample would be
  suppressed even more often than 31 times, not less, and a successor milestone that skips that
  measurement would be repeating M27's original mistake rather than correcting it.

---

## Status: M29-clamptable — the audit table shortens to three, and a zero is only as good as the channel that carried it

M26 found the truncating-diff-window class, M27 made it fail loud and freed `/bin/ps`, and M28 proved
the tripwire could fire and made a firing attributable. What all three left behind was written down
in one README paragraph: a named list of syscalls that still get a flat 64 KiB window, and one clamp
that "stays owed and unmeasured even for a covered syscall." **M29 pays the clamp and halves that list**, from the six
entries it named (`getdirentries64`, `recvfrom`, `getfsstat64`, `proc_info`,
`getattrlist`/`fgetattrlist`, `csops`) to three — plus one, `sysctlbyname`, that the list had never
named at all. It is a hardening milestone — it adds no capability, un-parks nothing,
and bumps no magic.

It is also the milestone that hit its own subject three times: **a measurement is only as good as the
channel that carried it.** M28's "zero band shrinks across the full gate" came off a channel no
passing test could have written to; M29's own first Apple-sweep measurement came off a file that was
grepped for something else and then deleted; and the controller repeated that second false zero
onward before a reviewer caught it. Two of the three were caught only by asking what the channel
could physically have carried, which is the habit this section is written to pass on.

**Task 1: the Apple sweep is a script now, not a memory.** `tools/apple-sweep.sh` and a committed
54-entry corpus (`tools/apple-sweep-binaries.txt`) record and replay each binary and print a `TALLY`
line, so the figure this repo has quoted since M22 is reproducible instead of remembered. The corpus
is a **reconstruction** — the original sample behind the 47 published at M27 was never committed — so
the new number is not strictly comparable to the old one, and is reported rather than massaged toward
it. Two bugs in the script's own first draft had to be fixed before it measured anything: a scratch
variable inside the timeout helper clobbered the caller's captured exit code, and the divergence check
compared a variable against itself, making it a tautology. With both fixed the tally is
**`TALLY pass=46 fail=8 skip=0`**, and fixing the tautology is what exposed **`/bin/launchctl`** as a
genuine replay divergence — it had always been diverging, and had never before been visible. The eight
are `csh`/`tcsh` (`recorder panicked` — the M10 fd table's fail-loud unmodelled `dup2`, by design);
`automationmodetool`, `desdp`, `dyld_info`, `flex` and `/bin/launchctl` (`replay diverged`); and
`/usr/bin/yes` (`timed out after 30s recording`). Those reason strings are what the sweep prints, not
why the binaries fail: `automationmodetool`, `desdp`, `dyld_info` and `flex` have been believed since
M23 to reach a `brk`, and the sweep neither confirms nor contradicts that — it sees a diverging
replay and nothing more. That cause remains unmeasured, with no parked gate standing for it.
`csh`/`tcsh` are the exception: their `dup2` panic text was read directly off a re-run outside the
sweep harness, so that one is a measurement rather than a category. `/usr/bin/yes` never terminates and so cannot
pass under any bounded method; it is counted FAIL **on purpose**, since excluding it would raise the
tally without changing retrace. `dddiagnose`'s documented intermittency stopped being an assertion and
became an observation: two runs of the same script against the same tree gave 45/9 and 46/8, with
`dddiagnose` the only mover.

**Task 2: four syscalls that already fit the table's shape, and one that nobody had noticed was
missing.** `retrace_arch::dest_buffer` gained `getdirentries64`(344) `=> (1, Reg(2))`,
`getfsstat64`(347) `=> (0, Reg(1))` (its `x1` is a byte count, not a mount count),
`recvfrom`/`recvfrom_nocancel`(29/403) `=> (1, Reg(2))`, and `sysctlbyname`(274) `=> (2, DerefU64(3))`.
`fd_operands` gained both `recvfrom` spellings, which had been in neither table while `sendto` was
already in one — the same both-tables-at-once asymmetry M27 found in `pread_nocancel`, and the fourth
time a `_nocancel` variant was missing from a table its plain sibling was in. Three of the four have a
*second* destination; each is named in the code and dismissed on a number (`getdirentries64`'s 8-byte
`*position`; `recvfrom`'s `from`, capped by the kernel at `sockaddr_storage`'s 128 bytes rather than
at `*fromlen`), because all of them sit far inside the flat window and so cannot produce the class
this table exists to prevent. No general multi-destination table was built.

One number attached to `getfsstat64` did not survive being checked. Its match arm is commented
"~24 mounts x sizeof(struct statfs64) on this machine, which crosses the 64 KiB cap," and the
README carried a matching "30 mounts crosses the cap and this machine has 24." Measured at this
milestone's close: `sizeof(struct statfs)` is **2168** bytes and this machine has **16** mounts, so
the call needs **31** mounts to cross a 65536-byte window and currently asks for 34,688. The table
entry is right and worth having — `getfsstat64` is structurally able to overrun, and its `x1` really
is a byte count — but the comment's justification is not, and the README sentence that repeated it
has been replaced rather than carried forward.

**`sysctlbyname` is the interesting one, twice over.** First, it was absent from `dest_buffer` **and**
from the README's own list of what was absent — a list of known gaps is only as complete as the audit
that wrote it, which is the argument for closing a table rather than extending it entry by entry.
Second, it was added with the **wrong indices**. The entry went in as `(1, DerefU64(2))` under the
comment "sysctl's exact shape, one index lower," reasoning from libc's 5-arg `sysctlbyname(3)`
prototype straight onto register positions. The raw kernel entry takes `namelen` first, exactly like
`SYS_SYSCTL`, so the true pair is `(2, DerefU64(3))` — **identical** to `sysctl`'s, not one lower.
The old sentence reached a true conclusion ("one arm covers both") from a false premise ("their
indices differ by one"), and it **survived a review that checked all five new entries against the
SDK**, because raw-versus-libc argument shape is invisible in a man page. It was caught in Task 4 and
settled by calling `syscall(274, …)` against the live kernel with the wrapper bypassed: the 6-arg form
on `"kern.ostype"` returns 0 with `oldp` filled and `*oldlenp` 7; the 5-arg reading returns −1. Had it
shipped, a real `sysctlbyname` would have read `want` out of the destination buffer's own first eight
bytes and treated `namelen` as the destination pointer — misdiagnosed as unbacked, not measured.

**Task 3 proved the entries take effect** rather than assuming the table is wired to anything.
`the_window_widens_for_each_m29_reg_addition` drives `Box_::diff_window_for_test` at the seam, with
`avail` set to 1 MiB so the backing is never the binding constraint — a too-small `avail` would make
every case clamp to the same number and pass for the wrong reason. It carries two negative controls
that matter more than the positive ones: `getdirentries64`'s `x3` (its `*position`, not its buffer)
still gets the flat cap, so the test would fail if `dest_buffer` widened *every* pointer argument;
and `write`, absent from the table, gets the flat cap at every index. It covers the three `Reg`
additions only. `sysctlbyname`'s `DerefU64` pair is asserted in `retrace-arch`'s own table test and
nowhere at the window seam — the first of several places this milestone's newest entry is thinner
than its siblings.

**Task 4 measured `*oldlenp` against its backing before anything was refused — and the first version
of that measurement was the exact failure this milestone exists to correct.** A diagnostic arm printed
`[M29 DEREFLEN]` (want > backing), `[M29 DEREFLEN-FIT]` (want fits) and `[M29 DEREFLEN-UNBACKED]` (no
backing) around `Box_::backing_of`, and the Apple sweep was run under it. It reported **zero
dispatches across all 54 binaries** — and that zero was structurally guaranteed: `apple-sweep.sh`
redirects each recorder's stderr into `$TMP/rec.err`, greps it only for `"panicked at"`, and deletes
it in an `EXIT` trap, so no `[M29 DEREFLEN*]` line ever had a path onto the channel being grepped, for
any guest, ever. The sweep script now greps that captured stderr for the tag and echoes matching
lines onto its own stdout, prefixed by guest name, gated behind `RETRACE_DEREFLEN`. The repaired
channel was then required to pass a **positive control** — a corrected channel still reporting 0 for
`/bin/ps` would still be broken — and `/bin/ps` returned 66 lines, matching its standalone run.
The false zero had already been reported onward as fact before a reviewer asked how the number could
have reached the log at all; the corrected figure below is the opposite extreme, not a near miss.

The corrected Phase A result, per corpus part, each from a command whose output was read:

| part | dispatches reaching `forward_and_diff` | oversized | unbacked |
|---|---|---|---|
| Apple sweep, all 54 binaries (`RETRACE_DEREFLEN=1 tools/apple-sweep.sh`) | 783 | 0 | 0 |
| `jq --version` (`record-dyn /opt/homebrew/bin/jq`) | 13 | 0 | 0 |
| CPython via the Homebrew launcher shim (`-c 'print(1)'`) | 15 | 0 | 0 |
| CPython via the real interpreter binary (`-c 'print(1)'`) | 15 | 0 | 0 |

**826 dispatches, every one fitting inside its backing.** `/bin/ps` is inside the sweep's 783 and is
**not** a fifth addend; its standalone count varies run to run (62, then 66, then 66) because it walks
the live host process table, which is guest-environment variance and not a recorder property. All 826
are `syscall 202`.

**Three narrownesses bound that zero, and they are the reason it is not a stronger claim.**
`sysctlbyname`(274) is exercised by **nothing** in any corpus — its table entry is correct by
measurement of the kernel's argument shape, but untested by any guest, and the refusal covers it by
code-path symmetry only. "Reached" means reaching `forward_and_diff`: the `sysctl(KERN_USRSTACK64)`
that `retrace-core` answers itself never arrives there and is not in the count. And the two CPython
rows are one interpreter startup measured twice — identical `want` value sets, differing only by a
small allocator offset in `avail` — so the evidence base is **56 effectively-independent binaries**,
not 57.

**Task 5 took Branch B-REFUSE: refuse, do not clamp.** `sysctl`'s `*oldlenp` is an *in-out* length —
the guest writes how much room it has, the kernel writes back how much it used — so a silent clamp
would tell the kernel a smaller buffer than the guest asked for and hand the guest a truncated reply
it has no way to know was truncated: a wrong answer dressed as a right one. `forward_and_diff` now
asserts by name when `want > avail`, the discipline `guest_workq_kernreturn` uses for an unenumerated
opcode, with `RETRACE_DEREFLEN=1` named in the message as the way to print the backing span that
settles which side is wrong. The 826-dispatch zero is what makes this safe rather than reckless:
nothing that runs today is refused. A purpose-built guest, `crates/retrace-guest/asm/oldlensysctl.s`,
issues a legal size query and then an illegal `*oldlenp = 1 << 40`; one test proves the refusal fires
and a second proves the NULL-`oldp` size query still passes.

A second bug surfaced only because TDD demanded a genuine RED: the test loop drove `forward_and_diff`
without ever calling `set_x0_err_and_return`, so the vCPU never advanced past the guest's first `svc`
and **both tests were re-driving the first syscall forever**, never reaching the illegal second one.
The RED they produced was textually the expected one and would have been accepted. Without the fix,
the assertion could never have been exercised by either test.

**A tension this creates, recorded so nobody re-derives a number that can no longer be taken:**
`forward_and_diff` now **aborts on the first oversized dispatch**, so the Phase A diagnostic can no
longer survey a corpus. Measurement mode and refusal mode are in tension; an OVERSIZED line is now
necessarily the last thing the process prints, and a corpus-wide OVERSIZED tally is not an available
measurement any more.

**Task 6 made M28's band-suppression count a real observation.** M28 published two numbers here: 31
suppressions on `/bin/ps`, hand-measured, and "zero band shrinks across the full gate" — and the
second could not have been true, because `[M28 BANDSHRINK]` is recorder stderr and every e2e test
drives the recorder as a child whose stderr `crates/retrace/tests/util/mod.rs` pipes into a `String`
a passing test never prints. `ps_records_and_replays` now records `/bin/ps` through a new
`util::record_dynamic_env` helper with **`RETRACE_BANDSHRINK=1`** set on the recorder — a gate
separate from `RETRACE_TRACE`, so the count is obtainable without the per-trap firehose — and asserts
the count is **greater than zero**, keeping all four of its pre-existing assertions. The gate
therefore enforces a **floor, not a number**; the number itself reaches only a `--nocapture` run,
which reported **32** on one run and **30** on another of the same tree at the milestone's close. So
the count is not a property of the machine either, and neither it nor its distance from M28's 31 is
**root-caused**: nothing measured which call produces a given suppression, and no explanation is
offered here. That is exactly why the assertion is `> 0` and not `== 31` — what fails should be a
portable property, and this milestone does not know what makes the count vary. The second
observation arrived late, in the final fix wave, and is recorded because it narrows the claim: an
earlier draft of this section said "32 on this machine", which reads as a stable machine property and
is not one.

**Gate: 537 passed / 0 failed / 2 ignored over 116 test binaries**, every one of ten chunks
`EXIT=0`, clippy clean over `--workspace --all-targets` at `-D warnings`. Reconciled against M28's
532 / 0 / 2 over 116 **file-by-file rather than by sum**: `crates/retrace-arch/src/lib.rs` **+2**
(Task 2), `crates/retrace-box/tests/truncguard.rs` **+3** (Task 3's window-widening test, Task 5's
two refusal tests). Every other file unchanged;
`--bins` **11 → 11**. Task 6 added no test — it extended `ps_records_and_replays`. No new test binary:
`oldlensysctl.s` is a guest fixture and `truncguard.rs` already existed, so the binary count stays
**116**. The count closes at both ends: the tree holds 534 `#[test]` at M28 = 532 + 2 ignored, and
**539** at M29 = 537 + 2. The two ignored gates (`stackoverflow_rust_e2e`,
`cache_symbol_e2e`) are unchanged. **M29 parked nothing new and un-parked nothing** — as its design
predicted for a hardening milestone.

The sweep was re-run at the close: **`TALLY pass=46 fail=8 skip=0`**, with both the PASS set and the
FAIL set byte-identical to Task 4's post-fix run — the third independent run to land there with the
same sets. The baseline it was compared against is Task 4's own post-fix sweep and deliberately
**not** the `/tmp` file the plan named, which is a pre-Task-4 artifact holding 51 passes from the
tautological run; diffing against that would have shown a large and entirely spurious set change.
A milestone that compares against a baseline without checking what produced it is making the same
mistake as one that trusts a channel without checking what could reach it.

**What stays owed, named rather than implied:**

- **M27's measured coverage false negative is untouched.** A 64-byte band of zeros still misses an
  overrun into zeros; M28 hardened the tripwire without closing that and M29 did not touch it either.
- **`Box_::diff_memory`'s own `.min(avail)` clamp** on the replay side is still unpaid — flagged in
  M1's own branch review and deferred at M2.
- **`proc_info`(336), `getattrlist`/`fgetattrlist`(220/228) and `csops`(169/170)** still get a flat
  64 KiB window. They are what remains of the audit table after M29, and none has been measured to
  overrun.
- **The `readv`/`recvmsg` family** (120/27/540/411/401/480) stays refused by value since M27, until
  the `translate_mwl_regions` treatment and its own measurement extend to them.
- **The refusal's boundary is pinned by nothing: no unit test drives a backed, *fitting* `sysctl`
  through the assert at all.** Only the oversized direction is covered.
  `a_null_oldp_sysctl_is_not_refused` passes `oldp = NULL`, so `host_span(0)` is `None` and it never
  reaches the comparison, and the other test's guest asks for `1 << 40`. So `want == avail` — the
  exact boundary the comparison turns on — is exercised by no test in the unit gate, and the sweep
  and e2e gates catch a change there only if some real call happens to sit on it.
  **The fix is cheap and is owed:** a third `sysctl` in `crates/retrace-guest/asm/oldlensysctl.s`
  between calls 1 and 2 — `oldp = buf`, `*oldlenp = 64`, which *fits* — driven by the null test.

  > **CLOSED, but NOT by the remedy this bullet prescribes — that remedy could not have worked.**
  > `avail` is the distance from the destination to the end of its *backing*, not to the end of the
  > 64-byte `buf` symbol. `buf` sits at `__DATA+0x18` inside a mapping at least a page long, so a
  > `*oldlenp = 64` request sits far *under* `avail`, and the strictness mutation `want < avail`
  > stays green against it exactly as it did before. Only `want == avail` separates `<` from `<=`,
  > and a freestanding guest cannot know `avail` — so no guest fixture can reach this boundary at
  > all. The bullet correctly identified the gap and then prescribed something that does not close
  > it.
  >
  > Closed instead by extracting the comparison as a pure predicate, `Box_::deref_len_fits(want,
  > avail)`, and testing the boundary directly — the same treatment `clamp_count` and
  > `overran_window` already get, for the reason `overran_window`'s own doc comment gives: the
  > policy is reviewable apart from the plumbing that feeds it. Here it is also the only way the
  > policy is *testable*. `truncguard.rs::a_deref_len_is_refused_only_past_its_backing` pins
  > `63/64` (under), `64/64` (exact), `65/64` (past), plus `0/0` and `1/0`.
  >
  > Verified by mutation, all four caught where `want < avail` previously survived the whole unit
  > gate: `<` → 14 passed/1 failed; `>=`, `>`, `!=` → 13 passed/2 failed each; source reverted
  > byte-for-byte after each, baseline re-run green. The generalisable point is the one this
  > milestone kept relearning: **a gap correctly identified is not a gap correctly closed**, and a
  > prescribed remedy deserves the same "does the channel reach it" test as a prescribed number.
  This is stated from measurement rather than from reading. Both mutations were applied to
  `crates/retrace-box/src/lib.rs` with `truncguard` re-run against each, then reverted byte-for-byte:
  the **inversion** `want >= avail` is **caught** (13 passed, 1 failed) —
  `an_oldlenp_past_its_backing_is_refused`'s own `NOT-THE-REFUSAL` sentinel fires when the second
  `sysctl` returns normally, and `should_panic(expected = "syscall 202 asked for")` rejects that
  message, which is precisely the job the sentinel was written to do. What **survives** is the
  **strictness** mutation `want < avail`: all 14 tests green. Task 5's review had recorded the
  inversion as the surviving case; testing it moved the finding to a narrower and more actionable
  place, which is the only reason the mutation results are recorded here at all.
- **The new BANDSHRINK assertion can be greened by an exported `RETRACE_TRACE=1`**, because
  `Command` inherits the parent environment and the box's gate fires on either variable. The test
  proves the count is non-zero; it does not prove `RETRACE_BANDSHRINK` is what delivered it.

  > **CLOSED before merge, by this same milestone — the bullet above was already false when M29
  > landed.** The final fix wave (`a4b7e90`) added `c.env_remove("RETRACE_TRACE")` to `run_env`
  > (`crates/retrace/tests/util/mod.rs:65`), placed after `c.args(args)` and *before* the caller's
  > env loop, so a caller passing the variable deliberately still overrides. It was proven rather
  > than argued: with the `RETRACE_BANDSHRINK` limb deleted and `RETRACE_TRACE=1` exported,
  > `sysbin_e2e` FAILS (`saw none`, exit 101) where it previously passed — which can only happen if
  > `env_remove` stripped the inherited variable *and* nothing but the deleted limb was enabling the
  > counter. The assertion now passes because of the gate it tests.
  >
  > The bullet is left standing rather than corrected in place, per this log's append-only
  > discipline. It is recorded here because the failure is worth more than the fix: the wave that
  > closed this item ran *after* the section above was written, and the owed-list was not re-checked
  > against it. A milestone that closes an item during its own fix wave must re-read what it already
  > published as owed — the same "a claim must trace to the state that produced it" rule this
  > milestone is about, applied to its own closing document.

---

## Status: M30-canary — a band you can lose a signal in, and three more instruments that could not fire

M26 found the truncating-diff-window class, M27 made it fail loud, M28 proved the tripwire could fire
and made a firing attributable, and M29 proved the channel that *reports* a firing could carry the
signal. All four left the same hole standing, and M27 had measured it rather than feared it: the
detector was `overran_window(pre, post)`, a pre/post **comparison**, so it could only ever report a
*change*. Whenever the kernel wrote bytes identical to the ones already in the band, it was **100%
blind** — and that is exactly what happened on `/bin/ps`, whose 139,880-byte overrun landed inside
`struct kinfo_proc`'s long zero runs. The kernel wrote zeros over zeros; the band reported nothing;
M27 could only write the finding down in prose.

**M30 changes the kind of detector, not its size.** `GUARD_BAND` is still **64**. `forward_and_diff`
now *fills* each shrunk band with `Box_::canary_byte(ipa) = (ipa as u8) ^ 0xA5` before forwarding,
asks `Box_::canary_intact` after, and restores the bytes before the guest resumes. A kernel write
across that band destroys a pattern retrace itself placed, whatever bytes it wrote — subject only to
the 1/256 residual recorded below — and **aborts the recording**. This is a hardening milestone: it adds no capability, un-parks nothing, bumps no magic,
and changes no recorded bytes.

It is also, unavoidably, a milestone about instruments that cannot fire — because during M30 **three
more** were found, and none of them was found by a test going red.

**Why the pattern is derived from the address.** A constant would have done the anti-coincidence job
half as well and the overlap job not at all. Two reasons, both load-bearing:

* **Anti-coincidence.** `0xA5` is neither `0x00` nor `0xFF` at `ipa = 0`, the two commonest
  uninitialised and accidental fills — including the exact zeros-over-zeros case that made this
  milestone necessary. A constant-value `memset` by the kernel cannot reproduce an address-varying
  pattern at more than one byte.
* **Overlap consistency.** Two bands that overlap must agree on every byte they share, or the second
  one's fill would look like a disturbance to the first. Deriving the byte from the address makes
  that hold by construction, with no ordering rule for the caller to get wrong.

**Task 3 had to move `band_not_covered` before the fill, and this is the part that would have been a
silent corruption.** Since M28 the band is *shrunk* to exclude the bytes some other window of the
same call already inspects, and that shrink used to run in the post-syscall loop. A canary written
into an unshrunk band would land inside another argument's diff window and be captured as a kernel
write that never happened — retrace's own bytes, recorded as the kernel's. So the shrink moved into
a pre-pass and `windows` grew a fifth field carrying the shrunk length; the post-loop reads it
instead of recomputing. Clippy's `type_complexity` refused the resulting five-tuple, so it is now a
`type Window` alias — clippy's own suggested fix, no behaviour change.

### The cost, which is not small and is not a footnote

**The canary is withheld from `retrace_arch::reads_guest_buffer`** — `write`/`pwrite`/`writev`, the
`send*` family, `sendfile`, `msync` and `mach_msg2_trap`, each with its `_nocancel` spelling —
because the kernel reads *through* those buffers and would consume the canary as data. That is
measured, not feared. Filling them made a 128 KiB-write guest produce a file with **64 corrupted
bytes matching `canary_byte` exactly**, while **record exited 0, replay exited 0, and the canary
count read 0** — record and replay agreeing perfectly while the guest's output was wrong, which is
the one failure a determinism oracle cannot see, and the same shape as M18's dropped-wake argument.
That family keeps `overran_window` **bit-for-bit**.

That bit-for-bit is a fix-round-2 correction, and round 1 had it wrong in the direction that matters.
Round 1 ran a *reconstruction* of the old condition on the unfilled bands and argued it was "no more
and no less" than M27. It was less: against a canary that was never written,
`post[k] != canary_byte(base+k)` is true but for a 1/256 coincidence, so the condition collapses to
`overran_window` **minus** that miss — strictly weaker than what shipped before this milestone, on
precisely the family the fill had been withdrawn from in order to protect it. Round 1 had checked
that the reconstruction could not false-alarm and never asked whether it could miss. The detector now
picks on `fill_canary`: `canary_intact` when filled, `overran_window` when not.

**Two of those exclusions cost real destination-side coverage.** `sendfile`'s 4th argument is an
in-out `off_t *` the kernel writes the transferred count back through. `mach_msg2`'s receive buffer
is a live destination: `machmsg.rs`'s `FORWARD_ALLOWLIST` forwards five ids through
`forward_and_diff` (`host_info` 200, `host_get_clock_service` 206, `semaphore_create` 3418,
`task_info` 3405, `host_get_special_port` 412), and the last two exist *precisely* because the kernel
writes a reply into guest memory — traffic every jq and CPython run exercises. Recovering that needs a
per-**argument** direction notion, a `dest_buffer`-shaped table of which arguments are sources, which
a predicate over the syscall number cannot express. **That is owed successor work**, and naming it is
the honest close rather than letting the assert read as covering the whole `forward_and_diff` surface.

**`reads_guest_buffer` is a list, and a list is not a proof.** `ioctl` is the named hole: a `_IOW`
request encodes its buffer length in the request code, so no rule over the syscall number can size
it. Path-taking calls are deliberately absent, and for a stated bound rather than a vibe — a path is
NUL-terminated and the kernel stops at `PATH_MAX` (1024), far inside the production window. `msync` is
the single entry justified by inference rather than measurement (all guest memory is anonymous, which
*probably* makes its read moot), listed anyway because nothing is lost by listing and a silent
corruption follows if the inference is wrong; it says so at its definition.

**And on a filled band, a kernel write that reproduces the canary pattern exactly is still
undetectable in principle** — 1/256 per byte, with the kernel having to hit it on every byte it
writes to stay invisible. That is inherent to any canary. It is the price of replacing a detector that
was 100% blind to the zeros case with one that is 1-in-256 blind, and it is stated rather than
claimed away.

### Phase A: measure, then flip

The counter shipped report-only first (`canary_disturbances`, `[M30 CANARY]` under `RETRACE_CANARY`),
and Phase B's flip was taken on that measurement rather than on confidence. **Every path carried its
own positive control, taken first, on that same path** — the M29 lesson applied before the fact:

| path | control (taken first) | measurement |
|---|---|---|
| in-process `cargo test` | 1 `[M30 CANARY]` line from the caught-half test | — |
| CLI `record-dyn` | **27** `[M28 BANDSHRINK]` lines off `/bin/ps` | `/bin/ps` 0, `jq --version` 0, real CPython 0, `python3` launcher 0 (**PARTIAL**) |
| Apple sweep | **392** `[M28 BANDSHRINK]` lines from **54** distinct guests | **0** canary lines; `TALLY pass=46 fail=8 skip=0`, unmoved |

```sh
RETRACE_CANARY=1 cargo test -p retrace-box --test truncguard -- --test-threads=1 --nocapture
RETRACE_BANDSHRINK=1 cargo run -q -p retrace -- record-dyn /bin/ps -o /tmp/ps.bin   # the control
RETRACE_CANARY=1    cargo run -q -p retrace -- record-dyn /bin/ps -o /tmp/ps.bin
RETRACE_BANDSHRINK=1 tools/apple-sweep.sh   # the control
RETRACE_CANARY=1     tools/apple-sweep.sh
```

`[M28 BANDSHRINK]` is the right control because it leaves `forward_and_diff` by the same `eprintln!`,
on the same stream, in the same process as the canary line, and is already measured non-zero — so a
zero canary count from a path that carries BANDSHRINK is a fact about the guests rather than about the
plumbing.

**`/opt/homebrew/bin/python3` is recorded as PARTIAL, not as a clean zero.** That path is the Homebrew
launcher shim, which ran dyld, the whole libSystem init and a long run of forwarded syscalls and then
died on its own `posix_spawn` — exec-in-place is unmodelled, the gap `cpython_e2e` pins. Its zero
covers the launcher only, so the real interpreter was measured separately as an extra row and ran to
completion. Two zeros that could have been reported as one.

**Task 6 took B-FLIP.** The filled branch's question became the assert, reusing the *same* `disturbed`
value the counter and the `RETRACE_CANARY` line read, so those three are three views of one decision
and cannot drift apart. `canary_overran` — the round-1 reconstruction — had no production caller left
and was deleted with its unit test.

**A consequence of the ordering, stated because the plan expected the opposite.** The assert fires
inside the check loop; the restore is a separate pass *after* it. So an aborting recording leaves the
canary in guest memory. That is deliberate — a panicking recorder produces no usable trace — but the
plan's own self-review had predicted the reverse ("the assert fires after the restore, so the abort
path leaves guest memory clean"), and it is wrong in the log rather than quietly right in the code.

The restore's shape was itself forced by two measured defects, neither reasoned about in advance.
Restoring *inside* the check loop manufactured disturbances: two arguments of one call can hold the
same value (`open`'s x0 and a stale x3; `stat64`'s x0 and x2, both seen on `jq`), and the first
entry's restore erased the canary the second was about to check — 2 phantom disturbances on one small
`jq` run, which would have gone straight into this milestone's headline measurement and then into a
panic on a correct recording. And restoring inside `if !err` skipped the error path entirely: **36
error-path restores** on that same `jq` run, 64 bytes each, left permanently in guest memory. Since
replay never calls `forward_and_diff`, a leaked canary is bytes the recording has and the replay does
not — a final full-memory divergence. Both are now regression tests
(`a_duplicated_pointer_argument_does_not_manufacture_a_disturbance`,
`a_failing_syscall_still_restores_the_canary`), each verified able to fail.

### The lesson: this milestone exists because instruments could not fire, and it grew three more

M27, M28 and M29 each shipped an instrument that could not fire, and M30 exists to fix the first of
them. During M30, **three more** appeared, and every one was caught by a positive control or a
mutation — **never by a test going red**:

1. **The sweep's `[M30 CANARY]` channel — the same file and the same line that defeated M29.**
   `tools/apple-sweep.sh` redirects each recording's stderr to a scratch file and only ever surfaced
   `[M29 DEREFLEN` back out of it, deleting the rest on its `EXIT` trap. Phase A's headline sweep
   measurement **would have returned 0 for every possible guest behaviour**. Not an analogue of the
   M29 defect: the same file, one milestone later. Fixed by two surfacing blocks beside the M29 one,
   and the `[M28 BANDSHRINK]` block is there because it is the control that makes the zero mean
   something.
2. **A `CAP` derived from the wrong syscall's reply size.** The plan measured `sizeof(struct stat)` as
   144 bytes via libc's `fstat()` — which routes to trap **339**. The guest issues the raw trap
   **189**, whose reply is **120** bytes. M28's own comment carried the 144 and its conclusion
   survived unchanged (a 64-byte window is genuinely overrun either way), but the number was wrong and
   Task 2's cap could not have been derived from it. Raw-versus-libc argument and reply shape is
   invisible in a man page — the same class M29 hit on `sysctlbyname`'s argument indices.
3. **The headline regression test itself.** `bigwrite`'s guest originally wrote its 128 KiB to
   **stdout**, and `retrace_arch::is_console_write` makes fd 0/1/2 mirrored and faked in
   `retrace-core` — read out of guest memory, never forwarded. The guest never reached
   `forward_and_diff` at all, so with the fix reverted the test stayed **green**. The test written to
   guard the milestone's most dangerous finding was vacuous, and only the mutation step said so.

The mutation that proves the flip is wired: `let fill_canary = false;` turns both zeros-over-zeros
tests red at their sentinels (`NOT-THE-CANARY: fstat put zeros over a zeroed band and nothing fired`),
while M28's change-visible control stays green — the mutation removes the new capability and nothing
else. What remains unguarded, and is stated rather than glossed: the *presence* of the `fill_canary`
gate on the detector is pinned by no test. Reverting it (running the canary question on both branches)
left the whole of `retrace-box` green, because reaching the unfilled miss needs a `reads_guest_buffer`
syscall that *also* writes past its own diff window with a byte equal to `canary_byte` at that offset,
and nothing in any corpus does the first two together.

### The gate

**549 passed / 0 failed / 2 ignored across 118 test binaries**, every chunk `EXIT=0`; clippy clean
over `--workspace --all-targets` with `-D warnings`. Reconciled against the M29 fast-follow's
538 / 0 / 2 over 116 **file-by-file rather than by sum**:

| file | M29 | M30 | delta |
|---|---|---|---|
| `crates/retrace-arch/src/lib.rs` | 29 | 30 | **+1** (`the_guest_buffer_readers_are_pinned_by_number`) |
| `crates/retrace-box/tests/canary.rs` | 0 | 5 | **+5, and a NEW binary** |
| `crates/retrace-box/tests/truncguard.rs` | 15 | 19 | **+4** — five added across Tasks 2 and 4 (15→16→17→19→20), minus `the_filled_detector_is_a_strict_subset_of_the_unfilled_one`, deleted at Task 6 with `canary_overran` |
| `crates/retrace/tests/bigwrite_e2e.rs` | 0 | 1 | **+1, and a NEW binary** |

Every other file unchanged, `--bins` **11 → 11**, and the two new test targets are what moves the
binary count 116 → 118. The count closes at both ends: the tree holds **551** `#[test]` = 549 running
+ 2 ignored. The ignored gates are unchanged at two — `stackoverflow_rust_e2e` (the M21 signal-model
wall) and `cache_symbol_e2e` (the M19 shared-cache symbol wall). M30 parked nothing new and un-parked
nothing.

One flake to expect rather than mistake for a red: **`/bin/ps` oscillates** between a clean run, a
guest `BRK` in record (`EC=0x3c ISS=0x1 FSC=0x1 pc=0x18032574c`, recorder exit 4) and an abort, **with
the environment variables unset**, plainly tracking the live process table `ps` enumerates. An
interleaved control run — two signed binaries, from HEAD and from the flip, alternated on `/bin/ps`
under identical load — had HEAD fail 2 of 6 and the flip 0 of 6. It is orthogonal to M30 and recorded
here rather than smoothed over.

### What stays owed

* **A per-argument direction table**, so `sendfile`'s in-out `off_t *` and `mach_msg2`'s receive
  buffer regain destination-side canary coverage. This is the largest single thing M30 gives up.
* **`ioctl`**, and any unlisted reader syscall: `reads_guest_buffer` is enumeration, and only
  enumeration prevents the class it guards.
* **The `fill_canary` gate's presence** is unguarded by any test, and cannot be guarded without a
  guest that does not exist.
* **A test *named* for the fill honouring the shrunk band.** This one is a documentation gap, not a
  coverage hole, and the distinction is worth getting right because the first draft of this entry got
  it wrong. Task 3 moved `band_not_covered` before the fill so a canary cannot land inside another
  argument's diff window — where the post-image capture would record retrace's own bytes into the
  trace as a kernel write, and replay would apply them. That is unreachable today **by
  construction**: `band_not_covered` truncates a band at the first overlapping window's *start* and
  truncates rather than differences, so it is strictly conservative, and the fill consumes the shrunk
  `*band`, never the raw `pre_band.len()`.
  A future edit reverting the fill to `pre_band.len()` would be caught — and **reliably**, not
  probably. A shrink event fires only when some other window genuinely intersects the raw band
  (`os < end && oe > start`), so every shrink implies a raw-band byte inside another window; with the
  fill reverted that byte is canaried and captured deterministically, modulo the 1/256 coincidence.
  `/bin/ps` produces 29–31 shrinks per recording and the 54-guest sweep 392, and
  `sysbin_e2e::ps_records_and_replays` both records and replays and asserts on divergence. So the
  invariant is guarded; what it lacks is a test that says so in its own name, which is why a reader
  auditing this function would not find it.
* **A named success-path restore test.** Converting the caught-half to `should_panic` cost its restore
  assertion; the error path is still covered directly, and the success path only indirectly, by every
  record/replay e2e in the workspace (a leaked canary is a final full-memory divergence).
* **The two holes M27 and M28 left**, untouched here: `Box_::diff_memory`'s `.min(avail)` clamp on the
  replay side, and the `if !err` gate that skips write capture — and band evaluation — on a failing
  syscall.
* **Widening the band itself** (sampling the whole remaining backing under a fixed byte budget rather
  than one contiguous 64-byte run) stays unblocked and unattempted. M28's suppression count is the
  warning for whoever takes it up: a naive wider sample is suppressed more often, not less.

---

## Status: M31-checkpointparity — a forcing function for field N+1, and a seven that was always a six

M24 built the `load`↔`restore` parity guard and named its own successor in the same breath:
*"`from_checkpoint` has no parity guard at all — this is the successor milestone."* M31 is that
milestone. `crates/retrace-box/tests/checkpointparity.rs` drives a `Box_` to a **mid-run** landmark,
checkpoints it, rebuilds a second box from that checkpoint, and diffs the two — `restoreparity.rs`'s
shape, applied to the replay path that restores the most state and runs where nothing sits at a
default. It carries the same written obligation: a new field must be compared there and **equal**,
asserted as **deliberately reset** with the replay-side mechanism that re-establishes it cited by
file and line, or **named as knowingly excluded** citing the comment that documents the exclusion.
There is no fourth option that is safe. Two test-only accessors were added to make the guard able to
see what it asserts: `Box_::dbg_debug_state()` and `Box_::dbg_tlbi_stub_ready()`.

**The premise M24 handed down was too strong, and this milestone corrected it in the spec rather than
inheriting it.** "No parity guard at all" is true of a *structural* guard and false of coverage.
`tests/checkpoint.rs::checkpoint_round_trip_is_lossless_mid_run` already covered registers, FP/SIMD,
the nine `dbg_internal_state` scalars and full memory, mid-run, with non-default state staged; six
point tests (`pacposture.rs`, `sigcheckpoint.rs`, `threads.rs`, `protnone.rs`, `tlbi.rs`,
`fdtable.rs`) each covered exactly one field — per-field carriage tests, written alongside or after
the field each covers. They are **not** one test per historical bug, and this section will not say
so: M7's `pac_enabled` and M13's `noaccess` were never dropped by `from_checkpoint` (M13's was
planned staging, `2ebbb7b`, not a review-caught bug), while `fdtable.rs` — missing from the list this
sentence originally carried — does cover a counted instance. Reading *same reason* as *same
instance* is the precise slip this milestone exists to correct, so it is not reproduced here. What was
missing was never coverage of the past. **It was a forcing function for the future**: one diff that
sees every field at once, and an obligation that makes field N+1 somebody's problem *before* it ships
rather than after it breaks. Stating that narrower claim is worth more than inheriting the wider one,
because a milestone that advertises a gap it does not have cannot be checked.

### The judgement a mid-run guard has to make

Two things legitimately differ across `from_checkpoint`, and both are **asserted** rather than
excused.

**The debugger four** — `bps_armed`, `wps_armed`, `watch_ranges`, `syscall_watch_hit` — are not
carried in `BoxState` and come back at their defaults. That is correct: the debugger owns the watch
list and re-arms from its own stored copy after every seek (`crates/retrace/src/debug.rs:608`, `:641`
and `:761`, each calling `ReplaySession::arm_watchpoints(&ws)`), so a box that restored them would be
a second authority for the same state. The guard asserts the reset positively instead of stripping
the fields in a `normalise()`, because **stripping excuses a difference invisibly and goes on passing
when the field stops being reset**, whereas asserting states what is supposed to happen and fails if
it stops happening. That assertion is only worth something if something was armed: the rich fixture
arms **both** a watchpoint and a hardware breakpoint before capture, so both halves of the reset
check observe a real reset rather than a field that was already at its default.

**The current thread's table entry is stale in a live box** — only `switch_to_thread` refreshes it —
and `Box_::checkpoint` folds the live vCPU into it before carrying it. So a restored table
legitimately holds *more* current state than the live box's own table, and comparing the two directly
asserts something that is false by design. **The guard's first run failed for exactly that reason**,
which is how the judgement got made rather than assumed. The current thread is therefore compared
against the live box's `save_ctx()` — making the fold itself the thing asserted — while every
non-current entry is compared against the **live** box rather than merely against the captured state.
Without that second rule the guard would be clone-fidelity only: a `checkpoint()` that folded into
the wrong index, or corrupted another thread's context, would make restored == captured and pass.

### The result: no asymmetry found, bounded by what the fixtures reach

On its first clean run the diff found **no `from_checkpoint` asymmetry**. `from_checkpoint`
reproduced every field both tiers reach — a two-thread table, fd slots (`Open` and `Closed`, the
latter distinct from `Free`), the signal table, all three pthread/workqueue scalars, a `PROT_NONE`
extent, the cache pager, a bootstrap port, an armed breakpoint, an armed watchpoint and
`tpidrro_el0` — and reset the debugger four. Nothing was carried, nothing was asserted as a
deliberate reset beyond the two judgements above, and nothing was parked.

That is a finding about the code's current state, not an absence of effort, and it is written into
the test's own module header in those words so that a reader meeting a green guard does not read it
as a guard that did not look. The bound is stated in the same place: **it is exactly what the
fixtures reach**, and the two tiers exist because a field left at its default is compared and the
comparison proves nothing — `Default == Default` passes for a reason unrelated to the assertion's
name. The static tier runs `HELLO` to its first syscall and moves only `pc`/`elr`/`spsr`; the rich
tier stages a non-default value into every field it can reach through `Box_`'s own public methods and
**asserts each one non-default before capturing**. The preconditions are what separate a guard that
agrees from a guard that cannot see.

### Two positive controls, because a guard nobody has watched fail is a guard nobody knows is wired up

M28's `let band = 0;` passed a 523-test gate before its own positive control existed. So this guard
was made to fail twice, on purpose, and both mutations are recorded on the test itself.

| control | mutation in `from_checkpoint` | result |
|---|---|---|
| 1 | `sigtable: state.sigtable.clone()` → `SigTable::default()` | rich tier **RED** at `rich: signal dispositions`; static tier green |
| 2 | `threads: state.threads.clone()` → a clone that zeroes every **non-current** thread's `ctx`, leaving thread count and `current` untouched | rich tier **RED** at `rich: the restored thread table must reproduce the CAPTURED table exactly`; static tier **GREEN** |

**Control 2's asymmetry is the point, not a side effect.** With one thread `cur == 0`, there is no
non-current entry to corrupt, so the mutation is invisible — on Task 2's single-thread fixture this
exact bug would have passed. The second thread is what gives the guard reach, and the two threads
carry deliberately *different* signal masks so that an index-swap in `checkpoint()`'s fold pass cannot
hide behind two near-identical contexts. A cruder first attempt (replacing the whole table with
`ThreadTable::new(ThreadCtx::zeroed())`) was discarded because it also changes the thread **count**
and so fails on both tiers, isolating nothing.

Both mutations were reverted and the revert verified (`git status --porcelain=v1` empty,
`grep -rn 'MUTATION' crates/` exit 1) before the suite was re-run clean.

### The enumeration, and a count that was always one too high

Five sites in the tree were carrying ordinals for this class, and they were counting **three
different quantities**: instances of the class on the `from_checkpoint` path (`restoreparity.rs`),
fields carried in `BoxState` (the field comments, `fdtable.rs`, `sigcheckpoint.rs`), and sites inside
`from_checkpoint` that stopped being re-derived from a constant. They were never in conflict; one
**cross-reference** between them was false. `restoreparity.rs` claimed its five were the ones "the
`BoxState` field comments enumerate by name" — wrong in both directions, since M9 t3 is not a
`BoxState` field at all (its fix was the opposite remedy: `from_checkpoint` **derives**
`tlbi_stub_ready` from the restored backings, commit `70629c4`) and the field comments additionally
name M7 t6, M8, M13 and M23 t1, none of which is an instance. The five ordinals are retired and every
site now points at one authority — the `M24-restoreaudit` section of this log.

**And that authority is one too high.** M24 named M18's `wq_thread_pc` as an instance. It is not:

- `e93f8dc` ("M18 t4: guest_bsdthread_register — the guest's registration is the guest's") adds the
  `Box_` field, the `pub BoxState` field, the `checkpoint()` carry **and** the `from_checkpoint`
  restore in one commit. There was never a window in which `from_checkpoint` dropped it.
- All four M18 status-log sections contain **zero** mentions of `from_checkpoint` or `BoxState`, and
  M18 files its own recurring bug under a different class entirely ("the guest's X is retrace's",
  with M10 and M11 as its siblings). A search of `docs/`, `.superpowers/` and every commit body
  (`--grep`, `-S`, `--all`) for `wq_thread_pc` turns up nothing recording such a drop.

**The provenance is the part worth keeping**, because it is what stops the next milestone re-deriving
the same seven from the same source. `docs/superpowers/specs/2026-08-31-retrace-m24-restoreaudit-design.md:32`
reads, in full, "**M18** — `wq_thread_pc`, same reason." — uncited; and line 127 of that same spec
names its source as the `BoxState` field comments, where `wq_thread_pc`'s comment says it is "carried
for the same reason as `thread_start_pc` immediately above". **Same *reason* was read as same
*instance*.** So the corrected counts are **four** on the `from_checkpoint` path (M9 t3, M10, M11,
M14) and **six** for the whole class (those four plus M21 and M23 t1 on `restore`). M24's section
stands as written; this one is its forward pointer, which is what the append-only rule is for.

**One candidate was raised and withdrawn**, recorded so it is not raised a third time. M13's
`noaccess` (introduced `2ebbb7b`, M13 t5; carried in `BoxState` at `86d3b30`, M13 t6) has the same
*shape* as M14's counted instance but is not the same *kind*: `2ebbb7b` documented the gap in code as
planned staging — "BoxState has no `noaccess` field yet (that's M13 t6's job) … safe today because
nothing calls `protect_none` yet" — whereas M14's arrived in `3e3b023`, "t7 **fix round 1**", i.e.
review-caught. Planned staging inside one milestone is not an instance of a bug class. (A Task 5
report cited `f47936f` for `noaccess`'s introduction; git shows that commit only mentions the field
in a `subtract_range` doc comment, and `2ebbb7b` is where the field is born.)

### The gate

**552 passed / 0 failed / 2 ignored across 119 test binaries**, every chunk `EXIT=0`; clippy clean
over `--workspace --all-targets` with `-D warnings`.

| chunk | passed | failed | ignored | binaries |
|---|---|---|---|---|
| workspace minus `retrace-box`/`retrace` | 137 | 0 | 0 | 23 |
| `retrace-box`, **whole package** | 266 | 0 | 0 | 36 |
| `retrace --bins` | 11 | 0 | 0 | 1 |
| e2e group 1 | 31 | 0 | 0 | 11 |
| e2e group 2 | 16 | 0 | 0 | 11 |
| e2e group 3 | 16 | 0 | 0 | 11 |
| e2e group 4 | 25 | 0 | 1 | 11 |
| e2e group 5 | 37 | 0 | 1 | 11 |
| e2e group 6 | 13 | 0 | 0 | 4 |
| **total** | **552** | **0** | **2** | **119** |

`retrace-box` ran as a **whole package** so its `Doc-tests` target was not silently dropped (M24's
lesson, standing practice since), and `retrace --bins` ran so the 11 unit tests that live only in the
binary were not silently lost. Reconciled against M30's 549 / 0 / 2 over 118 **file-by-file rather
than by sum**: the entire delta is `crates/retrace-box/tests/checkpointparity.rs`, 0 → **3** tests
and a **new binary**; every other file unchanged, `--bins` 11 → 11. The tree holds **554** `#[test]`
= 552 running + 2 ignored, against M30's 551 = 549 + 2. **The count was predicted from source before
the gate ran and the run matched it exactly**, which is the only way a chunked gate can tell a
missing chunk from a missing test. The two ignored gates are unchanged — `stackoverflow_rust_e2e`
(the M21 signal-model wall) and `cache_symbol_e2e` (the M19 shared-cache symbol wall). M31 parked
nothing new and un-parked nothing; it adds no capability, bumps no magic, and changes no recorded
bytes.

### What stays owed

* **Construction, not evolution.** The guard compares two boxes at one landmark and says nothing
  about their behaviour afterwards. `crates/retrace/tests/checkpoint_seek.rs` is that axis and this
  milestone did not rebuild it.
* **Symmetric-but-wrong stays invisible.** Two boxes wrong in the *same* way are invisible to any
  test that only diffs them against each other — inherited from M24 unchanged, and unchanged for the
  same structural reason the determinism oracle cannot see the class either.
* **Eight fields are still `Default == Default` in the rich fixture**, so the guard's comparison of
  them proves nothing: `synthetic_tsc`, `last_far`, `cache_refault_ipa`, `cache_refault_count`,
  `pac_enabled`, `fall_throughs`, `tpidr_el0`, `syscall_watch_hit`. Each needs a guest that executes
  the instruction or takes the fault rather than a setter — there is no public "bump the timebase" or
  "stage a fault" method — and `pac_enabled` is deliberately not staged because forcing it on over a
  non-arm64e guest would assert a posture the binary never claims (M7's finding).
* **`stack_top` / `stack_size` are a different class, and conflating them with the eight is the
  mistake to avoid.** They are not absent defaults but always-identical non-trivial constants
  (`STACK_TOP_IPA` / `GRANULE`) that nothing in `Box_`'s public interface moves post-load. The
  comparison therefore cannot distinguish a genuine carry-through of `state.stack_top` /
  `state.stack_size` from a hardcoded recomputation of the same constant. The test file documents
  both classes separately for this reason.
* **Control 2 is recorded in prose, not as a literal snippet.** M28's precedent (`let band = 0;`) is
  a mutation a reader can paste back in; the non-current-`ctx`-zeroing mutation is described in the
  guard's doc comment in words, which is where a reader auditing it will look, and is therefore less
  directly re-runnable than the control it is modelled on.
* **The memory comparison is over the map, never the contents.** `from_checkpoint` populates every
  backing's bytes with a `memcpy` straight from `state.mem`, so a byte-for-byte compare here would be
  near-tautological — but `restoreparity.rs` *does* byte-compare the EL1 vector table, so a reader
  must not assume the two files do the same thing.

## Status: M32-dirtable — the table said not to build the table

M32 set out to make the M30 canary decision belong to the **argument** rather than the syscall.
`retrace_arch::reads_guest_buffer` is a whole-syscall predicate: a syscall that reads *any* guest
buffer has *every* argument's guard band withheld from the canary fill, and two of the excluded
calls have a genuine kernel-**written** argument — `sendfile`'s in-out `off_t *`, and `mach_msg2`'s
receive buffer. Recovering that coverage needed a per-argument notion (`is_known_dest_arg`), six
tasks were planned for it, and Task 1 was the measurement that had to come first: *does a guard band
on `mach_msg2`'s buffer land past `send_size`, the boundary the kernel reads to?*

**The measurement said the entry would ship inert, so it was not built.** Tasks 2–6 were dropped and
the milestone closes as a measurement. That is the honest-gate discipline applied one level up: a
milestone that measures its own deliverable empty and says so is worth more than one that ships the
entry anyway and lets a future reader assume it does something.

### What was measured

Task 1, after two review-driven fix rounds, walked **35 real `mach_msg2` landmarks** across
`hello_dyn`, `jq` and CPython — all three present on the machine, all three walked, none skipped.
Each landmark was classified by calling the production `machmsg::route()`, never a hand-copied
allow-list.

| | |
|---|---|
| landmarks measured | 35 |
| **governed** by this milestone (`Route::Forward`) | **13** |
| maximum `avail` among governed calls | **24,672 bytes** |
| `window_cap` — the threshold below which no band exists at all | **65,536** |
| governed calls producing a nonzero band | **zero** |

A band exists only where `avail > window_cap`. No governed call comes within 40 KiB of that. So the
entry §5a of the spec exists to add would have changed nothing observable on the day it shipped —
by measurement, not by argument. `sendfile`, the only other member of `reads_guest_buffer` with a
kernel-written argument, has **no guest**: a `grep` across `crates/` finds it in the arch constant
table and the box's own implementation and nowhere else. Both candidates for this milestone's
coverage deliverable are dead, and the deliverable is empty.

### Why the 13 are shallow — one fact, not thirteen coincidences

All five ids in `FORWARD_ALLOWLIST` (200 `host_info`, 206 `host_get_clock_service`, 3418
`semaphore_create`, 3405 `task_info`, 412 `host_get_special_port`) are MIG-generated kernel-RPC
stubs, and a MIG stub builds `union { Request; Reply; } Mess;` as a **stack local** and passes
`&Mess`. `avail` is the distance from a buffer to the end of its backing, so for a governed call
`avail` **is** the stack depth measured from that stack's top. The geometry holds for all three
stacks retrace produces: the main stack is 256 KiB backed with the buffer below its top
(`crates/retrace-box/src/lib.rs:94-95`), a pthread stack is the guest's own mmap and what
libpthread hands `bsdthread_create` is the stack **TOP**, with SP starting there and growing down
(`crates/retrace-box/src/lib.rs:4681-4683`), and a workqueue worker's stack
puts the struct at the top and grows down (`crates/retrace-box/src/lib.rs:4426-4441`).

The same structure explains the corpus's one outlier. Exactly one landmark carried a nonzero band —
msgh_id `0x400000cf` at ~4.1 MB `avail`, band 64 — and it is a **libxpc message-queue send with a
heap buffer**, the only class in the corpus where `avail` has nothing to do with stack depth, and
precisely the class `route()` refuses as `Route::RefuseMqSend`. `forward_and_diff` never runs for
it. Counted naively it would have produced a confident, wrong "NOT inert" headline; the implementer
caught it by classifying through the real router before drawing the conclusion.

### What stays open in the finding itself: depth

**Nothing bounds the stack depth at which a governed id can fire.** All 13 measured calls are
process-initialisation calls, which are shallow by construction, so the population is **biased** —
the sample says less than 13 rows across three guests looks like it says. A `semaphore_create` from
a dispatch semaphore built deep inside a call chain, or a `host_info` behind a `sysconf`, are
ordinary things for a program to do, and 64 KiB of frames sits well inside a 256 KiB stack. The
finding is mechanistically explained and unlikely to reverse. It is **not proven**, and this section
will not say otherwise.

That is why the closing measurement now **asserts** rather than only reporting. Through the review
round the corpus walk printed its conclusion with `eprintln!` and was green by construction: the
number the milestone closed on could become false without anything going red, while spec §9's claim
stayed load-bearing for the successor's scoping with nothing watching it.
`every_real_mach_msg2_in_the_corpus_is_checked_for_a_nonzero_band` now asserts
`governed_max_avail < PTR_WINDOW_CAP` — deliberately one step stricter than "no governed band was
nonzero", since `avail == window_cap` exactly still yields band 0, so the tripwire fires before the
finding is actually overturned. It reds on an *improvement* (a fixture that finally reaches a deep
governed call), and that red is the correct signal: it says the closed milestone's premise moved.
The discipline is M28's "prove the instrument can fire" and M29's "gate the channel that reports it",
applied to a conclusion instead of an instrument — including the part M28 taught the hard way: the
assertion was **verified able to fail** before it was trusted, by temporarily lowering its threshold
to 24,000 against the corpus's real 24,672 and watching the test go red with its own message. The
mutation was reverted, the revert verified with an empty `git status --porcelain=v1`, and the suite
re-run green. The control is recorded on the test itself, which is where its next reader will be.

### The finding that replaces the deliverable: four views of one question

The per-argument defect is real, but it is not a defect in `reads_guest_buffer`. It is a defect in
the **schema**. Four functions answer one question — *what does this syscall do with each of its
arguments* — in four incompatible shapes, and two of them threw away the argument index the other
two keep:

| function | shape | keyed by |
|---|---|---|
| `fd_operands` | `&'static [usize]` | argument indices |
| `dest_buffer` | `Option<(usize, DestLen)>` | argument index + length source |
| `reads_guest_buffer` | `bool` | whole syscall |
| `writes_via_nested_pointer` | `bool` | whole syscall |

M32's entry would have added a **fifth** view, to recover per-argument information `dest_buffer`
already stores eight lines away. That is the M26–M32 lineage's recurring shape: each milestone adds
a view and reconciles it against the others, and each new view is a fresh chance to ship inert.

**The successor is therefore a unification, not another view**: one
`arg_kinds(num) -> &'static [ArgKind]` table from which all four current functions derive, proven by
an equivalence sweep over every syscall number. M32's per-argument direction then stops being a
table and becomes a field. A semantic-coverage / mutation-testing framework was designed to police
inert entries generally and then **abandoned deliberately**: it would have policed a symptom forever,
needed its own positive control, and cost corpus runs indefinitely, while the cause is that the
tables do not express what the code needs. Unification removes the symptom's source. If inert
entries keep appearing *after* unification, that framework becomes worth revisiting, and this
paragraph is the pointer back to it.

### What Task 1 landed, and it stands

- `Box_::band_len(avail, win)` hoisted out of `forward_and_diff` into a `pub`, pure function beside
  `band_not_covered`, so exactly one copy of `GUARD_BAND.min(avail - win)` exists and a test can
  call production rather than re-derive it. The bare, panicking subtraction was **preserved, not
  softened to `saturating_sub`** — hoisting an invariant must not weaken it.
- The structural proof (`crates/retrace-box/tests/machmsgband.rs`) that whenever a band exists it
  begins at least `window_cap` (65,536) bytes into the buffer, while every `mach_msg2` call's
  `send_size` is bounded at 4,096 — so a band can never land in a kernel-read region, at any
  `avail`. Each step is asserted rather than stated, the sweep spans 65,535 / 65,536 / 65,537, and
  all four production `Box_` constructors are checked against a live instance rather than cited.
- That proof's premise is now **asserted as a relation between its two constants**, at compile time:
  `const _: () = assert!(machmsg::SEND_SIZE_MAX < retrace_box::PTR_WINDOW_CAP, …)` at module scope in
  `crates/retrace-core/tests/machmsgband_dyn.rs` — the only place in the repo where both operands are
  visible, since `retrace-core` owns the ceiling and depends on the crate owning the cap, and
  `retrace-box` can never see the second one. Module scope rather than a test body is deliberate:
  the corpus test skips `jq` and CPython when they are absent, and its own runtime tripwire sits at
  the end of that function, so on a machine without Homebrew nothing in that file's bodies would
  evaluate the premise at all. A `const _` is checked whenever the crate compiles.
  **It took two rounds, and the first one is this milestone's own failure class recurring inside its
  own correction — a third time, caught by review rather than by any mechanism.** Round one turned
  the ceiling into a shared `pub const` and had the test import it rather than redefine it. That was
  a real improvement to the *per-landmark* checks, which now compare real captured sends against the
  real production bound. It was **not** drift detection, and the round claimed it was: every use is
  `send_size <= SEND_SIZE_MAX`, which a **widening** makes strictly more permissive, so setting the
  bound to `0x20000` left everything green while the proof's conclusion turned false — and four
  sites, one of them a failure message a future reader would meet mid-debugging, said it reds. The
  mirror constant `retrace-box`'s proof carried was **deleted rather than renamed**, because nothing
  compared it to the thing it mirrored: a second literal that nothing checks is what produced the
  finding in the first place. The compile-time assertion was verified able to fail before it was
  trusted (bound set to `0x20000`, crate stops compiling with its own message, reverted). The two
  files now split the proof honestly — `machmsgband.rs` proves in-crate that a band starts at
  `ipa + window_cap`; `machmsgband_dyn.rs` proves `window_cap` clears the read ceiling; neither
  needs the other's number.
- `dbg_window_len_for` returning `Option<usize>`, so "unmapped" and "zero-length window" stop
  collapsing into the same `0`.
- The 35-landmark corpus measurement itself, which is the evidence everything above rests on.

### The rule this milestone is worth remembering for

**Classify by calling the production router, never by a copy of its allow-list.** The same category
error appeared at two scales and was caught by two different mechanisms: Task 1's original fixture
measured msgh_id 4811, which `Route::ServiceVmMap` services and never forwards (caught by review, at
n=1); and the corpus walk initially risked counting the refused message-queue send above (caught by
the implementer, at corpus scale, by calling `route()` first). Both would have produced a confident
wrong headline about a milestone's central number. The generalisation holds beyond this milestone:
any test that decides "is this call on the path my change governs" is re-implementing a production
decision, and the copy is where the drift lives.

**And the class this milestone kept catching, it caught a third time inside its own correction.**
"Right conclusion, unmeasured supporting fact" — M20's name for it — was found twice during the
tasks, then once more in the fix wave dispatched to correct the second instance: the wave's own
`const` assertion carried a failure message stating that a *different file* would red if the
production bound moved, when nothing anywhere related the two constants. That is the worst place for
a wrong supporting fact, because a reader meets it at the moment they are debugging and least likely
to re-derive it. **Nothing mechanical caught any of the three.** All were caught by review, and this
one only because the reviewer ran `grep -rn SEND_SIZE_MAX crates/` instead of reading the claim. The
milestone has no instrument for this class and did not build one; the honest record is that its
detection rate here is a property of how the reviews were done, not of the tree.

And its process twin, learned expensively: **anything a successor must know belongs in
`docs/status-log.md`, never in the SDD workspace.** `.superpowers/` is gitignored scratch. Two
carried obligations lived only there and would have vanished at merge; the review's ledger triage is
what caught them, and they are in "What stays owed" below because of it.

### The gate

**556 passed / 0 failed / 2 ignored across 121 test binaries**, every chunk exit code **0** captured
before any pipe; clippy clean over `--workspace --all-targets` with `-D warnings`.

Reconciled against M31's 552 / 0 / 2 over 119 **file-by-file rather than by sum**: exactly two files
moved, `crates/retrace-box/tests/machmsgband.rs` 0 → **2** and
`crates/retrace-core/tests/machmsgband_dyn.rs` 0 → **2**, both new binaries; every other file
unchanged and `--bins` 11 → 11. That is +4 tests and +2 binaries, 119 → 121, fully accounted for.
The tree holds **558** `#[test]` = 556 running + 2 ignored, against M31's 554 = 552 + 2. **The
figure was derived twice and independently** — once predicted from the diff by the whole-branch
reviewer, once taken from the run by the controller — and the two agreed, which is the first time
this milestone's numbers were established from two directions. Both `retrace-box` and `retrace` ran
as **whole packages** rather than per-target, so neither `retrace-box`'s `Doc-tests` harness nor the
11 unit tests that live only in the `retrace` binary could be silently dropped — the two mouths of
the same trap, one loud and one silent.

The two ignored gates are unchanged: `stackoverflow_rust_e2e` (the M21 signal-model wall) and
`cache_symbol_e2e` (the M19 shared-cache symbol wall). M32 parked nothing new and un-parked nothing;
it adds no capability, bumps no magic, changes no recorded byte, and touches no code path that runs
during a recording.

### What stays owed

* **Carried to the reader-syscall enumeration milestone (M33 as charted): for each syscall newly
  added to `reads_guest_buffer`, check whether it has a destination argument whose backing can
  exceed 64 KiB.** That check is what would make the per-argument entry M32 measured inert become
  live, and it is a per-syscall question that only the milestone adding the syscalls can answer. It
  existed only in the SDD ledger until this section; that is why it is here.
* **Spec §6's Control 1 was reframed, and the reframing is a real weakening that must not be read
  as satisfied.** As written it said: revert `is_known_dest_arg` to return `false` and the new
  destination-side test must go RED. With no mechanism built, there is no such test and no such
  revert. The control that would have replaced it targets the **mechanism** (reverting the predicate
  reds a unit test proving `fills_band` consults the per-argument allow-list), **not restored
  coverage** — and even that was not built, because the mechanism was not built. A successor picking
  this up inherits an unexecuted control, not a discharged one.
* **The corpus is three fixtures, and its bias is known.** Every governed call measured is
  init-time. The cheapest way to change the *population* rather than merely the guest count is a
  repo-owned threaded/GCD fixture (`thread_rust`, or the guest behind `dispatch_e2e`), because
  libdispatch issues `semaphore_create` (3418) and the clock-service calls from **worker threads at
  runtime** rather than from the main thread at initialisation. It is repo-owned, so unlike `jq` and
  CPython it can never silently shrink the corpus to `hello_dyn` on another machine. `/bin/ps` was
  considered and rejected: its mach traffic is the same init-time set through the same main-stack
  geometry, so it would add shallow rows and gate time and settle nothing.
* **The single-triple test measures the replay side while the decision governs the record side.**
  That is argued benign at the top of `machmsgband_dyn.rs` — `Box_::restore` builds one backing per
  snapshot region, the stack is one such region, and `buf + len` was observed to land exactly on
  `DYN_STACK_TOP` — but it is an argument from one observed address, not a general proof that
  record-side and replay-side `avail` agree for every call.
* **The dropped tasks are still in the plan, behind a warning block, not deleted.** A reader of
  `docs/superpowers/plans/2026-09-09-retrace-m32-dirtable.md` sees six tasks of which five never
  ran. That is deliberate — the plan records what was intended and §9 records why it was dropped —
  but it means the plan cannot be read as a description of the tree.
* **Two process failures worth their own line, because neither is a code defect.** A background test
  run finished successfully in 185s and was never read back, costing **8.5 idle hours**; every run
  after it was foregrounded under a bounded timeout, and that is what caught the governed/ungoverned
  classification bug above. And the milestone's pace was mis-assessed against a seven-milestone
  overnight queue when the only available data — the M31 gate at ~40 minutes — contradicted it. Both
  are recorded here rather than in the scratch ledger for the same reason the two carried obligations
  are.

## Status: M33-readerenum — one table, five views, and a syscall that cannot be forwarded unclassified

M32 closed on a finding rather than a deliverable: four functions in `retrace-arch` answered one
question — *what does this syscall do with each of its arguments* — in four incompatible shapes,
two of them having thrown away the argument index the other two keep, and the successor it named
was a unification rather than a fifth view. M33 is that unification. `arg_kinds(num) ->
Option<&'static Shape>` is now the one table; `fd_operands`, `allocates_fd`, `dest_buffer`,
`writes_via_nested_pointer` and `reads_guest_buffer` are one-line views over it; a verbatim copy of
the five M32 tables is the fixture an equivalence sweep checks every view against in both
directions; and `forward_and_diff` refuses by name any syscall the table has no row for, before it
forwards anything. The rows came from a census of every syscall number the repo's four corpora
dispatch, and two of the spec's open classification questions were settled by measuring the kernel
rather than by applying a rule.

The milestone's own numbers: **22** `EXPECTED_DIFFS` entries, of which **16** are descriptors the
legacy `fd_operands` never translated (the M10 class, in the tree since M30) and **6** are
classifications the legacy tables had no opinion on; **108** distinct syscall numbers over **114**
recorded invocations; **129** numbers with a row (52 legacy + 77 census); the sweep tally **unmoved**
at `pass=46 fail=8 skip=0`; the gate **570 / 0 / 2 over 124**. `TRACE_MAGIC` did not move and no
recorded byte changed.

### What was measured

**The census (Task 1, 2026-09-12).** Every guest the repo can run, each invocation recorded under
`RETRACE_TRACE=1` with its `[trap] num=` lines collected. Three of the 59 built repo guests
(`crash`, `crashjmp`, `wildstore`) fault before their first syscall and contribute nothing, so 56
count. `/bin/ps`, listed on its own in the plan, is one of the 54 sweep binaries and is counted once.

| corpus | invocations | distinct numbers | only here |
|---|---|---|---|
| repo guests (`asm/*.s`, `c/*.c`, `rs/*.rs`) | 56 | 74 | 16: `-36`/`-33` (semaphores), 37, 52, 184, 189, 328, 329, 331, 360, 361, 367, 368, 478, 515, 516 — the thread and signal families only a purpose-built fixture exercises |
| `jq` (`--version`; `. file.json`) | 2 | 59 | 1: `pathconf` (191) |
| CPython (the interpreter; the launcher shim) | 2 | 75 | 1: `posix_spawn` (244) |
| the Apple sweep (all 54) | 54 | 90 | 14: `fchdir` (13), `sync` (36), `getppid` (39), `pipe` (42), `getlogin` (49), `execve` (59), `umask` (60), `getpgrp` (81), `dup2` (90), `getrusage` (117), `setrlimit` (195), `kqueue` (362), `writev_nocancel` (412), `task_read_for_pid` (539) |
| **all four** | **114** | **108** | 52 numbers appear in every corpus |

The census's first chunk was run once with a `sed | while read` pipe that shared fd 0 with the
guests inside the loop; `/bin/cat`, invoked with no argv, raced the loop's `read` for the same pipe
and the run silently produced 6 of 27 lines with no error. It was caught by cross-checking file
counts before trusting the aggregate, not by anything the run printed — the same trap M29's sweep
script documents and closes with a dedicated fd 3 — and re-run clean.

**`ioctl` (spec §4b).** Four distinct request codes in the whole census, decoded per
`sys/ioccom.h`: `FIODTYPE` (`0x4004667a`, 4 bytes OUT), `TIOCGWINSZ` (`0x40087468`, 8 OUT),
`TIOCGETA` (`0x40487413`, 72 OUT) and `DTRACEHIOC_ADDDOF` (`0x80086804`, `_IOW('h', 4,
user_addr_t)`, 8 bytes IN) — and the fourth's 8-byte parameter *is* a guest pointer, to a
`dof_ioctl_data_t` that xnu's `dtrace_ioctl_helper` follows with a nested `copyin`, issued by dyld
on nearly every dynamic guest to register DOF sections. Task 5 measured what that forwarded call
actually returns, with a temporary `eprintln!` (reverted, not committed) across ten guests — `jq`,
the CPython interpreter, `/bin/ps`, `ls`, `date`, `sh`, `zsh`, `sort`, `sleep`, `hostname`:

```
  17 ioctl req=0x40487413 ret=0x19 err=true      TIOCGETA    → ENOTTY (stdout is a file)
  17 ioctl req=0x4004667a ret=0x19 err=true      FIODTYPE    → ENOTTY
  10 ioctl req=0x80086804 ret=0xe err=true       DTRACEHIOC_ADDDOF → EFAULT, every guest
   5 ioctl req=0x4004667a ret=0x0 err=false      FIODTYPE on a real fd (CPython)
   2 ioctl req=0x40087468 ret=0x19 err=true      TIOCGWINSZ → ENOTTY
   1 ioctl req=0x40087468 ret=0x13 err=true      TIOCGWINSZ → ENODEV
```

`ret=0xe err=true` — EFAULT — on all ten. The nested copyin reads a guest address in retrace's
process and fails, so the `copyout` of generation ids downstream of it is unreachable, and no guest
byte is written. (`dtrace_dof_mode` defaults to `LAZY_ON` on macOS, so the early `KERN_SUCCESS`
return for `MODE_NEVER` is not what was seen; the errno proves the copyin ran.)

**`sysctl` `newp` (spec §4c).** Eight `num=202` rows in the census, seven with a non-null `newp`,
`newlen` 10..=32. Task 5 measured the MIB and the bytes behind `newp` (temporary print, reverted):
every one is MIB `{0, 3}` — libc's `name2oid` — with `newlen == strlen(name)`:
`security.mac.lockdown_mode_state` (32), `kern.bootargs` (13), `kern.osproductversion` (21, twice),
`kern.iossupportversion` (22), `kern.osvariant_status` (21) from `jq`; `hw.pagesize` (11) and
`hw.memsize` (10) from the sweep guests. The cited bound is xnu `kern_newsysctl.c`
`sysctl_sysctl_name2oid`: `newlen >= MAXPATHLEN → ENAMETOOLONG`. `sysctlbyname` (274) is dispatched
by no corpus guest.

**`map_with_linking_np` `link_info_size`**, measured because the only kernel cap is 64 MiB
(`MWL_MAX_LINK_INFO_SIZE`, "just a guess for now"): 11 calls, `region_count` 1 or 2, sizes
80..=2920 bytes.

**The sweep, before and after.** Baseline `TALLY pass=46 fail=8 skip=0` (M29–M32). Re-run at Task 6
on the fresh build with every row landed, in two halves of 27 (the script copied to the scratchpad
with its `LIST` pointed at each half; its fd-3 loop left as written), tallies summed by hand:
`pass=25 fail=2` + `pass=21 fail=6` = **`pass=46 fail=8 skip=0`**. The same eight by name and
reason: `csh`, `tcsh` (`recorder panicked` — re-recorded by hand to read the text: still `dup2 is
not modelled by the M10 fd table`, no `M33:` line); `automationmodetool`, `desdp`, `dyld_info`,
`flex`, `/bin/launchctl` (`replay diverged`); `/usr/bin/yes` (timed out). `dddiagnose` on a pass.
**Nothing moved.** The four binaries whose traps gained a load-bearing classification were then
recorded by hand and their traces read back, because the sweep's PASS says nothing about what a
guest received:

| binary | trap | what the guest got at M33 | why the sweep could not move |
|---|---|---|---|
| `/bin/ed` | `writev_nocancel` (412) | `fd=2`, `ret=0xe err=true` (EFAULT) | the fd is a console fd, which `translate_fds` maps to itself — identical to the raw forward before M33; the EFAULT is the untranslated nested `iov_base`, pre-existing, and how `ed`'s stderr message is lost |
| `/bin/ls` | `fchdir` (13) | `fd=4 → ret=0 err=false`, twice | now the guest's own directory; before M33 retrace's raw fd 4. Deterministic either way, so record and replay agreed both before and after |
| `/bin/wait4path` | `kqueue` (362) | `ret=4 err=false`, bound to a guest slot | exits at its usage message before using it |
| `/bin/zsh` | `pipe` (42) | `ret=0x12 err=false` | the host read-end in `x0`, the guest's own stale `x1` — `host_svc` captures `x0` and the carry only; `Ret::FdPair` is documentation of exactly this |

**And one thing the sweep cannot see, found while reading those traces.** `/bin/ls` PASSes while
its recorded stdout is `ls: .: Bad file descriptor`. Its `fstatat64(AT_FDCWD, ".", …)` returns
EBADF twice; `/bin/ed` issues one and gets the same. Every one arrives with `x0 = 0xfffffffe` — the
guest passes `-2` as a 32-bit `int` in `w0` — and `translate_fds`'s sentinel check is
`(v as i64) < 0`, which that value fails, so `AT_FDCWD` is looked up as a descriptor and rejected.
The `fdxlat` test for the sentinel passes `AT_FDCWD as u64`, the 64-bit sign-extended form, which
no observed guest produces. Present since M10 t3 (`e67dd65`, 2026-08-04). See Ruling 10.

### What was found

1. **Sixteen descriptors the legacy `fd_operands` never translated** — the M10 class, in the tree
   since M30 tabled the reader family from prototypes without asking which position held a
   descriptor. The equivalence sweep's first red named exactly these sixteen pairs and no other
   view: `pwrite` (154), `pwrite_nocancel` (415), `writev` (121), `writev_nocancel` (412), `pwritev`
   (541), `sendto_nocancel` (413), `sendmsg` (28), `sendmsg_nocancel` (402), `sendmsg_x` (481),
   `sendfile` (337, two descriptors), and the six the M27 assert refuses before translation could
   run — `readv` (120), `readv_nocancel` (411), `recvmsg` (27), `recvmsg_nocancel` (401), `preadv`
   (540), `recvmsg_x` (480) — moot but listed, because the sweep must not be taught to lie. One is
   in the census: 412, from `/bin/ed`. It was labelled "a live M10-class fix" on the strength of
   being exercised; measured at Task 6, the descriptor it carries is fd 2, which translated to
   itself before and after, so the fix is real for the class and inert for the only guest that
   reaches it.
2. **Six classifications the legacy tables had no opinion on**, each from a kernel prototype and
   each exercised by a named guest: `fchdir` (13) takes a descriptor; `kqueue` (362) returns one;
   `execve` (59) and `posix_spawn` (244) read `argv`/`envp` — and `posix_spawn`'s `adesc` — through
   nested pointers; `sigreturn` (184) follows `uctx->uc_mcontext64`, serviced above the trace so the
   view is consulted for it by nothing; and `map_with_linking_np` (550)'s `link_info` is a
   caller-sized read capped only at 64 MiB — `Source` under rule 4, the first `Source` row added
   since M30 wrote the list, and the one row whose kind changes what the box does on a hot path (the
   canary is now withheld for 550; measured sizes ≤ 2,920 bytes mean it could never have been
   reached on this corpus, and the call has no destination, so the cost is nothing).
3. **Seven prototype errors in the plan's starter rows, caught by the implementer at row level.**
   The plan's Task 5 listed rows from memory of the SDK; checked against xnu `syscalls.master` and
   `syscall_sw.h`, seven were wrong: `crossarch_trap` (38) was **absent** and is issued by nearly
   every dynamic guest; `gettid` (286) has two out-pointers, not `(void)`; `gettimeofday` (116) has
   three pointers (the kernel's `mach_absolute_time` third argument, not the SDK's two);
   `bsdthread_register` (366) has seven arguments with `flags` at index 2, not six;
   `map_with_linking_np` (550) is `(regions, region_count, link_info, link_info_size)`, not
   `(regions, count, files, nfiles)` — wrong prototype **and** wrong classification;
   `posix_spawn`'s `adesc` is a nested reader, not `Ptr`; and `mach_timebase_info_trap` (-89) is
   **forwarded**, not answered from the synthetic timebase as the plan said (the synthetic timebase
   is the CNTVCT MRS emulation in `Box_::run()`; `retrace-core` has no arm for -89). This is M20's
   "right conclusion, unmeasured supporting fact" class, caught here at row-writing time rather
   than by a review after it — because the spec's rule that every row carries its C prototype as
   its comment, with the source cited when it is not the public SDK, makes a row written from
   memory visibly incomplete before it is ever reviewed.
4. **`0x80000000`** in the census is dyld's inline `__mac_syscall("Sandbox", …)`,
   `MAC_SYSCALL_MAGIC` (`retrace-core/src/lib.rs:19-22`), synthesized and never forwarded. Task 1
   could not identify it; the controller's cross-check did; its row is `[Path, Scalar, Ptr]` and the
   "unidentified" sentence in `census.rs` is closed.
5. **`AT_FDCWD` is rejected as EBADF** for the form real guests pass — above, and Ruling 10.
6. **The README named four binaries the corpus does not contain.** "Among them … `grep`, `wc`,
   `uname`, … `bzip2`" survived from M22's uncommitted sample through M32's close; none of the four
   is in `tools/apple-sweep-binaries.txt`. Corrected in place at Task 6, with the correction noted
   in the sentence.

### What landed

- **`crates/retrace-arch/src/lib.rs`** — `ArgKind` (`Scalar`, `Fd`, `Path`, `Source`,
  `NestedSource`, `Dest(DestLen)`, `NestedDest`, `Ptr`), `Ret` (`Plain`, `Fd`, `FdPair`), `Shape`,
  `arg_kinds`, `forwarded_shape`, and the five views as one-liners. 129 rows: the 52 legacy numbers
  (Task 3, verified equal to the legacy union by set equality, not by count) and the 77 census
  numbers (Task 5), grouped by family, every row opening with its prototype and its source, every
  `Ptr` with its cited bound, no new `Dest`. The five old doc comments' 142 distinctive phrases —
  every number, citation and quoted claim — were migrated onto the variant and row they belong to
  and checked by grep; the `mach_msg2` reason M32 disproved is left *named* on its row, per
  CLAUDE.md.
  `IOCPARM_MASK`, `IOC_IN`, `IOC_OUT`, `iocparm_len` and fourteen `MACH_*_TRAP` selectors as arch
  facts, the latter pinned to `syscall_sw.h`. Six documentation corrections from the Task 5 review
  landed as Task 6's first commit — none changes a row's kinds; one splits `pipe`'s `FdPair`
  assertion into a test named for it.
- **`crates/retrace-arch/tests/legacy_equivalence.rs`** — the five M32 tables verbatim as
  `legacy_*` fixtures (Task 2, before any production change, green by identity), the both-directions
  sweep over the whole domain, `EXPECTED_DIFFS` (22), and the check that every `exercised` /
  `unexercised` label agrees with the census.
- **`crates/retrace-arch/tests/census.rs`** — `CENSUS: &[i64]` (108), sorted and deduplicated by
  test, and `every_census_number_has_a_row`.
- **`crates/retrace-box/src/lib.rs`** — one line: `translate_fds` iterates
  `forwarded_shape(num).fd_operands()`, the first statement `forward_and_diff` executes, so the
  panic sits upstream of every other view consulted there.
- **`crates/retrace-guest/asm/unenum.s`** and **`crates/retrace/tests/unenum_e2e.rs`** — a guest
  that issues syscall 8 (`nosys`) then exits 0, and the test that asserts on the `M33:` line and
  never on an exit code.
- No `retrace-core` edit. No `TRACE_MAGIC` bump. No recorded byte changed.

**Four instruments were proven able to fail before they were trusted**, each red quoted from the
run that produced it:

- *The oracle itself (Task 2)*: `legacy_dest_buffer`'s `getfsstat64` arm changed from `Reg(1)` to
  `Reg(2)` — `views disagree with the legacy tables and no EXPECTED_DIFFS entry says why:
  [(347, DestBuffer)]`. Reverted.
- *Control 1 (Task 3)*: (a) `dup2`'s row set to `[Fd, Scalar]` — `… no EXPECTED_DIFFS entry says
  why: [(90, FdOperands)]`; (b) the `(154, FdOperands)` entry deleted — `… [(154, FdOperands)]`.
  Both reverted; the revert confirmed from the commit, not the working tree.
- *Control 2 (Task 4)*: `forwarded_shape` mutated to `unwrap_or(&Shape { args: &[], ret: Plain })`
  — the unit test: `note: test did not panic as expected`; the e2e: `recorder did not refuse
  syscall 8 by name; code=0 stderr=`. Reverted; `git diff` empty against `c12efc0`.
- *Control 3 (Task 5)*: the `getentropy` (500) row deleted — `census numbers with no arg_kinds
  row: [500]`, and both `cpython_e2e` tests red with `M33: syscall 500 (500) has no arg_kinds row
  in crates/retrace-arch/src/lib.rs — it cannot be forwarded unclassified (…)`, because the
  launcher issues 500 too. Restored from a saved copy.

And the first red of the real sweep, the one the milestone exists for — `EXPECTED_DIFFS` still
empty after the views were rewritten:

```
views disagree with the legacy tables and no EXPECTED_DIFFS entry says why: [(27, FdOperands),
(28, FdOperands), (120, FdOperands), (121, FdOperands), (154, FdOperands), (337, FdOperands),
(401, FdOperands), (402, FdOperands), (411, FdOperands), (412, FdOperands), (413, FdOperands),
(415, FdOperands), (480, FdOperands), (481, FdOperands), (540, FdOperands), (541, FdOperands)]
```

Sixteen pairs, all `FdOperands`, none in any other view — so `AllocatesFd`, `DestBuffer`,
`NestedPointer` and `ReadsGuestBuffer` reproduced their legacy tables entry for entry over the
whole domain, and no legacy row had to be "fixed" against its prototype. Task 5's second sweep,
run before its six entries were listed, named exactly those six and nothing else.

### Rulings

Every `Ruling:` line from the milestone's ledger, verbatim, followed by the two Task 6 made.

Ruling 1: T4's loud forward lands before T5's census rows, so box/core tests that forward a
non-legacy syscall are red for the span of one task — accepted: the plan runs only three targets in
T4 and T5 Step 5 runs box+core in full; the gate is T6. Cost if wrong: one extra fix round in T5.

Ruling 2: T2's `legacy_*` functions are verbatim copies of production logic — mandated by spec §5b
as the equivalence oracle (a fixture, not production). A duplication finding against them is
answered by this ruling; a finding that the copy is NOT verbatim is real. Cost if wrong: none (the
sweep proves the copy).

Ruling 3: Task 3 also makes the three one-line signature adaptations the plan gave Task 4
(`retrace-box/src/lib.rs:3104` `for &i in`→`for i in`; `fdxlat.rs:13` same; `fdxlat.rs:106`
`.count()`), because `fd_operands` becoming an iterator otherwise leaves the workspace uncompilable
for one commit. Task 4 keeps only the `forwarded_shape` call and the unenum guest/e2e. Cost if
wrong: none — Task 4 finds the edits already made.

Ruling 4 (for Task 5, spec §4b ioctl): DTRACEHIOC_ADDDOF (_IOW('h',4,user_addr_t), nested read +
copyout of dof_ioctl_data_t per xnu bsd/dev/dtrace/dtrace.c dtrace_ioctl_helper — Task 5 to cite the
exact lines) is issued by nearly every dynamic guest via dyld. The spec's pre-authorised remedy — a
refuse-by-value ASSERT — would make every dynamic guest unrecordable, which is the spec's own §9
halt clause. Ruled instead: NO assert; row stays [Fd, Scalar, Ptr] with IOCPARM_MASK as the bound on
the DIRECT parameter; Task 5 MEASURES the recorded (ret, err) of that ioctl on a dynamic guest
(temporary eprintln, not committed) to ground "the nested copyin fails, so the copyout is
unreachable"; the residual (a nested pointer forwarded unrefused) goes on the row comment, the
README's rewritten ioctl paragraph, and the status-log's owed list. Cost if wrong: a latent
nested-pointer hazard stays forwarded exactly as it has since M2 — today's state, not a regression.

Ruling 5 (for Task 5, spec §4c sysctl newp): 7 measured non-null newp rows have newlen 10–32 =
string lengths, consistent with libc's name2oid idiom (MIB {0,3}, name via newp/newlen). Task 5
MEASURES the MIB (temporary print of the u32s at args[0], namelen args[1]; not committed). If
name2oid: rule 5 applies BEFORE rule 6 — xnu kern_newsysctl.c sysctl_sysctl_name2oid rejects newlen
>= MAXPATHLEN, a citable bound → Ptr (cite the line), and the general per-handler newlen check
covers other MIBs. If NOT name2oid and no bound can be cited: rule 6 → Source + EXPECTED_DIFFS (202,
ReadsGuestBuffer) + ledger the coverage cost on the KERN_PROC_ALL Dest. Cost if wrong: a
caller-sized kernel read stays canary-filled (M30 class) — bounded by MAXPATHLEN, so no band can be
reached.

Ruling 6 (for Task 5, pipe 42): add `Ret::FdPair` (documentation; `allocates_fd` view unchanged =
false, matching legacy) rather than binding one of two fds or asserting. Binding is a return model
M33 does not own (M10 successor work); asserting would move /bin/zsh to `recorder panicked` for a
gap M33 did not create. Owed item in status-log + README. Cost if wrong: zsh-class guests keep
getting EBADF on pipe use, silently — today's state.

Ruling 7 (for Task 5, execve 59 / posix_spawn 244): rows are header truth ([Path, NestedSource,
NestedSource] and the brief's posix_spawn row) → EXPECTED_DIFFS ReadsGuestBuffer entries, 59
"exercised (/bin/sh)". The fail-loud assert the bsdthread_create precedent demands is NOT added: it
would re-park cpython_e2e's launcher test (a NEW #[ignore] = charter §5 halt condition, the
operator's call) and move /bin/sh + the launcher into the sweep's panicked set. Surfaced to the
operator in the finish message; recorded as owed. Cost if wrong: a forwarded exec that ever stops
EFAULTing replaces retrace's process — the same exposure the tree has carried since M2.

Ruling 8: Task 6 excludes its Step 5 (memory files — controller-owned, written at finish) and the
merge half of Step 6 (done after the final whole-branch review via finishing-a-development-branch,
per the SDD skill). Task 6 delivers: sweep re-run + §9 rulings, the full chunked gate +
reconciliation, README, status-log section, the close commit on the branch. Cost if wrong: none —
the merge is one command later.

Ruling 9 (Task 6, spec §9, the sweep re-baseline): no binary moved — `pass=46 fail=8 skip=0`, the
same PASS set and the same FAIL set with the same reason strings, `dddiagnose` on a pass — so no
§9 clause fires and no M36 row is created by the sweep. The four candidates flagged for movement
were recorded by hand and their traces read (table above): `/bin/ed`'s newly translated descriptor
is fd 2, a console fd that translates to itself, so its outcome is byte-identical to the raw
forward and its EFAULT is the pre-existing nested `iov_base`; `/bin/ls`'s `fchdir(4)` now succeeds
where a raw fd 4 was forwarded before, deterministic on both sides both times; `/bin/wait4path`'s
`kqueue` return is now bound and unused; `/bin/zsh`'s `pipe` returns the host read-end in `x0`
with `x1` stale, the mechanism `Ret::FdPair`'s doc now states. The structural reason none could
move: the sweep's PASS is record/replay agreement, and an M10-class wrong descriptor is
deterministic on both sides, so a translation fix moves a binary only if the untranslated
descriptor had caused a divergence or a panic. Cost if wrong: none — the sets were compared by
name, not by count.

Ruling 10 (Task 6, spec §9's "other" clause, applied to a finding rather than a movement):
`translate_fds` rejects `AT_FDCWD` as EBADF in every instance a real guest was seen to pass it
(`/bin/ls` twice, `/bin/ed` once — `x0 = 0xfffffffe`, the 32-bit `-2` the ABI puts in `w0`,
non-negative under the `(v as i64) < 0` sentinel check), and the `fdxlat` test for the sentinel
passes the 64-bit sign-extended form no guest produces, so it is green while the real form fails.
Present since M10 t3 (`e67dd65`), deterministic on both sides, invisible to the sweep (`/bin/ls`
PASSes printing `ls: .: Bad file descriptor`). **Not fixed at M33**: spec §7 and §8 allow no
behavioural change beyond the sixteen translations, a fix changes what `ls` and every
`openat`/`fstatat64`-relative guest records, and the sweep would have to be re-baselined against
it — a successor's row, with the `fdxlat` fixture corrected to the measured form alongside the fix
(`(v as i32) < 0` is the obvious shape; it is not measured). Recorded in the README's descriptor
entry and in "What stays owed" below. Cost if wrong: today's state since M10 — a relative-path
`openat`/`fstatat64` keeps failing EBADF on both runs, agreeing with itself.

### What this milestone does not do

Spec §7, verbatim:

- **No new `Dest` rows.** `proc_info`, `getattrlist`/`fgetattrlist`, `csops` stay `Ptr` with a
  comment naming them as M34's. `dest_buffer` widens the forwarded-count clamp *and* the diff
  window; a wrong length is the M26 truncation class, and each of those three is a measurement
  the charter assigns to M34, "the milestone most exposed to right-conclusion-unmeasured-fact".
- **No per-argument canary fill.** The schema can now say "fill past argument 2, not argument 4"
  — that is the field M32 wanted — but nothing consults it. M32's Control 1 stays unexecuted and
  owed with the mechanism; §4c says when it stops being inert.
- **No nested-pointer translation.** `NestedSource` rows are forwarded exactly as today (the
  untranslated-read hazard M30 named and did not fix); `NestedDest` rows are refused exactly as
  today.
- **Rows only for what is dispatched or already tabled.** An unenumerated syscall is not a bug
  in this table; it is the loud failure working. The sweep re-run (§9) is where that shows.
- **`Scalar` vs `Ptr` is not verified by anything but the reviewer.** Stated in §3a; repeated here
  so the row count is not mistaken for a coverage claim.

Each held. No `Dest` row was added (the schema test's no-two-`Dest` rule now runs over the
`0x8000_0000` band too). Nothing consults `arg_kinds` per argument for the fill. `NestedSource`
rows are forwarded and `NestedDest` rows refused exactly as at M32. The sweep re-run showed no
unenumerated number. And `Scalar` versus `Ptr` on the 77 new rows was checked by the prototype and
the reviewer and by nothing else — the seven starter-row errors above are the measure of how much
that check is worth, and of how much it is not.

### The gate

**570 passed / 0 failed / 2 ignored across 124 test binaries**, every chunk exit code **0**
captured before any pipe; clippy clean over `--workspace --all-targets` with `-D warnings`. Four
chunks, the third in three groups: `--workspace --exclude retrace-box --exclude retrace` (whole
packages, so every library crate's `Doc-tests` harness ran — 7 of them, all zero tests);
`-p retrace-box` as a whole package (52 lib + 35 integration targets + its `Doc-tests`, 268
tests); `-p retrace --test <name>` for each of the sixty e2e targets, twenty per group (45 + 36 +
58, the 2 ignored in the last group); and **`-p retrace --bins`** (11), which is the only place
`crates/retrace/src/debug.rs`'s unit tests run and the chunk CLAUDE.md says not to omit. `jq_e2e`,
`jq_file_e2e` and `cpython_e2e` ran rather than skipped — Homebrew `jq` and Python are both
installed, and no skip line appears in any log.

Reconciled against M32's 556 / 0 / 2 over 121 **file-by-file rather than by sum**, every `.rs`
file in `crates/` diffed against `e13eb17`:

| file | M32 | M33 | delta |
|---|---|---|---|
| `crates/retrace-arch/src/lib.rs` | 30 | 36 | **+6** — `arg_kinds_reproduces_the_read_family_shape`, `an_unenumerated_syscall_panics_by_name`, `no_row_has_more_than_one_dest_argument_or_more_than_eight_arguments` (Task 3); `ioctl_request_codes_the_corpora_issue_decode_as_the_row_states`, `mach_trap_constants_match_syscall_sw_h` (Task 5); `pipe_return_is_a_pair_and_is_not_bound` (Task 6) |
| `crates/retrace-arch/tests/census.rs` | 0 | 2 | **+2, a NEW binary** — `census_is_sorted_and_deduplicated` (Task 1), `every_census_number_has_a_row` (Task 5) |
| `crates/retrace-arch/tests/legacy_equivalence.rs` | 0 | 3 | **+3, a NEW binary** — `every_view_reproduces_its_legacy_table`, `expected_diffs_name_only_numbers_in_the_domain` (Task 2), `exercised_and_unexercised_match_the_census` (Task 5); and it `#[path]`-includes `census.rs`, so census's two tests run **a second time** here — 5 results from 3 attributes, said in the file's own header |
| `crates/retrace/tests/unenum_e2e.rs` | 0 | 1 | **+1, a NEW binary** — `an_unenumerated_syscall_is_refused_by_name` (Task 4) |

No other file's count moved; `--bins` **11 → 11**; `Doc-tests` 7 → 7. Three new binaries,
121 → 124. **The two ends of the count must be read separately this time.** The tree holds **570**
`#[test]` attributes = 568 runnable + 2 ignored (M32 held 558 = 556 + 2; +12 attributes). The run
reports **570** passed = 568 + 2, because census's two tests execute in two binaries. The two 570s
are a coincidence of the same "+2", not one number derived twice. A bare `grep -c '#\[test\]'` over
the tree says 571, because a comment in `legacy_equivalence.rs` mentions the attribute in prose;
the file has three, and the reconciliation above counts attributes, not mentions. The brief's
prediction was 564 (+8); the +6 beyond it is the pipe split (+1), the two Task 5 unit tests it did
not know about (+2), and census's second execution (+2) plus the enforcement test (+1) — each
named above.

The two ignored gates are unchanged: `stackoverflow_rust_e2e` (the M21 signal-model wall) and
`cache_symbol_e2e` (the M19 shared-cache symbol wall), confirmed by their `#[ignore` attributes
being the only two in the tree. M33 parked nothing new and un-parked nothing.

### What stays owed

* **The per-argument canary fill, and M32's Control 1 with it.** `arg_kinds` is the per-argument
  direction notion M30 said the fill decision needed; nothing consults it per argument, because the
  stale-register reproduction (`bigwrite_e2e`'s `x4 = buf + 128`) is a filled band reached through
  a register the call does not declare, and no measurement has been taken of a per-argument fill
  against it. Spec §4c measured the case still inert: the one corpus call that could have made it
  live — a `Source` `newp` on the `sysctl` whose `KERN_PROC_ALL` `Dest` exceeds the window — is
  `Ptr`. Control 1 is still an unexecuted control, not a discharged one.
* **M34's three `Dest` rows** — `proc_info` (336), `getattrlist`/`fgetattrlist` (220/228), `csops`
  (169/170) — left `Ptr` with a comment naming them as M34's, because each is a length measurement
  the charter assigns there.
* **Nested-pointer translation.** `NestedSource` rows are forwarded with their nested pointers
  untranslated — `writev`'s `iov_base`s EFAULT in retrace's process, `execve`/`posix_spawn`'s
  `argv`/`envp` likewise, `DTRACEHIOC_ADDDOF`'s `dof_ioctl_data_t` likewise (measured) — and
  `NestedDest` rows are refused. The ioctl residual is a nested pointer forwarded unrefused; the
  spec's refuse-by-value assert was ruled out because dyld issues it on nearly every dynamic guest
  (Ruling 4).
* **`pipe`'s return.** `Ret::FdPair` is documentation. `host_svc` captures `x0` and the carry;
  `apply_and_return` sets `x0` alone. The guest gets the host read-end in `x0`, unbound, and its
  own stale `x1`; both host descriptors leak in the recorder. Capturing `x1` is the successor item
  ahead of any binding model (Ruling 6).
* **The `execve`/`posix_spawn` fail-loud assert** the `bsdthread_create` precedent demands. Both
  are forwarded and fail only because their nested pointers EFAULT; a forwarded exec that ever
  succeeded would replace retrace's process. Deferred to the operator, because adding it re-parks
  `cpython_e2e`'s launcher test — a new `#[ignore]`, charter §5 — and moves `/bin/sh` and the
  launcher into the sweep's panicked set (Ruling 7).
* **Console `writev` mirroring.** `is_console_write` covers `write`/`write_nocancel` only, so a
  `writev` to fd 1/2 is forwarded rather than mirrored, and `/bin/ed`'s stderr message is lost to
  the nested-pointer EFAULT. M9's class in a new spelling.
* **`__disable_threadsignal` (331)** is forwarded and acts on retrace's own thread. Pre-existing,
  documented on its row, not modelled.
* **`AT_FDCWD` in the 32-bit form real guests pass** (Ruling 10) — the sentinel check, the `fdxlat`
  fixture, and the sweep re-baseline the fix will cost.
* **`Scalar` versus `Ptr` is reviewer-verified only**, on 129 rows. The equivalence sweep proves
  the views, not the rows.
* **The corpus bias M32 named is carried unchanged.** Every governed `mach_msg2` call in the
  corpus is still init-time and shallow; the repo-owned threaded/GCD fixture that would change the
  population was not added.
* **The `unexercised` label is enforced against a census dated 2026-09-12.** `census.rs` pins the
  numbers and `legacy_equivalence.rs` checks every label against them, so a label cannot rot
  silently — but the census itself is a snapshot of the corpora on that day, and a guest added
  later is not in it until someone re-runs the census script (`.superpowers/` is gitignored, so
  the script lives only in the ledger; the procedure is in the M33 plan's Task 1).
* **The README carried four binary names the corpus does not contain from M22 through M32**, and
  nothing in the tree could have caught it: the sentence is prose and the corpus is a file. It is
  corrected, and the correction is noted in the sentence itself, but the class — a current-state
  claim with no instrument — is the same one the census closed for syscall numbers and did not
  close for anything else.
* **An undeclared cross-version consequence, found by the final reviewer.** `kqueue` (362) gaining
  `Ret::Fd` changes how `ReplaySession` (`crates/retrace-core/src/lib.rs:2340`, the `if !*err &&
  retrace_arch::allocates_fd(num)` arm) interprets a recorded successful `ret` for 362, so a
  PRE-M33 recording of `/bin/wait4path` now replays to the divergence "… trace predates M10's fd
  table" — a wrong diagnosis, not a panic, and not a hole for any M33 recording, but the first
  `allocates_fd` addition since M10 and the one thing "no recorded byte changed" does not cover.
  M25 and M29 set the no-`TRACE_MAGIC`-bump precedent for this class.

## Status: M34-destgaps — two `Dest` rows, one cited bound, and a corpus that reaches neither

M33 closed with three syscalls still on the README's flat-window list, each carrying a row comment
that named it as M34's: `proc_info` (336), `getattrlist`/`fgetattrlist` (220/228) and
`csops`/`csops_audittoken` (169/170) — three length measurements the M32–M38 charter assigns to
this milestone, "the milestone most exposed to the 'right conclusion, unmeasured supporting fact'
failure M20 named". M34 took the three measurements and found two rows to add and one premise to
retire. `proc_info` and `csops` gained `Dest` rows, because their blob and list callnums are
bounded by nothing below the window except the caller's own length. `getattrlist`/`fgetattrlist`
stay `Ptr`, because xnu rejects with `ENOMEM` before writing a byte whenever the packed result
exceeds 15,360 bytes — a cited bound four times inside the window, which is the table's own `Ptr`
rule applied rather than an exception to it (Ruling 1). Neither new row changes a *recorded* byte
on this corpus: the largest length operand across 851 dispatches of the five is 1,052 bytes, so
the window half of `Dest` is inert today, exactly as M32's per-argument fill was measured inert.
The half that is not inert is the forwarded-count clamp, the half M27 called serious — and it is
not inert on this corpus either, which this section's first draft got wrong (corrected by the fix
wave, below): on every dynamic guest, 76 of 76, it rewrites the forwarded length of dyld's
`proc_info(SET_DYLD_IMAGES)` from 368 to 128, because that destination straddles a shared-cache
page boundary and cache pages are individual 16 KiB backings — with no effect on any byte, since
the call transfers nothing and the kernel rejects it before reading the size. Control 3 measured
what the clamp prevents where it matters: without it, the host kernel — handed a 4,160-byte
`buffersize` over a 64-byte backing — wrote 3,612 bytes into retrace's own process and reported
no error. Taking the measurement also found a defect outside the milestone's scope,
recorded and routed rather than fixed (§4b, Ruling 3): `forward_and_diff`'s register probe rewrites
a *pid* that happens to land inside a guest backing, so for roughly half of all recorder pids every
`csops` and `proc_info(PIDINFO)` in the corpus fails `ESRCH` — on both runs identically, which is
why no oracle has ever seen it.

The milestone's own numbers: **three** rows changed shape (336, 169, 170 — the two syscall
families the title counts), **two** rows left `Ptr` with a citation (220, 228), **three**
`EXPECTED_DIFFS` entries, **two** tests (`truncguard.rs` 19 → 21, the only file whose count
moved), **one** committed instrument (`tools/destgaps-census.sh` and its summariser), **851**
dispatches over **76** guests with a corpus maximum of **1,052** bytes, the sweep at
`pass=46 fail=8 skip=0` on its second run (45/9 on its first — the documented intermittent, below),
the gate **572 / 0 / 2 over 124**. `TRACE_MAGIC` did not move, no trap arm was touched, no
`retrace-core` line changed, the one `retrace-box/src` change is the fix wave's gated diagnostic
(`RETRACE_REGCLAMP`, inert unless set), and no recorded byte changed.

### What it set out to do

The charter's entry, `docs/superpowers/specs/2026-09-09-retrace-m32-m38-program-charter-design.md`
§3, "M34 — `destgaps`: the three uncovered `dest_buffer` syscalls", quoted:

> **Discharges:** the README's "3 remain uncovered by `dest_buffer`" — `proc_info`,
> `getattrlist`/`fgetattrlist`, `csops`.
>
> Each needs its reply-length operand located and added, as M27 did for `ps`'s
> `sysctl(KERN_PROC_ALL)` (`*(size_t*)x3`, 205,416 bytes).
>
> **Plan certainty: high** for the mechanism, **medium** per operand — each location is a
> measurement. The plan must make "measure the operand" an explicit task step with a recorded
> result, never an assumption baked into an edit. This is the milestone most exposed to the "right
> conclusion, unmeasured supporting fact" failure M20 named.

What a `Ptr` costs at those rows is the two things `Dest` provides and nothing else does, both in
`crates/retrace-box/src/lib.rs` and both record-side: the **diff window** — `Box_::diff_window`
(`:3082`) takes `max(min(avail, window_cap), clamp_count(avail, len))` when `dest_len_bytes`
(`:3062`) knows `len`, and the flat 64 KiB `window_cap` otherwise, so a kernel write past the flat
window is the M26 truncation class, captured by no `Event`, restored stale on replay and invisible
to the `(num, args)` oracle (the M27/M30 guard band *detects* that class; it does not prevent it) —
and the **forwarded-count clamp**, the `DestLen::Reg(li)` arm (`:3286` at `adb0402`; `:3298` after
the fix wave's channel comment) that rewrites `hargs[li]`
to `clamp_count(avail, count)` so the host kernel is never told a length larger than the guest
backing behind the destination. The README claims to discharge were the sentence at line 452
("**Three still get a flat 64 KiB**: `proc_info` (336); `getattrlist`/`fgetattrlist` (220/228);
`csops` (169/170)") and the owed-list entry at line 602. The entry's own "each location is a
measurement" is what turned out to matter: one of the three measurements contradicted the entry's
premise, and the milestone was re-scoped on it, loudly, as charter §5 requires.

### Ruling 1 — re-scoped on xnu source

The spec's ruling, verbatim:

> **Ruling 1: re-scoped M34 — the premise that `getattrlist`/`fgetattrlist` (220/228) can overrun
> the 64 KiB window was contradicted by xnu source: both entry points reach
> `getattrlist_internal` → `getvolattrlist` / `vfs_attr_pack_internal`, each of which rejects
> with `ENOMEM` before any copyout when the packed result exceeds `attr_max_buffer`
> (`ATTR_MAX_BUFFER_LONGPATHS` = 8192 − 1024 + 8192 = 15,360; `bsd/vfs/vfs_attrlist.c`, the
> `ab.allocated > attr_max_buffer` gates, and the copy `lmin(buf_size, ab.allocated)` /
> `ulmin(bufferSize, ab.needed)`) — new scope for those two rows: stay `Ptr`, cite the bound,
> pin the decision with a test.**

The source, from xnu `main` as fetched 2026-09-13. `getattrlist` (`bsd/vfs/vfs_attrlist.c:3577`)
and `fgetattrlist` (`:3486`) both call `getattrlist_internal` (`:3250`), which dispatches to
`getvolattrlist` (`:992`) for volume attributes or `vfs_attr_pack_internal` (`:2817`) otherwise.
Both packers compute `ab.allocated = fixedsize + varsize` and then `if (((size_t)ab.allocated) >
attr_max_buffer) { error = ENOMEM; goto out; }` **before allocating or writing anything**, where
`attr_max_buffer` is `ATTR_MAX_BUFFER` (8192, `bsd/sys/attr.h:134`) or, for a long-paths process,
`ATTR_MAX_BUFFER_LONGPATHS` = `8192 − MAXPATHLEN + MAXLONGPATHLEN` = 8192 − 1024 + 8192 =
**15,360** (`attr.h:140`; `MAXLONGPATHLEN` 8192 from `bsd/sys/syslimits.h:137`, the constant this
table's `fsgetpath` row already cites). The user copy is then `ulmin(bufferSize, ab.needed)`
(`getvolattrlist`, `:1686`) or `lmin(buf_size, ab.allocated)` (`vfs_attr_pack_internal`, `:3108`)
— never more than `ab.allocated`. So whatever `bufferSize` says, the kernel never writes more than
15,360 bytes through `attributeBuffer`. The `ArgKind` doc states the membership rule
(`crates/retrace-arch/src/lib.rs:264–267`): `Ptr` is "a read the kernel itself bounds far inside
the window (a `sockaddr`, an `ioctl` parameter), a fixed struct it writes (`struct stat`), or an
in/out scalar. **The row comment names the bound and its citation.** A bound that cannot be cited
is not a bound". A write the kernel itself caps at 15,360 against a 65,536-byte window is that
case exactly.
A `Dest` here would not have been wrong so much as false to the table's own vocabulary: a reader
would take it to mean "the caller's length is the only bound", which is untrue, and which the
measurement below shows no guest has ever needed (corpus maximum 1,052 bytes, 14× inside the
kernel's own cap). The ruling is pinned by an assertion, not only by prose — control 2 below is
the proof that the pin holds.

### The measurement (spec §4)

**The corpus** is the M33 census corpus, re-run 2026-09-13 because M33's raw per-guest outputs
lived under a since-deleted worktree's gitignored `.superpowers/` and did not survive — the risk
M33's own "what stays owed" named, and the reason the instrument is committed this time (§5e):
every Mach-O in the `retrace-guest` `OUT_DIR` (static via `record`, dynamic via `record-dyn`,
classified by `LC_LOAD_DYLINKER`), `jq --version`, `jq .name <rung-3 fixture>`, the CPython
interpreter and its launcher with `-c 'print(1)'`, and all 54 of `tools/apple-sweep-binaries.txt`;
bare argv, stdin `/dev/null`, 30 s watchdog. The instrument is `RETRACE_TRACE=1`'s `[trap] num=…
args=[x0…x5]` line on the record side, filtered to the five numbers — `x5` and `x3` are printed, so
no dedicated probe was needed. Script: `tools/destgaps-census.sh`; summary:
`tools/destgaps-census-summary.py`.

**Coverage:** 76 guests issued at least one of the five — every dynamic guest, because the calls
are libSystem/dyld init-time — and 42 issued none: the static `-nostdlib` guests, among them the
three (`crash`, `crashjmp`, `wildstore`) that fault before their first syscall. 851 matching
dispatches. (The spec's §4 said "115 guests dispatched" until the fix wave corrected it; 76 + 42
is 118. The census ran in two
passes — the first's progress log has 115 lines, and `/usr/bin/yes`, `/usr/bin/true` and
`/usr/bin/printenv` came in a second — and 115 is the first pass's count. Checked against the raw
TSVs: 821 + 30 = 851 rows over 76 distinct labels, the per-syscall figures below reproduced from
them exactly. Recorded here because the log is where a wrong supporting fact is supposed to be
left standing with its correction, and this one is the class the charter's entry warned about.)

**Result — the largest length operand in the whole corpus is 1,052 bytes.** No dispatch comes
within two orders of magnitude of the 64 KiB window:

| syscall | dispatches | guests | op / flavor | length | who |
|---|---|---|---|---|---|
| `proc_info` 336 | 318 | 76 | callnum 2 `PIDINFO`, flavor 13 `PROC_PIDT_SHORTBSDINFO` | 64 = `sizeof(proc_bsdshortinfo)` | every dynamic guest |
| | | | callnum 2, flavor 17 `PROC_PIDUNIQIDENTIFIERINFO` | 56 | every dynamic guest |
| | | | callnum 5 `SETCONTROL`, flavor 2 `PROC_SELFSET_THREADNAME` | 4 (the name `main`, a copyin) | the 9 Rust guests |
| | | | callnum 15 `SET_DYLD_IMAGES` | 368 (no transfer) | every dynamic guest, from dyld at `pc=0x14000600c` |
| `getattrlist` 220 | 191 | 76 | — | 12, 64, 1036; **1052 max** (CPython) | every dynamic guest |
| `fgetattrlist` 228 | 140 | 76 | — | 12, 40 | every dynamic guest |
| `csops` 169 | 126 | 76 | op 0 `CS_OPS_STATUS` | 4 | every dynamic guest |
| | | | op 16 `CS_OPS_DER_ENTITLEMENTS_BLOB` | 1032 | 11 guests, 22 dispatches: nine Apple binaries (`bash`, `date`, `launchctl`, `zsh`, `automationmodetool`, `dddiagnose`, `desdp`, `dyld_info`, `flex`) and both CPython invocations, two each (first draft: "2 (an Apple binary, CPython)" — see the fix-wave note below) |
| `csops_audittoken` 170 | 76 | 76 | op 16 `CS_OPS_DER_ENTITLEMENTS_BLOB` | 1032 | every dynamic guest |

**The raw-vs-libc check M29 taught** — the one that caught `sysctlbyname`'s indices being
identical to `sysctl`'s rather than one lower — passes for all five: the register the row names
carries a byte count, verified against the SDK by `sizeof`. `proc_bsdshortinfo` = 64 matches
flavor 13's `x5`; `attrlist` = 24 is the `alist` copyin at `x1`, not the length at `x3`;
`CS_OPS_STATUS`'s `x3` = 4 = `sizeof(uint32_t)`.

**Stated plainly:** on this corpus neither new `Dest` row changes a single *recorded* byte — every
destination already fits the flat window, so the widening half is inert today. The half that is
not inert is the **clamp**, and — corrected by the fix wave, next — it is not merely structural:
it fires on every dynamic guest. Beyond that one call it is structural: after M34 a guest that
passes `proc_info` or `csops` a `buffersize`/`usersize` larger than its buffer's backing has the
forwarded length clamped to that backing, where before the host kernel would have been told the
guest's number and written past it in retrace's own process. That is the M27 "serious half"; the
window half is the correctness-by-contract that makes the README's sentence true rather than an
open item. Neither is a fix to a reproduced bug, and the ledger says so.

**Corrected by the fix wave — the clamp fires on 76 of 76 dynamic guests.** The table above
measured lengths against the window. The clamp compares a length against the *backing* behind the
destination, and that nothing had measured: the `DestLen::Reg` arm had no diagnostic channel (the
`DerefU64` arm has had `RETRACE_DEREFLEN` since M29, which is how M29 could say on measurement
that its refusal never fired), so this section's first draft inferred from lengths alone that "no
corpus call is in the regime the rows change". The final review recomputed the destinations from
the raw census and found otherwise, and the fix wave gave the arm its channel
(`RETRACE_REGCLAMP=1`, commit `cd92a7d`: one line per dispatch that reaches the arm with a mapped
destination, `[M34 REGCLAMP]` with the backing span when `count > avail` and `[M34 REGCLAMP-FIT]`
otherwise, so a zero-fire result can be told from an arm that never ran — M28's lesson, applied
to this arm as it was to the other) and measured. From the raw census: every `csops` /
`csops_audittoken` destination (126 + 76) and 242 of the 318 `proc_info` destinations are on the
dyn stack `[0x27C0000, 0x2800000)` and fit; all 76 `SET_DYLD_IMAGES` destinations are
`0x1ec6f7f80`, which is `0x3f80` into a shared-cache page, and cache pages are individual
backings (`page_in_cache`, one `Backing` of `GRANULE` per fault, nothing else mapping into the
window), so `host_span` gives `avail = 128` against `buffersize = 368` and the arm rewrites the
forwarded length to 128. Through the channel, `hello_dyn` and `jq --version` recorded
`RETRACE_REGCLAMP=1`, each printing exactly one firing line, verbatim:

```
[M34 REGCLAMP] syscall 336 count 368 avail 128 dest 0x1ec6f7f80 backing [0x1ec6f4000,0x1ec6f8000)
```

and `REGCLAMP-FIT` for every other 336/169/170 dispatch (`hello_dyn`: `169 count 4 avail 17332`,
`336 count 56 avail 20296`, `336 count 64 avail 16144`, `336 count 56 avail 16984`,
`170 count 1032 avail 17184`; `jq` the same five with its own stack offsets). With the variable
unset the run prints no such line and the recording replays. This is M29's "R1" case made
concrete — a buffer that legitimately continues into the next backing — and for a cache-DATA
destination it is the *normal* geometry, not the pathology the first draft's "already overruns
its own backing" implied. Why nothing saw it: the call transfers nothing, and the kernel rejects
it before reading the size (§4b, corrected below), so no return, no write and no recorded byte
moved, and the gate and sweep could not either. The clamp is doing what M27's "serious half"
asks — an unclamped 368 at a host pointer with 128 bytes of page left would overrun a 16 KiB
`mmap` into whatever follows — so this is a docs correction, not a code defect. What it does
expose is owed, below: any `Dest` destination in shared-cache DATA that straddles a 16 KiB
boundary is clamped at the boundary by the per-page cache backing, a fidelity hazard that
pre-dates M34 and applies to the `read` family too.

**Corrected by the fix wave — `csops` op 16 is 11 guests, 22 dispatches, not 2.** The table's
first draft said "2 (an Apple binary, CPython)". `tools/destgaps-census-summary.py` collapsed
every non-`guest:` label to its family (`apple`, `cpython`, `jq`) before counting and then printed
`[2 guests: apple, cpython]`, which was read as two guests — the charter's "right conclusion,
unmeasured supporting fact" class, in a source comment, from the instrument committed so the next
milestone would not lose its census. The summariser now counts full labels and abbreviates only
the display string; re-run on the raw census it prints, for op `0x10` on 169: `[11 guests, 22
dispatches: apple:/bin/bash, apple:/bin/date, apple:/bin/launchctl, apple:/bin/zsh,
apple:/usr/bin/automationmodetool, apple:/usr/bin/dddiagnose, … +5]` — the five elided being
`desdp`, `dyld_info`, `flex`, `cpython:interp`, `cpython:launcher`, two dispatches each.
Conclusion unaffected (max 1032, `Dest`).

### What changed

Three code commits — `3e05664` (Task 1, the instrument), `e3ec90a` (Task 2, rows, window test,
ledger), `486bab2` (Task 3, the clamp control) — the docs commit `adb0402`, and the fix wave after
the final review: `cd92a7d` (the `RETRACE_REGCLAMP` channel in `forward_and_diff`'s `Reg` arm —
the one `retrace-box/src` edit on the branch, a gated `eprintln!` with behaviour byte-identical
when unset), `6db8a27` (review minors M1–M3: the complete precondition loop, the census script's
trap before its `cp`, and `dest_buffer` view pins for 336/169/170 and for 220/228's `None` inside
the existing `dest_buffer_knows_where_each_length_lives` / `dest_buffer_omits_what_it_should`
tests — no new `#[test]`), `bbcc2c4` (the docs-and-instrument commit carrying the three
corrected facts), `e31c1a6` and `67a97df` (citation and wording follow-ups), and one further
docs-only commit after the scoped re-review, which adjudicated its four prose minors — this
paragraph is in it, so it cannot name itself.

- **`crates/retrace-arch/src/lib.rs`** — `proc_info` 336 (`:823` at `adb0402`; `:838` after the
  fix wave's comment corrections) is `[Scalar, Scalar, Scalar, Scalar, Dest(Reg(5)), Scalar]`,
  `x5` = `buffersize` (`uint32_t`); `csops` 169 (`:786`, now `:789`) is
  `[Scalar, Scalar, Dest(Reg(3)), Scalar]` and `csops_audittoken` 170 (`:787`, now `:790`) is
  `[Scalar, Scalar, Dest(Reg(3)), Scalar, Ptr]`, `x3` = `usersize` (`user_size_t`),
  `x4` the 32-byte audit-token copyin, still `Ptr`. `Reg`, not `DerefU64`, and clamp, not refuse:
  every length here is a pure in-value the kernel reads and never writes back, so nothing of
  M29's `sysctl` reasoning (an in-out `*oldlenp` that clamping would silently truncate) applies,
  and the rows take `read`'s posture exactly as `getdirentries64`, `recvfrom` and `getfsstat64`
  did in M29. Each row's comment now carries the kernel-source analysis in the M29 rows' shape.
  For `proc_info` (`bsd/kern/proc_info.c` `proc_info_internal`, dispatching on `callnum`):
  `LISTPIDS` (1), `KERNMSGBUF` (4), `LISTCOALITIONS` (11), `PIDDYNKQUEUEINFO` (13) and
  `UDATA_INFO` (14) are bounded by `buffersize` and a count the kernel owns — a tunable
  (`kern.maxproc`, `kern.msgbuf`), not a citable constant, hence `Dest`; `PIDINFO` (2),
  `PIDFDINFO` (3), `PIDFILEPORTINFO` (6) and `PIDORIGINATORINFO` (10) copy out a fixed struct after
  `if (buffersize < size) return ENOMEM`. Two callnums ride under `Dest` as an over-approximation,
  **Ruling 2**: `SETCONTROL` (5) with `PROC_SELFSET_THREADNAME` is a *copyin* of at most 63 bytes
  (`MAXTHREADNAMESIZE − 1`), a `Source` shape bounded 1000× inside the window, so no canary
  coverage is lost; `SET_DYLD_IMAGES` (15) transfers nothing at all ("don't need to copyin the
  buffer. just setting the buffer range in the task struct" — `proc_set_dyld_images`). For both
  the window widening is to ≤ 368 bytes, inside the flat window; the clamp `min(avail,
  buffersize)` fires whenever the destination's backing ends within `buffersize` of it — which,
  for callnum 15, is every dynamic guest (the first draft said "can fire only when the guest's
  buffer already overruns its own backing"; corrected by the fix wave, below). Accepted because
  the alternative, a per-callnum kind, is a schema change the charter's queue does not authorise
  and nothing measured needs. For `csops` (`bsd/kern/kern_proc.c`
  `csops_internal`, dispatching on `ops`): `CS_OPS_ENTITLEMENTS_BLOB` (7), `CS_OPS_BLOB` (10),
  `CS_OPS_DER_ENTITLEMENTS_BLOB` (16), `IDENTITY` (11) and `TEAMID` (14) copy out up to `usersize`
  through `csops_copy_token` (an 8-byte header and `ERANGE` if `usersize` is short), and
  `CS_OPS_BLOB` is the whole code-signing SuperBlob — one CodeDirectory hash per page of the
  binary, hundreds of KiB for a large one — so no citable bound below the window; the fixed-size
  ops write 4 bytes (`CS_OPS_STATUS` 0, with **no** `usersize` check; `VALIDATION_CATEGORY` 17),
  8 (`PIDOFFSET` 6) or a struct whose size `usersize` must equal (`CDHASH` 5,
  `CDHASH_WITH_INFO` 18), all inside the 64 KiB floor `diff_window` keeps under every `Dest`
  (`base.max(…)`) — which is what keeps a `CS_OPS_STATUS` with `usersize = 0` fully captured, and
  the row says so, so nobody later "fixes" the floor away for `Dest` rows and silently loses those
  4 bytes. `getattrlist` 220 (`:718`) and `fgetattrlist` 228 (`:514`) are unchanged in shape,
  `[Path, Ptr, Ptr, Scalar, Scalar]` and `[Fd, Ptr, Ptr, Scalar, Scalar]`; their comments now
  carry Ruling 1's citation — the two packers, the `ENOMEM`-before-copyout gate,
  `ATTR_MAX_BUFFER_LONGPATHS` = 15,360 — and the corpus maxima (1,052 and 40). The `ArgKind::Dest`
  doc paragraph (`:234–244`) is rewritten: M34 measured the three M29 left as "structurally capable
  of overrunning", two joined and one did not, and the sentence "`Ptr` there means 'not
  measured'" is deleted, because after M34 no `Ptr` in the table means that — every one names its
  bound.
- **`crates/retrace-arch/tests/legacy_equivalence.rs:127–129`** — three `EXPECTED_DIFFS`
  entries, `(336, View::DestBuffer, …)`, `(169, …)`, `(170, …)`, each "exercised (every dynamic
  guest; corpus max 368 / 1032 / 1032)", so the ledger records that the widening was inert on
  landing; the `exercised_and_unexercised_match_the_census` check would fail an entry that said
  otherwise for a census number. No entry for 220/228 — their view is unchanged, and a spurious
  entry fails as stale (control 1 shows the message).
- **`crates/retrace-box/tests/truncguard.rs`** — two tests, the only `#[test]` count that moved.
  `the_window_widens_for_the_m34_rows_and_not_for_getattrlist` (`:199`) goes through
  `Box_::diff_window_for_test` on a real `Box_` with `AVAIL = 1 << 20`, the seam M29's
  `the_window_widens_for_each_m29_reg_addition` used, so the production `diff_window` is exercised
  without a guest per syscall: 336 at index 4 with `args[5] = 200_000` → 200,000, and at index 5
  (the length, not the buffer) → the flat 65,536; 169 and 170 at index 2 with `args[3] = 150_000`
  → 150,000; 170 at index 4 (the audit token) → flat; and — Ruling 1 as a red bar — 220 and 228
  at index 2 with the same 150,000-byte `args[3]` → **flat**. A later reader who "finishes M34" by
  making `getattrlist` a `Dest` fails here and is sent to the citation.
  `the_clamp_reaches_proc_info` (`:246`) is control 3 (below): it loads the static `HELLO`, runs to
  the guest's first `Stop::Syscall`, and instead of forwarding that call calls
  `forward_and_diff(336, [1 /*LISTPIDS*/, 1 /*PROC_ALL_PIDS*/, 0, 0, dest, 4160, 0, 0])` with
  `dest = STACK_TOP_IPA − 64`, having asserted that `host_span_for_test(dest)` gives exactly 64
  bytes of backing and that every other register (`x0`–`x3`, `x5`–`x7`; the fix wave widened
  the check from five to seven) lands in no backing (so the test measures the clamp and not
  §4b's probe). `LISTPIDS` is the callnum precisely
  because it takes no pid.
- **`tools/destgaps-census.sh`** (109 lines) and **`tools/destgaps-census-summary.py`** (54
  lines) — the §4 instrument, committed so the next milestone re-runs it instead of rediscovering
  it, as M33's lost outputs forced this one to. Smoke-run at Task 1 on `/bin/echo`, `/bin/ls` and
  `/usr/bin/yes`: `matched=10` each, `yes` ending `CAPPED`, `DONE matched_total=30`, the
  summariser's five `max=` lines all `fits 64 KiB`. The script's header carries one lesson from
  its own first run: under `RETRACE_TRACE=1` a stdout-flooding guest floods *stderr* with the
  trace of its own writes — **measured at 5 GB in the 30 s before the watchdog fired**, on
  `/usr/bin/yes`, after which the first run spent its time grepping that file rather than measuring
  anything — so stderr is streamed through a line-capped filter (`head -n 400000`; `head` closes
  the pipe at the cap and the recorder dies on `SIGPIPE` at its next trace line), and the watchdog
  kills by command pattern because `$!` of a backgrounded pipeline is the filter, not the recorder.
  Not a test; runs in no gate. The summariser's per-combination line now reads `[N guests, M
  dispatches: <first 6 full labels>, … +k]`, counting full labels (the fix wave; the
  family-collapsed count is what produced the "2 guests" error above).
- **`README.md`** — the line-452 sentence replaced with what is true (two rows widened and
  clamped, two kernel-bounded at 15,360 and cited, corpus maximum 1,052, both new rows inert for
  the window and live for the clamp — and, since the fix wave, the 76-of-76 measurement of that
  clamp); the owed-list entry "M34's three `Dest` rows" removed, and the pid-collision probe
  entered in its place; the fix wave also moved the gate paragraph from M33's 570 to M34's 572
  with the reconciliation table, and added the `SET_DYLD_IMAGES`-above-the-trace and cache-page
  clamp entries to the owed list.
- No `retrace-core` edit, no `TRACE_MAGIC` bump, no new guest, no new trap arm; the one
  `retrace-box/src` edit is the fix wave's gated diagnostic in the `Reg` arm, which forwards and
  records nothing differently. Symmetry (spec §8) is by construction: `dest_buffer` is consulted
  only inside `forward_and_diff`, which replay never calls; the two effects are how many bytes the
  record side *looks at* after the syscall and the length the *host kernel* is handed — the second
  of which the clamp does change on every dynamic guest (368 → 128 at #24, above) without
  touching a recorded byte, because that call transfers nothing and is rejected before the size is
  read — and a call outside the corpus that does exceed the window is captured more completely,
  which replay applies as it applies everything else.

### Positive controls, run and recorded

Each mutation was applied, its red quoted from the run that produced it, and reverted before the
commit — the revert confirmed from `git diff` before staging, not by inspection.

- **Control 1 — the rows are wired to the window** (Task 2 Step 7; 336 reverted to `Ptr`). Two
  independent detectors, one through the box's production path and one through the table's own
  ledger. The window test: `proc_info's destination is x4 and its length x5`, `left: 65536`,
  `right: 200000`. The ledger's `every_view_reproduces_its_legacy_table`: `EXPECTED_DIFFS entries
  that no longer differ (stale — delete or explain): [(336, DestBuffer)]`. Restored, both green.
  One row proves the pair; the report names 336.
- **Control 2 — the ruling is enforced, not just written** (Task 2 Step 8; 220 changed to
  `[Path, Ptr, Dest(Reg(3)), Scalar, Scalar]`). The window test: `getattrlist is kernel-bounded at
  15,360 bytes (ATTR_MAX_BUFFER_LONGPATHS) and stays Ptr`, `left: 150000`, `right: 65536`. The
  sweep: `views disagree with the legacy tables and no EXPECTED_DIFFS entry says why: [(220,
  DestBuffer)]`. Restored, both green.
- **Control 3 — the clamp arm reaches the new rows** (Task 3 Step 3; 336 reverted to `Ptr`). No
  seam exposes `hargs`, so the clamp is observed through the kernel's own return value, the way
  `memdiff.rs`'s `forward_and_diff_captures_a_read_larger_than_the_window` observes the window
  through `ret`: `proc_listpids` copies out `min(nprocs + 20, buffersize / 4)` pids and returns the
  byte count, so with the clamp the kernel is handed `buffersize = 64` and returns exactly 64 with
  no error, and `(64, false)` is produced by the clamp and by nothing else. With the row, green:
  `(ret, err) == (64, false)`. Under the mutation, RED: `proc_info(LISTPIDS) with buffersize 4160
  into a 64-byte backing: the clamp must hand the kernel 64 and get 64 back; got ret=3612
  err=false`. Of the two outcomes the spec allowed the unclamped forward — `EFAULT` on the copyout,
  or an over-long return — it was the second, **measured**: the host kernel, handed
  `buffersize = 4160`, copied **3,612 bytes (903 pids) into a destination with 64 bytes of backing
  — 3,548 bytes past the guest backing, into retrace's own process, with no error**. That is the
  M27 "serious half" seen once. Restored, green; `git diff --stat crates/retrace-arch/src/lib.rs`
  empty before commit.

And the two reds the tests themselves owed before any control ran: the window test written first
(Task 2 Step 2) failed on exactly the predicted first assertion, `left: 65536, right: 200000`;
and with the rows landed but the ledger not yet written, the arch sweep went red naming
`[(169, DestBuffer), (170, DestBuffer), (336, DestBuffer)]` and nothing else — no other view moved,
so no legacy row had to be "fixed" against its prototype.

### A finding outside scope: the pid-collision probe (spec §4b, Ruling 3)

While designing control 3, reading what the recording says the kernel *returned* for these calls
showed that on this machine every one of them fails. `hello_dyn`, recorded 2026-09-13 with retrace
at pid `0x6a30` (27184), read back with a throwaway trace dumper (not committed):

| landmark | call | `ret` | `err` | natively |
|---|---|---|---|---|
| #24 | `proc_info(15 SET_DYLD_IMAGES, pid, …, 368)` | 22 `EINVAL` | true | 0 — but **not pid-caused**; see below |
| #145 | `csops(pid, 0 STATUS, …, 4)` | 3 `ESRCH` | true | 0 |
| #166, #226, #228 | `proc_info(2 PIDINFO, pid, 17/13, …)` | 3 `ESRCH` | true | 56 / 64 |
| #233 | `csops_audittoken(pid, 16, …, 1032)` | 3 `ESRCH` | true | 0 |

**Cause, located — for the `ESRCH` rows.** `forward_and_diff`'s per-register probe
(`crates/retrace-box/src/lib.rs:3188–3189`, `for i in 0..8 { match self.host_span(args[i]) …
hargs[i] = hp as i64 }`) treats *any* register whose value lands inside a backing as a pointer and
hands the host kernel the host address in its place. The pid is `x0` of `csops` and `x1` of
`proc_info`; on the dynamic path the backings at `TRAMPOLINE_IPA` `[0x4000, 0x8000)`, `PT_L2_IPA`
`[0x8000, 0xC000)` and `PT_L1_IPA` `[0xC000, 0x10000)` are contiguous, so **every recorder pid in
16384..=65535 is rewritten to a host pointer before forwarding** — roughly half the pid space, a
coin flip per record run. The kernel then sees a pid that is not this process (`ESRCH`) for #145,
#166/#226/#228 and #233. This is the "non-pointer whose value collides with a mapped IPA" hazard
`truncguard.rs` names in the comment above `a_band_with_no_neighbours_keeps_its_full_length`, with
"the dyld pread-count case" as its precedent; and M33's `ArgKind` doc says of exactly this:
"`Scalar`, `Path` and `Ptr` change nothing at runtime — `forward_and_diff` probes `host_span` on
all eight registers regardless — and are documentation until a later milestone consults them."

**#24's `EINVAL` has a different cause, and it is not pid-shaped.** *(Corrected by the fix wave.
This section's first draft attributed it to the same probe, through `proc_set_dyld_images`'s
`pid != proc_getpid(pself)` check — a causal claim that would have sent the fix milestone chasing
a symptom its fix cannot change.)* `proc_set_dyld_images` calls
`task_set_dyld_info(task, buffer, buffersize, false)` on the **calling** task, which for a
forwarded call is retrace's. xnu `osfmk/kern/task.c` on that function, quoted: *"called at most
three times. 1) at task struct creation to set addr/size to zero. 2) in mach_loader.c to set
location of __all_image_info section in loaded dyld. 3) is from dyld itself to update location
of all_image_info. For security any calls after that are ignored."* A non-zero-over-non-zero
update sets `TF_DYLD_ALL_IMAGE_FINAL`, and every later call returns `KERN_FAILURE`, which
`proc_set_dyld_images` turns into `EINVAL`. Retrace's *own* dyld made call 3 on retrace's task at
retrace's startup, so the task is final before any guest runs, and a forwarded `proc_info(15, …)`
returns `EINVAL` with the correct pid and with any size. Measured in the fix wave with a plain
dynamically-linked process calling `__proc_info(15, getpid(), 0, 0, buf, 368)` after its own dyld
had registered (`sdi.c`, in the SDD scratchpad; committed by nothing), verbatim:

```
pid=67548 (0x107dc) in_collision_range=no  proc_info(15, own pid, 368) -> ret=-1 errno=22 (Invalid argument)
  with size 128 -> ret=-1 errno=22 (Invalid argument)
  with wrong pid -> ret=-1 errno=22 (Invalid argument)
```

The pid is outside the collision range, the size is the one the clamp hands the kernel and the
one it does not, and the answer is `EINVAL` every time. So the `ESRCH` rows are pid-caused as
stated; #24 is not, and after the `Scalar`-audit fix below it will still read `EINVAL` — a fix
milestone using this table as its symptom list must not chase it. The right treatment is
different in kind and is owed, below: the forwarded call names *retrace's* task, and if it could
succeed it would point retrace's own dyld info at guest memory, so `SET_DYLD_IMAGES` should be
serviced above the trace (synthesise `0`; dyld ignores the return either way) rather than
forwarded — the same family as every "on self" `proc_info`/`csops` being answered about retrace's
process rather than the guest's.

**Why record and replay agree, and why it is not a halt.** Replay never forwards, so the probe
never runs there; it applies the recorded `ESRCH`, and the oracle compares the guest's own
`(num, args)`, which are the same on both sides. It is not an E2 flake of the oracle but a
record-vs-native fidelity defect whose *presence* depends on the recorder's pid — and nothing
short of a native comparison can tell a guest that got `ESRCH` on both runs from one that got its
pid info on both. Not a charter halt, then: a halt is a regression or a divergence, and this is
neither — record and replay agree, and nothing this milestone changed made it so. It does not
change §4's numbers: the `[trap]` line prints the guest's registers *before* translation, so every
length above is what the guest asked for.

**Why it is not fixed here.** The fix is for `forward_and_diff` to skip the probe for positions the
row says are `Scalar`. Its blast radius is every `Scalar` in a 129-row table, and M33 §7 states
that `Scalar`-versus-`Ptr` "is not verified by anything but the reviewer" — the seven starter-row
prototype errors M33 caught are the measure of what that check is worth — so the fix's
precondition is a `Scalar` audit with its own measurement: a milestone, not an edit inside this
one. Under charter §5 that is scope this spec does not cover, so it is **recorded, not taken**.
Its positive control is already in hand: the `hello_dyn` table above, with landmark #145 returning
0 and 4 bytes captured after the fix.

**Routed to M36.** A sweep row whose outcome depends on which half of the pid space the record
run drew is exactly the kind of row M36's `root_cause_class` must be able to name, and this is a
concrete, testable hypothesis for the README's one *intermittent* failure. The instruction is
concrete: **M36's measurement records the recorder's pid beside each sweep row**, in the shape the
controller's `ddd-probe.sh` already used for the probe below (`sh -c 'echo "recpid=$$" >&2; exec
"$0" record-dyn …'`, then whether the pid is in `[0x4000, 0x10000)`). The probe's own result
bounds the hypothesis before M36 starts: ten `dddiagnose` runs with every recorder pid in the
collision range all passed, so a pid in range does not by itself produce that binary's divergence.
One more thing M36 must know, from the fix wave: **the symptom is time-varying with the machine's
pid counter, not merely per-run.** The 10:16 probe's pids were `0xa2db`–`0xa425`, all in range;
the review's `sdi.c` probe an hour later ran at `0x10189` and the fix wave's re-run at `0x107dc`,
both *out* of range — the counter had crossed 65535 in between, so every record run made after
that point on this boot gets its `csops`/`PIDINFO` answers until the counter wraps. M36 records
the pid beside each row rather than assuming a run's pids share a half.

### The sweep

Spec §10 predicted the tally unmoved at `pass=46 fail=8 skip=0` — for the reason that is true, no
recorded byte changes on any corpus call; its first draft's reason, "no corpus call is in the
regime the rows change", was false (the clamp fires at #24 on every dynamic guest, above) — and
M33's baseline, the same tally, was the comparison. Run once at Task 4 Step
4 on the `486bab2` binary and, after the gate, once more.

**Run 1** (`sweep.log`): `TALLY pass=45 fail=9 skip=0`. The FAIL set: `/bin/csh` and `/bin/tcsh`
(recorder panicked), `/bin/launchctl`, `/usr/bin/automationmodetool`, `/usr/bin/desdp`,
`/usr/bin/dyld_info` and `/usr/bin/flex` (replay diverged), `/usr/bin/yes` (timed out after 30 s
recording) — M33's eight — **plus `/usr/bin/dddiagnose` (replay diverged)**, the row the README
already lists as the one intermittent failure and which M33's own section records "on a pass".

**Probe** (`ddd-probe.sh`, `ddd-probe.log`): `/usr/bin/dddiagnose` recorded and replayed five
times on the M34 binary and five times on the pre-M34 binary (the main checkout's build of
`e194b68` plus docs), each run logging the recorder's pid. **10/10 PASS**: every run `rc=139
rp=139` with byte-identical stdout (`dddiagnose` faults identically on both sides, which the sweep
counts as a pass). Recorder pids 41691–42021 (`0xa2db`–`0xa425`), **all inside
`[0x4000, 0x10000)`**, the §4b collision range.

**Run 2** (`sweep2.log`, after the gate): `TALLY pass=46 fail=8 skip=0`, the FAIL set
byte-identical to M33's eight, `dddiagnose` PASS.

**Controller's ruling, ledgered:** the run-1 ninth failure is the documented intermittent, not an
M34 regression. Evidence: not reproducible on either binary (10/10), and `dddiagnose`'s census
rows — `proc_info` 64/56/368, `csops` 4 and 1032, the §4 table — all sit inside the flat window,
so M34's rows change nothing it records. (Strengthened by the fix wave's correction rather than
weakened: M34's one corpus-visible mechanism change on `dddiagnose` is the 368 → 128 clamp at its
#24, which the kernel rejects before reading the length, so it cannot produce a divergence.)
Not a charter halt: that is a regression surviving a diagnose-edit-rerun cycle, and this one did
not survive the diagnose step. Cost if wrong: an
M34-caused flake in `dddiagnose` enters `main` labelled pre-existing; M36's per-row pid recording
is the check. The probe script is committed by nothing — it lives in the SDD workspace — and M36
should adopt its pid-logging shape. Against the prediction: run 2 landed on it, run 1 did not.

### Gate

**572 passed / 0 failed / 2 ignored across 124 test binaries.** Every chunk's cargo exit code was
captured to a file before any pipe (`gate/*.exit`): `ws=0 box=0 e2e1=0 e2e2=0 e2e3=0 bins=0
clippy=0`. Logs were sanitised (`LC_ALL=C tr -cd '\11\12\15\40-\176' | sed 's/\x1b\[[0-9;]*m//g'`)
before parsing. Wall clock 10:13:55 → 10:32:42 EDT, 2026-09-13. The chunks, in M33's shape, with
the sum of each chunk's `test result:` lines:

| chunk | invocation | binaries | passed | notes |
|---|---|---|---|---|
| `ws` | `cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1` | 26 | 152 | whole packages, so every library crate's `Doc-tests` harness ran (six: `hv_sys`, `retrace_arch`, `retrace_core`, `retrace_guest`, `retrace_sim`, `retrace_trace`) |
| `box` | `cargo test -p retrace-box --no-fail-fast -- --test-threads=1` | 37 | **270** | 1 lib + 35 integration + `Doc-tests`; M33: 268 |
| `e2e1`–`e2e3` | `cargo test -p retrace --test <20 names> --no-fail-fast -- --test-threads=1`, three groups of twenty, chunked index-free (`xargs -n20`) with the flattened membership `diff`ed against the sorted target list before anything ran | 20 + 20 + 20 | 45 + 36 + 58 | the 2 ignored are in `e2e3`: `stackoverflow_rust_e2e`'s `a_rust_stack_overflow_strikes_its_own_guard_page` (M21 wall) and `symbols_e2e`'s `cache_symbol_e2e` (M19 wall) — the same two as M33; nothing parked, nothing un-parked |
| `bins` | `cargo test -p retrace --bins --no-fail-fast -- --test-threads=1` | 1 | 11 | the `debug.rs` unit tests, the chunk CLAUDE.md says never to omit |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` | — | — | clean |

152 + 270 + 45 + 36 + 58 + 11 = 572. `jq_e2e`, `jq_file_e2e` and `cpython_e2e` ran — zero
`SKIPPED` lines across the three e2e logs.

**Reconciled against M33's 570 / 0 / 2 over 124, file-by-file**, every `.rs` under `crates/`
diffed against `main`; only three files changed at all, and only one's count moved:

| file | M33 | M34 | delta |
|---|---|---|---|
| `crates/retrace-arch/src/lib.rs` | 36 | 36 | 0 (rows and comments only) |
| `crates/retrace-arch/tests/legacy_equivalence.rs` | 3 | 3 | 0 (three `EXPECTED_DIFFS` entries, no test) |
| `crates/retrace-box/tests/truncguard.rs` | 19 | 21 | **+2** — `the_window_widens_for_the_m34_rows_and_not_for_getattrlist` (Task 2), `the_clamp_reaches_proc_info` (Task 3) |

No other file's count moved; binaries 124 → 124; `--bins` 11 → 11. The tree holds **572**
`#[test]` attributes = 570 runnable + 2 ignored (M33: 570 = 568 + 2). The run reports 572 passed
= 570 + 2, because `census.rs`'s two tests execute in two binaries (its own and
`legacy_equivalence`'s `#[path]` include) — the same "+2 twice" M33's section explains, so the two
572s are, as the two 570s were, a coincidence of the same +2 rather than one number derived twice.
The prediction made from source before the run was 572 / 0 / 2 over 124; the run matched it
exactly.

The fix wave after the final review adds **no** `#[test]` and no binary — its five view pins went
into two existing `retrace-arch` tests (36 → 36) and `truncguard.rs` stays at 21 — so the tree
still holds 572 attributes and the prediction for the post-wave gate is unchanged at 572 / 0 / 2
over 124; the targeted runs were `cargo test -p retrace-arch` (36 + 2 + 5 + 0 doc) and
`cargo test -p retrace-box --test truncguard` (21), both green, and
`cargo clippy -p retrace-arch -p retrace-box --all-targets -- -D warnings` clean. The controller's
full re-run after the wave is the record for the merged figure.

### What stays owed

* **The §4b pid-collision probe** — a fix milestone with a `Scalar` audit of the whole table as
  its task 1 (the audit is the precondition; M33's "reviewer-verified only" is why), the fix being
  `forward_and_diff` skipping the probe at `Scalar` positions, and the `hello_dyn` table above as
  its positive control (#145 returns 0 with 4 bytes captured). M36 to record the recorder's pid
  beside each sweep row, in `ddd-probe.sh`'s shape, and to test the hypothesis that the
  intermittent `dddiagnose` row is pid-shaped — knowing already that ten in-range pids all passed.
* **A `bigcsops`-shaped guest**, only if a later milestone finds a real guest whose `CS_OPS_BLOB`
  exceeds 64 KiB; none in the corpus does (a blob over 64 KiB needs a binary of more than 2,000
  pages), and a fixture whose only content is its size is what §7 declined to add.
* **`SET_DYLD_IMAGES` (336/15) serviced above the trace, not forwarded.** The forwarded call
  names *retrace's* task; today it fails `EINVAL` for a pid-independent reason
  (`task_set_dyld_info`'s three-call rule, §4b as corrected), and if it ever could succeed it
  would point retrace's own dyld info at guest memory. Synthesise `0` (dyld ignores the return) —
  the same family as every "on self" `proc_info`/`csops` being answered about retrace's process
  rather than the guest's, which the `Scalar`-audit fix above does not address either.
* **The per-page cache backing clamps any `Dest` destination that straddles a 16 KiB shared-cache
  boundary.** Measured at #24 (368 → 128), harmless there only because that call transfers
  nothing. The same geometry would truncate a `CS_OPS_BLOB`, a `LISTPIDS` or a `read` into a
  cache-DATA buffer that crosses a page — a fidelity hazard that pre-dates M34 and belongs to
  every `Reg`-clamped row, whose fix is contiguous host backing for the shared-region window,
  not a table change. No corpus guest does it today.
* **`getattrlistbulk` (461) and `getattrlistat` (468)** — neither in the census, so neither has a
  row (spec §7); the first that a guest issues is refused by name as a syscall with no
  `arg_kinds` row (M33) and gets its row then, with Ruling 1's bound to check against.
* **Of the four review minors the ledger deferred**, two are fixed by the fix wave (`6db8a27`):
  `tools/destgaps-census.sh` now installs its cleanup trap before the `cp`/`codesign`, and
  `the_clamp_reaches_proc_info`'s precondition loop is `(0..8).filter(|&i| i != 4)`. One owes
  nothing: a comment-wording note (the window test's intro paraphrases one non-citation sentence
  of its brief where the report said "verbatim"). One remains, documented in the test: that test
  leans on the host running more than 16 processes. (The `Reg` arm's missing diagnostic channel
  — the review's I1a — is not owed: it landed in this wave as `RETRACE_REGCLAMP`.)
* **Everything M33 left owed and M34 did not touch:** the per-argument canary fill and M32's
  Control 1 (still unexecuted, still inert); nested-pointer translation (`NestedSource` forwarded
  untranslated, `NestedDest` refused, the `DTRACEHIOC_ADDDOF` residual); `pipe`'s return
  (`Ret::FdPair` is documentation, `x1` uncaptured); the `execve`/`posix_spawn` fail-loud assert
  (M33 Ruling 7, the operator's call); console `writev` mirroring; `__disable_threadsignal` (331);
  `AT_FDCWD` in the 32-bit form real guests pass (M33 Ruling 10); M35's two holes — the
  replay-side `.min(avail)` and the `if !err` gate; the corpus bias (every governed `mach_msg2`
  still init-time and shallow); the `unexercised` label enforced against a census dated
  2026-09-12 for syscall numbers, and now a length census dated 2026-09-13 for these five, both
  snapshots; and the `kqueue` cross-version note. `Scalar`-versus-`Ptr` is no longer a standalone
  item: it is the pid fix's precondition, above.

## Status: M35-errholes — a failing syscall writes after all, and a clamp that hid the proof

M34 closed with the two holes M27 and M28 had left still standing in its owed list, carried
there by name from M30's sixth owed entry: `Box_::diff_memory`'s replay-side `.min(avail)` clamp,
and the `if !err` gate that skipped write capture — and the guard band with it — on a failing
syscall. The charter gave M35 medium certainty because "whether either [fixture] *reaches* the
`if !err` gate with a pointer argument worth banding is a measurement M35 still owes". The
measurement was taken before the spec was written, on the pre-M35 binary and the M28 fixture that
already existed, and it moved the milestone from "measure and decide" to "fix": **`failsysctl`
records cleanly and its replay diverges**, at the `oldlen` cell, because xnu's `sysctl()` writes
`*oldlenp` back on the `ENOMEM` path and the gate threw that write away. M28's "the kernel wrote
nothing, before or after" was true of the one buffer its test read and false of the call — its
test read `buf` and never `oldlenp` — and the README had carried the true-of-one-buffer sentence
as if it were about the call since M28. So M35 took the charter's Branch A on evidence: the
capture loop now runs on both paths (H2), `diff_memory` returns a divergence naming the recorded
length and the backing instead of comparing the part that fits (H1), and a second fixture
(`failproc`) pins the *data* half — a `kern.proc.all` that copies 648 bytes out and *then* fails.
No format change, no `retrace-core` edit: replay's generic arm has applied `writes` beside
`err = true` since M0 and had simply never received one. Taking the sweep also found that M34's
account of the `dddiagnose` row was the "right conclusion, wrong supporting fact" class again,
corrected below rather than carried.

The milestone's own numbers: **two** holes closed in **two** functions of one file
(`crates/retrace-box/src/lib.rs`); **one** existing test inverted by its own message
(`failwrite.rs`, 1 → 1); **one** new guest (`failproc.s`, 60 lines) and **one** new e2e binary
(`failsys_e2e.rs`, 2 tests); **three** positive controls, each run red then green with the red
pasted; `truncguard.rs` 21 → 22; **0** of 34 and **0** of 32 `err = true` landmarks carrying
writes on the two real guests measured (per-recording counts; the zero reproduces at 0 of 30 and
0 of 27); the sweep at `pass=45 fail=9 skip=0`, no binary moved in either direction; the gate
**575 / 0 / 2 over 125**, matching the prediction made from source. `TRACE_MAGIC` did not move,
no trap arm was touched, no `retrace-core` line changed, and nothing was parked or un-parked.

### What it set out to do

The charter's entry, `docs/superpowers/specs/2026-09-09-retrace-m32-m38-program-charter-design.md`
§3, "M35 — `errholes`: the two holes M27 and M28 left", quoted:

> **Discharges:** M30's owed-list, sixth entry.
>
> 1. `Box_::diff_memory`'s `.min(avail)` clamp on the **replay** side.
> 2. The `if !err` gate that skips write capture — and band evaluation — on a **failing**
>    syscall. The standing assumption is that a failed syscall writes nothing; M28 flagged it
>    unmeasured.
>
> **Plan certainty: medium.** Half 2 needs a guest that fails syscalls deliberately.
>
> **CORRECTED 2026-09-09:** an earlier draft of this entry said that guest "does not exist and
> must be written." It does exist — `crates/retrace-guest/asm/failsys.s` opens
> `/no/such/retrace/path` and exits with the errno, and `failsysctl.s` is a second one. Whether
> either *reaches* the `if !err` gate with a pointer argument worth banding is a measurement M35
> still owes, so the certainty stays medium; but the milestone starts from a fixture rather than
> from nothing, and it is no longer obviously the queue's most likely halt point. It stays last
> in the soundness phase regardless, since a halt there still banks M32–M34 as merged work.

What each hole cost, at the M34 merge (`8854146`; the spec's line numbers are at that commit and
have since moved — the citations below are at `12ac4e7`, the final code commit):

| hole | where (M34 merge) | code then | what it cost |
|---|---|---|---|
| **H1** replay-side clamp | `diff_memory` (`:3924`), the clamp at `:3930` | `let n = r.bytes.len().min(avail);` | a recorded region longer than its replay backing was compared only up to the backing and the excess **silently accepted** — the one place the terminal full-memory oracle could return `None` on bytes it never looked at |
| **H2** the error-path skip | `forward_and_diff` (`:3169`), the gate at `:3439`, its brace at `:3559` | `if !err {` around the whole post-diff capture loop | a kernel write made by a **failing** syscall was never captured, so replay never applied it; the guard band inside the same block was never evaluated on that path either |

H1's history: flagged in M1's own branch review, deferred at M2 "where only the clamp half was
paid" (`docs/status-log.md:5116–5118`), carried by M27, M28, M30, M33 and M34 as "still unpaid".
Its mirror on the *apply* side was already loud — `write_guest` asserts `bytes.len() <= avail`
with "overruns backing" (`:3648–3652` at `12ac4e7`) — so `diff_memory` was the odd one out. H2's
comment stated the assumption as a fact: "A failed syscall (carry set) wrote nothing to the
guest's buffers, so skip the post-diff write capture entirely." M27 narrowed it (`ps`'s 83
`sysctl`s all `err=false`, `:5156–5157`), M28 measured one case and found `buf` unchanged
(`:5304`), and nothing since had touched it.

### Ruling 1 — a fix milestone, not a measurement milestone

The spec's ruling, verbatim:

> **Ruling 1: M35 is a fix milestone, not a measurement milestone.** The charter's stated
> uncertainty ("whether either reaches the gate with a pointer argument worth banding") is
> resolved by §4's measurement in the direction that makes the fix mandatory: an existing fixture
> replays with a divergence exit (rc 3) on the pre-M35 tree. Under the charter's §1 argument
> (soundness before breadth) a known bit-for-bit replay failure on a repo-owned guest is the one
> thing the soundness phase exists to remove. Cost if wrong: none identified — the fix is three
> lines of control flow whose symmetry argument (§8) holds by construction.

Two further rulings shaped the code. **Ruling 2:** `diff_memory` returns a divergence rather than
panicking, because a compare that cannot complete has a caller (`ReplaySession`'s three terminal
arms) that already turns `Some` into the exit-3 path with the message printed — the Task 1 review
checked all seven callers and found none that treats `Some` as anything but divergence.
**Ruling 3:** the band assert goes live on the error path with no exemption list; a binary the
sweep turned red on an error-path band hit would have become an M36 row, not an M35 exemption.
None did (below).

### The measurement (spec §4)

**§4a — the existing fixture replays with a divergence.** Binary: the main checkout's build at
`e194b68` plus the M34 docs commits (pre-M34 code; the gate is identical at M34's merge), copied
to the scratchpad and ad-hoc signed. Guest: the M28 fixture `failsysctl`
(`crates/retrace-guest/asm/failsysctl.s`: `sysctl(kern.ostype)` into a 2-byte buffer, then
`write(1, buf, 2)`, then `exit(0)`). Verbatim:

```
retrace record  <OUT_DIR>/failsysctl -o failsysctl.bin    → rc=0, stdout = 00 00
retrace replay  failsysctl.bin                            → rc=3
DIVERGENCE at landmark 4 pc=0x1000003ec: memory divergence at ipa 0x100004010: replay=0x02 recorded=0x00
```

`0x100004010` is `oldlen` (`mib: .space 16` at `0x100004000`, `oldlen: .space 8` at
`0x100004010`, `buf: .space 64` at `0x100004018`). The recording's final snapshot has
`*oldlenp = 0`; the replay still has the `2` the guest stored. The failing `sysctl` wrote eight
bytes of guest memory and the `if !err` gate dropped them. M28's `buf_changed=false` is still
true — `buf` is untouched — and its conclusion about the call was wrong, because its test read
`args[2]` (`buf`) and never `args[3]` (`oldlenp`). **M28's Task 4 conclusion ("the kernel wrote
nothing, before or after", `docs/status-log.md:5304–5313`) and the Branch B decision built on it
(`:5315–5321`) are superseded here**; M28's section stands as written, with this pointer, and is
not edited.

**§4b — why, from xnu** (`bsd/kern/kern_newsysctl.c`, apple-oss-distributions `main`):

- `sysctl_old_user` (`:1637`): `if (req->oldlen - req->oldidx < l) return ENOMEM;` **before** the
  `copyout` and before `req->oldidx += l`. So the data buffer is untouched and `oldidx` stays 0 —
  M28's datum, correct as far as it went.
- `userland_sysctl` (`:2175`): `if (error && error != ENOMEM) return error;` — `ENOMEM` falls
  through — then `*retval = req2.oldidx > req2.oldlen ? req2.oldlen : req2.oldidx`, i.e. `0`.
- `sysctl` (`:2014`, the syscall entry): the same `ENOMEM` pass-through, then unconditionally
  `suulong(uap->oldlenp, oldlen)` — **`*oldlenp` is written back on the `ENOMEM` path**, with the
  value the handler left in `oldidx`.

So every `ENOMEM` from `sysctl` writes `*oldlenp`. That is the `DerefU64(3)` pointer M29 put in
the table for exactly this syscall — the argument M29 refused to clamp because "the kernel also
writes it back" — and the one argument M28's measurement did not read. Task 2 confirmed the
reading in-process before any edit: on the unmodified tree, `failwrite.rs`'s two measurement
assertions (`buf` unchanged; `*oldlenp == 0` through `read_bytes_for_test`) both **passed**, and
only the capture assertion failed (below).

**§4c — the data half, measured on the host** (`kpa.c`, a plain process, no retrace). Handlers
that emit through repeated `SYSCTL_OUT` calls write what fits and *then* fail. `kern.proc.*`
(`bsd/kern/kern_sysctl.c`, `sysdoproc_callback` `:810–841` copies out each `kinfo_proc` while
`buflen >= sizeof_kproc`; `sysctl_prochandle` `:845–945` returns `ENOMEM` when
`needed > oldlen`, before `req->oldidx += req->oldlen`). Verbatim:

```
sysctl({CTL_KERN, KERN_PROC, KERN_PROC_ALL}, 3, buf, &len=648, NULL, 0)
sizeof(kinfo_proc)=648 ret=-1 errno=12(Cannot allocate memory) oldlen_after=0
bytes_changed_in_first_648=648 bytes_changed_past_648=0
```

A failing call that writes **648 bytes of data** into the guest's buffer, plus `*oldlenp` → 0,
and nothing past the buffer. This is the data-half positive control (Control 3, below). A third
family exists in source and is **not** measured: `csops`' blob operations (`kern_proc.c`
`csops_copy_token` `:3511–3535`) copy out an 8-byte length header and return `ERANGE` — but
only when `8 <= usize < length`; a `usize` below 8 returns `ERANGE` with **no** write, so a probe
sized that way shows nothing and could be mistaken for the no-blob branch. A probe on an
unsigned binary took the no-blob branch and wrote nothing, so no control is built on it — owed,
below.

**§4d — what the corpus does on the error path** was not censused by the spec (M30 measured 36
error-path *restores* on one `jq` run — windows, not landmarks — with every capture skipped); it
was measured by the hoist itself, in "What the hoist recorded on real guests" below.

### What changed

Five code commits — `0beb06d` (Task 1, H1 and Control 1), `955d687` (its fix round: the control
relocated beside `the_clamp_reaches_proc_info`, a pure relocation the controller checked with a
line-multiset diff, 34/34, instead of a re-review), `a11e398` (Task 2, H2 and `failwrite.rs`),
`8a53c53` (its fix round: the last two "wrote nothing" comments retired, 3 insertions / 6
deletions, checked line by line against the review's three items instead of a re-review),
`12ac4e7` (Task 3, the fixture and the e2e) — and three docs commits: `504753e` (this section,
the README and spec §11), `46b5dc0` (the Task 4 fix round: the `dddiagnose` wall is the
RCV-shaped `mach_msg2`, not the serviced refusal) and the final-review fix wave's two —
`e527831` (comments and one assertion message in `crates/`, no code path) and `0708960` (this
section's dddiagnose correction) — plus one commit after the scoped re-review, comments and this
sentence only, which cannot name itself. Every citation below is at `12ac4e7`.

- **`crates/retrace-box/src/lib.rs`** — two functions, plus comments.
  - `diff_memory` (`:3933`): the clamp is gone; `if r.bytes.len() > avail` (`:3948`) returns
    `Some("recorded region at ipa {:#x} is {} bytes but its replay backing holds only {} from
    that address — the recording and the replay disagree about the guest's memory layout, which
    no byte compare can settle")`, naming the three numbers, and the compare runs over the whole
    recorded region or not at all. **Reachability, stated honestly:** on record every captured
    region lies inside one backing (`host_span` bounds every window by `avail`), and replay
    rebuilds the same backings from the same snapshot and the same deterministic demand-paging,
    so on a *correct* replay the branch never runs. It exists for the incorrect one — a layout
    drift, a checkpoint restored against a different backing set, a future edit to
    `page_in_cache` — and Control 1 proves it fires; nothing in the corpus reaches it.
  - `forward_and_diff`: `if !err {` is now a bare block (`:3447`; formerly `:3439` on `main`),
    the loop body untouched and not re-indented, so `blame` keeps its history — the comment
    says so (`:3444–3445`). The new comment above it (`:3434–3445`) states what was measured
    (§4a, §4b, §4c) and why the capture is unconditional: the pre-image and the canary fill are
    taken before `host_svc` on both paths, the restore below already ran on both, and replay's
    generic arm has applied `writes` beside `err = true` since M0 — "what was skipped was the
    looking." The Task 2 review read `forward_and_diff` (`:3167`) end to end: the loop body
    references `num`, the five window fields, `fill_canary`, `self.host_span` and
    `self.canary_disturbances`, and never `ret` or `err`. The M30 restore comment's point 2 no
    longer says "wrote nothing", and its opening clause (`:3569`) no longer names a gate that
    does not exist. `read_bytes_for_test`'s doc (`:3042–3046`, the seam at `:3047`) now says why
    the seam still exists — the test wants a view independent of the capture — rather than
    "that path captures nothing". And one edit
    neither spec nor plan listed, found by the Task 2 review: `forward_and_diff`'s M0-era rustdoc
    ("On error (`err`) no writes are captured — a failed syscall wrote nothing") had been glued
    onto the top of `translate_fds`'s doc block when M10 inserted `translate_fds` between them —
    a false public contract on the wrong function, rendered by `cargo doc`. Deleted whole;
    `forward_and_diff` keeps its own doc. After `8a53c53`, `grep -nE 'wrote nothing|if !err'`
    hits only the retraction inside the new comment (`:3435–3436`) and the two fd-bookkeeping
    gates (`:3608`, `:3614`), which stay: a failed `open` allocates no slot and a failed `close`
    retires none — the kernel's contract, not an assumption about memory (spec §3c).
- **`crates/retrace-box/tests/failwrite.rs`** — `a_failing_sysctl_is_measured_for_writes`
  (`:17`) rewritten in place (1 `#[test]` → 1). It now asserts the call, not the buffer: `err`
  and `ret == 12`; `buf` (64 bytes at `args[2]`) unchanged through the seam — M28's datum,
  still true; `*oldlenp == 0u64.to_le_bytes()` through the seam — the §4b write-back; and some
  captured region covering `args[3]..+8` carrying those same bytes — the seam and the capture
  agreeing is the point. The header comment says M28 measured `buf`, M35 measured the call.
- **`crates/retrace-box/tests/truncguard.rs`** — one test,
  `a_recorded_region_longer_than_its_replay_backing_is_a_divergence` (`:292`, 21 → 22), placed
  after `the_clamp_reaches_proc_info` whose `STACK_TOP_IPA − 64` setup it reuses: 64 bytes of
  backing, read through `read_bytes_for_test`, plus 64 zero bytes with nothing behind them, as
  one 128-byte `Region`; `diff_memory` must return `Some(msg)` with "128 bytes" and "holds
  only 64". Control 1.
- **`crates/retrace-guest/asm/failproc.s`** (new, 60 lines), **`build.rs:96–104`**,
  **`src/lib.rs:140`** (`FAILPROC`) — the data-half fixture: `sysctl({1, 14, 0}, 3, buf,
  &oldlen=648, NULL, 0)` with `buf: .space 648` exactly, then `write(1, buf, 8)` — the first
  eight bytes of the first `kinfo_proc`, so a dropped capture is visible as *output* (the
  `bigread` shape) and not only at the terminal compare — then `exit(0)`. The 648 bytes are host
  state (whichever process the kernel iterates first), recorded and replayed as `task_info`'s
  audit token is: forwarded-and-recorded, never regenerated. Built exactly as `failsysctl` is.
- **`crates/retrace/tests/failsys_e2e.rs`** (new, 86 lines): `captured` (`:23`), `the_sysctl`
  (`:32`), `a_failing_sysctl_replays_bit_for_bit` (`:46`) on `FAILSYSCTL` and
  `a_failing_proc_list_replays_bit_for_bit` (`:65`) on `FAILPROC`, both through the CLI via
  `util::record` / `util::replay`. Each asserts on the landmark itself — `err: true` **and** a
  captured region covering the bytes the kernel wrote (`*oldlenp` = 0; for `failproc` also the
  648 bytes at `buf`, whose first eight equal the guest's stdout and which are asserted not
  all-zero, since a `kinfo_proc` is never all-zero) — and only then on replay rc 0 and stdout
  equality. Exit codes alone would not do (CLAUDE.md): a replay that applies nothing exits 0 on
  any guest whose divergence the terminal compare cannot see.
- No `retrace-core` edit, no `retrace-arch` edit, no trap arm, no `TRACE_MAGIC` bump. Symmetry
  (spec §8) holds without an edit: the record side now emits `Event::Syscall { err: true, writes:
  non-empty }` and the replay side's generic arm has always applied `writes` before feeding
  `(ret, err)` — the Task 2 review confirmed `retrace-core/src/lib.rs:2358` and every sibling
  pass `*err, writes` straight through, `apply_and_return` (`retrace-box:3621`) loops over
  `writes` with no `err` test, and the only `err`-conditioned code in core is task-port and fd
  bookkeeping. A pre-M35 recording replays under M35 code exactly as it did under M34 (its
  failing landmarks carry empty `writes`). This is the M2-taskinfo posture, not a format change.

### Positive controls, run and recorded

Each mutation was applied, its red quoted from the run that produced it, and reverted before the
commit — the revert confirmed from `git diff` before staging.

- **Control 1 — H1 fires** (Task 1 Step 2, the test written first against the `.min(avail)`
  clamp). RED at the `expect`, `diff_memory` having returned `None`:

    ```
    thread 'a_recorded_region_longer_than_its_replay_backing_is_a_divergence' (31226460) panicked at crates/retrace-box/tests/truncguard.rs:58:40:
    a 128-byte recorded region over a 64-byte backing must be reported as a divergence, not compared up to the backing and passed — that silence is the M1 hole this test closes
    test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 21 filtered out; finished in 0.00s
    ```

  The old clamp compared the 64 bytes that match and never looked at the 64 that have no backing
  to compare against — the hole's silence, reproduced on purpose. Green after the fix
  (`truncguard`: 22 passed; `checkpoint`: 1 passed — the correct-replay path still returns
  `None`).
- **Control 2, unit half — the `oldlenp` write-back** (Task 2 Step 2, `failwrite.rs` rewritten
  and run on the unmodified `lib.rs`). RED at the capture assertion **only**:

    ```
    thread 'a_failing_sysctl_is_measured_for_writes' (31249284) panicked at crates/retrace-box/tests/failwrite.rs:55:17:
    assertion `left == right` failed: forward_and_diff captured no write covering *oldlenp on a failing syscall: the `if !err` skip is dropping a real kernel write (writes captured: 0)
      left: None
     right: Some([0, 0, 0, 0, 0, 0, 0, 0])
    ```

  The `right` is `oldlen_after` read through the seam, so the two measurement assertions before
  it — `buf` unchanged, `*oldlenp == 0` — passed on the tree that still skipped the capture:
  §4b confirmed in-process. Green after the hoist (1 passed), and the whole `retrace-box` chunk
  beside it: `binaries=37 passed=271 failed=0 ignored=0`, rc 0, including
  `a_failing_syscall_still_restores_the_canary` (which now runs the band *check* on the error
  path before the restore it tests) and
  `a_duplicated_pointer_argument_does_not_manufacture_a_disturbance`.
- **Controls 2 (e2e half) and 3 — the data half** (Task 3 Step 5; the gate reinstated by hand,
  `{` → `if !err {`, rebuilt, then reverted with `git checkout`). RED, all three:

    ```
    thread 'a_failing_proc_list_replays_bit_for_bit' (31310952) panicked at crates/retrace/tests/failsys_e2e.rs:74:10:
    the landmark must carry the 648-byte record the kernel copied out on its way to ENOMEM
    
    thread 'a_failing_sysctl_replays_bit_for_bit' (31310975) panicked at crates/retrace/tests/failsys_e2e.rs:54:5:
    assertion `left == right` failed: the landmark must carry the kernel's write-back of *oldlenp; without it replay keeps the guest's 2 and diverges at ipa 0x100004010 — the pre-M35 measurement. writes: 0
      left: None
     right: Some([0, 0, 0, 0, 0, 0, 0, 0])
    test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.27s
    ```

  and `failwrite` red as above. Then the hand-run replay of a `failsysctl` recording made under
  the mutation, through a throwaway signed copy of the CLI:

    ```
    $ ./retrace-handrun-copy replay /tmp/failsysctl-handrun.bin
    DIVERGENCE at landmark 4 pc=0x1000003ec: memory divergence at ipa 0x100004010: replay=0x02 recorded=0x00
    ```

  (exit 3) — the spec's §4a line, byte for byte: the milestone's own premise reproduced by its
  own fixture. `git checkout crates/retrace-box/src/lib.rs`, `git diff --stat` empty, then both
  files green (`failsys_e2e`: 2 passed; `failwrite`: 1 passed).

### What the hoist recorded on real guests (Task 2 Step 5)

With the capture — and the band — live on the error path for the first time, `hello_dyn` and
`jq --version` were recorded and replayed through a signed copy of the Task 2 CLI: **all four
exits 0** (`hello_dyn` record 0 / replay 0; `jq` record 0 / replay 0), **`[M30 CANARY]` lines 0
and 0**, recorder stderr only the usual `dyld __mac_syscall(Sandbox)` pair, the four
allowlist-forward lines and `fall-throughs: 0`. The canary line prints *ahead of* the assert, so
zero lines plus rc 0 means no band was disturbed on either path — Ruling 3's live band did not
fire. Traces: `hello_dyn` 29,018,547 bytes; `jq` 33,315,627 bytes. The Task 2 reviewer
reproduced the four exits and the two zeros independently.

**Landmark counts, per recording.** The debug CLI lists no landmarks, so a 30-line scratchpad
reader over `retrace_trace::Reader::open` (committed by nothing) counted them: on that `jq`
recording, **0 of 34** `err = true` landmarks carry writes (302 events, 299 syscalls; the failing
calls, by syscall number, are `open` (5), `access` (33), `crossarch_trap` (38), `ioctl` (54),
`csops` (169), `csops_audittoken` (170), `shared_region_check_np` (294), `proc_info` (336),
`stat64` (338), `__mac_syscall` (381), `csrctl` (483) and `map_with_linking_np` (550)); on that
`hello_dyn` recording, **0 of 32**; and, counted after the fact by the final review on the
thirteen kept `dddiagnose` traces (below), **0 of 63** and **0 of 75** — the zero holds on an
Apple binary too, not only on the two guests the repo can reach. The counts are properties of
the recordings, not the
binaries: the reviewer's own recordings from another cwd gave 0 of 30 and 0 of 27
(environment-sensitive `stat64` / `access` / `csrctl` failures, not a determinism issue). The
reproducible conclusion is the zero — the hoist recorded nothing new on either real guest, and
the difference it makes is visible only on a guest whose failing syscall writes.

**That zero is a measured zero** — M29's lesson, applied: before trusting it, the `failsysctl`
fixture was recorded through the *same* signed CLI. Replay rc **0** (spec §4a measured rc 3 before
the fix), `[M30 CANARY] count: 0`, and its one `err = true` landmark carries **two** captured
regions: `mib`'s `Ptr` window at `0x100004000` (16,384 bytes — arg 0's window spans the whole
16 KiB `__DATA` page and so also covers `oldlen`) and `oldlenp`'s at `0x100004010` (16,368). Those
are the same window-sized post-images the success path produces — not "one region at `args[3]`
of 8 bytes", which spec §5b said and which the capture has never produced; the plan relaxed the
assertion to "some region covering `args[3]..+8`" and the tests assert that. (Task 2's report
first summed the two as "one 32,752-byte region"; 16,384 + 16,368 = 32,752, and its fix round
corrected it.)

**The `failproc` landmark** (Task 3, printed once through a temporary `eprintln!` and removed
before commit) carries **three** regions, all ending at `0x100008000`, the end of the `__DATA`
page's window:

| ipa | bytes | argument | how `diff_window` sized it |
|---|---|---|---|
| `0x100004000` | 16,384 | `args[0]`, `mib`, `Ptr` | `base = avail.min(PTR_WINDOW_CAP)` = `avail` |
| `0x100004018` | 16,360 | `args[2]`, `buf`, `Dest(DerefU64(3))` | `base.max(clamp_count(avail, 648))` = `base` = 16,360 — the table length never surfaces |
| `0x100004010` | 16,368 | `args[3]`, `oldlenp`, `Ptr` | `base` = `avail` |

**Spec §5d said the `DerefU64(3)` window "is exactly `*oldlenp` = 648" — wrong.**
`Box_::diff_window` (`:3084–3090`) computes `base = avail.min(window_cap)` and then, for a `Dest`,
`base.max(clamp_count(avail, len))`: a table length only *widens* a window past the flat cap and
never narrows it, and here `base` = 16,360 already exceeds 648. The conclusion (the region covers
the 648 bytes) holds; the supporting fact did not — the charter's class, again, inside the spec
that quotes the charter warning about it. (Task 3's report also labelled the `0x100004018` region
"a third `Ptr` window"; it is `args[2]`'s own `Dest` window, sized through the other branch of
`diff_window` to the same number — the Task 3 review corrected it.) All three regions are reads
of the same live memory at the same point, so which one `captured()` hits first is immaterial to
the bytes it returns; in practice it is `windows[0]`, `mib`'s, which alone covers everything both
tests ask for.

**One plan defect, for the next reader:** the plan's Step 5 script exits 126 on every line as
written, because `mktemp -t` pre-creates the file `0600` and `cp` onto an existing file keeps that
mode, so the signed copy has no exec bit (`tools/apple-sweep.sh` copies to a path `mktemp -d`
did not pre-create, which is why its pattern works). One `chmod +x "$BIN"` between the `cp` and
the `codesign` fixes it; the reviewer's independent re-run needed the same.

### The sweep, and a `dddiagnose` finding that corrects M34's characterisation

Spec §10 said any binary that moves in either direction becomes an M36 row, and the hoist's
predicted direction of change was red → green (a write the old tree dropped, now applied), never
green → red except through the band assert (Ruling 3).

**Run 1** (`sweep.log`, the `12ac4e7` binary, started 12:34): `TALLY pass=45 fail=9 skip=0`.
The FAIL set: M33's eight — `/bin/csh`, `/bin/tcsh` (recorder panicked), `/bin/launchctl`,
`/usr/bin/automationmodetool`, `/usr/bin/desdp`, `/usr/bin/dyld_info`, `/usr/bin/flex` (replay
diverged), `/usr/bin/yes` (timed out after 30s recording) — **plus `/usr/bin/dddiagnose` (replay
diverged)**, exactly M34's run 1. **No binary moved in either direction relative to M34**: the
hoist retired no sweep row and created none, and the band assert reddened nothing — spec §10's
"unchanged" case. That evidence cannot cover `csh` and `tcsh`, which are already "recorder
panicked" rows whose reason the sweep discards; the final review re-recorded both through a
signed copy of the HEAD CLI, and each still panics at the M33 `dup2` assert
(`crates/retrace-core/src/lib.rs:1140`) with no band text — so the live band fired on nothing
either of them issued before `dup2`. M36's E1 fix (print the panic reason) covers it going
forward.

**The `dddiagnose` probe, run twice, traces kept this time** (`ddd-probe-m35.sh`,
`ddd-probe.log`, `ddd-keep/`; `ddd-probe-m35-inrange.sh`, `ddd-probe-inrange.log`,
`ddd-keep-inrange/`, all in the SDD workspace, committed by nothing). What M34's section says
(`:7176–7181`): its morning probe at 10:16 recorded and replayed `dddiagnose` **10/10 PASS** on
both the M34 and the pre-M34 binary, every run `rc=139 rp=139` with byte-identical stdout, with
recorder pids `0xa2db`–`0xa425`, all inside the §4b collision range `[0x4000, 0x10000)` — and
it drew from that (`:7153–7154`) "a pid in range does not by itself produce that binary's
divergence". What today's probes measured:

- **First probe** (logged 12:40; five runs on the pre-M34 binary, `main`'s build, then five on
  the M35 binary), recorder pids `0x257f`–`0x2662`, all **outside** the range: **FAIL 10 of 10
  on both binaries**, every run `rc=4 rp=3`, the replay reporting `DIVERGENCE at landmark 3xx
  pc=0x1804adc34: expected recorded syscall, got None (truncated=false)` at landmarks 379–388.
  The recorder's own stderr names the cause — the sweep discards it, the probe kept it. M35
  run 1's two lines (the `dest` port name in the first varies per run — `0x1403`, `0x1203`,
  `0x1203`, `0x1903`, `0xf03` on the M35 runs; `0x1603`, `0x1403`, `0x1703`, `0x1403`, `0x1203`
  on the pre-M34 runs — the second line is identical on all ten):

    ```
    [retrace] refusing mach_msg2 message-queue send (msgh_id 0x400000cf dest 0x1403 send_size 248): the box hosts no message-queue receivers
    RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape
    ```

  Two lines, two routes, and only the second is a wall. The first is M23 t5's **serviced**
  refusal, `Route::RefuseMqSend` (`crates/retrace-core/src/machmsg.rs:104–107`, dispatched at
  `crates/retrace-core/src/lib.rs:508–525`): `MACH_SEND_INVALID_DEST` is returned, an
  `Event::Syscall { err: false, writes: [] }` is appended, both sides recompute the identical
  refusal, and the guest **continues** — the README's own sentence about the XPC pipe
  (`README.md:174–175`). The exit 4 is the second line, `Route::Unsupported`
  (`machmsg.rs:108–109`, `lib.rs:557–559`): `0x404000102` is `MACH64_SEND_MQ_CALL |
  MACH64_RCV_MSG` with **no `MACH64_SEND_MSG`** — a message-queue *receive*-shaped call, the
  shape M23's router comment (`machmsg.rs:100–103`) says "has never been observed" and leaves
  at the fail-loud default. **This probe is that shape's first sighting.** So "replay diverged"
  is the sweep **mislabelling a refused recording**: the recorder exited 4 at a fail-loud wall,
  the trace ends there with no terminal event, and the replay — correctly — runs out of events
  at the next syscall. Record and replay agree; the guest reached a shape retrace has never
  modelled.
- **Second probe** (logged 12:59, after the gate had advanced the machine's pid counter into
  the range; M35 binary only), pids `0x66bb`–`0x6723`, all **inside**: **2 PASS
  (`rc=139 rp=139`), 3 FAIL**. The failing runs again took M23's serviced refusal (a port name
  that varies per run: `0x1203`, `0x1403`, `0x1603`), survived it, and then hit a `brk` — M23's
  "the other four `brk` regardless of which of seven refusal codes is returned" class
  (`machmsg.rs:97–99`), a second, distinct post-refusal path: `RECORD ERROR: non-syscall exit:
  exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110`,
  the replay reporting the same exception at the same pc as its divergence (landmarks 370–379).
  All three kept `rec.err`s show the `refusing` line followed by that `RECORD ERROR` — the proof
  that the refusal is survived and is not the wall.

**Controller's ruling, ledgered — as corrected by the final review, which ran the check this
paragraph's first draft had named.** Not an M35 regression — the pre-M34 binary fails
identically today — and **not a charter class-E2 row**: within every run, record and replay
agree. What varies *between* runs is not "the guest's own path, which depends on inputs the
recorder forwards faithfully", which is what this paragraph said until the final review read the
thirteen kept traces; it is retrace's own M34 §4b defect deciding which wall the guest reaches.
Measured over all thirteen (`ddd-keep/`, `ddd-keep-inrange/`; a scratchpad reader over
`retrace_trace::Reader::open_checked`, the reviewer's table re-derived by the controller): every
out-of-range trace — 10 of 10, pre-M34 and M35 alike — has **63** `err = true` landmarks, with
`csops` (169/170) failing **0** times and `proc_info` (336) once (the pid-independent
`SET_DYLD_IMAGES` `EINVAL`, M34's #24), and reaches the RCV-shaped `mach_msg2` wall; every kept
in-range trace — 3 of 3, the FAIL runs — has **75** = 63 + **12**: `csops` 169 ×7, 170 ×1,
`proc_info` 336 ×4 more — exactly the twelve self-pid calls §4b predicts the per-register probe
mis-translates into `ESRCH` (the pid looks like a mapped low IPA and is forwarded as a host
pointer) — and then hits the `brk`; the two unkept in-range PASS runs crashed `rc=139` on both
sides. Across M34's ten in-range runs and today's five, **0 of 15 in-range runs reached the RCV
wall; 10 of 10 out-of-range runs did.** So the pid hypothesis is **confirmed as the driver of
which wall** the guest reaches; what stays open is only why in-range runs split between an
identical crash and a `brk`. The row's class is **B, known-unmodelled** — now with **three**
walls behind one row, in the order they gate each other: the §4b `Scalar` fix first (it decides
which of the other two a run reaches), then the RCV-only message-queue `mach_msg2` shape
`Route::Unsupported` keeps fail-loud (first observed here), then the post-refusal `brk` class
M23 parked. The serviced `RefuseMqSend` line is not a wall, and its fix is not "message-queue
receivers in the box". Three consequences for M36/M37: **(a)** the sweep's `rc=139 rp=139` PASS
rows are almost certainly retrace-*induced* crashes — a process told by `csops` that its own pid
does not exist — so M36's "identical crash counted PASS" E1 row is a mislabelled retrace defect,
not merely a missing note; **(b)** after the §4b fix `dddiagnose` will hit the RCV wall
*deterministically*, so route §4b before the RCV-shape decision and expect the row to go from
intermittent to always-FAIL until the shape is modelled; **(c)** M34's sentence "a pid in range
does not by itself produce that binary's divergence" is true only of the *RCV-wall* divergence
— an in-range pid does by itself produce the twelve `ESRCH` answers. Two things this corrects in
M34's record: **(1)** "recorded and replayed 10/10" was true that morning, and the probe was run
*after* the sweep had failed the binary, so what it measured was `dddiagnose`'s state at 10:16 —
ten in-range pids, ten identical crashes — not a property of the binary; **(2)** read now, those
ten are ten in-range runs that never reached the RCV wall, the first ten of the fifteen. Two
sweep-harness defects go to M36 as **E1** rows: a record exit of 4 is reported as "replay
diverged" (the record rc is the primary signal and is never printed), and an identical crash on
both sides is counted PASS (`rc=139 rp=139`) with no note — per (a), very likely retrace's own
crash. Cost if wrong: the same as before — a genuine retrace nondeterminism in `dddiagnose`
entering `main` under a class-B label; the kept traces in `ddd-keep*/` remain the check, any of
them can be replayed again, and the thirteen counted above are the ones that have been.

And two things it corrects in this milestone's own record — the charter's class, "right
conclusion, wrong supporting fact", recurring **twice in this one subsection**, both times in
the controller's numbers file from which the subsection was transcribed. First: the numbers
file read the `refusing` line as the wall and called it "unmodelled since M2-mach" — wrong on
both counts (the SEND|RCV message-queue shape has been serviced since M23; the RCV shape was
unobserved until today) and in contradiction of the README's own `:174–175`. The Task 4 review
caught it against `machmsg.rs` and the kept traces; the numbers file was corrected in place and
this subsection rewritten before it entered the log. Second: the corrected file, and the
subsection transcribed from it, then said the between-run variation was "the guest's own path
varying with inputs the recorder forwards faithfully" and that the pid hypothesis was "neither
confirmed nor refuted" — a reading of the two probes' pass/fail labels, not of their traces.
The final whole-branch review read the thirteen kept traces — the check this subsection's own
"cost if wrong" had named — and found the 63-versus-75 split above; the numbers file was
corrected a second time and the ruling rewritten in the final-review fix wave. Both times the
conclusion stood and the supporting fact did not; the second time the supporting fact was the
one the section had offered as its own check, before the check had been run. What made the
second correction possible at all was that the probe kept its traces.

### Gate

**575 passed / 0 failed / 2 ignored across 125 test binaries.** Every chunk's cargo exit code was
captured to a file before any pipe (`gate/*.exit`): `ws=0 box=0 e2e1=0 e2e2=0 e2e3=0 e2e4=0
bins=0 clippy=0`. Logs were sanitised (`LC_ALL=C tr -cd '\11\12\15\40-\176' | sed
's/\x1b\[[0-9;]*m//g'`) before parsing. Wall clock 12:39:28 → 12:58:12 EDT, 2026-09-13. Zero
`SKIPPED` lines: `jq_e2e`, `jq_file_e2e` and `cpython_e2e` all ran. The chunks, in M34's shape,
with the sum of each chunk's `test result:` lines:

| chunk | invocation | binaries | passed | notes |
|---|---|---|---|---|
| `ws` | `cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1` | 26 | 152 | whole packages, so every library crate's `Doc-tests` harness ran |
| `box` | `cargo test -p retrace-box --no-fail-fast -- --test-threads=1` | 37 | **271** | M34: 270 (+1, Control 1) |
| `e2e1`–`e2e4` | `cargo test -p retrace --test <names> --no-fail-fast -- --test-threads=1`, chunked index-free (`xargs -n20`) over the sorted target list, flattened membership `diff`ed against it before anything ran | 20 + 20 + 20 + **1** | 46 + 30 + 64 + 1 | **61 targets** (M34: 60), so a fourth group of one; the 2 ignored are in `e2e3`: `stackoverflow_rust_e2e`'s `a_rust_stack_overflow_strikes_its_own_guard_page` (M21 wall) and `symbols_e2e`'s `cache_symbol_e2e` (M19 wall) — the same two as M34; un-parked nothing, parked nothing. `failsys_e2e` ran in `e2e1`: both tests `ok` |
| `bins` | `cargo test -p retrace --bins --no-fail-fast -- --test-threads=1` | 1 | 11 | the 11 `debug.rs` unit tests, the chunk CLAUDE.md says never to omit |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` | — | — | clean |

152 + 271 + 46 + 30 + 64 + 1 + 11 = 575.

**Reconciled against M34's 572 / 0 / 2 over 124, file-by-file**, every `.rs` under `crates/`
diffed against `main`; six files changed, two counts moved:

| file | M34 | M35 | delta |
|---|---|---|---|
| `crates/retrace-box/src/lib.rs` | 13 | 13 | 0 (two functions, comments) |
| `crates/retrace-box/tests/failwrite.rs` | 1 | 1 | 0 (rewritten in place) |
| `crates/retrace-box/tests/truncguard.rs` | 21 | 22 | **+1** — `a_recorded_region_longer_than_its_replay_backing_is_a_divergence` (Task 1) |
| `crates/retrace-guest/build.rs` | 0 | 0 | 0 |
| `crates/retrace-guest/src/lib.rs` | 9 | 9 | 0 |
| `crates/retrace/tests/failsys_e2e.rs` | — | 2 | **+2** — new binary (Task 3) |

Binaries 124 → 125; `--bins` 11 → 11. The tree holds **575** `#[test]` attributes = 573 runnable
+ 2 ignored (M34: 572 = 570 + 2); the run reports 575 passed = 573 + 2 because `census.rs`'s two
tests execute in two binaries (its own and `legacy_equivalence`'s `#[path]` include) — the same
"+2 twice" M33's and M34's sections explain, so the two 575s are a coincidence of the same +2
rather than one number derived twice. Bare `grep -r '#\[test\]' crates | wc -l`: 573 → 576 (one
non-attribute match, as at M34). The prediction made from source before the run was 575 / 0 / 2
over 125; the run matched it exactly.

### What stays owed

* **`csops`' `ERANGE` header write, unmeasured** (spec §4c/§7). `csops_copy_token` copies out an
  8-byte length header and returns `ERANGE` only when `8 <= usize < length` — a `usize` below 8
  returns `ERANGE` with no write, so the owed control must ask for at least 8 bytes or it shows
  nothing; the probe took the no-blob branch on an unsigned binary. Reaching the branch needs a
  signed binary with a blob and a pid that survives M34's §4b, which is M36's measurement and
  M37's fix. The hoist would capture it now; nothing has shown it does.
* **The band's width, still** (M27 → M34's owed lists). The band is now *evaluated* on both paths;
  how much of the backing it looks at — one contiguous 64-byte run past the window — is
  unchanged, and widening it was deliberately not attempted (spec §7).
* **The sweep's two labelling defects — M36's E1 rows.** A record exit of 4 is reported as
  "replay diverged" (the record rc is never printed and is the primary signal), and an identical
  crash on both sides is counted PASS with no note — and on `dddiagnose` that crash is almost
  certainly retrace-*induced* (consequence (a) above: every in-range run answers its twelve
  self-pid `csops`/`proc_info` calls with `ESRCH` first), so the second row is a mislabelled
  retrace defect and not merely a missing note. The `dddiagnose` row has been read through both
  labels since M29; the kept traces in the SDD workspace are the evidence.
* **The three class-B walls behind the `dddiagnose` row, in the order they gate each other:
  M34's §4b, then the RCV-only message-queue `mach_msg2` shape, then the `brk` M23 parked.**
  §4b first because it decides which of the other two a run reaches (the ruling above: 0 of 15
  in-range runs got past their twelve `ESRCH` answers to the RCV wall; 10 of 10 out-of-range
  runs did) — its fix is the next entry, and once it lands the row should sit at the RCV wall
  every run. Then the shape: `Route::Unsupported` keeps the receive-shaped message-queue call
  (`MACH64_SEND_MQ_CALL | MACH64_RCV_MSG`, no `MACH64_SEND_MSG`) fail-loud because it "has never
  been observed" (`machmsg.rs:100–103`); it has now, so a decision is owed — refuse it
  deterministically as the SEND|RCV shape is, or model it. Then the other post-refusal path, a
  guest that survives the serviced refusal and `brk`s (`machmsg.rs:97–99`, M23's "other four"),
  parked there since M23 and reached today only by in-range runs. Neither of those two fixes is
  "message-queue receivers in the box": the serviced refusal is not a wall. Routed as the
  charter routes class B.
* **The §4b pid-collision probe** (M34), with M35's addendum: the `Scalar` audit as task 1, the
  fix being `forward_and_diff` skipping the probe at `Scalar` positions, the `hello_dyn` table as
  its positive control (#145 returns 0 with 4 bytes captured) — and `dddiagnose` as a second:
  the twelve self-pid `csops`/`proc_info` calls that answer `ESRCH` in every in-range trace (75
  `err = true` landmarks against 63) must answer `0` after the fix, from any pid. M36 to record
  the recorder's pid beside each sweep row — knowing now that the pid is confirmed as the driver
  of which wall `dddiagnose` reaches, and that what it does not yet explain is the
  crash-versus-`brk` split among in-range runs.
* **A `bigcsops`-shaped guest**, only if a later milestone finds a real guest whose `CS_OPS_BLOB`
  exceeds 64 KiB; none in the corpus does (M34).
* **`SET_DYLD_IMAGES` (336/15) serviced above the trace, not forwarded** (M34): the forwarded
  call names *retrace's* task, fails `EINVAL` for a pid-independent reason
  (`task_set_dyld_info`'s three-call rule), and would point retrace's own dyld info at guest
  memory if it could succeed. Synthesise `0`.
* **The per-page cache backing clamps any `Dest` destination that straddles a 16 KiB
  shared-cache boundary** (M34): measured at #24 (368 → 128), harmless there only because that
  call transfers nothing; the fix is contiguous host backing for the shared-region window.
* **`getattrlistbulk` (461) and `getattrlistat` (468)** (M34) — neither in the census, so
  neither has a row; the first a guest issues is refused by name and gets its row then.
* **Review minors carried:** M34's `the_clamp_reaches_proc_info` leans on the host running more
  than 16 processes (documented in the test). From this milestone's reviews: `failwrite.rs:23`
  carries M28's misnomer ("the full 64-byte backing" for a `.space 64`), harmless; the bare
  block in `forward_and_diff` could be dedented in a cleanup commit (its comment says why it is
  there); and `util::record` in `crates/retrace/tests/util/mod.rs` does not scrub `RETRACE_TRACE`
  from the environment as `run_env` does, so a failing assertion's `rec.stderr` carries the trap
  firehose if the test process has it set — pre-existing and shared by every e2e.
* **Everything M33 left owed and M34 and M35 did not touch:** the per-argument canary fill and
  M32's Control 1 (still unexecuted, still inert); nested-pointer translation (`NestedSource`
  forwarded untranslated, `NestedDest` refused, the `DTRACEHIOC_ADDDOF` residual); `pipe`'s
  return (`Ret::FdPair` is documentation, `x1` uncaptured); the `execve`/`posix_spawn` fail-loud
  assert (M33 Ruling 7, the operator's call); console `writev` mirroring; `__disable_threadsignal`
  (331); `AT_FDCWD` in the 32-bit form real guests pass (M33 Ruling 10); the corpus bias (every
  governed `mach_msg2` still init-time and shallow); the `unexercised` label enforced against a
  census dated 2026-09-12 for syscall numbers and a length census dated 2026-09-13 for M34's
  five, both snapshots; and the `kqueue` cross-version note. **M35's two holes leave this list:**
  the replay-side `.min(avail)` and the `if !err` gate are paid, above.
* **Superseded, not owed — M28's `failwrite` datum.** M28's Task 4 paragraph
  (`docs/status-log.md:5304–5313`) concluded "the kernel wrote nothing, before or after" from one
  buffer, and its Task 5 (`:5315–5321`) took Branch B on that. Both stand as written there, as the
  log's rule requires, with this section as the forward pointer: the datum was true of `buf` and
  false of the call, `failwrite.rs` now asserts the call, and Branch A is taken. No census of the
  corpus's failing syscalls is owed either (spec §7): the hoist records what the kernel wrote,
  and on the two real guests measured it wrote nothing.

## Status: M36-sweepmeasure — nine rows read off kept evidence, and a parked gate for every wall

M35 closed owing M36 two harness defects by name — the sweep reported a record exit of 4 as
"replay diverged", and counted an identical crash on both sides as a bare PASS — and the charter
owed it something older: the README's own sentence that the five `replay diverged` rows' cause
"stands unmeasured to this day, with **no parked gate standing for it** — a gap in this repo's own
discipline rather than a decision". M36 is a measurement milestone. It changed no behaviour: the
branch's `crates/` diff against `main` is one new test file of eight `#[ignore]`d gates and nothing
under any `crates/*/src`; the only other edit is the sweep *harness*, `tools/apple-sweep.sh`, which
is not retrace. It taught the harness to print why a row fails and to keep the evidence, ran the
sweep three times with the recorder's pid steered into three regimes, symbolicated the `brk` the five
`launchctl`-group rows had been believed to reach since M23, and read one class off the kept evidence
for each of the nine rows that are not clean. **What the evidence said was not what the spec
expected.** The `brk` is not libxpc's and not a wall of its own: it is libdispatch's
`_firehose_task_buffer_init+0x12c` crashing on a `proc_info` of the guest's own pid that M34 §4b's
register probe had forwarded as a host pointer — the same defect M35 had found under `dddiagnose`,
now under the five `launchctl`-group rows and `dddiagnose`. And the pid window that defect covers is
`[0x4000, 0x18000)`, about 82 % of the pid space, not the `[0x4000, 0x10000)` M34 computed from the
fixed layout, because the guest maps its own `os_alloc_once` slab into the gap at `0x10000` before
it ever asks about its pid — which is why the run the brief designed as the non-colliding one was
colliding, and why a third run had to be made. Two beliefs are retired below as corrections with
forward pointers; M23's, M34's and M35's lines stand as written.

The milestone's own numbers: **three** full sweep runs of the 54-entry corpus (O, L, I; tallies
45/9, 45/9, 46/8; the same 45 PASS rows on every run); **nine** non-clean rows, **27** row-by-run
cells, every one with its stderr kept and **46** evidence files (46,778 bytes) committed under
`docs/sweep-evidence/2026-09-13-m36/`; **one** symbolication of **four** shared-cache addresses;
**eight** parked gates in **one** new test binary (`crates/retrace/tests/apple_walls_e2e.rs`, 55
lines), run once with `--ignored` and **8 of 8** red for the measured reason; **two** classes of the
charter's six occupied by nine rows (B ×2, B-then-C ×6, D ×1), and A, E1 and E2 empty — E1's two
items fixed in the harness; **two** beliefs retired by measurement and **one** M35 statement
corrected as right-for-the-wrong-reason; **zero** lines under `crates/*/src` (charter §3). The gate
figures are in their own subsection, measured on the code as of the last code commit, `0766a76`.

### What it set out to do

The charter's entry, `docs/superpowers/specs/2026-09-09-retrace-m32-m38-program-charter-design.md`
§3, "M36 — `sweepmeasure`: measurement, and the gates the README already owes", in its own words:

> **Deliverable: a table, and a parked gate per measured wall. No fixes.** This milestone is
> forbidden from changing *behaviour*; its production edits are documentation and `#[ignore]`d
> gates. A behavioural change appearing in an M36 diff is a defect in the run, not a bonus.
>
> […] **One trap this milestone must not fall into.** The README already warns that *"what the
> sweep reports is not why they fail"* — the four `replay diverged` binaries have been *believed*
> since M23 to reach a `brk`, and the sweep corroborates nothing about that. M36's
> `root_cause_class` must be derived from evidence it captures, never from the sweep's category
> string or from the inherited belief.

Its four conditions on a parked gate (a binary in the committed corpus; the measured evidence in
the reason, never the category string or the M23 belief; never park a passing test; name what
un-parks it) and its six-class `root_cause_class` enum (§6: A retired-by-soundness, B
known-unmodelled, C new-subsystem, D not-a-defect, E1 harness-nondeterministic, E2
retrace-nondeterministic) are what the spec
(`docs/superpowers/specs/2026-09-13-retrace-m36-sweepmeasure-design.md`) turned into evidence
tests (§3c) and a gate template (§3d). The wall the spec located (§1) is the harness's blindness:
at the M35 merge `tools/apple-sweep.sh` used `rc` only as a number to compare with `rp`, never
printed a `RECORD ERROR` exit, printed `recorder panicked` without the panic's message, printed
`replay diverged` for every replay of a refused recording (which *always* prints a `DIVERGENCE`
line, because the trace has no terminal event), counted `rc = rp = 139` as PASS with no note,
destroyed every trace and stderr file in its `EXIT` trap, and never recorded the recorder's pid —
the one input M34 §4b and M35 had shown decides what several of these binaries do.

### The harness, and Control 1

Two commits, both to `tools/apple-sweep.sh` only: `fc53853` (88 insertions, 11 deletions) and its
fix round `58fb0e7`. Per binary the script now derives the recorder's pid (`recpid`, printed by a
wrapper shell that `exec`s into the recorder — `sh -c 'echo "recpid=$$" >&2; exec "$0" record-dyn
…'`, M34's probe shape, so the pid is line 1 of `rec.err`), `rec_reason` (the first `RECORD
ERROR:` line, else the first `panicked at` line joined with the message line after it), `rp_line`
(the first `DIVERGENCE at landmark` line) and `landmark`; it prints the human line with a reason
and one tab-separated `ROW` line of nine fields beside it; `RETRACE_SWEEP_LIST` sweeps another
list and `RETRACE_SWEEP_KEEP=<dir>` copies every non-clean row's `rec.err`, `rp.err` and trace to
`<dir>/<basename>.{rec.err,rp.err,bin}`. The labels, in evaluation order, are the spec §3a table
with two rulings applied in the fix round: `identical fault` fires only for `rc = rp ≥ 128` — a
signal death on both sides — not the spec's `rc = rp ≠ 0`, because `/usr/bin/false` exits 1 on
both sides by design and was being labelled a fault (measured: `PASS /usr/bin/false (identical
fault, rc=1)` before the ruling, a bare `PASS /usr/bin/false` after); and a panic's `rec_reason`
carries the assert's message, not only its location line (Rust ≥ 1.73 prints the message on the
next line). `TALLY` is unchanged in shape so the series stays comparable with M33–M35.

Four defects in the plan's own code were found by measurement and fixed rather than transcribed
(the Task 1 report): the plan's `keep_row` condition parsed in `sh` as `KEEP && has-2nd-arg`, so a
plain `keep_row FAIL` kept nothing (the control's file count would have been zero); a row whose
recorder panicked or timed out `continue`s before replay, so the plan's `[ -f rp.err ] && cp`
would have copied the *previous* row's replay stderr under this row's name (`rp.out`/`rp.err` are
now removed per iteration with `t.bin`, and the control keeps **five** files, not the spec's
"six" — `csh.rp.err` cannot exist because `csh`'s replay never ran); `rc = 4` is checked before
the replay-timeout marker, in the spec's order rather than the plan's; and `keep_row`'s `cp`s
are not silenced, because a failed copy is missing evidence.

**Control 1 (spec §6.1), verbatim.** The new script over a three-binary list, after
`cargo build -p retrace` in the worktree:

```
PASS /usr/bin/true
ROW	/usr/bin/true	PASS	0	0	71425	n/a		
FAIL /bin/csh (recorder panicked: thread 'main' (31749357) panicked at crates/retrace-core/src/lib.rs:1140:17:)
ROW	/bin/csh	FAIL	101	n/a	71464	n/a	thread 'main' (31749357) panicked at crates/retrace-core/src/lib.rs:1140:17:	
FAIL /bin/launchctl (record error, rc=4: RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110)
ROW	/bin/launchctl	FAIL	4	3	71485	324	RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110	DIVERGENCE at landmark 324 pc=0x18035f084: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110
TALLY pass=1 fail=2 skip=0
```

with five kept files (`csh.bin`, `csh.rec.err`, `launchctl.bin`, `launchctl.rec.err`,
`launchctl.rp.err`), nine fields on every `ROW` line (`awk -F'\t' '/^ROW/{print NF}'` → 9 9 9),
and each `recpid` equal to line 1 of its kept `rec.err`. The **mutation** — the M35-merge script
(`git show main:tools/apple-sweep.sh`) over the same list:

```
PASS /usr/bin/true
FAIL /bin/csh (recorder panicked)
FAIL /bin/launchctl (replay diverged)
TALLY pass=1 fail=2 skip=0
```

`FAIL /bin/launchctl (replay diverged)` is the label this milestone retires; `FAIL /bin/csh
(recorder panicked)` the reason it discarded. After the fix round the four-binary re-run
(`/usr/bin/false` added) printed the panic with its message —
`FAIL /bin/csh (recorder panicked: thread 'main' (31768166) panicked at
crates/retrace-core/src/lib.rs:1140:17: dup2 is not modelled by the M10 fd table (unexercised by
any gate guest); implement target-slot allocation before a guest uses it)` — and a bare
`PASS /usr/bin/false`. One observation from those two runs, for whoever reads the table:
`launchctl`'s divergence landmark moved from 324 (recpid 71485) to 326 (recpid 72940) with the
same `RECORD ERROR` at the same pc. **The landmark is not a stable identifier for a row; the
`rec_reason` is.**

**The fix wave (final review), after the gate.** Three edits to `tools/apple-sweep.sh` — comment
and label text, and one condition — in the branch's last commit. (1) The `record error` label is
structural: it fires when `rec_reason` begins `RECORD ERROR:` and prints `rc` as corroboration,
no longer on `rc = 4` alone, because the CLI passes a guest's own exit status straight through
(`crates/retrace/src/main.rs` `Outcome::Exit { code } => exit(code)`), so a guest exiting 4 by
design on both sides would have been labelled a record error with an empty reason and counted a
`fail` — the same structural shape the `DIVERGENCE` check already had. No M36 row was mislabelled
by it: the final review checked that no `ROW` line in any run has `rc=4` with an empty
`rec_reason`, and the PASS rows' non-zero codes are 1, 2 and 64 (plus 139 for the fault). Spec
§3a wrote the condition as the exit code; the harness had been faithful to a spec defect. (2)
Both panic greps anchor on `panicked at crates/`, the recorder's own source paths, so a Rust
guest's own panic in the shared stderr is never labelled a recorder panic (a recorder panic
located in a dependency would print a registry path and miss the anchor — unobserved on the
corpus). (3) The pid comment states the measured window `[0x4000, 0x18000)` and points at the
evidence README instead of the `[0x4000, 0x10000)` it was written with at Task 1, and the
header's cut lengths are the code's (200 for a `RECORD ERROR:` line, 300 for a panic pair). The
`≥ 128` clause of `identical fault` is left as it is (ruled: the direction is harmless — a
designed exit ≥ 128 would be labelled a fault and still counted PASS; owed below). `sh -n` clean.
Control 1 re-run on the edited script, the three-binary list, recorder pids 48221 / 48263 / 48285
(`0xbc5d`–`0xbca5`, the page-table backing — colliding), verbatim:

```
PASS /usr/bin/true
FAIL /bin/csh (recorder panicked: thread 'main' (32154372) panicked at crates/retrace-core/src/lib.rs:1140:17: dup2 is not modelled by the M10 fd table (unexercised by any gate guest); implement target-slot allocation before a guest uses it)
FAIL /bin/launchctl (record error, rc=4: RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110)
TALLY pass=1 fail=2 skip=0
```

and a fourth list of `/bin/launchctl` and `/bin/csh` (recorder pids 48359 / 48388), so that both
new labels are shown firing on the edited condition and the anchored grep:

```
FAIL /bin/launchctl (record error, rc=4: RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110)
FAIL /bin/csh (recorder panicked: thread 'main' (32155020) panicked at crates/retrace-core/src/lib.rs:1140:17: dup2 is not modelled by the M10 fd table (unexercised by any gate guest); implement target-slot allocation before a guest uses it)
TALLY pass=0 fail=2 skip=0
```

Five files kept per run (`csh.{bin,rec.err}`, `launchctl.{bin,rec.err,rp.err}`), nine fields on
every `ROW` line, each `recpid` equal to line 1 of its kept `rec.err` (`fix-wave-control1/` in
the SDD workspace).

### The three runs

The spec asked for two: **O**, the recorder's pid outside M34 §4b's `[0x4000, 0x10000)`, and
**I**, inside. Run O was made exactly so — every pid above `0x10000` — and its `dddiagnose` row
came out with the *in-range* signature (the `brk`, 12 self-pid `ESRCH` in the kept trace). The kept
trace explained it (correction (a), below): the collision window is wider than M34 stated, and run O's
pids were inside the wider window. A third run, **L**, at pids below `0x4000` — the regime M35's
own out-of-range probes had used — was added as the genuinely non-colliding run. Nothing was
re-run or discarded; all three are kept and tabled. Each was one invocation of the committed
script at binary commit `58fb0e7` (whose `crates/` is byte-identical to the M35 merge `44d302a`:
`git diff 44d302a..58fb0e7 --stat -- crates/` is empty), launched detached with
`RETRACE_SWEEP_KEEP` and polled; the pid counter was read with `sh -c 'echo $$'` and advanced
with loops of `/usr/bin/true` (about 24k spawns to wrap past `PID_MAX`, about 13.4k to reach
17,000); run order O → wrap → L → I, strictly sequential; each run about four minutes, not the
10–14 the plan budgeted.

| run | `pidstart` | recpid min–max (dec) | recpid min–max (hex) | intended regime | **measured** regime | `TALLY` |
|---|---|---|---|---|---|---|
| **O** | 73426 | 73437–75432 | `0x11EDD`–`0x126A8` | outside `[0x4000,0x10000)` — true | **colliding**: every pid inside `[0x10000, 0x18000)`, the guest's own `os_alloc_once` slab (Finding 1) | `pass=45 fail=9 skip=0` |
| **L** | 800 | 812–2851 | `0x32C`–`0xB23` | (added) below `0x4000` | **non-colliding**: below every backing; 0 self-pid `ESRCH` in every kept trace | `pass=45 fail=9 skip=0` |
| **I** | 17198 | 17209–19410 | `0x4339`–`0x4BD2` | inside `[0x4000,0x10000)` — true | **colliding**: inside `[0x4000, 0x8000)`, the trampoline page | `pass=46 fail=8 skip=0` |

Every `recpid` on every `ROW` line of each run is inside its stated range (`awk` over the `ROW`
lines: 0 rows outside, 0 rows with an empty pid); no run crossed a boundary. Every human line that
is not a bare `PASS`, verbatim from `sweep-{O,L,I}.log`:

Run O:

```
pidstart=73426
FAIL /bin/csh (recorder panicked: thread 'main' (31774615) panicked at crates/retrace-core/src/lib.rs:1140:17: dup2 is not modelled by the M10 fd table (unexercised by any gate guest); implement target-slot allocation before a guest uses it)
FAIL /bin/launchctl (record error, rc=4: RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110)
FAIL /bin/tcsh (recorder panicked: thread 'main' (31780397) panicked at crates/retrace-core/src/lib.rs:1140:17: dup2 is not modelled by the M10 fd table (unexercised by any gate guest); implement target-slot allocation before a guest uses it)
FAIL /usr/bin/automationmodetool (record error, rc=4: RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110)
FAIL /usr/bin/desdp (record error, rc=4: RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110)
FAIL /usr/bin/dyld_info (record error, rc=4: RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110)
FAIL /usr/bin/flex (record error, rc=4: RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110)
FAIL /usr/bin/dddiagnose (record error, rc=4: RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110)
FAIL /usr/bin/yes (timed out after 30s recording)
TALLY pass=45 fail=9 skip=0
SWEEP_EXIT=0
```

Run L:

```
pidstart=800
FAIL /bin/csh (recorder panicked: thread 'main' (31850265) panicked at crates/retrace-core/src/lib.rs:1140:17: dup2 is not modelled by the M10 fd table (unexercised by any gate guest); implement target-slot allocation before a guest uses it)
FAIL /bin/launchctl (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /bin/tcsh (recorder panicked: thread 'main' (31856621) panicked at crates/retrace-core/src/lib.rs:1140:17: dup2 is not modelled by the M10 fd table (unexercised by any gate guest); implement target-slot allocation before a guest uses it)
FAIL /usr/bin/automationmodetool (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/desdp (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/dyld_info (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/flex (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/dddiagnose (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/yes (timed out after 30s recording)
TALLY pass=45 fail=9 skip=0
SWEEP_EXIT=0
```

Run I:

```
pidstart=17198
FAIL /bin/csh (recorder panicked: thread 'main' (31894566) panicked at crates/retrace-core/src/lib.rs:1140:17: dup2 is not modelled by the M10 fd table (unexercised by any gate guest); implement target-slot allocation before a guest uses it)
FAIL /bin/launchctl (record error, rc=4: RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110)
FAIL /bin/tcsh (recorder panicked: thread 'main' (31900825) panicked at crates/retrace-core/src/lib.rs:1140:17: dup2 is not modelled by the M10 fd table (unexercised by any gate guest); implement target-slot allocation before a guest uses it)
FAIL /usr/bin/automationmodetool (record error, rc=4: RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110)
FAIL /usr/bin/desdp (record error, rc=4: RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110)
FAIL /usr/bin/dyld_info (record error, rc=4: RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110)
FAIL /usr/bin/flex (record error, rc=4: RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110)
PASS /usr/bin/dddiagnose (identical fault, rc=139)
FAIL /usr/bin/yes (timed out after 30s recording)
TALLY pass=46 fail=8 skip=0
SWEEP_EXIT=0
```

**Every label that differs between runs.** O against I: `dddiagnose` only (`FAIL … record error,
rc=4`, the `brk` → `PASS … (identical fault, rc=139)`). O against L: exactly the six `rc=4` rows,
the `brk` → the RCV-shaped `mach_msg2`, `dddiagnose` among them. `csh`, `tcsh` and `yes` are
identical across all three (the panic's thread id is the only varying token). The
`PASS /usr/bin/dddiagnose (identical fault, rc=139)` line is spec §6's Control 3 — the row the old
script printed as a bare `PASS`, now labelled — and under Ruling 2 it is still counted in `pass`,
which is why run I tallies 46/8 while O and L tally 45/9 with the same 45 clean rows.

### The symbolication

The four addresses — the `brk` pc `0x18035f084`, its `elr` `0x1804af110`, the RCV-wall pc
`0x1804adc34`, and run I's `dddiagnose` crash pc `0x180302eb0` — are unslid shared-cache
addresses (the guest maps the cache at slide 0, `crates/retrace-box/src/lib.rs:1387`), so the
lookup is at `unslid + host slide`. The spec's `lldb` command could not be used: on `/usr/bin/true`
SIP refused the attach (`error: process exited with status -1 (attach failed (Not allowed to
attach to process. …`), and on a scratchpad-compiled no-op C program it attached and then printed
nothing for ten minutes at `image list` and was killed; `atos -p` on a live copy hung the same way.
The lookup was done with `dladdr(3)` from a process of my own (`sym2.c`, pasted in full in
`docs/sweep-evidence/2026-09-13-m36/README.md` § Symbolication), which resolves against the same
cache mapping every process shares, plus a raw read of the instruction words at the pc (`dis.c`).
`dladdr` names the nearest *exported* symbol; a local symbol closer to the address would not be
visible to it — the instruction words and the `elr` are what tie the `brk` to `proc_info`. Output,
2026-09-13, this machine:

```
shared cache base=0x18ecdc000 len=0x165170000 slide=0xecdc000
unslid 0x18035f084 -> slid 0x18f03b084: dladdr=1 fname=/usr/lib/system/libdispatch.dylib fbase=0x18f011000 sname=_firehose_task_buffer_init saddr=0x18f03af58 (+0x12c)
unslid 0x1804af110 -> slid 0x18f18b110: dladdr=1 fname=/usr/lib/system/libsystem_kernel.dylib fbase=0x18f189000 sname=__proc_info saddr=0x18f18b108 (+0x8)
unslid 0x1804adc34 -> slid 0x18f189c34: dladdr=1 fname=/usr/lib/system/libsystem_kernel.dylib fbase=0x18f189000 sname=mach_msg2_trap saddr=0x18f189c2c (+0x8)
unslid 0x180302eb0 -> slid 0x18efdeeb0: dladdr=1 fname=/usr/lib/system/libsystem_malloc.dylib fbase=0x18efc0000 sname=mfm_alloc saddr=0x18efdec80 (+0x230)
```

The instruction words around the `brk` pc, read from the live cache mapping (`pc-0x40` to `pc+8`):

```
0x18035f044: a9457bfd
0x18035f048: a9444ff4
0x18035f04c: 910183ff
0x18035f050: d65f0fff
0x18035f054: 350001a0
0x18035f058: d53bd068
0x18035f05c: f9400508
0x18035f060: b9800108
0x18035f064: a9bf57f4
0x18035f068: d00000d4
0x18035f06c: 911fce94
0x18035f070: f034c6d5
0x18035f074: 9128e2b5
0x18035f078: f90006b4
0x18035f07c: f9001ea8
0x18035f080: a8c157f4
0x18035f084: d4200020   <-- pc
0x18035f088: 93407c08
0x18035f08c: a9bf57f4
```

Read: `0xd65f0fff` at `0x18035f050` is `retab`, the end of `_firehose_task_buffer_init`
(`saddr + 0xf8`). The block from `0x18035f054` (`+0xfc`) is an outlined path: `cbnz w0`, then
`mrs x8, TPIDRRO_EL0` / `ldr x8, [x8, #8]` / `ldrsw x8, [x8]` — errno, read from the TSD's errno
slot — two `adrp`/`add` pairs and two `str`s into a global (a crash-reason store), then
`0xd4200020` = **`brk #1`** at the recorded pc: `EC=0x3c ISS=0x1` exactly, the shape of a
`DISPATCH_INTERNAL_CRASH(errno, …)`. The `elr` is `__proc_info + 8`, the return address of the last
`proc_info`; in every `brk` trace that last landmark is `proc_info(2 PIDINFO, <recorder pid>, 17)`
answered `ESRCH`. Flavor 17 is not in the public SDK header (`sys/proc_info.h` jumps from 16 to
19); xnu's `bsd/sys/proc_info_private.h` defines `PROC_PIDUNIQIDENTIFIERINFO 17`. So the `brk` is
libdispatch's firehose task-buffer init crashing on a failed
`proc_pidinfo(getpid(), PROC_PIDUNIQIDENTIFIERINFO, …)` — the `ESRCH` §4b manufactures. The RCV-wall
pc is `mach_msg2_trap + 8`, the trap's return address. The crash pc is libsystem_malloc
`mfm_alloc + 0x230`, a data abort (`esr=0x92000045`, DFSC `0x05`, translation fault at level 1),
identical on record and replay.

### The table

`sweep-table.md` in the SDD workspace is the single source; its Tables A and B and the per-row
justification are transcribed here verbatim. `lm` = the landmark on the replay's `DIVERGENCE`
line (`n/a` when replay did not run or did not print one). `err` = `err=true` landmarks in the
kept trace. `ESRCH` = self-pid `csops` (169/170) / `proc_info` (336) calls answered `ESRCH` in the
kept trace, counted with a `retrace_trace::Reader::open_checked` reader in the scratchpad (the
counting rules — index, `err`, self-pid `ESRCH` by `args[0]` for 169/170 and `args[1]` for 336
equal to that run's `recpid`, and the `0x10000` map by `num == -15`, `args[2] == 0x8000`,
`args[4] == 0x49000001` — are written out in the evidence README so every number is
re-derivable from a kept trace and nothing else). The table's own cross-references `Finding 1`–`4`
are `sweep-table.md`'s findings: 1 = the collision window (correction (a) below), 2 = the `brk`
(correction (b)), 3 = the crash/`brk` split (the `dddiagnose` subsection), 4 = `csh`/`tcsh`
§4b-touched but not §4b-walled.

**Table A — per-row facts (from the `ROW` lines and the kept files)**

| binary | run | label | rc/rp | recpid | lm | `rec_reason` (first line) | `err` | `ESRCH` |
|---|---|---|---|---|---|---|---|---|
| `/bin/csh` | O | `recorder panicked: … lib.rs:1140:17: dup2 is not modelled by the M10 fd table …` | 101/n/a | 73617 | n/a | `thread 'main' (31774615) panicked at crates/retrace-core/src/lib.rs:1140:17: dup2 is not modelled by the M10 fd table (unexercised by any gate guest); implement target-slot allocation before a guest uses it` | 38 | 5 |
| | L | same | 101/n/a | 1013 | n/a | same (thread id 31850265) | 33 | 0 |
| | I | same | 101/n/a | 17386 | n/a | same (thread id 31894566) | 38 | 5 |
| `/bin/tcsh` | O | same as `csh` | 101/n/a | 74616 | n/a | same (thread id 31780397) | 38 | 5 |
| | L | same | 101/n/a | 2044 | n/a | same (thread id 31856621) | 33 | 0 |
| | I | same | 101/n/a | 18480 | n/a | same (thread id 31900825) | 38 | 5 |
| `/bin/launchctl` | O | `record error, rc=4: RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110` | 4/3 | 74012 | 331 | the `brk` line | 52 | 11 |
| | L | `record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape` | 4/3 | 1415 | 338 | the RCV-shape line | 41 | 0 |
| | I | the `brk` line | 4/3 | 17867 | 325 | the `brk` line | 52 | 11 |
| `/usr/bin/automationmodetool` | O | the `brk` line | 4/3 | 74786 | 334 | the `brk` line | 56 | 11 |
| | L | the RCV-shape line | 4/3 | 2223 | 339 | the RCV-shape line | 45 | 0 |
| | I | the `brk` line | 4/3 | 18664 | 334 | the `brk` line | 56 | 11 |
| `/usr/bin/desdp` | O | the `brk` line | 4/3 | 74818 | 361 | the `brk` line | 50 | 11 |
| | L | the RCV-shape line | 4/3 | 2252 | 367 | the RCV-shape line | 39 | 0 |
| | I | the `brk` line | 4/3 | 18693 | 361 | the `brk` line | 50 | 11 |
| `/usr/bin/dyld_info` | O | the `brk` line | 4/3 | 74847 | 371 | the `brk` line | 50 | 11 |
| | L | the RCV-shape line | 4/3 | 2281 | 368 | the RCV-shape line | 39 | 0 |
| | I | the `brk` line | 4/3 | 18724 | 356 | the `brk` line | 50 | 11 |
| `/usr/bin/flex` | O | the `brk` line | 4/3 | 74877 | 359 | the `brk` line | 50 | 11 |
| | L | the RCV-shape line | 4/3 | 2310 | 370 | the RCV-shape line | 39 | 0 |
| | I | the `brk` line | 4/3 | 18752 | 362 | the `brk` line | 50 | 11 |
| `/usr/bin/dddiagnose` | O | the `brk` line | 4/3 | 74909 | 379 | the `brk` line | 75 | 12 |
| | L | the RCV-shape line | 4/3 | 2340 | 379 | the RCV-shape line | 63 | 0 |
| | I | `PASS … (identical fault, rc=139)` | 139/139 | 18781 | n/a | (none — `guest crashed: pc=0x180302eb0 far=0x2000050050 esr=0x92000045` on both sides; terminal `Event::Crash` + final snapshot, trace complete) | 71 | 11 |
| `/usr/bin/yes` | O | `timed out after 30s recording` | 137/n/a | 75271 | n/a | (none) | — | — |
| | L | same | 137/n/a | 2722 | n/a | (none) | — | — |
| | I | same | 137/n/a | 19283 | n/a | (none) | — | — |

`dddiagnose`'s three regimes, `errcount` (the scratchpad reader) verbatim — `syscall N: err=K`
lines that are absent from a run are absent from its trace:

```
keep-L/dddiagnose.bin: events=379 syscalls=378 err=true landmarks=63 of which with writes=0 (bytes=0)
  5: 2   33: 1   38: 6   54: 7   286: 1   294: 1   336: 1   338: 30   381: 5   483: 8   550: 1
keep-O/dddiagnose.bin: events=379 syscalls=378 err=true landmarks=75 of which with writes=0 (bytes=0)
  5: 2   33: 1   38: 6   54: 7   169: 7   170: 1   286: 1   294: 1   336: 5   338: 30   381: 5   483: 8   550: 1
keep-I/dddiagnose.bin: events=359 syscalls=356 err=true landmarks=71 of which with writes=0 (bytes=0)
  5: 1   33: 1   38: 6   54: 7   169: 7   170: 1   286: 1   294: 1   336: 4   338: 30   381: 4   483: 7   550: 1
```

Read: L has no `csops` (169/170) failure and one `proc_info` (336) failure — the pid-independent
`SET_DYLD_IMAGES` `EINVAL` (M34 #24). O adds 169 ×7, 170 ×1, 336 ×4 = **12** self-pid `ESRCH`
(75 = 63 + 12, M35's signature exactly). I adds 169 ×7, 170 ×1, 336 ×3 = **11** self-pid
`ESRCH` — not 71 − 63 = 8: the I trace ends at the crash 22 landmarks before L's wall and so
lacks three of L's *other* `err`s (one each of 5, 381, 483) and the twelfth self-pid call
(`proc_info(2, pid, 17)`, the one the `brk` path dies on). The `ESRCH` column is counted
directly from the pid-carrying calls, never inferred from the totals.

Every replay `rp_line` for a `rc=4` row is the replay running out of events at (the RCV shape:
`expected recorded syscall, got None (truncated=false)`) or re-reporting (the `brk`: the same
`EC=0x3c` exception at the same pc) the recorder's stop — never a divergence of record from
replay. For the RCV shape the refused call **is** the stop and is never recorded
(`Route::Unsupported` → `RECORD ERROR` before any append), so the replay's landmark is one past
the trace's last syscall: run L's `dddiagnose` trace ends on `issetugid` (327) #378 (`args=[0, 4,
…]`, `ret=0`) and replay reports 379. For the `brk` the last recorded landmark is the failed
`proc_info(2, pid, 17)` and the exception follows it, so replay's landmark is that landmark + 1
as well. Checked on all 17 `rc=4` traces (six rows × three runs, less `dddiagnose` I): replay's
landmark equals the trace's event count in every one; every run-L trace ends on `issetugid`
(327), every `brk` trace on the failed `proc_info(2, pid, 17)`. The `dest` port name on the
`refusing mach_msg2 message-queue send` line varies per run (`0xe03`–`0x1803`), as M35
documented (M2-xpcport's minted-port asymmetry); nothing else in the kept stderr varies between
runs of the same regime except the panic's thread id.

**Table B — trap, symbol, class, evidence, gate, route**

| binary | trap (O/I regime) | trap (L regime) | symbol | `root_cause_class` | evidence | gate | route |
|---|---|---|---|---|---|---|---|
| `/bin/csh` | panic: `crates/retrace-core/src/lib.rs:1140:17` (M33 `dup2` assert) | same | — (retrace assert) | **B** known-unmodelled (`dup2`) | `docs/sweep-evidence/2026-09-13-m36/csh.{O,L,I}.rec.err`; traces `keep-{O,L,I}/csh.bin` | `csh_records_and_replays` | M37 |
| `/bin/tcsh` | same | same | — | **B** (`dup2`) | `…/tcsh.{O,L,I}.rec.err`; `keep-{O,L,I}/tcsh.bin` | `tcsh_records_and_replays` | M37 |
| `/bin/launchctl` | `EC=0x3c` `brk #1` at `pc=0x18035f084`, `elr=0x1804af110` | `mach_msg2` options `0x404000102` at `pc=0x1804adc34`, `Route::Unsupported` | libdispatch `_firehose_task_buffer_init+0x12c` (`brk #1`); elr = libsystem_kernel `__proc_info+8`; RCV pc = `mach_msg2_trap+8` | **B then C**: B = M34 §4b (the eleven self-pid `ESRCH` answers, then libdispatch's own crash on the failed `proc_info(PIDUNIQIDENTIFIERINFO)`); C = the RCV-shaped message-queue `mach_msg2` | `…/launchctl.{O,L,I}.{rec,rp}.err`; `keep-{O,L,I}/launchctl.bin` | `launchctl_records_and_replays` | M37 for the B wall (§4b); the gate then stands parked at C, not routed |
| `/usr/bin/automationmodetool` | same `brk` | same RCV shape | same | **B then C** | `…/automationmodetool.{O,L,I}.{rec,rp}.err` | `automationmodetool_records_and_replays` | as `launchctl` |
| `/usr/bin/desdp` | same `brk` | same RCV shape | same | **B then C** | `…/desdp.{O,L,I}.{rec,rp}.err` | `desdp_records_and_replays` | as `launchctl` |
| `/usr/bin/dyld_info` | same `brk` | same RCV shape | same | **B then C** | `…/dyld_info.{O,L,I}.{rec,rp}.err` | `dyld_info_records_and_replays` | as `launchctl` |
| `/usr/bin/flex` | same `brk` | same RCV shape | same | **B then C** | `…/flex.{O,L,I}.{rec,rp}.err` | `flex_records_and_replays` | as `launchctl` |
| `/usr/bin/dddiagnose` | O: same `brk` (12 `ESRCH`); I: data abort `esr=0x92000045` (DFSC 0x05, translation level 1) at `pc=0x180302eb0`, `far=0x2000050050`, identical on both sides (11 `ESRCH`) | same RCV shape | `brk` as above; crash pc = libsystem_malloc `mfm_alloc+0x230` | **B then C** (B = §4b, with two downstream faces — the `brk` and the identical malloc crash; C = RCV shape) | `…/dddiagnose.{O,L,I}.{rec,rp}.err` | `dddiagnose_records_and_replays` | as `launchctl` |
| `/usr/bin/yes` | SIGKILL by the sweep's 30 s watchdog, no stdout captured | same | — | **D** not-a-defect | `…/yes.{O,L,I}.rec.err` (the recorder's stderr up to the kill) | `none (class D)` | retired |

Class A rows: **none** (no row that failed at M35 passes with `rc = rp = 0` on any run).
Class E2 rows: **none** (Finding 3 — the `dddiagnose` crash/`brk` split is between runs
whose forwarded inputs differ, and record and replay agree bit-for-bit within every run).
Class E1 items (the harness): the two M35 routed to M36 — `rc=4` reported as "replay diverged",
and an identical fault counted PASS without a note — are **fixed in harness** by Task 1
(`fc53853`, `58fb0e7`); both fixes are visible in these logs (`record error, rc=4: …` on the six
rows; `PASS /usr/bin/dddiagnose (identical fault, rc=139)` on run I — spec §6 control 3).

### The classes — which §3c evidence test each row met

The spec's §3c gives one evidence test per class; each row is placed by the test it met, quoted
from the table's per-row justification:

- **`csh`, `tcsh` → B.** `rec_reason` names a retrace fail-loud that a table entry closes:
  `crates/retrace-core/src/lib.rs:1140:17: dup2 is not modelled by the M10 fd table` — in all
  three runs, at the same assert (`keep-{O,L,I}/csh.rec.err` lines 9–10, `tcsh` likewise). The
  pid regime changes the `ESRCH` count (5 → 0) but not the wall: the trace reaches `dup2` either
  way (Finding 4: §4b-touched but not §4b-walled). Route M37.
- **`launchctl`, `automationmodetool`, `desdp`, `dyld_info`, `flex` → B then C.** The `brk`
  M23 believed was these rows' wall is not their wall. Runs O and I (colliding pids): the kept
  trace has 11 self-pid `csops`/`proc_info` calls answered `ESRCH` (0 in run L), the last landmark
  before the exception is `proc_info(2 PIDINFO, pid, 17)` → `ESRCH`, `elr` is `__proc_info+8`,
  and the `brk #1` is on the outlined crash path after libdispatch `_firehose_task_buffer_init`
  — the flavor-17 (`PROC_PIDUNIQIDENTIFIERINFO`, xnu `proc_info_private.h:145`) lookup that
  init makes of its own pid, failing because §4b forwarded the pid as a host pointer. That is
  the B test ("M34 §4b's pid mis-translation, the self-pid `ESRCH` answers"), and it is the same
  evidence M35 read for `dddiagnose`. Run L (non-colliding pids): `rec_reason` is literally
  `unsupported mach_msg2 … options 0x404000102: message-queue send without the send+rcv RPC
  shape` — the C test's second clause, `Route::Unsupported` on the RCV-shaped message-queue call
  (`keep-L/<b>.rec.err` last line, preceded by M23 t5's serviced `refusing … message-queue send`
  line, which is survived). Fix order as M35 wrote for `dddiagnose`: §4b first (it decides which
  wall a run reaches), then the RCV shape. Gate parked at the first; route the B wall to M37; the
  C wall is parked, not routed (Ruling 1). The C test's *first* clause — "the post-refusal `brk`
  in libxpc after the serviced `RefuseMqSend`" — was met by no row: the only `brk` observed is the
  §4b consequence above.
- **`dddiagnose` → B then C**, as M35 classified it; this measurement adds the non-colliding run
  (L: the RCV shape, 0 `ESRCH`, landmark 379 — M35's 10/10) and one more in-range crash
  (I: `pc=0x180302eb0 far=0x2000050050`, libsystem_malloc `mfm_alloc+0x230`, after 11 `ESRCH`).
  Both the crash and the `brk` are downstream of §4b (run L and M35's out-of-range runs reach
  neither: 16 of 16); why an in-range run takes one or the other stays open (Finding 3) and is
  retired with §4b or becomes a new row after it.
- **`yes` → D.** `rc=137`, no `rec_reason`, the watchdog's `.timedout` marker: killed at 30 s on
  the record side by design, on every run. No gate.
- **A: none. E2: none.** No row that failed at M35 passes now; within every run record and replay
  agree (the E2 test — same pid regime, different landmark sequences, or a complete recording
  replaying to different landmarks — is met by nothing; Finding 3 locates the one between-run
  split at a forwarded input). **E1: the two M35 items**, both fixed in the harness and both
  visible in the logs above.

### Two beliefs retired by measurement, and one M35 reason corrected — with forward pointers

The log is append-only. The three earlier statements below stand as written where they are; this
subsection is their forward pointer, and the README is edited in place.

**(a) M34 §4b's collision window is `[0x4000, 0x18000)`, not `[0x4000, 0x10000)` — about 82 % of
the pid space, and guest-dependent.** M34's section says (`docs/status-log.md:6734`, `:7088–7089`)
that "for roughly half of all recorder pids" — "every recorder pid in 16384..=65535 … roughly half
the pid space, a coin flip per record run" — the probe rewrites the pid, computed from the fixed
trampoline/page-table backings at `[0x4000, 0x10000)`; the same section's probe paragraphs
(`:7152`, `:7178–7181`) and M35 (`:7678`) call that range "the §4b collision range". That is the *static* picture, and it is
wrong at runtime for every dynamic guest measured (6 of 6 rows, three regimes). Measured: every
kept colliding trace (8 binaries × runs O and I, 16 of 16) has a `mach_vm_map(size 0x8000, flags
0x49000001)` — tag 73 = `VM_MEMORY_OS_ALLOC_ONCE`, libSystem's `os_alloc_once` slab — whose
out-pointer, read back from the landmark's recorded write, is **`0x10000`**: `first_fit` placed it
in the gap between `PT_L1_IPA`'s backing end (`0x10000`) and the TSD region (`0x28000`). It
precedes the first self-pid `ESRCH` in every one — at #182 → #200 in `dddiagnose`, #136 → #154
in `launchctl`, #144 → #162 in `automationmodetool`, #128 → #146 in `desdp`, `dyld_info`,
`flex`, `csh` and `tcsh` (the same map sits at the same index in the run-L traces, where no pid
collides with it) — and it is a pure function of the guest's syscall sequence, so it is at
`0x10000` on record and replay alike. The backings `forward_and_diff`'s per-register probe sees at
the pid-carrying calls are therefore contiguous over `[0x4000, 0x18000)` = pids 16384..=98303;
the non-colliding pids are 1..16383 and 98304..99998 (xnu's `PID_MAX` is 99999 with `nextpid`
reset at `>=`, so 99998 is the highest assignable pid; the upper band is inferred from run I's
final snapshot — nothing mapped in `[0x18000, 0x28000)`, and everything the guest maps later sits
at ≥ `0x40000`, above `PID_MAX` — no run used a pid ≥ `0x18000`). Measured on 6 binaries × 3 regimes: O
(`0x11EDD`–`0x126A8`) 11–12 `ESRCH`, L (`0x32C`–`0xB23`) 0, I (`0x4339`–`0x4BD2`) 11–12. The
window as stated here is what these six guests map before their first self-pid call; another
guest may map more (or less) below `0x100000` before its own, so the set is guest-dependent and
"outside `[0x4000, 0x10000)`" is not a regime. The only regime-independent fix is the one M34
§4b's own text names: stop probing `Scalar` registers as pointers — and M37's positive control
must use a pid inside `[0x10000, 0x18000)` as well as one inside `[0x4000, 0x10000)`.

**(b) The `brk` is libdispatch's, and it is a §4b consequence — not a post-refusal wall.** M23
found the `brk` on four binaries and recorded its cause as unmeasured (`:4583–4585`); its router
comment attributes the `brk` to the refusal it precedes ("the other four `brk` regardless of which
of seven refusal codes is returned", `crates/retrace-core/src/machmsg.rs:97–99`); M35 carried it
as "M23's post-refusal `brk` class" and as the third of `dddiagnose`'s three walls (`:7713–7716`,
`:7739–7743`, `:7846–7848`); the M36 spec's §3c wrote the C test as "the post-refusal `brk` in
libxpc after the serviced `RefuseMqSend`", and its §4 first reading — taken at recorder pids
`0x10806`–`0x10887`, which are inside the slab — saw the `brk` on all five `launchctl`-group rows
and called it "the M23 belief confirmed by measurement", class C. Retired on both counts by the
symbolication above and the kept traces: the image is libdispatch and the cause is the pid. The
serviced refusal merely precedes the `brk` in time, as it precedes the RCV wall in run L just the
same. With a non-colliding pid all five rows — and `dddiagnose` — reach the RCV-shaped
message-queue `mach_msg2` (options `0x404000102`, `Route::Unsupported`, pc `mach_msg2_trap+8`)
instead: run L 6 of 6, M35 10 of 10. **The `brk` has never been observed with a
correctly-forwarded pid.** So the five rows are class **B then C**, `dddiagnose`'s shape, and
the spec §4 first reading is contradicted. What is *not* claimed: whether a `brk` of M23's kind
lies behind the RCV shape is unmeasured, and cannot be measured until that shape is modelled.
`dladdr` sees exported symbols only, so a closer local symbol is possible; the instruction words
and the `elr` are what tie the `brk` to `proc_info`.

**(c) M35's "out-of-range → RCV wall" statement was right for the wrong reason.** M35's first
probe (`:7683`) called its recorder pids `0x257f`–`0x2662` "all **outside** the range" and drew
the 10-of-10 RCV-wall result from that: those pids were below `0x4000`, not above `0x10000`, which
is why they were non-colliding — the same reason run L's are — and a run above `0x10000` (this
milestone's run O) is not.

### `dddiagnose`'s three faces, and Finding 3

`dddiagnose` alone shows three faces across the three runs, and the row carries all three:

| run | recpid | result | `err` | self-pid `ESRCH` | last landmark before the stop |
|---|---|---|---|---|---|
| L | 2340 (`0x924`) | `RECORD ERROR: unsupported mach_msg2 … options 0x404000102` (RCV shape) | 63 | 0 | `issetugid` (327) #378, `args=[0, 4, …]`, `ret=0` — the refused `mach_msg2` **is** the stop and was never recorded, which is why replay reports landmark 379, one past the end |
| O | 74909 (`0x1249d`) | `RECORD ERROR: … EC=0x3c … pc=0x18035f084` (the libdispatch `brk`) | 75 = 63 + 12 | 12 (169 ×7, 170 ×1, 336 ×4) | `proc_info(2, pid, 17)` → `ESRCH` #378 |
| I | 18781 (`0x495d`) | `PASS (identical fault, rc=139)`: `guest crashed: pc=0x180302eb0 far=0x2000050050 esr=0x92000045` both sides | 71 | 11 (169 ×7, 170 ×1, 336 ×3) | `csops(pid, 0, …)` → `ESRCH` #356 |

The I row's label is the harness's retrace-induced-crash marker (spec §6 control 3), not a pass:
record and replay agree, and the crash follows the mis-answered self-pid calls. Class B, never a
pass (the mid-run ruling below).

**Finding 3 — the in-range crash/`brk` split is not E2.** Run I (pid `0x495d`) crashed in
libsystem_malloc `mfm_alloc+0x230` after its 11th `ESRCH`; run O (pid `0x1249d`) took the 12th
`ESRCH` and the `brk`. Measured: the two landmark sequences are identical (by `num`, `err`, write
count) through #248 and fork at #249, inside a `gettimeofday` (116) polling loop — run O runs it
16 times (#240–#255, recorded `tv_usec` 327863 → 384503, 56.6 ms), run I 9 times (#240–#248,
271784 → 301139, 29.4 ms), with different recorded `tv_sec` (1789325229 vs 1789326331). Nothing
traps between iterations, so the loop's exit condition is not syscall-visible; the only recorded
input inside the loop is the kernel's `gettimeofday` reply, and it differs between the runs before
the fork. (The `read_nocancel` (396) regions also differ, `w=1` vs `w=2` on the same reads — but
that post-dates the fork and adds nothing.) Within each run, record and replay agree bit-for-bit:
the crash is a terminal `Event::Crash` with its final snapshot, reproduced by replay; the O replay
re-reports the `brk` at the same landmark; the sweep's `identical fault` label is correct. That is
M35's ruling on the same row, now with the fork located. The spec's E2 test ("same pid regime,
different landmark sequences") is not met, because a forwarded input demonstrably differs between
the runs before the fork; the charter's ("retrace's own record/replay varies between identical
runs") is not met, because record and replay agree. **Why an in-range run then takes the crash
rather than the `brk` stays open**, attached to the §4b row. Task 3's control added one more
datum: its `dddiagnose` (pid `0x6d61`) crashed at the same pc and esr with `far=0x6000050040`,
where run I had `far=0x2000050050`; the controller replayed that trace with the signed CLI —
rc 139, `guest crashed: pc=0x180302eb0 far=0x6000050040 esr=0x92000045`, bit-identical to its
record — so the `far` varies between runs of the crash face while pc and esr do not, and replay
reproduces each run's `far`: the same forwarded-input dependence, not nondeterminism of retrace's
own. Cost if wrong: a retrace defect in the malloc crash path enters `main` under a B label; the
traces `keep-I/dddiagnose.bin` (crash) and `keep-O/dddiagnose.bin` (`brk`) are the check.

### Rulings

The spec's three, and the run's, each with what it cost if wrong where the ledger recorded one.

- **Spec Ruling 1** (§3c): class C rows are parked and not routed; the run continues on the B
  rows. What halts is any attempt to *fix* a C row, not the run. Cost if wrong: the operator wanted
  the run to stop at the first C row; instead M37 runs on B rows only, and the C parks are exactly
  what a stop would have left behind.
- **Spec Ruling 2:** `identical fault` rows count as `pass` in the tally so the 46/8 ↔ 45/9 series
  stays comparable across M33–M36; the note on the line is the correction, and this section says
  which PASS row is a fault (`dddiagnose`, run I). Cost if wrong: a reader of the bare tally still
  over-counts passes by the number of identical faults — the same over-count every earlier
  milestone made, now labelled.
- **Spec Ruling 3:** M36 parks gates under the charter's §5 exception and its four conditions; the
  "any need to park a NEW `#[ignore]`" halt rule is not triggered by them (recorded again at
  pre-flight so the halt list was not misread mid-run). Any gate failing a condition is a halt;
  none did.
- **Task 1, (a):** "identical fault" means signal death — the threshold is `rc ≥ 128`; a clean
  equal non-zero exit stays a bare PASS (spec §3a's `rc = rp ≠ 0` corrected in §11). Cost if wrong:
  a guest that exits non-zero by design on both sides for a retrace-caused reason is unlabelled, as
  before M36. **(b):** the panic's message line is joined into `rec_reason` so the label and the
  gate reasons carry the assert's text.
- **Task 2, mid-run** (after run O came out colliding): (1) keep all three runs — L is the
  no-collision regime, I and O are two collision bands; the table carries three column groups and
  the `dddiagnose` row states all three regimes with pid and `err` count (63 / 71 / 75). (2)
  `dddiagnose` in run I is `PASS (identical fault, rc=139)` after the same serviced refusal — the
  spec's retrace-induced-crash marker — so its class is B (§4b), never a pass; in L it reaches the
  RCV-shaped call (class C, parked/unrouted per Ruling 1); in O the `brk` (class B via §4b). (3)
  M36's docs correct M34 §4b's `[0x4000, 0x10000)` with a forward pointer (log, never a rewrite),
  and M37's §4b precondition becomes "a `Scalar` arg is never a pointer", not "avoid one band".
  Cost if wrong: if the low first-fit region is not what `0x1249D` collided with, the mechanism
  paragraph is wrong but every row's measured label/pid/count stands — the table is evidence, the
  mechanism is one paragraph.
- **Task 3, pre-dispatch:** the plan's reason templates were the spec's first reading and are
  retired by Task 2 — the five "replay diverged" rows are B-then-C, three runs, the window
  `[0x4000, 0x18000)`, and `dddiagnose`'s run-I crash is §4b's second face; written as an
  amendment that wins over the brief. Cost if wrong: the reasons are re-cut in the fix round.
- **Task 3, the `far`:** record == replay within the run (the controller's replay of the control's
  trace), so the varying `far` is the same forwarded-input dependence as Finding 3's fork, not E2;
  the `dddiagnose` reason gains one clause naming both `far`s and that replay reproduces each.
  Cost if wrong: none to the class or the gate.
- **Three scoped re-reviews replaced** by the controller's own check — Task 1's fix round by a read
  of the 40-line diff against the two rulings and the control log; Task 2's by a mechanical check
  (`cmp` of the 45 evidence files against `keep-{O,L,I}`, 0 mismatches; `crates/` and `tools/`
  untouched since `58fb0e7`; the corrected L cell against `errcount`'s event count); Task 3's by a
  read of the one-line diff. Cost if wrong: a prose slip survives to the final review.
- **The gate was launched on `0766a76`**, the last code commit, before Task 4 was dispatched:
  Task 4 is docs-only and changes no test, so the gate's tree and the merged code differ only by
  README, status-log and spec text — to be proven at merge by
  `git diff 0766a76..<merge> --stat -- crates tools` being empty.
- **Task 4:** three runs, not two, everywhere the brief said two; the two retired beliefs and the
  one corrected reason stated as corrections with evidence, never as if always known; class B →
  M37, class C parked and not routed, class D retired; the gate figures copied from the measured
  section of the numbers file and never written from the prediction.
- **At close — M37's acceptance criteria.** Two positive-control criteria in the routing and the
  owed list follow from the measurement but are not themselves measured: `dddiagnose`'s twelve
  self-pid `csops`/`proc_info` calls answer `0` after the §4b fix, from any pid; and all six
  B-then-C rows then sit at the RCV wall on every run (0 self-pid `ESRCH` ⇒ the RCV wall — run L
  6 of 6, M35 10 of 10). Ruled in as the routing's acceptance criteria. Cost if wrong: M37 chases
  a criterion the measurement did not license — one sentence to retract.
- **Fix wave — the harness after the gate.** The final review's three harness items are label and
  comment text and one condition in a script that is not a cargo input; they land after the gate
  without a re-run, on the proof that `git diff 0766a76..<merge> --stat -- crates` is empty,
  `sh -n` is clean, and Control 1 was re-run on the edited script with both labels firing. Cost if
  wrong: a shell edit that changes a label the sweep prints goes unmeasured — the re-run Control 1
  in the harness subsection is the measurement.

### The parked gates, and Control 2

`crates/retrace/tests/apple_walls_e2e.rs` (Task 3, `c702bfa` + its fix round `0766a76`; 55
lines): eight `#[test]`s, every one `#[ignore = "…"]`, each body the same shape —
`util::record_dynamic(<path>)` then `util::replay`, asserting record exit 0, replay exit 0 and
byte-equal stdout, the statement that becomes true when the wall falls; a binary absent from the
machine is announced with an `eprintln!` (`SKIPPED: … is not present on this machine`) and the
test returns, the same loud-skip shape as the `jq` gates. The assertion message prints the
recorder's stderr tail so a run with `--ignored` shows the wall by name. Each reason carries the
charter's condition-2 evidence — the sweep's label, `rc`/`rp` per face, the three recorder pids
and their regimes, the recorder's own `RECORD ERROR` / panic line with the symbol, the landmark,
the evidence file, the class, and `UN-IGNORE when …`: for `csh`/`tcsh`, when the fd table models
`dup2`; for the six B-then-C rows, when §4b is fixed (M37) **and** the box services the RCV-shaped
message-queue call. The `dddiagnose` reason names all three faces, both `far`s, and which question
stays open. The old category string "replay diverged" appears in no reason. `cargo test -p
retrace --test apple_walls_e2e -- --test-threads=1` → `0 passed; 0 failed; 8 ignored`; `cargo
clippy -p retrace --all-targets -- -D warnings` clean.

**Control 2 (spec §6.2) — every parked gate fails for its measured reason.**
`cargo test -p retrace --test apple_walls_e2e -- --ignored --test-threads=1`, cargo's exit
captured inside the braces before the `tee` (`task-3-control2.log`, excerpted: cargo's
`Finished`/`Running` lines, the eight per-test failure bodies and the `failures:` list between
the `running` block and the `test result` line are omitted here; three of the bodies are quoted
below):

```
pid just before the run: 27976 (0x6d48)
running 8 tests
test automationmodetool_records_and_replays ... FAILED
test csh_records_and_replays ... FAILED
test dddiagnose_records_and_replays ... FAILED
test desdp_records_and_replays ... FAILED
test dyld_info_records_and_replays ... FAILED
test flex_records_and_replays ... FAILED
test launchctl_records_and_replays ... FAILED
test tcsh_records_and_replays ... FAILED
test result: FAILED. 0 passed; 8 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.80s
CARGO_EXIT=101
pid just after the run: 28031 (0x6d7f)
```

Every spawned recorder's pid was inside `[0x4000, 0x8000)` — the trampoline page, run I's regime,
colliding — so every face is run I's face: `csh`/`tcsh` `record exited 101` at the `dup2` assert;
`launchctl`, `automationmodetool`, `desdp`, `dyld_info`, `flex` `record exited 4` with the `brk`
line; `dddiagnose` `record exited 139` with the identical-fault face. One assertion message per
class, verbatim:

```
---- csh_records_and_replays stdout ----

thread 'csh_records_and_replays' (31988841) panicked at crates/retrace/tests/apple_walls_e2e.rs:19:5:
assertion `left == right` failed: /bin/csh: record exited 101 — the wall this gate is parked at, in the recorder's own words:

thread 'main' (31988842) panicked at crates/retrace-core/src/lib.rs:1140:17:
dup2 is not modelled by the M10 fd table (unexercised by any gate guest); implement target-slot allocation before a guest uses it
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
  left: 101
 right: 0
```

```
---- launchctl_records_and_replays stdout ----

thread 'launchctl_records_and_replays' (31989173) panicked at crates/retrace/tests/apple_walls_e2e.rs:19:5:
assertion `left == right` failed: /bin/launchctl: record exited 4 — the wall this gate is parked at, in the recorder's own words:
[retrace] forwarding mach_msg2 task_info (msgh_id 3405) to host (decided allowlist)
[retrace] forwarding mach_msg2 host_get_special_port (msgh_id 412) to host (decided allowlist)
[retrace] refusing mach_msg2 message-queue send (msgh_id 0x400000cf dest 0x1103 send_size 248): the box hosts no message-queue receivers
RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110
  left: 4
 right: 0
```

```
---- dddiagnose_records_and_replays stdout ----

thread 'dddiagnose_records_and_replays' (31988878) panicked at crates/retrace/tests/apple_walls_e2e.rs:19:5:
assertion `left == right` failed: /usr/bin/dddiagnose: record exited 139 — the wall this gate is parked at, in the recorder's own words:
[retrace] forwarding mach_msg2 host_get_special_port (msgh_id 412) to host (decided allowlist)
[retrace] refusing mach_msg2 message-queue send (msgh_id 0x400000cf dest 0x1203 send_size 248): the box hosts no message-queue receivers
[retrace] fall-throughs: 7
guest crashed: pc=0x180302eb0 far=0x6000050040 esr=0x92000045
  left: 139
 right: 0
```

No test passed under `--ignored` (charter condition 3). Two limits of this control, stated: the
helper asserts on the record exit first, so for every gate the control stopped at record and no
replay ran — the reasons' `rp` values are the sweep's measurements, not this run's; and the helper
does not print the recorder's own pid the way the sweep's wrapper shell does, so the pid bracket
above is the measurement of the regime.

### The M37 routing

Class **B**, in the table's order, is M37's scope (charter §3: "whatever M36's table routes to
class B, in the order the table gives"):

1. `csh`, `tcsh` — `dup2` in the M10 fd table (target-slot allocation; the assert at
   `crates/retrace-core/src/lib.rs:1140`).
2. `launchctl`, `automationmodetool`, `desdp`, `dyld_info`, `flex`, `dddiagnose` — M34 §4b: a
   `Scalar` argument is never a pointer (consult M33's `arg_kinds`; the precondition is the `Scalar`
   audit of the whole table M34 named). The fix's positive control must use a pid inside
   `[0x10000, 0x18000)` as well as one inside `[0x4000, 0x10000)`, and `dddiagnose`'s twelve
   self-pid calls must answer `0` after it, from any pid; after it lands, all six rows should sit
   at the RCV wall on every run (both ruled at close — the routing's acceptance criteria, not a
   measurement; the Rulings subsection).

Class **C** — the RCV-shaped message-queue `mach_msg2` behind the same six rows — is **parked, not
routed** (Ruling 1); a decision is owed there (refuse it deterministically as the SEND|RCV shape
is, or model it), by a milestone that is not M37. Class **D** (`yes`) is retired with no gate.
Class **A** and **E2**: none. **E1**: the two harness defects, fixed by Task 1.

### Gate

**575 passed / 0 failed / 10 ignored across 126 test binaries**, on commit `0766a76` — Task 3's
fix commit, the last commit that touches anything cargo compiles; the gate ran on that tree while
Task 4 was written, and Task 4 changed README, status-log and spec text, and the final-review fix
wave after it changed label and comment text in `tools/apple-sweep.sh` — the harness, not a cargo
input, `sh -n`'d and re-controlled (the harness subsection above) — so the merged code and the
gated code differ by nothing cargo reads (to be shown at merge by `git diff 0766a76..<merge>
--stat -- crates` being empty). Every chunk's cargo exit code was captured to a file before
any pipe (`gate/*.exit`): `ws=0 box=0 e2e1=0 e2e2=0 e2e3=0 e2e4=0 bins=0 clippy=0`. Logs were
sanitised (`LC_ALL=C tr -cd '\11\12\15\40-\176' | sed 's/\x1b\[[0-9;]*m//g'`) before parsing.
Wall clock 15:53:23 → 16:11:58 EDT, 2026-09-13 (18.5 min). Zero `SKIPPED` lines: `jq_e2e`,
`jq_file_e2e` and `cpython_e2e` all ran. Script: the M35 `gate.sh` with only its two paths changed
(`.superpowers/sdd/2026-09-13-retrace-m36-sweepmeasure/gate.sh`). The chunks, in M35's shape, with
the sum of each chunk's `test result:` lines:

| chunk | invocation | binaries | passed | ignored | notes |
|---|---|---|---|---|---|
| `ws` | `cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1` | 26 | 152 | 0 | unchanged from M35 |
| `box` | `cargo test -p retrace-box --no-fail-fast -- --test-threads=1` | 37 | 271 | 0 | unchanged from M35 |
| `e2e1`–`e2e4` | `cargo test -p retrace --test <names> --no-fail-fast -- --test-threads=1`, chunked index-free (`xargs -n20`) over the sorted target list, flattened membership `diff`ed against it before anything ran | 20 + 20 + 20 + 2 | 43 + 32 + 63 + 3 = 141 | **8** + 0 + 2 + 0 | **62 targets** (M35: 61), so the groups re-cut: the sum 141 equals M35's 46 + 30 + 64 + 1. The 8 new ignored are `apple_walls_e2e`'s eight gates (in `e2e1`: `automationmodetool_`, `csh_`, `dddiagnose_`, `desdp_`, `dyld_info_`, `flex_`, `launchctl_`, `tcsh_records_and_replays`); the 2 old ones are unchanged, in `e2e3`: `stackoverflow_rust_e2e`'s `a_rust_stack_overflow_strikes_its_own_guard_page` (M21 wall) and `symbols_e2e`'s `cache_symbol_e2e` (M19 wall) |
| `bins` | `cargo test -p retrace --bins --no-fail-fast -- --test-threads=1` | 1 | 11 | 0 | the 11 `debug.rs` unit tests, the chunk CLAUDE.md says never to omit |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` | — | — | — | clean |

152 + 271 + 141 + 11 = 575. Ignored 8 + 2 = 10. Binaries 26 + 37 + 62 + 1 = 126.

**Reconciled against M35's 575 / 0 / 2 over 125, file-by-file**: one `.rs` file differs from
`main` under `crates/` — `crates/retrace/tests/apple_walls_e2e.rs`, new, 8 `#[test]`, 8
`#[ignore]` — and every other count is byte-for-byte M35's:

| file | M35 | M36 | delta |
|---|---|---|---|
| `crates/retrace/tests/apple_walls_e2e.rs` | — | 8 | **+8**, all `#[ignore]`d — new binary (Task 3) |
| every other `.rs` under `crates/` | unchanged | unchanged | 0 (`git diff --name-only b12af1b..HEAD -- crates` lists only the new file) |

Binaries 125 → 126; `--bins` 11 → 11. The tree holds **583** `#[test]` attributes = 573 runnable
+ 10 ignored (M35: 575 = 573 + 2); the run reports 575 passed = 573 + census's 2 (the "+2 twice"
of M33–M35) and 10 ignored — the same 575 as M35's, because nothing runnable moved. Bare
`grep -r '#\[test\]' crates | wc -l`: 576 → 584 (one non-attribute match, as at M34/M35). **The
prediction made from source before the run was 575 / 0 / 10 over 126; the run matched it
exactly.** Parked eight (the charter-authorised M36 exception, spec Ruling 3), un-parked nothing;
this is the first milestone to park more than one gate at once (every earlier step in this log's
gate series moved the ignored count by at most one), and each of the eight carries its measurement in its reason and was red under `--ignored` for it (Control 2).

### What stays owed

M35's list, item by item, with what this milestone discharged struck by name and what it added.

* **Discharged — the sweep's two labelling defects (M35's third entry, M36's E1 rows).** A record
  exit of 4 is now `FAIL … (record error, rc=4: <line>)`, and an identical signal death on both
  sides is `PASS … (identical fault, rc=N)`; both labels are in the three logs above, and the
  harness keeps its evidence. Fixed in `tools/apple-sweep.sh` (`fc53853`, `58fb0e7`), not in
  retrace.
* **Discharged — the "no parked gate" debt.** M23's "the new `brk` wall has no parked gate"
  (`:4583–4585`) and the README's "no parked gate standing for it", quoted by the charter as the
  reason M36 may park: eight gates stand in `apple_walls_e2e.rs`, one per non-clean row that is
  retrace's, each reason measured evidence, each red under `--ignored` for that reason.
* **`csops`' `ERANGE` header write, unmeasured** (M35's first entry, unchanged). Reaching the
  branch needs a signed binary with a blob and a pid that survives §4b — the pid is now known to be
  any pid outside `[0x4000, 0x18000)` for the six guests measured, and M37's fix removes the
  condition. The hoist would capture it; nothing has shown it does.
* **The band's width, still** (M27 → M35). Unchanged; not attempted.
* **The walls behind the six B-then-C rows, restated by this measurement.** M35 named three walls
  behind the `dddiagnose` row in the order they gate each other — §4b, then the RCV-only
  message-queue shape, then "the `brk` M23 parked". This milestone measured the third to be a
  downstream face of the first, not a wall of its own (correction (b) above), and found the same
  two walls behind five more rows. So what is owed is **two** walls on six rows: **§4b** (class B,
  routed to M37 — after it, all six should sit at the RCV wall on every run, from any pid: ruled
  at close as the routing's acceptance criterion, not a measurement); then
  the **RCV-shaped message-queue `mach_msg2`** (`MACH64_SEND_MQ_CALL | MACH64_RCV_MSG`, no
  `MACH64_SEND_MSG`), which `Route::Unsupported` keeps fail-loud because it "has never been
  observed" (`machmsg.rs:100–103`) — observed at M35 once and here 6 of 6 in run L, so a decision
  is owed: refuse it deterministically as the SEND|RCV shape is, or model it; class C, parked, not
  routed (Ruling 1). Neither is "message-queue receivers in the box": the serviced refusal is not
  a wall. What is *not* known is whether a `brk` of M23's kind lies behind the RCV shape; that
  cannot be measured until the shape is modelled.
* **The §4b pid-collision probe** (M34, with M35's addendum and now this milestone's): the
  `Scalar` audit as task 1, the fix being `forward_and_diff` skipping the probe at `Scalar`
  positions — a `Scalar` argument is never a pointer, whatever band it falls in — the `hello_dyn`
  table as its positive control (#145 returns 0 with 4 bytes captured), `dddiagnose` as a second
  (its twelve self-pid `csops`/`proc_info` calls, 75 `err = true` landmarks against 63, must answer
  `0` after the fix from any pid — ruled at close as an acceptance criterion, not a measurement),
  **and a third at a pid inside `[0x10000, 0x18000)`**, the band
  M34's window did not include and this milestone's run O fell in. The part M34 asked of M36 — the
  recorder's pid beside each sweep row — is done. Two questions attach here, both open: why an
  in-range `dddiagnose` run takes the identical malloc crash rather than the `brk` (Finding 3: the
  fork is located inside a `gettimeofday` loop whose recorded replies differ; the cause of the
  branch is not), and the `far` that varies between runs of the crash face (`0x2000050050`,
  `0x6000050040`) while pc and esr do not. Both are retired with §4b or become a new row after it.
* **A `bigcsops`-shaped guest**, only if a later milestone finds a real guest whose `CS_OPS_BLOB`
  exceeds 64 KiB; none in the corpus does (M34).
* **`SET_DYLD_IMAGES` (336/15) serviced above the trace, not forwarded** (M34). Unchanged, and
  re-confirmed pid-independent on the one trace whose `err` breakdown is shown above: it is the
  single `proc_info` failure in run L's `dddiagnose` trace (`336: 1`, 0 self-pid `ESRCH`).
* **The per-page cache backing clamps any `Dest` destination that straddles a 16 KiB
  shared-cache boundary** (M34). Unchanged.
* **`getattrlistbulk` (461) and `getattrlistat` (468)** (M34). Unchanged.
* **`crates/retrace-core/src/machmsg.rs:97–99`, one comment line, owed to M37.** The router
  comment still says the four binaries "`brk` regardless of which of seven refusal codes is
  returned, so they are parked at that wall" — the post-refusal-wall reading correction (b)
  retires. M36 could not touch `crates/*/src` (a hunk there is a defect under the charter), so the
  one-line correction is owed to the first milestone permitted to: M37. Unlike a log line, a code
  comment is current-state text with no forward pointer, and the next reader of the router — the
  milestone that owes the RCV-shape decision, or M37's §4b implementer reading how the refusal is
  serviced — meets a parked "`brk` wall" the measurement says is a §4b consequence.
* **The harness's `identical fault` label is exit-code-shaped.** It fires on `rc = rp ≥ 128`, a
  signal death on both sides; a guest exiting ≥ 128 by design on both sides would be labelled a
  fault with no `guest crashed:` line behind it — and still counted PASS, so the tally cannot
  move. When next touched, anchor it on the recorder's `guest crashed:` / `guest terminated by
  signal` line (`crates/retrace/src/main.rs:29`, `:33`), as the record-error label now anchors on
  `RECORD ERROR:` (fix wave; ruled left as is because the direction is harmless).
* **`crates/retrace-box/tests/truncguard.rs:237`** says "a pid in 16384..=65535 hits the
  trampoline/page-table backings" — true of the fixed backings, now the narrower subset of the
  measured `[0x4000, 0x18000)`. A `tests/` comment M36 was permitted to touch and left so the
  gated tree is untouched; one parenthetical — "(and, M36: the guest's `os_alloc_once` slab at
  `0x10000`)" — owed to M37.
* **Review minors carried:** M34's `the_clamp_reaches_proc_info` leaning on the host running
  more than 16 processes; M35's `failwrite.rs:23` misnomer, the bare block in `forward_and_diff`,
  and `util::record` not scrubbing `RETRACE_TRACE`. From this milestone: `apple_walls_e2e.rs`'s
  helper asserts on the record exit first, so under `--ignored` no replay ever runs for a gate
  whose record fails (the reasons' `rp` values are the sweep's, and the `139/139` face's replay
  was exercised only by the controller's hand replay of the control's trace); the helper does not
  print the recorder's pid, so a future `--ignored` run must bracket the pid counter itself to
  know its regime; the control's trace files were left in the system tempdir as every sibling
  e2e leaves its own; the sweep's `rec_reason` is cut to 200 characters for a `RECORD ERROR:` line and 300 for a
  panic pair (`rp_line` to 200); a skipped corpus binary
  emits no `ROW` line (all 54 are present on this machine, so it never fired); `dladdr` names the
  nearest exported symbol only; and the divergence landmark of a `brk` row moves between runs
  (324 → 326 on `launchctl` at two pids) while the `rec_reason` does not, so the reasons quote the
  line and give the landmark as a datum.
* **Everything M33 left owed and M34, M35 and M36 did not touch:** the per-argument canary fill
  and M32's Control 1 (still unexecuted, still inert); nested-pointer translation (`NestedSource`
  forwarded untranslated, `NestedDest` refused, the `DTRACEHIOC_ADDDOF` residual); `pipe`'s
  return (`Ret::FdPair` is documentation, `x1` uncaptured); the `execve`/`posix_spawn` fail-loud
  assert (M33 Ruling 7, the operator's call); console `writev` mirroring; `__disable_threadsignal`
  (331); `AT_FDCWD` in the 32-bit form real guests pass (M33 Ruling 10); the corpus bias (every
  governed `mach_msg2` still init-time and shallow); the `unexercised` label enforced against a
  census dated 2026-09-12 for syscall numbers and a length census dated 2026-09-13 for M34's
  five, both snapshots; and the `kqueue` cross-version note.
* **Superseded, not owed — three statements and one reading, with this section as their forward
  pointer.** M34 §4b's `[0x4000, 0x10000)` and "roughly half" (`:6734`, `:7088–7089`, `:7152`,
  `:7178–7181`); M23's `brk` as a post-refusal wall (`:4583–4585`; the router comment `machmsg.rs:97–99`
  that carries the same reading is a code comment, not a log line, and is in the owed list above) and M35's
  carriage of it as the third `dddiagnose` wall (`:7713–7716`, `:7739–7743`, `:7846–7848`); M35's
  "all outside the range" reason for its 10-of-10 (`:7683`); and the M36 spec's §4 first reading
  ("C for the five", "the M23 belief confirmed by measurement"). All stand as written where they
  are; corrections (a), (b) and (c) above and spec §11 say what was measured instead. The M28
  `failwrite` datum M35 listed here stays superseded as M35 left it.

## Status: M37-classb — dup2 modelled, a Scalar never probed, and the wall behind csh is fork

M36's table routed exactly two class-B (known-unmodelled) walls to this milestone, in the table's
order: `dup2`, the M33 fail-loud assert that `/bin/csh` and `/bin/tcsh` hit in every pid regime,
and M34 §4b's pid-collision probe, the `forward_and_diff` loop that rewrote *any* register whose
value landed in a guest backing to a host pointer — pids, lengths and offsets included — and that
stood between six corpus rows and the RCV-shaped `mach_msg2` behind them. M37 took both. Class B
was small, so under the charter's own rule (§3: "if class B is small, M37 takes it and M38 does
not exist") **the M32–M38 run ends here**; the last subsection says so. Two things the spec did
not know when it was written are the shape of this milestone. First, a measurement taken *before*
the spec found that the wall behind `dup2` is `fork`: both C shells issue exactly four `dup2`s
and, sixty landmarks later, `fork`, whose pre-fork hook `mach_ports_register` is a complex Mach
message the router does not know — so `dup2` is modelled and `csh`/`tcsh` are **not** freed; their
gates move to `fork` and stay parked there, class C. Second, the audit the §4b fix was
preconditioned on found a wrong row (`madvise`'s `addr` was `Scalar` and the call is forwarded),
and found that the probe had been rewriting **lengths** as well as pids — on the pre-fix tree
CPython's `madvise` failed 18 of 44 times because its length register equalled a mapped IPA. With
a `Scalar` never probed, the six §4b rows sit at the RCV wall from every pid, the identical crash
and the libdispatch `brk` M36 measured are gone, and the two open questions M36 attached to them
are retired with the defect. Three ledger rulings changed the design as written: a faked console
close now retires its slot on **both** sides (spec §7's "M9's deferral stands" is retired), an
alias closes the generic way, and `dup2`'s target is bounded.

The milestone's own numbers: **two** walls taken; **one** slot kind (`FdSlot::Console(u8)`) and
**one** table operation (`FdTable::dup2`) added, the console predicates moved from a number test in
`retrace-arch` to **one** shared method per question on `Box_`; **five** `dup2` guards watched
failing (controls 1 and 2, 1b beyond the brief, the console-close control, the tampered-return
test) and **one** for the `Scalar` skip; **190** `Scalar` positions over **129** rows audited statically against their prototypes, **one** finding fixed,
**17** pointer-typed positions kept `Scalar` with citations and **0** overturned by review;
**three** full sweeps of the 54-entry corpus with **every** trace kept (45/9/0 each, the same nine
labels in each), **0** self-pid `ESRCH` in every kept trace where M36 had measured 11–12; **272 =
272 = 272 = 272** `EFAULT`s on `Scalar`-bearing rows across baseline and the three regimes, none
new; **eight** parked gates moved in place to their measured walls, **8 of 8** red under
`--ignored` for the new reason; the **two** code comments M36 owed and **one** CLAUDE.md sentence
corrected; **52** evidence files committed under `docs/sweep-evidence/2026-09-13-m37/` (the README, 63,736
bytes after its fix round, plus 27 `rec.err` and 24 `rp.err`, 29,280 bytes, verbatim from the kept
runs); **seven** commits before this close, `crates/` and `tools/` byte-identical to the gated
commit `09b6bdb`. The gate figures are in their own subsection. (That "byte-identical" was true
at this close and is superseded one subsection later: the final review's fix wave touched
`crates/` — see "Final review, and the fix wave" below.)

### What it set out to do

The charter's entry (`docs/superpowers/specs/2026-09-09-retrace-m32-m38-program-charter-design.md`
§3 "M37–M38 — breadth fixes, routed by the table"): "Scope is **whatever M36's table routes to
class B (§6)**, in the order the table gives. If class B is **empty**, the run ends at M36 … If
class B is **small**, M37 takes it and M38 does not exist. If class B is **large**, M37 and M38
take two slices …". M36's routing (`docs/status-log.md`, "The M37 routing") gave two items:
`csh`/`tcsh` — "`dup2` in the M10 fd table (target-slot allocation; the assert at
`crates/retrace-core/src/lib.rs:1140`)" — and the six `launchctl`-group rows — "M34 §4b: a
`Scalar` argument is never a pointer (consult M33's `arg_kinds`; the precondition is the `Scalar`
audit of the whole table M34 named)", with two acceptance criteria ruled at M36's close: the fix's
positive control must use a pid inside `[0x10000, 0x18000)` as well as one inside
`[0x4000, 0x10000)`, and `dddiagnose`'s twelve self-pid calls must answer `0` after it, from any
pid, so that all six rows sit at the RCV wall on every run. The spec
(`docs/superpowers/specs/2026-09-13-retrace-m37-classb-design.md`) turned those into a design
(§3a the slot kind, §3b the skip and its three-part audit, §3c `RETRACE_SWEEP_KEEP_ALL`, §3d the
gates moved, §3e two comment corrections), four positive controls (§4), the three-sweep acceptance
(§5), the symmetry obligation (§6) and the deliberate non-goals (§7). Charter §9 items 2 and 4 —
measure the wall where the spec says it is, watch every new guard fail — bound each task.

### The measurement taken before the spec, and what it changed

Spec §2a, verbatim — the datum that turned "free `csh`/`tcsh`" into "model `dup2`, move the
gates":

> A throwaway probe (a local patch that let `dup2` through as `dup` + a table write, never
> committed; its patch and both stderr logs are in the session scratchpad `m37pre/`) recorded
> `/bin/csh` and `/bin/tcsh` with `RETRACE_TRACE=1`, stdin `/dev/null`. Both issue exactly four
> `dup2` calls, each followed by `fcntl(new, F_SETFD, FD_CLOEXEC)`:
>
> ```
> [trap] num=90 (0x5a) pc=0x1804b67bc args=[0x0,0x10,…]    dup2(0, 16)   host dup(0)=18
> [trap] num=90 (0x5a) pc=0x1804b67bc args=[0x1,0x11,…]    dup2(1, 17)   host dup(1)=19
> [trap] num=90 (0x5a) pc=0x1804b67bc args=[0x2,0x12,…]    dup2(2, 18)   host dup(2)=20
> [trap] num=90 (0x5a) pc=0x1804b67bc args=[0x10,0x13,…]   dup2(16, 19)  host dup(18)=21
> ```
>
> That is the C shell's classic descriptor move (`SHIN`/`SHOUT`/`SHDIAG`/`OLDSTD` to 16–19, so a
> script's own redirections never clobber the shell's). Every source is a **console** descriptor or
> an alias of one; every target is `>= 16` and free; nothing is displaced. Then both guests
> `ioctl` the aliases (18: `TIOCGETA`, `TIOCGWINSZ`), `sigaction`/`sigprocmask`, and — 130 traps
> later — stop at
>
> ```
> RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64
> ```
>
> whose backtrace `dladdr` reads as `_kernelrpc_mach_ports_register3+0x88` ← `mach_ports_register+0x80`
> ← libxpc `xpc_atfork_prepare+0x50` ← libSystem `libSystem_atfork_prepare+0x28` ← libsystem_c
> `fork+0x24` ← `csh+0x47e50`. **The wall behind `dup2` is `fork`**: `mach_ports_register` (3403,
> a complex message with three port descriptors, unknown to the router) is `fork`'s own pre-fork
> hook, and the `fork` syscall (2) has no `arg_kinds` row either (`forwarded_shape` would refuse it
> loudly). Process creation is a capability the tree does not have — charter class **C**, which M36
> Ruling 1 parks and does not route.

The kept post-fix traces confirm it in every regime (evidence README, "The `dup2` measurement"):
the same four calls with the same arguments and results at indices ±3 (run N `csh` #263 / #265 /
#267 / #269, each `err=false`, each followed by its `fcntl(new, F_SETFD, 1) → 0`), then the 3403
stop. What the pre-spec probe could not see, because the old assert stopped the trace before it,
is one landmark before the wall: both shells `pipe` — and `pipe` is unmodelled (`Ret::FdPair`),
so the guest received retrace's raw host read-end `0x17` in `x0` and its own stale `x1` (the
preceding `sigaction`'s second argument) as the write end, and `fcntl`'d both to `EBADF` (audit 3,
below; Ruling I4).

### `dup2`, modelled — and its controls

Two commits, `4807c30` (12 files, +314/−57) and its fix round `52509a8`. The design's central
move — **the console is a slot kind, not a number** — is carried through one enum variant:
`FdSlot::Console(u8)`, "this slot is (an alias of) console descriptor `n`"; `FdTable::new()` seeds
`[Console(0), Console(1), Console(2)]` with the identity host mapping, `is_open`/`alloc` treat
`Open | Console(_)` as open, and `from_slots` rebuilds the identity host mapping for a `Console(n)`
slot at index `n < 3` only (an alias on a restored box has no host mapping and needs none — replay
forwards nothing; the review verified that no replay path reads `fds.host` at all). `FdTable::dup2(fd,
fd2, host_fd2) -> Result<(u64, Option<i32>), u64>` is the pure table operation both sides run:
`Err(EBADF)` on a closed source or an out-of-range target, `Ok(fd2)` on `fd == fd2`, otherwise slot
`fd2` takes slot `fd`'s **kind** (`Console(n)` propagates), its host mapping becomes `host_fd2`, and
the displaced host mapping is handed back. Record: `forward_and_diff`'s first statement is `if num
== SYS_DUP2 { return self.guest_dup2(args); }` — host `dup` of the source's mapping, the table call,
the displaced mapping closed **iff it is > 2** (retrace's own 0/1/2 are dropped, never closed — the
M9 hazard), the `dup` closed on a table error, `(fd2, false, vec![])` returned; nothing is ever
forwarded as `dup2`, since `dup2(h, fd2)` on the host would overwrite retrace's own descriptor
`fd2`. Replay: inside the generic arm of `ReplaySession::advance`, beside the M10 `allocates_fd`/
`close` mirrors, `FdTable::dup2(args[0], args[1], None)` recomputes `(ret, err)` and byte-compares
it against the recording — a mismatch is a `Divergence` naming both. No returning arm was added:
`grep -c verify_thread` 20 → 20, `self.verify_thread(` call sites 7 → 7. The console predicates
are table-driven at all four call sites through `Box_::is_console_write(num, gfd)` =
`is_write_syscall(num) && console_of(gfd) ∈ {1, 2}` and `Box_::is_console_close`;
`retrace_arch::is_console_write`/`is_console_close` are **deleted**, not kept as a second predicate
that could drift. `arg_kinds`: `SYS_DUP2 => row!(P, [Fd, Scalar])` with an `EXPECTED_DIFFS` entry
("exercised (/bin/csh, /bin/tcsh)"; 90 is in the census). The fixture `dup2_dyn.c` does five
`dup2` shapes plus an `EBADF`; its native stdout `alias\nself=1\nebadf=1\n` (21 bytes) and file
`file18\nfile17\nvia1\n` (19 bytes) were measured before the gate was written and are its
`EXPECT_STDOUT`/`EXPECT_FILE`. `dup2_e2e` records and replays it (stdout byte-equal, the file's
bytes, six `dup2` landmarks including `dup2(40, 19) → (9, true)` and `dup2(f, f)`, `writes`
empty on each).

**Control 1 (spec §4 item 1) — `Box_::is_console_write` reverted to the by-number predicate
`is_write_syscall(num) && (gfd == 1 || gfd == 2)`.** cargo rc 101, both tests failed, verbatim:

```
thread 'console_aliases_are_mirrored_and_displaced_console_slots_write_the_file' (32562887) panicked at crates/retrace/tests/util/mod.rs:159:5:
assertion `left == right` failed: rung guest stdout mismatch — did it reach main? got "alias\nself=1\nebadf=1\nvia1\n", want "alias\nself=1\nebadf=1\n"
  left: [97, 108, 105, 97, 115, 10, 115, 101, 108, 102, 61, 49, 10, 101, 98, 97, 100, 102, 61, 49, 10, 118, 105, 97, 49, 10]
 right: [97, 108, 105, 97, 115, 10, 115, 101, 108, 102, 61, 49, 10, 101, 98, 97, 100, 102, 61, 49, 10]
```

Not the shape the spec predicted. With the by-number predicate the fixture's `write(17,
"alias\n")` is forwarded through the host `dup` of retrace's stdout and so still lands in the
recorder's captured stdout (once), **and** the later `printf("via1\n")` after `dup2(f, 1)` is
mirrored by number into the record's stdout instead of reaching the file; the rung helper compares
the *record's* stdout before it ever replays, so the displaced-console half fires first. The spec's
§4 prose describes the alias half — which the implementer then showed separately.

**Control 1b (beyond the brief) — the predicate kind-correct for the displaced slot, blind to
aliases: `is_write_syscall(num) && matches!(console_of(gfd), Some(1 | 2)) && gfd <= 2`.** cargo rc
101, both failed, spec §4 item 1's shape exactly — record stdout carries `alias\n` through the host
dup, the trace does not, replay lacks it:

```
thread 'console_aliases_are_mirrored_and_displaced_console_slots_write_the_file' (32565309) panicked at crates/retrace/tests/util/mod.rs:165:9:
assertion `left == right` failed: replay 0 stdout diverged from the recording
  left: [115, 101, 108, 102, 61, 49, 10, 101, 98, 97, 100, 102, 61, 49, 10]
 right: [97, 108, 105, 97, 115, 10, 115, 101, 108, 102, 61, 49, 10, 101, 98, 97, 100, 102, 61, 49, 10]
```

**Control 2 (spec §4 item 2) — the replay mirror's `FdTable::dup2` call deleted.** cargo rc 101,
both failed: slot 1 stays `Console(1)` on replay so `via1\n` is mirrored into replay's stdout, and
slot 17 never becomes `Console(1)` so `alias\n` is lost too:

```
thread 'console_aliases_are_mirrored_and_displaced_console_slots_write_the_file' (32566943) panicked at crates/retrace/tests/util/mod.rs:165:9:
assertion `left == right` failed: replay 0 stdout diverged from the recording
  left: [115, 101, 108, 102, 61, 49, 10, 101, 98, 97, 100, 102, 61, 49, 10, 118, 105, 97, 49, 10]
 right: [97, 108, 105, 97, 115, 10, 115, 101, 108, 102, 61, 49, 10, 101, 98, 97, 100, 102, 61, 49, 10]
```

Each mutation was reverted (`git diff | grep -c MUTATION` = 0) and the gate re-run green on the
reverted tree.

**The review's Critical, measured, and its fix round.** The review ran two scratch guests through
the built CLI and found that making the console predicate table-driven on replay had exposed an
M10 asymmetry that had been unobservable since M10: record's console-close arm faked `close(1)` and
never touched the table (slot 1 stayed `Console(1)`), while replay had no such arm and its generic
close mirror retired the slot unconditionally. A guest that writes to fd 1 after closing it —
the daemonize idiom — therefore recorded one stdout and replayed another, rc 0 on both sides, no
`DIVERGENCE`:

```
record rc=0   stdout: before\nafter1\nerr\nafter2\n
replay rc=0   stdout: before\nerr\n                 [retrace] fall-throughs: 0, no DIVERGENCE line
```

and its loud face blamed the wrong side — `close(1); dup2(1, 17)` recorded `(17, ok)` and replay
reported `dup2 divergence: recording says dup2(1, 17) returned (17, err=false), the guest's own
table yields (9, err=true)`. **Ruling C1, Option B**: both sides retire a faked console close's slot
through `FdTable::close` — record's fake arm gained one line, `b.fds_mut().close(args[0])`, and
replay's mirror stays unconditional — so a write after `close(1)` is `EBADF` on both sides, the
kernel's own answer. Spec §7's "M9's deferral stands for console slots" is **retired** by this
ruling. The control is the fixture `closewrite_dyn` (`write(1); close(1); write(1); write(2);
close(2); write(2)`, `main` returning 1 if either post-close write succeeds) under
`closewrite_e2e`, run red on the pre-fix tree first — at the rung helper's exit-0 demand, earlier
than the stdout compares, because the guest's own return value is checked first and it is 1 when
the post-close writes succeed on record:

```
thread 'a_write_after_closing_a_console_fd_is_ebadf_on_both_sides' (32625119) panicked at crates/retrace/tests/util/mod.rs:155:5:
assertion `left == right` failed: rung guest must reach a clean exit(0); 139 means it CRASHED (M6 records that as a successful recording, which is exactly what this assertion exists to reject). stderr:
  left: 1
 right: 0
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.59s
```

then `1 passed` after the fix, and the second face re-measured `record rc=0 stdout=dup2=-1
ebadf=1`, `replay rc=0 stdout=dup2=-1 ebadf=1`, no `DIVERGENCE`. **Ruling I2**: `is_console_close`
narrows to the identity slots, `is_close_syscall(num) && gfd < 3 && console_of(gfd) == Some(gfd)`,
so an alias (`close(16)` after `dup2(0, 16)`) and a displaced slot both close the generic way on
both sides — the spec's §3a worked example wins over its own bullet, because an alias's host
mapping is a `dup`, never retrace's 0/1/2, and faking its close would leak the dup and leave the
slot open forever. Tested against a real `Box_` in `crates/retrace-box/tests/consoleclose.rs`
(three tests: `an_alias_of_stdout_is_a_console_write_but_closes_the_generic_way`,
`a_displaced_console_slot_is_neither_a_console_write_nor_a_console_close`,
`a_closed_identity_slot_is_no_longer_the_console_on_either_predicate`). **Ruling I1**:
`DUP2_MAX_FD = 10240` (`OPEN_MAX`) — `dup2(f, -1)` arrives as `0xffff_ffff` and would have resized
both table vectors to ~40 GiB; a fixed constant because `RLIMIT_NOFILE` is forwarded and therefore
recorder-dependent; `dup2_rejects_a_negative_or_out_of_range_target_with_ebadf` pins `0xffff_ffff`,
`10240` and `u64::MAX` to `Err(EBADF)` growing nothing, `10239` to `Ok`. **I3**: the mirror's
byte-compare had no permanent verified-able-to-fail test (with the compare deleted and the call
kept, `dup2_e2e` passed — stdout equality carries the table controls and nothing carried the
compare); `a_tampered_dup2_return_is_caught_as_divergence` on `fdreplay.rs`'s pattern rewrites the
first successful `SYS_DUP2` landmark's `ret` to 99 and asserts a non-zero replay with `"dup2
divergence"` in stderr. Watched failing, with the compare deleted:

```
thread 'a_tampered_dup2_return_is_caught_as_divergence' (32634804) panicked at crates/retrace/tests/dup2_e2e.rs:84:5:
assertion `left != right` failed: replay must reject a dup2 return the guest's own table cannot produce. stdout:
  left: 0
 right: 0
test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 29.50s
```

The fix round also closed the report's own concern that `guest_dup2` tests `host(fd).is_none()`
while replay tests `!is_open(fd)`: the one state where they disagreed — a faked-closed console
slot with its identity mapping still present — no longer exists. Every console-predicate consumer
was re-run (`bigwrite_e2e`, `cpython_e2e` ran, `fdtable_e2e`, `hello_dyn_e2e`, `jq_e2e` ran,
`panic_e2e`, `stdio_e2e`, `closewrite_e2e`, `dup2_e2e`: all green, cargo rc 0), `retrace-core`
80/0/0 over 10 binaries, clippy clean.

### The §4b fix, its unit control, and the static audit

One commit, `0d02fc1`, and a comment-only fix round `aa8d7b8`. In `forward_and_diff`'s
per-register loop, `let shape = retrace_arch::forwarded_shape(num)` is computed once and a position
whose row kind is `ArgKind::Scalar` sets `hargs[i] = args[i] as i64; continue;` — never
`host_span`-probed, no window, no band. Only `Scalar`: `Fd` positions are host descriptors by then;
`Ptr`/`Path`/`Source`/`Dest`/`Nested*` keep the probe; positions **past a row's arity keep it
too**, on purpose (spec §3b: M30 measured that a stale register pointing into a live buffer plants
a canary 64 KiB past itself, and the windows those registers open are part of what the band logic
reasons about — narrowing that is a separate measurement this milestone did not take). The review
checked every boundary: an exact variant compare, `get(i)` past the slice falls through to the
probe, nothing downstream indexes `windows` by register, and the `Reg` clamp reads `args[li]`
because it wants the guest's count.

**Control 3 (spec §4 item 3) — `scalarprobe`.** The static fixture `scalarprobe.s` opens
`/etc/hosts` and calls `lseek(fd, 0x4000, SEEK_SET)`; `0x4000` is `TRAMPOLINE_IPA`, mapped on every
path, and `lseek`'s offset is a `Scalar`. The box test asserts `ret == 0x4000` and needs no
colliding pid, which is why it is the unit control and the sweeps are the acceptance. Run on the
tree **before** the loop change (fixture + test only), verbatim:

```
thread 'a_scalar_register_holding_a_mapped_ipa_is_forwarded_verbatim' (32673607) panicked at crates/retrace-box/tests/scalarprobe.rs:18:17:
assertion `left == right` failed: lseek returned 0x102fc0000: the offset register was rewritten to a host pointer — forward_and_diff probed a Scalar position (M34 §4b's class)
  left: 4345036800
 right: 16384
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

`0x102fc0000` is the trampoline backing's host address — the kernel seeked the file to it. After
the loop change: `test result: ok. 1 passed; 0 failed; 0 ignored`. The reviewer reproduced the red
independently in a throwaway worktree at the pre-fix commit (`lseek returned 0x1050a8000` — a
different 16 KiB-aligned host address, which is what a per-process anon backing looks like).

**The static audit (spec §3b item 1).** `arg_kinds` has **129 rows, 94 with a `Scalar`
position, 190 `Scalar` positions**, every one listed against its prototype parameter from xnu
`bsd/kern/syscalls.master` and `osfmk/mach/mach_traps.h` (names from the SDK's `sys/syscall.h`) —
the full table is pasted in the evidence README (`task-3-audit-static.md` in the SDD workspace is
its source). No position lies beyond its prototype's arity. The reviewer re-enumerated all 190
from source and matched them 1:1. **One finding, fixed:** `madvise` (75) x0 — prototype `caddr_ut
addr`, row `[Scalar, Scalar, Scalar]`. The kernel neither reads nor writes data through it
(`kern_mman.c`: `madvise_sanitize` → `mach_vm_behavior_set`, no copyin/copyout — M33's reason for
`Scalar`), but the call is **forwarded**, and with `Scalar` meaning "never probed" the raw IPA
would reach the host as the range to `MADV_FREE_REUSABLE` in retrace's own map. Measured on the
real CPython (`-c 'print(1)'`, 44 `madvise` calls per run), three trees, three kept traces:

| tree | `madvise` `(ret, err)` tally over 44 |
|---|---|
| pre-fix `52509a8` | `(0,false)` 26, `(EINVAL,true)` 18 |
| counterfactual: skip + old row (a throwaway build, not committed) | `(EINVAL,true)` 40, `(EPERM,true)` 4 — the 4 `EPERM` at `0xa00020000`/`0xa0002c000` mean the host kernel found those guest IPAs MAPPED in retrace's own process and acted on retrace's map |
| post-fix, row `[Ptr, Scalar, Scalar]` | `(0,false)` 44 |

The reviewer re-decoded all three with an independent script and got the same tallies, and read
the 18 pre-fix failures per **length**: `0x4000` 10/10 `EINVAL`, `0xc000` 6/6, `0x10000` 1/1,
`0x14000` 1/1, while `0x18000`/`0x1c000`/`0x20000` all succeeded — every failing length a mapped
IPA (`TRAMPOLINE_IPA`, `PT_L1_IPA`, M36's slab), every succeeding one not. **§4b was rewriting the
LENGTH register**, and had been since the probe existed; the pid was the instance M34 happened to
find. `Ptr` is in no legacy view, so no `EXPECTED_DIFFS` entry; the post-fix CPython recording
replays bit-identically. **Seventeen pointer-typed positions kept `Scalar`** ([K1]–[K8] in the
evidence README, each with the xnu line it rests on): `munmap`/`mprotect`/`mmap` x0 (VM ranges,
all emulated above the trace — forwarding would be fatal under *either* kind); `sigreturn` x2 (a
token the kernel *compares*, `unix_signal.c:594`; emulated); `bsdthread_create` x0–x2 (register
values; emulated); `bsdthread_terminate` x0/x3 (a range and a wake key; emulated);
`bsdthread_register` x0/x1/x4 — x4 is `pthread_init_data_size`, a **size** under the master's
stale name `targetconc_ptr` (libpthread `kern_support.c:535–566`), and marking it `Ptr` would
have re-created §4b on a length, the very class the `madvise` measurement had just exhibited;
`bsdthread_ctl` x0–x2 (**forwarded**; every arm of `pthread_workqueue.c`'s switch casts them to
port names, priorities or ints, the one dereference being x3, already `Ptr`); `csrctl` x2
(`usersize`, compared to `sizeof` at `kern_csr.c:355/373`); `ulock_wake` x1 (a hashed key,
`sys_ulock.c:978`, no copyin; emulated). Plus two integer-typed address positions
(`mach_vm_deallocate/protect_trap` x1, emulated), listed so the next audit does not rediscover
them. The brief's syntactic rule ("prototype says pointer → fix the row") was applied as the spec's
own hazard statement instead — "hand the host kernel a guest IPA as a pointer" — and the reviewer
checked all 19 line by line: **zero overturned**. Residuals the audit named and did not act on:
`fcntl` x2 / `ioctl` x2 are `Ptr` (probed) and **numbers** for some commands (`F_SETFD`,
`F_SETFL`, `F_NOCACHE`) — §4b's class behind a `Ptr`, unreached on the corpus (CPython's `fcntl`
commands measured: `F_GETPATH`, `F_ADDFILESIGS_RETURN`, `F_CHECK_LV`, `F_SETFD 1`, `F_GETFL`,
`F_GETFD`); `proc_info` x3 is an address in the *target's* map for the region flavors (corpus
callnums 2/5/15 only — inert); and whether a lazily-reclaimed `MADV_FREE_REUSABLE` guest page can
read back as zeros under host memory pressure is unchanged by M37 and unmeasured (the pre-fix tree
already applied it to the 26 that succeeded; now all 44). The fix round's four comment edits
(`aa8d7b8`): the `Reg` clamp comment no longer describes a hazard this commit removed; the
`madvise` row comment names the mmap-area four; the `ArgKind` rustdoc says `Ptr`/`Path` are the
probed *default* so that against `Scalar` the choice is load-bearing; and the audit's [K3] line
no longer repeats the M33 row comment's "asserted against" (next subsection but one).

Post-fix summary lines, cargo exit codes captured to files before any pipe: `retrace-box`
**280 / 0 / 0 over 39** (38 targets incl. `scalarprobe` + `Doc-tests`); `retrace-arch` 43 / 0 / 0
over 4; `retrace-guest` 12 / 0 / 0 over 3; `hello_dyn_e2e`/`failsys_e2e`/`dup2_e2e`/`bigread_e2e`
7 / 0 / 0 over 4; clippy clean.

### Audit 2 — `scalar-writes` — and audit 3 — baseline vs N

Both are Task 4's (`f2026c0`, fix round `09b6bdb`), read off every kept trace with the scratchpad
reader `m37reader` whose source is pasted in the evidence README (modelled on M36's `errcount`;
modes `scalar-writes`, `errs`, `efaults`, `selfpid`, `slabmap`, `calls`, `last`, `low`); the
reviewer confirmed the pasted source byte-identical to the built binary and re-derived every
number below with it or with a reviewer variant. The pre-fix baseline is Task 1's: the sweep
harness gained `RETRACE_SWEEP_KEEP_ALL=1` (`83dcc5c` — one condition in `keep_row`, the `rp.out`
copy, a header line; a control showed both branches on a three-binary list) and the `main` binary
`648d4cf` (sha256 `f14844ec…3f026`) was swept at pids 2021–4321, all below `0x4000`, `TALLY
45/9/0` with M36 run L's nine labels, **54 traces kept** (1.7 GB).

**Audit 2 (spec §3b item 2) — is any `Scalar` position's value inside a recorded `writes`
region?** Over the baseline (54), N and I (53 each — `yes.bin` had been deleted before the logged
pass; `yes`'s logged evidence is the baseline, S and M36 `keep-I/yes.bin` traces, ~3.3 million
`Scalar` positions each, 0 hits), S (54), M36's `keep-{O,L,I}` (27) and M35's `ddd-keep*` (13):

| corpus | traces | hits | in which traces |
|---|---|---|---|
| baseline (pre-fix, non-colliding) | 54 | 9 | `desdp` 3, `dyld_info` 3, `flex` 3 |
| N (post-fix, non-colliding) | 53 | 9 | the same |
| I (post-fix, `[0x4000,0x10000)`) | 53 | 9 | the same |
| S (post-fix, `[0x10000,0x18000)`) | 54 | 9 | the same |
| M36 O/L/I (pre-fix) | 27 | 27 | the same three × 3 runs |
| M35 `ddd-keep*` (pre-fix) | 13 | 0 | — |

**Not the 0 the brief expected, and every hit is one shape that is not a probe**: `mmap` (197) x0
with `flags = 0x40012` (`MAP_FIXED | MAP_PRIVATE | MAP_UNIX03`), `ret == x0`, the write region
exactly `[x0, x0 + len)` — the three segments of one non-cache dylib each of those three guests
maps `MAP_FIXED` from a file, e.g. run N `desdp` #270–#272 verbatim from `scalar-writes-N.log`:

```
HIT …/keep-N/desdp.bin #270 syscall 197 x0=0xa00554000 inside write [0xa00554000,0xa00560000) args=[0xa00554000, 0xc000, 0x5, 0x40012] ret=0xa00554000
HIT …/keep-N/desdp.bin #271 syscall 197 x0=0xa00560000 inside write [0xa00560000,0xa00564000) args=[0xa00560000, 0x4000, 0x3, 0x40012] ret=0xa00560000
HIT …/keep-N/desdp.bin #272 syscall 197 x0=0xa00568000 inside write [0xa00568000,0xa00570000) args=[0xa00568000, 0x8000, 0x1, 0x40012] ret=0xa00568000
```

`mmap` is emulated above the trace: a file-backed `MAP_FIXED` goes through `place_fixed`, which
`pread`s the file bytes into the anon backing at the address the guest named and records them as
this landmark's write (CLAUDE.md "SPTM / anon-only memory"). The call never reaches
`forward_and_diff`, its x0 was never probed before M37 and is never forwarded after it; the write
is the box's own staging, not a kernel dereference; the position is [K1]. **Ruled (audit 2)**: the
number the audit owes is hits on a *forwarded* syscall, or on any position other than [K1] — **0**
in every corpus, pre- and post-fix; the README states the raw 9/9/9/9/27/0 and the exclusion, and
spec §11 records the rule refinement. The `mmap` row is unchanged (`Ptr` there would document a
probe the arm never performs). Cost if wrong: an emulated arm's `Scalar` that IS a pointer — inert,
because emulated arms never probe.

**Audit 3 (spec §3b item 3) — the pre-fix baseline against post-fix run N, row by row.** Labels
(result, rc, rp, reason) for all 54 rows: identical except the two expected moves, `csh`/`tcsh`
from `recorder panicked: … lib.rs:1140:17: dup2 is not modelled …` (rc 101) to the 3403 record
error (rc/rp 4/3). Landmarks on the six §4b rows differ by a few (`launchctl` 329/338,
`automationmodetool` 338/343, `desdp` 365/366, `dyld_info` 364/365, `flex` 366/362, `dddiagnose`
382/383) — run-to-run variation of the guest's own path (pairs of `sigprocmask` appear and vanish,
allocation addresses shift), and two *pre-fix* runs show the same spread (M36 run L `launchctl` 338
vs the Task 1 baseline 329, both non-colliding, both the RCV label), so no landmark difference is
attributed to M37. `errs` per syscall number over the 53 baseline/N pairs (`yes` excluded as
killed mid-run): **identical for 48 of 53**; five binaries differ (six rows — `ps` twice), each
adjudicated by name:

| binary | baseline → N | adjudication |
|---|---|---|
| `csh` | `ioctl`(54) 3 → 7, `fcntl`(92) 0 → 2 | The N trace is longer (261 → 331 events): the baseline panicked at the `dup2` assert before recording it. Over N's first 261 events `errs` is **identical** to the baseline's; every extra `err` is past the old stop — `ioctl` `FIODTYPE`/`TIOCGETA` on the new aliases 17 and 18 answered `ENOTTY` (the sweep's stdout/stderr are files, #271–#274), and two `fcntl(F_SETFD)` `EBADF` at #329/#330 whose name is **`pipe`** (42), unmodelled (`Ret::FdPair`): at #328 the guest received retrace's raw host read-end `0x17` in `x0` (not in the guest table → `translate_fds` answers `EBADF` without forwarding) and its own stale `x1` `0x27fe298` (the second argument of the `sigaction` at #327, which `pipe`'s second return never overwrote) as the write end, and `fcntl`'d both. Same shape in I (#327–#329) and S (#331–#333) and in `tcsh`. Class B known-unmodelled, one landmark before the class-C `fork` wall. Not a probe delta. |
| `tcsh` | same | Same: identical over the baseline's 265 events; N #326 `pipe` → `0x17`, stale `x1` `0x27fe288`; #327/#328 `fcntl` `EBADF`. |
| `date` | `madvise`(75) 4 → 0 | Task 3's expected delta (a): the four baseline failures are `madvise(…, len 0xc000, 7)` → `EINVAL` — `0xc000` is `PT_L1_IPA`'s backing, mapped in every guest, so the pre-fix probe rewrote the LENGTH; post-fix all four return 0. |
| `zsh` | `madvise`(75) 2 → 0 | Same, two calls with `len 0xc000`. |
| `ps` | `madvise`(75) 32 → 0 | Same class: 30 × `len 0x100000` plus one `0x40000` and one `0x7c000`, all three mapped in that guest (`[0x40000, 0x44000)`, `[0x4c000, 0x454000)` in the final snapshot). All 32 return 0 post-fix. |
| `ps` | `sysctl`(202) 0 → 1 | **Not the fix.** `sysctl({CTL_KERN, KERN_PROCARGS2, pid}, 3, buf, &len)` → `EINVAL` at N #7916, the 5th of `ps`'s 15 per-process argument fetches; the same 15 succeed in the baseline, in I and in S. The target had exited between `ps`'s `KERN_PROC` listing (#269) and this call — `ps`'s own stdout (byte-identical on record and replay, so the row is `PASS`) prints that row as `35890 ttys001 0:00.00 (caffeinate)`, the parenthesised form `ps` uses when `KERN_PROCARGS2` fails, where the baseline printed `59315 ttys001 0:00.00 caffeinate -i -t 300`. `sysctl`'s `Scalar` positions are `namelen` (3) and `newlen` (0), unmapped IPAs in every regime; the host's process table is a forwarded input. |

**The `EFAULT` check with `ret` in view** (review I3 — the README's first draft said "no `ret=14`
appeared on any `Scalar` row in any post-fix trace", which is false): the `errs` mode counts `err`
per number without `ret`, so it cannot see an `EINVAL`→`EFAULT` swap at equal count; the reader's
`efaults` mode closes the gap. Count: **272 in the baseline less `yes`, 272 in N, 272 in I, 272 in
S** — 218 × `__mac_syscall` (381), 53 × `ioctl(3, 0x80086804)` (54), 1 × `writev_nocancel` (412,
`ed`) in every corpus: every dynamic guest carries five at dyld time and seven a sixth. With
`--list`, the per-binary `(num, x0, x1)` sequence is identical between the baseline and each of
N/I/S for all 53 binaries (159 pairs, 0 different; the reviewer's own comparison agreed), indices
identical except a late sixth shifted by the landmark spread above. So no `EFAULT` appears or
disappears anywhere and every one present pre-dates M37. **Regime independence** (spec §2b's
claim over the whole corpus): `errs` N-vs-I and N-vs-S, 53 pairs each, identical for 52 — the only
difference is the `ps` `sysctl` above, present in N only. No syscall's error count moves with the
recorder's pid any more.

### The three acceptance sweeps (spec §5)

Binary commit `aa8d7b8` (`git diff aa8d7b8 --stat -- crates/` empty at build; sha256
`58387f16…9aa1d7`, signed copy `d991f7c0…852b3a4`), script `tools/apple-sweep.sh` at `83dcc5c`, list at
`c1e4eb4`, every row kept, pid counter read with `sh -c 'echo $$'` and advanced with
`/usr/bin/true` loops (counter at 38104 → 62,000 spawns → wrapped to 727 → N; +13,500 → I;
+45,400 → S), each run one detached invocation. The slab is where M36 said: in every kept §4b-row
trace of all three runs the `mach_vm_map(size 0x8000, flags 0x49000001)` landmark returns
`0x10000` (reader `slabmap`; `csh` #128, `dddiagnose` #182 — M36's indices), so S is the slab
regime and I the trampoline-page regime.

| run | `pidstart` | recpid min–max | regime | `TALLY` | `SWEEP_EXIT` |
|---|---|---|---|---|---|
| **N** | 754 | 765–3291 (`0x2fd`–`0xcdb`) | below `0x4000`, non-colliding | `pass=45 fail=9 skip=0` | 0 |
| **I** | 17113 | 17124–20042 (`0x42e4`–`0x4e4a`) | inside `[0x4000,0x10000)`, the trampoline page | `pass=45 fail=9 skip=0` | 0 |
| **S** | 66152 | 66163–68793 (`0x10273`–`0x10cb9`) | inside `[0x10000,0x18000)`, the `os_alloc_once` slab | `pass=45 fail=9 skip=0` | 0 |

`awk` over the 54 `ROW` lines of each log, verbatim: `run N: ROW lines=54 recpid min=765 (0x2fd)
max=3291 (0xcdb) outside [1,0x4000)=0 empty=0`; `run I: ROW lines=54 recpid min=17124 (0x42e4)
max=20042 (0x4e4a) outside [0x4000,0x10000)=0 empty=0`; `run S: ROW lines=54 recpid min=66163
(0x10273) max=68793 (0x10cb9) outside [0x10000,0x18000)=0 empty=0`. Every human line that is not a
bare `PASS`, verbatim from `sweep-{N,I,S}.log`:

Run N:

```
pidstart=754
FAIL /bin/csh (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64)
FAIL /bin/launchctl (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /bin/tcsh (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64)
FAIL /usr/bin/automationmodetool (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/desdp (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/dyld_info (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/flex (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/dddiagnose (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/yes (timed out after 30s recording)
TALLY pass=45 fail=9 skip=0
SWEEP_EXIT=0
```

Run I:

```
pidstart=17113
FAIL /bin/csh (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64)
FAIL /bin/launchctl (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /bin/tcsh (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64)
FAIL /usr/bin/automationmodetool (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/desdp (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/dyld_info (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/flex (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/dddiagnose (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/yes (timed out after 30s recording)
TALLY pass=45 fail=9 skip=0
SWEEP_EXIT=0
```

Run S:

```
pidstart=66152
FAIL /bin/csh (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64)
FAIL /bin/launchctl (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /bin/tcsh (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64)
FAIL /usr/bin/automationmodetool (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/desdp (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/dyld_info (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/flex (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/dddiagnose (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape)
FAIL /usr/bin/yes (timed out after 30s recording)
TALLY pass=45 fail=9 skip=0
SWEEP_EXIT=0
```

**Every label identical across the three regimes** — spec §5's prediction holds exactly: `csh`/
`tcsh` at 3403, the six at the RCV shape, `yes` the watchdog, everything else `PASS`, no
`identical fault` row in any run (M36 run I's `dddiagnose` face, `rc=139` both sides after 11
self-pid `ESRCH`, did not recur; a `dddiagnose` crash or `brk` at any pid would have been the red).
The landmarks per row and run, from the `ROW` lines (`csh` 331/330/334, `tcsh` 329/332/332,
`launchctl` 338/330/331, `automationmodetool` 343/340/339, `desdp` 366/366/365, `dyld_info`
365/367/366, `flex` 362/369/366, `dddiagnose` 383/380/384), vary by the run-to-run spread audit 3
explains and are not part of the label. Every replay of a record-error trace ran out of events at
`landmark == events` (checked on all 24). The three `yes.bin` (~354 MB each, a recorder SIGKILLed
mid-`write` loop) were deleted after their labels were recorded, as ruled at pre-flight; `yes`'s
`rec.err` is kept per run.

**Acceptance — self-pid `ESRCH` (the M36 counting rule: `num ∈ {169, 170, 336}`, `ret == 3`,
`err == true`, the pid register — `args[0]` for `csops`/`csops_audittoken`, `args[1]` for
`proc_info` — equal to the row's `recpid`).** The rule was positive-controlled on M36's kept
colliding traces before use: `dddiagnose` keep-I (pid 18781) → 12 calls, **11** `ESRCH`; keep-O
(pid 74909) → 13 calls, **12** — M36's numbers, reproduced again by the reviewer. Then over every
kept trace: sum **0** in I (54 traces), **0** in S (54), **0** in N (53). `yes` in run I was
re-recorded alone inside the band (pid 20384) after its sweep trace had been deleted before its
count — the implementer's ordering error, ruled acceptable as `yes` is the class-D watchdog row
and the count is over kept traces of the same band. The nine rows:

| row | N calls / ESRCH | I calls / ESRCH | S calls / ESRCH |
|---|---|---|---|
| `csh` | 7 / 0 | 7 / 0 | 7 / 0 |
| `tcsh` | 7 / 0 | 7 / 0 | 7 / 0 |
| `launchctl` | 12 / 0 | 12 / 0 | 12 / 0 |
| `automationmodetool` | 12 / 0 | 12 / 0 | 12 / 0 |
| `desdp` | 12 / 0 | 12 / 0 | 12 / 0 |
| `dyld_info` | 12 / 0 | 12 / 0 | 12 / 0 |
| `flex` | 12 / 0 | 12 / 0 | 12 / 0 |
| `dddiagnose` | 13 / 0 | 13 / 0 | 13 / 0 |
| `yes` | (deleted before the count) | 5 / 0 (pid 20384, re-recorded) | 5 / 0 |

Both criteria M36 ruled at its close are met by this table: `dddiagnose`'s self-pid calls
(thirteen carry the pid, of which M36's run O had answered twelve `ESRCH`) answer `0` from any
pid, and the six rows sit at the RCV wall in all three regimes.

### The gates moved, and Control 2

`crates/retrace/tests/apple_walls_e2e.rs` rewritten in place (spec Ruling 4): eight `#[test]`s,
every one `#[ignore]`d, the helper and the body shape unchanged from M36, each reason the **new**
measurement in two shapes. `csh`/`tcsh`: "M37 wall, class C (new subsystem: process creation),
parked, not routed. … `dup2` was the M36 wall and is modelled (M37 t2) — the four calls …, each
followed by `fcntl(new, F_SETFD, 1)`, now record and succeed (run N landmarks #263/#265/#267/#269);
the row now stops ~60 landmarks later at `record error, rc=4: RECORD ERROR: unsupported mach_msg2
at pc 0x1804adc34: msgh_id 3403 …` — `mach_ports_register` (task.defs 3400+3 …) from libxpc
`xpc_atfork_prepare` ← `libSystem_atfork_prepare` ← `fork` …; behind it `fork`(2) itself, which
has no row. Identical in runs N/I/S (recpids …; landmarks …; 0 self-pid ESRCH in every kept
trace). Evidence …. UN-IGNORE when the box models process creation." The six: "M37 wall, class C,
parked, not routed. … the B half (M34 §4b) is retired: with `Scalar` positions never probed, runs
N/I/S (recpids …, non-colliding / [0x4000,0x10000) / [0x10000,0x18000) — the trampoline page and
the guest's os_alloc_once slab, the two regimes whose M36 face was the libdispatch `brk`) all stop
at `record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options
0x404000102: …`, rc/rp 4/3 (…), landmark …, 0 self-pid ESRCH in every kept trace (12 landmarks
carry the recorder's pid and all succeed). Evidence …. UN-IGNORE when the box services the
RCV-shaped message-queue call." No `…` survives in the file (the reviewer's grep); the landmarks
quoted in the `csh`/`tcsh` reasons are run N's exactly, I and S at ±3 (stated in the evidence
README, not in the reason).

**Control 2 — every moved gate fails for its new reason.** `cargo test -p retrace --test
apple_walls_e2e -- --test-threads=1` → rc 0, **0 passed / 0 failed / 8 ignored**. With
`--ignored`, at recorder pids 72339–72381 (`0x11a93`–`0x11abd`, the slab regime — M36's run-O
regime, whose face was the `brk` on all six rows and the `dup2` assert on two) → rc 101,
**0 passed / 8 failed**, each at its new line (the report's summary of the eight failure bodies):

```
/usr/bin/automationmodetool: record exited 4 … RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape
/bin/csh: record exited 4 … RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64
/usr/bin/dddiagnose, /usr/bin/desdp, /usr/bin/dyld_info, /usr/bin/flex, /bin/launchctl: the options 0x404000102 line
/bin/tcsh: the msgh_id 3403 line
test result: FAILED. 0 passed; 8 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.65s
```

The reviewer reproduced it independently at pid 73640 (`0x11fa8`, the slab): 0 passed / 8 failed,
2 × the 3403 line and 6 × the RCV line. M36's two stated limits of this control stand: the helper
asserts on the record exit first, so no replay ran (the reasons' `rp` values are the sweep's), and
the helper does not print the recorder's pid, so the bracket is the measurement of the regime.

### Two comment corrections M36 owed, and one sentence in CLAUDE.md

- `crates/retrace-core/src/machmsg.rs` (the router's refusal comment, M36's correction (b)):
  "the other four `brk`'d regardless of which of seven refusal codes was returned. M36 measured the
  `brk` those four binaries reach as libdispatch's own crash on a §4b-failed `proc_info`, not a
  consequence of this refusal — with a correctly-forwarded pid (M37) they reach the RCV-shaped
  message-queue call instead (…), and stay parked there rather than forcing a proxy design for a
  quarter of the set." The reviewer checked that M23's "other four" (`automationmodetool`, `desdp`,
  `dyld_info`, `flex`) are among the six RCV rows.
- `crates/retrace-box/tests/truncguard.rs:237` (M36's owed parenthetical): the precondition comment
  now states M36's measured window "[0x4000, 0x18000): the trampoline and page-table backings plus
  the guest's own os_alloc_once slab at 0x10000" and "Retired by M37: a `Scalar` is never probed,
  so the pid register is forwarded verbatim in every regime; the LISTPIDS shape is kept because it
  still isolates the clamp"; and the sentence below it says only the two positions past the row's
  arity, `x6`/`x7`, are still probed — true of row 336's arity 6.
- **CLAUDE.md, "Guest threads"**, corrected in place at this close (Ruling): "Forwarding
  `bsdthread_create` is not merely wrong but whole-process fatal (…), so it asserts" was **false**
  — no such assert exists; the emulating arm at `crates/retrace-core/src/lib.rs:1006` sits before
  the generic forward arm and that ordering is the only guard (the generic arm's asserts are
  `is_signal_syscall`, the workq pair and `writes_via_nested_pointer`). Found by the Task 3 review
  as a wrong supporting fact the audit's [K3] line had repeated; verified by the controller at the
  arm and by grep. The sentence stood in CLAUDE.md from M14 (`06a58d1`) to M37. The same claim in
  the M33 `SYS_BSDTHREAD_CREATE` row comment (`crates/retrace-arch/src/lib.rs`, "forwarding is
  asserted against in retrace-core because it is whole-process fatal") is **owed**, not fixed:
  `crates/` stays byte-identical to the gated commit.

### Rulings

The spec's four (§8) and every ruling the ledger recorded, each with what it cost if wrong where
the ledger recorded one.

- **Spec Ruling 1:** `dup2` is in scope although it frees no row — the table routed it, and the
  measurement that `fork` lies behind it changes the claim, not the work. Cost if wrong: a model
  with no corpus consumer beyond a fixture; every shell-like guest issues it.
- **Spec Ruling 2:** the console is a slot kind (one enum variant carried through
  `slots()`/`from_slots` for free), not a parallel vector that checkpoints must carry beside the
  table. Cost if wrong: `FdSlot` is public and one match arm in every consumer.
- **Spec Ruling 3:** the audit is three measurements, all in this milestone. Cost if wrong: two
  extra full sweeps (~8 min). (It found the `madvise` row, the length class, and `pipe`.)
- **Spec Ruling 4:** the gates are rewritten in place, not duplicated.
- **Pre-flight (a):** `yes.bin` files (~350 MB each, killed-run records) may be deleted from every
  keep dir after the row's label is recorded — the audit excludes `yes` by spec. Cost if wrong:
  none (the label is the evidence for a class-D row).
- **Pre-flight (b):** Task 2's `Box_::is_console_close` keeps M9's fake for `Console(_)` slots
  exactly, per spec §7 — **superseded by C1 and I2 below** within the same task.
- **Task 1's review replaced** by the controller's mechanical check: a 6-line diff read in full,
  a control showing both branches of the condition, the baseline's numbers re-derived from the
  log. Cost if wrong: a wording nit in the header, caught by the final review.
- **C1, Option B** (Task 2 review, Critical): BOTH sides retire a faked console close's slot
  through `FdTable::close`; spec §7's "M9's deferral stands for console slots" is RETIRED;
  write-after-`close(1)` is `EBADF` on both sides, the kernel's own answer; `closewrite_dyn` +
  `closewrite_e2e` is the control. Cost if wrong: a corpus guest that closes 1 and writes anyway
  now gets `EBADF` — which is what it gets natively; the sweep's labels are the check (Task 4:
  none moved).
- **I2:** `is_console_close` narrows to identity slots (`gfd < 3 && console_of(gfd) ==
  Some(gfd)`); alias closes go the generic way on both sides — the spec's worked example wins over
  its bullet.
- **I1:** `DUP2_MAX_FD = 10240` (`OPEN_MAX`), a fixed constant because `RLIMIT_NOFILE` is
  forwarded and recorder-dependent.
- **M5** (noted, owed): a displaced-then-closed slot < 3 (`dup2(f, 1); close(1)`) is never
  re-allocated by `open`, where the kernel returns 1, because `alloc`'s floor is 3 — symmetric,
  silent; M37 is the first change that makes a `Closed` at index < 3 reachable from dispatch.
- **M6** (noted, acceptable): a host `dup` failure on record records `(errno, true)`; replay
  recomputes `(fd2, false)` and diverges loudly with an attributable message — recorder
  environment, not guest behaviour; a record-time `RECORD ERROR` would avoid writing a trace that
  can never replay (judgment call, left).
- **Task 3 fix round's re-review replaced** by the controller's check (a 12+/8− comment diff
  matching the four requested sentences verbatim). Cost if wrong: a comment sentence, caught by
  the final review. And its collateral: the M33 row comment and CLAUDE.md's "asserts" claim are
  a doc correction owed at the close (README/CLAUDE.md are edited in place), not a Task 3 change.
- **Audit 2:** the 9/9/9/9/27/0 hits are ONE shape — emulated `SYS_MMAP` x0 with `MAP_FIXED`,
  `ret == x0`, the write region `[x0, x0+len)` = `place_fixed`'s own staging — never through
  `forward_and_diff`; not a red: the licence is "0 hits on any FORWARDED position", identical pre-
  and post-fix; the README states the raw number and the exclusion; spec §11 records the rule
  refinement. Cost if wrong: an emulated arm's `Scalar` that IS a pointer — inert.
- **88,472 bytes** of evidence accepted (the brief's "well under 100 KB" was a target; the 190-row
  table and the reader source are what the spec asked to paste). **`keep-I/yes.bin`** deleted
  before its `selfpid` count and re-recorded in-band (pid 20384, 5/0): accepted. The controller's
  own dispatch slip — "x1 for csops" — corrected: x0 is the pid (`retrace-arch/src/lib.rs:801`);
  the implementer used the prototypes.
- **I4 (Task 4 review):** `pipe`(42) is unmodelled (`Ret::FdPair`) and `csh`/`tcsh` now EXERCISE
  it — the guest receives retrace's raw host read-end and its own stale `x1` — one landmark before
  the class-C `fork` wall. Class B known-unmodelled behind class C; not a red, not a new gate (the
  rows are parked at `fork` already). The `retrace-arch` `FdPair` rustdoc's "`/bin/zsh` issues it
  and never uses the pair" corrected in the fix round (M37 touches `retrace-arch`; the rustdoc
  compiles under `--doc`, 43/0/0); modelling `pipe` joins the owed list. Cost if wrong: a comment.
- **The gate was launched on `09b6bdb`** — the last commit touching anything cargo compiles — with
  Task 5 dispatched concurrently and reading the tally at the end; `crates/` and `tools/` stay
  byte-identical to it through the close. (Superseded by the final review's ruling below: the fix
  wave touched `crates/` and the gate was re-run.)
- **Task 5 corrects CLAUDE.md** (above); the M33 row comment stays owed. (Discharged by the fix
  wave — it rode along once `crates/` was touched anyway.)

### Gate

**590 passed / 0 failed / 10 ignored across 130 test binaries**, on commit `09b6bdb` — Task 4's
fix commit, the last commit that touches anything cargo compiles; the gate ran on that tree while
Task 5 was written, and Task 5 changed README, status-log, spec and CLAUDE.md text only, so the
merged code and the gated code differ by nothing cargo reads (to be shown at merge by `git diff
09b6bdb..<merge> --stat -- crates tools` being empty — **no longer the case**: the final review's
fix wave is one commit on top of this close that touches `crates/`, and the gate was re-run on it;
the figures are in "Final review, and the fix wave" below, which supersedes this subsection's
totals). Every chunk's cargo exit code was captured
to a file before any pipe (`gate/*.exit`): `bins=0 box=0 clippy=0 e2e1=0 e2e2=0 e2e3=0 e2e4=0
ws=0`. Logs were sanitised (`LC_ALL=C tr -cd '\11\12\15\40-\176' | sed 's/\x1b\[[0-9;]*m//g'`)
before parsing; the script's own summary line reads `binaries=130 passed=590 failed=0
ignored=10`. Wall clock 19:55:34 → 20:15:15 EDT, 2026-09-13 (19.7 min). Zero `SKIPPED` lines:
`jq_e2e` (1 passed, 8.75 s), `jq_file_e2e` (2, 14.48 s) and `cpython_e2e` (2, 34.70 s) all ran.
Script: the M36 `gate.sh` with only its two paths changed
(`.superpowers/sdd/2026-09-13-retrace-m37-classb/gate.sh`). The chunks, in M36's shape, with the
sum of each chunk's `test result:` lines:

| chunk | invocation | binaries | passed | ignored | notes |
|---|---|---|---|---|---|
| `ws` | `cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1` | 26 | 154 | 0 | M36's 152 + `retrace-guest`'s two new `_parses` tests, in its existing lib binary |
| `box` | `cargo test -p retrace-box --no-fail-fast -- --test-threads=1` | 39 | 280 | 0 | M36's 271 + `fdtable` 5 + `consoleclose` 3 + `scalarprobe` 1; two new binaries (37 → 39), `Doc-tests retrace_box` still present |
| `e2e1`–`e2e4` | `cargo test -p retrace --test <names> --no-fail-fast -- --test-threads=1`, chunked index-free (`xargs -n20`) over the sorted target list, flattened membership `diff`ed against it before anything ran | 20 + 20 + 20 + 4 | 44 + 32 + 56 + 13 = 145 | **8** + 0 + 2 + 0 | **64 targets** (M36: 62 — `closewrite_e2e` 1, `dup2_e2e` 3, both sorting before `faultlog` and so both landing in `e2e1`, which re-cut every later group boundary by two); the sum 145 = M36's 141 + 4. The 8 ignored are `apple_walls_e2e`'s eight gates (in `e2e1`), each now printing its M37 reason; the 2 old ones are unchanged, in `e2e3`: `stackoverflow_rust_e2e`'s `a_rust_stack_overflow_strikes_its_own_guard_page` (M21 wall) and `symbols_e2e`'s `cache_symbol_e2e` (M19 wall) |
| `bins` | `cargo test -p retrace --bins --no-fail-fast -- --test-threads=1` | 1 | 11 | 0 | the 11 `debug.rs` unit tests, the chunk CLAUDE.md says never to omit |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` | — | — | — | clean |

154 + 280 + 145 + 11 = 590. Ignored 8 + 2 = 10. Binaries 26 + 39 + 64 + 1 = 130 (123 test
executables plus the 7 `Doc-tests` harnesses, the convention since M14).

**Reconciled against M36's 575 / 0 / 10 over 126, file-by-file**: 17 files under `crates/`
differ from `main` (`648d4cf`); six change their `#[test]` count, every other one is unchanged:

| file | M36 | M37 | delta |
|---|---|---|---|
| `crates/retrace-box/tests/fdtable.rs` | 10 | 15 | **+5** (four `dup2` table tests, T2; the `DUP2_MAX_FD` bound test, T2 fix round) |
| `crates/retrace-box/tests/consoleclose.rs` | — | 3 | **+3**, new binary (T2 fix round, ruling I2) |
| `crates/retrace-box/tests/scalarprobe.rs` | — | 1 | **+1**, new binary (T3, the unit control) |
| `crates/retrace-guest/src/lib.rs` | 9 | 11 | **+2** (`dup2_guest_parses`, `scalarprobe_guest_parses`) |
| `crates/retrace/tests/dup2_e2e.rs` | — | 3 | **+3**, new binary (T2; the tampered-return control in the fix round) |
| `crates/retrace/tests/closewrite_e2e.rs` | — | 1 | **+1**, new binary (T2 fix round, ruling C1's control) |
| every other `.rs` under `crates/` | unchanged | unchanged | 0 |

+15 runnable; `#[ignore]` attribute lines 10 → 10 (8 `apple_walls_e2e`, 1 `stackoverflow_rust_e2e`,
1 `symbols_e2e` — the eight moved in place, nothing parked or un-parked). Binaries 126 → 130 (two
`retrace-box`, two `retrace` e2e); `--bins` 11 → 11. The tree holds **598** `#[test]` attributes =
588 runnable + 10 ignored (M36: 583 = 573 + 10); the run reports 590 passed = 588 + census's 2
(the "+2 twice" of M33–M36: `census.rs` runs in its own binary and again inside
`legacy_equivalence`'s `#[path]` include). Bare `grep -r '#\[test\]' crates | wc -l`: 584 → 599
(one non-attribute match in a `legacy_equivalence.rs` comment, as at M33–M36). **The prediction
made from source before the run — `task-5-numbers.md`, 590 / 0 / 10 over 130, with per-chunk
expectations `ws` 154, `box` 280, e2e 145, `bins` 11 and the `e2e1` re-cut — matched the run
exactly.** Spec §9 had predicted 582 / 0 / 10 over 128: it counted the spec's own seven tests and
two binaries, and could not count the eight tests and two binaries the Task 2 fix round and the two
guest `_parses` tests added afterwards (spec §11.1 corrects it). `retrace-box` ran as a whole
package, so its `Doc-tests` harness could not be dropped (M24's lesson); `retrace` ran per-target
in four groups plus `--bins`.

### Final review, and the fix wave

The whole-branch review (`648d4cf..9b013fd`, read-only, every "measured" below run by the
reviewer on the branch binary and on Task 1's kept `baseline/retrace-648d4cf`) returned **Needs
one fix wave — 1 Critical / 0 Important / 6 Minor**. The `dup2` model, the `Scalar` skip, the
eight moved gates, the docs and the gate all checked out; the Critical was not in `dup2` but in
the alias-producing syscall beside it.

**C1 — `dup(1)`/`dup(2)` bind a console alias as `Open`.** `bind_returned_fd`
(`retrace-box:3172–3177`: `alloc()` → `Open`, then `bind(gfd, host_ret)`) is called for every
`allocates_fd` syscall, `SYS_DUP` included (`SYS_DUP => row!(F, [Fd])`), and replay's mirror at
`retrace-core:2359` does the same `alloc()`. Neither side consults `console_of(args[0])`, so a
`dup` of a `Console(n)` slot yields an `Open` slot on both sides — the tables agree,
`is_console_write` answers `false` on both, and record forwards the write through the host `dup`
of retrace's own stdout while the trace carries nothing. Measured through the CLI, three guests:

1. `dup_dyn.c` — `write(1,"a")`; `d = dup(1)`; `write(d,"via-dup")`; `dup2(1,17)`;
   `write(17,"via-dup2")`: **record rc 0, stdout `via-dup\na\nvia-dup2\n`** (`via-dup` printed
   by the host, out of order, ahead of the mirror); **replay rc 0, stdout `a\nvia-dup2\n`**; no
   `DIVERGENCE` line. The `dup2` alias is mirrored (M37 works); the `dup` alias is not.
2. `dup_only.c` (the `dup(1)` half alone) on **main's** `baseline/retrace-648d4cf`: record
   `via-dup\na\n`, replay `a\n`, rc 0/0 — the `dup`-alone leak is pre-existing (M10), not
   introduced here.
3. `saverestore.c` — the shell's `>` idiom: `write(1,"one")`; `saved = dup(1)`;
   `dup2(open("/dev/null"), 1)`; `write(1,"to-devnull")`; `dup2(saved, 1)`; `write(1,"two")`:
   native stdout `one\ntwo\n`; **main `648d4cf`: record rc 101**, `panicked at
   crates/retrace-core/src/lib.rs:1140:17: dup2 is not modelled …` (**loud**); **branch
   `9b013fd`: record rc 0, stdout `two\none\n`; replay rc 0, stdout `one\n`; 0 `DIVERGENCE`
   lines** (**silent**). After `dup2(saved, 1)` slot 1 is `Open` — the kind `FdTable::dup2`
   faithfully copies from the `dup`-made slot — so every later stdout write is forwarded to a host
   dup-of-a-dup of retrace's stdout: on the recorder's terminal, never in the trace, never on
   replay.

Why Critical rather than pre-existing-and-out-of-scope: it is the M9 class verbatim and the one
failure the determinism oracle cannot see (record and replay agree with each other); the branch's
own Task 2 review rated exactly this shape Critical (its C1, also pre-existing since M10) and it
was fixed in-branch; the branch turned a **loud** failure into a **silent** one for an idiom every
shell uses; and `dup` — the other alias producer, modelled since M10 — was named nowhere: the
README's new bullet read as complete for aliases, the rustdoc and the replay comment were careful
to say `dup2`, and the owed list named `F_DUPFD` only. **`dup` was MISSING from the owed list** —
there was nothing to remove from it; this milestone's own audit of "descriptor-producing calls
left unmodelled" counted `fcntl(F_DUPFD)` and `pipe` and did not count the one that was modelled
and wrong.

**Ruling: fix in code, this fix wave, not park.** (1) The spec's claim — the console mirror is
decided by the slot's *kind* — is exactly what `dup` breaks, so copying the kind on `dup`
completes §3a rather than adding scope. (2) The alternative was a *new* `#[ignore]` gate, itself a
charter halt condition, for a wall that is ~15 symmetric lines. (3) A loud→silent regression in
failure mode is not mergeable. Minors M1–M6 ride along, since `crates/` is touched and the gate
re-runs anyway. Cost if wrong: one gate re-run and ~40 lines.

**The fix, symmetric by construction (symmetry rule 1).** `FdTable::dup(src) -> Result<u64, u64>`:
`Err(EBADF)` if `src` is not open, else `alloc()` and copy `slots[src]` onto the new slot — a
`Console(n)` source makes a console alias, an `Open` source a plain duplicate; the host mapping
stays the caller's, as with `alloc` + `bind`. Record: `forward_and_diff`'s bind step calls
`fds.dup(gargs[0])` for `SYS_DUP` and then `bind(g, host_ret)` (the host `dup` is still forwarded
like any fd-producing call — `dup(h)` returns a fresh host descriptor and touches none of
retrace's — so only the binding changed; the table cannot refuse there, since `translate_fds`
already answered `EBADF` for a source with no host mapping, and it panics by name if it ever
does). Replay: the M10 fd mirror at the `alloc()` site calls `fds.dup(args[0])` for `SYS_DUP` in
its place, keeps the existing compare against the recorded return, and reports a source the
replay table has closed as an `fd divergence` through the same channel. Same method, same
argument, both sides; no new returning arm; `verify_thread` 7 → 7; `retrace-trace` untouched,
`TRACE_MAGIC` still `RT\x00\x09`.

**The control, red then green.** `dupkind_dyn.c` is probe 1 followed by probe 3 in one `main`,
every string distinct; `dupkind_e2e` demands, through the rung helper, that the recording's
stdout be the guest's own (`a\nvia-dup\nvia-dup2\none\ntwo\n`, computed natively and
hard-coded), that replay's equal the recording's, and rc 0/0. Run before the fix it **failed at
the record-stdout compare**: `got "via-dup\ntwo\na\nvia-dup2\none\n", want
"a\nvia-dup\nvia-dup2\none\ntwo\n"` — the two `dup`-routed lines printed by the host ahead of the
mirror — and through the CLI the same binary gave record rc 0 `via-dup\ntwo\na\nvia-dup2\none\n`,
replay rc 0 `a\nvia-dup2\none\n`, 0 `DIVERGENCE` lines, the review's measurement reproduced.
After the fix: 1 passed; record == replay == native, rc 0/0, 0 `DIVERGENCE` lines. Three
`FdTable::dup` unit tests beside it (`fdtable.rs`: a `Console` source yields `Console(n)` at the
alloc floor and `dup2` of that alias back onto 1 restores the kind; a closed or never-opened
source is `EBADF` and opens nothing; an `Open` source yields `Open`, on the record and replay
shapes). `fdtable_e2e` (M10's fixture, which `dup`s a file) still passes — the `Open` branch end
to end.

**The minors.** M1: `translate_fds`'s "first statement" is now "first statement of the forwarding
path", since M37's `SYS_DUP2` short-circuit precedes it (both the rustdoc and the inner comment).
M2: `guest_dup2` **keeps** its early self-return — dropping it is *not* a no-op, because the
table's self case returns without storing `host_fd2`, so a host `dup` made for it would be neither
kept nor handed back, one leaked descriptor per self-`dup2`; the rustdoc now states that, and why
the two orders agree on every reachable state (an open `fd` is always inside `[0, DUP2_MAX_FD)`).
M3: the conjunct **was** a one-liner and every existing test stayed green, so it is in:
`is_console_close` now also requires `host(gfd) == Some(gfd)`, so an identity slot re-aliased by
`dup2(17, 1)` or the shell's `dup2(saved, 1)` — `Console(1)` at 1 again, host mapping a dup —
closes the generic way (host dup closed, slot retired) rather than being faked and leaking the
dup; sound because the predicate is consulted only by `record_box`'s arm and replay's close mirror
is unconditional. A fourth `consoleclose` test pins it and was red first (`close(1) on the
re-aliased slot must NOT be faked`). M4: `is_console_write`'s rustdoc says "until the guest
`dup2`s over them or closes them, and every alias `dup2` or `dup` created". M5: the
`RETRACE_TRACE` echo prints `[fd17 (console 1)]` — the console the slot stands for beside the
number the guest used (`[fd4 (console 1)] via-dup` on the fixture; the alias landed at 4, since
dyld holds 3). M6: the M33 `SYS_BSDTHREAD_CREATE` row comment now says the arm's position before
the generic forward arm is the only guard and nothing asserts against forwarding it (measured
false at M37) — off the owed list.

**The gate, re-predicted from source.** Four files change their `#[test]` count against the
close above: `fdtable.rs` 15 → 18 (+3), `consoleclose.rs` 3 → 4 (+1), `retrace-guest/src/lib.rs`
11 → 12 (+1, `dupkind_guest_parses`), `dupkind_e2e.rs` new, 1 (+1, a new binary). +6 runnable;
`#[ignore]` attribute lines 10 → 10; `--bins` 11 → 11; binaries 130 → **131** (65 `retrace` e2e
targets; `dupkind_e2e` sorts between `dup2_e2e` and `faultlog`, so it lands in `e2e1` and every
later `xargs -n20` boundary moves by one, the last group 4 → 5). The tree holds **604** `#[test]`
attributes = 594 runnable + 10 ignored (bare grep 605, the one prose match as before); the run
should report **596 passed / 0 failed / 10 ignored over 131 binaries** (596 = 594 + census's 2),
per chunk `ws` 155, `box` 284, e2e 146, `bins` 11. The re-run, on `0f15f2b` (the fix-wave commit,
now the last commit touching anything cargo compiles), 20:58:30 → 21:18:36 EDT: **596 passed /
0 failed / 10 ignored over 131 binaries**, every chunk's cargo exit 0 (`ws=0 box=0 e2e1=0 e2e2=0
e2e3=0 e2e4=0 bins=0 clippy=0`), zero `SKIPPED`; per chunk `ws` 26 / 155, `box` 39 / 284, `e2e1`–`e2e4`
20 + 20 + 20 + 5 binaries / 44 + 30 + 52 + 20 = 146 with ignored 8 + 0 + 2 + 0, `bins` 1 / 11 —
the prediction matched exactly. The first run's logs are kept beside it (`gate-09b6bdb/`).

### M38 does not exist

Charter §3: "If class B is **small**, M37 takes it and M38 does not exist." Class B was two walls
— `dup2` and one probe rule — and this milestone took both, so **the M32–M38 run ends at M37.**
There is no M38 spec, no M38 plan, and nothing is owed to a milestone by that name; what stands
owed is below, addressed to whichever milestone next takes it up. The charter's other exit — "if
class B is empty, the run ends at M36" — was not the case (M36's table had eight class-B rows
across two walls), and its third — "if class B is large, M37 and M38 take two slices" — was not
either.
Forward pointer (2026-09-16): an M38 does exist after all, under a fresh charter, not this one's —
see `## Status: M38-owed` below.

### What stays owed

M36's list, item by item, with what this milestone discharged struck by name and what it added.

* **Discharged — the §4b pid-collision probe** (M34 → M35 → M36). A `Scalar` position is forwarded
  verbatim; `scalarprobe` is the unit control, the three sweeps the acceptance, 0 self-pid `ESRCH`
  from every regime. The two questions M36 attached to it — why an in-range `dddiagnose` run took
  the identical malloc crash rather than the `brk`, and the `far` that varied between runs of the
  crash face — are retired with it: neither face can be reached with a correctly-forwarded pid
  (three sweeps, 0 `identical fault`, 0 `brk`), and M36's own text said both would be "retired with
  §4b or become a new row after it". M36's suggested `hello_dyn` #145 control was not run; `scalarprobe` and the
  three sweeps were the controls.
* **Discharged — `dup2`** (M10 → M33 → M36, the charter's type specimen of class B).
* **Discharged — `machmsg.rs:97–99`** and **`truncguard.rs:237`**, the two comment corrections.
* **Discharged — spec §7's console-close deferral** (M9's), by ruling C1: both sides retire the
  slot.
* **Discharged — half of "`Scalar` versus `Ptr` is verified by nothing but the reviewer"** (M33):
  the 190 `Scalar` positions are audited three ways; a *new* row's `Scalar` is still checked by
  nothing but its prototype, and `Ptr` versus `Source`/`Dest` was never audited that way.
* **`fork` / process creation — class C, parked, not routed.** Behind `csh`/`tcsh`: `mach_ports_register`
  (msgh_id 3403, a complex message with three port descriptors, from libxpc `xpc_atfork_prepare`)
  and then `fork`(2) itself, which has no `arg_kinds` row. Servicing 3403 alone would move the wall
  one trap; process creation is a capability the tree does not have. The two gates say
  `UN-IGNORE when the box models process creation`.
* **The RCV-shaped `mach_msg2`** (`options 0x404000102`: `MACH64_SEND_MQ_CALL | MACH64_RCV_MSG`, no
  `MACH64_SEND_MSG`) — class C, parked, not routed, now reached by all six rows from **every** pid
  (18 of 18 cells); the decision M36 named is still owed: refuse it deterministically as the
  SEND|RCV shape is, or model it. Whether a `brk` of M23's kind lies behind it is unmeasurable
  until it is.
* **`fcntl(F_DUPFD)` / `F_DUPFD_CLOEXEC`** — M10's other named gap, still unmodelled, still issued
  by no corpus guest (census: `fcntl` present, no `F_DUPFD` command in any kept trace).
* **`pipe`'s return** (`Ret::FdPair`) — no longer merely documentation: `csh`/`tcsh` **use both
  ends** one landmark before their wall (the raw host read-end `0x17`, the stale `x1`), so the
  guest holds a host descriptor unbound in its table and a garbage write end. Capturing `x1` comes
  before any binding model; it changes nothing about the `fork` wall behind it.
* **The probe of positions past a row's arity** (`x6`/`x7` on a six-argument row, every register
  on a zero-arity one) and of `Fd` positions — kept on purpose for M30's band measurement;
  narrowing it is a separate measurement nobody has taken.
* **Discharged (fix wave) — the M33 `SYS_BSDTHREAD_CREATE` row comment** (`crates/retrace-arch/src/lib.rs`,
  "forwarding is asserted against in retrace-core because it is whole-process fatal") — false; no
  such assert; the arm's position is the only guard. CLAUDE.md was corrected at the close; the row
  comment rode along with the final review's fix wave once `crates/` was touched anyway.
* **Discharged (fix wave) — `dup` copying the slot's kind**, which was **missing from this list**
  when it was first written: the close counted the descriptor-producing calls left unmodelled
  (`F_DUPFD`, `pipe`) and not the one that was modelled and wrong. The final review measured it
  (three probes, above) and the fix wave closed it with `FdTable::dup` and `dupkind_e2e`.
* **§4b's residual class behind a `Ptr` that is sometimes a number**: `fcntl` x2 / `ioctl` x2 for
  argument-less commands (`F_SETFD`, `F_SETFL`, `F_NOCACHE`) are probed as pointers and would be
  rewritten if the number equalled a mapped IPA. Measured inert on the corpus (CPython's `fcntl`
  commands: `F_GETPATH`, `F_ADDFILESIGS_RETURN`, `F_CHECK_LV`, `F_SETFD 1`, `F_GETFL`, `F_GETFD`;
  `F_SETFD 1` is `1`, never mapped). A per-command kind is the fix's shape.
* **`MADV_FREE_REUSABLE` on the guest backing** as a pre-existing nondeterminism source: with
  `madvise`'s `addr` now `Ptr` the call succeeds on all 44 CPython calls rather than 26, so the host
  may lazily reclaim those guest pages; a later read of zeros there is nondeterminism no diff window
  captures. Not measured; not made worse in kind by M37.
* **The `alloc` floor (M5)**: a displaced-then-closed slot below 3 is never re-allocated.
  **A host `dup` failure (M6)** records `(errno, true)` and diverges loudly on replay.
* **`csops`' `ERANGE` header write, unmeasured** (M35 → M36, unchanged). The pid condition M36
  named is gone; nothing has yet shown the hoist captures the write.
* **The band's width, still** (M27 → M36). Unchanged; not attempted.
* **A `bigcsops`-shaped guest**, only if a later milestone finds a real guest whose `CS_OPS_BLOB`
  exceeds 64 KiB (M34); none in the corpus does.
* **`SET_DYLD_IMAGES` (336/15) serviced above the trace, not forwarded** (M34). Unchanged; still
  the single pid-independent `proc_info` failure in every `dddiagnose` trace (`336: 1` in run N).
* **The per-page cache backing clamps any `Dest` destination that straddles a 16 KiB shared-cache
  boundary** (M34). Unchanged.
* **`getattrlistbulk` (461) and `getattrlistat` (468)** (M34). Unchanged.
* **The harness's `identical fault` label is exit-code-shaped** (M36). Unchanged — it fired on no
  M37 row, but it still would on a guest exiting ≥ 128 by design on both sides.
* **Review minors carried:** M34's `the_clamp_reaches_proc_info` leaning on the host running more
  than 16 processes; M35's `failwrite.rs:23` misnomer, the bare block in `forward_and_diff`, and
  `util::record` not scrubbing `RETRACE_TRACE`; M36's `apple_walls_e2e` helper asserting on the
  record exit first (so `--ignored` never replays) and not printing the recorder's pid, the
  sweep's `rec_reason` cut at 200/300 characters, a skipped corpus binary emitting no `ROW` line,
  `dladdr` seeing exported symbols only. From this milestone: the `csh`/`tcsh` gate reasons quote
  run N's `dup2` landmarks (I and S at ±3, stated in the evidence README); the evidence `.err`
  files' `repro:` lines name tempdirs that no longer exist (as M36's do); the Task 3 red-run
  output was pasted but not kept as a log (the reviewer rebuilt the pre-fix tree to reproduce it);
  the reader binary postdates most audit logs (every re-run mode reproduced the logged numbers);
  and `dup2_e2e`'s two post-helper assertions restate the helper's byte-equality and cannot fail on
  their own (kept for the failure message each half of the model gets).
* **Everything M33 left owed and M34–M37 did not touch:** the per-argument canary fill and M32's
  Control 1 (still unexecuted, still inert); nested-pointer translation (`NestedSource` forwarded
  untranslated, `NestedDest` refused, the `DTRACEHIOC_ADDDOF` residual); the `execve`/`posix_spawn`
  fail-loud assert (M33 Ruling 7, the operator's call — and now with `fork` measured one wall
  behind `csh`, the same family); console `writev` mirroring (`Box_::is_console_write` covers
  `write`/`write_nocancel` only, aliases included since M37); `__disable_threadsignal` (331);
  `AT_FDCWD` in the 32-bit form real guests pass (M33 Ruling 10); the corpus bias (every governed
  `mach_msg2` still init-time and shallow); the `unexercised` label enforced against a census dated
  2026-09-12 for syscall numbers and a length census dated 2026-09-13 for M34's five, both
  snapshots — plus, since M37, "exercised (/bin/csh, /bin/tcsh)" on `dup2`'s `EXPECTED_DIFFS`
  entry, true as of the same census; and the `kqueue` cross-version note.
* **Superseded, not owed — with this section as their forward pointer.** M36's "M37 for the B
  wall (§4b)" routing and its two acceptance criteria are discharged as written; M36's two open
  questions on the `dddiagnose` faces (Finding 3, the `far`) are retired with §4b rather than
  becoming a new row; the M37 spec's §7 "M9's deferral stands for console slots" (ruling C1), its
  §4 item 1 prose (control 1b's shape), its §3a bullet on faking every `Console(_)` close (ruling
  I2), its §3b.2 rule as literally written (audit 2), and its §9 prediction (582/0/10 over 128) —
  all stand as written in the spec, and §11 says what was measured instead. M34's, M35's and M36's
  superseded lists stay as M36 left them.

## Status: M38-owed — five owed items closed: pipe's pair, F_DUPFD, AT_FDCWD, and two forwards refused

**This is a fresh charter reusing the next number, not the M32–M38 run's M38.** That run ended
at M37 under its own §3 rule and the section above says so ("M38 does not exist"); the number is
reused because it is the next one, and the older section now carries a forward pointer here
(spec R1). M38 took five items off M37's "What stays owed" — none a wall, none a new subsystem,
each independently small, each testable by a repo-owned fixture asserting on the difference it
makes — and re-baselined the sweep once. **Item 1:** `pipe`'s second descriptor never reached the
guest (`Ret::FdPair` had been documentation since M33; `csh`/`tcsh` had been *using* the garbage
pair since M37); now `Event::Syscall` carries `ret1`, `TRACE_MAGIC` is `RT\x00\x0a`, and both
ends are bound as guest descriptors on both sides. **Item 2:** `fcntl`/`ioctl` third-argument
kinds follow the command (`shape_of`) — M37's residual, a `Ptr` that is sometimes a number —
and `F_DUPFD`/`F_DUPFD_CLOEXEC` is a table operation honouring the *guest* minimum. **Item 3:**
`AT_FDCWD` is honoured in the 32-bit form real guests pass (`(v as i32) < 0`), the sentinel bug
that had made `/bin/ls` print `ls: .: Bad file descriptor` since M10 t3 — and the measurement
that followed is the shape of this milestone's sweep: with the sentinel fixed, `ls` *and* `ed`
run on to syscalls the box has no `arg_kinds` row for and fail **loud**, so two rows that had
"passed" by failing identically left the PASS column. **Item 4:** `execve`/`posix_spawn` are
refused in a record arm ahead of the generic forward, with the errno the forward had been
returning (measured `EFAULT` on both) — the operator's ruling of M33's "the operator's call".
**Item 5:** the receive-shaped message-queue `mach_msg2` that had parked six sweep rows since
M35 is refused deterministically with a code **chosen by measurement** (`MACH_RCV_INVALID_NAME`,
the only one all six accept — *not* the spec's default, which was not a tie); `launchctl` runs
to its own clean exit and is un-parked, the other five run 20–50 landmarks further to a missing
row each and are re-parked there, class B.

The milestone's own numbers: **five** items discharged; **one** trace field (`ret1`) and **one**
format bump (`RT\x00\x09` → `RT\x00\x0a`, pre-authorised); **one** new `Box_` method per side
of each model (`bind_returned_pair`, `set_ret1`, `guest_fcntl_dupfd`), **one** new table
operation (`FdTable::dup_from`), **one** new view (`returns_fd_pair`), **one** command-keyed
shape function (`shape_of`, thirteen command constants), **one** refusal predicate
(`exec_refusal_errno`) and **one** router route (`Route::RefuseMqRecv`) with **three** receive
codes named; **four** new fixtures (`pipe_dyn`, `dupfd_dyn`, `atfdcwd_dyn`, `exec_dyn`) and
**four** new e2e binaries (+7 gate tests), each with a pre-fix red run kept in its task report;
**one** tampered-trace control (`a_tampered_pipe_write_end_is_caught_as_divergence`); **18**
measurement cells for the receive code (three codes × six binaries, every candidate's stderr
kept); **one** gate un-parked, **five** re-parked in place, **zero** new `#[ignore]`;
**one** sweep (`pass=44 fail=10 skip=0`), **46** rows unchanged against M37's run N, **8** moved
and every one explained by name, **one** of them (`/bin/ed`) for a reason the plan had not
predicted and the close measured; **no** new returning arm, `verify_thread` **7 → 7**;
**eight** commits before this close (`423cfb1`/`d7a6ede` for the spec and plan; `673eefb`,
`6a29b14`, `2f0883c`, `109af3d`, `b3fc694` for the five tasks; `911214e` for Task 5's fix
round), `crates/` and `tools/` unchanged by
the close except for code comments and seven `#[ignore]` reason strings. The gate figures are
in their own subsection.

### What it set out to do

The spec (`docs/superpowers/specs/2026-09-16-retrace-m38-owed-design.md`) §1: "M37 closed the
M32–M38 run with every remaining Apple-sweep wall classed C … and a long owed list of items that
are not walls: descriptor-return gaps, one sentinel bug …, and two forwards that should never
have been forwards. None needs a new subsystem. This milestone takes the five that are (a)
independently small, (b) each testable by a repo-owned fixture asserting on the difference it
makes, and (c) together worth one gate and one sweep re-baseline." The operator ruled item 4 on
2026-09-16 (refuse deterministically) and pre-authorised the `TRACE_MAGIC` bump item 1 needs.
§7 named what it would not do — no `fork`, no *modelling* of the receive, no uniform `x1`, no
fail-loud default for unlisted `fcntl` commands, no nested-pointer translation — and §5 kept the
M32–M38 halt conditions (a red gate surviving one fix round, any new `#[ignore]`, a class-E row,
scope the spec lacks) with two changes: the bump is not a halt, and the close pushes once. §4
ordered the tasks so the format bump landed first (every evidence trace this run keeps is
`RT\x00\x0a`) and the two refusals last, with the sweep re-baselined once, at the close.

### Item 1 — `pipe`: `ret1` in the trace, both ends bound (`673eefb`)

**What was owed.** `host_svc`'s `asm!` block had `inout("x0") a[0] => ret` and `in("x1") a[1]`
and returned `(ret, carry)`; `apply_and_return` set `x0` alone; `Event::Syscall` had no field
for a second return register. So on a `pipe` the guest received retrace's host read-end,
unbound, in `x0`, and its own stale `x1` as the write end; both host ends leaked in the recorder;
every later use was `EBADF`. M37's audit 3 had measured `csh`/`tcsh` doing exactly that one
landmark before their `fork` wall (`x0 = 0x17`, `x1` = the preceding `sigaction`'s second
argument, two `fcntl(F_SETFD)` `EBADF`s).

**Measured before the change** — the fixture `pipe_dyn.c` (`pipe(p)`; `write(p[1], "pipe\n",
5)`; `read(p[0], …)`; print whether the pair is adjacent and guest-numbered, and the bytes) on
the unmodified recorder, verbatim from the Task 1 report:

```
$ cargo run -p retrace -- record-dyn …/out/pipe_dyn -o /tmp/m38-pipe-pre.bin
pair=0
low=0
write failed
record exit=4
```

`pair=0` (the write end is not the read end + 1), `low=0` (`p[0]` is retrace's unbound host
read-end, ≥ 16; `p[1]` is the stale `x1`), and exit 4 is the fixture's own "write failed" —
the guest's `write` into its stale `x1` was `EBADF`. Natively the same binary prints
`pair=1 / low=1 / bytes=pipe`, exit 0.

**What was built.** `Event::Syscall { num, args, ret, ret1: u64, err, writes, thread }` (the
field order is the wire order); `TRACE_MAGIC = *b"RT\x00\x0a"`, the M24 test renamed
`magic_bumped_for_the_m38_second_return_register` and pinned to it, `rejects_prior_format_version`
looping over `RT\x00\x02` and `RT\x00\x09`. `host_svc` returns `(x0, x1, carry)`
(`inout("x1") a[1] => ret1`); `forward_and_diff` returns `(ret, ret1, err, writes)` with `ret1`
`0` on every row except a `returns_fd_pair` one (a new view over `arg_kinds`, `ret ==
Ret::FdPair`; `allocates_fd(42)` stays false — it means "binds ONE fd"), where `!err` calls
`Box_::bind_returned_pair(host_r, host_w)`: two `alloc` + `bind`s, read end first, xnu's
`retval[0]`/`retval[1]` order. `Box_::set_ret1(ret1)` writes `x1`, and **both** dispatch arms
call it under the identical predicate `retrace_arch::returns_fd_pair(num)` — record before
`set_x0_err_and_return`, replay before `apply_and_return`, neither of which touches `x1`. The
replay mirror sits in the M10 fd block: two `alloc()`s compared to the recorded `(ret, ret1)`,
a mismatch reported through the existing `fd divergence` channel naming both pairs. The capture
is **narrow** (spec R2): 39 other `Event::Syscall` constructions in `retrace-core` carry
`ret1: 0`, and the gate counts them. Fallout the compiler found: 21 three-tuple destructures of
`forward_and_diff` across seven `retrace-box` test files now bind `_ret1`. `Ret::FdPair`'s
rustdoc lost its "unmodelled" paragraph and keeps the M37 evidence as history.

**Its control, red then green.** `pipe_e2e` — `both_ends_reach_the_guest_and_bytes_round_trip`
(the rung helper's byte-equal stdout, `pair=1\nlow=1\nbytes=pipe\n`),
`the_trace_carries_a_guest_numbered_write_end_in_ret1` (exactly one `pipe` landmark, `w == r + 1`,
`r >= 3`, `w < 16` — adjacent *guest* numbers, which neither a host descriptor ≥ 16 nor an
arbitrary stale register can be — and **no other landmark carries a non-zero `ret1`**, the R2
assertion over hundreds of libSystem landmarks), and `a_tampered_pipe_write_end_is_caught_as_divergence`
(rewrite the landmark's `ret1` to 99; replay must exit non-zero with `fd divergence` and `pair`
in stderr — so the mirror's compare is verified able to fail, not merely present). Post-fix:

```
pair=1
low=1
bytes=pipe
record exit=0      replay exit=0 (same three lines)
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 32.28s
```

Task 1's covering runs: `retrace-trace` 14, `retrace-arch` 36 + 2 + 5, `retrace-core` 80 over
10 targets (`machmsgband_dyn` alone 175 s), `retrace-guest` 13 + 1, `retrace-box` 285 over 39
(38 targets + `Doc-tests`), the six fd gates green, clippy clean, `verify_thread` 7. The review
approved it; one observation stands as a noted, unfixed edge: on a *failed* `pipe` both sides
write `x1 = 0` where xnu leaves `x1` untouched — deterministic, symmetric, and libc never reads
`x1` on failure.

### Item 2 — `fcntl`/`ioctl` kinds by command; `F_DUPFD` modelled (`6a29b14`)

**What was owed.** `SYS_FCNTL | SYS_FCNTL_NOCANCEL => row!(P, [Fd, Scalar, Ptr])` and `SYS_IOCTL
=> row!(P, [Fd, Scalar, Ptr])` fixed the third argument's kind per syscall number when it is
command-dependent — an `int` for `F_SETFD`/`F_SETFL`/`F_DUPFD`, a pointer for `F_GETPATH`/
`F_PREALLOCATE`. Under `Ptr` the M37 `Scalar`-skip does not apply, so a small integer was probed
by `host_span` and would have been rewritten had it equalled a mapped IPA (M37 measured it inert
on the corpus: `F_SETFD 1` is `1`). And `F_DUPFD` returned a *new* descriptor that
`allocates_fd(92)` (false) never bound; no corpus guest issued it (M33 census).

**Measured before the change** — `dupfd_dyn.c` (`open` a temp file; `n = fcntl(fd, F_DUPFD,
10)`; print `n`; `write(n, "dupfd\n", 6)`; `fcntl(n, F_SETFD, FD_CLOEXEC)`; then
`a = fcntl(1, F_DUPFD_CLOEXEC, 12)` and `write(a, "alias\n", 6)` through the alias) on the
unmodified recorder, verbatim:

```
$ cargo run -p retrace -- record-dyn …/out/dupfd_dyn -o /tmp/m38-dupfd-pre.bin -- /tmp/m38-dupfd-pre.txt
n=19
[retrace] fall-throughs: 0
exit=5
$ cat /tmp/m38-dupfd-pre.txt
(empty)
```

`n=19` is a **host** descriptor (retrace holds 0–16 open; 17/18 were taken by the open and
libSystem's extra), and exit 5 is the fixture's `write(n, …) != 6` branch — `translate_fds` has
no guest slot 19, so the write was `EBADF` and the file stayed empty. Unit reds first:
`E0425: cannot find value 'F_DUPFD'` (`retrace-arch`), `E0599: no method named 'dup_from'`
(`fdtable.rs`).

**What was built.** In `retrace-arch`: thirteen command constants (`F_DUPFD` 0, `F_GETFD` 1,
`F_SETFD` 2, `F_GETFL` 3, `F_SETFL` 4, `F_NOCACHE` 48, `F_DUPFD_CLOEXEC` 67, `F_PREALLOCATE` 42,
`F_GETPATH` 50, `F_ADDFILESIGS_RETURN` 97, `F_CHECK_LV` 98, `FIOCLEX` 0x20006601, `FIONCLEX`
0x20006602 — checked against the SDK's `sys/fcntl.h`/`sys/filio.h`), `shape_of(num, args) ->
&'static Shape` (two `static` shapes, `[Fd, Scalar, Scalar]`, for the scalar commands; an
unlisted command falls through to `forwarded_shape(num)` and so keeps `Ptr` and stays loud on an
unenumerated number — spec R5, the row comment naming the default and the census date), and
`is_fcntl_dupfd(num, args)`. In the box: `EINVAL` beside `EBADF`; `FdTable::dup_from(src, min)
-> Result<u64, u64>` — the lowest free slot ≥ `min` takes the **source slot's kind** (M37's
`dup` rule, so `F_DUPFD` on a console fd stays a console alias the M9 mirror catches); a
`guest_fcntl_dupfd` short-circuit directly after `guest_dup2`'s at the top of `forward_and_diff`
— the source's host fd (`EBADF` if none), a range check → `EINVAL` (after the source check,
xnu's order), `libc::dup(h)` on the host and **never a host `F_DUPFD`** (its minimum would be a
host number), the table call, `(g, 0, false, [])`; and both shape consultations in
`forward_and_diff` (`translate_fds` and the probe loop — the only two in the box) now call
`shape_of`. The replay mirror, directly after the `dup2` mirror and before the M10
`allocates_fd` block: `if !*err && is_fcntl_dupfd(num, &args)` → `fds_mut().dup_from(args[0],
args[2])` compared to the recorded `ret`. `F_DUPFD_CLOEXEC`'s close-on-exec bit has no
observable in the box (exec is refused, item 4), so both commands share the path.

**Its control.** `dupfd_e2e` — `f_dupfd_honours_the_guest_minimum_and_writes_reach_the_file`
(the recorded `F_DUPFD` returned exactly **10**, a guest number honouring the guest minimum that
no host `dup` could produce; the file's bytes are `dupfd\n`, asserted on the file per
`bigwrite_e2e`'s rule) and `the_trace_carries_f_dupfd_returning_the_guest_slot_and_f_setfd_forwarded_verbatim`
(the `F_SETFD` event's `args[2] == 1` — the assertion documents the kind; it cannot prove the
skip, since the probe's rewrite was inert on `1` anyway). Post-fix, verbatim:

```
n=10
setfd=0
alias
record exit=0          $ cat /tmp/m38-dupfd-post.txt → dupfd
replay exit=0 (same three lines)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 16.31s
```

plus `fdtable.rs`'s `dup_from_takes_the_lowest_free_slot_at_or_above_min_with_the_sources_kind`
and `fdxlat.rs`'s `fcntl_translates_only_its_descriptor`; the census guests that issue `fcntl`
re-run green through the new path (`cpython_e2e` 2/2 and `jq_e2e` 1/1 **ran**, no skip; `dup2`,
`dupkind`, `fdtable` gates green). The review approved it and carried one minor to the final
review: `guest_fcntl_dupfd`'s range guard (`EINVAL` for `min < 0` / `min >= DUP2_MAX_FD`) is
record-side only, where `dup2`'s lives in the table method both sides share — replay's
`dup_from` is guarded only by `!*err`, so a trace claiming success with a huge `min` would
`grow_to` it instead of diverging (M37-I1's shape); the fix's shape is the check inside
`FdTable::dup_from`, unit-tested.

### Item 3 — `AT_FDCWD` in the form real guests pass (`2f0883c`), and the row it un-hid

**What was owed.** `translate_fds`'s sentinel check was `(v as i64) < 0`. libc passes `-2` as a
32-bit `int` in `w0`, so `x0 = 0xfffffffe`, which is positive as an `i64`; the lookup failed and
the guest got `EBADF`. Measured at M33 on `/bin/ls` (twice) and `/bin/ed` (once); present since
M10 t3 (`e67dd65`); and `fdxlat.rs`'s sentinel test passed `AT_FDCWD as u64` — the sign-extended
form no guest produces — so the unit suite was green while the real form failed.

**Measured before the change**, verbatim from the Task 3 report. The rewritten unit test:

```
test at_fdcwd_passes_through_untranslated_in_the_form_the_abi_delivers ... FAILED
thread '...' panicked at crates/retrace-box/tests/fdxlat.rs:71:9:
AT_FDCWD as 0xfffffffe is a sentinel, not a descriptor — it must not be rejected as EBADF
test result: FAILED. 7 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

and the gate, `atfdcwd_dyn.c` (`fstatat(AT_FDCWD, ".", &st, 0)`; print `ok` or `errno=N`) on
the unfixed tree:

```
test a_relative_fstatat_through_at_fdcwd_succeeds ... FAILED
assertion `left == right` failed: rung guest stdout mismatch — did it reach main? got "errno=9\n", want "ok\n"
```

`errno=9` is `EBADF`. **A second red the plan did not predict:** with the fix applied to
`lib.rs` alone the unit test was *still* red, because `fdxlat.rs`'s local `translate()` helper
is a hand-maintained **mirror** of `Box_::translate_fds`'s walk (its own doc says so), still at
`(v as i64) < 0`. The implementer fixed it identically — one line outside the brief's named
range, inside a file the brief targeted, necessary for the brief's own "Step 1's test → pass" to
hold — and the review noted the duplication as a standing risk now carried twice.

**What was built.** `(v as i32) < 0` — a `u64 → i32` cast truncates to the low 32 bits, so
both `0xfffffffe` and `0xffff_ffff_ffff_fffe` read as `-2`, and a real descriptor never has bit
31 set; the comment says which form the ABI actually delivers and cites M33 Ruling 10. The
`AT_FDCWD` and `ArgKind::Fd` rustdocs say the same. `fdxlat.rs` passes `0xfffffffe` first and
keeps the sign-extended form as a second assertion. The review checked that no other 64-bit
fd-sentinel site disagrees (`dup2`'s `fd2` and `F_DUPFD`'s `min` were already `as i32`;
`mmap_file`/`mwl` operate on native `i32`).

**Its control.** `atfdcwd_e2e` asserts the recorded `fstatat64` (470) has **both** `args[0] ==
0xfffffffe` and `err == false` on the same landmark — the first half pins the form the guest
passed, so the fixture cannot pass by accident with a 64-bit sentinel; the second is the fix.
After both fixes: `fdxlat` 8 passed, `atfdcwd_e2e` 1 passed; neighbours (`fdtable_e2e`,
`jq_file_e2e`, `sysbin_e2e`) 8/8; `retrace-box` 263/263 across every target; clippy clean.

**The `/bin/ls` hand check, and what it changed.** The brief expected `ls` to "print a
directory listing". It did not:

```
thread 'main' panicked at crates/retrace-arch/src/lib.rs:938:38:
M33: syscall 461 (461) has no arg_kinds row in crates/retrace-arch/src/lib.rs — it cannot be
forwarded unclassified …
```

`RETRACE_TRACE=1` shows the real `/bin/ls` issuing `fstatat64(0xfffffffe, …)` — `AT_FDCWD` in
exactly the 32-bit form — and it does **not** abort the run with `EBADF` any more; `Bad file
descriptor` appears nowhere. What `ls` then reaches is `getattrlistbulk` (461), which modern `ls`
uses to bulk-enumerate a directory and which has no `arg_kinds` row — a pre-existing owed item
(M34: "`getattrlistbulk` (461) and `getattrlistat` (468). Unchanged."), previously *masked* by
the `EBADF`. **Ruling (Task 3, the ledger's):** the 461/468 rows are NOT added in M38 — spec §5
lists "scope the spec lacks" as a halt, and adding a `Dest` row unmeasured at the end of an
unattended run is the "right conclusion, unmeasured fact" class this repo warns about. Instead
the measurement re-scopes spec §3c and §6: `/bin/ls`'s sweep row moves from a **false PASS** (it
printed an error and exited, identically on both sides) to a **loud** `recorder panicked: no
arg_kinds row for 461` — the honest-gate discipline replacing a silent lie with a named wall —
and §6's "PASS ≥ 45" becomes "≥ 44 with `ls`'s move explained by name (≥ 45 if any of the six
RCV gates un-parks)". 461/468 move to the top of the owed list with the new fact that `/bin/ls`
reaches them. Cost if wrong: the operator wanted `ls` green and must charter a two-row follow-up
(both rows from the SDK prototypes: 461 `[Fd, Ptr, Dest(Reg(3)), Scalar, Scalar]`, 468 `[Fd,
Path, Ptr, Dest(Reg(4)), Scalar, Scalar]`, bounds the caller's size register, `read`'s citation).

### Item 4 — `execve`/`posix_spawn` refused, never forwarded (`109af3d`)

**What was owed.** Rows 59 and 244 were forwarded by the generic arm and failed only because
`NestedSource` is untranslated: the host kernel read `argv` at a guest address and returned
`EFAULT`. Had nested-pointer translation ever landed, a forwarded exec would have **replaced
retrace's own process**. M33 Ruling 7 had left the fail-loud "the operator's call" because
adding it re-parks the CPython launcher test; the operator ruled on 2026-09-16: refuse
deterministically, and keep the launcher test running by keeping the errno.

**Measured before the change** (spec §3d, R4 — the errno is measured, not chosen).
`exec_dyn.c` (`execve("/bin/echo", …)`, then `posix_spawn` of the same with
`POSIX_SPAWN_SETEXEC` — the launcher shim's exact call; print each errno) on the unmodified
recorder with `RETRACE_TRACE=1`, verbatim from the Task 4 report:

```
execve=14
posix_spawn=14
[trap] num=59 (0x3b) pc=0x1804badd0 args=[0x10000061c,0x27ff7f0,0x27ff7e8,0x27ffea8,0x1,0x20]
[trap] num=244 (0xf4) pc=0x1804b50dc args=[0x27ff7d4,0x10000061c,0x27ff6b0,0x27ff7f0,0x27ff7e8,0x27ff7e8]
landmark 245: num=59 ret=14 ret1=0 err=true writes=0 thread=0
landmark 247: num=244 ret=14 ret1=0 err=true writes=0 thread=0
grep -a -c refusing … → 0
```

**Both measure 14 (`EFAULT`), `err` set, no writes** — one value, so `exec_refusal_errno(num)
-> Option<u64>` returns `Some(14)` for both and no per-number split was needed; R4 stands as
written. The zero `refusing` lines are the red half of the gate.

**What was built.** `SYS_EXECVE = 59`, `SYS_POSIX_SPAWN = 244`, `exec_refusal_errno` (with the
measurement, the continuity-not-fidelity reason, and `ENOSYS` (78) named as the one-constant
change if a successor prefers "exec is unmodelled" to be what the guest reads, in its rustdoc).
The record arm `Stop::Syscall { num, args } if retrace_arch::exec_refusal_errno(num).is_some()`
sits immediately **before** the generic BSD arm — symmetry rule 1's ordering, the only guard, as
for `bsdthread_create` — prints `[retrace] refusing execve|posix_spawn (syscall N): exec-in-place
is unmodelled; returning errno 14 without forwarding`, appends `Event::Syscall { ret: 14, ret1:
0, err: true, writes: vec![], thread }` and `apply_and_return`s. The replay mirror sits inside
the generic replay `Syscall` arm directly after that arm's `(num, args)` compare and its
`verify_thread`, before the `SYS_SIGACTION` mirror: recompute the constant, `Divergence("exec
refusal mismatch: …")` if `ret`/`err`/`writes` disagree, else `apply_and_return` + `finish_event`.
**No new returning arm; `verify_thread` 7 → 7.** The two `arg_kinds` rows keep their kinds as
documentation of the prototype; nothing consults them for forwarding (their comments were
rewritten in place at this close — the appended "M38: refused" note under "Forwarded today …"
was now-text, not history).

**Its control — and why the obvious assertion is wrong.** Asserting the recorded event has
`ret == 14` and `writes.is_empty()` is exactly what the *forwarded* call produced (an `EFAULT`
with nothing written), so it cannot tell refusal from forward — honest-gate rule 1. The
difference the arm makes that a forward cannot fake is its stderr line, which only the refusal
prints: `exec_e2e`'s `exec_is_refused_and_says_so` asserts on it, and the CPython launcher test
(`the_launcher_records_and_replays_its_own_posix_spawn_failure`) keeps every assertion it had
(that *is* the continuity check — the guest saw the same errno) and gains the same stderr
assertion; an arm that drifted below the generic forward would go red on it. Green: `exec_e2e`
1 passed; `cpython_e2e` 2 passed (Homebrew Python present — the launcher test **ran**);
`sysbin_e2e` 3; `retrace-arch` 39 (`exec_refusal_covers_execve_and_posix_spawn_only`); clippy
clean. The `/bin/sh` hand check (the corpus's `execve` user, run exactly as the sweep does):
rc/rp 1/1, guest text `Failed to exec /bin/bash as variant for /bin/sh (14: Bad address).`
identical to M37's row, stderr now carrying the refusal line. One observation the report made
and the close repeats so a shifted index is not misread: four recordings of `exec_dyn` had
249/250/252/253 traps — `gettimeofday` fires 16 or 19 times during libSystem init, plus one
`ioctl` that depends on whether retrace's stdout is a pipe — which moves the exec landmarks'
indices (245/247 vs 248/250) but not their content, and each trace replays byte-identically.

### Item 5 — the receive-shaped message-queue `mach_msg2`, refused by a measured code (`b3fc694`, fix `911214e`)

**What was owed.** `route()`'s message-queue branch sent `SEND_MSG | RCV_MSG` with the MQ bit to
`Route::RefuseMqSend` (M23's deterministic `MACH_SEND_INVALID_DEST`) and any other MQ shape to
`Route::Unsupported("message-queue send without the send+rcv RPC shape")`, which aborted the
record with rc 4. The six gates M37 parked all stopped at options `0x404000102` =
`MACH64_SEND_MQ_CALL | MACH64_RCV_TIMEOUT (0x100) | MACH64_RCV_MSG (0x2)` — a **receive with a
timeout**, not a send; the `Unsupported` string misnamed it. M36 had named the decision (refuse
it as the SEND|RCV shape is, or model it) and M37 carried it.

**What was built.** `Route::RefuseMqRecv` for `RCV_MSG` set and `SEND_MSG` clear under the MQ
bit (`Msg2` already carried `options`; nothing new is read); a one-way send stays `Unsupported`
with its string corrected to say what it is; `Route` gained `Debug`. The record arm beside
`RefuseMqSend`'s: writes nothing, returns `MACH_RCV_REFUSAL`, `err: false`, `ret1: 0`, prints
`[retrace] refusing mach_msg2 message-queue receive (rcv_name … rcv_size … options …): the box
hosts no message-queue sender`; the replay mirror beside its twin recomputes and byte-compares —
the standard symmetric posture, not the `ServiceGetSpecialPort` verbatim-apply exception, since
the reply is a constant. No new returning arm; `verify_thread` 7 → 7. Three router tests, red
first (`E0425 MACH_RCV_REFUSAL`, `E0599 RefuseMqRecv`, `E0277 Route: Debug`), then green.

**The code, by measurement (spec §3e, R3).** Three candidates from `osfmk/mach/message.h`, each
built into the recorder, ad-hoc signed, and run record-then-replay against all six binaries
(`measure.sh`, 180 s alarm, stdin `/dev/null`; 18 cells, every cell's `rec.err`/`rp.err` kept
under `docs/sweep-evidence/2026-09-16-m38/`). Every cell recorded **exactly one** receive
refusal (`receive-refusals=1`, the positive control that the new arm and not another path is
what the guest reached). "wall N" = the recorder panicked at the M33 fail-loud on an
**unclassified** guest syscall N — the refusal was accepted and the guest ran on to a call the
box has never had a row for; "brk" = the guest crashed downstream of the refusal:

| binary | `MACH_RCV_TIMED_OUT` (…4003, the spec's default) | `MACH_RCV_INVALID_NAME` (…4002) | `MACH_RCV_PORT_DIED` (…4006) |
|---|---|---|---|
| `launchctl` | proceeds — its own `exit(1)` usage, rc/rp 1/1 | proceeds, 1/1 | proceeds, 1/1 |
| `automationmodetool` | wall `kevent_qos` (374), 101/3 | wall 374 | wall 374 |
| `desdp` | wall `openat_nocancel` (464), 101/3 | wall 464 | wall 464 |
| `dyld_info` | wall 464 | wall 464 | wall 464 |
| `flex` | wall 464 | wall 464 | wall 464 |
| `dddiagnose` | **brk** `pc=0x180302eb0 far=0x2000050050 esr=0x92000045`, 139/139 | **wall** `statfs64` (345), 101/3 | **brk** `pc=0x193bbbca0 far=0xfffffffffffffff0 esr=0x92000004`, 139/139 |

Accepted per code: `TIMED_OUT` **5** of 6, `INVALID_NAME` **6** of 6, `PORT_DIED` **5** of 6.
**Ruling (R3, measured): `MACH_RCV_REFUSAL = MACH_RCV_INVALID_NAME` (0x1000_4002).** Not a tie,
so the R3 default and tie-break did not apply: the semantically faithful "a queue no one can
send to times out" lost on exactly one binary, `dddiagnose`, which data-aborts in the guest
~10 landmarks after the receive under both losing codes and under the winner runs **50**
landmarks further (receive #381 → wall 431) to a next wall of its own. The test
`the_receive_refusal_is_a_receive_code` pins `MACH_RCV_REFUSAL == MACH_RCV_INVALID_NAME` and
`!= MACH_RCV_TIMED_OUT`, so a change to the constant must change the test — the choice is a
measurement, not a preference. The reviewer recounted the table from the 18 `.err` cells and
got the same 5/6/5.

**The six gates, each outcome one of spec §3e's three.** `/bin/launchctl` — record + replay
clean → **un-parked**: it records to its own no-argument usage `exit(1)` (4,484 bytes on stdout,
byte-identical to the host's native `/bin/launchctl` output measured 2026-09-16) and replays
bit-for-bit; its gate body asserts on that outcome (rc 1, the usage prefix, exactly one refusal
line, replay == recording) and **not** on the shared helper's `rc == 0` — a code a weaker
failure would also produce is not a difference. `test launchctl_records_and_replays ... ok`.
The other five — record stops at a **new** wall → **re-parked**, class B (an M33 fail-loud that
an `arg_kinds` row closes), each reason rewritten in the M37 shape: the syscall, the pc, rc/rp
101/3, the divergence landmark (replay's `expected recorded syscall, got None` on the trace with
no terminal event the panic left — not a divergence of its own), the recorder pid and its regime,
the same-wall-under-all-three-codes cross-check, the evidence files, and what un-parks it.
`automationmodetool` → `kevent_qos` (374) at pc `0x1804afa48`, divergence landmark 363
(libdispatch's kevent workloop may be a subsystem of its own behind the row);
`desdp`/`dyld_info`/`flex` → `openat_nocancel` (464) at `0x1804b3954`, landmarks 392/393/398 —
the three are one hard-linked Xcode `xcrun` stub that opens a random-named
`/var/tmp/xcrun_db-XXXXXX`, so the *intervening* path shifts run-to-run while the wall number is
stable across the measurement and the gate run, and a future un-park of any one un-parks all
three; `dddiagnose` → `statfs64` (345) at `0x1804bd0cc`, landmark 431. **None is class E** —
each recorder panics (rc 101) and writes no terminal event, so `rp = 3` is the expected read
past the end, the M37 pattern. `gates.log` is the file's positive control under `--ignored`:
each re-parked gate prints its wall by name (374 / 464 / 464 / 464 / 345). One pid regime (R6):
§4b is retired, and **0 self-pid `ESRCH`** on every winner trace (M37's `selfpid` reader).

**The fix round (`911214e`), two Importants, both factual corrections to permanent records the
task's own evidence contradicted.** (1) `MACH_RCV_REFUSAL`'s rustdoc said `desdp`/`dyld_info`/
`flex` reach `kevent_qos` (374) — the implementer's first cut of the reasons had cloned the
`automationmodetool` template with 374 for all five, was caught and fixed in the gate file
before commit, and had survived in the doc; it now names each binary's real wall, cross-checked
against the 18 `.rec.err` cells, the README table and the reasons. (2) The README said the
recorder pids 54954–55330 were "below `0x4000`, non-colliding" (run N): `0x4000` is 16384, so
every pid was `0xd6aa`–`0xd822`, **inside** M36's old §4b window `[0x4000, 0x10000)` — M37's
regime **I**. **Ruling:** the label is a measurement claim and the measurement says I; the
README and the five reasons now say so, and say why it is irrelevant (§4b retired) and what the
stronger fact is (0 self-pid `ESRCH` with every pid inside the window that pre-M37 answered all
but one of those calls `ESRCH`); `csh`/`tcsh`'s "run N" (pids 1005/2342, genuinely N) untouched.
Five minors deferred to the close were fixed there (below). Covering after the fix: `retrace-core
--lib` 49, `apple_walls_e2e` 1 passed / 7 ignored, clippy clean in a fresh target dir.

### The sweep re-baseline, and the row the plan did not predict

One run (spec §4: once, at the close) of `tools/apple-sweep.sh` over the 54-entry corpus from
the worktree, detached, on the close's binary — `911214e` copied to a scratch path and ad-hoc
signed (sha256 `80daf5c5…39502`) so later `cargo` runs could not swap it under the sweep —
with every non-clean row's stderr and trace kept (`RETRACE_SWEEP_KEEP`), the traces read and
then deleted. `pidstart=87610`; recpids 87626–90026 (`0x1564a`–`0x15faa`), inside M36's old
`[0x10000, 0x18000)` slab window — M37's regime S; one regime, because M37 had shown the pid
selects nothing (R6). Verbatim, every human line that is not a bare `PASS`:

```
FAIL /bin/csh (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64)
FAIL /bin/ed (recorder panicked: thread 'main' (35004702) panicked at crates/retrace-arch/src/lib.rs:944:38: M33: syscall 464 (464) has no arg_kinds row in crates/retrace-arch/src/lib.rs — it cannot be forwarded unclassified …)
FAIL /bin/ls (recorder panicked: thread 'main' (35007007) panicked at crates/retrace-arch/src/lib.rs:944:38: M33: syscall 461 (461) has no arg_kinds row in crates/retrace-arch/src/lib.rs — it cannot be forwarded unclassified …)
FAIL /bin/tcsh (record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64)
FAIL /usr/bin/automationmodetool (recorder panicked: thread 'main' (35011346) panicked at crates/retrace-arch/src/lib.rs:944:38: M33: syscall 374 (374) has no arg_kinds row …)
FAIL /usr/bin/desdp (recorder panicked: thread 'main' (35011394) panicked at crates/retrace-arch/src/lib.rs:944:38: M33: syscall 464 (464) has no arg_kinds row …)
FAIL /usr/bin/dyld_info (recorder panicked: thread 'main' (35011456) panicked at crates/retrace-arch/src/lib.rs:944:38: M33: syscall 464 (464) has no arg_kinds row …)
FAIL /usr/bin/flex (recorder panicked: thread 'main' (35011736) panicked at crates/retrace-arch/src/lib.rs:944:38: M33: syscall 464 (464) has no arg_kinds row …)
FAIL /usr/bin/dddiagnose (recorder panicked: thread 'main' (35011793) panicked at crates/retrace-arch/src/lib.rs:944:38: M33: syscall 345 (345) has no arg_kinds row …)
FAIL /usr/bin/yes (timed out after 30s recording)
TALLY pass=44 fail=10 skip=0
SWEEP_EXIT=0
```

(The `…` above elide the tail of the M33 panic message, which is identical on every row; the
log has it in full.) `awk` over the 54 `ROW` lines: `ROW lines=54 recpid min=87626 (0x1564a)
max=90026 (0x15faa)`. No `identical fault` row, no `replay diverged` row, no class-E row.

**Row by row against M37's run N** (label, `rc`, `rp`, `rec_reason` compared by binary path):
**46 unchanged, 8 moved.** The plan named seven of the eight; the eighth is `/bin/ed`. The tally
is 45/9 → 44/10 = **45 − `ls` − `ed` + `launchctl`**:

- `/bin/launchctl`: `FAIL` 4/3 (the RCV line) → **`PASS` 1/1**. Moved by item 5. Un-parked.
- `/usr/bin/automationmodetool`, `desdp`, `dyld_info`, `flex`, `dddiagnose`: `FAIL` 4/3 (the RCV
  line) → `FAIL` 101/n/a at `kevent_qos` 374 / `openat_nocancel` 464 ×3 / `statfs64` 345, the
  M33 fail-loud. Moved by item 5; re-parked, class B.
- `/bin/ls`: `PASS` 1/1 (printing `ls: .: Bad file descriptor`) → `FAIL` 101/n/a at
  `getattrlistbulk` 461 (kept trace, landmark #258: `fstatat64(0xfffffffe, …) ret=0 err=false
  writes=1`). Moved by item 3; the Task 3 ruling.
- `/bin/ed`: `PASS` 2/2 → `FAIL` 101/n/a at `openat_nocancel` 464. Moved by item 3 — **the row
  the plan did not predict.** Spec §3c had said `ls` and `ed` "were PASS before … and stay PASS;
  their recorded output changes". Measured at the close with two traced records of `/bin/ed`
  (stdin `/dev/null`), one on the M37 baseline binary `retrace-aa8d7b8` and one on the M38
  binary (`docs/sweep-evidence/2026-09-16-m38/sweep/ed.{m37-baseline.,}traced.rec.err`): on the
  M37 binary, `[trap]` line 398 is `num=470 … args=[0xfffffffe,0x100010080,…]` —
  `fstatat64(AT_FDCWD, "/tmp/ed.XXXXXX", …)`, `0x100010080` being `ed`'s scratch-buffer template
  (`strings /bin/ed`), the call libc `mkstemp`'s `_gettemp` makes to check the directory; M33
  measured it `EBADF` ("on `/bin/ed`, once"), and the trace shows what follows: `writev_nocancel(2,
  …)` (its error message, lost to EFAULT), `umask`, `exit(2)`. The M37 `PASS 2/2` was `ed`
  failing to create its scratch buffer on both sides — a determinism agreement about a failure,
  never a run of `ed`. On the M38 binary the same landmark succeeds (sweep trace #258: `ret=0
  err=false writes=1`) and the **next** trap is `num=464 args=[0xfffffffe,0x100010080,0xa02,
  0x180,…]` = `openat_nocancel(AT_FDCWD, "/tmp/ed.XXXXXX", O_RDWR|O_CREAT|O_EXCL, 0600)`,
  `mkstemp`'s open, the `_nocancel` twin of `openat` (463), no row → the M33 loud panic, rc 101,
  no terminal event, no replay. **Ruling (Task 6, the ledger's):** `/bin/ed` is handled exactly
  as `ls` — a silent lie replaced by a named wall; §6's floor is measured 44; the missing-row set
  stays {461, 468, 464, 345, 374} with `ed` added to 464's blockers (now four binaries behind one
  `_nocancel` row); no new `#[ignore]`, since `ed` never had a gate. Cost if wrong: the same as
  the `ls` ruling — a two-row follow-up the operator may want immediately; 464 is one line,
  `openat`'s row shared by its `_nocancel` twin.

The rows the plan said would keep their label did. `/bin/sh` is `PASS` 1/1 with the same guest
text, its stderr carrying the `execve` refusal line (`sweep/sh.rec.err`, a record taken beside
the sweep since a PASS row's stderr is not kept). `/bin/csh` and `/bin/tcsh` are `FAIL` 4/3 at
the same 3403 line (landmarks 336 / 344; M37 N: 331 / 329 — the guest's own spread, plus new
`dup` landmarks), and their **`pipe` landmark moved**, read off the kept sweep traces with a
scratch reader over `retrace-trace` (index = position in the event vector, `Snapshot` #0):

```
csh.bin (recpid 87861, 336 events)
#327 syscall 42 args=[0x27ff300, 0x27fe298, 0x0, 0x5] ret=0x4 ret1=0x5 err=false     pipe → (4, 5)
#328 dup(4) → 6   #329 close(4)   #330 fcntl(6, F_SETFD, 1) → 0
#331 dup(5) → 4   #332 dup(4) → 7   #333 close(4)   #334 close(5)   #335 fcntl(7, F_SETFD, 1) → 0
tcsh.bin (recpid 89125, 344 events): the same nine calls at #335–#343, pipe → (4, 5), both fcntls → 0
```

The guest receives guest fds `(4, 5)`, both bound, moves each end above the C shell's `FSAFE`
with `dup`/`close`, and **both `fcntl(F_SETFD, 1)` succeed** where M37 had two `EBADF`s on a raw
host descriptor and a stale register (spec §7's prediction, measured). The `dup`s are new
landmarks — M37's guest skipped them, since `0x17` and `0x27fe298` were already above `FSAFE`.
Traced runs the same day (`sweep/csh.traced.rec.err`, recpid 91162; `tcsh`, 91174) have the
identical shape at #326/#329/#334 (stop 335) and #328/#331/#336 (stop 337), and the `[trap]`
ordinal equals the landmark index (335 lines ↔ `DIVERGENCE at landmark 335`, the stop being the
one unrecorded trap). The two gate reasons quote the sweep-run landmarks and name the traced
runs; the wall is unchanged, class C. `/usr/bin/yes` is the watchdog as before.

### The close's own edits (Task 6)

Five doc-only minors the task reviews had deferred, in files the close edits anyway: the
`arg_kinds` rows 59/244's comments rewritten in place (they had kept "Forwarded today … the
fail-loud assert … is owed" above an appended "M38: refused" — a code comment is now-text, not
the append-only log; the old forward is one history clause now); `machmsg.rs`'s `MQ_RCV` test
comment says 50 landmarks (381 → 431), matching the constant's doc and the README, and names the
trailer bit `0x0400_0000` in `0x4_0400_0102` that the router ignores; the five re-parked reasons
say "the trace with no terminal event the panic left" rather than "the truncated trace" (the
reader's `truncated` flag means a torn record, which a panic-killed recorder does not leave —
`truncated=false` on every one); the evidence README's legend says `stdout-equal=y` is a real
check only on `launchctl` (both `.out` files are empty on the wall and crash rows) and its
"Files" names the committed `launchctl…rp.out`. Then the `csh`/`tcsh` reasons' `pipe` sentence
(above), the README edited in place, spec §10, CLAUDE.md (`RT\x00\x0a` at both mentions; the
four new e2e gates in the "Commands" list; the `verify_thread` paragraph untouched — still
seven), and this section. Covering runs after the `.rs` edits: `retrace-arch` 39 + 2 + 5 (+ 0
doc), `retrace-core --lib` 49, `apple_walls_e2e` 1 passed / 7 ignored, clippy clean over
`--workspace --all-targets -- -D warnings`, every cargo exit 0.

### Rulings

The spec's six (§8) and every ruling the ledger recorded, each with what it cost if wrong where
the ledger recorded one.

- **R1 — the milestone is M38.** "M38 does not exist" was a statement about the M32–M38
  charter's slot; this is a fresh charter reusing the next number. The old section carries a
  forward pointer; this section's first paragraph says so.
- **R2 — `x1` narrow, not uniform.** Held; `pipe_e2e` asserts no other row carries a non-zero
  `ret1`. Cost if wrong: a guest that reads `x1` after some other two-register syscall (`fork`,
  class C) still sees a stale value — today's state.
- **R3 — the receive code is measured; default `MACH_RCV_TIMED_OUT`; ties to the default.** Held
  in its procedure; the default did not apply (6/6 vs 5/6, not a tie). Cost if wrong: a binary
  that would have proceeded under another code re-parks one wall early; the reasons name the
  code tried and the cross-check under all three.
- **R4 — the exec errno is the measured one, for continuity.** Held, one value (14). Cost if
  wrong: the guest reads `EFAULT` for a call that was refused, not faulted — a wording lie the
  rustdoc owns; `ENOSYS` is one constant away.
- **R5 — unlisted `fcntl`/`ioctl` commands stay `Ptr`.** Held. Cost if wrong: a scalar command
  outside the census that equals a mapped IPA is rewritten — the §4b class, measured inert on
  every command seen.
- **R6 — one pid regime for the re-run of the six.** Held; the label "N" was wrong (regime I)
  and was corrected in the fix round; the sweep's single run was regime S. Cost if wrong: none —
  §4b is retired and 0 self-pid `ESRCH` was measured on every kept trace.
- **Pre-flight (a):** red evidence is taken by building each fixture first and recording it on
  the not-yet-modified tree, instead of the plan's `git stash push -- paths` recipe — the
  worktree shares the stash stack with other sessions. Cost if wrong: none (the evidence is the
  same guest stdout either way). Every task's red is in its report.
- **Pre-flight (b):** the SDD workspace lives inside the worktree (a harness constraint) and is
  copied to the main repo's `.superpowers/sdd/` at the close before the worktree is removed.
- **Task 3 — the 461/468 rows are not added; `ls` moves from a false PASS to a named wall; §6's
  floor becomes ≥ 44 with `ls` explained (≥ 45 if a gate un-parks).** Above.
- **Task 5 — the five new walls' rows (464, 345, 374) are not added; with 461/468 they are the
  successor's measured scope: the missing-row set, with the corpus binaries each blocks.** Cost
  if wrong: four binaries stay parked one milestone longer than two one-line rows would have
  taken.
- **Task 5 fix round — the reasons' "run N" is corrected to regime I along with the README** (the
  label is a measurement claim); the stronger fact is stated. Cost if wrong: wording only.
- **Task 6 — `/bin/ed` is handled exactly as `ls`; the floor is measured 44 = 45 − `ls` − `ed` +
  `launchctl`; `ed` joins 464's blockers; no new `#[ignore]`.** Above.
- **Task 1 (observation, no fix):** a failed `pipe` writes `x1 = 0` on both sides where xnu
  leaves `x1` untouched — deterministic, symmetric, unread by libc on failure.
- **Task 3 (accepted beyond the brief):** the `fdxlat.rs` local `translate()` mirror had to
  take the same one-line fix, or the brief's own "Step 1's test → pass" was false.
- **Task 4 (accepted beyond the brief):** one assertion-*message* phrase in `cpython_e2e.rs`
  ("is forwarded" → "is refused (M38)") so the file carries no stale claim.

### Gate

**617 passed / 0 failed / 9 ignored across 135 test binaries**, on commit `911214e` — Task 5's
fix commit, the last commit that touches anything cargo compiles; the close's edits to
`crates/` are code comments and seven `#[ignore]` reason strings, and the covering runs above
were taken after them. Run by the controller with the M37 `gate.sh` (paths changed), 2026-09-17,
from the worktree; every chunk's cargo exit captured to a file before any pipe: `ws=0 box=0
e2e1=0 e2e2=0 e2e3=0 e2e4=0 bins=0 clippy=0`; logs sanitised before parsing; the script's own
summary `binaries=135 passed=617 failed=0 ignored=9`; zero `SKIPPED` lines (`jq_e2e`,
`jq_file_e2e` and both `cpython_e2e` tests **ran** — Homebrew `jq` and `python@3.14` are
present). The chunks, in M37's shape, with the sum of each chunk's `test result:` lines:

| chunk | invocation | binaries | passed | ignored | notes |
|---|---|---|---|---|---|
| `ws` | `cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1` | 26 | 165 | 0 | M37's 155 + `retrace-arch` 3 + `machmsg` 3 + `retrace-guest` 4 |
| `box` | `cargo test -p retrace-box --no-fail-fast -- --test-threads=1` | 39 | 287 | 0 | M37's 284 + `fdtable` 2 + `fdxlat` 1; `Doc-tests retrace_box` present |
| `e2e1`–`e2e4` | `cargo test -p retrace --test <names> --no-fail-fast -- --test-threads=1`, `xargs -n20` over the sorted target list | 20 + 20 + 20 + 9 | 45 + 31 + 50 + 28 = 154 | **7** + 0 + 2 + 0 | **69 targets** (M37: 65 — `atfdcwd_e2e` and `dupfd_e2e` in the first group, `exec_e2e` opening the second, `pipe_e2e` in it; every boundary moved); 154 = M37's 146 + 3 + 2 + 1 + 1 + `launchctl` now passing. The 7 ignored are `apple_walls_e2e`'s seven; the 2 are `stackoverflow_rust_e2e`'s and `symbols_e2e`'s, in `e2e3` |
| `bins` | `cargo test -p retrace --bins --no-fail-fast -- --test-threads=1` | 1 | 11 | 0 | the 11 `debug.rs` unit tests, the chunk CLAUDE.md says never to omit |
| `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` | — | — | — | clean |

165 + 287 + 154 + 11 = 617. Ignored 7 + 2 = 9. Binaries 26 + 39 + 69 + 1 = 135 (128 test
executables plus the 7 `Doc-tests` harnesses, the convention since M14).

**Reconciled against M37's 596 / 0 / 10 over 131, file-by-file** (`git diff a663051 911214e --
crates`, `+` lines carrying `#[test]`, no `-` lines):

| file | M37 | M38 | delta |
|---|---|---|---|
| `crates/retrace-arch/src/lib.rs` | 36 | 39 | **+3** (`fcntl_and_ioctl_third_argument_kind_follows_the_command`, `f_dupfd_is_recognised_by_number_and_command`, `exec_refusal_covers_execve_and_posix_spawn_only`; `pipe_return_is_a_pair_and_both_are_bound` is a rewrite) |
| `crates/retrace-core/src/machmsg.rs` | 25 | 28 | **+3** (the three router tests) |
| `crates/retrace-box/tests/fdtable.rs` | 18 | 20 | **+2** |
| `crates/retrace-box/tests/fdxlat.rs` | 7 | 8 | **+1** (`fcntl_translates_only_its_descriptor`; the sentinel test is a rewrite) |
| `crates/retrace-guest/src/lib.rs` | 12 | 16 | **+4** (one parse test per new fixture — the plan's §9 forgot these) |
| `crates/retrace/tests/pipe_e2e.rs` | — | 3 | **+3**, new binary |
| `crates/retrace/tests/dupfd_e2e.rs` | — | 2 | **+2**, new binary |
| `crates/retrace/tests/atfdcwd_e2e.rs` | — | 1 | **+1**, new binary |
| `crates/retrace/tests/exec_e2e.rs` | — | 1 | **+1**, new binary |
| every other `.rs` under `crates/` | unchanged | unchanged | 0 |

+20 attributes; `#[ignore]` lines 10 → **9** (`launchctl_records_and_replays` un-parked; no new
one); binaries 131 → **135**; `--bins` 11 → 11. The tree holds **624** `#[test]` attributes =
615 runnable + 9 ignored (M37: 604 = 594 + 10); the run reports 617 = 615 + census's 2 (the
"+2 twice" since M33: `census.rs` runs in its own binary and again inside `legacy_equivalence`'s
`#[path]` include); bare `grep -c '#\[test\]'` 605 → 625 (the one prose match as before). So
596 + 20 + 1 (the un-parked gate now counts as passed) = 617, 10 − 1 = 9, 131 + 4 = 135 — the
tally the gate printed. **Spec §9 had predicted 613 / 0 / 9 over 134** (the plan's "612 + k / 0
/ 10 − k over 135" with k = 1, and §9's own rougher "603+/0/≤10 over 134"): it forgot the four
`retrace-guest` parse tests and counted the binaries as +3 where the four new e2e targets are +4
— both recorded in spec §10. `retrace-box` ran as a whole package (its `Doc-tests` could not be
dropped, M24's lesson); `retrace` ran per-target in four groups plus `--bins`.

### What stays owed

M37's list, item by item, with what this milestone discharged struck by name and what it added;
the first item is new and is the successor's measured scope.

* **The missing-row set — 461, 468, 464, 345, 374 — with the corpus binaries each blocks.**
  `getattrlistbulk` 461 (`/bin/ls`, reached since the sentinel fix), `getattrlistat` 468 (M34's
  pair to it, reached by nothing yet), `openat_nocancel` 464 (`/bin/ed`, `/usr/bin/desdp`,
  `/usr/bin/dyld_info`, `/usr/bin/flex` — the `_nocancel` twin of `openat` 463, precisely the
  documented nocancel trap), `statfs64` 345 (`/usr/bin/dddiagnose` — the fixed-struct twin of
  `fstatfs64`, M29), `kevent_qos` 374 (`/usr/bin/automationmodetool`; libdispatch's kevent
  workloop may be a subsystem of its own behind it). Each is the M33 fail-loud doing its job on
  a number the 2026-09-12 census never saw, because the guests that issue them never got that far
  before M38; each was ruled *not* added here (Tasks 3, 5, 6) as scope the spec lacks and as
  unmeasured rows at the end of an unattended run. Seven sweep rows and five parked gates stand
  behind these five numbers; two of the five are one-line copies of rows that exist.
* **Discharged — `pipe`'s return** (M10 → M33 → M37): `ret1` in the trace, both ends bound,
  `csh`/`tcsh`'s `fcntl`s succeed.
* **Discharged — `fcntl(F_DUPFD)` / `F_DUPFD_CLOEXEC`** (M10): `FdTable::dup_from`, `dupfd_e2e`.
  Still issued by no corpus guest (the census entry is the fixture).
* **Discharged — §4b's residual class behind a `Ptr` that is sometimes a number** (M37):
  `shape_of`. What replaces it, by ruling R5: an *unlisted* scalar command keeps `Ptr` and would
  be probed — unreached on the corpus; a fail-loud default is the successor's decision, weighed
  against turning every unseen command into a sweep failure.
* **Discharged — `AT_FDCWD` in the 32-bit form real guests pass** (M10 t3 → M33 Ruling 10):
  `(v as i32) < 0`, `atfdcwd_e2e`, the `fdxlat` test on the measured form. And the two rows it
  un-hid are the first item.
* **Discharged — the `execve`/`posix_spawn` fail-loud assert** (M2 → M33 Ruling 7): refused
  with the measured errno; `ENOSYS` one constant away if fidelity is preferred to continuity.
* **Discharged in half — the RCV-shaped `mach_msg2`** (M35 → M36 → M37): the decision M36 named
  is taken — **refused**, deterministically, by a measured code. **Modelling** it stays class C:
  a binary that needs a real reply on a port it actually holds would move one wall past the
  refusal, and the reasons say so. Whether a `brk` of M23's kind lies behind it was answered for
  these six (none; five reach a missing row, one exits clean) and for no other binary.
* **Uniform `x1` capture** — new. `x1` is written for `pipe` only (R2); xnu writes it from
  `retval[1]` after every syscall and retrace leaves it stale everywhere else. The field is in
  the trace now, so the uniform version is a measurement a later milestone can take without a
  format break. And the noted edge: a failed `pipe` writes `x1 = 0` where xnu leaves it.
* **`fork` / process creation — class C, parked, not routed.** Unchanged behind `csh`/`tcsh`:
  `mach_ports_register` (3403) then `fork`(2), which has no row. The `pipe` landmark before it
  is clean now; the wall is not moved by that.
* **The probe of positions past a row's arity** and of `Fd` positions — kept on purpose for
  M30's band measurement; unchanged.
* **`MADV_FREE_REUSABLE` on the guest backing** — unchanged, unmeasured.
* **The `alloc` floor (M37 M5)** and **a host `dup` failure (M37 M6)** — unchanged; `dup_from`
  inherits both edges.
* **`guest_fcntl_dupfd`'s range guard is record-side only** — new (Task 2 review, carried to the
  final review): replay's `dup_from` is guarded only by `!*err`, so a trace claiming success with
  a huge `min` would `grow_to` it rather than diverge; the fix is the check inside
  `FdTable::dup_from`, unit-tested. The `EINVAL` branch has no test.
* **`csops`' `ERANGE` header write, unmeasured** (M35 → M37). Unchanged.
* **The band's width, still** (M27 → M37). Unchanged.
* **A `bigcsops`-shaped guest** (M34). None in the corpus.
* **`SET_DYLD_IMAGES` (336/15) serviced above the trace, not forwarded** (M34). Unchanged.
* **The per-page cache backing clamps any `Dest` destination that straddles a 16 KiB
  shared-cache boundary** (M34). Unchanged.
* **The harness's `identical fault` label is exit-code-shaped** (M36). Unchanged; fired on no
  M38 row.
* **Review minors carried:** everything M37 carried (M34's `the_clamp_reaches_proc_info` host
  assumption; M35's `failwrite.rs:23` misnomer, the bare block in `forward_and_diff`,
  `util::record` not scrubbing `RETRACE_TRACE`; M36's `apple_walls_e2e` helper asserting on the
  record exit first and not printing the recorder's pid, the sweep's `rec_reason` cut at
  200/300 characters, a skipped corpus binary emitting no `ROW` line, `dladdr` seeing exported
  symbols only; M37's `dup2` landmarks quoted from run N, the evidence `repro:` tempdirs, the
  Task 3 red not kept as a log, the reader postdating its logs, `dup2_e2e`'s restating
  assertions). From this milestone: `retrace-trace`'s
  `a_trace_written_with_the_previous_magic_is_rejected_whole` still tests `RT\x00\x08` and calls
  it "immediately-prior (M16)" — stale since M24; `pipe_e2e`'s two `contains` asserts are
  subsumed by the rung helper's exact-stdout equality; `set_ret1`'s rustdoc says the two `Box_`
  methods call it where the two dispatch *arms* do; `(min as i32) < 0` is subsumed by `min >=
  DUP2_MAX_FD` and the doc reads as two checks; `dupfd_e2e`'s "`F_SETFD` forwarded verbatim"
  reads guest args (a fixture `F_GETFD` readback would observe the int reaching the kernel);
  `fdxlat`'s `fcntl_translates_only_its_descriptor` walks `fd_operands`, not `shape_of`;
  `dup_from` restates `alloc`'s free-slot search; `dupfd_dyn.c`'s unused `<errno.h>` and
  unchecked `write`; `fdxlat.rs`'s local `translate()` is a hand-maintained mirror of
  `Box_::translate_fds` — the duplication that let the `i64`/`i32` bug hide behind a green unit
  suite, now carried twice; the exec refusal's stderr label is `if num == SYS_EXECVE {"execve"}
  else {"posix_spawn"}` (a third refused number would print as `posix_spawn`); `exec_e2e` could
  also assert `ret1 == 0`; `exec_refusal_errno`'s "read a guest IPA as a host address" (the args
  are guest VAs); `cpython_e2e.rs`'s header nests two asides; the `RefuseMqRecv` mirror is a
  near-clone of `RefuseMqSend`'s (plan-mandated); the receive/crash landmark numbers and
  `selfpid` readings in the Task 5 evidence derive from uncommitted `.bin` traces (the `.err`
  files carry the divergence landmarks); the sweep's `csh`/`tcsh` reasons now quote one run's
  landmarks with the traced run's at ±1 stated beside them.
* **Everything M33 left owed and M34–M38 did not touch:** the per-argument canary fill and M32's
  Control 1 (still unexecuted, still inert); nested-pointer translation (`NestedSource` forwarded
  untranslated, `NestedDest` refused — and exec's refusal is precisely what makes adding it safe
  now); console `writev` mirroring; `__disable_threadsignal` (331); the corpus bias (every
  governed `mach_msg2` still init-time and shallow — the receive refusal included); the
  `unexercised` label enforced against a census dated 2026-09-12 (syscall numbers), 2026-09-13
  (M34's lengths, `dup2`'s `EXPECTED_DIFFS`) and now 2026-09-16 (`F_DUPFD` by fixture only) —
  snapshots; and the `kqueue` cross-version note.
* **Superseded, not owed — with this section as their forward pointer.** The "M38 does not
  exist" section's "nothing is owed to a milestone by that name" (R1); spec §3c's "both rows …
  stay PASS" (measured false twice); §3e/R3's `TIMED_OUT` default (measured, not a tie); §6's
  "PASS ≥ 45" (amended at Task 3, measured 44); §9's prediction (613/0/9 over 134); R6's "run N"
  label (regime I for the six, S for the sweep); §3a's `apply_and_return_pair` and §3d's "eighth
  `verify_thread` site" (corrected at plan time, recorded in §10 as history); the `MACH_RCV_REFUSAL`
  rustdoc's first draft (374 for four binaries) and the evidence README's first draft ("below
  `0x4000`") — both corrected in the Task 5 fix round; and the `Ret::FdPair`, fcntl-row, 59/244-row
  and `AT_FDCWD` rustdocs as M37 left them, all rewritten in place. M34's, M35's, M36's and
  M37's superseded lists stay as M37 left them.
