# retrace M5 — Write Watchpoints Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `watch <addr> [len]` write watchpoints in the `retrace debug` CLI — forward `continue` stops at the next write, `reverse-continue` stops at the most recent write before P — catching both guest stores (hardware DBGWVR/DBGWCR) and recorded kernel writes (software check in `apply_and_return`).

**Architecture:** Mirror the existing hardware-breakpoint machinery (DBGBVR0-5 / `arm_hw_breakpoint` / `Advance::Break` / the reverse-continue forward-rescan) with a 4-slot watchpoint sibling, plus one software range-intersection at the single funnel all recorded syscall writes pass through. Spec: `docs/superpowers/specs/2026-07-18-retrace-m5-watchpoints-design.md` (approved, committed `4a62164`).

**Tech Stack:** Rust 1.95.0 (pinned), Hypervisor.framework via `hv-sys`, macOS 26 Apple Silicon, non-root.

## Global Constraints

- Branch: create `m5-watchpoints` from `main` before Task 1; all commits land there. Commit messages: `M5 t<N>: <what>`.
- **`--test-threads=1` always** (one VM per process; a bare `cargo test` flakes with HV_BUSY). Full gate: `just gate`.
- Any test that spawns the CLI itself must go through `util::bin()` (it codesigns `CARGO_BIN_EXE_retrace` with `retrace.entitlements` — the cargo runner does NOT cover spawned binaries).
- `clippy.toml` bans `Instant::now`/`SystemTime::now`/`std::thread` — do not introduce any of them. `cargo clippy --workspace -- -D warnings` must stay clean.
- **Trace format untouched**: no `Event` change, no `TRACE_MAGIC` bump. Record-side behavior must be bit-identical (the watch set is empty on record).
- Watchpoints (like breakpoints) are **NEVER armed while single-stepping** — only around `advance()` scans. `step()`/`step_insns`/`resolve_hit_k` always run disarmed.
- All test addresses/coordinates are **discovered from the freshly recorded trace at test time** — never hardcoded.
- **Existing golden transcripts must stay byte-identical**: do not edit `crates/retrace/tests/debug_cli.rs`, `reverse_debug_e2e.rs`, or `checkpoint_seek.rs`. New CLI transcript tests go in NEW files.
- Gate arithmetic baseline: `just gate` = **106 passed / 0 failed / 0 ignored** at branch point. Each task's final step states the expected running total; verify the actual count and record any discrepancy honestly in the task report.

## Context primer (for an engineer with zero repo context)

retrace records a guest program's run (every syscall + the kernel's memory writes) and replays it bit-for-bit, never re-executing syscalls. The debugger (`retrace debug <trace> --script '…'`) replays under a position coordinate **P = (N, K)**: landmark N (trace events consumed) and K instructions single-stepped into landmark N's "window". Key machinery you will mirror (all verified at HEAD `ed3cf8b`):

- `crates/retrace-box/src/lib.rs` — `Box_` is the VM. `run()` returns `Stop::{Syscall,Other{esr},Step}`; a debug exception surfaces as `Stop::Other` via the generic arm at `:1369-1376`, which stores `last_far = e.virtual_address`. Breakpoint plumbing: `MDSCR_MDE`/`DBGBCR_ARM`/`HW_BREAKPOINT_SLOTS` (`:110-120`), `arm_hw_breakpoint` (`:1454`), `clear_hw_breakpoints` (`:1466` — clears MDE unconditionally, which Task 3 fixes), `apply_and_return` (`:1629` — the syscall-write funnel), `fault_ipa()` (`:762`, returns `last_far`).
- `crates/retrace-core/src/lib.rs` — `ReplaySession` wraps `Box_` + the event list. `advance()` (`:437`) consumes exactly one event (returns `Advance::Event`), exits (`Advance::Exited`), or surfaces an armed breakpoint (`Advance::Break`, via the `Ec::Breakpoint` check at `:666` BEFORE the cache-fault fallbacks). `arm_breakpoints`/`clear_breakpoints` (`:720/:727`), `step_insns` (`:753`), `seek`/`checkpointed_seek` (bottom of file). `retrace-arch` already decodes ESR EC `0x34|0x35 => Ec::Watchpoint`.
- `crates/retrace/src/debug.rs` — the scripted CLI executor `Exec` (`:134`): commands parse into `Cmd`, `cmd_continue` (`:269`) arms breakpoints around an `advance()` scan and K-resolves hardware hits via `resolve_hit_k` (`:120`), `cmd_reverse_continue` (`:340`) rescans forward from (1,0) keeping the last hit strictly before P.
- Guests are tiny `-static` asm programs in `crates/retrace-guest/asm/`, built by `build.rs` into `OUT_DIR`, exported as path constants. `FILEIO` already exists (open → fstat writes a 256-byte statbuf → read writes 19 bytes into `buf` → write → close → exit).
- Tests: `crates/retrace/tests/`, helpers in `tests/util/mod.rs` (`record`, `record_dynamic`, `bin`). CLI transcript tests spawn `retrace debug` and assert on exact printed lines.

