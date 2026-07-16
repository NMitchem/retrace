# M3 Implementation Plan — reverse execution: position, single-step, scripted debug

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Position replay at an arbitrary instant **P = (landmark N, step K)** in a recording and step it backward, shipped as a scripted `retrace debug <trace> --script '…'` subcommand whose transcript is byte-identical across sessions — the M3 determinism oracle.

**Architecture:** Nothing executes backward: every reverse op computes an earlier coordinate and re-seeks forward (restore snapshot → replay N events natively → single-step K instructions). Single-step is hardware (`MDSCR_EL1.SS` + `PSTATE.SS` + the already-wrapped `set_trap_debug_exceptions`) because the HVF guest has **no PMU instruction counter** (PMUVer=0, spike-proven) — architectural step is the only exact tick source. The engine is `ReplaySession`, a code-motion refactor of `replay()`'s loop (its five locals become fields); `replay()` is rebuilt on top so the oracle and the debugger share one engine. **Zero trace-format changes.** Spec: `docs/superpowers/specs/2026-07-16-retrace-m3-reverse-execution-design.md` — read it before starting.

**Tech Stack:** Rust workspace; Hypervisor.framework via `hv-sys`; arm64 guests built by `crates/retrace-guest/build.rs`; C spikes in `spikes/` (manual clang + codesign).

## Global Constraints

- **Branch:** create `m3-revexec` from `main` (spec is on main at `9b533e6`): `git checkout -b m3-revexec`.
- **Every test run uses `--test-threads=1`** (HVF: one VM per process). Full gate: `just gate`. **Baseline: 78 passed / 0 failed / 0 ignored, clippy clean** (post-M2-taskinfo merge).
- **One VM per process also inside a single test:** never hold two `Box_`/`ReplaySession` values alive at once — capture what you need, **drop the first session, then open the second**. Two live sessions = `HV_BUSY`.
- **Zero trace-format changes.** Do not touch `retrace-trace`'s `Event` or `TRACE_MAGIC`. Nothing about stepping/debugging enters a recording.
- **Never fake a green.** `reverse_debug_e2e` is written `#[ignore]`d (Task 6) and un-ignored only if the walk genuinely passes twice.
- **Spike gates architecture.** Task 2's step-exception arm and Task 5's breakpoint engine each have two candidate forms; Task 1's empirical findings pick one. Do not implement both live paths — implement the proven one, note the other in a comment.
- **Clippy `-D warnings` clean at every commit.** Codesigning is automatic for `cargo test`/`cargo run`; spikes and `CARGO_BIN_EXE_retrace`-spawned binaries sign manually (`util::bin()` already does the latter).
- **Commit messages:** `M3 tN: <what>` + trailing `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` (match the executing model).

### Exact values (verbatim — copy, do not reinvent)

- **Coordinate:** `P = (landmark N, step K)` — state after the first `N` trace events are consumed (`N` = `ReplaySession.idx`, same number as `Divergence.landmark`) plus `K` retired instructions. `reverse-stepi` from `(N, 0)` = `(N−1, window_len(N−1))`; from `(N, K>0)` = `(N, K−1)`.
- **Step accounting:** a below-the-trace emulated instruction (timebase MRS `try_emulate_timebase`, undef MRS `try_emulate_undef_mrs`, FPAC strip `try_emulate_fpac_auth` — `crates/retrace-box/src/lib.rs:407/:439/:471`) counts as **exactly one step**; a cache/reservation fault (`Stop::Other` handled by `page_in_cache`/`commit_reserved_page`) counts **zero** (nothing retired).
- **Bits:** `PSTATE.SS` = bit **21** (set in the value written to `reg::CPSR`); `MDSCR_EL1.SS` = bit **0**; `MDSCR_EL1.MDE` = bit **15**. EC codes (already decoded by `retrace_arch::ec_of`): SoftStep `0x32/0x33`, Breakpoint `0x30/0x31`, Watchpoint `0x34/0x35`.
- **hv-sys raw names:** `hv_sys_reg_t_HV_SYS_REG_MDSCR_EL1` (=32786), `hv_sys_reg_t_HV_SYS_REG_DBGBVR0_EL1` / `…DBGBCR0_EL1`, fn `hv_vcpu_set_trap_debug_reg_accesses`. The safe-wrapper pattern to mirror is `hv-sys/src/lib.rs:53–79` (`sysreg::` consts) and `:94` (`set_trap_debug_exceptions`).
- **DBGBCR0 arm value for an EL0 unlinked address match:** `0x1E5` = E(bit0)=1 | PMC(bits2:1)=0b10 (EL0) | BAS(bits8:5)=0xF.
- **Trampoline routing fact:** retrace's guest takes *every* EL0 exception through the EL1 vector at `TRAMPOLINE_IPA` whose slots are `hvc #0`, so exceptions surface to the VMM either **(a)** directly as an EL2 exit (outer `ec_of(exit.syndrome)`) or **(b)** as `Ec::Hvc` with the true cause in `ESR_EL1`. The spike decides which route debug exceptions take.
- **Known green-path landmark for the e2e:** the `write(1,"hi\n",3)` trap on `hello_dyn` is BSD syscall `num == 4`, `args[0] == 1` — discover its landmark index and pc from the recording at runtime (do not hardcode the pc; slides are fixed per-loader but discovery is cheaper than a constant that can rot).
- **Gate arithmetic:** T1: 78/0/0 (no cargo tests). T2: **82/0/0** (+4 box step tests). T3: **83/0/0** (+1 seek test). T4: **85/0/0** (+2). T5: **89/0/0** (+4 parser tests). T6 parked: **89/0/1**; T6 green: **90/0/0**.
- **CLI exit codes:** 2 usage, 3 divergence, 4 record error (existing); **5 = debug script error** (new).

---

### Task 1: `spikes/sstep.c` — prove step delivery, routing, re-arm, and HW breakpoints

**Files:**
- Create: `spikes/sstep.c`
- Modify: `spikes/README.md` — new `## sstep.c` findings section

