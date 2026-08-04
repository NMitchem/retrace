# retrace M10-fdtable — a real guest fd table, and rung 3 (`jq` over a file)

M9 stopped the guest from closing retrace's stdout by special-casing fd 0/1/2. Above fd 2 nothing is
virtualized at all: the guest's file descriptors **are** retrace's, handed out by the host kernel from
retrace's own process. M10 gives the box a real fd table, so a guest descriptor is a guest descriptor.

It also lands the rung-3 gate. Rung 3 needs no new capability — it already passes (measured below) —
but it is unpinned, and it is the guest that makes the fd table load-bearing.

## The problem, precisely

`forward_and_diff` (`crates/retrace-box/src/lib.rs:2086`) issues the guest's syscall through a raw
`svc #0x80` **in retrace's own process**. Nothing translates the fd operand. So:

1. A guest `open` returns whatever number the host kernel had free in *retrace's* fd table.
2. A guest `close(n)` closes retrace's real fd `n`.
3. A guest that names an fd it never opened reaches whatever retrace has open there.

M9 closed (2) for `n <= 2` only, and said so honestly: "retrace does not model the fd as CLOSED
afterwards … modeling it means giving the box a real fd table" (`crates/retrace-core/src/lib.rs:151`).

This is both a determinism defect and a correctness defect, and the second is the one that matters.

## Verified facts (measured on this host, HEAD `84983dc`, 2026-08-04)

Recorded and replayed `jq '.name' t.json` over `{"name":"retrace","rung":3}` via
`record-dyn … -- '.name' <abs path>`:

- **Rung 3 already passes.** Record printed `"retrace"`; two successive replays printed `"retrace"`.
- **The trace is self-contained.** Rewriting the input file to `{"name":"TAMPERED"}` between replays
  did not change replay output. The file's bytes live in the trace as recorded kernel writes, and
  replay executes no syscall — the forward-and-record posture holds for file reads.
- **The run is 352 traps.**
- **The guest's fds are retrace's host fds.** Observed guest descriptors: `0x11, 0x12, 0x13, 0x14,
  0x15, 0x16` — 17 through 22. `jq` starts at 17 because retrace's process already holds 0–16 open.
- **fd-taking syscalls actually exercised** (`RETRACE_TRACE=1` histogram, names resolved from the
  MacOSX SDK `sys/syscall.h`):

  | num | name | count | fd operand |
  |-----|------|-------|------------|
  | 6   | `close`          | 17 | x0 |
  | 92  | `fcntl`          | 17 | x0 |
  | 197 | `mmap`           | 14 | **x4** |
  | 5   | `open`           | 12 | — (allocates on return) |
  | 339 | `fstat64`        | 7  | x0 |
  | 54  | `ioctl`          | 5  | x0 |
  | 153 | `pread`          | 4  | x0 |
  | 41  | `dup`            | 3  | x0 (allocates on return) |
  | 399 | `close_nocancel` | 3  | x0 |
  | 463 | `openat`         | 2  | x0 (dirfd; `AT_FDCWD` passes through) |
  | 396 | `read_nocancel`  | 2  | x0 |
  | 398 | `open_nocancel`  | 2  | — (allocates on return) |
  | 397 | `write_nocancel` | 1  | x0 (console; M9 handles) |
  | 97  | `socket`         | 1  | — (allocates on return) |
  | 98  | `connect`        | 1  | x0 |
  | 133 | `sendto`         | 1  | x0 |

  Two facts here are load-bearing, and both were missed on a first pass over a truncated histogram:

  **The `_nocancel` trap recurs.** `read`(3) count is **0** while `read_nocancel`(396) is **2** — `jq`
  reads through the `_nocancel` variant, `pread`, and file-backed `mmap`, never through `read`. This is
  exactly the shape of the M9 defect (`write` 4 vs `write_nocancel` 397, where the plain variant was
  never taken and the console silently leaked to the host). A table row for `read` alone would forward
  a raw guest fd and nothing would look wrong. Every `_nocancel` variant of an fd syscall needs its row
  next to the plain one, and the mapping should be built variant-pair-wise rather than one number at a
  time.

  **The table is not files-only.** `socket`/`connect`/`sendto` appear, so a guest fd may be a socket.
  Allocation-on-return must cover `socket`, not just `open`-family calls.

  `stat64`(338), `proc_info`(336), `__mac_syscall`(381), `csrctl`(483), `getentropy`(500),
  `thread_selfid`(372), `getfsstat64`(347), `getattrlist`(220), `access`(33) and `getpid`(20) appear
  but take no fd operand.

## Why this is a correctness defect, not merely nondeterminism

The determinism half is real: guest fd numbers are a function of how many files *retrace* happens to
hold open, so adding a single `open` anywhere in the recorder shifts every recorded fd number. Replay
survives (it applies the recorded return), but the trace stops being a pure function of the guest,
which is the property the whole project exists to hold.

