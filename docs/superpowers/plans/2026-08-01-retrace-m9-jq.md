# retrace M9 — `brew jq` + the guest-side TLBI oracle — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give retrace a guest-side TLBI oracle so a `MAP_FIXED` `PROT_EXEC` mapping onto an already-translated block is legal, add argc/argv to the process-start stack, and drive `brew jq` as far up rung 2 of the breadth ladder as it will go.

**Architecture:** A hand-assembled EL1 stub (`tlbi vmalle1; dsb ish; isb; hvc #0`) runs on the guest vCPU itself — the same "run the real instruction on the guest, never emulate it" pattern the PAC signing oracle already uses. With TLB invalidation available, `place_fixed`'s exec-over-live-backing refusal becomes a real code path: promote the range with `set_region_exec`, then flush. Everything lives below the trace inside `Box_`, on a path shared by record and replay, so determinism is structural rather than argued.

**Tech Stack:** Rust 1.95.0 (pinned, `aarch64-apple-darwin`), Hypervisor.framework via `hv-sys`, hand-written arm64 assembly guests built by `retrace-guest/build.rs`, C spikes against `-framework Hypervisor`.

**Design spec:** `docs/superpowers/specs/2026-08-01-retrace-m9-jq-design.md`

## Global Constraints

- **Platform:** macOS 26.x on Apple Silicon. Non-root; SIP may stay enabled.
- **`--test-threads=1` is mandatory.** HVF allows one VM per process. `just gate` sets it; a bare `cargo test` flakes with `HV_BUSY`.
- **The exit gate is `just gate`** = `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings`.
- **Gate baseline is 173 passed / 0 failed / 0 ignored.** It must not regress. `hello_dyn_e2e` and `hello_rust_e2e` stay green and un-`#[ignore]`d — a new wall gets a NEW parked gate, never a regression of these.
- **Codesigning:** every binary touching `hv_*` needs `retrace.entitlements`. `.cargo/config.toml`'s runner signs what cargo invokes; a test that spawns `CARGO_BIN_EXE_retrace` itself must sign it by hand — always go through `util::bin()`.
- **W^X:** executing a writable guest page hangs the vCPU. Code pages are RO+exec (`ATTR_CODE`), data is RW+non-exec (`ATTR_DATA`).
- **SPTM:** a file-backed `hv_vm_map` hard-panics macOS 26. All guest memory is anonymous; file bytes are staged via `pread`.
- **Drop order:** `Box_`'s field order is load-bearing (`vcpu` before `vm`). Do not reorder struct fields.
- **Never reimplement Apple's PAC.** Sign/authenticate by running `pac*`/`aut*` on the guest vCPU.
- **`clippy.toml` bans** `Instant::now`, `SystemTime::now`, `std::thread`. These denials are load-bearing for determinism.
- **Symmetry rule 1:** a special case in record's `match stop` needs a mirror in replay's dispatch, recomputing identical addresses/bytes.
- **Symmetry rule 2:** deterministic emulation belongs **below** the trace, inside `Box_::run()` / `Box_` methods shared by record and replay — then it fires identically on both sides automatically. **Everything in this milestone follows rule 2.**
- **Trace format:** changing `Event`'s shape is a format break requiring a `TRACE_MAGIC` bump. **This milestone changes no trace record and must not bump it.**

---

## File Structure

**Create:**
- `spikes/tlbi.c` — the go/no-go probe: does `TLBI` at guest EL1 invalidate stage-1 entries retrace hand-edited?
- `crates/retrace-guest/asm/tlbiexec.s` — the capability fixture: touch a page as data, `MAP_FIXED`-exec-map a file of code over it, execute it.
- `crates/retrace-guest/c/argv_echo.c` — a dynamic guest that prints `argv[1]`, proving argv reaches a real program through dyld.
- `crates/retrace/tests/tlbiexec_e2e.rs` — the capability gate.
- `crates/retrace/tests/argv_e2e.rs` — the argv gate.
- `crates/retrace/tests/jq_e2e.rs` — the rung-2 gate.

**Modify:**
- `crates/retrace-box/src/lib.rs` — TLBI stub constants + IPA, `flush_guest_tlb`, `run_tlbi_stub`, the `place_fixed` exec relaxation, `build_start_stack`'s argv widening.
- `crates/retrace-core/src/lib.rs:64-67` — `record_dynamic`'s argv widening.
- `crates/retrace/src/main.rs:30-54` — `--` separator in `record-dyn`.
- `crates/retrace-guest/build.rs` — build `tlbiexec` + its code fixture, build `argv_echo`.
- `crates/retrace-guest/src/lib.rs:90-119` — path constants for the new fixtures.
- `crates/retrace/tests/util/mod.rs` — `record_dynamic_args`, and `assert_rung_records_and_replays` gains an argv parameter.
- `spikes/README.md` — the spike's build recipe and findings.
- `README.md` — a new Status section.

**Decomposition note:** the TLBI oracle (Tasks 1–3) is deliberately separable from rung 2 itself (Task 5). If jq's wall-chain proves deep, Tasks 1–4 still land a proven, permanently useful capability.

---

### Task 1: The spike — settle TLBI at guest EL1 before building on it

This is a **go/no-go probe**, not production code. It answers three questions on the real OS, and its answer decides whether Tasks 2–3 proceed as designed or fall back to spec risk R1.

**Files:**
- Create: `spikes/tlbi.c`
- Modify: `spikes/README.md`

