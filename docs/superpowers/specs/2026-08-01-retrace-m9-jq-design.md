# retrace M9 — rung 2 of the breadth ladder: `brew jq`, and the guest-side TLBI oracle

**Design spec — 2026-08-01.** The first post-M8-stack milestone. M8-stack closed with rung 1 green and
merged (`388f388`, gate 173/0/0, [M8-stack design](2026-07-31-retrace-m8-stack-design.md)): a real
full-`std` Rust binary records and replays bit-for-bit through real `/usr/lib/dyld` and reaches `main`.
That same push carried M6 and M7 to `origin`, which had been sitting at the M5 gate.

M6's spec named the arc: a **C → Rust → brew-jq breadth ladder**, whose purpose is not new debugger
capability but finding out what breaks when the guest is no longer a fixture. M9 is rung 2.

Every guest retrace has ever recorded — `hello_dyn`, `crashy`, `hello_rust` — links against exactly one
thing: `/usr/lib/libSystem.B.dylib`, which lives in the dyld shared cache and is reached through the
cache pager. **No guest has ever loaded a dylib that is a file on disk.** That is what rung 2 is really
about, and it fails on the fourth `mmap`.

## The problem, precisely

Two problems. The second is a capability the whole box has been built around not having.

**1. `brew jq` dies 105 traps in, before `main`, and the recorder aborts.** Not a guest crash — a
retrace `panic!`, exit 101. Details under "Verified facts".

**2. The abort is retrace refusing something the guest is entitled to ask for.** Real dyld loads a
non-cache dylib by reserving the image's whole span with one file-backed read-only `mmap`, then
`MAP_FIXED`-mapping each segment into that reservation with its own protections — including `__TEXT`
with `PROT_EXEC`. `Box_::place_fixed` refuses any exec FIXED map overlapping a live backing, because
`set_region_exec` promotes an entire 32 MiB L2 block from a data BLOCK to an L3 TABLE, and that is
sound only on a block the guest has never translated: **the VMM cannot issue a guest TLBI.**

M8-stack Task 5 built exactly the classification this needs — case 2, "FIXED fully contained in one
backing → reuse in place" — and the `exec` guard deliberately makes it unreachable. The guard is
correct as written. It is the *constraint behind it* that has to move.

This is not an unforeseen wall. `crates/retrace-box/src/lib.rs:1482` says so:

> A MAP_FIXED exec mmap onto a touched block would need a TLBI; dyld in private mode is not expected to
> do that — **if a run shows it, add a guest-side TLBI.**

A run has now shown it.

## Verified facts (measured on this host, HEAD `388f388`, 2026-08-01)

**The guest.** `brew install jq` → `/opt/homebrew/Cellar/jq/1.8.2/bin/jq` (symlinked from
`/opt/homebrew/bin/jq`):

- **54,272 bytes**, `Mach-O 64-bit executable arm64` — plain arm64, **not** arm64e. Rung 2 therefore
  does *not* drag in the deferred "arm64e main executables as full dynamic programs" gap.
- `LC_DYLD_CHAINED_FIXUPS` present, `LC_DYLD_INFO` **absent** — chained fixups, like `hello_dyn` and
  unlike `hello_rust`'s classic bind opcodes.
- `LC_LOAD_DYLINKER` = `/usr/lib/dyld`, `LC_MAIN`. No `LC_RPATH` on the executable itself.
- `otool -L` lists three dylibs, and **two of them are files on disk, not cache residents**:
  - `/opt/homebrew/Cellar/jq/1.8.2/lib/libjq.1.dylib` — on disk
  - `/opt/homebrew/opt/oniguruma/lib/libonig.5.dylib` — on disk, and the path is a **symlink** into
    `Cellar`
  - `/usr/lib/libSystem.B.dylib` — cache-resident, no file, already proven since M2
- Runs correctly outside retrace: `jq --version` → `jq-1.8.2`; `echo '{"a":[1,2,3]}' | jq -c '.a|length'`
  → `3`.

