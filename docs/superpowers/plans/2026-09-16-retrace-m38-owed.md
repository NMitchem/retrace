# M38-owed Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close five items from M37's owed list in one milestone — `pipe`'s second descriptor
reaches the guest (trace format `RT\x00\x0a`), `fcntl`/`ioctl` get per-command argument kinds
with `F_DUPFD` modelled, `AT_FDCWD` is honoured in the 32-bit form real guests pass, and two
forwards become deterministic refusals (`execve`/`posix_spawn`; the RCV-only message-queue
`mach_msg2` behind six parked Apple gates) — then re-baseline the sweep, gate, merge, push.

**Architecture:** Every fd-table change is a pure table operation on `FdTable` (identical on
record and replay) with a record-only host half inside `Box_::forward_and_diff`, mirrored in
`ReplaySession::advance`'s generic arm — the M37 `dup2` shape. Both refusals are the M23
`RefuseMqSend` shape: a constant return, an empty write set, replay recomputes and byte-compares.
No new returning arm in either dispatch loop, so the `verify_thread` count stays at seven. Each
item ships a repo-owned C fixture and an e2e gate asserting on the difference it makes.

**Tech Stack:** Rust 1.95 (pinned), Hypervisor.framework, C guest fixtures built by
`crates/retrace-guest/build.rs` (clang, arm64), POSIX sh (`tools/apple-sweep.sh`).

**Spec:** `docs/superpowers/specs/2026-09-16-retrace-m38-owed-design.md`

## Global Constraints

- **`--test-threads=1`** on every `cargo test` (one VM per process). The target is pinned by
  `.cargo/config.toml`; the cargo runner ad-hoc-signs binaries it runs, but a test that spawns the
  CLI itself must use `util::bin()` (it signs a copy). A hand-run of the CLI outside cargo needs
  `codesign -s - -f --entitlements retrace.entitlements <bin>` first.
- **`TRACE_MAGIC` moves exactly once, in Task 1, to `RT\x00\x0a`** (spec §3a, pre-authorised).
  No other task touches `crates/retrace-trace/src/lib.rs`.
- **No new returning arm; seven `verify_thread` sites.** Every mirror added here lives inside an
  existing generic replay arm *after* that arm's `verify_thread`. `grep -c 'self.verify_thread(' crates/retrace-core/src/lib.rs`
  prints `7` before and after every task.
- **Symmetry rule 1 by construction:** a table method (`FdTable::dup_from`, two `alloc`s for
  `pipe`) or a constant (`exec_refusal_errno`, `MACH_RCV_REFUSAL`) is called with the same
  arguments on both sides; replay byte-compares.
- **`x1` is written only for a `Ret::FdPair` row** (`returns_fd_pair(num)`), on both sides, via
  `Box_::set_ret1`. `ret1` is `0` in every other recorded event.
- **An unlisted `fcntl`/`ioctl` command keeps today's `Ptr`** (spec R5). No fail-loud default.
- **Every e2e gate asserts on the trace or on bytes** (CLAUDE.md honest-gate rule 1); the rung
  helper's exit-0 demand is the fixture-ran check, never the property.
- **Refusal `eprintln!` lines are record-only** and start with `[retrace] refusing`.
- **Halt conditions** (spec §5): a red gate surviving one fix round; any `#[ignore]` on a test not
  ignored at `a663051`; a class-E sweep row; scope the spec lacks. Stop with the branch intact and
  a written explanation.
- **Commit messages** end with the attribution line the session mandates. **Push only in Task 6
  Step 9**, after the merge to local `main`.
- **Bash tool ceiling is 10 minutes:** the sweep and the gate run `nohup … &` and are polled;
  the gate is chunked (`.superpowers/sdd/2026-09-13-retrace-m37-classb/gate.sh` copied with
  paths changed). macOS has no `timeout(1)`: bound a hand-run with
  `perl -e 'alarm shift; exec @ARGV' 120 <cmd…>`.
- **Worktree** `.claude/worktrees/m38-owed`, branch `m38-owed`; task reports under
  `.superpowers/sdd/2026-09-16-retrace-m38-owed/`; evidence under
  `docs/sweep-evidence/2026-09-16-m38/`.
- **Line numbers** in this plan are as of `423cfb1` (spec commit on `main`). Re-locate with the
  quoted text if a prior task shifted them.

---

### Task 1: `pipe` — capture `x1`, bind both ends, `TRACE_MAGIC` → `RT\x00\x0a`

**Files:**
- Modify: `crates/retrace-trace/src/lib.rs:16` (`Event::Syscall`), `:69` (`TRACE_MAGIC`), `:138`
  (test fixture event), `:298–304` (magic test), `rejects_prior_format_version`
- Modify: `crates/retrace-arch/src/lib.rs:311` (`Ret::FdPair` doc), `:953` (views),
  `:1617–1625` (test)
- Modify: `crates/retrace-box/src/lib.rs:963–980` (`host_svc`), `:2876` (beside
  `set_x0_err_and_return`), `:3191–3200` (beside `bind_returned_fd`), `:3262–3300` and
  `:3722–3745` (`forward_and_diff`), `:3773–3792` (`guest_dup2` return type)
- Modify: `crates/retrace-core/src/lib.rs` — every `Event::Syscall {` construction (≈35, all in
  `record_box`), the three `b.forward_and_diff(num, args)` calls at `:555`, `:683`, `:1175`, the
  exhaustive destructure at `:1709`, the fd block at `:2370`
- Modify: `crates/retrace-guest/build.rs` (after the `dup2_dyn` block at `:330–338`),
  `crates/retrace-guest/src/lib.rs:196` (beside `DUP2_DYN`) and its parse test at `:307`
- Create: `crates/retrace-guest/c/pipe_dyn.c`, `crates/retrace/tests/pipe_e2e.rs`
- Test: `crates/retrace-box/tests/fdtable.rs` (append)

**Interfaces:**
- Produces: `Event::Syscall { num, args, ret, ret1: u64, err, writes, thread }`;
  `retrace_arch::returns_fd_pair(num: u64) -> bool`;
  `Box_::set_ret1(&mut self, ret1: u64)`;
  `Box_::bind_returned_pair(&mut self, host_r: i32, host_w: i32) -> (u64, u64)`;
  `Box_::forward_and_diff(&mut self, num, args) -> (u64, u64, bool, Vec<Region>)` =
  `(ret, ret1, err, writes)`; `retrace_guest::PIPE_DYN`.
- Tasks 2, 4 and 5 build on the 4-tuple and on `ret1: 0` in every event they append.

- [ ] **Step 1: The trace field and the bump.** In `crates/retrace-trace/src/lib.rs`:

```rust
// line 16 — the field order is the wire order; ret1 sits beside ret.
    Syscall { num: u64, args: [u64;8], ret: u64, ret1: u64, err: bool, writes: Vec<Region>, thread: u32 },
```
```rust
// line 69
pub const TRACE_MAGIC: [u8;4] = *b"RT\x00\x0a"; // "RT" + format version 0x000a (M38: `Event::Syscall` gained `ret1`, the second return register — `pipe`'s write end; pre-M38 traces are refused whole)
```
Line 138's fixture event gains `ret1: 0,` after `ret:6,`. Rename the test at `:298` to
`magic_bumped_for_the_m38_second_return_register`, keep its comment and add one line — "M38:
`Event::Syscall` gained `ret1`; a pre-M38 record deserialises with its fields shifted, so the
version, not the CRC, is what refuses it" — and assert `*b"RT\x00\x0a"`. In
`rejects_prior_format_version`, turn the single `prior_magic` into a loop over
`[b"RT\x00\x02", b"RT\x00\x09"]` with the same body; the `\x09` case is the one this milestone
created.

- [ ] **Step 2: Run the trace crate's tests.** `cargo test -p retrace-trace -- --test-threads=1`.
  Expected: compiles, all pass (the crate is self-contained). Do NOT build the workspace yet.

- [ ] **Step 3: The view.** `crates/retrace-arch/src/lib.rs`, after `allocates_fd` at `:953`:

```rust
/// Does `num` return TWO new descriptors in `x0`/`x1` (`pipe`)? View over `arg_kinds`. The pair
/// is bound by `Box_::bind_returned_pair`, never by `bind_returned_fd`, and `x1` is written on
/// both sides only for a row this answers true for (M38).
pub fn returns_fd_pair(num: u64) -> bool { arg_kinds(num).is_some_and(|s| s.ret == Ret::FdPair) }
```
Replace `Ret::FdPair`'s doc (`:311`) with:
```rust
    /// Two new descriptors, in x0 and x1 — `pipe` (bsd/kern/sys_pipe.c, `retval[0]`/`retval[1]`).
    /// `allocates_fd` is false for it (that view means "bind ONE return via `bind_returned_fd`");
    /// `returns_fd_pair` is the view the pair path consults. M38 modelled it: `host_svc` returns
    /// `x1`, `bind_returned_pair` allocates the read end first (xnu's order), and `set_ret1` writes
    /// `x1` on both sides. Before M38 the guest received retrace's host read-end unbound in `x0`
    /// and its own stale `x1` — `/bin/csh`/`/bin/tcsh` used both ends one landmark before their
    /// `fork` wall and got EBADF twice (M37 evidence, audit 3).
    FdPair,
```
Rewrite the test at `:1617–1625`:
```rust
    // M38: pipe's two-descriptor return is bound as a PAIR — not through `allocates_fd` (that
    // view binds one return and would alias) but through `returns_fd_pair`.
    #[test]
    fn pipe_return_is_a_pair_and_both_are_bound() {
        assert_eq!(arg_kinds(42).unwrap().ret, Ret::FdPair);
        assert!(!allocates_fd(42), "binding one of pipe's two descriptors would alias");
        assert!(returns_fd_pair(42));
        assert!(!returns_fd_pair(SYS_OPEN) && !returns_fd_pair(SYS_DUP));
    }
```
`cargo test -p retrace-arch -- --test-threads=1` → pass.

- [ ] **Step 4: Failing table test.** Append to `crates/retrace-box/tests/fdtable.rs`:

```rust
// M38: pipe binds TWO slots, read end first — the order xnu fills retval[0]/retval[1].
#[test]
fn a_pair_takes_the_two_lowest_free_slots_read_end_first() {
    let mut t = FdTable::new();
    let a = t.alloc(); t.bind(a, 40);           // something already open at 3
    let (r, w) = { let r = t.alloc(); let w = t.alloc(); (r, w) };
    assert_eq!((r, w), (4, 5));
    assert!(t.is_open(r) && t.is_open(w));
    assert!(t.close(r));
    assert_eq!(t.alloc(), 4, "the read end's slot is reusable after close, the write end's is not");
    assert!(t.is_open(w));
}
```
This pins the table behaviour `bind_returned_pair` relies on (two `alloc`s). Run:
`cargo test -p retrace-box --test fdtable -- --test-threads=1` → passes already (it uses only
existing methods); keep it — it is the mirror's contract.

- [ ] **Step 5: `host_svc` returns `x1`.** `crates/retrace-box/src/lib.rs:963–980`:

```rust
// Raw macOS BSD syscall: x16 = number, args in x0..x7, `svc #0x80`. Returns (x0, x1, carry).
// Carry set => error (x0 = errno); clear => success (x0 = full 64-bit result, e.g. an mmap ptr).
// x1 is the kernel's retval[1] — meaningful for `pipe` (the write end) and captured for every
// call; the CALLER decides whether the guest sees it (`returns_fd_pair`, M38).
// The kernel may clobber the caller-saved scratch registers (x8-x15, x17); they are declared
// clobbered so the compiler keeps no live value across the svc. x18 is platform-reserved — never
// touch it. Flags are NOT preserved (we read the carry via `cset`), so no `preserves_flags`.
// SAFETY: record-only; the caller has already translated guest pointers to host addresses.
unsafe fn host_svc(num: u64, a: [u64; 8]) -> (u64, u64, bool) {
    let ret: u64;
    let ret1: u64;
    let carry: u64;
    core::arch::asm!(
        "svc #0x80",
        "cset {c}, cs",
        in("x16") num,
        inout("x0") a[0] => ret,
        inout("x1") a[1] => ret1,
        in("x2") a[2], in("x3") a[3],
        in("x4") a[4], in("x5") a[5], in("x6") a[6], in("x7") a[7],
        c = out(reg) carry,
        out("x8") _, out("x9") _, out("x10") _, out("x11") _,
        out("x12") _, out("x13") _, out("x14") _, out("x15") _, out("x17") _,
        options(nostack),
    );
    (ret, ret1, carry != 0)
}
```
Both callers (`grep -n 'host_svc(' crates/retrace-box/src/lib.rs`, two hits) destructure the
triple; the one outside `forward_and_diff` discards it as `_ret1`.

- [ ] **Step 6: `set_ret1` and `bind_returned_pair`.** After `set_x0_err_and_return` (`:2883`):

```rust
    /// M38: the second return register. Written on BOTH sides, only for a `returns_fd_pair` row
    /// — the caller gates it, so record's `set_x0_err_and_return` path and replay's
    /// `apply_and_return` path each call this with the same recorded value (symmetry rule 1).
    /// xnu writes x1 from retval[1] after every syscall; retrace leaves it stale for every other
    /// row on purpose (spec R2: narrow, measured later if ever).
    pub fn set_ret1(&mut self, ret1: u64) {
        self.vcpu.set_reg(reg::x(1), ret1).unwrap();
    }
```
After `bind_returned_fd` (`:3191–3200`):
```rust
    /// M38: bind `pipe`'s two host ends to two fresh guest slots, READ end first — the order xnu
    /// fills `retval[0]`/`retval[1]`, so the guest's numbers come out as they would natively.
    /// Returns the guest `(read, write)` pair; the write end is what `set_ret1` hands the guest.
    /// Replay mirrors this with two `alloc()`s on its own table and compares.
    pub fn bind_returned_pair(&mut self, host_r: i32, host_w: i32) -> (u64, u64) {
        let g_r = self.fds.alloc(); self.fds.bind(g_r, host_r);
        let g_w = self.fds.alloc(); self.fds.bind(g_w, host_w);
        (g_r, g_w)
    }
```

- [ ] **Step 7: `forward_and_diff` returns the 4-tuple.** Signature at `:3262`:
`pub fn forward_and_diff(&mut self, num: u64, args: [u64;8]) -> (u64, u64, bool, Vec<Region>)`.
Every early `return (e, true, Vec::new())` inside it becomes `return (e, 0, true, Vec::new())`
(the compiler lists them). The host call becomes `let (ret, ret1, err) = unsafe { host_svc(num, hargs) };`
(find it by `host_svc(num`). Replace the bind block at `:3728–3740` with:

```rust
        let (ret, ret1) = if !err && retrace_arch::allocates_fd(num) {
            let g = if num == retrace_arch::SYS_DUP {
                // `translate_fds` already answered EBADF for a source with no host mapping, and on
                // record a slot has a host mapping iff it is open, so the table cannot refuse here.
                let g = self.fds.dup(gargs[0])
                    .unwrap_or_else(|e| panic!("dup({}) forwarded to the host but the table says errno {e}: \
                        host mapping and slot kind have drifted apart", gargs[0]));
                self.fds.bind(g, ret as i32);
                g
            } else {
                self.bind_returned_fd(num, ret)
            };
            (g, 0)
        } else if !err && retrace_arch::returns_fd_pair(num) {
            // M38: pipe. Both host ends are bound, so a guest `close` of either retires it through
            // the ordinary path — before M38 both leaked in the recorder and the write end never
            // reached the guest at all (`Ret::FdPair`'s doc).
            self.bind_returned_pair(ret as i32, ret1 as i32)
        } else { (ret, 0) };
```
(Keep the existing comment above the block; the `dup` branch is unchanged, only re-nested.) The
function's final `return`/tail becomes `(ret, ret1, err, writes)`. `guest_dup2` (`:3773`) returns
`(u64, u64, bool, Vec<Region>)` with `0` in the second position on every path.

- [ ] **Step 8: Record side.** `crates/retrace-core/src/lib.rs`. The three `forward_and_diff`
  calls: `:555` and `:683` become `let (ret, _ret1, err, writes) = …` and their events get
  `ret1: 0`; `:1175` (the generic BSD arm) becomes:

```rust
                let (ret, ret1, err, writes) = b.forward_and_diff(num, args);
                // M38: pipe's write end. Gated on the row, not on `ret1 != 0`, so the guest's x1
                // is touched for exactly the rows replay touches it for.
                if retrace_arch::returns_fd_pair(num) { b.set_ret1(ret1); }
                w.append(&Event::Syscall { num, args, ret, ret1, err, writes, thread }).map_err(|e| format!("append syscall: {e}"))?; count += 1;
                b.set_x0_err_and_return(ret, err);
```
Every other `Event::Syscall {` construction in the file gets `ret1: 0,` after its `ret` field —
`cargo build -p retrace-core` lists each missing-field error (E0063); there are ≈35. Do not
guess a non-zero value anywhere: only the generic arm has one.

- [ ] **Step 9: Replay mirror.** The destructure at `:1709` becomes
`Some(Event::Syscall { num: rn, args: ra, ret, ret1, err, writes, thread: rthread })`. In the fd
block at `:2370`, after the `allocates_fd` `if` and before the `close` `if`:

```rust
                            // M38: pipe. Two `alloc()`s — the same two calls `bind_returned_pair`
                            // made on record, read end first — compared to the recorded pair. An
                            // fd the table cannot produce is reported through the same channel as
                            // a wrong single fd.
                            if !*err && retrace_arch::returns_fd_pair(num) {
                                let g_r = self.b.fds_mut().alloc();
                                let g_w = self.b.fds_mut().alloc();
                                if (g_r, g_w) != (*ret, *ret1) {
                                    return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "fd divergence: recording says syscall {num} returned the pair \
                                         ({ret}, {ret1}), but the guest's own open/close sequence yields \
                                         ({g_r}, {g_w})") });
                                }
                            }
