# Verification spikes

Throwaway probes that empirically verified the load-bearing HVF claims the design rests
on, run on the actual target OS before committing to the architecture.

**Environment:** Apple Silicon, macOS 26.4.1 (build 25E253), Darwin 25.4.0,
`kern.hv_support: 1`, Command Line Tools SDK. Run as non-root (uid 501), **SIP enabled**.

## Build & run

```sh
clang -o hvprobe hvprobe.c -framework Hypervisor -framework Foundation
codesign -s - -f --entitlements ent.plist hvprobe
./hvprobe

clang -o hvspike hvspike.c -framework Hypervisor
codesign -s - -f --entitlements ent.plist hvspike
./hvspike
```

## `hvprobe.c` — platform capability probe

Verified, both directions:

- **Entitlement is freely ad-hoc-signable.** No entitlement → `hv_vm_create` returns
  `HV_DENIED (0xfae94007)`. Ad-hoc signed with `com.apple.security.hypervisor` (no Apple
  account, no provisioning profile, non-root, SIP on) → `HV_SUCCESS`.
- **No instruction counter.** `ID_AA64DFR0_EL1` reports `PMUVer=0x0` (no PMUv3); the SDK
  headers contain zero `HV_SYS_REG_PM*` registers. Only `hv_vcpu_get_exec_time` (a time
  meter) exists. => instruction-exact positioning must be software single-step.
- **HW debug slots:** 6 breakpoints / 4 watchpoints.
- **Stage-2 RWX** of a plain `PROT_READ|PROT_WRITE` `MAP_ANON` buffer (no `MAP_JIT`,
  no `PROT_EXEC`) → `HV_SUCCESS`.
- **Default IPA:** 36 bits (64 GiB). `set_trap_debug_exceptions` → `HV_SUCCESS`.

## `hvspike.c` — the core vCPU loop, in miniature

Runs real guest instructions at EL1 (MMU off), traps out via `HVC`, decodes the syndrome:

```
guest: movz x0,#0x1234 ; add x0,x0,#1 ; hvc #0
=> hv_vcpu_run -> HV_SUCCESS
   exit.reason -> EXCEPTION
   ESR_EL2 EC=0x16 (EC_AA64_HVC)   # guest trap reached the VMM
   guest X0 = 0x1235               # guest executed natively
```

This is the trap-and-forward mechanism `retrace-box` is built on, proven end-to-end.

## `m2spike.c` — MMU-on, PAC, and shared-cache reachability (M2 loader spike)

Runs real EL0 guest code under **guest-built stage-1 page tables** (16 KiB granule,
`T0SZ=28`, start level 2, `MAIR` attr0 = Normal WBWA), three phases:

```
[A mmu-on enia=1] page-unaligned RW=1  block-unaligned RW=1  pac-roundtrip=1
                  signed ptr=0x54470c30_0001c001 (PAC bits ENGAGED)
                  DSC header read in-guest = "dyld_v1 " (matches host)
[B mmu-on enia=0] same, but PAC is identity (signing off is deterministic)
[D mmu-off ctrl ] same code faults: EC=0x24 data abort, alignment DFSC 0x21
=> MMU-on identity map + Normal memory + PAC verified
```

Establishes the M2 load-bearing claims: unaligned access needs MMU-on Normal memory
(phase D is the negative control), PAC keys set via `hv_vcpu_set_sys_reg` sign/auth
correctly in-guest, and the shared cache is readable through guest translation.

**SPTM safety finding (learned the hard way):** an earlier version mapped the cache file
into the guest with a **file-backed** `hv_vm_map`. On macOS 26 this **hard-panics the
machine** — `[SPTM] VIOLATION_ILLEGAL_MAPPING_TYPE`, an unrecoverable kernel reset. **All
guest memory must be anonymous; stage file bytes into anon pages (`pread`/`fread` then
map), never map a file page directly.** This spike and the M2 design now do exactly that.

## `dscprobe.c` — `DYLD_SHARED_REGION=private` host probe

Confirms dyld will map the shared cache itself (via ordinary syscalls the M1 recorder
handles) rather than joining the kernel-managed shared region:

```
$ ./dscprobe                          # &printf INSIDE kernel shared region  (exit 10)
$ DYLD_SHARED_REGION=private ./dscprobe # &printf OUTSIDE — PRIVATE mapping   (exit 20)
```

