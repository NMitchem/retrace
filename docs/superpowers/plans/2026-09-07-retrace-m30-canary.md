# M30-canary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the M27 guard band's measured false negative by filling the band with an address-derived canary before `host_svc` and verifying it after, so a kernel write past the diff window is detectable whatever bytes it writes.

**Architecture:** One seam, the record-side memory diff in `Box_::forward_and_diff`. `retrace-box` gains two pure predicates and a reordering: `band_not_covered` moves from the post-pass to the pre-pass so a canary can only ever land in bytes no window inspects. Nothing touches `retrace-trace`, `retrace-core`, or the replay side.

**Tech Stack:** Rust 1.95.0 (pinned, `aarch64-apple-darwin`), macOS 26.5 SDK, Hypervisor.framework.

**Spec:** `docs/superpowers/specs/2026-09-07-retrace-m30-canary-design.md`

## Global Constraints

- **`--test-threads=1` is mandatory** on every `cargo test`. HVF allows one VM per process; a bare `cargo test` flakes with `HV_BUSY`.
- **Never bump `TRACE_MAGIC`** (currently `RT\x00\x09`) and never change `Event`'s shape. No task here touches `crates/retrace-trace` or `crates/retrace-core`. If you believe you need to, stop and escalate.
- **`forward_and_diff` is record-side only.** An `assert!` mints no landmark, so nothing here owes a replay mirror. Do not add one.
- **`clippy.toml` denials are load-bearing:** no `Instant::now`/`SystemTime::now` (determinism), no `std::thread::Thread` (the recorder is single-threaded by design). Every task ends clippy-clean at `-D warnings`.
- **Do not weaken or delete an existing assertion** to make a new test pass. If an existing test fails, that is a finding — report it, do not edit around it.
- **`GUARD_BAND` stays 64.** Widening it is explicitly a non-goal.
- **A skipped test must announce itself** with a loud `eprintln!`. A silent skip reads as a green it did not earn.
- **Publish the gate figure only after the last commit that can change it.** M28 published at that step and then landed three more tests, leaving both documents stale.
- Commit messages end with:
  ```
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_015PUdcj3EfqPgDwrkqfHJEm
  ```

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `crates/retrace-box/src/lib.rs` | `canary_byte` + `canary_intact` predicates (T1); `band_not_covered` moved to the pre-pass (T3); fill/verify/restore + gated reporting (T4); the Phase B flip (T6) | 1,3,4,6 |
| `crates/retrace-box/tests/canary.rs` | **create** — unit tests for the pure predicates, mirroring `tests/clamp.rs` | 1 |
| `crates/retrace-box/tests/truncguard.rs` | the blind-half reproduction (T2) and the caught-half (T4) | 2,4 |
| `README.md`, `docs/status-log.md` | the close: edit in place / append-only | 7 |

---

## Task 1: The canary predicates, pure and tested

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (immediately after `deref_len_fits`)
- Create: `crates/retrace-box/tests/canary.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `Box_::canary_byte(ipa: u64) -> u8` and `Box_::canary_intact(band: &[u8], base_ipa: u64) -> bool`. Tasks 3–6 use both.

**Why pure:** the same reason `clamp_count`, `overran_window` and `deref_len_fits` are — the policy is reviewable, and testable, apart from the `unsafe` plumbing that feeds it. M29's fast-follow is the cautionary case: a boundary inlined into a call site was exercised by nothing, and the remedy its own status log prescribed could not have reached it.

- [ ] **Step 1: Write the failing tests**

Create `crates/retrace-box/tests/canary.rs`:

```rust
use retrace_box::Box_;

// The pattern must depend on the guest ADDRESS and nothing else. That is what makes two
// overlapping bands agree on every shared byte, so verification is order-independent.
#[test]
fn the_canary_is_a_pure_function_of_the_address() {
    assert_eq!(Box_::canary_byte(0x1000), Box_::canary_byte(0x1000));
    assert_ne!(Box_::canary_byte(0x1000), Box_::canary_byte(0x1001));
    // Never zero at ipa 0: an all-zero fill is the commonest accidental band content, and a
    // canary that matched it there would be blind in exactly the case this milestone exists for.
    assert_ne!(Box_::canary_byte(0), 0);
    // Nor all-ones, the other common uninitialised fill.
    assert_ne!(Box_::canary_byte(0), 0xFF);
}

