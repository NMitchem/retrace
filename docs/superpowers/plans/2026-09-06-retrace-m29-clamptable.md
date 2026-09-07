# M29-clamptable Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the `dest_buffer` audit table — pay the owed `DerefU64` gap with a measured refusal, add the four syscalls that fit the existing shape, and make M28's band-suppression count an observation the gate actually takes.

**Architecture:** Three independent components over one seam, the record-side memory-diff in `Box_::forward_and_diff`. `retrace-arch` owns the syscall facts (a pure table, no VM); `retrace-box` owns the clamp, the refusal and the diagnostic; `crates/retrace` owns the end-to-end assertion. Nothing here touches `Event`, `TRACE_MAGIC`, or the replay side.

**Tech Stack:** Rust 1.95.0 (pinned, `aarch64-apple-darwin`), macOS 26.5 SDK, Hypervisor.framework, `/bin/sh` for the sweep script, arm64 assembly for guest fixtures.

**Spec:** `docs/superpowers/specs/2026-09-06-retrace-m29-clamptable-design.md`

## Global Constraints

- **`--test-threads=1` is mandatory** on every `cargo test` invocation. HVF allows one VM per process; a bare `cargo test` flakes with `HV_BUSY`.
- **Never bump `TRACE_MAGIC`** (currently `RT\x00\x09`) and never change `Event`'s shape. No task here has any reason to touch `crates/retrace-trace` or `crates/retrace-core`. If you believe you need to, stop and escalate.
- **`forward_and_diff` is record-side only.** An `assert!` mints no landmark, so nothing in this plan owes a replay mirror. Do not add one.
- **`clippy.toml` denials are load-bearing:** no `Instant::now`/`SystemTime::now` (determinism), no `std::thread::Thread` (the recorder is single-threaded by design). Every task ends clippy-clean at `-D warnings`.
- **Do not weaken or delete an existing assertion** to make a new test pass. If an existing test fails, that is a finding — report it, do not edit around it.
- **Guest fixtures are freestanding** (`-nostdlib -static`), compiled by `crates/retrace-guest/build.rs` into `OUT_DIR`, reached through a path constant in `crates/retrace-guest/src/lib.rs`.
- **A skipped test must announce itself** with a loud `eprintln!` saying the gate did not run. A silent skip reads as a green it did not earn.
- Commit messages end with:
  ```
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01GsCTi11rokPvMP9y3ngSZy
  ```

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `tools/apple-sweep.sh` | **create** — record+replay each binary in the list, print one line each plus a tally | 1 |
| `tools/apple-sweep-binaries.txt` | **create** — the committed corpus, so the sweep number is reproducible rather than remembered | 1 |
| `crates/retrace-arch/src/lib.rs` | four new `SYS_*` constants, five new `dest_buffer` arms, two new `fd_operands` entries, unit tests | 2 |
| `crates/retrace-box/src/lib.rs` | `diff_window_for_test` seam (T3); `backing_of` + Phase A diagnostic (T4); Phase B refusal (T5); the `RETRACE_BANDSHRINK` second gate (T6) | 3,4,5,6 |
| `crates/retrace-box/tests/truncguard.rs` | window-widening seam test (T3), refusal tests (T5) | 3,5 |
| `crates/retrace-guest/asm/oldlensysctl.s` | **create** — a guest issuing a legal NULL-`oldp` sysctl and then an oversized-`oldlenp` one | 5 |
| `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs` | compile and expose that fixture as `OLDLENSYSCTL` | 5 |
| `crates/retrace/tests/util/mod.rs` | `record_dynamic_env` helper (no env-passing helper exists today) | 6 |
| `crates/retrace/tests/sysbin_e2e.rs` | assert the suppression count is observable | 6 |
| `README.md`, `docs/status-log.md` | the close: edit in place / append-only | 7 |

---

## Task 1: The Apple sweep, scripted

**Files:**
- Create: `tools/apple-sweep.sh`
- Create: `tools/apple-sweep-binaries.txt`

**Interfaces:**
- Consumes: nothing.
- Produces: `tools/apple-sweep.sh [path-to-retrace-binary]`, printing `PASS <path>` / `FAIL <path>` / `SKIP <path>` per binary and a final `TALLY pass=<n> fail=<n> skip=<n>`. Tasks 4 and 7 run it.

**Why this exists:** the "47 of 54" sweep has been reconstructed by hand every time a milestone quoted it since M22. Task 4 needs it to measure and Task 7 needs it to prove nothing regressed, so it gets committed once.

**A correctness note you must not skip:** every binary that calls `hv_*` needs the `com.apple.security.hypervisor` entitlement. `cargo run` gets it from `.cargo/config.toml`'s runner; a raw `target/.../retrace` does **not**. The script signs a copy itself, the same pattern as `crates/retrace/tests/util/mod.rs::bin()`.

- [ ] **Step 1: Write the binary list**

Create `tools/apple-sweep-binaries.txt`. Start with every file in `/bin` (37 on macOS 26.5) and add the `/usr/bin` entries named below — the five that the README records as failing there, plus enough neighbours to make a representative sample.

```bash
{
  echo "# The Apple-binary sweep corpus. One absolute path per line; '#' comments."
  echo "# Committed so the sweep number is reproducible rather than remembered."
  echo "# Criterion for PASS: record and replay both complete, their exit codes are"
  echo "# equal, and their stdout is byte-identical. A guest that legitimately exits"
  echo "# nonzero (e.g. /bin/false) still PASSes when both runs agree."
  ls -1 /bin | sed 's|^|/bin/|'
  for b in automationmodetool desdp dyld_info flex dddiagnose \
           basename dirname env head tail sort uniq cut tr yes true printenv; do
    echo "/usr/bin/$b"
  done
} > tools/apple-sweep-binaries.txt
wc -l tools/apple-sweep-binaries.txt
```

**Do not force the total to 54.** The original 54-binary sample was never committed, so this list is a *new* baseline that happens to be built the same way. Record whatever count it produces. If the pass tally differs from the README's 47, that is a finding to report in your task report, not a number to massage.

- [ ] **Step 2: Write the script**

Create `tools/apple-sweep.sh`:

```sh
#!/bin/sh
# Record and replay every binary in apple-sweep-binaries.txt.
#
# PASS means: both commands completed, their exit codes are equal, and their stdout
# is byte-identical. Exit codes are compared to each other, NOT to zero — /bin/false
# exits 1 on both runs and is a pass. A recorder panic is a FAIL even if the codes
# happen to match, so stderr is checked for it explicitly.
#
# Usage: tools/apple-sweep.sh [path-to-retrace-binary]
#        defaults to target/aarch64-apple-darwin/debug/retrace
set -u

ROOT=$(cd "$(dirname "$0")/.." && pwd)
RAW=${1:-$ROOT/target/aarch64-apple-darwin/debug/retrace}
LIST=$ROOT/tools/apple-sweep-binaries.txt

[ -x "$RAW" ] || { echo "no retrace binary at $RAW (cargo build -p retrace first)" >&2; exit 2; }

# Every hv_* caller needs the hypervisor entitlement, and a raw cargo output binary
# does not have it — .cargo/config.toml's runner only signs what cargo itself invokes.
# Sign a copy rather than the original: two concurrent users must never write one path.
BIN=$RAW-sweep-$$
cp "$RAW" "$BIN"
codesign -s - -f --entitlements "$ROOT/retrace.entitlements" "$BIN" >/dev/null 2>&1 \
  || { echo "codesign failed for $BIN" >&2; exit 2; }
TMP=$(mktemp -d -t retrace-sweep)
trap 'rm -rf "$TMP" "$BIN"' EXIT INT TERM

pass=0; fail=0; skip=0
while IFS= read -r g; do
    case "$g" in ''|\#*) continue ;; esac
    if [ ! -x "$g" ]; then echo "SKIP $g (not present)"; skip=$((skip+1)); continue; fi

    "$BIN" record-dyn "$g" -o "$TMP/t.bin" >"$TMP/rec.out" 2>"$TMP/rec.err"; rc=$?
    if grep -qa "panicked at" "$TMP/rec.err"; then
        echo "FAIL $g (recorder panicked)"; fail=$((fail+1)); continue
    fi
    "$BIN" replay "$TMP/t.bin" >"$TMP/rp.out" 2>"$TMP/rp.err"; rp=$?

    if [ "$rc" -eq "$rp" ] && cmp -s "$TMP/rec.out" "$TMP/rp.out"; then
        echo "PASS $g"; pass=$((pass+1))
    else
        echo "FAIL $g (record=$rc replay=$rp)"; fail=$((fail+1))
    fi
done < "$LIST"

echo "TALLY pass=$pass fail=$fail skip=$skip"
```

- [ ] **Step 3: Make it executable and run it**

```bash
chmod +x tools/apple-sweep.sh
cargo build -p retrace
tools/apple-sweep.sh 2>&1 | tee /tmp/m29-sweep-baseline.txt | tail -5
```

Expected: a `PASS`/`FAIL` line per binary and a `TALLY` line. This takes several minutes — each binary is a full record plus replay.

- [ ] **Step 4: Record the baseline set, not just the tally**

```bash
grep '^PASS ' /tmp/m29-sweep-baseline.txt | sort > /tmp/m29-sweep-baseline-set.txt
grep '^FAIL ' /tmp/m29-sweep-baseline.txt | sort
wc -l < /tmp/m29-sweep-baseline-set.txt
```

Copy the full `FAIL` list and the tally into your task report **verbatim**. Task 7 compares the *set* against this, not the count — a binary dropping out while another joins nets to the same number and would hide a regression.

- [ ] **Step 5: Commit**

```bash
git add tools/apple-sweep.sh tools/apple-sweep-binaries.txt
git commit -m "M29-clamptable t1: script the Apple sweep so its number is reproducible

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GsCTi11rokPvMP9y3ngSZy"
```

---

## Task 2: The four table additions

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (constants near the other `SYS_*`; `dest_buffer`; `fd_operands`; tests at the bottom)

**Interfaces:**
- Consumes: `DestLen` (already `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`, so `assert_eq!` on it compiles today).
- Produces: `SYS_RECVFROM = 29`, `SYS_SYSCTLBYNAME = 274`, `SYS_GETFSSTAT64 = 347`, `SYS_RECVFROM_NOCANCEL = 403`; `dest_buffer` returning `Some((1, DestLen::Reg(2)))` for 344/29/403, `Some((0, DestLen::Reg(1)))` for 347, `Some((2, DestLen::DerefU64(3)))` for 274 (CORRECTED in Task 4 fix round 1, Ruling 14 — this plan originally printed `Some((1, DestLen::DerefU64(2)))`, the libc wrapper's indices rather than the raw syscall's; measured wrong against the live kernel, see the code block below). Tasks 3, 4 and 5 depend on these.

This is a pure table crate with no VM and no I/O. All numbers below are verified against the macOS 26.5 SDK (`sys/syscall.h`, `sys/socket.h`, `sys/mount.h`).

- [ ] **Step 1: Write the failing tests**

Add to the test module at the bottom of `crates/retrace-arch/src/lib.rs`, beside the existing `dest_buffer` tests:

```rust
    #[test]
    fn dest_buffer_knows_the_m29_additions() {
        // getdirentries64(fd, buf, bufsize, off_t *position) — destination x1, length x2.
        assert_eq!(dest_buffer(SYS_GETDIRENTRIES64), Some((1, DestLen::Reg(2))));
        // getfsstat64(struct statfs64 *buf, int bufsize, int flags) — destination x0, and the
        // length in x1 is BYTES, not a mount count.
        assert_eq!(dest_buffer(SYS_GETFSSTAT64), Some((0, DestLen::Reg(1))));
        // recvfrom(s, buf, len, flags, sockaddr *from, socklen_t *fromlen) — both spellings.
        assert_eq!(dest_buffer(SYS_RECVFROM), Some((1, DestLen::Reg(2))));
        assert_eq!(dest_buffer(SYS_RECVFROM_NOCANCEL), Some((1, DestLen::Reg(2))));
        // sysctlbyname: the RAW syscall's shape (name, namelen, oldp, oldlenp, newp, newlen) —
        // IDENTICAL indices to `SYS_SYSCTL`, not "one index lower" as this plan originally (and
        // wrongly) printed; the raw syscall takes `namelen` first, which libc's `sysctlbyname(3)`
        // C prototype hides. CORRECTED in Task 4 fix round 1 (Ruling 14), measured against the
        // live kernel with a raw `syscall(274, ...)` bypassing the libc wrapper. It was missing
        // from this table AND from the README's list of what was missing.
        assert_eq!(dest_buffer(SYS_SYSCTLBYNAME), Some((2, DestLen::DerefU64(3))));
    }

    #[test]
    fn recvfrom_translates_its_socket_fd() {
        // sendto (133) has been in fd_operands since the fd table landed; recvfrom was not, so a
        // guest receiving on a socket handed the host kernel an untranslated guest fd — the M10
        // class, and the same both-tables-at-once asymmetry M27 found in pread_nocancel.
        assert_eq!(fd_operands(SYS_RECVFROM), &[0]);
        assert_eq!(fd_operands(SYS_RECVFROM_NOCANCEL), &[0]);
        // getfsstat64 and sysctlbyname take no fd, and must NOT have gained one.
        assert_eq!(fd_operands(SYS_GETFSSTAT64), &[] as &[usize]);
        assert_eq!(fd_operands(SYS_SYSCTLBYNAME), &[] as &[usize]);
    }
```