## `cacheprobe.c` — shared-cache slide/fixup format dump (M2 re-signing spike)

Pure host file parse (no HVF, no cache mmap — safe, and no entitlement/codesign needed). Parses each
subcache's `dyld_cache_header` (including its **`uuid`**, which names the cache build), the
`mapping_and_slide` entries, the slide-info blob, and **decodes real fixup slots by hand**, then
walks every chain:

```sh
clang -O2 -o cacheprobe cacheprobe.c && ./cacheprobe
```

```
offsetof mappingWithSlideOffset=0x138 (expect 0x138) ...
   uuid=157E6D2E-2E5C-39B1-8F2A-8866EE228BED
    slide-info: version=5 page_size=16384 page_starts_count=1543 value_add=0x180000000
    page[1] fileOff=0x4b68000 vmAddr=0x1ec46c000 start=0x22e0
      [ 0] @0x22e0 AUTH raw=0x801dab846c6f1a88 roff=0x06c6f1a88 div=0x6ae1 addrDiv=1 key=DA next=1
           => @slide0: slotVA=0x1ec46e2e0 targetVA=0x1ec6f1a88 key=DA modifier=blend(0x1ec46e2e0,0x6ae1)
    SCANNED 7888 fixup pages: 9487510 slots total (3719584 auth, 39.2%), avg 1202.8 slots/page, max 2048/page
```

Findings (Tahoe/arm64e; the output above re-captured on macOS 26.5.2 / 25F84 against the cache dated
2026-06-25): **every slide region is v5** (13 of them on that build), 16 KiB pages,
`value_add=0x180000000`; auth pointers use **A-family keys only** (IA/DA); every fixup chain is
**self-contained within its page** (0 cross-page chains over ~27M slots walked). Full decode
formulas + a worked example are in `.superpowers/sdd/m2cache-spike-findings.md`.

**Re-deriving `cache_pager.rs`'s worked example.** `crates/retrace-box/tests/cache_pager.rs` pins one
real auth slot (`DATA_SLOT_IPA` / `DATA_TARGET` / `DATA_DIVERSITY`) as a ground truth for the
re-signer, derived here rather than from `cache.rs`'s own walk — that independence is what makes the
assertion an oracle. Those constants are **cache-build-specific**: when the host cache moves, the
test FPAC-faults (`sign stub faulted at EL0: ESR_EL1 EC=Other(28)`). To re-derive, run `cacheprobe`
and pick any `AUTH ... addrDiv=1 key=DA` slot; its `@slide0:` line prints `slotVA`, `targetVA` and
the diversity — exactly the three constants — and the `uuid=` line names the build to document.

## `pacsign.c` — guest signing-oracle proof (M2 re-signing spike)

Proves we can re-sign cache auth pointers with the **guest's fixed PAC keys** by executing `pac*`
inside the VM (not reimplementing QARMA on the host). Sets the `retrace-box` `PAC_KEYS`, enables
PAC for all four families, and:

```
IA/DA/IB/DB signed != raw, aut* round-trips, signatures distinct, modifier load-bearing  => PASS
wrong-modifier autia => guest ESR_EL1 EC=0x1C (FEAT_FPAC auth-failure)  => PASS  (== task-9b wall)
```

Run under a bounded perl process-group timeout (there is no `timeout` binary); MMU off, only
anonymous guest memory mapped:

```sh
clang -O2 -o pacsign pacsign.c -framework Hypervisor
codesign -s - -f --entitlements ent.plist pacsign
perl -e '$p=fork;if(!$p){setpgrp;exec@ARGV or exit 127}$SIG{ALRM}=sub{kill"-KILL",$p;exit 124};alarm 15;wait;exit($?>>8)' ./pacsign
```

Verdict: the **lazy-per-page-map + fixup-walk + guest-oracle-sign** design is **GO** (per-page
batched signing, one vCPU run per demand-faulted DATA page; ≤2048 pointers/page).

## `sstep.c` — single-step + HW breakpoint delivery (M3)

