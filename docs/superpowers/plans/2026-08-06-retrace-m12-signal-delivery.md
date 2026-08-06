# M12-signal-delivery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run the guest's own signal handlers — build the signal frame, enter through the real
`sa_tramp` contract, and service `sigreturn(184)` to put the guest back — for signals the guest
raises on itself **and** for real hardware faults.

**Architecture:** A pure, VM-free `build_frame` in `retrace-box/src/sig.rs` lays out the measured
976-byte frame (siginfo ‖ ucontext ‖ mcontext). `Box_::deliver_signal` writes it into guest memory and
sets the entry registers; `Box_::sigreturn_restore` reads it back. Both are called by record **and**
replay, so "recompute identically" is true by construction rather than by discipline. Handler entry
is a first-class `Event::SignalDelivery` landmark — not hidden below the trace — because a reverse
debugger's users rewind to it.

**Tech Stack:** Rust 1.95.0 (pinned), `aarch64-apple-darwin`, Hypervisor.framework, `just` for the gate.

## Global Constraints

Copied from `CLAUDE.md` and the spec. Every task's requirements implicitly include this section.

- **macOS 26.x on Apple Silicon required.** Non-root; SIP may stay enabled.
- **`--test-threads=1` is mandatory.** HVF allows one VM per process. A bare `cargo test` flakes with
  `HV_BUSY`. `just gate` sets it.
- **`just gate` is THE exit gate:** `cargo test --workspace` + `cargo clippy -D warnings`. It must end
  green with **zero `#[ignore]`** — current baseline **240 passed / 0 failed / 0 ignored**
  (85 test binaries).
- **Codesigning:** any test that spawns `CARGO_BIN_EXE_retrace` itself must sign it first — use
  `util::bin()` (`crates/retrace/tests/util/mod.rs:12`). Never hand-roll this.
- **W^X:** executing a writable guest page hangs the vCPU. Code pages are RO+exec, data RW+non-exec.
- **SPTM / anon-only memory:** a file-backed `hv_vm_map` hard-panics macOS 26.
- **Drop order:** `Box_`'s `vcpu` field must stay declared before `vm`. Do not reorder struct fields.
- **Never reimplement Apple's PAC.**
- **`clippy.toml` bans `Instant::now`/`SystemTime::now`/`std::thread`.** Load-bearing, not style.
- **Symmetry rule 1:** a special case in record's `match stop` needs a mirror in replay's dispatch, and
  both must recompute identical bytes. **Rule 2:** deterministic emulation belongs below the trace in
  `Box_::run()` — and the spec argues explicitly why delivery is the exception (a control transfer,
  not an instruction emulation). Do not "simplify" it into `run()`.
- **Honest-gate discipline:** a new wall gets a NEW parked gate, never a regression of an existing one.
- **Resolve struct offsets from the probes in `spikes/`, never memory.** Every offset this plan uses
  came out of `spikes/sigabi.c` and `spikes/sigtramp.c`; re-run them if anything looks off.
- **`RETRACE_TRACE=1` is record-only.** `ReplaySession` prints no `[trap]`/`[fault]` lines. Do not
  write a debugging step that expects them on replay.

---

## File Structure

| File | Responsibility |
|------|----------------|
| `crates/retrace-arch/src/lib.rs` (modify) | `signal_of_esr`; `UC_FLAVOR`, `SA_*`, `SS_*`, `si_code`, and signal-number constants. Zero-dependency, pure. |
| `crates/retrace-box/src/sig.rs` (modify) | `SigAction.tramp`; `ThreadState`, `NeonState`, `FrameInput`, `EntryRegs`; the pure `choose_frame_base` and `build_frame`; `sigreturn_token`. No VM, no `Box_`. |
| `crates/retrace-box/src/lib.rs` (modify) | `Box_::deliver_signal`, `Box_::sigreturn_restore`, and the private `write_guest` they share. |
| `crates/retrace-trace/src/lib.rs` (modify) | `Event::SignalDelivery`; `TRACE_MAGIC` → `0x0006`. |
| `crates/retrace-core/src/lib.rs` (modify) | Three record dispatch sites (fault / raise-with-handler / `sigreturn`), three replay mirrors, and the new fail-loud boundaries. |
| `crates/retrace-guest/asm/sigframe.s` (create) | Own trampoline; asserts the `x0..x5`/`sp` entry contract. |
| `crates/retrace-guest/asm/segvcatch.s` (create) | Handler advances `__ss.__pc` by 4 and returns — proves `sigreturn` restores **mutated** state. |
| `crates/retrace-guest/asm/altstack.s` (create) | `SA_ONSTACK` + `sigaltstack`; handler asserts its `sp` is inside the alt stack. |
| `crates/retrace-guest/asm/vecsurvive.s` (create) | A known value in `v8` across a caught fault. |
| `crates/retrace-guest/c/sigcatch_dyn.c` (create) | Real libc `sigaction` → Apple's **real** `_sigtramp`. |
| `crates/retrace-guest/rs/segvy.rs` (create) | The headline: stock full-`std` Rust, wild pointer. |
| `crates/retrace-guest/build.rs`, `src/lib.rs` (modify) | Compile + export `SIGFRAME`, `SEGVCATCH`, `ALTSTACK`, `VECSURVIVE`, `SIGCATCH_DYN`, `SEGVY`. |
| `crates/retrace/tests/sigdeliver_e2e.rs` (create) | The four asm mechanism gates. |
| `crates/retrace/tests/sigcatch_dyn_e2e.rs` (create) | The real-`_sigtramp` gate. |
| `crates/retrace/tests/segv_rust_e2e.rs` (create) | The headline gate + the reverse-seek to the delivery landmark. |

---

## Task 1: Measure, then the arch facts

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (constants after the M11 block near line 205; tests in the
  existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing (zero-dependency crate).
- Produces:
  - `pub const SIGILL: u64 = 4; SIGTRAP = 5; SIGFPE = 8; SIGBUS = 10; SIGSEGV = 11;`
  - `pub const SEGV_MAPERR: u64 = 1; SEGV_ACCERR = 2; BUS_ADRALN = 1; BUS_ADRERR = 2;`
    `BUS_OBJERR = 3; ILL_ILLOPC = 1; TRAP_BRKPT = 1;`
  - `pub const SA_ONSTACK: u32 = 0x1; SA_RESTART = 0x2; SA_RESETHAND = 0x4; SA_NODEFER = 0x10;`
    `SA_SIGINFO: u32 = 0x40;`
  - `pub const SS_ONSTACK: u64 = 0x1; SS_DISABLE: u64 = 0x4;`
  - `pub const UC_FLAVOR: u64 = 30;`
  - `pub fn signal_of_esr(esr: u64) -> (u64, u64)` — returns `(signal, si_code)`.

**Background the implementer needs.** The spec lists five unmeasured facts and says measurement comes
first. Step 0 is that measurement, and it is not optional: M10's spec asserted the guest would see
"fd 3" and the real answer was fd 4; M9 missed six syscalls by reading a truncated histogram; M11
found that a default Rust `panic!()` never raises a signal at all. Every one of those was caught by
measuring before writing.

- [ ] **Step 0: Measure (spec §"Unmeasured", risks R1/R2/R3)**

Re-run the two committed probes to confirm the numbers this plan is built on still hold:

```bash
cd /Users/noahmitchem/Documents/GitHub/retrace
clang -arch arm64 -o spikes/sigabi spikes/sigabi.c && ./spikes/sigabi
clang -arch arm64 -O0 -o spikes/sigtramp spikes/sigtramp.c && ./spikes/sigtramp
```

Expected, and the plan is wrong if any differs: `sizeof(ucontext_t)==56`, `sizeof(siginfo_t)==104`,
`sizeof(mcontext64)==816`, `uc_mcontext` at offset 48, `__ss` at 16 within mcontext, `__pc` at 256
within thread state; and from the tramp probe `x1==0x1e`, `sp==x3`, `x4-x3==104`,
`uc_mcontext-x4==56`.

**R1 — does M12's fault routing break a green gate?** This is the one that can force a scope change.

```bash
cargo build --workspace
CRASHY=$(find target -name crashy -type f | head -1)
test -n "$CRASHY" || { echo "crashy not built"; exit 1; }
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn "$CRASHY" -o /tmp/m12-crashy.bin \
  > /tmp/m12-crashy.trace 2>&1
# Every sigaction(46) the guest made, in order, with its args — x1 != 0 means an install, and
# the installed handler VA is the first 8 bytes at x1.
grep -a 'num=46' /tmp/m12-crashy.trace
```

Answer in the commit message: **does `crashy` install a handler for `SIGSEGV`(11) or `SIGBUS`(10)
before it faults?** If it does, Task 7's fault arm converts `crashy_e2e`'s `Event::Crash` into a
delivery and breaks a green gate — stop and amend the spec before writing the arm.

Do the same for the seeded swarm's injected faults:

```bash
grep -rn "Fault\|inject" crates/retrace-sim/src/lib.rs | head -20
cargo test -p retrace --test seeded_swarm -- --test-threads=1
```

**R2/R3 — the two trampoline unknowns.** Disassemble Apple's real `_sigtramp` and read what it
touches beyond the registers the kernel passes:

```bash
# The dylib lives only in the shared cache on macOS 26; extract the symbol's code.
otool -tV /usr/lib/system/libsystem_platform.dylib 2>/dev/null | sed -n '/_sigtramp/,/^_/p' | head -60
```

Record: **(a)** does `_sigtramp` read any frame field other than through `x3`/`x4`? **(b)** what does
it pass as `sigreturn`'s second argument, and does it branch on infostyle? **(c)** what is infostyle
for a **non**-`SA_SIGINFO` handler — measure by editing `spikes/sigtramp.c` to pass `sa_flags = 0`
instead of `SA_SIGINFO` and re-running.

**If measurement contradicts the spec, amend the spec before writing code.** A row moving from
"assert" to "serviced", or a layout offset moving at all, is a scope change to be decided in the
open, not worked around during implementation.

- [ ] **Step 1: Write the failing test**

Add to `crates/retrace-arch/src/lib.rs`'s existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn signal_of_esr_maps_the_fault_classes_by_dfsc() {
    // EC 0x24 = data abort from a lower EL. DFSC lives in ISS[5:0].
    // 0b0001LL (0x04..0x07) = translation fault  -> SEGV_MAPERR (nothing is mapped there)
    assert_eq!(signal_of_esr(0x9200_0006), (SIGSEGV, SEGV_MAPERR), "translation fault, level 2");
    assert_eq!(signal_of_esr(0x9200_0005), (SIGSEGV, SEGV_MAPERR), "translation fault, level 1");
    // 0b0011LL (0x0C..0x0F) = permission fault -> SEGV_ACCERR (mapped, but not like that)
    assert_eq!(signal_of_esr(0x9200_000f), (SIGSEGV, SEGV_ACCERR), "permission fault, level 3");
    // 0b0010LL (0x08..0x0B) = access-flag fault -> also an access error
    assert_eq!(signal_of_esr(0x9200_0009), (SIGSEGV, SEGV_ACCERR), "access flag fault");
    // 0x21 = alignment fault -> SIGBUS
    assert_eq!(signal_of_esr(0x9200_0021), (SIGBUS, BUS_ADRALN), "alignment fault");
    // 0x10..0x13 = synchronous external abort -> SIGBUS/BUS_OBJERR
    assert_eq!(signal_of_esr(0x9200_0010), (SIGBUS, BUS_OBJERR), "external abort");
}

#[test]
fn signal_of_esr_maps_instruction_aborts_the_same_way() {
    // EC 0x20 = instruction abort from a lower EL; same DFSC encoding.
    assert_eq!(signal_of_esr(0x8200_0006), (SIGSEGV, SEGV_MAPERR));
    assert_eq!(signal_of_esr(0x8200_000f), (SIGSEGV, SEGV_ACCERR));
}

#[test]
fn signal_of_esr_maps_the_non_abort_classes() {
    assert_eq!(signal_of_esr(0x9600_0000), (SIGBUS, BUS_ADRALN), "EC 0x26: SP alignment");
    assert_eq!(signal_of_esr(0x0000_0000), (SIGILL, ILL_ILLOPC), "EC 0x00: unknown/undefined");
    assert_eq!(signal_of_esr(0x3800_0000), (SIGILL, ILL_ILLOPC), "EC 0x0e: illegal execution state");
    assert_eq!(signal_of_esr(0xf000_0000), (SIGTRAP, TRAP_BRKPT), "EC 0x3c: BRK");
}

// The measured ESR from spikes/sigtramp.c, end to end. A store to an unmapped page.
#[test]
fn signal_of_esr_classifies_the_measured_probe_esr() {
    assert_eq!(signal_of_esr(0x9200_0046), (SIGSEGV, SEGV_MAPERR),
        "0x92000046 is what the host kernel put in the probe's mcontext: EC 0x24, WnR set, DFSC 0x06");
}

