# retrace

**Record a failing test once. Your agent can then ask it who wrote a value, from which line, on which
thread, and get the same answer every time.**

retrace records a run of a program or a test suite on an Apple Silicon Mac and replays it exactly. Ask
the recording how the run ended, and it gives each panic with its backtrace. Ask who last wrote a
value, and it runs backwards to that instruction and names its function, its file and line (for a
program built with debug info) and its thread, as text or as JSON.

That suits a coding agent, Claude Code or any other. A live debugger session is a conversation the
agent has to steer one prompt at a time. A recording is a file: each question is its own command, and
it gets the same answer every time, the way grep does. It replaces the loop of adding a print,
rebuilding and rerunning for a value that is already wrong by the time anything looks at it.

What it needs and what it runs:

- macOS 26 or 27 on Apple Silicon.
- Rust, C and C++ programs, including the test binaries `cargo test` builds, and Swift programs that
  use neither Dispatch queues nor async/await (a C program's GCD queues work, but Swift's
  `DispatchQueue` reaches a libdispatch call retrace doesn't model).
- Not network servers: a listener stops at `bind`. A program that waits on a network socket through
  kqueue, `poll` or `select` is refused too (this is from reading the code; no network client has
  been run).
- Not programs that start other processes: `posix_spawn`, `system()` and `popen()` stop at a wall, and
  `fork` fails with `EAGAIN`.
- Not GUI programs.
- A wall is one `retrace: unsupported: …` line and exit status 4. [`docs/walls.md`](docs/walls.md)
  explains each kind.
- Threads run one at a time and switch only when one blocks, so a race that needs preemption won't
  reproduce.

It's a technical preview.

## When to reach for it

If the source shows the bug at a glance, read the source. retrace pays off when the wrong value was
written out of sight of the assert that caught it:

- A test fails because a value is wrong, and the code that set it isn't obvious: it is in another
  module, on another thread, one of many writers, or long before the check.
- A crash whose cause comes well before the symptom, such as a pointer overwritten long before it is
  used.
- A failure that doesn't reproduce on demand, which you finally caught once and don't want to lose.
  The recording replays the same way every time.
- A program you can't add prints to.

And when not to:

- A bug the source shows at a glance, or a logic error that one rerun will solve. Recording costs more
  than reading or a rerun.
- A Python or JavaScript bug. retrace records the interpreter, so you would be debugging its machine
  code, not your script.
- Anything that needs the network or a child process.

## Install

```sh
brew tap nmitchem/retrace https://github.com/NMitchem/retrace
brew install nmitchem/retrace/retrace
```

<!-- PRE_TAG_NOTE: remove this paragraph once v0.1.0 is tagged (docs/launch-checklist.md step 5) -->
Until `v0.1.0` is tagged, the formula has no stable version: install with
`brew install --HEAD nmitchem/retrace/retrace`.

Homebrew builds retrace from source with the pinned Rust toolchain and signs it with the hypervisor
entitlement. You need macOS 26 or 27 on Apple Silicon (27.2 and 26.5.2 are the measured builds; other
point releases run but are untested) and the Command Line Tools (`xcode-select --install`), which
retrace uses for file and line numbers and for lldb.
No root, no Apple developer account, and SIP can stay on. The first run of a freshly signed binary can
pause for a while in macOS's code-signing check; it isn't hung.

Then tell your agent about it. In Claude Code, the skill installs as a plugin:

```sh
claude plugin marketplace add NMitchem/retrace
claude plugin install retrace@retrace
```

Inside a session the same two steps are `/plugin marketplace add NMitchem/retrace` and
`/plugin install retrace@retrace`. The skill is also a plain file, which Homebrew installs at
`$(brew --prefix)/share/retrace/skills/retrace/SKILL.md`: copy it into `~/.claude/skills/retrace/`, or
hand it to any other agent.

For an `AGENTS.md` or `CLAUDE.md`, these lines tell an agent that retrace is there and when to use it:

```markdown
retrace records a failing Rust, C or C++ program or test on an Apple Silicon Mac, once, and replays it exactly, so you can ask the recording which instruction last wrote a value, from which line and on which thread.
Reach for it when a value is wrong and the code that set it is not obvious, or a crash happens long after its cause. Skip it for logic errors a rerun will solve, Python or JavaScript bugs, and programs that use the network, child processes or a GUI.
Record: `CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="retrace record" RETRACE_DIR="$PWD/target/retrace" cargo test --no-fail-fast`, or `retrace record ./prog`. Then `retrace debug <trace>` says how the run ended, with each panic's backtrace, and `retrace debug <trace> --script 'goto <n> <k>; last-write <address> 8'` finds the last write to a value. Add `--json` for JSON.
The full recipe is the retrace skill (`claude plugin install retrace@retrace`), also installed at `$(brew --prefix)/share/retrace/skills/retrace/SKILL.md`.
```

## The loop

The example is `ledger`, a small Rust crate that Homebrew installs beside retrace. One of its tests
deposits 500, settles 400 and expects 100 to be left. It gets 99. Copy the crate somewhere writable:

```sh
cp -R "$(brew --prefix)/share/retrace/examples/ledger" . && cd ledger
```

### Record

```sh
CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="retrace record" RETRACE_DIR="$PWD/target/retrace" cargo test --no-fail-fast
```

The runner variable makes cargo start each test binary under `retrace record`. `RETRACE_DIR` is where
the recordings go. It is absolute so that in a workspace, where cargo runs each member's tests in that
member's directory, every member records into the root's `target/`. `--no-fail-fast` keeps cargo going
after the first failing test binary, so every binary is recorded. Set these in the environment, not in
`.cargo/config.toml`, where the runner would also apply to `cargo run`. The output, abridged
(`/Users/you/ledger` stands for your copy, here and below):

```
     Running unittests src/lib.rs (target/debug/deps/ledger-844be10fb1d57b0c)

running 3 tests
test tests::deposits_add_up ... ok
test tests::settling_four_hundred_leaves_the_rest ... FAILED
test tests::small_settlement_is_exact ... ok
...
thread 'tests::settling_four_hundred_leaves_the_rest' (1048578) panicked at src/lib.rs:63:9:
assertion `left == right` failed
  left: 99
 right: 100
...
retrace: saved the recording to /Users/you/ledger/target/retrace/ledger-844be10fb1d57b0c.retrace
error: test failed, to rerun pass `--lib`
     Running tests/flow.rs (target/debug/deps/flow-b3e1a684154f3fc3)
...
retrace: saved the recording to /Users/you/ledger/target/retrace/flow-b3e1a684154f3fc3.retrace
...
error: 1 target failed:
    `--lib`
```

Each test binary gets its own recording, named after the binary, hash included. The failing test's
recording is the one saved under its `Running` line. cargo exits as it would without retrace, 101 here.
Doctests are recorded too, all as `rust_out.retrace`, each overwriting the last, with no saved line. A
test filter (`cargo test <name>`) still records every test binary.

A program records the same way: `retrace record ./prog arg1 arg2` writes `prog.retrace` (into
`$RETRACE_DIR` when that is set), and the program's output and exit status are its own.

Your hashes, addresses and positions may differ from the ones shown here, so use the ones your own
run prints: two recordings of the same test can put the failure at different positions. One recording
answers the same question the same way every time.

### What happened?

```sh
retrace debug target/retrace/ledger-844be10fb1d57b0c.retrace
```

On its own, `retrace debug <trace>` summarises the recording: how the program ended, each Rust panic
with its position, thread and backtrace, the operands of a failing `assert_eq!`, and the last lines of
the program's output (abridged):

```
> summary
program: /Users/you/ledger/target/debug/deps/ledger-844be10fb1d57b0c
ended: exited with 101
panic on thread 2 at (468, 12447)
  #0 0x100077ba8 in __rustc::rust_panic+0x0
  ...
  #8 0x100001198 in ledger::tests::settling_four_hundred_leaves_the_rest+0x14c at /Users/you/ledger/src/lib.rs:63
  #9 0x100001e5c in ledger::tests::settling_four_hundred_leaves_the_rest::{{closure}}+0x18 at /Users/you/ledger/src/lib.rs:56
  ...
  assert_eq! left operand at 0x304125c0, right at 0x10007e3a0
stdout (last 20 lines):
  ...
  thread 'tests::settling_four_hundred_leaves_the_rest' (1048578) panicked at src/lib.rs:63:9:
  assertion `left == right` failed
    left: 99
   right: 100
  ...
threads:
  * thread 0: Runnable
    thread 1: Exited(0)
    thread 2: Exited(0)
    thread 3: Exited(0)
```

Frame #8 is the failing assertion, `src/lib.rs` line 63. File paths are absolute, so an agent can open
them as printed. `(468, 12447)` is the panic's position in the recording: event 468, then 12447
instructions into it. The line under the backtrace gives the addresses of the `assert_eq!`'s two
sides. `left` is `l.balance`, the value that came out wrong. libtest prints a failing test's message
on stdout, so it shows up in the stdout tail.

`thread 2` is retrace's own index: 0 is the program's main thread, then threads in the order they were
created. It isn't the `(1048578)` in libtest's message, which is Rust's `ThreadId`, and lldb's
`thread #N` numbers threads its own way.

### Who wrote it?

Go to the panic and ask for the last write to `l.balance`. It is an `i64`, so the length is 8:

```sh
retrace debug target/retrace/ledger-844be10fb1d57b0c.retrace --script 'goto 468 12447; last-write 0x304125c0 8'
```

`--script` runs debugger commands in order, separated by `;`. `last-write` runs backwards from the
cursor to the last instruction that wrote that memory, and prints it with the backtrace at that point
(abridged):

```
> goto 468 12447
at (468, 12447) pc=0x100077ba8 thread=2  in __rustc::rust_panic
> last-write 0x304125c0 8
last write to 0x304125c0 (8 bytes): store at 0x100001ab4 in ledger::fees::charge+0x78 at /Users/you/ledger/src/fees.rs:14 on thread 2, (468, 2575)
#0 0x100001ab4 in ledger::fees::charge+0x78 at /Users/you/ledger/src/fees.rs:14
#1 0x100000d0c in ledger::Ledger::settle+0x1c at /Users/you/ledger/src/lib.rs:25
#2 0x100001088 in ledger::tests::settling_four_hundred_leaves_the_rest+0x3c at /Users/you/ledger/src/lib.rs:59
#3 0x100001e5c in ledger::tests::settling_four_hundred_leaves_the_rest::{{closure}}+0x18 at /Users/you/ledger/src/lib.rs:56
...
```

The store is in `fees::charge`, `src/fees.rs` line 14, on thread 2, the thread the test ran on. It was
called from the test's `settle(400)` on line 59. `fee_for` charges a fee of 1 unless
`amount < FREE_LIMIT`, so a settlement of exactly 400 pays one, while the comment on `FREE_LIMIT` says
settlements up to and including 400 are free. The fix is `amount <= FREE_LIMIT`.

`last-write` leaves the cursor on the write it found, so a second `last-write 0x304125c0 8` in the same
script finds the write before that one, the `deposit(500)`. When the kernel wrote the memory (a `read`
into a buffer, say), `last-write` names the system call instead of a store.

The length is the size of the left side's type, which the assert line or the field's declaration
gives: 1 for `u8`, `i8` or `bool`, 2 for `u16` or `i16`, 4 for `u32`, `i32`, `f32` or `char`, and 8 for
`u64`, `i64`, `usize`, `isize`, `f64`, a reference or a pointer. retrace refuses a length other than 1,
2, 4 or 8 (`watch len must be 1, 2, 4, or 8`), and an address that isn't a multiple of the length
(`watch address 0x… must be 4-byte aligned`), with exit 5. A length larger than the value also watches
the bytes beside it, so it can report a write to a neighbour. A larger value, such as a `u128` or a
struct, can't be watched whole: print the address of one of its fields with `{:p}` and watch that.

If the program crashes instead of panicking, the summary's `ended:` line names the signal and the
fault address, and the next lines say where it stopped: the thread, the position and the backtrace.
`summary` leaves the cursor at the end of the recording, which for a crash is the faulting
instruction, so a `last-write` after it in the same script starts from there.

### The same answers as JSON

Add `--json` and each command prints one JSON object on one line. The text is still there, in the
`output` array, and the answer is also in fields:

```sh
retrace debug target/retrace/ledger-844be10fb1d57b0c.retrace --json
retrace debug target/retrace/ledger-844be10fb1d57b0c.retrace --json --script 'goto 468 12447; last-write 0x304125c0 8'
```

From the summary's object and from `last-write`'s (excerpts):

```json
"panics":[{"position":[468,12447],"thread":2,"frames":[…],"assert":{"left":"0x304125c0","right":"0x10007e3a0"}}]
"position":[468,2575],"thread":2,"pc":"0x100001ab4","function":"ledger::fees::charge","offset":120,"file":"/Users/you/ledger/src/fees.rs","line":14,"found":true,"kind":"store","value_before":"f401000000000000","value_after":"6300000000000000"
```

`function` is the readable name alone, and `offset` is how far into it the instruction is (the text's
`+0x78`). `kind` is `store` for an instruction and `syscall` for a kernel write. `value_before` and
`value_after` are the watched bytes in memory order, in hex. Apple Silicon is little-endian, so
`f401000000000000` is 500 and `6300000000000000` is 99. A command that fails prints
`{"command":…,"ok":false,"error":"…"}` and retrace exits 5.

### Which thread?

`inventory`, the second example Homebrew installs, receives a delivery on a worker thread. Its test
expects 32 items on the shelf and gets 28. From the directory that holds your `ledger` copy, copy it,
record it with the same recipe, and read its summary:

```sh
cp -R "$(brew --prefix)/share/retrace/examples/inventory" . && cd inventory
CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="retrace record" RETRACE_DIR="$PWD/target/retrace" cargo test --no-fail-fast
retrace debug target/retrace/inventory-b1201d35949b6a76.retrace
```

The summary puts the panic on thread 1 at `(490, 10749)`, with the left operand at `0x302065c4`. The
count is a `u32`, so ask about 4 bytes:

```sh
retrace debug target/retrace/inventory-b1201d35949b6a76.retrace --script 'goto 490 10749; last-write 0x302065c4 4'
```

```
> goto 490 10749
at (490, 10749) pc=0x10007c72c thread=1  in __rustc::rust_panic
> last-write 0x302065c4 4
last write to 0x302065c4 (4 bytes): store at 0x10000534c in inventory::restock::add+0x9c at /Users/you/inventory/src/restock.rs:6 on thread 4, (485, 1797)
#0 0x10000534c in inventory::restock::add+0x9c at /Users/you/inventory/src/restock.rs:6
#1 0x100000944 in inventory::Shelf::receive::{{closure}}::{{closure}}+0x24 at /Users/you/inventory/src/lib.rs:20
...
```

The test failed on thread 1, but thread 4 wrote the count. That is the thread `Shelf::receive`
spawns: frame #1 is its closure, at `lib.rs` line 20, and the store is in `restock::add`,
`src/restock.rs` line 6. There `CASE` is 10, while its comment says a case holds 12.

### A value no assertion names

`assert_eq!` hands the summary the addresses of its two sides. For any other value, print its address
from the test with `{:p}` and record again. The address it prints is the address in that recording:

```rust
eprintln!("balance at {:p}", &l.balance);
```

libtest shows a failing test's output with its failure. The example crate has this line behind an
environment variable: recorded with `LEDGER_SHOW_ADDR=1` set, its failure output starts with
`balance at 0x304125c0`, and `last-write 0x304125c0 8` from the panic finds the same store in
`fees::charge`. Without a position to `goto`, `break rust_panic; continue` stops at the first panic.

## Commands and exit codes

| Command | What it does |
|---|---|
| `retrace record [-o <trace>] <program> [<args...>]` | record a program. The trace is `-o`'s name, else `$RETRACE_DIR/<basename>.retrace` when `RETRACE_DIR` is set, else `<basename>.retrace` in the working directory |
| `retrace replay <trace>` | replay the recording and check it against what was recorded; exits 3 on a divergence |
| `retrace debug <trace>` | summarise the recording: how it ended (for a crash or a signal, with the thread, position and backtrace where it stopped), each Rust panic with its position, thread and backtrace, a failing `assert_eq!`'s operand addresses, the last lines of the program's stdout and stderr, and the threads |
| `retrace debug <trace> [--json] --script '<cmds>'` | run the debugger commands below, separated by `;`, in order. `--json` prints one JSON object per command, one per line |
| `retrace lldb <trace> [--exe <path>] [-- <lldb args...>]` | serve the recording and open lldb on it, with `rsi` (step back one instruction) loaded |
| `retrace gdbserver <trace> [--port <n>] [--exe <path>]` | serve the recording to an lldb you start yourself |

The debugger's commands, for `--script`:

| Command | What it does |
|---|---|
| `summary` | what `retrace debug <trace>` prints. It leaves the cursor at the end of the recording |
| `last-write <addr> [<len>]` | from the cursor, run back to the last write to `len` bytes at `addr` (1, 2, 4 or 8; 8 if omitted) and print the writer: a store with its function, file and line, thread, position and backtrace, or the system call that wrote it. The cursor moves to the write, so another `last-write` finds the one before. If nothing wrote it since the program started or since an exec, it says so and the cursor stays |
| `bt [<thread>]` | the backtrace of the current thread or of another, a blocked one included, with file and line where the debug info has them |
| `goto <n> <k>` | move to position `(n, k)`, as `summary`, `where` and `last-write` print it |
| `where` | the cursor's position, pc, thread and function |
| `continue`, `reverse-continue` | run forward or backward to the next breakpoint, watch hit or end |
| `stepi [<n>]`, `reverse-stepi [<n>]` | step instructions forward or backward |
| `break <addr or name>`, `delete <addr or name>` | set or clear a breakpoint, at most six. Names are readable: `break rust_panic` |
| `watch <addr> [<len>] [thread <n>]`, `unwatch <addr>` | stop on writes to memory. There are four watch slots, and `last-write` uses one |
| `threads`, `regs [<thread>]`, `x <addr> <len>` | the threads and their states, registers, memory |

Every command ends in one of these exit statuses, except `retrace lldb` once lldb has started: it
then exits with lldb's own status, which under `--batch` is 0 even when a crash stop made lldb drop
the remaining `-o` commands.

| Status | Meaning |
|---|---|
| the program's own | the program ran to its end (under `cargo test`, cargo's own status: 101 when a test fails) |
| 128 + signal | the program died of a signal, as it would natively: 139 for SIGSEGV, 134 for SIGABRT |
| 2 | bad input: arguments, an unreadable or unwritable file, not an arm64 Mach-O or not a recording, no hypervisor entitlement |
| 3 | the replay diverged from its recording, which is a bug in retrace |
| 4 | a wall: `retrace: unsupported: …` on `record`, or `retrace: incomplete recording: …` on `replay` |
| 5 | a debugger error: a `debug` command that failed (`DEBUG ERROR: …`), a `gdbserver` error, or an incomplete recording, which `debug`, `gdbserver` and `lldb` refuse |
| 70 | a bug in retrace: the `retrace: internal error` banner |