**The one un-probed empirical claim** (Task 1's spike): watchpoint *delivery semantics* under HVF. The M3 spike proved breakpoints deliver direct-to-EL2 pre-retire (EC 0x30); watchpoints should deliver EC 0x34 with FAR = accessed VA, reported **before the store retires**. Tasks 3-6 are written assuming pre-retire; if the spike falsifies that, apply the spec's documented fallback (park at `(n, k+1)`, print `(write completed at …)`, drop the progress rule for hardware hits) and note the deviation in every affected task report.

## File structure (whole milestone)

| File | Change |
|---|---|
| `spikes/dbgw.c` | NEW — F4 delivery-semantics probe (Task 1) |
| `spikes/README.md` | append F4 findings (Task 1) |
| `crates/retrace-guest/asm/watchloop.s` | NEW — deterministic store-loop guest (Task 2) |
| `crates/retrace-guest/build.rs` | build watchloop (Task 2) |
| `crates/retrace-guest/src/lib.rs` | `WATCHLOOP` const + parse test (Task 2) |
| `crates/hv-sys/src/lib.rs` | DBGWVR0-3/DBGWCR0-3 sysreg consts (Task 3) |
| `crates/retrace-box/src/lib.rs` | watchpoint arm/clear, MDE sharing fix, watch-range bookkeeping (T3), `apply_and_return` hook + `take_syscall_watch_hit` (T4) |
| `crates/retrace-core/src/lib.rs` | `arm_watchpoints`/`clear_watchpoints`/`far()`, `Advance::Watch` (T3), `Advance::WatchSyscall` + `finish_event` (T4) |
| `crates/retrace/src/debug.rs` | `watch`/`unwatch` commands, hit handling, progress rule (T5), reverse-continue integration (T6) |
| `crates/retrace/tests/watch.rs` | NEW — session-level tests (T3, T4) |
| `crates/retrace/tests/watch_cli.rs` | NEW — CLI golden transcripts (T5, T6) |
| `README.md` | M5 Status section (T7) |

---

### Task 1: Spike `spikes/dbgw.c` — F4a-F4d delivery semantics

**Files:**
- Create: `spikes/dbgw.c`
- Modify: `spikes/README.md` (append an F4 section)

**Interfaces:**
- Produces: empirical findings F4a (delivery route), F4b (FAR validity), F4c (pre/post-retire), F4d (BAS byte-select) recorded in `spikes/README.md`. Tasks 3-6 depend on these findings; nothing consumes the binary (gitignored, like all spikes).

- [ ] **Step 1: Create the branch**

```bash
cd "$(git rev-parse --show-toplevel)"
git checkout -b m5-watchpoints main
```

- [ ] **Step 2: Write `spikes/dbgw.c`**

```c
// dbgw.c — prove HVF write-watchpoints (DBGWVR0/DBGWCR0) for M5, in retrace's exact shape:
// guest at EL0, VBAR_EL1 -> trampoline page of `hvc #0` slots, set_trap_debug_exceptions(true),
// MMU off (VA == IPA), only anonymous guest memory. Answers, empirically, on this OS/silicon:
//   F4a: does a watched EL0 store deliver DIRECT to EL2 (ESR_EL2 EC=0x34), not via the guest VBAR?
//   F4b: does hv_vcpu_exit's virtual_address (FAR) hold the accessed VA?
//   F4c: pre- or post-retire? (read the watched qword back at the hit)
//   F4d: BAS byte-select: a strb to byte 0 with only bytes 4..7 watched must NOT fire.
// SAFETY: every phase ends at a terminal `hvc #0` (no free spin); still run under the external
// perl process-group timeout (no `timeout` binary on this platform).
//   clang -O2 -o dbgw dbgw.c -framework Hypervisor
//   codesign -s - -f --entitlements ent.plist dbgw
//   perl -e '$p=fork;if(!$p){setpgrp;exec@ARGV or exit 127}$SIG{ALRM}=sub{kill"-KILL",$p;exit 124};alarm 15;wait;exit($?>>8)' ./dbgw
#include <Hypervisor/Hypervisor.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>

#define CODE_IPA  0x10000000ULL
#define TRAMP_IPA 0x10004000ULL
#define DATA_IPA  0x10008000ULL
#define PG        0x4000
#define MDSCR_MDE (1ULL << 15)
// DBGWCR: E=1 (bit0) | PAC=0b10 EL0-only (bits2:1) | LSC=0b10 store-only (bits4:3) | BAS<<5.
#define DBGWCR(bas) (0x15ULL | ((uint64_t)(bas) << 5))

static hv_vcpu_t vcpu; static hv_vcpu_exit_t *vexit;
static uint64_t rg(hv_reg_t r){uint64_t v=0;hv_vcpu_get_reg(vcpu,r,&v);return v;}

// Run once and classify: 1 = watchpoint exit (EC2 0x34/0x35), 2 = terminal hvc (EC2 0x16), 0 = other.
static int run_once(const char *tag, uint64_t *far_out){
    hv_vcpu_run(vcpu);
    uint64_t esr2 = vexit->exception.syndrome;
    uint32_t ec2 = (uint32_t)((esr2>>26)&0x3f);
    uint64_t pc = rg(HV_REG_PC), far = vexit->exception.virtual_address;
    printf("[%s] reason=%u EC2=0x%02x pc=0x%llx far=0x%llx\n", tag, vexit->reason, ec2, pc, far);
    if (far_out) *far_out = far;
    if (ec2==0x34||ec2==0x35) return 1;
    if (ec2==0x16) return 2;
    return 0;
}

static void reset_guest(uint64_t pc){
    hv_vcpu_set_reg(vcpu, HV_REG_PC, pc);
    hv_vcpu_set_reg(vcpu, HV_REG_CPSR, 0);            // EL0t, SS clear
}

int main(void){
    if (hv_vm_create(NULL)) { printf("no vm\n"); return 1; }
    // Guest code (offsets from CODE_IPA):
    //   +0x00 movz x1, #0x1000, lsl #16   ; x1 = 0x10000000
    //   +0x04 movk x1, #0x8000            ; x1 = DATA_IPA
    //   +0x08 movz x2, #0x42
    //   +0x0c str  x2, [x1]               ; the watched 8-byte store (F4a/b/c)
    //   +0x10 hvc  #0                     ; terminal
    //   +0x14 strb w2, [x1]               ; byte-0 store (F4d entry point)
    //   +0x18 hvc  #0                     ; terminal
    uint32_t code[7] = { 0xD2A20001, 0xF2900001, 0xD2800842, 0xF9000022,
                         0xD4000002, 0x39000022, 0xD4000002 };
    void *cb = mmap(NULL,PG,PROT_READ|PROT_WRITE,MAP_ANON|MAP_PRIVATE,-1,0);
    memcpy(cb,code,sizeof(code));
    hv_vm_map(cb,CODE_IPA,PG,HV_MEMORY_READ|HV_MEMORY_EXEC);
    void *tb = mmap(NULL,PG,PROT_READ|PROT_WRITE,MAP_ANON|MAP_PRIVATE,-1,0);
    for (int i=0;i<16;i++) ((uint32_t*)tb)[i*0x80/4]=0xD4000002;       // hvc #0 vectors
    hv_vm_map(tb,TRAMP_IPA,PG,HV_MEMORY_READ|HV_MEMORY_EXEC);
    void *db = mmap(NULL,PG,PROT_READ|PROT_WRITE,MAP_ANON|MAP_PRIVATE,-1,0);
    memset(db,0,PG);
    hv_vm_map(db,DATA_IPA,PG,HV_MEMORY_READ|HV_MEMORY_WRITE);
    volatile uint64_t *data = (volatile uint64_t *)db;

    hv_vcpu_config_t cfg = hv_vcpu_config_create();
    hv_vcpu_create(&vcpu,&vexit,cfg);
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_VBAR_EL1,TRAMP_IPA);
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_SCTLR_EL1,0x30d00800);         // MMU off
    printf("set_trap_debug_exceptions -> %d\n",
           hv_vcpu_set_trap_debug_exceptions(vcpu,true));

    // ---- F4a/b/c: watch the full qword at DATA_IPA, run into the str ----
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_DBGWVR0_EL1,DATA_IPA);
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_DBGWCR0_EL1,DBGWCR(0xFF));
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_MDSCR_EL1,MDSCR_MDE);
    reset_guest(CODE_IPA);
    uint64_t far=0; int k = run_once("F4a", &far);
    uint64_t pc = rg(HV_REG_PC);
    printf("F4a: %s\n", k==1?"DELIVERED DIRECT-EL2 (EC=0x34/0x35)":
                        (k==2?"NOT DELIVERED (ran free to terminal hvc)":"UNEXPECTED EXIT"));
    printf("F4b: far=0x%llx vs accessed VA 0x%llx -> %s\n", far, DATA_IPA,
           far==DATA_IPA?"EXACT":"NOT EXACT (record what it holds)");
    printf("F4c: watched mem=0x%llx -> %s; pc=0x%llx (%s the str at +0xc)\n",
           *data, *data==0?"PRE-RETIRE (store not yet landed)":"POST-RETIRE (store landed)",
           pc, pc==CODE_IPA+0xcULL?"AT":"PAST");

    // Disarm and resume from wherever the hit parked us: must reach the terminal hvc with 0x42 stored.
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_DBGWCR0_EL1,0);
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_MDSCR_EL1,0);
    reset_guest(pc);
    k = run_once("resume", NULL);
    printf("   resume disarmed: %s, mem=0x%llx (expect 0x42)\n",
           k==2?"terminal hvc":"UNEXPECTED", *data);

    // ---- F4d: watch only bytes 4..7 (BAS=0xF0); run the strb to byte 0 ----
    *data = 0;
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_DBGWVR0_EL1,DATA_IPA);
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_DBGWCR0_EL1,DBGWCR(0xF0));
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_MDSCR_EL1,MDSCR_MDE);
    reset_guest(CODE_IPA+0x14ULL);
    k = run_once("F4d", NULL);
    printf("F4d: strb to byte 0 under BAS=0xF0 -> %s (mem=0x%llx)\n",
           k==2?"NO FIRE (BAS is byte-selective)":"FIRED (BAS NOT byte-selective!)", *data);

    hv_vcpu_destroy(vcpu); hv_vm_destroy();
    return 0;
}
```

- [ ] **Step 3: Build, sign, run (from `spikes/`)**

```bash
cd spikes
clang -O2 -o dbgw dbgw.c -framework Hypervisor
codesign -s - -f --entitlements ent.plist dbgw
perl -e '$p=fork;if(!$p){setpgrp;exec@ARGV or exit 127}$SIG{ALRM}=sub{kill"-KILL",$p;exit 124};alarm 15;wait;exit($?>>8)' ./dbgw
```

Expected (the pre-retire hypothesis — record what ACTUALLY prints): `F4a: DELIVERED DIRECT-EL2`, `F4b: … EXACT`, `F4c: … PRE-RETIRE … AT the str`, `resume … mem=0x42`, `F4d: NO FIRE`. If any line differs, that is a real finding, not a spike bug — capture it verbatim.

- [ ] **Step 4: Append the F4 findings to `spikes/README.md`**

Follow the existing per-spike format (build/run recipe + a `Findings` list). Record the four F4 verdicts exactly as observed, including the raw `EC2`/`far`/`pc` values from the run. If F4c shows POST-retire or F4b shows a non-exact FAR, add a line: "Tasks 3-6 must apply the spec's fallback semantics (spec §Risk register)."

- [ ] **Step 5: Commit**

```bash
git add spikes/dbgw.c spikes/README.md
git commit -m "M5 t1: dbgw spike — DBGWVR/DBGWCR watchpoint delivery under HVF (F4a-F4d)"
```

---

### Task 2: `watchloop` guest

**Files:**
- Create: `crates/retrace-guest/asm/watchloop.s`
- Modify: `crates/retrace-guest/build.rs` (append a build stanza)
- Modify: `crates/retrace-guest/src/lib.rs` (constant + test)

**Interfaces:**
- Produces: `retrace_guest::WATCHLOOP: &str` (path to the built binary). Guest behavior contract relied on by T3/T5/T6: **window 1** (landmark 1) contains exactly 8 8-byte stores to `target` (all from the SAME store pc, values 1..=8) and exactly one `strb` to `target2` byte 0; then `write(1, target, 8)` — whose recorded `args[1]` publishes `target`'s address for discovery — then `exit(0)`. `target`/`target2` are consecutive 8-aligned `.quad 0` slots.

- [ ] **Step 1: Write the failing test** (append to `crates/retrace-guest/src/lib.rs` tests mod)

```rust
    #[test]
    fn watchloop_guest_parses() {
        let l = parse_macho(&std::fs::read(WATCHLOOP).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
    }
```

And the constant (after the `SPINLOOP` line):

```rust
pub const WATCHLOOP: &str = concat!(env!("OUT_DIR"), "/watchloop");
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p retrace-guest watchloop_guest_parses -- --test-threads=1`
Expected: FAIL — `No such file or directory` (build.rs doesn't build it yet).

- [ ] **Step 3: Create `crates/retrace-guest/asm/watchloop.s`**

```asm
.section __TEXT,__text
.global _start
.p2align 2
_start:
    adrp x1, target@PAGE
    add  x1, x1, target@PAGEOFF
    mov  x2, #0                  // store value
    mov  x3, #8                  // 8 stores, values 1..=8, all from the SAME str pc
sloop:
    add  x2, x2, #1
    str  x2, [x1]                // THE watched store
    subs x3, x3, #1
    b.ne sloop
    adrp x4, target2@PAGE
    add  x4, x4, target2@PAGEOFF
    mov  w5, #0x5A
    strb w5, [x4]                // byte-0 store: the BAS negative (watch target2+4 must NOT fire)
    mov  x0, #1
    adrp x1, target@PAGE
    add  x1, x1, target@PAGEOFF
    mov  x2, #8
    mov  x16, #4                 // SYS_write(1, target, 8): publishes target's addr in the trace args
    svc  #0x80
    mov  x0, #0
    mov  x16, #1                 // SYS_exit
    svc  #0x80
.section __DATA,__data
.p2align 3
target:  .quad 0
target2: .quad 0
```

- [ ] **Step 4: Append the build stanza to `crates/retrace-guest/build.rs`** (before the closing `}`, following the `spinloop` stanza's exact shape)

```rust
    // watchloop: 8 same-pc 8-byte stores to `target`, one strb to `target2` byte 0 (the BAS
    // negative), write(1, target, 8) — publishing target's address in the trace args — exit(0).
    // The M5 watchpoint guest: deterministic first-writer/last-writer ground truth.
    let src = format!("{}/asm/watchloop.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/watchloop");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang watchloop");
    assert!(status.success(), "watchloop guest build failed");
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p retrace-guest watchloop_guest_parses -- --test-threads=1`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-guest/asm/watchloop.s crates/retrace-guest/build.rs crates/retrace-guest/src/lib.rs
git commit -m "M5 t2: watchloop guest — deterministic store-loop watch target"
```

