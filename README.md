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
| 7 | the real **CPython** interpreter | `-c 'print(1)'` — the 2026-07-05 vision spec's headline target |

**Apple's own binaries, measured — and, since M29, re-measurable.** `tools/apple-sweep.sh` points
retrace straight at each file in a committed 54-entry corpus and prints a tally: **46 of 54 record
and replay**, stdout byte-identical and exit codes equal. Among them `cat`, `ls`, `cp`, `mv`, `rm`,
`chmod`, `mkdir`, `ln`, `df`, `grep`, `wc`, `uname`, `sh`, `dash`, `expr`, `bzip2`, and — since M27 —
`ps`. Before M22 that number was **zero**, and not for the reason
it looked like: every macOS system binary is a *universal* file whose first four bytes are
`0xcafebabe`, and the loader asserted `MH_MAGIC_64` against them. retrace could always run Apple's
binaries; it could not open them. The figure moved from 47 to 46 when the sweep became a script
rather than a memory: scripting it exposed one binary that had always been diverging and added one
that cannot terminate, while a third — intermittent — happened to land on a clean run. **Read that
decomposition as an account, not an audit**: the 54-binary sample behind the old 47 was never
committed, so the corpus here is a reconstruction and the two figures are not strictly comparable.
See Known limits for all 8 that fail, which two are new to the list, why one of them is a failure by
design, and the reconstruction caveat in full.

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
- **`DC ZVA` runs natively at EL0, and `fd_operands` covers directory reads.** Two facts M25
  established while walking the real CPython interpreter. `SCTLR_EL1.DZE` is now **set**, so a
  guest's `dc zva` — which Apple's `_platform_memset` issues above a size threshold, and which
  CPython's allocator reaches at startup — zeroes its cache line instead of trapping to EL1 as an
  unhandled `MSR/MRS` exit. And `retrace_arch::fd_operands` now knows `getdirentries64` (344) and
  `fstatfs64` (346), so a guest that lists a directory has its own fd translated instead of handing
  the host a number that means a different file there. `UCT` (15) and `UCI` (26) stay deliberately
  **clear**: nothing has measured a guest issuing `dc cvau` / `ic ivau` or reading `CTR_EL0` from
  EL0, and the existing exit fails loud if one does. Both changes sit below the trace or beside it —
  nothing new is recorded and `TRACE_MAGIC` did not move.
- **The record-side diff window covers what the kernel may write, and an overrun that reaches past
  it now fails loud rather than surfacing later or not at all.** `forward_and_diff` captures the
  kernel's writes by snapshotting a pre-image window of each pointer argument, forwarding, and
  diffing that window. M26 widened the window only where the destination's length was a register
  (`read`/`pread`/`read_nocancel`); **M27 adds `sysctl`(202)**, whose length lives at
  `*(size_t*)x3` in guest memory rather than a register — this is what makes it `/bin/ps`, whose
  `sysctl(KERN_PROC_ALL)` reply runs to 205,416 bytes against the old flat 64 KiB window — and
  **`pread_nocancel`(414)**, which before M27 was missing from `fd_operands`, the forwarded-count
  clamp, **and** the window at once; the missing clamp was the more serious half, since an
  unclamped forward lets the host kernel write past the guest's actual backing. The `readv`/
  `recvmsg` nested-pointer family (120/27/540/411/401/480) moved from "would hand the host kernel a
  guest address" to **refused by value**, fail-loud. Everything else that still gets only the 64 KiB
  heuristic is now backed by a guard band: a 64-byte region immediately past every *capped* diff
  window is snapshotted before the forward and compared after, and a changed byte there **panics**,
  because nothing but the halted guest's own `host_svc` call runs in between. **M28 made that panic
  trustworthy on both axes it previously lacked.** First, that it can fire at all: a positive control
  (`truncguard.rs`) shrinks the window cap to 64 bytes and drives `fileio`'s `fstat` — which writes a
  MEASURED `sizeof(struct stat) = 144` bytes and is deliberately absent from `dest_buffer`, so no
  widening can rescue it — into the band; the mutation `let band = 0;` was verified to **FAIL** that
  test (`NOT-THE-GUARD-BAND: fstat wrote past a 64-byte window and nothing fired`) before being
  reverted, where before M28 that identical mutation passed the entire 523-test gate unnoticed.
  Second, that a firing is attributable: `Box_::band_not_covered` shrinks each band to exclude only
  the bytes some *other* window of the same call already inspects, so a byte that still changes *is*
  proof of a kernel write past everything this call's diff inspected — not merely past this one
  argument's window, and not a maybe. It ran across the whole M28 gate and fired zero times, same as
  M27. What M28 *did* measure is how often the shrink itself narrows a band: **31 times on
  `/bin/ps`** alone, across six syscalls, every one a complete `64 -> 0` on a capped 65536-byte
  window — two arguments of the same call sharing a backing closely enough that one argument's
  window fully covers the other's band. That is not a blind spot: a suppressed byte is one some
  other window of the same call already inspects, so the write is captured anyway, just against that
  argument's own ipa. What that count measures is how much of M27's claimed proof was never
  attributable in the first place — a finding about M27, not a new weakness M28 introduced. **M29
  made the count a real observation**, which M28's own was not: M28 published "zero across the full
  gate" from a channel that could not have carried it, since every e2e test drives the recorder as a
  child and pipes its stderr into a `String` a passing test never prints. `ps_records_and_replays`
  now records `/bin/ps` with `RETRACE_BANDSHRINK=1` set on the recorder and **asserts the count is
  at least one** — the gate enforces a floor, not an exact number. **A silent band is
  still not proof the class is gone** — see Known limits for the measured false negative that makes
  this the honest statement rather than the stronger one.

