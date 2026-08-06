# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`retrace` is a record/replay reverse debugger for Apple Silicon. It runs a guest binary inside a
single-vCPU Hypervisor.framework VM, records every syscall/trap plus the kernel's memory writes, and
replays the run **bit-for-bit** from a snapshot — never re-executing a syscall, only re-applying
recorded effects. The determinism oracle proves zero divergence between the two runs.

Requires **macOS 26.x on Apple Silicon**. Runs non-root; SIP may stay enabled. Every binary that
touches `hv_*` needs the `com.apple.security.hypervisor` entitlement (ad-hoc signable — see
Codesigning below).

The full design and milestone history live in `docs/superpowers/specs/` and the README's "Status"
sections — read the **latest Status section** before starting work; it is the authoritative, honest
log of what runs today and what the next wall is.

## Commands

```sh
just gate          # THE exit gate: cargo test --workspace + clippy -D warnings. `just m0`/`just m1` are aliases.
```

- **`--test-threads=1` is mandatory.** HVF allows only one VM per process, so in-process VM tests
  must run serially. `just gate` sets it; a bare `cargo test` will flake with `HV_BUSY`.
- Single test: `cargo test -p <crate> <name> -- --test-threads=1`
  (e.g. `cargo test -p retrace-box --test pac -- --test-threads=1`).
- Nothing in the gate is `#[ignore]`d as of M9-jq (see "Honest-gate discipline" below). The
  headline e2e gates run with the rest: `cargo test -p retrace --test hello_rust_e2e -- --test-threads=1`
  (rung 1), `--test jq_e2e` (rung 2; skips loudly without `/opt/homebrew/bin/jq`), and
  `--test panic_e2e` (M11 — a Rust guest that aborts).
- CLI: `cargo run -p retrace -- record <macho> -o t.bin`, `... record-dyn <exe> -o t.bin` (runs the
  exe through real `/usr/lib/dyld`; append `-- <guest args…>` to pass the guest an argv),
  `... replay t.bin`.
- `RETRACE_TRACE=1` on a `record`/`record-dyn` run logs every dispatched trap (and decodes
  `mach_msg2` sends) — the first tool to reach for on a bring-up failure. **Record-only:** `ReplaySession`
  carries no trace instrumentation, so no `[trap]`/`[mach_msg2]`/`[fault]` line is ever printed on replay.

The toolchain is pinned (`rust-toolchain.toml`: 1.95.0, target `aarch64-apple-darwin`). `clippy.toml`
bans `Instant::now`/`SystemTime::now`/`std::thread` — see Determinism below; those denials are load-bearing, not style.

### Codesigning

`.cargo/config.toml` sets a cargo `runner` (`tools/codesign-run.sh`) that ad-hoc-signs the binary
cargo invokes with `retrace.entitlements` before running it. **But** a test that spawns a *separate*
binary itself (via `CARGO_BIN_EXE_retrace`) bypasses that runner, so it must codesign that binary by
hand first — see `crates/retrace/tests/util/mod.rs::bin()`. Copy that pattern for any new test that
spawns the CLI.

### Spikes

`spikes/*.c` are throwaway probes that empirically verified load-bearing HVF/SPTM/PAC claims on the
real OS before they were committed to the architecture. They are built and signed manually (binaries
are gitignored); build/run recipes and findings are in `spikes/README.md`.

## Architecture

### Crate graph (dependency order)

- **`hv-sys`** — bindgen FFI over Hypervisor.framework. Thin safe wrappers: `Vm`, `Vcpu`, `reg`,
  `sysreg`, `MemFlags`. `build.rs` runs bindgen against the macOS SDK.
- **`retrace-arch`** — zero-dependency arch facts: syscall numbers, `ec_of` (ESR exception-class
  decode), `decode_aut_rd` (PAC AUT* instruction decode), Mach-O/PSTATE constants.
- **`retrace-trace`** — the on-disk trace format: the `Event` enum (`Snapshot`/`Syscall`/`Exit`),
  `Writer`/`Reader` with a magic+version header and per-record CRC32. **Changing `Event`'s shape is a
  format break — bump `TRACE_MAGIC`.** `open_checked` drops a torn/corrupt tail rather than panicking.
