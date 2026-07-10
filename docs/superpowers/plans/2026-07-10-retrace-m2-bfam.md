# M2-bfam Implementation Plan — objc B-family PAC (strip-on-FPAC auth emulation)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Emulate objc's B-family pointer authentications of shared-cache pointers by intercepting each FEAT_FPAC auth-failure (EC=0x1C) in the run loop, stripping the `aut*` destination register to canonical, and skipping the instruction — then walk `hello_dyn` to `main → write → exit` and un-ignore `hello_dyn_e2e`.

**Architecture:** objc `autdb`-authenticates cache pointers signed with the host DB key; under the guest's fixed keys this FPAC-faults, and retrace's A-family-only v5 slide-info re-signer can't reach these pointers. Instead of finding/re-signing them, add one run-loop arm (`try_emulate_fpac_auth`) that mirrors the existing `try_emulate_undef_mrs`: decode the faulting `aut*`, strip its destination register with the 47-bit mask (proven by M2-va47's `strip47`), advance PC. It lives below the shared `run()`, so record and replay strip identically and nothing enters the trace.

**Tech Stack:** Rust workspace (`retrace-arch`, `retrace-box`, `retrace-guest`, `retrace`), Hypervisor.framework via `hv-sys`, arm64 asm guests. Spec: `docs/superpowers/specs/2026-07-10-retrace-m2-bfam-design.md` — read it before starting.

## Global Constraints

- Branch: `m2-bfam` (create from `main` before Task 1).
- All test runs: `cargo test --workspace -- --test-threads=1` (one VM per process; parallel VMs flake with `HV_BUSY`). `just m1` is the full gate.
- `cargo clippy --workspace --all-targets -- -D warnings` clean at every commit.
- Any binary calling `hv_*` must be ad-hoc codesigned with `retrace.entitlements`; for a manually-run `target/.../retrace`: `codesign -s - -f --entitlements retrace.entitlements target/aarch64-apple-darwin/debug/retrace`. Manual VM runs bounded: `perl -e 'alarm 60; exec @ARGV' -- <cmd>`.
- No `Date::now()`/randomness in tests (determinism deny-list).
- Never fake a green: if the gate can't pass honestly, keep it `#[ignore]`d with an updated reason and report DONE_WITH_CONCERNS.
- Commit messages: `M2-bfam tN: <what>`.
- Exact values (verbatim): FPAC syndrome EC=0x1C arrives as `Ec::Other(0x1C)` (via `ec_of`'s catch-all). AUT* register-variant bases (mask `0xFFFF_FC00`): AUTIA `0xDAC1_1000`, AUTIB `0xDAC1_1400`, AUTDA `0xDAC1_1800`, AUTDB `0xDAC1_1C00`; Z-modifier forms: AUTIZA `0xDAC1_3000`, AUTIZB `0xDAC1_3400`, AUTDZA `0xDAC1_3800`, AUTDZB `0xDAC1_3C00`. `Rd = insn & 0x1F`. Canonical strip: `& 0x0000_7FFF_FFFF_FFFF` (the same 47-bit mask as M2-va47's `strip47`). The observed objc fault is `AUTDB x16, x17` = `0xDAC1_1E30` at `addClassTableEntry+0x70`.

---

### Task 1: The strip-on-FPAC arm (`decode_aut_rd` + `try_emulate_fpac_auth` + dispatch)

Add the pure instruction decoder (retrace-arch, unit-tested), the emulation method + run-loop arm (retrace-box), and an in-VM micro-guest that synthesizes an FPAC fault to prove the arm end-to-end. The full suite is the regression gate (the arm must be inert for the A-family path, which never FPACs).

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` — `decode_aut_rd` + inline unit test.
- Modify: `crates/retrace-box/src/lib.rs` — `try_emulate_fpac_auth` + the `Ec::Other(0x1C)` arm.
- Create: `crates/retrace-guest/asm/bfamstrip.s`
- Modify: `crates/retrace-guest/build.rs` (stanza), `crates/retrace-guest/src/lib.rs` (const)
- Create: `crates/retrace/tests/bfamstrip_e2e.rs`

**Interfaces:**
- Produces: `retrace_arch::decode_aut_rd(insn: u32) -> Option<u32>`; `Box_::try_emulate_fpac_auth(&mut self) -> bool` (private); `retrace_guest::BFAMSTRIP` guest path const. Task 2 relies on the arm being live in the shared `run()`.

- [ ] **Step 1: Write the failing decoder unit test.** In `crates/retrace-arch/src/lib.rs`, in the existing `#[cfg(test)] mod tests` (next to the `ec_of` tests), add:

