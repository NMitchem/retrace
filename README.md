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
retrace gdbserver  <trace> [--port <n>] [--exe <path>] # serve a recorded trace to lldb
```

In development, invoke through cargo so the codesigning runner applies —
`cargo run -p retrace -- record-dyn <exe> -o t.bin`.

`gdbserver` (since M43) binds `127.0.0.1:<n>` (`--port 0`, the default, picks a free port), prints
exactly one line to stderr, `listening on 127.0.0.1:<port>`, once the recording is open, and serves
**one** gdb-remote connection. A second connection is refused. The `k` or `D` packet, or the peer
closing or resetting the connection, ends it with status 0. A usage error (a flag given last with no
value included) exits 2, and any other failure exits 5 with `GDBSERVER ERROR: …`. See *Debugging
with lldb* under "What works today".

`RETRACE_TRACE=1` on a `record`/`record-dyn` run logs every dispatched trap and decodes `mach_msg2`
sends. It is the first thing to reach for on a bring-up failure. Each `[trap]` line prints `x0`–`x7`
(since M44: `kevent_qos` carries its flags in `x7`). **Record-only** — `ReplaySession` carries no
trace instrumentation, so no `[trap]` line is ever printed on replay.

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

The same debugger also runs under lldb, through `retrace gdbserver`: lldb's own commands drive it
there, plus one retrace ships, `rsi` (*Debugging with lldb*, below).

## What works today

**Guest breadth.** In short: anything you compile yourself (C or Rust), stock Homebrew arm64
binaries, and — since M22 — most of the Apple binaries already on your machine. Each rung below
records and replays byte-identically, twice:

| Rung | Guest | Notes |
|---|---|---|
| 0 | freestanding `-nostdlib -static` arm64 | 46 `asm/*.s` fixtures |
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
from any recorder pid; since M44, with the two false passes M38 turned into named walls now clean.**
`tools/apple-sweep.sh` points retrace straight at each file in a committed 54-entry corpus and
prints a tally: **49 of 54 record and replay**, stdout byte-identical and exit codes equal (`TALLY
pass=49 fail=5 skip=0`, measured 2026-09-28 on M45's branch commit `09a105f`, whose record and
replay paths are Task 2's `28fddc4`, one run at recorder pids `0xc649`–`0xdb7d`). **That is
M44's tally, but not M44's rows**: five rows differ from M44's run, and each move is measured
(Known limits has them). `automationmodetool` is still a FAIL, moved by M45 from the missing row
for 374 to the second `kevent_qos` shape (the libdispatch entry under Capabilities, below).
`/bin/ps` went PASS → FAIL, a **class-E row**: record and replay each ran to the end and disagreed
at the final memory compare. That is host state, not M45. The final snapshot's page map reads as
the host reclaiming one guest page after a forwarded `madvise(MADV_FREE_REUSABLE)`, three
landmarks before exit; that is a fingerprint, not an observed reclaim. It is flaky (20 fresh runs
were clean), and it is the first measurement of a hazard M37 named. `dddiagnose` went FAIL →
PASS, but only as an identical fault. It is bimodal on M44's crates and M45's alike: about one
run in four faults at M36's `mfm_alloc+0x230` face, and the rest stop at `host_get_io_main` as
before. `csh` and `tcsh` moved their landmark only. `ps` out and `dddiagnose` in is why the count
did not move. M44's 49 had moved from M39's 44/10 by **five rows, each explained by name**. `ls`
and `ed` are *clean* now, rc 0 on both sides with byte-identical stdout: the rows
M44 added (`getattrlistbulk` 461 for `ls`; `openat_nocancel` 464 and `unlink` 10 for `ed`) carry
each past the wall it had stopped at, loud, since M38. (From M33 through M37 both had "passed" by
failing identically on an `AT_FDCWD` the fd table rejected; M38 honoured the sentinel and turned
that silent lie into two named walls.) The other three are `desdp`, `dyld_info` and `flex`, and
they pass **in the sweep's sense only**: from a cold xcrun cache past 464 and `rename` (128), and
from a warm one directly, each reaches xcrun's `posix_spawn`, which retrace refuses (exec-in-place is unmodelled), and exits 71 on both sides,
where natively `desdp` exits 2 with its usage, so their gates stay parked (Known limits). M37 had
run the sweep three times on 2026-09-13 with the recorder's pid steered into the three regimes M36
had measured (below `0x4000`; inside `[0x4000, 0x10000)`; inside `[0x10000, 0x18000)`) and tallied
45/9 in all three with the same nine labels in every regime — the pid-collision defect that once
selected a wall is gone, and every single-run sweep since (M38's, M39's, M44's, M45's) rests on
that. The five rows that are not clean are read off kept evidence, one class each. **Seven parked
gates** (`crates/retrace/tests/apple_walls_e2e.rs`) stand for three of them that are retrace's to
model (`csh`, `tcsh`, `automationmodetool`), for `dddiagnose`, whose gate stays parked although
this run counted it a PASS, and for the xcrun trio, each `#[ignore]` reason the measurement that
parks it. `ls`, `ed` and `launchctl` have gates in the same file that run. `ps` has no parked gate:
its class-E row is host state, and `sysbin_e2e`'s `ps_records_and_replays` runs.
Among the 49: `cat`, `cp`, `mv`, `rm`,
`chmod`, `mkdir`, `ln`, `df`, `sh`, `dash`, `bash`, `zsh`, `expr`, and since
M44 `ls` and `ed`; `dddiagnose` counts only as an identical fault. `ps`, among them from M27 through
M44, is out of this run's 49 on the reclaimed page. (This
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
See Known limits for the table of the five non-clean rows and `dddiagnose` — the face each row
shows, its class, its gate and its route — why one of them is a failure by design, and the
reconstruction caveat in full.

**Capabilities**

- **Reverse execution** — `(N,K)` landmark seeks, checkpointed for ~800× faster backward seeks.
- **Watchpoints** — hardware `DBGW` (pre-retire) plus software detection, with
  reverse-continue-to-last-writer, thread-attributed. Since M40 a hit is resolved by the
  **address** it wrote, by single-stepping with the watchpoint armed, not by the store's pc: a
  store instruction that ran on other addresses first — a `memset`-style loop — had made `continue`
  name an earlier run of it that never wrote the watched memory. `watchsweep_e2e` is the guard.
- **Hit order** — since M41 every hit has one place in one order, `(n, k, phase)`. At one
  coordinate a syscall's recorded write to a watched range comes first (only at `k = 0`, where the
  event that ended window `n − 1` wrote it), then a breakpoint on the instruction about to run, then
  that instruction's store to a watched range, stopped pre-retire. The debugger's position is a
  cursor in the same terms: `continue` gives the first hit after it, `reverse-continue` the last one
  before it, and reporting a hit puts the cursor on that hit. A breakpoint you arrived at (by
  stepping or seeking: `stepi`, `reverse-stepi`, the opening position) is reported in neither
  direction, gdb's rule, while a watched store you stepped up to still fires going forward; a step
  that does not move (`stepi 0`, `reverse-stepi 0`) is not an arrival. The end of the recording,
  an exit or a crash alike, parks on its last instruction (the exit `svc` or the faulting one) and
  sits after every hit, a breakpoint on that instruction included, so `reverse-continue` from the
  end finds the last hit and `continue` reports the end again. At a thread
  switch the position shows the thread that runs next: after a blocking syscall, `where`,
  `threads` and `regs` at `(n, 0)` name the incoming thread, not the one that just blocked. Before
  M41 the debugger had no defined order. M41's t0 measured eight cases (M1–M8) where it skipped a
  hit, reported one that nothing executes, or failed loud where a hit existed, M40's owed
  forward-`continue` skip and its thread-switch blind spot among them; each is a named regression
  in `hitorder_e2e`. The same file checks the debugger against a **brute-force hit oracle**
  (`tests/util/hits.rs`), which single-steps a recording with every breakpoint and watch armed and
  lists every hardware stop, sharing none of the debugger's resolution machinery. It runs six
  armings on three fixtures, three chains each: `continue` until the guest ends, `reverse-continue`
  until there is no earlier hit, and a zig-zag that asks each hit for its neighbour in the other
  direction. Five were red on M40's debugger; the sixth, hits in the exit window, was added by
  M41's final review and was red on M41's own debugger until the end of the recording moved to
  after them. That is the oracle's proof that it can fail.
- **Exclusive (LL/SC) pairs under the debugger** — since M42, stepping, seeking, `reverse-stepi`,
  checkpoints and native breakpoint/watch stops keep an AArch64 exclusive pair's store-exclusive
  exactly as the recording ran it. Any VM exit between an `ldxr` and its `stxr` clears the core's
  exclusive monitor, so before M42 a debugger exit inside a pair failed a store the recording had
  made: divergences, hangs on retry loops, phantom watch hits and a wrong `no earlier hit`, all
  measured. `Box_` now keeps a **shadow of the exclusive monitor**, below the trace. A stepped
  load-exclusive sets it (the step exit's syndrome reports one, ISS.EX), and every exit that is
  not a debug exit clears it, as the hardware's exception return does. `step()` then **emulates**
  the store-exclusive, once a pure validator has passed it (every refusal is a panic naming its
  check) and any breakpoint or watch stop the hardware would take there has been raised. A native
  breakpoint or watch stop inside a pair **infers** the shadow from the instructions behind it
  (the one heuristic; Known limits has its assumptions), `run()` entered inside a pair steps it to
  its end before resuming natively, and checkpoints carry the shadow. Record and plain replay never
  engage it, and both assert so. Nothing new is recorded and `TRACE_MAGIC` did not move.
  `llsc_e2e` is the guard: a fixture of nine LL/SC shapes, every t0 failure as a named regression,
  controls for the clear rule, and three hit-oracle armings. On `threadrust` a seek into dyld's
  `getpid` pair now replays to the end. Since M43 `step()` decodes each instruction before it runs
  it, so a load-exclusive that overwrites its own base (`ldxr x9, [x9]`) keeps the address it
  marked, and a store-exclusive whose target is unmapped or not EL0-writable is stepped natively
  into the fault the recording holds, not emulated. Both were panics under M42; Known limits has
  the details and the measurement. Since M44 each guest read behind that decode finds its backing
  by binary search over a sorted index kept beside the backing list (`retrace-box/src/backings.rs`,
  spans asserted disjoint), not by a linear scan; an equivalence test over randomised probes pins it
  to the scan it replaced, and the backing list itself is never reordered, so snapshot bytes do not
  move (spec R2). Known limits has the CPU it recovered and what it did not.
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

  **Since M45 libdispatch's workqueue-kqueue init is emulated too.** That is `kevent_qos` (374)
  with `KEVENT_FLAG_WORKQ`, the call `_dispatch_kq_init` makes to register the event manager's
  `EVFILT_USER` wake-up on the process's workqueue kqueue. It is in the workq pair's class:
  forwarded, it would act on retrace's own workqueue kqueue, and libdispatch crashes on any errno
  but `EINTR`, so it needs a modelled success. `Box_::guest_kevent_qos` answers **exactly one
  measured shape** with 0: the eight arguments and the 72-byte change entry t0 measured on
  `automationmodetool`, every byte and every argument compared except the change list's address,
  `int` arguments on their low 32 bits. The host kernel returns the same 0, carry clear, for that
  call (t0 M3, the fixture run natively). It writes nothing and keeps no state, and replay
  recomputes it through the same method, diverging on a different return or on a recorded `err` or
  writes. **Every other shape stops the recorder by name**, `M45: unmeasured kevent_qos shape: …`,
  naming the argument or entry field, its measured value and the value it has; an entry that does
  not fully translate is refused as `read N of 72 bytes`. The generic forward arm asserts that 374
  never reaches it. `kqinit_e2e` guards this with a hand-issued fixture, `kqinit_dyn.c`: inline
  `svc` with `x16 = 374`, after a `dispatch_async` has brought the workqueue up. Its five tests
  cover one emulated landmark (rc 0, no writes) replayed byte-identically twice; an entry
  straddling a 16 KiB page; the refusals by value (an `EV_ENABLE` flag, an untranslatable change
  list); a trace with the return or `err` rewritten, which replay must name as a divergence; and
  seeks either side of the landmark. **The emulation alone unblocks no real program yet** (outcome
  B, M45 spec §7 Halt 3). Every libdispatch path measured makes a second, different `kevent_qos`
  right after the init: an event list of 16, `x7 = 0x23`
  (`KEVENT_FLAG_WORKQ | KEVENT_FLAG_ERROR_EVENTS | KEVENT_FLAG_IMMEDIATE`), one entry with filter
  −14. By its values that is libdispatch's own memory-pressure source, which is inferred and not
  symbolicated. M45 refuses that shape and does not model it. `automationmodetool` records the
  init and stops at that call, so its gate stays parked there (Known limits). The two GCD
  candidates walked, a timer source and `dispatch_after`, stop at the same call, so **no GCD gate
  was added**; a third, a signal source, was dropped because it hangs natively. The successor that
  models the second shape is what moves them. Nothing new is recorded and `TRACE_MAGIC` did not
  move.
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
  fails, and a listed entry that no longer differs fails too. There are **35** such entries today,
  each with its reason: 26 at M43's close, and nine M44 added for the `_nocancel` twins it joined
  to their plain rows (464, 409, 542, 543) and for `getattrlistbulk`'s row (461). M33's twenty-two
  were these. **Sixteen** are descriptors the legacy `fd_operands` never translated —
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
  Since M44 it pins **113**: the five numbers the sweep's binaries reached once M38 moved their
  walls (`unlink` 10, `rename` 128, `statfs64` 345, `getattrlistbulk` 461, `openat_nocancel` 464),
  each measured by M44's t0 in a kept trace. Since M45 it pins **114**: `kevent_qos` (374), which
  `automationmodetool` dispatches (M44 t0 M1, M45 t0 M1). Its row is documentation, like the workq
  pair's, because the call is emulated above the trace and never forwarded. No view differs for
  it, so `EXPECTED_DIFFS` stays at 35.
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
  five ran on to a syscall with no `arg_kinds` row and were re-parked there, class B, until M44
  tabled three of those rows and routed the fourth (Known limits has where each stops now). Every mirror
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
- **Every `_nocancel` spelling shares its plain form's row, checked against the SDK; `ls` and `ed`
  record and replay; `F_DUPFD_CLOEXEC` sets close-on-exec.** Since M44.
  `crates/retrace-arch/tests/nocancel.rs` parses the SDK's `sys/syscall.h` at test time and, for
  each of its 32 `_nocancel` names (every one has a plain twin), asserts that `arg_kinds` gives the
  twin and its plain form the same row. That is the rule the table's own comment states, which five
  milestones had broken by hand, and it went red on exactly four twins: 409 `connect_nocancel`, 464
  `openat_nocancel`, 542 `preadv_nocancel` and 543 `pwritev_nocancel`. Each now shares its plain
  row, so 542 is refused upstream as 540 is and 543 is forwarded as 541 is. Beside them M44 tabled
  `statfs64` (345, `[Path, Ptr]`, `fstatfs64`'s path twin), `getattrlistbulk` (461,
  `[Fd, Ptr, Dest(Reg(3)), Scalar, Scalar]`: xnu never checks `bufferSize`, so the window follows
  it), `unlink` (10) and `rename` (128), each reached by a corpus binary. `/bin/ls` and `/bin/ed`
  now record and replay clean, and `apple_walls_e2e`'s `ls_records_and_replays` and
  `ed_records_and_replays` run un-ignored. And `guest_fcntl_dupfd` sets `FD_CLOEXEC` on the host
  `dup` when the command is `F_DUPFD_CLOEXEC`, so a forwarded `F_GETFD` reads 1 as native does,
  where it had read the host `dup`'s clear flag (`dupfd_e2e`). Nothing new is recorded and
  `TRACE_MAGIC` did not move.

**Gate:** 832 passed / 0 failed / 9 ignored across 146 test binaries. The close ran the full chunked gate
from one background script (`gate.sh` in the milestone's ledger directory), every test chunk
`--no-fail-fast` and every exit code captured before any pipe: `ws`, `box`, `--bins`, one
`--test <name>` invocation for each of the seventy-seven files in `crates/retrace/tests/`, and
clippy over `--workspace --all-targets` with `-D warnings`. The status log's M44 section has the
tally chunk by chunk. The testing note below says how the chunks are assembled. The "test binaries"
figure is test executables plus the `Doc-tests` harnesses cargo reports, each of which runs zero
tests — the convention every milestone since M14 has counted by, kept for comparability and
written out here so nobody has to re-derive it. No `#[ignore]` line was added or removed (nine at
M43's close and nine now, by `git grep` over `crates/`), so nine gates are parked: the two
long-standing — `stackoverflow_rust_e2e` (re-parked by M21 at a signal-model wall, **not** the M8
risk R3 wall it stood at from M8 through M20) and `cache_symbol_e2e` (the M19 shared-cache symbol
wall) — plus the seven in `apple_walls_e2e`, each reason the measurement that parks it. **M44
re-parked five of those seven at new, measured walls** and added two gates there that run, `ls` and
`ed`; Known limits has each. `lldb_e2e` needs `/usr/bin/lldb` (never the one on `PATH`): each of
its tests skips loudly (`SKIPPED …: This gate did NOT run.`) when `/usr/bin/lldb --version` does not
run, and its CPython test also skips without Homebrew Python. A skipped test is counted as passed,
as `jq_e2e`'s are, and since M44 every skip line reaches the ordinary gate log (Testing, below), so
grep the logs for `SKIPP` before reading a count as the gate having run.

Reconciled against M43's 800 / 0 / 9 over 144 **file-by-file rather than by sum**, by source: eight
files changed their `#[test]` count, and every other file's count is M43's (counting
`^\s*#\[test\]` per file at `64e471e` and at the branch head):

| file | M43 | M44 | delta |
|---|---|---|---|
| `retrace-arch/src/lib.rs` | 44 | 45 | **+1** (`m44_syscall_numbers`: the new constants for 464, 409, 345 and 461) |
| `retrace-arch/tests/nocancel.rs` | — | 1 | **+1**, new binary (every SDK `_nocancel` twin shares its plain form's row) |
| `retrace-box/src/backings.rs` | — | 6 | **+6**, new module (the index against the linear scan over randomised probes; the zero-length read at a backing's end; an overlapping insert fails loud on either side, and at the same start; a read whose end would overflow is held by nothing) |
| `retrace/tests/apple_walls_e2e.rs` | 8 | 10 | **+2** (`ls` and `ed` record and replay) |
| `retrace/tests/dupfd_e2e.rs` | 2 | 3 | **+1** (`F_DUPFD_CLOEXEC` sets close-on-exec as native does) |
| `retrace/tests/gdbserver_e2e.rs` | 28 | 39 | **+11** (`disarm-rsi`; a step across its own thread's exit; another thread's breakpoint, mid-window breakpoint and watched store ending a blocked step; a step of a thread that is not running, of one that does not exist and of one that has exited; a step the recording's end comes before, in both arms and at an exit; the finish's arrival past a breakpointed trap; a `c` after a step across its own thread's exit reports a breakpoint at the running thread's pc — two rows rewritten in place) |
| `retrace/tests/lldb_e2e.rs` | 5 | 12 | **+7** (a step across its own thread's exit without looping, and a breakpoint after it; `next`, `ni`, and `thread step-out`/`finish` over a call; `bt` on an arm64e guest; a step the recording ends before — one row rewritten in place) |
| `retrace/tests/skiplines.rs` | — | 3 | **+3**, new binary (the detector, its own positive control, and the `SKIPLINES CONTROL` line) |

+32 `#[test]` attributes, `--bins` unchanged at **32**, and **two new test binaries**, `nocancel`
(in `retrace-arch`) and `skiplines`. The tree holds **839** `#[test]` attributes by the same
per-file pattern (M43 held 807). The run still reports the 2 census tests twice (`census.rs`
executes in its own binary and again inside `legacy_equivalence`'s `#[path]` include), and a bare
`grep -c '#\[test\]'` over-counts by one, because a comment in `legacy_equivalence.rs` mentions the
attribute in prose. The plan predicted +18 before t0; the source count is +32, and every difference
is a review fix round or a row measurement asked for: Task 6 is +6 where it counted 3 (its fix round
pinned both overlap sides and the overflow); Task 8 is +3 where it counted 2 (the lldb row that
tells B3 apart); Task 9 is +3 where it counted 2 (`ni` split from `next`); Task 11 is +2 where it
counted 0 (the store-watch and mid-window rows); and Task 12 is +7 where it counted 1 (its first
commit replaced one row with two, and its fix round added five wire rows and one lldb row); and
the final review's fix wave added one wire row (B3's park, FW-2).

`retrace-box` ran as a **whole package**, so its `Doc-tests` harness could not be dropped (M24's
lesson). `retrace` ran **per-target** — seventy-seven `--test <name>` invocations, one after another
from a single background script, because the whole package exceeds the tool ceiling, over the
target list `ls crates/retrace/tests/*.rs` wrote — **plus the `--bins` chunk**, which is the only
place the 32 unit tests inside the `retrace` binary run: 20 in `crates/retrace/src/debug.rs` and
12 in `crates/retrace/src/rsp.rs`. The binaries count includes it. The two mouths of the same trap,
one loud and one silent, both closed by construction of the chunk list.

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

### Debugging with lldb

Since M43, `retrace gdbserver` serves one recording to the host's `lldb` over gdb-remote (RSP), and
lldb-2100 (Xcode's; measured on lldb-2100.0.17.203) debugs it **forward and backward**:
`process continue` and `thread step-inst` go forward, `process continue -R` goes backward, and
`rsi`, a command retrace ships as a small lldb Python script, steps back one instruction. The
server is a translation layer over the script debugger's own engine (`Exec` in
`crates/retrace/src/debug.rs`), not a second debugger, so every stop it reports is one M41's hit
order defines and M42's pair handling applies unchanged. The original design's exit criterion,
"reverse-step through a real crash in LLDB", is pinned by `lldb_e2e`: real lldb against the repo's
`crashy`, and against rung 8's CPython crash when Homebrew Python is installed.

In one terminal (an absolute path, so the recording names its executable; see `--exe` below):

```sh
$ retrace record-dyn "$PWD/mytool" -o t.bin
$ retrace gdbserver t.bin --port 5555
listening on 127.0.0.1:5555
```

In another:

```
$ lldb
(lldb) gdb-remote 127.0.0.1:5555
(lldb) command script import <repo>/crates/retrace/lldb/retrace.py
(lldb) process continue
(lldb) watchpoint set expression -w write -s 8 -- <addr>
(lldb) process continue -R
(lldb) rsi
(lldb) process continue -F
```

On `crashy`, `lldb_e2e`'s session prints this, abridged to the stops (the port, the script import,
disassembly, `Process 1 …` lines, most of `watchpoint set`'s output, and the `bt` and
`register read` commands cut); `0x1000080b0` is `&g.ptr`:

```
(lldb) gdb-remote 127.0.0.1:<port>
* thread #1, stop reason = start of recording
(lldb) process continue
* thread #1, stop reason = EXC_BAD_ACCESS (code=1, address=0x4000dead0000)
    frame #0: 0x00000001000005c4 crashy`main + 204
(lldb) watchpoint set expression -w write -s 8 -- 0x1000080b0
Watchpoint created: Watchpoint 1: addr = 0x1000080b0 size = 8 state = enabled type = w
(lldb) process continue -R
Watchpoint 1 hit:
old value: 70372480057344
new value: 4295000208
* thread #1, stop reason = watchpoint 1
    frame #0: 0x000000010000059c crashy`main + 164
->  0x10000059c <+164>: str    x8, [x9, x10, lsl #3]
(lldb) rsi
* thread #1, stop reason = trace
    frame #0: 0x0000000100000598 crashy`main + 160
(lldb) process continue -F
Watchpoint 1 hit:
old value: 4295000208
new value: 70372480057344
* thread #1, stop reason = watchpoint 1
    frame #0: 0x00000001000005a0 crashy`main + 168
```

`70372480057344` is `0x4000dead0000`, the garbage `crashy` stores; `4295000208` is `0x100008090`,
`&g.buf[0]`. Going backward a watch stops **before** its store, so memory still holds the value
from before it, which lldb prints as `new value:` against the garbage it last saw. Going forward it
stops **after** the store, which is why `-F` re-reports the same store, retired. The server
re-parks `Exec`'s stops to make that true (spec §3c) and advertises
`watchpoint_exceptions_received:after`, without which lldb would step forward after every reverse
watch stop and could never get backward past the most recent write (t0 L4c). The session is
deterministic: `lldb_e2e` runs it twice and compares the transcripts, port and script path
normalised.

**lldb's direction sticks.** After `process continue -R` or `rsi`, a plain `process continue` (or
`c`) also goes **backward**, until `process continue -F`. That is lldb's design (t0 L3), not the
server's. What `thread step-inst` does while the direction is reversed is unmeasured, so go
forward with `process continue -F` first.

`lldb_e2e` also drives `breakpoint set -a <addr>` (with `-i <n>`), `breakpoint delete`,
`register read pc`, `memory read`, `bt`, `thread step-inst`, `thread step-inst-over`, `next`,
`thread step-out`, `finish`, `thread list`, `thread select`, and
`process plugin packet monitor where`, which prints the server's own cursor,
`at (n, k) phase=<Sys|Bp|Watch> pc=… thread=…`. The server describes the general and FP registers
(x0–x28, fp, lr, sp, pc, cpsr; v0–v31, fpsr, fpcr) and reads them for any live thread, from its
saved context when it is not the running one. A motion that fails (a replay divergence, say)
re-seeks to where it started and answers a stop whose reason is the failure's text. The server
never answers a resume with an error, which would cost lldb its connection (t0 L7).

**Stepping over a call, and stepping threads, since M44.** Each item below is pinned by a wire row
in `gdbserver_e2e`, a bounded lldb session in `lldb_e2e`, or both (and the `__PAGEZERO` omission by
a unit test in `rsp.rs` too).
- **`thread step-inst-over` (`ni`), `next`, `thread step-out` and `finish`** over a call stop at
  the return address, the `bl`'s pc + 4, as native lldb does on the same binary: measured on
  `crashy`'s first `bl`, into `fstat`'s dyld stub. Until M44 none did. Each needs the saved return
  address off the stack, and the server's image list carried the executable's `__PAGEZERO`
  (maxprot 0, `[0, 4 GiB)`), which lldb's Darwin loader turns into an invalid-memory region: lldb
  then failed every read below 4 GiB locally, with no packet sent, and a retrace guest's main stack
  lives there (`DYN_STACK_TOP = 0x280_0000`). So `bt` stopped at frame #0, `ni` degraded to a bare
  step, `step-out` and `finish` failed with `Could not create return address breakpoint`, and `next`
  landed on the call's target. The server now leaves `__PAGEZERO` out (below), and `bt` from the
  stub shows `crashy`main + 60` as frame #1.
- **`bt` on an arm64e guest unwinds through PAC-signed return addresses.** `qHostInfo` carries
  `addressing_bits:47`, the guest's VA width (`T0SZ = 17`). Without it lldb-2100 printed frames #1
  and #2 of `btchain` with their signatures still on and unsymbolicated, and lost `start`; with it
  `bt` is `f3`, `f2`, `f1`, `start`.
- **A step across the stepped thread's own exit stops at the exit.** `thread step-inst` on the
  `svc` of `__bsdthread_terminate` stops at that landmark's boundary with `reason:exception`,
  `thread N exited during the step`, named on the thread that runs next, where it used to run to
  the end of the recording. lldb does not display that stop (Known limits), but its follow-up
  `continue` is now a real one: it stops at a breakpoint the other thread reaches after the exit,
  where before M44 it ran past every such breakpoint to the end. That includes a breakpoint at the
  very pc the other thread resumes at, because the stop is parked at the crossing's own position,
  as a blocked step's is (M44's final review, Ruling FW-b). Parked as an arrival, which it was
  until then, that breakpoint was stepped over in silence: measured under lldb, `thread step-inst`
  now ends at `breakpoint 2.1` on thread 1 with a hit count of 1, where it had run to the end with
  a hit count of 0 (`docs/sweep-evidence/2026-09-27-m44/fw2/`).
- **Another thread's hit ends a blocked step.** While the stepped thread is blocked in a syscall, the
  run until it runs again now has the user's breakpoints and watchpoints armed. A hit by another
  thread ends the step with a `reason:exception` stop on the **stepped** thread that names the hit —
  lldb shows `stop reason = thread 2 hit breakpoint at 0x1804ecc14 during thread 1's step` — and is
  re-parked exactly as `continue` would park it (a watched store stepped to retirement, so `m`
  reads the new value; a syscall write at `(n, 0, Bp)`), so a reverse `continue` finds it again.
  That is M43's measured-safe L7 form: lldb sends one `vCont;s` and stops. It also stops a forward
  `process continue` whose first act is lldb's own step-off from a breakpoint on a blocking `svc`,
  at another thread's hit during that wait (measured once, by Task 11's probe).
- **A step of a thread that is not running runs until that thread is scheduled, then steps it**
  (`thread select 2; thread step-inst`), as a blocked step already did. M43 refused it in place. A
  step of a thread that has exited, or never existed, is refused in place: `cannot step thread N:
  it is not a live thread`. If the recording ends first, by another thread's exit or crash, the step
  is refused **on the stepped thread**, parked at the end: `the recording ended (thread 2 crashed:
  pc=… far=… esr=…) before thread 1 ran` (`… ran again` for a blocked step). Answered on the thread
  that ended it instead, that end made lldb re-step the thread it had asked for until the session's
  120 s bound killed it: 508, 633 and 538 × `vCont;s` for a crash in each arm and for an exit.
  Refused, each is one step, and lldb prints `END` and exits 0.
- **`monitor disarm-rsi`** clears what `arm-rsi` set, and `rsi` sends it when lldb's reverse resume
  fails after arming, so the next `process continue -R` is a reverse continue again rather than one
  step back. The `rsi` side has no automated test: the server never sends the plain-signal stop
  that makes a reverse resume fail (t0 L4d); the wire row drives both monitor commands.

**`--exe <path>`** names the recorded executable. lldb symbolicates the exe (``crashy`main + 204``
above) from an image list the server builds out of the recording's own opening snapshot: the
Mach-O header and load commands at `0x1_0000_0000`, where every guest's executable loads, slide 0.
The trace does not hold the executable's path. For a `record-dyn` recording the server takes it
from `argv[0]` on the opening stack when that is absolute; `--exe` supplies it otherwise (a static
`record` guest, or one recorded by a relative path) and takes precedence over `argv[0]`. With no
path the server advertises no image list and lldb's older loader runs, with no symbols for the exe
until lldb is given the file: t0 L2 measured `target create <exe>` then
`target modules load --file <exe> --slide 0` doing that, against t0's own stub rather than this
server. Since M44 the image list leaves out the executable's `__PAGEZERO`: in a retrace guest
`[0, 4 GiB)` is not unmapped (the main stack lives there), and lldb turns a listed maxprot-0
`__PAGEZERO` into an invalid region no packet can reach. That is a divergence from a stock Mach-O
segment list, specific to retrace's guest layout.

`gdbserver_e2e` (39 tests) guards the protocol with a Rust RSP client and needs no lldb, so it runs
on any machine that runs the VM tests. `lldb_e2e` (12 tests) guards what lldb itself does with it,
and skips loudly without lldb. Known limits lists what the server does not do.

## Known limits

These are real and current, not aspirational gaps.

- **Five rows of 54 sampled Apple system binaries are not clean, and since M36 the sweep says
  why each one is not — since M37, the same why from any recorder pid.** `tools/apple-sweep.sh`,
  over the committed 54-entry corpus `tools/apple-sweep-binaries.txt`, records and replays each binary and prints a `TALLY` line, so
  since M29 this figure is **reproducible instead of remembered**. Since M36 the sweep also prints
  *why*: every row carries the recorder's exit code, the replay's, the recorder's pid, the first
  `RECORD ERROR:` / `panicked at` line and the replay's `DIVERGENCE` line (one machine-readable
  `ROW` line per binary beside the human one); a refused recording — the recorder's `RECORD
  ERROR:` line is the test, its exit code printed beside it — is labelled `FAIL … (record error,
  rc=4: …)` rather than `replay diverged`; an identical crash on both sides is labelled
  `PASS … (identical fault, rc=N)` rather than a bare PASS; `RETRACE_SWEEP_KEEP=<dir>` keeps
  each non-clean row's stderr and trace, and since M37 `RETRACE_SWEEP_KEEP_ALL=1` keeps every
  row's, PASS rows included — what the M37 audits were run over. **M45 ran it once on 2026-09-28**
  (branch commit `09a105f`, recorder pids 50761–56189 = `0xc649`–`0xdb7d`; `09a105f` touches only
  `apple_walls_e2e.rs` and evidence, so the swept record and replay paths are Task 2's `28fddc4`,
  and the one later `crates/` change, `64712bd`, only puts the refusal panic's `args` on one line)
  and tallied **`pass=49 fail=5 skip=0`**, M44's tally from different rows. M44 had run it once on
  2026-09-27 (branch commit `5ee07b8`, recorder pids 35186–36890 = `0x8972`–`0x901a`; the two task
  commits after it, `50c81df` and `cbe59d7`, touch only the debugger and its RSP server, which the
  sweep never runs) for the same tally. M39 had run it once on
  2026-09-21 (branch commit `123cb97`) for `pass=44 fail=10 skip=0`, and M38 once on 2026-09-16
  (branch commit `911214e`) for the same — one regime each time, because M37 had already shown
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
  | M39 | 29138–32207 (`0x71d2`–`0x7dcf`) | inside `[0x4000, 0x10000)`, the trampoline page again — irrelevant since M37 | `pass=44 fail=10 skip=0` |
  | M44 | 35186–36890 (`0x8972`–`0x901a`) | inside `[0x4000, 0x10000)` again — irrelevant since M37 | `pass=49 fail=5 skip=0` |
  | **M45** | 50761–56189 (`0xc649`–`0xdb7d`) | inside `[0x4000, 0x10000)` again — irrelevant since M37 | `pass=49 fail=5 skip=0` |

  Against M44's run, **49 rows are unchanged and 5 differ**
  (`docs/sweep-evidence/2026-09-28-m45/README.md` has the row-by-row diff, `rowdiff.txt`). Each
  move was measured against a base binary, M45's `a78f28f` built from `git archive` (its crates are
  M44's close), alternating with the swept one on the same host:

  - **`automationmodetool`** is still `FAIL` 101/n/a. It is the one row M45's diff moved: the init
    is emulated, and the run reaches the second `kevent_qos` shape, refused by value (the face
    below).
  - **`/bin/ps`** went `PASS` 0/0 → `FAIL` 0/3, a **class-E row**, the kind the M36, M38, M39
    and M44 runs each recorded none of. Record and replay each ran to the end: replay consumed
    all 16043 syscall landmarks (`#16043` is `Exit` 0) and stopped at `#16044`, the final full-memory compare, on `memory divergence at ipa
    0x701414078: replay=0xf5 recorded=0x00`. The cause is **host state**. Landmark `#253`, the
    process-table `sysctl`, wrote that byte, and nothing later writes it. `#16040`, three landmarks
    before `exit`, is a forwarded `madvise(0x701400000, 0x50000, MADV_FREE_REUSABLE)`. `madvise`'s
    row rebases it onto retrace's own backing, so on record the host kernel marks retrace's own
    pages reusable, and replay executes no syscall. In the final snapshot one whole aligned page,
    `0x701414000`, reads 0 non-zero bytes against 1,876 in `#253`'s write, while the other 11 pages
    `#253` filled match it count for count (`ps-pagemap.txt`). That page map is read as the host
    reclaiming a reusable page, which then reads as zero fill; it is a fingerprint, not an observed
    reclaim. It is not M45: the trace has zero 374 events, and the sweep's recording replayed on the
    base binary stops at the identical landmark, pc, ipa and bytes (`ps-replay-control.txt`). It is
    intermittent: 20 fresh record-then-replay runs, 10 per binary, were all clean
    (`ps-control.txt`). No fresh base recording reproduced the reclaim, because forcing one means
    real memory pressure on a shared host. This is the hazard M37 named and left unmeasured
    ("`MADV_FREE_REUSABLE` on the guest backing … the host may lazily reclaim those guest pages"),
    measured for the first time.
  - **`dddiagnose`** went `FAIL` 4/3 (msgh_id 205, landmark 455) → `PASS` 139/139, an identical
    fault: `guest crashed: pc=0x180302eb0 far=0x4000050050 esr=0x92000045`, a write with a level-1
    translation fault, the same on both sides. It is **host state**: of eight traced runs
    alternating the two binaries, each binary faulted once and stopped at msgh_id 205 three times
    (`dddiagnose-control.txt`, `dddiagnose-traps.txt`), with `far` varying run to run. Every run
    refuses the RCV-only receive once, and the two faulting runs fault about 9 landmarks after it
    (a sample of two). The runs part at one extra `gettimeofday`; before it only host-supplied
    values differ, after it the guest's region placement differs, and nothing was traced past that
    (`dddiagnose-fork.txt`, `dddiagnose-prefork.txt`). The root cause is not measured. The fault
    is M36's libsystem_malloc `mfm_alloc+0x230` face, which M37 retired as unreachable with a
    correctly forwarded pid; M45 reaches it with correctly forwarded pids, so that retirement is
    contradicted (the status log's M45 section has the pointer). It is also M38 R3's losing
    `MACH_RCV_TIMED_OUT` face, now seen under the winning `MACH_RCV_INVALID_NAME`, so R3's
    single-run choice is owed a re-measure. `INVALID_NAME` stays the refusal code.
  - **`csh`** went 338 → 336 and **`tcsh`** 333 → 336, at the same wall. Both sit inside M44's
    measured spread. Eight fresh samples, two per shell per binary, gave 336–344 traps, and 317 on
    all eight once the `gettimeofday` traps are subtracted, one more than M44's 316
    (`csh-samples.txt`). The base binary shows the same 317, so the shift is not in M45's diff.
    Whether the host or an M44 change after M44's sweep added the trap was not traced.

  M44's run, against M39's: **46 rows unchanged and 8 differ**
  (`docs/sweep-evidence/2026-09-27-m44/README.md` has the row-by-row diff), and every label that
  moved is a row M44 names. `ls` and `ed` went FAIL → PASS, rc 0/0, past the rows M44 added
  (`getattrlistbulk` 461 for `ls`; `openat_nocancel` 464 and `unlink` 10 for `ed`). `desdp`,
  `dyld_info` and `flex` went FAIL → PASS 71/71, **in the sweep's sense only** (below).
  `dddiagnose` stayed a FAIL, moved from its missing row (`statfs64` 345, now tabled) to a class-C
  record error. So 49 = 44 + `ls` + `ed` + the trio. The other two moves are landmarks, and M44
  measured their spread for the first time: `csh` went 346 → 338 and `tcsh` 341 → 333, at the same
  wall. Over eight recordings of `csh`, four on M44's base binary and four on the swept one,
  alternating, the trap count before the wall ranged 333–346 on both, and that count less the
  `gettimeofday` (116) traps was exactly 316 on all eight: the spread is the shell's own timing,
  not retrace's. `automationmodetool`'s panic line moved from `lib.rs:946:38` to `:982:38` only
  because M44's rows sit above `forwarded_shape`. The five rows M45's run left not clean, and
  `dddiagnose`, which that run counted a PASS as an identical fault but whose gate stays parked,
  each with its class, read off the kept evidence and never off the sweep's label, the gate that
  stands for it and where it is routed:

  | binary | face | class | gate (`crates/retrace/tests/apple_walls_e2e.rs`) | route |
  |---|---|---|---|---|
  | `/bin/csh` | `fork` — `mach_ports_register` | **C** new subsystem: process creation | `csh_records_and_replays` | parked, not routed |
  | `/bin/tcsh` | `fork` — `mach_ports_register` | **C** | `tcsh_records_and_replays` | parked, not routed |
  | `/bin/ps` | `memory divergence at ipa 0x701414078` at the final compare, rc/rp 0/3; intermittent (20 fresh runs clean) | **E** host state: a forwarded `MADV_FREE_REUSABLE` page reclaimed by the host before the final snapshot | none here; `sysbin_e2e`'s `ps_records_and_replays` runs | owed, not routed (M37's named hazard, first measured by M45) |
  | `/usr/bin/dddiagnose` | `host_get_io_main` (`mach_msg2` msgh_id 205), or, about one run in four, the `mfm_alloc+0x230` identical fault (139/139, this run's) | **C** new subsystem: the I/O Kit main port; the fault's root cause is unmeasured | `dddiagnose_records_and_replays` | parked, not routed |
  | `/usr/bin/automationmodetool` | a second `kevent_qos` (374) shape, refused by value: `M45: unmeasured kevent_qos shape: x3 (eventlist) is 0x27fedb8, measured 0x0` | **C** new subsystem: libdispatch's kevent source registration through the workqueue kqueue | `automationmodetool_records_and_replays` | parked, routed to its own milestone (M45 §7 Halt 3) |
  | `/usr/bin/yes` | 30 s watchdog | **D** not-a-defect | none | retired |

  **The xcrun trio's PASS is not the program's outcome.** `desdp`, `dyld_info` and `flex` are
  xcselect shims that dispatch by name: `dyld_info` and `flex` are one hard-linked file, `desdp`
  another. Each reaches xcrun's `posix_spawn`, which retrace refuses (exec-in-place is
  unmodelled), and exits 71 on both sides with identical stdout, so the sweep counts it a PASS.
  Natively, with the sweep's empty stdin, `desdp` exits 2 with its usage, `dyld_info` 0 with its
  usage on stderr, and `flex` 1 (`<stdin>:1: premature EOF`), each measured on its own by M44's
  final review (`docs/sweep-evidence/2026-09-27-m44/native-trio.txt`). None is 71. Their gates
  therefore stay parked at that refusal, class C, because asserting 71 would pin retrace's
  refusal, not the program. On the
  way, from a cold xcrun cache, each takes `openat_nocancel` (464) and `rename` (128) to rewrite
  `/var/tmp/xcrun_db`, and a recording's forwarded `rename` really installs that cache on the host;
  from a valid cache the trio skips both rows and goes straight to `posix_spawn` (M44 t0, Ruling
  T0-e). Task 4 measured each member alone from a cold cache: 464, `rename` three landmarks later,
  the refusal seven after. The M44 sweep ran with the cache present, and that path was measured
  after it: from the same cache, the M44 base binary (`ebd0266`, which has no 464 row) and the
  swept binary both record all three to 71/71 with no 464 or 128 trap. The sweep's trio move from
  M39 is therefore the host's cache, not the 464 row. The 464 gate and census claim rest on `ed`,
  whose 464 opens its own buffer file.

  `/bin/launchctl` left the table at M38: it is `PASS` 1/1, its own no-argument usage on stdout
  (4,484 bytes, byte-identical to the host's native output), and its gate runs, asserting on that
  outcome rather than on `rc == 0`. `ls` and `ed` left it at M44: `PASS` 0/0, and their gates,
  `ls_records_and_replays` and `ed_records_and_replays`, run.

  The faces, each in the recorder's own words. **A missing row**, the M33 fail-loud
  (`recorder panicked: … M33: syscall N (N) has no arg_kinds row in crates/retrace-arch/src/lib.rs
  — it cannot be forwarded unclassified …`, rc 101, no replay run, because the harness labels a
  recorder panic before replaying), is **reached by no sweep binary since M45**: the M45 sweep's
  log, `docs/sweep-evidence/2026-09-28-m45/sweep.log`, carries no `has no arg_kinds row` line. The
  last binary at it was `automationmodetool`, at 374. M44's t0 measured why no row could close
  that: libdispatch's `_dispatch_kq_init` issues `kevent_qos` with `x0 = −1` (no `kqueue` ran),
  flags `x7 = 0x21` (`KEVENT_FLAG_WORKQ | KEVENT_FLAG_IMMEDIATE`) and one `EVFILT_USER` entry,
  and forwarded, the workqueue flag would register on **retrace's own** process's workqueue
  kqueue, while libdispatch crashes on any errno but `EINTR`. M45 emulated that one measured shape
  as a modelled success, so the run now passes the init (landmark 362 in M45's walk) and stops at
  **the M45 refusal**: `recorder panicked: thread 'main' … panicked at
  crates/retrace-box/src/lib.rs:5222:13: M45: unmeasured kevent_qos shape: x3 (eventlist) is
  0x27fedb8, measured 0x0. …`, rc 101, no replay run, at the next landmark, 363. That is a second
  `kevent_qos` with an event list and `KEVENT_FLAG_ERROR_EVENTS`, refused by value and not
  modelled. Landmark numbers move with the host's `gettimeofday` count.
  **`host_get_io_main`** is `record error, rc=4: RECORD ERROR: unsupported
  mach_msg2 at pc 0x1804adc34: msgh_id 205 dest 0xc03 (guest task port Some(515)) send_size 24`,
  rc/rp 4/3, the I/O Kit main port (SDK `mach/mach_host.h:1313`); M44's Task 4 measured
  `dddiagnose` reaching it 21 landmarks after its `statfs64`, which records and fails `ENOENT`,
  harmlessly. **`dddiagnose`'s other face**, measured by M45, ends about one run in four before
  205, on M44's crates as well as M45's:
  `guest crashed: pc=0x180302eb0 far=0x4000050050 esr=0x92000045`, the
  `far` varying run to run, rc/rp 139/139, labelled `PASS /usr/bin/dddiagnose (identical fault,
  rc=139)`. That is libsystem_malloc `mfm_alloc+0x230`, about 9 landmarks after the refused
  receive in the two faulting runs traced. The **exec
  refusal** is `[retrace] refusing posix_spawn (syscall 244): exec-in-place is unmodelled;
  returning errno 14 without forwarding`, the trio's. **`fork`** is `record error, rc=4: RECORD ERROR:
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
  **The malloc crash has recurred since, with correctly forwarded pids**: it is `dddiagnose`'s
  other face above, so M37's retirement of it ("neither face can be reached with a
  correctly-forwarded pid") is contradicted. The `brk` has not recurred; the M45 sweep's log has
  no `brk` row.
  The **watchdog** is `timed out after 30s recording`: `yes` never terminates and is failed on
  purpose. The `refusing mach_msg2 message-queue send` line that precedes the receive is M23's
  *serviced* refusal of the SEND|RCV shape, survived by every guest that reaches it and
  unseparated from the receive (no run reaches the receive without it); since M38 a second line,
  `refusing mach_msg2 message-queue receive`, follows it on the six rows. Evidence:
  `docs/sweep-evidence/2026-09-28-m45/` — M45's sweep log, every non-clean row's stderr under
  `sweep/`, the row diff against M44, the `ps`, `dddiagnose` and `csh`/`tcsh` controls against the
  base binary, and the walk of `automationmodetool` and the GCD candidates (its README says what
  each file is); `docs/sweep-evidence/2026-09-28-m45-t0/` — M45's t0;
  `docs/sweep-evidence/2026-09-27-m44/` — M44's sweep log, every non-clean row's stderr under
  `sweep/`, the trio's re-run with its `rec.err` kept, and the eight-run `csh` measurement;
  `docs/sweep-evidence/2026-09-27-m44-t0/` — M44's t0 (its README says what each file is) and
  Task 4's per-binary re-measure under `t4/`; `docs/sweep-evidence/2026-09-16-m38/` — the refusal-code measurement (18 cells, all three
  candidates' stderr), the M38 sweep log and every non-clean row's stderr under `sweep/`, and the
  `/bin/ed` and `csh`/`tcsh` traced runs; `docs/sweep-evidence/2026-09-13-m37/<basename>.{N,I,S}.{rec,rp}.err`,
  verbatim, with the counting rules, the reader and the three audits in that directory's README;
  the pre-fix faces, the symbolication and the M36 counting rules are in
  `docs/sweep-evidence/2026-09-13-m36/`.
  The old label "replay diverged" appears in no gate reason and in none of the table's cells: the
  replay of a recording that ended at a `RECORD ERROR` *always* reports a `DIVERGENCE` — it runs out
  of events one past the trace's last syscall — and in every cell where a replay ran (24 of 24
  `rc=4` traces, `events == landmark` in each) record and replay agreed, the replay re-reporting
  the recorder's own stop. `ps`'s M45 row is the case the label was meant for: its recording is
  complete, and its replay's divergence at the final compare is its own (class E, above).
  **The pid-collision probe is retired: a `Scalar`-marked register is never probed — M37.** The
  window `[0x4000, 0x18000)` and its 82 % of the pid space, the `brk`, the identical crash and its
  varying `far`, and which of M35's and M36's runs fell where are history, in `docs/status-log.md`
  (M34 §4b, M36's corrections (a)–(c), M37's acceptance). M37 retired the two open questions M36
  attached to the probe (why a colliding run took the crash rather than the `brk`; the `far`) on
  the grounds that neither face could be reached any more. **For the crash that no longer holds**:
  M45 reached it with correctly forwarded pids (`dddiagnose`'s other face, above), with a varying
  `far` again, so why a run takes it is open once more, and owed. What the retirement leaves is in
  the descriptor entry below: positions past a row's arity keep the probe on purpose, and a `Ptr`
  that is sometimes a number is the same class behind a different kind.
  **History, kept short.** The corpus is a **reconstruction**: the sample behind the 47 published
  at M27 (46 at M23, 34 at M22) was never committed, so today's figures are not strictly comparable
  to those; they are simply the first a later reader can re-derive. `/bin/launchctl` had always
  been diverging until M38 — the script's first draft compared a variable against itself, making
  its exit-code check a tautology that reported four binaries as passing when they were not, and
  fixing it is what exposed `launchctl`; the receive refusal is what cleared it. `/usr/bin/yes`
  cannot pass under any method that requires a bounded
  comparison and is counted a FAIL **on purpose**, since excluding it would raise the tally without
  changing anything about retrace. The `identical fault` rows are still counted in `pass` so the
  tally series 46/8 ↔ 45/9 ↔ 44/10 ↔ 49/5 stays comparable across M33–M45 (none occurred at M37,
  M38 or M44; M45's run has one, `dddiagnose`);
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
  sentinel honoured each ran on to a missing row and failed loud, which is why the tally *fell*
  by two at a milestone that fixed a defect. An M10-class wrong descriptor is deterministic on
  both sides, so a translation fix moves a binary here only by letting it reach something else —
  there, two rows the census never saw, which M44 then tabled. M44's xcrun trio is the same lesson
  from the other side: three rows that exit 71 on both sides at retrace's own refusal count as
  PASS, which is why their gates, not the tally, carry them.
- **A guest must be arm64 or arm64e.** `slice_native` picks the slice this machine would execute —
  arm64e if the file has one, else plain arm64 — so universal files work, but an `x86_64`-only
  binary is refused by name. There is no emulation of another ISA and none is planned.
- **The record-side diff window still truncates for most syscalls. Since M30 the guard band catches
  a kernel write whose *effect* it cannot see — but only on the bands it is allowed to fill, and that
  exclusion is large.** `forward_and_diff`
  snapshots a pre-image window per pointer argument and diffs that same window; the window widens to
  the real length only where `retrace_arch::dest_buffer` knows it. M26 covered `read`(3)/`pread`(153)/
  `read_nocancel`(396); **M27 added `sysctl`(202)** (length at `*(size_t*)x3`, unbounded, and the
  reason `/bin/ps`'s process table now records whole; M45's one sweep failure of `ps` is a
  different mechanism, a host-reclaimed page, in the sweep entry above) and
  **`pread_nocancel`(414)**, which before M27 was missing from
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
  cannot distinguish from no write at all; `/bin/ps`'s process table records whole today because
  `dest_buffer` covers `sysctl`, never because the band caught it. M30 fills each shrunk band with
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
  to be **rewritten rather than defended** when exec-in-place lands. Since M44 the same refusal
  parks three Apple-sweep gates, the xcrun trio's (`desdp`, `dyld_info`, `flex`; the sweep entry
  above).
- **A syscall with no row cannot be forwarded — closed structurally at M33 — but a row's
  descriptor positions are still only as right as the reviewer, one number the corpus reaches has
  no row by design, two `_nocancel` twins escape their plain forms' arms, and `x1` is written for
  one syscall only.** Before M33, a syscall that took a descriptor but was missing from
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
  `[0, DUP2_MAX_FD)` is `EINVAL` on both sides rather than record-only; since M44
  `F_DUPFD_CLOEXEC` also sets `FD_CLOEXEC` on that host `dup` — so both commands share the path),
  and **`pipe`** binds
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
  milestone can take with the field already in the trace. **The rows M38 left missing are tabled
  or routed since M44.** 461 `getattrlistbulk` (`[Fd, Ptr, Dest(Reg(3)), Scalar, Scalar]`: xnu
  never checks `bufferSize`, so there is no cap to cite and the window follows the caller's
  length), 464 `openat_nocancel` (`openat`'s row) and 345 `statfs64` (`[Path, Ptr]`, `fstatfs64`'s
  path twin) are tabled, with `unlink` (10) and `rename` (128), which M44's t0 found behind them.
  374 `kevent_qos` is **emulated since M45**, for exactly the measured workqueue init
  (`KEVENT_FLAG_WORKQ | KEVENT_FLAG_IMMEDIATE`, `x0 = −1`, one `EVFILT_USER` `EV_ADD|EV_CLEAR`
  entry, no event list), returning 0; its row is documentation only. Any other `kevent_qos` shape
  is **refused by value**: the recorder panics with `M45: unmeasured kevent_qos shape: …`, naming
  the argument or change-entry field that differs, its measured value and its actual one.
  `kevent` (363), `kevent64` (369) and `kevent_id` (375) have **no row**, so each still stops at the
  M33 panic. 468 is `fchownat`; `getattrlistat` is 476; neither
  is reached, and neither has a row (M44 R1). **Two `_nocancel` twins escape their plain forms'
  arms**: `sigsuspend_nocancel` (410) and `__sigwait_nocancel` (422) are in neither
  `retrace_arch::is_signal_syscall` nor `record_box`'s sigsuspend/`__sigwait` panic arm, which
  match 111 and 330 only (M44 t0 M4). Both are row-less, so today each still fails loud at the
  forward, but the M33 panic's advice, "add the row", would be wrong for them: a row would forward a
  blocking signal wait into retrace's own thread, and `nocancel.rs` would not object, since both
  sides of each pair are row-less. No corpus guest reaches either (0 hits in t0's eleven kept
  traces); the fix, when one does, is the two numbers in each of those two places. What stays open
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
- **Exclusive (LL/SC) pairs: what M42's shadow monitor does not cover.** A debugger exit inside a
  pair now keeps the store as recorded (What works today). These are its residuals. Where a
  residual leaves a pair with no shadow, the debugger falls back to pre-M42 behaviour, which t0
  measured as a divergence, a hang, phantom watch hits or a wrong `no earlier hit`: loud or not,
  depending on the shape.
  - **An asynchronous host interrupt between the halves**, during record or replay. A host IRQ's
    exception return lands below anything retrace sees and clears the monitor. The spec estimates
    about 10⁻⁶ per sequence; nothing has measured it. For a discard-status pair it is a **loud**
    divergence, never a silent wrong recording.
  - **Inference at a native stop rests on assumptions it has not measured**: the pair is reached by
    fall-through, nothing branches into it from outside, and no plain store of an *identical*
    value hits the marked bytes before the stop. A fourth, that the base register is not rewritten
    between the halves, has been **checked** since M42 (spec §3d condition 5): every instruction
    between them must be a data-processing instruction that does not write the base, or a
    conditional branch, or nothing is inferred. When every destination register is rewritten
    between the halves, as in an in-place retry loop (`ldaxr x1, [x0]; add x1, x1, #1;
    stlxr w2, x1, [x0]`), the register-equality check has nothing left to compare, and "nothing
    branches into the pair" is then the **only** guard against a jump into its middle. Every
    LL/SC sequence the M42 census found satisfies all four. A violated one could emulate a store
    the recording did not make. That is **not loud by construction**: the divergence oracle sees it
    only if it changes a later syscall or the final memory.
  - **No inference past 16 instructions or across a page.** A load-exclusive more than 16
    instructions before a native stop, or on another 16 KiB page, is not found. The stop infers
    nothing, which is the pre-M42 fallback above. No census sequence is that long or straddles a
    page.
  - **`run()` drops a shadow it cannot finish in 16 steps** (plan R10). Two shapes get there: a
    load-exclusive whose sequence a branch left, and a straight-line sequence whose
    store-exclusive lies more than 16 instructions past `run()`'s entry. A store-exclusive to the
    marked bytes after the drop, with no exit between, would then fail under the debugger where it
    succeeded natively. No census shape does it. What it costs depends on the shape. For a
    discard-status pair it is **loud**: the replay diverges. A retry loop **absorbs** it: the loop
    retries the lost store and replay reaches the recorded end, though a native scan's hit counts
    can drift, which `resolve_nth` usually turns into a loud error. Anything else is
    shape-dependent: a one-shot CAS depends on what its caller does with the failure. The bound
    and the drop are pinned by `llsc_e2e`'s
    `a_shadow_outliving_its_sequence_is_dropped_at_the_step_bound`, on its own fixture.
  - **`wfe`** (EC `0x01`), including `ldxr; wfe` spin-waits, is unhandled on every path, not only
    under stepping. What such a guest does under retrace is unmeasured.
  - **Byte, halfword and `ldaxp` load-exclusives are unmeasured.** t0 measured the step exit's
    ISS.EX bit on `ldxr`, `ldaxr` and `ldxp` only. Since M43 `step()` decodes the instruction
    before it runs it, and a retire sets the shadow only as `excl::classify_retire` decides: with
    ISS.ISV = 1, ISS.EX must agree with the decode in both directions, or the step panics naming
    the disagreement; with ISV = 0 the decode decides. So a form that did not report ISS.EX now
    fails loud where M42 stepped it with no shadow. ISV = 0 itself is unmeasured on this core: M42's
    t0 saw ISV = 1 on every retire, and the ISV = 0 path is unit-tested only.

  Everything the shadow refuses is **loud**, a panic that names the check and the pc: a stepped
  instruction whose syndrome and decode disagree about being a load-exclusive, and a
  store-exclusive that does not match its load (address, size, pair), is misaligned, aliases its
  status register or finds the bytes changed since the load, **when its target is mapped and
  EL0-writable**. A store-exclusive whose target is **unmapped or not EL0-writable** is not
  emulated at all since M43: the shadow is dropped and the store is stepped natively, whatever the
  validator refused it for (Ruling T1-a), M42's deliberately loud refusals above included. Natively,
  with the monitor held, that store faults, so the recording holds a crash. Whether the core still
  raises the fault once retrace's exit has lost the monitor is IMPLEMENTATION DEFINED, and M43
  measured that it does, **outcome (a)**: on `llscedge.s`, whose recording crashes at
  `pc=0x1000003a0 far=0x1000003b0 esr=0x9200004f` (a permission fault), the stepped native store
  takes that fault and a `continue` reaches the recorded crash with no divergence, pinned by
  `llsc_e2e`'s `a_stepped_store_exclusive_to_a_read_only_word_ends_in_the_recorded_crash`. Had it
  been (b), the store would have failed with status 1 and the replay's divergence oracle would have
  failed loudly against the recorded `Event::Crash`. A load-exclusive whose base is also its
  destination (`ldxr x9, [x9]`) no longer panics either: `step()` reads the base before the load
  overwrites it (`a_load_exclusive_whose_base_is_its_destination_steps_without_panicking`). That
  decode costs one extra guest read, a page walk and a word, on **every** step, not only inside a
  pair. M44's t0 measured it on M41's step-bound hit oracle
  (`oracle_threadrust_breakpoints_at_both_switches`, median user CPU of three runs on a quiet
  machine): **21.27 s** before the decode (`c652cf1`) and **30.90 s** after it (M43's close), a gap
  of 9.63 s (+45 %; M43 had measured +38 %), with rung 8's reverse demo flat. **M44's backing index
  recovered a third of it, not the half it was meant to**: every guest read now finds its backing by
  binary search (What works today), and the same test's median is **27.63 s**, 3.27 s of the gap,
  against the spec's bar of 26.08 s. The miss is routed, not waived. A profile names what is left:
  about 6.0 s of the ~6.4 s residual is `Box_::insn_at` in `step()`'s pre-decode, which resolves the
  pc through a full guest page-table walk on every step — four index lookups (about 3.2 s in this
  unoptimised build), a heap allocation, copy and free per read (about 1.8 s), and an HVF
  `SCTLR_EL1` read. The CLI's debug children did not grow; all of it is in the test process's
  in-process oracle. Owed, proposed and unmeasured: a per-page VA → IPA or decoded-instruction
  cache, invalidated on remap and W^X promotion, and a non-allocating four-byte read. The hit
  oracle (`tests/util/hits.rs`) is no longer limited by pairs: on
  `threadrust` it starts at landmark 1 again, where M41 had started it at the `bsdthread_create`
  landmark to stay clear of dyld's three `getpid` pairs. It now steps through all three, at
  22.95 s CPU for that test at M42's close and 27.63 s median user since M44's index (above),
  against M41's 120 s budget.
- **Debugging with lldb: what `retrace gdbserver` does not do.** Since M43 (What works today). Each
  is by design or measured, unless it says otherwise.
  - **No interrupt.** `process interrupt`, ^C and the `0x03` byte do nothing during a motion: the
    server does not read the socket while it replays, so a long motion runs to completion (a
    `process continue -R` is a full forward replay from landmark 1, above), and lldb waits, because
    its resume packets have no timeout (t0 L9).
  - **Six breakpoints and four watchpoints at most, and step-over needs one of the six.** A seventh
    breakpoint or a fifth watchpoint is refused, which lldb reports as a clean creation error.
    lldb's step-over, step-out, step-in and `ni` over a call each insert one transient breakpoint
    that the server cannot tell from a user's (t0 L5), so **with all six breakpoints the user's**
    that transient is refused and lldb runs on to the next stop instead of stepping. With five or
    fewer, stepping is unaffected. (Spec R4 was amended from five to six at Task 3's review: a cap
    of five only moved the runaway from six user breakpoints to five.)
  - **Watchpoints are writes only.** Read and access watchpoints are refused: retrace watches
    writes. A watch's length must be 1, 2, 4 or 8, and its address a multiple of it.
  - **lldb does not display the stop at a thread's own exit** (M44 Ruling T8-a, as corrected). A
    step across the stepped thread's own exit stops at the exit on the wire, named on the thread
    that runs next (What works today). lldb-2100 had suspended that thread for the step, and it
    ignores a stop on a thread it suspended (`Thread::ShouldStop … should_stop = 0 (ignore since
    thread was suspended)`, `docs/sweep-evidence/2026-09-27-m44-t0/t8/t8-lldb-steplog.log` line
    46), so it sends one `c` and runs on. That `c` now stops at the user's breakpoints after the
    exit, so the step is not wasted, but its own stop is never shown. The successor is a
    one-argument change, naming the stop on the exited thread, plus two decisions: whether that
    thread joins `threads:` for that one stop, and which context to report, since its stale saved
    pc measured `0x4404`, not its exit `svc`. lldb's reaction to either is unmeasured.
  - **Another thread's syscall write during a blocked step has no fixture.** A breakpoint or a
    watched store hit by another thread while the stepped thread is blocked ends the step (What
    works today), and both have rows. A syscall's write to a watched range by another thread has
    none: in every candidate fixture the writing thread stores to that buffer first, so the store
    ends the step before the syscall writes (M44 Task 11, probed on `threadrust`, presumed for the
    rest). The path reuses the forward syscall-watch re-park, which rows do cover. Its successor
    needs a fixture whose other thread's syscall writes a buffer that thread never stored to.
  - **lldb applies no hit count, ignore count or condition to another thread's hit during a
    step** (M44 B6(a), L7's form). Such a hit ends the step as an exception on the **stepped**
    thread, naming the hit, so lldb never sees it as a hit of that breakpoint: it adds nothing to
    the breakpoint's hit count, spends none of an ignore count (`breakpoint set -i`), and evaluates
    no condition. This is inferred from the reply's form and unmeasured. What was measured is that
    the other thread's row in `thread list` shows no stop reason (M44 Task 11). lldb's step-off from
    a breakpoint on a blocking `svc` inside a plain `process continue` is such a step, so a
    conditional breakpoint on another thread can stop that `continue` unconditionally, and one with
    an ignore count can stop it at a hit the count should have passed.
  - **A breakpoint lldb lifts for its step is not armed for the other threads while the step
    waits** (M44 Ruling T12-b). Before it steps, lldb removes the breakpoints at the threads'
    current pcs (Task 11's probe measured it lifting both the one at the stepping thread's `svc` and
    one at the other thread's saved pc). During a blocked step, or a step of a thread that is not
    running, the run until the stepped thread runs arms only what is inserted, so another thread
    passing a lifted address is not reported (inferred from the code, not measured with lldb).
    Arming it anyway is not the obvious fix: lldb believes no site is inserted there, and what it
    does with a hit it cannot match is unmeasured.
  - **M43's T5-a stall is retired by inference, not by measurement.** M43 measured a breakpoint added at
    a non-running thread's pc stalling every forward `process continue` in place: lldb steps that
    thread off it first, and the server refused any step of a thread that was not running. M44
    replaced that refusal (a step of such a thread runs until it is scheduled), so the step-off
    should now run; the stall's own lldb shape has not been re-run since.
  - **The end of the recording during a step reads as a refusal on the stepped thread** (M44
    Ruling T12-a). When the recording ends, by another thread's exit or crash, before the stepped
    thread runs, the step is refused on the stepped thread, naming the end (What works today). So a
    crash shows as a description on the stepped thread rather than as the crashing thread's signal.
    lldb still shows `EXC_BAD_ACCESS` on the crashing thread's own row only where it had already
    stopped at the crash before the step (the permanent lldb row's shape). When the crash is first
    reached inside the step (the blocked arm), the crashing thread's `qThreadStopInfo` reports no
    stop reason, because the last stop is the stepped thread's, and lldb's `thread list` showed
    that row with none (M44 Task 12's fix round). lldb measured three shapes: a crash while
    stepping a thread that is not running, a crash during a blocked step, and an exit while
    stepping a parked worker. Only the first has a permanent lldb row; the
    other two have wire rows. A step of a live thread that never runs again (one blocked for good,
    a parked workqueue worker) waits for the next hit or for that end, and no row runs that wait
    forward to the end: the rows start at the terminal.
  - **Only the executable is symbolicated.** dyld and the shared cache are never listed (spec R5:
    listing dyld plants a persistent internal breakpoint, one of the six), so frames in dyld and
    `libsystem` show bare addresses under lldb, although the script debugger names dyld's (M19).
  - **No expression that runs code.** Register and memory writes are refused, because a recording
    is read-only, and refusing the register write is also what stops lldb's expression evaluator
    from resuming the replay under registers it invented (t0 L8). Constant expressions still
    evaluate.
  - **A reverse step exists only as `rsi`**, since stock lldb never sends one (t0 L3), and lldb's
    sticky direction is documented, not fixed: after `process continue -R` or `rsi`, a plain
    `continue` goes backward until `process continue -F`.
  - **Two lldb cosmetics.** lldb re-indexes a thread id it has seen exit (Task 5's probe listed tid
    2 as `#3`), and after a step of another thread it keeps a stale `breakpoint 1.1` stop reason on
    the thread it suspended, even for a breakpoint since deleted. Both come from lldb, not the
    server.
  - **The CLI's `where` still prints no phase** (M41's owed item). The server's
    `process plugin packet monitor where` prints the phase of its own cursor.
- **A bad debugger operand now fails later than it used to.** `where; break zzz` printed nothing and
  exited 5 before M20; it now runs the `where`, prints it, then fails — still exiting 5. That is the
  measured price of resolving at execution rather than at parse, it is deliberate, and a test pins it.
- **The script debugger has no DWARF, no line numbers and no backtraces.** M19 reads `LC_SYMTAB`
  only, so an address becomes `_child+0x30` and never `crashthread.c:35`, and `retrace debug` has no
  unwinder, so it prints no stack trace. Under lldb (M43) lldb's own unwinder runs over the
  registers and memory the server serves, and since M44 it is measured past frame #0: from
  `fstat`'s stub in `crashy`, `bt` gives `crashy`main + 60` as frame #1, and on the arm64e
  `btchain` all four frames through PAC-signed saved LRs (What works today). Line numbers from
  DWARF in the executable on disk are unmeasured.
- **The trace format is not stable.** `TRACE_MAGIC` broke in M15, M16, M24 and again in M38.
  Recordings are currently working artifacts, not things to keep across milestones — and M24 is the
  milestone that made the refusal honest, so a stale one is now rejected at open instead of
  half-read.
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
  symbol wall above. Seven are in `crates/retrace/tests/apple_walls_e2e.rs`, one per sweep row
  that is retrace's to fix or model and has a gate: the four with a gate in the table above
  (`dddiagnose` included, although M45's run counted it an identical-fault PASS) and the xcrun
  trio. Eight were parked
  by M36, **moved by M37** to the walls it measured, and at **M38 one was un-parked** (`launchctl`
  — the RCV-shaped `mach_msg2` is refused and it runs to its own usage exit; the test asserts on
  that, not on `rc == 0`) **and five moved in place** to the first missing `arg_kinds` row behind
  the refusal (class B: `kevent_qos` 374, `openat_nocancel` 464 ×3, `statfs64` 345). **M44 moved
  those five again**, each reason rewritten: `desdp`, `dyld_info` and `flex` to the `posix_spawn`
  refusal (class C, exec-in-place; un-parked when exec-in-place is modelled), `dddiagnose` to
  `host_get_io_main` (class C, the I/O Kit main port), and `automationmodetool` stayed at 374,
  reclassified class C and routed to its own milestone (M44 R4). **M45 emulated that 374 and
  re-parked `automationmodetool` at the new wall behind it**: the init records at landmark 362
  (rc 0, no writes, thread 0), and landmark 363 is a second `kevent_qos` shape that M45 refuses by
  value and does not model (M45 §7 Halt 3). That call has a 16-entry event list, `x7 = 0x23`
  (`KEVENT_FLAG_WORKQ | KEVENT_FLAG_ERROR_EVENTS | KEVENT_FLAG_IMMEDIATE`), and one entry with
  filter −14, flags `0x0185` and fflags `0xf0000037`; by those values it is libdispatch's own
  memory-pressure source, which is inferred and not symbolicated. It stays class C, routed to its
  own milestone, and is un-parked when that shape is measured and modelled and whatever comes next
  is cleared. M45 also gave `dddiagnose`'s reason its second, faulting outcome
  (`mfm_alloc+0x230`, about one run in four), and it is now un-parked only when msgh_id 205 is
  serviced **and** that face is explained or measured absent, because servicing 205 alone would
  leave a gate that fails about one run in four. `csh`/`tcsh` stay at `fork` (class C:
  `mach_ports_register` from `xpc_atfork_prepare`, `fork`(2) behind it; un-parked when the box
  models process creation) with their `pipe` landmark refreshed — each reason the measurement
  that parks it — the label, `rc`/`rp`, the recorder pid and its regime, the landmarks, the
  recorder's own line with its symbol, the evidence file, the class, and what un-parks it — and
  each run once with `--ignored` to show it fails for exactly that reason (M37: 8 of 8 at pids
  72339–72381; M38: the five re-parked, each printing its wall by name in
  `docs/sweep-evidence/2026-09-16-m38/gates.log`; M44: all seven, `0 passed; 7 failed`, each on its
  own wall; M45: `automationmodetool`, `0 passed; 1 failed`, its tail the refusal line with `args`
  on one line). **M39 moved none of them**: its one wall was cleared inside the milestone, so no gate
  was parked, un-parked or re-worded, and M39's sweep changed no row's label. `ls` and `ed`,
  non-clean from M38 to M44, got gates at M44 (spec R6: a wall with no gate is invisible to the gate
  log), and both run: `ls_records_and_replays` and `ed_records_and_replays`. Before M36 the
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

**Do not omit the `--bins` chunk.** `--test <name>` selects integration-test targets only, so the 32
unit tests inside the `retrace` binary itself (20 in `crates/retrace/src/debug.rs`, 12 in
`crates/retrace/src/rsp.rs`) run in none of the other chunks; **only the unchunked `--workspace`
run, or a whole-package `cargo test -p retrace` without a `--test` filter, reaches them.** Leaving it
out silently costs 32 tests and one binary —
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

Some end-to-end gates depend on `/opt/homebrew/bin/jq`, which is not a repo artifact. They skip
rather than fail when it is absent, and print a `SKIPPED` line, because a silent skip would read as a
green it did not earn. The same applies to the Homebrew CPython gates, to `lldb_e2e` (which needs
`/usr/bin/lldb`, and Homebrew Python for its CPython test), and to the gates that record binaries
out of `/bin` and `/usr/bin`: those are OS artifacts, present on any macOS 26 machine, but announced
rather than skipped silently if absent.

**Since M44 every skip line reaches the ordinary gate log.** libtest captures `eprintln!` in a test
that passes, and a skip passes, so an `eprintln!` skip line never reached a gate log (measured at
M43's close). Every skip now writes to the process's stderr past the capture: `crates/retrace`'s
tests through `util::announce` (`crates/retrace/tests/util/mod.rs`), and `retrace-core`'s
`machmsgband_dyn`, which cannot reach that module, through a local `announce` with the same body
(M44's final review found its two partial skips, `[M32 t1 corpus] SKIPPED …`, still on `eprintln!`).
`skiplines.rs` fails the gate if any `.rs` file under any crate's `tests/` directory, searched
recursively so `tests/util/` is included, calls `eprintln!` with `SKIP` anywhere in its first string
literal. Its control test announces a fixed `SKIPLINES CONTROL: …` line, which M44 found in an
ordinary run's log with no `--nocapture`. So to tell a skip from a run, grep the gate logs for
`SKIPP` (`fallthrough_e2e` says `SKIPPING`); `--nocapture` is no longer needed. The detector reads
only that literal: a skip line built from a variable, or written in lower case, slips past it.

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
