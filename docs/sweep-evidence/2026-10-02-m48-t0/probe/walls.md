# M48 scratch probe: node wall list

**Status: COMPLETE, 2026-10-02.** All four walks reach their natural end under record, and every
recording replays clean twice. **No NEW-SUBSYSTEM wall.** Two findings outside the wall list need
their own plan items: a pre-existing hv-sys bug that corrupts SIMD registers (§4), and a TLB flush
that panics inside `step()` (§4).

- Worktree: `m48-probe/wt`, detached at e6caa65. Nothing committed.
- Stub patch: `m48-probe/probe.patch` (`git diff`, plus the untracked
  `crates/retrace-box/src/probe.rs`).
- Logs: `m48-probe/logs/<walk>.{err,out,status}`. `.err` holds `RETRACE_TRACE` and the `[probe]` lines.
- Runner: `m48-probe/run.sh <tag> <profile> -- <node args>`. Stdin is `/dev/null`, stdout is piped
  through `cat` (the shape `Command::output()` gives a test), and it sets
  `RETRACE_TRACE=1 RETRACE_PROBE=1`.
- Census: `m48-probe/census.sh <tag>`.
- Native replicas: `m48-probe/native/`. Crash demo: `m48-probe/crash/` (addon, `crash.js`,
  `crash.json`). Trace dumper: `m48-probe/td` (`TD_ADDR=` lists every write that covers an address).
- Node: `/opt/homebrew/Cellar/node/25.6.1/bin/node`, an arm64 executable (not arm64e).

**Build note.** The walks ran on the **release** build. M47's wall takes 47 s on debug and about 3 s
on release. Walk 1 was also run on debug: record 45 s, replay 46 s, both rc 0.

## 1. Wall list (in order of encounter; walks 2–4 met no wall beyond walk 1's)

| # | landmark | call/fault | args (short) | caller | native behaviour | class | stub used |
|---|---|---|---|---|---|---|---|
| 0 | ~1001 | `getsockname` (32): no row | fd 0 | libuv `uv_guess_handle` on stdin | — | row (harness artefact) | None. It fires only when stdin is a socket, as the Claude Bash tool's stdin is. With stdin `/dev/null` it never fires, and `Command::output()` gives a null stdin. |
| 1 | 1101 | `kevent` (363): no row | kq 7. Two changes: `EVFILT_USER` ident 0x1e7e7711 `EV_ADD\|EV_CLEAR` (0x21), then `NOTE_TRIGGER`. nev 1, zero timeout. | `uv__kqueue_runtime_detection` | Returns **1**. event[0] = {ident, -10, flags **0x21**, fflags 0, data 0, udata 0}. Slot 1 (the second change) is left untouched. (`native/kqdetect.out`) | four (kqueue) | `probe_kevent`: a knote table per guest kq fd. `EVFILT_USER` add, trigger and `EV_CLEAR`. Delivery in activation order. A NULL or relative timeout blocks (`BlockReason::Probe` plus a deadline). The waker writes the events and `x0` into the waiter's saved ctx. |
| 2 | ~1180 | `mach_msg2` msgh_id **3419 `semaphore_destroy`**: complex, MOVE_SEND of the semaphore port | dest task 0x203, sem 0x2803 | `uv_sem_destroy`, after the loop thread's `uv_sem_post` (-33) | KERN_SUCCESS | row (forward allowlist, the mirror of 3418) | `(3419, "semaphore_destroy")` added to `FORWARD_ALLOWLIST` |
| 3 | ~1197 | **data abort**, translation fault, FAR 0xa3d040000 (UNMAPPED) | `mmap(hint, 0x7c000)` → `munmap(head 0x10000)` → `munmap(tail 0x2c000)` → store into the middle | V8 aligned allocation: over-allocate, then trim | The middle 256 KiB stays mapped | **small (not in the spec)** | `guest_munmap` unmapped the whole backing that contains `addr` (`lib.rs` "whole-backing unmap for M2's page-granular guests"). The stub splits the backing: stage-2 unmap, re-map head and tail, free the middle host pages. The end rounds up to 16 KiB, because V8 also munmaps unaligned lengths (0x10b20). 56 partial munmaps per walk. |
| 4 | ~1253 | **EC 0x00** (undef) at pc 0x1804f2ab0 in `pthread_jit_write_protect_np` | `msr S3_6_C15_C1_5, x0`. x0 = 0x2010002030300000 = commpage +0x110 (write-enable). x8 = 0xfffffc10c. | V8 code-space write | Per-thread register | four (JIT) | **The `msr` surfaces as EC 0x00, the same as the `mrs`** (answers §2d). The stub sits in `try_emulate_undef_mrs`. It decodes `0xD51EF1A0\|Rt` and admits only +0x110 or +0x118. It stores the value per thread, and the `mrs` returns it. When the running thread's mode differs from the view, it re-stamps every MAP_JIT range (`ATTR_DATA`/`ATTR_CODE`, skipping PROT_NONE extents) and runs `flush_guest_tlb`. It does the same on `switch_to_thread`, and forcibly after any `mprotect` over a JIT range. |
| 5 | ~1334 | **EC 0x18** ISS 0x12dd2a at pc 0x1804fc67c | `ic ivau, x9` (op0 1, op1 3, CRn 7, CRm 5, op2 1) | `sys_icache_invalidate` | Runs at EL0 | four (JIT, SCTLR) | Set **SCTLR UCI (bit 26) only**. Disassembly of `sys_icache_invalidate` (guest cache 0x1804fc654): `dsb ish`, then `ic ivau` per hardcoded 64-byte line, plus a commpage read at 0xfffffc080 (cpu family) to decide on a `dsb`. **No `DC CVAU` and no `CTR_EL0`.** No walk ever needed UCT. |
| 6 | ~1336 | `setsockopt` (105): no row | fd 1 (the stdout pipe), SOL_SOCKET, SO_OOBINLINE (0x100) | libuv `uv__stream_open` (Apple-only `SO_OOBINLINE`) | ENOTSOCK on a pipe, which libuv ignores | row | `105 => [Fd, Scalar, Scalar, Source, Scalar]` |
| — | | | | | | | **Walk 1 then records to exit 0.** |

