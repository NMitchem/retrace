# M28-bandproof Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development (recommended)
> or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax. Each task is dispatchable on
> its own to an implementer who has read nothing else.

**Goal:** Make the M27 guard band trustworthy — prove it can fire, define what a firing means, and
measure the one path where it is switched off.

**Architecture:** Three record-side components. (1) A **test seam** on the diff-window cap lets a
test give the band a real kernel overrun of a real syscall (`fstat`, deliberately absent from
`dest_buffer`) — the positive control the detector has never had. (2) A **band shrink** excludes
bytes another argument's window already covers, which makes "proof of an uninspected write" true by
construction instead of softened in prose. (3) A **purpose-built guest** provokes a failing `sysctl`
so the `if !err` path — where the band is disabled — is measured rather than suspected.

**Tech Stack:** Rust 1.95.0, `aarch64-apple-darwin`, macOS 26 on Apple Silicon,
Hypervisor.framework, freestanding arm64 asm guests built by `clang -nostdlib -static`.

**Spec:** `docs/superpowers/specs/2026-09-06-retrace-m28-bandproof-design.md`

**Branch:** `m28-bandproof`, cut from `main` at `fb79298` (M27 close: 523 / 0 / 2 over 115).

## Global Constraints

- **Never run cargo with a background flag.** Bounded foreground only; macOS has no `timeout(1)`:
  ```sh
  perl -e 'alarm 500; exec @ARGV' cargo test -p retrace-box -- --test-threads=1
  ```
- **`--test-threads=1` is mandatory.** HVF allows one VM per process; a bare `cargo test` flakes
  with `HV_BUSY`.
- **The gate is chunked.** Capture cargo's exit code **before any pipe** — this shell is zsh, where
  `${PIPESTATUS[0]}` is empty; the array is `$pipestatus` and it is 1-indexed. Run
  `cargo test -p retrace-box` as a **whole package** (a per-target split silently drops its
  `Doc-tests` harness), and split `-p retrace` into explicit `--test` sets. **Never omit `--bins`** —
  `--test <name>` selects integration targets only, so the 11 unit tests in
  `crates/retrace/src/debug.rs` run in no other chunk. `cargo test -p retrace-box --lib` is invalid
  and fails loudly; the trap is that the wrong flag is loud and the missing one is silent.
- **Grep test logs with `grep -a`** — they carry ANSI and UTF-8 that trips plain grep.
- **Reconcile the total file-by-file against `main`'s actual close** (M27: 523 / 0 / 2 over 115). Do
  not hard-code a baseline; read it off `main` when you get there.
- **`TRACE_MAGIC` does NOT move.** M28 adds no `Event` variant or field. If you believe you need a
  format change, stop — that is a spec deviation.
- **No replay mirror is owed.** `forward_and_diff` never runs on replay and an `assert!` produces no
  trace record, so nothing here needs an arm in `ReplaySession::advance`. Do not add one.
- **clippy clean** at `-D warnings` over `--workspace --all-targets`.
- **Codesigning.** A test that spawns `CARGO_BIN_EXE_retrace` bypasses cargo's signing runner; use
  `crates/retrace/tests/util/mod.rs::bin()`. **First execution of a freshly signed binary can stall
  for minutes at 0:00.00 CPU** — that is Gatekeeper validation, not a hang. Do not kill it; re-run
  and use the second timing. A cold `target/` multiplies this per binary.
