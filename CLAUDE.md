# CLAUDE.md

Guidance for coding agents, and people, working on retrace's source.

## What this is

retrace records a macOS program on Apple Silicon and replays it exactly. The program runs in a single-vCPU
Hypervisor.framework VM that retrace builds itself, with no kernel inside, so every syscall and trap exits to
retrace. On record, retrace forwards each syscall to the real kernel and appends its effects (the return values
and the memory the kernel wrote) to a trace. On replay it executes no syscall: it checks each trap against the
recording and re-applies the recorded effects. Running backwards is replaying forwards to a position, with
checkpoints to keep that short.

It needs macOS 26 or 27 on Apple Silicon, runs as a normal user, and works with SIP on. Every binary that calls
`hv_*` needs the `com.apple.security.hypervisor` entitlement; an ad-hoc signature can carry it.

## Commands

```sh
cargo build --release -p retrace
codesign -s - -f --entitlements retrace.entitlements target/aarch64-apple-darwin/release/retrace
cargo run -p retrace -- record ./prog    # cargo's runner (tools/codesign-run.sh) signs for you
just gate                                # every test, then clippy with -D warnings
```

- Tests need `--test-threads=1`: Hypervisor.framework allows one VM per process, and a bare `cargo test` fails
  with `HV_BUSY`. `just gate` sets it.
- One test file: `cargo test -p retrace --test cpython_e2e -- --test-threads=1`.
- The whole suite runs longer than ten minutes. To run it in pieces, give each piece `--no-fail-fast` and read
  cargo's exit status before any pipe:

  ```sh
  cargo test --workspace --exclude retrace-box --exclude retrace -- --test-threads=1
  cargo test -p retrace-box -- --test-threads=1
  cargo test -p retrace --test <name> -- --test-threads=1   # once per file in crates/retrace/tests/
  cargo test -p retrace --bins -- --test-threads=1          # the CLI's own unit tests
  ```

  `--test <name>` runs neither unit tests nor doc tests, and nothing warns you they were skipped: keep the
  `--bins` piece, and add `cargo test -p <crate> --doc` for a library crate you split by file. `crates/retrace`
  has no library target, so `--lib` fails there.
- A test that runs the `retrace` binary itself, rather than through cargo's runner, must sign it first. Copy
  `bin()` in `crates/retrace/tests/util/mod.rs`.
- The end-to-end tests write their recordings under `$TMPDIR`, several GiB per full run.
- `RETRACE_TRACE=1` on `record` logs every trap, and is the first thing to try on a new failure.
  `RETRACE_VERBOSE=1` prints retrace's own diagnostics. Replay prints neither.

The toolchain is pinned in `rust-toolchain.toml`. `clippy.toml` bans `Instant::now` and `SystemTime::now`
(replay must not see the wall clock) and `std::thread::Thread` (retrace's own core is single-threaded; the
programs it runs can have many threads).

## Crates, in dependency order

- `hv-sys`: bindgen bindings for Hypervisor.framework, and thin wrappers (`Vm`, `Vcpu`, registers).
- `retrace-arch`: arm64 and macOS facts with no dependencies: syscall numbers and argument kinds, exception
  decoding, PAC instruction decoding, Mach-O constants, and the wall type.
- `retrace-trace`: the trace format: `Event`, `Writer` and `Reader`, a versioned header and a CRC per record.
- `retrace-guest`: the Mach-O loader, and the test programs (assembly, C, C++, Rust, Swift, Python, node) that
  its `build.rs` compiles.
- `retrace-box`: the VM. Guest memory and page tables, PAC, the vCPU loop (`Box_::run`), syscall forwarding with
  memory diffing, the shared-cache pager and re-signer, threads, signals, kqueues. The largest crate; start here.
- `retrace-core`: record and replay (`record_box`, `ReplaySession`), and the Mach message codec.
- `retrace-sim`: a deterministic RNG and fault injection for tests.
- `retrace`: the CLI (`record`, `replay`, `debug`, `lldb`, `gdbserver`) and the end-to-end tests.