#[test]
fn an_intact_canary_is_recognised() {
    let base = 0x4000u64;
    let band: Vec<u8> = (0..64).map(|i| Box_::canary_byte(base + i)).collect();
    assert!(Box_::canary_intact(&band, base));
}

#[test]
fn a_single_disturbed_byte_is_caught() {
    let base = 0x4000u64;
    for victim in [0usize, 1, 31, 63] {
        let mut band: Vec<u8> = (0..64).map(|i| Box_::canary_byte(base + i)).collect();
        band[victim] ^= 0xFF;
        assert!(!Box_::canary_intact(&band, base), "byte {victim} flipped but not caught");
    }
}

// THE case this milestone exists to close: the kernel writes zeros over what was already zeros.
// `overran_window` cannot see it; the canary must.
#[test]
fn zeros_written_over_the_band_are_caught() {
    let base = 0x4000u64;
    assert!(!Box_::overran_window(&[0u8; 64], &[0u8; 64]), "the old detector is blind here");
    assert!(!Box_::canary_intact(&[0u8; 64], base), "the canary must not be");
}

// Matches `overran_window`'s own rule: an empty band (the window covered the whole backing)
// is never an overrun.
#[test]
fn an_empty_band_is_never_disturbed() {
    assert!(Box_::canary_intact(&[], 0x4000));
}
```

- [ ] **Step 2: Run them and watch them fail**

```bash
cargo test -p retrace-box --test canary -- --test-threads=1
```

Expected: compile failure, `no function or associated item named 'canary_byte' found`. That is the right failure — the feature is missing, not a typo.

- [ ] **Step 3: Write the minimal implementation**

In `crates/retrace-box/src/lib.rs`, immediately after `deref_len_fits`:

```rust
    /// The canary byte belonging to guest address `ipa`. (M30)
    ///
    /// A pure function of the address, not a constant, for two reasons that are both load-bearing.
    /// A kernel that memsets a constant cannot reproduce a per-byte-varying pattern, so the
    /// commonest accidental write cannot forge an intact band. And because the value depends only
    /// on the address, two *bands* that legitimately overlap each other agree on every shared byte,
    /// which makes filling and verification order-independent — an index- or counter-derived
    /// pattern would not have that property.
    ///
    /// `^ 0xA5` so neither an all-zero nor an all-`0xFF` fill matches at `ipa = 0`; those are the
    /// two commonest uninitialised contents, and the zero case is the one M27 measured going
    /// undetected on `/bin/ps`.
    pub fn canary_byte(ipa: u64) -> u8 { (ipa as u8) ^ 0xA5 }

    /// Does `band` still hold the canary written for guest address `base_ipa` onward? (M30)
    ///
    /// This replaces the question `overran_window` asks. A comparison of before against after can
    /// only report a change that happened, so it is blind whenever the kernel writes bytes
    /// identical to what was already there — most often zeros over zeros. Checking against a
    /// pattern we placed ourselves has a signal to lose in every case.
    ///
    /// An empty band is never disturbed, matching `overran_window`'s own rule for the case where
    /// the window already covered the whole backing.
    pub fn canary_intact(band: &[u8], base_ipa: u64) -> bool {
        band.iter().enumerate().all(|(i, &b)| b == Self::canary_byte(base_ipa + i as u64))
    }
```

- [ ] **Step 4: Run the tests and watch them pass**

```bash
cargo test -p retrace-box --test canary -- --test-threads=1
cargo clippy -p retrace-box --all-targets -- -D warnings
```

Expected: 5 passed, 0 failed; clippy silent.

- [ ] **Step 5: Mutation-test the predicate**

The point of a detector is that it can fail. Apply each mutation to `canary_intact`, re-run, confirm a FAILURE, then revert byte-for-byte:

```bash
# `all` -> `any`   : expect FAILED
# `== b` -> `!= b` : expect FAILED
# body -> `true`   : expect FAILED
cargo test -p retrace-box --test canary -- --test-threads=1
git diff --stat   # must be empty after reverting
```

Record each result in your report. **Commit before mutating** so `git checkout --` restores only the mutation.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/canary.rs
git commit -m "M30-canary t1: the canary predicates, pure and mutation-tested

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015PUdcj3EfqPgDwrkqfHJEm"
```

---

## Task 2: Reproduce the false negative in a repo-owned test

**Files:**
- Modify: `crates/retrace-box/tests/truncguard.rs` (append)

**Interfaces:**
- Consumes: `retrace_guest::FILEIO`, `Box_::set_window_cap_for_test`, `Box_::overran_window`.
- Produces: nothing later tasks call. Task 4 adds the matching caught-half beside it.

