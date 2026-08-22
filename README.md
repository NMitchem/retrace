# retrace

A record/replay reverse debugger for Apple Silicon.

`retrace` runs a guest binary inside a single-vCPU Hypervisor.framework VM and records every
syscall and trap it takes, along with the memory the kernel wrote back through it. It then replays
that run **bit-for-bit** from a snapshot — never re-executing a syscall, only re-applying recorded
effects. Because replay is deterministic, execution can be driven *backwards*: seek to any point in
the run, set a watchpoint on an address, and ask which instruction — and which **thread** — last
wrote it.

Determinism is the whole design constraint. Nothing nondeterministic is allowed into the trace.
Anything that would be (shared-cache page contents, timing, PAC signatures, the thread schedule) is
instead *regenerated identically* on both sides rather than recorded. A divergence oracle compares
the two runs at every landmark and fails loudly on the first mismatch.

- **What runs today**, and what does not, is in "What works today" and "Known limits" below. Those
  are edited in place as reality changes, so they are the ones to trust.
- **How it got here**, milestone by milestone, is in [`docs/status-log.md`](docs/status-log.md) —
  append-only, and historical by design: each entry is true as of its own milestone, not today.
- **Design specs and task plans** are in `docs/superpowers/specs/` and `docs/superpowers/plans/`.

## Requirements

- **macOS 26.x on Apple Silicon.** Not optional — the box depends on macOS 26 SPTM and libpthread
  behaviour that was measured on it.
- Runs **non-root**; SIP may stay enabled.
- Every binary touching `hv_*` needs the `com.apple.security.hypervisor` entitlement. It is
  ad-hoc signable, so no developer account is required.
- Rust toolchain is pinned by `rust-toolchain.toml` (1.95.0, target `aarch64-apple-darwin`).

## Build

```sh
cargo build
```

Codesigning is automatic for anything cargo runs: `.cargo/config.toml` sets a cargo `runner`
(`tools/codesign-run.sh`) that ad-hoc-signs the binary with `retrace.entitlements` first.

**One exception matters.** A test that spawns a *separate* binary itself (via
`CARGO_BIN_EXE_retrace`) bypasses that runner and must sign it by hand — see
`crates/retrace/tests/util/mod.rs::bin()`, which signs a pid-unique copy rather than the shared
binary. Copy that pattern for any new test that spawns the CLI.

## Usage

```sh
retrace record     <macho> -o <trace>                  # freestanding static guest
retrace record-dyn <exe>   -o <trace> [-- <args…>]     # real guest through /usr/lib/dyld
retrace replay     <trace>                             # replay + verify against the recording
retrace debug      <trace> --script '<cmds>'           # reverse-debug a recorded trace
```

In development, invoke through cargo so the codesigning runner applies —
`cargo run -p retrace -- record-dyn <exe> -o t.bin`.

`RETRACE_TRACE=1` on a `record`/`record-dyn` run logs every dispatched trap and decodes `mach_msg2`
sends. It is the first thing to reach for on a bring-up failure. **Record-only** — `ReplaySession`
carries no trace instrumentation, so no `[trap]` line is ever printed on replay.

`replay` exits **3** on a divergence, naming the landmark, PC, and what mismatched.

### Debugger commands

Passed to `debug --script`, semicolon-separated:

| Command | Effect |
|---|---|
| `continue` / `reverse-continue` | run forward / backward to the next stop |
| `stepi [n]` / `reverse-stepi [n]` | single-step forward / backward |
| `break <addr>` / `delete <addr>` | set / clear a breakpoint |
| `watch <addr> [len] [thread <n>]` | watch a write, optionally scoped to one thread |
| `unwatch <addr>` | clear a watch |
| `where` | current landmark coordinate `(N,K)` and owning thread |
| `regs [tid]` | registers — of the current thread, or of a named (possibly blocked) one |
| `threads` | list every thread with its state, marking the current |
| `x <addr> [len]` | examine guest memory |

## What works today

**Guest breadth** — each of these records and replays byte-identically, twice:

| Rung | Guest | Notes |
|---|---|---|
| 0 | freestanding `-nostdlib -static` arm64 | 36 `asm/*.s` fixtures |
| 0 | `hello_dyn` (C) | real dynamic linking through `/usr/lib/dyld` |
| 1 | `hello_rust` | full-`std` `rustc` binary |
| 2 | `jq` | stock `brew` binary |
| 3 | `jq` + a file argument | |
| 4 | `threadrust` | `std::thread::spawn` + `join` |