**Interfaces:**
- Consumes: nothing from the workspace (standalone C, adapts `spikes/hvspike.c`'s scaffold).
- Produces: three written findings Task 2 and Task 5 depend on — **(F1)** step-exception route (direct-EL2 vs trampoline/ESR_EL1), **(F2)** SS re-arm across a trapped-then-skipped instruction works, **(F3)** HW breakpoint (DBGBVR0/DBGBCR0 + MDE) delivery yes/no + its route.

- [ ] **Step 1: Write `spikes/sstep.c`.** Adapt `hvspike.c`'s scaffold (same `rstr`, map, vcpu boilerplate) to retrace's real shape: EL0 guest + EL1 `hvc #0` trampoline. Full program:

```c
// sstep.c — prove HVF single-step + HW breakpoints for M3, in retrace's exact shape:
// guest at EL0, VBAR_EL1 -> trampoline page whose 16 vector slots are each `hvc #0`.
// Answers: (F1) which route does a step exception take? (F2) does SS survive a
// trapped+manually-skipped instruction (retrace's below-the-trace emulation)? (F3) do
// DBGBVR0/DBGBCR0 HW breakpoints deliver, and via which route?
#include <Hypervisor/Hypervisor.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>

#define CODE_IPA  0x10000000ULL
#define TRAMP_IPA 0x10004000ULL
#define PG        0x4000
#define PSTATE_SS (1ULL << 21)
#define MDSCR_SS  (1ULL << 0)
#define MDSCR_MDE (1ULL << 15)

static const char *rstr(hv_return_t r){switch((uint32_t)r){case 0:return"HV_SUCCESS";
  case 0xfae94007:return"HV_DENIED";case 0xfae94003:return"HV_BAD_ARGUMENT";
  case 0xfae94004:return"HV_ILLEGAL_GUEST_STATE";default:return"OTHER";}}

static hv_vcpu_t vcpu; static hv_vcpu_exit_t *vexit;

static uint64_t sys(hv_sys_reg_t r){uint64_t v=0;hv_vcpu_get_sys_reg(vcpu,r,&v);return v;}
static uint64_t rg(hv_reg_t r){uint64_t v=0;hv_vcpu_get_reg(vcpu,r,&v);return v;}

// Run once; classify. Returns: 1=step via direct EL2, 2=step via trampoline(ESR_EL1),
// 3=other trap via trampoline (ESR_EL1 EC printed), 4=breakpoint direct, 5=breakpoint via
// trampoline, 0=anything else (printed).
static int run_once(const char *tag){
    hv_return_t r = hv_vcpu_run(vcpu);
    uint64_t esr2 = vexit->exception.syndrome; uint32_t ec2 = (esr2>>26)&0x3f;
    uint64_t pc = rg(HV_REG_PC), elr = sys(HV_SYS_REG_ELR_EL1), esr1 = sys(HV_SYS_REG_ESR_EL1);
    uint32_t ec1 = (uint32_t)((esr1>>26)&0x3f);
    printf("[%s] run=%s reason=%u EC2=0x%02x pc=0x%llx | ESR_EL1 EC1=0x%02x elr=0x%llx\n",
           tag, rstr(r), vexit->reason, ec2, pc, ec1, elr);
    if (ec2==0x32||ec2==0x33) return 1;
    if (ec2==0x16 && (ec1==0x32||ec1==0x33)) return 2;
    if (ec2==0x30||ec2==0x31) return 4;
    if (ec2==0x16 && (ec1==0x30||ec1==0x31)) return 5;
    if (ec2==0x16) return 3;
    return 0;
}

// After a trampoline-routed stop, resume EL0 at `at` with pstate `ps` (SPSR_EL1 semantics).
static void resume_el0(uint64_t at, uint64_t ps){
    hv_vcpu_set_reg(vcpu, HV_REG_PC, at);
    hv_vcpu_set_reg(vcpu, HV_REG_CPSR, ps);
}

int main(void){
    if (hv_vm_create(NULL)) { printf("no vm\n"); return 1; }
    // EL0 code page: nop x4, mrs x1,cntvct_el0 (traps EL0->EL1 when CNTKCTL.EL0VCTEN=0),
    // nop x4, then spin (we never run off the end under stepping).
    uint32_t code[11]; for (int i=0;i<11;i++) code[i]=0xD503201F;      // nop
    code[4]=0xD53BE041;                                                // mrs x1, cntvct_el0
    code[10]=0x14000000;                                               // b . (spin)
    void *cb = mmap(NULL,PG,PROT_READ|PROT_WRITE,MAP_ANON|MAP_PRIVATE,-1,0);
    memcpy(cb,code,sizeof(code));
    hv_vm_map(cb,CODE_IPA,PG,HV_MEMORY_READ|HV_MEMORY_EXEC);
    // EL1 trampoline: 16 vector slots, 0x80 apart, each `hvc #0`.
    void *tb = mmap(NULL,PG,PROT_READ|PROT_WRITE,MAP_ANON|MAP_PRIVATE,-1,0);
    for (int i=0;i<16;i++) ((uint32_t*)tb)[i*0x80/4]=0xD4000002;       // hvc #0
    hv_vm_map(tb,TRAMP_IPA,PG,HV_MEMORY_READ|HV_MEMORY_EXEC);

    hv_vcpu_config_t cfg = hv_vcpu_config_create();
    hv_vcpu_create(&vcpu,&vexit,cfg);
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_VBAR_EL1,TRAMP_IPA);
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_SCTLR_EL1,0x30d00800);         // MMU off
    printf("set_trap_debug_exceptions -> %s\n",
           rstr(hv_vcpu_set_trap_debug_exceptions(vcpu,true)));

    // ---- F1 + step loop: arm SS, expect one retired insn per run ----
    hv_vcpu_set_reg(vcpu,HV_REG_PC,CODE_IPA);
    hv_vcpu_set_reg(vcpu,HV_REG_CPSR,PSTATE_SS);                       // EL0t + SS
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_MDSCR_EL1,MDSCR_SS);
    int route = 0;
    for (int i=0;i<4;i++){                                             // step the 4 leading nops
        int k = run_once("SS");
        if (i==0) route = k;
        uint64_t next = (k==2) ? sys(HV_SYS_REG_ELR_EL1) : rg(HV_REG_PC);
        printf("   step %d -> next=0x%llx (expect 0x%llx)\n", i+1, next, CODE_IPA+4ULL*(i+1));
        if (k==2) resume_el0(next, PSTATE_SS);                          // trampoline route: re-enter EL0
        else      hv_vcpu_set_reg(vcpu,HV_REG_CPSR, rg(HV_REG_CPSR)|PSTATE_SS); // direct: re-arm SS
        hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_MDSCR_EL1,MDSCR_SS);
    }
    printf("F1: step route = %s\n", route==1?"DIRECT-EL2":route==2?"TRAMPOLINE(ESR_EL1)":"??");

    // ---- F2: the next insn is the trapping MRS. It should NOT step-except; it traps
    // (EC1=0x18 via trampoline). Manually skip it (+4, retrace's emulation), keep SS armed,
    // and confirm the NEXT run yields exactly one more step exception at +4 further. ----
    int k = run_once("MRS");
    printf("   mrs trapped as k=%d (expect 3 w/ EC1=0x18; if k==%d it stepped natively)\n", k, route);
    if (k==3) resume_el0(sys(HV_SYS_REG_ELR_EL1)+4, PSTATE_SS);        // skip the MRS like run() does
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_MDSCR_EL1,MDSCR_SS);
    k = run_once("F2");
    printf("F2: re-arm across skipped insn -> %s\n", (k==route)?"OK (one step)":"UNEXPECTED");

    // ---- F3: disarm SS; arm DBGBVR0/DBGBCR0 at nop #8; run free; expect breakpoint. ----
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_MDSCR_EL1,MDSCR_MDE);          // MDE on, SS off
    uint64_t cur = rg(HV_REG_PC); (void)cur;
    hv_vcpu_set_reg(vcpu,HV_REG_CPSR, rg(HV_REG_CPSR)&~PSTATE_SS);
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_DBGBVR0_EL1,CODE_IPA+8*4);     // addr of nop #8 (index 8)
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_DBGBCR0_EL1,0x1E5);            // E=1 PMC=EL0 BAS=0xF
    k = run_once("BVR");
    printf("F3: HW breakpoint = %s\n", k==4?"DIRECT-EL2":k==5?"TRAMPOLINE(ESR_EL1)":"NOT DELIVERED (step-scan fallback)");

    hv_vcpu_destroy(vcpu); hv_vm_destroy();
    return 0;
}
```

- [ ] **Step 2: Build, sign, run** (recipe per `spikes/README.md`; use the perl timeout wrapper — a wedged vCPU otherwise hangs forever):

```sh
cd spikes
clang -O2 -o sstep sstep.c -framework Hypervisor
codesign -s - -f --entitlements ent.plist sstep
perl -e '$p=fork;if(!$p){setpgrp;exec@ARGV or exit 127}$SIG{ALRM}=sub{kill"-KILL",$p;exit 124};alarm 15;wait;exit($?>>8)' ./sstep
```

Expected: `set_trap_debug_exceptions -> HV_SUCCESS`; four `step N -> next=…` lines matching `CODE_IPA+4·N`; an `F1:` route verdict; `F2: … OK (one step)`; an `F3:` verdict. If the F1 loop makes no progress (pc never advances) or exits with an unexpected EC, capture the exact output — that is a genuine platform wall; stop and report rather than proceeding to Task 2.

- [ ] **Step 3: Document findings in `spikes/README.md`.** Add a `## sstep.c — single-step + HW breakpoint delivery (M3)` section following the house shape (build recipe + quoted output + one bullet per finding F1/F2/F3, each phrased as the claim Task 2/5 will rely on).

