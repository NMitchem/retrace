# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`retrace` is a record/replay reverse debugger for Apple Silicon. It runs a guest binary inside a
single-vCPU Hypervisor.framework VM, records every syscall/trap plus the kernel's memory writes, and
replays the run **bit-for-bit** from a snapshot — never re-executing a syscall, only re-applying
recorded effects. The determinism oracle proves zero divergence between the two runs.

Requires **macOS 26 or 27 on Apple Silicon** (27.2 measured at M57; 26 kept by construction).
Runs non-root; SIP may stay enabled. Every binary that
touches `hv_*` needs the `com.apple.security.hypervisor` entitlement (ad-hoc signable — see
Codesigning below).

**`docs/current-state.md` is the current-state document** — what runs today, the known limits,
performance, and the gate. Read it before starting work. The **README** is the public front page:
a short summary, a demo, the headline limits and the performance table, drawn from
`current-state.md` and never contradicting it. The milestone-by-milestone history lives in
`docs/status-log.md` (verbatim, each entry true as of its own milestone) and the design specs in
`docs/superpowers/specs/`. Reach for the log when you need to know *how* something came to be or
what a past milestone measured; reach for `current-state.md` when you need to know what is true
now. (Until 2026-09-28 the README *was* the current-state document; it moved to
`docs/current-state.md` intact, so older specs, plans and comments that say "the README's Known
limits" mean that file.)

## Commands

```sh
just gate          # THE exit gate: cargo test --workspace + clippy -D warnings. `just m0`/`just m1` are aliases.
```

- **`just gate` does not currently complete as one command — expect to chunk it.** The full
  workspace run exceeds the 10-minute tool ceiling and gets killed; M14 through M20 each closed on
  a chunked run instead. Do not read a kill as a red. Split it, run every chunk `--no-fail-fast`,
  and capture cargo's exit code **before any pipe** (a pipe swallows it):
  ```sh
  cargo test --workspace --exclude retrace-box --exclude retrace -- --test-threads=1
  cargo test -p retrace-box -- --test-threads=1
  cargo test -p retrace --test <name> -- --test-threads=1   # per-target for the e2e gates
  cargo test -p retrace --bins -- --test-threads=1          # don't omit — see below
  ```
  **Do not omit the `--bins` chunk.** `--test <name>` selects integration-test targets only, so the
  81 unit tests inside the `retrace` binary itself (as of M58: 23 in `crates/retrace/src/debug.rs`, 14 in
  `crates/retrace/src/rsp.rs`, 13 in `crates/retrace/src/demangle.rs`, 8 each in `crates/retrace/src/main.rs` and
  `crates/retrace/src/unwind.rs`, 6 each in `crates/retrace/src/image.rs` and `crates/retrace/src/source.rs`, 3 in
  `crates/retrace/src/json.rs`) run in **none** of
  the other chunks; only the unchunked `--workspace` run, or a whole-package `cargo test -p retrace`
  with no `--test` filter, reaches them — which is why closes before M17 are not owed a
  correction. Leaving it out silently costs
  81 tests and one binary — and nothing fails to warn you. Contrast `cargo test -p retrace --lib`,
  which is invalid for this crate (there is no lib target) and fails the whole invocation
  **loudly**: the trap is that the wrong flag is loud and the missing one is silent.

  **`Doc-tests` is the same trap's second mouth.** `--test <name>` skips doc-test harnesses too, so
  splitting a *library* crate per-target drops that crate's `Doc-tests` target from every chunk. It
  runs zero tests, so nothing goes red; it silently costs one of the counted binaries. M24 split
  `retrace-box` per-target for CPU reasons and lost exactly that one, caught only by the file-by-file
  reconciliation below. If you split a library crate per-target, run `cargo test -p <crate> --doc`
  beside it.

  Then reconcile the total against the previous milestone's close by diffing `#[test]` counts
  file-by-file, rather than trusting a sum. Grep gate logs with `grep -a` — they carry ANSI and
  UTF-8 that trips plain grep.
- **`--test-threads=1` is mandatory.** HVF allows only one VM per process, so in-process VM tests
  must run serially. `just gate` sets it; a bare `cargo test` will flake with `HV_BUSY`.
- Single test: `cargo test -p <crate> <name> -- --test-threads=1`
  (e.g. `cargo test -p retrace-box --test pac -- --test-threads=1`).
- The headline end-to-end gates live in `crates/retrace/tests/` and run with the rest of the
  workspace; each records and replays a real guest program: `hello_dyn_e2e` (a dynamically-linked C
  program), `hello_rust_e2e` (rung 1 — full-`std` Rust), `jq_e2e` / `jq_file_e2e` (rungs 2–3 —
  `brew jq`, without and with a file argument), `panic_e2e` (a guest that aborts), `segv_rust_e2e`
  (a guest that faults and runs its own handler), `protnone_rust_e2e` (a guest that `PROT_NONE`s
  its own page and faults on it), `thread_rust_e2e` (rung 4 — a guest that spawns a thread and
  joins it), `thread_watch_e2e` (a guest whose two threads write different cells, where
  `reverse-continue` must name the thread that wrote the watched one), `sigthread_e2e` (a guest
  whose main signals its child by name, so the *child* runs the handler), `cpython_e2e` (rung 7 —
  the real CPython interpreter running `-c 'print(1)'`, plus the launcher shim, which since M54
  execs the real interpreter in place and must print what native prints), `bigread_e2e` (a guest whose one `read` returns more than the
  record-side diff window, guarding the M26 truncation class — repo-owned precisely because
  `cpython_e2e` *skips* without Homebrew Python and so cannot guard anything), `bigwrite_e2e` (its
  mirror on the write side: a guest whose one `write` sends more than the diff window, guarding the
  M30 class where retrace's own guard-band canary reached the kernel as data and corrupted the
  guest's output — record and replay both exited 0 while the output file was wrong, so it asserts on
  the written bytes and never on an exit code), `pipe_e2e` (a guest whose `pipe` pair must both be
  guest-numbered — the trace's `ret1` is the assertion, since before M38 the write end was the
  guest's own stale `x1`), `dupfd_e2e` (a guest whose `fcntl(F_DUPFD, 10)` must return exactly the
  guest minimum, a number no host `dup` could produce; since M44 also `F_DUPFD_CLOEXEC` then
  `F_GETFD` reading 1, as native does), `atfdcwd_e2e` (a guest whose
  `fstatat(AT_FDCWD, …)` must carry the 32-bit sentinel `0xfffffffe` in the trace AND succeed),
  `exec_e2e` (M54: exec in place — `execve`, `SETEXEC` `posix_spawn` and `#!` scripts run as native,
  a chain of twenty images, an exec from a worker thread, native's errnos, a pending signal ending the run
  as natively, and replay never reading the new binary from disk; each wall is asserted on its line and its
  `#process-creation` anchor), `execdebug_e2e` (M54: the debugger's exec epochs — `where` and `break` in the
  new image's symbols, an exec stop both ways, a checkpointed seek across an exec equal to a cold one, and the
  gdbserver's `reason:exec` stop and its refusal to reverse across one, through lldb as well), `cpython_crash_e2e`
  (rung 8 — the real interpreter on the repo's `crash.py`, asserting the marker line, the
  `Event::Crash` at the address the script computed from its data file, two byte-identical replays,
  and a scripted `reverse-continue` that reaches the store by its **effect** rather than by a
  symbol; skips loud without Homebrew Python), `vmremap_e2e` (the repo-owned guard for the M39
  `mach_vm_remap` stage-1 alias — a dynamically-linked C fixture that calls through an alias of its
  own text and `memcmp`s through an alias of a dylib's, so the mechanism is guarded on a machine without
  Python, where `cpython_crash_e2e` guards nothing), `watchsweep_e2e` (M40: a guest whose one
  store instruction sweeps a buffer before reaching the watched element, so `continue` and
  `reverse-continue` must resolve a watch hit by address, not by pc — the class that put rung 8's
  `continue` 1.7 M instructions early), `llsc_e2e` (M42: exclusive (LL/SC) pairs under stepping,
  seeking and native breakpoint/watch stops — the store-exclusive must land exactly as recorded;
  t0 measured hangs, phantom watch hits and a silent `no earlier hit` before the shadow monitor, and
  every CLI run is bounded so a regression fails rather than stalls), `hitorder_e2e` (M41: named
  regressions for every hit the debugger used to skip or invent, the thread-at-a-boundary
  invariant, and the hit oracle —
  `tests/util/hits.rs` single-steps a recording with everything armed and checks
  `continue`/`reverse-continue` chains against every hardware stop), `gdbserver_e2e` (M43: the
  `retrace gdbserver` RSP server on the wire, through a Rust client in `tests/util/rsp.rs` and no
  lldb — positions both ways, stepping, the refusals and the divergence recovery; M44 added
  `disarm-rsi`, a step across its own thread's exit, another thread's hit ending a blocked step, a
  step of a thread that is not running, and a step the recording's end comes before),
  `lldb_e2e` (M43: real lldb-2100 reverse-steps through `crashy`'s crash, plus the CPython demo, a
  determinism check, and the blocked-step and cross-thread `rsi` rows; M44 added `ni`, `next`,
  `thread step-out` and `finish` over a call, an arm64e `bt`, and bounded sessions for M44's
  stepping changes, each asserting lldb does not loop; it skips loudly without lldb, and its CPython
  test without Homebrew Python), `cli_e2e` (M56: one `record` for a static or dynamic program by a relative
  path, the default name, options after the program, the `record-dyn` refusal, `--version`/`--help`),
  `lldbcmd_e2e` (M56: `retrace lldb` reverse-debugs a relative-path recording in one command, the server gone
  after lldb exits, a SIGINT leaving retrace to reap the server, the trace directory as a symbol candidate, a
  rebuilt program without symbols and its UUID line, an incomplete recording ending at the server's line with
  no lldb, an unusable lldb and a missing trace as bad input; skips loudly without lldb), `apple_walls_e2e` (Apple binaries by path: M44's
  `ls_records_and_replays` and `ed_records_and_replays` run beside `launchctl`'s; the other seven
  are parked at their measured walls), `skiplines` (M44: the skip-line detector and its control), `evidence_secrets` (the M53 scrub: no file under `docs/` holds a credential, a credential-named variable's value or a known token prefix, each reported by name and never by value; and its control), `kqinit_e2e` (M45: libdispatch's
  workqueue-kqueue init, `kevent_qos` with `KEVENT_FLAG_WORKQ`, issued by hand from `kqinit_dyn.c`
  and emulated as one measured shape; asserts the landmark, the refusals by value, a rewritten
  return replay must name, and seeks across it), `gcdtimer_e2e` (M46: libdispatch's
  event manager and UPTIME timers on the synthetic clock — a `dispatch_after`, a repeating timer
  source, two timers in deadline order, the WALL refusal by value, a seek across the idle jump,
  `reverse-continue` to the handler's store on its worker, a replay-side refusal reported as a
  divergence, and R7's one-clock check through `mach_get_times`), `vmalign_e2e` (M47: a guest's
  masked `mach_vm_map`s, over the trap and the 4811 route, must come back aligned and replay — the
  repo-owned guard for the alignment mask retrace ignored before M47, which put xzone malloc's 4 MiB
  segment off its boundary and intermittently aborted git in libmalloc), `gitprims_e2e` (M47: the
  repo-owned fixtures for git's mechanisms — `fsops_dyn`'s forwarded path rows landing on disk,
  `madv_dyn`'s `MADV_ZERO` recomputed and never recorded, the no-op reuse pair and the refused
  advice, `rpath_dyn` loading through `@rpath` because AMFI is answered by the host, `sbxpath_dyn`'s
  Sandbox container-path call answered `ENOTSUP`, `forkfail_dyn`'s fork refused with `EAGAIN`, and
  two tampered-trace divergences), `git_e2e` (M47: Xcode's `git` — read commands against native,
  `add`, default-config `commit` through the refused maintenance fork, and `branch`, `tag`,
  `switch -c`, `mv` and `rm --cached`; skips loudly without Xcode), `simd_e2e` (M48: callee-saved
  SIMD registers across a thread switch and a `sigreturn`, and a threaded replay independent of the
  host environment — the hv-sys `set_simd` ABI fix), `trim_e2e` (M48: V8's aligned-reservation
  trim, a partial `munmap` that splits its backing), `kq_e2e` (M48: guest kqueues — libuv's
  runtime-detection probe, a cross-thread `EVFILT_USER` wake, a timeout reached by the idle jump, a
  pipe's readiness, `EV_ONESHOT`, the refusals by value, and seeks into a blocked `kevent` and
  inside a timed one), `condvar_e2e` (M48: psynch condition variables — ping-pong, broadcast, a
  timed wait that expires and one signalled first, node's {0, 1 ns} wait on the only thread, the
  refused mutex pair, a replay-side refusal, and a seek across a blocked `cvwait`), `jitwp_e2e`
  (M48: `MAP_JIT` write-protect per thread — native's toggle sequence, two threads in opposite
  modes, a store to a protected page recorded as the crash it is, V8's none-then-RWX shape, and a
  step across a toggle), `node_e2e` (M48 rung 9: node prints 1 with a `MAP_JIT` mapping in the
  recording and an SPRR write on the recorder's `RETRACE_SPRR` line, and a 2-second `setTimeout`
  lands exactly on its deadline by the idle jump; skips loudly without Homebrew node),
  `node_crash_e2e` (M48 rung 10: node's crash demo, reverse-continued from the crash to a store
  whose pc is in a `MAP_JIT` range; skips loudly without Homebrew node or its headers),
  `firstcontact_e2e` (M49: the tier-1 corpus — 30 repo-owned C, C++, Rust and Swift fixtures, each
  run native, recorded and replayed twice, with every fixture's outcome pinned by value in an
  outcome table, so a wall that moves fails the gate; Swift rows skip loudly without `swiftc`),
  `nullfault_e2e` (M49: NULL, unmapped and low-address faults recorded as crashes with native's
  signal, `si_code` and `si_addr`, with and without a handler, on a worker thread, as a stage-2
  instruction abort, a stack overflow on an alternate stack and one without, under single-step and
  across a seek; the readable TSD page pinned by value as a named
  limit, and a jump into unbacked memory as native's translation fault (M51 J1)), `xrewrite_e2e` (M49: discarded-status store-exclusives rewritten at every dynamic staging
  site — the executable, a `dlopen`ed dylib, and the census sites in dyld and libsystem_kernel —
  with the retry loop's documented limit pinned), `sleep_e2e` (M49: `sleep`, `usleep` and
  `nanosleep` on the synthetic clock, `clock_get_time` answered from the guest clock, two sleepers
  in deadline order, a polling loop on a short and on a 0-tick sleep, a sleep beside a runnable
  sibling, the refusals by value, a signal to a sleeper refused at the wake, replay-side refusals as
  divergences, and a seek across a wake), `cleanfail_e2e` (M50: one row per newcomer-reachable wall,
each asserting the program's output first, exactly one `retrace: unsupported:` line naming the call
with its docs anchor, exit 4, and a replay ending at the incomplete-recording line; plus bad input
with exit 2, an incomplete recording refused by `debug` and `gdbserver`, the bug banner's control
with exit 70, and `RUST_BACKTRACE=1` printing no frames for a wall), `output_e2e` (M51: stderr apart from stdout, both streamed as written and into a reader that goes away, no line of retrace's own on a plain run, `RETRACE_VERBOSE`, and the closing line), `traps_e2e` (M51: `brk`, `udf`, `hlt` and misaligned PC and SP as native's signal, `si_code`, `si_addr` and exit, with and without a handler, under single-step and across a seek; AMX and a trapped system register still walls), `framepush_e2e` (M51 T3-b: an unpushable handler frame, main or child thread, overflowed or bad alternate stack), `envp_e2e` (M51: the program's environment, `RETRACE_*` stripped, replay under another environment, the limit), `sigfidelity_e2e` (M51: `signal()` handlers, signal 0 for a pid and a thread, `abort_with_payload` from main, a thread and dyld), `badptr_e2e` (M51: native's error for a bad pointer, forwarded and emulated, and replay's compare), `vmfixed_e2e` (M52: a FIXED Mach VM request without OVERWRITE over occupied memory answers `KERN_NO_SPACE` as xnu does, native's own code per private window, the bump cursor floating past every FIXED placement, `mach_vm_copy` as a box-side copy; M57 added `mach_vm_reallocate` (4829) rows compared with native and a row for the bump allocator's checked placements), `threadid_e2e` (M52: each guest thread has its own synthetic id from `thread_selfid`, never forwarded, and the thread-lookup flavor of `proc_pidinfo` refused by value), `semtimed_e2e` (M52: `semaphore_timedwait_trap` with the kernel's count from create to destroy, a destroy waking its waiter with 37, each mode compared with native), `yield_e2e` (M52: `swtch_pri` and `thread_switch` yielding with one clock stride, and every due poll yielding first, so a zero-timeout poller beside a producer does not spin), `mutex_e2e` (M52: firstfit `psynch_mutexwait`/`mutexdrop` as a per-address FIFO with the kernel's word and its prepost, fairshare refused by value), `stall_e2e` (M52: a thread that runs 20 s without blocking while others are ready to run or waiting on a timeout prints the one stall line, on record only, and the replay of such a recording is pinned as never printing it), `windiff_e2e` (M52 fast-follow: 1000 `gettimeofday` calls with the timeval on the stack, where every byte the trace records for a call lies within a word of the timeval; before it, each call recorded its whole diff window, the rest of the stack), `posix_e2e` (M53: the 45 census rows and mach trap −25, then four second-layer rows, each mode of `posix_dyn` run native, recorded and replayed twice with its lines pinned by value, and a `flock` that would wait ending at one clean wall line), `iovec_e2e` (M53: `readv`, `writev`, `preadv` and `pwritev` translated — a console `writev` printed on record and both replays, an entry of 200 KiB, native's errnos for the count limits and an unreachable entry, a pipe `readv` with a later bad entry refused, and a pre-M53 trace refused by name), `kqdup_e2e` (M53: a guest kqueue shared by its `F_DUPFD_CLOEXEC` dups, mio's waker sequence waking a timed wait, closing a watched descriptor dropping its knote, and a seek into the blocked wait on the duplicate), `pollsel_e2e` (M53: `poll`, `select` and `pselect` over guest pipes compared with native state by state, a blocked poll woken by another thread's write, timeouts, the refusals by value and a seek into a blocked poll), `alarm_e2e` (M53: `ITIMER_REAL` and `alarm` on the synthetic clock, `getitimer`, `sigsuspend` returning `EINTR` with its mask restored, an alarm ending a sleep, the thread xnu picks for a process-directed signal, `EINVAL` for a bad timer or signal, `ITIMER_VIRTUAL` refused, and a seek across the fire), `sockpair_e2e` (M55: `socketpair` as a guest object numbered by retrace, both directions, `EAGAIN`, the buffer edges, EOF, `EPIPE`, `poll` and `select` per state, a pair made on a worker, dups and exec-close, the kqueue flags `EV_CLEAR` and `EV_RECEIPT` with T0's receipt rule, a knote dropped with its closed end, the default SIGPIPE ending a socket write with 141 and `MSG_NOSIGNAL` suppressing it, the refusals by value, a plain `sendmsg` among them, and a checkpointed seek into socket traffic; each mode's lines are native's), `blockio_e2e` (M55: a blocking read on an empty guest pipe or socket, a blocking write into a full one and a `recv` with `MSG_WAITALL` for more than is held end at one clean `Threads` wall line where they used to hang the recorder, with controls that complete; `O_NONBLOCK` per open file through `F_SETFL` and `FIONBIO`; every run is bounded so a hang fails), `sigwait_e2e` (M55: a handled signal to a thread blocked in a semaphore or `kevent`, process- or thread-directed, is delivered at the raise with native's return, `x0` 14 for the semaphore and `EINTR` for `kevent`, without changing the count or consuming an event, and a masked one rides through the wake; a terminating signal to a joining main ends the process at the raise; a handled signal, or SIGPIPE, to a condition-variable waiter is refused at the raise, and that SIGPIPE recording replays to the incomplete-recording line; SIGPIPE on a pipe reaches main, or the writer when main masks it, and with the default ends the process with 141; M17's own gate stays in `sigblocked_e2e`, unchanged; the `Mutex`/`Poll`/`Parked` refusals, the `F_SETNOSIGPIPE`/`SO_NOSIGPIPE` suppressions and the `F_SETFL` refusal are implemented and pinned by no test; `traps_e2e` pins a `brk` handler frame's `si_pid` at 0), `tokio_e2e` (M55: nine programs from a tokio `features = ["full"]` fixture crate at `crates/retrace-guest/tokio/`, built once with `--offline` — multi-thread and current-thread runtimes, a signal stream, a channel with `spawn_blocking`, `select!`, `fs`, and libtest's `tk_test` — each recorded and replayed twice against native; the multi-thread program that signals a thread blocked in `psynch_cvwait` is pinned as its refusal; a fixture build failure other than a missing cached crate must fail, not skip, and it skips loudly when the tokio crates are not in cargo's cache; `tk_kill_mt` is asserted on record only), `ipa40_e2e` (M57: a guest that reserves 24 GiB FIXED above 64 GiB and commits inside it, as 27's malloc does, recorded and replayed twice as native runs it, and a checkpointed seek into the committed range), `hostpin_e2e` (M57: the host record in the trace header, and another shared cache refused with exit 2 by name), `osgate_e2e` (M57: a macOS major other than 26 and 27 ends every command at one exit-4 line before any VM exists), `cargo_e2e` (M58: the repo-owned `ledger` crate's `cargo test` recorded through cargo's runner with `RETRACE_DIR` — one hashed trace per unit-test and `tests/` binary plus the doctests' `rust_out.retrace`, cargo's 101 passed through, the trace saved under the failing `Running unittests` line replaying to the same failure, and a passing filter exiting 0; skips loudly without cargo), `lldbbatch_e2e` (M58: the README's lldb form of the loop, `retrace lldb <trace> -- --batch -o …`, on the ledger recorded by the README's recipe word for word — the backtrace at `rust_panic` naming the test at its `assert_eq!` line, `assert_failed`'s `left=` leading a write watchpoint and `process continue -R` to the store in `fees::charge` on the same thread, the `{:p}` print agreeing with `left=`, lldb's batch mode dropping an `-o` after a crash stop while `-k` runs, file and line for `crashy.c` built with `-g` as C and as C++, and a check that the README shows the commands it runs; skips loudly without lldb, cargo or clang), `bt_e2e` (M58: `retrace debug`'s frame-pointer `bt` against lldb's over the same recording, frame pcs from frame 0 to the first frame of `main` or the test function, on `crashy`, at the ledger's panic and on a blocked thread; a mismatch fails, it never soft-skips; skips loudly without lldb), `lastwrite_e2e` (M58: `last-write` naming the ledger's bad store at its file and line with the value before and after and its JSON `function` the name alone with `offset` beside it, a second one walking back to the deposit, the inventory's store on the worker thread, a kernel write as `kind` `syscall` (`bigread`'s `read`), `no earlier write` leaving the cursor, the four-slot limit, `goto` in its three spellings, and a user breakpoint after the store not voiding `value_after`), `summary_e2e` (M58: `summary` and bare `retrace debug <trace>` — the ledger's one panic with its backtrace at the assert line, the operand addresses and the stdout tail, the left operand leading `goto`/`last-write` to the bad store, the text form, `crashy` as a crash with its fault address and, in text too, the thread, position and `#0` frame where it stopped, a passing run with no panics, and a caught panic listed while the run exits 0), `agent_e2e` (M58: the README's loop end to end on `ledger` and on `inventory` as separate commands — the cargo recipe, `retrace debug <trace>`, `goto` the panic, `last-write` its left operand — every `--json` line parsed, the inventory's writer a worker that is not the panicking thread with the spawned closure in its frames; its output under the SDD ledger's `readme-loop/` is where the README's transcripts come from), `agentdocs` (M58: the agent-facing docs read as source — every `retrace <command>` and `--script` verb in the README, the skill and `docs/agents-snippet.md` exists in `main.rs` or `debug.rs`'s parser, the skill's frontmatter, the snippet embedded in the README verbatim, and the plugin manifests agreeing)). Run one with
  `cargo test -p retrace --test <name> -- --test-threads=1`.
- Some gates are `#[ignore]`d, parked at a documented wall — see "Honest-gate discipline" below for
  the rule. Which ones and why is on the tests themselves (the `#[ignore]` reason is the primary
  record) and summarised under "Known limits" in `docs/current-state.md`.
- CLI: `cargo run -p retrace -- record [-o t.bin] <program> [<args>...]` (static or dynamic is read from
  the file; options go before the program; the trace defaults to `<basename>.retrace`, or
  `$RETRACE_DIR/<basename>.retrace` when `RETRACE_DIR` is set),
  `... replay t.bin`, `... lldb t.bin` (serves the recording and runs lldb on it),
  `... debug t.bin` (no script: the `summary` — how the run ended, each panic with its backtrace),
  `... debug t.bin --script '<cmds>'` (`;`-separated debugger commands; `--json` makes either form print one JSON
  object per command, one per line).
- `RETRACE_TRACE=1` on a `record` run logs every dispatched trap (and decodes
  `mach_msg2` sends) — the first tool to reach for on a bring-up failure. **Record-only:** `ReplaySession`
  carries no trace instrumentation, so no `[trap]`/`[mach_msg2]`/`[fault]` line is ever printed on replay.
- `RETRACE_VERBOSE=1` prints retrace's own diagnostic lines (fall-through counts, warnings), off by default since M51; `RETRACE_TRACE=1` implies it. Since M51 `util::run`, `run_env` and `run_bounded` start the child with an empty environment plus the test's own pairs, so a test that needs a variable passes it.

The toolchain is pinned (`rust-toolchain.toml`: 1.95.0, target `aarch64-apple-darwin`). The
`clippy.toml` denials are load-bearing, not style, and are **two separate rules**:
`Instant::now`/`SystemTime::now` for determinism (see below); `std::thread::Thread` because
retrace's core is single-threaded by design. The latter governs the *recorder's* threads, not the
*guest's* — a guest may be multi-threaded (see "Guest threads").

### Codesigning

`.cargo/config.toml` sets a cargo `runner` (`tools/codesign-run.sh`) that ad-hoc-signs the binary
cargo invokes with `retrace.entitlements` before running it. **But** a test that spawns a *separate*
binary itself (via `CARGO_BIN_EXE_retrace`) bypasses that runner, so it must codesign that binary by
hand first — see `crates/retrace/tests/util/mod.rs::bin()`. Copy that pattern for any new test that
spawns the CLI.

### Spikes

`spikes/*.c` are throwaway probes that empirically verified load-bearing HVF/SPTM/PAC claims on the
real OS before they were committed to the architecture. They are built and signed manually (binaries
are gitignored); build/run recipes and findings are in `spikes/README.md`.

## Architecture

### Crate graph (dependency order)

- **`hv-sys`** — bindgen FFI over Hypervisor.framework. Thin safe wrappers: `Vm`, `Vcpu`, `reg`,
  `sysreg`, `MemFlags`. `build.rs` runs bindgen against the macOS SDK.
- **`retrace-arch`** — zero-dependency arch facts: syscall numbers, `ec_of` (ESR exception-class
  decode), `decode_aut_rd` (PAC AUT* instruction decode), Mach-O/PSTATE constants.
- **`retrace-trace`** — the on-disk trace format: the `Event` enum (`Snapshot`/`Syscall`/`Exit`),
  `Writer`/`Reader` with a magic+version header and per-record CRC32. **Changing `Event`'s shape is a
  format break — bump `TRACE_MAGIC`.** **So is changing what a snapshot's bytes _mean_**: M23 changed
  the trampoline's vector padding without bumping, and a pre-M23 trace then restored its old padding
  under code that assumed the new — M24 bumped for it (to `RT\x00\x09` then; moved again by M38
  for `ret1`, by M47 for the `mach_vm_map` alignment mask, and by M49 for the store-exclusive
  rewrite and native's `si_code`, and by M51 for `Event::Crash`'s `sig`, and by M52 for the synthetic clock's stride, and by M53 for the iovec translation, and by M54 for `Event::Exec`, and by M55 for SIGPIPE on a guest pipe or socket end (a pre-M55 recording's `EPIPE` would replay with a signal its recording never took), and by M57 for the host record and the 40-bit IPA's page-table move, so every pre-M57 recording is unreadable and it is `RT\x00\x12` now).
  `open_checked` drops a torn/corrupt tail rather than panicking, and since M47 a trace whose header
  is another `RT` version is refused by name ("recorded by a different retrace trace format (not
  torn)") rather than as torn.
- **`retrace-guest`** — the Mach-O loader (`parse_macho`, `slice_arm64e`) **and** the guest test
  programs. `asm/*.s` (freestanding, `-nostdlib -static`) and `c/hello_dyn.c` are compiled by
  `build.rs` into `OUT_DIR`; path constants (`HELLO`, `HELLO_DYN`, …) point at them.
- **`retrace-box`** — the core. `Box_` is the VM: it builds guest memory, the W^X stage-1 page
  tables, and PAC state; runs the vCPU trap loop (`run() -> Stop`); forwards syscalls with
  memory-diff (`forward_and_diff`); applies recorded writes on replay (`apply_and_return`); hosts the
  shared-cache demand-pager + re-signer (`cache.rs`) and the in-guest PAC signing oracle. By far the
  largest, densest crate — start here.
- **`retrace-core`** — the record/replay orchestration: `record_box`/`replay` dispatch each `Stop`
  from the box, plus `machmsg.rs`, the pure `mach_msg2`/MIG codec + router.
- **`retrace-sim`** — deterministic `Rng` + fault injection for the seeded swarm (no deps).
- **`retrace`** — the CLI binary and all end-to-end integration tests.

### The record/replay model (retrace-core)

Record snapshots initial state, runs the guest, and on each trap forwards the real syscall to the
host kernel, diffs guest memory to capture what the kernel wrote, and appends an `Event`; on exit it
appends a final full-memory snapshot. Replay restores from the snapshot, runs the guest again, and
**never executes a syscall** — it verifies each trap's `(num, args)` against the recording (the
divergence oracle), applies the recorded writes, feeds the recorded return, and at exit does a
full-memory comparison.

**Determinism is the whole game.** Nothing nondeterministic may enter the trace. Anything that would
(shared-cache page contents, timing, PAC signatures) is instead *regenerated identically* on both
sides: cache pages are a pure function of (file bytes, fixed slide, fixed keys); the timebase is a
synthetic monotonic counter; PAC keys are fixed constants. That is why `clippy.toml` bans wall-clock
and threads.

**Two symmetry rules govern adding any new trap handler:**

1. A special case added to record's `match stop` needs a **mirror** in replay's dispatch, and both
   must recompute *identical* addresses/bytes. Both arms live in `crates/retrace-core/src/lib.rs` —
   record in `record_box`, replay in `ReplaySession::advance` — **not** in `retrace-box`, which owns
   only the `Box_` methods those arms call. A new arm goes in *both*, calling the *same* `Box_`
   method with the *same* arguments (that identity is what makes the rule hold by construction), and
   must sit *before* the generic forward arm. Replay byte-compares its recomputed reply against the
   recording — that comparison *is* the divergence check, so an asymmetry surfaces as a divergence,
   not silent corruption.
2. Deterministic instruction emulation is better done **below the trace**, inside `Box_::run()`
   (as with the timebase MRS, the Apple-IMPDEF undef-MRS, the B-family FPAC strip, since M48
   the per-thread SPRR register `S3_6_C15_C1_5` with the `MAP_JIT` view it selects, and since M49
   the stage-2 fault conversion, the A-narrow tables and the discarded-status store-exclusive
   rewrite, and since M51 the trap conversion (an EL0 brk, udf, hlt or misaligned PC or SP entering
   the guest's EL1 vector through enter_el1) and J1's translation fault, and since M53 the interval timer's fire, framed at a landmark finish or an idle jump with no `SignalDelivery` recorded): `run()` is
   shared by record and replay, so such an arm fires identically on both sides and never surfaces
   to the record/replay loop — determinism is then automatic.

   M42's store-exclusive emulation is also below the trace, but it is debugger-only: it lives in
   `step()` and `run()`'s pair prologue, engages only when single-stepping or at a debug stop, and
   record and plain replay assert its shadow is never set.

**Exec is one method, run by both sides (M54).** A successful `execve`, `SETEXEC` `posix_spawn` or `#!` script
is `Event::Exec`, which carries the path, the selected slice of the image (a script's interpreter) and of its
dynamic linker, a script's first line and the guest descriptors exec closed; argv, envp and a spawn's
attributes and file actions are read from guest memory on both sides, not recorded. Record and replay both call
`Box_::exec_image` with the same `ExecImage`, and replay never reads the new binary from disk, so symmetry
rule 1 holds by construction. Everything that can refuse (`read_exec_call`, `resolve`, the spawn-action
validation) runs **before the point of no return**, `exec_image`'s first line: past it a failure is a bug,
never a wall, so a new exec check belongs before it. `exec_epochs` splits a recording at its `Exec` events, an
exec's position is `(n, 0)` at the new entry, and the debugger resolves symbols, breakpoints and the crash
line in the epoch of the position. What exec keeps and resets is in `docs/superpowers/specs/2026-10-29-retrace-m54-exec-design.md` §3d.

**A guest-reachable refusal is a wall, never a plain panic (M50).** A box method refuses with
`Err(retrace_arch::wall(category, …))`, which `record_box` ends the recording on; a deep shared
helper with no error channel panics through `retrace_arch::refuse!(Category, …)`, and every
`refuse!` site is listed in `crates/retrace-box/tests/refuse-sites.txt`, which `refuse_audit.rs`
checks against the source. Decide the class at the site, never by matching message text afterwards:
internal asserts share prefixes with walls. Anything else that stops retrace is a bug, which prints
the bug banner and exits 70. Below the trace, a wall returns `Stop::Unsupported` from `run()` and
`step()`.

**A guest pointer is checked before it is used (M51).** A forwarded BSD call's pointer arguments go
through `Box_::prepare_forward_pointers` first, which answers EFAULT for memory the program could
not reach natively and makes a passing pointer's pages present. Record calls it in the generic arm
and in the three arms whose landmarks replay finishes in its generic tail (the console write,
`sigaction`, `sigaltstack`); replay calls it at the same points. `sigpending`'s out-set and
`sigprocmask`'s oldset are written only if `prepare_reply` passes and return 0 either way, as
natively (`retrace_arch::ignores_bad_out_pointer`). A new arm whose landmark
falls through to replay's tail and whose call has a pointer in its row must call it too. An
emulated call reads through `read_va_checked` and checks a reply with `prepare_reply`. A failed
`read_guest` or `write_guest` is retrace's bug, and every call of an unchecked accessor is listed in
`crates/retrace-box/tests/guest-ptr-sites.txt`, which `guest_ptr_audit.rs` checks against the source.

### Hard platform invariants (encoded in the box; violating them hangs or panics the machine)

- **W^X.** Executing a *writable* guest page hangs the vCPU on Apple Silicon. Code pages are RO+exec
  (`ATTR_CODE`), data is RW+non-exec (`ATTR_DATA`). Runtime data→exec promotion (`set_region_exec`)
  is sound with no further work only on a block the guest has never translated; on a block it *has*
  translated the stale RW/UXN entry must be invalidated first. The VMM cannot issue a guest TLBI, so
  **M9 has the guest issue it**: `flush_guest_tlb` runs `tlbi vmalle1` on the guest vCPU at EL1 from
  a scratch page (`ATTR_TRAMP` — `ATTR_CODE` sets PXN and `tlbi` is EL1-only), using the PAC signing
  oracle's save/restore discipline. `place_fixed` promotes-then-flushes on the FIXED-exec-over-live-
  backing path (dyld's non-cache-dylib strategy). Non-FIXED exec mmaps are still placed in fresh
  32 MiB-exclusive blocks — now an optimisation (a flush avoided), no longer a correctness rule.
  Since M48 a `MAP_JIT` range is the one region whose stamp changes at run time: the running
  thread's SPRR value selects `ATTR_CODE` or `ATTR_DATA` for its pages, never both, and the flip
  flushes through `flush_guest_tlb`, which disarms `MDSCR_EL1.SS` and `MDE` around its stub so a
  single step can cross it.
- **SPTM / anon-only memory.** A *file-backed* `hv_vm_map` hard-panics macOS 26
  (`VIOLATION_ILLEGAL_MAPPING_TYPE`). All guest memory is anonymous; file bytes (the shared cache,
  file-backed mmap) are staged via `pread` into anon pages and, on record, captured as writes.
- **One VM per process** → `--test-threads=1`.
- **Drop order.** `Box_`'s field declaration order is load-bearing: `vcpu` must be declared before
  `vm` (HVF requires `hv_vcpu_destroy` before `hv_vm_destroy`). Don't reorder the struct fields.
- **Never reimplement Apple's PAC.** The box signs/authenticates by running `pac*`/`aut*` on the
  guest vCPU itself (the signing-oracle stub in `retrace-box`), with fixed keys identical on both runs.

### MMU / PAC / shared-cache stack (M2 and its sub-milestones)

The dynamic path (`load_dynamic`) turns the MMU on with a **47-bit guest VA** (`TCR_EL1.T0SZ=17`, a
3-level 16 KiB-granule walk `TTBR0→L1→L2→L3`; IPA is 40-bit since M57, a 16-page L2 array at `PT_L2_IPA` 0x80_0000), enables PAC with fixed keys, loads a
real arm64 Mach-O plus `/usr/lib/dyld`, and builds dyld4's process-start stack. The arm64e **dyld
shared cache** is bound to the host process's PAC keys, so the box can't reuse the live cache: it
**demand-pages** each cache page from the on-disk file and **re-signs** every auth pointer with the
guest's keys (`cache.rs` `walk_page` + `Box_::sign_slots`). The v5 slide-info format only expresses
A-family (IA/DA) keys; B-family cache auths that FPAC-fault are handled by strip-on-FPAC emulation
(`try_emulate_fpac_auth`). The fixed guest IPA layout (trampoline, stack, page tables, TSD, commpage,
sign scratch, nano band, mmap base, shared-region window, which is the cache's own since M57) is defined and explained in the constants
at the top of `crates/retrace-box/src/lib.rs`.

### Guest threads

A guest may be multi-threaded even though retrace's core is not. `bsdthread_create` is **emulated in
the box, never forwarded**: `Box_` holds a thread table of register contexts, and a cooperative,
block-driven scheduler switches only when a thread blocks or exits. That choice is a pure function
of the guest's own syscall sequence, so record and replay produce identical schedules with **nothing
recorded** and no trace-format change — symmetry rule 2 doing its job. Since M41 a replayed
boundary shows the thread that runs next: `finish_event` settles the switch
(`Box_::settle_schedule`) instead of leaving it to the next `run()`/`step()` entry. The switch
reuses the save/restore discipline the PAC signing oracle and `flush_guest_tlb` already
established. A thread blocks for **eleven** reasons (`BlockReason`: `Join`, `Wait`, `Sem`, `Parked`, `Kevent`, `Cv`, `Mutex`, `Sleep`, `Yield`, `Poll`, `SigSuspend`), and the correlation key is not the same for
all of them: `__ulock_wait` (515) / `__ulock_wake` (516) are correlated by **address equality**
on `pthread + 0x34` — measured in both `__pthread_join` and `__pthread_joiner_wake`, so no
address→thread-index mapping is needed; the mach semaphore pair `semaphore_wait_trap` (`-36`) / `semaphore_signal_trap`
(`-33`) is correlated by **port name**, since what that trap carries is a name in retrace's own IPC
space and never a guest address (M18 Stage 2b); and a workqueue worker parked at `workq_kernreturn`
opcode `0x4` (`BlockReason::Parked`) has **no waker at all** — libpthread `brk`s if that call ever
returns. The event manager, parked at opcode `0x40` since M46, is the one exception: a knote
activation re-enters it (`ThreadTable::unpark`). Since M48 a thread also blocks in `kevent` on a
guest kqueue (`BlockReason::Kevent`), woken by a knote activation or its deadline, and in
`psynch_cvwait` (`BlockReason::Cv`), correlated by the condition variable's **guest address** and
woken by `cvsignal`/`cvbroad` or its deadline. Since M49 a thread also blocks in
`__semwait_signal`'s relative-timeout shape (`BlockReason::Sleep`), correlated by **thread** (every
sleeper shares libc's one `clock_sem`) and woken only by its deadline. Since M52 `Sem` carries an optional deadline and the box keeps the
kernel's semaphore count (`Semaphores`, `semaphore_timedwait_trap` -38), `Yield` (`swtch_pri` -59 and
`thread_switch` -61) has no waker and ends in the settle that blocked it after one clock stride, and
`Mutex` is a contended `psynch_mutexwait` keyed by the mutex's **guest address** and woken by
`psynch_mutexdrop`, first waiter first. Since M53 a thread also blocks in `poll`, `select` or `pselect` over
guest pipes (`BlockReason::Poll`, an optional deadline; the descriptors and the reply's address live in
`gkq`, and it is woken by a pipe change that makes one ready or by its deadline), and in `sigsuspend` (`pause`
is libc's `sigsuspend` of the current mask) as `BlockReason::SigSuspend { saved_mask }`, never timed, woken
only by a signal the call's mask lets through, which runs its handler and returns `EINTR`; with no signal
source left it is M50's deadlock wall. Every deadline is on
`synthetic_tsc`,
`schedule_after_block` jumps to the earliest of all of them, and again while a jump wakes nobody (M53's timer, up to `IDLE_JUMPS_MAX`), and a woken thread's
return is written at the wake (`deliver_wake`). Forwarding `bsdthread_create` would be not merely
wrong but whole-process fatal (the host would start a real thread on retrace's own `_pthread_start`,
which PAC-fails on the guest's pthread struct) — and **nothing asserts against it**: the emulating
arm in `record_box` (`crates/retrace-core/src/lib.rs`) sits before the generic forward arm and that ordering is
the only guard. The generic arm's asserts are `is_signal_syscall`, the workq pair, `kevent_qos`
(M45), since M52 `thread_selfid` (372), `writes_via_nested_pointer`, since M47 `madvise` and `fork`, since M48 `kevent` (363),
since M53 the poll family (`is_poll_family`), `setitimer` and `getitimer`, and the four iovec calls (`is_iovec`), since M54 `execve` and `posix_spawn` (`num != 59 && num != 244`: a child spawn is a wall and an exec goes to its own arm, so no exec reaches the generic arm),
and every SDK psynch number (`retrace_arch::is_psynch`), and since M49 `__semwait_signal` (334)
and its `_nocancel` twin (423) only; since M50 both generic arms call
`retrace_arch::refusal_before_forward`, which ends the recording at a wall for an unrowed number, a
nested-pointer write or an unmodelled semaphore trap. In the mach arm it is the first statement, so
the M18 semaphore assert and `forwarded_shape`'s panic are backstops no guest reaches; in the BSD arm
it sits after the twelve "never forwarded" asserts (they guard arms that must exist) and before the M27
assert, which with `forwarded_shape`'s panic is the backstop there; the claim that it
"asserts" stood here from M14 to M37 and was measured false at M37. Since M45 libdispatch's
workqueue-kqueue init, `kevent_qos` (374) with `KEVENT_FLAG_WORKQ`, is emulated beside the workq
pair for the same reason (forwarded, it acts on retrace's own workqueue kqueue): M45 emulated
exactly one measured shape, returning 0
(`Box_::guest_kevent_qos`), and refused every other by value, naming the field.

**A guest socket pair is a guest object, like a guest pipe (M55).** `socketpair` (135) with a stream pair
in the local domain is forwarded on record and numbered by retrace: `forward_and_diff` binds the two host
descriptors and writes the guest numbers over the recorded `sv`, and `note_fd_effects` reads `sv` back on
**both** sides and calls `gkq.socket_pair` (one call site, so symmetry rule 1 holds by construction; Ruling
T1-a). `FdKind::Sock` (`gkq.rs`) carries the end's model (`Socks`: peer, unread count, open count, per-end
`nonblock` and `nosigpipe`), and, like a pipe's, its bytes are **counted from each call's own return**,
never read from the kernel, so readiness is a pure function of the guest's calls and `poll`, `select` and
the kqueue filters (`EV_CLEAR` as an edge queued once per activation, `EV_RECEIPT` by T0's abort-at-error
rule; `EV_DISPATCH` still refused) agree on both sides. A count the model has lost refuses by value. Every
other socket call, a `SOCK_DGRAM` pair, `shutdown`, `sendmsg` (any, since its msghdr iovec is not translated), `sendmsg_x` and the buffer-size options are
walls named by `Box_::socket_refusal`, which record calls ahead of `refusal_before_forward`; a read or write
that **would block** (`Box_::would_block`: the end is blocking, the other side open, and the call cannot
complete from the model's counts; a pipe's capacity here is 65536, T0-blockwrite, apart from the 16384 of
its kqueue write data) is a `Threads` wall before any forward on both sides, where it used to hang the
recorder. These two are walls, not asserts: the generic arm's list of "never forwarded" asserts did not
change.
Since M46 the box also models libdispatch's **event manager**: `kevent_qos`'s memory-pressure
registration and manager poke join M45's init, the manager's `KEVENT_RETURN` (`0x40`) registers
UPTIME timers, and `schedule_after_block` fires them on `synthetic_tsc` — overdue timers first,
then, with nothing runnable, one jump of the clock to the earliest deadline. All of it is box state
(`Box_::kq`, carried in `BoxState`), rebuilt from the guest's own syscalls on both sides, so nothing
new is recorded. The recorder does rewrite one value: `gettimeofday`'s mach-time out-parameter
becomes the guest's own clock (M46 R7), because libdispatch reads its timer "now" through
`mach_get_times`, whose commpage path always falls back to 116 under a frozen commpage.

**Emulating a syscall's entry contract is not the same as emulating the syscall.** Besides the new
thread's registers, `guest_bsdthread_create` must reproduce what the *kernel* writes on the way
through, or `pthread_join` returns success without ever waiting: the child's mach port at
`pthread + 0xf8`, `TPIDRRO_EL0 = pthread + 0xe0` (the TSD sits *inside* the struct), and
`PTHREAD_START_TSD_BASE_SET` in `w5`, which `__pthread_start` `tbz`-tests and `brk`s on. Each is
documented with its measurement at the call site.

**Signals are per-thread too (M16–M17).** The signal path resolves `__pthread_kill`'s target port to a
thread — `thread_of_port` reads `[pthread + PTHREAD_KPORT_OFF]` back out of guest memory, so main
needs no special case — and `deliver_signal_to` builds the frame into *that thread's saved context*
rather than off the live vCPU. Masks, pending sets and alternate stacks live on `Thread`
(`crates/retrace-box/src/thread.rs`) because POSIX makes them per-thread; **dispositions stay
process-global** on `SigTable`, because POSIX makes those per-process. A signal pends when the target
cannot take it yet, for **two** independent reasons with **two** matching materialisation sites, both
of them syscall landmarks so that both dispatch loops can see them — the same argument that keeps
delivery above the trace — plus, since M18, a **third** blocked state that pends but deliberately
does *not* materialise:

- **Masked (M16)** — materialised at the *calling* thread's next unmasking
  `sigprocmask`/`pthread_sigmask`.
- **Blocked (M17)** — a target blocked in `__ulock_wait` cannot be redirected into a handler, because
  its saved context is the resume point its blocking syscall owes a return through. The signal is
  materialised at the `__ulock_wake` that makes the thread runnable, onto the **woken** thread. A
  signal pended for both reasons is released only when both have cleared. A signal to a thread
  nothing ever wakes is therefore never delivered; `assert_no_stranded_signals` fails loud at a clean
  exit rather than swallowing it.
- **Parked on a semaphore (M18)** — the exception, and it **aborts** rather than materialising.
  *Since M55 a handled signal no longer reaches this state:* `classify_raise` interrupts a `Sem` (and
  `Kevent`) waiter at the raise, so the `-33` arm's wall (`crates/retrace-core/src/lib.rs`, "carrying a
  pending deliverable signal") is a backstop for a signal pended while masked and unmasked before the
  wake; a masked one rides through the wake and stays pending. What follows is why the wake never
  materialises.
  M17's materialisation applies a *measured* correction to the woken thread's saved context
  (`blockedctx.rs`: saved `x0` 0, saved SPSR left C-set for a `__ulock_wait`-blocked thread). Nothing
  has measured the equivalent for a thread parked in `semaphore_wait_trap`, so the `-33` arm asserts
  and names the measurement it owes, on both sides identically. Dropping the wake silently was the
  alternative and is the one failure a determinism oracle **cannot** see: record and replay would
  agree with each other while the signal vanished.

**A raise is classified once, by disposition and then by state (M55).** `classify_raise` (`retrace-box`,
`lib.rs`) answers, in order: a signal masked by the target **pends** (M16's order, so a masked `SIG_IGN`
still pends); an `Ign` disposition, or `Dfl` with an ignore default, is **discarded at the raise**, and `Dfl`
with a terminate default **ends the process at the raise**, in every state of the target (natively no
thread needs to run for either; before M55 a blocked target pended it and then walled); a handler then
goes by state: `Sem`, `Kevent`, `Sleep` and `SigSuspend` **interrupt** (the wait ends with native's return,
which the handler's frame is built on: `x0` 14 for a semaphore, `x0` 4 with C set for `kevent`, `EINTR` for
the other two; the waiter leaves its FIFO or kqueue slot without changing the count or consuming an event);
`Join`, `Wait` and `Yield` **pend** as before; `Cv`, `Mutex`, `Poll` and `Parked` are **refused at the
raise** (`RaiseKind::Refuse`, `M55: a handled signal (…) to thread N, blocked in …`), because the kernel's
`0x104` return for a condition variable or mutex carries a sequence-word effect retrace does not model.
A `kill`'s refusal comes before record appends anything, so replay meets no landmark for it; a SIGPIPE's comes after the write's landmark, so replay reaches `mirror_raise`'s `Refuse`, a marked wall that `end_or_diverge` reads as the incomplete recording's end (Ruling F-2); the timer reaches it through `refuse!`
(listed in `refuse-sites.txt`). Both dispatch loops run the same arm through one shared function per side
(`record_raise`, `mirror_raise`), so nothing new is recorded for the interruption itself.

**A process-directed signal goes to the thread xnu picks (M53).** `kill(getpid(), sig)` and the interval
timer's `SIGALRM` go to the first live thread, main first, that does not mask the signal; if every thread
masks it, it pends on main and is taken when main unmasks. Measured (t0 §M8 and its Correction), and it
replaces the earlier rule that retrace delivered it to the caller. `__pthread_kill` names its thread and is
unchanged. A signal also ends a `__semwait_signal` sleep, whoever sends it, and `sigsuspend` is woken by one
the call's mask lets through. Since M55 SIGPIPE is one too: a write-family call that returned `EPIPE` on a guest pipe write end or socket end (`Box_::sigpipe_due`, read from the descriptor's kind, not an effect) raises it at the write's landmark, `si_code` 0 and `si_pid` the guest's pid, unless `F_SETNOSIGPIPE` or `SO_NOSIGPIPE` (T0-nosigpipe) or the call's `MSG_NOSIGNAL` (Ruling F-1) suppressed it; the recorder's own pid cannot be recomputed by a replay process, so `mirror_delivery` takes those four `si_pid` bytes from the recorded frame, only for a recorded SIGPIPE delivery whose `si_code` is 0, and cleared after it (a `brk`, also `si_code` 0, keeps native's `si_pid` 0; Ruling F-4) (Ruling T4-a, the one value replay takes from the recording verbatim, as M23's `ServiceGetSpecialPort` does). `verify_thread` stayed at seven through M53: the poll, `sigsuspend` and `setitimer` mirrors sit
inside the `Syscall` chain after its arm-top check, and `thread_oracle.rs` retags one landmark of each.

**The divergence oracle checks thread identity.** Every landmark variant carries a `thread` tag —
`Syscall` since M15, and `Exit`/`Crash`/`Signal`/`SignalDelivery` since M16 (which bumped
`TRACE_MAGIC` for it; the magic is **now** `RT\x00\x12`, moved again by M24, then by M38 for
`ret1`, by M47 for the `mach_vm_map` alignment mask, by M49 for the store-exclusive rewrite and
native's `si_code`, by M51 for `Event::Crash`'s `sig`, by M52 for the synthetic clock's stride, by M53 for the iovec translation, by M54 for `Event::Exec`, and by M55 for SIGPIPE on a guest end, and by M57 for the host record and the 40-bit IPA's page-table move, so every pre-M57 recording is unreadable)
— and replay recomputes the current thread and compares it. `verify_thread` has **eight** call
sites (M54's `Event::Exec` mirror is the eighth, and M55 added none: its socketpair, SIGPIPE and would-block mirrors sit in the generic `Syscall` tail or in `mirror_raise`'s existing kill-mirror site, and `BlockReason` is still **eleven**; `thread_oracle.rs` retags an exec landmark made from a worker), one in each arm that consumes a landmark and `return`s, each placed *after* that arm's own
field comparison so a genuine argument divergence still reports as itself; the `SignalDelivery`
landmark is checked by a further inline comparison in `mirror_delivery`, deliberately not
`verify_thread`, because its tag is the **receiving** thread rather than the current one. M48's
`kevent` and psynch mirrors sit inside the `Syscall` chain after its arm-top check, so the count
stayed seven; `thread_oracle.rs` retags one landmark of each. M49's 334 mirror and 1000 route sit
inside the same chain; `thread_oracle.rs` retags one landmark of each. M51's abort_with_payload
terminal reaches the kill mirror's Signal consumption through ReplaySession::signal_terminal, which
holds that site, so the count stayed seven then; thread_oracle.rs retags a payload landmark. That
count is the thing to check when adding an arm: each site exists because a mirror `return`s
before reaching the generic dispatch, so **every new mirror silently creates a new hole until its
oracle call is added** — nothing structural couples the two. Without
the check, two threads running the same code issue byte-identical `(num, args)` and a wrong-thread
replay continues in silence. `Event::Sched` is **gone**, not reserved: emitting it would silently
renumber every landmark, and nothing in either dispatch loop can see a switch. **All eight sites are
exercised**, the last of them only since the M18 fast-follow: `Crash` had no fixture, because every
crashing guest was single-threaded and every threaded guest exited cleanly. `crashthread` (C,
threaded AND fatal — main blocks in `pthread_join`, the child faults with no handler, so the
terminal `Event::Crash` carries a NONZERO tag) is that missing intersection, and `thread_oracle.rs`
retags it. That test was verified able to fail, and what it caught is the quiet kind of failure:
with the `Crash` arm's `verify_thread` deleted, replay **accepts** the wrong-thread trace and exits
139 — the identical outcome a correct replay produces, so nothing outside the oracle can tell them
apart. (M14-threads, M15-threaddebug, M16-threadsignal, M18-workq; see their specs and
`docs/status-log.md`.)

## Milestone / SDD workflow

Development is milestone-driven, M0 onward. Each milestone has a design spec in
`docs/superpowers/specs/` and a task plan in `docs/superpowers/plans/` — both date-prefixed and named
for the milestone, so `ls` them for the current list rather than trusting a list written here.
Per-task reports and code-review diffs land in `.superpowers/sdd/`.

**Two documents, two jobs, and they must not be merged.** `docs/current-state.md` says what is
true *now* — capability, limits, gate. It is **edited in place** as reality changes, so it never
needs a "superseded" note. `docs/status-log.md` is the **append-only history**: a closing milestone
adds a new section to it and never rewrites an old one, so an earlier claim that later proved wrong
is left standing with a forward pointer rather than quietly corrected.

At a milestone close you therefore touch **both**: append the new Status section to the log, and
**edit** `docs/current-state.md`'s "What works today" / "Known limits" so they describe the new
reality. Then check the **README**, the short public summary: if the milestone changed anything
it states (a limit in its "What it can't do" section, a headline capability, the Apple-sweep figure, the
performance table), edit it to match; otherwise leave it alone. It carries no milestone history
and no measurement detail. Do not restate any of these in this file — a copy is a copy that goes
stale.

## Honest-gate discipline

A headline end-to-end gate is parked `#[ignore]`d at the current wall, with the wall documented
honestly, rather than being faked green or deleted. When you clear a wall, move the gate forward and
rewrite that documentation — the test's `#[ignore]` reason, "Known limits" in
`docs/current-state.md` (and the README's "What it can't do" section, if the wall is one it names), and the new
section you append to `docs/status-log.md`. If
nothing is left to park it at, un-`#[ignore]` it and say so. A milestone that parks a *new* gate for
a capability it does not yet have has regressed nothing; that is the discipline working, not a
backslide.

Two rules these gates taught, which bind their successors:

- **Never assert on an exit code a weaker failure would also produce.** An *uncaught* fault exits 139
  exactly like a caught-then-fatal one (`crashy_e2e` asserts precisely that), so `segv_rust_e2e`
  asserts on the *trace* instead — the `SignalDelivery` to the handler libstd actually installed, the
  `sigreturn`, the terminal `Event::Crash`, the `resume_pc`. `protnone_rust_e2e` sharpens it: it
  asserts DFSC `0x0f` (permission) rather than `0x04..=0x07` (translation), because only the
  permission fault is the thing its milestone created. **Assert on the difference your work makes.**
  Per-test specifics belong in comments in the test file, next to the assertion they explain.
- **A skipped test must announce itself.** `jq_e2e` / `jq_file_e2e` depend on
  `/opt/homebrew/bin/jq`, which is not a repo artifact; they skip with a `SKIPPED` line rather than
  passing quietly. A silent skip reads as a green it did not earn.
  **Where that line goes matters.**
  libtest captures `eprintln!` in a *passing* test, and a skip passes, so an `eprintln!` skip line
  never reaches a gate log (measured at M43's close). Since M44 every skip writes past the capture:
  `crates/retrace`'s tests through `util::announce` (`crates/retrace/tests/util/mod.rs`), and a
  test in another crate, which cannot reach that module, through a local `announce` with the same
  body (`retrace-core`'s `machmsgband_dyn` has one). `skiplines.rs` fails the gate if any `.rs` file
  under any crate's `tests/`, searched recursively, calls `eprintln!` with `SKIP` anywhere in its
  first string literal. It reads only that literal: a skip line built from a variable, or in lower
  case, slips past it.

One distinction that is easy to get backwards when writing such a test: a signal the guest **raises**
is `Event::Signal`, while one derived from a **hardware fault** whose disposition is not a handler
stays on the `Event::Crash` path.
