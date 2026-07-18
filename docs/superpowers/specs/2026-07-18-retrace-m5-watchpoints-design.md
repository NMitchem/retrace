# retrace M5 — write watchpoints & reverse-continue-to-last-writer

**Design spec — 2026-07-18.** The first post-M4 milestone. M4 closed with checkpointed seeks merged
(`ed3cf8b`, gate 106/0/0 incl. fast-follow): repeated nearby seeks cost a handful of single-steps
instead of thousands ([M4 design](2026-07-16-retrace-m4-checkpoints-design.md)). The debugger can now
*move* cheaply in both directions — but it can only stop on **instruction addresses** (`break`,
DBGBVR0-5). It cannot yet answer the question a reverse debugger exists to answer: **"who wrote this
byte?"** M5 adds write watchpoints — `watch <addr> [len]` — that stop `continue` at the next write
and `reverse-continue` at the most recent write before P, covering **both** kinds of writer this
system has: guest store instructions *and* recorded kernel writes applied during replay.

## The problem, precisely

In replay, watched memory changes through exactly two funnels:

1. **Guest stores**, executed by the vCPU inside a trap window. Hardware watchpoints (DBGWVR/DBGWCR)
   are built for exactly this and run at silicon speed — no per-instruction exits, unlike a
   single-step-and-compare scheme (which would reintroduce the cost M4 just eliminated).
2. **Recorded kernel writes**, applied by `Box_::apply_and_return`
   (`crates/retrace-box/src/lib.rs:1629`) as host-side `ptr::copy_nonoverlapping` — e.g. a `read()`
   filling a buffer, `fstat` filling a statbuf. These never execute a guest store, so **hardware
   comparators can never see them**. A watchpoint feature that ignores them would silently skip the
   actual writer whenever the writer was the kernel — worse than useless for the "who wrote this"
   question, because it would confidently point at the wrong store.

M5 therefore has two detection paths feeding one user-facing feature: hardware watchpoints for guest
stores, and a software range-intersection at the single `apply_and_return` funnel for kernel writes.
Both produce ordinary `(N,K)` position coordinates, so the existing reverse-continue scan machinery
("replay forward, remember the last hit strictly before P") absorbs them without new seek machinery.

## Verified facts (this repo — read directly, HEAD `ed3cf8b`)

- **Breakpoint machinery to mirror.** `DBGBCR_ARM = 0x1E5` / `MDSCR_MDE = 1 << 15` /
  `HW_BREAKPOINT_SLOTS: [(SysReg, SysReg); 6]` (`retrace-box/src/lib.rs:110-113`);
  `arm_hw_breakpoint(slot, va)` (`:1454`) writes DBGBVRn/DBGBCRn and ORs `MDSCR_EL1.MDE`;
  `clear_hw_breakpoints()` (`:1466`) zeroes the slots **and clears MDE unconditionally** — a
  latent conflict M5 must fix, because MDE also enables watchpoints.
- **Delivery plumbing already exists.** `set_trap_debug_exceptions(true)` routes *all* debug
  exceptions to the VMM (called from `load`/`restore`/`from_checkpoint`). `Box_::run()`'s generic
  arm captures `last_far = e.virtual_address` and returns `Stop::Other { esr }` for any EC it
  doesn't special-case — breakpoints surface this way today, watchpoints would too. `retrace-arch`
  already decodes `0x34 | 0x35 => Ec::Watchpoint` (`retrace-arch/src/lib.rs:34`). No `Stop` enum
  change is needed.
- **The hardware inventory is already probed.** `spikes/README.md:31` (hvprobe): **6 breakpoint
  slots / 4 watchpoint slots** on this silicon. The watchpoint cap is 4, not 6.
- **The sysreg constants are already generated.** bindgen's output contains
  `HV_SYS_REG_DBGWVR0_EL1..DBGWVR15_EL1` / `DBGWCR0..15_EL1`; `hv-sys/src/lib.rs` simply never
  re-exported them (the DBGB block is at `:75-86`). Exposing DBGWVR0-3/DBGWCR0-3 is eight
  constant lines, no build.rs change.
