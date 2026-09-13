# M34-destgaps — two `Dest` rows, one cited bound, and a corpus that never reaches either

**Charter entry:** `docs/superpowers/specs/2026-09-09-retrace-m32-m38-program-charter-design.md`
§3, "M34 — `destgaps`". Written autonomously from the charter under §5's authority, on the
operator's 2026-09-13 instruction to run the queue unattended through M38. §9's contract is met in
§1 (wall), §4 (measurement), §8 (symmetry), §6 (positive controls) and §7 (deliberately not done).

**Re-scoped by its own measurement, recorded here as the charter requires.** The charter says
"the three uncovered `dest_buffer` syscalls — `proc_info`, `getattrlist`/`fgetattrlist`, `csops`
— each needs its reply-length operand located and added". §4 locates all three and finds that one
of them has no reply-length problem to solve: the kernel bounds `getattrlist`'s write at 15,360
bytes before writing a byte of it, which under the table's own membership rule is a `Ptr` with a
citation, not a `Dest`. So this milestone adds **two** `Dest` rows and **cites one bound**, and
the README's "three still get a flat 64 KiB" becomes zero by two different mechanisms.

> **Ruling 1: re-scoped M34 — the premise that `getattrlist`/`fgetattrlist` (220/228) can overrun
> the 64 KiB window was contradicted by xnu source: both entry points reach
> `getattrlist_internal` → `getvolattrlist` / `vfs_attr_pack_internal`, each of which rejects
> with `ENOMEM` before any copyout when the packed result exceeds `attr_max_buffer`
> (`ATTR_MAX_BUFFER_LONGPATHS` = 8192 − 1024 + 8192 = 15,360; `bsd/vfs/vfs_attrlist.c`, the
> `ab.allocated > attr_max_buffer` gates, and the copy `lmin(buf_size, ab.allocated)` /
> `ulmin(bufferSize, ab.needed)`) — new scope for those two rows: stay `Ptr`, cite the bound,
> pin the decision with a test.**

## 1. The wall, located

`crates/retrace-arch/src/lib.rs`, `arg_kinds`, at `e194b68`:

| row | line | today | comment says |
|---|---|---|---|
| `proc_info` 336 | 786 | `[Scalar, Scalar, Scalar, Scalar, Ptr, Scalar]` | "buffer is M34's destination to widen; Ptr until measured" |
| `csops` 169 | 764 | `[Scalar, Scalar, Ptr, Scalar]` | "M34's destination to widen; Ptr until measured" |
| `csops_audittoken` 170 | 765 | `[Scalar, Scalar, Ptr, Scalar, Ptr]` | (shares 169's comment) |
| `getattrlist` 220 | 706 | `[Path, Ptr, Ptr, Scalar, Scalar]` | "M34's row to widen (spec §7), Ptr until measured" |
| `fgetattrlist` 228 | 503 | `[Fd, Ptr, Ptr, Scalar, Scalar]` | "M34's row to widen (spec §7); Ptr until then" |

and the `ArgKind::Dest` doc comment (line 236–241), which names the same three as "still
structurally capable of overrunning … `Ptr` there means 'not measured' … M34 is to measure each".

What a `Ptr` costs at those rows is two things `Dest` provides and nothing else does
(`crates/retrace-box/src/lib.rs`):

- **the diff window** — `Box_::diff_window` (line 3082) takes `max(min(avail, window_cap),
  clamp_count(avail, len))` when `dest_len_bytes` (line 3062) knows `len`, and the flat
  `window_cap` (64 KiB) otherwise. A kernel write past the flat window is the M26 truncation
  class: captured by no `Event`, restored stale on replay, invisible to the `(num, args)` oracle.
  The M27/M30 guard band *detects* that class; it does not prevent it.
- **the forwarded-count clamp** — `forward_and_diff`'s `DestLen::Reg(li)` arm (line 3286 at
  `adb0402`; 3298 once the fix wave's `RETRACE_REGCLAMP` comment landed above it) rewrites
  `hargs[li]` to `clamp_count(avail, count)`, so the host kernel is never told a length larger
  than the guest backing behind the destination. README, M27: "the missing clamp was the
  serious half, since an unclamped forward lets the host kernel write past the guest's actual
  backing".

The README's claim to discharge is at `README.md:452` ("**Three still get a flat 64 KiB**:
`proc_info` (336); `getattrlist`/`fgetattrlist` (220/228); `csops` (169/170)") and the owed-list
entry at `README.md:602`.

## 2. Scope

Two rows widened, two rows' comments rewritten to cite a bound, one test that pins all four
decisions through the same `diff_window` production code path M29 used and one that sees the
clamp fire through the kernel's own return value, three `EXPECTED_DIFFS` entries, the measurement script committed so the next milestone does not lose its census the way
M33's raw outputs were lost, and the docs. No new guest, no new trap arm, no format change.

## 3. Design

### 3a. The rows after M34

| syscall | row after | length operand |
|---|---|---|
| `proc_info` 336 | `[Scalar, Scalar, Scalar, Scalar, Dest(Reg(5)), Scalar]` | `x5` = `buffersize` (`uint32_t`) |
| `csops` 169 | `[Scalar, Scalar, Dest(Reg(3)), Scalar]` | `x3` = `usersize` (`user_size_t`) |
| `csops_audittoken` 170 | `[Scalar, Scalar, Dest(Reg(3)), Scalar, Ptr]` | `x3` = `usersize`; `x4` stays the 32-byte audit-token copyin |
| `getattrlist` 220 | **unchanged** `[Path, Ptr, Ptr, Scalar, Scalar]` | — (bound cited, Ruling 1) |
| `fgetattrlist` 228 | **unchanged** `[Fd, Ptr, Ptr, Scalar, Scalar]` | — (bound cited, Ruling 1) |

**`Reg`, not `DerefU64`, and clamp, not refuse.** Every length here is a pure in-value: the kernel
reads it and never writes it back. M29 refused rather than clamped `sysctl` because `*oldlenp` is
*in-out* — clamping it would hand the guest a truncated reply it could not detect. Nothing of the
kind applies to `buffersize` or `usersize`, so these take `read`'s posture (`Dest(Reg(n))`, the
clamp arm), exactly as `getdirentries64`, `recvfrom` and `getfsstat64` did in M29.

