# M48-node t0 measurements

**Companion to** `2026-10-02-retrace-m48-node-design.md` (§3a: M1–M8; §7: the halts; §11: the
corrections) and to the plan's Task 0. Later tasks read it by section, through controller addenda.

**Where and when.** Measured **2026-10-03** (the file and the evidence directory keep the plan's
2026-10-02 names) on this machine:
- Apple M4 Pro, macOS 26.5.2 (25F84), kernel `xnu-12377.121.10~1/RELEASE_ARM64_T6041`;
- `Apple clang version 21.0.0 (clang-2100.1.1.101)`; `lldb-2100.0.17.203`, which **could not launch a
  process** (Deviation D2);
- Homebrew node 25.6.1 (`/opt/homebrew/Cellar/node/25.6.1/bin/node`, libuv 1.52.1, V8
  14.1.146.11-node.19), ad-hoc signed without the hardened runtime.

Branch `worktree-m48-node` was at **`50e716f`** for every run. Its `crates/` has no diff from
`e6caa65`, where the probe was taken, and `git apply --check` of the probe's patch passed. No product
source was committed; `crates/` of the m48 worktree was never touched.

**Binaries.** Signed copies under `/private/tmp/claude-501/`; the evidence README has each one's
build command and sha256.

| name | profile | source |
|---|---|---|
| **base** | debug | `50e716f` |
| **walk** | release | `50e716f` + `probe-final.patch` |
| **baserel** (t0 addition) | release | `50e716f` |
| **walkdbg** (t0 addition) | debug | `50e716f` + `probe-final.patch` |
| **hvopt** (t0 addition) | debug | `50e716f` + `[profile.dev.package.hv-sys] opt-level = 1` |

**Evidence.** `docs/sweep-evidence/2026-10-02-m48-t0/`. Its README says which command produced each
file, on which binary. No trace, built binary or scratch worktree is committed.

**Outcome in one paragraph.** Every walk in the brief's table reached the expected outcome and
replayed twice with the same stdout. No new wall; **H5 does not fire**. M1, M3, M4, M5, M7 and M8
match the brief's expected values. **M9 does not**: on the base (debug) binary the SIMD fixture prints
`intact` for `thread` and panics for `signal`, so as written it cannot fail at the gate profile. Two
causes, both measured: the debug build of `hv_sys::Vcpu::set_simd` happens to put its value in `v0`
before the call, and the fixture's `signal()` handler is one retrace refuses. That needs a Ruling
before Task 1; the Decisions list proposes one. Two smaller corrections: the `MAP_JIT` code range
does not start at `+0x40000` (M5), and the `{0, 1 ns}` waits are libuv's own shape (M4).

---

## M1. The register and the caches

### M1(a) — the SPRR register and EL0 cache maintenance on the base binary

**Command.** Brief Step 3: `sprrprobe.s` and `sprrprobe-msr.s` (the `ic ivau` line deleted), built
`clang -arch arm64 -nostdlib -static -Wl,-e,_start`, recorded with `base record`.

**Result** (`m1-sprrprobe.log`, `m1-sprrprobe-msr.log`):

```
RECORD ERROR: non-syscall exit: MSR/MRS/sysreg trap (EC=0x18 ISS=0x12dc6a FSC=0x2a) far/ipa=0x0 (UNMAPPED) pc=0x4404 elr=0x1000002f4
RECORD ERROR: non-syscall exit: unknown/uncategorized (EC=0x00 ISS=0x0 FSC=0x0) far/ipa=0x0 (UNMAPPED) pc=0x4404 elr=0x100000300
```

- **`sprrprobe`** stops at elr `0x1000002f4`, the `ic ivau, x3` (`otool -tv`). EC **0x18**. ISS
  `0x12dc6a` decodes as direction 0 (a write/SYS), CRm 5, Rt 3, CRn 7, op1 3, op2 1, op0 1.
- **`sprrprobe-msr`** stops at elr `0x100000300`, the `msr S3_6_C15_C1_5, x1`. EC **0x00**, ISS 0.
- In both, the `mrs` read **0**: `cbnz x0` fell through to the next instruction instead of branching
  to `exit(2)`.

**Against walls.md.** Agrees with §1 rows 4 and 5: EC 0x00 for the `msr`, EC 0x18 with op0 1, op1 3,
CRn 7, CRm 5, op2 1 for `ic ivau`. walls.md's ISS was `0x12dd2a`, which is Rt 9: libsystem's `x9`
where this probe uses `x3`.

**Decision.** Task 6's arm sits in the EC 0x00 path beside `try_emulate_undef_mrs`; the
register reads 0 before any write (R1). `ic ivau` needs SCTLR.UCI. **Halt considered:** none.

### M1(b) — `sys_icache_invalidate`

**Command.** Brief Step 4's `dyld_info -disassemble`, `_sys_icache_invalidate` to
`_sys_dcache_flush` (`m1-icache.txt`).

**Result.** `cbz x1`; masks to 64-byte lines; `dsb ish`; a loop of `ic ivau, x9` with
`add x9, x9, #0x40`, counting 20 lines per batch in `x2`; after the first batch, one read of the
commpage CPU family at `0xfffffc080`, checked against `_cpus_that_need_dsb_for_ic_ivau` to decide
whether to `dsb ish` between batches; then `dsb ish; isb; ret`. **No `mrs CTR_EL0` and no `dc cvau`
appear.**

**Against walls.md / plan F4.** Agrees with §1 row 5 and F4.

**Decision.** UCI (SCTLR bit 26) alone is the measured need. UCT stays clear (§11b item 3).
**Halt considered:** none.

### M1(c) — native values and idempotence

**Command.** `sprr.c` from the static evidence plus one line after the protect call:
`pthread_jit_write_protect_np(1); printf("main   protect2 …", rd());`. `cc -O1`, run natively
(`m1-sprr-native.out`, rc 0).

**Result.**

```
commpage +0x10c=3 +0x110=0x2010002030300000 +0x118=0x2010002030100000
supported_np=1
main   initial  sprr=0x2010002030100000
main   write-en sprr=0x2010002030300000
main   protect  sprr=0x2010002030100000
main   protect2 sprr=0x2010002030100000
jit call=42
child  initial  sprr=0x2010002030100000
main   after-child sprr=0x2010002030300000
```

All four checks hold:
- `+0x10c` = 3;
- `+0x110` = `0x2010002030300000` and `+0x118` = `0x2010002030100000`, whose XOR is `0x200000`,
  bit 21 alone;
- every thread starts at `+0x118`, including a child created while main is write-enabled;
- the second protect call leaves the register at `0x2010002030100000`.

**Against walls.md.** Agrees with §1 row 4 and with the static `sprr.out`.

**Decision.** Task 6 admits exactly these two values, read from the guest's commpage at run time.
The guest's commpage is a frozen copy of the host's (`lib.rs` `COMMPAGE_IPA`), so it holds the same
two values. **Halt considered:** none.

---

## M2. The wall list, re-walked

