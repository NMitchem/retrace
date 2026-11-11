---
name: retrace
description: Use when a Rust, C or C++ test or program on an Apple Silicon Mac crashes or fails because a value is wrong, and you need to know what set that value, from which line and on which thread, or a crash happens far from its cause, or a failure is rare and you want to keep the one run that showed it. retrace records the run once and replays it identically every time; retrace debug then says how the run ended and finds the last write to a value, with its function, file and line, thread and backtrace, as text or JSON. If the source shows the bug at a glance, read the source instead; retrace pays off when the value was written out of sight of the failing assert (in another module, on another thread, by one of many writers, or long before the check), when the failure does not reproduce on demand, or for a crash. Do not use it for Python or JavaScript bugs, or for programs that serve or wait on network sockets, start child processes or need a GUI.
---

# Debugging a recorded run with retrace

retrace runs a program inside a small virtual machine, records everything the outside world gave it
(system calls, signals, the kernel's writes to memory), and can replay the run bit for bit. Each
question about the recording is one command: how did the run end, which instruction last wrote this
value, from which line, on which thread. A recording gives the same answer every time. It is the
macOS counterpart of rr on Linux. It needs Apple Silicon and macOS 26 or 27.

## When to use it

If the source shows the bug at a glance, read the source. retrace pays off when the wrong value was
written out of sight of the assert that caught it.

- A test fails because a value is wrong, and the code that set it is not obvious: it is in another
  module, on another thread, one of many writers, or long before the check.
- A crash or panic happens long after the mistake that caused it.
- A failure is rare or does not reproduce on demand. Once recorded, it can be examined at will.
- You cannot add prints to the program.

## When not to use it

- A bug the source shows at a glance, or a logic error that one rerun will solve. Recording costs more
  than reading or a rerun.
- Python or JavaScript bugs. retrace records the interpreter, so you would be debugging its machine
  code, not your script.
- Network servers. `bind`, `listen` and `accept` are walls (see "Read the result"), and tokio's TCP and
  Unix listener tests stop at `bind`. `socket` and `connect` are forwarded, but waiting on such a socket
  through kqueue, `poll` or `select` is refused (this is from reading retrace's code; no network client
  has been run).
- Programs that start other processes: `posix_spawn`, `system()` and `popen()` stop at a wall, and
  `fork` fails with `EAGAIN` (retrace prints a `retrace: refused:` line and the program goes on).
- GUI programs.
- Races that need preemption. Guest threads switch only when one blocks, so a race that depends on
  the scheduler interrupting a thread will not reproduce.

Recordings are tens to hundreds of MiB. Delete them when you are done (last section).

## Record

A program, static or dynamic, by path. The trace defaults to `<basename>.retrace`:

```sh
retrace record ./prog arg1 arg2
retrace record -o crash.retrace ./prog
```

Options go before the program. The program's output is unchanged and its exit code is yours.
Build with debug info (`-g`, or a Cargo dev profile) so the debugger can show file and line, and leave
the binary in place: file and line are read from the binary on disk, and a rebuilt one gives names only.

A Cargo test suite, through cargo's runner. Set it in the environment, not in
`.cargo/config.toml`, where it would also apply to `cargo run`:

```sh
CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="retrace record" RETRACE_DIR="$PWD/target/retrace" cargo test --no-fail-fast
```

`RETRACE_DIR` is where a recording made without `-o` goes (created if missing). It is absolute so
that in a workspace, where cargo runs each member's tests in that member's directory, every member
records into the root's `target/`. Use `--no-fail-fast`: without it cargo stops after the first
failing test binary and the rest are never recorded. Facts about this form:

- Every test binary is recorded as `<name>-<hash>.retrace`: unit tests, each file in `tests/`.
- retrace prints `retrace: saved the recording to <path>` on stderr, under the `Running ...` line of
  its binary. The failing binary's trace is the one printed under the failing `Running` line.
- Doctests all record as `rust_out.retrace`, each overwriting the last, and print no saved line. To
  debug one doctest, run it alone with `cargo test --doc <name>`.
- A test filter still records every binary, including those the filter matches nothing in.
- Cargo's exit code is the test run's. A failing test gives 101, as without retrace.

## Ask the recording

### What happened

```sh
retrace debug target/retrace/ledger-844be10fb1d57b0c.retrace
```

With no other arguments, `retrace debug` prints a summary. On a failing `assert_eq!` (abridged):

```
> summary
program: /Users/you/ledger/target/debug/deps/ledger-844be10fb1d57b0c
ended: exited with 101
panic on thread 2 at (468, 12447)
  #0 0x100077ba8 in __rustc::rust_panic+0x0
  ...
  #8 0x100001198 in ledger::tests::settling_four_hundred_leaves_the_rest+0x14c at /Users/you/ledger/src/lib.rs:63
  ...
  assert_eq! left operand at 0x304125c0, right at 0x10007e3a0
stdout (last 20 lines):
  ...
```

- `ended:` is `exited with <code>`, `killed by <signal>`, or for a crash `killed by <signal>, fault at <address>`.
- Each Rust panic, a caught one included and up to 16, comes with its position `(n, k)`, its thread
  and its backtrace. A frame in the program's own code has its file and line; paths are absolute.
- A failing `assert_eq!` adds the addresses of the two values it compared. For `assert_eq!(x.field, 100)`,
  `left` is the field itself.
- The last 20 lines of the program's stdout and stderr follow. libtest prints a failing test's panic
  message on stdout.
- For a crash or a signal, the lines after `ended:` say where it stopped: the thread, the position and
  the backtrace (`crash on thread 0 at (<n>, <k>)` or `signal on …`, then `#0 …`).
- `summary` leaves the cursor at the end of the recording, which for a crash is the faulting
  instruction. `goto` goes back to a panic.
- `thread N` is retrace's own index: 0 is the program's main thread, then threads in the order they
  were created. It does not match the number in libtest's `thread '<name>' (1048578)`, which is Rust's
  `ThreadId`, or lldb's `thread #N`.
- Only the program the recording started as is searched for panics. If it execs another program, the
  new program's panics are not listed.

### Who wrote this value

```sh
retrace debug <trace> --script 'goto <n> <k>; last-write <address> <size>'
```

`<n> <k>` is the panic's position and `<address>` the left operand's address, both from the summary.
`<size>` is the size of the left side's type in bytes, from the assert line or the field's declaration
(8 if omitted): 1 for `u8`, `i8`, `bool`; 2 for `u16`, `i16`; 4 for `u32`, `i32`, `f32`, `char`; 8 for
`u64`, `i64`, `usize`, `isize`, `f64`, a reference or a pointer. retrace refuses any other size (`watch len must be 1, 2, 4, or 8`) and an address that is not a multiple
of the size (`watch address 0x… must be 4-byte aligned`), with exit 5. A size larger than the value also
watches the bytes beside it, so it can report a write to a neighbour. A larger value (a `u128`, a struct)
cannot be watched whole: print the address of one of its fields with `{:p}` and watch that.

On the example above, `l.balance` is an `i64`, and `goto 468 12447; last-write 0x304125c0 8` prints:

```
> last-write 0x304125c0 8
last write to 0x304125c0 (8 bytes): store at 0x100001ab4 in ledger::fees::charge+0x78 at /Users/you/ledger/src/fees.rs:14 on thread 2, (468, 2575)
#0 0x100001ab4 in ledger::fees::charge+0x78 at /Users/you/ledger/src/fees.rs:14
#1 0x100000d0c in ledger::Ledger::settle+0x1c at /Users/you/ledger/src/lib.rs:25
...
```

- The first line names the store's function, file and line, thread and position; its backtrace follows.
  The writer may be another thread than the one that failed: a worker's backtrace includes the closure
  it was spawned with.
- Memory the kernel wrote (a `read` into a buffer) is named by its system call instead:
  `last write to <address>: syscall read (3) on thread 0, (<n>, 0)`.
- With no earlier write it prints `no earlier write to <address>: it holds what it held when the program
  started`, or `no write to <address> since the exec at (<n>, 0)`, and the cursor stays where it was.
- Otherwise the cursor moves to the write, so a second `last-write` finds the write before it. Without a
  position, `break rust_panic; continue` stops at the first panic.
- `last-write` uses one of the four hardware watch slots. With four `watch`es armed it is an error.

Positions can differ between two recordings of the same test, so take them from the recording you are
asking. One recording answers the same question the same way every time.

### JSON

Add `--json` (`retrace debug <trace> --json [--script '<cmds>']`) and each command prints one JSON object on
its own line: `command`, `ok`, `output` (the text lines), and after any motion `position`, `thread`,
`pc`, `function` (the readable name alone), `offset` (into the function), `file` and `line`. Beyond
those:

- `summary`: `programs`; `ending` (`kind` is `exit`, `crash` or `signal`, with `code` or `signal`, a
  crash's `fault_address`, and its `frames`); `panics` (each with `position`, `thread`, `frames` and,
  for an `assert_eq!`, `assert` with `left` and `right`); `threads`; `stdout_tail`; `stderr_tail`.
- `last-write`: `found`. When true, `kind` (`store` or `syscall`), `value_before`, `value_after`, and
  `frames` for a store or `syscall` (`number`, `name`) for a kernel write. When false, `reason` (`start`
  or `exec`).
- `bt`: `frames`, each with `index`, `pc`, `function`, `offset`, `file` and `line`.
- `value_before` and `value_after` are the bytes in memory order, in hex. Apple Silicon is
  little-endian, so `f401000000000000` is 500.
- `file` and `line` are `null` where there is no line information, and `source_note` says why:
  `no line information at this address`, `the binary changed since the recording`, that the binary
  was not found, or that `dsymutil` or `atos` (which read the binary's debug info) failed or timed out.
- A command that fails prints `{"command":…,"ok":false,"error":"…"}`, and retrace exits 5.

### Other commands

`where`, `bt [<thread>]`, `goto <n> <k>` (also `goto (n, k)`), `continue`, `reverse-continue`,
`stepi [n]`, `reverse-stepi [n]`, `break <name or address>`, `delete <name or address>`,
`watch <address> [size] [thread <n>]`, `unwatch <address>`, `threads`, `regs [thread]`,
`x <address> <length>`. Function names are readable (Rust and C++ demangled, the Rust hash dropped),
and `break` takes them: `break rust_panic`, `break ledger::tests::settling_four_hundred_leaves_the_rest`.
There are at most six breakpoints. Commands are separated by `;` and run in order.

### The address of a value no assertion names

A Rust local has no name in `retrace debug`, so print its address from the test and record again. The
address it prints is the address in that recording:

```rust
eprintln!("{:p}", &account.balance);
assert_eq!(account.balance, 100);
```

Read the address from the recording's output and use it with `last-write`.

### Variables: lldb

For variables by name, use lldb: `retrace lldb <trace>` serves the same recording to lldb and starts
it. Run it in batch mode, with `-o` commands after `--`:

```sh
retrace lldb <trace> -- --batch -o 'breakpoint set -n rust_panic' -o 'process continue' -o 'bt'
```

For a C program, break on the failing function or on `abort` instead. `frame select N` followed by
`frame variable` shows that frame's locals. lldb has no Rust language plugin, so it shows Rust
variables only in part.

For a crash, `process continue` alone stops at the fault, and every command after it goes in a `-k`
flag instead of `-o`. Once the program stops on a crash, lldb's batch mode drops the remaining `-o`
commands and still exits 0; `-k` commands run after a crash:

```sh
retrace lldb crash.retrace -- --batch -o 'process continue' -k 'bt' -k 'frame variable'
```

## Read the result

| Exit | Meaning | What to do |
|---|---|---|
| the program's own | The program ran to its end. | Its code is what the program returned. |
| 128 + signal | The program died of a signal, as natively (139 is a segfault, 134 an abort). | Ask the recording. |
| 2 | Bad input to retrace: a missing file, a file that is not an arm64 program or not a recording, a missing entitlement. | Read the line retrace printed. |
| 3 | The replay did not match its recording. | A retrace bug. Report it with the trace. |
| 4 | A wall: the program did something retrace cannot run yet. | See below. |
| 5 | A debugger error: a `retrace debug` command that failed (`DEBUG ERROR: …`, or the `"ok":false` line), a `gdbserver` error, or an incomplete recording. | Read the line retrace printed. |
| 70 | An internal error in retrace. | A retrace bug. Report it. |

`retrace lldb` exits with lldb's own status once lldb has started, and under `--batch` that is 0 even
when a crash stop made lldb drop the remaining `-o` commands.

A wall prints the program's output so far, then one line:

```
retrace: unsupported: posix_spawn of a child process (syscall 244) - see https://github.com/NMitchem/retrace/blob/main/docs/walls.md#process-creation
```

The line names what the program did and links the section of `docs/walls.md` that explains it. That is
retrace's limit, not a fault in the program: stop and debug by other means, or change the program
so it does not do that under test. The recording up to the wall stays on disk: `retrace replay`
runs it to the wall and exits 4, but `retrace debug`, `retrace gdbserver` and `retrace lldb` refuse it
with exit 5 (`retrace lldb` before it starts lldb), because debugging up to a wall is not supported.

## Clean up

Delete recordings when you finish: `rm -rf target/retrace`, or `rm <name>.retrace`. They are tens to
hundreds of MiB each, and a recording is only valid for the retrace version that made it.