The correctness half is worse and is not hypothetical. Seventeen guest `close` calls are forwarded raw
in this one run. They happen to name the guest's own files today, but nothing enforces that, and
`cache.rs`'s demand-pager holds a live fd on the shared cache for the duration of the run. A guest
that closes a number retrace owns corrupts the recorder — and M9 measured exactly how that failure
presents: silently, with a zero exit status and no output, because the recorder's own writes go
nowhere.

## The mechanism

### M10-table — the split fd table

The table is two-sided, and the sides are deliberately different:

- **Guest-visible state.** `Vec<FdSlot>`, `FdSlot = Free | Open | Closed`, allocated POSIX lowest-free
  from 3, with 0/1/2 pre-seeded as console slots. Exists **identically on record and replay**. Pure
  function of the guest's own open/dup/close sequence. This is what decides `EBADF`.
- **Host mapping.** `guest_fd → host_fd`, **record-only**. Replay never opens a host fd — it executes
  no syscall — so there is nothing to map.

The split is what preserves the strong symmetry posture. Replay recomputes the guest-visible verdict
and byte-compares against the recording, so a table bug surfaces as a divergence rather than as silent
corruption (symmetry rule 1, the standard posture — *not* the deliberate verbatim-apply exception
M2-xpcport had to take for its minted port). The nondeterministic half never enters the trace.

### M10-xlat — one translation function, two callers

M9's lesson was that a condition spelled at each call site drifts, and drift here is silent. So the
`(syscall → which operands are fds)` mapping lives in `retrace-arch` beside `is_console_write`, and
one translation function consults it.

It has **two** callers, not one, and the spec is explicit about this because the tempting claim
("translate inside `forward_and_diff`") is false: file-backed `mmap` is special-cased upstream in
`retrace-core` and consumes its fd inside `guest_mmap_file`, which `pread`s from it directly and never
reaches `forward_and_diff`. `mmap`'s **x4** is therefore an explicit row, consumed at the second call
site.

`cache.rs`'s pager preads from retrace's own cache fd, which is never a guest fd, and is untouched.

A syscall retrace forgets is a **missing table row** — it fails loudly on first guest use instead of
forwarding a raw guest fd to the host kernel.

Lifecycle:

- `open`/`openat`/`dup`/`dup2` allocate a guest slot **on the way out**, binding it to the returned
  host fd.
- `close`/`close_nocancel` free the slot and close the host fd. For fd ≤ 2 the existing M9 fake stands.
- A `Closed` or `Free` slot returns `EBADF` (errno 9) **without forwarding at all**.

### M10-checkpoint — carry the table

The guest-visible table joins `BoxState` (`crates/retrace-box/src/lib.rs:401`). The host map does not:
`from_checkpoint` is a replay-side operation (`checkpointed_seek`), and replay has no host fds.

This is the third time this repo has paid for the same bug — `pac_enabled` ("must be carried
instead", M7 t6), `stack_top`/`stack_size` (M8-stack), and `tlbi_stub_ready` (M9 t3, where
`from_checkpoint` reset a flag the restored backings contradicted). If the table defaults to empty on
restore, every seeked session believes all fds are `Free`, so a post-seek guest `pread` returns
`EBADF` and reverse execution diverges from the forward run.

### M10-rung3 — pin what already works

`jq '.name'` over a small repo-fixture JSON, recorded and replayed, following `jq_e2e`'s loud-skip
discipline for the Homebrew dependency.

## Determinism posture

**Standard and symmetric.** Guest fd numbers become a pure function of the guest's own syscall
sequence, so both runs compute the same numbers and the divergence oracle checks them as ordinary
`(num, args)`. No new asymmetry is introduced, and the M2-xpcport exception is not extended.

Host fd numbers — the only nondeterministic quantity in the design — are record-only and never appear
in an `Event`.

## Correctness invariant

**No syscall retrace forwards to the host kernel may carry a guest-supplied fd operand.** Every fd
operand crossing into `host_svc` is a host fd the box itself allocated and owns; every fd the guest
names is validated against the guest-visible table first.

## Scope

**In:** the split table; the `(syscall → fd operand)` mapping in `retrace-arch`, built variant-pair-wise
so every `_nocancel` form is tabled beside its plain form; translation at its two call sites; `EBADF`
for closed/unknown; allocation-on-return for `open`/`open_nocancel`/`openat`/`dup`/`dup2`/`socket`;
`BoxState` carriage; the rung-3 gate; a guest program pinning the semantics.

**Out:** `RLIMIT_NOFILE` enforcement (the table grows; no guest in the gate exhausts fds) — a guest
querying `getrlimit` still gets the host's answer. Guest **stdin** (fd 0 remains retrace's; no gate
guest reads it). Guest-raised signal delivery. Threads. `dup3`/`fcntl(F_DUPFD)` allocation semantics
are handled only if the gate exercises them; otherwise they are missing rows that fail loudly.

## Exit criterion