```rust
    #[test]
    fn decodes_aut_destination_register() {
        // AUTDB x16, x17 — the observed objc fault at addClassTableEntry+0x70.
        assert_eq!(decode_aut_rd(0xDAC1_1E30), Some(16));
        // Each register variant returns Rd (bits [4:0]).
        assert_eq!(decode_aut_rd(0xDAC1_1000 | (1 << 5)), Some(0));        // AUTIA x0, x1
        assert_eq!(decode_aut_rd(0xDAC1_1400 | (2 << 5) | 3), Some(3));    // AUTIB x3, x2
        assert_eq!(decode_aut_rd(0xDAC1_1800 | (10 << 5) | 9), Some(9));   // AUTDA x9, x10
        assert_eq!(decode_aut_rd(0xDAC1_3800 | 30), Some(30));            // AUTDZA x30 (Z form)
        // Not an AUT-with-Rd: NOP, and PACIA (a SIGN, base 0xDAC1_0000) must return None.
        assert_eq!(decode_aut_rd(0xD503_201F), None);                     // NOP
        assert_eq!(decode_aut_rd(0xDAC1_0000 | (1 << 5)), None);          // PACIA x0, x1 (sign)
    }
```

- [ ] **Step 2: Run to verify it fails.** Run: `cargo test -p retrace-arch decodes_aut -- --test-threads=1` — Expected: FAIL to compile (`decode_aut_rd` not found).

- [ ] **Step 3: Implement `decode_aut_rd`.** In `crates/retrace-arch/src/lib.rs` (module scope, near `ec_of`):

```rust
/// If `insn` is an AArch64 pointer-authentication AUT* instruction whose authenticated result
/// lands in a destination register — the `AUTIA/AUTIB/AUTDA/AUTDB` register-modifier variants and
/// their `AUTIZA/AUTIZB/AUTDZA/AUTDZB` zero-modifier forms — return that register number (Rd).
/// Returns None otherwise. Used to emulate a B-family auth that FEAT_FPAC-faulted by stripping Rd
/// to canonical (see `Box_::try_emulate_fpac_auth`). Combined auth-and-{branch,load} forms
/// (`braab`/`ldrab`/…) have no Rd to fix and are intentionally NOT matched (they fail loud).
pub fn decode_aut_rd(insn: u32) -> Option<u32> {
    // "Data-processing (1 source)" PAC encodings: [31:10] fixed per op, [9:5] Rn, [4:0] Rd.
    match insn & 0xFFFF_FC00 {
        0xDAC1_1000 | 0xDAC1_1400 | 0xDAC1_1800 | 0xDAC1_1C00   // AUTIA/AUTIB/AUTDA/AUTDB Xd,Xn
        | 0xDAC1_3000 | 0xDAC1_3400 | 0xDAC1_3800 | 0xDAC1_3C00 // AUTIZA/AUTIZB/AUTDZA/AUTDZB Xd
            => Some(insn & 0x1F),
        _ => None,
    }
}
```

- [ ] **Step 4: Run to verify the decoder passes.** Run: `cargo test -p retrace-arch decodes_aut -- --test-threads=1` — Expected: PASS.

- [ ] **Step 5: Write the in-VM micro-guest + e2e test (they will RED until the arm lands).** Create `crates/retrace/tests/bfamstrip_e2e.rs`:

```rust
// Proves the strip-on-FPAC arm in isolation: a guest DATA-B-signs a canonical pointer, corrupts a
// PAC bit so `autdb` FEAT_FPAC-faults, then executes `autdb`. The box must intercept the FPAC,
// strip x0 to canonical (emulating a successful authenticate), and skip the instruction; the guest
// then finds the recovered pointer == the original and exits 0. Without the arm, the autdb FPACs,
// the box errors out, and record exits nonzero. Also replays identically.
mod util;
#[test]
fn bfamstrip_fpac_auth_emulated() {
    let (rec, trace) = util::record(retrace_guest::BFAMSTRIP);
    assert_eq!(rec.code, 0, "record failed (autdb FPAC not emulated?): {}", rec.stderr);
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
}
```

Create `crates/retrace-guest/asm/bfamstrip.s`:

