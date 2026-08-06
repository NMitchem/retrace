# M11-signals Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a signal the guest raises on itself a **recorded, replayable terminal event** instead of
a host signal that kills the recorder — and stop every signal syscall from reaching retrace's own
process.

**Architecture:** A box-owned `SigTable` holds the guest's per-signal disposition, blocked mask, and
alt stack. It is a pure function of the guest's own calls, so it exists identically on record and
replay and needs no trace bytes — the `FdTable::slots` posture. `sigaction`/`sigprocmask`/
`sigaltstack`/`sigpending` are serviced against it and **never forwarded**. A raise (`kill`,
`__pthread_kill`) consults it: `Ign` continues, `Handler` asserts (delivery is M12), and `Dfl` on a
fatal signal appends `Event::Signal` plus a final snapshot and ends the recording — the same terminal
shape M6 gave a fault.

**Tech Stack:** Rust 1.95.0 (pinned), `aarch64-apple-darwin`, Hypervisor.framework, `just` for the gate.

## Global Constraints

Copied from `CLAUDE.md` and the spec. Every task's requirements implicitly include this section.

- **macOS 26.x on Apple Silicon required.** Non-root; SIP may stay enabled.
- **`--test-threads=1` is mandatory.** HVF allows one VM per process. A bare `cargo test` flakes with
  `HV_BUSY`. `just gate` sets it.
- **`just gate` is THE exit gate:** `cargo test --workspace` + `cargo clippy -D warnings`. It must end
  green with **zero `#[ignore]`** — current baseline **212 passed / 0 failed / 0 ignored** (79 test
  binaries).
- **Codesigning:** any test that spawns `CARGO_BIN_EXE_retrace` itself must sign it first — use
  `util::bin()` (`crates/retrace/tests/util/mod.rs:12`). Never hand-roll this.
- **W^X:** executing a writable guest page hangs the vCPU. Code pages are RO+exec, data RW+non-exec.
- **SPTM / anon-only memory:** a file-backed `hv_vm_map` hard-panics macOS 26.
- **Drop order:** `Box_`'s `vcpu` field must stay declared before `vm`. Do not reorder struct fields.
- **Never reimplement Apple's PAC.**
- **`clippy.toml` bans `Instant::now`/`SystemTime::now`/`std::thread`.** Load-bearing, not style.
- **Symmetry rule 1:** a special case in record's `match stop` needs a mirror in replay's dispatch, and
  both must recompute identical bytes. **Rule 2:** deterministic emulation belongs below the trace in
  `Box_::run()`.
- **Honest-gate discipline:** a new wall gets a NEW parked gate, never a regression of an existing one.
- **Resolve syscall numbers from `$(xcrun --show-sdk-path)/usr/include/sys/syscall.h`, never memory.**

---

## File Structure

| File | Responsibility |
|------|----------------|
| `crates/retrace-arch/src/lib.rs` (modify) | Signal syscall constants, `NSIG`/`SIG_DFL`/`SIG_IGN`/`SIG_BLOCK`…, and the pure `default_action(sig)` classifier. Zero-dependency crate; no state. |
| `crates/retrace-box/src/sig.rs` (create) | `Disposition`, `SigAction`, `SigTable`, and the two struct codecs. Pure — no VM, no `Box_`. |
| `crates/retrace-box/src/lib.rs` (modify) | `mod sig;` + re-export; `Box_::sigtable` field; `BoxState` carriage + `from_checkpoint` restore. |
| `crates/retrace-trace/src/lib.rs` (modify) | `Event::Signal { sig, pc }`; `TRACE_MAGIC` → `0x0005`. |
| `crates/retrace-core/src/lib.rs` (modify) | `Outcome::Signal { sig }`; record arms + asserts; replay mirrors. |
| `crates/retrace/src/main.rs`, `debug.rs` (modify) | `Outcome::Signal` presentation; exit `128 + sig`. |
| `crates/retrace-guest/asm/raise.s` (create) | `getpid` then `kill(self, SIGABRT)` — the terminal mechanism. |
| `crates/retrace-guest/asm/sigign.s` (create) | `SIG_IGN` then raise then `write("ok\n")` then `exit(0)` — the non-terminal branch. |
| `crates/retrace-guest/asm/killother.s` (create) | `kill(1, SIGKILL)` — the safety boundary. |
| `crates/retrace-guest/rs/panicky.rs` (create) | A real Rust guest that `panic!()`s — the headline. |
| `crates/retrace-guest/build.rs`, `src/lib.rs` (modify) | Compile + export `RAISE`, `SIGIGN`, `KILLOTHER`, `PANICKY`. |
| `crates/retrace/tests/sigraise_e2e.rs` (create) | Terminal mechanism gate. |
| `crates/retrace/tests/sigign_e2e.rs` (create) | Non-terminal branch gate. |
| `crates/retrace/tests/killother_e2e.rs` (create) | Safety-boundary gate. |
| `crates/retrace/tests/panic_e2e.rs` (create) | The headline gate (green or honestly parked). |
| `README.md`, `CLAUDE.md` (modify) | Status section and the honest close. |

**Task order is dependency order.** Tasks 1–2 are pure and VM-free. Task 3 changes the trace format
but emits nothing. Task 4 is the single atomic record-side integration. Task 5 is its replay mirror —
until it lands, no signal trace can replay, which is why no e2e appears before Task 7.

**Exit-code convention, established by M6 and reused here:** a crash exits `139` = `128 + SIGSEGV(11)`
(`crates/retrace/src/main.rs:23`). `Outcome::Signal { sig }` therefore exits `128 + sig`, so a guest
`SIGABRT` exits **134**. This is not a new convention; it is the one already in the file.

---

### Task 1: Signal facts in `retrace-arch`

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (constants near line 65; classifier near `is_console_close`
  at line 37; tests in the existing `#[cfg(test)] mod tests` around line 190)

**Interfaces:**
- Consumes: nothing (zero-dependency crate).
- Produces:
  - `pub const SYS_KILL: u64 = 37;` `SYS_SIGACTION = 46`, `SYS_SIGPROCMASK = 48`,
    `SYS_SIGPENDING = 52`, `SYS_SIGALTSTACK = 53`, `SYS_SIGSUSPEND = 111`, `SYS_SIGRETURN = 184`,
    `SYS_PTHREAD_KILL = 328`, `SYS_PTHREAD_SIGMASK = 329`, `SYS_SIGWAIT = 330`,
    `SYS_TERMINATE_WITH_PAYLOAD = 520`, `SYS_ABORT_WITH_PAYLOAD = 521`, `SYS_GETPID = 20`
  - `pub const NSIG: usize = 32;` `SIGABRT: u64 = 6`, `SIG_DFL: u64 = 0`, `SIG_IGN: u64 = 1`,
    `SIG_BLOCK: u64 = 1`, `SIG_UNBLOCK: u64 = 2`, `SIG_SETMASK: u64 = 3`
  - `pub enum DefaultAction { Terminate, Ignore }`
  - `pub fn default_action(sig: u64) -> DefaultAction`
  - `pub fn is_signal_syscall(num: u64) -> bool` — true for every number M11 intercepts, used by
    Task 4 to prove the correctness invariant in one place.

**Background the implementer needs.** The spec lists five unmeasured facts and says measurement comes
before code. Step 0 is that measurement. Do not skip it: the M10 milestone's spec asserted the guest
would see "fd 3" and the real answer was fd 4, and the M9 milestone missed six syscalls by reading a
truncated histogram.

- [ ] **Step 0: Measure the real signal surface (spec §"Unmeasured", R1/R3 mitigation)**

```bash
cd /Users/noahmitchem/Documents/GitHub/retrace

# The C and Rust guests are build.rs artifacts in OUT_DIR, not repo files. Build the workspace
# first so they exist, then locate them.
cargo build --workspace
HELLO_DYN=$(find target -name hello_dyn -type f | head -1)
HELLO_RUST=$(find target -name hello_rust -type f | head -1)
test -n "$HELLO_DYN" && test -n "$HELLO_RUST" || { echo "guests not built"; exit 1; }

RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn "$HELLO_DYN" \
  -o /tmp/m11-hello.bin  > /tmp/m11-hello.trace 2>&1
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn "$HELLO_RUST" \
  -o /tmp/m11-rust.bin   > /tmp/m11-rust.trace 2>&1
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn /opt/homebrew/bin/jq \
  -o /tmp/m11-jq.bin -- '.name' crates/retrace/tests/fixtures/rung3.json \
  > /tmp/m11-jq.trace 2>&1
```

Then, for each trace, produce the **FULL** histogram — no `head`, no truncation, because the tail is
where the count-1 syscalls live:

```bash
for f in /tmp/m11-*.trace; do
  echo "=== $f ==="
  grep -ao 'num=[0-9-]*' "$f" | sort | uniq -c | sort -rn
done
```

Record the answers to these five questions in the commit message:

1. **Which of 37/46/48/52/53/111/184/328/329/330/520/521 appear, and how often?** Any that appears is
   evidence; any that does not is what justifies its assert.
2. **Does `getpid`(20) appear, and does the guest's returned pid equal retrace's own?** Check with:
   ```bash
   grep -a 'num=20' /tmp/m11-jq.trace | head -3   # then compare to the recorder's pid
   ```
   The `kill` self-check in Task 4 depends on this being true.
3. **What is `__pthread_kill`'s x0 (thread port) if it appears?** `grep -a 'num=328' /tmp/m11-*.trace`
4. **If any guest aborts, is it via 328 or 521?** This decides risk R2.
5. **Does any guest call `sigaction`(46) with a non-`SIG_DFL`/`SIG_IGN` x1?** This decides risk R3 —
   whether Task 4's handler assert immediately walls an existing green test.

**If measurement contradicts the spec, amend the spec before writing code.** A row moving from
"assert" to "serviced" is a scope change to be decided in the open, not worked around during
implementation.

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` in `crates/retrace-arch/src/lib.rs`:

```rust
#[test]
fn signal_syscall_numbers_match_the_sdk() {
    // Resolved from $(xcrun --show-sdk-path)/usr/include/sys/syscall.h on 2026-08-05.
    assert_eq!((SYS_GETPID, SYS_KILL, SYS_SIGACTION, SYS_SIGPROCMASK), (20, 37, 46, 48));
    assert_eq!((SYS_SIGPENDING, SYS_SIGALTSTACK, SYS_SIGSUSPEND, SYS_SIGRETURN), (52, 53, 111, 184));
    assert_eq!((SYS_PTHREAD_KILL, SYS_PTHREAD_SIGMASK, SYS_SIGWAIT), (328, 329, 330));
    assert_eq!((SYS_TERMINATE_WITH_PAYLOAD, SYS_ABORT_WITH_PAYLOAD), (520, 521));
}