**Interfaces:**
- Consumes: nothing.
- Produces: a recorded finding — **F1** (does `tlbi vmalle1` at guest EL1 execute without trapping to EL2?), **F2** (does a stale entry actually persist without it — is the guard's premise real?), **F3** (does execution succeed after the flush?). Tasks 2–3 depend only on F1 and F3 being yes.

- [ ] **Step 1: Write the spike**

Base it on `spikes/m2spike.c`, which already builds MMU-on guest identity page tables (16 KiB granule, L2 at `0x08000`, L3 at `0x0C000`, EL1 vector table). Copy that skeleton rather than rebuilding it.

```c
// tlbi.c — M9 go/no-go: can the GUEST invalidate its own stage-1 TLB entries for us?
// retrace hand-edits live stage-1 page tables (set_region_exec) but the VMM cannot issue a guest
// TLBI, so today a data->code flip is only sound on a block the guest never translated. jq's dyld
// breaks that: it MAP_FIXED-exec-maps __TEXT into a reservation it has already touched.
// Answers, empirically, on this OS/silicon:
//   F1: does `tlbi vmalle1` execute at guest EL1 without trapping to EL2?
//   F2: WITHOUT a TLBI, does a data->code leaf flip leave a stale entry (execute faults)?
//   F3: WITH the TLBI, does execution from the flipped page succeed?
// F2 is the control: if execution succeeds even without the flush, the guard was over-conservative
// and that is itself the finding.
// SAFETY: every phase ends at a terminal `hvc #0`; still run under the external perl
// process-group timeout (no `timeout` binary on this platform).
//   clang -O2 -o tlbi tlbi.c -framework Hypervisor
//   codesign -s - -f --entitlements ent.plist tlbi
//   perl -e '$p=fork;if(!$p){setpgrp;exec@ARGV or exit 127}$SIG{ALRM}=sub{kill"-KILL",$p;exit 124};alarm 15;wait;exit($?>>8)' ./tlbi
#include <Hypervisor/Hypervisor.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>

#define PG          0x4000ULL
#define VEC_IPA     0x04000ULL      // EL1 vector table (EL1-exec)
#define L2_IPA      0x08000ULL      // start-level table
#define L3_IPA      0x0C000ULL      // L3 covering the first 32 MiB
#define CODE_IPA    0x10000ULL      // EL0 guest code
#define STUB_IPA    0x14000ULL      // the EL1 TLBI stub (EL1-exec)
#define TEST_IPA    0x18000ULL      // the page flipped from data to code

// Stage-1 leaf attributes, mirrored from crates/retrace-box/src/lib.rs.
#define A_COMMON    (0x3ULL | (0ULL<<2) | (3ULL<<8) | (1ULL<<10))  // page desc, attr0, inner-share, AF
#define UXN         (1ULL<<54)
#define PXN         (1ULL<<53)
#define ATTR_DATA   (A_COMMON | 0x40  | UXN | PXN)   // RW both ELs, never executable
#define ATTR_CODE   (A_COMMON | 0xC0  | PXN)         // RO, EL0-exec (UXN clear)
#define ATTR_TRAMP  (A_COMMON | 0x80  | UXN)         // RO, EL1-exec (PXN clear)

static hv_vcpu_t vcpu; static hv_vcpu_exit_t *vexit;
static uint64_t *l3;

static uint64_t rg(hv_reg_t r){ uint64_t v=0; hv_vcpu_get_reg(vcpu,r,&v); return v; }

// Run once; return the EL2 exception class.
static uint32_t run_once(const char *tag){
    hv_vcpu_run(vcpu);
    uint64_t esr2 = vexit->exception.syndrome;
    uint32_t ec2 = (uint32_t)((esr2>>26)&0x3f);
    printf("[%s] reason=%u EC2=0x%02x pc=0x%llx far=0x%llx\n",
           tag, vexit->reason, ec2, rg(HV_REG_PC), vexit->exception.virtual_address);
    return ec2;
}

static void set_leaf(uint64_t ipa, uint64_t attr){ l3[ipa/PG] = (ipa & ~(PG-1)) | attr; }

int main(void){
    if (hv_vm_create(NULL)) { printf("no vm\n"); return 1; }

    // ---- guest memory ----
    void *mem = mmap(NULL, 0x20000, PROT_READ|PROT_WRITE, MAP_ANON|MAP_PRIVATE, -1, 0);
    memset(mem, 0, 0x20000);
    hv_vm_map(mem, 0, 0x20000, HV_MEMORY_READ|HV_MEMORY_WRITE|HV_MEMORY_EXEC);
    uint8_t *base = (uint8_t*)mem;

    // EL1 vectors: every slot is `hvc #0`, so any guest fault lands at EL2 identifiably.
    for (int i=0;i<16;i++) ((uint32_t*)(base+VEC_IPA))[i*0x80/4] = 0xD4000002;

    // L2: one entry pointing at L3, covering the first 32 MiB.
    uint64_t *l2 = (uint64_t*)(base+L2_IPA);
    l2[0] = L3_IPA | 0x3;                       // table descriptor
    l3 = (uint64_t*)(base+L3_IPA);
    for (uint64_t i=0;i<2048;i++) l3[i] = (i*PG) | ATTR_DATA;
    set_leaf(CODE_IPA, ATTR_CODE);
    set_leaf(VEC_IPA,  ATTR_TRAMP);
    set_leaf(STUB_IPA, ATTR_TRAMP);             // EL1-exec: TLBI is an EL1 instruction
    set_leaf(TEST_IPA, ATTR_DATA);              // starts as DATA — the whole point

    // EL0 guest code at CODE_IPA:
    //   +0x00 movz x1, #0x18000-ish  -> built with two movs
    //   read TEST_IPA (forces a stage-1 walk + TLB fill as DATA), then hvc
    //   +0x10 blr into TEST_IPA, then hvc
    uint32_t code[] = {
        0xD2803001,   // movz x1, #0x180
        0xD3607C21,   // lsl  x1, x1, #12      -> x1 = 0x180000? (adjust: see note)
        0xF9400022,   // ldr  x2, [x1]         -> the DATA read that fills the TLB
        0xD4000002,   // hvc #0                -> phase 1 done
        0xD63F0020,   // blr  x1               -> execute from the flipped page
        0xD4000002,   // hvc #0                -> phase 3 done (only if the blr worked)
    };
    // NOTE: build x1 = TEST_IPA exactly. 0x18000 = movz x1,#0x18000 is not encodable directly as a
    // 16-bit immediate at shift 0; use `movz x1, #1, lsl #16` then `movk x1, #0x8000`:
    code[0] = 0xD2A00021;   // movz x1, #1, lsl #16   -> 0x10000
    code[1] = 0xF2900001;   // movk x1, #0x8000       -> 0x18000
    memcpy(base+CODE_IPA, code, sizeof(code));

    // The payload the flipped page must run: `movz x0,#0x5A ; ret`.
    uint32_t payload[] = { 0xD2800B40, 0xD65F03C0 };
    memcpy(base+TEST_IPA, payload, sizeof(payload));

    // The EL1 TLBI stub (verified encodings — `clang -c` + `otool -t`):
    //   tlbi vmalle1 ; dsb ish ; isb ; hvc #0
    uint32_t stub[] = { 0xd508871f, 0xd5033b9f, 0xd5033fdf, 0xd4000002 };
    memcpy(base+STUB_IPA, stub, sizeof(stub));

    // ---- vCPU: MMU ON ----
    hv_vcpu_config_t cfg = hv_vcpu_config_create();
    hv_vcpu_create(&vcpu,&vexit,cfg);
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_VBAR_EL1, VEC_IPA);
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_MAIR_EL1, 0xFF);
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_TCR_EL1,  0x8000210080B511ULL); // as retrace: T0SZ=17, TG0=16K
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_TTBR0_EL1, L2_IPA);
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_SCTLR_EL1, 0x30d00800ULL | 1ULL); // M=1 => MMU ON

    // ---- Phase 1: EL0 reads TEST_IPA as DATA (fills the TLB with a UXN entry) ----
    hv_vcpu_set_reg(vcpu, HV_REG_PC, CODE_IPA);
    hv_vcpu_set_reg(vcpu, HV_REG_CPSR, 0);                    // EL0t
    uint32_t ec = run_once("phase1-read");
    printf("phase1: %s\n", ec==0x16 ? "read OK (entry now cached as DATA)" : "UNEXPECTED");

    // ---- Flip the leaf DATA -> CODE, WITHOUT any TLBI ----
    set_leaf(TEST_IPA, ATTR_CODE);

    // ---- F2 (control): execute WITHOUT the flush ----
    hv_vcpu_set_reg(vcpu, HV_REG_PC, CODE_IPA+0x10);
    hv_vcpu_set_reg(vcpu, HV_REG_CPSR, 0);
    ec = run_once("F2-control");
    int stale = (ec != 0x16);
    printf("F2: without TLBI -> %s\n", stale
        ? "FAULTED (stale entry is REAL; the guard's premise holds)"
        : "EXECUTED ANYWAY (no stale entry — the guard was over-conservative!)");

    // ---- F1 + F3: run the EL1 stub, then execute again ----
    hv_vcpu_set_reg(vcpu, HV_REG_PC, STUB_IPA);
    hv_vcpu_set_reg(vcpu, HV_REG_CPSR, 0x3C5);                // EL1h, DAIF masked
    ec = run_once("F1-tlbi");
    printf("F1: tlbi vmalle1 at EL1 -> %s\n",
           ec==0x16 ? "EXECUTED, reached its hvc" : "TRAPPED/FAULTED (TLBI unavailable!)");

    hv_vcpu_set_reg(vcpu, HV_REG_PC, CODE_IPA+0x10);
    hv_vcpu_set_reg(vcpu, HV_REG_CPSR, 0);
    ec = run_once("F3-exec");
    printf("F3: after TLBI, execute flipped page -> %s (x0=0x%llx, want 0x5a)\n",
           ec==0x16 ? "SUCCEEDED" : "STILL FAULTS", rg(HV_REG_X0));

    printf("\nVERDICT: %s\n",
        (ec==0x16 && rg(HV_REG_X0)==0x5a)
          ? "GO — guest-side TLBI works; M9 Tasks 2-3 proceed as designed."
          : "NO-GO — fall back to spec risk R1 (pre-promote reservations).");

    hv_vcpu_destroy(vcpu); hv_vm_destroy();
    return 0;
}
```

- [ ] **Step 2: Build, sign, and run it**

```bash
cd spikes
clang -O2 -o tlbi tlbi.c -framework Hypervisor
codesign -s - -f --entitlements ent.plist tlbi
perl -e '$p=fork;if(!$p){setpgrp;exec@ARGV or exit 127}$SIG{ALRM}=sub{kill"-KILL",$p;exit 124};alarm 15;wait;exit($?>>8)' ./tlbi
```

Expected: a `VERDICT:` line. If `x1` does not land on `TEST_IPA`, the phase-1 read faults — fix the two `movz`/`movk` words before interpreting anything else.

- [ ] **Step 3: Record the findings in `spikes/README.md`**

Append a section following the existing entries' shape: the build/run recipe above, and the measured F1/F2/F3 answers verbatim from the run — **not** the expected answers. If F2 reports "EXECUTED ANYWAY", say so plainly; that changes Task 3's justification (the guard would be over-conservative rather than protecting a real hazard) and must be written down, not smoothed over.

- [ ] **Step 4: Gate on the verdict**

- **GO** (F1 executed, F3 succeeded with `x0=0x5a`): proceed to Task 2 unchanged.
- **NO-GO**: stop. Do not write `flush_guest_tlb`. Report to the human partner and re-plan against spec risk R1 (pre-promote file-backed reservations to L3 at map time). Tasks 4 and 6 are unaffected and can still proceed.

- [ ] **Step 5: Commit**

```bash
git add spikes/tlbi.c spikes/README.md
git commit -m "M9 t1: spike — does guest-side TLBI invalidate hand-edited stage-1 entries?"
```

---

### Task 2: `flush_guest_tlb` — the EL1 TLBI stub

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (stub constants near `SIGN_STUB` ~line 236–268; `flush_guest_tlb` + `run_tlbi_stub` near `run_sign_stub` ~line 723–960)
- Test: `crates/retrace-box/tests/tlbi.rs` (create)

**Interfaces:**
- Consumes: Task 1's GO verdict; the existing `set_region_exec(&mut self, ipa: u64, len: u64)`, the lazy sign-scratch init pattern, and `ATTR_TRAMP`.
- Produces: `pub fn flush_guest_tlb(&mut self)` on `Box_` — invalidates the guest's stage-1 TLB by running an EL1 stub on the vCPU. Restores full architectural state; safe to call mid-run. Task 3 calls it.

- [ ] **Step 1: Write the failing test**

`crates/retrace-box/tests/tlbi.rs`:

```rust
// M9. The TLBI oracle: flush_guest_tlb must run the EL1 stub to completion on a live box and leave
// the caller's architectural state untouched, so it is safe to call in the middle of a guest run.
use retrace_box::Box_;

