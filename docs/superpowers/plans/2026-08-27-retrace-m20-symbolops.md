# M20-symbolops Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development (recommended)
> or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `break _main` / `delete _main` accept a symbol name where they accept an address today.

**Architecture:** Presentation-layer, like M19. A reverse lookup (`addrs_of`) in the pure
`symbols.rs`, an `Operand` enum in the CLI grammar, and resolution deferred to *execution* because
parsing completes before the trace is opened (S1). Nothing touches `record_box`,
`ReplaySession::advance`, or `Box_::run()`; **neither symmetry rule is engaged and the divergence
oracle cannot see this milestone.**

**Spec:** `docs/superpowers/specs/2026-08-27-retrace-m20-symbolops-design.md`
**Measurements:** `docs/superpowers/specs/2026-08-27-retrace-m20-symbolops-measurements.md` (S1–S6)

## Global Constraints

- **`--test-threads=1` is mandatory.** One VM per process; a bare `cargo test` flakes with `HV_BUSY`.
- **The gate is chunked.** Full `--workspace` exceeds the 10-minute ceiling. Run every chunk
  `--no-fail-fast`, capture cargo's exit code **before any pipe**, and **never omit `--bins`** — the
  unit tests in `debug.rs` run in no other chunk, and M20 adds more of them there than anywhere else.
  `cargo test -p retrace --lib` is invalid (no lib target) and fails loudly; the trap is that the
  wrong flag is loud and the missing one is silent.
- **`TRACE_MAGIC` does NOT move.** M20 records nothing. If you believe you need a format change, stop.
- **Never silently pick.** An ambiguous name is an error listing candidates (S4). A name with no match
  is an error, never a fallback to hex.
- **Baseline to preserve:** 463 `#[test]`, 461 run, 2 `#[ignore]`d, 105 binaries at `88aa758`.
- **Apply the M19 lesson before editing an assertion.** M19 lost four assertions to an appended
  suffix, and the *obvious* repair (`ends_with` → `contains`) would have destroyed a fifth. Before
  changing any existing assertion, read what it was protecting; prefer `util::strip_annot` +
  `ends_with` over loosening.

---

### Task 1: `addrs_of` — the reverse lookup, pure and VM-free

**TDD. Write each test before its implementation.** No VM; runs in the fast workspace chunk against
synthetic images built by the existing `Img` test helper in `symbols.rs`.

**Files:** modify `crates/retrace-core/src/symbols.rs`

**Interface:**
```rust
impl SymbolTable { pub fn addrs_of(&self, name: &str) -> Vec<u64>; }   // sorted, deduped
impl Symbols     { pub fn addrs_of(&self, name: &str) -> Vec<u64>; }   // exe first, then dyld
```

