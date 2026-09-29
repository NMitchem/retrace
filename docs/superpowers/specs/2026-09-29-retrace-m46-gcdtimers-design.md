# M46-gcdtimers: libdispatch timers, end to end, on the synthetic clock

**Date:** 2026-09-29. **Branch:** `worktree-m46-gcdtimers`, to be cut from `main` at the M45 merge
(`f907c33`, pushed 2026-09-29). **Companion:** `2026-09-29-retrace-m46-gcdtimers-measurements.md`,
written by t0 (§3a) and cited by section once it exists. Until then every claim below says where it
comes from: **libdispatch** (`libdispatch-1542.100.32`), **xnu** (`xnu-12377.121.6`), **libpthread**
(`libpthread-539.100.4`, including its `kern/` sources) and **libmalloc** (`libmalloc-812.100.31`),
the newest published tags on 2026-09-29, fetched during brainstorming. Also **code** (read at
`f907c33`, with a path), **measured** (with its evidence file), or **inferred**. The published tags
may be older than what macOS 26.5.2 ships. Every value measured so far matches them (§2a), but a
sourced claim is still owed a measurement wherever t0 can take one, and says so.

Source files are abbreviated as follows. **ek** is libdispatch `src/event/event_kevent.c`, **ev**
`src/event/event.c`, **so** `src/source.c` and **q** `src/queue.c`. **ke** is xnu
`bsd/kern/kern_event.c`, **pw** `bsd/pthread/pthread_workqueue.c` and **ws**
`bsd/pthread/workqueue_syscalls.h`. **ks** is libpthread `kern/kern_support.c` and **pt**
`src/pthread.c`. t0 copies each file it cites into the ledger, because the brainstorming copies live
in `/private/tmp`.

**Approach:** chosen by the operator in brainstorming on 2026-09-29. **Scope:** "GCD timers end to
end", the first of three scopes offered. The other two were "registrations only, no delivery" and
"shape two + walk", which is M45-sized. **Mechanism:** approach A of three, a below-the-trace,
kernel-faithful event manager. B, a fresh manager thread for every delivery, was rejected: it is not
what the kernel does, and it leaks a 512 KiB worker stack per fire. C, recorded delivery, was
rejected: it breaks the trace format and duplicates, above the trace, logic that symmetry rule 2
places below it.

## 1. Purpose

M45 emulated libdispatch's workqueue-kqueue init and measured **outcome B**: the emulation alone
unblocks no real program (status log, M45). Every libdispatch path measured makes a second
`kevent_qos` call right after the init, and M45 refuses it:
- `/usr/bin/automationmodetool`, parked at landmark 363 (`apple_walls_e2e.rs`, its `#[ignore]`
  reason);
- M45's `timer` candidate, stopped at landmark 247;
- M45's `after` candidate, stopped at landmark 246.

Past that call lies a whole mechanism M45 did not touch. libdispatch arms a timer from its **event
manager** thread. It ships the timer through the change list of a workqueue return the box refuses.
When the timer fires, the kernel delivers it on a workqueue thread the box has never started. M46
models that mechanism and makes timers fire on retrace's own synthetic clock. It does this
deterministically and without touching the host.

**Success** has five parts:

1. A repo-owned `dispatch_after` guest and a repo-owned repeating timer-source guest both record to
   exit 0 with their markers, and replay bit-identically, twice, on any machine.
2. Every step the model takes is a pure function of box state, rebuilt identically by record,
   replay, `restore()`, `from_checkpoint` and `step()`, and a seek across a fire proves it (§3f, test 5).
3. Every `kevent_qos` shape, change-list entry and `workq_kernreturn` opcode outside the measured
   set stops the recorder with a message naming the field, the measured value and the actual value.
4. `automationmodetool` is either un-ignored or re-parked at a **new, measured** wall, with its
   `#[ignore]` reason rewritten. The same holds for any other Apple-sweep row the model moves.
5. M45's two owed items are paid. A replay-side shape mismatch returns a `Divergence` instead of
   panicking. The `kevent_qos` mirror compares `ret1`.

## 2. What is known before t0

### 2a. The second call (measured, M45 Task 3; sourced)

The measurement comes from M45's walk (`docs/sweep-evidence/2026-09-28-m45/automationmodetool.entry.txt`):

```
kevent_qos(x0=0xffffffff, x1=<stack>, x2=1, x3=<stack>, x4=0x10, x5=0, x6=0, x7=0x23)
entry: ident 0, filter 0xfff2 (-14), flags 0x0185, qos 0x02000000, udata <heap>, fflags 0xf0000037, rest 0
```

