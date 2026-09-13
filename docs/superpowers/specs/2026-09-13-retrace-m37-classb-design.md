# M37-classb — the two class-B walls: `dup2` modelled, and a `Scalar` is never a pointer

*Written 2026-09-13 from M36's table (`docs/sweep-evidence/2026-09-13-m36/README.md`, spec §11.7),
under the M32–M38 charter (`2026-09-09-retrace-m32-m38-program-charter-design.md` §3 "M37–M38",
§6, §9). Class B was small — two named gaps — so M37 takes both and M38 does not exist (§10).*

## 1. The walls, located

M36 routed exactly two class-B (known-unmodelled) walls to this milestone, in the table's order:

**Wall 1 — `dup2` (rows `/bin/csh`, `/bin/tcsh`).** `crates/retrace-core/src/lib.rs:1140`, the
generic forward arm of `record_box`:

```rust
assert!(num != retrace_arch::SYS_DUP2,
    "dup2 is not modelled by the M10 fd table (unexercised by any gate guest); \
     implement target-slot allocation before a guest uses it");
```

M10 built the guest's descriptor table (`FdTable`, `crates/retrace-box/src/lib.rs:691`) around
`alloc` = "lowest slot not currently open", which `open`/`dup`/`socket`/`kqueue` all obey.
`dup2(fd, fd2)` names its own target slot, the table has no operation for that, and M10 refused
it loudly rather than model it wrong. The refusal is the wall; the recorder panics (rc 101) and
the sweep labels the row `recorder panicked: … lib.rs:1140:17: dup2 is not modelled …`.

**Wall 2 — the §4b probe (rows `launchctl`, `automationmodetool`, `desdp`, `dyld_info`, `flex`,
`dddiagnose`).** `crates/retrace-box/src/lib.rs:3186–3204`, `forward_and_diff`'s per-register loop:

```rust
for i in 0..8 {
    match self.host_span(args[i]) {
        Some((hp, avail)) => { … hargs[i] = hp as i64; }
        None => hargs[i] = args[i] as i64,
    }
}
```

Every register whose value happens to be a mapped guest IPA is rewritten to a host pointer before
the syscall is forwarded — including registers that carry a **number**. M34 §4b found it on the
recorder's own pid: a `csops(pid, …)`/`proc_info(…, pid, …)` whose `pid` is a mapped IPA reaches
the host kernel as a host address and is answered `ESRCH`. M36 measured the consequence on six
corpus rows (11–12 self-pid `ESRCH` per trace at a colliding pid, 0 otherwise) and its window:
`[0x4000, 0x18000)` = pids 16384..=98303, ~82 % of the pid space, because the guest's own
`os_alloc_once` slab lands first-fit at IPA 0x10000 — so the collision set is *every backing
mapped at the moment of the forward*, and no band is safe to "avoid". The symbol behind the
downstream `brk` is libdispatch `_firehose_task_buffer_init+0x12c` on the failed
`proc_info(2, pid, 17)`; the second downstream face is an identical malloc crash.

The fix M34 named and M36 confirmed: **a register the `arg_kinds` row marks `Scalar` is never a
memory reference and must never be probed.** M33's table (`crates/retrace-arch/src/lib.rs:333`,
`arg_kinds`) documents `Scalar` as "documentation until a later milestone consults it"
(`ArgKind` rustdoc, `:140–142`). This is that milestone.

## 2. Measurement — taken before this spec was written

Charter §9 item 2: the wall must be measured where the spec says it is. Both were, and the first
measurement changed this milestone's shape.

### 2a. What `csh`/`tcsh` do with `dup2`, and what lies behind it

A throwaway probe (a local patch that let `dup2` through as `dup` + a table write, never
committed; its patch and both stderr logs are in the session scratchpad `m37pre/`) recorded
`/bin/csh` and `/bin/tcsh` with `RETRACE_TRACE=1`, stdin `/dev/null`. Both issue exactly four
`dup2` calls, each followed by `fcntl(new, F_SETFD, FD_CLOEXEC)`:

```
[trap] num=90 (0x5a) pc=0x1804b67bc args=[0x0,0x10,…]    dup2(0, 16)   host dup(0)=18
[trap] num=90 (0x5a) pc=0x1804b67bc args=[0x1,0x11,…]    dup2(1, 17)   host dup(1)=19
[trap] num=90 (0x5a) pc=0x1804b67bc args=[0x2,0x12,…]    dup2(2, 18)   host dup(2)=20
[trap] num=90 (0x5a) pc=0x1804b67bc args=[0x10,0x13,…]   dup2(16, 19)  host dup(18)=21
```