```
And immediately before that arm's `self.b.apply_and_return(*ret, *err, writes);`:
```rust
                            if retrace_arch::returns_fd_pair(num) { self.b.set_ret1(*ret1); }
```
Every other exhaustive `Event::Syscall` pattern the compiler flags (E0027) gets `ret1: _` or
`..`. `cargo build --workspace` → clean. `cargo test -p retrace-core -- --test-threads=1` → pass
(its hand-built traces in `tests/*.rs` need `ret1: 0` too).

- [ ] **Step 10: The fixture.** `crates/retrace-guest/c/pipe_dyn.c`:

```c
// M38. The pipe fixture: both ends reach the guest as ITS OWN numbers, adjacent, read end first,
// and bytes written into the write end come back out of the read end. Prints invariants rather
// than absolute numbers (fdtable_dyn's lesson: libSystem holds one extra descriptor under
// retrace, so the first free slot is 4, not 3 — an absolute number tests libSystem, not the table).
//
// Expected stdout (record == replay, bit for bit):   pair=1\nlow=1\nbytes=pipe\n
#include <stdio.h>
#include <unistd.h>
#include <string.h>

int main(void) {
    int p[2] = { -1, -1 };
    if (pipe(p) != 0) { printf("pipe failed\n"); return 3; }
    printf("pair=%d\n", p[1] == p[0] + 1);              /* write end is the next slot */
    printf("low=%d\n", p[0] >= 3 && p[1] < 16);         /* guest numbers, not retrace's */
    char buf[8] = {0};
    if (write(p[1], "pipe", 4) != 4) { printf("write failed\n"); return 4; }
    if (read(p[0], buf, 4) != 4) { printf("read failed\n"); return 5; }
    printf("bytes=%s\n", buf);
    fflush(stdout);
    close(p[0]); close(p[1]);
    return 0;
}
```
`build.rs`, after the `dup2_dyn` block:
```rust
    // pipe_dyn: the M38 pipe fixture — both ends reach the guest as adjacent guest numbers and
    // bytes round-trip. Same recipe as hello_dyn.
    let src = format!("{}/c/pipe_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/pipe_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang pipe_dyn");
    assert!(status.success(), "pipe_dyn guest build failed");
```
`crates/retrace-guest/src/lib.rs`, beside `DUP2_DYN`:
```rust
/// M38: `pipe()`, then a write into the write end and a read from the read end — prints
/// `pair=1\nlow=1\nbytes=pipe\n` when both ends are the guest's own adjacent numbers.
pub const PIPE_DYN: &str = concat!(env!("OUT_DIR"), "/pipe_dyn");
```
and in the parse test at `:307` add, in the same shape as the `DUP2_DYN` line,
`let l = parse_macho(&std::fs::read(PIPE_DYN).unwrap()); assert!(l.entry != 0);` (copy the
assertion the neighbouring lines make).

- [ ] **Step 11: The gate — write it, run it red first.** `crates/retrace/tests/pipe_e2e.rs`:

```rust
// M38 gate. pipe's SECOND descriptor reaches the guest: the trace carries a guest-numbered write
// end in `ret1` (before M38 `x1` was never captured and the guest saw its own stale register),
// both ends are adjacent guest numbers, and bytes round-trip. Asserts on the trace and the bytes,
// never on an exit code alone (CLAUDE.md's first gate rule).
mod util;

const EXPECT_STDOUT: &[u8] = b"pair=1\nlow=1\nbytes=pipe\n";

#[test]
fn both_ends_reach_the_guest_and_bytes_round_trip() {
    let out = util::assert_rung_records_and_replays(retrace_guest::PIPE_DYN, &[], EXPECT_STDOUT);
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("pair=1"), "the write end must be the slot after the read end. Got:\n{s}");
    assert!(s.contains("bytes=pipe"), "bytes written into p[1] must come back out of p[0]. Got:\n{s}");
}

#[test]
fn the_trace_carries_a_guest_numbered_write_end_in_ret1() {
    let out = util::assert_rung_records_and_replays(retrace_guest::PIPE_DYN, &[], EXPECT_STDOUT);
    let events = retrace_trace::Reader::open(&out.trace).unwrap();
    let mut pairs = Vec::new();
    let mut others_with_ret1 = 0usize;
    for e in events.iter() {
        if let retrace_trace::Event::Syscall { num, ret, ret1, err, writes, .. } = e {
            if *num == 42 {
                assert!(writes.is_empty(), "pipe writes no guest memory");
                pairs.push((*ret, *ret1, *err));
            } else if *ret1 != 0 {
                others_with_ret1 += 1;
            }
        }
    }
    // The difference M38 makes: a write end that EXISTS in the trace, as a guest number.
    assert_eq!(pairs.len(), 1, "expected exactly one pipe landmark, saw {pairs:?}");
    let (r, w, err) = pairs[0];
    assert!(!err, "pipe must succeed: {pairs:?}");
    assert!(w == r + 1 && r >= 3 && w < 16,
        "pipe returned ({r}, {w}): both must be adjacent GUEST numbers (a host descriptor is >= 16, \
         a stale x1 is arbitrary)");
    // Narrow capture (spec R2): no other row carries a non-zero ret1.
    assert_eq!(others_with_ret1, 0, "ret1 must be 0 on every non-pipe landmark");
}