Sourced reading of that call:
- **The filter.** `EVFILT_MEMORYSTATUS` is −14 (xnu `event_private.h:81`).
- **The fflags.** `0xf0000037` is `NOTE_MEMORYSTATUS_PRESSURE_NORMAL|WARN|CRITICAL` (`0x7`),
  `PROC_LIMIT_WARN|PROC_LIMIT_CRITICAL` (`0x30`) and `NOTE_MEMORYSTATUS_MSL_STATUS` (`0xf0000000`)
  (`event_private.h:301-307`). `LOW_SWAP` (`0x8`) is absent. This is libmalloc's
  `malloc_memorypressure_mask_default_4libdispatch` after its init-time OR
  (libmalloc `src/internal.h:319-353`, `src/malloc.c:1150`).
- **The issuer.** `_dispatch_memorypressure_init` issues it, on the thread that ran
  `_dispatch_kq_init` and immediately after it (ek:737-745, ek:2745-2766). The
  `EV_UDATA_SPECIFIC|EV_DISPATCH` flags make it a direct unote (ek:2810-2813). The flags word `0x23`
  is `_dispatch_kq_update_one`'s `IMMEDIATE|ERROR_EVENTS` (ek:867-871) plus `_dispatch_kq_poll`'s
  `KEVENT_FLAG_WORKQ` (ek:764).
- **The kernel's answer.** It returns **0 and writes nothing** on success.
  - Only `EV_ERROR`/`EV_RECEIPT` events are copied out under `ERROR_EVENTS` (ke:8188-8203).
  - The scan is skipped (ke:8208).
  - `filt_memorystatusattach` returns 0, inactive. It fails `ENOTSUP` only for fflags outside
    `0xf000003f` (`kern_memorystatus_notify.c:183-196`).

  On failure the kernel writes one `EV_ERROR` event and returns 1.

**A possible third call (sourced, inferred reachable).** `_voucher_activity_debug_channel_init`
follows the memory-pressure init (ek:737-745). It calls `task_get_debug_control_port`. A non-null
port leads to `dispatch_mach_connect`, which registers an `EVFILT_MACHPORT` knote (libdispatch
`voucher.c:826-849`). Whether the guest takes that path depends on what retrace's
`task_get_debug_control_port` returns. t0 M3 owes the answer, and Halt H3 routes it.

### 2b. How a timer is armed (sourced)

- **The calling thread issues no timer kevent.** `dispatch_after` creates an "after" source
  (so:1407-1460). Activation marks it as needing rearm and wakes the **manager queue**
  (so:525-536, :994, :1042).
- **The calling thread's only kevent is a poke.** An idle manager is poked with
  `{ident 1, EVFILT_USER, flags 0, fflags NOTE_TRIGGER (0x01000000), udata ~7}`
  (`_dispatch_event_loop_poke`, ek:1979-1988; q:6476-6491). A thread with no deferred-items TSD,
  such as main, sends it immediately as `kevent_qos(-1, &ke, 1, out, 16, NULL, NULL, 0x23)`
  (ek:999-1013).
- **The manager arms the timer.** Its drain programs the anonymous timer heap
  (`_dispatch_event_loop_drain_anon_timers`, q:6802; ev:1179-1203; ek:2492-2539). The timer kevent
  goes into the manager's **deferred** list, which has 14 slots (ek:2072). It ships in the change
  list of `workq_kernreturn(WQOPS_THREAD_KEVENT_RETURN)` (§2c). Only an overflow sends an immediate
  `0x23` call (ek:955-975).
- **The timer knote** (ek:2502-2515):

  | field | value |
  |---|---|
  | ident | `0xffffffffffffff00 \| tidx` (mask ek:2488); tidx = clock×3 + qos bucket (`event_internal.h:681-695`) |
  | filter | −7 `EVFILT_TIMER` |
  | flags | `EV_ADD\|EV_ENABLE\|EV_ONESHOT` = `0x15`; a delete is `EV_DELETE\|EV_ONESHOT` = `0x12` (ek:2541-2545) |
  | fflags | UPTIME `NOTE_MACHTIME\|NOTE_ABSOLUTE\|NOTE_LEEWAY` = `0x118`; +`0x20` CRITICAL, +`0x40` BACKGROUND; MONOTONIC `0x198`; WALL `0x9c` (ek:49-75) |
  | data | the absolute deadline in `mach_absolute_time` ticks (ek:2525) |
  | ext[1] | leeway, in mach ticks; the other `ext` slots 0 |
  | qos | `0x02000000` |
  | udata | `_dispatch_timers_heap` (the array base) |

- **One kernel timer per bucket.** libdispatch keeps one kernel timer per `tidx` and programs it for
  that heap's earliest deadline. After a fire it reprograms for the next one. So the order of two
  `dispatch_after`s on one clock and QoS is the heap's job. The kernel only orders across buckets.
