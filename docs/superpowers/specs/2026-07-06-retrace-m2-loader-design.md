# retrace M2 — The loader (MMU-on, dyld, PAC, shared cache)

**Design spec — 2026-07-06**

> **Outcome (2026-07-07):** the loader (MMU-on W^X, PAC, real Mach-O + dyld) is built and merged on
> `m2-loader`. The shared-cache piece proved harder than "let dyld map it" — the arm64e cache is
> host-process-bound (per-process PAC keys + dirtied `__DATA`) — and was split into its own effort,
> **M2-cache** (`2026-07-07-retrace-m2-cache-resign-design.md`), which re-signs the cache with the
> guest's keys and is validated end-to-end. The original exit gate (a dynamically-linked binary
> records/replays) is **deferred**: past the cache, real dyld hits the libSystem mach-IPC runtime
> (`mach_msg2` RPCs + absent system daemons) — the next milestone.

## What this is

M0 proved the box & trace spine; M1 proved a general memory-diff syscall recorder — both on
static, freestanding, MMU-off guests whose only inputs were a handful of hand-loaded
syscalls. M2 is **the loader**: the machinery to get a *real, normally-compiled,
dynamically-linked* macOS binary running inside the box, so the recorder built in M1 can be
pointed at real programs.

The strategy is deliberately minimal. retrace-box emulates only the **kernel's process-setup
contract** — turn the MMU on, map the main Mach-O and `/usr/lib/dyld` into anonymous guest
memory, engage pointer authentication, build the initial stack, and jump to dyld's entry
point. From there **real dyld runs inside the guest** and maps the dyld shared cache itself,
using ordinary `open`/`mmap`/`pread`/`fstat` syscalls that the M1 recorder already traps and
forwards. **We write no shared-cache parser.** Apple's cache-format knowledge stays Apple's
problem across OS updates — directly mitigating the master spec's risk #1 (DSC loading
fidelity across point releases).

The memory-diff engine, trace format, divergence oracle, and seeded swarm from M1 carry
forward unchanged. What is new is entirely in the box (MMU, PAC, Mach-O loading, file-backed
mmap) plus the recorder-fidelity debts that only a real program makes reachable.

## Verification spike (done — task 0)

Per the M1→M2 boundary note, M2 got an HVF-style verification spike before this design was
finalized (`spikes/m2spike.c`, `spikes/dscprobe.c`), run on the target: Apple Silicon,
macOS 26.4.1 (build 25E253), non-root, SIP on. Verified:

- **MMU-on identity paging.** A guest-built stage-1 table (16 KiB granule, `T0SZ=28`, start
  level 2, `MAIR` attr0 = Normal WBWA) makes unaligned EL0 loads/stores work in both
  page-mapped and 32 MiB-block-mapped regions. Negative control: MMU-off, the same access
  faults (`EC=0x24` data abort, alignment DFSC `0x21`), proving MMU-on is load-bearing for
  real code, not cosmetic.
- **PAC in the guest.** With the five `APxKEY*_EL1` registers set via
  `hv_vcpu_set_sys_reg` and `SCTLR_EL1.EnIA=1`, an EL0 `pacia`/`autia` round-trip
  authenticates; the signed pointer carries PAC bits. With `EnIA=0`, `pacia` is identity —
  so PAC can be disabled deterministically if needed.
- **Shared-cache reachability.** The arm64e cache is standard `dyld_v1  arm64e` format at
  `/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_arm64e`
  (15 subcache files). The guest read its header through MMU translation; bytes matched host.
- **`DYLD_SHARED_REGION=private`** makes libSystem map **outside** the kernel-managed shared
  region (`shared_region_check_np` returns `-1`; `&printf` falls outside the region base),
  confirming dyld will map the cache itself via ordinary syscalls rather than joining the
  kernel region — the premise of the whole approach.