**Why this task exists, and why it comes before the fix:** M27 measured the false negative on `/bin/ps` and recorded it in prose. Prose is not a gate. Until a repo-owned test *demonstrates* the blindness, "the canary closed it" rests on reasoning — and this milestone's three predecessors were each derailed by an instrument that could not fire. This test stays green forever: it documents why `canary_intact` exists.

**The measurement you must take first, rather than assume:** M28's positive control drives `SYS_FSTAT` with a 64-byte window cap and detects the overrun because `struct stat`'s bytes past 64 are non-zero. This task needs the opposite: a window boundary placed so the bytes the kernel writes past it are **zero**. On macOS `struct stat64` ends with `st_qspare[2]`, 16 bytes the kernel writes as zero — but the size and the offset are exactly the kind of fact that must be measured, not read off a header comment.

- [ ] **Step 1: Measure the fstat reply's layout**

Write a throwaway probe (do NOT commit it) that calls `fstat` on an open file and prints the reply's size and which trailing bytes are zero:

```c
#include <stdio.h>
#include <sys/stat.h>
#include <fcntl.h>
int main(void) {
    struct stat st; int fd = open("/etc/hosts", O_RDONLY);
    for (size_t i = 0; i < sizeof st; i++) ((unsigned char*)&st)[i] = 0xEE;
    fstat(fd, &st);
    printf("sizeof(struct stat) = %zu\n", sizeof st);
    const unsigned char *p = (const unsigned char*)&st;
    size_t first_zero_tail = sizeof st;
    while (first_zero_tail > 0 && p[first_zero_tail-1] == 0) first_zero_tail--;
    printf("trailing zero run starts at offset %zu (%zu bytes)\n",
           first_zero_tail, sizeof st - first_zero_tail);
    return 0;
}
```

Build with `clang -o /tmp/statprobe /tmp/statprobe.c && /tmp/statprobe`. **Pre-filling with `0xEE` matters**: it distinguishes "the kernel wrote zeros here" from "this byte was never written", which is the whole distinction the test turns on.

Record both numbers in your report. If the trailing zero run is shorter than 8 bytes, **stop and report** — the fixture cannot be built this way and the controller must rule.

- [ ] **Step 2: Write the test**

Append to `crates/retrace-box/tests/truncguard.rs`, substituting your measured offset for `CAP`:

```rust
// M30: a repo-owned reproduction of the false negative M27 measured on /bin/ps, and the reason
// `canary_intact` exists. The window cap is placed so the only bytes the kernel writes past the
// window are `struct stat`'s trailing zero field — written over a band that is already zero. The
// band therefore reads identically before and after a REAL kernel overrun, and `overran_window`,
// which can only report a change, has nothing to report.
//
// This asserts the BLINDNESS, and it stays green after M30 closes the hole: it documents the
// question the old predicate asks, not the answer the new one gives. Task 4 adds the matching
// caught-half.
#[test]
fn the_old_comparison_is_blind_to_zeros_written_over_zeros() {
    // Measured, not assumed — see the M30 plan, Task 2 Step 1.
    const CAP: usize = 128; // replace with the measured offset of the trailing zero run
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FILEIO).unwrap());
    let mut b = Box_::load(&loaded);
    b.set_window_cap_for_test(CAP);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_FSTAT => {
                let (hp, avail) = b.host_span_for_test(args[1]).expect("stat buffer is mapped");
                let win = CAP.min(avail);
                let band = retrace_box::GUARD_BAND.min(avail - win);
                let pre: Vec<u8> = unsafe { std::slice::from_raw_parts(hp.add(win), band) }.to_vec();
                b.forward_and_diff(num, args);
                let post: Vec<u8> = unsafe { std::slice::from_raw_parts(hp.add(win), band) }.to_vec();
                assert!(pre.iter().all(|&x| x == 0), "precondition: the band starts zeroed");
                assert!(!Box_::overran_window(&pre, &post),
                    "this test exists because the old detector is blind here; if it now fires, \
                     the fixture no longer reproduces the false negative and must be re-measured");
                return;
            }
            Stop::Syscall { num, args } => {
                let (ret, _e, _w) = b.forward_and_diff(num, args);
                b.set_x0_and_return(ret);
            }
            other => panic!("guest stopped with {other:?} before its fstat"),
        }
    }
}
```

- [ ] **Step 3: Run it**

```bash
cargo test -p retrace-box --test truncguard -- --test-threads=1
```

