# M27-truncguard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development (recommended)
> or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax. Each task is dispatchable on
> its own to an implementer who has read nothing else.

**Goal:** Make the record-side diff-window truncation class **fail loud** instead of silent, then fix
exactly what that reveals — starting with `sysctl`, which is already measured to fire.

**Architecture:** A *guard band* past each capped diff window. Between the pre-image and post-image
copies the only thing that executes is `host_svc` (the guest vCPU is halted; `clippy.toml` bans
recorder threads), so any pre≠post byte is provably a kernel write from that syscall. Kernel writes
into a destination buffer are contiguous from the buffer start, so an overrun necessarily lands in a
band placed immediately past the window. All of it is **record-side** — `forward_and_diff` never runs
on replay — so no dispatch mirror is owed.

**Tech Stack:** Rust 1.95.0, `aarch64-apple-darwin`, macOS 26 on Apple Silicon, Hypervisor.framework.

**Spec:** `docs/superpowers/specs/2026-09-06-retrace-m27-truncguard-design.md`

**Branch:** `m27-truncguard`, cut from `main` at `29eac9a` (M26 close).

## Global Constraints

- **Never run cargo with `run_in_background`.** Bounded foreground only; macOS has no `timeout(1)`:
  ```sh
  perl -e 'alarm 500; exec @ARGV' cargo test -p retrace-box -- --test-threads=1
  ```
- **`--test-threads=1` is mandatory.** HVF allows one VM per process; a bare `cargo test` flakes with
  `HV_BUSY`.
- **The gate is chunked.** Capture cargo's exit code **before any pipe** — this shell is zsh, where
  `${PIPESTATUS[0]}` is empty; the array is `$pipestatus` and it is 1-indexed. Run
  `cargo test -p retrace-box` as a **whole package** (a per-target split drops its `Doc-tests`
  harness silently), and split `-p retrace` into explicit `--test` sets. **Never omit `--bins`** —
  `--test <name>` selects integration targets only, so the 11 unit tests in
  `crates/retrace/src/debug.rs` run in no other chunk. `cargo test -p retrace --lib` is invalid and
  fails loudly; the trap is that the wrong flag is loud and the missing one is silent.
- **Grep gate logs with `grep -a`** — they carry ANSI and UTF-8 that trips plain grep.
- **Reconcile the total file-by-file against `main`'s actual close** (M26: 515 / 0 / 2 over 114). Do
  not hard-code a baseline; read it off `main` when you get there.
- **`TRACE_MAGIC` does NOT move.** M27 adds no `Event` variant or field. If you believe you need a
  format change, stop — that is a spec deviation.
- **clippy clean** at `-D warnings` over `--workspace --all-targets`.
- **Codesigning.** A test that spawns `CARGO_BIN_EXE_retrace` bypasses cargo's signing runner; use
  `crates/retrace/tests/util/mod.rs::bin()`. **First execution of a freshly signed binary can stall
  for minutes at 0:00.00 CPU** (M26 measured 536s then 47s for the same test). That is Gatekeeper
  validation, not a hang. Do not kill it; re-run and use the second timing.
- **`RETRACE_TRACE=1` is record-only.** `ReplaySession` carries no trace instrumentation, so never
  expect a `[trap]` line from a replay.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/retrace-arch/src/lib.rs` | `DestLen`, `dest_buffer(num)`, `SYS_PREAD_NOCANCEL`, the scatter/gather predicate, `fd_operands` entry. Pure tables, no deps. |
| `crates/retrace-box/src/lib.rs` | `overran_window`, `GUARD_BAND`, the guard-band plumbing in `forward_and_diff`, `diff_window` rewritten over the table. |
| `crates/retrace-core/src/lib.rs` | The scatter/gather fail-loud refusal, in `record_box`'s generic arm. |
| `crates/retrace-arch/src/lib.rs` (tests) | Table unit tests. |
| `crates/retrace-box/tests/truncguard.rs` | **Create.** `overran_window` unit tests. |
| `crates/retrace/tests/sysbin_e2e.rs` | The `/bin/ps` gate. |

---

### Task 1: The guard band, as a WARNING

**This task must not panic.** Its deliverable is a measurement, not an assert. Task 3 flips it.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (add `GUARD_BAND`, `overran_window`; plumb the band
  through `forward_and_diff`'s window loop and post-diff loop)
- Create: `crates/retrace-box/tests/truncguard.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `pub const GUARD_BAND: usize`, and
  `pub fn overran_window(pre_guard: &[u8], post_guard: &[u8]) -> bool`.

