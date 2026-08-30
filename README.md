# retrace

A record/replay reverse debugger for Apple Silicon.

`retrace` runs a guest binary inside a single-vCPU Hypervisor.framework VM and records every
syscall and trap it takes, along with the memory the kernel wrote back through it. It then replays
that run **bit-for-bit** from a snapshot — never re-executing a syscall, only re-applying recorded
effects. Because replay is deterministic, execution can be driven *backwards*: seek to any point in
the run, set a watchpoint on an address, and ask which instruction — and which **thread** — last
wrote it.

It runs real programs, not toys: a full-`std` Rust binary, stock `brew jq`, a guest that spawns
threads, one that `dispatch_async`es onto a GCD queue, and — since M22 — most of the Apple binaries
already sitting in `/bin` and `/usr/bin`, arm64e and PAC and all.

```
$ retrace record-dyn ./mytool -o t.bin
$ retrace debug t.bin --script 'continue; watch 0x100008008 4; reverse-continue; where; regs'
hit watch 0x100008008 (write at 0x1804fb520) at (244, 242)
at (244, 242) pc=0x1804fb520 thread=0
x0 =0x0000000100008000  x1 =0x000000010000059c   …
```

That is a program run to completion, then run *backwards* to the instruction that last wrote a
corrupted word — with the thread that did it named.

Determinism is the whole design constraint. Nothing nondeterministic is allowed into the trace.
Anything that would be (shared-cache page contents, timing, PAC signatures, the thread schedule) is
instead *regenerated identically* on both sides rather than recorded. A divergence oracle compares
the two runs at every landmark and fails loudly on the first mismatch.

- **What runs today**, and what does not, is in "What works today" and "Known limits" below. Those
  are edited in place as reality changes, so they are the ones to trust.
- **How it got here**, milestone by milestone, is in [`docs/status-log.md`](docs/status-log.md) —
  append-only, and historical by design: each entry is true as of its own milestone, not today.
- **Design specs and task plans** are in `docs/superpowers/specs/` and `docs/superpowers/plans/`.

## Requirements

- **macOS 26.x on Apple Silicon.** Not optional — the box depends on macOS 26 SPTM and libpthread
  behaviour that was measured on it.
- Runs **non-root**; SIP may stay enabled.
- Every binary touching `hv_*` needs the `com.apple.security.hypervisor` entitlement. It is
  ad-hoc signable, so no developer account is required.
- Rust toolchain is pinned by `rust-toolchain.toml` (1.95.0, target `aarch64-apple-darwin`).

## Build

```sh
cargo build
```

Codesigning is automatic for anything cargo runs: `.cargo/config.toml` sets a cargo `runner`
(`tools/codesign-run.sh`) that ad-hoc-signs the binary with `retrace.entitlements` first.

**One exception matters.** A test that spawns a *separate* binary itself (via
`CARGO_BIN_EXE_retrace`) bypasses that runner and must sign it by hand — see
`crates/retrace/tests/util/mod.rs::bin()`, which signs a pid-unique copy rather than the shared
binary. Copy that pattern for any new test that spawns the CLI.

## Usage

```sh
retrace record     <macho> -o <trace>                  # freestanding static guest
retrace record-dyn <exe>   -o <trace> [-- <args…>]     # real guest through /usr/lib/dyld
retrace replay     <trace>                             # replay + verify against the recording
retrace debug      <trace> --script '<cmds>'           # reverse-debug a recorded trace
```

In development, invoke through cargo so the codesigning runner applies —
`cargo run -p retrace -- record-dyn <exe> -o t.bin`.

`RETRACE_TRACE=1` on a `record`/`record-dyn` run logs every dispatched trap and decodes `mach_msg2`
sends. It is the first thing to reach for on a bring-up failure. **Record-only** — `ReplaySession`
carries no trace instrumentation, so no `[trap]` line is ever printed on replay.

`replay` exits **3** on a divergence, naming the landmark, PC, and what mismatched.

### Debugger commands

Passed to `debug --script`, semicolon-separated:

| Command | Effect |
|---|---|
| `continue` / `reverse-continue` | run forward / backward to the next stop |
| `stepi [n]` / `reverse-stepi [n]` | single-step forward / backward |
| `break <addr\|symbol>` / `delete <addr\|symbol>` | set / clear a breakpoint, by address or by name |
| `watch <addr> [len] [thread <n>]` | watch a write, optionally scoped to one thread |
| `unwatch <addr>` | clear a watch |
| `where` | current landmark coordinate `(N,K)` and owning thread |
| `regs [tid]` | registers — of the current thread, or of a named (possibly blocked) one |
| `threads` | list every thread with its state, marking the current |
| `x <addr> [len]` | examine guest memory |

## What works today

**Guest breadth.** In short: anything you compile yourself (C or Rust), stock Homebrew arm64
binaries, and — since M22 — most of the Apple binaries already on your machine. Each rung below
records and replays byte-identically, twice:

| Rung | Guest | Notes |
|---|---|---|
| 0 | freestanding `-nostdlib -static` arm64 | 36 `asm/*.s` fixtures |
| 0 | `hello_dyn` (C) | real dynamic linking through `/usr/lib/dyld` |
| 1 | `hello_rust` | full-`std` `rustc` binary |
| 2 | `jq` | stock `brew` binary |
| 3 | `jq` + a file argument | |
| 4 | `threadrust` | `std::thread::spawn` + `join` |
| 5 | `dispatch_dyn` (C) | `dispatch_async` onto a global concurrent queue, joined by a `dispatch_semaphore` |
| 6 | `/bin/echo` | an **Apple system binary**, arm64e with PAC on, straight from `/bin` |

**Apple's own binaries, measured.** Sampled across `/bin` + `/usr/bin`, pointing retrace straight at
each file: **34 of 54 record and replay** — stdout byte-identical and exit codes equal. Among them
`cat`, `ls`, `cp`, `mv`, `rm`, `chmod`, `mkdir`, `ln`, `df`, `grep`, `wc`, `uname`, `sh`, `dash`,
`expr`, `bzip2`. Before M22 that number was **zero**, and not for the reason it looked like: every
macOS system binary is a *universal* file whose first four bytes are `0xcafebabe`, and the loader
asserted `MH_MAGIC_64` against them. retrace could always run Apple's binaries; it could not open
them. See Known limits for the 20 that still fail, which are four named causes rather than a tail.

**Capabilities**

- **Reverse execution** — `(N,K)` landmark seeks, checkpointed for ~800× faster backward seeks.
- **Watchpoints** — hardware `DBGW` (pre-retire) plus software detection, with
  reverse-continue-to-last-writer, thread-attributed.
- **Crashes are first-class** — a faulting guest is recorded, replayed, and seekable;
  reverse-continue reaches the corrupting store.
- **Symbolicated addresses** — since M19, pc-bearing debugger output names the function it is in:
  `guest crashed: pc=0x10000050c far=… esr=…  in _child+0x30`. The names are read from the
  recording's own opening snapshot, because `__LINKEDIT` is mapped into guest memory and the
  snapshot captures every backing — so **no binary path is supplied and no trace-format change was
  needed**, existing recordings gained symbols retroactively, and a stale-binary mismatch is not
  merely avoided but unrepresentable. The main executable and dyld resolve; see Known limits for
  where it stops.
- **Symbol operands** — since M20, `break _main` and `delete _main` accept a name wherever they
  accept an address, so the name the debugger just printed is a name it will take back. Resolution
  happens when the command *runs*, not when it parses, because parsing completes before the trace is
  opened and the symbol table does not exist yet. Name → address is **not** a function — a real
  `threadrust` binds 19 names to more than one address, and one dyld name carries 13 — so an
  ambiguous name is an **error listing every candidate address**, never a silent pick; a name that
  matches nothing is an error, never a fallback to reinterpreting the token as hex. A token that
  parses completely as hex stays an address, which is what keeps every existing debug script working
  verbatim.
- **Signals** — dispositions, handlers that actually run, `sigreturn`, alternate stacks, masks and
  pending sets. Per-thread since M16: `pthread_kill(child, SIGUSR1)` runs the handler on the child.
  Since M17 the child may be **blocked** in `__ulock_wait` when it is signalled: the signal pends and
  is materialised at the wake that makes the thread runnable.
- **Threads** — emulated `bsdthread_create`, a cooperative block-driven scheduler, and a divergence
  oracle that checks thread identity on every landmark. Every one of the oracle's eight checks now
  has a test that retags a real recording and proves it fires; the last of them, on the terminal
  `Crash` landmark, needed a guest that was threaded *and* fatal, which nothing in the tree was until
  `crashthread`.
