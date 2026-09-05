# M26-cpythonreplay — design

**Written after the work, and saying so.** M26 began as a debugging pass against the wall M25 parked
at, not as a planned milestone. The root cause was found in the first afternoon and turned out to be
one defect with a one-function fix, so a plan written first would have been fiction. This document
and the plan beside it are retroactive; the sequencing they describe is what actually happened, and
the M24 precedent ("the design and the plan, written after t1 and saying so") is the one being
followed. What is *not* retroactive is the measurement record below — every number in it was
captured before the fix was written.

## The wall, as M25 left it

M25 cleared every record-side wall on the CPython path and then parked, with this in
`cpython_e2e`'s `#[ignore]` reason:

> DIVERGENCE at landmark 568 pc=0x1804b1834: syscall mismatch: live (num=4, args=[2, …, 106, …]) !=
> recorded (num=75, args=[…]) … the two runs' syscall SEQUENCES have already diverged by this
> point, not merely one call's arguments.

Three things in that account were wrong, and each mattered:

1. **`num=75` is `madvise`, not `mmap`.** Verified against `sys/syscall.h:115` (`SYS_madvise 75`;
   `SYS_mmap` is 197). The recorded call was `madvise(0x701528000, 98304, MADV_FREE_REUSABLE)` —
   `MADV_FREE_REUSABLE` is 7, `sys/mman.h:217`. Routine libmalloc housekeeping. Reading it as `mmap`
   made the two sides look like unrelated code paths; reading it correctly makes them legible as
   *normal path* versus *error path*, which is what pointed at the answer.
2. **The sequences had not parted ways.** Landmarks 0..559 matched exactly — they must have, or the
   oracle would have complained earlier. The divergence was the guest reacting *correctly* to bytes
   replay had failed to restore.
3. **The landmark index was never a stable fact.** M25 pinned 568; this machine's recording diverges
   at 560, same `pc`, same live args. The index is an artifact of one trace.

## What the 106 bytes said

The measurement that closed it was reading the buffer live's `write(2, …, 106)` was about to emit:

```
Error in sitecustomize; set PYTHONVERBOSE for traceback:
ValueError: bad marshal data (unknown type code)
```

A *data* divergence, not a control-flow one. The guest was unmarshalling a `.pyc` and hit a byte
that is not a valid type code.

## Root cause

`Box_::forward_and_diff` records what the host kernel writes into guest memory by snapshotting a
pre-image *window* of each pointer-valued argument, forwarding the syscall, then diffing that same
window. The window was a flat `PTR_WINDOW_CAP` (64 KiB). The "Debt #1" clamp immediately below it
bounds the forwarded read **count** by the destination's backing (`avail`), **not** by the window.

Two bounds that must agree, written twice. The kernel may legitimately write far past what the diff
inspects; the excess lands in guest memory on record and in **no** `Event`, and replay restores stale
bytes there.

Measured, landmark 556 of the CPython recording:

```
num=3 (read) args=[fd=4, buf=0x701528020, count=0x15828=88104]
ret=88103   writes=1   bytes=65536
```

22567 bytes read and never recorded.

## How the class hid for 25 milestones

**It was booked and half-paid.** M1's plan (`2026-07-05-retrace-m1.md:874`) flagged the window policy
as needing revisiting "once real programs (large mappings, failing syscalls) are recorded". M2 then
deliberately declined to touch it (`2026-07-06-retrace-m2.md:462`) — correctly, because `x2` is only
a count for the read family and clamping the *snapshot* by it would under-snapshot `fstat`'s buffer
and regress M1. Nobody asked the inverse question: whether the window should be widened **up** to
`x2` for the syscalls where `x2` genuinely is a count.