**Capabilities**

- **Reverse execution** — `(N,K)` landmark seeks, checkpointed for ~800× faster backward seeks.
- **Watchpoints** — hardware `DBGW` (pre-retire) plus software detection, with
  reverse-continue-to-last-writer, thread-attributed.
- **Crashes are first-class** — a faulting guest is recorded, replayed, and seekable;
  reverse-continue reaches the corrupting store.
- **Signals** — dispositions, handlers that actually run, `sigreturn`, alternate stacks, masks and
  pending sets. Per-thread since M16: `pthread_kill(child, SIGUSR1)` runs the handler on the child.
  Since M17 the child may be **blocked** in `__ulock_wait` when it is signalled: the signal pends and
  is materialised at the wake that makes the thread runnable.
- **Threads** — emulated `bsdthread_create`, a cooperative block-driven scheduler, and a divergence
  oracle that checks thread identity on every landmark.

**Gate:** 420 passed / 0 failed / 2 ignored across 104 test binaries, **measured at `67e9a13`**,
clippy clean over `--workspace --all-targets` with `-D warnings` (clippy re-verified at `4d0f780`, the
only commit since — comments and documentation only, no executable code). See the testing note below
for how that number is assembled.

**Trace format:** `TRACE_MAGIC` is `RT\x00\x08`. Recordings from before M16 are rejected whole.

## Known limits

These are real and current, not aspirational gaps.

- **No GCD / libdispatch path.** Programs that get concurrency through libdispatch — most real macOS
  applications — are not supported. M18 has moved this wall twice and not yet cleared it. Stage 1
  stopped forwarding `bsdthread_register` to retrace's own already-registered host process, so it
  returns a real feature word, `_pthread_workqueue_supported` returns true, and libdispatch gets as
  far as bringing its workqueue up — `workq_open` (367) and `workq_kernreturn` (368) fire for the
  first time in this project's history. **Stage 2a emulates both below the trace**
  (`Box_::guest_workq_open` / `guest_workq_kernreturn`, a record arm and a replay mirror each, plus
  a fail-loud guard on the generic forward arm), so neither reaches the host kernel any more: the
  recorder no longer brings up a real workqueue for its own process and no longer has a host worker
  thread created inside it that jumps to address 0 and SIGSEGVs. What remains is **worker
  construction**. `workq_kernreturn`'s `REQTHREADS` opcode (`0x20`) is a deliberate named `panic!`,
  because the kernel enters a workqueue thread at the registered `wqthread` with a register contract
  no run here has measured, and building one from a guess would be invention. Behind that wall the
  next one is already measured
  (`docs/superpowers/specs/2026-08-21-retrace-m18-stage2b-measurements.md`): `dispatch_semaphore_wait`
  lowers to a raw Mach trap (`num=-36`) on a port minted by a forwarded `semaphore_create`, **not**
  to `__ulock_wait` — so the park/wake seam M14/M17 built on `pthread + 0x34` address equality does
  not fit it, and forwarding that trap wedges the recorder in an unbounded host blocking call.
- **The scheduler is cooperative,** switching only when a thread blocks or exits. That is what makes
  the schedule replayable without recording it, and it is a deliberate trade: interleavings that
  require preemption mid-critical-section never occur, so **races that need preemption to manifest
  will not reproduce here.**
- **Debugging is address-level.** No symbolization, no DWARF, no backtraces — every debugger operand
  is a raw address.
- **The trace format is not stable.** `TRACE_MAGIC` broke in both M15 and M16. Recordings are
  currently working artifacts, not things to keep across milestones.
- **A signal to a thread that never wakes is never delivered.** Signals to a blocked thread are
  pended and materialised at the wake that makes the thread runnable; retrace does not interrupt the
  wait with `EINTR` as a real kernel would. A guest that strands a signal this way fails loud at a
  **clean** exit rather than exiting 0 and swallowing it; a guest that is already crashing is
  diagnosed by its crash instead. **At most one signal materialises per wake**, and a second
  deliverable one aborts loudly rather than being dropped: queueing at a wake is unmodelled because
  no guest in the tree measures it.