- **libdispatch / GCD** — since M18, a guest that `dispatch_async`es onto a global concurrent queue
  runs, records and replays. `workq_open` (367) and `workq_kernreturn` (368) are emulated in the box
  and never forwarded; `REQTHREADS` builds the worker thread *inside* the VM and enters it at the
  guest's own registered `wqthread`; and the mach-semaphore pair — `semaphore_wait_trap` (`-36`) and
  `semaphore_signal_trap` (`-33`), which is what `dispatch_semaphore` actually lowers to — is a
  park/wake seam keyed on the port name. All of it is below or symmetric across the trace: nothing
  new is recorded and `TRACE_MAGIC` did not move.
- **Apple's own system binaries** — since M23, `/bin/date`, `bash`, `zsh` and `cal` record and
  replay. The wall was one unserviced `mach_msg2`: `host_get_special_port` (`msgh_id` 412), which
  **17 of the 20** M22 failures collapsed onto once the loader defect below it was fixed. It is
  forwarded and recorded rather than synthesized, because the reply carries a host-minted port name
  that is nondeterministic by construction — the `task_self` posture, and a documented exception to
  symmetry rule 1 rather than a drift from it. A message-queue send (the XPC pipe proper) is
  refused deterministically, both sides recomputing the identical refusal.
- **The trampoline's vector padding traps rather than undefs.** Each of the 16 EL1 vector slots is
  0x80 bytes of which only the first 4 held `hvc #0`; the remaining 0x7c were zero, which decodes as
  `UDF #0`. Execution that ran past a slot head then executed that `UDF` **at EL1**, overwriting
  `ELR_EL1`/`SPSR_EL1` and destroying the original exception's identity — reported as the notorious
  `pc=0x4204`, an address inside retrace's own trampoline with nothing to do with the guest. The
  padding is now `hvc #1`, so a fall-through is distinguishable at the VM exit, **counted**, and
  compared across record and replay by `fallthrough_e2e`. That single misattribution accounted for
  13 of M22's 20 failures, and it was never a capability wall.
- **A deep recursion reaches its own stack guard page.** Since M21, retrace *reserves* the stack the
  guest believes it has — macOS 26's libpthread reports a constant `0x7fc000` main-thread size that
  retrace cannot influence, so libstd installs its overflow guard 7.72 MiB below where retrace's
  256 KiB backing actually ends. The window `[0x2008000, 0x27C0000)` is reserved but unbacked, and
  `commit_reserved_page` grows into it one zeroed page per stage-2 fault, so a recursion walks all
  7.72 MiB down. The guard page is deliberately left **outside** the reservation, so it stays a
  backed `PROT_NONE` page that faults at **stage 1** and routes to libstd's handler as a signal:
  measured `far=0x2007f30`, inside the guard page, `DFSC 0x0f` — a *permission* fault, where before
  M21 the same run died on a *translation* fault at `far/ipa=0x27bff60 (UNMAPPED)`, 7.72 MiB away.
  Nothing about the reservation enters the trace; `Box_::restore` re-establishes it so replay starts
  from identical state.

**Gate:** 504 passed / 0 failed / 2 ignored across 111 test binaries, **measured at M21** over all
59 test chunks, every one `EXIT=0`; clippy clean over `--workspace --all-targets` with `-D warnings`.
See the testing note below for how that number is assembled. "111 test binaries" is 104 test
executables plus the 7 `Doc-tests` harnesses cargo reports, each of which runs zero tests — the
convention every milestone since M14 has counted by, kept for comparability and written out here so
nobody has to re-derive it. The two ignored gates are `stackoverflow_rust_e2e` (re-parked by M21 at a
signal-model wall, **not** the M8 risk R3 wall it stood at from M8 through M20) and
`cache_symbol_e2e` (the M19 shared-cache symbol wall); both are described under Known limits.

Reconciled against M23's 497 / 0 / 2 over 109 **file-by-file rather than by sum**. Per chunk:
A **129 → 129** (M21 touches no crate in that chunk), B 225 → **231**, and `--bins` **11 → 11**. All
six of B's are itemised: `stackgrow.rs` 1, three new `stack_geometry_tests`, `restorereserve.rs` 2.
The remaining +1 is `stackoverflow_rust_e2e`'s new *running* gate, which pins the progress M21 made
while its headline gate stays parked. Total **+7 running, ±0 ignored, +2 binaries**.