- [ ] **Step 1: Write the failing test.** Create `crates/retrace-box/tests/truncguard.rs`:

```rust
use retrace_box::Box_;

// M27: the guard band is DIRECT evidence, not a heuristic. Between the pre-image and post-image
// copies the only thing that runs is `host_svc` — the guest vCPU is halted and recorder threads are
// banned — so any pre != post byte in a band placed PAST the diff window is provably a kernel write
// that ran past that window. Kernel writes into a destination buffer are contiguous from the buffer
// start, so an overrun cannot skip the band.
#[test]
fn a_changed_guard_band_means_the_write_ran_past_the_window() {
    assert!(Box_::overran_window(&[0u8; 64], &[1u8; 64]),
        "a wholly rewritten band is an overrun");
    assert!(Box_::overran_window(&[0u8; 64], &{ let mut b = [0u8; 64]; b[0] = 1; b }),
        "ONE changed byte is enough: writes are contiguous, so the first byte past the window is \
         the one an overrun touches first");
}

#[test]
fn an_unchanged_guard_band_is_not_an_overrun() {
    assert!(!Box_::overran_window(&[0u8; 64], &[0u8; 64]));
    assert!(!Box_::overran_window(&[7u8; 64], &[7u8; 64]));
}

// A band that could not be taken (the window already covers the whole backing) is never an
// overrun: nothing can be past the backing without a separate memory-safety bug, which is a
// different failure with its own loud symptom.
#[test]
fn an_empty_guard_band_is_never_an_overrun() {
    assert!(!Box_::overran_window(&[], &[]));
}
```

- [ ] **Step 2: Run it and watch it fail.**

Run: `perl -e 'alarm 400; exec @ARGV' cargo test -p retrace-box --test truncguard -- --test-threads=1`
Expected: FAIL to compile — `no function or associated item named 'overran_window'`.

- [ ] **Step 3: Add the constant and the predicate.** In `crates/retrace-box/src/lib.rs`, beside
  `PTR_WINDOW_CAP` (line ~107):

```rust
/// Bytes snapshotted immediately PAST a capped diff window, to detect a kernel write that ran
/// past it (M27).
///
/// One byte would suffice for contiguity — kernel writes into a destination buffer start at the
/// buffer and run forward, so an overrun always touches the first byte past the window. Sixty-four
/// is for confidence rather than coverage: a single byte matching its pre-image by chance is ~1/256
/// for random data and far likelier for the zero-heavy data a `.pyc` or a freshly zeroed page
/// contains, and the consequence of a miss is a silently incomplete recording.
pub const GUARD_BAND: usize = 64;
```

And beside `clamp_count` in `impl Box_`:

```rust
/// Did the kernel write past the diff window? (M27)
///
/// Pure so the policy is reviewable apart from the `unsafe` slice plumbing that feeds it. The
/// caller guarantees both slices are the same region sampled before and after exactly one
/// `host_svc`, which is what makes a difference *proof* of a kernel write rather than evidence of
/// one: the guest vCPU is halted across that call and `clippy.toml` bans recorder threads, so
/// nothing else could have touched those bytes.
///
/// An empty band (the window already covered the whole backing) is never an overrun.
pub fn overran_window(pre_guard: &[u8], post_guard: &[u8]) -> bool {
    !pre_guard.is_empty() && pre_guard != post_guard
}
```

- [ ] **Step 4: Run the test and watch it pass.**

Run: `perl -e 'alarm 400; exec @ARGV' cargo test -p retrace-box --test truncguard -- --test-threads=1`
Expected: PASS, 3 tests.

- [ ] **Step 5: Plumb the band through `forward_and_diff`.** In `crates/retrace-box/src/lib.rs`,
  change the windows vector to carry the band, and take it in the arg loop. Replace:

