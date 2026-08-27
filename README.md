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
| 5 | `dispatch_dyn` (C) | `dispatch_async` onto a global concurrent queue, joined by a `dispatch_semaphore` |

**Capabilities**

- **Reverse execution** — `(N,K)` landmark seeks, checkpointed for ~800× faster backward seeks.
- **Watchpoints** — hardware `DBGW` (pre-retire) plus software detection, with
  reverse-continue-to-last-writer, thread-attributed.
- **Crashes are first-class** — a faulting guest is recorded, replayed, and seekable;
  reverse-continue reaches the corrupting store.
- **Symbolicated addresses** — since M19, pc-bearing debugger output names the function it is in:
  `guest crashed: pc=0x10000050c far=… esr=…  in _child+0x30`. The names are read from the
  recording's own opening snapshot, because `__LINKEDIT` is mapped into guest memory and the
  snapshot captures every backing — so **no binary path is supplied and no trace-format change was
  needed**, existing recordings gained symbols retroactively, and a stale-binary mismatch is not
  merely avoided but unrepresentable. The main executable and dyld resolve; see Known limits for
  where it stops.
- **Signals** — dispositions, handlers that actually run, `sigreturn`, alternate stacks, masks and
  pending sets. Per-thread since M16: `pthread_kill(child, SIGUSR1)` runs the handler on the child.
  Since M17 the child may be **blocked** in `__ulock_wait` when it is signalled: the signal pends and
  is materialised at the wake that makes the thread runnable.
- **Threads** — emulated `bsdthread_create`, a cooperative block-driven scheduler, and a divergence
  oracle that checks thread identity on every landmark. Every one of the oracle's eight checks now
  has a test that retags a real recording and proves it fires; the last of them, on the terminal
  `Crash` landmark, needed a guest that was threaded *and* fatal, which nothing in the tree was until
  `crashthread`.
- **libdispatch / GCD** — since M18, a guest that `dispatch_async`es onto a global concurrent queue
  runs, records and replays. `workq_open` (367) and `workq_kernreturn` (368) are emulated in the box
  and never forwarded; `REQTHREADS` builds the worker thread *inside* the VM and enters it at the
  guest's own registered `wqthread`; and the mach-semaphore pair — `semaphore_wait_trap` (`-36`) and
  `semaphore_signal_trap` (`-33`), which is what `dispatch_semaphore` actually lowers to — is a
  park/wake seam keyed on the port name. All of it is below or symmetric across the trace: nothing
  new is recorded and `TRACE_MAGIC` did not move.

**Gate:** 461 passed / 0 failed / 2 ignored across 105 test binaries, **measured at `97a4163`**,
clippy clean over `--workspace --all-targets` with `-D warnings`. See the testing note below for how
that number is assembled. The two ignored gates are `stackoverflow_rust_e2e` (M8 risk R3) and
`cache_symbol_e2e` (the M19 shared-cache symbol wall); both are described under Known limits.

**Trace format:** `TRACE_MAGIC` is `RT\x00\x08`. Recordings from before M16 are rejected whole.

## Known limits

These are real and current, not aspirational gaps.

- **libdispatch runs only as far as it has been measured.** Rung 5 records and replays, but the
  workqueue emulation is a floor built from measurements rather than an implementation of the
  kernel's, and everything past that floor refuses **by value** instead of guessing. `workq_kernreturn`
  knows exactly three opcodes — `0x400` (dispatch setup), `0x20` (`REQTHREADS`, which builds the
  worker) and `0x4` (the worker's park, which must never return) — and names any other in a `panic!`,
  because the opcodes a *running* worker can issue cannot be enumerated until one issues them.
  `semaphore_signal_trap` (`-33`) wakes **exactly one** waiter and asserts if it would wake more: the
  plural case owes two unmeasured answers, `semaphore_signal_all_trap` (`-34`, still refused by a
  family-wide guard over `-39..=-33`) and *which* waiter a single signal should pick. And a pending
  signal on a thread parked in `semaphore_wait_trap` **aborts** rather than being delivered — M17
  materialises at `__ulock_wake` using a measured correction to the woken thread's saved context, and
  nothing has measured the equivalent here, so the wake names the measurement it owes. One further
  value is an extrapolation and flagged as such at its call site: the QoS entry-flags word
  `0x244004`, which no live run reproduced.
- **The scheduler is cooperative,** switching only when a thread blocks or exits. That is what makes
  the schedule replayable without recording it, and it is a deliberate trade: interleavings that
  require preemption mid-critical-section never occur, so **races that need preemption to manifest
  will not reproduce here.**
- **Symbolication stops at the shared cache, and at whatever the binary kept.** Since M19 the
  debugger names functions in the guest's own image and in dyld, but three limits are real. **Shared
  cache addresses resolve to nothing** — cache images carry no `LC_SYMTAB` in the region mapped into
  the guest, and the cache's local-symbol area lives in the on-disk cache file that `cache.rs`
  demand-pages but never stages into guest memory, so those symbols are not in the recording at all.
  Since most of a dynamically-linked guest's executing pcs are *in* the cache, this is the difference
  between naming your own functions and naming everything; `cache_symbol_e2e` is parked there.
  **Stripped binaries yield nothing**, which is a property of the binary and not of retrace —
  `brew jq` ships with 7 defined text symbols against `threadrust`'s 969. And **Rust names are
  mangled** (`_ZN…E`): raw mangled names beat hex and need no demangler, but they are not pretty.
  Debugger *operands* are still raw addresses — you cannot yet `break _main`.
- **No DWARF, no line numbers, no backtraces.** M19 reads `LC_SYMTAB` only, so an address becomes
  `_child+0x30` and never `crashthread.c:35`. There is no unwinder, so there is no stack trace.
- **The trace format is not stable.** `TRACE_MAGIC` broke in both M15 and M16. Recordings are
  currently working artifacts, not things to keep across milestones.
- **A signal to a thread that never wakes is never delivered.** Signals to a blocked thread are
  pended and materialised at the wake that makes the thread runnable; retrace does not interrupt the
  wait with `EINTR` as a real kernel would. A guest that strands a signal this way fails loud at a
  **clean** exit rather than exiting 0 and swallowing it; a guest that is already crashing is
  diagnosed by its crash instead. **At most one signal materialises per wake**, and a second
  deliverable one aborts loudly rather than being dropped: queueing at a wake is unmodelled because
  no guest in the tree measures it.
- **Two gates are parked `#[ignore]`d** at documented, *measured* walls, and the reason is on each
  test itself. `stackoverflow_rust_e2e`, because libstd computes its guard page from a constant
  macOS 26 libpthread reports and retrace cannot influence, so the recursion takes a stage-2 fault
  instead of striking the guard (M8 risk R3). And `cache_symbol_e2e` since M19, at the shared-cache
  symbol wall above — a gate M19 parked for a capability it does not have, which by this repo's
  discipline has regressed nothing: `dispatch_e2e` was parked the same way by M18, moved twice as
  each measured wall fell, and then cleared.

## Testing

```sh
just gate     # cargo test --workspace + clippy -D warnings
```

**`--test-threads=1` is mandatory.** Hypervisor.framework allows one VM per process, so in-process
VM tests must run serially. `just gate` sets it; a bare `cargo test` flakes with `HV_BUSY`.

**`just gate` does not currently complete as one command.** The full workspace run exceeds a
10-minute ceiling and gets killed — M14 through M19 each closed on a chunked run instead.
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
453 / 0 / 2 over 104 instead of 461 / 0 / 2 over 105 — and nothing fails to warn you. Contrast
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