// The mirror's compare, verified able to fail (the dup2_e2e tamper pattern): a passing replay
// proves nothing about the compare on its own.
#[test]
fn a_tampered_pipe_write_end_is_caught_as_divergence() {
    let out = util::assert_rung_records_and_replays(retrace_guest::PIPE_DYN, &[], EXPECT_STDOUT);
    let mut events = retrace_trace::Reader::open(&out.trace).unwrap();
    let mut tampered = false;
    for e in events.iter_mut() {
        if let retrace_trace::Event::Syscall { num, ret1, err, .. } = e {
            if *num == 42 && !*err && !tampered { *ret1 = 99; tampered = true; }
        }
    }
    assert!(tampered, "no successful pipe landmark found to tamper");
    let mut w = retrace_trace::Writer::create(&out.trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);
    let rep = util::replay(&out.trace);
    assert_ne!(rep.code, 0, "replay must reject a write end the guest's own table cannot produce. stdout:\n{}",
        String::from_utf8_lossy(&rep.stdout));
    assert!(rep.stderr.contains("fd divergence") && rep.stderr.contains("pair"),
        "the divergence must name the pair mismatch, got stderr:\n{}", rep.stderr);
}
```
Red evidence first (the test itself cannot compile against the pre-fix tree — it names `ret1` —
so the red run is the FIXTURE on the pre-fix recorder): `git stash push -- crates/retrace-trace crates/retrace-arch crates/retrace-box crates/retrace-core`
(keeps the fixture wiring and this test file in the tree), then
`cargo build -p retrace-guest && P=$(ls target/aarch64-apple-darwin/debug/build/retrace-guest-*/out/pipe_dyn | head -1) && cargo run -p retrace -- record-dyn "$P" -o /tmp/m38-pipe-pre.bin`.
The pre-fix guest prints `pair=0` and then `write failed` (its `p[1]` is a stale register) —
paste that stdout into the task report as the state the gate rejects; then `git stash pop`.

- [ ] **Step 12: Green.** `cargo test -p retrace --test pipe_e2e -- --test-threads=1` → 3 passed.
  Then the fd-adjacent gates, which must not move:
  `cargo test -p retrace --test fdtable_e2e --test dup2_e2e --test dupkind_e2e --test closewrite_e2e --test stdio_e2e --test hello_dyn_e2e -- --test-threads=1`
  → all pass. `cargo test -p retrace-box -- --test-threads=1` → pass.

- [ ] **Step 13: Clippy + commit.** `cargo clippy --workspace --all-targets -- -D warnings` clean.
  `grep -c 'self.verify_thread(' crates/retrace-core/src/lib.rs` → `7`.
  Commit: `M38 t1: pipe's write end reaches the guest — ret1 in the trace (RT\x00\x0a), both ends bound`.

---

### Task 2: `fcntl`/`ioctl` per-command kinds; `F_DUPFD` modelled

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` — after `forwarded_shape` (`:937–946`): `shape_of`,
  `is_fcntl_dupfd`, the command constants; the row comments at `:491–495` and `:520–524`;
  `mod tests`
- Modify: `crates/retrace-box/src/lib.rs` — `FdTable` (`:833–838`, after `dup`), `EBADF` (`:668`,
  add `EINVAL`), `forward_and_diff` (`:3267` short-circuit, `:3294` shape, `translate_fds`
  `:3176`), a new `guest_fcntl_dupfd` after `guest_dup2`
- Modify: `crates/retrace-core/src/lib.rs` — the replay fd block (`:2370`, before the
  `allocates_fd` `if`)
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs` (fixture wiring, as
  Task 1 Step 10)
- Create: `crates/retrace-guest/c/dupfd_dyn.c`, `crates/retrace/tests/dupfd_e2e.rs`
- Test: `crates/retrace-box/tests/fdtable.rs` (append), `crates/retrace-box/tests/fdxlat.rs`
  (append)

**Interfaces:**
- Consumes: Task 1's 4-tuple `forward_and_diff`, `ret1: 0` convention.
- Produces: `retrace_arch::shape_of(num: u64, args: &[u64; 8]) -> &'static Shape`;
  `retrace_arch::is_fcntl_dupfd(num: u64, args: &[u64; 8]) -> bool`;
  `retrace_arch::{F_DUPFD, F_GETFD, F_SETFD, F_GETFL, F_SETFL, F_PREALLOCATE, F_NOCACHE,
  F_GETPATH, F_DUPFD_CLOEXEC, F_ADDFILESIGS_RETURN, F_CHECK_LV, FIOCLEX, FIONCLEX}: u64`;
  `FdTable::dup_from(&mut self, src: u64, min: u64) -> Result<u64, u64>`; `retrace_box::EINVAL`;
  `retrace_guest::DUPFD_DYN`.

- [ ] **Step 1: Failing arch tests.** In `crates/retrace-arch/src/lib.rs` `mod tests`, beside
  `pipe_return_is_a_pair_and_both_are_bound`:

```rust
    // M38: fcntl/ioctl's third argument is COMMAND-dependent. The per-command table answers for
    // the commands the M33 census saw plus the two dup commands; anything else keeps the row's
    // `Ptr` (spec R5 — an unchanged default, not a panic).
    #[test]
    fn fcntl_and_ioctl_third_argument_kind_follows_the_command() {
        use ArgKind::*;
        let args = |cmd: u64| { let mut a = [0u64; 8]; a[1] = cmd; a };
        for cmd in [F_DUPFD, F_GETFD, F_SETFD, F_GETFL, F_SETFL, F_NOCACHE, F_DUPFD_CLOEXEC] {
            assert_eq!(shape_of(SYS_FCNTL, &args(cmd)).args[2], Scalar, "fcntl cmd {cmd}");
            assert_eq!(shape_of(SYS_FCNTL_NOCANCEL, &args(cmd)).args[2], Scalar, "fcntl_nocancel cmd {cmd}");
        }
        for cmd in [F_PREALLOCATE, F_GETPATH, F_ADDFILESIGS_RETURN, F_CHECK_LV, 999] {
            assert_eq!(shape_of(SYS_FCNTL, &args(cmd)).args[2], Ptr, "fcntl cmd {cmd}");
        }
        for cmd in [FIOCLEX, FIONCLEX] {
            assert_eq!(shape_of(SYS_IOCTL, &args(cmd)).args[2], Scalar, "ioctl cmd {cmd:#x}");
        }
        assert_eq!(shape_of(SYS_IOCTL, &args(0x4004_667f)).args[2], Ptr, "FIONREAD stays Ptr");
        // Every position but the third is the row's, and a non-fcntl number is the row itself.
        assert_eq!(shape_of(SYS_FCNTL, &args(F_SETFD)).args[0], Fd);
        assert!(std::ptr::eq(shape_of(SYS_READ, &args(0)), forwarded_shape(SYS_READ)));
    }

    #[test]
    fn f_dupfd_is_recognised_by_number_and_command() {
        let a = |n: u64, cmd: u64| { let mut a = [0u64; 8]; a[0] = n; a[1] = cmd; a };
        assert!(is_fcntl_dupfd(SYS_FCNTL, &a(4, F_DUPFD)));
        assert!(is_fcntl_dupfd(SYS_FCNTL_NOCANCEL, &a(4, F_DUPFD_CLOEXEC)));
        assert!(!is_fcntl_dupfd(SYS_FCNTL, &a(4, F_SETFD)));
        assert!(!is_fcntl_dupfd(SYS_DUP, &a(4, F_DUPFD)));
    }
```
`cargo test -p retrace-arch -- --test-threads=1` → FAILS to compile (`shape_of` undefined).

- [ ] **Step 2: The table.** After `forwarded_shape` (`:946`):

```rust
// fcntl(2) commands (sys/fcntl.h) and the two argument-less ioctl(2) requests (sys/ioctl.h,
// `_IO('f', 1)` / `_IO('f', 2)`) whose third argument's KIND the row cannot state. Numbers from
// the macOS 26 SDK headers; the fcntl set is the M33 census's (jq, CPython, the Apple sweep)
// plus the two dup commands M38 models.
pub const F_DUPFD: u64 = 0;
pub const F_GETFD: u64 = 1;
pub const F_SETFD: u64 = 2;
pub const F_GETFL: u64 = 3;
pub const F_SETFL: u64 = 4;
pub const F_PREALLOCATE: u64 = 42;
pub const F_NOCACHE: u64 = 48;
pub const F_GETPATH: u64 = 50;
pub const F_DUPFD_CLOEXEC: u64 = 67;
pub const F_ADDFILESIGS_RETURN: u64 = 97;
pub const F_CHECK_LV: u64 = 98;
pub const FIOCLEX: u64 = 0x2000_6601;
pub const FIONCLEX: u64 = 0x2000_6602;

/// The shape of a syscall about to be forwarded, with the ONE refinement `forwarded_shape` cannot
/// make: `fcntl`/`fcntl_nocancel`/`ioctl`'s third argument is an `int` for some commands and a
/// pointer for others, and the row says `Ptr` for all of them. Under `Ptr` the M37 `Scalar`-skip
/// does not apply, so a small integer is probed by `host_span` and would be rewritten if it equalled
/// a mapped IPA (M37 measured that inert on every command seen; M38 closes the class rather than the
/// instance). An UNLISTED command keeps the row's `Ptr` — the default is today's behaviour, not a
/// panic, so a command outside the census cannot newly fail a guest (spec R5). Every other
/// position, and every other syscall, is exactly `forwarded_shape(num)` — loud on an unenumerated
/// number.
pub fn shape_of(num: u64, args: &[u64; 8]) -> &'static Shape {
    use ArgKind::*;
    // `static`, not `const`: the function returns `&'static Shape`, and a static's address is
    // stable (a `&CONST` would be a promoted temporary — fine today, but `ptr::eq` in the test
    // and the row-identity argument want one address).
    static FCNTL_INT: Shape = Shape { args: &[Fd, Scalar, Scalar], ret: Ret::Plain };
    static IOCTL_INT: Shape = Shape { args: &[Fd, Scalar, Scalar], ret: Ret::Plain };
    match num {
        SYS_FCNTL | SYS_FCNTL_NOCANCEL => match args[1] {
            F_DUPFD | F_GETFD | F_SETFD | F_GETFL | F_SETFL | F_NOCACHE | F_DUPFD_CLOEXEC => &FCNTL_INT,
            _ => forwarded_shape(num),
        },
        SYS_IOCTL => match args[1] {
            FIOCLEX | FIONCLEX => &IOCTL_INT,
            _ => forwarded_shape(num),
        },
        _ => forwarded_shape(num),
    }
}