Expected: it may fail to compile if `host_span_for_test` does not exist. `host_span` is private. Add a minimal test seam beside `set_window_cap_for_test`, following that seam's existing doc-comment style:

```rust
    /// Test seam (M30): `host_span` is private, and the false-negative reproduction needs the same
    /// host pointer and `avail` the diff loop computes. `&self`, pure delegation, no production
    /// caller — the same posture as `diff_window_for_test`.
    pub fn host_span_for_test(&self, ipa: u64) -> Option<(*mut u8, usize)> { self.host_span(ipa) }
```

If the assertion fails instead — meaning the band was NOT all zeros, or the old detector DID fire — **stop and report**. Either means the fixture does not reproduce the case, and inventing a different `CAP` until it passes would be fitting the test to the answer.

- [ ] **Step 4: Confirm the whole file still passes**

```bash
cargo test -p retrace-box --test truncguard -- --test-threads=1
cargo clippy -p retrace-box --all-targets -- -D warnings
```

Expected: 16 passed (15 + this one), 0 failed; clippy silent.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/truncguard.rs
git commit -m "M30-canary t2: reproduce M27's false negative as a repo-owned test

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015PUdcj3EfqPgDwrkqfHJEm"
```

---

## Task 3: Move `band_not_covered` to the pre-pass

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (the pre-image loop and the post-syscall loop in `forward_and_diff`)

**Interfaces:**
- Consumes: `Box_::band_not_covered(ipa, len, band, others) -> usize`, unchanged.
- Produces: `windows` carries a fifth element, the shrunk band length: `Vec<(u64, usize, Vec<u8>, Vec<u8>, usize)>`. Task 4 fills and verifies exactly `band` bytes.

**This task changes no behaviour.** It is a pure reordering, isolated so a reviewer can reject it on its own. It is nonetheless the load-bearing part of the milestone: today's raw band is `GUARD_BAND.min(avail - win)` and may overlap another argument's *window*. Task 4 writes into the band, and a canary landing inside a window would either contaminate that window's post-image or, when restored, **erase a genuine kernel write from the recording**. Shrinking before filling makes both impossible by construction rather than by care.

- [ ] **Step 1: Build `spans` and the shrunk bands in the pre-pass**

In `forward_and_diff`, immediately after the existing `for i in 0..8` pre-image loop closes, insert:

```rust
        // M30: the band shrink moves here, BEFORE the syscall, because Task 4 writes a canary into
        // the band. A canary in the raw band could land inside another argument's window, and
        // restoring it afterwards would erase a genuine kernel write from the recording. Shrinking
        // first confines every canary to bytes no window inspects.
        //
        // Each entry's own span ends exactly where its band begins, so passing the whole list
        // (including self) is correct — see `band_not_covered`.
        let spans: Vec<(u64, usize)> =
            windows.iter().map(|(ipa, len, _, _, _)| (*ipa, *len)).collect();
        for w in windows.iter_mut() {
            let (ipa, len, _, pre_band, band) = w;
            *band = Self::band_not_covered(*ipa, *len, pre_band.len(), &spans);
        }
```

Change the declaration and the `push` in the pre-image loop to carry the new field:

```rust
        // (guest_ipa, len, pre-image, pre-image of the M27 guard band past the window, shrunk band)
        let mut windows: Vec<(u64, usize, Vec<u8>, Vec<u8>, usize)> = Vec::new();
        // ... and in the loop, seeded with 0 and filled in by the shrink pass above:
        windows.push((args[i], win, pre, pre_band, 0));