### 3b. Why `proc_info` and `csops` are `Dest` and `getattrlist` is not — the rule applied

The `ArgKind` doc states the rule: `Ptr` is "a write the kernel itself bounds far inside the
window … the row comment names the bound and its citation. A bound that cannot be cited is not a
bound." Applying it, from xnu `main` as fetched 2026-09-13:

**`proc_info` (336)** — `bsd/kern/proc_info.c`, `proc_info_internal`, dispatching on `callnum`
(`bsd/sys/proc_info_private.h:274–293`):

- `PROC_INFO_CALL_LISTPIDS` (1): `copyout(ptr, buffer, n * sizeof(int))` with `n ≤ buffersize /
  sizeof(int)` and `n ≤ numprocs`. Bounded by `buffersize` and by the process count — a tunable
  (`kern.maxproc`), not a citable constant.
- `PROC_INFO_CALL_KERNMSGBUF` (4): the kernel message buffer up to `buffersize`; the buffer's size
  is `kern.msgbuf`, tunable.
- `PROC_INFO_CALL_LISTCOALITIONS` (11), `PIDDYNKQUEUEINFO` (13), `UDATA_INFO` (14): list results
  bounded by `buffersize` and a count the kernel owns.
- `PROC_INFO_CALL_PIDINFO` (2) / `PIDFDINFO` (3) / `PIDFILEPORTINFO` (6) / `PIDORIGINATORINFO`
  (10): fixed structs, each arm `if (buffersize < size) return ENOMEM` then `copyout(…, size)`;
  `PROC_PIDPATHINFO` is `MAXPATHLEN`-bounded. All inside the window, and all ≤ `buffersize`.
- `PROC_INFO_CALL_SETCONTROL` (5), sub-op `PROC_SELFSET_THREADNAME`: a **copyin** of `buffersize`
  bytes, rejected above `MAXTHREADNAMESIZE − 1` (63). A `Source` shape, bounded 1000× inside the
  window, so the row carrying `Dest` costs no canary coverage (the canary is skipped only for
  `Source`/`NestedSource` rows, and the kernel cannot reach the band through a 63-byte read).
- `PROC_INFO_CALL_SET_DYLD_IMAGES` (15): `proc_set_dyld_images` — quoting the source: *"don't
  need to copyin the buffer. just setting the buffer range in the task struct"*
  (`task_set_dyld_info(task, buffer, buffersize, false)`). **No transfer in either direction.**

So the write is bounded by nothing citable below the window for callnums 1, 4, 11, 13, 14 →
`Dest(Reg(5))` by the rule. The row is per-syscall, so callnums 5 and 15 ride under the same
`Dest` as an **over-approximation**: the window widening is to ≤ 368 bytes, inside the flat
window regardless, and the clamp `min(avail, buffersize)` fires whenever the destination's
backing ends within `buffersize` of it. *(Corrected by the fix wave. This paragraph originally
said the clamp "can only fire when the guest's buffer already overruns its own backing" — false
on measurement: on every dynamic guest, 76 of 76, callnum 15's destination `0x1ec6f7f80` sits
`0x3f80` into a shared-cache page whose backing is that one 16 KiB page (`page_in_cache`, one
`Backing` per fault), so `avail` is 128 against `buffersize` 368 and the arm rewrites the
forwarded length 368 → 128. Measured with the channel the fix wave added,
`RETRACE_REGCLAMP=1`, on `hello_dyn` and `jq --version`: `[M34 REGCLAMP] syscall 336 count 368
avail 128 dest 0x1ec6f7f80 backing [0x1ec6f4000,0x1ec6f8000)`, once each. That is M29's "R1"
case — a buffer that legitimately continues into the next backing — and for a cache-DATA
destination it is the normal geometry, not a pathology. Harmless for this call, because it
transfers nothing and is rejected before the size is read, §4b.)* Documented on the row; not
modelled per-callnum, because the table cannot express that and nothing needs it to.