- **Kernel semantics.** With `NOTE_MACHTIME|NOTE_ABSOLUTE`, `data` is compared verbatim against
  `mach_absolute_time()`, and `ext[1]` is the leeway (`filt_timervalidate`, ke:1396-1576;
  `filt_timer_is_ready`, ke:1607-1624). Attach forces `EV_CLEAR`, and `NOTE_ABSOLUTE` forces
  `EV_ONESHOT` (ke:1711-1751). A deadline already reached is active immediately. Any fire inside
  `[deadline, deadline + leeway]` is kernel-faithful.

### 2c. How a fire is delivered (sourced)

- **The request.** An activated knote in the manager bucket (`KQWQ_QOS_MANAGER`, ke:6456-6460)
  raises a **manager** thread request (`kqworkq_wakeup` → `kqueue_threadreq_initiate`,
  ke:5795-5805, 7051-7075). The `NOTE_TRIGGER` touch raises the same request (`filt_usertouch`,
  ke:1943-1969).
- **The upcall flags** (ws:51-66):
  - `NEWSPI` `0x40000`, `KEVENT` `0x80000`, `EVENT_MANAGER` `0x100000`;
  - `REUSE` `0x20000` unless this is the thread's first use (pw:4764-4780, 4965-4985);
  - `PRIO_QOS` `0x4000` plus the manager QoS 8 (pw:714);
  - `TSD_BASE_SET` `0x200000` on first use only (ks:951-960).

  The inferred words are:
  - a first-use manager: **`0x3C4008`**;
  - a reused manager: **`0x1E4008`**;
  - a redelivery from inside `KEVENT_RETURN`: **`0x1E0000`**, with no `PRIO_QOS` (pw:3695-3703).

  t0 M2 owes all three.
- **The entry ABI, arm64** (ks:861-872, 782-790):

  | register | value |
  |---|---|
  | pc | the registered wqthread entry |
  | x0 | `self`, the pthread struct |
  | x1 | kport |
  | x2 | stack bottom |
  | x3 | the kevent list |
  | x4 | the flags word |
  | x5 | nkevents |
  | sp | stack top |

  **The event list is at `self − 16×72 = self − 0x480`** (ks:887-913; `WQ_KEVENT_LIST_LEN` 16).
  The 32 KiB data buffer sits below it. With no mach-message payload, `sp` equals the list address.
  With zero events, `x3` is NULL and `x5` is 0 (ks:903-908).
- **The worker's side.** `_pthread_wqthread` calls `__libdispatch_keventfunction(&list, &n)`, then
  `__workq_kernreturn(0x40, list, n, 0)` (pt:2581-2635).
  - **Only a thread started with `EVENT_MANAGER` drains the anonymous timer heap** (q:6593-6598,
    6799-6803).
  - The outgoing changes are written in place over the incoming events (ek:2063-2076; q:6847).
- **The kernel's side of `KEVENT_RETURN`** (pw:3641-3745; ks:994-1021).
  - It **registers** the change list. Errors come back as `EV_ERROR` events.
  - It then scans the manager bucket (ke:7808-7910).
  - **If there are events**, the same thread re-enters `_pthread_wqthread` (`EJUSTRETURN`) with the
    events at `self − 0x480`.
  - **If there are none**, the thread unbinds and parks (ke:4474-4528; pw:3741).
- **The delivered timer event** (ke:1822-1905, 954-1013, 4427):

  | field | value |
  |---|---|
  | ident | as registered |
  | filter | −7 |
  | flags | registered flags, `EV_SYSFLAGS` stripped, plus `EV_CLEAR\|EV_ONESHOT` = `0x35` (inferred) |
  | fflags | 0 |
  | xflags | 0 |
  | data | 1, the expiration count |
  | ext[0] | 0 |
  | ext[1] | leeway |
  | qos | `0x02000000` |
  | udata | as registered |

  libdispatch reads ident, filter, the `EV_ERROR` bit, `data > 0` and udata
  (`_dispatch_kevent_timer_drain`, ek:2549-2560).
- **The delivered `USER` event.** It is `{1, −10, 0x21, 0, 0, udata ~7, qos 0x02000000}`
  (ke:1972-1986). `EV_CLEAR` resets it after delivery (ke:4437-4442). libdispatch ignores its
  content (ek:576-579). Its job is to bring the manager up.
- **One-shot.** A delivered `ONESHOT` knote is dropped (ke:4429-4436). Every arm is a fresh
  `EV_ADD`, and an `EV_ADD` on a live knote reprograms it (ke:1775-1819). After a fire, libdispatch
  marks the bucket disarmed and issues no delete (ev:1190-1194).

