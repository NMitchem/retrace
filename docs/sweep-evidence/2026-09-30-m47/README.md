# Sweep evidence — M47 Task 6, run 2026-10-01

The directory keeps the plan's 2026-09-30 name (ledger Ruling P6). **Every file here was produced on
2026-10-01**, on this machine: macOS 26.5.2 (25F84), 12 CPUs.

Three measurements live here:
- **The walk, node** (Step 1). `node -e 'console.log(1)'` on the finished M47 build, to its wall.
- **The walk, `csh` and `tcsh`** (Step 2). What the two shells do now that `fork` is refused with
  `EAGAIN` and its prepare handler's `mach_ports_register` (3403) is answered.
- **The sweep** (Step 3). The full 54-entry corpus, swept once on this task's binary and diffed row by
  row against the unloaded `427fa0a` baseline (`docs/sweep-evidence/2026-09-30-m47-probe/sweep-427fa0a.log`).
  Every row that differs was then re-measured on the base binary, alternating with the swept one.

**Result: `TALLY pass=50 fail=4 skip=0`,** against the baseline's `pass=49 fail=5`.

- **One row moved its outcome: `/usr/bin/dddiagnose`,** `FAIL` 4/3 at msgh_id 205 → `PASS` 139/139
  (identical fault). This is **host state**, M45's coin flip: both faces occur on both binaries in the
  controls (below). It is not M47's.
- **Three more rows differ in the normalised diff**, with the same outcome (`FAIL`):
  - **`csh` and `tcsh`** moved their wall, from `RECORD ERROR … msgh_id 3403` (rc/rp 4/3) to a recorder
    panic at `wait4` (7), rc 101. **This is M47's**: the controls stop both shells at the 3403 on the
    base binary and at `wait4` on the swept one, every round.
  - **`automationmodetool`** moved only its panic line's source location, `lib.rs:1002:38` →
    `lib.rs:1036:38`. The wall is the same `M33: syscall 375 (375) has no arg_kinds row`, on both
    binaries. M47's new `arg_kinds` rows sit above `forwarded_shape` (the M44 precedent).
- **The xcrun trio still passes** (`desdp`, `dyld_info`, `flex`: `PASS` 71/71, as in the baseline), so
  the `("Sandbox", 4)` ENOTSUP answer the operator ruled for them (t0 H7) keeps them where they were.
- **No moved outcome is unexplained**, so nothing here is H5.

**The host was loaded.** The 1-minute load was **6.23 at the sweep's start** and ranged 2.78–6.88
during it (`sweep-load.txt`, every 30 s); the 5-minute load stayed between 4.00 and 5.03. The baseline
ran at 1.93 → 1.85. An unrelated project's test binaries (two processes at 100% CPU each, seen in
`ps` just before the sweep) shared the host; they were not touched. One of the load's units is the
sweep's own recorder. No row timed out except `yes`, which always does.

## Method

**The binary.** `t6-build.sh` ran `cargo build -p retrace` first (exit 0, a no-op at `a8a1ecd`).
Nothing was built after it until the sweep and the controls were done. The throwaway trace reader
(`tracedump.rs`, a scratchpad crate on `crates/retrace-trace` by path) was built before the sweep too.

**The sweep** was `tools/apple-sweep.sh`, run by this task alone, detached (Bash `run_in_background`),
with no `cargo` running (M45 T3-a, ledger Ruling T6-a). The wrapper is `t6-sweep.sh`:
- It copied `target/aarch64-apple-darwin/debug/retrace` to the session scratchpad as `retrace-t6` and
  signed it there ad hoc with `retrace.entitlements`, so no build could swap it mid-sweep.
- After signing, its sha256 is
  `246d6f2cfacd13b1266c3da1f9a2989e56cbdb286314958d68f158713ab87847`. The commit swept is `a8a1ecd`
  (M47 Task 5).
- It printed `pidstart`, the binary's hash, commit and date, `uptime` and `vm.loadavg` before and
  after, and the state of xcrun's cache at the start: `/var/tmp/xcrun_db` was present (16 bytes,
  2026-09-27), so the trio took the warm-cache path straight to `posix_spawn` (M44 t0 Ruling T0-e).
- A 30 s load sampler ran for the sweep's duration (`sweep-load.txt`).
- `RETRACE_SWEEP_KEEP=…/sweep` kept every non-clean row's evidence. The watchdog was the default, 30 s.