- **One replay engine.** Debug-CLI replay (`ReplaySession` via `checkpointed_seek`,
  `retrace-core/src/lib.rs:917`) and plain `retrace replay` (`:954`) share
  `ReplaySession::advance()` (`:437`) → `apply_and_return`. A hook at `apply_and_return`'s write
  loop is automatically shared. On record, `record_box` calls the same function — the watch set is
  empty there, so record behavior is bit-identical.
- **Reverse-continue is a forward rescan.** `cmd_reverse_continue`
  (`crates/retrace/src/debug.rs:340`) rescans from `(1,0)`, arms breakpoints per landmark scan,
  records the last hit strictly before P via `resolve_hit_k` (`:120` — single-steps *unarmed* to
  match the hit pc, converting a pc-only hardware hit to exact `(N,K)`), and reseeks. Hits
  identified by `(N,K)` tuples order totally; new hit kinds slot into the same loop.
- **Test raw material exists.** `debug_cli.rs` discovers all addresses/coordinates from the
  freshly-recorded trace at test time (never hardcoded); `asm/fileio.s` (`retrace_guest::FILEIO`)
  already performs `fstat` (256-byte statbuf write) and `read` ("the money case: kernel writes
  file bytes into buf") — a ready-made deterministic syscall-write target, currently unused by any
  debug test.

## The mechanism

### M5-spike — `spikes/dbgw.c` (pre-implementation)

F3 proved hardware *breakpoints* deliver direct-to-EL2, pre-retire, PC == DBGBVR0. Watchpoints
should behave analogously (architecturally: synchronous, reported **before the store retires**,
FAR = accessed address), but that claim is load-bearing for hit semantics and has never been run on
this OS/silicon under HVF. `dbgw.c` copies `sstep.c`'s recipe (guest at EL0, VBAR trampoline,
`set_trap_debug_exceptions(true)`; `clang -framework Hypervisor`, ad-hoc codesign with
`ent.plist`, perl fork/alarm wrapper) and must establish, empirically:

- **F4a** — a store to a watched address exits to the VMM with `ESR_EL2` EC=0x34, not via the
  guest's VBAR.
- **F4b** — `hv_vcpu_exit`'s `virtual_address` (FAR) holds the accessed address.
- **F4c** — whether the store retired (read back the target byte). Pre-retire is expected and is
  what the hit semantics below assume; if it retires, the documented fallback applies (see Risk
  register).
- **F4d** — BAS byte-masking works: a store to a *non*-selected byte of the watched doubleword does
  not fire; a store to a selected byte does.

Findings land in `spikes/README.md` per house convention before any `retrace-box` code is written.

### M5-hw — hardware watchpoints (retrace-box, hv-sys)

- `hv-sys`: expose `DBGWVR0_EL1..DBGWVR3_EL1` / `DBGWCR0_EL1..DBGWCR3_EL1` in `sysreg`, copying the
  DBGB block.
- `retrace-box`:
  - `HW_WATCHPOINT_SLOTS: [(SysReg, SysReg); 4]`.
  - `DBGWCR_ARM_BASE`: `E=1` (bit 0), `PAC=0b10` EL0-only (bits 2:1), `LSC=0b10` store-only
    (bits 4:3); per-watch `BAS` (bits 12:5) = `((1 << len) - 1) << (addr & 7)`.
    `DBGWVRn = addr & !7` (the comparator works on an 8-byte-aligned doubleword + byte select).
  - `arm_hw_watchpoint(slot, va, len)` / `clear_hw_watchpoints()`, same shapes and the same
    load-bearing comment as the breakpoint pair: armed only around `advance()`/`run()` scans,
    **NEVER while single-stepping** (`step()`, `step_insns`, `resolve_hit_k` all run disarmed).
  - **MDE sharing fix.** `Box_` tracks `bps_armed: bool` / `wps_armed: bool`; both `clear_hw_*`
    functions zero only their own value/control registers and then recompute
    `MDSCR_EL1.MDE = bps_armed || wps_armed`. Today's unconditional MDE clear in
    `clear_hw_breakpoints` would otherwise silently disarm live watchpoints (the CLI clears
    breakpoints on landmark-boundary hits while a watch scan may still be in flight).