1. `jq '.name' <fixture>` records and replays bit-for-bit, un-`#[ignore]`d.
2. A guest program observes **fd 3** for its first `open` — not 17 — and gets `EBADF` after closing it.
3. The fd table survives a checkpoint restore.
4. `just gate` green with no `#[ignore]`, at or above 185 passing.

## Testing

- **`retrace-arch` unit:** the `(syscall → fd operand)` mapping, mirroring the existing
  `is_console_write`/`is_console_close` tests — including `mmap`'s x4 and `openat`'s dirfd.
- **`retrace-box` unit:** lowest-free allocation, `close` → `EBADF`, `dup` aliasing, 0/1/2 console
  pre-seed, and `fd_table_survives_checkpoint_restore` in the shape of M9's regression test.
- **Guest `fdtable_dyn.c`:** asserts the guest's first `open` returns **3, not 17** — the determinism
  property made directly observable — then close → `EBADF`, then a `dup` alias.
- **`jq_file_e2e`:** the rung-3 gate.
- **Regression:** dyld opens the shared cache and every dylib, so all 185 existing tests now run
  through translation. A translation bug fails immediately and loudly rather than subtly.

## Risk register

- **R1 — a missed fd-taking syscall.** A syscall with an fd operand and no table row forwards a guest
  fd raw. **This risk already fired once, during the authoring of this spec:** a first pass over a
  truncated histogram tabled `read`(3) — which `jq` never calls — and missed `read_nocancel`(396),
  `open_nocancel`(398), `socket`(97), `connect`(98) and `sendto`(133). *Mitigation:* the row is required
  to forward at all, so a miss fails loudly rather than silently; `_nocancel` variants are tabled
  pair-wise with their plain forms; task 1 re-derives the surface from a full untruncated histogram
  rather than trusting this table. **Residual: medium** — the histogram covers what `jq` and dyld do,
  not what every future guest does.
- **R2 — `EBADF` where the host previously succeeded.** Virtualization is a behavior change: a guest
  naming fd 17 gets `EBADF` instead of retrace's descriptor. That is the intent, but it could surface
  as a new wall in an existing gate guest. *Mitigation:* the full gate is the detector. **Low.**
- **R3 — checkpoint carriage missed.** Covered by an explicit regression test; the failure mode is
  known from M9 t3. **Low.**
- **R4 — `mmap`'s second call site drifts from `forward_and_diff`'s.** The two callers must consult
  one function. *Mitigation:* single translation fn, no per-site conditions. **Low.**

## Components

- `crates/retrace-arch/src/lib.rs` — the `(syscall → fd operand)` mapping + its tests; new syscall
  constants (`SYS_IOCTL` 54, `SYS_FSTAT64` 339, `SYS_DUP` 41, `SYS_DUP2` 90, `SYS_OPENAT` 463,
  `SYS_READ_NOCANCEL` 396, `SYS_OPEN_NOCANCEL` 398, `SYS_SOCKET` 97, `SYS_CONNECT` 98,
  `SYS_SENDTO` 133).
- `crates/retrace-box/src/lib.rs` — `FdTable`/`FdSlot`, translation, `BoxState` carriage,
  `from_checkpoint` restore, `guest_mmap_file`'s fd translation.
- `crates/retrace-core/src/lib.rs` — allocation on `open`/`openat`/`dup` return; replay-side
  recompute + compare.
- `crates/retrace-guest/c/fdtable_dyn.c` + `build.rs` — the semantics guest.
- `crates/retrace/tests/` — `fdtable_e2e.rs`, `jq_file_e2e.rs`, fixture JSON.
- `README.md`, `CLAUDE.md` — Status section and the honest close.

## Open questions for implementation planning

1. **Does `fcntl` need per-command handling?** `F_DUPFD`/`F_DUPFD_CLOEXEC` allocate a new fd and would
   need allocation-on-return, unlike other `fcntl` commands. 17 `fcntl` calls appear in the run; their
   commands were not decoded. Decode them in task 1 before assuming x0-translation suffices.
2. **Does anything in the gate rely on the guest seeing a specific fd number today?** Expected no, but
   the assertion in `fdtable_dyn.c` inverts a property no test currently states.
3. **Where exactly does allocation-on-return live** — in the box (so `forward_and_diff` returns an
   already-translated fd) or in `retrace-core`'s dispatch arms? The box keeps `retrace-core` thin and
   matches where the table lives; confirm against how the mmap arm is structured.
4. **`AT_FDCWD` (-2) must pass through untranslated.** Confirm no other negative sentinel fds appear.
5. **What is the socket for, and does anything read from it?** `socket`/`connect`/`sendto` appear once
   each with no matching `recvfrom`, which reads as fire-and-forget (notify/syslog). A guest that
   *received* on a socket would pull outside-world bytes into the trace as recorded writes — the same
   forward-and-record posture as `task_info`'s audit token, deterministic on replay but dependent on
   the host at record time. Confirm the no-receive shape in task 1; if a receive appears, name the
   posture explicitly rather than letting it in by default.
