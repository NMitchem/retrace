# M43-lldb Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** `retrace gdbserver <trace>`: lldb connects over gdb-remote and debugs a recording forward
and backward, down to "reverse-step through a real crash in LLDB".

**Architecture:** a gdb-remote (RSP) server in the `retrace` binary, over the existing scripted
debugger's `Exec` (`crates/retrace/src/debug.rs`). Every motion is one of `Exec`'s. The server maps
`Exec`'s cursor to the positions lldb believes it stands on (spec §3c), steps the way lldb needs
(§3d), and never answers a resume packet with an error (§3b). Task 1 first hardens three `Box_`
panics that lldb's stepping would reach (§3i).

**Tech Stack:** Rust 1.95 (pinned), Hypervisor.framework via `hv-sys`, `std::net` TCP, lldb-2100
(Xcode) for the end-to-end tests, a 20-line lldb Python command.

**Spec:** `docs/superpowers/specs/2026-09-25-retrace-m43-lldb-design.md`. Its companion,
`2026-09-25-retrace-m43-lldb-measurements.md`, holds t0's measurements (L1–L10, R1–R4). Read the
spec section a task names before starting it. The spec binds; this plan argues from it.

## Global Constraints

- Toolchain `1.95.0`, target `aarch64-apple-darwin`. The gate is `cargo test` for every chunk (with
  `--test-threads=1`) plus `cargo clippy --workspace --all-targets -- -D warnings`.
- **`clippy -D warnings` also rejects dead code**: an unused function, a never-constructed variant, a
  variant field that is never read (measured on rustc 1.95). Each task introduces only what its own
  non-test code uses. Tasks 2–4 say which `StopKind` and `Halt` variants each adds. Do not add a
  later task's variant early.
- `clippy.toml` bans `Instant::now`, `SystemTime::now` and `std::thread::Thread`. The server is
  single-threaded, with no threads and no async. Tests may `std::thread::sleep` in a polling loop,
  as `util::debug_bounded` does.
- `--test-threads=1` on every `cargo test` (one VM per process). The server is a separate process,
  so a test process may hold its own VM while a spawned server runs.
- `TRACE_MAGIC` does not move, and `crates/retrace-trace` has no diff.
- Every transcript of `retrace debug --script` stays byte-identical. No existing debug test changes
  an assertion.
- Every test that spawns the CLI uses `util::bin()` (the codesigned copy).
- The lldb tests run `lldb -x -b -s <file> </dev/null`, never `-o`. `-o` silently stops after a
  crash or boundary stop and still exits 0 (t0 L10). They end with `script print("END")` and
  assert on it. They skip with a loud `eprintln!` when `lldb --version` does not exit 0.
- **Worktree shell rules:**
  - no `VAR=val cmd` prefix; `export VAR=val` on its own line first;
  - no `git -C`;
  - put `echo "exit=$?"` in the **same** command as the cargo invocation it checks, **before**
    any pipe;
  - `--no-fail-fast` goes before `--`;
  - never `git stash`.
- **Controls:**
  - run on the **committed** tree;
  - restore with `git checkout -- <file>`;
  - record each control's actual symptom in the task report.

  A control that stays green is a finding, reported, never papered over.
- **Logs:** write every log to `L=/Users/noahmitchem/Documents/GitHub/retrace/.superpowers/sdd/2026-09-25-retrace-m43-lldb/`
  (`export L=…` on its own line), named `t<N>-<what>.log`.
- **Style:** match the surrounding code's comment density and idiom. Comments cite the spec as
  `M43 §3x`, and t0's measurements as `(t0 L4c)`. Test names are sentences
  (`a_forward_watch_is_reported_after_the_store`).
- End each commit message with the `Co-Authored-By:` trailer of the model that wrote it.

## Review Focus

1. **One TCP read holding two packets, or half of one:** lldb pipelines `+` acks and packets. The
   decoder must frame across reads. Task 2's `rsp.rs` unit tests pin a split packet and a
   two-packet read.
2. **An address lldb probes that no guest could map** (`mfffffff000002010,8`, t0 L1) must be an
   `E08`, not an overflow panic in the page walk. Task 2 pins it on the wire.
3. **The peer vanishing without `k` or `D`** (lldb killed, test harness dropped): the server must
   exit 0 on EOF, not spin. Task 2 pins it.
4. **A register read for a thread that does not exist or has exited** must be `E01`, not a panic.
   Task 2 pins it (`p0;thread:9;`).
5. **Re-inserting an existing breakpoint or watchpoint, or removing an absent one:** lldb re-sends
   `Z0` after stepping off a breakpoint (`z0 / s / Z0`). It must be `OK` and idempotent, and must
   not count twice against the cap. Task 3 pins it.

---

### Task 1: The M42 hardening (spec §3i)

`Box_::step()` has three panics lldb's stepping would reach: a base-aliasing load-exclusive, a
retire whose syndrome has ISV = 0, and a store-exclusive to a non-writable target. Fix all three,
and the stale comment F-5 named.

**Files:**
- Create: `crates/retrace-guest/asm/llscedge.s`
- Modify: `crates/retrace-guest/build.rs` (a build block after `llscbound`'s)
- Modify: `crates/retrace-guest/src/lib.rs` (`pub const LLSC_EDGE`, after `LLSC_BOUND`)
- Modify: `crates/retrace-box/src/excl.rs` (`classify_retire` plus unit tests)
- Modify: `crates/retrace-box/src/lib.rs`:
  - `step()`: the pre-decode;
  - `run_one_for_step`: takes `pre`;
  - `set_excl_from_retire`: takes `(ld, va)`;
  - `emulate_stx`: the non-writable branch;
  - `run()`'s prologue comment (F-5);
  - the `SS_ISV_EX` constant: delete it, because nothing uses it after the change.
- Test: `crates/retrace/tests/llsc_e2e.rs` (three tests, plus the module doc)

**Interfaces:**
- Produces: `pub fn excl::classify_retire(syndrome: u64, pre: Option<(ExclInsn, u64)>) -> Result<Option<(ExclInsn, u64)>, String>`;
  `pub const retrace_guest::LLSC_EDGE: &str`.

- [ ] **Step 1: The fixture.** This exact file was assembled and recorded at plan time (spec §3i,
  "Measured at plan time"). It records to `guest crashed: pc=0x1000003a0 far=0x1000003b0
  esr=0x9200004f`, exit 139, and reproduces both M42 panics unfixed. Create
  `crates/retrace-guest/asm/llscedge.s`:

```asm
// M43 §3i (M42 final review, Minors 2-4): the step-path shapes M42 left panicking. A separate
// fixture, so llsc.s and llscbound.s and their coordinates stay frozen.
//
// Window 1: a load-exclusive whose base is also its destination, `ldxr x9, [x9]`, with no
// store-exclusive after it. Stepping it panicked at the retire: the base was read AFTER the load
// overwrote it. getpid ends the window.
//
// Window 2: a load-exclusive then a store-exclusive on a word in __TEXT, which EL0 can read and
// cannot write. Natively the monitor is held, so the store takes a permission fault: the recording
// ENDS here, in a crash (exit 139). Stepped, the shadow is set and the emulation panicked on the
// non-writable target.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    adrp x9, celledge@PAGE
    add  x9, x9, celledge@PAGEOFF
edge_alias:
    ldxr x9, [x9]                   // (1, 2): base == destination; x9 becomes 0x5a5a
    mov  x16, #20                   // SYS_getpid: ends window 1 at (1, 4)
    svc  #0x80
    adrp x12, edge_ro@PAGE
    add  x12, x12, edge_ro@PAGEOFF
edge_ro_ldx:
    ldxr w10, [x12]                 // (2, 2): reads the read-only word
edge_ro_stx:
    stxr w13, w10, [x12]            // (2, 3): faults natively (permission); the recording's terminal
    mov  x0, #0
    mov  x16, #1                    // SYS_exit(0): never reached in the recording
    svc  #0x80
.p2align 2
edge_ro: .word 7                    // in __TEXT: EL0 read-only (ATTR_CODE)

.section __DATA,__data
.p2align 3
celledge: .quad 0x5a5a
```

In `crates/retrace-guest/build.rs`, directly after the `llscbound` block, add the same kind of block:

```rust
    // llscedge (M43 §3i): the three step-path shapes M42's final review left panicking — a
    // base-aliasing load-exclusive, and a pair on a read-only word whose recording ends in the fault.
    let src = format!("{}/asm/llscedge.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/llscedge");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang llscedge");
    assert!(status.success(), "llscedge guest build failed");
```

In `crates/retrace-guest/src/lib.rs`, after `LLSC_BOUND`:

```rust
/// M43 §3i: a base-aliasing load-exclusive (window 1), then a pair on a read-only `__TEXT` word
/// whose recording ends in the store's permission fault (window 2). Behaviour is `llsc_e2e`'s.
pub const LLSC_EDGE: &str = concat!(env!("OUT_DIR"), "/llscedge");
```

- [ ] **Step 2: The failing tests.** In `crates/retrace/tests/llsc_e2e.rs`:
  - extend the module doc's fixture sentence to name `llscedge.s` (M43 §3i);
  - add, next to `trace()`/`sym()`:

```rust
/// M43 §3i: `llscedge`, recorded once. Its recording ends in the crash at `edge_ro_stx`.
fn edge_trace() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    C.get_or_init(|| {
        let (rec, t) = util::record(retrace_guest::LLSC_EDGE);
        assert_eq!(rec.code, 139, "llscedge must record to the read-only store's fault: {}", rec.stderr);
        t
    })
}
fn edge_sym(name: &str) -> u64 {
    static M: OnceLock<HashMap<String, u64>> = OnceLock::new();
    *M.get_or_init(|| nm(retrace_guest::LLSC_EDGE)).get(name)
        .unwrap_or_else(|| panic!("no symbol {name} in llscedge"))
}
/// Replay `s` forward to its end, panicking on a divergence.
fn finish(mut s: ReplaySession) -> retrace_core::ReplayReport {
    loop {
        match s.advance() {
            Ok(Advance::Exited(r)) => return r,
            Ok(_) => {}
            Err(d) => panic!("diverged at landmark {}: {}", d.landmark, d.detail),
        }
    }
}

#[test]
fn the_edge_fixture_records_and_replays_its_crash_natively() {
    // Native record and replay take no step exit, so none of §3i's code runs here: this pins the
    // fixture's own shape (window 2 ends in the store's fault) before the stepped tests lean on it.
    let r = util::replay(edge_trace());
    assert_eq!(r.code, 139, "replay: {}", r.stderr);
    let end = finish(ReplaySession::open(edge_trace()).unwrap());
    assert!(matches!(end.outcome, Outcome::Crash { pc, .. } if pc == edge_sym("edge_ro_stx")),
        "{:?}", end.outcome);
}

#[test]
fn a_load_exclusive_whose_base_is_its_destination_steps_without_panicking() {
    // M43 §3i item 1: (1, 3) is just past `ldxr x9, [x9]`. The shadow must hold the cell's address
    // as it was BEFORE the load overwrote x9. Before the fix, the retire panicked ("its base is also
    // a destination").
    let s = retrace_core::seek(edge_trace(), 1, 3).expect("seek past the aliasing load");
    let ex = s.dbg_excl().expect("the stepped load-exclusive sets the shadow");
    assert_eq!(ex.va, edge_sym("celledge"), "the marked address is the base before the load");
    assert_eq!(ex.loaded, 0x5a5au64.to_le_bytes().to_vec());
    assert_eq!(ex.by, SetBy::Stepped);
    let end = finish(s);
    assert!(matches!(end.outcome, Outcome::Crash { pc, .. } if pc == edge_sym("edge_ro_stx")),
        "{:?}", end.outcome);
}

#[test]
fn a_stepped_store_exclusive_to_a_read_only_word_ends_in_the_recorded_crash() {
    // M43 §3i item 3: (2, 3) is just before `stxr`, with the shadow set by the stepped `ldxr`. The
    // target is in __TEXT, so the emulation must not run: the store runs natively, and the core
    // faults as it did in the recording (outcome (a), measured at plan time). Replay compares the
    // crash field by field, so reaching Exited(Crash) at the store IS the proof. `advance()` gets
    // there through run()'s pair prologue, which steps with the shadow set. Before the fix,
    // emulate_stx panicked ("EL0-writable").
    let s = retrace_core::seek(edge_trace(), 2, 3).expect("seek to the read-only store");
    assert!(s.dbg_excl().is_some(), "the shadow is set before the store");
    let end = finish(s);
    assert!(matches!(end.outcome, Outcome::Crash { pc, .. } if pc == edge_sym("edge_ro_stx")),
        "{:?}", end.outcome);
}
```

`ReplaySession` and `SetBy` are already imported at the top of `llsc_e2e.rs`. Add `ReplaySession`
to the `use retrace_core::{…}` list if it is not there.

- [ ] **Step 3: See them fail.**

```sh
export L=/Users/noahmitchem/Documents/GitHub/retrace/.superpowers/sdd/2026-09-25-retrace-m43-lldb
cargo test -p retrace --test llsc_e2e --no-fail-fast -- --test-threads=1 edge base_is_its read_only_word > $L/t1-red.log 2>&1; echo "exit=$?"
grep -a -E "^test |panicked|EL0-writable|also a destination" $L/t1-red.log
```

Expected: `the_edge_fixture_records_and_replays_its_crash_natively` passes. Both stepped tests fail
with M42's panics:
- `unmodelled load-exclusive … its base is also a destination` (seek to (1, 3));
- `unmodelled store-exclusive … EL0-writable` (the advance from (2, 3)).

If the fixture test fails instead, the fixture is wrong: fix it before going on.

- [ ] **Step 4: `classify_retire`.** In `crates/retrace-box/src/excl.rs`, after `plan_stx`:

```rust
/// M43 §3i: what a clean step retire means for the shadow.
///
/// `pre` is the instruction at pc BEFORE the step, when it decoded as a load-exclusive, with its
/// base VA read before the step too, because a load may overwrite its own base (`ldxr x9, [x9]`).
/// `syndrome` is the step exit's ESR.
/// - ISS.ISV = 1: ISS.EX says whether a load-exclusive retired (M42 t0 M8). It must agree with the
///   decode in both directions, and a disagreement is an Err naming both.
/// - ISS.ISV = 0: the syndrome does not say, so the decode decides. That is unmeasured on this core
///   (M42 t0 saw ISV = 1 on every retire); it is the hardening M42's final review asked for.
pub fn classify_retire(syndrome: u64, pre: Option<(ExclInsn, u64)>)
                       -> Result<Option<(ExclInsn, u64)>, String> {
    const ISV: u64 = 1 << 24;
    const EX: u64 = 1 << 6;
    if syndrome & ISV == 0 { return Ok(pre); }
    match (syndrome & EX != 0, pre) {
        (true, Some(p)) => Ok(Some(p)),
        (false, None) => Ok(None),
        (true, None) => Err(format!(
            "the step exit reported a load-exclusive (ISS.EX, ESR {syndrome:#x}), but the instruction \
             stepped does not decode as one: the hardware and retrace_arch::decode_excl disagree")),
        (false, Some((ld, _))) => Err(format!(
            "the instruction stepped decodes as a load-exclusive ({ld:?}), but the step exit's ISS.EX \
             is 0 (ESR {syndrome:#x}): the hardware and retrace_arch::decode_excl disagree")),
    }
}
```

Its unit tests go in `excl.rs`'s `mod tests`. The two syndromes are M42 t0 M8's measured retire
ESRs; clearing bit 24 gives an ISV = 0 one:

```rust
    #[test] fn a_retire_is_classified_by_iss_ex_when_isv_is_set_and_by_the_decode_otherwise() {
        let ld = (ExclInsn::Load { size: 8, pair: false, rt: 9, rt2: 31, rn: 9 }, 0x1_0000_4000u64);
        let (ldx_retire, plain_retire) = (0xcb00_0062u64, 0xcb00_0022u64); // M42 t0 M8
        let no_isv = plain_retire & !(1 << 24);
        assert_eq!(classify_retire(ldx_retire, Some(ld)), Ok(Some(ld)));
        assert_eq!(classify_retire(plain_retire, None), Ok(None));
        assert_eq!(classify_retire(no_isv, Some(ld)), Ok(Some(ld)), "ISV = 0: the decode decides");
        assert_eq!(classify_retire(no_isv, None), Ok(None));
    }
    #[test] fn a_retire_whose_syndrome_and_decode_disagree_is_an_error_both_ways() {
        let ld = (ExclInsn::Load { size: 4, pair: false, rt: 10, rt2: 31, rn: 12 }, 0x1_0000_0400u64);
        assert!(classify_retire(0xcb00_0062, None).unwrap_err().contains("does not decode as one"));
        assert!(classify_retire(0xcb00_0022, Some(ld)).unwrap_err().contains("ISS.EX is 0"));
    }
```

- [ ] **Step 5: The pre-decode in `Box_`.** In `crates/retrace-box/src/lib.rs`:

1. In `step()`, after the M42 `if self.excl.is_some() { … }` block and **before** the SS arming
   (`let mdscr = …`):

```rust
        // M43 §3i: decode the instruction before it runs. A load-exclusive's base is read now,
        // because the load may overwrite it (`ldxr x9, [x9]`), and the decode stands in for ISS.EX
        // when the step exit does not carry it (ISV = 0). Read after the M42 block above, which
        // returns early for an emulated store, so `pre` is only ever a load.
        let pre = self.insn_at(self.pc()).and_then(decode_excl).and_then(|i| match i {
            ExclInsn::Load { rn, .. } => Some((i, self.base_reg(rn) & excl::TAG_MASK)),
            _ => None,
        });
```

   Then change `let stop = self.run_one_for_step();` to `let stop = self.run_one_for_step(pre);`.

2. Change `fn run_one_for_step(&mut self) -> Stop` to
   `fn run_one_for_step(&mut self, pre: Option<(ExclInsn, u64)>) -> Stop`. In its clean-retire
   branch, replace
   `if e.syndrome & SS_ISV_EX == SS_ISV_EX { self.set_excl_from_retire(); }` with:

```rust
                        // M43 §3i: the syndrome, or where it is silent the pre-step decode, says
                        // whether a load-exclusive retired (M42 §3a, t0 M8).
                        match excl::classify_retire(e.syndrome, pre) {
                            Ok(Some((ld, va))) => self.set_excl_from_retire(ld, va),
                            Ok(None) => {}
                            Err(why) => panic!("M42: at {:#x}: {why}", self.pc() - 4),
                        }
```

   `grep -n "run_one_for_step(" crates/retrace-box/src/lib.rs` must show `step()` as its only
   caller. If there is another, stop and report it: that caller's pre-step decode is a design
   question, not a guess.

3. Replace `set_excl_from_retire`'s body and signature:

```rust
    /// M42 §3a, M43 §3i: set the shadow for a load-exclusive that just retired under `step()`. `va`
    /// is its base as read BEFORE the step (tag-stripped), so a load that overwrote its own base
    /// (`ldxr x9, [x9]`) still marks the address it read.
    fn set_excl_from_retire(&mut self, ld: ExclInsn, va: u64) {
        let ExclInsn::Load { size, pair, .. } = ld else {
            unreachable!("classify_retire only returns loads: {ld:?}")
        };
        let len = excl::access_len(size, pair);
        let loaded = self.va_to_ipa(va).and_then(|ipa| self.read_guest_checked(ipa, len))
            .unwrap_or_else(|| panic!("M42: the load-exclusive read {va:#x}, which does not map"));
        self.excl = Some(Excl { va, size, pair, loaded, by: SetBy::Stepped });
    }
```

   The `base_aliases_dest` assert goes away with the old body. Keep the `excl::base_aliases_dest`
   function itself: §3d's inference uses it (condition 2). `SS_ISV_EX` (lib.rs ~line 297) now has
   no use: delete it and its three-line `/// M42 (t0 M8)` doc comment. Its measured values live on
   in `classify_retire`'s doc and unit test.

4. In `emulate_stx`, replace the `.unwrap_or_else(|why| panic!(…))` on `plan_stx` with:

```rust
        let plan = match excl::plan_stx(&ex, st, base, self.xreg(rt), self.xreg(rt2), target.as_deref(), writable) {
            Ok(plan) => plan,
            // M43 §3i item 3: natively, with the monitor held, a store to a target EL0 cannot write
            // faults, and the recording holds that crash. Emulating it would invent a write. Drop the
            // shadow and step the store natively, so the core raises the fault as record's did.
            Err(_) if !writable || target.is_none() => {
                self.excl = None;
                return self.step();
            }
            Err(why) => panic!("M42: unmodelled store-exclusive at pc {pc:#x}: {why} (shadow {ex:?})"),
        };
```

- [ ] **Step 6: F-5.** In `run()`'s M42 §3e prologue comment, replace the bullet that begins
  `- Bounded at PAIR_STEP_BOUND (plan R10). A shadow that outlives it belongs to a load whose`
  (through `natively does to the hardware monitor anyway.`) with:

```rust
        // - Bounded at PAIR_STEP_BOUND (plan R10). A shadow can outlive it in two ways: a branch left
        //   its sequence (fixture shape (h), `llscbound.s`), or a straight-line store-exclusive sits
        //   more than 16 instructions past this entry (M42 final review F2). It is dropped, which is
        //   what resuming natively does to the hardware monitor anyway. That is loud only for a
        //   discard-status pair. A retry loop retries the lost store, and any other shape depends on
        //   its caller (README, Known limits).
```

- [ ] **Step 7: Green.**

```sh
cargo test -p retrace --test llsc_e2e --no-fail-fast -- --test-threads=1 > $L/t1-llsc.log 2>&1; echo "exit=$?"
cargo test -p retrace-box --lib --no-fail-fast -- --test-threads=1 excl > $L/t1-excl.log 2>&1; echo "exit=$?"
grep -a -E "^test result|FAILED|panicked" $L/t1-llsc.log $L/t1-excl.log
```

Expected: `llsc_e2e` 50 passed (47 + 3), `excl` units all pass.

Spec §3i's outcome was measured at plan time: it is (a). The core raises the permission fault for a
store-exclusive whose monitor is lost, so the crash assertion holds. If that test instead fails
with a **divergence**, the measurement did not reproduce. Report BLOCKED with the divergence text,
and do not rewrite the test.

- [ ] **Step 8: No regressions in the stepping suites.**

```sh
cargo test -p retrace-box --test step --test checkpointparity --no-fail-fast -- --test-threads=1 > $L/t1-box-step.log 2>&1; echo "exit=$?"
cargo test -p retrace --test hitorder_e2e --test reverse_debug_e2e --test debug_cli --no-fail-fast -- --test-threads=1 > $L/t1-debug.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets -- -D warnings > $L/t1-clippy.log 2>&1; echo "exit=$?"
```

Expected: all `exit=0`.

- [ ] **Step 9: Commit.**

```sh
git add crates/retrace-guest/asm/llscedge.s crates/retrace-guest/build.rs crates/retrace-guest/src/lib.rs crates/retrace-box/src/excl.rs crates/retrace-box/src/lib.rs crates/retrace/tests/llsc_e2e.rs
git commit -m "M43 t1: the step path decodes before it runs; a read-only store-exclusive steps natively (§3i)"
```

- [ ] **Step 10: Control C6 (on the committed tree).** In `set_excl_from_retire`, make the marked
  address the base as read **after** the load again. Replace `va` in the `va_to_ipa(va)` call and
  in the `Excl { va, … }` with
  `self.base_reg(match ld { ExclInsn::Load { rn, .. } => rn, _ => unreachable!() }) & excl::TAG_MASK`.

```sh
cargo test -p retrace --test llsc_e2e --no-fail-fast -- --test-threads=1 base_is_its > $L/t1-c6.log 2>&1; echo "exit=$?"
git checkout -- crates/retrace-box/src/lib.rs
```

Expected: RED. The load-exclusive "read 0x5a5a, which does not map" panics during the seek.
Record the symptom.

---

### Task 2: The wire: everything that does not move (spec §3a, §3g, §3h)

The CLI subcommand, the TCP loop, the pure protocol module, and every packet that does not resume
the guest. By the end, a client can connect, read registers, memory and threads, list the exe, and
disconnect. No motion yet: resume packets get the empty reply.

**Files:**
- Create: `crates/retrace/src/rsp.rs`
- Create: `crates/retrace/src/gdbserver.rs`
- Modify: `crates/retrace/src/main.rs` (`mod rsp; mod gdbserver;`, the `gdbserver` arm, usage text)
- Modify: `crates/retrace/src/debug.rs`:
  - `Exec`, `Exec::new` and `Exec::sess` become `pub(crate)`;
  - `Phase` becomes `pub(crate)`;
  - new `pub(crate) fn cursor(&self) -> (usize, u64, Phase)`.
- Modify: `crates/retrace-box/src/lib.rs` (`pub fn thread_ctx`, `pub fn read_va_prefix`)
- Modify: `crates/retrace-core/src/lib.rs`:
  - `ReplaySession::thread_ctx`;
  - `ReplaySession::read_mem_prefix`;
  - re-export `ThreadCtx` and `EXE_BASE`.
- Create: `crates/retrace/tests/util/rsp.rs`. Modify: `crates/retrace/tests/util/mod.rs` (`pub mod rsp;`)
- Create: `crates/retrace/tests/gdbserver_e2e.rs`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces, for Tasks 3–5:
  - **retrace-core (library):**
    - `ReplaySession::thread_ctx(&self, tid: usize) -> Option<ThreadCtx>`;
    - `ReplaySession::read_mem_prefix(&self, va: u64, len: usize) -> Vec<u8>`;
    - `retrace_core::{ThreadCtx, EXE_BASE}`.
  - **`crate::rsp`:**
    - `Decoder` / `Frame`, `encode`, `hex`, `unhex`;
    - `stop_reply(tid: u32, ctx: &ThreadCtx, threads: &[(u32, u64)], kind: &StopKind) -> String`;
    - `StopKind { HistoryBegin(String), None }`. Task 3 adds more variants; each has a
      `signal()` and a `keys()`.
  - **`crate::gdbserver::Server`:**
    - `handle(&mut self, p: &str) -> (Vec<String>, bool)`;
    - a `stop(&self, kind: StopKind, thread: Option<u32>) -> String` helper that fills
      threads and registers from the session.
  - **CLI:** `retrace gdbserver <trace> [--port <n>] [--exe <path>]`, which prints
    `listening on 127.0.0.1:<port>` to stderr.
  - **`tests/util/rsp.rs`:** `spawn_server(trace, extra) -> (Child, u16, PathBuf)` (also used
    by Task 5's lldb tests), `Rsp::spawn(trace, extra_args)`, `send`, `send_collect`, `send_raw`,
    `where_`, `kill`, `detach`, `drop_connection`, and the free helpers `description`, `key`,
    `le_u64`.

- [ ] **Step 1: Session accessors.** In `crates/retrace-box/src/lib.rs`, next to `dbg_regs_of`:

```rust
    /// M43 §3g: thread `tid`'s full register context, FP included. The current thread's comes off
    /// the live vCPU, because the table's slot is stale between switches (`dbg_regs_of`'s own split).
    /// Any other thread's is the saved one. None for an id past the table.
    pub fn thread_ctx(&self, tid: usize) -> Option<thread::ThreadCtx> {
        if tid >= self.threads.len() { return None; }
        if tid == self.threads.current() { return Some(self.save_ctx()); }
        Some(self.threads.ctx_of(tid).clone())
    }

    /// M43 §3g: the readable prefix of `[va, va + len)`. Each 16 KiB page goes through the guest's
    /// own stage-1 walk, as `insn_at` reads. It stops at the first byte that does not translate or
    /// read, so lldb's 0x200-byte reads that straddle a mapping's end get the mapped part (t0 L8).
    /// Never panics: an address past 47 bits translates to None, and the arithmetic saturates.
    pub fn read_va_prefix(&self, va: u64, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len.min(1 << 16));
        let mut a = va;
        while out.len() < len {
            let Some(ipa) = self.va_to_ipa(a) else { break };
            let page_end = (a | (GRANULE as u64 - 1)).saturating_add(1);
            let n = ((page_end - a) as usize).min(len - out.len());
            if ipa.checked_add(n as u64).is_none() { break; }
            match self.read_guest_checked(ipa, n) {
                Some(b) => out.extend_from_slice(&b),
                None => break,
            }
            a = a.saturating_add(n as u64);
            if a == u64::MAX { break; }
        }
        out
    }
```

In `crates/retrace-core/src/lib.rs`:
- extend the existing re-export to `pub use retrace_box::thread::{BlockReason, ThreadState, ThreadCtx};`;
- add `pub use retrace_box::EXE_BASE;`;
- add to `impl ReplaySession`, next to `dbg_regs_of`:

```rust
    /// M43 §3g: a thread's full register context (`Box_::thread_ctx`), for the gdb-remote server.
    pub fn thread_ctx(&self, tid: usize) -> Option<ThreadCtx> { self.b.thread_ctx(tid) }
    /// M43 §3g: the readable prefix of `[va, va + len)`, by guest VA (`Box_::read_va_prefix`).
    /// `read_mem` stays the debugger CLI's all-or-nothing read.
    pub fn read_mem_prefix(&self, va: u64, len: usize) -> Vec<u8> { self.b.read_va_prefix(va, len) }
```

- [ ] **Step 2: `rsp.rs`, with its unit tests first.** Create `crates/retrace/src/rsp.rs`. The
  module holds only what Task 2's server calls. The tests at the bottom are its first content; write
  them, then the code above them:

```rust
//! M43: the gdb-remote serial protocol, pure: framing, hex, the register description, stop replies
//! and the image list (spec `docs/superpowers/specs/2026-09-25-retrace-m43-lldb-design.md` §3a).
//! No VM and no socket: `gdbserver.rs` owns those, so everything here is unit-tested without either.
use retrace_core::ThreadCtx;

/// One unit off the wire.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    /// A packet's payload, checksum verified and `}` escapes undone.
    Packet(String),
    Ack,
    Nak,
    /// The raw 0x03 byte. §3h: ignored outside a motion.
    Interrupt,
    /// A packet whose checksum did not match.
    Bad,
}

/// Incremental framing: `push` whatever one read returned, then take frames until `next` says it
/// needs more bytes. One read may hold half a packet, or several (Review Focus 1).
#[derive(Default)]
pub(crate) struct Decoder { buf: Vec<u8> }

impl Decoder {
    pub(crate) fn push(&mut self, bytes: &[u8]) { self.buf.extend_from_slice(bytes); }

    /// The next whole frame, or None until more bytes arrive. Bytes that start no frame are dropped.
    pub(crate) fn next(&mut self) -> Option<Frame> {
        loop {
            let &b = self.buf.first()?;
            match b {
                b'+' => { self.buf.remove(0); return Some(Frame::Ack); }
                b'-' => { self.buf.remove(0); return Some(Frame::Nak); }
                0x03 => { self.buf.remove(0); return Some(Frame::Interrupt); }
                b'$' => {
                    // A `#` inside a payload is always escaped (`}` then 0x03), so the first raw `#`
                    // ends it.
                    let hash = self.buf.iter().position(|&c| c == b'#')?;
                    if self.buf.len() < hash + 3 { return None; }
                    let body = self.buf[1..hash].to_vec();
                    let cc = std::str::from_utf8(&self.buf[hash + 1..hash + 3]).ok()
                        .and_then(|s| u8::from_str_radix(s, 16).ok());
                    self.buf.drain(..hash + 3);
                    let sum = body.iter().fold(0u8, |a, &c| a.wrapping_add(c));
                    return Some(if cc == Some(sum) {
                        Frame::Packet(String::from_utf8_lossy(&unescape(&body)).into_owned())
                    } else {
                        Frame::Bad
                    });
                }
                _ => { self.buf.remove(0); }
            }
        }
    }
}

