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
| 8 | real CPython on a real script that crashes | `crash.py`: json/os/sys work on a data file, then a ctypes deref; recorded, replayed, and reverse-continued from the crash to the store of the pointer |

**Apple's own binaries, measured — and, since M29, re-measurable; since M36, with the reason each
failing row fails and a parked gate for every one that is retrace's; since M37, the same reasons
from any recorder pid; since M38, with two false passes turned into named walls.**
`tools/apple-sweep.sh` points
retrace straight at each file in a committed 54-entry corpus and prints a tally: **44 of 54 record
and replay**, stdout byte-identical and exit codes equal (`TALLY pass=44 fail=10 skip=0`, measured
2026-09-21 on the M39 close's binary, one run at recorder pids `0x71d2`–`0x7dcf`; the same figure
M38 measured 2026-09-16, and **no row changed its label** between the two runs — M39's one wall is
reached by `import ctypes`, and no corpus binary loads `_ctypes.so`). The figure last
moved at M38, from M37's 45/9, by **three rows, each explained by name**: `launchctl` is *clean* now (the
receive-shaped `mach_msg2` it stopped at is refused deterministically, and it runs to its own
usage `exit(1)`, byte-identical on both sides); `ls` and `ed` are *not* — and were not before
either: both had been "passing" by failing identically (`ls` printing an `EBADF` error for `.`;
`ed` exiting 2 with its error message lost) on an `AT_FDCWD` the fd table rejected, and with the
sentinel honoured each now runs on to a syscall the box has no `arg_kinds` row for
(`getattrlistbulk` 461; `openat_nocancel` 464) and stops **loud**. A silent lie replaced by a
named wall is the honest-gate discipline working, not a regression, and both are on the owed list
by number. M37 had run the sweep three times on 2026-09-13 with the recorder's pid steered into
the three regimes M36 had measured (below `0x4000`; inside `[0x4000, 0x10000)`; inside
`[0x10000, 0x18000)`) and tallied 45/9 in all three with the same nine labels in every regime —
the pid-collision defect that once selected a wall is gone, and M38's single run (spec R6) rests
on that. The ten rows that are not clean are read off kept evidence, one class each, and **seven
parked gates** (`crates/retrace/tests/apple_walls_e2e.rs`, one per binary that is retrace's to fix
or model and has a gate) stand for them, each `#[ignore]` reason the measurement that parks it.
Among the 44: `cat`, `cp`, `mv`, `rm`,
`chmod`, `mkdir`, `ln`, `df`, `sh`, `dash`, `bash`, `zsh`, `expr`, and — since M27 — `ps`. (This
sentence named `grep`, `wc`, `uname` and `bzip2` from M22 through M32; none of the four is in the
committed corpus, a leftover of the uncommitted sample the reconstruction caveat below describes,
corrected at M33.) Before M22 that number was **zero**, and not for the reason
it looked like: every macOS system binary is a *universal* file whose first four bytes are
`0xcafebabe`, and the loader asserted `MH_MAGIC_64` against them. retrace could always run Apple's
binaries; it could not open them. The figure moved from 47 to 46 when the sweep became a script
rather than a memory: scripting it exposed one binary that had always been diverging and added one
that cannot terminate, while a third — `dddiagnose` — happened to land on the face the old script
counted as a clean pass, which M36 measured to be that identical crash. **Read that
decomposition as an account, not an audit**: the 54-binary sample behind the old 47 was never
committed, so the corpus here is a reconstruction and the two figures are not strictly comparable.
See Known limits for the ten-row table — the face each row shows, its class, its gate and its
route — which rows are new to the list, why one of them is a failure by design, and the
reconstruction caveat in full.

**Capabilities**

- **Reverse execution** — `(N,K)` landmark seeks, checkpointed for ~800× faster backward seeks.
- **Watchpoints** — hardware `DBGW` (pre-retire) plus software detection, with
  reverse-continue-to-last-writer, thread-attributed. Since M40 a hit is resolved by the
  **address** it wrote, by single-stepping with the watchpoint armed, not by the store's pc: a
  store instruction that ran on other addresses first — a `memset`-style loop — had made `continue`
  name an earlier run of it that never wrote the watched memory. `watchsweep_e2e` is the guard.
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
  at least one** — the gate enforces a floor, not an exact number.
  **M30 changed the kind of detector, not its size.** `GUARD_BAND` is still **64**. What changed is
  the question: M27's band was a pre/post *comparison*, so it was **100% blind whenever the kernel
  wrote bytes identical to those already there** — not hypothetical, but what M27 measured on
  `/bin/ps`, whose 139,880-byte overrun landed inside `struct kinfo_proc`'s long zero runs and
  reported nothing. `forward_and_diff` now **fills** each shrunk band with
  `canary_byte(ipa) = (ipa as u8) ^ 0xA5` before forwarding, asks `canary_intact` after, and restores
  the bytes before the guest resumes. A kernel write of zeros over zeros destroys that pattern and
  **aborts the recording**; the same fixture and window cap that fooled the old comparison now does
  exactly that, in both halves of `truncguard.rs`'s before/after pair. The pattern is derived from the
  *address* rather than being a constant so that two overlapping bands agree on every shared byte and
  a constant-value `memset` cannot reproduce it.
  **That new strength does not cover every forwarded syscall, and the exclusion is not small.** The
  canary is withheld from `retrace_arch::reads_guest_buffer` — `write`/`pwrite`/`writev`, the `send*`
  family, `sendfile`, `msync`, and **`mach_msg2_trap`**, each with its `_nocancel` spelling — because
  the kernel reads *through* those buffers and would consume the canary as data. That is measured,
  not feared: filling them made a 128 KiB-write guest produce a file with **64 corrupted bytes
  matching `canary_byte` exactly**, while record exited 0, replay exited 0 and the canary count read
  0 — record and replay agreeing while the guest's output was wrong, which is the one failure a
  determinism oracle cannot see. `bigwrite_e2e` is the repo-owned reproduction. That family keeps
  M27's `overran_window` **bit-for-bit**: no weaker than before M30, and no stronger.
  **Two of those exclusions cost real destination-side coverage**, and it is not a footnote:
  `sendfile`'s 4th argument is an in-out `off_t *` the kernel writes the transferred count back
  through, and `mach_msg2`'s receive buffer is a live destination — `machmsg.rs`'s
  `FORWARD_ALLOWLIST` forwards five ids through `forward_and_diff`, and two of them
  (`3405 task_info`, `412 host_get_special_port`) exist *precisely* because the kernel writes a reply
  into guest memory. Recovering that needs a per-**argument** direction notion — a `dest_buffer`-shaped
  table of which arguments are sources — that a predicate over the syscall number cannot express;
  it is owed successor work, not something this milestone quietly has. `ioctl` is the named residual
  hole in the list itself: a `_IOW` request encodes its buffer length in the request code, so no rule
  over the syscall number can size it, and `msync` is the one entry justified by inference rather
  than measurement and says so at its definition. And on a *filled* band, a kernel write that
  reproduces the canary pattern **exactly** is still undetectable in principle — 1/256 per byte, with
  the kernel having to hit it on every byte it writes to stay invisible. That is inherent to any
  canary, and it is the price of replacing a detector that was 100% blind to the zeros case.
  **The flip was taken on a measurement, not on confidence.** Phase A ran the fill report-only and
  measured **zero** disturbed bands on every path, each carrying a positive control taken first on
  that same path: in-process (one `[M30 CANARY]` line from the caught-half test), the CLI (**27**
  `[M28 BANDSHRINK]` control lines off `/bin/ps`, then zero canary lines from `/bin/ps`,
  `jq --version` and the real CPython interpreter), and the Apple sweep (**392** control lines from
  **54** distinct guests, zero canary lines, tally unmoved at `pass=46 fail=8 skip=0`).