The line retrace prints decides which, never the status alone, since a program may exit 4 on its own.
A wall prints the program's output so far, then one line that names what the program did and links
its section of [`docs/walls.md`](docs/walls.md):

```
retrace: unsupported: posix_spawn of a child process (syscall 244) - see https://github.com/NMitchem/retrace/blob/main/docs/walls.md#process-creation
```

The limit is retrace's, and the program has not failed: debug it some other way, or change it so it
doesn't make that call under test. Under `cargo test` the line shows in cargo's output, followed by
cargo's `process didn't exit successfully: … (exit status: 4)`, and the recording is still saved. A
call that retrace answers with an error the program can handle, such as `fork`, is not a wall: it
prints `retrace: refused: <call>, returned <error> to the program` and the recording goes on. A
recording that ends at a wall replays up to the wall, but `retrace debug`, `retrace lldb` and
`retrace gdbserver` refuse it with exit 5: debugging up to a wall isn't supported. To report a wall or
a bug, record the program again with `RETRACE_TRACE=1` set, which logs every trap.

[`docs/current-state.md`](docs/current-state.md#usage) has the full CLI and debugger reference.

## What it can't do

Besides programs you build yourself, retrace runs Homebrew's `jq`, the CPython interpreter, Homebrew's
`node` with its JIT on, tokio programs that open no network sockets, Xcode's `git` for local work, and
most of `/bin` and `/usr/bin`. [`docs/current-state.md`](docs/current-state.md#known-limits) has every
limit and the measurement behind it. These are the ones you'll hit first:

- macOS 26 or 27 on Apple Silicon only. retrace depends on measured behaviour of macOS internals (dyld,
  libpthread, libdispatch, the shared cache), so an OS update can break it; another major is refused until it is
  measured. macOS 27 is measured (27.2); macOS 26 was measured at 26.5.2 and is kept by construction, not
  re-measured since. Other point releases of 26 and 27 run but are untested. A recording replays only on the
  macOS build that made it.
- Threads run one at a time and switch only when one blocks or exits. That keeps the schedule
  replayable without recording it, but races that need preemption won't reproduce and parallel
  programs run serially. A thread that spins waiting for another one, without ever blocking, hangs
  the run; after 20 seconds of that the recorder prints one `retrace: thread N has run for 20 s`
  line naming the threads it is starving (ready to run, or waiting on a timeout). A program that spins
  without a system call never reaches its `alarm` either.
- One process. A program that replaces itself (`execve`, `posix_spawn` with `POSIX_SPAWN_SETEXEC`, a `#!`
  script) runs, recorded and replayed, and the debugger follows each image. A program that starts a
  child with `posix_spawn`, `system()` or `popen()` stops at a wall, and `fork` fails with `EAGAIN`; a
  Rust test that runs a command through `std::process::Command` stops at the same wall. With
  Xcode selected, an `xcrun` shim such as `/usr/bin/git` can start a child first (when xcrun's cache is
  cold), which stops at that wall; run the tool itself or select the Command Line Tools.