/// `$<payload>#<checksum>`, escaping `#`, `$`, `}` and `*` as `}` + (byte ^ 0x20). The JSON image
/// list is full of `}`, so every reply goes through this.
pub(crate) fn encode(payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(payload.len() + 8);
    for &c in payload {
        if matches!(c, b'#' | b'$' | b'}' | b'*') { body.push(b'}'); body.push(c ^ 0x20); } else { body.push(c); }
    }
    let sum = body.iter().fold(0u8, |a, &c| a.wrapping_add(c));
    let mut out = Vec::with_capacity(body.len() + 4);
    out.push(b'$');
    out.extend_from_slice(&body);
    out.push(b'#');
    out.extend_from_slice(format!("{sum:02x}").as_bytes());
    out
}

/// Undo `}` escapes.
fn unescape(p: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(p.len());
    let mut it = p.iter();
    while let Some(&c) = it.next() {
        if c == b'}' { if let Some(&n) = it.next() { out.push(n ^ 0x20); } } else { out.push(c); }
    }
    out
}

pub(crate) fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }

pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 { return None; }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// §3g: target.xml's register order. x0-x28, fp, lr, sp, pc, cpsr (32 bits), v0-v31, fpsr, fpcr.
pub(crate) const NREGS: usize = 68;

/// §3g: register `n` of `ctx`, little-endian, in target.xml's order. `cpsr` is `regs.cpsr`, never
/// `spsr`: a blocked thread's `spsr` is raw exception-entry state (t0 R1).
pub(crate) fn reg_bytes(ctx: &ThreadCtx, n: usize) -> Option<Vec<u8>> {
    Some(match n {
        0..=30 => ctx.regs.x[n].to_le_bytes().to_vec(),
        31 => ctx.regs.sp_el0.to_le_bytes().to_vec(),
        32 => ctx.regs.pc.to_le_bytes().to_vec(),
        33 => (ctx.regs.cpsr as u32).to_le_bytes().to_vec(),
        34..=65 => ctx.fp[n - 34].to_le_bytes().to_vec(),
        66 => (ctx.fpsr as u32).to_le_bytes().to_vec(),
        67 => (ctx.fpcr as u32).to_le_bytes().to_vec(),
        _ => return None,
    })
}

/// `g`: every register, in order.
pub(crate) fn all_regs_hex(ctx: &ThreadCtx) -> String {
    (0..NREGS).map(|n| hex(&reg_bytes(ctx, n).expect("n < NREGS"))).collect()
}