- [ ] **Step 4: Commit.**

```bash
git add spikes/sstep.c spikes/README.md
git commit -m "M3 t1: sstep spike — step delivery route, SS re-arm, HW breakpoint verdict

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: `Box_::step()` — hv-sys MDSCR wrapper, `Stop::Step`, the steppy guest, box-level tests

**Files:**
- Modify: `crates/hv-sys/src/lib.rs` — `sysreg::MDSCR_EL1` (+ `DBGBVR0_EL1`/`DBGBCR0_EL1` and a `Vcpu::set_trap_debug_reg_accesses` wrapper **only if F3 said DELIVERED**).
- Modify: `crates/retrace-box/src/lib.rs` — `Stop::Step` variant, `Box_::step()`, fail-loud `Ec::SoftStep` arm in `run()`, enable `set_trap_debug_exceptions(true)` at vcpu setup.
- Create: `crates/retrace-guest/asm/steppy.s`; Modify: `crates/retrace-guest/build.rs` + `src/lib.rs` — the `STEPPY` path constant (mirror `HELLO`'s build+constant lines exactly).
- Test: `crates/retrace-box/tests/step.rs`

**Interfaces:**
- Consumes: Task 1's F1 route verdict (picks the step-classification arm) and F2 (validates emulation accounting).
- Produces: `Stop::Step` (new unit variant on `pub enum Stop`); `Box_::step(&mut self) -> Stop` — advances **exactly one** guest instruction (hardware step or one below-the-trace emulation), or returns `Stop::Syscall{..}` (window end — trap not consumed) / `Stop::Other{..}` (fault — nothing retired) unchanged for the caller to handle. Tasks 3–5 rely on this exact contract.

- [ ] **Step 1: Write the steppy guest.** Create `crates/retrace-guest/asm/steppy.s` copying `asm/hello.s`'s directives/entry-symbol/exit-sequence shape exactly, with this body between entry and exit (the exit is hello.s's existing raw `exit(0)` svc sequence):

```asm
    nop
    nop
    nop
    nop
    mrs x1, cntvct_el0    // may trap-and-emulate (timebase) or retire natively — either is one step
    nop
    nop
    nop
```

Wire it in `build.rs` and `src/lib.rs` by duplicating the `HELLO` compile step and path constant as `STEPPY` (same flags: freestanding, `-nostdlib -static`).

- [ ] **Step 2: Write the failing tests.** Create `crates/retrace-box/tests/step.rs`, constructing the box the same way the existing `crates/retrace-box/tests/` files do (e.g. `pac.rs`): `Box_::load(&retrace_guest::parse_macho(&std::fs::read(retrace_guest::STEPPY).unwrap()))` (match the exact construction the neighbors use):

```rust
// Box_::step(): one instruction per call — hardware step, or one below-the-trace
// emulation (the steppy MRS), each exactly one step; the window-ending svc is
// returned as Stop::Syscall, unconsumed.
use retrace_box::{Box_, Stop};

#[test]
fn step_advances_one_insn_at_a_time() {
    let mut b = load_steppy();
    let pc0 = b.pc();
    for i in 1..=4u64 {
        assert!(matches!(b.step(), Stop::Step), "step {i}");
        assert_eq!(b.pc(), pc0 + 4 * i, "pc after step {i}");
    }
}