```asm
.section __TEXT,__text
.global _start
.p2align 2
// pacdb-sign a canonical pointer, flip a PAC-field bit so autdb FPAC-faults, then autdb. The box's
// try_emulate_fpac_auth strips x0 to canonical (emulated auth). Exit 0 if recovered == original.
_start:
    movz x19, #0x2000, lsl #16    // P = 0x0000_0000_2000_0000 (canonical low VA; never dereferenced)
    mov  x0, x19
    movz x1, #0x5678              // modifier (fixed)
    pacdb x0, x1                  // x0 = DATA-B-signed P (guest APDB key)
    mov  x2, #1
    lsl  x2, x2, #48              // a bit inside the PAC field (under the 47-bit VA)
    eor  x0, x0, x2               // corrupt the signature -> autdb will FEAT_FPAC-fault
    autdb x0, x1                  // FPAC -> box strips x0 to canonical, skips this instruction
    cmp  x0, x19                  // recovered == original?
    b.ne fail
    mov  x0, #0
    b    exit
fail:
    mov  x0, #1
exit:
    mov  x16, #1                  // SYS_exit
    svc  #0x80
```

In `crates/retrace-guest/build.rs`, append (same shape as the `strip47` stanza):

```rust
    // bfamstrip: pacdb-sign + corrupt + autdb -> FEAT_FPAC fault the box emulates by stripping.
    // The M2-bfam strip-on-FPAC property test.
    let src = format!("{}/asm/bfamstrip.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/bfamstrip");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang bfamstrip");
    assert!(status.success(), "bfamstrip guest build failed");
```

In `crates/retrace-guest/src/lib.rs`, next to the other consts:

```rust
pub const BFAMSTRIP: &str = concat!(env!("OUT_DIR"), "/bfamstrip");
```

- [ ] **Step 6: Run to verify the e2e test fails (arm not yet present).** Run: `cargo test -p retrace --test bfamstrip_e2e -- --test-threads=1` — Expected: FAIL — `record failed (autdb FPAC not emulated?)`, because the `autdb` FPACs, surfaces as `Stop::Other`, and the record loop errors (record process exits nonzero). (If the build fails on `pacdb`/`autdb`, add `.arch armv8.3-a` at the top of `bfamstrip.s` — but `pacguest.s` already assembles PAC instructions under `-arch arm64`, so this should not be needed.)

- [ ] **Step 7: Implement `try_emulate_fpac_auth` + the dispatch arm.** In `crates/retrace-box/src/lib.rs`, add the method next to `try_emulate_undef_mrs` (~line 399):

```rust
    /// Emulate a B-family pointer authentication that FEAT_FPAC-faulted (ESR EC=0x1C). objc
    /// authenticates arm64e shared-cache pointers with the DATA-B/INSTR-B keys, but those cache
    /// pointers carry their host-process signature while the guest uses fixed guest keys, so
    /// `autdb`/`autib` fault; retrace's A-family cache re-signer can't reach them (v5 slide-info
    /// encodes only IA/DA). We trust the cache and emulate a *successful* authenticate: strip the
    /// aut*'s destination register to its canonical VA (the 47-bit strip proven by strip47) and
    /// skip the instruction. A pure function of the faulting register (deterministic) and below
    /// the record/replay layer, so both sides strip identically — nothing enters the trace.
    /// Returns false (→ Stop::Other) for any non-AUT-with-Rd instruction, so a combined
    /// auth+branch/load form fails loud rather than being silently mis-emulated.
    fn try_emulate_fpac_auth(&mut self) -> bool {
        let elr = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
        if self.host_span(elr).is_none() { return false; }
        let insn = u32::from_le_bytes(self.read_guest(elr, 4).try_into().unwrap());
        let Some(rd) = retrace_arch::decode_aut_rd(insn) else { return false; };
        if rd != 31 {                                    // x31 = XZR: nothing to strip/write
            let signed = self.vcpu.get_reg(reg::x(rd)).unwrap();
            self.vcpu.set_reg(reg::x(rd), signed & 0x0000_7FFF_FFFF_FFFF).unwrap();
        }
        let spsr = self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap();
        self.vcpu.set_reg(reg::PC, elr + 4).unwrap();
        self.vcpu.set_reg(reg::CPSR, spsr).unwrap();
        true
    }
```

In `Box_::run`, add the arm immediately after the `try_emulate_undef_mrs` arm (~line 1094):

```rust
                        // A B-family pointer-auth (autdb/autib) that FEAT_FPAC-faulted: objc auths
                        // arm64e cache pointers the A-family re-signer can't reach. Emulate the
                        // authenticate by stripping the aut* destination + skip, like the MRS
                        // emulations above — below the record/replay loop, so both stay in lockstep.
                        Ec::Other(0x1C) if self.try_emulate_fpac_auth() => continue,
```

- [ ] **Step 8: Run the micro-test to verify GREEN.** Run: `cargo test -p retrace --test bfamstrip_e2e -- --test-threads=1` — Expected: PASS (record exit 0, replay exit 0). The box now strips the corrupted-then-faulted pointer to canonical and the guest recovers `P`.