- [ ] **Step 2: Run them and watch them fail**

```bash
cargo test -p retrace-arch -- --test-threads=1 2>&1 | tail -20
```

Expected: FAIL to **compile**, with `cannot find value SYS_RECVFROM in this scope` (and the other three constants). A compile failure is the correct first red here — the constants do not exist yet.

- [ ] **Step 3: Add the four constants**

In `crates/retrace-arch/src/lib.rs`, beside the existing `SYS_*` block (the one holding `SYS_FSTATFS64` / `SYS_GETDIRENTRIES64` around line 74-79):

```rust
/// `recvfrom(int s, void *buf, size_t len, int flags, struct sockaddr *from, socklen_t *fromlen)`.
/// SDK `sys/syscall.h`. Its `x0` is a socket fd, so it belongs in `fd_operands` too — it was in
/// neither table before M29, while its `sendto` counterpart was already in `fd_operands`.
pub const SYS_RECVFROM: u64 = 29;
/// The `_nocancel` spelling of `recvfrom`. Every `_nocancel` variant this repo has met so far was
/// missing from a table its plain sibling was in (M9's console bug, M10's `read_nocancel`, M27's
/// `pread_nocancel`); adding both together is the only way that trap stops repeating.
pub const SYS_RECVFROM_NOCANCEL: u64 = 403;
/// `sysctlbyname(const char *name, void *oldp, size_t *oldlenp, void *newp, size_t newlen)`.
pub const SYS_SYSCTLBYNAME: u64 = 274;
/// `getfsstat64(struct statfs64 *buf, int bufsize, int flags)`. `bufsize` is in BYTES.
pub const SYS_GETFSSTAT64: u64 = 347;
```

- [ ] **Step 4: Add the `dest_buffer` arms**

Replace the body of `dest_buffer` with:

```rust
pub fn dest_buffer(num: u64) -> Option<(usize, DestLen)> {
    match num {
        SYS_READ | SYS_READ_NOCANCEL | SYS_PREAD | SYS_PREAD_NOCANCEL => Some((1, DestLen::Reg(2))),
        // sysctl(name, namelen, oldp, oldlenp, newp, newlen): the destination is x2 and its length
        // is `*(size_t*)x3`, in guest memory rather than a register. Measured via /bin/ps, whose
        // KERN_PROC_ALL buffer runs far past the 64 KiB window (M26).
        SYS_SYSCTL => Some((2, DestLen::DerefU64(3))),
        // M29 additions. Each names its own second destination where it has one, so a later reader
        // can see it was considered and dismissed on a number rather than overlooked.
        //
        // getdirentries64(fd, buf, bufsize, off_t *position): destination x1, length x2. It also
        // writes 8 bytes at `*position` (x3) — unmodelled by decision: 8 bytes sits far inside the
        // flat 64 KiB window every pointer argument already receives, so it cannot produce the
        // truncation class this table exists to prevent.
        SYS_GETDIRENTRIES64 => Some((1, DestLen::Reg(2))),
        // getfsstat64(buf, bufsize, flags): destination x0, length x1 in BYTES rather than a mount
        // count. ~24 mounts x sizeof(struct statfs64) on this machine, which crosses the 64 KiB cap.
        SYS_GETFSSTAT64 => Some((0, DestLen::Reg(1))),
        // recvfrom(s, buf, len, flags, from, fromlen): destination x1, length x2. It also writes
        // `from` (x4) — unmodelled by decision: the kernel caps that write at the real address size
        // (`sockaddr_storage` is 128 bytes), NOT at `*fromlen`, so it is self-bounding and already
        // deep inside the flat window.
        SYS_RECVFROM | SYS_RECVFROM_NOCANCEL => Some((1, DestLen::Reg(2))),
        // sysctlbyname: the RAW syscall's shape, not libc's 5-arg wrapper. `sysctlbyname(3)`'s C
        // signature is (name, oldp, oldlenp, newp, newlen), but the kernel entry point behind it
        // takes an extra `namelen` first, exactly like `SYS_SYSCTL`: (name, namelen, oldp, oldlenp,
        // newp, newlen). CORRECTED in Task 4 fix round 1 (Ruling 14) — this plan originally printed
        // `Some((1, DestLen::DerefU64(2)))` ("one index lower"), measured wrong against the live
        // kernel with a raw `syscall(274, ...)` bypassing libc's wrapper: the 6-arg form succeeds,
        // the naive 5-arg reading fails. `oldp` is index 2 and `oldlenp` index 3 — IDENTICAL to
        // `SYS_SYSCTL`. One `DerefU64` arm covers both syscalls because both use the same indices,
        // not despite them differing by one.
        SYS_SYSCTLBYNAME => Some((2, DestLen::DerefU64(3))),
        _ => None,
    }
}
```

- [ ] **Step 5: Add the two `fd_operands` entries**

In `fd_operands`, extend the arm that already returns `&[0]` — add `| SYS_RECVFROM | SYS_RECVFROM_NOCANCEL` next to `SYS_CONNECT | SYS_SENDTO`, and extend that line's comment:

```rust
        | SYS_CONNECT | SYS_SENDTO | SYS_RECVFROM | SYS_RECVFROM_NOCANCEL | SYS_FGETATTRLIST
```

- [ ] **Step 6: Run the tests and watch them pass**

```bash
cargo test -p retrace-arch -- --test-threads=1 2>&1 | tail -5
cargo clippy -p retrace-arch --all-targets -- -D warnings
```

Expected: the package's test count rises by exactly 2 over its previous total, 0 failed; clippy silent.

- [ ] **Step 7: Confirm nothing downstream broke**

Adding to `dest_buffer` widens diff windows and adds a clamp where neither applied before, so the box's own suite is the first place a mistake shows:

```bash
cargo test -p retrace-box -- --test-threads=1 2>&1 | grep -a "test result"
```

Expected: all green, no count change (this task adds no `retrace-box` tests).