- A hardware hit surfaces as `Stop::Other { esr }` with EC=0x34 and `last_far` already captured —
  no `Stop` change, no `run()` change.

### M5-soft — kernel-write detection (retrace-box)

- `Box_` gains `watch_ranges: Vec<(u64 /*va*/, u64 /*len*/)>`, set/cleared in lockstep with the
  hardware slots, and `syscall_watch_hit: Option<SyscallWatchHit { watched_va, write_ipa }>`.
- In `apply_and_return`'s `for w in writes` loop: before the copy, intersect
  `[w.ipa, w.ipa + w.bytes.len())` with each watch range; on the first overlap, record the hit
  (first overlap wins; subsequent overlaps in the same event are not queued — documented).
  The copy itself is **never** skipped or altered — detection is observation, not interference.
- `take_syscall_watch_hit()` accessor for retrace-core. On record and on plain `replay`,
  `watch_ranges` is empty: the loop's added cost is an is-empty check, and behavior is unchanged.
- Below-the-trace writes (shared-cache demand-paging, `commit_reserved_page`) do **not** pass
  through `apply_and_return` and stay invisible to watchpoints by design — consistent with their
  below-the-trace status everywhere else in the system.

### M5-core — surfacing hits (retrace-core)

- `ReplaySession::arm_watchpoints(&[(u64, u64)])` (assert ≤ 4) / `clear_watchpoints()` /
  `far() -> u64`, mirroring the breakpoint trio.
- `advance()` gains one arm alongside the existing `Ec::Breakpoint` match, **before** the
  cache-fault/FPAC fallbacks: `Ec::Watchpoint` → `Advance::Watch`.
- In the syscall-event branch, after `apply_and_return`: if `take_syscall_watch_hit()` returns a
  hit, return `Advance::WatchSyscall { watched_va }` instead of `Advance::Event` (the event **is**
  fully consumed first — state advances identically; only the report differs).
- `Advance` becomes `{ Event, Exited(ReplayReport), Break, Watch, WatchSyscall { watched: u64 } }`
  (`Watch` carries nothing: the CLI reads `far()`/`pc()` from the parked session, exactly as
  `Break` reads `pc()` today).

### M5-cli — commands & semantics (retrace/src/debug.rs)

- **`watch <addr> [len]`** — len ∈ {1, 2, 4, 8}, default 8; `addr` must be naturally aligned to
  len (guarantees one BAS doubleword = one slot, no silent splitting). `Exec.watches:
  Vec<(u64, u64)>`, sorted/deduped, capped at 4:
  `cannot arm more than 4 watchpoints (hardware limit: DBGWVR0-3)`.
- **`unwatch <addr>`** — removes; no-op report if absent. (`delete` stays breakpoint-only.)
- **Hit semantics (pre-retire).** A hardware hit parks *at the storing instruction, before the
  store executes*: `hit watch {watched:#x} (write at {pc:#x}) at ({n}, {k})`, K resolved from the
  hit pc by the existing `resolve_hit_k`. One `stepi` + `x` then shows the new value. A syscall
  hit parks at the post-event boundary: `hit watch {watched:#x} (syscall write) at ({n}, 0)` where
  n is the landmark index *after* consuming the writing event.
- **Progress rule (hardware hits only).** `Exec` remembers `last_watch_hit: Option<(usize, u64)>`
  — the position of the most recent reported *hardware* watch hit. If `continue` or
  `reverse-continue` starts parked exactly there, it pre-steps one instruction unarmed before
  arming (otherwise the un-retired store re-fires forever). It fires **only** when parked on a
  reported hardware hit, so a store the user manually stepped up to is never silently skipped.
  Syscall hits never set it: they park *after* their writer at a boundary, where a pre-step would
  wrongly skip the first instruction of the new window if that instruction is itself a watched
  store. This deliberately mirrors the parked-on-breakpoint pre-step in `cmd_continue`.