**The spike also revised the design (the point of doing it):** an early version mapped the
cache file into the guest with a **file-backed** `hv_vm_map` to avoid copying it. On
macOS 26 this is **fatal** — SPTM (Secure Page Table Monitor) hard-panics the machine with
`VIOLATION_ILLEGAL_MAPPING_TYPE`, an unrecoverable kernel reset. **Hard rule adopted below:
all guest memory is anonymous; file bytes are staged into anon pages, never mapped directly.**

## Scope

**In scope:**
- **MMU-on box.** Replace M0/M1's MMU-off 1:1 identity map with guest-built stage-1 page
  tables (the spike-verified layout). Guest VA still equals IPA, so forwarded-syscall
  pointers cross the boundary unchanged — the AppBox simplification is preserved.
- **PAC setup.** Set the five pointer-authentication key registers to fixed constants and
  enable authentication in `SCTLR_EL1`. Fixed keys make record and replay bit-identical.
- **Real Mach-O loader.** Load a dynamically-linked arm64e executable: parse
  `LC_SEGMENT_64`, `LC_LOAD_DYLINKER`, `LC_MAIN`/`LC_UNIXTHREAD`; map `/usr/lib/dyld`
  (arm64e slice of the universal binary); build the initial stack (`argc`, `argv`, `envp`
  with `DYLD_SHARED_REGION=private`, and the `apple[]` array including the executable path).
- **File-backed `mmap` special case.** Extend M1's `guest_mmap` to service file-backed
  mappings — dyld maps the cache and dylibs with `mmap(fd, …)`. Per the SPTM rule, a
  file-backed mmap is serviced by allocating **anonymous** guest pages and `pread`-ing the
  file bytes into them, tracking the backing exactly as M1 tracks anonymous mmap.
- **The four carried-forward recorder debts** (see below) — now reachable, so now paid.

