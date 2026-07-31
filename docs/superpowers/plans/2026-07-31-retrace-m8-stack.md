# retrace M8-stack Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the guest's stack identity truthful — synthesize `kern.usrstack64` and `RLIMIT_STACK` from the box's own stack geometry, honor `addr`/`MAP_FIXED` in the anonymous `mmap` path — and add an address-space determinism oracle that catches host addresses masquerading as guest addresses.

**Architecture:** Three synthesis/placement changes in the record dispatch (`retrace-core`) and the box (`retrace-box`), each with a mirrored replay arm so the existing byte-compare *is* the divergence check. Plus a test-side oracle that records the same guest twice and compares only address-shaped fields.

**Tech Stack:** Rust 1.95.0 (pinned), `aarch64-apple-darwin`, Hypervisor.framework, freestanding arm64 asm guest fixtures built by `clang` in `retrace-guest/build.rs`.

**Spec:** `docs/superpowers/specs/2026-07-31-retrace-m8-stack-design.md`

**Branch:** `m8-stack` (already created; the spec is committed there as `7b46ad3`).

## Global Constraints

- **`--test-threads=1` is mandatory.** HVF allows one VM per process. Use `just gate`, or `cargo test -p <crate> <name> -- --test-threads=1` for a single test. A bare `cargo test` flakes with `HV_BUSY`.
- **The exit gate is `just gate`** = `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings`.
- **Red-gate ruling (decided before execution).** This plan is staged TDD, so Tasks 2–4 deliberately end with M8 tests failing; the clean-gate requirement binds from **Task 5 onward**. The exact expected-red set is stated per task and is the *only* permitted failure — **no other test may regress at any point**, and clippy must be clean at every task. A failure outside the named set is a real regression, not staging.

  | After task | Expected RED | Expected GREEN |
  |---|---|---|
  | 2 | `usrstack64_reports_the_guests_own_stack_top`, `rlimit_stack_reports_the_guests_own_stack_size`, `anonymous_map_fixed_lands_at_the_requested_address`, `usrstack_records_deterministically` | `usrstack_replays_bit_for_bit`, all pre-existing tests |
  | 3 | `rlimit_stack_…`, `anonymous_map_fixed_…`, `usrstack_records_deterministically` | `usrstack64_…`, `usrstack_replays_bit_for_bit`, all pre-existing |
  | 4 | `anonymous_map_fixed_…`, `usrstack_records_deterministically` | `usrstack64_…`, `rlimit_stack_…`, `usrstack_replays_bit_for_bit`, all pre-existing |
  | 5 | *(none)* | everything — full `just gate` clean |
- **Symmetry rule 1:** every special case added to record's `match stop` needs a mirror in replay's dispatch, and both must recompute *identical* addresses/bytes. Replay byte-compares its recomputed reply against the recording — that comparison is the divergence check.
- **Symmetry rule 2:** deterministic instruction emulation belongs below the trace, inside `Box_::run()`. Not applicable to this milestone's changes (these are syscall-dispatch level), but do not move them below the trace.
- **Never reimplement Apple's PAC.** Not touched by this milestone.
- **Drop order:** `Box_`'s field declaration order is load-bearing — `vcpu` must be declared before `vm`. When adding fields, do **not** reorder existing ones.
- **`clippy.toml` bans `Instant::now`/`SystemTime::now`/`std::thread`.** These denials are load-bearing determinism guards, not style.
- **Honest-gate discipline:** `hello_rust_e2e` is un-ignored only on a genuine double pass. A stale `#[ignore]` reason is worse than none. Never delete the ignored test.
- **Baseline to beat:** HEAD of `main` (`48fe554`) measures **146 passed / 0 failed / 1 ignored**, clippy clean.

---

### Task 1: Address-space determinism oracle + calibrate it on `hello_dyn`

Builds the oracle harness and answers spec open question 1 empirically: *does `hello_dyn` itself leak a host address?* The answer determines nothing about the rest of the plan's shape — but it must be recorded, because if `hello_dyn` fails the oracle on HEAD that is a **second real leak**, and it gets its own finding rather than an oracle weakened to accommodate it.

**Files:**
- Modify: `crates/retrace/tests/util/mod.rs` (append two helpers)
- Create: `crates/retrace/tests/determinism.rs`

**Interfaces:**
- Consumes: `util::record`, `util::record_dynamic` (existing).
- Produces: `util::address_projection(&Path) -> Vec<(usize, &'static str, u64)>` and `util::assert_address_determinism(&Path, &Path)`, used by Tasks 2 and 6.

- [ ] **Step 1: Add the oracle helpers to `crates/retrace/tests/util/mod.rs`**

Append at the end of the file:

```rust
/// The ADDRESS-SHAPED projection of a trace: every field that names a guest address, in order.
///
/// Deliberately excludes opaque payload bytes, so values this project has explicitly and correctly
/// accepted as per-trace nondeterministic do NOT trip it: `getentropy`/`proc_info` (M2-cpuid design
/// spec, "Record legitimately differs run-to-run"), M2-xpcport's minted bootstrap port name, and
/// M2-taskinfo's audit token. What it DOES catch is a host address masquerading as a guest address
/// — the M8-stack defect class, which the replay divergence oracle is structurally blind to because
/// it only ever compares replay against one recording, never two recordings against each other.
pub fn address_projection(trace: &std::path::Path) -> Vec<(usize, &'static str, u64)> {
    use retrace_trace::Event;
    let events = retrace_trace::Reader::open(trace).expect("open trace");
    let mut out = Vec::new();
    for (i, e) in events.iter().enumerate() {
        match e {
            Event::Snapshot { mem, .. } => {
                for r in mem { out.push((i, "snapshot.ipa", r.ipa)); }
            }
            Event::Syscall { num, args, ret, writes, .. } => {
                match *num {
                    // mmap: arg0 is the requested address, ret is the placement.
                    197 => { out.push((i, "mmap.addr", args[0])); out.push((i, "mmap.ret", *ret)); }
                    // munmap / mprotect: arg0 is the target address.
                    73 | 74 => out.push((i, "munmap_or_mprotect.addr", args[0])),
                    _ => {}
                }
                for w in writes { out.push((i, "write.ipa", w.ipa)); }
            }
            _ => {}
        }
    }
    out
}

/// Assert two recordings of the SAME guest agree on every address-shaped field.
pub fn assert_address_determinism(a: &std::path::Path, b: &std::path::Path) {
    let (pa, pb) = (address_projection(a), address_projection(b));
    assert_eq!(pa.len(), pb.len(),
        "two recordings of the same guest produced different address-field counts ({} vs {}) — \
         the runs took structurally different paths", pa.len(), pb.len());
    for (x, y) in pa.iter().zip(pb.iter()) {
        assert_eq!(x, y,
            "address-shaped divergence between two recordings of the same guest: {x:?} vs {y:?} \
             — a host address is reaching the guest as a guest address");
    }
}
```

