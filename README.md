# retrace

A record/replay reverse debugger for Apple Silicon. See
`docs/superpowers/specs/2026-07-05-retrace-macos-record-replay-design.md`.

## Status: M0 — Box & Trace Spine ✅

Records a freestanding ARM64 guest inside a single-vCPU Hypervisor.framework VM and
replays it bit-for-bit from a snapshot, proving zero divergence over 200 fault-injection
seeds. Requires macOS 26.x on Apple Silicon.

M0's guest is a **freestanding synthetic binary** (`crates/retrace-guest/asm/hello.s`,
raw `write`/`exit` syscalls with the MMU off) — not the spec's `/bin/echo`-class
dynamically-linked program. Real dynamically-linked binaries need the dyld-shared-cache
loader, deferred to M1 (see below).

The divergence checker compares, per traced syscall, the `(num, args)` tuple and the
final exit code; byte-level memory/register landmark comparison (the spec's full oracle)
is deferred to M2. M0's bit-for-bit guarantee currently rests on determinism-by-construction
(the same recorded inputs replayed through the same deterministic handler) plus
CRC-checked trace integrity, not on an exhaustive state comparison.

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

## Next: M1 — real syscalls + dyld shared cache

M0 deliberately handles only `write`/`exit` on a freestanding guest with the MMU off.
M1 replaces the two-syscall handler with a **general memory-diff + pointer-chasing**
recorder (snapshot around pointer args, diff after, log the delta — no per-syscall models)
and adds a **dyld-shared-cache loader** so a real dynamically-linked binary
(`/bin/echo`-class, then interpreters) loads and runs. The trampoline, trace format,
snapshot, divergence checker, and seeded swarm from M0 carry forward unchanged.
