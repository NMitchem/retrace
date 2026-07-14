# retrace M2-bfam — objc B-family PAC (strip-on-FPAC auth emulation)

**Design spec — 2026-07-10.** Sub-milestone of M2 (the loader), sibling of M2-cache, M2-mach, and
M2-va47 — the final loader-completion step that clears the boundary documented in the memory
`retrace-objc-bfamily-pac-resign-wall` and `.superpowers/sdd/task-m2va47-2-report.md`, and (if the
walk reaches `main`) un-ignores the headline M2 gate `hello_dyn_e2e`.

> **⚠ Closeout correction (M2-tbi, 2026-07-14).** The strip-on-FPAC arm this spec designs is **sound
> and unaffected**. But the *next wall* the M2-bfam close-out reported past it — objc's
> `validateAlreadyRealizedClass` fatal, blamed on "objc shared-cache **preoptimization** / a
> preoptimized cache-resident `class_rw_t` / cache-trust" (see the gating-spike report
> `.superpowers/sdd/task-m2bfam-2-report.md` §2–§4) — was a **misdiagnosis**. The verified root cause
> is a one-line guest-MMU bug: the guest `TCR_EL1` left **TBI off**, so a re-signed data-pointer PAC
> occupied **bit 63** and collided with objc's `FAST_IS_RW_POINTER` realized-flag (the fatal class is
> `NSObject`; its `data()` points to `_OBJC_CLASS_RO_$_NSObject`, a `class_ro_t`, not a `class_rw_t`).
> The fix is TCR **TBI0+TBID0**. No objc-opt / cache-trust subsystem is needed. See
> `docs/superpowers/specs/2026-07-14-retrace-m2-tbi-design.md` for the full root cause, evidence, and
> fix. This spec is preserved as-is (history); only this annotation is added.

## What this is

Past the re-signed shared cache (M2-cache), the mach-IPC servicing (M2-mach), and the 47-bit guest
VA (M2-va47, which made objc's isa *strip* lossless), real dyld reaches Objective-C class
realization and faults: `addClassTableEntry+0x70` executes `autdb x16, x17` — a hardware
**authenticate** of the class `data()` pointer with the **DATA-B key** (`APDBKey`, address
diversity, discriminator `0xc93a`) — which FPAC-faults (EC=0x1C). The pointer carries its **host**
DB-key signature (baked in the cache) and the guest uses fixed guest keys, so it can't
authenticate. retrace's M2-cache re-signer is **A-family only** (the v5 slide-info format encodes a
single IA/DA key bit — see Verified facts — so B-family pointers aren't in the re-signing walk at
all).

M2-bfam handles this the way retrace already handles other guest instructions the host can't run
faithfully (timebase MRS, undefined Apple MRS): **intercept the auth-failure exception in the run
loop, emulate a successful authenticate by stripping the pointer to canonical, and skip the
instruction.** No key material, no host key, no objc-structure knowledge — the box trusts the cache
(as it already does) and produces the canonical pointer objc expects. The emulation lives *below*
the record/replay layer (`run()` is shared), so it fires identically on both sides and nothing
enters the trace.

## Verified facts (this codebase / host)

- **v5 slide-info is A-family only** (`crates/retrace-box/src/cache.rs` `decode5`): the auth slot
  packs runtime_offset[33:0], diversity[49:34], addr_div[50], **one** key bit[51] (IA vs DA),
  next[62:52], auth[63] — all 64 bits, no room for a second (A-vs-B) key bit. The DB-signed cache
  pointers are genuinely not re-signable via the existing slide-info walk.
- **The run loop already has the hook pattern** (`crates/retrace-box/src/lib.rs` `Box_::run`): the
  `Ec::Hvc` arm reads `ESR_EL1` and dispatches `Ec::Svc => {}`,
  `Ec::SysReg if try_emulate_timebase => continue`, `Ec::Other(0) if try_emulate_undef_mrs =>
  continue`, `_ => Stop::Other`. `try_emulate_undef_mrs` reads the faulting instruction at
  `ELR_EL1`, edits state, sets `PC = elr + 4`, and returns true — the exact shape M2-bfam mirrors.
- **The 47-bit strip is proven** (M2-va47 `strip47`): AND with `0x0000_7FFF_FFFF_FFFF` recovers the
  canonical VA of a signed low pointer under the current `TCR_EL1.T0SZ=17`. The observed fault was
  a **standalone** `autdb x16, x17` (destination register x16), which strip-and-skip fixes exactly.