**`csops` (169) / `csops_audittoken` (170)** — `bsd/kern/kern_proc.c`, both wrappers call
`csops_internal(pid, ops, useraddr, usersize, uaudittoken)`, dispatching on `ops`
(`bsd/sys/codesign.h:41–61`):

- `CS_OPS_ENTITLEMENTS_BLOB` (7), `CS_OPS_BLOB` (10), `CS_OPS_DER_ENTITLEMENTS_BLOB` (16):
  `csops_copy_token(start, length, usize, uaddr)` — copies `length` bytes if `usize ≥ length`,
  else an 8-byte header and `ERANGE`. **≤ `usersize`, and `CS_OPS_BLOB` is the whole code-signing
  SuperBlob** — a CodeDirectory carries one hash per page of the binary, so a large binary's blob
  is hundreds of KiB. No citable bound below the window.
- `CS_OPS_IDENTITY` (11), `CS_OPS_TEAMID` (14): an 8-byte header plus the identity, `ERANGE` if
  `usize` is short. ≤ `usersize`.
- `CS_OPS_STATUS` (0): `copyout(&retflags, uaddr, sizeof(uint32_t))`, **no `usersize` check** —
  4 bytes regardless of what the caller passed. `CS_OPS_VALIDATION_CATEGORY` (17): the same shape,
  4 bytes. `CS_OPS_PIDOFFSET` (6): 8 bytes. `CS_OPS_CDHASH` (5) / `CS_OPS_CDHASH_WITH_INFO` (18):
  `usize` must equal the struct size or `EINVAL`.
- Everything else (`MARK*`, `SET_STATUS`, `CLEAR*`): no copyout.

So `Dest(Reg(3))` by the rule, on the blob arms. The fixed arms that ignore `usersize` write 4 or
8 bytes — inside the 64 KiB floor `diff_window` keeps under every `Dest` (`base.max(…)`), so a
`CS_OPS_STATUS` with `usersize = 0` is still fully captured. Named on the row so nobody later
"fixes" the floor away for `Dest` rows and silently loses those 4 bytes.

**`getattrlist` (220) / `fgetattrlist` (228)** — `bsd/vfs/vfs_attrlist.c`. `getattrlist` (3577)
and `fgetattrlist` (3486) both call `getattrlist_internal` (3250), which dispatches to
`getvolattrlist` (992) for volume attributes or `vfs_attr_pack_internal` (2817) otherwise. Both
packers compute `ab.allocated = fixedsize + varsize` and then

```c
if (((size_t)ab.allocated) > attr_max_buffer) {
    error = ENOMEM;
    goto out;
}
```

**before allocating or writing anything**, where `attr_max_buffer` is `ATTR_MAX_BUFFER` (8192,
`bsd/sys/attr.h:134`) or, for a long-paths process, `ATTR_MAX_BUFFER_LONGPATHS` = `8192 −
MAXPATHLEN + MAXLONGPATHLEN` = 8192 − 1024 + 8192 = **15,360** (`attr.h:140`;
`MAXLONGPATHLEN` 8192 from `bsd/sys/syslimits.h:137`, already cited by this table's `fsgetpath`
row). The user copy is then `ulmin(bufferSize, ab.needed)` (getvolattrlist, 1686) or
`lmin(buf_size, ab.allocated)` (vfs_attr_pack_internal, 3108) — never more than `ab.allocated`.
The kernel therefore never writes more than 15,360 bytes through `attributeBuffer`, whatever
`bufferSize` says. That is the `Ptr` rule's case exactly, with a bound four times inside the
window. `Dest` here would not be wrong so much as false to the table's own vocabulary: a reader
would take it to mean "the caller's length is the only bound", which is untrue and which §4 shows
no guest has ever needed. Ruling 1.

### 3c. What consults the rows, and what does not

`dest_buffer(num)` has two consumers, both in `forward_and_diff`'s path and both record-side:
`Box_::dest_len_bytes` → `diff_window` (the capture extent) and the `DestLen::Reg` clamp arm
(the forwarded length). The canary decision (`reads_guest_buffer`) does not consult `Dest`.
Replay's `apply_and_return` applies whatever the record side captured; it never consults the table
for a length. §8.

## 4. Measurement — taken before any edit, and what it found

**Corpus** — the M33 census corpus, re-run 2026-09-13 because M33's raw per-guest outputs lived
under a since-deleted worktree's gitignored `.superpowers/` and did not survive (the risk M33's
own "what stays owed" named): every Mach-O in the `retrace-guest` `OUT_DIR` (static via `record`,
dynamic via `record-dyn`, classified by `LC_LOAD_DYLINKER`), `jq --version`, `jq .name <rung3
fixture>`, the CPython interpreter and its launcher with `-c 'print(1)'`, and all 54 of
`tools/apple-sweep-binaries.txt`, bare argv, stdin `/dev/null`, 30 s watchdog. Instrument:
`RETRACE_TRACE=1`'s `[trap] num=… args=[x0…x5]` line, record side, filtered to the five numbers —
`x5` and `x3` are printed, so no dedicated probe was needed. Script: `tools/destgaps-census.sh`
(committed by this milestone, §5e), summarised by `tools/destgaps-census-summary.py`.