**Gate:** 537 passed / 0 failed / 2 ignored across 116 test binaries, **measured at M29** over all
116 targets, every chunk `EXIT=0`; clippy clean over `--workspace --all-targets` with `-D warnings`.
See the testing note below for how that number is assembled. "116 test binaries" is 109 test
executables plus the 7 `Doc-tests` harnesses cargo reports, each of which runs zero tests — the
convention every milestone since M14 has counted by, kept for comparability and written out here so
nobody has to re-derive it. The ignored gates are unchanged at
**two**: `stackoverflow_rust_e2e` (re-parked by M21 at a signal-model wall, **not** the M8 risk R3
wall it stood at from M8 through M20) and `cache_symbol_e2e` (the M19 shared-cache symbol wall). Both
are described under Known limits. M29 parked nothing new and un-parked nothing.

Reconciled against M28's 532 / 0 / 2 over 116 **file-by-file rather than by sum**:
`retrace-arch/src/lib.rs` **+2** (the new `dest_buffer` entries and `recvfrom`'s `fd_operands` pair),
and the existing `retrace-box/tests/truncguard.rs` **+3** (one proving the window widens for each new
`Reg` entry, two for the `*oldlenp` refusal). Every other file unchanged, and `--bins` **11 → 11**.
**No new test binary**: `oldlensysctl.s` is a guest fixture rather than a test target and
`truncguard.rs` already existed, so the binary count holds at 116. The count closes at both ends: the
tree holds 534 `#[test]` at M28 = 532 running + 2 ignored, and **539** at M29 = 537 + 2.

Chunk B again ran `cargo test -p retrace-box` as a **whole package** so its `Doc-tests` harness is
not silently dropped (M24's lesson, now standing practice), and the `retrace` package was split into
four explicit target sets rather than run whole, so no chunk is killed by the ceiling and every
target runs in exactly one chunk.

One timing trap is worth knowing before it is mistaken for a hang: `bigread_e2e` took **536s** on its
first run and **47s** on its second, with the recording process sitting at 0:00.00 CPU throughout the
stall. That is first-execution codesign validation of a freshly signed binary, not a hung guest. The
second number is the honest one.

**Trace format:** `TRACE_MAGIC` is `RT\x00\x09`, moved by **M24**. Recordings from before M23 are
rejected whole, at `Reader::open_checked`, before a single byte of them is trusted. M23 had changed
the vector table's padding — which lives in the trampoline page and is therefore snapshot *content* —
without moving the magic, so a pre-M23 recording still opened and `Box_::restore` faithfully restored
its **old** zero padding while the current code assumed trapping padding, reproducing the exact
`pc=0x4204` misattribution M23 removed. M24 closes that at the layer it belongs to. The lesson is in
the rule now: the repo's written rule covered changing `Event`'s *shape*, and this was a change to
what a snapshot's bytes *mean*, which a shape rule cannot see. Both are format breaks and both bump
the magic.

## Known limits

These are real and current, not aspirational gaps.

- **Eight of 54 sampled Apple system binaries still fail, and the sweep that says so is a script
  now rather than a memory.** `tools/apple-sweep.sh`, over the committed 54-entry corpus
  `tools/apple-sweep-binaries.txt`, records and replays each binary and prints a `TALLY` line, so
  since M29 this figure is **reproducible instead of remembered**. The measured tally is
  **`TALLY pass=46 fail=8 skip=0`** — stdout byte-identical and exit codes equal for the 46 — and on
  every run that landed there, both the PASS set and the FAIL set were byte-for-byte the same. (One
  binary moves between runs; see `dddiagnose` below.) The corpus is a
  **reconstruction**: the original sample behind the 47 published at M27 (46 at M23, 34 at M22) was
  never committed, so 46 is not strictly comparable to those numbers. It is simply the first such
  figure a later reader can re-derive.
  The eight, with the reason string the sweep itself prints: `csh` and `tcsh` (`recorder panicked` —
  the M10 fd table's fail-loud unmodelled `dup2`, working exactly as designed);
  `automationmodetool`, `desdp`, `dyld_info`, `flex` and **`/bin/launchctl`** (`replay diverged`);
  and `/usr/bin/yes` (`timed out after 30s recording`).
  **What the sweep reports is not why they fail**, and for the M23 group the two must not be
  conflated: those four have been believed since M23 to reach a `brk`, but the sweep classifies them
  only as diverging replays and corroborates nothing about a `brk`. (`csh`/`tcsh` are different — the
  `dup2` panic text was read directly off a re-run, not inferred from the category.) That cause **stands unmeasured to
  this day**, with **no parked gate standing for it** — a gap in this repo's own discipline rather
  than a decision, recorded here rather than quietly left out.
  **Two of those eight are new to this list, and neither is a regression.** `/bin/launchctl` was
  always diverging — the sweep script's first draft compared a variable against itself, making its
  exit-code check a tautology that reported four binaries as passing when they were not, and fixing
  it is what exposed `launchctl`. `/usr/bin/yes` never terminates, so it cannot pass under any method
  that requires a bounded comparison; it is counted a FAIL **on purpose**, since excluding it would
  raise the tally without changing anything about retrace.
  **`dddiagnose` is intermittent, and M29 watched that happen** rather than only asserting it: two
  runs of the same script against the same tree gave 45/9 and 46/8, with `dddiagnose` the only mover.
  It is **not** among the eight above because the runs quoted here are ones it passed — which is
  exactly the point. The number is quoted as swept rather than as best-of, and a re-run that returns
  45/9 has found nothing new.
  M22's four named causes are down to one plus that unmeasured tail — the `pc=0x4204` group (13) and
  the `msgh_id` 412 group (4) were both cleared at M23 — and **`ps` was fixed at M27**. It was
  published here from M22 through M26 as "the oracle catching nondeterminism", a claim that could not
  have been true, since replay never *executes* a syscall, only applies recorded writes, so a process
  list cannot vary between the two runs; M26 corrected the *description* (the real cause is the
  truncating diff window) without closing it, and M27 closed it: `ps` sizes its
  `sysctl(KERN_PROC_ALL)` buffer at 205,416 bytes, `retrace_arch::dest_buffer` now knows that length
  lives at `*(size_t*)x3`, and the window widens to cover the whole reply. Separately — and this is a
  different eight from the failures above — eight of the 54 report a **nonzero** fall-through count
  that record and replay agree on: the first binaries ever to exercise that invariant at all.
- **A guest must be arm64 or arm64e.** `slice_native` picks the slice this machine would execute —
  arm64e if the file has one, else plain arm64 — so universal files work, but an `x86_64`-only
  binary is refused by name. There is no emulation of another ISA and none is planned.
- **The record-side diff window still truncates for most syscalls, and a guard band now proves an
  overrun when it fires — attributably, since M28 — but a silent band is still not proof there
  wasn't one.** `forward_and_diff`
  snapshots a pre-image window per pointer argument and diffs that same window; the window widens to
  the real length only where `retrace_arch::dest_buffer` knows it. M26 covered `read`(3)/`pread`(153)/
  `read_nocancel`(396); **M27 added `sysctl`(202)** (length at `*(size_t*)x3`, unbounded, and the
  reason `/bin/ps` now passes) and **`pread_nocancel`(414)**, which before M27 was missing from
  `fd_operands`, the clamp **and** the window at once — the missing clamp was the serious half, since
  an unclamped forward lets the host kernel write past the guest's actual backing. **M29 adds four
  more, leaving three — the shortest this list has been since M26**: `getdirentries64`(344) and
  `recvfrom`(29/403), which had exactly the shape M26/M27 fixed and were named right here as absent;
  `getfsstat64`(347), whose `x1` is a byte count rather than a mount count; and `sysctlbyname`(274) —
  the interesting one, because it was missing from the table **and** from this list of what was
  missing, so a list of known gaps turned out to be only as complete as the audit that wrote it. Its
  raw kernel entry takes `namelen` first, exactly like `sysctl`, so its indices are `(2, DerefU64(3))`
  — **identical** to `sysctl`'s, not one lower as this milestone's own plan and spec first said. That
  error survived a review that checked all five new entries against the SDK, because raw-versus-libc
  argument shape is invisible in a man page; it was caught only by calling `syscall(274, …)` against
  the live kernel with libc's 5-arg wrapper bypassed. **Three still get a flat 64 KiB**: `proc_info`
  (336); `getattrlist`/`fgetattrlist` (220/228); `csops` (169/170).
  **The clamp M27 and M28 both left owed is paid, and by refusing rather than clamping.** `sysctl`'s
  `*oldlenp` is an *in-out* length — the guest writes how much room it has, the kernel writes back
  how much it used — so silently clamping it would tell the kernel a smaller buffer than the guest
  asked for and hand the guest a truncated reply it has no way to know was truncated: a wrong answer
  dressed as a right one. `forward_and_diff` therefore **refuses**. When `*oldlenp` exceeds the
  backing behind `oldp`, it asserts by name instead of forwarding — the same fail-loud discipline
  `guest_workq_kernreturn` uses for an unenumerated opcode — and `RETRACE_DEREFLEN=1` prints the
  backing span that settles which side is wrong. Refusing was chosen on measurement, not taste:
  across **826 dispatches** of the `DerefU64` arm (783 across all 54 binaries of the Apple sweep, 13
  from `jq`, and 15 each from two CPython startups), **every one fit inside its backing** — none
  oversized, none unbacked — so nothing that runs today is refused. A purpose-built guest
  (`oldlensysctl.s`) asking for `1 << 40` bytes proves the refusal fires, and a second test proves a
  NULL-`oldp` size query still passes.
  **Three narrownesses bound that zero, because they say what it does not cover.** All 826 dispatches
  are `sysctl`(202): `sysctlbyname`(274) is exercised by **nothing** in any corpus, so its new entry
  is correct by measurement of the kernel's argument shape but untested by any guest — the refusal
  covers it by code-path symmetry only. "Reached" means reaching `forward_and_diff`, so the
  `sysctl(KERN_USRSTACK64)` that `retrace-core` answers itself never arrives there and is not in the
  count. And the two CPython rows are one interpreter startup measured twice — once through Homebrew's
  launcher shim, once through the real binary — so the evidence base is **56 effectively-independent
  binaries**, not 57. The `readv`/`recvmsg` family (120/27/540/411/401/480) **moved classes in
  M27**: their destination sits behind a pointer *inside* a guest struct that nothing translates, so
  before M27 a guest IPA would have reached the host kernel as a host address — a wild-write hazard,
  not a fidelity gap. They are now **refused by value**, fail-loud, the same discipline
  `guest_workq_kernreturn` uses for an unenumerated opcode, until the `translate_mwl_regions`
  treatment and its own measurement extend to them.
  There is still **no BSD-syscall allowlist** — everything not explicitly intercepted is forwarded —
  so the remaining set is open-ended rather than enumerable, which is what the **M27 guard band**
  exists for: it snapshots 64 bytes immediately past every *capped* diff window before the forward
  and compares them after, and **panics** if a byte changed, because between the two snapshots
  nothing but the halted guest's own `host_svc` call can have touched that memory — a changed byte
  is *proof* of a kernel write the diff may not have inspected, not an inference. **M28 closed the
  gap that used to sit right here**: it was not proof the write belonged to *this* argument's
  overrun, because `forward_and_diff` takes a window for every mapped-looking argument and a write by
  another argument of the same call, landing in that same range, would also trip it. `Box_::band_not_covered`
  now shrinks each band to exclude only the bytes some *other* window of the same call already
  inspects, so a byte that still changes in what remains cannot be a write that another window of
  this call already captured — it is proof of a kernel write past everything this call's diff
  inspected. Which argument overran is still not established: a different argument's overrun, running
  past its own window, can still reach this band. And M28 proved the band can
  fire at all, which nothing had: a positive control (`truncguard.rs`) shrinks the window cap to 64
  bytes and drives `fileio`'s `fstat` — writing a MEASURED `sizeof(struct stat) = 144` bytes,
  deliberately absent from `dest_buffer` so no widening can rescue it — into the band; `let band = 0;`
  was verified to FAIL that test before being reverted, where before M28 the identical mutation passed
  the entire 523-test gate unnoticed.
  It ran across the whole M28 gate too and fired zero times, including on `/bin/ps`. What M28 did
  measure is how often the *shrink itself* narrows a band: **31 times on `/bin/ps`** alone, across
  six syscalls — 344 (`getdirentries64`) ×15, 399 ×11, 33 (`access`) ×2, and one each of 5 (`open`),
  347, 339 (`fstat64`) — every one a complete `64 -> 0` on a capped 65536-byte window, because two
  arguments of that call share a backing closely enough (adjacent stack slots, 304 bytes apart) that
  one argument's 64 KiB window fully covers the other's band. **That is not a blind spot**: a
  suppressed byte is one some other window of the same call already inspects, so the kernel write
  there is captured anyway, recorded against that other argument's own ipa — nothing is lost by not
  flagging it a second time. And it cannot become one structurally: the window with the maximal end
  address in a backing can never itself be suppressed, since suppression needs some other window
  ending even further out, which is impossible for whichever window already ends furthest — so the
  band immediately past everything the call inspected stays guarded no matter how many inner bands
  get truncated to zero. What that count actually measures is how much of M27's claimed proof was
  never attributable in the first place, which is a finding about M27's own strength and not a
  weakness M28 introduced.
  **That 31 was hand-measured and could not be re-derived from the gate; since M29 it can.** M28
  also reported the count as zero across the full gate, which no channel could have delivered: the
  `[M28 BANDSHRINK]` line is recorder stderr, and every e2e test drives the recorder as a child
  process whose stderr `crates/retrace/tests/util/mod.rs` pipes into a `String` that a passing test
  never prints. M29's `ps_records_and_replays` records `/bin/ps` through a new
  `util::record_dynamic_env` helper with **`RETRACE_BANDSHRINK=1`** set on the recorder — a gate
  separate from `RETRACE_TRACE` so the count is obtainable without the per-trap firehose — and
  asserts the observed count is **greater than zero**. That is deliberately a **floor, not a
  number**: the assertion is what the gate enforces, and the count itself reaches only a
  `--nocapture` run, which reported **32** on this machine. Why 32 rather than M28's hand-counted 31
  is **not root-caused** — nothing measured which call produced the extra suppression — and that is
  precisely why the assertion is `> 0` rather than `== 31`: what fails should be a portable property,
  and this milestone does not know what makes the count vary.
  **That silence is not proof of absence, and this is measured rather than argued.** Before the
  `sysctl` fix landed, `ps`'s own overrun was the band's would-be catch: the kernel wrote 139,880
  bytes past the window, and the band still did not fire, because `struct kinfo_proc` carries long
  zero runs and the window boundary happened to land inside one — the kernel wrote zeros over zeros,
  indistinguishable from no write at all. `/bin/ps` passes today because `dest_buffer` covers
  `sysctl`, never because the band caught it. **The band is proof when it fires; it is not proof of
  absence when silent**, and the remainder of the table above is *guarded* by it in that weaker sense,
  not cleared by it. The saving grace underneath stays what it was: `Box_::diff_memory` compares
  every recorded region at exit and all three terminal replay arms fail on mismatch, so a truncation
  the band misses can still surface there, unless the guest acts on the stale bytes first or drops
  their backing before then — a read into a mapping that is then `munmap`'d would evade both, and no
  gate does that today. Two holes stay open and unmeasured: `Box_::diff_memory`'s own `.min(avail)`
  clamp on the replay side (flagged in M1's own branch review, deferred at M2, still unpaid), and the
  `if !err` gate, which skips write capture entirely on a failed syscall — and the band is not
  evaluated on that path either, so a failing syscall is neither diffed nor guarded. M27 measured
  that this is *not* what `ps` hit (`err=false` on all 83 of its `sysctl` calls), which narrows the
  question without closing it. **M28 measured one further, purpose-built case**: a guest whose
  `sysctl(KERN_OSTYPE)` deliberately fails on a 2-byte buffer (`"Darwin\0"` needs seven) came back
  `err=true ret=12 (ENOMEM) writes_captured=0 buf_changed=false` — the kernel wrote nothing into the
  guest's buffer either before or after the call, independently reproduced by disassembling the
  committed guest and replaying the identical syscall twice against the live host kernel. So the
  `if !err` skip loses nothing on this case, but this is one measured datum, not a general proof about
  failing syscalls — the gate stays open, now with a data point in it instead of none. Strengthening
  the band itself — sampling across the whole remaining backing under a fixed byte budget, rather
  than one contiguous 64-byte run immediately past the window — is now *unblocked*, since the "not
  covered by another window of this call" precondition Task 2 needed exists in code as
  `band_not_covered`, but was still deliberately not attempted in M28: the suppression count above is
  a warning to whoever takes it up, since a naive wider sample would be suppressed even more often,
  not less.
- **Exec-in-place is unmodelled — point retrace at the real binary, not the shim.** A launcher that
  `posix_spawn`s with `POSIX_SPAWN_SETEXEC`, which is exactly what Homebrew's `python3.14` shim does
  to hand off to the interpreter above, gets an **error** back instead of a replaced image and takes
  its own failure path. retrace records and replays *that* outcome byte-for-byte — the oracle has
  nothing to disagree about, so this is retrace working rather than a bug — but the guest you get is
  the shim reporting a failure, not the program you meant to run. The behaviour is pinned by a test
  whose job is to hold the limitation visible, and which is to be **rewritten rather than defended**
  when exec-in-place lands.
- **`fd_operands` fails quietly, not loudly, on a syscall it has never seen.** A syscall that takes a
  file descriptor but is missing from `retrace_arch::fd_operands` has its guest fd forwarded to the
  host **unchanged**, where the same integer names a different file. M25 hit precisely that:
  `getdirentries64` (344) and `fstatfs64` (346) were absent, so `os.listdir` handed guest fd 4
  straight to the host kernel, which returned `EINVAL` for a vnode that was not a directory. Both are
  in the table now, but the **class** is still open — the default arm is `_ => &[]`, so the next
  missing entry fails the same silent way, and unlike the fd table's `dup2` path it does not announce
  itself. Making the default fail loud needs a blast-radius measurement nobody has taken.
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
- **The trace format is not stable.** `TRACE_MAGIC` broke in M15, M16 and again in M24. Recordings
  are currently working artifacts, not things to keep across milestones — and M24 is the milestone
  that made the refusal honest, so a stale one is now rejected at open instead of half-read.
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
- **Record-only box state is guarded on one replay path and not the other.** `Box_` has three
  construction paths — `load`/`load_dynamic` (record only), `restore` and `from_checkpoint` (both
  replay only) — and anything a load path establishes that a replay path does not re-establish is a
  bug whose signature is *a passing record followed by a diverging replay*. The determinism oracle
  cannot see it when both replay paths are wrong the same way, because the oracle compares replay
  against record's **trace**, never against record's **box**. By this repo's own written record the
  class has shipped seven times (M9 t3, M10, M11, M14, M18, M21, M23), each fixed individually and
  none leaving behind anything that would catch the eighth. Since M24 the `load`↔`restore` pair is
  pinned by a standing test — `retrace-box/tests/restoreparity.rs` diffs a load box against a
  `restore` box built from that box's own snapshot, comparing 15 of `Box_`'s 27 state fields plus two
  sysregs and the 0x800 vector table, and it states an obligation: a new field must be either covered
  there and equal, or named in `normalise()` citing the mirrored replay mechanism by file and line.
  **`from_checkpoint` has no such guard**, and that is the path with the documented *five*-instance
  history — it restores far more state than `restore` does and runs mid-run where nothing is at a
  default. So the class is **not closed**; it is closed on the path it has bitten twice and open on
  the path it has bitten five times, which is the successor milestone. Two blind spots are structural
  even where the guard runs: it compares construction at landmark 0 and not evolution after it, and
  two boxes that are wrong in the *same* way (a static box's zeroed thread-0 context, identical on
  both sides) are invisible to any test that only diffs the two against each other.
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
at M29, 526 / 0 / 2 over 115 instead of 537 / 0 / 2 over 116 — and nothing fails to warn you. Contrast
`cargo test -p retrace --lib`, which is invalid for this crate (there is no lib target) and fails the
whole invocation loudly.

**The same trap has a second mouth: `Doc-tests`.** `--test <name>` skips those too, so splitting a
*library* crate per-target — as M24's gate had to for `retrace-box` — drops that crate's `Doc-tests`
harness from every chunk. It runs zero tests, so nothing fails; it just quietly costs one of the 116
binaries. If you split a library crate per-target, run `cargo test -p <crate> --doc` alongside it —
or, as M25's gate did, run that crate as a whole package and let cargo include it for you.

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