- [ ] **Step 2: Write the calibration test**

Create `crates/retrace/tests/determinism.rs`:

```rust
// M8-stack Task 1. The address-space determinism oracle, calibrated against a guest that is
// already green. `hello_dyn` records and replays bit-for-bit today, so if IT trips the oracle the
// oracle is either wrong or has found a second real host-address leak — either way that is a
// finding to investigate, never a reason to loosen the assertion.
mod util;

#[test]
fn hello_dyn_records_deterministic_addresses() {
    let (r1, t1) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(r1.code, 0, "first hello_dyn recording failed: {}", r1.stderr);
    let (r2, t2) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(r2.code, 0, "second hello_dyn recording failed: {}", r2.stderr);
    assert_eq!(r1.stdout, r2.stdout, "hello_dyn stdout differed between two recordings");
    util::assert_address_determinism(&t1, &t2);
}
```

- [ ] **Step 3: Run it and record the answer**

Run: `cargo test -p retrace --test determinism -- --test-threads=1 --nocapture`

This is a **reconnaissance step with two legitimate outcomes.** Record which one you got in the commit message:

- **PASS** — `hello_dyn` does not use a host address as a guest address. The oracle is calibrated against a known-good run. Proceed to Step 4.
- **FAIL** — `hello_dyn` leaks too. **Stop and report** before writing any production code. Capture the failing `(index, field, value)` pair and diagnose with
  `RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn <hello_dyn path> 2>&1 | grep -n '<the leaked value in hex>'`
  to find the carrier, exactly as the spec's "Verified facts" section did. That is a second finding and needs its own decision from the user; do not widen this task to fix it.

- [ ] **Step 4: Run the full gate**

Run: `just gate`
Expected: 147 passed / 0 failed / 1 ignored, clippy clean. (One new test over the 146 baseline.)

- [ ] **Step 5: Commit**

```bash
git add crates/retrace/tests/util/mod.rs crates/retrace/tests/determinism.rs
git commit -m "M8-stack t1: address-space determinism oracle, calibrated on hello_dyn

The replay divergence oracle compares replay against ONE recording and is
therefore structurally blind to a host address entering the trace -- which is
why the M8-stack defect survived seven milestones and 146 passing tests.

This adds the missing second oracle: record the same guest twice, compare only
ADDRESS-SHAPED fields. Address-shaped rather than byte-identical so it does not
disturb the per-trace nondeterminism the project has explicitly blessed
(getentropy/proc_info per M2-cpuid, M2-xpcport's minted port name,
M2-taskinfo's audit token) and needs no allowlist to maintain.

hello_dyn calibration result: <PASS: hello_dyn does not leak | FAIL: see report>

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: The `usrstack` guest fixture + the failing tests

Adds a freestanding asm guest that exercises all three defects, and the tests that pin them. **Both tests must FAIL at the end of this task** — that is the deliverable. Do not implement any fix here.

The fixture deliberately does **not** mmap at its own computed stack bottom. On the static (`record`) path the guest's entire stack is a single 16 KiB page (`retrace-box/src/lib.rs:624` maps `STACK_TOP_IPA - GRANULE`), so a `MAP_FIXED` there would unmap the stack the guest is currently running on. It uses an unrelated free IPA instead, which tests `MAP_FIXED` honoring in isolation.

**Files:**
- Create: `crates/retrace-guest/asm/usrstack.s`
- Modify: `crates/retrace-guest/build.rs` (append a build entry before the closing `}`)
- Modify: `crates/retrace-guest/src/lib.rs` (append a path constant after `HELLO_RUST` at `:116`)
- Modify: `crates/retrace/tests/determinism.rs`
- Create: `crates/retrace/tests/usrstack_e2e.rs`

**Interfaces:**
- Produces: `retrace_guest::USRSTACK` (a path constant), and a 32-byte stdout contract: four little-endian `u64`s — `usrstack`, `rlim_cur`, `rlim_max`, `mmap_ret`. Tasks 3, 4, 5 all assert against this contract.

- [ ] **Step 1: Create the guest fixture**

Create `crates/retrace-guest/asm/usrstack.s`:

```asm
// M8-stack. Exercises the three defects the milestone fixes, then publishes the results on
// stdout as four little-endian u64s so a test can assert on them:
//   [0]  kern.usrstack64   (sysctl {CTL_KERN=1, KERN_USRSTACK64=59})
//   [8]  rlim_cur          (getrlimit RLIMIT_STACK=3)
//   [16] rlim_max
//   [24] mmap return       (an anonymous MAP_FIXED request at 0xB_0000_0000)
//
// The MAP_FIXED target is deliberately an unrelated free IPA, NOT this guest's own stack bottom:
// on the static load path the whole guest stack is one 16 KiB page, so a MAP_FIXED there would
// unmap the stack this code is running on.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // sysctl(mib, 2, &out[0], &oldlen, NULL, 0)
    adrp x0, mib@PAGE
    add  x0, x0, mib@PAGEOFF
    mov  x1, #2
    adrp x2, out@PAGE
    add  x2, x2, out@PAGEOFF
    adrp x3, oldlen@PAGE
    add  x3, x3, oldlen@PAGEOFF
    mov  x4, #0
    mov  x5, #0
    mov  x16, #202                 // SYS___sysctl
    svc  #0x80

    // getrlimit(RLIMIT_STACK=3, &out[8])   -- writes rlim_cur then rlim_max
    mov  x0, #3
    adrp x1, out@PAGE
    add  x1, x1, out@PAGEOFF
    add  x1, x1, #8
    mov  x16, #194                 // SYS_getrlimit
    svc  #0x80

    // mmap(0xB_0000_0000, 0x4000, PROT_READ|PROT_WRITE, MAP_ANON|MAP_PRIVATE|MAP_FIXED, -1, 0)
    movz x0, #0xB, lsl #32
    mov  x1, #0x4000
    mov  x2, #3                    // PROT_READ|PROT_WRITE
    movz x3, #0x1012               // MAP_ANON(0x1000)|MAP_FIXED(0x10)|MAP_PRIVATE(0x02)
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197                 // SYS_mmap
    svc  #0x80
    adrp x9, out@PAGE
    add  x9, x9, out@PAGEOFF
    str  x0, [x9, #24]

    // write(1, out, 32)
    mov  x0, #1
    adrp x1, out@PAGE
    add  x1, x1, out@PAGEOFF
    mov  x2, #32
    mov  x16, #4                   // SYS_write
    svc  #0x80

    // exit(0)
    mov  x0, #0
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
.p2align 4
out:      .space 32                // usrstack, rlim_cur, rlim_max, mmap_ret
oldlen:   .quad 8                  // in/out: sizeof(u64)
mib:      .long 1                  // CTL_KERN
          .long 59                 // KERN_USRSTACK64
```

- [ ] **Step 2: Add the build entry**

In `crates/retrace-guest/build.rs`, insert immediately before the final closing `}` (after the `hello_rust` block that ends at `:278`):

```rust
    // usrstack: the M8-stack fixture. Issues sysctl(KERN_USRSTACK64), getrlimit(RLIMIT_STACK), and
    // an anonymous MAP_FIXED mmap, then publishes all four results as u64s on stdout. Plain
    // -arch arm64 freestanding, like the other micro-guests.
    let src = format!("{}/asm/usrstack.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/usrstack");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang usrstack");
    assert!(status.success(), "usrstack guest build failed");