#[test]
fn flush_guest_tlb_preserves_architectural_state() {
    let bytes = std::fs::read(retrace_guest::HELLO).expect("read hello");
    let loaded = retrace_guest::parse_macho(&bytes);
    let mut b = Box_::new(&loaded);

    let before = b.regs_snapshot();
    b.flush_guest_tlb();
    let after = b.regs_snapshot();

    assert_eq!(before, after,
        "flush_guest_tlb must restore every register it saved — a mid-run caller must see no \
         disturbance (same contract as sign_slots)");
}

#[test]
fn flush_guest_tlb_is_repeatable() {
    let bytes = std::fs::read(retrace_guest::HELLO).expect("read hello");
    let loaded = retrace_guest::parse_macho(&bytes);
    let mut b = Box_::new(&loaded);
    // Two flushes in a row must both reach the stub's terminating hvc (the bounded runner panics
    // otherwise), proving the scratch page and stub survive reuse.
    b.flush_guest_tlb();
    b.flush_guest_tlb();
}
```

If `Box_` has no `regs_snapshot()` accessor, add one in this task returning the same register vector `snapshot()` already captures — a small read-only helper, not new state.

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p retrace-box --test tlbi -- --test-threads=1
```

Expected: FAIL — `no method named flush_guest_tlb found for struct Box_`.

- [ ] **Step 3: Add the stub constants**

In `crates/retrace-box/src/lib.rs`, beside `SIGN_STUB_IPA` (~line 236):