#[test]
fn signal_constants_match_the_sdk() {
    assert_eq!((SIGILL, SIGTRAP, SIGFPE, SIGBUS, SIGSEGV), (4, 5, 8, 10, 11));
    assert_eq!((SEGV_MAPERR, SEGV_ACCERR), (1, 2));
    assert_eq!((BUS_ADRALN, BUS_ADRERR, BUS_OBJERR), (1, 2, 3));
    assert_eq!((SA_ONSTACK, SA_RESTART, SA_RESETHAND, SA_NODEFER, SA_SIGINFO),
               (0x1, 0x2, 0x4, 0x10, 0x40));
    assert_eq!((SS_ONSTACK, SS_DISABLE), (0x1, 0x4));
    assert_eq!(UC_FLAVOR, 30, "measured in spikes/sigtramp.c as x1 on trampoline entry");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p retrace-arch -- --test-threads=1`
Expected: FAIL — `cannot find function signal_of_esr in this scope`, plus the missing constants.

- [ ] **Step 3: Write minimal implementation**

Add to `crates/retrace-arch/src/lib.rs`, after the M11-signals block:

```rust
// ---- M12-signal-delivery ---------------------------------------------------------------------
// Signal numbers and si_codes from sys/signal.h; SA_*/SS_* from the same header. Every value here
// was read out of the live SDK by spikes/sigabi.c, not from memory.
pub const SIGILL: u64 = 4;
pub const SIGTRAP: u64 = 5;
pub const SIGFPE: u64 = 8;
pub const SIGBUS: u64 = 10;
pub const SIGSEGV: u64 = 11;

pub const SEGV_MAPERR: u64 = 1;
pub const SEGV_ACCERR: u64 = 2;
pub const BUS_ADRALN: u64 = 1;
pub const BUS_ADRERR: u64 = 2;
pub const BUS_OBJERR: u64 = 3;
pub const ILL_ILLOPC: u64 = 1;
pub const TRAP_BRKPT: u64 = 1;

pub const SA_ONSTACK: u32 = 0x1;
pub const SA_RESTART: u32 = 0x2;
pub const SA_RESETHAND: u32 = 0x4;
pub const SA_NODEFER: u32 = 0x10;
pub const SA_SIGINFO: u32 = 0x40;

pub const SS_ONSTACK: u64 = 0x1;
pub const SS_DISABLE: u64 = 0x4;

/// The `infostyle` the kernel passes in `x1` on `sa_tramp` entry for an `SA_SIGINFO` handler.
/// Measured as `0x1e` by `spikes/sigtramp.c`; `UC_FLAVOR` is xnu's name for it.
pub const UC_FLAVOR: u64 = 30;

/// Classify a guest fault into the `(signal, si_code)` a real kernel would deliver.
///
/// Pure: a function of the ESR alone. The DFSC (`ISS[5:0]`) distinguishes "nothing is mapped there"
/// (translation fault → `SEGV_MAPERR`) from "mapped, but not for that" (permission / access-flag
/// fault → `SEGV_ACCERR`).
///
/// **A deliberate divergence from one host observation.** `spikes/sigtramp.c` recorded the host
/// delivering `SEGV_ACCERR` for a store to a wholly unmapped address, where the DFSC says
/// `MAPERR`. The host's answer reflects its own VM regime (a Mach protection failure on a submap
/// retrace does not reproduce); the guest's fault is described completely by its ESR, so the ESR is
/// what retrace derives from. Nothing in the gate set depends on the choice — libstd keys on
/// `si_addr` — which is exactly why it is made deliberately here rather than by accident.
pub fn signal_of_esr(esr: u64) -> (u64, u64) {
    let ec = (esr >> 26) & 0x3f;
    match ec {
        // Instruction / data abort from a lower EL: the guest touched something it could not.
        0x20 | 0x24 => match esr & 0x3f {
            0x04..=0x07 => (SIGSEGV, SEGV_MAPERR), // translation fault, levels 0..3
            0x08..=0x0b => (SIGSEGV, SEGV_ACCERR), // access-flag fault
            0x0c..=0x0f => (SIGSEGV, SEGV_ACCERR), // permission fault
            0x10..=0x13 => (SIGBUS, BUS_OBJERR),   // synchronous external abort
            0x21 => (SIGBUS, BUS_ADRALN),          // alignment fault
            _ => (SIGSEGV, SEGV_ACCERR),
        },
        0x26 => (SIGBUS, BUS_ADRALN),  // SP alignment fault
        0x00 | 0x0e => (SIGILL, ILL_ILLOPC), // unknown reason / illegal execution state
        0x3c => (SIGTRAP, TRAP_BRKPT), // BRK instruction
        _ => panic!(
            "signal_of_esr: EC {ec:#x} (esr={esr:#x}) has no modelled signal mapping. It reached \
             the fault path, so it is a real guest fault retrace cannot name — add the class here \
             deliberately rather than defaulting it to SIGSEGV, which would be a plausible lie."),
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p retrace-arch -- --test-threads=1`
Expected: PASS, all five new tests.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-arch/src/lib.rs
git commit -m "M12 t1: measure the ABI, then the fault-to-signal mapping

Step 0 findings (record the real answers here):
- crashy installs SIGSEGV/SIGBUS handler before faulting: <yes/no>
- _sigtramp reads beyond x3/x4: <what>
- infostyle without SA_SIGINFO: <value>

signal_of_esr derives (signal, si_code) from the ESR's DFSC. Deliberately
diverges from the host's observed SEGV_ACCERR for an unmapped address: the
DFSC says MAPERR, the guest's fault is fully described by its ESR, and
nothing in the gate set depends on the choice."
```

---

## Task 2: `SigAction.tramp` and the pure frame builder

**Files:**
- Modify: `crates/retrace-box/src/sig.rs` (add types + functions; extend `#[cfg(test)] mod tests`)
- Modify: `crates/retrace-box/src/lib.rs` (extend the `pub use sig::{...}` re-export at line 11)

**Interfaces:**
- Consumes: `retrace_arch::{SA_ONSTACK, SA_SIGINFO, SS_ONSTACK, UC_FLAVOR}` (Task 1).
- Produces:
  - `SigAction` gains `pub tramp: u64`
  - `pub struct ThreadState { pub x: [u64; 29], pub fp: u64, pub lr: u64, pub sp: u64, pub pc: u64, pub cpsr: u64 }`
  - `pub struct NeonState { pub v: [u128; 32], pub fpsr: u32, pub fpcr: u32 }`
  - `pub struct FrameInput { pub sig: u64, pub si_code: u64, pub si_addr: u64, pub esr: u64, pub far: u64, pub ts: ThreadState, pub ns: NeonState, pub mask: u32, pub act: SigAction, pub frame_base: u64 }`
  - `pub struct EntryRegs { pub x: [u64; 6], pub sp: u64, pub pc: u64 }`
  - `pub const FRAME_SIGINFO_OFF: usize = 0; FRAME_UCONTEXT_OFF: usize = 104; FRAME_MCONTEXT_OFF: usize = 160; FRAME_LEN: usize = 976; FRAME_SLACK: u64 = 128;`
  - `pub fn choose_frame_base(sp: u64, act: SigAction, altstack: Option<(u64,u64,u64)>, on_alt: bool) -> (u64, bool)`
  - `pub fn build_frame(inp: &FrameInput) -> (Vec<u8>, EntryRegs)`
  - `pub fn sigreturn_token(uctx_ipa: u64) -> u64`

**Background the implementer needs.** This is the riskiest part of the milestone and it is
deliberately VM-free so it can be tested in microseconds. Every offset below came from
`spikes/sigabi.c`. The single most important one: **`uc_mcontext` is a pointer** at ucontext offset
48 — the mcontext is a separate 816-byte block at frame offset 160, not inline.

`SigAction` gains `tramp` but `encode_oldact` must **not** grow. The output struct (`struct
sigaction`, 16 bytes) has no `sa_tramp`; only the input struct (`struct __sigaction`, 24 bytes) does.
M11 already has a golden test pinning the 16-byte width — do not touch it, and add the test that
`tramp` cannot leak into the writeback.

- [ ] **Step 1: Write the failing test**

Add to `crates/retrace-box/src/sig.rs`'s `#[cfg(test)] mod tests`:

```rust
fn probe_ts() -> ThreadState {
    let mut x = [0u64; 29];
    for (i, xi) in x.iter_mut().enumerate() { *xi = 0x1000 + i as u64; }
    ThreadState { x, fp: 0xf000, lr: 0x1_0000, sp: 0x7fff_0000, pc: 0x1_2340, cpsr: 0x6000_0000 }
}
fn probe_input(base: u64) -> FrameInput {
    FrameInput {
        sig: 11, si_code: 1, si_addr: 0xdead_0000, esr: 0x9200_0046, far: 0xdead_0000,
        ts: probe_ts(), ns: NeonState { v: [0; 32], fpsr: 0, fpcr: 0 }, mask: 0,
        act: SigAction { disp: Disposition::Handler(0xabc0), tramp: 0xdef0, mask: 0, flags: 0x40 },
        frame_base: base,
    }
}

#[test]
fn frame_offsets_match_the_measured_layout() {
    // spikes/sigabi.c: siginfo_t=104, ucontext_t=56 (uc_mcontext is a POINTER), mcontext64=816.
    assert_eq!((FRAME_SIGINFO_OFF, FRAME_UCONTEXT_OFF, FRAME_MCONTEXT_OFF), (0, 104, 160));
    assert_eq!(FRAME_LEN, 976, "104 + 56 + 816");
}

#[test]
fn build_frame_lays_out_siginfo_at_offset_zero() {
    let (bytes, _) = build_frame(&probe_input(0x7000_0000));
    assert_eq!(bytes.len(), FRAME_LEN);
    let si = &bytes[FRAME_SIGINFO_OFF..];
    assert_eq!(u32::from_le_bytes(si[0..4].try_into().unwrap()), 11, "si_signo at 0");
    assert_eq!(u32::from_le_bytes(si[8..12].try_into().unwrap()), 1, "si_code at 8");
    assert_eq!(u64::from_le_bytes(si[24..32].try_into().unwrap()), 0xdead_0000, "si_addr at 24");
}

#[test]
fn build_frame_points_uc_mcontext_at_the_separate_block() {
    let base = 0x7000_0000u64;
    let (bytes, _) = build_frame(&probe_input(base));
    let uc = &bytes[FRAME_UCONTEXT_OFF..];
    assert_eq!(u32::from_le_bytes(uc[0..4].try_into().unwrap()), 0, "uc_onstack at 0");
    assert_eq!(u32::from_le_bytes(uc[4..8].try_into().unwrap()), 0, "uc_sigmask at 4");
    assert_eq!(u64::from_le_bytes(uc[40..48].try_into().unwrap()), 816, "uc_mcsize at 40");
    assert_eq!(u64::from_le_bytes(uc[48..56].try_into().unwrap()),
               base + FRAME_MCONTEXT_OFF as u64,
               "uc_mcontext at 48 is a POINTER to the mcontext block, not the mcontext itself");
}

#[test]
fn build_frame_writes_the_exception_and_thread_state() {
    let base = 0x7000_0000u64;
    let (bytes, _) = build_frame(&probe_input(base));
    let mc = &bytes[FRAME_MCONTEXT_OFF..];
    // exception_state64 at mcontext+0: far(8) esr(4) exception(4)
    assert_eq!(u64::from_le_bytes(mc[0..8].try_into().unwrap()), 0xdead_0000, "__es.__far");
    assert_eq!(u32::from_le_bytes(mc[8..12].try_into().unwrap()), 0x9200_0046, "__es.__esr");
    assert_eq!(u32::from_le_bytes(mc[12..16].try_into().unwrap()), 0, "__es.__exception");
    // thread_state64 at mcontext+16: x[29] then fp,lr,sp,pc at 232,240,248,256 and cpsr at 264
    let ss = &mc[16..];
    assert_eq!(u64::from_le_bytes(ss[0..8].try_into().unwrap()), 0x1000, "__ss.__x[0]");
    assert_eq!(u64::from_le_bytes(ss[224..232].try_into().unwrap()), 0x1000 + 28, "__ss.__x[28]");
    assert_eq!(u64::from_le_bytes(ss[232..240].try_into().unwrap()), 0xf000, "__ss.__fp");
    assert_eq!(u64::from_le_bytes(ss[240..248].try_into().unwrap()), 0x1_0000, "__ss.__lr");
    assert_eq!(u64::from_le_bytes(ss[248..256].try_into().unwrap()), 0x7fff_0000, "__ss.__sp");
    assert_eq!(u64::from_le_bytes(ss[256..264].try_into().unwrap()), 0x1_2340, "__ss.__pc");
    assert_eq!(u32::from_le_bytes(ss[264..268].try_into().unwrap()), 0x6000_0000, "__ss.__cpsr");
}

#[test]
fn build_frame_writes_the_neon_block() {
    let base = 0x7000_0000u64;
    let mut inp = probe_input(base);
    inp.ns.v[8] = 0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00;
    inp.ns.fpsr = 0x1234;
    inp.ns.fpcr = 0x5678;
    let (bytes, _) = build_frame(&inp);
    // neon_state64 at mcontext+288: v[32] (16 bytes each) then fpsr(4) fpcr(4)
    let ns = &bytes[FRAME_MCONTEXT_OFF + 288..];
    assert_eq!(u128::from_le_bytes(ns[128..144].try_into().unwrap()),
               0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00, "v8 at neon+8*16");
    assert_eq!(u32::from_le_bytes(ns[512..516].try_into().unwrap()), 0x1234, "fpsr");
    assert_eq!(u32::from_le_bytes(ns[516..520].try_into().unwrap()), 0x5678, "fpcr");
}

// THE entry-contract test. Measured in spikes/sigtramp.c: sp IS the siginfo pointer.
#[test]
fn build_frame_returns_the_measured_entry_registers() {
    let base = 0x7000_0000u64;
    let (_, regs) = build_frame(&probe_input(base));
    assert_eq!(regs.x[0], 0xabc0, "x0 = the catcher (handler VA)");
    assert_eq!(regs.x[1], 30, "x1 = infostyle UC_FLAVOR");
    assert_eq!(regs.x[2], 11, "x2 = the signal number");
    assert_eq!(regs.x[3], base, "x3 = siginfo*, which is the frame base");
    assert_eq!(regs.x[4], base + FRAME_UCONTEXT_OFF as u64, "x4 = ucontext*");
    assert_eq!(regs.x[5], sigreturn_token(base + FRAME_UCONTEXT_OFF as u64), "x5 = the token");
    assert_eq!(regs.sp, base, "sp == x3: the frame base IS sp");
    assert_eq!(regs.pc, 0xdef0, "pc = sa_tramp, NOT the handler — the kernel enters the trampoline");
}

#[test]
fn choose_frame_base_uses_the_current_stack_by_default() {
    let act = SigAction { disp: Disposition::Handler(1), tramp: 2, mask: 0, flags: 0 };
    let (base, on_alt) = choose_frame_base(0x7fff_1000, act, None, false);
    // 976-byte frame + 128 bytes of measured slack below the pre-signal sp, 16-byte aligned.
    assert_eq!(base, 0x7fff_1000 - 128 - 976);
    assert_eq!(base % 16, 0, "arm64 requires a 16-byte aligned sp");
    assert!(!on_alt);
}

#[test]
fn choose_frame_base_honours_sa_onstack_when_an_altstack_is_installed() {
    let act = SigAction { disp: Disposition::Handler(1), tramp: 2, mask: 0, flags: SA_ONSTACK };
    let (base, on_alt) = choose_frame_base(0x7fff_1000, act, Some((0x9_0000, 0x4000, 0)), false);
    assert!(on_alt, "SA_ONSTACK + an installed alt stack means run on it");
    assert!(base >= 0x9_0000 && base < 0x9_0000 + 0x4000,
        "the frame must sit INSIDE the alt stack [{:#x}, {:#x})", 0x9_0000, 0x9_0000 + 0x4000);
    assert_eq!(base, (0x9_0000 + 0x4000 - FRAME_LEN as u64) & !15);
}

#[test]
fn choose_frame_base_does_not_re_enter_an_altstack_it_is_already_on() {
    let act = SigAction { disp: Disposition::Handler(1), tramp: 2, mask: 0, flags: SA_ONSTACK };
    let sp_on_alt = 0x9_2000;
    let (base, on_alt) = choose_frame_base(sp_on_alt, act, Some((0x9_0000, 0x4000, 0)), true);
    assert!(on_alt, "still on the alt stack");
    assert_eq!(base, sp_on_alt - 128 - 976,
        "already on it: keep growing DOWN from the current sp, do not reset to its top and \
         clobber the frame the outer handler is running on");
}

#[test]
fn choose_frame_base_ignores_sa_onstack_with_no_altstack_installed() {
    let act = SigAction { disp: Disposition::Handler(1), tramp: 2, mask: 0, flags: SA_ONSTACK };
    let (base, on_alt) = choose_frame_base(0x7fff_1000, act, None, false);
    assert_eq!(base, 0x7fff_1000 - 128 - 976);
    assert!(!on_alt);
}

#[test]
fn sigreturn_token_is_deterministic_and_address_dependent() {
    assert_eq!(sigreturn_token(0x7000_0068), sigreturn_token(0x7000_0068),
        "a CONSTANT, unlike the host's process-randomized token: spikes/sigtramp.c returned a \
         different value on every run, which is exactly what must not enter a recording");
    assert_ne!(sigreturn_token(0x7000_0068), sigreturn_token(0x7000_0078));
}

#[test]
fn decode_act_now_captures_sa_tramp() {
    let mut b = [0u8; 24];
    b[0..8].copy_from_slice(&0xdead_0000u64.to_le_bytes());
    b[8..16].copy_from_slice(&0xbeef_0000u64.to_le_bytes()); // sa_tramp — M11 discarded it
    let a = decode_act(&b);
    assert_eq!(a.disp, Disposition::Handler(0xdead_0000));
    assert_eq!(a.tramp, 0xbeef_0000, "M12 needs it: the kernel enters the TRAMPOLINE, not the handler");
}

// The counterpart to M11's width test: capturing tramp must not widen the writeback.
#[test]
fn encode_oldact_still_omits_sa_tramp() {
    let out = encode_oldact(SigAction {
        disp: Disposition::Handler(0xdead_0000), tramp: 0xbeef_0000, mask: 0xff, flags: 0x42 });
    assert_eq!(out.len(), 16, "struct sigaction is 16 bytes and has NO sa_tramp");
    assert_eq!(u32::from_le_bytes(out[8..12].try_into().unwrap()), 0xff,
        "sa_mask sits at offset 8 — if tramp leaked in here it would land at 8 and corrupt it");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p retrace-box --lib -- --test-threads=1`
Expected: FAIL — `cannot find type FrameInput`, `no field tramp on SigAction`, and the existing
`SigAction` literals in M11's tests fail to compile for the missing field.

- [ ] **Step 3: Write minimal implementation**

In `crates/retrace-box/src/sig.rs`, add `tramp` to `SigAction` (and its `Default`), set it in
`decode_act` from offset 8, and add:

```rust
use retrace_arch::{SA_ONSTACK, SA_SIGINFO, UC_FLAVOR};

/// Frame geometry, measured by `spikes/sigabi.c` against the live SDK. The frame is ONE block at
/// the new `sp`; `spikes/sigtramp.c` measured `sp == x3`, i.e. siginfo sits at offset 0.
pub const FRAME_SIGINFO_OFF: usize = 0;
pub const FRAME_UCONTEXT_OFF: usize = 104; // == sizeof(siginfo_t)
pub const FRAME_MCONTEXT_OFF: usize = 160; // == 104 + sizeof(ucontext_t), and uc_mcontext points here
pub const FRAME_LEN: usize = 976;          // 104 + 56 + 816
/// The kernel left 128 bytes between the frame top and the pre-signal `sp` (measured: old sp
/// 0x16b9c6730, frame base 0x16b9c62e0, frame 976). Reproduced rather than explained.
pub const FRAME_SLACK: u64 = 128;

const MCONTEXT_LEN: u64 = 816;
const TS_OFF: usize = 16;   // thread_state64 within mcontext64
const NS_OFF: usize = 288;  // neon_state64 within mcontext64

/// Fixed key for the `sigreturn` token. The host randomizes its equivalent per process — measured,
/// two runs of `spikes/sigtramp.c` returned different values — so retrace, which synthesizes the
/// whole frame, must own it as a CONSTANT. Same posture as the fixed PAC keys.
const SIGRETURN_TOKEN_KEY: u64 = 0x5265_7472_6163_6512;

pub fn sigreturn_token(uctx_ipa: u64) -> u64 {
    SIGRETURN_TOKEN_KEY ^ uctx_ipa.rotate_left(17)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadState {
    pub x: [u64; 29], pub fp: u64, pub lr: u64, pub sp: u64, pub pc: u64, pub cpsr: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NeonState { pub v: [u128; 32], pub fpsr: u32, pub fpcr: u32 }

#[derive(Debug, Clone, Copy)]
pub struct FrameInput {
    pub sig: u64, pub si_code: u64, pub si_addr: u64,
    pub esr: u64, pub far: u64,
    pub ts: ThreadState, pub ns: NeonState,
    pub mask: u32,
    pub act: SigAction,
    pub frame_base: u64,
}

/// What the vCPU must be set to on entry. `pc` is the TRAMPOLINE, never the handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryRegs { pub x: [u64; 6], pub sp: u64, pub pc: u64 }

/// Where the frame goes, and whether the handler runs on the alternate stack.
///
/// `on_alt` in means "the guest is ALREADY running on its alt stack" — in that case the frame grows
/// down from the current `sp` instead of resetting to the alt stack's top, which would clobber the
/// frame an outer handler is still using.
pub fn choose_frame_base(
    sp: u64, act: SigAction, altstack: Option<(u64, u64, u64)>, on_alt: bool,
) -> (u64, bool) {
    let wants_alt = act.flags & SA_ONSTACK != 0;
    match (wants_alt, altstack, on_alt) {
        (true, Some((ss_sp, ss_size, _)), false) => (((ss_sp + ss_size) - FRAME_LEN as u64) & !15, true),
        (true, Some(_), true) => ((sp - FRAME_SLACK - FRAME_LEN as u64) & !15, true),
        _ => ((sp - FRAME_SLACK - FRAME_LEN as u64) & !15, false),
    }
}

/// Lay out the signal frame. Pure: no VM, no `Box_`, no I/O — every byte is a function of the
/// inputs, which is what makes record and replay produce identical frames and what lets the whole
/// layout be tested in microseconds.
pub fn build_frame(inp: &FrameInput) -> (Vec<u8>, EntryRegs) {
    let mut f = vec![0u8; FRAME_LEN];
    let base = inp.frame_base;
    let uc = base + FRAME_UCONTEXT_OFF as u64;
    let mc = base + FRAME_MCONTEXT_OFF as u64;

    // --- siginfo_t at +0 ---
    let si = FRAME_SIGINFO_OFF;
    f[si..si + 4].copy_from_slice(&(inp.sig as u32).to_le_bytes());          // si_signo
    f[si + 8..si + 12].copy_from_slice(&(inp.si_code as u32).to_le_bytes()); // si_code
    f[si + 24..si + 32].copy_from_slice(&inp.si_addr.to_le_bytes());         // si_addr

    // --- ucontext_t at +104. uc_mcontext is a POINTER; the mcontext is the block at +160. ---
    let u = FRAME_UCONTEXT_OFF;
    f[u..u + 4].copy_from_slice(&0u32.to_le_bytes());                        // uc_onstack
    f[u + 4..u + 8].copy_from_slice(&inp.mask.to_le_bytes());                // uc_sigmask
    f[u + 40..u + 48].copy_from_slice(&MCONTEXT_LEN.to_le_bytes());          // uc_mcsize
    f[u + 48..u + 56].copy_from_slice(&mc.to_le_bytes());                    // uc_mcontext

    // --- mcontext64 at +160: exception(16) | thread(272) | neon(528) ---
    let m = FRAME_MCONTEXT_OFF;
    f[m..m + 8].copy_from_slice(&inp.far.to_le_bytes());                     // __es.__far
    f[m + 8..m + 12].copy_from_slice(&(inp.esr as u32).to_le_bytes());       // __es.__esr

    let t = m + TS_OFF;
    for (i, xi) in inp.ts.x.iter().enumerate() {
        f[t + i * 8..t + i * 8 + 8].copy_from_slice(&xi.to_le_bytes());
    }
    f[t + 232..t + 240].copy_from_slice(&inp.ts.fp.to_le_bytes());
    f[t + 240..t + 248].copy_from_slice(&inp.ts.lr.to_le_bytes());
    f[t + 248..t + 256].copy_from_slice(&inp.ts.sp.to_le_bytes());
    f[t + 256..t + 264].copy_from_slice(&inp.ts.pc.to_le_bytes());
    f[t + 264..t + 268].copy_from_slice(&(inp.ts.cpsr as u32).to_le_bytes());

    let n = m + NS_OFF;
    for (i, vi) in inp.ns.v.iter().enumerate() {
        f[n + i * 16..n + i * 16 + 16].copy_from_slice(&vi.to_le_bytes());
    }
    f[n + 512..n + 516].copy_from_slice(&inp.ns.fpsr.to_le_bytes());
    f[n + 516..n + 520].copy_from_slice(&inp.ns.fpcr.to_le_bytes());

    let catcher = match inp.act.disp {
        Disposition::Handler(va) => va,
        other => panic!(
            "build_frame called for disposition {other:?} — only Handler has anything to deliver \
             to. The caller's disposition check is wrong."),
    };
    // infostyle: measured 0x1e (UC_FLAVOR) for an SA_SIGINFO handler. Task 1 Step 0 measures the
    // non-SA_SIGINFO value; until then a non-SA_SIGINFO handler is not modelled.
    assert!(inp.act.flags & SA_SIGINFO != 0,
        "a non-SA_SIGINFO handler's infostyle is unmeasured (spec R3). Measure it with \
         spikes/sigtramp.c before delivering to one, rather than guessing UC_FLAVOR.");
    let regs = EntryRegs {
        x: [catcher, UC_FLAVOR, inp.sig, base, uc, sigreturn_token(uc)],
        sp: base,
        pc: inp.act.tramp,
    };
    (f, regs)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p retrace-box --lib -- --test-threads=1`
Expected: PASS. If M11's existing `SigAction { .. }` literals fail to compile, add `tramp: 0` to
each — do not add `..Default::default()`, which would hide a future missing field.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/sig.rs crates/retrace-box/src/lib.rs
git commit -m "M12 t2: the signal frame, laid out by a pure function

Every offset from spikes/sigabi.c. The load-bearing one: uc_mcontext is a
POINTER at ucontext+48, so the 816-byte mcontext is a separate block at frame
offset 160 — a flat-struct design would be wrong by 816 bytes.

SigAction gains tramp (offset 8 of the 24-byte input struct, which M11
discarded). encode_oldact is unchanged and a new test pins that tramp cannot
leak into the 16-byte writeback, where it would land on sa_mask.

The sigreturn token is a fixed constant folded with the ucontext address: the
host randomizes its equivalent per process (two spike runs, two values), and
retrace synthesizes the whole frame, so it owns the token."
```

---

## Task 3: `Event::SignalDelivery` and the format bump

**Files:**
- Modify: `crates/retrace-trace/src/lib.rs` (the `Event` enum at line 14; `TRACE_MAGIC` at line 29)
- Test: `crates/retrace-trace/src/lib.rs`'s `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `Region` (existing).
- Produces: `Event::SignalDelivery { sig: u64, si_code: u64, si_addr: u64, handler: u64, resume_pc: u64, writes: Vec<Region> }`; `TRACE_MAGIC == *b"RT\x00\x06"`.

**Background the implementer needs.** `Event`'s shape is a cross-crate contract; changing it is a
format break, so `TRACE_MAGIC` must bump in the same commit. No fixture trace is checked into the
repo, so nothing is invalidated — verify that with `ls crates/retrace/tests/fixtures/` (it holds
`rung3.json`, a jq input, not a trace).

`resume_pc` is the pc the guest returns to, which for a fault is **the faulting instruction itself**
(it re-executes). That is not decoration: the headline gate asserts on it to prove the store was
re-executed rather than skipped.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn signal_delivery_round_trips_through_the_writer_and_reader() {
    let dir = std::env::temp_dir().join(format!("m12-trace-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("t.bin");
    let ev = Event::SignalDelivery {
        sig: 11, si_code: 1, si_addr: 0xdead_0000, handler: 0x1_0000, resume_pc: 0x2_0000,
        writes: vec![Region { ipa: 0x7000_0000, bytes: vec![1, 2, 3, 4] }],
    };
    { let mut w = Writer::create(&p).unwrap(); w.append(&ev).unwrap(); }
    let (evs, torn) = Reader::open_checked(&p).unwrap();
    assert!(!torn);
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0], ev);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn magic_bumped_for_the_signal_delivery_variant() {
    assert_eq!(TRACE_MAGIC, *b"RT\x00\x06",
        "adding an Event variant is a format break: the version must bump with it");
}

#[test]
fn a_trace_written_with_the_old_magic_is_rejected_whole() {
    let dir = std::env::temp_dir().join(format!("m12-oldmagic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("old.bin");
    std::fs::write(&p, b"RT\x00\x05rest-of-a-v5-trace").unwrap();
    let (evs, rejected) = Reader::open_checked(&p).unwrap();
    assert!(evs.is_empty() && rejected, "a v5 trace must be rejected, not half-read");
    std::fs::remove_dir_all(&dir).ok();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p retrace-trace -- --test-threads=1`
Expected: FAIL — `no variant named SignalDelivery`, and the magic assertion fails with `RT\x00\x05`.

- [ ] **Step 3: Write minimal implementation**

```rust
    /// M12: control transferred to one of the guest's own signal handlers.
    ///
    /// NOT terminal — the guest keeps running inside the handler, and a later `sigreturn`(184)
    /// syscall event brings it back. One shape for BOTH causes (a fault, and a self-raise via
    /// kill/__pthread_kill) so there is one seek target, one debug line, and one replay mirror.
    ///
    /// Deliberately a trace event rather than emulation hidden inside `Box_::run()`: symmetry rule
    /// 2's precedents (the timebase MRS, the undef-MRS, the FPAC strip) are INSTRUCTION emulations —
    /// micro, high-frequency, semantically invisible. Entering a handler is a control transfer, and
    /// "rewind to where the signal was delivered" is a query a reverse debugger's users have.
    ///
    /// `writes` carries the frame bytes; replay recomputes them and byte-compares before applying,
    /// the same posture as M11's `sigaction` oldact writeback. `resume_pc` is where the guest
    /// resumes on `sigreturn` — for a fault, the faulting instruction itself, which re-executes.
    SignalDelivery {
        sig: u64, si_code: u64, si_addr: u64, handler: u64, resume_pc: u64, writes: Vec<Region>,
    },
```

and

```rust
pub const TRACE_MAGIC: [u8;4] = *b"RT\x00\x06"; // "RT" + version 0x0006 (M12: Event::SignalDelivery)
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p retrace-trace -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-trace/src/lib.rs
git commit -m "M12 t3: Event::SignalDelivery, and TRACE_MAGIC 0x0005 -> 0x0006

One event shape for both causes — a fault and a self-raise — so there is one
seek target, one debug line, and one mirror. A first-class landmark rather
than emulation hidden in Box_::run(): rule 2's precedents are instruction
emulations, and entering a handler is a control transfer users rewind to.

No fixture trace is checked in, so the format break invalidates nothing."
```

---

## Task 4: `Box_::deliver_signal`

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (new methods near `apply_and_return` at line 2353)
- Test: `crates/retrace-box/tests/deliver.rs` (create)

**Interfaces:**
- Consumes: `sig::{build_frame, choose_frame_base, FrameInput, ThreadState, NeonState, EntryRegs, FRAME_UCONTEXT_OFF}` (Task 2); `retrace_arch::{SA_NODEFER, SA_RESETHAND}` (Task 1).
- Produces:
  - `pub fn deliver_signal(&mut self, sig: u64, si_code: u64, si_addr: u64, esr: u64, far: u64) -> (Vec<Region>, u64)` — returns `(frame writes, resume_pc)`.
  - `pub fn on_altstack(&self) -> bool`
  - `fn write_guest(&mut self, ipa: u64, bytes: &[u8])` (private)

**Background the implementer needs.** `apply_and_return` (line 2353) already contains the
host-span-and-copy write path plus the M5 watchpoint intersection check. Factor that copy into a
private `write_guest` and call it from both — do **not** duplicate it, and do **not** route the frame
through `apply_and_return`, which would also set `x0` and return from a syscall the fault path never
made.

Register sourcing: `x0..x28` via `reg::x(n)`, `fp`/`lr` via `reg::FP`/`reg::LR`, **`sp` via
`sysreg::SP_EL0`** (the guest runs at EL0, so the vCPU's `SP` is not it), `pc` via
`sysreg::ELR_EL1` — the same source `position()` uses at line 2514, because the vCPU's live PC is
parked in the trampoline. `cpsr` via `sysreg::SPSR_EL1`. Vector state via `get_simd(simd::q(i))` and
`reg::FPCR`/`reg::FPSR`, exactly as `capture()` does at line 2592.

Resuming into the handler is the mirror of `set_x0_err_and_return`: set `ELR_EL1` to the trampoline
and `SP_EL0` to the frame base, not `reg::PC`.

- [ ] **Step 1: Write the failing test**

Create `crates/retrace-box/tests/deliver.rs`:

```rust
// M12: Box_::deliver_signal builds the frame in guest memory and enters the trampoline.
use retrace_box::{Box_, Disposition, SigAction, FRAME_LEN, FRAME_UCONTEXT_OFF, sigreturn_token};
use retrace_trace::Regs;

/// A minimal box with one RW data page and a known stack, MMU off (identity IPA).
fn boxed() -> Box_ {
    // hello.s is the smallest static guest; restore() gives a box without running it.
    let regs = Regs { x: [0; 31], pc: 0x1_0000, sp_el0: 0x2_0000, cpsr: 0 };
    Box_::restore(&[retrace_trace::Region { ipa: 0x1_0000, bytes: vec![0u8; 0x4000] }], &regs)
}

#[test]
fn deliver_signal_writes_the_frame_and_enters_the_trampoline() {
    let mut b = boxed();
    b.sigtable_mut().set_action(11, SigAction {
        disp: Disposition::Handler(0xabc0), tramp: 0x1_0100, mask: 0, flags: retrace_arch::SA_SIGINFO,
    });
    let sp_before = b.regs_snapshot().sp_el0;
    let (writes, resume_pc) = b.deliver_signal(11, 1, 0xdead_0000, 0x9200_0046, 0xdead_0000);

    assert_eq!(writes.len(), 1, "the frame is one contiguous write");
    assert_eq!(writes[0].bytes.len(), FRAME_LEN);
    let base = writes[0].ipa;
    assert_eq!(base % 16, 0, "arm64 sp must be 16-byte aligned");
    assert_eq!(base, sp_before - 128 - FRAME_LEN as u64);

    let r = b.regs_snapshot();
    assert_eq!(r.sp_el0, base, "sp IS the frame base (measured: sp == x3)");
    assert_eq!(r.pc, 0x1_0100, "entered the TRAMPOLINE, not the handler");
    assert_eq!(r.x[0], 0xabc0, "x0 = the catcher");
    assert_eq!(r.x[1], 30);
    assert_eq!(r.x[2], 11);
    assert_eq!(r.x[3], base);
    assert_eq!(r.x[4], base + FRAME_UCONTEXT_OFF as u64);
    assert_eq!(r.x[5], sigreturn_token(base + FRAME_UCONTEXT_OFF as u64));
    assert_eq!(resume_pc, 0x1_0000, "a fault resumes at the FAULTING instruction — it re-executes");

    // The bytes really landed in guest memory, not just in the returned Vec.
    assert_eq!(b.read_guest(base, FRAME_LEN), writes[0].bytes);
}

#[test]
fn deliver_signal_blocks_the_signal_and_its_sa_mask_for_the_handler() {
    let mut b = boxed();
    b.sigtable_mut().set_action(11, SigAction {
        disp: Disposition::Handler(0xabc0), tramp: 0x1_0100,
        mask: 1 << 5 /* SIGABRT */, flags: retrace_arch::SA_SIGINFO,
    });
    b.deliver_signal(11, 1, 0, 0, 0);
    assert!(b.sigtable().is_blocked(11), "the delivered signal blocks itself");
    assert!(b.sigtable().is_blocked(6), "and everything in sa_mask");
}

#[test]
fn deliver_signal_honours_sa_nodefer() {
    let mut b = boxed();
    b.sigtable_mut().set_action(11, SigAction {
        disp: Disposition::Handler(0xabc0), tramp: 0x1_0100, mask: 0,
        flags: retrace_arch::SA_SIGINFO | retrace_arch::SA_NODEFER,
    });
    b.deliver_signal(11, 1, 0, 0, 0);
    assert!(!b.sigtable().is_blocked(11), "SA_NODEFER means do not block the signal itself");
}

#[test]
fn deliver_signal_honours_sa_resethand() {
    let mut b = boxed();
    b.sigtable_mut().set_action(11, SigAction {
        disp: Disposition::Handler(0xabc0), tramp: 0x1_0100, mask: 0,
        flags: retrace_arch::SA_SIGINFO | retrace_arch::SA_RESETHAND,
    });
    b.deliver_signal(11, 1, 0, 0, 0);
    assert_eq!(b.sigtable().action(11).disp, Disposition::Dfl,
        "SA_RESETHAND resets to SIG_DFL as the handler is entered");
}

#[test]
fn the_frame_records_the_pre_signal_mask_not_the_handler_mask() {
    let mut b = boxed();
    b.sigtable_mut().set_mask(retrace_arch::SIG_SETMASK, 0b1010);
    b.sigtable_mut().set_action(11, SigAction {
        disp: Disposition::Handler(0xabc0), tramp: 0x1_0100, mask: 0, flags: retrace_arch::SA_SIGINFO,
    });
    let (writes, _) = b.deliver_signal(11, 1, 0, 0, 0);
    let uc = &writes[0].bytes[FRAME_UCONTEXT_OFF..];
    assert_eq!(u32::from_le_bytes(uc[4..8].try_into().unwrap()), 0b1010,
        "uc_sigmask is what sigreturn restores — it must be the mask from BEFORE delivery, or the \
         handler's own blocking becomes permanent");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p retrace-box --test deliver -- --test-threads=1`
Expected: FAIL — `no method named deliver_signal`.

- [ ] **Step 3: Write minimal implementation**

Factor the copy out of `apply_and_return` and add:

```rust
    /// Copy `bytes` into guest memory at `ipa`. The write path `apply_and_return` uses, minus the
    /// syscall return — a delivered signal is not a syscall and must not set `x0`.
    fn write_guest(&mut self, ipa: u64, bytes: &[u8]) {
        let (hp, avail) = self.host_span(ipa)
            .unwrap_or_else(|| panic!("write_guest: ipa {ipa:#x} outside any mapped region"));
        assert!(bytes.len() <= avail,
            "write_guest at {ipa:#x} ({} bytes) overruns backing ({avail} avail)", bytes.len());
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), hp, bytes.len()); }
    }

    /// Is the guest currently executing on its alternate signal stack?
    pub fn on_altstack(&self) -> bool {
        match self.sigtable.altstack() {
            Some((sp, size, _)) => {
                let cur = self.vcpu.get_sys(sysreg::SP_EL0).unwrap();
                cur >= sp && cur < sp + size
            }
            None => false,
        }
    }

    /// Enter the guest's handler for `sig`: build the frame, write it, set the entry registers.
    ///
    /// Returns `(frame writes, resume_pc)`. Called by BOTH record and replay — that is what makes
    /// "both sides recompute the same frame" true by construction rather than by discipline.
    ///
    /// `esr`/`far` are the guest's own fault syndrome for a fault-derived signal, and 0 for a
    /// self-raise (no hardware fault happened, and inventing one would be the same lie M11 refused
    /// when it kept `Event::Signal` out of `Event::Crash`).
    pub fn deliver_signal(
        &mut self, sig: u64, si_code: u64, si_addr: u64, esr: u64, far: u64,
    ) -> (Vec<Region>, u64) {
        let act = self.sigtable.action(sig);
        let mut x = [0u64; 29];
        for (i, xi) in x.iter_mut().enumerate() { *xi = self.vcpu.get_reg(reg::x(i as u32)).unwrap(); }
        let ts = ThreadState {
            x,
            fp: self.vcpu.get_reg(reg::FP).unwrap(),
            lr: self.vcpu.get_reg(reg::LR).unwrap(),
            // The guest runs at EL0: its stack pointer is SP_EL0, and its pc is ELR_EL1 (the vCPU's
            // live PC is parked in the trampoline) — the same sources `position()` uses.
            sp: self.vcpu.get_sys(sysreg::SP_EL0).unwrap(),
            pc: self.vcpu.get_sys(sysreg::ELR_EL1).unwrap(),
            cpsr: self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap(),
        };
        let mut v = [0u128; 32];
        for (i, vi) in v.iter_mut().enumerate() { *vi = self.vcpu.get_simd(simd::q(i as u32)).unwrap(); }
        let ns = NeonState {
            v,
            fpsr: self.vcpu.get_reg(reg::FPSR).unwrap() as u32,
            fpcr: self.vcpu.get_reg(reg::FPCR).unwrap() as u32,
        };

        let (frame_base, _on_alt) =
            choose_frame_base(ts.sp, act, self.sigtable.altstack(), self.on_altstack());
        let inp = FrameInput {
            sig, si_code, si_addr, esr, far, ts, ns,
            mask: self.sigtable.mask(),   // the PRE-signal mask: what sigreturn restores
            act, frame_base,
        };
        let (bytes, entry) = build_frame(&inp);
        self.write_guest(frame_base, &bytes);

        // Block the signal for the handler's duration, unless SA_NODEFER.
        let mut newmask = self.sigtable.mask() | act.mask;
        if act.flags & retrace_arch::SA_NODEFER == 0 { newmask |= 1 << (sig - 1); }
        self.sigtable.set_mask(retrace_arch::SIG_SETMASK, newmask);
        if act.flags & retrace_arch::SA_RESETHAND != 0 {
            self.sigtable.set_action(sig, SigAction { disp: Disposition::Dfl, ..act });
        }

        for (i, xi) in entry.x.iter().enumerate() {
            self.vcpu.set_reg(reg::x(i as u32), *xi).unwrap();
        }
        self.vcpu.set_sys(sysreg::SP_EL0, entry.sp).unwrap();
        self.vcpu.set_sys(sysreg::ELR_EL1, entry.pc).unwrap();

        (vec![Region { ipa: frame_base, bytes }], ts.pc)
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p retrace-box --test deliver -- --test-threads=1`
Expected: PASS, all six.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/deliver.rs
git commit -m "M12 t4: Box_::deliver_signal enters the guest's handler

One method called by both record and replay, so 'both sides recompute the same
frame' is true by construction. Reads the guest's state from where the guest
actually keeps it: SP_EL0 for the stack, ELR_EL1 for the pc (the vCPU's live
PC is parked in the trampoline), SPSR_EL1 for cpsr, get_simd for the vectors.

uc_sigmask carries the PRE-delivery mask — that is what sigreturn restores, so
recording the post-block mask would make the handler's own blocking permanent."
```

---

## Task 5: `Box_::sigreturn_restore`

**Files:**
- Modify: `crates/retrace-box/src/lib.rs`
- Test: `crates/retrace-box/tests/deliver.rs` (extend)

**Interfaces:**
- Consumes: Task 4's `deliver_signal`; `sig::{sigreturn_token, FRAME_MCONTEXT_OFF}`.
- Produces: `pub fn sigreturn_restore(&mut self, uctx_ipa: u64, token: u64)`; `pub const PSTATE_USER_MASK: u64`.

**Background the implementer needs.** This is the only place in M12 where **guest-writable bytes
reach a system register**. The frame lives on the guest's own stack, so a guest can rewrite `__cpsr`
before calling `sigreturn`. Restoring it verbatim into `SPSR_EL1` would let the guest ask for EL1.
Mask it. `PSTATE_USER_MASK` keeps NZCV (bits 31:28) and nothing that selects an exception level.

The round trip is the real test: deliver, clobber every register, `sigreturn_restore`, and assert
the pre-signal state came back — including the vector registers, which is what makes a *returning*
handler safe.

- [ ] **Step 1: Write the failing test**

Append to `crates/retrace-box/tests/deliver.rs`:

```rust
#[test]
fn sigreturn_restores_the_pre_signal_state_including_vectors() {
    let mut b = boxed();
    b.vcpu_set_x(7, 0xcafe_f00d);
    b.vcpu_set_q(8, 0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00);
    b.sigtable_mut().set_mask(retrace_arch::SIG_SETMASK, 0b0110);
    b.sigtable_mut().set_action(11, SigAction {
        disp: Disposition::Handler(0xabc0), tramp: 0x1_0100, mask: 0, flags: retrace_arch::SA_SIGINFO,
    });
    let sp_before = b.regs_snapshot().sp_el0;
    let (writes, _) = b.deliver_signal(11, 1, 0, 0, 0);
    let uctx = writes[0].ipa + FRAME_UCONTEXT_OFF as u64;

    // The handler runs and clobbers everything it is allowed to.
    b.vcpu_set_x(7, 0xdead_beef);
    b.vcpu_set_q(8, 0);
    assert!(b.sigtable().is_blocked(11));

    b.sigreturn_restore(uctx, sigreturn_token(uctx));

    let r = b.regs_snapshot();
    assert_eq!(r.x[7], 0xcafe_f00d, "x7 restored");
    assert_eq!(r.sp_el0, sp_before, "sp restored");
    assert_eq!(r.pc, 0x1_0000, "pc restored to the pre-signal instruction");
    assert_eq!(b.vcpu_get_q(8), 0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00,
        "VECTOR state restored — a handler is ordinary compiled code and will use NEON; without \
         this a handler that RETURNS silently corrupts the guest");
    assert_eq!(b.sigtable().mask(), 0b0110, "the pre-signal mask is restored from uc_sigmask");
}

#[test]
#[should_panic(expected = "sigreturn token mismatch")]
fn sigreturn_rejects_a_bad_token() {
    let mut b = boxed();
    b.sigtable_mut().set_action(11, SigAction {
        disp: Disposition::Handler(0xabc0), tramp: 0x1_0100, mask: 0, flags: retrace_arch::SA_SIGINFO,
    });
    let (writes, _) = b.deliver_signal(11, 1, 0, 0, 0);
    let uctx = writes[0].ipa + FRAME_UCONTEXT_OFF as u64;
    b.sigreturn_restore(uctx, 0);
}

// The security-shaped one: the frame is on the GUEST's stack, so the guest can rewrite __cpsr.
#[test]
fn sigreturn_sanitizes_pstate_and_cannot_be_asked_for_el1() {
    let mut b = boxed();
    b.sigtable_mut().set_action(11, SigAction {
        disp: Disposition::Handler(0xabc0), tramp: 0x1_0100, mask: 0, flags: retrace_arch::SA_SIGINFO,
    });
    let (writes, _) = b.deliver_signal(11, 1, 0, 0, 0);
    let base = writes[0].ipa;
    let uctx = base + FRAME_UCONTEXT_OFF as u64;

    // Rewrite __ss.__cpsr in guest memory the way a hostile guest would: ask for EL1h with
    // interrupts masked, plus a legitimate NZCV.
    let cpsr_ipa = base + FRAME_MCONTEXT_OFF as u64 + 16 + 264;
    b.poke_guest(cpsr_ipa, &0x8000_03c5u32.to_le_bytes());

    b.sigreturn_restore(uctx, sigreturn_token(uctx));
    let spsr = b.regs_snapshot().cpsr;
    assert_eq!(spsr & !retrace_box::PSTATE_USER_MASK, 0,
        "only user-settable bits may survive: the guest must not be able to select an exception \
         level by writing its own signal frame");
    assert_eq!(spsr & 0x8000_0000, 0x8000_0000, "the legitimate N flag still round-trips");
}
```

Add the three tiny test accessors to `Box_` alongside the existing `regs()`:

```rust
    /// Test/diagnostic accessors for vector and general registers. Present so the M12 delivery
    /// tests can clobber and observe state without a running guest.
    pub fn vcpu_set_x(&mut self, n: u32, v: u64) { self.vcpu.set_reg(reg::x(n), v).unwrap(); }
    pub fn vcpu_set_q(&mut self, n: u32, v: u128) { self.vcpu.set_simd(simd::q(n), v).unwrap(); }
    pub fn vcpu_get_q(&self, n: u32) -> u128 { self.vcpu.get_simd(simd::q(n)).unwrap() }
    /// Write raw bytes into guest memory (tests only — the production path is `write_guest`).
    pub fn poke_guest(&mut self, ipa: u64, bytes: &[u8]) { self.write_guest(ipa, bytes) }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p retrace-box --test deliver -- --test-threads=1`
Expected: FAIL — `no method named sigreturn_restore`.

- [ ] **Step 3: Write minimal implementation**

```rust
/// PSTATE bits a guest may set through its own signal frame: NZCV only.
///
/// The frame sits on the GUEST's stack, so `__ss.__cpsr` is guest-writable before `sigreturn`.
/// Restoring it verbatim into SPSR_EL1 would let a guest select its own exception level. This is
/// the only place in M12 where guest-controlled bytes reach a system register.
pub const PSTATE_USER_MASK: u64 = 0xf000_0000;
```

```rust
    /// The inverse of [`deliver_signal`](Self::deliver_signal): read the mcontext back out of guest
    /// memory and resume the interrupted context. Called by BOTH record and replay.
    pub fn sigreturn_restore(&mut self, uctx_ipa: u64, token: u64) {
        let expected = sigreturn_token(uctx_ipa);
        assert_eq!(token, expected,
            "sigreturn token mismatch: got {token:#x}, expected {expected:#x} for uctx {uctx_ipa:#x}. \
             The token is a pure function of the ucontext address, so a mismatch means a corrupted \
             frame or a sigreturn the guest reached without a delivery.");

        let uc = self.read_guest(uctx_ipa, 56);
        let mask = u32::from_le_bytes(uc[4..8].try_into().unwrap());
        let mc_ipa = u64::from_le_bytes(uc[48..56].try_into().unwrap());
        let mc = self.read_guest(mc_ipa, 816);
        let ts = &mc[16..];

        for i in 0..29u32 {
            let o = i as usize * 8;
            self.vcpu.set_reg(reg::x(i), u64::from_le_bytes(ts[o..o + 8].try_into().unwrap())).unwrap();
        }
        self.vcpu.set_reg(reg::FP, u64::from_le_bytes(ts[232..240].try_into().unwrap())).unwrap();
        self.vcpu.set_reg(reg::LR, u64::from_le_bytes(ts[240..248].try_into().unwrap())).unwrap();
        self.vcpu.set_sys(sysreg::SP_EL0, u64::from_le_bytes(ts[248..256].try_into().unwrap())).unwrap();
        self.vcpu.set_sys(sysreg::ELR_EL1, u64::from_le_bytes(ts[256..264].try_into().unwrap())).unwrap();
        let cpsr = u32::from_le_bytes(ts[264..268].try_into().unwrap()) as u64;
        self.vcpu.set_sys(sysreg::SPSR_EL1, cpsr & PSTATE_USER_MASK).unwrap();

        let ns = &mc[288..];
        for i in 0..32u32 {
            let o = i as usize * 16;
            self.vcpu.set_simd(simd::q(i), u128::from_le_bytes(ns[o..o + 16].try_into().unwrap())).unwrap();
        }
        self.vcpu.set_reg(reg::FPSR, u32::from_le_bytes(ns[512..516].try_into().unwrap()) as u64).unwrap();
        self.vcpu.set_reg(reg::FPCR, u32::from_le_bytes(ns[516..520].try_into().unwrap()) as u64).unwrap();

        self.sigtable.set_mask(retrace_arch::SIG_SETMASK, mask);
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p retrace-box --test deliver -- --test-threads=1`
Expected: PASS, all nine.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/deliver.rs
git commit -m "M12 t5: sigreturn_restore, with PSTATE sanitized

The round trip: deliver, clobber, restore, and the pre-signal state comes back
— vector registers included, which is what makes a RETURNING handler safe.

PSTATE is masked to NZCV. The frame lives on the guest's own stack, so
__ss.__cpsr is guest-writable before sigreturn; restoring it verbatim into
SPSR_EL1 would let a guest select its exception level. Only place in M12
where guest-controlled bytes reach a system register."
```

---

## Task 6: The record-side integration (atomic)

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` (the `Stop::Fault` arm at line 130; the raise arm's
  `Handler` branch at line 554; the `SYS_SIGRETURN` panic at line 589)
- Test: `crates/retrace-core/tests/signals.rs` (extend)

**Interfaces:**
- Consumes: `Box_::{deliver_signal, sigreturn_restore}` (Tasks 4/5); `Event::SignalDelivery` (Task 3); `retrace_arch::signal_of_esr` (Task 1).
- Produces: no new public API — three changed dispatch arms.

**Background the implementer needs.** All three arms change together because they are one behaviour:
a handler is entered, runs, and returns. Splitting them would leave a commit where a guest can enter
a handler and never leave.

**The M6 boundary must not move.** `Stop::Fault` is a lower-EL **stage-1** abort
(`crates/retrace-box/src/lib.rs:1913`); demand paging (`page_in_cache`, `commit_reserved_page`)
arrives as `Stop::Other`, a stage-2 abort (`crates/retrace-core/src/lib.rs:631`). Touch only the
`Stop::Fault` arm. Adding a `Stop::Other` consumer would break the argument that M12 cannot steal a
demand-paging case, and a regression test pins it.

Ordering inside the fault arm: consult the disposition **first**; only the non-`Handler` path falls
through to M6's `Event::Crash`, byte-for-byte unchanged.

- [ ] **Step 1: Write the failing test**

Append to `crates/retrace-core/tests/signals.rs`:

```rust
/// Record a freestanding asm guest IN-PROCESS (the pattern every test in this file uses — there is
/// no shared helper; `crates/retrace/tests/util` is for the CLI-level gates).
fn record_asm(guest: &str) -> (retrace_core::Outcome, std::path::PathBuf) {
    let bytes = std::fs::read(guest).expect("read guest");
    let loaded = retrace_guest::parse_macho(&bytes);
    let name = guest.rsplit('/').next().unwrap();
    let p = std::env::temp_dir().join(format!("retrace-m12-{}-{name}.bin", std::process::id()));
    let s = retrace_core::record(&loaded, &p).expect("record must SUCCEED");
    (s.outcome, p)
}

// A fault with a handler installed must DELIVER, not crash. This is the live wrong answer M12
// exists to fix: Stop::Fault never consulted sigtable, so the handler was silently skipped.
#[test]
fn a_fault_with_a_handler_installed_delivers_instead_of_crashing() {
    let (outcome, trace) = record_asm(retrace_guest::SEGVCATCH);
    let (events, torn) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(!torn, "the recording must be complete");
    let deliveries: Vec<_> = events.iter()
        .filter(|e| matches!(e, retrace_trace::Event::SignalDelivery { .. })).collect();
    assert_eq!(deliveries.len(), 1, "exactly one delivery; events:\n{events:#?}");
    let retrace_trace::Event::SignalDelivery { sig, si_code, handler, .. } = deliveries[0] else {
        unreachable!()
    };
    assert_eq!(*sig, 11, "a store to an unmapped address is SIGSEGV");
    assert_eq!(*si_code, retrace_arch::SEGV_MAPERR, "nothing is mapped there: a translation fault");
    assert_ne!(*handler, 0);
    assert!(!events.iter().any(|e| matches!(e, retrace_trace::Event::Crash { .. })),
        "the handler ran, so this is NOT a crash");
    assert_eq!(outcome, retrace_core::Outcome::Exit { code: 0 },
        "segvcatch repairs the fault and exits 0 — a Crash outcome here means the handler was skipped");
}

#[test]
fn an_uncaught_fault_is_still_a_crash() {
    // The M6 regression. No handler installed => the Event::Crash path is untouched.
    let (outcome, trace) = record_asm(retrace_guest::WILDSTORE);
    assert!(matches!(outcome, retrace_core::Outcome::Crash { .. }), "got {outcome:?}");
    let (events, _) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(events.iter().any(|e| matches!(e, retrace_trace::Event::Crash { .. })),
        "an uncaught fault must still record as Crash, not a delivery");
    assert!(!events.iter().any(|e| matches!(e, retrace_trace::Event::SignalDelivery { .. })));
}

#[test]
fn sigreturn_is_recorded_as_an_ordinary_syscall_between_delivery_and_resumption() {
    let (_, trace) = record_asm(retrace_guest::SEGVCATCH);
    let (events, _) = retrace_trace::Reader::open_checked(&trace).unwrap();
    let di = events.iter().position(|e| matches!(e, retrace_trace::Event::SignalDelivery { .. }))
        .expect("a delivery");
    let si = events.iter().position(|e| matches!(e,
        retrace_trace::Event::Syscall { num, .. } if *num == retrace_arch::SYS_SIGRETURN))
        .expect("a sigreturn — the handler must have RETURNED, not aborted");
    assert!(si > di, "sigreturn comes after the delivery it returns from");
}

#[test]
#[should_panic(expected = "raising blocked signal")]
fn a_blocked_synchronous_fault_asserts_rather_than_guessing() {
    // POSIX leaves this undefined and Darwin force-delivers. M11 models no pending set, so
    // guessing here would be a plausible lie.
    record_asm(retrace_guest::BLOCKEDFAULT);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p retrace-core --test signals -- --test-threads=1`
Expected: FAIL — `SEGVCATCH` / `WILDSTORE` / `BLOCKEDFAULT` unresolved (Task 8 creates the guests).
**Write the arms now anyway**; this task's tests go green in Task 8. To keep this task
independently verifiable, assert the arms compile and the existing suite still passes.

- [ ] **Step 3: Write minimal implementation**

Replace the `Stop::Fault` arm:

```rust
            // M6: a stage-1 guest fault ends the recording as a CRASH — unless the guest installed
            // a handler for the signal that fault maps to, in which case M12 delivers it.
            //
            // The disposition check comes FIRST. Before M12 this arm never consulted sigtable at
            // all, so a guest that installed a SIGSEGV handler and then faulted was recorded as a
            // terminal crash with its handler silently skipped.
            //
            // Only Stop::Fault is touched. Demand paging (page_in_cache, commit_reserved_page)
            // arrives as Stop::Other — a stage-2 abort — so this cannot steal a demand-paging case,
            // for exactly the reason M6's arm couldn't.
            Stop::Fault { pc, esr, far } => {
                let (sig, si_code) = retrace_arch::signal_of_esr(esr);
                let act = b.sigtable().action(sig);
                if let retrace_box::Disposition::Handler(handler) = act.disp {
                    assert!(!b.sigtable().is_blocked(sig),
                        "raising blocked signal {sig} synchronously is not modelled: a fault cannot \
                         be deferred, POSIX leaves it undefined, and Darwin force-delivers. M11 \
                         models no pending set, so implement one — and revisit sigpending's \
                         always-empty answer — before a guest needs this.");
                    let (writes, resume_pc) = b.deliver_signal(sig, si_code, far, esr, far);
                    w.append(&Event::SignalDelivery { sig, si_code, si_addr: far, handler, resume_pc,
                                                      writes })
                        .map_err(|e| format!("append signal delivery: {e}"))?; count += 1;
                    continue;
                }
                let final_snap = b.snapshot();
                w.append(&Event::Crash { pc, esr, far }).map_err(|e| format!("append crash: {e}"))?; count += 1;
                w.append(&final_snap).map_err(|e| format!("append final snapshot: {e}"))?; count += 1;
                outcome = Outcome::Crash { pc, esr, far };
                break;
            }
```

Replace the raise arm's `Handler` branch (line 554):

```rust
                    // M12: the self-raise counterpart of the fault path. The ordinary Syscall event
                    // is appended FIRST, so the divergence oracle still checks (num, args) and the
                    // kill safety boundary above still runs; the delivery is a second landmark.
                    // esr/far are 0: no hardware fault happened, and inventing a syndrome would be
                    // the lie M11 refused when it kept Event::Signal out of Event::Crash.
                    retrace_box::Disposition::Handler(handler) => {
                        w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![] })
                            .map_err(|e| format!("append caught raise: {e}"))?; count += 1;
                        let (writes, resume_pc) = b.deliver_signal(sig, retrace_arch::SI_USER, 0, 0, 0);
                        w.append(&Event::SignalDelivery { sig, si_code: retrace_arch::SI_USER,
                                                          si_addr: 0, handler, resume_pc, writes })
                            .map_err(|e| format!("append signal delivery: {e}"))?; count += 1;
                    }
```

Replace the `SYS_SIGRETURN` panic (line 589):

```rust
            // M12: sigreturn(184) — the handler returning. Serviced, never forwarded. Its register
            // restore is recomputed identically on both sides by Box_::sigreturn_restore, so the
            // event carries no writes; (num, args) is still oracle-checked.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SIGRETURN => {
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![] })
                    .map_err(|e| format!("append sigreturn: {e}"))?; count += 1;
                b.sigreturn_restore(args[0], args[2]);
            }
```

Add `pub const SI_USER: u64 = 0x10001;` to `retrace-arch` (measured by `spikes/sigabi.c` as `65537`).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p retrace-core -- --test-threads=1` and `cargo build --workspace`
Expected: the workspace builds; the four new tests still fail on the missing guests (Task 8), and
**every pre-existing test still passes** — especially `crashy_e2e`:

```bash
cargo test -p retrace --test crashy_e2e -- --test-threads=1
```

If that goes red, Task 1 Step 0's R1 measurement was wrong. Stop and amend the spec.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace-arch/src/lib.rs crates/retrace-core/tests/signals.rs
git commit -m "M12 t6: the three record dispatch sites

Fault-with-handler delivers instead of crashing (the live wrong answer: this
arm never consulted sigtable). Self-raise-with-handler appends its ordinary
Syscall event first, so the oracle and the kill safety boundary still run, then
delivers. sigreturn is serviced, replacing M11's panic.

Only Stop::Fault is touched. Demand paging arrives as Stop::Other, a stage-2
abort, so this cannot steal a demand-paging case — the same argument M6 made.

Tests referencing the new guests stay red until t8 creates them."
```

---

## Task 7: The replay mirror

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` (replay's `advance()`, near the M11 mirror at line 755)
- Test: `crates/retrace-core/tests/replay.rs` (extend)

**Interfaces:**
- Consumes: Task 6's record arms.
- Produces: no new public API.

**Background the implementer needs.** Symmetry rule 1: replay must recompute the frame through the
**same** `deliver_signal` and byte-compare it against the recorded `writes` before applying. That
comparison *is* the divergence check — it is why an asymmetry surfaces as a loud divergence instead
of silent corruption.

Placement matters, and M11 already learned this the hard way (see the comment at line 757): a caught
fault arrives as a `Stop::Fault`, so its mirror must sit **before** the generic recorded-`Syscall`
lookup, or replay reports "expected recorded syscall, got SignalDelivery" — a confusing divergence
that looks like a recording bug and is a dispatch bug.

Note `deliver_signal` returns the frame it just **wrote**. Byte-comparing after writing is too late
to prevent the write, but not too late to detect it: the comparison runs before the session advances,
so a mismatch aborts replay at the right landmark with both byte strings in hand.

- [ ] **Step 1: Write the failing test**

```rust
fn record_segvcatch() -> std::path::PathBuf {
    let bytes = std::fs::read(retrace_guest::SEGVCATCH).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    let p = std::env::temp_dir().join(format!("retrace-m12-rep-{}.bin", std::process::id()));
    retrace_core::record(&loaded, &p).expect("record");
    p
}

#[test]
fn replay_recomputes_the_frame_and_matches_the_recording_byte_for_byte() {
    let trace = record_segvcatch();
    let r = retrace_core::replay(&trace).expect("replay must not diverge");
    assert_eq!(r.outcome, retrace_core::Outcome::Exit { code: 0 });
    assert_eq!(r.stdout, b"caught\nresumed\n");
}

// The mirror is load-bearing, and this proves it rather than asserting it: with the replay-side
// delivery arm removed, replay's sigtable never blocks the signal and never enters the handler,
// so it diverges at the recorded SignalDelivery.
#[test]
fn a_recorded_delivery_that_replay_does_not_reproduce_diverges_loudly() {
    let trace = record_segvcatch();
    // Corrupt one byte of the recorded frame in place, the way replay.rs's tamper test does.
    let mut events = retrace_trace::Reader::open(&trace).unwrap();
    let d = events.iter_mut()
        .find(|e| matches!(e, retrace_trace::Event::SignalDelivery { .. }))
        .expect("a recorded delivery");
    if let retrace_trace::Event::SignalDelivery { writes, .. } = d { writes[0].bytes[0] ^= 0xff; }
    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);

    let err = retrace_core::replay(&trace).unwrap_err();
    assert!(err.detail.contains("signal frame mismatch"), "detail: {}", err.detail);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p retrace-core --test replay -- --test-threads=1`
Expected: FAIL — replay hits `expected recorded syscall, got SignalDelivery`.

- [ ] **Step 3: Write minimal implementation**

In replay's `advance()`, add a `Stop::Fault` arm before the generic lookup, and mirror the raise and
`sigreturn` arms:

```rust
                // M12 mirror of record's fault-delivery arm. Must precede the generic recorded-
                // Syscall lookup: a caught fault arrives as Stop::Fault, and placing this after
                // yields "expected recorded syscall, got SignalDelivery" — a confusing divergence
                // that looks like a recording bug and is a dispatch bug. (M11 line 757's lesson.)
                Stop::Fault { pc, esr, far } => {
                    let (sig, si_code) = retrace_arch::signal_of_esr(esr);
                    if let retrace_box::Disposition::Handler(_) = self.b.sigtable().action(sig).disp {
                        match self.events.get(self.idx) {
                            Some(Event::SignalDelivery { sig: rsig, writes: rw, .. }) => {
                                let (rsig, rw) = (*rsig, rw.clone());
                                let (mine, _) = self.b.deliver_signal(sig, si_code, far, esr, far);
                                if sig != rsig || mine != rw {
                                    return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "signal frame mismatch: live sig={sig} recorded sig={rsig}; \
                                         recomputed {} bytes at {:#x}, recorded {} bytes at {:#x}",
                                        mine[0].bytes.len(), mine[0].ipa,
                                        rw[0].bytes.len(), rw[0].ipa) });
                                }
                                self.idx += 1;
                                continue;
                            }
                            other => return Err(Divergence { landmark: self.idx, pc, detail:
                                format!("expected recorded SignalDelivery, got {other:?} \
                                         (live fault: sig={sig} far={far:#x})") }),
                        }
                    }
                    // uncaught: fall through to M6's recorded-Crash verify, unchanged
                    ...
                }
```

and, inside the existing serviced-syscall mirror block, alongside M11's `sigaction` mirror:

```rust
                            if num == retrace_arch::SYS_SIGRETURN {
                                self.b.sigreturn_restore(args[0], args[2]);
                            }
```

plus the raise mirror's `Handler` branch, which recomputes and compares the same way.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p retrace-core -- --test-threads=1`
Expected: PASS once Task 8's guests exist; until then, `cargo build --workspace` clean and no
pre-existing test regressed.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace-core/tests/replay.rs
git commit -m "M12 t7: the replay mirror, and the byte-compare that IS the oracle

Replay recomputes the frame through the same deliver_signal and compares it
against the recording before advancing — an asymmetry surfaces as a loud
divergence rather than silent corruption.

The fault mirror sits BEFORE the generic recorded-Syscall lookup: a caught
fault arrives as Stop::Fault, and placing it after yields 'expected recorded
syscall, got SignalDelivery', which reads as a recording bug and is a dispatch
bug. M11 line 757 learned this once already."
```

---

## Task 8: The guest fixtures and the four mechanism gates

**Files:**
- Create: `crates/retrace-guest/asm/sigframe.s`, `segvcatch.s`, `altstack.s`, `vecsurvive.s`, `blockedfault.s`
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs`
- Create: `crates/retrace/tests/sigdeliver_e2e.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–7.
- Produces: `retrace_guest::{SIGFRAME, SEGVCATCH, ALTSTACK, VECSURVIVE, BLOCKEDFAULT}` path constants.

**Background the implementer needs.** These guests are freestanding (`-nostdlib -static`) and supply
their **own** trampoline, so they test retrace's contract without libc in the way. Follow
`asm/raise.s` and `asm/sigign.s` (M11) for the syscall convention: number in `x16`, args in `x0..`,
`svc #0x80`.

A guest trampoline is entered with the measured registers and must (a) check them, (b) call the
handler if it wants, and (c) `svc` `sigreturn`(184) with `x0 = ucontext*`, `x1 = infostyle`,
`x2 = token` — the values it received in `x4`, `x1`, `x5`.

**W^X applies.** The trampoline is guest code in the text segment, so it is already RO+exec. Do not
put it on the stack.

`segvcatch.s` is the important one: its handler adds 4 to `__ss.__pc` in the ucontext
(`uctx->uc_mcontext->__ss.__pc`, i.e. load the pointer at `uc+48`, then `+16+256`) so the guest
resumes **past** the faulting store. That is what proves `sigreturn` restores mutated state.

- [ ] **Step 1: Write the failing test**

Create `crates/retrace/tests/sigdeliver_e2e.rs`:

```rust
// M12 mechanism gates. Freestanding guests with their own trampolines: they test retrace's entry
// contract without Apple's _sigtramp in the way (that one is sigcatch_dyn_e2e's job).
mod util;

#[test]
fn the_trampoline_is_entered_with_the_measured_registers() {
    let (rec, trace) = util::record(retrace_guest::SIGFRAME);
    // sigframe.s exits 0 only if x0..x5 and sp all matched; each mismatch exits with its own code.
    assert_eq!(rec.code, 0,
        "entry-register contract violated (see sigframe.s for the per-check exit codes); stderr:\n{}",
        rec.stderr);
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0);
    assert_eq!(rep.stdout, rec.stdout);
}

#[test]
fn a_handler_can_repair_a_fault_and_sigreturn_past_it() {
    let (rec, trace) = util::record(retrace_guest::SEGVCATCH);
    assert_eq!(rec.code, 0, "the handler advances __ss.__pc by 4 and the guest continues; stderr:\n{}",
               rec.stderr);
    assert_eq!(rec.stdout, b"caught\nresumed\n",
        "both lines prove it: the handler ran AND the guest came back past the faulting store");
    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 0, "replay {i}");
        assert_eq!(rep.stdout, rec.stdout, "replay {i} diverged");
    }
}

#[test]
fn a_handler_with_sa_onstack_runs_on_the_alternate_stack() {
    // The headline gate CANNOT prove this: a wild-pointer fault runs fine on the main stack, so
    // SA_ONSTACK could be ignored entirely and the headline would still pass.
    let (rec, trace) = util::record(retrace_guest::ALTSTACK);
    assert_eq!(rec.code, 0, "the handler asserts its own sp is inside the alt stack; stderr:\n{}",
               rec.stderr);
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0);
}

#[test]
fn vector_state_survives_a_caught_fault() {
    // A handler is ordinary compiled code and will use NEON. Without sigreturn restoring Q0-Q31 a
    // handler that RETURNS silently corrupts the guest — and the headline gate cannot see it,
    // because its guest re-faults and dies immediately.
    let (rec, trace) = util::record(retrace_guest::VECSURVIVE);
    assert_eq!(rec.code, 0, "v8 must hold its pre-fault value after sigreturn; stderr:\n{}",
               rec.stderr);
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0);
}

#[test]
fn a_blocked_synchronous_fault_fails_loud() {
    // The fail-loud pattern from killother_e2e: a nonzero exit whose stderr names the boundary.
    let (rec, _trace) = util::record(retrace_guest::BLOCKEDFAULT);
    assert_ne!(rec.code, 0, "the guest must not reach exit(0); stderr:\n{}", rec.stderr);
    assert!(rec.stderr.contains("raising blocked signal"),
        "a fault cannot be deferred and M11 models no pending set — assert rather than guess; \
         stderr:\n{}", rec.stderr);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p retrace --test sigdeliver_e2e -- --test-threads=1`
Expected: FAIL — `retrace_guest::SIGFRAME` unresolved.

- [ ] **Step 3: Write minimal implementation**

Create the five guests. `segvcatch.s`, as the model — the others follow its shape:

```asm
// M12: install a SIGSEGV handler with our own trampoline, fault, repair, and continue.
// The handler advances the saved pc past the faulting store, so sigreturn resuming MUTATED state
// is what makes this guest exit 0 instead of looping on the same fault forever.
.text
.global _start
.align 2
_start:
    // sigaction(SIGSEGV=11, &act, NULL) — struct __sigaction is 24 bytes:
    //   +0 sa_handler  +8 sa_tramp  +16 sa_mask  +20 sa_flags
    adrp    x1, act@PAGE
    add     x1, x1, act@PAGEOFF
    adrp    x2, handler@PAGE
    add     x2, x2, handler@PAGEOFF
    str     x2, [x1, #0]
    adrp    x2, tramp@PAGE
    add     x2, x2, tramp@PAGEOFF
    str     x2, [x1, #8]
    mov     w2, #0x40                   // SA_SIGINFO
    str     w2, [x1, #20]
    mov     x0, #11
    mov     x2, #0
    mov     x16, #46
    svc     #0x80

    // Fault: store through an unmapped address. The handler advances past THIS instruction.
    movz    x9, #0xdead, lsl #16
    str     xzr, [x9]                   // <-- the faulting store

    // write(1, "resumed\n", 8); exit(0)
    mov     x0, #1
    adrp    x1, resumed@PAGE
    add     x1, x1, resumed@PAGEOFF
    mov     x2, #8
    mov     x16, #4
    svc     #0x80
    mov     x0, #0
    mov     x16, #1
    svc     #0x80

// Entered by retrace with x0=catcher x1=infostyle x2=sig x3=siginfo* x4=ucontext* x5=token.
tramp:
    stp     x4, x5, [sp, #-16]!         // keep ucontext* and token across the handler call
    str     x1, [sp, #-16]!
    blr     x0                          // call the handler (x0..x2 are already its args)
    ldr     x1, [sp], #16
    ldp     x0, x2, [sp], #16           // x0 = ucontext*, x2 = token
    mov     x16, #184                   // sigreturn(uctx, infostyle, token)
    svc     #0x80
    brk     #0                          // sigreturn must not return

// void handler(int sig, siginfo_t *si, ucontext_t *uc) — advance uc->uc_mcontext->__ss.__pc by 4.
handler:
    mov     x0, #1
    adrp    x1, caught@PAGE
    add     x1, x1, caught@PAGEOFF
    mov     x2, #7
    mov     x16, #4
    svc     #0x80
    // x2 was clobbered by the write; reload the ucontext from the frame the trampoline saved.
    ldr     x9, [sp, #16]               // ucontext*
    ldr     x10, [x9, #48]              // uc_mcontext (a POINTER — measured at ucontext+48)
    ldr     x11, [x10, #(16 + 256)]     // __ss.__pc: thread_state at mcontext+16, __pc at +256
    add     x11, x11, #4
    str     x11, [x10, #(16 + 256)]
    ret

.data
.align 4
act:      .space 24
caught:   .ascii "caught\n"
resumed:  .ascii "resumed\n"
```

Then wire all five into `build.rs` (copy the `raise.s` stanza) and export the path constants from
`src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p retrace --test sigdeliver_e2e -- --test-threads=1` and
`cargo test -p retrace-core -- --test-threads=1`
Expected: PASS — including Tasks 6 and 7's tests, which were waiting on these guests.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-guest crates/retrace/tests/sigdeliver_e2e.rs
git commit -m "M12 t8: five guests, and the mechanism gates go green

segvcatch proves the thing the headline cannot: sigreturn restores MUTATED
state (the handler advances __ss.__pc past the faulting store, so the guest
continues instead of re-faulting forever).

altstack and vecsurvive both cover blind spots the headline gate has by
construction — a wild-pointer fault runs fine on the main stack, and its guest
re-faults and dies before clobbered vector state could show."
```

---

## Task 9: Apple's real `_sigtramp`

**Files:**
- Create: `crates/retrace-guest/c/sigcatch_dyn.c`
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs`
- Create: `crates/retrace/tests/sigcatch_dyn_e2e.rs`

**Interfaces:**
- Consumes: Tasks 1–8.
- Produces: `retrace_guest::SIGCATCH_DYN`.

**Background the implementer needs.** Every guest so far supplies its own trampoline, so none of them
tests the thing that actually ships: Apple's `_sigtramp`, which libc installs into `sa_tramp` behind
`sigaction()`'s back. This gate is the only one that does. Follow `c/crashy.c` for the
dynamically-linked guest shape.

Use `sigaction()` — **not** `signal()`, which sets `SA_RESTART` and hides the flags — and
`SA_SIGINFO`, so the handler receives `siginfo_t*` and `ucontext_t*` and can be checked against the
frame retrace built.

- [ ] **Step 1: Write the failing test**

```rust
// The only gate that exercises APPLE's _sigtramp rather than a hand-written one: libc's sigaction()
// installs its own sa_tramp, which is what a real program actually runs through.
mod util;

#[test]
fn a_dynamic_c_guest_catches_repairs_and_continues_through_apples_sigtramp() {
    let (rec, trace) = util::record_dynamic(retrace_guest::SIGCATCH_DYN);
    assert_eq!(rec.code, 0, "stderr:\n{}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout);
    assert!(out.contains("si_addr=0xdead0000"),
        "the handler read si_addr out of the frame retrace built; stdout:\n{out}");
    assert!(out.contains("resumed"), "and sigreturn brought it back; stdout:\n{out}");
    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 0, "replay {i}; stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} diverged");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p retrace --test sigcatch_dyn_e2e -- --test-threads=1`
Expected: FAIL — `retrace_guest::SIGCATCH_DYN` unresolved.

- [ ] **Step 3: Write minimal implementation**

```c
// M12: a dynamically-linked guest that catches SIGSEGV through Apple's REAL _sigtramp.
// libc's sigaction() installs its own sa_tramp, so this is the only guest that exercises it.
#include <stdio.h>
#include <signal.h>
#include <stdint.h>
#include <sys/ucontext.h>

static void handler(int sig, siginfo_t *si, void *ucv) {
    ucontext_t *uc = (ucontext_t *)ucv;
    printf("caught sig=%d si_addr=%p\n", sig, si->si_addr);
    fflush(stdout);
    // Step past the faulting store so the guest can continue: proves sigreturn restores MUTATED
    // state through the real trampoline, not just through ours.
    uc->uc_mcontext->__ss.__pc += 4;
}

int main(void) {
    struct sigaction sa;
    sa.sa_sigaction = handler;
    sa.sa_flags = SA_SIGINFO;
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGSEGV, &sa, NULL) != 0) { printf("sigaction failed\n"); return 1; }
    printf("installed\n");
    fflush(stdout);
    *(volatile uint64_t *)0xdead0000 = 1;
    printf("resumed\n");
    fflush(stdout);
    return 0;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p retrace --test sigcatch_dyn_e2e -- --test-threads=1`
Expected: PASS. If it fails inside `_sigtramp`, re-read Task 1 Step 0's disassembly: the trampoline
is reading something the frame does not provide.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-guest crates/retrace/tests/sigcatch_dyn_e2e.rs
git commit -m "M12 t9: Apple's real _sigtramp, exercised

Every guest so far supplied its own trampoline. libc's sigaction() installs
Apple's, which is what real programs run through, so this is the only gate
that proves the frame satisfies the trampoline that actually ships."
```

---

## Task 10: The headline gate, the seek, and the honest close

**Files:**
- Create: `crates/retrace-guest/rs/segvy.rs`
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs`
- Create: `crates/retrace/tests/segv_rust_e2e.rs`
- Modify: `README.md` (new Status section), `CLAUDE.md` (gate count, milestone list)

**Interfaces:**
- Consumes: everything.
- Produces: `retrace_guest::SEGVY`.

**Background the implementer needs.** Measured natively (spec §"The headline guest's behaviour"), a
stock Rust binary storing through `0xdead0000` does this: fault → libstd's handler → decides it is
not a stack overflow → `sigaction(SIGSEGV, SIG_DFL)` → returns → `sigreturn` → the store re-executes
→ faults again → default action terminates → **exit 139**.

**Exit 139 is necessary and nowhere near sufficient.** An *uncaught* fault also exits 139 — that is
exactly what `crashy_e2e::record_and_replay_of_a_crash_exit_139_with_the_crash_line` asserts. If
M12's routing were entirely broken and the handler ignored as it is today, this guest would **still**
exit 139. The four trace assertions below are the gate; the exit code is a smoke test.

Build with default settings — **not** `-C panic=abort`. M11 needed that because a default
`panic!()` unwinds and never signals; a hardware fault is not a panic and needs no such flag.

- [ ] **Step 1: Write the failing test**

```rust
// THE M12 HEADLINE GATE. A stock full-std Rust binary faults on a wild pointer. libstd's own
// SIGSEGV handler runs, decides it is not a stack overflow, resets to SIG_DFL and RETURNS; the
// store re-executes, faults again, and the default action terminates the guest.
//
// Exit 139 alone proves nothing: an UNCAUGHT fault exits 139 too (crashy_e2e asserts exactly that),
// so a gate resting on the exit code would pass unchanged if M12's routing were entirely broken.
// The trace assertions are the gate.
mod util;
use retrace_trace::Event;

#[test]
fn a_faulting_rust_guest_runs_its_own_handler_and_records_its_death() {
    let (rec, trace) = util::record_dynamic(retrace_guest::SEGVY);
    assert_eq!(rec.code, 139, "139 == 128 + SIGSEGV; stderr:\n{}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout);
    assert!(out.starts_with("about to fault\n"),
            "the guest must reach its OWN code, not die inside dyld; stdout:\n{out}");
    assert!(!out.contains("has overflowed its stack"),
        "libstd compares si_addr against its guard range — this message means si_addr is WRONG \
         (and the guest would have exited 134, not 139); stdout:\n{out}");

    let (events, torn) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(!torn, "a recorder killed mid-run leaves a torn trace — this must be complete");

    // (1) exactly one delivery, for SIGSEGV, to the handler libstd actually installed
    let deliveries: Vec<_> = events.iter().enumerate()
        .filter(|(_, e)| matches!(e, Event::SignalDelivery { .. })).collect();
    assert_eq!(deliveries.len(), 1, "exactly one handler entry");
    let (di, Event::SignalDelivery { sig, handler, resume_pc, .. }) = deliveries[0] else {
        unreachable!()
    };
    assert_eq!(*sig, 11);
    let installed = installed_handler_for_sigsegv(&events)
        .expect("libstd installs a SIGSEGV handler at startup — M11 measured flags 0x41");
    assert_eq!(*handler, installed,
        "the delivery must target the handler the guest installed, not some other address");

    // (2) a sigreturn AFTER it: the handler RETURNED rather than aborting
    let si = events.iter().enumerate().position(|(i, e)| i > di && matches!(e,
        Event::Syscall { num, .. } if *num == retrace_arch::SYS_SIGRETURN))
        .expect("libstd's handler resets to SIG_DFL and returns — there must be a sigreturn");

    // (3) a terminal Signal after that: the re-fault took the default action
    let ti = events.iter().position(|e| matches!(e, Event::Signal { sig: 11, .. }))
        .expect("the re-fault must terminate the guest");
    assert!(ti > si, "the terminal signal follows the sigreturn");
    assert!(matches!(events.last(), Some(Event::Snapshot { .. })),
        "terminal events are followed by the final full-memory snapshot");

    // (4) the store re-executed rather than being skipped
    let crash_pc = match &events[ti] { Event::Signal { pc, .. } => *pc, _ => unreachable!() };
    assert_eq!(*resume_pc, crash_pc,
        "sigreturn resumed AT the faulting instruction, so the second fault is the same store");

    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 139, "replay {i}; stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged");
    }
}

/// The handler VA libstd installed, learned from the recorded sigaction rather than hardcoded
/// (it moves with every build).
fn installed_handler_for_sigsegv(events: &[Event]) -> Option<u64> { /* scan Syscall num==46, args[0]==11 */ }

#[test]
fn the_delivery_is_a_seekable_landmark() {
    // The payoff that justified a first-class event over below-the-trace handling. If this cannot
    // be done, the architecture decision in the spec was wrong and should be revisited, not the test.
    let (_, trace) = util::record_dynamic(retrace_guest::SEGVY);
    let (events, _) = retrace_trace::Reader::open_checked(&trace).unwrap();
    let di = events.iter().position(|e| matches!(e, Event::SignalDelivery { .. })).unwrap();
    let s = retrace_core::seek(&trace, di, 0).expect("seek to the delivery landmark");
    assert_eq!(s.landmark(), di);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p retrace --test segv_rust_e2e -- --test-threads=1`
Expected: FAIL — `retrace_guest::SEGVY` unresolved.

- [ ] **Step 3: Write minimal implementation**

`crates/retrace-guest/rs/segvy.rs`:

```rust
// M12 headline guest. A stock full-std Rust binary that faults on a wild pointer. Deliberately
// NOT built with -C panic=abort: a hardware fault is not a panic, so unlike M11's panicky.rs this
// needs no flag to reach a signal.
fn main() {
    println!("about to fault");
    unsafe { std::ptr::write_volatile(0xdead0000usize as *mut u64, 1) };
    println!("survived");
}
```

Wire it into `build.rs` next to `panicky.rs` (same `rustc` invocation, minus `-C panic=abort`).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p retrace --test segv_rust_e2e -- --test-threads=1`
Expected: PASS.

**If it does not clear**, apply honest-gate discipline: park it `#[ignore]`d with the *exact* first
line of the failure pasted into the reason, name the wall in the README, and leave the other five
headline gates untouched. Never loosen an assertion to make it pass.

- [ ] **Step 5: Run the full gate**

```bash
just gate 2>&1 | tail -30
```

Expected: green, **zero ignored**, with the count risen from 240 by the tests this milestone added.
Confirm `crashy_e2e`, `hello_dyn_e2e`, `hello_rust_e2e`, `jq_e2e`, `jq_file_e2e`, and `panic_e2e`
are all still green and un-ignored.

- [ ] **Step 6: Write the Status section and correct the M11 prose**

Add a `## Status: M12-signal-delivery` section to `README.md` covering: what runs today; the measured
facts that shaped it (the frame layout, the tramp contract, libstd's reset-and-return); what driving
it actually found; and the new boundary. Update `CLAUDE.md`'s gate count, milestone list, and
headline-gate paragraph.

**Correct the M11 Status sentence** — "the first guest that actually faults will hit the `Handler`
assert rather than a plausible lie" was not true of the code, and the correction belongs in the
record rather than being quietly overwritten.

Name the new top deferred item: **`PROT_NONE` enforcement**, with the reasoning from the spec (a
guard page that does not guard, and what a stack-overflow milestone would need).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "M12 t10: the headline gate is GREEN, and the honest close

A stock full-std Rust binary faults on a wild pointer, libstd's own SIGSEGV
handler runs and returns, the store re-executes, and the guest dies of the
signal — recorded and replayed bit-for-bit at exit 139.

The gate does not rest on 139: an uncaught fault exits 139 too (crashy_e2e
asserts exactly that), so it would pass unchanged if M12's routing were
broken. It asserts one delivery to libstd's actual installed handler, a
sigreturn after it, a terminal Signal after that, and resume_pc == the crash
pc — the store re-executed rather than being skipped.

Corrects the M11 Status claim that a faulting guest with a handler installed
would hit the Handler assert; it did not, and that was the milestone's premise.

New top deferred item: PROT_NONE enforcement — commit_reserved_page
demand-commits any reserved page, so libstd's guard page does not guard."
```

---

## Self-Review

**Spec coverage.** Every spec section maps to a task: M12-esr → T1; M12-frame → T2; M12-format → T3;
M12-deliver → T4; M12-sigreturn + PSTATE → T5; M12-neon → T4 (fill) + T5 (restore) + T8 (the gate);
M12-route → T6/T7; the fail-loud boundaries → T5 (token, PSTATE) and T6/T8 (blocked fault); the
five unmeasured facts → T1 Step 0; the gate set → T8/T9/T10; the exit criterion and honest-gate
discipline → T10 Steps 4–6.

Two spec items are deliberately **not** separate tasks and are called out here so their absence is a
decision rather than an omission:

- **Nested delivery** (a fault inside a handler) is listed as a fail-loud boundary but has no
  dedicated guest. `deliver_signal` blocks the signal on entry, so a *same-signal* nested fault hits
  T6's blocked-signal assert and is covered by `blockedfault.s`. A *different*-signal nested fault is
  not covered. If T1 Step 0 shows any gate guest can produce one, add a guest for it.
- **`si_code` for `SIGILL`/`SIGTRAP`** is implemented and unit-tested in T1 but never produced by a
  guest, because no fixture executes an illegal instruction. Tested, not exercised — worth a line in
  the Status section rather than a fake gate.

**Placeholder scan.** One intentional stub: `installed_handler_for_sigsegv` in T10 is written as a
signature plus a comment describing the scan. Implement it as a filter over `Event::Syscall` with
`num == 46 && args[0] == 11 && args[1] != 0`, reading the handler VA from the first 8 bytes at
`args[1]`. Everything else contains real code.

**Type consistency.** `SigAction` gains `tramp` in T2 and is used with that field in T4, T5, and the
tests. `deliver_signal` returns `(Vec<Region>, u64)` in T4 and is destructured that way in T6 and T7.
`sigreturn_restore(uctx_ipa, token)` takes two arguments in T5 and is called with `(args[0], args[2])`
in T6 and T7 — matching the trampoline's `sigreturn(uctx, infostyle, token)`, where `infostyle` is
`args[1]` and is deliberately unused. `FRAME_UCONTEXT_OFF`/`FRAME_MCONTEXT_OFF`/`FRAME_LEN` keep the
same names from T2 through T10.