- [ ] **Step 9: Regression gate.** Run: `just m1` — Expected: all green, 1 ignored (`hello_dyn_e2e`, still). The A-family path never FPACs, so the new arm must be inert for every existing test. Run: `cargo clippy --workspace --all-targets -- -D warnings` — Expected: clean. **If any existing test regresses, STOP and report it — the arm should be unreachable for A-family guests.**

- [ ] **Step 10: Commit.**
```bash
git add -A && git commit -m "M2-bfam t1: strip-on-FPAC auth emulation (decode_aut_rd + try_emulate_fpac_auth)

Intercept B-family pointer-auth FEAT_FPAC faults (EC=0x1C -> Ec::Other(0x1C)) in
the shared run loop: decode the aut* (retrace_arch::decode_aut_rd), strip its
destination register to canonical (47-bit mask, strip47-proven), skip it — an
emulated successful authenticate, mirroring try_emulate_undef_mrs. Below the
record/replay layer, so determinism is automatic. New bfamstrip guest synthesizes
an autdb FPAC (sign+corrupt+autdb) and proves the emulation end-to-end (RED without
the arm, GREEN with it). Full suite green (arm inert for the A-family path)."
```

---

### Task 2: Gating spike + empirical walk to un-ignore `hello_dyn_e2e`

Confirm the arm carries `hello_dyn` past objc class realization (go/no-go), then walk any remaining walls to `main → write → exit`. Investigation-shaped. An honest DONE_WITH_CONCERNS is acceptable.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` and/or `crates/retrace-core/src/lib.rs` (any per-wall fixes)
- Modify: `crates/retrace/tests/hello_dyn_e2e.rs` (remove `#[ignore]`, update comment, add double-replay)

**Interfaces:**
- Consumes: Task 1's strip-on-FPAC arm.
- Produces: the green (or honestly-still-blocked) M2 gate.

- [ ] **Step 1: The gating spike (go/no-go).** Build, codesign, run the bounded dynamic record:

```bash
cargo build -p retrace && codesign -s - -f --entitlements retrace.entitlements target/aarch64-apple-darwin/debug/retrace
HD=$(find target -name hello_dyn -path "*out*" | head -1)
RETRACE_TRACE=1 perl -e 'alarm 60; exec @ARGV' -- \
  ./target/aarch64-apple-darwin/debug/retrace record-dyn "$HD" -o /tmp/bfam-walk.bin 2>walk.log; tail -40 walk.log
```

Expected: the run advances **past `addClassTableEntry`** — the `autdb` no longer fatally faults; objc proceeds into the class it was realizing. **GO/NO-GO:** if objc immediately chokes right after the emulated auth (a garbage `class_rw_t` dereference — a data abort on a nonsense address derived from the stripped pointer), the "trust and strip" premise is wrong — STOP, capture the abort + backtrace, and report NEEDS_CONTEXT. If it proceeds, record the new first-failure and walk.

- [ ] **Step 2: Walk each wall.** Repeat the run; for each first-failure, diagnose (`RETRACE_TRACE=1` trap log + `Box_::dbg_backtrace`) and fix, committing each separately (`M2-bfam t2: <failure> — <fix>`). Determinism: any fix touching the record/replay dispatch (`crates/retrace-core/src/lib.rs`) must be mirrored across both arms. Standalone B-family auts are handled for free by Task 1's arm. Likely categories: more objc/runtime init; another mach RPC (extend the M2-mach codec + both dispatch arms); a plain syscall (forward + memory-diff). A **combined-form** B-family fault (`braab`/`ldrab` — an `aut*` that `decode_aut_rd` returns `None` for, surfacing as `Stop::Other` with EC=0x1C) or a whole absent subsystem is the honest stopping point — see Step 4.

- [ ] **Step 3: Un-ignore the gate + double-replay.** When `hello_dyn` records exit 0 with stdout `hi\n`: in `crates/retrace/tests/hello_dyn_e2e.rs`, delete the `#[ignore = ...]` line, rewrite the stale block comment to describe the full working path (cache re-signing → mach servicing → 47-bit VA → B-family strip-on-FPAC), and add:

```rust
// Determinism hardening: the SAME trace replays identically twice.
#[test]
fn hello_dyn_replays_twice_identically() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let a = util::replay(&trace);
    let b = util::replay(&trace);
    assert_eq!(a.code, 0, "first replay diverged: {}", a.stderr);
    assert_eq!(b.code, 0, "second replay diverged: {}", b.stderr);
    assert_eq!(a.stdout, b.stdout);
    assert_eq!(a.stdout, b"hi\n");
}
```