```rust
// The TLBI stub page (M9). W^X: RO + EL1-exec (ATTR_TRAMP) — `tlbi` is an EL1 instruction, so this
// CANNOT share the sign stub's ATTR_CODE page (ATTR_CODE sets PXN: EL0-exec, EL1 no-exec).
const TLBI_STUB_IPA: u64 = 0x0004_8000; // 288 KiB: a fresh IPA the guest never translates
// Safety net for the stub's own run loop: a correct stub reaches its terminating `hvc` in ONE
// hv_vcpu_run, so any higher count means a bug.
const TLBI_STUB_BOUND: u32 = 4;
// The TLBI stub, hand-assembled. Encodings verified with `clang -arch arm64 -c` + `otool -t`:
//   tlbi vmalle1 ; dsb ish ; isb ; hvc #0
// Runs at EL1 (ATTR_TRAMP is EL1-exec) and ends with `hvc #0`, which from EL1 traps DIRECTLY to
// EL2 — no trampoline indirection, unlike the sign stub's EL0 `svc`.
// VMALLE1 (not VMALLE1IS): single-vCPU, so there is no other PE whose TLB needs invalidating.
const TLBI_STUB: [u32; 4] = [0xd508_871f, 0xd503_3b9f, 0xd503_3fdf, 0xd400_0002];
// EL1h with DAIF masked — the exception level `tlbi` requires.
const TLBI_STUB_CPSR: u64 = 0x3C5;
```

- [ ] **Step 4: Implement `flush_guest_tlb` and its runner**

Add beside `sign_slots` / `run_sign_stub`. Mirror the existing save/restore discipline exactly — reuse the same state list `sign_slots` uses.

```rust
/// Invalidate the guest's stage-1 TLB by running `tlbi vmalle1` **on the guest vCPU itself** — the
/// VMM cannot issue a guest TLBI, and this project never emulates an instruction the guest can run
/// (the same rule that makes the PAC oracle run real `pac*`/`aut*`).
///
/// Required whenever a stage-1 attribute changes on a range the guest may already have translated.
/// `set_region_exec` alone is sound only on a pristine block; this is what makes it sound anywhere.
///
/// Does NOT disturb the caller's guest state: the stub runs on a dedicated scratch page and the
/// full architectural state is saved and restored around it, so a mid-run caller sees nothing.
///
/// Below the trace: called from paths shared by record and replay, so it fires identically on both
/// sides (symmetry rule 2) and never surfaces to the record/replay loop.
pub fn flush_guest_tlb(&mut self) {
    self.ensure_tlbi_stub();
    let saved = self.save_arch_state();
    self.vcpu.set_reg(reg::PC, TLBI_STUB_IPA).expect("set PC (tlbi stub)");
    self.vcpu.set_reg(reg::CPSR, TLBI_STUB_CPSR).expect("set CPSR (tlbi stub)");
    self.run_tlbi_stub();
    self.restore_arch_state(&saved);
}

/// Lazy-init the TLBI scratch on first use: one stub CODE page at a fixed reserved IPA, RO +
/// EL1-exec. W^X — it is written once, before it is ever promoted, and never written again.
fn ensure_tlbi_stub(&mut self) {
    if self.tlbi_stub_ready { return; }
    let (host, rlen) = alloc_pages(GRANULE);
    unsafe {
        std::ptr::copy_nonoverlapping(
            TLBI_STUB.as_ptr() as *const u8, host, std::mem::size_of_val(&TLBI_STUB));
    }
    self.vm.map(host, TLBI_STUB_IPA, rlen, MemFlags::RWX).expect("hv_vm_map (tlbi stub)");
    self.backings.push(Backing { host, ipa: TLBI_STUB_IPA, len: rlen });
    // No TLBI needed for this promotion itself: TLBI_STUB_IPA is a fresh IPA the guest has never
    // translated (same soundness argument as the sign stub and the cache pager).
    self.set_region_exec_attr(TLBI_STUB_IPA, GRANULE as u64, ATTR_TRAMP);
    self.tlbi_stub_ready = true;
}

/// Run the TLBI stub to its terminating `hvc`. From EL1 an `hvc` traps straight to EL2, so the
/// terminating exit is a plain `Ec::Hvc` — no ESR_EL1 indirection (contrast `run_sign_stub`, whose
/// EL0 `svc` arrives via the guest's EL1 trampoline).
fn run_tlbi_stub(&mut self) {
    for _ in 0..TLBI_STUB_BOUND {
        let e = self.vcpu.run().expect("hv_vcpu_run (tlbi stub)");
        if e.reason != EXIT_EXCEPTION { continue; }
        match ec_of(e.syndrome) {
            Ec::Hvc => return, // the stub's terminating hvc: the flush is done
            other => panic!(
                "tlbi stub faulted at EL1: EC={other:?} (syndrome={:#x} far={:#x}) — bad encoding, \
                 a non-EL1-exec stub page (ATTR_TRAMP required), or CPSR not EL1h",
                e.syndrome, e.virtual_address),
        }
    }
    panic!("tlbi stub did not reach its terminating hvc within {TLBI_STUB_BOUND} runs");
}
```

Add the `tlbi_stub_ready: bool` field to `Box_` **after** the existing fields — do not disturb the `vcpu`-before-`vm` ordering — and initialise it `false` at every construction site (`new`, `load_dynamic`, `restore`).

`set_region_exec_attr(ipa, len, attr)` is `set_region_exec`'s body with the attribute as a parameter; refactor `set_region_exec` to call it with `ATTR_CODE` so there is exactly one promotion implementation. `save_arch_state`/`restore_arch_state` are the existing save/restore used by `sign_slots` — extract them into named helpers if they are currently inline, so both oracles share one list.

- [ ] **Step 5: Run the test to verify it passes**

```bash
cargo test -p retrace-box --test tlbi -- --test-threads=1
```

Expected: PASS, 2 tests.

- [ ] **Step 6: Verify the encodings independently**

```bash
printf '.section __TEXT,__text\n_stub:\n tlbi vmalle1\n dsb ish\n isb\n hvc #0\n' > /tmp/t.s
clang -arch arm64 -c -o /tmp/t.o /tmp/t.s && otool -t /tmp/t.o
```

Expected: `d508871f d5033b9f d5033fdf d4000002` — byte-for-byte equal to `TLBI_STUB`. If they differ, the constant is wrong; trust the assembler.

- [ ] **Step 7: Run the full gate**

```bash
just gate
```

Expected: 175 passed / 0 failed / 0 ignored (173 + 2 new), clippy clean.

- [ ] **Step 8: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/tlbi.rs
git commit -m "M9 t2: flush_guest_tlb — run tlbi vmalle1 on the guest vCPU at EL1"
```

---

### Task 3: Relax `place_fixed` — the capability gate