```

- [ ] **Step 2: Consume it in the post-syscall loop**

Replace the post-syscall `spans`/`raw_band`/`band` computation with the value already carried. The `avail_now` re-clamp stays — it is documented as defensive against a future edit, and this task is exactly the kind of edit it guards against:

```rust
            for (ipa, len, pre, pre_band, band) in windows {
                let (hp, avail_now) = self.host_span(ipa).unwrap();
                // Re-clamp defensively: nothing between the pre-pass and here mutates
                // `self.backings`, so this is a no-op today.
                let band = band.min(avail_now.saturating_sub(len));
                let raw_band = pre_band.len().min(avail_now.saturating_sub(len));
```

Everything below — the `[M28 BANDSHRINK]` report, the `overran_window` assert, the write capture — is unchanged.

- [ ] **Step 3: Prove the reordering changed nothing**

```bash
cargo test -p retrace-box -- --test-threads=1
cargo test -p retrace --test sysbin_e2e --test bigread_e2e --test jq_e2e -- --test-threads=1
cargo clippy -p retrace-box --all-targets -- -D warnings
```

Expected: identical counts to Task 2's close (16 in `truncguard`, `retrace-box` whole-package green), `sysbin_e2e` 3 passed. `sysbin_e2e` matters specifically: it asserts the `[M28 BANDSHRINK]` count is `> 0`, so a reordering that broke the shrink would show up there rather than silently.

- [ ] **Step 4: Commit**

```bash
git add crates/retrace-box/src/lib.rs
git commit -m "M30-canary t3: shrink the guard band before the syscall, not after

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015PUdcj3EfqPgDwrkqfHJEm"
```

---

## Task 4: Fill, verify, restore — Phase A

**Files:**
- Modify: `crates/retrace-box/src/lib.rs`
- Modify: `crates/retrace-box/tests/truncguard.rs` (append the caught-half)

**Interfaces:**
- Consumes: `canary_byte`/`canary_intact` (T1), the pre-pass shrunk band (T3).
- Produces: a `[M30 CANARY]` stderr line per disturbance, gated behind `RETRACE_CANARY`.

**The gate covers the reporting, never the fill.** The canary is written and verified unconditionally, because Phase B must flip on a measurement of the path production actually takes. Gating the fill would measure a path nothing runs — the dead-channel trap M29 hit twice. Phase A's inertness is evidenced by the sweep tally not moving, not by the code being switched off.

- [ ] **Step 1: Fill the canary in the pre-pass**

Immediately after the shrink loop from Task 3:

```rust
        // M30: give the band a signal it can lose. Sound because the guest vCPU is halted across
        // `host_svc` and `clippy.toml` bans recorder threads, so nothing but the kernel can touch
        // these bytes in the interval; they are restored below before the vCPU resumes, so no guest
        // can observe them and nothing reaches the trace.
        for (ipa, len, _, _, band) in windows.iter() {
            let (hp, _) = self.host_span(*ipa).expect("pre-pass established this mapping");
            let base = *ipa + *len as u64;
            for k in 0..*band {
                unsafe { *hp.add(*len + k) = Self::canary_byte(base + k as u64) };
            }
        }
```

- [ ] **Step 2: Verify and restore in the post-syscall loop**

Immediately after `let band = band.min(...)` from Task 3, and **before** the existing `overran_window` assert:

```rust
                // M30 Phase A: report only. The flip to fail-loud is Phase B and is conditional on
                // this measuring zero — landing a panic without that measurement is the trap M27's
                // own status log names.
                let post_band_now = unsafe { std::slice::from_raw_parts(hp.add(len), band) };
                let disturbed = !Self::canary_intact(post_band_now, ipa + len as u64);
                if disturbed && std::env::var_os("RETRACE_CANARY").is_some() {
                    eprintln!("[M30 CANARY] syscall {} disturbed the {}-byte band past its \
                               {}-byte window at ipa {:#x}",
                        num as i64, band, len, ipa);
                }
                // Restore BEFORE the window comparison below, which must see true guest bytes.
                // Only `band` bytes were overwritten, so only `band` are restored.
                unsafe { std::ptr::copy_nonoverlapping(pre_band.as_ptr(), hp.add(len), band) };
```

- [ ] **Step 3: Write the caught-half of the positive control**

Append to `crates/retrace-box/tests/truncguard.rs`:

```rust
// M30: the other half of Task 2's reproduction. Same guest, same window cap, same real kernel
// overrun — but asked the new question. If this ever fails while
// `the_old_comparison_is_blind_to_zeros_written_over_zeros` still passes, the canary has stopped
// being written or stopped being checked, and the milestone's headline claim is false.
#[test]
fn the_canary_catches_zeros_written_over_zeros() {
    const CAP: usize = 128; // same measured value as Task 2
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FILEIO).unwrap());
    let mut b = Box_::load(&loaded);
    b.set_window_cap_for_test(CAP);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_FSTAT => {
                let (hp, avail) = b.host_span_for_test(args[1]).expect("stat buffer is mapped");
                let win = CAP.min(avail);
                let band = retrace_box::GUARD_BAND.min(avail - win);
                b.forward_and_diff(num, args);
                // forward_and_diff restores the band, so re-derive what the canary check saw:
                // the kernel's write landed in the band, so the canary cannot still be intact.
                let restored: Vec<u8> =
                    unsafe { std::slice::from_raw_parts(hp.add(win), band) }.to_vec();
                assert!(restored.iter().all(|&x| x == 0),
                    "the band must be restored to its pre-syscall bytes before the guest resumes");
                return;
            }
            Stop::Syscall { num, args } => {
                let (ret, _e, _w) = b.forward_and_diff(num, args);
                b.set_x0_and_return(ret);
            }
            other => panic!("guest stopped with {other:?} before its fstat"),
        }
    }
}
```

**Note the asymmetry deliberately:** this test asserts the *restore*, because Phase A only reports and a report cannot be observed from inside the box. The claim that the canary FIRED is Task 5's measurement, taken through `RETRACE_CANARY`. Task 6 converts it into an assertion this test can then make directly.

- [ ] **Step 4: Run everything the change can touch**

```bash
cargo test -p retrace-box -- --test-threads=1
cargo test -p retrace --test sysbin_e2e --test bigread_e2e --test cpython_e2e --test jq_e2e -- --test-threads=1
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `truncguard` 17, `retrace-box` whole-package green, the four e2e targets green, clippy silent. **`bigread_e2e` and `cpython_e2e` are the ones that matter**: they drive large real buffers through the diff, which is where a restore bug would corrupt a recording.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/truncguard.rs
git commit -m "M30-canary t4: fill, verify and restore the band — Phase A, report only

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015PUdcj3EfqPgDwrkqfHJEm"
```

---

## Task 5: Phase A — measure it

**Files:** none. **The deliverable is a measurement**, and Task 6's branch is decided entirely by the number it produces.

- [ ] **Step 1: Prove the channel can carry a signal, before believing any zero from it**

M29's Task 4 reported a corpus-wide zero from a sweep whose stderr was discarded before the grep ran; the number was structurally incapable of being anything else. So establish the positive control **first**:

```bash
cargo build -p retrace
RETRACE_CANARY=1 cargo test -p retrace-box --test truncguard \
  the_canary_catches_zeros_written_over_zeros -- --test-threads=1 --nocapture 2>&1 \
  | grep -ac "\[M30 CANARY\]"