### 2d. What the box already does (code, `f907c33`)

- **The synthetic clock.** It is `Box_::synthetic_tsc` (`crates/retrace-box/src/lib.rs:531`). It
  starts at `SYNTH_TSC_START` and advances by `SYNTH_TSC_STRIDE` on every emulated timebase `MRS`
  (`:178-179`; `try_emulate_timebase`, `:1292`). It sits below the trace. It is in `BoxState`
  (`:990`) and in `checkpointparity.rs`'s audit (`crates/retrace-box/tests/checkpointparity.rs:268`).
- **Two time sources exist.**
  - The timebase `MRS` is synthetic.
  - `gettimeofday` (116) is **forwarded**. Its third argument is xnu's `mach_absolute_time`
    out-param, so the host's real value is recorded and replayed.
  - The commpage is a copy of the host's, frozen at load (`:148-157`).
- **The workqueue** (M18).
  - `guest_workq_kernreturn` (`:4975`) admits `0x400`, `0x20` and `0x4`. Every other opcode is
    refused by value, `0x40` included.
  - `guest_workq_reqthreads` (`:5097`) builds a fresh worker with the measured register contract
    (flags `WQ_ENTRY_FLAGS_FRESH` = `0x24_4000`, `:700`).
  - `guest_workq_park` (`:5044`) rewinds `ELR` onto the `svc` and blocks the thread
    `BlockReason::Parked`, which nothing wakes (`crates/retrace-box/src/thread.rs:83-98`). That
    comment already names where a reuse wake belongs: re-entry at `_start_wqthread` with a fresh
    register block and bit 17 set.
- **`guest_kevent_qos`** (`lib.rs:5225`) takes `&self` and accepts M45's init shape only
  (`retrace_arch::kqinit_shape`, `crates/retrace-arch/src/lib.rs:1522`).
- **The scheduler.** `pick_next` returns the lowest-indexed runnable thread (`thread.rs:409`).
  `settle_schedule` (`lib.rs:5804`) calls `schedule_after_block` (`:5815`), which panics with
  `M14: DEADLOCK` when nothing is runnable (`:5818`).
- **The arms.**
  - The record arms are at `crates/retrace-core/src/lib.rs:1044` (workq) and `:1057` (374).
  - The replay mirrors are at `:2232` and `:2246`, inside the `Syscall` arm's chain, so they inherit
    its `verify_thread`.
  - The forward arm asserts both away (`:1235`, `:1244`).
  - `verify_thread` has 7 call sites.

## 3. Design

### 3a. t0: measurements first

t0 runs on the branch before any product code and writes the companion file. Native measurements
use lldb, which the gate already requires.

- **M1, the clocks.** This is the first measurement because Halt H1 depends on it.
  - **(a)** Record M45's `after` candidate, which stops at the refused second call, under
    `RETRACE_TRACE=1`. For every `gettimeofday` (116) landmark, `bt` the caller with
    `retrace debug`.
  - **(b)** Natively, run a `dispatch_after` guest under lldb. Break on `mach_get_times`,
    `gettimeofday` and `mach_absolute_time` while `_dispatch_timers_run`/`_dispatch_timers_program`
    are on the stack, and note which one supplies the UPTIME "now".
  - **(c)** Measure how the guest's `mach_absolute_time()` relates to `synthetic_tsc`: read the
    stub's disassembly and any commpage offset it adds. The firing rule (§3e) compares deadlines in
    guest-visible units, and this is the mapping it needs.

  **Halt H1** applies if the host `mach_absolute_time` from 116 reaches timer logic.
- **M2, the manager's entry, natively.** In a native `dispatch_after` run, break at
  `_pthread_wqthread`. For the first-use manager, the reused manager and the in-return redelivery,
  record `x0`–`x5`, `sp`, `TPIDRRO_EL0` and the 72-byte events at `x3`. **Halt H2** applies if
  these contradict §2c in a way §3d cannot express.
- **M3, the sequence.** Natively, for each fixture of §3f:
  - every `kevent_qos` and `workq_kernreturn` call with its arguments;
  - the change-list entries shipped in each `0x40`;
  - whether the §2a third call appears.

  Then read the M45 `automationmodetool` recording's `task_get_debug_control_port` reply. **Halt
  H3** routes a third registration.
- **M4, the baseline.** The M45 close's counts (846/0/9 over 148), re-derived at `f907c33` from
  source by the file-by-file method, so that §9 starts from a measured floor.

### 3b. The validators (`retrace-arch`)