**Coverage**: 118 guests dispatched (the first draft said 115 — the first pass's progress-log count;
the three-binary rerun added `yes`/`true`/`printenv`); 76 issued at least one of the five (every dynamic guest —
the calls are libSystem/dyld init-time), 42 issued none (the static `-nostdlib` asm guests, and
three that fault before their first syscall). 851 matching dispatches.

**Result — the largest length operand in the whole corpus is 1,052 bytes.** No dispatch comes
within two orders of magnitude of the 64 KiB window:

| syscall | dispatches | guests | op / flavor | length | who |
|---|---|---|---|---|---|
| `proc_info` 336 | 318 | 76 | callnum 2 `PIDINFO`, flavor 13 `PROC_PIDT_SHORTBSDINFO` | 64 = `sizeof(proc_bsdshortinfo)` | every dynamic guest |
| | | | callnum 2, flavor 17 `PROC_PIDUNIQIDENTIFIERINFO` | 56 | every dynamic guest |
| | | | callnum 5 `SETCONTROL`, flavor 2 `PROC_SELFSET_THREADNAME` | 4 (the name `main`, a copyin) | the 9 Rust guests |
| | | | callnum 15 `SET_DYLD_IMAGES` | 368 (no transfer) | every dynamic guest, from dyld at `pc=0x14000600c` |
| `getattrlist` 220 | 191 | 76 | — | 12, 64, 1036; **1052 max** (CPython) | every dynamic guest |
| `fgetattrlist` 228 | 140 | 76 | — | 12, 40 | every dynamic guest |
| `csops` 169 | 126 | 76 | op 0 `CS_OPS_STATUS` | 4 | every dynamic guest |
| | | | op 16 `CS_OPS_DER_ENTITLEMENTS_BLOB` | 1032 | 11 guests, 22 dispatches: nine Apple binaries (`bash`, `date`, `launchctl`, `zsh`, `automationmodetool`, `dddiagnose`, `desdp`, `dyld_info`, `flex`) and both CPython invocations, two each — the first draft said "2 (an Apple binary, CPython)", the summariser's collapsed family count |
| `csops_audittoken` 170 | 76 | 76 | op 16 `CS_OPS_DER_ENTITLEMENTS_BLOB` | 1032 | every dynamic guest |

**The raw-vs-libc check M29 taught** passes for all five: the register the row names carries a
byte count, verified against the SDK by `sizeof` — `proc_bsdshortinfo` = 64 matches flavor 13's
`x5`; `attrlist` = 24 is the `alist` copyin, not the length; `CS_OPS_STATUS`'s `x3` = 4 =
`sizeof(uint32_t)`.

**What the table does not show — the destinations, measured in the fix wave.** The table
measured lengths against the window; the clamp compares a length against the *backing* behind
the destination, which nothing above measured. From the raw census, every `csops` /
`csops_audittoken` destination (126 + 76) and 242 of the 318 `proc_info` destinations are on the
dyn stack `[0x27C0000, 0x2800000)` and fit their backing; the 76 `SET_DYLD_IMAGES` destinations
are all `0x1ec6f7f80` — `0x3f80` into a shared-cache page, whose backing is that one 16 KiB page
— so `avail` = 128 < 368 and the clamp rewrites the forwarded `buffersize` on every dynamic guest
(§3b). Confirmed through the `RETRACE_REGCLAMP=1` channel on `hello_dyn` and `jq --version`: one
`[M34 REGCLAMP] syscall 336 count 368 avail 128 dest 0x1ec6f7f80 backing
[0x1ec6f4000,0x1ec6f8000)` each, and `REGCLAMP-FIT` for every other 336/169/170 dispatch.

### 4b. A finding outside this milestone's scope, measured and routed rather than fixed

While designing control 3 (§6), reading what the recording says the kernel *returned* for these
calls showed that on this machine every one of them fails. `hello_dyn`, recorded 2026-09-13 with
retrace at pid `0x6a30` (27184), read back with a throwaway trace dumper (not committed):

| landmark | call | `ret` | `err` | natively |
|---|---|---|---|---|
| #24 | `proc_info(15 SET_DYLD_IMAGES, pid, …, 368)` | 22 `EINVAL` | true | 0 — but **not pid-caused**; see below |
| #145 | `csops(pid, 0 STATUS, …, 4)` | 3 `ESRCH` | true | 0 |
| #166, #226, #228 | `proc_info(2 PIDINFO, pid, 17/13, …)` | 3 `ESRCH` | true | 56 / 64 |
| #233 | `csops_audittoken(pid, 16, …, 1032)` | 3 `ESRCH` | true | 0 |