/// `fcntl(fd, F_DUPFD | F_DUPFD_CLOEXEC, min)` — the descriptor-producing fcntl commands, which
/// `forward_and_diff` short-circuits into `guest_fcntl_dupfd` and replay mirrors with
/// `FdTable::dup_from` (M38). `min` is a GUEST minimum and never reaches the host.
pub fn is_fcntl_dupfd(num: u64, args: &[u64; 8]) -> bool {
    (num == SYS_FCNTL || num == SYS_FCNTL_NOCANCEL) && (args[1] == F_DUPFD || args[1] == F_DUPFD_CLOEXEC)
}
```
Update the fcntl row comment (`:491–494`) to end with: "M38: the per-command refinement is
`shape_of`; `F_DUPFD`/`F_DUPFD_CLOEXEC` are table operations (`guest_fcntl_dupfd`), never
forwarded." and the ioctl row comment (`:520–523`) with "M38: `FIOCLEX`/`FIONCLEX` are `Scalar`
via `shape_of`." `cargo test -p retrace-arch -- --test-threads=1` → pass.

- [ ] **Step 3: Failing table tests.** Append to `crates/retrace-box/tests/fdtable.rs`:

```rust
// M38: F_DUPFD — the lowest free slot >= min, carrying the source's KIND (the M37 dup rule).
#[test]
fn dup_from_takes_the_lowest_free_slot_at_or_above_min_with_the_sources_kind() {
    let mut t = FdTable::new();
    let f = t.alloc(); t.bind(f, 40);                     // 3
    assert_eq!(t.dup_from(f, 10).unwrap(), 10);
    assert_eq!(t.dup_from(f, 10).unwrap(), 11, "10 is taken now");
    assert_eq!(t.dup_from(f, 0).unwrap(), 4, "a minimum below the table floor rounds up to the lowest free slot");
    assert_eq!(t.dup_from(1, 20).unwrap(), 20);
    assert_eq!(t.console_of(20), Some(1), "F_DUPFD on stdout is a console alias the M9 mirror must catch");
    assert_eq!(t.console_of(10), None, "a duplicate of a plain file is plain");
    assert!(t.close(10));
    assert_eq!(t.dup_from(f, 10).unwrap(), 10, "a closed slot is reusable");
    assert_eq!(t.dup_from(30, 3), Err(retrace_box::EBADF), "a closed source is EBADF");
}
```
`cargo test -p retrace-box --test fdtable -- --test-threads=1` → FAILS to compile.

- [ ] **Step 4: The table method.** After `FdTable::dup` (`:838`):

```rust
    /// M38: `fcntl(src, F_DUPFD, min)` on the guest-visible table — identical on record and
    /// replay. `Err(EBADF)` if `src` is not open. Otherwise the lowest slot >= `min` not currently
    /// open takes `src`'s KIND (`dup`'s rule: a `Console(n)` source makes an alias M9's mirror
    /// must catch) and is returned. `min` is the GUEST's minimum — it never reaches the host,
    /// whose `dup` picks any number; the binding is the caller's, as with `alloc` + `bind`.
    /// A `min` below the table floor rounds up to 3, as `alloc` does (the M5 floor).
    pub fn dup_from(&mut self, src: u64, min: u64) -> Result<u64, u64> {
        if !self.is_open(src) { return Err(EBADF); }
        let start = (min as usize).max(3);
        let gfd = (start..self.slots.len())
            .find(|&i| !matches!(self.slots[i], FdSlot::Open | FdSlot::Console(_)))
            .unwrap_or_else(|| self.slots.len().max(start));
        self.grow_to(gfd);
        self.slots[gfd] = self.slots[src as usize];
        Ok(gfd as u64)
    }
```
Beside `EBADF` (`:668`): `pub const EINVAL: u64 = 22;` (skip if it already exists —
`grep -n 'const EINVAL' crates/retrace-box/src/lib.rs`). Step 3's test → pass.

- [ ] **Step 5: Record side.** In `forward_and_diff`, directly after the `SYS_DUP2` short-circuit
  (`:3267`):

```rust
        // M38: F_DUPFD names a GUEST minimum; like dup2 it is a table operation with a host `dup`
        // behind it (never a host F_DUPFD, whose minimum would be a host number).
        if retrace_arch::is_fcntl_dupfd(num, &args) { return self.guest_fcntl_dupfd(args); }
```
Both shape consultations use the refinement: `translate_fds` (`:3176`)
`for i in retrace_arch::shape_of(num, args).fd_operands()` (its `args` is `&mut [u64; 8]` —
pass `&*args`), and the probe loop (`:3294`) `let shape = retrace_arch::shape_of(num, &args);`.
After `guest_dup2`:

```rust
    /// M38: `fcntl(fd, F_DUPFD | F_DUPFD_CLOEXEC, min)`. The table half is `FdTable::dup_from`
    /// (replay calls it with the same arguments); the host half is a plain `dup(h)`, because a
    /// host `F_DUPFD` would apply the guest's minimum to retrace's own descriptor space. The
    /// close-on-exec bit has no observable in the box (exec is refused, M38 t4), so both commands
    /// share this path. Range check as xnu's `finishdup`: a negative or too-large minimum is
    /// EINVAL, checked after the source (bsd/kern/kern_descrip.c order).
    fn guest_fcntl_dupfd(&mut self, args: [u64; 8]) -> (u64, u64, bool, Vec<Region>) {
        let (fd, min) = (args[0], args[2]);
        let Some(h) = self.fds.host(fd) else { return (EBADF, 0, true, Vec::new()); };
        if (min as i32) < 0 || min >= DUP2_MAX_FD { return (EINVAL, 0, true, Vec::new()); }
        let dup = unsafe { libc::dup(h) };
        if dup < 0 {
            let e = std::io::Error::last_os_error().raw_os_error().unwrap_or(EBADF as i32) as u64;
            return (e, 0, true, Vec::new());
        }
        match self.fds.dup_from(fd, min) {
            Ok(g) => { self.fds.bind(g, dup); (g, 0, false, Vec::new()) }
            Err(e) => { unsafe { libc::close(dup); } (e, 0, true, Vec::new()) }
        }
    }
```
(`DUP2_MAX_FD` exists — `dup2` uses it.)

- [ ] **Step 6: Replay mirror.** In the fd block at `:2370`, BEFORE the `allocates_fd` `if`:

```rust
                            // M38: F_DUPFD. The same table method record called, with the same
                            // guest arguments; the compare is the divergence check.
                            if !*err && retrace_arch::is_fcntl_dupfd(num, &args) {
                                let expect = self.b.fds_mut().dup_from(args[0], args[2]).map_err(|e| Divergence { landmark: self.idx, pc, detail: format!(
                                    "fd divergence: recording says fcntl({}, F_DUPFD, {}) returned fd {ret}, but the guest's own \
                                     open/close sequence has that source closed (errno {e})", args[0], args[2]) })?;
                                if expect != *ret {
                                    return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "fd divergence: recording says fcntl({}, F_DUPFD, {}) returned fd {ret}, but the guest's \
                                         own open/close sequence yields {expect}", args[0], args[2]) });
                                }
                            }
```
`cargo build --workspace` clean.

- [ ] **Step 7: A translation test for the Scalar third argument.** Append to
  `crates/retrace-box/tests/fdxlat.rs` (it already imports `FdTable`, `translate`, the SYS
  consts — add `SYS_FCNTL` and `F_SETFD` to the `use retrace_arch::{…}` line):

```rust
// M38: fcntl's fd is translated whatever the command; the third argument is never an fd.
#[test]
fn fcntl_translates_only_its_descriptor() {
    let mut t = FdTable::new();
    let g = t.alloc(); t.bind(g, 17);
    let mut args = [0u64; 8];
    args[0] = g; args[1] = F_SETFD; args[2] = 1;
    assert!(translate(&t, SYS_FCNTL, &mut args).is_ok());
    assert_eq!(args[0], 17);
    assert_eq!((args[1], args[2]), (F_SETFD, 1), "cmd and arg are forwarded verbatim");
}
```
`cargo test -p retrace-box --test fdxlat -- --test-threads=1` → pass.

- [ ] **Step 8: The fixture.** `crates/retrace-guest/c/dupfd_dyn.c`:

```c
// M38. The F_DUPFD fixture: the new descriptor honours the GUEST minimum (exactly 10 — nothing
// that low is open), writes through it reach the file, F_SETFD on it is a plain int argument,
// and F_DUPFD on stdout is a console alias (mirrored, not forwarded). argv[1] is a file path the
// test owns.
//
// Expected stdout (record == replay, bit for bit):   n=10\nsetfd=0\nalias\n
// Expected file after record:                        dupfd\n
#include <stdio.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>

int main(int argc, char **argv) {
    if (argc < 2) return 2;
    int f = open(argv[1], O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (f < 0) return 3;
    int n = fcntl(f, F_DUPFD, 10);
    printf("n=%d\n", n);
    if (n < 0) return 4;
    if (write(n, "dupfd\n", 6) != 6) return 5;
    printf("setfd=%d\n", fcntl(n, F_SETFD, FD_CLOEXEC));
    fflush(stdout);                       /* stdio is a pipe under retrace: flush BEFORE the raw write, or "alias" lands first */
    int a = fcntl(1, F_DUPFD_CLOEXEC, 12);
    if (a < 0) return 6;
    write(a, "alias\n", 6);
    close(a); close(n); close(f);
    return 0;
}
```
Wire it in `build.rs` and `lib.rs` exactly as Task 1 Step 10 did for `pipe_dyn` (name
`dupfd_dyn`, const `DUPFD_DYN`, doc: "M38: `fcntl(F_DUPFD, 10)` returns 10, writes through it
reach the file, `F_DUPFD_CLOEXEC` on stdout is a console alias."), plus the parse-test line.

- [ ] **Step 9: The gate — red first.** `crates/retrace/tests/dupfd_e2e.rs`:

```rust
// M38 gate. F_DUPFD is modelled: the returned descriptor is a GUEST number honouring the guest
// minimum (a host dup could never return 10 in a process holding 0-16 open), the file receives
// the bytes written through it, F_DUPFD_CLOEXEC on stdout is a console alias the M9 mirror sees,
// and F_SETFD's int argument is forwarded verbatim. Bytes and trace, never an exit code alone.
mod util;

const EXPECT_STDOUT: &[u8] = b"n=10\nsetfd=0\nalias\n";
const EXPECT_FILE: &[u8] = b"dupfd\n";

fn scratch_file(test: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("retrace-dupfd-{}-{test}.txt", std::process::id()))
}