`retrace-arch` stays zero-dependency. It gains:
- `EVFILT_TIMER`, `EVFILT_MEMORYSTATUS`, `NOTE_TRIGGER`, `EV_ONESHOT`, `EV_DELETE`, `EV_DISPATCH`,
  `EV_UDATA_SPECIFIC`, `KEVENT_FLAG_ERROR_EVENTS`, the timer fflags (`NOTE_MACHTIME`,
  `NOTE_ABSOLUTE`, `NOTE_LEEWAY`, the CRITICAL and BACKGROUND bits), and the `WQ_FLAG_THREAD_*` and
  `WQOPS_*` values of §2c. Each value is checked against the SDK or xnu header in a test, as M45's
  constants were.
- `KeventShape`, the classification of one `kevent_qos` call:
  - `Init`, M45's `KQINIT`;
  - `MemoryStatusAdd`;
  - `ManagerPoke`;
  - `TimerAdd { ident, deadline, leeway, udata }`, for the overflow path only if t0 M3 sees it.

  `kevent_qos_shape(args, entry) -> Result<KeventShape, String>` checks the arguments and the entry
  by value, and names the first mismatch as `kqinit_shape` does. `int` arguments are compared on 32
  bits and pointers on 64 (M45 R2). The `MEMORYSTATUS` fflags are compared exactly (R1): libmalloc's
  mask is one measured value, and xnu's accept-mask is wider than what is measured.
- `ChangeEntry`, the classification of one entry in a `0x40` change list:
  - `TimerAdd { ident, deadline, leeway, udata }`: filter −7, flags `0x15`, fflags in the measured
    UPTIME set;
  - `TimerDelete { ident }`: flags `0x12`.

  `kevent_return_change(entry) -> Result<ChangeEntry, String>` refuses everything else by value.
  That includes MONOTONIC and WALL timers, whose fflags name the clock the model lacks.

`kqinit_shape` stays, as the `Init` arm of the new function, so that `kqinit.rs`'s eight tests keep
their meaning.

### 3c. The state (`retrace-box`)

A new module, `crates/retrace-box/src/kq.rs`, holds `WorkqKqueue`. It is a pure data structure with
unit tests and no `Box_` access:
- **`user`**: the manager's `EVFILT_USER` knote. It is registered by the init and has an `active`
  bit, which `NOTE_TRIGGER` sets and delivery clears (`EV_CLEAR`).
- **`memstatus`**: `Option<udata>`. It is registered and **never activates**, because the model has
  no memory pressure. That is a deterministic answer, and a kernel-faithful one on a host under none.
- **`timers`**: a `BTreeMap<ident, Timer { deadline, leeway, udata, fired }>`. It is keyed by ident,
  so iteration is ordered and deterministic.
- **`manager`**: `Option<tid>`, plus the manager's state: `Bound` (running) or `Unbound` (parked
  awaiting events).

`Box_` gains one field, `kq: WorkqKqueue`. It is added in every place M24 and M31 taught:
- `BoxState`;
- the checkpoint capture;
- `from_checkpoint`;
- `restore()`'s landmark-0 default, which is empty;
- `load_dynamic`'s initialiser;
- `checkpointparity.rs`'s audit, with a named row, as `synthetic_tsc` has.

A field missing from any one of these is record-only state that replay never rebuilds: a passing
record with a diverging replay.

### 3d. The box methods

- **`guest_kevent_qos(&mut self, args)`.** It classifies the call with `kevent_qos_shape`.
  - `Init` registers `user`.
  - `MemoryStatusAdd` sets `memstatus`.
  - `ManagerPoke` sets `user.active` and calls `request_manager()`.
  - `TimerAdd` inserts into `timers`.

  Every accepted shape returns 0 and writes nothing (§2a). Every refusal panics as M45's does. The
  prefix becomes `M46:`, the message names the field, and `args` stays on one line (M45 T3-e).
- **`request_manager()`.** It runs whenever a knote activates.
  - **With no manager thread**, it spawns one on the M18 `place_worker_stack` path, with the M2
    first-use register block (`x4` = `0x3C4008` pending t0), and writes the active events.
  - **With a parked manager**, it re-enters it: the saved context is replaced by a fresh block at
    the wqthread entry, `REUSE` is set (`0x1E4008` pending t0), `sp = x3 = self − 0x480`, and the
    events are written.
  - **With a bound manager**, it does nothing. The manager's next `0x40` scan collects the events.

  In the first two cases the thread becomes Runnable and runs when the current thread blocks. That
  is today's cooperative rule, unchanged.