**Rejected subject: `/usr/bin/jq`.** The system jq is a universal `[x86_64 | arm64e]` binary that links
*only* `libSystem`. It would exercise no non-cache dylib loading at all, while simultaneously becoming
the first real dynamically-linked arm64e program — two walls at once, with an ambiguous failure mode.
The ladder means the Homebrew one.

**The failure.** `RETRACE_TRACE=1 cargo run -p retrace -- record-dyn /opt/homebrew/bin/jq`:

- Exit **101** — a retrace `panic!`, *not* a recorded guest crash.
- **105 dispatched traps** (exact `[trap]` count). No jq output of any kind appears; `main` is never
  reached.
- Panic: `crates/retrace-box/src/lib.rs:1354`:
  `FIXED exec map at 0xa000c8000..0xa0010c000 overlaps a live backing: exec promotion of an
  already-translated block would need a guest TLBI the VMM cannot issue`

**The four `mmap` traps, in order** (`num=197`, `pc=0x180119190` — one dyld call site):

| # | addr | len | prot | flags | fd |
| --- | --- | --- | --- | --- | --- |
| 1 | `0` | `0xd400` | `1` (R) | `0x40002` | 17 |
| 2 | `0` | `0x59080` | `1` (R) | `0x40002` | 17 |
| 3 | `0` | `0x59080` | `1` (R) | `0x40002` | 17 |
| 4 | **`0xa000c8000`** | `0x44000` | **`5` (R\|X)** | **`0x40012`** (+`MAP_FIXED`) | 17 |

Interleaved: `munmap` (`num=73`) of `0xa00000000` len `0xd400` after #1, and of `0xa00010000` len
`0x59080` after #2 — dyld mapping an image read-only to inspect it, unmapping, then reserving. #3 is the
surviving reservation; #4 drops `__TEXT` into it with `PROT_EXEC` and trips the guard.

**The mechanism already exists in halves.**

- `run_sign_stub` (`lib.rs`) runs hand-assembled instructions **on the guest vCPU**: full architectural
  state saved and restored, bounded run loop, fail-loud on an unexpected exit. It runs at **EL0** and
  terminates with `svc #0` because EL0 cannot `hvc`.
- `ATTR_CODE = A_COMMON | AP-RO-both | PXN` — RO, **EL0-exec, EL1 no-exec**. The sign stub's page.
- `ATTR_TRAMP = A_COMMON | AP-EL1-RO | UXN` — RO, **EL1-exec** (`PXN` clear). Already used for the
  trampoline.

`TLBI VMALLE1` is an EL1 instruction, so it cannot run on the sign stub's page. `ATTR_TRAMP` is exactly
the attribute it needs, and an EL1 stub can terminate with `hvc #0` directly to EL2 rather than the
sign stub's `svc`-through-trampoline path.

**argv rides free in the snapshot.** `build_start_stack` takes `argv0: &str` and hardcodes `argc=1`
with an empty envp. Replay opens with `Box_::restore(&mem, &regs)` from the leading `Snapshot`
(`retrace-core/src/lib.rs:502`) and **never calls `load_dynamic`** — so it never rebuilds the start
stack. Widening argv changes no trace record and needs no `TRACE_MAGIC` bump.

## The mechanism

### M9-spike — settle TLBI before building on it

`spikes/tlbi.c`, in the established spike posture (built and signed by hand, binary gitignored,
findings recorded in `spikes/README.md`). The skeleton already exists and should be copied rather than
rebuilt: five spikes stand up stage-1 page tables, and `dbgw.c` does it in 113 lines. The probe: map a
page as data, **make the guest translate it** by
reading it, flip its stage-1 leaf to `ATTR_CODE`, run the TLBI stub, and execute from the page. A
control run that omits the TLBI must fail. This answers two independent questions in one probe —
whether `TLBI` at guest EL1 is permitted under HVF at all, and whether it invalidates the entries
retrace hand-edits. Nothing downstream is built until it answers yes.

### M9-tlbi — `Box_::flush_guest_tlb()`

