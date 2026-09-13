# M35-errholes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the two holes M27 and M28 left in `retrace-box`: make `Box_::diff_memory` report
a recorded region longer than its replay backing instead of comparing only the part that fits
(H1), and run `forward_and_diff`'s write capture — and the guard band — on a **failing** syscall
too (H2), which the spec measured as a live bit-for-bit replay failure on the M28 fixture.

**Architecture:** Two edits in `crates/retrace-box/src/lib.rs`, each inside one function, with no
change to the trace format and none to `retrace-core`: replay's generic arm already applies
`writes` on an `err = true` landmark (spec §3b, §8). One existing test inverts its own assertion
(`failwrite.rs`, whose message anticipated this), one new unit test proves H1 fires, one new
static guest (`failproc.s`) makes the data half of H2 visible as stdout, and one new e2e file
records and replays both fixtures through the CLI and inspects the landmark. Docs, sweep, gate.

**Tech Stack:** Rust 1.95.0, `aarch64-apple-darwin`, Hypervisor.framework, clang for the
freestanding guest. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-13-retrace-m35-errholes-design.md`

## Global Constraints

- **`--test-threads=1` is mandatory** on every `cargo test` invocation (one VM per process).
- **Target is pinned** by `.cargo/config.toml`; do not override it or the codesigning `runner`.
- **The premise is measured** (spec §4a: `failsysctl` replay rc 3, `ipa 0x100004010 replay=0x02
  recorded=0x00`; §4c: `kern.proc.all` into 648 bytes → `ENOMEM`, 648 bytes written, `*oldlenp`
  → 0). Every number below comes from it. If what you measure disagrees, stop and say so — do
  not pick one silently.
- **The code changes are spec §3a and §3b, verbatim.** H1 returns `Some(..)` (Ruling 2: not a
  panic). H2 deletes the `if !err` around the capture loop and nothing else; the band assert
  goes live on the error path with **no** exemption (Ruling 3). Do not touch the fd bookkeeping
  `!err` gates (spec §3c), the per-register probe loop (M34 §4b), or the M30 restore pass.
- **No new `#[ignore]`. No `TRACE_MAGIC` bump. No `retrace-core` edit.** Any of these is a
  charter §5 halt: stop with the branch intact and a written explanation.
- **Do not push.** Commit and merge locally only.
- Every positive control is run **red then green** and the red output is pasted into the task
  report verbatim.
- Run targeted tests only in Tasks 1–3; the full chunked gate runs once, in Task 4, in the
  controller's shell (it exceeds the tool ceiling).

---

### Task 1: H1 — `diff_memory` refuses what it cannot compare

**Files:**
- Modify: `crates/retrace-box/src/lib.rs:3924–3940` (`diff_memory`)
- Test: `crates/retrace-box/tests/truncguard.rs` (insert one test after
  `the_clamp_reaches_proc_info`, which ends at line 283)

**Interfaces:**
- Consumes: `Box_::diff_memory(&self, &[Region]) -> Option<String>` (unchanged signature);
  `Box_::read_bytes_for_test`, `Box_::host_span_for_test`, `retrace_box::STACK_TOP_IPA`.
- Produces: nothing new; the `Some` message text below is what Task 4's docs quote.

- [ ] **Step 1: Write the failing test**

Append to `crates/retrace-box/tests/truncguard.rs`:

```rust
// M35 Control 1: `diff_memory` used to compare a recorded region only up to its replay backing
// (`.min(avail)`) and report the excess as nothing — the one place the terminal full-memory oracle
// could return `None` on bytes it never looked at. A region built to overrun a 64-byte backing by
// 64 bytes, whose first 64 bytes match the guest exactly, must now come back as a divergence
// naming all three numbers. Under the old clamp this returned `None`: it compared the 64 that
// match and never saw the 64 that have no backing to compare against.
#[test]
fn a_recorded_region_longer_than_its_replay_backing_is_a_divergence() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    match b.run() {
        Stop::Syscall { .. } => {}
        other => panic!("expected the guest's first syscall stop, got {other:?}"),
    }

    const AVAIL: u64 = 64;
    let dest = retrace_box::STACK_TOP_IPA - AVAIL;
    let (_, avail) = b.host_span_for_test(dest).expect("the static stack backing ends at STACK_TOP_IPA");
    assert_eq!(avail as u64, AVAIL, "dest must sit exactly {AVAIL} bytes before the end of its backing");

    // The first 64 bytes are the guest's own, so a compare that stops at the backing sees no
    // mismatch; the next 64 have nothing behind them at all.
    let mut bytes = b.read_bytes_for_test(dest, AVAIL as usize);
    bytes.extend(std::iter::repeat_n(0u8, AVAIL as usize));
    assert_eq!(bytes.len(), 128);
    let region = retrace_trace::Region { ipa: dest, bytes };

    let msg = b.diff_memory(&[region]).expect(
        "a 128-byte recorded region over a 64-byte backing must be reported as a divergence, not \
         compared up to the backing and passed — that silence is the M1 hole this test closes");
    assert!(msg.contains("128 bytes") && msg.contains("holds only 64"),
        "the divergence must name the recorded length and the backing; got: {msg}");
}
```

