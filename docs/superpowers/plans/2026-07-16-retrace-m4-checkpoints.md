# M4 Implementation Plan — checkpointed reverse-execution seeks

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Accelerate `retrace debug`'s reverse-execution seeks by caching mid-run guest state so repeated single-stepping near a previously-visited position resumes from there instead of re-walking from the trace's landmark-0 snapshot every time.

**Architecture:** A new in-memory-only `BoxState` captures `Box_`'s COMPLETE internal state (unlike the trace-format snapshot, which is only correct to restore at landmark 0). A `CheckpointCache` (owned by the CLI's `Exec`) stores `SessionCheckpoint`s keyed by trace-execution-order position `(N, K)`, gated on single-step cost so only genuinely-expensive-to-reach positions get cached, with byte-budget + LRU eviction. `checkpointed_seek` replaces every raw `seek()` call in `debug.rs`; on a miss it falls back to the existing cold path, so correctness never depends on the cache. **Zero trace-format changes.** Spec: `docs/superpowers/specs/2026-07-16-retrace-m4-checkpoints-design.md` — read it before starting.

**Tech Stack:** Rust workspace; Hypervisor.framework via `hv-sys`; arm64 guests built by `crates/retrace-guest/build.rs`.

## Global Constraints

- **Branch:** create `m4-checkpoints` from `main` (spec is on main at `4da7b22`): `git checkout -b m4-checkpoints`.
- **Every test run uses `--test-threads=1`** (HVF: one VM per process). Full gate: `just gate`. **Baseline: 97 passed / 0 failed / 0 ignored, clippy clean** (confirmed by running `just gate` on `main` at HEAD `9267b7a` before this plan was written).
- **One VM per process, even inside a single test:** never hold two `Box_`/`ReplaySession` values alive at once — capture what you need, **drop the first, then open the second**. Two live sessions = `HV_BUSY`.
- **Zero trace-format changes.** Do not touch `retrace-trace`'s `Event` or `TRACE_MAGIC`. `BoxState`/`SessionCheckpoint`/`CheckpointCache` are in-memory-only, session-scoped, never persisted.
- **Never fake a green.** If FP/SIMD register access under HVF does not behave as documented (unexpected, but the codebase's honest-gate discipline applies regardless), stop and report the exact boundary rather than loosening an assertion.
- **Clippy `-D warnings` clean at every commit.** Codesigning is automatic for `cargo test`/`cargo run`; `CARGO_BIN_EXE_retrace`-spawned binaries sign manually (`crates/retrace/tests/util/mod.rs::bin()` already does this).
- **Commit messages:** `M4 tN: <what>` + trailing `Co-Authored-By: <executing model> <noreply@anthropic.com>` (match the executing model — do not hardcode a specific one).

### Exact values (verbatim — copy, do not reinvent)

- **`BoxState`'s field list**, cross-checked field-by-field against the real `Box_` struct (`crates/retrace-box/src/lib.rs:207-252`): `regs: Regs` (x0-x30, pc, sp_el0, cpsr — the existing trace-format type), `fp: [u128; 32]` (NEW — V0-V31), `fpcr: u64`, `fpsr: u64` (NEW), `tpidr_el0: u64` (captured, not forced to 0 like `Box_::restore` does), `elr: u64`, `spsr: u64` (ELR_EL1/SPSR_EL1 — added during Task 2, load-bearing at syscall-trap landmarks; the original list omitted them), `mem: Vec<Region>` (full backing contents, existing trace-format type), `reservations: Vec<(u64, u64)>` (matches `Box_`'s own field type exactly — a plain tuple, NOT a named struct), `mmap_next: u64`, `bootstrap_port: Option<u32>` (note: **`u32`, not `u64`** — `Box_::bootstrap_port` is `Option<u32>`), `cache_installed: bool` (NOT the `CacheMeta` itself — it owns a `File` handle, not `Clone`), `last_far: u64`, `synthetic_tsc: u64`, `cache_refault_ipa: u64`, `cache_refault_count: u64`. `l2_host`/`next_l3` are deliberately **excluded** — they're host pointers, recomputed post-restore from the freshly-allocated `backings`, exactly as `Box_::restore` (`:1495-1507`) already does.
- **hv-sys FP/SIMD raw bindings already exist** (only the safe wrapper is missing): `hv_reg_t_HV_REG_FPCR = 32`, `hv_reg_t_HV_REG_FPSR = 33` (ordinary `hv_reg_t` values, readable/writable via the *already-wrapped* `hv_vcpu_get_reg`/`hv_vcpu_set_reg`). `hv_simd_fp_reg_t_HV_SIMD_FP_REG_Q0..Q31 = 0..31` (contiguous, like `HV_REG_X0..X30`). `hv_simd_fp_uchar16_t = u128`. Functions: `hv_vcpu_get_simd_fp_reg(vcpu: hv_vcpu_t, reg: hv_simd_fp_reg_t, value: *mut hv_simd_fp_uchar16_t) -> hv_return_t` and `hv_vcpu_set_simd_fp_reg(vcpu: hv_vcpu_t, reg: hv_simd_fp_reg_t, value: hv_simd_fp_uchar16_t) -> hv_return_t`.
- **`spinloop.s`'s exact instruction counts** (verify in Task 3; used by every later task's coordinates): window 1 (landmark 1, up to the `write` syscall) = 606 instructions (`mov x0,#300` + 300×(`subs`+`b.ne`) + 5 more before `svc`). Window 2 (landmark 2, up to `exit`) = 4003 instructions (`mov x0,#2000` + 2000×(`subs`+`b.ne`) + 2 more before `svc`). If your assembled count differs (e.g. the assembler pads differently), **discover the true counts via `window_len_here()` in Task 4's first test and adjust every hardcoded K value in Tasks 4-6 to match** — do not silently clamp or guess.
- **Production cache constants** (Task 6, `crates/retrace/src/debug.rs`): `CHECKPOINT_BYTE_BUDGET: usize = 256 * 1024 * 1024` (256 MiB), `CHECKPOINT_COST_GATE_STEPS: u64 = 64`. Tests use their own constructor arguments — these two are the CLI's production values only.
- **Gate arithmetic** (from baseline 97/0/0): T1 **98/0/0** (+1 hv-sys test). T2 **99/0/0** (+1). T3 **100/0/0** (+1 guest-parse smoke test). T4 **102/0/0** (+2). T5 **103/0/0** (+1). T6 **104/0/0** (+1; existing `debug_cli.rs`/`reverse_debug_e2e.rs` tests must pass UNMODIFIED). T7: **104/0/0** (no new tests, final confirmation).

---

### Task 1: hv-sys FP/SIMD register wrappers

**Files:**
- Modify: `crates/hv-sys/src/lib.rs` — `reg::FPCR`/`reg::FPSR`, a new `SimdReg` type + `simd` module, `Vcpu::get_simd`/`set_simd`.
- Test: `crates/hv-sys/tests/simd.rs`