- **One table, six views, and a syscall that cannot be forwarded unclassified.** Since M33,
  `retrace_arch::arg_kinds(num) -> Option<&'static Shape>` is the one table that says what a
  syscall does with each of its arguments — `Scalar`, `Fd`, `Path`, `Source`, `NestedSource`,
  `Dest(DestLen)`, `NestedDest` or `Ptr` per register, plus a return kind (`Plain`, `Fd`,
  `FdPair`) — and the five functions the M26–M32 lineage accreted (`fd_operands`, `allocates_fd`,
  `dest_buffer`, `writes_via_nested_pointer`, `reads_guest_buffer`) are one-line **views** over
  it, joined at M38 by a sixth, `returns_fd_pair` (`ret == Ret::FdPair` — the row `pipe`'s
  two-descriptor binding consults on both sides).
  Every row opens with its kernel prototype (xnu `syscalls.master` / `syscall_sw.h`, or the SDK
  header, and it says which) and every `Ptr` names the cited bound that keeps it out of `Source`
  and `Dest` — all but one, `__mac_syscall`'s (381) policy-defined `arg`, whose row says it rests
  on reasoning rather than on a number nobody outside Apple can cite. The refactor is proven rather
  than asserted: `legacy_equivalence.rs` carries the five
  M32 tables **verbatim** as a fixture and sweeps every syscall number in the domain (BSD
  `0..=1023`, mach traps `-1..=-128`, the `MAC_SYSCALL_MAGIC` band) through every view **in both
  directions** — a view that disagrees with its legacy table without an `EXPECTED_DIFFS` entry
  fails, and a listed entry that no longer differs fails too. There are **22** such entries, each
  with its reason. **Sixteen** are descriptors the legacy `fd_operands` never translated —
  `pwrite`/`pwrite_nocancel`/`writev`/`writev_nocancel`/`pwritev`, `sendto_nocancel`/`sendmsg`/
  `sendmsg_nocancel`/`sendmsg_x`/`sendfile`, and the six refused `readv`/`recvmsg` spellings (moot:
  refused upstream, before translation runs) — the M10 class, in the tree since M30 tabled them as
  readers from their prototypes, and hit by exactly one corpus guest (`/bin/ed`'s
  `writev_nocancel`, on fd 2, which translates to itself). **Six** are census rows the legacy
  tables had no opinion on: `fchdir` (13, `/bin/ls`), `kqueue` (362, `/bin/wait4path`), `execve`
  (59, `/bin/sh`) and `posix_spawn` (244, the CPython launcher) as nested readers — both
  **refused** since M38, their rows documentation of the prototype only — `sigreturn`
  (184, serviced above the trace), and `map_with_linking_np` (550), whose `link_info` is a
  caller-sized `Source` capped only at 64 MiB. **The forward path is loud now.** `Box_::translate_fds`
  — the first statement `forward_and_diff` executes — calls `forwarded_shape`, which panics by name
  on a syscall with no row (`M33: syscall 8 (8) has no arg_kinds row in
  crates/retrace-arch/src/lib.rs — it cannot be forwarded unclassified …`), and `unenum_e2e`
  drives a guest that issues syscall 8 and asserts on that line, never on an exit code, since
  before M33 the same guest recorded and exited 0. The rows come from a **census**: every syscall
  number the corpora dispatch — 56 repo guests, `jq` twice, CPython twice (interpreter and
  launcher), and all 54 Apple-sweep binaries — recorded under `RETRACE_TRACE=1` on 2026-09-12:
  **108 distinct numbers over 114 invocations**, pinned by `census.rs`, which fails if any census
  number lacks a row and checks every `unexercised` label against the census in both directions.
  Two of the spec's open questions were settled by measurement rather than by rule. The corpora
  issue **four** `ioctl` requests — `FIODTYPE`, `TIOCGWINSZ`, `TIOCGETA`, and dyld's
  `DTRACEHIOC_ADDDOF`, whose 8-byte `_IOW` parameter *is* a guest pointer — and the last was
  measured `ret=0xe err=true` (EFAULT) on all ten guests probed, so its nested copyout is
  unreachable today; and all seven non-null `sysctl` `newp` calls are libc's `name2oid` idiom (MIB
  `{0,3}`, `newlen` 10..=32, the name's own length), bounded by xnu's `newlen >= MAXPATHLEN`
  rejection, so `newp` is `Ptr` and `sysctl`'s `KERN_PROC_ALL` `Dest` keeps its canary. The sweep
  re-baseline after all of it: `TALLY pass=46 fail=8 skip=0`, same PASS set, same FAIL set,
  **nothing moved** — and Known limits says what the four binaries the new classifications touch
  actually did, because the sweep's PASS cannot. Nothing new is recorded and `TRACE_MAGIC` did not
  move: the views are pure functions of `num`, the loud failure is record-only by construction, and
  the one behavioural change — descriptors translated for sixteen more syscalls — changes what the
  host kernel sees, never what is recorded.