**Command.** Brief Step 6. The walk binary (release, `50e716f` + `probe-final.patch`) runs
`walk.sh <tag>`. Each run records with `RETRACE_TRACE=1 RETRACE_PROBE=1`, stdin `/dev/null` and
stdout through `| cat`, then replays twice. The censuses run `census.sh`. Landmarks below are
**trap ordinals** (`T<n>`, the count of `[trap]` lines up to the event, from `extract.sh`). They
track the landmark index closely but are not the same number.

**Result** (`m2-*.status`):

| walk | record rc | prints | trace (bytes) | traps | record (release) | replay 1 | replay 2 |
|---|---|---|---|---|---|---|---|
| `e` | 0 | `1` | 525 840 709 | 1470 | 4 s | rc 0, `same_stdout=yes`, 4 s | rc 0, `same_stdout=yes`, 4 s |
| `t10` | 0 | `2` | 525 718 006 | 1471 | 4 s | rc 0, yes, 4 s | rc 0, yes, 4 s |
| `t2000` | 0 | `2` | 530 151 961 | 1482 | 4 s | rc 0, yes, 4 s | rc 0, yes, 4 s |
| `natives` | 0 | `3` | 525 766 782 | 1471 | 4 s | rc 0, yes, 3 s | rc 0, yes, 4 s |
| `crash` | **139** | `CRASHJS cell=0x700c142f0 target=0x4000dead0000 rows=2 opt=101001` (no `UNREACHED`) | 1 203 864 923 | 1407 | 10 s | rc 139, yes, 9 s | rc 139, yes, 9 s |

Every row matches the brief's table. Each walk printed one refusal, M23's existing
`refusing mach_msg2 message-queue send (msgh_id 0x400000cf …)`, and node carried on (P1).

**The wall list** (walk `e` unless stated; each wall is passed by the walk binary's stub, so its
landmark is where the stub fired):