That is the C shell's classic descriptor move (`SHIN`/`SHOUT`/`SHDIAG`/`OLDSTD` to 16–19, so a
script's own redirections never clobber the shell's). Every source is a **console** descriptor or
an alias of one; every target is `>= 16` and free; nothing is displaced. Then both guests
`ioctl` the aliases (18: `TIOCGETA`, `TIOCGWINSZ`), `sigaction`/`sigprocmask`, and — 130 traps
later — stop at

```
RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64
```

whose backtrace `dladdr` reads as `_kernelrpc_mach_ports_register3+0x88` ← `mach_ports_register+0x80`
← libxpc `xpc_atfork_prepare+0x50` ← libSystem `libSystem_atfork_prepare+0x28` ← libsystem_c
`fork+0x24` ← `csh+0x47e50`. **The wall behind `dup2` is `fork`**: `mach_ports_register` (3403,
a complex message with three port descriptors, unknown to the router) is `fork`'s own pre-fork
hook, and the `fork` syscall (2) has no `arg_kinds` row either (`forwarded_shape` would refuse it
loudly). Process creation is a capability the tree does not have — charter class **C**, which M36
Ruling 1 parks and does not route.

**So M37 models `dup2` and does not free `csh`/`tcsh`.** Their gates move forward to the measured
next wall and stay parked there; the reason is rewritten (honest-gate discipline: move the gate,
rewrite the wall). The `dup2` work is still owed — it is the charter's type specimen of class B,
the table's gap is real, and the model is what any future shell-like guest needs — but the row's
retirement is not this milestone's claim. §7 says so again.

### 2b. The §4b window, and what the probe does to a scalar

M36's measurement stands as the wall's location (spec §11.3a, evidence README "Why run O
collided"). One more datum this spec relies on, measured at M36's Task 3 control and in the
scratchpad reader: at a colliding pid every `csops`(169)/`csops_audittoken`(170)/`proc_info`(336)
carrying the recorder's pid in a `Scalar` position (`args[0]`, `args[0]`, `args[1]`) is answered
`ret=3` (`ESRCH`) with `err=true`; at a non-colliding pid the same calls succeed. No other
syscall's error count moves between the regimes (M36 Table A: `err` 63 vs 75/71 on `dddiagnose`,
the delta entirely in 169/170/336). That is the measurement the fix's audit (§3b) extends.

## 3. Design

### 3a. `dup2`, modelled — the table learns about console aliases

`dup2(fd, fd2)` on POSIX: if `fd` is not open → `EBADF`; if `fd == fd2` → returns `fd2` (no-op);
otherwise `fd2` is silently closed if open, then becomes a duplicate of `fd` (same open file
description), and `fd2` is returned. Three things make retrace's version more than a table write:

1. **The source may be a console descriptor.** M9 mirrors writes to guest fd 1/2 into the trace
   and fakes them (never forwarded), and replay reproduces stdout from the mirror. That decision
   is made **by fd number** (`retrace_arch::is_console_write(num, fd)`: `fd == 1 || fd == 2`).
   After `dup2(1, 17)` a write to 17 is a console write and must be mirrored, or the recorder
   forwards it to a host dup of its own stdout — the output appears on the terminal, the trace
   holds nothing, and replay prints nothing (M9's 397 defect, re-opened through a new door).
2. **The target may be a console descriptor.** After `dup2(f, 1)` a write to 1 is a write to
   `f`'s file, not a console write. By-number mirroring would silently swallow it into the trace's
   stdout while the file stayed empty.
3. **Replay executes no syscall**, so the guest-visible half of the table must be a pure function
   of the guest's own sequence and the recorded `ret` must be recomputable (symmetry rule 1, the
   fd mirror's existing posture at `retrace-core/src/lib.rs:2340`).

**The design: the console is a slot kind, not a number.**

- `FdSlot` gains `Console(u8)` — "this slot is (an alias of) console descriptor `n`", `n ∈ 0..3`.
  `FdTable::new()` becomes `[Console(0), Console(1), Console(2)]` with the identity host mapping
  as now. `is_open` is true for `Open | Console(_)`; `alloc` skips both; `close` marks `Closed`
  as now; `from_slots` rebuilds the identity host mapping for a `Console(n)` slot **at index n**
  only (an alias slot has no host mapping on a restored box, and needs none — replay forwards
  nothing).
- `FdTable::dup2(&mut self, fd: u64, fd2: u64, host_fd2: Option<i32>) -> Result<u64, u64>` —
  the pure table operation, identical on both sides: `Err(EBADF)` if `fd` is not open; `Ok(fd2)`
  if `fd == fd2`; otherwise slot `fd2` takes slot `fd`'s **kind** (`Console(n)` propagates,
  `Open` stays `Open`), its host mapping becomes `host_fd2`, and the displaced host mapping (if
  any) is returned to the caller through an out-parameter for closing. Guest-visible state moves
  identically on record (with a real `host_fd2`) and replay (`None`).