**Out of scope (M3 and later):**
- Instruction-exact positioning, signals, threads, the debugger seam (M3–M6).
- Multithreaded dyld/program startup (the M2 target program is single-threaded).
- Cross-OS / cross-cache-version replay portability beyond pinning the host build in the
  trace (master-spec risk #4; unchanged from M1).
- Any shared-cache format parsing on our side.

## Exit criterion

**Record + replay a normally-compiled, dynamically-linked C program that links `libSystem`
through the shared cache, with zero divergence — including the seeded fault-injection swarm
re-pointed at it.** The program is an ordinary `cc hello.c`-class binary (`main` calls into
libSystem, e.g. `write`/`puts`, and exits). It must:

1. Load and run to a correct exit in the box (dyld maps the cache and resolves symbols).
2. Replay byte-for-byte: the exit-code oracle, every syscall landmark, and the final
   full-memory comparison all match, over N fresh seeds, with the recorded cache/dylib files
   served from the trace on replay.

This is the oracle-gated exit chosen during design — not merely "load and run" — so that the
syscall-surface explosion that comes with real dyld is caught at a named landmark, not
deferred into M3.

## The mechanism

### MMU-on and PAC (box setup)

At load, retrace-box builds a stage-1 translation table in anonymous guest memory (identity:
guest VA == IPA, so the M1 recorder's pointer arithmetic is unchanged), sets `TTBR0_EL1`,
`TCR_EL1`, `MAIR_EL1` to the spike-verified values, sets the PAC keys to fixed constants,
and starts the guest with `SCTLR_EL1.{M,C,I,EnIA…}` on. `restore()` (replay) re-establishes
the identical MMU + PAC state so replayed memory translation and pointer signing match the
recording exactly.

### Loading a dynamic executable

retrace-box maps the executable's segments and `/usr/lib/dyld` (arm64e) into anon guest
pages, builds the stack, and sets PC to dyld's entry. Everything after is dyld executing
guest-side: it opens and maps the shared cache, applies rebases/binds (pure in-guest
computation — free, no traps), resolves the program's imports, and calls `main`. Each syscall
dyld makes traps to the VMM and is recorded by the unchanged M1 engine.

### File-backed mmap

When the guest calls `mmap` with a real fd (not `MAP_ANON`), the box: allocates anonymous
guest pages at a fresh deterministic IPA (as M1's `guest_mmap` already does), `pread`s the
requested file extent into them, maps them, and returns the IPA. On replay the same call
sequence yields the same IPAs and the file bytes are served from the recorded writes — so a
deleted or changed cache still replays identically. `MAP_ANON` mmap keeps M1's behavior.

## Carried-forward recorder debts (now paid)

These were documented in the M1 plan as safe to defer while guests were trusted, static, and
MMU-off. A real dyld run makes each reachable, so M2 pays them; each is a gate item because
each will otherwise surface as a divergence or a fault:

1. **`forward_and_diff` write-extent clamp (memory safety).** Guest-controlled size/count
   args are currently forwarded unclamped to the host syscall; a `read`/`pread` with a count
   larger than the destination backing would have the host kernel write past it. Clamp/assert
   that the write extent fits the backing before forwarding. Mandatory: dyld issues large,
   variable-size reads.
2. **Honoring `munmap`/`mprotect`.** Recorded as no-ops in M1 (no address reuse under a
   trusted static guest). Real dyld unmaps and reprotects; address reuse becomes real, so
   these must actually update the box's backing set.
3. **Error-ABI fidelity (carry flag).** M1 assumed syscalls succeed. dyld deliberately makes
   failing probe syscalls (`stat`/`open` of absent paths, `fcntl` feature probes); the arm64
   BSD ABI signals error via the carry flag with the errno in `x0`. Record and replay the
   carry flag + errno, not just `x0`.
4. **Raw-`svc` forwarding for 64-bit `x0`.** M1 forwards via `libc::syscall`, whose C
   signature narrows the return toward 32 bits. `mmap` returns a full 64-bit address; forward
   via a raw `svc` (or an equivalent that preserves the full 64-bit `x0` and the carry flag)
   so returns and errors are captured at full fidelity.

## The divergence oracle (unchanged from M1)

M1's oracle carries forward verbatim: per-syscall `(num, args)` equality at every landmark,
the exit-code oracle (compare live exit vs. recorded before the final check), and the final
full-guest-memory comparison — all failing loudly at the first diverging byte with a
one-command seed repro. M2 adds no new oracle; it re-points the existing one (and the seeded
swarm) at the dynamic guest. The larger, nondeterminism-prone syscall surface is exactly why
the exit is oracle-gated.

## Components (building on M1's crates)

- **`retrace-box`** — the milestone's center of gravity: MMU/page-table construction, PAC
  key setup, real Mach-O + dyld loading, stack building, file-backed `guest_mmap`, and the
  four recorder-fidelity fixes. Grows most; watch its size and split loader vs. recorder
  responsibilities if it gets unwieldy.
- **`retrace-guest`** — add a dynamically-linked arm64e test executable and its build step
  (normal `cc`, not `-nostdlib -static`), plus a fixture recording the host chip + macOS
  build the trace was captured on.
- **`retrace-arch`** — any additional Mach-O load-command / ESR constants needed.
- **`retrace-trace`, `retrace-core`, `retrace-sim`, `retrace`** — unchanged in mechanism;
  the swarm and e2e tests re-point at the dynamic guest.

## Risk register

1. **DSC loading brittleness across macOS point releases** (master risk #1). *Mitigation:*
   by construction we run *real dyld*, so we never parse the cache; the OS's own loader
   tracks its own format. We pin the host macOS build in the trace and fail loudly if replay
   is attempted on a mismatched build. This is the single biggest reason for the
   let-dyld-do-it approach.
2. **A dyld syscall/mach-trap the diff engine can't capture** (opaque kernel state — some
   mach-port right or shared-region call). *Mitigation:* the oracle catches it at a named
   landmark immediately; special-case as found. Main schedule risk and the project's long
   tail (master risk #2).
3. **PAC determinism.** If any pointer signing depends on state we don't fix (keys, `SCTLR`
   bits, address layout), replay diverges. *Mitigation:* fixed keys + fixed MMU layout +
   identity VA==IPA; the oracle flags any residual nondeterminism. PAC can be disabled
   (`EnIA=0`) as a fallback, verified in the spike.
4. **`CNTVCT_EL0`/commpage reads during dyld startup** (master risk #3). *Mitigation:* the
   oracle flags the divergence; widen the read-site handling. Deferred instrumentation lands
   with M5, but M2's oracle will surface it if dyld trips it early.
5. **retrace-box outgrowing one file.** *Mitigation:* split loader vs. recorder modules
   within the crate as it grows; keep each unit independently testable.

## Non-goals / explicitly deferred

- Shared-cache format parsing; a hand-written DSC loader (we rely on real dyld — a fallback
  was considered and rejected as unnecessary given the spike results).
- Multi-threaded startup, signals, positioning, the debugger seam (M3+).
- Recording untrusted/adversarial binaries at full hardening — M2's target is a benign,
  developer-compiled program; the write-extent clamp is the one memory-safety fix pulled
  forward because it is trivially reachable.
- Cross-machine / cross-OS trace portability (same-chip-class, pinned-build; unchanged).

## Dependencies

- Hypervisor.framework (the substrate) — MMU, PAC key, and sysreg APIs all confirmed present
  and functional in the spike.
- The in-tree `hv-sys` binding — extend with any missing PAC/MMU sysreg constants.
- `/usr/lib/dyld` and the dyld shared cache on the recording host (system files; read-only).
- Nothing new inside the deterministic boundary (`retrace-core` stays IO/threading/time-free).

## Open questions for implementation planning

- **Stack/`apple[]` fidelity:** exactly which `apple[]` entries and env does dyld require to
  start (executable path, `dyld_file=`, `executable_file=`, entropy)? Determine empirically
  against real dyld; the oracle will flag anything missing.
- **PAC on vs. off for M2:** start with PAC enabled (real arm64e), or disable (`EnIA=0`) to
  shrink the first dynamic bring-up and enable it in a later task? Leaning enabled, since the
  spike shows it works and disabling risks masking a real determinism bug.
- **File-backed mmap window vs. whole-extent:** `pread` the whole requested extent eagerly,
  or fault it in lazily? Leaning eager (simpler, deterministic); revisit if cache-sized
  mappings make traces unwieldy (compression is deferred post-v1 per the master spec).
- **Where the raw-`svc` forwarder lives:** a small asm shim in `retrace-box` vs. extending
  `hv-sys`. Leaning a local shim to keep the syscall-forwarding path under the box's control.

## Self-review notes (author)

- **Spec coverage vs. master spec:** M2 here matches the master spec's "M2 — the loader"
  intent (MMU-on, standalone dyld + PAC, DSC) and the M1→M2 boundary note; the boundary
  note's four carried-forward debts are all captured as gate items.
- **Known deviations:** the master spec's original wording implied we might drive the DSC
  loader "through supported APIs"; this spec commits firmly to *not* parsing the cache and
  letting real dyld map it, justified by the spike. The `DYLD_SHARED_REGION=private` +
  file-backed-mmap-via-anon-staging path is the concrete realization.
- **Biggest risk to the plan:** #2 (an uncapturable dyld syscall) is unbounded a priori;
  the oracle bounds it to "fails loudly at a named landmark," but the number of special cases
  dyld startup needs is not known until we run it. The plan should front-load a bring-up task
  that just gets dyld to its first `main` call under the oracle, then iterate special cases.
- **Safety:** the SPTM finding is load-bearing and non-obvious; it is recorded in the spike
  source and in project memory so it is not rediscovered the hard way.