These are on the path but are not walls:
- **`uv__stream_try_select`** does `kqueue()` then
  `kevent(kq13, [fd 1 EVFILT_READ EV_ADD|EV_ENABLE], nev 1, {0, 1 ns})` on the stdout pipe's
  write end. **Native returns 0**, both for a pipe's write end and for a piped fd 1
  (`native/kqpipe.out`), and the stub answers 0. libuv needs only "0, no EV_ERROR" to keep kqueue for
  the fd. With stdout a regular file, this probe and `setsockopt` never happen: 10 kevents, no
  kq 13.
- **One XPC message-queue send** (msgh_id 0x400000cf; its body holds "targetpid" and
  "domain-port") is refused by M23's existing `RefuseMqSend`, and node carries on.

Probe-only follow-ons, needed to measure the crash demo's reverse-continue:
- **`flush_guest_tlb` panics inside `step()`.** `reverse-continue` single-steps, a JIT toggle runs
  inside that window, and the TLBI stub runs at EL1 with `MDSCR_EL1.SS` still armed:
  `tlbi stub faulted at EL1: EC=SoftStep (syndrome=0xca000022)`. The stub clears `SS` and `MDE`
  around the stub run and restores them afterwards. **This is a real J1 design item.** §3f says the
  flip lives in `run()`/`step()`, but `flush_guest_tlb` is not step-safe today.
- **Probe state in `BoxState`.** It is carried through `checkpoint`/`from_checkpoint`, as §3c and §3f
  already require.

## 2. How far each walk got

| walk | command | result | landmarks | trace | record (release) | replay |
|---|---|---|---|---|---|---|
| 1 | `-e 'console.log(1)'` | **exit 0, prints `1`** | 1471–1478 (varies run to run) | 526 MB | 4–5 s (debug 45 s) | rc 0, many runs (debug 46 s). It diverged before the SIMD fix (§4). |
| 1f | the same, stdout to a regular file | exit 0, prints `1` | — | — | 4 s | rc 0 |
| 2 | `-e 'setTimeout(() => console.log(2), 10)'` | **exit 0, prints `2`** | 1474 | 526 MB | 4 s | rc 0 ×2 |
| 2b | the same with **2000** ms | exit 0, prints `2` | 1483 | 530 MB | 4 s | rc 0 ×2 |
| 3 | `--allow-natives-syntax`, BigUint64Array, `%OptimizeFunctionOnNextCall` | **exit 0, prints `3`** | 1474 | 530 MB | 4 s | rc 0 ×2 |
| 4 | crash demo (§3g shape): `--allow-natives-syntax crash/crash.js <addon> <crash.json>` | **rc 139**. Marker printed, `UNREACHED` not printed. | 1395–1402 | **1.2 GB** | 9–10 s | rc 139 ×2, the same crash |

**The crash demo meets every assertion of §1 part 3 on the scratch build** (`logs/w4.dbg`):
- **The addon.** It builds with
  `cc -bundle -undefined dynamic_lookup -I/opt/homebrew/include/node` and loads through
  `require(argv path)`.