Proves the primitives M3 reverse-execution needs, in retrace's exact shape (guest at **EL0**,
`VBAR_EL1` → a 16-slot trampoline of `hvc #0`, `set_trap_debug_exceptions(true)`). A code page of
`nop ×4; mrs x1,cntvct_el0; nop ×4; hvc` is stepped with `PSTATE.SS`+`MDSCR_EL1.SS`, then a
`DBGBVR0`/`DBGBCR0` breakpoint is armed and the guest is run free:

```sh
clang -O2 -o sstep sstep.c -framework Hypervisor
codesign -s - -f --entitlements ent.plist sstep
perl -e '$p=fork;if(!$p){setpgrp;exec@ARGV or exit 127}$SIG{ALRM}=sub{kill"-KILL",$p;exit 124};alarm 15;wait;exit($?>>8)' ./sstep
```

```
set_trap_debug_exceptions -> HV_SUCCESS
[SS] run=HV_SUCCESS reason=1 EC2=0x32 pc=0x10000004 EL0 | ESR_EL1 EC1=0x00 elr=0x0
   step 1 -> pc=0x10000004 (expect 0x10000004) OK
   ... (steps 2..4 each advance pc by 4, guest stays at EL0) ...
F1: step route = DIRECT-EL2 (ESR_EL2 EC=0x32) (4/4 clean single-steps)
[MRS] run=HV_SUCCESS reason=1 EC2=0x32 pc=0x10004400 EL1 | ESR_EL1 EC1=0x18 elr=0x10000010
   MRS stepped -> k=6 ec1=0x18 elr=0x10000010 TRAPPED at EL1 (needs emulation)
[F2] run=HV_SUCCESS reason=1 EC2=0x32 pc=0x10000018 EL0 | ESR_EL1 EC1=0x18 elr=0x10000010
F2: re-arm across skipped insn -> OK (pc=0x10000018 expect 0x10000018, one clean step)
[BVR] run=HV_SUCCESS reason=1 EC2=0x30 pc=0x10000020 EL0 | ESR_EL1 EC1=0x18 elr=0x10000010
F3: HW breakpoint = DELIVERED, DIRECT-EL2 (ESR_EL2 EC=0x30, pc=DBGBVR0, insn not yet retired)

=> F1=PASS  F2=PASS  F3=DELIVERED
```

- **(F1) A software single-step delivers DIRECTLY to the VMM as an `hv_vcpu_run` exit** — reason
  `EXCEPTION`, `ESR_EL2` EC=**0x32** (SS from a lower EL), the guest still at EL0 with `PC` advanced
  by one instruction. It does **not** vector through the guest's EL1 `VBAR` trampoline. So
  `Box_::step()` reads the step off the same exit path the box already uses, discriminating on
  `EC2 ∈ {0x32,0x33}`; one `hv_vcpu_run` retires exactly one guest instruction. Arm with
  `MDSCR_EL1.SS=1` **and** `PSTATE.SS=1`, both re-set before every step.
- **(F2) SS survives a trapped-then-manually-skipped instruction** (retrace's below-the-trace
  emulation). When the stepped EL0 instruction itself traps to EL1 (here the `cntvct_el0` MRS,
  `ESR_EL1` EC=0x18), it does **not** retire: it surfaces as a **direct-EL2 step exit** (`EC2=0x32`)
  with the guest now at **EL1** (`PC` in the trampoline, `ELR_EL1` = the faulting-insn address,
  `ESR_EL1` = the real trap syndrome). `Box_::step()` must detect this by the guest EL (EL1, not
  EL0), run its existing emulation off `ESR_EL1`, then re-enter EL0 at `ELR_EL1+4` and re-arm SS —
  the next `hv_vcpu_run` then retires **exactly one** further instruction (`0x…14 → 0x…18`). No lost
  or doubled steps. (Note: while stepping, a below-the-trace instruction surfaces via the **step**
  exit at EL1, *not* the usual trampoline `hvc` exit `EC2=0x16` — the `hvc` never executes.)
- **(F3) HW instruction breakpoints DELIVER, and DIRECTLY to the VMM.** `DBGBVR0_EL1`=target +
  `DBGBCR0_EL1`=`0x1E5` (E=1, PMC=EL0, BAS=0xF) with `MDSCR_EL1.MDE=1` and SS off: running the guest
  free stops with `ESR_EL2` EC=**0x30** (breakpoint from a lower EL), `PC` == `DBGBVR0` and the
  guest at EL0 — the match fires **before** the instruction at that address retires. This is the
  positioning primitive Task 5 needs: place a breakpoint at a target PC, run free, land exactly on
  it. (`ESR_EL1` on the F3 line is stale from F2 — irrelevant to a direct-EL2 breakpoint.)