- [ ] **Step 8: Commit**

```bash
git add crates/retrace-arch/src/lib.rs
git commit -m "M29-clamptable t2: four syscalls that already fit the table's shape

getdirentries64 (344), getfsstat64 (347), recvfrom (29/403) and
sysctlbyname (274). recvfrom was missing from fd_operands while sendto
was present — the M10 class. sysctlbyname was missing from the table and
from the README's own list of what was missing.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GsCTi11rokPvMP9y3ngSZy"
```

---

## Task 3: Prove the window actually widens for the new entries

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (add one test seam beside the existing `_for_test` seams, around line 2891)
- Modify: `crates/retrace-box/tests/truncguard.rs` (append one test)

**Interfaces:**
- Consumes: Task 2's `dest_buffer` entries.
- Produces: `pub fn diff_window_for_test(&self, num: u64, i: usize, avail: usize, args: &[u64; 8]) -> usize` on `Box_`.

**Why a seam and not a guest:** proving the widening through a real guest would need a program that calls `getdirentries64` with a large buffer, which is a lot of fixture for a fact that is a pure function of the table. `Box_` already carries `set_window_cap_for_test` and `read_bytes_for_test` from M28 — this follows that established pattern.

- [ ] **Step 1: Write the failing test**

Append to `crates/retrace-box/tests/truncguard.rs`:

```rust
// M29: `dest_buffer`'s job is to widen the diff window past the flat cap for syscalls whose
// destination is bigger than the cap. That is a pure function of the table and the arguments, so
// it is tested at the seam rather than through a guest that would have to be built to call each
// of these four syscalls with a large buffer.
//
// `avail` is passed as 1 MiB so the backing never becomes the binding constraint — this test is
// about the table, and a too-small `avail` would silently make every case clamp to the same
// number and pass for the wrong reason.
#[test]
fn the_window_widens_for_each_m29_reg_addition() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO).unwrap());
    let b = Box_::load(&loaded);
    const AVAIL: usize = 1 << 20;
    const FLAT: usize = 64 * 1024; // PTR_WINDOW_CAP

    let mut args = [0u64; 8];

    args[2] = 200_000; // getdirentries64 bufsize
    assert_eq!(b.diff_window_for_test(retrace_arch::SYS_GETDIRENTRIES64, 1, AVAIL, &args), 200_000,
        "getdirentries64's destination is x1 and its length x2");

    args[1] = 150_000; // getfsstat64 bufsize, in bytes
    assert_eq!(b.diff_window_for_test(retrace_arch::SYS_GETFSSTAT64, 0, AVAIL, &args), 150_000,
        "getfsstat64's destination is x0 and its length x1");

    args[2] = 90_000; // recvfrom len
    assert_eq!(b.diff_window_for_test(retrace_arch::SYS_RECVFROM, 1, AVAIL, &args), 90_000);
    assert_eq!(b.diff_window_for_test(retrace_arch::SYS_RECVFROM_NOCANCEL, 1, AVAIL, &args), 90_000,
        "the _nocancel spelling must widen identically — that pairing is the trap M9/M10/M27 each hit");

    // An argument index that is NOT this syscall's destination still gets the flat cap. Without
    // this the test would pass even if `dest_buffer` widened every pointer argument, which would
    // be a far worse bug than the one it is guarding.
    assert_eq!(b.diff_window_for_test(retrace_arch::SYS_GETDIRENTRIES64, 3, AVAIL, &args), FLAT,
        "x3 is getdirentries64's *position, not its buffer");
    // A syscall absent from the table gets the flat cap at every index.
    assert_eq!(b.diff_window_for_test(retrace_arch::SYS_WRITE, 1, AVAIL, &args), FLAT);
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p retrace-box --test truncguard -- --test-threads=1 2>&1 | tail -20
```

Expected: FAIL to compile — `no method named diff_window_for_test found for struct Box_`.

- [ ] **Step 3: Add the seam**

In `crates/retrace-box/src/lib.rs`, immediately after `set_window_cap_for_test` / `read_bytes_for_test`:

```rust
    /// Test-only view of `diff_window`, which is private because nothing outside the diff needs to
    /// choose a window. Exposed so the M29 table additions can be proven to widen the window
    /// without building a guest per syscall — the widening is a pure function of
    /// `retrace_arch::dest_buffer` and the arguments.
    pub fn diff_window_for_test(&self, num: u64, i: usize, avail: usize, args: &[u64; 8]) -> usize {
        self.diff_window(num, i, avail, args)
    }
```

- [ ] **Step 4: Run it and watch it pass**

```bash
cargo test -p retrace-box --test truncguard -- --test-threads=1 2>&1 | grep -a "test result"
cargo clippy -p retrace-box --all-targets -- -D warnings
```

Expected: `truncguard` rises from 11 to 12 passed, 0 failed; clippy silent.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/truncguard.rs
git commit -m "M29-clamptable t3: prove the window widens for each new table entry

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GsCTi11rokPvMP9y3ngSZy"
```

---

## Task 4: Phase A — measure `*oldlenp` against the backing

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (`backing_of` helper; the `DerefU64` arm of the clamp block, currently the empty `retrace_arch::DestLen::DerefU64(_) => {}` around line 3074)

**Interfaces:**
- Consumes: Task 2's `dest_buffer` entries (so `sysctlbyname` is measured too), Task 1's sweep script.
- Produces: a `[M29 DEREFLEN]` stderr line per occurrence of `*oldlenp > avail`, gated behind `RETRACE_DEREFLEN`; `fn backing_of(&self, ipa: u64) -> Option<(u64, usize)>` on `Box_`.

**This task's deliverable is a measurement, not a feature.** Task 5 is conditional on the number you produce here, so the number and how you took it are the whole point.

- [ ] **Step 1: Add the backing-span helper**

In `crates/retrace-box/src/lib.rs`, immediately after `host_span`:

```rust
    /// The `[ipa, ipa+len)` span of the backing containing `ipa`, or `None` if unmapped.
    ///
    /// `host_span` returns *remaining* bytes from `ipa`, which cannot distinguish "this buffer is
    /// genuinely oversized" from "this buffer legitimately continues into the next backing". The
    /// M29 diagnostic reports the whole span so a reader can check whether a neighbouring backing
    /// starts exactly where this one ends — see the M29 spec's R1.
    fn backing_of(&self, ipa: u64) -> Option<(u64, usize)> {
        self.backings.iter()
            .find(|bk| ipa >= bk.ipa && ipa < bk.ipa + bk.len as u64)
            .map(|bk| (bk.ipa, bk.len))
    }