```

Expected: **non-zero**. If this is 0, the channel is dead and every measurement below is meaningless — stop and report.

- [ ] **Step 2: Measure the Apple sweep**

```bash
RETRACE_CANARY=1 tools/apple-sweep.sh > /tmp/m30-phaseA-sweep.txt 2>&1
grep -ac "\[M30 CANARY\]" /tmp/m30-phaseA-sweep.txt
grep -a  "\[M30 CANARY\]" /tmp/m30-phaseA-sweep.txt | sort | uniq -c | sort -rn | head -20
tail -1 /tmp/m30-phaseA-sweep.txt
```

Run it in the background; it takes ~5 minutes over 54 record+replay pairs, and **an alarm-kill is not a red** — this repo's `CLAUDE.md` says so. The tally must still read `TALLY pass=46 fail=8 skip=0`: Phase A writes and restores on every recording, so a moved tally means the restore is wrong, which is a finding, not a number to massage.

- [ ] **Step 3: Measure the dynamic guests**

```bash
S=/tmp/m30-phaseA
RETRACE_CANARY=1 cargo run -p retrace -- record-dyn /bin/ps -o /tmp/ps.bin > $S-ps.out 2>&1
grep -ac "\[M30 CANARY\]" $S-ps.out
for g in /opt/homebrew/bin/jq /opt/homebrew/bin/python3; do
  if [ -x "$g" ]; then
    RETRACE_CANARY=1 cargo run -p retrace -- record-dyn "$g" -o /tmp/g.bin -- --version > $S-$(basename $g).out 2>&1
    echo "$g: $(grep -ac '\[M30 CANARY\]' $S-$(basename $g).out)"
  else
    echo "SKIPPED $g — NOT PRESENT. This part of the measurement did NOT run."
  fi