Run: `just m1` — Expected: all green, **0 ignored**. Clippy clean. Measure + report the `hello_dyn` record+replay wall-clock (for the swarm go/no-go, deferred per spec).

- [ ] **Step 4: Honesty clause.** If Step 1 is a NO-GO, or Step 2 hits a combined-form B-family fault or a hard distinct-subsystem boundary: keep `#[ignore]` with a new precise reason, write the boundary's anatomy into the task report, and STOP as DONE_WITH_CONCERNS. Do not fake any part of the gate.

- [ ] **Step 5: Commit.**
```bash
git add -A && git commit -m "M2-bfam t2: hello_dyn past objc B-family — gate un-ignored + double-replay"
```

---

### Task 3: Docs close-out

**Files:**
- Modify: `README.md` (new M2-bfam status section after M2-va47)
- Modify: `docs/superpowers/specs/2026-07-05-retrace-macos-record-replay-design.md` (milestone note)

- [ ] **Step 1: README.** Add `## Status: M2-bfam — objc B-family PAC ✅` (or the honest blocked variant per Task 2's outcome) after the M2-va47 section, matching the established voice (read the M2 / M2-cache / M2-mach / M2-va47 sections first). Cover: what it does (intercept B-family auth FPAC, strip aut* destination to canonical, skip — an emulated authenticate; below the record/replay layer so determinism is automatic); proven (the `bfamstrip` micro-test + `decode_aut_rd` unit test) with the real `just m1` count (RUN IT, don't guess); and — **if the gate went green** — say plainly that `hello_dyn_e2e` now records and replays a real dynamically-linked program (the headline M2 gate cleared), listing what's deferred (combined auth+branch/load B-family forms, arm64e guest, swarm extension). If still blocked, describe the new boundary honestly. Point at `docs/superpowers/specs/2026-07-10-retrace-m2-bfam-design.md`. (An `✅` header with a deferred gate matches this repo's convention; if the gate is green, no deferral qualifier is needed.)

- [ ] **Step 2: Main spec milestone note.** In `docs/superpowers/specs/2026-07-05-retrace-macos-record-replay-design.md`, extend the "2026-07-05 update" blockquote's sub-milestone sentence to include M2-bfam (objc B-family PAC, `2026-07-10-retrace-m2-bfam-design.md`) alongside M2-cache/M2-mach/M2-va47. If the gate went green, note that M2 is complete (a real dynamically-linked program records+replays). Do not renumber M3–M6.

- [ ] **Step 3: Verify claims.** Every number/claim in Steps 1–2 must trace to something run/read this task (test count from `just m1`; gate state from the actual `hello_dyn_e2e.rs`).

- [ ] **Step 4: Commit.**
```bash
git add README.md docs/ && git commit -m "M2-bfam t3: README + spec status"
```

---

## Self-Review (author)

- **Spec coverage:** `try_emulate_fpac_auth` + `decode_aut_rd` + `Ec::Other(0x1C)` arm (T1 Steps 3,7); 47-bit strip (T1 Step 7 — `& 0x0000_7FFF_FFFF_FFFF`); determinism below the trace layer (T1 — the arm is in shared `run()`, no trace change); the gating spike (T2 Step 1); the walk + un-ignore + double-replay (T2); standalone-vs-combined edge (T1 `decode_aut_rd` returns None for combined forms → they surface loud; T2 Step 2/4); regression inertness (T1 Step 9); docs (T3). Spec open questions: (1) `decode_aut_rd` coverage → register + Z variants (T1 Step 3), combined on-demand (T2); (2) micro-test worth it → yes, `bfamstrip` (T1 Steps 5–8).
- **Placeholder scan:** none — the per-wall fixes in T2 are genuinely unknowable ahead of the empirical run (investigation task), with a concrete triage method.
- **Type consistency:** `decode_aut_rd(insn: u32) -> Option<u32>` used identically in the retrace-arch test, `try_emulate_fpac_auth`, and referenced as `retrace_arch::decode_aut_rd`; `BFAMSTRIP` const matches the test's `retrace_guest::BFAMSTRIP`; the strip mask `0x0000_7FFF_FFFF_FFFF` matches M2-va47's `strip47`; AUTDB `0xDAC1_1E30` consistent between the Global Constraints, the decoder test, and the guest's `autdb x0,x1` (the guest uses x0/x1, encoding `0xDAC1_1C00 | (1<<5) | 0 = 0xDAC1_1C20`, which `decode_aut_rd` maps to `Some(0)` — the arm strips x0).