#[test]
fn step_crosses_the_mrs_as_one_step() {
    let mut b = load_steppy();
    for _ in 0..4 { assert!(matches!(b.step(), Stop::Step)); }
    let at_mrs = b.pc();
    assert!(matches!(b.step(), Stop::Step), "the MRS is one step (emulated or native)");
    assert_eq!(b.pc(), at_mrs + 4);
}

#[test]
fn step_reaches_window_end_as_unconsumed_syscall() {
    let mut b = load_steppy();
    let mut steps = 0u64;
    loop {
        match b.step() {
            Stop::Step => { steps += 1; assert!(steps < 64, "runaway"); }
            Stop::Syscall { num, .. } => { assert_eq!(num, 1, "exit(0) svc"); break; }
            other => panic!("unexpected: {other:?}"),
        }
    }
    assert_eq!(steps, 8, "4 nops + mrs + 3 nops before the exit sequence begins"); // adjust to steppy.s's exact pre-svc insn count incl. hello.s's exit-sequence setup instructions
}
```

And the spec's fail-loud case — an SS leak reaching `run()` must panic, not masquerade as `Stop::Other`:

```rust
#[test]
#[should_panic(expected = "software-step exception outside Box_::step()")]
fn unarmed_step_exception_fails_loud() {
    let mut b = load_steppy();
    b.dbg_leak_ss(); // test-only: arm MDSCR_EL1.SS + PSTATE.SS without using step()
    b.run();         // must hit the fail-loud Ec::SoftStep arm
}
```

(`load_steppy()` is a local helper wrapping the construction above. If `Stop` doesn't derive `Debug`, add `#[derive(Debug)]` to it. `dbg_leak_ss` is a `#[doc(hidden)] pub fn` on `Box_` — two `set_sys`/`set_reg` calls mirroring `step()`'s arm sequence; the `Drop` order (`vcpu` before `vm`) makes the unwind safe under `--test-threads=1`.)

- [ ] **Step 3: Run to verify failure.** `cargo test -p retrace-box --test step -- --test-threads=1` — Expected: FAIL to compile (`no variant Step`, `no method step`).

- [ ] **Step 4: Implement.** In `crates/hv-sys/src/lib.rs`, add to the `sysreg` consts (pattern of lines 53–79):

```rust
pub const MDSCR_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_MDSCR_EL1);
```

In `crates/retrace-box/src/lib.rs`: add `Step` to `Stop` (line 233); call `vcpu.set_trap_debug_exceptions(true)` once wherever the vcpu is configured in `load`/`load_dynamic`/`restore` (next to the existing `VBAR_EL1` setup); add near the top:

```rust
const PSTATE_SS: u64 = 1 << 21; // PSTATE/SPSR software-step bit
const MDSCR_SS: u64 = 1 << 0;   // MDSCR_EL1.SS
```

Then `step()`. **Pick ONE routing arm per F1 and delete the other's marker comment.** The shape (trampoline-route version shown; the direct-EL2 version differs only where marked):

```rust
    /// Advance exactly one guest instruction. Returns Stop::Step on success;
    /// Stop::Syscall if the next instruction is the window-ending trap (NOT consumed —
    /// caller decides); Stop::Other for faults (nothing retired — caller pages in and
    /// retries). Below-the-trace emulations (timebase/undef-MRS/FPAC) count as the step.
    pub fn step(&mut self) -> Stop {
        let mdscr = self.vcpu.get_sys(sysreg::MDSCR_EL1).unwrap();
        self.vcpu.set_sys(sysreg::MDSCR_EL1, mdscr | MDSCR_SS).unwrap();
        let cpsr = self.vcpu.get_reg(reg::CPSR).unwrap();
        self.vcpu.set_reg(reg::CPSR, cpsr | PSTATE_SS).unwrap();
        let stop = self.run_one_for_step();
        // disarm: SS out of MDSCR and out of live PSTATE so run()/forward paths never step
        let mdscr = self.vcpu.get_sys(sysreg::MDSCR_EL1).unwrap();
        self.vcpu.set_sys(sysreg::MDSCR_EL1, mdscr & !MDSCR_SS).unwrap();
        let cpsr = self.vcpu.get_reg(reg::CPSR).unwrap();
        self.vcpu.set_reg(reg::CPSR, cpsr & !PSTATE_SS).unwrap();
        stop
    }

    /// One hv_vcpu_run classification for step(): mirrors run()'s match exactly, EXCEPT
    /// (1) a SoftStep exception is the success case, and (2) an emulation arm firing
    /// RETURNS Stop::Step instead of `continue` (the emulated insn IS the step).
    fn run_one_for_step(&mut self) -> Stop {
        loop {
            let e = self.vcpu.run().expect("hv_vcpu_run");
            if e.reason != EXIT_EXCEPTION { continue; }
            match ec_of(e.syndrome) {
                Ec::Hvc => {
                    let esr1 = self.vcpu.get_sys(sysreg::ESR_EL1).unwrap();
                    match ec_of(esr1) {
                        // F1 said TRAMPOLINE: the step exception arrives here. Resume
                        // point is ELR_EL1 with SPSR_EL1 (SS already consumed).
                        Ec::SoftStep => {
                            let elr = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
                            let spsr = self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap();
                            self.vcpu.set_reg(reg::PC, elr).unwrap();
                            self.vcpu.set_reg(reg::CPSR, spsr & !PSTATE_SS).unwrap();
                            return Stop::Step;
                        }
                        Ec::Svc => {}
                        Ec::SysReg if self.try_emulate_timebase(esr1) => return Stop::Step,
                        Ec::Other(0)    if self.try_emulate_undef_mrs() => return Stop::Step,
                        Ec::Other(0x1C) if self.try_emulate_fpac_auth() => return Stop::Step,
                        _ => { self.last_far = self.vcpu.get_sys(sysreg::FAR_EL1).unwrap();
                               return Stop::Other { esr: esr1 }; }
                    }
                    let num = self.vcpu.get_reg(reg::x(16)).unwrap();
                    let mut args = [0u64; 8];
                    for (i, a) in args.iter_mut().enumerate() { *a = self.vcpu.get_reg(reg::x(i as u32)).unwrap(); }
                    return Stop::Syscall { num, args };
                }
                // F1 said DIRECT-EL2 instead: match Ec::SoftStep HERE on e.syndrome and
                // return Stop::Step (pc is already at the boundary; just fall through to
                // the disarm in step()).
                _ => { self.last_far = e.virtual_address; return Stop::Other { esr: e.syndrome } }
            }
        }
    }
```