```rust
        let mut windows: Vec<(u64, usize, Vec<u8>)> = Vec::new(); // (guest_ipa, len, pre-image)
```

with:

```rust
        // (guest_ipa, len, pre-image, pre-image of the M27 guard band past the window)
        let mut windows: Vec<(u64, usize, Vec<u8>, Vec<u8>)> = Vec::new();
```

and replace the body of the `Some((hp, avail))` arm:

```rust
                Some((hp, avail)) => {
                    // M26: not a flat cap. For a buffer-filling syscall the destination window
                    // must cover what the clamp below will let the kernel write, or the tail is
                    // written to guest memory and captured nowhere. See `diff_window`.
                    let win = Self::diff_window(num, i, avail, args[2] as usize);
                    let pre = unsafe { std::slice::from_raw_parts(hp, win) }.to_vec();
                    // M27: when the window is CAPPED there is backing beyond what the diff will
                    // look at, and a kernel write reaching into it would be captured nowhere.
                    // Snapshot a band immediately past the window so an overrun is provable rather
                    // than inferred. Bounded by `avail`, so this never reads past the backing.
                    let band = GUARD_BAND.min(avail - win);
                    let pre_band = unsafe { std::slice::from_raw_parts(hp.add(win), band) }.to_vec();
                    windows.push((args[i], win, pre, pre_band));
                    hargs[i] = hp as i64;
                }
```

- [ ] **Step 6: Check the band after the forward.** Replace the post-diff loop:

```rust
            for (ipa, len, pre, pre_band) in windows {
                // Take `avail` rather than discarding it: the guard-band read below is `unsafe` and
                // must stay inside this backing. The band was sized from the PRE-forward `avail`,
                // and a syscall that remapped guest memory could in principle shrink it — mmap /
                // munmap / mprotect are all intercepted upstream and never reach here, but re-
                // clamping costs nothing and does not rely on that staying true.
                let (hp, avail_now) = self.host_span(ipa).unwrap();
                let band = pre_band.len().min(avail_now.saturating_sub(len));
                let post = unsafe { std::slice::from_raw_parts(hp, len) };
                // M27 Task 1: WARNING ONLY. Task 3 turns this into a panic, and deliberately not
                // before: landing a fail-loud assert without first measuring what it fires on
                // across the whole gate is the unmeasured-supporting-fact trap this milestone
                // exists to avoid.
                let post_band = unsafe { std::slice::from_raw_parts(hp.add(len), band) };
                if Self::overran_window(&pre_band[..band], post_band) {
                    eprintln!("[M27 TRUNCATION] syscall {} wrote past its {}-byte diff window at \
                               ipa {:#x}: the bytes past it are captured in NO Event, so replay \
                               will restore stale data there",
                        num as i64, len, ipa);
                }
                if post != pre.as_slice() {
                    writes.push(Region { ipa, bytes: post.to_vec() });
                }
            }
```

- [ ] **Step 7: Confirm the negative control is silent.** `bigread_e2e`'s window is no longer capped
  after M26, so the band must never fire on it. If it does, the plumbing is wrong — not a discovery.

Run: `perl -e 'alarm 500; exec @ARGV' cargo test -p retrace --test bigread_e2e -- --test-threads=1 2>&1 | grep -a "M27 TRUNCATION"`
Expected: **no output**, and the test passes.