- **The event write.** The active knotes are written in table order: `user` first if active, then
  fired timers by ident. Each is a `kevent_qos_s` per §2c, at `self − 0x480`, 16 entries at most.
  - **Where the bytes come from.** They are **box writes**, computed from box state that record and
    replay hold identically. They are recomputed on both sides and not recorded, exactly as
    `guest_bsdthread_create`'s kport write at `pthread + 0xf8` is.
  - **What catches an asymmetry.** It surfaces at the next landmark's argument check or at the
    exit-time full-memory compare. §3f's test 5, the seek, is what proves the rebuild on the
    non-linear paths.
  - **The page.** The write goes through the box's demand-commit path, because the manager's stack
    is a fresh reservation.
- **`guest_workq_kernreturn`, opcode `0x40`** (`KEVENT_RETURN`). It is admitted only from the
  manager thread; from any other thread it is refused.
  1. It reads `n` 72-byte entries at `args[1]` (`n = args[2]`, at most 16) and classifies each with
     `kevent_return_change`. A `TimerAdd` inserts or reprograms. A `TimerDelete` removes.
  2. It fires every timer whose deadline has passed (§3e rule 1).
  3. It scans for active knotes.
     - **If there are any**, it redelivers on the same thread: the register block is replaced with
       `x4` = `0x1E0000` (pending t0), `x3`/`x5` point at the rewritten events, and `sp` is reset.
     - **If there are none**, the manager becomes `Unbound`, and the thread parks exactly as
       `guest_workq_park` does.
  - **Hazard, named here so that the plan cannot miss it.** Both dispatch arms write `x0` and the
    carry after the box call (`set_x0_err_and_return`). A redelivery that replaced the live
    registers inside the call would have its `x0` (`self`) clobbered. The redelivery must take
    effect after the return write, through the saved-context switch path or an arm that skips the
    return write for this opcode, identically on both sides.
- **`THREAD_RETURN` (`0x4`)** is unchanged for plain workers. A worker is never a manager, so the
  M18 park stays exactly as it is.

### 3e. The firing rule

The rule lives in `schedule_after_block`, the one place both dispatch loops, `run()`, `step()` and
replay's `finish_event` already reach whenever the running thread has blocked or exited:

1. **Overdue timers fire.** Before picking, every timer with `deadline ≤ now_guest()` fires. It is
   dropped (it is `ONESHOT`), queued as an event with data 1 and flags `0x35`, and
   `request_manager()` is called. `now_guest()` is the guest-visible `mach_absolute_time` for the
   current `synthetic_tsc`, as t0 M1(c) measures it.
2. **The idle jump.** If `pick_next` then finds nothing runnable and a timer is armed,
   `synthetic_tsc` jumps forward so that `now_guest()` equals the earliest deadline. Ties are broken
   by ident. Rule 1 is then applied again, and the pick is retried. The clock **never moves
   backwards**: a deadline already passed fires under rule 1 without a jump. If the pick still
   finds nothing, for example because the manager is blocked on something other than its park,
   the `M14: DEADLOCK` panic of rule 3 fires. There is exactly one jump per settle, so the rule
   cannot loop.
3. **Otherwise nothing changes.** With nothing runnable and nothing armed, the `M14: DEADLOCK` panic
   stands, with its message extended to list the knote table.