Gate arithmetic: 106 + 1 = **107 / 0 / 0** expected at `just gate`.

---

### Task 3: Hardware watchpoints — hv-sys, Box_, ReplaySession, `Advance::Watch`

**Files:**
- Modify: `crates/hv-sys/src/lib.rs` (sysreg module, after the DBGB block at `:75-86`)
- Modify: `crates/retrace-box/src/lib.rs` (constants near `:110`; `Box_` struct fields; arm/clear near `:1449-1473`)
- Modify: `crates/retrace-core/src/lib.rs` (`Advance` enum `:409`; `Stop::Other` arm `:661-673`; session methods near `:720`)
- Modify: `crates/retrace/src/debug.rs` (fail-loud stop-gap arms so the exhaustive `Advance` matches compile; replaced in T5/T6)
- Create: `crates/retrace/tests/watch.rs`

**Interfaces:**
- Consumes: `retrace_guest::WATCHLOOP` (T2); F4 findings (T1).
- Produces (relied on by T4-T6):
  - `Box_::arm_hw_watchpoint(&mut self, slot: usize, va: u64, len: u64)` — panics on slot > 3, asserts len ∈ {1,2,4,8} and `va % len == 0`; also records `(va, len)` in `watch_ranges`.
  - `Box_::clear_hw_watchpoints(&mut self)` — zeroes DBGWVR/DBGWCR 0-3, clears `watch_ranges`, recomputes MDE.
  - `Box_` fields: `bps_armed: bool`, `wps_armed: bool`, `watch_ranges: Vec<(u64, u64)>` (all default false/empty in every constructor; NOT captured in `BoxState` — checkpoints are never taken while armed).
  - `ReplaySession::arm_watchpoints(&mut self, ranges: &[(u64, u64)])` (asserts ≤ 4), `clear_watchpoints(&mut self)`, `far(&self) -> u64`.
  - `Advance::Watch` (carries nothing — callers read `far()`/`pc()`/`landmark()` from the parked session).

- [ ] **Step 1: Write the failing tests** — create `crates/retrace/tests/watch.rs`

```rust
// Session-level watchpoint tests (M5). All addresses discovered from the freshly recorded trace:
// watchloop's write(1, target, 8) publishes `target` in the recorded syscall args.
mod util;
use std::path::Path;
use retrace_core::{Advance, ReplaySession};

/// `target`'s guest VA, from the recorded write(1, target, 8): the first fd-1 write's args[1].
fn discover_target(trace: &Path) -> u64 {
    let mut s = ReplaySession::open(trace).unwrap();
    loop {
        if let Some((4, args)) = s.peek_syscall() {
            if args[0] == 1 { return args[1]; }
        }
        s.advance().unwrap();
    }
}

#[test]
fn hw_watchpoint_fires_on_store_pre_retire_with_far() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let target = discover_target(tp);
    let mut s = ReplaySession::open(tp).unwrap();
    s.arm_watchpoints(&[(target, 8)]);
    match s.advance().unwrap() {
        Advance::Watch => {
            let far = s.far();
            assert!(far >= target && far < target + 8, "far {far:#x} outside watched [{target:#x}; +8)");
            // Pre-retire (spike F4c): the first store (value 1) has NOT landed yet.
            assert_eq!(s.read_mem(target, 8).unwrap(), vec![0u8; 8], "store must not have retired");
        }
        _ => panic!("expected Advance::Watch"),
    }
}

#[test]
fn watch_on_untouched_bytes_never_fires() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let target = discover_target(tp);
    // target2 = target + 8 (consecutive .quad slots). The guest strb's ONLY byte 0 of target2;
    // watching bytes 4..8 (BAS=0xF0 in the same doubleword) must never fire.
    let mut s = ReplaySession::open(tp).unwrap();
    s.arm_watchpoints(&[(target + 8 + 4, 4)]);
    loop {
        match s.advance().unwrap() {
            Advance::Event => continue,
            Advance::Exited(report) => { assert_eq!(report.exit_code, 0); break; }
            _ => panic!("watchpoint fired on untouched bytes"),
        }
    }
}

#[test]
fn mde_survives_clear_breakpoints_with_watches_armed() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let target = discover_target(tp);
    let mut s = ReplaySession::open(tp).unwrap();
    s.arm_watchpoints(&[(target, 8)]);
    s.arm_breakpoints(&[0xdead_0000]); // never matched
    s.clear_breakpoints();             // must NOT disarm the watchpoint (shared MDSCR.MDE)
    assert!(matches!(s.advance().unwrap(), Advance::Watch),
        "watchpoint died when breakpoints were cleared (MDE sharing bug)");
}
```

- [ ] **Step 2: Run to verify RED**

Run: `cargo test -p retrace --test watch -- --test-threads=1`
Expected: COMPILE FAIL — `arm_watchpoints`/`far` not found, `Advance::Watch` not found.

- [ ] **Step 3: hv-sys constants** — in `crates/hv-sys/src/lib.rs`, after the `DBGBVR5/DBGBCR5` lines:

```rust
    // Hardware data-watchpoint slots (4 comparators on Apple Silicon — spikes/README.md hvprobe).
    // DBGWVRn holds the 8-aligned doubleword base; DBGWCRn is the control word (E/PAC/LSC/BAS).
    // Verified in the M5 dbgw spike (F4): a watched EL0 store delivers direct to EL2, EC=0x34.
    pub const DBGWVR0_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_DBGWVR0_EL1);
    pub const DBGWCR0_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_DBGWCR0_EL1);
    pub const DBGWVR1_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_DBGWVR1_EL1);
    pub const DBGWCR1_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_DBGWCR1_EL1);
    pub const DBGWVR2_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_DBGWVR2_EL1);
    pub const DBGWCR2_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_DBGWCR2_EL1);
    pub const DBGWVR3_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_DBGWVR3_EL1);
    pub const DBGWCR3_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_DBGWCR3_EL1);
```