Finally the fail-loud arm in `run()` itself — inside its inner `match ec_of(esr1)`, before the `_ =>` arm (and mirrored on the outer match if F1 said direct):

```rust
                        Ec::SoftStep => panic!(
                            "software-step exception outside Box_::step() — SS leaked; pc=0x{:x}",
                            self.vcpu.get_sys(sysreg::ELR_EL1).unwrap()),
```

- [ ] **Step 5: Run the tests.** `cargo test -p retrace-box --test step -- --test-threads=1` — Expected: PASS (4 tests). If `step_reaches_window_end_as_unconsumed_syscall`'s count assert fails, count steppy.s's actual pre-svc instructions (including hello.s's exit-sequence `mov` setup you copied) and fix the constant — the count must be exact, not `>=`.

- [ ] **Step 6: Full gate.** `just gate` — Expected: **82 / 0 / 0**, clippy clean.

- [ ] **Step 7: Commit.**

```bash
git add crates/hv-sys/src/lib.rs crates/retrace-box/src/lib.rs crates/retrace-guest crates/retrace-box/tests/step.rs
git commit -m "M3 t2: Box_::step() — hardware single-step below the trace, one insn per call

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: `ReplaySession` — code-motion refactor of `replay()`, landmark seek, determinism test

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` — `ReplaySession`, `Advance`; `replay()` rebuilt on top.
- Test: `crates/retrace/tests/seek.rs` (+ `mod util;` reuse)

**Interfaces:**
- Consumes: nothing new (pure motion of the existing loop; `Box_::restore/snapshot/diff_memory/dbg_regs/read_guest/is_mapped/position` all exist).
- Produces (Tasks 4–6 rely on these exact signatures):

```rust
pub struct ReplaySession { /* b, events, idx, stdout, guest_task_port */ }
pub enum Advance { Event, Exited(ReplayReport) }
impl ReplaySession {
    pub fn open(trace_path: &Path) -> Result<Self, String>;
    pub fn advance(&mut self) -> Result<Advance, Divergence>;          // consume exactly ONE trace event (or exit)
    pub fn advance_to_landmark(&mut self, n: usize) -> Result<(), Divergence>;
    pub fn landmark(&self) -> usize;                                    // = idx
    pub fn pc(&self) -> u64;                                            // Box_::position()
    pub fn dbg_regs(&self) -> String;
    pub fn read_mem(&self, va: u64, len: usize) -> Option<Vec<u8>>;     // None if unmapped
    pub fn snapshot(&mut self) -> (retrace_trace::Regs, Vec<retrace_trace::Region>);
    pub fn diff_memory(&self, expect: &[retrace_trace::Region]) -> Option<String>; // thin delegate;
        // mirror Box_::diff_memory's EXACT return type (crates/retrace-box/src/lib.rs:1479)
}
```

- [ ] **Step 1: Write the failing test.** Create `crates/retrace/tests/seek.rs`:

```rust
// The M3-pos oracle: seeking the same landmark twice yields byte-identical machine state.
mod util;
use retrace_core::ReplaySession;

#[test]
fn landmark_seek_is_deterministic() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let trace = std::path::Path::new(&trace);

    // Session 1: seek landmark 100, capture state, then DROP (one VM per process).
    let (regs1, pc1, snap_mem) = {
        let mut s = ReplaySession::open(trace).unwrap();
        s.advance_to_landmark(100).unwrap();
        assert_eq!(s.landmark(), 100);
        let (_, mem) = s.snapshot();
        (s.dbg_regs(), s.pc(), mem)
    };

    // Session 2: same seek, byte-compare registers and full memory.
    let mut s = ReplaySession::open(trace).unwrap();
    s.advance_to_landmark(100).unwrap();
    assert_eq!(s.dbg_regs(), regs1);
    assert_eq!(s.pc(), pc1);
    assert!(s.diff_memory(&snap_mem).is_none(), "memory diverged between two seeks");
}
```

(Expose `diff_memory` on the session as a thin delegate to `Box_::diff_memory` — add it to the Produces block; the e2e never needs it but tests do.)

- [ ] **Step 2: Verify failure.** `cargo test -p retrace --test seek -- --test-threads=1` — Expected: FAIL to compile (`no ReplaySession in retrace_core`).

- [ ] **Step 3: The code motion.** In `crates/retrace-core/src/lib.rs`, mechanical transformation of `replay()` (currently :371–:621) — **move, do not rewrite**:

1. Define `ReplaySession` with fields exactly the five locals: `b: Box_`, `events: Vec<Event>`, `idx: usize`, `stdout: Vec<u8>`, `guest_task_port: Option<u64>`. `open()` = the current prologue (`open_checked`, snapshot extraction, `Box_::restore`, field init `idx = 1`).
2. `advance()` = ONE iteration of the current `loop { match b.run() { … } }`, with these exact rewrites: every local `b`/`idx`/`stdout`/`guest_task_port` reference becomes `self.…`; every `idx += 1; continue;` at the end of a consumed-event arm becomes `self.idx += 1; return Ok(Advance::Event);`; the `Stop::Other` page-in/commit `continue`s STAY as `continue` inside `advance`'s own inner `loop` (they consume no event — `advance` returns only on event consumption or exit); the `SYS_EXIT` path (Exit landmark check + final-snapshot `diff_memory` + `return Ok(ReplayReport…)`) becomes `return Ok(Advance::Exited(ReplayReport { … }))`.
3. `replay()` becomes:

```rust
pub fn replay(trace_path: &Path) -> Result<ReplayReport, Divergence> {
    let mut s = ReplaySession::open(trace_path)
        .map_err(|e| Divergence { landmark: 0, pc: 0, detail: e })?;
    loop {
        if let Advance::Exited(report) = s.advance()? { return Ok(report); }
    }
}
```

4. `advance_to_landmark(n)`: `while self.idx < n { if let Advance::Exited(_) = self.advance()? { return Err(Divergence { landmark: self.idx, pc: self.pc(), detail: format!("run exited before landmark {n}") }); } } Ok(())` — note `advance` consumes exactly one event so this lands on exactly `idx == n` (error if `n` < current `idx` too: add that guard with the same `Divergence`-shaped error).
5. The accessors are one-line delegates to the `Box_` methods named in Produces.

- [ ] **Step 4: Verify the refactor changed nothing.** `just gate` — Expected: **83 / 0 / 0** (78 + 4 step tests + this seek test), clippy clean. The e2e `hello_dyn_e2e` passing here IS the proof the motion was faithful.

- [ ] **Step 5: Commit.**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace/tests/seek.rs
git commit -m "M3 t3: ReplaySession — replay() as a resumable engine; landmark seek deterministic

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: `step_insns` / `seek` / `window_len` — the (N, K) coordinate is live

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` — three methods on `ReplaySession` + one free function.
- Test: `crates/retrace/tests/seek.rs` (extend)

**Interfaces:**
- Consumes: `Box_::step() -> Stop` (Task 2), `ReplaySession` (Task 3).
- Produces:

```rust
impl ReplaySession {
    /// Step exactly k instructions past the current landmark. Err names the window
    /// length if the window ends first (no silent clamp). Session is spent on Err.
    pub fn step_insns(&mut self, k: u64) -> Result<(), String>;
    /// Steps until the window-ending trap, returning the window length. Session is spent.
    pub fn window_len_here(&mut self) -> Result<u64, String>;
}
/// Fresh session positioned at P = (n, k).
pub fn seek(trace_path: &Path, n: usize, k: u64) -> Result<ReplaySession, String>;
```

- [ ] **Step 1: Write the failing tests.** Append to `crates/retrace/tests/seek.rs`:

```rust
#[test]
fn window_len_is_deterministic() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0);
    let trace = std::path::Path::new(&trace);
    let (n, l1) = first_window_with_len(trace, 4);
    let l2 = { let mut s = retrace_core::seek(trace, n, 0).unwrap(); s.window_len_here().unwrap() };
    assert_eq!(l1, l2, "window {n} length differs between sessions");
}