- [ ] **Step 8: Commit.**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/truncguard.rs
git commit -m "M27-truncguard t1: a guard band past the window, as a warning"
```

**Acceptance:** `truncguard` green (3 tests), `bigread_e2e` green and silent, clippy clean.

---

### Task 2: The blast-radius measurement

**No production code.** This task's deliverable is a list. It is the whole reason M26 did not land
the assert, and Task 3 must not begin until it is complete.

**Files:** none modified. Write findings into the commit message.

- [ ] **Step 1: Run the FULL chunked gate with the warning in place**, capturing every log. Use the
  chunking in Global Constraints; every chunk `--no-fail-fast`, exit code captured before any pipe.

- [ ] **Step 2: Collect every firing.**

```sh
grep -ah "M27 TRUNCATION" <every gate log> | sort | uniq -c | sort -rn
```

- [ ] **Step 3: Record `/bin/ps` separately**, since it is not in any gate:

```sh
perl -e 'alarm 300; exec @ARGV' cargo run -q -p retrace -- record-dyn /bin/ps -o /tmp/ps.bin
```
Expected, from M26's measurement: exactly one firing, on syscall **202** (`sysctl`).

- [ ] **Step 4: Classify each distinct firing** into one of exactly two buckets, and write the
  classification down:
  - **Knowable length** — the syscall's destination length is in a register or behind a guest
    pointer, and can be added to `dest_buffer`. Task 4 handles `sysctl`; anything else found here
    gets its own entry.
  - **Unknown length** — nothing measures where the length lives. **Do not guess one.** Park it,
    name the measurement owed, and say so in the close.

- [ ] **Step 5: Commit the finding.**

```bash
git commit --allow-empty -m "M27-truncguard t2: the blast-radius measurement"
```

**Acceptance:** every gate chunk `EXIT=0`, and a written list of distinct firings with each one
classified. An empty list is a legitimate result for the gate guests — `/bin/ps` fires regardless.

---

### Task 3: `pread_nocancel` (414), and the table

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (constant, `fd_operands`, `DestLen`, `dest_buffer`, tests)
- Modify: `crates/retrace-box/src/lib.rs` (`diff_window` and the clamp consult the table)

**Interfaces:**
- Consumes: `Box_::overran_window`, `GUARD_BAND` from Task 1.
- Produces: `retrace_arch::DestLen`, `retrace_arch::dest_buffer(num) -> Option<(usize, DestLen)>`,
  `retrace_arch::SYS_PREAD_NOCANCEL`. `Box_::writes_x2_bytes_to_x1` is **removed** — Task 4 relies
  on the table having replaced it.

- [ ] **Step 1: Write the failing tests.** In `crates/retrace-arch/src/lib.rs`'s test module:

```rust
    // M27: 414 was missing from THREE places at once — fd_operands, the forwarded-count clamp, and
    // the diff window — and the missing clamp is the serious one: an unclamped forward lets the
    // host kernel write past the guest buffer's backing. `fd_operands`' own doc comment already
    // states the rule this broke: "A plain-only table fails *silently*."
    #[test]
    fn pread_nocancel_is_treated_exactly_like_pread() {
        assert_eq!(SYS_PREAD_NOCANCEL, 414);
        assert_eq!(fd_operands(SYS_PREAD_NOCANCEL), fd_operands(SYS_PREAD));
        assert_eq!(dest_buffer(SYS_PREAD_NOCANCEL), dest_buffer(SYS_PREAD));
    }

    // The read family's length is a register. sysctl's is behind a guest pointer — the shape M26's
    // yes/no predicate could not express, and the reason this is a table.
    #[test]
    fn dest_buffer_knows_where_each_length_lives() {
        assert_eq!(dest_buffer(SYS_READ),     Some((1, DestLen::Reg(2))));
        assert_eq!(dest_buffer(SYS_PREAD),    Some((1, DestLen::Reg(2))));
        assert_eq!(dest_buffer(SYS_READ_NOCANCEL), Some((1, DestLen::Reg(2))));
        assert_eq!(dest_buffer(SYS_SYSCTL),   Some((2, DestLen::DerefU64(3))));
    }

    // Absence must mean "provably writes no buffer we can size", never "not gotten to yet".
    // fsgetpath takes an fsid_t* naming a VOLUME, not a descriptor and not a sized buffer; M25
    // pinned that and it must not silently reopen.
    #[test]
    fn dest_buffer_omits_what_it_should() {
        // 427 as a bare literal, matching how `fd_operands`' existing assertion pins it —
        // there is no SYS_FSGETPATH constant in this crate and this test must not invent one.
        assert_eq!(dest_buffer(427), None, "fsgetpath takes an fsid_t*, not a sized buffer");
        assert_eq!(dest_buffer(SYS_WRITE), None, "write reads the buffer, it does not fill it");
    }