- [ ] **Step 1: `SymbolTable::addrs_of`.** Exact-match only (design open question 3 — substring
  matching reintroduces the ambiguity S4 exists to refuse). Tests: a unique name returns exactly one
  address; an absent name returns empty; **a name at two addresses returns both, sorted** (this is
  S4's case and the one that must not regress); the result is deduped.

- [ ] **Step 2: `Symbols::addrs_of` with exe-before-dyld precedence.** Search the executable's table
  first; consult dyld **only if the executable yields nothing**. Tests: a name in the exe alone
  resolves; a name in dyld alone resolves; **a name present in BOTH returns only the exe's** — pin
  the precedence rule rather than letting vector order imply it.

- [ ] **Step 3: data symbols are reachable by name (S6).** A `__DATA` symbol is in `syms` but past
  `text_end`, so `resolve` cannot return it while `addrs_of` can. Test both directions on one
  synthetic symbol. This **confirms S6**, which was source-read rather than measured — and it does
  **not** license `watch <name>`, which S5 blocks independently.

**Verification:** `cargo test -p retrace-core -- --test-threads=1` green; clippy clean; no VM used.

---

### Task 2: `Operand` and the classification rule

**TDD.** These are `parse_script` unit tests inside `debug.rs`, so they land in the **`--bins`** chunk.

**Files:** modify `crates/retrace/src/debug.rs`

- [ ] **Step 1: `enum Operand { Addr(u64), Sym(String) }`** in `debug.rs` (design open question 1 —
  it is CLI grammar, and `symbols.rs` stays free of CLI concepts). `Cmd::Break`/`Cmd::Delete` carry
  it; every other command keeps `parse_addr` unchanged.

- [ ] **Step 2: the classification rule**, in the spec's order. Tests, one per rule and one per trap:
  `break 0x1000` → `Addr`; `break 1000` → `Addr` (bare hex still an address — the backward-compat
  rule); `break _main` → `Sym`; `break deadbeef` → **`Addr`**, pinning the documented collision;
  `break _ZN4core3ptrE` → `Sym`.

- [ ] **Step 3: replace, do not delete, `debug.rs:763`.** `assert!(parse_script("break zzz").is_err())`
  is the only assertion whose meaning changes (S3). It becomes an assertion that `zzz` now parses as
  `Sym("zzz")`. **`debug.rs:762` (`frobnicate`, an unknown verb) must stay an error** — add an
  explicit test that it still is, so the two cases cannot be confused by a later reader.

**Verification:** `cargo test -p retrace --bins -- --test-threads=1` green. Do not use `--lib`.

---

### Task 3: Resolve at execution, and the errors

**Files:** modify `crates/retrace/src/debug.rs`

- [ ] **Step 1: resolve in `Exec`.** A helper `fn addr_of(&self, op: &Operand) -> Result<u64, String>`:
  `Addr(a)` → `a`; `Sym(n)` → `self.syms.addrs_of(n)` with the 0/1/many rule. Wire `cmd_break` and
  `cmd_delete` through it. Returning `Result` matches the existing `Err(String)` → exit 5 path
  (design open question 2).

- [ ] **Step 2: the two error messages, written to be acted on.**
  `no symbol "foo"` for zero matches; for many, name the count **and list the addresses** — a bare
  "ambiguous" leaves the user no way forward, and the addresses are what M19 taught the tool to
  print. Test both strings.

- [ ] **Step 3: the break echo.** `cmd_break` prints `Symbols::format(addr)` rather than
  `{addr:#x}`, so `break _main` echoes `breakpoint at 0x100001130 (_main)`. **Pre-verified safe** (R5):
  both existing assertions on that line use `contains`, not `ends_with`. Re-check by grep before
  editing anyway — that check is exactly what M19 skipped.

- [ ] **Step 4: pin the ordering change (S2, R3).** A test that `where; break zzz` now emits the
  `where` line **before** failing, and still exits 5. This is the milestone's one behavioural
  regression and it must be asserted, not merely mentioned in the README.

**Verification:** `cargo test -p retrace --bins -- --test-threads=1` green.

---

### Task 4: The headline gate

**Files:** create `crates/retrace/tests/symbolops_e2e.rs`

- [ ] **Step 1: the headline.** Record `crashthread`; `break _child; continue`; assert the **stop pc**
  equals `_child`'s address. Per honest-gate discipline the assertion is the *pc*, never the echo
  text or a successful parse — a no-op that accepted the token and set no breakpoint would produce
  both of those.

- [ ] **Step 2: verify the gate can fail.** Stub `addrs_of` to return `vec![]`, confirm the headline
  goes **red**, restore, confirm green. Record *how* it fails. A gate never observed failing is not
  yet a gate — and M19's own fail-verification is the model, including that a negative test needs a
  guard against its own vacuity.

- [ ] **Step 3: the adversary.** On a stripped guest (`jq`, 7 symbols — M3), `break _nonexistent`
  errors with `no symbol` and does **not** panic, exit 5. Skip loudly with `eprintln!` if
  `/opt/homebrew/bin/jq` is absent — a silent skip reads as a green it did not earn.

- [ ] **Step 4: `delete` round-trips by name.** `break _child; delete _child; continue` runs to the
  crash without stopping — proving `delete` resolves to the *same* address `break` did, which a
  test that only checks `delete` is accepted would not.

**Verification:** `cargo test -p retrace --test symbolops_e2e -- --test-threads=1` green and verified
red under the stub; `debug_cli`, `watch_cli`, `crashy_cli`, `reverse_debug_e2e` still green (the R2
regression corpus).

---

### Task 5: The gate and the two documents

- [ ] **Step 1: run the full chunked gate** per Global Constraints. Reconcile against the 463/461/2
  baseline by **diffing `#[test]` counts file-by-file**, not by trusting a sum. Expect: +N in
  `symbols.rs`, +N in `debug.rs` (the `--bins` chunk), +N in the new `symbolops_e2e` target, and
  **+1 binary** (104 → 105 was M19; M20 makes it 106). Clippy clean at `-D warnings`.

- [ ] **Step 2: both documents, which must not be merged.**
  - **README** — edited **in place**: the debugger-commands table gains `<addr|symbol>` for
    `break`/`delete`; "What works today" notes symbol operands; "Known limits" states what stays
    address-only (`watch`, `x`) **and why** (S5, no size in `nlist_64`), plus the S2 ordering change;
    the gate line's new counts.
  - **`docs/status-log.md`** — **append** a `## Status: M20-symbolops` section. Never rewrite an
    earlier one.
  - **CLAUDE.md** — only if a statement became false. Expected: **none**.

**Verification:** gate green, both documents updated, `git status` clean apart from intended files.

---

## Self-Review

1. **`TRACE_MAGIC` is still `RT\x00\x08`** and no `Event` variant changed.
2. **Nothing under `record_box` / `ReplaySession::advance` / `Box_::run()` was modified.**
3. **`verify_thread` still has exactly seven call sites** plus `mirror_delivery`'s inline eighth.
4. **An ambiguous name errors and lists addresses** — check against a real dyld name from S4,
   not only a synthetic one.
5. **A bare-hex token is still an address.** `break 1000` must not become a symbol lookup; the
   existing debug scripts in the regression corpus are the evidence.
6. **A stripped guest errors rather than panicking**, and the skip announces itself.
7. **No existing assertion was loosened to make something pass.** If one changed, it was because its
   subject changed, and the replacement pins the new rule.