**Cause, located — for the `ESRCH` rows:** `forward_and_diff`'s per-register probe
(`crates/retrace-box/src/lib.rs:3188–3189`, `for i in 0..8 { match self.host_span(args[i]) …
hargs[i] = hp as i64 }`) treats *any* register whose value lands inside a backing as a pointer
and hands the host kernel the host address in its place. The pid is `x0` of `csops` and `x1` of
`proc_info`; on the dynamic path the backings at `TRAMPOLINE_IPA` `[0x4000, 0x8000)`, `PT_L2_IPA`
`[0x8000, 0xC000)` and `PT_L1_IPA` `[0xC000, 0x10000)` are contiguous, so **every retrace pid in
16384..=65535 is rewritten to a host pointer** before forwarding — roughly half the pid space, a
coin flip per record run. The kernel then sees a pid that is not this process (`ESRCH`) for #145,
#166/#226/#228 and #233. This is the "non-pointer whose value collides with a mapped IPA" case
`truncguard.rs` (the comment above `a_band_with_no_neighbours_keeps_its_full_length`) names as a
known hazard, with "the dyld pread-count case" as its precedent; M33's `ArgKind` doc says of it:
"`Scalar`, `Path` and `Ptr` change nothing at runtime — `forward_and_diff` probes `host_span` on
all eight registers regardless — and are documentation until a later milestone consults them."

**#24's `EINVAL` is a different cause, and not pid-shaped** *(corrected by the fix wave; the
first draft attributed it to the same probe, via `proc_set_dyld_images`' `pid != proc_getpid`
check)*. `proc_set_dyld_images` calls `task_set_dyld_info(task, buffer, buffersize, false)` on
the **calling** task — retrace's — and xnu `osfmk/kern/task.c` says of that function: *"called
at most three times. 1) at task struct creation … 2) in mach_loader.c … 3) is from dyld itself …
For security any calls after that are ignored"*: a non-zero-over-non-zero update sets
`TF_DYLD_ALL_IMAGE_FINAL`, and every later call returns `KERN_FAILURE`, which
`proc_set_dyld_images` turns into `EINVAL`. Retrace's *own* dyld made call 3 on retrace's task at
retrace's startup, so the task is final before any guest runs, and a forwarded
`proc_info(15, …)` returns `EINVAL` with the correct pid and with any size. Measured with a plain
dynamically-linked process (`sdi.c`, `__proc_info(15, getpid(), 0, 0, buf, 368)` after its own
dyld registered): pid 67548 = `0x107dc`, *outside* the collision range → `ret=-1 errno=22`; with
size 128 → `EINVAL`; with a wrong pid → `EINVAL`. So after the `Scalar`-audit fix below, #24
will still read `EINVAL` — a fix milestone using this table as its symptom list must not chase
it. The right treatment is different in kind: the forwarded call names *retrace's* task, and if
it could succeed it would point retrace's own dyld info at guest memory, so `SET_DYLD_IMAGES`
should be **serviced above the trace** (synthesise `0`; dyld ignores the return either way)
rather than forwarded — the same family as every "on self" `proc_info`/`csops` being answered
about retrace's process rather than the guest's. Owed, not taken here.

**Why it is not fixed here.** The fix is that later milestone: skip the probe for positions the
row says are `Scalar`. Its blast radius is every `Scalar` in the table, and M33 §7 states that
`Scalar`-vs-`Ptr` "is not verified by anything but the reviewer" — so the fix's precondition is a
`Scalar` audit with its own measurement, which is a milestone, not an edit inside this one. Under
charter §5 that is scope this spec does not cover, so it is **recorded, not taken**.

**Why it is not a halt.** Record and replay agree (replay applies the recorded `ESRCH`), so it is
not a class-E2 flake of the oracle; it is a record-vs-native fidelity defect whose *presence*
depends on the recorder's pid. It does not change §4's numbers — the `[trap]` line prints the
guest's registers before translation, so every length above is what the guest asked for.

**Routed to M36.** A sweep row whose outcome depends on which half of the pid space the record
run drew is exactly the kind of row M36's `root_cause_class` must be able to name, and this is a
concrete, testable hypothesis for the README's one *intermittent* failure: M36's measurement
should record the recorder's pid beside each row. Also owed, for the fix milestone: the
`hello_dyn` numbers above are its positive control (after the fix, #145 returns 0 with 4 bytes
captured).

**What this means for the milestone, stated plainly.** On this corpus, neither new `Dest` row
changes a single *recorded* byte: every destination already fits the flat window, so the widening
half is inert today, exactly as M32's per-argument fill was measured inert. The half that is not
inert is the **clamp**, and — corrected by the fix wave — it is not merely structural: it
rewrites the forwarded length at #24 on every dynamic guest (368 → 128, the cache-page boundary,
§3b/§4), with no effect on any byte because that call transfers nothing and is rejected before
the size is read. Beyond #24 it is structural: after M34 a guest that passes `proc_info` or
`csops` a `buffersize`/`usersize` larger than its buffer's backing has the forwarded length
clamped to that backing, where before the host kernel would have been told the guest's number and
written past it in retrace's own process. That is the M27 "serious half", and it is the value
this milestone delivers; the window half is the *correctness by contract* that makes the README's
sentence true rather than an open item. Neither is a fix to a reproduced bug, and the ledger says
so.

## 5. What must change

### 5a. `crates/retrace-arch/src/lib.rs`

- Rows 336, 169, 170 as in §3a. Each row's comment replaced with the §3b analysis for that
  syscall in a form the M29 rows set: the callnums/ops that bound the write only by the caller's
  length (why `Dest`), the fixed-size arms and the floor that covers them, and for `proc_info` the
  two callnums riding as an over-approximation (5, a ≤63-byte copyin; 15, no transfer) with the
  measured `pc`/size for 15.