```

- [ ] **Step 2: Run and watch them fail.**

Run: `perl -e 'alarm 300; exec @ARGV' cargo test -p retrace-arch -- --test-threads=1`
Expected: FAIL to compile — `cannot find value 'SYS_PREAD_NOCANCEL'`, `cannot find function 'dest_buffer'`.

- [ ] **Step 3: Add the constant, the type and the table** to `crates/retrace-arch/src/lib.rs`:

```rust
/// `pread_nocancel`. Header-derived (`sys/syscall.h`: `#define SYS_pread_nocancel 414`).
pub const SYS_PREAD_NOCANCEL: u64 = 414;

/// Where a destination buffer's byte length lives for a given syscall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestLen {
    /// The length is the value of register `x{n}`.
    Reg(usize),
    /// The length is a `u64` in GUEST MEMORY at the address in `x{n}` — `sysctl`'s `*oldlenp`.
    DerefU64(usize),
}

/// The destination buffer `num` fills, as `(argument index, where its length lives)`.
///
/// **The forwarded-count clamp and the diff window must both consult this**, which is why it is one
/// table rather than a predicate per shape. The clamp decides how many bytes the host kernel may
/// write into the guest buffer; the window decides how many are looked at afterwards and captured
/// as `Event::Syscall` writes. A disagreement between them is the M26 defect: the kernel writes past
/// what the diff inspects, the excess lands in guest memory on record and in no `Event`, and replay
/// restores stale bytes there — invisibly, because `(num, args)` still match.
///
/// **Seeded only with what is measured or SDK-verified.** Other syscalls are structurally capable of
/// overrunning (`getdirentries64`, `recvfrom`, `getfsstat64`, `proc_info`, `getattrlist`, `csops`)
/// and are deliberately ABSENT: none has been measured to do so, and the M27 guard band exists
/// precisely so they announce themselves instead of being guessed at. Absence means "not measured",
/// and the guard band is what makes that safe.
pub fn dest_buffer(num: u64) -> Option<(usize, DestLen)> {
    match num {
        SYS_READ | SYS_READ_NOCANCEL | SYS_PREAD | SYS_PREAD_NOCANCEL => Some((1, DestLen::Reg(2))),
        // sysctl(name, namelen, oldp, oldlenp, newp, newlen): the destination is x2 and its length
        // is `*(size_t*)x3`, in guest memory rather than a register. Measured via /bin/ps, whose
        // KERN_PROC_ALL buffer runs far past the 64 KiB window (M26).
        SYS_SYSCTL => Some((2, DestLen::DerefU64(3))),
        _ => None,
    }
}
```

Add `SYS_PREAD_NOCANCEL` to `fd_operands`' `&[0]` arm, beside `SYS_PREAD`.

- [ ] **Step 4: Run the arch tests and watch them pass.**

Run: `perl -e 'alarm 300; exec @ARGV' cargo test -p retrace-arch -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Point the box at the table.** In `crates/retrace-box/src/lib.rs`, **delete**
  `writes_x2_bytes_to_x1` entirely and replace `diff_window` with a method (it needs `&self` to
  read guest memory for `DerefU64`):

```rust
    /// The destination length `num` will fill at argument `i`, if the table knows it.
    ///
    /// Takes `args` as a parameter and stores nothing: `Box_` gets no new field for this. Reads
    /// guest memory for the `DerefU64` shape, which is the only reason it needs `&self`.
    fn dest_len_bytes(&self, num: u64, i: usize, args: &[u64; 8]) -> Option<usize> {
        match retrace_arch::dest_buffer(num) {
            Some((di, len)) if di == i => Some(match len {
                retrace_arch::DestLen::Reg(n) => args[n] as usize,
                // `*(size_t*)args[n]` — sysctl's oldlenp. A null or unmapped pointer would be a
                // guest bug; `read_u64` is the same accessor the rest of the box uses for guest
                // scalars, so it behaves identically to every other guest-memory read here.
                retrace_arch::DestLen::DerefU64(n) => self.read_u64(args[n]) as usize,
            }),
            _ => None,
        }
    }
```

Then:

```rust
    /// How many bytes of the region behind `args[i]` to snapshot for the pre/post memory diff.
    ///
    /// `PTR_WINDOW_CAP` is a heuristic and has been since M1: most pointer args name a struct whose
    /// size the box does not know. Where `dest_buffer` DOES know the length, widen to cover it;
    /// everything else keeps the 64 KiB heuristic, because widening unconditionally costs a
    /// pre-image copy on every pointer operand of every syscall (M8 measured that per-syscall diff
    /// time is not free). Never exceeds `avail`.
    fn diff_window(&self, num: u64, i: usize, avail: usize, args: &[u64; 8]) -> usize {
        let base = avail.min(PTR_WINDOW_CAP);
        match self.dest_len_bytes(num, i, args) {
            Some(len) => base.max(Self::clamp_count(avail, len)),
            None => base,
        }
    }
```

Update the call site in the arg loop to `self.diff_window(num, i, avail, &args)`, and update the
"Debt #1" clamp to use `dest_buffer` for its `Reg` shape rather than the deleted predicate.

- [ ] **Step 6: Run the box package and the negative control.**

Run: `perl -e 'alarm 560; exec @ARGV' cargo test -p retrace-box -- --test-threads=1`
Expected: PASS, and `memdiff.rs`'s M26 test still green — it is the regression guard for the shape
this task generalises.

- [ ] **Step 7: Commit.**

```bash
git add crates/retrace-arch/src/lib.rs crates/retrace-box/src/lib.rs
git commit -m "M27-truncguard t3: pread_nocancel, and a table instead of a predicate"
```

**Acceptance:** arch and box chunks green; `writes_x2_bytes_to_x1` is gone with no callers left.

---

### Task 4: `/bin/ps` records and replays

**Files:**
- Modify: `crates/retrace/tests/sysbin_e2e.rs`

**Interfaces:**
- Consumes: `dest_buffer`'s `SYS_SYSCTL` entry from Task 3.

- [ ] **Step 1: Write the failing gate.** Append to `crates/retrace/tests/sysbin_e2e.rs`:

```rust
// M27: /bin/ps was published in the README from M22 to M26 as "a genuine replay divergence — the
// oracle catching nondeterminism". That could not have been true: replay never EXECUTES a syscall,
// it applies recorded writes, so a process list cannot vary between the two runs. M22's own
// measurement document said "also not diagnosed" while the README stated it with confidence.
//
// It was the M26 truncation class. `ps` sizes a sysctl(KERN_PROC_ALL) buffer at roughly
// nproc * sizeof(struct kinfo_proc) — far past the old 64 KiB window — and replay diverged at
// ipa 0x700810091, 145 bytes past that window's end, holding zeros where the recording held data.
//
// This asserts ps actually records AND REPLAYS, not merely that the guard band stopped firing:
// M23's measurements left open "whether ps's divergence is one cause or several", so a quiet
// tripwire would not be evidence that the guest is correct.
#[test]
fn ps_records_and_replays() {
    if !std::path::Path::new("/bin/ps").exists() {
        eprintln!("SKIPPED ps_records_and_replays: /bin/ps not found. This gate did NOT run — it \
                   is not evidence of anything.");
        return;
    }
    let (rec, trace) = util::record_dynamic("/bin/ps");
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert!(!rec.stdout.is_empty(), "ps printed nothing; it should list at least its own process");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, rec.stdout, "replay stdout diverged from the recording");
}
```

- [ ] **Step 2: Run it.**

Run: `perl -e 'alarm 500; exec @ARGV' cargo test -p retrace --test sysbin_e2e -- --test-threads=1`
Expected with Task 3 in place: **PASS**. If it fails, that is R2 from the spec — `ps` has a second
cause. Do **not** loosen the assertion; measure the new failure and either fix it or park this test
with the verbatim evidence, per honest-gate discipline.

- [ ] **Step 3: Commit.**

```bash
git add crates/retrace/tests/sysbin_e2e.rs
git commit -m "M27-truncguard t4: /bin/ps records and replays, and was never nondeterminism"
```