- **Determinism is automatic**: `run()` is shared by record and replay; the guest executes the same
  `autdb` and gets stripped identically on both. Same posture as the timebase/undef-MRS emulations.

## Scope

**In:** an FPAC auth-failure emulation arm in `Box_::run` (`try_emulate_fpac_auth`): decode the
faulting **AUT\*** instruction at `ELR_EL1` (register variants `AUTIA/AUTIB/AUTDA/AUTDB` and the
zero-modifier `AUTIZA/AUTIZB/AUTDZA/AUTDZB` forms), strip its destination register with the 47-bit
mask, advance `PC` past it; the gating spike; the empirical walk of any further standalone B-family
faults to `main → write → exit`; un-ignore `hello_dyn_e2e` + a double-replay determinism test.

**Out / the honest edge:** **combined auth-and-use** B-family instructions — `braab`/`blraab`
(authenticate + branch) and `ldraa`/`ldrab` (authenticate + load) — have no destination register to
fix; the auth is implicit in control flow or a memory access. If a B-family *combined* fault appears
in the walk, it needs a per-form handler (strip the target, then perform the branch/load) or is
documented as the next boundary. Also deferred: A-family combined-form faults (they don't occur —
A-family is re-signed and authenticates natively); performance optimization (re-signing in place to
avoid per-auth traps — fine to trap-per-auth for the trivial gate); arm64e guest support; any objc
runtime feature the trivial `write()`-only gate doesn't exercise.

## Exit criterion

`hello_dyn_e2e` un-ignored and green — record prints `hi\n`; replay reproduces stdout
byte-for-byte; per-syscall and final full-memory checks pass — plus a double-replay stability test.
The full existing suite (`just m1`) stays green (the new arm must not perturb the A-family path,
which never FPACs), and `clippy -D warnings` is clean. If honestly blocked past the standalone
B-family auts (a combined-form B-family fault, or a different subsystem), the milestone lands
DONE_WITH_CONCERNS with the new boundary documented and the gate kept `#[ignore]`d.

## The mechanism

### 1. `try_emulate_fpac_auth` (new, mirrors `try_emulate_undef_mrs`)

```rust
fn try_emulate_fpac_auth(&mut self) -> bool {
    let elr = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
    if self.host_span(elr).is_none() { return false; }        // can't read the faulting insn
    let insn = u32::from_le_bytes(self.read_guest(elr, 4).try_into().unwrap());
    let Some(rd) = decode_aut_rd(insn) else { return false; }; // AUT* register/Z variant → Rd
    let signed = self.vcpu.get_reg(reg::x(rd)).unwrap();
    let canonical = signed & 0x0000_7FFF_FFFF_FFFF;            // 47-bit strip (strip47-proven)
    self.vcpu.set_reg(reg::x(rd), canonical).unwrap();
    let spsr = self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap();
    self.vcpu.set_reg(reg::PC, elr + 4).unwrap();              // skip the aut*
    self.vcpu.set_reg(reg::CPSR, spsr).unwrap();
    true
}
```

`decode_aut_rd(insn)`: the AUT\* register variants are `0xDAC1_1000` (AUTIA) / `_1400` (AUTIB) /
`_1800` (AUTDA) / `_1C00` (AUTDB) under mask `0xFFFF_FC00`, with `Rd = insn & 0x1F`; the
zero-modifier `…Z` variants are the `0xDAC1_3xxx` group. Returns `None` for anything else (so an
unrecognized FPAC surfaces loudly as `Stop::Other` rather than being mis-emulated). Exact
encodings pinned in the implementation plan.

### 2. Dispatch arm in `Box_::run`

In the `Ec::Hvc` match, beside the existing emulation arms:

```rust
Ec::Other(0x1C) if self.try_emulate_fpac_auth() => continue,
```

Confirmed: `retrace_arch::ec_of` has a catch-all `other => Ec::Other(other)` (named variants only
for Svc/Hvc/SysReg/SoftStep/Breakpoint/Watchpoint/DataAbort), so EC=0x1C (FPAC) arrives as
`Ec::Other(0x1C)` — no `retrace-arch` change needed, and the arm slots directly beside the existing
`Ec::Other(0) if try_emulate_undef_mrs`. The FPAC syndrome is an EL1-taken synchronous exception
surfaced via the trampoline, exactly like the undef-MRS path.