/// §3g: the aarch64 register description, in the shape t0's stub served and lldb-2100 accepted (t0
/// L1: GPRs and FP, `generic` names, lldb derives the `w` registers itself).
pub(crate) fn target_xml() -> String {
    let mut x = vec![
        r#"<?xml version="1.0"?>"#.to_string(),
        r#"<!DOCTYPE target SYSTEM "gdb-target.dtd">"#.to_string(),
        r#"<target version="1.0">"#.to_string(),
        "<architecture>aarch64</architecture>".to_string(),
        r#"<feature name="org.gnu.gdb.aarch64.core">"#.to_string(),
    ];
    let mut off = 0;
    for i in 0..=33usize {
        let name = match i { 29 => "fp".to_string(), 30 => "lr".into(), 31 => "sp".into(),
                             32 => "pc".into(), 33 => "cpsr".into(), n => format!("x{n}") };
        let bits = if i == 33 { 32 } else { 64 };
        let ty = match i { 32 => "code_ptr", 29 | 31 => "data_ptr", _ => "int" };
        let alt = match i { 29 => r#" altname="x29""#, 30 => r#" altname="x30""#, _ => "" };
        let generic = match i { 0..=7 => format!(r#" generic="arg{}""#, i + 1), 29 => r#" generic="fp""#.into(),
                                30 => r#" generic="ra""#.into(), 31 => r#" generic="sp""#.into(),
                                32 => r#" generic="pc""#.into(), 33 => r#" generic="flags""#.into(),
                                _ => String::new() };
        x.push(format!(r#"<reg name="{name}"{alt} bitsize="{bits}" offset="{off}" regnum="{i}" type="{ty}" group="general"{generic}/>"#));
        off += bits / 8;
    }
    x.push("</feature>".into());
    x.push(r#"<feature name="org.gnu.gdb.aarch64.fpu">"#.into());
    for v in 0..32usize {
        x.push(format!(r#"<reg name="v{v}" bitsize="128" offset="{off}" regnum="{}" encoding="vector" format="vector-uint8" group="float"/>"#, 34 + v));
        off += 16;
    }
    for (i, name) in [(66, "fpsr"), (67, "fpcr")] {
        x.push(format!(r#"<reg name="{name}" bitsize="32" offset="{off}" regnum="{i}" type="int" group="float"/>"#));
        off += 4;
    }
    x.push("</feature>".into());
    x.push("</target>".into());
    x.join("\n")
}

/// A `qXfer:…:read` answer for `[off, off + len)` of `doc`: `m` + chunk while more remains, `l` +
/// the last chunk (possibly empty).
pub(crate) fn xfer_chunk(doc: &[u8], off: usize, len: usize) -> String {
    let start = off.min(doc.len());
    let end = off.saturating_add(len).min(doc.len());
    let tag = if end < doc.len() { 'm' } else { 'l' };
    format!("{tag}{}", String::from_utf8_lossy(&doc[start..end]))
}

/// §3c: what kind of stop a reply reports. Task 2 has the two its server sends. Tasks 3 and 4 add
/// the rest, with their `signal()` and `keys()` arms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StopKind {
    /// `replaylog:begin` with a description: lldb's reversible "history boundary" stop (t0 L4f).
    HistoryBegin(String),
    /// No reason: `qThreadStopInfo` for a thread that is not the one reporting (t0 L7).
    None,
}

impl StopKind {
    fn signal(&self) -> u8 {
        match self { StopKind::HistoryBegin(_) => 5, StopKind::None => 0 }
    }
    fn keys(&self) -> String {
        match self {
            StopKind::HistoryBegin(d) => format!("replaylog:begin;description:{};", hex(d.as_bytes())),
            StopKind::None => String::new(),
        }
    }
}

/// §3c: `T<sig>thread:<tid>;threads:…;thread-pcs:…;<fp lr sp pc cpsr>;<kind>`. `tid` and
/// `threads` are RSP thread ids (retrace's + 1: tid 0 is unusable, t0 L7).
pub(crate) fn stop_reply(tid: u32, ctx: &ThreadCtx, threads: &[(u32, u64)], kind: &StopKind) -> String {
    let join = |f: &dyn Fn(&(u32, u64)) -> String| threads.iter().map(f).collect::<Vec<_>>().join(",");
    let mut s = format!("T{:02x}thread:{tid:x};threads:{};thread-pcs:{};", kind.signal(),
                        join(&|(t, _)| format!("{t:x}")), join(&|(_, pc)| format!("{pc:x}")));
    for r in 29..=33usize {
        s += &format!("{r:02x}:{};", hex(&reg_bytes(ctx, r).expect("an expedited register")));
    }
    s + &kind.keys()
}

/// §3g: `jGetLoadedDynamicLibrariesInfos`' answer for one image, in the shape lldb-2100 took
/// from t0's stub (t0 L2, `l2_jimg_exe`). `hdr` is the image's `mach_header_64` followed by its
/// load commands, read out of the recording. `path` must name the same binary on disk, or lldb
/// loads the wrong file.
pub(crate) fn image_json(hdr: &[u8], load_address: u64, path: &str) -> Result<String, String> {
    let u32at = |o: usize| hdr.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .ok_or_else(|| format!("Mach-O header truncated at {o:#x}"));
    let u64at = |o: usize| hdr.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
        .ok_or_else(|| format!("Mach-O header truncated at {o:#x}"));
    let magic = u32at(0)?;
    if magic != 0xfeed_facf { return Err(format!("not a 64-bit Mach-O header (magic {magic:#x})")); }
    let (cputype, cpusub, ftype, ncmds, flags) = (u32at(4)?, u32at(8)?, u32at(12)?, u32at(16)?, u32at(24)?);
    struct Seg { name: String, vmaddr: u64, vmsize: u64, fileoff: u64, filesize: u64, maxprot: i32 }
    let (mut segs, mut uuid, mut off) = (Vec::new(), None, 32usize);
    for _ in 0..ncmds {
        let (cmd, size) = (u32at(off)?, u32at(off + 4)? as usize);
        if size == 0 { return Err(format!("a zero-size load command at {off:#x}")); }
        match cmd {
            0x19 => { // LC_SEGMENT_64
                let raw = hdr.get(off + 8..off + 24).ok_or("segment name truncated")?;
                let name = String::from_utf8_lossy(raw).trim_end_matches('\0').to_string();
                segs.push(Seg { name, vmaddr: u64at(off + 24)?, vmsize: u64at(off + 32)?,
                                fileoff: u64at(off + 40)?, filesize: u64at(off + 48)?,
                                maxprot: u32at(off + 56)? as i32 });
            }
            0x1b => { // LC_UUID
                let u = hdr.get(off + 8..off + 24).ok_or("uuid truncated")?;
                let h = hex(u).to_uppercase();
                uuid = Some(format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]));
            }
            _ => {}
        }
        off += size;
    }
    let text = segs.iter().find(|s| s.name == "__TEXT").ok_or("no __TEXT segment")?;
    let slide = load_address.wrapping_sub(text.vmaddr);
    let uuid = uuid.ok_or("no LC_UUID")?;
    let seg_json: Vec<String> = segs.iter().map(|s| {
        let vmaddr = if s.name == "__PAGEZERO" { s.vmaddr } else { s.vmaddr.wrapping_add(slide) };
        format!(r#"{{"name":"{}","vmaddr":{vmaddr},"vmsize":{},"fileoff":{},"filesize":{},"maxprot":{}}}"#,
                s.name, s.vmsize, s.fileoff, s.filesize, s.maxprot)
    }).collect();
    let path = path.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!(concat!(r#"{{"images":[{{"load_address":{},"mod_date":0,"pathname":"{}","uuid":"{}","#,
                       r#""min_version_os_name":"macosx","min_version_os_sdk":"26.0","#,
                       r#""mach_header":{{"magic":{},"cputype":{},"cpusubtype":{},"filetype":{},"flags":{}}},"#,
                       r#""segments":[{}]}}]}}"#),
               load_address, path, uuid, magic, cputype as i32, cpusub, ftype, flags, seg_json.join(",")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(p: &str) -> Vec<u8> { encode(p.as_bytes()) }

    #[test] fn a_packet_split_across_reads_and_two_in_one_read_both_frame() {
        let mut d = Decoder::default();
        let whole = packet("qSupported:xmlRegisters=aarch64");
        d.push(&whole[..7]);
        assert_eq!(d.next(), None, "half a packet is not a frame");
        d.push(&whole[7..]);
        assert_eq!(d.next(), Some(Frame::Packet("qSupported:xmlRegisters=aarch64".into())));
        let mut two = b"+".to_vec();
        two.extend(packet("?"));
        two.push(0x03);
        two.extend(packet("g"));
        d.push(&two);
        assert_eq!(d.next(), Some(Frame::Ack));
        assert_eq!(d.next(), Some(Frame::Packet("?".into())));
        assert_eq!(d.next(), Some(Frame::Interrupt));
        assert_eq!(d.next(), Some(Frame::Packet("g".into())));
        assert_eq!(d.next(), None);
    }

    #[test] fn a_bad_checksum_is_a_bad_frame_and_escapes_round_trip() {
        let mut d = Decoder::default();
        d.push(b"$g#00");
        assert_eq!(d.next(), Some(Frame::Bad));
        let tricky = r#"{"a":"$#*}"}"#;
        let wire = encode(tricky.as_bytes());
        assert!(!wire[1..wire.len() - 3].contains(&b'#'), "no raw # inside the body");
        d.push(&wire);
        assert_eq!(d.next(), Some(Frame::Packet(tricky.into())));
    }

    #[test] fn hex_round_trips_and_rejects_odd_input() {
        assert_eq!(hex(&[0x00, 0xab, 0xff]), "00abff");
        assert_eq!(unhex("00abff"), Some(vec![0x00, 0xab, 0xff]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
    }

    #[test] fn xfer_reads_in_chunks_and_past_the_end() {
        let doc = b"0123456789";
        assert_eq!(xfer_chunk(doc, 0, 4), "m0123");
        assert_eq!(xfer_chunk(doc, 8, 4), "l89");
        assert_eq!(xfer_chunk(doc, 20, 4), "l", "past the end: the last, empty chunk");
    }

    #[test] fn the_register_description_numbers_68_registers_in_gdb_order() {
        let x = target_xml();
        assert!(x.contains(r#"<reg name="x0" bitsize="64" offset="0" regnum="0""#), "{x}");
        assert!(x.contains(r#"<reg name="pc" bitsize="64" offset="256" regnum="32" type="code_ptr""#), "{x}");
        assert!(x.contains(r#"<reg name="cpsr" bitsize="32" offset="264" regnum="33""#), "{x}");
        assert!(x.contains(r#"<reg name="v0" bitsize="128" offset="268" regnum="34""#), "{x}");
        assert!(x.contains(r#"<reg name="fpcr" bitsize="32" offset="784" regnum="67""#), "{x}");
        let ctx = ThreadCtx::zeroed();
        assert_eq!(all_regs_hex(&ctx).len(), 788 * 2, "g is every register's bytes, in order");
        assert_eq!(reg_bytes(&ctx, NREGS), None);
    }

    #[test] fn a_start_stop_names_its_thread_every_thread_and_the_expedited_registers() {
        let mut ctx = ThreadCtx::zeroed();
        ctx.regs.pc = 0x1_0000_0380;
        ctx.regs.sp_el0 = 0x1_c000;
        let s = stop_reply(1, &ctx, &[(1, 0x1_0000_0380)], &StopKind::HistoryBegin("start of recording".into()));
        assert!(s.starts_with("T05thread:1;threads:1;thread-pcs:100000380;"), "{s}");
        assert!(s.contains("1f:00c0010000000000;20:8003000001000000;21:00000000;"), "{s}");
        assert!(s.ends_with(&format!("replaylog:begin;description:{};", hex(b"start of recording"))), "{s}");
        assert_eq!(stop_reply(2, &ctx, &[(1, 0), (2, 0)], &StopKind::None).get(..3), Some("T00"));
    }

    #[test] fn the_image_list_describes_a_real_binary_at_its_load_address() {
        let file = std::fs::read(retrace_guest::HELLO).expect("the hello fixture");
        let j = image_json(&file, 0x1_0000_0000, retrace_guest::HELLO).unwrap();
        assert!(j.starts_with(r#"{"images":[{"load_address":4294967296,"mod_date":0,"pathname":""#), "{j}");
        assert!(j.contains(r#""name":"__TEXT","vmaddr":4294967296"#), "{j}");
        let uuid = j.split(r#""uuid":""#).nth(1).and_then(|r| r.split('"').next()).unwrap();
        assert_eq!(uuid.len(), 36, "{uuid}");
        assert!(image_json(&file[..16], 0x1_0000_0000, "x").is_err(), "a truncated header is an Err, not a panic");
    }
}
```

`ThreadCtx::zeroed()` is `pub` on `retrace_box::thread::ThreadCtx` (re-exported by Step 1).
`retrace_guest::HELLO` is a thin arm64 Mach-O with `LC_UUID`. If it lacks `LC_UUID`, use the first
fixture that has one (`otool -l <file> | grep LC_UUID`), and say which in the report.

- [ ] **Step 3: `Exec` visibility.** In `crates/retrace/src/debug.rs`:
  - make `struct Exec`, `fn new`, `fn sess` and `enum Phase` `pub(crate)`;
  - add, next to `sess`:

```rust
    /// M43: the cursor `(n, k, phase)` (M41 §3b), for the gdb-remote server.
    pub(crate) fn cursor(&self) -> (usize, u64, Phase) { (self.n, self.k, self.phase) }
```

- [ ] **Step 4: `gdbserver.rs`, the non-motion server.** Create `crates/retrace/src/gdbserver.rs`:

```rust
//! M43: `retrace gdbserver <trace>` — a gdb-remote server over one recording, for lldb (spec
//! `docs/superpowers/specs/2026-09-25-retrace-m43-lldb-design.md`). A translation layer, not a
//! second debugger: every motion is one of `Exec`'s, so M41's hit order and M42's pair handling hold
//! under lldb by construction. This file owns the socket, the dispatch, §3c's position mapping and
//! §3d's step rule.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use retrace_core::{ReplaySession, ThreadState, EXE_BASE};
use crate::debug::Exec;
use crate::rsp::{self, Decoder, Frame, StopKind};

/// §3g, spec R10: fixed, so lldb's transcripts are stable (t0 L10), and `os_version` only selects
/// lldb's newer macOS loader (t0 L2).
const QHOSTINFO: &str = "cputype:16777228;cpusubtype:0;ostype:macosx;vendor:apple;endian:little;ptrsize:8;watchpoint_exceptions_received:after;";
const OS_VERSION: &str = "os_version:26.0.0;";
const QPROCESSINFO: &str = "pid:1;parent-pid:1;cputype:100000c;cpusubtype:0;ostype:macosx;vendor:apple;endian:little;ptrsize:8;";
const QSUPPORTED: &str = "PacketSize=20000;QStartNoAckMode+;qXfer:features:read+;QThreadSuffixSupported+;QListThreadsInStopReply+;ReverseContinue+;ReverseStep+";
/// The largest `m` answered, in bytes: half the advertised PacketSize, in hex.
const MAX_READ: usize = 0x10000;

/// Serve one lldb connection on `127.0.0.1:port` (0 picks a free port), then return. The one line
/// on stderr is how a caller learns the port.
///
/// The session is opened BEFORE the port is announced. Opening decodes the whole recording
/// (seconds for CPython), and lldb gives its first packet only its packet timeout. Once the line is
/// printed, every reply is ready.
pub fn serve(trace: &Path, port: u16, exe: Option<String>) -> Result<(), String> {
    let mut srv = Server::new(trace, exe)?;
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("bind 127.0.0.1:{port}: {e}"))?;
    let port = listener.local_addr().map_err(|e| format!("local_addr: {e}"))?.port();
    eprintln!("listening on 127.0.0.1:{port}");
    let (mut sock, _) = listener.accept().map_err(|e| format!("accept: {e}"))?;
    let _ = sock.set_nodelay(true);
    let mut dec = Decoder::default();
    let mut buf = vec![0u8; 64 * 1024];
    let mut last_sent: Vec<u8> = Vec::new();
    let io = |e: std::io::Error| format!("socket: {e}");
    loop {
        let n = sock.read(&mut buf).map_err(io)?;
        if n == 0 { return Ok(()); } // the peer closed without `k` or `D` (Review Focus 3)
        dec.push(&buf[..n]);
        while let Some(frame) = dec.next() {
            match frame {
                Frame::Ack | Frame::Interrupt => {} // §3h: 0x03 outside a motion is ignored
                Frame::Nak => { if !srv.no_ack { sock.write_all(&last_sent).map_err(io)?; } }
                Frame::Bad => { if !srv.no_ack { sock.write_all(b"-").map_err(io)?; } }
                Frame::Packet(p) => {
                    if !srv.no_ack { sock.write_all(b"+").map_err(io)?; }
                    let (replies, close) = srv.handle(&p);
                    for r in replies {
                        last_sent = rsp::encode(r.as_bytes());
                        sock.write_all(&last_sent).map_err(io)?;
                    }
                    if p == "QStartNoAckMode" { srv.no_ack = true; } // after its OK went out acked
                    if close { return Ok(()); }
                }
            }
        }
    }
}

pub(crate) struct Server<'a> {
    ex: Exec<'a>,
    /// §3g: the exe's path when known. With it, lldb gets `os_version` and the image list.
    exe: Option<String>,
    /// §3g: the exe's `mach_header_64` and load commands, read once from the opening snapshot.
    exe_hdr: Vec<u8>,
    no_ack: bool,
    /// The reply to `?`: the last stop reported.
    last_stop: String,
    /// The RSP tid `last_stop` names, for `qThreadStopInfo`.
    last_tid: u32,
    /// `Hg`: the thread register reads without a thread suffix go to (an RSP tid; 0 means current).
    hg: u32,
}

impl<'a> Server<'a> {
    fn new(trace: &'a Path, exe_arg: Option<String>) -> Result<Self, String> {
        let ex = Exec::new(trace)?;
        let s = ex.sess();
        let head = s.read_mem_prefix(EXE_BASE, 32);
        let sizeofcmds = head.get(20..24).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
        let exe_hdr = s.read_mem_prefix(EXE_BASE, 32 + sizeofcmds);
        let exe = exe_arg.or_else(|| argv0(s));
        let mut srv = Server { ex, exe, exe_hdr, no_ack: false, last_stop: String::new(), last_tid: 0, hg: 0 };
        srv.stop(StopKind::HistoryBegin("start of recording".into()), None); // the answer to `?`
        Ok(srv)
    }

    /// `(RSP tid, pc)` for every thread that has not exited, in id order.
    fn live_threads(&self) -> Vec<(u32, u64)> {
        let s = self.ex.sess();
        s.thread_summaries().into_iter()
            .filter(|t| !matches!(t.state, ThreadState::Exited(_)))
            .filter_map(|t| s.thread_ctx(t.tid as usize).map(|c| (t.tid + 1, c.regs.pc)))
            .collect()
    }

    /// §3c: a stop reply for `thread` (retrace's id; the current thread when None), and remember
    /// it as the answer to `?`.
    fn stop(&mut self, kind: StopKind, thread: Option<u32>) -> String {
        let t = thread.unwrap_or_else(|| self.ex.sess().current_thread());
        let ctx = self.ex.sess().thread_ctx(t as usize).expect("a stop names a thread in the table");
        let r = rsp::stop_reply(t + 1, &ctx, &self.live_threads(), &kind);
        self.last_stop = r.clone();
        self.last_tid = t + 1;
        r
    }

    /// Which retrace thread a register read addresses: the packet's `;thread:<tid>;` suffix, else
    /// `Hg`, else the current thread. None for an id that does not exist (Review Focus 4).
    fn reg_thread(&self, suffix: Option<&str>) -> Option<usize> {
        let rsp_tid = match suffix { Some(t) => u32::from_str_radix(t, 16).ok()?, None => self.hg };
        let cur = self.ex.sess().current_thread();
        let t = if rsp_tid == 0 { cur } else { rsp_tid - 1 };
        self.live_threads().iter().any(|&(r, _)| r == t + 1).then_some(t as usize)
    }

    /// One packet's replies, and whether the session ends after them.
    pub(crate) fn handle(&mut self, p: &str) -> (Vec<String>, bool) {
        let one = |s: &str| (vec![s.to_string()], false);
        let (body, suffix) = match p.split_once(";thread:") {
            Some((b, t)) => (b, Some(t.trim_end_matches(';'))),
            None => (p, None),
        };
        match body {
            "QStartNoAckMode" | "QThreadSuffixSupported" | "QListThreadsInStopReply" | "QEnableErrorStrings" => one("OK"),
            "qHostInfo" => one(&match self.exe { Some(_) => format!("{QHOSTINFO}{OS_VERSION}"), None => QHOSTINFO.into() }),
            "qProcessInfo" => one(QPROCESSINFO),
            "vCont?" => one("vCont;c;C;s;S"),
            "?" => (vec![self.last_stop.clone()], false),
            "qC" => one(&format!("QC{:x}", self.ex.sess().current_thread() + 1)),
            "qfThreadInfo" => one(&format!("m{}", self.live_threads().iter()
                .map(|(t, _)| format!("{t:x}")).collect::<Vec<_>>().join(","))),
            "qsThreadInfo" => one("l"),
            "g" => match self.reg_thread(suffix).and_then(|t| self.ex.sess().thread_ctx(t)) {
                Some(ctx) => one(&rsp::all_regs_hex(&ctx)),
                None => one("E01"),
            },
            "k" => (vec!["X09".into()], true),
            // `D;<pid>` is the multiprocess form.
            _ if body == "D" || body.starts_with("D;") => (vec!["OK".into()], true),
            _ if body.starts_with("qSupported") => one(QSUPPORTED),
            _ if body.starts_with("qXfer:features:read:target.xml:") => {
                let range = &body["qXfer:features:read:target.xml:".len()..];
                match range.split_once(',').and_then(|(o, l)| Some((usize::from_str_radix(o, 16).ok()?, usize::from_str_radix(l, 16).ok()?))) {
                    Some((o, l)) => one(&rsp::xfer_chunk(rsp::target_xml().as_bytes(), o, l)),
                    None => one("E01"),
                }
            }
            _ if body.starts_with("qThreadStopInfo") => {
                let Ok(t) = u32::from_str_radix(&body["qThreadStopInfo".len()..], 16) else { return one("E01") };
                if t == self.last_tid { return (vec![self.last_stop.clone()], false); }
                match t.checked_sub(1).and_then(|i| self.ex.sess().thread_ctx(i as usize)) {
                    Some(ctx) if self.live_threads().iter().any(|&(r, _)| r == t) =>
                        one(&rsp::stop_reply(t, &ctx, &self.live_threads(), &StopKind::None)),
                    _ => one("E01"),
                }
            }
            _ if body.starts_with("Hg") || body.starts_with("Hc") => {
                if body.starts_with("Hg") {
                    // `Hg-1` and `Hg0` both mean "any thread": the current one.
                    self.hg = u32::from_str_radix(&body[2..], 16).unwrap_or(0);
                }
                one("OK")
            }
            _ if body.starts_with('p') => {
                let Ok(n) = usize::from_str_radix(&body[1..], 16) else { return one("E01") };
                match self.reg_thread(suffix).and_then(|t| self.ex.sess().thread_ctx(t))
                    .and_then(|ctx| rsp::reg_bytes(&ctx, n)) {
                    Some(b) => one(&rsp::hex(&b)),
                    None => one("E01"),
                }
            }
            _ if body.starts_with('m') => {
                let parsed = body[1..].split_once(',').and_then(|(a, l)|
                    Some((u64::from_str_radix(a, 16).ok()?, usize::from_str_radix(l, 16).ok()?)));
                let Some((addr, len)) = parsed else { return one("E01") };
                let bytes = self.ex.sess().read_mem_prefix(addr, len.min(MAX_READ));
                if bytes.is_empty() && len > 0 { one("E08") } else { one(&rsp::hex(&bytes)) }
            }
            // §3h: a recording is read-only. Refusing P also stops expression evaluation from
            // resuming the replay under registers lldb invented (t0 L8).
            _ if body.starts_with('P') || body.starts_with('G') || body.starts_with('M') || body.starts_with('X')
                || body.starts_with("QSaveRegisterState") || body.starts_with("QRestoreRegisterState") => one("E01"),
            _ if body.starts_with("qRcmd,") => self.monitor(&body["qRcmd,".len()..]),
            _ if body.starts_with("jGetLoadedDynamicLibrariesInfos:") => {
                let arg = &body["jGetLoadedDynamicLibrariesInfos:".len()..];
                match &self.exe {
                    None => one(""),
                    Some(_) if arg.is_empty() => one("OK"), // the support probe (t0 L2)
                    Some(path) => {
                        let wants_exe = arg.contains("\"fetch_all_solibs\":true")
                            || arg.contains(&EXE_BASE.to_string());
                        let json = if wants_exe {
                            rsp::image_json(&self.exe_hdr, EXE_BASE, path).unwrap_or_else(|_| r#"{"images":[]}"#.into())
                        } else {
                            r#"{"images":[]}"#.into()
                        };
                        one(&json)
                    }
                }
            }
            _ => one(""), // §3h: every other packet is unsupported, motion included until Task 3
        }
    }

    /// `qRcmd`: `process plugin packet monitor <cmd>` (§3e). Output goes out as `O` packets, then
    /// the final `OK`.
    fn monitor(&mut self, hexcmd: &str) -> (Vec<String>, bool) {
        let cmd = rsp::unhex(hexcmd).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        match cmd.trim() {
            "where" => {
                let (n, k, phase) = self.ex.cursor();
                let s = self.ex.sess();
                let text = format!("at ({n}, {k}) phase={phase:?} pc={:#x} thread={}\n", s.pc(), s.current_thread() + 1);
                (vec![format!("O{}", rsp::hex(text.as_bytes())), "OK".into()], false)
            }
            _ => (vec!["E01".into()], false),
        }
    }
}

/// §3g (t0 R3): `record-dyn`'s argv[0], out of the opening stack: `[sp]` is the main image's
/// mach_header (the KernelArgs `build_start_stack` writes) and `[sp + 16]` points at argv[0]. Only
/// an absolute path is trusted, because the recording's working directory is not recorded. A
/// static guest has no KernelArgs, so `[sp]` is not `EXE_BASE` and this is None.
fn argv0(s: &ReplaySession) -> Option<String> {
    let sp = s.thread_ctx(0)?.regs.sp_el0;
    let word = |a: u64| <[u8; 8]>::try_from(s.read_mem_prefix(a, 8)).ok().map(u64::from_le_bytes);
    if word(sp)? != EXE_BASE { return None; }
    let bytes = s.read_mem_prefix(word(sp + 16)?, 1024);
    let end = bytes.iter().position(|&b| b == 0)?;
    let path = String::from_utf8(bytes[..end].to_vec()).ok()?;
    path.starts_with('/').then_some(path)
}
```

Notes for the implementer:
- `thread_summaries()` returns `ThreadSummary { tid: u32, state, is_current }`.
- If clippy flags a style lint in this code (`manual_strip` on a `starts_with` guard, for
  example), fix it with the idiomatic form and keep the behaviour. That is not a plan deviation.
- If a borrow-checker detail forces a different shape (e.g. `stop()` taking `&mut self`), keep the
  behaviour and adjust.
- Do not add `StopKind` variants here: Task 3 adds them with their first use.

- [ ] **Step 5: The CLI arm.** In `crates/retrace/src/main.rs`:
  - add `mod rsp;` and `mod gdbserver;` next to `mod debug;`;
  - add the arm after `Some("debug")`;
  - add `| gdbserver <trace> [--port <n>] [--exe <path>]` to the usage line.

```rust
        Some("gdbserver") => {
            // retrace gdbserver <trace> [--port <n>] [--exe <path>] (M43): a gdb-remote server for
            // lldb over one recording. Usage errors exit 2 before any socket or VM work.
            let opt = |name: &str| a.iter().position(|s| s == name).and_then(|i| a.get(i + 1));
            let port = match opt("--port") { None => Some(0u16), Some(p) => p.parse::<u16>().ok() };
            match (a.get(2).filter(|t| !t.starts_with("--")), port) {
                (Some(trace), Some(port)) => match gdbserver::serve(Path::new(trace), port, opt("--exe").cloned()) {
                    Ok(()) => exit(0),
                    Err(e) => { eprintln!("GDBSERVER ERROR: {e}"); exit(5); }
                },
                _ => { eprintln!("usage: retrace gdbserver <trace> [--port <n>] [--exe <path>]"); exit(2); }
            }
        }
```

- [ ] **Step 6: The test client.** Create `crates/retrace/tests/util/rsp.rs` and add `pub mod rsp;`
  to `util/mod.rs` beside `pub mod hits;`:

```rust
//! M43: a minimal gdb-remote client for `gdbserver_e2e`. It spawns `retrace gdbserver` (the
//! codesigned copy), learns the port from its one stderr line, and exchanges packets. The
//! handshake uses ack mode; everything after `QStartNoAckMode` uses no-ack mode, as lldb does.
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Rsp { child: Child, stream: TcpStream, buf: Vec<u8> }

/// Seconds a reply may take before the test fails instead of hanging the gate.
const REPLY_BOUND: u64 = 180;

/// Start `retrace gdbserver <trace> --port 0 <extra…>` (the codesigned copy) and wait for its
/// `listening on 127.0.0.1:<port>` line. The server's stderr goes to a file, never a pipe: a pipe
/// nobody reads after the first line would block a server that writes more (`debug_bounded`'s
/// reasoning). It polls with `thread::sleep`, because clippy bans reading a clock. Returns the child,
/// the port, and the stderr file (the caller removes it).
pub fn spawn_server(trace: &Path, extra: &[&str]) -> (Child, u16, PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let err_p = std::env::temp_dir().join(format!("retrace-gdbserver-{}-{n}.err", std::process::id()));
    let mut child = Command::new(super::bin())
        .args(["gdbserver", trace.to_str().unwrap(), "--port", "0"]).args(extra)
        .stdin(Stdio::null()).stdout(Stdio::null())
        .stderr(std::fs::File::create(&err_p).expect("create gdbserver stderr file"))
        .spawn().expect("spawn gdbserver");
    for _ in 0..REPLY_BOUND * 20 {
        let s = std::fs::read_to_string(&err_p).unwrap_or_default();
        if let Some(line) = s.split_once("listening on 127.0.0.1:").and_then(|(_, r)| r.split_once('\n')) {
            return (child, line.0.trim().parse().expect("a port number"), err_p);
        }
        if let Some(st) = child.try_wait().unwrap() {
            panic!("gdbserver exited ({st:?}) before listening:\n{s}");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.kill();
    panic!("gdbserver never printed its port");
}

fn encode(p: &str) -> Vec<u8> {
    let mut body = Vec::new();
    for &c in p.as_bytes() {
        if matches!(c, b'#' | b'$' | b'}' | b'*') { body.push(b'}'); body.push(c ^ 0x20); } else { body.push(c); }
    }
    let sum = body.iter().fold(0u8, |a, &c| a.wrapping_add(c));
    let mut out = vec![b'$'];
    out.extend(body);
    out.extend(format!("#{sum:02x}").as_bytes());
    out
}

impl Rsp {
    /// Start `retrace gdbserver <trace> --port 0 <extra…>` and connect, completing
    /// `QStartNoAckMode` in ack mode.
    pub fn spawn(trace: &Path, extra: &[&str]) -> Rsp {
        let (child, port, err_p) = spawn_server(trace, extra);
        let _ = std::fs::remove_file(err_p);
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream.set_read_timeout(Some(std::time::Duration::from_secs(REPLY_BOUND))).unwrap();
        let mut r = Rsp { child, stream, buf: Vec::new() };
        r.stream.write_all(&encode("QStartNoAckMode")).unwrap();
        assert_eq!(r.read_byte(), b'+', "the server acks in ack mode");
        assert_eq!(r.read_packet(), "OK");
        r.stream.write_all(b"+").unwrap();
        r
    }

    fn read_byte(&mut self) -> u8 {
        while self.buf.is_empty() {
            let mut b = [0u8; 65536];
            let n = self.stream.read(&mut b).expect("read (a timeout here means the server hung)");
            assert!(n > 0, "the server closed the connection");
            self.buf.extend_from_slice(&b[..n]);
        }
        self.buf.remove(0)
    }

    fn read_packet(&mut self) -> String {
        while self.read_byte() != b'$' {}
        let mut body = Vec::new();
        loop {
            match self.read_byte() {
                b'#' => break,
                b'}' => { let n = self.read_byte(); body.push(n ^ 0x20); }
                c => body.push(c),
            }
        }
        let (_, _) = (self.read_byte(), self.read_byte()); // the checksum; framing is the unit tests' job
        String::from_utf8(body).expect("a UTF-8 reply")
    }

    /// Send one packet and return the one reply.
    pub fn send(&mut self, p: &str) -> String {
        self.stream.write_all(&encode(p)).unwrap();
        self.read_packet()
    }

    /// Send one packet, collect `O` output packets (hex-decoded) until the final reply.
    pub fn send_collect(&mut self, p: &str) -> (String, String) {
        self.stream.write_all(&encode(p)).unwrap();
        let mut out = String::new();
        loop {
            let r = self.read_packet();
            match r.strip_prefix('O') {
                Some(h) if !h.is_empty() && h.len() % 2 == 0 && h.bytes().all(|c| c.is_ascii_hexdigit()) =>
                    out += &String::from_utf8((0..h.len()).step_by(2)
                        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect()).unwrap(),
                _ => return (out, r),
            }
        }
    }

    pub fn send_raw(&mut self, bytes: &[u8]) { self.stream.write_all(bytes).unwrap(); }

    /// `qRcmd,where`'s text, trimmed: `at (n, k) phase=… pc=0x… thread=t`.
    pub fn where_(&mut self) -> String {
        let hexcmd: String = "where".bytes().map(|b| format!("{b:02x}")).collect();
        let (out, fin) = self.send_collect(&format!("qRcmd,{hexcmd}"));
        assert_eq!(fin, "OK");
        out.trim().to_string()
    }

    fn wait_exit(&mut self) -> i32 {
        for _ in 0..REPLY_BOUND * 20 {
            if let Some(st) = self.child.try_wait().unwrap() { return st.code().unwrap_or(-1); }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = self.child.kill();
        panic!("gdbserver did not exit");
    }

    /// `k`: the reply, and the server's exit status.
    pub fn kill(mut self) -> (String, i32) { let r = self.send("k"); (r, self.wait_exit()) }
    /// `D`: the reply, and the server's exit status.
    pub fn detach(mut self) -> (String, i32) { let r = self.send("D"); (r, self.wait_exit()) }
    /// Close the socket without a word (Review Focus 3); the server's exit status.
    pub fn drop_connection(mut self) -> i32 {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        self.wait_exit()
    }
}

impl Drop for Rsp {
    fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); }
}

/// Decode a stop reply's `description:` (hex) field, if it has one.
pub fn description(stop: &str) -> Option<String> {
    let h = stop.split("description:").nth(1)?.split(';').next()?;
    String::from_utf8((0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect()).ok()
}

/// A stop reply's `key:` value (the first one). The reply's first three bytes are `T<sig>`, so the
/// first field reads `thread:<tid>` once they are skipped.
pub fn key<'a>(stop: &'a str, k: &str) -> Option<&'a str> {
    stop.get(3..)?.split(';').find_map(|f| f.strip_prefix(k)?.strip_prefix(':'))
}

/// A register value out of `p`'s little-endian hex.
pub fn le_u64(h: &str) -> u64 {
    let b: Vec<u8> = (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect();
    let mut a = [0u8; 8];
    a[..b.len().min(8)].copy_from_slice(&b[..b.len().min(8)]);
    u64::from_le_bytes(a)
}
```

`kill`, `detach` and `drop_connection` take `self`. `Drop` then kills a child that already exited,
which is harmless.

- [ ] **Step 7: `gdbserver_e2e`'s non-motion rows.** Create `crates/retrace/tests/gdbserver_e2e.rs`:

```rust
//! M43: the gdb-remote server, over the wire, without lldb (spec
//! `docs/superpowers/specs/2026-09-25-retrace-m43-lldb-design.md` §4). Repo-owned: it guards the
//! protocol on any machine. `lldb_e2e` guards lldb itself. Every expected value comes from a source
//! the server cannot influence: the recording, the fixture binary, or a fresh `ReplaySession` in
//! this process. The server is its own process, so the two VMs never meet.
mod util;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use util::rsp::{self as r, Rsp};

fn watchsweep() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    C.get_or_init(|| {
        let (rec, t) = util::record(retrace_guest::WATCHSWEEP);
        assert_eq!(rec.code, 0, "record watchsweep: {}", rec.stderr);
        t
    })
}
fn crashy() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    C.get_or_init(|| {
        let (rec, t) = util::record_dynamic(retrace_guest::CRASHY);
        assert_eq!(rec.code, 139, "record crashy: {}", rec.stderr);
        t
    })
}
fn unhex(h: &str) -> Vec<u8> { (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect() }
fn hexs(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

#[test]
fn the_handshake_stops_at_the_start_of_recording() {
    let mut c = Rsp::spawn(watchsweep(), &[]);
    let sup = c.send("qSupported:xmlRegisters=i386,arm,mips,arc;multiprocess+;swbreak+;hwbreak+");
    assert!(sup.contains("ReverseContinue+") && sup.contains("qXfer:features:read+"), "{sup}");
    let stop = c.send("?");
    assert!(stop.starts_with("T05thread:1;threads:1;"), "{stop}");
    assert_eq!(r::description(&stop).as_deref(), Some("start of recording"));
    assert!(stop.contains("replaylog:begin;"), "{stop}");
    assert_eq!(c.send("qC"), "QC1");
    assert_eq!(c.send("qfThreadInfo"), "m1");
    assert_eq!(c.send("qsThreadInfo"), "l");
    assert_eq!(c.send("vCont?"), "vCont;c;C;s;S");
    let xml = c.send("qXfer:features:read:target.xml:0,1ffff");
    assert!(xml.starts_with('l') && xml.contains(r#"regnum="67""#), "{xml}");
    assert_eq!(c.where_(), format!("at (1, 0) phase=Bp pc={:#x} thread=1",
        retrace_core::seek(watchsweep(), 1, 0).unwrap().pc()));
}

#[test]
fn registers_match_a_replay_session_at_the_same_position() {
    let s = retrace_core::seek(watchsweep(), 1, 0).unwrap();
    // The oracle is the script debugger's text dump, not thread_ctx. It pads single-digit names
    // (`x1 =0x…`, `format_gprs`), so close the gap before splitting.
    let text = s.dbg_regs().replace(" =", "=");
    let field = |name: &str| text.split_whitespace().find_map(|w| w.strip_prefix(&format!("{name}=")))
        .map(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).unwrap())
        .unwrap_or_else(|| panic!("no {name}= in dbg_regs:\n{text}"));
    let mut c = Rsp::spawn(watchsweep(), &[]);
    assert_eq!(r::le_u64(&c.send("p20;thread:1;")), s.pc(), "pc");
    assert_eq!(r::le_u64(&c.send("p1f;thread:1;")), field("sp"), "sp");
    let g = c.send("g");
    assert_eq!(g.len(), 788 * 2);
    for i in 0..31usize {
        assert_eq!(r::le_u64(&g[i * 16..i * 16 + 16]), field(&format!("x{i}")), "g's x{i}");
    }
    assert_eq!(r::le_u64(&g[32 * 16..32 * 16 + 16]), s.pc(), "g's pc slot");
    assert_eq!(c.send("p0;thread:9;"), "E01", "a thread that does not exist (Review Focus 4)");
    assert_eq!(c.send("p44;thread:1;"), "E01", "a register past fpcr");
}

#[test]
fn memory_reads_match_the_recording_and_a_straddling_read_returns_its_prefix() {
    let s = retrace_core::seek(watchsweep(), 1, 0).unwrap();
    let pc = s.pc();
    let mut c = Rsp::spawn(watchsweep(), &[]);
    assert_eq!(c.send(&format!("m{pc:x},10")), hexs(&s.read_mem(pc, 16).unwrap()));
    // Find the end of the mapping above pc, 16 KiB at a time, then read across it.
    let mut end = None;
    for j in 1..=256u64 {
        let a = (pc & !0x3fff) + j * 0x4000;
        if c.send(&format!("m{a:x},1")) == "E08" { end = Some(a); break; }
    }
    let end = end.expect("an unmapped page within 4 MiB of the code");
    assert_eq!(c.send(&format!("m{:x},10", end - 8)).len(), 16, "8 readable bytes, then the gap");
    assert_eq!(c.send("mfffffff000002010,8"), "E08", "lldb's kernel probe: refused, no panic (Review Focus 2)");
}

#[test]
fn writes_are_refused_and_move_nothing() {
    let mut c = Rsp::spawn(watchsweep(), &[]);
    let before = c.where_();
    for p in ["P0=0000000000000000", "G00", "M100004000,1:00", "X100004000,0:", "QSaveRegisterState",
              "QRestoreRegisterState:1"] {
        assert_eq!(c.send(p), "E01", "{p}");
    }
    assert_eq!(c.send("_M1000,rwx"), "", "no allocation (spec R8)");
    assert_eq!(c.where_(), before);
}

/// The first `LC_UUID` in a thin Mach-O file, formatted as lldb prints it.
fn file_uuid(path: &str) -> String {
    let f = std::fs::read(path).unwrap();
    let u32at = |o: usize| u32::from_le_bytes(f[o..o + 4].try_into().unwrap());
    let (mut off, n) = (32usize, u32at(16));
    for _ in 0..n {
        if u32at(off) == 0x1b {
            let h = hexs(&f[off + 8..off + 24]).to_uppercase();
            return format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]);
        }
        off += u32at(off + 4) as usize;
    }
    panic!("no LC_UUID in {path}")
}

#[test]
fn the_exe_is_listed_from_argv0_when_the_recording_has_one() {
    let mut c = Rsp::spawn(crashy(), &[]);
    assert!(c.send("qHostInfo").contains("os_version:"), "a known path selects lldb's newer loader");
    assert_eq!(c.send("jGetLoadedDynamicLibrariesInfos:"), "OK");
    let j = c.send(r#"jGetLoadedDynamicLibrariesInfos:{"fetch_all_solibs":true}"#);
    assert!(j.contains(&format!(r#""pathname":"{}""#, retrace_guest::CRASHY)), "{j}");
    assert!(j.contains(r#""load_address":4294967296"#), "{j}");
    assert!(j.contains(&format!(r#""uuid":"{}""#, file_uuid(retrace_guest::CRASHY))), "{j}");
}

#[test]
fn a_static_guest_lists_nothing_unless_given_its_path() {
    let mut c = Rsp::spawn(watchsweep(), &[]);
    assert!(!c.send("qHostInfo").contains("os_version:"), "no path: lldb's older loader (t0 L2)");
    assert_eq!(c.send("jGetLoadedDynamicLibrariesInfos:"), "");
    drop(c);
    let mut c = Rsp::spawn(watchsweep(), &["--exe", retrace_guest::WATCHSWEEP]);
    assert!(c.send("qHostInfo").contains("os_version:"));
    let j = c.send(r#"jGetLoadedDynamicLibrariesInfos:{"fetch_all_solibs":true}"#);
    assert!(j.contains(&format!(r#""pathname":"{}""#, retrace_guest::WATCHSWEEP)), "{j}");
}

#[test]
fn k_and_d_end_the_server_cleanly_and_so_does_a_dropped_connection() {
    let (reply, code) = Rsp::spawn(watchsweep(), &[]).kill();
    assert_eq!((reply.as_str(), code), ("X09", 0));
    let (reply, code) = Rsp::spawn(watchsweep(), &[]).detach();
    assert_eq!((reply.as_str(), code), ("OK", 0));
    assert_eq!(Rsp::spawn(watchsweep(), &[]).drop_connection(), 0, "EOF ends the session (Review Focus 3)");
}
```

`g`'s first 31 slots are 8 bytes each, so the hex offsets `i * 16` hold for x0–x30, and slot 32
(pc) starts at byte 256, hex offset 512.

- [ ] **Step 8: Run.**

```sh
cargo test -p retrace --bins --no-fail-fast -- --test-threads=1 > $L/t2-bins.log 2>&1; echo "exit=$?"
cargo test -p retrace --test gdbserver_e2e --no-fail-fast -- --test-threads=1 > $L/t2-e2e.log 2>&1; echo "exit=$?"
cargo test -p retrace --test debug_cli --test crashy_cli --test watch_cli --no-fail-fast -- --test-threads=1 > $L/t2-cli.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets -- -D warnings > $L/t2-clippy.log 2>&1; echo "exit=$?"
```

Expected:
- `--bins`: 17 + 7 = 24 passed;
- `gdbserver_e2e`: 7 passed;
- the CLI suites unchanged;
- clippy clean.

A clippy dead-code error means a Task 3 item leaked in: remove it.

- [ ] **Step 9: Commit.**

```sh
git add crates/retrace/src/rsp.rs crates/retrace/src/gdbserver.rs crates/retrace/src/main.rs crates/retrace/src/debug.rs crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs crates/retrace/tests/util/rsp.rs crates/retrace/tests/util/mod.rs crates/retrace/tests/gdbserver_e2e.rs
git commit -m "M43 t2: retrace gdbserver — the wire, registers, memory, threads and the image list"
```

---

### Task 3: Continue, both ways (spec §3b, §3c, §3f)

`Exec`'s four motions return what happened (`Halt`). The server's `c` and `bc` use them, with §3c's
re-parks, and every error becomes a stop at the saved cursor. Breakpoints and watchpoints arrive.
Stepping is Task 4.

**Files:**
- Modify: `crates/retrace/src/debug.rs`:
  - `Halt`;
  - four motions return it;
  - `park_at_terminal` returns it;
  - `recover`, `set_phase`, `park_before_event`;
  - breakpoint and watch accessors;
  - `pub(crate)` on `cmd_break`, `cmd_delete`, `cmd_watch`, `cmd_unwatch`, `cmd_continue`,
    `cmd_reverse_continue`, `cmd_stepi`, `cmd_reverse_stepi`;
  - two unit tests.
- Modify: `crates/retrace/src/rsp.rs` (`StopKind` gains `Breakpoint`, `Watch`, `HistoryEnd`,
  `MachBadAccess`, `Exception`)
- Modify: `crates/retrace/src/gdbserver.rs`:
  - the motion bracket;
  - `c`/`C`/`vCont;c`/`vCont;C` and `bc`;
  - `Z0`–`Z4`/`z0`–`z4`;
  - terminal stops;
  - `dead`.
- Modify: `crates/retrace/tests/util/mod.rs` (`tamper_last_write`)
- Test: `crates/retrace/tests/gdbserver_e2e.rs` (the continue rows)

**Interfaces:**
- Consumes: Task 2's `Server`, `stop()`, `StopKind`, `Exec::cursor`, `Rsp`.
- Produces for Task 4: `Halt` as below; `Exec::recover`, `Exec::set_phase`,
  `Exec::park_before_event`; `Server::motion`, `Server::reply_forward`.

```rust
/// M43 §3b: what one motion did, alongside the lines it printed. The script CLI ignores it; the
/// gdb-remote server reports it.
#[derive(Debug)]
pub(crate) enum Halt {
    /// Parked on a breakpoint, (n, k, Bp). `pc()` is its address.
    Break,
    /// A store to a watched range, parked pre-retire, (n, k, Watch).
    Watch { watched: u64 },
    /// A syscall's recorded write to a watched range, (n, 0, Sys); `thread` wrote it.
    WatchSys { watched: u64, thread: u32 },
    /// The recording's end (exit, crash, fatal signal), parked at (T, K_f, Watch) (M41 R18).
    Terminal(ReplayReport),
    /// `reverse-continue` found nothing before the cursor, which did not move.
    NoEarlierHit,
    /// A step completed.
    Stepped,
    /// `reverse-stepi` stopped at (1, 0).
    AtStart,
    /// Nothing moved: `stepi`'s window end or fault, with its text.
    Refused(String),
}
```

(`WatchStepped` is Task 4's.)

- [ ] **Step 1: The failing tests.** First, add to `crates/retrace/tests/util/mod.rs`:

```rust
/// M43: a copy of `trace` whose last `write` claims one byte more than was written, so replay's
/// divergence oracle fails at that landmark. Every record is re-framed with a fresh CRC by
/// `Writer`, so the trace is well-formed and only its content lies.
pub fn tamper_last_write(trace: &std::path::Path) -> std::path::PathBuf {
    let mut ev = retrace_trace::Reader::open(trace).unwrap();
    let i = ev.iter().rposition(|e| matches!(e, retrace_trace::Event::Syscall { num: 4, .. }))
        .expect("the trace has a write");
    if let retrace_trace::Event::Syscall { args, .. } = &mut ev[i] { args[2] += 1; }
    let out = trace.with_extension("tampered.bin");
    let mut w = retrace_trace::Writer::create(&out).unwrap();
    for e in &ev { w.append(e).unwrap(); }
    out
}
```

Then add the continue rows to `gdbserver_e2e.rs`. Each one names the spec row it pins:

```rust
/// `&buf[40]`: the address watchsweep publishes in its write(1, …) (the watchsweep_e2e oracle).
fn ws_target() -> u64 {
    let mut s = retrace_core::ReplaySession::open(watchsweep()).unwrap();
    loop {
        if let Some((4, args)) = s.peek_syscall() { if args[0] == 1 { return args[1]; } }
        s.advance().unwrap();
    }
}
fn pc_of(stop: &str) -> u64 { r::le_u64(r::key(stop, "20").expect("an expedited pc")) }
fn mem_u64(c: &mut Rsp, a: u64) -> u64 { r::le_u64(&c.send(&format!("m{a:x},8"))) }

#[test]
fn a_forward_watch_is_reported_after_its_store_and_a_reverse_one_before_it() {
    // §3c rows 2 and 5. watchsweep writes buf[40] twice: the sweeping store (0x1111…+40), then
    // a second store (0xbeef).
    let t = ws_target();
    let mut c = Rsp::spawn(watchsweep(), &[]);
    assert_eq!(c.send(&format!("Z2,{t:x},8")), "OK");
    let s1 = c.send("c");
    assert_eq!(r::key(&s1, "watch"), Some(format!("{t:x}").as_str()), "{s1}");
    assert_eq!(mem_u64(&mut c, t), 0x1111_1111_1111_1111 + 40, "post-retire: the new value");
    let sweep_after = pc_of(&s1);
    let s2 = c.send("c");
    assert_eq!(mem_u64(&mut c, t), 0xbeef, "{s2}");
    let second_after = pc_of(&s2);
    // Backward: the second store, pre-retire, with the old value in memory.
    let b1 = c.send("bc");
    assert_eq!(r::key(&b1, "watch"), Some(format!("{t:x}").as_str()), "{b1}");
    assert_eq!(pc_of(&b1), second_after - 4, "before the store");
    assert_eq!(mem_u64(&mut c, t), 0x1111_1111_1111_1111 + 40);
    // §3c row 5's point: from a reverse stop, forward reports that same store again (control C2).
    let f = c.send("c");
    assert_eq!(pc_of(&f), second_after, "{f}");
    assert_eq!(mem_u64(&mut c, t), 0xbeef);
    let b2 = c.send("bc");
    assert_eq!(pc_of(&b2), second_after - 4);
    let b3 = c.send("bc");
    assert_eq!(pc_of(&b3), sweep_after - 4, "the sweeping store, before it wrote buf[40]");
    assert_eq!(mem_u64(&mut c, t), 0);
}

#[test]
fn no_earlier_hit_goes_to_the_start_and_the_end_of_recording_is_reversible() {
    // §3c rows 7 and 8, and the exit terminal.
    let mut c = Rsp::spawn(watchsweep(), &[]);
    let entry = retrace_core::seek(watchsweep(), 1, 0).unwrap().pc();
    let end = c.send("c");
    assert!(end.starts_with("T05") && end.contains("replaylog:end;"), "{end}");
    assert_eq!(r::description(&end).as_deref(), Some("exited (code 0)"));
    assert_eq!(c.send("c"), end, "continue at the end reports the end again");
    let back = c.send("bc");
    assert_eq!(r::description(&back).as_deref(), Some("start of recording"), "{back}");
    assert_eq!(pc_of(&back), entry);
    assert!(c.where_().starts_with("at (1, 0) phase=Bp"));
}

#[test]
fn a_syscall_write_is_reported_after_the_syscall_forward_and_at_its_trap_backward() {
    // §3c rows 3 and 6, on crashy's fstat(1, &g.st).
    let (st, _ptr) = util::discover_crashy_addrs(crashy());
    let mut c = Rsp::spawn(crashy(), &[]);
    assert_eq!(c.send(&format!("Z2,{st:x},8")), "OK");
    let f = c.send("c");
    assert_eq!(r::key(&f, "watch"), Some(format!("{st:x}").as_str()), "{f}");
    let written = mem_u64(&mut c, st);
    assert_ne!(written, 0, "after the syscall: its write is in memory");
    let b = c.send("bc");
    assert_eq!(r::key(&b, "watch"), Some(format!("{st:x}").as_str()), "{b}");
    let pc = pc_of(&b);
    assert_eq!(r::le_u64(&c.send(&format!("m{pc:x},4"))) as u32, 0xd400_1001, "parked at the svc #0x80");
    assert_eq!(mem_u64(&mut c, st), 0, "before the syscall: g.st is still BSS");
    let again = c.send("c");
    assert_eq!(again, f, "forward from the trap crosses it and reports the same write");
}

#[test]
fn the_crash_is_exc_bad_access_and_reverse_reaches_the_corrupting_store() {
    // The headline, without lldb: §3c's terminal and row 5 on crashy.
    const GARBAGE_VA: u64 = 0x4000_DEAD_0000;
    let (_st, ptr) = util::discover_crashy_addrs(crashy());
    let crash_pc = retrace_trace::Reader::open(crashy()).unwrap().iter().find_map(|e| match e {
        retrace_trace::Event::Crash { pc, .. } => Some(*pc), _ => None }).unwrap();
    let mut c = Rsp::spawn(crashy(), &[]);
    let crash = c.send("c");
    assert!(crash.starts_with("T0b"), "{crash}");
    assert!(crash.contains(&format!("metype:1;mecount:2;medata:1;medata:{GARBAGE_VA:x};")), "{crash}");
    assert_eq!(pc_of(&crash), crash_pc);
    assert_eq!(c.send(&format!("Z2,{ptr:x},8")), "OK");
    let b = c.send("bc");
    assert_eq!(r::key(&b, "watch"), Some(format!("{ptr:x}").as_str()), "{b}");
    assert_eq!(mem_u64(&mut c, ptr), ptr - 32, "before the store: g.ptr is still &g.buf[0]");
    let f = c.send("c");
    assert_eq!(mem_u64(&mut c, ptr), GARBAGE_VA, "after it: the garbage");
    assert_eq!(pc_of(&f), pc_of(&b) + 4);
    assert_eq!(c.send("c"), crash, "then the crash again");
}

#[test]
fn breakpoints_cap_at_five_and_reinsertion_is_idempotent() {
    // §3f, spec R4, Review Focus 5.
    let mut c = Rsp::spawn(watchsweep(), &[]);
    let base = retrace_core::seek(watchsweep(), 1, 0).unwrap().pc();
    for i in 0..5u64 { assert_eq!(c.send(&format!("Z0,{:x},4", base + 4 * i)), "OK"); }
    assert_eq!(c.send(&format!("Z0,{base:x},4")), "OK", "a duplicate is not a sixth");
    assert_eq!(c.send(&format!("Z1,{:x},4", base + 40)), "E01", "the sixth is refused: lldb's step keeps a slot");
    assert_eq!(c.send(&format!("z0,{base:x},4")), "OK");
    assert_eq!(c.send(&format!("z0,{base:x},4")), "OK", "removing an absent one is OK");
    assert_eq!(c.send(&format!("Z0,{:x},4", base + 40)), "OK", "the slot is free again");
}

#[test]
fn watchpoints_are_write_only_and_cap_at_four() {
    let t = ws_target();
    let mut c = Rsp::spawn(watchsweep(), &[]);
    assert_eq!(c.send(&format!("Z3,{t:x},8")), "E01", "read");
    assert_eq!(c.send(&format!("Z4,{t:x},8")), "E01", "access");
    assert_eq!(c.send(&format!("Z2,{:x},8", t + 1)), "E01", "misaligned");
    for i in 0..4u64 { assert_eq!(c.send(&format!("Z2,{:x},8", t + 8 * i)), "OK"); }
    assert_eq!(c.send(&format!("Z2,{t:x},8")), "OK", "a duplicate is not a fifth");
    assert_eq!(c.send(&format!("Z2,{:x},8", t + 64)), "E01", "the fifth");
}

#[test]
fn a_divergence_is_a_stop_at_the_saved_cursor_never_an_error_reply() {
    // §3b and M41's owed `?`-armed session: the scan diverges mid-flight with a breakpoint armed. The
    // reply is an exception stop (an `E` would drop lldb, t0 L7), and the cursor is where it was.
    let bad = util::tamper_last_write(watchsweep());
    let exit_svc = retrace_core::seek(watchsweep(), 2, 0).unwrap().pc() + 8; // mov x0; mov x16; svc
    let mut c = Rsp::spawn(&bad, &[]);
    assert_eq!(c.send(&format!("Z0,{exit_svc:x},4")), "OK");
    let before = c.where_();
    let s = c.send("c");
    assert!(s.starts_with("T05") && s.contains("reason:exception;"), "{s}");
    assert!(r::description(&s).unwrap().contains("diverged"), "{s}");
    assert_eq!(c.where_(), before, "recovered to the saved cursor");
    assert_eq!(c.send("c"), s, "deterministic: the same divergence again");
}
```

`exit_svc` assumes window 2 starts at watchsweep's `mov x0, #0` after the write. Check it against
`watchsweep.s`; if the layout differs, compute the pc from `nm` (as `llsc_e2e`'s `sym` does) instead.

And the unit tests at the end of `debug.rs`'s `mod tests`:

```rust
    #[test] fn each_motion_returns_what_it_printed() {
        // M43 §3b: the Halt is the printed line's structured twin.
        let trace = record_watchsweep("halts");
        let t = watchsweep_target(&trace);
        let mut ex = Exec::new(&trace).unwrap();
        let mut sink = Vec::new();
        assert!(matches!(ex.cmd_reverse_stepi(1, &mut sink).unwrap(), Halt::AtStart));
        assert!(matches!(ex.cmd_stepi(3, &mut sink).unwrap(), Halt::Stepped));
        assert!(matches!(ex.cmd_reverse_continue(&mut sink).unwrap(), Halt::NoEarlierHit));
        ex.cmd_watch(t, 8, None, &mut sink).unwrap();
        assert!(matches!(ex.cmd_continue(&mut sink).unwrap(), Halt::Watch { watched } if watched == t));
        assert!(matches!(ex.cmd_continue(&mut sink).unwrap(), Halt::Watch { watched } if watched == t));
        assert!(matches!(ex.cmd_continue(&mut sink).unwrap(),
            Halt::Terminal(ref r) if r.outcome == retrace_core::Outcome::Exit { code: 0 }));
        assert!(matches!(ex.cmd_reverse_continue(&mut sink).unwrap(), Halt::Watch { .. }));
        let text = String::from_utf8_lossy(&sink).into_owned();
        assert_eq!(text.matches("hit watch").count(), 3, "{text}");
        assert!(text.contains("no earlier hit") && text.contains("at start of recording"), "{text}");
    }

    #[test] fn recover_leaves_a_usable_session_at_the_saved_cursor() {
        // M43 §3b, M41's owed item: a scan that diverges with breakpoints armed leaves the session
        // armed AND moved (t0 R2, debug.rs's scan `advance()?`). `recover` undoes both, because it
        // replaces the session with a fresh `reseek`, which is breakpoint-clean by construction.
        // What this pins is the observable half: the position is back, and the next motion works.
        let good = record_watchsweep("recover");
        let mut ev = retrace_trace::Reader::open(&good).unwrap();
        let i = ev.iter().rposition(|e| matches!(e, retrace_trace::Event::Syscall { num: 4, .. })).unwrap();
        if let retrace_trace::Event::Syscall { args, .. } = &mut ev[i] { args[2] += 1; }
        let bad = good.with_extension("tampered.bin");
        let mut w = retrace_trace::Writer::create(&bad).unwrap();
        for e in &ev { w.append(e).unwrap(); }
        drop(w);
        // In window 2 (exit's `svc`), which the scan never reaches: window 1's write diverges first.
        // Taken before `Exec::new`, whose session is this process's one VM from then on.
        let exit_svc = retrace_core::seek(&good, 2, 0).unwrap().pc() + 8;
        let mut ex = Exec::new(&bad).unwrap();
        let mut sink = Vec::new();
        let pc0 = ex.sess().pc();
        ex.cmd_break(exit_svc, &mut sink).unwrap();
        let at = ex.cursor();
        let err = ex.cmd_continue(&mut sink).unwrap_err();
        assert!(err.contains("diverged"), "{err}");
        assert_ne!(ex.sess().pc(), pc0, "the failed scan left its session moved");
        ex.recover(at).unwrap();
        assert_eq!(ex.cursor(), at);
        assert_eq!(ex.sess().pc(), pc0, "the session is back at the saved cursor");
        assert!(matches!(ex.cmd_stepi(1, &mut sink).unwrap(), Halt::Stepped));
        assert_eq!(ex.cursor(), (1, 1, Phase::Bp));
    }
```

watchsweep has two landmarks: window 1 ends in its `write` (the one the tamper corrupts), and
window 2 is `mov x0, #0; mov x16, #1; svc`, so `(2, 0)` + 8 is the exit `svc`.

- [ ] **Step 2: See them fail.** With `util::tamper_last_write` written (it is test support, not
  the code under test), run:

```sh
cargo test -p retrace --test gdbserver_e2e --no-fail-fast -- --test-threads=1 > $L/t3-red.log 2>&1; echo "exit=$?"
grep -a -E "^test " $L/t3-red.log
```

Expected: Task 2's 7 rows pass. The 7 new rows fail: `c`, `bc` and `Z*` answer `""`, so the first
stop assertions fail. `debug.rs`'s new tests do not compile yet (no `Halt`), which is expected.

- [ ] **Step 3: `Halt` through `Exec`.** In `debug.rs`:
  - Add `Halt` (above), and `use retrace_core::ReplayReport;` (it is already reachable as
    `retrace_core::ReplayReport`).
  - `park_at_terminal` returns `Result<Halt, String>`: `match &report.outcome` (by reference, so
    `report` survives), and at the end `Ok(Halt::Terminal(report))`.
  - `cmd_continue` returns `Result<Halt, String>`. Its return sites, top to bottom:
    - the Sys-then-bp report `return Ok(())` → `return Ok(Halt::Break)`;
    - the finish's watch report → `return Ok(Halt::Watch { watched })`;
    - both `Advance::Exited(report) => return self.park_at_terminal(report, out)` stay as written
      (they now return the `Halt`);
    - the finish's `WatchSyscall` report → `return Ok(Halt::WatchSys { watched, thread })`;
    - the scan's `Advance::Break` `return self.reseek(n, k);` →
      `self.reseek(n, k)?; return Ok(Halt::Break);` (phase Bp);
    - the scan's boundary-breakpoint `return Ok(())` → `return Ok(Halt::Break)`;
    - the scan's matched watch `return Ok(())` → `return Ok(Halt::Watch { watched })`;
    - the scan's matched `WatchSyscall` `return Ok(())` → `return Ok(Halt::WatchSys { watched, thread })`.
  - `cmd_reverse_continue` returns `Result<Halt, String>`. `RHit::WatchSys` gains `thread: u32`,
    filled from `Advance::WatchSyscall { watched, thread }`. The four arms become:
    - `Bp`: `self.reseek(n, k)?; Ok(Halt::Break)`;
    - `Watch`: `…; self.phase = Phase::Watch; Ok(Halt::Watch { watched })`;
    - `WatchSys`: `…; self.phase = Phase::Sys; Ok(Halt::WatchSys { watched, thread })`;
    - `None`: `…; self.phase = pphase; Ok(Halt::NoEarlierHit)`.
  - `cmd_stepi`: `Ok(()) => { …; Ok(Halt::Stepped) }`, and the Err arm ends
    `Ok(Halt::Refused(head.to_string()))` after its re-seek.
  - `cmd_reverse_stepi`: `Ok(if at_start { Halt::AtStart } else { Halt::Stepped })`.
  - `exec`: map each motion's result to `()` (`.map(|_| ())`). The script prints exactly what it
    printed.
  - Make the eight `cmd_*` motions and edits named in **Files** `pub(crate)`.
  - Add:

```rust
    /// M43 §3b: re-seek to a saved cursor, restoring its phase. `reseek` drops whatever session
    /// exists (armed, moved, or none, t0 R2) and seeks a fresh, breakpoint-clean one, so one call
    /// undoes every state an `Err` can leave. The gdb-remote server calls it after every failed
    /// motion. The script CLI does not, because an `Err` ends the script.
    pub(crate) fn recover(&mut self, at: (usize, u64, Phase)) -> Result<(), String> {
        self.reseek(at.0, at.1)?;
        self.phase = at.2;
        Ok(())
    }

    /// M43 §3c: set the cursor's phase without moving (a reverse store watch becomes `Bp`).
    pub(crate) fn set_phase(&mut self, phase: Phase) { self.phase = phase; }

    /// M43 §3c: park just before the event that ends window `n − 1`, at its trap, which is where a
    /// reverse syscall-watch stop belongs: before the write. `n ≥ 2` always (landmark 1's write
    /// lands at (2, 0)).
    pub(crate) fn park_before_event(&mut self, n: usize) -> Result<(), String> {
        debug_assert!(n >= 2, "a syscall write at ({n}, 0) has no event before it");
        let len = self.probe_window_len(n - 1)?;
        self.reseek(n - 1, len) // phase Bp: an arrival
    }

    /// M43 §3f: the armed breakpoints and watches, for the server's slot accounting.
    pub(crate) fn breakpoints(&self) -> &[u64] { &self.breakpoints }
    pub(crate) fn watches(&self) -> impl Iterator<Item = (u64, u64)> + '_ { self.watches.iter().map(|&(a, l, _)| (a, l)) }
```

- [ ] **Step 4: The new stop kinds.** In `rsp.rs`, add these to `StopKind`, with their `signal()`
  and `keys()` arms (t0 L4a, L4c, L4d, L4e):

```rust
    /// `reason:breakpoint` (t0 L4a).
    Breakpoint,
    /// `watch:<addr>`: lldb then reads memory NOW, which §3c's positions make right (t0 L4c).
    Watch(u64),
    /// `replaylog:end` with a description: the recording's end, reversible (t0 L4e).
    HistoryEnd(String),
    /// A Mach `EXC_BAD_ACCESS` (`metype:1`): lldb's native crash display, reversible (t0 L4d).
    MachBadAccess { code: u64, far: u64 },
    /// `reason:exception` with a description. Reversible, unlike a plain signal stop (t0 L4d).
    Exception { signal: u8, text: String },
```

- `signal()`: `Breakpoint | Watch(_) | HistoryEnd(_) => 5`, `MachBadAccess { .. } => 0x0b`,
  `Exception { signal, .. } => *signal`.
- `keys()`:
  - `Breakpoint` → `"reason:breakpoint;"`;
  - `Watch(a)` → `format!("watch:{a:x};")`;
  - `HistoryEnd(d)` → `format!("replaylog:end;description:{};", hex(d.as_bytes()))`;
  - `MachBadAccess { code, far }` → `format!("metype:1;mecount:2;medata:{code:x};medata:{far:x};")`;
  - `Exception { text, .. }` → `format!("reason:exception;description:{};", hex(text.as_bytes()))`.

Add one unit test to `rsp.rs`'s `mod tests`, pinning each new kind's signal and exact tail:

```rust
    #[test] fn every_stop_kind_carries_its_measured_signal_and_keys() {
        // The key shapes are t0's (L4a, L4c, L4d, L4e); lldb parses them, so they are pinned byte
        // for byte.
        let ctx = ThreadCtx::zeroed();
        let reply = |k: StopKind| stop_reply(1, &ctx, &[(1, 0)], &k);
        let bp = reply(StopKind::Breakpoint);
        assert!(bp.starts_with("T05") && bp.ends_with("reason:breakpoint;"), "{bp}");
        let w = reply(StopKind::Watch(0x1_0000_4140));
        assert!(w.starts_with("T05") && w.ends_with("watch:100004140;"), "{w}");
        let end = reply(StopKind::HistoryEnd("exited (code 0)".into()));
        assert!(end.starts_with("T05")
            && end.ends_with(&format!("replaylog:end;description:{};", hex(b"exited (code 0)"))), "{end}");
        let crash = reply(StopKind::MachBadAccess { code: 1, far: 0x4000_dead_0000 });
        assert!(crash.starts_with("T0b")
            && crash.ends_with("metype:1;mecount:2;medata:1;medata:4000dead0000;"), "{crash}");
        let exc = reply(StopKind::Exception { signal: 6, text: "guest terminated by signal 6".into() });
        assert!(exc.starts_with("T06")
            && exc.ends_with(&format!("reason:exception;description:{};", hex(b"guest terminated by signal 6"))), "{exc}");
    }
```

- [ ] **Step 5: Motion in the server.** In `gdbserver.rs`:

```rust
    /// §3b: one motion, bracketed. An `Err` re-seeks the saved cursor and becomes a non-moving
    /// exception stop: never an `E`, which drops lldb's connection (t0 L7). If the re-seek fails as
    /// well, the server has no session left, and `handle_dead` answers from then on.
    fn motion(&mut self, f: impl FnOnce(&mut Self) -> Result<String, String>) -> String {
        let at = self.ex.cursor();
        match f(self) {
            Ok(reply) => reply,
            Err(e) => match self.ex.recover(at) {
                Ok(()) => self.stop(StopKind::Exception { signal: 5, text: e }, None),
                Err(e2) => {
                    let d = format!("{e}; and re-seeking the cursor failed: {e2}");
                    let r = dead_stop(&d);
                    self.dead = Some(d);
                    self.last_stop = r.clone();
                    r
                }
            },
        }
    }

    /// §3c: forward. A store watch is reported AFTER the store retires, so the server steps it.
    fn reply_forward(&mut self, h: Halt) -> Result<String, String> {
        Ok(match h {
            Halt::Break => self.stop(StopKind::Breakpoint, None),
            Halt::Watch { watched } => match self.ex.cmd_stepi(1, &mut std::io::sink())? {
                Halt::Stepped => self.stop(StopKind::Watch(watched), None),
                // The store did not retire (it faults). Say so, at the store; the cursor stays ON its
                // watch, so the next `c` crosses to the fault through Exec's own finish.
                Halt::Refused(why) => self.stop(StopKind::Exception { signal: 5,
                    text: format!("the watched store at {:#x} did not retire: {why}", self.ex.sess().pc()) }, None),
                other => return Err(format!("stepping a watched store reported {other:?}")),
            },
            Halt::WatchSys { watched, thread } => {
                // An arrival at (n, 0), so a reverse `c` finds this write again (§3c, the forward
                // syscall-watch row).
                self.ex.set_phase(Phase::Bp);
                self.stop(StopKind::Watch(watched), Some(thread))
            }
            Halt::Terminal(r) => self.terminal(&r.outcome),
            other => return Err(format!("a forward motion reported {other:?}")),
        })
    }

    /// §3c: backward. Every stop is reported BEFORE its crossing.
    fn reply_backward(&mut self, h: Halt) -> Result<String, String> {
        Ok(match h {
            Halt::Break => self.stop(StopKind::Breakpoint, None),
            Halt::Watch { watched } => {
                // Before the store, as an arrival: a forward `c` re-reports it.
                self.ex.set_phase(Phase::Bp);
                self.stop(StopKind::Watch(watched), None)
            }
            Halt::WatchSys { watched, .. } => {
                // Before the syscall: its trap, with the old value in memory.
                let (n, _, _) = self.ex.cursor();
                self.ex.park_before_event(n)?;
                self.stop(StopKind::Watch(watched), None)
            }
            Halt::NoEarlierHit => {
                self.ex.recover((1, 0, Phase::Bp))?;
                self.stop(StopKind::HistoryBegin("start of recording".into()), None)
            }
            other => return Err(format!("a backward motion reported {other:?}")),
        })
    }

    /// §3c's terminal list (t0 L4d, L4e).
    fn terminal(&mut self, o: &Outcome) -> String {
        let kind = match *o {
            Outcome::Exit { code } => StopKind::HistoryEnd(format!("exited (code {code})")),
            Outcome::Crash { pc, esr, far } => match (esr >> 26) & 0x3f {
                0x20 | 0x21 | 0x24 | 0x25 => StopKind::MachBadAccess {
                    code: if (0x0c..=0x0f).contains(&(esr & 0x3f)) { 2 } else { 1 }, far },
                _ => StopKind::Exception { signal: 0x0b,
                    text: format!("guest crashed: pc={pc:#x} far={far:#x} esr={esr:#x}") },
            },
            Outcome::Signal { sig } => StopKind::Exception { signal: sig as u8,
                text: format!("guest terminated by signal {sig}") },
        };
        self.stop(kind, None)
    }
```

Two free functions, and one method. None of them reads a session, because a dead server has none:

```rust
/// A stop that needs no session: `T05` on RSP thread 1, with the reason.
fn dead_stop(d: &str) -> String {
    format!("T05thread:1;reason:exception;description:{};", rsp::hex(d.as_bytes()))
}

/// Every packet that resumes the guest (§3h's resume forms).
fn is_resume(body: &str) -> bool {
    matches!(body, "c" | "s" | "bc" | "bs") || body.starts_with("vCont;") || body.starts_with('C') || body.starts_with('S')
}

impl Server<'_> {
    /// §3b: after a failed re-seek there is no session. Every resume repeats why, `?` repeats the
    /// last stop, `k` and `D` still end the session, and everything else is refused.
    fn handle_dead(&self, body: &str, why: &str) -> (Vec<String>, bool) {
        match body {
            "k" => (vec!["X09".into()], true),
            _ if body == "D" || body.starts_with("D;") => (vec!["OK".into()], true),
            "?" => (vec![self.last_stop.clone()], false),
            _ if is_resume(body) => (vec![dead_stop(why)], false),
            _ => (vec!["E01".into()], false),
        }
    }
}
```

Add a field `dead: Option<String>` to `Server` (`None` in `new`), and imports `retrace_core::Outcome`
and `crate::debug::{Halt, Phase}`. In `handle`, right after the `;thread:` suffix split, add:

```rust
        if let Some(why) = &self.dead { return self.handle_dead(body, why); }
```

Then add these arms to `handle` **before** the catch-all:

```rust
            // §3h: every continue form. A signal to deliver is ignored: a recording's signals are its own.
            "c" | "vCont;c" => (vec![self.motion(|s| { let h = s.ex.cmd_continue(&mut std::io::sink())?; s.reply_forward(h) })], false),
            _ if body.starts_with('C') || body.starts_with("vCont;C") || body.starts_with("vCont;c:") =>
                (vec![self.motion(|s| { let h = s.ex.cmd_continue(&mut std::io::sink())?; s.reply_forward(h) })], false),
            "bc" => (vec![self.motion(|s| { let h = s.ex.cmd_reverse_continue(&mut std::io::sink())?; s.reply_backward(h) })], false),
            _ if body.starts_with('Z') || body.starts_with('z') => one(&self.z_packet(body)),
```

`z_packet`:

```rust
    /// §3f. `Z<t>,<addr>,<kind|len>` / `z…`. 0 and 1 are hardware breakpoints, capped at 5 so lldb's
    /// transient step breakpoint always has the sixth slot (spec R4, t0 L5). 2 is a write watch,
    /// capped at 4. 3 and 4 (read, access) are refused. Re-inserting what is there, or removing what
    /// is not, is `OK` (Review Focus 5).
    fn z_packet(&mut self, body: &str) -> String {
        let insert = body.starts_with('Z');
        let mut f = body[1..].split(',');
        let (Some(t), Some(a), Some(l)) = (f.next(), f.next(), f.next()) else { return "E01".into() };
        let (Ok(addr), Ok(len)) = (u64::from_str_radix(a, 16), u64::from_str_radix(l, 16)) else { return "E01".into() };
        let sink = &mut std::io::sink();
        match (t, insert) {
            ("0" | "1", true) => {
                if self.ex.breakpoints().contains(&addr) { return "OK".into(); }
                if self.ex.breakpoints().len() >= 5 { return "E01".into(); }
                if self.ex.cmd_break(addr, sink).is_ok() { "OK" } else { "E01" }.into()
            }
            ("0" | "1", false) => { let _ = self.ex.cmd_delete(addr, sink); "OK".into() }
            ("2", true) => {
                if self.ex.watches().any(|(wa, wl)| wa == addr && wl == len) { return "OK".into(); }
                if !matches!(len, 1 | 2 | 4 | 8) || addr % len != 0 { return "E01".into(); }
                if self.ex.cmd_watch(addr, len, None, sink).is_ok() { "OK" } else { "E01" }.into()
            }
            ("2", false) => { let _ = self.ex.cmd_unwatch(addr, sink); "OK".into() }
            (_, true) => "E01".into(),
            (_, false) => "OK".into(),
        }
    }
```

`cmd_break` and `cmd_delete` take an address, not an `Operand`: call them directly. `cmd_watch`'s
own 4-slot check makes the fifth `E01`.

- [ ] **Step 6: Green.**

```sh
cargo test -p retrace --bins --no-fail-fast -- --test-threads=1 > $L/t3-bins.log 2>&1; echo "exit=$?"
cargo test -p retrace --test gdbserver_e2e --no-fail-fast -- --test-threads=1 > $L/t3-e2e.log 2>&1; echo "exit=$?"
cargo test -p retrace --test debug_cli --test crashy_cli --test watch_cli --test watchsweep_e2e --test hitorder_e2e --test reverse_debug_e2e --test thread_watch_e2e --no-fail-fast -- --test-threads=1 > $L/t3-cli.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets -- -D warnings > $L/t3-clippy.log 2>&1; echo "exit=$?"
grep -a -E "^test result|FAILED" $L/t3-*.log
```

Expected:
- `--bins`: 24 + 3 = 27;
- `gdbserver_e2e`: 14;
- the script-debugger suites green and unchanged;
- clippy clean.

- [ ] **Step 7: Commit.**

```sh
git add crates/retrace/src/debug.rs crates/retrace/src/rsp.rs crates/retrace/src/gdbserver.rs crates/retrace/tests/util/mod.rs crates/retrace/tests/gdbserver_e2e.rs
git commit -m "M43 t3: continue both ways over RSP — Exec's Halt, §3c's positions, breakpoints, watches, recovery"
```

- [ ] **Step 8: Controls (committed tree; restore each with `git checkout -- crates/retrace/src/gdbserver.rs`).**
  - **C1:** in `reply_forward`, replace the `Halt::Watch` arm's body with
    `self.stop(StopKind::Watch(watched), None)`, so there is no step. Expected RED in
    `a_forward_watch_is_reported_after…`: memory still holds the old value at the first forward
    stop.
  - **C2:** in `reply_backward`, delete `self.ex.set_phase(Phase::Bp);`. Expected RED: the forward
    `c` after `b1` skips the second store (`pc_of(&f) != second_after`).
  - **C3:** in `motion`'s `Err` arm, return `"E01".to_string()`. Expected RED in
    `a_divergence_is_a_stop…`.

```sh
cargo test -p retrace --test gdbserver_e2e --no-fail-fast -- --test-threads=1 > $L/t3-c1.log 2>&1; echo "exit=$?"
```

Run the same command for each control, with its own log. Record each symptom.

---

### Task 4: Stepping (spec §3d, §3e)

lldb's `s` must cross a trap, run through a blocking syscall until the stepped thread runs again,
and never answer `trace` without moving the stepped thread's pc. `bs` and the armed `bc` step
backward.

**Files:**
- Modify: `crates/retrace/src/debug.rs`:
  - `Halt::WatchStepped`;
  - `step_thread`;
  - `cmd_continue` becomes `continue_until(until: Option<u32>, …)` plus a one-line `cmd_continue`;
  - one unit test.
- Modify: `crates/retrace/src/rsp.rs` (`StopKind::Trace`)
- Modify: `crates/retrace/src/gdbserver.rs` (`s`/`S`/`vCont;s`/`vCont;S`, `bs`, `arm-rsi`, the armed
  `bc`, `Hc`)
- Test: `crates/retrace/tests/gdbserver_e2e.rs` (the step rows)

**Interfaces:**
- Consumes: Task 3's `Halt`, `Server::motion`, `Server::reply_forward`, `Exec::recover`.
- Produces: `Exec::step_thread(&mut self, t: u32, out) -> Result<Halt, String>`;
  `Halt::WatchStepped { watched: u64 }`; `StopKind::Trace`.

- [ ] **Step 1: The failing tests.** Add to `gdbserver_e2e.rs`:

```rust
/// The threadrust recording, and the first `__ulock_wait` (515) whose NEXT landmark runs another
/// thread: a wait that really blocked. Returns (trace, landmark n of the wait, its thread).
fn threadrust_block() -> (&'static Path, usize, u32) {
    static C: OnceLock<(PathBuf, usize, u32)> = OnceLock::new();
    let (p, n, t) = C.get_or_init(|| {
        let (rec, tr) = util::record_dynamic(retrace_guest::THREADRUST);
        assert_eq!(rec.code, 0, "record threadrust: {}", rec.stderr);
        let ev = retrace_trace::Reader::open(&tr).unwrap();
        let thread_of = |e: &retrace_trace::Event| match e {
            retrace_trace::Event::Syscall { thread, .. } => Some(*thread), _ => None };
        let n = (1..ev.len() - 1).find(|&i| matches!(ev[i], retrace_trace::Event::Syscall { num: 515, .. })
            && thread_of(&ev[i + 1]).is_some() && thread_of(&ev[i + 1]) != thread_of(&ev[i]))
            .expect("a __ulock_wait that blocked");
        let t = thread_of(&ev[n]).unwrap();
        (tr, n, t)
    });
    (p.as_path(), *n, *t)
}

/// The pc of the trap that ends window `n` (landmark n's svc): seek the window's full length.
fn trap_pc(trace: &Path, n: usize) -> u64 {
    let len = retrace_core::seek(trace, n, 0).unwrap().window_len_here().unwrap();
    retrace_core::seek(trace, n, len).unwrap().pc()
}

/// With a breakpoint on landmark `n`'s svc, `c` until the cursor stands in window `n`, and return
/// that stop. The svc is libsystem_kernel's, and every `__ulock_wait` runs it, so earlier waits stop
/// there first.
fn continue_to_window(c: &mut Rsp, n: usize) -> String {
    for _ in 0..1000 {
        let s = c.send("c");
        if c.where_().starts_with(&format!("at ({n}, ")) { return s; }
        assert!(!s.contains("replaylog:end;") && !s.starts_with("T0b"), "ran past landmark {n}: {s}");
    }
    panic!("landmark {n} not reached in 1000 stops");
}

#[test]
fn a_step_crosses_a_syscall_and_lands_after_it() {
    // §3d rule 3, AtTrap: `si` on an `svc` is ordinary. watchsweep's window 1 ends at its write.
    let svc = trap_pc(watchsweep(), 1);
    let mut c = Rsp::spawn(watchsweep(), &[]);
    assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
    let at = c.send("c");
    assert_eq!(pc_of(&at), svc, "{at}");
    assert_eq!(c.send(&format!("z0,{svc:x},4")), "OK");
    let s = c.send("vCont;s:1");
    assert!(s.contains("reason:trace;"), "{s}");
    assert_eq!(pc_of(&s), svc + 4, "the syscall returned");
    assert!(c.where_().starts_with("at (2, 0) phase=Bp"), "{}", c.where_());
}

#[test]
fn a_step_over_a_blocking_syscall_ends_when_the_stepped_thread_runs_again() {
    // §3d, the until-thread run (control C4). The wait blocks, other threads run, and the step
    // ends on the stepped thread at its svc + 4, some landmarks later. Answering on another thread
    // would loop lldb forever (t0 L7, 349,194 steps in 60 s).
    let (tr, n, t) = threadrust_block();
    let svc = trap_pc(tr, n);
    let mut c = Rsp::spawn(tr, &[]);
    assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
    let at = continue_to_window(&mut c, n);
    assert_eq!(r::key(&at, "thread"), Some(format!("{:x}", t + 1).as_str()), "{at}");
    assert_eq!(pc_of(&at), svc);
    assert_eq!(c.send(&format!("z0,{svc:x},4")), "OK");
    let s = c.send(&format!("vCont;s:{:x}", t + 1));
    assert!(s.contains("reason:trace;"), "{s}");
    assert_eq!(r::key(&s, "thread"), Some(format!("{:x}", t + 1).as_str()), "on the stepped thread: {s}");
    assert_eq!(pc_of(&s), svc + 4);
    let w = c.where_();
    let landed: usize = w["at (".len()..].split(',').next().unwrap().parse().unwrap();
    assert!(landed > n + 1, "other threads ran in between: {w}");
}

#[test]
fn a_step_on_a_thread_that_is_not_running_is_refused_in_place() {
    // §3d rule 1: a non-moving exception stop, measured safe (t0 L7).
    let (tr, n, t) = threadrust_block();
    let svc = trap_pc(tr, n);
    let mut c = Rsp::spawn(tr, &[]);
    assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
    continue_to_window(&mut c, n);
    let other = if t == 0 { 2 } else { 1 }; // an RSP tid that is not t + 1
    let before = c.where_();
    let s = c.send(&format!("vCont;s:{other:x}"));
    assert!(s.contains("reason:exception;"), "{s}");
    assert!(r::description(&s).unwrap().contains("cannot step thread"), "{s}");
    assert_eq!(c.where_(), before);
}

#[test]
fn a_reverse_step_moves_back_one_and_stops_at_the_start() {
    // §3e: `bs`, and `bc` armed by `qRcmd arm-rsi` (what `rsi` sends).
    let mut c = Rsp::spawn(watchsweep(), &[]);
    let entry = retrace_core::seek(watchsweep(), 1, 0).unwrap().pc();
    for _ in 0..3 { assert!(c.send("s").contains("reason:trace;")); }
    let b = c.send("bs");
    assert!(b.contains("reason:trace;"), "{b}");
    assert_eq!(pc_of(&b), entry + 8);
    let hexcmd: String = "arm-rsi".bytes().map(|x| format!("{x:02x}")).collect();
    assert_eq!(c.send_collect(&format!("qRcmd,{hexcmd}")).1, "OK");
    let a = c.send("bc");
    assert!(a.contains("reason:trace;"), "an armed bc is one step back: {a}");
    assert_eq!(pc_of(&a), entry + 4);
    assert!(c.send("bs").contains("reason:trace;"));
    let start = c.send("bs");
    assert_eq!(r::description(&start).as_deref(), Some("start of recording"), "{start}");
    assert_eq!(c.send("bc"), start, "unarmed again: bc with nothing armed runs to the start");
}

#[test]
fn a_step_at_the_end_of_recording_reports_the_end_again() {
    // §3d rule 2: never `trace` at a terminal (t0 L4b's loop).
    let mut c = Rsp::spawn(crashy(), &[]);
    let crash = c.send("c");
    assert_eq!(c.send("vCont;s:1"), crash);
    assert_eq!(c.send("s"), crash);
}
```

In `debug.rs`'s tests:

```rust
    #[test] fn step_thread_steps_a_watched_store_and_reports_it_retired() {
        // M43 §3d: stepping onto a watched store reports it post-retire (WatchStepped), cursor at
        // (n, k + 1, Bp).
        let trace = record_watchsweep("stepthread");
        let buf0 = watchsweep_target(&trace) - 320;
        let mut ex = Exec::new(&trace).unwrap();
        let mut sink = Vec::new();
        run_cmds(&mut ex, &format!("watch 0x{buf0:x}; stepi 8"), &mut sink);
        let h = ex.step_thread(0, &mut sink).unwrap();
        assert!(matches!(h, Halt::WatchStepped { watched } if watched == buf0), "{h:?}");
        assert_eq!(ex.cursor(), (1, 9, Phase::Bp));
    }
```

(The store at K = 8 writes `buf[0]`, as `a_zero_count_step_is_not_an_arrival` already relies on.)

- [ ] **Step 2: See them fail.** The e2e rows fail: `s`, `vCont;s` and `bs` answer `""`. The unit
  test does not compile.

```sh
cargo test -p retrace --test gdbserver_e2e --no-fail-fast -- --test-threads=1 > $L/t4-red.log 2>&1; echo "exit=$?"
```

- [ ] **Step 3: `continue_until` and `step_thread`.** In `debug.rs`:

1. Rename `cmd_continue`'s body to
   `pub(crate) fn continue_until<W: Write>(&mut self, until: Option<u32>, out: &mut W) -> Result<Halt, String>`.
   Add `pub(crate) fn cmd_continue<W: Write>(&mut self, out: &mut W) -> Result<Halt, String> { self.continue_until(None, out) }`.
   Then add the arrival check in two places.
   - As the **first statement inside the finish's inner `loop`**, the one under
     `// ---- Finish (n, k): its hits still ahead of the cursor ----`, before `let pc = …`. That is
     the top of every turn of the finish, so the check runs at entry and again after each crossing
     the finish makes (its `AtTrap | Fault` arm stays in this loop):

```rust
                // M43 §3d: a step whose thread blocked ends when that thread runs again, before
                // anything at the coordinate it arrives at (an arrival, M41 R4). Only a crossing
                // leaves k == 0 with the phase Sys. The step's own entry uses phase Sys too, but with
                // another thread current, so this cannot fire before the run has moved.
                if let Some(t) = until {
                    if self.k == 0 && self.phase == Phase::Sys && self.sess().current_thread() == t {
                        self.phase = Phase::Bp;
                        return Ok(Halt::Stepped);
                    }
                }
```

   - In the **scan's `Advance::Event` arm**, first thing:

```rust
                        if let Some(t) = until {
                            if self.sess().current_thread() == t {
                                let n = self.sess().landmark();
                                self.sess_mut().clear_breakpoints(); // keep this session, hit-clean
                                self.sess_mut().clear_watchpoints();
                                (self.n, self.k, self.phase) = (n, 0, Phase::Bp);
                                return Ok(Halt::Stepped);
                            }
                        }
```

   With `until: None`, both are skipped: `cmd_continue` is today's `continue`, and the existing
   debug tests pin it.

2. Add `Halt::WatchStepped { watched: u64 }` (doc: "M43 §3d: `step_thread` stepped a store to a
   watched range; it retired, (n, k + 1, Bp)"), and:

```rust
    /// M43 §3d: one instruction of thread `t` (retrace's id), as lldb's `s` needs it. It crosses a
    /// trap, and a blocking syscall until `t` runs again. Nothing is ever `Stepped` without `t`'s pc
    /// moving (t0 L4b, L7: lldb re-steps forever).
    pub(crate) fn step_thread<W: Write>(&mut self, t: u32, out: &mut W) -> Result<Halt, String> {
        let cur = self.sess().current_thread();
        if t != cur {
            return Ok(Halt::Refused(format!(
                "cannot step thread {}: only the running thread ({}) can step", t + 1, cur + 1)));
        }
        let ws: Vec<(u64, u64)> = self.watches.iter().map(|&(a, l, _)| (a, l)).collect();
        let pc0 = self.sess().pc();
        self.sess_mut().arm_watchpoints(&ws);
        let stepped = self.sess_mut().step_armed();
        self.sess_mut().clear_watchpoints(); // the kept-session invariant
        match stepped? {
            Armed::Retired => { self.k += 1; self.phase = Phase::Bp; Ok(Halt::Stepped) }
            Armed::Watch => {
                let watched = watched_of(&ws, self.sess().far());
                self.sess_mut().step_insns(1)?; // retire it with nothing armed
                self.k += 1;
                self.phase = Phase::Bp;
                Ok(Halt::WatchStepped { watched })
            }
            Armed::AtTrap | Armed::Fault => {
                // cmd_continue's own crossing (R10): a fresh session parked before the trap, the
                // watches armed for this one event.
                let (n, k) = (self.n, self.k);
                self.reseek(n, k)?;
                self.sess_mut().arm_watchpoints(&ws);
                let adv = self.sess_mut().advance()
                    .map_err(|d| format!("step diverged at landmark {} pc {:#x}: {}", d.landmark, d.pc, d.detail));
                self.sess_mut().clear_watchpoints();
                match adv? {
                    Advance::Exited(report) => self.park_at_terminal(report, out),
                    Advance::WatchSyscall { watched, thread } => {
                        let n = self.sess().landmark();
                        (self.n, self.k, self.phase) = (n, 0, Phase::Sys);
                        Ok(Halt::WatchSys { watched, thread })
                    }
                    Advance::Event => {
                        let n = self.sess().landmark();
                        if self.sess().current_thread() == t {
                            (self.n, self.k, self.phase) = (n, 0, Phase::Bp);
                            if self.sess().pc() == pc0 {
                                return Ok(Halt::Refused("step did not move".into()));
                            }
                            return Ok(Halt::Stepped);
                        }
                        // `t` blocked. Run until it is current again, with the user's hits armed:
                        // another thread's hit is reported as its own stop (spec R7). Phase Sys, so
                        // a breakpoint at (n, 0) on the thread now running is still ahead.
                        (self.n, self.k, self.phase) = (n, 0, Phase::Sys);
                        self.continue_until(Some(t), out)
                    }
                    Advance::Break | Advance::Watch { .. } => Err(
                        "step: a hardware stop during a one-event crossing (breakpoints are off and the \
                         guest is parked on the trap or fault)".into()),
                }
            }
            Armed::Break => Err("step: a breakpoint stop with no breakpoint armed".into()),
        }
    }
```

`Armed` and `Advance` are already imported in `debug.rs`.

- [ ] **Step 4: The server's step packets.** In `rsp.rs`, add `StopKind::Trace` (`reason:trace;`,
  signal 5). In `gdbserver.rs`:
  - add fields `rsi_armed: bool` and `hc: u32` to `Server`;
  - store `Hc`'s id in `hc`, as `Hg`'s goes in `hg`;
  - add:

```rust
    /// §3d: step thread `rsp_tid` (0 or none: the `Hc` thread, else the current one).
    fn step(&mut self, rsp_tid: Option<u32>) -> String {
        let pick = rsp_tid.filter(|&t| t != 0 && t != u32::MAX).or(Some(self.hc).filter(|&t| t != 0 && t != u32::MAX));
        self.motion(|s| {
            let t = match pick { Some(r) => r - 1, None => s.ex.sess().current_thread() };
            match s.ex.step_thread(t, &mut std::io::sink())? {
                Halt::Stepped => Ok(s.stop(StopKind::Trace, None)),
                // Reported on the running thread, the measured-safe form (t0 L7).
                Halt::Refused(why) => Ok(s.stop(StopKind::Exception { signal: 5, text: why }, None)),
                Halt::WatchStepped { watched } => Ok(s.stop(StopKind::Watch(watched), None)),
                other => s.reply_forward(other),
            }
        })
    }

    /// §3e: one instruction back, on whichever thread ran it.
    fn back_step(&mut self) -> String {
        self.motion(|s| match s.ex.cmd_reverse_stepi(1, &mut std::io::sink())? {
            Halt::Stepped => Ok(s.stop(StopKind::Trace, None)),
            Halt::AtStart => Ok(s.stop(StopKind::HistoryBegin("start of recording".into()), None)),
            other => Err(format!("reverse-stepi reported {other:?}")),
        })
    }
```

Then, in `handle`:
- `"s"` → `self.step(None)`;
- `S<sig>` → `self.step(None)`;
- `vCont;s…` / `vCont;S…`: lldb may send a default action after the step, e.g. `vCont;s:2;c`, so
  parse only the first action:

```rust
            _ if body.starts_with("vCont;s") || body.starts_with("vCont;S") => {
                // The first action names the thread (`s:<tid>`, `S05:<tid>`, or none). A trailing
                // default for the other threads (`;c`) is moot: only the running thread can step
                // (§3d rule 1).
                let act = body["vCont;".len()..].split(';').next().unwrap_or("");
                let tid = act.split_once(':').and_then(|(_, t)| u32::from_str_radix(t, 16).ok());
                (vec![self.step(tid)], false)
            }
```

- `"bs"` → `self.back_step()`;
- replace `"bc"`'s arm with:
  `if std::mem::take(&mut self.rsi_armed) { self.back_step() } else { <Task 3's reverse continue> }`;
- in `monitor`, add `"arm-rsi" => { self.rsi_armed = true; (vec!["OK".into()], false) }`.

Place every step arm before the catch-all. None of Task 3's continue arms matches a step packet,
because their prefixes (`c`, `C`, `vCont;c`, `vCont;C`) are all different.

- [ ] **Step 5: Green.**

```sh
cargo test -p retrace --bins --no-fail-fast -- --test-threads=1 > $L/t4-bins.log 2>&1; echo "exit=$?"
cargo test -p retrace --test gdbserver_e2e --no-fail-fast -- --test-threads=1 > $L/t4-e2e.log 2>&1; echo "exit=$?"
cargo test -p retrace --test debug_cli --test crashy_cli --test watch_cli --test watchsweep_e2e --test hitorder_e2e --test reverse_debug_e2e --test thread_watch_e2e --test llsc_e2e --no-fail-fast -- --test-threads=1 > $L/t4-cli.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets -- -D warnings > $L/t4-clippy.log 2>&1; echo "exit=$?"
```

Expected: `--bins` 28; `gdbserver_e2e` 19; the script-debugger suites unchanged; clippy clean.

- [ ] **Step 6: Commit.**

```sh
git add crates/retrace/src/debug.rs crates/retrace/src/rsp.rs crates/retrace/src/gdbserver.rs crates/retrace/tests/gdbserver_e2e.rs
git commit -m "M43 t4: stepping over RSP — across traps, through blocking syscalls, backward (§3d, §3e)"
```

- [ ] **Step 7: Control C4 (committed tree).** Delete both `if let Some(t) = until { … }` blocks
  from `continue_until`. Expected: `a_step_over_a_blocking_syscall…` goes RED: the step runs to the
  end of the recording, or stops on another thread. Record which. Restore with
  `git checkout -- crates/retrace/src/debug.rs`.

- [ ] **Step 8: Spec R7's measurement: lldb and a step that ends on another thread's breakpoint.**
  Build `target/…/retrace` and record `threadrust` (the tests' pattern). Pick:
  - `svc`: the blocking wait's `trap_pc`, as in Step 1;
  - `B`: the pc at `(n+1, 0)` from `retrace_core::seek(tr, n + 1, 0).pc()`, where the other thread
    runs.

  Also count `m`, the number of `Syscall { num: 515 }` events before landmark `n` (indices
  `1..n`). Every `__ulock_wait` runs the same svc, so lldb's breakpoint must ignore that many hits
  first. Write a one-off Rust test, **not committed**, or a scratch script. It starts the server
  and runs `lldb -x -b -s cmds </dev/null` with:

```
gdb-remote 127.0.0.1:<port>
breakpoint set -a <svc> -i <m>
process continue
breakpoint delete 1
breakpoint set -a <B>
thread step-inst
thread list
script print("END")
```

  Before the step, check lldb's stop line shows thread `t + 1` at `<svc>`. If it does not, `m` is
  wrong: fix the count, not the script.

  Bound it at 60 s. Record lldb's output and whether it printed `END`.
  - **Pass:** it reports `stop reason = breakpoint 2.1` on the other thread and reaches `END`.
    R7 stands.
  - **Loop:** it spins until the bound. Apply R7's fallback: `continue_until(Some(t))` runs with
    nothing armed. Pass empty breakpoint and watch lists through a flag, or clone
    `bps`/`ws` as empty when `until.is_some()`. Add a `gdbserver_e2e` row pinning it, and say so
    in the report.

  Either way, the report quotes the lldb transcript. Task 5 pins the chosen behaviour with lldb.

---

### Task 5: lldb (spec §3e, §4)

The shipped `rsi` command, and real lldb against the server: the headline on `crashy`, the demo on
CPython, and determinism.

**Files:**
- Create: `crates/retrace/lldb/retrace.py`
- Create: `crates/retrace/tests/lldb_e2e.rs`

**Interfaces:**
- Consumes: `util::rsp::spawn_server` (Task 2), and the server's behaviour from Tasks 3–4. If
  Task 4 Step 8 applied R7's fallback, its ledger ruling says so.

- [ ] **Step 1: `retrace.py`.**

```python
"""retrace's lldb commands (M43). Load with `command script import <repo>/crates/retrace/lldb/retrace.py`.

rsi: reverse step one instruction. lldb-2100 has no reverse step of its own (it never sends `bs`),
so this arms the server (`monitor arm-rsi`) and continues in reverse; the server answers that one
`bc` with a single step back. Like `process continue -R`, it leaves lldb's direction reversed: a
plain `continue` afterwards also goes BACKWARD until `process continue -F`.
"""
import lldb


def rsi(debugger, command, result, internal_dict):
    ci = debugger.GetCommandInterpreter()
    r = lldb.SBCommandReturnObject()
    ci.HandleCommand("process plugin packet monitor arm-rsi", r)
    if not r.Succeeded():
        result.SetError("retrace rsi: arming the server failed: " + (r.GetError() or ""))
        return
    process = debugger.GetSelectedTarget().GetProcess()
    err = process.ContinueInDirection(lldb.eRunReverse)
    if not err.Success():
        result.SetError("retrace rsi: " + str(err))
        return
    ci.HandleCommand("process status", result)


def __lldb_init_module(debugger, internal_dict):
    debugger.HandleCommand("command script add -f retrace.rsi rsi")
```

- [ ] **Step 2: The tests.** Create `crates/retrace/tests/lldb_e2e.rs`:

```rust
//! M43: real lldb against `retrace gdbserver` (spec
//! `docs/superpowers/specs/2026-09-25-retrace-m43-lldb-design.md` §4). The original design's exit
//! criterion, "reverse-step through a real crash in LLDB", on the repo-owned `crashy` fixture, and
//! on CPython when Homebrew's is installed.
//!
//! lldb is not a repo artifact, so each test skips with a loud `eprintln!` when `lldb --version`
//! does not run. A silent skip reads as a green it did not earn. The lldb invocation is
//! `lldb -x -b -s <file> </dev/null`, never `-o`, which silently stops after a crash or boundary
//! stop and exits 0 (t0 L10). The script's last command prints `END`, and every test asserts it.
mod util;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const GARBAGE_VA: u64 = 0x4000_DEAD_0000; // mirrors c/crashy.c
const BOUND: u64 = 300;

fn lldb_runs() -> bool {
    Command::new("lldb").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn retrace_py() -> String { concat!(env!("CARGO_MANIFEST_DIR"), "/lldb/retrace.py").to_string() }

/// Start `retrace gdbserver <trace>`, run lldb with `cmds` (a `gdb-remote` line is prepended and an
/// `END` sentinel appended), return (lldb's exit code or None if killed at the bound, stdout, stderr).
fn session(trace: &Path, cmds: &[String]) -> (Option<i32>, String, String) {
    let (mut srv, port, srv_err) = util::rsp::spawn_server(trace, &[]);
    let base = std::env::temp_dir().join(format!("retrace-lldb-{}-{port}", std::process::id()));
    let (cmd_p, out_p, err_p) = (base.with_extension("cmds"), base.with_extension("out"), base.with_extension("err"));
    let mut script = vec![format!("gdb-remote 127.0.0.1:{port}"), format!("command script import {}", retrace_py())];
    script.extend(cmds.iter().cloned());
    script.push(r#"script print("END")"#.into());
    std::fs::write(&cmd_p, script.join("\n") + "\n").unwrap();
    let mut child = Command::new("lldb").args(["-x", "-b", "-s", cmd_p.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&out_p).unwrap()).stderr(std::fs::File::create(&err_p).unwrap())
        .spawn().expect("spawn lldb");
    let mut code = None;
    for _ in 0..BOUND * 20 {
        if let Some(st) = child.try_wait().unwrap() { code = Some(st.code().unwrap_or(-1)); break; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if code.is_none() { let _ = child.kill(); let _ = child.wait(); }
    let _ = srv.kill();
    let _ = srv.wait();
    let read = |p: &PathBuf| { let s = std::fs::read_to_string(p).unwrap_or_default(); let _ = std::fs::remove_file(p); s };
    let _ = std::fs::remove_file(&cmd_p);
    let srv_log = read(&srv_err);
    (code, read(&out_p), read(&err_p) + "\n--- gdbserver stderr\n" + &srv_log)
}

/// The corrupting store, by an oracle the server cannot influence: step window T (the crash's)
/// with a fresh session until g.ptr changes. Returns the pc before the store, the store's, and the
/// one after it. The first and last are read, not computed: the loop may branch.
fn crashy_store(trace: &Path, ptr: u64) -> (u64, u64, u64) {
    let events = retrace_trace::Reader::open(trace).unwrap();
    let t = events.iter().position(|e| matches!(e, retrace_trace::Event::Crash { .. })).expect("a crash");
    let mut s = retrace_core::seek(trace, t, 0).unwrap();
    let before = s.read_mem(ptr, 8).unwrap();
    let mut prev = None;
    loop {
        let pc = s.pc();
        s.step_insns(1).expect("the store comes before the fault");
        if s.read_mem(ptr, 8).unwrap() != before {
            return (prev.expect("the store is not window T's first instruction"), pc, s.pc());
        }
        prev = Some(pc);
    }
}

/// Every value lldb printed after `key` (`old value:` / `new value:`), in order. lldb prints an
/// integer in decimal, or in hex with `0x`. Both are accepted, so the assertions do not depend on
/// the format.
fn values(out: &str, key: &str) -> Vec<u64> {
    out.match_indices(key).filter_map(|(i, _)| {
        let t = out[i + key.len()..].split_whitespace().next()?;
        match t.strip_prefix("0x") { Some(h) => u64::from_str_radix(h, 16).ok(), None => t.parse().ok() }
    }).collect()
}

fn crashy_trace() -> PathBuf {
    let (rec, t) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "record crashy: {}", rec.stderr);
    t
}

fn crashy_script(ptr: u64) -> Vec<String> {
    vec!["process continue".into(), "bt 1".into(),
         format!("watchpoint set expression -w write -s 8 -- {ptr:#x}"),
         "process continue -R".into(), "register read pc".into(),
         "rsi".into(), "register read pc".into(),
         "process continue -F".into(), "register read pc".into()]
}

#[test]
fn lldb_reverse_debugs_crashy_from_the_crash_to_its_corrupting_store() {
    if !lldb_runs() {
        eprintln!("SKIPPED lldb_reverse_debugs_crashy…: `lldb --version` did not run. This gate did NOT run.");
        return;
    }
    let trace = crashy_trace();
    let (_st, ptr) = util::discover_crashy_addrs(&trace);
    let (prev, store, next) = crashy_store(&trace, ptr);
    let (code, out, err) = session(&trace, &crashy_script(ptr));
    let t = format!("exit {code:?}\n--- stdout\n{out}\n--- stderr\n{err}");
    assert!(out.lines().any(|l| l.trim() == "END"), "the batch ran to its end: {t}");
    assert_eq!(code, Some(0), "{t}");
    assert!(out.contains("stop reason = start of recording"), "connect: {t}");
    assert!(out.contains(&format!("stop reason = EXC_BAD_ACCESS (code=1, address={GARBAGE_VA:#x})")), "{t}");
    assert!(out.contains("crashy`main"), "frame #0 is symbolicated from the recording's exe: {t}");
    assert!(out.contains("stop reason = watchpoint 1"), "{t}");
    let pcs: Vec<u64> = out.lines().filter_map(|l| l.trim().strip_prefix("pc = "))
        .map(|v| u64::from_str_radix(v.split_whitespace().next().unwrap().trim_start_matches("0x"), 16).unwrap())
        .collect();
    assert_eq!(pcs, vec![store, prev, next],
        "c -R stops before the store, rsi one instruction earlier, c -F after the store: {t}");
    // Backward, lldb's old value is the one it last saw (the garbage, at the crash), and the new one
    // is memory before the store (&g.buf[0]). Forward again, the store writes the garbage back.
    assert_eq!(values(&out, "old value:"), vec![GARBAGE_VA, ptr - 32], "{t}");
    assert_eq!(values(&out, "new value:"), vec![ptr - 32, GARBAGE_VA], "{t}");
    assert!(out.contains("stop reason = trace"), "rsi: {t}");
}

#[test]
fn an_lldb_session_is_deterministic() {
    if !lldb_runs() {
        eprintln!("SKIPPED an_lldb_session_is_deterministic: `lldb --version` did not run. This gate did NOT run.");
        return;
    }
    let trace = crashy_trace();
    let (_st, ptr) = util::discover_crashy_addrs(&trace);
    let norm = |s: String| s.lines().map(|l| if l.contains("gdb-remote 127.0.0.1:") { "gdb-remote <port>" } else { l })
        .collect::<Vec<_>>().join("\n");
    let a = norm(session(&trace, &crashy_script(ptr)).1);
    let b = norm(session(&trace, &crashy_script(ptr)).1);
    assert_eq!(a, b, "two sessions, one transcript (t0 L10)");
}

#[test]
fn lldb_reverse_debugs_cpython_from_the_crash_to_the_store_of_the_pointer() {
    const REAL: &str = "/opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python";
    const TARGET: u64 = 0x4000_DEAD_0000;
    if !lldb_runs() || !Path::new(REAL).exists() {
        eprintln!("SKIPPED lldb_reverse_debugs_cpython…: needs lldb and {REAL}. This gate did NOT run.");
        return;
    }
    let (rec, trace) = util::record_dynamic_args(REAL, &[retrace_guest::CRASH_PY]);
    let stdout = String::from_utf8_lossy(&rec.stdout).into_owned();
    let start = stdout.find("CRASHPY cell=0x").expect("the marker line") + "CRASHPY cell=0x".len();
    let cell = u64::from_str_radix(&stdout[start..].chars().take_while(|c| c.is_ascii_hexdigit()).collect::<String>(), 16).unwrap();
    // Forward again with `-F`, not `thread step-inst`: after `-R` lldb's direction is reverse, and
    // what a step does then is not something t0 measured. `-F` re-reports the same store, retired
    // (§3c: the reverse stop is an arrival before it).
    let cmds = vec!["process continue".into(),
        format!("watchpoint set expression -w write -s 8 -- {cell:#x}"),
        "process continue -R".into(),
        format!("memory read -s8 -fx -c1 {cell:#x}"),
        "process continue -F".into(),
        format!("memory read -s8 -fx -c1 {cell:#x}")];
    let (code, out, err) = session(&trace, &cmds);
    let t = format!("exit {code:?}\n--- stdout\n{out}\n--- stderr\n{err}");
    assert!(out.lines().any(|l| l.trim() == "END"), "{t}");
    assert!(out.contains(&format!("address={TARGET:#x})")), "the crash is the deref: {t}");
    assert_eq!(out.matches("stop reason = watchpoint 1").count(), 2, "backward, then forward: {t}");
    // By effect (cpython_crash_e2e's proof): before the store the cell is not TARGET; after the
    // forward watch stop it is. `memory read -fx` prints "0x<addr>: 0x<value>".
    let reads: Vec<&str> = out.lines().filter(|l| l.starts_with(&format!("{cell:#x}:"))).collect();
    assert_eq!(reads.len(), 2, "{t}");
    assert!(!reads[0].contains(&format!("{TARGET:#018x}")) && reads[1].contains(&format!("{TARGET:#018x}")), "{t}");
}
```

The exact text lldb prints for `old value:` / `new value:`, `register read pc` (`pc = 0x…`, then a
symbol) and `memory read` may differ in format from what these assertions guess. Before the
assertions are final:
1. run each test once;
2. read the transcript;
3. adjust **only the parsing** (never what is asserted) to lldb's actual format;
4. quote the lines in the report.

The assertions' content is fixed:
- the crash address;
- the three pcs, `[store, prev, next]`, from the step oracle;
- old values `[garbage, &g.buf[0]]` and new values `[&g.buf[0], garbage]`;
- `END`;
- the by-effect pair.
**If Task 4 Step 8 measured R7's fallback,** add a row here pinning it: the threadrust session from
that step, asserting the step completes on the stepped thread.

- [ ] **Step 3: Run.**

```sh
cargo test -p retrace --test lldb_e2e --no-fail-fast -- --test-threads=1 --nocapture > $L/t5-lldb.log 2>&1; echo "exit=$?"
grep -a -E "^test |SKIPPED|panicked" $L/t5-lldb.log
```

Expected: 3 passed, none SKIPPED on this machine (lldb and Homebrew Python are both installed). A
SKIPPED line here is a finding: report it.

- [ ] **Step 4: Commit.**

```sh
git add crates/retrace/lldb/retrace.py crates/retrace/tests/lldb_e2e.rs
git commit -m "M43 t5: lldb reverse-debugs a real crash through retrace gdbserver; rsi"
```

- [ ] **Step 5: Control C5 (committed tree).** In `gdbserver.rs`, remove
  `watchpoint_exceptions_received:after;` from `QHOSTINFO`. Expected RED in
  `lldb_reverse_debugs_crashy…`: after `c -R`, lldb single-steps forward over the store itself, so
  the first pc is `store + 4` and old == new (t0 L4c). Record the symptom. Restore with
  `git checkout -- crates/retrace/src/gdbserver.rs`.

---

### Task 6: Close

- [ ] **Step 1: The chunked gate.** Every test binary, each chunk `--no-fail-fast`, with the exit
  code captured before any pipe (CLAUDE.md):

```sh
cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1 > $L/gate-ws.log 2>&1; echo "exit=$?"
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 > $L/gate-box.log 2>&1; echo "exit=$?"
cargo test -p retrace --bins --no-fail-fast -- --test-threads=1 > $L/gate-bins.log 2>&1; echo "exit=$?"
ls crates/retrace/tests/*.rs | xargs -n1 basename | sed 's/\.rs$//' > $L/gate-targets.txt
```

Then run one `cargo test -p retrace --test <name> --no-fail-fast -- --test-threads=1 > $L/gate-e2e-<name>.log 2>&1; echo "exit=$?"`
per name in `gate-targets.txt`, from a script file. Finish with
`cargo clippy --workspace --all-targets -- -D warnings > $L/gate-clippy.log 2>&1; echo "exit=$?"`.

- [ ] **Step 2: Reconcile.** Sum `passed` / `failed` / `ignored` across every `test result:` line
  (`grep -a`), and count the binaries. Diff each file's `#[test]` count against M42's close
  (747 / 0 / 9 over 142). Predicted: +5 (T1), +7 unit and +7 e2e (T2), +3 unit and +7 e2e (T3),
  +1 unit and +5 e2e (T4), +3 (T5). That gives **785 / 0 / 9 over 144** (spec §9's ≈ 791 was an estimate made before these per-task
  counts existed) (plus R7's row, if the
  fallback applied). Explain any difference file by file.

- [ ] **Step 3: The audit.** Confirm that `git diff c652cf1 -- crates/retrace-trace` is empty,
  and that no existing debug test's assertion changed (`git diff c652cf1 -- crates/retrace/tests/`
  touches only new files, `util/mod.rs`, `util/rsp.rs` and `llsc_e2e.rs`'s additions).

- [ ] **Step 4: Docs.**
  - **README, "What works today":** a *Debugging with lldb* section with the exact commands:
    `retrace gdbserver t.bin --port 5555`, then in lldb `gdb-remote 5555`,
    `command script import crates/retrace/lldb/retrace.py`, `process continue`,
    `watchpoint set expression -w write -s 8 -- <addr>`, `process continue -R`, `rsi`,
    `process continue -F`. Include the sticky-direction warning.
  - **README, "Known limits":** spec §7's list. Remove the three M42 panics that §3i fixed, and
    note outcome (a)/(b).
  - **README:** the gate line.
  - **`docs/status-log.md`:** append the M43-lldb section (never edit an older one).
  - **CLAUDE.md:** add `gdbserver_e2e` and `lldb_e2e` to the gate list with one clause each.
  - **Spec §10:** the outcome.