```

- [ ] **Step 3: Add the path constant**

In `crates/retrace-guest/src/lib.rs`, after line 116 (`pub const HELLO_RUST: ...`):

```rust
pub const USRSTACK: &str = concat!(env!("OUT_DIR"), "/usrstack");
```

- [ ] **Step 4: Write the failing determinism test**

Append to `crates/retrace/tests/determinism.rs`:

```rust
#[test]
fn usrstack_records_deterministically() {
    let (r1, t1) = util::record(retrace_guest::USRSTACK);
    assert_eq!(r1.code, 0, "first usrstack recording failed: {}", r1.stderr);
    let (r2, t2) = util::record(retrace_guest::USRSTACK);
    assert_eq!(r2.code, 0, "second usrstack recording failed: {}", r2.stderr);
    assert_eq!(r1.stdout, r2.stdout,
        "two recordings of the same guest disagree on kern.usrstack64 / RLIMIT_STACK / the \
         MAP_FIXED placement — a host-derived value is reaching the guest");
    util::assert_address_determinism(&t1, &t2);
}
```

- [ ] **Step 5: Write the failing semantics test**

Create `crates/retrace/tests/usrstack_e2e.rs`:

```rust
// M8-stack. The guest stack identity contract: retrace must tell the guest the truth about its
// OWN address space, and must honor addr/MAP_FIXED for anonymous mmap.
//
// Static (`record`) load path geometry, from crates/retrace-box/src/lib.rs:
//   STACK_TOP_IPA = 0x20000, and load() maps ONE granule at STACK_TOP_IPA - GRANULE,
//   so the guest stack is [0x1C000, 0x20000) -- top 0x20000, size 0x4000.
mod util;

const STATIC_STACK_TOP:  u64 = 0x0002_0000;
const STATIC_STACK_SIZE: u64 = 0x0000_4000;
const FIXED_TARGET:      u64 = 0x000B_0000_0000;

fn fields(stdout: &[u8]) -> (u64, u64, u64, u64) {
    assert_eq!(stdout.len(), 32, "guest must publish exactly four u64s, got {} bytes", stdout.len());
    let g = |i: usize| u64::from_le_bytes(stdout[i..i + 8].try_into().unwrap());
    (g(0), g(8), g(16), g(24))
}

#[test]
fn usrstack64_reports_the_guests_own_stack_top() {
    let (rec, _t) = util::record(retrace_guest::USRSTACK);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let (usrstack, _, _, _) = fields(&rec.stdout);
    assert_eq!(usrstack, STATIC_STACK_TOP,
        "kern.usrstack64 must report the GUEST's stack top, not the host's ({usrstack:#x})");
}

#[test]
fn rlimit_stack_reports_the_guests_own_stack_size() {
    let (rec, _t) = util::record(retrace_guest::USRSTACK);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let (_, cur, max, _) = fields(&rec.stdout);
    assert_eq!((cur, max), (STATIC_STACK_SIZE, STATIC_STACK_SIZE),
        "RLIMIT_STACK must report the GUEST's stack size, not the host's");
}

#[test]
fn anonymous_map_fixed_lands_at_the_requested_address() {
    let (rec, _t) = util::record(retrace_guest::USRSTACK);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let (_, _, _, mapret) = fields(&rec.stdout);
    assert_eq!(mapret, FIXED_TARGET,
        "an anonymous MAP_FIXED mmap must land at the requested address, not at the bump \
         allocator's next slot ({mapret:#x})");
}