- **`retrace-guest`** — the Mach-O loader (`parse_macho`, `slice_arm64e`) **and** the guest test
  programs. `asm/*.s` (freestanding, `-nostdlib -static`) and `c/hello_dyn.c` are compiled by
  `build.rs` into `OUT_DIR`; path constants (`HELLO`, `HELLO_DYN`, …) point at them.
- **`retrace-box`** — the core. `Box_` is the VM: it builds guest memory, the W^X stage-1 page
  tables, and PAC state; runs the vCPU trap loop (`run() -> Stop`); forwards syscalls with
  memory-diff (`forward_and_diff`); applies recorded writes on replay (`apply_and_return`); hosts the
  shared-cache demand-pager + re-signer (`cache.rs`) and the in-guest PAC signing oracle. By far the
  largest, densest crate — start here.
- **`retrace-core`** — the record/replay orchestration: `record_box`/`replay` dispatch each `Stop`
  from the box, plus `machmsg.rs`, the pure `mach_msg2`/MIG codec + router.
- **`retrace-sim`** — deterministic `Rng` + fault injection for the seeded swarm (no deps).
- **`retrace`** — the CLI binary and all end-to-end integration tests.

### The record/replay model (retrace-core)

Record snapshots initial state, runs the guest, and on each trap forwards the real syscall to the
host kernel, diffs guest memory to capture what the kernel wrote, and appends an `Event`; on exit it
appends a final full-memory snapshot. Replay restores from the snapshot, runs the guest again, and
**never executes a syscall** — it verifies each trap's `(num, args)` against the recording (the
divergence oracle), applies the recorded writes, feeds the recorded return, and at exit does a
full-memory comparison.

**Determinism is the whole game.** Nothing nondeterministic may enter the trace. Anything that would
(shared-cache page contents, timing, PAC signatures) is instead *regenerated identically* on both
sides: cache pages are a pure function of (file bytes, fixed slide, fixed keys); the timebase is a
synthetic monotonic counter; PAC keys are fixed constants. That is why `clippy.toml` bans wall-clock
and threads.

**Two symmetry rules govern adding any new trap handler:**

1. A special case added to record's `match stop` (in `record_box`) needs a **mirror** in replay's
   dispatch, and both must recompute *identical* addresses/bytes. Replay byte-compares its recomputed
   reply against the recording — that comparison *is* the divergence check, so an asymmetry surfaces
   as a divergence, not silent corruption.
2. Deterministic instruction emulation is better done **below the trace**, inside `Box_::run()`
   (as with the timebase MRS, the Apple-IMPDEF undef-MRS, and the B-family FPAC strip): `run()` is
   shared by record and replay, so such an arm fires identically on both sides and never surfaces to
   the record/replay loop — determinism is then automatic.

### Hard platform invariants (encoded in the box; violating them hangs or panics the machine)

