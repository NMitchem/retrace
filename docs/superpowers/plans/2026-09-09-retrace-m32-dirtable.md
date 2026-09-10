# M32-dirtable Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the M30 guard-band fill decision a property of the *argument* rather than the
*syscall*, so that a syscall the kernel reads through can still get destination-side canary coverage
on the arguments the kernel only writes.

**Architecture:** `retrace-arch` gains a per-argument **allow-list** predicate `is_known_dest_arg`.
`retrace-box`'s `Window` tuple gains the argument index so `forward_and_diff` can consult it, and
the four sites currently gated on the per-syscall `fill_canary` boolean move to a per-window
predicate together. The allow-list is seeded from a measurement taken in Task 1, not from the
syscall's ABI.

**Tech Stack:** Rust 1.95.0, `aarch64-apple-darwin`, Hypervisor.framework. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-09-retrace-m32-dirtable-design.md`

## Global Constraints

- **`--test-threads=1` is mandatory** on every `cargo test` invocation. HVF allows one VM per
  process; a bare `cargo test` flakes with `HV_BUSY`.
- **Target is pinned** by `.cargo/config.toml` to `aarch64-apple-darwin`, with a codesigning
  `runner` (`tools/codesign-run.sh`). Do not override either.
- **Polarity is an allow-list, never a deny-list.** `fill_canary(arg i) := !reads_guest_buffer(num)
  || is_known_dest_arg(num, i)`. A source deny-list re-opens M30 — see spec §3. This constraint
  governs Tasks 3, 4 and 6.
- **Fill ⟺ restore.** A band is filled if and only if it is restored. A filled-but-unrestored band
  is captured into the trace as a kernel write.
- **No `TRACE_MAGIC` bump.** This change sits below the trace. If any edit changes a recorded byte
  in `Event::Syscall`, stop — that is a charter §5 halt condition.
- **Do not push.** Commit and merge locally only.
- During tasks run targeted tests only; the full chunked gate runs once, in Task 6.

---

### Task 1: Measure where the band lands relative to `send_size`

**This task makes no production edit.** Its deliverable is a measurement and a decision.

**Files:**
- Create: `crates/retrace-box/tests/machmsgband.rs`
- Read only: `crates/retrace-core/src/machmsg.rs:30-36`, `crates/retrace-box/src/lib.rs:3160-3250`

**Interfaces:**
- Consumes: nothing.
- Produces: a decision recorded in the task report — `SEED_MACH_MSG2 = true | false` — consumed by
  Task 3.

**Background the implementer needs.** `mach_msg2_trap` is trap number −47, written throughout this
repo as `0xffff_ffff_ffff_ffd1`. Its operands are already decoded in
`crates/retrace-core/src/machmsg.rs:30-36`:

| field | where |
|---|---|
| `data` — the message buffer, used for **both** send and receive | `args[0]` |
| `send_size` | the high 32 bits of `args[2]` |
| `rcv_size` | the low 32 bits of `args[6]` |

A Mach reply overwrites the same buffer, so send and receive overlap by design. The kernel **reads**
`[data, data + send_size)`. The canary band is placed at `ipa + len` (`lib.rs:3240`: `let base = *ipa
+ *len;`) — that is, *past* the window.

**The hypothesis to confirm or refute:** if the window `len` for `args[0]` is always `>= send_size`,
the band lands where the kernel never reads, and filling it is safe.

- [ ] **Step 1: Write the measurement test**

Create `crates/retrace-box/tests/machmsgband.rs`. Record, for every `mach_msg2` dispatched by
`retrace_guest::MACHMSG`, the triple `(len, send_size, rcv_size)` and assert the hypothesis
explicitly so a refutation is a red test rather than a silent note.

```rust
// M32 Task 1: a MEASUREMENT, not a guard. It exists to answer one question — does the M30 guard
// band for mach_msg2's message buffer land at or past `send_size`, where the kernel never reads?
// If it does, the buffer's argument may be canary-filled without re-creating the M30 corruption.
//
// This test is expected to survive the milestone as a regression pin on the measured fact.
use retrace_box::Box_;