Rule 2 fires at the deadline itself, the earliest point of the kernel's `[deadline, deadline +
leeway]` window. Because every later timebase read is at least the deadline, libdispatch's
`_dispatch_timers_run` never sees its target still in the future. The sources warn that it would
otherwise re-arm without end.

**The limit this creates, documented in `docs/current-state.md`.** A timer fires only when some
thread blocks. A guest that spins without blocking, for example polling a flag the timer handler
would set, never lets a timer fire, and hangs. This is the cooperative scheduler's existing limit
(no preemption), extended to time. It is not new nondeterminism.

### 3f. The fixtures and the gate

The fixtures are C, in `crates/retrace-guest/c/`, and are built by `build.rs` like
`dispatch_dyn.c`, with path constants in `retrace-guest`.

- **`after_dyn.c`.**
  - **The default mode.** `dispatch_after(100 ms)` on the default global queue. The block writes
    `fired\n` and signals a semaphore; main waits, then writes `done\n`.
  - **`two`.** B is registered at 200 ms, then A at 100 ms. The output must be `A` then `B`. This
    exercises the reprogram after a fire, through `0x40`.
  - **`wall`.** `dispatch_after(dispatch_walltime(NULL, 100 ms))`, which is the refusal fixture.
- **`timer_dyn.c`.** A `DISPATCH_SOURCE_TYPE_TIMER` with a 50 ms interval prints `tick 1`..`tick 3`,
  cancels itself and signals main, which writes `done\n`. This exercises re-arm and manager reuse
  across fires.

Every fixture is first run natively, in t0 M3, for its reference output.

**The gate is `crates/retrace/tests/gcdtimer_e2e.rs`.** Each test asserts the difference M46 makes
and not only an exit code:

1. **`after` records and replays.** Record exits 0 with `fired\ndone\n`. Before M46 it panics at the
   second call's refusal, so the markers are the difference. The trace holds a
   `workq_kernreturn`/`0x40` landmark on a **nonzero** thread, the manager, which is the model's
   visible trace footprint. Two replays are byte-identical.
2. **`two` fires in deadline order.**
3. **`timer` ticks three times** and replays twice.
4. **`wall` is refused by value.** The recorder stops with the refusal naming fflags `0x9c`. The test
   asserts on the named field, never on the exit code alone.
5. **Seeks across the fire.** From a checkpoint taken before the idle jump, seek to a position after
   it on the manager thread. Registers, `synthetic_tsc` (through a test accessor) and memory must
   equal straight-line replay. Then `reverse-continue` with a watch on `after_dyn`'s marker
   variable must reach the handler's store. This is the test that proves §3c's field is rebuilt on
   every path.
6. **A tampered trace diverges.** A recorded landmark after the fire is rewritten, and replay must
   report a `Divergence` (exit 3), not a panic. This covers M45's owed item.

**Unit tests:**
- `retrace-arch` gains `tests/gcdshapes.rs`: bit-flip refusal sweeps over each accepted shape and
  change entry, the constants against the headers, and the 32/64-bit argument widths.
- `retrace-box`'s `kq.rs` tests the ordering, the `ONESHOT` drop, reprogramming, the `EV_CLEAR` of
  `user`, and "never backwards".

**The existing gates** must stay green unchanged. The ones nearest the change are `dispatch_e2e`,
`kqinit_e2e` (its `flags` refusal of `0x25` still refuses), `thread_oracle`, `checkpoint_seek`,
`hitorder_e2e` and `llsc_e2e`.

### 3g. The owed items from M45

- **The replay-side validator diverges.** `guest_kevent_qos` and the `0x40` path return a
  classification `Result` to both arms. The record arm panics on `Err`. The replay mirror returns
  `Divergence { detail: "kevent_qos shape: …" }`. A replay-side mismatch is reachable only after an
  earlier silent divergence, so it must name itself as a divergence and not blame an unmeasured
  shape (M45 F-2).
- **The `kevent_qos` mirror compares `ret1`**, and so does the `0x40` mirror.

### 3h. The walk

With the model landed:
1. Walk `automationmodetool` to its next wall: un-ignore it, or re-park it with a rewritten reason
   in the house form.
2. The controller runs the Apple sweep **detached, on a signed scratchpad copy, with no concurrent
   `cargo`** (M45 T3-a), and compares it row by row against M45's `rowdiff.txt` baseline.
3. Each moved row is measured against a base binary built from `f907c33`, alternating with the swept
   binary, as M45 did. A move that the base binary shows too is host state, not M46.

## 4. Guards: each asserts the difference it makes

| Guard | What it catches | Proven able to fail by |
|---|---|---|
| `gcdtimer_e2e` 1–3 | the model missing or wrong: the recorder panics or the markers are absent | red on the unmodified branch, before §3d lands (the second call's refusal) |
| `gcdtimer_e2e` 4 | a WALL timer accepted silently | control: widen the fflags check and watch it go red |
| `gcdtimer_e2e` 5 | `kq` or the clock jump missing from a rebuild path | control: drop `kq` from `from_checkpoint` and watch it go red |
| `gcdtimer_e2e` 6 | the replay validator panicking instead of diverging | control: restore M45's panic and watch it go red |
| `gcdshapes.rs` | a validator accepting a bit it should not | the bit-flip sweep itself |
| `checkpointparity` row | the field missing from `BoxState` | the audit's existing mechanism |

Each control runs on the committed tree and is restored with `git checkout`, as in M45.

## 5. Task order and why

1. **t0**, the measurements (§3a). No product code until H1 and H2 are cleared.
2. **The validators** (§3b), which are pure and independent of the box.
3. **The state** (§3c), the `kq.rs` module and its field in all six places, with the parity row.
4. **The box methods and the arms** (§3d, §3g), with RED as the fixtures' current refusal. The
   firing rule (§3e) belongs to this task, because `after` cannot pass without it.
5. **The fixtures and the gate** (§3f), including the seek and tamper tests and their controls. If
   the plan finds it cleaner, this can be written RED ahead of task 4.
6. **The walk and the sweep** (§3h).
7. **The docs:**
   - append the status-log section;
   - edit `docs/current-state.md` for what works, the known limits (§3e's limit and the clock notes)
     and the Apple-sweep figure;
   - update the README and CLAUDE.md only where they state something that changed (CLAUDE.md's
     list of gates gains `gcdtimer_e2e`).
8. **The gate**, chunked as CLAUDE.md requires, then a file-by-file reconciliation against §9.

## 6. Acceptance

- §1's five parts, each shown by the named gate test.
- `TRACE_MAGIC` is unmoved, with no `crates/retrace-trace` diff.
- `verify_thread` stays at 7 sites. The new mirrors live inside the `Syscall` chain.
- `#[ignore]` stays at 9 or lower, with every change explained by a reason rewritten in the house
  form.