- Command-line programs only. XPC service lookups, I/O Kit and GUI frameworks aren't modelled, and
  kqueue, `poll` and `select` readiness cover only a program's own pipes and `socketpair`s, so waiting on a
  terminal, an inherited descriptor or a network socket is refused (the network case is from reading the
  code: `socket` and `connect` are forwarded, but no network client has been run). A tokio program that
  listens stops at `bind`. A blocking read or write that has to wait for another thread to feed or drain a pipe or
  socket stops at a wall, and so does a handled signal sent to a thread blocked in a condition variable or mutex.
- A syscall or Mach message retrace has no model for stops the recording at a wall. In the latest
  sweep, 46 of the 54 Apple binaries in the committed sample recorded and replayed with native's
  stdout and exit status, and 48 recorded and replayed identically (most of the rest start another
  process, which stops at a wall, or print host state).
- Of the 42 small C, C++, Rust and Swift programs in the first-contact corpus
  (`firstcontact_e2e`), the kind a newcomer tries first, 39 record and replay as they run natively.
  The other three are a program that starts a child process, which stops at a wall, and two Swift programs that stop at a libdispatch call retrace does not model (a workloop, a workqueue opcode).
- A recording holds the program's environment, so a recording you share shares it; run retrace under env -i to pass only what the program needs.
- Traces are big: tens of MiB to over a GiB, uncompressed. Each `ledger` recording above is tens of
  MiB, and node's `console.log(1)` is about 501 MiB because retrace backs V8's address-space
  reservations in full. Delete them when you're done. The format isn't stable, and recordings from an
  older retrace are rejected.