- Rows 220, 228: shape unchanged; comment replaced with Ruling 1's citation — the two packers, the
  `ENOMEM` gate, `ATTR_MAX_BUFFER_LONGPATHS` = 15,360, the corpus maximum 1,052 — in the shape of
  the `fsgetpath` row's comment (a cited constant, far inside the window).
- `ArgKind::Dest`'s doc comment, the paragraph beginning "Other syscalls are still structurally
  capable of overrunning": rewritten to say M34 measured all three, added two, and re-classified
  one; the `Ptr` sentence ("`Ptr` there means 'not measured'") deleted, because after M34 no `Ptr`
  in the table means that.

### 5b. `crates/retrace-arch/tests/legacy_equivalence.rs`

Three `EXPECTED_DIFFS` entries, `(336, View::DestBuffer, …)`, `(169, …)`, `(170, …)`, each saying
"exercised by the census" (all three are in `census.rs`; `exercised_and_unexercised_match_the_census`
fails an entry that says "unexercised" for a census number) and naming the corpus maximum so the
entry records that the widening was inert on landing. No entry for 220/228 — their view is
unchanged, and a spurious entry would fail as stale.

### 5c. `crates/retrace-box/tests/truncguard.rs`

One new test beside M29's, through `Box_::diff_window_for_test` with `AVAIL = 1 << 20` and a
length above the flat cap, so it exercises the production `diff_window` on a real `Box_` without
a guest per syscall:

- `336` at index 4 with `args[5] = 200_000` → `200_000`; at index 5 (the length, not the buffer)
  → `FLAT`.
- `169` and `170` at index 2 with `args[3] = 150_000` → `150_000`; `170` at index 4 (the audit
  token) → `FLAT`.
- **Ruling 1 pinned:** `220` and `228` at index 2 with `args[3] = 150_000` → `FLAT`. This is the
  assertion that turns the ruling from prose into a red bar: a later reader who "finishes M34" by
  making `getattrlist` a `Dest` fails here and is sent to the citation.

### 5d. Docs

- `README.md:452` — the sentence "**Three still get a flat 64 KiB**: …" replaced with what is
  true: `proc_info` and `csops`/`csops_audittoken` widened and clamped, `getattrlist`/
  `fgetattrlist` kernel-bounded at 15,360 and cited, corpus maximum 1,052, both new rows inert on
  the corpus and live for the clamp. `README.md:602` — the owed-list entry "M34's three `Dest`
  rows" removed.
- `docs/status-log.md` — a new `## Status: M34-destgaps` section, append-only, with Ruling 1, the
  §4 table, the gate, and "what stays owed".
- This spec's §11 — the outcome, after the gate.

### 5e. `tools/destgaps-census.sh` and `tools/destgaps-census-summary.py` (new)

The §4 instrument, committed. The script carries one lesson from its own first run, in its header:
under `RETRACE_TRACE=1` a stdout-flooding guest (`/usr/bin/yes`) floods stderr, **measured at 5 GB
in the 30 s before the watchdog fired**, so the script streams stderr through a line-capped filter
(`head -n 400000`; the recorder dies on `SIGPIPE`) rather than into a file, and its watchdog kills
by command pattern because `$!` of a backgrounded pipeline is the filter, not the recorder. It is
not a test and runs in no gate; it exists so the next milestone that needs this census can re-run
it instead of rediscovering it.

## 6. Positive controls

Each is run, its red output recorded in the task report, and the mutation reverted before commit.

1. **The rows are wired to the window.** Revert 336 to `Ptr`. The new truncguard test must go RED
   on the `proc_info` assertion, **and** `legacy_equivalence::every_view_reproduces_its_legacy_table`
   must go RED naming `(336, DestBuffer)` as a *stale* `EXPECTED_DIFFS` entry — two independent
   detectors, one through the box's production path and one through the table's own ledger. Repeat
   for 169 (one row is enough to prove the pair; the report says which).