#[test]
fn step_seek_is_deterministic_and_window_end_errors() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0);
    let trace = std::path::Path::new(&trace);
    let (n, len) = first_window_with_len(trace, 4);
    let k = len / 2;
    let (regs1, pc1, mem1) = {
        let mut s = retrace_core::seek(trace, n, k).unwrap();
        let (_, mem) = s.snapshot(); (s.dbg_regs(), s.pc(), mem)
    };
    let mut s = retrace_core::seek(trace, n, k).unwrap();
    assert_eq!(s.dbg_regs(), regs1);
    assert_eq!(s.pc(), pc1);
    assert!(s.diff_memory(&mem1).is_none());
    // past-the-end is a clean, length-naming error
    let err = retrace_core::seek(trace, n, len + 1).unwrap_err();
    assert!(err.contains(&len.to_string()), "error should name the window length: {err}");
}

/// Probe a few landmarks for a window of at least `min` instructions (one session each,
/// SEQUENTIAL — never two alive). Deterministic per-trace.
fn first_window_with_len(trace: &std::path::Path, min: u64) -> (usize, u64) {
    for n in [10usize, 30, 60, 100, 150] {
        let mut s = retrace_core::seek(trace, n, 0).unwrap();
        let l = s.window_len_here().unwrap();
        drop(s);
        if l >= min { return (n, l); }
    }
    panic!("no window of >= {min} insns among the probes");
}
```

- [ ] **Step 2: Verify failure.** `cargo test -p retrace --test seek -- --test-threads=1` — Expected: FAIL to compile (`no fn seek`, `no method step_insns`).

- [ ] **Step 3: Implement.** In `crates/retrace-core/src/lib.rs`:

```rust
impl ReplaySession {
    pub fn step_insns(&mut self, k: u64) -> Result<(), String> {
        for done in 0..k {
            loop {
                match self.b.step() {
                    Stop::Step => break,
                    Stop::Other { esr } => {
                        // deterministic replay faults: page in and retry, zero steps counted
                        if self.b.page_in_cache(self.b.fault_ipa()) { continue; }
                        if self.b.commit_reserved_page(self.b.fault_ipa()) { continue; }
                        return Err(format!("fault during step {done}/{k}: {}", self.b.describe_stop(esr)));
                    }
                    Stop::Syscall { .. } => return Err(format!(
                        "window {} ends after {done} instruction(s); cannot step {k}", self.idx)),
                }
            }
        }
        Ok(())
    }

    pub fn window_len_here(&mut self) -> Result<u64, String> {
        let mut n = 0u64;
        loop {
            match self.b.step() {
                Stop::Step => n += 1,
                Stop::Other { esr } => {
                    if self.b.page_in_cache(self.b.fault_ipa()) { continue; }
                    if self.b.commit_reserved_page(self.b.fault_ipa()) { continue; }
                    return Err(format!("fault at step {n}: {}", self.b.describe_stop(esr)));
                }
                Stop::Syscall { .. } => return Ok(n),
            }
        }
    }
}

pub fn seek(trace_path: &Path, n: usize, k: u64) -> Result<ReplaySession, String> {
    let mut s = ReplaySession::open(trace_path)?;
    s.advance_to_landmark(n).map_err(|d| format!("seek to landmark {n}: {}", d.detail))?;
    s.step_insns(k)?;
    Ok(s)
}
```

(`page_in_cache`/`commit_reserved_page`/`fault_ipa`/`describe_stop` are already `pub` — replay's dispatch calls them today. If any is private to the crate, make it `pub` with a doc comment rather than duplicating logic.)

- [ ] **Step 4: Run the tests, then the full gate.** `cargo test -p retrace --test seek -- --test-threads=1` → PASS (3). `just gate` → **85 / 0 / 0**, clippy clean.

- [ ] **Step 5: Commit.**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace/tests/seek.rs
git commit -m "M3 t4: (N,K) coordinate live — step_insns/window_len/seek, deterministic

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: `retrace debug <trace> --script '…'` — parser, executor, breakpoints, reverse ops

**Files:**
- Create: `crates/retrace/src/debug.rs` — command parser + script executor (+ its `#[cfg(test)]` parser tests).
- Modify: `crates/retrace/src/main.rs` — the `debug` arm.
- (If F3 said DELIVERED) Modify: `crates/hv-sys/src/lib.rs` (`DBGBVR0_EL1`/`DBGBCR0_EL1` consts, `set_trap_debug_reg_accesses` wrapper), `crates/retrace-box/src/lib.rs` (`arm_hw_breakpoint(slot, va)` / `clear_hw_breakpoint(slot)` setters + a `Breakpoint` classification the session can see), `crates/retrace-core/src/lib.rs` (mid-window hit support in `run_to_break`).