- **`RETRACE_TRACE=1` is record-only.** `ReplaySession` carries no trace instrumentation.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/retrace-box/src/lib.rs` | The `window_cap` field + its test-only setter; `diff_window` reading the field; `Box_::band_not_covered`; the band-shrink wiring in `forward_and_diff`'s post-forward loop. |
| `crates/retrace-box/tests/truncguard.rs` | The positive control, and `band_not_covered`'s unit tests. The band's own test home. |
| `crates/retrace-guest/asm/failsysctl.s` | **Create.** A guest whose `sysctl` fails with an undersized buffer. |
| `crates/retrace-guest/build.rs` | Builds the new guest. |
| `crates/retrace-guest/src/lib.rs` | `FAILSYSCTL` path constant. |
| `crates/retrace-box/tests/failwrite.rs` | **Create.** The `if !err` measurement, then (conditionally) its assertion. |
| `crates/retrace/tests/failsysctl_e2e.rs` | **Create, conditionally (Task 5).** End-to-end gate, only if Task 4 measures a write. |
| `README.md`, `docs/status-log.md` | The two documents at close. |

---

### Task 1: The positive control

**This is the milestone's reason for existing.** The bar is not "the test passes" — it is that
`let band = 0;` **fails** it. That mutation passes the entire current 523-test gate.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (`window_cap` field, setter, `diff_window`, 4 constructors)
- Modify: `crates/retrace-box/tests/truncguard.rs` (the control)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `Box_::set_window_cap_for_test(&mut self, cap: usize)`, and a `window_cap` field whose
  default is `PTR_WINDOW_CAP`.

- [ ] **Step 1: Measure what `fstat` actually writes.** Do NOT trust "about 144". Run:

```sh
cat > /tmp/statsize.c <<'EOF'
#include <sys/stat.h>
#include <stdio.h>
int main(void){ printf("sizeof(struct stat) = %zu\n", sizeof(struct stat)); return 0; }
EOF
clang -arch arm64 -o /tmp/statsize /tmp/statsize.c && /tmp/statsize
```

Write the number down; it goes in the test's comment as the measured justification. Pick the test
cap as **64** if the measured size is comfortably above it (it should be); if the measured size is
64 or less, stop and report — the whole approach needs a different syscall.

- [ ] **Step 2: Write the failing test.** Append to `crates/retrace-box/tests/truncguard.rs`:

```rust
// M28: the POSITIVE control. Everything else touching the guard band is a NEGATIVE control —
// `bigread_e2e` and `memdiff`'s M26 guard prove it does not FALSE-fire. Nothing proved it fires at
// all. `overran_window`'s unit tests above cover `!pre.is_empty() && pre != post`, the one part
// that cannot be wrong; the offset (`hp.add(win)`), the sizing (`GUARD_BAND.min(avail - win)`) and
// whether the assert is REACHED were covered by nothing. `let band = 0;` passed the entire
// 523-test gate identically to the shipped code, and the M27 band has never been observed to fire
// on a real syscall — M26's "fired exactly once" was the tail-of-window PROTOTYPE, a different
// detector, and /bin/ps was measured NOT to trip this one.
//
// `fstat` is the right syscall here and `read` is the wrong one. `read` is in
// `retrace_arch::dest_buffer`, so `diff_window` widens its window to the full byte count no matter
// how small the cap is, and this test would be vacuously green. `fstat` is deliberately absent from
// that table (its length is not in a register), so a shrunken cap really does truncate it.
//
// MEASURED: `sizeof(struct stat)` is <FILL IN FROM STEP 1> bytes on this SDK, so a 64-byte window
// is genuinely overrun by a real kernel write.
#[test]
#[should_panic(expected = "changed a byte in the")]
fn the_band_fires_when_the_kernel_writes_past_the_window() {
    const CAP: usize = 64;
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FILEIO).unwrap());
    let mut b = Box_::load(&loaded);
    b.set_window_cap_for_test(CAP);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_FSTAT => {
                b.forward_and_diff(num, args);
                // Deliberately worded to share NO substring with the assert's message, so
                // `should_panic` cannot be satisfied by this panic instead of the real one.
                panic!("NOT-THE-GUARD-BAND: fstat wrote past a {CAP}-byte window and nothing fired");
            }
            Stop::Syscall { num, args } => {
                let (ret, _e, _w) = b.forward_and_diff(num, args);
                b.set_x0_and_return(ret);
            }
            Stop::Other { esr } => panic!("unexpected exit esr=0x{esr:x}"),
            Stop::Fault { pc, esr, far } => panic!("guest crashed pc=0x{pc:x} esr=0x{esr:x} far=0x{far:x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}
```

Replace `<FILL IN FROM STEP 1>` with the measured number.

- [ ] **Step 3: Run it and watch it fail.**

Run: `perl -e 'alarm 400; exec @ARGV' cargo test -p retrace-box --test truncguard -- --test-threads=1`
Expected: FAIL to compile — `no method named 'set_window_cap_for_test'`.

- [ ] **Step 4: Add the field.** In `crates/retrace-box/src/lib.rs`, append to the `Box_` struct,
  AFTER `fall_throughs` (the struct's field order is load-bearing — `vcpu` must precede `vm` — and
  appending a `Drop`-free scalar last preserves it):

```rust
    /// M28: the diff-window cap, defaulting to `PTR_WINDOW_CAP`. A FIELD rather than the bare
    /// constant so a test can shrink it and hand the guard band a REAL kernel overrun to detect.
    /// Before M28 the band had no positive control at all: `let band = 0;` passed the whole gate.
    /// Production never writes this except at construction. Plain `usize` (no Drop), declared last,
    /// so the load-bearing vcpu-before-vm drop order is unaffected. Deliberately NOT carried in
    /// `BoxState`: it is always `PTR_WINDOW_CAP` in production, so `from_checkpoint` restoring the
    /// default is correct rather than lossy.
    window_cap: usize,
```

- [ ] **Step 5: Initialise it in all four constructors.** There are exactly four literal
  `Box_ { … }` constructions in the file (`load_with_pac`, `load_dynamic`, `restore`,
  `from_checkpoint`). Add `window_cap: PTR_WINDOW_CAP` to each. Verify you found them all:

```sh
grep -c "window_cap: PTR_WINDOW_CAP" crates/retrace-box/src/lib.rs   # must print 4
```

- [ ] **Step 6: Add the setter.** In `impl Box_`, beside `overran_window`:

```rust
    /// Test seam (M28). Shrinks the diff-window cap so a syscall the `dest_buffer` table does not
    /// know can be made to overrun its window on purpose. **Production never calls this.**
    ///
    /// Shrinking the cap does NOT weaken a `dest_buffer` widening — `diff_window` takes the MAX of
    /// this base and the table's length, so a known length still widens past a small cap. That is
    /// exactly why the positive control uses `fstat` (absent from the table) rather than `read`.
    pub fn set_window_cap_for_test(&mut self, cap: usize) { self.window_cap = cap; }
```

- [ ] **Step 7: Point `diff_window` at the field.** Replace `let base = avail.min(PTR_WINDOW_CAP);`
  with `let base = avail.min(self.window_cap);`. (`diff_window` is already `&self` as of M27.)

- [ ] **Step 8: Run the test and watch it pass.**

Run: `perl -e 'alarm 400; exec @ARGV' cargo test -p retrace-box --test truncguard -- --test-threads=1`
Expected: PASS, 4 tests (3 pre-existing + the new one).

- [ ] **Step 9: THE MUTATION CHECK — the step this task exists for.** Temporarily change the band
  computation in `forward_and_diff`'s post-forward loop to `let band = 0;` and re-run:

Run: `perl -e 'alarm 400; exec @ARGV' cargo test -p retrace-box --test truncguard -- --test-threads=1`
Expected: **FAIL** — `the_band_fires_when_the_kernel_writes_past_the_window` must panic with
`NOT-THE-GUARD-BAND`, not with the guard band's message.

Then **revert the mutation** and re-run to confirm green again. Record both outputs in your report.
If the mutated build still PASSES, the control is not controlling anything — stop and report;
do not proceed.

- [ ] **Step 10: Confirm the negative controls stay green.**

Run: `perl -e 'alarm 560; exec @ARGV' cargo test -p retrace-box -- --test-threads=1`
Expected: PASS, 242 (241 + your 1). `memdiff`'s two tests must be green.

Run: `perl -e 'alarm 500; exec @ARGV' cargo test -p retrace --test bigread_e2e -- --test-threads=1`
Expected: PASS, 1.

- [ ] **Step 11: Commit.**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/truncguard.rs
git commit -m "M28-bandproof t1: a positive control — the band must be able to fire"
```

**Acceptance:** `truncguard` green at 4 tests; the `let band = 0;` mutation verified to FAIL it and
the revert verified to restore green; `retrace-box` 242/0/0; `bigread_e2e` green.

---

### Task 2: Shrink the band to what no other window covers

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (`band_not_covered`, and the post-forward loop)
- Modify: `crates/retrace-box/tests/truncguard.rs` (its unit tests)

**Interfaces:**
- Consumes: Task 1's `set_window_cap_for_test` (the positive control must survive this change).
- Produces: `Box_::band_not_covered(ipa: u64, len: usize, band: usize, others: &[(u64, usize)]) -> usize`.

- [ ] **Step 1: Write the failing tests.** Append to `crates/retrace-box/tests/truncguard.rs`:

```rust
// M28: a changed guard-band byte proves A KERNEL WRITE in that range — the guest vCPU is halted
// across `host_svc` and recorder threads are banned, so nothing else could have touched it. It does
// NOT prove the write was THIS argument's overrun. `forward_and_diff` takes a window for EVERY
// argument that looks like a mapped pointer (including a non-pointer whose value collides with a
// mapped IPA — see the dyld pread-count case in that function), so a write belonging to another
// argument of the same call, fully captured by ITS window, would trip this argument's band and
// panic a correct recording.
#[test]
fn a_band_with_no_neighbours_keeps_its_full_length() {
    assert_eq!(Box_::band_not_covered(0x1000, 256, 64, &[]), 64);
    // A window entirely past the band does not shrink it: band is [0x1100, 0x1140).
    assert_eq!(Box_::band_not_covered(0x1000, 256, 64, &[(0x2000, 16)]), 64);
}

#[test]
fn a_neighbour_starting_inside_the_band_truncates_it_there() {
    // band is [0x1100, 0x1140); a neighbour at 0x1120 leaves the first 0x20 bytes unambiguous.
    assert_eq!(Box_::band_not_covered(0x1000, 256, 64, &[(0x1120, 8)]), 0x20);
}

// The rule is SPAN INTERSECTION, not start position. A window beginning BEFORE the band but
// extending into it overlaps exactly as much as one beginning inside it, and a rule phrased on
// start position alone would miss precisely this case.
#[test]
fn a_neighbour_starting_before_the_band_but_reaching_into_it_still_truncates() {
    // band is [0x1100, 0x1140); neighbour spans [0x10f0, 0x1110) and covers the band's start.
    assert_eq!(Box_::band_not_covered(0x1000, 256, 64, &[(0x10f0, 0x20)]), 0);
}

// The argument's OWN window ends exactly where its band begins, so it can never suppress its own
// band. This is why the caller may pass every span without filtering itself out.
#[test]
fn an_argument_never_suppresses_its_own_band() {
    assert_eq!(Box_::band_not_covered(0x1000, 256, 64, &[(0x1000, 256)]), 64);
}
```

- [ ] **Step 2: Run and watch them fail.**

Run: `perl -e 'alarm 400; exec @ARGV' cargo test -p retrace-box --test truncguard -- --test-threads=1`
Expected: FAIL to compile — `no function or associated item named 'band_not_covered'`.

- [ ] **Step 3: Implement it.** In `impl Box_`, beside `overran_window`:

```rust
    /// M28: how many of a guard band's bytes are attributable to THIS argument.
    ///
    /// A changed band byte proves a kernel write in that range — but not that the write was this
    /// argument's overrun. `forward_and_diff` snapshots a window for every mapped-looking argument,
    /// so a band past argument *i* can overlap argument *j*'s window, where the kernel's write is
    /// real and FULLY CAPTURED. Blaming it on *i* would panic a correct recording. Shrink the band
    /// to the bytes no other window covers; a change in those is unambiguously past everything this
    /// call inspected.
    ///
    /// **Shrink, not skip-on-overlap.** Several arguments of one call routinely land in one backing
    /// (`/bin/ps`'s `sysctl` had three, two adjacent on the stack), so skipping wholesale would
    /// disable the detector exactly where arguments crowd and truncation is likeliest.
    ///
    /// The test is **span intersection, not start position**: a window beginning before the band
    /// but extending into it overlaps just as much as one beginning inside it. An argument's own
    /// window ends exactly where its band begins, so it never suppresses itself and the caller need
    /// not filter it out.
    pub fn band_not_covered(ipa: u64, len: usize, band: usize, others: &[(u64, usize)]) -> usize {
        let start = ipa + len as u64;
        let mut end = start + band as u64;
        for &(oi, ol) in others {
            let (os, oe) = (oi, oi + ol as u64);
            if os < end && oe > start { end = end.min(os.max(start)); }
        }
        (end - start) as usize
    }
```

- [ ] **Step 4: Run the unit tests and watch them pass.**

Run: `perl -e 'alarm 400; exec @ARGV' cargo test -p retrace-box --test truncguard -- --test-threads=1`
Expected: PASS, 8 tests.

- [ ] **Step 5: Wire it into `forward_and_diff`.** The shrink must happen in the **post-forward**
  loop, not in the argument loop: when the band for argument *i* is computed pre-forward, the
  windows for arguments *j > i* have not been pushed yet, so the full span list does not exist. The
  pre-image band is still taken at full `GUARD_BAND` length (harmless); only the COMPARISON is
  shrunk.

  Immediately before `for (ipa, len, pre, pre_band) in windows {`, add:

```rust
            // M28: every window this call took, so a band comparison can exclude bytes another
            // argument's window already covers. Collected BEFORE the loop because the loop consumes
            // `windows`. Each entry's own span ends exactly where its band begins, so passing the
            // whole list (including self) is correct — see `band_not_covered`.
            let spans: Vec<(u64, usize)> = windows.iter().map(|(i, l, _, _)| (*i, *l)).collect();
```

  Then inside the loop, replace the single `let band = …` line with:

```rust
                let raw_band = pre_band.len().min(avail_now.saturating_sub(len));
                let band = Self::band_not_covered(ipa, len, raw_band, &spans);
                // M28 Task 2: WARNING ONLY, and Task 3 is the measurement it exists for. R1 says
                // this shrink could suppress far more than expected, which would quietly weaken the
                // detector this milestone is meant to strengthen. Count it across the gate rather
                // than assume it is rare.
                if band < raw_band {
                    eprintln!("[M28 BANDSHRINK] syscall {} band past ipa {:#x} len {} shrunk {} -> {} \
                               by an overlapping window of the same call",
                        num as i64, ipa, len, raw_band, band);
                }
```

- [ ] **Step 6: Confirm the positive control SURVIVES the shrink.** This is the step that catches a
  shrink that suppresses everything.

Run: `perl -e 'alarm 400; exec @ARGV' cargo test -p retrace-box --test truncguard -- --test-threads=1`
Expected: PASS, 8 tests — including
`the_band_fires_when_the_kernel_writes_past_the_window`, which must STILL panic with the guard
band's message. If it now fails with `NOT-THE-GUARD-BAND`, the shrink has suppressed a band that
nothing overlaps; that is a bug in `band_not_covered` or its wiring, not a discovery.

- [ ] **Step 7: Confirm the negative controls stay green.**

Run: `perl -e 'alarm 560; exec @ARGV' cargo test -p retrace-box -- --test-threads=1`
Expected: PASS, 246 (242 + your 4).

Run: `perl -e 'alarm 500; exec @ARGV' cargo test -p retrace --test bigread_e2e -- --test-threads=1`
Expected: PASS, 1.

- [ ] **Step 8: Commit.**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/truncguard.rs
git commit -m "M28-bandproof t2: a band means this argument, or it means nothing"
```

**Acceptance:** `band_not_covered` green over all four shapes including the span-intersection case;
the positive control still fires; `retrace-box` 246/0/0; `bigread_e2e` green.

---

### Task 3: How often does the shrink suppress? (R1)

**No production code.** The deliverable is a number and its classification. This is the same shape as
M27's blast-radius measurement, and it exists because assuming overlaps are rare is exactly the
unmeasured supporting fact this repo keeps catching in itself.

- [ ] **Step 1: Run the FULL chunked gate**, capturing every log, per Global Constraints. Every
  chunk `EXIT=0`, exit code captured before any pipe.

- [ ] **Step 2: Count and group every suppression.**

```sh
grep -ah "M28 BANDSHRINK" "$LOGDIR"/*.log   # the per-chunk logs from Step 1 | sed 's/ ipa [^ ]*//' | sort | uniq -c | sort -rn
```

- [ ] **Step 3: Record `/bin/ps` separately**, since it is not in any gate and is the call known to
  put three arguments in one backing:

```sh
perl -e 'alarm 400; exec @ARGV' cargo run -q -p retrace -- record-dyn /bin/ps -o "$LOGDIR"/ps.bin
```

- [ ] **Step 4: Classify the result into exactly one of these, and write it down:**
  - **Rare** (suppressions are a small fraction of capped windows) — the shrink costs little
    detection and Component 2 is a clean win. Say so with the number.
  - **Common** — the shrink disables a large share of bands. That is a **finding**, not a failure:
    it means arguments crowd backings far more than assumed, and the honest close says the detector
    is weaker than M27's on those calls while being *correct* for the first time. Do NOT weaken the
    shrink to make the number look better; the whole point is that a band which cannot be attributed
    proves nothing.

- [ ] **Step 5: Commit the finding.**

```bash
git commit --allow-empty -m "M28-bandproof t3: how often a band cannot be attributed"
```

**Acceptance:** every gate chunk `EXIT=0`; a written count of suppressions grouped by syscall, plus
the `/bin/ps` number, plus the Rare/Common classification.

---

### Task 4: Provoke the `if !err` case and measure it

**Files:**
- Create: `crates/retrace-guest/asm/failsysctl.s`
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs`
- Create: `crates/retrace-box/tests/failwrite.rs`

**Interfaces:**
- Consumes: nothing from Tasks 1-3.
- Produces: `retrace_guest::FAILSYSCTL`.

- [ ] **Step 1: Write the guest.** Create `crates/retrace-guest/asm/failsysctl.s`:

```asm
// M28: a guest whose sysctl FAILS with a deliberately undersized buffer.
//
// `forward_and_diff` skips write capture entirely when the syscall sets the carry flag ("A failed
// syscall wrote nothing to the guest's buffers"), and the M27 guard band lives INSIDE that same
// `if !err` — so the detector is off on this path too. The README has named this hole since M27 and
// named this exact suspect: sysctl with an undersized `oldp` returns ENOMEM and MAY copy out what
// fits. Nothing has measured it.
//
// This guest asks for kern.ostype ("Darwin") into a 2-byte buffer, then emits those 2 bytes. If the
// kernel wrote despite failing, the recording shows them and a replay — which captured no writes —
// shows zeros. Same shape as `bigread`: a silent truncation becomes visible OUTPUT rather than a
// divergence the oracle cannot see, because (num, args) are identical on both sides.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // mib[0] = CTL_KERN (1), mib[1] = KERN_OSTYPE (1)
    adrp x9, mib@PAGE
    add  x9, x9, mib@PAGEOFF
    mov  w10, #1
    str  w10, [x9]
    str  w10, [x9, #4]

    // *oldlenp = 2, deliberately smaller than "Darwin\0"
    adrp x11, oldlen@PAGE
    add  x11, x11, oldlen@PAGEOFF
    mov  x12, #2
    str  x12, [x11]

    // sysctl(mib, 2, buf, oldlenp, NULL, 0)
    mov  x0, x9
    mov  x1, #2
    adrp x2, buf@PAGE
    add  x2, x2, buf@PAGEOFF
    mov  x3, x11
    mov  x4, #0
    mov  x5, #0
    mov  x16, #202              // SYS___sysctl
    svc  #0x80

    // write(1, buf, 2) — the bytes the kernel may or may not have written
    mov  x0, #1
    adrp x1, buf@PAGE
    add  x1, x1, buf@PAGEOFF
    mov  x2, #2
    mov  x16, #4                // SYS_write
    svc  #0x80

    // exit(0)
    mov  x0, #0
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
.p2align 4
mib:      .space 16
oldlen:   .space 8
buf:      .space 64
```

- [ ] **Step 2: Register it in the build.** In `crates/retrace-guest/build.rs`, beside the `bigread`
  block (this guest needs no fixture and no generated path constant, so it follows the simpler
  `spinloop` shape):

```rust
    // M28: a guest whose sysctl FAILS with an undersized buffer, to measure whether the kernel
    // writes anyway — the `if !err` path, where write capture AND the guard band are both off.
    let src = format!("{}/asm/failsysctl.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/failsysctl");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang failsysctl");
    assert!(status.success(), "failsysctl guest build failed");
```

  And in `crates/retrace-guest/src/lib.rs`, beside `BIGREAD`:

```rust
/// M28: a guest whose one `sysctl` fails (`ENOMEM`, undersized `oldp`). Measures whether the kernel
/// writes into a guest buffer on a FAILING syscall — the path where `forward_and_diff` skips write
/// capture and the guard band alike.
pub const FAILSYSCTL: &str = concat!(env!("OUT_DIR"), "/failsysctl");
```

- [ ] **Step 3: Write the measurement.** Create `crates/retrace-box/tests/failwrite.rs`:

```rust
use retrace_box::*;

// M28: does a FAILING syscall write into the guest's buffer?
//
// `forward_and_diff` answers "no" by construction — it skips the whole post-diff block when the
// carry flag is set, and the M27 guard band sits inside that same block, so neither the capture nor
// the detector runs. The comment there states it as fact; nothing has measured it. This test drives
// the one case the README already names as suspect.
//
// It asserts only what is already known (the call fails) and PRINTS the rest. Task 5 turns the
// measured answer into an assertion — writing one now would be guessing at the result.
#[test]
fn a_failing_sysctl_is_measured_for_writes() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FAILSYSCTL).unwrap());
    let mut b = Box_::load(&loaded);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SYSCTL => {
                // Snapshot the destination before, so the measurement does not depend on
                // forward_and_diff's own (skipped) capture.
                let before = b.read_bytes_for_test(args[2], 16);
                let (ret, err, writes) = b.forward_and_diff(num, args);
                let after = b.read_bytes_for_test(args[2], 16);
                assert!(err, "the undersized sysctl should FAIL; got ret={ret} err={err}");
                eprintln!("[M28 FAILWRITE] err={err} ret={} writes_captured={} \
                           buf_changed={} before={:02x?} after={:02x?}",
                    ret as i64, writes.len(), before != after, before, after);
                return;
            }
            Stop::Syscall { num, args } => {
                let (ret, _e, _w) = b.forward_and_diff(num, args);
                b.set_x0_and_return(ret);
            }
            Stop::Other { esr } => panic!("unexpected exit esr=0x{esr:x}"),
            Stop::Fault { pc, esr, far } => panic!("guest crashed pc=0x{pc:x} esr=0x{esr:x} far=0x{far:x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}
```

- [ ] **Step 3b: Run it and watch it fail.**

Run: `perl -e 'alarm 400; exec @ARGV' cargo test -p retrace-box --test failwrite -- --test-threads=1`
Expected: FAIL to compile — `no method named 'read_bytes_for_test'`.

- [ ] **Step 4: Add the small reader the test needs.** `Box_` has `read_u64` but no byte-slice
  reader on its public surface. In `impl Box_`, beside `set_window_cap_for_test`:

```rust
    /// Test seam (M28). Reads `len` bytes of guest memory at `ipa`, for tests that must observe
    /// memory independently of `forward_and_diff`'s own capture — which is exactly what the
    /// `if !err` measurement needs, since that path captures nothing. Production never calls this.
    pub fn read_bytes_for_test(&self, ipa: u64, len: usize) -> Vec<u8> {
        let (hp, avail) = self.host_span(ipa).expect("read_bytes_for_test: ipa not mapped");
        unsafe { std::slice::from_raw_parts(hp, len.min(avail)) }.to_vec()
    }
```

- [ ] **Step 5: Run it and read the measurement.**

Run: `perl -e 'alarm 400; exec @ARGV' cargo test -p retrace-box --test failwrite -- --test-threads=1 --nocapture`
Expected: PASS, and one `[M28 FAILWRITE]` line. Then run the whole package —
`perl -e 'alarm 560; exec @ARGV' cargo test -p retrace-box -- --test-threads=1` — and expect
**247 over one more binary than before** (246 + your 1); a per-target run would silently skip the
new binary, which is the trap this repo has been caught by twice. **Write that line down verbatim** — it is the
deliverable. The decisive field is `buf_changed`.

- [ ] **Step 6: Commit.**

```bash
git add crates/retrace-guest/asm/failsysctl.s crates/retrace-guest/build.rs \
        crates/retrace-guest/src/lib.rs crates/retrace-box/tests/failwrite.rs \
        crates/retrace-box/src/lib.rs
git commit -m "M28-bandproof t4: provoke the if-!err case instead of waiting for it"
```

**Acceptance:** the guest builds and runs; the sysctl fails as expected; the `[M28 FAILWRITE]` line
is captured verbatim with its `buf_changed` value.

---

### Task 5: Act on the measurement

**Do not start until Task 4's `[M28 FAILWRITE]` line exists.** Which branch you take is decided by
`buf_changed`, not by this document.

#### Branch A — `buf_changed=true` (the kernel wrote despite failing)

The `if !err` gate is dropping real kernel writes, and the comment asserting otherwise is false.

- [ ] **A1: Write the failing e2e gate.** Create `crates/retrace/tests/failsysctl_e2e.rs`:

```rust
mod util;

// M28: a FAILING syscall that writes anyway. `forward_and_diff` skipped write capture whenever the
// carry flag was set — "A failed syscall wrote nothing to the guest's buffers" — which Task 4
// measured to be false for sysctl with an undersized buffer: it returns ENOMEM and copies out what
// fits. Those bytes reached guest memory on record and no `Event`, so replay restored zeros there.
//
// Asserts on the DIFFERENCE this work makes: the two bytes the guest prints come from a buffer the
// kernel filled on a failing call. A truncated capture shows as different stdout, not as a
// divergence — the oracle cannot see it, since (num, args) match on both sides.
#[test]
fn a_failing_syscall_that_writes_still_replays() {
    let (rec, trace) = util::record(retrace_guest::FAILSYSCTL);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, rec.stdout,
        "replay stdout diverged: the failing sysctl's partial write was captured in no Event");
}
```

- [ ] **A2: Run it and watch it fail** (replay's stdout should differ from the recording's).

Run: `perl -e 'alarm 500; exec @ARGV' cargo test -p retrace --test failsysctl_e2e -- --test-threads=1`

- [ ] **A3: Capture writes on the error path.** In `forward_and_diff`, hoist the post-forward diff
  out of `if !err`. Replace the `if !err {` guard around the window loop so the loop always runs,
  and update the stale comment above it to say what Task 4 measured. **Leave the fd bookkeeping and
  everything else still gated on `!err`** — only the memory-diff loop moves.

- [ ] **A4: Run the gate test and watch it pass**, then re-run `bigread_e2e`, the whole
  `retrace-box` package, and `failwrite`.

- [ ] **A5: Commit.**

```bash
git commit -am "M28-bandproof t5: a failed syscall does write, and now it is captured"
```

#### Branch B — `buf_changed=false` (the kernel wrote nothing)

The gate's comment is correct for this case. Do **not** invent a fix for a bug you did not find.

- [ ] **B1: Turn the measurement into an assertion.** In `crates/retrace-box/tests/failwrite.rs`,
  replace the `eprintln!` with:

```rust
                // MEASURED (M28 Task 4): this failing sysctl writes NOTHING into the guest buffer,
                // so `forward_and_diff`'s `if !err` skip loses nothing HERE. That is a measurement
                // of one case, not a proof about failing syscalls in general — the gate stays open,
                // now with one datum in it instead of none.
                assert_eq!(before, after,
                    "a failing sysctl wrote into the guest buffer after all: the `if !err` skip is \
                     dropping real kernel writes, and this test's premise has changed — see the \
                     M28 spec's Component 3, Branch A");
```

- [ ] **B2: Run it, confirm green**, then re-run the whole `retrace-box` package.

- [ ] **B3: Commit.**

```bash
git commit -am "M28-bandproof t5: the if-!err skip loses nothing on a failing sysctl, measured"
```

**Acceptance (either branch):** the branch taken is the one the measurement dictated, the reasoning
is written down, and no assertion anywhere was loosened to accommodate a result.

---

### Task 6: The strong claim, the gate, and the two documents

- [ ] **Step 1: Merge `main` if it moved,** then run the full chunked gate per Global Constraints,
  including `--bins`.

- [ ] **Step 2: Reconcile file-by-file** against `main`'s actual close (M27: 523 / 0 / 2 over 115).
  Expected, to be confirmed rather than assumed: `truncguard.rs` **+5** (1 positive control + 4
  `band_not_covered` tests); `failwrite.rs` **+1 and +1 binary**; `failsysctl_e2e.rs` **+1 and +1
  binary** only if Task 5 took Branch A; `--bins` **unchanged**.

- [ ] **Step 3: clippy** clean over `--workspace --all-targets`.

- [ ] **Step 4: Restore the strong claim in the assert message.** M27 softened it because it was
  false. Task 2 made it true: the band now covers only bytes no other window of this call inspected.
  Rewrite the message so it says a changed band byte IS proof of a write past everything this call
  inspected — and keep the actionable `dest_buffer` instruction. **Then re-run the positive control**,
  whose `#[should_panic(expected = …)]` substring must still match the new wording; update it in the
  same commit if it does not.

- [ ] **Step 5: The two documents, which must not be merged.**
  - **README, edited in place.** "What works today": the band now has a positive control and its
    firings are attributable. "Known limits": the false-negative entry STAYS — Task 2 did not fix
    coverage, and M27's zeros-over-zeros measurement is still true. Replace the passage that softens
    the proof claim with the strong one. Add what Task 3 measured about suppression, and what Task 4
    measured about `if !err`. Update the gate line.
  - **`docs/status-log.md`** — **append** a `## Status: M28-bandproof` section. Never rewrite M27's.
    Say what the suppression count was, which branch Task 5 took and why, and that the band's
    *coverage* limit is unchanged and still deferred.
  - **CLAUDE.md** — only if a statement became false.

- [ ] **Step 6: State the outcome without hedging.** Say whether the band can now be shown to fire,
  what a firing now proves, how often it is suppressed, and what `if !err` was measured to do.

**Acceptance:** every chunk `EXIT=0`, clippy clean, total reconciled file-by-file, both documents
updated, `git status` clean apart from intended files.

---

## Sequencing

Task 1 → 2 → **3** → 4 → 5 → 6. Task 3 is a barrier only for the *documents* (Task 6 must report a
measured suppression rate, not an assumed one); Task 4 does not depend on it and may be reordered
before it if that is more convenient. Task 5 is hard-gated on Task 4's measurement.

## Self-Review

1. **`TRACE_MAGIC` unchanged** and no `Event` variant or field touched, in either Task 5 branch.
   Branch A changes which writes a recording CONTAINS, never the trace's shape.
2. **No replay mirror added.** Both fail-loud sites are `assert!`s, which emit no landmark, and
   `forward_and_diff` never runs on replay.
3. **`window_cap` is initialised in all four `Box_ { … }` constructions** — the grep in Task 1
   Step 5 must print 4, not 3.
4. **The positive control is verified to fail under `let band = 0;`.** A control nobody tried to
   break is not a control.
5. **The positive control uses `fstat`, not `read`.** `read` is in `dest_buffer`, so its window
   widens past any cap and the test would be vacuous.
6. **`band_not_covered` is tested on span intersection**, not only on start position — the case a
   naive implementation misses.
7. **Task 5's branch is chosen by Task 4's measurement**, and the unchosen branch is not
   half-implemented "just in case".
8. **The band's COVERAGE limit is untouched and still documented.** M28 makes firings trustworthy;
   it does not make silence meaningful. Any README wording implying otherwise is a defect.
