# M27-truncguard — design

**Goal:** Make the record-side diff-window truncation class **fail loud** instead of silent, then fix
exactly what that reveals. M26 proved the class exists, fixed the one shape it could measure, and
published the rest as a table. M27 stops guessing at that table and makes the machine name its own
instances.

**Predecessor:** M26 (`29eac9a`), which found the defect and deliberately did **not** land the
tripwire — landing a `panic!` without knowing whether it fires on existing gate guests would have
been the "right conclusion resting on an unmeasured supporting fact" this repo keeps catching in
itself. Measuring that is M27's Task 1, and it is the reason M27 exists as its own milestone.

## The problem M26 left

`Box_::forward_and_diff` captures the kernel's writes into guest memory by snapshotting a pre-image
*window* of each pointer argument, forwarding, then diffing that window. M26 widened the window to
the true length for the three syscalls whose `x2` is a byte count. **Everything else still gets a
flat 64 KiB**, and there is **no BSD-syscall allowlist** — everything not explicitly intercepted is
forwarded — so the set of syscalls that might overrun is open-ended rather than enumerable.

The failure is *latently* silent: `(num, args)` match on both sides so the per-landmark oracle sees
nothing, and `Box_::diff_memory`'s terminal compare only catches it if the guest neither acts on the
bad bytes nor drops their backing first.

## Approach: prove the overrun, don't infer it

The obvious tripwire tests the **last K bytes of the window** — if they all changed, the write
reached the end and probably ran past. That is inference, and it carries a false positive (a write
ending exactly at the boundary) and a false negative (a tail of zeros written over zeros; K=1 would
have missed M26 itself).

M27 looks **past** the window instead. The justification is a property of the recorder, not a
heuristic: between the pre-image copy and the post-image copy the only thing that executes is
`host_svc` — the guest vCPU is halted and `clippy.toml` bans recorder threads — so **any** pre≠post
byte is provably a kernel write from that syscall.

> **The guard band.** When the window is capped (`win < avail`), snapshot `GUARD_BAND` bytes
> immediately past it, bounded by `avail`. Kernel writes into a destination buffer are contiguous
> from the buffer start, so any overrun necessarily lands in that band. A changed band is therefore
> **direct evidence** that the write ran past the diff window, not a guess.

`GUARD_BAND = 64`. One byte would suffice for contiguity but not for confidence: a single byte
matching its pre-image by chance is ~1/256 for random data and far likelier for the zero-heavy data
a `.pyc` or a zeroed page contains. Sixty-four bytes costs one extra 64-byte copy per capped window
and makes a missed overrun negligible.

**Degenerate case is clean.** When `win == avail` the window already covers the whole backing, so
there is no band and none is needed: nothing can be written past the backing without a separate
memory-safety bug, which is a different failure with its own loud symptom.

### Rejected alternatives

- **Tail sentinel inside the window** — same cost class, strictly less precise. Rejected because the
  consequence of a firing is a `panic!` that kills a long record run, and a false positive there is
  expensive.
- **Refuse any capped window with an unmodelled destination length** — maximally strict, but it
  fires on every `fstat`-shaped buffer that never overruns. Unusable.

## Sequencing — measurement before the assert

This ordering is the milestone's spine and is not an implementation detail:

1. **Land the tripwire as a `eprintln!` warning.** Run the **full gate** and record every firing.
   This is the blast-radius measurement M26 owed and did not take.
2. **Resolve each firing** — either the destination length is knowable (add it) or the firing is a
   genuine new finding (record it).
3. **Only then flip the warning to `panic!`.**

Inverting steps 1 and 3 would be the exact error this milestone exists to avoid.

## What it is already known to fire on

`/bin/ps`, measured during M26 with the prototype:

```
[M26 TRUNC?] num=202 ipa=0x700800000 win=65536 tail64 all-changed
DIVERGENCE at landmark 11652 ... memory divergence at ipa 0x700810091: replay=0x00 recorded=0xc0
```

`0x700810091 − 0x700800000 = 65681` — 145 bytes past the window, replay holding zeros where the
recording holds data. `ps` sizes a `sysctl(KERN_PROC_ALL)` buffer at roughly
`nproc × sizeof(struct kinfo_proc)`; this machine has 582 processes.

**`sysctl` is therefore in scope by the milestone's own rule** ("fix what it fires on"), and its
length has a shape M26's predicate cannot express: it lives at `*(size_t*)x3`, in guest memory rather
than in a register. This is why M27 generalises the predicate into a small table rather than adding
a third special case.

## Components

### 1. `Box_::overran_window(pre_guard, post_guard) -> bool`

A pure predicate, unit-tested the way `Box_::clamp_count` is. Keeping the decision pure keeps the
policy reviewable separately from the `unsafe` slice plumbing around it.

### 2. `dest_len(num) -> Option<DestLen>` in `retrace-arch`

M26's `writes_x2_bytes_to_x1` answers a yes/no question about one shape. `sysctl` needs a second
shape, and hard-coding a second predicate would recreate the duplication M26 just removed. So:

```rust
pub enum DestLen { Reg(usize), DerefU64(usize) }   // length in a register / behind a guest pointer
pub fn dest_buffer(num: u64) -> Option<(usize, DestLen)>   // (dest arg index, where its length is)
```

Seeded **only with what is measured or SDK-verified**, not with the whole audit table:
`read`/`pread`/`read_nocancel`/`pread_nocancel` → `(1, Reg(2))`; `sysctl` → `(2, DerefU64(3))`.
Others are added when the tripwire names them, which is the entire point of building the tripwire
first.

The clamp and the window both consult this one function, preserving M26's invariant that the two
bounds cannot drift apart.

### 3. `pread_nocancel` (414)

Absent from `fd_operands`, the clamp, **and** the window. The missing clamp is the serious half: it
forwards **unclamped**, so the host kernel may write past the guest backing — a host memory-safety
hazard rather than a fidelity gap. `fd_operands`' own doc comment already states the rule this
breaks ("A plain-only table fails *silently*").

### 4. A fail-loud refusal for the nested-pointer family

`readv` (120), `readv_nocancel` (411), `recvmsg` (27), `recvmsg_nocancel` (401), `preadv` (540),
`recvmsg_x` (480) put their destination behind a pointer *inside* a guest struct (`iovec.iov_base`,
`msghdr.msg_iov`). `forward_and_diff` translates only top-level register arguments, so today a guest
IPA would reach the host kernel **as a host address**.

The audit's inference is that these would `EFAULT` because guest IPAs are unlikely to be mapped in
retrace's process. That is an inference, and if it is wrong the failure is a wild write into
retrace's own memory. M27 does **not** test that inference and does **not** translate the pointers;
it refuses by value and names the measurement owed, the way `guest_workq_kernreturn` refuses an
unenumerated opcode. Translating them properly needs the `translate_mwl_regions` treatment and its
own measurement.

## Symmetry

Nothing here reaches the record/replay dispatch:

- The tripwire and the refusal are **record-side panics**. A panic produces no trace, so there is no
  landmark for replay to disagree about and **no mirror arm is owed** under symmetry rule 1.
- `dest_buffer` widens a *window*, which changes only how much of the kernel's write is captured. The
  captured bytes are ordinary `Event::Syscall` writes that replay already applies.
- **`TRACE_MAGIC` does not move.** No `Event` variant or field changes. If a format change seems
  necessary, that is a spec deviation — stop.

## Testing

- **Unit:** `overran_window` (pure); `dest_buffer`'s table, including that `fsgetpath` (427) stays
  absent because its `fsid_t*` names a volume, not a descriptor — M25 pinned that and it must not
  silently reopen.
- **Negative control:** `bigread_e2e` must stay silent. Its window is no longer capped after M26, so
  a firing there would mean the guard-band logic is wrong rather than that a bug was found. This is
  the test that keeps the tripwire from becoming noise.
- **`pread_nocancel`:** an arch-layer test that 414 and 153 agree, the shape `fd_operands`' existing
  plain-vs-`_nocancel` assertions already use.
- **Integration:** `/bin/ps` records and replays once `sysctl` is covered. This is the milestone's
  headline gate and moves the Apple-binary count 46 → 47.

## Risks

- **R1 — the tripwire fires somewhere unexpected during Task 1.** That is a *success*, not a
  setback: it is the measurement. But it may enlarge the milestone, and if a firing needs modelling
  the repo has not measured, the honest move is to park that one and say so rather than guess a
  length.
- **R2 — `ps` may not be one cause.** M23's measurements left open "whether `ps`'s divergence is one
  cause or several". Fixing `sysctl` may reveal a second. The gate must therefore assert `ps`
  actually records *and replays*, not merely that the tripwire stopped firing.
- **R3 — a `panic!` on a long record run is expensive** (CPython records in ~50s, and a panic
  discards it). Accepted deliberately: a warning on a run that later replays wrong is precisely the
  silent green this repo forbids, and Task 1's warning phase exists so the panic lands only after
  the firings are known.
- **R4 — guard-band reads near a backing edge.** The band is clamped to `avail`, and the `win ==
  avail` case is skipped entirely, so the tripwire never reads past a backing it was given.

## Explicitly out of scope, and why

- **`diff_memory`'s `.min(avail)` truncation** — the terminal compare silently shortens a recorded
  region longer than its replay backing. Real, unpaid since M1, and a *replay*-side concern; mixing
  it with a record-side milestone would make both harder to review.
- **The `if !err` gate**, which skips write capture entirely on a failed syscall. `sysctl` with an
  undersized buffer copies out what fits and returns `ENOMEM`, which may contradict the comment's
  universal claim. **Unmeasured**, and named rather than fixed on inference.
- **The remaining audit table** (`getdirentries64`, `recvfrom`, `getfsstat64`, `proc_info`,
  `getattrlist`/`fgetattrlist`, `csops`). Structurally at risk, none measured to overrun. They are
  added when the tripwire names them — building the tripwire *is* the alternative to guessing.