**Interfaces:**
- Consumes: nothing new (mirrors the existing `reg`/`sysreg`/`Vcpu::get_reg`/`set_reg` pattern at `crates/hv-sys/src/lib.rs:42-51,106-109`).
- Produces (Task 2 relies on these exact names):

```rust
pub mod reg { pub const FPCR: Reg; pub const FPSR: Reg; /* existing PC/CPSR/x(n) unchanged */ }
#[derive(Clone, Copy)] pub struct SimdReg(pub hv_simd_fp_reg_t);
pub mod simd { pub fn q(n: u32) -> SimdReg; }
impl Vcpu {
    pub fn get_simd(&self, r: SimdReg) -> Result<u128, HvError>;
    pub fn set_simd(&self, r: SimdReg, v: u128) -> Result<(), HvError>;
}
```

- [ ] **Step 1: Write the failing test.** Create `crates/hv-sys/tests/simd.rs`, mirroring `crates/hv-sys/tests/sysregs.rs`'s exact shape:

```rust
use hv_sys::{Vm, Vcpu, reg, simd};

// FPCR/FPSR (ordinary Reg values) and the V0-V31 SIMD/FP registers must be settable and read back
// on a real vCPU — the M4 checkpoint machinery depends on this to capture/restore live NEON state
// across a mid-run checkpoint (dyld's early init uses NEON for memcpy/hashing).
#[test]
fn fp_and_simd_regs_roundtrip() {
    let vm = Vm::create().unwrap();
    let vcpu = Vcpu::create(&vm).unwrap();
    // DN (bit 25) + FZ (bit 24): defined, always-implemented FPCR fields.
    vcpu.set_reg(reg::FPCR, 0x0300_0000).unwrap();
    assert_eq!(vcpu.get_reg(reg::FPCR).unwrap(), 0x0300_0000);
    // IOC (bit 0): a defined, writable FPSR cumulative-exception flag.
    vcpu.set_reg(reg::FPSR, 0x0000_0001).unwrap();
    assert_eq!(vcpu.get_reg(reg::FPSR).unwrap(), 0x0000_0001);
    for n in [0u32, 15, 31] {
        let v: u128 = 0x0102_0304_0506_0708_090A_0B0C_0D0E_0F00 | n as u128;
        vcpu.set_simd(simd::q(n), v).unwrap();
        assert_eq!(vcpu.get_simd(simd::q(n)).unwrap(), v, "Q{n} did not round-trip");
    }
}
```

- [ ] **Step 2: Verify failure.** `cargo test -p hv-sys --test simd -- --test-threads=1` — Expected: FAIL to compile (`no FPCR in reg`, `no simd module`).

- [ ] **Step 3: Implement.** In `crates/hv-sys/src/lib.rs`, add to the `reg` module (after the `CPSR` const, before `pub fn x`, matching lines 44-51):

```rust
    pub const FPCR: Reg = Reg(hv_reg_t_HV_REG_FPCR);
    pub const FPSR: Reg = Reg(hv_reg_t_HV_REG_FPSR);
```