- **`dup2` is modelled, and a `Scalar` register is never probed.** Since M37 — the two class-B
  walls M36's table routed, and the last milestone of the M32–M38 run. `FdTable` gained a slot
  kind, `FdSlot::Console(u8)` — "this slot is (an alias of) console descriptor `n`" — so the M9
  console mirror is decided by the slot's *kind* through one shared predicate per question
  (`Box_::is_console_write`, `Box_::is_console_close`) rather than by the number 1 or 2: after
  `dup2(1, 17)` a write to 17 is mirrored into the trace and faked, and after `dup2(f, 1)` a write
  to 1 reaches `f`'s file. `FdTable::dup2` is the pure table operation both sides run (record
  with a host `dup`, replay with none); replay recomputes the `(ret, err)` pair inside the generic
  arm and byte-compares it against the recording — the M10 fd mirror's own posture, no new
  returning arm, `verify_thread`'s seven sites unchanged, and a test that tampers with a recorded
  `dup2` return watches the compare fire. **`dup` copies the slot's kind too** (the M37 fix wave,
  from the final review): `FdTable::dup` gives `dup(1)`'s new slot its source's kind rather than a
  plain `Open`, on both sides through the same method — so a write through a `dup` alias is
  mirrored like one through a `dup2` alias, and the shell's save/restore-stdout idiom
  (`saved = dup(1); dup2(file, 1); …; dup2(saved, 1)`) hands slot 1 its console kind back
  (`dupkind_e2e`). Until that fix the idiom was the M9 class in a new coat: every stdout write
  after the restore was forwarded to a host dup of retrace's own stdout, on the terminal and in
  neither the trace nor the replay, rc 0/0, no divergence — and `main` had been *loud* on it (the
  `dup2` assert), so the branch had turned a loud failure silent. A faked console close now
  retires its slot on **both** sides (a write after `close(1)` is `EBADF` on both, the kernel's
  own answer — M9's deferral retired), an alias closes the generic way — as does an identity slot
  re-aliased by `dup2(saved, 1)`, whose host mapping is a dup — and the target is bounded at
  `DUP2_MAX_FD = 10240`.
  Nothing is forwarded as `dup2`: forwarding it would overwrite retrace's own descriptor. And
  `forward_and_diff` forwards a register whose `arg_kinds` row marks it `Scalar` **verbatim** —
  never probed against the guest's backings — which is the fix M34 §4b named: a pid, a length or
  an offset that happened to equal a mapped guest IPA was being rewritten to a host pointer
  (`scalarprobe` pins it — `lseek(fd, 0x4000, SEEK_SET)` returned the trampoline backing's host
  address before, `0x4000` after). The precondition was an audit of every `Scalar` position (190,
  over 129 rows) against its kernel prototype, statically and over every kept corpus trace; it
  found one wrong row (`madvise`'s `addr`, now `Ptr` — 44 of 44 CPython `madvise`s succeed where
  26 did, because the pre-fix probe had been rewriting the *length* `0x4000`/`0xc000`/`0x10000`)
  and seventeen pointer-*typed* positions correctly kept `Scalar` because the kernel never
  dereferences them. Measured: 0 self-pid `ESRCH` in every kept trace of three full sweeps at
  three pid regimes, where M36 had measured 11–12 per colliding trace. Nothing new is recorded and
  `TRACE_MAGIC` did not move: `FdSlot` is box state, never traced, and the skip changes what the
  host kernel is *asked*, never what is recorded or compared.
- **`pipe`'s pair reaches the guest, `F_DUPFD` is modelled, `AT_FDCWD` is honoured, and two
  forwards are refusals.** Since M38 — five items off the owed list, none a new subsystem, each
  with a fixture that asserts on the difference it makes. `Event::Syscall` carries **`ret1`**, the
  second return register, and `TRACE_MAGIC` moved to `RT\x00\x0a` for it; `host_svc` captures
  `x1`, and on a `pipe` both host ends are bound as guest descriptors (`Box_::bind_returned_pair`,
  read end first, xnu's `retval[0]`/`retval[1]` order) so the guest sees two adjacent guest
  numbers — `(4, 5)` in `csh` — where it used to see retrace's raw host read-end and its own
  stale `x1` — replay does the same two `alloc`s in
  the fd mirror and compares the pair, and `Box_::set_ret1` is called on both sides under the
  same predicate (`returns_fd_pair`), so `x1` is set identically; the capture is **narrow** (`x1`
  written for `pipe` only, spec R2). `fcntl`/`ioctl` third-argument kinds follow the *command*
  (`shape_of`: `F_SETFD`/`F_SETFL`/`F_DUPFD`… are `Scalar`, never probed; `F_GETPATH`/
  `F_PREALLOCATE`… stay `Ptr`; an unlisted command keeps `Ptr`, spec R5), and `F_DUPFD`/
  `F_DUPFD_CLOEXEC` is a table operation like `dup2` — `FdTable::dup_from(src, min)` takes the
  lowest free **guest** slot ≥ `min` with the source's kind, on both sides, with a host `dup`
  behind it on record and never a host `F_DUPFD` (whose minimum would be a host number);
  `dupfd_e2e` pins `fcntl(fd, F_DUPFD, 10) == 10`. `translate_fds` tests the sentinel as the ABI
  delivers it — `(v as i32) < 0`, so `0xfffffffe` is `AT_FDCWD` and not a descriptor — which is
  what un-broke `/bin/ls`'s and `/bin/ed`'s `fstatat64` and moved both to the walls behind them.
  `execve`/`posix_spawn` are **refused** in a record arm ahead of the generic forward, with the
  errno the forward had been returning (`EFAULT`, measured on both — continuity, not fidelity,
  spec R4) and a stderr line only the refusal prints, which is what `exec_e2e` and the CPython
  launcher test assert on; a forwarded exec that ever *succeeded* would have replaced retrace's
  own process. And the receive-shaped message-queue `mach_msg2` that parked six sweep rows is
  refused too, with a code **chosen by measurement** over the six (`MACH_RCV_INVALID_NAME` — the
  only one all six accept, 6/6 against 5/6 for the spec's `TIMED_OUT` default, which crashes
  `dddiagnose` in the guest): `launchctl` runs to its own clean exit and is un-parked; the other
  five run on to a syscall with no `arg_kinds` row and are re-parked there, class B. Every mirror
  sits inside an existing arm — no new returning arm, `verify_thread`'s seven sites unchanged.
- **`import ctypes` works, and a real script that crashes records, replays and reverse-debugs.**
  Rung 8, since M39. The interpreter runs `crates/retrace-guest/py/crash.py`, a script file that
  `json.load`s a data file beside it, computes `0x4000_DEAD_0000` from hex strings in that file
  (never a literal in the script), prints the address of the `ctypes` pointer object's own buffer,
  and derefs. The run records, replays byte-identically twice, and reverse-continues from the fault
  to the store that wrote the bad pointer. The store is asserted by its **effect**, never by a
  symbol — `cast()` is static inside `_ctypes.so`, which M19's symbolication does not reach — so
  `cpython_crash_e2e` asserts that the watched cell does *not* hold the target at the
  `reverse-continue` stop and *does* hold it one `stepi` later. The crash is asserted on the trace
  too: the terminal `Event::Crash` carries the computed target as its `far`, DFSC `0x05` (a level-1
  translation fault — bit 46 of the VA selects an L1 slot that was never mapped, the same face
  `crashy.c` shows at the same address), and the thread tag of the landmark that wrote the marker.
  Never on exit 139 **alone**, which a guest that died inside dyld produces identically — the gate
  does check the code, on the record and on each replay, but only beside those trace assertions.
  **Exactly one wall stood between rung 7 and rung 8** — the spec budgeted six — and the walk
  cleared it and met no second: `mach_vm_remap` (`msgh_id` 4813), which libffi's Apple trampoline
  table issues on every `import ctypes` to alias `libffi-trampolines.dylib`'s freshly-placed
  `__TEXT` into a region it `vm_allocate`d, shared (`copy = FALSE`), `FIXED|OVERWRITE`. It is
  serviced as a **stage-1 alias** in the `mach_vm_map` (4811) posture: a route, a pure decoder, one
  `Box_` method, a pure reply encoder — record synthesises the 60-byte reply and replay recomputes
  it through the *same* method and byte-compares before applying, so an asymmetry surfaces as a
  divergence. `Box_::guest_vm_remap` copies each source page's live L3 descriptor into the target's
  L3 slot — **the first non-identity stage-1 entries the box writes** — and then calls
  `flush_guest_tlb`, because M9's rule is that a stale RW/UXN entry under a now-executable page
  must be invalidated by the guest's own `tlbi` before it is executed. The reply's protections are
  **derived from the live tables, not chosen**: `cur` from the source page's stage-1 attribute
  (`ATTR_CODE` → 5, `ATTR_DATA` → 3, `ATTR_NONE` → 0, anything else panics by name), `max` from the
  source's band (a kernel-placed image — the executable, dyld, the shared cache — → 5; guest-allocated
  memory at or above the nano band → 7). That is a pure function of the address and the live tables,
  so record, replay and a checkpoint seek recompute it identically with no new box state — and it
  reproduces *both* numbers a native probe measured, where one constant pair could not: `cur=5 max=5`
  aliasing a program's own `r-x` text, `cur=5 max=7` aliasing the `dlopen`'d trampoline dylib, whose
  `__TEXT` carries an elevated max precisely so it can hand out writable JIT sub-mappings under W^X.
  Two shapes the band rule would answer wrongly are unmodelled and named at the method: a guest
  `FIXED` mmap below the nano band, and a read-only `MAP_SHARED` source above it. Neither is
  measured and no known caller issues either. `copy = TRUE`, `VM_FLAGS_ANYWHERE`, a `src_task` that
  is not the guest's own, and a target overlapping its source each **assert by name**, on both
  sides. `vmremap_e2e` is the repo-owned guard — a dynamically-linked C fixture that remaps its own
  text page and *calls through* the alias, then remaps a dylib's text and `memcmp`s through it — so the
  mechanism is guarded on a machine without Homebrew Python, where `cpython_crash_e2e` skips loud
  and guards nothing. The route sits inside the existing `mach_msg2` arm, after its oracle call, so
  `verify_thread`'s **seven** sites are unchanged and `TRACE_MAGIC` did not move.

  **Reverse-debugging CPython.** Since M40 this is a session that takes seconds. M39's demo
  script, run standalone on a fresh recording of `crash.py` (record and replay each exit 139),
  prints, verbatim:

  ```
  > continue
  guest crashed: pc=0xa017dee60 far=0x4000dead0000 esr=0x92000005
  > watch 0xa016daac8 8
  watch at 0xa016daac8 len 8
  > reverse-continue
  hit watch 0xa016daac8 (write at 0xa017da45c) at (1136, 127306)
  > where
  at (1136, 127306) pc=0xa017da45c thread=0
  > x 0xa016daac8 8
  0xa016daac8: 00 00 00 00 00 00 00 00
  > stepi
  > x 0xa016daac8 8
  0xa016daac8: 00 00 ad de 00 40 00 00
  ```

  The watch stops on the store **pre-retire**, so the cell still reads zero; one `stepi` retires it,
  and the cell then holds `0x4000dead0000` little-endian — the `far` the crash reported, which the
  script computed from its data file. No symbol follows the pcs because `cast()` is static inside
  `_ctypes.so` (Known limits). The addresses and coordinates are this recording's: M40's t0
  recording of the same script put the cell at `0xa01722ac8` and its last writer at
  `(1143, 125704)`. **CPU:** on that t0 recording (97.6 MB, 1,146 events), the same debug session
  with and without `reverse-continue` took 8.64 s and 8.11 s of CPU, so the command itself costs
  **0.53 s** — where M39 attributed 3.42 h of wall-clock to it — at a load average of 1.45–1.92.
  **Memory:** that session peaked at **422,379,520 B (≈ 403 MB)** resident, where M39's standalone
  attempt at this demo had peaked at 7.41 GB RSS, and swap in use had grown to 33.9 GB, before it
  was stopped without a result (M39 R17). The demo run's own 10-second RSS sampler recorded
  nothing, because the session finished before its first sample. Forward `continue` with the watch
  armed names the real write too: on the t0 recording it resolves `(1126, 1765682)` and one `stepi`
  sets the cell to `0x701238000`, where M39's tree resolved `(1126, 29627)` — an earlier run of the
  same store on another address, 1,736,055 instructions early.

**Gate:** 639 passed / 0 failed / 9 ignored across 140 test binaries, **measured at M40** over the
whole workspace, every chunk `EXIT=0` (captured before any pipe); clippy clean over
`--workspace --all-targets` with `-D warnings`. Measured on commit `5cea9a8`, the head after the
six implementing tasks and their fix rounds; the commit that follows it is this close's
documentation and changes nothing under `crates/`. The whole chunked run took 17 min 8 s of
wall-clock, `cpython_crash_e2e` 41.28 s of it. See the testing note below for how that number is
assembled. The "test binaries" figure is test executables plus the `Doc-tests` harnesses cargo
reports, each of which runs zero tests — the convention every milestone since M14 has counted by,
kept for comparability and written out here so nobody has to re-derive it. The ignored gates are
**nine**, and they are M38's nine unchanged: the two long-standing — `stackoverflow_rust_e2e`
(re-parked by M21 at a signal-model wall, **not** the M8 risk R3 wall it stood at from M8 through
M20) and `cache_symbol_e2e` (the M19 shared-cache symbol wall) — plus the seven in
`apple_walls_e2e`, one per non-clean Apple-sweep row that is retrace's to fix or model and has a
gate, each reason the measurement that parks it. All nine are described under Known limits. **M40
parked nothing new and un-parked nothing** — it changed the debugger's positioning and `Box_`'s
memory ownership, not what records — so the seven Apple rows stand exactly where M38 left them.

Reconciled against M39's 629 / 0 / 9 over 138 **file-by-file rather than by sum** — five files
changed their count, everything else is byte-for-byte M39's:

| file | M39 | M40 | delta |
|---|---|---|---|
| `retrace-guest/src/lib.rs` | 18 | 19 | **+1** (`watchsweep_guest_parses`) |
| `retrace-box/tests/backingfree.rs` | — | 1 | **+1**, new binary (a dropped `Box_` releases every byte of guest backing it allocated, and `guest_munmap` releases exactly its region) |
| `retrace/src/debug.rs` | 11 | 14 | **+3** (one `reverse-continue` makes at most three seeks whatever the hits; a debug session decodes its trace once; `watched_of` names the range a wider store covers) |
| `retrace/tests/watchsweep_e2e.rs` | — | 3 | **+3**, new binary (`continue` resolves a watch hit to the write, not an earlier run of the store; `reverse-continue` walks back through both writers; a watch-aware step stops pre-retire exactly at the watched writes) |
| `retrace/tests/watch_cli.rs` | 9 | 11 | **+2** (a `reverse-continue` from a syscall hit finds nothing earlier; a `reverse-continue` crosses a breakpointed `svc` and resolves ordinal two) |

+10 `#[test]` attributes, `#[ignore]` **9 → 9**, `--bins` **11 → 14**, and **two new test
binaries**, 138 → 140. The count closes at both ends, and the two ends must still be read
separately: the tree holds **646** `#[test]` attributes = 637 runnable + 9 ignored (M39 held 636 =
627 + 9), while the run reports 637 + the 2 census tests that run twice (`census.rs` executes in its
own binary and again inside `legacy_equivalence`'s `#[path]` include). (A bare
`grep -c '#\[test\]'` says 647, because a comment in `legacy_equivalence.rs` mentions the attribute
in prose; the file has three.) The spec's §9 prediction, ≈ 636 / 0 / 9 over 139, was short by three
tests and one binary: it did not count `watchsweep_guest_parses`, the two `watch_cli` guards Task 3's
review added, or the leak test's own binary.

`retrace-box` ran as a **whole package**, so its `Doc-tests` harness could not be dropped (M24's
lesson), and that is also what picks up the new `tests/backingfree.rs` target without a per-target
entry. `retrace` ran **per-target** — seventy-two `--test <name>` invocations, one after another
from a single background script, because the whole package exceeds the tool ceiling — **plus the
`--bins` chunk**, which is the only place the 14 unit tests in `crates/retrace/src/debug.rs` run;
the binaries count includes it. The two mouths of the same trap, one loud and one silent, both
closed by construction of the chunk list.

One timing trap is worth knowing before it is mistaken for a hang: `bigread_e2e` took **536s** on its
first run and **47s** on its second, with the recording process sitting at 0:00.00 CPU throughout the
stall. That is first-execution codesign validation of a freshly signed binary, not a hung guest. The
second number is the honest one.

**Trace format:** `TRACE_MAGIC` is `RT\x00\x0a`, moved by **M38** for `ret1` — `Event::Syscall`
gained the second return register, a change to the record's bytes and so a format break — and
before that by **M24**. Recordings from before M38 are
rejected whole, at `Reader::open_checked`, before a single byte of them is trusted (the reader is
tested against both `RT\x00\x02` and `RT\x00\x09`). M24's reason is the lesson worth keeping: M23 had changed
the vector table's padding — which lives in the trampoline page and is therefore snapshot *content* —
without moving the magic, so a pre-M23 recording still opened and `Box_::restore` faithfully restored
its **old** zero padding while the current code assumed trapping padding, reproducing the exact
`pc=0x4204` misattribution M23 removed. M24 closes that at the layer it belongs to. The lesson is in
the rule now: the repo's written rule covered changing `Event`'s *shape*, and this was a change to
what a snapshot's bytes *mean*, which a shape rule cannot see. Both are format breaks and both bump
the magic.

## Known limits

These are real and current, not aspirational gaps.

- **Ten rows of 54 sampled Apple system binaries are not clean, and since M36 the sweep says
  why each one is not — since M37, the same why from any recorder pid; since M38, two of the ten
  are rows that used to "pass" by failing identically.** `tools/apple-sweep.sh`,
  over the committed 54-entry corpus `tools/apple-sweep-binaries.txt`, records and replays each binary and prints a `TALLY` line, so
  since M29 this figure is **reproducible instead of remembered**. Since M36 the sweep also prints
  *why*: every row carries the recorder's exit code, the replay's, the recorder's pid, the first
  `RECORD ERROR:` / `panicked at` line and the replay's `DIVERGENCE` line (one machine-readable
  `ROW` line per binary beside the human one); a refused recording — the recorder's `RECORD
  ERROR:` line is the test, its exit code printed beside it — is labelled `FAIL … (record error,
  rc=4: …)` rather than `replay diverged`; an identical crash on both sides is labelled
  `PASS … (identical fault, rc=N)` rather than a bare PASS; `RETRACE_SWEEP_KEEP=<dir>` keeps
  each non-clean row's stderr and trace, and since M37 `RETRACE_SWEEP_KEEP_ALL=1` keeps every
  row's, PASS rows included — what the M37 audits were run over. **M39 ran it once on 2026-09-21**
  on the close's binary (branch commit `123cb97`, recorder pids 29138–32207 = `0x71d2`–`0x7dcf`)
  and tallied **`pass=44 fail=10 skip=0`** with **no row changing its label** against M38's run;
  M38 had run it once on 2026-09-16 (branch commit `911214e`, recorder pids
  `0x1564a`–`0x15faa`) for the same tally — one regime each time, because M37 had already shown
  the pid selects nothing; M37 had run it three times on 2026-09-13
  with the recorder's pid steered into the three regimes M36 measured and tallied 45/9 in all
  three with the same nine labels in every regime — the acceptance measurement the §4b fix owed,
  and the reason one regime is enough now:

  | run | recorder pids | regime | `TALLY` |
  |---|---|---|---|
  | M37 N | 765–3291 (`0x2fd`–`0xcdb`) | below `0x4000` — non-colliding before M37 too | `pass=45 fail=9 skip=0` |
  | M37 I | 17124–20042 (`0x42e4`–`0x4e4a`) | inside `[0x4000, 0x10000)`, the trampoline page — colliding before M37 | `pass=45 fail=9 skip=0` |
  | M37 S | 66163–68793 (`0x10273`–`0x10cb9`) | inside `[0x10000, 0x18000)`, the guest's own `os_alloc_once` slab — colliding before M37 | `pass=45 fail=9 skip=0` |
  | M38 | 87626–90026 (`0x1564a`–`0x15faa`) | inside `[0x10000, 0x18000)` again — irrelevant since M37 | `pass=44 fail=10 skip=0` |
  | **M39** | 29138–32207 (`0x71d2`–`0x7dcf`) | inside `[0x4000, 0x10000)`, the trampoline page again — irrelevant since M37 | `pass=44 fail=10 skip=0` |

  **M39's run moved no row's label**, and the reason it was expected not to is worth stating:
  its one wall is reached by `import ctypes`, and no binary in the corpus loads `_ctypes.so`.
  Nine rows differ in a field that is not a label — seven carry a fresh panic thread id and a
  source line number that moved inside M38's own close (M38 swept `911214e` and did not
  re-sweep after `cbc75ff`), and `csh`/`tcsh` moved their divergence landmark up ten and down
  three, inside the run-to-run spread M37 measured. `docs/sweep-evidence/2026-09-17-m39/README.md`
  has both measurements.

  Against M37's run N, **46 rows are unchanged and 8 moved**, every one for a reason M38 made
  (M38's own evidence README has that row-by-row diff): `launchctl` FAIL → PASS (the receive-shaped
  `mach_msg2` is refused, it runs to its own usage exit); `automationmodetool`, `desdp`,
  `dyld_info`, `flex`, `dddiagnose` from that receive to the first syscall behind it with no
  `arg_kinds` row (rc 101, the M33 fail-loud); and `ls` and `ed` from a false PASS to the same
  kind of wall (below). So 44 = 45 − `ls` − `ed` + `launchctl`. The ten rows that are not clean,
  each with the class the M32–M38 charter's enum gives it, read off the kept evidence and never
  off the sweep's label, the gate that stands for it and where it is routed:

  | binary | face | class | gate (`crates/retrace/tests/apple_walls_e2e.rs`) | route |
  |---|---|---|---|---|
  | `/bin/csh` | `fork` — `mach_ports_register` | **C** new subsystem: process creation | `csh_records_and_replays` | parked, not routed |
  | `/bin/tcsh` | `fork` — `mach_ports_register` | **C** | `tcsh_records_and_replays` | parked, not routed |
  | `/usr/bin/automationmodetool` | no row for `kevent_qos` (374) | **B** known-unmodelled (a row closes it; libdispatch's kevent workloop may be a subsystem behind it) | `automationmodetool_records_and_replays` | parked at the row, owed |
  | `/usr/bin/desdp` | no row for `openat_nocancel` (464) | **B** | `desdp_records_and_replays` | parked at the row, owed |
  | `/usr/bin/dyld_info` | no row for `openat_nocancel` (464) | **B** (one hard-linked xcrun stub with `desdp`/`flex`) | `dyld_info_records_and_replays` | parked at the row, owed |
  | `/usr/bin/flex` | no row for `openat_nocancel` (464) | **B** | `flex_records_and_replays` | parked at the row, owed |
  | `/usr/bin/dddiagnose` | no row for `statfs64` (345) | **B** | `dddiagnose_records_and_replays` | parked at the row, owed |
  | `/bin/ls` | no row for `getattrlistbulk` (461) | **B** — new to the list at M38, a false PASS before | none (its row was never parked) | owed |
  | `/bin/ed` | no row for `openat_nocancel` (464) | **B** — new to the list at M38, a false PASS before | none | owed |
  | `/usr/bin/yes` | 30 s watchdog | **D** not-a-defect | none | retired |

  `/bin/launchctl` left the table at M38: it is `PASS` 1/1, its own no-argument usage on stdout
  (4,484 bytes, byte-identical to the host's native output), and its gate runs, asserting on that
  outcome rather than on `rc == 0`.

  The faces, each in the recorder's own words. **A missing row** is `recorder panicked: thread
  'main' … panicked at crates/retrace-arch/src/lib.rs:944:38: M33: syscall N (N) has no arg_kinds
  row in crates/retrace-arch/src/lib.rs — it cannot be forwarded unclassified …`, rc 101, no
  replay run (the harness labels a recorder panic before replaying): the M33 fail-loud doing its
  job on a syscall the census never saw, reached by seven rows since M38 — five because the
  receive-shaped `mach_msg2` that used to stop them is now refused (`MACH_RCV_INVALID_NAME`,
  chosen by measurement, the one code all six accepted) and they run 20–50 landmarks further, two
  (`ls`, `ed`) because their `fstatat64(AT_FDCWD, …)` succeeds now and they run on. The missing
  rows are **five numbers** — 461 `getattrlistbulk` (`ls`), 468 `getattrlistat` (M34's pair to
  it), 464 `openat_nocancel` (`ed`, `desdp`, `dyld_info`, `flex` — **four corpus binaries behind
  one row**, the `_nocancel` twin of `openat` 463, precisely the documented nocancel trap, which
  is what sharpens the successor's case: one line frees four rows), 345 `statfs64` (`dddiagnose` — the
  fixed-struct twin of `fstatfs64`, M29), 374 `kevent_qos` (`automationmodetool`) — the successor's
  measured scope, at the top of the owed list. **`fork`** is `record error, rc=4: RECORD ERROR:
  unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515))
  send_size 64`, rc/rp 4/3: `mach_ports_register` (`task.defs` 3400+3, a complex message with
  three port descriptors the router does not know) from libxpc `xpc_atfork_prepare` ←
  `libSystem_atfork_prepare` ← libsystem_c `fork+0x24` — `fork`'s own pre-fork hook, and behind it
  `fork`(2) itself, which has no `arg_kinds` row. Before M37 these two rows stopped ~60 landmarks
  earlier at the M33 assert that refused `dup2` by name (rc 101); the fd table models `dup2` now —
  both shells issue exactly four, `dup2(0,16)`, `(1,17)`, `(2,18)`, `(16,19)`, the C shell's
  classic descriptor move, and all four record and succeed — and that is what moved the wall to
  `fork`. A few landmarks before it both shells `pipe`, and since M38 receive a bound pair
  (`(4, 5)`), move each end above `FSAFE` with `dup`/`close` and `fcntl(F_SETFD)` both moved ends
  successfully — where M37 had them `fcntl` a raw host descriptor and a stale register to
  `EBADF`; the wall is unchanged. The **RCV shape** — `RECORD ERROR: unsupported mach_msg2 at pc
  0x1804adc34: options 0x404000102: …`, rc/rp 4/3, the pc `mach_msg2_trap+8` — is **history since
  M38**: a *receive*-shaped message-queue call (`MACH64_SEND_MQ_CALL | MACH64_RCV_MSG`, no
  `MACH64_SEND_MSG`) that `Route::Unsupported` kept fail-loud from M35 (first seen on
  `dddiagnose`) through M37, reached by six binaries from every pid, and refused deterministically
  by `Route::RefuseMqRecv` since M38 (nothing written, the constant returned, replay recomputes
  and byte-compares — the `RefuseMqSend` posture); *modelling* the receive is still class C, and
  a binary that needs a real reply on a port it holds would move one wall past the refusal. Before
  M37 a colliding pid took one
  of two other faces first — libdispatch's `brk #1` in `_firehose_task_buffer_init+0x12c` on a
  `proc_info(2, <recorder pid>, 17)` answered `ESRCH`, or `dddiagnose`'s identical malloc crash
  (`mfm_alloc+0x230`, `rc=139` both sides) — both downstream of M34 §4b's pid mis-translation;
  neither recurred in any M37 run (0 `identical fault` rows in three sweeps; 0 self-pid `ESRCH` in
  every kept trace, 12–13 pid-carrying calls per row all succeeding, where M36 had 11–12 `ESRCH`).
  The **watchdog** is `timed out after 30s recording`: `yes` never terminates and is failed on
  purpose. The `refusing mach_msg2 message-queue send` line that precedes the receive is M23's
  *serviced* refusal of the SEND|RCV shape, survived by every guest that reaches it and
  unseparated from the receive (no run reaches the receive without it); since M38 a second line,
  `refusing mach_msg2 message-queue receive`, follows it on the six rows. Evidence:
  `docs/sweep-evidence/2026-09-16-m38/` — the refusal-code measurement (18 cells, all three
  candidates' stderr), the M38 sweep log and every non-clean row's stderr under `sweep/`, and the
  `/bin/ed` and `csh`/`tcsh` traced runs; `docs/sweep-evidence/2026-09-13-m37/<basename>.{N,I,S}.{rec,rp}.err`,
  verbatim, with the counting rules, the reader and the three audits in that directory's README;
  the pre-fix faces, the symbolication and the M36 counting rules are in
  `docs/sweep-evidence/2026-09-13-m36/`.
  The old label "replay diverged" appears in no gate reason and in none of the table's cells: the
  replay of a recording that ended at a `RECORD ERROR` *always* reports a `DIVERGENCE` — it runs out
  of events one past the trace's last syscall — and in every cell where a replay ran (24 of 24
  `rc=4` traces, `events == landmark` in each) record and replay agreed, the replay re-reporting
  the recorder's own stop.
  **The pid-collision probe is retired: a `Scalar`-marked register is never probed — M37.** The
  window `[0x4000, 0x18000)` and its 82 % of the pid space, the `brk`, the identical crash and its
  varying `far`, and which of M35's and M36's runs fell where are history, in `docs/status-log.md`
  (M34 §4b, M36's corrections (a)–(c), M37's acceptance); the two open questions M36 attached to
  the probe (why a colliding run took the crash rather than the `brk`; the `far`) are retired with
  it, since neither face can now be reached. What the retirement leaves is in the descriptor entry
  below: positions past a row's arity keep the probe on purpose, and a `Ptr` that is sometimes a
  number is the same class behind a different kind.
  **History, kept short.** The corpus is a **reconstruction**: the sample behind the 47 published
  at M27 (46 at M23, 34 at M22) was never committed, so today's figures are not strictly comparable
  to those; they are simply the first a later reader can re-derive. `/bin/launchctl` had always
  been diverging until M38 — the script's first draft compared a variable against itself, making
  its exit-code check a tautology that reported four binaries as passing when they were not, and
  fixing it is what exposed `launchctl`; the receive refusal is what cleared it. `/usr/bin/yes`
  cannot pass under any method that requires a bounded
  comparison and is counted a FAIL **on purpose**, since excluding it would raise the tally without
  changing anything about retrace. The `identical fault` rows are still counted in `pass` so the
  tally series 46/8 ↔ 45/9 ↔ 44/10 stays comparable across M33–M38 (none occurred at M37 or M38);
  the label on the line is the correction.
  M22's four named causes are all accounted for — the `pc=0x4204` group (13) and the `msgh_id` 412
  group (4) were cleared at M23 (the 13-group's residue, the `brk`, was M36's colliding-pid face of
  the six RCV rows above, retired with §4b at M37), the `dup2` pair is the `csh`/`tcsh` rows
  (`dup2` modelled at M37; their wall is `fork`), and **`ps` was fixed at M27**. It was published here
  from M22 through M26 as "the oracle catching nondeterminism", a claim that could not have been
  true, since replay never *executes* a syscall, only applies recorded
  writes, so a process list cannot vary between the two runs; M26 corrected the *description* (the
  real cause is the truncating diff window) without closing it, and M27 closed it: `ps` sizes its
  `sysctl(KERN_PROC_ALL)` buffer at 205,416 bytes, `retrace_arch::dest_buffer` now knows that length
  lives at `*(size_t*)x3`, and the window widens to cover the whole reply. Separately — and this is a
  different eight from the rows above — eight of the 54 report a **nonzero** fall-through count
  that record and replay agree on: the first binaries ever to exercise that invariant at all.
  **A PASS here is record/replay agreement, not correctness**, and M33 measured what that hides
  on two rows that M38 then un-hid: `ls` and `ed` "passed" in every sweep of the committed
  corpus from M33 through M37 by failing identically on an `AT_FDCWD` the fd table rejected
  (the defect itself dates from M10 t3; the descriptor entry below); with the
  sentinel honoured each runs on to a missing row and fails loud, which is why the tally *fell*
  by two at a milestone that fixed a defect. An M10-class wrong descriptor is deterministic on
  both sides, so a translation fix moves a binary here only by letting it reach something else —
  here, two rows the census never saw.
