# M19-symbols Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `retrace debug` print `pc=0x10000050c (_child+0x30)` instead of `pc=0x10000050c`,
reading the symbol table out of the recording's own snapshot — no trace-format change, no binary
path supplied — or park the gate at a wall M19 actually reached and measured.

**Architecture:** Entirely presentation-layer, and that is the milestone's defining property. A new
pure module `crates/retrace-core/src/symbols.rs` (on `machmsg.rs`'s precedent) reads `nlist_64`
records out of `Event::Snapshot`'s regions and answers address→name; `crates/retrace/src/debug.rs`
consumes it at its pc-printing sites. Nothing touches `record_box`, `ReplaySession::advance`, or
`Box_::run()`. **Neither symmetry rule is engaged and the divergence oracle cannot see this
milestone** — M19 is incapable of making a recording diverge, which is why it is safe to do in one
pass.

**Tech Stack:** Rust 1.95.0, `aarch64-apple-darwin`, macOS 26.x on Apple Silicon. No new
dependencies (no demangler, no object-file crate — the reader is ~200 lines of integer parsing).

**Spec:** `docs/superpowers/specs/2026-08-25-retrace-m19-symbols-design.md`
**Measurements:** `docs/superpowers/specs/2026-08-25-retrace-m19-symbols-measurements.md` (cited as M1–M7)

## Global Constraints

- **`--test-threads=1` is mandatory.** HVF allows one VM per process; a bare `cargo test` flakes with `HV_BUSY`.
- **The gate is chunked** — the full `--workspace` run exceeds the 10-minute tool ceiling. Run every chunk `--no-fail-fast` and capture cargo's exit code **before any pipe**:
  ```sh
  cargo test --workspace --exclude retrace-box --exclude retrace -- --test-threads=1
  cargo test -p retrace-box -- --test-threads=1
  cargo test -p retrace --test <name> -- --test-threads=1   # per-target, for each e2e gate
  cargo test -p retrace --bins -- --test-threads=1          # NEVER omit: unit tests run in no other chunk
  ```
  `cargo test -p retrace --lib` is **invalid** for this crate (no lib target) and fails loudly. The trap is that the wrong flag is loud and the missing one is silent. **In zsh, `${PIPESTATUS[0]}` is empty** — the array is `$pipestatus` and 1-indexed; redirect to a file and read `$?` instead of trusting a pipe.
- **Grep gate logs with `grep -a`** — they carry ANSI and UTF-8 that trips plain grep.
- **clippy must be clean at `-D warnings`.**
- **`TRACE_MAGIC` does NOT move.** M19 adds no `Event` variant and no field. The entire point of M4/M5 is that it does not have to. If you believe you need a format change, stop — that is a spec deviation.
- **Symbolication never fails a replay.** No `Divergence` may originate in this milestone. A table that cannot be built degrades to hex output; it does not error.
- **Absence is data, malformation is a bug.** A missing `LC_SYMTAB` or a stripped binary is normal (M3: `jq` has 7 text symbols) and must print hex silently. A *malformed* table — offsets outside `__LINKEDIT`, `n_strx` past `strsize` — is a bug in the reader and asserts.
- **Honest-gate discipline.** Assert on the difference your work makes. The raw address printed *before* M19 too, so an assertion on `0x10000050c` passes on a no-op; the assertion must be the **name**.
- **Baseline to preserve:** 444 `#[test]`, one live `#[ignore]` (`stackoverflow_rust_e2e`). M19 is additive; the ignored count moves 1→2 only via the deliberate parked gate in Task 5, which the status log must state.

---

### Task 1: Settle the three open questions against a real snapshot

**No production code.** Appends a `## Post-design measurements` section to the existing measurements
document. The design spec's "Open questions for implementation planning" names exactly these; each
must become a number before the reader is written, because each one silently changes the reader's
shape.

**Files:**
- Modify: `docs/superpowers/specs/2026-08-25-retrace-m19-symbols-measurements.md` (append only)

- [ ] **Step 1: Does one `Region` cover the whole `__LINKEDIT`?** (R1 / open question 1)
  Record `crashthread`, read the trace, and for the initial `Event::Snapshot` print each `Region`'s
  `(ipa, len)`. Determine whether `[0x100008000, 0x10000c000)` falls inside a single region or spans
  several. **If it spans, the reader must gather bytes across regions by IPA**; if not, it may take
  the simpler single-region path — but write the gathering helper either way, since a *different*
  guest may span even if `crashthread` does not. Record the answer as a number, not a belief.

- [ ] **Step 2: Pin the `nlist_64` constants.** (R2)
  `N_STAB = 0xe0`, `N_TYPE = 0x0e`, `N_SECT = 0x0e`, `N_EXT = 0x01` — verify against
  `/usr/include/mach-o/nlist.h` on *this* machine and quote the file. Any constant not verified there
  is **attributed**, not measured, and must be labelled so at its use site (the discipline
  `guest_workq_kernreturn`'s opcode constants already follow).

- [ ] **Step 3: Confirm dyld's slide arithmetic.** (R3 — the risk whose failure mode is confident
  wrong names) Read `/usr/lib/dyld`'s `__TEXT` vmaddr with `otool -l`, and record
  `DYLD_BASE − that`. Name one dyld symbol and its expected post-slide guest address, to be asserted
  in Task 4. **Never hardcode the slide** — derive it from the header at runtime; this step only
  produces the expected value a test checks against.

**Verification:** the document contains three numbers and their commands; no code changed;
`git diff --stat` touches one file.

---

### Task 2: `symbols.rs` — the reader and the resolver, pure and VM-free

**TDD. Write each test before its implementation.** Every test in this task runs with no VM, in the
fast workspace chunk, against synthetic `Region`s built in the test — which is the whole reason the
module is pure. Do not reach for a real recording here; Task 3 does that.

**Files:**
- Create: `crates/retrace-core/src/symbols.rs`
- Modify: `crates/retrace-core/src/lib.rs` (add `pub mod symbols;`)

**Interfaces produced** (consumed by name in Tasks 3 and 4):
```rust
pub struct SymbolTable { /* sorted (addr, name), text_end */ }
impl SymbolTable {
    /// Build for the image based at `base`. `None` when the image has no usable symbols
    /// (missing LC_SYMTAB, zero defined symbols) — that is NORMAL, not an error.
    pub fn for_image(mem: &[Region], base: u64) -> Option<SymbolTable>;
    /// (name, offset) for the nearest preceding symbol, or None past `text_end`.
    pub fn resolve(&self, addr: u64) -> Option<(&str, u64)>;
}
/// All images M19 knows, resolved in one call. Never panics on a stripped image.
pub struct Symbols { /* main exe, dyld */ }
impl Symbols {
    pub fn from_snapshot(mem: &[Region]) -> Symbols;
    /// "0x10000050c (_child+0x30)" | "0x100000460 (_main)" | "0x1c0004000"
    pub fn format(&self, addr: u64) -> String;
}
```

- [ ] **Step 1: A byte-gathering helper over regions.** `fn read(mem, ipa, len) -> Option<Vec<u8>>`,
  spanning regions if Task 1 Step 1 says it must. Test: a value split across two adjacent regions is
  read correctly; a value in no region returns `None` (not a panic — a caller decides).

- [ ] **Step 2: Header + `LC_SYMTAB` + `__LINKEDIT`/`__TEXT` discovery.** Walk load commands from the
  header at `base`. Test with a synthetic Mach-O header: finds `symoff`/`nsyms`/`stroff`/`strsize`;
  returns `None` when `LC_SYMTAB` is absent (the `jq` case); asserts when `MH_MAGIC_64` is missing at
  `base` (reusing the "refusing to guess" posture of `retrace-box/src/lib.rs:183`).

- [ ] **Step 3: File-offset → guest-VA conversion through `__LINKEDIT`.** The M4 arithmetic:
  `va = linkedit_vmaddr + (fileoff − linkedit_fileoff) + slide`. Test the exact worked example from
  M4: `symoff 32960`, `__LINKEDIT` at vmaddr `0x100008000` / fileoff `32768`, slide 0 → `0x1000080c0`.

- [ ] **Step 4: `nlist_64` parse with filtering.** 16-byte records
  (`n_strx:u32, n_type:u8, n_sect:u8, n_desc:u16, n_value:u64`). Keep `n_type & N_TYPE == N_SECT`
  with `n_sect != 0`; **skip `n_type & N_STAB != 0`**. Tests: a local (`t`) symbol is kept — this is
  M1's point and the one that must not regress; an undefined (`U`) symbol is dropped; a synthetic
  `N_STAB` entry is dropped; `n_strx` past `strsize` asserts.

- [ ] **Step 5: Sorting, ties, and the upper clamp.** Sort by `(addr, name)` so ties are
  deterministic. `resolve` returns the nearest preceding symbol and `addr − sym`; returns `None` at
  or past `text_end` (`__TEXT` vmaddr+vmsize+slide). Tests: exact hit → offset 0; midpoint →
  correct offset; two aliases at one address → the *same* name on repeated calls; an address past
  `text_end` → `None`, **not** `last+huge`.

- [ ] **Step 6: `format`.** `"{addr:#x} ({name}+{off:#x})"`, `"{addr:#x} ({name})"` at offset 0, and
  bare `"{addr:#x}"` when unresolved. Test all three, and specifically that **the raw address is
  present in every case** — a later assertion elsewhere in the tree may still grep for it.

**Verification:** `cargo test -p retrace-core -- --test-threads=1` green; clippy clean; no VM used;
the module compiles without touching any other crate.

---

### Task 3: Route images, wire the debug CLI, and land the headline gate

**Files:**
- Modify: `crates/retrace-core/src/symbols.rs` (`Symbols::from_snapshot`, main-exe routing)
- Modify: `crates/retrace/src/debug.rs` (pc-printing sites)
- Create: `crates/retrace/tests/symbols_e2e.rs`

- [ ] **Step 1: `Symbols::from_snapshot` for the main executable.** Build `for_image(mem, EXE_BASE)`.
  `EXE_BASE` lives in `retrace-box`; do **not** duplicate the literal — import it, or if the
  dependency direction forbids that, re-export it and say so in a comment. A second copy of a layout
  constant is exactly the drift this codebase avoids.

- [ ] **Step 2: Wire `debug.rs`.** Symbolicate the **pc-bearing** lines only (design open question 2):
  `at ({n}, {k}) pc=…`, `hit {pc:#x}`, `hit watch … (write at {pc:#x})`, `guest crashed: pc=…`.
  Leave `far` alone (a data address whose symbol is usually meaningless) and leave `x <addr>` alone
  (the user already named it). Build the table **once per session, lazily**, not per query.

- [ ] **Step 3: The headline gate `symbols_e2e`.** Record `crashthread`; drive the debug CLI to the
  crash; assert the crash line **contains `_child`**. Per honest-gate discipline the assertion is the
  *name*, never the address — the address printed before M19 too, so asserting on it would pass
  against a no-op. Add a second assertion that the raw address is **still** present alongside it.

- [ ] **Step 4: Verify the gate can fail.** Stub `resolve` to return `None`, confirm `symbols_e2e`
  goes red, restore. A gate never observed failing is not yet a gate — the M18 fast-follow's own test
  is the model, and its lesson is that the interesting failure is the one that *looks like success*.

**Verification:** `cargo test -p retrace --test symbols_e2e -- --test-threads=1` green and verified
red under the stub; `debug_cli`, `watch_cli`, `crashy_cli` still green (they grep addresses that must
survive).

---

### Task 4: dyld as a second image *(droppable — see Scope)*

Sequenced last deliberately: it is the same mechanism at a different slide (M7), so nothing before it
depends on it, and dropping it costs only coverage.

**Files:** modify `crates/retrace-core/src/symbols.rs`; extend `symbols_e2e`.

- [ ] **Step 1: Derive dyld's slide from its own header** — `DYLD_BASE − dyld __TEXT vmaddr` — and
  never hardcode it (R3). Assert the expected value Task 1 Step 3 recorded.
- [ ] **Step 2: Route by the M7 constants.** `EXE_BASE` → main; `DYLD_BASE` → dyld; anything in
  `[SHARED_REGION_START, SHARED_REGION_END)` → **unresolved by construction**, with a comment naming
  the wall (the cache's local-symbol area is on disk, never staged into guest memory).
- [ ] **Step 3: Assert one known dyld symbol** resolves at its expected post-slide address.

**Verification:** a dyld-region pc prints a name; a cache-region pc prints bare hex and does not
panic.

---

### Task 5: The gate, the parked wall, and the two documents

- [ ] **Step 1: Park `cache_symbol_e2e` `#[ignore]`d** at the shared-cache wall, the `#[ignore]`
  reason naming the measurement it owes: cache images carry no `LC_SYMTAB` in the mapped region, and
  the cache's local-symbol area lives in the on-disk cache file that `cache.rs` demand-pages but
  never stages into guest memory, so symbolicating one would reintroduce the external-file dependency
  M6 eliminated. **The `#[ignore]` reason is the primary record** — the README summarises it, this
  plan does not restate it.

- [ ] **Step 2: Run the full chunked gate** per Global Constraints. Reconcile the total against the
  444/1 baseline by **diffing `#[test]` counts file-by-file**, not by trusting a sum. Expected
  movement: +N unit tests in `retrace-core`, +1 `symbols_e2e`, +1 *ignored* (`cache_symbol_e2e`).

- [ ] **Step 3: Both documents, which must not be merged.**
  - **README** — edited **in place**: "What works today" gains symbolicated addresses; "Known
    limits" gains the three honest limits (shared cache, stripped binaries à la `jq`'s 7 symbols,
    mangled Rust names) and the gate line's new counts.
  - **`docs/status-log.md`** — **append** a new `## Status: M19-symbols` section. Never rewrite an
    earlier section; if an older claim is now wrong, leave it standing with a forward pointer.
  - **`CLAUDE.md`** — only if a statement in it became false. M19 touches no invariant it documents,
    so the expected edit is **none**; do not add a third copy of the README's content.

**Verification:** gate green with exactly one *new* ignored test, both documents updated, `git status`
clean apart from intended files.

---

## Self-Review

Before declaring M19 done, confirm each of these — they are the failure modes this plan was shaped
to prevent:

1. **`TRACE_MAGIC` is still `RT\x00\x08`** and no `Event` variant changed. If either moved, M4/M5
   were misread and the milestone took a format break it did not need.
2. **No file under `record_box` / `ReplaySession::advance` / `Box_::run()` was modified.** If one was,
   symbolication leaked below the presentation layer and the determinism argument no longer holds.
3. **`verify_thread` still has exactly seven call sites** and `mirror_delivery`'s inline eighth is
   intact. M19 should not have touched them; confirm it did not.
4. **A stripped binary prints hex and does not panic.** Run the debug CLI against a `jq` recording if
   one is available; `jq`'s 7 symbols make it the natural adversary.
5. **An unresolved address prints the bare address, never a nearest guess.** The clamp at `text_end`
   is what enforces this; confirm by resolving an address deliberately past it.
6. **The parked gate is parked at a wall that was actually reached**, not at one imagined. The
   shared-cache limit was measured (M7), not assumed.