| `pidstart` | recpid range | `TALLY` | `SWEEP_EXIT` | ran |
|---|---|---|---|---|
| 99787 | 99814–5459 (the pid counter wrapped) | `pass=50 fail=4 skip=0` | 0 | 23:36:44–23:43:22 |

**The base binary** is t0's `/private/tmp/claude-501/m47-base-retrace`, sha256
`e4ae912e44c071e33ca978f9f78613b21709cac4a996b8ed50deaac01582defb` (t0's record, unchanged). t0 built
it from `aa16f01`. `git diff --stat 427fa0a aa16f01 -- crates Cargo.toml Cargo.lock tools` is empty, so its
code is the baseline's.

**The row diff** (`t6-rowdiff.sh`):
- `rowdiff.txt` is the brief's diff of `path` and `result` only (`rows-base.txt` against `rows.txt`). It
  marks the one moved outcome.
- `rowdiff-norm.txt` is M45/M46's normalised comparison. It keys on the path and compares `result`,
  `rc`, `rp`, `landmark` and `rec_reason`, with the Rust thread id normalised out. **50 rows are
  identical and 4 differ.**

**The controls** (`t6-controls.sh` → `controls.txt`) re-swept the four differing rows
(`moved-rows.txt`) with `tools/apple-sweep.sh` itself, through `RETRACE_SWEEP_LIST`, so each row is
judged by the sweep's own labels and watchdog:
- two rounds of all four rows, in the order base, t6, base, t6;
- then four more rounds of `dddiagnose` alone, alternating base and t6, because its outcome is a coin
  flip. Each binary has six `dddiagnose` samples.

Every control run recorded and replayed with one binary. `TRACE_MAGIC` differs between them
(`RT\x00\x0a` on base, `RT\x00\x0b` on t6), so no trace crossed binaries. The controls ran at 1-minute
loads of 4.33–12.37 (logged per round in `controls.txt`).

## The non-clean rows

| row | label | `rc`/`rp` | wall |
|---|---|---|---|
| `/bin/csh` | `FAIL` (recorder panicked) | 101 / n/a | `M33: syscall 7 (7) has no arg_kinds row` — `wait4`, the wait `remotehost` issues after the refused fork; class C, process creation (the walk, below) |
| `/bin/tcsh` | `FAIL` (recorder panicked) | 101 / n/a | the same (`tcsh` is the same file as `csh`) |
| `/usr/bin/automationmodetool` | `FAIL` (recorder panicked) | 101 / n/a | `M33: syscall 375 (375) has no arg_kinds row` — `kevent_id`, unchanged (M46 §7 H5) |
| `/usr/bin/yes` | `FAIL` (timed out after 30s recording) | 137 / n/a | never terminates; the watchdog, by design (unchanged) |

`dddiagnose` is a `PASS` here only as an identical fault (139/139), M45's `mfm_alloc` face: the kept
`sweep/dddiagnose.rec.err` ends `guest crashed: pc=0x180302eb0 far=0x4000050050 esr=0x92000045`, and the
trace's `Crash` is landmark 392, ten after the refused message-queue receive at 382. Its gate stays
parked (its reason already names both faces).

## Row-by-row diff against `427fa0a`

| row | `427fa0a` | M47 | moved by |
|---|---|---|---|
| `/usr/bin/dddiagnose` | `FAIL` 4/3, msgh_id 205, landmark 448 | `PASS` 139/139 (identical fault) | **host state**: both faces on both binaries (below) |
| `/bin/csh` | `FAIL` 4/3, 3403, landmark 338 | `FAIL` 101/n/a, `M33: syscall 7` | **M47**: the 3403 answered, the fork refused, the run reaches `wait4` |
| `/bin/tcsh` | `FAIL` 4/3, 3403, landmark 341 | `FAIL` 101/n/a, `M33: syscall 7` | **M47**, the same |
| `/usr/bin/automationmodetool` | `FAIL` 101/n/a, `lib.rs:1002:38` | `FAIL` 101/n/a, `lib.rs:1036:38` | **M47's rows**, a source line only; the wall is unchanged |

A `PASS` row carries no landmark in its `ROW` line, so the sweep cannot show the landmark moves that
M47's AMFI answer might cause on passing rows. On the rows that do carry one, the shells, the 3403 still
sits at 317 plus the guest's `gettimeofday` count (below), M45's and M46's constant: the AMFI call
changed its result, not the number of landmarks before the 3403.

### The controls, row by row

| row | base r1 | t6 r1 | base r2 | t6 r2 |
|---|---|---|---|---|
| `/bin/csh` | 4, 3403 | 101, `M33: syscall 7` | 4, 3403 | 101, `M33: syscall 7` |
| `/bin/tcsh` | 4, 3403 | 101, `M33: syscall 7` | 4, 3403 | 101, `M33: syscall 7` |
| `automationmodetool` | 101, 375, `lib.rs:1002` | 101, 375, `lib.rs:1036` | 101, 375, `lib.rs:1002` | 101, 375, `lib.rs:1036` |
| `/usr/bin/dddiagnose` | 4, 205 | 4, 205 | `PASS` 139 | `PASS` 139 |

**`csh` and `tcsh` are the rows whose outcome follows the binary.** Both base runs stop at the 3403,
and both t6 runs at `wait4`.

### `dddiagnose`: the coin flip, both faces on both binaries

| binary | 205 (`FAIL` 4/3) | `mfm_alloc` fault (`PASS` 139/139) | other |
|---|---|---|---|
| base (`427fa0a`'s code) | r1, d4 | r2, d3, d6 | d5: a third face (below) |
| t6 (`a8a1ecd`) | r1, d5, d6 | the sweep, r2, d3, d4 | — |

- Every fault is at pc `0x180302eb0`, esr `0x92000045`, with `far` varying run to run
  (`0x4000050050`, `0x2000050050`, `0x2000050060`), M45's face.
- The fault came up in 3 of 6 base runs and 4 of 7 t6 runs, more often than M45's "about one run in
  four". The sample is small, and the rate is the same on both binaries.
- **The move is host state.** The gate's `#[ignore]` reason already names both faces and is unchanged.

**A third face, on the base binary only, once** (`dddiagnose.d5-base.rec.err`, `dddiagnose.face3.txt`,
`t6-ddd-face.sh`): `RECORD ERROR: non-syscall exit: data abort (EC=0x24 ISS=0x7 FSC=0x7) far/ipa=0x1bf0
(UNMAPPED) pc=0x193bbbca0 elr=0x193bbbc9c`. lldb against the host's shared cache puts it at
libswiftCore `_swift_release_dealloc+48`, the `ldr x8, [x16, #-0x10]!` after the `autda` of an object's
metadata pointer: a Swift object whose isa read as `0x1c00`. It was seen on the **base** binary, in
1 of its 6 runs, and in none of the swept binary's 7. So it is not M47's. Whether it is the
heap-corruption class Task 3b's `mach_vm_map` mask fix removed (`docs/sweep-evidence/2026-09-30-m47-abort/`)
was **not measured**.

### `automationmodetool`: a source line

Its panic is `M33: syscall 375 (375) has no arg_kinds row` on both binaries, in every run. Only the
location moved, `lib.rs:1002:38` on base and `lib.rs:1036:38` on t6, because M47's rows were added
above `forwarded_shape`. Its gate's reason quotes M46's `1002:38` line as M46 measured it, and is
unchanged.

## The walk: node (Step 1)

**The wall is unchanged: `kevent` (363).** `t6-step1.sh` → `node-walk.txt`, `node.rec.err`:
- `/opt/homebrew/Cellar/node/25.6.1/bin/node`, `node --version` = `v25.6.1`; libuv 1.52.1, linked
  dynamically.
- The traced record exited **101** after 47 s, with **1,101 `[trap]` lines**, a 354,715,684-byte trace
  and no stdout.
- The recorder's stop line, verbatim:
  ```
  thread 'main' (17896196) panicked at crates/retrace-arch/src/lib.rs:1036:38:
  M33: syscall 363 (363) has no arg_kinds row in crates/retrace-arch/src/lib.rs — it cannot be forwarded unclassified (an untranslated guest fd would act on retrace's own descriptor of that number). Classify each argument from the SDK prototype under the rules in ArgKind's docs and add the row; if a guest in the corpora dispatches it, add the number to tests/census.rs too.
  ```
- The trap: `kevent` at pc `0x1804b3fc4`, **landmark 1,101**. The trace holds 1,101 events (the Snapshot
  and 1,100 syscalls, `torn=false`); the `kevent` was never appended (`node.landmarks.txt`).
- `node.rec.err` keeps the last 400 of its 1,319 lines, under a first line that says so (`t6-cap.sh`).

**Its arguments** (`node.entry.txt`, `t6-node-entry.sh`, read by `retrace debug` at the stub's svc,
pc `0x1804b3fc0`, position (1101, 27), thread 0):
- `kevent(7, 0x27ff368, 2, 0x27ff368, 1, 0x27ff358)`: two changes, and a one-entry event list at the
  same address.
- change 0: ident `0x1e7e7711`, filter `0xfff6` (-10, `EVFILT_USER`), flags `0x0021` = `EV_ADD|EV_CLEAR`.
- change 1: the same ident and filter, flags 0, fflags `0x01000000` = `NOTE_TRIGGER`.
- the timeout at `0x27ff358`: `{0, 0}`.

**Its descriptor** (`node.landmarks.txt`): fd 7 is what the **second** `kqueue()` returned, at landmark
1,100. The first `kqueue()`, at landmark 1,094, returned 4; then come `fcntl(4, F_SETFD, 1)`, `pipe` →
(5, 6), two more `fcntl`s, and a one-byte `write` to fd 6.

**Its caller** (`node.frame.txt`, `t6-node-frame.sh`): `x30` at the svc is `0xa0bd066c0`. In the host's
libuv, `0x66c0` is the return address of the `bl _kevent` at `0x66bc`, inside
`_uv__kqueue_runtime_detection` (an `nm` `t` symbol at `0x6640`). That function loads the ident
`0x1e7e7711`, the filter/flags word `0x21fff6`, the `NOTE_TRIGGER` word and a zeroed timeout, which
match the entry byte for byte. So node stops at **libuv's check that `EVFILT_USER` works, on a
throwaway kqueue**, not on its loop's descriptor (fd 4). This corrects the probe's wording ("on the
descriptor kqueue (362) returned — libuv's loop"): the descriptor is a kqueue's, but not the loop's.

**Elsewhere in the run** (`node.landmarks.txt`, `t6-node-jit.sh`): no `bsdthread_create` (360); no
`MAP_JIT` among its 121 `mmap`s; AMFI's dyld-policy `__mac_syscall` (0x5a) at landmark 59 answered
rc 0 with one write, the host's answer.

**The gate.** `node_e2e::node_prints_one_and_replays` keeps its body and its `#[ignore]`, with the
reason rewritten in the house form from these fields. It is class C, `kevent` on a guest `kqueue()`,
routed to node's next milestone (M47 §7).

## The walk: `csh` and `tcsh` (Step 2)

**Neither exits cleanly. Both re-park at `wait4` (7).** `t6-step2.sh` → `shells-walk.txt`:

| | `csh` | `tcsh` |
|---|---|---|
| record rc | 101 | 101 |
| `[trap]` lines | 341 | 345 |
| `gettimeofday` (116) events | 19 | 23 |
| 3403, answered (one write) | landmark 336 | 340 |
| `fork`, refused `ret=35 err=true` | 337 | 341 |
| `close(7)`, `read(6, …, 0x1000)` → 0, `close(6)` | 338–340 | 342–344 |
| `wait4(-1, …, 0, NULL)`, pc `0x1804b5478` | **341**, status at `0x27fe2cc` | **345**, `0x27fe2bc` |
| replay | rc 3, `DIVERGENCE at landmark 341 … expected recorded syscall, got None (truncated=false)` | rc 3, landmark 345 |
| stdout, record vs replay | `cmp` 0, both empty | `cmp` 0, both empty |

Each shell's record prints the refusal, `[retrace] refusing fork (syscall 2): process creation is
unmodelled; returning errno 35 without forwarding`, and then panics: `thread 'main' (17921385) panicked
at crates/retrace-arch/src/lib.rs:1036:38: M33: syscall 7 (7) has no arg_kinds row …` (csh; tcsh's
thread id is 17921529). The replay's `DIVERGENCE` is the trace with no terminal event that the panic
left, not a divergence of its own.

**The landmark** is 322 plus the guest's own `gettimeofday` count, on both shells. The 3403 sits at
317 plus that count, the constant M45 and M46 measured.

**Who forks and waits** (`csh.frames.txt`, `t6-shells-frames.sh`: the debugger at each svc on csh's
recording, the frame-pointer chain walked, each lr stripped of its PAC bits, lldb against `/bin/csh`
and the host's shared cache, unslid):
- the fork: libsystem_c `fork+56` ← csh `remotehost+108`;
- the `wait4`: `x30` is csh `remotehost+236`, ← csh `main+3120`.
- `/bin/csh` and `/bin/tcsh` are **one hard-linked file** (the same inode; `cmp` 0), so this applies
  to both, and tcsh's own trace has the same calls at the same pcs.

So `remotehost` makes a pipe and forks. It does not test fork's `-1`, so it takes the parent's path:
it closes the write end, reads EOF from the read end, closes it, and waits on `wait4(-1, &status, 0,
NULL)`. That call has no `arg_kinds` row.

**Native** (`shells-native.txt`, `t6-shells-native.sh`; `shells-forkfail.txt`, `t6-shells-forkfail.sh`):
- With this host's environment, both shells exit 1 with `Bad : modifier in $ '/'.` (`csh.native.err`,
  `tcsh.native.err`). This host's `~/.tcshrc` holds Docker Desktop's sh-syntax line `export
  PATH="$PATH:…"`, which is what prints it. That is not the guest's situation.
- retrace gives every guest an **empty** environment (`load_dynamic` pushes no `envp`). With an empty
  environment (`env -i`), both shells exit 0 silently.
- With an empty environment **and every fork failing `EAGAIN`** (`ulimit -u 1`), both still exit 0
  silently. The positive control under the same limit, `/bin/sh -c /usr/bin/true`, prints `/bin/sh:
  fork: Resource temporarily unavailable` and exits 128, so the limit was in force.
- Whether the native shells fork at all with an empty environment was **not traced**.

**The gates.** Both stay parked, so neither passes by refusal. `apple_walls_e2e`'s `csh` and `tcsh`
reasons are rewritten from these fields, class C (process creation: the `wait4` a refused fork's caller
issues), not routed, and the file's header comment gains one sentence saying M47 moved them there.

## Ruling

- **dddiagnose** moved its outcome by host state, the M45 coin flip. Both faces occur on both binaries
  in the controls.
- **csh/tcsh** moved by M47: the 3403 answer and the fork refusal carry them to `wait4`, and the
  controls show the move follows the binary. They are re-parked there, class C.
- **automationmodetool** moved only its panic's source line, because of M47's rows. Its wall is
  unchanged.
- **node**'s wall is unchanged, `kevent` (363), now named: libuv's `EVFILT_USER` runtime probe.
- **No moved outcome is unexplained.** The one observation without an attribution is a third
  `dddiagnose` face on the **base** binary (once in six), which is not M47's.

## Files

- **The sweep:**
  - `sweep.log`: the full detached log (wrapper lines, 54 `ROW` lines, `TALLY`, `SWEEP_EXIT`, loads);
  - `sweep-load.txt`: the 30 s load samples;
  - `rows-base.txt`, `rows.txt`, `rowdiff.txt`: the brief's outcome diff;
  - `rowdiff-norm.txt`: the normalised diff;
  - `sweep/<basename>.rec.err` for every non-clean row and for `dddiagnose`'s identical fault, with its
    `.rp.err`/`.rp.out`.
- **The controls:** `moved-rows.txt`, `controls.txt`, `dddiagnose.d5-base.rec.err`,
  `dddiagnose.face3.txt`.
- **node:** `node-walk.txt`, `node.rec.err` (capped), `node.rec.out` (empty), `node.entry.txt`,
  `node.frame.txt`, `node.landmarks.txt`.
- **The shells:** `shells-walk.txt`; `csh.{rec,rp}.{err,out}` and `tcsh.{rec,rp}.{err,out}` (the
  traced records, whole, and their replays); `csh.frames.txt`; `shells-native.txt`,
  `{csh,tcsh}.native.{out,err}`; `shells-forkfail.txt`.
- **The scripts**, with the session's scratchpad paths left as they ran: `t6-build.sh`,
  `t6-step1.sh`, `t6-node-entry.sh`, `t6-node-frame.sh`, `t6-node-jit.sh`, `t6-step2.sh`,
  `t6-shells-native.sh`, `t6-shells-forkfail.sh`, `t6-shells-frames.sh`, `t6-sweep.sh`,
  `t6-rowdiff.sh`, `t6-controls.sh`, `t6-ddd-face.sh`, `t6-cap.sh`, `t6-tests.sh`.
- `tracedump.rs`: the throwaway reader's source (`td`).
- **No `.bin` trace files are committed.**
  - The sweep kept five (`automationmodetool`, `csh`, `dddiagnose`'s identical fault, `tcsh`, `yes`).
    They were read, then removed (`t6-cap.sh`).
  - The walk's traces (`/private/tmp/claude-501/m47-{node,csh,tcsh}.bin`) and the controls' (in the
    scratchpad) were removed after this README's measurements had read them.