**Interfaces:**
- Consumes: `ReplaySession` + `seek`/`step_insns`/`window_len_here` (Tasks 3–4).
- Produces: `debug::run_script(trace: &Path, script: &str, out: &mut impl std::io::Write) -> Result<(), String>` — the CLI arm calls it with stdout; tests call it with a `Vec<u8>`. Exit 5 on `Err`.

**Command semantics (implement exactly — this is the transcript contract the e2e byte-compares):**

- Script = `;`-separated commands, trimmed. Each command echoes itself first: `> <command>\n`, then its output.
- `break <hex-addr>` / `delete <hex-addr>` — maintain a `Vec<u64>` (ordered, deduped). Output `breakpoint at 0x…` / `deleted 0x…`.
- `continue` — from the current position, `advance()` in a loop; after each consumed event, if `self.pc()` (the trap pc) equals a breakpoint, stop: position becomes `(landmark, 0)`, print `hit 0x… at (N, 0)`. If the run exits first, print `exited (code C)` and position becomes the exit. **(F3 DELIVERED only)** additionally arm HW slots for up to 6 breakpoints before running so mid-window hits also stop, printing `hit 0x… at (N, +?)` — then resolve K by a fresh window scan counting steps to that pc/hit-ordinal, and reprint `resolved (N, K)`.
- `reverse-continue` — fresh scan session from the trace start recording every hit `< current P` (same hit test as `continue`); seek the last one; print `hit 0x… at (N, K)` or `no earlier hit`.
- `stepi [n]` (default 1) — `step_insns(n)` from the current position; window end is a printed error line (`error: window N ends after M instruction(s)`), position unchanged (re-seek to the pre-command coordinate — sessions are spent by errors).
- `reverse-stepi [n]` — n times: `(N, K>0) → (N, K−1)`; `(N, 0) → (N−1, window_len(N−1))` via one probe seek. At `(1, 0)` print `at start of recording`. Implemented as coordinate arithmetic + one final `seek`.
- `regs` — print `dbg_regs()` verbatim. `x <hex-addr> <len>` — `read_mem`; print `<addr>: <2-hex bytes space-separated>` or `unmapped`. `where` — print `at (N, K) pc=0x…`.
- Position state in the executor = `(n: usize, k: u64)` + one live `Option<ReplaySession>`; every command that moves re-seeks fresh (drop the old session FIRST — one VM per process). Unknown command / bad hex = `Err` (exit 5).

- [ ] **Step 1: Write the failing parser tests** in `debug.rs`'s `#[cfg(test)]` (pure — no VM):

```rust
#[test] fn parses_commands() {
    let cs = parse_script("break 0x1804af834; continue; reverse-stepi 2; x 0x1000 16; where").unwrap();
    assert_eq!(cs, vec![Cmd::Break(0x1804af834), Cmd::Continue, Cmd::ReverseStepi(2),
                        Cmd::Examine(0x1000, 16), Cmd::Where]);
}
#[test] fn stepi_defaults_to_one() {
    assert_eq!(parse_script("stepi").unwrap(), vec![Cmd::Stepi(1)]);
    assert_eq!(parse_script("reverse-stepi").unwrap(), vec![Cmd::ReverseStepi(1)]);
}
#[test] fn rejects_unknown_and_bad_hex() {
    assert!(parse_script("frobnicate").is_err());
    assert!(parse_script("break zzz").is_err());
}
#[test] fn empty_segments_are_skipped() {
    assert_eq!(parse_script("regs;; where ;").unwrap(), vec![Cmd::Regs, Cmd::Where]);
}
```

- [ ] **Step 2: Verify failure.** `cargo test -p retrace debug -- --test-threads=1` — Expected: FAIL to compile.

- [ ] **Step 3: Implement `debug.rs`** — `enum Cmd { Break(u64), Delete(u64), Continue, ReverseContinue, Stepi(u64), ReverseStepi(u64), Regs, Examine(u64, usize), Where }` (derive `Debug, PartialEq`), `parse_script(&str) -> Result<Vec<Cmd>, String>`, and `run_script` executing the semantics block above verbatim. Wire `main.rs`:

```rust
        Some("debug") => {
            let trace = &a[2];
            let script = a.iter().position(|s| s == "--script").map(|i| a[i + 1].clone())
                .expect("--script '<cmds>'");
            match debug::run_script(Path::new(trace), &script, &mut std::io::stdout()) {
                Ok(()) => exit(0),
                Err(e) => { eprintln!("DEBUG ERROR: {e}"); exit(5); }
            }
        }
```

(with `mod debug;` declared at the top of `main.rs` — the bin crate has no lib target.)

- [ ] **Step 4: Run parser tests + a smoke script.** `cargo test -p retrace debug -- --test-threads=1` → PASS (4). Then a live smoke — the compiled hello_dyn lives in retrace-guest's build OUT_DIR:

```sh
HD=$(find target/debug/build -path '*retrace-guest*' -name hello_dyn -type f | head -1)
cargo run -p retrace -- record-dyn "$HD" -o "$TMPDIR/m3smoke.bin"
cargo run -p retrace -- debug "$TMPDIR/m3smoke.bin" --script 'where; stepi 3; where; reverse-stepi; where; regs'
```

Expected: transcript shows `(1, 0)` → `(1, 3)` → `(1, 2)` and a register dump; run the debug line twice — output byte-identical (`diff <(…) <(…)`).

- [ ] **Step 5: Full gate.** `just gate` — Expected: **89 / 0 / 0**, clippy clean.

- [ ] **Step 6: Commit.**