**Files:**
- Create: `crates/retrace-guest/asm/tlbiexec.s`
- Create: `crates/retrace/tests/tlbiexec_e2e.rs`
- Modify: `crates/retrace-box/src/lib.rs` (`place_fixed`, ~line 1344–1370)
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs`

**Interfaces:**
- Consumes: `flush_guest_tlb` from Task 2; `set_region_exec`; M8-stack's containment classification in `place_fixed`.
- Produces: `place_fixed` accepting `exec == true` over a live backing. `retrace_guest::TLBIEXEC` and `retrace_guest::TLBIEXEC_FIXTURE` path constants.

- [ ] **Step 1: Write the guest fixture**

`crates/retrace-guest/asm/tlbiexec.s` — a variant of `execmap.s`, but the exec mapping is `MAP_FIXED` **over a page the guest has already touched**. That is jq's exact shape in miniature.

```asm
// M9. The TLBI capability fixture: a MAP_FIXED PROT_EXEC mapping landing inside a backing the guest
// has ALREADY TRANSLATED must become executable. Before the TLBI oracle, place_fixed refused this
// outright ("exec promotion of an already-translated block would need a guest TLBI the VMM cannot
// issue") and the RECORDER ABORTED — exit 101, no guest error.
//
// This is dyld's non-cache-dylib strategy in miniature: reserve a span, touch it, then drop an
// executable segment into it at a fixed address.
//
// Exits with the mapped code's return value (42), so a wrong answer cannot look like success.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // mmap(0, 0x8000, PROT_READ|PROT_WRITE(3), MAP_ANON|MAP_PRIVATE(0x1002), -1, 0) -> 2 pages
    mov  x0, #0
    mov  x1, #0x8000
    mov  x2, #3
    movz x3, #0x1002
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197                 // SYS_mmap
    svc  #0x80
    mov  x19, x0                   // reservation base

    // TOUCH IT. This is the whole point: the store forces a stage-1 walk, so the block is
    // translated and its entry cached as DATA (RW, UXN) before the exec map arrives.
    mov  w9, #0x5A
    strb w9, [x19]
    ldrb w10, [x19]                // read it back too, so the entry is definitely live

    // open(path, O_RDONLY=0, 0) -- the file of code
    adrp x0, path@PAGE
    add  x0, x0, path@PAGEOFF
    mov  x1, #0
    mov  x2, #0
    mov  x16, #5                   // SYS_open
    svc  #0x80
    mov  x20, x0                   // fd

    // mmap(base, 0x4000, PROT_READ|PROT_EXEC(5), MAP_FIXED|MAP_PRIVATE(0x12), fd, 0)
    // FIXED, exec, and wholly CONTAINED in the live backing above -> the case place_fixed refused.
    mov  x0, x19
    mov  x1, #0x4000
    mov  x2, #5                    // PROT_READ | PROT_EXEC
    movz x3, #0x12                 // MAP_FIXED | MAP_PRIVATE (no MAP_ANON => file-backed)
    mov  x4, x20                   // fd
    mov  x5, #0
    mov  x16, #197
    svc  #0x80
    mov  x21, x0                   // must equal x19

    // Execute from it. The payload is `movz x0,#42 ; ret`.
    blr  x21

    // exit(x0)  -- 42 only if the flipped page really became executable
    mov  x16, #1                   // SYS_exit
    svc  #0x80

// `path:` is appended by the build script (generated) so it matches the fixture location.
```

- [ ] **Step 2: Wire it into the build**

In `crates/retrace-guest/build.rs`, after the `execmap` block (~line 134), add — reusing execmap's exact recipe:

```rust
    // tlbiexec: the M9 capability fixture. mmaps an anon RW region, TOUCHES it (so the block is
    // translated), then MAP_FIXED-exec-maps a file of code over it and blr's in. Proves the guest-side
    // TLBI oracle: without it, place_fixed refused the exec-over-live-backing map and the recorder
    // aborted. Same code fixture as execmap: `movz x0, #42 ; ret`.
    let fixture = format!("{out}/tlbiexec_fixture.bin");
    std::fs::write(&fixture, [0x40u8, 0x05, 0x80, 0xD2, 0xC0, 0x03, 0x5F, 0xD6]).unwrap();
    let gen = format!("{out}/tlbiexec_gen.s");
    std::fs::write(&gen, format!(".section __DATA,__data\n.p2align 3\n.global path\npath: .asciz \"{fixture}\"\n")).unwrap();
    let src = format!("{}/asm/tlbiexec.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/tlbiexec");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src,&gen])
        .status().expect("clang tlbiexec");
    assert!(status.success(), "tlbiexec guest build failed");
```

In `crates/retrace-guest/src/lib.rs`, after line 119:

```rust
pub const TLBIEXEC: &str = concat!(env!("OUT_DIR"), "/tlbiexec");
pub const TLBIEXEC_FIXTURE: &str = concat!(env!("OUT_DIR"), "/tlbiexec_fixture.bin");
```

- [ ] **Step 3: Write the failing e2e test**

`crates/retrace/tests/tlbiexec_e2e.rs`:

```rust
// M9 capability gate. A MAP_FIXED PROT_EXEC mapping onto an ALREADY-TRANSLATED backing must work:
// promote the range, then invalidate the guest's stale TLB entry with the guest-side TLBI oracle.
//
// This is deliberately separate from the jq gate. jq's wall-chain past this point is of unknown
// depth, and the capability must stay proven whether or not rung 2 goes green.
mod util;

#[test]
fn fixed_exec_over_touched_backing_records_and_replays() {
    let (rec, trace) = util::record(retrace_guest::TLBIEXEC);
    assert_eq!(rec.code, 42,
        "guest must exit with the mapped code's return value (42). 101 means the RECORDER aborted \
         in place_fixed — the exec-over-live-backing refusal. stderr:\n{}", rec.stderr);

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 42, "replay must reproduce the exit code. stderr:\n{}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout, "replay stdout diverged from the recording");
}

#[test]
fn fixed_exec_over_touched_backing_is_trace_reproducible() {
    // Freestanding (-nostdlib -static): no clock, no entropy, no libmalloc — so the second oracle
    // applies. Two recordings must be byte-identical, proving the TLBI path introduces nothing
    // nondeterministic into the trace.
    util::assert_trace_reproducible(retrace_guest::TLBIEXEC);
}
```

- [ ] **Step 4: Run it to verify it fails**

```bash
cargo test -p retrace --test tlbiexec_e2e -- --test-threads=1
```

Expected: FAIL, with `rec.code == 101` and stderr containing `FIXED exec map at ... overlaps a live backing`. **This is the wall reproduced in miniature** — confirm that message appears before fixing anything. If the guest exits 42 already, the fixture is not reproducing the case; fix the fixture, not the box.

- [ ] **Step 5: Relax the guard**

In `crates/retrace-box/src/lib.rs`, `place_fixed` (~line 1344). Delete the `assert!(!exec || ...)` and handle `exec` on the containment path instead. The containment branch currently copies `host` over the range inside the existing backing and returns `Some(addr)`; add the promotion + flush before that return:

```rust
        // (the `assert!(!exec || !self.overlaps_backing(..))` that was here is GONE — M9)
        let covers_all = /* unchanged */;
        if covers_all {
            self.unmap_overlapping(addr, rlen as u64);                // case 1
            return None;
        }
        if let Some((bhost, bipa)) = self.backings.iter()
            .find(|b| addr >= b.ipa && end <= b.ipa + b.len as u64)
            .map(|b| (b.host, b.ipa))
        {
            // ... existing containment copy of `host` into the backing, then munmap(host) ...

            // M9: an exec FIXED map contained in a LIVE backing is dyld's non-cache-dylib strategy
            // (reserve the image's span, then MAP_FIXED each segment with its own protections).
            // The block may already be translated, so promotion alone would leave the guest running
            // on a stale RW/UXN entry — promote, then invalidate on the guest itself.
            //
            // Idempotent with the caller's own set_region_exec (retrace-core's mmap dispatch calls
            // it on both record and replay): a range that is already ATTR_CODE is found and left
            // unchanged. Doing it here keeps the flush adjacent to the reason for it.
            if exec {
                self.set_region_exec(addr, rlen as u64);
                self.flush_guest_tlb();
            }
            return Some(addr);
        }
        // case 3: true partial straddle -> unchanged fail-loud assert
