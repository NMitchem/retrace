# retrace

Time-travel debugging for Apple Silicon Macs.

retrace records a run of a program or a test suite. Then you, or your coding agent, can ask the recording how
the run ended and which line wrote any value, on which thread. Each question is one command, and it gives the
same answer every time.

## The crash is in SQLite. The bug isn't.

[`examples/visits.c`](examples/visits.c) logs three random visitors to an in-memory SQLite database. About one
run in five, it crashes:

```c
struct { char last_visitor[16]; sqlite3_stmt *insert; int visits; } app;
...
memcpy(app.last_visitor, who, strlen(who) + 1);    // line 21
```

The usual tools point at SQLite, or at nothing:

| Tool | What it tells you |
|---|---|
| Running it again | Passes four times in five. |
| lldb | `EXC_BAD_ACCESS` inside `libsqlite3.dylib`, under your `sqlite3_bind_text` or `sqlite3_finalize`. |
| AddressSanitizer | `SEGV on unknown address` inside `libsqlite3.dylib`. No overflow found. |

With retrace, record until a run fails, then ask who wrote the bad pointer:

```console
$ while retrace record ./visits; do :; done
...
retrace: the program was killed by SIGSEGV (fault at 0x6f48207961727275, pc 0x186f08d20 in a system library)

$ retrace debug visits.retrace --script 'summary; last-write 0x100008070'
...
last write to 0x100008070 (8 bytes): store at 0x185555f08 on thread 0, (546, 13428)
#0 0x185555f08
#1 0x100000630 in _main+0x100 at /Users/you/visits.c:21
```

`0x100008070` is `&app.insert`, which `lldb --batch -o 'p &app.insert' ./visits` prints. Frame #0 is `memcpy`,
and frame #1 is the line that called it. The fault address is text: `urray Ho`, from "Grace Brewster Murray
Hopper", the one name too long for the 16-byte field. The recording keeps the run that failed, so you can ask it
anything else, as often as you like.

## Install

```sh
brew tap nmitchem/retrace https://github.com/NMitchem/retrace
brew install nmitchem/retrace/retrace
```

You need macOS 26 or 27 on Apple Silicon and the Command Line Tools (`xcode-select --install`). No root, and SIP
stays on. The demo above is installed in `$(brew --prefix)/share/retrace/examples`.

To teach Claude Code when and how to use retrace:

```sh
claude plugin marketplace add NMitchem/retrace
claude plugin install retrace@retrace
```

For other agents, the [guide](docs/guide.md#telling-your-agent-about-retrace) has lines to paste into `AGENTS.md`.

## Use

```sh
retrace record ./prog args        # run it once, recorded, into prog.retrace
retrace debug prog.retrace        # how it ended: the crash or panic, its thread and backtrace
retrace debug prog.retrace --script 'summary; last-write <address> 8'   # which line last wrote those 8 bytes
retrace lldb prog.retrace         # lldb, with reverse-continue and reverse-step
CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="retrace record" cargo test   # record every test binary
```

Add `--json` to `debug` for one JSON object per command. The [guide](docs/guide.md) walks through a failing Rust
test, the same questions in lldb, and every command.

## Limits

retrace is a technical preview. It runs command-line programs and test suites written in C, C++, Rust and Swift
(Swift without Dispatch queues or async/await). It doesn't run network servers, programs that start other
processes, or GUI apps. Threads take turns, switching only when one blocks, so a race that needs two threads
running at once won't reproduce. [All limits](docs/limits.md).

## How it works

The program runs on one virtual CPU in a Hypervisor.framework VM that retrace builds itself. While recording,
retrace passes each system call to macOS and saves what it returned and wrote. A replay runs the same
instructions and feeds back the saved results, so it is exact, and going backwards means replaying forwards to the
right instruction. Startup costs about 0.3 s, and computation runs within about 11% of native speed; system
calls that move a lot of memory are the slow part. [More](docs/how-it-works.md).

## More

[Guide](docs/guide.md) · [Limits](docs/limits.md) · [How it works](docs/how-it-works.md) ·
[What a wall means](docs/walls.md) · [Contributing](CONTRIBUTING.md)

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
