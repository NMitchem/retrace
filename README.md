# retrace

An rr-style reverse debugger for macOS on Apple Silicon.

retrace records a run of a macOS command-line program, replays it exactly, and lets you debug the
replay backwards in lldb. Set a watchpoint on the memory a crash tripped over, run
`process continue -R`, and lldb stops on the instruction that last wrote it, in the thread that
wrote it.

It's a technical preview. It runs Rust and C programs, Homebrew's `jq`, the CPython interpreter,
Homebrew's `node` with its JIT on, tokio programs that open no network sockets, Xcode's `git` for local work, and most of `/bin` and `/usr/bin`.
The big differences from rr: threads only switch when one blocks, so races that need preemption
won't reproduce, and there's no `fork`: a program that starts a child process stops at a wall. See [Limits](#limits).

## Example

[`crashy.c`](crates/retrace-guest/c/crashy.c) overruns an array into the pointer stored next to it,
then dereferences that pointer. Record it once (the recording exits 139, the crash's SIGSEGV) and open it in lldb:

```sh
$ clang -o crashy crashy.c
$ retrace record ./crashy
retrace: the program was killed by SIGSEGV (fault at 0x4000dead0000, pc 0x1000005c4 in _main+0xcc)
retrace: saved the recording to crashy.retrace
$ retrace lldb crashy.retrace
```

Then, in lldb (`0x1000080b0` is `&g.ptr`):

```
(lldb) process continue
Process 1 stopped
* thread #1, stop reason = EXC_BAD_ACCESS (code=1, address=0x4000dead0000)
    frame #0: 0x00000001000005c4 crashy`main + 204
crashy`main:
->  0x1000005c4 <+204>: str    x8, [x9]
(lldb) watchpoint set expression -w write -s 8 -- 0x1000080b0
Watchpoint created: Watchpoint 1: addr = 0x1000080b0 size = 8 state = enabled type = w
(lldb) process continue -R
Watchpoint 1 hit:
old value: 70372480057344
new value: 4295000208
Process 1 stopped
* thread #1, stop reason = watchpoint 1
    frame #0: 0x000000010000059c crashy`main + 164
crashy`main:
->  0x10000059c <+164>: str    x8, [x9, x10, lsl #3]
(lldb) rsi
Process 1 stopped
* thread #1, stop reason = trace
    frame #0: 0x0000000100000598 crashy`main + 160
crashy`main:
->  0x100000598 <+160>: movk   x8, #0x4000, lsl #32
```

lldb runs to the crash, goes back to the `str` that wrote the bad pointer, then steps back one more
instruction. The `lldb_e2e` test drives this session, and the same one on CPython crashing inside a
script.

## Features

- Replay re-runs the program without executing a single syscall, checks every step against the
  recording, and fails loudly at the first mismatch.
- Reverse execution: seek to any instruction, reverse-step, or reverse-continue to the previous
  breakpoint or watchpoint hit. Checkpoints keep backward seeks fast.
- Multi-threaded programs record and replay, including pthreads and their condition variables,
  `std::thread`, contended mutexes, GCD queues and timers, kqueues with timeouts, and the harness
  `cargo test` builds. Timed waits, `sleep()` and `alarm()` run on a synthetic clock, so a 2-second timer or `sleep(2)` takes no wall-clock time.
- Signals reach the thread they were sent to, through the handler the program installed.
- A crash is an ordinary recording that ends at the fault, a NULL dereference or a Swift runtime trap included. It exits 128 + its signal, as natively, and a handler the program installed gets the signal, code and address it would get natively ([Known limits](docs/current-state.md#known-limits) has the edge cases).
- The program's stdout and stderr stay apart and appear as it writes them, and it sees the environment you ran retrace in.
- JIT code: retrace models Apple's per-thread JIT write protection, so you can reverse-continue into
  code V8 compiled at run time.
- `retrace gdbserver` speaks gdb-remote to lldb: continue and step in both directions, breakpoints,
  watchpoints, `bt`, registers, memory and threads.
- Apple's own binaries run as arm64e, with pointer authentication on.

## Limits

[`docs/current-state.md`](docs/current-state.md#known-limits) has every limit and the measurement
behind it. These are the ones you'll hit first:

- macOS 26 on Apple Silicon only. retrace depends on measured behaviour of macOS 26 internals (dyld,
  libpthread, libdispatch, the shared cache), so an OS update can break it.
- Threads run one at a time and switch only when one blocks or exits. That keeps the schedule
  replayable without recording it, but races that need preemption won't reproduce and parallel
  programs run serially. A thread that spins waiting for another one, without ever blocking, hangs
  the run; after 20 seconds of that the recorder prints one `retrace: thread N has run for 20 s`
  line naming the threads it is starving (ready to run, or waiting on a timeout). A program that spins
  without a system call never reaches its `alarm` either.
- One process. A program that replaces itself (`execve`, `posix_spawn` with `POSIX_SPAWN_SETEXEC`, a `#!`
  script) runs, recorded and replayed, and the debugger follows each image. A program that starts a
  child with `posix_spawn`, `system()` or `popen()` stops at a wall, and `fork` fails with `EAGAIN`. With
  Xcode selected, an `xcrun` shim such as `/usr/bin/git` can start a child first (when xcrun's cache is
  cold), which stops at that wall; run the tool itself or select the Command Line Tools.
- Command-line programs only. XPC service lookups, I/O Kit and GUI frameworks aren't modelled, and
  kqueue, `poll` and `select` readiness cover only a program's own pipes and `socketpair`s, so nothing that waits on a
  network socket, a terminal or an inherited descriptor runs; a tokio program that listens stops at `bind`,
  and one that connects stops when it first waits on its socket. A blocking read or write that has to wait for another thread to feed or drain a pipe or
  socket stops at a wall, and so does a handled signal sent to a thread blocked in a condition variable or mutex.
- A syscall or Mach message retrace has no model for stops the recording with one line naming
  it, `retrace: unsupported: …`, and exit status 4; [`docs/walls.md`](docs/walls.md) explains each
  kind. In the latest sweep, 46 of the 54 Apple binaries in the committed sample recorded and
  replayed with native's stdout and exit status, and 49 recorded and replayed identically
  (most of the rest start another process, which stops at a wall, or print host state).
- Of the 42 small C, C++, Rust and Swift programs in the first-contact corpus
  (`firstcontact_e2e`), the kind a newcomer tries first, 39 record and replay as they run natively
  (up from 36). The other three are a program that starts a child process, which stops at a wall, and two Swift programs that stop at a libdispatch call retrace does not model (a workloop, a workqueue opcode).
- A recording holds the program's environment, so a recording you share shares it; run retrace under env -i to pass only what the program needs.
- Traces are big: tens of MiB to over a GiB, uncompressed. node's `console.log(1)` is about 501 MiB
  because retrace backs V8's address-space reservations in full. The format isn't stable, and
  recordings from an older retrace are rejected.
- Debugging is instruction-level. The built-in debugger has no DWARF, line numbers or backtraces.
  Under lldb you get at most six breakpoints and four watchpoints (the hardware's), watchpoints
  trigger on writes only, and `process interrupt` can't stop a long reverse motion.

## Performance

Median of 5 runs with `tools/bench.py`, Apple M4 Pro, macOS 26.5.2, release build, 2026-10-04:

| workload | native | record | replay | recorder RSS | trace |
|---|---|---|---|---|---|
| `/bin/echo hi` | 0.001 s | 0.20 s | 0.20 s | 104 MiB | 28 MiB |
| `/bin/ls /usr/bin` | 0.004 s | 0.23 s | 0.24 s | 107 MiB | 33 MiB |
| `jq` filtering a 5 MB JSON file | 0.08 s | 1.98 s | 2.05 s | 446 MiB | 286 MiB |
| CPython `-c 'print(1)'` | 0.012 s | 0.60 s | 0.62 s | 214 MiB | 85 MiB |
| CPython, a 30M-step loop | 0.95 s | 1.63 s | 1.64 s | 215 MiB | 85 MiB |
| node `-e 'console.log(1)'` | 0.041 s | 3.45 s | 3.53 s | 733 MiB | 502 MiB |

Building the VM and paging in the shared cache costs about 0.2 s at startup (0.6 s for CPython,
3.4 s for node). After that, compute runs within about 10% of native, since the guest executes
directly on the CPU. Syscalls that move a lot of memory are the slow case: each one is diffed and
stored in the trace.

## Getting started

```sh
brew tap nmitchem/retrace https://github.com/NMitchem/retrace
brew install retrace
```

<<PRE_TAG_NOTE>> Until `v0.1.0` is tagged, the formula has no stable version: install with
`brew install --HEAD retrace`.

Homebrew builds retrace from source with the pinned Rust toolchain and signs it with the hypervisor
entitlement. You need macOS 26 on Apple Silicon and the Command Line Tools for lldb
(`xcode-select --install`). No root, no Apple developer account, and SIP can stay on.

To build it yourself you need a Rust toolchain (`rust-toolchain.toml` pins the version):

```sh
git clone https://github.com/NMitchem/retrace && cd retrace
cargo build --release
codesign -s - -f --entitlements retrace.entitlements target/aarch64-apple-darwin/release/retrace
```

Anything that uses Hypervisor.framework needs the `com.apple.security.hypervisor` entitlement, which
an ad-hoc signature can carry. Re-sign after every build. For development, `cargo run -p retrace -- ...`
signs for you through the runner in `.cargo/config.toml`. The first run of a freshly signed binary can
pause for a while in macOS's code-signing check; it isn't hung. The examples call the binary `retrace`,
so put its directory on your `PATH` or use the full path.

| Command | What it does |
|---|---|
| `retrace record [-o <trace>] <program> [<args...>]` | record a program; the trace is `<program>.retrace` unless `-o` names it |
| `retrace replay <trace>` | replay and verify; exits 3 on a divergence |
| `retrace lldb <trace> [--exe <path>] [-- <lldb args...>]` | serve the recording and open lldb on it, with `rsi` loaded |
| `retrace debug <trace> --script '<cmds>'` | the built-in debugger (`continue`, `reverse-continue`, `stepi`, `reverse-stepi`, `break`, `watch`, `where`, `regs`, `threads`, `x`) |
| `retrace gdbserver <trace> [--port <n>] [--exe <path>]` | serve the recording to an lldb you start yourself |

If a recording stops at a wall, rerun it with `RETRACE_TRACE=1` to log every trap.
[`docs/current-state.md`](docs/current-state.md#usage) has the full CLI and debugger reference.

## How it works

The program runs natively on one virtual CPU, in a Hypervisor.framework VM that retrace builds
itself: page tables, pointer-authentication keys, the dyld process-start stack. There's no kernel in
the VM, so every syscall traps out to retrace.

While recording, retrace forwards each syscall to the real macOS kernel, diffs the guest memory the
kernel could have written, and appends the result to the trace. Replay starts from the opening
snapshot and runs the same instructions, but never executes a syscall: it checks each one against
the recording, applies the recorded writes and return values, and compares the final memory byte
for byte.

Anything nondeterministic is regenerated identically on both sides instead of recorded. The shared
cache is paged in from disk and re-signed with fixed keys, the clock is synthetic, and the scheduler
switches threads only at points that follow from the program's own syscalls.

Running backwards means replaying forwards to the right place. Apple Silicon has no
retired-instruction counter, so a position is a syscall plus a single-stepped instruction offset,
and checkpoints limit how far any seek has to replay.

The original design is in
[`docs/superpowers/specs/2026-07-05-retrace-macos-record-replay-design.md`](docs/superpowers/specs/2026-07-05-retrace-macos-record-replay-design.md).

## Prior art

- [Warpspeed](https://github.com/kallsyms/warpspeed) ([talk](https://nickgregory.me/files/talks/warpspeed.pdf),
  [write-up](https://nickgregory.me/post/2024/06/23/warpspeed/)) is the earlier macOS record/replay
  debugger, and retrace is built on its idea: run the program in Hypervisor.framework, forward its
  syscalls, and capture the kernel's writes by diffing memory. retrace shares no code with it, and
  adds thread and signal replay, instruction-exact reverse execution, arm64e/PAC and lldb support.
- [rr](https://rr-project.org/) is the model for all of this, on Linux. It needs hardware
  performance counters that Apple's hypervisor doesn't expose to a Linux VM;
  [rr.soft](https://github.com/sidkshatriya/rr.soft) works around that to run rr on Linux programs
  in a VM on a Mac.
- WinDbg's Time Travel Debugging and Undo's UDB do the same job on Windows and Linux.
- lldb's reverse execution over gdb-remote
  ([llvm-project#123945](https://github.com/llvm/llvm-project/pull/123945)) uses the same packets
  rr's gdbserver speaks. `retrace gdbserver` is a macOS backend for it.

## Documentation

- [`docs/current-state.md`](docs/current-state.md): what works and what doesn't, with
  measurements, and the test gate. Trust it over anything else.
- [`docs/status-log.md`](docs/status-log.md): the milestone-by-milestone history.
- [`CLAUDE.md`](CLAUDE.md): architecture and the platform invariants for contributors.

## Contributing

There's no CI: the tests need macOS 26 on Apple Silicon and a working `hv_vm_create`, which hosted
macOS runners (themselves VMs) don't provide. Run `just gate` locally and paste the counts;
[`docs/current-state.md`](docs/current-state.md#testing) explains why it has to be chunked and why
`--test-threads=1` is required.

When a test hits a real limit, park it with `#[ignore]` and write the reason on it rather than
loosening an assertion to make it pass. Assert on the difference your change makes, and make a
skipped test say it skipped.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.

`hv-sys` generates its Hypervisor.framework bindings with `bindgen` against the macOS SDK on your
machine at build time. No Apple headers, source or binaries are redistributed, and dyld and
shared-cache bytes are read from the host at runtime.