- **A guest must be arm64 or arm64e.** `slice_native` picks the slice this machine would execute —
  arm64e if the file has one, else plain arm64 — so universal files work, but an `x86_64`-only
  binary is refused by name. There is no emulation of another ISA and none is planned.
- **The record-side diff window still truncates for most syscalls. Since M30 the guard band catches
  a kernel write whose *effect* it cannot see — but only on the bands it is allowed to fill, and that
  exclusion is large.** `forward_and_diff`
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
  the live kernel with libc's 5-arg wrapper bypassed.
  **M34 closes that list**, by two mechanisms: `proc_info` (336) and `csops`/`csops_audittoken`
  (169/170) gained `Dest` rows — their blob and list callnums are bounded only by the caller's
  length, so the window now follows it and the forwarded count is clamped to the backing; and
  `getattrlist`/`fgetattrlist` (220/228) turned out not to belong on the list at all, because the
  kernel rejects with `ENOMEM` before writing anything when the packed result exceeds
  `ATTR_MAX_BUFFER_LONGPATHS` (15,360 bytes) — a cited bound four times inside the window, so they
  stay `Ptr` and a test pins them there. The corpus maximum across all five, measured 2026-09-13
  over 851 dispatches from 76 guests, is 1,052 bytes: both new rows are inert for the window today.
  The clamp is **not** inert: on every dynamic guest (76 of 76) dyld's `proc_info(SET_DYLD_IMAGES)`
  destination `0x1ec6f7f80` sits `0x3f80` into a shared-cache page whose backing is that one 16 KiB
  page, so the forwarded `buffersize` is rewritten 368 → 128 — measured through the
  `RETRACE_REGCLAMP=1` channel M34's fix wave added to the `Reg` arm (`[M34 REGCLAMP] syscall 336
  count 368 avail 128 dest 0x1ec6f7f80 backing [0x1ec6f4000,0x1ec6f8000)`, once per run on
  `hello_dyn` and `jq`). No recorded byte changes: that call transfers nothing, and from inside
  retrace the kernel rejects it before reading the size (see the owed list later in this section).
  Every `csops` destination and every other `proc_info` destination is on the dyn stack and fits.
  Measuring that also found a defect outside M34's scope, recorded later in this section (the
  pid-collision probe).
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
  `--nocapture` run, which reported **32** on one run and **30** on another of the same tree. The
  count is therefore not a property of the machine, and neither it nor its distance from M28's
  hand-counted 31 is **root-caused** — nothing measured which call produces a given suppression.
  That is precisely why the assertion is `> 0` rather than any exact number: what fails should be a
  portable property, and this milestone does not know what makes the count vary.
  **The false negative M27 measured is closed on a filled band, and M30 closed it by changing the
  question rather than the size.** The measurement that forced this: before the `sysctl` fix landed,
  `ps`'s own overrun was the band's would-be catch — the kernel wrote 139,880 bytes past the window
  and the band did not fire, because `struct kinfo_proc` carries long zero runs and the window
  boundary landed inside one. The kernel wrote **zeros over zeros**, which a pre/post *comparison*
  cannot distinguish from no write at all; `/bin/ps` passes today because `dest_buffer` covers
  `sysctl`, never because the band caught it. M30 fills each shrunk band with
  `canary_byte(ipa) = (ipa as u8) ^ 0xA5` before the forward, asks `Box_::canary_intact` after, and
  restores it before the guest resumes, so a kernel write over the band destroys a pattern retrace
  itself placed, whatever bytes it wrote — subject only to residual (4) below — and **aborts the
  recording**. `GUARD_BAND` is unchanged at **64**.
  The address-derived pattern buys two things a constant would not: two overlapping bands agree on
  every byte they share, and a constant-value `memset` cannot reproduce it. `truncguard.rs` carries
  the before/after pair at one fixture and one cap — the same real trap-189 overrun that the old
  comparison could not see now aborts — and `canary.rs`'s `zeros_written_over_the_band_are_caught`
  states the blindness itself as two pure predicates over identical all-zero buffers.
  **Four things keep a silent band from being proof of absence, and none of them is small.**
  *(1)* The canary is not filled for `retrace_arch::reads_guest_buffer` —
  `write`/`pwrite`/`writev`, the `send*` family, `sendfile`, `msync` and **`mach_msg2_trap`**, each
  with its `_nocancel` spelling — because the kernel reads *through* those buffers and would consume
  the canary as data. Filling them was measured to corrupt the guest's own output: a 128 KiB-write
  guest produced a file with **64 corrupted bytes matching `canary_byte` exactly** while record
  exited 0, replay exited 0 and the canary count read 0 — record and replay agreeing while the
  guest's output was wrong, the one failure a determinism oracle cannot see. `bigwrite_e2e` is the
  repo-owned reproduction, and it went green with the fix reverted until its guest was changed to
  write to a **file** rather than to fd 1, which `is_console_write` mirrors and never forwards.
  That family keeps `overran_window` **bit-for-bit**, so it is exactly as strong as M27 left it and
  no stronger — a fix-round-2 correction, since running the canary question over an *unfilled* band
  would have made it strictly *weaker* than M27 by that same 1/256.
  *(2)* Two of those exclusions cost real **destination-side** coverage: `sendfile`'s 4th argument is
  an in-out `off_t *` the kernel writes the transferred count back through, and `mach_msg2`'s receive
  buffer is a live destination — `machmsg.rs`'s `FORWARD_ALLOWLIST` forwards five ids through
  `forward_and_diff` (`host_info` 200, `host_get_clock_service` 206, `semaphore_create` 3418,
  `task_info` 3405, `host_get_special_port` 412), and the last two exist *precisely* because the
  kernel writes a reply into guest memory — traffic every jq and CPython run exercises. Recovering it
  needs a per-**argument** direction notion. Since M33 that notion **exists** — `arg_kinds` says,
  per register, whether an argument is a `Source` — but nothing consults it per argument yet: the
  fill decision is still the whole-syscall `reads_guest_buffer` view, because the stale-register
  reproduction (a filled band reached through a *different* register's pointer into the same
  buffer — `bigwrite_e2e` sets `x4 = buf + 128` deliberately to stand for the stale case) is what
  forced the whole-syscall exclusion, and nothing has re-measured that against a
  per-argument fill. The coverage is still **owed**; the table it needs is not.
  **M32 went to build that entry and measured it inert instead, so the gap above is still open and is
  now known to cost nothing observable today.** The exclusion is unchanged — `mach_msg2`'s band is
  still never filled — but what would change if it were filled has been measured rather than
  guessed. A band exists only where a buffer sits more than `window_cap` (65,536) bytes from the end
  of its backing; M32 walked **35 real `mach_msg2` landmarks** across `hello_dyn`, `jq` and CPython,
  classified each by calling the production `machmsg::route()` rather than a copy of its allow-list,
  and found **13 governed** (`Route::Forward`, the only route `forward_and_diff` ever runs for) with
  a **maximum `avail` of 24,672 bytes** and **zero bands**. The entry would have shipped inert.
  That is structural, not luck: all five forwarded ids are MIG-generated kernel-RPC stubs, and a MIG
  stub builds its `union { Request; Reply; }` as a **stack local**, so a governed call's `avail` is
  its stack depth measured from the top of whichever stack it runs on — and every stack retrace
  builds puts the buffer below the top (main 256 KiB, a pthread stack the guest's own mmap, a
  workqueue worker's struct-at-top growing down). The single landmark in the whole corpus that *did*
  carry a nonzero band is the heap-backed libxpc message-queue send `route()` refuses, which
  `forward_and_diff` never sees.
  **The residual is depth, and it is a real one**: nothing bounds the stack depth at which a
  governed id can fire, and all 13 measured calls are process-initialisation calls, shallow by
  construction — a biased population. A `semaphore_create` from a dispatch semaphore built deep in a
  call chain, or a `host_info` behind a `sysconf`, are ordinary, and 64 KiB of frames sits well
  inside a 256 KiB stack. So the finding is mechanistically explained and unlikely to reverse, **not
  proven** — which is why `machmsgband_dyn.rs` **asserts** the maximum governed `avail` stays under
  the threshold rather than only printing it: the first fixture **in that test's own corpus** that
  contradicts this paragraph reds the gate instead of silently ageing it — a new e2e guest added
  elsewhere in the repo is not walked by it and would not. `sendfile`'s half of the gap cannot be measured at all
  here, for the reason M32 re-confirmed by `grep`: nothing in this repo exercises it.
  **M33 built the unification M32 named** — `arg_kinds`, with the five old functions as views and
  an equivalence sweep proving each reproduces its legacy table — so the shape of the owed work is
  settled and the list is now concrete. Owed, each with its reason: **the per-argument canary fill
  and M32's Control 1** (the mechanism that would consult `arg_kinds` per argument; still
  unexecuted, and §4c of the M33 spec measured it **still inert**: the one corpus call that could
  have made it live, a `Source` `newp` on the same `sysctl` whose `KERN_PROC_ALL` `Dest` exceeds
  the window, turned out to be `Ptr`, so no call in the corpus carries both a `Source` and a
  destination the whole-syscall exclusion would cost);
  **the pid-collision probe — retired at M37** (M34 §4b: `forward_and_diff` rewrote *any*
  register whose value landed in a guest backing to a host pointer, a pid, a length or an offset
  included, and record and replay agreed so the oracle could not see it. Since M37 a position the
  `arg_kinds` row marks `Scalar` is forwarded verbatim, never probed; the measured window, its
  two-band acceptance and the faces it produced are history in `docs/status-log.md`. What the
  retirement leaves, on purpose: `Fd` positions and positions *past a row's arity* — `x6`/`x7` on
  a six-argument row, every register on a zero-arity one — keep the probe, because M30 measured
  that a stale register pointing into a live buffer plants a canary 64 KiB past itself and the
  windows those registers open are part of what the band logic reasons about; narrowing the probe
  there is a separate measurement nobody has taken. The `Ptr` position that was sometimes a
  number — `fcntl`/`ioctl` `x2` for argument-less commands such as `F_SETFD`/`F_SETFL`, §4b's
  class behind a different kind, measured inert on the corpus — is **fixed since M38**:
  `shape_of(num, args)` keys the third argument's kind on the command, so the census's scalar
  commands (`F_DUPFD`, `F_GETFD`, `F_SETFD`, `F_GETFL`, `F_SETFL`, `F_NOCACHE`,
  `F_DUPFD_CLOEXEC`; `FIOCLEX`/`FIONCLEX`) are `Scalar` and never probed while the pointer ones
  (`F_PREALLOCATE`, `F_GETPATH`, `F_ADDFILESIGS_RETURN`, `F_CHECK_LV`) stay `Ptr`; a command the
  census has not seen keeps today's `Ptr` on purpose (spec R5 — a fail-loud default would turn
  every unseen command into a new sweep failure for a gap the milestone did not create), so the
  residual is now "an unlisted scalar command that equals a mapped IPA", unreached on the corpus);
  **`SET_DYLD_IMAGES` (336/15) serviced above the trace** (the same `hello_dyn` recording shows
  it returning `EINVAL`, but that is *not* pid-caused — M34's first draft said it was: the
  forwarded call names *retrace's* task, whose dyld info retrace's own dyld already finalised
  (`task_set_dyld_info`'s three-call rule, xnu `osfmk/kern/task.c`), so it fails `EINVAL` with
  any pid and any size, measured natively at pid `0x107dc`; and if it ever could succeed it would
  point retrace's own dyld info at guest memory, so it should be synthesised `0` rather than
  forwarded — the same family as every "on self" `proc_info`/`csops` being answered about
  retrace's process rather than the guest's); **the per-page shared-cache backing** (any `Dest`
  destination in cache DATA that straddles a 16 KiB boundary is clamped at the boundary, because
  cache pages are individual backings — measured at `SET_DYLD_IMAGES` (368 → 128), harmless there
  only because that call transfers nothing; the same geometry would truncate a `CS_OPS_BLOB`, a
  `LISTPIDS` or a `read` into such a buffer — a fidelity hazard that pre-dates M34, whose fix is
  contiguous host backing for the shared-region window, not a table change; no corpus guest does
  it today); **nested-pointer translation** (`NestedSource` rows are
  forwarded exactly as before — `writev`'s `iov_base`s EFAULT in retrace's process, which is how
  `/bin/ed`'s stderr message is lost — and `NestedDest` rows are refused exactly as before);
  **`execve`/`posix_spawn` are refused, not forwarded — since M38** (the fail-loud the
  `bsdthread_create` precedent demanded, in the operator's chosen shape: a record arm ahead of the
  generic forward returns `EFAULT` — the errno the forward had been returning, measured on both
  numbers, chosen for continuity so the CPython launcher's output and `/bin/sh`'s sweep row are
  unchanged, `ENOSYS` the one-constant change if a successor prefers "unmodelled" to be what the
  guest reads — and prints a stderr line the tests assert on; a forwarded exec that ever
  *succeeded* would have replaced retrace's own process, which is what made forwarding it unsafe
  to leave for nested-pointer translation to enable); **console `writev` mirroring** (`Box_::is_console_write`
  covers `write`/`write_nocancel` only, so a `writev` to fd 1/2 — or, since M37, to a `dup2` alias
  of them — is forwarded, not mirrored — M9's class in a new spelling);
  **`__disable_threadsignal` (331)**, forwarded and therefore applied to retrace's own thread; and the fact that **a row's memory kinds are verified by nothing but the reviewer** — the
  equivalence sweep proves the views, not the rows, so the row count is not a coverage claim.
  `Scalar` is the exception since M37: every `Scalar` position (190, over 129 rows) was audited
  against its prototype and over every kept corpus trace, one was wrong (`madvise`'s `addr`,
  fixed) and seventeen pointer-*typed* ones were kept on the measured ground that the kernel
  never dereferences them — but a *new* row's `Scalar` is still checked by nothing but its
  prototype, and `Ptr` versus `Source`/`Dest` was never audited that way.
  *(3)* `reads_guest_buffer` is a view over rows, and a row is only as right as its prototype.
  **`ioctl` is narrower than the hole this entry used to name, and what is left is a nested
  pointer, not a length.** The *direct* parameter is bounded: the kernel copies
  `IOCPARM_LEN(request)` bytes in or out, at most `IOCPARM_MASK` (0x1fff), so the row is
  `[Fd, Scalar, Ptr]` with a cited bound. The corpora issue exactly four requests (`FIODTYPE`,
  `TIOCGWINSZ`, `TIOCGETA`, `DTRACEHIOC_ADDDOF`), decoded and pinned by a unit test. The residual is
  the fourth: `DTRACEHIOC_ADDDOF` is `_IOW('h', 4, user_addr_t)`, an 8-byte parameter that *is* a
  guest pointer to a `dof_ioctl_data_t` the kernel follows, and dyld issues it on nearly every
  dynamic guest. It is **forwarded, unrefused**, and fails today by EFAULT — measured
  `ret=0xe err=true` on all ten guests probed — because that nested copyin reads a guest address in
  retrace's process, so the later copyout of generation ids is unreachable. The refuse-by-value
  assert the M33 spec pre-authorised would have made every dynamic guest unrecordable, so it was
  ruled out; the hazard is the M27 nested-pointer class and stays with that owed item. Path-taking
  calls are deliberately absent because a path is NUL-terminated and bounded by `PATH_MAX` (1024),
  far inside the production window — the bound is the argument, not "paths are short". `msync` is
  the single entry justified by inference rather than measurement, listed because nothing is lost
  by listing and a silent corruption follows if the inference is wrong, and it says so at its
  definition. An unlisted reader syscall no longer corrupts silently — it has no row, and a syscall
  with no row is refused by name before it is forwarded; a *mis-rowed* one still does.
  *(4)* On a filled band, a kernel write that reproduces the canary pattern **exactly** is still
  undetectable in principle — 1/256 per byte, with the kernel having to hit it on every byte it
  writes. That is inherent to any canary, and it is the price of replacing a detector that was 100%
  blind to the zeros case rather than 1-in-256 blind.
  The saving grace underneath stays what it was: `Box_::diff_memory` compares
  every recorded region at exit and all three terminal replay arms fail on mismatch, so a truncation
  the band misses can still surface there, unless the guest acts on the stale bytes first or drops
  their backing before then — a read into a mapping that is then `munmap`'d would evade both, and no
  gate does that today. Both holes M27 and M28 left are closed at M35. `Box_::diff_memory` now
  returns a divergence naming the recorded length and the backing when a region is longer than
  what replay has behind
  it, instead of comparing the part that fits (flagged in M1's own review, unpaid until now; a
  correct replay never takes the branch, and `truncguard.rs` proves it fires). And
  `forward_and_diff` captures — and bands — on a **failing** syscall too, because the assumption
  that a failed syscall writes nothing was **measured false**: the M28 fixture `failsysctl`
  recorded cleanly on the pre-M35 tree and its replay diverged (`ipa 0x100004010 replay=0x02
  recorded=0x00` — the `oldlen` cell), since xnu's `sysctl()` writes `*oldlenp` back on the
  `ENOMEM` path and the `if !err` skip threw that write away. M28's datum stands as far as it went
  — the *data* buffer is untouched, `sysctl_old_user` refuses before copying — but its test read
  `buf` and never `oldlenp`. The data half is real too: `kern.proc.all` into one record's worth of
  buffer fails `ENOMEM` *after* copying 648 bytes out (`failproc`, `failsys_e2e`). No format
  change: replay's generic arm has applied `writes` beside `err = true` since M0 and simply never
  received one. Strengthening the band itself — sampling across the whole remaining backing under
  a fixed byte budget, rather
  than one contiguous 64-byte run immediately past the window — is now *unblocked*, since the "not
  covered by another window of this call" precondition Task 2 needed exists in code as
  `band_not_covered`, but was deliberately not attempted in M28, not in M30 — M30 changed
  what the band asks, not how much of the backing it looks at — and not in M35, which made the
  band run on the failing path without widening it. The suppression count above is a
  warning to whoever takes it up, since a naive wider sample would be suppressed even more often,
  not less.
- **Exec-in-place is unmodelled and refused — point retrace at the real binary, not the shim.** A
  launcher that `posix_spawn`s with `POSIX_SPAWN_SETEXEC`, which is exactly what Homebrew's
  `python3.14` shim does to hand off to the interpreter above, gets an **error** back instead of a
  replaced image and takes its own failure path. Since M38 that error is retrace's own refusal
  (`[retrace] refusing posix_spawn (syscall 244): exec-in-place is unmodelled; returning errno 14
  without forwarding`) rather than the host kernel's `EFAULT` on an untranslated `argv` — the
  same errno, by measurement and on purpose, so nothing the guest sees changed. retrace records
  and replays *that* outcome byte-for-byte — the oracle has nothing to disagree about, so this is
  retrace working rather than a bug — but the guest you get is the shim reporting a failure, not
  the program you meant to run. The behaviour is pinned by a test whose job is to hold the
  limitation visible (it asserts on the refusal line, which only the refusal prints), and which is
  to be **rewritten rather than defended** when exec-in-place lands.
- **A syscall with no row cannot be forwarded — closed structurally at M33 — but a row's
  descriptor positions are still only as right as the reviewer, five rows the corpus reaches are
  missing, and `x1` is written for one syscall only.** Before M33, a syscall that took a descriptor but was missing from
  `retrace_arch::fd_operands` had its guest fd forwarded to the host **unchanged**, where the same
  integer names a different file — the class M25 hit with `getdirentries64`/`fstatfs64`, and the
  default arm `_ => &[]` meant the next missing entry failed the same silent way. The blast-radius
  measurement that entry said nobody had taken was the census: 108 numbers across every guest the
  repo can run, each given a row, and `translate_fds` now consults `forwarded_shape`, which panics
  by name on a number with no row before anything is forwarded. Sixteen already-tabled readers had
  exactly that untranslated-fd defect and now translate. **`dup2` is modelled since M37**
  (`FdSlot::Console(u8)`, `FdTable::dup2`; a displaced host mapping is closed iff it is > 2, so a
  guest's `dup2` can never close retrace's own 0/1/2), **and `dup` copies the slot's kind since
  the M37 fix wave** (`FdTable::dup`: before it, `dup(1)` bound its alias as a plain `Open` slot on
  both sides, so a write through it — and every stdout write after `dup2(saved, 1)` — was forwarded
  to the host and absent from the trace, rc 0/0, no divergence; the final review measured it and
  `dupkind_e2e` guards it), **and the two descriptor-producing calls that list left unmodelled are
  modelled since M38**: **`fcntl(F_DUPFD)`/`F_DUPFD_CLOEXEC`** is a table operation
  (`FdTable::dup_from(src, min)` — the lowest free *guest* slot ≥ `min`, the source's kind, both
  sides; a host `dup` behind it on record, never a host `F_DUPFD`, whose minimum would be a host
  number; the range check is the table's too since the final-review fix, so a `min` outside
  `[0, DUP2_MAX_FD)` is `EINVAL` on both sides rather than record-only; the close-on-exec bit is
  not modelled — exec is refused, and its one observable is a forwarded `F_GETFD`, which reads
  the host `dup`'s *clear* flag, so a guest doing `F_DUPFD_CLOEXEC` then `F_GETFD` reads 0 where
  native reads 1, deterministic across record and replay because the recorded return carries it,
  a fidelity gap and not a divergence — so both commands share the path), and **`pipe`** binds
  both ends (`Box_::bind_returned_pair`, read end
  first; `Event::Syscall::ret1` carries the write end; `csh`/`tcsh` now receive `(4, 5)` and
  `fcntl` their moved ends successfully where they used to `EBADF` a raw host descriptor and a
  stale register). Two edges of the model are known and symmetric: a displaced-then-closed slot below 3
  (`dup2(f, 1); close(1)`) is never re-allocated, because `alloc`'s floor is 3, where the kernel
  would hand 1 back; and a host `dup` failure on record is recorded as `(errno, true)` and diverges
  loudly on replay, which recomputes success. Two limits are M38's own, by ruling. **`x1` is
  written only for `pipe`** (narrow capture, spec R2): every other syscall leaves the guest's `x1`
  stale where xnu would write `retval[1]` — deterministic on both sides, and `fork` is the only
  other two-register call in the ABI, itself class C; the uniform capture is a measurement a later
  milestone can take with the field already in the trace. And **five `arg_kinds` rows the corpus
  reaches are missing** — 461, 468, 464, 345, 374 — each a loud M33 panic on the row that
  reaches it (`ls`; `ed`/`desdp`/`dyld_info`/`flex`; `dddiagnose`; `automationmodetool`), left for
  the successor rather than added unmeasured at the end of an unattended run (the ledger's
  rulings at Tasks 3, 5 and 6); 464 is `openat`'s `_nocancel` twin and 345 is `fstatfs64`'s
  fixed-struct twin, so two of the five are one-line copies of rows that exist. A third is the
  final review's, ruled out of M38's scope as an unmeasured behaviour change at the close and
  owed here instead: **`F_DUPFD_CLOEXEC`'s bit on the host dup** (one line in
  `guest_fcntl_dupfd`; only `F_GETFD` observes it). What stays open
  is one level down. **A wrong position in a row is silent**: the equivalence sweep proves the
  views reproduce the legacy tables, and `Scalar`-versus-`Fd` on a *new* row is checked by nothing
  but the prototype and the reviewer. **`AT_FDCWD` is honoured in the 32-bit form since M38**:
  the guest passes `-2` as a 32-bit `int` in `w0`, so `x0` arrives as `0xfffffffe`, and
  `translate_fds` tests `(v as i32) < 0` (both that form and the sign-extended one read as `-2`;
  a real descriptor never has bit 31 set) where it had tested `(v as i64) < 0` from M10 t3
  (`e67dd65`) through M37 and rejected every real guest's sentinel as `EBADF`; `atfdcwd_e2e` pins
  the form and the success on one landmark, and the `fdxlat` test passes the measured form first.
- **A stage-1 alias is invisible to every reader that goes by address.** Since M39 the box
  services `mach_vm_remap` (4813) by writing the source page's L3 descriptor into the target's L3
  slot, so the *hardware* translates the alias range to the source's memory. Nothing else does.
  `Box_::read_guest` and `read_guest_checked` resolve an address against the box's list of
  **backings**, not by walking the stage-1 tables — every mapping before M39 was identity, so the
  two agreed by construction — which means the debugger's `x`, and everything downstream of
  `ReplaySession::read_mem`, read an aliased range's **old** backing rather than the source bytes
  the guest executes there. On rung 8's path nothing reads a trampoline page by VA, so the gap
  costs nothing measured; it is documented at the method rather than fixed, and a reader that
  needs the alias would have to walk the tables. Watchpoints are unaffected either way: they are
  hardware `DBGW` on a VA. The same route leaves `copy = TRUE`, `VM_FLAGS_ANYWHERE`, a foreign
  `src_task` and an overlapping target **unmodelled and asserting by name** — none is issued by
  anything measured — and its derived `max` protection would answer two unmeasured shapes wrongly
  (a guest `FIXED` mmap below the nano band, a read-only `MAP_SHARED` source above it).
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
- **`reverse-continue` is one forward replay plus one resolution, so its cost scales with the
  recording's length, not with the hits.** Since M40 it no longer restarts a session per hit: one
  session scans from landmark 1 at native speed with the breakpoints and watchpoints armed,
  stepping over each hit in place; the current window is single-stepped only up to the current
  position; and only the last qualifying hit is then resolved — at most three session opens however
  many hits lie between (`debug.rs`'s `reverse_continue_makes_at_most_three_seeks_whatever_the_hits`
  pins `≤ 3`). That costs about one replay plus a window of single-steps. On rung 8's recording the
  command itself measured **0.53 s CPU** by subtraction (8.64 s for
  `continue; watch; reverse-continue` against 8.11 s for `continue; watch`, dev build) at
  **≈ 403 MB** peak RSS, with the machine at a load average of 1.45–1.92; M39 had attributed 3.42 h
  of wall-clock to the same command, and two concurrent runs exhausted this 24 GB machine's RAM and
  all 63 GB of its swap. `cpython_crash_e2e`, which runs it, finished in 40.02 s against M39's
  12,364 s. What remains is the floor: the scan is still a **full forward replay from landmark 1**,
  so a recording long enough that one replay is slow makes every `reverse-continue` on it that
  slow, and there is no backward, checkpoint-segmented search. A debug session now decodes its
  trace once rather than per seek, but that decode is still dominated by a bit-at-a-time `crc32`
  (64 % of a decode in M40's t0 profile), and every `replay` and test pays it in full; **a faster
  `crc32` is owed.**
- **A `reverse-continue` to a breakpoint can land one hit off across a thread switch.** Breakpoint
  hits are counted by pc. At `(n, 0)` after a blocking syscall, `pc()` is still the **outgoing**
  thread's resume pc — `run()` and `step()` switch threads on entry — so the resolver and the
  partial-window step miss the incoming thread's first instruction, and a breakpoint whose pc
  recurs in that window can be resolved one hit off. It is silent and inherited, not new: the
  pre-M40 resolver had the same blind spot. Nothing exercises it yet; closing it needs a threaded
  breakpoint fixture.
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
- **Nine gates are parked `#[ignore]`d** at documented, *measured* walls, and the reason is on each
  test itself. Two are long-standing. `stackoverflow_rust_e2e` — but **no longer for the reason it
  carried from M8 through
  M20**. M8 risk R3 is CLEARED: the recursion now grows through M21's reservation and strikes its own
  guard page at stage 1. It is re-parked one wall further on, at the blocked-signal limit below, and
  the progress it used to stand for is gated by a *running* test beside it so it cannot regress in
  silence. And `cache_symbol_e2e` since M19, at the shared-cache
  symbol wall above. Seven are in `crates/retrace/tests/apple_walls_e2e.rs`, one per non-clean
  sweep row that is retrace's to fix or model and has a gate (the table above): eight were parked
  by M36, **moved by M37** to the walls it measured, and at **M38 one was un-parked** (`launchctl`
  — the RCV-shaped `mach_msg2` is refused and it runs to its own usage exit; the test asserts on
  that, not on `rc == 0`) **and five moved in place** to the first missing `arg_kinds` row behind
  the refusal (class B: `kevent_qos` 374, `openat_nocancel` 464 ×3, `statfs64` 345; un-parked
  when the row exists and the row records past it), while `csh`/`tcsh` stay at `fork` (class C:
  `mach_ports_register` from `xpc_atfork_prepare`, `fork`(2) behind it; un-parked when the box
  models process creation) with their `pipe` landmark refreshed — each reason the measurement
  that parks it — the label, `rc`/`rp`, the recorder pid and its regime, the landmarks, the
  recorder's own line with its symbol, the evidence file, the class, and what un-parks it — and
  each run once with `--ignored` to show it fails for exactly that reason (M37: 8 of 8 at pids
  72339–72381; M38: the five re-parked, each printing its wall by name in
  `docs/sweep-evidence/2026-09-16-m38/gates.log`). **M39 moved none of them**: its one wall was
  cleared inside the milestone, so no gate was parked, un-parked or re-worded, and M39's sweep
  changed no row's label. `ls` and `ed`, non-clean since M38, have no
  gate: their rows were never parked, and the same five-number list un-parks them. Before M36 the
  two long-standing ones were the whole count, and the `brk` wall M23 found had **no** gate from
  M23 to M35 — a gap this README recorded in its own voice as a gap rather than a decision, now
  paid. It was **three** between M22 and M23 — M22 parked `sysbin_e2e`'s second gate at
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
- **Record-only box state now has a structural guard on both replay paths, and neither guard closes
  the class.** `Box_` has three construction paths — `load`/`load_dynamic` (record only), `restore`
  and `from_checkpoint` (both replay only) — and anything a load path establishes that a replay path
  does not re-establish is a bug whose signature is *a passing record followed by a diverging
  replay*. The determinism oracle cannot see it when both replay paths are wrong the same way,
  because the oracle compares replay against record's **trace**, never against record's **box**.
  Since M24 the `load`↔`restore` pair is pinned by `retrace-box/tests/restoreparity.rs`, which diffs
  a load box against a `restore` box built from that box's own snapshot (15 of `Box_`'s 27 state
  fields plus two sysregs and the 0x800 vector table). Since M31 the `load`↔`from_checkpoint` pair is
  pinned the same way by `retrace-box/tests/checkpointparity.rs`, which drives a box to a **mid-run**
  landmark, checkpoints it, rebuilds from that checkpoint and diffs the two — and it carries the same
  written obligation: a new field must be compared there and equal, asserted as **deliberately
  reset** with the mechanism that re-establishes it on the replay side cited by file and line, or
  named as knowingly excluded citing the comment that documents the exclusion. There is no fourth
  option that is safe.
  **The count this entry published for seven milestones was too high by one, and M31 found the
  error's provenance rather than merely the absence of evidence.** This entry used to say the class
  had shipped **seven** times (M9 t3, M10, M11, M14, M18, M21, M23), with a documented *five*-instance
  history on `from_checkpoint`. M18 is not an instance: commit `e93f8dc` adds `wq_thread_pc`'s `Box_`
  field, its `BoxState` field, the `checkpoint()` carry **and** the `from_checkpoint` restore in one
  commit, so no gap ever existed; all four M18 status-log sections mention neither `from_checkpoint`
  nor `BoxState`, and M18 files its own recurring bug under a different class. The origin is a single
  uncited line in M24's design spec — "**M18** — `wq_thread_pc`, same reason." — whose own named
  source is the `BoxState` field comments, where `wq_thread_pc`'s comment reads "carried for the same
  reason as `thread_start_pc` immediately above". **Same *reason* was read as same *instance*.** So
  the corrected counts are **six** for the whole class and **four** on `from_checkpoint` (M9 t3, M10,
  M11, M14), and the provenance is written down here because the comments that produced the seven are
  still in the tree and would produce it again.
  **The M31 guard found no `from_checkpoint` asymmetry** on anything its fixtures reach — a
  two-thread table, fd slots (Open and Closed, distinct from Free), the signal table, all three
  pthread/workqueue scalars, a `PROT_NONE` extent, the cache pager, a bootstrap port, an armed
  breakpoint, an armed watchpoint and `tpidrro_el0` all came back equal, and the four debugger fields
  came back correctly reset with the debugger's own re-arm cited. That is a measured statement about
  the code's current state, not a milestone that did not look: two mutations prove the guard fails
  when it should — resetting `sigtable` in `from_checkpoint` turns the rich tier red at `rich: signal
  dispositions`, and zeroing every non-current thread's context turns the rich tier red while leaving
  the static tier **green**, which is what makes the two-thread fixture load-bearing rather than
  decorative.
  Three limits remain, the first two carried over from M24 unchanged. The guard compares
  **construction** at one landmark and not **evolution** after it (`retrace/tests/checkpoint_seek.rs`
  is that axis). Two boxes wrong in the *same* way stay invisible to any test that only diffs them
  against each other. And reach is bounded by what a fixture can stage from a static box: eight
  fields the diff reaches are still `Default == Default` there — `synthetic_tsc`, `last_far`,
  `cache_refault_ipa`, `cache_refault_count`, `pac_enabled`, `fall_throughs`, `tpidr_el0` and
  `syscall_watch_hit`, each needing a guest that executes the instruction or takes the fault, not a
  setter. `stack_top`/`stack_size` are a **different** class and not part of that eight: they are
  always-identical non-trivial constants that no public API moves post-load, so the comparison cannot
  tell a genuine carry-through from a hardcoded recomputation of the same constant.
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