#[test]
fn f_dupfd_honours_the_guest_minimum_and_writes_reach_the_file() {
    let path = scratch_file("bytes");
    let _ = std::fs::remove_file(&path);
    let out = util::assert_rung_records_and_replays(retrace_guest::DUPFD_DYN, &[path.to_str().unwrap()], EXPECT_STDOUT);
    let file = std::fs::read(&path).expect("the fixture created its file on record");
    assert_eq!(file, EXPECT_FILE, "file bytes: {:?}", String::from_utf8_lossy(&file));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.starts_with("n=10\n"), "fcntl(f, F_DUPFD, 10) must return the guest number 10. Got:\n{s}");
    assert!(s.ends_with("alias\n"), "write through the F_DUPFD_CLOEXEC alias of stdout must be a mirrored console write. Got:\n{s}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_trace_carries_f_dupfd_returning_the_guest_slot_and_f_setfd_forwarded_verbatim() {
    let path = scratch_file("trace");
    let _ = std::fs::remove_file(&path);
    let out = util::assert_rung_records_and_replays(retrace_guest::DUPFD_DYN, &[path.to_str().unwrap()], EXPECT_STDOUT);
    let events = retrace_trace::Reader::open(&out.trace).unwrap();
    let (mut dupfds, mut setfds) = (Vec::new(), Vec::new());
    for e in events.iter() {
        if let retrace_trace::Event::Syscall { num, args, ret, err, writes, .. } = e {
            if *num == retrace_arch::SYS_FCNTL || *num == retrace_arch::SYS_FCNTL_NOCANCEL {
                match args[1] {
                    retrace_arch::F_DUPFD | retrace_arch::F_DUPFD_CLOEXEC => {
                        assert!(writes.is_empty(), "F_DUPFD writes no guest memory");
                        dupfds.push((args[0], args[2], *ret, *err));
                    }
                    retrace_arch::F_SETFD => setfds.push((args[0], args[2], *ret, *err)),
                    _ => {}
                }
            }
        }
    }
    // (f, 10) -> 10 and (1, 12) -> 12: the return is the lowest free GUEST slot >= min.
    assert_eq!(dupfds.len(), 2, "expected two F_DUPFD landmarks, saw {dupfds:?}");
    assert!(dupfds.iter().all(|(_, min, ret, err)| !*err && ret == min),
        "each F_DUPFD must return exactly its minimum (nothing that high is open): {dupfds:?}");
    assert!(setfds.iter().any(|(fd, arg, ret, err)| *fd == 10 && *arg == 1 && *ret == 0 && !*err),
        "fcntl(10, F_SETFD, 1) must be forwarded with its int argument verbatim and succeed: {setfds:?}");
    let _ = std::fs::remove_file(&path);
}
```
Red evidence first (the test names `retrace_arch::F_DUPFD`, so it cannot compile pre-fix; the
red run is the FIXTURE on the pre-fix recorder):
`git stash push -- crates/retrace-arch crates/retrace-box crates/retrace-core`, then
`cargo build -p retrace-guest && P=$(ls target/aarch64-apple-darwin/debug/build/retrace-guest-*/out/dupfd_dyn | head -1) && cargo run -p retrace -- record-dyn "$P" -o /tmp/m38-dupfd-pre.bin -- /tmp/m38-dupfd-pre.txt`.
The pre-fix tree forwards `F_DUPFD` to the host, so the guest prints a HOST number
(`n=1x`, ≥ 16 — retrace holds 0–16 open) and its later `write(n, …)` is EBADF (`translate_fds`
has no slot for it). Paste that stdout; `git stash pop`.

- [ ] **Step 10: Green + neighbours.** `cargo test -p retrace --test dupfd_e2e --test dup2_e2e --test dupkind_e2e --test fdtable_e2e --test jq_e2e --test cpython_e2e -- --test-threads=1`
  → pass (jq and CPython issue `fcntl` with the census commands; if either is absent it skips
  loudly — note that in the report).

- [ ] **Step 11: Clippy + commit.** Clippy clean; `verify_thread` count 7.
  Commit: `M38 t2: fcntl/ioctl kinds follow the command; F_DUPFD is a table operation honouring the guest minimum`.

---

### Task 3: `AT_FDCWD` in the form real guests pass

**Files:**
- Modify: `crates/retrace-box/src/lib.rs:3178–3179` (`translate_fds` sentinel)
- Modify: `crates/retrace-box/tests/fdxlat.rs:60–68` (`at_fdcwd_passes_through_untranslated`)
- Modify: `crates/retrace-arch/src/lib.rs:111–113` (`AT_FDCWD` doc), `:172–173` (`ArgKind::Fd` doc)
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs` (fixture wiring)
- Create: `crates/retrace-guest/c/atfdcwd_dyn.c`, `crates/retrace/tests/atfdcwd_e2e.rs`

**Interfaces:**
- Produces: `retrace_guest::ATFDCWD_DYN`. No new API.

- [ ] **Step 1: Failing unit test.** Rewrite `at_fdcwd_passes_through_untranslated` in
  `crates/retrace-box/tests/fdxlat.rs`:

```rust
#[test]
fn at_fdcwd_passes_through_untranslated_in_the_form_the_abi_delivers() {
    // libc passes `int dirfd = -2` in w0, so x0 arrives as 0x0000_0000_ffff_fffe — MEASURED on
    // /bin/ls (twice) and /bin/ed (once) at M33 (Ruling 10). The 64-bit sign-extended form is
    // kept as a second case; it is what an earlier version of this test passed, which is how the
    // test stayed green while every real guest got EBADF.
    let t = FdTable::new();
    for form in [0xffff_fffeu64, AT_FDCWD as u64] {
        let mut args = [0u64; 8];
        args[0] = form;
        assert!(translate(&t, SYS_OPENAT, &mut args).is_ok(),
            "AT_FDCWD as {form:#x} is a sentinel, not a descriptor — it must not be rejected as EBADF");
        assert_eq!(args[0], form, "the sentinel must reach the kernel untouched");
    }
}
```
`cargo test -p retrace-box --test fdxlat -- --test-threads=1` → FAILS on the `0xfffffffe` case
with EBADF. Paste the failure.

- [ ] **Step 2: The fix.** `crates/retrace-box/src/lib.rs:3178–3179`:

```rust
            // AT_FDCWD (-2) and friends are sentinels, not descriptors. `int fd` arrives in w0,
            // so the sentinel is 0xffff_fffe in x0, not the sign-extended form — the low 32 bits
            // are what carry the sign (M33 Ruling 10, fixed M38). A real descriptor never has
            // bit 31 set.
            if (v as i32) < 0 { continue; }
```
Step 1's test → pass. Update `AT_FDCWD`'s doc at `retrace-arch:111` to add "The ABI delivers it
as `0xffff_fffe` in `x0`; `translate_fds` tests the low 32 bits (M38)." and the
`ArgKind::Fd` doc's last sentence (`:172–173`) to "`AT_FDCWD` is negative *in its low 32 bits*
and passes through translation untouched."

- [ ] **Step 3: The fixture.** `crates/retrace-guest/c/atfdcwd_dyn.c`:

```c
// M38. The AT_FDCWD fixture: a relative fstatat through the sentinel succeeds. Before M38 every
// real guest's AT_FDCWD (0xfffffffe in x0 — a 32-bit -2) was looked up as a descriptor and got
// EBADF; /bin/ls printed "ls: .: Bad file descriptor" on both runs and the sweep called it a PASS.
//
// Expected stdout (record == replay, bit for bit):   ok\n
#include <stdio.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <errno.h>

int main(void) {
    struct stat st;
    if (fstatat(AT_FDCWD, ".", &st, 0) == 0) printf("ok\n");
    else printf("errno=%d\n", errno);
    return 0;
}
```
Wire it as before (`atfdcwd_dyn`, `ATFDCWD_DYN`, doc: "M38: `fstatat(AT_FDCWD, \".\")` — prints
`ok` when the sentinel passes through translation.").

- [ ] **Step 4: The gate — red first.** `crates/retrace/tests/atfdcwd_e2e.rs`:

```rust
// M38 gate. AT_FDCWD in the form real guests pass. Asserts BOTH halves on the trace: that the
// guest passed the 32-bit form (0xfffffffe — so this test cannot go green on a fixture that
// happened to sign-extend) and that the call succeeded.
mod util;

#[test]
fn a_relative_fstatat_through_at_fdcwd_succeeds() {
    let out = util::assert_rung_records_and_replays(retrace_guest::ATFDCWD_DYN, &[], b"ok\n");
    let events = retrace_trace::Reader::open(&out.trace).unwrap();
    let seen: Vec<(u64, u64, bool)> = events.iter().filter_map(|e| match e {
        retrace_trace::Event::Syscall { num, args, ret, err, .. } if *num == retrace_arch::SYS_FSTATAT64 =>
            Some((args[0], *ret, *err)),
        _ => None,
    }).collect();
    assert!(!seen.is_empty(), "expected an fstatat64 landmark");
    assert!(seen.iter().any(|(dirfd, _, _)| *dirfd == 0xffff_fffe),
        "the guest must pass AT_FDCWD as the 32-bit form 0xfffffffe (the form the ABI delivers); saw {seen:?}");
    assert!(seen.iter().all(|(dirfd, _, err)| *dirfd != 0xffff_fffe || !*err),
        "fstatat64(AT_FDCWD, ...) must succeed — EBADF here is the M10-t3 sentinel bug: {seen:?}");
}
```
Red run: stash `crates/retrace-box`, `cargo test -p retrace --test atfdcwd_e2e -- --test-threads=1`
→ fails (guest prints `errno=9`); paste; pop.

- [ ] **Step 5: Green + neighbours.** `cargo test -p retrace --test atfdcwd_e2e --test fdtable_e2e --test jq_file_e2e --test sysbin_e2e -- --test-threads=1`
  → pass. Then a hand check of the sweep effect (not a gate — recorded in the task report):
  `cargo run -p retrace -- record-dyn /bin/ls -o /tmp/m38-ls.bin` prints a directory listing
  and NOT `ls: .: Bad file descriptor`.

- [ ] **Step 6: Clippy + commit.** Clippy clean.
  Commit: `M38 t3: AT_FDCWD is honoured in the 32-bit form real guests pass`.

---

### Task 4: `execve`/`posix_spawn` — measured, then refused

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` — constants beside `SYS_FSTATAT64` (`:77`);
  `exec_refusal_errno` beside `is_signal_syscall` (`:1206`); row comments for `59` (`:753`) and
  `244`; `mod tests`
- Modify: `crates/retrace-core/src/lib.rs` — a record arm before the generic BSD arm (`:1140`,
  the `Stop::Syscall { num, args } =>` that opens "Every other syscall goes through the general
  memory-diff engine"); a replay mirror inside the generic arm after `verify_thread` (`:1723`)
- Modify: `crates/retrace/tests/cpython_e2e.rs` (the launcher test, after its last assertion)
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs` (fixture wiring)
- Create: `crates/retrace-guest/c/exec_dyn.c`, `crates/retrace/tests/exec_e2e.rs`

**Interfaces:**
- Produces: `retrace_arch::{SYS_EXECVE, SYS_POSIX_SPAWN}: u64`;
  `retrace_arch::exec_refusal_errno(num: u64) -> Option<u64>` (`Some(errno)` for 59/244, else
  `None` — the predicate and the value in one place); `retrace_guest::EXEC_DYN`.

- [ ] **Step 1: The fixture (it is also the measuring instrument).** `crates/retrace-guest/c/exec_dyn.c`:

```c
// M38. The exec fixture: execve(2) and posix_spawn(2)+POSIX_SPAWN_SETEXEC both RETURN to the
// guest with the refusal errno instead of replacing the image. Before M38 both were forwarded and
// failed only because argv/envp are untranslated guest pointers; a forwarded exec that ever
// stopped EFAULTing would replace retrace's own process. The two errnos are printed so the
// pre-fix run MEASURES what the forward returned (the refusal reproduces it for continuity, spec
// R4) and the post-fix run proves nothing the guest sees changed.
//
// Expected stdout: execve=<E>\nposix_spawn=<E>\n   (E = retrace_arch::exec_refusal_errno)
#include <stdio.h>
#include <unistd.h>
#include <errno.h>
#include <spawn.h>

int main(void) {
    char *argv[] = { "/bin/echo", "should-not-run", NULL };
    char *envp[] = { NULL };
    execve("/bin/echo", argv, envp);
    printf("execve=%d\n", errno);
    posix_spawnattr_t attr;
    posix_spawnattr_init(&attr);
    posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETEXEC);
    pid_t pid = 0;
    int rc = posix_spawn(&pid, "/bin/echo", NULL, &attr, argv, envp);
    printf("posix_spawn=%d\n", rc);
    fflush(stdout);
    return 0;
}
```
Wire it (`exec_dyn`, `EXEC_DYN`, doc: "M38: `execve` then `posix_spawn(SETEXEC)`; prints the
errno each returns — both are refused, never forwarded.").

- [ ] **Step 2: MEASURE on the pre-fix tree.** With nothing else changed yet:
  `cargo build -p retrace-guest && P=$(ls target/aarch64-apple-darwin/debug/build/retrace-guest-*/out/exec_dyn | head -1) && RETRACE_TRACE=1 cargo run -p retrace -- record-dyn "$P" -o /tmp/m38-exec-pre.bin 2>/tmp/m38-exec-pre.err`.
  Record the two lines the guest prints (`execve=<E>` / `posix_spawn=<E>`) and, from
  `grep -a '\[trap\]' /tmp/m38-exec-pre.err | grep -a -w -e ' 59 ' -e ' 244 '` (adjust to the
  `[trap]` line's actual format — look at one first), the `(num, ret, err)` for both. **Expected
  `14` (EFAULT) for both.** Whatever is measured is the constant; if the two differ,
  `exec_refusal_errno` returns each its own value and spec §3d/R4 are corrected in Task 6 Step 3.
  Paste both into the task report — this is the measurement R4 rests on.

- [ ] **Step 3: Failing arch test + the constants.** In `mod tests`:

```rust
    // M38: exec is refused with the errno the FORWARD returned before M38 (measured, Task 4
    // Step 2) — continuity, not fidelity (spec R4). Every other number is None.
    #[test]
    fn exec_refusal_covers_execve_and_posix_spawn_only() {
        assert_eq!(exec_refusal_errno(SYS_EXECVE), Some(14));
        assert_eq!(exec_refusal_errno(SYS_POSIX_SPAWN), Some(14));
        assert_eq!(exec_refusal_errno(SYS_OPEN), None);
        assert_eq!((SYS_EXECVE, SYS_POSIX_SPAWN), (59, 244));
    }
```
(Substitute the measured values if not 14.) Beside `SYS_FSTATAT64`:
```rust
pub const SYS_EXECVE: u64 = 59;
pub const SYS_POSIX_SPAWN: u64 = 244;
```
Beside `is_signal_syscall`:
```rust
/// M38: `execve`(59) and `posix_spawn`(244) are REFUSED, never forwarded. Before M38 both were
/// forwarded and failed only because their `argv`/`envp` are untranslated guest pointers (the
/// host kernel read a guest IPA as a host address and returned EFAULT); if nested-pointer
/// translation ever landed, a forwarded exec would replace retrace's own process. The value is
/// what the forward RETURNED, measured on `exec_dyn` at M38 Task 4 — chosen for continuity (the
/// CPython launcher's output, `/bin/sh`'s sweep row and every existing trace are unchanged), not
/// fidelity; `ENOSYS` (78) is the one-constant change if a successor prefers "exec is unmodelled"
/// to be what the guest reads (spec R4). `Some` doubles as the predicate the record arm and the
/// replay mirror share.
pub fn exec_refusal_errno(num: u64) -> Option<u64> {
    match num { SYS_EXECVE | SYS_POSIX_SPAWN => Some(14), _ => None }
}
```
Row comments for 59 and 244: append "M38: refused, never forwarded (`exec_refusal_errno`); the
row is documentation of the prototype only." `cargo test -p retrace-arch -- --test-threads=1` → pass.

- [ ] **Step 4: Record arm.** In `record_box`, immediately BEFORE the generic BSD arm
  (`Stop::Syscall { num, args } =>` at `:1140`, the one whose first statement asserts
  `!is_signal_syscall`):

```rust
            // M38: exec is refused, never forwarded — placed BEFORE the generic forward arm, which
            // is the only guard (the bsdthread_create precedent). Constant return, no writes, so
            // replay recomputes and byte-compares (symmetry rule 1, the standard posture). The
            // errno is the one the forward produced before M38, for continuity (spec R4).
            Stop::Syscall { num, args } if retrace_arch::exec_refusal_errno(num).is_some() => {
                let e = retrace_arch::exec_refusal_errno(num).unwrap();
                eprintln!("[retrace] refusing {} (syscall {num}): exec-in-place is unmodelled; returning errno {e} without forwarding",
                    if num == retrace_arch::SYS_EXECVE { "execve" } else { "posix_spawn" });
                w.append(&Event::Syscall { num, args, ret: e, ret1: 0, err: true, writes: vec![], thread })
                    .map_err(|e| format!("append exec refusal: {e}"))?; count += 1;
                b.apply_and_return(e, true, &[]);
            }
```

- [ ] **Step 5: Replay mirror.** Inside the generic replay arm, directly after
  `self.verify_thread(*rthread, pc)?;` (`:1723`) and before the `SYS_SIGACTION` mirror:

```rust
                            // M38 mirror of record's exec refusal: recompute the constant, compare.
                            if let Some(e) = retrace_arch::exec_refusal_errno(num) {
                                if *ret != e || !*err || !writes.is_empty() {
                                    return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "exec refusal mismatch: recorded ret {ret} err {err} with {} write(s), \
                                         expected errno {e}, err, no writes", writes.len()) });
                                }
                                self.b.apply_and_return(*ret, *err, writes);
                                return self.finish_event();
                            }
```
`cargo build --workspace` clean; `grep -c 'self.verify_thread(' …` → `7`.

- [ ] **Step 6: The gate — with the continuity control.** `crates/retrace/tests/exec_e2e.rs`:

```rust
// M38 gate. execve/posix_spawn are REFUSED, never forwarded. Two assertions that a forward
// cannot both satisfy: (1) the recorder's stderr carries the refusal line — the only observable
// a forwarded call that happens to EFAULT cannot fake (CLAUDE.md's first gate rule: the errno
// and the empty write set are exactly what the pre-M38 forward produced, so they are the
// CONTINUITY half, asserted second and labelled as such); (2) the trace's events for 59 and 244
// carry the constant and no writes. Repo-owned because the CPython launcher test skips without
// Homebrew Python and so guards nothing on its own.
mod util;

fn expect_stdout() -> Vec<u8> {
    let e = retrace_arch::exec_refusal_errno(retrace_arch::SYS_EXECVE).unwrap();
    let p = retrace_arch::exec_refusal_errno(retrace_arch::SYS_POSIX_SPAWN).unwrap();
    format!("execve={e}\nposix_spawn={p}\n").into_bytes()
}

#[test]
fn exec_is_refused_and_says_so() {
    let (rec, trace) = util::record_dynamic(retrace_guest::EXEC_DYN);
    assert_eq!(rec.code, 0, "the fixture must run to its own exit(0); a replaced image or a crash \
        cannot print anything. stderr:\n{}", rec.stderr);
    // The difference M38 makes.
    assert!(rec.stderr.contains("[retrace] refusing execve") && rec.stderr.contains("[retrace] refusing posix_spawn"),
        "both refusal lines must be on the recorder's stderr; a FORWARDED exec prints none. stderr:\n{}", rec.stderr);
    // Continuity (spec R4): what the guest reads is what the pre-M38 forward returned.
    assert_eq!(rec.stdout, expect_stdout(), "got {:?}", String::from_utf8_lossy(&rec.stdout));
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay: {}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout);
    let events = retrace_trace::Reader::open(&trace).unwrap();
    let mut seen = Vec::new();
    for e in events.iter() {
        if let retrace_trace::Event::Syscall { num, ret, err, writes, .. } = e {
            if let Some(want) = retrace_arch::exec_refusal_errno(*num) {
                assert!(*err && *ret == want && writes.is_empty(), "syscall {num}: ret {ret} err {err} writes {}", writes.len());
                seen.push(*num);
            }
        }
    }
    assert_eq!(seen, vec![retrace_arch::SYS_EXECVE, retrace_arch::SYS_POSIX_SPAWN], "both exec spellings must be landmarks: {seen:?}");
}
```
Red run: stash `crates/retrace-core`, run it — the stdout assertion passes (continuity!) and the
**stderr assertion fails**; paste that: it is the proof the stderr line is the discriminating
half. Pop.

- [ ] **Step 7: The launcher test's added assertion.** In `cpython_e2e.rs`'s launcher test, after
  the replay-equality assertions at the end of the function:

```rust
    // M38: the posix_spawn is REFUSED now, not forwarded — the refusal line is the only thing
    // that distinguishes the two, since the errno the guest reads was chosen to match (spec R4).
    assert!(rec.stderr.contains("[retrace] refusing posix_spawn"),
        "the launcher's posix_spawn must be refused by the M38 arm, not forwarded. stderr:\n{}", rec.stderr);
```
Also rewrite the file's header comment paragraph that says "the forwarded `posix_spawn` returns
an error" to "the `posix_spawn` is refused (M38) with the errno the forward used to return".

- [ ] **Step 8: Green.** `cargo test -p retrace --test exec_e2e --test cpython_e2e --test sysbin_e2e -- --test-threads=1`
  → pass (cpython skips loudly if absent — say so).
  Hand check: `cargo run -p retrace -- record-dyn /bin/sh -o /tmp/m38-sh.bin </dev/null` exits
  as it did before (compare with the M37 sweep's row for `/bin/sh` in
  `docs/sweep-evidence/2026-09-13-m37/README.md`) and its stderr has the refusal line.

- [ ] **Step 9: Clippy + commit.** Clippy clean.
  Commit: `M38 t4: execve/posix_spawn are refused with the measured errno, never forwarded`.

---

### Task 5: The RCV-only message-queue `mach_msg2`, refused deterministically

**Files:**
- Modify: `crates/retrace-core/src/machmsg.rs` — `Route` (`:66–67`), `route()` MQ branch
  (`:106–114`), constants after `MACH_SEND_INVALID_DEST` (`:163`), `mod tests` (after
  `the_refusal_is_send_invalid_dest_and_not_invalid_right`)
- Modify: `crates/retrace-core/src/lib.rs` — record arm beside `RefuseMqSend` (`:521–538`),
  replay mirror beside its twin (`:1895–1910`)
- Modify: `crates/retrace/tests/apple_walls_e2e.rs:35–56` (six `#[ignore]` reasons — rewritten
  or removed)
- Create: `docs/sweep-evidence/2026-09-16-m38/README.md` and the kept `.err` files

**Interfaces:**
- Produces: `machmsg::Route::RefuseMqRecv`; `machmsg::MACH_RCV_TIMED_OUT`,
  `machmsg::MACH_RCV_INVALID_NAME`, `machmsg::MACH_RCV_PORT_DIED`, `machmsg::MACH_RCV_REFUSAL`
  (the chosen one).

- [ ] **Step 1: Failing router tests.** In `machmsg.rs` `mod tests`:

```rust
    /// The RCV-only shape all six parked binaries reach (M36/M37 evidence): MQ_CALL | RCV_TIMEOUT
    /// (0x100) | RCV_MSG (0x2) — a receive with a timeout, no SEND_MSG bit.
    const MQ_RCV: u64 = 0x4_0400_0102;

    #[test]
    fn routes_a_message_queue_receive_to_refusal() {
        // A receive on a queue nothing in the box can send to. Timing out is the faithful answer,
        // not a stub — the box contains no senders, exactly as it contains no receivers.
        assert!(matches!(route(&msg(0, 0x1403, MQ_RCV), Some(0x203)), Route::RefuseMqRecv));
        // Not keyed on rcv_name/dest: nondeterministic port names (as for RefuseMqSend).
        assert!(matches!(route(&msg(0, 0x1103, MQ_RCV), Some(0x203)), Route::RefuseMqRecv));
        // Without the timeout bit it is still a receive.
        assert!(matches!(route(&msg(0, 0x1403, 0x4_0000_0002), Some(0x203)), Route::RefuseMqRecv));
    }

    #[test]
    fn a_one_way_message_queue_send_still_fails_loud() {
        // SEND without RCV: never observed; keeps the fail-loud default, and the string now says
        // what it is (it used to call every non-RPC shape a "send").
        match route(&msg(0, 0x1403, 0x4_0000_0001), Some(0x203)) {
            Route::Unsupported(s) => assert!(s.contains("one-way"), "{s}"),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn the_receive_refusal_is_a_receive_code() {
        // Every candidate is in the MACH_RCV family (0x1000_40xx); the chosen one is asserted so
        // a measurement-driven change to the constant has to change this test too.
        assert_eq!(MACH_RCV_REFUSAL & 0xffff_ff00, 0x1000_4000);
        assert_eq!(MACH_RCV_REFUSAL, MACH_RCV_TIMED_OUT);
    }
```
(`Route` needs `#[derive(Debug)]` if it lacks one — check; add it.) `cargo test -p retrace-core --lib -- --test-threads=1`
→ FAILS to compile.

- [ ] **Step 2: The router.** `Route` gains `RefuseMqRecv` (doc: "a message-queue RECEIVE;
  the box hosts no senders, so it times out — deterministic, no host contact"). The MQ branch:

```rust
    if m.options & MACH64_SEND_MQ_CALL != 0 {
        let sr = m.options & (MACH64_SEND_MSG | MACH64_RCV_MSG);
        if sr == MACH64_SEND_MSG | MACH64_RCV_MSG { return Route::RefuseMqSend; }
        // M38: a RECEIVE on a message queue (all six M37-parked Apple binaries reach it as
        // options 0x4_0400_0102 — RCV_MSG | RCV_TIMEOUT, no SEND_MSG). The box has no senders,
        // so the receive can only time out; `MACH_RCV_REFUSAL` is that answer, chosen by
        // measurement against the six (docs/sweep-evidence/2026-09-16-m38/README.md).
        if sr == MACH64_RCV_MSG { return Route::RefuseMqRecv; }
        return Route::Unsupported(format!(
            "options {:#x}: a message-queue call that is {}", m.options,
            if sr == MACH64_SEND_MSG { "a one-way send (SEND without RCV), never observed" }
            else { "neither a send nor a receive (no SEND_MSG, no RCV_MSG), never observed" }));
    }
```
`Route` gets `#[derive(Debug)]` (it has none today; the tests' `panic!("… got {other:?}")`
needs it). The prose comment above this branch (`:93–105`) keeps its M23/M36/M37 history; add one
sentence: "M38 narrowed the fallthrough again: a receive is `RefuseMqRecv`." Constants after
`MACH_SEND_INVALID_DEST`:
```rust
/// Receive-side codes (osfmk/mach/message.h). `MACH_RCV_REFUSAL` is what `Route::RefuseMqRecv`
/// returns — **chosen by measurement** (M38 Task 5 Step 5): each candidate was built into the
/// recorder and run against the six parked binaries; the table is in the evidence README. Ties
/// go to `MACH_RCV_TIMED_OUT`, because the options word carries `RCV_TIMEOUT` and a queue no one
/// can send to is a queue whose receive times out.
pub const MACH_RCV_TIMED_OUT: u64 = 0x1000_4003;
pub const MACH_RCV_INVALID_NAME: u64 = 0x1000_4002;
pub const MACH_RCV_PORT_DIED: u64 = 0x1000_4006;
pub const MACH_RCV_REFUSAL: u64 = MACH_RCV_TIMED_OUT;
```
The existing test `a_message_queue_send_without_the_rpc_shape_still_fails_loud` (its
`0x4_0000_0000` case has neither SEND nor RCV) still expects `Unsupported(_)` and needs no
change. Step 1 → pass.

- [ ] **Step 3: Record arm.** After the `RefuseMqSend` arm (`:538`):

```rust
                    machmsg::Route::RefuseMqRecv => {
                        // M38. A receive on a message queue. No sender exists in the box, so the
                        // faithful answer is the timeout the options word already allows for.
                        // NEVER forwarded: a real receive would block retrace's own thread on a
                        // queue only a daemon could fill. Writes NOTHING — the receive buffer is
                        // untouched — so both the return and the empty write set are constants
                        // replay recomputes (the RefuseMqSend posture).
                        eprintln!("[retrace] refusing mach_msg2 message-queue receive (rcv_name {:#x} \
                            rcv_size {} options {:#x}): the box hosts no message-queue senders",
                            m.rcv_name, m.rcv_size, m.options);
                        w.append(&Event::Syscall { num, args, ret: machmsg::MACH_RCV_REFUSAL, ret1: 0,
                            err: false, writes: vec![], thread })
                            .map_err(|e| format!("append mach_msg2 mq receive refusal: {e}"))?; count += 1;
                        b.apply_and_return(machmsg::MACH_RCV_REFUSAL, false, &[]);
                    }
```

- [ ] **Step 4: Replay mirror.** After the `RefuseMqSend` mirror (`:1910`):

```rust
                                    machmsg::Route::RefuseMqRecv => {
                                        // M38: standard symmetric posture, as RefuseMqSend.
                                        if *ret != machmsg::MACH_RCV_REFUSAL || *err || !writes.is_empty() {
                                            return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                                "mach_msg2 message-queue receive refusal mismatch: recorded \
                                                 ret {ret:#x} err {err} with {} write(s)", writes.len()) });
                                        }
                                        self.b.apply_and_return(*ret, *err, writes);
                                    }
```
`cargo build --workspace`; `cargo test -p retrace-core -- --test-threads=1` → pass;
`cargo test -p retrace --test xpc_e2e --test sysbin_e2e -- --test-threads=1` → pass (the
SEND|RCV refusal is untouched).

- [ ] **Step 5: MEASURE the code.** Build the signed CLI once per candidate and run the six.
  Script (run from the worktree; `nohup … &` and poll — six binaries × three codes):

```sh
K=docs/sweep-evidence/2026-09-16-m38; mkdir -p "$K"
BINS="/bin/launchctl /usr/bin/automationmodetool /usr/bin/desdp /usr/bin/dyld_info /usr/bin/flex /usr/bin/dddiagnose"
for code in MACH_RCV_TIMED_OUT MACH_RCV_INVALID_NAME MACH_RCV_PORT_DIED; do
  sed -i '' "s/^pub const MACH_RCV_REFUSAL: u64 = .*/pub const MACH_RCV_REFUSAL: u64 = $code;/" crates/retrace-core/src/machmsg.rs
  cargo build -p retrace 2>&1 | tail -1
  cp target/aarch64-apple-darwin/debug/retrace /tmp/retrace-$code
  codesign -s - -f --entitlements retrace.entitlements /tmp/retrace-$code
  for b in $BINS; do n=$(basename $b)
    perl -e 'alarm shift; exec @ARGV' 180 /tmp/retrace-$code record-dyn $b -o /tmp/m38-$n-$code.bin >"$K/$n.$code.rec.out" 2>"$K/$n.$code.rec.err" </dev/null; rc=$?
    perl -e 'alarm shift; exec @ARGV' 180 /tmp/retrace-$code replay /tmp/m38-$n-$code.bin >"$K/$n.$code.rp.out" 2>"$K/$n.$code.rp.err" </dev/null; rp=$?
    refusals=$(grep -ac 'refusing mach_msg2 message-queue receive' "$K/$n.$code.rec.err")
    wall=$(grep -a 'RECORD ERROR\|panicked at\|DIVERGENCE' "$K/$n.$code.rec.err" "$K/$n.$code.rp.err" | head -1 | cut -c1-160)
    printf 'ROW\t%s\t%s\trc=%s\trp=%s\treceive-refusals=%s\tstdout-equal=%s\t%s\n' "$n" "$code" "$rc" "$rp" "$refusals" "$(cmp -s $K/$n.$code.rec.out $K/$n.$code.rp.out && echo y || echo n)" "$wall"
  done
done | tee "$K/measure.tsv"
git checkout crates/retrace-core/src/machmsg.rs   # restore the default before choosing
```
Classify each cell: **proceeds** (record rc 0 or the guest's own exit, no `RECORD ERROR`,
receive-refusals small — ≤ 3), **retry-loops** (receive-refusals ≥ 10, or the 180 s alarm
fired: rc 142), **new wall** (a `RECORD ERROR`/panic at a *different* landmark than the RCV
call), **brk** (record rc 139 / `identical fault`). Choose the code with the most binaries in
"proceeds" or "new wall" (both mean the refusal was accepted); ties → `MACH_RCV_TIMED_OUT`. Set
`MACH_RCV_REFUSAL` to the winner (and the `the_receive_refusal_is_a_receive_code` assertion if
it is not `TIMED_OUT`). Write `$K/README.md`: the table (18 cells), the ruling (spec R3), the
recorder pid regime (one, N — spec R6, with `recpid` if the harness printed it), and for each
binary the landmark it now stops at with the line quoted.

- [ ] **Step 6: The gates.** Rebuild with the winner. Run
  `cargo test -p retrace --test apple_walls_e2e -- --test-threads=1 --ignored 2>&1 | tee $K/gates.log`
  (each test prints the wall by name on failure). For each of the six:
  - **passes** → delete its `#[ignore = …]` line and prepend to its doc comment "M38: un-parked
    — the RCV-only message-queue call is refused (`MACH_RCV_REFUSAL`), and the binary records to
    a clean exit and replays bit-for-bit (evidence `$K/<bin>.<CODE>.{rec,rp}.err`)."
  - **fails at a new wall** → rewrite its reason in the M37 shape: `M38 wall, class <C or the
    measured class>, parked, not routed. <path>: the RCV-only message-queue call is refused (M38
    t5, MACH_RCV_REFUSAL = <code>); the row now stops N landmarks later at \`<the RECORD ERROR /
    panic line verbatim>\`, rc/rp <rc>/<rp>, landmark <n>, run N (recpid <p>; one regime — §4b is
    retired, M37). Evidence docs/sweep-evidence/2026-09-16-m38/<bin>.<CODE>.{rec,rp}.err.
    UN-IGNORE when <what the new wall needs>.`
  - **record clean, replay diverges** → **HALT** (class E, spec §5). Write the report and stop.
  A binary that retry-loops under every code is "fails at a new wall" with the wall being the
  loop itself; say so in the reason and in the README.
  Copy the winner's `.err` files to the evidence dir if the script did not already.

- [ ] **Step 7: Sanity — nothing else moved.** `cargo test -p retrace --test apple_walls_e2e --test xpc_e2e --test sysbin_e2e --test hello_dyn_e2e -- --test-threads=1`
  (non-ignored only) → pass; `csh`/`tcsh` reasons are untouched here (Task 6 refreshes their
  quoted `pipe` landmark).

- [ ] **Step 8: Clippy + commit.** Clippy clean; `verify_thread` count 7.
  Commit: `M38 t5: the RCV-only message-queue mach_msg2 is refused (<code>); <k> of six gates un-parked, <6-k> re-parked at their next wall`.

---

### Task 6: Close — sweep re-baseline, docs, gate, merge, push

**Files:**
- Modify: `README.md` — "What works today" (`:95–447`: the Apple-binaries paragraph at `:122`,
  the descriptor sentences at `:300`, `:318`, the gate paragraph `:381–415`), "Known limits"
  (`:448–1048`: the RCV entry `:502`, the `ls` note `:555–556`, the `pipe`/exec entry `:779–785`,
  the launcher entry `:850`, the descriptor entry `:873–887`, the parked-gates bullet `:957`)
- Modify: `docs/status-log.md` — append `## Status: M38-owed — …`; add ONE forward-pointer
  sentence at the end of the existing "### M38 does not exist" paragraph (an append inside an
  old section is the log's documented exception for forward pointers; change no other old line)
- Modify: `docs/superpowers/specs/2026-09-16-retrace-m38-owed-design.md` (§10 only, plus the
  three corrections named in Step 3)
- Modify: `CLAUDE.md` — the `TRACE_MAGIC` value (two mentions of `RT\x00\x09`), the e2e gate
  list in "Commands" (add `pipe_e2e`, `dupfd_e2e`, `atfdcwd_e2e`, `exec_e2e` in one sentence)
- Modify: `crates/retrace/tests/apple_walls_e2e.rs:28–33` (`csh`/`tcsh` reasons: refresh the
  quoted `pipe` landmark only)
- Create: `.superpowers/sdd/2026-09-16-retrace-m38-owed/gate.sh` (copy of the M37 one with
  `W=…/m38-owed` and `L=…/2026-09-16-retrace-m38-owed/gate`), `task-6-numbers.md`

**Interfaces:**
- Consumes: Tasks 1–5 reports, `docs/sweep-evidence/2026-09-16-m38/README.md`.

- [ ] **Step 1: The sweep re-baseline.** From the worktree, `cargo build -p retrace`, then
  `RETRACE_SWEEP_KEEP=docs/sweep-evidence/2026-09-16-m38/sweep nohup tools/apple-sweep.sh > docs/sweep-evidence/2026-09-16-m38/sweep.log 2>&1 &`
  and poll (`until grep -q '^TALLY' …; do sleep 30; done`, bounded). Then diff the `ROW` lines
  against `docs/sweep-evidence/2026-09-13-m37/README.md`'s table by binary name: every row whose
  label or `rec_reason` changed is listed in the evidence README with the item that moved it
  (`AT_FDCWD` for `/bin/ls`/`/bin/ed`'s output; the exec refusal for `/bin/sh` — label
  unchanged, stderr gains the line; the RCV refusal for the six; `pipe` for `csh`/`tcsh`'s
  landmark shift). **A row that moved for a reason this plan does not name is a finding**:
  ledger it and decide (spec §5 — scope the spec lacks → halt; a class-E row → halt). PASS
  count ≥ 45. Copy the `csh`/`tcsh` `.err` files in and refresh their `#[ignore]` reasons' quoted
  `pipe` landmark (the reason quotes `dup2` landmarks — leave those; add "pipe's pair is bound
  since M38 t1, so the `fcntl(F_SETFD)` on each end now succeeds (landmarks #…)").

- [ ] **Step 2: README.** Edit in place. `:122` and `:957`: the sweep count and the six rows'
  state (un-parked / re-parked at `<wall>`); `:300`/`:318`: `FdPair` is modelled, exec is
  refused; the gate paragraph: the numbers from Step 6 (both figures and the `#[test]` census
  sentence); Known limits `:502`: the RCV entry becomes "refused deterministically (M38); modelling
  it is still class C"; `:555–556`: delete the `ls` note (it is history now — the descriptor entry
  keeps one sentence: "`AT_FDCWD` is honoured in the 32-bit form since M38"); `:779–785`: the
  `pipe` entry is retired and the exec entry says refused-with-measured-errno; `:850`: the
  launcher paragraph says refused; `:873–887`: `F_DUPFD` modelled, `pipe` modelled, the
  `AT_FDCWD` paragraph reduced to its one sentence; add to Known limits, under the descriptor
  entry: "`x1` is written only for `pipe` (narrow capture, M38 R2); every other syscall leaves the
  guest's `x1` stale where xnu would write `retval[1]`." Greps that must return nothing:
  `Ret::FdPair. is documentation`, `Bad file descriptor` (except as history in a status-log
  citation), `RT\\x00\\x09` (outside history), `unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102`
  (outside history), `F_DUPFD.*unmodelled`.

- [ ] **Step 3: Spec §10.** Outcome vs §9 (numbers), the measured exec errnos, the RCV code
  table's ruling, which of the six moved, the sweep diff, and this spec's own corrections. Two
  were already applied at plan-writing time and are only *recorded* here as history: §3a's
  `apply_and_return_pair` (never existed — the `x1` write is `Box_::set_ret1`, gated by
  `returns_fd_pair` on both sides) and §3d's "eighth `verify_thread` site" (none; the mirrors
  sit inside the generic arms, count seven). One is decided by measurement: §3d/R4 — if the two
  exec errnos measured differently, say so and name both.

- [ ] **Step 4: Status log.** Append `## Status: M38-owed — five owed items closed: pipe's pair,
  F_DUPFD, AT_FDCWD, and two forwards refused` in M37's shape: the charter note (fresh, reuses
  the number; R1); each item — what was owed, what was measured before the change (the pre-fix
  red runs pasted from the task reports, the exec errnos, the RCV code table), what was built,
  its control; the six gates' outcomes with the reasons; the sweep diff, every moved row
  explained; rulings R1–R6 plus every ledger `Ruling:` from the tasks; the gate + reconciliation;
  **What stays owed** — M37's list minus the five discharged (struck by name, as M37 did), plus
  anything a task added (uniform `x1`; a fail-loud default for unlisted commands; modelling the
  RCV call; the new walls the six re-parked at, by name). Then, at the end of the existing
  "### M38 does not exist" paragraph, append the one sentence: *"Forward pointer (2026-09-16):
  an M38 does exist after all, under a fresh charter, not this one's — see `## Status: M38-owed`
  below."*

- [ ] **Step 5: CLAUDE.md.** The two `RT\x00\x09` mentions → `RT\x00\x0a` with "(moved again by
  M38 for `ret1`)"; the gate-list sentence gains the four new e2e names with one clause each;
  the `verify_thread` paragraph is unchanged (still seven — say nothing). Do not add a third
  copy of README/status-log content.

- [ ] **Step 6: Commit** `M38 t6: README, status-log section, spec outcome, sweep re-baseline`.

- [ ] **Step 7 (controller): the gate.** Copy the M37 `gate.sh` to
  `.superpowers/sdd/2026-09-16-retrace-m38-owed/gate.sh` with `W` and `L` changed; run it with
  `nohup … &` and poll `GATE DONE`. Prediction from source, reconciled file by file against M37's
  596/0/10 over 131, per file: `retrace-trace` +0 (a rename and a loop inside existing tests);
  `retrace-arch` +3 (`fcntl_and_ioctl_third_argument_kind_follows_the_command`,
  `f_dupfd_is_recognised_by_number_and_command`, `exec_refusal_covers_execve_and_posix_spawn_only`;
  `pipe_return_is_a_pair_and_both_are_bound` is a rewrite, +0); `retrace-core` lib +3 (the
  three router tests); `retrace-box` `fdtable.rs` +2, `fdxlat.rs` +1 (`fcntl_translates_only_its_descriptor`;
  the sentinel test is a rewrite, +0); `retrace` e2e: `pipe_e2e` +3, `dupfd_e2e` +2,
  `atfdcwd_e2e` +1, `exec_e2e` +1 (four new binaries); `apple_walls_e2e` −k ignored for the k
  un-parked. That is +16 tests: **612 + k passed / 0 failed / 10 − k ignored over 135 binaries**
  — reconcile with `grep -c '#\[test\]'` per file diffed against `a663051`, not by trusting this
  sum. Every chunk's `.exit` is `0`; `clippy.exit` is `0`; the `bins` chunk ran
  (11 tests in `debug.rs`). Write `task-6-numbers.md`.

- [ ] **Step 8 (controller): merge.** From the repo root: `git merge --no-ff m38-owed -m "Merge M38-owed: pipe's pair, F_DUPFD, AT_FDCWD, and two forwards refused"`
  (with the attribution line), then `git worktree remove .claude/worktrees/m38-owed` and
  `git branch -d m38-owed`.

- [ ] **Step 9 (controller): push.** `git push origin main` — the one push this milestone makes
  (spec §5). Then the memory file `retrace-m38-owed.md` + its `MEMORY.md` line.

---

## Self-Review

**Spec coverage.** §3a → Task 1 (Steps 1–12: field + bump, view, `host_svc`, `set_ret1`,
`bind_returned_pair`, 4-tuple, record arm, mirror, fixture, gate, tamper test). §3b → Task 2
(Steps 1–2 `shape_of` + constants + R5 default; 3–4 `dup_from`; 5 `guest_fcntl_dupfd` + both
shape consultations; 6 mirror; 7 translation test; 8–9 fixture + gate). §3c → Task 3 (Steps 1–4).
§3d → Task 4 (Step 2 measurement; 3 constants; 4 arm before the generic forward; 5 mirror; 6 the
stderr-line gate with the continuity half; 7 launcher assertion). §3e → Task 5 (Steps 1–2 router
+ tests; 3–4 arms; 5 the code by measurement; 6 gates moved). §3f → each task's mirror step +
the `verify_thread` grep in every commit step. §4 order → task order. §5 envelope → Global
Constraints + Task 6 Steps 8–9. §6 acceptance → Task 6 Steps 1, 7. §7 → nothing in this plan
models `fork`, the RCV call, uniform `x1`, a loud command default, or nested translation. §8
rulings → R1 Task 6 Step 4; R2 Task 1 Steps 6/8/9; R3 Task 5 Step 5; R4 Task 4 Steps 2–3; R5
Task 2 Step 2; R6 Task 5 Steps 5–6. §9 → Task 6 Step 7. §10 → Task 6 Step 3.

**Placeholder scan.** Every code step carries its code; the two "the compiler lists them"
instructions (Task 1 Steps 7–9) name the error codes and the expected count; Task 5 Step 6's
reason template is filled from the measurement script's outputs, all named. Task 4 Step 2's
`cargo run` path expression is awkward — the executor may equally read the path from
`retrace_guest::EXEC_DYN` via `cargo test … exec_e2e` output; both are stated.

**Type consistency.** `forward_and_diff -> (u64, u64, bool, Vec<Region>)` (Task 1 Step 7) is
what Task 2's `guest_fcntl_dupfd` returns and what Task 4's arm does not call. `ret1: 0` is on
every event Tasks 4 and 5 append. `FdTable::dup_from(src: u64, min: u64) -> Result<u64, u64>` is
identical in Task 2 Steps 3, 4, 5, 6. `retrace_arch::exec_refusal_errno(u64) -> Option<u64>` is
used identically in Task 4 Steps 3–6. `machmsg::MACH_RCV_REFUSAL` in Task 5 Steps 1–5.
`returns_fd_pair` in Task 1 Steps 3, 7, 8, 9, 11. `shape_of(num, &args)` takes `&[u64; 8]`
everywhere (Task 2 Steps 1, 2, 5).