2. **The ruling is enforced, not just written.** Change 220's row to `[Path, Ptr, Dest(Reg(3)),
   Scalar, Scalar]`. The truncguard test must go RED on the `getattrlist` assertion (`FLAT`
   expected), and `every_view_reproduces_its_legacy_table` must go RED naming `(220, DestBuffer)`
   as an *unlisted* difference. Both, for the same reason as control 1.
3. **The clamp arm reaches the new rows.** This is the half §4 calls live, and it must be seen to
   fire once. No seam exposes `hargs` (it is local to `forward_and_diff`; `tests/clamp.rs` proves
   `clamp_count` as a pure function and nothing proves the `Reg` arm reaches a given row), so the
   control observes the clamp through the **kernel's return value**, the way `memdiff.rs`'s
   `forward_and_diff_captures_a_read_larger_than_the_window` observes the window through `ret`.
   The test loads a static guest (`HELLO`), runs to its first `Stop::Syscall`, and — instead of
   forwarding that call — calls `b.forward_and_diff(336, args)` with args of its own:
   `[1 /*PROC_INFO_CALL_LISTPIDS*/, 1 /*PROC_ALL_PIDS*/, 0, 0, dest, avail + 4096, 0, 0]`, where
   `dest = STACK_TOP_IPA - 64` so that `host_span_for_test(dest)` gives `avail = 64` (the static
   stack backing is `[STACK_TOP_IPA − GRANULE, STACK_TOP_IPA)`), asserted as a precondition.
   `proc_listpids` copies out `min(numprocs, buffersize / 4)` pids and returns the byte count
   (`bsd/kern/proc_info.c`); any Mac runs far more than 16 processes, so with the clamp the kernel
   is handed `buffersize = 64` and **returns exactly 64 with `err == false`**; without it the
   kernel is handed 4160 and either writes past the backing (`ret > 64`) or faults on the copyout
   (`err`, `EFAULT`). The assertion `(ret, err) == (64, false)` is satisfied by the clamp and by
   nothing else. `LISTPIDS` is chosen over `PIDINFO` precisely because of §4b: it takes no pid,
   so the control cannot be confounded by the probe rewriting one. The test also asserts
   `host_span_for_test(0)` and `host_span_for_test(1)` are `None` — the scalar arguments must not
   themselves collide with a backing, or the control would be measuring §4b instead. Mutation:
   revert 336 to `Ptr`; the test must go RED with `ret`/`err` printed.

## 7. What this milestone deliberately does not do

- **No guest fixture.** No flavor of `proc_info` or op of `csops` that any corpus guest issues can
  be made to write more than 64 KiB without faking the guest (a `CS_OPS_BLOB` over 64 KiB needs a
  binary with >2,000 pages; no repo guest is that large, and adding one for the purpose would be a
  `bigread`-shaped fixture whose only content is its size). The window half is proven through
  `diff_window_for_test`, as M29's rows were; the clamp half through control 3. A `bigcsops` guest
  is named as owed only if a later milestone finds a real guest in that regime.
- **No per-callnum or per-op modelling.** The table is per-syscall; `proc_info`'s callnums 5/15
  and `csops`'s fixed-size ops ride under `Dest` as documented over-approximations (§3b).
- **No per-argument canary fill.** Unchanged from M33 §7; still inert, still owed.
- **No change to the refuse posture.** `sysctl`'s `DerefU64` refusal is untouched; nothing here
  is in-out.
- **No `getattrlistbulk` (461) or `getattrlistat` (468) row.** Neither is in the census.
- **M35's two holes untouched** — the replay-side `.min(avail)` and the `if !err` gate.
- **The §4b pid-collision defect is not fixed.** Recorded with its measurement, its located cause,
  its fix shape and its positive control; routed to M36's table and to a fix milestone with a
  `Scalar` audit as its task 1. Fixing it here would be scope this spec's charter entry does not
  cover.

## 8. Symmetry obligation

No trap-handling arm is added or changed. `dest_buffer` is a pure function of `num` consulted only
inside `forward_and_diff` (`diff_window` and the clamp arm), which replay never calls — the same
record-only-by-construction argument M33 §8 made. The two effects are (a) how many bytes the
record side *looks at* after the syscall, and (b) the length the *host kernel* is handed. Neither
touches a recorded byte on this corpus (§4: every destination already fits the flat window, so the
capture extent is unchanged for every call actually made), and a call outside the corpus that
does exceed the window is captured *more completely* — replay applies what was captured, so there
is no replay mirror to keep in step. `TRACE_MAGIC` untouched. The forwarded-length clamp changes
what the host kernel sees, not what is recorded — M10's contract, as M33 §8 said of `translate_fds`.

## 9. Rulings

- **Ruling 1** (top of this document): `getattrlist`/`fgetattrlist` re-scoped from "widen" to
  "cite the bound", on xnu source. Not a halt — a re-scope, recorded loudly, per charter §5.
- **Ruling 2:** `proc_info`'s `Dest` over-approximates callnums 5 and 15 (§3b). Accepted because
  the alternative — a per-callnum kind — is a schema change the charter's queue does not authorise
  and nothing measured needs.
- **Ruling 3:** the §4b pid-collision defect — every `csops`/`proc_info(PIDINFO)` in the corpus
  failing `ESRCH` whenever the recorder's pid is in 16384..=65535 (the `SET_DYLD_IMAGES` `EINVAL`
  is a separate, pid-independent cause, §4b) — is recorded and routed, not fixed, because its fix's precondition (a `Scalar` audit of the whole table) is a milestone the
  charter does not contain. Not a halt: record and replay agree, so it is not an E2 flake; it is
  a fidelity defect with a pid-shaped trigger, which is M36's business to classify.

## 10. Gate

The full chunked gate, `--bins` included, `cargo test -p retrace-arch --doc` and `-p retrace-box
--doc` beside any per-target split, counts reconciled file-by-file against **570 / 0 / 2 over
124**. Expected: **+2 tests in `truncguard.rs`** (the widening/ruling test, and control 3's clamp
test), **0 new binaries** → **572 / 0 / 2 over 124**. The two `#[ignore]`s are untouched.
The Apple sweep is re-run once after the change and its tally recorded; §4 predicts it unmoved at
`pass=46 fail=8` — no recorded byte changes on any corpus call. *(The first draft said "no corpus
call is in the regime the rows change", which is false: the clamp rewrites the forwarded length
at #24 on every dynamic guest, §3b/§4. The prediction stands for the reason that is true — that
call transfers nothing and is rejected before the size is read, so nothing recorded moves.)*