#[test]
fn usrstack_replays_bit_for_bit() {
    let (rec, trace) = util::record(retrace_guest::USRSTACK);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, rec.stdout, "replay stdout must match record stdout byte-for-byte");
}
```

- [ ] **Step 6: Run the tests to verify they fail for the RIGHT reasons**

Run: `cargo test -p retrace --test usrstack_e2e --test determinism -- --test-threads=1 --nocapture`

Expected failures — check each message, because a failure for the wrong reason means the fixture is broken, not the product:
- `usrstack64_reports_the_guests_own_stack_top` — FAIL, got a host-ASLR value around `0x16c…`–`0x16f…`.
- `rlimit_stack_reports_the_guests_own_stack_size` — FAIL, got `0x7fc000` (the host's 8176 KiB).
- `anonymous_map_fixed_lands_at_the_requested_address` — FAIL, got `0xa00000000` (`MMAP_BASE`).
- `usrstack_records_deterministically` — FAIL on the stdout comparison.
- `usrstack_replays_bit_for_bit` — **should PASS** even now: replay applies the recorded writes, so a single trace is self-consistent. This test is here to catch a *regression* introduced by Tasks 3–5, and its passing now is the proof that the replay oracle alone cannot see this defect.

- [ ] **Step 7: Commit the failing tests**

```bash
git add crates/retrace-guest/asm/usrstack.s crates/retrace-guest/build.rs \
        crates/retrace-guest/src/lib.rs crates/retrace/tests/usrstack_e2e.rs \
        crates/retrace/tests/determinism.rs
git commit -m "M8-stack t2: usrstack fixture + failing tests (RED)

A freestanding guest that issues sysctl(KERN_USRSTACK64),
getrlimit(RLIMIT_STACK), and an anonymous MAP_FIXED mmap, publishing all
four results on stdout as u64s.

Four tests fail as intended: usrstack64 reports a host ASLR address,
RLIMIT_STACK reports the host's 8176 KiB, MAP_FIXED lands at MMAP_BASE
instead of the requested address, and two recordings disagree.

usrstack_replays_bit_for_bit PASSES already -- that is the point. A single
trace is self-consistent because replay applies the recorded writes, which
is exactly why the replay divergence oracle cannot see this class of defect.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Teach `Box_` its own stack geometry + synthesize `kern.usrstack64`

The synthesis must be **path-aware**: the static path's stack is `[0x1C000, 0x20000)` (one granule) and the dynamic path's is `[0x1C0000, 0x200000)` (256 KiB). Hardcoding either constant would make the other path lie. So `Box_` gains the geometry as state, set at load.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (struct fields, three `Box_ { … }` literals, `BoxState`, capture/restore, one accessor)
- Modify: `crates/retrace-arch/src/lib.rs` (syscall + sysctl constants)
- Modify: `crates/retrace-core/src/lib.rs` (record arm + mirrored replay arm)

**Interfaces:**
- Consumes: `retrace_guest::USRSTACK` and the stdout contract from Task 2.
- Produces: `Box_::stack_top() -> u64` and `Box_::stack_size() -> u64` (used by Task 4); `retrace_arch::{SYS_SYSCTL, SYS_GETRLIMIT, CTL_KERN, KERN_USRSTACK64}`.

- [ ] **Step 1: Add the arch constants**

In `crates/retrace-arch/src/lib.rs`, alongside the existing `SYS_*` constants (after `SYS_SHARED_REGION_MAP_AND_SLIDE_2_NP` at `:18`):

```rust
pub const SYS_SYSCTL: u64 = 202;
pub const SYS_GETRLIMIT: u64 = 194;
/// sysctl top-level: `CTL_KERN` (`sys/sysctl.h`).
pub const CTL_KERN: u32 = 1;
/// `sys/sysctl.h:276` — "LP64 user stack query". Forwarding this hands the guest the HOST
/// process's ASLR'd stack address; retrace must answer it from the guest's own geometry.
pub const KERN_USRSTACK64: u32 = 59;
```

Extend the existing constant test in the same file (the `assert_eq!` block around `:72-74`) with:

```rust
        assert_eq!((SYS_SYSCTL, SYS_GETRLIMIT), (202, 194));
        assert_eq!((CTL_KERN, KERN_USRSTACK64), (1, 59));
```

- [ ] **Step 2: Add the stack geometry to `Box_` and `BoxState`**

In `crates/retrace-box/src/lib.rs`:

Add two fields to the `Box_` struct near `mmap_next` (declared at `:272`) — **append them, do not reorder existing fields** (`vcpu` must stay declared before `vm`):

```rust
    // The guest's OWN stack geometry, set at load. `kern.usrstack64` and `RLIMIT_STACK` are
    // answered from these rather than forwarded, so the guest is never told its stack lives at a
    // host address (M8-stack). Path-aware by construction: the static path maps one granule below
    // STACK_TOP_IPA, the dynamic path maps DYN_STACK_SIZE below DYN_STACK_TOP.
    stack_top: u64,
    stack_size: u64,
```

Add the matching public accessors in the `impl Box_` block:

```rust
    /// Top of the guest's stack (exclusive) — what `kern.usrstack64` must report.
    pub fn stack_top(&self) -> u64 { self.stack_top }
    /// Size of the guest's stack in bytes — what `RLIMIT_STACK` must report.
    pub fn stack_size(&self) -> u64 { self.stack_size }
```

Add the same two fields to `pub struct BoxState` (at `:323`), next to `mmap_next` at `:336`:

```rust
    pub stack_top: u64,
    pub stack_size: u64,
```

Set them in **all three** `Box_ { … }` construction literals:
- `:652` (static `load`) — `stack_top: STACK_TOP_IPA, stack_size: GRANULE as u64,`
- `:1072` (`load_dynamic`) — `stack_top: DYN_STACK_TOP, stack_size: DYN_STACK_SIZE,`
- `:1679` (`restore`) — `stack_top: DYN_STACK_TOP, stack_size: DYN_STACK_SIZE,`