- **W^X.** Executing a *writable* guest page hangs the vCPU on Apple Silicon. Code pages are RO+exec
  (`ATTR_CODE`), data is RW+non-exec (`ATTR_DATA`). Runtime data→exec promotion (`set_region_exec`)
  is sound with no further work only on a block the guest has never translated; on a block it *has*
  translated the stale RW/UXN entry must be invalidated first. The VMM cannot issue a guest TLBI, so
  **M9 has the guest issue it**: `flush_guest_tlb` runs `tlbi vmalle1` on the guest vCPU at EL1 from
  a scratch page (`ATTR_TRAMP` — `ATTR_CODE` sets PXN and `tlbi` is EL1-only), using the PAC signing
  oracle's save/restore discipline. `place_fixed` promotes-then-flushes on the FIXED-exec-over-live-
  backing path (dyld's non-cache-dylib strategy). Non-FIXED exec mmaps are still placed in fresh
  32 MiB-exclusive blocks — now an optimisation (a flush avoided), no longer a correctness rule.
- **SPTM / anon-only memory.** A *file-backed* `hv_vm_map` hard-panics macOS 26
  (`VIOLATION_ILLEGAL_MAPPING_TYPE`). All guest memory is anonymous; file bytes (the shared cache,
  file-backed mmap) are staged via `pread` into anon pages and, on record, captured as writes.
- **One VM per process** → `--test-threads=1`.
- **Drop order.** `Box_`'s field declaration order is load-bearing: `vcpu` must be declared before
  `vm` (HVF requires `hv_vcpu_destroy` before `hv_vm_destroy`). Don't reorder the struct fields.
- **Never reimplement Apple's PAC.** The box signs/authenticates by running `pac*`/`aut*` on the
  guest vCPU itself (the signing-oracle stub in `retrace-box`), with fixed keys identical on both runs.

### MMU / PAC / shared-cache stack (M2 and its sub-milestones)

The dynamic path (`load_dynamic`) turns the MMU on with a **47-bit guest VA** (`TCR_EL1.T0SZ=17`, a
3-level 16 KiB-granule walk `TTBR0→L1→L2→L3`; IPA stays 36-bit), enables PAC with fixed keys, loads a
real arm64 Mach-O plus `/usr/lib/dyld`, and builds dyld4's process-start stack. The arm64e **dyld
shared cache** is bound to the host process's PAC keys, so the box can't reuse the live cache: it
**demand-pages** each cache page from the on-disk file and **re-signs** every auth pointer with the
guest's keys (`cache.rs` `walk_page` + `Box_::sign_slots`). The v5 slide-info format only expresses
A-family (IA/DA) keys; B-family cache auths that FPAC-fault are handled by strip-on-FPAC emulation
(`try_emulate_fpac_auth`). The fixed guest IPA layout (trampoline, stack, page tables, TSD, commpage,
sign scratch, nano band, mmap base, shared-region window) is defined and explained in the constants
at the top of `crates/retrace-box/src/lib.rs`.

### Milestone / SDD workflow

Development is milestone-driven: **M0** (box + trace spine), **M1** (general memory-diff recorder),
**M2** (the loader: MMU-on, dyld, PAC) and its sub-milestones **M2-cache** (shared-cache re-signing),
**M2-mach** (`mach_msg2` kernel-RPC servicing), **M2-va47** (47-bit guest VA), **M2-bfam** (objc
B-family PAC); then **M3** (reverse execution), **M4** (checkpoints), **M5** (watchpoints),
**M6** (crash recording), **M7** (rung 1, a Rust guest), **M8-stack** (guest stack identity),
**M9-jq** (the guest-side TLBI oracle, argc/argv, and rung 2 — `brew jq`), **M10-fdtable** (a real
guest-visible fd table, and rung 3 — `jq` over a file), and **M11-signals** (the guest's own signal
dispositions, and a recordable abort). Each milestone has a
design spec in `docs/superpowers/specs/` and a task plan in
`docs/superpowers/plans/`; per-task reports and code-review diffs land in `.superpowers/sdd/`.

**Honest-gate discipline.** A headline end-to-end gate is parked `#[ignore]`d at the current wall,
with the wall documented honestly, rather than being faked green or deleted. When you clear a wall,
move the gate forward and rewrite that documentation — both the test's `#[ignore]` reason and the
README Status section. If nothing is left to park it at, un-`#[ignore]` it and say so.

As of M11-signals **all five headline gates are GREEN and un-ignored**: `hello_dyn_e2e` (a
dynamically-linked C program, green since M2-taskinfo), `hello_rust_e2e` (rung 1 — a real
full-`std` Rust binary reaching `main`, green since M8-stack), `jq_e2e` (rung 2 — `brew jq`,
the first guest loading dylibs outside the shared cache, green since M9-jq), `jq_file_e2e`
(rung 3 — `jq` over a file argument; it already passed before M10 and is *pinned*, not newly earned),
and `panic_e2e` (a full-`std` Rust guest that `panic!()`s into `abort()`/SIGABRT and records+replays
its own death, green since M11-signals — no gate was parked that milestone).
The gate is currently **240 passed / 0 failed / 0 ignored** (85 test binaries). Keep it that way: a
new wall gets a NEW parked gate for the capability it blocks, not a regression of these.

`jq_e2e` and `jq_file_e2e` depend on `/opt/homebrew/bin/jq`, which is not a repo artifact: they skip
with a loud `eprintln!` when jq is absent rather than passing quietly. That announcement is not
optional — a silent skip reads as a green it did not earn.