## Keeping replay exact

Nothing that varies between runs may enter the program unless it is recorded. Everything else that could vary
is regenerated identically on both sides: shared-cache pages are a function of the file's bytes, a fixed slide
and fixed PAC keys; the clock is a synthetic counter; threads switch only at points that follow from the
program's own syscalls.

1. **Record and replay mirror each other.** A syscall handled specially on record (`record_box` in
   `crates/retrace-core/src/lib.rs`) needs the matching arm on replay (`ReplaySession::advance`), placed before
   the generic forwarding arm and calling the same `Box_` method with the same arguments. Replay compares what
   it recomputes with the recording, so a mismatch is reported as a divergence instead of corrupting the run.
2. **Prefer emulating below the trace.** What `Box_::run()` emulates (timebase reads, some system registers, the
   PAC strip on a B-key fault, store-exclusive rewrites, fault and trap conversion) happens the same way on both
   sides and never touches the trace.
3. **Thread identity is checked.** Every landmark event carries the thread that produced it, and replay checks
   it with `verify_thread`. A new replay arm that consumes an event and returns early needs its own check, or a
   replay on the wrong thread passes unnoticed.
4. **Changing `Event`, or what a snapshot's bytes mean, changes the format.** Bump `TRACE_MAGIC` in
   `retrace-trace`.
5. **Guest threads are emulated, never forwarded.** Thread creation, workqueues, psynch, Mach semaphores,
   kqueues, and polling on the program's own pipes and sockets are modelled in the box. Forwarding
   `bsdthread_create` would start a real host thread on the guest's code.
6. **Check a guest pointer before using it.** Forwarded calls go through `Box_::prepare_forward_pointers`;
   emulated calls read through `read_va_checked`. Each use of an unchecked accessor is listed in
   `crates/retrace-box/tests/guest-ptr-sites.txt`, and a test compares that list with the source.

## Walls and bugs

When a program does something retrace doesn't model, that is a wall: a box method returns
`Err(retrace_arch::wall(...))`, or code with no error path calls `retrace_arch::refuse!`. The CLI prints one
`retrace: unsupported: ...` line that links its section of `docs/walls.md`, and exits 4. Every `refuse!` site is
listed in `crates/retrace-box/tests/refuse-sites.txt`, which a test checks. Anything else that stops retrace is
a bug: the `internal error` banner and exit 70. Decide which at the site, never by matching message text later.

## Platform rules (breaking them hangs the vCPU or panics macOS)

- **W^X.** Executing from a writable guest page hangs the vCPU. Code pages are read-only and executable, data
  pages writable and not executable. Making executable a page the guest has already used as data needs a guest
  TLB flush (`flush_guest_tlb`).
- **Anonymous memory only.** A file-backed `hv_vm_map` panics macOS. File contents are copied into anonymous
  pages.
- **One VM per process.**
- **Field order in `Box_`.** `vcpu` is declared before `vm` so the vCPU is destroyed first. Don't reorder them.
- **Don't reimplement Apple's PAC.** Sign and authenticate by running `pac*` and `aut*` on the guest vCPU (the
  signing stub in `retrace-box`), with keys that are the same on every run.

## Tests

- The end-to-end tests in `crates/retrace/tests/` record and replay real programs. Each file's header says what
  it guards.
- A test that needs something outside the repository (Homebrew's `jq`, `python3` or `node`, Xcode, lldb) skips
  with a line written through `util::announce`, because libtest hides a passing test's `eprintln!`.
  `skiplines.rs` fails on an `eprintln!` skip.
- When a test reaches something retrace can't do yet, mark it `#[ignore = "..."]` with the reason. Don't loosen
  its assertion to make it pass.
- Assert on the difference your change makes. An exit status is rarely enough: an uncaught crash and a caught
  one that then dies both exit 139, so assert on the recording.
