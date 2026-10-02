# M48-node: real `node`, JIT on, recorded, replayed, and reverse-debugged into its JIT code

**Date:** 2026-10-02. **Branch:** `worktree-m48-node`, to be cut from `main` at this spec's plan
commit, whose ancestor is the M47 merge (M47-gitwrite, branch head `95bf845`, re-gated on 2026-10-02
before its merge).

**Companions:**
- `2026-10-02-retrace-m48-node-measurements.md`, written by t0 (§3a);
- `docs/sweep-evidence/2026-10-02-m48-static/`, committed with this spec. It is the brainstorming
  evidence: a native probe and four disassembly or import listings. Its README lists every file;
- `docs/sweep-evidence/2026-09-30-m47/` (M47's), whose `node.*` files are node's measured wall.

**Sources.** Every claim below says where it comes from:
- **static**: the committed evidence directory above, with a file name;
- **M47**: M47's evidence directory, status-log section or `node_e2e` reason, with a file name;
- **code**: read at `95bf845`, with a path;
- **native**: a native run on the probe host, Apple M4 Pro, macOS 26.5.2;
- **inferred**.

A claim that is inferred is still owed a measurement wherever t0 can take one, and says so.

**Scope and approach.** All chosen by the operator in brainstorming on 2026-10-02.
- **Scope (Q1): node with the JIT on, all in one milestone.** Default flags, not `--jitless`. The
  rejected options were a `--jitless` milestone with JIT as M49, the `kevent` wall alone, and a
  measurement-only milestone.
- **Gate bar (Q2): the full rung-8 shape.** A repo-owned crash script, recorded, replayed, and
  reverse-continued from the crash to the corrupting store — here, a store executed from JIT code.
- **Overflow (Q3): absorb small, halt on a new subsystem.** t0 walks node to its exit with scratch
  stubs before scope freezes. A missing row or a small model it finds is folded into M48 as a
  ledgered ruling. A wall needing a new subsystem beyond the four this spec designs halts (H5).
- **Approaches (Q4): K1, P1, J1, D1**, each the recommended option:
  - **K1**, guest kqueues as box state, never forwarded. K2 (forward to the host kqueue with the
    idents translated and the timeout zeroed) was rejected: a woken thread would have to re-poll
    through a landmark the guest never issued.
  - **P1**, a kernel-faithful port of the psynch condition-variable paths, keyed by guest address,
    with one deadline queue for every timed wait. P2 (a "wake any waiter" shortcut) was rejected:
    libpthread's sequence words would drift into hangs that record and replay reproduce
    identically, which the oracle cannot see.
  - **J1**, `S3_6_C15_C1_5` emulated per thread, with `MAP_JIT` pages' stage-1 permissions following
    the running thread's mode. J2 (flip a page on whichever permission fault it takes) was rejected:
    it silently repairs a write that natively faults. J3 (zero the commpage byte so V8 believes
    write protection is unsupported) was rejected: V8's fallback is unmeasured and plausibly maps
    pages RWX, the one thing that hangs the vCPU, and it shows the guest a value native never does
    (the M8 lesson).
  - **D1**, a repo-owned N-API addon gives `crash.js` a cell's address and the faulting deref; the
    store of the bad pointer is made by a function forced through TurboFan. D2 (pure JS ending in
    `process.abort()`) and D3 (the addon stores and derefs in C, never touching JIT code) were
    rejected: neither reverse-continues into JIT code, the capability node adds over CPython.
- **Autonomy.** The operator then said to write this spec and the plan and run subagent-driven
  development without further check-ins. The halt list in §7 is the operator's.

## 1. Purpose

The 2026-07-05 vision spec's v1 bar is recording and reverse-debugging `python3`, `node` and `git`
on real workloads (`2026-07-05-retrace-macos-record-replay-design.md:261-270`). `python3` has been
met since M26 (rung 7) and M39 (rung 8), and `git` since M47. `node` is the last. M47 took it past
its first wall, AMFI's dyld policy, and parked it at the second: `kevent` on a guest `kqueue()`
(M47 `node.entry.txt`).

M48 makes `node` the third, and so closes the v1 bar. On the way it gives every guest four
capabilities none has had: kqueues of its own, condition variables, timed waits that a deadline
ends, and JIT code.

**Success** has six parts:

1. **Rung 9: node prints 1.** `/opt/homebrew/bin/node -e 'console.log(1)'` (the Cellar binary,
   v25.6.1) records to exit 0 and replays byte-identically twice. The test asserts JIT ran: the
   recording holds at least one `MAP_JIT` mapping, and the guest wrote `S3_6_C15_C1_5` at least
   once. A run that never reached V8's code space would otherwise pass identically.
2. **Node's timer.** `node -e 'setTimeout(() => console.log(2), 10)'` records and replays the same
   way. The 10 ms elapse on the synthetic clock: the deadline is reached by the idle jump, never by
   the host's time.
3. **Rung 10: node crashes, and the crash is reverse-debugged into JIT code.** `node_crash_e2e`
   asserts:
   - the marker line is printed and `UNREACHED` is not;
   - the terminal `Event::Crash` faults at the target computed from `crash.json`, with a
     translation DFSC, on the marker's thread;
   - two replays are byte-identical;
   - `watch <cell>; reverse-continue` from the crash lands on the store, shown by its effect;
   - the store's pc lies inside a `MAP_JIT` mapping of the recording.
4. **The three mechanisms are guarded on any machine.** Repo-owned C fixtures exercise guest
   kqueues, condition variables and JIT write-protect without node (§3h). Each records, replays
   and seeks.
5. **Nothing new reaches the host kernel unmodelled.** `kevent` (363), the psynch calls the walk
   reaches, and `MAP_JIT` mappings are modelled. Every shape the model has not measured stops the
   recorder by value, naming the field.
6. **No trace-format change.** All new state is box state, rebuilt from the guest's own syscalls on
   both sides. `TRACE_MAGIC` stays `RT\x00\x0b`. A measured need to bump it is halt H3.

## 2. What is known before t0

### 2a. node's wall (M47: `node.entry.txt`, `node.frame.txt`, `node.landmarks.txt`)

Since M47, node loads all its `@rpath` dylibs and records 1,100 landmarks in 47 s, a 355 MB trace.
No thread has been created by then, and none of its 121 `mmap`s is `MAP_JIT`. At landmark 1,101
libuv's `uv__kqueue_runtime_detection` calls `kevent(7, changes, 2, events, 1, &zero)` on a
throwaway kqueue (fd 7; the loop's own kqueue is fd 4, from landmark 1,094):
- change 1: ident `0x1e7e7711`, `EVFILT_USER` (-10), `EV_ADD|EV_CLEAR`;
- change 2: the same ident and filter, fflags `NOTE_TRIGGER`;
- the event list is at the same address as the change list, with one entry and a zero timeout.

`kevent` has no row, so the recorder stops (`M33: syscall 363 (363) has no arg_kinds row`).

### 2b. What node will ask for next (static: `node-imports.txt`)

- **libnode** imports `pthread_jit_write_protect_np` and `sys_icache_invalidate`, which is V8's JIT.
  It also imports `pthread_create`, `pthread_cond_wait`, `pthread_cond_signal`,
  `pthread_cond_timedwait` and `pthread_cond_timedwait_relative_np`, plus `dispatch_semaphore_*`
  and `uv_sem_*`.
- **libuv** imports `kqueue`, `kevent`, `pipe`, `socketpair`, the `posix_spawn` family,
  `pthread_cond_*` (including `_timedwait_relative_np`) and mach `semaphore_*`.
- **Inferred.** Node starts V8's platform workers (4 by default), a delayed-task scheduler thread
  with its own libuv loop and kqueue, and an inspector signal thread blocked on a `uv_sem`. Workers
  wait on condition variables. Other threads wake the scheduler's loop with
  `uv_async_send` → `EVFILT_USER` `NOTE_TRIGGER`. t0 M6 measures all of it.
- `posix_spawn` is unreachable for `-e` and `crash.js` (inferred). If the walk reaches it, M38's
  refusal applies, and §7 lists real process creation as out.

### 2c. The JIT write-protect mechanism (static: `pthread-jit-disasm.txt`; native: `sprr.out`)

`pthread_jit_write_protect_np(enabled)` reads the commpage byte at `0xfffffc10c`:
- **0**: it returns at once;
- **1**: it uses APRR (`S3_4_C15_C2_7`);
- **2 or 3**: it uses SPRR (`S3_6_C15_C1_5`).

It then:
1. loads the commpage value for the requested mode — `+0x118` to protect (`enabled != 0`), `+0x110`
   to write-enable;
2. `msr`s it to the register, then `isb`;
3. `mrs`es the register back and compares it with the value loaded;
4. `brk #1` on a mismatch.

Native, on this host:
- **The commpage.** `+0x10c` = 3. `+0x110` = `0x2010002030300000` (write-enabled) and `+0x118` =
  `0x2010002030100000` (protected). The two differ only in bit 21.
- **Each thread's start.** The main thread starts at the protected value. A thread created while its
  parent is write-enabled also starts protected. The register is per thread.
- **A `MAP_JIT` RWX page.** Code written after `(0)` and executed after `(1)` plus
  `sys_icache_invalidate` returns 42.

### 2d. retrace and `S3_6_C15_C1_5` today (code: `crates/retrace-box/src/lib.rs`; static: `dyld-sprr-disasm.txt`)

- **Reads.** HVF does not expose the register, so an EL0 `mrs` surfaces as an undefined
  instruction (EC 0x00). `try_emulate_undef_mrs` (`lib.rs:1401-1429`) answers it with **0**: libdyld
  probes it, tests bit 36, and with the bit clear takes its normal path. Bit 36 is also clear in
  both native values, so 0 and native agree on that branch.
- **Writes.** No `msr` to the register is emulated, so a guest write ends as `Stop::Other`. No gate
  guest has ever written it (inferred from the absence of a handler and of any such failure).
  dyld's own writes, of commpage `+0xd0`/`+0xd8` in `MemoryManager`'s writable-memory toggle, sit
  inside a protected-stack frame that retrace's dyld never enters.
- **So the first `pthread_jit_write_protect_np` under retrace dies twice over:** once at the `msr`,
  and, if that were skipped, again at the read-back `brk`. Whether the `msr` traps as EC 0x00 or as
  EC 0x18 is unmeasured. t0 M1 measures it.

### 2e. `MAP_JIT` and exec pages under retrace today (code: `lib.rs:2605-2690`, `:1320-1365`, `:445-454`)

- `map_mmap_region` reads only `MAP_FIXED` and `PROT_EXEC` from the flags and prot. `MAP_JIT`
  (`0x800`) is ignored.
- A `PROT_EXEC` region gets `ATTR_CODE` (RO, EL0-exec) through `set_region_exec`. Data is
  `ATTR_DATA` (RW, never exec). No page is ever writable and executable at once: that is the W^X
  invariant.
- **Inferred.** V8 maps its code space `PROT_READ|PROT_WRITE|PROT_EXEC` with `MAP_JIT`, write-enables,
  and stores into it. Under retrace today that store takes a stage-1 permission fault, recorded as
  a guest crash. t0 M5 measures it.
- `guest_mprotect` (`lib.rs:2772`) changes only stage 2 (and `PROT_NONE`). It never re-stamps
  stage 1. A V8 `mprotect` of a code page would therefore leave the stage-1 view where it was
  (inferred; t0 M5 counts any).

### 2f. Cache maintenance at EL0 (code: `lib.rs:202-208`)

`SCTLR_EL1` sets DZE and leaves UCT (15) and UCI (26) clear, "pending measurement: nothing has
measured a guest issuing `DC CVAU` / `IC IVAU` or reading `CTR_EL0` from EL0". `sys_icache_invalidate`
does exactly that (inferred from its purpose; t0 M1 disassembles it). It will trap EC 0x18, and
today that fails loud. This is a predicted small wall: two SCTLR bits, the DZE precedent.

### 2g. psynch (code: `crates/retrace-arch/src/lib.rs`; SDK `sys/syscall.h`)

The SDK numbers are:

| Number | Call |
|---|---|
| 301 | `psynch_mutexwait` |
| 302 | `psynch_mutexdrop` |
| 303 | `psynch_cvbroad` |
| 304 | `psynch_cvsignal` |
| 305 | `psynch_cvwait` |
| 306–309 | the rwlock calls |
| 312 | `psynch_cvclrprepost` |

None has a row, and nothing in the box models any of them. The only mention of psynch is a doc
comment (`retrace-arch/src/lib.rs:1291`) noting that `__pthread_join` does not use it.

### 2h. Kqueues today (code: `retrace-arch/src/lib.rs:858-862`, `retrace-box/src/kq.rs`)

- `kqueue` (362) is forwarded, and its descriptor is bound in the `FdTable` "like open's". Nothing
  consumes the bound slot.
- `kevent` (363) and `kevent64` (369) have no row.
- M46's `kq::WorkqKqueue` models only the process's workqueue kqueue as libdispatch uses it: its
  `EVFILT_USER` knote (ident 1), the memory-pressure knote, UPTIME timers and the event manager. It
  is carried in `BoxState`.

### 2i. The scheduler and the clock (code: `retrace-box/src/thread.rs`, `lib.rs:6325-6371`)

- `BlockReason` has `Join`, `Wait { addr }` (ulock), `Sem { port }` and `Parked`. None of them
  carries a deadline.
- `schedule_after_block` fires overdue workqueue timers. With nothing runnable, it makes **one**
  jump of `synthetic_tsc` to the earliest timer deadline (`kq.earliest_deadline()`); otherwise it
  panics as a deadlock.
- "A timer fires only when some thread blocks" is a documented limit.
- The guest's clock is `now_guest()` = `synthetic_tsc` plus the commpage offset (`lib.rs:5718`).

### 2j. The rung-8 template (code: `crates/retrace/tests/cpython_crash_e2e.rs`, `crates/retrace-guest/py/crash.py`)

- **The script.** It computes a target from `crash.json` (`base + offset`, giving
  `0x4000_DEAD_0000`, bit 46 set, never mapped). It stores the target into a ctypes cell, prints
  `CRASHPY cell=0x… target=0x… rows=N`, and derefs.
- **The test.** It discovers the cell from the recording, and asserts the four properties §1 part 3
  restates. The reverse-continue's proof is by effect: before the store the cell is not the
  target, and one `stepi` later it is.

## 3. Design

### 3a. t0: measurements first

t0 runs on the branch before any product code and writes the companion file. Scratch code (stubs,
hacks, counters) lives in a scratch worktree or a patch in the ledger, never on the branch.

- **M1, the register and the caches.**
  - **(a)** A freestanding fixture (`asm/sprrprobe.s`, scratch until Task 4 makes it real) runs
    four instructions: `mrs x0, S3_6_C15_C1_5`; `msr S3_6_C15_C1_5, x1`; `mrs x2, S3_6_C15_C1_5`;
    `ic ivau, x3`. Record the exception class, ISS and ELR each produces in an HVF guest, at EL0
    and with today's SCTLR. This decides whether J1's arm sits beside `try_emulate_undef_mrs`
    (EC 0x00) or on the EC 0x18 sysreg path, and whether UCI/UCT are what `IC IVAU`, `DC CVAU` and
    `CTR_EL0` need.
  - **(b)** Disassemble `sys_icache_invalidate` from the guest's cache, and list its instructions.
  - **(c)** Re-run `sprr.c` natively and commit its output beside this spec's evidence, with one
    addition: `pthread_jit_write_protect_np` called twice in a row with the same argument, to show
    the register value is idempotent.
- **M2, the walk.** Walk node to exit under a scratch build that stubs each wall just enough to
  continue, recording each stop's landmark, syscall, arguments, caller symbol and the stub that
  passed it:
  - `-e 'console.log(1)'`;
  - the `setTimeout` row;
  - `crash.js` (§3g).

  The output is the **wall list**. Each entry is classed **row**, **small model** (absorbed, Ruling)
  or **new subsystem** (H5). Every absorbed entry gets a plan task or a step in an existing one.
- **M3, the kevent census.** For every `kevent` in M2's walks, record:
  - the kq fd and the thread;
  - every change (ident, filter, flags, fflags, data), and nevents;
  - the timeout (NULL, zero or relative);
  - what native returns for the same shape, measured with a C replica of each distinct shape.

  Two results matter most: whether any fd filter appears, on which fd kind, and whether it is ever
  ready; and the ordering of a multi-event return. Measure `uv__kqueue_runtime_detection`'s exact
  call natively in the replica: its return count, the returned entry, and what is left in the
  change slots.
- **M4, the psynch census.** For every psynch call in M2's walks, record its number, arguments,
  flags and thread, and what the caller does with the return. Use the guest cache's disassembly of
  `_pthread_cond_wait`, `_pthread_cond_signal`, `_pthread_cond_broadcast` and the mutex slow
  paths. Pin the kernel semantics to the open-source `libpthread` matching macOS 26
  (`kern/kern_synch.c`, `kern/synch_internal.h`), with the version named. Measure natively, with a
  C probe, the return words of the shapes the walk uses: a wait satisfied by a prepost, a wait then
  a signal, a broadcast to three, and a timed wait that expires. Record whether `psynch_mutexwait`
  ever occurs, and on which mutex.
- **M5, the JIT census.** For `console.log(1)` and `crash.js`, record:
  - every `MAP_JIT` mmap (addr, len, prot, flags, FIXED or not);
  - every `mprotect` or `munmap` over one;
  - toggles per thread, and the write values seen;
  - any write to a protected `MAP_JIT` page;
  - the wall-clock cost of the toggles under a scratch implementation of §3f.

  If toggles × flush make `console.log(1)`'s record take more than 10 minutes, write Ruling R9
  (§8) before the plan freezes the design.
- **M6, the thread census.** Record thread creates, each thread's block reasons over time, the
  maximum runnable count, and any scheduler deadlock panic, for each walk.
- **M7, the crash demo.** Measure:
  - that the addon builds against Homebrew's `node_api.h` and loads through `require`, on the FIXED
    exec dylib path;
  - that `%OptimizeFunctionOnNextCall` produces synchronous TurboFan code without the worker
    threads having run;
  - where the store's pc lies (a `MAP_JIT` range or `libnode`'s text);
  - that the deref faults at the target.

  If the store does not land in a `MAP_JIT` range, write a Ruling before Task 7. `--no-concurrent-recompilation`
  is the first lever, and `%PrepareFunctionForOptimization` the second.
- **M8, the baseline.** Re-derive the M47 close's counts, 950 / 0 / 10 over 160, at the branch base
  from source, by the file-by-file method, so that §9 starts from a measured floor.

### 3b. The rows (`retrace-arch`)

- **`kevent` (363) gets a row**, from its SDK prototype, marked **emulated, never forwarded**:
  `[Fd, Ptr, Scalar, Ptr, Scalar, Ptr]`. The row exists for decoding, for the census test and for
  the debugger. The emulating arm sits before the generic forward arm. The generic arm's assert
  list (`is_signal_syscall`, the workq pair, `kevent_qos`, `writes_via_nested_pointer`) gains 363
  and every psynch number the box models. This is CLAUDE.md's "nothing asserts against it" lesson
  for `bsdthread_create`, applied before the fact.
- **The psynch calls the walk reaches get rows** marked the same way. The rest stay rowless, so they
  stop the recorder through M33's loud path.
- **`kevent64` (369) stays rowless.** If the walk reaches it, it is a Ruling.
- Each number joins `tests/census.rs` with a sourced doc line.

### 3c. Guest kqueues (K1, `retrace-box/src/gkq.rs`)

A new pure-data module beside `kq.rs`, with no `Box_` access. `Box_::gkq` owns a `GuestKqueues`,
carried through every rebuild path in `BoxState`, and through the three `Box_ { … }` construction
sites (`lib.rs:1518`, `:2120`, `:3400`).

- **Lifecycle.**
  - A forwarded `kqueue()` (362) that returns a guest fd creates an empty table for that fd. This
    happens in both arms, from the recorded return on replay.
  - `close` of that guest fd drops the table, after the existing `FdTable` close.
  - A `dup`/`dup2`/`F_DUPFD` of a kqueue fd is refused by value. The kernel's kqueue is per open
    file, and the model has not measured sharing.
- **Knotes.** Each is keyed by `(ident, filter)` and holds flags, fflags, data, udata, an enabled
  bit and an active bit, plus an activation sequence number, so that delivery order is the kernel's
  activation order (pinned by t0 M3).
- **Changes**, applied in list order, faithful to `kern_event.c`:
  - **`EVFILT_USER`**: `EV_ADD` (with `EV_CLEAR` and `EV_ONESHOT`), `EV_ENABLE`, `EV_DISABLE`,
    `EV_DELETE`, and `NOTE_TRIGGER` with the `NOTE_FF*` fflags operations (`filt_usertouch`).
  - **`EVFILT_READ` / `EVFILT_WRITE` on a guest pipe end**, readiness by byte accounting (below).
  - **Every other filter, flag, fd kind or `EV_RECEIPT`/`EV_ERROR` shape** is refused by value,
    naming it, unless t0 M3 measured it and a Ruling admits it.
- **Pipe readiness is a pure function of the trace (Ruling R4).** M38's pipe pair already records
  both guest descriptors (`Ret::FdPair`). The box keeps an unread-byte count per guest pipe. A
  `read`/`write` family call on a pipe end adjusts it by that call's return — forwarded on record,
  the recorded return on replay. A hook after the generic arm runs identically on both sides.
  - `EVFILT_READ` on the read end is ready when the count is > 0, with `data` = the count.
  - `EVFILT_WRITE` on the write end is ready while the count is below the capacity t0 M3 measures.
  - A pipe end whose peer is not a guest descriptor (an inherited stdin or stdout) has no count.
    A readiness question about it is refused by value.
- **The call.** `kevent(kq, changes, nchanges, events, nevents, timeout)`:
  1. Apply the changes.
  2. Collect the active, enabled knotes in activation order, up to `nevents`, and write them to
     `events`. `EV_CLEAR` resets on delivery, and `EV_ONESHOT` deletes.
  3. If any were collected, return their count.
  4. Otherwise, a zero timeout returns 0.
  5. Otherwise, a NULL timeout blocks with no deadline, and a relative timeout blocks until
     `now_guest()` plus the timeout, converted on the synthetic timebase (M46's
     `kq::tsc_for_deadline`).

  An unmapped pointer argument answers `EFAULT` like the kernel, never a box panic.
- **Blocking.** `BlockReason::Kevent { kq, deadline: Option<u64> }`. A blocked thread is woken by:
  - a change on the same kqueue, from any thread, that activates an enabled knote
    (`NOTE_TRIGGER`);
  - a pipe write that makes a watched read end readable;
  - its deadline (§3d).

  The woken thread's delivery — the event writes, `x0` = the count, the carry flag clear — is
  applied to its saved context and to guest memory when it is switched in, by the same
  `settle_schedule` path M46's event manager uses. A deadline wake delivers 0.
- **Record and replay** both call `Box_::guest_kevent(args)` with identical arguments. The arm
  appends `Event::Syscall { writes: vec![] }` with the immediate return, or 0 when the thread
  blocks (the ulock arm's shape). Replay compares the recomputed return, then calls
  `verify_thread` (§3j). The event-list bytes are not recorded. They are a pure function of box
  state, and the exit-time full-memory comparison covers them (Ruling R3, the `bsdthread_create`
  posture).

### 3d. One deadline queue (`retrace-box`, extends `schedule_after_block`)

- A blocked thread may carry a deadline: a `kevent` timeout, or a timed `psynch_cvwait`.
- `schedule_after_block`:
  1. fires overdue workqueue timers, as M46 does;
  2. wakes every blocked thread whose deadline is reached, in deadline order with ties broken by
     thread index, each with its timeout answer;
  3. picks a thread.

  With nothing runnable, the one idle jump goes to the **earliest of all deadlines**, workqueue
  timers and thread deadlines alike, then rules 1–2 run again and the pick is retried once.
  Otherwise it is a deadlock, and the panic lists every blocked thread with its reason and deadline.
- The clock never moves backwards (`kq::tsc_for_deadline`).
- A deadline is reached only when some thread blocks. M46's documented limit stands, extended to
  every timed wait.
- **This pays the "timed waits" item M46 and M47 owed**, for the two primitives node uses.
  `__semwait_signal` and `semaphore_timedwait_trap` stay unmodelled. If the walk reaches one, it is
  a Ruling: small, since §3d's queue is the hard part.

### 3e. psynch condition variables (P1, `retrace-box/src/psynch.rs`)

- A pure-data module keyed by **guest cv address**, the correlation the ulock pair uses. Each cv
  holds what the kernel's `ksyn_wait_queue` holds for the measured shapes: the L/S/U sequence words,
  its waiters (thread, sequence, deadline), and its prepost and pending-signal state.
- **Semantics are ported, not invented.** Each operation follows the libpthread `kern_synch.c`
  version pinned by t0 M4: `psynch_cvwait`, `psynch_cvsignal` (including the targeted form, if the
  walk shows a thread port), `psynch_cvbroad` and `psynch_cvclrprepost`. That covers what each one
  returns, what it updates, and whom it wakes.
  - Each branch the measured shapes do not reach is refused by value: an `Err` naming the branch,
    which the box panics with on both sides (M46's R5 pattern).
  - The `psynch_mutexwait`/`psynch_mutexdrop` pair is in scope only if t0 M4 measures it. It would
    then be keyed by guest mutex address and ported the same way, absorbed under Q3 because it is
    the same subsystem.
- **Blocking.** `BlockReason::Cv { addr, deadline }`. A signal or broadcast wakes per the ported
  rules. The woken thread's return word is written into its saved `x0` at the wake, the
  `blockedctx.rs` discipline. A deadline wake answers the timeout return that t0 M4 measured
  (ETIMEDOUT's shape).
- **Never forwarded.** The host's psynch state is keyed by retrace's own addresses, so a forwarded
  call would block, or wake, retrace itself.
- **Signals.** A signal aimed at a cv-blocked thread aborts on both sides, naming the measurement it
  owes. That is M18's semaphore posture, for M18's reason: a dropped wake is the one failure the
  oracle cannot see.
- **Record and replay** mirror §3c's arm shape, with `verify_thread` after the comparison.

### 3f. JIT write-protect (J1, `retrace-box`, below the trace)

- **Per-thread state.** `Thread` gains `sprr: u64`, starting at **0** for every thread, the main
  thread included (Ruling R1).
  - 0 is what retrace has always answered, so libdyld's bit-36 probe (§2d) is untouched.
  - 0 means "protected", the state every native thread starts in (§2c).
  - It rides `ThreadTable`, and so `BoxState`.
- **Writes.** The arm t0 M1 locates (beside `try_emulate_undef_mrs`, or on the EC 0x18 path) decodes
  `msr S3_6_C15_C1_5, Xt` and admits exactly two values, read from the guest's frozen commpage:
  - `+0x110` sets the thread write-enabled;
  - `+0x118` sets it protected.

  Any other value, `Xt` included, stops with a message naming the value and the pc (Ruling R2). This
  admits nothing of dyld's `+0xd0`/`+0xd8` toggle, which no guest reaches. The `mrs` arm returns the
  thread's `sprr`.
- **The `MAP_JIT` set.** `Box_::jit` holds every `MAP_JIT` range (from `mmap` with flag `0x800`,
  FIXED or not), and the **view** currently stamped into stage 1: `Rx` or `Rw`.
  - `Rx` stamps `ATTR_CODE`, and `Rw` stamps `ATTR_DATA`. No page is ever both, so the W^X invariant
    holds.
  - A new `MAP_JIT` range is stamped with the current view.
  - `munmap` or `mprotect` covering a whole range removes or keeps it. A partial overlap is refused
    by value.
  - A non-`MAP_JIT` RWX request keeps today's `ATTR_CODE` stamp unchanged. Refusing it could
    regress a corpus guest that maps RWX and never writes. t0 M5 counts such requests across the
    corpus and the walks, and a Ruling covers any that write.
- **Flips.** When the running thread's mode differs from the view — after its own `msr`, or after
  `switch_to_thread` brings in a thread in the other mode — every range is re-stamped with
  `set_region_attr` and `flush_guest_tlb` runs once. The view is per process, and it always equals
  the running thread's mode. Under one vCPU and a cooperative scheduler, that is exactly the native
  per-thread view.
- **Cache maintenance.** Set SCTLR UCI (26) and UCT (15), or whichever bits t0 M1(a) shows are
  needed, in `sctlr_el1_for` (the one derivation, `lib.rs:232-243`). That lets `DC CVAU`, `IC IVAU`
  and `CTR_EL0` run at EL0 natively: the DZE precedent. The change applies to every guest. Task 4
  re-runs the corpus gates that execute exec mmaps.
- **Determinism.** Everything here sits in `run()`/`step()` or in box methods both arms call
  (symmetry rule 2). Nothing is recorded.
- **Seeks.** A restore stamps the restored view and flushes the guest TLB whenever the `MAP_JIT` set
  is non-empty. This is `restore`'s M24 parity discipline: a restore that does not re-derive it
  replays fine and diverges on seek.
- **The debugger.** Hardware watchpoints and single-step are unaffected by stage-1 permissions. A
  stop inside JIT code reads its bytes through stage 2. `reverse-continue` into JIT code is the
  rung-10 assertion.

### 3g. The crash demo (D1, `crates/retrace-guest/node/`)

- **`crash_addon.c`** is an N-API addon with two functions:
  - `addressOf(ab)` returns an `ArrayBuffer`'s backing-store address as a BigInt
    (`napi_get_arraybuffer_info`);
  - `deref(ab)` reads the first 8 bytes of that store as a pointer and loads through it.

  It uses no external buffers, because the V8 sandbox may forbid them (inferred; M7 measures).
- **The build.** `retrace-guest`'s `build.rs` builds the addon only if
  `/opt/homebrew/include/node/node_api.h` exists (`cc -bundle -undefined dynamic_lookup`). It
  exports `NODE_CRASH_ADDON: Option<&str>`, so a machine without node builds clean and the test
  skips loudly.
- **`crash.json`** has rows like `crash.py`'s, so the target is computed, never a literal. It is
  `0x4000_DEAD_0000`, bit 46 set and never mapped.
- **`crash.js`**:
  1. reads `crash.json` and computes the target;
  2. allocates `new ArrayBuffer(8)` and gets `cell = addon.addressOf(ab)`;
  3. defines `function store(view, v) { view[0] = v; }`, warms it, and calls
     `%OptimizeFunctionOnNextCall(store)`;
  4. calls `store(new BigUint64Array(ab), target)`;
  5. prints `CRASHJS cell=0x… target=0x… rows=N`;
  6. calls `addon.deref(ab)`, which faults at the target;
  7. prints `UNREACHED` (never reached).

  The command is `node --allow-natives-syntax crash.js <addon path>` (Ruling R6).
- **`node_crash_e2e`** asserts §1 part 3's five properties, following `cpython_crash_e2e`'s
  structure and helpers. The cell is discovered from the marker. The `MAP_JIT` ranges are
  discovered from the recording's `mmap` events (flag `0x800`) and their returns.

### 3h. The fixtures and the gates

Repo-owned C fixtures in `crates/retrace-guest/c/`, built by `build.rs`, each with modes selected by
argv, the `madv_dyn` pattern:

- **`kq_dyn.c`** has five modes:
  - `probe`: libuv's runtime-detection shape, exactly as M47 measured it;
  - `wake`: thread B triggers the `EVFILT_USER` knote thread A is blocked on;
  - `timeout`: a 5 ms timeout with no trigger returns 0 after the idle jump;
  - `pipe`: thread B writes a pipe whose read end thread A waits on with `EVFILT_READ`;
  - `oneshot`: `EV_ONESHOT` delivers once, then nothing.

  Each prints what it observed, and the e2e compares that with native's output.
- **`condvar_dyn.c`** has four modes: `pingpong` (N rounds between two threads), `broadcast` (three
  waiters), `timedout` (a relative timed wait that expires) and `timedsignal` (a timed wait
  signalled before its deadline).
- **`jitwp_dyn.c`** has three modes:
  - `basic`: native's `sprr.c` sequence, run and compared;
  - `twothreads`: thread A write-enabled while thread B, protected, executes the same page
    concurrently with A's writes interleaved at block points;
  - `fault`: a store to a protected `MAP_JIT` page faults as native does. That mode's native exit
    is measured, and the e2e asserts the recorded `Event::Crash`, so the J2-class silent repair is
    guarded by name.

The tests:

- **Unit tests** cover `gkq.rs` and `psynch.rs` as pure data, and the `S3_6_C15_C1_5` decode as a
  pure function. There is one failing-on-deletion test per refusal.
- **`kq_e2e`, `condvar_e2e` and `jitwp_e2e`** each record and replay every mode, compare its output
  with native's, and seek across its blocking point or toggles (the `checkpoint_seek` pattern). In
  `jitwp_e2e`, `reverse-continue` to the code write is checked by effect.
- **`node_e2e`** (§1 parts 1–2): un-ignore `node_prints_one_and_replays`, add its JIT assertion, and
  add `node_timer_replays`.
- **`node_crash_e2e`** (§1 part 3).

Every node test skips loudly through `util::announce` without Homebrew node.

### 3i. The walk and the sweep

- **The walk.** Re-walk every node command on the final binary, record the landmark counts, trace
  sizes and times, and commit the evidence under `docs/sweep-evidence/2026-10-02-m48/`.
- **The sweep.** Re-run the Apple sweep on an idle host (1-minute load below 3, noted). The SCTLR
  change and the `kevent` row are the two changes that could move a row. Expect 49/54, or 50/54 on
  `dddiagnose`'s coin flip. Any other move is a finding.

### 3j. The oracle sites and the audit

- **The oracle sites.** Each new replay mirror that consumes a landmark and returns gets its
  `verify_thread` after its own comparison: `kevent` and each psynch number. CLAUDE.md's count of
  seven sites becomes 7 plus the number added, and the doc is updated with the exact figure.
  Each new site is verified able to fail: delete it, retag a fixture's trace, and replay must
  accept it (the `thread_oracle.rs` control). The control runs on a committed tree (the M42 lesson).
- **The audit** (Task 9) checks:
  - symmetry rule 1, arm by arm: the same `Box_` method with the same arguments on both sides;
  - rule 2 for the JIT;
  - that every new `Box_` field is in `BoxState` and in all three construction sites;
  - `restore` parity, through a seek to a position inside a blocked `kevent`, a blocked `cvwait`,
    and a write-enabled window;
  - that the generic arm's assert list includes every new number.

### 3k. The docs

- **`docs/current-state.md`.**
  - "What works": rungs 9 and 10; guest kqueues, condition variables and JIT write-protect as
    capabilities; node in the guest-breadth line.
  - "Known limits": what stays refused (non-pipe fd readiness, `kevent64`/`kevent_qos` on a guest
    kqueue, the rwlock calls, kqueue dup); the cooperative scheduler's effect on V8 (background
    compilation and GC helpers run only when the main thread blocks — fidelity, not determinism);
    the timed-wait limit extended; and the measured JIT toggle cost.
- **`README.md`.** All three v1 workloads are now met. Edit its headline and limits to match, and
  add node to its performance table if t0 or the walk measured one.
- **`docs/status-log.md`.** Append the M48 section.
- **`CLAUDE.md`.** Add the new e2e gates to the list; update the `verify_thread` site count; note
  the third below-the-trace emulation beside the timebase MRS and the IMPDEF MRS; and name the
  generic arm's new asserts.

## 4. Guards: each asserts the difference it makes

| Guard | The difference it asserts | What it would miss without the assertion |
|---|---|---|
| `kq_e2e probe` | the throwaway kqueue returns native's one event | a kevent that returns 0 lets libuv fall back to a pipe silently |
| `kq_e2e wake` | the blocked thread's delivered event, after the waker's landmark | a model that never blocks would print the same line, but at the wrong landmark |
| `kq_e2e timeout` | the deadline is reached by the idle jump (the jump is visible in `synthetic_tsc`) | a model that returns 0 at once passes the output check |
| `condvar_e2e pingpong` | N rounds in strict alternation, as native | a wake-any model passes 1 round |
| `condvar_e2e timedout` | ETIMEDOUT's measured return word | a wait that never blocks returns early with 0 |
| `jitwp_e2e basic` | 42 from JIT code, and the register sequence native prints | a flip-on-fault model also returns 42 — hence `fault` |
| `jitwp_e2e fault` | the recorded `Event::Crash` at the protected-page store | J2's silent repair would exit 0 |
| `jitwp_e2e twothreads` | B executes while A is write-enabled | a process-wide (not per-thread) view hangs or faults |
| seeks (all three) | a position inside each new state, restored and replayed | restore-parity holes replay fine and diverge only on seek |
| `node_e2e` | `MAP_JIT` present and an SPRR write | a `--jitless`-equivalent run prints 1 too |
| `node_crash_e2e` | the store's pc in a `MAP_JIT` range | an interpreter store passes every other assertion |
| oracle controls | each new `verify_thread` site, deleted, lets a retagged trace through | a missing site is silent |

## 5. Task order and why

| Task | Contents |
|---|---|
| **0** | t0 (§3a). Everything after it can be re-scoped by its wall list. |
| **1** | Rows and the generic arm's asserts (§3b). Cheap, and every later arm needs its row. |
| **2** | The deadline queue (§3d), then guest kqueues (§3c) with `kq_dyn`/`kq_e2e`. The queue comes first because both kevent and psynch block on it. |
| **3** | psynch (§3e) with `condvar_dyn`/`condvar_e2e`. |
| **4** | JIT write-protect (§3f): the register arm, `MAP_JIT`, the view, the SCTLR bits, restore parity, `jitwp_dyn`/`jitwp_e2e`. |
| **5** | The walls t0 absorbed. One step each, at the task its subsystem belongs to, or here if none. |
| **6** | `node_e2e` (§1 parts 1–2). |
| **7** | The crash demo (§3g). |
| **8** | The walk and the sweep (§3i). |
| **9** | The audit and the oracle controls (§3j). |
| **10** | The close: the gate, the docs, the final review, the fix wave, the merge, the push. |

Tasks 2 to 4 are independent of each other's code, except for §3d. They run in that order because
node meets them in that order (§2a, inferred for 3 and 4), so each task's node walk reaches the
next wall.

## 6. Acceptance

- §1's six parts hold.
- **The gate**, chunked as CLAUDE.md requires (with `--bins`, and `--doc` beside any per-target
  library split), is green. Its counts reconcile file by file against M47's close.
- `node_e2e` is un-ignored. No new `#[ignore]` exists, so the ignored count falls by one.
- **The final review** finds no Critical or Important issue. If it does, one fix wave runs, then a
  scoped re-review.
- **The docs** (§3k) are true at the merge commit.

## 7. Halt rules, and what this milestone deliberately does not do

**Halts** — stop, leave the branch intact, write the explanation into the ledger and report it:
- **H1:** a red gate that survives one fix round.
- **H2:** any new `#[ignore]`. Un-ignoring `node_e2e` is not one.
- **H3:** any `TRACE_MAGIC` bump.
- **H4:** a nondeterministic record/replay flake (class E2).
- **H5:** a t0 or walk wall that needs a new subsystem beyond guest kqueues, the deadline queue,
  psynch and JIT write-protect. Real process creation, workloops (`kevent_id`) or a network stack
  are examples.
- **H6:** anything destructive or outside the repo.

A measurement that contradicts this spec is a ledgered Ruling and a re-scope, never a silent
narrowing, and never a halt unless it is one of the above.

**Not done:**
- real process creation (`fork`, `vfork`, `posix_spawn`, exec-in-place);
- workloops, and `kevent64`/`kevent_qos` on a guest kqueue;
- readiness on fds that are not guest pipes (ttys, sockets, files, inherited pipes), so no
  interactive node and no network;
- the psynch rwlock calls;
- lldb-driven node sessions (`gdbserver` is untouched, and an lldb row is a successor's);
- WebAssembly;
- performance work beyond R9's measurement.

## 8. Rulings (made while writing this spec)

- **R1.** Every thread's `S3_6_C15_C1_5` starts at 0, not at native's protected value. 0 is what
  retrace has always answered, so libdyld's path is unchanged and measured. 0 also maps to
  "protected", so every JIT-relevant state is native's. Named deviation: a guest that reads the
  register before writing it sees 0, not `0x2010002030100000`.
- **R2.** Only commpage `+0x110` and `+0x118` are admitted as written values. Anything else is
  refused by value.
- **R3.** Nothing new is recorded. `kevent`'s events, psynch's return words and the JIT view are
  pure functions of box state, so the events carry no writes. The exit-time full-memory comparison
  is what keeps that honest.
- **R4.** Pipe readiness comes from byte accounting over the trace's own returns, not from a host
  probe. A host probe at a wake would be an input at a non-landmark point, which the format cannot
  carry (H3).
- **R5.** A refusal is by value and identical on both sides. A refusal inside `run()` is a panic
  naming the value, raised on both sides.
- **R6.** `crash.js` uses `--allow-natives-syntax` to force synchronous optimization. Its own
  heuristics would put TurboFan on a worker thread that the cooperative scheduler runs only when
  main blocks, and main never blocks in `store`'s loop.
- **R7.** One deadline queue serves the workqueue timers, `kevent` timeouts and timed `cvwait`s,
  with ties broken by thread index.
- **R8.** SCTLR UCI/UCT are set for every guest, not only for JIT guests. One derivation, one
  posture, the DZE precedent.
- **R9** (conditional, written by t0 M5 if it fires): what to do if toggle cost exceeds 10 minutes
  for `console.log(1)`. The candidate is to stamp `MAP_JIT` ranges through hierarchical
  table-descriptor permission bits (APTable/UXNTable), one descriptor per 32 MiB. It would be
  spiked before it is adopted, since whether hierarchical RO prevents the W^X hang is unmeasured.

## 9. Gate prediction (provisional; the plan pins it)

- **M47's close** was 950 passed / 0 failed / 10 ignored over 160 binaries, with 958 `#[test]` in
  the tree.
- **M48 adds:**
  - unit tests in `retrace-box` (`gkq`, `psynch`, the decode) and `retrace-arch` (census rows);
  - three e2e binaries (`kq_e2e`, `condvar_e2e`, `jitwp_e2e`) and one more (`node_crash_e2e`);
  - `node_timer_replays` in `node_e2e`.
- **Ignored count:** 10 → 9.
- **Binaries:** 160 → at least 164, plus any new `retrace-box` test files.
- The plan derives the exact figure from its own test list, by the file-by-file method, before
  Task 10 runs the gate.

## 10. Conformance with the governing documents

- **CLAUDE.md symmetry rule 1.** The `kevent` and psynch arms are in both `record_box` and
  `ReplaySession::advance`, call the same `Box_` method with the same arguments, and sit before
  the generic forward arm. **Rule 2:** the JIT register and its view live in `run()`/`step()`.
- **The oracle.** Every new mirror gets `verify_thread`, and the doc count is updated (§3j).
- **Determinism.** No wall clock: every deadline is on `synthetic_tsc`. No recorder threads. PAC
  and the commpage are unchanged (the commpage is read, never rewritten).
- **The platform invariants.**
  - W^X holds by construction: a page is stamped `ATTR_CODE` or `ATTR_DATA`, never both.
  - Memory stays anonymous.
  - `Box_`'s field order keeps `vcpu` before `vm`; new fields go after both.
- **Honest gates.** Every node test announces its skip. The guards assert the difference (§4).
  `node_e2e`'s reason is deleted with its `#[ignore]`, and the docs record the un-parking.
- **Two documents.** `current-state.md` is edited in place, and `status-log.md` is appended to.

## 11. Corrections from the plan and from the probe

This spec was written before its plan and before t0. Writing the plan's executable steps found the
first eight corrections below. A parallel scratch probe (`node -e 'console.log(1)'` walked to exit
with throwaway stubs, before t0) found the rest. Each correction is applied in the plan. Where a
correction contradicts the text above, **the correction is what binds**, and the text above is left
standing so the reasoning that was corrected stays visible.

### 11a. From the plan

1. **§3j's oracle sites.** §3j says each new mirror gets its own `verify_thread`, so the count
   becomes 7 + N, with a delete-the-site control for each.
   - The `kevent` and psynch mirrors sit inside the `Event::Syscall` chain, after the arm-top
     `verify_thread` (the M45/M46/M47 precedent). A per-mirror call there could not fail
     independently, so its control could not go red.
   - **The count stays 7.** The control becomes a retag test over the new landmarks, plus deleting
     the one arm-top call.
2. **§2e/§3f, non-`MAP_JIT` RWX.** §3f says a non-`MAP_JIT` RWX request keeps an `ATTR_CODE` stamp.
   In fact an anonymous `PROT_EXEC` mmap gets `ATTR_DATA` plus a warning today; only file-backed
   exec mmaps get `ATTR_CODE`. The plan keeps today's anonymous behaviour, and only `MAP_JIT`
   (always `MAP_ANON`) opts into W^X toggling.
3. **§3f, seeks.** §3f says a restore re-stamps the view and flushes the guest TLB. In fact
   `from_checkpoint`/`restore` build a fresh VM and vCPU whose TLB is empty, and the stage-1 stamps
   ride in `mem`. A flush would add the TLBI-stub backing on one side only. The plan carries `jit`
   in `BoxState`, asserts at restore that the stamped tables agree with the view, and does **not**
   flush.
4. **§3f, a function name.** The function is `sctlr_mmu_on` (`lib.rs:242`), over the constant
   `SCTLR_MMU_ON_BASE` (`lib.rs:202-208`), not `sctlr_el1_for`.
5. **§3f, partial `mprotect`/`munmap` of a `MAP_JIT` range.** §3f has these refused by value, but V8
   reserves its code range and commits sub-ranges. `jit.rs` models per-page protection segments,
   with splitting. Only a read-only `MAP_JIT` page is refused, since the box has no read-only data
   attribute (§11b item 4 has the measured shape).
6. **§3c, where a wake is delivered.** §3c says at switch-in, through `settle_schedule`. M46 in fact
   writes at **activation** (`request_manager` → `enter_manager`): onto the live vCPU if the target
   is current, else into its saved context. Kevent and cv wakes are delivered the same way
   (`deliver_wake`), which also covers a wake whose target is the current thread.
7. **§3b, the `kevent` row.** Its `Ptr` kinds cite their bound in the row comment: never forwarded,
   with nchanges/nevents × 32 bytes read or written through the stage-1 walk. `(363,
   View::FdOperands, …)` joins `EXPECTED_DIFFS`. The psynch rows carry only Scalar/Ptr kinds and
   need no entry.
8. **§3a M1(a) / §3h, `sprrprobe`.** A static guest has no commpage, so it cannot drive a real
   toggle. `sprrprobe` guards three things: the `mrs` reading 0, EL0 `ic ivau`, and the refusal of
   an inadmissible `msr` value. The real write and flip path is guarded by `jitwp_dyn`.

### 11b. From the probe

Evidence: the session scratchpad's `m48-probe/walls.md` and `probe.patch`. t0 re-runs each
measurement from committed scripts into the measurements companion, and the companion is what the
tasks cite.

1. **A pre-existing determinism defect, ahead of every node gate.** `hv_vcpu_set_simd_fp_reg` takes
   a 16-byte vector, which AAPCS64 passes in `v0`. bindgen maps it to `u128`, which Rust passes in a
   GPR pair. So every `set_simd` writes the host's `v0` into the guest:
   - `load_ctx` on every thread switch;
   - `from_checkpoint`;
   - the signal-frame restore;
   - `vcpu_set_q`.

   `crates/hv-sys/tests/simd.rs` passes only because the constant happens to be in `v0`. Node's
   replay diverged at its final memory compare under some host environments and not others, and the
   probe localised that to thread 1's q registers at its first landmark. **Ruling:** root-caused,
   so it is a task (the plan's SIMD task) rather than an H4 halt. It runs before any node gate.
2. **§2d's open question.** The `msr S3_6_C15_C1_5` traps as EC 0x00, exactly like the `mrs`. J1's
   arm sits beside `try_emulate_undef_mrs`.
3. **§2f/§3f/R8, the SCTLR bits.** `sys_icache_invalidate` is `dsb ish` then `ic ivau` per 64-byte
   line, plus a commpage read. It issues no `DC CVAU` and reads no `CTR_EL0`. **Only UCI (26)** is
   set. UCT stays clear, because nothing measured needs it.
4. **§3f, the `MAP_JIT` shape.** There is one `MAP_JIT` mmap:
   - 256 MiB, `PROT_NONE`, flags `0x41842`, not FIXED;
   - then an `mprotect` of a sub-range (`+0x40000`, `0xffc0000`) to RWX;
   - `madvise` REUSE/REUSABLE over it;
   - a whole-range `munmap` at exit.

   The view applies to `MAP_JIT` ranges **minus their `PROT_NONE` extents**. The walk made 268 SPRR
   writes and 270 view flips, and the record took 4 s on a release build, so **R9 does not fire**.
5. **A new small wall: partial `munmap`.** `guest_munmap` drops whole backings. V8 over-allocates and
   trims the head and tail: 56 partial `munmap`s in walk 1, some with unaligned lengths such as
   `0x10b20`. Under Q3 it is absorbed as a small model, with its own task: split the backing,
   rounding the end up to 16 KiB.
6. **Rows and forwards absorbed:**
   - `mach_msg2` msgh_id 3419 (`semaphore_destroy`) joins `FORWARD_ALLOWLIST`, mirroring 3418;
   - `setsockopt` (105) gets a row. libuv sets `SO_OOBINLINE` on the stdout pipe and ignores the
     `ENOTSOCK`.
   - `getsockname` (32) fires only when stdin is a socket, which it never is on the e2e path.
7. **§3c/§7, one external-fd readiness answer node needs.** libuv's `uv__stream_try_select` registers
   `EVFILT_READ` on the inherited stdout pipe's **write** end, on a throwaway kqueue, with a 1 ns
   timeout, and needs "0 events, no `EV_ERROR`". Refusing that shape would stop node. t0's native
   replica measures it, and the model answers it per that measurement. Every other external-fd
   readiness question stays refused.
8. **§2i/§3e, the census.**
   - **Threads:** 7, from six `bsdthread_create`s. One libuv loop thread blocks in `kevent` with a
     NULL timeout until exit. Four V8 workers `cvwait` on one task-queue cv.
   - **psynch:** 26 calls (16 `cvwait`, 9 `cvsignal`, 1 `cvbroad`). There is no `mutexwait`,
     `mutexdrop` or `cvclrprepost`. Every `cvwait` carries flags `0xa0` and mutex 0. The only timed
     shape is `sec 0, nsec 1`, libpthread's deadline-already-passed form.
   - **kevent:** 11 calls on 5 kqueues, all `EVFILT_USER` except item 7's.
9. **The build.** Node reaches M47's wall in about 3 s on a release build, against 47 s on debug. A
   whole `console.log(1)` record is 4–5 s on release.
10. **No wall needs a new subsystem (H5 does not fire).** On the probe's stubs, all four walks ran
    to their natural end and replayed clean at least twice:
    - `console.log(1)`;
    - `setTimeout` at 10 ms and at 2 s;
    - a forced-optimization one-liner;
    - the §3g crash demo.

    In the crash demo, `watch <cell>; reverse-continue` landed at a pc inside the `MAP_JIT` range.
    The cell read 2 there and the target one `stepi` later.
11. **A second defect the JIT design must handle.** `flush_guest_tlb` called inside `step()` panics:
    the TLBI stub runs with `MDSCR_EL1.SS` armed and takes a SoftStep exception at EL1. It fires
    when stepping or reverse-continuing across a JIT toggle. J1 clears SS and MDE around the stub,
    and a test steps across a toggle.
12. **§1 part 2 and §3h, timer lengths.** `SYNTH_TSC_STRIDE` advances the guest clock about 384 µs
    per timebase read. A 10 ms `setTimeout` therefore never blocks or idle-jumps: the reads alone
    carry the clock past its deadline. Only the 2 s variant blocked in `kevent` (1.986 s) and
    jumped. The timer row uses a delay of at least 2 s and asserts the jump. The fixtures' timeout
    modes use timeouts long enough to need one.
13. **§2a.** The main loop is kq 10 in the probe's walks, not kq 4.
14. **Trace size.** The `PROT_NONE` reservations are fully backed (about 640 MB). Node's traces are
    526 MB (`console.log(1)`) to 1.2 GB (the crash demo), and the crash demo's debugger session
    peaked at 4.2 GB RSS. This becomes a Known-limits line. The node tests record into temporary
    directories, so the gate leaves nothing behind.