Thread them through the `BoxState` capture (near `mmap_next: self.mmap_next` at `:1993`) and restore (near `mmap_next: state.mmap_next` at `:2052`):

```rust
            stack_top: self.stack_top,
            stack_size: self.stack_size,
```
```rust
            stack_top: state.stack_top,
            stack_size: state.stack_size,
```

> Note: `restore()` is the dynamic path (it rebuilds a `load_dynamic`-shaped box), which is why it uses the `DYN_*` constants. If the compiler reports a fourth `Box_ { … }` literal, set it to match whichever path it rebuilds.

- [ ] **Step 3: Add a box-level unit test for the geometry**

Append to the existing `#[cfg(test)] mod tests` in `crates/retrace-box/src/lib.rs`:

```rust
    #[test]
    fn static_load_records_its_own_stack_geometry() {
        let bytes = std::fs::read(retrace_guest::HELLO).unwrap();
        let loaded = retrace_guest::parse_macho(&bytes);
        let b = Box_::load(&loaded);
        assert_eq!(b.stack_top(), STACK_TOP_IPA, "static stack top");
        assert_eq!(b.stack_size(), GRANULE as u64, "static stack is exactly one granule");
        assert_eq!(b.stack_top() - b.stack_size(), STACK_TOP_IPA - GRANULE as u64,
                   "the computed stack bottom must equal the IPA load() actually maps");
    }
```

> If `Box_::load`'s signature differs from `load(&loaded)`, match the call shape used by the neighbouring tests in that module rather than inventing one.

- [ ] **Step 4: Add the record-side synthesis arm**

In `crates/retrace-core/src/lib.rs`, add a new arm **before** the catch-all `Stop::Syscall { num, args } =>` arm at `:376`, next to the existing `mprotect` arm at `:162`:

```rust
            // sysctl({CTL_KERN, KERN_USRSTACK64}): answer from the guest's OWN stack top. Forwarding
            // this returns RETRACE's host-process stack address (ASLR'd, different every run), which
            // the guest then uses as a guest address — libstd computes its guard page from it. That
            // is semantically wrong independent of determinism, exactly like M2-cpuid's TPIDR_EL0.
            // Every other mib keeps forwarding unchanged. Deterministic => STANDARD symmetric
            // posture: replay recomputes these same bytes and byte-compares (symmetry rule 1).
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SYSCTL
                && is_usrstack64_mib(&b, args) => {
                let writes = b.write_usrstack64_reply(args);
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes })
                    .map_err(|e| format!("append sysctl usrstack64: {e}"))?; count += 1;
                b.set_x0_err_and_return(0, false);
            }
```

Add these two helpers to `crates/retrace-core/src/lib.rs` at module scope (near the top, beside the other file-level helpers):

```rust
/// True if this `sysctl` is `{CTL_KERN, KERN_USRSTACK64}` — a 2-element mib read out of guest memory.
fn is_usrstack64_mib(b: &retrace_box::Box_, args: [u64; 8]) -> bool {
    if args[1] != 2 { return false; }
    let raw = b.read_guest(args[0], 8);
    if raw.len() < 8 { return false; }
    let name0 = u32::from_le_bytes(raw[0..4].try_into().unwrap());
    let name1 = u32::from_le_bytes(raw[4..8].try_into().unwrap());
    name0 == retrace_arch::CTL_KERN && name1 == retrace_arch::KERN_USRSTACK64
}
```

And on `Box_` in `crates/retrace-box/src/lib.rs`:

```rust
    /// Answer `sysctl({CTL_KERN, KERN_USRSTACK64})` from the guest's own stack top. Writes the u64
    /// into the guest's `oldp` and updates `*oldlenp`, returning both as recorded regions so replay
    /// recomputes and byte-compares them. `oldp == 0` is a size probe: only `*oldlenp` is written.
    pub fn write_usrstack64_reply(&mut self, args: [u64; 8]) -> Vec<Region> {
        let (oldp, oldlenp) = (args[2], args[3]);
        let mut out = Vec::new();
        if oldp != 0 {
            let bytes = self.stack_top.to_le_bytes().to_vec();
            self.write_guest(oldp, &bytes);
            out.push(Region { ipa: oldp, bytes });
        }
        if oldlenp != 0 {
            let bytes = 8u64.to_le_bytes().to_vec();
            self.write_guest(oldlenp, &bytes);
            out.push(Region { ipa: oldlenp, bytes });
        }
        out
    }
```

> **`write_guest` does not exist and must be added.** Verified: `retrace-box/src/lib.rs` has
> `read_guest(&self, ipa, len) -> Vec<u8>` at `:1818` and `read_guest_checked` at `:1832`, but no
> writer. Add one immediately after `read_guest`, reusing that function's backing-lookup loop
> verbatim and keeping its fail-loud behaviour on an unbacked address:
>
> ```rust
>     /// Write `bytes` into guest memory at `ipa`. The mirror of [`read_guest`](Self::read_guest);
>     /// panics identically on an address no backing covers (fail loud — a silent no-op here would
>     /// surface later as an unexplained divergence).
>     pub fn write_guest(&mut self, ipa: u64, bytes: &[u8]) {
>         for bk in &self.backings {
>             if ipa >= bk.ipa && ipa + bytes.len() as u64 <= bk.ipa + bk.len as u64 {
>                 let off = (ipa - bk.ipa) as usize;
>                 unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), bk.host.add(off), bytes.len()); }
>                 return;
>             }
>         }
>         panic!("write_guest: no backing covers {ipa:#x}..{:#x}", ipa + bytes.len() as u64);
>     }
> ```
>
> Match `read_guest`'s actual panic/return convention at `:1818-1831` if it differs from this sketch.

- [ ] **Step 5: Add the mirrored replay arm**