**Trace format:** `TRACE_MAGIC` is `RT\x00\x08`. Recordings from before M16 are rejected whole.
M23 did **not** move it, and that is a known sharp edge rather than a clean bill of health: M23
changed the vector table's padding, which lives in the trampoline page and is therefore snapshot
*content*. A pre-M23 recording still opens, and `Box_::restore` faithfully restores its **old**
zero padding while the current code assumes trapping padding — so a fall-through on that replay
reproduces the exact `pc=0x4204` misattribution M23 removed. The rule this repo writes down covers
changing `Event`'s *shape*; this was a change to what a snapshot's bytes *mean*, which the rule does
not name and which went unbumped.

## Known limits

These are real and current, not aspirational gaps.

- **About one in seven of Apple's system binaries still fails, and the remaining wall is one
  named cause.** Of 54 sampled, **46 now record and replay** (M23, up from 34 at M22), stdout
  byte-identical and exit codes equal. M22's four named causes are down to one plus a tail: the
  `pc=0x4204` group (13) and the `msgh_id` 412 group (4) are both cleared, `csh`/`tcsh` still hit the
  M10 fd table's fail-loud unmodelled `dup2` (working exactly as designed), and `ps` remains a
  genuine replay divergence — the oracle catching nondeterminism rather than reproducing something
  wrong in silence. The **new** group is four binaries (`automationmodetool`, `desdp`, `dyld_info`,
  `flex`) that reach a `brk`. That cause is **unmeasured**, and unlike M22's wall it has **no parked
  gate standing for it** — a gap in this repo's own discipline rather than a decision, recorded here
  rather than quietly left out. The 46 is the swept number and deliberately not the flattering one:
  `dddiagnose` is **intermittent** — 5 of 6 repeat runs record cleanly, 1 of 6 hits the `brk` — so it
  is counted as a failure, making the honest figure "46 as swept, 47 on most runs" rather than a 47
  that picks the run it likes. Eight of the 54 now report a **nonzero** fall-through count that
  record and replay agree on: the first binaries ever to exercise that invariant at all.
- **A guest must be arm64 or arm64e.** `slice_native` picks the slice this machine would execute —
  arm64e if the file has one, else plain arm64 — so universal files work, but an `x86_64`-only
  binary is refused by name. There is no emulation of another ISA and none is planned.