**And no guest could reach it.** Every bulk file read in the tree bypasses `forward_and_diff`:
file-backed `mmap` goes through `guest_mmap_file`, which records its full extent; the shared-cache
pager reads fixed 16 KiB pages; retrace's own Mach-O loading is host-side `std::fs::read`. The
largest count measured through `forward_and_diff` before M25 was dyld's `pread` of 0x4000 (16 KiB).
`jq_file_e2e`'s fixture is 28 bytes. The defect stayed dormant by construction, not by luck.

## The failure is *latently* silent, and the distinction is load-bearing

The per-landmark oracle structurally cannot see it: `(num, args)` are identical on both sides, so the
recording is self-consistent and merely incomplete.

But there is a second backstop, and it is real. `Box_::diff_memory` byte-compares every recorded
region at exit, and all three terminal replay arms `return Err(Divergence)` on mismatch. Stale bytes
surface there **unless** the guest acts on them first, or drops their backing.

That is why the two known instances failed in two different places, and why neither was ever a
passing green:

| guest | what it did with the bad bytes | where it failed |
|---|---|---|
| CPython (M25) | branched on them | syscall landmark ~560 |
| `bigread` (M26) | ignored them | terminal memory compare, at buf+0x10000 |

The genuinely silent escape hatch is a read into a mapping that is then `munmap`'d, since
`guest_munmap` removes the backing and the terminal compare has nothing left to look at. **No gate
does that today** — stated so it is not left as an unexamined assumption.

## The fix

Not a bigger constant: 300 KiB would break identically. The defect is the duplication, so the two
bounds now share one predicate.

- `Box_::writes_x2_bytes_to_x1(num)` — the set where `x1` is a destination buffer and `x2` its byte
  count. The clamp and the window both consult it, so they cannot drift again.
- `Box_::diff_window(num, i, avail, count)` — returns the 64 KiB heuristic except for that one
  argument of those syscalls, where it widens to cover the clamped count. Never exceeds `avail`,
  because `clamp_count` bounds it.

Everything else keeps the heuristic deliberately: widening unconditionally costs a pre-image copy on
every pointer operand of every syscall, and M8 measured that per-syscall diff time is not free.

## Coverage — the negative space

An audit milestone that does not publish what it did **not** fix is indistinguishable from a lucky
patch. `writes_x2_bytes_to_x1` covers `read` (3), `pread` (153), `read_nocancel` (396). Everything
below is **still truncating**, and none of it is hypothetical hand-waving — each was checked against
the SDK headers:

| syscall | destination / length | status |
|---|---|---|
| `sysctl` (202) | dest `x2`, length at **`*(size_t*)x3`** — in guest memory, not a register | **A CONFIRMED SECOND INSTANCE — see below** |
| `pread_nocancel` (414) | dest `x1`, len `x2` | **absent from `fd_operands`, the clamp, AND the window.** The missing clamp means it forwards **unclamped**: a host memory-safety hazard, not a fidelity gap |
| `getdirentries64` (344) | dest `x1`, len `x2` | exactly the fixed shape, not in the predicate; measured at 8 KiB, unbounded in principle |
| `recvfrom` (29) / `_nocancel` (403) | dest `x1`, len `x2` | same shape, not in the predicate, not measured to occur |
| `getfsstat64` (347) | dest `x0`, len `x1` | `sizeof(struct statfs)` = 2168, so 30 mounts crosses 64 KiB; this machine has 24 |
| `proc_info` (336) | dest `x4`, len `x5` | unbounded in principle; common flavors are small |
| `getattrlist` (220) / `fgetattrlist` (228), `csops` (169/170) | dest `x2`, len `x3` | small in practice, unbounded in principle |
| `readv` (120), `recvmsg` (27), `preadv` (540), and `_nocancel` kin | destination behind a pointer **inside a guest struct** | a worse class: the nested pointer is never translated, so a guest IPA reaches the host kernel as a host address |

Two further holes found by the same audit and **not** fixed here:

- **`diff_memory` truncates its own comparison**: `let n = r.bytes.len().min(avail)`. The backstop
  this whole story rests on silently shortens a recorded region longer than its replay backing.
  Flagged in M1's own branch review, deferred to M2 alongside the clamp, and only the clamp was paid.