## `dbgw.c` — write-watchpoint (DBGWVR/DBGWCR) delivery semantics (M5)

Proves the primitives M5 write-watchpoints need, in retrace's exact shape (guest at **EL0**,
`VBAR_EL1` → a 16-slot trampoline of `hvc #0`, `set_trap_debug_exceptions(true)`, MMU off, only
anonymous guest memory). `DBGWVR0_EL1`/`DBGWCR0_EL1` watch an 8-byte qword at `DATA_IPA`
(`BAS=0xFF`, store-only, EL0-only) and the guest runs `mov x1,#DATA_IPA; mov x2,#0x42; str x2,[x1];
hvc`; a second phase re-arms with `BAS=0xF0` (bytes 4..7 only) and runs a `strb` to byte 0:

```sh
clang -O2 -o dbgw dbgw.c -framework Hypervisor
codesign -s - -f --entitlements ent.plist dbgw
perl -e '$p=fork;if(!$p){setpgrp;exec@ARGV or exit 127}$SIG{ALRM}=sub{kill"-KILL",$p;exit 124};alarm 15;wait;exit($?>>8)' ./dbgw
```

```
set_trap_debug_exceptions -> 0
[F4a] reason=1 EC2=0x34 pc=0x1000000c far=0x10008000
F4a: DELIVERED DIRECT-EL2 (EC=0x34/0x35)
F4b: far=0x10008000 vs accessed VA 0x10008000 -> EXACT
F4c: watched mem=0x0 -> PRE-RETIRE (store not yet landed); pc=0x1000000c (AT the str at +0xc)
[resume] reason=1 EC2=0x16 pc=0x10004404 far=0x0
   resume disarmed: terminal hvc, mem=0x42 (expect 0x42)
[F4d] reason=1 EC2=0x16 pc=0x10004404 far=0x0
F4d: strb to byte 0 under BAS=0xF0 -> NO FIRE (BAS is byte-selective) (mem=0x42)
```

All four sub-findings confirm the pre-retire hypothesis the M5 design assumed, exactly:

- **(F4a) A watched EL0 store delivers DIRECTLY to the VMM as an `hv_vcpu_run` exit** — reason
  `EXCEPTION`, `ESR_EL2` EC=**0x34** (Watchpoint from a lower EL), the guest still at EL0 with `PC`
  parked **on** the faulting `str` (`0x1000000c`, offset `+0xc`). It does **not** vector through the
  guest's EL1 `VBAR` trampoline — same direct-EL2 delivery shape as M3's single-step (`EC2=0x32`)
  and HW breakpoint (`EC2=0x30`) exits, so `Box_::run()` can discriminate write-watchpoint exits by
  `EC2 ∈ {0x34,0x35}` on the same exit path already used for those.
- **(F4b) `hv_vcpu_exit`'s `virtual_address` (FAR) holds the exact accessed VA.** `far=0x10008000`
  matches `DATA_IPA` exactly (not page-truncated, not offset) — the watchpoint hit can be attributed
  to the precise address without decoding the trapped instruction's operands.
- **(F4c) The exit is PRE-RETIRE.** At the hit, the watched qword still reads `0x0` (the `str`'s
  value, `0x42`, has not landed) and `PC` sits **at** the `str` itself (`+0xc`), not past it — the
  watchpoint fires *before* the store completes, mirroring F3's HW-breakpoint pre-retire semantics.
  Resuming from the same `PC` with the watchpoint disarmed re-executes the `str` once (not
  double-applied) and reaches the terminal `hvc` with `mem=0x42` as expected — confirming the
  instruction is safely re-executable after disarm, the primitive M5's watchpoint-hit handling
  needs.
- **(F4d) BAS is byte-selective, confirmed both ways.** Arming only bytes 4..7 (`BAS=0xF0`) and
  running a `strb` to byte 0 does **not** fire (`EC2=0x16`, straight through to the terminal `hvc`,
  same as an unwatched run) — and the store *did* execute (`mem=0x42` afterward, from `w2`'s
  leftover `0x42`), proving the non-overlapping byte range was correctly excluded rather than the
  watchpoint being silently disabled.

