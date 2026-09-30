# M47-gitwrite: git's local workflow, with `madvise`, `__mac_syscall` and `fork` modelled

**Date:** 2026-09-30. **Branch:** `worktree-m47-gitwrite`, to be cut from `main` at this spec's
commit, whose parent is the M46 merge `427fa0a`, pushed 2026-09-30.

**Companions:**
- `2026-09-30-retrace-m47-gitwrite-measurements.md`, written by t0 (§3a);
- `docs/sweep-evidence/2026-09-30-m47-probe/`, committed with this spec. It is the brainstorming probe:
  what was run, on which binary, and the excerpts. Its README lists every file.

**Sources.** Every claim below says where it comes from:
- **probe**: the committed evidence directory, with a file name;
- **code**: read at `427fa0a`, with a path;
- **native**: lldb, `otool` or a native run on the probe host, macOS 26.5.2 (25F84);
- **inferred**.

A claim that is inferred is still owed a measurement wherever t0 can take one, and says so.

**Scope and approach.** Both were chosen by the operator in brainstorming on 2026-09-30.
- **Scope.** `git`'s local workflow, plus the three mechanisms the probe found it needs, plus the
  AMFI fix that unblocks `@rpath` guests.
  - Timed waits were **routed out** (Q1). The probe had blamed them, and further measurement showed
    git never reaches one (§2d).
  - The fork that default-config `git commit` makes is **refused by value** (Q2). It is not left as a
    documented limit, and it is not modelled for real.
- **`madvise`.** Approach **1A**: a `Box_` method run identically on both sides, never forwarded.
  - 1B was rejected. It records the `MADV_ZERO` zeros, which is 512 KiB of trace per call, and
    replay applies them blindly.
  - 1C was rejected. It keeps forwarding, which keeps the silent corruption.
- **`__mac_syscall`.** Approach **2A**: modelled per `(policy, call)`, with AMFI answered by the host
  into a host-owned slot and recorded.
  - 2B was rejected. It synthesizes AMFI's answer from a constant table, which invents a policy
    retrace does not control. That is the M8 lesson: do not out-synthesize a constant you do not
    control.

## 1. Purpose

The 2026-07-05 vision spec's v1 bar is recording and reverse-debugging `python3`, `node` and `git` on
real workloads (`2026-07-05-retrace-macos-record-replay-design.md:261-270`). `python3` has been met
since M26 (rung 7) and M39 (rung 8). `git` and `node` had never been run until the 2026-09-30 probe.
The probe found read-only `git` already working, and git's write path blocked by a handful of
measured, bounded walls. One of those walls is a **silent heap corruption** that the determinism
oracle cannot see.

M47 makes `git` the second of the three. It also closes the corruption class, and the one
nested-pointer forward the `arg_kinds` table misfiles, for every guest.

**Success** has five parts:

1. **git's local workflow records and replays.** The commands are `status` (short and long),
   `diff`, `log`, `show`, `rev-parse`, `-C <dir>`, `add`, and `commit` under default config, plus
   whatever t0 M4 adds to the list. For each:
   - record exits 0, and two replays are byte-identical;
   - a read-only command's stdout equals native;
   - a write command is asserted on the repository it leaves, never on an exit code alone.
2. **`madvise` never reaches the host kernel.** Every advice value in the measured set is modelled.
   Every other value stops the recorder with a message naming the value.
3. **`__mac_syscall` never reaches the host kernel unmodelled.** A repo-owned guest that loads a
   dylib through `@rpath` records and replays.
4. **`fork` is refused with `EAGAIN`.** Default-config `git commit` then completes: it prints git's
   own "cannot fork" line, and the recorder prints its refusal line.
5. **`node` is parked at a measured wall.** It is a gate `#[ignore]`d at `kevent` (363), with its
   reason in the house form. It skips loudly where Homebrew `node` is absent.

## 2. What is known before t0

### 2a. Read-only git already works (probe: `git-runs.txt`)

The binary is `/Applications/Xcode.app/Contents/Developer/usr/bin/git`: arm64, Apple-signed, linking
only system dylibs. `/usr/bin/git` is an `xcrun` shim, and a shim reaches M38's `posix_spawn`
refusal (probe README).

On the unpatched `427fa0a` CLI, `--version`, `rev-parse HEAD`, `log -1` and `show --stat` record to
exit 0, with traces of 59–70 MB, and replay with byte-identical stdout (`git1`, `git11`, `git12`,
`git15`).

The guest environment is **empty**. `load_dynamic` pushes an empty `envp`
(`crates/retrace-box/src/lib.rs:2142`). So git reads no `HOME`, no `~/.gitconfig` and no `GIT_*`
variables. It needs `-c user.name=… -c user.email=…` to commit, and it takes its timestamps from the
host's wall clock, through the forwarded `gettimeofday`. A recorded commit's **hash** therefore
differs from a native twin's, and its **tree** hash does not.

### 2b. Four missing rows (probe: `git-runs.txt`; code)

These numbers have no `arg_kinds` row and are absent from `tests/census.rs`, so each stops the
recorder at the M33 panic:
- `chdir` (12): `status`, `diff` and `-C`, because git calls it even without `-C` (`git2`–`git4`,
  `git13`, `git14`);
- `mkdir` (136) and `link` (9): `add`;
- `utimes` (138): `commit`.

`rename` (128) and `fchdir` (13) already have rows, from M44 and earlier
(`crates/retrace-arch/src/lib.rs:803`, `:819`).