**Acceptance:** `ps_records_and_replays` green, or parked with measured evidence and the README's
Apple-binary count left at 46 rather than optimistically raised.

---

### Task 5: Flip the warning to a panic, and refuse the nested-pointer family

**Do not start until Task 2's list is complete and every knowable firing is fixed.**

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (warning → `panic!`)
- Modify: `crates/retrace-arch/src/lib.rs` (`writes_via_nested_pointer`)
- Modify: `crates/retrace-core/src/lib.rs` (the refusal, in `record_box`'s generic `Stop::Syscall` arm)

**Interfaces:**
- Consumes: everything from Tasks 1–4.
- Produces: `retrace_arch::writes_via_nested_pointer(num) -> bool`.

- [ ] **Step 1: Write the failing test** in `crates/retrace-arch/src/lib.rs`'s test module:

```rust
    // M27: these put their destination behind a pointer INSIDE a guest struct (iovec.iov_base,
    // msghdr.msg_iov). forward_and_diff translates only top-level register arguments, so a guest
    // IPA would reach the host kernel AS A HOST ADDRESS. That is not a fidelity gap like the
    // truncation class — it is a potential wild write into retrace's own process.
    //
    // The reading that they would merely EFAULT (guest IPAs being unlikely to be mapped in
    // retrace's process) is an INFERENCE, and the downside of it being wrong is severe. So they are
    // refused by value rather than tested or translated, the way guest_workq_kernreturn refuses an
    // unenumerated opcode. Translating them properly needs the translate_mwl_regions treatment and
    // its own measurement.
    #[test]
    fn the_nested_pointer_family_is_named_in_full() {
        for num in [120u64, 411, 27, 401, 540, 480] {
            assert!(writes_via_nested_pointer(num), "syscall {num} must be refused");
        }
        for num in [SYS_READ, SYS_PREAD, SYS_SYSCTL, SYS_WRITE] {
            assert!(!writes_via_nested_pointer(num), "syscall {num} has top-level operands");
        }
    }
```

- [ ] **Step 2: Run and watch it fail.**

Run: `perl -e 'alarm 300; exec @ARGV' cargo test -p retrace-arch -- --test-threads=1`
Expected: FAIL to compile — `cannot find function 'writes_via_nested_pointer'`.

- [ ] **Step 3: Implement it** in `crates/retrace-arch/src/lib.rs`:

```rust
/// `readv`(120) / `readv_nocancel`(411) / `recvmsg`(27) / `recvmsg_nocancel`(401) /
/// `preadv`(540) / `recvmsg_x`(480). All header-derived from `sys/syscall.h`.
pub fn writes_via_nested_pointer(num: u64) -> bool {
    matches!(num, 120 | 411 | 27 | 401 | 540 | 480)
}
```

- [ ] **Step 4: Add the refusal.** In `crates/retrace-core/src/lib.rs`, in `record_box`'s generic
  `Stop::Syscall { num, args }` arm, beside the existing `is_signal_syscall` and `SYS_DUP2` asserts:

```rust
                // M27: the destination sits behind a pointer INSIDE a guest struct, which
                // forward_and_diff never translates — so forwarding hands the host kernel a guest
                // IPA as a host address. No guest in the gate calls these (measured: absent from
                // M25's 69-number CPython census), so refuse rather than model it wrong.
                assert!(!retrace_arch::writes_via_nested_pointer(num),
                    "syscall {num} writes through a nested guest pointer (iovec.iov_base / \
                     msghdr.msg_iov) and retrace translates only top-level register operands, so \
                     forwarding it would hand the host kernel a guest address. Translating it needs \
                     the translate_mwl_regions treatment plus a measurement of the struct layout. \
                     Implement that before a guest needs this; do not forward it.");
```

- [ ] **Step 5: Flip the guard band to a panic.** In `crates/retrace-box/src/lib.rs`, replace the
  Task 1 `eprintln!` with:

```rust
                assert!(!Self::overran_window(&pre_band, post_band),
                    "syscall {} wrote PAST its {}-byte diff window at ipa {:#x}. The bytes past it \
                     are captured in no Event, so the recording is silently incomplete and replay \
                     would restore stale data there — a failure the divergence oracle cannot see, \
                     because (num, args) match on both sides. Add this syscall's destination buffer \
                     to retrace_arch::dest_buffer with the argument its length lives in; if that \
                     length is not knowable, measure it before guessing.",
                    num as i64, len, ipa);
```

- [ ] **Step 6: Run the full chunked gate.** Every chunk `EXIT=0`. Any panic here is a Task 2 miss —
  go back, do not weaken the assert.

- [ ] **Step 7: Commit.**

```bash
git add crates/retrace-arch/src/lib.rs crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs
git commit -m "M27-truncguard t5: the class fails loud"
```

**Acceptance:** full gate green with the assert live; the nested-pointer family refused by value.

---

### Task 6: The gate and the two documents

- [ ] **Step 1: Merge `main` if it moved,** then run the full chunked gate per Global Constraints,
  including `--bins`.
- [ ] **Step 2: Reconcile file-by-file** against `main`'s actual close (M26: 515 / 0 / 2 over 114).
  Expected, to be confirmed rather than assumed: `retrace-box/tests/truncguard.rs` **+3 and +1
  binary**; `retrace-arch/src/lib.rs` **+4**; `retrace/tests/sysbin_e2e.rs` **+1**; `--bins`
  **unchanged**.
- [ ] **Step 3: clippy** clean over `--workspace --all-targets`.
- [ ] **Step 4: The two documents, which must not be merged.**
  - **README, edited in place.** Under "What works today", the truncation class now fails loud
    rather than silently. Update the Apple-binary count **only if Task 4 actually passed** (46 → 47)
    — and if it did not, leave 46 and say what `ps` is parked at. Rewrite the Known-limits entry:
    the residual table shrinks by `sysctl` and `pread_nocancel`, the nested-pointer family moves
    from "would hand the kernel a guest address" to "refused by value", and the guard band replaces
    "a tripwire was prototyped but not landed". Update the gate line.
  - **`docs/status-log.md`** — **append** a `## Status: M27-truncguard` section. Never rewrite M26's.
    Say what the blast-radius measurement found (Task 2's list is the interesting part, whatever it
    contained), that `ps` was misfiled for four milestones, and what is still standing:
    `diff_memory`'s `.min(avail)`, the `if !err` gate, and the unmeasured remainder of the audit
    table — which is now *guarded* rather than merely listed.
  - **CLAUDE.md** — only if a statement became false.
- [ ] **Step 5: State the outcome without hedging.** Say whether `ps` records and replays, and say
  what the guard band fires on today.

**Acceptance:** every chunk `EXIT=0`, clippy clean, total reconciled file-by-file, both documents
updated, `git status` clean apart from intended files.

---

## Sequencing

Task 1 → **2** → 3 → 4 → 5 → 6. Task 2 is a hard barrier: Task 5's panic must not land before the
measurement exists. Tasks 3 and 4 could swap only if `sysctl` were dropped, which would leave `ps`
panicking under Task 5.

## Self-Review

1. **`TRACE_MAGIC` unchanged** and no `Event` variant or field touched.
2. **`verify_thread` still has seven call sites** plus `mirror_delivery`'s inline eighth. M27 adds no
   dispatch arm that consumes a landmark — the refusal is an `assert!`, which produces no trace.
3. **`writes_x2_bytes_to_x1` is deleted, not orphaned.** `grep` returns no callers after Task 3.
4. **`dest_buffer` is seeded only with measured or SDK-verified entries.** If an entry appears for a
   syscall nothing measured, the guard band's whole purpose has been circumvented.
5. **`fsgetpath` (427) is still absent** from both `fd_operands` and `dest_buffer`, and a test pins
   it. M25 established it takes an `fsid_t*`; that must not silently reopen.
6. **`bigread_e2e` stays silent** under the guard band. A firing there is a bug in the plumbing, not
   a discovery.
7. **The `ps` gate asserts record AND replay**, not merely a quiet tripwire — R2 says `ps` may have
   more than one cause.
8. **No existing assertion was loosened.** If Task 5's gate panics, the fix is a `dest_buffer` entry
   or a park, never a weaker assert.