```

Update the doc comment above `place_fixed`: the paragraph beginning "`exec` is the W^X guard" now describes the opposite behaviour. Replace it with what is now true — that an exec request over a live backing is promoted and then flushed via `flush_guest_tlb`, and that case 2 is therefore reachable with `exec` set.

Also update `map_mmap_region`'s step-3c comment (~line 1477–1483): the parenthetical "if a run shows it, add a guest-side TLBI" has been discharged. Say so, and say that block-exclusive placement for non-FIXED exec mmaps is now an optimisation (it avoids a flush) rather than a correctness requirement.

- [ ] **Step 6: Run the test to verify it passes**

```bash
cargo test -p retrace --test tlbiexec_e2e -- --test-threads=1
```

Expected: PASS, 2 tests.

- [ ] **Step 7: Run the full gate**

```bash
just gate
```

Expected: 177 passed / 0 failed / 0 ignored, clippy clean. Pay attention to `execmap_e2e`, `fixedinner_e2e`, `fixedstraddle`, and `fixedwild` — they cover the paths this task edited.

- [ ] **Step 8: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-guest/asm/tlbiexec.s \
        crates/retrace-guest/build.rs crates/retrace-guest/src/lib.rs \
        crates/retrace/tests/tlbiexec_e2e.rs
git commit -m "M9 t3: honor MAP_FIXED PROT_EXEC over a live backing (promote, then flush)"
```

---

### Task 4: argc/argv in the process-start stack

Independent of Tasks 1–3 — it can proceed even on a NO-GO verdict.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs:1153-1184` (`build_start_stack`), and `load_dynamic`'s call at line 1090
- Modify: `crates/retrace-core/src/lib.rs:64-67` (`record_dynamic`)
- Modify: `crates/retrace/src/main.rs:30-54`
- Modify: `crates/retrace/tests/util/mod.rs`
- Create: `crates/retrace-guest/c/argv_echo.c`
- Create: `crates/retrace/tests/argv_e2e.rs`

**Interfaces:**
- Consumes: nothing from Tasks 1–3.
- Produces: `Box_::load_dynamic(exe, dyld, argv: &[String])`, `retrace_core::record_dynamic(exe, dyld, argv: &[String], trace_path)`, `util::record_dynamic_args(guest, &["-n", "1+1"])`, and `util::assert_rung_records_and_replays(guest, argv, expect_stdout)`. Task 5 uses all of them.

- [ ] **Step 1: Write the guest**

`crates/retrace-guest/c/argv_echo.c` — a real dynamic guest, same recipe as `hello_dyn.c`:

```c
// M9. Proves argc/argv reach a real dynamically-linked program through dyld's process-start stack.
// Before M9, build_start_stack pushed argv[0] only and hardcoded argc=1, so no guest could take an
// argument — and jq without a filter does nothing.
#include <unistd.h>
#include <string.h>

int main(int argc, char **argv) {
    if (argc < 2) { write(1, "NOARG\n", 6); return 1; }
    write(1, argv[1], strlen(argv[1]));
    write(1, "\n", 1);
    return 0;
}
```

Add to `crates/retrace-guest/build.rs` (after the `crashy` block) and `src/lib.rs`:

```rust
    // argv_echo: prints argv[1]. The M9 argv fixture — a real dynamic guest, same recipe as
    // hello_dyn (real toolchain, links libSystem, plain -arch arm64).
    let src = format!("{}/c/argv_echo.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/argv_echo");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang argv_echo");
    assert!(status.success(), "argv_echo guest build failed");
```

```rust
pub const ARGV_ECHO: &str = concat!(env!("OUT_DIR"), "/argv_echo");
```

- [ ] **Step 2: Write the failing test**

`crates/retrace/tests/argv_e2e.rs`:

```rust
// M9. argc/argv must reach a real dynamically-linked guest through dyld's process-start stack.
mod util;

#[test]
fn argv_reaches_a_dynamic_guest_and_replays() {
    let out = util::assert_rung_records_and_replays(
        retrace_guest::ARGV_ECHO, &["M9-ARGV"], b"M9-ARGV\n");
    assert_eq!(out.stdout, b"M9-ARGV\n");
}