| walls.md §1 | landmark | call / fault | args | caller | class | task |
|---|---|---|---|---|---|---|
| row 0 | — | `getsockname` (32) | — | libuv `uv_guess_handle` on stdin | row (harness artefact) | **noted, not added**: 0 calls in all five walks, because stdin is `/dev/null` (walls.md row 0's reason) |
| row 1 | T1095 (t10 T1096, t2000 T1100, natives T1096, crash T1098) | `kevent` (363) | `[kq 7, changes 0x27ff368, nch 2, events 0x27ff368, nev 1, timeout 0x27ff358 = {0, 0}]`; the event list aliases the change list | `uv__kqueue_runtime_detection` (walls.md) | subsystem K1 | **Task 4** (the row: Task 2) |
| row 2 | T1113 and T1467 (crash: T1116 only) | `mach_msg2` msgh_id 3419 `semaphore_destroy` | dest `0x203` (task self), one MOVE_SEND descriptor (disposition `0x11`) of semaphore `0x1303`, the port of the walk's one `-36`/`-33` pair | `uv_sem_destroy` (walls.md) | row (forward allowlist) | **Task 2** |
| row 3 | T1185 (first of 57) | partial `munmap` | `munmap(0xa3d060000, 0x20000)`, the head of backing `0xa3d060000..0xa3d0dc000` (`0x7c000`) | V8's aligned allocation: over-allocate, trim | small model | **Task 3** |
| row 4 | T1248 (first of 264) | `msr S3_6_C15_C1_5, x0` at pc `0x1804f2ab0` (write-enable; the protect write is at `0x1804f2a60`) | the two commpage values (M1(c)) | `pthread_jit_write_protect_np` | subsystem J1 | **Task 6** |
| row 5 | not visible in a walk | `ic ivau` | — | `sys_icache_invalidate` | subsystem J1 (SCTLR) | **Task 6**. The walk binary sets UCI, so it no longer traps; M1(a) is its evidence |
| row 6 | T1328 (t10 T1328, t2000 T1350, natives T1330, crash T1396) | `setsockopt` (105) | `[fd 1, SOL_SOCKET 0xffff, SO_OOBINLINE 0x100, val 0x27fb40c, len 4]` | libuv `uv__stream_open` | row | **Task 2** |

**The partial `munmap` shapes** (Task 3's measured shapes; `m2-e.munmap.txt`, `m2-crash.munmap.txt`):

| walk | partial `munmap`s | head | tail | interior | unaligned end | past its backing |
|---|---|---|---|---|---|---|
| `e` | 57 | 25 | 32 | 0 | 0 | 0 |
| `crash` | 47 | 23 | 24 | 0 | 0 | 0 |

- **Lengths** run from `0x4000` to `0x3c000`, every one a multiple of 16 KiB. In `e`: `0x4000`×3,
  `0x8000`×9, `0xc000`×3, `0x10000`×3, `0x14000`×3, `0x18000`×4, `0x1c000`×5, `0x20000`×5,
  `0x24000`×5, `0x28000`×2, `0x2c000`×3, `0x30000`×3, `0x34000`×3, `0x38000`×3, `0x3c000`×3.
- **The shape.** V8 maps `0x7c000` (or a 256 KiB-aligned remainder), trims a head to the next
  256 KiB boundary, then trims the tail. Sometimes it trims the tail of the remainder a second time
  (T1189 then T1190: `0xa3d140000+0x18000`, then `0xa3d11c000+0x24000` of `0xa3d100000..0xa3d140000`).
  Five of `e`'s tails are not V8's: at T1431–T1451, during the shutdown joins, each trims the last
  `0x8000` of a `0x4c000` thread mapping at `0x30000000` and up.
- **Unaligned lengths are whole-backing.** 43 `munmap`s in `e` (45 in `crash`) have an unaligned
  end, `0x10b20` among them. Every one is dyld unmapping a file mapping it staged (T85–T605, before
  `main`; the crash addon's `0xc570` at T1350/T1362), and every one is a **whole**-backing unmap: the
  probe classed none of them partial. The 16 KiB round-up is what lets them read as whole.
- **A limit of this census.** The probe looks only at the backing that contains `addr`, and clamps
  the end to it. A `munmap` that starts at a backing's start and runs past its end would therefore
  read as whole and never be logged. Every partial range above ends inside its backing.

**Against walls.md.**
- The six rows, their order, their classes and the calls' shapes all agree.
- walls.md's landmark numbers (~1101, ~1180, ~1197, ~1253, ~1334, ~1336) are approximate and differ
  from these trap ordinals (1095, 1113, 1185, 1248, —, 1328). The order is the same.
- **One sharpening.** walls.md §1 row 3 and spec §11b item 5 say V8 "also munmaps unaligned lengths
  (`0x10b20`)". No partial `munmap` has an unaligned end. The unaligned ones, `0x10b20` included,
  are dyld's whole-backing unmaps. Task 3's round-up is still needed, for those.
- Counts vary run to run: 57/56/56/58/47 partial `munmap`s (walls.md: 56).

**Decision.** Each wall goes to the task the brief names; `getsockname` is noted, not added. Task 3
models head and tail splits, with the end rounded up to 16 KiB. No interior split was measured.
**Halt considered:** H5. No new wall, so it does not fire.

---

## M3. The kevent census and the native replica

**Command.** The `e`, `t2000` and `crash` censuses (`m2-*.census`, `m2-*.probe.txt`), then brief
Step 7: `kqprobe.c`, `kqdetect.c` and `kqpipe.c` built with `cc -O1`, stdout piped.

**The kqueues** (walk `e`; the same five in every walk):

| kq | thread | created | lifecycle | changes (ident, filter, flags, fflags) | calls (nch, nev, timeout → outcome) |
|---|---|---|---|---|---|
| 4 | 0 | T1088, then `fcntl(4, F_SETFD, 1)` | closed at T1458 (shutdown) | (7, USER, `0x21`, 0) | (1, 0, NULL → 0 immediate); (0, 1024, `{0,0}` → 0) |
| 7 | 0 | T1094 | closed at T1096 (`close_nocancel`) | (`0x1e7e7711`, USER, `0x21`, 0), then (`0x1e7e7711`, USER, 0, `NOTE_TRIGGER`) in one call | (2, 1, `{0,0}` → **1** immediate; the event list aliases the change list) |
| 8 | 1 | T1107, `F_SETFD` | closed at T1427 | (9, USER, `0x21`, 0) by tid 1; (9, USER, 0, `NOTE_TRIGGER`) by tid 0 at T1424 | tid 1: (1, 0, NULL → 0); (0, 1024, NULL → **blocked**, woken at T1424 by main's trigger with n=1). tid 0: (1, 0, NULL → 0) |
| 10 | 0 (the main loop) | T1173, `F_SETFD` | never closed | (`0xb`, USER, `0x21`, 0); (`0xb`, USER, 0, `NOTE_TRIGGER`) | (1, 0, NULL → 0) ×2; (0, 1024, `{0,0}` → 1, then → 0) |
| 13 | 0 | T1325 | closed at T1327 | (1, **READ**, `0x5`, 0) | (1, 1, `{0, 1 ns}` → **blocked, timed out** in the same schedule → 0) |

- **`t2000` adds** one kq 10 poll `(0, 1024, {1, 984000000})`. It blocked at T1318, the clock
  idle-jumped at T1331 (`idle jump to 0x71919287df3 (now 0x719165505f3)`), and it timed out with 0.
  It made 14 kevents, 3 blocks, 1 wake and 2 timeouts. walls.md had `{1, 986000000}`: the remainder
  depends on how far the stride has carried the clock.
- **`crash`** makes 7 kevents and stops before shutdown. tid 1 is still blocked on kq 8 at the crash.
- **The fd filter.** One, in every walk: `EVFILT_READ` `EV_ADD|EV_ENABLE` on ident 1, the guest's
  stdout, which in these walks is the write end of the `| cat` pipe, on the throwaway kq 13 with
  `{0, 1 ns}`. No `EVFILT_WRITE`. No fd filter on a loop kqueue.
- **Flags seen:** `0x21` (`EV_ADD|EV_CLEAR`), `0x5` (`EV_ADD|EV_ENABLE`), 0 with `NOTE_TRIGGER`. No
  `EV_DELETE`, `EV_ONESHOT`, `EV_RECEIPT` or `EV_DISABLE`.
- **Shapes:** nevents 0, 1, 1024. Timeouts NULL, `{0, 0}`, `{0, 1}`, and (`t2000`) `{1, 984000000}`.
- **Outcomes:** immediate (every `nev 0` call, the detection and the polls); blocked then woken (kq 8,
  tid 1, by main's cross-thread `NOTE_TRIGGER`); blocked then timed out (kq 13's 1 ns; `t2000`'s
  1.984 s, through the idle jump). Every multi-event return held one event.

**The native replica** (`m3-kqprobe.out`):

```
detect n=1 [ident=0x1e7e7711 filter=-10 flags=0x21 fflags=0 data=0 udata=0x0]
detect slot 1 afterwards n=1 [ident=0x1e7e7711 filter=-10 flags=0 fflags=0x1000000 data=0 udata=0x0]
read on a pipe write end, 1 ns n=0
read on fd 1, 1 ns n=0
write-ready on an empty pipe n=1 [ident=0x4 filter=-2 flags=0x1 fflags=0 data=16384 udata=0x0]
read-ready after 3 bytes n=1 [ident=0x3 filter=-1 flags=0x1 fflags=0 data=3 udata=0x0]
write-ready after 3 bytes n=1 [ident=0x4 filter=-2 flags=0x1 fflags=0 data=16381 udata=0x0]
read on the write end, reader closed n=1 [ident=0x4 filter=-1 flags=0x8005 fflags=0 data=0 udata=0x0]
write on the write end, reader closed n=1 [ident=0x4 filter=-2 flags=0x8001 fflags=0 data=0 udata=0x0]
```

| probe | expected | measured |
|---|---|---|
| `detect` | `n=1`, ident `0x1e7e7711`, filter −10, flags `0x21`, fflags 0, data 0 | **as expected** |
| `detect slot 1 afterwards` | untouched (flags 0, fflags `0x1000000`) | **as expected** |
| read on a pipe write end, 1 ns | `n=0` | **n=0** |
| read on fd 1, 1 ns | `n=0` | **n=0** |
| write-ready on an empty pipe | `n=1`, data = capacity (16384) | **n=1, data 16384**, flags `0x1` |
| read-ready after 3 bytes | `n=1`, data 3, flags `0x1` | **as expected** |
| write-ready after 3 bytes | data 16381 | **16381**, flags `0x1` |
| read on the write end, reader closed | `n=1`, flags with `EV_EOF` | **n=1, flags `0x8005`** (`EV_EOF\|EV_ENABLE\|EV_ADD`), data 0 |
| write on the write end, reader closed | `n=1`, flags with `EV_EOF` | **n=1, flags `0x8001`** (`EV_EOF\|EV_ADD`), data 0 |

The returned flags are the flags the knote was added with (`EV_ENABLE` kept when it was passed),
plus `EV_EOF`. `m3-kqdetect.out` and `m3-kqpipe.out` are byte-identical to the probe's outputs
(`diff` empty). In particular, deleting an unknown ident returns one `EV_ERROR` event (flags
`0x4002`, data 2 = `ENOENT`), and a second poll after the `EV_CLEAR` delivery returns 0.

**Against walls.md.** Agrees with §3 (kevent) and with "not walls": five kqueues; 4, 7 and 13 as
described; 10 the main loop. walls.md counts 10–14 calls per walk. `crash` makes 7, because it stops
before shutdown.

**Decision.** Task 4's `T0(M3)` constants are the measured column: capacity 16384, the returned
flags as added (`0x21`; `0x1`; `0x5` keeps `EV_ENABLE`), `EV_EOF` `0x8000` OR-ed in with data 0 for
both filters once the reader is closed, and slot 1 untouched. **Halt considered:** none.

---

## M4. The psynch census and the kernel's return words

**Census command.** `m2-*.probe.txt` and `m2-*.census`.

| walk | 303 `cvbroad` | 304 `cvsignal` | 305 `cvwait` | timed `cvwait`s (all `sec 0, nsec 1`) | other psynch |
|---|---|---|---|---|---|
| `e` | 1 | 9 | 16 | 6 | none |
| `t10` | 1 | 9 | 16 | 6 | none |
| `t2000` | 1 | 10 | 18 | 6 | none |
| `natives` | 1 | 9 | 16 | 6 | none |
| `crash` | 0 | 8 | 9 | 1 | none |

P4 is confirmed in every walk:
- only 303, 304 and 305 occur;
- every `cvwait` has flags `0xa0` and mutex 0;
- every timed one is `sec 0, nsec 1`;
- no `cvsignal` carries a nonzero thread port.

There is no prepost: no `cvsignal` ever found no waiter. walk `e`'s order (`m6-e.timeline.txt`):
- main `cvwait`s on the start-up cv `0x27ff458` four times, once per V8 worker (tids 2–5). Each
  worker `cvsignal`s it, then `cvwait`s on the task-queue cv `0x700c7a5d8`;
- main's fifth wait on the start-up cv is the `{0, 1}` one (T1150);
- main `cvsignal`s the task-queue cv four times (T1221–T1246);
- **the one `cvbroad` is tid 2's**, at T1334, waking main from an untimed wait on cv `0x700c7a608`.
  It is not at shutdown, as walls.md §3 says;
- the other five timed waits are at shutdown, T1452–T1459.

**What the caller does with the return** (`src/pthread_cond.c` at the pinned tag):
- **`_pthread_psynch_cond_wait`.** A return of `(uint32_t)-1` (the stub's carry path; `errno` = the
  raw word) is switched on `err & 0xff`: `ETIMEDOUT` returns ETIMEDOUT, `EINTR` returns 0, anything
  else EINVAL. Then `_pthread_cond_updateval(cond, mutex, err, 0)`. A nonzero success word goes to
  `_pthread_cond_updateval(cond, mutex, 0, updateval)`. 0 does nothing.
- **The signal/broadcast path.** A return that is neither −1 nor 0 goes to
  `_pthread_cond_updateval(cond, NULL, 0, updateval)`.
- **`_pthread_cond_updateval`** turns an error into `PTHRW_INC`, plus `CBIT` if `ECVCLEARED` (0x100)
  and `PBIT` if `ECVPREPOST` (0x200). It adds the word's count to S, ORs in its C/P bits, and clears
  `busy` once L == S.

**Pin.** `m4-pin.txt`:
- the guest's `libsystem_pthread.dylib` `LC_ID_DYLIB` `cur-vers: 539.100.4`;
- `apple-oss-distributions/libpthread` tag `libpthread-539.100.4` = commit
  `1f4f5265b319111142f1bf3a27d4484ef5a98314` (2026-04-17);
- sha256 `kern/kern_synch.c` `99f1445afa395a714fbc82c178dbbbdce662fdf8bdc393aaaca714226032f067`,
  `kern/synch_internal.h` `0ef6d51c695deb9adccc65c6b13277d722a3739124caae32354d262b73b23564`,
  `src/pthread_cond.c` `1ae8db5b479ad2a7314fc5c5d61b765a0a88be5c9521a2db64448f40b1232a0b`;
- `kern.pthread_mutex_default_policy: 0`, `hw.tbfrequency: 24000000`.

**The stubs** (`m4-stubs.txt`). Each of `___psynch_{mutexwait,mutexdrop,cvwait,cvbroad,cvsignal}` is
`mov x16, #N; svc #0x80; b.lo …`, with N = `0x12d`, `0x12e`, `0x131`, `0x12f`, `0x130`. Stub + 8
is the instruction after the `svc` (F5).

**The native return words.** **lldb could not launch any process** in this session (Deviation D2). The
measurement was taken with an interposer instead (`m4interpose.c`, loaded with
`DYLD_INSERT_LIBRARIES`). It replaces libsystem_kernel's psynch stubs, which libsystem_pthread
imports. Each wrapper issues the same `svc` itself and logs the eight argument registers and the
kernel's raw `x0` and carry, which is what the stub sees at +8. It then returns as the stub does.
`cvprobe` ran unchanged under it (`m4-<mode>.out`):

```
waitsignal: cvwait  enter x1=0x100000100 x2=0 x3=0 x4=0 x5=0xa0 x6=0 x7=0
            cvsignal enter x1=0x100 x2=0 x3=0 …  -> return x0=0x101 carry=0
            cvwait  return x0=0 carry=0
broad3:     cvwait  x1=0x100000100 / 0x200 / 0x300, x5=0xa0
            cvbroad enter x1=0x300 x2=0x300  -> return x0=0x301 carry=0
            three cvwaits return x0=0 carry=0
timeout:    cvwait  enter x5=0xa0 x6=0 x7=0x2faf080 -> return x0=0x13c carry=1   (rc=60)
onens:      cvwait  enter x5=0xa0 x6=0 x7=0x1       -> return x0=0x13c carry=1   (rc=60)
```

| value | expected | measured |
|---|---|---|
| a woken waiter's `cvwait` | — | **0, carry clear** |
| the signaller's `cvsignal`, one waiter | `0x101` (CBIT once L and S balance); the probe's stub said `0x100` | **`0x101`, carry clear** |
| `cvbroad` to three | — | **`0x301`, carry clear** (3 × `PTHRW_INC` \| CBIT) |
| `timeout` (50 ms) | `0x13c` with carry | **`0x13c` (ETIMEDOUT \| ECVCLEARED), carry set** |
| `onens` (`{0, 1}`) | `0x13c` with carry | **`0x13c`, carry set** |
| `cvwait` flags | `0xa0` | **`0xa0`** |
| `mutexwait`/`mutexdrop` in any mode | none | **none** (and no `cvclrprepost`) |

**`c_seq`** (`m4-layout.out`):

```
fresh:            bbb1b03c 00000000 00000000 00000000 00000000 00000000 00000000 00000000 00000000 …
one waiter:       454e4f43 00000000 00000000 00000080 0040cc00 01000000 00010000 00000000 00000000 …
after the signal: 454e4f43 00000000 00000000 00000080 00000000 00000000 00010000 01010000 00010000 …
```

The words that move by `0x100` are bytes **24–27 (L), 28–31 (S) and 32–35 (U)**, as expected. After
the signal, L = `0x100`, S = `0x101` (CBIT) and U = `0x100`. Bytes 16–23 hold `busy`, the mutex
(`0x100cc4000`), while a waiter is in the kernel, and 0 after. Bytes 12–15 hold `0x80000000`: pshared
2 (private) and misalign 0, so `COND_GETSEQ_ADDR` takes `c_seq[0..2]` in L, S, U order. After a lone
timed-out wait (`m4-timeout.out`, `m4-onens.out`): L `0x100`, S `0x101`, U 0.

**Where native node's `{0, 1 ns}` waits come from** (`m4-node-onens.out`, `m4-uv-cond-destroy.txt`).
Native `node -e 'console.log(1)'` under the interposer issues **6** `cvwait`s with `x6 == 0, x7 == 1`.
Every one comes from `libsystem_pthread _pthread_cond_wait + 1024` ← **`libuv uv_cond_destroy + 92`**,
reached from V8Platform/NodePlatform start-up and from TaskQueue, WorkerThreadsTaskRunner and
DelayedTaskScheduler teardown. `uv_cond_destroy` (libuv 1.52.1) builds a private mutex, locks it, and
calls `pthread_cond_timedwait_relative_np(cond, &m, &ts)` with a **constant** `ts = {0, 1}`
(`__TEXT,__const`), accepting 0 or ETIMEDOUT. This is libuv's Darwin workaround for destroying a
condvar that was signalled but never waited on. **So the `{0, 1 ns}` waits are libuv's own shape,
not an artefact of the synthetic clock.** Each walk's 6 matches native's 6.

**Native node also does what the walk never does** (`m4-node-onens.out`):
- 5 `mutexwait`s (returns `0x103`, `0x203`, `0x403`) and 3 `mutexdrop`s;
- 2 `cvwait`s with flags **`0x10a0`** (`_PTHREAD_MTX_OPT_NOTIFY`) and a nonzero mutex;
- 30 `cvsignal`s (24 return `0x100`, 6 return `0x101`), and 1 `cvbroad` returning `0x401`.

These are preemption's contention shapes. Under retrace's cooperative scheduler no mutex is ever
contended in the walks. They are the shapes Task 5 refuses (F3), and they are native, not reachable
from these walks.

**Against walls.md.** The census agrees with §3 psynch, except the `cvbroad`'s timing (above).
walls.md's open question about the 1 ns origin is answered: it is libuv's, not clock drift. The
probe's stub return words (0 to a waiter, `0x100` per woken waiter, ETIMEDOUT 60 alone) are not the
kernel's. The kernel adds CBIT and ECVCLEARED.

**Decision.** Task 5's `T0(M4)` values are the measured column: `cvsignal` → `0x101` when it wakes
the last waiter, `cvbroad` → n × `0x100` | CBIT, a woken `cvwait` → 0, a timed-out lone `cvwait` →
`0x13c` with carry, flags `0xa0`, `c_seq` at bytes 24–35. The design is unchanged by the 1 ns
origin (Global Constraints, the passed-deadline rule). Task 5's port should still cross-check the
native `0x100` that a `cvsignal` returns when waiters remain, which `cvprobe` did not exercise.
**Not measured:** spec §3a M4's "wait satisfied by a prepost". The brief's `cvprobe` has no such
mode, and no walk produced a prepost. **Halt considered:** none.

---

## M5. The JIT census

**Command.** The `e`, `natives` and `crash` censuses and `[probe-summary]` lines (`m2-*.err`), the
`MAP_JIT` lines of `m2-*.probe.txt`, and `/usr/bin/time -l` over a debug record and replay of
`console.log(1)` on walkdbg (`m5-debug-*.err`).

**The mapping.** Exactly one `MAP_JIT` mmap per walk, at T1180–T1185:
`mmap(hint 0xa08470000, 0x10000000, PROT_NONE, 0x41842, fd 0xff000000 (VM tag 255), 0)`. Its flags
are `MAP_JIT|MAP_ANON|MAP_NORESERVE|MAP_PRIVATE|MAP_UNIX03`, **not FIXED**; retrace places it past
the hint. Then:

| walk | `MAP_JIT` base | `mprotect(…, 0xffc0000, RWX)` at | PROT_NONE head | PROT_NONE tail |
|---|---|---|---|---|
| `e` | `0xa2d060000` | `0xa2d080000` | `0x20000` | `0x20000` |
| `t10` | `0xa2d038000` | `0xa2d040000` | `0x8000` | `0x38000` |
| `t2000` | `0xa2d050000` | `0xa2d080000` | `0x30000` | `0x10000` |
| `natives` | `0xa2cf9c000` | `0xa2cfc0000` | `0x24000` | `0x1c000` |
| `crash` | `0xa2cfbc000` | `0xa2cfc0000` | `0x4000` | `0x3c000` |
| walls.md's own walk 1 (`probe/logs/w1.err`) | `0xa2d00c000` | `0xa2d040000` | `0x34000` | `0xc000` |

- The committed range always starts on a **256 KiB boundary** and is `0xffc0000` long, so a
  `PROT_NONE` head of `0x4000`–`0x34000` (measured) precedes it and a `PROT_NONE` tail of
  `0x40000 − head` follows it. Inferred, not measured: V8 rounds `base + 16 KiB` up to 256 KiB.
- The single `mprotect` is the only one over the range.
- `madvise` covers the committed range: REUSE (8) then REUSABLE (7) over all `0xffc0000` right after
  the `mprotect`. Later REUSE/REUSABLE pairs cover `0x40000` at its start (and in `natives`, the next
  256 KiB chunk too). This is M47's model, unchanged.
- One whole-range `munmap` at shutdown (`e` T1383); none in `crash`.
- No non-`MAP_JIT` exec mmap. No write to a protected `MAP_JIT` page (nothing faulted).

**Toggles** (`[probe-summary]`):

| walk | SPRR writes | by thread | view flips | flips caused by a switch | max runnable |
|---|---|---|---|---|---|
| `e` | 264 | main 16, tid 2 248 | 266 | 0 | 6 |
| `t10` | 264 | main 16, tid 2 248 | 266 | 0 | 6 |
| `t2000` | 264 | main 16, tid 2 248 | 266 | 0 | 5 |
| `natives` | 274 | main 22, tid 2 252 | 276 | 0 | 6 |
| `crash` | 20 | main 20 | 22 | 0 | 4 |

- Every write was one of the two commpage values: the probe asserts it, and no assert fired. The
  sampled writes (the first 20 of each walk) alternate RW at pc `0x1804f2ab0` and RX at pc
  `0x1804f2a60`.
- View flips = writes + 2, the mmap and the `mprotect` re-stamps.

**Cost.**
- Release (walk binary): `console.log(1)` records in **4 s** (crash demo 10 s).
- Debug (walkdbg, `m5-debug-*.err`): record **42.57 s** (773 MB max RSS), replay **44.91 s**
  (962 MB). Both rc 0, same stdout.

**Against walls.md.**
- Agrees with the mapping, the flags and the no-switch-flip result.
- Write counts differ slightly: 264 here (main 16, tid 2 248) against walls.md's 268 (main 16,
  tid 2 252), and 274 against 278. They vary run to run.
- The release figure (4 s) is within a factor of 2 of walls.md's 4–5 s. The debug figure was taken
  anyway, because walkdbg existed; it matches walls.md's 45 s / 46 s.
- **Correction.** walls.md §1 row 4 and §3, spec §11b item 4 (`+0x40000`, `0xffc0000`), plan P5 and
  Task 6 Step 1 all say the `mprotect` is at `+0x40000`. It is at the first 256 KiB boundary past
  the base, never `+0x40000` in six walks (walls.md's own walk 1 was `+0x34000`). The range also
  keeps a `PROT_NONE` **tail**. Task 6's stamped view (the ranges minus their no-access extents) must
  handle both extents. Its fixture's synthetic `OFF = 0x40000` in a 1 MiB reservation is still a
  valid shape, but no test or comment should say V8's offset is `+0x40000`.

**Decision.**
- **R9 does not fire**: debug `console.log(1)` records in 42.57 s, far below 10 minutes.
- Task 6 admits the two values above and stamps the `MAP_JIT` range minus both of its `PROT_NONE`
  extents.

**Halt considered:** none.

---

## M6. The thread census

**Command.** The censuses, `[probe-summary]`, and `thread-timeline.sh` (`m6-*.timeline.txt`). Trap
lines carry no thread, so the timeline shows them as `tid=?`.

- **Creates.** 6 `bsdthread_create`s in every walk: 7 threads.
  - The first (T1103 in `e`, func `0xa08057bec`) is the libuv loop thread, tid 1. Main then blocks
    in `semaphore_wait_trap` (−36) on port `0x1303` until tid 1's `semaphore_signal_trap` (−33).
  - Four more (T1117–T1129, func `0xa080559f0`) are V8 platform workers, tids 2–5.
  - The last (T1235, func `0xa080f8eac`, arg 0) is tid 6, the late thread.
- **Block reasons over time** (walk `e`):

  | thread | blocks on |
  |---|---|
  | main (0) | `semaphore_wait` (once); `cvwait` on the start-up cv ×4 (untimed); `{0, 1}` `cvwait` ×1 at start-up and ×5 at shutdown, each timing out in the same schedule; `kevent` kq 13 `{0, 1 ns}` (timed out); untimed `cvwait` on `0x700c7a608` (woken by tid 2's `cvbroad`); `__ulock_wait` ×5 at shutdown, on `pthread + 0x34` of each exiting thread (`pthread_join`; the trap lines carry no thread, so "main" is inferred) |
  | 1 | `kevent` kq 8, nev 1024, NULL, from T1112 until main's `NOTE_TRIGGER` at T1424; then exits |
  | 2–5 | `cvwait` on the task-queue cv `0x700c7a5d8` after the start-up handshake. Each is woken by one of main's four `cvsignal`s; tid 2 runs work and waits again at T1341, and is woken at T1342. All exit at shutdown (`__ulock_wake` + `bsdthread_terminate`, T1429–T1450) |
  | 6 | Runnable at exit, never scheduled (`e`, `t10`, `natives`). In `t2000` it ran and parked in `semaphore_wait` (`Blocked(Sem { port: 4627 })`) |

- **Max runnable:** 6 (`e`, `t10`, `natives`), 5 (`t2000`), 4 (`crash`).
- **States at exit:** `e`/`t10`/`natives`: `0:Runnable, 1–5:Exited(0), 6:Runnable`. `t2000`: the
  same, but 6 is `Blocked(Sem)`. `crash`: `0:Runnable, 1:Blocked` (kevent kq 8), `2–6:Runnable`.
- **No deadlock panic** in any walk.

**Against walls.md.** Agrees with §3 threads (P6). The one sharpening is the `cvbroad`'s timing (M4).

**Decision.** Nothing new for Tasks 4–5 beyond the census. **Halt considered:** none.

---

## M7. The crash demo

**Command.** Brief Step 9. The addon was built with
`cc -bundle -undefined dynamic_lookup -I /opt/homebrew/include/node` (exit 0) and run natively. The
walk binary's `crash` trace was replayed under `/usr/bin/time -l … debug --script …`. A second
recording added `where` and `threads` (t0 addition, `m7-where.*`).

**Result.**
- **Native** (`m7-native.out`): `CRASHJS cell=0xa70cc6b40 target=0x4000dead0000 rows=2 opt=101001`,
  rc **139**. `opt=101001` = kIsFunction | kOptimized | kTurboFanned.
- **Recorded** (`m2-crash.out`): `CRASHJS cell=0x700c142f0 target=0x4000dead0000 rows=2 opt=101001`,
  rc 139, no `UNREACHED`. Both replays rc 139 with the same stdout.
- **The addon loads on the FIXED exec dylib path.** Its text is
  `mmap(0xa3da68000, 0x4000, R-X, 0x40012 (FIXED), fd 0xc, 0)` (T1358).
- **The fault** (`m2-crash.err`): `[fault] pc=0xa3da68738 esr=0x92000005 far=0x4000dead0000 ec=0x24`,
  twice (T1397, T1407). The first fault goes to node's own SIGSEGV handler, which returns
  (`sigreturn`, trap 184) and re-faults. The second is the terminal `Event::Crash`. DFSC `0x05` is a
  level-1 translation fault. The pc is inside the addon's text (`deref`).
- **The debug session** (`m7-debug.out`):

  ```
  > continue
  guest crashed: pc=0xa3da68738 far=0x4000dead0000 esr=0x92000005
  > watch 0x700c142f0
  watch at 0x700c142f0 len 8
  > reverse-continue
  hit watch 0x700c142f0 (write at 0xa2cfc09a8) at (1378, 1403315)
  > x 0x700c142f0 8
  0x700c142f0: 02 00 00 00 00 00 00 00
  > stepi
  > x 0x700c142f0 8
  0x700c142f0: 00 00 ad de 00 40 00 00
  ```

  - The store's pc `0xa2cfc09a8` is **inside the `MAP_JIT` range** `[0xa2cfbc000, +0x10000000)`, and
    inside its RWX committed part `[0xa2cfc0000, +0xffc0000)`, at offset `0x9a8`.
  - The cell holds **2** (the warm-up value) at the hit, and the **target** `0x4000dead0000` one
    `stepi` later.
  - **Cost** (`m7-debug.err`): **13.63 s** real, **4 135 682 048 B maximum RSS** (peak memory
    footprint 4 196 702 256 B).
- **The second recording** (`m7-where.out`): the crash is at `(1404, 0)` on **thread 0**. Threads:
  `0` Runnable, `1` Blocked (kevent), `2–6` Runnable. The `reverse-continue` lands at
  `(1373, 1404642)`, pc `0xa2d0009a8`, thread 0, where the cell reads 2. That is again `+0x9a8` into
  that run's 256 KiB-aligned code range. Cells and heap addresses differ between recordings and from
  native.
- **Synchronous TurboFan:** all 20 SPRR writes are on tid 0. Workers 2–5 ran only their start-up
  handshake and are Runnable, never rescheduled, at the crash.

**Against walls.md.** Agrees with §2 (walk 4) and `logs/w4.dbg`: the same marker, ESR, DFSC, thread,
`+0x9a8` store offset, cell values and cost (13.6 s, 4.2 GB). P10's 4.2 GB peak RSS is confirmed.

**Decision.** D1 holds as designed. No Ruling is needed: the pc is inside the `MAP_JIT` range.
**Halt considered:** none.

---

## M8. The base counts

**Command.** Brief Step 10.

```
$ grep -r -c -E '^\s*#\[test\]' crates --include='*.rs' | awk -F: '{s+=$2} END {print s}'
958
$ ls crates/*/tests/*.rs | wc -l
     145
$ grep -a -c 'test result:' …/2026-09-30-retrace-m47-gitwrite/regate-95bf845/gate-*.log | awk -F: '{s+=$2} END {print s}'
160
```

**Result.** 958 `#[test]` lines, 145 test files, 160 binaries, as expected. M47 closed at 950 + 10 =
960, the 958 lines plus the two `census.rs` tests compiled twice. 160 = 145 test files + 7 library
unit targets + the `retrace` bin + 7 doc-test targets.

**Decision.** The plan's baseline stands. Nothing to reconcile. **Halt considered:** none.

---

## M9. The SIMD fixture

**Command.** Brief Step 5. `simd_dyn.c` is the plan's Task 1 Step 5 text, byte for byte. It was
built with `clang -arch arm64`, run natively, then recorded on base, then on walk. t0 additions, to
explain the result:
- the same fixture on **baserel**;
- `simd_dyn_sa.c`, the fixture with its signal mode on `sigaction(SA_SIGINFO)`, on every binary;
- the plan's Task 1 Step 2 hv-sys test and Step 3 `simdctx` tests on base and fixed sources, in
  scratch builds.

**Result: `simd_dyn.c` as the plan writes it.**

| run | `thread` | `signal` |
|---|---|---|
| native | `simd thread intact`, rc 0 | `simd signal intact`, rc 0 |
| **base** (debug) | **`simd thread intact`, rc 0** | **rc 101**: `panicked at crates/retrace-box/src/sig.rs:300:5: a non-SA_SIGINFO handler is not modelled. Its infostyle is 0x1 …` |
| walk (release, fixed) | `simd thread intact`, rc 0 | rc 101, the same panic |
| baserel (release) | **`simd thread MISMATCH d8 got 0 want 0x123456789ab90ef`, rc 1** | rc 101, the same panic |

**The brief's expectation fails twice.**
1. **`signal` cannot run under retrace on any binary.** The fixture installs its handler with
   `signal()`, which is not `SA_SIGINFO`, and retrace asserts against that frame
   (`sig.rs:300`). This is a fixture defect, independent of the SIMD bug.
2. **`thread` is `intact` on base.** The bug does not show on a debug build. Disassembling base's
   `hv_sys::Vcpu::set_simd` (debug) shows why: before calling `hv_vcpu_set_simd_fp_reg` it executes
   `mov.d v0[0], x2; mov.d v0[1], x3` to spill its `u128` argument through `q0` to its stack slot,
   and nothing touches `v0` before the `bl`. **At opt-level 0 the bindgen call is correct by
   accident.** On release (baserel) the fixture shows the bug: `MISMATCH`, rc 1.

**Result: the `SA_SIGINFO` candidate `simd_dyn_sa.c`** (record, then replay; `m9sa-*`):

| binary | `thread` | `signal` |
|---|---|---|
| native | intact, rc 0 | intact, rc 0 |
| base (debug) | intact, rc 0; replay intact | intact, rc 0; replay intact |
| walk (release, fixed) | intact; replay intact | intact; replay intact |
| walkdbg (debug, fixed) | intact; replay intact | intact; replay intact |
| **baserel** (release) | **MISMATCH d8 got `0x27ff5a0`**, rc 1; replay the same | **MISMATCH d8 got `0x67b97aa2c264a479`**, rc 1; replay the same |
| **hvopt** (debug, hv-sys at opt-level 1) | **MISMATCH d8 got 0**, rc 1 | **MISMATCH d8 got 0**, rc 1 |

**Result: Task 1's red-first tests** (`m9-hvsys-*.log`, `m9-simdctx-*.log`):

| test | base, dev | base, dev + hv-sys opt-level 1 | base, `--release` | fixed, dev | fixed, dev + opt 1 | fixed, `--release` |
|---|---|---|---|---|---|---|
| hv-sys `set_simd_installs_the_passed_value_not_what_the_host_left_in_v0` | **ok** | FAILED (opt-level 2 and 3 likewise) | FAILED (`left` = all-ones, the poison) | ok | — | ok |
| `simdctx::the_debuggers_register_write_installs_its_value_in_every_q_register` | **ok** | **ok** | FAILED | ok | ok | ok |
| `simdctx::a_thread_switch_installs_the_incoming_threads_simd_registers` | **ok** | FAILED | FAILED | ok | ok | ok |
| `simdctx::a_checkpoint_restore_installs_the_captured_simd_registers` | **ok** | FAILED | FAILED | ok | ok | ok |

**Against walls.md.** walls.md §4 item 1 measured the bug on the **release** probe build. Its debug
walk 1 replayed clean (record 45 s, replay 46 s, rc 0), which is consistent with the debug build being
correct by accident. Neither walls.md nor the plan says the bug is release-only.

**Decision.** **A Ruling is needed before Task 1** (brief Step 5: "do not start Task 1 on a guard
that cannot fail"). The Decisions list proposes one. **Halt considered:**
- **H4** (a nondeterministic flake) does not fire. The defect is root-caused and deterministic per
  build profile.
- **H1** is not reached: no gate ran.

---

## Deviations from the brief

- **D1. `walk.sh` records `secs=`.** Step 9 asks for the record time, and the brief's script did not
  capture it. The only change is a `secs=` field on each status line.
- **D2. lldb could not launch a process.** `DevToolsSecurity -status` reports developer mode
  disabled, and the `system.privilege.taskport` right requires `authenticate-user` for group
  `_developer`, an interactive prompt. `lldb -b -o run` hangs until the alarm on any target
  (`m4-lldb-launchtest.out`: `kqprobe`, exit 142). M47's t0 met the same wall, and the operator ruled
  "fallbacks, no host security change" then. The same posture is taken here: no host setting was
  changed. Step 8's two lldb measurements were taken with `m4interpose.c` (the stub-level interposer
  above) and `alarmexec.c`, a non-SIP stand-in for `perl -e 'alarm N; exec @ARGV'`. `perl` is a
  SIP-protected platform binary, so dyld strips `DYLD_INSERT_LIBRARIES` from it and its children.
  The interposer sees exactly what the brief's breakpoints at stub entry and stub + 8 would have:
  the argument registers, and the raw `x0` plus carry.
- **D3. t0 additions beyond the brief**, each labelled in the evidence README:
  - the baserel, walkdbg and hvopt binaries;
  - `simd_dyn_sa.c` and the hv-sys/`simdctx` matrix (M9);
  - the walkdbg debug timing (M5);
  - the second crash recording for the thread (M7);
  - the `extract.sh`, `munmap-census.sh` and `thread-timeline.sh` extractors.

  Each extra scratch worktree (`m48-t0-hvopt`, `m48-t0-simdctx`) was created at `HEAD` and removed
  with `--force`, and the m48 worktree's `git status --short` was empty afterwards.
- **D4. `probe.patch` ≠ `probe-final.patch` byte for byte.** The controller reported them identical.
  They carry the same hunks, with the `thread.rs`, `retrace-core` and `machmsg.rs` diffs in a
  different order. `probe-final.patch` was used, as the brief says.
- **D5. Concurrent load.** Another project's `cargo test` (`charpente`) was running on the machine
  during Step 2 and the walks. The release timings are seconds-scale and match walls.md, so it is
  noted, not corrected for.

---

## Decisions

1. **The SPRR `msr` traps EC 0x00** (ISS 0), like the `mrs`, which reads 0 (R1). Task 6's arm goes
   in the EC 0x00 path beside `try_emulate_undef_mrs`. **Admitted values:** commpage `+0x110` =
   `0x2010002030300000` (write-enabled) and `+0x118` = `0x2010002030100000` (protected). They differ
   only in bit 21. Every native thread starts at `+0x118`, and a repeated protect is idempotent.
2. **SCTLR: set UCI (bit 26) only.** `ic ivau` traps EC 0x18, ISS `0x12dc6a` (op0 1, op1 3, CRn 7,
   CRm 5, op2 1, Rt 3). `sys_icache_invalidate` issues no `CTR_EL0` read and no `dc cvau`. UCT stays
   clear.
3. **kevent constants (Task 4 `T0(M3)`).**
   - `EVFILT_USER` returns the add flags (`0x21`), fflags 0, data 0. Change slot 1 is left untouched.
   - Pipe capacity is **16384**. Read-ready returns data = the byte count, flags `0x1`.
   - Write-ready returns data = 16384 − count (16381 after 3 bytes), flags `0x1`.
   - With the reader closed, both filters on the write end return `n=1`, data 0, flags = add flags |
     `EV_EOF` (`0x8005` read with `EV_ENABLE`, `0x8001` write).
   - `EVFILT_READ` with 1 ns on a pipe's write end or on fd 1 returns 0.
4. **psynch (Task 5 `T0(M4)`).**
   - `cvsignal` waking the last waiter → `0x101`. `cvbroad` to n → n × `0x100` | CBIT (`0x301` for 3).
   - A woken `cvwait` → 0. A timed-out lone `cvwait` → `0x13c` (ETIMEDOUT | ECVCLEARED), carry set.
   - The `cvwait` flags word is `0xa0`. `c_seq` = bytes 24–35 of the 48 (L, S, U).
   - The `{0, 1 ns}` origin is **libuv's `uv_cond_destroy`**, whose constant is `ts = {0, 1}`: 6 per
     full walk, and 6 natively.
   - Natively only, node also contends mutexes (`mutexwait`/`mutexdrop`, `cvwait` flags `0x10a0`);
     the walks never do.
5. **Partial `munmap` (Task 3).** Head and tail trims only (`e`: 25 head + 32 tail; `crash`: 23 + 24).
   - No interior split. No unaligned partial end. No partial range past its backing.
   - Lengths are `0x4000`–`0x3c000`.
   - Unaligned lengths (`0x10b20` …) are all whole-backing dyld unmaps, which the 16 KiB round-up must
     keep whole.
6. **`MAP_JIT` (Task 6).**
   - One 256 MiB `PROT_NONE` mmap, flags `0x41842`, not FIXED.
   - **One `mprotect(…, 0xffc0000, RWX)` starting at the first 256 KiB boundary past the base, not at
     `+0x40000`**, so a `PROT_NONE` head (`0x4000`–`0x34000` measured) and a `PROT_NONE` tail
     (`0x40000 − head`) remain.
   - REUSE/REUSABLE `madvise` over the committed range; a whole `munmap` at exit.
   - Toggles: 264–274 writes per full walk (20 in the crash demo), 0 switch flips.
   - Toggle cost: record 4 s release, **42.57 s debug** (replay 44.91 s). **R9 does not fire.**
7. **Crash demo (Task 8).**
   - Native marker `CRASHJS cell=… target=0x4000dead0000 rows=2 opt=101001`, rc 139.
   - The terminal crash is at the addon's `deref`, pc in its FIXED R-X text, FAR `0x4000dead0000`,
     ESR `0x92000005`, thread 0, after node's own SIGSEGV handler ran once.
   - `reverse-continue` to the cell lands at a pc inside the `MAP_JIT` RWX range (`+0x9a8` into the
     code range, both recordings), with the cell at 2, and at the target one `stepi` later.
   - Session cost: 13.63 s, 4.14 GB max RSS (4.20 GB peak footprint).
8. **SIMD fixture (Task 1).**
   - **On base (debug): `thread` intact, `signal` panics** (the non-`SA_SIGINFO` assert).
   - On walk (release, fixed): `thread` intact, `signal` panics.
   - On baserel (release, unfixed): `thread` MISMATCH.
   - The `SA_SIGINFO` candidate prints MISMATCH in both modes on baserel and on hvopt, and intact on
     base, walk and walkdbg.
9. **Base counts:** 958 `#[test]` / 145 test files / 160 binaries.
10. **New walls:** none (H5 does not fire). `getsockname` (32) is noted, not added: 0 calls with stdin
    `/dev/null`.

**Ruling T0-SIMD (proposed by t0 for the controller; brief Step 5 makes it a Ruling before Task 1).**
- **(a) The fixture's signal mode uses `sigaction` with `SA_SIGINFO`.** `simd_dyn_sa.c` in the
  evidence is the measured text. As planned, `signal()` stops retrace at `sig.rs:300` on every binary,
  so that mode can never print `intact`.
- **(b) The red-first steps need an optimised `hv-sys`.** At the gate's profile (dev, opt-level 0)
  the pre-fix binding is correct by accident, so every Task 1 guard, and every node gate, is green
  on the unfixed code. Measured levers:
  - **(b1)** Append `[profile.dev.package.hv-sys] opt-level = 1` to the workspace `Cargo.toml`. On
    base this turns the hv-sys test, two of the three `simdctx` tests and both `simd_dyn_sa` modes
    red on the debug binary, and the fixed source turns them green. It keeps the guard live in the
    gate itself, so a regression of the fix would fail `just gate`.
    `the_debuggers_register_write_installs_its_value_in_every_q_register` stays green on base under
    (b1). Its value reaches `set_simd` through debug code that leaves it in `v0`, so it can fail only
    under (b2).
  - **(b2)** Run Task 1's red steps with `--release`. All four tests are red on base and green on the
    fixed source. But the gate would not guard the fix afterwards.

  t0 recommends **(b1)**, with Task 1's red step also run once under (b2) for the debugger-write
  test. The controller should weigh the gate-time cost of an optimised `hv-sys`, which is a small
  crate and was not measured.
