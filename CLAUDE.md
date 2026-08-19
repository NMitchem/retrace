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
- The headline end-to-end gates live in `crates/retrace/tests/` and run with the rest of the
  workspace; each records and replays a real guest program: `hello_dyn_e2e` (a dynamically-linked C
  program), `hello_rust_e2e` (rung 1 — full-`std` Rust), `jq_e2e` / `jq_file_e2e` (rungs 2–3 —
  `brew jq`, without and with a file argument), `panic_e2e` (a guest that aborts), `segv_rust_e2e`
  (a guest that faults and runs its own handler), `protnone_rust_e2e` (a guest that `PROT_NONE`s
  its own page and faults on it), `thread_rust_e2e` (rung 4 — a guest that spawns a thread and
  joins it), `thread_watch_e2e` (a guest whose two threads write different cells, where
  `reverse-continue` must name the thread that wrote the watched one). Run one with
  `cargo test -p retrace --test <name> -- --test-threads=1`.
- Some gates are `#[ignore]`d, parked at a documented wall — see "Honest-gate discipline" below for
  the rule, and the README's latest Status section for which ones and why.
- CLI: `cargo run -p retrace -- record <macho> -o t.bin`, `... record-dyn <exe> -o t.bin` (runs the
  exe through real `/usr/lib/dyld`; append `-- <guest args…>` to pass the guest an argv),
  `... replay t.bin`.
- `RETRACE_TRACE=1` on a `record`/`record-dyn` run logs every dispatched trap (and decodes
  `mach_msg2` sends) — the first tool to reach for on a bring-up failure. **Record-only:** `ReplaySession`
  carries no trace instrumentation, so no `[trap]`/`[mach_msg2]`/`[fault]` line is ever printed on replay.

The toolchain is pinned (`rust-toolchain.toml`: 1.95.0, target `aarch64-apple-darwin`). The
`clippy.toml` denials are load-bearing, not style, and are **two separate rules**:
`Instant::now`/`SystemTime::now` for determinism (see below); `std::thread::Thread` because
retrace's core is single-threaded by design. The latter governs the *recorder's* threads, not the
*guest's* — a guest may be multi-threaded (see "Guest threads").

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

1. A special case added to record's `match stop` needs a **mirror** in replay's dispatch, and both
   must recompute *identical* addresses/bytes. Both arms live in `crates/retrace-core/src/lib.rs` —
   record in `record_box`, replay in `ReplaySession::advance` — **not** in `retrace-box`, which owns
   only the `Box_` methods those arms call. A new arm goes in *both*, calling the *same* `Box_`
   method with the *same* arguments (that identity is what makes the rule hold by construction), and
   must sit *before* the generic forward arm. Replay byte-compares its recomputed reply against the
   recording — that comparison *is* the divergence check, so an asymmetry surfaces as a divergence,
   not silent corruption.
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

### Guest threads

A guest may be multi-threaded even though retrace's core is not. `bsdthread_create` is **emulated in
the box, never forwarded**: `Box_` holds a thread table of register contexts, and a cooperative,
block-driven scheduler switches only when a thread blocks or exits. That choice is a pure function of
the guest's own syscall sequence, so record and replay produce identical schedules with **nothing
recorded** and no trace-format change — symmetry rule 2 doing its job. The switch reuses the
save/restore discipline the PAC signing oracle and `flush_guest_tlb` already established. Blocking is
`__ulock_wait` (515) and waking is `__ulock_wake` (516), correlated by **address equality** on
`pthread + 0x34` — measured in both `__pthread_join` and `__pthread_joiner_wake`, so no
address→thread-index mapping is needed. Forwarding `bsdthread_create` is not merely wrong but
whole-process fatal (the host starts a real thread on retrace's own `_pthread_start`, which
PAC-fails on the guest's pthread struct), so it asserts.

**Emulating a syscall's entry contract is not the same as emulating the syscall.** Besides the new
thread's registers, `guest_bsdthread_create` must reproduce what the *kernel* writes on the way
through, or `pthread_join` returns success without ever waiting: the child's mach port at
`pthread + 0xf8`, `TPIDRRO_EL0 = pthread + 0xe0` (the TSD sits *inside* the struct), and
`PTHREAD_START_TSD_BASE_SET` in `w5`, which `__pthread_start` `tbz`-tests and `brk`s on. Each is
documented with its measurement at the call site.

**The divergence oracle checks thread identity.** `Event::Syscall` carries a `thread` tag (M15;
`TRACE_MAGIC` is now `RT\x00\x07`, so every pre-M15 recording is unreadable), and replay recomputes
the current thread and compares it at every syscall landmark — at all three landmark-consuming arms,
the generic dispatch plus the caught-raise and `sigreturn` mirrors. Without that check, two threads
running the same code issue byte-identical `(num, args)` and a wrong-thread replay continues in
silence. `Event::Sched` is **gone**, not reserved: emitting it would silently renumber every
landmark, and nothing in either dispatch loop can see a switch. The caveat that survives: only the
generic arm is exercised against a genuinely live second thread — the two signal-path arms are
reached only by a single-threaded fixture, so their tests prove the check *fires*, not that it
*distinguishes two live schedules*. (M14-threads, M15-threaddebug; see their specs and the README's
Status sections.)

## Milestone / SDD workflow

Development is milestone-driven, M0 onward. Each milestone has a design spec in
`docs/superpowers/specs/` and a task plan in `docs/superpowers/plans/` — both date-prefixed and named
for the milestone, so `ls` them for the current list rather than trusting a list written here.
Per-task reports and code-review diffs land in `.superpowers/sdd/`.

**The README's latest "Status" section is the single authoritative record** of what runs today, which
gates are green, and where the next wall is. Read it before starting work. Do not restate that status
in this file — a second copy is a copy that goes stale.

## Honest-gate discipline

A headline end-to-end gate is parked `#[ignore]`d at the current wall, with the wall documented
honestly, rather than being faked green or deleted. When you clear a wall, move the gate forward and
rewrite that documentation — both the test's `#[ignore]` reason and the README Status section. If
nothing is left to park it at, un-`#[ignore]` it and say so. A milestone that parks a *new* gate for
a capability it does not yet have has regressed nothing; that is the discipline working, not a
backslide.

Two rules these gates taught, which bind their successors:

- **Never assert on an exit code a weaker failure would also produce.** An *uncaught* fault exits 139
  exactly like a caught-then-fatal one (`crashy_e2e` asserts precisely that), so `segv_rust_e2e`
  asserts on the *trace* instead — the `SignalDelivery` to the handler libstd actually installed, the
  `sigreturn`, the terminal `Event::Crash`, the `resume_pc`. `protnone_rust_e2e` sharpens it: it
  asserts DFSC `0x0f` (permission) rather than `0x04..=0x07` (translation), because only the
  permission fault is the thing its milestone created. **Assert on the difference your work makes.**
  Per-test specifics belong in comments in the test file, next to the assertion they explain.
- **A skipped test must announce itself.** `jq_e2e` / `jq_file_e2e` depend on
  `/opt/homebrew/bin/jq`, which is not a repo artifact; they skip with a loud `eprintln!` rather than
  passing quietly. A silent skip reads as a green it did not earn.

One distinction that is easy to get backwards when writing such a test: a signal the guest **raises**
is `Event::Signal`, while one derived from a **hardware fault** whose disposition is not a handler
stays on the `Event::Crash` path.