```bash
git add crates/retrace/src/debug.rs crates/retrace/src/main.rs crates/hv-sys/src/lib.rs crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs
git commit -m "M3 t5: retrace debug — scripted breakpoints, stepi/reverse-stepi, reverse-continue

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: `reverse_debug_e2e` — the M3 headline gate, walked honestly

**Files:**
- Create: `crates/retrace/tests/reverse_debug_e2e.rs`
- Modify: `README.md` — `## Status: M3 — reverse execution` section.
- (Memory update is the CONTROLLER's job — report outcome details; do NOT edit `~/.claude/`.)

**Interfaces:**
- Consumes: everything above, via the spawned CLI (`util::bin()` codesign pattern) plus in-process `ReplaySession` for landmark discovery.
- Produces: the honest M3 gate — green and un-ignored, or parked at a documented boundary.

- [ ] **Step 1: Write the gate, `#[ignore]`d.**

```rust
// M3 headline gate: a scripted reverse-debug session on a hello_dyn recording is
// deterministic — the full transcript is byte-identical across two independent sessions,
// and the coordinates behave (break -> reverse-stepi -> stepi round-trips).
mod util;

#[test]
#[ignore = "M3: parked until the debug walk is proven end-to-end"]
fn reverse_debug_transcript_is_deterministic() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);

    // Discover the write(1,...) trap's landmark + pc in-process (num 4, args[0]==1),
    // then DROP the session before any CLI spawn (one VM per process).
    let (bp_pc, wr_landmark) = {
        let mut s = retrace_core::ReplaySession::open(std::path::Path::new(&trace)).unwrap();
        loop {
            // advance until the NEXT event is the write; peek via the events accessor
            // (add `pub fn peek_syscall(&self) -> Option<(u64, [u64; 8])>` to ReplaySession
            // if not present: the (num, args) of events[idx] when it is a Syscall).
            if let Some((4, args)) = s.peek_syscall() {
                if args[0] == 1 { /* position AT the write trap: */
                    s.advance().unwrap();
                    break (s.pc(), s.landmark());
                }
            }
            s.advance().unwrap();
        }
    };

    let script = format!(
        "break 0x{bp_pc:x}; continue; where; regs; reverse-stepi; where; stepi; where; reverse-continue; where");
    let run = |()| {
        let out = std::process::Command::new(util::bin())
            .args(["debug", &trace, "--script", &script])
            .output().expect("spawn debug");
        assert_eq!(out.status.code(), Some(0), "debug failed: {}", String::from_utf8_lossy(&out.stderr));
        out.stdout
    };
    let t1 = run(()); let t2 = run(());
    assert_eq!(t1, t2, "transcript not byte-identical across sessions");

    let text = String::from_utf8(t1).unwrap();
    assert!(text.contains(&format!("hit 0x{bp_pc:x} at ({wr_landmark}, 0)")), "continue must hit the write trap:\n{text}");
    assert!(text.contains(&format!("at ({wr_landmark}, 0)")), "stepi after reverse-stepi must round-trip:\n{text}");
    assert!(text.contains("no earlier hit") || text.contains(&format!("hit 0x{bp_pc:x}")), "reverse-continue outcome must be stated:\n{text}");
}
```

Note the round-trip subtlety this asserts: `reverse-stepi` from `(W, 0)` goes to `(W−1, len)`; `stepi` from there must land back on a position whose `where` prints `(W, 0)`-equivalent state — if your position arithmetic prints `(W−1, len)` after the stepi instead, normalize: a position at a full window's end IS the next landmark's `(N, 0)` **only after the event is consumed**; keep them distinct (`(W−1, len)` = at the trap, unconsumed) and adjust the assertion to the exact strings your Task 5 semantics produce — then freeze them; the byte-compare is the real oracle.

- [ ] **Step 2: The walk.** `cargo test -p retrace --test reverse_debug_e2e -- --ignored --test-threads=1 --nocapture 2>&1 | tee <scratchpad>/m3-walk.log`. Triage — exactly one holds:
  - **(A) PASSES:** run it a SECOND time (fresh recording, fresh sessions) → PASS again. → Step 3A.
  - **(B) A genuine wall** (step exception misbehaves mid-dyld, a window fault class the step loop doesn't handle, transcript divergence): capture the exact failure (command, coordinate, pc, error text). → Step 3B. **Do NOT loosen an assertion to get green.**

- [ ] **Step 3A: Un-ignore.** Delete the `#[ignore]` attribute. `just gate` → **90 / 0 / 0**, clippy clean.

- [ ] **Step 3B: Re-park honestly.** Rewrite the `#[ignore]` reason naming the exact boundary (coordinate, pc, failure) and what sub-milestone would clear it. Gate stays **89 / 0 / 1**.

- [ ] **Step 4: README.** Add `## Status: M3 — reverse execution` mirroring the M2-taskinfo section's shape: what P=(N,K) is, the engine (re-replay + hardware single-step, no PMU → SS is the only tick source, spike findings F1–F3), the debug command surface, the gate outcome with honest arithmetic, and a Deferred list (checkpoints when replay time hurts; watchpoints; symbolication; interactive REPL; mid-window breakpoints if F3 said NOT DELIVERED).

- [ ] **Step 5: Final gate + commit.**

```bash
just gate   # 90/0/0 (outcome A) or 89/0/1 (outcome B)
git add crates/retrace/tests/reverse_debug_e2e.rs README.md
git commit -m "M3 t6: reverse_debug_e2e — <headline gate GREEN | parked at BOUNDARY>

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

Report the outcome (A/B) and, if B, the exact boundary one-liner back to the controller for the memory update.

---

## Notes for the executor

- **The one thing not to get wrong:** `step()` must count a below-the-trace emulation (timebase MRS / undef MRS / FPAC strip) as **exactly one step** and a page-in fault as **zero** — otherwise K silently differs between two seeks and every determinism test flakes in ways that look like HVF bugs. The accounting lives in `run_one_for_step`'s return-vs-continue choices; re-read them against the spec's "Step accounting" line before debugging anything else.
- **Sequential VMs, always:** a determinism test that holds two sessions alive gets `HV_BUSY`, which presents as a confusing `hv_vcpu_run` expect-panic. Capture → drop → reopen.
- **Task 1 gates Task 2/5 code shape.** If F1 = DIRECT-EL2, the `Ec::SoftStep` arm moves to the OUTER match (on `e.syndrome`) in both `run_one_for_step` and `run()`'s fail-loud arm. If F3 = NOT DELIVERED, skip every DBG*-related change in Task 5 and document boundary-only breakpoints in the README's Deferred list — do not build a step-scan `continue` (it would step-scan all of dyld init; the e2e only needs trap-boundary hits).
- **Transcript stability is the product.** Any nondeterministic content in a command's output (pointers from the host, timing, HashMap iteration order) breaks the byte-compare oracle. Format everything from guest state or the script itself.
- **If Task 6 parks (outcome B), stop.** The milestone's honest deliverable is the named boundary; do not begin fixing it inside this plan.
