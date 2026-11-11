---
name: retrace
description: Use when a Rust, C or C++ test or program on an Apple Silicon Mac crashes or fails because a value is wrong, and you need to know what set that value, from which line and on which thread, or a crash happens far from its cause, or a failure is rare and you want to keep the one run that showed it. retrace records the run once and replays it identically every time, so you can stop at the failure and run backwards to the last write of a value. Do not use it for logic errors that reading the code and one rerun will solve, for Python or JavaScript bugs, or for programs that open network sockets, start child processes or need a GUI.
---

# Debugging a recorded run with retrace

retrace runs a program inside a small virtual machine, records everything the outside world gave it
(system calls, signals, the kernel's writes to memory), and can replay the run bit for bit. In the
replay you can stop at the failure and step or run backwards: "who last wrote this value?" has one
answer, and it is the same every time. It is the macOS counterpart of rr on Linux. It needs Apple
Silicon and macOS 26 or 27.

## When to use it

- A test fails because a value is wrong, and the code that set it is not obvious.
- A crash or panic happens long after the mistake that caused it.
- A failure is rare. Once recorded, it can be examined as often as you like.
- You cannot add prints, or adding them changes the behavior.

## When not to use it

- A logic error that reading the code and one rerun will solve. Recording costs more than a rerun.
- Python or JavaScript bugs. retrace records the interpreter, so you would be debugging its machine
  code, not your script.
- Programs that open network sockets, start child processes or need a GUI. These stop at a wall
  (see "Exit codes").
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
Build with debug info (`-g`, or a Cargo dev profile) so the debugger can show file and line.

A Cargo test suite, through cargo's runner. Set it in the environment, not in
`.cargo/config.toml`, where it would also apply to `cargo run`:

```sh
CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="retrace record" RETRACE_DIR=target/retrace cargo test --no-fail-fast
```

`RETRACE_DIR` is where a recording made without `-o` goes (created if missing). Use
`--no-fail-fast`: without it cargo stops after the first failing test binary and the rest are never
recorded. Facts about this form:

- Every test binary is recorded as `<name>-<hash>.retrace`: unit tests, each file in `tests/`.
- retrace prints `retrace: saved the recording to <path>` on stderr, under the `Running ...` line of
  its binary. The failing binary's trace is the one printed under the failing `Running` line.
- Doctests all record as `rust_out.retrace`, each overwriting the last, and print no saved line. To
  debug one doctest, run it alone with `cargo test --doc <name>`.
- A test filter still records every binary, including those the filter matches nothing in.
- Cargo's exit code is the test run's. A failing test gives 101, as without retrace.

## Query a recording

`retrace lldb <trace>` serves the recording to lldb and starts it. Run it in batch mode with
`--batch -o` commands after `--` so one command gives the whole answer.

Where did it stop, and with what call stack (file and line):

```sh
retrace lldb target/retrace/ledger-1a2b3c4d5e6f7a8b.retrace -- --batch -o 'breakpoint set -n rust_panic' -o 'process continue' -o 'bt'
```

For a C program, break on the failing function or `abort`. `bt` prints each frame with its file and
line, and `frame select N` followed by `frame variable` shows that frame's locals at the failure.

For a crash, `process continue` alone stops at the fault, and every command after it goes in a `-k`
flag instead of `-o`. Once the program stops on a crash, lldb's batch mode drops the remaining `-o`
commands and still exits 0; `-k` commands run after a crash:

```sh
retrace lldb crash.retrace -- --batch -o 'process continue' -k 'bt' -k 'watchpoint set expression -w write -s 8 -- <address>' -k 'process continue -R' -k 'bt'
```

Who last wrote this value. Stop at the failure, put a write watchpoint on the value's address, and
run backwards. The watchpoint stops at the instruction that stored to it, and `bt` there names the
line and thread:

```sh
retrace lldb <trace> -- --batch -o 'breakpoint set -n rust_panic' -o 'process continue' -o 'watchpoint set expression -w write -s 8 -- <address>' -o 'process continue -R' -o 'bt' -o 'thread list'
```

`-s` is the size in bytes. Run `process continue -R` again for the write before that one. If the
watchpoint's stop is not the one you want, `bt` shows who made it.

### The address of a Rust value

When the failure is an `assert_eq!`, the backtrace at `rust_panic` already has it: the
`core::panicking::assert_failed` frame shows `left=0x…`, the address of the assertion's left side.
For a variable or a field, that is the value's own address, so watch it in the same recording.

Otherwise a Rust local has no stable name in the debugger, so print its address from the test and
record again. The recording is deterministic, so the same address holds in the replay:

```rust
eprintln!("{:p}", &account.balance);
assert_eq!(account.balance, 100);
```

Run the recording recipe again, read the address from the output, and use it as `<address>`. For a C
variable, `lldb` can use the expression directly: `watchpoint set expression -w write -s 4 -- &counter`.

### The scripted debugger

`retrace debug <trace> --script '<commands>'` runs `;`-separated commands without lldb. It has no
file and line, so use it for positions and raw memory:

```sh
retrace debug <trace> --script 'where; continue; regs; threads'
```

The commands: `where`, `continue`, `reverse-continue`, `stepi [n]`, `reverse-stepi [n]`,
`break <symbol or address>`, `delete <symbol or address>`, `watch <address> [...]`, `unwatch <address>`,
`regs [thread]`, `threads`, `x <address> <length>`. Every position is `(n, k)`: event n, k instructions
into it. Output is the same on every run of the same trace.

## Read the result

| Exit | Meaning | What to do |
|---|---|---|
| the program's own | The program ran to its end. | Its code is what the program returned. |
| 128 + signal | The program died of a signal, as natively (139 is a segfault, 134 an abort). | Query the recording. |
| 2 | Bad input to retrace: a missing file, a file that is not an arm64 program, a missing entitlement. | Read the line retrace printed. |
| 3 | The replay did not match its recording. | A retrace bug. Report it with the trace. |
| 4 | A wall: the program did something retrace cannot run yet. | See below. |
| 5 | The debugger could not run, including on an incomplete recording. | Read the line retrace printed. |
| 70 | An internal error in retrace. | A retrace bug. Report it. |

A wall prints the program's output so far, then one line:

```
retrace: unsupported: posix_spawn of a child process (syscall 244) - see https://github.com/NMitchem/retrace/blob/main/docs/walls.md#process-creation
```

The line names what the program did and links the section of `docs/walls.md` that explains it. That is
retrace's limit, not a fault in the program: stop and debug by other means, or change the program
so it does not do that under test. The recording up to the wall stays on disk: `retrace replay`
runs it to the wall and exits 4, but `retrace lldb` and `retrace debug` refuse it with exit 5, because
debugging up to a wall is not supported.

## Clean up

Delete recordings when you finish: `rm -rf target/retrace`, or `rm <name>.retrace`. They are tens to
hundreds of MiB each, and a recording is only valid for the retrace version that made it.
