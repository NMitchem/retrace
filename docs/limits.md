# Limits

retrace is a technical preview. This page lists what it runs, what it doesn't, and when it is the wrong tool.

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

## What it runs

Besides programs you build yourself, retrace runs Homebrew's `jq`, the CPython interpreter, Homebrew's
`node` with its JIT on, tokio programs that open no network sockets, Xcode's `git` for local work, and
most of `/bin` and `/usr/bin`. These are the limits you'll hit first:

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
  ([guide](guide.md#debugging-the-same-way-in-lldb)), but it has no Rust language plugin, so it shows Rust
  variables only in part. `summary` lists the panics of the program the recording started as, not of
  a program it execs. There are six breakpoints and four watch slots (the hardware's), and `last-write`
  takes one of the four. Watches trigger on writes only. A write retrace makes into the program's memory
  itself is not seen by `last-write` or a watch, so `last-write` names an earlier write: the result of a
  call it emulates (a `kevent` event list or a `poll` or `select` result, also one handed to a thread as it
  wakes) and a signal handler's frame. A Mach reply retrace answers is recorded, and is seen. Under lldb, each `process continue -R`
  replays forward from the start of the recording, and `process interrupt` can't stop a long reverse
  motion.