=> F4a=DELIVERED DIRECT-EL2 (EC=0x34) F4b=EXACT F4c=PRE-RETIRE F4d=NO FIRE (BAS byte-selective) — the
M5 design's pre-retire watchpoint hypothesis holds on this OS/silicon exactly as assumed; no spec
fallback needed.

## `tlbi.c` — guest-side stage-1 TLB invalidation (M9 go/no-go)

`retrace` hand-edits live stage-1 page tables (`set_region_exec`) but the VMM cannot issue a
guest TLBI, so today a data→code flip is only sound on a block the guest has never translated.
jq's dyld breaks that assumption (`MAP_FIXED`-exec-maps `__TEXT` into an already-touched
reservation), so M9 needs to know: can the **guest itself** invalidate its own stage-1 TLB
entries via `tlbi vmalle1` at EL1, and does that actually make a hand-edited leaf visible?

**Deviation from the M9 task-1 brief:** the brief's `TCR_EL1` literal
(`0x8000210080B511`, "T0SZ=17, TG0=16K") configures a 47-bit-VA **three-level** walk, but the
brief's own table-build code constructs only a **two-level** table (start-level `L2 -> L3`) —
contradictory, and would fault on the very first guest access. The TLBI question is independent
of VA size, so this spike keeps the brief's IPA layout, attributes, stub encodings and phase
structure verbatim but borrows the known-working two-level MMU config straight from
`m2spike.c`: `TCR_EL1=0x10080B51C` (T0SZ=28, 36-bit VA, TG0=16K, start level 2).