done
```

`/bin/ps` is the binary M27 measured the false negative on, so it is the most likely to fire.

- [ ] **Step 4: Report per part, never as one sum**

For each part: the count, the exact command, and for any non-zero count the full `[M30 CANARY]` lines. State plainly whether any part was SKIPPED — **a skipped part is not a zero.** State the sweep tally and whether it moved.

A zero is a good result. Do not manufacture a non-zero one, and do not soften a zero into "probably non-zero elsewhere."

---

## Task 6: Phase B — flip, if and only if Phase A measured zero

**Files:**
- Modify: `crates/retrace-box/src/lib.rs`
- Modify: `crates/retrace-box/tests/truncguard.rs`

**READ THIS FIRST — the branch:**

- **Phase A measured ZERO across every part that ran → Branch B-FLIP.** Do the steps below.
- **Phase A measured NON-ZERO anywhere → Branch B-CHASE.** Do **not** flip. Each occurrence is a kernel write past everything the diff inspected — a real finding. Write up each one: the syscall, the ipa, the band, and what the destination is. That write-up is the deliverable, and the README's hedge stays. Do not half-flip "behind a flag": an assertion nobody reaches is a claim nobody checked.

### Branch B-FLIP

- [ ] **Step 1: Replace the comparison with the canary check**

Replace the existing `overran_window` assert with one driven by `disturbed` from Task 4. Keep the message's substance — it is the only thing a person hitting this will read — and add what the canary makes newly true:

```rust
                assert!(!disturbed,
                    "syscall {} wrote into the {}-byte guard band past its {}-byte diff window at \
                     ipa {:#x}. The band is filled with a known pattern before the call and checked \
                     after, so this is proof of a kernel write whatever bytes it wrote — including \
                     zeros over zeros, which the pre/post comparison this replaced could not see. \
                     The band already excludes every byte any OTHER window of this same call \
                     inspects (see `band_not_covered`). Add this syscall's destination buffer to \
                     retrace_arch::dest_buffer with the argument its length lives in; if that \
                     length is not knowable, measure it before guessing.",
                    num as i64, band, len, ipa);
```

`overran_window` stays as a pure predicate: Task 2's blindness test is its remaining caller and documents why it was replaced.

- [ ] **Step 2: Strengthen the caught-half to assert the panic**

Now that a disturbance aborts, Task 4's test can make the claim directly. Replace its body's restore-assertion with a `should_panic`, following `an_oldlenp_past_its_backing_is_refused`'s shape — pin a substring unique to the new message, and keep a sentinel that shares **no** substring with it so `should_panic` cannot be satisfied by the wrong panic:

```rust
#[test]
#[should_panic(expected = "wrote into the")]
fn the_canary_catches_zeros_written_over_zeros() {
    // ... same setup ...
            Stop::Syscall { num, args } if num == retrace_arch::SYS_FSTAT => {
                b.forward_and_diff(num, args);
                panic!("NOT-THE-CANARY: fstat wrote zeros past the window and nothing fired");
            }
    // ...
}
```

- [ ] **Step 3: Run the full local set**

```bash
cargo test -p retrace-box -- --test-threads=1
cargo test -p retrace --test sysbin_e2e --test bigread_e2e --test cpython_e2e --test jq_e2e -- --test-threads=1
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 4: Re-run the sweep with the assertion live**

```bash
tools/apple-sweep.sh > /tmp/m30-sweep-after-flip.txt 2>&1
tail -1 /tmp/m30-sweep-after-flip.txt
```

Expected `TALLY pass=46 fail=8 skip=0`. A moved tally means the assertion fires on a real Apple binary — which would contradict Phase A and is a finding to report, not a number to massage.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/truncguard.rs
git commit -m "M30-canary t6: flip the band to fail-loud, measured first

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015PUdcj3EfqPgDwrkqfHJEm"
```

---

## Task 7: The gate and the two documents

**Files:**
- Modify: `README.md`
- Modify: `docs/status-log.md` (**append only**)

- [ ] **Step 1: Run the full gate, chunked**

The whole workspace exceeds the tool ceiling. Capture each exit code **before any pipe** and use `--no-fail-fast`:

```bash
cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1; echo "EXIT=$?"
cargo test -p retrace-box --no-fail-fast -- --test-threads=1; echo "EXIT=$?"
cargo test -p retrace --bins --no-fail-fast -- --test-threads=1; echo "EXIT=$?"
```

Then the `retrace` e2e targets in groups of at most 11 `--test` flags, then:

```bash
cargo clippy --workspace --all-targets -- -D warnings; echo "EXIT=$?"
```

**Do not omit the `--bins` chunk** — the 11 unit tests in `crates/retrace/src/debug.rs` run in no other chunk and nothing warns you. **Run `retrace-box` as a whole package**, never split per-target, or its `Doc-tests` harness is silently dropped (that cost M24 a binary).

- [ ] **Step 2: Reconcile file-by-file, not by sum**

The M29 fast-follow closed at **538 passed / 0 failed / 2 ignored over 116 binaries**. Expected deltas:

| file | delta | from |
|---|---|---|
| `crates/retrace-box/tests/canary.rs` | **+5, and +1 binary** | Task 1 |
| `crates/retrace-box/tests/truncguard.rs` | +2 | Tasks 2, 4 |
| everything else | 0 | — |

So **545 over 117** under B-FLIP. `canary.rs` is a NEW test target, so unlike M29 the binary count moves. Verify by diffing `#[test]` counts against `git show main:<file>`, not by trusting the sum. Grep gate logs with `grep -a` — they carry ANSI and UTF-8 that trips plain grep.