A hand-assembled EL1 stub — `TLBI VMALLE1; DSB ISH; ISB; hvc #0` — at a fixed reserved IPA beside
`SIGN_STUB_IPA`, on an `ATTR_TRAMP` page, lazily initialised like the signing scratch. Its runner
mirrors `run_sign_stub`: save the full architectural state, park the vCPU at EL1 with PC at the stub,
run with a small bound, restore, fail loud on any exit that is not the terminating `hvc`.

### M9-fixed — relax the guard

`place_fixed`'s `assert!(!exec || !self.overlaps_backing(..))` becomes a real path: take M8-stack's
containment case, `set_region_exec` the range, then `flush_guest_tlb()`. The `fixed_fits` assertion
above it is untouched — a wild address is still the guest's error, answered with `EINVAL`, per
M8-stack's fast-follow rule.

### M9-argv — argc/argv in the process-start stack

`build_start_stack(&Backing, argv0: &str, main_hdr)` widens to `argv: &[String]`, pushing
`argc = argv.len()`, each string pointer, then the NULL terminator. `apple[]` and the empty envp are
unchanged; `executable_path=` still derives from `argv[0]`. `record_dynamic`'s `argv0` widens with it.
The CLI gains a `--` separator:

```
retrace record-dyn /opt/homebrew/bin/jq -o t.bin -- -n '1+1'
```

## Determinism posture

Nothing here reaches the trace. The TLBI stub lives below it, inside `Box_`, on a path shared by
record and replay — **symmetry rule 2**: it fires identically on both sides and never surfaces to the
record/replay loop, so determinism is automatic rather than argued. `set_region_exec` is already
called from the replay side of the `mmap` dispatch (`retrace-core/src/lib.rs:172`), so the flush
follows it on both runs by construction.

argv is snapshot state, not trace state, and replay restores rather than rebuilds. No new
nondeterminism enters, and no existing posture changes.

## Correctness invariant

**A stage-1 attribute change that the guest may already have cached must be followed by a guest TLBI
before the guest is resumed.** Today the box discharges this by construction — exec regions are placed
in block-exclusive 32 MiB blocks so promotion always hits a pristine block. After M9 the invariant is
discharged *directly*, and the placement hack becomes an optimisation rather than a correctness
requirement.

## Scope

**In:** the spike; `flush_guest_tlb`; the `place_fixed` exec relaxation; argc/argv through
`build_start_stack`, `record_dynamic`, and the CLI; the freestanding TLBI regression fixture; the jq
e2e gate; whatever wall-chain work rung 2 turns out to need after the fourth `mmap` succeeds.

**Out:** stdin plumbing (`-n` needs none — deliberately, so rung 2 is not three capabilities at once);
threads; guest-raised signal delivery; `prot`/`PROT_NONE` enforcement; arm64e dynamic guests;
`guest_munmap`'s wholesale-drop defect; the `guest_mmap_replay` rename. All remain deferred, unchanged.

**Explicitly not promised:** that jq goes green. See "Exit criterion".

## Exit criterion

**The capability gate — must pass, un-`#[ignore]`d:** the freestanding fixture maps a page as data,
reads it, `MAP_FIXED`-exec-maps over it, and executes from it, recording and replaying bit-for-bit.
This pins the TLBI oracle independently of jq, so the capability stays proven even if the wall-chain
parks the milestone short.

**The rung-2 gate:** `record-dyn /opt/homebrew/bin/jq -o t.bin -- -n '1+1'` produces stdout `2\n` and
exit 0, then replays bit-for-bit and double-replays, asserted through M7's strict
`util::assert_rung_records_and_replays` — which checks exit status *and* stdout, so a crash trace
cannot satisfy it.

If a further wall stops jq, that gate is parked `#[ignore]`d **at the new wall, honestly documented**,
and a new Status section records it. `hello_dyn_e2e` and `hello_rust_e2e` stay green and un-ignored:
per CLAUDE.md, a new wall gets a NEW parked gate, never a regression of these.

## Testing