const MACH_MSG2: u64 = (-47i64) as u64;

#[test]
fn the_band_for_mach_msg2s_buffer_lands_at_or_past_send_size() {
    let mut b = Box_::load(retrace_guest::MACHMSG).expect("machmsg guest loads");
    let mut seen = 0usize;
    loop {
        match b.run().expect("guest runs") {
            retrace_box::Stop::Syscall { num, args, .. } if num == MACH_MSG2 => {
                let send_size = (args[2] >> 32) as usize;
                // The window length forward_and_diff would use for args[0].
                let len = b.dbg_window_len_for(args[0]);
                assert!(len >= send_size,
                    "band would land INSIDE the kernel-read region: len {len} < send_size \
                     {send_size} for buffer {:#x}. The hypothesis is REFUTED — mach_msg2 must \
                     stay withheld (spec §7, last bullet).", args[0]);
                seen += 1;
                b.forward_and_diff(num, &args).expect("forward");
            }
            retrace_box::Stop::Exit { .. } => break,
            _ => {}
        }
    }
    assert!(seen > 0,
        "machmsg guest dispatched ZERO mach_msg2 calls — this measurement measured nothing, \
         which is the dead-channel trap the spec's §4b exists to catch");
}
```

**Note on `dbg_window_len_for`:** if no such accessor exists, add it as a `#[doc(hidden)]`
test-only method on `Box_` returning the same `len` `forward_and_diff` computes for a pointer
argument, following the pattern of the existing `dbg_next_l3` / `dbg_backings` accessors
(`crates/retrace-box/src/lib.rs:5349` and nearby). Adding it is part of this task.

- [ ] **Step 2: Run it and read the result**

```sh
cargo test -p retrace-box --test machmsgband -- --test-threads=1 --nocapture
```

Expected: either PASS (hypothesis confirmed) or FAIL with the `len < send_size` message
(hypothesis refuted). **Both are valid outcomes of this task.**

- [ ] **Step 3: Record the decision**

Write into the task report, verbatim, the observed `(len, send_size, rcv_size)` triples and one line:

- `SEED_MACH_MSG2 = true` if every observed `len >= send_size`.
- `SEED_MACH_MSG2 = false` otherwise. **This is a success, not a failure** (spec §7, last bullet).
  Task 3's allow-list is then seeded empty of `mach_msg2` and the milestone still lands the
  mechanism.

- [ ] **Step 4: Run the dead-channel check for `sendfile`**

```sh
grep -rn "sendfile" crates/ | grep -v "^crates/retrace-arch/src/lib.rs" | grep -v "^crates/retrace-box/src/lib.rs"
```

Expected: no hits outside comments. Confirms the spec's §2 finding still holds. Record the output.

- [ ] **Step 5: Commit**

```sh
git add crates/retrace-box/tests/machmsgband.rs crates/retrace-box/src/lib.rs
git commit -m "M32 t1: measure where mach_msg2's guard band lands relative to send_size"
```

---

### Task 2: Carry the argument index through `Window`

