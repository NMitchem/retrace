# retrace

Time-travel debugging for Apple Silicon Macs.

retrace records one run of a program or a test suite. Then you, or your coding agent, ask the recording what
happened: how the run ended, and which line wrote any value, on which thread. Each question is one command. The
answer never changes, because the run only happened once.

## The crash is in SQLite. The bug isn't.

[`examples/visits.c`](examples/visits.c) is 25 lines of C that log 3 random visitors to an in-memory SQLite
database. About 1 run in 5, it crashes inside SQLite:

```c
struct { char last_visitor[16]; sqlite3_stmt *insert; int visits; } app;
...
memcpy(app.last_visitor, who, strlen(who) + 1);    // line 21
```

Every tool you'd reach for points at SQLite:

| Tool | What it tells you |
|---|---|
| Running it again | It passes 4 times in 5. |
| lldb | `EXC_BAD_ACCESS` inside `libsqlite3.dylib`, under your `sqlite3_bind_text` or `sqlite3_finalize`. |
| AddressSanitizer | `SEGV on unknown address` inside `libsqlite3.dylib`. No overflow found. |

Three tools, zero line numbers.

retrace takes the other route: record until a run fails, then ask who wrote the bad pointer.

```console
$ while retrace record ./visits; do :; done
...
retrace: the program was killed by SIGSEGV (fault at 0x6f48207961727275, pc 0x186f08d20 in vdbeUnbind+0x20 (libsqlite3.dylib))

$ retrace debug visits.retrace --script 'last-write app.insert'
> last-write app.insert
at the start of the recording: searching back from its end
last write to 0x100008070 (8 bytes): store at 0x185555f08 in _platform_memmove+0x1a8 (libsystem_platform.dylib) on thread 0, (546, 13428)
#0 0x185555f08 in _platform_memmove+0x1a8 (libsystem_platform.dylib)
#1 0x100000630 in _main+0x100 at /Users/you/visits.c:21
#2 0x1804b8d7c in start+0x19fc (dyld)
```

`last-write` runs the recording backwards to the last instruction that wrote `app.insert`: a `memcpy` on line
21. The fault address is text. `urray Ho` is 8 bytes of "Grace Brewster Murray Hopper", the one name too long
for the 16-byte field.

The loop is cheap. A recorded run of `visits` takes 0.33 s, so at 1 crash in 5 it catches one in under 2
seconds, on average. Then the failing run stays on disk, and you can ask it anything, as often as you like.

## Install

```sh
brew install nmitchem/retrace/retrace
```

You'll need macOS 26 or 27 on Apple Silicon and the Command Line Tools (`xcode-select --install`). No root, and
SIP stays on. The demo above is installed in `$(brew --prefix)/share/retrace/examples`.

To teach Claude Code when to reach for retrace, and how:

```sh
claude plugin marketplace add NMitchem/retrace
claude plugin install retrace@retrace
```

Other agents get the same instructions from the [guide](docs/guide.md#telling-your-agent-about-retrace): a few
lines to paste into `AGENTS.md`.

## Use

```sh
retrace record ./prog args        # run it once, recorded, into prog.retrace
retrace debug prog.retrace        # how it ended: the crash or panic, its thread and backtrace
retrace debug prog.retrace --script 'last-write counter'   # which line last wrote a global (or an address)
retrace lldb prog.retrace         # lldb, with reverse-continue and reverse-step
CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="retrace record" cargo test   # record every test binary
```

Every command is one line of shell, and its answer is plain text. Add `--json` to `debug` and each command
prints one JSON object instead, so an agent runs the same loop you do and parses the answer. The
[guide](docs/guide.md) walks through a failing Rust test, the same questions in lldb, and every command.

## How it works

The program runs on 1 virtual CPU, in a Hypervisor.framework VM that retrace builds itself, with no kernel
inside. Every system call exits to retrace. While recording, retrace passes each one to macOS and saves what
came back: the return value and every byte the kernel wrote. A replay runs the same instructions and feeds the
saved answers back. It never calls the kernel, so it's exact. Going backwards is replaying forwards to the right
instruction.

The cost: about 0.3 s to start, then computation within about 11% of native. System calls that move a lot of
memory are the slow part. [More](docs/how-it-works.md).

## Where it breaks

retrace is a technical preview, and its limits are real:

- **Command-line programs and test suites only.** C, C++, Rust and Swift (Swift without Dispatch queues or
  async/await). No GUI apps.
- **No network servers.** `bind`, `listen` and `accept` aren't supported yet.
- **One process.** A program can replace itself with `exec`, but starting a child process stops the recording.
- **Threads take turns.** They switch only when one blocks, so a race that needs 2 threads running at once
  won't reproduce.

When a program reaches something retrace can't run, it doesn't guess: it prints 1 line naming the call and a
link to [what that means](docs/walls.md), then exits with status 4. [Every limit](docs/limits.md).

Next time a test fails 1 run in 5, don't rerun it. Record it.

## More

[Guide](docs/guide.md) · [Limits](docs/limits.md) · [How it works](docs/how-it-works.md) ·
[What a wall means](docs/walls.md) · [Contributing](CONTRIBUTING.md)

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