- **Spike** (manual, outside the gate): `spikes/tlbi.c` plus its control.
- **Unit, `retrace-box`:** `place_fixed` accepts an exec FIXED map contained in a live backing and
  leaves the surrounding backing intact; the stub page is `ATTR_TRAMP` and the stub's encoding
  round-trips.
- **Fixture, `retrace-guest`:** a freestanding `asm/tlbiexec.s` — map data, touch, `MAP_FIXED` exec
  over it, execute, exit with a known code.
- **E2E, `retrace`:** `tlbiexec_e2e` (the capability gate) and `jq_e2e` (the rung-2 gate).
- **Regression:** the full gate stays at ≥173 passed / 0 failed, with `hello_dyn_e2e` and
  `hello_rust_e2e` un-ignored.

## Risk register

- **R1 — `TLBI` at guest EL1 is unavailable or ineffective under HVF.** Killed early by design: the
  spike runs before anything depends on it. Fallback is to pre-promote file-backed reservations to L3
  at map time (sound only if the guest never translates the reservation before the FIXED exec map —
  measurable, and unsound for the header page if it does), which narrows M9 to jq's shape and leaves
  the anon-`PROT_EXEC`/JIT gap open.
- **R2 — `TLBI VMALLE1` is coarse and flushes everything.** A chatty dyld could make that expensive.
  M8-stack's lesson is that per-syscall cost scales and can blow the gate timeout, so this gets
  measured, not assumed. Fallback: narrow to `TLBI VAE1` per page.
- **R3 — the wall-chain past the fourth `mmap` is of unknown depth.** Chained-fixup binding against two
  non-cache dylibs, rpath and symlink resolution through `Cellar`, and jq's own init are all
  unexercised surface. M2's ten sub-milestones are the precedent. Mitigated by structure, not by hope:
  the capability gate is separable from the rung-2 gate, so the milestone delivers value even parked.
- **R4 — EL1 stub state handling.** Parking the vCPU at EL1 and restoring cleanly is new; the sign
  stub's save/restore list was tuned for EL0 and may be incomplete for EL1 (SPSR/ELR in particular).
  Mitigated by reusing `run_sign_stub`'s discipline and asserting the restored state.

## Components

- `spikes/tlbi.c`, `spikes/README.md` — the probe and its findings.
- `crates/retrace-box/src/lib.rs` — the TLBI stub constant + IPA, `flush_guest_tlb`, its runner, the
  `place_fixed` relaxation, `build_start_stack`'s argv widening.
- `crates/retrace-core/src/lib.rs` — `record_dynamic`'s argv widening.
- `crates/retrace/src/main.rs` — `--` separator in `record-dyn`.
- `crates/retrace-guest/asm/tlbiexec.s`, `build.rs`, `src/lib.rs` — the regression fixture.
- `crates/retrace/tests/tlbiexec_e2e.rs`, `crates/retrace/tests/jq_e2e.rs` — the two gates.
- `README.md` — a new Status section.

## Open questions for implementation planning

1. **Does the jq gate need jq installed?** `brew jq` is not a repo artifact, so `jq_e2e` must skip
   cleanly (not fail) when `/opt/homebrew/bin/jq` is absent, or the gate breaks on any machine without
   it. Skipping-when-absent is a hole in the honest-gate posture; a guard that *fails loud* on this
   host and skips elsewhere needs a decision.
2. **Which TLBI variant, and how broad?** `VMALLE1` vs `VAE1` per page — decided by R2's measurement,
   but the stub's shape depends on the answer.
3. **Is `TLBI VMALLE1` enough, or is `VMALLE1IS` required?** Single-vCPU means no other PE to
   invalidate, so the non-IS form should suffice; the spike should confirm rather than assume.
4. **Does the reservation's own backing need its stage-2 flags widened?** `__TEXT` becomes RO+exec at
   stage 1, but the reservation was mapped for data at stage 2. `set_region_exec` already handles its
   own `hv_vm_map`; whether the containment path needs the same treatment is unverified.
5. **How many exec FIXED maps does a full jq load produce?** Two dylibs × segments each. Bears on R2's
   cost and on whether the block-exclusive placement hack can actually be retired or merely demoted.