#[test]
fn default_action_classifies_the_three_ignored_signals() {
    // SIGCHLD=20, SIGURG=16, SIGWINCH=28 default to ignore; everything else terminates.
    assert_eq!(default_action(20), DefaultAction::Ignore);
    assert_eq!(default_action(16), DefaultAction::Ignore);
    assert_eq!(default_action(28), DefaultAction::Ignore);
    assert_eq!(default_action(SIGABRT), DefaultAction::Terminate);
    assert_eq!(default_action(9), DefaultAction::Terminate);   // SIGKILL
    assert_eq!(default_action(11), DefaultAction::Terminate);  // SIGSEGV
}

#[test]
fn is_signal_syscall_covers_every_intercepted_number_and_nothing_else() {
    for n in [37u64, 46, 48, 52, 53, 111, 184, 328, 329, 330, 520, 521] {
        assert!(is_signal_syscall(n), "{n} must be intercepted");
    }
    // getpid is NOT intercepted — it keeps forwarding, and Task 4's self-check relies on that.
    for n in [20u64, 1, 3, 4, 5, 6, 197, 333] {
        assert!(!is_signal_syscall(n), "{n} must keep forwarding");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p retrace-arch -- --test-threads=1`
Expected: FAIL — `cannot find value SYS_KILL in this scope` and similar.

- [ ] **Step 3: Write minimal implementation**

Add to `crates/retrace-arch/src/lib.rs`:

```rust
// ---- M11-signals -------------------------------------------------------------------------
// Numbers resolved from $(xcrun --show-sdk-path)/usr/include/sys/syscall.h, never from memory.
// The `_nocancel` pairing rule (M10) was checked: the only _nocancel signal syscalls are
// sigsuspend_nocancel(410) and __sigwait_nocancel(422), and both pair with calls M11 asserts on
// anyway — so no SERVICED call here has a silent-fallthrough twin.
pub const SYS_GETPID: u64 = 20;
pub const SYS_KILL: u64 = 37;
pub const SYS_SIGACTION: u64 = 46;
pub const SYS_SIGPROCMASK: u64 = 48;
pub const SYS_SIGPENDING: u64 = 52;
pub const SYS_SIGALTSTACK: u64 = 53;
pub const SYS_SIGSUSPEND: u64 = 111;
pub const SYS_SIGRETURN: u64 = 184;
pub const SYS_PTHREAD_KILL: u64 = 328;
pub const SYS_PTHREAD_SIGMASK: u64 = 329;
pub const SYS_SIGWAIT: u64 = 330;
pub const SYS_TERMINATE_WITH_PAYLOAD: u64 = 520;
pub const SYS_ABORT_WITH_PAYLOAD: u64 = 521;

/// `NSIG` from `sys/signal.h:76` — "counting 0; could be 33 (mask is 1-32)". Signal numbers run
/// 1..=31 in the table; index 0 is unused so indexing mirrors signal numbering.
pub const NSIG: usize = 32;
pub const SIGABRT: u64 = 6;
pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;
pub const SIG_BLOCK: u64 = 1;
pub const SIG_UNBLOCK: u64 = 2;
pub const SIG_SETMASK: u64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultAction { Terminate, Ignore }

/// The kernel's default disposition for `sig` when the guest has installed nothing.
///
/// An arch fact, not policy — which is why it lives here beside `ec_of` rather than in the box.
/// Record's raise arm and replay's mirror both consult THIS function, and that shared call is what
/// keeps them from drifting (symmetry rule 1).
pub fn default_action(sig: u64) -> DefaultAction {
    match sig {
        16 | 20 | 28 => DefaultAction::Ignore,  // SIGURG, SIGCHLD, SIGWINCH
        _ => DefaultAction::Terminate,
    }
}

/// Every syscall M11 intercepts — serviced against the guest's `SigTable` or asserted, but in no
/// case forwarded. This is the single place the correctness invariant ("no signal syscall is ever
/// issued in retrace's process") is expressed, so Task 4 can assert it rather than restate it.
pub fn is_signal_syscall(num: u64) -> bool {
    matches!(num,
        SYS_KILL | SYS_SIGACTION | SYS_SIGPROCMASK | SYS_SIGPENDING | SYS_SIGALTSTACK
        | SYS_SIGSUSPEND | SYS_SIGRETURN | SYS_PTHREAD_KILL | SYS_PTHREAD_SIGMASK
        | SYS_SIGWAIT | SYS_TERMINATE_WITH_PAYLOAD | SYS_ABORT_WITH_PAYLOAD)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p retrace-arch -- --test-threads=1`
Expected: PASS (3 new tests).

Run: `cargo clippy -p retrace-arch --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-arch/src/lib.rs
git commit -m "M11 t1: signal syscall numbers, NSIG, and default_action in retrace-arch

Measured surface (RETRACE_TRACE=1, full histogram, no truncation):
<paste the five Step 0 answers here — they are the evidence every assert
in Task 4 rests on>"
```

---

### Task 2: `SigTable` — the guest's dispositions, pure and VM-free

**Files:**
- Create: `crates/retrace-box/src/sig.rs`
- Modify: `crates/retrace-box/src/lib.rs` (add `mod sig;` + `pub use` near the other module
  declarations at the top of the file)

**Interfaces:**
- Consumes: `retrace_arch::{NSIG, SIG_DFL, SIG_IGN, SIG_BLOCK, SIG_UNBLOCK, SIG_SETMASK}`.
- Produces:
  - `pub enum Disposition { Dfl, Ign, Handler(u64) }` (`Copy`, `PartialEq`)
  - `pub struct SigAction { pub disp: Disposition, pub mask: u32, pub flags: u32 }` (`Copy`)
  - `pub struct SigTable` with:
    - `pub fn action(&self, sig: u64) -> SigAction`
    - `pub fn set_action(&mut self, sig: u64, a: SigAction) -> SigAction` (returns the OLD one)
    - `pub fn is_blocked(&self, sig: u64) -> bool`
    - `pub fn mask(&self) -> u32`
    - `pub fn set_mask(&mut self, how: u64, set: u32) -> u32` (returns the OLD mask)
    - `pub fn altstack(&self) -> Option<(u64, u64, u64)>`
    - `pub fn set_altstack(&mut self, ss: Option<(u64, u64, u64)>) -> Option<(u64, u64, u64)>`
  - `pub fn decode_act(bytes: &[u8]) -> SigAction` — the 24-byte `struct __sigaction`
  - `pub fn encode_oldact(a: SigAction) -> [u8; 16]` — the 16-byte `struct sigaction`

**Background the implementer needs — read this before writing the codecs.** `sigaction(2)`'s input
and output are *different C structs*, verified from `sys/signal.h:277` and `:287` on 2026-08-05:

```c
struct __sigaction {                 /* the ACT argument — 24 bytes */
        union __sigaction_u __sigaction_u;   /* offset 0,  8 bytes */
        void  (*sa_tramp)(...);              /* offset 8,  8 bytes  <-- ONLY here */
        sigset_t sa_mask;                    /* offset 16, 4 bytes  (sigset_t = __uint32_t) */
        int      sa_flags;                   /* offset 20, 4 bytes */
};
struct sigaction {                   /* the OLDACT writeback — 16 bytes */
        union __sigaction_u __sigaction_u;   /* offset 0,  8 bytes */
        sigset_t sa_mask;                    /* offset 8,  4 bytes */
        int      sa_flags;                   /* offset 12, 4 bytes */
};
```

Writing 24 bytes where the guest expects 16 corrupts it 8 bytes past the struct, and the damage
surfaces much later as something unrelated. That is why `encode_oldact` returns a fixed `[u8; 16]`
rather than a `Vec`, and why the golden test below pins every offset.

`sa_tramp` is decoded and **discarded**: it points at libc's signal trampoline, which only matters
when a handler is actually delivered, and M11 delivers nothing. Task 4's raise arm asserts before any
trampoline could be used.

- [ ] **Step 1: Write the failing tests**

Create `crates/retrace-box/src/sig.rs` containing ONLY this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use retrace_arch::{SIG_BLOCK, SIG_UNBLOCK, SIG_SETMASK};

    #[test]
    fn fresh_table_is_all_default_unblocked_no_altstack() {
        let t = SigTable::default();
        for sig in 1..32u64 {
            assert_eq!(t.action(sig).disp, Disposition::Dfl, "sig {sig}");
            assert!(!t.is_blocked(sig), "sig {sig}");
        }
        assert_eq!(t.mask(), 0);
        assert_eq!(t.altstack(), None);
    }

    #[test]
    fn set_action_returns_the_previous_action() {
        let mut t = SigTable::default();
        let old = t.set_action(6, SigAction { disp: Disposition::Ign, mask: 0, flags: 0 });
        assert_eq!(old.disp, Disposition::Dfl, "first install returns the default");
        let old = t.set_action(6, SigAction { disp: Disposition::Handler(0x1_0000), mask: 3, flags: 4 });
        assert_eq!(old.disp, Disposition::Ign, "second install returns what the first set");
        assert_eq!(t.action(6), SigAction { disp: Disposition::Handler(0x1_0000), mask: 3, flags: 4 });
    }

    #[test]
    fn mask_honours_block_unblock_setmask() {
        let mut t = SigTable::default();
        assert_eq!(t.set_mask(SIG_BLOCK, 0b0110), 0, "returns the OLD mask");
        assert_eq!(t.mask(), 0b0110);
        assert_eq!(t.set_mask(SIG_BLOCK, 0b1000), 0b0110);
        assert_eq!(t.mask(), 0b1110, "BLOCK is a union");
        assert_eq!(t.set_mask(SIG_UNBLOCK, 0b0100), 0b1110);
        assert_eq!(t.mask(), 0b1010, "UNBLOCK clears");
        assert_eq!(t.set_mask(SIG_SETMASK, 0b0001), 0b1010);
        assert_eq!(t.mask(), 0b0001, "SETMASK replaces");
    }

    #[test]
    fn is_blocked_indexes_by_sig_minus_one() {
        let mut t = SigTable::default();
        t.set_mask(SIG_SETMASK, 1 << 5);   // bit 5 == signal 6
        assert!(t.is_blocked(6), "bit (sig-1) is the encoding");
        assert!(!t.is_blocked(5));
        assert!(!t.is_blocked(7));
    }

    // THE golden test. See the struct layouts in the task background: 24 bytes in, 16 bytes out.
    #[test]
    fn decode_act_reads_24_bytes_and_ignores_sa_tramp() {
        let mut b = [0u8; 24];
        b[0..8].copy_from_slice(&0xdead_0000u64.to_le_bytes());   // handler VA
        b[8..16].copy_from_slice(&0xbeef_0000u64.to_le_bytes());  // sa_tramp — MUST be ignored
        b[16..20].copy_from_slice(&0x0000_00ffu32.to_le_bytes()); // sa_mask
        b[20..24].copy_from_slice(&0x0000_0042u32.to_le_bytes()); // sa_flags
        let a = decode_act(&b);
        assert_eq!(a.disp, Disposition::Handler(0xdead_0000));
        assert_eq!(a.mask, 0xff);
        assert_eq!(a.flags, 0x42);
    }

    #[test]
    fn decode_act_maps_sig_dfl_and_sig_ign() {
        let mut b = [0u8; 24];
        b[0..8].copy_from_slice(&0u64.to_le_bytes());
        assert_eq!(decode_act(&b).disp, Disposition::Dfl);
        b[0..8].copy_from_slice(&1u64.to_le_bytes());
        assert_eq!(decode_act(&b).disp, Disposition::Ign);
    }

    // The one that stops the 8-byte guest corruption. Every offset is pinned.
    #[test]
    fn encode_oldact_is_exactly_16_bytes_with_no_sa_tramp() {
        let out = encode_oldact(SigAction {
            disp: Disposition::Handler(0xdead_0000), mask: 0xff, flags: 0x42 });
        assert_eq!(out.len(), 16, "struct sigaction is 16 bytes — NOT struct __sigaction's 24");
        assert_eq!(u64::from_le_bytes(out[0..8].try_into().unwrap()), 0xdead_0000);
        assert_eq!(u32::from_le_bytes(out[8..12].try_into().unwrap()), 0xff,
                   "sa_mask sits at offset 8, where sa_tramp would be in the INPUT struct");
        assert_eq!(u32::from_le_bytes(out[12..16].try_into().unwrap()), 0x42);
    }

    #[test]
    fn encode_oldact_round_trips_dfl_and_ign_as_0_and_1() {
        let d = encode_oldact(SigAction { disp: Disposition::Dfl, mask: 0, flags: 0 });
        assert_eq!(u64::from_le_bytes(d[0..8].try_into().unwrap()), 0);
        let i = encode_oldact(SigAction { disp: Disposition::Ign, mask: 0, flags: 0 });
        assert_eq!(u64::from_le_bytes(i[0..8].try_into().unwrap()), 1);
    }

    #[test]
    fn altstack_is_stored_and_returns_the_previous_value() {
        let mut t = SigTable::default();
        assert_eq!(t.set_altstack(Some((0x9000, 0x4000, 0))), None);
        assert_eq!(t.altstack(), Some((0x9000, 0x4000, 0)));
        assert_eq!(t.set_altstack(None), Some((0x9000, 0x4000, 0)));
    }
}
```

Add `mod sig;` and `pub use sig::{Disposition, SigAction, SigTable};` near the top of
`crates/retrace-box/src/lib.rs`, beside the existing `mod cache;`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p retrace-box --lib sig -- --test-threads=1`
Expected: FAIL to compile — `cannot find type SigTable in this scope`.

- [ ] **Step 3: Write minimal implementation**

Prepend to `crates/retrace-box/src/sig.rs`, above the test module:

```rust
//! The guest's signal dispositions (M11-signals).
//!
//! **Pure guest state.** Every field is a function of the guest's own `sigaction`/`sigprocmask`/
//! `sigaltstack` calls, so record and replay compute an identical table from an identical syscall
//! sequence and nothing here ever enters the trace. That is `FdTable::slots`' posture, and it is why
//! this module has no `Box_` and no VM in it — the whole thing is unit-testable at full speed.
//!
//! **Disposition, not delivery.** M11 models what the guest ASKED for. It never runs a handler:
//! `Handler` exists so the raise path can fail loud instead of silently applying the default action.

use retrace_arch::{NSIG, SIG_BLOCK, SIG_DFL, SIG_IGN, SIG_SETMASK, SIG_UNBLOCK};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition { Dfl, Ign, Handler(u64) }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SigAction { pub disp: Disposition, pub mask: u32, pub flags: u32 }

impl Default for SigAction {
    fn default() -> Self { SigAction { disp: Disposition::Dfl, mask: 0, flags: 0 } }
}

/// Per-signal disposition, the blocked mask, and the alternate stack.
///
/// `altstack` is STORED but never honoured: no handler runs this milestone, so there is nothing to
/// run on an alternate stack. Keeping it makes `sigaltstack` a real syscall with a real writeback
/// rather than a lie, and costs one field.
#[derive(Debug, Clone)]
pub struct SigTable {
    /// Indexed by signal number; `[0]` is unused so indexing mirrors signal numbering (1..=31).
    disp: [SigAction; NSIG],
    /// Bit `(sig - 1)`, matching `sigset_t`'s encoding for signals 1..=32.
    blocked: u32,
    altstack: Option<(u64, u64, u64)>,
}

impl Default for SigTable {
    /// All-default, nothing blocked, no alt stack — which is genuinely correct for a fresh process,
    /// so there is no seeding step that could be got wrong.
    fn default() -> Self {
        SigTable { disp: [SigAction::default(); NSIG], blocked: 0, altstack: None }
    }
}

impl SigTable {
    fn idx(sig: u64) -> usize {
        assert!(sig >= 1 && (sig as usize) < NSIG,
                "signal {sig} out of range 1..{NSIG} — the guest passed a signal number the table \
                 cannot represent; widen NSIG or reject it at the syscall arm");
        sig as usize
    }

    pub fn action(&self, sig: u64) -> SigAction { self.disp[Self::idx(sig)] }

    pub fn set_action(&mut self, sig: u64, a: SigAction) -> SigAction {
        std::mem::replace(&mut self.disp[Self::idx(sig)], a)
    }

    pub fn is_blocked(&self, sig: u64) -> bool {
        self.blocked & (1u32 << (Self::idx(sig) - 1)) != 0
    }

    pub fn mask(&self) -> u32 { self.blocked }

    pub fn set_mask(&mut self, how: u64, set: u32) -> u32 {
        let old = self.blocked;
        self.blocked = match how {
            SIG_BLOCK => old | set,
            SIG_UNBLOCK => old & !set,
            SIG_SETMASK => set,
            _ => panic!("sigprocmask how={how} is not BLOCK(1)/UNBLOCK(2)/SETMASK(3) — an \
                         unmodelled value, not a guest error to swallow"),
        };
        old
    }

    pub fn altstack(&self) -> Option<(u64, u64, u64)> { self.altstack }

    pub fn set_altstack(&mut self, ss: Option<(u64, u64, u64)>) -> Option<(u64, u64, u64)> {
        std::mem::replace(&mut self.altstack, ss)
    }
}

/// Decode the ACT argument: `struct __sigaction`, 24 bytes (`sys/signal.h:277`).
///
/// `sa_tramp` (offset 8) is read past and DISCARDED — it addresses libc's signal trampoline, which
/// only matters once a handler is delivered, and M11 delivers nothing.
pub fn decode_act(bytes: &[u8]) -> SigAction {
    assert!(bytes.len() >= 24,
            "struct __sigaction is 24 bytes, got {} — the caller read too few guest bytes",
            bytes.len());
    let h = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    SigAction {
        disp: match h {
            SIG_DFL => Disposition::Dfl,
            SIG_IGN => Disposition::Ign,
            va => Disposition::Handler(va),
        },
        mask: u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
        flags: u32::from_le_bytes(bytes[20..24].try_into().unwrap()),
    }
}

/// Encode the OLDACT writeback: `struct sigaction`, **16 bytes** (`sys/signal.h:287`) — the input
/// struct's `sa_tramp` is absent, so `sa_mask` moves from offset 16 to offset 8.
///
/// The return type is a fixed `[u8; 16]` on purpose: emitting 24 bytes here would corrupt the guest
/// 8 bytes past the struct, and the fixed width makes that impossible rather than merely tested.
pub fn encode_oldact(a: SigAction) -> [u8; 16] {
    let h = match a.disp {
        Disposition::Dfl => SIG_DFL,
        Disposition::Ign => SIG_IGN,
        Disposition::Handler(va) => va,
    };
    let mut o = [0u8; 16];
    o[0..8].copy_from_slice(&h.to_le_bytes());
    o[8..12].copy_from_slice(&a.mask.to_le_bytes());
    o[12..16].copy_from_slice(&a.flags.to_le_bytes());
    o
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p retrace-box --lib sig -- --test-threads=1`
Expected: PASS (9 tests).

Run: `cargo clippy -p retrace-box --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/sig.rs crates/retrace-box/src/lib.rs
git commit -m "M11 t2: SigTable — the guest's dispositions, pure and VM-free

The codecs are split because sigaction(2)'s in-param and out-param are
different C structs: struct __sigaction is 24 bytes (it carries sa_tramp),
struct sigaction is 16. encode_oldact returns a fixed [u8; 16] so emitting
the input width is impossible rather than merely tested."
```

---

### Task 3: `Event::Signal` and the format bump

**Files:**
- Modify: `crates/retrace-trace/src/lib.rs` (`Event` at line 14; `TRACE_MAGIC` at line 22)
- Modify: `crates/retrace-core/src/lib.rs` (`Outcome` at line 54)
- Modify: `crates/retrace/src/main.rs` (three `Outcome::Crash` arms at lines 21, 51, 68)
- Modify: `crates/retrace/src/debug.rs` (`Outcome::Crash` arm at line 339)

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `retrace_trace::Event::Signal { sig: u64, pc: u64 }`
  - `retrace_core::Outcome::Signal { sig: u64 }`
  - `TRACE_MAGIC` = `*b"RT\x00\x05"`

**Why this is its own task.** Adding a variant to `Event` and `Outcome` breaks every exhaustive
`match` in the workspace. Doing it separately means Task 4 starts from a compiling tree, and it keeps
"the format changed" as one reviewable commit. Nothing emits `Event::Signal` yet — the gate stays at
212 green.

No trace binary is checked into the repo (`rung3.json` is jq's input; `mach_msg2_capture.txt` is a
message capture), so the magic bump invalidates no fixture. It does invalidate any stale `t.bin` a
developer has lying around, which `open_checked` already rejects loudly.

- [ ] **Step 1: Write the failing test**

Add to `crates/retrace-trace/src/lib.rs`'s existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn signal_event_round_trips() {
    let dir = std::env::temp_dir().join(format!("retrace-sigev-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("sig.bin");
    let mut w = Writer::create(&p).unwrap();
    w.append(&Event::Signal { sig: 6, pc: 0x1_0000 }).unwrap();
    drop(w);
    let (events, torn) = Reader::open_checked(&p).unwrap();
    assert!(!torn);
    assert_eq!(events.len(), 1);
    match &events[0] {
        Event::Signal { sig, pc } => { assert_eq!(*sig, 6); assert_eq!(*pc, 0x1_0000); }
        other => panic!("expected Signal, got {other:?}"),
    }
    std::fs::remove_file(&p).ok();
}

#[test]
fn the_magic_is_version_5_and_rejects_version_4() {
    assert_eq!(TRACE_MAGIC, *b"RT\x00\x05", "M11 added Event::Signal — a format break");
    let dir = std::env::temp_dir().join(format!("retrace-oldmagic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("v4.bin");
    std::fs::write(&p, b"RT\x00\x04junkjunk").unwrap();
    let (events, torn) = Reader::open_checked(&p).unwrap();
    assert!(torn, "a v4 trace must be rejected, not misparsed");
    assert!(events.is_empty());
    std::fs::remove_file(&p).ok();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p retrace-trace -- --test-threads=1`
Expected: FAIL — `no variant named Signal found for enum Event`.

- [ ] **Step 3: Write minimal implementation**

In `crates/retrace-trace/src/lib.rs`:

```rust
pub enum Event {
    Snapshot { regs: Regs, mem: Vec<Region> },
    Syscall { num: u64, args: [u64;8], ret: u64, err: bool, writes: Vec<Region> },
    Sched { thread: u32, until: u64 },
    Exit { code: u64 },
    Crash { pc: u64, esr: u64, far: u64 },
    /// M11: the guest raised a signal on itself whose disposition is the default fatal action.
    /// Terminal, exactly like `Crash` — followed by the final full-memory `Snapshot`.
    ///
    /// Deliberately NOT folded into `Crash` with a synthetic ESR: a signal is not a fault, and a
    /// SIGABRT printing as a fault bearing an ESR the hardware never produced is a lie the debug
    /// output would carry forever. `pc` names the raise site, which is what makes it useful.
    Signal { sig: u64, pc: u64 },
}

pub const TRACE_MAGIC: [u8;4] = *b"RT\x00\x05"; // "RT" + format version 0x0005 (M11: Event::Signal)
```

In `crates/retrace-core/src/lib.rs` at line 54:

```rust
pub enum Outcome {
    Exit { code: u64 },
    Crash { pc: u64, esr: u64, far: u64 },
    /// M11: terminated by a signal the guest raised on itself.
    Signal { sig: u64 },
}
```

In `crates/retrace/src/main.rs`, add an arm beside **each** of the three `Outcome::Crash` arms
(lines 21, 51, 68). All three are identical:

```rust
                        retrace_core::Outcome::Signal { sig } => {
                            eprintln!("guest terminated by signal {sig}");
                            // 128 + sig: the convention M6 already uses for a crash
                            // (139 == 128 + SIGSEGV). SIGABRT therefore exits 134.
                            exit(128 + sig as i32);
                        }
```

In `crates/retrace/src/debug.rs`, beside the `Outcome::Crash` arm at line 339, mirroring however that
arm presents itself:

```rust
            Outcome::Signal { sig } => {
                println!("guest terminated by signal {sig}");
            }
```

- [ ] **Step 4: Run the full gate to verify nothing regressed**

Run: `cargo test -p retrace-trace -- --test-threads=1`
Expected: PASS (2 new tests).

Run: `just gate`
Expected: **226 passed / 0 failed / 0 ignored** (212 baseline + 3 from Task 1 + 9 from Task 2 + 2
here), clippy clean. Nothing emits `Event::Signal` yet.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-trace/src/lib.rs crates/retrace-core/src/lib.rs \
        crates/retrace/src/main.rs crates/retrace/src/debug.rs
git commit -m "M11 t3: Event::Signal, Outcome::Signal, TRACE_MAGIC 0x0004 -> 0x0005

A format break, taken deliberately rather than reusing Event::Crash with a
synthetic ESR: a signal is not a fault, and printing SIGABRT as a fault
bearing an ESR the hardware never produced is a lie the debug output would
carry forever. No trace binary is checked in, so no fixture is invalidated.

Exit code is 128+sig, the convention M6 already established (139 = 128+11).
Nothing emits the event yet; gate stays green."
```

---

### Task 4: The record-side integration (atomic)

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (`Box_` struct — add the `sigtable` field; a `sig_*`
  accessor pair near the fd-table accessors)
- Modify: `crates/retrace-core/src/lib.rs` (new arms in `record_box`'s `match stop`, **above** the
  generic forward arm at line 441)
- Test: `crates/retrace-core/tests/signals.rs` (create)

**Interfaces:**
- Consumes: Task 1's constants and `default_action`/`is_signal_syscall`; Task 2's `SigTable`,
  `decode_act`, `encode_oldact`; Task 3's `Event::Signal` / `Outcome::Signal`.
- Produces:
  - `Box_::sigtable(&self) -> &SigTable` and `Box_::sigtable_mut(&mut self) -> &mut SigTable`
  - Record-side behaviour every later task depends on.

**This task cannot be split.** A partial version — servicing `sigaction` but still forwarding
`kill` — leaves the recorder-killing bug live while claiming the table works, which is worse than
either end state.

**Placement is the fix.** The new arms go **above** `Stop::Syscall { num, args } =>` at line 441.
That ordering is the entire correctness argument: it is what keeps `forward_and_diff` from ever
seeing a signal syscall.

- [ ] **Step 1: Write the failing tests**

Create `crates/retrace-core/tests/signals.rs`:

```rust
// M11: the record-side signal contract, exercised through the freestanding asm guests.
// Task 7 adds the CLI-level e2e gates; these are the in-process record assertions.
use retrace_core::Outcome;

#[test]
fn a_self_raised_sigabrt_is_a_recorded_terminal_signal_not_a_dead_recorder() {
    let dir = std::env::temp_dir().join(format!("retrace-m11-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let trace = dir.join("raise.bin");
    let bytes = std::fs::read(retrace_guest::RAISE).expect("read raise guest");
    let loaded = retrace_guest::parse_macho(&bytes);
    let s = retrace_core::record(&loaded, &trace).expect("record must SUCCEED — the whole point");
    match s.outcome {
        Outcome::Signal { sig } => assert_eq!(sig, 6, "SIGABRT"),
        other => panic!("expected Outcome::Signal, got {other:?}"),
    }
    // The terminal pair: Signal, then the final full-memory snapshot.
    let (events, torn) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(!torn, "a complete recording must not be torn");
    assert!(matches!(events[events.len() - 2], retrace_trace::Event::Signal { sig: 6, .. }),
            "second-to-last event must be Signal");
    assert!(matches!(events[events.len() - 1], retrace_trace::Event::Snapshot { .. }),
            "last event must be the final memory snapshot");
    std::fs::remove_file(&trace).ok();
}

#[test]
fn an_ignored_signal_does_not_terminate_the_guest() {
    let dir = std::env::temp_dir().join(format!("retrace-m11-ign-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let trace = dir.join("sigign.bin");
    let bytes = std::fs::read(retrace_guest::SIGIGN).expect("read sigign guest");
    let loaded = retrace_guest::parse_macho(&bytes);
    let s = retrace_core::record(&loaded, &trace).expect("record");
    match s.outcome {
        Outcome::Exit { code } => assert_eq!(code, 0, "the guest ran PAST the ignored raise"),
        other => panic!("SIG_IGN must not terminate; got {other:?}"),
    }
    assert_eq!(s.stdout, b"ok\n", "the guest kept running and produced output");
    std::fs::remove_file(&trace).ok();
}

#[test]
#[should_panic(expected = "kill to a pid other than the guest's own")]
fn killing_another_process_fails_loud_instead_of_signalling_the_host() {
    let dir = std::env::temp_dir().join(format!("retrace-m11-ko-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let trace = dir.join("killother.bin");
    let bytes = std::fs::read(retrace_guest::KILLOTHER).expect("read killother guest");
    let loaded = retrace_guest::parse_macho(&bytes);
    let _ = retrace_core::record(&loaded, &trace);
}
```

These depend on Task 7's guest fixtures. **Build the three asm guests and their path constants
first** — that is Task 7 Step 1, and it is fine to pull it forward into this task since a test
without a guest cannot run. If you prefer strict task order, run Task 7 Step 1 now and commit it with
this task.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p retrace-core --test signals -- --test-threads=1`
Expected: FAIL — the first test's `record` returns `Err`, or the process dies with signal 6 (which is
precisely the bug under repair).

- [ ] **Step 3a: Add the table to the box**

In `crates/retrace-box/src/lib.rs`, add the field to `Box_` (anywhere **except** before `vcpu`/`vm` —
do not disturb the declared order of those two):

```rust
    /// M11: the guest's signal dispositions. Pure guest state — a function of the guest's own
    /// sigaction/sigprocmask/sigaltstack calls, identical on record and replay, so it never enters
    /// the trace. See `sig.rs`.
    sigtable: SigTable,
```

Initialise it as `SigTable::default()` wherever `Box_` is constructed, and add the accessors beside
the existing fd-table ones:

```rust
    pub fn sigtable(&self) -> &SigTable { &self.sigtable }
    pub fn sigtable_mut(&mut self) -> &mut SigTable { &mut self.sigtable }
```

- [ ] **Step 3b: Add the record arms**

In `crates/retrace-core/src/lib.rs`, insert **above** `Stop::Syscall { num, args } => {` at line 441:

```rust
            // ---- M11-signals ---------------------------------------------------------------
            // Placed ABOVE the generic forward arm on purpose: that ordering is what keeps
            // forward_and_diff — which issues a raw svc in RETRACE's process — from ever seeing a
            // signal syscall. Before M11, `__pthread_kill(self, SIGABRT)` killed the recorder,
            // `sigaction` installed a guest VA as the RECORDER's handler, and `kill` reached any
            // host pid. All three are gone by construction here, not by guard.

            // Serviced state calls. Never forwarded; each synthesizes its own writeback and appends
            // an ordinary Event::Syscall, so the divergence oracle still checks (num, args) and
            // RETRACE_TRACE=1 still shows the sequence. Replay mirrors these in Task 5.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SIGACTION => {
                let sig = args[0];
                let new = if args[1] != 0 {
                    Some(retrace_box::decode_act(&b.read_guest(args[1], 24)))
                } else { None };
                let old = match new {
                    Some(a) => b.sigtable_mut().set_action(sig, a),
                    None => b.sigtable().action(sig),
                };
                // oldact is `struct sigaction` — 16 bytes, NOT the 24-byte input struct.
                let writes = if args[2] != 0 {
                    let bytes = retrace_box::encode_oldact(old).to_vec();
                    b.write_guest(args[2], &bytes);
                    vec![retrace_trace::Region { addr: args[2], data: bytes }]
                } else { vec![] };
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes })
                    .map_err(|e| format!("append sigaction: {e}"))?; count += 1;
                b.set_x0_err_and_return(0, false);
            }
            Stop::Syscall { num, args }
                if num == retrace_arch::SYS_SIGPROCMASK || num == retrace_arch::SYS_PTHREAD_SIGMASK => {
                // (how, set*, oldset*). A NULL `set` is a pure query — read the mask, change nothing.
                let old = if args[1] != 0 {
                    let set = u32::from_le_bytes(b.read_guest(args[1], 4).try_into().unwrap());
                    b.sigtable_mut().set_mask(args[0], set)
                } else {
                    b.sigtable().mask()
                };
                let writes = if args[2] != 0 {
                    let bytes = old.to_le_bytes().to_vec();
                    b.write_guest(args[2], &bytes);
                    vec![retrace_trace::Region { addr: args[2], data: bytes }]
                } else { vec![] };
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes })
                    .map_err(|e| format!("append sigprocmask: {e}"))?; count += 1;
                b.set_x0_err_and_return(0, false);
            }
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SIGPENDING => {
                // Always empty, and TRUE by construction: raising a blocked signal asserts below,
                // so no signal can ever be pending. These two decisions stand or fall together.
                let writes = if args[0] != 0 {
                    let bytes = 0u32.to_le_bytes().to_vec();
                    b.write_guest(args[0], &bytes);
                    vec![retrace_trace::Region { addr: args[0], data: bytes }]
                } else { vec![] };
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes })
                    .map_err(|e| format!("append sigpending: {e}"))?; count += 1;
                b.set_x0_err_and_return(0, false);
            }
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SIGALTSTACK => {
                // stack_t { void *ss_sp; size_t ss_size; int ss_flags; } — 24 bytes with padding.
                let new = if args[0] != 0 {
                    let raw = b.read_guest(args[0], 24);
                    Some((u64::from_le_bytes(raw[0..8].try_into().unwrap()),
                          u64::from_le_bytes(raw[8..16].try_into().unwrap()),
                          u32::from_le_bytes(raw[16..20].try_into().unwrap()) as u64))
                } else { None };
                let old = match new {
                    Some(ss) => b.sigtable_mut().set_altstack(Some(ss)),
                    None => b.sigtable().altstack(),
                };
                let writes = if args[1] != 0 {
                    let (sp, size, flags) = old.unwrap_or((0, 0, 0));
                    let mut bytes = vec![0u8; 24];
                    bytes[0..8].copy_from_slice(&sp.to_le_bytes());
                    bytes[8..16].copy_from_slice(&size.to_le_bytes());
                    bytes[16..20].copy_from_slice(&(flags as u32).to_le_bytes());
                    b.write_guest(args[1], &bytes);
                    vec![retrace_trace::Region { addr: args[1], data: bytes }]
                } else { vec![] };
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes })
                    .map_err(|e| format!("append sigaltstack: {e}"))?; count += 1;
                b.set_x0_err_and_return(0, false);
            }

            // The raise path. `kill(pid, sig)` and `__pthread_kill(port, sig)` differ only in how
            // the target is validated; the disposition decision below is shared.
            Stop::Syscall { num, args }
                if num == retrace_arch::SYS_KILL || num == retrace_arch::SYS_PTHREAD_KILL => {
                if num == retrace_arch::SYS_KILL {
                    // A SAFETY boundary, not a fidelity one: forwarding this would signal a REAL
                    // host process. getpid is not intercepted, so the guest's pid IS retrace's.
                    let self_pid = std::process::id() as u64;
                    assert_eq!(args[0], self_pid,
                        "kill to a pid other than the guest's own ({} != {self_pid}) is not \
                         modelled: the guest has no children and no other process it may signal, \
                         and forwarding would signal a REAL host process. Implement a guest pid \
                         namespace before a guest needs this.", args[0]);
                }
                let sig = args[1];
                let act = b.sigtable().action(sig);
                assert!(!b.sigtable().is_blocked(sig),
                    "raising blocked signal {sig} is not modelled: it must go PENDING until \
                     unblocked, and M11 models no pending set (measured: no gate guest does this; \
                     abort() unblocks SIGABRT before raising). Implement a pending mask before a \
                     guest needs this — and note that sigpending's always-empty answer stops being \
                     true the moment you do.");
                match act.disp {
                    retrace_box::Disposition::Handler(va) => panic!(
                        "signal {sig} has a handler installed at {va:#x}, and M11 models \
                         DISPOSITION but not DELIVERY — running it needs a signal frame, the \
                         __sigtramp ABI, and sigreturn(184). Implement those (M12) before a guest \
                         raises a caught signal."),
                    retrace_box::Disposition::Ign => {
                        w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![] })
                            .map_err(|e| format!("append ignored raise: {e}"))?; count += 1;
                        b.set_x0_err_and_return(0, false);
                    }
                    retrace_box::Disposition::Dfl => match retrace_arch::default_action(sig) {
                        retrace_arch::DefaultAction::Ignore => {
                            w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![] })
                                .map_err(|e| format!("append default-ignored raise: {e}"))?; count += 1;
                            b.set_x0_err_and_return(0, false);
                        }
                        // TERMINAL. Same shape as the Exit and Crash arms above: the event, then
                        // the final full-memory snapshot, then break.
                        retrace_arch::DefaultAction::Terminate => {
                            let pc = b.position();
                            let final_snap = b.snapshot();
                            w.append(&Event::Signal { sig, pc })
                                .map_err(|e| format!("append signal: {e}"))?; count += 1;
                            w.append(&final_snap)
                                .map_err(|e| format!("append final snapshot: {e}"))?; count += 1;
                            outcome = Outcome::Signal { sig };
                            break;
                        }
                    },
                }
            }

            // Unmodelled, and loud about it. Each of these would otherwise reach forward_and_diff
            // and execute against RETRACE's process — 520/521 are live recorder-killing hazards
            // today, which is why they are asserted even though modelling them is out of scope.
            Stop::Syscall { num, .. } if num == retrace_arch::SYS_SIGRETURN => panic!(
                "sigreturn(184) is unreachable by construction: it can only be called from inside a \
                 signal handler, and M11 never delivers one. Reaching it means the disposition \
                 model itself is wrong — do not add a handler for it, find the bug."),
            Stop::Syscall { num, .. }
                if num == retrace_arch::SYS_SIGSUSPEND || num == retrace_arch::SYS_SIGWAIT => panic!(
                "syscall {num} (sigsuspend/__sigwait) blocks until a signal arrives, and the guest \
                 has ONE thread on ONE vCPU with nothing to wake it — servicing it would deadlock. \
                 Implement threads before a guest needs this."),
            Stop::Syscall { num, .. }
                if num == retrace_arch::SYS_TERMINATE_WITH_PAYLOAD
                || num == retrace_arch::SYS_ABORT_WITH_PAYLOAD => panic!(
                "syscall {num} (terminate/abort_with_payload) is a terminal path that bypasses \
                 signal disposition entirely and is not modelled (measured: unexercised by any gate \
                 guest). It is asserted rather than forwarded because forwarding it kills the \
                 RECORDER. Model it as a second terminal event shape if a guest needs it."),
```

- [ ] **Step 3c: Prove the correctness invariant in one place**

Add to the top of the generic forward arm at what is now the end of the chain
(`Stop::Syscall { num, args } => {`), beside the existing `dup2` assert:

```rust
                // M11 correctness invariant: no signal syscall may reach forward_and_diff, which
                // issues a raw svc in retrace's own process. If one does, an arm above is missing.
                assert!(!retrace_arch::is_signal_syscall(num),
                    "signal syscall {num} reached the generic forward arm — it must be serviced or \
                     asserted above (M11 correctness invariant)");
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p retrace-core --test signals -- --test-threads=1`
Expected: PASS (3 tests).

Run: `just gate`
Expected: **229 passed / 0 failed / 0 ignored**, clippy clean.

If `write_guest` does not exist on `Box_` with that name, use whatever the fd/mmap paths already use
to stage bytes into guest memory and record them as a `Region` — do **not** invent a second
mechanism. Grep for how `forward_and_diff` builds its `writes` vector and follow it exactly.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs \
        crates/retrace-core/tests/signals.rs
git commit -m "M11 t4: service signals against the guest, never against retrace

The arms go ABOVE the generic forward arm, and that ordering IS the fix:
forward_and_diff issues a raw svc in retrace's own process, so every signal
syscall reaching it executed against the recorder. __pthread_kill killed it,
sigaction installed a guest VA as ITS handler, and kill reached any host pid.

A raise now consults the guest's own table: Ign continues, Handler asserts
(delivery is M12), Dfl+fatal appends Event::Signal and ends the recording in
the shape M6 gave a fault. The invariant is asserted in the forward arm
rather than merely documented."
```

---

### Task 5: The replay mirror

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` (`ReplaySession::advance`, the `Stop::Syscall` arm at
  line 557 and the terminal arms beside `Stop::Fault` at line 836)
- Test: `crates/retrace-core/tests/signals.rs` (extend)

**Interfaces:**
- Consumes: everything from Task 4.
- Produces: replay that reproduces `Outcome::Signal` and diverges on any mismatch.

**Symmetry rule 1 is the whole task.** Each serviced call must recompute the *same* table transition
and the *same* writeback bytes, then byte-compare against the recording. That comparison **is** the
divergence check — an asymmetry must surface as a `Divergence`, never as silent corruption.

- [ ] **Step 1: Write the failing tests**

Append to `crates/retrace-core/tests/signals.rs`:

```rust
#[test]
fn a_recorded_signal_replays_identically_twice() {
    let dir = std::env::temp_dir().join(format!("retrace-m11-rep-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let trace = dir.join("raise-replay.bin");
    let bytes = std::fs::read(retrace_guest::RAISE).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    let rec = retrace_core::record(&loaded, &trace).expect("record");
    for i in 0..2 {
        let rep = retrace_core::replay(&trace)
            .unwrap_or_else(|d| panic!("replay {i} diverged at landmark {}: {}", d.landmark, d.detail));
        match rep.outcome {
            Outcome::Signal { sig } => assert_eq!(sig, 6),
            other => panic!("replay {i}: expected Outcome::Signal, got {other:?}"),
        }
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged");
    }
    std::fs::remove_file(&trace).ok();
}

#[test]
fn the_sigign_guest_replays_bit_for_bit() {
    let dir = std::env::temp_dir().join(format!("retrace-m11-ignrep-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let trace = dir.join("sigign-replay.bin");
    let bytes = std::fs::read(retrace_guest::SIGIGN).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    let rec = retrace_core::record(&loaded, &trace).expect("record");
    let rep = retrace_core::replay(&trace)
        .unwrap_or_else(|d| panic!("diverged at landmark {}: {}", d.landmark, d.detail));
    assert_eq!(rep.stdout, rec.stdout);
    assert_eq!(rep.stdout, b"ok\n");
    std::fs::remove_file(&trace).ok();
}

// The second oracle: two RECORDINGS byte-compared. Valid because these are freestanding guests —
// no clock, no entropy, no libmalloc, no mach ports. See util::assert_trace_reproducible's doc.
#[test]
fn two_recordings_of_the_raise_guest_are_byte_identical() {
    let dir = std::env::temp_dir().join(format!("retrace-m11-det-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bytes = std::fs::read(retrace_guest::RAISE).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    let (t1, t2) = (dir.join("d1.bin"), dir.join("d2.bin"));
    retrace_core::record(&loaded, &t1).expect("record 1");
    retrace_core::record(&loaded, &t2).expect("record 2");
    assert_eq!(std::fs::read(&t1).unwrap(), std::fs::read(&t2).unwrap(),
               "a nondeterministic value entered the trace");
    std::fs::remove_file(&t1).ok();
    std::fs::remove_file(&t2).ok();
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p retrace-core --test signals -- --test-threads=1`
Expected: FAIL — `expected recorded syscall, got Some(Signal { .. })`, because replay has no arm for
the serviced calls or the terminal event.

- [ ] **Step 3: Write the replay mirrors**

In `ReplaySession::advance`, inside the `Stop::Syscall` arm's recorded-`Event::Syscall` branch (after
the existing `(num, args)` mismatch check at line 586), add the recompute-and-compare for the
serviced calls:

```rust
                            // M11 mirror of record's serviced-signal arms. Recompute the SAME table
                            // transition and the SAME writeback bytes, then byte-compare against the
                            // recording — that comparison IS the divergence check (symmetry rule 1).
                            if num == retrace_arch::SYS_SIGACTION {
                                let new = if args[1] != 0 {
                                    Some(retrace_box::decode_act(&self.b.read_guest(args[1], 24)))
                                } else { None };
                                let old = match new {
                                    Some(a) => self.b.sigtable_mut().set_action(args[0], a),
                                    None => self.b.sigtable().action(args[0]),
                                };
                                if args[2] != 0 {
                                    let mine = retrace_box::encode_oldact(old).to_vec();
                                    let recorded = writes.iter().find(|r| r.addr == args[2])
                                        .map(|r| r.data.clone()).unwrap_or_default();
                                    if mine != recorded {
                                        return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                            "sigaction oldact mismatch at {:#x}: recomputed {mine:02x?} \
                                             != recorded {recorded:02x?}", args[2]) });
                                    }
                                }
                            }
                            if num == retrace_arch::SYS_SIGPROCMASK
                                || num == retrace_arch::SYS_PTHREAD_SIGMASK {
                                let old = if args[1] != 0 {
                                    let set = u32::from_le_bytes(
                                        self.b.read_guest(args[1], 4).try_into().unwrap());
                                    self.b.sigtable_mut().set_mask(args[0], set)
                                } else {
                                    self.b.sigtable().mask()
                                };
                                if args[2] != 0 {
                                    let mine = old.to_le_bytes().to_vec();
                                    let recorded = writes.iter().find(|r| r.addr == args[2])
                                        .map(|r| r.data.clone()).unwrap_or_default();
                                    if mine != recorded {
                                        return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                            "sigprocmask oldset mismatch at {:#x}: recomputed \
                                             {mine:02x?} != recorded {recorded:02x?}", args[2]) });
                                    }
                                }
                            }
                            if num == retrace_arch::SYS_SIGALTSTACK {
                                let new = if args[0] != 0 {
                                    let raw = self.b.read_guest(args[0], 24);
                                    Some((u64::from_le_bytes(raw[0..8].try_into().unwrap()),
                                          u64::from_le_bytes(raw[8..16].try_into().unwrap()),
                                          u32::from_le_bytes(raw[16..20].try_into().unwrap()) as u64))
                                } else { None };
                                match new {
                                    Some(ss) => { self.b.sigtable_mut().set_altstack(Some(ss)); }
                                    None => {}
                                }
                            }
```

The recorded writes are then applied by the existing `apply_and_return` path, exactly as for any
other syscall — the mirror's job is to keep the *table* in step and to prove the bytes match, not to
re-perform the write.

Then add the terminal arm. Place it beside `Stop::Fault` at line 836, inside the same
`match self.b.run()`. Note that a signal arrives as a **`Stop::Syscall`**, not a `Stop::Fault`, so
this goes in the `Stop::Syscall` arm — before the generic recorded-`Event::Syscall` lookup, mirroring
how record's raise arm precedes its generic arm:

```rust
                    // M11 mirror of record's terminal raise. Structure copied from the Exit and
                    // Crash verifies: compare the event, then the final-memory landmark.
                    if num == retrace_arch::SYS_KILL || num == retrace_arch::SYS_PTHREAD_KILL {
                        let sig = args[1];
                        let act = self.b.sigtable().action(sig);
                        let terminal = matches!(act.disp, retrace_box::Disposition::Dfl)
                            && retrace_arch::default_action(sig) == retrace_arch::DefaultAction::Terminate;
                        if terminal {
                            match self.events.get(self.idx) {
                                Some(Event::Signal { sig: rsig, pc: rpc }) => {
                                    if sig != *rsig || pc != *rpc {
                                        return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                            "signal mismatch: live (sig={sig}, pc={pc:#x}) != \
                                             recorded (sig={rsig}, pc={rpc:#x})") });
                                    }
                                    match self.events.get(self.idx + 1) {
                                        Some(Event::Snapshot { mem: final_mem, .. }) => {
                                            if let Some(d) = self.b.diff_memory(final_mem) {
                                                return Err(Divergence { landmark: self.idx + 1, pc, detail: d });
                                            }
                                            return Ok(Advance::Exited(ReplayReport {
                                                stdout: std::mem::take(&mut self.stdout),
                                                outcome: Outcome::Signal { sig: *rsig } }));
                                        }
                                        other => return Err(Divergence { landmark: self.idx + 1, pc,
                                            detail: format!("expected final memory Snapshot after Signal, got {other:?}") }),
                                    }
                                }
                                other => return Err(Divergence { landmark: self.idx, pc,
                                    detail: format!("expected recorded Signal, got {other:?} (live raise: sig={sig})") }),
                            }
                        }
                    }
```

`DefaultAction` needs `PartialEq` for the comparison above — it is already derived in Task 1.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p retrace-core --test signals -- --test-threads=1`
Expected: PASS (6 tests).

Run: `just gate`
Expected: **232 passed / 0 failed / 0 ignored**, clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace-core/tests/signals.rs
git commit -m "M11 t5: replay recomputes the disposition and diverges on mismatch

Symmetry rule 1: each serviced call recomputes the same table transition and
the same writeback bytes, then byte-compares against the recording. That
comparison IS the divergence check, so an asymmetry surfaces as a Divergence
rather than as silent corruption.

The terminal arm mirrors the Exit/Crash verifies: compare (sig, pc), then the
final-memory landmark. A signal arrives as a Stop::Syscall, not a Stop::Fault,
so it is checked before the generic recorded-Syscall lookup."
```

---

### Task 6: Carry the table through checkpoints

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (`BoxState` at line 515; `checkpoint()` and
  `from_checkpoint()`)
- Create: `crates/retrace-box/tests/sigcheckpoint.rs` (modelled on
  `crates/retrace-box/tests/pacposture.rs:68`, which is the closest existing carriage test)

**Interfaces:**
- Consumes: Task 2's `SigTable` (needs `Clone`, already derived).
- Produces: a `BoxState` that restores dispositions.

**Why this is needed.** `BoxState` already carries `pac_enabled`, `stack_top`, `stack_size`, and the
fd slots for one stated reason: a mid-run capture cannot re-derive them. `SigTable` is the same. A
`seek` into a run that had installed a disposition would otherwise restore a box that has forgotten
it, and the first raise after the seek takes the wrong branch — a divergence that looks like a signal
bug and is actually a checkpoint bug.

- [ ] **Step 1: Write the failing test**

Create `crates/retrace-box/tests/sigcheckpoint.rs`. The structure is lifted from
`crates/retrace-box/tests/pacposture.rs:68` — including the `drop(b)` before `from_checkpoint`,
which is **mandatory**: HVF allows one VM per process, so the old box must be torn down before the
restore creates the next one.

```rust
// M11 t6, in the shape of pacposture.rs's from_checkpoint_carries_the_posture and M10 t4's
// fd_table_survives_checkpoint_restore. State a mid-run capture cannot re-derive must be CARRIED.
// If from_checkpoint installed a fresh SigTable, a seeked session would believe every signal is at
// its default disposition — so a post-seek raise of an IGNORED signal would terminate the guest,
// and reverse execution would diverge from the forward run.
use retrace_box::{Box_, Disposition, SigAction};
use retrace_guest::{parse_macho, HELLO};

#[test]
fn from_checkpoint_carries_the_signal_table() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let _ = b.run(); // reach the first syscall so the checkpoint is genuinely mid-run

    b.sigtable_mut().set_action(6, SigAction { disp: Disposition::Ign, mask: 0xf, flags: 0x2 });
    b.sigtable_mut().set_mask(retrace_arch::SIG_SETMASK, 0b1010);
    b.sigtable_mut().set_altstack(Some((0x9000, 0x4000, 0)));

    let st = b.checkpoint();
    drop(b); // one VM per process — tear down before from_checkpoint creates the next one
    let r = Box_::from_checkpoint(&st);

    assert_eq!(r.sigtable().action(6),
               SigAction { disp: Disposition::Ign, mask: 0xf, flags: 0x2 },
               "a seek must not resurrect a default disposition");
    assert_eq!(r.sigtable().mask(), 0b1010, "the blocked mask must survive the restore");
    assert_eq!(r.sigtable().altstack(), Some((0x9000, 0x4000, 0)));
}
```

If `Box_::load` is not the constructor the surrounding tests use for a static guest, copy whichever
one `pacposture.rs` uses (`load_with_pac(&loaded, false)` for a PAC-off static guest) — do not invent
a new construction path.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p retrace-box --test sigcheckpoint -- --test-threads=1`
Expected: FAIL — the restored table is `Default` (all `Dfl`, mask 0, no altstack).

- [ ] **Step 3: Write minimal implementation**

Add to `BoxState`, beside the fd-slot field and with the same shape of comment:

```rust
    // M11: carried for the same reason as `pac_enabled`, `stack_top`, and the fd slots — a mid-run
    // capture cannot re-derive it. Without this, a seek into a run that installed a disposition
    // restores a box that has forgotten it, and the next raise takes the wrong branch.
    pub sigtable: SigTable,
```

Populate it in `checkpoint()` (`sigtable: self.sigtable.clone()`) and restore it in
`from_checkpoint()` (`sigtable: st.sigtable.clone()`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p retrace-box -- --test-threads=1`
Expected: PASS.

Run: `just gate`
Expected: **233 passed / 0 failed / 0 ignored**, clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/sigcheckpoint.rs
git commit -m "M11 t6: carry the SigTable through checkpoints

Same reason already written for pac_enabled, stack_top, and the fd slots: a
mid-run capture cannot re-derive it. Without this a seek resurrects default
dispositions and the next raise takes the wrong branch — a divergence that
would read as a signal bug and actually be a checkpoint bug."
```

---

### Task 7: The guest fixtures and the three mechanism gates

**Files:**
- Create: `crates/retrace-guest/asm/raise.s`, `asm/sigign.s`, `asm/killother.s`
- Modify: `crates/retrace-guest/build.rs` (three new clang invocations, following the existing
  `-nostdlib -static -Wl,-e,_start` recipe verbatim)
- Modify: `crates/retrace-guest/src/lib.rs` (three new path constants near line 119)
- Create: `crates/retrace/tests/sigraise_e2e.rs`, `sigign_e2e.rs`, `killother_e2e.rs`

**Interfaces:**
- Consumes: everything above.
- Produces: `retrace_guest::RAISE`, `SIGIGN`, `KILLOTHER`.

**Note on `sa_tramp = 0`.** `sigign.s` hand-builds a `struct __sigaction` with a NULL `sa_tramp`,
which the real kernel would likely reject. That is fine and is the point: M11 **never forwards**
`sigaction`, so the kernel never sees it. If this fixture ever starts failing with an errno, it means
the servicing arm was bypassed.

- [ ] **Step 1: Create the three guests and wire the build**

`crates/retrace-guest/asm/raise.s`:

```asm
// M11 mechanism guest: raise SIGABRT on ourselves via kill(getpid(), SIGABRT).
// kill(37) rather than __pthread_kill(328) because a freestanding guest has no thread port
// without a mach trap, and this shape also exercises the self-pid safety check.
// The raise is TERMINAL: the exit(1) below must never execute.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    mov  x16, #20               // SYS_getpid
    svc  #0x80                  // x0 = pid (retrace's own — getpid is not intercepted)
    mov  x1, #6                 // SIGABRT
    mov  x16, #37               // SYS_kill
    svc  #0x80
    mov  x0, #1                 // UNREACHABLE — a nonzero exit makes a missed terminal loud
    mov  x16, #1                // SYS_exit
    svc  #0x80
```

`crates/retrace-guest/asm/sigign.s`:

```asm
// M11 non-terminal guest: ignore SIGABRT, raise it, then prove we kept running.
// sa_tramp is 0 — legal here ONLY because M11 never forwards sigaction to the kernel.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    mov  x0, #6                 // SIGABRT
    adrp x1, act@PAGE
    add  x1, x1, act@PAGEOFF    // act (struct __sigaction, 24 bytes)
    mov  x2, #0                 // oldact = NULL
    mov  x16, #46               // SYS_sigaction
    svc  #0x80

    mov  x16, #20               // SYS_getpid
    svc  #0x80
    mov  x1, #6                 // SIGABRT
    mov  x16, #37               // SYS_kill -- ignored, must return and continue
    svc  #0x80

    mov  x0, #1                 // fd = stdout
    adrp x1, msg@PAGE
    add  x1, x1, msg@PAGEOFF
    mov  x2, #3
    mov  x16, #4                // SYS_write
    svc  #0x80

    mov  x0, #0
    mov  x16, #1                // SYS_exit
    svc  #0x80

.section __DATA,__data
.p2align 3
act:
    .quad 1                     // __sigaction_u = SIG_IGN
    .quad 0                     // sa_tramp (unused: never forwarded)
    .long 0                     // sa_mask
    .long 0                     // sa_flags
msg:
    .ascii "ok\n"
```

`crates/retrace-guest/asm/killother.s`:

```asm
// M11 safety-boundary guest: try to signal a process that is NOT the guest.
// The recorder must abort loudly. If this ever reaches its exit(0), retrace signalled pid 1.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    mov  x0, #1                 // pid 1 (launchd)
    mov  x1, #9                 // SIGKILL
    mov  x16, #37               // SYS_kill
    svc  #0x80
    mov  x0, #0
    mov  x16, #1                // SYS_exit
    svc  #0x80
```

Add to `crates/retrace-guest/build.rs`, following the existing pattern exactly:

```rust
    // M11 signal guests. raise: kill(getpid(), SIGABRT) — the terminal mechanism. sigign: the same
    // raise with SIGABRT set to SIG_IGN first, proving the guest keeps running. killother:
    // kill(1, SIGKILL) — the safety boundary; the recorder must abort rather than signal launchd.
    for name in ["raise", "sigign", "killother"] {
        let src = format!("{}/asm/{name}.s", env!("CARGO_MANIFEST_DIR"));
        let bin = format!("{out}/{name}");
        println!("cargo:rerun-if-changed={src}");
        let status = Command::new("clang")
            .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
            .status().expect("clang signal guest");
        assert!(status.success(), "{name} guest build failed");
    }
```

Add to `crates/retrace-guest/src/lib.rs` after line 119:

```rust
pub const RAISE: &str = concat!(env!("OUT_DIR"), "/raise");
pub const SIGIGN: &str = concat!(env!("OUT_DIR"), "/sigign");
pub const KILLOTHER: &str = concat!(env!("OUT_DIR"), "/killother");
```

- [ ] **Step 2: Write the three e2e gates**

`crates/retrace/tests/sigraise_e2e.rs`:

```rust
// M11 mechanism gate: a guest that raises SIGABRT on itself records and replays bit-for-bit,
// and the RECORDER SURVIVES. Before M11 the raise was forwarded and killed the recorder (exit 134),
// leaving a trace with no terminal event that replay could only diverge on.
mod util;

#[test]
fn a_self_raised_sigabrt_records_and_replays_with_the_recorder_intact() {
    let (rec, trace) = util::record(retrace_guest::RAISE);
    // 134 == 128 + SIGABRT(6), the convention M6 established (139 == 128 + SIGSEGV).
    assert_eq!(rec.code, 134, "the GUEST died by signal 6; stderr: {}", rec.stderr);
    assert!(rec.stderr.contains("guest terminated by signal 6"), "stderr: {}", rec.stderr);

    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 134, "replay {i}; stderr: {}", rep.stderr);
        assert!(rep.stderr.contains("guest terminated by signal 6"), "replay {i}: {}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged");
    }
}

/// THE regression this milestone exists to fix, asserted directly rather than inferred.
/// Before M11 the recorder itself took the SIGABRT; a trace was never written at all.
#[test]
fn the_trace_ends_with_signal_then_the_final_snapshot() {
    let (rec, trace) = util::record(retrace_guest::RAISE);
    assert_eq!(rec.code, 134, "stderr: {}", rec.stderr);
    let (events, torn) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(!torn, "a recorder killed mid-run leaves a TORN trace — this must be complete");
    let n = events.len();
    assert!(matches!(events[n - 2], retrace_trace::Event::Signal { sig: 6, .. }),
            "expected Event::Signal at n-2, got {:?}", events[n - 2]);
    assert!(matches!(events[n - 1], retrace_trace::Event::Snapshot { .. }),
            "expected the final memory Snapshot at n-1, got {:?}", events[n - 1]);
}

#[test]
fn two_recordings_are_byte_identical() {
    // Freestanding guest: no clock, no entropy, no mach ports — the second oracle applies.
    util::assert_trace_reproducible(retrace_guest::RAISE);
}
```

`crates/retrace/tests/sigign_e2e.rs`:

```rust
// M11: the branch the terminal gate cannot reach. Without this, a bug that made EVERY raise
// terminal would pass the entire suite.
mod util;

#[test]
fn an_ignored_sigabrt_lets_the_guest_run_to_a_clean_exit() {
    let (rec, trace) = util::record(retrace_guest::SIGIGN);
    assert_eq!(rec.code, 0, "SIG_IGN must not terminate the guest; stderr: {}", rec.stderr);
    assert_eq!(rec.stdout, b"ok\n", "the guest ran PAST the raise and produced output");

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "stderr: {}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout);
}

#[test]
fn two_recordings_are_byte_identical() {
    util::assert_trace_reproducible(retrace_guest::SIGIGN);
}
```

`crates/retrace/tests/killother_e2e.rs`:

```rust
// M11 SAFETY boundary. A guest kill() aimed at another process must abort the recorder loudly.
// This is the only defect in this milestone that escapes the sandbox: before M11 the operand was
// untranslated and unchecked, so `kill(1, SIGKILL)` from a guest would have been forwarded into
// retrace's own process and signalled launchd.
mod util;

#[test]
fn killing_pid_1_aborts_the_recorder_instead_of_signalling_launchd() {
    let (rec, _trace) = util::record(retrace_guest::KILLOTHER);
    assert_ne!(rec.code, 0, "the guest must NOT reach its exit(0); stderr: {}", rec.stderr);
    assert!(rec.stderr.contains("kill to a pid other than the guest's own"),
            "the abort must name the boundary it enforced; stderr: {}", rec.stderr);
    // And launchd is still there, which is the actual thing being protected.
    assert_eq!(unsafe { libc_kill_probe(1) }, 0,
               "pid 1 must still exist — signalling it is what this test prevents");
}

/// `kill(pid, 0)` — the existence probe, sends no signal. Returns 0 if the process is alive.
/// Written inline rather than pulling in the `libc` crate for one call.
unsafe fn libc_kill_probe(pid: i32) -> i32 {
    unsafe extern "C" { fn kill(pid: i32, sig: i32) -> i32; }
    unsafe { kill(pid, 0) }
}
```

- [ ] **Step 3: Run the new gates**

Run:
```bash
cargo test -p retrace --test sigraise_e2e -- --test-threads=1
cargo test -p retrace --test sigign_e2e -- --test-threads=1
cargo test -p retrace --test killother_e2e -- --test-threads=1
```
Expected: all PASS.

If `killother_e2e`'s `kill(1, 0)` probe returns `-1` with `EPERM` rather than `0`, that still proves
pid 1 exists (permission denied means the process is there) — relax the assertion to
`assert!(r == 0 || errno == EPERM)` rather than deleting it.

- [ ] **Step 4: Run the full gate**

Run: `just gate`
Expected: **239 passed / 0 failed / 0 ignored**, clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-guest/asm/raise.s crates/retrace-guest/asm/sigign.s \
        crates/retrace-guest/asm/killother.s crates/retrace-guest/build.rs \
        crates/retrace-guest/src/lib.rs crates/retrace/tests/sigraise_e2e.rs \
        crates/retrace/tests/sigign_e2e.rs crates/retrace/tests/killother_e2e.rs
git commit -m "M11 t7: the three mechanism gates — terminal, ignored, and the safety boundary

sigraise asserts the recorder SURVIVES (exit 134 is the guest dying, not the
recorder) and that the trace is complete rather than torn — that is the actual
regression, stated directly instead of inferred from a passing replay.

sigign covers the branch the terminal gate cannot reach: without it, a bug
making every raise terminal would pass the whole suite.

killother proves pid 1 is still alive afterwards."
```

---

### Task 8: The headline gate, and the honest close

**Files:**
- Create: `crates/retrace-guest/rs/panicky.rs`
- Modify: `crates/retrace-guest/build.rs`, `src/lib.rs`
- Create: `crates/retrace/tests/panic_e2e.rs`
- Modify: `README.md` (new Status section at the end), `CLAUDE.md` (gate count, milestone list)

**Interfaces:**
- Consumes: everything above.
- Produces: `retrace_guest::PANICKY`.

- [ ] **Step 1: Create the Rust panic guest**

`crates/retrace-guest/rs/panicky.rs`:

```rust
// M11 headline guest: a real full-std Rust binary that panics. libstd's panic path ends in
// abort(), which raises SIGABRT on itself — the exact path that killed the recorder before M11.
// Prints first so the trace proves it reached its own code rather than dying inside dyld.
fn main() {
    println!("about to panic");
    panic!("M11");
}
```

Add to `build.rs`, following the `hello_rust` recipe verbatim (same `RUSTC`, same target):

```rust
    // panicky: M11's headline — a real full-std Rust binary whose panic reaches abort()/SIGABRT.
    let src = format!("{}/rs/panicky.rs", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/panicky");
    println!("cargo:rerun-if-changed={src}");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let status = Command::new(rustc)
        .args(["--target", "aarch64-apple-darwin", "-o", &bin, &src])
        .status().expect("rustc panicky");
    assert!(status.success(), "panicky guest build failed");
```

Add to `src/lib.rs`: `pub const PANICKY: &str = concat!(env!("OUT_DIR"), "/panicky");`

- [ ] **Step 2: Write the headline gate**

`crates/retrace/tests/panic_e2e.rs`:

```rust
// THE M11 HEADLINE GATE. A real dynamically-linked, full-std Rust binary panics; libstd's panic
// path reaches abort(), which raises SIGABRT on itself. Before M11 that signal was forwarded to
// the host and killed the RECORDER, so the program could not be recorded at all.
mod util;

#[test]
fn a_panicking_rust_guest_records_and_replays_its_own_death() {
    let (rec, trace) = util::record_dynamic(retrace_guest::PANICKY);
    assert_eq!(rec.code, 134, "134 == 128 + SIGABRT; stderr:\n{}", rec.stderr);
    assert!(rec.stderr.contains("guest terminated by signal 6"), "stderr:\n{}", rec.stderr);
    assert_eq!(rec.stdout, b"about to panic\n",
               "the guest must reach its OWN code, not die inside dyld");

    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 134, "replay {i}; stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged");
    }
}
```

- [ ] **Step 3: Run it and decide green-or-parked**

Run: `cargo test -p retrace --test panic_e2e -- --test-threads=1 --nocapture`

**If it passes:** leave it un-`#[ignore]`d. This is the headline and it cleared.

**If it fails**, diagnose with `RETRACE_TRACE=1` first:

```bash
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn \
  "$(find target -name panicky -type f | head -1)" -o /tmp/panicky.bin 2>&1 | tail -40
```

Three failure shapes and what each means:

- **An M11 assert fired** (`abort_with_payload`, a handler, a blocked raise): risk R2 or R3 came
  true. The assert message names what to implement. This is a *scope decision*, not a bug — write it
  up and raise it rather than silently expanding the milestone.
- **A libSystem wall before the panic** (a syscall or mach RPC unrelated to signals): the classic
  wall-chain outcome. Park the gate.
- **A divergence on replay:** a real symmetry-rule-1 bug in Task 5. Fix it; do not park.

To park, add above the test — with the **exact observed signature**, not a paraphrase:

```rust
#[ignore = "M11 wall: <paste the exact first line of the failure here>. \
            The signal mechanism itself is green (sigraise_e2e, sigign_e2e); this guest \
            does not reach its panic. See README Status: M11-signals."]
```

- [ ] **Step 4: Run the full gate and write the Status section**

Run: `just gate`

Expected: **240 passed / 0 failed / 0 ignored** if the headline cleared, or **239 / 0 / 1** if it is
parked. Record the real number — do not copy the estimate.

Append a `## Status: M11-signals — ...` section to `README.md` following the shape of the M10 section
(read it first; it is the last one in the file). It must state, honestly:

- What now works: a self-raised fatal signal is a recorded, replayable terminal event; the recorder
  survives; no signal syscall reaches retrace's process.
- The two defects fixed that were never previously documented: guest `sigaction` installing a guest
  VA as the recorder's handler, and guest `kill` reaching any host pid.
- What is explicitly NOT done: handler delivery (signal frames, `__sigtramp`, `sigreturn`) — the
  larger half, and now the top deferred item in place of the one this milestone retired.
- The measured answers from Task 1 Step 0.
- The exact gate tally, and the parked gate's signature if there is one.
- The remaining carried-forward list from M10 (`dup2`, `fcntl(F_DUPFD)`, guest stdin,
  `RLIMIT_NOFILE`, block-exclusive exec placement, `prot`, `guest_munmap`, threads, arm64e).

Update `CLAUDE.md`: the gate count, the milestone list (add M11-signals), and the honest-gate
paragraph if the headline parked.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-guest/rs/panicky.rs crates/retrace-guest/build.rs \
        crates/retrace-guest/src/lib.rs crates/retrace/tests/panic_e2e.rs \
        README.md CLAUDE.md
git commit -m "M11 t8: the panicking-Rust headline gate and the M11 Status section

<state plainly whether the gate is green or parked, and if parked, at what
exact signature — a stale or vague ignore reason is worse than none>"
```

---

## Self-Review

**Spec coverage.** Every mechanism section maps to a task: M11-table → Task 2; M11-service → Task 4;
M11-mirror → Task 5; M11-checkpoint → Task 6; M11-format → Task 3. Every fail-loud boundary in the
spec appears as a concrete `assert!`/`panic!` in Task 4 Step 3b. Every test layer in the spec's
Testing section appears: unit (Task 2), format (Task 3), `sigraise_e2e`/`sigign_e2e`/`killother_e2e`
(Task 7), `panic_e2e` (Task 8). The five unmeasured facts are Task 1 Step 0, gating everything.

**Two places where the plan deliberately departs from a naive reading of the spec:**

1. The spec's testing section says the mechanism fixture uses `getpid` + `kill`. Task 4's raise arm
   therefore handles `kill`(37) and `__pthread_kill`(328) together, since the fixture exercises 37
   while the real libc path uses 328. Both are wired; only 37 is covered by a freestanding gate,
   and 328's coverage rides on `panic_e2e`. **If `panic_e2e` parks, `__pthread_kill` is wired but
   ungated** — say so in the Status section rather than letting it read as covered.
2. Task 4 pulls Task 7's guest fixtures forward, because the record-side tests cannot run without a
   guest. Committing them together is fine; committing Task 4 without them is not.

**Known ordering hazard.** Task 5's terminal replay arm must sit *before* the generic
recorded-`Event::Syscall` lookup inside the `Stop::Syscall` arm, mirroring how record's raise arm
precedes its generic arm. Placing it after produces `expected recorded syscall, got Signal` — a
confusing divergence that looks like a recording bug and is a dispatch-ordering bug.

**Gate arithmetic.** Baseline 212 → Task 1 +3 (215) → Task 2 +9 (224) → Task 3 +2 (226) → Task 4 +3
(229) → Task 5 +3 (232) → Task 6 +1 (233) → Task 7 +6 (239) → Task 8 +1 (**240**). The running totals
in each task's Step 4 match this chain. They are still only an arithmetic prediction: **trust the
actual output over the estimate**, and correct the numbers as you go rather than assuming a mismatch
means something broke.