- **The marker.** `CRASHJS cell=0x700c158d0 target=0x4000dead0000 rows=2 opt=101001`. `opt` is
  `%GetOptimizationStatus`: 0b101001 = kIsFunction | kOptimized | kTurboFanned. Native prints the same
  line and exits 139.
- **The crash.** The terminal `Event::Crash` has pc 0xa3da14738 (the addon's `deref`), FAR
  0x4000dead0000, ESR 0x92000005: DFSC 0x05, a level-1 **translation** fault, on thread 0, the
  marker's thread. Before it, node's own SIGSEGV handler runs: `SignalDelivery` sig 11, then the
  re-raise.
- **The reverse-continue.** `watch 0x700c158d0 8; reverse-continue` lands at `(1369, 1407045)`, pc
  **0xa2cf80948**, thread 0. The cell holds `2` (the warm-up value), and one `stepi` later it holds
  `0x4000dead0000`. **The pc lies inside the MAP_JIT range** [0xa2cf68000, +256 MiB).
- **The workers.** TurboFan compiled synchronously on main: walk 4's SPRR writes are all on tid 0,
  and the workers never ran past the handshake.
- **Cost.** The debug session took 13.6 s and reached **4.2 GB peak RSS**. Checkpoints copy every
  backing, and retrace backs V8's PROT_NONE reservations in full (§3).

## 3. Census

**Threads (M6).**
- **Count.** 7: main plus 6 `bsdthread_create`s. All 6 are created by main between landmarks ~1110
  and ~1245. After the first, main blocks in `semaphore_wait_trap` (-36), and the new thread wakes it
  with -33.
- **tid 1** is a libuv loop thread with its own kq 8 (`EVFILT_USER` ident 9). It blocks in
  `kevent(kq8, nev 1024, NULL timeout)` until main's `NOTE_TRIGGER` at shutdown, and exits 0. This
  is the delayed-task scheduler of §2b.
- **tids 2–5** are V8 platform workers. Each `cvsignal`s main's start-up cv (main `cvwait`s for each,
  4 rounds), then blocks in `cvwait` on the task-queue cv. They exit 0 at shutdown, after a
  `cvbroad`.
- **tid 6** is created late (func 0xa080f8eac, arg 0). It is Runnable at exit in walks 1–4, never
  scheduled because main never blocks after creating it. In walk 2b it ran and parked in
  `semaphore_wait_trap` (`Blocked(Sem)`): the inspector thread of §2b, on a mach semaphore that M18
  already models.
- **Scheduling.** At most 6 runnable at once. No deadlock panic.

**kevent (M3).**
- **Volume.** 10–14 calls per walk on 5 kqueues: 4, 7 (the detection throwaway, closed), 8 (tid 1),
  10 (**the main loop**: every main poll is on kq 10) and 13 (the `try_select` throwaway, closed).
- **Filters.** Only `EVFILT_USER`, plus the one `EVFILT_READ` on fd 1 in `try_select`.
- **Flags and fflags.** Flags 0x21 (`EV_ADD|EV_CLEAR`) and 0x5 (`EV_ADD|EV_ENABLE`); fflags
  `NOTE_TRIGGER`. No `EV_DELETE`, `EV_ONESHOT`, `EV_RECEIPT` or `EV_DISABLE`.
- **Sizes and timeouts.** nevents 0, 1 or 1024. Timeouts: zero, NULL, {0, 1 ns}, and in walk 2b
  **{1, 986000000}** on kq 10.
- **Outcomes.** Every multi-event return held one event. 1–2 calls blocked per walk. 1 was woken by
  a cross-thread `NOTE_TRIGGER` (main → kq 8, at shutdown). The 1 ns one timed out, and in walk 2b
  the 1.986 s one timed out through an **idle jump**.
- **No fd filter on a guest pipe is ever registered** on a loop kq in any walk, so R4's
  byte-accounting is not exercised by node.

**psynch (M4).**
- **Calls.** 26–29 per walk: `cvwait`, `cvsignal`, `cvbroad` (once, at shutdown).
- **Never seen:** `mutexwait`, `mutexdrop`, `cvclrprepost`, any rwlock call, a targeted signal (the
  thread port is always 0), or a prepost (no `cvsignal` ever found no waiter).