- **`cmd_continue`**: arms both sets (`arm_breakpoints` + `arm_watchpoints`) per scan; handles
  `Advance::Watch` (resolve K, park, print) and `Advance::WatchSyscall` (park at boundary, print);
  `Advance::Break`/`Event`/`Exited` behavior unchanged.
- **`cmd_reverse_continue`**: the scan loop treats `Break`, `Watch`, and `WatchSyscall` uniformly
  as candidate hits with `(N,K)` coordinates; keeps the last strictly before P; resumes each scan
  from `(n, k+1)` exactly as today (which also naturally steps over the pre-retire store,
  disarmed). Boundary syscall hits at `(n, 0)` order against mid-window hits `(n', k)` by plain
  tuple comparison — no special cases.

## Correctness invariant

Watchpoints observe; they never mutate. Nothing about M5 may change what executes, what is written,
what enters the trace, or what existing commands print:

1. The trace format is untouched (`TRACE_MAGIC` unchanged); record's code path through
   `apply_and_return` sees an empty watch set and is behaviorally identical.
2. `apply_and_return` applies every recorded byte exactly as before — detection happens beside the
   copy, never instead of it. The divergence oracle is untouched.
3. Watchpoints are armed only around scans, never during single-stepping — so `resolve_hit_k`,
   `step_insns`, and every checkpoint capture/restore run exactly as at M4.
4. All existing golden transcripts (7 `debug_cli`, `reverse_debug_e2e`, `checkpoint_seek`) remain
   byte-identical: M5 changes what *can* stop a scan, never what existing commands print.

## Scope

**In:** the `dbgw.c` spike; DBGWVR0-3/DBGWCR0-3 exposure; `arm_hw_watchpoint`/
`clear_hw_watchpoints` + MDE-sharing fix; `apply_and_return` watch-range intersection;
`arm_watchpoints`/`clear_watchpoints`/`far()`; `Advance::Watch`/`WatchSyscall`; `watch`/`unwatch`
commands with continue/reverse-continue integration and the progress rule; `asm/watchloop.s` test
guest; tests + README M5 Status section.

**Out (named, deferred):** read/access watchpoints (`rwatch`/`awatch` — LSC supports it; doubles
the CLI/test surface for a rarer use case); old→new value printing on hit (presentation, not
capability); ranges wider than 8 bytes or crossing a doubleword (multi-slot or software fallback);
symbol- or expression-based watch addresses; watchpoint hits during plain `retrace replay` (the
feature is debugger-only; `replay` never arms watches).

## Exit criterion

The M5 headline gate: a scripted debug session on a freshly recorded `WATCHLOOP` trace proves, in
one byte-exact transcript, **(a)** `watch` + `continue` stops at the first store with exact
`(N,K)`, and **(b)** `reverse-continue` from the end finds the *last* store — plus a second
transcript on `FILEIO` proving **(c)** a `read()`-driven kernel write to a watched buffer is
reported as a syscall watch hit and found again by `reverse-continue`. `just gate` green from
106/0/0 upward with **0 ignored**, clippy clean, all pre-existing transcripts byte-identical.

## Testing

All addresses/coordinates discovered from the freshly recorded trace at test time (house rule —
`record-dyn` landmark indices are not run-stable; `WATCHLOOP`/`FILEIO` data addresses are exported
constants from `retrace-guest`, not magic numbers in tests).

1. **Spike findings** recorded in `spikes/README.md` (F4a-F4d) — gate for starting M5-hw.
2. **Box-level:** `hw_watchpoint_fires_on_store_with_far` (arm slot 0, run a store, assert
   `Stop::Other` with `Ec::Watchpoint` + correct `last_far`); `bas_masks_unwatched_bytes` (store to
   a non-selected byte of the doubleword does not fire); `mde_shared_by_breaks_and_watches`
   (`clear_hw_breakpoints` leaves an armed watchpoint live, and vice versa).