(If the F4a delivery EC differs from 0x34, adjust the comment to what the spike actually printed.)

- [ ] **Step 4: retrace-box** — constants (after `HW_BREAKPOINT_SLOTS`, `:120`):

```rust
// Hardware write-watchpoints (M5 debugger `watch`). DBGWCR_BASE = E=1 (bit0) | PAC=0b10 EL0-only
// (bits2:1) | LSC=0b10 store-only (bits4:3); the per-watch BAS byte-select mask goes in bits 12:5.
// 4 comparator pairs on this silicon (hvprobe), vs 6 breakpoints.
const DBGWCR_BASE: u64 = 0x15;
const HW_WATCHPOINT_SLOTS: [(hv_sys::SysReg, hv_sys::SysReg); 4] = [
    (sysreg::DBGWVR0_EL1, sysreg::DBGWCR0_EL1),
    (sysreg::DBGWVR1_EL1, sysreg::DBGWCR1_EL1),
    (sysreg::DBGWVR2_EL1, sysreg::DBGWCR2_EL1),
    (sysreg::DBGWVR3_EL1, sysreg::DBGWCR3_EL1),
];
```

`Box_` struct: add three fields (put them beside the existing debug-machinery fields; remember `vcpu` before `vm` order is load-bearing — do not reorder anything):

```rust
    // M5 watchpoint state. NOT captured in BoxState: checkpoints are only ever taken via
    // checkpointed_seek on freshly-seeked (unarmed) sessions, so armed state never needs to persist.
    bps_armed: bool,
    wps_armed: bool,
    watch_ranges: Vec<(u64, u64)>, // (va, len) armed write-watch ranges, for the software (syscall) check
```

Then `cargo build -p retrace-box 2>&1 | head -40` and add `bps_armed: false, wps_armed: false, watch_ranges: Vec::new(),` to every struct-literal constructor the compiler flags (expected: `load`/`restore`/`from_checkpoint` — however many exist, fix them all).

New methods (directly under `clear_hw_breakpoints`, `:1473`):

```rust
    /// Arm hardware write-watchpoint slot `slot` (0..=3) over `[va, va+len)`, len ∈ {1,2,4,8},
    /// va len-aligned (so the range sits inside one BAS doubleword — one watch, one slot). A watched
    /// EL0 store surfaces from `run()` as `Stop::Other` with an ESR_EL2 watchpoint class (EC=0x34)
    /// and FAR in `last_far`, before the store retires (spike F4). Armed only around
    /// `advance()`/`run()` scans — NEVER while single-stepping (same discipline as breakpoints).
    pub fn arm_hw_watchpoint(&mut self, slot: usize, va: u64, len: u64) {
        assert!(matches!(len, 1 | 2 | 4 | 8), "watch len must be 1/2/4/8, got {len}");
        assert_eq!(va % len, 0, "watch va {va:#x} must be {len}-aligned");
        let (wvr, wcr) = HW_WATCHPOINT_SLOTS.get(slot)
            .copied()
            .unwrap_or_else(|| panic!("HW watchpoint slot {slot} out of range (0..=3)"));
        let bas = ((1u64 << len) - 1) << (va & 7);
        self.vcpu.set_sys(wvr, va & !7).unwrap();
        self.vcpu.set_sys(wcr, DBGWCR_BASE | (bas << 5)).unwrap();
        self.watch_ranges.push((va, len));
        self.wps_armed = true;
        self.sync_mde();
    }

    /// Disarm every hardware watchpoint slot and forget the watch ranges, recomputing MDE (which is
    /// shared with breakpoints — clearing one side must not disarm the other).
    pub fn clear_hw_watchpoints(&mut self) {
        for (wvr, wcr) in HW_WATCHPOINT_SLOTS {
            self.vcpu.set_sys(wvr, 0).unwrap();
            self.vcpu.set_sys(wcr, 0).unwrap();
        }
        self.watch_ranges.clear();
        self.wps_armed = false;
        self.sync_mde();
    }

    /// MDSCR_EL1.MDE gates breakpoints AND watchpoints; keep it set iff either side is armed.
    fn sync_mde(&mut self) {
        let mdscr = self.vcpu.get_sys(sysreg::MDSCR_EL1).unwrap();
        let v = if self.bps_armed || self.wps_armed { mdscr | MDSCR_MDE } else { mdscr & !MDSCR_MDE };
        self.vcpu.set_sys(sysreg::MDSCR_EL1, v).unwrap();
    }
```

**The MDE sharing fix** — edit the two existing breakpoint methods:
- `arm_hw_breakpoint` (`:1454`): replace its trailing two MDSCR lines with `self.bps_armed = true; self.sync_mde();`
- `clear_hw_breakpoints` (`:1466`): replace its trailing two MDSCR lines with `self.bps_armed = false; self.sync_mde();`

- [ ] **Step 5: retrace-core** — `Advance` (`:409`) becomes:

```rust
pub enum Advance { Event, Exited(ReplayReport), Break, Watch }
```

In `advance()`'s `Stop::Other` arm, right after the `Ec::Breakpoint` check (`:666-668`):

```rust
                    // A hardware watchpoint (M5 debugger) delivers here identically; surface it
                    // BEFORE the fault fallbacks. Only the debugger arms watchpoints.
                    if matches!(retrace_arch::ec_of(esr), retrace_arch::Ec::Watchpoint) {
                        return Ok(Advance::Watch);
                    }
```

Session methods (under `clear_breakpoints`, `:727`):

```rust
    /// Arm one hardware write-watchpoint per (va, len) range (one DBGWVR slot each) so a watched
    /// guest store surfaces from `advance()` as `Advance::Watch`. The 4-slot hardware limit is
    /// enforced upstream by the debugger's `watch` command. Cleared by `clear_watchpoints` or drop.
    pub fn arm_watchpoints(&mut self, ranges: &[(u64, u64)]) {
        assert!(ranges.len() <= 4, "watch command enforces the limit");
        for (slot, &(va, len)) in ranges.iter().enumerate() {
            self.b.arm_hw_watchpoint(slot, va, len);
        }
    }
    /// Disarm every hardware watchpoint (single-step-safe again).
    pub fn clear_watchpoints(&mut self) { self.b.clear_hw_watchpoints(); }
    /// The fault/watch address of the last `Stop::Other` (for a watchpoint hit: the accessed VA).
    pub fn far(&self) -> u64 { self.b.fault_ipa() }
```

- [ ] **Step 6: debug.rs stop-gap arms** — the exhaustive `Advance` matches now fail to compile. Add to BOTH the `cmd_continue` main loop match (after the `Advance::Exited` arm, `:328-332`) and `cmd_reverse_continue`'s inner loop match (`:350-354`):

```rust
                Advance::Watch => return Err("internal: watchpoint hit but none armed".into()),
```

(These are honest fail-loud arms — the CLI cannot arm watchpoints until T5/T6, which replace them.)

- [ ] **Step 7: Run to verify GREEN**

Run: `cargo test -p retrace --test watch -- --test-threads=1`
Expected: 3 passed. Note `mde_survives_clear_breakpoints_with_watches_armed` is the RED-turned-GREEN proof of the MDE fix — if you implement `sync_mde` but forget to rewire `clear_hw_breakpoints`, exactly that test fails.

Then regression: `cargo test -p retrace --test debug_cli --test reverse_debug_e2e --test checkpoint_seek -- --test-threads=1` — all pass unmodified.

- [ ] **Step 8: Clippy + commit**

Run: `cargo clippy --workspace -- -D warnings` — clean.

```bash
git add crates/hv-sys/src/lib.rs crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs crates/retrace/src/debug.rs crates/retrace/tests/watch.rs
git commit -m "M5 t3: hardware write-watchpoints — DBGW slots, MDE sharing fix, Advance::Watch"
```

Gate arithmetic: 107 + 3 = **110 / 0 / 0** expected at `just gate`.

---

### Task 4: Kernel-write detection — `apply_and_return` hook + `Advance::WatchSyscall`

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (`Box_` field; `apply_and_return` `:1629`; `take_syscall_watch_hit`)
- Modify: `crates/retrace-core/src/lib.rs` (`Advance` enum; `finish_event` helper; the ~11 `self.idx += 1; return Ok(Advance::Event);` sites)
- Modify: `crates/retrace/src/debug.rs` (extend the two stop-gap arms)
- Modify: `crates/retrace/tests/watch.rs` (new tests)

