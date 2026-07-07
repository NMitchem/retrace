# retrace M1 — General memory-diff syscall recorder

**Design spec — 2026-07-05**

## What this is

M0 proved the box & trace spine on a static freestanding guest whose only syscalls were
`write` and `exit`, handled by hand. M1 replaces those hand-written handlers with a
**general recorder** that captures *any* syscall's effects by **memory-diff +
pointer-chasing** — snapshot the memory the kernel might write before the call, diff it
after, log the delta — so we never model per-syscall semantics. It is proven on a static,
MMU-off freestanding guest doing real file I/O.

This is a deliberate split of the design spec's original M1 (which bundled the recording
engine with the dyld-shared-cache loader). Those are two orthogonal risks — the *engine*
records any program's syscalls; the *loader* is separate machinery to get real binaries
running. M1 is the engine; the loader is M2. See [Milestone reshuffle](#milestone-reshuffle).

## Scope

**In scope:**
- A general syscall record/replay handler using memory-diff + pointer-chasing for
  data-carrying syscalls (`open`/`openat`, `read`/`pread`, `write`, `close`, `lseek`,
  `fstat`/`fstatat`, and the like) — driven by the *shape* of the arguments, not a
  per-syscall model.
- The map-changing special cases: `mmap`, `munmap`, `mprotect` (they change the memory map,
  not just its contents).
- A stronger divergence oracle: a final full-guest-memory comparison between record and
  replay, in addition to M0's per-syscall `(num, args)` equality.
- A dedicated M1 file-I/O guest (freestanding `-static -nostdlib`, raw `svc`).
- The box↔core encapsulation refactor deferred from M0's final review.

**Out of scope (M2 and later):**
- The dyld-shared-cache loader, standalone `dyld` startup, pointer authentication (PAC),
  and guest stage-1 page tables (MMU-on). The M1 guest stays MMU-off (guest VA == IPA), as
  in M0.
- Instruction-exact positioning, signals, threads, the debugger seam (M3–M5).
- Byte-level divergence at *intermediate* landmarks (M1 adds it only at the final landmark;
  per-tick landmark comparison lands with M3's positioning work).

## Exit criterion

**Record + replay a static file-I/O program that replays byte-for-byte identically after
its input file is deleted.** The guest does `open` → `fstat` → `read` → `write(stdout)` →
`close`. On replay the read data is served from the trace (the file is gone), and the final
full-memory comparison plus every syscall landmark match with zero divergence — proven over
N fresh fault-injection seeds.

## The mechanism

### Record

On each syscall trap (an `HVC` exit whose `ESR_EL1` decodes to `Ec::Svc`):

1. **Pointer-chase the arguments.** Scan `x0..x7`. For each value that falls inside a mapped
   guest region, treat it as a candidate pointer and snapshot a **window** around it (a
   bounded region within its enclosing backing). One level of chasing covers `read`'s `buf`,
   `fstat`'s `statbuf`, etc. The window is bounded (not the whole backing) so the technique
   scales toward M2+; under-coverage is caught by the divergence oracle and the window
   widened — the sim-first discipline.
2. **Forward the real syscall** to the host kernel (during record only). Because the 1:1
   mapping makes guest pointers == host pointers, the kernel writes directly into the guest
   backing.
3. **Diff** each snapshotted window; the changed bytes are the kernel's writes. Log
   `Event::Syscall { num, args, ret, writes: Vec<Region> }`.
4. **Resume** the guest via `set_x0_and_return(ret)`.

### Replay

On each syscall trap:

1. **Verify** the live `(num, args)` equal the recorded event; a mismatch is a named
   `Divergence`.
2. **Apply** the recorded `writes` to guest memory and feed the recorded `ret` — **never
   execute the syscall.** This is the negative-space invariant that makes the recording
   independent of the host (deleted files, changed cwd, etc. do not affect replay), enforced
   by construction: `retrace-core` has no path to issue a host syscall (no `hv_sys`/`libc`
   dependency). `Box_::guest_mmap` does legitimately call `libc::mmap` during replay, but
   that allocates host-side backing at a deterministic address — it is not forwarding the
   guest's syscall for its effect — so it does not violate the invariant.
3. **Resume** the guest.

### Map-changing special cases

`mmap`/`munmap`/`mprotect` are handled explicitly because they change the guest's set of
mapped regions:

- **Record `mmap`:** forward the real `mmap`; map the returned host region into the guest
  1:1 as a new backing (tracked via `Box_::map_region`); log the syscall plus the new
  region's `(ipa, len, prot)` so replay can recreate the map. Anonymous and, later,
  file-backed mappings both go through this path.
- **Replay `mmap`:** recreate the identical guest mapping (same IPA, same length) from the
  log **without** a host `mmap`, and feed the recorded return address. Because M1 is MMU-off
  and 1:1, the recorded IPA is the address the guest expects.
- **`munmap`/`mprotect`:** update the guest map / stage-2 permissions to match the recording;
  no host call on replay.

A static guest that `mmap`s an anonymous region and writes to it exercises this path; M2's
`dyld` will depend on it immediately.

## The divergence oracle (stronger than M0)

Record/replay's built-in oracle: replay must reproduce the recording bit-for-bit. M1
strengthens it:

- **Per-syscall landmark** (from M0): live `(num, args)` must equal the recorded event.
- **Final full-memory landmark (new):** at the guest's `exit`, replay's entire guest memory
  (every backing) must equal record's, byte-for-byte. The checker halts at the first
  diverging `(ipa, byte)` and prints it with the seed and a one-command repro. This is the
  real oracle for the memory-diff engine — an under-captured kernel write during record
  makes replay's memory diverge here (or earlier, as a syscall-arg mismatch, if it changes
  control flow). This directly implements the byte-level comparison the M0 final review
  flagged as missing (Important #2), at the final landmark.
- **Negative-space invariants:** no host syscall executes during replay — enforced by
  construction (`retrace-core` has no `hv_sys`/`libc` dependency; `Box_::guest_mmap`'s
  `libc::mmap` call during replay allocates host-side backing at a deterministic address
  rather than forwarding the guest's syscall, so it does not violate this). Release-on
  assertions guard the rest: the recorded `writes` never fall outside a mapped region on
  apply; the map-changing special cases keep the guest map and the trace in agreement.

## Components (building on M0's crates)

1. **Task 1 — box↔core encapsulation** (deferred M0 final-review Important #1). Move
   `snapshot_of` into `retrace-box` as `Box_::snapshot()`; expose a position accessor
   (`Box_::position()` → `ELR_EL1`); make `vcpu`/`backings` private; drop the `hv_sys`
   dependency and `reg`/`sysreg` imports from `retrace-core`. One caller today, so cheapest
   now. No behavior change; the M0 tests must stay green.
2. **retrace-box — runtime backing management.** `map_region(ipa, len, prot)`,
   `unmap_region(ipa, len)`, `protect_region(ipa, len, prot)`, `snapshot_region(ipa, len) ->
   Vec<u8>`, `apply_region(ipa, &[u8])`, `writable_regions() -> &[Backing]`, and a
   pointer-classifier `region_of(ipa) -> Option<&Backing>`. Assert IPA alignment and
   non-overlap on every map (M0 final-review minor, now load-bearing).
3. **retrace-trace — extend `Event::Syscall`** with `writes: Vec<Region>`. M1 owns the trace
   format; bump a format version byte in the trace header so a stale trace fails loudly
   rather than mis-parsing.
4. **retrace-core — the memory-diff handler.** The general record path (pointer-chase →
   snapshot → forward → diff → log), the general replay path (verify → apply → feed), the
   `mmap`/`munmap`/`mprotect` special cases, and the final-memory divergence check. Replaces
   M0's `write`/`exit` arms; `exit` and `write` still work (as ordinary cases of the general
   engine plus stdout capture for output comparison).
5. **retrace-guest — the M1 file-I/O guest.** A freestanding `-static -nostdlib` guest
   issuing raw `svc` syscalls for `open`/`fstat`/`read`/`write`/`close` on a fixture file
   the build script creates, plus a second guest variant that `mmap`s an anonymous region,
   writes a pattern, and reads it back (to exercise the map-changing path).
6. **retrace-sim — the seeded swarm, re-pointed.** N seeds of record→(trace-IO
   fault)→replay of the file-I/O guest, asserting either zero divergence (final memory +
   every landmark match) or a clean named failure (exit 3, stderr names the divergence).
   Any bug seed is pinned in `REGRESSION_SEEDS.md` forever.

## Milestone reshuffle

Splitting the original M1 shifts the roadmap by one:

| New | Was | Content |
|-----|-----|---------|
| M1  | M1 (first half) | General memory-diff syscall recorder (this spec) |
| M2  | M1 (second half) | The loader: MMU-on page tables, standalone dyld + PAC, dyld-shared-cache loader |
| M3  | M2 | Instruction-exact positioning |
| M4  | M3 | Async signal + thread-switch replay |
| M5  | M4 | Debugger seam (LLDB-remote reverse ops) |
| M6  | M5 | Real interpreter (python3) |

The design spec's dependency ordering is otherwise intact. This split is recorded here and
should be reflected in the design spec's milestone list.

## Risk register

1. **Pointer-chase under-coverage** — the snapshot window misses a kernel write outside it.
   *Mitigation:* the final-memory oracle catches it at a named byte with a repro seed; widen
   the window (or add a one-level struct chase) as found. This is M1's main engine risk and
   is caught loudly, never silently.
2. **False-positive pointers** — an integer argument that happens to look like a mapped
   address triggers a needless snapshot. *Mitigation:* harmless (an extra snapshot/diff that
   finds no change); costs a little time, never correctness.
3. **`mmap` address determinism** — replay must recreate the exact guest mapping the record
   produced. *Mitigation:* M1 is MMU-off and 1:1, so the recorded IPA *is* the guest address;
   replay maps at that IPA without a host call. (Address-space-layout nondeterminism becomes
   real only under MMU-on in M2.)
4. **Trace format drift** — the new `writes` field changes the on-disk format. *Mitigation:*
   a version byte in the trace header; a mismatched version fails loudly.

## Non-goals / explicitly deferred

- MMU-on, dyld, PAC, the DSC loader (all M2 — each gets a verification spike before M2's
  design is finalized, mirroring M0's HVF pass).
- Per-tick (intermediate) byte-level landmark comparison (M3, with positioning).
- Multi-threaded guests, signals (M4).

## Dependencies

No new external dependencies. The engine stays inside the deterministic boundary; the only
host IO remains the trace file and (during record only) the forwarded syscalls. `serde`/
`bincode` continue to serialize the trace.

## Open questions for implementation planning

- Snapshot-window size/policy: fixed page-multiple around each pointer vs. the enclosing
  backing capped at a maximum. Start with the enclosing backing capped at, say, 64 KiB
  (cheap for M1's small guest, robust), and record the cap so the policy is explicit;
  revisit when M2 introduces large mappings.
- Whether the M1 guest exercises `mmap` in the same binary as file I/O or as a separate
  guest variant (leaning separate, so each landmark set is easy to reason about).
- One-level pointer chasing (follow a pointer found inside a snapshotted struct) — include
  in M1 for `fstat`-class writes, or defer until a real syscall needs it. (Leaning: only
  scan `x0..x7` in M1; add struct-chasing when a divergence seed demands it.)