Symmetry rule 1: without this, replay diverges. In `crates/retrace-core/src/lib.rs`'s replay dispatch, alongside the existing anon-mmap mirror at `:589` and the `mprotect` mirror at `:672`:

```rust
                            // sysctl(KERN_USRSTACK64) mirror: recompute the SAME reply from the
                            // box's own stack geometry and byte-compare it against the recording —
                            // that comparison IS the divergence check (symmetry rule 1).
                            if num == retrace_arch::SYS_SYSCTL && is_usrstack64_mib(&self.b, *args) {
                                let recomputed = self.b.write_usrstack64_reply(*args);
                                if &recomputed != writes {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!(
                                            "sysctl usrstack64 reply mismatch: replay {recomputed:?} != recorded {writes:?}") });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
```

> Control flow verified against the anon-mmap mirror at `:589-597`: these arms end with
> `return self.finish_event();`, **not** `continue`. `Divergence` is
> `{ landmark: usize, pc: u64, detail: String }` (`retrace-core/src/lib.rs:393`) — all three fields
> are required; `self.idx` and the in-scope `pc` are what the neighbouring arms pass.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p retrace --test usrstack_e2e -- --test-threads=1 --nocapture`
Expected: `usrstack64_reports_the_guests_own_stack_top` now PASSES; `rlimit_stack_…` and `anonymous_map_fixed_…` still FAIL; `usrstack_replays_bit_for_bit` still PASSES (proves the mirror is correct — if it now fails, the replay arm is asymmetric).

Run: `cargo test -p retrace-box -- --test-threads=1`
Expected: `static_load_records_its_own_stack_geometry` PASSES.

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-arch/src/lib.rs crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs
git commit -m "M8-stack t3: synthesize kern.usrstack64 from the guest's own stack top

Box_ now carries its stack geometry (stack_top/stack_size), set at load and
threaded through BoxState, so the answer is path-aware: the static path's
stack is one granule below STACK_TOP_IPA, the dynamic path's is
DYN_STACK_SIZE below DYN_STACK_TOP. Hardcoding either would make the other
path lie.

sysctl({CTL_KERN, KERN_USRSTACK64}) is now answered from that geometry
instead of forwarded. Standard symmetric posture: the replay arm recomputes
the identical reply and byte-compares it.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Synthesize `getrlimit(RLIMIT_STACK)`

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs`
- Modify: `crates/retrace-box/src/lib.rs`
- Modify: `crates/retrace-core/src/lib.rs`

**Interfaces:**
- Consumes: `Box_::stack_size()` from Task 3.
- Produces: `Box_::write_rlimit_stack_reply(args) -> Vec<Region>`.

- [ ] **Step 1: Add the constants**

In `crates/retrace-arch/src/lib.rs`:

```rust
/// `sys/resource.h:446`.
pub const RLIMIT_STACK: u64 = 3;
/// `sys/resource.h:458` — libc ORs this in for strict-POSIX `getrlimit`; the guest is observed
/// passing `0x1003`, so the resource must be masked before comparison.
pub const RLIMIT_POSIX_FLAG: u64 = 0x1000;
```

Extend the constant test:

```rust
        assert_eq!((RLIMIT_STACK, RLIMIT_POSIX_FLAG), (3, 0x1000));
```

- [ ] **Step 2: Add the box helper**

In `crates/retrace-box/src/lib.rs`:

```rust
    /// Answer `getrlimit(RLIMIT_STACK)` from the guest's own stack size. `struct rlimit` is two
    /// u64s: `{ rlim_cur, rlim_max }`. Both report the real mapped size — retrace does not grow
    /// the guest stack on demand, so a larger `rlim_max` would be a lie the guest could act on.
    pub fn write_rlimit_stack_reply(&mut self, args: [u64; 8]) -> Vec<Region> {
        let rlp = args[1];
        if rlp == 0 { return Vec::new(); }
        let mut bytes = self.stack_size.to_le_bytes().to_vec();
        bytes.extend_from_slice(&self.stack_size.to_le_bytes());
        self.write_guest(rlp, &bytes);
        vec![Region { ipa: rlp, bytes }]
    }
```

- [ ] **Step 3: Add the record arm**

In `crates/retrace-core/src/lib.rs`, beside the Task 3 arm:

```rust
            // getrlimit(RLIMIT_STACK): answer from the guest's own stack size. Forwarding returns
            // the HOST's limit (8176 KiB), which libstd subtracts from usrstack64 to locate its
            // guard page — the two must describe the SAME stack or the result is a wild address.
            // The guest passes RLIMIT_STACK | _RLIMIT_POSIX_FLAG (0x1003), so mask before compare.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_GETRLIMIT
                && (args[0] & !retrace_arch::RLIMIT_POSIX_FLAG) == retrace_arch::RLIMIT_STACK => {
                let writes = b.write_rlimit_stack_reply(args);
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes })
                    .map_err(|e| format!("append getrlimit stack: {e}"))?; count += 1;
                b.set_x0_err_and_return(0, false);
            }
```

- [ ] **Step 4: Add the mirrored replay arm**

```rust
                            // getrlimit(RLIMIT_STACK) mirror — see the usrstack64 mirror above.
                            if num == retrace_arch::SYS_GETRLIMIT
                                && (args[0] & !retrace_arch::RLIMIT_POSIX_FLAG) == retrace_arch::RLIMIT_STACK {
                                let recomputed = self.b.write_rlimit_stack_reply(*args);
                                if &recomputed != writes {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!(
                                            "getrlimit stack reply mismatch: replay {recomputed:?} != recorded {writes:?}") });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p retrace --test usrstack_e2e -- --test-threads=1 --nocapture`
Expected: `rlimit_stack_reports_the_guests_own_stack_size` now PASSES; `anonymous_map_fixed_…` still FAILS; `usrstack_replays_bit_for_bit` still PASSES.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-arch/src/lib.rs crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs
git commit -m "M8-stack t4: synthesize getrlimit(RLIMIT_STACK) from the guest's stack size