`retrace-trace` is a regular dependency of `retrace-box` (`Cargo.toml:11`) and `fixedstraddle.rs`
builds `Region`s the same way; the fully qualified path above needs no import.
`std::iter::repeat_n` is stable since 1.82 (toolchain 1.95).

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p retrace-box --test truncguard a_recorded_region_longer -- --test-threads=1`
Expected: FAIL at the `expect` — `diff_memory` returned `None` (the clamp compared 64 matching
bytes and stopped). Paste the failure line into the report: this is Control 1's red.

- [ ] **Step 3: Implement**

In `crates/retrace-box/src/lib.rs`, `diff_memory`, replace

```rust
            let n = r.bytes.len().min(avail);
            let cur = unsafe { std::slice::from_raw_parts(hp, n) };
            if let Some(off) = (0..n).find(|&i| cur[i] != r.bytes[i]) {
```

with

```rust
            // M35 (H1): a recorded region longer than its replay backing is a divergence in its
            // own right, not a prefix to compare. On a correct replay this never fires — every
            // captured region lies inside one record-side backing, and replay rebuilds the same
            // backings from the same snapshot — so the branch exists for the INCORRECT replay: a
            // layout drift, a checkpoint restored against a different backing set, a future edit to
            // the pager. Until M35 this was `.min(avail)`, which compared the part that fit and
            // said nothing about the rest: flagged in M1's own review, deferred at M2, carried by
            // M27, M28, M30, M33 and M34 as "still unpaid". `write_guest` (the apply side) has
            // asserted the same bound since M0; this is the compare side catching up.
            if r.bytes.len() > avail {
                return Some(format!(
                    "recorded region at ipa {:#x} is {} bytes but its replay backing holds only {} \
                     from that address — the recording and the replay disagree about the guest's \
                     memory layout, which no byte compare can settle",
                    r.ipa, r.bytes.len(), avail));
            }
            let n = r.bytes.len();
            let cur = unsafe { std::slice::from_raw_parts(hp, n) };
            if let Some(off) = (0..n).find(|&i| cur[i] != r.bytes[i]) {
```

- [ ] **Step 4: Run the test to verify it passes, plus the file's neighbours**

Run: `cargo test -p retrace-box --test truncguard --test checkpoint -- --test-threads=1`
Expected: `truncguard` 22 passed (M34: 21); `checkpoint` passes (it calls `diff_memory` on a
restored box — the correct-replay path must still return `None`).

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/truncguard.rs
git commit -m "M35 t1: diff_memory reports a region longer than its replay backing instead of comparing the part that fits"
```

---

### Task 2: H2 — the capture runs on the failing path, and `failwrite.rs` says so

**Files:**
- Modify: `crates/retrace-box/src/lib.rs:3436–3439` and the closing `}` at `:3559`
  (`forward_and_diff`'s capture loop); `:3572–3577` (M30 restore comment, point 2);
  `:3042–3044` (`read_bytes_for_test` doc)
- Modify: `crates/retrace-box/tests/failwrite.rs` (whole file — one test, rewritten)

**Interfaces:**
- Consumes: `Box_::forward_and_diff(num, args) -> (u64, bool, Vec<Region>)` unchanged;
  `Box_::read_bytes_for_test`.
- Produces: `forward_and_diff` now returns non-empty `writes` beside `err = true` when the kernel
  wrote. Task 3's e2e asserts the same through the trace.

- [ ] **Step 1: Rewrite the test so it fails on the current tree**

Replace the whole of `crates/retrace-box/tests/failwrite.rs` with:

```rust
use retrace_box::*;

// M28 asked whether a FAILING syscall writes into the guest's buffer, drove `sysctl(kern.ostype)`
// into a 2-byte buffer (ENOMEM: "Darwin\0" needs seven), read `buf` before and after, found it
// unchanged, and concluded "the kernel wrote nothing, before or after". That was true of `buf`
// and false of the call: it read `args[2]` and never `args[3]`. xnu's `sysctl()` entry
// (bsd/kern/kern_newsysctl.c, `sysctl`: `if (error && error != ENOMEM) return error;` then
// `suulong(uap->oldlenp, oldlen)`) writes `*oldlenp` back on the ENOMEM path, with the `oldidx`
// the handler left — 0 here, because `sysctl_old_user` refuses before copying. M35 measured it
// end to end first: this fixture recorded cleanly and its replay DIVERGED at `oldlen`
// (`ipa 0x100004010 replay=0x02 recorded=0x00`), because `forward_and_diff` skipped the capture
// on `err` and replay had nothing to apply.
//
// So this test now asserts the call, not the buffer: `buf` is still untouched (M28's datum
// stands), and the eight bytes at `oldlenp` are captured as a write and read back as zero.
#[test]
fn a_failing_sysctl_is_measured_for_writes() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FAILSYSCTL).unwrap());
    let mut b = Box_::load(&loaded);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SYSCTL => {
                // `buf` is the full 64-byte backing (`.space 64` in failsysctl.s); `oldlen` is the
                // 8 bytes the guest set to 2. Both read through the seam, independently of the
                // capture, so the capture can be checked AGAINST them.
                let buf_before = b.read_bytes_for_test(args[2], 64);
                let oldlen_before = b.read_bytes_for_test(args[3], 8);
                assert_eq!(oldlen_before, 2u64.to_le_bytes(), "precondition: the guest asked for 2 bytes");

                let (ret, err, writes) = b.forward_and_diff(num, args);
                assert!(err, "the undersized sysctl should FAIL; got ret={ret} err={err}");
                assert_eq!(ret, 12, "ENOMEM");

                // MEASURED (M28 Task 4, still true): the data buffer is untouched — xnu's
                // `sysctl_old_user` returns ENOMEM before its copyout.
                assert_eq!(buf_before, b.read_bytes_for_test(args[2], 64),
                    "a failing sysctl wrote into `buf` after all; xnu's sysctl_old_user must have \
                     changed shape — re-read it before touching this test");

                // MEASURED (M35): `*oldlenp` is written back as 0 on the ENOMEM path.
                let oldlen_after = b.read_bytes_for_test(args[3], 8);
                assert_eq!(oldlen_after, 0u64.to_le_bytes(),
                    "the kernel writes *oldlenp back on ENOMEM (xnu sysctl(): suulong after the \
                     ENOMEM pass-through); the seam sees it did not — re-read kern_newsysctl.c");

                // And the capture must agree with the seam: some captured region covers the
                // 8 bytes at args[3] and carries the zero. Until M35 `forward_and_diff` returned
                // NO writes on `err` — the `if !err` skip — which is exactly the divergence this
                // fixture's replay showed.
                let captured = writes.iter().find_map(|r| {
                    let end = r.ipa + r.bytes.len() as u64;
                    (r.ipa <= args[3] && args[3] + 8 <= end)
                        .then(|| r.bytes[(args[3] - r.ipa) as usize..][..8].to_vec())
                });
                assert_eq!(captured, Some(oldlen_after),
                    "forward_and_diff captured no write covering *oldlenp on a failing syscall: \
                     the `if !err` skip is dropping a real kernel write (writes captured: {})",
                    writes.len());
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

- [ ] **Step 2: Run it to verify it fails at the capture assertion, not earlier**

Run: `cargo test -p retrace-box --test failwrite -- --test-threads=1`
Expected: FAIL at "forward_and_diff captured no write covering *oldlenp" with `writes captured:
0`. The two assertions before it (`buf` unchanged; `oldlen_after == 0` through the seam) must
PASS on the unmodified tree — they are the measurement, independent of the fix. If
`oldlen_after` is not 0, stop: the spec's §4b reading is wrong and the milestone re-scopes.
Paste the failure into the report: this is Control 2's red (unit half).

- [ ] **Step 3: Remove the gate**

In `crates/retrace-box/src/lib.rs`, replace

```rust
        // A failed syscall (carry set) wrote nothing to the guest's buffers, so skip the
        // post-diff write capture entirely.
        let mut writes = Vec::new();
        if !err {
            for (ipa, len, pre, pre_band, band) in windows.iter() {
```

with

```rust
        // M35 (H2): the capture — and the guard band inside it — runs on the FAILING path too.
        // Until M35 this loop sat under `if !err` on the stated assumption that a failed syscall
        // wrote nothing; M27 narrowed it (`ps`'s 83 sysctls all succeeded), M28 measured one case
        // and found the data buffer untouched, and M35 measured the call: xnu's `sysctl()` writes
        // `*oldlenp` back on ENOMEM (`suulong` after the `error != ENOMEM` pass-through), and the
        // `kern.proc` handlers copy out every record that fits BEFORE returning ENOMEM — 648 bytes
        // of data on a failing call. The M28 fixture recorded cleanly and its replay diverged at
        // `oldlen`. Nothing in this loop depends on `err`: the pre-image and the canary fill were
        // taken before `host_svc` on both paths, the restore below already ran on both, and
        // replay's generic arm has applied `writes` beside `err = true` since M0. What was
        // skipped was the looking.
        let mut writes = Vec::new();
        {
            for (ipa, len, pre, pre_band, band) in windows.iter() {
```

(The bare block keeps the diff to the two lines that matter and the closing `}` at `:3559`
untouched; do not re-indent the loop body.)

Then in the M30 restore comment, replace point 2:

```rust
        // 2. **On the error path too.** The fill is unconditional, so the restore must be. A failed
        //    syscall wrote nothing — which is exactly why the capture loop skips it — so the band
        //    still holds the canary retrace itself wrote, and leaving it there would hand the guest
        //    bytes no kernel ever produced, on the ordinary path every EINTR/ENOENT/EAGAIN takes.
        //    Measured: 36 error-path restores in that same jq recording, so this is load-bearing,
        //    not defensive. Nothing is CHECKED there for the same reason nothing is captured.
```

with

```rust
        // 2. **On the error path too.** The fill is unconditional, so the restore must be: a band
        //    the kernel did not touch still holds the canary retrace itself wrote, and leaving it
        //    there would hand the guest bytes no kernel ever produced, on the ordinary path every
        //    EINTR/ENOENT/EAGAIN takes. Measured: 36 error-path restores in that same jq recording,
        //    so this is load-bearing, not defensive. Since M35 the band is also CHECKED on that
        //    path before it is restored, because a failing syscall can write (the capture loop
        //    above says where that was measured).
```

And in `read_bytes_for_test`'s doc, replace

```rust
    /// memory independently of `forward_and_diff`'s own capture — which is exactly what the
    /// `if !err` measurement needs, since that path captures nothing. Production never calls this.
```

with

```rust
    /// memory independently of `forward_and_diff`'s own capture, so the capture can be checked
    /// against the memory rather than against itself — M28's failing-syscall measurement, and
    /// M35's proof that the capture now sees what that measurement missed. Production never calls
    /// this.
```

- [ ] **Step 4: Run the test to verify it passes, then the neighbours that exercise the loop**

Run: `cargo test -p retrace-box --test failwrite --test truncguard --test failsys -- --test-threads=1`
(`truncguard.rs` holds `a_failing_syscall_still_restores_the_canary` at `:505` and
`a_duplicated_pointer_argument_does_not_manufacture_a_disturbance` at `:559`.)
Expected: all pass — `truncguard` 22, `failwrite` 1, `failsys` as before. `a_failing_syscall_still_restores_the_canary` is the one that now runs the
band CHECK on the error path before the restore it tests; if it goes red, read the assertion —
it means a failing `open` disturbed its band, which would be a finding, not a test to weaken.

- [ ] **Step 5: Smoke the real guests on the error path, and count the band**

The error path is busy (M30: 36 restores on one `jq` run). Two recordings through the CLI, with
the canary channel on, so a band firing on the error path is named rather than inferred:

```bash
cargo build -p retrace
BIN=$(mktemp -t retrace-m35); cp target/aarch64-apple-darwin/debug/retrace "$BIN"
codesign -s - -f --entitlements retrace.entitlements "$BIN"
OUT=$(ls -dt target/aarch64-apple-darwin/debug/build/retrace-guest-*/out | head -1)
RETRACE_CANARY=1 "$BIN" record-dyn "$OUT/hello_dyn" -o /tmp/m35-hd.bin 2> /tmp/m35-hd.err; echo "hello_dyn record rc=$?"
"$BIN" replay /tmp/m35-hd.bin > /dev/null 2> /tmp/m35-hd-rep.err; echo "hello_dyn replay rc=$?"
RETRACE_CANARY=1 "$BIN" record-dyn /opt/homebrew/bin/jq -o /tmp/m35-jq.bin -- --version 2> /tmp/m35-jq.err; echo "jq record rc=$?"
"$BIN" replay /tmp/m35-jq.bin > /dev/null 2> /tmp/m35-jq-rep.err; echo "jq replay rc=$?"
grep -c 'M30 CANARY' /tmp/m35-hd.err /tmp/m35-jq.err
rm -f "$BIN"
```

Expected: all four rc 0; zero `[M30 CANARY]` lines. Paste the four rc lines and the two counts
into the report. If a recorder panics on the band assert, paste the whole assertion message
(syscall, band, window, ipa): it is a **finding** — do not add an exemption (Ruling 3); report
it and stop the task with status BLOCKED so the controller can rule.

Also count how many landmarks now carry writes beside `err = true` on `jq`, so the status log
can say what the hoist recorded (a small Rust snippet is not worth a binary — use the debug CLI
if it lists landmarks, else skip and say so; the e2e in Task 3 is the standing evidence).

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/failwrite.rs
git commit -m "M35 t2: capture kernel writes on a failing syscall too — the if !err skip dropped *oldlenp on ENOMEM"
```

---

### Task 3: The data-half fixture and the end-to-end controls

**Files:**
- Create: `crates/retrace-guest/asm/failproc.s`
- Modify: `crates/retrace-guest/build.rs` (after the `failsysctl` block, `:86–94`)
- Modify: `crates/retrace-guest/src/lib.rs` (after `FAILSYSCTL`, `:136`)
- Create: `crates/retrace/tests/failsys_e2e.rs`

**Interfaces:**
- Consumes: `util::record`, `util::replay` (`crates/retrace/tests/util/mod.rs`);
  `retrace_trace::Reader::open_checked`; `retrace_trace::Event::Syscall { num, args, ret, err,
  writes, thread }`; `retrace_arch::SYS_SYSCTL` (202).
- Produces: `retrace_guest::FAILPROC`.

- [ ] **Step 1: The guest**

Create `crates/retrace-guest/asm/failproc.s`:

```asm
// M35: a guest whose sysctl(kern.proc.all) FAILS (ENOMEM) after the kernel has already copied
// one full kinfo_proc into its buffer — the DATA half of the `if !err` hole.
//
// xnu's kern.proc handlers (bsd/kern/kern_sysctl.c: sysdoproc_callback copies out each record
// while it fits; sysctl_prochandle returns ENOMEM when `needed > oldlen`) write what fits and
// THEN fail. Measured on the host before this guest was written: with *oldlenp = 648 =
// sizeof(kinfo_proc), ret=-1 errno=12, 648 bytes changed, nothing past them, *oldlenp -> 0.
//
// The guest then writes the first 8 bytes of the record to stdout, so a capture that misses
// them is visible as OUTPUT (the bigread shape) and not only at the terminal memory compare.
// The bytes themselves are host state (whichever process the kernel iterates first) — recorded
// and replayed, never regenerated, like task_info's audit token.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // mib = { CTL_KERN (1), KERN_PROC (14), KERN_PROC_ALL (0) }
    adrp x9, mib@PAGE
    add  x9, x9, mib@PAGEOFF
    mov  w10, #1
    str  w10, [x9]
    mov  w10, #14
    str  w10, [x9, #4]
    str  wzr, [x9, #8]

    // *oldlenp = 648: room for exactly one record, and the machine runs hundreds of processes
    adrp x11, oldlen@PAGE
    add  x11, x11, oldlen@PAGEOFF
    mov  x12, #648
    str  x12, [x11]

    // sysctl(mib, 3, buf, oldlenp, NULL, 0)
    mov  x0, x9
    mov  x1, #3
    adrp x2, buf@PAGE
    add  x2, x2, buf@PAGEOFF
    mov  x3, x11
    mov  x4, #0
    mov  x5, #0
    mov  x16, #202              // SYS___sysctl
    svc  #0x80

    // write(1, buf, 8) — the first 8 bytes the kernel copied out despite failing
    mov  x0, #1
    adrp x1, buf@PAGE
    add  x1, x1, buf@PAGEOFF
    mov  x2, #8
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
buf:      .space 648
```

- [ ] **Step 2: Build it**

In `crates/retrace-guest/build.rs`, after the `failsysctl` block (ends `assert!(status.success(),
"failsysctl guest build failed");`), add:

```rust
    // M35: a guest whose sysctl(kern.proc.all) fails ENOMEM AFTER the kernel copied one full
    // kinfo_proc into its 648-byte buffer — the data half of the `if !err` hole.
    let src = format!("{}/asm/failproc.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/failproc");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang failproc");
    assert!(status.success(), "failproc guest build failed");
```

In `crates/retrace-guest/src/lib.rs`, after `FAILSYSCTL`:

```rust
/// M35: a guest whose `sysctl(kern.proc.all)` fails `ENOMEM` after the kernel has copied one full
/// 648-byte `kinfo_proc` into its buffer, then writes that record's first 8 bytes to stdout —
/// the data half of the failing-syscall capture, visible as output.
pub const FAILPROC: &str = concat!(env!("OUT_DIR"), "/failproc");
```

Run: `cargo build -p retrace-guest` — expected: success, and
`ls $(ls -dt target/aarch64-apple-darwin/debug/build/retrace-guest-*/out | head -1)/failproc`
exists.

- [ ] **Step 3: Write the two e2e tests**

Create `crates/retrace/tests/failsys_e2e.rs`:

```rust
// M35: a FAILING syscall's kernel writes are recorded and replayed.
//
// Until M35 `forward_and_diff` skipped write capture whenever the carry flag was set, on the
// stated assumption that a failed syscall writes nothing. Two fixtures say otherwise, and each
// was measured before its test was written (spec §4):
//
//   failsysctl (M28): sysctl(kern.ostype) into 2 bytes -> ENOMEM, and xnu's sysctl() entry writes
//     *oldlenp back (= 0) on that path. On the pre-M35 tree this guest recorded cleanly and its
//     replay DIVERGED: `ipa 0x100004010 replay=0x02 recorded=0x00`. The oracle saw it only because
//     `oldlen` survives to the final snapshot.
//   failproc (M35): sysctl(kern.proc.all) into exactly one kinfo_proc -> ENOMEM after 648 bytes
//     of data were copied out. The guest prints 8 of them, so a dropped capture is visible as a
//     stdout mismatch too (the bigread shape).
//
// Exit codes alone would not do (CLAUDE.md): a replay that applies nothing exits 0 on any guest
// whose divergence the terminal compare cannot see. So each test asserts on the landmark itself —
// `err: true` AND a captured region covering the bytes the kernel wrote — and only then on the
// replay's exit and output.
mod util;
use retrace_trace::Event;

/// The bytes a captured `writes` set holds for `[ipa, ipa + len)`, if some region covers it.
fn captured(writes: &[retrace_trace::Region], ipa: u64, len: usize) -> Option<Vec<u8>> {
    writes.iter().find_map(|r| {
        let end = r.ipa + r.bytes.len() as u64;
        (r.ipa <= ipa && ipa + len as u64 <= end)
            .then(|| r.bytes[(ipa - r.ipa) as usize..][..len].to_vec())
    })
}

/// The one `sysctl` landmark in `trace`: `(args, err, writes)`.
fn the_sysctl(trace: &std::path::Path) -> ([u64; 8], bool, Vec<retrace_trace::Region>) {
    let (events, torn) = retrace_trace::Reader::open_checked(trace).unwrap();
    assert!(!torn, "the recording must be complete");
    let mut hits = events.iter().filter_map(|e| match e {
        Event::Syscall { num, args, err, writes, .. } if *num == retrace_arch::SYS_SYSCTL =>
            Some((*args, *err, writes.clone())),
        _ => None,
    });
    let hit = hits.next().expect("the guest issues exactly one sysctl");
    assert!(hits.next().is_none(), "the guest issues exactly one sysctl");
    hit
}

#[test]
fn a_failing_sysctl_replays_bit_for_bit() {
    let (rec, trace) = util::record(retrace_guest::FAILSYSCTL);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert_eq!(rec.stdout, [0u8, 0], "the 2-byte buffer stays zero (xnu refuses before copying)");

    let (args, err, writes) = the_sysctl(&trace);
    assert!(err, "the undersized sysctl must FAIL on record");
    // The write M35 makes recordable: *oldlenp (args[3]) written back as 0 on the ENOMEM path.
    assert_eq!(captured(&writes, args[3], 8), Some(0u64.to_le_bytes().to_vec()),
        "the landmark must carry the kernel's write-back of *oldlenp; without it replay keeps \
         the guest's 2 and diverges at ipa {:#x} — the pre-M35 measurement. writes: {}",
        args[3], writes.len());

    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, rec.stdout, "replay stdout diverged from the recording");
}

#[test]
fn a_failing_proc_list_replays_bit_for_bit() {
    let (rec, trace) = util::record(retrace_guest::FAILPROC);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert_eq!(rec.stdout.len(), 8, "the guest writes the record's first 8 bytes");

    let (args, err, writes) = the_sysctl(&trace);
    assert!(err, "kern.proc.all into one record's worth of buffer must FAIL (ENOMEM) on record");
    // The data half: 648 bytes of kinfo_proc copied out BEFORE the failure, and *oldlenp -> 0.
    let data = captured(&writes, args[2], 648)
        .expect("the landmark must carry the 648-byte record the kernel copied out on its way to ENOMEM");
    assert_eq!(&data[..8], &rec.stdout[..],
        "the captured record's first 8 bytes are what the guest printed");
    assert_ne!(data, vec![0u8; 648],
        "a kinfo_proc is never all zero (p_pid, p_comm, ...); an all-zero capture means the window \
         was diffed before the kernel wrote it, which cannot happen — or the bytes are the guest's own");
    assert_eq!(captured(&writes, args[3], 8), Some(0u64.to_le_bytes().to_vec()),
        "*oldlenp is written back as 0 (sysctl_prochandle returns ENOMEM before oldidx advances)");

    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, rec.stdout, "replay printed different record bytes than the recording");
}
```

- [ ] **Step 4: Run both — green on the Task 2 tree**

Run: `cargo test -p retrace --test failsys_e2e -- --test-threads=1`
Expected: 2 passed. If `a_failing_proc_list_replays_bit_for_bit` fails at "must carry the
648-byte record", print `writes.iter().map(|r| (r.ipa, r.bytes.len()))` in the message and
report: the spec expects the `DerefU64(3)` window for `args[2]` to be exactly 648 (the `*oldlenp`
the guest stored) and the `Ptr` window for `args[0]` (`mib`) to cover the whole `__DATA` page
(`PTR_WINDOW_CAP.min(avail)`), so at least one region covers `buf`.

- [ ] **Step 5: Controls 2 and 3 — red under the mutation, then green again**

Temporarily reinstate the gate in `crates/retrace-box/src/lib.rs`: change the bare `{` that
Task 2 left above `for (ipa, len, pre, pre_band, band) in windows.iter()` back to `if !err {`.
Rebuild and run:

```
cargo test -p retrace --test failsys_e2e -- --test-threads=1
cargo test -p retrace-box --test failwrite -- --test-threads=1
```

Expected, and paste all three verbatim into the report:
- `a_failing_sysctl_replays_bit_for_bit` RED at "the landmark must carry the kernel's write-back
  of *oldlenp" with `writes: 0` (the assertion fires before replay runs — so ALSO run
  `retrace replay` by hand on a recording made under the mutation, from the trace path the util
  prints or a fresh `record`, and paste the `DIVERGENCE at landmark 4 … ipa 0x100004010
  replay=0x02 recorded=0x00` line: the milestone's own premise, reproduced by its own fixture).
- `a_failing_proc_list_replays_bit_for_bit` RED at "must carry the 648-byte record".
- `failwrite` RED at "captured no write covering *oldlenp".

Revert the mutation (`git checkout crates/retrace-box/src/lib.rs`, then `git diff --stat` must
be empty for that file). Re-run both commands: green.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-guest/asm/failproc.s crates/retrace-guest/build.rs crates/retrace-guest/src/lib.rs crates/retrace/tests/failsys_e2e.rs
git commit -m "M35 t3: failproc fixture and failsys_e2e — a failing syscall's writes record and replay"
```

---

### Task 4: Docs, the sweep, the gate, the merge

**Files:**
- Modify: `README.md` (the gate paragraph `:329–356`; the two-holes paragraph `:680–695`)
- Modify: `docs/status-log.md` (append one section; never edit an earlier line)
- Modify: `docs/superpowers/specs/2026-09-13-retrace-m35-errholes-design.md` (§11 only)

**Interfaces:**
- Consumes: the controller's numbers file (`task-4-numbers.md` in the SDD workspace: gate
  figures, sweep tally, reconciliation) and the three task reports.

- [ ] **Step 1: README — the two-holes paragraph**

Replace, in the paragraph beginning "The saving grace underneath stays what it was", the span
from "Two holes stay open and unmeasured:" through "the gate stays open, now with a data point
in it instead of none." with:

```
Both holes M27 and M28 left are closed at M35. `Box_::diff_memory` now returns a divergence
naming the recorded length and the backing when a region is longer than what replay has behind
it, instead of comparing the part that fits (flagged in M1's own review, unpaid until now; a
correct replay never takes the branch, and `truncguard.rs` proves it fires). And
`forward_and_diff` captures — and bands — on a **failing** syscall too, because the assumption
that a failed syscall writes nothing was **measured false**: the M28 fixture `failsysctl`
recorded cleanly on the pre-M35 tree and its replay diverged (`ipa 0x100004010 replay=0x02
recorded=0x00` — the `oldlen` cell), since xnu's `sysctl()` writes `*oldlenp` back on the
`ENOMEM` path and the `if !err` skip threw that write away. M28's datum stands as far as it went
— the *data* buffer is untouched, `sysctl_old_user` refuses before copying — but its test read
`buf` and never `oldlenp`. The data half is real too: `kern.proc.all` into one record's worth of
buffer fails `ENOMEM` *after* copying 648 bytes out (`failproc`, `failsys_e2e`). No format
change: replay's generic arm has applied `writes` beside `err = true` since M0 and simply never
received one.
```

- [ ] **Step 2: README — the gate paragraph**

Rewrite `:329–356` for M35 from the numbers file: the new totals, "measured at M35", "M35 parked
nothing new and un-parked nothing", the reconciliation table M34 → M35 (`truncguard.rs` 21 → 22;
`failwrite.rs` 1 → 1; `failsys_e2e.rs` new, 2; `--bins` 11 → 11; binaries 124 → 125), the "+2
twice" note carried unchanged, the bare-grep figure.

- [ ] **Step 3: Status log — append the M35 section**

Append to `docs/status-log.md` a section `## Status: M35-errholes — a failing syscall writes
after all, and a clamp that hid the proof`, in the M34 section's shape: what it set out to do
(the charter's medium certainty and why); the premise measurement verbatim (spec §4a, with the
binary and the fixture named); the xnu reading (§4b, three functions, three lines); the host
measurement (§4c verbatim); what changed (per file, with `:line` citations from the tree at the
final code commit — not the spec's, which are at the M34 merge); the three positive controls
with their red output from the task reports; Task 2 Step 5's smoke (four rcs, two canary counts,
and whatever landmark count was obtained); the sweep (from the numbers file, with any moved
binary named and routed to M36); the gate table and reconciliation; what stays owed (carry every
M34 item by name; add: `csops`' `ERANGE` header write, unmeasured; the band's width, still).
State M28's Task 4 conclusion as superseded here **with a pointer to M28's section**, and do not
edit M28's section.

- [ ] **Step 4: Spec §11**

Replace `*(Written at close.)*` with the outcome: what landed against what §5 said would, every
number the section quotes (gate, sweep, reconciliation, controls), the rulings the run added, and
any spec line numbers the tree moved.

- [ ] **Step 5: Commit the docs**

```bash
git add README.md docs/status-log.md docs/superpowers/specs/2026-09-13-retrace-m35-errholes-design.md
git commit -m "M35 t4: README, status-log section, spec outcome"
```

- [ ] **Step 6 (controller): the sweep**

`tools/apple-sweep.sh` on the branch's signed binary; compare the FAIL set to M34's
(`csh`, `tcsh`, `launchctl`, `automationmodetool`, `desdp`, `dyld_info`, `flex`, `yes`; 46/8).
Any binary that moves — either direction — is named in the numbers file with the sweep's reason
and goes into the status-log section as an M36 row.

- [ ] **Step 7 (controller): the gate, then the merge**

The full chunked gate in CLAUDE.md's shape (`ws`, `box`, `e2e` ×3 index-free groups of twenty
plus the one new target, `bins`, `clippy`), exit codes captured before pipes, logs sanitised,
`#[test]` reconciled file-by-file against M34's 572 / 0 / 2 over 124. Prediction: **575 / 0 / 2
over 125**. Then `git merge --no-ff m35-errholes` into local `main`; never push.

---

## Self-Review

**Spec coverage.** §3a → Task 1 Step 3. §3b → Task 2 Step 3 (the three comment sites in §5a are
all in Step 3). §3c → the Global Constraint that the fd gates stay. §4a/§4b → Task 2's test
comment and Step 2's "stop if `oldlen_after` ≠ 0". §4c → Task 3 Step 1 (the guest) and the
`assert_ne!(data, zeros)`. §5b → Task 2 Step 1. §5c → Task 3 Steps 1–2. §5d → Task 3 Step 3.
§5e → Task 4 Steps 1–4. §6 Controls 1/2/3 → Task 1 Step 2, Task 2 Step 2 + Task 3 Step 5, Task 3
Step 5. §10 sweep and gate → Task 4 Steps 6–7. §7's "not done" list needs no task.

**Placeholder scan.** No TBD/TODO. Every code step carries its code. Task 4's prose steps name
their source (the numbers file, the task reports) rather than inventing figures.

**Type consistency.** `captured(&[Region], u64, usize) -> Option<Vec<u8>>` is defined and used
in Task 3 only; Task 2's inline `find_map` is the same expression. `retrace_trace::Region` is
built with `{ ipa, bytes }` in Task 1, matching `fixedstraddle.rs`. `the_sysctl` returns `([u64;
8], bool, Vec<Region>)` and both tests destructure it that way. `retrace_guest::FAILPROC` is
declared in Task 3 Step 2 before `failsys_e2e.rs` uses it.

**One thing the plan cannot know:** whether a real guest's failing syscall disturbs its band now
that the check is live on the error path (Ruling 3). Task 2 Step 5 measures it on the two
guests that matter before Task 3 builds on the change; the sweep and the gate measure the rest.
