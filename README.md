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
- **Threads** — emulated `bsdthread_create`, a cooperative block-driven scheduler, and a divergence
  oracle that checks thread identity on every landmark.

**Gate:** 391 passed / 0 failed / 2 ignored across 102 test binaries, **measured at `f000c0d`**,
clippy clean over `--workspace --all-targets` with `-D warnings`. See the testing note below for how
that number is assembled.

**Trace format:** `TRACE_MAGIC` is `RT\x00\x08`. Recordings from before M16 are rejected whole.

## Known limits

These are real and current, not aspirational gaps.

- **No GCD / libdispatch path.** Threading is emulated at `bsdthread_create`; there is no
  `workq_open` / `workq_kernreturn` handling. Programs that get concurrency through libdispatch —
  most real macOS applications — are not supported.
- **The scheduler is cooperative,** switching only when a thread blocks or exits. That is what makes
  the schedule replayable without recording it, and it is a deliberate trade: interleavings that
  require preemption mid-critical-section never occur, so **races that need preemption to manifest
  will not reproduce here.**
- **Debugging is address-level.** No symbolization, no DWARF, no backtraces — every debugger operand
  is a raw address.
- **The trace format is not stable.** `TRACE_MAGIC` broke in both M15 and M16. Recordings are
  currently working artifacts, not things to keep across milestones.
- **Two gates are parked `#[ignore]`d** at documented, *measured* walls — the reasons are on the
  tests themselves:
  - `stackoverflow_rust_e2e` — libstd computes its guard page from a constant macOS 26 libpthread
    reports and retrace cannot influence, so the recursion takes a stage-2 fault instead of striking
    the guard (M8 risk R3).
  - `sigblocked_e2e` — signalling a thread **blocked** in `__ulock_wait` is unmodelled; a fail-loud
    guard fires rather than corrupting the blocked thread's resume point (M16 wall).

## Testing

```sh
just gate     # cargo test --workspace + clippy -D warnings
```

**`--test-threads=1` is mandatory.** Hypervisor.framework allows one VM per process, so in-process
VM tests must run serially. `just gate` sets it; a bare `cargo test` flakes with `HV_BUSY`.

**`just gate` does not currently complete as one command.** The full workspace run exceeds a
10-minute ceiling and gets killed — M14, M15 and M16 each closed on a chunked run instead. Split it,
run every chunk `--no-fail-fast`, and capture cargo's exit code *before* any pipe:

```sh
cargo test --workspace --exclude retrace-box --exclude retrace -- --test-threads=1
cargo test -p retrace-box -- --test-threads=1
cargo test -p retrace --test <name> -- --test-threads=1     # per-target for the e2e gates
```

Note that `cargo test -p retrace --lib` is invalid and fails the whole invocation.

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