**Interfaces:**
- Consumes: T3's `watch_ranges` bookkeeping (already maintained by `arm_hw_watchpoint`/`clear_hw_watchpoints`).
- Produces (relied on by T5/T6):
  - `Box_::take_syscall_watch_hit(&mut self) -> Option<(u64, u64)>` — `(watched_va, write_ipa)` of the first overlap this event, cleared by the take.
  - `Advance::WatchSyscall { watched: u64 }` — returned INSTEAD of `Advance::Event` when the just-consumed event's recorded writes overlapped a watched range. The event is fully consumed first: state/idx advance identically, only the report differs.

- [ ] **Step 1: Write the failing tests** (append to `crates/retrace/tests/watch.rs`)

```rust
/// The read()'s buffer VA and the landmark index AFTER consuming the read event, from the trace.
fn discover_read(trace: &Path) -> (usize, u64) {
    let mut s = ReplaySession::open(trace).unwrap();
    loop {
        if let Some((3, args)) = s.peek_syscall() {
            s.advance().unwrap();
            return (s.landmark(), args[1]);
        }
        s.advance().unwrap();
    }
}

/// fstat()'s statbuf VA and the landmark index AFTER consuming the fstat event.
fn discover_fstat(trace: &Path) -> (usize, u64) {
    let mut s = ReplaySession::open(trace).unwrap();
    loop {
        if let Some((189, args)) = s.peek_syscall() {
            s.advance().unwrap();
            return (s.landmark(), args[1]);
        }
        s.advance().unwrap();
    }
}

#[test]
fn syscall_write_to_watched_buf_is_reported_and_replay_completes() {
    let (rec, trace) = util::record(retrace_guest::FILEIO);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let (after_read, buf) = discover_read(tp);
    let mut s = ReplaySession::open(tp).unwrap();
    s.arm_watchpoints(&[(buf, 8)]);
    // open consumes as Event (no writes hit); fstat writes statbuf only; read MUST report.
    let hit_at = loop {
        match s.advance().unwrap() {
            Advance::WatchSyscall { watched } => { assert_eq!(watched, buf); break s.landmark(); }
            Advance::Event => continue,
            _ => panic!("unexpected advance kind before the read"),
        }
    };
    assert_eq!(hit_at, after_read, "hit must be the read event's boundary");
    // Detection observed, never interfered: the rest of the replay completes byte-perfectly.
    loop {
        match s.advance().unwrap() {
            Advance::Event => continue,
            Advance::Exited(report) => {
                assert_eq!(report.exit_code, 0);
                assert_eq!(report.stdout, b"retrace-m1-fixture\n".to_vec());
                break;
            }
            _ => panic!("no further watch hits expected"),
        }
    }
}

#[test]
fn fstat_statbuf_write_is_detected() {
    let (rec, trace) = util::record(retrace_guest::FILEIO);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let (after_fstat, statbuf) = discover_fstat(tp);
    let mut s = ReplaySession::open(tp).unwrap();
    s.arm_watchpoints(&[(statbuf, 8)]);
    loop {
        match s.advance().unwrap() {
            Advance::WatchSyscall { watched } => {
                assert_eq!(watched, statbuf);
                assert_eq!(s.landmark(), after_fstat);
                break;
            }
            Advance::Event => continue,
            _ => panic!("expected the fstat WatchSyscall first"),
        }
    }
}
```

- [ ] **Step 2: Run to verify RED**

Run: `cargo test -p retrace --test watch -- --test-threads=1`
Expected: COMPILE FAIL — no `Advance::WatchSyscall`.

- [ ] **Step 3: retrace-box** — add the field to `Box_` (next to `watch_ranges`; same NOT-in-BoxState comment applies):

```rust
    syscall_watch_hit: Option<(u64, u64)>, // (watched_va, write_ipa): first overlap this event
```

(add `syscall_watch_hit: None,` to every constructor the compiler flags). Also clear it in `clear_hw_watchpoints` (`self.syscall_watch_hit = None;` next to `watch_ranges.clear()`).

Hook `apply_and_return` (`:1629`) — insert the detection BEFORE the copy, never altering it:

```rust
    pub fn apply_and_return(&mut self, ret: u64, err: bool, writes: &[Region]) {
        for w in writes {
            // M5: watched-range intersection (observation only — the copy below is unconditional).
            // Empty watch_ranges on record and plain replay => this is a no-op is_empty check there.
            if self.syscall_watch_hit.is_none() && !self.watch_ranges.is_empty() {
                let end = w.ipa + w.bytes.len() as u64;
                if let Some(&(va, len)) = self.watch_ranges.iter()
                    .find(|&&(va, len)| w.ipa < va + len && va < end)
                {
                    let _ = len;
                    self.syscall_watch_hit = Some((va, w.ipa));
                }
            }
            let (hp, avail) = self.host_span(w.ipa)
                .unwrap_or_else(|| panic!("apply_and_return: write ipa {:#x} outside any mapped region", w.ipa));
            assert!(w.bytes.len() <= avail,
                "apply_and_return: write at {:#x} ({} bytes) overruns backing ({} avail)", w.ipa, w.bytes.len(), avail);
            unsafe { std::ptr::copy_nonoverlapping(w.bytes.as_ptr(), hp, w.bytes.len()); }
        }
        self.set_x0_err_and_return(ret, err);
    }

    /// Take (and clear) the syscall-write watch hit recorded by `apply_and_return` this event.
    pub fn take_syscall_watch_hit(&mut self) -> Option<(u64, u64)> { self.syscall_watch_hit.take() }
```

- [ ] **Step 4: retrace-core** — extend the enum:

```rust
pub enum Advance { Event, Exited(ReplayReport), Break, Watch, WatchSyscall { watched: u64 } }
```

Add the helper (private, near the top of `impl ReplaySession`):

```rust
    /// Finish consuming one trace event: bump idx and report it — as `WatchSyscall` if this event's
    /// applied writes overlapped an armed watch range (the event is consumed identically either
    /// way; only the report differs), else as plain `Event`.
    fn finish_event(&mut self) -> Result<Advance, Divergence> {
        self.idx += 1;
        if let Some((watched, _ipa)) = self.b.take_syscall_watch_hit() {
            return Ok(Advance::WatchSyscall { watched });
        }
        Ok(Advance::Event)
    }
```

Then mechanically replace EVERY `self.idx += 1; return Ok(Advance::Event);` pair inside `advance()`'s syscall branch with `return self.finish_event();` — there are ~11 (mach_msg2 shared tail `:550-551`, mmap-anon `:562-563`, mmap-file `:579-580`, mach_vm_allocate/map `:606-607`, deallocate `:612-613`, protect `:617-618`, shared_region_check `:626-627`, map_and_slide `:635-636`, munmap `:643-644`, mprotect `:648-650`, generic `:654-655`). Branches that never write guest memory route through the same helper harmlessly (`take` returns None). Verify none remain:

Run: `grep -n "idx += 1" crates/retrace-core/src/lib.rs` — expected: only `finish_event`'s own line (plus any occurrence OUTSIDE `advance()`, e.g. none known).

Note: `advance_to_landmark` (`:687-692`) matches only `Advance::Exited` via `if let` — a `WatchSyscall` during an unarmed seek is impossible (empty watch set), and during T5/T6 scans, seeks always run on fresh unarmed sessions. No change needed there.

- [ ] **Step 5: debug.rs stop-gaps** — extend both T3 arms to also cover the new variant:

```rust
                Advance::Watch | Advance::WatchSyscall { .. } =>
                    return Err("internal: watchpoint hit but none armed".into()),
```

- [ ] **Step 6: Run to verify GREEN**

Run: `cargo test -p retrace --test watch -- --test-threads=1`
Expected: 5 passed.

Regression (record-side + oracle untouched): `cargo test -p retrace --test e2e --test seeded_swarm -- --test-threads=1` and `cargo test -p retrace --test hello_dyn_e2e -- --test-threads=1` — all pass.

- [ ] **Step 7: Clippy + commit**

```bash
cargo clippy --workspace -- -D warnings
git add crates/retrace-box/src/lib.rs crates/retrace-core/src/lib.rs crates/retrace/src/debug.rs crates/retrace/tests/watch.rs
git commit -m "M5 t4: syscall-write watch detection — apply_and_return hook + Advance::WatchSyscall"
```

Gate arithmetic: 110 + 2 = **112 / 0 / 0** expected at `just gate`.

---