usrstack64 and RLIMIT_STACK must describe the SAME stack -- libstd subtracts
one from the other to locate its guard page, so a host limit against a guest
stack top yields a wild address. Masks _RLIMIT_POSIX_FLAG: the guest is
observed passing 0x1003, not 3. Mirrored replay arm, symmetric posture.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Honor `addr`/`MAP_FIXED` in the anonymous mmap path

The file-backed path already does this correctly via `map_mmap_region` (`retrace-box/src/lib.rs:1312-1321`, FIXED handled at `:1316`). The anonymous path never received the same treatment: `guest_mmap` takes only a length.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs:1291-1298`
- Modify: `crates/retrace-core/src/lib.rs:133` (record) and `:589` (replay mirror)

**Interfaces:**
- Produces: `Box_::guest_mmap(addr: u64, len: u64, prot: u64, flags: u64) -> u64` (was `guest_mmap(len)`).

- [ ] **Step 1: Find every caller before changing the signature**

Run: `grep -rn "guest_mmap(" crates/ --include="*.rs"`

Expected: the record site (`retrace-core/src/lib.rs:140`), the replay mirror (`:590`), the definition (`retrace-box/src/lib.rs:1291`), and possibly box-level tests. Update every one. If a caller exists that this plan does not anticipate, update it to pass the four arguments rather than adding a wrapper.

- [ ] **Step 2: Widen `guest_mmap`**

Replace `crates/retrace-box/src/lib.rs:1289-1298` with:

```rust
    /// Special case for anonymous mmap: allocate host pages, map them at a deterministic guest IPA,
    /// track as a backing, return the guest IPA. Same call sequence => same IPAs on replay.
    ///
    /// M8-stack: `addr`/`flags` now reach this function. Previously it took only a length and always
    /// bump-allocated, so an anonymous MAP_FIXED request silently landed at `mmap_next` — libstd's
    /// guard-page install checks `result != stackptr` and panics with errno untouched
    /// ("os error 0"). Placement is delegated to `map_mmap_region`, which the file-backed path
    /// already uses, so both paths now share one FIXED implementation.
    pub fn guest_mmap(&mut self, addr: u64, len: u64, prot: u64, flags: u64) -> u64 {
        let (host, rlen) = alloc_pages(len as usize);
        self.map_mmap_region(host, rlen, addr, prot, flags)
    }
```

- [ ] **Step 3: Make `map_mmap_region` unmap an overlapping FIXED range**

A `MAP_FIXED` request may target already-mapped guest memory (the kernel silently replaces it there). `hv_vm_map` over a live mapping fails, so drop the overlap first — the same thing `guest_vm_map`'s FIXED path already does at `:1157`. In `map_mmap_region`, replace the `let ipa = …` line at `:1316` with:

```rust
        let ipa = if flags & Self::MAP_FIXED != 0 {
            // FIXED may land on already-mapped guest memory; the kernel replaces it silently and
            // hv_vm_map refuses to overlap, so drop the overlapping backing(s) first.
            self.unmap_overlapping(addr, rlen as u64);
            addr
        } else { self.mmap_next };
```

- [ ] **Step 4: Update the record call site**

`crates/retrace-core/src/lib.rs:140` becomes:

```rust
                let ipa = b.guest_mmap(args[0], args[1], args[2], args[3]);
```

- [ ] **Step 5: Update the replay mirror**

`crates/retrace-core/src/lib.rs:590` becomes:

```rust
                                let ipa = self.b.guest_mmap(args[0], args[1], args[2], args[3]);
```

The existing `ipa != *ret` divergence check immediately below it is what enforces symmetry — leave it in place.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p retrace --test usrstack_e2e --test determinism -- --test-threads=1 --nocapture`
Expected: all four `usrstack_e2e` tests PASS, and `usrstack_records_deterministically` PASSES.

Run the guests most likely to regress from a placement change:
`cargo test -p retrace --test e2e --test remap_e2e --test mmapfile_e2e --test execmap_e2e --test carveout_e2e --test reservecommit_e2e -- --test-threads=1`
Expected: all PASS. A failure here means the FIXED/ANYWHERE split changed placement for a guest that depended on the old behaviour — diagnose before proceeding, do not adjust the tests.

- [ ] **Step 7: Run the full gate**

Run: `just gate`
Expected: green, clippy clean. Compare the tally against the 146 baseline plus the tests added in Tasks 1–2.

- [ ] **Step 8: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs
git commit -m "M8-stack t5: honor addr/MAP_FIXED for anonymous mmap

guest_mmap took only a length and always bump-allocated from MMAP_BASE, so an
anonymous MAP_FIXED request could never return the requested address -- it was
arithmetically impossible, since MMAP_BASE is 0xA_0000_0000 and only increases.
libstd checks `result != stackptr` and panics with errno untouched, which is
the 'Undefined error: 0 (os error 0)' signature.

Placement now delegates to map_mmap_region, which the file-backed path already
used, so both paths share one FIXED implementation. FIXED additionally unmaps
an overlapping backing first, matching guest_vm_map's FIXED path.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Re-evaluate the `hello_rust` headline gate and document the outcome

The fix is complete; this task finds out where `hello_rust` actually lands and records that honestly. **Both outcomes are legitimate.** Spec risk R1 says walls come in chains, and `sigaltstack`/signal delivery sits immediately downstream in libstd's init.

**Files:**
- Modify: `crates/retrace/tests/hello_rust_e2e.rs`
- Modify: `README.md` (append an M8 Status section)
- Modify: `CLAUDE.md` (only if a command or invariant changed)

- [ ] **Step 1: Find out where `hello_rust` gets to**

Run:
```bash
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn \
  "$(ls -t target/aarch64-apple-darwin/debug/build/retrace-guest-*/out/hello_rust | head -1)" \
  -o /tmp/m8-hello-rust.bin 2>&1 | tail -40
```

