# M42-llsc Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Single-stepping, seeking and native debug stops must keep an AArch64 exclusive pair's
store-exclusive, exactly as the recording ran it, with no change to the trace or to step semantics.

**Architecture:**
- **The shadow.** `Box_` gains `excl: Option<Excl>`, a shadow of the PE's local exclusive monitor
  (spec §3a).
  - It is set when `step()`'s exit reports that a load-exclusive retired (ISS.ISV + ISS.EX, t0 M8).
  - Every non-debug exit clears it, and so do `clrex`, the emulated store and a thread switch.
- **The emulated store.** `step()` at a store-exclusive with the shadow set validates the store
  (pure, fail-loud), raises any breakpoint or watch stop the hardware would, then writes the bytes,
  zeroes the status register and advances pc (§3b, §3c).
- **`run()` inside a pair.** `run()` entered with the shadow set steps the sequence to its end
  first, bounded at 16 steps (§3e).
- **Native stops.** A native breakpoint or watchpoint stop infers the shadow by a backward scan
  (§3d).
- **Carried, and pinned.** `BoxState` carries the shadow. Record and plain replay assert it is
  never set.

**Tech Stack:** Rust 1.95.0 (pinned), Hypervisor.framework via `hv-sys`, arm64 asm guest fixtures
built by `retrace-guest/build.rs`.

