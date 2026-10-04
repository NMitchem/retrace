# retrace

**Reverse debugging for command-line programs on macOS (Apple Silicon).**

`retrace` records a run of a real macOS binary, replays it bit-for-bit as many times as you like,
and lets you run it **backwards**. Set a watchpoint on the memory a crash tripped over, reverse-continue,
and land on the instruction, and the thread, that last wrote it. It serves the recording to
lldb, so `process continue -R` works.

It is a **technical preview**. It runs real programs (full-`std` Rust, C, stock Homebrew `jq`,
the CPython interpreter, Homebrew's `node` with its JIT on, Xcode's `git` for its local workflow,
and most of the Apple binaries in `/bin` and `/usr/bin`), but it is not yet "rr for macOS": no
preemptive thread scheduling, no `fork` or `exec`, and nothing that needs system services, I/O Kit,
or a GUI. [Limits](#limits) has the list.

## Demo: running a crash backwards in lldb

[`crashy.c`](crates/retrace-guest/c/crashy.c), a test fixture in this repo, overruns an array
into the pointer next to it, then dereferences that pointer. Build it and record it once (the
recording exits 139, the crash):

```sh
$ clang -o crashy crates/retrace-guest/c/crashy.c
$ retrace record-dyn "$PWD/crashy" -o t.bin
$ retrace gdbserver t.bin --port 5555
listening on 127.0.0.1:5555
```

Then debug the recording in stock lldb. This is abridged from the session `lldb_e2e` drives
against lldb-2100; it runs it twice and checks the transcripts match. `0x1000080b0` is `&g.ptr`:

```
(lldb) gdb-remote 127.0.0.1:5555
* thread #1, stop reason = start of recording
(lldb) process continue
* thread #1, stop reason = EXC_BAD_ACCESS (code=1, address=0x4000dead0000)
    frame #0: 0x00000001000005c4 crashy`main + 204
(lldb) watchpoint set expression -w write -s 8 -- 0x1000080b0
(lldb) process continue -R
Watchpoint 1 hit:
* thread #1, stop reason = watchpoint 1
    frame #0: 0x000000010000059c crashy`main + 164
->  0x10000059c <+164>: str    x8, [x9, x10, lsl #3]
(lldb) rsi
* thread #1, stop reason = trace
    frame #0: 0x0000000100000598 crashy`main + 160
```

That is the program run to its crash, then run **backwards** to the `str` that wrote the bad
pointer, then stepped back one more instruction. `lldb_e2e` runs the same session on the real
CPython interpreter crashing inside a script, too. A scriptable debugger is built in as well:

```
$ retrace debug t.bin --script 'continue; watch 0x1000080b0 8; reverse-continue; where'
…
> reverse-continue
hit watch 0x1000080b0 (write at 0x10000059c) at (245, 70)  in _main+0xa4
> where
at (245, 70) pc=0x10000059c thread=0  in _main+0xa4
```

## What it does

- **Record and replay.** `record-dyn` runs the program through the real `/usr/lib/dyld`; `replay`
  re-runs it without executing a single syscall and checks every step against the recording. A
  mismatch fails loudly, naming the event and pc.
- **Run backwards.** Seek to any instruction, `reverse-stepi`, `reverse-continue` to the previous
  breakpoint or watchpoint hit. Checkpoints make backward seeks fast.
- **Watchpoints that name the writer.** A hardware watchpoint plus reverse-continue finds the last
  store to an address, and says which thread made it.
- **Threads, timers and signals.** Multi-threaded guests (`std::thread`, pthreads with their
  condition variables, GCD's global queues, GCD timers on the uptime clock, and kqueues with
  timeouts) record and replay; every timed wait runs on a synthetic clock, so a 2-second timer costs
  no wall-clock time. Signals reach the thread they were sent to, through the handler the program
  installed.
- **Crashes.** A crashing run is a normal recording that ends at the fault, so you can debug it
  backwards.
- **JIT code.** node's V8 compiles JavaScript at run time; retrace models Apple's per-thread JIT
  write-protect, so a recording can be reverse-continued into JIT-compiled code, to the store that
  wrote a bad pointer.
- **lldb.** `retrace gdbserver` speaks gdb-remote: continue and step both ways, breakpoints,
  watchpoints, `bt`, registers, memory, threads. `rsi` (reverse step) ships as a small lldb script.
- **arm64e and PAC.** Apple's own binaries run with pointer authentication on.

## Limits

These are the ones you will meet first. [`docs/current-state.md`](docs/current-state.md#known-limits)
has every limit with the measurement behind it.

- **macOS 26 on Apple Silicon only.** retrace depends on measured behaviour of macOS 26 internals
  (dyld, libpthread, libdispatch, the shared cache), so a macOS update can break it.
- **Threads run one at a time and switch only when one blocks or exits.** That is what makes the
  schedule replayable without recording it, and it means **races that need preemption will not
  reproduce**. Parallel programs also run serially.
- **No `fork`, `exec` or `posix_spawn`.** `exec` and `posix_spawn` are refused with a message, and
  `fork` fails with `EAGAIN`, as it does natively at a process limit, so a program that must fork
  cannot (`git commit` still commits, after git's own `cannot fork() for maintenance` error).
  Record the program that does the work, not a launcher or wrapper: Homebrew's `python3`, for
  example, is a launcher that re-executes the real interpreter, and `/usr/bin/git` is an `xcrun`
  shim for Xcode's `git`.
- **Command-line programs only.** Service lookups over XPC, I/O Kit, and GUI frameworks are not
  modelled. A program that needs one stops at a named wall. kqueue readiness is modelled for a
  program's own pipes, so nothing that waits on a socket, a terminal or an inherited descriptor
  runs (no network, no interactive node).
- **Unmodelled syscalls are refused, never guessed at.** A program that reaches a syscall or Mach
  message retrace has no model for stops with a `RECORD ERROR` naming it. Of the 54 Apple binaries
  in the committed sample, 49 record and replay identically on an idle host, three of those by
  reaching the refused `posix_spawn` identically on both sides. The count moves by a row between
  runs with host state, `dddiagnose`'s bimodal fault (the latest run, on an idle host, counted 49;
  the one before it, 50); see [`docs/current-state.md`](docs/current-state.md#known-limits).
- **Traces are large and the format is not stable.** Tens of MiB to over a GiB, uncompressed
  (node's V8 reserves address space retrace backs in full: about 501 MiB for `console.log(1)`), and
  recordings from an older retrace are rejected rather than misread.
- **Debugging is instruction-level.** The built-in debugger has no DWARF, line numbers or
  backtraces, and names functions from the program's own symbols and dyld's, not the shared
  cache's. Under lldb you get at most six breakpoints and four watchpoints (the hardware's), the
  watchpoints are writes only, and `process interrupt` cannot stop a long reverse motion.

## Performance

`tools/bench.py` measures native vs record vs replay. Median of 5 runs, Apple M4 Pro, macOS 26.5.2,
release build, 2026-10-04:

| workload | native | record | replay | recorder RSS | trace |
|---|---|---|---|---|---|
| `/bin/echo hi` | 0.001 s | 0.20 s | 0.20 s | 104 MiB | 28 MiB |
| `/bin/ls /usr/bin` | 0.004 s | 0.23 s | 0.24 s | 107 MiB | 33 MiB |
| `jq` filtering a 5 MB JSON file | 0.08 s | 1.98 s | 2.05 s | 446 MiB | 286 MiB |
| CPython `-c 'print(1)'` | 0.012 s | 0.60 s | 0.62 s | 214 MiB | 85 MiB |
| CPython, a 30M-step loop | 0.95 s | 1.63 s | 1.64 s | 215 MiB | 85 MiB |
| node `-e 'console.log(1)'` | 0.041 s | 3.45 s | 3.53 s | 733 MiB | 502 MiB |

There is a **fixed start-up of about 0.2 s** (0.6 s for CPython) to build the VM and page in the
shared cache. node's is about 3.4 s; its V8 reserves hundreds of MiB of address space, which
retrace backs in full and keeps in the trace. After that, **compute runs within about 10% of
native**, because the guest executes natively on the CPU. **Syscalls that move a lot of memory are
the slow case**: each one is diffed and kept in the trace. [`docs/current-state.md`](docs/current-state.md#performance) has the full table
and how to read it.

## Getting started

**Requirements:** macOS 26 on Apple Silicon and a Rust toolchain (pinned by `rust-toolchain.toml`).
No root, SIP can stay on, and no Apple developer account is needed.

```sh
git clone https://github.com/NMitchem/retrace && cd retrace
cargo build --release
```

Every binary that uses Hypervisor.framework needs the `com.apple.security.hypervisor` entitlement,
which an ad-hoc signature can carry. `cargo run` signs for you (`.cargo/config.toml` sets
`tools/codesign-run.sh` as the runner). A binary you run directly must be signed once after each
build:

```sh
codesign -s - -f --entitlements retrace.entitlements target/aarch64-apple-darwin/release/retrace
```

The first run of a freshly signed binary can pause for a while in macOS's code-signing check. That
is not a hang. The examples here call that binary `retrace`: put its directory on your `PATH`, or
use the full path.

| Command | What it does |
|---|---|
| `retrace record-dyn <exe> -o <trace> [-- <args…>]` | record a program, running it through the real dyld |
| `retrace replay <trace>` | replay and verify; exits 3 on a divergence |
| `retrace debug <trace> --script '<cmds>'` | the built-in debugger (`continue`, `reverse-continue`, `stepi`, `reverse-stepi`, `break`, `watch`, `where`, `regs`, `threads`, `x`) |
| `retrace gdbserver <trace> [--port <n>] [--exe <path>]` | serve the recording to lldb |
| `retrace record <macho> -o <trace>` | record a freestanding static binary |

Pass the program an absolute path when you record, so lldb can find its symbols later. For lldb,
load the reverse-step command with
`command script import <repo>/crates/retrace/lldb/retrace.py`. `RETRACE_TRACE=1` on a recording
logs every trap, which is the first thing to look at when a program hits a wall.
[`docs/current-state.md`](docs/current-state.md#usage) has the full CLI and debugger reference.

## How it works

The program runs **natively, on one virtual CPU**, inside a Hypervisor.framework VM that retrace
builds itself: page tables, pointer-authentication keys, the dyld process-start stack. There is no
kernel in the VM. Every syscall traps out to retrace.

- **Recording** forwards each syscall to the real macOS kernel, diffs the guest memory the kernel
  could have written, and appends the result to the trace. Mach messages are serviced or forwarded
  one known message type at a time.
- **Replay** restores the opening snapshot and runs the same instructions again, but **never
  executes a syscall**: it checks each one against the recording, applies the recorded memory
  writes and return values, and compares the final memory byte for byte.
- **Nothing nondeterministic enters the trace.** What would (the shared cache, the clock,
  pointer-authentication signatures, the thread schedule) is regenerated identically on both
  sides instead. The shared cache is paged in from disk and re-signed with fixed keys, the timebase
  is synthetic, and the scheduler switches threads only at points that follow from the program's
  own syscalls.
- **Running backwards** is replaying forwards to the right place. Apple Silicon has no
  retired-instruction counter, so a position is a syscall landmark plus a single-stepped
  instruction offset, and checkpoints bound how far any seek has to replay.

The original design is in
[`docs/superpowers/specs/2026-07-05-retrace-macos-record-replay-design.md`](docs/superpowers/specs/2026-07-05-retrace-macos-record-replay-design.md);
`CLAUDE.md` holds the platform invariants (violating them hangs or panics the machine).

## Prior art

- **Warpspeed** (REcon 2023: [talk](https://nickgregory.me/files/talks/warpspeed.pdf),
  [write-up by Nick Gregory](https://nickgregory.me/post/2024/06/23/warpspeed/),
  [code](https://github.com/kallsyms/warpspeed)) is the earlier macOS record/replay debugger, and
  retrace is built on its idea: box the program in Hypervisor.framework, forward its syscalls, and
  record the kernel's writes by diffing memory. Its 2024 write-up calls it a proof of concept, with
  thread-switch and signal replay still to be built and no debugger front end. retrace shares no
  code with it (Warpspeed's repository carries no license), and adds those pieces:
  thread and signal replay, instruction-exact reverse execution, arm64e/PAC, and lldb.
- **[rr](https://rr-project.org/)** is the model for all of this, on Linux. It needs hardware
  performance counters, which Apple's hypervisor does not give a Linux VM; **[rr.soft](https://github.com/sidkshatriya/rr.soft)**
  works around that to run rr in a Linux VM on a Mac, for Linux programs.
- **WinDbg's Time Travel Debugging** on Windows and Undo's **UDB** on Linux are the equivalents
  elsewhere.
- **lldb** gained reverse execution over gdb-remote, the `bc`/`bs` packets rr's gdbserver speaks
  ([llvm-project#123945](https://github.com/llvm/llvm-project/pull/123945)). `retrace gdbserver`
  is a macOS backend for that same client.

## Documentation

- [`docs/current-state.md`](docs/current-state.md): what works today and what does not, in full,
  with measurements, plus the test gate. Edited in place; trust it over anything else.
- [`docs/status-log.md`](docs/status-log.md): the milestone-by-milestone engineering history,
  append-only.
- `docs/superpowers/specs/` and `docs/superpowers/plans/`: per-milestone designs and plans.
- `docs/sweep-evidence/`: the kept evidence behind the Apple-binary figures.
- `CLAUDE.md`: architecture, invariants, and working rules for contributors.

## Contributing

There is no CI, and there cannot be: the test suite needs macOS 26 on Apple Silicon and a working
`hv_vm_create`, which hosted macOS runners (themselves VMs) do not provide. Run the gate locally
(`just gate`; [`docs/current-state.md`](docs/current-state.md#testing) explains why it has to be
chunked and why `--test-threads=1` is mandatory) and paste the counts.

The one rule that matters most: **walls are documented honestly.** A test parked `#[ignore]` at a
measured limit, with the reason written on it, is worth more than a green bought by loosening an
assertion. Assert on the difference your change makes, and make a skipped test say so.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.

`hv-sys` binds Hypervisor.framework by running `bindgen` against the macOS SDK on your machine at
build time. No Apple headers, source or binaries are redistributed, and no dyld or shared-cache
bytes are vendored; they are read from the host at runtime.