**A measurement pitfall found and fixed while getting this to run correctly:** all 16 EL1 vector
slots are an unconditional `hvc #0`, so *every* EL0 trap — the deliberate `hvc` trigger
instruction (itself UNDEFINED at EL0) **and** a genuine instruction-abort on a stale/faulting
fetch — funnels through the same vector and re-emerges as the identical `ESR_EL2 EC2=0x16` exit
at the identical `pc`. A first pass that discriminated F2 on `EC2` alone reported "EXECUTED
ANYWAY" — a false positive purely from that ambiguity, not a real finding. Reading `ESR_EL1` (the
real local EL0→EL1 trap cause, latched *before* the vector's own `hvc` fires) resolves it
unambiguously: `EC1=0x00` (Unknown reason — the undefined `hvc`) means the code truly reached the
trigger; `EC1=0x20` (Instruction Abort from a lower EL) means it never got there. `x0` (only the
flipped page's payload sets it to `0x5a`, seeded to a sentinel `0xdead` beforehand) is the second,
independent confirmation. Every subsequent run below uses that corrected instrumentation.

```sh
cd spikes
clang -O2 -o tlbi tlbi.c -framework Hypervisor
codesign -s - -f --entitlements ent.plist tlbi
perl -e '$p=fork;if(!$p){setpgrp;exec@ARGV or exit 127}$SIG{ALRM}=sub{kill"-KILL",$p;exit 124};alarm 15;wait;exit($?>>8)' ./tlbi
```

```
[phase1-read] reason=1 EC2=0x16 pc=0x4404 far=0x0 | ESR_EL1 EC1=0x00 esr1=0x2000000
phase1: read OK (entry now cached as DATA)
[F2-control] reason=1 EC2=0x16 pc=0x4404 far=0x0 | ESR_EL1 EC1=0x20 esr1=0x8200000f
F2: without TLBI -> ESR_EL1 EC1=0x20, x0=0xdead (sentinel 0xdead, payload sets 0x5a) -> FAULTED (stale entry is REAL; the guard's premise holds)
[F1-tlbi] reason=1 EC2=0x16 pc=0x14010 far=0x0 | ESR_EL1 EC1=0x20 esr1=0x8200000f
F1: tlbi vmalle1 at EL1 -> EXECUTED, reached its hvc
[F3-exec] reason=1 EC2=0x16 pc=0x4404 far=0x0 | ESR_EL1 EC1=0x00 esr1=0x2000000
F3: after TLBI, execute flipped page -> SUCCEEDED (x0=0x5a, want 0x5a)

VERDICT: GO — guest-side TLBI works; M9 Tasks 2-3 proceed as designed.
```

- **(F1) `tlbi vmalle1` executes cleanly at guest EL1, no trap to EL2.** Running the stub
  (`tlbi vmalle1; dsb ish; isb; hvc #0`) directly at EL1 lands `pc` exactly on the stub's own
  `hvc` (`STUB_IPA+0xc+4 = 0x14010`), **not** routed through the EL1 vector table — proof that
  none of `tlbi`/`dsb`/`isb` trapped. (`ESR_EL1 EC1=0x20` on this line is stale, carried over
  from the prior F2 run — the stub takes no local EL1 exception of its own, mirroring the
  documented staleness note on `sstep.c`'s F3 line.)
- **(F2, the control) a stale TLB entry is REAL.** Without a TLBI, flipping the leaf DATA→CODE
  and executing from it faults: `ESR_EL1 EC1=0x20` (Instruction Abort from a lower EL) with
  `esr1=0x8200000f` — IFSC `0x0F` = **Permission fault, level 3**, the textbook signature of a
  TLB entry still carrying the old leaf's `UXN=1` after the underlying table entry changed.
  `x0` stays at its `0xdead` sentinel (the payload never ran) confirming it independently. The
  M9 design's guard is **not** over-conservative — the hazard it guards against is real on this
  OS/silicon.
- **(F3) after the TLBI, execution succeeds.** Same flipped leaf, same code path, but preceded
  by the EL1 stub: `ESR_EL1 EC1=0x00` (the deliberate trigger, not a fault) and `x0=0x5a` —
  the payload genuinely ran. The flush made the hand-edited leaf visible.

=> **VERDICT: GO.** Guest-side `tlbi vmalle1` at EL1 is available, untrapped, and sufficient to
invalidate a stale stage-1 entry after a hand-edited data→code leaf flip. M9 Tasks 2–3
(`flush_guest_tlb`) proceed as designed; spec risk R1 (pre-promote reservations) is not needed.

## Proven for M2; still open for later milestones

- MMU-on paging, PAC, and DSC reachability — **proven** (`m2spike.c`); `private` cache
  mapping — **proven** (`dscprobe.c`).
- **Full dyld startup to `main` under the oracle** — the M2 bring-up itself; not a spike.
- **Single-step + HW breakpoint primitives** — **proven** (`sstep.c`); the M3 reverse-execution
  bring-up itself is not a spike. **Signals, threads** — still M3+.
- **Memory-diff fidelity across the real syscall surface** — the long tail; M2 pays the
  four carried-forward recorder debts (clamp, `munmap`/`mprotect`, error ABI, raw `svc`).

## `sigabi.c` / `sigtramp.c` — the signal-frame ABI (M12 delivery spike)

Neither needs the hypervisor entitlement or codesigning — they are plain user programs:

```sh
clang -arch arm64 -o sigabi sigabi.c && ./sigabi
clang -arch arm64 -O0 -o sigtramp sigtramp.c && ./sigtramp
```

`sigabi.c` reports `sizeof`/`offsetof` for every struct in a signal frame, straight from the live
SDK. The load-bearing result: **`sizeof(ucontext_t) == 56` because `uc_mcontext` is a POINTER** —
the 816-byte mcontext lives elsewhere in the frame, so a design assuming one flat struct is wrong
by 816 bytes.

`sigtramp.c` installs its **own** trampoline via the raw `__sigaction` syscall (libc's
`sigaction()` overwrites `sa_tramp` with `_sigtramp`, so it cannot answer this) and faults, to
measure the kernel's entry contract:

```
x0=catcher  x1=0x1e infostyle(UC_FLAVOR, for SA_SIGINFO)  x2=sig  x3=siginfo*  x4=ucontext*
x5=sigreturn token (process-randomized)      sp == x3  => THE FRAME BASE IS sp
```

Frame layout, derived from those pointers — siginfo at `sp+0` (104 B), ucontext at `sp+104`
(56 B), mcontext at `sp+160` (816 B), total 976, with 128 bytes of slack between the frame top
and the pre-signal `sp`. The mcontext carries the real hardware ESR and FAR.

Findings and their consequences are written up in
`docs/superpowers/specs/2026-08-06-retrace-m12-signal-delivery-design.md`.

## `sigraisex0.c` — what a frame delivered at a SYSCALL boundary carries (M12 t7)

`sigtramp.c` above measures a frame built from a **fault**. This one measures the other case: a
signal the process **raises on itself**, where the delivery happens at a syscall boundary and the
frame therefore has a syscall result to account for.

```sh
clang -O0 -o sigraisex0 sigraisex0.c && ./sigraisex0
```

## `protnone.c` — Darwin's `PROT_NONE` signal, measured (M13 R1)

`crates/retrace-arch/src/lib.rs:307` maps an AArch64 permission fault (DFSC `0x0c..=0x0f`) to
`(SIGSEGV, SEGV_ACCERR)` — the Linux answer, and a row **no guest has ever exercised** (M6/M11/M12
only ever recorded translation faults). Darwin's `ux_exception` maps `EXC_BAD_ACCESS` by *code*:
`KERN_INVALID_ADDRESS` → SIGSEGV, everything else (including `KERN_PROTECTION_FAILURE`) → SIGBUS;
libstd's `install_main_guard` comment claims *"This ensures SIGBUS will be raised on stack
overflow."* Needs no entitlement or codesigning — plain user memory, no `hv_*` API:

```sh
clang -o protnone protnone.c
./protnone
```

```
page size = 16384
SEGV_MAPERR=1 SEGV_ACCERR=2 BUS_ADRALN=1 BUS_ADRERR=2 BUS_OBJERR=3
PROT_NONE page at 0x104bd0000
PROT_NONE load         SIGBUS   si_code=1 si_addr=0x104bd0000
PROT_NONE store        SIGBUS   si_code=1 si_addr=0x104bd0000
unmapped load          SIGSEGV  si_code=2 si_addr=0x4000dead0000
unmapped store         SIGSEGV  si_code=2 si_addr=0x4000dead0000
PROT_READ store        SIGBUS   si_code=1 si_addr=0x104bd4000
```

Verified, both directions:

- **A `PROT_NONE` access — load and store alike — raises `SIGBUS` with `si_code=BUS_ADRALN` (1),
  not `SIGSEGV`/`SEGV_ACCERR`.** libstd's comment is right and retrace-arch's table (the Linux
  answer) is wrong for this row: Darwin's `KERN_PROTECTION_FAILURE` → SIGBUS holds for both
  directions, not just the store libstd's guard page cares about.
- **The unmapped control still raises `SIGSEGV`**, both directions — `crashy_e2e`'s M6
  classification is unaffected. Its `si_code=SEGV_ACCERR` (2), not `SEGV_MAPERR`, matching the
  divergence `sigtramp.c` already recorded for a store to a wholly unmapped address (see the doc
  comment above `signal_of_esr`) — this run reconfirms it and extends it to a load.
- **`PROT_READ` store (informational only — M13 honors `prot == 0` exclusively)** also raises
  `SIGBUS`/`BUS_ADRALN` — the same code Darwin uses for `PROT_NONE`. XNU does not distinguish
  "no permission at all" from "wrong permission for this access" in `si_code`; both surface as the
  alignment-named constant despite neither access being misaligned. Not consumed by M13, recorded
  for completeness.

It forces `PSTATE.C = 1` (and `Z = 1`) immediately before `kill(getpid(), SIGUSR1)`, then reports
what the handler's `ucontext` actually holds:

```
kill() returned   = 0
frame saved x0    = 0            <- the syscall's RETURN, not the pid argument that was in x0
frame saved cpsr  = 0x40000000   <- Z survived, C did NOT: the success flags, not the pre-call ones
```

**The kernel snapshots the context *after* completing the syscall return.** So a frame built from
the raw trap state is a frame whose guest, on `sigreturn`, reads its own successful `kill()` as a
failure — `x0` holds the pid and `PSTATE.C` says "error". That is why the caught-raise arm calls
`Box_::complete_syscall_before_delivery` rather than plain `set_x0_err_and_return`: the frame's
PSTATE comes from `SPSR_EL1`, which the latter never writes.

The fault path is deliberately untouched by this — a fault has no syscall result, and its `x0` and
PSTATE *are* the guest's genuine trap state.