- The full chunked gate is green. No `SKIPPED` line appears where the tool is present.

## 7. Halt rules, and what this milestone deliberately does not do

- **H1, two clocks.** If t0 M1 finds the host `mach_absolute_time` (116's out-param) reaching timer
  logic, stop and bring it to the operator.
  - The candidate remedy is to make the frozen commpage's time fields consistent with
    `synthetic_tsc`, so that the commpage path succeeds and 116 is never taken on the timer path.
  - That remedy is a change to the determinism model, and this spec does not authorise it.
- **H2, the entry contract.** If the native registers or the event layout contradict §2c in a way
  §3d cannot express, re-plan before task 3.
- **H3, a third registration.** If an `EVFILT_MACHPORT` or other registration appears on the timer
  path:
  - model it as register-only if it never needs to fire in the fixtures;
  - otherwise stop widening and route it to its own milestone.
- **H4, the format.** Stop if anything requires a trace-format change. Approach A needs none.
- **H5, widening.** As in M45, the walk halts widening, not the milestone. A new subsystem found
  past the timers is recorded and routed. The milestone rests on §3f's gate.

**Not in M46.** Each is refused by value where the guest can reach it:
- MONOTONIC and WALL timers;
- memory-pressure **delivery**;
- `EVFILT_MACHPORT` and `EVFILT_SIGNAL` knotes;
- workloops and `kevent_id` (375);
- `kevent`, `kevent64` or `kevent_qos` on a guest `kqueue()` descriptor, which is node's libuv path;
- `sleep()` and `__semwait_signal` (334), and every other timed wait;
- thread reuse for plain (non-manager) workers;
- preemptive firing.

## 8. Rulings (made while writing this spec)

- **R1 — exact comparison.** Each accepted entry is compared exactly, except the fields the kernel
  and libdispatch let vary per call: `udata`, the timer `ident`'s bucket, `data` (the deadline) and
  `ext[1]` (the leeway). Those are read, not compared.
- **R2 — widths.** `int` parameters are compared on 32 bits and pointers on 64, as in M45.
- **R3 — delivery is recomputed.** The event writes are recomputed, not recorded. This is the
  approach the operator chose. `bsdthread_create`'s kport is the precedent.
- **R4 — fire at the deadline.** The firing point is the deadline itself, the earliest
  kernel-faithful point, and not somewhere within the leeway.
- **R5 — a refusal is a panic** on the record side (M45 R5), and a `Divergence` on the replay side
  (§3g).
- **R6 — no `TRACE_MAGIC` bump.** No `Event` shape changes, and no snapshot byte changes meaning.
  The `kq` table is box state, rebuilt from the guest's own syscalls.

## 9. Gate prediction (provisional; the plan pins it)

The prediction starts from M45's **846 / 0 / 9 over 148**, which t0 M4 re-derives. Expected
additions:
- `gcdtimer_e2e`: 6 tests, a new binary;
- `retrace-arch` `gcdshapes.rs`: about 6 tests, a new binary;
- `retrace-box` `kq.rs` unit tests: about 5, inside the existing lib binary;
- `retrace-guest`: 2 fixture-parse tests, in the existing binary.

That is roughly **865 / 0 / 9 over 150**, or **866 / 0 / 8** if `automationmodetool` un-ignores.
The plan computes the exact figure from its own test list, and the close reconciles against it file
by file.

## 10. Conformance with the governing documents

- **Symmetry rule 1.** Both arms call the same `Box_` methods with the same arguments. The new
  replay code lives inside the existing mirrors.
- **Symmetry rule 2.** The firing rule and the event writes are below the trace, in code that
  record, replay, `step()` and the rebuild paths share.
- **The M24 lesson** (`restore` against `load_dynamic`). §3c names all six places the field goes,
  and guard 5 proves them.
- **Honest-gate discipline.** `automationmodetool` moves only on a measured wall. A skipped tool
  announces itself through `util::announce`.
- **"Never assert on an exit code a weaker failure would also produce."** Tests 1–4 assert markers
  and named fields, test 6 asserts a divergence exit together with its detail, and test 5 asserts
  state.