- **libdispatch runs only as far as it has been measured.** Rung 5 records and replays, but the
  workqueue emulation is a floor built from measurements rather than an implementation of the
  kernel's, and everything past that floor refuses **by value** instead of guessing. `workq_kernreturn`
  knows exactly three opcodes — `0x400` (dispatch setup), `0x20` (`REQTHREADS`, which builds the
  worker) and `0x4` (the worker's park, which must never return) — and names any other in a `panic!`,
  because the opcodes a *running* worker can issue cannot be enumerated until one issues them.
  `semaphore_signal_trap` (`-33`) wakes **exactly one** waiter and asserts if it would wake more: the
  plural case owes two unmeasured answers, `semaphore_signal_all_trap` (`-34`, still refused by a
  family-wide guard over `-39..=-33`) and *which* waiter a single signal should pick. And a pending
  signal on a thread parked in `semaphore_wait_trap` **aborts** rather than being delivered — M17
  materialises at `__ulock_wake` using a measured correction to the woken thread's saved context, and
  nothing has measured the equivalent here, so the wake names the measurement it owes. One further
  value is an extrapolation and flagged as such at its call site: the QoS entry-flags word
  `0x244004`, which no live run reproduced.
- **The scheduler is cooperative,** switching only when a thread blocks or exits. That is what makes
  the schedule replayable without recording it, and it is a deliberate trade: interleavings that
  require preemption mid-critical-section never occur, so **races that need preemption to manifest
  will not reproduce here.**
- **Symbolication stops at the shared cache, and at whatever the binary kept.** Since M19 the
  debugger names functions in the guest's own image and in dyld, but three limits are real. **Shared
  cache addresses resolve to nothing** — and re-measured during M20, the reason is not the one M19
  gave. There is no local-symbol area to stage: `localSymbolsOffset` is **zero in all thirteen**
  cache headers on this machine and no `*.symbols*` artifact ships at all. The cached dylibs *do*
  carry `LC_SYMTAB`, and their `__LINKEDIT` — 1.37 GiB of the cache's 5.40 GiB — sits inside the
  guest's 6.00 GiB shared-region window and is **already routed** by `cache.rs`'s demand-pager. Those
  pages are simply never *faulted*, because nothing in the guest reads a symbol table at runtime, so
  they are never staged into an anon page and never snapshotted. The exe and dyld resolve for the
  mirror reason: the guest's own loading does touch their `__LINKEDIT`. Since most of a
  dynamically-linked guest's executing pcs are *in* the cache, this is the difference between naming
  your own functions and naming everything; `cache_symbol_e2e` is parked there.
  **Stripped binaries yield nothing**, which is a property of the binary and not of retrace —
  `brew jq` ships with 7 defined text symbols against `threadrust`'s 969. And **Rust names are
  mangled** (`_ZN…E`): raw mangled names beat hex and need no demangler, but they are not pretty.
  Since M20 `break`/`delete` take a name, but **`watch` and `x` stay address-only** — and on
  evidence, not effort: `nlist_64` has five fields and **no size**, so `watch _global` would have to
  invent a width, and a watch of the wrong width silently misses writes to the bytes it failed to
  cover. That is the same quiet wrongness that makes an ambiguous `break` an error, refused for the
  same reason. A symbol named in pure hex (`deadbeef`) is also unreachable by name, because the
  hex-wins rule is what preserves existing scripts; Mach-O's leading underscore means real C symbols
  never collide.
- **A bad debugger operand now fails later than it used to.** `where; break zzz` printed nothing and
  exited 5 before M20; it now runs the `where`, prints it, then fails — still exiting 5. That is the
  measured price of resolving at execution rather than at parse, it is deliberate, and a test pins it.
- **No DWARF, no line numbers, no backtraces.** M19 reads `LC_SYMTAB` only, so an address becomes
  `_child+0x30` and never `crashthread.c:35`. There is no unwinder, so there is no stack trace.
- **The trace format is not stable.** `TRACE_MAGIC` broke in both M15 and M16. Recordings are
  currently working artifacts, not things to keep across milestones.
- **A signal to a thread that never wakes is never delivered.** Signals to a blocked thread are
  pended and materialised at the wake that makes the thread runnable; retrace does not interrupt the
  wait with `EINTR` as a real kernel would. A guest that strands a signal this way fails loud at a
  **clean** exit rather than exiting 0 and swallowing it; a guest that is already crashing is
  diagnosed by its crash instead. **At most one signal materialises per wake**, and a second
  deliverable one aborts loudly rather than being dropped: queueing at a wake is unmodelled because
  no guest in the tree measures it.
- **Two gates are parked `#[ignore]`d** at documented, *measured* walls, and the reason is on each
  test itself. `stackoverflow_rust_e2e` — but **no longer for the reason it carried from M8 through
  M20**. M8 risk R3 is CLEARED: the recursion now grows through M21's reservation and strikes its own
  guard page at stage 1. It is re-parked one wall further on, at the blocked-signal limit below, and
  the progress it used to stand for is gated by a *running* test beside it so it cannot regress in
  silence. And `cache_symbol_e2e` since M19, at the shared-cache
  symbol wall above. It was **three** between M22 and M23 — M22 parked `sysbin_e2e`'s second gate at
  `pc=0x4204`, reading it as a capability wall, and M23 un-parked it after finding it was a masking
  defect in retrace's own trampoline. This bullet said "two" throughout that window and was simply
  wrong; it is noted rather than silently corrected, because a current-state document that
  contradicted itself for a milestone is exactly the failure the two-document split exists to catch — a gate M19 parked for a capability it does not have, which by this repo's
  discipline has regressed nothing: `dispatch_e2e` was parked the same way by M18, moved twice as
  each measured wall fell, and then cleared.
- **A fall-through that arrives after its exception was already dispatched is undetectable.** The
  padding fall-through is counted, and a fall-through reported from outside the vector table fails
  loud. But a *duplicate* — a stale-PC resume landing on the padding after `ESR_EL1` was already
  serviced — presents a byte-identical PC, `ESR_EL1` and `SPSR_EL1` to a genuine first fall-through,
  because `set_x0_and_return` clears none of them. It would re-dispatch the same `(num, args)`,
  **record and replay would agree on the duplicate, and the divergence oracle structurally cannot
  see it.** Closing it needs resume-side state, not a check at the exit. The stale-PC resume itself
  was never root-caused; M23 root-caused only the masking that hid it.
- **`Box_::restore` does not rebuild the vector table.** `build_vector_table` is called from
  `Box_::load` and `load_dynamic` only, so the trapping padding reaches replay purely because the
  trampoline page happens to be a snapshot backing — correct today by luck rather than by
  construction, and the same shape as a defect found concurrently in M21 where the luck did not
  hold. An assert in `restore` would pin it and would also turn the stale-trace case above into a
  loud refusal instead of a wrong replay.
- **The trampoline page is padded for only 0x800 of its 16 KiB.** The rest is zero, which is
  `UDF #0` — the very encoding M23 removed from the vector slots. Nothing reaches it today, and a
  test pins the boundary, but the hazard is the one M23 exists to have eliminated.
- **The fall-through count is compared in one gate, not in the product.** It is deliberately not in
  the trace, so no single process ever holds both numbers; `fallthrough_e2e` diffs two stderr lines.
  `retrace replay` prints a count nobody checks and `retrace debug` never reports it, so "fails loud
  on mismatch" is true of the gate and not of retrace. It is also not comparable across a seek:
  `run_one_for_step` has no `Ec::Hvc` arm, so a stepped window cannot take a fall-through that the
  same window takes under `run()`.
- **A synchronously-raised signal that the target thread has blocked is not modelled.** A hardware
  fault cannot be deferred — POSIX leaves the case undefined and Darwin force-delivers — but M11
  models no pending set for it, so `retrace-core` **asserts by name** rather than guessing. This is
  now reachable rather than theoretical: a Rust stack overflow strikes its guard page, libstd *has* a
  handler installed for the resulting signal (10, SIGBUS), and the faulting thread has that signal
  blocked. Clearing it means giving M11 a pending set and revisiting `sigpending`'s always-empty
  answer. `stackoverflow_rust_e2e` is parked exactly there.
- **A stack frame larger than one granule can still vault the guard.** The reservation stops one
  granule above the guard page so the guard itself keeps faulting. A single frame bigger than 16 KiB
  can therefore step over it into unreserved space and take the old fatal stage-2 fault. Accepted by
  decision at M21, not overlooked.

## Testing

```sh
just gate     # cargo test --workspace + clippy -D warnings
```

**`--test-threads=1` is mandatory.** Hypervisor.framework allows one VM per process, so in-process
VM tests must run serially. `just gate` sets it; a bare `cargo test` flakes with `HV_BUSY`.

**`just gate` does not currently complete as one command.** The full workspace run exceeds a
10-minute ceiling and gets killed — M14 through M20 each closed on a chunked run instead.
Split it, run every chunk `--no-fail-fast`, and capture cargo's exit code *before* any pipe:

```sh
cargo test --workspace --exclude retrace-box --exclude retrace -- --test-threads=1
cargo test -p retrace-box -- --test-threads=1
cargo test -p retrace --test <name> -- --test-threads=1     # per-target for the e2e gates
cargo test -p retrace --bins -- --test-threads=1            # don't omit: see below
```

**Do not omit the `--bins` chunk.** `--test <name>` selects integration-test targets only, so the 11
unit tests inside the `retrace` binary itself (`crates/retrace/src/debug.rs`) run in none of the
other chunks; **only the unchunked `--workspace` run, or a whole-package `cargo test -p retrace`
without a `--test` filter, reaches them.** Leaving it out silently costs 11 tests and one binary —
465 / 0 / 2 over 105 instead of 476 / 0 / 2 over 106 — and nothing fails to warn you. Contrast
`cargo test -p retrace --lib`, which is invalid for this crate (there is no lib target) and fails the
whole invocation loudly.

**Run each `crates/retrace` test target as its own cargo invocation** — that is what keeps a chunk
inside the 10-minute ceiling above. It is no longer a codesigning requirement: `bin()` signs a
pid-unique copy (see Codesigning above), so concurrent test processes do not contend for it.

Some end-to-end gates depend on `/opt/homebrew/bin/jq`, which is not a repo artifact. They skip with
a loud `eprintln!` rather than passing quietly — a silent skip would read as a green it did not earn.
The same applies to the gates that record binaries out of `/bin` and `/usr/bin`: those are OS
artifacts, present on any macOS 26 machine, but announced rather than skipped silently if absent.

### Continuous integration — there isn't any, and there can't be

**No hosted CI can run this test suite.** It needs macOS 26 on Apple Silicon, and every VM test needs
the `com.apple.security.hypervisor` entitlement and a working `hv_vm_create`. GitHub-hosted macOS
runners are virtualized and do not offer nested virtualization, so `hv_*` is unavailable there; the
suite cannot merely be slow on hosted CI, it cannot start. This is a property of the platform, not
an unfinished chore.

Two consequences worth stating plainly, because they change what review means here:

- **A contributor must run the gate locally, on real hardware**, and paste the counts. There is no
  automated check that will catch a red for you.
- **A pull request cannot be validated by the maintainer without the same hardware.** If you do not
  have an Apple Silicon Mac on macOS 26, you can still usefully contribute to `retrace-arch`,
  `retrace-trace`, `retrace-sim` and the docs — those crates have no VM dependency and their tests
  run anywhere the toolchain does.

A self-hosted Apple Silicon runner would work, and is the only route to automation. None is
configured.

## Repository layout

| Crate | Role |
|---|---|
| `hv-sys` | bindgen FFI over Hypervisor.framework; thin safe wrappers |
| `retrace-arch` | zero-dependency arch facts: syscall numbers, ESR/PAC decode, Mach-O constants |
| `retrace-trace` | on-disk trace format: the `Event` enum, `Writer`/`Reader`, per-record CRC32 |
| `retrace-guest` | the Mach-O loader **and** the guest test programs (`asm/`, `c/`, `rs/`) |
| `retrace-box` | the core: guest memory, W^X page tables, PAC, the vCPU trap loop, the shared-cache pager |
| `retrace-core` | record/replay orchestration and the `mach_msg2`/MIG codec |
| `retrace-sim` | deterministic RNG + fault injection for the seeded swarm |
| `retrace` | the CLI binary and the end-to-end gates |

`spikes/*.c` are throwaway probes that empirically verified load-bearing HVF/SPTM/PAC claims on the
real OS before they were committed to the architecture; see `spikes/README.md`.

## Documentation

- [`docs/status-log.md`](docs/status-log.md) — the milestone-by-milestone engineering record,
  preserved verbatim and append-only. Historical by design: each entry is true as of its own
  milestone, so a claim that later proved wrong is left standing with a forward pointer rather than
  quietly corrected. This README is the document that is edited in place to say what is true *now*.
- `docs/superpowers/specs/` — per-milestone design specs and the measurements they rest on.
- `docs/superpowers/plans/` — per-milestone task plans.
- `CLAUDE.md` — architecture invariants and working rules for this repository.

## Contributing

The one rule that matters most here: **the walls are documented honestly, and they stay that way.**
A gate parked at a limit with its reason written on the test is worth more than a green that was
bought by loosening an assertion. If you clear a wall, move the gate forward and rewrite all three
places its reason lives — the test's `#[ignore]`, "Known limits" above, and a new appended section in
`docs/status-log.md`. If you cannot clear it, say so precisely and park it.

Two rules those gates taught, which bind their successors:

- **Never assert on an exit code a weaker failure would also produce.** An uncaught fault exits 139
  exactly like a caught-then-fatal one, so `segv_rust_e2e` asserts on the *trace* instead. Assert on
  the difference your change makes.
- **A skipped test must announce itself.** A silent skip reads as a green it did not earn.

Read `CLAUDE.md` before starting — it holds the platform invariants that will otherwise hang or panic
your machine (W^X, anon-only memory, one VM per process, `Box_`'s field drop order). They are not
style rules; violating them takes the whole system down.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.

`hv-sys` binds Hypervisor.framework by running `bindgen` against the macOS SDK **on your machine at
build time**. No Apple headers, source, or binaries are redistributed here, and none of Apple's dyld
or shared-cache bytes are vendored — they are read from the host at runtime.