Record the exit code and the last traps. Exit 0 with `hi from rust\n` means the wall is cleared.

- [ ] **Step 2A: If the guest now reaches `main` — un-ignore the gate**

Remove the `#[ignore = "…"]` attribute from `crates/retrace/tests/hello_rust_e2e.rs:10-22`, leaving the test and its explanatory header comment intact.

Verify a **genuine double pass** — the spec's exit criterion, not a single lucky run:
```bash
cargo test -p retrace --test hello_rust_e2e -- --test-threads=1
cargo test -p retrace --test hello_rust_e2e -- --test-threads=1
```
Both must pass. If either fails, this is **not** a pass: restore the `#[ignore]` and go to Step 2B.

- [ ] **Step 2B: If it re-parks — rewrite the ignore reason to the NEW signature**

Replace the `#[ignore = "…"]` text with the new wall's evidence: the failing syscall/trap, the guest's own error text if any, whether there is an HVF fault (`pc`/`esr`/`far`) or none, and why it is a different mechanism from the guard-page wall. Delete the old guard-page text entirely — a stale reason is worse than none. Keep the `See docs/superpowers/specs/…` pointer, updated to the M8 spec.

- [ ] **Step 3: Run the full gate and capture the real tally**

Run: `just gate 2>&1 | tail -40`

Read the tally from the raw output. Grep on the gate log can be confused by ANSI colour codes — read the tail directly rather than grepping for `passed`.

- [ ] **Step 4: Write the README M8 Status section**

Append a `## Status: M8-stack — guest stack identity ✅` section to `README.md`, following the structure of the M7 section at `:1153`. It must state:
- what the milestone proved and fixed (both defects, with the closing arithmetic);
- that the defect was *semantically* wrong, not merely nondeterministic, and how that squares with M2-cpuid's position on forwarded-syscall variance;
- the new address-space oracle, what it does and does not compare, and why address-shaped rather than byte-identical;
- the real `just gate` tally from Step 3;
- where `hello_rust` stands now — green or re-parked, with the new boundary named;
- a "Deferred / the next boundary" list carrying forward: signal delivery, threads, stack *size* (R3), arm64e dynamic guests, and rung 2 (`brew jq`).

- [ ] **Step 5: Final gate**

Run: `just gate`
Expected: green, clippy clean, and the tally in the README matches what the gate actually printed.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace/tests/hello_rust_e2e.rs README.md CLAUDE.md
git commit -m "M8-stack t6: re-evaluate the hello_rust gate + README M8 Status

<Either: 'hello_rust now reaches main -- gate UN-IGNORED, verified by a genuine
double pass' OR 'rung 1 re-parked at <new wall>; ignore reason rewritten to the
new signature, old guard-page text deleted per honest-gate discipline'.>

just gate: <real tally>, clippy clean.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage:** Mechanism A (synthesize `KERN_USRSTACK64`) → Task 3. Mechanism B (synthesize `RLIMIT_STACK`) → Task 4. Mechanism C (anon `MAP_FIXED`) → Task 5. Determinism posture / symmetry rule 1 → mirrored replay arms in Tasks 3, 4, 5, with `usrstack_replays_bit_for_bit` as the standing check. Address-space oracle → Task 1. `usrstack.s` fixture → Task 2. Exit criteria 1–3 → Tasks 1, 2, 5. Exit criterion 4 (honest gate) → Task 6. Open question 1 (`hello_dyn` leak?) → Task 1 Step 3. Open question 2 (where the projection lives) → resolved: the test harness, per the spec's stated preference. Open question 3 (`guest_mmap` callers) → Task 5 Step 1. Stack geometry decision (unchanged, 256 KiB) → encoded in Task 3's path-aware fields rather than a constant change. No spec requirement is unassigned.

**Placeholder scan:** No TBD/TODO. Every code step carries real code. Three steps are deliberately conditional rather than placeholder — Task 1 Step 3 (calibration has two defined outcomes with defined follow-ups), Task 6 Steps 2A/2B (the gate outcome is genuinely unknown until measured, which is the point of honest-gate discipline), and three "if the surrounding code differs, match it" notes where I could not verify an exact control-flow shape without reading code the implementer will have open.

**Type consistency:** `stack_top`/`stack_size` are `u64` at every site — `Box_` fields, `BoxState` fields, accessors, and both helpers. `write_usrstack64_reply` and `write_rlimit_stack_reply` both take `[u64; 8]` and return `Vec<Region>`, matching `Event::Syscall.writes`. `guest_mmap(addr, len, prot, flags) -> u64` is used identically at both call sites. `address_projection` returns `Vec<(usize, &'static str, u64)>` and is consumed only by `assert_address_determinism`. `is_usrstack64_mib` takes `(&Box_, [u64; 8])` in the record arm and `(&self.b, *args)` in the replay arm — same types.

**Soft spots, checked and resolved.** All three were verified against the source after the first draft; two of the original sketches were wrong and are now corrected in place:
- **Replay-arm control flow — was wrong, fixed.** The sketches used `continue` and a one-field `Divergence`. Verified against the anon-mmap mirror at `retrace-core/src/lib.rs:589-597`: these arms `return self.finish_event();`, and `Divergence` is `{ landmark: usize, pc: u64, detail: String }` (`:393`). Tasks 3 and 4 now carry the correct shape.
- **`Box_::write_guest` — confirmed absent.** Only `read_guest` (`:1818`) and `read_guest_checked` (`:1832`) exist. Task 3 Step 4 now includes the full function to add.
- **`unmap_overlapping` — confirmed correct.** `fn unmap_overlapping(&mut self, ipa: u64, len: u64)` at `:1245`, exactly as Task 5 Step 3 assumes.

One genuine unknown remains, and it is unknowable before implementation: whether `Box_::load`'s signature matches the `Box_::load(&loaded)` call shape used in Task 3 Step 3's unit test. The step says to match the neighbouring tests in that module rather than invent a shape.