**Spec:** `docs/superpowers/specs/2026-09-24-retrace-m42-llsc-design.md` (commit `7ccae96`, amended
in this plan's commit). Its measurements are in `2026-09-24-retrace-m42-llsc-measurements.md` beside
it, cited as "t0 M1–M8". Read both before starting.

## Global Constraints

- **Toolchain:** `1.95.0`, target `aarch64-apple-darwin`.
- **`-- --test-threads=1` on every test command.** HVF allows one VM per process.
- **Clippy stays clean:** `cargo clippy --workspace --all-targets -- -D warnings`. `clippy.toml`
  bans `Instant::now`, `SystemTime::now` and `std::thread::Thread`. The bounded-run helper polls
  with `std::thread::sleep` and counts iterations. It never reads a clock.
- **No trace-format change.** `TRACE_MAGIC` stays `RT\x00\x0a`, and `Event` does not change.
  `BoxState` is memory-only, so a field there is no format change.
- **No dispatch-arm change** in `record_box` or in the arms of `ReplaySession::advance`.
  - `verify_thread` stays at **seven** call sites.
  - The only record-path edit is Task 3's one `assert!` after `b.run()`.
- **`Box_` field order is load-bearing** (`vcpu`, then `vm`, then `backings`). `excl` is declared
  **last**. Never reorder.
- **No new `#[ignore]`.**
- **Fail loud by panic** from `Box_` (spec R4). Refusals come from the pure functions in
  `crates/retrace-box/src/excl.rs`, and each refusal names its check.
- **Existing debugger transcripts stay byte-identical:**
  - `debug_cli`, `watch_cli`, `watch`, `watch_dyn`
  - `watchsweep_e2e`, `thread_watch_e2e`, `hitorder_e2e`
  - `crashy_cli`, `crashy_e2e`, `reverse_debug_e2e`
  - `checkpoint_seek`, `cpython_crash_e2e`, `sigcatch_dyn_e2e`
  - `debug.rs`'s unit tests

  The spec predicts no moved assertion, because no existing fixture steps an exclusive pair. If one
  moves, **stop and report it. Do not edit the expectation** (spec §7).
- **Halt rules (spec §7):**
  - The fixture's recording shows a store-exclusive **succeeding** after a `clrex`, a trapped
    timebase read or a syscall between its halves: Task 2 Step 3 reports it.
  - An existing test's output moves.
  - A nondeterministic record/replay difference.
- **Addresses:**
  - `llsc`'s are resolved by symbol (`nm`), never hardcoded.
  - `threadrust`'s landmark is discovered from the trace.
  - The t0 K positions for windows 1–4 are literal constants, because they are the measured facts
    this milestone flips. Task 3's M7 test re-derives every window length and would catch a drift.
- **Spawn the CLI through `util::bin()`**, which codesigns a copy.
- **Every `llsc` CLI run goes through `util::debug_bounded`.** t0 measured hangs on these scripts,
  so a regression must fail its test, never stall the gate.
- **Commit messages end with**
  `Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>`.
- **Grep logs with `grep -a`** (they carry ANSI and UTF-8).
- **Measure CPU seconds (user + sys from `/usr/bin/time -l`) and counts, never wall-clock.** The
  operator runs concurrent sessions.
- **The ledger** is `.superpowers/sdd/2026-09-24-retrace-m42-llsc/`. It is excluded through
  `.git/info/exclude` and never committed. Logs named in steps go there.
- **The session runs in the worktree `.claude/worktrees/m42-llsc`.** The harness there refuses:
  - any command prefixed with `VAR=value` (write `export VAR=value` on its own line instead);
  - `git -C <other checkout>`;
  - heredocs whose text mentions git.

  Keep shell commands simple: one command per line where you can. To capture cargo's exit code
  before any pipe, write the log with `> file 2>&1; echo "exit=$?"` in ONE shell call: each tool
  call is a fresh shell, so an `echo` in a separate call reports 0. The guard also refuses
  `/usr/bin/time -l cargo …`; put that line in a script under the session scratchpad and run the
  script (Task 2 measured the baselines that way). `--no-fail-fast` is a cargo flag and goes
  BEFORE `--`; libtest rejects it after.
- **Controls (deliberate breakages) run only on a COMMITTED tree.** Commit the task's
  implementation first, then apply one control, run its test, and undo it with
  `git checkout -- <file>`, which restores the committed version. Confirm `git status --short`
  lists no modified tracked file before the next control. `git checkout -- <file>` on a file that
  holds uncommitted work destroys that work. Never use `git stash` (it is shared across worktrees).
- **An implementer never dispatches subagents.**
- **Execution rulings are numbered from R13.** R1–R8 are the spec's. R9–R12 are this plan's.

## Plan-time rulings

- **R9: spec §3e (`run()` entered inside a pair) lands in Task 3, not with §3d.** The debugger
  resumes native execution from a stepped position (`stepi` inside a pair, then `continue`). Without
  the prologue, M2 stays red however exact the stepping is. The spec's §5 is amended to match.
- **R10: `run()`'s stepping prologue is bounded at 16 steps**, then drops the shadow. After a
  branch-out (h), or dyld's `getpid` finding its cache filled, the shadow can outlive its sequence
  indefinitely, and an unbounded prologue would single-step to the next syscall. That is correct,
  but it costs one VM exit per instruction. The residual it leaves is recorded in the spec's §3e and
  becomes a README line at the close.
- **R11: a raised watch stop's FAR is the lowest byte of the access that an armed range covers.**
  It lies in both the access and the watch, so `watched_of` resolves it by exact byte. The spec's
  first draft said the access start, which reaches a watch on a pair's second element only through
  `watched_of`'s 64-byte fallback. Spec R5 is amended.
- **R12: an asynchronous exit (vtimer or cancel) inside `run_one_for_step` leaves the shadow.** The
  spec's §3a clears on every non-debug exit because "the non-debug exits are the exits record also
  takes, at the same instruction". An asynchronous exit is not one of those, since record never
  takes it at the same instruction. Inside a step it is retrace's own exit, like the step exit
  itself. Clearing there would make a stepped pair's outcome depend on host timing: the E2 flake
  class. `run()`'s arm still calls `note_exit(false)`: the shadow is always clear there (§3e), and
  the call keeps "every exit arm classifies" true. The spec's §3a is amended.

## The fixture's coordinates (from the source in Task 2, and t0 for windows 1–4)

Window `n` ends at landmark `n`. The session opens at `(1, 0)`. A position `(n, len)` is **on**
window `n`'s trap instruction, and that is where `reverse-stepi` and the terminal park land.

| n | Shape | Window length | Key K |
|---|---|---|---|
| 1 | (a) discard-status | 16 | `a_ldx` 3, `a_stx` 5, `a_svc` 16 |
| 2 | (b) retry ×3 | 33 | `b_ldx` 5, `b_stx` 7 / 14 / 21, `b_done` 25, `b_svc` 33 |
| 3 | (c) CAS | 19 | `c_ldx` 6, `c_stx` 9 |
| 4 | (d) pair | 16 | `d_ldx` 4, `d_stx` 7 |
| 5 | (e) `clrex` between | 14 | `e_ldx` 3, `e_clrex` 4, `e_stx` 5 |
| 6 | (f) trapped timebase read between | 14 | `f_ldx` 3, `f_mrs` 4, `f_stx` 5 |
| 7 | (g) first half, ends at `getpid` | 5 | `g_ldx` 3, `g_svc` 5 |
| 8 | (g) second half | 9 | `g_stx` 0 |
| 9 | (h) branch-out | 13 | `h_ldx` 3, `h_cbnz` 4, `h_after` 5 |
| 10 | (i) exit window | 10 | `i_ldx` 4, `i_stx` 6, `i_svc` 10 |

What the recording holds at each landmark, as `(num, x3, x4)`:

| Landmark | num | x3 | x4 |
|---|---|---|---|
| 1 | 4 | `0x4242` | 0 |
| 2 | 4 | 3 | 3 |
| 3 | 4 | 9 | 1 |
| 4 | 4 | 11 | 1 |
| 5 | 4 | 1 | 0 |
| 6 | 4 | 1 | 0 |
| 7 | 20 | 1 | 0 |
| 8 | 4 | 1 | 0 |
| 9 | 4 | `0x4242` | 0 |

The run ends in `exit(0)`, and stdout is `a\nb\nc\nd\ne\nf\ng\nh\n`.

## Review Focus

The five inputs a debugger user is most likely to hit that the spec implies but its named
regressions do not cover. Each has its test in the owning task:

1. **`stepi` into a pair, then `reverse-stepi`, then `continue`.** The user expects the step back to
   land on the previous instruction, with the replay still exact after it. Task 3, test
   `reverse_stepi_inside_a_pair_then_continue_replays_to_the_end`.
2. **A checkpoint captured inside a pair and restored later.** The user expects the restored session
   to finish the pair as the recording did. Task 3, test
   `a_checkpoint_taken_inside_a_pair_restores_the_shadow`.
3. **`x` on the cell across the emulated store.** The user expects the old value before the
   `stxr` and the stored value after it. Task 3, test
   `x_shows_the_cell_before_and_after_the_emulated_store`.
4. **A breakpoint on the load-exclusive itself.** The native stop must infer nothing, since the load
   has not run yet, and both directions must count it. Task 5: the A3 arming includes `a_ldx`, and
   test `a_native_stop_on_the_load_itself_infers_nothing`.
5. **The terminal park single-stepping a window that holds a retry loop** (M41's owed item). The user
   expects the end of the replay, not a hang. Task 3: M2's final `where` is `(10, 10)`. Task 5: the
   A2 and A3 armings each arm `i_svc`.

---

### Task 1: Decode the exclusive-monitor instructions (`retrace-arch`)

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs`. Add after `decode_aut_rd` (around line 1085), and add
  tests in `mod tests` (around line 1393).

**Interfaces:**
- Produces:
  - `pub enum ExclInsn { Load { size: u8, pair: bool, rt: u32, rt2: u32, rn: u32 }, Store { size: u8, pair: bool, rs: u32, rt: u32, rt2: u32, rn: u32 }, Clrex }`,
    which derives `Debug, Clone, Copy, PartialEq, Eq`;
  - `pub fn decode_excl(insn: u32) -> Option<ExclInsn>`;
  - `pub fn is_fallthrough_barrier(insn: u32) -> bool`.
- In both variants, `size` is the bytes of **one element**, and `rt2` is 31 for the single forms.

- [ ] **Step 1: Write the failing tests** (in `mod tests`)

```rust
    #[test]
    fn decode_excl_reads_t0s_words() {
        use ExclInsn::*;
        // (a) and dyld's getpid: ldxr w10, [x9]; stxr wzr, w0, [x9]
        assert_eq!(decode_excl(0x885f_7d2a), Some(Load { size: 4, pair: false, rt: 10, rt2: 31, rn: 9 }));
        assert_eq!(decode_excl(0x881f_7d20), Some(Store { size: 4, pair: false, rs: 31, rt: 0, rt2: 31, rn: 9 }));
        // (b): ldaxr x1, [x0]; stlxr w2, x1, [x0]. (c): stlxr w2, x4, [x0]
        assert_eq!(decode_excl(0xc85f_fc01), Some(Load { size: 8, pair: false, rt: 1, rt2: 31, rn: 0 }));
        assert_eq!(decode_excl(0xc802_fc01), Some(Store { size: 8, pair: false, rs: 2, rt: 1, rt2: 31, rn: 0 }));
        assert_eq!(decode_excl(0xc802_fc04), Some(Store { size: 8, pair: false, rs: 2, rt: 4, rt2: 31, rn: 0 }));
        // (d): ldxp x1, x2, [x0]; stxp w3, x4, x5, [x0]. And stlxp w9, x1, x2, [x3]
        assert_eq!(decode_excl(0xc87f_0801), Some(Load { size: 8, pair: true, rt: 1, rt2: 2, rn: 0 }));
        assert_eq!(decode_excl(0xc823_1404), Some(Store { size: 8, pair: true, rs: 3, rt: 4, rt2: 5, rn: 0 }));
        assert_eq!(decode_excl(0xc829_8861), Some(Store { size: 8, pair: true, rs: 9, rt: 1, rt2: 2, rn: 3 }));
    }

    #[test]
    fn decode_excl_covers_every_width_the_acquire_pair_and_an_sp_base() {
        use ExclInsn::*;
        assert_eq!(decode_excl(0x085f_7c01), Some(Load { size: 1, pair: false, rt: 1, rt2: 31, rn: 0 }));  // ldxrb w1, [x0]
        assert_eq!(decode_excl(0x485f_7c01), Some(Load { size: 2, pair: false, rt: 1, rt2: 31, rn: 0 }));  // ldxrh w1, [x0]
        assert_eq!(decode_excl(0x887f_0801), Some(Load { size: 4, pair: true, rt: 1, rt2: 2, rn: 0 }));    // ldxp w1, w2, [x0]
        assert_eq!(decode_excl(0xc87f_8801), Some(Load { size: 8, pair: true, rt: 1, rt2: 2, rn: 0 }));    // ldaxp x1, x2, [x0]
        assert_eq!(decode_excl(0x0802_7c01), Some(Store { size: 1, pair: false, rs: 2, rt: 1, rt2: 31, rn: 0 })); // stxrb w2, w1, [x0]
        assert_eq!(decode_excl(0xc85f_7fe1), Some(Load { size: 8, pair: false, rt: 1, rt2: 31, rn: 31 })); // ldxr x1, [sp]
    }

    #[test]
    fn decode_excl_rejects_every_neighbour_that_does_not_touch_the_monitor() {
        assert_eq!(decode_excl(0x0820_7c82), None); // casp w0, w1, w2, w3, [x4]: o1 = 1 but size<1> = 0
        assert_eq!(decode_excl(0x88df_fc01), None); // ldar w1, [x0]: o2 = 1
        assert_eq!(decode_excl(0x889f_fc01), None); // stlr w1, [x0]
        assert_eq!(decode_excl(0x88a0_7c41), None); // cas w0, w1, [x2]
        assert_eq!(decode_excl(0xb940_0001), None); // ldr w1, [x0]
        assert_eq!(decode_excl(0xd503_201f), None); // nop
    }

    #[test]
    fn decode_excl_reads_clrex_with_any_crm() {
        assert_eq!(decode_excl(0xd503_3f5f), Some(ExclInsn::Clrex)); // clrex (CRm = 15, the default)
        assert_eq!(decode_excl(0xd503_305f), Some(ExclInsn::Clrex)); // clrex #0
        assert_eq!(decode_excl(0xd503_3f9f), None);                   // dsb sy: same space, not clrex
    }

    #[test]
    fn fallthrough_barriers_are_exactly_the_unconditional_transfers() {
        // b, bl, ret, br x16, blr x16, retaa, svc #0x80, brk #0
        for w in [0x1400_0002u32, 0x9400_0002, 0xd65f_03c0, 0xd61f_0200, 0xd63f_0200, 0xd65f_0bff, 0xd400_1001, 0xd420_0000] {
            assert!(is_fallthrough_barrier(w), "{w:#010x} is a barrier");
        }
        // cbnz, b.lo, tbz, ldr, nop, ldxr, add: all fall through
        for w in [0x3500_004au32, 0x5400_0103, 0x3600_0040, 0xb940_0001, 0xd503_201f, 0x885f_7d2a, 0x8b00_0020] {
            assert!(!is_fallthrough_barrier(w), "{w:#010x} falls through");
        }
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p retrace-arch -- --test-threads=1`
Expected: FAIL to compile. `decode_excl`, `ExclInsn` and `is_fallthrough_barrier` are not defined.

- [ ] **Step 3: Implement** (after `decode_aut_rd`)

```rust
/// M42: an AArch64 exclusive-monitor instruction. `size` is the bytes of ONE element (1, 2, 4 or 8);
/// a pair moves two. Register fields are raw, so 31 means XZR/WZR in `rs`/`rt`/`rt2` and SP in `rn`,
/// as the architecture reads them. `rt2` is 31 for the single forms, where the field is
/// should-be-one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExclInsn {
    /// `LDXR`/`LDAXR` (B, H, W, X) and `LDXP`/`LDAXP` (W, X).
    Load { size: u8, pair: bool, rt: u32, rt2: u32, rn: u32 },
    /// `STXR`/`STLXR` (B, H, W, X) and `STXP`/`STLXP` (W, X). `rs` receives the status.
    Store { size: u8, pair: bool, rs: u32, rt: u32, rt2: u32, rn: u32 },
    /// `CLREX`, any CRm.
    Clrex,
}

/// M42: decode `insn` as a load-exclusive, a store-exclusive or `CLREX`, or None.
///
/// The load/store-exclusive class is `size:2 001000 o2 L o1 Rs o0 Rt2 Rn Rt` with `o2 == 0`:
/// - `o2 == 1` is `LDAR`/`STLR`/`LDLAR`/`STLLR` and the single-register CAS family, which never touch
///   the monitor.
/// - A pair (`o1 == 1`) exists only with `size<1> == 1`. `size<1> == 0` with `o1 == 1` is CASP, which
///   the pair masks exclude by requiring bit 31.
pub fn decode_excl(insn: u32) -> Option<ExclInsn> {
    if insn & 0xFFFF_F0FF == 0xD503_305F { return Some(ExclInsn::Clrex); }
    let (rt, rn, rt2, rs) = (insn & 0x1F, (insn >> 5) & 0x1F, (insn >> 10) & 0x1F, (insn >> 16) & 0x1F);
    let single = 1u8 << (insn >> 30);                        // size<1:0>: 1, 2, 4, 8 bytes
    let paired: u8 = if (insn >> 30) & 1 == 1 { 8 } else { 4 }; // size<0> is sz: X or W elements
    if insn & 0x3FE0_0000 == 0x0840_0000 { return Some(ExclInsn::Load { size: single, pair: false, rt, rt2: 31, rn }); }
    if insn & 0xBFE0_0000 == 0x8860_0000 { return Some(ExclInsn::Load { size: paired, pair: true, rt, rt2, rn }); }
    if insn & 0x3FE0_0000 == 0x0800_0000 { return Some(ExclInsn::Store { size: single, pair: false, rs, rt, rt2: 31, rn }); }
    if insn & 0xBFE0_0000 == 0x8820_0000 { return Some(ExclInsn::Store { size: paired, pair: true, rs, rt, rt2, rn }); }
    None
}

/// M42: true if the instruction after `insn` cannot be reached from it by falling through. That
/// covers three classes:
/// - `B`/`BL`;
/// - a branch to a register (`BR`/`BLR`/`RET`/`ERET` and their PAC forms);
/// - an exception-generating instruction (`SVC`/`HVC`/`SMC`/`BRK`/`HLT`/`DCPS`).
///
/// A conditional branch (`B.cond`, `CBZ`/`CBNZ`, `TBZ`/`TBNZ`) falls through when not taken, so it is
/// not a barrier. The §3d backward scan never infers across a barrier.
pub fn is_fallthrough_barrier(insn: u32) -> bool {
    insn & 0x7C00_0000 == 0x1400_0000       // B, BL
        || insn & 0xFE00_0000 == 0xD600_0000 // branch to register, including the 0xD7 PAC forms
        || insn & 0xFF00_0000 == 0xD400_0000 // exception generation
}
```

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p retrace-arch -- --test-threads=1`
Expected: PASS, including the 5 new tests.

- [ ] **Step 5: Clippy, then commit**

```bash
cargo clippy -p retrace-arch --all-targets -- -D warnings
git add crates/retrace-arch/src/lib.rs
git commit -m "M42 t1: decode the exclusive-monitor instructions (ldx/stx/clrex) and fall-through barriers

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: The repo fixture, the bounded runner, and the REDs

**Files:**
- Create: `crates/retrace-guest/asm/llsc.s`
- Modify: `crates/retrace-guest/build.rs`. Add a block after watchsweep's (around line 497).
- Modify: `crates/retrace-guest/src/lib.rs`. Add `pub const LLSC` after `WATCHSWEEP` (around line
  181).
- Modify: `crates/retrace/tests/util/mod.rs`. Add `debug_bounded`.
- Modify: `crates/retrace/tests/util/hits.rs`. Make `debug` go through `debug_bounded`.
- Create: `crates/retrace/tests/llsc_e2e.rs`

**Interfaces:**
- Produces:
  - `retrace_guest::LLSC: &str`;
  - `util::debug_bounded(trace: &str, script: &str, secs: u64) -> (Option<i32>, String, String)`,
    where `None` means killed at the bound;
  - in `llsc_e2e.rs`: `trace()`, `ts()`, `sym(name)`, `h(name)`, `run_ok(script)`, `wheres(out)`,
    `has_line(out, want)`, `seek_then_finish(n, k)`, `every_position_replays(n, len)`, `STDOUT`,
    `BOUND`. Later tasks append tests to this file and use these helpers.

- [ ] **Step 1: Measure the CPU baselines** (before any `src` change; Task 7 compares against them)

```bash
cargo test -p retrace --test cpython_crash_e2e --test hitorder_e2e --no-run
/usr/bin/time -l cargo test -p retrace --test cpython_crash_e2e -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m42-llsc/t2-cpu-cpython-before.log 2>&1
/usr/bin/time -l cargo test -p retrace --test hitorder_e2e -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m42-llsc/t2-cpu-hitorder-before.log 2>&1
grep -a -e 'user' -e 'test result' .superpowers/sdd/2026-09-24-retrace-m42-llsc/t2-cpu-*-before.log
```

Record the user + sys seconds of each in the report. If `cpython_crash_e2e` skips (no Homebrew
Python), say so. Its figure is then "skipped", and Task 7 compares `hitorder_e2e` only.

- [ ] **Step 2: Write the fixture** `crates/retrace-guest/asm/llsc.s`

Shapes (a)–(d) are t0's scratch fixture byte for byte, plus labels. Labels add no instruction, so
every t0 address and K in windows 1–4 still holds.

```asm
// M42 repo fixture: exclusive (LL/SC) pairs under stepping and debug stops (spec
// docs/superpowers/specs/2026-09-24-retrace-m42-llsc-design.md §4).
//
// Shapes (a)-(d) are M42 t0's scratch fixture byte for byte, so every coordinate in the t0
// measurements document holds for windows 1-4. (e)-(i) are the spec's additions.
//
// Each shape is followed by one syscall whose ARGUMENTS carry its result, so replay's divergence
// oracle, which compares (num, args[0..8]) at every landmark, names any difference there:
//   x3 = the cell after the shape, or a store-exclusive's status;
//   x4 = how many times a retry loop was ENTERED (1 per logical update when no store failed);
//   x5 = a second value.
// write(2) ignores x3..x5; they are there for the oracle. The exit window publishes in exit's
// status instead, because an Exit event records only its code.
//
// Landmarks (window n ends at landmark n):
//    1 write "a"  (a) discard-status, dyld getpid's words: fill an empty cell once
//    2 write "b"  (b) retry loop, three increments
//    3 write "c"  (c) CAS: if (*cas == 5) *cas = 9
//    4 write "d"  (d) ldxp/stxp pair
//    5 write "e"  (e) clrex between the halves: the store fails natively (x3 = 1)
//    6 write "f"  (f) a trapped timebase read between the halves: an exit, so it fails (x3 = 1)
//    7 getpid     (g) a syscall between the halves...
//    8 write "g"      ...so the store, first in window 8, fails (x3 = 1)
//    9 write "h"  (h) (a)'s shape on the filled cell: the cbnz is taken, an LDX with no STX
//   10 exit       (i) a one-pass retry loop in the EXIT window: exit(entries - 1) = exit(0)
// (a) adds ONE extra landmark (getpid) before its write iff its store was lost.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // ---- (a) discard-status, dyld getpid style: fill an empty cell once ----
    adrp x9, cella@PAGE
    add  x9, x9, cella@PAGEOFF
    movz w0, #0x4242                // the value to cache
a_ldx:
    ldxr w10, [x9]                  // K = 3 in window 1
    cbnz w10, a_after               // already filled: skip the store
a_stx:
    stxr wzr, w0, [x9]              // status DISCARDED (dyld getpid shape)
a_after:
    ldr  w11, [x9]
    cbnz w11, a_report
    mov  x16, #20                   // SYS_getpid: issued ONLY if the store was lost
    svc  #0x80
a_report:
    ldr  w3, [x9]                   // x3 = cell (0x4242 when the store landed)
    mov  x4, #0
    mov  x5, #0
    mov  x0, #1
    adrp x1, msga@PAGE
    add  x1, x1, msga@PAGEOFF
    mov  x2, #2
    mov  x16, #4                    // SYS_write(1, "a\n", 2), x3 = cell
a_svc:
    svc  #0x80

    // ---- (b) retry loop: counter += 1, three times ----
b_start:
    adrp x0, ctr@PAGE
    add  x0, x0, ctr@PAGEOFF
    mov  x19, #3                    // three logical increments
    mov  x20, #0                    // loop entries
b_retry:
    add  x20, x20, #1
b_ldx:
    ldaxr x1, [x0]
    add  x1, x1, #1
b_stx:
    stlxr w2, x1, [x0]
    cbnz w2, b_retry
b_next:
    subs x19, x19, #1
    b.ne b_retry
b_done:
    ldr  x3, [x0]                   // x3 = counter (3)
    mov  x4, x20                    // x4 = loop entries (3 when no stlxr failed)
    mov  x5, #0
    mov  x0, #1
    adrp x1, msgb@PAGE
    add  x1, x1, msgb@PAGEOFF
    mov  x2, #2
    mov  x16, #4                    // SYS_write(1, "b\n", 2), x3 = counter, x4 = entries
b_svc:
    svc  #0x80

    // ---- (c) CAS shape: if (*cas == 5) *cas = 9 ----
c_start:
    adrp x0, cas@PAGE
    add  x0, x0, cas@PAGEOFF
    mov  x3, #5                     // expected
    mov  x4, #9                     // new
    mov  x20, #0
c_retry:
    add  x20, x20, #1
c_ldx:
    ldaxr x1, [x0]
    cmp  x1, x3
    b.ne c_out
c_stx:
    stlxr w2, x4, [x0]
    cbnz w2, c_retry
c_out:
    ldr  x3, [x0]                   // x3 = cell (9)
    mov  x4, x20                    // x4 = loop entries (1)
    mov  x5, #0
    mov  x0, #1
    adrp x1, msgc@PAGE
    add  x1, x1, msgc@PAGEOFF
    mov  x2, #2
    mov  x16, #4                    // SYS_write(1, "c\n", 2), x3 = cell, x4 = entries
c_svc:
    svc  #0x80

    // ---- (d) pair: pair[0] += 10, pair[1] += 20 ----
d_start:
    adrp x0, pair@PAGE
    add  x0, x0, pair@PAGEOFF
    mov  x20, #0
d_retry:
    add  x20, x20, #1
d_ldx:
    ldxp x1, x2, [x0]
    add  x4, x1, #10
    add  x5, x2, #20
d_stx:
    stxp w3, x4, x5, [x0]
    cbnz w3, d_retry
d_done:
    ldp  x3, x5, [x0]               // x3 = pair[0] (11), x5 = pair[1] (22)
    mov  x4, x20                    // x4 = loop entries (1)
    mov  x0, #1
    adrp x1, msgd@PAGE
    add  x1, x1, msgd@PAGEOFF
    mov  x2, #2
    mov  x16, #4                    // SYS_write(1, "d\n", 2), x3/x5 = pair, x4 = entries
d_svc:
    svc  #0x80

    // ---- (e) clrex between the halves: the store fails natively ----
e_start:
    adrp x0, celle@PAGE
    add  x0, x0, celle@PAGEOFF
    mov  w1, #7
e_ldx:
    ldxr w6, [x0]
e_clrex:
    clrex
e_stx:
    stxr w2, w1, [x0]
    mov  w3, w2                     // x3 = status (1: the store failed)
    ldr  w4, [x0]                   // x4 = cell (0: nothing was stored)
    mov  x5, #0
    mov  x0, #1
    adrp x1, msge@PAGE
    add  x1, x1, msge@PAGEOFF
    mov  x2, #2
    mov  x16, #4                    // SYS_write(1, "e\n", 2), x3 = status, x4 = cell
e_svc:
    svc  #0x80

    // ---- (f) a trapped timebase read between the halves: an exit, so the store fails ----
f_start:
    adrp x0, cellf@PAGE
    add  x0, x0, cellf@PAGEOFF
    mov  w1, #7
f_ldx:
    ldxr w6, [x0]
f_mrs:
    mrs  x7, cntvct_el0             // trapped and emulated below the trace (try_emulate_timebase)
f_stx:
    stxr w2, w1, [x0]
    mov  w3, w2                     // x3 = status (1: the store failed)
    ldr  w4, [x0]                   // x4 = cell (0)
    mov  x5, #0
    mov  x0, #1
    adrp x1, msgf@PAGE
    add  x1, x1, msgf@PAGEOFF
    mov  x2, #2
    mov  x16, #4                    // SYS_write(1, "f\n", 2), x3 = status, x4 = cell
f_svc:
    svc  #0x80

    // ---- (g) a syscall between the halves: the store, first in window 8, fails ----
g_start:
    adrp x19, cellg@PAGE            // callee-saved registers: the syscall returns in x0/x1
    add  x19, x19, cellg@PAGEOFF
    mov  w20, #7
g_ldx:
    ldxr w6, [x19]
    mov  x16, #20                   // SYS_getpid, between the halves
g_svc:
    svc  #0x80
g_stx:
    stxr w21, w20, [x19]
    mov  w3, w21                    // x3 = status (1: the store failed)
    ldr  w4, [x19]                  // x4 = cell (0)
    mov  x5, #0
    mov  x0, #1
    adrp x1, msgg@PAGE
    add  x1, x1, msgg@PAGEOFF
    mov  x2, #2
    mov  x16, #4                    // SYS_write(1, "g\n", 2), x3 = status, x4 = cell
g_wsvc:
    svc  #0x80

    // ---- (h) (a)'s shape on the filled cell: the cbnz is taken, a load with no store ----
h_start:
    adrp x9, cella@PAGE
    add  x9, x9, cella@PAGEOFF
    movz w0, #0x4343
h_ldx:
    ldxr w10, [x9]                  // cella already holds 0x4242
h_cbnz:
    cbnz w10, h_after               // taken: this load-exclusive has no store-exclusive
h_stx:
    stxr wzr, w0, [x9]              // never executed
h_after:
    ldr  w3, [x9]                   // x3 = cell (0x4242)
    mov  x4, #0
    mov  x5, #0
    mov  x0, #1
    adrp x1, msgh@PAGE
    add  x1, x1, msgh@PAGEOFF
    mov  x2, #2
    mov  x16, #4                    // SYS_write(1, "h\n", 2), x3 = cell
h_svc:
    svc  #0x80

    // ---- (i) the EXIT window holds a one-pass retry loop ----
i_start:
    adrp x0, ctri@PAGE
    add  x0, x0, ctri@PAGEOFF
    mov  x20, #0
i_retry:
    add  x20, x20, #1
i_ldx:
    ldaxr x1, [x0]
    add  x1, x1, #1
i_stx:
    stlxr w2, x1, [x0]
    cbnz w2, i_retry
    sub  x0, x20, #1                // exit status = entries - 1: 0 when no stlxr failed
    mov  x16, #1                    // SYS_exit
i_svc:
    svc  #0x80

.section __DATA,__data
// Each cell in its own 64-byte block, so a watch's FAR can only name its own cell.
.p2align 6
cella: .word 0
.p2align 6
ctr:   .quad 0
.p2align 6
cas:   .quad 5
.p2align 6
pair:  .quad 1, 2
.p2align 6
msga:  .ascii "a\n"
msgb:  .ascii "b\n"
msgc:  .ascii "c\n"
msgd:  .ascii "d\n"
// The spec's additions, appended so that t0's data addresses do not move.
.p2align 6
celle: .word 0
.p2align 6
cellf: .word 0
.p2align 6
cellg: .word 0
.p2align 6
ctri:  .quad 0
msge:  .ascii "e\n"
msgf:  .ascii "f\n"
msgg:  .ascii "g\n"
msgh:  .ascii "h\n"
```

In `crates/retrace-guest/build.rs`, after the watchsweep block:

```rust
    // llsc (M42): exclusive (LL/SC) pairs under stepping and debug stops. t0's four shapes verbatim,
    // then clrex / a trapped timebase read / a syscall between the halves, a branch-out, and a
    // retry loop in the exit window. Each shape publishes its outcome in the next syscall's args.
    let src = format!("{}/asm/llsc.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/llsc");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang llsc");
    assert!(status.success(), "llsc guest build failed");
```

In `crates/retrace-guest/src/lib.rs`, after `WATCHSWEEP`:

```rust
pub const LLSC: &str = concat!(env!("OUT_DIR"), "/llsc");
```

- [ ] **Step 3: Record it, and check t0's layout and the native outcomes** (the spec's first halt rule)

```bash
cargo build -p retrace-guest
ls target/aarch64-apple-darwin/debug/build/
```

Find the `retrace-guest-*/out/llsc` binary that was just built. Check it against t0: `nm` should show
`a_ldx` `0x10000038c`, `a_stx` `0x100000394`, `b_ldx` `0x1000003e0`, `b_stx` `0x1000003e8`,
`c_ldx` `0x100000434`, `d_ldx` `0x10000047c`, `cella` `0x100004000`, `ctr` `0x100004040`.
Paste the `nm` output into the report. If an address differs, compare `otool -tv` of the new
binary's (a)–(d) range with t0's binary,
`/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/7b6f2ab3-f34f-4380-be19-9af66e2826a4/scratchpad/m42/t0/llsc.bin`.
- **Identical instructions at shifted addresses:** a layout move. Record it; nothing in this plan
  hardcodes an address.
- **Differing instructions:** (a)–(d) are not t0's. Fix the asm, never the expectation.

Then write the recording test below as the first test of `llsc_e2e.rs` (Step 5 gives the file's
header), and run it. It pins what each shape did **natively**:

```rust
/// The recording's own landmarks: what each shape did natively. (e), (f) and (g) publish a FAILED
/// store (x3 = 1), because each puts an exit between its halves: `clrex`, a trapped timebase read,
/// a syscall. This test failing on (e), (f) or (g) is spec §7's first halt rule.
#[test]
fn the_recording_holds_what_each_shape_did_natively() {
    let mut s = ReplaySession::open(trace()).unwrap();
    let mut got = Vec::new();
    let outcome = loop {
        if let Some((num, a)) = s.peek_syscall() { got.push((s.landmark(), num, a[3], a[4])); }
        if let Advance::Exited(r) = s.advance().unwrap() { break r.outcome; }
    };
    assert_eq!(got, vec![
        (1, 4, 0x4242, 0), // (a) the cell filled; no getpid
        (2, 4, 3, 3),      // (b) counter 3 in 3 entries
        (3, 4, 9, 1),      // (c) cas 9 in 1 entry
        (4, 4, 11, 1),     // (d) pair[0] 11 in 1 entry
        (5, 4, 1, 0),      // (e) clrex: status 1, cell untouched
        (6, 4, 1, 0),      // (f) trapped timebase read: status 1
        (7, 20, 1, 0),     // (g) the getpid between the halves (x3/x4 still (f)'s)
        (8, 4, 1, 0),      // (g) status 1
        (9, 4, 0x4242, 0), // (h) no store: the cell still holds (a)'s value
    ]);
    assert_eq!(outcome, Outcome::Exit { code: 0 }, "(i): one entry, so exit(entries - 1) = 0");
}
```

Run: `cargo test -p retrace --test llsc_e2e the_recording_holds -- --test-threads=1`
Expected: PASS.
- **If landmark 6's `x3` is 0**, `cntvct_el0` did not trap on this host, so (f) has no exit between
  its halves. Change `f_mrs` to `mrs x7, S3_4_C15_C10_6` (Apple's fast counter, which
  `try_emulate_timebase` also emulates) and re-run. The instruction count is unchanged.
  - If neither encoding traps, delete shape (f), with its cell and its message, renumber this test's
    rows, and report it as the Ruling "the emulated-trap exit class has no static-fixture reach".
- **If landmark 5 or 8 shows `x3 = 0`**, stop and report: that is the halt rule.

- [ ] **Step 4: The bounded runner** (`crates/retrace/tests/util/mod.rs`, after `replay`)

```rust
/// M42: `retrace debug <trace> --script <script>`, killed after `secs` seconds. `None` for the
/// exit code means the bound fired. t0 measured hangs on exactly these scripts, and a hang must
/// fail its test rather than stall the gate.
///
/// It polls with `thread::sleep` and counts iterations, because clippy bans reading a clock. The
/// child writes to two temp files, never to pipes: a pipe nobody reads until exit blocks a child
/// whose transcript outgrows the pipe buffer, which would turn a long passing chain into a kill.
/// What was written before a kill is still returned.
pub fn debug_bounded(trace: &str, script: &str, secs: u64) -> (Option<i32>, String, String) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let base = std::env::temp_dir().join(format!("retrace-dbg-{}-{n}", std::process::id()));
    let (out_p, err_p) = (base.with_extension("out"), base.with_extension("err"));
    let mut child = Command::new(bin())
        .args(["debug", trace, "--script", script])
        .stdout(std::fs::File::create(&out_p).expect("create debug stdout file"))
        .stderr(std::fs::File::create(&err_p).expect("create debug stderr file"))
        .spawn().expect("spawn debug");
    let mut code = None;
    for _ in 0..secs * 20 {
        if let Some(st) = child.try_wait().expect("try_wait debug") {
            code = Some(st.code().unwrap_or(-1));
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if code.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let read = |p: &std::path::Path| {
        let s = String::from_utf8_lossy(&std::fs::read(p).unwrap_or_default()).into_owned();
        let _ = std::fs::remove_file(p);
        s
    };
    (code, read(&out_p), read(&err_p))
}
```

In `crates/retrace/tests/util/hits.rs`, replace the body of `debug` so every oracle chain is bounded
too. Its signature and its callers are unchanged.

```rust
/// `retrace debug <trace> --script <script>` on the codesigned copy: (exit code, stdout, stderr).
/// Bounded since M42 (`util::debug_bounded`, 600 s): a chain that hangs fails, naming the script.
pub fn debug(trace: &str, script: &str) -> (i32, String, String) {
    let (code, out, err) = super::debug_bounded(trace, script, 600);
    let code = code.unwrap_or_else(|| panic!("debug killed at the 600 s bound (a hang):\n{script}\n{out}"));
    (code, out, err)
}
```

- [ ] **Step 5: Write `llsc_e2e.rs`: the header, the helpers, and every named regression**

Put the file header and helpers first, then the Step 3 test, then the tests below. Each doc comment
names the t0 measurement it flips. "Green at" says which task turns it green. A test that goes green
**earlier** is recorded in the report and is not a defect. One still red **after** its task is a
defect.

```rust
//! M42: stepping, seeking and debug stops inside AArch64 exclusive pairs (spec
//! `docs/superpowers/specs/2026-09-24-retrace-m42-llsc-design.md`). The fixture is
//! `retrace-guest/asm/llsc.s`: t0's four shapes (a)-(d) verbatim, then (e)-(i). Every CLI run goes
//! through `util::debug_bounded`, because t0 measured hangs on these scripts.
mod util;
use retrace_core::{Advance, Outcome, ReplaySession};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Seconds before a debugger run is killed. t0 killed its hangs at 60 s; a green run takes under one.
const BOUND: u64 = 60;
/// What the guest writes, in order: one letter per shape that ends in a `write`.
const STDOUT: &[u8] = b"a\nb\nc\nd\ne\nf\ng\nh\n";

fn trace() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    C.get_or_init(|| {
        let (rec, t) = util::record(retrace_guest::LLSC);
        assert_eq!(rec.code, 0, "record llsc: {}", rec.stderr);
        t
    })
}
fn ts() -> &'static str { trace().to_str().unwrap() }

/// A fixture label's address, from `nm`. The labels are local symbols in LC_SYMTAB.
fn sym(name: &str) -> u64 {
    static M: OnceLock<HashMap<String, u64>> = OnceLock::new();
    let m = M.get_or_init(|| {
        let out = std::process::Command::new("nm").arg(retrace_guest::LLSC).output().expect("nm");
        assert!(out.status.success(), "nm llsc: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().lines().filter_map(|l| {
            let mut f = l.split_whitespace();
            let (a, _kind, n) = (f.next()?, f.next()?, f.next()?);
            Some((n.to_string(), u64::from_str_radix(a, 16).ok()?))
        }).collect()
    });
    *m.get(name).unwrap_or_else(|| panic!("no symbol {name} in llsc"))
}
fn h(name: &str) -> String { format!("{:#x}", sym(name)) }

/// A script's stdout. It must exit 0 inside the bound; anything else panics, naming the symptom.
fn run_ok(script: &str) -> String {
    let (code, out, err) = util::debug_bounded(ts(), script, BOUND);
    match code {
        Some(0) => out,
        None => panic!("killed at the {BOUND} s bound (t0's hang)\nscript: {script}\n{out}"),
        Some(c) => panic!("exit {c}\nscript: {script}\nstderr: {err}\n{out}"),
    }
}
/// The `where` answers of a transcript, annotations stripped, in order.
fn wheres(out: &str) -> Vec<&str> {
    out.lines().filter(|l| l.starts_with("at (")).map(util::strip_annot).collect()
}
fn has_line(out: &str, want: &str) -> bool { out.lines().map(util::strip_annot).any(|l| l == want) }
fn resolved(out: &str) -> Vec<String> {
    out.lines().filter(|l| l.starts_with("resolved (")).map(str::to_string).collect()
}

/// Seek to (n, k), then replay forward to the end: the outcome and the guest's stdout.
fn seek_then_finish(n: usize, k: u64) -> Result<(Outcome, Vec<u8>), String> {
    let mut s = retrace_core::seek(trace(), n, k)?;
    loop {
        if let Advance::Exited(r) = s.advance()
            .map_err(|d| format!("diverged at landmark {}: {}", d.landmark, d.detail))? {
            return Ok((r.outcome, r.stdout));
        }
    }
}

/// Every (n, k) with k = 0..=len replays to the recorded end, and k = len + 1 is refused by naming
/// exactly `len`. That pins the STEPPED length to the recorded path (t0: window 1 measured 9, not 16).
fn every_position_replays(n: usize, len: u64) {
    for k in 0..=len {
        let (o, out) = seek_then_finish(n, k).unwrap_or_else(|e| panic!("({n}, {k}): {e}"));
        assert_eq!(o, Outcome::Exit { code: 0 }, "({n}, {k})");
        assert_eq!(out, STDOUT, "({n}, {k}): stdout");
    }
    let e = retrace_core::seek(trace(), n, len + 1).map(|_| ()).unwrap_err();
    assert!(e.contains(&format!("window {n} ends after {len} instruction(s)")), "({n}, {}): {e}", len + 1);
}

/// `break at`, then `continue` once per expected hit and once more: each hit resolves at its
/// coordinate, and the answer after the last is the end.
fn break_forward(at: &str, want: &[(usize, u64)]) {
    let out = run_ok(&format!("break {}; {}", h(at), "continue; ".repeat(want.len() + 1)));
    let exp: Vec<String> = want.iter().map(|(n, k)| format!("resolved ({n}, {k})")).collect();
    assert_eq!(resolved(&out), exp, "{out}");
    assert!(out.contains("exited (code 0)"), "{out}");
}
/// From the end, `break at`, then `reverse-continue` once per expected hit and once more.
fn break_backward(at: &str, want: &[(usize, u64)]) {
    let out = run_ok(&format!("continue; break {}; {}", h(at), "reverse-continue; ".repeat(want.len() + 1)));
    let got: Vec<&str> = out.lines().filter(|l| l.starts_with("hit 0x")).map(util::strip_annot).collect();
    let exp: Vec<String> = want.iter().rev().map(|(n, k)| format!("hit {} at ({n}, {k})", h(at))).collect();
    assert_eq!(got, exp, "{out}");
    assert!(out.contains("no earlier hit"), "{out}");
}
/// `watch cell+off len`, then `continue` once per expected hit and once more. Every hit is the
/// store-exclusive `stx`.
fn watch_forward(cell: &str, off: u64, len: u64, stx: &str, want: &[(usize, u64)]) {
    let a = sym(cell) + off;
    let out = run_ok(&format!("watch {a:#x} {len}; {}", "continue; ".repeat(want.len() + 1)));
    assert_eq!(out.matches(&format!("hit watch {a:#x} (write at {})", h(stx))).count(), want.len(), "{out}");
    let exp: Vec<String> = want.iter().map(|(n, k)| format!("resolved ({n}, {k})")).collect();
    assert_eq!(resolved(&out), exp, "{out}");
    assert!(out.contains("exited (code 0)"), "{out}");
}
/// From the end, `watch cell+off len`, then `reverse-continue` once per expected hit and once more.
fn watch_backward(cell: &str, off: u64, len: u64, stx: &str, want: &[(usize, u64)]) {
    let a = sym(cell) + off;
    let out = run_ok(&format!("continue; watch {a:#x} {len}; {}", "reverse-continue; ".repeat(want.len() + 1)));
    let got: Vec<&str> = out.lines().filter(|l| l.starts_with("hit watch")).map(util::strip_annot).collect();
    let exp: Vec<String> = want.iter().rev()
        .map(|(n, k)| format!("hit watch {a:#x} (write at {}) at ({n}, {k})", h(stx))).collect();
    assert_eq!(got, exp, "{out}");
    assert!(out.contains("no earlier hit"), "{out}");
}

// ---- Named regressions (spec §4), each RED on 1d95a93 ------------------------------------------

/// t0 M2: `stepi` past (a)'s `ldxr`, then `continue`. It diverged, with an extra `getpid` at
/// landmark 1. Green at Task 3. The final `where` is the terminal park on (i)'s window, which
/// single-steps a retry loop (Review Focus 5).
#[test]
fn m2_stepi_past_the_ldxr_then_continue_replays_to_the_end() {
    let out = run_ok("stepi 4; where; continue; where");
    let w = wheres(&out);
    assert_eq!(w.first().copied(), Some(format!("at (1, 4) pc={:#x} thread=0", sym("a_ldx") + 4).as_str()), "{out}");
    assert!(out.contains("exited (code 0)"), "{out}");
    assert_eq!(w.last().copied(), Some(format!("at (10, 10) pc={} thread=0", h("i_svc")).as_str()), "{out}");
}

/// t0 M3a: `stepi` through (b)'s retry loop. It livelocked: the counter never moved. Green at Task 3.
#[test]
fn m3a_stepi_through_the_retry_loop_reaches_b_done() {
    let out = run_ok(&format!("break {0}; continue; delete {0}; stepi 25; where; x {1} 8", h("b_start"), h("ctr")));
    assert_eq!(wheres(&out).last().copied(), Some(format!("at (2, 25) pc={} thread=0", h("b_done")).as_str()), "{out}");
    assert!(has_line(&out, &format!("{}: 03 00 00 00 00 00 00 00", h("ctr"))), "{out}");
}

/// t0 M3b: `continue` to a breakpoint after the loop. It hung. Green at Task 3.
#[test]
fn m3b_continue_to_a_breakpoint_after_the_loop() {
    let out = run_ok(&format!("break {}; continue; where", h("b_done")));
    assert_eq!(resolved(&out), vec!["resolved (2, 25)".to_string()], "{out}");
}

/// t0 M3c: `reverse-continue` to a breakpoint after the loop. It hung. Green at Task 3.
#[test]
fn m3c_reverse_continue_to_a_breakpoint_after_the_loop() {
    let out = run_ok(&format!("break {0}; continue; delete {0}; break {1}; reverse-continue; where",
        h("c_start"), h("b_done")));
    assert!(has_line(&out, &format!("hit {} at (2, 25)", h("b_done"))), "{out}");
}

/// t0 M3d: `reverse-stepi` into window 2 measures its length by stepping. It hung. Green at Task 3.
#[test]
fn m3d_reverse_stepi_into_the_loop_window_lands_on_its_svc() {
    let out = run_ok(&format!("break {0}; continue; delete {0}; reverse-stepi; where", h("c_start")));
    assert_eq!(wheres(&out).last().copied(), Some(format!("at (2, 33) pc={} thread=0", h("b_svc")).as_str()), "{out}");
}

/// t0's (1, 9) cross-check: `reverse-stepi` from (2, 0) parked on a `getpid` the recording never
/// ran, and the next `continue` diverged. Green at Task 3.
#[test]
fn m3d_prime_reverse_stepi_measures_window_1_on_the_recorded_path() {
    let out = run_ok(&format!("break {0}; continue; delete {0}; reverse-stepi; where; continue", h("b_start")));
    assert_eq!(wheres(&out).last().copied(), Some(format!("at (1, 16) pc={} thread=0", h("a_svc")).as_str()), "{out}");
    assert!(out.contains("exited (code 0)"), "{out}");
}

/// t0 M3e: a `reverse-continue` whose phase 2 steps window 2. It answered `no earlier hit`, silently,
/// with exit 0. Green at Task 3.
#[test]
fn m3e_reverse_continue_whose_phase_2_steps_the_loop() {
    let out = run_ok(&format!("break {0}; continue; delete {0}; stepi 30; where; break {1}; reverse-continue; where",
        h("b_start"), h("b_done")));
    // [0] is the `where` after `stepi 30`; [1] is after `reverse-continue`, which re-seeks to the hit
    // (corrected in Task 2's review: the plan first asserted (2, 30) on [1]).
    assert_eq!(wheres(&out)[0], format!("at (2, 30) pc={:#x} thread=0", sym("b_done") + 0x14), "{out}");
    assert_eq!(wheres(&out)[1], format!("at (2, 25) pc={} thread=0", h("b_done")), "{out}");
    assert!(has_line(&out, &format!("hit {} at (2, 25)", h("b_done"))), "{out}");
}

/// t0 M4: a breakpoint on (a)'s discard-status `stxr`. It diverged. Forward: green at Task 4 or 5.
#[test]
fn m4_break_on_the_discard_status_store_forward() { break_forward("a_stx", &[(1, 5)]); }
/// Backward: green at Task 5.
#[test]
fn m4_break_on_the_discard_status_store_backward() { break_backward("a_stx", &[(1, 5)]); }
/// t0 M4d/M4e: a breakpoint on (b)'s `stlxr`. Forward gave phantom hits at (2, 12), (2, 17)…;
/// backward hung. Forward: green at Task 4 or 5.
#[test]
fn m4_break_on_the_retry_store_forward() { break_forward("b_stx", &[(2, 7), (2, 14), (2, 21)]); }
/// Backward: green at Task 5.
#[test]
fn m4_break_on_the_retry_store_backward() { break_backward("b_stx", &[(2, 7), (2, 14), (2, 21)]); }

/// t0 M5: a watch on (b)'s counter. Forward gave phantom hits that never ended; backward hung.
/// Forward: green at Task 4 or 5.
#[test]
fn m5_watch_the_retry_counter_forward() { watch_forward("ctr", 0, 8, "b_stx", &[(2, 7), (2, 14), (2, 21)]); }
/// Backward: green at Task 5.
#[test]
fn m5_watch_the_retry_counter_backward() { watch_backward("ctr", 0, 8, "b_stx", &[(2, 7), (2, 14), (2, 21)]); }
/// t0 M6: the same on (c), (d)'s second element, and (a). Forward: green at Task 4 or 5.
#[test]
fn m6_watch_the_cas_cell_forward() { watch_forward("cas", 0, 8, "c_stx", &[(3, 9)]); }
/// Backward: green at Task 5.
#[test]
fn m6_watch_the_cas_cell_backward() { watch_backward("cas", 0, 8, "c_stx", &[(3, 9)]); }
/// Forward: green at Task 4 or 5.
#[test]
fn m6_watch_the_pairs_second_element_forward() { watch_forward("pair", 8, 8, "d_stx", &[(4, 7)]); }
/// Backward: green at Task 5.
#[test]
fn m6_watch_the_pairs_second_element_backward() { watch_backward("pair", 8, 8, "d_stx", &[(4, 7)]); }
/// Forward: green at Task 4 or 5.
#[test]
fn m6_watch_the_discard_status_cell_forward() { watch_forward("cella", 0, 4, "a_stx", &[(1, 5)]); }
/// Backward: green at Task 5.
#[test]
fn m6_watch_the_discard_status_cell_backward() { watch_backward("cella", 0, 4, "a_stx", &[(1, 5)]); }

/// t0 M7: seek to every K of windows 1-4, then replay to the end. Every K from LDX+1 through STX+1
/// diverged. Green at Task 3.
#[test]
fn m7_every_position_of_windows_1_to_4_replays_to_the_end() {
    for (n, len) in [(1, 16), (2, 33), (3, 19), (4, 16)] { every_position_replays(n, len); }
}

/// t0 M7's CLI form, `stepi K; continue`, at each pair's LDX+1 and STX+1. Green at Task 3.
#[test]
fn m7_stepi_into_each_pair_then_continue_replays_to_the_end() {
    for (start, ldx_k, stx_k) in [(None, 3u64, 5u64), (Some("b_start"), 5, 7), (Some("c_start"), 6, 9), (Some("d_start"), 4, 7)] {
        for k in [ldx_k + 1, stx_k + 1] {
            let reach = start.map(|s| format!("break {0}; continue; delete {0}; ", h(s))).unwrap_or_default();
            let out = run_ok(&format!("{reach}stepi {k}; continue; where"));
            assert!(out.contains("exited (code 0)"), "{start:?} K={k}:\n{out}");
            assert_eq!(wheres(&out).last().copied(), Some(format!("at (10, 10) pc={} thread=0", h("i_svc")).as_str()),
                "{start:?} K={k}:\n{out}");
        }
    }
}

// ---- The clear rule's positive controls (spec §4) ----------------------------------------------
// Green on 1d95a93 and after: stepping these pairs fails their stores natively, exactly as record
// did. Task 3 shows each one ABLE to fail by deleting the clear it guards.

/// (e): `clrex` between the halves.
#[test]
fn control_e_every_position_of_the_clrex_window_replays() { every_position_replays(5, 14); }
/// (f): a trapped timebase read between the halves.
#[test]
fn control_f_every_position_of_the_trapped_read_window_replays() { every_position_replays(6, 14); }
/// (g): a syscall between the halves, across windows 7 and 8.
#[test]
fn control_g_every_position_across_the_syscall_replays() {
    every_position_replays(7, 5);
    every_position_replays(8, 9);
}

/// (h) and (i). Window 10's retry loop diverges on 1d95a93 (red), and window 9 is green. Green at
/// Task 3.
#[test]
fn every_position_of_the_branch_out_and_exit_windows_replays() {
    every_position_replays(9, 13);
    every_position_replays(10, 10);
}
```

- [ ] **Step 6: Run the file and ledger every RED**

```bash
cargo test -p retrace --test llsc_e2e --no-fail-fast -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m42-llsc/t2-red.log 2>&1; echo "exit=$?"
grep -a -e '^test ' -e 'test result' .superpowers/sdd/2026-09-24-retrace-m42-llsc/t2-red.log
```

Expected:
- **Green:** `the_recording_holds_what_each_shape_did_natively`, the three `control_*` tests.
- **Red:** everything else. That includes `every_position_of_the_branch_out_and_exit_windows_replays`,
  which fails on window 10.

For each red test, copy its failure symptom into the report. The symptom is one of:
- "killed at the 60 s bound" (the M3 and M5 hangs);
- "exit 5 … diverged";
- a wrong `resolved (…)` list (the phantom hits);
- a wrong `where`.

A red test whose symptom is a panic in the test's own setup (for example `no symbol …`) is a test
bug. Fix it before committing.

- [ ] **Step 7: Clippy, then commit**

```bash
cargo clippy -p retrace --all-targets -- -D warnings
git add crates/retrace-guest/asm/llsc.s crates/retrace-guest/build.rs crates/retrace-guest/src/lib.rs crates/retrace/tests/util/mod.rs crates/retrace/tests/util/hits.rs crates/retrace/tests/llsc_e2e.rs
git commit -m "M42 t2: the llsc fixture, a bounded debug runner, and the named regressions (RED)

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: The shadow, the emulated store-exclusive, and `run()` inside a pair

Spec §3a, §3b, §3e and §3f. It adds the whole mechanism except raising debug stops (Task 4) and
inference (Task 5). A breakpoint or watch that applies at an emulated store **panics** here ("not yet
raised") rather than emulating past a hit silently.

**Files:**
- Create: `crates/retrace-box/src/excl.rs`
- Modify: `crates/retrace-box/src/lib.rs`, at these sites:
  - the imports and module declarations (lines 1–16);
  - a new const near `MDSCR_SS` (around line 290);
  - the `Box_` struct's last field (after `canary_disturbances`, around line 608);
  - `BoxState` after `fall_throughs` (around line 995);
  - the three one-line constructors (lines 1395, 1997 and 3024);
  - `run()` (2634–2751);
  - `step()` (2759–2797);
  - `run_one_for_step()` (2806–2852);
  - `va_to_ipa` (around line 4464);
  - `switch_to_thread` (around line 5525);
  - `checkpoint()` (around line 5693);
  - `from_checkpoint()` (around line 5813).
- Modify: `crates/retrace-core/src/lib.rs`:
  - a `pub use` (line 9);
  - the `record_box` assert (after line 141);
  - the `replay()` assert (around line 3156);
  - `ReplaySession::dbg_excl`.
- Modify: `crates/retrace-box/tests/step.rs`, `crates/retrace-box/tests/checkpointparity.rs`.
- Modify: `crates/retrace/tests/llsc_e2e.rs`. Append the shadow-lifecycle and Review Focus tests.

**Interfaces:**
- Consumes: `retrace_arch::{decode_excl, ExclInsn}` (Task 1), `retrace_guest::LLSC` (Task 2).
- Produces:
  - `retrace_box::{Excl, SetBy}` (`pub struct Excl { pub va: u64, pub size: u8, pub pair: bool, pub loaded: Vec<u8>, pub by: SetBy }`, with `pub enum SetBy { Stepped, Inferred }`);
  - `Box_::dbg_excl(&self) -> Option<Excl>`;
  - `ReplaySession::dbg_excl(&self) -> Option<Excl>`;
  - a re-export, `retrace_core::{Excl, SetBy}`;
  - in `excl.rs`: `TAG_MASK`, `access_len`, `el0_writable`, `StxPlan`, `plan_stx`,
    `base_aliases_dest`;
  - private `Box_` methods that Tasks 4 and 5 edit: `note_exit(&mut self, debug: bool)`,
    `insn_at`, `xreg`, `base_reg`, `va_leaf`, `bp_armed_at`, `watch_overlaps`,
    `raise_debug_stop(&mut self, pc: u64, va: u64, len: usize) -> Option<Stop>`, `emulate_stx`.

- [ ] **Step 1: The pure module, with its tests first** (`crates/retrace-box/src/excl.rs`)

```rust
//! M42: the shadow of the PE's local exclusive monitor, and the pure checks around it (spec
//! `docs/superpowers/specs/2026-09-24-retrace-m42-llsc-design.md` §3). Everything here is pure:
//! `Box_` reads the vCPU and guest memory and passes the values in, so that every fail-loud branch
//! of the store-exclusive emulator has a unit test with no VM.
use retrace_arch::ExclInsn;

/// TBI: `TCR_EL1` sets TBI0, so a data VA may carry a tag in [63:56], and `va_to_ipa` does not
/// strip it. Every address the shadow holds or compares is stripped with this mask first.
pub const TAG_MASK: u64 = 0x00FF_FFFF_FFFF_FFFF;

/// How a shadow came to be set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetBy {
    /// `step()` retired the load-exclusive (the step exit's ISS.EX, t0 M8).
    Stepped,
    /// A native breakpoint or watchpoint stop, by spec §3d's backward scan.
    Inferred,
}

/// The shadow: what the hardware monitor would hold, had retrace's own exits not cleared it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Excl {
    /// The marked VA, tag stripped.
    pub va: u64,
    /// Bytes per element: 1, 2, 4 or 8.
    pub size: u8,
    pub pair: bool,
    /// The bytes the load returned: `access_len(size, pair)` of them.
    pub loaded: Vec<u8>,
    pub by: SetBy,
}

/// Bytes one exclusive access moves.
pub fn access_len(size: u8, pair: bool) -> usize { size as usize * if pair { 2 } else { 1 } }

/// Stage-1 AP[2:1] (descriptor bits 7:6) == 0b01 is the only encoding that lets EL0 write
/// (`ATTR_DATA`). `ATTR_CODE` (0b11), `ATTR_TRAMP` (0b10) and `ATTR_NONE` (0b00) do not.
pub fn el0_writable(leaf: u64) -> bool { (leaf >> 6) & 3 == 1 }

/// What an emulated store-exclusive writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StxPlan {
    /// The tag-stripped VA.
    pub va: u64,
    pub bytes: Vec<u8>,
    /// The status register to zero, or None for WZR.
    pub status: Option<u32>,
}

/// Spec §3b step 1: validate a store-exclusive against the shadow.
///
/// The caller reads every input:
/// - `base` is Rn's value, from SP_EL0 when Rn is 31;
/// - `rt_val` and `rt2_val` are the data registers, 0 for XZR;
/// - `target` is the bytes now at the VA, None if unmapped;
/// - `writable` says whether the stage-1 leaf grants EL0 write.
///
/// Each refusal names its check, and the caller panics with it.
pub fn plan_stx(ex: &Excl, st: ExclInsn, base: u64, rt_val: u64, rt2_val: u64,
                target: Option<&[u8]>, writable: bool) -> Result<StxPlan, String> {
    let ExclInsn::Store { size, pair, rs, rt, rt2, rn } = st else {
        return Err(format!("not a store-exclusive: {st:?}"));
    };
    if rs == rt || (pair && rs == rt2) || (rs == rn && rn != 31) {
        return Err(format!("status register w{rs} aliases a data or base register (CONSTRAINED UNPREDICTABLE)"));
    }
    let va = base & TAG_MASK;
    if va != ex.va || size != ex.size || pair != ex.pair {
        return Err(format!("the store ({va:#x}, {size} B, pair={pair}) does not match the load ({:#x}, {} B, pair={})",
            ex.va, ex.size, ex.pair));
    }
    let len = access_len(size, pair);
    if va % len as u64 != 0 { return Err(format!("{va:#x} is not {len}-byte aligned")); }
    let Some(target) = target else { return Err(format!("{va:#x} is unmapped")) };
    if target != ex.loaded.as_slice() {
        return Err(format!("the bytes at {va:#x} changed since the load ({:02x?} -> {target:02x?})", ex.loaded));
    }
    if !writable { return Err(format!("{va:#x} is not EL0-writable")); }
    let s = size as usize;
    let mut bytes = rt_val.to_le_bytes()[..s].to_vec();
    if pair { bytes.extend_from_slice(&rt2_val.to_le_bytes()[..s]); }
    Ok(StxPlan { va, bytes, status: (rs != 31).then_some(rs) })
}

/// Spec §3a: a base register that is also a destination was overwritten by the load, so the marked
/// address is gone. SP (31) never is: 31 in a destination is XZR.
pub fn base_aliases_dest(ld: ExclInsn) -> bool {
    let ExclInsn::Load { pair, rt, rt2, rn, .. } = ld else { return false };
    rn != 31 && (rn == rt || (pair && rn == rt2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shadow(va: u64, size: u8, pair: bool, loaded: &[u8]) -> Excl {
        Excl { va, size, pair, loaded: loaded.to_vec(), by: SetBy::Stepped }
    }
    /// `stxr wzr, w0, [x9]`: (a)'s store, and dyld getpid's.
    const A_STX: ExclInsn = ExclInsn::Store { size: 4, pair: false, rs: 31, rt: 0, rt2: 31, rn: 9 };
    /// `stlxr w2, x1, [x0]`: (b)'s.
    const B_STX: ExclInsn = ExclInsn::Store { size: 8, pair: false, rs: 2, rt: 1, rt2: 31, rn: 0 };
    /// `stxp w3, x4, x5, [x0]`: (d)'s.
    const D_STX: ExclInsn = ExclInsn::Store { size: 8, pair: true, rs: 3, rt: 4, rt2: 5, rn: 0 };

    #[test]
    fn a_matching_store_plans_its_bytes_and_its_status() {
        let ex = shadow(0x1_0000_4000, 4, false, &[0; 4]);
        assert_eq!(plan_stx(&ex, A_STX, 0x1_0000_4000, 0x4242, 0, Some(&[0; 4]), true),
            Ok(StxPlan { va: 0x1_0000_4000, bytes: vec![0x42, 0x42, 0, 0], status: None }));
        let two = 2u64.to_le_bytes();
        let ex = shadow(0x1_0000_4040, 8, false, &two);
        assert_eq!(plan_stx(&ex, B_STX, 0x1_0000_4040, 3, 0, Some(&two), true),
            Ok(StxPlan { va: 0x1_0000_4040, bytes: 3u64.to_le_bytes().to_vec(), status: Some(2) }));
    }

    #[test]
    fn a_pair_writes_both_elements_in_order() {
        let loaded: Vec<u8> = [1u64.to_le_bytes(), 2u64.to_le_bytes()].concat();
        let ex = shadow(0x1_0000_40c0, 8, true, &loaded);
        let p = plan_stx(&ex, D_STX, 0x1_0000_40c0, 11, 22, Some(&loaded), true).unwrap();
        assert_eq!(p.bytes, [11u64.to_le_bytes(), 22u64.to_le_bytes()].concat());
        assert_eq!(p.status, Some(3));
    }

    #[test]
    fn a_tagged_base_is_stripped_before_it_is_compared() {
        let ex = shadow(0x1_0000_4000, 4, false, &[0; 4]);
        let p = plan_stx(&ex, A_STX, 0x5a00_0001_0000_4000, 7, 0, Some(&[0; 4]), true).unwrap();
        assert_eq!(p.va, 0x1_0000_4000);
    }

    #[test]
    fn a_store_that_does_not_match_the_load_is_refused() {
        let ex = shadow(0x1_0000_4000, 4, false, &[0; 4]);
        let other_va = plan_stx(&ex, A_STX, 0x1_0000_4004, 0, 0, Some(&[0; 4]), true).unwrap_err();
        assert!(other_va.contains("does not match the load"), "{other_va}");
        let wider = ExclInsn::Store { size: 8, pair: false, rs: 31, rt: 0, rt2: 31, rn: 9 };
        assert!(plan_stx(&ex, wider, 0x1_0000_4000, 0, 0, Some(&[0; 8]), true).unwrap_err().contains("does not match"));
        let paired = ExclInsn::Store { size: 4, pair: true, rs: 31, rt: 0, rt2: 1, rn: 9 };
        assert!(plan_stx(&ex, paired, 0x1_0000_4000, 0, 0, Some(&[0; 8]), true).unwrap_err().contains("does not match"));
    }

    #[test]
    fn a_misaligned_address_is_refused() {
        let ex = shadow(0x1_0000_4002, 4, false, &[0; 4]);
        assert!(plan_stx(&ex, A_STX, 0x1_0000_4002, 0, 0, Some(&[0; 4]), true).unwrap_err().contains("aligned"));
    }

    #[test]
    fn every_aliasing_status_register_is_refused_and_an_sp_base_is_not_wzr() {
        let ex = shadow(0x1_0000_4000, 8, false, &[0; 8]);
        let s_is_t = ExclInsn::Store { size: 8, pair: false, rs: 1, rt: 1, rt2: 31, rn: 0 };
        let s_is_n = ExclInsn::Store { size: 8, pair: false, rs: 0, rt: 1, rt2: 31, rn: 0 };
        let zr_zr = ExclInsn::Store { size: 8, pair: false, rs: 31, rt: 31, rt2: 31, rn: 0 };
        for st in [s_is_t, s_is_n, zr_zr] {
            let e = plan_stx(&ex, st, 0x1_0000_4000, 0, 0, Some(&[0; 8]), true).unwrap_err();
            assert!(e.contains("aliases"), "{st:?}: {e}");
        }
        let exp = shadow(0x1_0000_40c0, 8, true, &[0; 16]);
        let s_is_t2 = ExclInsn::Store { size: 8, pair: true, rs: 5, rt: 4, rt2: 5, rn: 0 };
        assert!(plan_stx(&exp, s_is_t2, 0x1_0000_40c0, 0, 0, Some(&[0; 16]), true).unwrap_err().contains("aliases"));
        // A WZR status with an SP base names two different registers: allowed.
        let sp = ExclInsn::Store { size: 8, pair: false, rs: 31, rt: 1, rt2: 31, rn: 31 };
        assert!(plan_stx(&ex, sp, 0x1_0000_4000, 9, 0, Some(&[0; 8]), true).is_ok());
    }

    #[test]
    fn drifted_bytes_an_unmapped_target_and_a_read_only_page_are_refused() {
        let ex = shadow(0x1_0000_4000, 4, false, &[0; 4]);
        assert!(plan_stx(&ex, A_STX, 0x1_0000_4000, 0, 0, Some(&[1, 0, 0, 0]), true).unwrap_err().contains("changed since the load"));
        assert!(plan_stx(&ex, A_STX, 0x1_0000_4000, 0, 0, None, true).unwrap_err().contains("unmapped"));
        assert!(plan_stx(&ex, A_STX, 0x1_0000_4000, 0, 0, Some(&[0; 4]), false).unwrap_err().contains("EL0-writable"));
    }

    #[test]
    fn only_the_data_attribute_is_el0_writable() {
        assert!(el0_writable(crate::ATTR_DATA));
        assert!(!el0_writable(crate::ATTR_CODE));
        assert!(!el0_writable(crate::ATTR_TRAMP));
        assert!(!el0_writable(crate::ATTR_NONE));
    }

    #[test]
    fn a_base_that_is_also_a_destination_is_flagged_and_sp_never_is() {
        assert!(base_aliases_dest(ExclInsn::Load { size: 8, pair: false, rt: 0, rt2: 31, rn: 0 }));
        assert!(base_aliases_dest(ExclInsn::Load { size: 8, pair: true, rt: 1, rt2: 0, rn: 0 }));
        assert!(!base_aliases_dest(ExclInsn::Load { size: 8, pair: false, rt: 31, rt2: 31, rn: 31 }));
        assert!(!base_aliases_dest(ExclInsn::Load { size: 4, pair: false, rt: 10, rt2: 31, rn: 9 }));
    }
}
```

In `lib.rs`:
- Change line 2 to `use retrace_arch::{decode_excl, ec_of, Ec, ExclInsn};`.
- After `pub mod thread;`, add:

```rust
mod excl;
pub use excl::{Excl, SetBy};
```

Run: `cargo test -p retrace-box --lib excl -- --test-threads=1`
Expected: 9 PASS. `ATTR_*` are crate-root `const`s, so `crate::ATTR_DATA` resolves from the child
module.

- [ ] **Step 2: Write the box-level and session-level tests that the mechanism must turn green**

`crates/retrace-box/tests/step.rs` (it spells `retrace_guest::` out in full, and imports
`use retrace_box::{Box_, Stop};`): add `SetBy` to that import, then append:

```rust
/// M42 §3a/§3b, on (a), at the box:
/// - stepping the `ldxr` sets the shadow from the step exit's ISS.EX;
/// - the `cbnz`, a debug exit, leaves it;
/// - the `stxr` is emulated: the store lands, the shadow clears, and pc moves one instruction.
///
/// Before M42 the stepped `stxr` failed and the cell stayed 0 (t0 M2).
#[test]
fn stepping_a_load_exclusive_sets_the_shadow_and_its_store_lands() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::LLSC).unwrap());
    let mut b = Box_::load(&loaded);
    for i in 1..=3 { assert!(matches!(b.step(), Stop::Step), "step {i}"); } // adrp, add, movz
    assert_eq!(b.dbg_excl(), None, "nothing exclusive has retired yet");
    assert!(matches!(b.step(), Stop::Step));                                 // ldxr w10, [x9]
    let ex = b.dbg_excl().expect("the ldxr's retire sets the shadow");
    assert_eq!((ex.size, ex.pair, ex.loaded.as_slice(), ex.by), (4, false, &[0u8; 4][..], SetBy::Stepped));
    assert!(matches!(b.step(), Stop::Step));                                 // cbnz w10 (not taken)
    assert_eq!(b.dbg_excl().map(|e| e.va), Some(ex.va), "a debug exit leaves the shadow standing");
    let stx = b.pc();
    assert!(matches!(b.step(), Stop::Step));                                 // stxr wzr, w0, [x9]
    assert_eq!(b.dbg_excl(), None, "the emulated store clears the shadow");
    assert_eq!(b.pc(), stx + 4);
    assert_eq!(b.read_guest(b.va_to_ipa(ex.va).unwrap(), 4), vec![0x42, 0x42, 0, 0]);
}
```

`crates/retrace-box/tests/checkpointparity.rs`:
- Change the import to `use retrace_box::{Box_, Stop};` and add `LLSC` to the `retrace_guest`
  import.
- In `assert_checkpoint_parity`, add `let excl = b.dbg_excl();` beside `let fts = b.fall_throughs();`.
- Add this after the fall-through assertion:
  `assert_eq!(r.dbg_excl(), excl, "{label}: exclusive-monitor shadow (M42)");`.
- Then append the tier:

```rust
/// M42: the mid-pair tier. It steps (a)'s first four instructions, so the `ldxr` has retired and
/// the exclusive shadow is set. No other tier can make that field non-default: without this one the
/// `dbg_excl` row compares `None == None`, the trap `restoreparity.rs` names.
#[test]
fn a_checkpointed_box_inside_an_exclusive_pair_matches_the_box_it_came_from() {
    let loaded = parse_macho(&std::fs::read(LLSC).unwrap());
    let mut b = Box_::load(&loaded);
    for i in 1..=4 { assert!(matches!(b.step(), Stop::Step), "step {i}"); }
    assert!(b.dbg_excl().is_some(), "precondition: the stepped ldxr set the shadow");
    assert_checkpoint_parity(b, "mid-pair");
}
```

`crates/retrace/tests/llsc_e2e.rs`: add `SetBy` to the `retrace_core` import, then append:

```rust
// ---- The shadow's life cycle (spec §3a), at the session level ----------------------------------

/// (e): the stepped `ldxr` sets the shadow, and `clrex` clears it.
#[test]
fn the_shadow_is_set_by_a_stepped_ldx_and_cleared_by_clrex() {
    let mut s = retrace_core::seek(trace(), 5, 4).unwrap(); // (e)'s ldxr (K=3) has retired
    assert_eq!(s.pc(), sym("e_clrex"));
    let ex = s.dbg_excl().expect("set by the stepped ldxr");
    assert_eq!((ex.va, ex.size, ex.pair, ex.by), (sym("celle"), 4, false, SetBy::Stepped));
    s.step_insns(1).unwrap(); // clrex
    assert_eq!(s.dbg_excl(), None, "clrex clears the shadow");
}

/// (h): a branch out of the sequence leaves the monitor set, and so the shadow too, until the next
/// non-debug exit.
#[test]
fn the_shadow_outlives_a_branch_out_until_the_next_syscall() {
    let mut s = retrace_core::seek(trace(), 9, 4).unwrap(); // (h)'s ldxr has retired
    assert!(s.dbg_excl().is_some());
    s.step_insns(1).unwrap(); // cbnz, taken: no stxr follows
    assert_eq!(s.pc(), sym("h_after"));
    assert!(s.dbg_excl().is_some(), "the hardware monitor stays set past a branch-out, and so does the shadow");
    assert!(matches!(s.advance().unwrap(), Advance::Event)); // run() steps to the write svc and crosses it
    assert_eq!(s.dbg_excl(), None, "the syscall exit clears it");
}

/// (g): a syscall between the halves clears the shadow, so the store in window 8 fails as it did
/// natively.
#[test]
fn a_syscall_between_the_halves_clears_the_shadow() {
    let mut s = retrace_core::seek(trace(), 7, 4).unwrap(); // (g)'s ldxr has retired
    assert!(s.dbg_excl().is_some());
    assert!(matches!(s.advance().unwrap(), Advance::Event)); // the getpid
    assert_eq!(s.dbg_excl(), None, "the syscall exit clears the shadow");
    assert_eq!(s.pc(), sym("g_stx"));
}

// ---- Review Focus (plan) -----------------------------------------------------------------------

/// Review Focus 1: step into the pair, step back, then continue. `reverse-stepi` re-seeks by
/// stepping, which recomputes the shadow from scratch.
#[test]
fn reverse_stepi_inside_a_pair_then_continue_replays_to_the_end() {
    let out = run_ok("stepi 5; reverse-stepi; where; continue; where");
    assert_eq!(wheres(&out)[0], format!("at (1, 4) pc={:#x} thread=0", sym("a_ldx") + 4), "{out}");
    assert!(out.contains("exited (code 0)"), "{out}");
}

/// Review Focus 2: a checkpoint captured inside a pair carries the shadow, and a session restored
/// from it finishes the pair's store. The research's §4c: without the shadow, every position after
/// a stepped load-exclusive is poison.
#[test]
fn a_checkpoint_taken_inside_a_pair_restores_the_shadow() {
    let s = retrace_core::seek(trace(), 2, 6).unwrap(); // (b)'s first ldaxr (K=5) has retired
    let ex = s.dbg_excl().expect("set by the stepped ldaxr");
    let cp = s.checkpoint();
    drop(s); // one VM per process
    let mut r = ReplaySession::from_checkpoint(trace(), &cp).unwrap();
    assert_eq!(r.dbg_excl(), Some(ex));
    loop {
        if let Advance::Exited(rep) = r.advance().unwrap() {
            assert_eq!(rep.outcome, Outcome::Exit { code: 0 });
            break;
        }
    }
}

/// Review Focus 3: `x` shows the cell before and after the emulated store.
#[test]
fn x_shows_the_cell_before_and_after_the_emulated_store() {
    let out = run_ok(&format!("stepi 5; x {0} 4; stepi; x {0} 4", h("cella")));
    assert!(has_line(&out, &format!("{}: 00 00 00 00", h("cella"))), "before the stxr:\n{out}");
    assert!(has_line(&out, &format!("{}: 42 42 00 00", h("cella"))), "after the stxr:\n{out}");
}
```

Run them to see them fail:
- `cargo test -p retrace-box --test step --test checkpointparity -- --test-threads=1`
- `cargo test -p retrace --test llsc_e2e -- --test-threads=1`

Expected: FAIL to compile, because `dbg_excl`, `SetBy` and `retrace_core::SetBy` are not defined.

- [ ] **Step 3: The shadow and the classifier** (`lib.rs`)

Near `MDSCR_SS`:

```rust
/// M42 (t0 M8): in a software-step exit's ESR, ISS.ISV (bit 24) says the EX bit is valid, and
/// ISS.EX (bit 6) says the stepped instruction was a load-exclusive. Apple's cores and HVF deliver
/// both on the EL2 step exit: `0xcb000062` for a load-exclusive, `0xcb000022` for everything else.
const SS_ISV_EX: u64 = (1 << 24) | (1 << 6);
```

The `Box_` field, after `canary_disturbances: u64,`:

```rust
    /// M42: the shadow of this vCPU's local exclusive monitor (spec §3a).
    /// - Set when `step()` retires a load-exclusive (and, from Task 5, inferred at a native debug
    ///   stop).
    /// - Cleared by every non-debug exit (`note_exit`), a `clrex`, the emulated store-exclusive and
    ///   a thread switch.
    /// - Carried in `BoxState`: a checkpoint is always taken at an exit, where the hardware monitor
    ///   is open, so this is the whole monitor state.
    ///
    /// Declared last. It holds a `Vec`, so it has Drop, but it comes after `vcpu`/`vm`, so the
    /// load-bearing vcpu-before-vm drop order is unaffected.
    excl: Option<Excl>,
```

`BoxState`, after `pub fall_throughs: u64,`:

```rust
    // M42: carried because a mid-pair capture cannot re-derive it. The load-exclusive that set it
    // retired behind the checkpoint, and the hardware monitor never survives an exit. Resetting it
    // would make a seek past a stepped load-exclusive fail that pair's store, which is the bug M42
    // exists to fix (spec §3f).
    pub excl: Option<Excl>,
```

In the three one-line constructors (`load_with_pac` 1395, `load_dynamic` 1997, `restore` 3024),
replace `canary_disturbances: 0 }` with `canary_disturbances: 0, excl: None }`. That is exactly three
occurrences, and a mismatch in the count means a constructor was missed. In `checkpoint()`, after
`fall_throughs: self.fall_throughs,`, add `excl: self.excl.clone(),`. In `from_checkpoint()`, after
`canary_disturbances: 0,`, add:

```rust
            // M42: RESTORED from the capture, never reset (spec §3f); see the `BoxState` field.
            excl: state.excl.clone(),
```

The helpers go right after `step()`:

```rust
    /// M42 §3a: every VM exit `run()` and `run_one_for_step()` take reports its class here.
    ///
    /// Only the three debug exits leave the shadow standing: the EL0 step retire, a breakpoint and
    /// a watchpoint. They exist only in a debugger session. Every other exit ends in an ERET that
    /// clears the hardware monitor (Sail `AArch64_ExceptionReturn`), and record takes that same exit
    /// at the same instruction, so it clears the shadow.
    fn note_exit(&mut self, debug: bool) {
        if !debug { self.excl = None; }
    }

    /// The guest word at VA `va`, read as an instruction, or None if unmapped.
    fn insn_at(&self, va: u64) -> Option<u32> {
        let b = self.read_guest_checked(self.va_to_ipa(va)?, 4)?;
        Some(u32::from_le_bytes(b.try_into().unwrap()))
    }

    /// Xn read as a DATA register, where 31 is XZR. Never `reg::x(31)`, which is the PC in hv-sys.
    fn xreg(&self, r: u32) -> u64 {
        if r == 31 { 0 } else { self.vcpu.get_reg(reg::x(r)).unwrap() }
    }

    /// Xn read as a BASE register, where 31 is SP. The guest runs at EL0, so that is SP_EL0.
    fn base_reg(&self, r: u32) -> u64 {
        if r == 31 { self.vcpu.get_sys(sysreg::SP_EL0).unwrap() } else { self.vcpu.get_reg(reg::x(r)).unwrap() }
    }

    /// M42 §3a: the step exit reported a load-exclusive (t0 M8). Record the shadow from the
    /// instruction at `pc - 4` (a load never branches) and the bytes now at its VA. There is one
    /// vCPU, so memory now IS what the load returned, even when Rt is XZR.
    fn set_excl_from_retire(&mut self) {
        let at = self.pc() - 4;
        let word = self.insn_at(at)
            .unwrap_or_else(|| panic!("M42: ISS.EX retire at {at:#x}, whose word does not map"));
        let ld = decode_excl(word).filter(|i| matches!(i, ExclInsn::Load { .. })).unwrap_or_else(|| panic!(
            "M42: the step exit reported a load-exclusive (ISS.EX) at {at:#x}, but {word:#010x} does not \
             decode as one: the hardware and retrace_arch::decode_excl disagree"));
        assert!(!excl::base_aliases_dest(ld),
            "M42: unmodelled load-exclusive at {at:#x} ({word:#010x}): its base is also a destination, \
             so the marked address is gone");
        let ExclInsn::Load { size, pair, rn, .. } = ld else { unreachable!() };
        let va = self.base_reg(rn) & excl::TAG_MASK;
        let len = excl::access_len(size, pair);
        let loaded = self.va_to_ipa(va).and_then(|ipa| self.read_guest_checked(ipa, len))
            .unwrap_or_else(|| panic!("M42: the load-exclusive at {at:#x} read {va:#x}, which does not map"));
        self.excl = Some(Excl { va, size, pair, loaded, by: SetBy::Stepped });
    }

    /// M42: the exclusive-monitor shadow (spec §3a), for tests and the record/replay asserts.
    pub fn dbg_excl(&self) -> Option<Excl> { self.excl.clone() }
```

In `run_one_for_step()`:
- Leave its vtimer line (2809) unchanged (plan R12), and add a comment above it:

```rust
            // M42 (plan R12): a vtimer/cancel exit inside a step is retrace's own, like the step
            // exit. Record never takes it at this instruction, so it leaves the shadow standing.
            if e.reason != EXIT_EXCEPTION { continue; }
```
- Replace the EL0-retire line `if (cpsr >> 2) & 3 == 0 { return Stop::Step; }` with:

```rust
                    if (cpsr >> 2) & 3 == 0 {
                        self.note_exit(true);
                        // M42 §3a (t0 M8): the step exit says whether a load-exclusive just retired.
                        if e.syndrome & SS_ISV_EX == SS_ISV_EX { self.set_excl_from_retire(); }
                        return Stop::Step;
                    }
                    // M42: the stepped instruction trapped to EL1 (F2). That is an exception entry and,
                    // below, an ERET: not a debug exit, whichever arm follows.
                    self.note_exit(false);
```

- Its outer `_` arm becomes:

```rust
                _ => {
                    // M42 §3a: a breakpoint or watchpoint is a debug exit; a stage-2 abort is not.
                    self.note_exit(matches!(ec_of(e.syndrome), Ec::Breakpoint | Ec::Watchpoint));
                    self.last_far = e.virtual_address;
                    return Stop::Other { esr: e.syndrome };
                }
```

In `run()`:
- Change its vtimer line (2660) to
  `if e.reason != EXIT_EXCEPTION { self.note_exit(false); continue; } // vtimer/canceled: control-plane only`.
  The shadow is always clear here (§3e), so this is the §3a "every arm classifies" guard, not a
  behaviour change.
- Add `self.note_exit(false);` as the first statement of the `Ec::Hvc => {` arm, with the comment
  `// M42 §3a: every exception EL0 takes to EL1 comes here, and resumes by an ERET.`
- Make the outer `_` arm call `self.note_exit(matches!(ec_of(e.syndrome), Ec::Breakpoint | Ec::Watchpoint));`
  before its `self.last_far = …`. Task 5 replaces that call.

In `switch_to_thread`, after the `cur == tid` early return:

```rust
        // M42 §3a: the monitor belongs to the PE. Every switch follows a syscall exit, which has
        // already cleared the shadow; clearing it here states that a switch can never carry one.
        self.excl = None;
```

- [ ] **Step 4: The emulated store-exclusive in `step()`** (§3b)

Refactor `va_to_ipa` into a walk that also returns the leaf, keeping its doc comment and behaviour.
`va_to_ipa`'s body becomes `self.va_leaf(va).map(|(ipa, _)| ipa)`, and this goes after it:

```rust
    /// M42: `va_to_ipa`'s walk, also returning the stage-1 leaf descriptor, whose AP bits say whether
    /// EL0 may write (`excl::el0_writable`). With the MMU off there is no leaf, so it returns `None`
    /// in that slot.
    fn va_leaf(&self, va: u64) -> Option<(u64, Option<u64>)> {
        let sctlr = self.vcpu.get_sys(sysreg::SCTLR_EL1).unwrap();
        if sctlr & 1 == 0 { return Some((va, None)); }
        if va >> 47 != 0 { return None; }
        let l1e = self.pt_entry(PT_L1_IPA, (va >> 36) & 0x7FF)?;
        if l1e & 0x3 != DESC_TABLE { return None; }
        let l2e = self.pt_entry(l1e & PT_ADDR, (va >> 25) & 0x7FF)?;
        match l2e & 0x3 {
            DESC_BLOCK => Some(((l2e & PT_ADDR & !(BLK - 1)) | (va & (BLK - 1)), Some(l2e))),
            DESC_TABLE => {
                let l3e = self.pt_entry(l2e & PT_ADDR, (va >> 14) & 0x7FF)?;
                if l3e & 0x3 != DESC_PAGE { return None; }
                Some(((l3e & PT_ADDR) | (va & (GRANULE as u64 - 1)), Some(l3e)))
            }
            _ => None,
        }
    }
```

After `set_excl_from_retire`:

```rust
    /// Is a hardware breakpoint armed at `pc`? `Box_` keeps no breakpoint list (`arm_hw_breakpoint`),
    /// so read the six slots back: the registers are the ground truth.
    fn bp_armed_at(&self, pc: u64) -> bool {
        self.bps_armed && HW_BREAKPOINT_SLOTS.iter().any(|&(bvr, bcr)|
            self.vcpu.get_sys(bcr).unwrap() == DBGBCR_ARM && self.vcpu.get_sys(bvr).unwrap() == pc)
    }

    /// Does an armed write-watch range overlap `[va, va + len)`? It is byte-exact, as the hardware's
    /// BAS match is.
    fn watch_overlaps(&self, va: u64, len: usize) -> bool {
        self.wps_armed && self.watch_ranges.iter().any(|&(w, wl)| w < va + len as u64 && va < w + wl)
    }

    /// M42 §3c: the debug stop the hardware would raise at an emulated store-exclusive. Raising it
    /// is Task 4. Until then, refuse loudly rather than emulate past a hit: skipping it would be
    /// silent.
    fn raise_debug_stop(&mut self, pc: u64, va: u64, len: usize) -> Option<Stop> {
        assert!(!self.bp_armed_at(pc) && !self.watch_overlaps(va, len),
            "M42: a breakpoint or watch applies at the emulated store-exclusive at {pc:#x}, and it is \
             not yet raised (Task 4)");
        None
    }

    /// M42 §3b: `step()` at a store-exclusive while the shadow is set. retrace's own exits lost the
    /// hardware monitor, so the store is performed here, as the native run performed it. The order
    /// is: validate, then raise the stops the hardware would raise, then write.
    fn emulate_stx(&mut self, st: ExclInsn) -> Stop {
        let ExclInsn::Store { size, pair, rt, rt2, rn, .. } = st else { unreachable!("emulate_stx: {st:?}") };
        let ex = self.excl.clone().expect("emulate_stx without a shadow");
        let pc = self.pc();
        let base = self.base_reg(rn);
        let len = excl::access_len(size, pair);
        let leaf = self.va_leaf(base & excl::TAG_MASK);
        let target = leaf.and_then(|(ipa, _)| self.read_guest_checked(ipa, len));
        let writable = match leaf { Some((_, None)) => true, Some((_, Some(d))) => excl::el0_writable(d), None => false };
        let plan = excl::plan_stx(&ex, st, base, self.xreg(rt), self.xreg(rt2), target.as_deref(), writable)
            .unwrap_or_else(|why| panic!("M42: unmodelled store-exclusive at pc {pc:#x}: {why} (shadow {ex:?})"));
        if let Some(stop) = self.raise_debug_stop(pc, plan.va, len) { return stop; }
        let (ipa, _) = leaf.expect("plan_stx refuses an unmapped target");
        self.write_guest(ipa, &plan.bytes);
        if let Some(s) = plan.status { self.vcpu.set_reg(reg::x(s), 0).unwrap(); }
        self.vcpu.set_reg(reg::PC, pc + 4).unwrap();
        self.excl = None;
        Stop::Step
    }
```

In `step()`, between the M15 `debug_assert!` and `let mdscr = …`:

```rust
        // M42 §3b: inside a pair, retrace's own exits have cleared the hardware monitor, so a
        // store-exclusive is emulated rather than stepped, and a `clrex` is stepped and then clears
        // the shadow. This sits after the reschedule, because a switch clears the shadow, and before
        // the SS arming, because the emulation takes no exit.
        let mut clrex_next = false;
        if self.excl.is_some() {
            match self.insn_at(self.pc()).and_then(decode_excl) {
                Some(st @ ExclInsn::Store { .. }) => return self.emulate_stx(st),
                Some(ExclInsn::Clrex) => clrex_next = true,
                _ => {}
            }
        }
```

At the end of `step()`, replace the final `stop` with:

```rust
        if clrex_next && matches!(stop, Stop::Step) { self.excl = None; }
        stop
```

- [ ] **Step 5: `run()` entered inside a pair** (§3e, plan R9/R10)

Near `SS_ISV_EX`:

```rust
/// M42 §3e (plan R10): the most instructions `run()` steps to finish a pair before resuming
/// natively. It is the §3d scan bound, and gdb's.
const PAIR_STEP_BOUND: usize = 16;
```

In `run()`, between the M15 `debug_assert!` and `loop {`:

```rust
        // M42 §3e: entering the guest is an ERET, which clears the hardware monitor, so native
        // execution cannot resume inside a pair. Step until the shadow clears. What each stop does:
        // - A debug stop or a stage-2 abort met on the way returns exactly as run() would return it.
        // - A stop that came through the guest's EL1 vector (a syscall, an EL1 fault, an unemulated
        //   trap) leaves the guest parked at EL1. The native loop below re-enters there and delivers
        //   that same stop through run()'s own arms: the path M41's `AtTrap` -> `advance()` takes.
        // - Bounded at PAIR_STEP_BOUND (plan R10). A shadow that outlives it belongs to a load whose
        //   sequence a branch left (fixture shape (h)). It is dropped, which is what resuming
        //   natively does to the hardware monitor anyway.
        for _ in 0..PAIR_STEP_BOUND {
            if self.excl.is_none() { break; }
            let stop = self.step();
            if (self.vcpu.get_reg(reg::CPSR).unwrap() >> 2) & 3 != 0 { break; }
            if !matches!(stop, Stop::Step) { return stop; }
        }
        self.excl = None;
```

- [ ] **Step 6: The record/replay asserts and the session accessor** (`retrace-core`)

- Change line 9 to also export the shadow: add `pub use retrace_box::{Excl, SetBy};` on its own
  line after it.
- In `record_box`, immediately after `let stop = b.run();`:

```rust
        // M42 §3f: record never steps and never arms a debug register, so no debug exit reaches it
        // and the exclusive shadow can never be set here. A shadow would mean an exit path skipped
        // `note_exit`.
        assert!(b.dbg_excl().is_none(), "M42: record set the exclusive shadow at pc {:#x}: {:?}",
            b.pc(), b.dbg_excl());
```

- `replay()`'s loop becomes:

```rust
    loop {
        let adv = s.advance()?;
        // M42 §3f: plain replay never arms a debug register either; see record_box's twin assert.
        assert!(s.dbg_excl().is_none(), "M42: plain replay set the exclusive shadow at landmark {}", s.landmark());
        if let Advance::Exited(report) = adv { return Ok(report); }
    }
```

- Beside `ReplaySession::dbg_regs`:

```rust
    /// M42: the box's exclusive-monitor shadow (spec §3a).
    pub fn dbg_excl(&self) -> Option<Excl> { self.b.dbg_excl() }
```

- [ ] **Step 7: Run everything this task touches**

```bash
cargo test -p retrace-box --lib -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m42-llsc/t3-box-lib.log 2>&1; echo "exit=$?"
cargo test -p retrace-box --test step --test checkpointparity --test checkpoint --test threads --test restoreparity -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m42-llsc/t3-box.log 2>&1; echo "exit=$?"
cargo test -p retrace --test llsc_e2e --no-fail-fast -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m42-llsc/t3-llsc.log 2>&1; echo "exit=$?"
grep -a -e '^test ' -e 'test result' .superpowers/sdd/2026-09-24-retrace-m42-llsc/t3-llsc.log
```

Expected in `llsc_e2e`:
- **Green:** the recording test, the three controls, M2, M3a–e, M3d′, both M7 tests, the
  branch-out/exit-window test, the three life-cycle tests and the three Review Focus tests.
- **Red:** every M4, M5 and M6 test, forward and backward. They fail with the "not yet raised
  (Task 4)" panic or a divergence, never with a hang. Ledger the symptoms. A forward `continue`
  stops NATIVELY at the store-exclusive, and the shadow exists there only from Task 5's inference.

- [ ] **Step 8: Show each guard able to fail** (M28's lesson). **Run this step after Step 9's
commit**, on the committed tree (Global Constraints: controls). Ledger each run's result, then undo
the control with `git checkout -- <file>` and confirm `git status --short` lists no modified
tracked file.

| # | Deletion | Test that must go RED |
|---|---|---|
| C1 | `note_exit`'s body becomes a no-op | `control_f_…`, `control_g_…` |
| C2 | Only the trapped-branch `self.note_exit(false);` in `run_one_for_step` is deleted | `control_f_…` |
| C3 | The `clrex_next` clear is deleted | `control_e_…` and `the_shadow_is_set_by_a_stepped_ldx_and_cleared_by_clrex` |
| C4 | `from_checkpoint`'s `excl: state.excl.clone(),` becomes `excl: None,` | `a_checkpointed_box_inside_an_exclusive_pair_matches_the_box_it_came_from` and `a_checkpoint_taken_inside_a_pair_restores_the_shadow` |
| C5 | The §3e prologue loop is deleted | `m2_stepi_past_the_ldxr_then_continue_replays_to_the_end` |

The `switch_to_thread` clear has no control, because it is unobservable: every switch follows a
syscall exit that has already cleared the shadow. Say so in the report.

- [ ] **Step 8b: No existing suite moved**

```bash
cargo test -p retrace --test debug_cli --test watch_cli --test watch --test hitorder_e2e --test watchsweep_e2e --test reverse_debug_e2e --test checkpoint_seek --test crashy_cli --test thread_watch_e2e --no-fail-fast -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m42-llsc/t3-stepping-suites.log 2>&1; echo "exit=$?"
cargo test -p retrace-core -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m42-llsc/t3-core.log 2>&1; echo "exit=$?"
cargo test -p retrace --bins -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m42-llsc/t3-bins.log 2>&1; echo "exit=$?"
```

Expected: all green. A failure here is the halt rule "an existing test's output moved": stop and
report, and do not edit the expectation.

- [ ] **Step 9: Clippy, then commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/retrace-box/src/excl.rs crates/retrace-box/src/lib.rs crates/retrace-box/tests/step.rs crates/retrace-box/tests/checkpointparity.rs crates/retrace-core/src/lib.rs crates/retrace/tests/llsc_e2e.rs
git commit -m "M42 t3: a shadow exclusive monitor; step() emulates the store-exclusive; run() finishes a pair before resuming

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Raise the debug stops the hardware would (§3c)

**Files:**
- Modify: `crates/retrace-box/src/lib.rs`: `raise_debug_stop`, and `watch_overlaps` becomes
  `watch_hit`.
- Modify: `crates/retrace/tests/llsc_e2e.rs`: the oracle ground-truth lists.

**Interfaces:**
- Consumes (Task 3): `raise_debug_stop(&mut self, pc: u64, va: u64, len: usize) -> Option<Stop>`,
  `bp_armed_at`, `watch_overlaps`.
- Produces: `raise_debug_stop` returns `Stop::Other { esr }` with EC `0x30` or `0x34`, and sets
  `last_far`.

- [ ] **Step 1: Write the ground-truth tests** (append to `llsc_e2e.rs`; add `mod` imports
  `use util::hits::{self, Phase};`)

```rust
// ---- The hit oracle's ground truth on llsc (spec §4) ------------------------------------------
// `enumerate_hits` single-steps from (1, 0) with everything armed, so it steps every pair. Each
// list below is derived from the fixture SOURCE, not from the emulator: a Bp comes before a Watch
// at one coordinate (M41 §3b).

fn keys(hs: &[hits::Hit]) -> Vec<(usize, u64, Phase, u64)> { hs.iter().map(|h| (h.n, h.k, h.phase, h.pc)).collect() }

/// A1: {b_stx, i_stx} + watch ctr. (b)'s stlxr at K = 7, 14, 21 is a breakpoint, then a watched
/// store, each time. (i)'s stlxr at (10, 6) is a breakpoint on an unwatched cell.
fn a1() -> (Vec<u64>, Vec<(u64, u64)>) { (vec![sym("b_stx"), sym("i_stx")], vec![(sym("ctr"), 8)]) }
/// A2: watch pair+8 + break i_svc. The stxp at (4, 7) covers pair[1]; the exit svc is (10, 10).
fn a2() -> (Vec<u64>, Vec<(u64, u64)>) { (vec![sym("i_svc")], vec![(sym("pair") + 8, 8)]) }
/// A3: {a_ldx, a_stx, g_stx, i_svc} + watch cella. The breakpoint on the LOAD itself comes first,
/// then (a)'s stxr (a Bp and a Watch), then g_stx at (8, 0), where the syscall has cleared the
/// shadow so the hardware breakpoint fires, then the exit svc. (h) stores nothing.
fn a3() -> (Vec<u64>, Vec<(u64, u64)>) {
    (vec![sym("a_ldx"), sym("a_stx"), sym("g_stx"), sym("i_svc")], vec![(sym("cella"), 4)])
}

#[test]
fn oracle_a1_lists_every_hit_the_source_implies() {
    let (bps, ws) = a1();
    let b = sym("b_stx");
    assert_eq!(keys(&hits::enumerate_hits(trace(), &bps, &ws, 1)), vec![
        (2, 7, Phase::Bp, b), (2, 7, Phase::Watch, b), (2, 14, Phase::Bp, b), (2, 14, Phase::Watch, b),
        (2, 21, Phase::Bp, b), (2, 21, Phase::Watch, b), (10, 6, Phase::Bp, sym("i_stx")),
    ]);
}

#[test]
fn oracle_a2_lists_every_hit_the_source_implies() {
    let (bps, ws) = a2();
    assert_eq!(keys(&hits::enumerate_hits(trace(), &bps, &ws, 1)),
        vec![(4, 7, Phase::Watch, sym("d_stx")), (10, 10, Phase::Bp, sym("i_svc"))]);
}

#[test]
fn oracle_a3_lists_every_hit_the_source_implies() {
    let (bps, ws) = a3();
    let a = sym("a_stx");
    assert_eq!(keys(&hits::enumerate_hits(trace(), &bps, &ws, 1)), vec![
        (1, 3, Phase::Bp, sym("a_ldx")), (1, 5, Phase::Bp, a), (1, 5, Phase::Watch, a),
        (8, 0, Phase::Bp, sym("g_stx")), (10, 10, Phase::Bp, sym("i_svc")),
    ]);
}
```

Run: `cargo test -p retrace --test llsc_e2e oracle_ -- --test-threads=1`
Expected: FAIL. A1 and A3 panic with "not yet raised (Task 4)". A2 panics the same way, at the stxp
with the watch armed. Ledger the panics: this is the oracle lists' RED.

- [ ] **Step 2: Implement** (`lib.rs`)

Near `SS_ISV_EX`:

```rust
/// M42 §3c: the syndromes of the debug stops raised at an emulated store-exclusive:
/// - EC 0x30 is a breakpoint from a lower EL, and EC 0x34 a watchpoint from a lower EL;
/// - IL is set, and so is WnR for the watch.
///
/// Every consumer reads only `ec_of(esr)` (`advance`, `step_watched`, `step_armed`).
const ESR_RAISED_BP: u64 = (0x30 << 26) | (1 << 25);
const ESR_RAISED_WATCH: u64 = (0x34 << 26) | (1 << 25) | (1 << 6);
```

Replace `watch_overlaps` with `watch_hit`. Delete `watch_overlaps`, since `raise_debug_stop` is its
only caller:

```rust
    /// The lowest byte of `[va, va + len)` that an armed write-watch range covers, or None (plan
    /// R11). That byte lies in both the access and the watch, so `watched_of` resolves it by exact
    /// byte. It is byte-exact, as the hardware's BAS match is.
    fn watch_hit(&self, va: u64, len: usize) -> Option<u64> {
        if !self.wps_armed { return None; }
        self.watch_ranges.iter()
            .filter(|&&(w, wl)| w < va + len as u64 && va < w + wl)
            .map(|&(w, _)| w.max(va))
            .min()
    }
```

`raise_debug_stop` becomes:

```rust
    /// M42 §3c: the debug stop the hardware would raise at an emulated store-exclusive, in hardware
    /// order: a breakpoint armed at `pc` first, then a watch the access overlaps. A watch fires even
    /// on a store-exclusive that will fail (t0 M5), so raising it before emulating the store gives
    /// the same answer the hardware gives. The caller clears what fired and steps again, so the
    /// next call raises the next stop or emulates.
    fn raise_debug_stop(&mut self, pc: u64, va: u64, len: usize) -> Option<Stop> {
        if self.bp_armed_at(pc) {
            self.last_far = pc; // no consumer reads a breakpoint's FAR (spec R5)
            return Some(Stop::Other { esr: ESR_RAISED_BP });
        }
        let far = self.watch_hit(va, len)?;
        self.last_far = far;
        Some(Stop::Other { esr: ESR_RAISED_WATCH })
    }
```

- [ ] **Step 3: Run**

```bash
cargo test -p retrace --test llsc_e2e --no-fail-fast -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m42-llsc/t4-llsc.log 2>&1; echo "exit=$?"
grep -a -e '^test ' -e 'test result' .superpowers/sdd/2026-09-24-retrace-m42-llsc/t4-llsc.log
```

Expected:
- **Green:** everything green after Task 3, plus the three oracle lists.
- **May be green:** a **forward** M4, M5 or M6 test, if the debugger resolves the native hit's K by
  stepping and continues from that stepped position. Record which.
- **Red:** every **backward** M4, M5 and M6 test, and every forward one not green above. They die
  by the 60 s bound or a divergence: a native breakpoint or watch stop inside the pair infers no
  shadow before Task 5.

- [ ] **Step 4: Show the order is load-bearing** (after Step 5's commit, on the committed tree;
ledgered, then undone with `git checkout -- <file>`). Swap the two checks in
`raise_debug_stop`, raising the watch first. `oracle_a1_lists_every_hit_the_source_implies` must go
RED, with Watch before Bp at `(2, 7)`.

- [ ] **Step 5: The stepping suites still pass, then clippy and commit**

Run the Task 3 Step 8b commands again, into `t4-stepping-suites.log` and `t4-core.log`. Then:

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/retrace-box/src/lib.rs crates/retrace/tests/llsc_e2e.rs
git commit -m "M42 t4: raise the breakpoint and watch stops the hardware would at an emulated store-exclusive

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Infer the shadow at a native debug stop (§3d)

**Files:**
- Modify: `crates/retrace-box/src/excl.rs`: `scan_back`, `dests_match`, and their tests.
- Modify: `crates/retrace-box/src/lib.rs`: `run()`'s entry pc, its outer `_` arm, and
  `infer_excl`.
- Modify: `crates/retrace/tests/llsc_e2e.rs`: the inference tests and the oracle chains.
- Modify: `crates/retrace/tests/util/hits.rs`: the module doc.

**Interfaces:**
- Consumes (Tasks 1 and 3): `decode_excl`, `is_fallthrough_barrier`, `excl::{base_aliases_dest, access_len, TAG_MASK}`,
  `Box_::{insn_at, base_reg, xreg, note_exit}`.
- Produces: `excl::scan_back(words: &[u32]) -> Option<(usize, ExclInsn)>`,
  `excl::dests_match(ld: ExclInsn, rt_val: u64, rt2_val: u64, bytes: &[u8]) -> bool`,
  `Box_::infer_excl(&mut self, entry_pc: u64)`.

- [ ] **Step 1: The pure scan and its tests** (`excl.rs`; change the import to
  `use retrace_arch::{decode_excl, is_fallthrough_barrier, ExclInsn};`)

```rust
/// Spec §3d's backward scan.
///
/// `words[i]` is the instruction at `P - 4 * (i + 1)`, nearest first. The caller bounds the slice
/// at 16 words and never goes past the start of P's page.
///
/// Returns the nearest load-exclusive and its index. Returns None if a store-exclusive, a `clrex` or
/// a fall-through barrier comes first: past any of those, the monitor cannot still hold that load's
/// mark on the path that falls through to P.
pub fn scan_back(words: &[u32]) -> Option<(usize, ExclInsn)> {
    for (i, &w) in words.iter().enumerate() {
        match decode_excl(w) {
            Some(ld @ ExclInsn::Load { .. }) => return Some((i, ld)),
            Some(_) => return None,
            None if is_fallthrough_barrier(w) => return None,
            None => {}
        }
    }
    None
}

/// Spec §3d condition 3: the load's destination registers still hold what is at its VA. A jump into
/// the middle of a pair, or a rewritten register, almost always breaks this, and then nothing is
/// inferred. A destination of 31 (XZR) loads nothing to compare.
pub fn dests_match(ld: ExclInsn, rt_val: u64, rt2_val: u64, bytes: &[u8]) -> bool {
    let ExclInsn::Load { size, pair, rt, rt2, .. } = ld else { return false };
    let s = size as usize;
    (rt == 31 || rt_val.to_le_bytes()[..s] == bytes[..s])
        && (!pair || rt2 == 31 || rt2_val.to_le_bytes()[..s] == bytes[s..2 * s])
}
```

Tests, appended to `excl.rs`'s `mod tests`:

```rust
    #[test]
    fn the_scan_finds_the_nearest_load_across_neutral_instructions() {
        // dyld's getpid, back from its stxr: cbnz, then ldxr.
        assert_eq!(scan_back(&[0x3500_004a, 0x885f_7d2a]).map(|(i, _)| i), Some(1));
        // (b), back from its stlxr: add x1, x1, #1, then ldaxr.
        assert_eq!(scan_back(&[0x9100_0421, 0xc85f_fc01]).map(|(i, _)| i), Some(1));
        // (f), back from its stxr: the mrs is not a barrier. The ENTRY check rejects this one, not
        // the scan.
        assert!(scan_back(&[0xd53b_e047, 0x885f_7c06]).is_some());
    }

    #[test]
    fn the_scan_stops_at_a_store_exclusive_a_clrex_or_a_barrier() {
        assert_eq!(scan_back(&[0x881f_7d20, 0x885f_7d2a]), None); // an stxr consumed the older load
        assert_eq!(scan_back(&[0xd503_3f5f, 0x885f_7d2a]), None); // clrex
        assert_eq!(scan_back(&[0xd400_1001, 0x885f_7d2a]), None); // svc
        assert_eq!(scan_back(&[0x1400_0002, 0x885f_7d2a]), None); // b
        assert_eq!(scan_back(&[0xd65f_03c0, 0x885f_7d2a]), None); // ret
        assert_eq!(scan_back(&[0xd503_201f; 16]), None);          // sixteen nops, no load
    }

    #[test]
    fn the_destination_check_compares_each_element_and_skips_xzr() {
        let ldxr = ExclInsn::Load { size: 4, pair: false, rt: 10, rt2: 31, rn: 9 };
        assert!(dests_match(ldxr, 0x4242, 0, &[0x42, 0x42, 0, 0]));
        assert!(!dests_match(ldxr, 0x4343, 0, &[0x42, 0x42, 0, 0]));
        let ldxp = ExclInsn::Load { size: 8, pair: true, rt: 1, rt2: 2, rn: 0 };
        let b: Vec<u8> = [1u64.to_le_bytes(), 2u64.to_le_bytes()].concat();
        assert!(dests_match(ldxp, 1, 2, &b));
        assert!(!dests_match(ldxp, 1, 3, &b));
        let xzr = ExclInsn::Load { size: 8, pair: false, rt: 31, rt2: 31, rn: 0 };
        assert!(dests_match(xzr, 99, 0, &[7; 8]), "ldxr xzr loads nothing to compare");
    }
```

`0xd53be047` is `mrs x7, cntvct_el0` and `0x885f7c06` is `ldxr w6, [x0]`. If Task 2 switched (f)
to the Apple counter, use that word instead. Assemble it and read it with `otool -tv`.

Run: `cargo test -p retrace-box --lib excl -- --test-threads=1`, which should give 12 PASS.

- [ ] **Step 2: Write the session-level inference tests** (append to `llsc_e2e.rs`)

```rust
// ---- Inference at a native debug stop (spec §3d) -----------------------------------------------

/// Replay a session forward to the end: exit 0 with the recorded stdout. The session must come from
/// `open` or `seek`, which replay from the snapshot and so collect every write; a `from_checkpoint`
/// session starts with an empty stdout.
fn finish(mut s: ReplaySession) {
    loop {
        if let Advance::Exited(r) = s.advance().unwrap_or_else(|d| panic!("diverged at landmark {}: {}", d.landmark, d.detail)) {
            assert_eq!((r.outcome, r.stdout.as_slice()), (Outcome::Exit { code: 0 }, STDOUT));
            return;
        }
    }
}

/// E2: a native breakpoint ON (a)'s stxr. The ldxr ran natively, and the resuming ERET would break
/// the pair, so the shadow is inferred here and stepping off emulates the store (M41's
/// `diag_q4_break_between`).
#[test]
fn a_native_breakpoint_on_the_store_infers_the_shadow_and_the_store_lands() {
    let mut s = ReplaySession::open(trace()).unwrap();
    s.arm_breakpoints(&[sym("a_stx")]);
    assert!(matches!(s.advance().unwrap(), Advance::Break));
    assert_eq!(s.pc(), sym("a_stx"));
    let ex = s.dbg_excl().expect("inferred at the native stop");
    assert_eq!((ex.va, ex.size, ex.pair, ex.by), (sym("cella"), 4, false, SetBy::Inferred));
    s.clear_breakpoints();
    s.step_insns(1).unwrap();
    assert_eq!(s.read_mem(sym("cella"), 4), Some(vec![0x42, 0x42, 0, 0]));
    assert_eq!(s.dbg_excl(), None);
    finish(s);
}

/// E2 between the halves: a breakpoint on (a)'s cbnz. The inferred shadow survives the step off it,
/// and run()'s prologue emulates the stxr.
#[test]
fn a_native_breakpoint_between_the_halves_infers_the_shadow() {
    let mut s = ReplaySession::open(trace()).unwrap();
    s.arm_breakpoints(&[sym("a_ldx") + 4]);
    assert!(matches!(s.advance().unwrap(), Advance::Break));
    assert!(s.dbg_excl().is_some());
    s.clear_breakpoints();
    finish(s);
}

/// E3: a native watch stop at (b)'s stlxr infers the shadow, and stepping over the store emulates it.
/// Without inference the store failed and the loop re-entered: 4 entries against the recorded 3.
#[test]
fn a_native_watch_stop_on_the_retry_store_infers_the_shadow() {
    let mut s = retrace_core::seek(trace(), 2, 0).unwrap(); // past window 1's write, which ends the first advance
    assert_eq!(s.read_mem(sym("ctr"), 8), Some(vec![0; 8]));
    s.arm_watchpoints(&[(sym("ctr"), 8)]);
    assert!(matches!(s.advance().unwrap(), Advance::Watch { .. }));
    assert_eq!(s.pc(), sym("b_stx"));
    assert_eq!(s.dbg_excl().map(|e| (e.va, e.size, e.by)), Some((sym("ctr"), 8, SetBy::Inferred)));
    s.clear_watchpoints();
    s.step_insns(1).unwrap();
    assert_eq!(s.read_mem(sym("ctr"), 8), Some(1u64.to_le_bytes().to_vec()));
    finish(s);
}

/// Review Focus 4: a stop ON the load-exclusive infers nothing. It has not run yet, and the scan
/// back from (b)'s ldaxr meets (a)'s write svc before any load.
#[test]
fn a_native_stop_on_the_load_itself_infers_nothing() {
    let mut s = retrace_core::seek(trace(), 2, 0).unwrap();
    s.arm_breakpoints(&[sym("b_ldx")]);
    assert!(matches!(s.advance().unwrap(), Advance::Break));
    assert_eq!(s.pc(), sym("b_ldx"));
    assert_eq!(s.dbg_excl(), None);
}

/// (e): the scan meets the `clrex` first, so nothing is inferred, and the store fails as recorded.
#[test]
fn a_native_stop_after_clrex_infers_nothing() {
    let mut s = retrace_core::seek(trace(), 5, 0).unwrap();
    s.arm_breakpoints(&[sym("e_stx")]);
    assert!(matches!(s.advance().unwrap(), Advance::Break));
    assert_eq!(s.dbg_excl(), None);
    s.clear_breakpoints();
    finish(s);
}

/// (f): the emulated timebase read re-entered the guest AT f_stx. That entry's ERET cleared the
/// monitor, so the entry check infers nothing, and the store fails as recorded (status 1).
#[test]
fn a_native_stop_after_a_reentry_inside_the_pair_infers_nothing() {
    let mut s = retrace_core::seek(trace(), 6, 0).unwrap();
    s.arm_breakpoints(&[sym("f_stx")]);
    assert!(matches!(s.advance().unwrap(), Advance::Break));
    assert_eq!(s.pc(), sym("f_stx"));
    assert_eq!(s.dbg_excl(), None, "the last entry was at f_stx itself, inside (ldxr, stxr]");
    s.clear_breakpoints();
    finish(s);
}

// ---- The oracle's three chains on llsc (M41 §3e) -----------------------------------------------

fn arming(bps: &[u64], ws: &[(u64, u64)]) -> String {
    bps.iter().map(|b| format!("break {b:#x}")).chain(ws.iter().map(|(a, l)| format!("watch {a:#x} {l}")))
        .collect::<Vec<_>>().join("; ")
}

#[test]
fn oracle_a1_chains() {
    let (bps, ws) = a1();
    hits::check_chains(ts(), &arming(&bps, &ws), &hits::enumerate_hits(trace(), &bps, &ws, 1));
}
#[test]
fn oracle_a2_chains() {
    let (bps, ws) = a2();
    hits::check_chains(ts(), &arming(&bps, &ws), &hits::enumerate_hits(trace(), &bps, &ws, 1));
}
#[test]
fn oracle_a3_chains() {
    let (bps, ws) = a3();
    hits::check_chains(ts(), &arming(&bps, &ws), &hits::enumerate_hits(trace(), &bps, &ws, 1));
}
```

`seek(trace(), n, 0)` stops before window `n`, so each test's `advance()` runs natively only
through window `n`.

Run: `cargo test -p retrace --test llsc_e2e --no-fail-fast -- --test-threads=1`
Expected: the four inference tests that expect a shadow FAIL (`dbg_excl()` is None, or a divergence
at `finish`). The three "infers nothing" tests PASS already. The backward (and any still-red forward) M4–M6 tests and the three
`oracle_*_chains` fail: the chains die at the 600 s bound or with exit 5. **The chains can take up
to 600 s each when red. To save the wait, it is acceptable to run only the inference tests red, and
say so in the report.**

- [ ] **Step 3: Implement** (`lib.rs`)

In `run()`'s `loop {`, make the first statement `let entry_pc = self.pc();`, with the comment:

```rust
            // M42 §3d: where this entry resumes the guest. That entry's ERET cleared the monitor, so
            // a native stop cannot infer a load-exclusive at or before a point after it.
```

`run()`'s outer `_` arm becomes:

```rust
                _ => {
                    self.last_far = e.virtual_address;
                    if matches!(ec_of(e.syndrome), Ec::Breakpoint | Ec::Watchpoint) {
                        self.note_exit(true);
                        self.infer_excl(entry_pc);
                    } else {
                        self.note_exit(false);
                    }
                    return Stop::Other { esr: e.syndrome };
                }
```

After `emulate_stx`:

```rust
    /// M42 §3d: a breakpoint or watchpoint stop taken NATIVELY by `run()` at `P = pc()`.
    ///
    /// If a load-exclusive ran natively since the last entry, the ERET that resumes the guest breaks
    /// its pair just as a step does, so the shadow is inferred here. This is M42's one heuristic.
    /// Every condition below that fails infers nothing. That is the pre-M42 behaviour, never a wrong
    /// emulation.
    fn infer_excl(&mut self, entry_pc: u64) {
        debug_assert!(self.excl.is_none(), "run()'s native loop runs only with the shadow clear (§3e)");
        let p = self.pc();
        let page = p & !(GRANULE as u64 - 1);
        let mut words = Vec::with_capacity(16);
        let mut a = p;
        while words.len() < 16 && a > page {
            a -= 4;
            let Some(w) = self.insn_at(a) else { break };
            words.push(w);
        }
        let Some((i, ld)) = excl::scan_back(&words) else { return };
        let l = p - 4 * (i as u64 + 1);
        // 1. The last entry came after the load, and that entry's ERET cleared the monitor.
        if entry_pc > l && entry_pc <= p { return; }
        // 2. A base overwritten by the load no longer names the marked address.
        if excl::base_aliases_dest(ld) { return; }
        let ExclInsn::Load { size, pair, rt, rt2, rn } = ld else { unreachable!() };
        let va = self.base_reg(rn) & excl::TAG_MASK;
        // 4. The target maps.
        let Some(bytes) = self.va_to_ipa(va).and_then(|ipa| self.read_guest_checked(ipa, excl::access_len(size, pair)))
            else { return };
        // 3. The destinations still hold what is there.
        if !excl::dests_match(ld, self.xreg(rt), self.xreg(rt2), &bytes) { return; }
        self.excl = Some(Excl { va, size, pair, loaded: bytes, by: SetBy::Inferred });
    }
```

In `crates/retrace/tests/util/hits.rs`, replace the module doc's paragraph beginning "The oracle is
exact only over windows with no load/store-exclusive pair" with:

```rust
//! Since M42 a pair no longer limits the oracle. Stepping an exclusive pair keeps its store
//! (`Box_`'s shadow monitor), so every hit here is a HARDWARE stop, or the stop the emulator raises
//! in its place at an emulated store-exclusive, in hardware order (`Box_::raise_debug_stop`). The
//! llsc fixture's ground-truth lists (`llsc_e2e`) pin those raised stops against the fixture
//! source, independently of the emulator.
```

- [ ] **Step 4: Run, and show the entry check able to fail**

```bash
cargo test -p retrace-box --lib -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m42-llsc/t5-box-lib.log 2>&1; echo "exit=$?"
cargo test -p retrace --test llsc_e2e --no-fail-fast -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m42-llsc/t5-llsc.log 2>&1; echo "exit=$?"
grep -a -e '^test ' -e 'test result' .superpowers/sdd/2026-09-24-retrace-m42-llsc/t5-llsc.log
```

Expected: **every** `llsc_e2e` test PASSES.

Control C6 (after Step 5's commit, on the committed tree; ledgered, then undone with
`git checkout -- <file>`): delete the `entry_pc` check (condition 1).
`a_native_stop_after_a_reentry_inside_the_pair_infers_nothing` must go RED. The shadow is inferred,
the stxr is emulated, and status 0 then diverges from the recorded 1 at landmark 6.

- [ ] **Step 5: The stepping suites still pass, then clippy and commit**

Run the Task 3 Step 8b commands again, into `t5-*.log`. `hitorder_e2e` must be green: its hits
module now has the bounded `debug` and the new doc, and no behaviour change. Then:

```bash
cargo clippy --workspace --all-targets -- -D warnings
git add crates/retrace-box/src/excl.rs crates/retrace-box/src/lib.rs crates/retrace/tests/llsc_e2e.rs crates/retrace/tests/util/hits.rs
git commit -m "M42 t5: infer the exclusive shadow at a native breakpoint or watch stop

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: The dynamic path, on `threadrust`

**Files:**
- Modify: `crates/retrace/tests/hitorder_e2e.rs`: a Q3 test, and possibly `tr_oracle_from`.

**Interfaces:**
- Consumes: `ReplaySession::dbg_excl` (Task 3), `retrace_core::seek`, `tr_trace()` (existing in
  the file).

- [ ] **Step 1: Write the Q3 test**

```rust
/// M42 (spec §4, "the dynamic path"): M41's Q3, flipped.
///
/// dyld's `_getpid` fills its pid cache right after the first `getpid` (20) syscall, at
/// (g+1, 1)..(g+1, 3): `ldxr w10, [x9]; cbnz w10, …; stxr wzr, w0, [x9]`. A seek that stepped the
/// `ldxr` used to fail the `stxr`, and the next image's `getpid()` then issued a syscall the
/// recording does not hold (M41 t1: a divergence six landmarks later).
#[test]
fn a_seek_into_dylds_getpid_pair_replays_to_the_end() {
    let tp = tr_trace();
    let g = {
        let mut s = ReplaySession::open(tp).unwrap();
        loop {
            if let Some((20, _)) = s.peek_syscall() { break s.landmark(); }
            s.advance().unwrap();
        }
    };
    {
        let s = retrace_core::seek(tp, g + 1, 1).unwrap();
        let w = s.read_mem(s.pc(), 4).expect("the pc maps");
        assert_eq!(u32::from_le_bytes(w.try_into().unwrap()), 0x885f_7d2a,
            "(g+1, 1) must be dyld's getpid `ldxr w10, [x9]` (M41 t1); if the OS moved it, re-measure");
    }
    let want = retrace_core::replay(tp).expect("plain replay").outcome;
    for j in [2u64, 3, 4] {
        let mut s = retrace_core::seek(tp, g + 1, j).unwrap();
        assert_eq!(s.dbg_excl().is_some(), j < 4,
            "(g+1, {j}): the stepped ldxr sets the shadow, and the emulated stxr clears it");
        loop {
            let adv = s.advance().unwrap_or_else(|d| panic!("(g+1, {j}): diverged at landmark {}: {}", d.landmark, d.detail));
            if let Advance::Exited(r) = adv {
                assert_eq!(r.outcome, want, "(g+1, {j})");
                break;
            }
        }
    }
}
```

Run: `cargo test -p retrace --test hitorder_e2e a_seek_into_dylds_getpid -- --test-threads=1`
Expected: PASS on the Task 5 tree. After Step 3's commit, show it RED on `1d95a93`'s behaviour with a
temporary revert of `step()`'s emulation hunk, undone with `git checkout -- <file>` (`git stash` is shared across worktrees and forbidden, so edit and revert
by hand). The RED is a divergence at `(g+1, 2)`. Ledger both.

If the word assertion fails, the OS moved dyld's getpid. Report the word found, then re-derive the
pair's K by reading the words at `(g+1, 0..6)`, and pin those. Do not delete the assertion.

- [ ] **Step 2: Measure the oracle from landmark 1** (spec R7)

Temporarily change `tr_oracle_from`'s body to `1`. Then run:

```bash
cargo test -p retrace --test hitorder_e2e --no-run
/usr/bin/time -l cargo test -p retrace --test hitorder_e2e oracle_threadrust -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m42-llsc/t6-oracle-from-1.log 2>&1; echo "exit=$?"
grep -a -e 'user' -e 'sys' -e 'test result' .superpowers/sdd/2026-09-24-retrace-m42-llsc/t6-oracle-from-1.log
```

- **Green, with user + sys ≤ 120 s:** keep `1`. Rewrite `tr_oracle_from`'s doc: M42 made stepping
  through dyld's `getpid` pairs exact, so the list from landmark 1 is the whole list and it steps
  all three pairs (M41's R13 is reverted). Name the measured CPU in the doc.
- **Otherwise:** revert to `t.n_create`. Append to the doc: "M42 measured the start at 1: `<result>`,
  `<cpu>` s CPU, over M41's 120 s budget; the getpid pairs are guarded by
  `a_seek_into_dylds_getpid_pair_replays_to_the_end` instead."
- **If it is red for any reason other than cost**, that is a finding outside M42's fixtures. Stop and
  report it with the log.

- [ ] **Step 3: Commit**

```bash
cargo clippy -p retrace --all-targets -- -D warnings
git add crates/retrace/tests/hitorder_e2e.rs
git commit -m "M42 t6: dyld's getpid pair survives a seek into it on threadrust (M41 Q3 flipped)

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Close

**Files:**
- Modify: `README.md` (What works today, Known limits, the gate line)
- Modify: `docs/status-log.md` (append an M42 section)
- Modify: `CLAUDE.md` (the e2e gate list, symmetry rule 2's examples)
- Modify: `docs/superpowers/specs/2026-09-24-retrace-m42-llsc-design.md` §10

- [ ] **Step 1: Predict the gate from source, file by file**

Count the `#[test]` functions added by this branch in each file:

```bash
git diff 1d95a93 --stat
grep -c '#\[test\]' crates/retrace/tests/llsc_e2e.rs
```

Tabulate the M41 close (**666 / 0 / 9 over 141**) plus each file's delta, in these rows:
- `retrace-arch` lib;
- `retrace-box` lib (`excl.rs`);
- `retrace-box` `step`;
- `retrace-box` `checkpointparity`;
- `llsc_e2e`, a new binary;
- `hitorder_e2e`.

Write the predicted total before running anything.

- [ ] **Step 2: Run the chunked gate** (CLAUDE.md: never one command, each chunk `--no-fail-fast`)

```bash
cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m42-llsc/gate-ws.log 2>&1; echo "exit=$?"
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m42-llsc/gate-box.log 2>&1; echo "exit=$?"
cargo test -p retrace --bins -- --test-threads=1 > .superpowers/sdd/2026-09-24-retrace-m42-llsc/gate-bins.log 2>&1; echo "exit=$?"
```

Then run one `cargo test -p retrace --test <name> -- --test-threads=1` per integration-test target
in `crates/retrace/tests/` (list them with `ls crates/retrace/tests/*.rs`), each into
`gate-e2e-<name>.log`, recording each `exit=`. Then:

```bash
cargo clippy --workspace --all-targets -- -D warnings > .superpowers/sdd/2026-09-24-retrace-m42-llsc/gate-clippy.log 2>&1; echo "exit=$?"
```

Sum passed, failed, ignored and binaries from the `test result` lines (`grep -a`). Reconcile against
Step 1 file by file. Any difference is explained in the report or it is a defect. Nothing may be
failed, and ignored must be 9.

- [ ] **Step 3: CPU, before and after**

Repeat Task 2 Step 1's two measurements into `t7-cpu-*-after.log`. `cpython_crash_e2e` must be
within 10 % of its before-figure, or the report proposes a Ruling. `hitorder_e2e` is reported with
its difference, and Task 6 explains any change in its oracle's start.

- [ ] **Step 4: The audit.** List every existing test file whose assertions touch stepping or
`continue`/`reverse-continue`, and confirm from the gate logs that each passed unchanged. The files
are `debug_cli`, `watch_cli`, `watch`, `watch_dyn`, `watchsweep_e2e`, `thread_watch_e2e`,
`hitorder_e2e`, `crashy_cli`, `crashy_e2e`, `reverse_debug_e2e`, `checkpoint_seek`,
`cpython_crash_e2e` and `sigcatch_dyn_e2e`, plus `debug.rs`'s unit tests in the `--bins` chunk. Write
the list to `.superpowers/sdd/2026-09-24-retrace-m42-llsc/audit.md`.

- [ ] **Step 5: Docs.**

**README**, edited in place:

- **Known limits:**
  - Replace the bullet "A debugger stop between a load-exclusive and its store-exclusive makes the
    store fail…" with the residuals of spec §7 and §3d/§3e:
    - the asynchronous host-interrupt ERET between the halves, rare and loud;
    - inference's four assumptions: fall-through, no branch into the pair, base not rewritten, no
      identical-value plain store;
    - a pair straddling a page, or a load more than 16 instructions before the stop, gets no
      inference;
    - the 16-step `run()` bound's drop (R10);
    - `wfe`, unhandled everywhere;
    - byte, halfword and `ldaxp` retires, whose ISS.EX is not measured.
  - Keep each one short, and say which are loud.
  - The M41 hit-oracle sentence ("safe from it by where it starts") goes, with Task 6's outcome in
    its place.
- **What works today:** a short paragraph.
  - Stepping, seeking, `reverse-stepi`, checkpoints and native breakpoint/watch stops keep an
    exclusive pair's store as the recording ran it, through a shadow monitor below the trace.
  - Record and plain replay never engage it (asserted).
  - `llsc_e2e` is the guard.
- **The gate line:** the new figures.

**`docs/status-log.md`**: append an "M42-llsc" section in the house style of M41's section:
- what was measured (t0 M1–M8);
- what landed, per task, with commit hashes;
- the gate, reconciled;
- the controls C1–C6 and the order swap, each shown RED;
- rulings R9–R11 and any execution rulings;
- the CPU figures;
- the owed list: the spec §7 "Not done" items, plus anything the reviews parked.

**CLAUDE.md:**
- In "The headline end-to-end gates" list, after `watchsweep_e2e`, add:

  > `llsc_e2e` (M42: exclusive (LL/SC) pairs under stepping, seeking and native breakpoint/watch
  > stops — the store-exclusive must land exactly as recorded; t0 measured hangs, phantom watch
  > hits and a silent `no earlier hit` before the shadow monitor, and every CLI run is bounded so a
  > regression fails rather than stalls)

- In symmetry rule 2's parenthesis ("as with the timebase MRS, the Apple-IMPDEF undef-MRS, and the
  B-family FPAC strip"), add "and the M42 store-exclusive emulation under a shadow monitor".

**Spec §10:** fill in the outcome: the gate, what went green where, each Ruling, and each prediction
that was wrong and by how much.

- [ ] **Step 6: Commit**

```bash
git add README.md docs/status-log.md CLAUDE.md docs/superpowers/specs/2026-09-24-retrace-m42-llsc-design.md
git commit -m "M42 close: the gate (<figures>), the audit, docs

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```