### Task 5: CLI — `watch`/`unwatch`, forward `continue` hits, progress rule

**Files:**
- Modify: `crates/retrace/src/debug.rs`
- Create: `crates/retrace/tests/watch_cli.rs`

**Interfaces:**
- Consumes: `ReplaySession::{arm_watchpoints, clear_watchpoints, far}`, `Advance::{Watch, WatchSyscall}`.
- Produces (relied on by T6): `Cmd::Watch(u64, u64)` / `Cmd::Unwatch(u64)`; `Exec.watches: Vec<(u64, u64)>` (sorted by addr, deduped, ≤ 4); `Exec.last_watch_hit: Option<(usize, u64)>`; `fn watched_of(ws: &[(u64, u64)], far: u64) -> u64`; transcript grammar:
  - echo: `watch at {addr:#x} len {len}` / `unwatched {addr:#x}`
  - hw hit: `hit watch {watched:#x} (write at {pc:#x}) at ({n}, +?)` then `resolved ({n}, {k})`
  - syscall hit: `hit watch {watched:#x} (syscall write) at ({n}, 0)`
  - cap error: `cannot arm more than 4 watchpoints (hardware limit: DBGWVR0-3)`
  - parse errors: `watch len must be 1, 2, 4, or 8; got {len}` / `watch address {addr:#x} must be {len}-byte aligned`