**Do not omit the `--bins` chunk.** `--test <name>` selects integration-test targets only, so the 14
unit tests inside the `retrace` binary itself (`crates/retrace/src/debug.rs`) run in none of the
other chunks; **only the unchunked `--workspace` run, or a whole-package `cargo test -p retrace`
without a `--test` filter, reaches them.** Leaving it out silently costs 14 tests and one binary —
at M29, when there were 11, 526 / 0 / 2 over 115 instead of 537 / 0 / 2 over 116 — and nothing
fails to warn you. Contrast `cargo test -p retrace --lib`, which is invalid for this crate (there is
no lib target) and fails the whole invocation loudly.

**The same trap has a second mouth: `Doc-tests`.** `--test <name>` skips those too, so splitting a
*library* crate per-target — as M24's gate had to for `retrace-box` — drops that crate's `Doc-tests`
harness from every chunk. It runs zero tests, so nothing fails; it just quietly costs one of the
counted binaries (the figure was 118 when M24 hit it, and it moves every milestone — which is why
this sentence no longer names one). If you split a library crate per-target, run `cargo test -p <crate> --doc` alongside it —
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
- `docs/sweep-evidence/<date>-<milestone>/` — the Apple sweep's kept evidence: the decisive stderr
  of every non-clean row per run, verbatim, with a README giving the binary commit, the pid
  regimes, the symbolication and the counting rules (M36), and the audits and the reader that
  re-derives every number (M37).
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