```

- [ ] **Step 2: Replace the empty `DerefU64` arm with the diagnostic**

```rust
                // M29 Phase A: measure before deciding. `*oldlenp` is an in-out length the kernel
                // also writes back, so clamping it would mean writing into guest memory the guest
                // reads back, turning a call that succeeds natively into ENOMEM — a fidelity change
                // wearing a safety fix's clothes. Whether a refusal is safe to land depends on
                // whether `want > avail` ever actually happens, which nothing had measured.
                //
                // Gated behind its own env var rather than RETRACE_TRACE, which is also the
                // full trap firehose: M28's lesson is that a diagnostic must reach a channel
                // someone can actually read without paying for tracing every dispatched trap.
                retrace_arch::DestLen::DerefU64(n) => {
                    let want = self.read_u64(args[n]) as usize;
                    if let Some((_, avail)) = self.host_span(args[di]) {
                        if want > avail && std::env::var_os("RETRACE_DEREFLEN").is_some() {
                            let (bi, bl) = self.backing_of(args[di]).unwrap();
                            eprintln!("[M29 DEREFLEN] syscall {} want {} avail {} dest {:#x} \
                                       backing [{:#x},{:#x})",
                                num as i64, want, avail, args[di], bi, bi + bl as u64);
                        }
                    }
                }
```

Note `read_u64` on a NULL or unmapped `oldlenp` behaves exactly as it already does in
`dest_len_bytes`, which reads the same pointer the same way — this arm changes nothing about that.

- [ ] **Step 3: Build and confirm the suite is unmoved**

```bash
cargo test -p retrace-box -- --test-threads=1 2>&1 | grep -a "test result"
cargo clippy -p retrace-box --all-targets -- -D warnings
```

Expected: the same counts as after Task 3 (this adds no tests), 0 failed, clippy silent.

- [ ] **Step 4: Measure part (a) — the Apple sweep**

```bash
cargo build -p retrace
RETRACE_DEREFLEN=1 tools/apple-sweep.sh > /tmp/m29-phaseA-sweep.txt 2>&1
grep -ac "\[M29 DEREFLEN\]" /tmp/m29-phaseA-sweep.txt
grep -a "\[M29 DEREFLEN\]" /tmp/m29-phaseA-sweep.txt | sort | uniq -c | sort -rn | head -20
```

- [ ] **Step 5: Measure part (b) — the heaviest `sysctl` users**

```bash
S=/tmp/m29-phaseA
RETRACE_DEREFLEN=1 cargo run -p retrace -- record-dyn /bin/ps -o /tmp/ps.bin > $S-ps.out 2>&1
grep -ac "\[M29 DEREFLEN\]" $S-ps.out

PY=/opt/homebrew/bin/python3
if [ -x "$PY" ]; then
  RETRACE_DEREFLEN=1 cargo run -p retrace -- record-dyn "$PY" -o /tmp/py.bin -- -c 'print(1)' > $S-py.out 2>&1
  grep -ac "\[M29 DEREFLEN\]" $S-py.out
else
  echo "SKIPPED CPython part: $PY not present. This part of the measurement did NOT run."
fi
```

- [ ] **Step 6: Measure part (c) — `jq`**

```bash
JQ=/opt/homebrew/bin/jq
if [ -x "$JQ" ]; then
  RETRACE_DEREFLEN=1 cargo run -p retrace -- record-dyn "$JQ" -o /tmp/jq.bin -- --version > /tmp/m29-phaseA-jq.out 2>&1
  grep -ac "\[M29 DEREFLEN\]" /tmp/m29-phaseA-jq.out
else
  echo "SKIPPED jq part: $JQ not present. This part of the measurement did NOT run."
fi
```

- [ ] **Step 7: Write the measurement into the task report**

Report **per part**, never as one sum — a single number hides which part produced it. For each part give: the count, the exact command that produced it, and (for any nonzero count) the full `[M29 DEREFLEN]` lines.

For every occurrence, state the R1 verdict explicitly: does another backing begin exactly where this one's `[ipa, ipa+len)` ends? If yes, the buffer legitimately spans backings and the refusal would be a false positive; if no, the request is genuinely larger than its backing.

State plainly whether any part was SKIPPED. A skipped part is not a zero.

- [ ] **Step 8: Commit**

```bash
git add crates/retrace-box/src/lib.rs
git commit -m "M29-clamptable t4: measure *oldlenp against its backing before refusing anything

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GsCTi11rokPvMP9y3ngSZy"
```

---

## Task 5: Phase B — refuse, if and only if Task 4 measured zero

**Files:**
- Create: `crates/retrace-guest/asm/oldlensysctl.s`
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs`
- Modify: `crates/retrace-box/src/lib.rs` (the `DerefU64` arm from Task 4)
- Modify: `crates/retrace-box/tests/truncguard.rs`

**Interfaces:**
- Consumes: Task 4's measurement and its `DerefU64` arm; `retrace_guest::FILEIO`'s build.rs registration pattern.
- Produces: `retrace_guest::OLDLENSYSCTL`.

**READ THIS FIRST — the branch:**

- **If Task 4's count is ZERO across every part that ran → Branch B-REFUSE.** Do all steps below.
- **If Task 4's count is NONZERO anywhere → Branch B-DOCUMENT.** Do **not** add the assert. Skip to Step 8, which tells you what to do instead. Do not half-implement the assert "behind a flag just in case" — an unreachable assert is a claim nobody checked.
- **If a corpus part was SKIPPED and every part that ran was zero → still Branch B-REFUSE**, but say so explicitly in your report: the evidence is narrower than the corpus the spec asked for.

### Branch B-REFUSE

- [ ] **Step 1: Write the guest fixture**

Create `crates/retrace-guest/asm/oldlensysctl.s`. It issues two `sysctl(KERN_OSTYPE)` calls in this order — the legal one first, because the second is expected to abort the recorder:

```asm
// A guest that exercises both sides of M29's DerefU64 refusal, in one program and in this
// order: the LEGAL call first (oldp == NULL, the "just tell me the size" form, which has no
// destination to bound), then an ILLEGAL one whose *oldlenp is larger than any backing.
//
// The order matters: the second call is expected to abort the recorder, so anything that must
// be observed has to happen before it.
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

    // ---- call 1: sysctl(mib, 2, NULL, &oldlen, NULL, 0) — legal, no destination buffer.
    adrp x11, oldlen@PAGE
    add  x11, x11, oldlen@PAGEOFF
    mov  x12, #0
    str  x12, [x11]
    mov  x0, x9
    mov  x1, #2
    mov  x2, #0                 // oldp == NULL
    mov  x3, x11                // oldlenp
    mov  x4, #0
    mov  x5, #0
    mov  x16, #202              // SYS_sysctl
    svc  #0x80

    // ---- call 2: *oldlenp = 1 << 40, oldp = buf — far past any backing.
    mov  x12, #1
    lsl  x12, x12, #40
    str  x12, [x11]
    adrp x13, buf@PAGE
    add  x13, x13, buf@PAGEOFF
    mov  x0, x9
    mov  x1, #2
    mov  x2, x13                // oldp = buf
    mov  x3, x11                // oldlenp, now 1 TiB
    mov  x4, #0
    mov  x5, #0
    mov  x16, #202
    svc  #0x80

    // exit(0) — reached only if the refusal did not fire, which is itself the finding.
    mov  x0, #0
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
.p2align 4
mib:      .space 16
oldlen:   .space 8
buf:      .space 64
```

- [ ] **Step 2: Register it in the guest build**

