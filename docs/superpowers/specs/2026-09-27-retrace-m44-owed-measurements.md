# M44-owed t0 measurements

**Companion to** `2026-09-27-retrace-m44-owed-design.md` (§3a's table: M1–M5). Measured 2026-09-27
on this machine: macOS 26.5.2 (build 25F84, kernel `xnu-12377.121.10~1/RELEASE_ARM64_T6041`),
`/usr/bin/lldb` `lldb-2100.0.17.203`. Branch `worktree-m44-owed` at **`b2ff28e`** (t0 Step 1, the
`[trap]` widening; its parent `ebd0266` is code-identical to M43's `64e471e`).

**Binary.** `target/aarch64-apple-darwin/debug/retrace` built from `b2ff28e` and ad-hoc signed with
`retrace.entitlements`: sha256
`0b6784d3c6f98d8014ecec82c00e573bee7ecaf11d7f1cbd3e6498ded64cc76d` (re-signing reproduces the same
hash). M2 and M5 ran this binary. **M1 ran a throwaway build** (`b2ff28e` plus the change-list dump
quoted in M1; its hash was not kept), restored with `git checkout` afterwards.

**Sources cited.** xnu from `github.com/apple-oss-distributions/xnu` `main`, which is
**`xnu-12377.1.9`** (commit `f6217f891ac0bb64f3d375211650a4c1ff8ca1ea`). That is macOS 26.0's
kernel; the host runs `xnu-12377.121.10`, whose source is not published, so every xnu citation is
one point release older than the measured kernel. libdispatch from
`apple-oss-distributions/libdispatch` `main` (`libdispatch-1542.0.4`). File and line numbers below
are those revisions'.

**Evidence.** `docs/sweep-evidence/2026-09-27-m44-t0/` (its README says what each file is). Trace
files (`.bin`) are not committed (Ruling P1). Scratch logs named `t0-*.log` live in
`.superpowers/sdd/2026-09-27-retrace-m44-owed/` and are not committed.

**Split (Ruling P5).** M3 and M5(i) are controller-run and are **PENDING** below.

---

## M1 — automationmodetool's `kevent_qos` (374)

**Method.** Step 1's `[trap]` line prints `x0`–`x7`. A throwaway dump inside the same
`if trace_log` block printed the change list (`72 × nchanges` bytes at `va_to_ipa(x1)`) and the
guest fd table's view of `x0`; a throwaway `[ret]` line after the generic forward printed each
forwarded return (to see any `kqueue`'s). **No row for 374 was added, even temporarily.**
`export RETRACE_TRACE=1`, then `record-dyn /usr/bin/automationmodetool` with stdin `/dev/null`.
Exit **101** (the M33 panic, unchanged: `panicked at crates/retrace-arch/src/lib.rs:946:38: M33:
syscall 374 (374) has no arg_kinds row`). Evidence: `m1-automationmodetool.err` (361 `[trap]`
lines).

**Raw.**

```
[trap] num=368 (0x170) pc=0x1804af9f0 args=[0x400,0x27ff258,0x18,0x0,0x1,0x0,0x0,0x3]
[trap] num=367 (0x16f) pc=0x1804afa1c args=[0x0,0x27ff258,0x18,0x0,0x1,0x0,0x0,0x3]
[trap] num=374 (0x176) pc=0x1804afa48 args=[0xffffffff,0x27ff348,0x1,0x0,0x0,0x0,0x0,0x21]
[kevent_qos changelist] Some([01, 00, 00, 00, 00, 00, 00, 00, f6, ff, 21, 00, 00, 00, 00, 02, f8, ff, ff, ff, ff, ff, ff, ff, 00, … 00])
[kevent_qos kq] x0=4294967295 is_open=false host=None slots=[Console(0), Console(1), Console(2), Open, Closed, Closed, Closed, Closed, Closed]
```

**Layout and constants, cited.**
- Prototype, `bsd/sys/event_private.h:897-900`: `int kevent_qos(int kq, const struct kevent_qos_s
  *changelist, int nchanges, struct kevent_qos_s *eventlist, int nevents, void *data_out, size_t
  *data_available, unsigned int flags);` — `syscalls.master:555` gives the same eight arguments, so
  `flags` is the eighth, **`x7`**.
- `struct kevent_qos_s`, `event_private.h:115-125`: `uint64_t ident; int16_t filter; uint16_t flags;
  int32_t qos; uint64_t udata; uint32_t fflags; uint32_t xflags; int64_t data; uint64_t ext[4];` —
  8+2+2+4+8+4+4+8+32 = **72 bytes**, no padding; `ident` at offset 0, `filter` at offset 8. The
  72-byte dump was the right size; no re-run.
- Flags: `KEVENT_FLAG_IMMEDIATE 0x000001` (`bsd/sys/event.h:140`), `KEVENT_FLAG_WORKQ 0x000020 /*
  interact with the default workq kq */` (`event_private.h:141`), `KEVENT_FLAG_WORKLOOP 0x000400`
  (`event_private.h:146`).
- Descriptor filters are the ones whose `sysfilt_ops` entry is `file_filtops` (`.f_isfd = 1`,
  `bsd/kern/kern_event.c:1028-1031`): `EVFILT_READ` −1, `EVFILT_WRITE` −2, `EVFILT_VNODE` −4,
  `EVFILT_SOCK` −13, `EVFILT_EXCEPT` −15, `EVFILT_NW_CHANNEL` −16 (`kern_event.c:342-362`; the
  values from `event.h:70-84` and `event_private.h:80-84`). `EVFILT_USER` (−10) maps to
  `user_filtops` (`kern_event.c:351`), not a descriptor filter.

**Decode.**
- `x0` = `0xffffffff` = `-1`. `x2` nchanges = 1; `x3` eventlist = NULL; `x4` nevents = 0; `x5`
  data_out = NULL; `x6` data_available = NULL; **`x7` flags = `0x21` = `KEVENT_FLAG_WORKQ |
  KEVENT_FLAG_IMMEDIATE`**.
- The one entry: `ident` = 1; `filter` = `0xfff6` = −10 = `EVFILT_USER`; `flags` = `0x0021` =
  `EV_ADD | EV_CLEAR` (`event.h:144`, `:151`); `qos` = `0x02000000` =
  `_PTHREAD_PRIORITY_EVENT_MANAGER_FLAG` (xnu `bsd/pthread/priority_private.h:161`); `udata` =
  `0xfffffffffffffff8` = `DISPATCH_WLH_MANAGER` (libdispatch `src/event/event_internal.h:100`,
  `((dispatch_wlh_t)(void*)(~0x7ul))`); the rest zero.
- The caller is libdispatch's `_dispatch_kq_init` (`src/event/event_kevent.c:675-711`), whose
  entry literal is exactly this one (`.ident = 1, .filter = EVFILT_USER, .flags = EV_ADD|EV_CLEAR,
  .qos = _PTHREAD_PRIORITY_EVENT_MANAGER_FLAG, .udata = … DISPATCH_WLH_MANAGER`) and whose call is
  `kevent_qos(kqfd, &ke, 1, NULL, 0, NULL, NULL, KEVENT_FLAG_WORKQ|KEVENT_FLAG_IMMEDIATE)`
  (`:698-699`), right after the two workqueue calls the box already emulates (368 then 367, above).
  On any error but `EINTR` it `DISPATCH_CLIENT_CRASH`es ("Failed to initalize workqueue kevent",
  `:700-709`).
- The run issued **no `kqueue` (362)** at all: zero `num=362` lines in 361 traps.

**The four conditions (R4).**
1. **Fails.** `x7 & KEVENT_FLAG_WORKQ` ≠ 0. In xnu, `kevent_qos()` (`kern_event.c:8406`) takes
   `if (__probable(flags & KEVENT_FLAG_WORKQ)) error = kevent_get_kqwq(p, flags, uap->nevents,
   &kq);` (`:8430-8431`), and `kevent_get_kqwq` works on `p->p_fd.fd_wqkqueue`, allocating it with
   `kqworkq_alloc(p, flags)` if absent (`:7404-7420`). Forwarded, that registers an event-manager
   knote on **retrace's own process's** workqueue kqueue: spec §7 halt 4's class.
2. **Fails.** `x0` = −1 is not a guest slot (`is_open=false`), and no `kqueue` ran.
3. Holds: the only filter is `EVFILT_USER`, not a descriptor filter.
4. Holds trivially: `nevents` = 0, `eventlist` NULL, `data_out`/`data_available` NULL (extent 0).

**Decision: 374 is ROUTED.** No row. `automationmodetool` stays parked at the M33 panic on 374,
with this measurement as its reason. For the successor: the call must be **emulated**, never
forwarded, and a refusal is not a usable answer — libdispatch crashes on every errno but `EINTR`, so
anything short of a modelled success (`0`, no events) re-parks the guest at a
`DISPATCH_CLIENT_CRASH`.

Restored with `git checkout -- crates/retrace-core/src/lib.rs`; `git status` clean afterwards.

## M2 — `getattrlistbulk` (461): the kernel's cap, and `ls`'s size

**Method.** Read `getattrlistbulk()` in xnu `bsd/vfs/vfs_attrlist.c`. Then, on the `b2ff28e`
binary with `RETRACE_TRACE=1` exported, `record-dyn /bin/ls` from the worktree root, stdin
`/dev/null`: exit 101 (M33 panic on 461). Evidence: `m2-ls.err`.

**Raw.** `[trap] num=461 (0x1cd) pc=0x1804b14b4 args=[0x6,0x27ff028,0x700c04000,0x8000,0x8,0x1,0x0,0x0]`
— `dirfd` 6, `alist` `0x27ff028`, `attrBuf` `0x700c04000`, **`bufferSize` (`x3`) = `0x8000` =
32,768**, `options` `0x8`.

**Source.** `getattrlistbulk()` (`vfs_attrlist.c:4204`) never checks `uap->bufferSize`; its only use
is `uio_addiov(auio, uap->attributeBuffer, (user_size_t)uap->bufferSize);` (`:4343`), so the uio
spans the caller's whole buffer. It fills that uio through `VNOP_GETATTRLISTBULK(dvp, &al, va,
auio, …)` (`:4387`, the filesystem's native implementation — APFS's, not in xnu, not citable) or
the default `readdirattr(dvp, fvdata, auio, …)` (`:4412`). `readdirattr` (`:3970`) caps only its
**per-entry** kernel buffer — `if (kern_attr_buf_siz > attr_max_buffer) { kern_attr_buf_siz =
attr_max_buffer; }` (`:3993`), `ATTR_MAX_BUFFER 8192` (`bsd/sys/attr.h:134`) — and then loops
`while (uio_resid(auio) > (user_ssize_t)MIN_BUF_SIZE_REQUIRED)` (`:4002`), `uiomove`-ing one entry
after another into the user buffer (`:4152`). The total written is bounded by `bufferSize` alone.
(The `if (uap->bufferSize > attr_max_buffer)` at `:4569` is `setattrlist_internal`'s, not this
call's.)

**Decision: `Dest(Reg(3))`.** No citable cap ≤ 65,536 exists, so the `getattrlist` `Ptr`
precedent (M34 Ruling 1) does not apply. The row is
`461 => [Fd, Ptr, Dest(Reg(3)), Scalar, Scalar]` (`alist` the fixed 24-byte `struct attrlist`),
which widens the diff window and clamps the forward. `ls`'s 32,768 bytes sit inside the 64 KiB
window, so the clamp is inert for `ls` today. A2 extends the `truncguard` window test with 461.

## M3 — where each target lands with the throwaway rows

**PENDING (controller-run, Ruling P5).** The sweep, the native `ed`/`ls` outcomes and the
per-binary un-ignore / re-park decisions are the controller's, and are folded in here on resume.
One M3 fact is already measured in M2: `ls` reaches 461 on the committed tree.

## M4 — the `_nocancel` twin set and the intercepting arms

**Method.** `m4-pairs.sh` reads every `#define SYS_*_nocancel` in the SDK's
`usr/include/sys/syscall.h` (**32**, `m4-nocancel.txt`) and finds each plain name's number in the
same header (`m4-pairs.txt`). A throwaway test (`m4-pairs-test.rs`, run from
`crates/retrace-arch/tests/` and deleted) compared `arg_kinds(plain)` with `arg_kinds(nocancel)`
for every pair (`m4-pairs.log`). The arms were read by grep over `crates/retrace-core/src`,
`crates/retrace-box/src` and `crates/retrace/src` for every constant and literal of a paired
number.

**Pairs.** Every one of the 32 has a plain twin in the header: **`ORPHANS` = `[]`** (empty; no
reason entries). 14 pairs share one row; 14 have no row on either side (`pselect` 394/395, `wait4`
7/400, `accept` 30/404, `select` 93/407, `fsync` 95/408, `sigsuspend` 111/410, `waitid` 173/416,
`poll` 230/417, `msgsnd` 260/418, `msgrcv` 261/419, `sem_wait` 271/420, `aio_suspend` 315/421,
`__sigwait` 330/422, `__semwait_signal` 334/423); no pair has two different rows.

**The twin set — (a), the pairs where exactly one side has a row (A1's red set):**

| nocancel | plain | plain's row today |
|---|---|---|
| **409** `connect_nocancel` | 98 `connect` | `[Fd, Ptr, Scalar]`, `Plain` |
| **464** `openat_nocancel` | 463 `openat` | `[Fd, Path, Scalar, Scalar]`, `Fd` |
| **542** `preadv_nocancel` | 540 `preadv` | `[Fd, NestedDest, Scalar, Scalar]`, `Plain` |
| **543** `pwritev_nocancel` | 541 `pwritev` | `[Fd, NestedSource, Scalar, Scalar]`, `Plain` |

In every case the `_nocancel` side is the one missing. The set **contains 464**. 542 and 543 are
the two the spec did not predict (§2c named 464 and 409). Sharing 540's row makes 542 refused by the
generic arm's nested-pointer assert, as 540 is; sharing 541's makes 543 forwarded exactly as 541 is
today.

**Arms — (b), places that match a plain number without its twin:**
1. `record_box`'s blocking-signal panic arm, `crates/retrace-core/src/lib.rs:998-1002`: `if num ==
   retrace_arch::SYS_SIGSUSPEND || num == retrace_arch::SYS_SIGWAIT => panic!(…)` matches 111 and
   330, not **410** (`sigsuspend_nocancel`) or **422** (`__sigwait_nocancel`).
2. `retrace_arch::is_signal_syscall`, `crates/retrace-arch/src/lib.rs:1321-1337` — the single
   expression of M11's "no signal syscall is issued in retrace's process", asserted by the generic
   arm at `crates/retrace-core/src/lib.rs:1211` — lists `SYS_SIGSUSPEND` and `SYS_SIGWAIT` but not
   410 or 422.

Today 410/422 still fail loud: they pass the M11 assert, reach `forward_and_diff`, and panic at
`forwarded_shape` (M33) because neither has a row. The hazard is the M33 message's advice ("add the
row"): a row for either would forward a blocking signal wait into retrace's own thread, and A1 would
not object, because both sides of each pair are row-less. **Corpus reach:** none of 409, 464, 542,
543, 410, 422 is in `crates/retrace-arch/tests/census.rs`'s `CENSUS` (measured 2026-09-12, before
M38 moved the walls, which is why 464 is absent though the sweep reaches it); zero hits in M1's and
M2's traces. M3's traces are pending (controller). If they stay at zero, 410/422 go to Known limits
by spec §3a's rule; the fix is two constants in each of the two places above.

**No mismatch** in the rest: `is_console_write` (`is_write_syscall`: 4 and 397), the console-close
arm and the close retirement on both sides (`is_close_syscall`, `crates/retrace-box/src/lib.rs:4223`,
`crates/retrace-core/src/lib.rs:2593-2594`: 6 and 399), `guest_fcntl_dupfd` and `shape_of`
(`is_fcntl_dupfd`: 92 and 406). `dup`, `dup2`, `mmap`, `map_with_linking_np`, the exec refusal and
the thread/workq arms match numbers that have no `_nocancel` twin. `crates/retrace/src` has no
syscall-number special case.

## M5 — debugger baselines

### (i) CPU

**PENDING (controller-run, Ruling P5).**

### (ii) arm64e fixture

`grep -n 'arm64e' crates/retrace-guest/build.rs` → lines 247, 414-425, 430-439, 555. The two
guests built `-arch arm64e` are `strip47` (`asm/strip47.s`) and `bfamstrip` (`asm/bfamstrip.s`):
freestanding asm with one `_start` each and no `bl`/`blr`/`ret` (`grep` finds none); 247 and 555
are comments saying a guest is *not* arm64e. No arm64e guest has a call chain. **Decision: B5
builds `btchain`.**

### (iii) lldb baselines on the three stepping shapes

**Method.** One CLI recording of `threadrust`: `record-dyn` of
`target/aarch64-apple-darwin/debug/build/retrace-guest-1c1babd99bbb9451/out/threadrust`, exit 0,
272 events. The sessions' parameters were computed the way `lldb_e2e` computes them
(`m5-params-test.rs`, `m5-params.log`): the first `__ulock_wait` (515) whose next landmark is
another thread's is **n = 261** on thread 0 (RSP tid 1); its svc is `0x1804afaf4` with ignore count
**m = 0**; the other thread (tid 2) first runs at `b = 0x1804ecc14`, which is also its saved pc at
the wait. `m5-lldb.sh` starts `retrace gdbserver <trace> --port 0`, waits for its port, runs `lldb
-x -b -s <cmds> </dev/null` under `perl -e 'alarm shift; exec @ARGV' 120`, then kills the server.
Every command file begins `log enable -f <packets> gdb-remote packets`, then connects, imports
`retrace.py`, and ends `script print("END")`. Counts are `grep -c 'vCont;s'` over the packet log
(the `vCont?` reply, `vCont;c;C;s;S`, does not match).

**The child's last landmark is `bsdthread_terminate` (361)**, at n = 267 on thread 1 (tid 2), svc
`0x1804b0b78`, ignore count 0; the next landmark (268) is thread 0's `-12`. The brief's assumption
holds.

| shape | commands after the connect | `vCont;s` | `END` | lldb exit | outcome |
|---|---|---|---|---|---|
| **A** blocked step past another thread's breakpoint (session A) | `breakpoint set -a 0x1804afaf4 -i 0`, `process continue`, `breakpoint delete 1`, `where`, `breakpoint set -a 0x1804ecc14`, `thread step-inst`, `thread list`, `where` | **1** (`vCont;s:1`) | yes | 0 | before: `at (261, 312) phase=Bp pc=0x1804afaf4 thread=1`; after: tid 1 at `0x1804afaf8` (svc + 4), `instruction step into`, `at (268, 0)` — the step ran past tid 2's breakpoint to the stepped thread |
| **B** step a thread that is not running (session B) | same first three, `thread select 2`, `thread step-inst`, `thread list` | **1** (`vCont;s:2`) | yes | 0 | refused in place: tid 2 at `0x1804ecc14`, `cannot step thread 2: only the running thread (1) can step` |
| **C** step across the child's `bsdthread_terminate` | `breakpoint set -a 0x1804b0b78 -i 0`, `process continue`, `breakpoint delete 1`, `where`, `thread list`, `thread step-inst`, `thread list`, `where` | **1** (`vCont;s:2`), then one `c` from lldb | yes | 0 | before: `at (267, 58) phase=Bp pc=0x1804b0b78 thread=2`; the step's reply is `replaylog:end`, `exited (code 0)`, on tid 1 at `0x1804b5580`; after: `at (270, 3977)` — **the step ran to the end of the recording** (spec §3c B3's "today", confirmed) |

(`where` is `process plugin packet monitor where`.) Evidence: `m5-{A,B,C}.{cmds,out,packets}`.

### (iv) `next` at a `bl`, no line table

**Method.** `crashy`'s `_main` is at `0x1000004f8` (`nm`); its `__TEXT` vmaddr is `0x100000000`
(`otool -l`), equal to `EXE_BASE`. The first word with `w & 0xfc000000 == 0x94000000` is
`0x9400002a` at offset `0x38`, so **B = `0x100000530`**, `bl 0x1000005d8` (the `_fstat` symbol
stub); B + 4 = `0x100000534`. Script: `breakpoint set -a 0x100000530`, `process continue`, `register
read pc`, `next`, `register read pc`, `thread list`, `where`. Run twice: on a recording whose argv0
is relative (lldb loads no module: "No executable module") and on one whose argv0 is absolute, as
`util::record_dynamic` records it (lldb loads `crashy` and symbolicates `crashy`main + 56`).

**Result, identical in both:** `next` sends **one `vCont;s:1`** and stops with `instruction step
over` at **`pc = 0x1000005d8`** — the `bl`'s *target* (`crashy`fstat`, the symbol stub), **not**
B + 4. lldb exit 0, `END` printed; 43 and 46 packets sent in total. **Decision for B4:** without a
line table, lldb-2100's `next` is a single instruction step that follows the call; its row expects
`pc == target(bl)`, computed from the instruction word, not B + 4. (That is lldb's behaviour, not
the server's: the server answered the one step it was asked for.) Evidence:
`m5-next{,-abs}.{cmds,out,packets}`.

---

## Step 7 halt conditions (spec §7 halt 1)

| condition | status |
|---|---|
| `automationmodetool` does not reach 374 | **not triggered** — it reaches 374 (M1) |
| `ls` does not reach 461 | M3-dependent, **pending** (controller); M2 independently measured `ls` reaching 461 |
| any of `ed`/`desdp`/`dyld_info`/`flex` does not reach 464 | M3-dependent, **pending** (controller) |
| `dddiagnose` does not reach 345 | M3-dependent, **pending** (controller) |
| the twin set does not contain 464 | **not triggered** — it contains 464 (M4) |

No measurement here contradicts a spec premise. Two refinements to the spec's expectations, neither
a contradiction: the twin set is four numbers, not the "at least 464 and 409" of §2c (542 and 543
join), and M4 found an arm-level gap outside the row table (410/422 and the signal arms).