- **Two gates are parked `#[ignore]`d** at documented, *measured* walls — the reason is on each test
  itself:
  - `stackoverflow_rust_e2e` — libstd computes its guard page from a constant macOS 26 libpthread
    reports and retrace cannot influence, so the recursion takes a stage-2 fault instead of striking
    the guard (M8 risk R3).
  - `dispatch_e2e` (rung 5, a guest that `dispatch_async`es onto a global concurrent queue) — parked
    at the Stage-2b wall above: retrace's own deliberate `REQTHREADS` refusal, because worker
    construction is not built. M18 parked this gate for a capability retrace does not yet have, and
    has moved it twice — once when Stage 1 cleared the `_pthread_workqueue_supported` BRK, and again
    when Stage 2a removed the host-worker hazard. Its file also carries an **un-parked** gate,
    `the_workqueue_syscalls_are_emulated_not_forwarded`, which asserts the difference Stage 2a made:
    the record run stops at retrace's own named wall, in its own process, rather than on a host
    workqueue thread.

## Testing

```sh
just gate     # cargo test --workspace + clippy -D warnings
```

**`--test-threads=1` is mandatory.** Hypervisor.framework allows one VM per process, so in-process
VM tests must run serially. `just gate` sets it; a bare `cargo test` flakes with `HV_BUSY`.

**`just gate` does not currently complete as one command.** The full workspace run exceeds a
10-minute ceiling and gets killed — M14 through M18 each closed on a chunked run instead.
Split it, run every chunk `--no-fail-fast`, and capture cargo's exit code *before* any pipe:

```sh
cargo test --workspace --exclude retrace-box --exclude retrace -- --test-threads=1
cargo test -p retrace-box -- --test-threads=1
cargo test -p retrace --test <name> -- --test-threads=1     # per-target for the e2e gates
cargo test -p retrace --bins -- --test-threads=1            # don't omit: see below
```

**Do not omit the `--bins` chunk.** `--test <name>` selects integration-test targets only, so the 8
unit tests inside the `retrace` binary itself (`crates/retrace/src/debug.rs`) run in none of the
other chunks; **only the unchunked `--workspace` run, or a whole-package `cargo test -p retrace`
without a `--test` filter, reaches them.** Leaving it out silently costs 8 tests and one binary —
412 / 0 / 2 over 103 instead of 420 / 0 / 2 over 104 — and nothing fails to warn you. Contrast
`cargo test -p retrace --lib`, which is invalid for this crate (there is no lib target) and fails the
whole invocation loudly.

**Run each `crates/retrace` test target as its own cargo invocation** — that is what keeps a chunk
inside the 10-minute ceiling above. It is no longer a codesigning requirement: `bin()` signs a
pid-unique copy (see Codesigning above), so concurrent test processes do not contend for it.

Some end-to-end gates depend on `/opt/homebrew/bin/jq`, which is not a repo artifact. They skip with
a loud `eprintln!` rather than passing quietly — a silent skip would read as a green it did not earn.

## Repository layout

| Crate | Role |
|---|---|
| `hv-sys` | bindgen FFI over Hypervisor.framework; thin safe wrappers |
| `retrace-arch` | zero-dependency arch facts: syscall numbers, ESR/PAC decode, Mach-O constants |
| `retrace-trace` | on-disk trace format: the `Event` enum, `Writer`/`Reader`, per-record CRC32 |
| `retrace-guest` | the Mach-O loader **and** the guest test programs (`asm/`, `c/`, `rs/`) |
| `retrace-box` | the core: guest memory, W^X page tables, PAC, the vCPU trap loop, the shared-cache pager |
| `retrace-core` | record/replay orchestration and the `mach_msg2`/MIG codec |
| `retrace-sim` | deterministic RNG + fault injection for the seeded swarm |
| `retrace` | the CLI binary and the end-to-end gates |

`spikes/*.c` are throwaway probes that empirically verified load-bearing HVF/SPTM/PAC claims on the
real OS before they were committed to the architecture; see `spikes/README.md`.

## Documentation

- [`docs/status-log.md`](docs/status-log.md) — the milestone-by-milestone engineering record, M0–M16,
  preserved verbatim. Historical: each entry is true as of its own milestone.
- `docs/superpowers/specs/` — per-milestone design specs.
- `docs/superpowers/plans/` — per-milestone task plans.
- `CLAUDE.md` — architecture invariants and working rules for this repository.