- Record side, in `forward_and_diff` — **not** a new record arm, so M10's "one function owns both
  halves of the fd contract" still holds: `if num == SYS_DUP2 { return self.guest_dup2(args); }`
  at the top, before `translate_fds`. `guest_dup2` looks up `fd`'s host mapping (`EBADF` if none),
  `libc::dup`s it (the errno if that fails), calls `FdTable::dup2` with the new host fd, closes
  the displaced host mapping **iff it is > 2** (a displaced identity mapping of retrace's own
  0/1/2 is dropped, never closed — the M9 hazard), and returns `(fd2, false, vec![])`. Nothing is
  forwarded as `dup2` to the host: forwarding `dup2(h, fd2)` would overwrite retrace's OWN
  descriptor `fd2`.
- Replay side, inside the generic syscall arm of `ReplaySession::advance` beside the existing
  `allocates_fd`/`close` mirrors (`retrace-core/src/lib.rs:2340–2356`) — **not** a new returning
  arm, so no new `verify_thread` site is created (CLAUDE.md's seven-sites rule): recompute
  `(ret, err)` with `FdTable::dup2(args[0], args[1], None)` and byte-compare against the recorded
  pair; a mismatch is a `Divergence` naming both. Then `apply_and_return` as for any landmark.
- The console predicates become table-driven at their three call sites (`retrace-core/src/lib.rs:153`
  trace-log echo, `:235` record mirror arm, `:1817` replay mirror) through ONE shared method,
  `Box_::is_console_write(num, gfd)` = `retrace_arch::is_write_syscall(num) && self.fds.console_of(gfd)
  ∈ {1, 2}`; and `is_console_close` likewise fakes the close only when the slot is `Console(_)`
  (a displaced console slot — `Open` after `dup2(f, 1)` — is a real descriptor and closes through
  the generic path, retiring its slot). `retrace_arch::is_console_write` is deleted, not kept as a
  second predicate that could drift; `is_write_syscall`/`is_close_syscall` remain the pure
  number tests.
- `arg_kinds`: `SYS_DUP2 => row!(P, [Fd, Scalar])` — the target is the guest's own slot number,
  never translated; `EXPECTED_DIFFS` in `legacy_equivalence.rs` records the `FdOperands` view's
  change with the reason and "exercised (/bin/csh, /bin/tcsh)" (90 is in the census). The `dup2`
  paragraph of the `Ret` rustdoc (`:278–282`) and the `SYS_DUP2` tests (`:1347`, `:1489–1491`)
  say what is true now.

**What `dup2(0, 16)` then does on record**: slot 16 = `Console(0)`, host = `dup(0)`; a read on 16
translates to the dup and forwards, exactly as a read on 0 does today; a write on 17
(`Console(1)`) is mirrored and faked; `close(16)` goes the generic way (host close of the dup,
slot `Closed`); `close(1)` on the still-`Console(1)` slot 1 is faked as M9 does.

### 3b. The §4b fix — `Scalar` positions skip the probe, and the table is audited first

In `forward_and_diff`'s loop, a position the row marks `Scalar` is forwarded **verbatim**:

```rust
let shape = retrace_arch::forwarded_shape(num);
for i in 0..8 {
    if shape.args.get(i) == Some(&ArgKind::Scalar) { hargs[i] = args[i] as i64; continue; }
    match self.host_span(args[i]) { … }
}
```

Only `Scalar`. `Fd` positions are already host descriptors (small integers) by then; `Ptr`,
`Path`, `Source`, `Dest`, `Nested*` are memory references and keep the probe; positions **beyond
the row's arity keep the probe too** — M30 measured that a stale register pointing into a live
buffer plants a canary 64 KiB past itself, and the windows those registers open are part of the
capture the band logic reasons about; narrowing that is a separate measurement this milestone
does not take (§7).

**The audit is the precondition, not a formality**: a position wrongly marked `Scalar` would,
after this change, hand the host kernel a guest IPA as a pointer — an `EFAULT` where there was
none, or a read of retrace's own memory at that address. M34 named the audit as the fix's
precondition; it is three measurements, each recorded in the evidence README:

1. **Static**: every `Scalar` position in every row of `arg_kinds`, listed with the prototype it
   is checked against (`sys/syscall.h`, `syscalls.master`, the Mach trap prototypes) — a table in
   the report, one line per (num, position). A position whose prototype says pointer is a table
   defect to fix in this milestone and record as a finding.
2. **Dynamic, pre-fix**: over every kept corpus trace (M36's `keep-{O,L,I}/`, M35's `ddd-keep*/`,
   plus a fresh baseline sweep with **every** trace kept — §3c), for every landmark and every
   `Scalar` position, no recorded `writes` region contains `args[i]`'s value. The kernel never
   wrote through a "scalar" — else the table is wrong there.
3. **Dynamic, post-fix**: the same full-corpus sweep on the fixed binary at a **non-colliding**
   pid (so §4b cannot confound), compared row by row with the pre-fix baseline: every label
   identical, and per binary the count of `err=true` landmarks **per syscall number** identical.
   Any difference is adjudicated by name in the report; an `EFAULT` (`ret=14`) that appears on a
   syscall whose row has a `Scalar` position is the audit catching a defect, and the row is fixed.

### 3c. The harness: keep everything, once

`tools/apple-sweep.sh` gains `RETRACE_SWEEP_KEEP_ALL=1`: with `RETRACE_SWEEP_KEEP` set, keep
every row's files (PASS rows included). One env, one condition in `keep_row`. The script takes
the binary path as `$1` already, so the pre-fix baseline runs the **main** binary (`648d4cf`,
built once into the scratchpad) and the post-fix sweep runs the branch's — same script, same
list, same pid regime, different binary.

### 3d. The gates move

`crates/retrace/tests/apple_walls_e2e.rs` is rewritten, not extended: the eight gates stay
`#[ignore]`d, each reason the **new** measurement (§5): `csh`/`tcsh` at the `fork` wall (class C:
`mach_ports_register` 3403 from `xpc_atfork_prepare`, then `fork` itself — parked, not routed),
the six §4b rows at the RCV-shaped `mach_msg2` **on every pid** (class C, parked, not routed; the
B half retired by this milestone with the acceptance sweeps as evidence). The M36 evidence
directory is untouched (history); this milestone's evidence goes to
`docs/sweep-evidence/2026-09-13-m37/`.

### 3e. Two comment corrections M36 owed

`crates/retrace-core/src/machmsg.rs:97–99` ("`brk` regardless of which of seven refusal codes …
parked at that wall") and `crates/retrace-box/tests/truncguard.rs:237` ("a pid in 16384..=65535
hits the trampoline/page-table backings") — each one sentence, to what M36 measured, with a
pointer to the M36 evidence README.

## 4. Positive controls (charter §9 item 4)

Each new guard must be watched failing, and the failure recorded in the task report:

1. **`dup2_e2e` vs the console alias.** With `Box_::is_console_write` reverted to the by-number
   predicate, the fixture's `write(17, "alias\n")` is forwarded to the host dup of stdout: the
   bytes reach the recorder's captured stdout but not the trace, replay's stdout lacks them, and
   the gate's record-vs-replay stdout comparison fails. Run it; paste the assertion.
2. **`dup2_e2e` vs the replay mirror.** With the replay-side `FdTable::dup2` call removed, slot 1
   stays `Console(1)` on replay after the fixture's `dup2(f, 1)`, replay mirrors the later
   `printf` into its stdout while the recording sent it to the file, and the comparison fails.
3. **`scalarprobe` vs the probe.** The static fixture `scalarprobe.s` opens `/etc/hosts` and
   calls `lseek(fd, 0x4000, SEEK_SET)` — `0x4000` is `TRAMPOLINE_IPA`, mapped on every path.
   Before the fix `forward_and_diff` rewrites the offset to the trampoline's host address and
   `lseek` returns that address (a 47-bit number); after it, `0x4000`. The box test asserts
   `ret == 0x4000` and is run red on the pre-fix tree first. It needs no colliding pid, which is
   why it is the unit control and the sweeps are the acceptance measurement.
4. **The acceptance sweeps** (§5) are the end-to-end control for §4b: 0 self-pid `ESRCH` in every
   kept trace at two colliding bands, and the six rows at the RCV wall in all three regimes.

## 5. Acceptance — what M36 ruled this milestone must show

Three full sweeps on the fixed binary with every trace kept: recorder pids inside
`[0x4000, 0x10000)`, inside `[0x10000, 0x18000)`, and below `0x4000`. Expected, identical across
all three: `csh`/`tcsh` → `record error, rc=4: RECORD ERROR: unsupported mach_msg2 … msgh_id 3403`;
the six §4b rows → `record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34:
options 0x404000102 …`; `yes` → the watchdog; everything else `PASS`; tally 45/9/0 each; and
`csops`/`csops_audittoken`/`proc_info` with the recorder's pid answered `ESRCH` **zero** times in
every trace (the M36 counting rule). The two M36 acceptance criteria ruled at its close are
exactly these. A row that does not match is a finding, adjudicated in the ledger; a `dddiagnose`
identical-fault or `brk` face at any pid means the fix did not reach that path and is a red.

## 6. Symmetry obligation (charter §9 item 3)

- **`dup2`**: record computes `(ret, err)` in `forward_and_diff` via `FdTable::dup2` (+ a host
  `dup`); replay recomputes `(ret, err)` via the same `FdTable::dup2` inside the generic arm and
  byte-compares — the standard posture (rule 1), the fd mirror's own shape. Neither side adds a
  returning arm, so `verify_thread`'s seven sites stay seven. The console predicates read the same
  table on both sides through one method.
- **§4b**: `forward_and_diff` is record-only (replay forwards nothing), so there is no mirror to
  keep in step; the change alters what the host kernel is *asked*, never what is recorded or
  compared. The determinism argument is that a scalar forwarded verbatim is what a real process
  sends.

## 7. What this milestone deliberately does not do

- **Does not free `csh`/`tcsh`.** Behind `dup2` is `fork` (§2a): process creation is class C and
  stays parked. Servicing `mach_ports_register` alone would move the wall one trap, to `fork`(2)
  itself, and is not attempted.
- **Does not model `fcntl(F_DUPFD)`/`F_DUPFD_CLOEXEC`** — M10's other named gap; no corpus guest
  issues it (census: `fcntl` is present, its `F_DUPFD` commands are not in any kept trace). Left
  named.
- **Does not make `close(1)`/`close(2)` on a `Console` slot retire the slot** — M9's deferral
  stands for console slots; only a *displaced* console slot (now `Open`) closes for real.
- **Does not narrow the probe beyond `Scalar`** — positions past a row's arity and `Fd` positions
  keep it; the stale-register band interaction (M30) is unmeasured for a narrower probe.
- **Does not service the RCV-shaped `mach_msg2`** (class C) — the six rows stay parked there.
- **Does not touch `TRACE_MAGIC`** — no event shape changes; `FdSlot` is box state, never traced.

## 8. Rulings (made while writing this spec)

1. **`dup2` is in scope although it frees no row.** Charter §3 routes class B by the table, and
   the table routed it; the measurement that `fork` lies behind it changes the claim, not the
   work. Cost if wrong: a model with no corpus consumer beyond a fixture — but every shell-like
   guest issues it, and the fixture is a real consumer.
2. **The console is a slot kind.** The alternative — a parallel `console_alias` vector — would
   split guest-visible state across two fields that checkpoints must carry together. One enum
   variant carries it through `slots()`/`from_slots` for free. Cost if wrong: `FdSlot` is public
   (`retrace_box::FdSlot`) and one match arm in every consumer.
3. **The audit is three measurements, all in this milestone.** A static-only audit is what M33
   did, and M33's rows were right; the dynamic halves are what make "never a pointer" a measured
   claim. Cost if wrong: two extra full sweeps (~8 min).
4. **The gates are rewritten in place**, not duplicated: one file, eight gates, each reason the
   current wall. The M36 reasons live in git history and the M36 evidence directory.

## 9. Gate

Prediction, from source: new tests — `FdTable::dup2` unit tests in `retrace-box` (four),
`crates/retrace-box/tests/scalarprobe.rs` (one, new binary), `crates/retrace/tests/dup2_e2e.rs`
(two, new binary); `apple_walls_e2e` stays eight ignored; `legacy_equivalence` unchanged in count.
575 + 7 = **582 passed / 0 failed / 10 ignored over 128 binaries**, to be reconciled file by file
against M36's 575 / 0 / 10 over 126.

## 10. M38

Class B is small and this milestone takes all of it; **M38 does not exist** (charter §3). The
status log says so at close.

## 11. Outcome

*(Written at close.)*