**Pure refactor. No behaviour change.** Its whole purpose is to make Task 4 possible.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs:961-964` (the `Window` alias and its doc comment)
- Modify: `crates/retrace-box/src/lib.rs:3182` (the push), `:3196`, `:3197-3199`, `:3235`, `:3394`,
  `:3541` (the destructures)

**Interfaces:**
- Consumes: nothing.
- Produces: `type Window = (u64, usize, Vec<u8>, Vec<u8>, usize, usize);` — the sixth field is the
  **argument index** `i` the window was built from. Task 4 consumes it.

- [ ] **Step 1: Widen the alias and its comment**

At `crates/retrace-box/src/lib.rs:961-964`, change:

```rust
/// `forward_and_diff`'s per-argument bookkeeping: (guest_ipa, len, pre-image, pre-image of the
/// M27 guard band past the window, M28/M30 shrunk band length, ARGUMENT INDEX).
/// Factored out at M30 because the fifth field pushed the inline tuple over clippy's
/// type-complexity threshold. The sixth field is M32's: the fill decision is per-ARGUMENT, and
/// without the index the fill site cannot tell which argument a window came from.
type Window = (u64, usize, Vec<u8>, Vec<u8>, usize, usize);
```

- [ ] **Step 2: Update the push site**

At `:3182`, change `windows.push((args[i], win, pre, pre_band, 0));` to carry `i`:

```rust
windows.push((args[i], win, pre, pre_band, 0, i));
```

- [ ] **Step 3: Update every destructure**

Five sites. Add a trailing `_` (or a binding where Task 4 will need it):

- `:3196` — `windows.iter().map(|(ipa, len, _, _, _, _)| (*ipa, *len)).collect();`
- `:3199` — `let (ipa, len, _, pre_band, band, _) = w;`
- `:3235` — `for (ipa, len, _, _, band, _) in windows.iter() {`
- `:3394` — `for (ipa, len, pre, pre_band, band, _) in windows.iter() {`
- `:3541` — `for (ipa, len, _pre, pre_band, band, _) in windows.iter() {`

- [ ] **Step 4: Verify nothing changed behaviourally**

```sh
cargo test -p retrace-box -- --test-threads=1
cargo test -p retrace --test bigwrite_e2e --test bigread_e2e -- --test-threads=1
```

Expected: PASS, with the same counts as before this task. A refactor that changes a test result is
not a refactor — stop and investigate.

- [ ] **Step 5: Commit**

```sh
git add crates/retrace-box/src/lib.rs
git commit -m "M32 t2: carry the argument index through Window — no behaviour change"
```

---

### Task 3: `is_known_dest_arg` — the allow-list

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (add beside `reads_guest_buffer` at `:308`)
- Test: `crates/retrace-arch/src/lib.rs` (the existing `#[cfg(test)]` module, near `:835`)

**Interfaces:**
- Consumes: Task 1's `SEED_MACH_MSG2` decision.
- Produces: `pub fn is_known_dest_arg(num: u64, idx: usize) -> bool`.

- [ ] **Step 1: Write the failing tests**

Add to the existing test module in `crates/retrace-arch/src/lib.rs`:

```rust
// M32: the fill decision is per-ARGUMENT. This is an ALLOW-list of arguments the kernel WRITES,
// never a deny-list of arguments it reads — see the M32 design §3. Under a deny-list, a stale
// register that merely looks like a pointer is "not a source", gets canaried, and re-creates the
// M30 corruption that bigwrite_e2e exists to catch.
#[test]
fn the_dest_arg_list_is_default_deny() {
    // Nothing is a destination unless listed — including arguments of syscalls that are not
    // reads_guest_buffer members at all, and including out-of-range indices.
    for idx in 0..8 {
        assert!(!is_known_dest_arg(SYS_WRITE, idx),
            "write has no kernel-written argument; index {idx} must not be allow-listed");
        assert!(!is_known_dest_arg(SYS_READ, idx),
            "read is not a reads_guest_buffer syscall; this predicate must still say no");
    }
}

#[test]
fn an_unlisted_syscall_has_no_dest_args() {
    // 999 is not a real syscall. Default-deny means this is false for every index.
    for idx in 0..8 { assert!(!is_known_dest_arg(999, idx)); }
}
```

- [ ] **Step 2: Run to verify they fail**

```sh
cargo test -p retrace-arch -- --test-threads=1
```

Expected: FAIL — `cannot find function is_known_dest_arg`.

- [ ] **Step 3: Implement**

Add beside `reads_guest_buffer` in `crates/retrace-arch/src/lib.rs`:

```rust
/// Arguments of a `reads_guest_buffer` syscall that the kernel **writes** rather than reads.
///
/// **An allow-list, never a deny-list**, and the distinction is the whole of M32. `forward_and_diff`
/// builds a window for every mapped-looking argument — all eight registers, not merely the
/// syscall's declared ones. M30's corrupting entry was a stale register that was not an argument at
/// all (`crates/retrace-box/src/lib.rs:3222`). Under a deny-list such a register is "not a source",
/// falls through to the fill, and the corruption returns. Under this allow-list it is simply not
/// listed, and stays withheld.
///
/// Entries are seeded from MEASUREMENT, never from a syscall's ABI. Absence means "not measured".
pub fn is_known_dest_arg(num: u64, idx: usize) -> bool {
    match (num, idx) {
        // SEEDED BY TASK 1. See that task's report for the observed (len, send_size, rcv_size)
        // triples. Include this arm ONLY if Task 1 recorded SEED_MACH_MSG2 = true.
        (0xffff_ffff_ffff_ffd1, 0) => true, // mach_msg2_trap's message buffer (receive side)
        _ => false,
    }
}
```

**Branch on Task 1's decision:**
- If `SEED_MACH_MSG2 = true`, keep the `mach_msg2` arm as written.
- If `SEED_MACH_MSG2 = false`, **delete that arm** and leave only `_ => false`, and replace its
  comment with one naming the refuting measurement. The rest of this plan is unchanged; Task 5's
  coverage test then pins the *absence*, as its own branch describes.

**`sendfile` is deliberately not an arm.** It has no guest in this repo (spec §2), so an entry would
be untested. It is named in `reads_guest_buffer`'s doc comment as owed instead.

- [ ] **Step 4: Run to verify they pass**

```sh
cargo test -p retrace-arch -- --test-threads=1
```

Expected: PASS.

- [ ] **Step 5: Commit**

```sh
git add crates/retrace-arch/src/lib.rs
git commit -m "M32 t3: is_known_dest_arg — a default-deny per-argument allow-list"
```

---

### Task 4: Move all four sites to the per-window predicate

**The dangerous task.** Four sites must move together, and fill must imply restore.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs:3233` (the boolean), `:3235` (fill), `:3447` (`disturbed`),
  `:3485` and `:3499` (`overran`), `:3541` (restore)
- Test: `crates/retrace-box/tests/canary.rs`

**Interfaces:**
- Consumes: `retrace_arch::is_known_dest_arg` (Task 3); `Window`'s sixth field (Task 2).
- Produces: a private helper on `Box_`:
  `fn fills_band(num: u64, argi: usize) -> bool { !retrace_arch::reads_guest_buffer(num) || retrace_arch::is_known_dest_arg(num, argi) }`

- [ ] **Step 1: Write the failing invariant test**

Add to `crates/retrace-box/tests/canary.rs`:

```rust
// M32: fill <=> restore. A band that is filled and NOT restored is captured into the trace as a
// kernel write — retrace's own canary bytes recorded as guest state. A band restored but never
// filled overwrites real pre-image bytes. The two sites must agree for EVERY argument index, not
// merely for every syscall.
#[test]
fn the_fill_and_restore_predicates_agree_for_every_argument() {
    const MACH_MSG2: u64 = (-47i64) as u64;
    for num in [MACH_MSG2, retrace_arch::SYS_WRITE, retrace_arch::SYS_READ, 337, 999] {
        for argi in 0..8 {
            assert_eq!(Box_::dbg_fills_band(num, argi), Box_::dbg_restores_band(num, argi),
                "fill/restore disagree for syscall {num} argument {argi} — a filled-but-unrestored \
                 band reaches the trace as a kernel write");
        }
    }
}

// The polarity guard, as a unit test rather than only as a manual control (spec §6, control 2).
#[test]
fn a_reader_syscalls_unlisted_arguments_are_never_filled() {
    // write(fd, buf, nbyte): every argument, including registers that are not arguments at all,
    // must stay withheld. This is the M30 guarantee expressed per-argument.
    for argi in 0..8 {
        assert!(!Box_::dbg_fills_band(retrace_arch::SYS_WRITE, argi),
            "write argument {argi} must never be canary-filled — the kernel reads through it");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```sh
cargo test -p retrace-box --test canary -- --test-threads=1
```

Expected: FAIL — `cannot find function dbg_fills_band`.

- [ ] **Step 3: Implement the helper and move the four sites**

Add to `impl Box_`, beside the other `#[doc(hidden)]` accessors:

```rust
/// M32: the per-ARGUMENT fill decision. Allow-list polarity — see `is_known_dest_arg`.
fn fills_band(num: u64, argi: usize) -> bool {
    !retrace_arch::reads_guest_buffer(num) || retrace_arch::is_known_dest_arg(num, argi)
}

/// Test-only (M32): the fill predicate, exposed so the fill/restore invariant can be asserted.
#[doc(hidden)]
pub fn dbg_fills_band(num: u64, argi: usize) -> bool { Self::fills_band(num, argi) }

/// Test-only (M32): the RESTORE predicate. Deliberately a SEPARATE function from `dbg_fills_band`
/// rather than an alias — the invariant test must be able to catch the two diverging, and an alias
/// makes that test vacuous by construction.
#[doc(hidden)]
pub fn dbg_restores_band(num: u64, argi: usize) -> bool { Self::fills_band(num, argi) }
```

Then, at each of the four sites, replace the per-syscall `fill_canary` with the per-window call:

- **`:3233`** — delete `let fill_canary = !retrace_arch::reads_guest_buffer(num);`
- **`:3235`** (fill) — bind the index and gate per window:
  ```rust
  for (ipa, len, _, _, band, argi) in windows.iter() {
      if !Self::fills_band(num, *argi) { continue; }
      // ... existing body unchanged ...
  }
  ```
  and delete the enclosing `if fill_canary {`.
- **`:3447`** (`disturbed`) — `let disturbed = Self::fills_band(num, *argi) && !Self::canary_intact(post_band, base);`
- **`:3485` / `:3499`** (`overran`) — replace both `fill_canary` reads with `Self::fills_band(num, *argi)`.
- **`:3541`** (restore) — same shape as the fill: bind `argi`, `continue` when
  `!Self::fills_band(num, *argi)`, delete the enclosing `if fill_canary {`.

**Update the comment block at `:3219-3224`.** It currently states the decision is per-syscall and
covers all eight registers. That is no longer literally true, and the reason it *was* true must be
preserved, not deleted — rewrite it to say the decision is now per-argument with **allow-list
polarity**, and that all eight registers remain withheld by default for exactly the stale-register
reason the old comment gave.

- [ ] **Step 4: Run to verify it passes, and that M30's guarantee survives**

```sh
cargo test -p retrace-box --test canary --test truncguard -- --test-threads=1
cargo test -p retrace --test bigwrite_e2e --test bigread_e2e -- --test-threads=1
```

Expected: PASS. `bigwrite_e2e` is the M30 regression test — if it goes red here, the polarity is
inverted. Stop and re-read spec §3.

- [ ] **Step 5: Commit**

```sh
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/canary.rs
git commit -m "M32 t4: the fill decision moves to the argument, at all four sites"
```

---

### Task 5: Destination-side coverage, and positive control 1

**Files:**
- Modify: `crates/retrace-box/tests/machmsgband.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–4.
- Produces: the test that positive control 1 must turn red.

- [ ] **Step 1: Write the coverage test**

**If `SEED_MACH_MSG2 = true`**, add to `crates/retrace-box/tests/machmsgband.rs`:

```rust
// M32's deliverable, expressed as the difference this milestone makes (CLAUDE.md's honest-gate
// rule: assert on the difference your work makes). Before M32, mach_msg2's message buffer received
// NO band fill because the whole syscall was withheld. After M32, argument 0 is allow-listed and
// its band is filled — so a kernel write past the window into that buffer is now detectable.
#[test]
fn mach_msg2s_buffer_argument_is_now_band_covered() {
    const MACH_MSG2: u64 = (-47i64) as u64;
    assert!(Box_::dbg_fills_band(MACH_MSG2, 0),
        "mach_msg2 argument 0 must be band-covered after M32 — this is the milestone's deliverable");
    // Every OTHER argument stays withheld: the allow-list is one argument wide, not a blanket
    // un-withholding of the syscall.
    for argi in 1..8 {
        assert!(!Box_::dbg_fills_band(MACH_MSG2, argi),
            "mach_msg2 argument {argi} is not measured as a destination and must stay withheld");
    }
}
```

**If `SEED_MACH_MSG2 = false`**, write the mirror instead — it pins the refutation so a later reader
cannot mistake the absence for an oversight:

```rust
#[test]
fn mach_msg2_stays_withheld_because_task_1_refuted_the_hypothesis() {
    const MACH_MSG2: u64 = (-47i64) as u64;
    // Task 1 measured the band landing INSIDE [data, data+send_size) — the kernel-read region.
    // Filling it would re-create the M30 corruption. See the M32 t1 report for the triples.
    for argi in 0..8 {
        assert!(!Box_::dbg_fills_band(MACH_MSG2, argi));
    }
}
```

- [ ] **Step 2: Run it**

```sh
cargo test -p retrace-box --test machmsgband -- --test-threads=1
```

Expected: PASS.

- [ ] **Step 3: Run positive control 1 — the mechanism is wired up**

Temporarily change `is_known_dest_arg` in `crates/retrace-arch/src/lib.rs` to `_ => false` for all
inputs (delete the seeded arm).

```sh
cargo test -p retrace-box --test machmsgband -- --test-threads=1
```

Expected (when `SEED_MACH_MSG2 = true`): **RED** at
`mach_msg2s_buffer_argument_is_now_band_covered`. Record the exact failure message in the task
report.

When `SEED_MACH_MSG2 = false` this control does not apply; instead invert the mirror test (assert
`dbg_fills_band` is true) and confirm it goes red. Record that.

- [ ] **Step 4: Revert the control and confirm green**

```sh
git checkout -- crates/retrace-arch/src/lib.rs
cargo test -p retrace-box --test machmsgband -- --test-threads=1
```

Expected: PASS. Confirm `git status` is clean of the mutation before continuing.

- [ ] **Step 5: Commit**

```sh
git add crates/retrace-box/tests/machmsgband.rs
git commit -m "M32 t5: destination-side coverage for mach_msg2's buffer, with its positive control"
```

---

### Task 6: Positive control 2, the gate, and the documentation

**Files:**
- Modify: `README.md` ("Known limits")
- Modify: `docs/status-log.md` (append a new section — never rewrite an old one)

- [ ] **Step 1: Run positive control 2 — the polarity is load-bearing**

Temporarily invert `fills_band` in `crates/retrace-box/src/lib.rs` to a source deny-list:

```rust
fn fills_band(num: u64, argi: usize) -> bool {
    !retrace_arch::is_known_source_arg(num, argi)   // WRONG ON PURPOSE
}
```

Implement `is_known_source_arg` as the naive inverse (true only for arguments explicitly known to be
read), then run:

```sh
cargo test -p retrace --test bigwrite_e2e -- --test-threads=1
```

Expected: **RED**. `bigwrite_e2e` is M30's regression test and must catch the inversion.

**If it stays GREEN, STOP AND HALT** (charter §5). That means the existing regression test cannot
see the deny-list inversion, which is a finding worth more than this milestone. Report it; do not
proceed.

- [ ] **Step 2: Revert the control**

```sh
git checkout -- crates/retrace-box/src/lib.rs crates/retrace-arch/src/lib.rs
git status --porcelain=v1   # must be empty
```

- [ ] **Step 3: Run the full chunked gate**

Per CLAUDE.md. Capture each exit code **before any pipe**; do not omit `--bins`.

```sh
cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1 > /tmp/c1.log 2>&1; echo "c1 EXIT=$?"
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 > /tmp/c2.log 2>&1; echo "c2 EXIT=$?"
cargo test -p retrace --no-fail-fast -- --test-threads=1 > /tmp/c3.log 2>&1; echo "c3 EXIT=$?"
cargo clippy --workspace --all-targets -- -D warnings > /tmp/clippy.log 2>&1; echo "clippy EXIT=$?"
```

Grep the logs with `grep -a` — they carry ANSI and UTF-8 that trips plain grep.

- [ ] **Step 4: Reconcile the test count file-by-file**

The previous close (M31) was **552 passed / 0 failed / 2 ignored across 119 binaries**.

```sh
git grep -c -E '^[[:space:]]*#\[test\]' HEAD -- '*.rs' | sort > /tmp/now.cnt
git grep -c -E '^[[:space:]]*#\[test\]' a33983d -- '*.rs' | sed 's|^a33983d:||' | sort > /tmp/m31.cnt
diff /tmp/m31.cnt /tmp/now.cnt
```

Every line of the diff must be explainable by this milestone's new tests. Do not trust a sum.

- [ ] **Step 5: Edit the README and append to the status log**

In `README.md`'s "Known limits", the paragraph on the guard band being withheld from
reader-consumed buffers must be **edited in place** to say the withholding is now per-argument, that
`mach_msg2`'s buffer is covered (or explicitly is not, per Task 1), and that `sendfile` remains
table-only with no guest.

Append a new `## Status: M32-dirtable — ...` section to `docs/status-log.md`. It must include: the
Task 1 measurement triples, both positive controls' exact failure messages, the gate figure, and a
**"What stays owed"** section naming at minimum `sendfile`'s missing guest and `dest_buffer`'s
single-destination limit.

- [ ] **Step 6: Commit and merge**

```sh
git add README.md docs/status-log.md
git commit -m "M32-dirtable: the honest close"
git checkout main && git merge --no-ff m32-dirtable
```

**Do not push.**

---

## Self-Review

**Spec coverage.** §1 wall → Tasks 2/4. §2 scope and the `sendfile` finding → Task 1 step 4, Task 3's
non-arm, Task 6 step 5. §3 polarity → Tasks 3, 4, 6 step 1. §4a measurement → Task 1. §4b
dead-channel check → Task 1 step 4. §5a predicate → Task 3. §5b index + four sites → Tasks 2 and 4.
§6 both controls → Task 5 step 3 and Task 6 step 1. §7 deliberately-not → Task 6 step 5's owed
section. §8 symmetry → the Global Constraints' no-`TRACE_MAGIC` line and Task 4 step 4.

**Placeholder scan.** No TBD/TODO. The one genuinely unknown value — whether `mach_msg2` is seeded —
is not a placeholder but a measured decision, and every task that depends on it carries both
branches written out.

**Type consistency.** `is_known_dest_arg(num: u64, idx: usize) -> bool` is used with that signature
in Tasks 3, 4, 5. `fills_band(num, argi)` and its two `dbg_` wrappers are consistent across Tasks 4
and 5. `Window`'s sixth field is `usize` in Task 2 and consumed as `*argi: usize` in Task 4.

**One known weakness, stated rather than hidden.** `dbg_restores_band` currently delegates to the
same `fills_band` as `dbg_fills_band`, so the invariant test in Task 4 cannot fail *today* — it is a
tripwire against a future edit that gives the restore site its own predicate, not a proof about the
present code. Task 4 step 3 says so at the definition. The real present-day guard on fill/restore
symmetry is `bigwrite_e2e` plus the final full-memory comparison every e2e already performs.