### 3. Determinism

The strip is a pure function of the faulting register's value, which is deterministic (it comes
from the deterministic re-signed/pristine cache pages). `run()` is shared by record and replay, so
the same `autdb` faults and is stripped identically on both; nothing is written to the trace. This
is the established posture for `try_emulate_timebase` / `try_emulate_undef_mrs`.

## The spike (gating, Task 1)

Wire the arm, build+codesign, run the bounded `hello_dyn` record. Confirm the run advances **past
`addClassTableEntry`** — the `autdb` no longer fatally faults, and objc proceeds into the class it
was realizing. **Go/no-go:** if stripping yields a pointer objc immediately chokes on (a garbage
`class_rw_t` dereference right after the emulated auth), the "trust and strip" premise is wrong —
STOP and reconsider (the pointer may need genuine re-signing, or the strip mask/VA interaction
differs from `strip47`'s). If it proceeds, record the new first-failure and walk.

## Components

- `crates/retrace-box/src/lib.rs` — `try_emulate_fpac_auth` + the `decode_aut_rd` helper + the
  `Ec::Other(0x1C)` dispatch arm. Possibly a diagnostic count of emulated auths under
  `RETRACE_TRACE`.
- `crates/retrace-arch` — unchanged (`ec_of` already surfaces EC=0x1C as `Ec::Other(0x1C)` via its
  catch-all).
- `crates/retrace/tests/hello_dyn_e2e.rs` — remove `#[ignore]`, rewrite the comment, add the
  double-replay test (on success).
- Any per-wall fixes the walk surfaces (mirrored automatically — the emulation is below the trace).
- README + main-spec milestone note; memory update at close.

## Testing

1. **Spike assertion** (Task 1): the dynamic run clears `addClassTableEntry` (the go/no-go gate).
2. **Regression**: `just m1` green — the A-family path never FPACs, so the new arm must be inert for
   every existing test. PAC round-trips (pacguest, sign_oracle), cache, mach all still green.
3. **The gate**: `hello_dyn_e2e` un-ignored + double-replay determinism test (on success).
4. `clippy -D warnings` clean.

## Risk register

1. **"Trust and strip" is wrong for some B-family pointer** (objc expects the pointer to stay signed,
   or the strip yields the wrong canonical value). *Mitigation:* the gating spike observes objc's
   immediate behavior after the first emulated auth; `decode_aut_rd` returns `None` on anything
   unrecognized so a surprising instruction fails loud, not silently mis-emulated.
2. **A combined-form B-family fault** (`braab`/`ldrab`) appears — no `Rd` to fix. *Mitigation:*
   documented honest edge; handle per-form (strip target, then branch/load) if small, else the
   next boundary. Not expected for the trivial gate.
3. **The new arm perturbs the A-family path.** *Mitigation:* the A-family auts are re-signed and
   never FPAC, so the arm is never reached for them; the full regression suite proves inertness.
4. **The FPAC syndrome doesn't reach the arm as expected** (e.g. surfaced as a stage-2 abort rather
   than the EL1-HVC path). *Mitigation:* `ec_of` already routes EC=0x1C to `Ec::Other(0x1C)`;
   Task 1's spike confirms the observed `autdb` fault hits the arm; assert-and-fail loudly on an
   unrecognized syndrome.
5. **Walls past objc B-family** (more objc/libdispatch/xpc init before `main`). *Mitigation:* the
   empirical walk with fail-loud triage (the M2-mach/M2-va47 method); a new distinct boundary is
   documented and deferred, not faked.

## Non-goals / explicitly deferred

Combined-form auth-and-use handling unless the walk demands it; re-signing-in-place performance
optimization; arm64e guest support; objc runtime features beyond the trivial gate; anything past
the first *distinct* non-standalone-B-family boundary.

## Open questions for implementation planning

1. Exact `decode_aut_rd` coverage (register + Z variants confirmed; whether the walk surfaces any
   combined form — on-demand).
2. Whether a standalone micro-test for the strip-on-FPAC arm is worth it, or `hello_dyn_e2e` +
   regression suffice (decide in planning; the fault is hard to synthesize without a host-key
   signature, so the real gate may be the better test).