After the `sysreg` module (after line 95's closing `}`), add:

```rust
#[derive(Clone, Copy)] pub struct SimdReg(pub hv_simd_fp_reg_t);
pub mod simd {
    use super::*;
    pub fn q(n: u32) -> SimdReg { SimdReg(hv_simd_fp_reg_t_HV_SIMD_FP_REG_Q0 + n) } // Q0..Q31 contiguous
}
```

In `impl Vcpu` (after `get_sys`/`set_sys`, around line 109, before `set_trap_debug_exceptions`):

```rust
    pub fn get_simd(&self, r: SimdReg) -> Result<u128, HvError> {
        let mut v: u128 = 0;
        check(unsafe { hv_vcpu_get_simd_fp_reg(self.id, r.0, &mut v) })?;
        Ok(v)
    }
    pub fn set_simd(&self, r: SimdReg, v: u128) -> Result<(), HvError> {
        check(unsafe { hv_vcpu_set_simd_fp_reg(self.id, r.0, v) })
    }
```

- [ ] **Step 4: Run the test.** `cargo test -p hv-sys --test simd -- --test-threads=1` — Expected: PASS (1 test).

- [ ] **Step 5: Full gate.** `just gate` — Expected: **98 / 0 / 0**, clippy clean.

- [ ] **Step 6: Commit.**

```bash
git add crates/hv-sys/src/lib.rs crates/hv-sys/tests/simd.rs
git commit -m "M4 t1: hv-sys FPCR/FPSR/SIMD register wrappers

Co-Authored-By: <executing model> <noreply@anthropic.com>"
```

---

### Task 2: `BoxState` — full internal-state capture/restore in retrace-box

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` — imports, `BoxState` struct, `Box_::checkpoint()`, `Box_::from_checkpoint()`, `Box_::dbg_fp_regs()`, `Box_::dbg_internal_state()`.
- Test: `crates/retrace-box/tests/checkpoint.rs`

**Interfaces:**
- Consumes: Task 1's `hv_sys::simd`/`Vcpu::get_simd`/`set_simd`/`reg::FPCR`/`reg::FPSR`.
- Produces (Task 4 relies on these exact names):

```rust
#[derive(Clone)]
pub struct BoxState { /* see the Exact Values field list above; all fields pub */ }
impl Box_ {
    pub fn checkpoint(&self) -> BoxState;
    pub fn from_checkpoint(state: &BoxState) -> Box_;
    pub fn dbg_fp_regs(&self) -> String;          // V0..V31 + FPCR/FPSR, for test byte-compares
    #[doc(hidden)] pub fn dbg_internal_state(&self) -> String; // reservations/mmap_next/etc, test-only
}
```

- [ ] **Step 1: Write the failing test.** Create `crates/retrace-box/tests/checkpoint.rs`:

```rust
// Box_::checkpoint()/from_checkpoint() round-trip: a MID-RUN capture (not landmark-0, where
// Box_::restore's defaults would coincidentally look correct) must restore byte-identical state
// with zero further execution — registers (incl. FP/SIMD), all memory, and the internal bookkeeping
// (reservations/mmap_next/bootstrap_port/cache_installed/...) that Box_::restore only gets right at
// landmark 0.
use retrace_box::{Box_, Stop};
use retrace_guest::{parse_macho, STEPPY};

#[test]
fn checkpoint_round_trip_is_lossless_mid_run() {
    let loaded = parse_macho(&std::fs::read(STEPPY).unwrap());
    let mut b = Box_::load(&loaded);

    // Exercise non-default internal state via Box_'s own public bump-allocator/cache/port methods
    // (no real syscall forwarding needed — these are exactly what record/replay's dispatch calls).
    let _reserved = b.guest_vm_reserve(0x9999_0000, 0x4000, true);
    let _mapped = b.guest_mmap(0x4000);
    b.install_cache_pager();
    let _port = b.mint_bootstrap_port();
    // Single-step a few instructions (crosses steppy's timebase-emulated MRS, advancing
    // synthetic_tsc — the field most likely to be silently reset by a naive restore).
    for i in 1..=5u64 {
        assert!(matches!(b.step(), Stop::Step), "step {i}");
    }

    let original_regs = b.dbg_regs();
    let original_fp = b.dbg_fp_regs();
    let original_internal = b.dbg_internal_state();
    let original_mem = match b.snapshot() {
        retrace_trace::Event::Snapshot { mem, .. } => mem,
        _ => unreachable!(),
    };

    let checkpoint = b.checkpoint();
    let restored = Box_::from_checkpoint(&checkpoint);

    assert_eq!(restored.dbg_regs(), original_regs, "registers diverged on restore");
    assert_eq!(restored.dbg_fp_regs(), original_fp, "FP/SIMD state diverged on restore");
    assert_eq!(restored.dbg_internal_state(), original_internal,
        "internal bookkeeping (reservations/mmap_next/bootstrap_port/cache/...) diverged on restore");
    assert!(restored.diff_memory(&original_mem).is_none(), "memory diverged on restore");
}
```

- [ ] **Step 2: Verify failure.** `cargo test -p retrace-box --test checkpoint -- --test-threads=1` — Expected: FAIL to compile (`no method checkpoint`, `no method dbg_fp_regs`, `no method dbg_internal_state`).

- [ ] **Step 3: Implement.** In `crates/retrace-box/src/lib.rs`, change the import at line 1 to add `simd`:

```rust
use hv_sys::{Vm, Vcpu, reg, sysreg, simd, MemFlags, EXIT_EXCEPTION};
```

After the `Stop` enum (line 255), add:

```rust
/// A complete, in-memory-only capture of `Box_`'s internal state at an ARBITRARY position — unlike
/// `Event::Snapshot` (the trace format), which is only correct to restore from at landmark 0.
/// Never persisted, never enters a trace file. See the M4 design spec for why each field is here.
#[derive(Clone)]
pub struct BoxState {
    pub regs: Regs,
    pub fp: [u128; 32],
    pub fpcr: u64,
    pub fpsr: u64,
    pub tpidr_el0: u64,
    pub mem: Vec<Region>,
    pub reservations: Vec<(u64, u64)>,
    pub mmap_next: u64,
    pub bootstrap_port: Option<u32>,
    pub cache_installed: bool,
    pub last_far: u64,
    pub synthetic_tsc: u64,
    pub cache_refault_ipa: u64,
    pub cache_refault_count: u64,
}
```

At the end of `impl Box_` (after `describe_stop`, near line 1740), add:

```rust
    /// Capture COMPLETE internal state at the current (arbitrary) position — the M4 checkpoint
    /// primitive. Unlike `snapshot()` (trace format, landmark-0-only-correct on restore), this
    /// additionally captures FP/SIMD state and the true values of every field `restore()` defaults.
    pub fn checkpoint(&self) -> BoxState {
        let mut mem = Vec::new();
        for bk in &self.backings {
            let bytes = unsafe { std::slice::from_raw_parts(bk.host, bk.len) }.to_vec();
            mem.push(Region { ipa: bk.ipa, bytes });
        }
        let mut x = [0u64; 31];
        for (i, xi) in x.iter_mut().enumerate() { *xi = self.vcpu.get_reg(reg::x(i as u32)).unwrap(); }
        let regs = Regs {
            x, pc: self.vcpu.get_reg(reg::PC).unwrap(),
            sp_el0: self.vcpu.get_sys(sysreg::SP_EL0).unwrap(),
            cpsr: self.vcpu.get_reg(reg::CPSR).unwrap(),
        };
        let mut fp = [0u128; 32];
        for (i, fi) in fp.iter_mut().enumerate() { *fi = self.vcpu.get_simd(simd::q(i as u32)).unwrap(); }
        BoxState {
            regs, fp,
            fpcr: self.vcpu.get_reg(reg::FPCR).unwrap(),
            fpsr: self.vcpu.get_reg(reg::FPSR).unwrap(),
            tpidr_el0: self.vcpu.get_sys(sysreg::TPIDR_EL0).unwrap(),
            mem,
            reservations: self.reservations.clone(),
            mmap_next: self.mmap_next,
            bootstrap_port: self.bootstrap_port,
            cache_installed: self.cache.is_some(),
            last_far: self.last_far,
            synthetic_tsc: self.synthetic_tsc,
            cache_refault_ipa: self.cache_refault_ipa,
            cache_refault_count: self.cache_refault_count,
        }
    }

    /// Rebuild a fresh guest from a `BoxState` captured at an ARBITRARY position — the M4 twin of
    /// `restore()` (which is only correct at landmark 0). Mirrors `restore()`'s structure exactly
    /// (fresh VM/vcpu, remap every backing, fixed EL1 sysregs, PAC keys, `set_trap_debug_exceptions`)
    /// but restores the TRUE captured values instead of `restore()`'s landmark-0 defaults for
    /// `reservations`/`mmap_next`/`bootstrap_port`/`cache`/`tpidr_el0`, plus FP/SIMD state `restore()`
    /// never touched at all. `l2_host`/`next_l3` are recomputed from the freshly-allocated `backings`
    /// — never stored raw (they are host pointers, meaningless across VM instances).
    pub fn from_checkpoint(state: &BoxState) -> Box_ {
        let vm = Vm::create().expect("hv_vm_create");
        let vcpu = Vcpu::create(&vm).expect("hv_vcpu_create");
        let mut backings = Vec::new();
        for r in &state.mem {
            let (host, len) = alloc_pages(r.bytes.len().max(GRANULE));
            unsafe { std::ptr::copy_nonoverlapping(r.bytes.as_ptr(), host, r.bytes.len()); }
            vm.map(host, r.ipa, len, MemFlags::RWX).expect("hv_vm_map (from_checkpoint)");
            backings.push(Backing { host, ipa: r.ipa, len });
        }
        vcpu.set_sys(sysreg::MAIR_EL1,  MAIR_EL1_V).unwrap();
        vcpu.set_sys(sysreg::TCR_EL1,   TCR_EL1_V).unwrap();
        vcpu.set_sys(sysreg::TTBR0_EL1, PT_L1_IPA).unwrap();
        vcpu.set_sys(sysreg::CPACR_EL1, CPACR_FP_ON).unwrap();
        vcpu.set_sys(sysreg::TPIDRRO_EL0, TSD_IPA).unwrap();
        vcpu.set_sys(sysreg::TPIDR_EL0, state.tpidr_el0).unwrap(); // captured, NOT forced to 0
        Self::set_pac_keys(&vcpu);
        vcpu.set_sys(sysreg::SCTLR_EL1, SCTLR_MMU_ON).unwrap();
        vcpu.set_sys(sysreg::VBAR_EL1, TRAMPOLINE_IPA).unwrap();
        vcpu.set_trap_debug_exceptions(true).unwrap(); // must not be omitted or step() stops trapping
        for i in 0..31 { vcpu.set_reg(reg::x(i as u32), state.regs.x[i]).unwrap(); }
        vcpu.set_reg(reg::PC, state.regs.pc).unwrap();
        vcpu.set_reg(reg::CPSR, state.regs.cpsr).unwrap();
        vcpu.set_sys(sysreg::SP_EL0, state.regs.sp_el0).unwrap();
        vcpu.set_reg(reg::FPCR, state.fpcr).unwrap();
        vcpu.set_reg(reg::FPSR, state.fpsr).unwrap();
        for (i, &v) in state.fp.iter().enumerate() { vcpu.set_simd(simd::q(i as u32), v).unwrap(); }
        let l2_host = backings.iter().find(|b| b.ipa == PT_L2_IPA).map(|b| b.host)
            .unwrap_or(std::ptr::null_mut());
        for b in backings.iter().filter(|b| b.ipa >= PT_L3_BASE && b.ipa < PT_L3_CEIL) {
            assert_eq!(b.len, GRANULE,
                "from_checkpoint: backing at L3-window ipa {:#x} has len {} != GRANULE", b.ipa, b.len);
        }
        let next_l3 = backings.iter()
            .filter(|b| b.ipa >= PT_L3_BASE && b.ipa < PT_L3_CEIL)
            .map(|b| b.ipa + GRANULE as u64).max().unwrap_or(PT_L3_BASE);
        let mut b = Box_ {
            vm, vcpu, backings,
            reservations: state.reservations.clone(),
            mmap_next: state.mmap_next,
            bootstrap_port: state.bootstrap_port,
            l2_host, next_l3,
            last_far: state.last_far,
            synthetic_tsc: state.synthetic_tsc,
            cache_refault_ipa: state.cache_refault_ipa,
            cache_refault_count: state.cache_refault_count,
            cache: None,
        };
        if state.cache_installed { b.install_cache_pager(); }
        b
    }

    /// Bring-up/test diagnostic: dump V0..V31, FPCR, FPSR — the checkpoint round-trip's FP half of
    /// `dbg_regs()`. Kept separate from `dbg_regs()` deliberately: `dbg_regs()` is the `debug` CLI's
    /// `regs` command output (a byte-compare contract for existing golden-transcript tests); this is
    /// test-only and never wired into the CLI.
    pub fn dbg_fp_regs(&self) -> String {
        let mut s = String::new();
        for i in 0..32u32 {
            s += &format!("q{i:<2}={:#034x}  ", self.vcpu.get_simd(simd::q(i)).unwrap());
            if i % 2 == 1 { s.push('\n'); }
        }
        s += &format!("fpcr={:#x} fpsr={:#x}", self.vcpu.get_reg(reg::FPCR).unwrap(), self.vcpu.get_reg(reg::FPSR).unwrap());
        s
    }

    /// Test-only diagnostic: the internal bookkeeping fields `checkpoint()`/`from_checkpoint()` must
    /// round-trip that have no other observable accessor. Never used by production code.
    #[doc(hidden)]
    pub fn dbg_internal_state(&self) -> String {
        format!("reservations={:?} mmap_next={:#x} bootstrap_port={:?} cache_installed={} last_far={:#x} synthetic_tsc={:#x} cache_refault_ipa={:#x} cache_refault_count={}",
            self.reservations, self.mmap_next, self.bootstrap_port, self.cache.is_some(),
            self.last_far, self.synthetic_tsc, self.cache_refault_ipa, self.cache_refault_count)
    }
```

- [ ] **Step 4: Run the test.** `cargo test -p retrace-box --test checkpoint -- --test-threads=1` — Expected: PASS (1 test).

- [ ] **Step 5: Full gate.** `just gate` — Expected: **99 / 0 / 0**, clippy clean.

- [ ] **Step 6: Commit.**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/checkpoint.rs
git commit -m "M4 t2: BoxState — mid-run Box_ capture/restore, round-trip proven lossless

Co-Authored-By: <executing model> <noreply@anthropic.com>"
```

---

### Task 3: `spinloop` guest — a deliberately huge single window

**Files:**
- Create: `crates/retrace-guest/asm/spinloop.s`
- Modify: `crates/retrace-guest/build.rs` (add the compile step), `crates/retrace-guest/src/lib.rs` (the `SPINLOOP` path constant + a parse-smoke test).

**Interfaces:**
- Consumes: nothing new.
- Produces: `retrace_guest::SPINLOOP: &str` (path to the built binary), a guest recording two syscalls (`write`, `exit`) with two very differently-sized windows for Tasks 4-6 to seek into.

- [ ] **Step 1: Write `crates/retrace-guest/asm/spinloop.s`**, copying `hello.s`'s section/entry/exit-sequence shape exactly:

```asm
.section __TEXT,__text
.global _start
.p2align 2
_start:
    mov  x0, #300                // window 1 (landmark 1, up to `write`): a modest spin, ~606 insns
loop1:
    subs x0, x0, #1
    b.ne loop1
    mov  x0, #1                  // fd = stdout
    adrp x1, msg@PAGE
    add  x1, x1, msg@PAGEOFF
    mov  x2, #6                  // len
    mov  x16, #4                 // SYS_write
    svc  #0x80
    mov  x0, #2000                // window 2 (landmark 2, up to `exit`): the huge spin, ~4003 insns
loop2:
    subs x0, x0, #1
    b.ne loop2
    mov  x0, #0                  // status
    mov  x16, #1                 // SYS_exit
    svc  #0x80
.section __DATA,__data
msg:
    .ascii "spin!\n"
```

- [ ] **Step 2: Wire the build.** In `crates/retrace-guest/build.rs`, after the `steppy` build block (after the `assert!(status.success(), "steppy guest build failed");` line), add:

```rust
    // spinloop: write(1,"spin!\n",6) after a ~606-insn spin, then exit(0) after a ~4003-insn spin.
    // Landmark 1's window is modest (clears a cost-gate cache threshold); landmark 2's window is
    // deliberately huge — the M4 checkpoint acceleration's synthetic target.
    let src = format!("{}/asm/spinloop.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/spinloop");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang spinloop");
    assert!(status.success(), "spinloop guest build failed");
```

In `crates/retrace-guest/src/lib.rs`, add near the other path constants (after `pub const STEPPY: ...`):

```rust
pub const SPINLOOP: &str = concat!(env!("OUT_DIR"), "/spinloop");
```

- [ ] **Step 3: Write the failing smoke test.** In `crates/retrace-guest/src/lib.rs`'s `#[cfg(test)] mod tests`, add (mirroring `fileio_guest_parses`):

```rust
    #[test]
    fn spinloop_guest_parses() {
        let l = parse_macho(&std::fs::read(SPINLOOP).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
        assert!(l.segments.iter().any(|s| s.data.windows(6).any(|w| w == b"spin!\n")));
    }
```

- [ ] **Step 4: Run the test.** `cargo test -p retrace-guest spinloop_guest_parses -- --test-threads=1` — Expected: PASS (build.rs compiles the new guest as a side effect of the build).

- [ ] **Step 5: Full gate.** `just gate` — Expected: **100 / 0 / 0**, clippy clean.

- [ ] **Step 6: Commit.**

```bash
git add crates/retrace-guest/asm/spinloop.s crates/retrace-guest/build.rs crates/retrace-guest/src/lib.rs
git commit -m "M4 t3: spinloop guest — a huge single window for the checkpoint acceleration tests

Co-Authored-By: <executing model> <noreply@anthropic.com>"
```

---

### Task 4: `SessionCheckpoint` / `CheckpointCache` / `checkpointed_seek`

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` — `SessionCheckpoint`, `CheckpointCache`, `ReplaySession::checkpoint`/`from_checkpoint`/`dbg_fp_regs`, `checkpointed_seek`.
- Test: `crates/retrace/tests/checkpoint_seek.rs`

**Interfaces:**
- Consumes: `Box_::checkpoint`/`from_checkpoint`/`dbg_fp_regs` (Task 2), `retrace_guest::SPINLOOP` (Task 3).
- Produces (Tasks 5-6 rely on these exact names):

```rust
pub struct SessionCheckpoint { /* box_state, idx, guest_task_port — private fields */ }
pub struct CheckpointCache { /* private */ }
impl CheckpointCache {
    pub fn new(byte_budget: usize, cost_gate_steps: u64) -> Self;
    pub fn total_single_steps(&self) -> u64;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn used_bytes(&self) -> usize;
}
impl ReplaySession {
    pub fn checkpoint(&self) -> SessionCheckpoint;
    pub fn from_checkpoint(trace_path: &Path, checkpoint: SessionCheckpoint) -> Result<Self, String>;
    pub fn dbg_fp_regs(&self) -> String; // delegates to Box_::dbg_fp_regs
}
pub fn checkpointed_seek(trace_path: &Path, cache: &mut CheckpointCache, n: usize, k: u64)
    -> Result<ReplaySession, String>;
```

- [ ] **Step 1: Write the failing tests.** Create `crates/retrace/tests/checkpoint_seek.rs`:

```rust
// The M4 correctness invariant, directly generalizing M3's oracle: a checkpoint-restored-and-
// continued session must produce byte-identical results to a cold seek() to the same (N,K). Plus
// CheckpointCache's byte-budget/LRU eviction, proven via the observable behavior it produces
// (never via wall-clock — this project bans timing-based assertions by policy).
mod util;
use std::path::Path;

#[test]
fn checkpointed_seek_same_and_earlier_window_hits_match_cold() {
    let (rec, trace) = util::record(retrace_guest::SPINLOOP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let trace = Path::new(&trace);
    let mut cache = retrace_core::CheckpointCache::new(256 * 1024 * 1024, 64);

    // SAME-WINDOW hit: checkpoint deep in window 2 (landmark 2, ~4003 insns), then seek a few
    // steps further — must resume from the cache, not restart from landmark 0.
    let _ = retrace_core::checkpointed_seek(trace, &mut cache, 2, 3990).unwrap();
    assert_eq!(cache.len(), 1, "a >=64-step seek must clear the cost gate and get cached");
    let before = cache.total_single_steps();
    let (regs_a, fp_a, mem_a) = {
        let mut s = retrace_core::checkpointed_seek(trace, &mut cache, 2, 3995).unwrap();
        (s.dbg_regs(), s.dbg_fp_regs(), { let (_, mem) = s.snapshot(); mem })
    };
    let same_window_cost = cache.total_single_steps() - before;
    assert!(same_window_cost <= 10, "same-window hit should need ~5 steps, paid {same_window_cost}");
    let mut cold_a = retrace_core::seek(trace, 2, 3995).unwrap();
    assert_eq!(cold_a.dbg_regs(), regs_a, "registers diverged: checkpointed vs cold");
    assert_eq!(cold_a.dbg_fp_regs(), fp_a, "FP/SIMD diverged: checkpointed vs cold");
    assert!(cold_a.diff_memory(&mem_a).is_none(), "memory diverged: checkpointed vs cold");

    // EARLIER-WINDOW hit: checkpoint deep in window 1 (landmark 1, ~606 insns; clears the cost
    // gate), then seek into window 2 — must resume via advance_to_landmark(2) + step_insns, not miss.
    let mut cache2 = retrace_core::CheckpointCache::new(256 * 1024 * 1024, 64);
    let _ = retrace_core::checkpointed_seek(trace, &mut cache2, 1, 590).unwrap();
    assert_eq!(cache2.len(), 1);
    let (regs_b, fp_b, mem_b) = {
        let mut s = retrace_core::checkpointed_seek(trace, &mut cache2, 2, 50).unwrap();
        (s.dbg_regs(), s.dbg_fp_regs(), { let (_, mem) = s.snapshot(); mem })
    };
    let mut cold_b = retrace_core::seek(trace, 2, 50).unwrap();
    assert_eq!(cold_b.dbg_regs(), regs_b, "registers diverged (earlier-window hit): checkpointed vs cold");
    assert_eq!(cold_b.dbg_fp_regs(), fp_b, "FP/SIMD diverged (earlier-window hit): checkpointed vs cold");
    assert!(cold_b.diff_memory(&mem_b).is_none(), "memory diverged (earlier-window hit): checkpointed vs cold");
}

#[test]
fn checkpoint_cache_respects_byte_budget_and_evicts_lru() {
    let (rec, trace) = util::record(retrace_guest::SPINLOOP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let trace = Path::new(&trace);

    // Measure one checkpoint's real footprint first (cost_gate_steps=1: always cached).
    let mut probe = retrace_core::CheckpointCache::new(usize::MAX, 1);
    let _ = retrace_core::checkpointed_seek(trace, &mut probe, 1, 50).unwrap();
    let one_checkpoint_bytes = probe.used_bytes();
    assert!(one_checkpoint_bytes > 0, "a cached checkpoint must have nonzero measured size");

    // A budget that comfortably fits ONE checkpoint but not three: repeated inserts must evict,
    // never exceed budget, and keep only the most recently used entries.
    let budget = one_checkpoint_bytes + one_checkpoint_bytes / 2;
    let mut cache = retrace_core::CheckpointCache::new(budget, 1);
    for k in [50u64, 150, 250, 350, 450] {
        let _ = retrace_core::checkpointed_seek(trace, &mut cache, 1, k).unwrap();
        assert!(cache.used_bytes() <= budget,
            "cache exceeded its byte budget after seeking to (1,{k}): {} > {budget}", cache.used_bytes());
    }
    assert!(cache.len() < 5,
        "5 inserts into a ~1.5-checkpoint budget must have evicted at least one entry, got {} entries", cache.len());

    // The MOST RECENT position (1, 450) must still be resident (LRU keeps the freshest).
    let before = cache.total_single_steps();
    let _ = retrace_core::checkpointed_seek(trace, &mut cache, 1, 455).unwrap();
    let paid = cache.total_single_steps() - before;
    assert!(paid <= 10, "the most recent checkpoint should still be resident: expected ~5 steps, paid {paid}");
}
```

- [ ] **Step 2: Verify failure.** `cargo test -p retrace --test checkpoint_seek -- --test-threads=1` — Expected: FAIL to compile (`no CheckpointCache in retrace_core`, `no fn checkpointed_seek`).

- [ ] **Step 3: Implement.** In `crates/retrace-core/src/lib.rs`, add after `ReplaySession`'s `impl` block (after `window_len_here`, before `seek`):

```rust
    /// Re-open a session's trace-level constants (`events`, `truncated`) and restore a `Box_` +
    /// position from a previously captured checkpoint, skipping the landmark-0 replay a cold `open`
    /// would pay. `stdout` starts empty — no checkpoint consumer reads it.
    pub fn from_checkpoint(trace_path: &Path, checkpoint: SessionCheckpoint) -> Result<Self, String> {
        let (events, truncated) = retrace_trace::Reader::open_checked(trace_path)
            .map_err(|e| format!("cannot open trace: {e}"))?;
        let b = Box_::from_checkpoint(&checkpoint.box_state);
        Ok(ReplaySession { b, events, idx: checkpoint.idx, stdout: Vec::new(),
                            guest_task_port: checkpoint.guest_task_port, truncated })
    }

    /// Capture this session's current position as a `SessionCheckpoint`.
    pub fn checkpoint(&self) -> SessionCheckpoint {
        SessionCheckpoint { box_state: self.b.checkpoint(), idx: self.idx,
                             guest_task_port: self.guest_task_port }
    }

    /// FP/SIMD register dump — the checkpoint determinism tests' FP half of `dbg_regs()`.
    pub fn dbg_fp_regs(&self) -> String { self.b.dbg_fp_regs() }
}

/// An in-memory-only capture of a `ReplaySession`'s complete position-varying state: `Box_`'s full
/// internal state (`BoxState`) plus the two `ReplaySession` fields that vary by position (`idx`,
/// `guest_task_port`). `stdout` is deliberately not captured — nothing that reads a
/// checkpoint-restored session inspects it.
#[derive(Clone)]
pub struct SessionCheckpoint {
    box_state: retrace_box::BoxState,
    idx: usize,
    guest_task_port: Option<u64>,
}

impl SessionCheckpoint {
    fn approx_bytes(&self) -> usize { self.box_state.mem.iter().map(|r| r.bytes.len()).sum() }
}

/// A session-scoped, single-trace cache of `SessionCheckpoint`s keyed by trace-execution-order
/// position `(landmark N, step K)`. Purely a performance layer for `checkpointed_seek` — never
/// consulted for correctness. Only positions expensive to REACH (single-step count >=
/// `cost_gate_steps`) get stored, evicting the least-recently-used entry first once `byte_budget`
/// would be exceeded. No invalidation: a checkpoint's validity depends only on (trace file,
/// position), both fixed for this cache's lifetime — entries are only ever evicted for space.
pub struct CheckpointCache {
    entries: std::collections::BTreeMap<(usize, u64), SessionCheckpoint>,
    recency: Vec<(usize, u64)>, // oldest-used first; touched entries move to the back
    byte_budget: usize,
    used_bytes: usize,
    cost_gate_steps: u64,
    total_single_steps: u64,
}

impl CheckpointCache {
    pub fn new(byte_budget: usize, cost_gate_steps: u64) -> Self {
        CheckpointCache { entries: std::collections::BTreeMap::new(), recency: Vec::new(),
                           byte_budget, used_bytes: 0, cost_gate_steps, total_single_steps: 0 }
    }

    /// Total single-steps ever paid across every `checkpointed_seek` call against this cache — the
    /// cost-gating input, and the deterministic proxy the test suite uses to prove acceleration.
    pub fn total_single_steps(&self) -> u64 { self.total_single_steps }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn used_bytes(&self) -> usize { self.used_bytes }

    fn touch(&mut self, key: (usize, u64)) {
        self.recency.retain(|&k| k != key);
        self.recency.push(key);
    }

    /// The best cached position at or before `(n, k)` in execution order, if any — cloned out and
    /// marked most-recently-used.
    fn best_at_or_before(&mut self, n: usize, k: u64) -> Option<((usize, u64), SessionCheckpoint)> {
        let key = *self.entries.range(..=(n, k)).next_back()?.0;
        self.touch(key);
        Some((key, self.entries[&key].clone()))
    }

    /// Record `steps_paid` toward the running total, and — only if it clears the cost gate — store
    /// `checkpoint` at `(n, k)`, evicting least-recently-used entries first while over budget.
    fn record_and_maybe_insert(&mut self, n: usize, k: u64, steps_paid: u64, checkpoint: SessionCheckpoint) {
        self.total_single_steps += steps_paid;
        if steps_paid < self.cost_gate_steps { return; }
        let bytes = checkpoint.approx_bytes();
        while self.used_bytes + bytes > self.byte_budget && !self.recency.is_empty() {
            let oldest = self.recency.remove(0);
            if let Some(evicted) = self.entries.remove(&oldest) { self.used_bytes -= evicted.approx_bytes(); }
        }
        if bytes > self.byte_budget { return; } // a single entry over budget is never cached
        self.entries.insert((n, k), checkpoint);
        self.used_bytes += bytes;
        self.touch((n, k));
    }
}

/// Same contract as `seek` — the cache is purely an accelerator; a miss falls back to the cold path,
/// so no new failure mode reaches callers. On a same-window hit, resumes with only the remaining
/// `step_insns`; on an earlier-window hit, resumes with `advance_to_landmark` then `step_insns`; on
/// a miss, seeks cold. After landing, the single-step count actually paid this call (landmark
/// replay is native-speed and deliberately excluded from the cost gate) is recorded, and the
/// position stored as a fresh checkpoint if that count clears `cache`'s cost gate.
pub fn checkpointed_seek(trace_path: &Path, cache: &mut CheckpointCache, n: usize, k: u64)
    -> Result<ReplaySession, String> {
    let hit = cache.best_at_or_before(n, k);
    let (s, steps_paid) = match hit {
        Some(((n0, k0), checkpoint)) if n0 == n => {
            let mut s = ReplaySession::from_checkpoint(trace_path, checkpoint)?;
            s.step_insns(k - k0)?;
            (s, k - k0)
        }
        Some((_, checkpoint)) => {
            let mut s = ReplaySession::from_checkpoint(trace_path, checkpoint)?;
            s.advance_to_landmark(n).map_err(|d| format!("seek to landmark {n}: {}", d.detail))?;
            s.step_insns(k)?;
            (s, k)
        }
        None => {
            let mut s = ReplaySession::open(trace_path)?;
            s.advance_to_landmark(n).map_err(|d| format!("seek to landmark {n}: {}", d.detail))?;
            s.step_insns(k)?;
            (s, k)
        }
    };
    let checkpoint = s.checkpoint();
    cache.record_and_maybe_insert(n, k, steps_paid, checkpoint);
    Ok(s)
}
```

(`ReplaySession`'s existing `impl` block: insert `from_checkpoint`/`checkpoint`/`dbg_fp_regs` as three new methods anywhere inside it, e.g. right after `window_len_here`.)

- [ ] **Step 4: Run the tests.** `cargo test -p retrace --test checkpoint_seek -- --test-threads=1` — Expected: PASS (2 tests). If a K value assertion fails because Task 3's actual window lengths differ from 606/4003, adjust the hardcoded K values in this file (and re-run) — do not loosen the assertions themselves.

- [ ] **Step 5: Full gate.** `just gate` — Expected: **102 / 0 / 0**, clippy clean.

- [ ] **Step 6: Commit.**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace/tests/checkpoint_seek.rs
git commit -m "M4 t4: CheckpointCache + checkpointed_seek — same/earlier-window resume, cost-gated LRU

Co-Authored-By: <executing model> <noreply@anthropic.com>"
```

---

### Task 5: NEON-crossing determinism test on `hello_dyn`

**Files:**
- Test: `crates/retrace/tests/checkpoint_seek.rs` (extend)

**Interfaces:**
- Consumes: `checkpointed_seek`, `CheckpointCache` (Task 4); `retrace_guest::HELLO_DYN`, `util::record_dynamic` (existing).
- Produces: nothing new — this task is proof, not API surface.

- [ ] **Step 1: Write the failing test.** Append to `crates/retrace/tests/checkpoint_seek.rs`:

```rust
/// Probe increasing landmarks for one whose window is at least `min` instructions long (one
/// session per probe, sequential — never two alive). If NO candidate clears `min`, widen this list
/// rather than lowering `min` below the cache's cost gate.
fn first_window_with_len(trace: &Path, min: u64) -> (usize, u64) {
    for n in [3usize, 5, 8, 12, 20, 30, 50, 80, 100, 130, 160, 200, 250, 300] {
        let mut s = retrace_core::seek(trace, n, 0).unwrap();
        let l = s.window_len_here().unwrap();
        drop(s);
        if l >= min { return (n, l); }
    }
    panic!("no window of >= {min} insns among the probes — widen the candidate landmark list");
}

#[test]
fn checkpointed_seek_matches_cold_across_a_neon_window() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let trace = Path::new(&trace);
    // dyld's early init uses NEON (memcpy, hashing) well before any application code runs; a
    // checkpoint taken partway through such a window and resumed must carry the LIVE V-register
    // state, not the zeroed defaults Box_::restore silently assumes at landmark 0.
    let (n, len) = first_window_with_len(trace, 100);
    let k0 = len / 2;
    let mut cache = retrace_core::CheckpointCache::new(256 * 1024 * 1024, 64);
    let _ = retrace_core::checkpointed_seek(trace, &mut cache, n, k0).unwrap();
    assert!(cache.len() >= 1, "a >=50-step seek into a >=100-insn window must clear the cost gate");
    let k1 = k0 + 10;
    let (regs, fp, mem) = {
        let mut s = retrace_core::checkpointed_seek(trace, &mut cache, n, k1).unwrap();
        (s.dbg_regs(), s.dbg_fp_regs(), { let (_, mem) = s.snapshot(); mem })
    };
    let cold = retrace_core::seek(trace, n, k1).unwrap();
    assert_eq!(cold.dbg_regs(), regs, "registers diverged across a NEON-crossing window");
    assert_eq!(cold.dbg_fp_regs(), fp, "FP/SIMD state diverged across a NEON-crossing window");
    assert!(cold.diff_memory(&mem).is_none(), "memory diverged across a NEON-crossing window");
}
```

- [ ] **Step 2: Verify failure, then run.** `cargo test -p retrace --test checkpoint_seek -- --test-threads=1` — Expected: FAIL to compile first (new test not yet present is not applicable here since Task 4's tests already compile; instead this step is just: run immediately). Expected: PASS (3 tests total in this file). If `first_window_with_len` panics ("no window..."), widen the candidate list per its panic message and re-run — do not lower `min` below 64 (the cost gate).

- [ ] **Step 3: Full gate.** `just gate` — Expected: **103 / 0 / 0**, clippy clean.

- [ ] **Step 4: Commit.**

```bash
git add crates/retrace/tests/checkpoint_seek.rs
git commit -m "M4 t5: prove checkpointed_seek carries live FP/SIMD state across a NEON window

Co-Authored-By: <executing model> <noreply@anthropic.com>"
```

---

### Task 6: Wire `debug.rs`'s `Exec` to `checkpointed_seek` + the large-window speedup proof

**Files:**
- Modify: `crates/retrace/src/debug.rs` — imports, `Exec` struct, `Exec::new`/`reseek`/`probe_window_len`, `resolve_hit_k`, `cmd_continue`, `cmd_reverse_continue`.
- Test: `crates/retrace/tests/checkpoint_seek.rs` (extend)

**Interfaces:**
- Consumes: `checkpointed_seek`, `CheckpointCache` (Task 4).
- Produces: no new public API — `debug::run_script`'s signature and every command's transcript output are UNCHANGED (checkpointing must be invisible to CLI output; this is the whole point).

- [ ] **Step 1: Write the failing test.** Append to `crates/retrace/tests/checkpoint_seek.rs`:

```rust
#[test]
fn large_window_second_nearby_seek_is_far_cheaper_than_the_first() {
    let (rec, trace) = util::record(retrace_guest::SPINLOOP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let trace = Path::new(&trace);
    let mut cache = retrace_core::CheckpointCache::new(256 * 1024 * 1024, 64);
    // Landmark 2 = the ~4003-instruction loop2 window (the M4 acceleration target).
    let before1 = cache.total_single_steps();
    let _ = retrace_core::checkpointed_seek(trace, &mut cache, 2, 3990).unwrap();
    let first_cost = cache.total_single_steps() - before1;
    assert!(first_cost >= 3000, "the first seek into a ~4003-insn window should pay most of it, paid {first_cost}");

    let before2 = cache.total_single_steps();
    let _ = retrace_core::checkpointed_seek(trace, &mut cache, 2, 3995).unwrap();
    let second_cost = cache.total_single_steps() - before2;
    assert!(second_cost <= 20, "a nearby second seek should reuse the checkpoint, paid {second_cost}");
    assert!(second_cost * 50 < first_cost,
        "second seek ({second_cost} steps) should be far cheaper than the first ({first_cost})");
}
```

(This duplicates Task 4's same-window shape at a different call site — deliberately: it exercises the standalone function directly, independent of `Exec`, so a future regression in `Exec`'s wiring below can't hide a regression here.)

- [ ] **Step 2: Run it.** `cargo test -p retrace --test checkpoint_seek large_window -- --test-threads=1` — Expected: PASS (this test alone doesn't depend on `Exec`, so it should already pass using Task 4's `checkpointed_seek` — confirms the acceleration claim before wiring the CLI).

- [ ] **Step 3: Wire `Exec`.** In `crates/retrace/src/debug.rs`, change line 8's import:

```rust
use retrace_core::{checkpointed_seek, Advance, CheckpointCache, ReplaySession};
```

Add near the top (after `MAX_EXAMINE_LEN`):

```rust
/// Checkpoint cache sizing (M4): a byte budget generous enough to hold several tens of mid-run
/// checkpoints without unbounded growth on a long debug session, and a single-step cost gate that
/// only bothers caching positions genuinely expensive to reach (landmark replay is native-speed and
/// excluded from this count).
const CHECKPOINT_BYTE_BUDGET: usize = 256 * 1024 * 1024;
const CHECKPOINT_COST_GATE_STEPS: u64 = 64;
```

Change the `Exec` struct (add a field) and its four seek-touching methods:

```rust
struct Exec<'a> {
    trace: &'a Path,
    session: Option<ReplaySession>,
    n: usize,
    k: u64,
    breakpoints: Vec<u64>,
    cache: CheckpointCache,
}

impl<'a> Exec<'a> {
    fn new(trace: &'a Path) -> Result<Self, String> {
        let mut cache = CheckpointCache::new(CHECKPOINT_BYTE_BUDGET, CHECKPOINT_COST_GATE_STEPS);
        let session = checkpointed_seek(trace, &mut cache, 1, 0)?;
        Ok(Exec { trace, session: Some(session), n: 1, k: 0, breakpoints: Vec::new(), cache })
    }
```

```rust
    fn reseek(&mut self, n: usize, k: u64) -> Result<(), String> {
        self.session = None; // free the old VM BEFORE opening a new one
        self.session = Some(checkpointed_seek(self.trace, &mut self.cache, n, k)?);
        self.n = n;
        self.k = k;
        Ok(())
    }
```

```rust
    fn probe_window_len(&mut self, n: usize) -> Result<u64, String> {
        self.session = None; // free the live VM before the probe
        let mut probe = checkpointed_seek(self.trace, &mut self.cache, n, 0)?;
        probe.window_len_here()
    }
```

Change `resolve_hit_k`'s signature and body:

```rust
fn resolve_hit_k(trace: &Path, cache: &mut CheckpointCache, n: usize, pc: u64, from_k: u64) -> Result<u64, String> {
    let mut s = checkpointed_seek(trace, cache, n, from_k)?;
    let mut k = from_k;
    loop {
        if s.pc() == pc { return Ok(k); }
        s.step_insns(1).map_err(|e| format!("resolve K in window {n}: {e}"))?;
        k += 1;
    }
}
```

In `cmd_continue`, the one call site (`let k = resolve_hit_k(self.trace, n, p_hit, kctx + 1)?;`) becomes:

```rust
                    let k = resolve_hit_k(self.trace, &mut self.cache, n, p_hit, kctx + 1)?;
```

In `cmd_reverse_continue`, the loop's seek call:

```rust
            let mut s = checkpointed_seek(self.trace, &mut self.cache, cur_n, cur_k)?;
```

and its `resolve_hit_k` call:

```rust
            let k = resolve_hit_k(self.trace, &mut self.cache, n, pc, from_k)?;
```

- [ ] **Step 4: Run the full gate — this is the regression proof.** `just gate` — Expected: **104 / 0 / 0**, clippy clean. Critically, `crates/retrace/tests/debug_cli.rs` (6 tests) and `crates/retrace/tests/reverse_debug_e2e.rs` (1 test) must pass **UNMODIFIED** — every one of those tests spawns a fresh CLI process per script (so each gets a cold `CheckpointCache` by construction), and their byte-identical-transcript assertions are exactly the oracle that proves checkpointing is invisible to CLI output. If any of them fail, the bug is in the wiring above, not in those tests — do not edit them to make it pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/retrace/src/debug.rs crates/retrace/tests/checkpoint_seek.rs
git commit -m "M4 t6: wire retrace debug's Exec to checkpointed_seek — existing e2e transcripts unchanged

Co-Authored-By: <executing model> <noreply@anthropic.com>"
```

---

### Task 7: README Status + final verification

**Files:**
- Modify: `README.md` — new `## Status: M4 — checkpointed reverse-execution seeks` section.

**Interfaces:**
- Consumes: everything above.
- Produces: the closing documentation; report back to the controller for the memory update (do NOT edit `~/.claude/`).

- [ ] **Step 1: Add the README section**, mirroring the M3 status section's shape (`README.md:780`, `## Status: M3 — Reverse Execution ✅ (the M3 headline gate is GREEN)`): what the acceleration is (checkpointed positions, cost-gated, byte-budget + LRU), why it was needed (single-step-within-a-window was the real bottleneck, not landmark replay), the FP/SIMD capture gap it had to close, the gate outcome, and a Deferred list (window-length memoization for `probe_window_len`/`window_len_here` — noted but out of scope this milestone; a user-facing config knob for the byte budget/cost gate; persisting checkpoints across sessions — deliberately never, per the design).

- [ ] **Step 2: Final full gate.** `just gate` — Expected: **104 / 0 / 0**, clippy clean.

- [ ] **Step 3: Commit.**

```bash
git add README.md
git commit -m "M4 t7: README Status — checkpointed reverse-execution seeks, gate 104/0/0

Co-Authored-By: <executing model> <noreply@anthropic.com>"
```

Report the final gate count and confirm no test file outside this plan's scope needed modification, back to the controller for the memory update.

---

## Notes for the executor

- **The one thing not to get wrong:** `Box_::from_checkpoint` must call `set_trap_debug_exceptions(true)` — it's easy to copy `restore()`'s sysreg block and miss this one non-sysreg call sitting right after it. Missing it doesn't fail loudly; it silently makes every subsequent `step()` on a checkpoint-restored session stop trapping, which surfaces much later as a confusing hang or an unrelated-looking `Stop::Other`.
- **`checkpoint()`/`from_checkpoint()` correctness lives or dies on the field list.** Task 2's round-trip test can NOT catch an omitted field — capture→restore→capture of a field nobody captures trivially matches (the trio only compares what it knows to compare). If you add any NEW field to `Box_` in a future change, it must be added to `BoxState` (and `dbg_internal_state`) in the same change; the only enforcement is review discipline, not a test. This is a standing invariant, not a one-time check.
- **Sequential VMs, always.** A test that holds two `ReplaySession`/`Box_` values alive gets `HV_BUSY`, surfacing as a confusing `hv_vcpu_run` panic. Every test above is written to drop one before opening the next (via scoped blocks) — preserve that shape if you restructure anything.
- **A known, accepted residual gap:** `probe_window_len`/`window_len_here` still single-step the FULL window from K=0 every single time they're called — `checkpointed_seek` only accelerates *position* seeks, not window-length discovery. This means `reverse-stepi` crossing a landmark boundary into a huge window still pays that window's full length on every crossing. This is a legitimate follow-on (window-length memoization), explicitly out of scope for this milestone — do not silently expand into it.
- **Transcript stability is still the product.** Task 6's wiring must not change one byte of `debug.rs`'s existing output in any script. If you find yourself wanting to print or branch on "was this a cache hit," that's a debugging aid to delete before committing, not a feature — the cache is invisible by design.