- [ ] **Step 1: Write the failing unit tests** (append inside `debug.rs`'s `mod tests`)

```rust
    #[test] fn parses_watch_and_unwatch() {
        assert_eq!(parse_script("watch 0x1000").unwrap(), vec![Cmd::Watch(0x1000, 8)]);
        assert_eq!(parse_script("watch 0x1004 4; unwatch 0x1004").unwrap(),
                   vec![Cmd::Watch(0x1004, 4), Cmd::Unwatch(0x1004)]);
    }
    #[test] fn rejects_bad_watch_len_and_alignment() {
        assert!(parse_script("watch 0x1000 3").unwrap_err().contains("must be 1, 2, 4, or 8"));
        assert!(parse_script("watch 0x1001 8").unwrap_err().contains("8-byte aligned"));
        assert!(parse_script("watch").is_err());
        assert!(parse_script("watch 0x1000 8 extra").is_err());
    }
```

- [ ] **Step 2: Write the failing CLI tests** — create `crates/retrace/tests/watch_cli.rs`

```rust
// Golden-transcript tests for M5 watchpoints. NEW file: the pre-M5 transcripts in debug_cli.rs
// are a regression oracle and must stay byte-identical. Every coordinate here is DISCOVERED:
// `target` from the recorded write(1, target, 8) args; the store coordinates by an independent
// memory-scan oracle (step + read_mem), so the watchpoint machinery is checked against ground
// truth it cannot influence.
mod util;
use std::path::Path;

fn debug_run(trace: &str, script: &str) -> (i32, String, String) {
    let out = std::process::Command::new(util::bin())
        .args(["debug", trace, "--script", script])
        .output().expect("spawn debug");
    (out.status.code().unwrap_or(-1),
     String::from_utf8(out.stdout).unwrap(),
     String::from_utf8(out.stderr).unwrap())
}

fn discover_target(trace: &Path) -> u64 {
    let mut s = retrace_core::ReplaySession::open(trace).unwrap();
    loop {
        if let Some((4, args)) = s.peek_syscall() {
            if args[0] == 1 { return args[1]; }
        }
        s.advance().unwrap();
    }
}

/// Ground-truth store coordinates in window 1: step one instruction at a time from (1,0) and
/// record every K whose instruction changed `target`'s qword. Independent of the watch machinery.
fn discover_store_ks(trace: &Path, target: u64) -> Vec<u64> {
    let mut s = retrace_core::seek(trace, 1, 0).unwrap();
    let mut ks = Vec::new();
    let mut prev = s.read_mem(target, 8).unwrap();
    let mut k = 0u64;
    while s.step_insns(1).is_ok() {
        let cur = s.read_mem(target, 8).unwrap();
        if cur != prev { ks.push(k); prev = cur; }
        k += 1;
    }
    ks
}

#[test]
fn watch_continue_hits_first_store_and_progress_rule_advances() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let t = discover_target(tp);
    let ks = discover_store_ks(tp, t);
    assert!(ks.len() >= 2, "watchloop must store at least twice, got {ks:?}");
    let spc = { let s = retrace_core::seek(tp, 1, ks[0]).unwrap(); s.pc() }; // the (single) store pc

    let (code, out, err) = debug_run(ts, &format!("watch 0x{t:x}; continue; where; continue; where"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("watch at 0x{t:x} len 8")), "watch echo:\n{out}");
    assert!(out.contains(&format!("hit watch 0x{t:x} (write at 0x{spc:x}) at (1, +?)")), "hit line:\n{out}");
    assert!(out.contains(&format!("resolved (1, {})", ks[0])), "first store K:\n{out}");
    assert!(out.contains(&format!("at (1, {}) pc=0x{spc:x}", ks[0])), "where after first hit:\n{out}");
    // Progress rule: the second continue pre-steps off the un-retired store and lands on the NEXT
    // execution of the same store pc — ks[1], not ks[0] again.
    assert!(out.contains(&format!("resolved (1, {})", ks[1])), "second hit advances:\n{out}");
    assert!(out.trim_end().ends_with(&format!("at (1, {}) pc=0x{spc:x}", ks[1])), "final where:\n{out}");
}

#[test]
fn watch_validation_is_fail_loud() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0);
    let ts = trace.to_str().unwrap();
    // Parse-time errors: exit 5, no stdout at all.
    let (c1, o1, e1) = debug_run(ts, "watch 0x1001 8");
    assert_eq!(c1, 5); assert!(o1.is_empty());
    assert!(e1.contains("watch address 0x1001 must be 8-byte aligned"), "stderr: {e1}");
    let (c2, _, e2) = debug_run(ts, "watch 0x1000 3");
    assert_eq!(c2, 5);
    assert!(e2.contains("watch len must be 1, 2, 4, or 8; got 3"), "stderr: {e2}");
    // Exec-time cap: the 5th watch errors naming the hardware limit.
    let script = (0..5).map(|i| format!("watch 0x{:x}", 0x10000u64 + i * 8))
        .collect::<Vec<_>>().join("; ");
    let (c3, _, e3) = debug_run(ts, &script);
    assert_eq!(c3, 5, "5th watch must be a loud error");
    assert!(e3.contains("cannot arm more than 4 watchpoints (hardware limit: DBGWVR0-3)"), "stderr: {e3}");
}

#[test]
fn unwatch_disarms() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let t = discover_target(tp);
    let (code, out, err) = debug_run(ts, &format!("watch 0x{t:x}; unwatch 0x{t:x}; continue"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("unwatched 0x{t:x}")), "unwatch echo:\n{out}");
    assert!(out.contains("exited (code 0)"), "runs to exit:\n{out}");
    assert!(!out.contains("hit watch"), "no hit after unwatch:\n{out}");
}
```

- [ ] **Step 3: Run to verify RED**

Run: `cargo test -p retrace --test watch_cli -- --test-threads=1` and `cargo test -p retrace debug:: -- --test-threads=1`
Expected: COMPILE FAIL on `Cmd::Watch` in the unit tests; `watch_cli` fails with `unknown command: watch` transcripts (exit 5).

- [ ] **Step 4: Implement in `debug.rs`**

`Cmd` enum: add `Watch(u64, u64), Unwatch(u64)`. `parse_one` match, after the `"delete"` arm:

```rust
        "watch"           => {
            if ops.is_empty() || ops.len() > 2 {
                return Err(format!("`watch` takes <addr> [len]; got {} operand(s)", ops.len()));
            }
            let addr = parse_addr(ops[0])?;
            let len = match ops.get(1) {
                None => 8u64,
                Some(t) => t.parse::<u64>().map_err(|_| format!("bad watch len: {t}"))?,
            };
            if !matches!(len, 1 | 2 | 4 | 8) {
                return Err(format!("watch len must be 1, 2, 4, or 8; got {len}"));
            }
            if addr % len != 0 {
                return Err(format!("watch address {addr:#x} must be {len}-byte aligned"));
            }
            Ok(Cmd::Watch(addr, len))
        }
        "unwatch"         => { one_operand(verb, &ops)?; Ok(Cmd::Unwatch(parse_addr(ops[0])?)) }
```

`Exec` struct: add `watches: Vec<(u64, u64)>` and `last_watch_hit: Option<(usize, u64)>` (both init empty/None in `Exec::new`). Doc-comment on `watches`: sorted by addr + deduped (≤ 4: one per DBGWVR slot). `exec()` dispatch: add the two arms:

```rust
            Cmd::Watch(a, l)      => self.cmd_watch(*a, *l, out),
            Cmd::Unwatch(a)       => self.cmd_unwatch(*a, out),
```

Module-level helper (near `resolve_hit_k`):

```rust
/// The armed watch range containing `far` (exact byte), else the range overlapping `far`'s aligned
/// doubleword (FAR may report the comparator base — spike F4b), else `far` itself (honest fallback,
/// never a wrong range). Deterministic: `ws` is sorted, first match wins.
fn watched_of(ws: &[(u64, u64)], far: u64) -> u64 {
    ws.iter().find(|&&(a, l)| far >= a && far < a + l)
        .or_else(|| ws.iter().find(|&&(a, l)| { let d = far & !7; d < a + l && a < d + 8 }))
        .map(|&(a, _)| a)
        .unwrap_or(far)
}
```

Commands:

```rust
    fn cmd_watch<W: Write>(&mut self, addr: u64, len: u64, out: &mut W) -> Result<(), String> {
        if let Err(i) = self.watches.binary_search_by_key(&addr, |&(a, _)| a) {
            if self.watches.len() >= 4 {
                return Err("cannot arm more than 4 watchpoints (hardware limit: DBGWVR0-3)".into());
            }
            self.watches.insert(i, (addr, len));
        }
        line(out, format_args!("watch at {addr:#x} len {len}"))
    }

    fn cmd_unwatch<W: Write>(&mut self, addr: u64, out: &mut W) -> Result<(), String> {
        if let Ok(i) = self.watches.binary_search_by_key(&addr, |&(a, _)| a) {
            self.watches.remove(i);
        }
        line(out, format_args!("unwatched {addr:#x}"))
    }
```

`cmd_continue` changes (three edits):

1. **Progress rule** — widen the pre-step condition (`:277`). A hardware watch hit parks AT the un-retired store, so continuing from exactly the last reported hit must pre-step one instruction (unarmed) or it re-fires at zero progress. It fires ONLY when parked on the remembered hit — a store the user manually stepped up to still hits:

```rust
        if self.last_watch_hit == Some((self.n, self.k)) || self.breakpoints.contains(&self.sess().pc()) {
```

2. **Arm both sets** — after the `arm_breakpoints` call (`:299`):

```rust
        let ws = self.watches.clone();
        self.sess_mut().arm_watchpoints(&ws);
```

3. **New match arms** in the scan loop (before or after `Advance::Break`; keep `Break`'s `kctx + 1` untouched):

```rust
                Advance::Watch => {
                    let n = self.sess().landmark();
                    let p_hit = self.sess().pc();
                    let watched = watched_of(&ws, self.sess().far());
                    line(out, format_args!("hit watch {watched:#x} (write at {p_hit:#x}) at ({n}, +?)"))?;
                    // Resolve from kctx, NOT kctx+1: unlike a breakpoint (whose parked-on case the
                    // pre-step already moved off), a watched store CAN legitimately fire at the
                    // exact parked coordinate (the user stepi'd up to it), and the store pc repeats
                    // in loops — searching from kctx+1 would misresolve to the NEXT iteration.
                    let kctx = if n == start_n { start_k } else { 0 };
                    self.session = None; // free the VM before the resolution seek
                    let k = resolve_hit_k(self.trace, &mut self.cache, n, p_hit, kctx)?;
                    line(out, format_args!("resolved ({n}, {k})"))?;
                    self.last_watch_hit = Some((n, k));
                    return self.reseek(n, k);
                }
                Advance::WatchSyscall { watched } => {
                    let n = self.sess().landmark();
                    line(out, format_args!("hit watch {watched:#x} (syscall write) at ({n}, 0)"))?;
                    self.sess_mut().clear_breakpoints();  // keep this session, hit-clean
                    self.sess_mut().clear_watchpoints();
                    self.n = n;
                    self.k = 0;
                    return Ok(());
                }
```

(Delete the T3/T4 stop-gap arm from `cmd_continue`; `cmd_reverse_continue`'s stop-gap stays until T6.) Also note: the pre-step's window-crossing `advance()` (`:283`) runs on an unarmed session — a `Watch`/`WatchSyscall` there is impossible; extend ITS match's `_ =>` arm only if the compiler demands (it already has a catch-all `_`).

- [ ] **Step 5: Run to verify GREEN**

Run: `cargo test -p retrace --test watch_cli -- --test-threads=1` — 3 passed.
Run: `cargo test -p retrace debug:: -- --test-threads=1` — unit tests pass (old + 2 new).
Regression: `cargo test -p retrace --test debug_cli --test reverse_debug_e2e --test checkpoint_seek -- --test-threads=1` — byte-identical transcripts still pass.

- [ ] **Step 6: Clippy + commit**

```bash
cargo clippy --workspace -- -D warnings
git add crates/retrace/src/debug.rs crates/retrace/tests/watch_cli.rs
git commit -m "M5 t5: watch/unwatch commands — forward continue hits + progress rule"
```

Gate arithmetic: 112 + 5 (2 unit + 3 transcript) = **117 / 0 / 0** expected at `just gate`.

---

### Task 6: `reverse-continue` integration + FILEIO syscall-write transcripts

**Files:**
- Modify: `crates/retrace/src/debug.rs` (`cmd_reverse_continue`)
- Modify: `crates/retrace/tests/watch_cli.rs` (new tests)

**Interfaces:**
- Consumes: everything from T5 (`watched_of`, transcript grammar, `last_watch_hit`).
- Produces: `reverse-continue` reports the latest hit of ANY kind strictly before P, with the same hit-line grammar as `cmd_continue`'s resolved forms:
  - `hit {pc:#x} at ({n}, {k})` (breakpoint — unchanged)
  - `hit watch {watched:#x} (write at {pc:#x}) at ({n}, {k})` (hardware)
  - `hit watch {watched:#x} (syscall write) at ({n}, 0)` (syscall)
  - `no earlier hit` (unchanged)

- [ ] **Step 1: Write the failing tests** (append to `crates/retrace/tests/watch_cli.rs`)

```rust
#[test]
fn reverse_continue_finds_last_store() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let t = discover_target(tp);
    let ks = discover_store_ks(tp, t);
    let k_last = *ks.last().unwrap();
    let spc = { let s = retrace_core::seek(tp, 1, ks[0]).unwrap(); s.pc() };
    // Park just past the last store via stepi (watches are never armed during stepping), then ask
    // for the most recent writer: it must be the LAST store, not the first.
    let (code, out, err) = debug_run(ts,
        &format!("stepi {}; watch 0x{t:x}; reverse-continue; where", k_last + 1));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("hit watch 0x{t:x} (write at 0x{spc:x}) at (1, {k_last})")),
        "last-writer hit:\n{out}");
    assert!(out.trim_end().ends_with(&format!("at (1, {k_last}) pc=0x{spc:x}")), "final where:\n{out}");
}

#[test]
fn reverse_continue_with_no_earlier_write_reports_none() {
    let (rec, trace) = util::record(retrace_guest::WATCHLOOP);
    assert_eq!(rec.code, 0);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let t = discover_target(tp);
    // At (1, 0) nothing has written target yet.
    let (code, out, err) = debug_run(ts, &format!("watch 0x{t:x}; reverse-continue; where"));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains("no earlier hit"), "no writer before (1,0):\n{out}");
    assert!(out.contains("at (1, 0)"), "position unchanged:\n{out}");
}

/// The read()'s buffer VA, the boundary landmark AFTER it, and that boundary's pc.
fn discover_read_cli(trace: &Path) -> (usize, u64, u64) {
    let mut s = retrace_core::ReplaySession::open(trace).unwrap();
    loop {
        if let Some((3, args)) = s.peek_syscall() {
            s.advance().unwrap();
            return (s.landmark(), args[1], s.position());
        }
        s.advance().unwrap();
    }
}

#[test]
fn syscall_writer_is_found_forward_and_backward() {
    let (rec, trace) = util::record(retrace_guest::FILEIO);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let tp = Path::new(&trace);
    let ts = trace.to_str().unwrap();
    let (after_read, buf, bpc) = discover_read_cli(tp);
    let hit_line = format!("hit watch 0x{buf:x} (syscall write) at ({after_read}, 0)");
    let (code, out, err) = debug_run(ts,
        &format!("watch 0x{buf:x}; continue; where; stepi 2; reverse-continue; where"));
    assert_eq!(code, 0, "stderr: {err}");
    // Forward: continue stops at the read's boundary. Backward from (after_read, 2): the same
    // syscall hit at (after_read, 0) is strictly earlier — found again.
    assert_eq!(out.matches(&hit_line).count(), 2, "forward + reverse hits:\n{out}");
    assert!(out.contains(&format!("at ({after_read}, 0) pc=0x{bpc:x}")), "parked at boundary:\n{out}");
}
```

- [ ] **Step 2: Run to verify RED**

Run: `cargo test -p retrace --test watch_cli -- --test-threads=1`
Expected: the three new tests FAIL — `reverse_continue_finds_last_store` errs with the T3 stop-gap (`internal: watchpoint hit but none armed` — reverse-continue never arms watches yet, so actually it reports `no earlier hit`; either way the required hit line is absent), `syscall_writer_…` similarly lacks the second hit line.

- [ ] **Step 3: Rewrite `cmd_reverse_continue`** (full replacement of the function body; same scan structure, hits generalized):

```rust
    /// Run backward to the latest hit — breakpoint, hardware watch, or syscall watch — strictly
    /// before the current position P. Scans forward from the start (the only direction replay
    /// runs), recording each hit's coordinate and stepping the cursor past it, until a hit at/after
    /// P or exit. A hardware watch hit resolves K from the hit pc (searching from the scan cursor,
    /// NOT cursor+1 — a store can fire at the cursor's own coordinate); a syscall hit's coordinate
    /// is the post-event boundary (n, 0) and its cursor resumes AT (n, 0): the writing event is
    /// already consumed by the (unarmed) seek, so it cannot re-fire, but a first-instruction store
    /// in window n can still be caught.
    fn cmd_reverse_continue<W: Write>(&mut self, out: &mut W) -> Result<(), String> {
        enum RHit { Bp(u64), Watch { watched: u64, pc: u64 }, WatchSys { watched: u64 } }
        let (pn, pk) = (self.n, self.k);
        let bps = self.breakpoints.clone();
        let ws = self.watches.clone();
        self.session = None; // the scan uses its own transient sessions
        let mut last: Option<(usize, u64, RHit)> = None; // (n, k, kind) of the latest hit < P
        let (mut cur_n, mut cur_k) = (1usize, 0u64);     // scan cursor
        loop {
            let mut s = checkpointed_seek(self.trace, &mut self.cache, cur_n, cur_k)?;
            s.arm_breakpoints(&bps);
            s.arm_watchpoints(&ws);
            let hit = loop {
                match s.advance().map_err(|d| format!("reverse-continue diverged: {}", d.detail))? {
                    Advance::Break => break Some((s.landmark(), RHit::Bp(s.pc()))),
                    Advance::Watch => {
                        let watched = watched_of(&ws, s.far());
                        break Some((s.landmark(), RHit::Watch { watched, pc: s.pc() }));
                    }
                    Advance::WatchSyscall { watched } =>
                        break Some((s.landmark(), RHit::WatchSys { watched })),
                    Advance::Event => continue,
                    Advance::Exited(_) => break None,
                }
            };
            drop(s); // free the VM before resolving K
            let (n, rh) = match hit { Some(h) => h, None => break };
            let (k, resume) = match &rh {
                RHit::Bp(pc) | RHit::Watch { pc, .. } => {
                    let from_k = if n == cur_n { cur_k } else { 0 };
                    let k = resolve_hit_k(self.trace, &mut self.cache, n, *pc, from_k)?;
                    (k, (n, k + 1)) // resume strictly past a resolved instruction hit
                }
                RHit::WatchSys { .. } => (0u64, (n, 0u64)),
            };
            if (n, k) < (pn, pk) {
                last = Some((n, k, rh));
                (cur_n, cur_k) = resume;
            } else {
                break; // reached P; earlier hits are already recorded
            }
        }
        match last {
            Some((n, k, RHit::Bp(pc))) => {
                line(out, format_args!("hit {pc:#x} at ({n}, {k})"))?;
                self.reseek(n, k)
            }
            Some((n, k, RHit::Watch { watched, pc })) => {
                line(out, format_args!("hit watch {watched:#x} (write at {pc:#x}) at ({n}, {k})"))?;
                self.last_watch_hit = Some((n, k));
                self.reseek(n, k)
            }
            Some((n, _, RHit::WatchSys { watched })) => {
                line(out, format_args!("hit watch {watched:#x} (syscall write) at ({n}, 0)"))?;
                self.reseek(n, 0)
            }
            None => { line(out, format_args!("no earlier hit"))?; self.reseek(pn, pk) }
        }
    }
```

(This removes the last stop-gap arm. Note reverse-continue needs NO pre-step: the strict `(n, k) < (pn, pk)` comparison already excludes P itself, so a hit parked at P is never re-reported — the spec's progress rule is a live concern only for forward `continue`.)

- [ ] **Step 4: Run to verify GREEN**

Run: `cargo test -p retrace --test watch_cli -- --test-threads=1` — 6 passed.
Regression: `cargo test -p retrace --test debug_cli --test reverse_debug_e2e --test checkpoint_seek --test watch -- --test-threads=1` — all pass.

- [ ] **Step 5: Clippy + commit**

```bash
cargo clippy --workspace -- -D warnings
git add crates/retrace/src/debug.rs crates/retrace/tests/watch_cli.rs
git commit -m "M5 t6: reverse-continue-to-last-writer — watch hits in the backward scan"
```

Gate arithmetic: 117 + 3 = **120 / 0 / 0** expected at `just gate`.

---

### Task 7: README M5 Status section + full-gate close

**Files:**
- Modify: `README.md` (append a new Status section after the M4 one at `:851`)

**Interfaces:**
- Consumes: the whole branch. Produces: the honest, fact-checked M5 record.

- [ ] **Step 1: Run the full gate and capture the truth**

Run: `just gate`
Expected: **120 passed / 0 failed / 0 ignored** (or the actual number — record what prints), clippy clean. If anything fails, STOP and fix before writing docs.

- [ ] **Step 2: Append `## Status: M5 — write watchpoints & reverse-continue-to-last-writer ✅`**

Follow the M4 section's shape exactly. Required content, every claim fact-checked against the code (field-by-field — the M4 close caught two fabricated claims in review, do not repeat that):
- The capability: `watch <addr> [len]` / `unwatch`; forward `continue` and `reverse-continue` stop on both guest stores (hardware DBGWVR/DBGWCR, 4 slots, EL0 store-only, BAS byte-select) and recorded kernel writes (software intersection in `apply_and_return`); hit grammar with the pre-retire park-at-the-store semantics and the `kctx`-not-`kctx+1` resolve rationale.
- The spike: `dbgw.c` F4a-F4d verdicts as recorded in `spikes/README.md`.
- The MDE sharing fix (`clear_hw_breakpoints` no longer silently disarms watchpoints) and its regression test.
- The exact new test names (list all from `watch.rs`, `watch_cli.rs`, the two `debug.rs` unit tests, `watchloop_guest_parses`) and the gate arithmetic 106 → final.
- Deferred (from the spec's Scope/Out): `rwatch`/`awatch`, old→new value printing, >8-byte or doubleword-crossing ranges, symbol-based watch, watch hits in plain `retrace replay`.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "M5 t7: README M5 Status — watchpoints, fact-checked against source"
```

---

## After Task 7

Final whole-branch review (per house SDD process), then `superpowers:finishing-a-development-branch` for the merge decision. Update the SDD ledger (`.superpowers/sdd/progress.md`) per task as usual.

## Self-review notes (already applied)

- Spec coverage: every spec Mechanism subsection maps to a task (spike→T1, M5-hw→T3, M5-soft→T4, M5-core→T3/T4, M5-cli→T5/T6); Exit criterion covered by `watch_continue_hits_first_store_…`, `reverse_continue_finds_last_store`, `syscall_writer_is_found_forward_and_backward`; the spec's testing list items 1-7 all appear.
- Deviation from spec, intentional: the progress rule is implemented for forward `continue` only — reverse-continue's strict `< P` comparison makes a pre-step provably unnecessary (noted in T6). The spec sentence naming both commands is satisfied vacuously; carry this note into the T6 report.
- Type consistency: `watches: Vec<(u64, u64)>` / `arm_watchpoints(&[(u64, u64)])` / `arm_hw_watchpoint(slot, va, len)` / `far() -> u64` / `take_syscall_watch_hit() -> Option<(u64, u64)>` used identically across T3-T6.
- The F4c pre-retire assumption is flagged at every dependent step (T3 step 1 test comment, T5 hit semantics, T6 resolve comment) with the spec's fallback named in the primer.