#[test]
fn no_argv_still_works() {
    // argc==1 must remain valid — every existing dynamic guest passes no arguments, and this is
    // what pins that the widening did not change the argc=1 layout dyld already accepts.
    let (rec, _trace) = util::record_dynamic(retrace_guest::ARGV_ECHO);
    assert_eq!(rec.code, 1, "with no argument the guest takes its NOARG branch. stderr:\n{}", rec.stderr);
    assert_eq!(rec.stdout, b"NOARG\n");
}
```

- [ ] **Step 3: Run it to verify it fails**

```bash
cargo test -p retrace --test argv_e2e -- --test-threads=1
```

Expected: FAIL to compile — `assert_rung_records_and_replays` takes 2 arguments, not 3.

- [ ] **Step 4: Widen `build_start_stack`**

In `crates/retrace-box/src/lib.rs`, change the signature and the two argv-dependent lines:

```rust
    fn build_start_stack(stack: &Backing, argv: &[String], main_hdr: u64) -> u64 {
        let base_ipa = stack.ipa;
        let top = stack.ipa + stack.len as u64;
        let argv0 = argv.first().map(String::as_str).unwrap_or("");
        // strings[0..argc] = argv; the rest are apple[] entries (order irrelevant — parsed by key).
        let mut strings: Vec<Vec<u8>> = Vec::new();
        let push = |s: String, out: &mut Vec<Vec<u8>>| { let mut v = s.into_bytes(); v.push(0); out.push(v); };
        for a in argv { push(a.clone(), &mut strings); }                         // argv[0..argc]
        push(format!("executable_path={argv0}"), &mut strings);                  // apple[0]
        push("ptr_munge=0x1a2b3c4d5e6f7a8b".to_string(), &mut strings);          // libpthread cookie (nonzero)
        push("stack_guard=0x000a0b0c0d0e0f00".to_string(), &mut strings);        // __stack_chk_guard (low byte 0)
        push("malloc_entropy=0x00112233445566778899aabbccddeeff".to_string(), &mut strings); // libmalloc 2x64 entropy
        let argc = argv.len();
        let n_apple = strings.len() - argc;
```

and the words block:

```rust
        // KernelArgs words: mainExecutable, argc, argv[0..argc], NULL, NULL(envp), apple[0..], NULL.
        let mut words = vec![main_hdr, argc as u64];
        words.extend((0..argc).map(|i| addr[i]));
        words.push(0);                                          // argv terminator
        words.push(0);                                          // envp terminator (empty)
        words.extend((0..n_apple).map(|i| addr[argc + i]));
        words.push(0); // apple[] terminator
```

Note the original built `vec![main_hdr, 1, addr[0], 0, 0]` — the two trailing zeros are the argv terminator and the empty envp. Keeping them as separate pushes makes that explicit and correct for any `argc`.

Update `load_dynamic`'s signature to `argv: &[String]` and its call at line 1090 to pass it through. Update the doc comment on `build_start_stack` to describe `argv[0..argc]` instead of a single `argv[0]`.

- [ ] **Step 5: Thread it through core and the CLI**

`crates/retrace-core/src/lib.rs`:

```rust
pub fn record_dynamic(exe: &retrace_guest::Loaded, dyld: &retrace_guest::Loaded, argv: &[String],
                      trace_path: &Path) -> Result<RecordSummary, String> {
    record_box(Box_::load_dynamic(exe, dyld, argv), trace_path)
}
```

`crates/retrace/src/main.rs`, in the `record-dyn` arm — everything after `--` is the guest's, and `argv[0]` is the guest path (what the kernel passes and what `executable_path=` needs):

```rust
            // retrace record-dyn <exe> -o <trace> [-- <guest args…>]
            let guest = &a[2];
            let out = a.iter().position(|s| s == "-o").map(|i| a[i+1].clone()).expect("-o <trace>");
            // Guest argv: argv[0] is the exe path (what the kernel passes and what dyld's
            // `executable_path=` is derived from); everything after `--` is the guest's own.
            let mut argv = vec![guest.clone()];
            if let Some(i) = a.iter().position(|s| s == "--") { argv.extend_from_slice(&a[i+1..]); }
```

and change the call to `retrace_core::record_dynamic(&exe, &dyld, &argv, Path::new(&out))`.

Update the usage string at line 91 to mention the separator.

- [ ] **Step 6: Extend the test helpers**

In `crates/retrace/tests/util/mod.rs`:

```rust
// Record a dynamically-linked guest through real dyld, passing `args` as the guest's argv[1..].
pub fn record_dynamic_args(guest: &str, args: &[&str]) -> (RunOut, std::path::PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(2_000_000);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let trace = std::env::temp_dir().join(format!("retrace-argv-{}-{n}.bin", std::process::id()));
    let mut argv = vec!["record-dyn", guest, "-o", trace.to_str().unwrap()];
    if !args.is_empty() { argv.push("--"); argv.extend_from_slice(args); }
    (run(&argv), trace)
}
```

and give `assert_rung_records_and_replays` an `argv: &[&str]` parameter between `guest` and `expect_stdout`, routing through `record_dynamic_args`. Update its two existing callers (`hello_dyn_e2e`, `hello_rust_e2e`) to pass `&[]`.

- [ ] **Step 7: Run the tests to verify they pass**

```bash
cargo test -p retrace --test argv_e2e -- --test-threads=1
cargo test -p retrace --test hello_dyn_e2e --test hello_rust_e2e -- --test-threads=1
```

Expected: all PASS. The two rung gates passing with `&[]` is what proves the widening did not disturb the `argc=1` layout dyld already accepts.

- [ ] **Step 8: Run the full gate and commit**

```bash
just gate
git add -A
git commit -m "M9 t4: argc/argv in dyld's process-start stack, and -- on record-dyn"
```

Expected gate: 179 passed / 0 failed / 0 ignored.

---

### Task 5: The rung-2 gate — drive `brew jq`

**This task is iterative and its endpoint is not knowable in advance.** Spec risk R3: the wall-chain past the fourth `mmap` is of unknown depth. Work the loop — probe, read the wall, fix, probe — exactly as the M2 sub-milestones did, and stop when either jq prints `2` or you hit a wall that needs its own milestone.

**Files:**
- Create: `crates/retrace/tests/jq_e2e.rs`
- Modify: whatever the wall-chain demands (record honestly in the commit messages)

**Interfaces:**
- Consumes: `flush_guest_tlb` (Task 2), the relaxed `place_fixed` (Task 3), `record_dynamic_args` and the argv-aware rung helper (Task 4).
- Produces: either a green rung-2 gate or a parked one with a documented wall.

- [ ] **Step 1: Confirm the environment, and decide the skip policy**

```bash
ls -l /opt/homebrew/bin/jq && /opt/homebrew/bin/jq --version && file "$(python3 -c "import os;print(os.path.realpath('/opt/homebrew/bin/jq'))")"
```

Expected: `jq-1.8.2`, `Mach-O 64-bit executable arm64`.

`jq` is not a repo artifact, so the gate must behave on a machine without it. **Spec open question 1 — resolve it before writing the test, and write the decision into the test's header comment.** The default this plan recommends: skip with an explicit `eprintln!` when `/opt/homebrew/bin/jq` is absent, because a gate that fails on a clean checkout is worse than one that announces it was skipped — but a silent skip is exactly the kind of quiet green this project's honest-gate discipline exists to prevent, so the announcement is not optional.

- [ ] **Step 2: Write the rung-2 gate**

`crates/retrace/tests/jq_e2e.rs`:

```rust
// M9 rung-2 gate. `brew jq` — the first guest that loads dylibs which are NOT in the dyld shared
// cache (libjq.1.dylib and libonig.5.dylib, both files on disk under /opt/homebrew).
//
// `-n` (null input) is deliberate: it makes jq do real work driven by a real argument without
// opening the stdin surface, so this gate tests one new capability, not three.
//
// NOT a repo artifact: jq comes from Homebrew. When it is absent the test announces the skip loudly
// rather than passing quietly — a silent skip reads as a green it did not earn.
mod util;

const JQ: &str = "/opt/homebrew/bin/jq";

#[test]
fn jq_records_and_replays() {
    if !std::path::Path::new(JQ).exists() {
        eprintln!("SKIPPED jq_records_and_replays: {JQ} not installed (`brew install jq`). \
                   This gate did NOT run — it is not evidence of anything.");
        return;
    }
    let out = util::assert_rung_records_and_replays(JQ, &["-n", "1+1"], b"2\n");
    assert_eq!(out.stdout, b"2\n");
}
```

- [ ] **Step 3: Run it and read the wall**

```bash
cargo test -p retrace --test jq_e2e -- --test-threads=1 --nocapture
```

If it fails, get the actual wall — this is the first tool to reach for:

```bash
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn /opt/homebrew/bin/jq -o /tmp/jq.bin -- -n '1+1' 2>&1 | tail -40
```

Recorded baseline before this milestone: **exit 101, 105 traps**, panicking in `place_fixed`. After Task 3 that specific wall is gone, so the trap count must be **higher** than 105. If it is not, Task 3 did not take effect on this path — diagnose that before anything else.

- [ ] **Step 4: Work the wall-chain**

For each wall, in order:

1. Read the last few `[trap]` lines and the failure. `RETRACE_TRACE=1` decodes `mach_msg2` sends too.
2. Classify it: is the guest asking for something legitimate (retrace must implement it), or something impossible (the guest gets an errno, per M8-stack's fast-follow rule — only retrace's OWN invariants may fail loud)?
3. Fix it, honoring symmetry rule 1 if the fix touches record's `match stop`, or rule 2 if it belongs below the trace.
4. Add a focused regression test for the specific mechanism — not just "jq got further".
5. Re-run the gate. Commit each wall separately with a message naming the wall and the mechanism.

Expected territory, from the spec: chained-fixup binding against two non-cache dylibs, rpath and symlink resolution through `Cellar` (`/opt/homebrew/opt/oniguruma` is a symlink), and jq's own init and allocation load.

- [ ] **Step 5: Land the honest outcome**

- **jq prints `2`:** leave `jq_e2e` un-`#[ignore]`d. Rung 2 is green.
- **A wall remains:** park `jq_e2e` with `#[ignore = "..."]` whose reason states the **actual** wall — the trap number, the pc, the mechanism — not a vague "not yet supported". Do **not** weaken the assertions to make it pass, and do **not** touch `hello_dyn_e2e` or `hello_rust_e2e`.

- [ ] **Step 6: Run the full gate and commit**

```bash
just gate
git add -A
git commit -m "M9 t5: rung 2 — <the honest outcome, green or parked at <wall>>"
```

---

### Task 6: Documentation — the Status section and the honest close

**Files:**
- Modify: `README.md` (append a new `## Status:` section), `CLAUDE.md` (gate count + milestone list)

**Interfaces:**
- Consumes: the measured outcomes of Tasks 1–5.
- Produces: the authoritative log of what runs today and what the next wall is.

- [ ] **Step 1: Write the Status section**

Append to `README.md`, following the shape of the existing 21 Status sections. It must state:

- What the TLBI oracle is and why it was the right shape (the codebase predicted it at `lib.rs:1482`; both halves — the sign stub's discipline and `ATTR_TRAMP` — already existed).
- The spike's **measured** F1/F2/F3 answers, including F2's control result even if it was surprising.
- The exact gate count from a real `just gate` run.
- Rung 2's honest outcome — green, or parked with the wall named precisely.
- The next boundary, carried forward: guest-raised signal delivery (still the top item), `prot` ignored except `PROT_EXEC`, spec risk R3 (8 MiB believed vs 256 KiB backed), `guest_munmap`'s wholesale-drop defect, the `guest_mmap_replay` rename, threads, arm64e dynamic guests.
- Whether the block-exclusive exec placement hack and the anon-`PROT_EXEC`/JIT gap were actually retired or merely made retirable.

**Do not write the Status section from this plan.** Write it from what the runs actually did. If Task 1's F2 said the stale entry was not real, or Task 5 parked, the section says that.

- [ ] **Step 2: Update `CLAUDE.md`**

Update the gate count in the "Honest-gate discipline" paragraph to the measured number, and add M9 to the milestone list. If `jq_e2e` is parked, the sentence "Nothing in the gate is `#[ignore]`d" becomes false — fix it and name what is parked and why.

- [ ] **Step 3: Verify the claims**

```bash
just gate 2>&1 | tee /tmp/gate-m9.log
grep -a 'test result:' /tmp/gate-m9.log | sed 's/\x1b\[[0-9;]*m//g' \
  | awk '{for(i=1;i<=NF;i++){if($(i+1)~/^passed/)p+=$i; if($(i+1)~/^failed/)f+=$i; if($(i+1)~/^ignored/)g+=$i}} END{printf "passed=%d failed=%d ignored=%d\n",p,f,g}'
```

The gate log carries ANSI escapes — strip them before grepping or the counts silently miss lines. Put the number this prints into the docs, not a remembered one.

- [ ] **Step 4: Commit**

```bash
git add README.md CLAUDE.md
git commit -m "M9 close: <the honest one-line outcome>"
```

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
| --- | --- |
| M9-spike | Task 1 |
| M9-tlbi (`flush_guest_tlb`) | Task 2 |
| M9-fixed (`place_fixed` relaxation) | Task 3 |
| M9-argv | Task 4 |
| Exit criterion — capability gate | Task 3 Step 3 |
| Exit criterion — rung-2 gate | Task 5 |
| Determinism posture | Task 2 (below-trace placement), Task 3 Step 3 (`assert_trace_reproducible`) |
| Correctness invariant | Task 3 Step 5 (the promote-then-flush path + comment rewrite) |
| Testing | Tasks 2, 3, 4, 5 |
| R1 (TLBI unavailable) | Task 1 Step 4 gate |
| R2 (VMALLE1 coarse) | Task 2 constant comment; measured in Task 5 |
| R3 (wall-chain depth) | Task 5 structure; Task 6 honest close |
| R4 (EL1 stub state) | Task 2 Step 1 test (`preserves_architectural_state`) |
| Open question 1 (jq availability) | Task 5 Step 1 |
| Open question 2/3 (TLBI variant/scope) | Task 1 (spike), Task 2 constant comment |
| Open question 4 (stage-2 flags) | Task 3 Step 6 — surfaces as a `tlbiexec_e2e` failure if real |
| Open question 5 (exec map count) | Task 5 Step 3's trap log |

**Placeholder scan:** no TBD/TODO; every code step carries real code. Task 5 is deliberately open-ended — that is the milestone's honest shape, not a placeholder, and its steps are concrete procedures rather than "figure it out".

**Type consistency:** `flush_guest_tlb(&mut self)` — defined Task 2, called Task 3. `set_region_exec_attr(ipa, len, attr)` — introduced Task 2 Step 4, used by `ensure_tlbi_stub`. `build_start_stack(stack, argv: &[String], main_hdr)`, `load_dynamic(exe, dyld, argv: &[String])`, `record_dynamic(exe, dyld, argv: &[String], trace_path)` — all widened together in Task 4. `record_dynamic_args(guest, args: &[&str])` and `assert_rung_records_and_replays(guest, argv: &[&str], expect_stdout: &[u8])` — defined Task 4 Step 6, used in Task 4 Step 2 and Task 5 Step 2. `TLBIEXEC`/`TLBIEXEC_FIXTURE`/`ARGV_ECHO` constants — defined alongside their build.rs entries.

**One gap the implementer must close:** Task 2's test calls `b.regs_snapshot()`, which may not exist. Step 4 says to add it as a read-only helper over the vector `snapshot()` already captures. If `Box_` exposes an equivalent accessor under another name, use that instead of adding one.
