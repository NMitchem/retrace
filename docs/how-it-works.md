# How it works

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