- **Shape.** Every `cvwait` has flags 0xa0 and **mutex 0**, so no in-kernel mutex drop.
- **Timed waits.** Every timed `cvwait` (6 per walk) has **`sec 0, nsec 1`**. That is libpthread's
  "deadline already elapsed" form. No `gettimeofday` precedes it, so it is plausibly a relative wait
  whose delta the synthetic clock had already used up. This is unmeasured; owed to t0 M4. The stub
  answers ETIMEDOUT (errno 60, carry set).
- **Return words used.** A woken waiter gets 0. A signaller gets 0x100 per woken waiter.

**JIT (M5).**
- **The mapping.** Exactly **one `MAP_JIT` mmap** per walk:
  `mmap(hint 0xa08470000, 256 MiB, PROT_NONE, 0x41842 = MAP_JIT|MAP_ANON|MAP_NORESERVE|MAP_PRIVATE|MAP_UNIX03, fd 0xff000000 = VM tag 255)`.
  It is not FIXED.
- **What follows.** One `mprotect(+0x40000 into it, 0xffc0000, RWX)`; the first 256 KiB stays
  PROT_NONE. Then `madvise` REUSE and REUSABLE over the code range, and **one whole-range `munmap`
  at exit**. No other `mprotect` or `munmap` touches it. No non-`MAP_JIT` RWX or exec mmap. No write
  to a protected JIT page (none faulted).
- **Toggles.** **268 SPRR writes** in walks 1–2 (main 16, tid 2 252), 278 in walk 3, 20 in walk 4
  (all main). View flips = writes + 2 (the mmap and `mprotect` re-stamps). **0 flips caused by a
  thread switch**: every thread is back in RX before it blocks.
- **Cost.** Each flip re-stamps 256 MiB of L3 entries and runs one guest TLBI. The whole record is
  4–5 s release and 45 s debug, so **R9 does not fire**.
- **Memory.** retrace backs PROT_NONE mmaps in full (`alloc_pages` plus `protect_none`). V8's
  reservations (256 MiB JIT, 256 MiB, 128 MiB, 0x80000) are therefore fully backed, are in every
  snapshot and checkpoint, and drive the 526 MB / 1.2 GB traces and the 4.2 GB debug RSS.

**Timing.**
- Walk 2's 10 ms timer **never blocks**. `SYNTH_TSC_STRIDE` (0x2400 ticks, about 384 µs per
  timebase read) carries the synthetic clock past 10 ms before the loop's first poll, so every kq 10
  poll has a zero timeout and **there is no idle jump**.
- Only a long timer exercises the deadline path. Walk 2b (2 s) blocks in
  `kevent(kq10, timeout 1.986 s)`; nothing is runnable, so the clock idle-jumps to the deadline.

## 4. NEW-SUBSYSTEM walls

**None.** Every wall was a row, a small model (partial munmap), or part of the four subsystems.

Two non-wall findings are **plan-blocking**:

1. **hv-sys `set_simd` passes its value in the wrong register.** This bug predates M48, and its
   class is H4.
   - **Symptom.** Walk 1 recorded clean, but plain `replay` diverged at the final memory compare:
     `memory divergence at ipa 0x27f6e00`, a main-stack word. **It diverged only for some host
     environments.** The same trace replayed clean with any extra env var set, or with `PWD` unset,
     and diverged under the exact login env, 3/3 each way. A second recording flipped the direction.
   - **Localisation.** Hashing the registers per landmark under the two envs puts the first
     difference in thread 1's **SIMD registers** at its first landmark (L1112), right after
     `switch_to_thread` loaded its zeroed `ThreadCtx`. In the failing env every q register read
     `0x00000000600000000000000030047000`, which is host garbage. The GPRs and every backing were
     identical.
   - **Cause.** bindgen maps `hv_simd_fp_uchar16_t`, a 16-byte `ext_vector_type`, to `u128`. AAPCS64
     passes that argument in **v0**, but Rust passes a `u128` in a GPR pair. So
     `hv_vcpu_set_simd_fp_reg` (`crates/hv-sys/src/lib.rs:134`) installs whatever the host had in
     v0.
   - **Reach.** Every `set_simd` site: `load_ctx` (every thread switch), `from_checkpoint`, the
     signal-frame restore (`lib.rs:4593`), and `vcpu_set_q`. `get_simd` takes a pointer and is
     correct.
   - **Why the existing test passes.** `crates/hv-sys/tests/simd.rs` sets a constant, and the
     compiler happens to have that constant in v0 as well.
   - **Fix.** The scratch fix calls the function through inline asm with the value in v0
     (`fmov d0, lo; mov v0.d[1], hi; blr`, with `clobber_abi("C")`; `#![feature(simd_ffi)]` is not
     available on stable). With it, a fresh recording replays clean under every env tried: plain,
     `FOO=1`, `-u PWD`, and a long variable.
   - **Why node exposes it.** node is the first gated guest whose threads keep live SIMD state across
     a switch. **It needs its own task, with a test that loads a non-constant value, before any node
     gate can be trusted.**