- [ ] **Step 3: Edit the README in place**

The README says what is true **now**, so edit; never add a "superseded" note.

1. The truncation-guard paragraph currently says the band is "**still not proof the class is gone**" and points at the measured false negative. Under B-FLIP that hedge changes: state what the canary makes true, and keep the narrower residual honestly — a kernel write that reproduces the pattern exactly is still undetectable in principle (spec R2). Under B-CHASE the hedge stands and gains what Phase A found.
2. Update the gate line and its reconciliation with Step 2's numbers, including the binary count moving to 117.
3. Note that `GUARD_BAND` is unchanged at 64: this milestone changed the *kind* of detector, not its size.

- [ ] **Step 4: Append to the status log**

Add a new `## Status: M30-canary` section at the end and modify **no earlier section**. Cover: why a comparison could not see zeros over zeros; the canary and why the pattern is address-derived (anti-coincidence *and* overlap consistency); why `band_not_covered` had to move before the fill; the Phase A measurement per corpus part with the commands that produced it; which branch Task 6 took; the gate; and what stays owed.

- [ ] **Step 5: Verify the append-only discipline held**

```bash
git diff main -- docs/status-log.md | grep -a "^-" | grep -av "^---"
```

Expected: **no output**. Any deleted line means an earlier section was edited — fix it before committing.

- [ ] **Step 6: Commit**

```bash
git add README.md docs/status-log.md
git commit -m "M30-canary t7: the gate, and a band that can lose a signal

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015PUdcj3EfqPgDwrkqfHJEm"
```

---

## Self-Review

**Spec coverage.** Component 1 (the canary) → Tasks 1, 3, 4. Component 2 (the positive control) → Tasks 2 and 4, split into the blind half and the caught half so the closure is demonstrated rather than asserted. Component 3 (measure then flip) → Tasks 5 and 6, both branches written. Goals 1–3 → Tasks 4, 2+4, 5. Risks: R1 → Task 3 (shrink before fill, by construction); R2 → Task 7 Step 3 (stated, not claimed away); R3 → Task 6 Step 1 (the assert fires after the restore, so the abort path leaves guest memory clean — a deliberate consequence of the ordering, worth stating in the status log); R4 → Task 6's B-CHASE branch; R5 → Task 5 Step 2 (the sweep is the timing check). Non-goals are enforced by the Global Constraints block.

**Type consistency.** `canary_byte(ipa: u64) -> u8` and `canary_intact(band: &[u8], base_ipa: u64) -> bool` are defined in Task 1 and used with those signatures in Tasks 1, 4, 6. `host_span_for_test(&self, ipa: u64) -> Option<(*mut u8, usize)>` is added in Task 2 Step 3 and used in Tasks 2 and 4. `windows` gains its fifth element in Task 3 and is destructured with five fields in Tasks 3 and 4. `band_not_covered`'s signature is unchanged.

**Facts the plan rests on, verified against the tree rather than assumed** (this milestone's own subject, applied to its plan): `GUARD_BAND` is already `pub` at `crates/retrace-box/src/lib.rs:116`, so the tests can name it. `set_window_cap_for_test(&mut self, cap: usize)` exists at `:2922`. `SYS_FSTAT` is `189` (distinct from `SYS_FSTAT64`, `339` — Task 2 must drive the same one M28's positive control does). `fstat` is **absent** from `dest_buffer`, which is what makes it usable here at all: a `dest_buffer` entry would widen its window via `diff_window`'s `base.max(clamp_count(...))` to cover the whole reply, and it could then never overrun its own window. `tests/clamp.rs` opens with `use retrace_box::Box_;` — the import style Task 1's new file follows.

**Known imprecision, stated rather than hidden.** Task 2's `CAP = 128` is a placeholder for a value Step 1 measures. It is written as a constant with a comment saying so, and the task says to stop and report rather than tune it until the test passes — fitting the constant to the answer would produce a test that proves nothing.