With scratch rows, the following all record and replay identically (`git21`–`git25`, `g32`,
`g41`):
- `status --porcelain`;
- long `status`;
- `diff`;
- `-C … rev-parse`;
- `add`.

`utimes` was reached only after a crashed run had left objects behind (git "freshens" an existing
object's time instead of writing it). So the minimal row set for a clean repo is inferred, and t0
M4 measures it.

### 2c. `madvise` (probe: `git-runs.txt` `g33`/`g35`/`g36`, `commit-abort.txt`; code; docs)

Row 75 is `[Ptr, Scalar, Scalar]`, and it is **forwarded** with `addr` rebased onto retrace's own
backing (`crates/retrace-arch/src/lib.rs:834-848`). `git commit` meets two advice values there.

- **`MADV_ZERO` (11)** over `0x80000` bytes at `0x7009a4000` (`g33`). The kernel zeroes the range,
  which is a **write** of 512 KiB of guest memory. The M30 guard band catches it and the recorder
  panics. That is loud, and correct as far as it goes.
- **`MADV_FREE_REUSABLE` (7)** at `0x7009a4000`, length `0x20000` (`g35`, landmark `#593`).
  - The forward succeeds, and 19 landmarks later the guest aborts in libmalloc:
    `*** error for object %p: pointer being freed was not allocated`.
  - The frame chain, symbolicated, is `git cmd_commit` → `fmt_ident` → `datestamp` → libc
    `localtime_r` → `tzsetwall_basic` → `tzload` → `tzparse` → `tzload` → `free`.
  - Replay reproduces the abort **identically**: record and replay agree while the program is wrong.
    That is the one failure a determinism oracle cannot see.
- **The A/B (`g36`).**
  - With advice 7 and 8 turned into scratch no-ops, the abort does not occur and the commit is
    written.
  - A minimal `localtime_r` guest records and replays cleanly on the unpatched CLI
    (`lt-control.txt`), so the abort needs git's heap history.
  - Whether the abort reproduces on every forwarded run is unmeasured. It was seen 1 of 1 times, and
    t0 M1 measures it.
- **The mechanism is inferred, and M47 does not need it.** It is the hazard M37 named and M45 first
  measured on `/bin/ps` (`docs/current-state.md`, the `/bin/ps` sweep entry and the Known-limits
  bullet). The host may reclaim a page of retrace's backing that was marked reusable, even though
  the guest wrote it afterwards through stage 2. Approach 1A removes the class by construction,
  because nothing is forwarded, whatever the mechanism.
- **jq's 300,000-element abort is not this class.** It aborts identically with 57 `madvise(…, 7)`
  calls no-op'd (`jq-ab.txt`). It stays undiagnosed and out of scope.

### 2d. Syscall 333 is the fork, not a wait (probe: `git-runs.txt` `g40`, `g41`; `fork-disasm.txt`)

Default-config `commit` writes the commit, then issues the following (`g40`):
- `pipe` (42);
- `pthread_sigmask` (329);
- `__pthread_canceled(2)` (333), which is `pthread_setcancelstate(PTHREAD_CANCEL_DISABLE)`;
- `mach_msg2` 3403, which stops the recorder with `RECORD ERROR: unsupported mach_msg2 … msgh_id
  3403`.

This is git's run-command path forking its auto-maintenance child. 3403 is `mach_ports_register`
from libxpc's `xpc_atfork_prepare` ← `libSystem_atfork_prepare` ← `fork`. That backtrace is already
recorded in the `csh`/`tcsh` `#[ignore]` reasons (`crates/retrace/tests/apple_walls_e2e.rs:38`,
`:42`), which park at the same message.

Natively, libc's `fork` runs as follows (`fork-disasm.txt`):
1. It calls the prepare handler.
2. It calls `__fork`, which is `mov x16, #2; svc`.
3. On carry, it calls `cerror` and then the **parent** handler, and returns −1 with `errno` set.

A refusal at syscall 2 is therefore a path libc already takes, provided the prepare handler's 3403
is answered first.

With `-c maintenance.auto=false`, `commit` records to exit 0 and replays identically, with zero 333,
334 or 42 landmarks (`g41`). **git needs no timed wait.**

### 2e. `__mac_syscall` forwards a nested out-pointer (probe: `node-amfi.txt`, `amfi-disasm.txt`, `amfi.out`; code)

Row 381 is `[Path, Scalar, Ptr]` (`crates/retrace-arch/src/lib.rs:891-900`). Its comment reasons
that xnu hands `arg` to the policy uncopied, so the size is the callee's. That is true, and it
misses that the callee may write through a pointer **inside** `arg`.

Every dyld guest issues three shapes, each forwarded:

| Policy | Call | Caller | Result under retrace |
|---|---|---|---|
| `AMFI` | `0x5a` | dyld's `amfi_check_dyld_policy_self` | EFAULT |
| `Sandbox` | `2` | dyld (×3 in git) | EFAULT |
| `Sandbox` | `2` | a second caller, policy string at `0x18df1ac55` | EINVAL |

How the AMFI call is built and consumed (`amfi-disasm.txt`):
- The call is `__mac_syscall("AMFI", 0x5a, &args)` with `args = {u64 inFlags; u64 *outFlags}`.
- The kernel writes `*outFlags`.
- `SyscallDelegate::amfiFlags` returns the flags on success and **0 on failure** (`csel x0, x8, xzr,
  eq`).

The forward rebases `&args` onto retrace's backing, but `outFlags` is a guest stack VA
(`0x27ff2f8`). That address falls in retrace's own `__PAGEZERO`, so the kernel returned EFAULT. **That
EFAULT is luck.** Any `outFlags` that happened to be mapped in retrace's process would have been a
wild 8-byte write into retrace itself. This is precisely the risk `ArgKind::NestedDest` exists to
refuse (`crates/retrace-arch/src/lib.rs:281-298`).

The consequence is that every dyld guest has run with `amfiFlags == 0`, which disallows
`@rpath`, `@executable_path` and `@loader_path` expansion. jq, CPython and every Apple-sweep binary
use absolute install names, which is why nothing noticed. Homebrew `node` links
`@rpath/libnode.141.dylib`, and dyld aborts it: `Library not loaded … (security policy does not
allow @ path expansion)`, then `abort_with_payload` (521), which retrace asserts on.

**Native answer** (`amfi.out`), for an ad-hoc binary, with the same result with or without
`retrace.entitlements`:

| inFlags | Answer |
|---|---|
| 0 | `0x1df` |
| 2 | `0x140` |
| 4 | `0x1df` |
| 6 | `0x140` |

Under retrace, dyld passed inFlags 0. The scratch fix recorded `0x1df`, and node then loaded every
dylib (`node-kevent.txt`).

### 2f. node's second wall (probe: `node-kevent.txt`)

Past AMFI, node records 1,101 traps in 28 s (a 355 MB trace), then stops at `kevent` (363), which
has no row, on the descriptor from `kqueue()` (362). This is libuv's loop. The run had created no
threads and made no `MAP_JIT` mapping.

This is M46's owed "kevent on a guest `kqueue()` descriptor", and it is not in M47 (§7).

### 2g. The sweep baseline (probe: `sweep-427fa0a.log`)

M46 owed an unloaded re-sweep (T6-a). It was run on the unpatched `427fa0a` CLI at load 1.93 → 1.85
and took 5 minutes. Its result was `TALLY pass=49 fail=5`:
- `csh` and `tcsh`: 3403;
- `automationmodetool`: 375;
- `dddiagnose`: 205;
- `yes`: a 30 s timeout, by design.

`[` and `kill`, which M46's loaded sweep lost, pass. **The published 49/54 stands**, and this log is
M47's row baseline.

## 3. Design

### 3a. t0: measurements first

t0 runs on the branch before any product code and writes the companion file. It re-runs, from the
committed scripts, every probe claim it cites.

- **M1, `madvise`.**
  - **(a)** Record the census of advice values, with counts, over:
    - every gate guest;
    - the sweep corpus;
    - the §3f fixtures;
    - `git` (M4's list);
    - `node` (to its wall);
    - CPython.
  - **(b)** Measure how reproducible `g35`'s abort is: N ≥ 5 fresh records of `commit` with advice
    7 forwarded (a base-binary build carrying only the four rows), in a fresh repo each time.
  - **(c)** Look for a repo-owned trigger. `madv_dyn`'s `reuse` mode (§3f) writes after
    `FREE_REUSABLE`, then reads back. Can forwarding make it fail? If it can, that mode is RED;
    otherwise §4 says so.
  - **(d)** Measure native `madvise` for each advice in (a): alignment (EINVAL on an unaligned
    `addr`?) and whether `len` is rounded. §3c's refusal rules are pinned to these results.
- **M2, `__mac_syscall`.**
  - **(a)** Record the census of `(policy, call)` pairs, each with its result and caller symbol,
    over the same corpus as M1(a).
  - **(b)** Identify `Sandbox` call 2: its caller, its argument struct, and why one caller gets
    EFAULT and the other EINVAL. Measure what it returns natively for an unsandboxed process.
  - **Halt H2** applies if a continuity answer cannot be expressed as a function of the arguments
    (§3d).
- **M3, `fork`.**
  - **(a)** Decode the 3403 request: destination, the port descriptors, and `count`. Measure what
    `xpc_atfork_prepare` does with the reply in the parent.
  - **(b)** List every trap between the prepare handler's first and `__fork` (git, `forkfail_dyn`),
    and every trap in the parent handler after a failed `__fork`, measured natively under a low
    `ulimit -u`.
  - **(c)** Record git's native stderr and exit status when its maintenance fork fails with EAGAIN
    under that `ulimit -u`.
  - **Halt H1** applies if (b) finds more than 3403 to answer.
- **M4, git's command list.** Each candidate command runs natively and under the probe build (the
  committed patch as-is), in a fresh repo created by Xcode's git. The probe build **never** gains a
  row for 2: forwarded, `fork` would start a real child of retrace. It has no fork refusal yet
  either, so commands that auto-maintain run with `-c maintenance.auto=false` there. The candidates
  are:
  - `status`, `diff`, `diff --cached`, `log`, `show` and `rev-parse`;
  - `-C`, `add` and `commit`;
  - `branch`, `switch -c`, `tag`, `mv` and `rm`;
  - `stash`;
  - `merge` (fast-forward).

  A command is **in** if it records to its native exit status and output with the M47 mechanisms
  alone. It is **out**, recorded with its wall, if it needs a pager, an editor, the network, a hook,
  or a spawn other than auto-maintenance. The in-list is the gate's list. This measurement also
  settles §2b's minimal row set.
- **M5, `chdir`.** Audit retrace's own code for any relative path it opens after the guest starts,
  on the record and debugger paths. Examples to look for: a lazily created trace file, a sidecar,
  or a symbol file. **Halt H6** applies if one exists.
- **M6, the baseline.** Re-derive the M46 close's counts, 898 / 0 / 9 over 152, at `427fa0a` from
  source by the file-by-file method, so that §9 starts from a measured floor.

### 3b. The rows (`retrace-arch`)

Each row is written from its SDK prototype, with its comment in the house form, and its number is
added to `tests/census.rs` with a sourced doc line:
- `chdir(const char *path)`: `12 => [Path]`;
- `mkdir(const char *path, mode_t mode)`: `136 => [Path, Scalar]`;
- `link(const char *path1, const char *path2)`: `9 => [Path, Path]`;
- `utimes(const char *path, const struct timeval times[2])`: `138 => [Path, Ptr]`. The `Ptr` bound
  is two `timeval`s, 32 bytes, that the kernel copies in, which the row cites.
- `fork(void)`: `2 => []`. It is refused above the generic arm (§3e), and its row documents the
  refusal as rows 59 and 244 do.

Twins (`mkdirat`, `linkat`, `futimes`, `utimensat` and the rest) get rows **only if t0 measures
them**. That is M33's rule: the table is the measured set.

**`chdir` is forwarded (R1).** It moves retrace's own current directory. That is correct on record,
because the guest's later relative paths are forwarded and must resolve where the guest put them. It
is inert on replay, which forwards nothing. The debugger and `gdbserver` run replay only. t0 M5
proves retrace opens nothing relative after the guest starts.

### 3c. The `madvise` model (1A)

**`retrace-arch`** gains:
- the `MADV_*` values, each checked against the SDK's `sys/mman.h` in a test, as M45's constants
  were;
- `madvise_effect(advice: u32) -> Result<MadviseEffect, String>`, where `MadviseEffect` is `NoOp`
  or `Zero`. The accepted set is **exactly** t0 M1(a)'s census:
  - `FREE_REUSABLE` (7) and `FREE_REUSE` (8) are `NoOp`;
  - `ZERO` (11) is `Zero`;
  - any hint or free value the census finds, such as `WILLNEED` (3) or `FREE` (5), is `NoOp`.

  Everything else is `Err`, naming the value.

**`NoOp` is kernel-faithful for these values.** A kernel may keep a reusable or freed page's
contents indefinitely. No guest can depend on reclamation, because native reclamation depends on
memory pressure. A deterministic "never reclaims" is one legal kernel.

**`retrace-box`** gains `Box_::guest_madvise(&mut self, args) -> Result<u64, String>`:
- **The range** `[addr, addr + len)` must be aligned as t0 M1(d) measured, and must lie inside the
  guest's own mapping bookkeeping, committed or reserved. That is the bookkeeping `is_mapped` and
  `commit_reserved_page` consult. A range outside it is **refused**, not answered with a guessed
  errno. No corpus call is outside it (M1(a)). If one ever is, its native errno gets measured then.
- **`Zero`** zeroes every committed page in the range through the box's write path. A
  reserved-but-uncommitted page needs nothing, because it commits as zero.
  - **The zeros are recomputed on both sides and not recorded (R3).** M46's event writes and
    `guest_bsdthread_create`'s kport write are the precedent.
  - An asymmetry surfaces at the next landmark's argument check or at the exit-time full-memory
    compare.
- **`NoOp`** changes nothing.
- **Return value.** Every accepted call returns 0.

**The arms** (`crates/retrace-core/src/lib.rs`):
- A record arm in `record_box` and a mirror in `ReplaySession::advance`, both placed **before** the
  generic forward and calling `guest_madvise` with the same arguments (symmetry rule 1).
- **On `Err`,** the record arm panics with an `M47:` message (R5). The mirror returns
  `Divergence { detail: "madvise: …" }`, as M46 §3g did.
- **The mirror compares** `ret`, `ret1`, `err`, and the recorded `writes`, which must be empty.
- **The mirror lives inside the existing `Syscall` chain,** so `verify_thread` stays at 7 sites
  (M46 §6).

**The structural guard.** The generic forward arm (`crates/retrace-core/src/lib.rs:1262-1274`) gains
an assert that `num != SYS_MADVISE`, beside its `kevent_qos` assert. Forwarding `madvise` then fails
loudly, rather than depending on arm order. Row 75's comment is rewritten: "never forwarded since
M47", with the row kept for the census and the views.

**Docs.** The `/bin/ps` class-E hazard is **retired** from `docs/current-state.md`'s Known limits. It
is not "reduced": the forward that caused it no longer exists.

### 3d. The `__mac_syscall` model (2A)

**Rows 381 and `0x8000_0000` become `[Path, Scalar, NestedDest]`.** The generic arm's existing
`writes_via_nested_pointer` assert then refuses any unmodelled `__mac_syscall` **structurally**,
which is what the `NestedDest` doc says this class needs.
- `forwarded_shape` and the views are re-checked for what the kind change does to them.
- The `WritesViaNestedPointer` view then disagrees with `legacy_writes_via_nested_pointer` for 381
  and `0x8000_0000`, so `legacy_equivalence.rs`'s `EXPECTED_DIFFS` gains one entry for each. Both
  are marked "exercised", since both numbers are in the census, with the reason: "M47: the policy
  writes through `outFlags` inside `arg`".

**`retrace-arch`** gains `mac_syscall_model(policy: &[u8], call: u32) -> Result<MacCall, String>`:
- `("AMFI", 0x5a)` is `AmfiDyldPolicy`;
- `("Sandbox", 2)` is `SandboxContinuity(..)`, whose payload is the rule t0 M2(b) measures (R7);
- everything else is `Err`, naming the policy (at most 32 bytes, `MAC_MAX_POLICY_NAME`) and the call.

The inline `MAC_SYSCALL_MAGIC` Sandbox arm (`crates/retrace-core/src/lib.rs:393-407`) is unchanged.

**The record arm** (in `record_box`, before the generic forward) works as follows:
1. It reads the policy string from `args[0]` and classifies it.
2. For **`AmfiDyldPolicy`:**
   - It reads `{inFlags, outFlags}` from the 16 bytes at `args[2]`.
   - It asks the host with a **host-owned** `{inFlags, &slot}`, so the host kernel never sees a
     guest address.
   - On success, it records an ordinary `Syscall` event whose one `Region` is the 8-byte `slot` at
     `outFlags`, and applies it.
   - On a host error, it records that error with no write, which is what a native failure looks
     like to dyld.

   This is the forward-and-record posture of `task_info`'s audit token (M2-taskinfo). The answer is
   the host's, about retrace's own process (R4).
3. For **`SandboxContinuity`,** it returns the continuity errno and writes nothing.

**The replay mirror** classifies the same way from guest memory. A mismatch is a `Divergence`. For
`AmfiDyldPolicy`, it checks that the recorded `Region`'s address equals the `outFlags` replay reads,
then applies it. The recorded answer is the truth on replay, so nothing is recomputed. For
`SandboxContinuity`, it recomputes and compares.

**The named consequence.**
- **What changes.** Every dyld guest's `amfiFlags` changes from 0, today's EFAULT, to the host's
  answer (`0x1df` on the probe host). That changes dyld's policy for the **whole corpus**, towards
  native.
- **What does not.** The guest environment is empty (§2a), so the `DYLD_*`-variable half of that
  policy has nothing to act on.
- **What remains exposed.** The `@path`, fallback-path and interposing halves could move a guest.
- **How it is checked.** The gate and the sweep (§3g) are the regression check. Any moved row is
  attributed with a base binary built from `427fa0a`, alternating with the swept binary, as M45 and
  M46 did.

### 3e. The `fork` refusal

**`retrace-arch`** gains `fork_refusal_errno(num) -> Option<u64>`, which is `Some(35)` (`EAGAIN`)
for 2 and `None` otherwise. It sits beside M38's `exec_refusal_errno` and shares its doc form.

**The arms.** A record arm and a replay mirror sit beside M38's exec arms
(`crates/retrace-core/src/lib.rs:1229`):
- they write nothing;
- they return the errno with the carry set;
- the recorder prints `[retrace] refusing fork (syscall 2): process creation is unmodelled;
  returning errno 35 without forwarding`, in M38's form. The gate asserts on that line.

**The pre-fork message.** `machmsg.rs` gains a `Route` for msgh_id 3403 to the guest task port,
shaped by t0 M3(a).
- **The candidate** is to decode, validate by value, and answer with a `KERN_SUCCESS`
  `mig_reply_error`, as the 3410 route does (`crates/retrace-core/src/machmsg.rs:62`, `:152`). It is
  never forwarded. The standard symmetric posture applies: replay recomputes the reply and
  byte-compares it.
- **Why this is faithful in the parent** (inferred, measured by M3(a)).
  `mach_ports_register` sets the port array a **child** would inherit. With no child ever created,
  the parent observes nothing but the reply.

**Halt H1.** If t0 M3(b) finds more than this one message to answer before `__fork`, or if the
parent handler's traps after a failed `__fork` are not already modelled, stop widening. Default-config
`commit` is then documented as a limit, and the gate passes `-c maintenance.auto=false`. That was the
operator's fallback (Q2).

**Why `EAGAIN` (R2).** It is `fork`'s documented failure when a process limit is reached. It is
fidelity, not continuity: before M47 a fork was a record error, with no errno for continuity to keep.
`vfork` (66) and every other process-creation path stay unmodelled and unrowed, unless t0 measures
them.

### 3f. The fixtures and the gate

The fixtures are C, in `crates/retrace-guest/c/`, built by `build.rs` like `after_dyn.c`, with path
constants in `retrace-guest`. Each is first run natively in t0 for its reference output. Each test
names the RED it had at `427fa0a`.

- **`fsops_dyn.c <dir>`** does the following, then prints markers:
  1. `mkdir <dir>/d`;
  2. `chdir` into it;
  3. create `f`;
  4. `link` `f` to `g`;
  5. `rename` `g` to `h`;
  6. `utimes` `h` to a fixed time;
  7. `stat` `h`.

  The test asserts on the markers and on the stat'd `mtime`, which is the effect `utimes` makes.
  RED: the M33 panic at 136.
- **`madv_dyn.c <mode>`:**
  - **`zero`:** map 1 MiB, fill it with `0xAB`, `MADV_ZERO` the first 512 KiB, then check that the
    first half is zero and the second half is still `0xAB`. RED: the guard-band panic.
  - **`reuse`:** fill, `FREE_REUSABLE`, `FREE_REUSE`, write a pattern, read it back. RED: none at
    `427fa0a` unless t0 M1(c) finds one. It guards the no-op semantics.
  - **`bad`:** an advice outside the measured set. RED: none, because it is forwarded today. After
    M47 it is the refusal, and the test asserts on the named value.
- **`rpath_dyn.c` with `librpath_dyn.dylib`,** linked with install name `@rpath/…` and `-rpath
  @executable_path`. The program calls into the dylib and prints its marker. The test also asserts
  that the AMFI landmark's recorded write has bit 0 (`ALLOW_AT_PATH`) set. RED: dyld's `@path`
  refusal, then the 521 panic.
- **`forkfail_dyn.c`** calls `fork()`, prints `fork failed errno=35` on −1, and `_exit`s in any child.
  The test asserts on that line and on the recorder's refusal line. RED: the 3403 `RECORD ERROR`.

**The gates** (the plan pins the exact list):
1. **`crates/retrace/tests/gitprims_e2e.rs`.** One test per fixture mode. Each records, replays
   twice byte-identically, and asserts the difference named above. `madv_dyn zero` also asserts that
   the trace holds a `madvise` landmark with **empty** writes, which is the recompute-not-record
   ruling.
2. **`crates/retrace/tests/git_e2e.rs`,** which skips loudly through `util::announce` where Xcode's
   `git` is absent. Each test builds a fresh temp repo natively with Xcode's `git`.
   - **Reads:** each in-list read command's stdout equals native, byte for byte, and two replays are
     identical.
   - **`add`:** after record, native `git ls-files --stage` lists the blob with the expected object
     id.
   - **`commit`, default config:**
     - the recorder's fork-refusal line is printed;
     - git's own "cannot fork" line is printed, matched against t0 M3(c)'s native text;
     - record exits 0;
     - native `git log -1 --format=%T` equals the tree id a native twin commit produces;
     - two replays are identical.
   - **The rest of M4's in-list,** one test each where the list grows.
3. **`crates/retrace/tests/node_e2e.rs`.** One test, `#[ignore]`d at `kevent` (363) with its
   measured reason, skipping loudly where `/opt/homebrew/bin/node` is absent. The skip is still
   announced when the test is run explicitly.

**Unit tests:**
- `retrace-arch` gains `tests/gitshapes.rs`:
  - the `MADV_*` values against the SDK;
  - a refusal sweep over `madvise_effect`;
  - `mac_syscall_model`, with the policy and call bit-flipped and truncated;
  - `fork_refusal_errno`;
  - the four new rows' kinds.
- `census.rs` gains 2, 9, 12, 136 and 138, and its doc gains the M47 paragraph.

**The existing gates** must stay green unchanged. The ones nearest the change are:
- `hello_dyn_e2e` and every dyld guest, because the AMFI flags change;
- `cpython_e2e` and `cpython_crash_e2e`, because of `madvise`;
- `sysbin_e2e`'s `ps`, because the hazard is retired;
- `exec_e2e`, because it sits beside the new fork arm;
- `jq_e2e`, `thread_oracle`, `checkpoint_seek` and `dispatch_e2e`.

### 3g. The walk

With the mechanisms landed:
1. **Walk `node`** to its wall and park `node_e2e` there. If the wall is not `kevent`, record what it
   is.
2. **Sweep.** The controller runs the Apple sweep **detached, on a signed scratchpad copy, with no
   concurrent `cargo`** (M45 T3-a). It diffs the result row by row against
   `docs/sweep-evidence/2026-09-30-m47-probe/sweep-427fa0a.log`.
3. **Attribute each moved row** against the `427fa0a` base binary, alternating with the swept one.
4. **`csh` and `tcsh`** now see a refused fork instead of a record error.
   - The walk measures what each does next.
   - Each either re-parks at a new measured wall, or counts as "passes only by refusal", marked the
     way the `xcrun` trio is in `docs/current-state.md`.
   - Either way, its `#[ignore]` reason is rewritten in the house form.

### 3h. The docs

- The status log gains a new M47 section.
- `docs/current-state.md` is edited in place:
  - What works gains `git` and `@rpath` guests;
  - Known limits:
    - retire the `madvise` hazard;
    - fork is refused with `EAGAIN`;
    - AMFI's answer is the host's;
    - node's wall;
  - the Apple-sweep figure and its idle-host confirmation.
- The README changes only where it states something that changed: its Limits list, and its headline
  if `git` belongs there.
- CLAUDE.md's gate list gains `gitprims_e2e`, `git_e2e` and `node_e2e`. Its "Guest threads" paragraph
  does not change.

## 4. Guards: each asserts the difference it makes

| Guard | What it catches | Proven able to fail by |
|---|---|---|
| `gitprims_e2e` fsops | a missing row | RED at `427fa0a` (M33 panic at 136) |
| `gitprims_e2e` madv `zero` | `MADV_ZERO` forwarded, or zeros recorded instead of recomputed | RED at `427fa0a` (guard band); control: record the zeros and watch the empty-writes assert go red |
| `gitprims_e2e` madv `bad` | an unmeasured advice accepted silently | control: widen `madvise_effect` and watch it go red |
| generic-arm `madvise` assert | `madvise` reaching the forward by arm order | control: move the M47 arm below the generic arm and watch every `madvise` guest panic |
| `gitprims_e2e` rpath | AMFI still forwarded, or answered 0 | RED at `427fa0a` (the 521 panic) |
| `NestedDest` on 381 | an unmodelled `(policy, call)` forwarded | control: drop the `Sandbox` arm and watch the generic assert fire |
| `gitprims_e2e` forkfail | fork reaching the host, or a record error | RED at `427fa0a` (3403) |
| `git_e2e` | git's workflow broken end to end, including the corruption | RED at `427fa0a` for every write command; for the corruption, t0 M1(b)'s reproduction rate says how strong a guard it is |
| `gitshapes.rs` | a validator accepting a bit it should not | the sweep itself |

Each control runs on the committed tree and is restored with `git checkout`, as in M45 and M46.

**Named weakness.** Unless t0 M1(c) finds a repo-owned trigger, the reclaim corruption is guarded by
construction (the assert) and by `git_e2e` only, and `git_e2e` skips without Xcode. The status log
says so.

## 5. Task order and why

1. **t0**, the measurements (§3a). No product code until H1, H2 and H6 are cleared.
2. **The rows and the census** (§3b). They are pure, and they unblock every read-only git test.
3. **The `madvise` model** (§3c): validator, box method, arms, assert, and the `madv_dyn` fixture as
   RED first.
4. **The `__mac_syscall` model** (§3d): the kind change, validator, arms, and `rpath_dyn` as RED
   first.
5. **The fork refusal** (§3e): validator, 3403 route, arms, and `forkfail_dyn` as RED first.
6. **`git_e2e` and `node_e2e`** (§3f), with the controls of §4.
7. **The walk and the sweep** (§3g).
8. **The docs** (§3h).
9. **The gate,** chunked as CLAUDE.md requires, then a file-by-file reconciliation against §9.

Tasks 3, 4 and 5 are independent of each other, but they are not run in parallel. Each touches
`record_box` and `ReplaySession::advance`, so they run in order on one branch.

## 6. Acceptance

- §1's five parts, each shown by the named gate test.
- `TRACE_MAGIC` is unmoved, and there is no `crates/retrace-trace` diff.
- `verify_thread` stays at 7 sites. The new mirrors live inside the `Syscall` chain.
- `#[ignore]` goes up by at most `node_e2e`'s one. Every other change is a reason rewritten in the
  house form.
- The full chunked gate is green. No `SKIPPED` line appears where the tool is present.

## 7. Halt rules, and what this milestone deliberately does not do

- **H1, fork.** More than one message to answer before `__fork`, or unmodelled parent-handler traps
  (§3e): fall back to the documented limit.
- **H2, Sandbox.** A continuity answer that is not a function of the arguments (§3d): stop and bring
  the choice between continuity and native behaviour to the operator.
- **H3, `madvise`.** A census advice whose native effect is neither a no-op nor a zero-fill, such as
  a query that reports status: keep it refused and route it. Stop only if a gate guest needs it.
- **H4, the format.** Stop if anything requires a trace-format change. M47 needs none: AMFI's answer
  is an ordinary `Region`, and every refusal is an ordinary `Syscall` event.
- **H5, widening.** A git command outside M4's in-list, or node past its wall, is recorded with its
  wall and routed. The milestone rests on §3f's gates.
- **H6, `chdir`.** t0 M5 finds retrace opening a relative path after the guest starts: stop and bring
  it to the operator.

**Not in M47:**
- timed waits (`sleep`, `nanosleep`, `__semwait_signal`, `__ulock_wait2`), routed out by the
  operator (Q1) and still owed from M46;
- real process creation, which covers `fork` beyond the refusal, `vfork`, exec-in-place, and
  `posix_spawn`;
- `kevent` on a guest `kqueue()`, which is node's next milestone;
- the V8 JIT;
- git's pager, editor, network, hooks, and any spawn other than auto-maintenance;
- jq's 300k abort, measured not to be this class (§2c);
- `*at` and `f*` twins that t0 does not measure;
- the `Sandbox` policy beyond continuity.

## 8. Rulings (made while writing this spec)

- **R1: `chdir` is forwarded.** It moves retrace's own current directory, which is correct on record
  and inert on replay (§3b), subject to M5.
- **R2: a refused fork returns `EAGAIN`,** the errno native `fork` documents. This is fidelity: there
  was no errno for continuity to keep.
- **R3: the `madvise` zeros are recomputed, not recorded.** M46 R3 is the precedent.
- **R4: AMFI's answer is the host's, about retrace's own process.** On the probe host this matches
  what an ad-hoc guest gets natively (`amfi.out`). For a platform-binary guest it may not, and that
  fidelity gap is documented in Known limits rather than synthesized away (approach 2B, rejected).
- **R5: a refusal is a panic on the record side and a `Divergence` on the replay side,** as in M46.
- **R6: no `TRACE_MAGIC` bump.** No `Event` shape changes, and no snapshot byte changes meaning.
- **R7: `Sandbox` call 2 keeps the answer it has today** (EFAULT or EINVAL, per t0 M2(b)'s rule),
  because the corpus passes with it. Changing it is out of scope. H2 guards the case where the rule
  cannot be stated by value.

## 9. Gate prediction (provisional; the plan pins it)

The prediction starts from M46's **898 / 0 / 9 over 152**, which t0 M6 re-derives. Expected
additions:
- `gitprims_e2e`: about 6 tests, a new binary;
- `git_e2e`: about 4 tests, or more with M4's list, a new binary;
- `node_e2e`: 1 test, `#[ignore]`d, a new binary;
- `retrace-arch` `gitshapes.rs`: about 6 tests, a new binary;
- `retrace-guest`: 4 fixture-path tests, in the existing binary.

That is roughly **918 / 0 / 10 over 156**. It could move by two if `csh`/`tcsh` un-ignore as "pass by
refusal". The plan computes the exact figure from its own test list, and the close reconciles
against it file by file.

## 10. Conformance with the governing documents

- **Symmetry rule 1.** Every new arm has its mirror in `ReplaySession::advance`, calling the same
  `Box_` or `retrace-arch` function with the same arguments, before the generic forward.
- **Symmetry rule 2.** `MADV_ZERO`'s zeros are recomputed by the shared box method on both sides.
  AMFI's answer is host data, and so it is recorded, as the audit token is.
- **"Never forward a nested pointer"** (`ArgKind::NestedDest`'s doc). Row 381 is brought under it,
  and the generic arm's existing assert enforces it.
- **The M24 lesson** (record-only state). M47 adds no `Box_` field. Everything it models is a pure
  function of the call and of guest memory.
- **Honest-gate discipline.**
  - `node_e2e` parks at a measured wall.
  - `csh` and `tcsh` move only on a measurement.
  - Every skip announces itself through `util::announce`.
  - The one guard `git_e2e` cannot give without Xcode is named in §4.
- **"Never assert on an exit code a weaker failure would also produce."**
  - `git_e2e` asserts output against native and repository state against a native twin.
  - `forkfail` asserts the errno line and the refusal line.
  - `rpath` asserts the recorded AMFI bit.
  - `madv zero` asserts the zeros and the empty writes.

## 11. Corrections from the plan

Writing the plan (`docs/superpowers/plans/2026-09-30-retrace-m47-gitwrite.md`) against the code
found these. Where this section and §1–§10 disagree, this section holds. Items 1 and 2 add to the
milestone's scope and are the operator's to approve.

1. **Row 333 was missing.** The probe's `g36` stopped at the M33 panic for `__pthread_canceled`
   (333), the fork path's `pthread_setcancelstate`, before it reached 3403 (`git-runs.txt`). §2d
   named the call and §3b omitted its row. The plan adds `333 => [Scalar]`, forwarded on the 331
   precedent ("noted, not modelled": retrace never cancels a thread), and adds 333 to the census.
2. **Fork's row lands with its refusal, and the generic arm gains a fork assert.** A row makes
   `forwarded_shape` accept 2, so from that commit on a missing refusal arm would forward `fork`
   and start a real child of the recorder. The row therefore lands in the same task as the refusal
   (plan Task 4), never earlier. Beside the `madvise` assert, the generic arm asserts
   `fork_refusal_errno(num).is_none()`, so "never forwarded" is a checked fact and not a matter of
   arm order. Task 4's control deletes the refusal arm and watches that assert fire.
3. **`guest_madvise(&self, args) -> Result<Vec<Region>, String>`**, not `Result<u64, String>`
   (§3c). It returns the zero-fill writes; both the record arm and the mirror apply them with
   `apply_and_return`, so the M5 watch check sees a zero-fill as a write. Every accepted call
   returns 0. A reserved page the guest never touched gets no write, because it commits as zero on
   first touch. It takes `&self`: the model commits nothing.
4. **`Box_::read_guest_cstr(va, cap)` is new.** The policy and operation names are shared-cache
   `__cstring`s the code computes an address for without loading from them, so their pages may be
   unstaged at the `svc`. The read pages in a missing cache page (`page_in_cache`), the same
   deterministic operation a guest load would have triggered, on both sides at the same landmark.
   It reads at most `cap` bytes, NUL included, as `copyinstr` does. The policy uses
   `MAC_MAX_POLICY_NAME` (32); the operation uses M47's own `SANDBOX_OPERATION_MAX` (64).
5. **Sandbox continuity is keyed by the operation name at `*(arg + 16)`**
   (`docs/sweep-evidence/2026-09-30-m47-probe/sandbox-call2.txt`): `syscall-unix` → 14 and
   `file-write-data` → 22. That is R7's "rule t0 M2(b) measures", measured while planning. t0
   re-measures it across the corpus (M2(a)). M2(b) now measures only the native answers, for the
   fidelity gap Known limits names.
6. **`MacCall::SandboxCheck`**, not `SandboxContinuity(..)` (§3d). The classifier returns the pair;
   `sandbox_check_continuity(operation)` supplies the errno; `Box_::guest_mac_syscall` returns
   `MacSyscall::{AmfiDyldPolicy { in_flags, out_ipa }, SandboxCheck { errno }}`. One record arm
   handles both pairs, so §4's "drop the `Sandbox` arm" control becomes "delete the `__mac_syscall`
   arm", and "move the arm below the generic arm" becomes "delete the arm". Each is the same
   breakage with the same expected assert.
7. **CLAUDE.md's "Guest threads" paragraph does change** (§3h said it does not). It lists the
   generic arm's asserts by name, and M47 adds two (`madvise`, `fork`).
8. **The 3403 decoder is exact.** Exactly 64 bytes, `COMPLEX`, id 3403, descriptor count 3, and
   each descriptor's type byte 0 (`MACH_MSG_PORT_DESCRIPTOR`), all per t0 M3(a). The names and
   dispositions are not modelled; if M3(a) measures `MOVE_SEND`, the decoder's doc names the extra
   user reference the model leaves in retrace's own IPC space.
9. **Two plan halts join §7:**
   - **H7:** t0 M2(a) finds a `(policy, call)` pair other than the two.
   - **H8:** a Sandbox call-2 operation other than the two, or a forwarded call-2 result that
     carried writes or differed between two runs of one guest. This is H2's case, measured.
10. **§9's prediction is superseded.** The plan counts +39 + k `#[test]` lines, where k is t0 M4's
    in-list write commands beyond `add` and `commit`. That is 946 + k passed-plus-ignored over 158
    binaries, 10 of them ignored unless `csh`/`tcsh` move.