2. **The JIT view flip inside `step()`.** This is §1's `flush_guest_tlb` SoftStep panic. The J1 arm
   must disarm `SS` and `MDE` around the TLBI stub, or defer the flip. Either way it needs a guard of
   its own: reverse-continue into JIT code crosses toggles.

## 5. Where the walk contradicts or sharpens the spec's §2

- **§1 part 2 / §4 (the node timer).** The claim "the 10 ms elapse … reached by the idle jump" is
  **false for a 10 ms timer**: no kevent blocks, and there is no idle jump (walk 2 census). The timer
  test needs a longer delay (2 s demonstrated) if it is to assert the jump. The same caution applies
  to `kq_e2e timeout` (5 ms): a fixture that reads the clock between arming and waiting may see the
  stride pass the deadline first.
- **§2b.** Confirmed: 4 platform workers, a scheduler thread with its own loop and kqueue woken by
  `NOTE_TRIGGER`, and a late thread parked on a uv_sem, which is a **mach semaphore** (-36, M18's
  `Sem`), not psynch. Workers wait on condition variables. `posix_spawn` was never reached.
- **§2d.** The `msr` traps as **EC 0x00**, like the `mrs`, so J1's arm belongs beside
  `try_emulate_undef_mrs`.
- **§2e.** V8 does not mmap RWX. It mmaps **PROT_NONE with `MAP_JIT`**, then `mprotect`s a sub-range
  (offset 0x40000) to RWX. §3f's `MAP_JIT` set must therefore survive `protect_none`/`unprotect`, and
  must re-stamp after an `mprotect` that `unprotect` turned back into `ATTR_DATA`. The partial
  overlap is an `mprotect` *inside* the range, which §3f's "partial overlap refused by value" would
  refuse. It needs an admitted shape: an `mprotect` of a sub-range to 7.
- **§2f.** Only **UCI** is needed. `sys_icache_invalidate` issues `IC IVAU` only; no `DC CVAU`, no
  `CTR_EL0` read (disassembly in §1, row 5). UCT was never needed. R8's "UCI and UCT" is safe but
  measured as half-needed.
- **§2g.** psynch is only `cvwait`, `cvsignal` and `cvbroad`. Mutex 0 always, flags 0xa0, no
  `mutexwait`/`mutexdrop`/`cvclrprepost`, no targeted signal, no prepost. The mutex pair stays out of
  scope (§3e's condition).
- **§2h / §3c.**
  - Kqueue 7 and kqueue 13 are closed (`close_nocancel` 399) after one use, so the close-drops-table
    lifecycle is exercised.
  - No kq fd is ever dup'd. Each long-lived kqueue (4, 8, 10) gets a forwarded
    `fcntl(kq, F_SETFD, FD_CLOEXEC)` right after it is created. That is harmless, because `kqueue`
    itself stays forwarded.
  - The main loop is kq 10, not kq 4 as §2a implies (kq 4 gets one `EVFILT_USER` and one zero poll).
- **§3c R4.** Pipe readiness is never needed by node: no fd filter on a loop kq, and `try_select`'s
  1 ns probe answers 0 natively.
- **§3e "timed cvwait".** Every timed wait reached is the 1 ns already-elapsed form. Whether this is
  synthetic-clock drift or a realtime-vs-commpage skew is owed to t0 M4.
- **§3f "switch flips".** None occurred. The `twothreads` fixture is the only guard of that path.
- **§3g D1.** Confirmed end to end on the scratch build (§2 above): the addon, synchronous TurboFan,
  the store in `MAP_JIT`, the translation fault at the target. One addition: node's own SIGSEGV
  handler runs before the terminal Crash (`SignalDelivery` sig 11, then the re-raise). The test's
  "terminal `Event::Crash`" assertion holds, but the event sequence has a `SignalDelivery` before it.
- **Not in the spec.**
  - **Partial `munmap`** (wall 3) is a small model the plan must absorb (Q3). V8's aligned-reservation
    trim needs backing splits, and so may any other guest that trims.
  - **`semaphore_destroy` (3419)** and **`setsockopt` (105)** are rows.
  - **Trace size and debugger memory** follow from fully backed PROT_NONE reservations: 526 MB for
    `console.log(1)`, 1.2 GB for the crash, 4.2 GB RSS for one reverse-continue. Worth a line in
    Known limits, and possibly an R9-style ruling.