- `retrace debug` works in addresses, functions, files and lines, and reads no variables by name. File
  and line come from the program's own debug info (the tests measure a Cargo dev profile; C built with
  `-g` was checked by hand), read from the binary on disk, so a rebuilt program gives names only and the
  note `the binary changed since the recording`. Where there is no line information, as at `rust_panic`
  in Rust's standard library, the note is `no line information at this address`. lldb reads variables
  ([below](#debugging-the-same-way-in-lldb)), but it has no Rust language plugin, so it shows Rust
  variables only in part. `summary` lists the panics of the program the recording started as, not of
  a program it execs. There are six breakpoints and four watch slots (the hardware's), and `last-write`
  takes one of the four. Watches trigger on writes only. Under lldb, each `process continue -R`
  replays forward from the start of the recording, and `process interrupt` can't stop a long reverse
  motion.

## Debugging the same way in lldb

To read variables, open the same recording in lldb. `retrace lldb` serves the recording to lldb and
starts it. Everything after `--` goes to lldb, and `--batch` runs the `-o`
commands in order, then quits. These are the ledger's two questions asked of lldb:

```sh
retrace lldb target/retrace/ledger-844be10fb1d57b0c.retrace -- --batch -o 'breakpoint set -n rust_panic' -o 'process continue' -o 'bt 12'
retrace lldb target/retrace/ledger-844be10fb1d57b0c.retrace -- --batch -o 'breakpoint set -n rust_panic' -o 'process continue' -o 'watchpoint set expression -w write -s 8 -- 0x304125c0' -o 'process continue -R' -o 'bt 4'
```

In the first backtrace, the `core::panicking::assert_failed` frame shows `left=0x00000000304125c0`.
The second stops at the same store and shows the arguments' values (abridged):

```
(lldb) process continue -R
* thread #2, stop reason = watchpoint 1
      frame #0: 0x0000000100001ab4 ledger-844be10fb1d57b0c`ledger::fees::charge::hb1eb88357e6affd1(balance=0x00000000304125c0, amount=400) at fees.rs:14:5
   11
   12  	pub(crate) fn charge(balance: &mut i64, amount: i64) {
   13  	    let fee = fee_for(amount);
-> 14  	    *balance -= amount + fee;
   15  	}
```

lldb prints a file's base name rather than its path, numbers threads its own way, and has no Rust
language plugin, so it shows Rust variables only in part. In a C program built with `-g`,
`frame variable` shows a frame's locals.

The same questions work in an interactive lldb session.
[`crashy.c`](crates/retrace-guest/c/crashy.c) overruns an array into the pointer stored next to it,
then dereferences that pointer. Record it once (the recording exits 139, the crash's SIGSEGV) and open it in lldb:

```sh
$ cp "$(brew --prefix)/share/retrace/examples/crashy.c" .
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

Run in batch mode, the session needs `-k` for everything after the crash. Once the program stops on a
crash, lldb drops the remaining `-o` commands and exits 0 without saying so, while commands given with
`-k` run:

```sh
retrace lldb crashy.retrace -- --batch -o 'process continue' -k 'bt' -k 'watchpoint set expression -w write -s 8 -- 0x1000080b0' -k 'process continue -R' -k 'bt'
```

`lldbbatch_e2e` runs this, and checks that an `-o` command after the crash is dropped. Built with `-g`
(`clang -g -o crashy crashy.c`, which also writes `crashy.dSYM` beside it), the same stops name source
lines instead of instructions: the crash at `crashy.c:31` and the store at `crashy.c:30`. The test checks
both, with `crashy.c` compiled as C and as C++.

## How it works

The program runs natively on one virtual CPU, in a Hypervisor.framework VM that retrace builds
itself: page tables, pointer-authentication keys, the dyld process-start stack. There's no kernel in
the VM, so every syscall traps out to retrace.

While recording, retrace forwards each syscall to the real macOS kernel, diffs the guest memory the
kernel could have written, and appends the result to the trace. Replay starts from the opening
snapshot and runs the same instructions, but never executes a syscall: it checks each one against
the recording, applies the recorded writes and return values, and compares the final memory byte
for byte. It fails loudly at the first mismatch.

Anything nondeterministic is regenerated identically on both sides instead of recorded. The shared
cache is paged in from disk and re-signed with fixed keys, the clock is synthetic, and the scheduler
switches threads only at points that follow from the program's own syscalls. Timed waits, `sleep()`
and `alarm()` run on the synthetic clock, so a 2-second timer takes no wall-clock time.

Running backwards means replaying forwards to the right place. Apple Silicon has no
retired-instruction counter, so a position is a syscall plus a single-stepped instruction offset,
and checkpoints limit how far any seek has to replay.

That model covers:

- Multi-threaded programs, including pthreads and their condition variables, `std::thread`, contended
  mutexes, GCD queues and timers, kqueues with timeouts, and the harness `cargo test` builds.
- Signals, delivered to the thread they were sent to through the handler the program installed.
- Crashes. A crash is an ordinary recording that ends at the fault and exits 128 + its signal.
- The program's stdout and stderr, kept apart and shown as it writes them, and the environment you ran
  retrace in.
- JIT code. retrace models Apple's per-thread JIT write protection, so you can reverse-continue into
  code V8 compiled at run time.
- Apple's own binaries, as arm64e with pointer authentication on.

`retrace gdbserver` speaks gdb-remote to lldb: continue and step in both directions, breakpoints,
watchpoints, `bt`, registers, memory and threads.

In short, it's rr for macOS. rr is the record-and-replay debugger for Linux, and retrace does the same
job for macOS programs. The main differences are that threads switch only when one blocks and that a
program can't start a child process.

The original design is in
[`docs/superpowers/specs/2026-07-05-retrace-macos-record-replay-design.md`](docs/superpowers/specs/2026-07-05-retrace-macos-record-replay-design.md).

### Prior art

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

## Performance

Median of 5 runs with `tools/bench.py`, Apple M4 Pro, release build, macOS 27.2 (26B123), 2026-11-10:

| workload | native | record | replay | recorder RSS | trace |
|---|---|---|---|---|---|
| `/bin/echo hi` | 0.002 s | 0.28 s | 0.28 s | 172 MiB | 37 MiB |
| `/bin/ls /usr/bin` | 0.006 s | 0.33 s | 0.33 s | 175 MiB | 41 MiB |
| `jq` filtering a 5 MB JSON file | 0.08 s | 1.12 s | 1.09 s | 599 MiB | 136 MiB |
| CPython `-c 'print(1)'` | 0.017 s | 0.60 s | 0.59 s | 282 MiB | 78 MiB |
| CPython, a 30M-step loop | 0.97 s | 1.66 s | 1.65 s | 284 MiB | 78 MiB |
| node `-e 'console.log(1)'` | 0.050 s | 3.60 s | 3.62 s | 1006 MiB | 501 MiB |

Building the VM and paging in the shared cache costs about 0.3 s at startup (0.6 s for CPython,
3.6 s for node). After that, compute runs within about 11% of native, since the guest executes
directly on the CPU. Syscalls that move a lot of memory are the slow case: each one is diffed and
stored in the trace.

## Building from source

You need a Rust toolchain (`rust-toolchain.toml` pins the version):

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

## Contributing

There's no CI: the tests need macOS 26 or 27 on Apple Silicon and a working `hv_vm_create`, which hosted
macOS runners (themselves VMs) don't provide. Run `just gate` locally and paste the counts;
[`docs/current-state.md`](docs/current-state.md#testing) explains why it has to be chunked and why
`--test-threads=1` is required.

When a test hits a real limit, park it with `#[ignore]` and write the reason on it rather than
loosening an assertion to make it pass. Assert on the difference your change makes, and make a
skipped test say it skipped.

- [`docs/current-state.md`](docs/current-state.md): what works and what doesn't, with
  measurements, and the test gate. Trust it over anything else.
- [`docs/status-log.md`](docs/status-log.md): the milestone-by-milestone history.
- [`CLAUDE.md`](CLAUDE.md): architecture and the platform invariants for contributors.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.

`hv-sys` generates its Hypervisor.framework bindings with `bindgen` against the macOS SDK on your
machine at build time. No Apple headers, source or binaries are redistributed, and dyld and
shared-cache bytes are read from the host at runtime.