3. **New asm guest** `asm/watchloop.s` (static, `SPINLOOP`-shaped): N stores to a fixed exported
   data address with distinct values, one `write(1, …)`, `exit`. Exported as
   `retrace_guest::WATCHLOOP` + `WATCHLOOP_DATA_VA`. `watchloop_guest_parses` smoke test.
4. **CLI parse tests:** `watch`/`unwatch` grammar, len/alignment validation errors, the 4-cap
   message, dedupe.
5. **Golden transcripts (`debug_cli.rs`):** forward first-hit at exact `(N,K)`; progress rule
   (continue after a hit advances past it, and a manual `stepi`-up-to-the-store then `continue`
   still hits it); `reverse-continue` finds the last store; `unwatch` then `continue` runs to the
   next breakpoint/exit.
6. **Syscall-write transcripts (`FILEIO`):** watch `buf` → `continue` reports the `read` landmark's
   syscall hit at `(n, 0)`; `reverse-continue` from later finds it; watch `statbuf` covers the
   `fstat` case.
7. **Regression:** all existing transcripts byte-identical; full `just gate` with
   `--test-threads=1`.

## Risk register

- **F4c falsifies pre-retire** (store retires before the exception reports): hit semantics change
  to "parked just after the store" — the CLI parks at `(n, k+1)`, the printed line says
  `(write completed at …)`, and the progress rule becomes unnecessary for hardware hits. This is a
  semantics tweak confined to M5-cli; the architecture is unchanged. The spike settles it before
  any implementation.
- **HVF rejects DBGW sysreg writes** (constants generated but untested): fallback is approach B
  from the design discussion (single-step + compare) — grossly slower, and it would demote M5 to
  "correct but slow"; the spike settles this on day one, before any retrace code is written.
- **FAR imprecision** (FAR reports the doubleword base or an in-range-but-not-first byte): the
  printed `watched` address comes from matching FAR against armed ranges, not from trusting FAR
  verbatim; F4b tells us how much to trust.
- **MDE regression risk:** the sharing fix touches the breakpoint path. Covered by
  `mde_shared_by_breaks_and_watches` plus the existing breakpoint transcripts (which would catch a
  breakpoint that stopped firing).
- **Reverse-continue cost:** each watch hit found on the way back to P pays a `resolve_hit_k`
  single-step walk, exactly like breakpoint hits. M4's `CheckpointCache` already amortizes the
  rescans; a store-heavy loop with thousands of hits before P would still be slow — accepted for
  M5 (same posture as M3 accepted slow seeks before M4 existed), and `watchloop.s` keeps N modest.

## Components

| Crate | Change |
|---|---|
| `hv-sys` | +8 `sysreg` constants (DBGWVR0-3, DBGWCR0-3) |
| `retrace-arch` | none (`Ec::Watchpoint` already decoded) |
| `retrace-trace` | none (no format change) |
| `retrace-guest` | +`asm/watchloop.s`, `WATCHLOOP`/`WATCHLOOP_DATA_VA` constants |
| `retrace-box` | `HW_WATCHPOINT_SLOTS`, `DBGWCR` encoding, `arm_hw_watchpoint`/`clear_hw_watchpoints`, MDE-sharing fix, `watch_ranges` + `apply_and_return` intersection, `take_syscall_watch_hit` |
| `retrace-core` | `arm_watchpoints`/`clear_watchpoints`/`far()`, `Ec::Watchpoint` arm in `advance()`, `Advance::Watch`/`WatchSyscall`, syscall-branch hook |
| `retrace` | `watch`/`unwatch` commands, hit handling in `cmd_continue`/`cmd_reverse_continue`, progress rule, tests |
| `spikes` | `dbgw.c` + README findings |

## Open questions for implementation planning

- Exact `Advance::WatchSyscall` payload (watched va only, vs. also the write's ipa/len for the
  printed line) — decide when writing the transcript format.
- Whether `arm_watchpoints` shares `cmd_continue`'s arming site or the two arm calls are fused into
  one `arm_all` helper — cosmetic, decide in-plan.
- `watchloop.s` store count and values (small enough to keep `resolve_hit_k` walks trivial, large
  enough that first-hit ≠ last-hit is a real assertion).
