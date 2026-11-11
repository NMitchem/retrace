# Guide

This walks through retrace on two small Rust crates with a failing test each, then the same questions in lldb,
then every command. The crates and `crashy.c` are installed with retrace, under
`$(brew --prefix)/share/retrace/examples`.

## Contents

- [The loop: record, summarise, find the writer](#the-loop)
- [Debugging the same way in lldb](#debugging-the-same-way-in-lldb)
- [Commands and exit codes](#commands-and-exit-codes)
- [Telling your agent about retrace](#telling-your-agent-about-retrace)


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
panic on thread 2 at (462, 12447)
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
them as printed. `(462, 12447)` is the panic's position in the recording: event 462, then 12447
instructions into it. The line under the backtrace gives the addresses of the `assert_eq!`'s two
sides. `left` is `l.balance`, the value that came out wrong. libtest prints a failing test's message
on stdout, so it shows up in the stdout tail.

`thread 2` is retrace's own index: 0 is the program's main thread, then threads in the order they were
created. It isn't the `(1048578)` in libtest's message, which is Rust's `ThreadId`, and lldb's
`thread #N` numbers threads its own way.

### Who wrote it?

Go to the panic and ask for the last write to `l.balance`. It is an `i64`, so the length is 8:

```sh
retrace debug target/retrace/ledger-844be10fb1d57b0c.retrace --script 'goto 462 12447; last-write 0x304125c0 8'
```

`--script` runs debugger commands in order, separated by `;`. `last-write` runs backwards from the
cursor to the last instruction that wrote that memory, and prints it with the backtrace at that point
(abridged):

```
> goto 462 12447
at (462, 12447) pc=0x100077ba8 thread=2  in __rustc::rust_panic
> last-write 0x304125c0 8
last write to 0x304125c0 (8 bytes): store at 0x100001ab4 in ledger::fees::charge+0x78 at /Users/you/ledger/src/fees.rs:14 on thread 2, (462, 2575)
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
retrace debug target/retrace/ledger-844be10fb1d57b0c.retrace --json --script 'goto 462 12447; last-write 0x304125c0 8'
```

From the summary's object and from `last-write`'s (excerpts):

```json
"panics":[{"position":[462,12447],"thread":2,"frames":[…],"assert":{"left":"0x304125c0","right":"0x10007e3a0"}}]
"position":[462,2575],"thread":2,"pc":"0x100001ab4","function":"ledger::fees::charge","offset":120,"file":"/Users/you/ledger/src/fees.rs","line":14,"found":true,"kind":"store","value_before":"f401000000000000","value_after":"6300000000000000"
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

The summary puts the panic on thread 1 at `(493, 10749)`, with the left operand at `0x302065c4`. The
count is a `u32`, so ask about 4 bytes:

```sh
retrace debug target/retrace/inventory-b1201d35949b6a76.retrace --script 'goto 493 10749; last-write 0x302065c4 4'
```

```
> goto 493 10749
at (493, 10749) pc=0x10007c72c thread=1  in __rustc::rust_panic
> last-write 0x302065c4 4
last write to 0x302065c4 (4 bytes): store at 0x10000534c in inventory::restock::add+0x9c at /Users/you/inventory/src/restock.rs:6 on thread 4, (488, 1797)
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
[`crashy.c`](../crates/retrace-guest/c/crashy.c) overruns an array into the pointer stored next to it,
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

Run in batch mode, the session needs `-k` for everything after the crash, and stdin should be `/dev/null`: after a crash stop, lldb otherwise waits for input on stdin. Once the program stops on a
crash, lldb drops the remaining `-o` commands and exits 0 without saying so, while commands given with
`-k` run:

```sh
retrace lldb crashy.retrace -- --batch -o 'process continue' -k 'bt' -k 'watchpoint set expression -w write -s 8 -- 0x1000080b0' -k 'process continue -R' -k 'bt'
```

`lldbbatch_e2e` runs this, and checks that an `-o` command after the crash is dropped. Built with `-g`
(`clang -g -o crashy crashy.c`, which also writes `crashy.dSYM` beside it), the same stops name source
lines instead of instructions: the crash at `crashy.c:31` and the store at `crashy.c:30`. The test checks
both, with `crashy.c` compiled as C and as C++.

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
its section of [`docs/walls.md`](walls.md):

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


## Telling your agent about retrace

In Claude Code, the skill installs as a plugin:

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
