# M2-tbi Implementation Plan — arm64e data-pointer PAC placement (guest TCR TBI)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable TBI0+TBID0 in the guest `TCR_EL1` so re-signed data-pointer PACs stop colliding with objc's `FAST_IS_RW_POINTER` (bit 63) flag — clearing the misdiagnosed "objc preoptimization" wall — and correct every artifact that documented that wall wrongly, re-parking the headline gate at the newly-exposed mmap demand-commit boundary.

**Architecture:** objc reads bit 63 of the raw `class_data_bits_t::bits` word as its is-realized flag. With `TCR_EL1.TBI0 = 0` and a 47-bit VA, the guest PAC field spans bits [63:56]∪[54:47], so a re-signed data pointer sets bit 63 and unrealized `NSObject` looks already-realized → `validateAlreadyRealizedClass` fatal. Setting TBI0(37)+TBID0(51) matches Apple's arm64e user config: data-pointer PAC moves to [54:47], the top byte (incl. bit 63) is preserved = 0, and instruction-pointer PAC stays full-strength. The change is a single constant consumed by all CPU-init sites; it lives below the shared `run()`/`load` path so record and replay are identical. **The root cause and fix are already confirmed by the 2026-07-14 investigation spike** (objc fatal gone, advances to an mmap-region data abort, full gate 58/0/1, clippy clean) — this plan lands the confirmed fix and the honest re-documentation.

**Tech Stack:** Rust workspace (`retrace-arch`, `retrace-box`, `retrace-guest`, `retrace`), Hypervisor.framework via `hv-sys`, arm64 asm guests. Spec: `docs/superpowers/specs/2026-07-14-retrace-m2-tbi-design.md` — read it before starting.

## Global Constraints

- Branch: `m2-tbi` (create from `main` before Task 1).
- All test runs: `cargo test --workspace -- --test-threads=1` (one VM per process; parallel VMs flake with `HV_BUSY`). `just gate` (= `just m1`) is the full gate.
- `cargo clippy --workspace --all-targets -- -D warnings` clean at every commit.
- Any binary calling `hv_*` must be ad-hoc codesigned with `retrace.entitlements`; for a manually-run `target/.../retrace`: `codesign -s - -f --entitlements retrace.entitlements target/aarch64-apple-darwin/debug/retrace`. Manual VM runs bounded: `perl -e 'alarm 60; exec @ARGV' -- <cmd>`.
- No `Date::now()`/randomness in tests (determinism deny-list).
- Never fake a green: `hello_dyn_e2e` stays `#[ignore]`d — this milestone does NOT reach `main`; it re-parks the gate at the mmap wall.
- Commit messages: `M2-tbi tN: <what>`.
- Exact values (verbatim): current `TCR_EL1_V = 0x1_0080_B511` (T0SZ=17, TG0=16K, WBWA, inner-share, EPD1, IPS=36-bit, TBI0=0, TBID0=0). Fixed `TCR_EL1_V = 0x8_0021_0080_B511` (sets bit 37 = TBI0 and bit 51 = TBID0; nothing else changes). `FAST_IS_RW_POINTER = 0x8000000000000000` (bit 63). Observed pre-fix guest `bits = 0x964a8001ed950f80` (bit 63 set); fatal class = `NSObject` (`0x1ec2f1618`), `data()` = `_OBJC_CLASS_RO_$_NSObject` (`0x1ed950f80`). New wall: `data abort EC=0x24 ISS=0x7 FSC=0x7 far=0xa0010e744 (UNMAPPED)` = `MMAP_BASE (0xA_0000_0000) + 0x10e744`.

---

### Task 1: Land the confirmed TCR fix (TBI0+TBID0)

Change the single `TCR_EL1_V` constant and document the rationale. The full regression suite is the gate — the change must perturb no existing test, and must advance `record-dyn hello_dyn` past the objc fatal.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` — the `TCR_EL1_V` constant + comment.

**Interfaces:**
- Produces: the corrected `TCR_EL1_V` consumed unchanged at the three `set_sys(sysreg::TCR_EL1, TCR_EL1_V)` sites. No signature/API change.

- [ ] **Step 1: Change the constant.** In `crates/retrace-box/src/lib.rs`, replace:
  ```rust
  const TCR_EL1_V:  u64 = 0x1_0080_B511;        // T0SZ=17 (47-bit VA), TG0=16K, WBWA, inner-share, EPD1, IPS=36-bit
  ```
  with:
  ```rust
  // arm64e data-pointer PAC placement: TBI0(bit37)+TBID0(bit51) match Apple's user TCR so a signed
  // DATA pointer's PAC lands in [54:47] with the top byte (incl. bit 63) preserved = 0. Without TBI
  // the 47-bit-VA PAC field spans [63:56]∪[54:47]; a re-signed class_data_bits pointer then sets
  // bit 63, which objc reads as FAST_IS_RW_POINTER (isRealized) → spurious already-realized →
  // validateAlreadyRealizedClass fatal. See docs/.../2026-07-14-retrace-m2-tbi-design.md.
  const TCR_EL1_V:  u64 = 0x8_0021_0080_B511;    // +TBI0+TBID0. T0SZ=17 (47-bit VA), TG0=16K, WBWA, inner-share, EPD1, IPS=36-bit
  ```

- [ ] **Step 2: Regression gate.** Run `just gate`. Expected: **58 passed, 0 failed, 1 ignored**, clippy clean — identical to pre-change. If any test regresses, STOP: the TBI change is perturbing a path the spike didn't exercise; investigate before proceeding (do not paper over).

- [ ] **Step 3: Fix confirmation (manual, documented in the task report).** Build + codesign, then bounded `record-dyn hello_dyn` with `RETRACE_TRACE=1`. Confirm the run **no longer** prints `objc[…]: realized class 0x1ec2f1618 has corrupt data pointer` and instead reaches the mmap-region data abort `far=0xa0010e744 (UNMAPPED)` (exit 4 via `RECORD ERROR`, not exit 134). Record the tail of the walk log in the task report. (Recipe in the spec's "Reproduce".)

- [ ] **Step 4: Commit.** `M2-tbi t1: TBI0+TBID0 in guest TCR — clears the objc validateAlreadyRealizedClass wall (bit-63/FAST_IS_RW_POINTER collision)`.

---

### Task 2: Honest correction pass + re-park the gate

Every artifact that documented the misdiagnosed "objc shared-cache preoptimization" wall must be corrected to the verified root cause and re-parked at the mmap demand-commit boundary. No fake green; the gate stays `#[ignore]`d.