## 11. Outcome

**Landed as designed.** The two `Dest` rows of §3a — `proc_info` 336 `Dest(Reg(5))`, `csops` 169
and `csops_audittoken` 170 `Dest(Reg(3))` — with §3b's kernel-source analysis on each row;
`getattrlist` 220 and `fgetattrlist` 228 unchanged in shape with Ruling 1's bound cited (15,360,
`ATTR_MAX_BUFFER_LONGPATHS`) and their corpus maxima (1,052 and 40); the `ArgKind::Dest` doc
paragraph rewritten with the "`Ptr` means 'not measured'" sentence gone; two tests in
`crates/retrace-box/tests/truncguard.rs` —
`the_window_widens_for_the_m34_rows_and_not_for_getattrlist`, which pins all four decisions
including Ruling 1 through the production `diff_window`, and `the_clamp_reaches_proc_info`,
control 3; three `EXPECTED_DIFFS` entries (336, 169, 170, each "exercised", none for 220/228); and
the §4 instrument committed as `tools/destgaps-census.sh` and `tools/destgaps-census-summary.py`
with the 5 GB stderr lesson in its header. Commits `3e05664`, `e3ec90a`, `486bab2`, plus the docs
commit. No trap arm, no guest, no `TRACE_MAGIC` bump, no `retrace-core` or `retrace-box/src` edit
— §8's symmetry argument held with nothing to mirror. §4b was recorded and routed, not fixed, per
Ruling 3; the README's owed list now carries it.

**What the controls showed.** All three fired as §6 predicted, each red quoted in its task report.
Control 1 (336 → `Ptr`): the window test red at `left: 65536, right: 200000` and the ledger red
naming `[(336, DestBuffer)]` as stale. Control 2 (220 → `Dest(Reg(3))`): the window test red at
`left: 150000, right: 65536` on the `getattrlist … stays Ptr` assertion and the sweep red naming
`[(220, DestBuffer)]` as unlisted — the ruling is enforced, not merely written. Control 3 (336 →
`Ptr`) measured the unclamped outcome, and it was the over-long return rather than `EFAULT`: `got
ret=3612 err=false` — the host kernel, handed `buffersize = 4160` over a 64-byte backing, copied
3,612 bytes (903 pids) into that destination, 3,548 bytes past the guest backing into retrace's own
process, and reported no error. With the row, `(64, false)`. That is the M27 "serious half" seen
once, and it is the value this milestone delivers on a corpus where the window half is inert.

**Gate and sweep.** Gate **572 / 0 / 2 over 124**, exactly the §10 prediction, every chunk's exit
code 0 captured before any pipe, clippy clean, `jq`/CPython gates run rather than skipped;
reconciled file-by-file against M33's 570 / 0 / 2 over 124 with `truncguard.rs` 19 → 21 the only
count that moved. The sweep did not land on §10's prediction the first time: run 1 gave
`pass=45 fail=9 skip=0`, M33's eight plus `/usr/bin/dddiagnose` (replay diverged), the README's
documented intermittent; a ten-run probe (five on the M34 binary, five on the pre-M34 one, every
recorder pid logged and every one inside the §4b collision range) passed 10/10, and run 2 after the
gate gave `pass=46 fail=8 skip=0` with the FAIL set byte-identical to M33's. Ruled the intermittent,
not a regression — `dddiagnose`'s own census rows all sit inside the flat window, so M34's rows
change nothing it does — and the ten in-range pids bound the §4b hypothesis for M36: a pid in
range does not by itself produce that divergence.

**Corrected by the fix wave (after the final whole-branch review), in the spirit of M33 t7.**
Three supporting facts in this spec were wrong on measurement and are corrected in place above,
each marked *(corrected by the fix wave)*: (1) §4b attributed landmark #24's `EINVAL` to the pid
probe — it is `task_set_dyld_info`'s three-call rule on retrace's own task, pid-independent,
measured with `sdi.c` at pid `0x107dc`; (2) §3b/§10 said the clamp "can only fire when the
guest's buffer already overruns its own backing" and that "no corpus call is in the regime the
rows change" — it fires at #24 on 76 of 76 dynamic guests (368 → 128 at a shared-cache page
boundary), measured through the `RETRACE_REGCLAMP=1` channel the fix wave added to the `Reg`
arm; (3) §4's table said `csops` op 16 came from "2 (an Apple binary, CPython)" — it is 11
guests, 22 dispatches, the summariser having collapsed every `apple:*` label to `apple` before
counting (the instrument now counts full labels). Also corrected: §4's "115 guests dispatched"
→ 118; the line citations `diff_window` 3081 → 3082, the `Reg` arm 3284 → 3286 (3298 after the
channel landed), the probe loop 3189 → 3188–3189. Newly owed, recorded in the status-log and
README: `SET_DYLD_IMAGES` serviced above the trace rather than forwarded; the per-page cache
backing clamping any `Dest` destination that straddles a 16 KiB shared-cache boundary — a
pre-existing fidelity hazard for the `read` family too, whose fix is contiguous host backing for
the window, not a table change.