- **`if !err` skips capture entirely** on a failed syscall. The comment calls that universal. It is
  not obviously so — `sysctl` with an undersized buffer copies out what fits, sets `*oldlenp`, and
  returns `ENOMEM`. **Not measured**, and named here rather than fixed on inference.

## `/bin/ps` was this bug, filed as something else

The README has said since M22 that `ps` is "a genuine replay divergence — the oracle catching
nondeterminism rather than reproducing something wrong in silence". That reading cannot be right:
replay never *executes* a syscall, it applies recorded writes, so a process list cannot vary between
the two runs. The repo's own M22 measurement document says "also not diagnosed" — the README stated
it with more confidence than the measurement behind it.

Measured during M26 with a prototype tripwire (below), recording `/bin/ps`:

```
[M26 TRUNC?] num=202 ipa=0x700800000 win=65536 tail64 all-changed
DIVERGENCE at landmark 11652 ... memory divergence at ipa 0x700810091: replay=0x00 recorded=0xc0
```

`0x700810091 − 0x700800000` = 65681 — **145 bytes past the end of that 64 KiB window**, replay
holding zeros where the recording holds data. `ps` sizes a `sysctl(KERN_PROC_ALL)` buffer at roughly
`nproc × sizeof(struct kinfo_proc)`; this machine has 582 processes. It is the same defect, and M26's
fix does **not** cover it, because `sysctl`'s length is behind a pointer.

## The tripwire, prototyped and deliberately not landed

Between the pre-image copy and the post-image copy the only thing that executes is `host_svc`: the
guest vCPU is halted and `clippy.toml` bans recorder threads. So **any** pre≠post byte is provably a
kernel write from that syscall. Therefore: if the window was capped (`win < avail`) and its final 64
bytes all changed, the write reached the window's last byte and very likely ran past it.

Prototyped as a warning and measured: it fired **exactly once** on `/bin/ps`, on the actual culprit,
with no false positives on that run. A 64-byte tail is chosen over 1 byte because a single byte
matching its pre-image by chance is ~1/256 for random data and much likelier for the zero-heavy data
a `.pyc` contains — K=1 could have missed M26 itself.

It is **not landed here**, and the reason is discipline rather than doubt: turning it into the
`panic!` it should be requires knowing whether it fires on any existing gate guest, and that is a
full-gate measurement M26 did not run. Landing a fail-loud assert without that measurement would be
exactly the "right conclusion resting on an unmeasured supporting fact" this repo keeps catching in
itself. It is the first task of the successor.

## What was NOT measured, stated so nobody cites it as measured

- Whether the tripwire fires on any current gate guest. **This is the blast radius**, and it is the
  one thing that must be measured before the assert lands.
- The actual buffer sizes any guest passes to 347 / 202 / 336.
- Whether `sysctl` writes to the guest buffer on an `ENOMEM` return.
- Whether `pread_nocancel` (414) is reachable by any guest this repo runs.
- Whether the `readv`/`recvmsg` family would `EFAULT` or corrupt host memory. The inference is that
  guest IPAs are unlikely to be mapped in retrace's process, so they would fault — **an inference,
  and a wrong guess there is a wild host write.**
- The post-fix CPython syscall census. M25's was taken before the fix and stops at the `encodings`
  import; the gate now runs to completion, so the reachable set is strictly larger and unenumerated.

## Risks

- **R1 — the fix is narrow by choice.** Three syscalls of an open-ended forwarded set. Mitigated by
  publishing the table above rather than implying closure, and by naming the successor.
- **R2 — `bigread` could rot into a vacuous gate** if a future change made its buffer smaller than
  the window. The test asserts `ret == 0x18000` explicitly so that shows up as a failure.
- **R3 — the terminal compare is load-bearing and itself holed** (`diff_memory`'s `.min(avail)`).
  Named above; unpaid.