**Files:**
- Modify: `crates/retrace/tests/hello_dyn_e2e.rs` — `#[ignore]` reason.
- Modify: `README.md` — M2-bfam Status "next wall" paragraph + new M2-tbi Status section.
- Modify: `docs/superpowers/specs/2026-07-10-retrace-m2-bfam-design.md` — closeout/status correction note.
- Modify: `.superpowers/sdd/task-m2bfam-2-report.md` — appended correction (gitignored scratch; correct anyway for honesty).

- [ ] **Step 1: Rewrite the `hello_dyn_e2e` `#[ignore]` reason.** Replace the objc-preoptimization narrative with: the objc `validateAlreadyRealizedClass` wall was a bit-63/`FAST_IS_RW_POINTER` collision from TBI-off PAC placement, fixed in M2-tbi; the guest now realizes classes and blocks at the mmap demand-commit wall — a level-3 translation fault on `MMAP_BASE + 0x10e744` (an anonymous `mmap` page libmalloc obtained but retrace reserved without backing). Cite `2026-07-14-retrace-m2-tbi-design.md`.

- [ ] **Step 2: Correct README.** In the M2-bfam Status section, correct the "Honestly blocked" paragraph that names "objc shared-cache preoptimization" as the next wall — mark it superseded and point to the new section. Add a `## Status: M2-tbi — arm64e data-pointer PAC (TCR TBI) ✅` section: the root cause (bit 63 collision, NSObject/RO evidence, the `OBJC_DISABLE_PREOPTIMIZATION=YES` host control, the objc4 source facts), the one-line fix, the regression result (58/0/1), and the new mmap demand-commit boundary.

- [ ] **Step 3: Correct the M2-bfam design spec.** Add a dated closeout note at the top or in its status/exit-criterion area: the "objc preoptimization" boundary it documented was misdiagnosed; see `2026-07-14-retrace-m2-tbi-design.md` for the verified root cause and fix. Do not rewrite history — annotate.

- [ ] **Step 4: Correct task-m2bfam-2-report.md.** Append a `## Correction (2026-07-14)` section: §2/§3's "preoptimized cache-resident `class_rw_t`" and "objc refuses to trust the re-signed cache" framing was wrong; the real cause is the bit-63 PAC-field collision (TBI off), fixed by TCR TBI0+TBID0; the strip-on-FPAC arm was sound and is unaffected.

- [ ] **Step 5: Consistency grep.** `grep -rn -iE "preoptimiz|class_rw_t|cache-trust|objc_opt" README.md docs/ crates/retrace/tests/hello_dyn_e2e.rs` and confirm no remaining text asserts the disproven narrative **as the current wall** (historical/annotated mentions are fine). Confirm the memory `retrace-objc-preoptimization-wall` is consistent (updated in the investigation session).

- [ ] **Step 6: Final gate + commit.** `just gate` green (58/0/1), clippy clean. Commit: `M2-tbi t2: honest correction — objc-preopt wall was a bit-63/TBI misdiagnosis; re-park gate at mmap demand-commit`.

---

## Integration & close-out

- After Task 2, `just gate` is 58/0/1 green, clippy clean, and `hello_dyn_e2e` is `#[ignore]`d with the accurate mmap-demand-commit reason.
- Update memory `retrace-objc-preoptimization-wall` if any detail drifted from this plan (it was updated during the investigation; keep it the single source of truth for the correction).
- Merge `m2-tbi` → `main` (`Merge M2-tbi (arm64e data-pointer PAC / TCR TBI) into main`).
- The next milestone is the **mmap demand-commit wall** (`MMAP_BASE + 0x10e744` translation fault) — spec it separately; it is explicitly out of scope here.

## Notes for the implementer

- This is a **one-constant** functional change de-risked by the spike; the bulk of the work is the honest documentation correction (Task 2). Resist scope creep into the mmap wall — that is a separate milestone with its own spike/spec/plan.
- `TCR_EL1` is a load-bearing platform invariant. The only sanctioned value is `0x8_0021_0080_B511`; do not add TBI1/TBID1 (TTBR1 is unused — EPD1 is set) or alter T0SZ/IPS.