In `crates/retrace-guest/build.rs`, copy the block that compiles `failsysctl.s` (M28's fixture, the nearest neighbour — same spinloop-free shape, no generated path) at lines 71-78 and change the name to `oldlensysctl`. The link line is `-Wl,-e,_start`, which is why the fixture above declares `_start` and not `_main` — every guest in this crate does, and `_main` would not link. In `crates/retrace-guest/src/lib.rs`, add the path constant beside `FAILSYSCTL`:

```rust
/// A guest issuing a legal NULL-`oldp` `sysctl` and then one whose `*oldlenp` (1 TiB) is far
/// larger than any backing — the fixture for M29's `DerefU64` refusal.
pub const OLDLENSYSCTL: &str = concat!(env!("OUT_DIR"), "/oldlensysctl");
```

- [ ] **Step 3: Write the two failing tests**

Append to `crates/retrace-box/tests/truncguard.rs`:

```rust
// M29 Phase B. Two tests over ONE guest, split because a panic ends a test: the first drives only
// the legal call and must complete, the second drives both and must abort on the second.
//
// `expected` pins the SYSCALL NUMBER, not just a message fragment — M28's positive-control lesson.
// A message-only match would also be satisfied by the refusal firing on the wrong call.
#[test]
#[should_panic(expected = "syscall 202 asked for")]
fn an_oldlenp_past_its_backing_is_refused() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::OLDLENSYSCTL).unwrap());
    let mut b = Box_::load(&loaded);
    let mut seen = 0;
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SYSCTL => {
                seen += 1;
                b.forward_and_diff(num, args);
                assert!(seen < 2, "NOT-THE-REFUSAL: the second sysctl carries *oldlenp = 1 TiB and \
                                   forward_and_diff returned normally");
            }
            Stop::Syscall { num, args } => { b.forward_and_diff(num, args); }
            other => panic!("NOT-THE-REFUSAL: guest stopped with {other:?} before the second sysctl"),
        }
    }
}

// The other half, and the one that makes the refusal narrow rather than blunt: `oldp == NULL` is a
// legal sysctl asking only for the size. There is no destination to bound, so it must forward
// untouched — a refusal that fired here would break every size-query in every guest.
#[test]
fn a_null_oldp_sysctl_is_not_refused() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::OLDLENSYSCTL).unwrap());
    let mut b = Box_::load(&loaded);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SYSCTL => {
                assert_eq!(args[2], 0, "the FIRST sysctl this guest issues has oldp == NULL");
                b.forward_and_diff(num, args); // must not panic
                return;
            }
            Stop::Syscall { num, args } => { b.forward_and_diff(num, args); }
            other => panic!("guest stopped with {other:?} before its first sysctl"),
        }
    }
}
```

- [ ] **Step 4: Run them and watch the first fail**

```bash
cargo test -p retrace-box --test truncguard -- --test-threads=1 2>&1 | tail -25
```

Expected: `a_null_oldp_sysctl_is_not_refused` PASSES already (nothing refuses anything yet), and
`an_oldlenp_past_its_backing_is_refused` FAILS with the `NOT-THE-REFUSAL:` message — the guest ran
both calls and nothing fired. That specific failure text is the proof the test is wired to the
refusal and not to some other panic.

- [ ] **Step 5: Land the refusal**

In the `DerefU64` arm, keep Task 4's diagnostic and add the assertion after it:

```rust
                retrace_arch::DestLen::DerefU64(n) => {
                    let want = self.read_u64(args[n]) as usize;
                    // `oldp == NULL` is a legal "just tell me the size" call — there is no
                    // destination to bound, and `host_span` returning None is exactly that case.
                    if let Some((_, avail)) = self.host_span(args[di]) {
                        if want > avail && std::env::var_os("RETRACE_DEREFLEN").is_some() {
                            let (bi, bl) = self.backing_of(args[di]).unwrap();
                            eprintln!("[M29 DEREFLEN] syscall {} want {} avail {} dest {:#x} \
                                       backing [{:#x},{:#x})",
                                num as i64, want, avail, args[di], bi, bi + bl as u64);
                        }
                        // Refuse rather than clamp. Clamping would write into guest memory the
                        // guest reads back, turning a natively-succeeding call into ENOMEM; that
                        // is a fidelity change, not a safety fix. Measured across the Apple sweep,
                        // /bin/ps, CPython and jq at M29: zero occurrences, so every legitimate
                        // call forwards untouched and this only catches the unmodelled case.
                        assert!(
                            want <= avail,
                            "unmodelled: syscall {} asked for {want} bytes at ipa {:#x} whose \
                             backing holds only {avail} — forwarding it would let the host kernel \
                             write past the backing. See the M29 spec, Component 1.",
                            num as i64, args[di],
                        );
                    }
                }
```

- [ ] **Step 6: Run them and watch both pass**

```bash
cargo test -p retrace-box --test truncguard -- --test-threads=1 2>&1 | grep -a "test result"
cargo clippy -p retrace-box --all-targets -- -D warnings
```

Expected: `truncguard` rises from 12 to 14 passed, 0 failed; clippy silent.

- [ ] **Step 7: Prove the refusal broke nothing that worked**

```bash
tools/apple-sweep.sh > /tmp/m29-sweep-after-refusal.txt 2>&1
grep '^PASS ' /tmp/m29-sweep-after-refusal.txt | sort > /tmp/m29-after-set.txt
diff /tmp/m29-sweep-baseline-set.txt /tmp/m29-after-set.txt && echo "SET UNCHANGED"
cargo test -p retrace --test cpython_e2e --test jq_e2e --test sysbin_e2e --no-fail-fast \
  -- --test-threads=1 2>&1 | grep -a "test result"
```

Expected: `SET UNCHANGED`, and the three e2e targets green (or loudly skipped where Homebrew is absent). If a binary dropped out, that is R4 — report it with the assert's message; do not delete the assert.

- [ ] **Step 8: Commit**

```bash
git add crates/retrace-guest/asm/oldlensysctl.s crates/retrace-guest/build.rs \
        crates/retrace-guest/src/lib.rs crates/retrace-box/src/lib.rs \
        crates/retrace-box/tests/truncguard.rs
git commit -m "M29-clamptable t5: refuse an oldlenp past its backing, measured first

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GsCTi11rokPvMP9y3ngSZy"
```

### Branch B-DOCUMENT (Task 4 measured a nonzero count)

- [ ] **Step 8 (alternative): Land the finding instead of the assert**

Do not create the guest fixture, do not add the tests, do not add the assert. Instead:

1. Extend the `DerefU64` arm's existing comment with the measured occurrences — count, syscall, and the R1 verdict (does a neighbouring backing begin where this one ends?).
2. Commit that comment alone:

```bash
git add crates/retrace-box/src/lib.rs
git commit -m "M29-clamptable t5: the DerefU64 gap stays owed, now with a measurement

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GsCTi11rokPvMP9y3ngSZy"
```

3. Say clearly in your report that Branch B-DOCUMENT was taken and why. Task 7 must carry this into the README and the status log as a debt that is still open — better than M27's, which owed it with no measurement at all, but still open.

---

## Task 6: Make M28's suppression count observable

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (the `[M28 BANDSHRINK]` gate, ~line 3094)
- Modify: `crates/retrace/tests/util/mod.rs`
- Modify: `crates/retrace/tests/sysbin_e2e.rs`

**Interfaces:**
- Consumes: `util::record_dynamic`'s existing shape (`RunOut { code, stdout, stderr }`).
- Produces: `pub fn record_dynamic_env(guest: &str, env: &[(&str, &str)]) -> (RunOut, std::path::PathBuf)`.

**Background:** M28 claimed "the full gate shrank a band zero times". That was not a measurement its method could make — `[M28 BANDSHRINK]` is *recorder* stderr, and every e2e gate spawns the recorder as a child through `util::run`'s `Command::output()`, piping stderr into a `String` a passing test never prints. This task makes the claim checkable.

**Keep the tag spelled `[M28 BANDSHRINK]`.** It names the milestone that created the mechanism, and M28's status-log section carries a reproduction command that greps for exactly that string.

- [ ] **Step 1: Write the failing test**

Replace `ps_records_and_replays`'s body in `crates/retrace/tests/sysbin_e2e.rs` — keep every existing assertion, add the suppression check:

```rust
    let (rec, trace) = util::record_dynamic_env("/bin/ps", &[("RETRACE_BANDSHRINK", "1")]);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert!(!rec.stdout.is_empty(), "ps printed nothing; it should list at least its own process");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, rec.stdout, "replay stdout diverged from the recording");

    // M29: M28 published "the full gate shrank a band zero times", which its method could not have
    // measured — the recorder's stderr is piped into a String a passing test never prints. This is
    // that claim made checkable. `> 0` rather than `== 31`: pinning the exact count would break on
    // a different mount count, machine or OS point release, and a brittle failure here teaches
    // nothing. What matters is that the gate reaches the suppression path at all.
    let shrinks = rec.stderr.matches("[M28 BANDSHRINK]").count();
    assert!(shrinks > 0,
        "expected /bin/ps to shrink at least one guard band (M28 measured 31 by hand); saw none. \
         Either the band-suppression path stopped being reached, or RETRACE_BANDSHRINK stopped \
         reaching the recorder.");
    eprintln!("ps_records_and_replays: {shrinks} band suppressions observed");
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p retrace --test sysbin_e2e -- --test-threads=1 2>&1 | tail -20
```

Expected: FAIL to compile — `cannot find function record_dynamic_env in module util`.

- [ ] **Step 3: Add the env-passing helper**

In `crates/retrace/tests/util/mod.rs`, beside `run` and `record_dynamic`:

```rust
fn run_env(args: &[&str], env: &[(&str, &str)]) -> RunOut {
    let mut c = Command::new(bin());
    c.args(args);
    for (k, v) in env { c.env(k, v); }
    let out = c.output().unwrap();
    RunOut {
        code: out.status.code().unwrap_or(-1),
        stdout: out.stdout,
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

// Record a dynamically-linked guest with extra environment set on the RECORDER, not the guest.
// Set on the child rather than via std::env::set_var, which is process-global and `unsafe` under
// the pinned 2024-edition toolchain — a test binary runs many tests in one process.
pub fn record_dynamic_env(guest: &str, env: &[(&str, &str)]) -> (RunOut, std::path::PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(2_000_000);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let trace = std::env::temp_dir().join(format!("retrace-dynenv-{}-{n}.bin", std::process::id()));
    let out = run_env(&["record-dyn", guest, "-o", trace.to_str().unwrap()], env);
    (out, trace)
}
```

The counter starts at 2,000,000 because `record` uses 0 and `record_dynamic` uses 1,000,000 — the ranges must not collide, or two helpers in one test binary would write the same trace path.

- [ ] **Step 4: Add the second env gate**

In `crates/retrace-box/src/lib.rs`, widen the `[M28 BANDSHRINK]` condition:

```rust
                if band < raw_band
                    && (std::env::var_os("RETRACE_TRACE").is_some()
                        || std::env::var_os("RETRACE_BANDSHRINK").is_some())
                {
```

and extend the comment above it with one line:

```rust
                // M29 adds RETRACE_BANDSHRINK as a second gate so a test can turn this counter on
                // without also turning on RETRACE_TRACE's per-trap firehose.
```

- [ ] **Step 5: Run it and watch it pass**

```bash
cargo test -p retrace --test sysbin_e2e -- --test-threads=1 --nocapture 2>&1 | tail -20
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 3 passed, 0 failed, and the `band suppressions observed` line naming a number. Record that number in your report — it is the measurement M28 could not take.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace/tests/util/mod.rs \
        crates/retrace/tests/sysbin_e2e.rs
git commit -m "M29-clamptable t6: make M28's band-suppression count a real observation

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GsCTi11rokPvMP9y3ngSZy"
```

---

## Task 7: The gate and the two documents

**Files:**
- Modify: `README.md` (the `README.md:298-318` diff-window paragraph in Known limits; the gate line and its reconciliation; the Apple-sweep paragraph if the number moved)
- Modify: `docs/status-log.md` (**append only** — a new `## Status: M29-clamptable` section)

**Interfaces:**
- Consumes: every earlier task's measurements and counts.

- [ ] **Step 1: Run the full gate, chunked**

The whole workspace exceeds the tool ceiling. Run these separately, capture each exit code **before any pipe**, and use `--no-fail-fast` throughout:

```bash
cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1; echo "EXIT=$?"
cargo test -p retrace-box --no-fail-fast -- --test-threads=1; echo "EXIT=$?"
cargo test -p retrace --bins --no-fail-fast -- --test-threads=1; echo "EXIT=$?"
```

Then the `retrace` e2e targets, split into groups of at most 11 `--test` flags each (a single group of 22 was alarm-killed at M28). **Do not omit the `--bins` chunk** — the 11 unit tests in `crates/retrace/src/debug.rs` run in no other chunk and nothing warns you. `retrace-box` runs as a **whole package**, never split per-target, so its `Doc-tests` harness is not silently dropped.

```bash
cargo clippy --workspace --all-targets -- -D warnings; echo "EXIT=$?"
```

- [ ] **Step 2: Reconcile file-by-file, not by sum**

M28 closed at **532 passed / 0 failed / 2 ignored over 116 binaries**. Expected deltas:

| file | delta | from |
|---|---|---|
| `crates/retrace-arch/src/lib.rs` | +2 | Task 2 |
| `crates/retrace-box/tests/truncguard.rs` | +1 (Task 3), +2 more if Branch B-REFUSE | Tasks 3, 5 |
| everything else | 0 | — |

So 532 + 2 + 1 + 2 = **537** under B-REFUSE and 532 + 2 + 1 = **535** under B-DOCUMENT, over 116 binaries either way (`oldlensysctl.s` is a guest fixture, not a test target, and `truncguard.rs` already exists) — no new test binary is created by this milestone. Verify by diffing `#[test]` counts file-by-file against `git show main:<file>`, not by trusting the sum. Grep gate logs with `grep -a`; they carry ANSI and UTF-8 that trips plain grep.

**Publish the number only after the last commit that can change it.** M28 published its gate figure at this step and then a fix wave added three tests, leaving both documents stale by three.

- [ ] **Step 3: Re-run the sweep and compare the SET**

```bash
tools/apple-sweep.sh > /tmp/m29-sweep-final.txt 2>&1
grep '^PASS ' /tmp/m29-sweep-final.txt | sort > /tmp/m29-final-set.txt
diff /tmp/m29-sweep-baseline-set.txt /tmp/m29-final-set.txt && echo "SET UNCHANGED"
tail -1 /tmp/m29-sweep-final.txt
```

- [ ] **Step 4: Edit the README in place**

The README says what is true **now**, so edit; never add a "superseded" note.

1. **The diff-window paragraph** (`README.md:298-318`) is the one this milestone exists to shorten. Move `getdirentries64` (344), `getfsstat64` (347) and `recvfrom` (29/403) out of the "still gets a flat 64 KiB" list and into the covered list. Add `sysctlbyname` (274) as covered, noting it was missing from the table *and* from that list until M29. Leave `proc_info` (336), `getattrlist`/`fgetattrlist` (220/228) and `csops` (169/170) named as still flat.
2. **The owed clamp sentence** ("One clamp stays owed and unmeasured even for a covered syscall") — under B-REFUSE, replace it with what M29 did instead of clamping and why refusing is the right operation for an in-out length. Under B-DOCUMENT, keep it and attach the measurement.
3. **The gate line and its reconciliation**, with Step 2's numbers.
4. **The Apple-sweep paragraph** only if Step 3 showed the set moved. If it did not, change nothing there — and note that `tools/apple-sweep.sh` now makes the figure reproducible.

- [ ] **Step 5: Append to the status log**

`docs/status-log.md` is **append-only**. Add a new `## Status: M29-clamptable` section at the end and modify **no earlier section** — not M28's, not M27's. Cover: the four table additions and why `sysctlbyname` was the interesting one; the Phase A measurement per corpus part with the commands that produced it; which branch Task 5 took and why; the suppression count now observable and its value; the gate; and what stays owed (M27's coverage false negative, `diff_memory`'s `.min(avail)`, `proc_info`/`getattrlist`/`csops`, the `readv` family).

- [ ] **Step 6: Verify the append-only discipline held**

```bash
git diff main -- docs/status-log.md | grep -a "^-" | grep -av "^---"
```

Expected: **no output**. Any deleted line means an earlier section was edited — fix it before committing.

- [ ] **Step 7: Commit**

```bash
git add README.md docs/status-log.md
git commit -m "M29-clamptable t7: the gate, the two documents, and a table that is now short

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01GsCTi11rokPvMP9y3ngSZy"
```

---

## Self-Review

**Spec coverage.** Component 1 → Tasks 4 (Phase A) and 5 (Phase B, both branches). Component 2 → Task 2, with Task 3 proving the entries take effect. Component 3 → Task 6. The scripted corpus → Task 1. Testing table → Tasks 2, 3, 5, 6. R1 → Task 4 Step 7's explicit verdict. R2 → Task 2 Step 7 and Task 5 Step 7. R3 → Task 7 Step 3. R4 → Task 5 Step 7. Gate posture and both documents → Task 7. No spec section is unclaimed.

**Type consistency.** `diff_window_for_test(&self, num: u64, i: usize, avail: usize, args: &[u64; 8]) -> usize` is defined in Task 3 and used only there. `backing_of(&self, ipa: u64) -> Option<(u64, usize)>` is defined in Task 4 and used in Tasks 4 and 5 with the same destructuring. `record_dynamic_env(guest: &str, env: &[(&str, &str)]) -> (RunOut, PathBuf)` is defined and used in Task 6. `OLDLENSYSCTL` is created in Task 5 Step 2 and used in Step 3. `DestLen` already derives `PartialEq`/`Debug`, so Task 2's `assert_eq!` compiles.

**Known imprecision, stated rather than hidden.** Task 1's binary list is *reconstructed*, because the original 54-binary sample was never committed. Its tally may not be exactly 47/54, and the task says to report the difference rather than massage the list. Every later comparison is against the set that list actually produces, so nothing downstream depends on reproducing the historical number.
